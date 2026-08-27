//! GH #47, the core acceptance case: an `llm` cell with a live HTTP call in
//! flight when the shutdown arrives.
//!
//! This is the 2026-06-15 measurement without a provider. The mock server holds
//! the response for 800 ms — long enough that the pre-drain shutdown would have
//! torn the colony down while the cell was still awaiting its socket, which is
//! exactly the shape that produced "20 lines → 0 answers" against OpenRouter.
//!
//! Nothing about the cell is faked: real `llm` cell, real `reqwest` call, real
//! chat-completions body. Only the far end of the socket is ours.
//!
//! The mock is the CAPTURING variant of `meclaw_testing::mock_http`, which
//! records a request before it honours the delay. That turns "the cell is
//! inside its HTTP call" from a slept-for assumption into a measured fact: the
//! request is on the wire and the response is being withheld. Everything else
//! about the fixture is the plan's.
//!
//! Both tests take their positive receipt from the capture cell at `/sink`, and
//! they take it with `try_recv()` the moment `shutdown()` returns — the claim of
//! this lane is not "the answer showed up eventually", it is "the answer was
//! already delivered when the colony finished shutting down".

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker, generous per the 30 s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// How long the mock withholds the response. The whole point of the file: an
/// interval in which the cell holds no message in its mailbox, has emitted
/// nothing, and is nevertheless not done.
const HELD: Duration = Duration::from_millis(800);

/// The drain budget of the fixture. Set to `0` for the counter-proof of step
/// 18.3 — with the drain off the two tests below MUST fail, which is what makes
/// them a measurement of the drain rather than of the machine.
const DRAIN_BUDGET_MS: u64 = 20_000;

/// A real chat-completions response body, the shape `translate::parse_openai_response`
/// expects. Nothing here is a substrate stand-in — this is what a provider sends.
const COMPLETION: &[u8] =
    br#"{"id":"chatcmpl-gh47","object":"chat.completion","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#;

fn llm_config(base_url: &str) -> String {
    to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "test-model",
            "api_key": "test-key",
            "base_url": base_url,
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .expect("the llm config is serializable")
}

/// The colony of both tests: an `llm` cell at `/llm` pointed at the mock, a
/// capture cell at `/sink`, and the out-edge that carries the assistant turn
/// from the one to the other.
///
/// An emission is routed by the EMITTING cell's out-edges, so without that edge
/// the answer would dead-letter as `no_route` no matter what the drain did.
struct Fixture {
    /// Held for the lifetime of the test: the colony's tree lives in here.
    _td: tempfile::TempDir,
    h: ColonyHandle,
    sink_rx: mpsc::Receiver<Message>,
    /// Requests the mock has taken off the wire. A request in here whose
    /// response is still withheld IS a call in flight.
    captured: Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
}

async fn boot(drain_budget_ms: u64) -> Fixture {
    // Dropping the returned `JoinHandle` DETACHES the mock server rather than
    // stopping it (tokio semantics), so it serves for the whole test; the
    // fixture only needs its address and its capture log.
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(COMPLETION).with_delay(HELD)]).await;
    let base_url = format!("http://{addr}/v1");

    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {drain_budget_ms}}}"#),
    )
    .expect("write the test colony.json");
    std::fs::create_dir_all(td.path().join("main/llm")).expect("create the llm cell directory");
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./llm","to":"/sink"}]}}}"#,
    )
    .expect("write the root hive config");
    std::fs::write(
        td.path().join("main/llm/config.json"),
        llm_config(&base_url),
    )
    .expect("write the llm cell config");

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    Fixture {
        _td: td,
        h,
        sink_rx,
        captured,
    }
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": text}]
        })))
        .build()
}

fn text_of(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v["messages"][0]["text"].as_str().unwrap_or("").to_string()
}

/// Waits until the mock has taken at least `n` requests off the wire.
///
/// The capture happens BEFORE the delay, so a captured request whose response is
/// still withheld is exactly the state the pre-#47 shutdown mistook for "done":
/// the cell took the message out of its mailbox, has emitted nothing, and is
/// parked on its socket.
async fn await_calls_in_flight(
    captured: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
    n: usize,
) {
    tokio::time::timeout(MARKER, async {
        loop {
            if captured.lock().await.len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("the llm cell must have {n} chat-completions request(s) on the wire")
    });
}

/// Everything the capture cell holds right now, drained without waiting.
///
/// Called after `shutdown()` has returned: whatever is NOT in here was not
/// delivered before the teardown, and a drain that let it through afterwards
/// would not be a drain.
fn already_delivered(rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        out.push(m);
    }
    out
}

/// One turn, one real HTTP call in flight when the shutdown lands, one answer
/// that still arrives.
///
/// The positive receipt is the captured answer at `/sink` — NOT the absence of a
/// dead letter — a test called "proves X" has to prove X through a positive
/// signal, never through an absence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_llm_answer_in_flight_at_shutdown_still_arrives() {
    let Fixture {
        _td,
        h,
        mut sink_rx,
        captured,
    } = boot(DRAIN_BUDGET_MS).await;

    h.send(turn("ping")).await;

    // The cell is now INSIDE its HTTP call: its mailbox is empty and nothing has
    // been emitted. That is the exact state the pre-#47 shutdown read as "done".
    await_calls_in_flight(&captured, 1).await;

    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");

    let delivered = already_delivered(&mut sink_rx);
    assert_eq!(
        delivered.len(),
        1,
        "the answer to the in-flight call must have reached /sink BEFORE the \
         colony finished shutting down — got {} message(s)",
        delivered.len()
    );
    assert_eq!(
        text_of(&delivered[0]),
        "pong",
        "the captured message is the provider's assistant turn, not a stand-in"
    );
}

/// The same shape with FIVE calls, because a batch pipe is a fan-out and a drain
/// that only carried the first would still lose nineteen of twenty in the real
/// case.
///
/// One `llm` cell is one task, so the five turns queue behind each other: when
/// the shutdown lands, one call is on the wire and four turns are still waiting
/// in the mailbox. That IS the 2026-06-15 shape — twenty lines piped into one
/// cell — and it exercises both halves of quiescence at once: the handler that
/// has not returned, and the mailbox that is not empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_llm_answers_in_flight_at_shutdown_all_arrive() {
    const TURNS: usize = 5;

    let Fixture {
        _td,
        h,
        mut sink_rx,
        captured,
    } = boot(DRAIN_BUDGET_MS).await;

    for i in 0..TURNS {
        h.send(turn(&format!("ping {i}"))).await;
    }

    await_calls_in_flight(&captured, 1).await;

    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");

    let delivered = already_delivered(&mut sink_rx);
    assert_eq!(
        delivered.len(),
        TURNS,
        "every one of the {TURNS} turns must be answered before the teardown — a \
         drain that carried only the call already on the wire would lose the rest \
         of the batch"
    );
    for (i, m) in delivered.iter().enumerate() {
        assert_eq!(
            text_of(m),
            "pong",
            "captured message {i} is the provider's assistant turn"
        );
    }
}
