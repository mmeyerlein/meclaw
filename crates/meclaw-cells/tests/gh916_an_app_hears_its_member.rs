//! GH #916 -- an app hears its member: the section it offered, the sessions
//! that closed, and the curators it pins into.
//!
//! An app declares three things the member owes it (`install_app`,
//! `gh916_an_app_declares_close_and_pins.rs` locks the rendered edges): the
//! sidecar section it `offers`, `listens: ["close"]`, and `pins` through one of
//! its cells. This file locks that the declaration is HEARD: a fixture app
//! (`tests/fixtures/gh916_probe_app`, section `probe`, whose one cell writes
//! every message it receives to a log file) gets the section from `talky`,
//! `talky-chat` and `cogny` exactly once each with the round it answered,
//! exactly one `in_close` per closed session, and its `pin` stands in the
//! curator of every brain. A second app built from the same hive and cell,
//! `quiet-app`, offers another section and declares neither `close` nor `pins`
//! -- it gets none of it. (`replace_sources` is locked in
//! `gh916_a_pin_source_is_replaced.rs`; here only that the pin arrives.)
//!
//! Two layers, cheapest first:
//!
//! 1. **The road** (no colony): the shipped member, container, generation,
//!    brain and curator edges plus the edges `install_app` renders for both
//!    apps, walked with `apply_edges`:
//!    1. the section `probe` of each brain's splitter reaches `probe-app`'s cell
//!       on exactly one path with its `turn_id`, never `quiet-app`; `other`
//!       reaches `quiet-app` and never `probe-app`;
//!    2. `memory` stays with each brain's curator, and from the curator of a
//!       voice it reaches the memory hive and never an app; `cogny`'s stays;
//!    3. the close batch (`write`) of a voice's curator reaches `probe-app` once
//!       as `in_close` with the session, round and channel, still reaches the
//!       memory hive's close pass and the member's own exit, never `quiet-app`;
//!    4. a `pin` of `probe-app`'s cell reaches the curator of each of the three
//!       brains once as `in_pin`, the round untouched; a `pin` of `quiet-app`'s
//!       cell is taken by no edge at all;
//!    5. an `answer` without a channel still leaves the member exactly once;
//!    6. the fixture's declaration is the one the reviewed golden renders.
//! 2. **The colony** (the member road of `gh895`: a generation `sam` of the
//!    shipped assistant, the shipped memory hive, both apps, the rendered edges
//!    written into the member's graph; stub models), measured at the receiver:
//!    1. the section written by each brain's model -- `cogny` through a consult
//!       -- stands in `probe-app`'s log exactly once per brain, with a session,
//!       a round and the brain's own sender, `quiet-app`'s log holds none;
//!    2. a closed session stands in `probe-app`'s log as exactly one `in_close`
//!       carrying the batch, `quiet-app`'s log holds none;
//!    3. a pin commanded at `probe-app` stands as one live row of source
//!       `probe` in each of the three curator ledgers; one commanded at
//!       `quiet-app` stands in none.
//!
//! Free of a real provider by construction: every `llm` cell of the tree talks
//! to a local stub, the memory hive's embedder too.
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
use meclaw_testing::{ColonyHandle, NEVER_CRON, emit_all, override_params_on_disk, shipped_script};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long a count is left standing after it was reached, so that a second
/// delivery -- the defect a count of "exactly one" is about -- has time to land.
const SETTLE: Duration = Duration::from_secs(2);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const AUDIENCE: &str = r#"["member:owner","agent:sam"]"#;
const ROUND: &str = "S-916#3";

/// The generation every declaration of this file is installed for.
const GENERATION: &str = "sam";
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];
const SECTION: &str = "probe";
const OTHER_SECTION: &str = "other";
const PROBE_APP: &str = "probe-app";
const QUIET_APP: &str = "quiet-app";

// Road paths: the member one segment below the root.
const MEMBER: &str = "/m";
const HIVE: &str = "/m/memory-hive";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/sam";
const APPS: &str = "/m/apps";

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/member/apps/config.json",
        "templates/memory-hive/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/cogny/config.json",
        "templates/collector/config.json",
        "templates/curator/config.json",
        "templates/dispatcher/config.json",
        "templates/session-keeper/config.json",
        "templates/tools/config.json",
        "templates/builder/recipes/config.json",
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

/// The fixture directory an app is built from: both apps share the probe
/// app's hive and cell, only their declarations differ.
fn declaration_dir(app: &str) -> std::path::PathBuf {
    match app {
        PROBE_APP => fixture("gh916_probe_app"),
        QUIET_APP => fixture("gh916_quiet_app"),
        other => panic!("no fixture for {other}"),
    }
}

fn declaration(app: &str) -> Value {
    let tpl = read_json(&declaration_dir(app).join("template.json"));
    assert_eq!(
        tpl["name"],
        json!(app),
        "the fixture is named after its app"
    );
    assert!(
        tpl["app"].is_object(),
        "{app}: the fixture carries no `app` declaration: {tpl}"
    );
    tpl["app"].clone()
}

/// What `install_app` draws for `app` at a member, member-relative: the
/// SHIPPED renderer over stdin, as `gh916_an_app_declares_close_and_pins` runs it.
fn install_edges(app: &str) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(&repo("templates/builder/recipes/config.json").to_string_lossy()),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/os/orgs/acme/members/alex",
                                                    "app": app,
                                                    "template": format!("{app}@1.0.0"),
                                                    "screen": "display",
                                                    "generation": GENERATION,
                                                    "declaration": declaration(app)}})
                                  .to_string()}],
        }),
    );
    let first = out.first().expect("the renderer emits");
    assert!(
        first["header"]["error_code"].is_null(),
        "{app}: install_app refused the declaration: {first}"
    );
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{app}: no add_edges in {first}"))
        .clone()
}

/// The container as `examples/organism` grows it, with its generation renamed
/// to the one the declarations are installed for.
fn container_edges() -> Vec<Value> {
    let grown = read_json(&repo("examples/organism/grow-assistant.json"));
    let raw = meclaw_core::serde_json::to_string(&grown["diff"]["add_edges"]).expect("serialise");
    let renamed = raw
        .replace("./scribe", &format!("./{GENERATION}"))
        .replace("'scribe'", &format!("'{GENERATION}'"))
        .replace("/assistants/scribe/", &format!("/assistants/{GENERATION}/"));
    assert!(
        !renamed.contains("./scribe") && !renamed.contains("'scribe'"),
        "the example's generation was not renamed whole"
    );
    meclaw_core::serde_json::from_str(&renamed).expect("reparse")
}

// ═══════════════════════════════════════════════════════ 1. the road (router)

fn specs(values: &[Value], label: &str) -> Vec<EdgeSpec> {
    values
        .iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{label}: edge {e}: {err}"))
        })
        .collect()
}

fn hive_edges_of(p: &std::path::Path) -> Vec<EdgeSpec> {
    let params = read_json(p)["params"].clone();
    let hp: HiveParams = meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
    hp.graph.edges
}

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    hive_edges_of(&repo(rel))
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
        });
    }
}

/// Every level the roads of this file cross: the member, its container as
/// `examples/organism` wires it, the generation, each brain with its curator,
/// the edges `install_app` renders for both apps, and each app's own hive.
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
        BOX,
        &specs(&container_edges(), "container"),
        "container",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    for brain in BRAINS {
        let at = format!("{GEN}/{brain}");
        let composite = if brain == "cogny" {
            "templates/cogny/config.json"
        } else {
            // `talky-chat` is a ref to `talky`: the same edges.
            "templates/talky/config.json"
        };
        add_edges(&mut t, &at, &hive_edges(composite), brain);
        add_edges(
            &mut t,
            &format!("{at}/curator"),
            &hive_edges("templates/curator/config.json"),
            "curator",
        );
    }
    for app in [PROBE_APP, QUIET_APP] {
        add_edges(&mut t, MEMBER, &specs(&install_edges(app), app), app);
        add_edges(
            &mut t,
            &format!("{APPS}/{app}"),
            &hive_edges_of(&fixture("gh916_probe_app/config.json")),
            app,
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
/// addressee per step: a fan-out on these roads is a second delivery.
fn walk(table: &EdgeTable, from: &str, headers: Headers) -> (Vec<String>, Headers) {
    walk_until(table, from, headers, "")
}

/// `walk`, stopping where the message reaches `stop`.
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

/// Where the roads of this file end: the member's own exit, the memory hive's
/// rim, the channels, a curator's door, an app's cell.
fn is_end(p: &str) -> bool {
    p == MEMBER
        || p == HIVE
        || p == "/m/channels"
        || p.ends_with("/curator/intake")
        || (p.starts_with(APPS) && p.ends_with("/sink"))
}

/// Follow one message through EVERY fan-out and return where each branch
/// arrives: at an end of [`is_end`], or where nothing takes it further. A
/// message no edge takes at all arrives nowhere (an empty list).
fn spread(table: &EdgeTable, from: &str, headers: Headers) -> Vec<(String, Headers)> {
    let mut arrived = Vec::new();
    let mut frontier = vec![(from.to_string(), headers, 0usize)];
    while let Some((here, hs, depth)) = frontier.pop() {
        assert!(depth < 32, "a branch did not settle in 32 hops at {here}");
        if depth > 0 && is_end(&here) {
            arrived.push((here, hs));
            continue;
        }
        let out = apply_edges(table, &Path::new(&here), &hs);
        if out.is_empty() {
            if depth > 0 {
                arrived.push((here, hs));
            }
            continue;
        }
        for d in out {
            frontier.push((d.target.as_str().to_string(), d.headers_out, depth + 1));
        }
    }
    arrived.sort_by(|a, b| a.0.cmp(&b.0));
    arrived
}

fn at<'a>(arrived: &'a [(String, Headers)], path: &str) -> Vec<&'a Headers> {
    arrived
        .iter()
        .filter(|(p, _)| p == path)
        .map(|(_, h)| h)
        .collect()
}

fn paths(arrived: &[(String, Headers)]) -> Vec<&str> {
    arrived.iter().map(|(p, _)| p.as_str()).collect()
}

/// The context a round of `brain` runs under.
fn round_context(brain: &str) -> Value {
    let mut c = json!({"assistant": GENERATION, "session_id": "S-916", "turn_id": ROUND,
                       "iter": "0", "audience_set": AUDIENCE, "channel": "chat:916"});
    if brain == "talky-chat" {
        c["channel_node"] = json!("chat");
    }
    c
}

/// One section as a brain's splitter emits it (`header` of its per-section
/// message).
fn section_hop(section: &str) -> Map<String, Value> {
    as_map(&json!({"route": "sidecar", "section": section, "turn_id": ROUND}))
}

#[test]
fn the_probe_app_declares_what_the_reviewed_golden_renders() {
    if !shipped() {
        return;
    }
    let golden = read_json(&fixture("gh916_install_edges.json"));
    assert_eq!(
        declaration(PROBE_APP),
        golden["declaration"],
        "the fixture app declares what the golden was reviewed for"
    );
    let set = |v: &[Value]| -> BTreeSet<String> { v.iter().map(|e| e.to_string()).collect() };
    let got = install_edges(PROBE_APP);
    let want = golden["edges"].as_array().expect("edges").clone();
    assert_eq!(set(&got), set(&want), "rendered: {got:#?}");
}

#[test]
fn a_section_reaches_the_app_that_offered_it_once_from_every_brain() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let probe_sink = format!("{APPS}/{PROBE_APP}/sink");
    let quiet_sink = format!("{APPS}/{QUIET_APP}/sink");
    for brain in BRAINS {
        let from = format!("{GEN}/{brain}/splitter");
        let start = Headers::from_parts(as_map(&round_context(brain)), section_hop(SECTION));
        let (trace, arrived) = walk(&t, &from, start);
        assert_eq!(
            trace.last().map(String::as_str),
            Some(probe_sink.as_str()),
            "{brain}: the section `{SECTION}` must reach the cell the app offered it at, on \
             one path: {trace:?}"
        );
        assert!(
            !trace.iter().any(|p| p.contains(QUIET_APP)),
            "{brain}: {trace:?}"
        );
        assert_eq!(hop_of(&arrived, "route"), "sidecar");
        assert_eq!(hop_of(&arrived, "section"), SECTION);
        assert_eq!(
            hop_of(&arrived, "turn_id"),
            ROUND,
            "{brain}: the round the section answered travels with it"
        );
        assert_eq!(ctx_of(&arrived, "session_id"), "S-916");
        assert_eq!(ctx_of(&arrived, "audience_set"), AUDIENCE);

        // The counter-probe is wired: its own section reaches it, and only it.
        let start = Headers::from_parts(as_map(&round_context(brain)), section_hop(OTHER_SECTION));
        let (trace, _) = walk(&t, &from, start);
        assert_eq!(
            trace.last().map(String::as_str),
            Some(quiet_sink.as_str()),
            "{brain}: {trace:?}"
        );
        assert!(
            !trace.iter().any(|p| p.contains(PROBE_APP)),
            "{brain}: {trace:?}"
        );
    }
}

/// The advice sections of a duplex call (`fact`, `context`, `correction`) are
/// what the CALLER hears next: the member turns them into the channel's
/// `in_advise`. The core's collector assembles the advise mode too whenever
/// `context.engine == "duplex"` (no edge deletes it on the consult roads), so a
/// core that wrote such a section would speak to the caller ahead of, or on top
/// of, the voice that consulted it. Only the voices' advice reaches the
/// channel; the core's section of any other name still leaves (the app road).
#[test]
fn the_cores_advice_never_reaches_the_channel_and_a_voices_still_does() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for section in ["fact", "context", "correction"] {
        for brain in BRAINS {
            let mut ctx = round_context(brain);
            ctx["channel_node"] = json!("voice");
            ctx["engine"] = json!("duplex");
            let from = format!("{GEN}/{brain}/splitter");
            let start = Headers::from_parts(as_map(&ctx), section_hop(section));
            let arrived = spread(&t, &from, start);
            let at_channels = at(&arrived, "/m/channels");
            if brain == "cogny" {
                assert!(
                    at_channels.is_empty(),
                    "cogny's `{section}` must never reach the caller: {:?}",
                    paths(&arrived)
                );
            } else {
                assert_eq!(
                    at_channels.len(),
                    1,
                    "{brain}'s `{section}` reaches the channel once: {:?}",
                    paths(&arrived)
                );
                assert_eq!(hop_of(at_channels[0], "route"), "in_advise");
            }
        }
    }
}

#[test]
fn the_memory_section_reaches_the_memory_and_never_an_app() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for brain in BRAINS {
        // Out of the splitter the memory section is the curator's.
        let from = format!("{GEN}/{brain}/splitter");
        let start = Headers::from_parts(as_map(&round_context(brain)), section_hop("memory"));
        let (trace, arrived) = walk(&t, &from, start);
        assert_eq!(
            trace.last().map(String::as_str),
            Some(format!("{GEN}/{brain}/curator/intake").as_str()),
            "{brain}: {trace:?}"
        );
        assert_eq!(hop_of(&arrived, "route"), "in_section");

        // What the curator hands on: a voice's reaches the memory hive once and
        // no app; the core's stays inside the core.
        let from = format!("{GEN}/{brain}/curator/intake");
        let start = Headers::from_parts(
            as_map(&round_context(brain)),
            as_map(&json!({"route": "sidecar", "section": "memory"})),
        );
        let arrived = spread(&t, &from, start);
        assert!(
            !paths(&arrived).iter().any(|p| p.starts_with(APPS)),
            "{brain}: the memory section reached an app: {:?}",
            paths(&arrived)
        );
        let at_hive = at(&arrived, HIVE);
        if brain == "cogny" {
            assert!(
                at_hive.is_empty(),
                "cogny's memory section stays with its curator: {:?}",
                paths(&arrived)
            );
        } else {
            assert_eq!(at_hive.len(), 1, "{brain}: {:?}", paths(&arrived));
            assert_eq!(hop_of(at_hive[0], "route"), "in_remember");
        }
    }
}

#[test]
fn the_close_batch_reaches_the_listening_app_once_and_its_other_readers_still() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    for brain in ["talky", "talky-chat"] {
        let from = format!("{GEN}/{brain}/curator/writer");
        let start = Headers::from_parts(
            as_map(&round_context(brain)),
            as_map(&json!({"route": "write"})),
        );
        let arrived = spread(&t, &from, start);
        let seen = paths(&arrived);

        let heard = at(&arrived, &format!("{APPS}/{PROBE_APP}/sink"));
        assert_eq!(
            heard.len(),
            1,
            "{brain}: the app hears the close once: {seen:?}"
        );
        assert_eq!(hop_of(heard[0], "route"), "in_close");
        assert_eq!(ctx_of(heard[0], "session_id"), "S-916");
        assert_eq!(ctx_of(heard[0], "audience_set"), AUDIENCE);
        assert_eq!(ctx_of(heard[0], "channel"), "chat:916");

        let pass = at(&arrived, HIVE);
        assert_eq!(
            pass.len(),
            1,
            "{brain}: the close pass still runs: {seen:?}"
        );
        assert_eq!(hop_of(pass[0], "route"), "in_close_pass");

        let out = at(&arrived, MEMBER);
        assert_eq!(
            out.len(),
            1,
            "{brain}: the member's own exit stands: {seen:?}"
        );
        assert_eq!(hop_of(out[0], "route"), "write");

        assert!(
            !seen.iter().any(|p| p.contains(QUIET_APP)),
            "{brain}: an app that does not listen to `close` heard it: {seen:?}"
        );
    }
}

#[test]
fn a_pin_reaches_the_curator_of_every_brain_once() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let ctx = as_map(&json!({"audience_set": AUDIENCE, "channel": "chat:916"}));
    let from = format!("{APPS}/{PROBE_APP}/sink");
    let arrived = spread(
        &t,
        &from,
        Headers::from_parts(ctx.clone(), as_map(&json!({"route": "pin"}))),
    );
    let want: Vec<String> = {
        let mut v: Vec<String> = BRAINS
            .iter()
            .map(|b| format!("{GEN}/{b}/curator/intake"))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        paths(&arrived),
        want.iter().map(String::as_str).collect::<Vec<_>>(),
        "the pin reaches the curator of each brain, once each, and nothing else"
    );
    for (p, h) in &arrived {
        assert_eq!(hop_of(h, "route"), "in_pin", "{p}");
        assert_eq!(
            ctx_of(h, "audience_set"),
            AUDIENCE,
            "{p}: the round a pin was made in is not rewritten on its way"
        );
    }

    // An app that declares no `pins` has no edge from its cell: the pin is
    // taken by nothing (the colony dead-letters it).
    let quiet = apply_edges(
        &t,
        &Path::new(&format!("{APPS}/{QUIET_APP}/sink")),
        &Headers::from_parts(ctx, as_map(&json!({"route": "pin"}))),
    );
    assert!(
        quiet.is_empty(),
        "a pin of an app without `pins` went somewhere: {:?}",
        quiet.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn an_answer_without_a_channel_still_leaves_the_member_once() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let mut ctx = round_context("talky");
    ctx.as_object_mut().expect("object").remove("channel_node");
    let (trace, arrived) = walk(
        &t,
        &format!("{GEN}/talky"),
        Headers::from_parts(as_map(&ctx), as_map(&json!({"route": "answer"}))),
    );
    assert_eq!(
        trace.last().map(String::as_str),
        Some(MEMBER),
        "the member's default exit for an answer stands: {trace:?}"
    );
    assert_eq!(hop_of(&arrived, "route"), "answer");
    assert!(!trace.iter().any(|p| p.starts_with(APPS)), "{trace:?}");
}

// ═══════════════════════════════════════════════ 2. the colony (stub models)

/// The shipped template, copied the way instantiation lays it out (the
/// `copy_resolved` of `gh889`/`gh895`).
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

/// One stub per brain, one for everything else.
struct Stubs {
    talky: String,
    talky_chat: String,
    cogny: String,
    background: String,
}

/// Point EVERY `llm` cell of the tree at a local stub: each brain at its own,
/// all others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, stubs: &Stubs) -> Vec<String> {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let brains: BTreeMap<String, &String> = [
        ("talky", &stubs.talky),
        ("talky-chat", &stubs.talky_chat),
        ("cogny", &stubs.cogny),
    ]
    .into_iter()
    .map(|(b, url)| (format!("assistants/{GENERATION}/{b}/brain"), url))
    .collect();
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
        let (url, model) = match brains.get(&rel) {
            Some(url) => (*url, BRAIN_MODEL),
            None => (&stubs.background, BACKGROUND_MODEL),
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0916_0000 + n));
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

/// The member's own edges this road needs, verbatim off `templates/member`:
/// everything between its generations and its memory (the recall door, the
/// memory section, the episodes, the close pass, the menu), and the one edge
/// that hands a section to its apps.
fn member_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (e["from"].as_str(), e["to"].as_str());
            let sidecar = e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'sidecar'"));
            matches!(
                (f, t),
                (Some("./assistants"), Some("./memory-hive"))
                    | (Some("./memory-hive"), Some("./assistants"))
            ) || (f == Some("./assistants") && t == Some("./apps") && sidecar)
        })
        .collect()
}

/// Where each app's cell writes what it hears.
struct Logs {
    probe: std::path::PathBuf,
    quiet: std::path::PathBuf,
}

/// The member stand-in: `./assistants` is the container, grown with the
/// generation `sam` the way `examples/organism/grow-assistant.json` grows it;
/// `./memory-hive` is the person's memory; `./apps` holds both apps; the
/// member's own edges and the ones `install_app` renders for both apps stand
/// in the member's graph; everything else that leaves is drained.
fn build_member(td: &tempfile::TempDir, stubs: &Stubs) -> Logs {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join(format!("assistants/{GENERATION}")),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": container_edges()}}}),
    );
    for s in ["talky", "talky-chat"] {
        // A sweep closes every channel that has been silent at all.
        override_params_on_disk(
            &main.join(format!("assistants/{GENERATION}/{s}/session-keeper/close")),
            &json!({"idle_ms": 1}),
        );
        // No member affinity stands here: the brief leg is not this road.
        override_params_on_disk(
            &main.join(format!("assistants/{GENERATION}/{s}/collector/assemble")),
            &json!({"brief_slots": []}),
        );
    }
    // The apps container as the member ships it: open, no edges of its own.
    write_json(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    let logs = Logs {
        probe: root.join("probe-app.jsonl"),
        quiet: root.join("quiet-app.jsonl"),
    };
    for (app, log) in [(PROBE_APP, &logs.probe), (QUIET_APP, &logs.quiet)] {
        copy_resolved(
            &fixture("gh916_probe_app"),
            &main.join(format!("apps/{app}")),
            0,
        );
        override_params_on_disk(
            &main.join(format!("apps/{app}/sink")),
            &json!({"log_path": log.to_string_lossy()}),
        );
    }

    let mut edges = member_edges();
    for app in [PROBE_APP, QUIET_APP] {
        edges.extend(install_edges(app));
    }
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    // A section no installed app offered (the handover's, say) ends here.
    edges.push(json!({"from": "./apps", "to": "/park",
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
    let pointed = point_llms_at_stubs(&main, stubs);
    for b in BRAINS {
        let brain = format!("assistants/{GENERATION}/{b}/brain");
        assert!(
            pointed.contains(&brain),
            "{brain} is not an llm cell of the tree: {pointed:?}"
        );
    }
    write_env(root, &main, stubs);
    logs
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
            // never called -- no stub brain calls one of its tools.
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
        .expect("the generation, the member road, the memory hive and both apps must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

async fn background_stub() -> MockOpenAI {
    MockOpenAI::start(
        (0..64)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await
}

/// The turn id the channel of `surface` minted for its one turn of this file,
/// as the member's firewall hands it on (`context.turn_id`).
fn channel_turn(surface: &str) -> String {
    format!("T-916-{surface}")
}

/// A person's words at the container's door, on the channel of `surface`, with
/// the turn id its channel minted.
fn person(surface: &str, text: &str) -> Message {
    let mut ctx = json!({"assistant": GENERATION, "channel": format!("{surface}:916"),
                         "audience_set": AUDIENCE, "turn_id": channel_turn(surface)});
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
    MessageBuilder::new(Path::new(&format!("/assistants/{GENERATION}/{surface}")))
        .hop(as_map(&json!({"route": "in_sweep"})))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

/// What an app's cell is commanded to pin, at the cell itself.
fn command_pin(app: &str, text: &str, source: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/apps/{app}/sink")))
        .hop(as_map(&json!({"route": "command_pin"})))
        .context(as_map(
            &json!({"audience_set": AUDIENCE, "channel": "chat:916"}),
        ))
        .body(Body::Inline(
            json!({"messages": [], "pins": [{"text": text, "source": source}]}),
        ))
        .ttl(400)
        .build()
}

/// A brain's answer: a few words and the block, with the section this file
/// offers (tagged with the brain that wrote it) and the memory's nothing form.
/// A section's value is always an object -- the splitter drops a list.
fn with_block(text: &str, brain: &str) -> String {
    let block = json!({
        SECTION: {"entries": [{"brain": brain, "note": "noted for the app"}]},
        "memory": {"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}
    });
    format!("{text}\n\n```sidecar\n{block}\n```")
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn said(m: &Message) -> String {
    body_of(m)["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `(error_code, sender, resolved_target, hop.route)` of every dead letter.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(
        "SELECT error_code, sender_path, resolved_target, message_json \
         FROM dead_letters ORDER BY id",
    ) else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            .map(|(code, sender, target, msg)| {
                let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
                let route = m["headers"]["hop"]["route"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                (code, sender, target, route)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The next answer that leaves the generation and says `needle`.
async fn answer_saying(ports: &mut Ports, root: &std::path::Path, needle: &str) -> Message {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.sink.recv()).await {
            Ok(Some(m)) => {
                if said(&m).contains(needle) {
                    return m;
                }
                seen.push(said(&m));
            }
            _ => panic!(
                "no answer saying {needle:?} left the generation within {DEADLINE:?}. \
                 Answers so far: {seen:#?}. Dead letters: {:#?}",
                dead_letters(root)
            ),
        }
    }
}

/// Every line an app's cell wrote; a cell that heard nothing wrote no file.
fn log_lines(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            meclaw_core::serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("{}: a line that is no JSON ({e}): {l}", p.display()))
        })
        .collect()
}

fn is_section(line: &Value, section: &str) -> bool {
    line["route"] == "sidecar" && line["hop"]["section"] == json!(section)
}

/// Until `p` holds at least `n` lines that `pick` takes, or the deadline says
/// what did not arrive.
async fn until_lines(
    root: &std::path::Path,
    p: &std::path::Path,
    pick: impl Fn(&Value) -> bool,
    n: usize,
    what: &str,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let lines = log_lines(p);
        if lines.iter().filter(|&l| pick(l)).count() >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: fewer than {n} line(s) within {DEADLINE:?}. The app heard: {:#?}. \
             Dead letters: {:#?}",
            lines
                .iter()
                .map(|l| (
                    l["route"].clone(),
                    l["hop"]["section"].clone(),
                    l["reply_to"].clone()
                ))
                .collect::<Vec<_>>(),
            dead_letters(root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

fn ledger(td: &tempfile::TempDir, brain: &str) -> std::path::PathBuf {
    td.path().join(format!(
        "main/assistants/{GENERATION}/{brain}/curator/ledger/cell.db"
    ))
}

/// The live pins of `source` in a curator's ledger: an `until` that is empty
/// (the visibility rule of `gh916_a_pin_source_is_replaced`).
fn live_pins(db: &std::path::Path, source: &str) -> usize {
    rows(db, "SELECT source, until FROM pins")
        .into_iter()
        .filter(|r| r[0] == source && r[1].is_empty())
        .count()
}

const CHAT_ASKS: &str = "What is on my list for today?";
const CHAT_SAYS: &str = "Two things: the dentist and the letter.";
const TALKY_ASKS: &str = "Can you work out a plan for the weekend?";
const INTERIM: &str = "Let me ask my core.";
const CONSULT_ID: &str = "call-c916";
const CONSULT_ARGS: &str = r#"{"question": "Plan a weekend for the person.", "context": "Goal: a quiet weekend. Facts: none yet. Constraints: none named. Form: a short plan. Length: keep the answer under 4000 characters when you can."}"#;
const COGNY_SAYS: &str = "Saturday a walk, Sunday a book.";
const TALKY_SAYS: &str = "A walk on Saturday and a book on Sunday.";

/// The section every brain writes reaches the app that offered it, once per
/// brain, with its round; the quiet app hears none of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_hears_its_section_from_every_brain_and_a_quiet_app_hears_none() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let talky = MockOpenAI::start(vec![
        // A sentence for the person and the consult, in one breath: a tool
        // round, which the splitter leaves whole -- no section out of it.
        canned_content_and_tool_calls(INTERIM, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        // The core's advice arrives; the voice says it, with its block.
        canned_chat_completion(&with_block(TALKY_SAYS, "talky"), "stop"),
    ])
    .await;
    let cogny = MockOpenAI::start(vec![canned_chat_completion(
        &with_block(COGNY_SAYS, "cogny"),
        "stop",
    )])
    .await;
    let chat = MockOpenAI::start(vec![canned_chat_completion(
        &with_block(CHAT_SAYS, "talky-chat"),
        "stop",
    )])
    .await;
    let background = background_stub().await;
    let stubs = Stubs {
        talky: talky.base_url.clone(),
        talky_chat: chat.base_url.clone(),
        cogny: cogny.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let logs = build_member(&td, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    h.send(person("talky-chat", CHAT_ASKS)).await;
    answer_saying(&mut ports, &root, CHAT_SAYS).await;
    h.send(person("talky", TALKY_ASKS)).await;
    answer_saying(&mut ports, &root, TALKY_SAYS).await;
    until_lines(
        &root,
        &logs.probe,
        |l| is_section(l, SECTION),
        BRAINS.len(),
        "the section of every brain reaches the app",
    )
    .await;
    tokio::time::sleep(SETTLE).await;
    let probe = log_lines(&logs.probe);
    let quiet = log_lines(&logs.quiet);
    let asked = (
        talky.recorded_requests().await.len(),
        chat.recorded_requests().await.len(),
        cogny.recorded_requests().await.len(),
    );
    h.shutdown().await;

    assert_eq!(
        asked,
        (2, 1, 1),
        "(talky, talky-chat, cogny) model calls: the voice twice around the consult, the \
         chat once, the core once"
    );
    let sections: Vec<&Value> = probe.iter().filter(|l| is_section(l, SECTION)).collect();
    assert_eq!(
        sections.len(),
        BRAINS.len(),
        "one section per brain answer, none twice: {sections:#?}"
    );
    for brain in BRAINS {
        let sender = format!("/assistants/{GENERATION}/{brain}/");
        let mine: Vec<&&Value> = sections
            .iter()
            .filter(|l| {
                l["reply_to"]
                    .as_str()
                    .is_some_and(|r| r.starts_with(&sender))
            })
            .collect();
        assert_eq!(
            mine.len(),
            1,
            "{brain}: exactly one section from this brain: {sections:#?}"
        );
        let line = mine[0];
        assert!(
            line["context"]["session_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "{brain}: the section carries its session: {line}"
        );
        // The turn reference by VALUE (GH #916): a voice's section carries the
        // turn id of the channel turn it answered; the core's carries an id of
        // its own -- the consult edge deletes `turn_id`, so the core's curator
        // runs the consult under its own call id -- never empty and never the
        // voice's.
        let turn = line["hop"]["turn_id"].as_str().unwrap_or("");
        if brain == "cogny" {
            assert!(
                !turn.is_empty()
                    && turn != channel_turn("talky")
                    && turn != channel_turn("talky-chat"),
                "cogny: the section carries the id its consult ran under, not the voice's \
                 turn: {line}"
            );
        } else {
            assert_eq!(
                turn,
                channel_turn(brain),
                "{brain}: the section carries the turn id of the channel turn it answered: {line}"
            );
        }
        let payload = &line["body"]["payload"];
        assert!(
            payload.is_object(),
            "{brain}: the payload is an object: {line}"
        );
        assert_eq!(
            payload["entries"][0]["brain"],
            json!(brain),
            "{brain}: the section is the one this brain's model wrote: {line}"
        );
    }
    assert!(
        !probe
            .iter()
            .any(|l| l["route"] == "sidecar" && l["hop"]["section"] != json!(SECTION)),
        "a section the app did not offer reached it (the memory is the memory hive's): {probe:#?}"
    );
    assert!(
        !quiet
            .iter()
            .any(|l| l["route"] == "sidecar" || l["route"] == "in_close"),
        "the quiet app heard a section it did not offer: {quiet:#?}"
    );
}

/// A closed session reaches the app that listens to `close` exactly once,
/// with its batch; the quiet app hears nothing of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_hears_each_closed_session_once() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let chat = MockOpenAI::start(vec![canned_chat_completion(
        &with_block(CHAT_SAYS, "talky-chat"),
        "stop",
    )])
    .await;
    let background = background_stub().await;
    let stubs = Stubs {
        talky: background.base_url.clone(),
        talky_chat: chat.base_url.clone(),
        cogny: background.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let logs = build_member(&td, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    h.send(person("talky-chat", CHAT_ASKS)).await;
    answer_saying(&mut ports, &root, CHAT_SAYS).await;
    h.send(sweep("talky-chat")).await;
    let sessions = root.join(format!(
        "main/assistants/{GENERATION}/talky-chat/session-keeper/sessions/cell.db"
    ));
    until_rows(
        &sessions,
        "SELECT session_id FROM sessions WHERE closed = 1",
        1,
        "the sweep ends the session",
    )
    .await;
    until_lines(
        &root,
        &logs.probe,
        |l| l["route"] == "in_close",
        1,
        "the closed session reaches the app",
    )
    .await;
    tokio::time::sleep(SETTLE).await;
    let closed: Vec<String> = rows(
        &sessions,
        "SELECT session_id FROM sessions WHERE closed = 1",
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect();
    let probe = log_lines(&logs.probe);
    let quiet = log_lines(&logs.quiet);
    h.shutdown().await;

    assert_eq!(closed.len(), 1, "one session was closed: {closed:?}");
    let heard: Vec<&Value> = probe.iter().filter(|l| l["route"] == "in_close").collect();
    assert_eq!(
        heard.len(),
        closed.len(),
        "exactly one `in_close` per closed session: {heard:#?}"
    );
    let line = heard[0];
    assert_eq!(
        line["context"]["session_id"],
        json!(closed[0]),
        "the close names the session it closed: {line}"
    );
    assert_eq!(line["context"]["audience_set"], json!(AUDIENCE), "{line}");
    assert_eq!(
        line["context"]["channel"],
        json!("talky-chat:916"),
        "{line}"
    );
    assert!(
        line["body"]["messages"]
            .as_array()
            .is_some_and(|m| !m.is_empty()),
        "the close carries the session's batch: {line}"
    );
    assert!(
        !quiet.iter().any(|l| l["route"] == "in_close"),
        "an app that does not listen to `close` heard it: {quiet:#?}"
    );
}

/// What the app pins stands in the curator of every brain, once each; what an
/// app without `pins` tries to pin stands nowhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_pin_stands_in_every_curator() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let background = background_stub().await;
    let stubs = Stubs {
        talky: background.base_url.clone(),
        talky_chat: background.base_url.clone(),
        cogny: background.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let logs = build_member(&td, &stubs);
    let root = td.path().to_path_buf();
    let (h, _ports) = boot(&td).await;

    h.send(command_pin(QUIET_APP, "The quiet app was here.", "quiet"))
        .await;
    h.send(command_pin(
        PROBE_APP,
        "Two things are open today.",
        SECTION,
    ))
    .await;
    for brain in BRAINS {
        let db = ledger(&td, brain);
        let deadline = Instant::now() + DEADLINE;
        while live_pins(&db, SECTION) < 1 {
            assert!(
                Instant::now() < deadline,
                "{brain}: the app's pin does not stand in the curator's ledger within \
                 {DEADLINE:?}: {:?}. The app heard: {:#?}. Dead letters: {:#?}",
                rows(&db, "SELECT source, until FROM pins"),
                log_lines(&logs.probe),
                dead_letters(&root)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    tokio::time::sleep(SETTLE).await;
    let counts: Vec<(&str, usize, usize)> = BRAINS
        .iter()
        .map(|b| {
            let db = ledger(&td, b);
            (
                *b,
                live_pins(&db, SECTION),
                rows(&db, "SELECT source FROM pins WHERE source = 'quiet'").len(),
            )
        })
        .collect();
    let probe = log_lines(&logs.probe);
    h.shutdown().await;

    for (brain, live, quiet) in counts {
        assert_eq!(live, 1, "{brain}: one live pin of source `{SECTION}`");
        assert_eq!(
            quiet, 0,
            "{brain}: an app that declares no `pins` pinned into the curator"
        );
    }
    let commanded: Vec<&Value> = probe
        .iter()
        .filter(|l| l["route"] == "command_pin")
        .collect();
    assert_eq!(
        commanded.len(),
        1,
        "the cell was commanded once: {probe:#?}"
    );
    assert_eq!(commanded[0]["context"]["audience_set"], json!(AUDIENCE));
}
