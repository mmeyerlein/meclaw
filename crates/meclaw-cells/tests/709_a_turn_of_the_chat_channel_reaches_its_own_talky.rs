//! GH #709 — one talky per channel: a turn of the channel `chat` reaches the talky that
//! serves `chat`, and a spoken turn reaches the other one.
//!
//! `display-hive.md` § 8.2 and `meclaw-next/archive/03-os-structure.md` ("One talky per
//! channel; one identity, one tonality, one personality per channel") both say the same
//! thing, and until `assistant@2.7.0` the level had one talky for every channel a person
//! is reached on. `./talky-chat` is the second, and it is deliberately the SAME ref onto
//! the same template with the same overrides: one identity, one tone. What it does not
//! share is the one thing a talky owns, its sessions — which is the point of the rule
//! and the price of it.
//!
//! # What is measured
//!
//! **On the file.** Four children instead of three, the fourth a ref onto the talky the
//! tree ships, with the overrides of its sibling word for word. The split conditions,
//! written out. And the symmetry: every edge drawn around `./talky` has a counterpart
//! around `./talky-chat`, derived from the file rather than listed here — a twin that
//! goes missing is a lane the chat keeper never hears, and the ones that would go
//! missing quietly are the housekeeping lanes nobody drives in a test.
//!
//! **On a booted colony,** with all four refs replaced by answering doubles: a turn
//! stamped `context.channel_node == 'chat'` is served by `talky-chat`; one stamped
//! `voice`, and one stamped nothing at all, by `talky`. And a tool round started by
//! `talky-chat` comes back to `talky-chat`: the discriminator is `context.tool_caller`,
//! exactly as it already was for the core, because an answer travels back through the
//! path the call left from (W7-R4).
//!
//! Guarded like every other template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/assistant");
    p.join("config.json").is_file().then_some(p)
}

fn config_at(dir: &std::path::Path) -> Value {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn hive_params(dir: &std::path::Path) -> HiveParams {
    let cfg = config_at(dir);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{}: the hive has no params", dir.display()));
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}: params: {e}", dir.display()))
}

// ══════════════════════════════════════════════════════ 1. the fourth child

#[test]
fn the_level_holds_a_second_talky_and_it_is_the_same_talky() {
    let Some(root) = shipped() else { return };

    let mut children: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.is_dir() && p.join("config.json").is_file())
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    children.sort();
    assert_eq!(
        children,
        vec![
            "cogny".to_string(),
            "talky".to_string(),
            "talky-chat".to_string(),
            "tools".to_string()
        ],
        "one talky per channel that asks for its own (§ 8.2). The fourth child is not a \
         fourth KIND of occupant: it is a second node onto the template `./talky` already \
         references"
    );

    let spoken = config_at(&root.join("talky"));
    let typed = config_at(&root.join("talky-chat"));
    assert_eq!(
        typed["cell"], spoken["cell"],
        "the same ref at the same version. A chat that ran a different model would be a \
         different assistant to the same person"
    );
    assert_eq!(
        typed["override_params"], spoken["override_params"],
        "one identity, one tone (03-os-structure, 'the advisor pattern'): the model, the \
         declared tools and the memory tier are the LEVEL's decisions, so they are \
         repeated here word for word rather than varied"
    );
}

// ════════════════════════════════════════════════════ 2. the split, in the file

/// The condition of the one edge from `from` to `to` on this lane, panicking when there
/// is not exactly one.
fn edge<'a>(hp: &'a HiveParams, from: &str, to: &str, lane: &str) -> &'a EdgeSpec {
    let hits: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| {
            e.from == from
                && e.to == to
                && e.condition
                    .as_deref()
                    .unwrap_or_default()
                    .contains(&format!("hop.route == '{lane}'"))
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one {from} -> {to} edge on `{lane}`: {hits:#?}"
    );
    hits[0]
}

#[test]
fn a_turn_is_split_on_the_channel_and_a_tool_round_on_its_caller() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    for lane in ["in_turn", "in_advice"] {
        let spoken = edge(&hp, ".", "./talky", lane);
        let typed = edge(&hp, ".", "./talky-chat", lane);
        assert!(
            spoken
                .condition
                .as_deref()
                .unwrap_or_default()
                .contains("(!has(context.channel_node) || context.channel_node != 'chat')"),
            "the spoken door has to EXCLUDE the chat, or a typed turn is delivered twice \
             and answered twice: {spoken:#?}"
        );
        assert!(
            typed
                .condition
                .as_deref()
                .unwrap_or_default()
                .contains("has(context.channel_node) && context.channel_node == 'chat'"),
            "the typed door reads the stamp the member's own channel edge wrote: {typed:#?}"
        );
    }

    // The tool round back. `talky-chat` is excluded from the spoken doors the same way
    // `cogny` already was, and gets doors of its own: an answer travels back through the
    // path the call left from (W7-R4), and a door that reads only the lane hands the
    // chat keeper's tool result to the voice keeper.
    for (from, lane) in [("./tools", "tool_result"), ("./tools", "tool_schemas")] {
        let spoken = edge(&hp, from, "./talky", lane);
        let cond = spoken.condition.as_deref().unwrap_or_default().to_string();
        assert!(
            cond.contains("context.tool_caller != 'talky-chat'"),
            "{from} -> ./talky on `{lane}` still takes everything that is not the core: \
             {cond}"
        );
        let typed = edge(&hp, from, "./talky-chat", lane);
        assert!(
            typed
                .condition
                .as_deref()
                .unwrap_or_default()
                .contains("context.tool_caller == 'talky-chat'"),
            "{typed:#?}"
        );
    }
    for to in ["./tools", "./cogny"] {
        let e = edge(&hp, "./talky-chat", to, "schemas");
        assert_eq!(
            e.modifier
                .as_ref()
                .and_then(|m| m.set_context.get("tool_caller"))
                .map(String::as_str),
            Some("'talky-chat'"),
            "the chat keeper signs its own request, or the menu comes back to the other \
             one: {e:#?}"
        );
    }
}

/// **The symmetry, derived rather than listed.**
///
/// Every edge around `./talky` has a counterpart around `./talky-chat` on the same lane,
/// with the same other end. The lanes that would go missing quietly are exactly the ones
/// no test drives — a sweep, a prune, an export — and a keeper whose sessions are never
/// swept is a store that grows until the disk does.
#[test]
fn every_edge_around_the_one_talky_has_a_twin_around_the_other() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    /// `(direction, other end, lane)` of one edge at `node`, or `None` when it is not
    /// this node's edge.
    fn rim(e: &EdgeSpec, node: &str) -> Option<(&'static str, String, String)> {
        let lane = e
            .condition
            .as_deref()
            .unwrap_or_default()
            .split("hop.route == '")
            .nth(1)
            .and_then(|r| r.split('\'').next())
            .unwrap_or_default()
            .to_string();
        if e.to == node {
            Some(("in", e.from.clone(), lane))
        } else if e.from == node {
            Some(("out", e.to.clone(), lane))
        } else {
            None
        }
    }
    let side = |node: &str| -> Vec<(&'static str, String, String)> {
        let mut v: Vec<_> = hp
            .graph
            .edges
            .iter()
            .filter_map(|e| rim(e, node))
            .map(|(d, other, lane)| {
                // The other end is read as a ROLE, so the two rims compare: the chat
                // keeper's neighbour on a lane is `./talky-chat` where the spoken one's
                // is `./talky`, and everything else is the same node.
                (d, other.replace("./talky-chat", "./talky"), lane)
            })
            .collect();
        v.sort();
        v
    };
    // The ONE declared exception, and it is named rather than tolerated (GH #709). The
    // transfer lanes address a keeper by the name of its HIVE and not of its node:
    // `templates/session-keeper/porter/config.json` writes into `<dest>/session-keeper`
    // and says why -- *"a member names ONE generation per export, because two keepers
    // would otherwise both claim `session-keeper` and the directory would hold whichever
    // walk finished last"* -- and `templates/member/config.json` addresses an import with
    // `hop.import_hive == 'session-keeper'`, which BOTH keepers answer to. Two keepers
    // under one generation therefore collide on the way out and cannot be told apart on
    // the way in; the measured result was an export of one schema header and no row.
    // Making them distinguishable is a `session-keeper` change -- a per-node directory
    // and a per-node import address -- not an `assistant` one, so until then the typed
    // keeper takes no part in transfer and its sessions do not travel. Said out loud
    // here, in the README and in the CHANGELOG rather than found as an empty export.
    const NO_TRANSFER: [&str; 4] = ["in_export", "in_import", "export_done", "dump"];
    let spoken: Vec<_> = side("./talky")
        .into_iter()
        .filter(|(_, _, lane)| !NO_TRANSFER.contains(&lane.as_str()))
        .collect();
    assert_eq!(
        spoken,
        side("./talky-chat"),
        "outside the transfer lanes the two keepers carry the same rim. Every sweep, \
         prune and mutation receipt fans out to BOTH, because each of them has its own \
         sessions to tidy, and everything either of them says leaves the level the same way"
    );
    for lane in NO_TRANSFER {
        assert!(
            !side("./talky-chat").iter().any(|(_, _, l)| l == lane),
            "`{lane}` reaches the typed keeper. Both keepers then claim the directory \
             `session-keeper` on the way out and both answer `import_hive == \
             'session-keeper'` on the way in. Give the porter a per-node directory and a \
             per-node import address FIRST, in `session-keeper`, then draw these four"
        );
    }
    assert_eq!(
        hp.graph.edges.len(),
        61,
        "thirty-eight edges and twenty-three twins. The number is asserted so that an \
         edge added on one side and forgotten on the other is loud"
    );
}

#[test]
fn every_connect_point_names_both_rims() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);
    let contract = hp.contract.as_ref().expect("params.contract");
    let mut named = Vec::new();
    for l in contract.accepts.iter().chain(contract.emits.iter()) {
        let at = &l.at;
        if at.is_empty() {
            continue;
        }
        if at.iter().any(|a| a == "./talky") {
            named.push(l.route.clone());
            assert!(
                at.iter().any(|a| a == "./talky-chat"),
                "lane '{}' lets a v-lane dock on the spoken keeper and not on the typed \
                 one: the chat keeper would then have no identity pack and no memory leg, \
                 and a connect point is the only thing that would have permitted one \
                 (ADR-0020): {at:?}",
                l.route
            );
        }
    }
    named.sort();
    assert_eq!(
        named,
        vec![
            "in_bundle",
            "in_pack",
            "pack_ack",
            "recall",
            "schemas",
            "tool"
        ],
        "the six lanes that dock on a brain rim"
    );
}

// ═══════════════════════════════════════════════════════════════ 3. the colony

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

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

/// A keeper, doubled: it says who it is, and starts a tool round when the turn names one.
const KEEPER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
me = str((doc.get("params") or {}).get("who") or "?")
route = str(hop.get("route") or "")
name = str(hop.get("tool_name") or "")
if route == "in_turn" and name:
    out = {"route": "tool", "tool_name": name, "served_by": me}
else:
    out = {"route": "answer", "served_by": me, "in_route": route}
sys.stdout.write(json.dumps({
    "header": out,
    "messages": [{"origin": "assistant", "type": "text", "text": "ok"}]}))
"#;

/// The tool surface, doubled: it answers whatever it was called with.
const TOOLS: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "tool_result"},
    "messages": [{"origin": "tool", "type": "tool_result", "id": "t-1", "text": "{}"}]}))
"#;

/// The reasoning core, doubled: inert, this round never consults it.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// Puts one screened turn on the level's door, carrying the channel and the errand.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
# `want_channel` is copied onto the OWN emission on purpose: a hop is per-message and
# dies with the emission that carried it, so a driver that only read it here would
# hand the level an unstamped turn and the edge below would find nothing to read.
sys.stdout.write(json.dumps({
    "header": {"route": "in_turn", "tool_name": str(hop.get("tool_name") or ""),
               "want_channel": str(hop.get("want_channel") or "")},
    "messages": doc["body"].get("messages") or []}))
"#;

fn cell(script: &str, params: Value, routes: Value, purpose: &str) -> Value {
    let mut p = json!({"runner": "python3", "script_inline": script, "external_timeout_ms": 10000});
    if let Some(extra) = params.as_object() {
        for (k, v) in extra {
            p[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": p,
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": routes, "required": true},
                    "tool_name": {"type": "string", "required": false},
                    "want_channel": {"type": "string", "required": false},
                    "served_by": {"type": "string", "required": false},
                    "in_route": {"type": "string", "required": false}
                }
            },
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

fn main_config() -> Value {
    let mut edges = vec![json!({
        "from": "./driver", "to": "./agent",
        "condition": "has(hop.route) && hop.route == 'in_turn'",
        "modifier": {"set_context": {
            "channel_node": "has(hop.want_channel) ? hop.want_channel : ''"
        }}
    })];
    for lane in [
        "answer",
        "write",
        "turn_write",
        "sidecar",
        "recall",
        "prune",
        "error",
        "build",
        "tool",
        "schemas",
    ] {
        edges.push(json!({"from": "./agent", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, source: &std::path::Path) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &cell(
            DRIVER,
            json!({}),
            json!(["in_turn"]),
            "Test stand-in for the member's screening handing a turn down.",
        ),
    );
    copy_cells(source, &root.join("main/agent"));
    for who in ["talky", "talky-chat"] {
        write(
            root,
            &format!("main/agent/{who}/config.json"),
            &cell(
                KEEPER,
                json!({"who": who}),
                json!(["answer", "tool"]),
                "Test double for one of the level's conversation surfaces.",
            ),
        );
    }
    write(
        root,
        "main/agent/tools/config.json",
        &cell(
            TOOLS,
            json!({}),
            json!(["tool_result"]),
            "Test double for the level's tool surface.",
        ),
    );
    write(
        root,
        "main/agent/cogny/config.json",
        &cell(
            INERT,
            json!({}),
            json!(["answer"]),
            "Inert double for the reasoning core, which this round never reaches.",
        ),
    );
    std::fs::write(root.join(".env"), "").unwrap();
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
        .expect("the shipped assistant tree must boot");
    (h, sink_rx)
}

fn turn(channel: &str, tool: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("want_channel".into(), json!(channel));
    hop.insert("tool_name".into(), json!(tool));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": "hallo"}]}),
        ))
        .hop(hop)
        .ttl(200)
        .build()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("assistant did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

/// One round: a turn on a channel, and who answered it.
async fn served_by(channel: &str, tool: &str) -> Message {
    let source = shipped().expect("guarded by the caller");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &source);
    let (h, mut rx) = boot(&td).await;
    h.send(turn(channel, tool)).await;
    let got = recv_bounded(&mut rx)
        .await
        .expect("the round has to reach the sink -- both keepers answer");
    h.shutdown().await;
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_turn_is_served_by_the_chat_keeper_and_a_spoken_one_is_not() {
    if skip() {
        return;
    }
    assert_eq!(
        hop_of(&served_by("chat", "").await, "served_by"),
        "talky-chat",
        "§ 8.2: the channel `chat` has its own talky, and the stamp the member's channel \
         edge wrote is what finds it"
    );
    assert_eq!(
        hop_of(&served_by("voice", "").await, "served_by"),
        "talky",
        "a spoken turn keeps the keeper it always had. Nothing about the voice road moves \
         when a second channel grows a voice of its own"
    );
    assert_eq!(
        hop_of(&served_by("", "").await, "served_by"),
        "talky",
        "a turn with no channel stamp at all is served by the spoken keeper: the chat \
         door is the POSITIVE one, so an unstamped turn is never orphaned"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_round_comes_back_to_the_keeper_that_started_it() {
    if skip() {
        return;
    }
    let got = served_by("chat", "web_search").await;
    assert_eq!(
        hop_of(&got, "served_by"),
        "talky-chat",
        "the tool result came back to the wrong keeper. The discriminator is \
         `context.tool_caller`, which the outbound edge stamps: an answer travels back \
         through the path the call left from (W7-R4)"
    );
    assert_eq!(
        hop_of(&got, "in_route"),
        "in_tool",
        "and it came back on the tool lane, not as a fresh turn"
    );
}
