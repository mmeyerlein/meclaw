//! GH #1557: the model's text whose audio never reached the caller is not
//! counted as said.
//!
//! A live model's transcript runs seconds ahead of its audio. Where the audio
//! stops -- the line goes, or the caller cuts in and the model falls silent --
//! the transcript already stands further on, and that overhang was never
//! heard. The `turn` carries only the words the model's speech carried (at
//! most one character per 30 ms of its span, first to last audible window,
//! cut at a word boundary); the rest leaves `messages`, stands as `unsaid` in
//! the header, and the turn is `cut`. Only where something is `unsaid` do the
//! words kept end on the mark "\u{2026}": every reader of the turn, the seed
//! of the next session too, sees that the answer broke off there. A turn the
//! end of the line marks `cut` after its answer was heard keeps no mark.
//!
//! On the fake provider, with the caller's ear as the pace: every chunk of
//! audio is sent once the caller heard the one before, so the connection has
//! counted it before the next event leaves. No clock is a verdict here.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{FakeSession, Live, RATE, boot_echo, boot_fake_timed, speak_msg};
use duplex_cell::{duplex_params, emission_route, header, marker};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::voice_client::VoiceClient;

/// The caller's question.
const ASKED: &str = "What is on tomorrow?";

/// An answer of about ten seconds at a speaking rate of 20 characters a
/// second -- far more than the audio that goes out in the cut cases.
const LONG_ANSWER: &str = "Tomorrow at ten you have the dentist on the main street, \
     then at noon lunch with your sister at the little place by the river, and in \
     the afternoon the plumber wants to come by to look at the kitchen sink.";

/// One chunk of model audio: 200 ms of loud PCM16 at [`RATE`].
fn chunk() -> Vec<u8> {
    loud(200)
}

/// `ms` of PCM16 at [`RATE`]: loud speech, or silence.
fn pcm(ms: u32, level: i16) -> Vec<u8> {
    let samples = usize::try_from(RATE / 1_000 * ms).unwrap_or(0);
    (0..samples).flat_map(|_| level.to_le_bytes()).collect()
}

fn loud(ms: u32) -> Vec<u8> {
    pcm(ms, 8_000)
}

/// One chunk of speech with its pauses: 160 ms loud, then 160 ms of silence
/// -- half of it audible, the way a voice's pauses, unvoiced sounds and
/// fading ends fall under the level.
fn paused_chunk() -> Vec<u8> {
    [loud(160), pcm(160, 0)].concat()
}

async fn says(session: &FakeSession, speaker: Speaker, text: &str, start_ms: u64, end_ms: u64) {
    session
        .say(DuplexEvent::Transcript {
            speaker,
            delta: text.to_string(),
            start_ms,
            end_ms,
        })
        .await;
}

/// The model's audio goes out chunk by chunk, each once the caller heard the
/// one before; the bytes of it the caller heard so far.
async fn speak(session: &FakeSession, client: &mut VoiceClient, chunks: usize) -> usize {
    speak_as(session, client, chunks, chunk).await
}

/// [`speak`] with chunks made by `make`.
async fn speak_as(
    session: &FakeSession,
    client: &mut VoiceClient,
    chunks: usize,
    make: fn() -> Vec<u8>,
) -> usize {
    let mut bytes = 0;
    for n in 0..chunks {
        session
            .audio_out
            .send(make())
            .await
            .expect("the model's audio goes out");
        let heard = client
            .collect_until(|f| f.as_audio().is_some(), marker())
            .await;
        assert!(
            heard.iter().any(|(_, f)| f.as_audio().is_some()),
            "HANG: the caller never heard chunk {n} within {} s",
            marker().as_secs()
        );
        bytes += heard
            .iter()
            .filter_map(|(_, f)| f.as_audio().map(<[u8]>::len))
            .sum::<usize>();
    }
    bytes
}

/// The caller hears the model's audio to its last byte: `heard` bytes came
/// with the chunks already, the rest of `chunks` chunks comes now. Once the
/// last byte reached the caller, the connection has counted all of it.
async fn hear_all(client: &mut VoiceClient, heard: usize, chunks: usize) {
    hear_all_of(client, heard, chunk().len() * chunks).await;
}

/// [`hear_all`] of `all` bytes.
async fn hear_all_of(client: &mut VoiceClient, mut heard: usize, all: usize) {
    while heard < all {
        let frames = client
            .collect_until(|f| f.as_audio().is_some(), marker())
            .await;
        let got: usize = frames
            .iter()
            .filter_map(|(_, f)| f.as_audio().map(<[u8]>::len))
            .sum();
        assert!(
            got > 0,
            "HANG: the caller heard {heard} of {all} bytes within {} s",
            marker().as_secs()
        );
        heard += got;
    }
}

/// A line whose turns close on the end, a barge-in or the caller's next
/// words only: the gap and the quiet are the hang mark (see the cut pin in
/// `gh_d54g_callseam`), never a verdict.
async fn boot() -> Live {
    let quiet_ms = u64::try_from(marker().as_millis()).unwrap_or(u64::MAX);
    let params = duplex_params(json!({
        "default_mode": "auto",
        "emit_usage": true,
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": RATE,
            "turn_gap_ms": quiet_ms,
            "backchannel_max_ms": 900,
        },
    }));
    boot_fake_timed(params, quiet_ms, quiet_ms.saturating_mul(2)).await
}

fn said_by(emission: &Value, origin: &str) -> String {
    emission
        .get("messages")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter(|m| m.get("origin").and_then(Value::as_str) == Some(origin))
                .filter_map(|m| m.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

async fn next_emission(live: &mut Live, waiting_for: &str) -> Value {
    match tokio::time::timeout(marker(), live.emissions.recv()).await {
        Ok(Some(e)) => e.content,
        Ok(None) => panic!("the emission channel closed while waiting for {waiting_for}"),
        Err(_) => panic!("HANG: no {waiting_for} within {} s", marker().as_secs()),
    }
}

async fn next_turn(live: &mut Live) -> Value {
    loop {
        let e = next_emission(live, "a `turn`").await;
        if emission_route(&e) == Some("turn") {
            return e;
        }
    }
}

/// The model's part is a word-boundary prefix of `answer` of at most
/// `budget` characters that ends on the mark "\u{2026}", and with `unsaid`
/// it is the whole answer.
fn assert_cut_to_heard(turn: &Value, answer: &str, budget: usize) {
    let marked = said_by(turn, "assistant");
    let said = marked
        .strip_suffix('\u{2026}')
        .unwrap_or_else(|| panic!("the heard words end on the mark \u{2026}: {turn}"))
        .to_string();
    assert!(
        !said.is_empty() && !said.contains('\u{2026}'),
        "the heard words stand, the mark once: {turn}"
    );
    let unsaid = header(turn, "unsaid")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the unheard rest stands as `unsaid`: {turn}"))
        .to_string();
    assert!(
        said.chars().count() <= budget,
        "the model's part holds no more than its audio carried ({budget} chars): {said:?}"
    );
    assert!(
        answer.starts_with(&said)
            && (said.is_empty() || answer[said.len()..].starts_with(char::is_whitespace)),
        "the model's part ends at a word boundary of the answer: {said:?}"
    );
    let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(
        [words(&said), words(&unsaid)].concat(),
        words(answer),
        "said and unsaid together are the whole answer: {turn}"
    );
    assert!(
        !said.contains(unsaid.trim()),
        "the overhang is not in the turn's text: {turn}"
    );
    assert_eq!(header(turn, "cut"), Some(&json!(true)), "{turn}");
}

/// R1: the line goes while the model's transcript stands about ten seconds
/// ahead of the one second of audio the caller heard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1557_unheard_text_is_not_said_a_connection_end_keeps_only_the_heard_words() {
    let mut live = boot().await;
    let (mut client, hello) = live.connect("session=gh1557-r1&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    says(&session, Speaker::Assistant, LONG_ANSWER, 1_000, 11_000).await;
    let _ = live.emission("spoken").await;
    speak(&session, &mut client, 5).await;
    client.close().await.expect("the caller hangs up");

    let turn = next_turn(&mut live).await;
    assert_eq!(said_by(&turn, "user"), ASKED, "{turn}");
    // 1 000 ms of speech carry 33 characters.
    assert_cut_to_heard(&turn, LONG_ANSWER, 33);
}

/// R2: the model falls silent after about two seconds of audio because the
/// caller cuts in; its transcript already ran on. The overhang is not in the
/// turn the barge-in closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1557_unheard_text_is_not_said_an_interrupted_answer_loses_its_overhang() {
    let mut live = boot().await;
    let (mut client, hello) = live.connect("session=gh1557-r2&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    says(&session, Speaker::Assistant, LONG_ANSWER, 1_000, 11_000).await;
    let _ = live.emission("spoken").await;
    speak(&session, &mut client, 10).await;
    // The caller cuts in while the model's transcript still runs: a run
    // longer than `backchannel_max_ms` inside the block.
    says(
        &session,
        Speaker::User,
        "Wait, the dentist moved.",
        3_000,
        4_500,
    )
    .await;
    let turn = next_turn(&mut live).await;
    assert_eq!(said_by(&turn, "user"), ASKED, "{turn}");
    // 2 000 ms of speech carry 66 characters.
    assert_cut_to_heard(&turn, LONG_ANSWER, 66);

    // The model's next answer is a block of its own -- it starts after a
    // silence longer than the gap -- and its audio is its own.
    let next_block = 11_000 + u64::try_from(marker().as_millis()).unwrap_or(u64::MAX) + 1_000;
    says(
        &session,
        Speaker::Assistant,
        "Oh, I see.",
        next_block,
        next_block + 600,
    )
    .await;
    speak(&session, &mut client, 5).await;
    client.close().await.expect("the caller hangs up");
    let next = next_turn(&mut live).await;
    // The hang-up comes while that answer's audio is the latest on the line
    // (`spoken_quiet_ms` is the hang mark): the end marks the turn `cut`. Its
    // audio carried all of it, so nothing is unsaid and the text keeps no
    // mark -- the mark stands only where words were not heard.
    assert_eq!(header(&next, "cut"), Some(&json!(true)), "{next}");
    assert_eq!(said_by(&next, "assistant"), "Oh, I see.", "{next}");
    assert_eq!(header(&next, "unsaid"), None, "{next}");
}

/// Counter-check: an answer whose audio went out to its end stays whole --
/// no `cut`, no `unsaid`, no mark -- even where it is barely heard and the
/// turn closes on a path that cuts: its audio runs just above the budget
/// (23 characters at 30 ms each need 690 ms; 800 ms went out), and the
/// caller then cuts in while the transcript's block still runs, so the
/// barge-in measures the answer against its audio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1557_unheard_text_is_not_said_a_whole_answer_stays_whole() {
    const ANSWER: &str = "Nine to six, every day.";
    let mut live = boot().await;
    let (mut client, hello) = live.connect("session=gh1557-whole&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    says(&session, Speaker::Assistant, ANSWER, 1_000, 2_200).await;
    let _ = live.emission("spoken").await;
    let heard = speak(&session, &mut client, 4).await;
    hear_all(&mut client, heard, 4).await;
    // A run longer than `backchannel_max_ms` inside the block: a barge-in.
    says(&session, Speaker::User, "And on Sunday?", 1_500, 3_000).await;
    let turn = next_turn(&mut live).await;
    assert_eq!(said_by(&turn, "user"), ASKED, "{turn}");
    assert_eq!(
        said_by(&turn, "assistant"),
        ANSWER,
        "no mark, nothing cut: {turn}"
    );
    assert_eq!(header(&turn, "cut"), None, "{turn}");
    assert_eq!(header(&turn, "unsaid"), None, "{turn}");
    client.close().await.expect("the caller hangs up");
}

/// An answer as a live model speaks it stays whole: measured on a German
/// answer, 132 characters went out over 7.7 s from its first to its last
/// audible window, 4.8 s of it audible (shares of 0.52 to 0.63 over three
/// recordings). Here 132 characters ride on 24 chunks of speech with its
/// pauses: 7.5 s from first to last audible window, 3.84 s audible -- a share
/// just below the lowest measured, so that counting only the audible windows
/// (128 characters) would cut the answer, and the span (250) does not. The
/// caller hears all of it, then cuts in while the transcript's block still
/// runs: the barge-in measures the answer against its audio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1557_unheard_text_is_not_said_a_measured_answer_stays_whole() {
    const ANSWER: &str = "Tomorrow at ten the dentist, then lunch with your sister by the \
         river, and in the afternoon the plumber comes by to see the kitchen.";
    const CHUNKS: usize = 24;
    assert_eq!(ANSWER.chars().count(), 132, "the measured length");
    let mut live = boot().await;
    let (mut client, hello) = live.connect("session=gh1557-measured&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    says(&session, Speaker::Assistant, ANSWER, 1_000, 8_700).await;
    let _ = live.emission("spoken").await;
    let heard = speak_as(&session, &mut client, CHUNKS, paused_chunk).await;
    hear_all_of(&mut client, heard, paused_chunk().len() * CHUNKS).await;
    // A run longer than `backchannel_max_ms` inside the block: a barge-in.
    says(&session, Speaker::User, "And the plumber?", 2_000, 3_500).await;
    let turn = next_turn(&mut live).await;
    assert_eq!(said_by(&turn, "user"), ASKED, "{turn}");
    assert_eq!(
        said_by(&turn, "assistant"),
        ANSWER,
        "no mark, nothing cut: {turn}"
    );
    assert_eq!(header(&turn, "cut"), None, "{turn}");
    assert_eq!(header(&turn, "unsaid"), None, "{turn}");
    client.close().await.expect("the caller hangs up");
}

/// The loopback (`provider: echo`) mirrors an append as the model's text, and
/// its audio is the echo of the caller's microphone, not that text. Its
/// transcript is not the transcript of its own audio, so no audio measures
/// it: a session of such a provider keeps its mirrored text whole, however
/// little audio went out beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1557_unheard_text_is_not_said_the_loopback_keeps_its_mirrored_text() {
    let mut live = boot_echo(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, hello) = live.connect("session=gh1557-echo&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");

    // 200 ms of loud microphone comes back as the "model's" audio.
    client
        .send_audio(&chunk())
        .await
        .expect("the socket is open");
    let echoed = client
        .collect_until(|f| f.as_audio().is_some(), marker())
        .await;
    assert!(
        echoed.iter().any(|(_, f)| f.as_audio().is_some()),
        "HANG: the loopback echoed nothing within {} s",
        marker().as_secs()
    );

    live.send(speak_msg("gh1557-echo", LONG_ANSWER)).await;
    let _ = live.emission("spoken").await;
    client.close().await.expect("the caller hangs up");

    let turn = next_turn(&mut live).await;
    assert!(
        said_by(&turn, "assistant").contains(LONG_ANSWER),
        "the mirrored text stands whole: {turn}"
    );
    assert_eq!(header(&turn, "unsaid"), None, "{turn}");
}
