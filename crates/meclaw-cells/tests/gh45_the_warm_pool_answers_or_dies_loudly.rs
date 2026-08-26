//! GH #45 / #420 -- the warm pool answers every message, whatever the child does.
//!
//! The pool's broker and its child tasks are not supervised (a stateless cell's
//! workers never were). So the guarantee is not "a panic reaches the
//! supervisor" but the stronger, cheaper one: there is no panic, and there is
//! no silence. Every pathological child below produces exactly one emission
//! with a documented `error_code` -- and the cell keeps serving afterwards.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

/// Spawn a warm `code` cell around `script` and return its mailbox + outputs.
async fn warm_cell(
    script: &str,
    timeout_ms: u64,
) -> (
    tokio::sync::mpsc::Sender<meclaw_core::Message>,
    tokio::sync::mpsc::Receiver<meclaw_core::CellEmission>,
    tempfile::TempDir,
) {
    let raw = json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": timeout_ms,
        "runner_mode": "warm",
        "max_concurrency": 1
    });
    let (otx, orx) = tokio::sync::mpsc::channel(16);
    let td = tempfile::TempDir::new().unwrap();
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
    let sender = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };
    (sender, orx, td)
}

fn msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages":[]})))
        .reply_to(Path::new("/sink"))
        .build()
}

/// A child that kills itself mid-run: the message still gets an answer, and the
/// NEXT message runs on a fresh child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_hard_exits_answers_and_is_replaced() {
    let (tx, mut rx, _td) = warm_cell(
        "import os,sys,json\n\
         d = json.load(sys.stdin)\n\
         if d['body'].get('die'): os._exit(0)\n\
         sys.stdout.write(json.dumps({'messages':[]}))\n",
        5_000,
    )
    .await;
    tx.send(
        MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[],"die":true})))
            .reply_to(Path::new("/sink"))
            .build(),
    )
    .await
    .unwrap();
    let dead = rx
        .recv()
        .await
        .expect("a dead child still produces an answer");
    assert_eq!(
        dead.content["header"]["error_code"], "invalid_json",
        "exit 0 with no stdout is what a cold run reports too"
    );
    tx.send(msg()).await.unwrap();
    let alive = rx.recv().await.expect("the cell serves on");
    assert_eq!(alive.content["header"]["exit_code"], 0);
}

/// Garbage on the real stdout is diagnostics, not a protocol break: the harness
/// frame that follows it still finds its way home.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_line_that_is_not_a_frame_does_not_break_the_run() {
    let (tx, mut rx, _td) = warm_cell(
        "import os,sys,json\n\
         os.write(1, b'a banner nobody asked for\\n')\n\
         sys.stdout.write(json.dumps({'messages':[]}))\n",
        5_000,
    )
    .await;
    tx.send(msg()).await.unwrap();
    let em = rx.recv().await.expect("the frame after the banner arrives");
    assert_eq!(em.content["header"]["exit_code"], 0);
}

/// Twelve messages, one child, one killer among them: twelve answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_message_gets_exactly_one_answer() {
    let (tx, mut rx, _td) = warm_cell(
        "import os,sys,json\n\
         d = json.load(sys.stdin)\n\
         if d['body'].get('n') == 5: os._exit(9)\n\
         sys.stdout.write(json.dumps({'messages':[]}))\n",
        5_000,
    )
    .await;
    for n in 0..12 {
        tx.send(
            MessageBuilder::new(Path::new("/code"))
                .body(Body::Inline(json!({"messages":[],"n":n})))
                .reply_to(Path::new("/sink"))
                .build(),
        )
        .await
        .unwrap();
    }
    let mut seen = 0;
    while seen < 12 {
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv()).await {
            Ok(Some(_)) => seen += 1,
            other => panic!("only {seen} of 12 answers arrived: {other:?}"),
        }
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv())
            .await
            .is_err(),
        "no thirteenth answer -- exactly one per message"
    );
}
