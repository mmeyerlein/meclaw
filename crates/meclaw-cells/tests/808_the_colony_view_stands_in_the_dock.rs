//! GH #808 -- the colony view stands in the dock, a tap opens it, and a
//! committed mutation lifts it onto the canvas. Measured at the `web` cell.
//!
//! The script lock beside this one (`808_the_colony_view_is_a_window`) shows
//! that the layout says the curator's words. This one shows that the screen
//! ACTS on them, in a real colony: `examples/display-colony-view` as it ships
//! (its seed, its `grow.json`, the library templates), booted, grown, and the
//! patch bundles that reached `/display/web` folded into the tree a browser
//! holds. Three claims, one per sentence of the #808 acceptance:
//!
//! (a) after the grow receipt the dock holds a tile for the view,
//! (b) a tap on that tile opens the window onto the canvas band (`ord < 0`),
//! (c) a committed mutation sends a new view with a greater `touched`, and the
//!     window is drawn present again.
//!
//! Read at the receiver, never at the curator: the tree is the fold of the
//! `patch` hops (`object.*` calls) and nothing here reads the state row, so the
//! lock holds on the display that ships today and on the one that keeps its
//! state elsewhere (the three claims are about what a browser is sent).
//!
//! Two values of the example are bent, both the screen's timing: `linger_ms`
//! and `fade_ms`, so the window that the grow receipt opens leaves the canvas
//! in seconds and the tap in (b) is an opening tap rather than a putting-away
//! one (a tap on an OPEN window puts it away, display-hive.md § 5.2).

#[path = "support/display_colony.rs"]
mod display_colony;
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use display_colony::{MARKER, calls_of, hop_of};
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::api_dto::MessageLogFilter;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

/// The layout cell of the grown app: the owner half of the view's identity.
const LAYOUT: &str = "/colony-view/layout";
const WEB: &str = "/display/web";
const COMPOSE: &str = "/display/compose";
/// The dock of the screen (`compose.py`, `DOCK_ID`).
const DOCK: &str = "display.dock";
/// The key the layout gives its window (`layout.py`, `WINDOW_KEY`).
const WINDOW_KEY: &str = "colony-view.window";
/// Short, so a lock that waits for the window to leave is a lock and not a nap.
const LINGER_MS: u64 = 1500;
const FADE_MS: u64 = 2000;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("dirs");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

fn window_oid() -> String {
    format!("view.{}.colony-view", LAYOUT.replace('/', "~"))
}

struct Stage {
    _td: tempfile::TempDir,
    h: ColonyHandle,
}

async fn mutate(h: &ColonyHandle, payload: Value) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    let outcome = ack_rx.await.expect("mutation ack");
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "precondition: the mutation commits; got {outcome:?}"
    );
}

/// `examples/display-colony-view`, booted and grown as it ships -- except for the
/// screen's two timing dials.
async fn stage() -> Stage {
    let ex = repo("examples/display-colony-view");
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    copy_tree(&ex.join("seed"), root);
    for name in ["display", "colony-view", "terminal", "web"] {
        copy_tree(
            &repo(&format!("templates/{name}")),
            &root.join("templates").join(name),
        );
    }

    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            (
                "web".to_string(),
                Arc::new(WebCellFactory::new(Arc::clone(&surfaces))),
            ),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the seed of examples/display-colony-view boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("the template table fills");

    let mut grow = read_json(&ex.join("grow.json"));
    for node in grow["manifest"][0]["diff"]["add_nodes"]
        .as_array_mut()
        .expect("add_nodes")
    {
        if node["name"] == "display" {
            node["override_params"]["compose"] =
                json!({"linger_ms": LINGER_MS, "fade_ms": FADE_MS});
        }
    }
    for entry in grow["manifest"].as_array().expect("manifest").clone() {
        mutate(&h, entry).await;
    }
    Stage { _td: td, h }
}

impl Stage {
    /// Every patch bundle that reached the `web` cell, oldest first, as its calls.
    async fn patches(&self) -> Vec<Vec<Value>> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    to_path_prefix: Some(WEB.to_string()),
                    limit: 100_000,
                    scan_budget: 500_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the log answers")
            .expect("ack");
        assert!(!reply.scan_truncated, "the log outgrew the scan budget");
        let mut rows = reply.entries;
        rows.reverse();
        rows.iter()
            .filter(|row| hop_of(row)["route"] == "patch")
            .filter_map(calls_of)
            .collect()
    }

    /// The tree a browser holds: every patch bundle folded, in order.
    async fn tree(&self) -> Vec<Value> {
        let mut held = json!([]);
        for calls in self.patches().await {
            support::apply(&mut held, &calls);
        }
        held.as_array().expect("a list").clone()
    }

    /// Every `touched` the app said on an `in_view` that reached the pass.
    async fn touches(&self) -> Vec<u64> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    to_path_prefix: Some(COMPOSE.to_string()),
                    limit: 100_000,
                    scan_budget: 500_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the log answers")
            .expect("ack");
        let mut rows = reply.entries;
        rows.reverse();
        rows.iter()
            .filter(|row| hop_of(row)["route"] == "in_view")
            .filter_map(|row| {
                let body = display_colony::body_of(row);
                let content = match &body["content"] {
                    Value::String(s) => meclaw_core::serde_json::from_str(s).ok()?,
                    v => v.clone(),
                };
                content["props"]["touched"].as_str()?.parse().ok()
            })
            .collect()
    }

    /// Wait until the folded tree satisfies `want`; the tree it held then.
    async fn wait_tree(&self, why: &str, want: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + MARKER;
        loop {
            let tree = self.tree().await;
            if want(&tree) {
                return tree;
            }
            if Instant::now() > deadline {
                let mine: Vec<&Value> = tree
                    .iter()
                    .filter(|o| {
                        o["parent"] == DOCK
                            || o["id"]
                                .as_str()
                                .is_some_and(|id| id.starts_with(&window_oid()))
                    })
                    .map(|o| &o["props"])
                    .collect();
                let dlq = self.h.drain_dead_letters().await;
                panic!(
                    "{why} did not hold within 30 s; {} objects held, the view's and the \
                     dock's: {mine:?}; DLQ {dlq:?}",
                    tree.len()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A finger on the tile, as the `web` cell hands it on: `event` from `./web`.
    ///
    /// Sent FROM the web cell's path because that is the only way in. Addressed from
    /// outside, `/display/compose` is an interior address and the message dead-letters
    /// as `HiveBoundary` (measured on the first run of this lock); the hive's own edge
    /// `./web -> ./compose` on `event` is what carries a browser's tap. The socket leg
    /// before it -- LiveView push to web cell -- is the business of
    /// `710_the_screen_hears_only_what_is_said` and not repeated here.
    async fn tap(&self, oid: &str) {
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".into(), json!("event"));
        self.h
            .send_from(
                Path::new(WEB),
                MessageBuilder::new(Path::new(COMPOSE))
                    .reply_to(Path::new(WEB))
                    .hop(hop)
                    .body(Body::Inline(json!({
                        "messages": [],
                        "event": {"name": "tap", "value": {"for": oid}}
                    })))
                    .ttl(24)
                    .build(),
            )
            .await;
    }
}

fn object<'a>(tree: &'a [Value], id: &str) -> Option<&'a Value> {
    tree.iter().find(|o| o["id"] == id)
}

/// The tile of the view in the dock, if the dock holds one.
fn tile(tree: &[Value]) -> Option<&Value> {
    let oid = window_oid();
    tree.iter().find(|o| {
        o["parent"] == DOCK && o["component"] == "display-tile" && o["props"]["oid"] == oid
    })
}

fn is_open(tree: &[Value]) -> bool {
    tile(tree).is_some_and(|t| t["props"]["open"] == "1")
}

fn on_canvas(tree: &[Value]) -> bool {
    object(tree, &window_oid()).is_some_and(|w| w["ord"].as_i64().is_some_and(|o| o < 0))
}

fn pane(tree: &[Value]) -> Option<&Value> {
    object(tree, &format!("{}/{WINDOW_KEY}", window_oid()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_colony_view_stands_in_the_dock_opens_on_a_tap_and_rises_on_a_mutation() {
    if !repo("examples/display-colony-view/grow.json").is_file()
        || !repo("templates/colony-view/template.json").is_file()
        || !repo("templates/display/template.json").is_file()
        || !display_colony::have_python()
    {
        return;
    }
    let started = Instant::now();
    let s = stage().await;
    let oid = window_oid();

    // (a) The grow receipt draws the first picture, and the dock holds its tile.
    let tree = s
        .wait_tree("(a) a tile for the colony view in the dock", |t| {
            tile(t).is_some()
        })
        .await;
    let to_tile = started.elapsed();
    let t = tile(&tree).expect("the tile");
    eprintln!("(a) tile after {to_tile:?}: {}", t["props"]);
    assert_eq!(
        t["props"]["for"], "colony-view-window",
        "the tile points at the pane's DOM id"
    );
    assert_eq!(t["props"]["unit"], "cells");
    assert!(
        t["props"]["value"]
            .as_str()
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > 0),
        "the tile counts the colony's cells: {}",
        t["props"]
    );
    assert_eq!(t["props"]["pinned"], "1", "the colony's tile is pinned");

    // The window the grow receipt opened leaves the canvas after the linger.
    s.wait_tree("the first opening lingers out", |t| {
        tile(t).is_some() && !is_open(t)
    })
    .await;

    // (b) A tap on the tile opens the window onto the canvas band.
    s.tap(&oid).await;
    let tree = s
        .wait_tree("(b) the tapped window stands on the canvas", |t| {
            on_canvas(t) && is_open(t)
        })
        .await;
    eprintln!(
        "(b) window ord {} after the tap",
        object(&tree, &oid).expect("the window")["ord"]
    );

    // And leaves again, so what (c) sees is the mutation's doing.
    s.wait_tree("the tapped window lingers out", |t| !is_open(t))
        .await;

    // (c) A committed mutation: a new view with a greater `touched`, drawn present.
    let before = s.touches().await;
    let last = *before.iter().max().expect("the app has written its view");
    mutate(
        &s.h,
        json!({"scope": "/", "diff": {"add_edges": [{
            "from": "./tick", "to": "./sink",
            "condition": "has(hop.route) && hop.route == 'never'"
        }]}}),
    )
    .await;
    let deadline = Instant::now() + MARKER;
    let newer = loop {
        let now = s.touches().await;
        if let Some(t) = now.iter().copied().find(|t| *t > last) {
            break t;
        }
        assert!(
            Instant::now() < deadline,
            "(c) no view with a touched greater than {last} after the mutation: {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let tree = s
        .wait_tree("(c) the mutation lifts the window", |t| {
            is_open(t)
                && pane(t).is_some_and(|p| {
                    let rung = p["props"]["rung"].as_str().unwrap_or("");
                    !rung.is_empty() && rung != "hidden"
                })
        })
        .await;
    eprintln!(
        "(c) touched {last} -> {newer}; rung {}",
        pane(&tree).expect("the pane")["props"]["rung"]
    );
}
