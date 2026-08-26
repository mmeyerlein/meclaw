//! GH #420 -- a resident child is a cache, and a cache may be dropped.
//!
//! The script below keeps its counter in a module global (the point of the
//! mode) and mirrors it into a file (the durable truth). Run A never loses its
//! child; run B has it SIGKILLed in the middle. The two output sequences must
//! be identical -- if they are not, the mode is keeping truth in RAM.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

/// counter in RAM, mirrored to `state_path`; the pid so the test can kill it.
fn script(state_path: &str) -> String {
    format!(
        "import json, os, sys\n\
         STATE = {state:?}\n\
         d = json.load(sys.stdin)\n\
         n = globals().get('n')\n\
         if n is None:\n\
         \x20   try:\n\
         \x20       n = json.load(open(STATE))['n']\n\
         \x20   except Exception:\n\
         \x20       n = 0\n\
         n += 1\n\
         globals()['n'] = n\n\
         json.dump({{'n': n, 'pid': os.getpid()}}, open(STATE, 'w'))\n\
         sys.stdout.write(json.dumps({{'messages': [], 'n': n}}))\n",
        state = state_path
    )
}

async fn resident_cell(
    state_path: &str,
) -> (
    tokio::sync::mpsc::Sender<meclaw_core::Message>,
    tokio::sync::mpsc::Receiver<meclaw_core::CellEmission>,
) {
    let raw = json!({
        "runner": "python3",
        "script_inline": script(state_path),
        "external_timeout_ms": 10000,
        "runner_mode": "resident"
    });
    let (otx, orx) = tokio::sync::mpsc::channel(16);
    let td = Box::leak(Box::new(tempfile::TempDir::new().unwrap()));
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/code"),
            raw,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    match spawned {
        SpawnedCellKind::Active { sender, .. } => (sender, orx),
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    }
}

async fn one(
    tx: &tokio::sync::mpsc::Sender<meclaw_core::Message>,
    rx: &mut tokio::sync::mpsc::Receiver<meclaw_core::CellEmission>,
) -> i64 {
    tx.send(
        MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages": []})))
            .reply_to(Path::new("/sink"))
            .build(),
    )
    .await
    .unwrap();
    let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("no answer within 30s")
        .expect("channel open");
    em.content["n"]
        .as_i64()
        .expect("the script emits its counter")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_stream_survives_a_kill_in_the_middle() {
    let dir = tempfile::TempDir::new().unwrap();

    // Run A -- nothing is killed.
    let a_state = dir.path().join("a.json");
    let (tx, mut rx) = resident_cell(a_state.to_str().unwrap()).await;
    let mut a = Vec::new();
    for _ in 0..5 {
        a.push(one(&tx, &mut rx).await);
    }
    drop(tx);

    // Run B -- the child is SIGKILLed after the second message.
    let b_state = dir.path().join("b.json");
    let (tx, mut rx) = resident_cell(b_state.to_str().unwrap()).await;
    let mut b = Vec::new();
    b.push(one(&tx, &mut rx).await);
    b.push(one(&tx, &mut rx).await);
    let state: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&b_state).unwrap()).unwrap();
    let pid = state["pid"].as_i64().unwrap() as i32;
    assert_eq!(
        unsafe { libc::kill(pid, libc::SIGKILL) },
        0,
        "the child was alive"
    );
    // Give the kernel a moment to deliver the signal, so the next message meets
    // a dead child rather than a dying one.
    //
    // Deliberately NOT a "wait until the pid is gone" loop: the pool's child
    // task still holds the `Child` handle, so the killed process stays a ZOMBIE
    // -- `kill(pid, 0)` keeps answering 0 -- until the next job reaps it. Such a
    // loop could only ever run out its full budget, which is a sleep dressed up
    // as a condition. The reap is what the assertion below observes anyway.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        unsafe { libc::kill(pid, libc::SIGCONT) } == 0,
        "the pid is still the (now dead) child's -- nobody else may have taken it"
    );
    for _ in 0..3 {
        b.push(one(&tx, &mut rx).await);
    }

    assert_eq!(
        a, b,
        "a killed child must not change what the stream produces"
    );
    assert_eq!(a, vec![1, 2, 3, 4, 5], "and the counter is the obvious one");
}
