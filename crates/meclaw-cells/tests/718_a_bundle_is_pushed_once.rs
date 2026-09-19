//! GH #718 — one patch bundle is one push, and the push carries the end state.
//!
//! Measured on the fresh instance during the human acceptance of wave H
//! (18.09.2026, Chromium and WebKit, touch too): a single tap on a tile sent
//! exactly one `tap`, and the window still opened, closed and opened again
//! within ~250 ms. One tap is one curator pass, and one pass arrives at this
//! cell as ONE bundle of eight `object.update` legs. The cell pushed per leg —
//! five of those legs were root updates, each of which re-sends the whole
//! packed tree AS IT STANDS mid-bundle, with the window still closed — so the
//! browser was handed four intermediate pictures before the last leg opened
//! the window. Measured `data-level`: Chromium `1@34 ms → 0@175 → 1@255`,
//! WebKit `1@66 → 0@418 → 1@463`.
//!
//! The claim locked here: a bundle of n legs produces exactly ONE push, and
//! the tree that goes out carries the END state — never a step of the pass.
//! A single call (no bundle) is unchanged: it pushes where it always did.
//!
//! The fixture is the shape the live pass has: five root updates
//! (`Touched { slots: [], structural: true }`) and three slot updates, the
//! last of which is the window opening.

use meclaw_cells::web::cell::{WebCell, WebReconfig};
use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::params::WebParams;
use meclaw_cells::web::render::PageMap;
use meclaw_cells::web::{AssetMap, WebIo};
use meclaw_colony::{DbConn, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// The mount the fixture's display registers. Nothing serves it — `run_io` is
/// never started here — but the cell is built the way the factory builds it.
const MOUNT: &str = "screen";

/// The route the one page of the fixture hangs off.
const ROUTE: &str = "/";

/// A database with the fixture display in it: one root, two root children
/// (`dock` and `aside`, the window), one page.
fn fixture_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    setup_web_schema(&conn).expect("schema");
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
    for (id, parent, component, ord, props) in [
        ("root", None, "screen", 0, r#"{"tick":"0"}"#),
        (
            "dock",
            Some("root"),
            "pane",
            0,
            r#"{"name":"dock","level":"0"}"#,
        ),
        (
            "aside",
            Some("root"),
            "pane",
            1,
            r#"{"name":"aside","level":"0"}"#,
        ),
    ] {
        conn.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, parent, component, ord, props],
        )
        .expect("object");
    }
    conn.execute(
        "INSERT INTO pages (route, root, title) VALUES (?1, ?2, ?3)",
        rusqlite::params![ROUTE, "root", "fixture"],
    )
    .expect("page");
    conn
}

/// The two routes of the second fixture, named after the two outputs of the
/// instance the find was measured on.
const TV: &str = "/tv";
const MONITOR: &str = "/monitor";

/// A display with TWO outputs, which is what `display@2.5.0` builds: one page
/// per output, each with a root of its own.
fn two_output_db() -> rusqlite::Connection {
    let conn = fixture_db();
    for (id, parent, component, ord, props) in [
        ("root-tv", None, "screen", 0, r#"{"tick":"0"}"#),
        (
            "aside-tv",
            Some("root-tv"),
            "pane",
            0,
            r#"{"name":"aside-tv","level":"0"}"#,
        ),
        ("root-monitor", None, "screen", 0, r#"{"tick":"0"}"#),
        (
            "aside-monitor",
            Some("root-monitor"),
            "pane",
            0,
            r#"{"name":"aside-monitor","level":"0"}"#,
        ),
    ] {
        conn.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, parent, component, ord, props],
        )
        .expect("object");
    }
    conn.execute("DELETE FROM pages WHERE route = ?1", [ROUTE])
        .expect("drop the single-output page");
    for (route, root) in [(TV, "root-tv"), (MONITOR, "root-monitor")] {
        conn.execute(
            "INSERT INTO pages (route, root, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![route, root, "fixture"],
        )
        .expect("page");
    }
    conn
}

/// Everything one run needs: the cell, its database, and the push channel the
/// I/O half would be reading.
struct Harness {
    cell: WebCell,
    db: DbConn,
    pushes: mpsc::Receiver<WebReconfig>,
    /// Held: the handler writes into it on every publish, and a closed watch
    /// channel would be a different code path than the live one.
    _pages: watch::Receiver<Arc<PageMap>>,
    _assets: watch::Receiver<Arc<AssetMap>>,
    _ready: watch::Receiver<bool>,
    _reconfig: mpsc::Sender<WebReconfig>,
}

fn harness(db: rusqlite::Connection) -> Harness {
    let (pages_tx, pages_rx) = watch::channel(Arc::new(PageMap::new()));
    let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
    let (ready_tx, ready_rx) = watch::channel(false);
    // Two channels on purpose: the cell pushes into one this test reads, and
    // the I/O half — which never runs here — holds the other.
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

/// A sink that goes nowhere the assertions look — the reply is not what this
/// file is about.
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
    json!({
        "origin": "assistant",
        "type": "tool_call",
        "id": format!("c{i}"),
        "text": json!({"op": "object.update", "id": id, "props": props}).to_string(),
    })
}

/// Run one message of `messages` against the fixture and hand back every push
/// it produced.
async fn run(turns: Vec<Value>) -> Vec<WebReconfig> {
    run_on(fixture_db(), turns).await
}

/// The same against a display of the caller's choosing.
async fn run_on(db: rusqlite::Connection, turns: Vec<Value>) -> Vec<WebReconfig> {
    let mut h = harness(db);
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
    seen
}

/// The HTML of the slot holding `id`, out of a packed tree.
fn slot_html(diff: &Value, id: &str) -> String {
    let needle = format!("id=\"{id}\"");
    diff.as_object()
        .expect("a packed tree is an object")
        .iter()
        .filter(|(k, _)| *k != "s")
        .filter_map(|(_, v)| v.as_str())
        .find(|html| html.contains(&needle))
        .unwrap_or_else(|| panic!("no slot carries {id}: {diff}"))
        .to_string()
}

/// The shape of one tap: eight legs, five of them root updates, and the window
/// opens on the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_of_eight_legs_is_one_push_carrying_the_end_state() {
    let bundle = vec![
        leg(0, "root", json!({"tick": "1"})),
        leg(1, "root", json!({"tick": "2"})),
        leg(2, "root", json!({"tick": "3"})),
        leg(3, "dock", json!({"level": "1"})),
        leg(4, "root", json!({"tick": "4"})),
        leg(5, "aside", json!({"level": "0"})),
        leg(6, "root", json!({"tick": "5"})),
        leg(7, "aside", json!({"level": "1"})),
    ];
    let pushes = run(bundle).await;

    assert_eq!(
        pushes.len(),
        1,
        "a bundle is one push — the live pass sent seven, four of them whole \
         trees from the middle of the bundle"
    );
    let WebReconfig::Push { route, diff } = &pushes[0];
    assert_eq!(route, ROUTE);
    let window = slot_html(diff, "aside");
    assert!(
        window.contains(r#"data-level="1""#),
        "the one push carries the END state of the window, not a step of the \
         pass: {window}"
    );
    let dock = slot_html(diff, "dock");
    assert!(
        dock.contains(r#"data-level="1""#),
        "and every other leg of the same bundle is in it too: {dock}"
    );
}

/// A bundle that touches the same slot repeatedly is one push as well: the
/// dedup is what keeps n legs from becoming n frames.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_that_writes_one_slot_three_times_is_one_push() {
    let pushes = run(vec![
        leg(0, "aside", json!({"level": "1"})),
        leg(1, "aside", json!({"level": "2"})),
        leg(2, "aside", json!({"level": "3"})),
    ])
    .await;
    assert_eq!(pushes.len(), 1, "three legs, one frame");
    let WebReconfig::Push { diff, .. } = &pushes[0];
    let window = slot_html(diff, "aside");
    assert!(window.contains(r#"data-level="3""#), "{window}");
}

/// A single call is not a bundle and keeps the path it always had: one write,
/// one push, immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_call_still_pushes_on_its_own() {
    let pushes = run(vec![leg(0, "aside", json!({"level": "1"}))]).await;
    assert_eq!(pushes.len(), 1, "one write, one push");
    let WebReconfig::Push { diff, .. } = &pushes[0];
    assert!(
        slot_html(diff, "aside").contains(r#"data-level="1""#),
        "{diff}"
    );

    // And the root case of a single call, which pushes the whole tree to every
    // route rather than a slot (GH #414's ROOT-object arm).
    let pushes = run(vec![leg(0, "root", json!({"tick": "9"}))]).await;
    assert_eq!(pushes.len(), 1, "one root write, one push");
    let WebReconfig::Push { diff, .. } = &pushes[0];
    let statics = diff
        .get("s")
        .and_then(Value::as_array)
        .expect("the root case sends the packed tree");
    assert!(
        statics.iter().any(|s| s
            .as_str()
            .map(|s| s.contains(r#"data-tick="9""#))
            .unwrap_or(false)),
        "the root's own props ride the statics: {diff}"
    );
}

/// The broadcast survives a mixed bundle (review of 18.09.2026).
///
/// A root update names no slot — `touched_by` cannot, the whole page IS the
/// slot — so `publish_and_push` answers it by sending every route its packed
/// tree. Accumulating the bundle put the OTHER legs' slots into the same
/// `Touched`, and a `Touched` that names slots takes the addressed path: the
/// output whose root moved would have been left out of its own bundle.
///
/// `display@2.5.0` makes that a live shape and not a hypothesis: it lays down
/// one page per output, each with a root of its own, and one curator pass
/// writes the root props of one output (focus, scale, `data-switch`, `vocab`)
/// and a window of another in the same breath.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_that_moves_one_outputs_root_and_another_outputs_slot_reaches_both() {
    let pushes = run_on(
        two_output_db(),
        vec![
            leg(0, "root-tv", json!({"tick": "9"})),
            leg(1, "aside-monitor", json!({"level": "1"})),
        ],
    )
    .await;

    let mut routes: Vec<&str> = pushes
        .iter()
        .map(|WebReconfig::Push { route, .. }| route.as_str())
        .collect();
    routes.sort_unstable();
    assert_eq!(
        routes,
        vec![MONITOR, TV],
        "both outputs hear the bundle, each exactly once — the output whose \
         root moved must not be left out of its own pass"
    );

    for WebReconfig::Push { route, diff } in &pushes {
        if route == TV {
            let statics = diff
                .get("s")
                .and_then(Value::as_array)
                .expect("the root case sends the packed tree");
            assert!(
                statics.iter().any(|s| s
                    .as_str()
                    .map(|s| s.contains(r#"data-tick="9""#))
                    .unwrap_or(false)),
                "the television carries its own new root props: {diff}"
            );
        } else {
            assert!(
                slot_html(diff, "aside-monitor").contains(r#"data-level="1""#),
                "and the monitor the end state of its window: {diff}"
            );
        }
    }
}
