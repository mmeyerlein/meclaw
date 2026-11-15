//! GH #1537 -- **the opening answer of a `gpt_live` session ends after the
//! greeting.**
//!
//! Measured on test colonies (0.87.0): the model answered the greeting
//! append with the greeting and then went on, up to about 25 s of an
//! invented first-person story, once with a name for the caller nobody had
//! said -- 3 of 7 calls on freshly born colonies. Nothing on this side
//! carried the text in; the adapter handed all of it to the caller and into
//! the transcript.
//!
//! The promise, in seven pins:
//! - (a) a session that greets: the opening answer may speak until it said
//!   half of the spoken greeting (the quotation in `greeting`, else all of
//!   it) and ends at the next sentence end; what it says after that --
//!   audio, transcript, and with them `spoken` and the turn -- reaches
//!   neither the caller nor the cell, and the model is told once
//!   (`session.instructions.append`) to stop and wait for the caller;
//! - (b) once the caller has spoken, the next answer passes whole, however
//!   many sentences it has, and nothing is appended;
//! - (c) a renewed session (GH #896) greets nobody and is not touched;
//! - (d) the deployed form, an instruction that quotes the sentence to say:
//!   the first sentence of the tail does not pass;
//! - (e) a greeting said as two short sentences ("Hello!" and the rest):
//!   both pass, the tail does not;
//! - (f) a tail run on from the greeting without any sentence end: cut at
//!   twice the spoken greeting, the rest does not pass;
//! - (g) the caller hangs up during the tail: what the model says while the
//!   close is under way does not pass either.
//!
//! Measured with the shipped `gpt_live` adapter against the strict mock
//! socket of `meclaw_testing::mock_gpt_live`: (a) inside a duplex cell, (b)
//! to (g) on the adapter's own channels. Loopback sockets, so real time
//! (development rules § 11): every wait is the named hang marker, every
//! claim is about what arrived, none is about time, and no model is called.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_recovering_with, duplex_params, emission_route, header, marker};
use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts, Speaker,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::io_liveness::IoLivenessMark;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The greeting append without a quotation: all of it is the base.
const GREETING: &str = "Greet the caller now in one sentence, then listen.";

/// The greeting append in its deployed form: an instruction that quotes the
/// sentence to say and goes on after it.
const QUOTING_GREETING: &str = "Say this one sentence in your own words, warmly: \u{201E}Hello, \
                                this is your companion, glad we can talk.\u{201C} Then listen.";

/// What the model says as its greeting.
const SAID_GREETING: &str = "Hello, this is your companion.";

/// A word that stands only in the invented tail, so its absence is
/// readable in any emission.
const TAIL_WORD: &str = "Quillfeather";

/// One 100 ms chunk of PCM16 at the cell's rate.
const CHUNK_BYTES: usize = (duplex_cell::RATE as usize / 10) * 2;

/// A `gpt_live` block against `mock` that greets with `greeting`.
fn gpt_live_params(mock: &MockGptLive, greeting: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": mock.base_url(),
        "instructions": "A test line.",
        "greeting": greeting,
        "sample_rate": duplex_cell::RATE,
    }))
    .expect("the gpt_live block parses")
}

/// `n` chunks of 100 ms of audio, each its own action.
fn audio(n: usize, byte: u8) -> Vec<LiveAction> {
    (0..n)
        .map(|_| LiveAction::SendAudio(vec![byte; CHUNK_BYTES]))
        .collect()
}

/// One transcript fragment of the model.
fn model(delta: &'static str, start_ms: u64, end_ms: u64) -> LiveAction {
    LiveAction::Transcript {
        speaker: "assistant",
        delta,
        start_ms,
        end_ms,
    }
}

/// The `session.instructions.append`s the mock received.
async fn instruction_appends(mock: &MockGptLive) -> Vec<Value> {
    mock.appends()
        .await
        .into_iter()
        .filter(|a| a["type"] == json!("session.instructions.append"))
        .collect()
}

/// Every emission up to and including the call's `final` usage.
async fn until_final(live: &mut Live) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = tokio::time::timeout(marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| {
                let routes: Vec<_> = seen
                    .iter()
                    .map(|e| emission_route(e).unwrap_or_default().to_string())
                    .collect();
                panic!("HANG: no final usage; saw {routes:?}")
            })
            .expect("the handler is running")
            .content;
        let last = emission_route(&e) == Some("usage") && header(&e, "final") == Some(&json!(true));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// Pin (a): the greeting reaches the caller and the cell, the tail reaches
/// neither, and the model is told once to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_the_tail_of_the_opening_answer_reaches_nobody() {
    let mut actions = vec![LiveAction::ExpectAppend {
        kind: "commentary",
        contains: "Greet the caller",
    }];
    // The greeting, one sentence over one second, and its second of audio.
    actions.push(model(SAID_GREETING, 0, 1_000));
    actions.extend(audio(10, 0x11));
    // The invented tail, two more sentences and two seconds of audio.
    actions.push(model(" I just came back from the lake.", 1_000, 2_000));
    actions.push(model(" My bicycle Quillfeather was in it.", 2_000, 3_000));
    actions.extend(audio(20, 0x22));
    // The marker: the adapter told the model to stop. Then the call ends.
    actions.push(LiveAction::ExpectAppend {
        kind: "instructions",
        contains: "",
    });
    actions.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 3.0,
    });
    let mock = MockGptLive::start(LiveScript {
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");
    let provider: Arc<dyn DuplexProvider> = Arc::new(
        GptLiveDuplex::new(gpt_live_params(&mock, GREETING))
            .with_timeouts(ProviderTimeouts::default()),
    );
    let mut live = boot_recovering_with(
        duplex_params(json!({"default_mode": "auto", "emit_usage": true})),
        provider,
        0,
        0,
    )
    .await;
    let (mut client, _hello) = live.connect("session=gh1537-a&mode=auto").await;

    let seen = until_final(&mut live).await;

    // Everything the caller heard, up to the end of the call.
    let mut heard = 0usize;
    while let Ok(frame) = client.next_frame(marker()).await {
        if let Some(bytes) = frame.as_audio() {
            heard += bytes.len();
        }
        if frame.as_close().is_some() {
            break;
        }
    }

    for e in &seen {
        assert!(
            !e.to_string().contains(TAIL_WORD) && !e.to_string().contains("lake"),
            "the tail of the opening answer reaches no lane: {e}"
        );
    }
    assert!(
        seen.iter().any(
            |e| emission_route(e) == Some("spoken") && e.to_string().contains("your companion")
        ),
        "the greeting itself is spoken: {seen:?}"
    );
    assert_eq!(
        heard,
        10 * CHUNK_BYTES,
        "the caller hears the greeting's audio and none of the tail's"
    );
    let stops = instruction_appends(&mock).await;
    assert_eq!(stops.len(), 1, "the model is told once to stop: {stops:?}");
    assert_eq!(stops[0]["delegation_id"], Value::Null, "{stops:?}");
}

/// The adapter's own channels, for pins (b) and (c).
struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    audio: tokio::task::JoinHandle<usize>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

/// One session of the adapter against `mock` that greets with [`GREETING`];
/// a renewal where `renewed`.
fn start(mock: &MockGptLive, renewed: bool) -> Session {
    start_greeting(mock, renewed, GREETING)
}

/// One session of the adapter against `mock` that greets with `greeting`.
fn start_greeting(mock: &MockGptLive, renewed: bool, greeting: &str) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, mut audio_out_rx) = mpsc::channel::<Vec<u8>>(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(gpt_live_params(mock, greeting))
        .with_timeouts(ProviderTimeouts::default());
    let session = DuplexSession {
        audio_in: audio_in_rx,
        audio_out: audio_out_tx,
        events: events_tx,
        control: control_rx,
    };
    let future = if renewed {
        provider.run_renewed_session(provider.format(), session, IoLivenessMark::disabled())
    } else {
        provider.run_session(provider.format(), session, IoLivenessMark::disabled())
    };
    // The audio outlet is read for as long as the session holds it: it ends
    // when the session does.
    let audio = tokio::spawn(async move {
        let mut bytes = 0usize;
        while let Some(chunk) = audio_out_rx.recv().await {
            bytes += chunk.len();
        }
        bytes
    });
    Session {
        audio_in: audio_in_tx,
        events: events_rx,
        control: control_tx,
        audio,
        join: tokio::spawn(future),
    }
}

/// What the model said in a session, and how much audio left it, up to its
/// `Closed`.
async fn said_and_heard(mut session: Session) -> (String, usize) {
    let mut said = String::new();
    loop {
        let event = tokio::time::timeout(marker(), session.events.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: no Closed; the model said so far {said:?}"))
            .expect("the session reports its end before it goes");
        match event {
            DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta,
                ..
            } => said.push_str(&delta),
            DuplexEvent::Closed { .. } => break,
            _ => {}
        }
    }
    drop(session.audio_in);
    drop(session.control);
    let heard = tokio::time::timeout(marker(), session.audio)
        .await
        .expect("HANG: the audio outlet closes with the session")
        .expect("the audio reader ran");
    let _ = tokio::time::timeout(marker(), session.join).await;
    (said, heard)
}

/// Pin (b): once the caller spoke, an answer of three sentences passes
/// whole, and the model is told nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_after_the_caller_spoke_the_answer_passes_whole() {
    let mut actions = vec![LiveAction::ExpectAppend {
        kind: "commentary",
        contains: "Greet the caller",
    }];
    actions.push(model(SAID_GREETING, 0, 1_000));
    actions.extend(audio(10, 0x11));
    actions.push(LiveAction::Transcript {
        speaker: "user",
        delta: "Hi, how was your day?",
        start_ms: 1_500,
        end_ms: 2_500,
    });
    actions.push(model("It was calm.", 3_000, 4_000));
    actions.push(model(" I sorted my notes.", 4_000, 5_000));
    actions.push(model(" The cat Quillfeather slept.", 5_000, 6_000));
    actions.extend(audio(30, 0x33));
    actions.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 6.0,
    });
    let mock = MockGptLive::start(LiveScript {
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");

    let (said, heard) = said_and_heard(start(&mock, false)).await;

    assert_eq!(
        said,
        "Hello, this is your companion.It was calm. I sorted my notes. The cat Quillfeather slept.",
        "every word of the answer after the caller's passes"
    );
    assert_eq!(heard, 40 * CHUNK_BYTES, "every byte of audio passes");
    let stops = instruction_appends(&mock).await;
    assert!(stops.is_empty(), "nothing is appended: {stops:?}");
}

/// Pin (c): a renewed session greets nobody, and what its model says --
/// three sentences before the caller spoke -- passes whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_a_renewed_session_is_not_touched() {
    let mut actions = vec![
        model("Where were we.", 0, 1_000),
        model(" The lake was cold.", 1_000, 2_000),
        model(" My bicycle Quillfeather was in it.", 2_000, 3_000),
    ];
    actions.extend(audio(30, 0x44));
    actions.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 3.0,
    });
    let mock = MockGptLive::start(LiveScript {
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");

    let (said, heard) = said_and_heard(start(&mock, true)).await;

    assert_eq!(
        said, "Where were we. The lake was cold. My bicycle Quillfeather was in it.",
        "a renewal's model is not cut"
    );
    assert_eq!(heard, 30 * CHUNK_BYTES, "every byte of audio passes");
    assert!(
        mock.appends().await.is_empty(),
        "a renewal sends no greeting and no stop"
    );
}

/// The mock for pins (d) and (e): the greeting append, `actions`, the
/// marker that the model was told to stop, the end of the call.
async fn opening_mock(actions: Vec<LiveAction>) -> MockGptLive {
    let mut all = vec![LiveAction::ExpectAppend {
        kind: "commentary",
        contains: "Say this one sentence",
    }];
    all.extend(actions);
    all.push(LiveAction::ExpectAppend {
        kind: "instructions",
        contains: "",
    });
    all.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 3.0,
    });
    MockGptLive::start(LiveScript {
        actions: all,
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds")
}

/// Pin (d): the deployed greeting quotes its sentence inside an instruction
/// that has more sentences of its own; the first sentence of the tail still
/// does not pass, and the model is told once to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_the_quoted_greeting_stops_before_the_first_tail_sentence() {
    let mut actions = vec![model(
        "Hello, this is your companion, glad we can talk.",
        0,
        1_000,
    )];
    actions.extend(audio(10, 0x55));
    actions.push(model(" I just came back from the lake.", 1_000, 2_000));
    actions.push(model(" My bicycle Quillfeather was in it.", 2_000, 3_000));
    actions.extend(audio(20, 0x66));
    let mock = opening_mock(actions).await;

    let (said, heard) = said_and_heard(start_greeting(&mock, false, QUOTING_GREETING)).await;

    assert_eq!(
        said, "Hello, this is your companion, glad we can talk.",
        "the greeting passes, the tail's first sentence does not"
    );
    assert_eq!(heard, 10 * CHUNK_BYTES, "only the greeting's audio passes");
    let stops = instruction_appends(&mock).await;
    assert_eq!(stops.len(), 1, "the model is told once to stop: {stops:?}");
}

/// Pin (e): the model says the greeting as two short sentences; both pass,
/// the tail does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_a_greeting_in_two_short_sentences_passes_whole() {
    let mut actions = vec![
        model("Hello!", 0, 300),
        model(" This is your companion, glad we can talk.", 300, 1_500),
    ];
    actions.extend(audio(15, 0x77));
    actions.push(model(" I just came back from the lake.", 1_500, 2_500));
    actions.extend(audio(10, 0x88));
    let mock = opening_mock(actions).await;

    let (said, heard) = said_and_heard(start_greeting(&mock, false, QUOTING_GREETING)).await;

    assert_eq!(
        said, "Hello! This is your companion, glad we can talk.",
        "both sentences of the greeting pass, the tail does not"
    );
    assert_eq!(heard, 15 * CHUNK_BYTES, "only the greeting's audio passes");
    let stops = instruction_appends(&mock).await;
    assert_eq!(stops.len(), 1, "the model is told once to stop: {stops:?}");
}

/// Pin (f): the model runs its tail on from the greeting without any
/// sentence end; the opening answer is cut at twice the spoken greeting's
/// characters (48 → 96), the rest -- text and the audio past that point --
/// does not pass, and the model is told once to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_a_run_on_tail_is_cut_at_twice_the_greeting() {
    const SAID: &str = "Hello, this is your companion, glad we can talk";
    // 98 characters: the cut at 96 falls after its 49th, half-way along.
    const RUN_ON: &str = ", but listen, I am out on the water right now with my older \
                          bicycle Quillfeather and the fish bite";
    let mut actions = vec![model(SAID, 0, 1_000)];
    actions.extend(audio(10, 0x99));
    actions.push(model(RUN_ON, 1_000, 2_000));
    actions.extend(audio(20, 0xAA));
    let mock = opening_mock(actions).await;

    let (said, heard) = said_and_heard(start_greeting(&mock, false, QUOTING_GREETING)).await;

    let expected: String = format!("{SAID}{RUN_ON}").chars().take(96).collect();
    assert_eq!(said, expected, "the opening answer ends at 96 characters");
    assert!(!said.contains(TAIL_WORD), "{said}");
    assert_eq!(
        heard,
        15 * CHUNK_BYTES,
        "audio passes up to the cut, half-way through the run-on fragment"
    );
    let stops = instruction_appends(&mock).await;
    assert_eq!(stops.len(), 1, "the model is told once to stop: {stops:?}");
}

/// Pin (g): the caller hangs up during the tail. The close is under way, the
/// model still says the rest, and none of it passes: the guard holds while
/// the session waits for the provider's own close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1537_a_close_during_the_tail_passes_none_of_it() {
    let mut actions = vec![LiveAction::ExpectAppend {
        kind: "commentary",
        contains: "Say this one sentence",
    }];
    actions.push(model(
        "Hello, this is your companion, glad we can talk.",
        0,
        1_000,
    ));
    actions.extend(audio(10, 0xBB));
    actions.push(model(" I just came back from the lake.", 1_000, 2_000));
    actions.extend(audio(10, 0xCC));
    // The stop is acknowledged; the test closes the session on that
    // acknowledgement. The mock then stops reading, so it does not answer the
    // `session.close` at once as it otherwise would: like the real service,
    // it still sends what the model says and closes after it. The pause only
    // orders the rest after the close, no claim rests on it.
    actions.push(LiveAction::ExpectAppend {
        kind: "instructions",
        contains: "",
    });
    actions.push(LiveAction::GoDeaf);
    actions.push(LiveAction::Delay(std::time::Duration::from_millis(500)));
    actions.push(model(" My bicycle Quillfeather was in it.", 2_000, 3_000));
    actions.extend(audio(10, 0xDD));
    actions.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 3.0,
    });
    let mock = MockGptLive::start(LiveScript {
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");

    let mut session = start_greeting(&mock, false, QUOTING_GREETING);
    let mut said = String::new();
    let mut closing = false;
    loop {
        let event = tokio::time::timeout(marker(), session.events.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: no Closed; the model said so far {said:?}"))
            .expect("the session reports its end before it goes");
        match event {
            DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta,
                ..
            } => said.push_str(&delta),
            DuplexEvent::Appended {
                kind: AppendKind::Instructions,
                ..
            } if !closing => {
                closing = true;
                session
                    .control
                    .send(DuplexControl::Close)
                    .await
                    .expect("the session takes its close");
            }
            DuplexEvent::Closed { .. } => break,
            _ => {}
        }
    }
    assert!(closing, "the stop was acknowledged before the end");
    let heard = heard_after_closed(session).await;

    assert_eq!(
        said, "Hello, this is your companion, glad we can talk.",
        "nothing of the tail passes, before or after the close"
    );
    assert_eq!(heard, 10 * CHUNK_BYTES, "only the greeting's audio passes");
}

/// The audio that left a session whose events were read up to its `Closed`.
async fn heard_after_closed(session: Session) -> usize {
    drop(session.audio_in);
    drop(session.control);
    let heard = tokio::time::timeout(marker(), session.audio)
        .await
        .expect("HANG: the audio outlet closes with the session")
        .expect("the audio reader ran");
    let _ = tokio::time::timeout(marker(), session.join).await;
    heard
}
