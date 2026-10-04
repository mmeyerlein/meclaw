//! GH #983 / OR-NL-159 (2) -- a resident the member does not have gets no road.
//!
//! `install_app` drew one edge pair per resident the declaration reads, also to
//! residents the member does not have. The presenter reads nine; a member without
//! `apps/colony-view` (or without any other of them) then had its whole installation
//! refused at the door: `edge_schema`, `to='./apps/colony-view' unknown in scope`
//! (`crates/meclaw-colony/src/mutation/validate.rs`, the `add_edges` endpoint check).
//!
//! The recipe reads no tree, so the wish tells it which residents the member has:
//! `residents_present`. A resident the declaration reads and the list does not name is
//! left out -- no edge, no refusal. Without the parameter every resident the presenter
//! read before GH #976 is drawn, as before (the GH #965 locks render that way); the two
//! member APPS GH #976 added (`./apps/daily-digest`, `./apps/research-assistant`) are
//! drawn only when `residents_present` names them (Y fix round 1, I-1): almost no member
//! has them, and a caller that does not know the parameter -- the builder's composer, an
//! Egon rebuild -- would otherwise have every presenter installation refused at the door.
//!
//! # What is measured
//!
//! 1. Pure: the SHIPPED recipe over stdin, the presenter's own block. Without
//!    `colony-view` in `residents_present`: no edge touches `./apps/colony-view`, no
//!    `error_code`, every other resident's road byte-identical. With every resident
//!    named: exactly the rendering without the parameter.
//!    Without the parameter: no edge touches the two member apps, every other resident
//!    is drawn; naming all nine adds exactly the two apps' roads.
//! 2. Colony: a member with `file-space` and WITHOUT `apps/colony-view` takes the
//!    presenter's rendered resident road -- every edge of it between the presenter and
//!    any of the nine resident cells, unfiltered by the test -- as one committed
//!    mutation. A sure `colony` verdict then places no block: the question to the colony
//!    view matches no edge, nothing answers, and the window is withdrawn (sentinel: the
//!    `in_withdraw` at the screen; no block arrived before it on the same ordered lane).
//!    Positive control on the same member: `files` is answered and drawn.
//! 3. Colony (I-1): a member with the seven residents the presenter read before GH #976
//!    (doubles) and WITHOUT both member apps takes the road rendered WITHOUT
//!    `residents_present` -- unfiltered, every edge between the presenter and a resident
//!    cell -- as one committed mutation, and the presenter answers `files` on it.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use display_colony::{SCREEN, body_of, copy_tree, hop_of, read_json, repo, write_json};
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::mock_http::MockResponse;
use presenter_colony::{Dials, PRESENTER, Stage, boot, decision, guard, warm};
use tokio::sync::oneshot;

const ROUND: &str = r#"["agent:sam","member:alex"]"#;
const GENERATION: &str = "sam";
const PERSON: &str = "alex";
/// Where the recipe puts the colony view (`RESIDENTS`).
const VIEW_CELL: &str = "./apps/colony-view";
/// Every resident cell the presenter reads (`RESIDENTS` of the recipe).
const RESIDENT_CELLS: [&str; 9] = [
    "./memory-hive",
    "./file-space",
    "./graph-space",
    "./objects",
    "./librarian",
    "./affinity",
    "./apps/colony-view",
    "./apps/daily-digest",
    "./apps/research-assistant",
];
/// The two member apps GH #976 made residents: drawn only when named (I-1).
const MEMBER_APPS: [&str; 2] = ["./apps/daily-digest", "./apps/research-assistant"];
/// The residents the presenter read before GH #976, as node names under the member.
const OLD_RESIDENT_NODES: [&str; 7] = [
    "memory-hive",
    "file-space",
    "graph-space",
    "objects",
    "librarian",
    "affinity",
    "apps/colony-view",
];
const ALL_RESIDENTS: [&str; 9] = [
    "memory-hive",
    "file-space",
    "graph-space",
    "objects",
    "librarian",
    "affinity",
    "colony-view",
    "daily-digest",
    "research-assistant",
];

fn round() -> Value {
    meclaw_core::serde_json::from_str(ROUND).expect("the round parses")
}

fn shipped() -> bool {
    [
        "templates/builder/recipes/config.json",
        "templates/presenter/template.json",
        "templates/presenter/stage/config.json",
        "templates/web/template.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

// ═══════════════════════════════════════════════════════════════ the recipe

/// The recipe's whole answer for the presenter's `reads_residents`, with
/// `residents_present` when given.
fn render(present: Option<Value>) -> Value {
    let block = read_json(&repo("templates/presenter/template.json"))["app"].clone();
    let declaration = json!({"reads_residents": block["reads_residents"]});
    let mut params = json!({"scope": "/alex", "app": "presenter",
                            "template": "presenter@1.2.1", "screen": "display-main",
                            "generation": GENERATION, "declaration": declaration,
                            "ctx": {"member_person": PERSON}});
    if let Some(p) = present {
        params["residents_present"] = p;
    }
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    out.first().cloned().expect("the recipe emitted nothing")
}

fn edges_of(answer: &Value) -> Vec<Value> {
    assert!(
        answer["header"]["error_code"].is_null(),
        "the recipe refused the installation: {answer}"
    );
    answer["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn touches(e: &Value, cell: &str) -> bool {
    e["from"].as_str() == Some(cell) || e["to"].as_str() == Some(cell)
}

#[test]
fn a_member_without_the_colony_view_gets_no_road_to_it_and_no_refusal() {
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let before = edges_of(&render(Some(json!(ALL_RESIDENTS))));
    assert!(
        before.iter().any(|e| touches(e, VIEW_CELL)),
        "with every resident named the colony view's road is drawn"
    );
    let without: Vec<&str> = ALL_RESIDENTS
        .iter()
        .copied()
        .filter(|r| *r != "colony-view")
        .collect();
    let answer = render(Some(json!(without)));
    let after = edges_of(&answer);
    assert!(
        !after.iter().any(|e| touches(e, VIEW_CELL)),
        "no edge touches a resident the member does not have: {after:#?}"
    );
    let kept: Vec<Value> = before
        .into_iter()
        .filter(|e| !touches(e, VIEW_CELL))
        .collect();
    assert_eq!(
        after, kept,
        "every other edge is rendered exactly as before, in order"
    );
}

#[test]
fn every_resident_present_draws_the_old_road_and_the_two_member_apps() {
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let before = render(None);
    let all = render(Some(json!(ALL_RESIDENTS)));
    let without_apps: Vec<Value> = edges_of(&all)
        .into_iter()
        .filter(|e| !MEMBER_APPS.iter().any(|app| touches(e, app)))
        .collect();
    assert_eq!(
        without_apps,
        edges_of(&before),
        "naming every resident adds the two member apps' roads and changes nothing else"
    );
    for cell in RESIDENT_CELLS {
        assert!(
            edges_of(&all).iter().any(|e| touches(e, cell)),
            "a road to every resident the presenter reads: {cell}"
        );
    }
}

#[test]
fn without_the_parameter_the_member_apps_are_left_out_and_nothing_is_refused() {
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let edges = edges_of(&render(None));
    for app in MEMBER_APPS {
        assert!(
            !edges.iter().any(|e| touches(e, app)),
            "without `residents_present` no edge touches {app}: {edges:#?}"
        );
    }
    for cell in RESIDENT_CELLS.iter().filter(|c| !MEMBER_APPS.contains(c)) {
        assert!(
            edges.iter().any(|e| touches(e, cell)),
            "the residents the presenter read before GH #976 are drawn as before: {cell}"
        );
    }
}

#[test]
fn residents_present_that_is_no_list_is_refused_by_name() {
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let answer = render(Some(json!("colony-view")));
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
    let text = answer["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("residents_present"), "{text}");
}

// ═══════════════════════════════════════════════════════════════ the colony

/// `file-space` on `in_read`: one listing (`op: list`) or the space's info
/// (`op: dir_info`), `op` and `op_id` mirrored like the real cell.
const FILE_SPACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
if str(hop.get("route") or "") == "in_read":
    op = str(hop.get("op") or "")
    sys.stdout.write(json.dumps({
        "header": {"route": "answer", "op": op, "op_id": str(hop.get("op_id") or "")},
        "messages": [], "ok": True, "op": op,
        "entries": [{"path": "/notes/plan.txt", "bytes": 120}],
        "path": "/", "files": 1, "summary": "one file"}))
else:
    sys.stdout.write(json.dumps([]))
"#;

fn file_space_double() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": FILE_SPACE,
                   "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {
                    "messages": {"type": "array", "required": false},
                    "ok": {"type": "boolean", "required": false},
                    "op": {"type": "string", "required": false},
                    "entries": {"type": "array", "required": false},
                    "path": {"type": "string", "required": false},
                    "files": {"type": "number", "required": false},
                    "summary": {"type": "string", "required": false}
                },
                "hop": {
                    "route": {"type": "string", "required": false},
                    "op": {"type": "string", "required": false},
                    "op_id": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": "Double of the member's file space.",
                        "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
    })
}

/// A sure verdict for `topic` with `lead`, every other asked key answered with its quiet
/// default (the translate wants every key).
fn verdict(topic: &str, lead: &str) -> MockResponse {
    let cfg = read_json(&repo("templates/presenter/stage/config.json"));
    let mut owned: Vec<(String, String, f64)> =
        vec![("topic".to_string(), topic.to_string(), 0.95)];
    for t in cfg["params"]["builtin_topics"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = t["topic"].as_str().unwrap_or_default();
        let pick = if name == topic {
            lead
        } else {
            t["standard"].as_str().unwrap_or_default()
        };
        owned.push((format!("{name}.lead"), pick.to_string(), 0.95));
        owned.push((format!("{name}.also"), "none".to_string(), 0.95));
    }
    let refs: Vec<(&str, &str, f64)> = owned
        .iter()
        .map(|(k, c, p)| (k.as_str(), c.as_str(), *p))
        .collect();
    decision(&refs)
}

/// The member has `file-space` and no colony view. The presenter's rendered resident
/// road -- EVERY edge between the presenter's rim and any of the nine resident cells,
/// as rendered with `residents_present` -- is grown with the double in ONE mutation;
/// the test filters out no missing resident, the recipe has to.
async fn install_without_the_view(s: &Stage) -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("a temporary directory");
    let root = td.path();
    copy_tree(&repo("templates/web"), &root.join("web"));
    write_json(
        &root.join("file-space").join("template.json"),
        &json!({"name": "file-space"}),
    );
    write_json(
        &root.join("file-space").join("config.json"),
        &file_space_double(),
    );
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.to_path_buf(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");

    // What a wish author holding the tree names: the member's residents (and its
    // other apps -- a name the recipe has no resident for is ignored).
    let answer = render(Some(json!(["file-space", "presenter"])));
    let road: Vec<Value> = edges_of(&answer)
        .into_iter()
        .filter(|e| {
            RESIDENT_CELLS.iter().any(|cell| {
                (e["from"].as_str() == Some("./apps/presenter") && e["to"].as_str() == Some(cell))
                    || (e["to"].as_str() == Some("./apps/presenter")
                        && e["from"].as_str() == Some(cell))
            })
        })
        .collect();
    assert_eq!(
        road.len(),
        2,
        "out and back to file-space, nothing to the absent residents: {road:#?}"
    );
    let diff = json!({
        "add_nodes": [{"name": "file-space", "template": "file-space"}],
        "add_edges": road,
    });
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/alex", "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    let outcome = ack_rx.await.expect("the mutation answers");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a member without the colony view takes the presenter's road: {outcome:?}"
    );
    td
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_the_colony_view_boots_with_the_presenter() {
    if !guard("a_member_without_the_colony_view_boots_with_the_presenter") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        data_wait_ms: 5_000,
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_without_the_view(&s).await;
    warm(&s).await;

    // The colony topic: its source is the colony view, which this member does not have.
    s.turn_in("t1", "how is the colony doing", round()).await;
    s.ask().await.release(verdict("colony", "counts"));
    s.c.wait_until("the colony window is withdrawn at the screen", || async {
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .any(|r| hop_of(r)["route"] == "in_withdraw" && body_of(r)["view_id"] == "show-colony")
    })
    .await;
    let stage = format!("{PRESENTER}/stage");
    assert!(
        !s.c.log(Some(&stage)).await.iter().any(|r| {
            let h = hop_of(r);
            h["route"] == "resident_answer" && h["resident"] == "colony-view"
        }),
        "nothing answered for a resident that does not stand"
    );
    let at_screen: Vec<String> =
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .filter(|r| hop_of(r)["route"] == "in_view")
            .map(|r| body_of(r).to_string())
            .collect();
    for block in ["show-colony-counts", "show-colony-kinds"] {
        assert!(
            !at_screen.iter().any(|b| b.contains(block)),
            "a block of the absent resident reached the screen: {block}"
        );
        assert!(!s.drawn(block).await, "{block} stands at web");
    }
    let row = s.journal_of("t1").await;
    assert_eq!(row["topic"], json!("colony"), "{row}");
    assert_eq!(row["fallback"], json!("no_data"), "{row}");

    // The positive control: the resident the member has answers and is drawn.
    s.turn_in("t2", "show me my files", round()).await;
    s.ask().await.release(verdict("files", "tree"));
    s.wait_drawn("show-files-tree").await;
    s.c.shutdown().await;
}

/// I-1: the member has the seven residents the presenter read before GH #976 (each a
/// double that answers like `file-space`) and NEITHER member app. The road the recipe
/// renders WITHOUT `residents_present` -- every edge between the presenter's rim and any
/// resident cell, unfiltered by the test -- is grown in ONE mutation.
async fn install_the_old_residents(s: &Stage) -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("a temporary directory");
    let root = td.path();
    copy_tree(&repo("templates/web"), &root.join("web"));
    let mut nodes = Vec::new();
    for node in OLD_RESIDENT_NODES {
        let template = node.rsplit('/').next().expect("a node name");
        write_json(
            &root.join(template).join("template.json"),
            &json!({"name": template}),
        );
        write_json(
            &root.join(template).join("config.json"),
            &file_space_double(),
        );
        nodes.push(json!({"name": node, "template": template}));
    }
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.to_path_buf(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");

    let road: Vec<Value> = edges_of(&render(None))
        .into_iter()
        .filter(|e| {
            RESIDENT_CELLS.iter().any(|cell| {
                (e["from"].as_str() == Some("./apps/presenter") && e["to"].as_str() == Some(cell))
                    || (e["to"].as_str() == Some("./apps/presenter")
                        && e["from"].as_str() == Some(cell))
            })
        })
        .collect();
    assert!(!road.is_empty(), "the presenter reads residents");
    let diff = json!({"add_nodes": nodes, "add_edges": road});
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/alex", "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    let outcome = ack_rx.await.expect("the mutation answers");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a member without the two member apps takes the presenter's road rendered \
         without `residents_present`: {outcome:?}"
    );
    td
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_the_two_apps_takes_the_presenter_without_the_parameter() {
    if !guard("a_member_without_the_two_apps_takes_the_presenter_without_the_parameter") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        data_wait_ms: 5_000,
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_the_old_residents(&s).await;
    warm(&s).await;

    s.turn_in("t1", "show me my files", round()).await;
    s.ask().await.release(verdict("files", "tree"));
    s.wait_drawn("show-files-tree").await;
    s.c.shutdown().await;
}
