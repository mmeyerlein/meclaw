//! GH #1353 — a params push moves only the cell it names, and that cell answers.
//!
//! One rule in `params_overlay`, hung into every overlay type. What is claimed,
//! measured at the cell's own `cell.db` and its emissions:
//!
//! 1. a `params` slot whose `hop.subscriber` is another cell's path -- the empty
//!    string and a non-string included -- moves nothing and is refused loudly,
//!    without a `refused_*` key (that refusal is no report to the sender);
//! 2. a push on the lane `in_params` addressed to this cell is answered: applied
//!    (`hop.params_answer == 'params_applied'`, `hop.applied_subscriber`) or refused
//!    (`'params_refused'`, `hop.refused_subscriber`, the type's own `error_code`, the
//!    detail in `meta.detail`);
//! 3. without a subscriber (the operator's direct push) and on any other lane
//!    (the registry's `in_model`) accepting stays silent, as before;
//! 4. the next life of the type is rebuilt from what an update left in the file;
//! 5. the lane crosses every seal, so it carries an update or nothing: work on it
//!    is not run, whoever it names, with a `params` slot beside it or without.
//!
//! What the running cell does with a value it took is each type's own pin.
//!
//! The answer names itself on keys of its own, not on `route`: the shipped cells
//! declare `route` with a closed value list, and the hop of the push is not
//! inherited by the answer (ADR-0001), so nothing else tells it apart.

use meclaw_cells::harness::db::setup_harness_schema;
use meclaw_cells::harness::{HarnessCell, HarnessOverlay, HarnessParams};
use meclaw_cells::llm::{LlmCell, LlmParams};
use meclaw_cells::mcp::McpOverlay;
use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::params_overlay::{OverlayParams, restore};
use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::setup_proxy_schema;
use meclaw_cells::proxy::io::ProxyReconfig;
use meclaw_cells::proxy::meclaw::{cell::MeclawCell, params::MeclawParams};
use meclaw_cells::proxy::params::ProxyOverlay;
use meclaw_cells::proxy::slack::cell::SlackCell;
use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::db::setup_slack_schema;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_cells::proxy::webhook::cell::WebhookCell;
use meclaw_cells::proxy::webhook::params::WebhookParams;
use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_cells::subcolony::{SubcolonyCell, SubcolonyOverlay, SubcolonyParams};
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::setup_timer_schema;
use meclaw_cells::timer::params::TimerOverlay;
use meclaw_cells::voice::cell::VoiceCell;
use meclaw_cells::voice::contract::ProviderTimeouts;
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::{VoiceOverlay, VoiceParams};
use meclaw_cells::voice::providers::build_stt;
use meclaw_cells::voice::wire::Mode;
use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::params::WebOverlay;
use meclaw_cells::web::{AssetMap, WebCell, WebIo, WebParams, WebReconfig};
use meclaw_colony::persist::open_or_create_cell_db;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::surfaces::SurfaceRegistry;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json, to_value};
use meclaw_core::{
    Body, CellEmission, CompiledEmits, EmitsBlock, Headers, MessageBuilder, OutputSink, Path, Uuid,
    validate_emits,
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

/// The cell under test, as the substrate stamps it on the sink.
const OWN: &str = "/org/hive/cell";
/// Another cell of the same colony.
const OTHER: &str = "/org/other/cell";

/// One overlay type under test.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Llm,
    Timer,
    Store,
    Voice,
    Proxy,
    Mcp,
    Web,
    Harness,
    Subcolony,
}

const KINDS: [Kind; 9] = [
    Kind::Llm,
    Kind::Timer,
    Kind::Store,
    Kind::Voice,
    Kind::Proxy,
    Kind::Mcp,
    Kind::Web,
    Kind::Harness,
    Kind::Subcolony,
];

enum Cell {
    Llm(Box<LlmCell>),
    Timer(Box<TimerCell>),
    Store(Box<StoreCell>),
    Voice(Box<VoiceCell>),
    Proxy(Box<ProxyCell>),
    Mcp(Box<McpCell>),
    Web(Box<WebCell>),
    Harness(Box<HarnessCell>),
    Subcolony(Box<SubcolonyCell>),
}

impl Kind {
    /// The birth params the type starts on.
    fn birth(self) -> Value {
        match self {
            Kind::Llm => json!({"provider": "openai", "model": "start/model",
                                "api_key": "test-key-1353",
                                "base_url": "http://127.0.0.1:1/v1"}),
            Kind::Timer => json!({"query_timeout_ms": 5000, "schedules": []}),
            Kind::Store => json!({"schema": {"items": {"id": "int"}}}),
            Kind::Voice => json!({"mount": "voice", "stt": {"provider": "echo"}}),
            Kind::Proxy => json!({"bot_token": "T", "emit_to": "/dst",
                                  "long_poll_timeout_ms": 35000,
                                  "long_poll_request_secs": 30,
                                  "send_timeout_ms": 10000, "query_timeout_ms": 5000}),
            Kind::Mcp => json!({"external_timeout_ms": 30000, "query_timeout_ms": 5000}),
            Kind::Web => json!({"mount": "display"}),
            Kind::Harness => json!({"adapter": "claude-code", "emit_to": "/dst",
                                    "workspace_root": std::env::temp_dir()}),
            Kind::Subcolony => json!({"root": std::env::temp_dir()}),
        }
    }

    /// A key the type takes at run time, with a value that is not its default.
    fn mutable(self) -> (&'static str, Value) {
        match self {
            Kind::Llm => ("max_tokens", json!(777)),
            Kind::Timer => ("query_timeout_ms", json!(1234)),
            Kind::Store => ("query_timeout_ms", json!(1234)),
            Kind::Voice => ("release_grace_ms", json!(321)),
            Kind::Proxy => ("send_timeout_ms", json!(222)),
            Kind::Mcp => ("external_timeout_ms", json!(222)),
            Kind::Web => ("external_timeout_ms", json!(222)),
            Kind::Harness => ("startup_timeout_ms", json!(222)),
            Kind::Subcolony => ("request_timeout_ms", json!(222)),
        }
    }

    /// A key the type refuses. The timer has no immutable key (`schedules` is
    /// its ops') and the web cell has none at all; both refuse an unknown one.
    fn refused(self) -> (&'static str, Value) {
        match self {
            Kind::Llm => ("api_key", json!("leaked")),
            Kind::Timer => ("schedules", json!([])),
            Kind::Store => ("schema", json!({})),
            Kind::Voice => ("x_title", json!("other")),
            Kind::Proxy => ("bot_token", json!("leaked")),
            Kind::Mcp => ("auth", json!("leaked")),
            Kind::Web => ("listen", json!("leaked")),
            Kind::Harness => ("workspace_root", json!("leaked")),
            Kind::Subcolony => ("root", json!("leaked")),
        }
    }

    /// What the factory rebuilds at the next life, as JSON.
    fn next_life(self, conn: &rusqlite::Connection) -> Value {
        fn of<P: OverlayParams>(conn: &rusqlite::Connection, birth: &Value) -> Value {
            to_value(restore::<P>(conn, birth).expect("restore")).expect("json")
        }
        let birth = self.birth();
        match self {
            Kind::Llm => of::<LlmParams>(conn, &birth),
            Kind::Timer => of::<TimerOverlay>(conn, &birth),
            Kind::Store => of::<StoreParams>(conn, &birth),
            Kind::Voice => of::<VoiceOverlay>(conn, &birth),
            Kind::Proxy => of::<ProxyOverlay>(conn, &birth),
            Kind::Mcp => of::<McpOverlay>(conn, &birth),
            Kind::Web => of::<WebOverlay>(conn, &birth),
            Kind::Harness => of::<HarnessOverlay>(conn, &birth),
            Kind::Subcolony => of::<SubcolonyOverlay>(conn, &birth),
        }
    }
}

struct Subject {
    cell: Cell,
    db: DbConn,
    td: TempDir,
    kind: Kind,
}

fn subject(kind: Kind) -> Subject {
    let td = TempDir::new().unwrap();
    let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let birth = kind.birth();
    let cell = match kind {
        Kind::Llm => Cell::Llm(Box::new(
            LlmCell::restored(&conn, &birth, reqwest::Client::builder().build().unwrap())
                .expect("restore"),
        )),
        Kind::Timer => {
            setup_timer_schema(&conn).unwrap();
            Cell::Timer(Box::new(TimerCell::new(Path::new(OWN), vec![], 5000)))
        }
        Kind::Store => {
            conn.execute_batch("CREATE TABLE items (id INTEGER)")
                .unwrap();
            Cell::Store(Box::new(StoreCell::new(
                StoreParams::parse(&birth).expect("store params"),
            )))
        }
        Kind::Voice => {
            let params = VoiceParams::parse(&birth).expect("the echo config parses");
            let stt = build_stt(&params.stt, ProviderTimeouts::default()).expect("echo");
            let (events_tx, _events_rx) = mpsc::channel(8);
            let io = VoiceIo::new(
                params.mount.clone(),
                stt,
                None,
                Mode::Auto,
                Duration::from_millis(params.external_timeout_ms),
                Duration::from_millis(params.provider_idle_timeout_ms),
                events_tx,
            );
            Cell::Voice(Box::new(VoiceCell::new(
                Path::new(OWN),
                io,
                &params,
                &birth,
            )))
        }
        Kind::Proxy => {
            setup_proxy_schema(&conn).unwrap();
            Cell::Proxy(Box::new(ProxyCell::new(
                TelegramClient::new("http://127.0.0.1:1", "T").unwrap(),
                Path::new("/dst"),
                0,
                35000,
                30,
                10000,
                5000,
                "https://api.telegram.org".into(),
            )))
        }
        Kind::Mcp => {
            setup_mcp_schema(&conn).unwrap();
            Cell::Mcp(Box::new(McpCell::new_unset(30_000, 5_000, "mcp".into())))
        }
        Kind::Web => {
            setup_web_schema(&conn).unwrap();
            let params = WebParams::parse(&birth).expect("web params");
            let (pages_tx, pages_rx) = watch::channel(Arc::default());
            let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
            let (ready_tx, ready_rx) = watch::channel(false);
            let (push_tx, push_rx) = mpsc::channel::<WebReconfig>(8);
            let io = WebIo::new(
                params.mount.clone(),
                String::new(),
                OWN,
                pages_rx,
                assets_rx,
                ready_rx,
                push_rx,
                Arc::new(SurfaceRegistry::new()),
            );
            Cell::Web(Box::new(WebCell::new(
                OWN.to_string(),
                io,
                &params,
                pages_tx,
                assets_tx,
                ready_tx,
                push_tx,
            )))
        }
        Kind::Harness => {
            setup_harness_schema(&conn).unwrap();
            Cell::Harness(Box::new(HarnessCell::new(
                HarnessParams::parse(&birth).expect("harness params"),
            )))
        }
        Kind::Subcolony => Cell::Subcolony(Box::new(SubcolonyCell::new(
            SubcolonyParams::parse(&birth).expect("subcolony params"),
        ))),
    };
    Subject {
        cell,
        db: DbConn::wrap(conn, Some(Duration::from_secs(5))),
        td,
        kind,
    }
}

impl Subject {
    /// Deliver one body as the colony would; every emission of the call.
    async fn deliver(&mut self, hop: Value, body: Value) -> Vec<CellEmission> {
        let hop = hop.as_object().cloned().unwrap_or_default();
        let headers = Headers::from_parts(Map::new(), hop);
        let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
        let sink = OutputSink::new(
            tx,
            Path::new(OWN),
            Uuid::now_v7(),
            Uuid::now_v7(),
            32,
            headers.clone(),
            None,
        );
        let msg = MessageBuilder::new(Path::new(OWN))
            .reply_to(Path::new("/observer"))
            .headers(headers)
            .body(Body::Inline(body))
            .build();
        match &mut self.cell {
            Cell::Llm(c) => c.handle(msg, &sink, &mut self.db).await,
            Cell::Store(c) => c.handle(msg, &sink, &mut self.db).await,
            Cell::Timer(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Voice(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Proxy(c) => {
                let (rc, _keep) = mpsc::channel::<ProxyReconfig>(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Mcp(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Web(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Harness(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
            Cell::Subcolony(c) => {
                let (rc, _keep) = mpsc::channel(8);
                c.handle(msg, &sink, &mut self.db, &rc).await;
            }
        }
        drop(sink);
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        out
    }

    /// The rows of the cell's params overlay in its own `cell.db`.
    async fn overlay(&mut self) -> Vec<(String, String)> {
        self.db
            .call(|c| -> rusqlite::Result<Vec<(String, String)>> {
                let mut st = c.prepare("SELECT key, value FROM params ORDER BY key")?;
                let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect()
            })
            .await
            .unwrap()
    }
}

impl Subject {
    /// The rows the type's own work would leave behind (a store's table, a
    /// timer's schedules; 0 for a type whose work leaves none).
    async fn rows(&mut self) -> i64 {
        let table = match self.kind {
            Kind::Store => "items",
            Kind::Timer => "schedules",
            _ => return 0,
        };
        self.db
            .call(move |c| -> rusqlite::Result<i64> {
                c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            })
            .await
            .unwrap()
    }
}

/// A params-only body of one key.
fn push(pair: (&str, Value)) -> Value {
    let mut update = Map::new();
    update.insert(pair.0.to_string(), pair.1);
    json!({"system": {}, "params": update})
}

/// The hop of a push through the params door.
fn in_params(subscriber: &str) -> Value {
    json!({"route": "in_params", "subscriber": subscriber})
}

fn header(e: &CellEmission) -> Map<String, Value> {
    e.content["header"].as_object().cloned().unwrap_or_default()
}

/// Claim 1, for one type: nothing moves, and the refusal is loud but no report.
async fn a_foreign_push_moves_nothing(kind: Kind) {
    for address in [json!(OTHER), json!(""), json!(5)] {
        let mut s = subject(kind);
        let before = kind.next_life(&open_or_create_cell_db(&s.td.path().join("cell.db")).unwrap());
        let out = s
            .deliver(
                json!({"route": "in_params", "subscriber": address}),
                push(kind.mutable()),
            )
            .await;
        assert_eq!(out.len(), 1, "{kind:?} {address}: one refusal: {out:?}");
        let h = header(&out[0]);
        assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
        assert!(
            !h.contains_key("refused_subscriber") && !h.contains_key("params_answer"),
            "{kind:?} {address}: a foreign push's refusal is no report: {h:?}"
        );
        assert!(
            out[0].content.to_string().contains("not for this cell"),
            "{kind:?} {address}: the refusal says why: {}",
            out[0].content
        );
        assert!(s.overlay().await.is_empty(), "{kind:?} {address}: moved");
        let after = kind.next_life(&open_or_create_cell_db(&s.td.path().join("cell.db")).unwrap());
        assert_eq!(before, after, "{kind:?} {address}");
        // The positive half: the same push, addressed right, does move.
        let out = s.deliver(in_params(OWN), push(kind.mutable())).await;
        assert_eq!(out.len(), 1, "{kind:?}: {out:?}");
        assert_eq!(s.overlay().await.len(), 1, "{kind:?}: the own push applies");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_llm() {
    a_foreign_push_moves_nothing(Kind::Llm).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_timer() {
    a_foreign_push_moves_nothing(Kind::Timer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_store() {
    a_foreign_push_moves_nothing(Kind::Store).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_voice() {
    a_foreign_push_moves_nothing(Kind::Voice).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_proxy() {
    a_foreign_push_moves_nothing(Kind::Proxy).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_mcp() {
    a_foreign_push_moves_nothing(Kind::Mcp).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_web() {
    a_foreign_push_moves_nothing(Kind::Web).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_harness() {
    a_foreign_push_moves_nothing(Kind::Harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_push_for_another_cell_moves_nothing_subcolony() {
    a_foreign_push_moves_nothing(Kind::Subcolony).await;
}

/// Claim 1 beside a turn, off the params lane: a store op that carries a foreign
/// slot runs without it, as an `llm` turn does (GH #862). On the lane it does
/// not run (claim 5).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_store_op_runs_without_a_foreign_slot() {
    let mut s = subject(Kind::Store);
    let mut body = push(Kind::Store.mutable());
    body["messages"] = json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                               "text": "{\"operation\": \"nonsense\"}"}]);
    let out = s
        .deliver(json!({"route": "in_model", "subscriber": OTHER}), body)
        .await;
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(
        !out[0].content.to_string().contains("not for this cell"),
        "the op is answered, not the slot: {}",
        out[0].content
    );
    // The op's own name in the header, not `params_update`: the op ran.
    assert_eq!(
        header(&out[0]).get("operation"),
        Some(&json!("nonsense")),
        "the answer is the op's: {}",
        out[0].content
    );
    assert!(s.overlay().await.is_empty());
}

/// Work of the type, in the shape the cell runs it off the lane. The first one
/// of a store and a timer leaves a row behind, which is what is counted.
fn work(kind: Kind) -> Value {
    match kind {
        Kind::Store => json!({"messages": [{"origin": "assistant", "type": "tool_call",
            "id": "c1", "text": "{\"operation\": \"insert\", \"table\": \"items\", \
                                \"row\": {\"id\": 1}}"}]}),
        Kind::Timer => json!({"op": "add", "schedule_id": Uuid::now_v7().to_string(),
            "schedule_name": "n", "cron": "0 0 4 * * *", "emit_to": "/dst", "emit_body": {}}),
        _ => json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
            "text": "{\"operation\": \"nonsense\"}"}]}),
    }
}

/// Claim 5, for one type. Every answer is the refusal and the only emission;
/// the valid `params` beside the work is not applied either.
async fn work_on_the_lane_is_refused(kind: Kind) {
    let update = push(kind.mutable())["params"].clone();
    let mut beside = work(kind);
    beside["params"] = update.clone();
    let bodies = [
        work(kind),
        beside,
        json!({"params": update, "system": {"persona": {"text": "another"}}}),
        json!({"params": update, "system": "another"}),
        json!({"params": update, "messages": null}),
        json!({"params": update, "attachments": [{"blob_id": "b"}]}),
        json!({"params": update, "sealed": {"epk": "", "nonce": "", "ciphertext": ""}}),
        json!({"params": update, "tool_scope": ["a"]}),
        json!({"system": {}, "messages": []}),
        json!({}),
        json!([]),
    ];
    for body in bodies {
        let mut s = subject(kind);
        let out = s.deliver(in_params(OWN), body.clone()).await;
        assert_eq!(out.len(), 1, "{kind:?} {body}: one answer: {out:?}");
        let h = header(&out[0]);
        assert_eq!(
            h.get("params_answer"),
            Some(&json!("params_refused")),
            "{kind:?} {body}: {h:?}"
        );
        assert_eq!(h.get("refused_subscriber"), Some(&json!(OWN)), "{kind:?}");
        assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
        assert!(s.overlay().await.is_empty(), "{kind:?} {body}: moved");
        assert_eq!(s.rows().await, 0, "{kind:?} {body}: the work ran");

        // Nobody to answer: refused in the type's own shape, and done as little.
        for hop in [in_params(OTHER), json!({"route": "in_params"})] {
            let mut s = subject(kind);
            let out = s.deliver(hop.clone(), body.clone()).await;
            assert_eq!(out.len(), 1, "{kind:?} {hop} {body}: {out:?}");
            let h = header(&out[0]);
            assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
            assert!(!h.contains_key("params_answer"), "{kind:?} {hop}: {h:?}");
            assert!(s.overlay().await.is_empty(), "{kind:?} {hop} {body}");
            assert_eq!(s.rows().await, 0, "{kind:?} {hop} {body}");
        }
    }
    // The empty slots of an update are no work: each form applies, once answered.
    for slots in [
        json!({"messages": []}),
        json!({"system": []}),
        json!({"system": {}, "messages": []}),
    ] {
        let mut s = subject(kind);
        let mut body = slots.clone();
        body["params"] = push(kind.mutable())["params"].clone();
        let out = s.deliver(in_params(OWN), body).await;
        assert_eq!(out.len(), 1, "{kind:?} {slots}: {out:?}");
        assert_eq!(
            header(&out[0]).get("params_answer"),
            Some(&json!("params_applied")),
            "{kind:?} {slots}: {out:?}"
        );
        assert_eq!(s.overlay().await.len(), 1, "{kind:?} {slots}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_llm() {
    work_on_the_lane_is_refused(Kind::Llm).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_timer() {
    // The control: off the lane the same body is work, and it runs.
    let mut s = subject(Kind::Timer);
    s.deliver(json!({}), work(Kind::Timer)).await;
    assert_eq!(s.rows().await, 1);
    work_on_the_lane_is_refused(Kind::Timer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_store() {
    // The control: off the lane the same body is work, and it runs.
    let mut s = subject(Kind::Store);
    s.deliver(json!({}), work(Kind::Store)).await;
    assert_eq!(s.rows().await, 1);
    work_on_the_lane_is_refused(Kind::Store).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_voice() {
    work_on_the_lane_is_refused(Kind::Voice).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_proxy() {
    work_on_the_lane_is_refused(Kind::Proxy).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_mcp() {
    work_on_the_lane_is_refused(Kind::Mcp).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_web() {
    work_on_the_lane_is_refused(Kind::Web).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_harness() {
    work_on_the_lane_is_refused(Kind::Harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_on_the_params_lane_is_refused_subcolony() {
    work_on_the_lane_is_refused(Kind::Subcolony).await;
}

/// The proxy platforms whose every param is fixed at birth (Telegram is the
/// one with an overlay) still answer: one refusal to the update's own sender,
/// never the send path's `missing_chat_id`.
async fn a_platform_without_an_overlay_refuses<C: LongRunningCell>(
    what: &str,
    mut cell: C,
    db: rusqlite::Connection,
) {
    let mut db = DbConn::wrap(db, Some(Duration::from_secs(5)));
    let update = json!({"params": {"emit_to": "/elsewhere"}});
    for (hop, body) in [
        (in_params(OWN), update.clone()),
        (in_params(OWN), work(Kind::Proxy)),
        (in_params(OTHER), update.clone()),
        (json!({"route": "in_params"}), work(Kind::Proxy)),
    ] {
        let headers = Headers::from_parts(Map::new(), hop.as_object().cloned().unwrap());
        let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
        let sink = OutputSink::new(
            tx,
            Path::new(OWN),
            Uuid::now_v7(),
            Uuid::now_v7(),
            32,
            headers.clone(),
            None,
        );
        let msg = MessageBuilder::new(Path::new(OWN))
            .reply_to(Path::new("/observer"))
            .headers(headers)
            .body(Body::Inline(body.clone()))
            .build();
        let (rc, mut frames) = mpsc::channel(8);
        cell.handle(msg, &sink, &mut db, &rc).await;
        drop(sink);
        drop(rc);
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        assert_eq!(out.len(), 1, "{what} {hop} {body}: {out:?}");
        assert!(frames.recv().await.is_none(), "{what}: nothing to the I/O");
        let h = header(&out[0]);
        assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
        if hop["subscriber"] == OWN {
            assert_eq!(
                h.get("params_answer"),
                Some(&json!("params_refused")),
                "{what} {body}: {h:?}"
            );
            assert_eq!(h.get("refused_subscriber"), Some(&json!(OWN)), "{what}");
            assert_eq!(out[0].content["messages"], json!([]), "{what}");
        } else {
            assert!(!h.contains_key("params_answer"), "{what} {hop}: {h:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_slack_proxy_refuses_an_update() {
    let params = SlackParams::parse(&json!({"app_token": "xapp-x", "bot_token": "xoxb-x",
        "emit_to": "/agent", "base_url": "http://127.0.0.1:1"}))
    .expect("params");
    let cell = SlackCell::new(&params, SlackClient::new(&params).expect("client"));
    let db = rusqlite::Connection::open_in_memory().expect("db");
    setup_slack_schema(&db).expect("schema");
    a_platform_without_an_overlay_refuses("slack", cell, db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_webhook_proxy_refuses_an_update() {
    let params = WebhookParams::parse(&json!({"platform": "webhook", "mount": "probe",
        "route": "probe", "emit_to": "/sink",
        "verify": {"kind": "hmac_sha256", "secret": "s", "header": "X-Sig"}}))
    .expect("params");
    let db = rusqlite::Connection::open_in_memory().expect("db");
    a_platform_without_an_overlay_refuses("webhook", WebhookCell::new(&params), db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_peer_proxy_refuses_an_update() {
    let params = MeclawParams::parse(&json!({"platform": "meclaw", "mount": "peer",
        "identity_header": "X-Meclaw-Peer", "boundary": "north", "emit_to": "/sink",
        "external_timeout_ms": 5000, "lanes": {"accepts": [], "emits": [
            {"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}}))
    .expect("params");
    let db = rusqlite::Connection::open_in_memory().expect("db");
    let cell = MeclawCell::new(&params).expect("cell");
    a_platform_without_an_overlay_refuses("meclaw", cell, db).await;
}

/// Claim 2, refused: named, with the type's own code and the detail, and the
/// detail never carries the value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_immutable_key_is_refused_and_answered() {
    for kind in KINDS {
        let mut s = subject(kind);
        let (key, value) = kind.refused();
        let out = s.deliver(in_params(OWN), push((key, value.clone()))).await;
        assert_eq!(out.len(), 1, "{kind:?}: one answer: {out:?}");
        let h = header(&out[0]);
        assert_eq!(
            h.get("params_answer"),
            Some(&json!("params_refused")),
            "{kind:?}: {h:?}"
        );
        assert_eq!(h.get("refused_subscriber"), Some(&json!(OWN)), "{kind:?}");
        assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
        assert_eq!(h.get("finish_reason"), Some(&json!("error")), "{h:?}");
        assert!(
            !h.contains_key("route") && !h.contains_key("refused_model"),
            "{kind:?}: neither a lane of its own nor a model push's refusal: {h:?}"
        );
        let detail = out[0].content["meta"]["detail"].as_str().unwrap_or("");
        assert!(detail.contains(key), "{kind:?}: {detail}");
        assert!(!detail.contains("leaked"), "{kind:?}: {detail}");
        assert_eq!(out[0].content["messages"], json!([]), "{kind:?}");
        assert!(s.overlay().await.is_empty(), "{kind:?}: no partial apply");

        // A slot that is no object is refused the same way.
        let out = s
            .deliver(in_params(OWN), json!({"system": {}, "params": 5}))
            .await;
        assert_eq!(out.len(), 1, "{kind:?}: {out:?}");
        assert_eq!(
            header(&out[0]).get("params_answer"),
            Some(&json!("params_refused")),
            "{kind:?}: {out:?}"
        );
    }
}

/// Claims 2 and 3: an applied update is answered on the params lane with the
/// own address -- and silent without one, or on another lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_applied_update_is_answered_only_with_a_subscriber() {
    for kind in KINDS {
        for (what, hop) in [
            ("no subscriber", json!({"route": "in_params"})),
            ("no hop at all", json!({})),
            (
                "another lane",
                json!({"route": "in_model", "subscriber": OWN}),
            ),
        ] {
            let mut s = subject(kind);
            assert!(
                s.overlay().await.is_empty(),
                "{kind:?} {what}: a fresh cell"
            );
            let out = s.deliver(hop, push(kind.mutable())).await;
            assert!(out.is_empty(), "{kind:?} {what}: silent: {out:?}");
            assert_eq!(s.overlay().await.len(), 1, "{kind:?} {what}: applied");
        }

        let mut s = subject(kind);
        let out = s.deliver(in_params(OWN), push(kind.mutable())).await;
        assert_eq!(out.len(), 1, "{kind:?}: one answer: {out:?}");
        let h = header(&out[0]);
        assert_eq!(
            h.get("params_answer"),
            Some(&json!("params_applied")),
            "{kind:?}: {h:?}"
        );
        assert_eq!(h.get("applied_subscriber"), Some(&json!(OWN)), "{kind:?}");
        for absent in ["refused_subscriber", "error_code", "finish_reason", "route"] {
            assert!(!h.contains_key(absent), "{kind:?}: {absent}: {h:?}");
        }
        assert_eq!(out[0].content["messages"], json!([]), "{kind:?}");
        assert_eq!(out[0].target, Path::new("/observer"), "{kind:?}");
    }
}

/// Claim 3, the refused half: a refusal outside the params lane keeps the
/// shape each type had (the `llm` one is pinned by `gh863_*`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_refusal_outside_the_lane_keeps_its_shape() {
    for kind in KINDS {
        let mut s = subject(kind);
        for hop in [
            json!({}),
            json!({"route": "in_params"}),
            json!({"route": "in_model", "subscriber": OWN}),
        ] {
            let out = s.deliver(hop.clone(), push(kind.refused())).await;
            assert_eq!(out.len(), 1, "{kind:?} {hop}: {out:?}");
            let h = header(&out[0]);
            assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
            assert!(!h.contains_key("params_answer"), "{kind:?} {hop}: {h:?}");
        }
    }
}

/// Claim 2 on a code that is not `invalid_input`: the type's own gates answer
/// with their own code, and no new one is invented.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_types_own_gate_answers_with_its_own_code() {
    let mut timer = subject(Kind::Timer);
    timer.cell = Cell::Timer(Box::new(
        TimerCell::new(Path::new(OWN), vec![], 5000).with_accept_ops(Some(vec!["trigger".into()])),
    ));
    let mut store = subject(Kind::Store);
    store.cell = Cell::Store(Box::new(StoreCell::new(
        StoreParams::parse(&json!({"schema": {"items": {"id": "int"}},
                                   "write_surface": "internal"}))
        .expect("store params"),
    )));
    for (s, code) in [
        (&mut timer, "op_not_accepted"),
        (&mut store, "write_denied"),
    ] {
        let kind = s.kind;
        let out = s.deliver(in_params(OWN), push(kind.mutable())).await;
        assert_eq!(out.len(), 1, "{kind:?}: {out:?}");
        let h = header(&out[0]);
        assert_eq!(h.get("error_code"), Some(&json!(code)), "{kind:?}: {h:?}");
        assert_eq!(
            h.get("params_answer"),
            Some(&json!("params_refused")),
            "{kind:?}: {h:?}"
        );
        assert!(s.overlay().await.is_empty(), "{kind:?}");
    }
}

/// OR-w2.tuer.1: the shared answer of every type passes an `emits` contract that
/// owes keys it does not carry. The contract module matches on the same words
/// the answer is written with, so this is also what keeps the two together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_the_answer_of_every_type_owes_no_key_of_the_contract() {
    let emits: EmitsBlock = meclaw_core::serde_json::from_value(json!({
        "body": {"transcript": {"type": "string"}},
        "hop": {"event_id": {"type": "string"}},
    }))
    .unwrap();
    let compiled = CompiledEmits::compile(&emits).unwrap();
    for kind in KINDS {
        for pair in [kind.mutable(), kind.refused()] {
            let mut s = subject(kind);
            let out = s.deliver(in_params(OWN), push(pair)).await;
            assert_eq!(out.len(), 1, "{kind:?}: {out:?}");
            assert_eq!(
                validate_emits(&out[0].content, &compiled),
                Ok(()),
                "{kind:?}: {}",
                out[0].content
            );
        }
        // The control: the same cell's refusal outside the lane still owes them.
        let mut s = subject(kind);
        let out = s.deliver(json!({}), push(kind.refused())).await;
        assert!(
            validate_emits(&out[0].content, &compiled).is_err(),
            "{kind:?}: {}",
            out[0].content
        );
    }
}

/// Claim 4. What the push set is what the next life starts on, read back from
/// the file by the way the factory rebuilds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_the_applied_param_survives_a_restart() {
    for kind in KINDS {
        let mut s = subject(kind);
        let (key, value) = kind.mutable();
        let out = s.deliver(in_params(OWN), push((key, value.clone()))).await;
        assert_eq!(
            header(&out[0]).get("params_answer"),
            Some(&json!("params_applied")),
            "{kind:?}: {out:?}"
        );
        let Subject { cell, db, td, .. } = s;
        drop(cell);
        drop(db);
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        assert_eq!(kind.next_life(&conn)[key], value, "{kind:?}: {key}");
    }
}
