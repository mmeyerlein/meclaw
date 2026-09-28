//! GH #862 (R-SN-9) — a model push works only in the cell it names.
//!
//! The registry addresses a push by `hop.subscriber`, the path of the `llm`
//! cell it resolved a package for. Before 0.47.0 the cell never read that
//! address: any edge that forwarded the registry's `update` lane into another
//! composite's `in_model` door moved that composite's brain too (OR-SN.L2a.14,
//! pinned then as a broker question in
//! `gh855_the_model_road_is_a_form_at_the_gate.rs`). What is claimed, measured
//! where it lands -- the cell's own `cell.db`, its emission, the request the
//! next turn sends to the provider:
//!
//! 1. a `params` slot on a message whose `hop.subscriber` names another cell
//!    is not applied; a params-only message is refused loudly
//!    (`invalid_input`, the detail names the address, and no `refused_*` key:
//!    that refusal is not this cell's to report back, GH #863), and a
//!    `WARN` line on `meclaw::llm::params` says the same;
//! 2. the empty string and a non-string address are another cell (fail
//!    closed);
//! 3. on a turn the slot is skipped whole and the turn runs on;
//! 4. a push addressed to this cell, a message without an address (the
//!    operator's `POST /messages`, `argus`, `steward`) and a `system` write
//!    that carries an address (an `affinity` push) apply as before;
//! 5. in a colony, a forward of the registry's road into a second brain
//!    moves only the brain the push names.
//!
//! The own address is the path the substrate stamps on the output sink
//! (`OutputSink::sender_path`, GH #132), never a value from the message.
//!
//! Beside it, the `base_url` half of GH #863 (OR-SN.L2.4), at the same seam: a
//! start `base_url` with a trailing `/` is the same endpoint as the one
//! without, and the request path is appended without `//`.
//!
//! Free of a paid call by construction: the only provider is the in-process
//! mock. R2b / GH #49: the colony test skips on a tree without the templates.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The log target of the params sight (`llm::package::PARAMS_LOG_TARGET`).
const TARGET: &str = "meclaw::llm::params";

/// The cell under test, as the substrate stamps it on the sink.
const OWN: &str = "/org/talky/brain";
/// Another subscriber's path -- a brain in another composite.
const OTHER: &str = "/org/cogny/brain";
/// What the cell is born on.
const START: &str = "start/model";
/// What a push moves it to.
const M2: &str = "test/m2";

mod collect {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    /// One captured event: target, level, field name → rendered value.
    #[derive(Clone, Debug)]
    pub struct Event {
        pub target: String,
        pub level: tracing::Level,
        pub fields: HashMap<String, String>,
    }

    impl Event {
        pub fn message(&self) -> &str {
            self.fields.get("message").map(String::as_str).unwrap_or("")
        }
    }

    pub type Events = Arc<Mutex<Vec<Event>>>;

    #[derive(Default)]
    struct Grab(HashMap<String, String>);

    impl tracing::field::Visit for Grab {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
    }

    struct Collector(Events);

    impl tracing::Subscriber for Collector {
        fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _a: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _i: &tracing::Id, _v: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _i: &tracing::Id, _f: &tracing::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut grab = Grab::default();
            event.record(&mut grab);
            self.0.lock().unwrap().push(Event {
                target: event.metadata().target().to_string(),
                level: *event.metadata().level(),
                fields: grab.0,
            });
        }
        fn enter(&self, _i: &tracing::Id) {}
        fn exit(&self, _i: &tracing::Id) {}
    }

    static EVENTS: OnceLock<Events> = OnceLock::new();

    /// Install the collector once per test binary and return the shared buffer.
    pub fn install() -> Events {
        EVENTS
            .get_or_init(|| {
                let buf: Events = Arc::new(Mutex::new(Vec::new()));
                let _ = tracing::subscriber::set_global_default(Collector(buf.clone()));
                buf
            })
            .clone()
    }

    /// The events on `target` whose message names `path` followed by a space.
    pub fn on(buf: &Events, target: &str, path: &str) -> Vec<Event> {
        let needle = format!("{path} ");
        buf.lock()
            .unwrap()
            .iter()
            .filter(|e| e.target == target && e.message().contains(&needle))
            .cloned()
            .collect()
    }
}

// ─────────────────────────────────────────────────────────── the cell alone

fn birth(base_url: &str) -> Value {
    json!({
        "provider": "openai",
        "model": START,
        "api_key": "test-key-862",
        "base_url": base_url,
        "system_order": ["identity"],
        "system_writable": ["identity"],
    })
}

/// A real `llm` cell on a fresh `cell.db`, born the way the factory births it
/// (the birth params are its start value).
fn a_cell(td: &TempDir, birth: &Value) -> (LlmCell, DbConn) {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let cell = LlmCell::restored(&conn, birth, reqwest::Client::builder().build().unwrap())
        .expect("restore");
    (cell, DbConn::wrap(conn, None))
}

fn hop_of(hop: Option<Value>) -> Map<String, Value> {
    hop.and_then(|h| h.as_object().cloned()).unwrap_or_default()
}

/// Deliver one body into the cell as the colony would: the sink carries the
/// cell's own registry path and the input headers, the message the hop.
/// Returns every emission of the call.
async fn deliver(
    cell: &mut LlmCell,
    db: &mut DbConn,
    own: &str,
    hop: Option<Value>,
    body: Value,
) -> Vec<CellEmission> {
    let headers = Headers::from_parts(Map::new(), hop_of(hop));
    let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        tx,
        Path::new(own),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        headers.clone(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(own))
        .reply_to(Path::new("/observer"))
        .headers(headers)
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e);
    }
    out
}

/// The package push the registry sends (B-8 form).
fn push(update: Value) -> Value {
    json!({"system": {}, "params": update})
}

fn turn(text: &str) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": text}]})
}

/// The keys of the cell's params overlay in its own `cell.db`.
async fn overlay_keys(db: &mut DbConn) -> Vec<String> {
    db.call(|conn| -> rusqlite::Result<Vec<String>> {
        let mut st = conn.prepare("SELECT key FROM params ORDER BY key")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    })
    .await
    .unwrap()
}

async fn slot(db: &mut DbConn, slot_path: &str) -> Option<String> {
    let p = slot_path.to_string();
    db.call(move |conn| -> rusqlite::Result<Option<String>> {
        Ok(conn
            .query_row(
                "SELECT value FROM system WHERE slot_path = ?",
                rusqlite::params![p],
                |r| r.get::<_, String>(0),
            )
            .ok())
    })
    .await
    .unwrap()
}

fn header(e: &CellEmission) -> Map<String, Value> {
    e.content["header"].as_object().cloned().unwrap_or_default()
}

fn detail(e: &CellEmission) -> String {
    e.content["meta"]["error"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The one refusal a foreign params-only push earns: `invalid_input`, the
/// detail names the address, and no `refused_*` key -- the refusal of a push
/// that was never this cell's is no report to the registry (GH #863).
fn assert_foreign_refusal(out: &[CellEmission], address: &str) {
    assert_eq!(out.len(), 1, "exactly one emission: {out:?}");
    let h = header(&out[0]);
    assert_eq!(h.get("finish_reason"), Some(&json!("error")), "{h:?}");
    assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
    assert!(
        !h.contains_key("refused_subscriber") && !h.contains_key("refused_model"),
        "a foreign push's refusal carries no refused_* key: {h:?}"
    );
    let d = detail(&out[0]);
    assert!(d.contains(address), "the detail names the address: {d}");
    assert!(d.contains("not for this cell"), "{d}");
    assert!(!d.contains("test-key-862"), "no secret: {d}");
}

/// One turn through the cell; the request it sent to the provider.
async fn turn_request(
    cell: &mut LlmCell,
    db: &mut DbConn,
    mock: &MockOpenAI,
    own: &str,
) -> mock_openai::OpenAiRequestSnapshot {
    let before = mock.recorded_requests().await.len();
    let out = deliver(cell, db, own, None, turn("hello")).await;
    assert!(
        out.iter()
            .all(|e| header(e).get("finish_reason") != Some(&json!("error"))),
        "the turn is answered: {out:?}"
    );
    let mut reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), before + 1, "one provider call per turn");
    reqs.pop().unwrap()
}

/// Claim 1. A push addressed to another cell moves nothing here, and says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_addressed_to_another_cell_is_not_applied() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth(&format!("{}/v1", mock.base_url)));

    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        Some(json!({"subscriber": OTHER, "route": "in_model"})),
        push(json!({"model": M2, "max_tokens": 777})),
    )
    .await;

    assert_foreign_refusal(&out, OTHER);
    assert!(
        overlay_keys(&mut db).await.is_empty(),
        "nothing is written to the overlay"
    );
    let req = turn_request(&mut cell, &mut db, &mock, OWN).await;
    assert_eq!(
        req.body["model"], START,
        "the cell runs on its start: {}",
        req.body
    );
    assert_ne!(req.body["max_tokens"], 777, "{}", req.body);
}

/// Claim 2. The empty string and a non-string are another cell: an address
/// that is not this cell's path is not this cell's, whatever it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_or_odd_address_is_another_cell() {
    for (address, shown) in [(json!(""), "''"), (json!(5), "'5'")] {
        let td = TempDir::new().unwrap();
        let (mut cell, mut db) = a_cell(&td, &birth("http://127.0.0.1:1/v1"));
        let out = deliver(
            &mut cell,
            &mut db,
            OWN,
            Some(json!({"subscriber": address})),
            push(json!({"model": M2})),
        )
        .await;
        assert_foreign_refusal(&out, shown);
        assert!(
            overlay_keys(&mut db).await.is_empty(),
            "{address}: nothing applied"
        );
    }
}

/// Claim 3. On a turn the foreign slot is skipped whole -- a key a turn may
/// otherwise carry (`http_referer`) included -- and the turn runs on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_with_a_foreign_address_runs_without_its_slot() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth(&format!("{}/v1", mock.base_url)));

    let mut body = turn("hello");
    body["params"] = json!({"http_referer": "https://foreign.example"});
    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        Some(json!({"subscriber": OTHER})),
        body,
    )
    .await;

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "the turn ran: {out:?}");
    assert!(
        !reqs[0].headers.contains_key("http-referer"),
        "the foreign slot did not reach the wire: {:?}",
        reqs[0].headers
    );
    assert!(
        out.iter()
            .all(|e| header(e).get("finish_reason") != Some(&json!("error"))),
        "the turn is answered, not refused: {out:?}"
    );
    assert!(
        overlay_keys(&mut db).await.is_empty(),
        "nothing is written to the overlay"
    );
}

/// Claim 1, the sight: the refusal is on the params log target, naming this
/// cell and the address, and no secret.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refusal_names_the_address_on_a_warning() {
    let events = collect::install();
    let own = "/org/warned/brain";
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth("http://127.0.0.1:1/v1"));

    let _ = deliver(
        &mut cell,
        &mut db,
        own,
        Some(json!({"subscriber": OTHER})),
        push(json!({"model": M2})),
    )
    .await;

    let seen = collect::on(&events, TARGET, own);
    let warn = seen
        .iter()
        .find(|e| e.level == tracing::Level::WARN)
        .unwrap_or_else(|| panic!("no warning on {TARGET}: {seen:?}"));
    let text = warn.message();
    assert!(text.contains(OTHER), "the address is named: {text}");
    assert!(text.contains("not for this cell"), "{text}");
    assert!(!text.contains("test-key-862"), "no secret: {text}");
    assert!(
        !seen.iter().any(
            |e| e.level == tracing::Level::INFO && e.message().contains(&format!("model={M2}"))
        ),
        "no params line for an update that was not applied: {seen:?}"
    );
}

/// Claim 4, counter-proof (green before the fix). A push addressed to this
/// cell applies as before: silent, in the overlay, on the next call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_addressed_to_this_cell_is_applied_and_silent() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth(&format!("{}/v1", mock.base_url)));

    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        Some(json!({"subscriber": OWN, "route": "in_model"})),
        push(json!({"model": M2})),
    )
    .await;

    assert!(out.is_empty(), "a push answers with no emission: {out:?}");
    assert_eq!(overlay_keys(&mut db).await, vec!["model".to_string()]);
    let req = turn_request(&mut cell, &mut db, &mock, OWN).await;
    assert_eq!(req.body["model"], M2, "{}", req.body);
}

/// Claim 4, counter-proof (green before the fix). A message without an
/// address is the operator's and applies as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_without_an_address_is_the_operators_and_applies_as_before() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth(&format!("{}/v1", mock.base_url)));

    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        Some(json!({"route": "in_model"})),
        push(json!({"model": M2})),
    )
    .await;

    assert!(out.is_empty(), "a push answers with no emission: {out:?}");
    assert_eq!(overlay_keys(&mut db).await, vec!["model".to_string()]);
    let req = turn_request(&mut cell, &mut db, &mock, OWN).await;
    assert_eq!(req.body["model"], M2, "{}", req.body);
}

/// Claim 4, counter-proof (green before the fix). Only the `params` slot is
/// bound: an `affinity` push is a `system` write addressed to its subscriber,
/// and it lands whatever that address says.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_write_with_an_address_is_not_a_push() {
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth("http://127.0.0.1:1/v1"));

    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        Some(json!({"subscriber": "/main/consumer", "route": "in_push"})),
        json!({"system": {"identity": {"text": "written by a push"}}}),
    )
    .await;

    assert!(out.is_empty(), "a system write stays silent: {out:?}");
    assert_eq!(
        slot(&mut db, "identity").await.as_deref(),
        Some(r#"{"text":"written by a push"}"#)
    );
}

// ──────────────────────────────────────────────── base_url (GH #863, R1 half)

/// OR-SN.L2.4. A start `base_url` with a trailing `/` calls
/// `/v1/chat/completions` (not `/v1//…`), and a package naming the same
/// endpoint without the `/` is its own start value coming back: applied, not
/// refused, although no list exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_endpoint_with_a_slash_takes_the_same_endpoint_without_one() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("one", "stop"),
        canned_chat_completion("two", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = a_cell(&td, &birth(&format!("{}/v1/", mock.base_url)));

    let req = turn_request(&mut cell, &mut db, &mock, OWN).await;
    assert_eq!(req.path, "/v1/chat/completions", "the start alone");

    let out = deliver(
        &mut cell,
        &mut db,
        OWN,
        None,
        push(json!({"base_url": format!("{}/v1", mock.base_url), "model": M2})),
    )
    .await;
    assert!(
        out.is_empty(),
        "the start endpoint in another spelling is no change the guard refuses: {:?}",
        out.iter().map(detail).collect::<Vec<_>>()
    );

    let req = turn_request(&mut cell, &mut db, &mock, OWN).await;
    assert_eq!(req.path, "/v1/chat/completions");
    assert_eq!(req.body["model"], M2, "the package applied: {}", req.body);
}

// ──────────────────────────────────────────────────────────── in a colony

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped() -> bool {
    ["llm-registry", "talky"]
        .iter()
        .all(|t| templates_root().join(t).join("config.json").exists())
}

/// The shipped template, copied cell by cell, refs resolved (the helper of
/// `gh855_an_override_reaches_the_brain_through_its_door.rs`).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let name = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template")
            .split('@')
            .next()
            .unwrap_or_default()
            .to_string();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": true}}, "hop": hop},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the registry and two composites.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A harness text becomes the surface turn, on the ingress lane.
const SURFACE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn", "chat_id": "c-862"},
                             "messages": d.get("messages", [])}))
"#;

/// A harness text is a registry command, as a tool_call turn.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": "operator"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": raw}]}))
"#;

/// The boot-graph edge into `./store`, for the one catalogue row this file adds.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1", "text": raw}]}))
"#;

const NEVER: &str = "0 0 0 1 1 *";
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;
/// The brain the registry's subscriber row names.
const BRAIN_A: &str = "/talky/brain";

fn colony_config() -> Value {
    let turn_edge = |from: &str, to: &str| {
        json!({"from": from, "to": to,
               "condition": "has(hop.route) && hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"},
                            "set_context": {"channel": "hop.chat_id",
                                            "audience_set": AUDIENCE_CEL}}})
    };
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        turn_edge("./surface", "./talky/session-keeper"),
        turn_edge("./surface_b", "./talky_b/session-keeper"),
        {"from": "./talky/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./talky_b/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'answer'"},
        {"from": "./talky_b", "to": "/park_b",
         "condition": "has(hop.route) && hop.route != 'answer'"},
        {"from": "./operator", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"actor": "hop.actor"}}},
        {"from": "./llm_registry", "to": "/acks",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./llm_registry", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // THE ROAD onto A's door, addressed by A's path -- as the builder draws it.
        {"from": "./llm_registry", "to": "./talky",
         "condition": format!("has(hop.route) && hop.route == 'update' && \
                               has(hop.subscriber) && hop.subscriber == '{BRAIN_A}'"),
         "modifier": {"set_hop": {"route": "'in_model'"}}},
        // OR-SN.L2a.14: a forward of the same road into B's door, whatever
        // the address -- the edge the submit gate can only ask about.
        {"from": "./llm_registry", "to": "./talky_b",
         "condition": "has(hop.route) && hop.route == 'update' && has(hop.subscriber)",
         "modifier": {"set_hop": {"route": "'in_model'"}}},
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/acks",
         "condition": "context.registry_origin == 'admin'"}
    ]}}})
}

fn build_tree(td: &TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &colony_config());
    for surface in ["surface", "surface_b"] {
        write(
            root,
            &format!("main/{surface}/config.json"),
            &code_cell(
                SURFACE,
                &["turn"],
                json!({"chat_id": {"type": "string", "required": false}}),
            ),
        );
    }
    write(
        root,
        "main/operator/config.json",
        &code_cell(
            OPERATOR,
            &["command"],
            json!({"actor": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/admin/config.json",
        &code_cell(ADMIN, &["astore"], json!({})),
    );
    copy_cells(
        &templates_root().join("llm-registry"),
        &root.join("main/llm_registry"),
    );
    for (composite, schedule) in [
        ("talky", "0190a3f2-0000-7000-8000-000000000862"),
        ("talky_b", "0190a3f2-0000-7000-8000-000000008620"),
    ] {
        copy_cells(
            &templates_root().join("talky"),
            &root.join(format!("main/{composite}")),
        );
        patch(
            root,
            &format!("main/{composite}/session-keeper/night/config.json"),
            |v| {
                v["params"]["schedules"][0]["schedule_id"] = json!(schedule);
                v["params"]["schedules"][0]["cron"] = json!(NEVER);
            },
        );
        patch(root, &format!("main/{composite}/brain/config.json"), |v| {
            v["params"]["base_url"] = json!(base_url);
            v["params"]["model"] = json!(START);
        });
        // GH #889: each talky carries its own curator, and the curator's
        // summarizer is an `llm` cell whose model is `${ctx.model}` -- an
        // instantiation-side substitution a tree booted from disk cannot
        // resolve. It names the mock here; a run this short never reaches a
        // rebuild, and the pushes below name a brain, so it is never called.
        patch(
            root,
            &format!("main/{composite}/curator/summarizer/config.json"),
            |v| {
                v["params"]["base_url"] = json!(base_url);
                v["params"]["model"] = json!(START);
            },
        );
    }
}

struct Ports {
    answers: mpsc::Receiver<Message>,
    acks: mpsc::Receiver<Message>,
    _park: mpsc::Receiver<Message>,
    park_b: mpsc::Receiver<Message>,
}

async fn boot(td: &TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (a_tx, a_rx) = mpsc::channel::<Message>(64);
    let (k_tx, k_rx) = mpsc::channel::<Message>(64);
    let (p_tx, p_rx) = mpsc::channel::<Message>(256);
    let (b_tx, b_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(a_tx.clone()))
        .await;
    h.spawn(Path::new("/acks"), move || CaptureCell::new(k_tx.clone()))
        .await;
    h.spawn(Path::new("/park"), move || CaptureCell::new(p_tx.clone()))
        .await;
    h.spawn(Path::new("/park_b"), move || CaptureCell::new(b_tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (
        h,
        Ports {
            answers: a_rx,
            acks: k_rx,
            _park: p_rx,
            park_b: b_rx,
        },
    )
}

fn text_to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(200)
        .build()
}

fn turn_json(m: &Message) -> Value {
    let Body::Inline(b) = &m.body else {
        panic!("inline expected")
    };
    meclaw_core::serde_json::from_str(b["messages"][0]["text"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null)
}

async fn recv(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what} never arrived"))
}

async fn command(h: &ColonyHandle, p: &mut Ports, cmd: Value) -> Value {
    h.send(text_to("/operator", &cmd.to_string())).await;
    loop {
        let m = recv(&mut p.acks, "the ack").await;
        if m.headers.hop.get("route").and_then(|v| v.as_str()) == Some("ack") {
            return turn_json(&m);
        }
    }
}

/// One turn through one composite; the model its provider call named.
async fn model_of_a_turn(
    h: &ColonyHandle,
    p: &mut Ports,
    mock: &MockOpenAI,
    surface: &str,
) -> Value {
    let before = mock.recorded_requests().await.len();
    h.send(text_to(surface, "which model?")).await;
    let _ = recv(&mut p.answers, "the answer").await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), before + 1, "one provider call per turn");
    reqs[before].body["model"].clone()
}

/// Claim 5. The registry pushes a package to A; a forward of the same road
/// also delivers it to B's door. A runs on the package, B on its start, and
/// B's refusal leaves its composite as an `invalid_input` from its brain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forward_into_another_brain_moves_nothing() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("a", "stop"),
        canned_chat_completion("b", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
    let (h, mut p) = boot(&td).await;

    h.send(text_to(
        "/admin",
        &json!({"operation": "insert", "table": "models",
                "row": {"model_id": M2, "provider": "gateway", "base_url": "",
                        "wire_dialect": "", "context_window": 32000, "cost_in": 1,
                        "cost_out": 1, "caps": {}, "traits": {}, "status": "active",
                        "note": "TEST ROW", "package": {}, "prompt": ""}})
        .to_string(),
    ))
    .await;
    let _ = recv(&mut p.acks, "the catalogue write").await;
    let ack = command(
        &h,
        &mut p,
        json!({"op": "subscribe", "cell_path": BRAIN_A, "start_model": START}),
    )
    .await;
    assert_eq!(ack["outcome"], "accepted", "{ack}");
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": BRAIN_A, "model_id": M2}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");

    assert_eq!(
        model_of_a_turn(&h, &mut p, &mock, "/surface").await,
        json!(M2),
        "the brain the push names moved"
    );
    assert_eq!(
        model_of_a_turn(&h, &mut p, &mock, "/surface_b").await,
        json!(START),
        "the brain the push was forwarded to did not"
    );

    // B's refusal left its composite, from its brain, as invalid_input.
    let refused = loop {
        let m = recv(&mut p.park_b, "B's refusal").await;
        let hop = &m.headers.hop;
        if hop.get("error_code") == Some(&json!("invalid_input")) {
            break m;
        }
    };
    assert_eq!(
        refused.headers.hop.get("error_source"),
        Some(&json!("brain")),
        "{:?}",
        refused.headers.hop
    );
    h.shutdown().await;
}
