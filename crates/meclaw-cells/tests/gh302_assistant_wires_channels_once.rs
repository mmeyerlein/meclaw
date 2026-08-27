//! GH #302 / GH #303 — `assistant@1.0.0` wires the `channels` level ONCE.
//!
//! The level rule of this wave is *a level owns what its siblings must share*.
//! Two channels of one assistant share exactly two things: the reasoning core
//! they consult and the tool surface they call. So this level owns those two
//! and the container the channels stand in — and because that container is a
//! node of the TEMPLATE, the fan-in edges between it and its siblings are
//! internal edges of the template instead of per-instance wiring. That is
//! #303's ruling, and it is the property this file holds: a second channel
//! costs two instantiations inside `./channels` and their pairing edges, never
//! a re-run of the sibling fan-in.
//!
//! # What is asked of the FILES
//!
//! 1. **Three children, and `channels` ships empty.** A channel is
//!    instantiated, never shipped.
//! 2. **The container declares no contract and no ports** (driver ruling
//!    W7-R2). The ports half is the container convention of this wave. The
//!    contract half is sharper and is measured rather than assumed:
//!    `check_lane_doors` skips a hive only while *nobody* addresses its path
//!    (`hive_path_is_wired`), and this level addresses `./channels` on
//!    eighteen edges — so from the first instantiation every lane the container
//!    declared would owe a door to a cell INSIDE it, and an empty container has
//!    no inside. A contract here would refuse every mutation of the colony
//!    until the first channel stands there.
//! 3. **One edge to the tool surface, and it names no tool.** A single guarded
//!    default (GH #283, ruling Q1); the two consult errands stay ordinary
//!    conditioned edges. This is the #286 + #283 win, measured.
//! 4. **No unconditional tee out of `./channels`.** Suppression is per SENDER:
//!    if any regular out-edge of `./channels` fires, the default is silent. A
//!    logger, a tap or a mirror without its own route condition would take the
//!    tool surface dark for every call.
//! 5. **Every edge that discriminates on `hop.tool_name` reads the lane
//!    first** (driver ruling W7-R4, GH #286). An answer travels back through
//!    the same hive path the dispatch left from; a door that asks only about
//!    the discriminator hands an answer back to its own sender until the TTL
//!    runs out. That was observed once in the tools hive and it is the same
//!    class here.
//! 6. **The lanes are DERIVED from the occupants, and the boundary matches the
//!    member the level is instantiated into.** Both are read off the tree —
//!    `templates/talky/config.json`, `templates/cogny/config.json`,
//!    `templates/member/config.json` — rather than from a list written here.
//!    `org` and `meclaw-os` were both authored against a neighbour that did not
//!    exist yet and both guessed wrong; a list written down here would have to
//!    be re-derived by hand every time an occupant moves, which is the thing
//!    that already failed twice.
//!
//! # What is asked of the SUBSTRATE
//!
//! The shipped tree is booted with `cogny` and `tools` replaced by answering
//! `code` doubles, and with one — then two — connector + talky pairs standing
//! in `./channels`. Doubling by REPLACING a `config.json` rather than by
//! deleting a directory is the lesson of GH #286's own runtime test: a hive
//! door pointing at a directory that is not there leaves the inside unroutable,
//! and three cells that never answer are a different topology from the shipped
//! one on exactly the property under test.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `templates/assistant`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/assistant");
    p.join("config.json").is_file().then_some(p)
}

fn config_at(dir: &std::path::Path) -> Value {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `params` of one shipped hive `config.json`, as the substrate reads it.
fn hive_params(dir: &std::path::Path) -> HiveParams {
    let cfg = config_at(dir);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{}: the hive has no params", dir.display()));
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}: params: {e}", dir.display()))
}

/// The lane routes of one contract, in declaration order.
fn lanes(hp: &HiveParams) -> (Vec<String>, Vec<String>) {
    let c = hp
        .contract
        .as_ref()
        .expect("a level a caller addresses by path and lane owes a contract");
    (
        c.accepts.iter().map(|l| l.route.clone()).collect(),
        c.emits.iter().map(|l| l.route.clone()).collect(),
    )
}

/// The single `hop.route` literal a condition names, if it names exactly one.
fn stated_route(condition: Option<&str>) -> Option<String> {
    let c = condition?;
    let at = c.find("hop.route == '")?;
    let rest = &c[at + "hop.route == '".len()..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

// ───────────────────────────── (1) three children, and the container is empty

#[test]
fn the_level_ships_two_refs_and_one_empty_container() {
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
            "channels".to_string(),
            "cogny".to_string(),
            "tools".to_string()
        ],
        "an assistant owns the reasoning core, the tool surface and the container its \
         channels stand in. A fourth child is a sibling this level did not have to own — \
         a memory, a firewall and an identity all belong to the MEMBER (GH #122)."
    );

    for (name, want) in [("cogny", "cogny@"), ("tools", "tools@")] {
        let cfg = config_at(&root.join(name));
        assert_eq!(
            cfg["cell"]["type"].as_str(),
            Some("ref"),
            "templates/assistant/{name}: a level composes templates, it does not copy them"
        );
        let reference = cfg["cell"]["template"].as_str().unwrap_or_default();
        assert!(
            reference.starts_with(want),
            "templates/assistant/{name}: refers to {reference:?}"
        );
        // The pin resolves against the tree it ships in, version and all: a bare
        // name would resolve to the highest version present, which is the drift
        // `template_chain` exists to make visible.
        let (short, version) = reference.split_once('@').expect("name@version");
        let declared = repo(&format!("templates/{short}/template.json"));
        if let Ok(raw) = std::fs::read_to_string(&declared) {
            let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
            assert_eq!(
                v["version"].as_str(),
                Some(version),
                "templates/assistant/{name} pins {reference}, but the tree ships \
                 {short}@{}",
                v["version"].as_str().unwrap_or("?")
            );
        }
    }

    let grandchildren: Vec<String> = std::fs::read_dir(root.join("channels"))
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            p.is_dir()
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        grandchildren.is_empty(),
        "the container ships EMPTY — a channel is instantiated, never shipped: \
         {grandchildren:?}"
    );
}

// ───────────── (2) the container declares nothing, and the level declares no ports

/// Driver ruling **W7-R2**, and the measurement behind it.
///
/// The plan's own text called an empty container with a contract *"green as it
/// ships"*, on the strength of `check_lane_doors` skipping an unwired hive. That
/// is true only while nobody addresses the path — and this level addresses
/// `./channels` on eighteen of its own edges, so the container is wired from the
/// moment the assistant is instantiated. From then on every declared `accepts`
/// lane owes a door to a cell INSIDE the container, and an empty container has
/// no inside: the declaration would refuse EVERY mutation of the colony with
/// `hive_contract` until the first channel stood there. An assistant with no
/// channel yet is a legitimate intermediate state — Task 16's own example grows
/// the assistant and the channel in two separate declarations.
///
/// So the container declares neither, and the lanes are declared by the LEVEL,
/// whose own edges satisfy the door and the exit check from birth.
#[test]
fn the_container_declares_neither_a_contract_nor_ports() {
    let Some(root) = shipped() else { return };

    let container = config_at(&root.join("channels"));
    assert!(
        container.get("params").is_none(),
        "templates/assistant/channels carries a params block: {:?}. A container its own \
         level wires is a WIRED hive from birth (mutation::hive_contract::hive_path_is_wired), \
         so a contract here would owe a door inside an empty container and refuse every \
         later mutation of the colony. Ports would seal it and refuse the pairing edge \
         between a connector and its talky, which is the wiring this container exists for.",
        container.get("params")
    );

    let level = config_at(&root);
    assert!(
        level["params"].get("ports").is_none(),
        "templates/assistant declares params.ports = {:?}. The level is wired INTO and \
         sealing it would refuse exactly those endpoints (hive_port_boundary). No slot \
         either: a slot governs an address that does not exist, and both this level's \
         children do (unbound_slot_behaviour, colony.rs).",
        level["params"].get("ports")
    );
}

// ─────────────────────── (3) one edge to the tool surface, naming no tool at all

/// The #286 + #283 win, measured on the shipped file.
///
/// The exclusion this edge replaces named every tool on the live tree — nine
/// terms, hand-kept in sync with nine positive edges. #286 put the tool surface
/// behind one contract, which reduced it to two errands that are not tools at
/// all; #283's default edge removes the last two. So this edge names no tool and
/// no errand, and adding a tool touches nothing here.
#[test]
fn the_only_edge_to_the_tool_surface_is_one_guarded_default_naming_no_tool() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    let to_tools: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./channels" && e.to == "./tools")
        .collect();
    assert_eq!(
        to_tools.len(),
        1,
        "the N+1-edge shape of #286 reappeared at this level: {to_tools:#?}"
    );
    let tools_edge = to_tools[0];
    assert!(
        tools_edge.is_default,
        "the tool exit is the GUARDED DEFAULT of ./channels — without `default: true` it \
         is a regular edge and every consult is delivered twice, which is the defect #283 \
         measured: {tools_edge:#?}"
    );
    let cond = tools_edge.condition.as_deref().unwrap_or_default();
    assert_eq!(
        stated_route(Some(cond)).as_deref(),
        Some("tool"),
        "the default is GUARDED and the guard is the lane: {cond:?}"
    );
    assert!(
        !cond.contains("tool_name"),
        "the tools edge names a tool or an errand. The whole point of the guarded default \
         is that the two-term exclusion disappears: a consult fires a regular edge and \
         silences this one. {cond:?}"
    );
    assert_eq!(
        tools_edge
            .modifier
            .as_ref()
            .and_then(|m| m.set_hop.get("route"))
            .map(String::as_str),
        Some("'tool_call'"),
        "the default stamps the tool surface's one inbound lane: {tools_edge:#?}"
    );

    // The two consult errands: ORDINARY conditioned edges, so that firing one
    // suppresses the default for that message.
    let consults: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./channels" && e.to == "./cogny")
        .collect();
    assert_eq!(
        consults.len(),
        2,
        "consult_cogny and ask_memory: {consults:#?}"
    );
    let mut named: Vec<String> = consults
        .iter()
        .map(|e| {
            let c = e.condition.as_deref().unwrap_or_default();
            assert!(
                !e.is_default,
                "a consult edge declared `default: true` competes with the tools edge \
                 instead of suppressing it — guarded defaults do not compete, they all \
                 fire: {e:#?}"
            );
            let at = c
                .find("hop.tool_name == '")
                .expect("a consult names its errand");
            let rest = &c[at + "hop.tool_name == '".len()..];
            rest[..rest.find('\'').unwrap()].to_string()
        })
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec!["ask_memory".to_string(), "consult_cogny".to_string()]
    );
}

// ─────────────────── (4) the suppression precondition: no unconditional tee

/// Suppression is per SENDER (`crates/meclaw-colony/src/edge_table.rs`, the
/// two-phase evaluation): if ANY regular out-edge of `./channels` decided, the
/// default phase never runs. Every other edge out of `./channels` is therefore
/// conditioned on something a `tool` message does not carry — the seven outward
/// lanes, and the two errands by name.
///
/// If the authored set ever grows a logger, a tap or a mirror without its own
/// route condition, the tool surface goes dark for every call. That is the
/// requirement, and it is written into the config's own `because` next to the
/// default edge as well as here.
#[test]
fn no_regular_out_edge_of_the_channels_level_is_unconditional() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    for e in hp.graph.edges.iter().filter(|e| e.from == "./channels") {
        if e.is_default {
            continue;
        }
        let cond = e.condition.as_deref().unwrap_or_default();
        assert!(
            cond.contains("hop.route"),
            "the edge ./channels -> {} carries no route condition. Suppression is per \
             SENDER: an unconditional tee out of ./channels fires for every tool call and \
             silences the guarded default, and the tool surface goes dark. {e:#?}",
            e.to
        );
        let route = stated_route(Some(cond)).unwrap_or_default();
        assert!(
            route != "tool" || cond.contains("hop.tool_name"),
            "the edge ./channels -> {} takes the whole `tool` lane without naming an \
             errand, so no tool call ever reaches the default: {e:#?}",
            e.to
        );
    }
}

// ───────────────────── (5) W7-R4: a discriminator is never read before the lane

/// The loop class GH #286 found and driver ruling **W7-R4** closed, checked here
/// because this level has the same shape: a message that leaves `./channels` on
/// a discriminator comes back through `./channels`, and a door that reads only
/// the discriminator would dispatch an answer to its own sender, round after
/// round, until the TTL runs out.
#[test]
fn every_edge_that_discriminates_reads_the_lane_first() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    for e in &hp.graph.edges {
        let Some(cond) = e.condition.as_deref() else {
            continue;
        };
        if !cond.contains("hop.tool_name") && !cond.contains("context.tool_caller") {
            continue;
        }
        assert!(
            cond.contains("hop.route =="),
            "the edge {} -> {} discriminates without first naming the lane (W7-R4). An \
             answer travels back through the very path the dispatch left from: {e:#?}",
            e.from,
            e.to
        );
    }
}

// ────────────────────────── (6) the lanes are derived, and the boundary matches

/// A template's declared version, or `None` when it did not travel (GH #49).
fn reference(name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(repo(&format!("templates/{name}/template.json"))).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    Some(format!(
        "{}@{}",
        v["name"].as_str()?,
        v["version"].as_str()?
    ))
}

/// Every lane of this level names the occupant version it was derived from.
#[test]
fn every_lane_names_the_version_it_was_derived_from() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);
    let contract = hp.contract.as_ref().expect("params.contract");

    let pins: Vec<String> = ["talky", "cogny", "tools", "telegram-connector"]
        .iter()
        .filter_map(|n| reference(n))
        .collect();
    assert!(!pins.is_empty(), "no occupant template travelled at all");

    for l in contract.accepts.iter().chain(contract.emits.iter()) {
        assert!(
            !l.because.trim().is_empty(),
            "lane '{}' says nothing about why it crosses the level",
            l.route
        );
        // `turn` is the one lane no occupant declares: the connector emits ONE
        // wire since telegram-connector@2.0.0 and the level normalises it.
        if l.route == "turn" {
            continue;
        }
        assert!(
            pins.iter().any(|p| l.because.contains(p)),
            "lane '{}': a container level's lanes are DERIVED, and the derivation rule says \
             to name the version they were derived from. This one names none of {pins:?}: \
             {:?}",
            l.route,
            l.because
        );
    }
}

/// The derivation itself, read off the occupants rather than from a list here.
///
/// *A level declares the union of the lanes its occupants ship, minus the lanes
/// a sibling inside the level consumes itself.* `org` and `meclaw-os` were both
/// authored against a neighbour that did not exist yet and both guessed wrong —
/// `org` declared a `turn` no member ever emits while three emitted lanes died
/// silently at the boundary. Reading the occupant's file is what makes the
/// boundary a pinned fact instead of a list somebody re-derives by hand.
#[test]
fn the_level_declares_the_lanes_its_occupants_ship() {
    let Some(root) = shipped() else { return };
    let talky = repo("templates/talky");
    let cogny = repo("templates/cogny");
    if !talky.join("config.json").is_file() || !cogny.join("config.json").is_file() {
        return;
    }

    let (accepts, emits) = lanes(&hive_params(&root));
    let (talky_accepts, talky_emits) = lanes(&hive_params(&talky));
    let (_, cogny_emits) = lanes(&hive_params(&cogny));
    // The THIRD occupant. Until R6 (GH #425) the tool surface shipped no lane
    // that crossed this level — `tool_call` and `tool_result` are both consumed
    // inside it — so it was absent from the derivation and the absence looked
    // like a rule. It was a coincidence: the surface now reaches out of the
    // assistant on `build` and takes the answer back on `in_build_result`, and
    // both cross. A derivation that read two of three occupants would call the
    // level's own contract a lie.
    let tools = repo("templates/tools");
    let (tools_accepts, tools_emits) = if tools.join("config.json").is_file() {
        lanes(&hive_params(&tools))
    } else {
        (Vec::new(), Vec::new())
    };

    // Every inbound lane is one the surface really takes.
    for a in &accepts {
        assert!(
            talky_accepts.contains(a) || tools_accepts.contains(a),
            "the level accepts '{a}', which no occupant does: talky {talky_accepts:?}, \
             tools {tools_accepts:?}. An accepted lane no occupant takes is an interface \
             that lies."
        );
    }
    // Every outbound lane is one an occupant really produces — except `turn`,
    // which the level itself normalises out of the connector's one wire.
    for e in emits.iter().filter(|e| *e != "turn") {
        assert!(
            talky_emits.contains(e) || cogny_emits.contains(e) || tools_emits.contains(e),
            "the level emits '{e}', which no occupant produces: talky {talky_emits:?}, \
             cogny {cogny_emits:?}, tools {tools_emits:?}"
        );
    }

    // The subtractions, each of them a lane an occupant DOES ship and this level
    // deliberately does not — because a sibling inside consumes it.
    for gone in ["answer", "tool"] {
        assert!(
            talky_emits.contains(&gone.to_string()),
            "the subtraction of '{gone}' is stale: talky no longer emits it"
        );
        assert!(
            !emits.contains(&gone.to_string()),
            "'{gone}' is consumed INSIDE this level — `answer` by the connector on the \
             per-channel pairing edge, `tool` by ./tools through the guarded default. A \
             level that re-declared it would promise a lane whose messages never leave."
        );
    }
    assert!(
        talky_accepts.contains(&"in_tool".to_string()) && !accepts.contains(&"in_tool".to_string()),
        "`in_tool` is supplied INSIDE the level by ./tools and must not be an inbound lane \
         of the assistant: {accepts:?}"
    );
}

/// The other side of the boundary: what the member sends down must be accepted
/// here, and what this level sends up must be a lane the member routes.
///
/// Read off `templates/member/config.json`'s own edges rather than off its prose:
/// the container `<member>/assistants` carries no contract, so the member's
/// EDGES are the only statement about the boundary that the substrate itself
/// reads.
#[test]
fn the_boundary_matches_the_member_this_level_is_instantiated_into() {
    let Some(root) = shipped() else { return };
    let member = repo("templates/member");
    if !member.join("config.json").is_file() {
        return;
    }

    let (accepts, emits) = lanes(&hive_params(&root));
    let mhp = hive_params(&member);

    let sends_down: BTreeSet<String> = mhp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == "./assistants")
        .filter_map(|e| {
            e.modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(|s| s.trim_matches('\'').to_string())
                .or_else(|| stated_route(e.condition.as_deref()))
        })
        .collect();
    assert!(
        !sends_down.is_empty(),
        "the member routes nothing down into ./assistants — the boundary cannot be checked"
    );
    for lane in &sends_down {
        assert!(
            accepts.contains(lane),
            "the member sends '{lane}' down into ./assistants and this level does not \
             accept it: {accepts:?}. A lane a level does not declare is a message that \
             dies as no_route at a boundary."
        );
    }

    let takes_up: BTreeSet<String> = mhp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./assistants")
        .filter_map(|e| stated_route(e.condition.as_deref()))
        .collect();
    assert!(
        !takes_up.is_empty(),
        "the member takes nothing off ./assistants — the boundary cannot be checked"
    );
    for lane in &takes_up {
        assert!(
            emits.contains(lane),
            "the member takes '{lane}' off ./assistants and no assistant emits it: \
             {emits:?}. That is the `turn` defect of `org`, in the other direction."
        );
    }
}

// ══════════════════════════════════════════════════════ the substrate half

const ASSISTANT: &str = "/agent";
const CHANNELS: &str = "/agent/channels";
const SIBLINGS: [&str; 3] = ["/agent", "/agent/cogny", "/agent/tools"];

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// The tool surface, doubled: it answers every call on the one outward lane and
/// reports back WHICH caller the level said it was serving.
///
/// `context.tool_caller` is the only thing that tells the two callers of the one
/// tool surface apart on the way back. It is context rather than hop precisely
/// because the answer comes back through a cell, and this double is what proves
/// it survives one.
///
/// `id` is on the turn because `TurnObject.allOf` in
/// `crates/meclaw-core/schemas/ubf-body.json` makes it required for
/// `tool_call`/`tool_result`, and a turn without one dies at the first emit with
/// `InvalidUbfBody` in the DLQ.
const TOOLS: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
sys.stdout.write(json.dumps({
    "header": {"route": "tool_result", "served_by": "tools",
               "in_route": str(hop.get("route") or ""),
               "in_tool_name": str(hop.get("tool_name") or ""),
               "in_tool_caller": str(ctx.get("tool_caller") or "")},
    "messages": [{"origin": "tool", "type": "tool_result", "id": "double-tools",
                  "text": "tools"}]}))
"#;

fn tools_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": TOOLS, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["tool_result"], "required": true},
                    "served_by": {"type": "string", "required": false},
                    "in_route": {"type": "string", "required": false},
                    "in_tool_name": {"type": "string", "required": false},
                    "in_tool_caller": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test double for the tool surface of the assistant level.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The reasoning core, doubled — and it calls a tool of its own, because that is
/// the leg the `context.tool_caller` discriminator exists for. A consult
/// therefore runs the whole way: errand in, tool call out to the SHARED surface,
/// result back HERE and not to the surface that asked, advice home.
const COGNY: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "")
if route == "in_turn":
    out = {"route": "tool", "tool_name": "web_search", "served_by": "cogny"}
else:
    out = {"route": "answer", "served_by": "cogny",
           "via": str(hop.get("served_by") or ""),
           "via_caller": str(hop.get("in_tool_caller") or "")}
sys.stdout.write(json.dumps({
    "header": out,
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "cogny-1",
                  "text": "{}"}]}))
"#;

fn cogny_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": COGNY, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["tool", "answer"], "required": true},
                    "tool_name": {"type": "string", "required": false},
                    "served_by": {"type": "string", "required": false},
                    "via": {"type": "string", "required": false},
                    "via_caller": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test double for the reasoning core of the assistant level.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The per-channel wiring one `add_nodes` mutation draws, as edges of the colony
/// root — deep endpoints, so the shipped `channels/config.json` stays untouched.
///
/// It is the recipe the README states, and every endpoint of it is BELOW
/// `<assistant>/channels`: that is why the container has to stay open, and why
/// none of these edges is an edge between `./channels` and a sibling.
fn channel_edges(tag: &str) -> Vec<Value> {
    let connector = format!("./agent/channels/{tag}-conn");
    let talky = format!("./agent/channels/{tag}-talky");
    let hive = "./agent/channels";
    let mut out = vec![
        // the connector's ONE wire, normalised by the level the connector sits in
        json!({"from": connector, "to": hive,
               "condition": "!has(hop.error_code)",
               "modifier": {"set_hop": {"route": "'turn'"},
                            "set_context": {"channel": "'test'"}}}),
        json!({"from": connector, "to": hive,
               "condition": "has(hop.error_code)",
               "modifier": {"set_hop": {"route": "'error'"}}}),
        // the pairing edge: the talky's answer is the connector's inbound
        json!({"from": talky, "to": connector,
               "condition": "has(hop.route) && hop.route == 'answer'"}),
    ];
    // the level's inbound lanes, carried down to the talky
    for lane in [
        "in_turn",
        "in_bundle",
        "in_advice",
        "in_sweep",
        "in_prune",
        "in_round_sweep",
        "in_tool",
    ] {
        out.push(json!({"from": hive, "to": talky,
                        "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    // the talky's outbound lanes, up to the level
    for lane in [
        "write",
        "turn_write",
        "extraction",
        "recall",
        "tool",
        "prune",
        "error",
    ] {
        out.push(json!({"from": talky, "to": hive,
                        "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    out
}

/// The colony around the assistant: one door in on `in_turn`, and a drain for
/// every lane the level emits.
fn main_config(tags: &[&str]) -> Value {
    let mut edges = vec![json!({"from": "./driver", "to": "./agent",
                                "condition": "has(hop.route) && hop.route == 'in_turn'"})];
    for lane in [
        "turn",
        "write",
        "turn_write",
        "extraction",
        "recall",
        "prune",
        "error",
    ] {
        edges.push(json!({"from": "./agent", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    for tag in tags {
        edges.extend(channel_edges(tag));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// Stands in for the member's firewall on the way in: it hands the assistant one
/// screened turn and carries the errand name on the hop.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "in_turn",
               "tool_name": str(hop.get("tool_name") or "")},
    "messages": doc["body"].get("messages", [])}))
"#;

fn driver_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": DRIVER, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["in_turn"], "required": true},
                    "tool_name": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the member's firewall handing a screened turn down.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A talky double: it takes the screened turn and emits whatever the hop asks
/// for — a tool call, a consult errand, or a plain answer.
const TALKY: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
name = str(hop.get("tool_name") or "")
route = str(hop.get("route") or "")
if route == "in_turn" and name:
    out = {"route": "tool", "tool_name": name, "consult_id": "c-1", "served_by": "talky"}
elif route == "in_advice" or route == "in_tool":
    out = {"route": "answer", "served_by": "talky", "in_route": route,
           "in_served_by": str(hop.get("served_by") or ""),
           "via_caller": str(hop.get("via_caller") or hop.get("in_tool_caller") or "")}
else:
    out = {"route": "answer", "served_by": "talky", "in_route": route}
sys.stdout.write(json.dumps({
    "header": out,
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                  "text": "{}"}]}))
"#;

fn talky_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": TALKY, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["tool", "answer"], "required": true},
                    "tool_name": {"type": "string", "required": false},
                    "consult_id": {"type": "string", "required": false},
                    "served_by": {"type": "string", "required": false},
                    "in_route": {"type": "string", "required": false},
                    "in_served_by": {"type": "string", "required": false},
                    "via_caller": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test double for the talky standing in an assistant's channels level.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The connector double: it takes the finished answer and reports it upward as
/// an inbound turn, which is what the shipped connector's one wire looks like.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"served_by": "connector",
               "in_served_by": str(hop.get("in_served_by") or ""),
               "via_caller": str(hop.get("via_caller") or ""),
               "in_route": str(hop.get("route") or "")},
    "messages": [{"origin": "user", "type": "text", "text": "hi"}]}))
"#;

fn connector_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": CONNECTOR, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "served_by": {"type": "string", "required": false},
                    "in_served_by": {"type": "string", "required": false},
                    "via_caller": {"type": "string", "required": false},
                    "in_route": {"type": "string", "required": false},
                    "error_code": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test double for the connector standing in an assistant's channels level.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Build the tree: the SHIPPED assistant, with the two `ref` markers replaced by
/// answering `code` doubles and one connector + talky pair per tag standing in
/// `./channels` — which is what the per-channel mutation stages there.
fn build_tree(td: &tempfile::TempDir, source: &std::path::Path, tags: &[&str]) {
    let root = td.path();
    write(root, "main/config.json", &main_config(tags));
    write(root, "main/driver/config.json", &driver_cell());
    copy_cells(source, &root.join("main/agent"));
    write(root, "main/agent/cogny/config.json", &cogny_cell());
    write(root, "main/agent/tools/config.json", &tools_cell());
    for tag in tags {
        write(
            root,
            &format!("main/agent/channels/{tag}-conn/config.json"),
            &connector_cell(),
        );
        write(
            root,
            &format!("main/agent/channels/{tag}-talky/config.json"),
            &talky_cell(),
        );
    }
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

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One screened turn, with the name of the tool or errand the surface will call.
fn turn(tool_name: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("tool_name".into(), json!(tool_name));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "hi"}
        ]})))
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

/// A real tool call takes the guarded default to `./tools` and nothing else; a
/// consult errand takes the regular edge to `./cogny` and the default stays
/// silent.
///
/// The round is followed the whole way, which is also assertion (b) of the
/// task: the screened turn goes in on `in_turn`, reaches the talky, the answer
/// reaches the connector on the per-channel pairing edge, and what leaves the
/// assistant is a `turn` on its way to the member's screen. `in_served_by` is a
/// POSITIVE receipt carried through: every double answers, so a second occupant
/// being served would be a second message rather than a silence to be waited
/// out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_takes_the_default_and_a_consult_takes_the_regular_edge() {
    let Some(source) = shipped() else { return };

    for (name, want_served, want_caller) in [
        ("web_search", "tools", "channels"),
        ("consult_cogny", "cogny", "cogny"),
        ("ask_memory", "cogny", "cogny"),
    ] {
        let td = tempfile::TempDir::new().unwrap();
        build_tree(&td, &source, &["a"]);
        let (h, mut sink_rx) = boot(&td).await;

        h.send(turn(name)).await;
        let out = recv_bounded(&mut sink_rx)
            .await
            .unwrap_or_else(|| panic!("{name}: the round never left the assistant"));

        assert_eq!(
            hop_of(&out, "in_served_by"),
            want_served,
            "{name}: the call was served by the wrong sibling — hop {:?}",
            out.headers.hop
        );
        assert_eq!(
            hop_of(&out, "route"),
            "turn",
            "{name}: what leaves an assistant after a channel produced it is a `turn` on \
             its way to the member's screen — hop {:?}",
            out.headers.hop
        );
        assert_eq!(
            hop_of(&out, "served_by"),
            "connector",
            "{name}: the answer did not reach the connector on the per-channel pairing \
             edge — hop {:?}",
            out.headers.hop
        );
        // The two callers of the ONE tool surface, told apart on the way back by
        // `context.tool_caller`. It is context and not hop because the answer
        // comes back through a cell, and a hop key would not survive it. A
        // broken discriminator shows up here as the wrong `in_served_by`: the
        // core's own tool result would land at the surface instead.
        assert_eq!(
            hop_of(&out, "via_caller"),
            want_caller,
            "{name}: the tool surface answered the wrong caller — hop {:?}",
            out.headers.hop
        );

        // ONE round, ONE turn out. A second message here is a second delivery of
        // the same call — the guarded default firing beside a consult, or the
        // tool surface answering both callers because the discriminator stopped
        // discriminating. Every double answers, so a second delivery is a second
        // message rather than a silence to be waited out; the wait exists only
        // to let one that is already in flight land.
        tokio::time::sleep(Duration::from_millis(750)).await;
        if let Ok(extra) = sink_rx.try_recv() {
            panic!(
                "{name}: the assistant produced a SECOND turn for one call — hop {:?}",
                extra.headers.hop
            );
        }

        assert!(
            h.drain_dead_letters().await.is_empty(),
            "{name}: a served round must not dead-letter on the way"
        );
        h.shutdown().await;
    }
}

/// The tool surface is reached exactly once per call: the guarded default and
/// the two consult edges never fire together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_never_reaches_the_tool_surface() {
    let Some(source) = shipped() else { return };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &source, &["a"]);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(turn("consult_cogny")).await;
    let first = recv_bounded(&mut sink_rx)
        .await
        .expect("the consult is answered");
    assert_eq!(
        hop_of(&first, "in_served_by"),
        "cogny",
        "hop {:?}",
        first.headers.hop
    );

    // Every edge out of `./channels` is decided in ONE `apply_edges` call on ONE
    // message, so a competing delivery is already in flight by the time the
    // winning one arrives. The wait exists only to let it land.
    tokio::time::sleep(Duration::from_millis(750)).await;
    if let Ok(second) = sink_rx.try_recv() {
        panic!(
            "the guarded default fired beside the consult edge — the tool surface was \
             served a consult: hop {:?}",
            second.headers.hop
        );
    }

    assert!(h.drain_dead_letters().await.is_empty());
    h.shutdown().await;
}

/// #303's acceptance, measured: a second channel adds only its own pairing
/// edges. The edges between `./channels` and its siblings are the template's,
/// drawn once, and their number does not move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_channel_adds_no_edge_between_the_channels_level_and_its_siblings() {
    let Some(source) = shipped() else { return };

    async fn sibling_edges(source: &std::path::Path, tags: &[&str]) -> (usize, usize) {
        let td = tempfile::TempDir::new().unwrap();
        build_tree(&td, source, tags);
        let (h, _rx) = boot(&td).await;
        let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
        h.inbox_tx
            .send(ColonyMsg::ReadGraph {
                scope: Path::new("/"),
                ack: ack_tx,
            })
            .await
            .unwrap();
        let graph = ack_rx.await.unwrap();
        let between = graph
            .edges
            .iter()
            .filter(|e| {
                (e.from.as_str() == CHANNELS && SIBLINGS.contains(&e.to.as_str()))
                    || (e.to.as_str() == CHANNELS && SIBLINGS.contains(&e.from.as_str()))
            })
            .count();
        let inside = graph
            .edges
            .iter()
            .filter(|e| {
                e.from.as_str().starts_with(&format!("{CHANNELS}/"))
                    || e.to.as_str().starts_with(&format!("{CHANNELS}/"))
            })
            .count();
        h.shutdown().await;
        (between, inside)
    }

    let (one_between, one_inside) = sibling_edges(&source, &["a"]).await;
    let (two_between, two_inside) = sibling_edges(&source, &["a", "b"]).await;

    // The template's own count, read off the file, so the runtime number and the
    // shipped number are the same statement.
    let hp = hive_params(&source);
    let declared = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./channels" || e.to == "./channels")
        .count();
    assert_eq!(
        one_between, declared,
        "the live tree draws a different number of channels-to-sibling edges than the \
         template declares"
    );
    assert_eq!(
        one_between, two_between,
        "a second channel moved the fan-in between ./channels and its siblings from \
         {one_between} to {two_between}. #303's whole ruling is that this number is a \
         property of the TEMPLATE and is drawn once."
    );
    assert_eq!(
        two_inside,
        one_inside * 2,
        "a second channel costs exactly its own pairing edges: {one_inside} -> {two_inside}"
    );
    assert_eq!(
        one_between, 18,
        "the measured fan-in changed. #303 counted 14 on the live tree (the core, four \
         tool cells, the drain, the sink and the assistant itself); this template draws \
         {one_between} for reasons the README states. Move the README with the number."
    );
    // The level really is the assistant's whole address space.
    assert!(
        SIBLINGS.contains(&ASSISTANT),
        "the assistant path itself is one of the siblings the container is wired to"
    );
}

/// GH #425 — the reach of the tool surface crosses this level, in both
/// directions, and is therefore declared here.
///
/// ADR-0013 § Consequences: *"A level declares the union of its occupants' lanes
/// that CROSS it. An emit lane crosses by definition. An accepts lane crosses
/// only when its producer sits outside the level and addresses through it."*
/// `build` leaves `./tools` and no sibling inside this level consumes it →
/// crosses. `in_build_result` comes from four levels up and addresses THROUGH
/// this level → crosses. Both fall out of the rule rather than out of a
/// decision — and both are derived from `templates/tools/config.json`, off the
/// tree, so a lane that moves in the occupant goes red HERE instead of losing a
/// message to `no_route`.
#[test]
fn the_level_carries_the_reach_of_its_tool_surface() {
    let Some(root) = shipped() else { return };
    let tools = repo("templates/tools");
    if !tools.join("config.json").is_file() {
        return;
    }
    let (tools_accepts, tools_emits) = lanes(&hive_params(&tools));
    let (accepts, emits) = lanes(&hive_params(&root));

    let out = "build".to_string();
    assert!(
        tools_emits.contains(&out),
        "tools no longer emits {out}: {tools_emits:?}"
    );
    assert!(
        emits.contains(&out),
        "the assistant does not carry {out}: {emits:?}"
    );

    let back = "in_build_result".to_string();
    assert!(
        tools_accepts.contains(&back),
        "tools no longer accepts {back}: {tools_accepts:?}"
    );
    assert!(
        accepts.contains(&back),
        "the assistant does not carry {back}: {accepts:?}"
    );
}
