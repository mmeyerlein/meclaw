//! GH #82 — the TTL budget of a store-backed tool loop.
//!
//! One user-visible tool round in that shape is not one hop. It is a dozen:
//! planner, dispatch, tool, collector, and then the collector's read-modify-write
//! conversation with the store (insert, select, guarded update, select again),
//! each leg a routing decision that decrements `ttl`. `ttl` is never refreshed,
//! so the loop guard becomes the ceiling on how many tool rounds an assistant can
//! take, and the round that runs out dies mid fan-in — terminal, direct to the
//! dead-letter queue, nothing emitted toward the origin.
//!
//! Five pins:
//! 1. six rounds on the default budget of 64 do NOT finish (the measured ceiling),
//! 2. six rounds on a budget sized for the shape DO finish,
//! 3. a TTL death is loud and names the message, instead of being silent,
//! 4. six rounds DO finish on the DEFAULT budget when the loopback edge carries
//!    `modifier.restore_ttl` (GH #82, ruling 2026-08-13 — the loop pays for one
//!    round at a time instead of all of them),
//! 5. that restore is a reset, not an accumulation: after six of them the TTL is
//!    still at or below the budget the message started with.
//!
//! The topology under test is the checked-in store-backed loop fixture, the same
//! shape the walkthrough teaches, with a deterministic `MockOpenAI` as the brain.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_core::{Body, MessageBuilder, Path, serde_json::json};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{boot, copy_tree_patch_base_url, example_dir, recv_bounded};
use tracing::field::{Field, Visit};
use tracing::subscriber::Subscriber;
use tracing::{Event, Level, Metadata, span};

/// Rounds the task asks for. Five is the measured ceiling on the default budget,
/// so six is the first number that proves the ceiling moved.
const ROUNDS: usize = 6;

/// A budget sized for the shape: the walkthrough's formula
/// `2 + rounds * 12` for twelve rounds, rounded up.
const SIZED_TTL: u32 = 160;

/// One canned tool round per iteration, then the final answer.
async fn six_round_brain() -> MockOpenAI {
    let mut script = Vec::new();
    for i in 0..ROUNDS {
        script.push(canned_tool_calls(vec![(
            Box::leak(format!("call-{i}").into_boxed_str()),
            "calc",
            r#"{"x":1,"y":1}"#,
        )]));
    }
    script.push(canned_chat_completion("done", "stop"));
    MockOpenAI::start(script).await
}

/// The fixture's ingress turn, with an explicit TTL. In a running colony this
/// number comes from `colony.json` `message_default_ttl` (pinned in
/// `colony.rs::outputs_arm_source_emission_gets_colony_config_ttl`) or from the
/// `ttl` field of the initial HTTP message; here it is set directly so the test
/// is about the budget and nothing else.
fn user_probe_with_ttl(turn_id: &str, ttl: u32) -> meclaw_core::Message {
    let tool_schema = json!({
        "type": "function",
        "function": {"name": "calc", "description": "calc", "parameters": {"type": "object", "properties": {}}}
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("turn_id".into(), json!(turn_id));
    MessageBuilder::new(Path::new("/capture"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calc": {"text": tool_schema_str}}},
            "messages": [{"origin": "user", "type": "text", "text": "six steps please"}]
        })))
        .context(headers)
        .ttl(ttl)
        .build()
}

/// The ceiling, measured. `message_default_ttl` defaults to 64; a store-backed
/// tool round costs about a dozen routing hops; the sixth round therefore runs
/// out mid fan-in. The receipt is positive: `ttl_expired` rows appear in the
/// dead-letter queue, and the brain is never asked the seventh time (the call
/// that would have produced the answer).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_tool_rounds_do_not_fit_the_default_ttl_budget() {
    let mock = six_round_brain().await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    let (h, _sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe_with_ttl(
        "t-default",
        meclaw_core::MESSAGE_DEFAULT_TTL,
    ))
    .await;

    // Positive receipt: poll the DLQ until the TTL death shows up.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut ttl_deaths = Vec::new();
    while std::time::Instant::now() < deadline {
        for dl in h.drain_dead_letters().await {
            if matches!(
                dl.reason,
                meclaw_colony::dead_letter::DeadLetterReason::TtlExpired
            ) {
                ttl_deaths.push(dl);
            }
        }
        if !ttl_deaths.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !ttl_deaths.is_empty(),
        "six rounds on the default budget must run out of TTL — that is the measured ceiling"
    );
    // The brain was never asked the seventh time, so no answer was ever produced.
    let calls = mock.recorded_requests().await.len();
    assert!(
        calls <= ROUNDS,
        "the answering call ({}) must never happen on the default budget, saw {calls}",
        ROUNDS + 1
    );
    h.shutdown().await;
}

/// The same six rounds, on a budget sized for the shape: the loop finishes and
/// the answer reaches `/sink`. This is the pin the issue asks for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_tool_rounds_complete_when_the_budget_is_sized_for_the_shape() {
    let mock = six_round_brain().await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe_with_ttl("t-sized", SIZED_TTL)).await;

    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("with a budget sized for the shape, six tool rounds must reach an answer");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "the loop ends on the brain's stop, not on TTL"
    );
    assert_eq!(
        fin.headers.context["iter"], ROUNDS,
        "the fire edge counted one iteration per tool round"
    );
    // Measured, not guessed: 160 - 84 = 76 routing hops for six rounds end to
    // end, about a dozen per round. The default budget of 64 cannot hold six.
    assert!(
        fin.ttl < SIZED_TTL - meclaw_core::MESSAGE_DEFAULT_TTL,
        "six rounds must cost more than the default budget of {} (remaining {})",
        meclaw_core::MESSAGE_DEFAULT_TTL,
        fin.ttl
    );
    let calls = mock.recorded_requests().await.len();
    assert!(
        calls > ROUNDS,
        "six tool rounds plus the answering call = more than {ROUNDS} provider calls, saw {calls}"
    );
    h.shutdown().await;
}

// ───── the restoring loopback edge (GH #82, ruling 2026-08-13) ─────

/// Turn the fixture's loopback edge into a ttl-restoring one.
///
/// The fixture's re-entry edge is `./collector -> /llm` under
/// `hop.route == 'fire'`, the edge that already carries the iteration counter
/// (`set_context.iter = int(context.iter) + 1`). The ruling puts the ttl
/// restore exactly there: an explicit modifier field, visible in the topology
/// JSON, next to the bound that makes the loop legitimate.
///
/// Patching the COPY in the TempDir (not the checked-in fixture) is deliberate:
/// the two pins above measure the un-restored ceiling on the same fixture and
/// must stay untouched.
fn make_loopback_restore_ttl(root: &std::path::Path) {
    let p = root.join("main/tool-loop/config.json");
    let mut v: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let edges = v["params"]["graph"]["edges"].as_array_mut().unwrap();
    let fire = edges
        .iter_mut()
        .find(|e| e["condition"] == "hop.route == 'fire'")
        .expect("the fixture must carry the `fire` loopback edge");
    fire["modifier"]["restore_ttl"] = json!(true);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The ruling, pinned: with `modifier.restore_ttl` on the loopback edge, six
/// tool rounds complete on the SUBSTRATE DEFAULT budget of 64 — the budget that
/// `six_tool_rounds_do_not_fit_the_default_ttl_budget` proves is one round too
/// short without it. Same fixture, same brain, same number of rounds; the only
/// difference is the modifier on the re-entry edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_tool_rounds_complete_on_the_default_budget_when_the_loopback_edge_restores_ttl() {
    let mock = six_round_brain().await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    make_loopback_restore_ttl(td.path());
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe_with_ttl(
        "t-restore",
        meclaw_core::MESSAGE_DEFAULT_TTL,
    ))
    .await;

    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("a restoring loopback edge must carry six tool rounds on the default budget of 64");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "the loop ends on the brain's stop, not on TTL"
    );
    assert_eq!(
        fin.headers.context["iter"], ROUNDS,
        "the restoring edge is the same edge that counts the iteration — six rounds, six counts"
    );
    let calls = mock.recorded_requests().await.len();
    assert!(
        calls > ROUNDS,
        "six tool rounds plus the answering call = more than {ROUNDS} provider calls, saw {calls}"
    );
    h.shutdown().await;
}

/// The guard the restore must not dissolve: a restore is a RESET to the budget,
/// never an accumulation. Six restores on a message that started at the default
/// budget of 64 must leave it at or below 64 — if the modifier added budget
/// instead of resetting it, the surviving TTL would be a multiple of it and the
/// loop guard would be gone for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restoring_loopback_edge_never_lifts_ttl_above_the_initial_budget() {
    let mock = six_round_brain().await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    make_loopback_restore_ttl(td.path());
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe_with_ttl(
        "t-ceiling",
        meclaw_core::MESSAGE_DEFAULT_TTL,
    ))
    .await;

    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("the restoring loop must reach an answer");
    assert!(
        fin.ttl <= meclaw_core::MESSAGE_DEFAULT_TTL,
        "after {ROUNDS} restores the TTL must still be at or below the initial budget of {} \
         (saw {}) — restore resets to the budget, it never accumulates",
        meclaw_core::MESSAGE_DEFAULT_TTL,
        fin.ttl
    );
    h.shutdown().await;
}

// ───── the silent half ─────

/// ERROR-only subscriber: renders every ERROR event as one `name=value …` line.
/// Same shape as `code_failure_modes.rs`'s WARN capture — a process-wide default
/// (a scoped dispatcher does not survive the colony's own tasks), filtered per
/// test by the message id.
struct ErrorCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl Subscriber for ErrorCapture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() == Level::ERROR
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut visitor = LineVisitor { parts: Vec::new() };
        event.record(&mut visitor);
        self.lines.lock().unwrap().push(visitor.parts.join(" "));
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

struct LineVisitor {
    parts: Vec<String>,
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.parts.push(format!("{}={value}", field.name()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.parts.push(format!("{}={value:?}", field.name()));
    }
}

fn global_error_lines() -> &'static Arc<Mutex<Vec<String>>> {
    static LINES: std::sync::OnceLock<Arc<Mutex<Vec<String>>>> = std::sync::OnceLock::new();
    LINES.get_or_init(|| {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let _ = tracing::subscriber::set_global_default(ErrorCapture {
            lines: Arc::clone(&lines),
        });
        lines
    })
}

/// TTL exhaustion is terminal by spec: no reply, no cascade, nothing toward the
/// origin. What must NOT also happen is silence. The death names the message, so
/// an operator reading the log can find which message died and where — and the
/// line says what the budget is for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ttl_death_is_loud_and_names_the_message() {
    let lines = global_error_lines();
    let td = tempfile::TempDir::new().unwrap();
    // Smallest bootable tree: one empty hive. The target of the dying message is
    // the `/sink` capture cell the boot helper spawns.
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}).to_string(),
    )
    .unwrap();
    let (h, _sink_rx) = support::boot_code_only(&td).await;

    let msg = MessageBuilder::new(Path::new("/sink"))
        .body(Body::Inline(json!({"messages": []})))
        .ttl(0)
        .build();
    let id = msg.id.to_string();
    h.send(msg).await;

    // Positive receipt: the dead letter is written, then the line is there.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        if h.drain_dead_letters().await.iter().any(|dl| {
            matches!(
                dl.reason,
                meclaw_colony::dead_letter::DeadLetterReason::TtlExpired
            )
        }) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen, "a message with an exhausted TTL must dead-letter");

    let captured: Vec<String> = lines
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.contains(&id))
        .cloned()
        .collect();
    assert_eq!(
        captured.len(),
        1,
        "exactly one loud line per TTL death, naming the message: {captured:?}"
    );
    let line = &captured[0];
    assert!(
        line.contains("target=/sink"),
        "the line names the target: {line}"
    );
    assert!(
        line.contains("TtlExpired"),
        "the line names the reason: {line}"
    );
    assert!(
        line.contains("message_default_ttl"),
        "the line says what to size: {line}"
    );
    h.shutdown().await;
}
