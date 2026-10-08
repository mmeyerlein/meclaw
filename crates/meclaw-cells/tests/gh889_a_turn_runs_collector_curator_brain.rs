//! GH #889 (R-27-1) -- a turn runs collector -> curator -> brain.
//!
//! R-27-1 moves the window out of the collector: the collector gathers a round
//! and hands it on UNCUT on route `curate`, and the curator -- one sealed hive
//! per model -- owns everything the model sees. It builds the window, sends it
//! to the brain on route `brain` under a fresh `curator_call`, hears every
//! answer back through a tap on the brain, and writes the conversation into
//! memory on `turn_write` itself. So `talky` and `cogny` each gained a
//! `./curator` between `./collector` and `./brain`, and the collector's own
//! edges to the brain and to the parent's memory lanes are gone.
//!
//! A config pin cannot see whether that seam carries a turn: the edges are
//! drawn in one file, the lanes they stamp are read by two templates, and the
//! hop keys the edges promote are minted by a third. So this file boots the
//! SHIPPED composites with a stub provider behind the brain and measures where
//! things arrive, not where they leave:
//!
//! 1. **The seam.** A turn with one tool round reaches the curator on
//!    `in_curate` once per provider call, from the collector; every message the
//!    brain receives comes from the curator and carries a non-empty
//!    `context.curator_call`, one per call; and every answer of the brain comes
//!    back to the curator on `in_llm` under the call it answers. Read off the
//!    colony's `message_log`, i.e. the deliveries themselves.
//! 2. **The round survives the curator.** The provider sees the person's words
//!    on the first call and the tool result under its call id on the second --
//!    the mock refuses a conversation whose tool call is not answered.
//! 3. **talky remembers each turn exactly once.** The memory hive's `in_episode`
//!    lane gets one message per participant turn -- the opening `user` turn and
//!    the final `assistant` answer; the tool call and its result are never
//!    turns -- and its store holds exactly those two `episodes` rows under
//!    `<session_id>#<tag>-0` and `<session_id>#<tag>-1`, `tag` the round's
//!    (GH #932: a per-round index, so the id no longer counts other rounds'
//!    turns). That is the `turn_write` contract of `collector@4.4.1`, one
//!    message per turn, with the id form GH #932 gave it.
//! 4. **cogny remembers nothing.** Its curator ships `writer.turn_write "0"`
//!    (OR-KX-V3; before, every consult turn left a `turn_write` nobody drained),
//!    and no message on `turn_write` travels anywhere in the run.
//! 5. **No dead letter** in the talky and the cogny run.
//! 6. **A briefed peer turn is named on the wire** (lock B-9 of the M1 review,
//!    review I-4). One turn of the other side's words on a channel whose
//!    collector briefs: the `who` the brief names reaches the provider as the
//!    frame `[peer <ref> · <name>]` of that turn and as the legend line
//!    `<ref> = <name> (<identity>)` in the system part -- the peer row's
//!    `speaker`/`speaker_ref` and the collector's `system.roster` and
//!    `system.instructions.peer` survive the curator.
//!
//! The harness is the parent a composite is grown into, reduced to its lanes:
//! every lane the composite (and the memory hive) declares at its rim is drained
//! -- derived from the templates' own contracts, not listed here -- the tool the
//! stub asks for is one `code` cell, and the talky's `turn_write` reaches the
//! memory hive through the member's own edge, read off `templates/member`. For
//! the briefed turn the parent also answers the brief: the member's own two
//! brief edges (GH #834), read off `templates/member` as well, carry it to a
//! stand-in for affinity and back.
//!
//! Free of a real provider by construction: EVERY `llm` cell of the booted tree
//! is pointed at one of two local stubs -- the brain at its scripted one, every
//! other (the curator's summarizer, the memory hive's night and recall models)
//! at a background stub -- and the memory hive's embedder at the background
//! stub too. Nothing here can reach a paid endpoint.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry one of the templates this road needs is skipped, never judged.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long a run is left alone after its last expected arrival before
/// "exactly" and "nothing else" are read: long enough that a late duplicate or
/// a late dead letter would have landed.
const SETTLE: Duration = Duration::from_secs(3);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const SAID: &str = "which way does the river run past the old mill?";
const TOOL_CALL_ID: &str = "call-889";
const TOOL_ARGS: &str = r#"{"q":"river past the old mill"}"#;
const LOOKUP_RESULT: &str = "the river runs north past the old mill";
const ANSWER: &str = "It runs north, past the old mill.";

/// The round the talky turn is spoken in, in the affinity vocabulary. The
/// memory hive refuses an episode without it (#244).
const CHANNEL: &str = "chat:889";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;

const CONSULT_ID: &str = "k-889";
const CONSULT_SESSION: &str = "s-889";
const QUESTION: &str = r#"{"question":"which way does the river run past the old mill?"}"#;

/// The briefed peer turn (B-9): the other side's words on a channel with one
/// counterpart, the counterpart the channel's entry edge stamps (GH #834), and
/// the assistant the member's brief edge names the asker by.
const PEER_SAID: &str = "hello from the north";
const PEER_REPLY: &str = "Hello, North.";
const PEER_CHANNEL: &str = "peer-channel:north";
const PEER_AUDIENCE: &str = r#"["agent:alpha","peer:north"]"#;
const COUNTERPART: &str = "peer:north";
const ASSISTANT: &str = "alpha";
/// The `who` the brief names the counterpart with. The reference is
/// `participant_ref("north")`, the first 8 hex characters of sha256 of the
/// identity (`templates/affinity/README.md` § Who is speaking) -- what affinity
/// answers for this counterpart; that it does is
/// `gh848_affinity_names_who_is_speaking.rs`.
const NORTH_REF: &str = "dc365e79";
const NORTH_NAME: &str = "North";
const NORTH_IDENTITY: &str = "north";

/// Dead letters a run of this file may leave behind, as `(error_code,
/// resolved_target suffix, why)`. Every entry has to say why it is expected.
/// None is: the curator drains every lane it emits into its composite, the
/// composites drain every lane of their inner cells, and this harness drains
/// every lane the composites and the memory hive declare at their rims.
const EXPECTED_DEAD_LETTERS: &[(&str, &str, &str)] = &[];

// ───────────────────────────────────────────────────────────── the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this road boots or reads. One missing = skipped (GH #49).
fn shipped() -> bool {
    [
        "talky",
        "cogny",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
        "memory-hive",
        "member",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
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

/// The shipped template, copied the way instantiation lays it out: every
/// `config.json` and the seeds beside them. A `cell.type: "ref"` directory is a
/// REFERENCE (GH #277) -- the referenced template's tree takes its place, and
/// the marker's own `override_params` are then applied to the cells they name,
/// addressed by the cell's path inside the referenced template (GH #140), which
/// is what the mutation door does to a staged tree. That is how `cogny`'s
/// curator arrives with `writer.turn_write "0"` and `talky`'s collector with its
/// tool scope, exactly as a grown colony carries them.
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

/// Every `config.json` below `dir`, seeds excluded.
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

/// The two local provider stubs a run talks to.
struct Stubs {
    brain: String,
    background: String,
}

/// Point EVERY `llm` cell of the tree at a local stub: `brain` (the cell path
/// under `main/`) at the scripted one, all others at the background one.
/// Returns the path of every cell it pointed, so a run can say which models it
/// has and prove the list is not empty where it must not be.
///
/// `${ctx.model}` is an instantiation-side substitution; a tree booted from
/// disk has to be told which model to name, and here that is the stub's.
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

/// Every timer of the tree, out of the run's way: a cron moves to a date no run
/// reaches, and a `${uuid7:*}` schedule id -- an instantiation-side
/// substitution -- becomes a fixed one. Swept rather than named, so a clock a
/// template adds is quieted without this file learning its name.
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0889_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file. Every `${VAR}` the tree references without a
/// default is bound to a dummy; the ones that name an endpoint are bound to the
/// background stub, so a code cell that calls out (the memory hive's embedder)
/// calls the stub and nothing else.
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

// ────────────────────────────────────────────────────────────── the harness

/// The lanes a template declares at its own path, in declaration order. A lane
/// carrying `at` docks somewhere else (ADR-0020) and is not the rim's.
fn rim_emits(template: &str) -> Vec<String> {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    cfg["params"]["contract"]["emits"]
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

/// One plain drain per lane: `answer` to the capture the run waits on,
/// everything else to the park. An undrained lane is a dead letter, and the
/// dead-letter assertion would then be reading a silence.
fn drains(from: &str, lanes: &[String]) -> Vec<Value> {
    lanes
        .iter()
        .map(|lane| {
            let to = if lane == "answer" { "/sink" } else { "/park" };
            json!({"from": from, "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

/// The one per-instance tool lane: which cell answers `lookup`. Reads the lane
/// first (driver ruling W7-R4, GH #286).
fn tool_edges(composite: &str) -> Vec<Value> {
    vec![
        json!({"from": composite, "to": "./lookup",
               "condition": "has(hop.route) && hop.route == 'tool' \
                             && has(hop.tool_name) && hop.tool_name == 'lookup'"}),
        json!({"from": "./lookup", "to": composite,
               "condition": "has(hop.route) && hop.route == 'res'",
               "modifier": {"set_hop": {"route": "'in_tool'"}}}),
    ]
}

/// The member's own episode edge (GH #527), re-pointed from `./assistants` to
/// the composite this harness grows: `turn_write` becomes the memory hive's
/// `in_episode`, with `turn_id` promoted off the hop. Read off the shipped
/// member so the harness and the member cannot drift apart.
fn member_episode_edge(from: &str) -> Value {
    let member = read_json(&repo("templates/member/config.json"));
    let mut edge = member["params"]["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| {
            e["from"] == json!("./assistants")
                && e["to"] == json!("./memory-hive")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'turn_write'"))
        })
        .cloned()
        .expect("the member draws turn_write -> memory-hive `in_episode` (GH #527)");
    edge["from"] = json!(from);
    edge
}

/// The tool the stub brain asks for: it answers at once, under the call id it
/// was handed, so the round can close.
fn lookup_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "external_timeout_ms": 10000,
                   "result_text": LOOKUP_RESULT,
                   "script_inline": r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
call_id = str(hop.get("tool_call_id") or "")
sys.stdout.write(json.dumps({
    "header": {"route": "res", "tool_call_id": call_id},
    "messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
                  "text": str(doc["params"]["result_text"])}]}))
"#},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": ["res"], "required": false},
                        "tool_call_id": {"type": "string", "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in: the one tool the stub brain asks for.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn hive(edges: Vec<Value>) -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// talky grown into a parent that holds a memory hive, the way a member holds
/// both. Returns the `llm` cells it pointed at the stubs.
fn build_talky(td: &tempfile::TempDir, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/talky"), &main.join("talky"), 0);
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    write_json(&main.join("lookup/config.json"), &lookup_cell());
    let mut edges = drains("./talky", &rim_emits("talky"));
    edges.extend(tool_edges("./talky"));
    edges.push(member_episode_edge("./talky"));
    edges.extend(drains("./memory-hive", &rim_emits("memory-hive")));
    write_json(&main.join("config.json"), &hive(edges));
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, "talky/brain", stubs);
    write_env(root, &main, stubs);
    pointed
}

/// cogny grown into a parent that asks it. There is no memory beside it: cogny
/// declares no `turn_write` at its rim, so what this run pins is that nothing
/// on that lane travels anywhere at all.
fn build_cogny(td: &tempfile::TempDir, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/cogny"), &main.join("cogny"), 0);
    write_json(&main.join("lookup/config.json"), &lookup_cell());
    let mut edges = drains("./cogny", &rim_emits("cogny"));
    edges.extend(tool_edges("./cogny"));
    write_json(&main.join("config.json"), &hive(edges));
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, "cogny/brain", stubs);
    write_env(root, &main, stubs);
    pointed
}

/// The member's two brief edges (GH #834), re-pointed from `./assistants` to
/// `surface`: edge A turns the surface's `brief` into affinity's `in_brief` and
/// stamps the turn, the asker and `brief_caller 'inside'`; edge B restamps
/// affinity's answer as `in_briefing` with `hop.brief_outcome`. Read off the
/// shipped member, so the stamps this run relies on are the member's own.
fn member_brief_edges(surface: &str) -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    let edges = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let find = |from: &str, to: &str, needle: &str| {
        edges
            .iter()
            .find(|e| {
                e["from"] == json!(from)
                    && e["to"] == json!(to)
                    && e["condition"].as_str().is_some_and(|c| c.contains(needle))
            })
            .cloned()
    };
    let mut ask = find("./assistants", "./affinity", "'brief'")
        .expect("the member draws a surface's `brief` -> affinity `in_brief` (GH #834, edge A)");
    let mut back = find("./affinity", "./assistants", "'inside'")
        .expect("the member draws affinity's answer -> `in_briefing` (GH #834, edge B)");
    ask["from"] = json!(surface);
    back["to"] = json!(surface);
    vec![ask, back]
}

/// Test stand-in for affinity (GH #834/#848): it answers the one `tool_call`
/// it is asked with the way `affinity/brief` answers the tool lane -- the
/// `tool_result` under the call id it was sent, affinity's hop keys with the
/// subject and the slots echoed, and the `who` block beside `messages` -- and
/// names the counterpart with the `who` this file hands it. What affinity
/// computes there is `gh848_affinity_names_who_is_speaking.rs`; what this file
/// pins is where that `who` arrives.
fn affinity_stand_in() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "external_timeout_ms": 10000,
                   "who_ref": NORTH_REF, "who_name": NORTH_NAME,
                   "who_identity": NORTH_IDENTITY,
                   "script_inline": r#"
import sys, json
doc = json.load(sys.stdin)
p = doc["params"]
ask = ((doc.get("body") or {}).get("messages") or [{}])[0]
req = json.loads(str(ask.get("text") or "{}"))
slots = [str(s) for s in (req.get("slots") or [])]
sys.stdout.write(json.dumps({
    "header": {"route": "answer", "phase": "", "subject": str(req.get("subject") or ""),
               "audience": "", "channel": str(req.get("channel") or ""),
               "slots": json.dumps(slots), "subscriber": "", "carry": ""},
    "messages": [{"origin": "tool", "type": "tool_result", "id": str(ask.get("id") or ""),
                  "text": "affinity brief on %s (peer): slots %s"
                          % (p["who_name"], ", ".join(slots))}],
    "who": {"ref": str(p["who_ref"]), "name": str(p["who_name"]),
            "identity": str(p["who_identity"]), "known": True}}))
"#},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "who": {"type": "object", "required": false}},
                "hop": {"route": {"type": "string", "values": ["answer"], "required": false},
                        "phase": {"type": "string", "required": false},
                        "subject": {"type": "string", "required": false},
                        "audience": {"type": "string", "required": false},
                        "channel": {"type": "string", "required": false},
                        "slots": {"type": "string", "required": false},
                        "subscriber": {"type": "string", "required": false},
                        "carry": {"type": "string", "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in: answers a brief the way affinity's tool lane does.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// talky with the brief leg switched on (`brief_slots`, the knob both surfaces
/// of the shipped assistant set), grown into a parent that answers the brief
/// the way a member does: the member's own brief edges, with a stand-in for
/// affinity at their far end. No memory hive: this run pins the wire, and the
/// talky's `turn_write` is drained like every other rim lane.
fn build_briefed_talky(td: &tempfile::TempDir, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/talky"), &main.join("talky"), 0);
    override_params_on_disk(
        &main.join("talky/collector/assemble"),
        &json!({"brief_slots": ["peer", "channel"]}),
    );
    write_json(&main.join("affinity/config.json"), &affinity_stand_in());
    // `brief` travels on the member's edge, not into the park.
    let lanes: Vec<String> = rim_emits("talky")
        .into_iter()
        .filter(|lane| lane != "brief")
        .collect();
    let mut edges = drains("./talky", &lanes);
    edges.extend(member_brief_edges("./talky"));
    write_json(&main.join("config.json"), &hive(edges));
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, "talky/brain", stubs);
    write_env(root, &main, stubs);
    pointed
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    /// Everything this file does not wait on. Held, not dropped: a capture
    /// whose receiver is gone turns every delivery into a send error.
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
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
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
        .expect("the shipped composites, their curators and the memory hive must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// A person's turn at the talky's door, with the round the channel stamps.
fn talky_turn() -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": SAID}]}),
        ))
        .ttl(400)
        .build()
}

/// A consult at cogny's door, in the shape a talky's dispatcher sends it and
/// with the context the documented ingress edge sets (`templates/cogny/README.md`).
fn consult() -> Message {
    MessageBuilder::new(Path::new("/cogny"))
        .hop(map(json!({"route": "in_turn", "consult_id": CONSULT_ID,
                        "session_id": CONSULT_SESSION, "tool_name": "consult_cogny"})))
        .context(map(json!({"consult_id": CONSULT_ID,
                            "session_id": CONSULT_SESSION, "col_phase": ""})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": CONSULT_ID, "text": QUESTION}]})))
        .ttl(400)
        .build()
}

/// The other side's words at the talky's door, with the round, the counterpart
/// and the assistant the channel's entry edge stamps. The turn carries neither
/// `speaker` nor `speaker_ref` -- the mount strips what a sender wrote there
/// (GH #847) -- so a name or a reference on the wire can only have come from
/// the brief.
fn peer_turn() -> Message {
    let ctx = json!({"channel": PEER_CHANNEL, "audience_set": PEER_AUDIENCE,
                     "counterpart": COUNTERPART, "assistant": ASSISTANT});
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "peer", "type": "text", "text": PEER_SAID}]}),
        ))
        .ttl(400)
        .build()
}

// ──────────────────────────────────────────────────────────────── the readers

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn says(m: &Message, needle: &str) -> bool {
    body_of(m)["messages"].as_array().is_some_and(|ms| {
        ms.iter()
            .any(|t| t["text"].as_str().is_some_and(|s| s.contains(needle)))
    })
}

/// One routing hop as the colony logged it: who sent it, where it was
/// delivered, and the two header compartments it arrived with.
#[derive(Debug)]
struct Logged {
    from: String,
    to: String,
    hop: Value,
    context: Value,
}

impl Logged {
    fn route(&self) -> String {
        text_of(&self.hop, "route")
    }
}

fn message_log(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, headers)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        Logged {
            from,
            to,
            hop: h["hop"].clone(),
            context: h["context"].clone(),
        }
    })
    .collect()
}

/// `(error_code, sender, resolved_target, hop.route)` of every dead letter the
/// colony recorded -- what `/colony/dead_letters` serves -- minus the ones
/// [`EXPECTED_DEAD_LETTERS`] names.
fn unexpected_dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT error_code, sender_path, resolved_target, message_json \
             FROM dead_letters ORDER BY id",
        )
        .expect("dead_letters");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(code, sender, target, msg)| {
        let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
        (code, sender, target, text_of(&m["headers"]["hop"], "route"))
    })
    .filter(|(code, _, target, _)| {
        !EXPECTED_DEAD_LETTERS
            .iter()
            .any(|(c, suffix, _why)| code.as_str() == *c && target.ends_with(*suffix))
    })
    .collect()
}

/// Rows of one query against a cell's own `cell.db`. The store mints its tables
/// when it first wakes, so "no such table" is the honest "nothing written yet".
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
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// `hop.turn_id` of an episode as the curator writer stamps it (GH #932,
/// OR-S3.K.1): `<session>#<tag>-<index>`, `tag` the first 8 hex of sha256
/// over the round in its canonical form (a JSON array, sorted, no duplicates,
/// no whitespace -- the writer's `audience_of`).
fn curator_turn_id(session: &str, round: &str, index: u32) -> String {
    use sha2::{Digest, Sha256};
    let mut names: Vec<String> = meclaw_core::serde_json::from_str(round).expect("a round");
    names.sort();
    names.dedup();
    let canonical = meclaw_core::serde_json::to_string(&names).expect("serialise");
    let tag: String = Sha256::digest(canonical.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{session}#{tag}-{index}")
}

fn episodes(db: &std::path::Path) -> Vec<Vec<String>> {
    rows(
        db,
        "SELECT turn_id, session_id, sender, channel, content FROM episodes ORDER BY rowid",
    )
}

/// The hop of everything that reached the park so far.
fn parked_hops(rx: &mut mpsc::Receiver<Message>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        out.push(Value::Object(m.headers.hop.clone()));
    }
    out
}

async fn answer_or_explain(
    rx: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    what: &str,
) -> Message {
    match tokio::time::timeout(DEADLINE, rx.recv())
        .await
        .ok()
        .flatten()
    {
        Some(m) => m,
        None => panic!(
            "{what}: no answer left the composite within {DEADLINE:?}. Dead letters so far: {:#?}",
            unexpected_dead_letters(root)
        ),
    }
}

/// The calls the stub recorded, once there are at least `n` of them -- or the
/// run's dead letters, if there are not within [`DEADLINE`].
async fn provider_requests(
    mock: &MockOpenAI,
    root: &std::path::Path,
    n: usize,
) -> Vec<OpenAiRequestSnapshot> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reqs = mock.recorded_requests().await;
        if reqs.len() >= n {
            return reqs;
        }
        if Instant::now() >= deadline {
            panic!(
                "the provider was called {} time(s), not {n}, within {DEADLINE:?}. Dead letters \
                 so far: {:#?}",
                reqs.len(),
                unexpected_dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn wire(req: &OpenAiRequestSnapshot) -> String {
    meclaw_core::serde_json::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

/// The provider's side of one tool round: the person's words on the first
/// call, the tool result under its call id on the second -- and no third call.
fn assert_the_round_reached_the_provider(reqs: &[OpenAiRequestSnapshot], opening: &str) {
    assert_eq!(
        reqs.len(),
        2,
        "one tool round is two provider calls -- ask for the tool, then answer; got {}",
        reqs.len()
    );
    assert!(
        wire(&reqs[0]).contains(opening),
        "the curator's first window does not carry the opening turn: {}",
        wire(&reqs[0])
    );
    let second = reqs[1].messages().expect("wire messages");
    let result = second
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == TOOL_CALL_ID)
        .unwrap_or_else(|| {
            panic!(
                "the second window carries no tool result under `{TOOL_CALL_ID}` -- the curator \
                 lost the round the collector handed it: {}",
                wire(&reqs[1])
            )
        });
    assert!(
        result["content"]
            .as_str()
            .is_some_and(|c| c.contains(LOOKUP_RESULT)),
        "the tool result is the lookup's own answer: {result}"
    );
}

/// The seam GH #889 drew, read off the deliveries.
///
/// * the collector hands the curator one `in_curate` per provider call
///   (`curate` is sent exactly when `collector@4.4.1` sent `brain`);
/// * every message the brain receives comes from the curator and carries a
///   non-empty `context.curator_call`, and there are as many distinct calls as
///   the provider saw;
/// * every answer of the brain comes back to the curator on `in_llm` under the
///   call it answers, and every call gets one back.
fn assert_collector_curator_brain(log: &[Logged], composite: &str, calls: usize) {
    let collector = format!("/{composite}/collector");
    let curator = format!("/{composite}/curator");
    let brain = format!("/{composite}/brain");

    let curates: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == curator && r.route() == "in_curate")
        .collect();
    assert_eq!(
        curates.len(),
        calls,
        "{composite}: one `in_curate` per provider call -- the collector hands the curator \
         the round exactly when it used to call the brain: {curates:#?}"
    );
    for r in &curates {
        assert!(
            r.from.starts_with(&collector),
            "{composite}: `in_curate` comes from the collector, not from {}",
            r.from
        );
    }

    let to_brain: Vec<&Logged> = log.iter().filter(|r| r.to == brain).collect();
    assert!(
        !to_brain.is_empty(),
        "{composite}: nothing was ever delivered to the brain"
    );
    for r in &to_brain {
        assert!(
            r.from.starts_with(&curator),
            "{composite}: the curator owns the window (R-27-1) -- every message the brain \
             receives comes through it, and this one came from {}: {r:?}",
            r.from
        );
        assert!(
            !text_of(&r.context, "curator_call").is_empty(),
            "{composite}: a brain request without `context.curator_call` -- the ledger can \
             never name the call this answer belongs to: {r:?}"
        );
    }
    let call_ids: BTreeSet<String> = to_brain
        .iter()
        .map(|r| text_of(&r.context, "curator_call"))
        .collect();
    assert_eq!(
        call_ids.len(),
        calls,
        "{composite}: one fresh `curator_call` per provider call: {call_ids:?}"
    );

    let taps: BTreeSet<String> = log
        .iter()
        .filter(|r| r.to == curator && r.route() == "in_llm")
        .map(|r| text_of(&r.context, "curator_call"))
        .collect();
    assert_eq!(
        taps, call_ids,
        "{composite}: every answer of the brain reaches the curator's tap under the call it \
         answers, and every call is answered there"
    );
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// talky: a person's turn with one tool round runs collector -> curator ->
/// brain, and the memory hive holds exactly one episode per turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_talky_turn_runs_collector_curator_brain_and_writes_one_episode_per_turn() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![(TOOL_CALL_ID, "lookup", TOOL_ARGS)]),
        canned_chat_completion(ANSWER, "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_talky(&td, &stubs);
    assert!(
        pointed.iter().any(|p| p == "talky/brain")
            && pointed.iter().any(|p| p == "talky/curator/summarizer"),
        "the brain and the curator's summarizer are `llm` cells and both talk to a stub \
         here; pointed: {pointed:?}"
    );
    let (h, mut ports) = boot(&td).await;

    h.send(talky_turn()).await;
    let answer = answer_or_explain(&mut ports.sink, td.path(), "talky").await;
    assert!(
        says(&answer, ANSWER),
        "the answer that left the talky is the brain's final one: {:?}",
        body_of(&answer)
    );

    // The episodes are one store write behind the answer.
    let db = td.path().join("main/memory-hive/store/cell.db");
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline && episodes(&db).len() < 2 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(SETTLE).await;

    let eps = episodes(&db);
    let log = message_log(td.path());
    let dead = unexpected_dead_letters(td.path());
    let parked = parked_hops(&mut ports.park);
    let reqs = brain.recorded_requests().await;
    h.shutdown().await;

    // 1. + 2. The round, at the provider and at the seam.
    assert_the_round_reached_the_provider(&reqs, SAID);
    assert_collector_curator_brain(&log, "talky", reqs.len());

    // 3. One message per turn on the memory hive's lane, one row per turn in its
    //    store: the opening `user` turn and the final `assistant` answer.
    let lane: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/memory-hive" && r.route() == "in_episode")
        .collect();
    assert_eq!(
        lane.len(),
        2,
        "one `in_episode` per participant turn -- the user's words and the final answer; \
         the tool call and its result are no turns: {lane:#?}"
    );
    assert_eq!(
        eps.len(),
        2,
        "the memory hive holds exactly one episode per turn: {eps:#?}. Parked: {parked:#?}"
    );
    let session = eps[0][1].clone();
    assert!(
        !session.is_empty() && eps.iter().all(|e| e[1] == session),
        "both episodes belong to the one session of the turn: {eps:#?}"
    );
    let by_turn: BTreeMap<String, &Vec<String>> = eps.iter().map(|e| (e[0].clone(), e)).collect();
    // GH #932 (OR-S3.K.1): the curator writer's id is `<session>#<tag>-<index>`,
    // `tag` the first 8 hex of sha256 over the canonical round, the index
    // counted per (session, round) -- `<session>#<index>` over all rounds let
    // a round count the turns it was not allowed to see.
    let (first, second) = (
        curator_turn_id(&session, AUDIENCE, 0),
        curator_turn_id(&session, AUDIENCE, 1),
    );
    assert_eq!(
        by_turn.keys().cloned().collect::<Vec<_>>(),
        vec![first.clone(), second.clone()],
        "each turn under its own deterministic `<session_id>#<tag>-<index>` and none \
         twice (the `turn_write` contract of collector@4.4.1, GH #932): {eps:#?}"
    );
    let (user, assistant) = (by_turn[&first], by_turn[&second]);
    assert_eq!(user[2], "user", "{user:?}");
    assert_eq!(user[4], SAID, "the person's own words, unframed: {user:?}");
    assert_eq!(assistant[2], "assistant", "{assistant:?}");
    assert!(
        assistant[4].contains(ANSWER),
        "the final answer, not the tool round: {assistant:?}"
    );
    assert!(
        eps.iter().all(|e| e[3] == CHANNEL),
        "the round the turn was spoken in reached the memory through the curator: {eps:#?}"
    );
    assert!(
        eps.iter()
            .all(|e| !e[4].contains(LOOKUP_RESULT) && !e[4].contains(TOOL_ARGS)),
        "a tool call or a tool result became an episode: {eps:#?}"
    );

    // 4. Nothing refused, nothing lost.
    let refused: Vec<&Value> = parked
        .iter()
        .filter(|hop| matches!(hop["route"].as_str(), Some("reject" | "error")))
        .collect();
    assert!(
        refused.is_empty(),
        "a refusal or an error left the run: {refused:#?}"
    );
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// cogny: a consult with one tool round runs collector -> curator -> brain, and
/// nothing is written to any memory -- its curator ships `turn_write "0"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cogny_consult_runs_collector_curator_brain_and_writes_no_episode() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let marker = read_json(&repo("templates/cogny/curator/config.json"));
    assert_eq!(
        marker["override_params"]["writer"]["turn_write"],
        json!("0"),
        "cogny's curator ships with the writer off (OR-KX-V3): the core has no memory lane \
         at its rim, so a `turn_write` would be a dead letter per consult turn"
    );

    let brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![(TOOL_CALL_ID, "lookup", TOOL_ARGS)]),
        canned_chat_completion(ANSWER, "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_cogny(&td, &stubs);
    assert!(
        pointed.iter().any(|p| p == "cogny/brain")
            && pointed.iter().any(|p| p == "cogny/curator/summarizer"),
        "the brain and the curator's summarizer are `llm` cells and both talk to a stub \
         here; pointed: {pointed:?}"
    );
    assert_eq!(
        read_json(&td.path().join("main/cogny/curator/writer/config.json"))["params"]["turn_write"],
        json!("0"),
        "the ref's override reached the writer, as the mutation door applies it"
    );
    let (h, mut ports) = boot(&td).await;

    h.send(consult()).await;
    let advice = answer_or_explain(&mut ports.sink, td.path(), "cogny").await;
    assert!(
        says(&advice, ANSWER),
        "the advice that left cogny is the brain's final answer: {:?}",
        body_of(&advice)
    );
    assert_eq!(
        advice
            .headers
            .context
            .get("consult_id")
            .and_then(Value::as_str),
        Some(CONSULT_ID),
        "the correlation survives the curator -- it is the only way a talky tells one \
         consultation from another: {:?}",
        advice.headers.context
    );
    tokio::time::sleep(SETTLE).await;

    let log = message_log(td.path());
    let dead = unexpected_dead_letters(td.path());
    let parked = parked_hops(&mut ports.park);
    let reqs = brain.recorded_requests().await;
    h.shutdown().await;

    assert_the_round_reached_the_provider(&reqs, "which way does the river run");
    assert_collector_curator_brain(&log, "cogny", reqs.len());

    let written: Vec<&Logged> = log.iter().filter(|r| r.route() == "turn_write").collect();
    assert!(
        written.is_empty(),
        "cogny writes no episode: nothing on `turn_write` travels anywhere in the run: \
         {written:#?}"
    );
    let errors: Vec<&Value> = parked
        .iter()
        .filter(|hop| hop["route"].as_str() == Some("error"))
        .collect();
    assert!(errors.is_empty(), "an error left the run: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// Lock B-9 of the M1 review, after R-27-1: the `who` a brief names reaches the
/// provider -- as the frame of the peer turn and as the line of the legend --
/// now across collector -> curator -> brain.
///
/// B-9 lived in `gh845_two_turns_of_a_session_reach_the_provider_with_one_prefix.rs`
/// beside the prefix pin. The prefix moved to the curator with R-27-1, V deleted
/// that file, and the lock went with it; review I-4 asks for its successor,
/// because neither R-27-1 nor R-27-2 lifts it. The curator is now the cell that
/// owns what the model sees, so it is exactly the one that could lose a peer
/// row's `speaker`/`speaker_ref` or the collector's `system.roster` and
/// `system.instructions.peer` on the way.
///
/// One turn, measured at the receiver: the request body the brain's stub
/// recorded, not what the collector emitted. The brief travels the member's own
/// edges to a stand-in that names the counterpart, and the turn arrives
/// unnamed, so the reference and the name can reach the wire from that `who`
/// and from nowhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_briefed_peer_turn_reaches_the_provider_framed_and_in_the_legend() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = MockOpenAI::start(vec![canned_chat_completion(PEER_REPLY, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_briefed_talky(&td, &stubs);
    assert!(
        pointed.iter().any(|p| p == "talky/brain"),
        "the brain is an `llm` cell and talks to the stub here; pointed: {pointed:?}"
    );
    // Held, not dropped: the answer and the drained lanes need a receiver.
    let (h, _ports) = boot(&td).await;

    h.send(peer_turn()).await;
    let reqs = provider_requests(&brain, td.path(), 1).await;
    let log = message_log(td.path());
    h.shutdown().await;

    // The brief road ran: affinity was asked once, through the member's edge A.
    let asked: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/affinity" && r.route() == "in_brief")
        .collect();
    assert_eq!(
        asked.len(),
        1,
        "one brief per turn, about the counterpart: {asked:#?}"
    );

    let msgs = reqs[0].messages().expect("wire messages");
    // 1. The legend and the rule, in the system part the curator sent.
    let system = msgs
        .iter()
        .find(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default();
    assert!(
        system.contains(&format!(
            "Participants of this channel:\n- {NORTH_REF} = {NORTH_NAME} ({NORTH_IDENTITY})"
        )),
        "the legend names the counterpart by the brief's `who` -- the collector's \
         `system.roster` survived the curator: {}",
        wire(&reqs[0])
    );
    assert!(
        system.contains("is someone else's words") && system.contains("never your person"),
        "the peer rule (`system.instructions.peer`) stands beside the legend: {system}"
    );

    // 2. The peer turn, framed on the wire from the fields the brief filled.
    let peer: Vec<_> = msgs
        .iter()
        .filter(|m| m["content"].to_string().contains(PEER_SAID))
        .collect();
    assert_eq!(
        peer.len(),
        1,
        "the other side's words stand in the window once: {}",
        wire(&reqs[0])
    );
    assert_eq!(peer[0]["role"], "user", "{}", wire(&reqs[0]));
    // Since GH #892 the talky's curator puts the block's short id in front of
    // somebody else's words: `[#<12 hex>] `, inside the frame the llm cell
    // builds around them.
    let content = peer[0]["content"].as_str().unwrap_or_default();
    let frame = format!("[peer {NORTH_REF} \u{b7} {NORTH_NAME}]\n[#");
    let id = content
        .strip_prefix(frame.as_str())
        .and_then(|rest| rest.strip_suffix(&format!("] {PEER_SAID}")))
        .unwrap_or_else(|| {
            panic!(
                "the frame carries the reference and the name of the brief's `who` -- the \
                 peer row's `speaker`/`speaker_ref` survived the curator: {content:?}"
            )
        });
    assert!(
        id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "a short id of 12 hex digits: {content:?}"
    );
}

/// OR-KX.K.17: the curator renders the sidecar's nothing-form into the wall
/// element of a sentence said beside a tool call (#871 I-1) -- and it knows
/// no sidecar language of its own (R-KX-2), so the talky that runs a splitter
/// hands it the SAME form the splitter cuts. talky-chat is a ref onto talky
/// and carries the curator ref with it; a level that overrode the splitter's
/// form without the curator's would split the two again, so no level may.
#[test]
fn the_talky_curator_renders_the_nothing_form_its_splitter_cuts() {
    let splitter = read_json(&repo("templates/talky/splitter/config.json"));
    let form = splitter["params"]["nothing_block"]
        .as_str()
        .expect("the splitter's nothing_block");
    assert!(!form.is_empty(), "talky's splitter speaks a sidecar");
    let curator = read_json(&repo("templates/talky/curator/config.json"));
    assert_eq!(
        curator["cell"]["template"], "curator@1.11.3",
        "the ref this road boots: {curator}"
    );
    assert_eq!(
        curator["override_params"]["intake"]["nothing_block"], form,
        "the curator's nothing-form is the splitter's, byte for byte: {curator}"
    );
    for level in ["assistant/talky", "assistant/talky-chat"] {
        let r = read_json(&repo(&format!("templates/{level}/config.json")));
        let over = r["override_params"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !over
                .keys()
                .any(|k| k == "splitter" || k.starts_with("curator")),
            "{level} overrides neither half, so both stay equal: {over:?}"
        );
    }
    // GH #892: cogny grew the talky's splitter, and its curator renders the
    // form that splitter cuts, byte for byte, the same way.
    let cogny = read_json(&repo("templates/cogny/curator/config.json"));
    let cogny_splitter = read_json(&repo("templates/cogny/splitter/config.json"));
    assert_eq!(
        cogny["override_params"]["intake"]["nothing_block"],
        cogny_splitter["params"]["nothing_block"],
        "cogny's curator renders its splitter's nothing-form: {cogny}"
    );
}
