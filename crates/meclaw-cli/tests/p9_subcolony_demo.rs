//! P9 package demo — a real child colony behaves as ONE cell in a parent tree.
//!
//! The child here is a genuine `meclaw` process booting a genuine colony from
//! `tests/fixtures/subcolony-echo`: filesystem bootstrap, a root hive, a code
//! cell, its own `colony.db`. The parent never learns any of that. It sends a
//! message to `/child` and gets an answer back, exactly as it would from a
//! local cell.
//!
//! Also the composition locks (D5). "The child tree is not addressable" is true
//! by construction, and these two tests are what turn it into an invariant
//! rather than an accident.
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

/// The child colony fixture, copied out of the repo so the demo never writes
/// into `tests/fixtures/` (no-delete policy).
fn child_root(td: &TempDir) -> std::path::PathBuf {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/subcolony-echo");
    let dst = td.path().join("subcolony-echo");
    copy_tree(&src, &dst);
    dst
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Parent topology: `/sink` first (anti-cascade), then `/child`, then the edge.
async fn parent(td: &TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let cell_dir = td.path().join("child-cell");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");

    let spawned = Arc::new(SubcolonyCellFactory)
        .spawn_cell(
            Path::new("/child"),
            json!({
                "root": child_root(td).display().to_string(),
                "command": env!("CARGO_BIN_EXE_meclaw"),
                // A real colony boot is slower than a fixture's; give it room.
                "boot_timeout_ms": 20000,
                "request_timeout_ms": 20000,
                "external_timeout_ms": 5000,
                "query_timeout_ms": 1000,
                "kill_grace_ms": 1000
            }),
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
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/child"),
        Path::new("/sink"),
    )
    .await;
    (h, recv_rx)
}

fn ask_at(target: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

async fn captured(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// THE package demo: a whole colony, driven as one cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_child_colony_behaves_as_one_cell_in_the_parent_tree() {
    let td = TempDir::new().expect("tempdir");
    let (h, mut rx) = parent(&td).await;

    let msg = ask_at("/child", "hello");
    let trace = msg.trace_id;
    h.send_from(Path::new("/parent"), msg).await;

    let got = captured(&mut rx)
        .await
        .expect("the child colony produced no answer within 30s");
    let Body::Inline(body) = &got.body else {
        panic!("inline body expected")
    };

    assert_eq!(
        got.headers.hop["subcolony_event"], "reply",
        "the facade tagged its answer; hop: {:?}",
        got.headers.hop
    );
    assert_eq!(
        body["messages"][0]["text"], "child-colony saw: hello",
        "a real colony booted, routed and answered; got {body}"
    );
    assert_eq!(
        got.trace_id, trace,
        "one conversation, one trace, across two colonies and two message logs"
    );
    // And what does NOT cross: the child's own hop compartment. Inside the child
    // the code cell set `header.finish_reason`, which the child's substrate
    // lifted into ITS hop — a single-hop compartment, spent on arrival at the
    // child's root. The parent sees only what the facade itself declares. A
    // parent edge therefore conditions on `hop.subcolony_event`, not on the
    // child's internals; that is the boundary being opaque in both directions.
    assert!(
        !got.headers.hop.contains_key("finish_reason"),
        "the child's hop compartment leaked into the parent: {:?}",
        got.headers.hop
    );
}

/// D5 lock 1: the child's internal tree is not addressable from the parent.
///
/// `/child/echo` exists — inside the child colony. From the parent it is simply
/// not a node, and routing must say so rather than reach across.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_inside_the_child_tree_is_not_routable_from_the_parent() {
    let td = TempDir::new().expect("tempdir");
    let (h, mut rx) = parent(&td).await;

    h.send_from(Path::new("/parent"), ask_at("/child/echo", "reach in"))
        .await;

    assert!(
        captured(&mut rx).await.is_none(),
        "a parent message reached INTO the child tree — that is federation, not composition"
    );
    let dead = h.drain_dead_letters().await;
    assert!(
        dead.iter().any(
            |d| format!("{:?}", d.reason).to_lowercase().contains("route")
                || format!("{:?}", d.reason)
                    .to_lowercase()
                    .contains("unresolved")
        ),
        "the attempt must be refused as unroutable; dead letters: {dead:?}"
    );
}

/// D5 lock 2: the parent's mutation authority stops at the facade.
///
/// A mutation scoped into the child tree names nothing the parent knows, so it
/// is rejected. The child is mutated only through its own operator surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_scoped_into_the_child_tree_creates_nothing() {
    let td = TempDir::new().expect("tempdir");
    let (h, mut rx) = parent(&td).await;

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/child/echo",
                "add_nodes": [{"name": "smuggled", "cell": {"type": "bash"}, "params": {}}]
            }),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony reachable");
    let _ = tokio::time::timeout(Duration::from_secs(10), ack_rx).await;

    // Proof by behaviour rather than by introspection: if the mutation had
    // created anything under the child's namespace, that node would now be
    // addressable. It is not — the parent's write authority stops at the facade,
    // and the child's tree lives in another process behind another colony.db.
    h.send_from(
        Path::new("/parent"),
        ask_at("/child/echo/smuggled", "are you there"),
    )
    .await;
    assert!(
        captured(&mut rx).await.is_none(),
        "the parent mutated a node into the child's namespace — that is federation"
    );
    let dead = h.drain_dead_letters().await;
    assert!(
        dead.iter().any(
            |d| format!("{:?}", d.reason).to_lowercase().contains("route")
                || format!("{:?}", d.reason)
                    .to_lowercase()
                    .contains("unresolved")
        ),
        "the smuggled path must stay unroutable; dead letters: {dead:?}"
    );
}
