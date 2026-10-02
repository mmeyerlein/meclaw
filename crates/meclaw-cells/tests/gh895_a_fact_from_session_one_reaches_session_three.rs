//! GH #895 (R-GT-9) -- a fact from session one reaches session three.
//!
//! The gate wave measured the owner's memory scenarios red: what was said in a
//! first conversation was missing from the third (B1/B2/B4/B5), and the store
//! was not asked at all on any closing question (G1: zero recalls). Read at the
//! tree, the typed surface had no road to the memory: `talky-chat` asks on
//! every turn (`memory_tier` "1" on its ref marker), the generation's contract
//! vouches for it as an asker, and the recipe drew the recall v-lane for `talky`
//! and `cogny` only -- so a chat turn waited for its memory leg until
//! `round_idle_ms` and answered without it.
//!
//! This file is the lock, not a measurement (the measurement runs once at the
//! end of the program, A1-Ledger PP-5). Two layers, cheapest first:
//!
//! 1. **The road** (no colony): for BOTH surfaces the collector's ask reaches
//!    the surface's curator, the curator's ask reaches the memory hive's
//!    `recall` cell as `in_query` with the member's stamps, the bundle walks home
//!    into the surface's collector -- and a gap's own ask comes home to the
//!    curator's `push` and never into the collector, which has no round for it.
//!    A gap's find in a duplex call leaves the surface ONCE and reaches the
//!    channel as advice.
//! 2. **The conversation** (a colony of the shipped member road, generation and
//!    memory hive, with stub models): a fact said in session one stands in the
//!    memory pair of the question in session three -- on `talky` and on
//!    `talky-chat`; the question the curator built reaches the memory's search
//!    whole, enrichment and the person's words, which the memory itself says by
//!    not saying `query_hygiene`; the prompt hygiene stands in the brain's
//!    system part; and a `gap` in an answer is looked up exactly once, its find
//!    opening the next round.
//!
//! Free of a real provider by construction: every `llm` cell of the tree talks
//! to one of two local stubs, the memory hive's embedder too.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry one of the templates or the example is skipped, never judged.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

/// The round every turn of this file is spoken in, in the affinity vocabulary:
/// ONE audience for all three sessions -- what the lock asks of the memory is
/// that the same round sees what it said; another round seeing it would be the
/// defect (OR-KY-P2).
const AUDIENCE: &str = r#"["member:owner","agent:scribe"]"#;

const FACT_SAID: &str = "My sister Hannah moved to Porto last spring.";
/// What the memory pair of session three has to carry.
const FACT_TOKEN: &str = "Porto";
/// Session two names things -- the entities session three's question is
/// enriched with (plan P § 1: the last three rounds of the wall).
const OTHER_SAID: &str =
    "Can you recommend a book about sailing, something like Moby Dick or Treasure Island?";
const OTHER_REPLY: &str = "Try The Cruel Sea by Nicholas Monsarrat.";
/// Long enough that the question and every name the wall offers would not fit
/// the memory's `query_safe_chars` together: a budget over it would cost the
/// enrichment at the memory (review I-3), the talky's budget does not.
const QUESTION: &str = "Sorry, my memory is a sieve these days and I keep mixing up the \
                        family news I told you about, so where does my sister Hannah live now?";
const GAP: &str = "Hannah ferry Porto";

/// The member of the worked example and its generation, one segment shorter
/// than `examples/organism`, exactly as `gh841` reads them.
const MEMBER: &str = "/m";
const HIVE: &str = "/m/memory-hive";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/scribe";

const SURFACES: [&str; 2] = ["talky", "talky-chat"];

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/memory-hive/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/cogny/config.json",
        "templates/collector/config.json",
        "templates/curator/config.json",
        "templates/curator/push/config.json",
        "templates/dispatcher/config.json",
        "templates/session-keeper/config.json",
        "templates/tools/config.json",
        "examples/organism/grow-assistant.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
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
    v.as_object().cloned().expect("a JSON object")
}

// ═══════════════════════════════════════════════════════ 1. the road (router)

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    let params = read_json(&repo(rel))["params"].clone();
    let hp: HiveParams = meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{rel}: params: {e}"));
    hp.graph.edges
}

fn recipe_edges(rel: &str) -> Vec<EdgeSpec> {
    read_json(&repo(rel))["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: no diff.add_edges"))
        .iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{rel}: add_edges entry {e}: {err}"))
        })
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
}

/// Every level the road crosses: the member, its memory hive, the container as
/// `examples/organism` wires it, one generation, and for each surface its own
/// composite, collector and curator (`talky-chat` is a ref to `talky`, so both
/// carry the same edges).
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        HIVE,
        &hive_edges("templates/memory-hive/config.json"),
        "memory-hive",
    );
    add_edges(
        &mut t,
        BOX,
        &recipe_edges("examples/organism/grow-assistant.json"),
        "grow-assistant",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    for surface in SURFACES {
        let at = format!("{GEN}/{surface}");
        add_edges(
            &mut t,
            &at,
            &hive_edges("templates/talky/config.json"),
            surface,
        );
        add_edges(
            &mut t,
            &format!("{at}/collector"),
            &hive_edges("templates/collector/config.json"),
            "collector",
        );
        add_edges(
            &mut t,
            &format!("{at}/curator"),
            &hive_edges("templates/curator/config.json"),
            "curator",
        );
    }
    t
}

fn ctx_of(h: &Headers, key: &str) -> String {
    h.context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .into()
}

fn hop_of(h: &Headers, key: &str) -> String {
    h.hop.get(key).and_then(|v| v.as_str()).unwrap_or("").into()
}

/// Follow one message until nothing takes it further, insisting on exactly one
/// addressee per step: a fan-out on this road is two asks where one was made.
fn walk(table: &EdgeTable, from: &str, headers: Headers) -> (Vec<String>, Headers) {
    walk_until(table, from, headers, "")
}

/// `walk`, stopping where the message reaches `stop` -- a level that fans out
/// on purpose (the member hands a section to every party that hears it).
fn walk_until(
    table: &EdgeTable,
    from: &str,
    headers: Headers,
    stop: &str,
) -> (Vec<String>, Headers) {
    let mut trace = vec![from.to_string()];
    let mut here = Path::new(from);
    let mut hs = headers;
    for _ in 0..32 {
        if here.as_str() == stop {
            return (trace, hs);
        }
        let out = apply_edges(table, &here, &hs);
        if out.is_empty() {
            return (trace, hs);
        }
        assert_eq!(
            out.len(),
            1,
            "at {} the message fans out to {:?}",
            here.as_str(),
            out.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
        );
        let d = out.into_iter().next().expect("checked non-empty");
        here = d.target;
        hs = d.headers_out;
        trace.push(here.as_str().to_string());
    }
    panic!("the walk did not settle in 32 hops: {trace:?}");
}

/// The context a turn of `surface` runs under: the generation, the round and
/// the channel the member's door stamped, the session its keeper minted.
fn turn_context(surface: &str) -> Value {
    let mut c = json!({"assistant": "scribe", "session_id": "S-895", "turn_id": "S-895#4",
                       "iter": "0", "audience_set": AUDIENCE, "channel": "chat:895"});
    if surface == "talky-chat" {
        c["channel_node"] = json!("chat");
    }
    c
}

/// The collector's ask, key for key (`collector` `head` + `recall_ask`).
fn ask_hop(extra: &[(&str, &str)]) -> Map<String, Value> {
    let mut h = as_map(
        &json!({"route": "recall", "phase": "recall", "turn_id": "S-895#4",
                               "session_id": "S-895", "iter": "0",
                               "recall_query": QUESTION, "memory_tier": "1",
                               "recall_window_from": "", "recall_window_to": ""}),
    );
    for (k, v) in extra {
        h.insert((*k).to_string(), json!(v));
    }
    h
}

#[test]
fn the_ask_of_every_surface_reaches_its_curator() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for surface in SURFACES {
        let from = format!("{GEN}/{surface}/collector/assemble");
        let start = Headers::from_parts(as_map(&turn_context(surface)), ask_hop(&[]));
        let (trace, arrived) = walk(&t, &from, start);
        assert_eq!(
            trace.last().map(String::as_str),
            Some(format!("{GEN}/{surface}/curator/policy").as_str()),
            "{surface}: the collector's ask goes to the curator, which builds its question \
             (OR-KY-G4): {trace:?}"
        );
        assert_eq!(hop_of(&arrived, "route"), "in_recall_ask");
        assert_eq!(hop_of(&arrived, "recall_query"), QUESTION);
    }
}

#[test]
fn the_curators_ask_reaches_the_memory_and_its_bundle_the_collector() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for surface in SURFACES {
        // Out: what `push` emits, through the member's door.
        let from = format!("{GEN}/{surface}/curator/push");
        let start = Headers::from_parts(as_map(&turn_context(surface)), ask_hop(&[]));
        let (trace, at_hive) = walk(&t, &from, start);
        assert_eq!(
            trace.last().map(String::as_str),
            Some("/m/memory-hive/recall"),
            "{surface}: the ask must reach the memory hive's recall cell -- a surface \
             without its v-lane stops at its own rim and its collector waits for \
             `round_idle_ms` (R-GT-9): {trace:?}"
        );
        assert_eq!(hop_of(&at_hive, "route"), "in_query");
        assert_eq!(
            ctx_of(&at_hive, "recall_caller"),
            surface,
            "the asker's token"
        );
        assert_eq!(
            ctx_of(&at_hive, "audience_now"),
            AUDIENCE,
            "the member stamps"
        );
        assert_eq!(ctx_of(&at_hive, "channel"), "chat:895");
        assert_eq!(ctx_of(&at_hive, "recall_query"), QUESTION);
        assert!(
            !at_hive.context.contains_key("gap_ask") && !at_hive.hop.contains_key("gap_ask"),
            "an ambient ask is no gap's"
        );
        // Back: the bundle, home into the surface that asked.
        let mut hop = Map::new();
        hop.insert("route".into(), json!("bundle"));
        let back = Headers::from_parts(at_hive.context.clone(), hop);
        let (trace, home) = walk(&t, "/m/memory-hive/recall", back);
        assert_eq!(
            trace.last().map(String::as_str),
            Some(format!("{GEN}/{surface}/collector/assemble").as_str()),
            "{surface}: the bundle walks home into the asking surface's collector: {trace:?}"
        );
        assert_eq!(hop_of(&home, "route"), "in_bundle");
    }
}

#[test]
fn a_gaps_bundle_finds_the_curator_and_never_the_collector() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for surface in SURFACES {
        let from = format!("{GEN}/{surface}/curator/push");
        let hop = ask_hop(&[("recall_query", GAP), ("gap_ask", "g-895")]);
        let (trace, at_hive) = walk(
            &t,
            &from,
            Headers::from_parts(as_map(&turn_context(surface)), hop),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some("/m/memory-hive/recall"),
            "{surface}: {trace:?}"
        );
        assert_eq!(
            ctx_of(&at_hive, "gap_ask"),
            "g-895",
            "the surface's rim lifts the mark into context"
        );
        assert!(
            !at_hive.hop.contains_key("gap_ask"),
            "and leaves the member exactly the collector's hop keys"
        );
        let mut hop = Map::new();
        hop.insert("route".into(), json!("bundle"));
        let (trace, home) = walk(
            &t,
            "/m/memory-hive/recall",
            Headers::from_parts(at_hive.context.clone(), hop),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some(format!("{GEN}/{surface}/curator/push").as_str()),
            "{surface}: a gap's answer belongs to no round of the collector's: {trace:?}"
        );
        assert!(
            !trace.iter().any(|p| p.contains("/collector")),
            "{surface}: the gap's bundle touched the collector: {trace:?}"
        );
        assert_eq!(hop_of(&home, "route"), "in_gap_bundle");
    }
}

/// A gap's find in a duplex call is said, and said ONCE (review I-4): the
/// section `fact` leaves the curator, crosses the surface on exactly one edge
/// -- a second edge would say it twice -- and reaches the member, whose channel
/// takes it as advice. The gap's mark stays inside the surface.
#[test]
fn a_duplex_find_is_said_once_on_its_channel() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let from = format!("{GEN}/talky/curator/push");
    let mut ctx = turn_context("talky");
    ctx["engine"] = json!("duplex");
    ctx["channel_node"] = json!("phone");
    ctx["gap_ask"] = json!("g-895");
    let hop = as_map(&json!({"route": "sidecar", "section": "fact"}));
    let (trace, at_box) = walk_until(&t, &from, Headers::from_parts(as_map(&ctx), hop), BOX);
    assert_eq!(
        trace.last().map(String::as_str),
        Some(BOX),
        "the find climbs to the member's container: {trace:?}"
    );
    let out = apply_edges(&t, &Path::new(BOX), &at_box);
    let heard: Vec<_> = out
        .iter()
        .filter(|d| d.target.as_str() == "/m/channels")
        .collect();
    assert_eq!(
        heard.len(),
        1,
        "the channel hears it once: {:?}",
        out.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(hop_of(&heard[0].headers_out, "route"), "in_advise");
    assert!(
        !heard[0].headers_out.context.contains_key("gap_ask"),
        "the gap's mark stays inside the surface"
    );
}

// ═══════════════════════════════════════════════ 2. the conversation (colony)

/// The shipped template, copied the way instantiation lays it out (the
/// `copy_resolved` of `gh889`): a `cell.type: "ref"` directory is replaced by
/// the referenced template and its `override_params` applied (GH #140, #277).
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
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

struct Stubs {
    brain: String,
    background: String,
}

/// Point EVERY `llm` cell of the tree at a local stub: the surface's brain at
/// the scripted one, all others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, brain: &str, stubs: &Stubs) -> Vec<String> {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut pointed = Vec::new();
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(main)
            .expect("under main")
            .to_string_lossy()
            .replace('\\', "/");
        let (url, model) = if rel == brain {
            (&stubs.brain, BRAIN_MODEL)
        } else {
            (&stubs.background, BACKGROUND_MODEL)
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
        pointed.push(rel);
    }
    pointed.sort();
    pointed
}

/// Every timer of the tree, out of the run's way (the sweep of `gh889`).
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0895_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` bound to a dummy, the
/// memory's model and embedder endpoints to the background stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, stubs: &Stubs) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    vars.insert("MEMORY_LLM_BASE_URL".into(), stubs.background.clone());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", stubs.background),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (an `at` lane docks
/// elsewhere, ADR-0020).
fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The member's own edges between its generations and its memory, verbatim
/// off `templates/member`: the recall door and the bundle's way back, the
/// memory section, the episodes, the close pass, the memory tool and its menu.
fn member_memory_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (e["from"].as_str(), e["to"].as_str());
            matches!(
                (f, t),
                (Some("./assistants"), Some("./memory-hive"))
                    | (Some("./memory-hive"), Some("./assistants"))
            )
        })
        .collect()
}

/// The member stand-in: `./assistants` is the container, grown with the
/// generation `scribe` the way `examples/organism/grow-assistant.json` grows
/// it; `./memory-hive` is the person's memory; the member's own edges between
/// the two; everything else that leaves either is drained.
fn build_member(td: &tempfile::TempDir, surface: &str, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    let grown = read_json(&repo("examples/organism/grow-assistant.json"));
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    // A sweep closes every channel that has been silent at all.
    for s in SURFACES {
        override_params_on_disk(
            &main.join(format!("assistants/scribe/{s}/session-keeper/close")),
            &json!({"idle_ms": 1}),
        );
    }
    let mut edges = member_memory_edges();
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    for lane in rim_emits("memory-hive") {
        let taken = ["bundle", "tool_result", "tool_schemas", "reject"];
        if !taken.contains(&lane.as_str()) {
            edges.push(json!({"from": "./memory-hive", "to": "/park",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
    }
    edges.push(json!({"from": "./memory-hive", "to": "/park",
                      "condition": "has(hop.route) && hop.route == 'reject' && \
                                    (!has(hop.recall_caller) || hop.recall_caller == 'outside')"}));
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, &format!("assistants/scribe/{surface}/brain"), stubs);
    write_env(root, &main, stubs);
    pointed
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    #[allow(dead_code)]
    park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
            // The generation's own tools hive (`templates/tools`): spawned,
            // never called -- the stub brains call no tool.
            ("bash".to_string(), Arc::new(BashCellFactory)),
            ("edit".to_string(), Arc::new(EditCellFactory)),
            ("file".to_string(), Arc::new(FileCellFactory)),
            ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
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
        .expect("the generation, the member road and the memory hive must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

/// A person's words at the container's door, in the round of this file, on the
/// channel of `surface` (`channel_node` "chat" is what sends a turn to
/// `talky-chat`).
fn person(surface: &str, text: &str) -> Message {
    let mut ctx = json!({"assistant": "scribe", "channel": format!("{surface}:895"),
                         "audience_set": AUDIENCE});
    if surface == "talky-chat" {
        ctx["channel_node"] = json!("chat");
    }
    MessageBuilder::new(Path::new("/assistants"))
        .hop(as_map(&json!({"route": "in_turn"})))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// An operator's sweep at the surface's own path: its keeper ends every
/// channel that has been silent (`idle_ms` 1 here), and the close pass runs.
fn sweep(surface: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/assistants/scribe/{surface}")))
        .hop(as_map(&json!({"route": "in_sweep"})))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .unwrap_or_default()
}

/// Until `sql` returns at least `n` rows in `db`, or the deadline says what
/// did not happen.
async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) {
    let deadline = Instant::now() + DEADLINE;
    while rows(db, sql).len() < n {
        assert!(
            Instant::now() < deadline,
            "{what}: fewer than {n} row(s) of `{sql}` within {DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn answer(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(DEADLINE, rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            panic!(
                "{what}: no answer left the generation within {DEADLINE:?} -- a surface \
                 without a memory road waits for its memory leg until `round_idle_ms`"
            )
        })
}

/// One turn, one answer; returns every request the scripted brain has had.
async fn say(
    h: &ColonyHandle,
    ports: &mut Ports,
    brain: &MockOpenAI,
    surface: &str,
    text: &str,
) -> Vec<OpenAiRequestSnapshot> {
    h.send(person(surface, text)).await;
    answer(&mut ports.sink, &format!("{surface}: {text:?}")).await;
    brain.recorded_requests().await
}

/// Close the channel's session and wait until its keeper says so.
async fn end_session(td: &tempfile::TempDir, h: &ColonyHandle, surface: &str, closed: usize) {
    h.send(sweep(surface)).await;
    let db = td.path().join(format!(
        "main/assistants/scribe/{surface}/session-keeper/sessions/cell.db"
    ));
    until_rows(
        &db,
        "SELECT session_id FROM sessions WHERE closed = 1",
        closed,
        &format!("{surface}: the sweep ends the session"),
    )
    .await;
}

/// The role/content wire of the LAST request, and the index of the last
/// person's turn in it.
fn wire_of(req: &OpenAiRequestSnapshot) -> Vec<Value> {
    req.messages().cloned().unwrap_or_default()
}

fn content_of(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The `memory_recall` results that stand in `wire` after position `from`.
fn recall_results_after(wire: &[Value], from: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for m in &wire[from..] {
        for c in m["tool_calls"].as_array().cloned().unwrap_or_default() {
            if c["function"]["name"] == "memory_recall" {
                ids.push(c["id"].as_str().unwrap_or("").to_string());
            }
        }
    }
    wire[from..]
        .iter()
        .filter(|m| {
            m["role"] == "tool"
                && ids
                    .iter()
                    .any(|id| m["tool_call_id"].as_str() == Some(id.as_str()))
        })
        .map(content_of)
        .collect()
}

fn last_person(wire: &[Value], text: &str) -> usize {
    wire.iter()
        .rposition(|m| m["role"] == "user" && content_of(m).contains(text))
        .unwrap_or_else(|| panic!("{text:?} is not on the wire: {wire:#?}"))
}

/// The answer of session one: a few words and the memory section the block
/// contract asks for, with the fact in it.
fn remembering_reply() -> String {
    let block = json!({"memory": {
        "facts": [{"subject": "Hannah", "predicate": "lives_in", "claim": "Porto",
                   "fact_kind": "world", "valid_from": null}],
        "topic": {"movement": "start", "name": "Hannah's move"}}});
    format!("Porto -- how lovely for her.\n\n```sidecar\n{block}\n```")
}

async fn a_fact_crosses_two_sessions(surface: &str) {
    let brain = MockOpenAI::start(vec![
        canned_chat_completion(&remembering_reply(), "stop"),
        canned_chat_completion(OTHER_REPLY, "stop"),
        canned_chat_completion("In Porto.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_member(&td, surface, &stubs);
    assert!(
        pointed
            .iter()
            .any(|p| *p == format!("assistants/scribe/{surface}/brain")),
        "{surface}: the brain under test talks to the scripted stub; pointed: {pointed:?}"
    );
    let (h, mut ports) = boot(&td).await;
    let memory = td.path().join("main/memory-hive/store/cell.db");

    // Session one: the fact, and it is in the memory before the session ends.
    let reqs = say(&h, &mut ports, &brain, surface, FACT_SAID).await;
    until_rows(
        &memory,
        &format!("SELECT id FROM episodes WHERE content LIKE '%{FACT_TOKEN}%'"),
        1,
        "session one's words reach the memory",
    )
    .await;
    // The prompt hygiene stands in the brain's own instructions (R-27-11 B).
    let system = content_of(&wire_of(&reqs[0])[0]);
    assert!(
        system.contains("ask them instead of guessing") && system.contains("history_read"),
        "{surface}: the brain's system part lacks the hygiene: {system}"
    );
    end_session(&td, &h, surface, 1).await;

    // Session two: another topic.
    say(&h, &mut ports, &brain, surface, OTHER_SAID).await;
    end_session(&td, &h, surface, 2).await;

    // Session three: the question.
    let reqs = say(&h, &mut ports, &brain, surface, QUESTION).await;
    let log = message_log(td.path());
    h.shutdown().await;
    let wire = wire_of(reqs.last().expect("the question's request"));
    let at = last_person(&wire, QUESTION);
    let found = recall_results_after(&wire, at);
    assert!(
        found.iter().any(|r| r.contains(FACT_TOKEN)),
        "{surface}: the memory pair of session three does not carry the fact of session \
         one. Recall results after the question: {found:#?}"
    );
    // The question the curator built, measured where it is used: the memory
    // hive's door has it with the enrichment in front of the person's words,
    // and the memory's own query guard (GH #88) cut nothing -- a cut says
    // `query_hygiene` on everything the recall answers with.
    let asked: Vec<String> = log
        .iter()
        .filter(|(to, hop)| to == "/memory-hive" && hop["route"] == "in_query")
        .filter_map(|(_, hop)| hop["recall_query"].as_str().map(str::to_string))
        .filter(|q| q.ends_with(QUESTION))
        .collect();
    assert_eq!(
        asked.len(),
        1,
        "{surface}: one ask for the question: {asked:#?}"
    );
    assert!(
        asked[0].starts_with("[mentioned: "),
        "{surface}: the curator's enrichment reaches the memory: {asked:?}"
    );
    let cut: Vec<&(String, Value)> = log
        .iter()
        .filter(|(_, hop)| hop.get("query_hygiene").is_some())
        .collect();
    assert!(
        cut.is_empty(),
        "{surface}: the memory cut a query (GH #88) -- the enrichment or the person's \
         words never reached its search: {cut:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_from_session_one_reaches_session_three_on_talky() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    a_fact_crosses_two_sessions("talky").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_from_session_one_reaches_session_three_on_talky_chat() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    a_fact_crosses_two_sessions("talky-chat").await;
}

/// R-27-11 C: an answer that names a gap is looked up once, after it has left,
/// and the next round of the session begins with the find.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gap_is_looked_up_once_and_its_find_opens_the_next_round() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let surface = "talky-chat";
    let unsure = format!(
        "I am not sure which ferry that was.\n\n```sidecar\n{}\n```",
        json!({"gap": GAP})
    );
    let brain = MockOpenAI::start(vec![
        canned_chat_completion(&remembering_reply(), "stop"),
        canned_chat_completion(&unsure, "stop"),
        canned_chat_completion("It left at nine.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build_member(&td, surface, &stubs);
    let (h, mut ports) = boot(&td).await;
    let memory = td.path().join("main/memory-hive/store/cell.db");
    let ledger = td.path().join(format!(
        "main/assistants/scribe/{surface}/curator/ledger/cell.db"
    ));

    say(&h, &mut ports, &brain, surface, FACT_SAID).await;
    until_rows(
        &memory,
        &format!("SELECT id FROM episodes WHERE content LIKE '%{FACT_TOKEN}%'"),
        1,
        "the fact reaches the memory",
    )
    .await;
    say(
        &h,
        &mut ports,
        &brain,
        surface,
        "Which ferry did Hannah take?",
    )
    .await;
    until_rows(
        &ledger,
        "SELECT seq FROM marks WHERE kind = 'addendum'",
        1,
        "the gap's find is kept",
    )
    .await;
    let reqs = say(&h, &mut ports, &brain, surface, "And when did it leave?").await;
    let log = message_log(td.path());
    h.shutdown().await;

    let gap_asks: Vec<&(String, Value)> = log
        .iter()
        .filter(|(to, hop)| {
            to == "/memory-hive" && hop["route"] == "in_query" && hop["recall_query"] == GAP
        })
        .collect();
    assert_eq!(
        gap_asks.len(),
        1,
        "the gap is looked up exactly once: {gap_asks:#?}"
    );
    // The topic the memory section of the first answer named reaches the
    // memory with a later question of the session: filed by the curator's
    // intake, read by its push past the nameless `continue` of the quiet turn
    // between (OR-KY-71, review I-1) -- measured where it is used.
    let later: Vec<String> = log
        .iter()
        .filter(|(to, hop)| to == "/memory-hive" && hop["route"] == "in_query")
        .filter_map(|(_, hop)| hop["recall_query"].as_str().map(str::to_string))
        .filter(|q| q.ends_with("And when did it leave?"))
        .collect();
    assert_eq!(later.len(), 1, "one ask for the question: {later:#?}");
    assert!(
        later[0].contains("topic: Hannah's move"),
        "the session's topic does not reach the memory: {later:?}"
    );
    let wire = wire_of(reqs.last().expect("the next round's request"));
    let at = last_person(&wire, "And when did it leave?");
    let addendum = wire[..at]
        .iter()
        .rposition(|m| {
            // A tool result carries its short id in front (GH #892).
            m["role"] == "tool" && content_of(m).contains("[addendum to your last answer")
        })
        .unwrap_or_else(|| panic!("the next round does not begin with the find: {wire:#?}"));
    assert!(content_of(&wire[addendum]).contains(FACT_TOKEN));
    let asked = last_person(&wire, "Which ferry did Hannah take?");
    assert!(
        asked < addendum && addendum < at,
        "the addendum stands after the answer it adds to and before the next turn"
    );
}

/// `(to_path, hop)` of every delivery the colony logged.
fn message_log(root: &std::path::Path) -> Vec<(String, Value)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(to, headers)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        (to, h["hop"].clone())
    })
    .collect()
}
