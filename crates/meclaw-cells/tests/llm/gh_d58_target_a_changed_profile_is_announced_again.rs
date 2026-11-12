//! D58 target (GH #1483, contract point 2) -- a changed profile is announced
//! again.
//!
//! WHY this file exists: a cell reports its package after a params-only
//! message that sets a key of the registry (`MODEL_PACKAGE_KEYS`). The
//! profile is no such key, but a set that changes it must reach the curator
//! all the same: `window_profile` stands in a second list of triggers, so a
//! params-only message that sets or resets it is answered with the line,
//! carrying the new profile. A params-only message that moves neither stays
//! silent as before.
//!
//! Free of a paid call by construction: no message here is a turn.

use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::llm::{LlmCell, PACKAGE_ROUTE};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;
use tokio::sync::mpsc;

const OWN: &str = "/comp/seat";

async fn send(cell: &mut LlmCell, db: &mut DbConn, body: Value) -> Vec<Value> {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(OWN),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(OWN))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e.content);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d58_target_a_changed_profile_is_announced_again() {
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "start/model", "api_key": "sk-test",
        "base_url": "http://127.0.0.1:9/v1", "package_line": true,
        "window_profile": {"keep_recent": 4},
    }))
    .expect("an llm cell takes the param `window_profile`");
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());

    let changed = json!({"keep_recent": 12, "speaks": true});
    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {}, "params": {"window_profile": changed}}),
    )
    .await;
    assert_eq!(out.len(), 1, "a changed profile is announced: {out:?}");
    assert_eq!(out[0]["header"]["route"], PACKAGE_ROUTE, "{}", out[0]);
    assert_eq!(
        out[0]["window_profile"], changed,
        "the new profile: {}",
        out[0]
    );

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {}, "params": {"$reset": ["window_profile"]}}),
    )
    .await;
    assert_eq!(out.len(), 1, "a reset is announced too: {out:?}");
    assert_ne!(
        out[0]["window_profile"], changed,
        "the line says what the reset left: {}",
        out[0]
    );

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {}, "params": {"x_title": "meclaw-lab test"}}),
    )
    .await;
    assert!(
        out.is_empty(),
        "no package key, no profile: silence: {out:?}"
    );
}
