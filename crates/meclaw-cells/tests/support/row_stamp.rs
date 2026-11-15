//! GH #1532 (d58-3 raten): the catalog row of the raten pins and the header
//! of one real answer of an `llm` cell born on it (its provider a mock). The
//! live pin and the push's pin both read the same stamp -- review raten I1:
//! no stamp built by hand.

#![allow(dead_code)]

use crate::mock_openai::{MockOpenAI, canned_chat_completion};
use meclaw_cells::llm::LlmCell;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The row of these pins: a window, the rate 4 and the image cost 1600 --
/// neither the core's 3 nor its 1024.
pub const ROW_SOFT: i64 = 250_000;
pub const ROW_RATE: f64 = 4.0;
pub const ROW_IMAGE: i64 = 1600;

pub fn row() -> Value {
    json!({"model": "test/row-model", "input_soft": ROW_SOFT, "input_hard": 1_000_000,
           "chars_per_token": ROW_RATE, "tokens_per_image": ROW_IMAGE})
}

/// The header of one real answer of an `llm` cell born on `row()`.
pub async fn real_stamp() -> Map<String, Value> {
    let mock = MockOpenAI::start(vec![canned_chat_completion("noted", "stop")]).await;
    let td = TempDir::new().unwrap();
    let birth = json!({
        "provider": "openai", "model": "test/row-model", "api_key": "test-key-1532",
        "base_url": format!("{}/v1", mock.base_url),
        "x_title": "meclaw-lab raten-live", "http_referer": "https://meclaw.ai",
        "input_soft": ROW_SOFT, "input_hard": 1_000_000,
        "chars_per_token": ROW_RATE, "tokens_per_image": ROW_IMAGE,
    });
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut cell = LlmCell::restored(&conn, &birth, reqwest::Client::builder().build().unwrap())
        .expect("restore");
    let mut db = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let turn = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"identity": {"text": "I am the persona."}},
            "messages": [{"origin": "user", "type": "text", "text": "Who is my son?"}],
        })))
        .build();
    cell.handle(turn, &sink, &mut db).await;
    let em = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("an answer within the hang mark")
        .expect("the sink is open");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "{}",
        em.content
    );
    em.content["header"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}
