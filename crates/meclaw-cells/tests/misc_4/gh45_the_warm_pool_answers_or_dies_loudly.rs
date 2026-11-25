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
    // 32 > the 24 emissions of the test, so a mark goes out at the drop of its
    // own delivery and never through the task `WorkGuard::drop` spawns for a
    // full channel. Either way it stands behind its own handler's emissions; the
    // contract says nothing about its place among other deliveries (#1644).
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
    // channel, after every emission its OWN handler made (GH #1015,
    // `CellEmission::consumed_mark`, `WorkGuard`). Across deliveries there is
    // no order: the worker frees its slot before it drops its guard, so the
    // answer and the mark of n+1 may overtake the mark of n. The form that
    // wanted mark n first was red in 355 of 1000 runs under 24 busy loops
    // (#1644). So every emission is booked on the delivery it names, and the
    // order is judged within one delivery only.
    //
    // The read ends when the output channel closes: without its mailbox the
    // dispatcher returns, and the last worker takes the last sender with it.
    // So nothing can follow unseen, not even behind the last mark. The wait
    // carries only the hang mark.
    drop(tx);
    let mut answers = vec![0_usize; ids.len()];
    let mut closed = vec![false; ids.len()];
    while let Some(em) =
        meclaw_testing::deadline::hang("the end of the output channel", rx.recv()).await
    {
        let parent = em.parent_message_id;
        let n = ids
            .iter()
            .position(|id| Some(*id) == parent)
            .unwrap_or_else(|| panic!("an emission names no sent message: {em:?}"));
        if em.consumed_mark_id().is_some() {
            assert!(!closed[n], "message {n} was closed by a second mark");
            closed[n] = true;
        } else {
            assert!(!closed[n], "message {n} got an answer after its own mark");
            answers[n] += 1;
        }
    }
    for (n, (got, closed)) in answers.into_iter().zip(closed).enumerate() {
        assert_eq!(
            got, 1,
            "message {n} got {got} answers -- exactly one per message"
        );
        assert!(closed, "message {n} was never closed by its mark");
    }
}
