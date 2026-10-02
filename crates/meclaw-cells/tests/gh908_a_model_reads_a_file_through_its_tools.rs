//! GH #908 -- a model reads a file through its tools, and only the core writes.
//!
//! B1 built the file space as lanes (`in_read`, `in_write`, `in_ws`); #908 puts
//! it on the models' menus. The space grew `./schemas` (the menu, 37 `file_*`
//! declarations) and `./tools` (a tool call in, a request under `caller`
//! 'tools' out, the one answer back as a `tool_result` under the call id); the
//! member grew a `./file-space` node and four edges to its assistants; every surface
//! of the assistant level carries a `file_*` call up with `context.tool_caller`
//! naming it; and the two voices declare the fourteen READ tools while the core
//! declares `*`. `./tools` refuses a write or workspace op from any surface
//! but the core (`read_only`).
//!
//! Each of those pieces is pinned on its own elsewhere (the menu table in
//! `gh908_every_file_tool_maps_to_one_op.rs`). This file boots the SHIPPED
//! templates and measures whether the road closes, at the receivers: the
//! request bodies the provider stubs recorded, the deliveries in the colony's
//! `message_log`, the answers that leave the space.
//!
//! 1. **A voice reads** (`a_talky_reads_a_seeded_file_through_its_tools`). The
//!    first request of a talky turn offers the fourteen read tools and none of the
//!    twenty-three write and workspace tools; the stub calls `file_summary` and
//!    `file_read` on the address of a file seeded beforehand, and the NEXT
//!    request carries both results under their call ids, each naming the
//!    file's `version` token -- the summary being the one `./derive` wrote, not
//!    `pending`. Both calls reached the space stamped `tool_caller` 'talky' and
//!    both results came back to the talky.
//! 2. **The core writes** (`cogny_opens_a_workspace_replaces_against_its_base_and_commits`).
//!    The talky consults the core; the core's stub calls `file_ws_open`,
//!    `file_replace` (with `base` = the seeded version and `ws` = the
//!    workspace) and `file_ws_commit`, one per request, and each next request
//!    carries the previous result `ok`. Afterwards `history`, asked of the space
//!    directly on `in_read`, shows the commit on top: `entries[0].commit` set,
//!    `entries[0].version` the replaced content's token.
//! 3. **A voice cannot write** (`a_write_call_without_the_core_mark_is_refused_read_only`).
//!    A `file_replace` and a `file_ws_open` sent straight to the space's
//!    `in_tool`, once without `context.tool_caller` and once as 'talky', each
//!    come back as ONE `tool_result` whose text is `{ok: false, error.code:
//!    'read_only'}` with `hop.error_code` 'read_only' -- and nothing reached
//!    `./write` or `./ws`.
//! 4. **No dead letter** in any of the three runs.
//!
//! THE BOOT FORM. Runs 1 and 2 boot the root as the member reduced to what this
//! road touches: the shipped assistant level as a node named `assistants`, and
//! the member's own `./file-space` node beside it (the `file-space@1.3.0` ref,
//! resolved). The node is named `assistants` so the member's four `./file-space`
//! edges can be read off `templates/member/config.json` and drawn VERBATIM --
//! nothing in them is retyped or re-pointed. What this leaves out is the
//! container a builder-grown member puts between `./assistants` and a
//! generation; its rim for this road (builder recipe, "GH #552 -- the memory
//! TOOL road": `tool`/`schemas` out, `in_tool`/`in_menu` in) passes the lanes
//! through unchanged, so a level standing where the container stands is the
//! same road with one pass-through fewer. The memory hive and the member's
//! other nodes are not here: the surfaces' memory and brief legs are switched
//! off (`memory_tier` and `brief_slots` empty, as in
//! `gh894_a_consult_asks_back_under_its_id.rs`), and the level's own tool hive
//! is a silent stand-in -- no `file_*` call may reach it, and none does. Every
//! other lane the level declares is drained, `tool` and `schemas` excepted:
//! those two leave through the member's edges, so a `tool` that is not a
//! `file_*` call would be a dead letter and the assertion below would see it.
//! Run 3 needs no model at all and boots the space alone, its rim drained.
//!
//! THE SEED goes through the space's own door: `create` on `in_write` from
//! outside (`caller` empty) with `notify` '1', its `answer` and its `derived`
//! drained to a capture. The run waits for that `derived` before the turn, so
//! `file_summary` finds the summary the space wrote at birth rather than the
//! `pending` it answers until then -- the summary assertion is a real one, not
//! one relaxed to `ok`. The address the stubs use is the file's PATH (an
//! address form of the space, README § Address), because a stub's script is
//! fixed before the colony boots and the `fh-` id is minted at random; the
//! `version` token is sha256 of the bytes, computed here, and asserted equal to
//! the one the seed's answer names before anything relies on it.
//!
//! Free of a paid provider by construction: every `llm` cell of the booted tree
//! is pointed at a local stub (the surface's and the core's brains at scripted
//! ones, every other -- curators' summarizers, the space's summarizer -- at a
//! background stub), and the space's `./embed` calls a local embeddings stub
//! through `MEMORY_EMBED_ENDPOINT`, the setting it shares with the memory hive.
//!
//! Guarded like every template-reading test (GH #49): a tree without one of the
//! templates this road boots is skipped, never judged.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{
    MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_content_and_tool_calls,
    canned_tool_calls,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// Left alone after the last expected arrival before "nothing else" is read.
const SETTLE: Duration = Duration::from_secs(2);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
/// What every background model answers -- and so the one line the space's
/// summarizer writes for the seeded file (its first non-empty line).
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const CHANNEL: &str = "chat:908";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;

/// The seeded file: its path is the address the stubs use.
const PATH: &str = "/notes/mill.md";
const TEXT: &str =
    "# The old mill\n\nThe river runs north past the old mill.\nThe wheel turns at dawn.\n";
const LINE: &str = "The river runs north past the old mill.";
const OLD: &str = "north";
const NEW: &str = "south";
const SEED_ID: &str = "seed-908";
const HISTORY_ID: &str = "history-908";

/// The fourteen tools a voice declares (`templates/assistant/talky` and
/// `talky-chat`, `collector/assemble.tools`), in the plan's words (L § 5);
/// `file_dir_info` and `file_dir_summary` (GH #947) joined with GH #950.
const READ_TOOLS: [&str; 14] = [
    "file_info",
    "file_read",
    "file_search",
    "file_summary",
    "file_ask",
    "file_history",
    "file_show",
    "file_diff",
    "file_list",
    "file_find",
    "file_outline",
    "file_links",
    "file_dir_info",
    "file_dir_summary",
];

// run 1: the talky reads
const SAID_A: &str = "What does my note about the old mill say?";
const SUMMARY_ID: &str = "call-sum-908";
const READ_ID: &str = "call-read-908";
const ANSWER_A: &str = "Your note says the river runs north past the old mill.";

// run 2: the core writes
const SAID_B: &str = "Please correct my mill note: the river runs south.";
const CONSULT_ID: &str = "call-consult-908";
const INTERIM_B: &str = "I will have that corrected.";
const WS: &str = "fix-mill";
const WS_OPEN_ID: &str = "call-ws-open-908";
const REPLACE_ID: &str = "call-replace-908";
const COMMIT_ID: &str = "call-commit-908";
const CORE_DONE: &str = "The note now says the river runs south; the change is committed.";
const FINAL_B: &str = "Done -- your note now says the river runs south.";

// run 3: a voice asks for a write
const RO_REPLACE_ID: &str = "call-ro-replace-908";
const RO_OPEN_ID: &str = "call-ro-open-908";

// ═══════════════════════════════════════════════════════════════ the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this road boots or reads. One missing = skipped (GH #49).
fn shipped() -> bool {
    [
        "assistant",
        "talky",
        "cogny",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
        "member",
        "file-space",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write");
}

/// The shipped template, copied the way instantiation lays it out (the reader
/// of `gh889_a_turn_runs_collector_curator_brain.rs`): a ref marker is replaced
/// by the referenced template's tree and its `override_params` are applied to
/// the cells they name, which is what the mutation door does to a staged tree.
/// That is how the voices arrive with the fourteen read tools on their collectors.
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

/// The local stubs of one run: the voice's brain, the core's brain, every
/// other model, and the embeddings endpoint.
struct Stubs {
    surface: String,
    core: String,
    background: String,
    embed: String,
}

/// Point EVERY `llm` cell of the tree at a stub: the talky's brain and the
/// core's brain at their scripted ones, all others at the background one.
/// `${ctx.model}` is an instantiation-side substitution a tree booted from disk
/// cannot resolve, so the model is named here. Returns the cells it pointed.
fn point_llms_at_stubs(main: &std::path::Path, stubs: &Stubs) -> Vec<String> {
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
        let (url, model) = if rel == "assistants/talky/brain" {
            (&stubs.surface, BRAIN_MODEL)
        } else if rel == "assistants/cogny/brain" {
            (&stubs.core, BRAIN_MODEL)
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

/// Every timer of the tree out of the run's way (swept, never named).
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0908_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file. Every `${VAR}` the tree references without a
/// default is bound to a dummy; the embeddings endpoint the space's `./embed`
/// reads (`MEMORY_EMBED_ENDPOINT`, shared with the memory hive) is the local
/// embeddings stub, so nothing calls out.
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
    vars.insert("MEMORY_EMBED_ENDPOINT".into(), stubs.embed.clone());
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1 (the form of
/// `gh903_a_file_knows_itself_from_birth.rs`): one vector of the asked
/// `dimensions` per input, a pure function of the input's length. Counts its
/// requests, so a run can say the space's `./embed` really reached it.
struct EmbedStub {
    url: String,
    calls: Arc<AtomicUsize>,
}

fn embed_stub() -> EmbedStub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the embeddings stub");
    let url = format!(
        "http://{}/v1/embeddings",
        listener.local_addr().expect("a bound address")
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Ok(half) = conn.try_clone() else { continue };
            let mut reader = BufReader::new(half);
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let req: Value = sj::from_slice(&body).unwrap_or(Value::Null);
            let dim = req["dimensions"].as_u64().unwrap_or(64) as usize;
            let data: Vec<Value> = req["input"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let n = t.as_str().map_or(0, str::len);
                    let v: Vec<f64> = (0..dim)
                        .map(|k| if (k + n) % 3 == 0 { 1.0 } else { -1.0 })
                        .collect();
                    json!({"index": i, "embedding": v})
                })
                .collect();
            seen.fetch_add(1, Ordering::SeqCst);
            let out = json!({"data": data, "model": "embed-stub",
                             "usage": {"prompt_tokens": 3}})
            .to_string();
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    EmbedStub { url, calls }
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// The `version` token of a content: the first 12 hex digits of its sha256
/// (README § Address).
fn token(text: &str) -> String {
    sha256_hex(text.as_bytes())[..12].to_string()
}

// ══════════════════════════════════════════════════════════════════ the harness

/// The lanes a template declares at its own path, in declaration order.
fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// One plain drain per lane, to the capture `to(lane)` names.
fn drains(from: &str, lanes: &[String], to: impl Fn(&str) -> &'static str) -> Vec<Value> {
    lanes
        .iter()
        .map(|lane| {
            json!({"from": from, "to": to(lane),
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

/// The member's own `./file-space` edges (GH #908), read off the shipped member and
/// drawn as they stand: the call and the menu question down, the result and
/// the menu answer up. Exactly four beside the document intake (GH #907), or
/// the member changed under this file.
fn member_file_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    let edges: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["from"] == json!("./file-space") || e["to"] == json!("./file-space"))
        // The graph space's index road (GH #945: `source_changed`, `pull` and
        // the `gs:` answer) is not the tools' road; its lock is gh945's.
        .filter(|e| e["from"] != json!("./graph-space") && e["to"] != json!("./graph-space"))
        // Nor is the librarian's (GH #950: `source_changed`, the `lib:f:` pull
        // and the `lib:` answer); its tools are the `lib_` ones, not these.
        .filter(|e| e["from"] != json!("./librarian") && e["to"] != json!("./librarian"))
        // The document intake of GH #907 (`./firewall -> ./file-space` on
        // `pass` with a file, `./file-space -> ./assistants` on `turn`) is the
        // turn's road, not the tools'; its lock is gh907's seam lock.
        .filter(|e| {
            e["from"] != json!("./firewall")
                && !e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains("hop.route == 'turn'")
        })
        .collect();
    assert_eq!(
        edges.len(),
        4,
        "the member draws four `./file-space` edges -- `tool` and `schemas` down, `tool_result` \
         and `tool_schemas` up: {edges:#?}"
    );
    for e in &edges {
        let other = if e["from"] == json!("./file-space") {
            &e["to"]
        } else {
            &e["from"]
        };
        assert_eq!(
            other,
            &json!("./assistants"),
            "every `./file-space` edge of the member runs to its assistants: {e}"
        );
    }
    edges
}

/// The level's tool hive, silent: it hears and says nothing. No `file_*` call
/// may reach it (the surfaces' `file_*` edges win over their `default` tool
/// edge), and what it would answer to a menu question is not this road.
fn silent_tools() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "external_timeout_ms": 10000,
                   "script_inline": "import sys, json\njson.load(sys.stdin)\nsys.stdout.write('[]')\n"},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the tool hive: it answers nothing.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The assistant level, its occupants resolved, its tool hive silent.
fn copy_level(dst: &std::path::Path) {
    let src = repo("templates/assistant");
    std::fs::create_dir_all(dst).expect("mkdir");
    std::fs::copy(src.join("config.json"), dst.join("config.json")).expect("copy");
    for entry in std::fs::read_dir(&src).expect("readable") {
        let entry = entry.expect("entry");
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name == "tools" {
            write_json(&dst.join("tools/config.json"), &silent_tools());
        } else {
            copy_resolved(&entry.path(), &dst.join(name), 0);
        }
    }
}

/// Runs 1 and 2: the member reduced to this road (see the file doc). Returns
/// the `llm` cells it pointed at the stubs.
fn build_member(td: &tempfile::TempDir, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_level(&main.join("assistants"));
    for node in ["talky", "talky-chat"] {
        override_params_on_disk(
            &main.join(format!("assistants/{node}/collector/assemble")),
            &json!({"memory_tier": "", "brief_slots": []}),
        );
    }
    copy_resolved(
        &repo("templates/member/file-space"),
        &main.join("file-space"),
        0,
    );

    let level: Vec<String> = rim_emits("assistant")
        .into_iter()
        .filter(|l| l != "tool" && l != "schemas")
        .collect();
    let mut edges = drains("./assistants", &level, |l| {
        if l == "answer" { "/sink" } else { "/park" }
    });
    edges.extend(member_file_edges());
    let space: Vec<String> = rim_emits("file-space")
        .into_iter()
        .filter(|l| l != "tool_result" && l != "tool_schemas")
        .collect();
    edges.extend(drains("./file-space", &space, |l| {
        if l == "answer" || l == "derived" {
            "/fsink"
        } else {
            "/park"
        }
    }));
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, stubs);
    write_env(root, &main, stubs);
    pointed
}

/// Run 3: the space alone, every lane of its rim drained -- the tool result and
/// any answer to the capture the run reads.
fn build_space(td: &tempfile::TempDir, stubs: &Stubs) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/member/file-space"),
        &main.join("file-space"),
        0,
    );
    let edges = drains("./file-space", &rim_emits("file-space"), |l| {
        if l == "tool_result" || l == "answer" {
            "/sink"
        } else {
            "/park"
        }
    });
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    point_llms_at_stubs(&main, stubs);
    write_env(root, &main, stubs);
}

struct Ports {
    /// What leaves the assistant level on `answer` (run 3: what leaves the space).
    sink: mpsc::Receiver<Message>,
    /// What leaves the space on `answer` and `derived`.
    fsink: mpsc::Receiver<Message>,
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
    let (fsink_tx, fsink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/fsink"), move || {
        CaptureCell::new(fsink_tx.clone())
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
        .expect("the shipped assistant level and the file space must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            fsink: fsink_rx,
            park: park_rx,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// The mutation receipt that makes every collector of the level ask for its
/// menu (GH #553); a colony booted without `mutation_receipts` gets none.
fn menu_tick() -> Message {
    MessageBuilder::new(Path::new("/assistants"))
        .hop(map(json!({"route": "mutation_committed"})))
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

/// A person's turn at the level's door, with the round the channel stamps.
fn person(turn_id: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new("/assistants"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(
            json!({"channel": CHANNEL, "audience_set": AUDIENCE, "turn_id": turn_id}),
        ))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// A request at the space's door from outside (`caller` empty), in the form of
/// its README (§ Request and answer).
fn space_request(lane: &str, op: &str, op_id: &str, file: Option<&str>, args: Value) -> Message {
    let mut body = json!({"op": op, "args": args, "messages": []});
    if let Some(f) = file {
        body["file"] = json!(f);
    }
    MessageBuilder::new(Path::new("/file-space"))
        .hop(map(json!({"route": lane, "op": op, "op_id": op_id})))
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

/// One tool call at the space's `in_tool`, in the form a surface's dispatcher
/// sends it (the arguments as the text of the one `tool_call` turn), with the
/// `context.tool_caller` the assistant level would stamp -- or none.
fn tool_call(name: &str, id: &str, args: &str, tool_caller: Option<&str>) -> Message {
    let ctx = match tool_caller {
        Some(c) => json!({"tool_caller": c}),
        None => json!({}),
    };
    MessageBuilder::new(Path::new("/file-space"))
        .hop(map(
            json!({"route": "in_tool", "tool_name": name, "tool_call_id": id}),
        ))
        .context(map(ctx))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": id, "text": args}]})))
        .ttl(400)
        .build()
}

// ═══════════════════════════════════════════════════════════════════ the readers

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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

/// One routing hop as the colony logged it.
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
        let h: Value = sj::from_str(&headers).unwrap_or(Value::Null);
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
/// colony recorded. None is expected: the level drains its inner lanes, the
/// space its own, and this harness every lane either declares at its rim.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
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
        let m: Value = sj::from_str(&msg).unwrap_or(Value::Null);
        // The route alone named nothing when a dead letter carried none; the
        // head of the message is what tells a body the substrate refused.
        let head: String = msg.chars().take(1200).collect();
        (
            code,
            sender,
            target,
            format!("{} {head}", text_of(&m["headers"]["hop"], "route")),
        )
    })
    .collect()
}

/// The first column of one query against a cell's own `cell.db`; a store that
/// has not woken yet has no table, which is the honest "nothing written yet".
fn column(db: &std::path::Path, sql: &str) -> Vec<String> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    st.query_map([], |r| r.get::<_, Option<String>>(0))
        .map(|rows| -> Vec<String> { rows.filter_map(Result::ok).flatten().collect() })
        .unwrap_or_default()
}

/// Wait until every `(surface, tool)` stands in that surface's curator ledger
/// as a `tools.<name>` slot (the durable end of the menu road since GH #889):
/// every brain call after that offers it.
async fn wait_for_menus(root: &std::path::Path, wanted: &[(&str, &str)]) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut missing = Vec::new();
        for (surface, tool) in wanted {
            let db = root.join(format!("main/assistants/{surface}/curator/ledger/cell.db"));
            let slots = column(&db, "SELECT path FROM slots");
            if !slots.iter().any(|p| p == &format!("tools.{tool}")) {
                missing.push(format!("{surface}:{tool}"));
            }
        }
        if missing.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "the menus did not reach the curators within {DEADLINE:?}; missing {missing:?}. \
                 Dead letters: {:#?}",
                dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The next message on `rx` that `hit` accepts, every other one kept in `seen`.
async fn next_matching(
    rx: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    what: &str,
    within: Duration,
    hit: impl Fn(&Message) -> bool,
    seen: &mut Vec<Message>,
) -> Message {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(m)) => {
                if hit(&m) {
                    return m;
                }
                seen.push(m);
            }
            _ => panic!(
                "{what}: nothing arrived within {within:?}. Seen on the way: {:#?}. Dead \
                 letters: {:#?}",
                seen.iter()
                    .map(|m| (Value::Object(m.headers.hop.clone()), body_of(m).clone()))
                    .collect::<Vec<_>>(),
                dead_letters(root)
            ),
        }
    }
}

/// Seed the file through the space's own door and wait until `./derive` has
/// written what it derives at birth. Returns the file's `fh-` id.
async fn seed(h: &ColonyHandle, ports: &mut Ports, root: &std::path::Path) -> String {
    h.send(space_request(
        "in_write",
        "create",
        SEED_ID,
        None,
        json!({"path": PATH, "text": TEXT, "notify": "1"}),
    ))
    .await;
    let mut seen = Vec::new();
    let created = next_matching(
        &mut ports.fsink,
        root,
        "the seed's answer",
        DEADLINE,
        |m| hop_str(m, "route") == "answer" && hop_str(m, "op_id") == SEED_ID,
        &mut seen,
    )
    .await;
    let a = body_of(&created);
    assert_eq!(a["ok"], json!(true), "the seed was refused: {a}");
    let file = text_of(a, "file");
    assert!(
        file.starts_with("fh-"),
        "the answer names the file's id: {a}"
    );
    assert_eq!(
        text_of(a, "version"),
        token(TEXT),
        "the token the stubs pass back is sha256 of the bytes, first 12 hex: {a}"
    );
    // `derived` may have overtaken the answer; it is in `seen` then.
    let early = seen
        .iter()
        .position(|m| hop_str(m, "route") == "derived" && text_of(body_of(m), "file") == file);
    let derived = match early {
        Some(i) => seen.remove(i),
        None => {
            next_matching(
                &mut ports.fsink,
                root,
                "the seed's `derived`",
                DEADLINE,
                |m| hop_str(m, "route") == "derived" && text_of(body_of(m), "file") == file,
                &mut seen,
            )
            .await
        }
    };
    let d = body_of(&derived);
    assert_eq!(
        d["ok"],
        json!(true),
        "`./derive` wrote the seed's summary and embeddings: {d}"
    );
    assert_eq!(text_of(d, "version"), token(TEXT), "{d}");
    assert_eq!(
        text_of(d, "oneline"),
        BACKGROUND_REPLY,
        "the summary is the space's summarizer's, i.e. the background stub's: {d}"
    );
    file
}

fn tools_offered(req: &OpenAiRequestSnapshot) -> Vec<String> {
    let mut v: Vec<String> = req
        .tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            t["function"]["name"]
                .as_str()
                .or_else(|| t["name"].as_str())
                .map(str::to_string)
        })
        .collect();
    v.sort();
    v
}

fn wire(req: &OpenAiRequestSnapshot) -> String {
    sj::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

/// The content of the `tool` message under `id` in one request.
fn tool_content(req: &OpenAiRequestSnapshot, id: &str) -> String {
    req.messages()
        .expect("wire messages")
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| {
            panic!(
                "the request carries no tool result under `{id}` -- the round did not close \
                 through the space: {}",
                wire(req)
            )
        })
}

/// Every `file_*` tool the space declares, off the shipped `./schemas` script
/// (not typed here, so a tool the space adds is judged too).
fn every_file_tool() -> BTreeSet<String> {
    let cfg = read_json(&repo("templates/file-space/schemas/config.json"));
    let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
    let mut names = BTreeSet::new();
    let mut rest = script;
    while let Some(i) = rest.find("\"name\": \"file_") {
        rest = &rest[i + "\"name\": \"".len()..];
        let end = rest.find('"').unwrap_or(0);
        names.insert(rest[..end].to_string());
        rest = &rest[end..];
    }
    assert_eq!(
        names.len(),
        37,
        "the space declares 37 file tools (14 read, 16 write, 7 workspace): {names:?}"
    );
    names
}

fn write_tools() -> BTreeSet<String> {
    let reads: BTreeSet<String> = READ_TOOLS.iter().map(|s| s.to_string()).collect();
    every_file_tool().difference(&reads).cloned().collect()
}

fn parked_errors(rx: &mut mpsc::Receiver<Message>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        if matches!(hop_str(&m, "route").as_str(), "error" | "reject") {
            out.push(Value::Object(m.headers.hop.clone()));
        }
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════ pins

/// Run 1: a voice reads a seeded file through `file_summary` and `file_read`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_talky_reads_a_seeded_file_through_its_tools() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let file_arg = json!({"file": PATH}).to_string();
    let voice = MockOpenAI::start(vec![
        canned_tool_calls(vec![
            (SUMMARY_ID, "file_summary", file_arg.as_str()),
            (READ_ID, "file_read", file_arg.as_str()),
        ]),
        canned_chat_completion(ANSWER_A, "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![canned_chat_completion(CORE_DONE, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let embed = embed_stub();
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
        embed: embed.url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_member(&td, &stubs);
    assert!(
        pointed.iter().any(|p| p == "assistants/talky/brain")
            && pointed.iter().any(|p| p == "file-space/summarizer"),
        "the talky's brain and the space's summarizer are `llm` cells and talk to stubs: \
         {pointed:?}"
    );
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    h.send(menu_tick()).await;
    seed(&h, &mut ports, &root).await;
    let wanted: Vec<(&str, &str)> = READ_TOOLS.iter().map(|t| ("talky", *t)).collect();
    wait_for_menus(&root, &wanted).await;

    h.send(person("t-1", SAID_A)).await;
    let mut seen = Vec::new();
    next_matching(
        &mut ports.sink,
        &root,
        "the talky's answer",
        DEADLINE,
        |m| said(m).contains(ANSWER_A),
        &mut seen,
    )
    .await;
    tokio::time::sleep(SETTLE).await;

    let log = message_log(&root);
    let dead = dead_letters(&root);
    let errors = parked_errors(&mut ports.park);
    let reqs = voice.recorded_requests().await;
    h.shutdown().await;

    // 1. The menu: the fourteen read tools, no write or workspace tool.
    assert_eq!(
        reqs.len(),
        2,
        "one tool round is two calls of the talky's brain -- ask, then answer"
    );
    let writes = write_tools();
    for (i, r) in reqs.iter().enumerate() {
        let offered = tools_offered(r);
        for t in READ_TOOLS {
            assert!(
                offered.iter().any(|o| o == t),
                "talky request {i} does not offer `{t}`: {offered:?}"
            );
        }
        let wrong: Vec<&String> = offered.iter().filter(|o| writes.contains(*o)).collect();
        assert!(
            wrong.is_empty(),
            "talky request {i} offers write or workspace tools, which only the core may \
             call: {wrong:?}"
        );
    }

    // 2. Both results in the next request, under their call ids, with the token.
    let v = token(TEXT);
    let summary = tool_content(&reqs[1], SUMMARY_ID);
    assert!(
        summary.contains("\"ok\": true") && summary.contains(&format!("\"version\": \"{v}\"")),
        "the summary's result is the space's answer and names the token `{v}`: {summary}"
    );
    assert!(
        summary.contains(BACKGROUND_REPLY) && !summary.contains("\"pending\": true"),
        "the summary is the one `./derive` wrote at birth, not `pending`: {summary}"
    );
    let read = tool_content(&reqs[1], READ_ID);
    assert!(
        read.contains("\"ok\": true") && read.contains(&format!("\"version\": \"{v}\"")),
        "the read's result is the space's answer and names the token `{v}`: {read}"
    );
    assert!(
        read.contains(LINE),
        "the read carries the file's lines: {read}"
    );
    assert!(
        !summary.contains("\"op_id\"")
            && !summary.contains("\"caller\"")
            && !read.contains("\"op_id\"")
            && !read.contains("\"caller\""),
        "a result carries the answer without its correlation: {summary} / {read}"
    );

    // 3. At the seams: both calls reached the space stamped 'talky', both
    //    results reached the talky, and no call went anywhere but to `./read`.
    let calls: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/file-space" && r.route() == "in_tool")
        .collect();
    assert_eq!(calls.len(), 2, "two calls at the space's door: {calls:#?}");
    for r in &calls {
        assert!(
            // The log names the sender at the hive boundary it crossed
            // (`/assistants`), not the surface inside it; which surface
            // sent the call is the stamp below.
            r.from.starts_with("/assistants"),
            "a call from the talky: {r:?}"
        );
        assert_eq!(
            text_of(&r.context, "tool_caller"),
            "talky",
            "the level stamps the surface: {r:?}"
        );
    }
    let names: BTreeSet<String> = calls.iter().map(|r| text_of(&r.hop, "tool_name")).collect();
    assert_eq!(
        names,
        BTreeSet::from(["file_read".to_string(), "file_summary".to_string()]),
        "{calls:#?}"
    );
    let back: BTreeSet<String> = log
        .iter()
        .filter(|r| r.to == "/assistants/talky" && r.route() == "in_tool")
        .map(|r| text_of(&r.hop, "tool_call_id"))
        .collect();
    assert_eq!(
        back,
        BTreeSet::from([READ_ID.to_string(), SUMMARY_ID.to_string()]),
        "both results come back to the talky under their call ids"
    );
    let moved: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/file-space/write" || r.to == "/file-space/ws")
        .filter(|r| text_of(&r.hop, "caller") == "tools")
        .collect();
    assert!(moved.is_empty(), "a read call moved something: {moved:#?}");
    let asked_tools: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to.starts_with("/assistants/tools") && r.route() == "tool_call")
        .collect();
    assert!(
        asked_tools.is_empty(),
        "a `file_*` call reached the level's tool hive: {asked_tools:#?}"
    );
    assert!(
        embed.calls.load(Ordering::SeqCst) >= 1,
        "the space's `./embed` reached the embeddings stub"
    );

    // 4. Nothing refused, nothing lost.
    assert!(errors.is_empty(), "an error left the run: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// Run 2: the core opens a workspace, replaces against its base and commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cogny_opens_a_workspace_replaces_against_its_base_and_commits() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let base = token(TEXT);
    let changed = TEXT.replacen(OLD, NEW, 1);
    let new = token(&changed);
    let consult_args = json!({
        "question": format!("Replace `{OLD}` with `{NEW}` in {PATH}."),
        "context": format!("Goal: correct the person's note {PATH}. Facts: it says the river runs \
                            {OLD}; its current version is {base}. Constraints: change nothing \
                            else. Form: one sentence. Length: short."),
    })
    .to_string();
    let open_args = json!({"name": WS}).to_string();
    let replace_args =
        json!({"file": PATH, "base": base, "old": OLD, "new": NEW, "ws": WS}).to_string();
    let commit_args = json!({"ws": WS, "note": "the river runs south"}).to_string();

    let voice = MockOpenAI::start(vec![
        canned_content_and_tool_calls(
            INTERIM_B,
            vec![(CONSULT_ID, "consult_cogny", consult_args.as_str())],
        ),
        canned_chat_completion(FINAL_B, "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![
        canned_tool_calls(vec![(WS_OPEN_ID, "file_ws_open", open_args.as_str())]),
        canned_tool_calls(vec![(REPLACE_ID, "file_replace", replace_args.as_str())]),
        canned_tool_calls(vec![(COMMIT_ID, "file_ws_commit", commit_args.as_str())]),
        canned_chat_completion(CORE_DONE, "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let embed = embed_stub();
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
        embed: embed.url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_member(&td, &stubs);
    assert!(
        pointed.iter().any(|p| p == "assistants/cogny/brain"),
        "the core's brain is an `llm` cell and talks to its stub: {pointed:?}"
    );
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    h.send(menu_tick()).await;
    seed(&h, &mut ports, &root).await;
    wait_for_menus(
        &root,
        &[
            ("talky", "consult_cogny"),
            ("cogny", "file_ws_open"),
            ("cogny", "file_replace"),
            ("cogny", "file_ws_commit"),
        ],
    )
    .await;

    h.send(person("t-1", SAID_B)).await;
    let mut seen = Vec::new();
    next_matching(
        &mut ports.sink,
        &root,
        "the talky's interim",
        DEADLINE,
        |m| said(m).contains(INTERIM_B),
        &mut seen,
    )
    .await;
    // Four calls of the core, each a tool round through the space, then the
    // advice back to the talky and one more call there: twice the one-wait
    // budget, still a failure marker.
    next_matching(
        &mut ports.sink,
        &root,
        "the talky's answer after the core's advice",
        DEADLINE * 2,
        |m| said(m).contains(FINAL_B),
        &mut seen,
    )
    .await;

    // The history, asked of the space directly.
    h.send(space_request(
        "in_read",
        "history",
        HISTORY_ID,
        Some(PATH),
        json!({"limit": 5}),
    ))
    .await;
    let mut fseen = Vec::new();
    let history = next_matching(
        &mut ports.fsink,
        &root,
        "the history's answer",
        DEADLINE,
        |m| hop_str(m, "route") == "answer" && hop_str(m, "op_id") == HISTORY_ID,
        &mut fseen,
    )
    .await;
    tokio::time::sleep(SETTLE).await;

    let log = message_log(&root);
    let dead = dead_letters(&root);
    let errors = parked_errors(&mut ports.park);
    let voice_reqs = voice.recorded_requests().await;
    let core_reqs = core.recorded_requests().await;
    h.shutdown().await;

    // The provider side: the talky consulted and answered; the core ran three
    // tool rounds and answered.
    assert_eq!(voice_reqs.len(), 2, "the consult, then the advice");
    assert_eq!(
        core_reqs.len(),
        4,
        "ws_open, replace, commit -- one round each -- then the core's answer"
    );
    let offered = tools_offered(&core_reqs[0]);
    for t in [
        "file_ws_open",
        "file_replace",
        "file_ws_commit",
        "file_read",
    ] {
        assert!(
            offered.iter().any(|o| o == t),
            "the core declares `*` and is offered `{t}`: {offered:?}"
        );
    }

    let opened = tool_content(&core_reqs[1], WS_OPEN_ID);
    assert!(
        opened.contains("\"ok\": true") && opened.contains(WS),
        "the workspace opened: {opened}"
    );
    let replaced = tool_content(&core_reqs[2], REPLACE_ID);
    assert!(
        replaced.contains("\"ok\": true") && replaced.contains(&format!("\"version\": \"{new}\"")),
        "the replace against base `{base}` landed in the workspace as `{new}`: {replaced}"
    );
    let committed = tool_content(&core_reqs[3], COMMIT_ID);
    assert!(
        committed.contains("\"ok\": true")
            && committed.contains("\"commit\": \"")
            && committed.contains(&new),
        "the commit names its id and the file's new token `{new}`: {committed}"
    );

    // The main line moved, by the commit: the history's top entry names it.
    let hb = body_of(&history);
    assert_eq!(hb["ok"], json!(true), "{hb}");
    assert_eq!(
        text_of(hb, "version"),
        new,
        "the head is the replaced content: {hb}"
    );
    let top = &hb["entries"][0];
    assert!(
        !text_of(top, "commit").is_empty(),
        "the newest history entry is the commit's: {hb}"
    );
    assert_eq!(text_of(top, "version"), new, "{hb}");
    assert!(
        hb["entries"]
            .as_array()
            .is_some_and(|es| es.iter().any(|e| text_of(e, "version") == base)),
        "the seeded version stays in the history below it: {hb}"
    );

    // At the seams: three calls at the space's door stamped 'cogny', three
    // results back to the core and none to a voice.
    let calls: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/file-space" && r.route() == "in_tool")
        .collect();
    assert_eq!(
        calls.len(),
        3,
        "three calls at the space's door: {calls:#?}"
    );
    for r in &calls {
        assert!(
            // The log names the sender at the hive boundary it crossed
            // (`/assistants`), not the surface inside it; which surface
            // sent the call is the stamp below.
            r.from.starts_with("/assistants"),
            "a call from the core: {r:?}"
        );
        assert_eq!(text_of(&r.context, "tool_caller"), "cogny", "{r:?}");
    }
    let ids = [WS_OPEN_ID, REPLACE_ID, COMMIT_ID];
    let back: BTreeSet<String> = log
        .iter()
        .filter(|r| r.to == "/assistants/cogny" && r.route() == "in_tool")
        .map(|r| text_of(&r.hop, "tool_call_id"))
        .filter(|id| ids.contains(&id.as_str()))
        .collect();
    assert_eq!(
        back,
        ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        "every result comes back to the core under its call id"
    );
    let astray: Vec<&Logged> = log
        .iter()
        .filter(|r| {
            (r.to == "/assistants/talky" || r.to == "/assistants/talky-chat")
                && r.route() == "in_tool"
                && ids.contains(&text_of(&r.hop, "tool_call_id").as_str())
        })
        .collect();
    assert!(
        astray.is_empty(),
        "a core's result reached a voice: {astray:#?}"
    );

    assert!(errors.is_empty(), "an error left the run: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// Run 3: a write or workspace call from anyone but the core is refused at the
/// space's door, without asking any other cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_call_without_the_core_mark_is_refused_read_only() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let embed = embed_stub();
    let stubs = Stubs {
        surface: background.base_url.clone(),
        core: background.base_url.clone(),
        background: background.base_url.clone(),
        embed: embed.url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build_space(&td, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    let replace_args =
        json!({"file": PATH, "base": token(TEXT), "old": OLD, "new": NEW}).to_string();
    let open_args = json!({"name": WS}).to_string();
    let cases: Vec<(String, &str, String, Option<&str>)> = vec![
        (
            format!("{RO_REPLACE_ID}-none"),
            "file_replace",
            replace_args.clone(),
            None,
        ),
        (
            format!("{RO_REPLACE_ID}-talky"),
            "file_replace",
            replace_args,
            Some("talky"),
        ),
        (
            format!("{RO_OPEN_ID}-none"),
            "file_ws_open",
            open_args.clone(),
            None,
        ),
        (
            format!("{RO_OPEN_ID}-chat"),
            "file_ws_open",
            open_args,
            Some("talky-chat"),
        ),
    ];
    for (id, name, args, caller) in &cases {
        h.send(tool_call(name, id, args, *caller)).await;
        let mut seen = Vec::new();
        let m = next_matching(
            &mut ports.sink,
            &root,
            &format!("the result of `{id}`"),
            DEADLINE,
            |m| hop_str(m, "route") == "tool_result" && hop_str(m, "tool_call_id") == *id,
            &mut seen,
        )
        .await;
        assert!(seen.is_empty(), "nothing but the one result: {seen:#?}");
        assert_eq!(
            hop_str(&m, "error_code"),
            "read_only",
            "`{name}` from {caller:?}: {:?}",
            m.headers.hop
        );
        let turns = body_of(&m)["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(turns.len(), 1, "one turn: {turns:?}");
        assert_eq!(turns[0]["id"], json!(id), "under the call's id: {turns:?}");
        let answer: Value = sj::from_str(turns[0]["text"].as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("the result's text is the answer as JSON: {e}: {turns:?}"));
        assert_eq!(answer["ok"], json!(false), "{answer}");
        assert_eq!(answer["error"]["code"], json!("read_only"), "{answer}");
    }
    tokio::time::sleep(SETTLE).await;

    let log = message_log(&root);
    let dead = dead_letters(&root);
    let errors = parked_errors(&mut ports.park);
    h.shutdown().await;

    let moved: Vec<&Logged> = log
        .iter()
        .filter(|r| {
            r.to.starts_with("/file-space/")
                && r.to != "/file-space/tools"
                && !r.to.starts_with("/file-space/tools/")
        })
        .collect();
    assert!(
        moved.is_empty(),
        "a refused call reached a cell of the space beyond `./tools`: {moved:#?}"
    );
    assert!(errors.is_empty(), "an error left the run: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}
