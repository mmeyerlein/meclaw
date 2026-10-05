//! GH #1005 — a bundle may answer with its refusals only.
//!
//! Measured with 300 figures panning on a display (GH #1005): the
//! colony log grew 138.6 MB/h, 82.7 MB/h of it message bytes, and a bundle's
//! answer was 0.94 × its request — one `tool_result` turn AND one `results[]`
//! entry per leg, two pictures of the same fact (≈ 180 B per leg). The log is
//! append-only and complete (nothing is deleted, nothing is condensed), so the
//! only place to save is the *form* of what is written, never the history.
//!
//! The claim locked here (opt-in; the default form is untouched): a bundle that carries the
//! top-level slot `"answer": "errors"` is answered with the header as before
//! plus `legs`, and with a turn and a result for every REFUSED leg only. Every
//! refusal is still in the answer — no fact is missing, only the repetition of
//! the successes. Without the slot the answer is byte-for-byte what it was.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_cells::web::cell::{WebCell, WebReconfig};
use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::params::WebParams;
use meclaw_cells::web::render::PageMap;
use meclaw_cells::web::{AssetMap, WebCellFactory, WebIo};
use meclaw_colony::{CellFactory, ContractView, DbConn, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const MOUNT: &str = "screen";

use web_fixture::log_lab::{LogClasses, file_bytes, read_classes, seed_figures};

// ---------------------------------------------------------------- in-process harness
// The 723 harness: the cell's `handle` driven directly, the reply read off the
// sink. Enough for the answer's SHAPE; the log needs a colony (lab below).

/// A page whose root holds `n` flat figures `fig-0 … fig-(n-1)`, the village's
/// shape (flat root children).
fn figures_db(n: usize) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    setup_web_schema(&conn).expect("schema");
    for (name, template, schema) in [
        ("screen", r#"<main>{{children}}</main>"#, r#"{}"#),
        (
            "fig",
            r#"<i data-x="{{x}}" data-y="{{y}}"></i>"#,
            r#"{"x":"text","y":"text"}"#,
        ),
    ] {
        conn.execute(
            "INSERT INTO components (name, template, prop_schema) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, template, schema],
        )
        .expect("component");
    }
    conn.execute(
        "INSERT INTO objects (id, parent, component, ord, props) VALUES ('root', NULL, 'screen', 0, '{}')",
        [],
    )
    .expect("root");
    for i in 0..n {
        conn.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, 'root', 'fig', ?2, '{\"x\":\"0\",\"y\":\"0\"}')",
            rusqlite::params![format!("fig-{i}"), i as i64],
        )
        .expect("figure");
    }
    conn.execute(
        "INSERT INTO pages (route, root, title) VALUES ('/', 'root', 'fixture')",
        [],
    )
    .expect("page");
    conn
}

struct Harness {
    cell: WebCell,
    db: DbConn,
    out_rx: mpsc::Receiver<CellEmission>,
    out_tx: mpsc::Sender<CellEmission>,
    pushes: mpsc::Receiver<WebReconfig>,
    _pages: watch::Receiver<Arc<PageMap>>,
    _assets: watch::Receiver<Arc<AssetMap>>,
    _ready: watch::Receiver<bool>,
    _reconfig: mpsc::Sender<WebReconfig>,
}

fn harness(db: rusqlite::Connection) -> Harness {
    let (pages_tx, pages_rx) = watch::channel(Arc::new(PageMap::new()));
    let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
    let (ready_tx, ready_rx) = watch::channel(false);
    let (push_tx, pushes) = mpsc::channel::<WebReconfig>(1024);
    let (io_push_tx, io_push_rx) = mpsc::channel::<WebReconfig>(1);
    let params = WebParams::parse(&json!({"mount": MOUNT})).expect("params");
    let io = WebIo::new(
        MOUNT.to_string(),
        String::new(),
        "/display",
        pages_rx.clone(),
        assets_rx.clone(),
        ready_rx.clone(),
        io_push_rx,
        Arc::new(SurfaceRegistry::new()),
    );
    let cell = WebCell::new(
        "/display".to_string(),
        io,
        &params,
        pages_tx,
        assets_tx,
        ready_tx,
        push_tx,
    );
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    Harness {
        cell,
        db: DbConn::wrap(db, None),
        out_rx,
        out_tx,
        pushes,
        _pages: pages_rx,
        _assets: assets_rx,
        _ready: ready_rx,
        _reconfig: io_push_tx,
    }
}

fn sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/display"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    )
}

/// One `object.update` leg moving `fig-<n>` (or any id) to `(x, y)`.
fn mv(i: usize, id: &str, x: usize, y: usize) -> Value {
    json!({
        "origin": "assistant",
        "type": "tool_call",
        "id": format!("c{i}"),
        "text": json!({"op": "object.update", "id": id, "props": {"x": x.to_string(), "y": y.to_string()}}).to_string(),
    })
}

/// A pan step of `n` legs: every figure moves; the legs listed in `refused`
/// address an object that does not exist and come back refused.
fn pan(n: usize, step: usize, refused: &[usize]) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let id = if refused.contains(&i) {
                format!("ghost-{i}")
            } else {
                format!("fig-{i}")
            };
            mv(i, &id, i * 7 + step, i * 3 + step)
        })
        .collect()
}

/// Run one body through the cell and hand back the one reply.
async fn answer(h: &mut Harness, body: Value) -> Value {
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let msg = MessageBuilder::new(Path::new("/display"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build();
    h.cell
        .handle(msg, &sink(h.out_tx.clone()), &mut h.db, &rc_tx)
        .await;
    tokio::time::timeout(Duration::from_secs(10), h.out_rx.recv())
        .await
        .expect("the cell answers")
        .expect("an emission")
        .content
}

/// `duration_ms` is wall clock; everything else of the answer is fixed.
fn without_clock(mut v: Value) -> Value {
    if let Some(h) = v.get_mut("header").and_then(Value::as_object_mut)
        && h.contains_key("duration_ms")
    {
        h.insert("duration_ms".into(), json!(0));
    }
    if let Some(rs) = v.get_mut("results").and_then(Value::as_array_mut) {
        for r in rs {
            if let Some(m) = r.as_object_mut() {
                m.insert("duration_ms".into(), json!(0));
            }
        }
    }
    v
}

/// The answer to `pan(3, 0, &[1])` as `main` wrote it before GH #1005, clock
/// zeroed. Recorded from the code before GH #1005; the lock is that the
/// opt-in form costs a caller who never asks for it NOTHING, not even a byte.
const GOLDEN_FULL: &str = r#"{"header":{"bundle_errors":1,"duration_ms":0,"operation":"bundle","rows_affected":2},"messages":[{"id":"c0","origin":"tool","text":"null","type":"tool_result"},{"id":"c1","origin":"tool","text":"no object \"ghost-1\"","type":"tool_result"},{"id":"c2","origin":"tool","text":"null","type":"tool_result"}],"results":[{"duration_ms":0,"operation":"object.update","rows_affected":1,"tool_call_id":"c0"},{"duration_ms":0,"error_code":"unknown_object","operation":"object.update","rows_affected":0,"tool_call_id":"c1"},{"duration_ms":0,"operation":"object.update","rows_affected":1,"tool_call_id":"c2"}]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1005_the_full_answer_is_unchanged_by_default() {
    let mut h = harness(figures_db(3));
    let got = answer(&mut h, json!({ "messages": pan(3, 0, &[1]) })).await;
    let got = without_clock(got).to_string();
    assert_eq!(
        got, GOLDEN_FULL,
        "a bundle without the slot must answer byte-for-byte as before"
    );

    // `"answer": "full"` names the default; it is the same bytes.
    let mut h = harness(figures_db(3));
    let got = answer(
        &mut h,
        json!({ "messages": pan(3, 0, &[1]), "answer": "full" }),
    )
    .await;
    assert_eq!(without_clock(got).to_string(), GOLDEN_FULL);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1005_a_compact_answer_names_every_refused_leg() {
    let refused = [17, 150, 299];
    let mut h = harness(figures_db(300));
    let got = answer(
        &mut h,
        json!({ "messages": pan(300, 1, &refused), "answer": "errors" }),
    )
    .await;

    let header = &got["header"];
    assert_eq!(header["operation"], json!("bundle"));
    assert_eq!(header["legs"], json!(300), "{header}");
    assert_eq!(header["bundle_errors"], json!(3), "{header}");
    assert_eq!(header["rows_affected"], json!(297), "{header}");
    assert!(header.get("duration_ms").is_some(), "{header}");
    assert!(
        header.get("error_code").is_none(),
        "a partial failure never sets the whole-reply error_code: {header}"
    );

    let turns = got["messages"].as_array().expect("messages");
    let results = got["results"].as_array().expect("results");
    let want: Vec<String> = refused.iter().map(|i| format!("c{i}")).collect();
    let turn_ids: Vec<&str> = turns.iter().map(|t| t["id"].as_str().unwrap()).collect();
    let result_ids: Vec<&str> = results
        .iter()
        .map(|r| r["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(turn_ids, want, "one turn per refused leg, in call order");
    assert_eq!(
        result_ids, want,
        "one result per refused leg, in call order"
    );
    for (t, i) in turns.iter().zip(refused) {
        assert_eq!(t["type"], json!("tool_result"));
        assert_eq!(t["text"], json!(format!("no object \"ghost-{i}\"")));
    }
    for r in results {
        assert_eq!(r["error_code"], json!("unknown_object"), "{r}");
        assert_eq!(r["operation"], json!("object.update"), "{r}");
    }

    // The writes happened all the same: the slot changes the answer, never the work.
    let x: String =
        h.db.call(|c| {
            c.query_row("SELECT props FROM objects WHERE id = 'fig-0'", [], |r| {
                r.get(0)
            })
        })
        .await
        .expect("fig-0");
    assert!(x.contains(r#""x":"1""#), "the leg wrote: {x}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1005_a_compact_answer_is_smaller_than_its_request() {
    let mut h = harness(figures_db(300));
    let body = json!({ "messages": pan(300, 2, &[]), "answer": "errors" });
    let request_bytes = body.to_string().len();
    let got = answer(&mut h, body).await;
    let answer_bytes = got.to_string().len();
    assert_eq!(got["messages"], json!([]), "{got}");
    assert_eq!(got["results"], json!([]), "{got}");
    assert_eq!(got["header"]["bundle_errors"], json!(0), "{got}");
    assert_eq!(got["header"]["legs"], json!(300), "{got}");
    // The target of GH #1005: answer ≤ 0.05 × request. Measured before: 0.94.
    assert!(
        (answer_bytes as f64) <= 0.05 * request_bytes as f64,
        "answer {answer_bytes} B vs request {request_bytes} B"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1005_an_unknown_answer_form_is_refused() {
    for bad in [json!("short"), json!(true), json!({"form": "errors"})] {
        let mut h = harness(figures_db(3));
        let got = answer(
            &mut h,
            json!({ "messages": pan(3, 5, &[]), "answer": bad.clone() }),
        )
        .await;
        assert_eq!(got["header"]["error_code"], json!("invalid_input"), "{got}");
        assert_eq!(got["header"]["finish_reason"], json!("error"), "{got}");
        let props: String =
            h.db.call(|c| {
                c.query_row("SELECT props FROM objects WHERE id = 'fig-0'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .expect("fig-0");
        assert_eq!(
            props, r#"{"x":"0","y":"0"}"#,
            "a refused form writes nothing ({bad})"
        );
        assert!(h.pushes.try_recv().is_err(), "and pushes nothing ({bad})");
    }
}

// ---------------------------------------------------------------- lab: the colony log
// The log helpers live in the shared fixture (`web_fixture::log_lab`, OR-H4-15).

/// One run of the village's pan: 300 figures, bundles of 75 legs, `bundles`
/// of them, each answered before the next goes out. Returns the log classes
/// and the colony file growth.
async fn lab_run(compact: bool, bundles: usize) -> (LogClasses, u64) {
    use meclaw_testing::ColonyHandle;
    use meclaw_testing::topologies::phase_3b::CaptureCell;

    const FIGURES: usize = 300;
    const LEGS: usize = 75;
    let td = tempfile::TempDir::new().expect("td");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    let cell_dir = td.path().join("web-cell");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_figures(&cell_dir, FIGURES);

    let surfaces = Arc::new(SurfaceRegistry::new());
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({ "mount": MOUNT }),
            h.outputs_sender(),
            cell_dir,
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn web");
    h.register_spawned(Path::new("/web"), spawned).await;
    meclaw_testing::wait_for_mount(&surfaces, MOUNT).await;

    let (cap_tx, mut cap_rx) = mpsc::channel(1024);
    h.spawn(Path::new("/caller"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    // The wiring a template would declare: the display answers its caller.
    h.add_edge(Uuid::now_v7(), Path::new("/web"), Path::new("/caller"))
        .await;

    let before_classes = read_classes(&h.inbox_tx, "/web", "/caller").await;
    let before_file = file_bytes(td.path());

    for b in 0..bundles {
        let first = (b * LEGS) % FIGURES;
        let turns: Vec<Value> = (0..LEGS)
            .map(|k| {
                let i = first + k;
                mv(k, &format!("fig-{i}"), i * 7 + b, i * 3 + b)
            })
            .collect();
        let mut body = json!({ "messages": turns });
        if compact {
            body["answer"] = json!("errors");
        }
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(body))
            .build();
        h.send_from(Path::new("/caller"), msg).await;
        let got = match tokio::time::timeout(Duration::from_secs(30), cap_rx.recv()).await {
            Ok(Some(m)) => m,
            other => {
                let dead = h.drain_dead_letters().await;
                panic!(
                    "bundle {b}: the answer never reached the caller ({other:?}); dead letters: {dead:?}"
                )
            }
        };
        let Body::Inline(v) = &got.body else {
            panic!("inline answer")
        };
        // The colony lifts `header` onto the envelope; the body keeps the turns.
        let turns = v["messages"].as_array().map_or(0, Vec::len);
        assert_eq!(turns, if compact { 0 } else { LEGS }, "bundle {b}");
    }

    // GH #1014: the colony logs each hop fire-and-forget through its writer
    // thread, and `ReadMessages` reads without a write fence — the last answer
    // reaches the caller before its row is committed. Wait (bounded, 30 s
    // failure marker) until the log holds a request and an answer per bundle;
    // a row that never comes still fails the count below.
    let want = before_classes.rows + 2 * bundles as u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let after_classes = loop {
        let c = read_classes(&h.inbox_tx, "/web", "/caller").await;
        if c.rows >= want || tokio::time::Instant::now() >= deadline {
            break c;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let after_file = file_bytes(td.path());
    h.shutdown().await;
    let d = LogClasses {
        request: after_classes.request - before_classes.request,
        answer: after_classes.answer - before_classes.answer,
        event: after_classes.event - before_classes.event,
        other: after_classes.other - before_classes.other,
        rows: after_classes.rows - before_classes.rows,
    };
    (d, after_file.saturating_sub(before_file))
}

/// Lab (GH #1005): 300 updates/s as bundles of 75 legs at
/// 4/s for 60 s = 240 bundles. The bundles go out back to back rather than
/// paced — bytes per write do not depend on the clock, and the per-second
/// column is that figure × 4 bundles/s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1005_log_bytes_per_write_are_bounded() {
    const BUNDLES: usize = 240;
    const PER_S: f64 = 4.0; // bundles per second at 300 updates/s

    let (full, full_file) = lab_run(false, BUNDLES).await;
    let (compact, compact_file) = lab_run(true, BUNDLES).await;

    let table = |name: &str, c: &LogClasses, file: u64| {
        let per_bundle = |v: u64| v as f64 / BUNDLES as f64;
        let per_s = |v: u64| per_bundle(v) * PER_S;
        let total = c.total().max(1);
        format!(
            "{name:>8} | rows {rows:>4} | request {rq:>8.0} B/s ({rqp:>4.1} %) | answer {an:>8.0} B/s ({anp:>4.1} %) | event {ev:>6.0} B/s ({evp:>4.1} %) | other {ot:>6.0} B/s | log {lg:>8.0} B/s = {mbh:>6.1} MB/h | file {fl:>8.0} B/s, factor {ff:.2} | log/request per write {ratio:.3}",
            rows = c.rows,
            rq = per_s(c.request),
            rqp = 100.0 * c.request as f64 / total as f64,
            an = per_s(c.answer),
            anp = 100.0 * c.answer as f64 / total as f64,
            ev = per_s(c.event),
            evp = 100.0 * c.event as f64 / total as f64,
            ot = per_s(c.other),
            lg = per_s(c.total()),
            mbh = per_s(c.total()) * 3600.0 / 1e6,
            fl = per_s(file),
            ff = file as f64 / total as f64,
            ratio = (c.request + c.answer) as f64 / c.request.max(1) as f64,
        )
    };
    println!("GH1005-LAB {}", table("full", &full, full_file));
    println!("GH1005-LAB {}", table("compact", &compact, compact_file));

    assert_eq!(full.rows, compact.rows, "the slot never drops a message");
    assert!(
        full.answer as f64 > 0.5 * full.request as f64,
        "control: the full answer repeats the request (measured 0.94), got {full:?}"
    );
    let ratio = (compact.request + compact.answer) as f64 / compact.request as f64;
    assert!(
        ratio <= 1.1,
        "log bytes per write must stay ≤ 1.1 × request with the compact answer, got {ratio:.3} ({compact:?})"
    );
}
