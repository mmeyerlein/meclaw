//! Ducking the model while the caller talks over it (GH #1055).
//!
//! # Why the connection does this and not the model
//!
//! A live duplex model stops talking on its own when the caller cuts in — it
//! "hears while it speaks and holds back" (S0, `st-barge2`/`st-barge3`). How
//! long it takes is the model's business, and on a telephone line it was
//! measured at 0.12 to 1.52 s between the caller's first word and the
//! companion falling silent (five calls, quiet window). The provider paces its
//! output in real time — the lead over the wall clock is one chunk, about
//! 100 ms (S0 a2) — so there is no buffer to empty on this side: what the
//! caller goes on hearing is the model still speaking. The transcript-based
//! barge-in of [`crate::voice::live_turns`] could not help either: the
//! caller's transcript arrives after the model has already stopped, and in
//! those five calls it never fired.
//!
//! So the one place that hears both voices at once — this connection, which
//! carries the caller's frames in and the model's chunks out — decides on the
//! audio itself: when the caller's frames stay above a level for `onset_ms`
//! while the model's chunks are voiced, the model's chunks are replaced by
//! silence of the same length until the caller has been quiet again for
//! `release_ms`. Silence and not nothing: the edge behind the socket plays a
//! stream, and a gap is what a playback buffer stutters on (see
//! `connection.rs`, *Outbound framing*).
//!
//! # What it does not do
//!
//! It does not tell the model anything and it does not end a spoken section.
//! The model goes on deciding for itself whether the caller interrupted or
//! only said "mhm"; a backchannel costs the caller a few hundred milliseconds
//! of the model's words and nothing else, and a real interruption is silent
//! here at once and silent at the model a moment later. A provider that has
//! nothing to say sends silence, and silence is never ducked: the gate closes
//! only on a model that is audibly speaking.
//!
//! Everything is counted in AUDIO milliseconds — frame lengths, not a wall
//! clock — so the gate is a pure function of the two streams and its locks
//! need no timer.

/// The knobs, read from `params.duplex` by the factory. `onset_ms == 0` is
/// off, which is what every cell has unless its params ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuckParams {
    /// How long the caller must stay above the level, while the model is
    /// voiced, before the model is ducked. `0` switches ducking off.
    pub onset_ms: u64,
    /// How long the caller must stay below the level before the model is
    /// heard again.
    pub release_ms: u64,
    /// The level a frame must reach to count as voice, in dBFS (RMS of the
    /// caller's frame and of the model's chunk alike).
    pub level_dbfs: f64,
    /// How far below the model's own level the caller may be and still close
    /// the gate, in dB: a caller frame counts only when its RMS reaches the
    /// model's RMS over the last 300 ms minus this margin. What a line hands
    /// back of the model's own voice lies further below it than this and
    /// never ducks the model.
    pub echo_margin_db: f64,
}

/// Shipped values (GH #1055): off, and the two numbers a colony that switches
/// it on starts from.
pub const DEFAULT_BARGE_DUCK_MS: u64 = 0;
/// See [`DuckParams::release_ms`].
pub const DEFAULT_BARGE_RELEASE_MS: u64 = 300;
/// See [`DuckParams::level_dbfs`]. Telephone speech sits around -20 to
/// -30 dBFS RMS; line noise and handset echo well below -40.
pub const DEFAULT_BARGE_LEVEL_DBFS: f64 = -35.0;
/// See [`DuckParams::echo_margin_db`]. An unbalanced telephone hybrid returns
/// the far end 6 to 12 dB down, a handset's residual echo far lower; a caller
/// talking over the model sits within a few dB of it. Ten keeps an echo 12 dB
/// down out and a caller 6 dB under the model in.
pub const DEFAULT_BARGE_ECHO_MARGIN_DB: f64 = 10.0;

impl Default for DuckParams {
    fn default() -> Self {
        Self {
            onset_ms: DEFAULT_BARGE_DUCK_MS,
            release_ms: DEFAULT_BARGE_RELEASE_MS,
            level_dbfs: DEFAULT_BARGE_LEVEL_DBFS,
            echo_margin_db: DEFAULT_BARGE_ECHO_MARGIN_DB,
        }
    }
}

/// How long the caller may pause inside one utterance and still be one run.
/// Two 20 ms frames of a stop consonant must not reset the onset.
const CALLER_HANGOVER_MS: u64 = 100;

/// How long the model may be quiet and still count as speaking: the pause
/// between two words is not the end of a sentence.
const MODEL_HANGOVER_MS: u64 = 300;

/// The gate of one connection.
#[derive(Debug)]
pub struct Duck {
    params: DuckParams,
    /// Linear amplitude of `level_dbfs`, full scale 32 767.
    level: f64,
    /// `10^(-echo_margin_db / 20)`: a caller frame counts against a voiced
    /// model only at or above the model's RMS times this.
    echo_factor: f64,
    /// The model's last chunks as (milliseconds, sum of squares, samples),
    /// back to [`MODEL_HANGOVER_MS`]: the window its level is read over.
    model_window: std::collections::VecDeque<(u64, f64, usize)>,
    /// Sample rate of both directions (one format, see `run_duplex`).
    rate: u32,
    caller_voiced_ms: u64,
    caller_quiet_ms: u64,
    /// Audio milliseconds since the model last produced a voiced chunk;
    /// `None` before it ever did.
    model_quiet_ms: Option<u64>,
    closed: bool,
    /// How often the gate closed, for the connection's close log.
    pub ducked: u32,
}

impl Duck {
    /// A gate for one connection at `rate` (PCM16 mono).
    pub fn new(params: DuckParams, rate: u32) -> Self {
        Self {
            params,
            level: linear(params.level_dbfs),
            echo_factor: 10f64.powf(-params.echo_margin_db / 20.0),
            model_window: std::collections::VecDeque::new(),
            rate: rate.max(1),
            caller_voiced_ms: 0,
            caller_quiet_ms: 0,
            model_quiet_ms: None,
            closed: false,
            ducked: 0,
        }
    }

    /// Whether ducking is switched on at all.
    pub fn enabled(&self) -> bool {
        self.params.onset_ms > 0
    }

    /// Whether the model is ducked right now.
    pub fn closed(&self) -> bool {
        self.closed
    }

    fn ms_of(&self, bytes: usize) -> u64 {
        (bytes as u64 / 2) * 1000 / u64::from(self.rate)
    }

    fn model_voiced(&self) -> bool {
        matches!(self.model_quiet_ms, Some(q) if q < MODEL_HANGOVER_MS)
    }

    /// The model's RMS over its last [`MODEL_HANGOVER_MS`] of audio.
    fn model_level(&self) -> f64 {
        let (sum, n) = self
            .model_window
            .iter()
            .fold((0.0, 0usize), |(s, n), (_, q, k)| (s + q, n + k));
        if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
    }

    /// One frame of the caller. Returns `Some(true)` when this frame closed
    /// the gate, `Some(false)` when it opened it, `None` otherwise.
    pub fn caller(&mut self, frame: &[u8]) -> Option<bool> {
        if !self.enabled() {
            return None;
        }
        let ms = self.ms_of(frame.len());
        let level = rms(frame);
        // While the gate is open, a frame counts only when it is not what the
        // line hands back of the model: the review measured nothing, but an
        // unbalanced hybrid returns the far end 6-12 dB down, above -35 dBFS
        // for a model at -20 (GH #1055 R4). Once the gate is closed the model
        // is silence on the line and its echo with it, so the release reads
        // the level alone.
        let voiced =
            level >= self.level && (self.closed || level >= self.model_level() * self.echo_factor);
        if voiced {
            self.caller_voiced_ms += ms;
            self.caller_quiet_ms = 0;
        } else {
            self.caller_quiet_ms += ms;
            if self.caller_quiet_ms >= CALLER_HANGOVER_MS {
                self.caller_voiced_ms = 0;
            }
        }
        if !self.closed && self.caller_voiced_ms >= self.params.onset_ms && self.model_voiced() {
            self.closed = true;
            self.ducked = self.ducked.saturating_add(1);
            return Some(true);
        }
        if self.closed && self.caller_quiet_ms >= self.params.release_ms {
            self.closed = false;
            return Some(false);
        }
        None
    }

    /// One chunk of the model, on its way out. Read for its level first, then
    /// replaced by silence of the same length while the gate is closed.
    pub fn model(&mut self, chunk: &mut [u8]) {
        if !self.enabled() {
            return;
        }
        let ms = self.ms_of(chunk.len());
        let (sum, n) = sum_of_squares(chunk);
        self.model_window.push_back((ms, sum, n));
        let mut held: u64 = self.model_window.iter().map(|(m, _, _)| m).sum();
        while held > MODEL_HANGOVER_MS {
            let Some((front, _, _)) = self.model_window.front().copied() else {
                break;
            };
            if held - front < MODEL_HANGOVER_MS {
                break;
            }
            held -= front;
            self.model_window.pop_front();
        }
        // RMS, as on the caller's side, not peak: one click in a chunk of
        // silence is not the model speaking (GH #1055 R9).
        if rms(chunk) >= self.level {
            self.model_quiet_ms = Some(0);
        } else if let Some(q) = self.model_quiet_ms.as_mut() {
            *q += ms;
        }
        if self.closed {
            chunk.fill(0);
        }
    }
}

/// Linear amplitude of a level in dBFS, full scale 32 767.
pub(crate) fn linear(level_dbfs: f64) -> f64 {
    32_767.0 * 10f64.powf(level_dbfs / 20.0)
}

/// Sum of squares and sample count of PCM16 LE samples; an odd trailing byte
/// is ignored.
fn sum_of_squares(pcm: &[u8]) -> (f64, usize) {
    let (samples, _) = pcm.as_chunks::<2>();
    let sum = samples
        .iter()
        .map(|s| {
            let v = f64::from(i16::from_le_bytes(*s));
            v * v
        })
        .sum();
    (sum, samples.len())
}

/// RMS of PCM16 LE samples; an odd trailing byte is ignored.
fn rms(pcm: &[u8]) -> f64 {
    let (sum, n) = sum_of_squares(pcm);
    if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
}

/// Where the first audible 20 ms of a chunk begins, in bytes: the first
/// window whose RMS reaches `level` (linear, see [`linear`]). `None` for a
/// chunk that is silence throughout.
///
/// The same test the gate puts the caller's frames to, on the same 20 ms the
/// wire recommends, so a chunk a live model streams as "silence" (S0: a
/// continuous channel, silence included, in 100 ms chunks) is told from the
/// first syllable of its greeting to within one frame (GH #1055 R1).
pub(crate) fn first_audible(chunk: &[u8], rate: u32, level: f64) -> Option<usize> {
    let window = ((rate.max(50) / 50) as usize) * 2;
    let mut at = 0;
    while at < chunk.len() {
        let end = (at + window).min(chunk.len());
        if rms(&chunk[at..end]) >= level {
            return Some(at);
        }
        at = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// 20 ms of a constant sample at 16 kHz.
    fn frame(v: i16) -> Vec<u8> {
        (0..320).flat_map(|_| v.to_le_bytes()).collect()
    }

    fn on() -> DuckParams {
        DuckParams {
            onset_ms: 160,
            release_ms: 300,
            level_dbfs: -35.0,
            echo_margin_db: DEFAULT_BARGE_ECHO_MARGIN_DB,
        }
    }

    /// `v` attenuated by `db` decibels.
    fn down(v: i16, db: f64) -> i16 {
        (f64::from(v) * 10f64.powf(-db / 20.0)).round() as i16
    }

    #[test]
    fn gh1055_an_echo_12_db_under_the_model_does_not_duck_it() {
        let mut d = Duck::new(on(), RATE);
        // The model speaks; the line hands it back 12 dB down, which is still
        // well above the level (-24 dBFS against -35).
        for _ in 0..50 {
            let mut out = frame(LOUD);
            d.model(&mut out);
            assert_eq!(d.caller(&frame(down(LOUD, 12.0))), None, "its own echo");
            assert_eq!(out, frame(LOUD), "the model is heard");
        }
        assert!(!d.closed());
        assert_eq!(d.ducked, 0);
    }

    #[test]
    fn gh1055_a_caller_6_db_under_the_model_still_ducks_it() {
        let mut d = Duck::new(on(), RATE);
        let mut closed = None;
        for i in 0..8 {
            let mut out = frame(LOUD);
            d.model(&mut out);
            if let Some(c) = d.caller(&frame(down(LOUD, 6.0))) {
                closed = Some((i, c));
            }
        }
        assert_eq!(closed, Some((7, true)), "160 ms of a caller 6 dB down");
    }

    #[test]
    fn gh1055_a_click_in_a_silent_chunk_does_not_count_as_the_model_speaking() {
        let mut d = Duck::new(on(), RATE);
        // One loud sample in 20 ms of silence: its peak is -12 dBFS, its RMS
        // about -37 dBFS, under the level.
        let mut click = frame(0);
        click[0..2].copy_from_slice(&LOUD.to_le_bytes());
        d.model(&mut click);
        for _ in 0..20 {
            assert_eq!(d.caller(&frame(LOUD)), None, "nothing to duck");
        }
    }

    /// Speech level (about -12 dBFS) and line noise (about -60 dBFS).
    const LOUD: i16 = 8_000;
    const QUIET: i16 = 30;

    #[test]
    fn gh1055_off_by_default_touches_nothing() {
        let mut d = Duck::new(DuckParams::default(), RATE);
        assert!(!d.enabled());
        let mut out = frame(LOUD);
        d.model(&mut out);
        for _ in 0..50 {
            assert_eq!(d.caller(&frame(LOUD)), None);
        }
        let mut out = frame(LOUD);
        d.model(&mut out);
        assert_eq!(
            out,
            frame(LOUD),
            "a cell without the knob sends the model as it came"
        );
    }

    #[test]
    fn gh1055_the_caller_talking_over_a_speaking_model_ducks_it_after_the_onset() {
        let mut d = Duck::new(on(), RATE);
        let mut out = frame(LOUD);
        d.model(&mut out);
        // 140 ms: still under the onset, the model is heard.
        for _ in 0..7 {
            assert_eq!(d.caller(&frame(LOUD)), None);
        }
        let mut out = frame(LOUD);
        d.model(&mut out);
        assert_eq!(out, frame(LOUD));
        // The eighth frame reaches 160 ms and closes the gate.
        assert_eq!(d.caller(&frame(LOUD)), Some(true));
        let mut out = frame(LOUD);
        d.model(&mut out);
        assert!(
            out.iter().all(|b| *b == 0),
            "ducked: silence of the same length"
        );
        assert_eq!(out.len(), 640);
        assert_eq!(d.ducked, 1);
    }

    #[test]
    fn gh1055_a_silent_model_is_never_ducked() {
        let mut d = Duck::new(on(), RATE);
        // The provider streams silence while nobody speaks.
        let mut out = frame(0);
        d.model(&mut out);
        for _ in 0..30 {
            assert_eq!(d.caller(&frame(LOUD)), None, "nothing to cut");
        }
        assert!(!d.closed());
    }

    #[test]
    fn gh1055_a_model_quiet_longer_than_its_hangover_counts_as_silent() {
        let mut d = Duck::new(on(), RATE);
        let mut out = frame(LOUD);
        d.model(&mut out);
        // 300 ms of silence after the last voiced chunk: the sentence is over.
        for _ in 0..15 {
            let mut s = frame(0);
            d.model(&mut s);
        }
        for _ in 0..10 {
            assert_eq!(d.caller(&frame(LOUD)), None);
        }
    }

    #[test]
    fn gh1055_the_model_is_heard_again_after_the_release() {
        let mut d = Duck::new(on(), RATE);
        let mut out = frame(LOUD);
        d.model(&mut out);
        for _ in 0..8 {
            d.caller(&frame(LOUD));
        }
        assert!(d.closed());
        // 280 ms quiet: still ducked.
        for _ in 0..14 {
            assert_eq!(d.caller(&frame(QUIET)), None);
        }
        assert!(d.closed());
        // 300 ms: open again, and the model's next chunk passes untouched.
        assert_eq!(d.caller(&frame(QUIET)), Some(false));
        let mut out = frame(LOUD);
        d.model(&mut out);
        assert_eq!(out, frame(LOUD));
    }

    #[test]
    fn gh1055_a_short_pause_inside_the_callers_words_does_not_reset_the_onset() {
        let mut d = Duck::new(on(), RATE);
        let mut out = frame(LOUD);
        d.model(&mut out);
        for _ in 0..4 {
            d.caller(&frame(LOUD));
        }
        // 40 ms of a stop consonant.
        d.caller(&frame(QUIET));
        d.caller(&frame(QUIET));
        for _ in 0..3 {
            assert_eq!(d.caller(&frame(LOUD)), None);
        }
        assert_eq!(
            d.caller(&frame(LOUD)),
            Some(true),
            "80 + 80 ms voiced closes it"
        );
    }

    #[test]
    fn gh1055_line_noise_below_the_level_never_ducks() {
        let mut d = Duck::new(on(), RATE);
        let mut out = frame(LOUD);
        d.model(&mut out);
        for _ in 0..50 {
            assert_eq!(d.caller(&frame(QUIET)), None);
        }
    }
}
