//! The park buffer of a session opened while the line rings (GH #1055, the
//! three findings its review left open), and the greeting played to its end.
//!
//! A live model streams one continuous channel, silence included. While its
//! session is parked the cell keeps what the model said for the caller who
//! picks up. Three things were wrong with what it kept:
//!
//! - every chunk after the first audible one was kept, the silence the model
//!   streams until the pickup included, so the caller got the whole ring as
//!   silence in front of every later word;
//! - the cap on what is kept counted that silence too, so a ring longer than
//!   the cap pushed the greeting out of the buffer;
//! - a session whose provider had already given up while it was parked was
//!   adopted like a running one.
//!
//! The locks here measure what the caller HEARS, as an order of frames. None
//! of them reads a clock: the park buffer counts audio, not time, so a ring of
//! ten seconds is ten seconds of audio the model sent, however fast the test
//! sends it.
//!
//! The second half of the file is the greeting itself: it plays to its end
//! whatever the line carries, and only the caller's own words may cut it.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{DeadHow, FakeSession, Live};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_cells::voice::duck::DuckParams;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::voice_client::VoiceClient;
use meclaw_testing::wait;

/// A line fact as the switch posts it: `hop.route` and `hop.call_uuid`, an
/// empty body.
fn line_fact(route: &str, call: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    hop.insert("call_uuid".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

fn off() -> DuckParams {
    DuckParams::default()
}

/// 20 ms at 16 kHz of one constant sample: what the caller gets per frame.
fn frame(v: i16) -> Vec<u8> {
    (0..320).flat_map(|_| v.to_le_bytes()).collect()
}

/// 100 ms at 16 kHz of one constant sample: the chunk a live model streams.
fn chunk(v: i16) -> Vec<u8> {
    (0..1_600).flat_map(|_| v.to_le_bytes()).collect()
}

/// The greeting's sample, the one of a second phrase said into the ring, the
/// one of a word said after the pickup, and the caller's line noise.
const GREET: i16 = 8_000;
const SECOND: i16 = 10_000;
const LATER: i16 = 12_000;
const NOISE: i16 = 6_000;

/// The params every test here boots with: frames of 20 ms on the way out.
fn params(extra: Value) -> Value {
    let mut raw = json!({"default_mode": "auto", "audio_out_frame_ms": 20});
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    duplex_cell::duplex_params(raw)
}

/// The model says `chunks` chunks of `sample`.
async fn model_says(session: &FakeSession, sample: i16, chunks: usize) {
    for _ in 0..chunks {
        session
            .audio_out
            .send(chunk(sample))
            .await
            .expect("the session takes the model's audio");
    }
}

/// Every chunk the model sent into the ring has left its queue.
///
/// The park pump takes a chunk and files it in the same step, and it looks at
/// the adoption only in a later one, so an empty queue is "all of it is
/// parked" and not "most of it".
async fn all_of_it_is_parked(session: &FakeSession) {
    wait::until(
        "the parked session's audio queue is taken up by the park pump",
        || {
            let empty = session.audio_out.capacity() == session.audio_out.max_capacity();
            async move { empty }
        },
    )
    .await;
}

/// What reached the caller, in order.
#[derive(Debug, Default)]
struct Heard {
    /// The sample of every audio frame.
    samples: Vec<i16>,
    /// Every text frame.
    texts: Vec<Value>,
}

impl Heard {
    /// How many frames carried `sample`.
    fn frames_of(&self, sample: i16) -> usize {
        self.samples.iter().filter(|s| **s == sample).count()
    }

    /// No section was cancelled on the wire: `speak_end` is what the telephony
    /// side turns into a break of the playback.
    fn assert_nothing_was_cancelled(&self) {
        for t in &self.texts {
            assert_ne!(
                t["type"],
                json!("speak_end"),
                "a section ended on the wire while the greeting played: {t}"
            );
        }
    }
}

/// The one sample a frame is made of. A frame mixed of two is a frame the
/// cell cut where the model did not.
fn sample_of(audio: &[u8]) -> i16 {
    assert_eq!(audio.len(), 640, "a frame of 20 ms");
    let first = i16::from_le_bytes([audio[0], audio[1]]);
    assert!(
        audio
            .as_chunks::<2>()
            .0
            .iter()
            .all(|s| i16::from_le_bytes(*s) == first),
        "a frame mixed of two sounds reached the caller"
    );
    first
}

/// The next audio frame's sample, skipping text frames.
async fn next_sample(client: &mut VoiceClient) -> i16 {
    loop {
        let f = client
            .next_frame(duplex_cell::marker())
            .await
            .expect("HANG: the caller gets its next frame");
        if let Some(audio) = f.as_audio() {
            return sample_of(audio);
        }
    }
}

/// Everything the caller gets up to, and without, the first frame of `stop`.
async fn heard_until(client: &mut VoiceClient, stop: i16) -> Heard {
    let mut heard = Heard::default();
    loop {
        let f = client
            .next_frame(duplex_cell::marker())
            .await
            .expect("HANG: the word the model said after the pickup reaches the caller");
        if let Some(audio) = f.as_audio() {
            let s = sample_of(audio);
            if s == stop {
                return heard;
            }
            heard.samples.push(s);
        } else if let Some(text) = f.as_text() {
            heard.texts.push(text.clone());
        }
    }
}

/// The caller's line carries `frames` frames of noise, and the model is fed
/// every one of them: the noise is on the line before the test goes on.
async fn line_noise_reaches_the_model(client: &mut VoiceClient, live: &mut Live, frames: usize) {
    for _ in 0..frames {
        client
            .send_audio(&frame(NOISE))
            .await
            .expect("the socket takes the line's audio");
    }
    let mut fed = 0usize;
    while fed < frames {
        let f = tokio::time::timeout(duplex_cell::marker(), live.audio.recv())
            .await
            .expect("HANG: the line's noise reaches the model")
            .expect("the provider is running");
        // What the ring fed the model before the pickup is silence.
        if f == frame(NOISE) {
            fed += 1;
        } else {
            assert!(
                f.iter().all(|b| *b == 0),
                "the model is fed the ring's silence and the line, nothing else"
            );
        }
    }
}

/// C1. The model greets into the ring and then streams ten seconds of silence
/// until the pickup. After the pickup it streams 300 ms more and says a word.
///
/// The caller hears the greeting and then the word. The silence between the
/// two is at most what the model streamed AFTER the pickup: nothing of the
/// ring is queued in front of the word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_a_word_after_a_long_ring_is_heard_without_backlog() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-c1")).await;
    let session = live.session().await;
    model_says(&session, GREET, 10).await;
    model_says(&session, 0, 100).await;
    all_of_it_is_parked(&session).await;

    let (mut client, hello) = live
        .connect("session=call-c1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    // The first frame is the proof of the adoption: what the model says from
    // here on, it says after the pickup.
    assert_eq!(
        next_sample(&mut client).await,
        GREET,
        "the greeting first, at once"
    );
    model_says(&session, 0, 3).await;
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.frames_of(GREET) + 1,
        50,
        "the greeting whole and once"
    );
    let silence_ms = heard.frames_of(0) * 20;
    assert!(
        silence_ms <= 300,
        "the ring is queued in front of the word the model said after the pickup: \
         {silence_ms} ms of silence between the greeting and the word, the model \
         streamed 300 ms after the pickup"
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one call, one model"
    );
}

/// The cap. What is kept of a parked model is capped in audible audio. A ring
/// whose silence alone is longer than the cap leaves the greeting where it
/// was: the caller hears all of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_a_ring_longer_than_the_park_cap_still_plays_the_whole_greeting() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-cap")).await;
    let session = live.session().await;
    // One second of greeting, then sixteen of silence: past the 15 s the park
    // buffer keeps.
    model_says(&session, GREET, 10).await;
    model_says(&session, 0, 160).await;
    all_of_it_is_parked(&session).await;

    let (mut client, _hello) = live
        .connect("session=call-cap&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(
        next_sample(&mut client).await,
        GREET,
        "the greeting first: the ring's silence did not push it out of the park buffer"
    );
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.frames_of(GREET) + 1,
        50,
        "the greeting whole after a ring longer than the cap"
    );
    assert!(
        heard.samples.iter().all(|s| *s == GREET || *s == 0),
        "only the greeting and silence before the later word"
    );
}

/// C1, the other half. The model says two phrases into the ring with a pause
/// between them, and then streams silence until the pickup.
///
/// The pause between the phrases is the model's own and is kept whole. The
/// silence after the last phrase is dropped at the adoption: the word the
/// model says after the pickup follows the second phrase directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_trailing_silence_is_dropped_at_the_adoption() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-tail")).await;
    let session = live.session().await;
    model_says(&session, GREET, 2).await;
    model_says(&session, 0, 3).await;
    model_says(&session, SECOND, 2).await;
    model_says(&session, 0, 20).await;
    all_of_it_is_parked(&session).await;

    let (mut client, _hello) = live
        .connect("session=call-tail&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(next_sample(&mut client).await, GREET, "the greeting first");
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    let mut all = vec![GREET];
    all.extend(&heard.samples);
    let second_at = all
        .iter()
        .position(|s| *s == SECOND)
        .expect("the second phrase reaches the caller");
    let second_end = all
        .iter()
        .rposition(|s| *s == SECOND)
        .expect("the second phrase reaches the caller");
    let pause = all[..second_at].iter().filter(|s| **s == 0).count();
    let tail = all[second_end + 1..].len();
    assert_eq!(
        all.iter().filter(|s| **s == GREET).count(),
        10,
        "the first phrase whole"
    );
    assert_eq!(
        pause * 20,
        300,
        "the model's own pause between its phrases is kept as it was"
    );
    assert_eq!(
        all.iter().filter(|s| **s == SECOND).count(),
        10,
        "the second phrase whole"
    );
    assert_eq!(
        tail * 20,
        0,
        "the silence the model streamed after its last phrase, until the pickup, \
         is dropped at the adoption"
    );
}

/// M1. The provider of the early session gave up while the line rang: its
/// handshake failed, or it ran and ended. The media connection does not adopt
/// what is left of it. It opens a fresh session as if the line had never rung,
/// the caller hears that one's greeting and no other, and the call's last
/// `usage` says its total may be short.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_a_dead_early_session_is_replaced_and_greets_once() {
    for (how, call) in [(DeadHow::Refused, "call-m1a"), (DeadHow::Over, "call-m1b")] {
        let raw = params(json!({"emit_usage": true}));
        let (mut live, early) = duplex_cell::boot_edge_dead_first(raw, 60_000, off(), how).await;

        live.send(line_fact("call_ringing", call)).await;
        early.first_run_is_over().await;
        assert_eq!(
            early.opened(),
            1,
            "{how:?}: the ring opened the early session"
        );

        let (mut client, hello) = live
            .connect(&format!("session={call}&sample_rate=16000&mode=auto"))
            .await;
        assert_eq!(hello["duplex"], true, "{how:?}: got {hello}");
        let fresh = tokio::time::timeout(duplex_cell::marker(), live.sessions.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "HANG: {how:?}: no fresh session -- the media connection adopted an \
                     early session whose provider was already gone"
                )
            })
            .expect("the provider is running");
        assert_eq!(early.opened(), 2, "{how:?}: one dead, one fresh");

        model_says(&fresh, GREET, 1).await;
        model_says(&fresh, LATER, 1).await;
        let heard = heard_until(&mut client, LATER).await;
        assert_eq!(
            heard.samples,
            vec![GREET; 5],
            "{how:?}: the fresh session's greeting, once, and nothing else"
        );

        fresh
            .say(DuplexEvent::Closed {
                reason: "close_requested".to_string(),
                usage_seconds: 3.0,
            })
            .await;
        drop(client);
        let e = loop {
            let e = live.emission("usage").await;
            if duplex_cell::header(&e, "final") == Some(&json!(true)) {
                break e;
            }
        };
        assert_eq!(
            duplex_cell::header(&e, "seconds"),
            Some(&json!(3.0)),
            "{how:?}: {e}"
        );
        assert_eq!(
            duplex_cell::header(&e, "complete"),
            Some(&json!(false)),
            "{how:?}: the early session was opened and its close never came: {e}"
        );
    }
}

/// The greeting plays to its end over line noise. The session opened while
/// the line rang, the caller picks up in the middle of the greeting, and the
/// line carries noise at the level of speech that the provider transcribes
/// nothing of. Every frame of the greeting reaches the caller as the model
/// said it, and no section is cancelled on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_the_greeting_plays_to_its_end_over_line_noise() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-g1")).await;
    let session = live.session().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is your companion.".to_string(),
            start_ms: 0,
            end_ms: 1_000,
        })
        .await;
    model_says(&session, GREET, 5).await;
    all_of_it_is_parked(&session).await;

    let (mut client, _hello) = live
        .connect("session=call-g1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(next_sample(&mut client).await, GREET, "the greeting first");

    line_noise_reaches_the_model(&mut client, &mut live, 10).await;
    model_says(&session, GREET, 5).await;
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.samples,
        vec![GREET; 49],
        "the greeting to its end, unchanged, over the noise on the line"
    );
    heard.assert_nothing_was_cancelled();
}

/// The investigation of the greeting that was cut on a call (the provider's
/// transcript whole, the audio not): the same call without a ring, as it ran
/// that day. Noise on the line BEFORE the model greets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_noise_before_the_greeting_cuts_nothing_of_it() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 0, off()).await;
    let (mut client, _hello) = live
        .connect("session=call-g2&sample_rate=16000&mode=auto")
        .await;
    let session = live.session().await;

    line_noise_reaches_the_model(&mut client, &mut live, 10).await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is your companion.".to_string(),
            start_ms: 0,
            end_ms: 1_000,
        })
        .await;
    model_says(&session, GREET, 10).await;
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.samples,
        vec![GREET; 50],
        "the greeting whole after noise on the line"
    );
    heard.assert_nothing_was_cancelled();
}

/// The same call, noise on the line WHILE the model greets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_noise_during_the_greeting_without_a_ring_cuts_nothing_of_it() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 0, off()).await;
    let (mut client, _hello) = live
        .connect("session=call-g3&sample_rate=16000&mode=auto")
        .await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is your companion.".to_string(),
            start_ms: 0,
            end_ms: 1_000,
        })
        .await;
    model_says(&session, GREET, 5).await;
    for _ in 0..25 {
        assert_eq!(next_sample(&mut client).await, GREET, "the greeting starts");
    }
    line_noise_reaches_the_model(&mut client, &mut live, 10).await;
    model_says(&session, GREET, 5).await;
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.samples,
        vec![GREET; 25],
        "the rest of the greeting, unchanged, over the noise on the line"
    );
    heard.assert_nothing_was_cancelled();
}

/// The same call, and the provider reports a fragment of the caller WITHOUT
/// WORDS inside the greeting, long enough to be more than a backchannel on
/// the model's clock. That is the turn machine's barge-in. It cuts nothing of
/// the greeting: no section ends on the wire, and the rest of the greeting
/// reaches the caller unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_a_fragment_without_words_does_not_cut_the_greeting() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 0, off()).await;
    let (mut client, _hello) = live
        .connect("session=call-g4&sample_rate=16000&mode=auto")
        .await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is".to_string(),
            start_ms: 0,
            end_ms: 800,
        })
        .await;
    model_says(&session, GREET, 5).await;
    for _ in 0..25 {
        assert_eq!(next_sample(&mut client).await, GREET, "the greeting starts");
    }
    // Two seconds of "the caller" inside the model's block, and not a word.
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: String::new(),
            start_ms: 600,
            end_ms: 2_600,
        })
        .await;
    // The model's next fragment travels the road a cancel would: once the
    // caller's side has it, whatever the wordless fragment set off is on the
    // wire in front of it.
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: " your sentinel companion.".to_string(),
            start_ms: 2_600,
            end_ms: 3_400,
        })
        .await;
    let mut before = Heard::default();
    loop {
        let f = client
            .next_frame(duplex_cell::marker())
            .await
            .expect("HANG: the model's next fragment reaches the caller's side");
        if let Some(audio) = f.as_audio() {
            before.samples.push(sample_of(audio));
        } else if let Some(text) = f.as_text() {
            let last = text["type"] == json!("spoken")
                && text["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("sentinel"));
            before.texts.push(text.clone());
            if last {
                break;
            }
        }
    }
    before.assert_nothing_was_cancelled();
    assert!(
        before.samples.is_empty(),
        "nothing was played that the model did not say: {:?}",
        before.samples
    );

    model_says(&session, GREET, 5).await;
    model_says(&session, LATER, 1).await;
    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.samples,
        vec![GREET; 25],
        "the rest of the greeting, unchanged, after a fragment without words"
    );
    heard.assert_nothing_was_cancelled();
}

/// The next `frames` audio frames the caller gets, and every text frame on
/// the way.
async fn heard_frames(client: &mut VoiceClient, frames: usize) -> Heard {
    let mut heard = Heard::default();
    while heard.samples.len() < frames {
        let f = client
            .next_frame(duplex_cell::marker())
            .await
            .expect("HANG: the caller gets the audio the model sent");
        if let Some(audio) = f.as_audio() {
            heard.samples.push(sample_of(audio));
        } else if let Some(text) = f.as_text() {
            heard.texts.push(text.clone());
        }
    }
    heard
}

/// The gate that ducks the model under a caller talking over it, switched on
/// with the values a colony would start from.
fn ducking() -> DuckParams {
    DuckParams {
        onset_ms: 160,
        release_ms: 300,
        level_dbfs: -35.0,
        echo_margin_db: meclaw_cells::voice::duck::DEFAULT_BARGE_ECHO_MARGIN_DB,
    }
}

/// The greeting over line noise with ducking switched on (`barge_duck_ms`
/// above zero). The session opened while the line rang; the caller picks up
/// and the model is still greeting. The line then carries 200 ms of noise at
/// the level of speech, longer than the gate's onset, and the provider
/// transcribes NOTHING of the caller: nobody spoke.
///
/// Noise is not the caller cutting in. The rest of the greeting reaches the
/// caller as the model said it: not replaced by silence, and no section
/// cancelled on the wire. Only a fragment of the caller's own words may take
/// the greeting away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_line_noise_without_a_word_does_not_mute_the_greeting_when_ducking_is_on() {
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, ducking()).await;

    live.send(line_fact("call_ringing", "call-g5")).await;
    let session = live.session().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is your companion.".to_string(),
            start_ms: 0,
            end_ms: 1_500,
        })
        .await;
    model_says(&session, GREET, 5).await;
    all_of_it_is_parked(&session).await;

    let (mut client, _hello) = live
        .connect("session=call-g5&sample_rate=16000&mode=auto")
        .await;
    // What the model said into the ring, and half a second it says after the
    // pickup: once the caller has that, the model is speaking on this
    // connection, and the gate has heard it speak.
    model_says(&session, GREET, 5).await;
    let start = heard_frames(&mut client, 50).await;
    assert_eq!(start.samples, vec![GREET; 50], "the greeting starts");

    line_noise_reaches_the_model(&mut client, &mut live, 10).await;
    model_says(&session, GREET, 5).await;
    model_says(&session, LATER, 1).await;

    let rest = heard_frames(&mut client, 25).await;
    assert_eq!(
        rest.samples,
        vec![GREET; 25],
        "noise on the line that nobody's words are in took the greeting away"
    );
    rest.assert_nothing_was_cancelled();
    start.assert_nothing_was_cancelled();
    assert_eq!(
        next_sample(&mut client).await,
        LATER,
        "and what the model says after the greeting is heard"
    );
}

/// Only the greeting is parked. The model greets into the ring, is silent for
/// eight seconds and then says a second phrase, still into the ring: words
/// for a caller who is not there yet. Then the caller picks up.
///
/// The caller hears the greeting, whole and at once, and not the second
/// phrase: what a parked model says after its first answer is dropped at the
/// adoption. Nothing of the ring is queued either: the word the model says
/// after the pickup follows the greeting with at most the silence the model
/// streamed after the pickup.
///
/// The provider here sends what a provider sends: audio, and transcript
/// fragments on the model's clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_prewarm_only_the_greeting_is_parked_and_a_second_phrase_into_the_ring_is_dropped()
{
    let mut live = duplex_cell::boot_edge(params(json!({})), 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-d1")).await;
    let session = live.session().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hello, this is your companion.".to_string(),
            start_ms: 0,
            end_ms: 1_000,
        })
        .await;
    model_says(&session, GREET, 10).await;
    model_says(&session, 0, 80).await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: " Are you still there?".to_string(),
            start_ms: 9_000,
            end_ms: 10_000,
        })
        .await;
    model_says(&session, SECOND, 10).await;
    model_says(&session, 0, 5).await;
    all_of_it_is_parked(&session).await;

    let (mut client, hello) = live
        .connect("session=call-d1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    // The first frame is the proof of the adoption: what the model says from
    // here on, it says after the pickup.
    assert_eq!(
        next_sample(&mut client).await,
        GREET,
        "the greeting first, at once"
    );
    model_says(&session, 0, 3).await;
    model_says(&session, LATER, 1).await;

    let heard = heard_until(&mut client, LATER).await;
    assert_eq!(
        heard.frames_of(GREET) + 1,
        50,
        "the greeting whole and once"
    );
    assert_eq!(
        heard.frames_of(SECOND),
        0,
        "the caller heard what the model said into the ring after its greeting"
    );
    let silence_ms = heard.frames_of(0) * 20;
    assert!(
        silence_ms <= 300,
        "the ring is queued behind the greeting: {silence_ms} ms of silence between \
         the greeting and the word the model said after the pickup, the model \
         streamed 300 ms after the pickup"
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one call, one model"
    );
}
