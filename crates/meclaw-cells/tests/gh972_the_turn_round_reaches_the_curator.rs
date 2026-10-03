//! GH #972 (PE-DP-13, ruling R-NL-4; review I-1) -- the round a turn is born
//! in reaches the curator through the real road, and an app's pin in that
//! round is placed only in a turn of it.
//!
//! `gh972_a_pin_carries_the_round_its_row_proves.rs` locks the curator's rule
//! with a hand-set context; `gh967` locks that the recipe RENDERS
//! `context.turn_round` on the ingress edge. Neither shows that the stamp
//! survives the way between them -- an emission is not an arrival. This file
//! boots that way and reads the result where it is decided, in the curator's
//! ledger:
//!
//! ```text
//! connector --ingress (recipe `_channel_level`, add_edges at the door)--> ./channels
//!   --member edge (turn -> in_turn)--> ./firewall (shipped, screen + rules store)
//!   --install_app `listens: turn` (pass -> turn)--> ./apps --> ./apps/goal-app/pin
//!   --install_app `pins` (pin -> in_pin, audience_set := member round)--> ./assistants/scribe
//!   --assistant edge--> ./talky --talky edge--> ./curator (shipped, role talky) -> ledger
//! ```
//!
//! Every edge on that way is the shipped one: the member's, the firewall's,
//! the assistant's and talky's own (filtered to the lanes of this road), and
//! the ones the SHIPPED `recipes` renders for the channel (`grow_level`) and
//! the app (`install_app`). The app is a fixture: one code cell that, on every
//! turn it hears, pins two goals of the source `goal` in ONE message (the form
//! of GH #972 L.5) -- a group goal labelled with the group's round, and a
//! private goal labelled with the member's.
//!
//! Two channels of one member stand in the tree. `chat` is rendered by the
//! recipe as it is (the member's round). `room` is the group with a guest:
//! meclaw has no recipe that births such a round (OR-NL.L.2 -- soul2me rooms
//! and the orga lab wire it themselves), so its ingress is the recipe's edge
//! with both round literals swapped for the group's, and it goes through the
//! mutation door as `add_edges` -- the one place a stamped key may be set.
//!
//! Measured at the receiver:
//!
//! 1. a member turn: the private goal stands in the member's round, the group
//!    goal stands NOWHERE (`pin:held` -- the turn proves only the member's
//!    round, and the group's is wider than the ceiling);
//! 2. a group turn: the group goal now stands in the group's round -- the
//!    turn's stamp crossed connector, firewall, app and the builder's pin edge
//!    -- and the private goal still stands in the member's round only: no row
//!    of it ever names the guest;
//! 3. the same app with an inner edge of its own that sets `turn_round` is
//!    refused at the mutation door (`edge_schema`).
//!
//! Red before (and against a rebuild without the stamp): with the ingress
//! edge not stamping `turn_round` -- or with any edge on the way dropping it --
//! step 2 never sees the group goal (it stays `pin:held`).
//!
//! No model and no network: the one `llm` cell of the tree (the curator's
//! summarizer) points at a closed local port and is never asked. Guarded like
//! every template-reading test (GH #49): a tree without one of the templates
//! is skipped, never judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, emit_all, override_params_on_disk, shipped_script};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The failure-marker convention of this repo, not a timing discriminator:
/// every wait below ends on an event (a ledger row), this only bounds it.
const DEADLINE: Duration = Duration::from_secs(60);

const MEMBER_DIR: &str = "/os/orgs/acme/members/e";
const AGENT: &str = "scribe";
const PERSON: &str = "e";
/// The member's round (canonical, as the ledger writes it): the ceiling the
/// builder's pin edge stamps, and the round of a turn on `chat`.
const MEMBER: &str = r#"["agent:scribe","member:e"]"#;
/// The group with a guest (`member:b`): wider than the member's round.
const GROUP: &str = r#"["agent:scribe","member:b","member:e"]"#;
const APP: &str = "goal-app";
const SOURCE: &str = "goal";
const GROUP_GOAL: &str = "paint the fence together";
const PRIVATE_GOAL: &str = "my secret goal";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// GH #49: every template this road reads, or the test is skipped.
fn shipped() -> bool {
    [
        "templates/builder/recipes/config.json",
        "templates/member/config.json",
        "templates/firewall/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/talky/curator/config.json",
        "templates/curator/config.json",
    ]
    .iter()
    .all(|f| repo(f).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

// ══════════════════════════════════════════════════════════ the renderer

fn recipe(recipe: &str, params: Value) -> Value {
    let out = emit_all(
        &shipped_script(&repo("templates/builder/recipes/config.json").to_string_lossy()),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    let first = out.first().expect("the renderer emits").clone();
    assert!(
        first["header"]["error_code"].is_null(),
        "{recipe}: refused: {first}"
    );
    first["manifest"][0].clone()
}

/// The channel `node` as `grow_level` renders it, moved to the container this
/// colony has (`/channels`); the node itself is booted from the tree (a grown
/// channel is born asleep, `gh940`), so only the edges stay in the diff.
fn channel_edges(node: &str) -> Value {
    let mut decl = recipe(
        "grow_level",
        json!({"scope": MEMBER_DIR, "level": "channel", "name": node,
               "template": "chat-channel@1.0.1", "assistant": AGENT,
               "ctx": {"member_person": PERSON}}),
    );
    let edges = decl["diff"]["add_edges"].clone();
    decl["scope"] = json!("/channels");
    decl["diff"] = json!({"add_edges": edges});
    decl
}

/// The group's ingress: the recipe's edge with both round literals -- the
/// ceiling `audience_set` and the stamp `turn_round` -- naming the group with
/// its guest (the wiring soul2me rooms and the orga lab draw, OR-NL.L.2).
fn group_channel_edges(node: &str) -> Value {
    let mut decl = channel_edges(node);
    let member = format!("'{MEMBER}'");
    let group = format!("'{GROUP}'");
    let mut swapped = 0;
    for e in decl["diff"]["add_edges"]
        .as_array_mut()
        .expect("add_edges")
        .iter_mut()
    {
        // `get_mut`, not `IndexMut`: indexing would plant `modifier: null`
        // on every edge without one, and the door refuses that.
        let Some(set) = e
            .get_mut("modifier")
            .and_then(|m| m.get_mut("set_context"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for key in ["audience_set", "turn_round"] {
            if set.get(key) == Some(&json!(member)) {
                set.insert(key.into(), json!(group));
                swapped += 1;
            }
        }
    }
    // Only the ceiling is required here: whether the recipe stamps the turn's
    // round is what `gh967` reads off the rendered edge, and what THIS file
    // measures at the ledger (a rebuild without it fails step 2, not here).
    assert!(
        swapped >= 1,
        "the recipe's ingress no longer stamps the member's round: {decl}"
    );
    decl
}

/// The app's declaration: it hears the screened turn and pins through `./pin`.
fn app_declaration() -> Value {
    json!({"listens": ["turn"], "pins": "./pin"})
}

/// What `install_app` draws for the app, member-relative.
fn install_edges() -> Vec<Value> {
    recipe(
        "install_app",
        json!({"scope": MEMBER_DIR, "app": APP, "template": format!("{APP}@1.0.0"),
               "screen": "display", "generation": AGENT,
               "ctx": {"member_person": PERSON}, "declaration": app_declaration()}),
    )["diff"]["add_edges"]
        .as_array()
        .expect("install_app renders edges")
        .clone()
}

// ═══════════════════════════════════════════════════════════════ the tree

/// The connector, doubled: a wake becomes an inbound line.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({"header": {"user_id": "u-1"},
                             "messages": doc["body"].get("messages", [])}))
"#;

/// The app's one cell: every turn it hears, it pins both goals of its source
/// in ONE message, each with its own round.
fn pin_script() -> String {
    format!(
        r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc.get("envelope") or {{}}).get("header") or {{}}).get("hop") or {{}}
out = []
if hop.get("route") == "turn":
    out.append({{"header": {{"route": "pin"}}, "messages": [],
                "replace_sources": ["{SOURCE}"],
                "pins": [{{"text": "{GROUP_GOAL}", "source": "{SOURCE}",
                           "audience_set": {group}}},
                         {{"text": "{PRIVATE_GOAL}", "source": "{SOURCE}",
                           "audience_set": {member}}}]}})
sys.stdout.write(json.dumps(out))
"#,
        group = json!(GROUP),
        member = json!(MEMBER),
    )
}

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script,
                   "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0", "settings": {}, "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(depth < 8, "ref chain does not end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let name = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template")
                .split('@')
                .next()
                .unwrap_or_default()
                .to_string();
            copy_resolved(&repo("templates").join(name), dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every timer out of the run's way, every `llm` cell at a closed local port
/// (none is asked on this road).
fn inert_clocks_and_models(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for (n, f) in files.into_iter().enumerate() {
        let mut cfg = read_json(&f);
        match cfg["cell"]["type"].as_str() {
            Some("timer") => {
                // The curator's clock ships without schedules (its member
                // writes them); a timer boots only with a list.
                if !cfg["params"]["schedules"].is_array() {
                    cfg["params"]["schedules"] = json!([]);
                }
                if let Some(schedules) = cfg["params"]["schedules"].as_array_mut() {
                    for s in schedules.iter_mut() {
                        if s["schedule_id"]
                            .as_str()
                            .is_some_and(|id| id.contains("${"))
                        {
                            s["schedule_id"] =
                                json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0972_0000 + n));
                        }
                        if s.get("cron").is_some() {
                            s["cron"] = json!(NEVER_CRON);
                        }
                    }
                }
            }
            Some("llm") => {
                cfg["params"]["base_url"] = json!("http://127.0.0.1:9/v1");
                cfg["params"]["model"] = json!("stub");
                cfg["params"]["api_key"] = json!("sk-test");
            }
            _ => continue,
        }
        write_json(&f, &cfg);
    }
}

/// The edges of `template` (a shipped hive) that `keep` selects.
fn template_edges(template: &str, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(keep)
        .collect()
}

fn on_route(e: &Value, route: &str) -> bool {
    e["condition"]
        .as_str()
        .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
}

/// The member stand-in. Its graph: the member's own edges of the firewall
/// (every one leaving the tree ends at `/park` -- the assistants' turn, the
/// file space, the member's rim), the edges `install_app` renders for the
/// app, and a parking default for the rest. `./channels` is the member's
/// container with an EMPTY graph (the recipe's edges go through the door);
/// `./assistants/scribe` and its `talky` carry the shipped `in_pin` edges of
/// `assistant` and `talky`; the curator is the shipped one in the role talky.
fn build_tree(root: &std::path::Path) {
    let main = root.join("main");
    let mut edges: Vec<Value> = template_edges("member", |e| {
        e["from"] == json!("./firewall") || e["to"] == json!("./firewall")
    })
    .into_iter()
    .filter(|e| e["from"] != json!("."))
    .map(|mut e| {
        if e["from"] == json!("./firewall") {
            e["to"] = json!("/park");
        }
        e
    })
    .collect();
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == json!("./channels") && on_route(e, "turn")),
        "the member no longer hands a channel's turn to its firewall"
    );
    edges.extend(install_edges());
    for from in ["./channels", "./apps", "./assistants", "./firewall"] {
        edges.push(json!({"from": from, "to": "/park",
                          "condition": "has(hop.route)", "default": true}));
    }
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );

    write_json(
        &main.join("channels/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    for node in ["chat", "room"] {
        write_json(
            &main.join(format!("channels/{node}/config.json")),
            &double(CONNECTOR, "Test double for a chat connector."),
        );
    }

    copy_resolved(&repo("templates/firewall"), &main.join("firewall"), 0);

    write_json(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write_json(
        &main.join(format!("apps/{APP}/config.json")),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./pin", "condition": "has(hop.route) && hop.route == 'turn'"}
        ]}}}),
    );
    write_json(
        &main.join(format!("apps/{APP}/pin/config.json")),
        &double(
            &pin_script(),
            "Test app: pins a group goal and a private goal.",
        ),
    );

    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    let generation = template_edges("assistant", |e| {
        on_route(e, "in_pin") && e["to"] == json!("./talky")
    });
    assert_eq!(
        generation.len(),
        1,
        "the generation hands a pin to talky once"
    );
    write_json(
        &main.join(format!("assistants/{AGENT}/config.json")),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": generation}}}),
    );
    let brain = template_edges("talky", |e| {
        on_route(e, "in_pin") && e["to"] == json!("./curator")
    });
    assert_eq!(brain.len(), 1, "talky hands a pin to its curator once");
    write_json(
        &main.join(format!("assistants/{AGENT}/talky/config.json")),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": brain}}}),
    );
    copy_resolved(
        &repo("templates/talky/curator"),
        &main.join(format!("assistants/{AGENT}/talky/curator")),
        0,
    );
    inert_clocks_and_models(&main);
}

// ═══════════════════════════════════════════════════════════════ the colony

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("channels, firewall, app, generation and curator must boot");
    (h, park_rx)
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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
    ack_rx.await.expect("mutation ack")
}

/// One line typed on the connector `node`.
fn typed(node: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/channels/{node}")))
        .hop(as_map(&json!({"route": "in_typed"})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn ledger(td: &tempfile::TempDir) -> std::path::PathBuf {
    td.path().join(format!(
        "main/assistants/{AGENT}/talky/curator/ledger/cell.db"
    ))
}

/// `(text, audience_set)` of every pin row of the source, live or not.
fn pin_rows(db: &std::path::Path) -> Vec<(String, String)> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open the ledger");
    let Ok(mut st) = conn.prepare(
        "SELECT b.body, p.audience_set FROM pins p JOIN blocks b ON b.hash = p.hash \
         WHERE p.source = ?1",
    ) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = st
        .query_map([SOURCE], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })
        .expect("query")
        .filter_map(Result::ok)
        .map(|(body, round)| {
            let body: Value = meclaw_core::serde_json::from_str(&body).unwrap_or(Value::Null);
            (body["text"].as_str().unwrap_or("").to_string(), round)
        })
        .collect();
    out.sort();
    out
}

/// Wait for an EVENT -- a row `(text, round)` in the ledger -- never a window.
async fn until_row(db: &std::path::Path, text: &str, round: &str, what: &str) {
    let deadline = Instant::now() + DEADLINE;
    let want = (text.to_string(), round.to_string());
    while !pin_rows(db).contains(&want) {
        assert!(
            Instant::now() < deadline,
            "{what}: no row {want:?} within {DEADLINE:?}; the ledger holds {:?}",
            pin_rows(db)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The lock: a turn's round reaches the curator through connector, firewall,
/// app and the builder's pin edge, and only a turn of the group places the
/// group's goal; the private goal never stands in the guest's round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_round_of_a_turn_reaches_the_curator_through_the_road() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().expect("a temporary directory");
    build_tree(td.path());
    let (h, _park) = boot(&td).await;

    // The wiring goes through the door as `add_edges`: the one form that may
    // stamp `turn_round`.
    for decl in [channel_edges("chat"), group_channel_edges("room")] {
        let outcome = mutate(&h, decl.clone()).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "the channel's wiring was not committed: {outcome:?}\n{decl}"
        );
    }
    let db = ledger(&td);

    // (1) A member turn. The private goal is placed -- the event this step
    // waits for; the group goal rode in the SAME message, so its verdict is
    // in as well: held, nowhere in the ledger.
    h.send(typed("chat", "what is on my list?")).await;
    until_row(&db, PRIVATE_GOAL, MEMBER, "the member turn").await;
    let rows = pin_rows(&db);
    assert!(
        !rows.iter().any(|(t, _)| t == GROUP_GOAL),
        "the group goal was placed in a turn of the member's round -- only a \
         turn of the group proves the group's round: {rows:?}"
    );

    // (2) A turn of the group with the guest: its stamp crossed the whole
    // road, so the group goal stands in the group's round now.
    h.send(typed("room", "shall we paint the fence?")).await;
    until_row(&db, GROUP_GOAL, GROUP, "the group turn").await;
    let rows = pin_rows(&db);
    assert!(
        rows.contains(&(PRIVATE_GOAL.to_string(), MEMBER.to_string())),
        "the private goal left its round in the group's turn: {rows:?}"
    );
    for (text, round) in &rows {
        assert!(
            !(text == PRIVATE_GOAL && round.contains("member:b")),
            "the private goal stands in the guest's round: {rows:?}"
        );
    }

    // (3) The same app drawing the stamp itself is refused at the door.
    let forged = json!({"scope": "/apps", "diff": {"add_nodes": [{
        "name": "forger", "template": format!("{APP}@1.0.0"),
        "override_params": {"graph": {"edges": [
            {"from": "./pin", "to": ".",
             "modifier": {"set_context": {"turn_round": format!("'{GROUP}'")}}}
        ]}}
    }]}});
    match mutate(&h, forged).await {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "{details}");
            assert!(details.contains("turn_round"), "{details}");
        }
        other => panic!("an app's own edge set `turn_round` and was let in: {other:?}"),
    }

    h.shutdown().await;
}
