//! GH #965 -- the member's residents show on the screen: the presenter's built-in topics
//! read a REAL resident over the road the builder renders, and the screen's round decides
//! what of the answer may stand.
//!
//! # Why this file exists
//!
//! The pure locks of GH #965 (`gh965_the_residents_have_topics_on_the_screen.rs`,
//! `gh965_a_person_appears_by_the_names_released_to_the_round.rs`) run the shipped
//! scripts one message at a time. What they cannot show is the road between them: the
//! presenter's `resident_read` leaving its rim, the builder's edge restamping it onto the
//! resident's own read lane with the member's round, the resident answering, and the edge
//! back stamping `resident_answer` with the round of the ANSWER -- the only round the
//! presenter gates on. This file boots that road on a colony.
//!
//! # What is booted
//!
//! The presenter colony of `support/presenter_colony.rs` (the shipped presenter under
//! `/alex/apps/presenter`, a held decider, the shipped display), and then, by ONE ordinary
//! mutation at the member's scope `/alex`:
//!
//! - the SHIPPED `colony-view` as `/alex/apps/colony-view` -- a real resident without a
//!   model, answering `in_read {op: stats}` out of its last snapshot of `/colony/graph`;
//! - two `code` doubles, `/alex/file-space` and `/alex/affinity`, that answer their read
//!   lane the way the real cells do (a listing, and affinity's `list` of released names);
//! - the edges `install_app` renders for the presenter's own `reads_residents`
//!   (`templates/presenter/template.json`, generation `sam`, `ctx.member_person` `alex`),
//!   exactly as rendered, for those three residents.
//!
//! The recipe renders edges for the seven residents the presenter read before GH #976
//! (the two member apps `daily-digest` and `research-assistant` only when
//! `residents_present` names them -- GH #976's own lock,
//! `gh976_the_last_result_shows_only_to_its_round`). `memory-hive`, `graph-space`,
//! `objects` and `librarian` are not booted here, so their edges are left out (an edge to
//! a node that does not stand is no edge the mutation takes); the rendered `in_show` and
//! `show_*` edges are left out as well, because the harness draws its own lane from
//! `in_show` to the test. The presenter node itself already stands, so the rendered
//! `add_nodes` is not applied either.
//!
//! # What is measured, at the receiver
//!
//! 1. A sure `colony` verdict opens the window with its working hint, and then the
//!    standard block `counts` stands at `web` carrying the numbers of the REAL colony
//!    view's answer (`cells > 0`), the answer having come back in the round `["*"]`.
//! 2. On a screen whose round the member's round does not cover (a third party in the
//!    room), a member-round topic (`files`) places nothing and is withdrawn after
//!    `data_wait_ms` -- the answer arrived, and the gate is what held it. The colony's
//!    counts (`["*"]`) still stand there: the positive control.
//! 3. While the decider holds a `people` turn, its request carries the turn's text and the
//!    topic descriptions and no person: affinity has not been read, nothing of it is on
//!    the screen. The names reach the screen only after the verdict.
//!
//! Absence is proven by a later event of the same order, never by waiting. Guarded like
//! every template-reading test (GH #49).

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use display_colony::{SCREEN, body_of, copy_tree, hop_of, read_json, repo, write_json};
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::mock_http::MockResponse;
use presenter_colony::{Dials, PRESENTER, Stage, boot, decision, guard, to_path, warm};
use tokio::sync::oneshot;

/// The member's round, as the builder's edge stamps it (`agent:<generation>`,
/// `member:<ctx.member_person>`).
const ROUND: &str = r#"["agent:sam","member:alex"]"#;
const GENERATION: &str = "sam";
const PERSON: &str = "alex";
/// Where the rendered road puts the colony view (`RESIDENTS` of the recipe).
const VIEW: &str = "/alex/apps/colony-view";

fn round() -> Value {
    meclaw_core::serde_json::from_str(ROUND).expect("the round parses")
}

/// The templates this file reads besides the presenter colony's own (R2b, GH #9).
fn shipped() -> bool {
    [
        "templates/colony-view/template.json",
        "templates/builder/recipes/config.json",
        "templates/presenter/template.json",
        "templates/presenter/stage/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

// ══════════════════════════════════════════════════════════════ the doubles

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

/// `affinity` on `in_brief`: the `list` of the people whose names are released, as
/// the shipped brief answers it -- the double knows a person by name.
const AFFINITY: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
if str(hop.get("route") or "") == "in_brief":
    sys.stdout.write(json.dumps({
        "header": {"route": "answer"},
        "messages": [], "op": "list",
        "people": [{"name": "Jonas Berg"}]}))
else:
    sys.stdout.write(json.dumps([]))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
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
                    "summary": {"type": "string", "required": false},
                    "people": {"type": "array", "required": false}
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
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

// ═══════════════════════════════════════════════════════════════ the road

/// `install_app` as the builder renders it over the presenter's own `reads_residents`.
fn rendered_diff() -> Value {
    let block = read_json(&repo("templates/presenter/template.json"))["app"].clone();
    let declaration = json!({"reads_residents": block["reads_residents"]});
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
                                         "params": {"scope": "/alex", "app": "presenter",
                                                    "template": "presenter@1.2.1",
                                                    "screen": "display-main",
                                                    "generation": GENERATION,
                                                    "declaration": declaration,
                                                    "ctx": {"member_person": PERSON}}})
                                      .to_string()}],
        }),
    );
    let first = out.first().expect("the recipe emitted nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the presenter's resident words: {first}"
    );
    first["manifest"][0]["diff"].clone()
}

/// The rendered edges between the presenter and the three residents booted here, as
/// rendered. The others name residents that do not stand (see the module comment).
fn resident_road() -> Vec<Value> {
    let booted = ["./apps/colony-view", "./file-space", "./affinity"];
    let diff = rendered_diff();
    let edges = diff["add_edges"].as_array().cloned().unwrap_or_default();
    let ends: Vec<&str> = edges
        .iter()
        .flat_map(|e| [e["from"].as_str(), e["to"].as_str()])
        .flatten()
        .collect();
    for resident in [
        "./memory-hive",
        "./file-space",
        "./graph-space",
        "./objects",
        "./librarian",
        "./affinity",
        "./apps/colony-view",
    ] {
        assert!(
            ends.contains(&resident),
            "the recipe renders a road to every resident the presenter reads: {resident}"
        );
    }
    let kept: Vec<Value> = edges
        .into_iter()
        .filter(|e| {
            let (from, to) = (e["from"].as_str(), e["to"].as_str());
            (from == Some("./apps/presenter") && booted.contains(&to.unwrap_or("")))
                || (to == Some("./apps/presenter") && booted.contains(&from.unwrap_or("")))
        })
        .collect();
    // Out and back for each of the three; affinity answers on two routes.
    assert_eq!(
        kept.len(),
        7,
        "the rendered road of the booted residents: {kept:#?}"
    );
    kept
}

/// The templates the mutation grows from: the shipped `colony-view` and `web` (the table
/// is rescanned whole), and the two doubles.
fn templates_dir() -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("a temporary directory");
    let root = td.path();
    copy_tree(&repo("templates/colony-view"), &root.join("colony-view"));
    copy_tree(&repo("templates/web"), &root.join("web"));
    for (name, script, purpose) in [
        (
            "file-space",
            FILE_SPACE,
            "Double of the member's file space.",
        ),
        ("affinity", AFFINITY, "Double of the member's affinity."),
    ] {
        write_json(
            &root.join(name).join("template.json"),
            &json!({"name": name}),
        );
        write_json(
            &root.join(name).join("config.json"),
            &double(script, purpose),
        );
    }
    td
}

/// The residents and their road, grown into the running member by one mutation; the
/// returned directory holds the templates and lives as long as the test.
async fn install_residents(s: &Stage) -> tempfile::TempDir {
    let td = templates_dir();
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().to_path_buf(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");
    let diff = json!({
        "add_nodes": [
            {"name": "apps/colony-view", "template": "colony-view"},
            {"name": "file-space", "template": "file-space"},
            {"name": "affinity", "template": "affinity"}
        ],
        "add_edges": resident_road(),
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
        "growing the residents and their rendered road is one ordinary mutation: {outcome:?}"
    );
    td
}

/// The colony view takes its first snapshot (what the boot receipt does in a colony that
/// names `mutation_receipts`; this one does not), observed where it lands: the layout.
async fn snapshot_colony_view(s: &Stage) {
    s.c.h
        .send(to_path(
            VIEW,
            json!({"route": "in_refresh"}),
            json!({}),
            json!({"messages": []}),
        ))
        .await;
    let layout = format!("{VIEW}/layout");
    s.c.wait_until("the colony view holds a snapshot", || async {
        s.c.log(Some(&layout))
            .await
            .iter()
            .any(|r| hop_of(r)["route"] == "snapshot")
    })
    .await;
}

/// A sure verdict for `topic` with `lead`, every other asked key answered with its quiet
/// default (the translate wants every key: `decision_incomplete` otherwise).
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

/// The round a logged `resident_round` names, read whether it travels as a list or as
/// its JSON text.
fn round_of(v: &Value) -> Value {
    match v.as_str() {
        Some(text) => meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null),
        None => v.clone(),
    }
}

/// The `resident_answer` from `resident` as it reached `stage` -- the receiver.
async fn answer_at_stage(s: &Stage, resident: &str) -> (Value, Value) {
    let stage = format!("{PRESENTER}/stage");
    s.c.wait_until("the resident's answer reaches stage", || async {
        s.c.log(Some(&stage)).await.iter().any(|r| {
            let h = hop_of(r);
            h["route"] == "resident_answer" && h["resident"] == resident
        })
    })
    .await;
    let rows = s.c.log(Some(&stage)).await;
    let row = rows
        .iter()
        .find(|r| {
            let h = hop_of(r);
            h["route"] == "resident_answer" && h["resident"] == resident
        })
        .expect("the answer stands in the log");
    (hop_of(row), body_of(row))
}

/// The props of the one object at `web` whose id carries `needle` and that is drawn as
/// `component`.
async fn drawn_props(s: &Stage, needle: &str, component: &str) -> Value {
    let tree = s.c.tree().await;
    tree.as_object()
        .and_then(|m| {
            m.iter()
                .find(|(k, v)| k.contains(needle) && v["component"] == component)
                .map(|(_, v)| v["props"].clone())
        })
        .unwrap_or(Value::Null)
}

fn as_number(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))
}

/// The index of the first view the screen RECEIVED (its `in_view` lane) that names
/// `needle` -- read at the screen's door, where the presenter's lane arrives in order.
async fn first_view_with(s: &Stage, needle: &str) -> Option<usize> {
    s.c.log(Some(SCREEN))
        .await
        .iter()
        .filter(|r| hop_of(r)["route"] == "in_view")
        .position(|r| body_of(r).to_string().contains(needle))
}

// ═══════════════════════════════════════════════════════════ the measurement

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_colony_topic_shows_the_real_colony_views_counts() {
    if !guard("the_colony_topic_shows_the_real_colony_views_counts") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        // The shipped built-in topics: this lock answers every question they add.
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_residents(&s).await;
    warm(&s).await;
    snapshot_colony_view(&s).await;

    s.turn_in("t1", "how is the colony doing", round()).await;
    s.ask().await.release(verdict("colony", "counts"));

    s.wait_drawn("show-colony-counts").await;
    let hint = first_view_with(&s, "show-colony-hint").await;
    let counts = first_view_with(&s, "show-colony-counts").await;
    assert!(
        matches!((hint, counts), (Some(h), Some(c)) if h < c),
        "the window with its working hint reaches the screen first, then the block: \
         hint at {hint:?}, counts at {counts:?}"
    );

    let (hop, body) = answer_at_stage(&s, "colony-view").await;
    assert_eq!(
        round_of(&hop["resident_round"]),
        json!(["*"]),
        "the colony's counts come back in the round of everybody: {hop}"
    );
    assert_eq!(
        hop["op_id"], "t1/counts",
        "the asker's own id comes back: {hop}"
    );
    let cells = body["cells"].as_i64().unwrap_or(0);
    assert!(
        cells > 0,
        "the real colony view counts the cells of this colony: {body}"
    );
    let card = drawn_props(&s, "show-colony-counts", "display-card").await;
    assert_eq!(
        as_number(&card["value"]),
        Some(cells),
        "the card carries the number the colony view answered: {card}"
    );

    let row = s.journal_of("t1").await;
    assert_eq!(row["topic"], json!("colony"), "{row}");
    assert_eq!(row["fallback"], json!("none"), "{row}");
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_with_a_third_party_shows_no_member_topic_but_the_counts() {
    if !guard("a_screen_with_a_third_party_shows_no_member_topic_but_the_counts") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex", "peer:x"]),
        data_wait_ms: 5_000,
        // The shipped built-in topics: this lock answers every question they add.
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_residents(&s).await;
    warm(&s).await;
    snapshot_colony_view(&s).await;

    // A member-round topic on a screen the member's round does not cover.
    s.turn_in("t1", "show me my files", round()).await;
    s.ask().await.release(verdict("files", "tree"));
    let (hop, body) = answer_at_stage(&s, "file-space").await;
    assert_eq!(
        round_of(&hop["resident_round"]),
        round(),
        "the files came back in the member's round: {hop}"
    );
    assert!(
        body["entries"].as_array().is_some_and(|e| !e.is_empty()),
        "the double answered with a listing -- the gate, not a missing answer, is what \
         holds it: {body}"
    );

    // The sentinel: the window's withdrawal reaches the screen. The screen takes the
    // presenter's lane in order, so a block written before it would have arrived before.
    s.c.wait_until("the files window is withdrawn at the screen", || async {
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .any(|r| hop_of(r)["route"] == "in_withdraw" && body_of(r)["view_id"] == "show-files")
    })
    .await;
    let rows = s.c.log(None).await;
    let stage = format!("{PRESENTER}/stage");
    let answered = rows
        .iter()
        .position(|r| {
            r.to_path == stage
                && hop_of(r)["route"] == "resident_answer"
                && hop_of(r)["resident"] == "file-space"
        })
        .expect("the answer reached stage");
    let withdrawn = rows
        .iter()
        .position(|r| r.from_path == stage && hop_of(r)["route"] == "withdraw")
        .expect("stage withdrew");
    let tick = rows[..withdrawn]
        .iter()
        .rposition(|r| r.to_path == stage && hop_of(r)["route"] == "in_tick")
        .expect("the clock struck before the withdrawal");
    assert!(
        answered < tick,
        "the answer was handled before data_wait_ms ran out (else this run proves \
         nothing): answer at {answered}, tick at {tick}"
    );
    let at_screen: Vec<String> =
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .filter(|r| hop_of(r)["route"] == "in_view")
            .map(|r| body_of(r).to_string())
            .collect();
    assert!(
        at_screen.iter().any(|b| b.contains("show-files-hint")),
        "the window itself reached the screen: {at_screen:?}"
    );
    for block in ["show-files-tree", "show-files-root", "/notes/plan.txt"] {
        assert!(
            !at_screen.iter().any(|b| b.contains(block)),
            "a member-round set reached a screen with a third party: {block}"
        );
        assert!(!s.drawn(block).await, "{block} stands at web");
    }
    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("no_data"), "{row}");

    // The positive control: the counts are everybody's and stand on the same screen.
    s.turn_in("t2", "how is the colony doing", round()).await;
    s.ask().await.release(verdict("colony", "counts"));
    s.wait_drawn("show-colony-counts").await;
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_decider_sees_no_person_and_the_names_follow_the_verdict() {
    if !guard("the_decider_sees_no_person_and_the_names_follow_the_verdict") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        // The shipped built-in topics: this lock answers every question they add.
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_residents(&s).await;
    warm(&s).await;

    s.turn_in("t1", "who do I know", round()).await;
    let held = s.ask().await;
    let request = held.json().to_string();
    assert!(
        request.contains("who do I know"),
        "the decider gets the turn's text: {request}"
    );
    assert!(
        request.contains("the people the member knows"),
        "the decider gets the topic descriptions: {request}"
    );
    for never in ["Jonas", "Berg", "peer:"] {
        assert!(
            !request.contains(never),
            "the decider's request names a person ({never}): {request}"
        );
    }
    // While the verdict is held, affinity has not been asked and nothing of it stands.
    assert!(
        s.c.log(Some("/alex/affinity")).await.is_empty(),
        "affinity was read before the verdict"
    );
    assert!(
        !s.c.tree().await.to_string().contains("Jonas"),
        "a name reached the screen before the verdict"
    );

    held.release(verdict("people", "known"));
    s.wait_drawn("show-people-known").await;
    assert!(
        s.c.tree().await.to_string().contains("Jonas Berg"),
        "the released name reaches the screen after the verdict"
    );
    let asked = s.c.log(Some("/alex/affinity")).await;
    let ctx: Value = asked
        .first()
        .and_then(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .map(|h| h["context"].clone())
        .unwrap_or(Value::Null);
    assert_eq!(
        round_of(&ctx["audience_set"]),
        round(),
        "the edge asks affinity in the member's round, never the app's: {ctx}"
    );
    let (hop, _) = answer_at_stage(&s, "affinity").await;
    assert_eq!(round_of(&hop["resident_round"]), round(), "{hop}");
    s.c.shutdown().await;
}
