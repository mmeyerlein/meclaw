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
use meclaw_testing::EmissionsExt;
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
    // 32 > the 24 emissions of the test: the consume mark goes out through a
    // spawned task (`WorkGuard::drop`), so a full channel could let it lose the
    // race against the next message's answer (review stille, Minor 1).
    let (otx, orx) = tokio::sync::mpsc::channel(32);
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
        .recv_answer()
        .await
        .expect("a dead child still produces an answer");
    assert_eq!(
        dead.content["header"]["error_code"], "invalid_json",
        "exit 0 with no stdout is what a cold run reports too"
    );
    tx.send(msg()).await.unwrap();
    let alive = rx.recv_answer().await.expect("the cell serves on");
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
    let em = rx
        .recv_answer()
        .await
        .expect("the frame after the banner arrives");
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
    let mut ids = Vec::new();
    for n in 0..12 {
        let m = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[],"n":n})))
            .reply_to(Path::new("/sink"))
            .build();
        ids.push(m.id);
        tx.send(m).await.unwrap();
    }
    // GH #1589: absence by a marker, not by a silence. The cell task closes
    // every handled delivery with its consume mark on the same FIFO output
    // channel, after every emission the handler made for it (GH #1015). So the
    // answers of message n are exactly what arrives before the mark carrying
    // n's id; a second answer, however late, lands before that mark. The old
    // proof was 300 ms of quiet after the twelfth answer, which passed any
    // duplicate slower than that. The wait carries only the hang mark.
    for (n, id) in ids.iter().enumerate() {
        let mut answers = 0;
        loop {
            let what = format!("the consume mark of message {n}");
            let em = meclaw_testing::deadline::hang(&what, rx.recv())
                .await
                .unwrap_or_else(|| panic!("the cell dropped its output channel at message {n}"));
            if let Some(closed) = em.consumed_mark_id() {
                assert_eq!(
                    closed, *id,
                    "message {n} was closed by the mark of another delivery"
                );
                break;
            }
            answers += 1;
        }
        assert_eq!(
            answers, 1,
            "message {n} got {answers} answers -- exactly one per message"
        );
    }
}
