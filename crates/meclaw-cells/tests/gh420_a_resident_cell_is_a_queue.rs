//! GH #420 -- a resident `code` cell answers in mailbox order, always.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_messages_come_back_in_the_order_they_went_in() {
    // A body whose runtime shrinks with n: if anything ran in parallel or out
    // of order, the later (faster) messages would overtake the earlier ones.
    let raw = json!({
        "runner": "python3",
        "script_inline":
            "import sys, json, time\n\
             d = json.load(sys.stdin)\n\
             n = d['body']['n']\n\
             time.sleep((20 - n) * 0.005)\n\
             sys.stdout.write(json.dumps({'messages': [], 'n': n}))\n",
        "external_timeout_ms": 10000,
        "runner_mode": "resident"
    });
    let (otx, mut orx) = tokio::sync::mpsc::channel(64);
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
    for n in 0..20 {
        sender
            .send(
                MessageBuilder::new(Path::new("/code"))
                    .body(Body::Inline(json!({"messages": [], "n": n})))
                    .reply_to(Path::new("/sink"))
                    .build(),
            )
            .await
            .unwrap();
    }
    let mut got = Vec::new();
    for _ in 0..20 {
        let em = tokio::time::timeout(std::time::Duration::from_secs(30), orx.recv())
            .await
            .expect("no answer within 30s")
            .expect("channel open");
        got.push(em.content["n"].as_i64().unwrap());
    }
    assert_eq!(
        got,
        (0..20).collect::<Vec<_>>(),
        "resident is a queue, not a pool"
    );
}
