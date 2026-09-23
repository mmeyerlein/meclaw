//! GH #455 -- what the screen promises, driven through the SHIPPED bytes.
//!
//! Every door test here reads its script out of `params.script_inline` and runs
//! it through the runner the `code` cell declares, which is the same command the
//! substrate builds. A test that ran the `.py` beside it would prove the source and
//! not the product; `gh455_the_two_templates_ship` is what pins that the two agree.
//! What needs the curator's memory across messages -- two owners on one screen, a
//! withdrawal, the patch a real display takes -- talks to ONE living cell the way
//! a `resident` code cell runs it (`support::Screen`, GH #809), with the store and
//! the display beside it, and reads what came out at the seams: the store bundle,
//! the store's rows, the patch.
//!
//! The six promises, in the order they are made:
//!
//! (a) the owner of a view is the ENVELOPE, and a body that claims a different
//!     one is refused rather than believed;
//! (b) two owners hold two views on one screen at the same time, in an order
//!     that no rewrite moves (`newest first` until GH #609);
//! (c) a withdrawal removes the caller's own view and no other;
//! (d) an application's view carries its components, every name prefixed with
//!     the view's own id -- driven with the bytes `colony-view` really emits;
//! (e) both views reach a REAL display, over HTTP, on a page a browser gets;
//! (f) a view whose `ttl_ms` has elapsed stays in the store -- the store holds
//!     what an app wrote -- and the PASS is what takes it off the screen.
//!
//! Free of a provider by construction: neither template holds a model.

mod support;

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::{surface_listener, wait_for_mount};
use support::Screen;

/// The name this fixture's display answers to. The port in every URL below is
/// the LISTENER's: a `web` cell has none since `web@2.0.0`.
const MOUNT: &str = "display";
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The two hives, or `None` when either is not on disk. A file that silently
/// disappears makes these tests skip rather than pass (R2b).
fn shipped(name: &str, marker: &str) -> Option<std::path::PathBuf> {
    let root = core_root().join("templates").join(name);
    root.join(marker).exists().then_some(root)
}

fn display() -> Option<std::path::PathBuf> {
    shipped("display", "compose/config.json")
}

fn colony_view() -> Option<std::path::PathBuf> {
    shipped("colony-view", "layout/config.json")
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

/// Hand the script to the runner on STDIN instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the app's
/// layout carries the whole browser half, so `<runner> -c <whole script>` is a
/// harness that breaks on size rather than on behaviour (GH #349, GH #279).
fn run_script_on_stdin(runner: &str, script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new(runner)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run one shipped cell exactly as the `code` cell runs it.
fn run_shipped(root: &std::path::Path, cell: &str, stdin_doc: Value) -> Vec<Value> {
    let cfg = read_json(&root.join(cell).join("config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap();
    let out = run_script_on_stdin(runner, script, &stdin_doc.to_string());
    assert!(
        out.status.success(),
        "{cell} exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell} stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The three-key document `wire::build_stdin_json` builds.
fn stdin_doc(body: Value, hop: Value, context: Value, reply_to: Option<&str>) -> Value {
    let mut envelope = json!({
        "header": {"hop": hop, "context": context},
        "target": "/display",
        "trace_id": "00000000-0000-0000-0000-000000000000",
        "ttl": 64,
    });
    if let Some(r) = reply_to {
        envelope["reply_to"] = json!(r);
    }
    json!({"envelope": envelope, "body": body, "params": {}})
}

// ─────────────────────────────────────────────────────────── shared fixtures

const ALICE: &str = "/os/orgs/example/members/one/assistants/alice";
const BOB: &str = "/os/orgs/example/members/one/assistants/bob";

fn prose_body(view_id: &str, title: &str, text: &str) -> Value {
    json!({
        "messages": [],
        "view_id": view_id,
        "kind": "prose",
        "content": {"title": title, "body": text},
    })
}

/// One row of the `views` table, as an app's write leaves it in the store.
///
/// `at` is `updated_at`, which since GH #609 is the EXPIRY clock and nothing
/// else: it no longer has any bearing on where the row stands on the page.
fn row(owner: &str, view_id: &str, at: i64, ttl: i64, title: &str, text: &str) -> Value {
    json!({
        "owner": owner,
        "view_id": view_id,
        "region": "main",
        "ord": 0,
        "kind": "prose",
        "content": meclaw_core::serde_json::to_string(
            &json!({"body": text, "title": title})).unwrap(),
        "components": "[]",
        "ttl_ms": ttl,
        "updated_at": at,
    })
}

/// A message through the screen's door on `route`, sent by `owner`.
fn door(body: Value, route: &str, owner: &str) -> Value {
    json!({
        "body": body,
        "envelope": {"reply_to": owner, "header": {"hop": {"route": route}, "context": {}}},
    })
}

fn only(mut out: Vec<Value>) -> Value {
    assert_eq!(out.len(), 1, "exactly one emission: {out:#?}");
    out.remove(0)
}

/// The id a window of `owner` stands under, in the state and in the tree (§ 2 Id).
fn oid(owner: &str, view_id: &str) -> String {
    format!("view.{}.{view_id}", owner.replace('/', "~"))
}

/// The store bundle of the last message that carried an app's write: the `views` hop
/// whose mark is `{"write": {owner, view_id, withdraw}}`.
fn write_hop(screen: &Screen) -> Value {
    screen
        .hops()
        .iter()
        .find(|h| h["route"] == "views" && h["request"]["write"].is_object())
        .cloned()
        .unwrap_or_else(|| panic!("the write left as a store bundle: {:#?}", screen.hops()))
}

/// The legs of a store bundle that touch an APP's row. The curator's rest row
/// (`display`/`screen-rest`, OR-D3) rides the same bundle when its content changed,
/// and is not the app's write.
fn app_legs(hop: &Value) -> Vec<Value> {
    hop["calls"]
        .as_array()
        .expect("a hop lists its calls")
        .iter()
        .filter(|c| c["where"]["view_id"] != "screen-rest" && c["row"]["view_id"] != "screen-rest")
        .cloned()
        .collect()
}

fn operations(legs: &[Value]) -> Vec<&str> {
    legs.iter()
        .map(|c| c["operation"].as_str().unwrap_or_default())
        .collect()
}

/// The emission that writes an accepted view to the store, if the cell sent one.
/// A cold cell sends its boot select beside it (§ 2.2); that is not the write.
fn write_of(out: &[Value]) -> Option<&Value> {
    out.iter().find(|em| {
        em["header"]["route"] == "views"
            && em["header"]["display_request"]
                .as_str()
                .is_some_and(|mark| mark.contains("\"write\""))
    })
}

/// The windows the display holds on the unprefixed tree, in the order a page shows
/// siblings: `ord`, then the id (`web`'s own `ORDER BY ord, id`).
fn window_order(screen: &Screen) -> Vec<String> {
    let mut windows: Vec<(i64, String)> = screen
        .held
        .as_array()
        .expect("the display holds a list")
        .iter()
        .filter_map(|o| {
            let id = o["id"].as_str()?;
            (id.starts_with("view.") && !id.contains('/'))
                .then(|| (o["ord"].as_i64().unwrap_or(0), id.to_string()))
        })
        .collect();
    windows.sort();
    windows.into_iter().map(|(_, id)| id).collect()
}

/// One output, named and complete: since § 4.7 `display_type` and `default_screen` are
/// both mandatory, so there is no pass without a profile.
fn params() -> Value {
    json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
           "default_screen": "tv"})
}

/// A component row: a `display-pane` with one line of text under it.
///
/// Not a prose row, although the promise is the same one: `display-view-prose` is the
/// one window class whose `prop_schema` was not carried over to the contract of § 3.1
/// (see the report of this strand), so a real `web` cell refuses its objects today and
/// this test would go red on somebody else's gap.
fn component_row(owner: &str, view_id: &str, title: &str, text: &str) -> Value {
    let tree = json!({
        "component": "display-pane", "key": format!("c.{view_id}"),
        "props": {"pane_id": view_id, "title": title},
        "children": [{"component": "display-text", "key": "body", "props": {"body": text}}],
    });
    json!({
        "owner": owner, "view_id": view_id, "region": "main", "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 2_000,
    })
}

// ────────────────────────────────────────────────────── (a) the owner rule

#[test]
fn a_body_that_claims_a_foreign_owner_is_refused() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }

    let mut body = prose_body("note", "Mine", "…");
    body["owner"] = json!(BOB);
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));

    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "not_owner");
    assert_eq!(
        out["receipt"]["owner"], ALICE,
        "the receipt names the sender, so the level above can route it back"
    );
    assert!(
        out.get("messages").is_some(),
        "every body crossing the substrate carries `messages`"
    );
}

#[test]
fn a_message_without_a_sender_has_no_owner_and_writes_nothing() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            prose_body("note", "Mine", "…"),
            json!({"route": "in_view"}),
            json!({}),
            None,
        ),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "owner_unknown");
}

#[test]
fn an_accepted_view_is_written_under_the_envelopes_path() {
    if display().is_none() || !have_python() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.send(
        door(prose_body("note", "Mine", "hello"), "in_view", ALICE),
        1_000,
    );
    assert!(
        screen.lane("receipt").is_empty(),
        "the door took the view: {:#?}",
        screen.last
    );

    // Delete-then-insert IS the primary key: a store schema cannot declare one. And it
    // is ONE bundle with nothing read in it: the cell holds the rows in memory (GH #809).
    let hop = write_hop(&screen);
    let legs = app_legs(&hop);
    assert_eq!(operations(&legs), vec!["delete", "insert"]);
    assert!(
        !hop["ops"]
            .as_array()
            .expect("ops")
            .contains(&json!("select")),
        "a write reads nothing back: {hop:#?}"
    );

    let inserted = &legs[1]["row"];
    assert_eq!(inserted["owner"], ALICE);
    assert_eq!(inserted["view_id"], "note");
    let deleted = &legs[0]["where"];
    assert_eq!(
        deleted["owner"], ALICE,
        "the delete is scoped to the sender"
    );
    assert_eq!(hop["request"]["write"]["owner"], ALICE);
    assert!(
        screen
            .table()
            .iter()
            .any(|r| r["owner"] == ALICE && r["view_id"] == "note"),
        "the row stands in the store: {:#?}",
        screen.table()
    );

    // The next write of the same view is a pass of a LIVE cell: its one store bundle and
    // its one patch, and no select anywhere -- only a boot asks the store for its rows.
    screen.send(
        door(prose_body("note", "Mine", "again"), "in_view", ALICE),
        2_000,
    );
    let ops: Vec<&Value> = screen
        .hops()
        .iter()
        .filter(|h| h["route"] == "views")
        .flat_map(|h| h["ops"].as_array().expect("ops"))
        .collect();
    assert!(
        !ops.contains(&&json!("select")),
        "no select after the boot: {:#?}",
        screen.hops()
    );
    assert_eq!(
        operations(&app_legs(&write_hop(&screen))),
        vec!["delete", "insert"]
    );
}

// ───────────────────────── (b) two owners, one deterministic order, (f) ttl

/// Both owners reach the screen, and their order carries no clock.
///
/// Until GH #609 this asserted `newest first`, and the older row here is the
/// one that would have to come SECOND under that reading. It comes second
/// under this one too -- but on `(owner, view_id)`, with `updated_at` playing no
/// part: `alice` sorts before `bob`, and swapping the two timestamps does not
/// change the answer. The two rows stand in the store before the cell wakes; its
/// boot reads them once and replays each as the write it was, at its own moment,
/// so the write times are the ONLY thing the two runs differ in.
#[test]
fn two_owners_hold_two_views_in_an_order_that_reads_no_clock() {
    if display().is_none() || !have_python() {
        return;
    }
    let mine = oid(ALICE, "note");
    let theirs = oid(BOB, "board");
    for (alice_at, bob_at) in [(2_000, 1_000), (1_000, 2_000)] {
        let mut screen = Screen::new(params());
        screen.put(row(ALICE, "note", alice_at, 0, "Mine", "the newer one"));
        screen.put(row(BOB, "board", bob_at, 0, "Theirs", "the older one"));
        screen.pass(json!({"kind": "stroke"}), 3_000);
        assert_eq!(
            window_order(&screen),
            vec![mine.clone(), theirs.clone()],
            "both owners are on the screen, and with Alice written at {alice_at} and Bob \
             at {bob_at} identity decides: the last write is not a sort key any more \
             (GH #609)"
        );
    }
}

/// An elapsed view stays in the store -- and the PASS is what takes it off the screen.
///
/// Until the contract of § 4.34 the cell dropped a row whose `ttl_ms` had run out before
/// the pass saw it, so a window could never be drawn one last time on its way out. The
/// filter is gone: the store holds what an application wrote until the application
/// withdraws it, and the pass decides when the view leaves the state (step 4, step 12).
#[test]
fn an_elapsed_view_stays_in_the_store_and_leaves_in_the_pass() {
    if display().is_none() || !have_python() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.put(row(BOB, "flash", 1_000, 1_000, "Gone", "…"));
    screen.write(
        component_row(ALICE, "note", "Here", "the one window"),
        2_000,
    );

    assert!(
        screen
            .table()
            .iter()
            .any(|r| r["owner"] == BOB && r["view_id"] == "flash"),
        "the store still holds what Bob wrote, elapsed or not: {:#?}",
        screen.table()
    );
    let mine = format!("{}/c.note", oid(ALICE, "note"));
    let theirs = oid(BOB, "flash");
    assert!(
        screen.holds(&mine),
        "the written view is drawn: {}",
        screen.held
    );
    assert!(
        !screen.holds(&theirs),
        "and the elapsed one is on no screen: {}",
        screen.held
    );
    assert!(
        screen.screen_state()["views"].get(&theirs).is_none(),
        "the pass took it out of the state"
    );
}

/// A store that refuses a leg of an app's write answers the cell with the write's own
/// mark on the reply; the cell turns that into a receipt to the app, and draws nothing
/// from it. One reply document, no memory needed: the door's own bytes suffice.
#[test]
fn a_failed_write_leg_is_a_receipt_and_not_a_picture() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let mark = json!({"write": {"owner": ALICE, "view_id": "note", "withdraw": false}});
    let reply = json!({
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "d-delete",
             "text": "{\"rows_affected\":1}"},
            {"origin": "tool", "type": "tool_result", "id": "d-insert",
             "text": "constraint violated"},
        ],
        "results": [
            {"tool_call_id": "d-delete", "operation": "delete", "rows_affected": 1},
            {"tool_call_id": "d-insert", "operation": "insert", "rows_affected": 0,
             "error_code": "constraint_violation"},
        ],
    });
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            reply,
            json!({"operation": "bundle", "rows_affected": 1, "bundle_errors": 1}),
            json!({"display_origin": "views", "display_request": mark.to_string()}),
            None,
        ),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "store_failed");
    assert_eq!(
        out["receipt"]["owner"], ALICE,
        "the receipt goes to the writer"
    );
    assert_eq!(out["receipt"]["view_id"], "note");
}

// ──────────────────────────────────────────────────────── (c) the withdrawal

#[test]
fn a_withdrawal_removes_only_the_callers_own_view() {
    if display().is_none() || !have_python() {
        return;
    }
    // Bob holds a view with the SAME view_id, which is exactly the collision an
    // owner-blind delete would have taken with it.
    let mut screen = Screen::new(params());
    screen.write(component_row(BOB, "note", "Theirs", "still there"), 1_000);
    screen.write(component_row(ALICE, "note", "Mine", "going away"), 2_000);
    screen.take_down(ALICE, "note", 3_000);

    let hop = write_hop(&screen);
    assert_eq!(hop["request"]["write"]["withdraw"], json!(true));
    let legs = app_legs(&hop);
    assert_eq!(
        operations(&legs),
        vec!["delete"],
        "a withdrawal inserts nothing"
    );
    assert_eq!(legs[0]["where"]["owner"], ALICE);
    assert_eq!(legs[0]["where"]["view_id"], "note");

    // And the screen that follows keeps everybody else's row.
    let left: Vec<&Value> = screen
        .rows
        .iter()
        .filter(|r| r["view_id"] == "note")
        .collect();
    assert_eq!(left.len(), 1, "only the caller's own row left: {left:#?}");
    assert_eq!(left[0]["owner"], BOB);
    let state = screen.screen_state();
    assert_eq!(state["views"][oid(ALICE, "note")]["withdrawn"], json!(true));
    assert_eq!(state["views"][oid(BOB, "note")]["withdrawn"], json!(false));
    assert!(
        screen.holds(&oid(BOB, "note")),
        "Bob's window stands: {}",
        screen.held
    );
}

// ──────────────────────────────────── the round terminates, which is the point

#[test]
fn the_acknowledgement_of_a_patch_ends_the_round() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let out = run_shipped(
        &root,
        "compose",
        stdin_doc(
            json!({"messages": [{"origin": "tool", "type": "tool_result",
                                 "id": "d-0", "text": "{}"}]}),
            json!({"operation": "bundle", "bundle_errors": 0}),
            json!({"display_origin": "patch"}),
            None,
        ),
    );
    assert!(
        out.is_empty(),
        "a cell that cannot recognise the reply to its own write has no way to \
         stop (GH #161): {out:#?}"
    );
}

// ─────────────────────────────────────────── (d) the application's own view

/// The app's `layout`, driven over a snapshot, and its output taken through the
/// screen's own door. Two templates, one wire contract, one test — because the
/// contract is the thing that can rot, and neither half can see it alone.
#[test]
fn the_app_view_declares_components_and_every_name_carries_its_prefix() {
    let Some(app) = colony_view() else { return };
    let Some(screen) = display() else { return };
    if !have_python() {
        return;
    }

    let snapshot = json!({
        "messages": [],
        "graph": {
            "scope": "/",
            "nodes": [
                {"path": "/one", "cell_type": "code"},
                {"path": "/two", "cell_type": "store"},
            ],
            "edges": [{"from": "/one", "to": "/two"}],
        },
    });
    let view = only(run_shipped(
        &app,
        "layout",
        stdin_doc(snapshot, json!({"route": "snapshot"}), json!({}), None),
    ));

    assert_eq!(view["header"]["route"], "view");
    assert_eq!(view["view_id"], "colony-view");
    assert_eq!(view["kind"], "component");

    let declared = view["components"]
        .as_array()
        .expect("the app brings its own");
    assert!(!declared.is_empty());
    for c in declared {
        let name = c["name"].as_str().expect("a component has a name");
        assert!(
            name.starts_with("colony-view-"),
            "a component name is prefixed with the view's id, so two apps on \
             one screen cannot collide: {name}"
        );
    }

    // And the screen accepts it: same body, through the door, with an owner. What
    // leaves is the store bundle of the write, and no receipt.
    let mut body = view.clone();
    body.as_object_mut().unwrap().remove("header");
    let out = run_shipped(
        &screen,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    );
    assert!(
        write_of(&out).is_some() && !out.iter().any(|em| em["header"]["route"] == "receipt"),
        "the screen took the app's view: {out:#?}"
    );
}

#[test]
fn a_component_without_the_prefix_is_refused() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let body = json!({
        "messages": [],
        "view_id": "colony-view",
        "kind": "component",
        "content": {"component": "colony-view-shell", "props": {}},
        "components": [{"name": "sneaky-shell", "template": "<i>{{x}}</i>",
                        "prop_schema": {"x": "text"}, "editable": [],
                        "layer": "content"}],
    });
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "component_prefix");
}

/// GH #568: two siblings naming the same `key` mint the same object id. The
/// tree is refused as a whole rather than losing the first node silently.
#[test]
fn two_siblings_naming_the_same_key_are_refused() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let body = json!({
        "messages": [],
        "view_id": "keyed",
        "kind": "component",
        "content": {
            "component": "display-card",
            "props": {},
            "children": [
                {"component": "display-text", "key": "dup", "props": {}},
                {"component": "display-text", "key": "dup", "props": {}},
            ],
        },
    });
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "invalid_view");
    let detail = out["receipt"]["detail"]
        .as_str()
        .expect("a refusal says why");
    assert!(
        detail.contains("dup"),
        "the refusal names the key that collided: {detail}"
    );
}

/// GH #568: a numeric key lands in the index's own language -- a node keyed
/// `"3"` names exactly the object the unkeyed fourth child beside it names.
#[test]
fn a_numeric_key_is_refused() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let body = json!({
        "messages": [],
        "view_id": "keyed",
        "kind": "component",
        "content": {
            "component": "display-card",
            "props": {},
            "children": [{"component": "display-text", "key": "3", "props": {}}],
        },
    });
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "invalid_view");
    let detail = out["receipt"]["detail"]
        .as_str()
        .expect("a refusal says why");
    assert!(
        detail.contains("number"),
        "the refusal says a key may not be a number: {detail}"
    );
}

/// The collision is per parent, not per tree: the same key under two different
/// parents mints two different ids and is taken.
#[test]
fn distinct_keys_across_different_parents_are_fine() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let body = json!({
        "messages": [],
        "view_id": "keyed",
        "kind": "component",
        "content": {
            "component": "display-card",
            "props": {},
            "children": [
                {"component": "display-card", "key": "left", "props": {},
                 "children": [{"component": "display-text", "key": "row", "props": {}}]},
                {"component": "display-card", "key": "right", "props": {},
                 "children": [{"component": "display-text", "key": "row", "props": {}}]},
            ],
        },
    });
    let out = run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    );
    assert!(
        write_of(&out).is_some() && !out.iter().any(|em| em["header"]["route"] == "receipt"),
        "the same key under two parents is two ids: {out:#?}"
    );
}

// ──────────────────────────────────────────────── (e) it reaches a real page

struct Live {
    /// The port of the one listener in front of the cell.
    port: u16,
    _listener: tokio::task::JoinHandle<()>,
    cell_dir: std::path::PathBuf,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

/// A `web` cell over `cell_dir`. Handed a `seed/`, it seeds itself from it --
/// which is what a real `display/web` gets, because the ref brings the `web`
/// template's own demo seed with it. Believing otherwise is how GH #402
/// shipped.
async fn start(cell_dir: &std::path::Path) -> Live {
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/display/web"),
            json!({ "mount": MOUNT }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("Active");
    };

    // An empty display has no page and answers 404 -- which is still the cell
    // answering. Waiting for a 200 would wait for a bootstrap this test has
    // not sent yet.
    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let port = addr.port();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/"))
            .await
            .is_ok()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the cell never answered on its mount"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Live {
        port,
        _listener: listener,
        cell_dir: cell_dir.to_path_buf(),
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

async fn apply(live: &mut Live, calls: &[Value]) -> Value {
    let turns: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, args)| {
            json!({"origin": "assistant", "type": "tool_call",
                   "text": args.to_string(), "id": format!("c{i}")})
        })
        .collect();
    let msg = MessageBuilder::new(Path::new("/display/web"))
        .body(Body::Inline(json!({ "messages": turns })))
        .reply_to(Path::new("/display/compose"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");
    tokio::time::timeout(Duration::from_secs(60), live.out_rx.recv())
        .await
        .expect("the display must answer a bundle")
        .expect("an emission")
        .content
}

/// The whole point of the re-cut, end to end: two owners' views come out of ONE living
/// compose cell, a real `web` cell takes the patches it sent, and the page a browser
/// would get carries both of them.
///
/// Two owners are two WRITES here, and that is the contract and not the harness: a view
/// enters the screen state through the `app_write` event of its own pass (§ 4.8 a), so a
/// screen with two applications on it has seen two writes -- and each of them sent the
/// display exactly one patch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_views_reach_a_real_display() {
    if display().is_none() || !have_python() {
        return;
    }

    let mut screen = Screen::new(params());
    let mut patches = Vec::new();
    for (row, now) in [
        (
            component_row(BOB, "board", "From Bob", "the older paragraph"),
            1_000,
        ),
        (
            component_row(ALICE, "note", "From Alice", "the newer paragraph"),
            2_000,
        ),
    ] {
        let calls = screen.write(row, now);
        let sent = screen
            .hops()
            .iter()
            .filter(|h| h["route"] == "patch")
            .count();
        assert_eq!(sent, 1, "one patch per pass: {:#?}", screen.hops());
        patches.push(calls);
    }

    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;

    for calls in &patches {
        let reply = apply(&mut live, calls).await;
        assert_eq!(
            reply["header"]["bundle_errors"],
            json!(0),
            "the bundle the screen builds is one the display accepts: {reply:#?}"
        );
    }

    // One wrapper object per view, straight out of the display's own database. Counted
    // on the unprefixed tree alone -- every named exit is a copy of it (§ 6), so the
    // whole table holds one wrapper per view per exit.
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let wrappers: i64 = conn
        .query_row(
            "SELECT count(*) FROM objects WHERE component LIKE 'display-view-%' \
             AND id LIKE 'view.%'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(
        wrappers, 2,
        "one wrapper per view, and two owners wrote one each"
    );
    drop(conn);

    // ...and the page a browser gets.
    let body = reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");

    for needle in [
        "the newer paragraph",
        "the older paragraph",
        "From Alice",
        "From Bob",
    ] {
        assert!(
            body.contains(needle),
            "one screen carries both owners' views -- missing {needle:?} in:\n{body}"
        );
    }
    assert!(
        body.find("the newer paragraph") < body.find("the older paragraph"),
        "the order holds on the page and not only in the tree. Neither window is open \
         here -- both are `fresh` in their own pass and neither reaches the bar -- so \
         they stand on the same seat and the display orders them by id: `alice` sorts \
         before `bob` (§ 6.3, GH #609):\n{body}"
    );
    // Each wrapper says whose it is, which is what a member reads off a browser
    // event to route it back to the one agent that put the view up.
    for owner in [ALICE, BOB] {
        assert!(
            body.contains(&format!("data-owner=\"{owner}\"")),
            "the markup names the owner: {owner}"
        );
    }

    live.join.abort();
}
