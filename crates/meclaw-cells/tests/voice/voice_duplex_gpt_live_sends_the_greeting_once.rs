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
            greeting
        ]),
        "the greeting is the last message behind the seed, which carries no turn (GH #1654)"
    );
}

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

/// The same seed, message by message, as it stands on the wire with its turns.
fn as_turns(seed: &[SeedMessage]) -> Vec<JsonValue> {
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

/// GH #1654 (ruling R-W25fix-1): every call starts as a new conversation. A
/// session that opens with a greeting carries no `user` or `assistant`
/// message of its seed and none of the `developer` messages between its first
/// and its last turn (a delegation, an advice, an event of the earlier call):
/// they belong to the earlier conversation. The `developer` messages in front
/// of and behind that stretch keep their order, and the greeting is the last
/// message. Folded into one `developer` message (GH #1641) the earlier call
/// still left the model silent when it ended on an open question of its own;
/// what the greeting needs of it comes from the curator, not from the turns.
/// The scripted peer cannot show the silence; this holds the form on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_greeted_start_carries_no_turn_of_an_earlier_conversation() {
    let input = opened_with(
        json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
        Some(seed_of_a_second_call()),
    )
    .await;
    assert_eq!(
        input,
        json!([
            message("developer", "input_text", "Today is Tuesday.\nIt is ten."),
            message("developer", "input_text", "The caller lives in Hamburg."),
            message("developer", "input_text", "The caller likes tea."),
            message("developer", "input_text", GREETING),
        ]),
        "no turn of the earlier call and nothing between its turns, \
         the developer messages in front unchanged, the greeting last"
    );

    // Behind the last turn a developer message keeps its place, unchanged.
    let input = opened_with(
        json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
        Some(vec![
            seeded("developer", "Today is Tuesday."),
            seeded("user", "Good night."),
            seeded("developer", "(advice) The caller is tired."),
            seeded("assistant", "See you."),
            seeded("developer", "The caller moved\nto Bremen."),
        ]),
    )
    .await;
    assert_eq!(
        input,
        json!([
            message("developer", "input_text", "Today is Tuesday."),
            message("developer", "input_text", "The caller moved\nto Bremen."),
            message("developer", "input_text", GREETING),
        ]),
        "what stands in front of and behind the turns stays, in its order"
    );

    // A seed of nothing but turns leaves the greeting alone.
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
            Some(vec![
                seeded("user", "Two things:\nassistant: is tea there?"),
                seeded("assistant", "Will you call tomorrow?"),
            ]),
        )
        .await,
        json!([message("developer", "input_text", GREETING)]),
        "the greeting opens a new conversation"
    );
}

/// GH #1654, review I1: a greeted seed that is left empty once its turns are
/// gone is still the seed that came. It wins over the block's `start_input`
/// (a seed wins), so neither the configured start's turns nor its developer
/// messages stand in front of the greeting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_of_nothing_but_turns_still_wins_over_the_start_input() {
    let start_input = json!([
        {"role": "developer", "text": "Today is Monday."},
        {"role": "assistant", "text": "See you tomorrow."}
    ]);
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "start_input": start_input, "seed_wait_ms": 20_000}),
            Some(vec![
                seeded("user", "Good night."),
                seeded("assistant", "Will you call tomorrow?"),
            ]),
        )
        .await,
        json!([message("developer", "input_text", GREETING)]),
        "the seed came: the configured start does not open the session"
    );
}

/// GH #1654, the other side: a start that greets nobody keeps its turns, a
/// seed without a turn loses nothing, the block's own `start_input` is no
/// earlier call, and neither a renewal nor a recovery (`greet` is off, GH
/// #896) changes its input although the block has a greeting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_greeted_start_drops_the_turns_of_its_seed() {
    let seed = seed_of_a_second_call();
    assert_eq!(
        opened_with(
            json!({"greeting": "", "seed_wait_ms": 20_000}),
            Some(seed.clone()),
        )
        .await,
        json!(as_turns(&seed)),
        "without a greeting the turns stay turns"
    );

    let context = seed[..3].to_vec();
    let mut want = as_turns(&context);
    want.push(message("developer", "input_text", GREETING));
    assert_eq!(
        opened_with(
            json!({"greeting": GREETING, "seed_wait_ms": 20_000}),
            Some(context),
        )
        .await,
        json!(want),
        "a seed without a turn goes out whole"
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
        "the configured start keeps its turns"
    );

    let params = params("ws://127.0.0.1:1", json!({"greeting": GREETING}));
    let format = GptLiveDuplex::new(params.clone()).format();
    assert_eq!(
        session_start_frame(&params, format, Some(&seed), false)["session"]["input"],
        json!(as_turns(&seed)),
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
