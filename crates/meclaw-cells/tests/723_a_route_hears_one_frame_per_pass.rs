//! GH #723 — one curator pass is ONE frame per route, whatever it touched.
//!
//! Follow-up of #718, which made a bundle one push per route for the root and
//! the structural arm and left the plain slot arm at one push per
//! `(route, slot)`. A curator pass that opens or closes a window touches main,
//! aside and dock of the same output in one breath, so the browser was handed
//! three frames a few milliseconds apart.
//!
//! Measured on the twin `e25t` (18.09.2026, Chromium 1920x1080, a real mouse
//! click on the chat tile, `MutationObserver` on `#chat[data-level]`, frames
//! recorded in the page). Closing the window:
//!
//! | frame      | at      | bytes | diff keys            |
//! |------------|---------|-------|----------------------|
//! | `phx_reply`| +69 ms  |   112 | --                   |
//! | `diff`     | +167 ms |  1539 | `{"2"}` (dock)       |
//! | `diff`     | +172 ms |  1938 | `{"0"}` (main)       |
//!
//! `data-level`: `2 -> 0@30 (optimistic) -> 2@172 -> 0@176`. The revert rides
//! the frame that carries ONLY the dock slot: LiveView re-renders the whole
//! container from its cached tree on every diff, and the cached main slot is
//! still the one from before the pass, so a frame without key `0` writes the
//! old window back over the attribute the client set optimistically. That is a
//! visible blink, and § 5.7 ("a deviation is a defect") and § 5.8 ("only the
//! end state of the series, no window flashes") both forbid it.
//!
//! The claim locked here: a bundle over ONE route is ONE push whose diff names
//! every slot it touched; a bundle over two routes is two pushes, one each.
//! The same run on the live twin proves the client reads that shape — opening
//! the window sends one frame with keys `["0","1","2","3","s"]`.

use meclaw_cells::web::cell::{WebCell, WebReconfig};
use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::params::WebParams;
use meclaw_cells::web::render::{PageMap, materialize_all, wire_html};
use meclaw_cells::web::{AssetMap, WebIo};
use meclaw_colony::{DbConn, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// The mount the fixture's display registers.
const MOUNT: &str = "screen";

/// The single-output route.
const ROUTE: &str = "/";

/// The two outputs of the two-output fixture, named after the instance the
/// find was measured on.
const TV: &str = "/tv";
const MONITOR: &str = "/monitor";

/// The two components every fixture here is built from.
fn components(conn: &rusqlite::Connection) {
    for (name, template, schema) in [
        (
            "screen",
            r#"<main data-tick="{{tick}}">{{children}}</main>"#,
            r#"{"tick":"text"}"#,
        ),
        (
            "pane",
            r#"<section id="{{name}}" data-level="{{level}}"></section>"#,
            r#"{"name":"text","level":"text"}"#,
        ),
    ] {
        conn.execute(
            "INSERT INTO components (name, template, prop_schema) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, template, schema],
        )
        .expect("component");
    }
}

fn pane(conn: &rusqlite::Connection, id: &str, parent: &str, ord: i64) {
    conn.execute(
        "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, 'pane', ?3, ?4)",
        rusqlite::params![id, parent, ord, format!(r#"{{"name":"{id}","level":"0"}}"#)],
    )
    .expect("object");
}

fn root(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, NULL, 'screen', 0, '{\"tick\":\"0\"}')",
        rusqlite::params![id],
    )
    .expect("root");
}

fn page(conn: &rusqlite::Connection, route: &str, root_id: &str) {
    conn.execute(
        "INSERT INTO pages (route, root, title) VALUES (?1, ?2, 'fixture')",
        rusqlite::params![route, root_id],
    )
    .expect("page");
}

/// One output with the three slots a curator pass touches when a window opens
/// or closes: `main` (slot 0), `aside` (slot 1), `dock` (slot 2).
fn one_output_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    setup_web_schema(&conn).expect("schema");
    components(&conn);
    root(&conn, "root");
    for (i, id) in ["main", "aside", "dock"].iter().enumerate() {
        pane(&conn, id, "root", i as i64);
    }
    page(&conn, ROUTE, "root");
    conn
}

/// Two outputs, two slots each — the shape `display@2.5.0` lays down.
fn two_output_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    setup_web_schema(&conn).expect("schema");
    components(&conn);
    for (root_id, route, kids) in [
        ("root-tv", TV, ["main-tv", "dock-tv"]),
        ("root-monitor", MONITOR, ["main-monitor", "dock-monitor"]),
    ] {
        root(&conn, root_id);
        for (i, id) in kids.iter().enumerate() {
            pane(&conn, id, root_id, i as i64);
        }
        page(&conn, route, root_id);
    }
    conn
}

/// Everything one run needs: the cell, its database, and the push channel the
/// I/O half would be reading.
struct Harness {
    cell: WebCell,
    db: DbConn,
    pushes: mpsc::Receiver<WebReconfig>,
    /// Held: a closed watch channel would be a different code path.
    _pages: watch::Receiver<Arc<PageMap>>,
    _assets: watch::Receiver<Arc<AssetMap>>,
    _ready: watch::Receiver<bool>,
    _reconfig: mpsc::Sender<WebReconfig>,
}

fn harness(db: rusqlite::Connection) -> Harness {
    // The pages a booted cell has published before its first write: what the
    // viewers hold, and what every frame is computed against (GH #1001).
    let boot = materialize_all(&db).expect("the fixture renders");
    let (pages_tx, pages_rx) = watch::channel(Arc::new(boot));
    let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
    let (ready_tx, ready_rx) = watch::channel(false);
    let (push_tx, pushes) = mpsc::channel::<WebReconfig>(64);
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
    Harness {
        cell,
        db: DbConn::wrap(db, None),
        pushes,
        _pages: pages_rx,
        _assets: assets_rx,
        _ready: ready_rx,
        _reconfig: io_push_tx,
    }
}

/// A sink that goes nowhere the assertions look.
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

/// One `object.update` leg of a tool-call turn.
fn leg(i: usize, id: &str, props: Value) -> Value {
    op(i, json!({"op": "object.update", "id": id, "props": props}))
}

/// One leg of a tool-call turn, whatever the op.
fn op(i: usize, body: Value) -> Value {
    json!({
        "origin": "assistant",
        "type": "tool_call",
        "id": format!("c{i}"),
        "text": body.to_string(),
    })
}

/// Run one message against the given display and hand back every push.
async fn run_on(db: rusqlite::Connection, turns: Vec<Value>) -> (Vec<WebReconfig>, PageMap) {
    let mut h = harness(db);
    let boot = (**h._pages.borrow()).clone();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(64);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let msg = MessageBuilder::new(Path::new("/display"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(json!({ "messages": turns })))
        .build();
    h.cell.handle(msg, &sink(out_tx), &mut h.db, &rc_tx).await;
    let mut seen = Vec::new();
    while let Ok(push) = h.pushes.try_recv() {
        seen.push(push);
    }
    (seen, boot)
}

/// The diff's keys, sorted, with the statics key `s` kept — a slot diff has
/// none, a packed tree has one.
fn keys(diff: &Value) -> Vec<String> {
    // GH #1013: the root's children are one keyed list (`"0"`, `"k"`); a
    // frame names the entries it changes by index, beside the list's count
    // (`"kc"`) and, for a move, `"km"`. GH #1001: the frame's shared statics
    // (`"p"`) travel beside the list.
    let mut k: Vec<String> = diff["0"]["k"]
        .as_object()
        .unwrap_or_else(|| panic!("a diff names the root list: {diff}"))
        .keys()
        .filter(|k| *k != "kc" && *k != "km")
        .cloned()
        .collect();
    k.sort();
    k
}

/// The HTML of the slot under `key` once the frame has landed on the page a
/// viewer of `route` held from `boot`.
fn slot(boot: &PageMap, route: &str, diff: &Value, key: &str) -> String {
    // GH #1001: a slot is a rendered part, and a frame carries only what
    // changed in it; its markup is what the client builds by merging the
    // frame into the part it holds. A part that brings its statics (`"s"`)
    // stands on its own, against the frame's shared statics.
    let part = diff["0"]["k"]
        .get(key)
        .map(|entry| &entry["0"])
        .unwrap_or_else(|| panic!("no slot under {key}: {diff}"));
    if part.get("s").is_some() || part.is_string() {
        return wire_html(part, &diff["p"]);
    }
    let tree = boot[route].packed_tree();
    let mut held = tree["0"]["k"][key]["0"].clone();
    merge(&mut held, part);
    wire_html(&held, &tree["p"])
}

/// A value-only diff merged into a held part, key by key.
fn merge(held: &mut Value, diff: &Value) {
    match (held, diff) {
        (Value::Object(h), Value::Object(d)) if !d.contains_key("s") => {
            for (k, v) in d {
                match h.get_mut(k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        h.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (h, d) => *h = d.clone(),
    }
}

/// The find: three slots of one route, one frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_over_three_slots_of_one_route_is_one_frame() {
    let (pushes, boot) = run_on(
        one_output_db(),
        vec![
            leg(0, "main", json!({"level": "2"})),
            leg(1, "aside", json!({"level": "1"})),
            leg(2, "dock", json!({"level": "3"})),
        ],
    )
    .await;

    assert_eq!(
        pushes.len(),
        1,
        "one route hears one frame per pass — the live twin sent one per slot, \
         and the frame without the main slot put the optimistic tap back"
    );
    let WebReconfig::Push { route, diff, .. } = &pushes[0] else {
        panic!("a push, not a viewers request")
    };
    assert_eq!(route, ROUTE);
    assert_eq!(
        keys(diff),
        vec!["0", "1", "2"],
        "and that one frame names every slot the pass touched: {diff}"
    );
    for (key, level) in [("0", "2"), ("1", "1"), ("2", "3")] {
        let html = slot(&boot, ROUTE, diff, key);
        assert!(
            html.contains(&format!(r#"data-level="{level}""#)),
            "slot {key} carries the end state of the pass: {html}"
        );
    }
}

/// Two routes stay two frames, one each — the grouping must not merge outputs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_over_two_routes_is_one_frame_each() {
    let (pushes, boot) = run_on(
        two_output_db(),
        vec![
            leg(0, "main-tv", json!({"level": "2"})),
            leg(1, "dock-tv", json!({"level": "1"})),
            leg(2, "main-monitor", json!({"level": "3"})),
            leg(3, "dock-monitor", json!({"level": "4"})),
        ],
    )
    .await;

    assert_eq!(pushes.len(), 2, "two outputs, two frames");
    let mut by_route: Vec<(&str, &Value)> = pushes
        .iter()
        .filter_map(|p| match p {
            WebReconfig::Push { route, diff, .. } => Some((route.as_str(), diff)),
            WebReconfig::Viewers { .. } => None,
        })
        .collect();
    by_route.sort_by_key(|(route, _)| *route);
    assert_eq!(
        by_route.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        vec![MONITOR, TV],
        "each output hears its own pass exactly once"
    );
    for (route, diff) in by_route {
        assert_eq!(
            keys(diff),
            vec!["0", "1"],
            "{route} hears both of its slots in one frame: {diff}"
        );
        let (main, dock) = if route == TV { ("2", "1") } else { ("3", "4") };
        assert!(slot(&boot, route, diff, "0").contains(&format!(r#"data-level="{main}""#)));
        assert!(slot(&boot, route, diff, "1").contains(&format!(r#"data-level="{dock}""#)));
    }
}

/// A pass that leaves one root child for another is ONE frame, and that frame
/// is the root list's difference (GH #1013; it was the whole tree while the
/// root was positional).
///
/// `object.move` names two slots of the same route — the one it left and the
/// one it reached — and it is structural, so each of them used to send that
/// route its whole packed tree: the same 150 KB twice, back to back. It is
/// also the only way the substrate has to make a named slot stop being a slot,
/// which is the case `publish_and_push` answers with the tree for the whole
/// group rather than a positional patch that would miss.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_between_two_slots_of_one_route_is_one_frame_with_the_tree() {
    let (pushes, _) = run_on(
        one_output_db(),
        vec![op(
            0,
            json!({"op": "object.move", "id": "aside", "parent": "dock"}),
        )],
    )
    .await;

    assert_eq!(
        pushes.len(),
        1,
        "the slot it left and the slot it reached are one route, so they are          one frame — not the same tree sent twice"
    );
    let WebReconfig::Push { route, diff, .. } = &pushes[0] else {
        panic!("a push, not a viewers request")
    };
    assert_eq!(route, ROUTE);
    assert!(
        diff.get("s").is_none(),
        "and the frame is no packed tree: the root's children are a keyed list, \
         so `aside` leaving it is a difference of that list (GH #1013): {diff}"
    );
    assert_eq!(
        diff["0"]["k"]["kc"],
        json!(2),
        "the page is down to two slots, and the frame says so in one piece: {diff}"
    );
    assert_eq!(
        diff["0"]["k"]["1"],
        json!(2),
        "`dock` moves up from the index `aside` left, with the node the client \
         holds for it: {diff}"
    );
}

/// A single call keeps its old shape: one slot, one frame, one key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_call_still_sends_its_one_slot() {
    let (pushes, _) = run_on(
        one_output_db(),
        vec![leg(0, "aside", json!({"level": "1"}))],
    )
    .await;
    assert_eq!(pushes.len(), 1, "one write, one push");
    let WebReconfig::Push { diff, .. } = &pushes[0] else {
        panic!("a push, not a viewers request")
    };
    assert_eq!(keys(diff), vec!["1"], "and only the slot it wrote: {diff}");
}
