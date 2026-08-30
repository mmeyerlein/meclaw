//! GH #302 / GH #303 / GH #454 — `assistant@2.0.0` ships its fan-in ONCE, and
//! the channels are not part of it any more.
//!
//! The level rule of this wave is *a level owns what its siblings must share*.
//! GH #454 read it one step further and moved the CHANNELS out of this level
//! entirely. A bot a generation owns is one agent's bot: a second agent of the
//! same person is not reachable through it, a generation swap takes the chat
//! account with it, and a screen that two of a person's agents both draw on has
//! no owner at all. So a channel belongs to the PERSON. It stands in
//! `<member>/channels`, and this level is what a channel ADDRESSES rather than
//! what contains one — *a member with two assistants sharing one channel* is
//! the shape the move exists for.
//!
//! In the file that is one substitution and one lane swap. The open `channels`
//! container became `surface`, a single `ref` on the talky that keeps this
//! generation's sessions; the emit `turn` was removed and the emit `answer`
//! added. Both fall out of the derivation rule rather than out of a decision:
//! the raw wire that produced `turn` sits outside this level now and reaches
//! the member's screen without crossing it, and the `answer` a connector used
//! to consume INSIDE this level has no consumer here any more, so it crosses.
//! Taking a lane and an address away is the first digit.
//!
//! The name of this file still says what it holds, only more strongly than it
//! did: the fan-in edges between the occupants are internal edges of the
//! TEMPLATE, drawn once and shipped with it. A second channel used to cost two
//! instantiations inside `./channels` plus their pairing edges; since 2.0.0 it
//! costs this level nothing whatsoever, because there is nothing here to add it
//! to.
//!
//! # What is asked of the FILES
//!
//! 1. **Three children, all `ref`s, and no container at all.** `surface`,
//!    `cogny` and `tools`, each pinning the exact version the tree ships — a
//!    bare `<name>` resolves to the highest one present, which is the drift
//!    `template_chain` exists to make visible.
//! 2. **No open container is left on this level**, and `surface` is a ref on
//!    the talky version standing in the tree. The container that used to be
//!    here is the subject GH #454 removed: a channel is instantiated into the
//!    MEMBER now, so there is nothing at this level for a mutation to put one
//!    into, and the level is complete at birth.
//! 3. **One edge to the tool surface, and it names no tool.** A single guarded
//!    default (GH #283, ruling Q1); the two consult errands stay ordinary
//!    conditioned edges. This is the #286 + #283 win, measured.
//! 4. **No unconditional tee out of `./surface`.** Suppression is per SENDER:
//!    if any regular out-edge of `./surface` fires, the default is silent. A
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
//! The shipped tree is booted with all three refs replaced by answering `code`
//! doubles. Doubling by REPLACING a `config.json` rather than by deleting a
//! directory is the lesson of GH #286's own runtime test: a hive door pointing
//! at a directory that is not there leaves the inside unroutable, and three
//! cells that never answer are a different topology from the shipped one on
//! exactly the property under test.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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

/// The three occupants of the level, as this template addresses them. There is
/// no fourth endpoint: what used to be `./channels` is a node of the MEMBER now.
const SIBLINGS: [&str; 3] = ["./surface", "./cogny", "./tools"];

// ─────────────────────────────── (1) three children, all refs, no container

#[test]
fn the_level_ships_three_refs_and_no_container() {
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
            "surface".to_string(),
            "tools".to_string()
        ],
        "one generation is the conversation surface, the reasoning core and the tool \
         surface — three refs and nothing else. A fourth child is a sibling this level did \
         not have to own: a memory, a firewall and an identity belong to the MEMBER \
         (GH #122), and since GH #454 so do the CHANNELS, which is why the container that \
         used to stand here is gone rather than empty."
    );

    for (name, want) in [
        ("surface", "talky@"),
        ("cogny", "cogny@"),
        ("tools", "tools@"),
    ] {
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
}

// ────────── (2) nothing on this level is a container, and the surface is a talky

/// What GH #454 took away, measured on the file rather than assumed from prose.
///
/// The old shape was two refs plus one OPEN container the channels were
/// instantiated into, and the sharpest fact about that container was that it
/// could declare neither a contract nor ports: this level addressed `./channels`
/// on eighteen of its own edges, so `check_lane_doors` saw a WIRED hive from
/// birth (`hive_path_is_wired`), every declared lane owed a door to a cell
/// INSIDE it, and an empty container has no inside.
///
/// That whole argument lost its subject. A channel is the person's now, it is
/// instantiated into `<member>/channels`, and this level has nowhere to put one
/// — which is the point: the assistant is COMPLETE at birth, and no per-channel
/// follow-up mutation exists any more. So what is measured here is the absence
/// itself: no child of this level is a hive, and `surface` is a ref on the talky
/// version standing in the tree, read off `templates/talky/template.json` rather
/// than written down here, because a pin that ages inside a test goes red for a
/// reason that has nothing to do with what the test is about.
///
/// The level itself stays OPEN — it is wired INTO, and sealing it would refuse
/// exactly the endpoints the member's container needs.
#[test]
fn the_level_carries_no_open_container_and_the_surface_is_the_shipped_talky() {
    let Some(root) = shipped() else { return };

    for entry in std::fs::read_dir(&root).unwrap() {
        let p = entry.unwrap().path();
        if !p.is_dir() || !p.join("config.json").is_file() {
            continue;
        }
        let cfg = config_at(&p);
        assert_ne!(
            cfg["cell"]["type"].as_str(),
            Some("hive"),
            "{}: a hive child of this level is a CONTAINER — something a mutation puts a \
             node into. GH #454 took the only one this level had: a channel belongs to the \
             person, so a generation has nothing left to be filled with and is complete at \
             birth.",
            p.display()
        );
    }
    assert!(
        !root.join("channels").exists(),
        "templates/assistant/channels is back. A bot a generation owns is one agent's bot: \
         the person's second agent cannot be reached through it and a shared screen has no \
         owner — the container belongs to the member (GH #454)."
    );

    // `surface` is a ref on the talky the tree actually ships.
    let surface = config_at(&root.join("surface"));
    let talky = repo("templates/talky/template.json");
    if let Ok(raw) = std::fs::read_to_string(&talky) {
        let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
        let want = format!(
            "{}@{}",
            v["name"].as_str().unwrap_or("talky"),
            v["version"].as_str().unwrap_or_default()
        );
        assert_eq!(
            surface["cell"]["template"].as_str(),
            Some(want.as_str()),
            "templates/assistant/surface must pin the talky standing in this tree. It is \
             the one surface of this generation: what used to be one talky per channel is \
             one per assistant, with the channels talking to it from outside."
        );
    }

    // Taking a documented address away is the FIRST digit — the same reading as
    // `the_removal_moved_the_first_digit` in gh303_the_connector_is_one_cell.rs.
    // The digit is read off the tree so that a later repair of this template
    // does not go red for a reason that has nothing to do with GH #454.
    let declared = config_at(&root); // parsed for the ports check below
    let version = std::fs::read_to_string(repo("templates/assistant/template.json"))
        .ok()
        .and_then(|raw| meclaw_core::serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v["version"].as_str().map(str::to_string))
        .unwrap_or_default();
    assert_eq!(
        version.split('.').next().unwrap_or_default(),
        "2",
        "templates/assistant/template.json says {version:?}. Removing the `channels` \
         address and the `turn` lane is a removal, and neither rule of \
         docs/development-rules.md § 4 covers one — it is the first digit, and it must \
         never go back below 2."
    );

    assert!(
        declared["params"].get("ports").is_none(),
        "templates/assistant declares params.ports = {:?}. The level is wired INTO — by the \
         member's firewall, its memory and its channels container — and sealing it would \
         refuse exactly those endpoints (hive_port_boundary).",
        declared["params"].get("ports")
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

    // Two edges reach the tool surface from the conversation surface and they
    // are two DECISIONS, not two tools: the guarded default that carries every
    // tool call whatever it is called, and the `schemas` request of GH #464 --
    // one lane, one edge, and it names no tool either. What #286 removed and
    // what must never come back is an edge PER TOOL, so the measurement is on
    // the lanes: every edge here is conditioned on `hop.route` and none of them
    // reads `hop.tool_name`.
    let to_tools: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./surface" && e.to == "./tools")
        .collect();
    let lanes: Vec<Option<String>> = to_tools
        .iter()
        .map(|e| stated_route(e.condition.as_deref()))
        .collect();
    assert_eq!(
        lanes,
        vec![Some("tool".to_string()), Some("schemas".to_string())],
        "the N+1-edge shape of #286 reappeared at this level: one edge per LANE is the          shape, one edge per tool is the defect: {to_tools:#?}"
    );
    let schemas_edge = to_tools[1];
    assert!(
        !schemas_edge.is_default,
        "the schemas request is an ORDINARY edge: a second default out of ./surface would          make the two compete for every message nothing regular carried: {schemas_edge:#?}"
    );
    assert_eq!(
        schemas_edge
            .modifier
            .as_ref()
            .and_then(|m| m.set_hop.get("route"))
            .map(String::as_str),
        Some("'in_schemas'"),
        "the request stamps the tool surface's own declaration lane: {schemas_edge:#?}"
    );
    let tools_edge = to_tools[0];
    assert!(
        tools_edge.is_default,
        "the tool exit is the GUARDED DEFAULT of ./surface — without `default: true` it \
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

    // The consult errand: an ORDINARY conditioned edge, so that firing it
    // suppresses the default for that message. Since GH #530 there is exactly
    // ONE — `ask_memory` is retired, because the lookup class it carried
    // assumed the core could answer a memory question and the core has no
    // memory leg, while the surface has one one hop away through the
    // `memory_recall` its own collector serves. Since GH #529 this sender also
    // has a SCHEMAS edge to the core, which is not an errand: it is filtered
    // out by the thing that makes an errand an errand, a `hop.tool_name` term.
    let consults: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./surface" && e.to == "./cogny")
        .filter(|e| {
            e.condition
                .as_deref()
                .unwrap_or_default()
                .contains("hop.tool_name == '")
        })
        .collect();
    assert_eq!(
        consults.len(),
        1,
        "one errand from the surface to the core, and it is `consult_cogny` (GH #530): \
         {consults:#?}"
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
        vec!["consult_cogny".to_string()],
        "`ask_memory` is retired (GH #530): a fast memory question is asked by the surface \
         itself, and what comes here is synthesis, a time series or anything multi-step"
    );
}

// ─────────────────── (4) the suppression precondition: no unconditional tee

/// Suppression is per SENDER (`crates/meclaw-colony/src/edge_table.rs`, the
/// two-phase evaluation): if ANY regular out-edge of `./surface` decided, the
/// default phase never runs. Every other edge out of `./surface` is therefore
/// conditioned on something a `tool` message does not carry — the seven lanes
/// the surface sends out of the level, and the two errands by name.
///
/// If the authored set ever grows a logger, a tap or a mirror without its own
/// route condition, the tool surface goes dark for every call. That is the
/// requirement, and it is written into the config's own `because` next to the
/// default edge as well as here.
#[test]
fn no_regular_out_edge_of_the_surface_is_unconditional() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    for e in hp.graph.edges.iter().filter(|e| e.from == "./surface") {
        if e.is_default {
            continue;
        }
        let cond = e.condition.as_deref().unwrap_or_default();
        assert!(
            cond.contains("hop.route"),
            "the edge ./surface -> {} carries no route condition. Suppression is per \
             SENDER: an unconditional tee out of ./surface fires for every tool call and \
             silences the guarded default, and the tool surface goes dark. {e:#?}",
            e.to
        );
        let route = stated_route(Some(cond)).unwrap_or_default();
        assert!(
            route != "tool" || cond.contains("hop.tool_name"),
            "the edge ./surface -> {} takes the whole `tool` lane without naming an \
             errand, so no tool call ever reaches the default: {e:#?}",
            e.to
        );
    }
}

// ───────────────────── (5) W7-R4: a discriminator is never read before the lane

/// The loop class GH #286 found and driver ruling **W7-R4** closed, checked here
/// because this level has the same shape: a message that leaves `./surface` on
/// a discriminator comes back through `./surface`, and a door that reads only
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

    // The three occupants and nothing else. `telegram-connector` used to be on
    // this list because a connector stood inside the level; since GH #454 it
    // stands in the member's channels container, and a lane of this level that
    // cited it would be citing a template that is not an occupant here.
    let pins: Vec<String> = ["talky", "cogny", "tools"]
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
    // Every outbound lane is one an occupant really produces. There is no
    // exception any more: `turn` used to be one, normalised by this level out of
    // the connector's single wire, and the connector left with GH #454.
    for e in &emits {
        assert!(
            talky_emits.contains(e) || cogny_emits.contains(e) || tools_emits.contains(e),
            "the level emits '{e}', which no occupant produces: talky {talky_emits:?}, \
             cogny {cogny_emits:?}, tools {tools_emits:?}"
        );
    }
    assert!(
        !emits.contains(&"turn".to_string()),
        "the level still emits `turn`. Nothing inside it produces a raw inbound wire any \
         more — the channel is the member's (GH #454) and reaches the member's screen \
         without crossing this level at all: {emits:?}"
    );

    // The subtraction. Since GH #454 there is exactly ONE — `tool`, consumed
    // inside the level by ./tools through the guarded default. `answer` used to
    // be the second and is now the level's own emit, which is the whole of
    // GH #454 read off the derivation rule.
    let gone = "tool".to_string();
    assert!(
        talky_emits.contains(&gone),
        "the subtraction of `{gone}` is stale: talky no longer emits it"
    );
    assert!(
        !emits.contains(&gone),
        "`{gone}` is consumed INSIDE this level by ./tools through the guarded default. A \
         level that re-declared it would promise a lane whose messages never leave."
    );

    // And the lane that STOPPED being a subtraction, which is the whole of
    // GH #454 read off the derivation rule. The connector that consumed the
    // surface's `answer` inside this level is the member's now and stands
    // outside, so the lane crosses and must be declared.
    assert!(
        talky_emits.contains(&"answer".to_string()),
        "the surface no longer emits `answer`: {talky_emits:?}"
    );
    assert!(
        emits.contains(&"answer".to_string()),
        "the level does not carry `answer`. Since GH #454 nothing inside consumes it — the \
         connector moved up to the member — so it crosses, and a lane that crosses without \
         being declared dies as no_route at the boundary: {emits:?}"
    );
    let carries_answer_out = hive_params(&root).graph.edges.iter().any(|e| {
        e.from == "./surface"
            && e.to == "."
            && stated_route(e.condition.as_deref()).as_deref() == Some("answer")
    });
    assert!(
        carries_answer_out,
        "`answer` is declared and no edge carries it out of ./surface — the declaration \
         would be a promise with nothing behind it"
    );
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
    assert!(
        !takes_up.contains("turn"),
        "the member still reads `turn` off ./assistants. Since GH #454 the raw wire is the \
         CHANNEL's and reaches the screen from ./channels; an assistant emits an `answer` \
         and never a turn: {takes_up:?}"
    );

    // What the member CONSUMES of this level's output, as opposed to re-emitting
    // it. Read off the edges, because the container carries no contract and the
    // edges are the only statement about the boundary the substrate reads.
    // `answer` is on this list since GH #454 — it goes back to a channel of the
    // person — and `write` is on it twice over, as the close pass fan-out of
    // GH #447 beside the archive copy that leaves the level. `dump` joined them
    // with GH #475: the session ledger of a generation is filed by the member's
    // own export sink, in a directory beside the three holders' own, because a
    // document that only exists as messages is not a backup. `turn_write`
    // joined them with GH #527, the second fan-out beside `write`: it is the
    // only path in this substrate from a conversation into an `episodes` table,
    // and the level that HOLDS the memory declined it until then — nine hops up
    // and a `hive_no_route` at the OS root, once per stored turn.
    let consumed_by_the_member: BTreeSet<String> = mhp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./assistants" && e.to != ".")
        .filter_map(|e| stated_route(e.condition.as_deref()))
        .collect();
    let want: BTreeSet<String> = [
        "answer",
        "recall",
        "extraction",
        "write",
        "turn_write",
        "dump",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        consumed_by_the_member, want,
        "the member consumes exactly the six lanes of this level it has a holder for: the \
         `answer` goes to a channel of the PERSON (GH #454), `recall` and `extraction` to \
         the memory that belongs to the person (GH #122), `write` is fanned onto the \
         memory's close pass as well as leaving the level (GH #447), `turn_write` is fanned \
         onto that same memory's episode lane (GH #527) -- the only path a conversation has \
         into an `episodes` table -- and `dump`, the transfer document of the generation's \
         session keeper, lands in the member's own export sink (GH #475). Every other lane \
         an assistant raises crosses the member and is the parent's to drain."
    );
    for lane in &consumed_by_the_member {
        assert!(
            emits.contains(lane),
            "the member consumes '{lane}' inside itself and no assistant emits it: {emits:?}"
        );
    }
}

// ─────────────────── (7) the countable promises on the public surface (§ 2d)

/// The English number word for a small count — the spelling this README uses.
fn number_word(n: usize) -> String {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    if n < 20 {
        return ONES[n].to_string();
    }
    let tens = ["", "", "twenty", "thirty", "forty", "fifty"];
    let t = tens
        .get(n / 10)
        .copied()
        .unwrap_or_else(|| panic!("no word for {n} — extend the table with the prose"));
    if n.is_multiple_of(10) {
        t.to_string()
    } else {
        format!("{t}-{}", ONES[n % 10])
    }
}

/// The one paragraph of the README that carries `needle`, joined into one line.
///
/// Paragraphs rather than lines, because the README is hard-wrapped and a
/// sentence that names three numbers at once routinely straddles a break.
fn paragraph_with(readme: &str, needle: &str) -> String {
    readme
        .split("\n\n")
        .find(|p| p.contains(needle))
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| {
            panic!(
                "templates/assistant/README.md carries no paragraph with {needle:?} — a \
                 drift lock that stops finding its sentence pins nothing"
            )
        })
}

/// **The drift lock the counted prose owes** (`docs/development-rules.md` § 2d).
///
/// The public surface of this level states four countable promises — how many
/// lanes it declares, how many drain pairings, how many edges it draws, and how
/// many of those are drawn around `./surface`. W5 measured the failure mode this
/// prevents: a template README kept describing a lane count the tree had already
/// moved past, and no test was red because no test ever read the sentence.
///
/// Every number below is DERIVED from `templates/assistant/config.json` inside
/// the test. Grepping the sentence alone would pin a string; asserting the
/// mechanism alone would let the prose drift away from it.
#[test]
fn the_readme_counts_are_the_lanes_and_edges_this_level_declares() {
    let Some(root) = shipped() else { return };
    let readme = std::fs::read_to_string(root.join("README.md")).expect("the README ships");
    let hp = hive_params(&root);
    let (accepts, emits) = lanes(&hp);

    let total = hp.graph.edges.len();
    let drains = hp.required_drains.as_ref().map(Vec::len).unwrap_or(0);
    let all_lanes = accepts.len() + emits.len();
    let around_surface = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./surface" || e.to == "./surface")
        .count();
    let into_surface = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "." && e.to == "./surface")
        .count();
    let out_of_surface = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./surface" && e.to == ".")
        .count();

    for (n, needle, what) in [
        (
            total,
            "no container at all",
            "the total in the opening line",
        ),
        (
            all_lanes,
            "all at the assistant's own path",
            "the lane count above the two tables",
        ),
        (
            drains,
            "pairings are declared in `params.required_drains`",
            "the drain pairings",
        ),
    ] {
        let para = paragraph_with(&readme, needle);
        assert!(
            para.to_lowercase().contains(&number_word(n)),
            "{what}: templates/assistant/README.md must say `{}` ({n} derived from \
             params):\n  {para}",
            number_word(n)
        );
    }

    // The `What it ships` block names three of them at once, and a block that
    // drifts one number at a time is exactly what § 2d is about.
    let ships = paragraph_with(&readme, "config.json            the level:");
    for (n, what) in [
        (all_lanes, "lanes"),
        (drains, "drain pairings"),
        (total, "edges"),
    ] {
        assert!(
            ships.to_lowercase().contains(&number_word(n)),
            "the `What it ships` block must say `{}` for the {what} ({n} derived):\n  \
             {ships}",
            number_word(n)
        );
    }

    // The fan-in measurement, which is written in digits rather than in words —
    // it is a measurement beside a historical one and reads as a comparison.
    let fan_in = paragraph_with(&readme, "around `./surface` today");
    assert!(
        fan_in.contains(&format!("**{around_surface}** around `./surface` today")),
        "the fan-in measurement must say {around_surface}, which is what the template \
         draws around ./surface:\n  {fan_in}"
    );
    // `paragraph_with` collapses runs of whitespace, so the aligned table reads
    // as `<count> <from> -> <to> <what>` here — the alignment is the file's, the
    // count is what this asserts.
    let table = paragraph_with(&readme, "the entry lanes that reach the surface");
    for (n, line) in [
        (into_surface, ". -> ./surface the entry lanes"),
        (out_of_surface, "./surface -> . the exits it produces"),
    ] {
        assert!(
            table.contains(&format!("{n} {line}")),
            "the fan-in table must open its `{line}` row with {n}:\n  {table}"
        );
    }
    let rest = paragraph_with(&readme, "for the level.");
    assert!(
        rest.to_lowercase()
            .contains(&format!("**{}** for the level", number_word(total))),
        "the edges that do NOT touch the surface are counted up to the level total, and \
         that total is {} ({total} derived):\n  {rest}",
        number_word(total)
    );

    // The library entry counts the same lanes a third time, in its own words. It
    // is the file a builder reads before it draws a single edge, so a wrong count
    // there is a mutation that leaves a lane unwired — and it is the surface that
    // drifted twice already, once for `in_pack` and once for `pack_ack`.
    let meta: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("template.json")).expect("the library entry ships"),
    )
    .expect("the library entry is json");
    let ports = meta["description"]["examples"][0]
        .as_str()
        .expect("the first example is the PORTS block");
    for (n, sentence) in [
        (
            accepts.len(),
            format!(
                "Entry lanes, all {} addressed at the assistant path itself",
                number_word(accepts.len())
            ),
        ),
        (
            emits.len(),
            format!(
                "Exits, all {} leaving the assistant path",
                number_word(emits.len())
            ),
        ),
        (
            drains,
            format!("{} pairings are declared in params.required_drains", {
                let w = number_word(drains);
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => w,
                }
            }),
        ),
    ] {
        assert!(
            ports.contains(&sentence),
            "the PORTS block of templates/assistant/template.json must say {sentence:?} \
             ({n} derived from params):\n{ports}"
        );
    }
    let where_from = meta["description"]["examples"][3]
        .as_str()
        .expect("the fourth example is the derivation");
    assert!(
        where_from.starts_with(&format!(
            "WHERE THE {} LANES COME FROM:",
            number_word(all_lanes).to_uppercase()
        )),
        "the derivation example counts the lanes a fourth time and must say {} \
         ({all_lanes} derived):\n{where_from}",
        number_word(all_lanes).to_uppercase()
    );
}

// ══════════════════════════════════════════════════════ the substrate half

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

/// The colony around the assistant: one door in on `in_turn`, and a drain for
/// every lane the level emits.
///
/// This is the whole of the per-instance wiring since GH #454. It used to carry
/// a connector + talky pair per channel as well, with deep endpoints below
/// `<assistant>/channels`; the channel is the member's now, so an assistant is
/// instantiated and wired in ONE mutation and nothing follows it.
fn main_config() -> Value {
    let mut edges = vec![json!({"from": "./driver", "to": "./agent",
                                "condition": "has(hop.route) && hop.route == 'in_turn'"})];
    for lane in [
        "answer",
        "write",
        "turn_write",
        "extraction",
        "recall",
        "prune",
        "error",
        "build",
    ] {
        edges.push(json!({"from": "./agent", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
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

/// The conversation surface, doubled: it takes the screened turn and emits
/// whatever the hop asks for — a tool call, a consult errand, or a plain answer.
///
/// Since GH #454 this is the cell an inbound lane of the level reaches directly:
/// there is no container between the level and it, and its `answer` leaves the
/// level on the level's own edge instead of being eaten by a connector beside
/// it.
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
            "purpose": "Test double for the conversation surface of the assistant level.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Build the tree: the SHIPPED assistant, with all THREE `ref` markers replaced
/// by answering `code` doubles.
///
/// Nothing is staged below the level any more. The tree under test is the
/// template and the template alone, which is the point of 2.0.0: the level is
/// complete at birth, and the mutation that instantiates it draws no
/// per-channel follow-up.
fn build_tree(td: &tempfile::TempDir, source: &std::path::Path) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(root, "main/driver/config.json", &driver_cell());
    copy_cells(source, &root.join("main/agent"));
    write(root, "main/agent/surface/config.json", &talky_cell());
    write(root, "main/agent/cogny/config.json", &cogny_cell());
    write(root, "main/agent/tools/config.json", &tools_cell());
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
/// The round is followed the whole way: the screened turn goes in on `in_turn`,
/// reaches the SURFACE directly — there is no container between the level and it
/// since GH #454 — and what leaves the assistant is the `answer` on its way to
/// the channel that asked. The channel is the member's, so this level neither
/// knows nor needs to know which one it was; `context.channel` rode in on the
/// turn and rides back out on the answer, and the member's own edge into
/// `./channels` is what turns that name into an address.
///
/// `in_served_by` is a POSITIVE receipt carried through: every double answers,
/// so a second occupant being served would be a second message rather than a
/// silence to be waited out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_takes_the_default_and_a_consult_takes_the_regular_edge() {
    let Some(source) = shipped() else { return };

    // The discriminator the ONE tool surface tells its two callers apart by, read
    // off the shipped edges rather than written down here: the guarded default
    // stamps one value and the reasoning core's own tool edge stamps another,
    // and all the round proves is that the answer came back to whoever asked.
    let hp = hive_params(&source);
    let stamped_caller = |from: &str| -> String {
        hp.graph
            .edges
            .iter()
            .find(|e| e.from == from && e.to == "./tools")
            .and_then(|e| e.modifier.as_ref())
            .and_then(|m| m.set_context.get("tool_caller"))
            .map(|v| v.trim_matches('\'').to_string())
            .unwrap_or_else(|| panic!("no edge {from} -> ./tools stamps context.tool_caller"))
    };
    let from_surface = stamped_caller("./surface");
    let from_cogny = stamped_caller("./cogny");
    assert_ne!(
        from_surface, from_cogny,
        "the two callers of the one tool surface stamp the same discriminator — the answer \
         to a consult would come back to the surface that never asked"
    );

    for (name, want_served, want_caller) in [
        ("web_search", "tools", from_surface.as_str()),
        ("consult_cogny", "cogny", from_cogny.as_str()),
    ] {
        let td = tempfile::TempDir::new().unwrap();
        build_tree(&td, &source);
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
            "answer",
            "{name}: what leaves an assistant is the ANSWER, on its way to the channel that \
             asked (GH #454). A `turn` here would mean a raw wire still lives inside the \
             generation — hop {:?}",
            out.headers.hop
        );
        assert_eq!(
            hop_of(&out, "served_by"),
            "talky",
            "{name}: the answer left the level from somewhere other than the surface. There \
             is no connector between the two any more — it is the member's — so the exit is \
             `./surface -> .` and nothing else: hop {:?}",
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

        // ONE round, ONE answer out. A second message here is a second delivery
        // of the same call — the guarded default firing beside a consult, or the
        // tool surface answering both callers because the discriminator stopped
        // discriminating. Every double answers, so a second delivery is a second
        // message rather than a silence to be waited out; the wait exists only
        // to let one that is already in flight land.
        tokio::time::sleep(Duration::from_millis(750)).await;
        if let Ok(extra) = sink_rx.try_recv() {
            panic!(
                "{name}: the assistant produced a SECOND answer for one call — hop {:?}",
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
/// the consult edge never fire together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_never_reaches_the_tool_surface() {
    let Some(source) = shipped() else { return };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &source);
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

    // Every edge out of `./surface` is decided in ONE `apply_edges` call on ONE
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

/// #303's acceptance, read after GH #454 moved the subject: a second channel
/// costs this level NOTHING AT ALL.
///
/// #303's ruling was that the fan-in edges between the container and its
/// siblings are internal edges of the TEMPLATE, so a second channel cost two
/// instantiations plus their pairing edges and never a re-run of the fan-in.
/// That was already the cheap answer; 2.0.0 is the free one. The channel stands
/// one level up, in `<member>/channels`, and this level has no endpoint for it:
/// a turn from any channel arrives on the same `in_turn` door, the answer leaves
/// on the same `answer` exit, and `context.channel` — which this level never
/// reads — is what tells the MEMBER where to send it back.
///
/// So the measurement is over the template's own edges rather than over a booted
/// tree: two channels and twenty produce the same file, because the file does
/// not mention a channel anywhere. A count that grew per channel would have to
/// grow HERE, and there is nothing here for it to grow.
#[test]
fn a_second_channel_costs_this_level_nothing_at_all() {
    let Some(root) = shipped() else { return };
    let hp = hive_params(&root);

    for e in &hp.graph.edges {
        for endpoint in [&e.from, &e.to] {
            assert!(
                !endpoint.contains("channels"),
                "the edge {} -> {} names a channel path. A channel is the person's since \
                 GH #454: this level is ADDRESSED by one and contains none, so no edge of \
                 the template can have an endpoint in one.",
                e.from,
                e.to
            );
            assert!(
                endpoint == "." || SIBLINGS.contains(&endpoint.as_str()),
                "the edge {} -> {} touches {endpoint:?}, which is neither this level's own \
                 path nor one of its three occupants {SIBLINGS:?}. A fourth endpoint is a \
                 node the level would have to be filled with — and the level is complete at \
                 birth.",
                e.from,
                e.to
            );
        }
    }

    // The door and the exit are ONE each, and neither is per-channel. Adding a
    // channel to the member adds one node and two edges THERE; here it adds
    // nothing, because both ends of the conversation already exist.
    let doors = hp
        .graph
        .edges
        .iter()
        .filter(|e| {
            e.from == "."
                && e.to == "./surface"
                && stated_route(e.condition.as_deref()).as_deref() == Some("in_turn")
        })
        .count();
    assert_eq!(
        doors, 1,
        "a screened turn reaches the surface through exactly one door, whatever channel it \
         came from — a door per channel is the shape GH #454 removed"
    );
    let exits = hp
        .graph
        .edges
        .iter()
        .filter(|e| {
            e.from == "./surface"
                && e.to == "."
                && stated_route(e.condition.as_deref()).as_deref() == Some("answer")
        })
        .count();
    assert_eq!(
        exits, 1,
        "the answer leaves on exactly one exit. Two would mean this level had started to \
         tell channels apart, which is the member's job and needs `context.channel`, a key \
         nothing at this level reads"
    );

    assert!(
        !root.join("channels").exists(),
        "a channels container is back on the assistant level"
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
