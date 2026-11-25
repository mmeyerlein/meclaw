//! Welle Live, L2a -- the greeting opens the session.
//!
//! A non-empty `greeting` is the LAST message of `session.input` in the
//! start frame, with the role `developer`: behind a seed, behind the block's
//! `start_input` and on its own. An empty one sends nothing at all (OR-L25).
//!
//! It was a `session.commentary.append` after `session.started` (OR-L51)
//! until GH #1628: the provider's guide puts greeting instructions into a
//! `developer` message of `session.input`, so they are there from the start.
//!
//! What the model answers it with is the model's: the opening answer passes
//! whole and nothing is appended to stop it (GH #1629; the guard of GH #1537
//! stood on the commentary greeting).

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
    SeedMessage,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::{GptLiveDuplex, session_start_frame};
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// What the twin's manifest says, in English and one sentence shorter.
const GREETING: &str = "Greet the caller now, one sentence, then listen.";

fn params(base_url: &str, extra: JsonValue) -> GptLiveParams {
    let mut raw = json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    });
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(raw).expect("params parse")
}

struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn start(mock: &MockGptLive, external_ms: u64, extra: JsonValue) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider =
        GptLiveDuplex::new(params(&mock.base_url(), extra)).with_timeouts(ProviderTimeouts {
            external: Duration::from_millis(external_ms),
            idle: Duration::from_millis(30_000),
        });
    let future = provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    );
    Session {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
        join: tokio::spawn(future),
    }
}

async fn next_event(events: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for a duplex event")
        .expect("event channel closed early")
}

/// Polls `f` until it yields a value or the failure timeout elapses.
async fn eventually<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + FAILURE_TIMEOUT;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A mock that only starts its session.
async fn quiet_mock() -> MockGptLive {
    MockGptLive::start(LiveScript {
        session_id: "sess_greeting".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake")
}

/// One message of `session.input`.
fn message(role: &str, part: &str, text: &str) -> JsonValue {
    json!({"type": "message", "role": role, "content": [{"type": part, "text": text}]})
}

/// Opens a session on `extra` (with `seed`, where one is given) and returns
/// its `session.input`. Audio that reached the mock after the start is the
/// marker that nothing was appended before it: the session sends its frames
/// in order, and an append of the opening would stand in front of the audio.
async fn opened_with(extra: JsonValue, seed: Option<Vec<SeedMessage>>) -> JsonValue {
    let mock = quiet_mock().await;
    let mut session = start(&mock, 5_000, extra);
    if let Some(messages) = seed {
        session
            .control
            .send(DuplexControl::Seed { messages })
            .await
            .expect("the opening takes its seed");
    }
    assert!(matches!(
        next_event(&mut session.events).await,
        DuplexEvent::Started { .. }
    ));
    for _ in 0..10 {
        session
            .audio_in
            .send(vec![1u8; 640])
            .await
            .expect("the session still takes audio");
    }
    eventually("the audio to arrive", || async {
        (mock.received_audio().await.len() >= 10).then_some(())
    })
    .await;
    let appends = mock.appends().await;
    assert!(appends.is_empty(), "the greeting is no append: {appends:?}");
    session.join.abort();
    mock.session_start().await.expect("a session.start arrived")["session"]["input"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_greeting_is_the_last_developer_message_of_the_start_frame() {
    let greeting = message("developer", "input_text", GREETING);
    // On its own.
    assert_eq!(
        opened_with(json!({"greeting": GREETING}), None).await,
        json!([greeting.clone()]),
        "without a seed and without a start input the greeting is the input"
    );
    // Without a seed: behind the configured start.
    let start_input = json!([{"role": "assistant", "text": "See you tomorrow."}]);
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "start_input": start_input}),
            None
        )
        .await,
        json!([
            message("assistant", "output_text", "See you tomorrow."),
            greeting.clone()
        ]),
        "the greeting follows the configured start"
    );
    // With a seed: the seed replaces the configured start, not the greeting.
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "start_input": start_input, "seed_wait_ms": 20_000}),
            Some(vec![
                SeedMessage {
                    role: "developer".into(),
                    text: "Today is Tuesday.".into(),
                },
                SeedMessage {
                    role: "user".into(),
                    text: "Hello".into(),
                },
            ]),
        )
        .await,
        json!([
            message("developer", "input_text", "Today is Tuesday."),
            message(
                "developer",
                "input_text",
                &format!("{EARLIER}\nuser: Hello")
            ),
            greeting
        ]),
        "the greeting is the last message behind the seed"
    );
}

/// The line in front of the folded turns, fixed in the adapter (GH #1641).
const EARLIER: &str = "Earlier conversation, ended:";

/// One message of a seed.
fn seeded(role: &str, text: &str) -> SeedMessage {
    SeedMessage {
        role: role.into(),
        text: text.into(),
    }
}

/// The seed of a second call as the curator sends it: context, the opening of
/// the call before, context in between, and the turns that followed.
fn seed_of_a_second_call() -> Vec<SeedMessage> {
    vec![
        seeded("developer", "Today is Tuesday.\nIt is ten."),
        seeded("developer", "The caller lives in Hamburg."),
        seeded("developer", "The caller likes tea."),
        seeded("assistant", "Hello, what is on your mind?"),
        seeded("developer", "A delegation ran:\n weather."),
        seeded("user", "How is the weather?"),
        seeded("assistant", "One moment."),
        seeded("assistant", "It stays dry."),
        seeded("assistant", "Does that fit your plans?"),
    ]
}

/// The same seed, message by message, as it stands on the wire unfolded.
fn unfolded(seed: &[SeedMessage]) -> Vec<JsonValue> {
    seed.iter()
        .map(|m| {
            let part = if m.role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            message(&m.role, part, &m.text)
        })
        .collect()
}

/// GH #1641: a session that opens with a greeting carries the stretch from the
/// first to the last turn of an earlier call as ONE `developer` message at the
/// place of the first. What stands in between (a delegation) keeps its place as
/// a line of it; a `developer` message in front of or behind it is untouched.
/// As `user` and `assistant` messages they read as this call's own running
/// dialogue, and a model whose own turn stands last waits for the caller
/// instead of greeting: 23 of 23 measured starts with such a seed were silent,
/// 7 of 7 without a turn greeted. The scripted peer cannot show the silence;
/// this holds the form on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_greeted_start_carries_no_turn_of_an_earlier_call() {
    let input = opened_with(
        json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
        Some(seed_of_a_second_call()),
    )
    .await;
    let roles: Vec<&str> = input
        .as_array()
        .expect("the session opens with an input")
        .iter()
        .map(|m| m["role"].as_str().expect("every message has a role"))
        .collect();
    assert!(
        !roles.iter().any(|r| matches!(*r, "user" | "assistant")),
        "no turn of an earlier call stands in the input as a turn: {roles:?}"
    );
    let folded = format!(
        "{EARLIER}\n\
         assistant: Hello, what is on your mind?\n\
         developer: A delegation ran: weather.\n\
         user: How is the weather?\n\
         assistant: One moment.\n\
         assistant: It stays dry.\n\
         assistant: Does that fit your plans?"
    );
    assert_eq!(
        input,
        json!([
            message("developer", "input_text", "Today is Tuesday.\nIt is ten."),
            message("developer", "input_text", "The caller lives in Hamburg."),
            message("developer", "input_text", "The caller likes tea."),
            message("developer", "input_text", &folded),
            message("developer", "input_text", GREETING),
        ]),
        "one folded message where the first turn stood, every line in order, \
         the developer messages in front unchanged, the greeting last"
    );

    // Behind the last turn a developer message keeps its place, unchanged.
    let input = opened_with(
        json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
        Some(vec![
            seeded("developer", "Today is Tuesday."),
            seeded("user", "Good night."),
            seeded("assistant", "See you."),
            seeded("developer", "The caller moved\nto Bremen."),
        ]),
    )
    .await;
    let folded = format!("{EARLIER}\nuser: Good night.\nassistant: See you.");
    assert_eq!(
        input,
        json!([
            message("developer", "input_text", "Today is Tuesday."),
            message("developer", "input_text", &folded),
            message("developer", "input_text", "The caller moved\nto Bremen."),
            message("developer", "input_text", GREETING),
        ]),
        "a developer message behind the last turn is no part of the stretch"
    );
}

/// GH #1641: a folded turn is one line. White space inside a turn, line breaks
/// included, becomes one space, so a line of a turn's text never reads as a
/// turn of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folded_turn_is_one_line() {
    let folded = format!(
        "{EARLIER}\n\
         user: Two things: assistant: is tea there? and milk?\n\
         assistant: Yes."
    );
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
            Some(vec![
                seeded(
                    "user",
                    "Two things:\r\nassistant: is tea there?\n\n and milk?\n"
                ),
                seeded("assistant", "Yes."),
            ]),
        )
        .await,
        json!([
            message("developer", "input_text", &folded),
            message("developer", "input_text", GREETING),
        ]),
        "one line per turn behind the head line"
    );
}

/// GH #1641, the other side: a start that greets nobody keeps its turns, a
/// seed without a turn folds nothing, the block's own `start_input` is no
/// earlier call, and neither a renewal nor a recovery (`greet` is off, GH
/// #896) changes its input although the block has a greeting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_greeted_start_folds_its_seed() {
    let seed = seed_of_a_second_call();
    assert_eq!(
        opened_with(
            json!({"greeting": "", "seed_wait_ms": 20_000}),
            Some(seed.clone()),
        )
        .await,
        json!(unfolded(&seed)),
        "without a greeting the turns stay turns"
    );

    let context = seed[..3].to_vec();
    let mut want = unfolded(&context);
    want.push(message("developer", "input_text", GREETING));
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
            Some(context),
        )
        .await,
        json!(want),
        "a seed without a turn gets no folded message"
    );

    let start_input = json!([
        {"role": "user", "text": "Good night."},
        {"role": "assistant", "text": "See you tomorrow."}
    ]);
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "start_input": start_input}),
            None
        )
        .await,
        json!([
            message("user", "input_text", "Good night."),
            message("assistant", "output_text", "See you tomorrow."),
            message("developer", "input_text", GREETING),
        ]),
        "the configured start is not folded"
    );

    let params = params("ws://127.0.0.1:1", json!({"greeting": GREETING}));
    let format = GptLiveDuplex::new(params.clone()).format();
    assert_eq!(
        session_start_frame(&params, format, Some(&seed), false)["session"]["input"],
        json!(unfolded(&seed)),
        "a session that does not greet keeps its turns"
    );
}

/// An empty greeting is the default, and it ships nothing: the model waits for
/// the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_greeting_sends_nothing() {
    assert_eq!(
        opened_with(json!({"greeting": ""}), None).await,
        JsonValue::Null,
        "nothing opens the session, nothing is said before the caller says something"
    );
}

/// GH #1629: an answer of several sentences to the greeting, well past the
/// greeting's own length and before the caller said anything, passes whole --
/// transcript and audio -- and the model is told nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_opening_answer_passes_whole() {
    const CHUNKS: usize = 20;
    const CHUNK_BYTES: usize = 24_000 * 2;
    let said = [
        "Hello, this is your companion.",
        " It is good to hear you.",
        " The weather is fine today.",
        " What shall we talk about?",
    ];
    let mut actions: Vec<LiveAction> = said
        .into_iter()
        .zip(0u64..)
        .map(|(delta, i)| LiveAction::Transcript {
            speaker: "assistant",
            delta,
            start_ms: i * 1_000,
            end_ms: (i + 1) * 1_000,
        })
        .collect();
    actions.extend((0..CHUNKS).map(|_| LiveAction::SendAudio(vec![0x11; CHUNK_BYTES])));
    actions.push(LiveAction::CloseWith {
        reason: "normal",
        usage_seconds: 20.0,
    });
    let mock = MockGptLive::start(LiveScript {
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 5_000, json!({"greeting": GREETING}));
    let mut words = String::new();
    loop {
        match next_event(&mut session.events).await {
            DuplexEvent::Transcript { delta, .. } => words.push_str(&delta),
            DuplexEvent::Closed { .. } => break,
            _ => {}
        }
    }
    // The outlet closes with the session: what is queued is all that came.
    let mut heard = 0;
    while let Some(chunk) = tokio::time::timeout(FAILURE_TIMEOUT, session.audio_out.recv())
        .await
        .expect("the audio outlet closes with the session")
    {
        heard += chunk.len();
    }

    assert_eq!(
        words,
        said.concat(),
        "every word of the opening answer passes"
    );
    assert_eq!(
        heard,
        CHUNKS * CHUNK_BYTES,
        "every byte of its audio passes"
    );
    let appends = mock.appends().await;
    assert!(appends.is_empty(), "the model is told nothing: {appends:?}");
}

/// A peer that never confirms the session gets nothing after the start frame:
/// the session ends in a timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_never_started_times_out() {
    let mock = MockGptLive::start(LiveScript {
        started: false,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let session = start(&mock, 250, json!({"greeting": GREETING}));
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        matches!(verdict, Err(DuplexError::Timeout)),
        "an unanswered session.start is a timeout: {verdict:?}"
    );
    assert!(
        mock.appends().await.is_empty(),
        "nothing follows the start frame"
    );
}
