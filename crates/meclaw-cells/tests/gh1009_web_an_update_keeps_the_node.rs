//! GH #1009 — a write keeps the browser node of every part it does not replace.
//!
//! The client patches the DOM with morphdom and keys every node of a root part
//! by `id || data-phx-id` (`phoenix_live_view.min.js`, `getNodeKey` in
//! `DOMPatch`). It hands out that `data-phx-id` once per part it holds
//! (`nextMagicID`) and keeps it as long as frames merge INTO the part. A part
//! that arrives whole — with `"s"` — is a new part to the client, gets a new
//! id, and its node is thrown away and inserted again: a card that glides in on
//! mount glides in again on a plain update, and 200 marks given new targets are
//! re-created instead of moved (the display's motion lock, GH #961, went red
//! with exactly that after the part form of GH #1001: `same_node: false`,
//! `replaced: 6`, `moved: 0`).
//!
//! These locks feed the lab page's join and one write's frame to the vendored
//! client (`support/lv_client.mjs`) and follow every element's `data-phx-id`
//! across the frame, for the three ways a frame used to bring parts whole:
//!
//! - a write that names the page root (the root-object broadcast, one curator
//!   pass writes a root prop the template does not even show): the whole page
//!   came as the packed tree;
//! - a child list that grows or shrinks: its statics (n + 1 strings) changed,
//!   so the list came whole with every child in it;
//! - a reorder: positions were diffed, so the node at a position was handed the
//!   next object's values and the object changed its node.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use std::collections::BTreeMap;

use meclaw_cells::web::cell::WebReconfig;
use meclaw_core::serde_json::{Value, json};
use web_fixture::held::HeldLab;
use web_fixture::{Lab, ROOT, Shape, kid, update};

fn client_js() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-surface/src/client/phoenix_live_view.min.js")
}

fn driver() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/lv_client.mjs")
}

/// The client's untracked markup after the join and after every diff, or
/// `None` when there is no node on this host (the gate hosts have one).
fn client_fulls(join: &Value, diffs: &[Value]) -> Option<Vec<String>> {
    let td = tempfile::TempDir::new().expect("tempdir");
    let steps = td.path().join("steps.json");
    std::fs::write(&steps, json!({"join": join, "diffs": diffs}).to_string()).expect("steps");
    let out = std::process::Command::new("node")
        .arg(driver())
        .arg(client_js())
        .arg(&steps)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) {
        println!("{stderr}");
        return None;
    }
    assert!(out.status.success(), "the client driver failed: {stderr}");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("driver JSON");
    Some(
        v["fulls"]
            .as_array()
            .expect("fulls")
            .iter()
            .map(|s| s.as_str().expect("html").to_string())
            .collect(),
    )
}

/// Every element that carries a `data-phx-id`, by its opening tag without the
/// id: the lab's tags name their object (`data-id`, `data-c`, `data-x` +
/// `data-y`), so the key is the object and the value is its node.
fn nodes(html: &str) -> BTreeMap<String, String> {
    const ATTR: &str = " data-phx-id=\"";
    let mut out = BTreeMap::new();
    let mut from = 0;
    while let Some(rel) = html[from..].find(ATTR) {
        let at = from + rel;
        let start = html[..at].rfind('<').expect("the tag opens");
        let id_start = at + ATTR.len();
        let id_end = id_start + html[id_start..].find('"').expect("the id closes");
        let end = id_end + html[id_end..].find('>').expect("the tag closes");
        let key = format!("{}{}", &html[start..at], &html[id_end + 1..end]);
        let id = html[id_start..id_end].to_string();
        assert!(
            out.insert(key.clone(), id).is_none(),
            "two nodes carry the same tag, the lab names every object once: {key}"
        );
        from = end;
    }
    out
}

/// Every object in both renderings that is not in `except` stands on the node
/// it stood on before the frame.
fn kept(before: &str, after: &str, except: &[&str]) -> Vec<String> {
    let (a, b) = (nodes(before), nodes(after));
    assert!(a.len() > 40, "the lab page has its parts: {}", a.len());
    a.iter()
        .filter(|(tag, _)| !except.iter().any(|e| tag.contains(e)))
        .filter_map(|(tag, id)| match b.get(tag) {
            Some(now) if now != id => Some(format!("{tag}: {id} -> {now}")),
            _ => None,
        })
        .collect()
}

/// The answer reports no error.
fn ok(answer: &Value) {
    let text = answer.to_string();
    assert!(
        !text.contains("\"error_code\""),
        "the write was refused: {text}"
    );
}

/// The page body a GET serves, cut out of the shell.
fn served_body(page: &str) -> String {
    let open = "data-phx-static=\"\">\n";
    let start = page.find(open).expect("the shell's container") + open.len();
    let end = start
        + page[start..]
            .find("\n</div>\n<script")
            .expect("the container closes before the scripts");
    page[start..end].to_string()
}

/// The markup with the client's own bookkeeping attributes taken out.
fn without_client_ids(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(" data-phx-id=\"") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + " data-phx-id=\"".len()..];
        let end = tail.find('"').expect("closing quote");
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn small() -> Shape {
    Shape {
        figures: 40,
        chunks: 4,
        kids: 16,
    }
}

/// One write through the lab, through the client: the renderings before and
/// after its frame, and the frame. Asserts that the client ends on the page a
/// GET serves.
async fn through_the_client(ops: Vec<Value>) -> Option<(String, String, Value)> {
    through_the_client_on(small(), ops).await
}

/// [`through_the_client`] on a lab of `shape`.
async fn through_the_client_on(shape: Shape, ops: Vec<Value>) -> Option<(String, String, Value)> {
    let mut lab = Lab::start(shape).await;
    let mut viewer = lab.viewer().await;
    ok(&lab.call(ops).await);
    let (_, diff) = viewer.next_diff().await;
    let fulls = client_fulls(&viewer.rendered, std::slice::from_ref(&diff))?;
    let body = served_body(&lab.get_page().await);
    assert_eq!(
        without_client_ids(&fulls[1]),
        body,
        "the client builds the page a GET serves"
    );
    Some((fulls[0].clone(), fulls[1].clone(), diff))
}

/// The root-object path: a bundle that writes the page root — here a value its
/// template shows unchanged, as a curator's `due` is not shown at all — and a
/// lamp in one house. The root's statics are the same, so the frame is the
/// house's difference and every node stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1009_a_root_write_keeps_every_node() {
    let Some((before, after, diff)) = through_the_client(vec![
        update(ROOT, json!({"w": 4096})),
        update(&kid(1, 2), json!({"lit": true})),
    ])
    .await
    else {
        return;
    };
    assert!(
        after.contains("<i class=\"lamp\"></i>"),
        "the lamp is drawn"
    );
    let moved = kept(&before, &after, &[]);
    assert!(
        moved.is_empty(),
        "every node stays where it was, {} did not: {moved:?}\nframe: {}",
        moved.len(),
        &diff.to_string()[..diff.to_string().len().min(400)]
    );
    assert!(
        diff.get("s").is_none(),
        "the root's statics did not change, so the frame is no packed tree"
    );
}

/// The child-list path: in one chunk a child created and one deleted (the
/// same length, every child behind the new one one place further), in another
/// one deleted (shorter), in a third one created at the end (longer). Every
/// child that was there before and is there after keeps its node.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1009_a_child_list_that_grows_or_shrinks_keeps_its_nodes() {
    let Some((before, after, diff)) = through_the_client(vec![
        json!({"op": "object.create", "id": "c1-new", "parent": "chunk-1",
               "component": "tree", "ord": 3, "props": {"x": 7, "y": 7}}),
        json!({"op": "object.delete", "id": kid(1, 6)}),
        json!({"op": "object.delete", "id": kid(2, 9)}),
        json!({"op": "object.create", "id": "c3-new", "parent": "chunk-3",
               "component": "tree", "ord": 100, "props": {"x": 9, "y": 9}}),
    ])
    .await
    else {
        return;
    };
    assert!(
        after.contains("data-x=\"7\" data-y=\"7\""),
        "the new child is drawn"
    );
    let moved = kept(&before, &after, &[]);
    assert!(
        moved.is_empty(),
        "every node that stays stays where it was, {} did not: {moved:?}\nframe: {}",
        moved.len(),
        &diff.to_string()[..diff.to_string().len().min(400)]
    );
}

/// The reorder path: a child moved to the end of its chunk, and one moved into
/// another chunk. Every object keeps its node — the one that changed parents
/// is a new node in its new list, as it was before parts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1009_a_reorder_keeps_each_objects_node() {
    let mv = |id: &str, parent: &str, ord: i64| json!({"op": "object.move", "id": id, "parent": parent, "ord": ord});
    let Some((before, after, diff)) = through_the_client(vec![
        mv(&kid(0, 3), "chunk-0", 100),
        mv(&kid(1, 5), "chunk-2", 2),
    ])
    .await
    else {
        return;
    };
    // kid(1, 5) is a tree at x 80, y 16.
    let moved = kept(&before, &after, &["data-x=\"80\" data-y=\"16\""]);
    assert!(
        moved.is_empty(),
        "every object stays on its node, {} did not: {moved:?}\nframe: {}",
        moved.len(),
        &diff.to_string()[..diff.to_string().len().min(400)]
    );
}

/// A lab of 1 000 root children: 999 figures and one chunk (GH #1013).
fn root_of_1000() -> Shape {
    Shape {
        figures: 999,
        chunks: 1,
        kids: 2,
    }
}

/// GH #1013: one structure op directly under the root, through the client.
/// Every object that is there before and after the frame keeps its node, and
/// the frame is no packed tree. Before the fix the root list was positional:
/// a create under the root changed the root's statics, the page went out as
/// the packed tree, and 1 002 of 1 002 nodes were new (frame 107 693 B,
/// OR-H4.W1b.1).
async fn under_the_root(what: &str, ops: Vec<Value>) -> Option<String> {
    let (before, after, diff) = through_the_client_on(root_of_1000(), ops).await?;
    let replaced = kept(&before, &after, &[]);
    let frame = diff.to_string();
    println!(
        "NOTE {what} under a root of 1000: {} of {} nodes replaced, frame {} B",
        replaced.len(),
        nodes(&before).len(),
        frame.len()
    );
    assert!(
        replaced.is_empty(),
        "{what}: every other root child keeps its node, {} did not: {:?}\nframe: {}",
        replaced.len(),
        &replaced[..replaced.len().min(5)],
        &frame[..frame.len().min(400)]
    );
    assert!(
        diff.get("s").is_none(),
        "{what}: the root's statics did not change, so the frame is no packed tree"
    );
    Some(after)
}

/// GH #1013: a create under the root adds one node and keeps every other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1013_a_create_under_the_root_keeps_every_other_node() {
    let Some(after) = under_the_root(
        "create",
        vec![
            json!({"op": "object.create", "id": "fig-new", "parent": ROOT,
                    "component": "fig", "ord": 5000,
                    "props": {"kind": "walker", "name": "fig-new", "x": 1, "y": 1, "dir": "s"}}),
        ],
    )
    .await
    else {
        return;
    };
    assert!(
        after.contains("data-id=\"fig-new\""),
        "the new figure is drawn"
    );
}

/// GH #1013: a delete under the root removes one node and keeps every other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1013_a_delete_under_the_root_keeps_every_other_node() {
    let Some(after) = under_the_root(
        "delete",
        vec![json!({"op": "object.delete", "id": "fig-500"})],
    )
    .await
    else {
        return;
    };
    assert!(!after.contains("data-id=\"fig-500\""), "the figure is gone");
}

/// GH #1013: a move under the root keeps every node, the moved one included.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1013_a_move_under_the_root_keeps_every_node() {
    let Some(after) = under_the_root(
        "move",
        vec![json!({"op": "object.move", "id": "fig-10", "parent": ROOT, "ord": 5000})],
    )
    .await
    else {
        return;
    };
    let (moved, last) = (
        after.find("data-id=\"fig-10\"").expect("fig-10 is drawn"),
        after.find("data-id=\"fig-998\"").expect("fig-998 is drawn"),
    );
    assert!(moved > last, "fig-10 stands after the last figure");
}

/// GH #1013 (review C1): a join that falls between a write's publish and its
/// fan-out reads a snapshot that already holds the write — and the write's
/// diff used to reach it as well. Under the root a diff is a keyed-list diff
/// whose moves copy entries the client HOLDS (`mergeKeyed`), so applying it a
/// second time is no longer harmless: the delete below, applied to the page
/// that already lacks the figure, drops a second figure and leaves an entry
/// undefined. The window is forced here ([`HeldLab`]): write, join, then the
/// write's diff, then a later write's diff as the end marker. The client must
/// end on the page a GET serves, and the stale diff must not reach it.
async fn a_diff_older_than_the_join(shape: Shape, pieces: bool) -> Option<()> {
    let mut lab = HeldLab::start(shape, json!({})).await;
    let older = lab
        .write(vec![json!({"op": "object.delete", "id": "fig-5"})])
        .await;
    assert_eq!(older.len(), 1, "one route, one frame");
    let WebReconfig::Push {
        diff: stale_diff, ..
    } = &older[0]
    else {
        panic!("a write pushes a diff");
    };
    let stale_diff = stale_diff.clone();
    let mut viewer = lab.viewer().await;
    lab.release(older).await;
    let newer = lab
        .write(vec![update("fig-7", json!({"x": 7, "y": 3}))])
        .await;
    let WebReconfig::Push { diff: marker, .. } = &newer[0] else {
        panic!("a write pushes a diff");
    };
    let marker = marker.clone();
    lab.release(newer).await;
    // Join pieces (a large page), then whatever else arrives up to the marker.
    let mut diffs = Vec::new();
    loop {
        let (_, diff) = viewer.next_diff().await;
        let end = diff == marker;
        diffs.push(diff);
        if end {
            break;
        }
    }
    let stale = diffs.iter().filter(|d| **d == stale_diff).count();
    if pieces {
        assert!(
            diffs.len() > 1 + stale,
            "the join came in pieces: {} diffs before the marker",
            diffs.len()
        );
    }
    let fulls = client_fulls(&viewer.rendered, &diffs)?;
    // A page above the join cut is served as its first picture only
    // (GH #1002); its whole body is what the pieces build.
    let body = if pieces {
        lab.whole_body().await
    } else {
        served_body(&lab.get_page().await)
    };
    assert_eq!(
        without_client_ids(fulls.last().expect("a rendering")),
        body,
        "the client builds the page a GET serves ({stale} delete diff(s) older than the join reached it)"
    );
    assert_eq!(
        stale, 0,
        "the delete's diff is older than the join and stays away"
    );
    Some(())
}

/// C1, a join in one frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1013_a_diff_older_than_the_join_does_not_reach_the_viewer() {
    a_diff_older_than_the_join(small(), false).await;
}

/// C1, a join in pieces (GH #1002): the default lab page is above the cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1013_a_diff_older_than_a_join_in_pieces_does_not_reach_the_viewer() {
    a_diff_older_than_the_join(Shape::default(), true).await;
}
