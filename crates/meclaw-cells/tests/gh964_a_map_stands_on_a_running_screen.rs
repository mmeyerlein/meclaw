//! GH #964 -- a map on a running screen: the tiles come from the operator's
//! setting, never from the application.
//!
//! A colony whose display carries `map_tiles` (an address pattern on a test host
//! that is never fetched here -- the tree is read where `web` takes it, not in a
//! browser) gets a window with a `display-map` of a capital at zoom 12, two by two.
//! The tree that reaches `web` holds the map and four `display-map-tile` children,
//! each addressed from the setting with the expected indices, the setting's credit
//! line and a pin. Two views the door refuses go first: one whose latitude is an
//! address, one that sends a tile of its own. Both are `invalid_view` receipts and
//! neither reaches the screen.
//!
//! The second colony is the shipped default, `map_tiles` empty: the same map
//! stands with its name and coordinates and without one tile.
//!
//! The correct view is written LAST and is the sentinel: the compose cell takes one
//! message at a time, so once it stands drawn, the refusals before it have been
//! answered (the same order `gh958_a_mistyped_block_is_refused_at_the_door` uses).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::{Duration, Instant};

use display_colony::{
    Boot, MARKER, boot, boot_with, have_python, library_ships, patch_json, to_screen,
};
use meclaw_core::Body;
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/place";
const SETTING: &str = "https://tiles.example.test/{z}/{x}/{y}.png";
const CREDIT: &str = "Tiles: example data";

fn pane(children: Value) -> Value {
    json!({"component": "display-pane", "key": "c.win",
           "props": {"pane_id": "pane-place", "title": "Place", "context": "conversation",
                     "relevance": "0.8", "topic": "show:place"},
           "children": children})
}

fn the_map() -> Value {
    json!({"component": "display-map", "key": "map",
           "props": {"lat": "52.52", "lon": "13.405", "zoom": 12, "w": 2, "h": 2,
                     "label": "Capital"}})
}

fn view(view_id: &str, content: Value) -> Value {
    json!({"view_id": view_id, "region": "main", "kind": "component",
           "content": content, "components": [], "ttl_ms": 0, "messages": []})
}

/// The receipts that left the hive, as `(error_code, view_id, detail)`, until `n`
/// refusals are in hand.
async fn refusals(colony: &mut display_colony::Colony, n: usize) -> Vec<(String, String, String)> {
    let deadline = Instant::now() + MARKER;
    let mut out = Vec::new();
    loop {
        while let Ok(m) = colony.egress.try_recv() {
            if m.headers.hop.get("route").and_then(Value::as_str) != Some("receipt") {
                continue;
            }
            let Body::Inline(body) = &m.body else {
                continue;
            };
            let r = &body["receipt"];
            if r["error_code"].as_str().unwrap_or("").is_empty() {
                continue;
            }
            out.push((
                r["error_code"].as_str().unwrap_or("").to_string(),
                r["view_id"].as_str().unwrap_or("").to_string(),
                r["detail"].as_str().unwrap_or("").to_string(),
            ));
        }
        if out.len() >= n {
            return out;
        }
        assert!(
            Instant::now() < deadline,
            "only {} of {n} refusals left the hive within 30s: {out:?}",
            out.len()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The objects of the folded tree whose component is `component`, as `(id, props)` --
/// on the main tree only (`view.…`): each configured output carries its own mirror of
/// the same view (`tv.view.…`, `monitor.view.…`, `phone.view.…`), so one map stands
/// once per output.
fn of(tree: &Value, component: &str) -> Vec<(String, Value)> {
    tree.as_object()
        .map(|m| {
            m.iter()
                .filter(|(id, o)| id.starts_with("view.") && o["component"] == component)
                .map(|(id, o)| (id.clone(), o["props"].clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_map_draws_its_tiles_from_the_setting_only() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut colony = boot_with(Boot::default(), |root| {
        patch_json(
            &root.join("main/alex/channels/display/compose/config.json"),
            |v| {
                v["params"]["map_tiles"] = json!(SETTING);
                v["params"]["map_attribution"] = json!(CREDIT);
            },
        );
    })
    .await;

    let bad = [
        (
            "address",
            pane(json!([{"component": "display-map",
                         "props": {"lat": "https://tiles.example.test/1/1/1.png",
                                   "lon": "13.405"}}])),
            "display-map.lat: number -90..90",
        ),
        (
            "own-tile",
            pane(
                json!([{"component": "display-map", "props": {"lat": "1", "lon": "1"},
                         "children": [{"component": "display-map-tile",
                                       "props": {"src": "https://elsewhere.example/x.png",
                                                 "col": 1, "row": 1}}]}]),
            ),
            "display-map-tile: written by the screen",
        ),
    ];
    for (id, content, _) in &bad {
        colony
            .h
            .send(to_screen("in_view", APP, view(id, content.clone())))
            .await;
    }

    let tree = colony
        .put_content(APP, "place", pane(json!([the_map()])))
        .await;
    let maps = of(&tree, "display-map");
    assert_eq!(maps.len(), 1, "one map on the screen: {maps:?}");
    let (map_id, props) = &maps[0];
    assert_eq!(props["attribution"], CREDIT, "{props}");
    assert_eq!(props["label"], "Capital", "{props}");
    assert!(
        props["pin_x"].as_i64().unwrap_or(0) > 0 && props["pin_y"].as_i64().unwrap_or(0) > 0,
        "the screen wrote the pin: {props}"
    );
    let mut srcs: Vec<String> = of(&tree, "display-map-tile")
        .iter()
        .map(|(id, p)| {
            assert!(
                id.starts_with(&format!("{map_id}/")),
                "{id} hangs under the map {map_id}"
            );
            p["src"].as_str().unwrap_or("").to_string()
        })
        .collect();
    srcs.sort();
    // The capital at zoom 12 lies on 2200/1342; two by two around it the grid starts
    // one tile up and to the left of the point's own tile or on it, whichever centres it.
    assert_eq!(srcs.len(), 4, "four tiles: {srcs:?}");
    for s in &srcs {
        assert!(
            s.starts_with("https://tiles.example.test/12/") && s.ends_with(".png"),
            "every tile is addressed from the setting: {s}"
        );
    }
    assert!(
        srcs.iter()
            .any(|s| s == "https://tiles.example.test/12/2200/1342.png"),
        "the point's own tile is one of them: {srcs:?}"
    );

    let got = refusals(&mut colony, bad.len()).await;
    let drawn = tree.to_string();
    for (id, _, reason) in &bad {
        let hit = got
            .iter()
            .find(|(_, v, _)| v == id)
            .unwrap_or_else(|| panic!("no receipt for `{id}`: {got:?}"));
        assert_eq!(hit.0, "invalid_view", "`{id}` is an invalid view");
        assert!(
            hit.2.contains(reason),
            "`{id}` is refused with the reason `{reason}`: {}",
            hit.2
        );
        let oid = colony.oid(APP, id);
        assert!(
            !drawn.contains(&oid),
            "the refused view `{id}` reached the screen as {oid}"
        );
    }
    assert!(
        !drawn.contains("elsewhere.example"),
        "no address an application named reached the screen"
    );
    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_setting_the_map_is_its_name_and_its_coordinates() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot(Boot::default()).await;
    let tree = colony
        .put_content(APP, "place", pane(json!([the_map()])))
        .await;
    let maps = of(&tree, "display-map");
    assert_eq!(maps.len(), 1, "one map on the screen: {maps:?}");
    let props = &maps[0].1;
    assert_eq!(
        (
            props["lat"].as_str(),
            props["lon"].as_str(),
            props["label"].as_str()
        ),
        (Some("52.52"), Some("13.405"), Some("Capital")),
        "{props}"
    );
    assert_eq!(
        props["attribution"], "",
        "no tiles, no credit line: {props}"
    );
    assert!(
        of(&tree, "display-map-tile").is_empty(),
        "the shipped default fetches nothing: {tree}"
    );
    colony.shutdown().await;
}
