//! GH #835 — a member's door: one assistant receives every turn that names no
//! agent.
//!
//! A member may own several assistants (`gh454_two_assistants_one_channel.rs`),
//! and which one a turn reaches is decided by the channel that stamped
//! `context.assistant` — never by a model. The container fans out on that name
//! with one STRICT edge per assistant (`has(context.assistant) &&
//! context.assistant == '<name>'`), because `Edge.to` is a static path. So a
//! turn that names no agent — an `in_turn` at the member's own rim, a frame from
//! a channel that stamps no default — matched nothing at the container and died
//! there as `hive_no_route`: "a turn that names no generation has nowhere to go"
//! (`templates/member/README.md` § Addressing an assistant through a channel).
//!
//! The substrate already had the lever for exactly that case: a DEFAULT edge
//! (GH #283), evaluated only when no regular out-edge of the same sender decided
//! (`crates/meclaw-colony/src/edge_table.rs`, `apply_edges`). `grow_level
//! assistant … door: true` draws one, beside the strict guard, and stamps the
//! door's name on the way in, so the turn carries it from there on.
//!
//! # What is measured, and where
//!
//! On a BOOTED colony, at the receiver (the generation's own surface reports the
//! context it was handed), never at the emitter:
//!
//! - a turn with no `context.assistant` reaches the door, and the door's surface
//!   sees `context.assistant == '<door>'`;
//! - a turn that names the other assistant reaches that one and NOT the door —
//!   the regular edge decided, so the default phase never ran;
//! - a turn that names an agent this member does not have reaches the door too:
//!   nothing else decided for it (`templates/member/README.md` § The member's
//!   door says so, and this is the pin behind the sentence);
//! - without a door the unaddressed turn is still `hive_no_route` at the
//!   container, which is the state every colony grown before this change is in;
//! - the rendered manifest IS the example `examples/organism/grow-member-door.json`,
//!   down to the digest a human says yes to.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant` templates, cell for cell, doubled the way
//! `gh454` doubles them (a `config.json` REPLACED, never a directory deleted —
//! GH #286). The two generations are NOT wired by hand: the edges in the
//! `assistants` container are the ones the SHIPPED `recipes` cell renders for
//! each of them, placed verbatim into the container's graph — the declaration
//! stands at `<member>/assistants` (GH #503), so its `.` is that container.
//!
//! Guarded like every other template-reading test (GH #49): a tree that did not
//! ship the templates is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, DeadLetterReason, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

/// The golden this change ships beside `grow-assistant.json`.
const EXAMPLE: &str = "examples/organism/grow-member-door.json";

/// The two templates this test boots, or `None` when the tree under test did not
/// travel into the public export.
fn shipped() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let member = repo("templates/member");
    let assistant = repo("templates/assistant");
    (member.join("config.json").is_file() && assistant.join("config.json").is_file())
        .then_some((member, assistant))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

// ═══════════════════════════════════════════════════════════════ the renderer

/// The wish that grows one generation, with or without the door. The values are
/// the ones `examples/organism/grow-assistant.json` carries, so the door form
/// differs from it in exactly one key.
fn wish(scope: &str, name: &str, door: bool) -> Value {
    let mut params = json!({
        "scope": scope, "level": "assistant", "name": name,
        "template": "assistant@2.9.0",
        "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                "model_surface": "${MODEL_SURFACE}"},
        "override_params": {"cogny/brain": {"temperature": 0.2}}
    });
    if door {
        params["door"] = json!(true);
    }
    json!({"recipe": "grow_level", "request": "…", "params": params})
}

/// What the SHIPPED `recipes` cell answers to a wish: the one manifest emission.
fn render(wish: Value) -> Value {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": wish.to_string()}],
        }),
    )
    .into_iter()
    .find(|m| m["header"]["operation"] == json!("recipe"))
    .unwrap_or_else(|| panic!("the recipe rendered nothing for {wish}"))
}

/// The one declaration an assistant wish renders.
fn declaration(wish: Value) -> Value {
    let out = render(wish);
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"));
    assert_eq!(decls.len(), 1, "an assistant is one declaration: {decls:?}");
    decls[0].clone()
}

/// The bytes `templates/builder/recipes` digests: `json.dumps(sort_keys=True,
/// separators=(",", ":"), ensure_ascii=False)`. Keys are sorted HERE rather than
/// left to the map type, so the statement does not depend on whether some crate
/// in the build switched on `serde_json/preserve_order`.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        meclaw_core::serde_json::to_string(k).expect("a key"),
                        canonical(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        leaf => meclaw_core::serde_json::to_string(leaf).expect("a leaf"),
    }
}

/// The door edge exactly as GH #835 and ruling OR-AG-25 spell it, in the form
/// the member's own three default edges are written in (`default`, `condition`,
/// `modifier.set_context`), for the child `name` of a declaration that stands in
/// the container.
fn door_edge(name: &str) -> Value {
    json!({"from": ".", "to": format!("./{name}"), "default": true,
           "condition": "has(hop.route) && hop.route == 'in_turn'",
           "modifier": {"set_context": {"assistant": format!("'{name}'")}}})
}

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders of the member this round never
/// reaches. They exist because a hive door pointing at an absent directory
/// leaves the inside unroutable (GH #286).
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's screen, doubled: it passes every turn and touches no context, so
/// what arrives at the container is exactly what the driver sent — with or
/// without a `context.assistant`.
const FIREWALL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "pass", "screened": "1"},
    "messages": doc["body"].get("messages", [])}))
"#;

/// The conversation surface of one generation, doubled. It answers and carries
/// back WHO it is and WHICH name the turn context held when it arrived — the
/// reading at the receiver, which is where a route is proved.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
ctx = hdr.get("context") or {}
who = str((doc.get("params") or {}).get("who") or "")
sys.stdout.write(json.dumps({
    "header": {"route": "answer", "served_by": who,
               "saw_has_assistant": "1" if "assistant" in ctx else "0",
               "saw_assistant": str(ctx.get("assistant") or "")},
    "messages": [{"origin": "assistant", "type": "text", "text": "answered by " + who}]}))
"#;

/// Puts one turn on the member's own rim door, `in_turn`. The name travels as a
/// hop key and the main graph decides whether it becomes a context key at all.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "in_turn", "addressed_to": str(hop.get("addressed_to") or "")},
    "messages": doc["body"].get("messages", [])}))
"#;

fn double(script: &str, params: Value, purpose: &str) -> Value {
    let mut p = json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 10000
    });
    if let Value::Object(extra) = params {
        for (k, v) in extra {
            p[k] = v;
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": p,
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": purpose,
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ══════════════════════════════════════════════════════════════════ the tree

/// The member this colony holds, as a path. The wish names it as its scope.
const MEMBER: &str = "/person";
/// The generation grown as the door, when there is one.
const DOOR: &str = "alpha";
/// The generation a turn has to NAME to reach.
const OTHER: &str = "beta";

/// The colony around the member: one driver and a drain for every lane the
/// member emits, so nothing this file reads is a silence caused by a missing
/// drain.
///
/// TWO driver edges, because what is under test is a turn with NO
/// `context.assistant` at all — not one with an empty string. An operator that
/// names an agent promotes the name; one that names none promotes nothing.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn' && \
                          has(hop.addressed_to) && hop.addressed_to != ''",
            "modifier": {"set_context": {"assistant": "hop.addressed_to"}}
        }),
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn' && \
                          (!has(hop.addressed_to) || hop.addressed_to == '')"
        }),
    ];
    for lane in [
        "answer",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "prune",
        "build",
        "close_report",
        "export_done",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// Stage the tree: the shipped member, the shipped assistant twice, the doubles,
/// and in the `assistants` container the edges the recipe renders for both
/// generations — the first grown with `door`, when `with_door` says so.
fn build_tree(
    td: &tempfile::TempDir,
    member: &std::path::Path,
    assistant: &std::path::Path,
    with_door: bool,
) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(
            DRIVER,
            json!({}),
            "Test driver: puts one turn on the member's rim.",
        ),
    );

    copy_cells(member, &root.join("main/person"));
    for holder in ["access", "affinity", "memory-hive"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(
                INERT,
                json!({}),
                "Inert double for a holder this round never reaches.",
            ),
        );
    }
    write(
        root,
        "main/person/firewall/config.json",
        &double(FIREWALL, json!({}), "Test double for the member's screen."),
    );

    let mut container_edges: Vec<Value> = Vec::new();
    for (who, door) in [(DOOR, with_door), (OTHER, false)] {
        let dst = root.join(format!("main/person/assistants/{who}"));
        copy_cells(assistant, &dst);
        // Both keepers answer, because what is measured is WHICH GENERATION a
        // turn reaches and not which keeper inside it (GH #709 split them by
        // channel, and a rim turn names none).
        for keeper in ["talky", "talky-chat"] {
            write(
                root,
                &format!("main/person/assistants/{who}/{keeper}/config.json"),
                &double(
                    SURFACE,
                    json!({"who": who}),
                    "Test double for a conversation surface of one generation.",
                ),
            );
        }
        for sibling in ["cogny", "tools"] {
            write(
                root,
                &format!("main/person/assistants/{who}/{sibling}/config.json"),
                &double(
                    INERT,
                    json!({}),
                    "Inert double for a sibling this round never reaches.",
                ),
            );
        }
        // The RENDERED level, not a hand-drawn pair. The declaration stands in
        // the container (GH #503), so its edges are that container's graph.
        let decl = declaration(wish(MEMBER, who, door));
        assert_eq!(
            decl["scope"],
            json!(format!("{MEMBER}/assistants")),
            "an assistant declares itself at the container it grows into, so its \
             edges belong to the container's own graph: {decl}"
        );
        container_edges.extend(
            decl["diff"]["add_edges"]
                .as_array()
                .expect("the level renders edges")
                .iter()
                .cloned(),
        );
    }

    let container = root.join("main/person/assistants/config.json");
    let mut cfg = read_json(&container);
    cfg["params"] = json!({"graph": {"edges": container_edges}});
    std::fs::write(
        &container,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the container config");

    // The fixture's `.env` is written HERE, at run time, and it is empty: no
    // double reads the environment.
    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member, two rendered generations and the doubles must boot");
    (h, sink_rx)
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One turn into the driver. `addressed_to` empty = the turn names no agent.
fn inject(addressed_to: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("addressed_to".into(), json!(addressed_to));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "hello"}
        ]})))
        .hop(hop)
        .ttl(200)
        .build()
}

/// Failure-marker timeout: generous on purpose (30 s convention), robust against
/// cargo-parallel load.
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The settle before a "nobody else answered" assertion — a semantic
/// discriminator, argued rather than made generous (the `gh286` reading): every
/// edge out of the container is decided in ONE `apply_edges` call on ONE
/// message, so a second generation being handed the same turn is not a slow
/// second round, it is already in flight when the first answer arrives. The
/// wait only lets an in-flight delivery land, and it starts AFTER the positive
/// receipt.
const SETTLE: Duration = Duration::from_millis(750);

/// Boot the tree, send one turn, hand back the first thing that reached the sink
/// and — after the settle — whatever else did, plus the dead letters.
async fn round(with_door: bool, addressed_to: &str) -> (Message, Vec<Message>) {
    let Some((member, assistant)) = shipped() else {
        panic!("guarded by the caller");
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant, with_door);
    let (h, mut rx) = boot(&td).await;
    h.send(inject(addressed_to)).await;
    let first = recv_bounded(&mut rx)
        .await
        .expect("the round has to reach the sink -- every surface double answers");
    tokio::time::sleep(SETTLE).await;
    let mut more = Vec::new();
    while let Ok(m) = rx.try_recv() {
        more.push(m);
    }
    let dead = h.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "an answered turn must not dead-letter anywhere on the way: {:?}",
        dead.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    h.shutdown().await;
    (first, more)
}

// ══════════════════════════════════════════════════════════════ the measurements

/// The whole acceptance of #835 in one round: a turn that names no agent is
/// taken by the door, and from the door on it carries the door's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_an_agent_name_reaches_the_door() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (got, more) = round(true, "").await;

    assert_eq!(
        hop_of(&got, "route"),
        "answer",
        "the unaddressed turn is answered on the member's own answer lane: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "served_by"),
        DOOR,
        "the door is the generation that answered: {:?}",
        got.headers.hop
    );
    assert_eq!(
        (
            hop_of(&got, "saw_has_assistant"),
            hop_of(&got, "saw_assistant")
        ),
        ("1".to_string(), DOOR.to_string()),
        "the turn arrived at the door CARRYING the door's name -- the default edge \
         stamps `context.assistant`, so every guard downstream of it (the memory \
         road, the tool doors, a build result) addresses the door like any named \
         agent: {:?}",
        got.headers.hop
    );
    assert!(
        more.is_empty(),
        "exactly one generation answered; a second answer means the turn was \
         handed to both: {:?}",
        more.iter()
            .map(|m| hop_of(m, "served_by"))
            .collect::<Vec<_>>()
    );
}

/// A turn that names the other assistant reaches that one — and the door stays
/// silent, because its edge is a DEFAULT and a regular edge of the same sender
/// decided (GH #283).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_addressed_turn_reaches_the_named_assistant_and_not_the_door() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (got, more) = round(true, OTHER).await;

    assert_eq!(
        hop_of(&got, "served_by"),
        OTHER,
        "the named assistant answered: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "saw_assistant"),
        OTHER,
        "and under its own name -- the door's stamp never touched this turn"
    );
    assert!(
        more.is_empty(),
        "the door answered a turn that named somebody else -- its edge is not a \
         default, or it is not conditioned on the lane: {:?}",
        more.iter()
            .map(|m| hop_of(m, "served_by"))
            .collect::<Vec<_>>()
    );
}

/// A turn that names an agent this member does NOT have reaches the door as
/// well: no regular edge decided for it, which is the only question a default
/// edge asks. Its name is replaced by the door's on the way in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_naming_an_agent_the_member_does_not_have_reaches_the_door() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (got, more) = round(true, "gamma").await;

    assert_eq!(hop_of(&got, "served_by"), DOOR, "{:?}", got.headers.hop);
    assert_eq!(
        hop_of(&got, "saw_assistant"),
        DOOR,
        "the door's stamp replaced a name nobody in this member answers to"
    );
    assert!(more.is_empty(), "one answer, not two");
}

/// Without a door nothing changes: the unaddressed turn dies at the container as
/// `hive_no_route`, exactly as it did in every colony grown before #835.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_door_the_unaddressed_turn_is_hive_no_route() {
    let Some((member, assistant)) = shipped() else {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant, false);
    let (h, mut rx) = boot(&td).await;
    h.send(inject("")).await;

    // The dead letter IS the positive receipt here: poll the queue, bounded,
    // until it names the container.
    let container = format!("{MEMBER}/assistants");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut seen = Vec::new();
    let found = loop {
        let batch = h.drain_dead_letters().await;
        let hit = batch.iter().any(|d| {
            d.resolved_target.as_str() == container
                && matches!(d.reason, DeadLetterReason::HiveNoRoute)
        });
        seen.extend(
            batch
                .iter()
                .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code())),
        );
        if hit || tokio::time::Instant::now() >= deadline {
            break hit;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        found,
        "a turn that names no agent, in a member without a door, has to die at the \
         container as hive_no_route -- dead letters seen: {seen:?}"
    );
    assert!(
        rx.try_recv().is_err(),
        "and no generation answered it: the strict guards stay strict"
    );
    h.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════ the golden

/// The rendered door manifest IS `examples/organism/grow-member-door.json`, down
/// to the digest the builder puts on the draft.
///
/// Three statements, cheapest first: the door is ONE edge, the LAST one, spelled
/// as the issue and ruling OR-AG-25 spell it; every edge in front of it is the
/// plain level's, in order, so a door moves no index of the level's own set or
/// of the older opt-ins; and the example is byte-identical to what the renderer
/// draws — its canonical bytes hash to the renderer's own `manifest_sha256`.
#[test]
fn the_rendered_door_manifest_is_byte_identical_to_the_example() {
    let scope = "/os/orgs/acme/members/alex";
    let plain = declaration(wish(scope, "scribe", false));
    let out = render(wish(scope, "scribe", true));
    let got = out["manifest"][0].clone();

    let edges = got["diff"]["add_edges"].as_array().expect("add_edges");
    let plain_edges = plain["diff"]["add_edges"].as_array().expect("add_edges");
    assert_eq!(
        edges.len(),
        plain_edges.len() + 1,
        "the door is ONE edge beside the level's own set"
    );
    assert_eq!(
        edges.last(),
        Some(&door_edge("scribe")),
        "the door edge: a default in the container, on `in_turn` only, stamping the \
         door's name"
    );
    assert_eq!(
        &edges[..plain_edges.len()],
        plain_edges.as_slice(),
        "every edge in front of the door is the plain level's, in order"
    );
    assert_eq!(
        (&got["scope"], &got["diff"]["add_nodes"]),
        (&plain["scope"], &plain["diff"]["add_nodes"]),
        "the door is not a scope or a node: it stands where the level stands"
    );

    let path = repo(EXAMPLE);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!("{EXAMPLE} did not travel into this tree -- golden skipped (GH #49)");
        return;
    };
    let example: Value = meclaw_core::serde_json::from_str(&raw).expect("the example is json");
    let want = json!({
        "scope": example["scope"],
        "ctx": if example["ctx"].is_object() { example["ctx"].clone() } else { json!({}) },
        "diff": example["diff"],
    });
    assert_eq!(
        got, want,
        "the rendered door declaration is not the one {EXAMPLE} carries"
    );
    // The digest is sha256 over the CANONICAL bytes of the manifest list -- the
    // form `recipes` hashes (`canonical()`: keys sorted, no spaces, no ascii
    // escaping).
    let digest: String = Sha256::digest(canonical(&json!([want])).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        out["header"]["manifest_sha256"],
        json!(digest),
        "the digest a human approves for the rendered door is not the digest of \
         {EXAMPLE} -- the two are not the same bytes"
    );
    // And the file is written the way every sibling in examples/organism is: two
    // spaces, a trailing newline -- so the golden is regenerated, never edited.
    assert!(raw.ends_with("}\n"), "{EXAMPLE} ends with one newline");
}
