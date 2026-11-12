//! D58 target (GH #1483, contract point 2, open question of the plan part) --
//! a model twin announces its package once its handover wait ends.
//!
//! WHY this file exists: a `swap_nodes` puts a twin with another model in
//! place of an `llm` cell and swings the original's edges onto it, the
//! `package` edge to the curator included. Nothing pushes the twin a params
//! update after its birth, so before this pin the curator kept the row and
//! the profile of the original -- another model id, other window bounds --
//! for as long as the twin lived. A twin with `package_line` on now sends its
//! line when its wait ends, by import or by expiry, before any parked turn
//! runs. A twin with `package_line` off stays silent, as before.
//!
//! Free of a paid call by construction: the twin holds no credential and no
//! turn reaches a provider.

use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::llm::{LlmCell, PACKAGE_ROUTE};
use meclaw_colony::{DbConn, StatefulCell};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

const TWIN: &str = "/comp/seat-twin";

fn cell(raw: Value) -> LlmCell {
    LlmCell::new(
        LlmParams::parse(&raw).expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    )
}

fn twin(package_line: bool) -> LlmCell {
    cell(json!({
        "provider": "openai", "model": "twin/model", "api_key": "",
        "credential_grant_id": "grant:twin", "base_url": "http://127.0.0.1:1/v1",
        "handover_wait_ms": 5_000, "package_line": package_line, "input_soft": 64_000,
        "system_order": ["identity"],
        "window_profile": {"keep_recent": 6},
    }))
}

fn db_at(td: &tempfile::TempDir, name: &str) -> DbConn {
    let conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join(name)).expect("cell.db");
    DbConn::wrap(conn, None)
}

fn sink(at: &str, tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new(at),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn to(at: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(at))
        .body(Body::Inline(body))
        .build()
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em.content);
    }
    out
}

/// The original's handover, as its export answers it.
async fn handover_of_an_original(td: &tempfile::TempDir) -> Value {
    let mut db = db_at(td, "original.db");
    let mut original = cell(json!({
        "provider": "openai", "model": "original/model", "api_key": "k",
        "base_url": "http://127.0.0.1:1/v1", "package_line": true,
    }));
    let (tx, mut rx) = mpsc::channel(8);
    let sink = sink("/comp/seat", &tx);
    original
        .handle(
            to(
                "/comp/seat",
                json!({"system": {"identity": {"text": "I am Sam."}}}),
            ),
            &sink,
            &mut db,
        )
        .await;
    let ask = MessageBuilder::new(Path::new("/comp/seat"))
        .reply_to(Path::new(TWIN))
        .body(Body::Inline(json!({"handover": {"operation": "export"}})))
        .build();
    original.handle(ask, &sink, &mut db).await;
    rx.try_recv().expect("the export answer").content
}

fn is_twin_line(line: &Value) -> bool {
    line["header"]["route"] == PACKAGE_ROUTE
        && line["header"]["package_of"] == TWIN
        && line["package"]["model"] == "twin/model"
        && line["package"]["input_soft"] == 64_000
        && line["window_profile"] == json!({"keep_recent": 6})
        && line["system_order"] == json!(["identity"])
}

#[tokio::test(start_paused = true)]
async fn gh_d58_target_a_twin_announces_its_package_after_the_handover() {
    // By import.
    let td = tempfile::TempDir::new().unwrap();
    let handover = handover_of_an_original(&td).await;
    let mut db = db_at(&td, "twin.db");
    let mut cell = twin(true);
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink(TWIN, &tx);
    cell.handle(to(TWIN, handover), &sink, &mut db).await;
    let out = drain(&mut rx);
    assert_eq!(out.len(), 1, "one line after the import: {out:?}");
    assert!(is_twin_line(&out[0]), "the twin's own package: {}", out[0]);

    // By expiry.
    let mut db = db_at(&td, "twin-expired.db");
    let mut cell = twin(true);
    cell.handle(
        to(TWIN, json!({"handover": {"operation": "expire"}})),
        &sink,
        &mut db,
    )
    .await;
    let out = drain(&mut rx);
    assert_eq!(out.len(), 1, "one line after the expiry: {out:?}");
    assert!(is_twin_line(&out[0]), "the twin's own package: {}", out[0]);

    // A second expiry of a wait that already ended: silence.
    cell.handle(
        to(TWIN, json!({"handover": {"operation": "expire"}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "the wait ended once");

    // Without `package_line`: silence, as before.
    let handover = handover_of_an_original(&td).await;
    let mut db = db_at(&td, "twin-silent.db");
    let mut cell = twin(false);
    cell.handle(to(TWIN, handover), &sink, &mut db).await;
    let out = drain(&mut rx);
    assert!(out.is_empty(), "no line without `package_line`: {out:?}");
}
