//! GH #571 — a `/colony/*` read touches no disk.
//!
//! `docs/meclaw-overview.md` states the contract in one sentence: topology is
//! answered "from Colony's in-memory registry, **without any database access**".
//! The read itself kept that promise — `build_graph_read_reply` is a synchronous
//! projection over two in-memory tables — but the DISPATCHER's prologue did not.
//! Before the fix every `/colony/*` dispatch, whatever its endpoint, first
//! * read the whole `templates` table out of `colony.db`,
//! * cloned it into a `TemplatesRegistry` snapshot, and
//! * built the rescan future — whose SYNCHRONOUS prologue walks the entire
//!   templates library with `std::fs::read_dir` and reads every `template.json`.
//!
//! All of it ran on the colony task before the first `.await`, and all of it was
//! thrown away again for every endpoint except the three that actually need
//! templates. On a real library that is hundreds of `read_dir` calls and dozens
//! of file reads per read request — the half-second loop stall the issue
//! measured at the top of every minute, when the display's refresh tick fires a
//! `/colony/graph` read.
//!
//! The proof is a POSITIVE measurement, not a timing assertion: `/proc/self/io`
//! counts the read syscalls this process issued, and nextest gives every test its
//! own process. A batch of `/colony/graph` reads that walks the library shows up
//! as thousands of read syscalls; a batch that keeps the contract shows up as
//! none. The second half of the test pins the contract itself — the library is
//! renamed out from under the running colony and the read answers exactly as
//! before, because it never looks there.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, RespawnFn, colony_task,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Headers, Message, Path, Uuid};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// How many templates the fixture library holds. Sized after a real deployment
/// (47 templates in 262 directories); the point is that the walk is expensive
/// enough that doing it is unmistakable in the syscall count.
const TEMPLATES: usize = 48;
/// Subdirectories per template — what makes the walk a walk.
const SUBDIRS: usize = 5;
/// Reads per measured batch.
const READS: usize = 40;

/// Read syscalls this process has issued so far (`/proc/self/io`, field `syscr`).
///
/// `None` where the file does not exist; the caller then reports the measurement
/// as unavailable instead of asserting on a number it does not have.
fn read_syscalls() -> Option<u64> {
    let io = std::fs::read_to_string("/proc/self/io").ok()?;
    io.lines()
        .find_map(|l| l.strip_prefix("syscr:"))
        .and_then(|v| v.trim().parse().ok())
}

/// A templates library big enough that walking it is visible: [`TEMPLATES`]
/// template directories, each with [`SUBDIRS`] child directories.
fn write_library(root: &std::path::Path) {
    for t in 0..TEMPLATES {
        let tpl = root.join(format!("tpl-{t:03}"));
        std::fs::create_dir_all(&tpl).expect("create template dir");
        std::fs::write(
            tpl.join("template.json"),
            format!(r#"{{"name":"tpl-{t:03}"}}"#),
        )
        .expect("write template.json");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        )
        .expect("write config.json");
        for s in 0..SUBDIRS {
            let child = tpl.join(format!("c{s}"));
            std::fs::create_dir_all(&child).expect("create child dir");
            std::fs::write(
                child.join("config.json"),
                r#"{"cell":{"type":"echo"},"params":{}}"#,
            )
            .expect("write child config.json");
        }
    }
}

/// A registered stand-in for a cell: the colony holds a plain mailbox sender, the
/// test holds the receiver. Enough to be a routable node and a reply anchor
/// without booting a cell task.
struct Stub {
    rx: mpsc::Receiver<Message>,
    _peace_tx: oneshot::Sender<()>,
    _backstop_tx: oneshot::Sender<()>,
}

async fn register_stub(inbox_tx: &mpsc::Sender<ColonyMsg>, path: Path) -> Stub {
    let (tx, rx) = mpsc::channel::<Message>(256);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("the stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Register {
            path,
            sender: tx,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "test-stub".into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
    Stub {
        rx,
        _peace_tx: peace_tx,
        _backstop_tx: backstop_tx,
    }
}

/// One `/colony/graph` read the way the display's refresh tick takes it: a cell
/// emission on the outputs channel whose own target is an ordinary path, resolved
/// onto the virtual endpoint by the probe's out-edge, answered back to the
/// emitting cell. That is the call site the issue measured — the outputs arm's
/// `RouteAction::ColonyDispatch`, not the inbox door.
async fn read_graph(
    outputs_tx: &mpsc::Sender<CellEmission>,
    probe: &Path,
    stub: &mut Stub,
) -> Value {
    outputs_tx
        .send(CellEmission {
            sender_path: probe.clone(),
            parent_message_id: Some(Uuid::now_v7()),
            trace_id: Uuid::now_v7(),
            input_ttl: 8,
            input_headers: Headers::default(),
            input_reply_to: None,
            target: Path::new("/sink"),
            content: json!({"messages": []}),
            direct_reply: false,
        })
        .await
        .expect("the outputs channel is the production emission path");
    let msg = tokio::time::timeout(Duration::from_secs(30), stub.rx.recv())
        .await
        .expect("the graph read must answer within the failure-marker timeout")
        .expect("the reply mailbox stays open");
    match msg.body {
        meclaw_core::Body::Inline(v) => v,
        other => panic!("the graph reply is an inline body, got {other:?}"),
    }
}

fn node_paths(reply: &Value) -> Vec<String> {
    let mut v: Vec<String> = reply["graph"]["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("the reply carries a nodes array: {reply}"))
        .iter()
        .map(|n| n["path"].as_str().unwrap_or_default().to_string())
        .collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graph_read_never_walks_the_templates_library() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    let library = root.join("templates");
    write_library(&library);

    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel::<CellEmission>(256);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            CellFactoryRegistry::new(),
            root.to_path_buf(),
            // The stubs are mailboxes, not cell tasks: nothing ever acks a
            // delivery, so the graceful drain would wait out its whole budget at
            // the end of the test. Ruling O7's documented off switch skips it.
            ColonyConfig {
                shutdown_drain_timeout_ms: 0,
                ..ColonyConfig::default()
            },
            None,
            None,
        )
        .with_templates_root(library.clone()),
    ));

    // A topology worth reading: the probe answers itself, `/a -> /b` is an edge
    // the reply must carry, and `/probe -> /colony/graph` is the shipped shape of
    // a read (a lane out of a cell onto the virtual endpoint).
    let probe = Path::new("/probe");
    let mut stub = register_stub(&inbox_tx, probe.clone()).await;
    let _a = register_stub(&inbox_tx, Path::new("/a")).await;
    let _b = register_stub(&inbox_tx, Path::new("/b")).await;
    for (from, to) in [
        (probe.clone(), Path::new("/colony/graph")),
        (Path::new("/a"), Path::new("/b")),
    ] {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from,
                to,
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("add_edge ack");
    }

    // Warm-up: one read outside the measurement, so nothing that happens once
    // (mailbox growth, the first statement cache) lands in the count.
    let first = read_graph(&outputs_tx, &probe, &mut stub).await;
    assert_eq!(
        node_paths(&first),
        vec!["/a".to_string(), "/b".to_string(), "/probe".to_string()],
        "the read answers the running registry"
    );

    let before = read_syscalls();
    let t = Instant::now();
    for _ in 0..READS {
        let reply = read_graph(&outputs_tx, &probe, &mut stub).await;
        assert_eq!(
            reply["graph"]["edges"]
                .as_array()
                .map(|e| e.len())
                .unwrap_or_default(),
            2,
            "every read answers the same two edges: {reply}"
        );
    }
    let elapsed = t.elapsed();
    let after = read_syscalls();
    eprintln!(
        "GH #571 measurement: {READS} /colony/graph reads over a library of \
         {TEMPLATES} templates took {elapsed:?} ({:?} per read); read syscalls: {before:?} -> {after:?}",
        elapsed / READS as u32
    );

    match (before, after) {
        (Some(b), Some(a)) => {
            let spent = a.saturating_sub(b);
            assert!(
                spent < READS as u64,
                "a `/colony/*` read is answered from memory: {READS} reads must \
                 not cost one read syscall each, but they spent {spent} \
                 (a library walk costs ~{} per read)",
                TEMPLATES
            );
        }
        _ => eprintln!(
            "GH #571: /proc/self/io is unavailable — the syscall half of this \
             test could not be measured on this host"
        ),
    }

    // The contract itself: the library is moved out from under the running
    // colony. A read that answers unchanged is a read that never looked there.
    std::fs::rename(&library, root.join("templates-moved-away")).expect("rename the library");
    let after_rename = read_graph(&outputs_tx, &probe, &mut stub).await;
    assert_eq!(
        node_paths(&after_rename),
        node_paths(&first),
        "the topology answer does not depend on the templates library existing"
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}
