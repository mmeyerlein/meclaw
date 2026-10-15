//! GH #119 — TTL exhaustion inside a tool loop must not be a silent stall.
//!
//! When a message dies of TTL exhaustion the corridor `route()` dead-letters it
//! and deliberately takes no `reply_to` cascade. Inside a fan-in that reads as a
//! stall: the collector never completes its round, the origin waits forever, and
//! the topology has nothing to route on.
//!
//! The guarantee built here: a TTL death that carries a **reply anchor**
//! (`reply_to`) produces exactly ONE terminal notice addressed to that anchor —
//! a substrate error reply in the canonical shape
//! (`hop.finish_reason == "error"`, `hop.error_code == "ttl_expired"`). The
//! notice is itself terminal (`reply_to == None`), so it can never produce a
//! notice of its own. Without an anchor nothing changes: the DLQ, as before.
//!
//! The guarantee is **opt-in** (`colony.json` `ttl_notice`, default `false`) for
//! the same reason `modifier.restore_ttl` is: the notice carries a fresh routing
//! budget, so a colony that turns it on has taken its loops out of the TTL guard
//! and bounds them with an iteration counter instead.

use meclaw_colony::DeadLetterReason;
use meclaw_core::{Cell, CellOutput, Message, OutputSink, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// The waiting side of a tool round, reduced to what the guarantee needs: it
/// fans out once, and when a terminal notice comes back it CLOSES the round by
/// emitting the round result. Both emissions travel the same single out-edge,
/// so the receipt is the TEXT that arrives at the tool path, not the arrival
/// itself — a positive signal for "the round closed", not a negative one.
struct RoundCell;

impl Cell for RoundCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(&mut self, msg: Message, sink: &OutputSink) -> impl Future<Output = ()> + Send {
        let sink = sink.clone();
        async move {
            let is_notice = msg
                .headers
                .hop
                .get("error_code")
                .and_then(|v| v.as_str())
                .is_some_and(|c| c == "ttl_expired");
            let text = if is_notice { "round closed" } else { "fan-out" };
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/loop/tool"),
                    content: json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": text}]
                    }),
                })
                .await;
        }
    }
}

/// A colony root whose `colony.json` opts into the terminal notice.
fn opted_in_root() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"schema_version": 1, "ttl_notice": true}"#,
    )
    .expect("write colony.json");
    td
}

/// Assemble `/loop/collector` (the waiting side) → `/loop/tool` (capture).
async fn build_loop(root: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let colony = ColonyHandle::new_with_echo_at(root.path());
    let (tool_tx, tool_rx) = mpsc::channel::<Message>(64);

    colony
        .spawn(Path::new("/loop/collector"), || RoundCell)
        .await;
    let t = tool_tx.clone();
    colony
        .spawn(Path::new("/loop/tool"), move || CaptureCell::new(t.clone()))
        .await;
    colony
        .add_edge(
            Uuid::now_v7(),
            Path::new("/loop/collector"),
            Path::new("/loop/tool"),
        )
        .await;

    (colony, tool_rx)
}

/// Text of the first UBF turn, for the receipt assertions.
fn first_text(msg: &Message) -> String {
    let meclaw_core::Body::Inline(v) = &msg.body else {
        return String::new();
    };
    v.get("messages")
        .and_then(|m| m.as_array())
        .and_then(|a| a.first())
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Collect every message that reaches the tool path within `budget`.
/// Generous by the 30s failure-marker convention.
async fn drain_until(rx: &mut mpsc::Receiver<Message>, want: usize) -> Vec<Message> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut out = Vec::new();
    while out.len() < want && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Some(m)) => out.push(m),
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ttl_death_with_a_reply_anchor_closes_the_waiting_round() {
    let root = opted_in_root();
    let (colony, mut tool_rx) = build_loop(&root).await;

    // ttl = 1: `route()` decrements to 0 on the hop into the collector, so the
    // collector's fan-out emission starts at 0 and dies at its next routing
    // decision — exactly the shape of a tool round that outruns its budget.
    let start = TestMessageBuilder::new("/loop/collector")
        .with_ttl(1)
        .build();
    colony.send_from(Path::new("/"), start).await;

    let got = drain_until(&mut tool_rx, 1).await;
    assert_eq!(
        got.len(),
        1,
        "the round must close: exactly one message reaches /loop/tool"
    );
    assert_eq!(
        first_text(&got[0]),
        "round closed",
        "the message that arrives is the ROUND RESULT — the fan-out itself died \
         of TTL, so only the terminal notice can have produced this"
    );

    // The dead letter itself stays: the notice is an ADDITION, not a swap.
    let dls = colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "exactly one dead letter: {dls:?}");
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_notice_is_the_canonical_substrate_error_reply_and_is_terminal() {
    let root = opted_in_root();
    let colony = ColonyHandle::new_with_echo_at(root.path());
    let (waiter_tx, mut waiter_rx) = mpsc::channel::<Message>(64);
    let w = waiter_tx.clone();
    colony
        .spawn(Path::new("/loop/waiter"), move || {
            CaptureCell::new(w.clone())
        })
        .await;

    // A message that dies before it can ever reach `/loop/tool` (which does not
    // even exist — the TTL check short-circuits before the lookup), anchored at
    // the waiter. Context is the persistent compartment and must survive.
    let dying = TestMessageBuilder::new("/loop/tool")
        .with_ttl(0)
        .with_reply_to("/loop/waiter")
        .build();
    let dying_id = dying.id;
    let dying_trace = dying.trace_id;
    colony.send_from(Path::new("/loop/collector"), dying).await;

    let got = drain_until(&mut waiter_rx, 1).await;
    assert_eq!(got.len(), 1, "the anchor must receive exactly one notice");
    let n = &got[0];
    assert_eq!(
        n.headers.hop.get("finish_reason").and_then(|v| v.as_str()),
        Some("error"),
        "canonical substrate error-reply shape"
    );
    assert_eq!(
        n.headers.hop.get("error_code").and_then(|v| v.as_str()),
        Some("ttl_expired"),
        "the error code is the DLQ reason token"
    );
    assert_eq!(
        n.reply_to, None,
        "the notice is TERMINAL — no anchor of its own, so it can never produce \
         a notice itself"
    );
    assert_eq!(
        n.parent_message_id,
        Some(dying_id),
        "the notice hangs under the message that died"
    );
    assert_eq!(n.trace_id, dying_trace, "same trace");
    assert!(
        n.ttl > 0,
        "the notice carries a live budget — a round closes over several hops"
    );

    let dls = colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "the dead letter stays: {dls:?}");
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_reply_anchor_nothing_changes() {
    let root = opted_in_root();
    let (colony, mut tool_rx) = build_loop(&root).await;

    // No `reply_to` — the pre-#119 behaviour must be byte-for-byte the same.
    let dying = TestMessageBuilder::new("/loop/tool").with_ttl(0).build();
    colony.send_from(Path::new("/"), dying).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        tool_rx.try_recv().is_err(),
        "no anchor, no notice — nothing is delivered anywhere"
    );

    let dls = colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "DLQ as before: {dls:?}");
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_that_did_not_opt_in_keeps_the_silent_terminal_ttl() {
    // No `colony.json` at all — the substrate default. This is the discriminator
    // for the whole feature: the SAME topology and the SAME budget that closes
    // its round above must stall here, because an expired TTL is terminal and
    // silent unless a colony says otherwise.
    let root = tempfile::TempDir::new().expect("tempdir");
    let (colony, mut tool_rx) = build_loop(&root).await;

    let start = TestMessageBuilder::new("/loop/collector")
        .with_ttl(1)
        .build();
    colony.send_from(Path::new("/"), start).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        tool_rx.try_recv().is_err(),
        "opt-in means opt-in — without the flag the fan-out dies silently"
    );

    let dls = colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "the dead letter, and nothing else: {dls:?}");
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);

    colony.shutdown().await;
}
