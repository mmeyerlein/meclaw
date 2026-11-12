//! D58 target (GH #1483, contract point 2) -- an llm cell carries its window
//! profile in the package line.
//!
//! WHY this file exists: what a cell uses its model for is written by its
//! composite on the cell (`window_profile`, an uninterpreted JSON value like
//! `reasoning` or `provider_extra`), and the curator learns it over the same
//! lane as the catalogue row. The package line of a cell with
//! `package_line` on carries the profile in a body slot of its own,
//! `window_profile`, beside `package`, and the cell's `system_order` in the
//! slot `system_order`, so the curator can write the system part in the
//! order the provider gets it. The keys of `package` stay the registry's.
//!
//! Free of a paid call by construction: no message here is a turn.

use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::llm::{LlmCell, PACKAGE_LINE_KEYS, PACKAGE_ROUTE};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;
use tokio::sync::mpsc;

const OWN: &str = "/os/orgs/acme/members/alex/assistants/scribe/comp/seat";

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
async fn gh_d58_target_an_llm_cell_carries_its_window_profile_in_the_package_line() {
    let profile = json!({"keep_recent": 6, "window": "turn", "speaks": true,
                         "instructions": {"slot": "voice_rules", "text": "Be short."}});
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "start/model", "api_key": "sk-test",
        "base_url": "http://127.0.0.1:9/v1", "package_line": true,
        "system_order": ["identity", "instructions"],
        "window_profile": profile,
    }))
    .expect("an llm cell takes the param `window_profile`");
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());

    // The question: the empty params slot.
    let out = send(&mut cell, &mut db, json!({"system": {}, "params": {}})).await;
    assert_eq!(out.len(), 1, "one line: {out:?}");
    let line = &out[0];
    assert_eq!(line["header"]["route"], PACKAGE_ROUTE, "{line}");
    assert_eq!(
        line["window_profile"], profile,
        "the profile in its own slot: {line}"
    );
    assert_eq!(
        line["system_order"],
        json!(["identity", "instructions"]),
        "the cell's order of the system part beside it: {line}"
    );
    let mut keys: Vec<&str> = line["package"]
        .as_object()
        .expect("package is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut want: Vec<&str> = PACKAGE_LINE_KEYS.to_vec();
    want.sort_unstable();
    assert_eq!(
        keys, want,
        "the package keeps the registry's keys only: {line}"
    );

    // A push of the registry carries the same slots.
    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {}, "params": {"model": "test/m2", "input_soft": 90000}}),
    )
    .await;
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["window_profile"], profile, "{}", out[0]);
    assert_eq!(out[0]["package"]["model"], "test/m2", "{}", out[0]);
}
