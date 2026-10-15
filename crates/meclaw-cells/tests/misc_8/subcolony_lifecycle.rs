//! P9 steps C9–C17 — the facade as a live cell.
//!
//! A child colony behaves as ONE cell: a message goes in, an answer comes out,
//! and the parent never learns anything about the tree on the other side. The
//! child here is the protocol fixture rather than a real `meclaw` (that is the
//! package demo's job) — it can produce the failure modes a correct binary
//! never would.
//!
//! Positive receipts: every claim is proven by a message ARRIVING at `/sink`.

use meclaw_cells::subcolony::SubcolonyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_subcolony_protocol_fixture");

/// `/sink` first (anti-cascade), then `/child`, then the edge.
async fn topology(extra: serde_json::Value) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("child-cell");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    // The child colony's own tree. The fixture ignores it; the params check is
    // real, so it has to exist.
    let child_root = td.path().join("child-root");
    std::fs::create_dir_all(&child_root).expect("child root");

    let mut params = json!({
        "root": child_root.display().to_string(),
        "command": FIXTURE,
        "emit_to": "/sink",
        "boot_timeout_ms": 5000,
        "request_timeout_ms": 5000,
        "external_timeout_ms": 5000,
        "query_timeout_ms": 1000,
        "kill_grace_ms": 500
    });
    if let (Some(o), Some(e)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }

    // The edge FIRST, before the child process exists (GH #70). The protocol
    // fixture writes its unsolicited frame as its second action, right after the
    // ready handshake and before it reads a request, so a lane wired after the
    // spawn is a race the loaded runner wins: a frame with no matching edge is
    // dropped, not queued, and the case then waits out its whole marker for a
    // message that no longer exists. An edge to a path that is not registered
    // yet is legal, only its delivery needs the cell.
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/child"),
        Path::new("/sink"),
    )
    .await;

    let spawned = Arc::new(SubcolonyCellFactory)
        .spawn_cell(
            Path::new("/child"),
            params,
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .expect("spawn subcolony cell");
    h.register_spawned(Path::new("/child"), spawned).await;
    (h, recv_rx, td)
}

fn ask(text: &str) -> Message {
    MessageBuilder::new(Path::new("/child"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

/// Await one message at `/sink`. Failure-marker timeout, generous per
/// convention, and back at the usual 30s (GH #70): the width bought under GH #58
/// was paying for the wrong diagnosis. Both CI reds were an unsolicited frame
/// dropped for want of an edge, not a starved runner, which the runtime of the
/// binary said out loud both times by landing exactly on the marker. With the
/// edge wired before the child exists there is nothing left for width to buy,
/// and a narrow marker reports the next real hang in half a minute instead of
/// two. The semantic timing checks below stay tight either way.
async fn captured(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no message reached /sink within 30s")
        .expect("capture channel closed")
}

/// Give the I/O sub-task time to reach its boot verdict.
async fn settled() {
    tokio::time::sleep(Duration::from_millis(1200)).await;
}

/// The `header` slot of an emitted body is lifted into the `hop` compartment by
/// the substrate — that is exactly what lets a parent edge condition on whatever
/// the child colony put in its own header.
fn hop(msg: &Message) -> serde_json::Value {
    serde_json::Value::Object(msg.headers.hop.clone())
}

fn body_of(msg: &Message) -> serde_json::Value {
    match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected an inline body"),
    }
}

/// The package's core claim: a child colony answers like a cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_colony_answers_a_message_like_any_cell() {
    let (h, mut rx, _td) = topology(json!({})).await;
    h.send_from(Path::new("/parent"), ask("ping")).await;

    let got = captured(&mut rx).await;
    let body = body_of(&got);
    assert_eq!(hop(&got)["subcolony_event"], "reply");
    assert_eq!(
        body["messages"][0]["text"], "echo:ping",
        "the child topology produced this; got {body}"
    );
}

/// The trace survives into the child and the answer stays in the parent's trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_stays_inside_the_requesters_trace() {
    let (h, mut rx, _td) = topology(json!({})).await;
    let msg = ask("ping");
    let trace = msg.trace_id;
    h.send_from(Path::new("/parent"), msg).await;

    let got = captured(&mut rx).await;
    assert_eq!(
        got.trace_id, trace,
        "the reply must ride the requester's trace"
    );
}

/// T-TTL: the decrement is proven ON THE CHILD SIDE. The fixture reports the TTL
/// it received, so this is an observation rather than a parent-side claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_child_receives_a_decremented_ttl() {
    let (h, mut rx, _td) = topology(json!({})).await;
    let mut msg = ask("ping");
    msg.ttl = 12;
    h.send_from(Path::new("/parent"), msg).await;

    let got = captured(&mut rx).await;
    let body = body_of(&got);
    // The fixture echoes what it saw into its own reply body.
    assert_eq!(
        body["messages"][0]["text"], "echo:ping",
        "sanity: the round trip happened"
    );
    // 12 at the sender, 11 after the colony's routing hop (the cell sees that),
    // 10 in the child: crossing the boundary costs a hop of its own, ON TOP of
    // the routing hop. That is what makes a sub-colony cycle die.
    assert_eq!(
        hop(&got)["received_ttl"],
        10,
        "the boundary must cost one hop beyond the routing hop; got {body}"
    );
}

/// T-HDR: nothing of the parent context crosses unless declared — proven by the
/// child reporting what it actually received.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_context_key_never_reaches_the_child() {
    let (h, mut rx, _td) = topology(json!({})).await;
    let mut msg = ask("ping");
    msg.headers
        .context
        .insert("secret_hint".into(), json!("do-not-cross"));
    h.send_from(Path::new("/parent"), msg).await;

    let got = captured(&mut rx).await;
    let seen = &hop(&got)["received_context"];
    assert!(
        seen.get("secret_hint").is_none(),
        "an undeclared key crossed the boundary: {seen}"
    );
    assert!(
        seen.get("turn_id").is_some(),
        "the correlation key does cross: {seen}"
    );
}

/// T-VER-1: a child speaking another protocol is refused loudly, and stays
/// refused — no traffic ever flows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_with_a_foreign_protocol_is_refused_and_stays_refused() {
    let (h, mut rx, _td) = topology(json!({"extra_args": ["--protocol", "2"]})).await;
    // Let the boot verdict land first. A request that RACES the boot is a
    // different case: it fails on request_timeout, which is loud and correct but
    // less specific. See the plan's known-limits section.
    settled().await;
    h.send_from(Path::new("/parent"), ask("ping")).await;

    let got = captured(&mut rx).await;
    assert_eq!(hop(&got)["subcolony_event"], "error");
    assert_eq!(hop(&got)["error_code"], "protocol_mismatch");
}

/// T-BOOT-1: a child that never says hello fails on its budget, not silently.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_never_says_hello_fails_on_the_boot_budget() {
    let (h, mut rx, _td) =
        topology(json!({"extra_args": ["--no-ready"], "boot_timeout_ms": 500})).await;
    settled().await;
    h.send_from(Path::new("/parent"), ask("ping")).await;

    let got = captured(&mut rx).await;
    assert_eq!(hop(&got)["subcolony_event"], "error");
    assert_eq!(hop(&got)["error_code"], "boot_timeout");
}

/// T-LIFE-1: a child that goes away while a caller is waiting releases that
/// caller with a typed failure instead of letting it sit out its timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_dies_under_a_request_releases_the_caller() {
    let (h, mut rx, _td) = topology(json!({"extra_args": ["--die-on-request"]})).await;
    settled().await;

    let started = std::time::Instant::now();
    h.send_from(Path::new("/parent"), ask("ping")).await;
    let got = captured(&mut rx).await;

    assert_eq!(hop(&got)["subcolony_event"], "error");
    assert_eq!(hop(&got)["error_code"], "subcolony_gone");
    // Semantic timing discriminator, deliberately tight: request_timeout_ms is
    // 5s, so returning well under it proves the failure came from KNOWING the
    // child is gone (the ratified liveness slice) rather than from waiting.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the caller waited {:?} — that is a timeout, not a release",
        started.elapsed()
    );
}

/// D6: a frame nobody asked for reaches the topology instead of vanishing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsolicited_child_frame_reaches_the_configured_lane() {
    let (_h, mut rx, _td) = topology(json!({"extra_args": ["--unsolicited"]})).await;

    let got = captured(&mut rx).await;
    let body = body_of(&got);
    assert_eq!(hop(&got)["subcolony_event"], "unsolicited");
    assert_eq!(body["messages"][0]["text"], "nobody asked");
}

/// T-TO-1: a child that takes too long is cut off by the A-timeout (rule 12)
/// with a typed error, not left hanging.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_takes_too_long_hits_the_request_timeout() {
    let (h, mut rx, _td) = topology(json!({
        "extra_args": ["--delay-ms", "10000"],
        "request_timeout_ms": 700
    }))
    .await;
    settled().await;

    let started = std::time::Instant::now();
    h.send_from(Path::new("/parent"), ask("slow")).await;
    let got = captured(&mut rx).await;

    assert_eq!(hop(&got)["subcolony_event"], "error");
    assert_eq!(hop(&got)["error_code"], "request_timeout");
    // Semantic timing discriminator: the child sleeps 10s, the budget is 0.7s.
    // Returning under 5s proves the A-timeout fired rather than the child
    // eventually answering.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the A-timeout did not fire; waited {:?}",
        started.elapsed()
    );
}

/// Grandchild reaping rides on the P7 core unchanged (`process_group: true` in
/// `child_spec`), which `stdio_child_reaping.rs` already proves with a negative
/// control. Pinned here as a configuration assertion rather than duplicated as a
/// second process test: what would regress is the flag, not the mechanism.
#[test]
fn the_child_runs_in_its_own_process_group() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let p = meclaw_cells::subcolony::SubcolonyParams::parse(
        &json!({"root": td.path().to_string_lossy()}),
    )
    .expect("params");
    let spec = meclaw_cells::subcolony::io::child_spec_for_test(&p);
    assert!(
        spec.process_group,
        "a child colony spawns process trees; without a group they outlive the cell"
    );
    assert!(
        spec.env_clear,
        "the parent's secrets must not be the child's secrets"
    );
    assert!(
        spec.args.iter().any(|a| a == "json"),
        "the facade sets the wire itself: {:?}",
        spec.args
    );
    assert!(
        !spec.args.iter().any(|a| a == "--daemon" || a == "--api"),
        "the child must die on stdin EOF and open no port: {:?}",
        spec.args
    );
}
