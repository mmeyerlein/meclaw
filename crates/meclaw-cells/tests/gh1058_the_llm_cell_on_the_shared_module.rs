//! GH #1058 — the `llm` cell on the shared credential module.
//!
//! The grant round moved out of `llm/cell.rs` into `meclaw_cells::credential`.
//! The `llm` cell must behave exactly as before — the same emissions, the same
//! receipts, the same log lines — with ONE intended change (OR-VG-4): a literal
//! `api_key` next to a grant is ignored. Before, a grant plus a key meant the
//! key won and the vault was never asked, which is the `${VAR}` fallback the
//! 22.09. ruling forbids.
//!
//! | claim | test |
//! |---|---|
//! | T7 a park-release round and a timed-out round look as before (emissions + log lines) | [`gh1058_the_llm_cell_behaves_as_before_on_the_shared_module`] |
//! | T9 transition: a literal key without a grant is presented | [`gh1058_a_literal_key_wins_while_no_grant_is_set`] |
//! | T10 OR-VG-4: a literal key beside a grant is never presented | [`gh1058_a_literal_key_is_ignored_once_a_grant_is_set`] |
//!
//! The vault half is played by the test (as in `gh457_*`): it reads the
//! recipient key out of the cell's `credential_request` and seals a stub value
//! to it. Stub values only (`stub-secret-<n>`); none of them may reach a log
//! line.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::{LlmParams, sealed};
use meclaw_colony::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// ─────────────────────────────────────────────────────────────── log capture

/// One captured tracing event: level, target, message, and the NAMES of its
/// other fields (values carry timings and ids, which differ run to run).
type Line = (String, String, String, Vec<String>);

/// A minimal subscriber that records every event. Installed per test with
/// `set_default` (thread-local) on a current-thread runtime, so the warden
/// task — spawned on the same thread — is captured too, and no other test's
/// lines can leak in. A std mutex in a TEST collector; the no-lock rule is
/// about cell state.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Line>>>);

impl Capture {
    fn lines(&self) -> Vec<Line> {
        self.0.lock().unwrap().clone()
    }
}

struct Fields {
    message: String,
    names: Vec<String>,
}

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.names.push(field.name().to_string());
        }
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut f = Fields {
            message: String::new(),
            names: Vec::new(),
        };
        event.record(&mut f);
        let meta = event.metadata();
        self.0.lock().unwrap().push((
            meta.level().to_string(),
            meta.target().to_string(),
            f.message,
            f.names,
        ));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// The lines of the credential round: everything that names the credential,
/// the box or the key. Other lines (latency phases, params) are not this
/// module's and stay out of the comparison.
fn credential_lines(all: &[Line]) -> Vec<Line> {
    all.iter()
        .filter(|(_, _, m, _)| {
            m.contains("credential") || m.contains("sealed") || m.contains("api_key")
        })
        .cloned()
        .collect()
}

fn line(level: &str, target: &str, message: &str, fields: &[&str]) -> Line {
    (
        level.to_string(),
        target.to_string(),
        message.to_string(),
        fields.iter().map(|s| (*s).to_string()).collect(),
    )
}

// ─────────────────────────────────────────────────────────────── the harness

fn cell(raw: Value) -> LlmCell {
    LlmCell::new(
        LlmParams::parse(&raw).expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    )
}

fn sink_for(tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn user_turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

fn cell_db(td: &tempfile::TempDir) -> meclaw_colony::DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    meclaw_colony::DbConn::wrap(conn, None)
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em.content);
    }
    out
}

fn request_of(seen: &[Value]) -> &Value {
    seen.iter()
        .find(|e| e["header"]["route"] == "credential_request")
        .unwrap_or_else(|| panic!("the cell never asked for its credential: {seen:?}"))
}

fn seal_for(seen: &[Value], secret: &str) -> Message {
    let args: Value = meclaw_core::serde_json::from_str(
        request_of(seen)["messages"][0]["text"]
            .as_str()
            .expect("args as text"),
    )
    .expect("args are JSON");
    let recipient = args["payload"]["recipient_key"]
        .as_str()
        .expect("recipient_key");
    let boxed = sealed::seal_to(recipient, secret.as_bytes())
        .expect("seal")
        .to_json();
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"sealed": boxed})))
        .build()
}

fn authorizations(reqs: &[meclaw_testing::mock_http::CapturedRequest]) -> Vec<String> {
    reqs.iter()
        .map(|r| r.headers.get("authorization").cloned().unwrap_or_default())
        .collect()
}

const TARGET: &str = "meclaw_cells::llm::cell";

// ═══════════════════════════════════════════════════════════════════ the locks

/// T7. One park-release round and one round that times out, with every
/// emission shape and every credential log line pinned — the same before and
/// after the move (run on the base commit as "before", see the strand report).
#[tokio::test]
async fn gh1058_the_llm_cell_behaves_as_before_on_the_shared_module() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());

    // ── the park-release round ──
    let (addr, _server, captured) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let mut c = cell(json!({
        "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
        "credential_grant_id": "grant:1058",
        "base_url": format!("http://{addr}/v1"), "external_timeout_ms": 5_000u64,
        "credential_wait_ms": 30_000u64, "credential_wait_max": 16usize,
    }));
    let (tx, mut rx) = mpsc::channel(64);
    c.handle(user_turn("ping"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    assert_eq!(
        asked.len(),
        1,
        "exactly the request, nothing else: {asked:?}"
    );
    let req = &asked[0];
    assert_eq!(
        req["header"],
        json!({"route": "credential_request", "grant_id": "grant:1058"})
    );
    let msg = &req["messages"][0];
    assert_eq!(
        (&msg["origin"], &msg["type"]),
        (&json!("assistant"), &json!("tool_call"))
    );
    assert_eq!(
        msg["id"].as_str().map(str::len),
        Some(32),
        "a uuid, simple form"
    );
    let args: Value = meclaw_core::serde_json::from_str(msg["text"].as_str().unwrap()).unwrap();
    let mut keys: Vec<&str> = args
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["grant_id", "operation", "payload"]);
    assert_eq!(args["operation"], "vault.deliver");
    assert_eq!(
        args["payload"]["recipient_key"].as_str().map(str::len),
        Some(64)
    );

    c.handle(seal_for(&asked, "stub-secret-7"), &sink_for(&tx), &mut db)
        .await;
    let answered = drain(&mut rx);
    assert!(
        answered.iter().all(|e| e["header"]["error_code"].is_null()),
        "no receipt, no reject: {answered:?}"
    );
    assert!(
        answered
            .iter()
            .any(|e| e["messages"].to_string().contains("pong")),
        "the parked turn was answered: {answered:?}"
    );
    let reqs = captured.lock().await.clone();
    assert_eq!(authorizations(&reqs), ["Bearer stub-secret-7"]);

    // ── the round that times out ──
    let td_late = tempfile::TempDir::new().unwrap();
    let mut db_late = cell_db(&td_late);
    let mut late = cell(json!({
        "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
        "credential_grant_id": "grant:1058",
        "base_url": "http://127.0.0.1:1/v1", "external_timeout_ms": 5_000u64,
        "credential_wait_ms": 50u64, "credential_wait_max": 16usize,
    }));
    late.handle(user_turn("ping"), &sink_for(&tx), &mut db_late)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let seen = drain(&mut rx);
    assert_eq!(seen.len(), 2, "one request, one receipt: {seen:?}");
    assert_eq!(seen[0]["header"]["route"], "credential_request");
    assert_eq!(seen[1]["header"]["error_code"], "credential_pending");
    assert!(
        seen[1].to_string().contains(
            "the bearer credential was requested from the access hive but did not arrive; retry"
        ),
        "the receipt's wording is unchanged: {}",
        seen[1]
    );

    // ── the log lines of both rounds ──
    let all = capture.lines();
    assert!(
        !format!("{all:?}").contains("stub-secret-7"),
        "the secret is in no log line"
    );
    assert_eq!(
        credential_lines(&all),
        vec![
            line(
                "INFO",
                TARGET,
                "llm: bearer credential received sealed and opened in RAM",
                &[]
            ),
            line(
                "WARN",
                TARGET,
                "llm: the sealed credential did not arrive in time",
                &["wait_ms"]
            ),
        ],
        "the credential log lines are the same as before the move"
    );
}

/// T9 (A8). The transition: an instance that still carries a literal key and
/// spends no grant presents the key — the 0.61 instance runs unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1058_a_literal_key_wins_while_no_grant_is_set() {
    let (addr, _server, captured) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let mut c = cell(json!({
        "provider": "openai", "model": "gpt-4o-mini", "api_key": "stub-secret-9",
        "base_url": format!("http://{addr}/v1"), "external_timeout_ms": 5_000u64,
    }));
    let (tx, mut rx) = mpsc::channel(64);
    c.handle(user_turn("ping"), &sink_for(&tx), &mut db).await;
    let seen = drain(&mut rx);
    assert!(
        seen.iter()
            .all(|e| e["header"]["route"] != "credential_request"),
        "no grant, no request: {seen:?}"
    );
    let reqs = captured.lock().await.clone();
    assert_eq!(authorizations(&reqs), ["Bearer stub-secret-9"]);
}

/// T10 (OR-VG-4). A grant and a literal key: the literal is never presented,
/// not even while the box is missing. The turn parks, the cell asks, and the
/// provider sees the vaulted value only. One WARN line names the ignored param
/// — never its value.
#[tokio::test]
async fn gh1058_a_literal_key_is_ignored_once_a_grant_is_set() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    let (addr, _server, captured) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let mut c = cell(json!({
        "provider": "openai", "model": "gpt-4o-mini", "api_key": "stub-literal-10",
        "credential_grant_id": "grant:1058",
        "base_url": format!("http://{addr}/v1"), "external_timeout_ms": 5_000u64,
        "credential_wait_ms": 30_000u64,
    }));
    let (tx, mut rx) = mpsc::channel(64);
    c.handle(user_turn("ping"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    assert_eq!(
        asked.len(),
        1,
        "the turn parked and the cell asked: {asked:?}"
    );
    assert!(
        captured.lock().await.is_empty(),
        "no provider call before the box — the literal is no fallback"
    );

    c.handle(seal_for(&asked, "stub-secret-10"), &sink_for(&tx), &mut db)
        .await;
    let reqs = captured.lock().await.clone();
    assert_eq!(authorizations(&reqs), ["Bearer stub-secret-10"]);

    let all = capture.lines();
    let printed = format!("{all:?}");
    assert!(!printed.contains("stub-literal-10"), "{printed}");
    assert!(!printed.contains("stub-secret-10"), "{printed}");
    let warns: Vec<&Line> = all
        .iter()
        .filter(|(lvl, _, m, _)| lvl == "WARN" && m.contains("api_key"))
        .collect();
    assert_eq!(warns.len(), 1, "one WARN names the ignored param: {all:?}");
    assert!(warns[0].2.contains("credential_grant_id"), "{:?}", warns[0]);
}
