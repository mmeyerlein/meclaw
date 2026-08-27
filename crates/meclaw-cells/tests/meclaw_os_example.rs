//! meclaw-os -- `examples/meclaw-os` really grows into an agent.
//!
//! The example makes one claim, and it is the claim the whole idea stands on:
//! an EMPTY folder plus a template library plus ONE declaration is a working
//! agent. A README cannot hold that claim; only a run can. So this file takes
//! the shipped seed and the shipped `grow.json` verbatim -- no inlined copy, no
//! paraphrase -- boots the one, applies the other, and drives a turn all the way
//! through:
//!
//!   HTTP turn -> door -> firewall screen -> talky keeper -> seam ->
//!   brain(mock) -> split -> seam -> answer
//!
//! Sixteen cells, and NOBODY wrote a single one of them here: one comes from
//! `door@1`, two from `firewall@1`, twelve from `talky`, one from
//! `terminal@1`. What is checked in is two config files -- a colony default and
//! a hive with an empty graph.
//!
//! Then the second declaration, on the colony that is already up and answering:
//! `grow-cogny.json` adds the thinking core, five more cells and three edges,
//! without a restart.
//!
//! The two templates this example brought into the library -- `door@1` and
//! `terminal@1` -- are pinned here rather than in files of their own. They are
//! single `code` cells with no wiring semantics of their own (unlike `retry`'s
//! counter or `firewall`'s rules), they exist FOR this example, and their colony
//! half would be a copy of the run below: the turn enters through the door and
//! two lanes end in the terminal -- the two that are genuinely undecided, since
//! GH #284 took the `reject` and the `error` edge out of the declaration and
//! left them to the dead-letter queue. What the E2E cannot show -- the door's
//! channel fallback, the terminal's empty emission -- is pinned against the
//! shipped `script_inline` directly, in the shape `retry_template.rs` uses.
//!
//! Free of a real provider by construction: every `llm` cell of both composites
//! talks to the mock OpenAI wire, everything else is a `code`/`store`/`timer`
//! cell. The file spends nothing.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ─────────────────────────────────────────────────────── the shipped example

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn example_path(rel: &str) -> std::path::PathBuf {
    repo_path("examples/meclaw-os").join(rel)
}

/// The templates `grow.json` names, each next to the path this test really
/// reads, in the order the declaration names them. All four are part of the
/// public distribution, so the file runs in the open clone exactly as it does
/// here -- and the paths are spelled out rather than formatted, so the export's
/// R2b check can read the names off them (GH #9: a runtime path a gate cannot
/// see is the whole defect class).
const GROWN_FROM: [(&str, &str); 4] = [
    ("door", "templates/door"),
    ("firewall", "templates/firewall"),
    ("talky", "templates/talky"),
    ("terminal", "templates/terminal"),
];

/// The second declaration names exactly one template, and it ships too.
const GROWN_FROM_COGNY: [(&str, &str); 1] = [("cogny", "templates/cogny")];

/// GH #277: `talky` REFERENCES its four sub-units instead of carrying copies of
/// them, so the library the colony scans has to hold them next to it. They are
/// NOT `grow.json` entries -- the mutation still names `talky` alone, and the
/// registry resolves the rest.
const REFERENCED_SUB_UNITS: [(&str, &str); 4] = [
    ("collector", "templates/collector"),
    ("summarizer", "templates/summarizer"),
    ("session-keeper", "templates/session-keeper"),
    ("dispatcher", "templates/dispatcher"),
];

/// One cell from `door@1`, two from `firewall@1`, twelve from `talky` (the
/// twelfth is the sidecar `splitter`, `talky@4.1.0`, GH #379), one from
/// `terminal@1`.
const CELLS_AFTER_GROW: usize = 16;

/// Plus five from `cogny`: the two brains -- the thinking lane and the
/// lookup lane of 1.1.0 -- the two collector cells and the split.
const CELLS_AFTER_COGNY: usize = 21;

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Copy a directory tree verbatim -- template seeds (`rules.jsonl`) travel too.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn patch(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ═════════════════════════════════════════════════════ 1. the seed IS empty

/// The premise, measured rather than asserted in prose: what is checked in is a
/// colony config and a hive with an EMPTY graph. Not one cell, not one edge.
#[test]
fn the_seed_carries_two_files_and_not_one_cell() {
    let mut files: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("seed dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .expect("under seed")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let seed = example_path("seed");
    walk(&seed, &seed, &mut files);
    files.sort();
    assert_eq!(
        files,
        vec!["colony.json".to_string(), "main/config.json".to_string()],
        "the seed grew a file -- either it belongs in a template or the README lies"
    );

    let hive = read_json(&seed.join("main/config.json"));
    assert_eq!(hive["cell"]["type"], json!("hive"));
    assert_eq!(
        hive["params"]["graph"]["edges"],
        json!([]),
        "the seed draws NO edge: every edge of this colony is in grow.json"
    );
}

fn assert_declaration_ships(file: &str, expected_from: &[(&str, &str)]) {
    let grow = read_json(&example_path(file));
    let named: Vec<&str> = grow["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .map(|n| n["template"].as_str().expect("template"))
        .collect();
    let expected: Vec<&str> = expected_from.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, expected, "{file} grew a template: {named:?}");
    for (name, dir) in expected_from {
        assert!(
            repo_path(dir).join("template.json").is_file(),
            "{name}@1 is missing from the tree this test runs in"
        );
    }
}

/// `grow.json` names only templates that ship publicly -- otherwise the example
/// is a promise the open clone cannot keep.
#[test]
fn grow_json_only_names_templates_that_ship() {
    assert_declaration_ships("grow.json", &GROWN_FROM);
}

/// The same for the second step: `cogny@1` went public on 2026-08-15, so the
/// thinking core is a regular grow step and no longer a shape to look at.
#[test]
fn grow_cogny_json_only_names_templates_that_ship() {
    assert_declaration_ships("grow-cogny.json", &GROWN_FROM_COGNY);
}

/// GH #298, ruling Q11: `memory-drain` left the live stack.
///
/// This example never had a memory behind its episode edge -- the drain was a
/// decomposer feeding a `terminal`. What replaces it is the talky's own
/// per-turn route ending at that same sink: a lane that ends HERE is a decision
/// this example has not made for you, which is what `terminal@1` is for. So no
/// node grows from `memory-drain`, no edge names `./drain`, and the lane that
/// carries what was just said is the collector's own `turn_write`.
#[test]
fn grow_json_ends_the_per_turn_route_at_the_terminal() {
    let grow = read_json(&example_path("grow.json"));
    let edges = grow["diff"]["add_edges"].as_array().expect("add_edges");
    assert!(
        !edges
            .iter()
            .any(|e| e["from"] == json!("./drain") || e["to"] == json!("./drain")),
        "an edge still names ./drain -- the drain is back on the live path"
    );

    let turn_write = edges
        .iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'turn_write'"))
        })
        .expect("the per-turn route is not wired at all");
    assert_eq!(turn_write["from"], json!("./talky"));
    assert_eq!(
        turn_write["to"],
        json!("./sink"),
        "the per-turn route ends somewhere other than the terminal"
    );
}

// ════════════════════ 2. the two templates this example put into the library

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Runs a shipped `params.script_inline` against a real stdin document.
fn run_script(template_dir: &str, doc: Value) -> String {
    let cfg = read_json(&repo_path(template_dir).join("config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let out = run_script_on_stdin(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "{template_dir} script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 output")
}

fn door_emission(context: Value, hop: Value) -> Value {
    let raw = run_script(
        "templates/door",
        json!({
            "header": {"context": context, "hop": hop},
            "messages": [{"origin": "user", "type": "text", "text": "hi"}],
        }),
    );
    meclaw_core::serde_json::from_str(&raw).expect("door emits json")
}

/// `door@1` does the one thing no edge can do above the first cell: it names the
/// lane. And it carries the channel identity, with a fallback, so a colony
/// without a channel notion needs no special case in its wiring.
#[test]
fn the_door_names_the_lane_and_promotes_the_channel() {
    let with_channel = door_emission(json!({"channel": "chat-1"}), json!({}));
    assert_eq!(with_channel["header"]["route"], json!("turn"));
    assert_eq!(with_channel["header"]["chat_id"], json!("chat-1"));
    assert_eq!(
        with_channel["messages"],
        json!([{"origin": "user", "type": "text", "text": "hi"}]),
        "the door touched the body -- it decides nothing about content"
    );

    let from_hop = door_emission(json!({}), json!({"chat_id": "chat-2"}));
    assert_eq!(from_hop["header"]["chat_id"], json!("chat-2"));

    let bare = door_emission(json!({}), json!({}));
    assert_eq!(
        bare["header"]["chat_id"],
        json!("default"),
        "a request without a channel must still arrive on a named lane"
    );
    assert_eq!(bare["header"]["route"], json!("turn"));
}

/// `terminal@1` swallows, and that is the whole contract: an emission would make
/// it a component, and a missing destination would dead-letter.
#[test]
fn the_terminal_swallows_every_lane() {
    for doc in [
        json!({"header": {"hop": {"route": "answer"}}, "messages": [{"type": "text", "text": "x"}]}),
        json!({"header": {"hop": {"route": "reject"}}, "messages": []}),
        json!({"header": {}, "messages": []}),
    ] {
        let raw = run_script("templates/terminal", doc);
        let out: Value = meclaw_core::serde_json::from_str(&raw).expect("terminal emits json");
        assert_eq!(
            out,
            json!([]),
            "the terminal emitted something: it is a stop, not a component"
        );
    }
}

// ══════════════════════════════════════════ 3. the seed really grows into one

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

/// Never during a test run: the shipped keeper default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

/// The seed as the reader gets it, plus the template library next to it and the
/// `.env` the README asks for. Only the provider endpoints are bent -- towards
/// the mock wire, so the run costs nothing.
fn build_root(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    copy_tree(&example_path("seed"), root);
    for (name, dir) in GROWN_FROM
        .iter()
        .chain(GROWN_FROM_COGNY.iter())
        .chain(REFERENCED_SUB_UNITS.iter())
    {
        copy_tree(&repo_path(dir), &root.join("templates").join(name));
    }
    for rel in [
        "templates/talky/brain/config.json",
        "templates/summarizer/writer/config.json",
        "templates/cogny/brain/config.json",
        "templates/cogny/brain_fast/config.json",
    ] {
        patch(&root.join(rel), |v| {
            v["params"]["base_url"] = json!(base_url)
        });
    }
    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\nMODEL_BRAIN=gpt-4o-mock\nMODEL_CORE=gpt-4o-mock\n\
             MODEL_CORE_FAST=gpt-4o-mock-fast\nKEEPER_NIGHT_CRON={NEVER}\n"
        ),
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the seed must boot");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    h
}

/// A shipped declaration, applied verbatim -- the file the reader `curl`s at
/// `/colony/mutations` is the file this test hands the colony.
async fn grow(h: &ColonyHandle, file: &str) -> meclaw_colony::mutation::MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: read_json(&example_path(file)),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

fn assert_committed(file: &str, outcome: &meclaw_colony::mutation::MutationOutcome) {
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "{file} was not committed: {outcome:?}"
    );
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read registry");
    let mut v: Vec<String> = ack_rx
        .await
        .expect("registry ack")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    v.sort();
    v
}

fn turn(channel: &str, text: &str) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("channel".into(), json!(channel));
    MessageBuilder::new(Path::new("/door"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(200)
        .build()
}

fn texts(m: &Message) -> Vec<String> {
    match &m.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["text"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        Body::Blob(_) => Vec::new(),
    }
}

/// The whole claim of the example in one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_seed_plus_grow_json_is_a_living_agent() {
    let mock = MockOpenAI::start(vec![canned_chat_completion(
        "Berlin, and it is raining.",
        "stop",
    )])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_root(&td, &mock.base_url);

    // --- the seed alone: NOTHING. Not one cell.
    let h = boot(&td).await;
    let before = registry_paths(&h).await;
    assert!(
        before.is_empty(),
        "the seed booted with cells in it -- it is supposed to be empty: {before:?}"
    );

    // --- ONE declaration
    let outcome = grow(&h, "grow.json").await;
    assert_committed("grow.json", &outcome);

    // --- ... and the tree is an agent
    let after = registry_paths(&h).await;
    for expected in [
        "/door",
        "/firewall/screen",
        "/firewall/rules",
        "/talky/session-keeper/stamp",
        "/talky/session-keeper/close",
        "/talky/session-keeper/sessions",
        "/talky/session-keeper/night",
        "/talky/collector/assemble",
        "/talky/collector/window",
        "/talky/dispatcher",
        "/talky/brain",
        "/talky/summarizer/prep",
        "/talky/summarizer/writer",
        "/talky/errors",
        "/sink",
    ] {
        assert!(
            after.iter().any(|p| p == expected),
            "{expected} did not grow: {after:?}"
        );
    }
    // GH #298, ruling Q11: the drain left the live stack, so neither of its two
    // cells grows here any more.
    for gone in ["/drain/drain", "/drain/ledger"] {
        assert!(
            !after.iter().any(|p| p == gone),
            "{gone} still grows: the drain is back on the live path: {after:?}"
        );
    }
    assert_eq!(
        after.len(),
        CELLS_AFTER_GROW,
        "zero checked-in cells plus sixteen instantiated ones: {after:?}"
    );

    // --- the liveness proof: one turn, all the way through.
    // The probe is test-only wiring; it watches the reply port the example
    // sends to its terminal, because a terminal that swallows leaves nothing
    // to assert.
    let (probe_tx, mut probe_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/probe"), move || {
        CaptureCell::new(probe_tx.clone())
    })
    .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/talky/collector/assemble"),
        Path::new("/probe"),
    )
    .await;

    h.send(turn("chat-1", "Where am I and what is the weather?"))
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut seen: Vec<String> = Vec::new();
    let answer = loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "no answer reached the reply port: {seen:?}"
        );
        let Ok(Some(m)) = tokio::time::timeout(left, probe_rx.recv()).await else {
            panic!("the reply port went quiet: {seen:?}");
        };
        let t = texts(&m);
        seen.push(format!("{:?}", m.headers.hop.get("route")));
        if t.iter().any(|s| s == "Berlin, and it is raining.") {
            break m;
        }
    };
    assert_eq!(
        answer.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("answer"),
        "the model's words came home on some other lane: {:?}",
        answer.headers.hop
    );
    assert!(
        answer
            .headers
            .hop
            .get("session_id")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "the answer carries no session id, so the keeper never stamped it: {:?}",
        answer.headers.hop
    );

    // --- the SECOND declaration, on a colony that is up and has already
    // answered: growing is not a boot-time act.
    let outcome = grow(&h, "grow-cogny.json").await;
    assert_committed("grow-cogny.json", &outcome);
    let with_core = registry_paths(&h).await;
    for expected in [
        "/cogny/brain",
        "/cogny/brain_fast",
        "/cogny/collector/assemble",
        "/cogny/collector/window",
        "/cogny/dispatcher",
    ] {
        assert!(
            with_core.iter().any(|p| p == expected),
            "{expected} did not grow: {with_core:?}"
        );
    }
    assert_eq!(
        with_core.len(),
        CELLS_AFTER_COGNY + 1,
        "sixteen plus the core's five, plus the test-only probe: {with_core:?}"
    );

    h.shutdown().await;
}
