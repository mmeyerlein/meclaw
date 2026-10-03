//! GH #958 -- a mistyped block is refused at the door, on a running colony.
//!
//! Before the catalogue the `web` cell asked one question of a prop -- is it
//! declared -- and nobody asked what it held: a progress bar sent `"abc"`
//! rendered `--value: abc`, and a step without a label would have drawn an
//! empty row. The compose cell's door now asks the catalogue
//! (`templates/display/compose/catalog.json`) before anything is written: the
//! type of every prop that is said, the props an entry requires, the children it
//! takes. Each refusal is an `invalid_view` receipt to the sender, with the
//! reason `<component>.<prop>: <expected>`, and nothing of the view reaches the
//! store or the screen.
//!
//! Read at the receivers: the receipts where they leave the hive (the marked
//! egress), and the one correct view in the tree `web` was patched with. The
//! correct view is written LAST and is the sentinel: the cell takes one message
//! at a time, so once it stands drawn, the three refusals before it have been
//! answered -- the wait below is for receipts that are already on their way,
//! never a window for something that might not come.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::{Duration, Instant};

use display_colony::{Boot, MARKER, boot, have_python, library_ships, to_screen};
use meclaw_core::Body;
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/steps";

fn pane(children: Value) -> Value {
    json!({"component": "display-pane", "key": "c.win",
           "props": {"pane_id": "pane-steps", "title": "Path", "context": "work",
                     "relevance": "0.9", "topic": "steps:1"},
           "children": children})
}

fn view(view_id: &str, content: Value) -> Value {
    json!({"view_id": view_id, "region": "main", "kind": "component",
           "content": content, "components": [], "ttl_ms": 0, "messages": []})
}

/// The receipts that left the hive, as `(error_code, view_id, detail)`, until
/// `n` refusals are in hand.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mistyped_block_is_refused_at_the_door() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut colony = boot(Boot::default()).await;

    // Three views the catalogue refuses, each for one reason.
    let bad = [
        (
            "typed",
            pane(json!([{"component": "display-progress", "props": {"value": "abc"}}])),
            "display-progress.value: int",
        ),
        (
            "required",
            pane(
                json!([{"component": "display-steps", "props": {"title": "Path"},
                         "children": [{"component": "display-step",
                                       "props": {"state": "done"}}]}]),
            ),
            "display-step.label: required text",
        ),
        (
            "slot",
            pane(
                json!([{"component": "display-steps", "props": {"title": "Path"},
                         "children": [{"component": "display-text",
                                       "props": {"body": "not a step"}}]}]),
            ),
            "display-steps.children: display-step",
        ),
    ];
    for (id, content, _) in &bad {
        colony
            .h
            .send(to_screen("in_view", APP, view(id, content.clone())))
            .await;
    }

    // The same view, correct, is the sentinel: it stands drawn at `web`.
    let good = pane(json!([
        {"component": "display-steps", "props": {"title": "Path"},
         "children": [
            {"component": "display-step", "props": {"label": "Plan", "state": "done"}},
            {"component": "display-step", "props": {"label": "Build", "state": "running"}},
            {"component": "display-step", "props": {"label": "Ship", "state": "todo"}}]},
        {"component": "display-progress", "props": {"value": "40", "label": "Build"}}
    ]));
    let tree = colony.put_content(APP, "good", good).await;
    let drawn = tree.to_string();
    for label in ["Plan", "Build", "Ship"] {
        assert!(
            drawn.contains(&format!("\"{label}\"")),
            "the correct steps view stands at web with its step `{label}`"
        );
    }

    let got = refusals(&mut colony, bad.len()).await;
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
    }
    // And none of the three reached the screen.
    for (id, _, _) in &bad {
        let oid = colony.oid(APP, id);
        assert!(
            !drawn.contains(&oid),
            "the refused view `{id}` reached the screen as {oid}"
        );
    }
    colony.shutdown().await;
}
