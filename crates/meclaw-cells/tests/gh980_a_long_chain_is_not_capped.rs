//! GH #980 -- a long tool chain of the core is not capped.
//!
//! A writing core works in ONE round -- read, write, test, commit, export,
//! push, note: ten to thirteen tool calls, one per provider answer. The
//! collector ends a round at its cap (`collector/assemble`, `spent = it >=
//! MAX_ITER`) and leaves on `answer` with `hop.round_capped == "1"` and a
//! partial answer. At the default `max_iter` of 8 such a chain was cut off.
//! GH #980 overrides cogny's `assemble.max_iter` to 16 (`round_idle_ms`
//! 630000) and moves the guards on cogny's own edges (`./collector ->
//! ./curator` on `curate`, `./curator -> ./brain` on `brain`) from
//! `int(hop.iter) < 12` to `< 20`, so no edge cuts a round the collector lets
//! run.
//!
//! The bound lives in a script, the guards in edges, the counter travels on
//! the hop and the context -- so this file boots the SHIPPED `cogny` with a
//! scripted stub behind the brain and measures at the receiver (the answer
//! that leaves the composite, the requests the stub recorded):
//!
//! 1. **Thirteen calls are not capped**: fourteen requests, the scripted text,
//!    no cap mark.
//! 2. **Seventeen calls are**: `round_capped == "1"`, `partial == "1"`, and
//!    exactly as many requests as `max_iter = 16` allows.
//! 3. **The bounds agree**: the edge guards sit above `max_iter`, and a
//!    round's idle window outlasts one projection run.
//!
//! No dead letter in either run. Every rim lane of cogny is drained (read off
//! its contract), the tool is one `code` cell, and EVERY `llm` cell points at a
//! local stub -- no real provider by construction. Guarded like every
//! template-reading test (GH #49): a tree without the templates is skipped.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator. A
/// chain of seventeen rounds runs every cell of the round seventeen times, so
/// the marker sits further out than for a single round.
const DEADLINE: Duration = Duration::from_secs(180);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const LOOKUP_RESULT: &str = "step done";
const FINAL: &str = "All thirteen steps are done.";

const CONSULT_ID: &str = "k-980";
const CONSULT_SESSION: &str = "s-980";
const QUESTION: &str = r#"{"question":"carry the change through all of its steps"}"#;

/// The core's bound, as the cogny ref marker of its collector sets it.
const MAX_ITER: usize = 16;

// ───────────────────────────────────────────────────────────── the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this road boots or reads. One missing = skipped (GH #49).
fn shipped() -> bool {
    ["cogny", "collector", "curator", "dispatcher", "projection"]
        .iter()
        .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
        && repo("templates/collector/assemble/config.json").is_file()
        && repo("templates/projection/run/config.json").is_file()
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
/// the marker's own `override_params` are then applied to the cells they name
/// (GH #140). That is how cogny's collector arrives with `max_iter: 16`.
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

/// One sweep over the tree, three jobs:
///
/// * EVERY `llm` cell points at a local stub -- `cogny/brain` at the scripted
///   one, all others (the curator's summarizer) at the background one, each
///   told the stub's model (`${ctx.model}` is an instantiation-side
///   substitution). Returns the cells it pointed.
/// * every timer is out of the run's way: a cron moves to a date no run
///   reaches, a `${uuid7:*}` schedule id becomes a fixed one.
/// * every `${VAR}` the tree names is bound to a dummy in the run's own env
///   file, so nothing can reach a real endpoint.
fn prepare_tree(root: &std::path::Path, brain: &str, background: &str) -> Vec<String> {
    let main = root.join("main");
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    let mut pointed = Vec::new();
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut n: u64 = 0;
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
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] == "llm" {
            let rel = f
                .parent()
                .expect("a cell directory")
                .strip_prefix(&main)
                .expect("under main")
                .to_string_lossy()
                .replace('\\', "/");
            let (url, model) = if rel == "cogny/brain" {
                (brain, BRAIN_MODEL)
            } else {
                (background, BACKGROUND_MODEL)
            };
            cfg["params"]["base_url"] = json!(url);
            cfg["params"]["model"] = json!(model);
            cfg["params"]["api_key"] = json!("sk-test");
            write_json(&f, &cfg);
            pointed.push(rel);
        } else if cfg["cell"]["type"] == "timer" {
            // `get_mut`, not `cfg["params"]["schedules"]`: the index operator
            // inserts a `null` for a missing key, and a timer without
            // schedules (curator/clock) then refuses to boot with
            // `params.schedules: must be array`.
            let Some(schedules) = cfg["params"]
                .get_mut("schedules")
                .and_then(Value::as_array_mut)
            else {
                continue;
            };
            for s in schedules.iter_mut() {
                n += 1;
                if s["schedule_id"]
                    .as_str()
                    .is_some_and(|id| id.contains("${"))
                {
                    s["schedule_id"] =
                        json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0980_0000 + n));
                }
                if s.get("cron").is_some() {
                    s["cron"] = json!(NEVER_CRON);
                }
            }
            write_json(&f, &cfg);
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    vars.insert("MEMORY_LLM_BASE_URL".into(), background.to_string());
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
    pointed.sort();
    pointed
}

// ────────────────────────────────────────────────────────────── the harness

/// One drain per lane cogny declares at its own path (a lane carrying `at`
/// docks somewhere else, ADR-0020): `answer` to the capture the run waits on,
/// all else to the park. An undrained lane is a dead letter.
fn drains() -> Vec<Value> {
    let cfg = read_json(&repo("templates/cogny/config.json"));
    let emits = cfg["params"]["contract"]["emits"].as_array().cloned();
    emits
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .map(|lane| {
            let to = if lane == "answer" { "/sink" } else { "/park" };
            json!({"from": "./cogny", "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

/// The one per-instance tool lane: which cell answers `lookup`.
fn tool_edges() -> Vec<Value> {
    vec![
        json!({"from": "./cogny", "to": "./lookup",
               "condition": "has(hop.route) && hop.route == 'tool' \
                             && has(hop.tool_name) && hop.tool_name == 'lookup'"}),
        json!({"from": "./lookup", "to": "./cogny",
               "condition": "has(hop.route) && hop.route == 'res'",
               "modifier": {"set_hop": {"route": "'in_tool'"}}}),
    ]
}

/// The tool the stub brain asks for: it answers at once, under the call id it
/// was handed, so every round of the chain can close.
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
                  "text": "%s (%s)" % (doc["params"]["result_text"], call_id)}]}))
"#},
        "contract": {
            "version": "1.0.0", "settings": {}, "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": ["res"], "required": false},
                        "tool_call_id": {"type": "string", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]},
        "description": {"purpose": "Test stand-in: the one tool the stub brain asks for.",
                        "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
    })
}

/// cogny grown into a parent that asks it. Returns the `llm` cells pointed at
/// the stubs.
fn build_cogny(td: &tempfile::TempDir, brain: &str, background: &str) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/cogny"), &main.join("cogny"), 0);
    write_json(&main.join("lookup/config.json"), &lookup_cell());
    let mut edges = drains();
    edges.extend(tool_edges());
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    prepare_tree(root, brain, background)
}

/// The colony, the sink the run waits on, and the park: everything else, held
/// rather than dropped, and wide -- a chain of seventeen parks a copy per call.
type Booted = (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
);

async fn boot(td: &tempfile::TempDir) -> Booted {
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
    let (park_tx, park_rx) = mpsc::channel::<Message>(4096);
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
        .expect("the shipped cogny composite and its curator must boot");
    (h, sink_rx, park_rx)
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// A consult at cogny's door, in the shape a dispatcher sends it and with the
/// context the documented ingress edge sets (`templates/cogny/README.md`).
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

// ──────────────────────────────────────────────────────────────── the readers

fn call_id(n: usize) -> String {
    format!("call-980-{n:02}")
}

/// `n` stub answers, each ONE tool call under its own id and with its own
/// arguments -- one call per provider request, so every call is a round.
fn tool_chain(n: usize) -> Vec<MockResponse> {
    (1..=n)
        .map(|i| {
            let id = call_id(i);
            let args = format!(r#"{{"q":"step {i}"}}"#);
            canned_tool_calls(vec![(id.as_str(), "lookup", args.as_str())])
        })
        .collect()
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

/// A hop key of the arrived answer; `None` when the key is absent.
fn hop_key(m: &Message, key: &str) -> Option<String> {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn shown(v: Option<&str>) -> &str {
    v.unwrap_or("<absent>")
}

/// `(error_code, sender, resolved_target, hop.route)` of one dead letter.
type DeadLetter = (String, String, String, String);

/// Every dead letter the colony recorded -- what `/colony/dead_letters`
/// serves. None is expected: the composite drains every lane of its inner
/// cells and this harness drains every lane of its rim.
fn dead_letters(root: &std::path::Path) -> Vec<DeadLetter> {
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
        let route = m["headers"]["hop"]["route"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (code, sender, target, route)
    })
    .collect()
}

/// What one run leaves behind, all read once the answer has arrived.
struct Run {
    answer: Message,
    reqs: Vec<OpenAiRequestSnapshot>,
    dead: Vec<DeadLetter>,
    /// The hops of everything that left on the `error` lane.
    errors: Vec<Value>,
}

/// Boot cogny behind a brain stub scripted with `script` and send one consult.
async fn run_chain(script: Vec<MockResponse>, what: &str) -> Run {
    let brain = MockOpenAI::start(script).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_cogny(&td, &brain.base_url, &background.base_url);
    assert!(
        pointed.iter().any(|p| p == "cogny/brain"),
        "expected the brain among the `llm` cells pointed at a stub, got {pointed:?}"
    );
    let assemble = read_json(&td.path().join("main/cogny/collector/assemble/config.json"));
    assert_eq!(
        assemble["params"]["max_iter"],
        json!(MAX_ITER),
        "expected the ref's override `max_iter: {MAX_ITER}` on the resolved assemble cell, \
         got {}",
        assemble["params"]["max_iter"]
    );
    let (h, mut sink, mut park) = boot(&td).await;

    h.send(consult()).await;
    // The answer is the receipt; DEADLINE only marks a run that never got one.
    let arrived = tokio::time::timeout(DEADLINE, sink.recv())
        .await
        .ok()
        .flatten();
    let reqs = brain.recorded_requests().await;
    let dead = dead_letters(td.path());
    let Some(answer) = arrived else {
        panic!(
            "{what}: expected an answer to leave cogny within {DEADLINE:?}, got none after {} \
             provider request(s); dead letters: {dead:#?}",
            reqs.len()
        );
    };
    let mut errors = Vec::new();
    while let Ok(m) = park.try_recv() {
        if m.headers.hop.get("route").and_then(Value::as_str) == Some("error") {
            errors.push(Value::Object(m.headers.hop));
        }
    }
    h.shutdown().await;
    Run {
        answer,
        reqs,
        dead,
        errors,
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// What both chains share: the receipt line, the `answer` lane, the exact
/// request count, no `error` lane and no dead letter. Returns the hop's
/// `(round_capped, partial)`, `None` where a key is absent.
fn common(run: &Run, chain: usize, requests: usize) -> (Option<String>, Option<String>) {
    let hop = &run.answer.headers.hop;
    let capped = hop_key(&run.answer, "round_capped");
    println!(
        "gh980 chain={chain} requests={} capped={}",
        run.reqs.len(),
        shown(capped.as_deref())
    );
    assert_eq!(
        hop_key(&run.answer, "route").as_deref(),
        Some("answer"),
        "expected the round to leave on the `answer` lane, got hop {hop:?}"
    );
    assert_eq!(
        run.reqs.len(),
        requests,
        "expected exactly {requests} provider requests for a chain of {chain} under \
         max_iter={MAX_ITER}, got {}",
        run.reqs.len()
    );
    assert!(
        run.errors.is_empty(),
        "expected no `error` lane, got {:#?}",
        run.errors
    );
    assert!(
        run.dead.is_empty(),
        "expected no dead letters, got {:#?}",
        run.dead
    );
    (capped, hop_key(&run.answer, "partial"))
}

/// Thirteen sequential tool calls and a text answer run as ONE uncapped round:
/// fourteen provider requests, the scripted text on `answer`, no cap mark.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_of_thirteen_calls_is_not_capped() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const CHAIN: usize = 13;
    let mut script = tool_chain(CHAIN);
    script.push(canned_chat_completion(FINAL, "stop"));
    let run = run_chain(script, "chain of 13").await;
    // One request per tool call, one more that the text answers.
    let (capped, partial) = common(&run, CHAIN, CHAIN + 1);
    let hop = &run.answer.headers.hop;

    // A real answer leaves the collector through `in_answer`, whose head stamps
    // neither cap key (`head("answer", extra=mark)`; measured and recorded in
    // gh277's notes on GH #570). Absent and "0" both say "not capped"; only "1"
    // -- the spent seam -- is the failure this pin exists for.
    assert!(
        matches!(capped.as_deref(), None | Some("0")),
        "expected an uncapped round (`round_capped` absent or \"0\"), got {} -- hop {hop:?}",
        shown(capped.as_deref())
    );
    assert!(
        matches!(partial.as_deref(), None | Some("0")),
        "expected no partial answer (`partial` absent or \"0\"), got {} -- hop {hop:?}",
        shown(partial.as_deref())
    );
    assert!(
        says(&run.answer, FINAL) && !says(&run.answer, "iteration cap"),
        "expected the scripted final text {FINAL:?} and no partial-answer sentence, got {:?}",
        body_of(&run.answer)
    );
    let last = run.reqs.last().expect("a last request").messages();
    let last_id = call_id(CHAIN);
    assert!(
        last.is_some_and(|ms| {
            ms.iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == json!(last_id))
        }),
        "expected the last request to carry the result of `{last_id}`, got {last:?}"
    );
}

/// A chain longer than the bound is ended by the seam: `round_capped == "1"`,
/// a partial answer naming `max_iter=16`, and exactly as many provider requests
/// as the bound allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_of_seventeen_calls_is_capped() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const CHAIN: usize = 17;
    // The arithmetic of `collector/assemble`:
    //   it = (0 if lane in ("in_turn", ...) else int(ctx.get("iter", 0) or 0))
    //   spent = it >= MAX_ITER                       (curate_message)
    //   emit(seam_out(rows, 0) ...)                  (the opening assembly)
    //   emit(seam_out(rows, it + 1) ...)             (a closed tool round)
    // The consult opens at it = 0 and goes to the brain with iter 0 (request 1).
    // The round of the k-th tool call runs at it = k - 1, so its seam is spent
    // iff k - 1 >= 16: calls 1..=16 each re-enter the brain (requests 2..=17,
    // iter 1..=16), call 17's round runs at it = 16 and leaves on `answer` with
    // round_capped = 1 and hop.iter = "17" (`"iter": str(nxt)`). So the provider
    // sees exactly MAX_ITER + 1 = 17 requests, the seventeenth call still runs,
    // and an eighteenth request never leaves. The script holds more tool-call
    // answers than that and no final text: anything past 17 is a failed cap.
    let run = run_chain(tool_chain(CHAIN + 3), "chain of 17").await;
    let (capped, partial) = common(&run, CHAIN, MAX_ITER + 1);
    let hop = &run.answer.headers.hop;
    let iter = hop_key(&run.answer, "iter");

    assert_eq!(
        capped.as_deref(),
        Some("1"),
        "expected `round_capped == \"1\"` at the bound, got {} -- hop {hop:?}",
        shown(capped.as_deref())
    );
    assert_eq!(
        partial.as_deref(),
        Some("1"),
        "expected `partial == \"1\"` at the bound, got {} -- hop {hop:?}",
        shown(partial.as_deref())
    );
    assert_eq!(
        iter.as_deref(),
        Some("17"),
        "expected the spent seam to carry iter \"17\" (it + 1 after the round at it = 16), \
         got {} -- hop {hop:?}",
        shown(iter.as_deref())
    );
    assert!(
        says(&run.answer, &format!("max_iter={MAX_ITER}")),
        "expected the partial answer to name `max_iter={MAX_ITER}`, got {:?}",
        body_of(&run.answer)
    );
}

/// A number at `v`, or a failure that says where it was expected.
fn number(v: &Value, what: &str) -> u64 {
    v.as_u64()
        .unwrap_or_else(|| panic!("expected a number at {what}, got {v}"))
}

/// The core's bounds agree with each other: the edge guards sit strictly above
/// the collector's cap, and a round's idle window outlasts one projection run.
#[test]
fn the_core_bounds_agree() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let marker = read_json(&repo("templates/cogny/collector/config.json"));
    let over = &marker["override_params"]["assemble"];
    let max_iter = number(&over["max_iter"], "assemble.max_iter");
    let idle = number(&over["round_idle_ms"], "assemble.round_idle_ms");
    assert_eq!(
        (max_iter, idle),
        (MAX_ITER as u64, 630_000),
        "expected cogny's collector override (max_iter, round_idle_ms) = ({MAX_ITER}, \
         630000), got ({max_iter}, {idle})"
    );

    // Both guards: the number after `int(hop.iter) < ` on the edge, strictly
    // above max_iter, so no edge cuts a round the collector still lets run.
    let cogny = read_json(&repo("templates/cogny/config.json"));
    let edges = cogny["params"]["graph"]["edges"].as_array().cloned();
    let edges = edges.unwrap_or_default();
    let needle = "int(hop.iter) < ";
    for (from, to, lane) in [
        ("./collector", "./curator", "curate"),
        ("./curator", "./brain", "brain"),
    ] {
        let condition = edges
            .iter()
            .filter(|e| e["from"] == json!(from) && e["to"] == json!(to))
            .filter_map(|e| e["condition"].as_str())
            .find(|c| c.contains(&format!("hop.route == '{lane}'")) && c.contains(needle))
            .unwrap_or_else(|| {
                panic!("expected an edge {from} -> {to} on `{lane}` guarded by `{needle}N`")
            });
        let at = condition.find(needle).expect("the needle") + needle.len();
        let bound: u64 = condition[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or_else(|e| panic!("expected a number after `{needle}` in {condition:?}: {e}"));
        assert!(
            bound > max_iter,
            "expected the guard N of {from} -> {to} strictly above max_iter {max_iter}, \
             got N = {bound} in {condition:?}"
        );
    }

    // A round waiting on one program run (`projection/run`, capped at its
    // `exec_timeout_ms`) must not be called idle before the run can answer.
    let run = read_json(&repo("templates/projection/run/config.json"));
    let exec = number(
        &run["params"]["exec_timeout_ms"],
        "projection/run exec_timeout_ms",
    );
    assert!(
        idle >= exec + 30_000,
        "expected round_idle_ms >= exec_timeout_ms + 30 s = {}, got {idle}",
        exec + 30_000
    );
    let assemble = read_json(&repo("templates/collector/assemble/config.json"));
    let default_idle = number(
        &assemble["params"]["round_idle_ms"],
        "collector round_idle_ms",
    );
    assert!(
        idle >= 120_000 && idle >= default_idle,
        "expected round_idle_ms >= 120000 and >= the collector default {default_idle}, got {idle}"
    );
}
