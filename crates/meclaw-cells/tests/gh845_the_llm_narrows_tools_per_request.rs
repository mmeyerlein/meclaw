//! GH #845 — the `llm` cell narrows its tool menu for ONE request.
//!
//! The menu lives durably in the brain (`system.tools.*` in `cell.db`); a
//! channel that may use only part of it used to have no way to say so short of
//! rewriting the menu, which every other channel of the same brain would then
//! see. The body slot `tool_scope: {"allow": [..], "deny": [..]}` filters the
//! menu for the request it rides on and for nothing else. The filter keeps the
//! menu's order and never re-sorts (OR-SN-31): the same scope yields the same
//! `tools` bytes, which is what keeps a provider's prefix cache warm across the
//! turns of one session (R-SN-1).
//!
//! Measured at the seam: the request bodies the mock provider received.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// One `llm` cell with its own `cell.db`, driven message by message.
struct Rig {
    cell: LlmCell,
    db: DbConn,
    _td: TempDir,
}

impl Rig {
    fn new(mock: &MockOpenAI) -> Self {
        let td = TempDir::new().unwrap();
        let params = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
            "base_url": format!("{}/v1", mock.base_url),
        }))
        .expect("params");
        let cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        Self {
            cell,
            db: DbConn::wrap(conn, None),
            _td: td,
        }
    }

    async fn send(&mut self, body: Value) {
        let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/llm"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            32,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(body))
            .build();
        self.cell.handle(msg, &sink, &mut self.db).await;
        let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("the cell must emit")
            .expect("the sink stays open");
        assert_eq!(
            em.content["header"]["finish_reason"], "stop",
            "{}",
            em.content
        );
    }
}

/// A `system.tools` subtree: one leaf per `(slot key, function name)`.
fn menu(entries: &[(&str, &str)]) -> Value {
    let mut tools = serde_json::Map::new();
    for (key, name) in entries {
        let tool = json!({"type": "function", "function": {"name": name, "parameters": {}}});
        tools.insert((*key).into(), json!({"text": tool.to_string()}));
    }
    Value::Object(tools)
}

fn turn(text: &str) -> Value {
    json!([{"origin": "user", "type": "text", "text": text}])
}

/// The function names on the wire, in wire order; `None` when `tools` is absent.
fn names(body: &Value) -> Option<Vec<String>> {
    body.get("tools").map(|t| {
        t.as_array()
            .expect("tools is an array")
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect()
    })
}

async fn wire_bodies(mock: &MockOpenAI) -> Vec<Value> {
    mock.recorded_requests()
        .await
        .into_iter()
        .map(|s| s.body)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_narrows_one_request_and_the_stored_menu_stays() {
    let mock = MockOpenAI::start(
        (0..5)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    let mut rig = Rig::new(&mock);
    rig.send(json!({"system": {"tools": menu(&[("a", "a"), ("x", "x")])},
        "messages": turn("1"), "tool_scope": {"deny": ["x"]}}))
        .await;
    rig.send(json!({"messages": turn("2")})).await;
    rig.send(json!({"messages": turn("3"), "tool_scope": {"allow": ["a"]}}))
        .await;
    rig.send(json!({"messages": turn("4"), "tool_scope": {"deny": ["nobody-knows-me"]}}))
        .await;
    rig.send(json!({"messages": turn("5"), "tool_scope": {"deny": ["a", "x"]}}))
        .await;
    let got: Vec<Option<Vec<String>>> = wire_bodies(&mock).await.iter().map(names).collect();
    let v = |n: &[&str]| Some(n.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        got,
        vec![
            v(&["a"]),      // deny x, for this request
            v(&["a", "x"]), // no scope: the stored menu, untouched
            v(&["a"]),      // allow a
            v(&["a", "x"]), // a name the menu does not know is ignored
            None,           // nothing left: a request without tools
        ]
    );
}

/// The filter keeps the menu's order and never takes the scope's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_filter_keeps_the_menu_order() {
    let mock = MockOpenAI::start(
        (0..3)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    let mut rig = Rig::new(&mock);
    // The slot keys fix the menu order c, a, x, b; the scope names functions.
    let m = menu(&[("t1", "c"), ("t2", "a"), ("t3", "x"), ("t4", "b")]);
    rig.send(json!({"system": {"tools": m}, "messages": turn("1")}))
        .await;
    rig.send(json!({"messages": turn("2"), "tool_scope": {"deny": ["x"]}}))
        .await;
    rig.send(json!({"messages": turn("3"), "tool_scope": {"allow": ["b", "c"]}}))
        .await;
    let got: Vec<Vec<String>> = wire_bodies(&mock)
        .await
        .iter()
        .map(|b| names(b).unwrap())
        .collect();
    assert_eq!(
        got,
        vec![
            vec!["c", "a", "x", "b"],
            vec!["c", "a", "b"],
            vec!["c", "b"], // menu order, not the order the allow list names them in
        ]
    );
}

/// R-SN-1's measuring point on the wire: two turns of one session with the same
/// scope carry byte-identical `tools` and a byte-identical system prefix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_scope_gives_the_same_bytes() {
    let mock = MockOpenAI::start(
        (0..2)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    let mut rig = Rig::new(&mock);
    let scope = json!({"allow": ["b", "a", "c"], "deny": ["c"]});
    rig.send(json!({
        "system": {"persona": {"text": "a helpful brain"},
            "tools": menu(&[("a", "a"), ("b", "b"), ("c", "c"), ("d", "d")])},
        "messages": turn("first"), "tool_scope": scope}))
        .await;
    rig.send(json!({"messages": turn("second"), "tool_scope": scope}))
        .await;
    let caps = mock.captured.lock().await;
    assert_eq!(caps.len(), 2);
    let raw: Vec<String> = caps
        .iter()
        .map(|c| String::from_utf8(c.body.clone()).unwrap())
        .collect();
    drop(caps);
    let parsed: Vec<Value> = raw
        .iter()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect();
    assert_eq!(names(&parsed[0]).unwrap(), vec!["a", "b"]);
    let tools0 = serde_json::to_string(&parsed[0]["tools"]).unwrap();
    let tools1 = serde_json::to_string(&parsed[1]["tools"]).unwrap();
    assert_eq!(tools0, tools1, "the same scope, the same tools bytes");
    let sys0 = serde_json::to_string(&parsed[0]["messages"][0]).unwrap();
    let sys1 = serde_json::to_string(&parsed[1]["messages"][0]).unwrap();
    assert_eq!(parsed[0]["messages"][0]["role"], "system");
    assert_eq!(sys0, sys1, "and the same system prefix");
    for r in &raw {
        assert!(
            r.contains(&tools0),
            "the bytes are on the wire as sent: {r}"
        );
        assert!(r.contains(&sys0), "{r}");
    }
}

/// OR-SN-69 (rev-L1 M-3): `tool_scope: null` means no scope — the request goes
/// out with the whole menu instead of being refused `invalid_input`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_null_scope_is_no_scope() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let mut rig = Rig::new(&mock);
    rig.send(json!({"system": {"tools": menu(&[("a", "a"), ("x", "x")])},
        "messages": turn("1"), "tool_scope": null}))
        .await;
    let got: Vec<Option<Vec<String>>> = wire_bodies(&mock).await.iter().map(names).collect();
    assert_eq!(got, vec![Some(vec!["a".to_string(), "x".to_string()])]);
}
