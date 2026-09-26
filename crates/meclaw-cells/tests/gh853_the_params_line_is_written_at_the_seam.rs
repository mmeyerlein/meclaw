//! GH #853, review of L2 M-2 and of the fix strand I-1: the params sight is
//! measured where an operator reads it -- the `meclaw::llm::params` log target
//! -- not at the formatter.
//!
//! `gh853_a_model_switch_is_visible_reversible_and_operator_only.rs` calls
//! `params_line()` and `restore_check()` itself. What it cannot see is whether
//! the lines are WRITTEN: on a run-time update (`cell.rs`, the params step), on
//! a restore (`factory.rs` `restore_cell`, through the real wake closure), the
//! warning beside a restored overlay the run-time guards would refuse, and the
//! warning when a turn carries package keys and they are not applied. The last
//! case measures the factory's `message_timeout` at the cell: the backstop
//! guard refuses a call timeout it does not clear only because the factory
//! handed the backstop over.
//!
//! The events are caught by a hand-rolled `tracing::Subscriber` -- the pattern
//! of `llm_latency_instrumentation.rs`; `tracing-subscriber` is no dependency
//! of this crate. Every test drives its own cell path and only reads events
//! naming that path, so tests sharing the collector cannot see each other.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::{LlmCell, LlmCellFactory};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, ColonyMsg, ContractView, DbConn, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const TARGET: &str = "meclaw::llm::params";

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

    /// The events on `target` that name `path` -- in the message (the params
    /// line and the turn-slot warning carry it there) or in a `path` field
    /// (the restore warning).
    pub fn on(buf: &Events, target: &str, path: &str) -> Vec<Event> {
        let needle = format!("{path} ");
        buf.lock()
            .unwrap()
            .iter()
            .filter(|e| {
                e.target == target
                    && (e.message().contains(&needle)
                        || e.fields.get("path").is_some_and(|p| p == path))
            })
            .cloned()
            .collect()
    }
}

fn birth(base_url: &str) -> Value {
    json!({
        "provider": "openai",
        "model": "start-model",
        "api_key": "test-key-853",
        "base_url": base_url,
    })
}

fn sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    )
}

fn params_only(target: &str, update: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/operator"))
        .body(Body::Inline(json!({ "params": update })))
        .build()
}

/// The real factory's spawn: Dormant, with the wake closure every first
/// message and every sleep → wake runs through.
struct Spawned {
    sender: mpsc::Sender<meclaw_core::Message>,
    receiver: Option<mpsc::Receiver<meclaw_core::Message>>,
    wake: meclaw_colony::WakeFn,
    outputs: mpsc::Receiver<CellEmission>,
    _inbox: mpsc::Receiver<ColonyMsg>,
}

fn spawn(
    path: &str,
    birth: &Value,
    cell_dir: &std::path::Path,
    backstop: Option<Duration>,
) -> Spawned {
    let (otx, orx) = mpsc::channel::<CellEmission>(16);
    let (itx, irx) = mpsc::channel::<ColonyMsg>(16);
    let kind = Arc::new(LlmCellFactory)
        .spawn_cell(
            Path::new(path),
            birth.clone(),
            otx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            itx,
            None,
            0,
            backstop,
            None,
            16,
        )
        .expect("the llm factory spawns");
    match kind {
        SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            ..
        } => Spawned {
            sender,
            receiver: Some(receiver),
            wake,
            outputs: orx,
            _inbox: irx,
        },
        SpawnedCellKind::Active { .. } => panic!("the llm cell is lazy"),
    }
}

/// An overlay row in the cell's own `cell.db`, the way an older update left it.
fn write_overlay(cell_dir: &std::path::Path, key: &str, value: &Value) {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES (?, ?, 0)",
        [key, value.to_string().as_str()],
    )
    .unwrap();
}

/// A restore -- the factory's wake -- writes the params line naming the
/// overlay, and beside it the warning for an overlay the run-time guards would
/// refuse: key and rule -- without a list, not even the refused URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_writes_the_line_and_warns_of_an_overlay_the_guards_refuse() {
    let events = collect::install();
    let path = "/restore-refused";
    let td = TempDir::new().unwrap();
    let mut s = spawn(path, &birth("http://127.0.0.1:1/v1"), td.path(), None);
    // No list: a run-time base_url other than the start endpoint is refused.
    write_overlay(td.path(), "base_url", &json!("http://127.0.0.1:2/v1"));

    let _wiring = (s.wake)(s.receiver.take().unwrap());

    let seen = collect::on(&events, TARGET, path);
    let line = seen
        .iter()
        .find(|e| e.level == tracing::Level::INFO && e.message().starts_with("llm: params "))
        .unwrap_or_else(|| panic!("no params line on the restore: {seen:?}"));
    assert!(
        line.message()
            .contains("base_url=http://127.0.0.1:2/v1[overlay]"),
        "the line shows the overlay in force: {}",
        line.message()
    );
    assert!(
        line.message().ends_with("(overlay: base_url)"),
        "{}",
        line.message()
    );

    let warn = seen
        .iter()
        .find(|e| e.level == tracing::Level::WARN)
        .unwrap_or_else(|| panic!("no warning beside the refused overlay: {seen:?}"));
    assert_eq!(warn.fields.get("path").map(String::as_str), Some(path));
    let text = warn.message();
    assert!(text.contains("base_url_allow"), "the rule is named: {text}");
    assert!(text.contains("stays in force"), "{text}");
    assert!(text.contains("$reset"), "the way back is named: {text}");
    assert!(!text.contains("127.0.0.1:2"), "the value is not: {text}");
    assert!(!text.contains("test-key-853"), "no secret: {text}");
    drop(s.sender);
}

/// The restore warning of the two other guard branches (review rev-F2, m5):
/// with a list, a restored `base_url` whose origin is not on it, and a
/// restored `external_timeout_ms` the backstop does not clear. The detail
/// names what the operator needs to act on -- the ORIGIN, never the whole URL
/// with its path, and the ms against the backstop they need -- and never a
/// key's secret.
fn restore_warning(
    path: &str,
    birth: &Value,
    key: &str,
    value: &Value,
    backstop: Option<Duration>,
) -> String {
    let events = collect::install();
    let td = TempDir::new().unwrap();
    let mut s = spawn(path, birth, td.path(), backstop);
    write_overlay(td.path(), key, value);

    let _wiring = (s.wake)(s.receiver.take().unwrap());

    let seen = collect::on(&events, TARGET, path);
    assert!(
        seen.iter()
            .any(|e| e.level == tracing::Level::INFO && e.message().starts_with("llm: params ")),
        "no params line on the restore: {seen:?}"
    );
    let warn = seen
        .iter()
        .find(|e| e.level == tracing::Level::WARN)
        .unwrap_or_else(|| panic!("no warning beside the refused overlay: {seen:?}"));
    assert_eq!(warn.fields.get("path").map(String::as_str), Some(path));
    let text = warn.message().to_string();
    assert!(text.contains("stays in force"), "{text}");
    assert!(text.contains("$reset"), "the way back is named: {text}");
    assert!(!text.contains("test-key-853"), "no secret: {text}");
    drop(s.sender);
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_warns_of_an_origin_off_the_list_by_its_origin_only() {
    let mut born = birth("http://127.0.0.1:1/v1");
    born["base_url_allow"] = json!(["http://127.0.0.1:4"]);
    let text = restore_warning(
        "/restore-foreign",
        &born,
        "base_url",
        &json!("http://127.0.0.1:3/deep-route/v1"),
        None,
    );
    assert!(text.contains("base_url_allow"), "the rule is named: {text}");
    assert!(
        text.contains("the origin http://127.0.0.1:3 of 'base_url'"),
        "the origin is named: {text}"
    );
    assert!(!text.contains("deep-route"), "the path is not: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_warns_of_a_timeout_the_backstop_does_not_clear_by_its_ms() {
    let text = restore_warning(
        "/restore-timeout",
        &birth("http://127.0.0.1:1/v1"),
        "external_timeout_ms",
        &json!(175_000),
        Some(Duration::from_millis(180_000)),
    );
    assert!(
        text.contains("'external_timeout_ms' 175000 ms"),
        "the ms are named: {text}"
    );
    assert!(
        text.contains("at least 192500 ms"),
        "and the backstop they need: {text}"
    );
}

/// An overlay the guards take restores with its line and no warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_of_an_overlay_the_guards_take_writes_the_line_only() {
    let events = collect::install();
    let path = "/restore-taken";
    let td = TempDir::new().unwrap();
    let mut s = spawn(path, &birth("http://127.0.0.1:1/v1"), td.path(), None);
    write_overlay(td.path(), "model", &json!("overlay-model"));

    let _wiring = (s.wake)(s.receiver.take().unwrap());

    let seen = collect::on(&events, TARGET, path);
    assert!(
        seen.iter().any(|e| e.level == tracing::Level::INFO
            && e.message().contains("model=overlay-model[overlay]")),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|e| e.level == tracing::Level::WARN),
        "a taken overlay is not warned of: {seen:?}"
    );
    drop(s.sender);
}

/// Every run-time update writes the line after it is applied -- the answer to
/// a params-only message is this line, not an emission (OR-SN.L2.1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_writes_the_params_line() {
    let events = collect::install();
    let path = "/update";
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut cell = LlmCell::restored(
        &conn,
        &birth("http://127.0.0.1:1/v1"),
        reqwest::Client::builder().build().unwrap(),
    )
    .expect("restore");
    let mut db = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
    let out = sink(tx);

    cell.handle(
        params_only(path, json!({"model": "second-model"})),
        &out,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err(), "an update answers with no emission");
    let seen = collect::on(&events, TARGET, path);
    assert!(
        seen.iter().any(|e| e.level == tracing::Level::INFO
            && e.message().starts_with(&format!("llm: params {path} "))
            && e.message().contains("model=second-model[overlay]")
            && e.message().ends_with("(overlay: model)")),
        "{seen:?}"
    );

    // `{"params": {}}` is the sight on demand: the same line, nothing changed.
    let before = seen.len();
    cell.handle(params_only(path, json!({})), &out, &mut db)
        .await;
    let seen = collect::on(&events, TARGET, path);
    assert_eq!(seen.len(), before + 1, "{seen:?}");
    assert!(
        seen[before]
            .message()
            .contains("model=second-model[overlay]")
    );

    // `$reset` returns the key, and the line says so.
    cell.handle(
        params_only(path, json!({"$reset": ["model"]})),
        &out,
        &mut db,
    )
    .await;
    let seen = collect::on(&events, TARGET, path);
    let last = seen.last().expect("a line after the reset");
    assert!(
        last.message().contains("model=start-model[start]"),
        "{}",
        last.message()
    );
    assert!(
        last.message().ends_with("(overlay: -)"),
        "{}",
        last.message()
    );
}

/// A turn that carries package keys in its params slot is answered as a turn,
/// and the slot is named on a warning: the keys, never their values.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_slot_with_package_keys_is_named_on_a_warning() {
    let events = collect::install();
    let path = "/turn-slot";
    let mock = MockOpenAI::start(vec![canned_chat_completion("hello", "stop")]).await;
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut cell = LlmCell::restored(
        &conn,
        &birth(&format!("{}/v1", mock.base_url)),
        reqwest::Client::builder().build().unwrap(),
    )
    .expect("restore");
    let mut db = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
    let out = sink(tx);

    let turn = MessageBuilder::new(Path::new(path))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"identity": {"text": "I am the persona."}},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}],
            "params": {"model": "smuggled-model", "temperature": 0.1},
        })))
        .build();
    cell.handle(turn, &out, &mut db).await;
    let em = rx.recv().await.expect("the turn still runs");
    assert_eq!(em.content["header"]["finish_reason"], "stop");

    let seen = collect::on(&events, TARGET, path);
    let warn = seen
        .iter()
        .find(|e| e.level == tracing::Level::WARN)
        .unwrap_or_else(|| panic!("no warning for the discarded slot: {seen:?}"));
    let text = warn.message();
    assert!(text.contains("not applied"), "{text}");
    assert!(
        text.contains("(model,temperature)"),
        "the keys are named: {text}"
    );
    assert!(!text.contains("smuggled-model"), "the value is not: {text}");
    assert!(
        !seen.iter().any(|e| e.level == tracing::Level::INFO),
        "nothing was applied, so no params line: {seen:?}"
    );
}

/// The factory hands its `message_timeout` to the cell it wakes: the backstop
/// guard refuses a call timeout the backstop does not clear. Without the
/// hand-over the cell has no backstop, and the same update would be taken
/// silently.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_factory_hands_its_backstop_to_the_cell() {
    let _events = collect::install();
    let path = "/backstop";
    let td = TempDir::new().unwrap();
    let mut s = spawn(
        path,
        &birth("http://127.0.0.1:1/v1"),
        td.path(),
        Some(Duration::from_millis(180_000)),
    );
    let _wiring = (s.wake)(s.receiver.take().unwrap());

    s.sender
        .send(params_only(path, json!({"external_timeout_ms": 175_000})))
        .await
        .expect("the woken cell takes mail");
    let em = tokio::time::timeout(Duration::from_secs(30), s.outputs.recv())
        .await
        .expect("a refusal within 30 s -- none means the cell had no backstop")
        .expect("the outputs stay open");
    assert_eq!(
        em.content["header"]["error_code"].as_str(),
        Some("invalid_input"),
        "{}",
        em.content
    );
}
