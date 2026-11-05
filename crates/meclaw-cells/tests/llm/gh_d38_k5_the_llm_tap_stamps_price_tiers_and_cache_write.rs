//! GH #1431 (D38 K5) -- the llm cell stamps the price ladder and the cache
//! write price on every successful answer.
//!
//! WHY this file exists: `priced_for` (GH #1318) replaces `hop.cost_in` /
//! `hop.cost_cached_in` with the prices of the step the answer's prompt
//! falls in. The curator's cost rule needs the OTHER steps too: the rebuilt
//! window lands below the step and is read at the base price and written at
//! the base cache-write price, while the held window is read at the step's
//! cached price. So the cell stamps the whole ladder as data
//! (`hop.price_tiers`: the base step first, `above_input` 0, then the
//! package's steps as listed) and the base cache-write price
//! (`hop.cost_cache_write`) -- both only when the package carries them, so
//! a cell on one price stamps byte for byte what it stamped before.
//!
//! Measured at the seam: a mock provider answers, the emission's header is
//! read. Nothing asserts from the inside.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// The base prices of the Haiku 5.5 row (cents per million tokens) and,
/// with `stepped`, its step above 100k as the catalogue lists it since
/// GH #1431: input 50, output 250, cache read 5, cache write 62.5.
fn birth(base_url: &str, stepped: bool) -> Value {
    let mut p = json!({
        "provider": "openai",
        "model": "stepped-model",
        "api_key": "test-key-1431",
        "base_url": base_url,
        "input_soft": 200000,
        "input_hard": 950000,
        "cost_in": 10,
        "cost_cached_in": 1,
    });
    if stepped {
        p["cost_cache_write"] = json!(12.5);
        p["price_tiers"] = json!([{
            "above_input": 100000, "cost_in": 50, "cost_out": 250,
            "cost_cached_in": 5, "cost_cache_write": 62.5
        }]);
    }
    p
}

fn born(birth: &Value, td: &TempDir) -> (LlmCell, DbConn) {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let cell = LlmCell::restored(&conn, birth, reqwest::Client::builder().build().unwrap())
        .expect("restore");
    (cell, DbConn::wrap(conn, None))
}

fn turn() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"identity": {"text": "I am the persona."}},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}],
        })))
        .build()
}

async fn answer_header(stepped: bool) -> Value {
    let one = MockOpenAI::start(vec![canned_chat_completion("hello", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = born(&birth(&format!("{}/v1", one.base_url), stepped), &td);
    let (sink, mut rx) = sink();
    cell.handle(turn(), &sink, &mut db).await;
    let em = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("an answer within the hang mark")
        .expect("the sink is open");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "{}",
        em.content
    );
    em.content["header"].clone()
}

/// The ladder is stamped as data, base step first; the base cache-write
/// price beside it. The mock reports 10 prompt tokens, so the stamped
/// `cost_in` / `cost_cached_in` stay the base step (GH #1318 unchanged).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tap_stamps_the_price_ladder_and_the_cache_write_price() {
    let h = answer_header(true).await;
    assert_eq!(
        h["price_tiers"],
        json!([
            {"above_input": 0, "cost_in": 10.0, "cost_cached_in": 1.0, "cost_cache_write": 12.5},
            {"above_input": 100000, "cost_in": 50.0, "cost_out": 250.0,
             "cost_cached_in": 5.0, "cost_cache_write": 62.5}
        ]),
        "the whole ladder, the base step first: {h}"
    );
    assert_eq!(h["cost_cache_write"], json!(12.5), "{h}");
    assert_eq!(
        h["cost_in"],
        json!(10.0),
        "a 10-token prompt is read at the base step: {h}"
    );
    assert_eq!(h["cost_cached_in"], json!(1.0), "{h}");
}

/// Without the package fields the header is what it was: no ladder, no
/// cache-write price, no key that says nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_package_fields_nothing_is_stamped() {
    let h = answer_header(false).await;
    assert!(h.get("price_tiers").is_none(), "{h}");
    assert!(h.get("cost_cache_write").is_none(), "{h}");
    assert_eq!(h["cost_in"], json!(10.0), "{h}");
    assert_eq!(h["cost_cached_in"], json!(1.0), "{h}");
}
