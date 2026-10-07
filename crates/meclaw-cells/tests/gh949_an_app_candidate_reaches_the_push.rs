//! GH #949 -- an app's push candidate reaches the push of the turn that names
//! it, and the things a turn names leave the generation once.
//!
//! An app that declares `candidates: "./<cell>"` (`install_app`,
//! `gh949_an_app_declares_candidates.rs` locks the rendered edge) hands the
//! generation `{source, candidates: [...]}` on `candidate`, restamped
//! `in_candidate`. The generation fans it out to `./talky`, `./talky-chat` and
//! `./cogny`, each brain hands it to its own curator, and the curator keeps it
//! for the round in `context.audience_set` (`candidates` in its ledger). The
//! curator's push shows a candidate to the model of a LATER turn: only a turn
//! whose round the candidate covers, and -- with triggers -- only one whose
//! words name a trigger. It shows it the way it shows a gap's find: as ONE
//! `memory_recall` call/result pair that `./push` hands `./intake` on
//! `in_addendum`, so the pair stands in the window of THAT round, ahead of its
//! person's words (OR-BC.K.2). The memory question itself stays byte for byte
//! the one curator 1.5.0 asked -- a candidate is no part of what the memory is
//! asked, and nothing an app pushes leaves the hive toward the memory. The
//! road is the enriched ask, so it needs the push on: the voice's curator
//! resolves `recall_push` "1" from its role `talky`
//! (`templates/talky/curator`), and the run reads that off the hop instead of
//! setting it. The other half of the
//! issue rides the same road the other way: a model that writes the section
//! `things` (offered when the curator's `things_section` is "1") gives exactly
//! one `thing_seen` at the generation's rim, and the section never leaves a
//! second time on `sidecar`. `candidate_ack` exists only on request
//! (`hop.ack` "1"): three curators per push must not leave three dead letters.
//!
//! One colony (the member road of `gh916_an_app_hears_its_member.rs`: a
//! generation `sam` of the shipped assistant, the shipped memory hive, an app
//! with one code cell, the edges `install_app` renders written into the
//! member's graph; stub models), measured at the receiver:
//!
//! 1. the candidate stands in the ledger of the curator of every brain, in
//!    the MEMBER's round -- though the app's cell sent it in the universal
//!    one: the round an app's message carries is the app's own word, and the
//!    edge `install_app` draws stamps the member's over it (review I-1; red
//!    before, when the forged `["*"]` reached the guest's turn in (2));
//! 2. the request a turn of a round the candidate does NOT cover puts to the
//!    voice's model carries none of it, though the person names the trigger --
//!    read where the push is used, at the model's stub;
//! 3. the request of the next turn of the covered round, naming the trigger,
//!    carries the candidate's own part `[<text>]` in exactly one element: a
//!    tool result answering a `memory_recall` call, ahead of the person's
//!    words; and the memory question of neither turn carries it -- read at
//!    the memory's door (`in_query`, `context.recall_query`, off the colony's
//!    own `message_log`);
//! 4. the answer of that turn writes `things`: exactly one `thing_seen` leaves
//!    the generation with the round and the item, the splitter routed the
//!    section exactly once and to the curator, and no `sidecar` named `things`
//!    left at all;
//! 5. a push without `hop.ack` is answered by nobody, a push with it by each
//!    of the three curators once -- and no dead letter carries a lane of this
//!    road.
//!
//! The generation's container and the member stand-in are drawn here, as in
//! `gh916`, and two of their edges stand in for what this issue does not draw:
//! `./sam -> .` on `thing_seen` is the member's pickup of a lane the generation
//! emits at its own rim; `./sam/<brain> -> .` with `lane: "candidate_ack"` is
//! the corridor a receiver of receipts draws, one per brain, because the
//! receipt DOCKS at the brain rims (`at`, like `pack_ack`) and never reaches the
//! generation's. Both end at `/rim` through `./assistants -> /rim`. Without the
//! stand-ins a lane that got out would end as `hive_no_route` one level up, and
//! this file could not tell that from a lane that never got there.
//!
//! Red before GH #949: `install_app` refuses the declaration word
//! `candidates` (`app_declaration_invalid`), so the member cannot be built;
//! with the renderer in place and without the generation's fan-out, step 1
//! fails ("the candidate stands in the curator of every brain") and the push
//! dead-letters `hive_no_route` at the generation. A push that puts the
//! candidate in front of the memory question instead of into the window fails
//! step 3 at the model: the text then rides only in the arguments of the
//! ambient `memory_recall` call (the question, echoed by the collector), never
//! in a tool result ahead of the person's words.
//!
//! Free of a real provider by construction: every `llm` cell of the tree talks
//! to a local stub, the memory hive's embedder too.
//!
//! **R2b guard (GH #49 form).** The road reads the shipped templates and the
//! organism example (`copy_resolved` over `templates/`, the `install_app`
//! renderer of `templates/builder/recipes`). Where one of them does not travel
//! into a tree, every test here skips instead of failing on a dead
//! `templates/` reference -- [`shipped`] is the guard, and nothing touches the
//! tree before it said yes.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, emit_all, override_params_on_disk, shipped_script};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[path = "support/organism_assistant.rs"]
mod organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

/// The round the app speaks in, and the one of the person who asks.
const AUDIENCE: &str = r#"["member:owner","agent:sam"]"#;
/// A round the candidate does not cover: another person with the same agent.
/// `{owner, sam}` does not hold it, so no candidate of `AUDIENCE` reaches it.
const OTHER: &str = r#"["member:guest","agent:sam"]"#;
/// The round the app's cell claims (review I-1): the universal one, which
/// would cover `OTHER` too. The install edge overwrites it with the member's.
const FORGED: &str = r#"["*"]"#;

/// The generation every declaration of this file is installed for.
const GENERATION: &str = "sam";
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];
const APP: &str = "remind-app";

/// The candidate: its source, id, trigger and text. The text says nothing the
/// person says, so a question that carries it can only have it from the push.
const SOURCE: &str = "remind";
const CANDIDATE_ID: &str = "router-noon";
const TRIGGER: &str = "router";
const CANDIDATE: &str = "The hall router restarts itself every day at noon.";
/// What the receipt half pushes, under a source of its own: every
/// `candidate_ack` this file sees must name it, never `SOURCE`.
const ACK_SOURCE: &str = "remind-ack";

const OWNER_CHANNEL: &str = "talky:949";
const GUEST_CHANNEL: &str = "talky:949-guest";
const GUEST_ASKS: &str = "Is the router back online?";
const GUEST_HEARS: &str = "I cannot see that from here.";
const OWNER_ASKS: &str = "Why did the router drop again?";
const OWNER_HEARS: &str = "It restarts at noon, give it a minute.";
/// The one thing the owner's answer names in its `things` section.
const THING: &str = "hall router";

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The R2b guard: every template and example this road reads.
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
        "templates/curator/intake/config.json",
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

/// The declaration of the app: one word, `candidates`, naming its one cell.
fn declaration() -> Value {
    json!({"candidates": "./sink"})
}

/// What `install_app` draws for the app at a member, member-relative: the
/// SHIPPED renderer over stdin, as `gh916_an_app_hears_its_member` runs it.
fn install_edges() -> Vec<Value> {
    let out = emit_all(
        &shipped_script(&repo("templates/builder/recipes/config.json").to_string_lossy()),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/os/orgs/acme/members/alex",
                                                    "app": APP,
                                                    "template": format!("{APP}@1.0.0"),
                                                    "screen": "display",
                                                    "generation": GENERATION,
                                                    "ctx": {"member_person": "owner"},
                                                    "declaration": declaration()}})
                                  .to_string()}],
        }),
    );
    let first = out.first().expect("the renderer emits");
    assert!(
        first["header"]["error_code"].is_null(),
        "install_app refused the declaration {}: {first}",
        declaration()
    );
    let edges = first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no add_edges in {first}"))
        .clone();
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == json!(format!("./apps/{APP}/sink"))
                && e["to"] == json!(format!("./assistants/{GENERATION}"))),
        "the declaration draws no edge from the app's cell onto the generation: {edges:#?}"
    );
    edges
}

/// The container as `examples/organism` grows it, with its generation renamed
/// to the one the declaration is installed for.
fn container_edges() -> Vec<Value> {
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
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

/// The shipped template, copied the way instantiation lays it out (the
/// `copy_resolved` of `gh889`/`gh895`/`gh916`): a `ref` resolves to
/// `templates/<name>` and its `override_params` land on the copied cells.
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

/// Point EVERY `llm` cell of the tree at a local stub: the voice's brain at
/// its own, all others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, talky: &str, background: &str) -> Vec<String> {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let voice = format!("assistants/{GENERATION}/talky/brain");
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
        let (url, model) = if rel == voice {
            (talky, BRAIN_MODEL)
        } else {
            (background, BACKGROUND_MODEL)
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0949_0000 + n));
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
fn write_env(root: &std::path::Path, main: &std::path::Path, background: &str) {
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
    vars.insert("MEMORY_LLM_BASE_URL".into(), background.to_string());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{background}/v1/embeddings"),
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
/// off `templates/member` -- the recall door this file reads the push at.
fn member_memory_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            matches!(
                (e["from"].as_str(), e["to"].as_str()),
                (Some("./assistants"), Some("./memory-hive"))
                    | (Some("./memory-hive"), Some("./assistants"))
            )
        })
        .collect()
}

/// The app's one cell: on `command_candidate` it pushes the body's candidates
/// on `candidate`, with `hop.ack` "1" when the command asks for a receipt --
/// what any app that declared `candidates` does with what it wants shown.
const APP_CELL: &str = r#"import sys, json
doc = json.load(sys.stdin)
env = doc.get("envelope") or {}
hop = (env.get("header") or {}).get("hop") or {}
body = doc.get("body") or {}
out = []
if hop.get("route") == "command_candidate":
    head = {"route": "candidate"}
    if str(body.get("ack") or "") == "1":
        head["ack"] = "1"
    push = {"header": head, "messages": [], "source": body.get("source"),
            "candidates": body.get("candidates") or []}
    if "withdraw" in body:
        push["withdraw"] = body["withdraw"]
    out.append(push)
sys.stdout.write(json.dumps(out))
"#;

/// What reaches the member stand-in's `/rim`: the two lanes of this road that
/// leave the generation (`thing_seen` at its rim, `candidate_ack` through the
/// corridor), and the one `sidecar` that must never leave: `things`.
const RIM_LANES: &str = "has(hop.route) && (hop.route == 'thing_seen' || \
                         hop.route == 'candidate_ack' || (hop.route == 'sidecar' && \
                         has(hop.section) && hop.section == 'things'))";

/// The member stand-in: `./assistants` is the container, grown with the
/// generation `sam` the way `examples/organism/grow-assistant.json` grows it;
/// `./memory-hive` is the person's memory; `./apps/remind-app` holds the app;
/// the member's memory edges and the ones `install_app` renders stand in the
/// member's graph; the two lanes the member collects end at `/rim`;
/// everything else that leaves is drained.
fn build_member(td: &tempfile::TempDir, talky: &str, background: &str) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join(format!("assistants/{GENERATION}")),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    // The stand-ins of the file header: one receipt corridor per brain, drawn
    // the way the container draws the recall road (`lane` named, from the
    // brain rim). The member's pickup of `thing_seen` at the generation's rim
    // is NOT drawn here: `examples/organism/grow-assistant.json` ships it, and
    // a second copy fanned every `thing_seen` out twice (two at `/rim` for one
    // answer, Fix-Runde 2 of GH #949).
    let mut container = container_edges();
    assert_eq!(
        container
            .iter()
            .filter(|e| e["from"] == format!("./{GENERATION}")
                && e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'thing_seen'")))
            .count(),
        1,
        "the grown container picks `thing_seen` up at the generation's rim exactly once"
    );
    for brain in BRAINS {
        container.push(json!({"from": format!("./{GENERATION}/{brain}"), "to": ".",
                              "lane": "candidate_ack",
                              "condition": "has(hop.route) && hop.route == 'candidate_ack'"}));
    }
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": container}}}),
    );
    for brain in BRAINS {
        // The section is offered and taken by every curator of the generation.
        for cell in ["schemas", "intake"] {
            override_params_on_disk(
                &main.join(format!("assistants/{GENERATION}/{brain}/curator/{cell}")),
                &json!({"things_section": "1"}),
            );
        }
    }
    for s in ["talky", "talky-chat"] {
        // No member affinity stands here: the brief leg is not this road.
        override_params_on_disk(
            &main.join(format!("assistants/{GENERATION}/{s}/collector/assemble")),
            &json!({"brief_slots": []}),
        );
    }
    // The apps container as the member ships it, and the app: an open hive
    // with one code cell (no `params.ports`, so its edge may leave the cell).
    write_json(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write_json(
        &main.join(format!("apps/{APP}/config.json")),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    // The cell's contract is the one of `gh916`'s probe cell: a code cell that
    // may send a list, reading and writing only `messages` it declares.
    write_json(
        &main.join(format!("apps/{APP}/sink/config.json")),
        &json!({"cell": {"type": "code"},
                "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                             "emits": {"body": {"messages": {"type": "array", "required": false}}},
                             "consumes": {"body": {"messages": {"type": "array",
                                                                "required": false}}},
                             "capabilities": ["shell:exec"]},
                "params": {"runner": "python3", "external_timeout_ms": 15000,
                           "script_inline": APP_CELL}}),
    );

    let mut edges = member_memory_edges();
    edges.extend(install_edges());
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/rim", "condition": RIM_LANES}));
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
    let pointed = point_llms_at_stubs(&main, talky, background);
    let voice = format!("assistants/{GENERATION}/talky/brain");
    assert!(
        pointed.contains(&voice),
        "{voice} is not an llm cell of the tree: {pointed:?}"
    );
    write_env(root, &main, background);
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    rim: mpsc::Receiver<Message>,
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
    let (rim_tx, rim_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/rim"), move || CaptureCell::new(rim_tx.clone()))
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
        .expect("the generation, the member road, the memory hive and the app must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            rim: rim_rx,
            park: park_rx,
        },
    )
}

// ─────────────────────────────────────────────────────────────── the turns

/// A person's words at the container's door, on `channel`, in `round`.
fn person(channel: &str, round: &str, turn: &str, text: &str) -> Message {
    let ctx = json!({"assistant": GENERATION, "channel": channel,
                     "audience_set": round, "turn_id": turn});
    MessageBuilder::new(Path::new("/assistants"))
        .hop(as_map(&json!({"route": "in_turn"})))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// What the app's cell is commanded to push, at the cell itself, in the
/// round it claims: `FORGED` (review I-1).
fn command_candidates(source: &str, candidates: Value, ack: bool) -> Message {
    let mut body = json!({"messages": [], "source": source, "candidates": candidates});
    if ack {
        body["ack"] = json!("1");
    }
    MessageBuilder::new(Path::new(&format!("/apps/{APP}/sink")))
        .hop(as_map(&json!({"route": "command_candidate"})))
        .context(as_map(
            &json!({"audience_set": FORGED, "channel": OWNER_CHANNEL}),
        ))
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

/// The owner's answer: a few words and the block, with the section `things`
/// (an object holding `items`, the form the curator offers -- the splitter
/// drops a section whose value is a list) and the memory's nothing form.
fn with_things(text: &str) -> String {
    let block = json!({
        "things": {"items": [{"name": THING, "kind": "device", "note": "restarts at noon"}]},
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

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn ctx_str(m: &Message, key: &str) -> String {
    m.headers
        .context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
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

/// One routing decision of the run, off the colony's own `message_log`: who
/// sent it, where it went, the headers it went with (after the edge).
struct Logged {
    from: String,
    to: String,
    headers: Value,
}

impl Logged {
    fn hop(&self, key: &str) -> &str {
        self.headers["hop"][key].as_str().unwrap_or("")
    }
    fn ctx(&self, key: &str) -> &str {
        self.headers["context"][key].as_str().unwrap_or("")
    }
}

/// Every delivery the colony logged, in order. Read after `shutdown`, which
/// flushes the log writer.
fn logged(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, headers)| Logged {
        from,
        to,
        headers: meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null),
    })
    .collect()
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

fn ledger(td: &tempfile::TempDir, brain: &str) -> std::path::PathBuf {
    td.path().join(format!(
        "main/assistants/{GENERATION}/{brain}/curator/ledger/cell.db"
    ))
}

/// Whether the curator of `brain` keeps the candidate `id` of `source`.
fn keeps(td: &tempfile::TempDir, brain: &str, source: &str, id: &str) -> bool {
    rows(&ledger(td, brain), "SELECT source, cand_id FROM candidates")
        .iter()
        .any(|r| r[0] == source && r[1] == id)
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

/// Take what reaches `/rim` into `got` until `n` of it satisfy `pick`, or the
/// deadline says what did not arrive.
async fn until_rim(
    ports: &mut Ports,
    root: &std::path::Path,
    got: &mut Vec<Message>,
    pick: impl Fn(&Message) -> bool,
    n: usize,
    what: &str,
) {
    let deadline = Instant::now() + DEADLINE;
    while got.iter().filter(|&m| pick(m)).count() < n {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.rim.recv()).await {
            Ok(Some(m)) => got.push(m),
            _ => panic!(
                "{what}: fewer than {n} within {DEADLINE:?}. The rim saw: {:#?}. \
                 Dead letters: {:#?}",
                got.iter()
                    .map(|m| (
                        hop_str(m, "route"),
                        hop_str(m, "section"),
                        body_of(m).clone()
                    ))
                    .collect::<Vec<_>>(),
                dead_letters(root)
            ),
        }
    }
}

/// The memory questions that reached the memory's door from `channel`
/// (`in_query`, the question in `context.recall_query`).
fn questions_at_the_memory<'a>(log: &'a [Logged], channel: &str) -> Vec<&'a str> {
    log.iter()
        .filter(|d| d.to == "/memory-hive" && d.hop("route") == "in_query")
        .filter(|d| d.ctx("channel") == channel)
        .map(|d| d.ctx("recall_query"))
        .collect()
}

/// The text of one element of a request at a model: a string, or the text
/// parts of a list (the two forms the chat wire knows).
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

/// The one request the voice's model had for the turn whose person's words --
/// the LAST `user` element of the request -- say `words`: its elements, and
/// the index of those words in them. Picked by content, not by arrival order.
fn turn_at_the_model(reqs: &[OpenAiRequestSnapshot], words: &str) -> (Vec<Value>, usize) {
    let hits: Vec<(Vec<Value>, usize)> = reqs
        .iter()
        .filter_map(|r| {
            let wire = r.messages().cloned().unwrap_or_default();
            let at = wire.iter().rposition(|m| m["role"] == "user")?;
            content_of(&wire[at]).contains(words).then_some((wire, at))
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly one request at the voice's model is the turn saying {words:?}: {:#?}",
        reqs.iter().map(|r| r.body.clone()).collect::<Vec<_>>()
    );
    hits.into_iter().next().expect("one hit")
}

/// The lanes of this road: a dead letter carrying one of them is a rim that
/// does not pass it (or a receipt nobody asked for).
const ROAD_LANES: [&str; 4] = ["candidate", "in_candidate", "candidate_ack", "thing_seen"];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_candidate_reaches_the_push_of_a_covered_turn_and_things_leave_once() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let talky = MockOpenAI::start(vec![
        // The guest's turn: a plain answer.
        canned_chat_completion(GUEST_HEARS, "stop"),
        // The owner's turn: the answer names one thing.
        canned_chat_completion(&with_things(OWNER_HEARS), "stop"),
    ])
    .await;
    let background = MockOpenAI::start(
        (0..64)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build_member(&td, &talky.base_url, &background.base_url);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;
    let mut rim: Vec<Message> = Vec::new();

    // (1) The app pushes one candidate with a trigger, no receipt asked for.
    h.send(command_candidates(
        SOURCE,
        json!([{"id": CANDIDATE_ID, "text": CANDIDATE, "triggers": [TRIGGER]}]),
        false,
    ))
    .await;
    for brain in BRAINS {
        let deadline = Instant::now() + DEADLINE;
        while !keeps(&td, brain, SOURCE, CANDIDATE_ID) {
            assert!(
                Instant::now() < deadline,
                "the candidate stands in the curator of every brain: {brain}'s ledger does not \
                 keep ({SOURCE}, {CANDIDATE_ID}) within {DEADLINE:?}: {:?}. Dead letters: {:#?}",
                rows(
                    &ledger(&td, brain),
                    "SELECT source, cand_id, audience_set FROM candidates"
                ),
                dead_letters(&root)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let kept = rows(
            &ledger(&td, brain),
            &format!("SELECT audience_set FROM candidates WHERE cand_id = '{CANDIDATE_ID}'"),
        );
        let round: BTreeSet<String> =
            meclaw_core::serde_json::from_str(&kept[0][0]).expect("a JSON round");
        assert_eq!(
            round,
            ["agent:sam", "member:owner"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>(),
            "{brain}: the candidate is kept in the member's round, not the app's {FORGED}"
        );
    }

    // (2) A turn of a round the candidate does not cover, naming the trigger.
    h.send(person(GUEST_CHANNEL, OTHER, "T-949-guest", GUEST_ASKS))
        .await;
    answer_saying(&mut ports, &root, GUEST_HEARS).await;

    // (3) The next turn of the covered round, naming the trigger.
    h.send(person(OWNER_CHANNEL, AUDIENCE, "T-949-owner", OWNER_ASKS))
        .await;
    answer_saying(&mut ports, &root, OWNER_HEARS).await;

    // (4) Its answer named a thing.
    until_rim(
        &mut ports,
        &root,
        &mut rim,
        |m| hop_str(m, "route") == "thing_seen",
        1,
        "the thing the owner's answer named leaves the generation",
    )
    .await;

    // (5) A push that asks for its receipt, under a source of its own and with
    // a trigger nobody says: each of the three curators answers it once.
    h.send(command_candidates(
        ACK_SOURCE,
        json!([{"id": "receipt-probe", "text": "Asked for a receipt.",
                "triggers": ["never-named"]}]),
        true,
    ))
    .await;
    until_rim(
        &mut ports,
        &root,
        &mut rim,
        |m| hop_str(m, "route") == "candidate_ack",
        BRAINS.len(),
        "a push with `hop.ack` \"1\" is answered by each curator",
    )
    .await;

    // Both answers left, so both requests reached the model before this read:
    // nothing here waits on a clock.
    let reqs = talky.recorded_requests().await;
    h.shutdown().await;
    while let Ok(m) = ports.rim.try_recv() {
        rim.push(m);
    }
    let log = logged(&root);
    let dead = dead_letters(&root);

    assert_eq!(reqs.len(), 2, "the voice's model was called once per turn");

    // The road of a candidate is the enriched ask (`./push`, phase `ask`), which
    // runs only with `recall_push` "1": read off the hop the voice's policy
    // stamped on every ask it handed its push, so a red request below is never
    // a knob that was off.
    let push = format!("/assistants/{GENERATION}/talky/curator/push");
    let knobs: Vec<&str> = log
        .iter()
        .filter(|d| d.to == push && d.hop("route") == "push_ask")
        .map(|d| d.hop("recall_push"))
        .collect();
    assert!(
        knobs.len() >= 2 && knobs.iter().all(|k| *k == "1"),
        "the voice's curator enriches the ask of each turn (`recall_push` \"1\", role `talky`): \
         {knobs:?}"
    );
    let handed: Vec<&str> = log
        .iter()
        .filter(|d| d.from == push && d.hop("route") == "in_addendum")
        .map(|d| d.to.as_str())
        .collect();
    let guest = questions_at_the_memory(&log, GUEST_CHANNEL);
    let owner = questions_at_the_memory(&log, OWNER_CHANNEL);

    // (2) The push, read where it is used: the request of the guest's turn at
    // the voice's model -- every element, the system part included.
    let (guest_wire, _) = turn_at_the_model(&reqs, GUEST_ASKS);
    let leaked: Vec<&Value> = guest_wire
        .iter()
        .filter(|m| m.to_string().contains(CANDIDATE))
        .collect();
    assert!(
        leaked.is_empty(),
        "a candidate of {AUDIENCE} reached the model in a turn of the round {OTHER}, which it \
         does not cover: {leaked:#?}"
    );

    // (3) The request of the owner's turn: the candidate's own part in exactly
    // ONE element, a tool result answering a `memory_recall` call, in the
    // window of this round ahead of the person's words. The frame line and the
    // call id are the push's to choose; the part `[<text>]` is the contract.
    let (owner_wire, person) = turn_at_the_model(&reqs, OWNER_ASKS);
    let carrying: Vec<usize> = owner_wire
        .iter()
        .enumerate()
        .filter(|(_, m)| m.to_string().contains(CANDIDATE))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        carrying.len(),
        1,
        "the covered turn that names `{TRIGGER}` shows the candidate to the model in exactly \
         one element, the result of its pair -- not in the question the memory was asked (the \
         ambient call's arguments echo it). Carrying: {carrying:?}; the push handed \
         `in_addendum` to {handed:?}; the owner's questions {owner:?}; the request: \
         {owner_wire:#?}"
    );
    let at = carrying[0];
    let shown = &owner_wire[at];
    assert_eq!(
        shown["role"], "tool",
        "the candidate stands in the window as a tool result: {shown:#}"
    );
    assert!(
        content_of(shown).contains(&format!("[{CANDIDATE}]")),
        "the result says the candidate's own part `[<text>]`: {shown:#}"
    );
    assert!(
        at < person,
        "the pair stands in the window of this round ahead of its person's words (element \
         {at}, the words at {person}): {owner_wire:#?}"
    );
    let id = shown["tool_call_id"].as_str().unwrap_or_default();
    let call = owner_wire[..at]
        .iter()
        .flat_map(|m| m["tool_calls"].as_array().cloned().unwrap_or_default())
        .find(|c| !id.is_empty() && c["id"].as_str() == Some(id))
        .unwrap_or_else(|| {
            panic!("the result answers a call of the window ({id:?}): {owner_wire:#?}")
        });
    assert_eq!(
        call["function"]["name"], "memory_recall",
        "the pair is a `memory_recall` round, the way a gap's find is shown: {call:#}"
    );

    // (2)/(3) The memory question of neither turn carries it: byte for byte
    // what curator 1.5.0 asked -- the person's words close it, and no part of
    // a candidate stands in front of them. Read at the memory's door.
    assert!(
        !guest.is_empty() && !owner.is_empty(),
        "each turn asks the memory (the guest's {guest:?}, the owner's {owner:?})"
    );
    for (words, questions) in [(GUEST_ASKS, &guest), (OWNER_ASKS, &owner)] {
        for q in questions {
            assert!(
                q.ends_with(words) && !q.contains(CANDIDATE),
                "a candidate is no part of the memory question (`recall_query` as curator 1.5.0 \
                 composed it, closed by the person's words {words:?}): {q:?}"
            );
        }
    }

    // (4) Exactly one `thing_seen`, with the round, and no `sidecar` of it.
    let seen: Vec<&Message> = rim
        .iter()
        .filter(|m| hop_str(m, "route") == "thing_seen")
        .collect();
    assert_eq!(seen.len(), 1, "one answer, one `thing_seen`: {seen:#?}");
    let thing = seen[0];
    assert_eq!(
        ctx_str(thing, "audience_set"),
        AUDIENCE,
        "`thing_seen` carries the round of the turn that named the thing"
    );
    assert!(
        thing.reply_to.as_ref().is_some_and(|p| p
            .as_str()
            .starts_with(&format!("/assistants/{GENERATION}/talky/"))),
        "the voice that answered raised it: {:?}",
        thing.reply_to
    );
    let items = body_of(thing)["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(items.len(), 1, "{}", body_of(thing));
    assert_eq!(items[0]["name"], json!(THING), "{}", body_of(thing));
    for key in ["turn_id", "session_id"] {
        assert!(
            body_of(thing)[key].as_str().is_some_and(|s| !s.is_empty()),
            "`thing_seen` names its {key}: {}",
            body_of(thing)
        );
    }
    let copies: Vec<&Message> = rim
        .iter()
        .filter(|m| hop_str(m, "route") == "sidecar" && hop_str(m, "section") == "things")
        .collect();
    assert!(
        copies.is_empty(),
        "the section `things` left the generation as a `sidecar` too: {copies:#?}"
    );
    // Deterministic beside the receiver: the splitter's routing decision for
    // the section is logged in one step, every target at once -- one target,
    // the curator, and nothing toward the voice's own rim.
    let splitter = format!("/assistants/{GENERATION}/talky/splitter");
    let curator = format!("/assistants/{GENERATION}/talky/curator");
    let routed: Vec<&str> = log
        .iter()
        .filter(|d| d.from == splitter && d.hop("section") == "things")
        .map(|d| d.to.as_str())
        .collect();
    assert_eq!(
        routed,
        vec![curator.as_str()],
        "the splitter routes the section `things` to the curator, once, and nowhere else"
    );
    let out_of_the_generation = log
        .iter()
        .filter(|d| d.hop("route") == "thing_seen" && d.to == "/assistants")
        .count();
    assert_eq!(
        out_of_the_generation, 1,
        "one `thing_seen` crosses the generation's rim"
    );

    // (5) The receipts: three, all for the push that asked, one per curator.
    let acks: Vec<&Message> = rim
        .iter()
        .filter(|m| hop_str(m, "route") == "candidate_ack")
        .collect();
    let sources: Vec<&str> = acks
        .iter()
        .map(|m| body_of(m)["source"].as_str().unwrap_or(""))
        .collect();
    assert!(
        sources.iter().all(|s| *s == ACK_SOURCE),
        "a push without `hop.ack` \"1\" was answered: {sources:?}"
    );
    assert_eq!(
        acks.len(),
        BRAINS.len(),
        "one receipt per curator of the generation: {sources:?}"
    );
    let answered_by: BTreeSet<String> = acks
        .iter()
        .filter_map(|m| m.reply_to.as_ref())
        .filter_map(|p| {
            p.as_str()
                .strip_prefix(&format!("/assistants/{GENERATION}/"))
                .and_then(|rest| rest.split('/').next())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        answered_by,
        BRAINS
            .iter()
            .map(|b| b.to_string())
            .collect::<BTreeSet<_>>(),
        "each brain's curator answers for itself"
    );
    for m in &acks {
        assert!(
            hop_str(m, "error_code").is_empty(),
            "a receipt of a push with a round carries no error: {:?}",
            m.headers.hop
        );
    }
    // The receipt docks at the brain rims (`at`): the generation's own rim
    // never carries it, so nothing above a brain owes it a holder.
    let generation = format!("/assistants/{GENERATION}");
    let through_the_rim: Vec<&str> = log
        .iter()
        .filter(|d| d.hop("route") == "candidate_ack" && d.to == generation)
        .map(|d| d.from.as_str())
        .collect();
    assert!(
        through_the_rim.is_empty(),
        "a receipt crossed the generation's own rim: from {through_the_rim:?}"
    );

    let lost: Vec<&(String, String, String, String)> = dead
        .iter()
        .filter(|(_, _, _, route)| ROAD_LANES.contains(&route.as_str()))
        .collect();
    assert!(
        lost.is_empty(),
        "a lane of this road dead-lettered (a rim that does not pass it, or a receipt nobody \
         asked for): {lost:#?}"
    );
}
