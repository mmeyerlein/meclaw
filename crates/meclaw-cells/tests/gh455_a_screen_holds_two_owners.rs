//! GH #455 -- what the screen promises, driven through the SHIPPED bytes.
//!
//! Every script here is read out of `params.script_inline` and run through the
//! runner the `code` cell declares, which is the same command the substrate
//! builds. A test that ran the `.py` beside it would prove the source and not
//! the product; `gh455_the_two_templates_ship` is what pins that the two agree.
//!
//! The six promises, in the order they are made:
//!
//! (a) the owner of a view is the ENVELOPE, and a body that claims a different
//!     one is refused rather than believed;
//! (b) two owners hold two views on one screen at the same time, newest first;
//! (c) a withdrawal removes the caller's own view and no other;
//! (d) an application's view carries its components, every name prefixed with
//!     the view's own id -- driven with the bytes `colony-view` really emits;
//! (e) both views reach a REAL display, over HTTP, on a page a browser gets;
//! (f) a view whose `ttl_ms` has elapsed is not laid out.
//!
//! Free of a provider by construction: neither template holds a model.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::free_port;
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

/// One row of the `views` table, as the store hands it back.
fn row(owner: &str, view_id: &str, at: i64, ttl: i64, title: &str, text: &str) -> Value {
    json!({
        "owner": owner,
        "view_id": view_id,
        "region": "main",
        "kind": "prose",
        "content": meclaw_core::serde_json::to_string(
            &json!({"body": text, "title": title})).unwrap(),
        "components": "[]",
        "ttl_ms": ttl,
        "updated_at": at,
    })
}

/// A store bundle reply: leg 0's rows in the first turn, `results[]` beside it.
fn store_reply(rows: Vec<Value>, legs: &[&str]) -> Value {
    let mut turns = vec![json!({
        "origin": "tool", "type": "tool_result", "id": "d-select",
        "text": meclaw_core::serde_json::to_string(&rows).unwrap(),
    })];
    let mut results = vec![json!({"tool_call_id": "d-select", "operation": "select"})];
    for leg in legs {
        turns.push(json!({
            "origin": "tool", "type": "tool_result", "id": format!("d-{leg}"),
            "text": "{\"rows_affected\":1}",
        }));
        results.push(json!({"tool_call_id": format!("d-{leg}"), "operation": leg}));
    }
    json!({"messages": turns, "results": results})
}

/// The context the hive's own edge stamps on the store's reply.
fn views_context(request: &Value) -> Value {
    json!({
        "display_origin": "views",
        "display_request": meclaw_core::serde_json::to_string(request).unwrap(),
    })
}

fn only(mut out: Vec<Value>) -> Value {
    assert_eq!(out.len(), 1, "exactly one emission: {out:#?}");
    out.remove(0)
}

/// Drive pass 1 and read the request it put on the hop.
fn request_of(emission: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        emission["header"]["display_request"]
            .as_str()
            .expect("pass 1 carries the request on the hop"),
    )
    .expect("the request is JSON")
}

/// Drive pass 2 and read the plan it put on the hop.
fn plan_of(emission: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        emission["header"]["display_views"]
            .as_str()
            .expect("pass 2 carries the plan on the hop"),
    )
    .expect("the plan is JSON")
}

/// The `op` argument of every leg of a patch bundle, in call order.
fn calls_of(emission: &Value) -> Vec<Value> {
    emission["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|t| {
            meclaw_core::serde_json::from_str(t["text"].as_str().expect("text")).expect("args")
        })
        .collect()
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
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            prose_body("note", "Mine", "hello"),
            json!({"route": "in_view"}),
            json!({}),
            Some(ALICE),
        ),
    ));
    assert_eq!(out["header"]["route"], "views");

    // Delete-then-insert IS the primary key: a store schema cannot declare one.
    let ops: Vec<String> = calls_of(&out)
        .iter()
        .map(|c| c["operation"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ops, vec!["select", "delete", "insert"]);

    let inserted = &calls_of(&out)[2]["row"];
    assert_eq!(inserted["owner"], ALICE);
    assert_eq!(inserted["view_id"], "note");
    let deleted = &calls_of(&out)[1]["where"];
    assert_eq!(
        deleted["owner"], ALICE,
        "the delete is scoped to the sender"
    );
    assert_eq!(request_of(&out)["owner"], ALICE);
}

// ───────────────────────────────────── (b) two owners, newest first, (f) ttl

#[test]
fn two_owners_hold_two_views_and_the_newest_is_first() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }

    let mine = row(ALICE, "note", 2_000, 0, "Mine", "the newer one");
    let theirs = row(BOB, "board", 1_000, 0, "Theirs", "the older one");
    let request = json!({"withdraw": false, "owner": ALICE, "view_id": "note",
                         "row": mine});

    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            // The before-state carries only the OTHER owner's row: this write
            // is the first one Alice ever made.
            store_reply(vec![theirs.clone()], &["delete", "insert"]),
            json!({"operation": "bundle", "bundle_errors": 0}),
            views_context(&request),
            None,
        ),
    ));

    assert_eq!(out["header"]["route"], "read");
    let plan = plan_of(&out);
    let views = plan["views"].as_array().expect("views");
    assert_eq!(views.len(), 2, "both owners are on the screen: {views:#?}");
    assert_eq!(views[0]["owner"], ALICE, "newest first");
    assert_eq!(views[1]["owner"], BOB);
}

#[test]
fn an_elapsed_view_is_not_laid_out() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }

    // Written at the epoch with a one-second life: elapsed by any clock this
    // test could run under, which is what makes the assertion deterministic
    // rather than a race with `time.time()`.
    let stale = row(BOB, "flash", 1_000, 1_000, "Gone", "…");
    let fresh = row(ALICE, "note", 2_000, 0, "Here", "…");
    let request = json!({"withdraw": false, "owner": ALICE, "view_id": "note",
                         "row": fresh});

    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            store_reply(vec![stale], &["delete", "insert"]),
            json!({"operation": "bundle", "bundle_errors": 0}),
            views_context(&request),
            None,
        ),
    ));
    let plan = plan_of(&out);
    let views = plan["views"].as_array().expect("views");
    assert_eq!(views.len(), 1, "the elapsed one is not drawn: {views:#?}");
    assert_eq!(views[0]["owner"], ALICE);
}

#[test]
fn a_failed_write_leg_is_a_receipt_and_not_a_picture() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }
    let mut reply = store_reply(vec![], &["delete", "insert"]);
    reply["results"][2]["error_code"] = json!("constraint_violation");
    let request = json!({"withdraw": false, "owner": ALICE, "view_id": "note",
                         "row": row(ALICE, "note", 2_000, 0, "x", "y")});

    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            reply,
            json!({"operation": "bundle", "bundle_errors": 1}),
            views_context(&request),
            None,
        ),
    ));
    assert_eq!(out["header"]["route"], "receipt");
    assert_eq!(out["receipt"]["error_code"], "store_failed");
}

// ──────────────────────────────────────────────────────── (c) the withdrawal

#[test]
fn a_withdrawal_removes_only_the_callers_own_view() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }

    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            json!({"messages": [], "view_id": "note"}),
            json!({"route": "in_withdraw"}),
            json!({}),
            Some(ALICE),
        ),
    ));
    let calls = calls_of(&out);
    let ops: Vec<&str> = calls
        .iter()
        .map(|c| c["operation"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ops,
        vec!["select", "delete"],
        "a withdrawal inserts nothing"
    );
    assert_eq!(calls[1]["where"]["owner"], ALICE);
    assert_eq!(calls[1]["where"]["view_id"], "note");

    // And the picture that follows keeps everybody else's row. Bob holds a
    // view with the SAME view_id, which is exactly the collision an owner-blind
    // delete would have taken with it.
    let theirs = row(BOB, "note", 1_000, 0, "Theirs", "still there");
    let mine = row(ALICE, "note", 2_000, 0, "Mine", "going away");
    let after = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            store_reply(vec![theirs, mine], &["delete"]),
            json!({"operation": "bundle", "bundle_errors": 0}),
            views_context(&request_of(&out)),
            None,
        ),
    ));
    let plan = plan_of(&after);
    let views = plan["views"].as_array().expect("views");
    assert_eq!(views.len(), 1, "only the caller's own row left: {views:#?}");
    assert_eq!(views[0]["owner"], BOB);
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

    // And the screen accepts it: same body, through the door, with an owner.
    let mut body = view.clone();
    body.as_object_mut().unwrap().remove("header");
    let out = only(run_shipped(
        &screen,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));
    assert_eq!(
        out["header"]["route"], "views",
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
    let out = only(run_shipped(
        &root,
        "compose",
        stdin_doc(body, json!({"route": "in_view"}), json!({}), Some(ALICE)),
    ));
    assert_eq!(
        out["header"]["route"], "views",
        "the same key under two parents is two ids: {out:#?}"
    );
}

// ──────────────────────────────────────────────── (e) it reaches a real page

struct Live {
    port: u16,
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
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/display/web"),
            json!({ "port": port }),
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

    // An empty display has no page and answers 404 -- which is still the
    // listener answering. Waiting for a 200 would wait for a bootstrap this
    // test has not sent yet.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if reqwest::get(format!("http://127.0.0.1:{port}/"))
            .await
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never bound its port");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Live {
        port,
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

/// The whole point of the re-cut, end to end: two owners' views come out of the
/// shipped compose cell, a real `web` cell takes the bundle, and the page a
/// browser would get carries both of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_views_reach_a_real_display() {
    let Some(root) = display() else { return };
    if !have_python() {
        return;
    }

    let mine = row(ALICE, "note", 2_000, 0, "From Alice", "the newer paragraph");
    let theirs = row(BOB, "board", 1_000, 0, "From Bob", "the older paragraph");
    let request = json!({"withdraw": false, "owner": ALICE, "view_id": "note",
                         "row": mine});

    // Pass 2 -> the plan; pass 3 against an empty display -> the bootstrap.
    let read = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            store_reply(vec![theirs], &["delete", "insert"]),
            json!({"operation": "bundle", "bundle_errors": 0}),
            views_context(&request),
            None,
        ),
    ));
    let plan = read["header"]["display_views"]
        .as_str()
        .unwrap()
        .to_string();
    let patch = only(run_shipped(
        &root,
        "compose",
        stdin_doc(
            // A display whose `/` was never set refuses the query. That is one
            // of the two bootstrap cases, and the cheaper one to stage.
            json!({"messages": [{"origin": "tool", "type": "tool_result",
                                 "id": "d-query", "text": "{}"}]}),
            json!({"operation": "query", "error_code": "invalid_input"}),
            json!({"display_origin": "read", "display_views": plan}),
            None,
        ),
    ));
    let calls = calls_of(&patch);

    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;

    let reply = apply(&mut live, &calls).await;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(0),
        "the bundle the screen builds is one the display accepts: {reply:#?}"
    );

    // One wrapper object per view, straight out of the display's own database.
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let wrappers: i64 = conn
        .query_row(
            "SELECT count(*) FROM objects WHERE component LIKE 'display-view-%'",
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
    let body = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
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
        "newest first, on the page and not only in the plan:\n{body}"
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
