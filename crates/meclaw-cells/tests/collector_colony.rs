//! meclaw-os 1 -- the collector hive in a running colony (GitHub #27).
//!
//! The script-level pins live in `collector_window.rs`. This file asks the
//! question the issue is actually about, and it asks it of a COLONY: can a
//! conversation reference its own previous turns without relying on retrieval
//! luck? There is no memory hive in these trees at all -- no recall port, no
//! embedder, no index. If turn 3 can name what turn 1 said, the rolling window
//! is the only thing that could have carried it.
//!
//! Free by construction: the brain is a `code` cell that reports what it was
//! given rather than a model that guesses it, so every assertion is about the
//! context that was ASSEMBLED, not about what an LLM made of it.

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::{boot, recv_bounded};
use tokio::sync::mpsc;

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
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

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builder/templates/collector")
}

/// A `code` cell config with the contract the substrate validates against.
fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({});
    if !routes.is_empty() {
        hop["route"] = json!({"type": "string", "values": routes, "required": false});
    }
    if let Some(obj) = extra_hop.as_object() {
        for (k, v) in obj {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for a colony that exercises the collector ports.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Turns a harness message into an inbound turn -- or, on the magic texts,
/// into the close request a session keeper would send or the prune request a
/// timer would send. The lane name is set by the PORT EDGE, which is what
/// makes this a port test and not a script test.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
last = str(msgs[-1].get("text", "")) if msgs else ""
route = {"/close": "close", "/prune": "prune", "/sweep": "sweep"}.get(last, "turn")
sys.stdout.write(json.dumps({"header": {"route": route}, "messages": msgs}))
"#;

/// A brain that answers by REPORTING its context: how many turns it was given,
/// what the oldest user turn in it said, what the newest said. A model would
/// have to be believed; this cell can be measured.
const BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
users = [str(m.get("text", "")) for m in msgs if m.get("origin") == "user"]
ans = "seen=%d|first=%s|last=%s" % (len(msgs), users[0] if users else "<none>",
                                    users[-1] if users else "<none>")
sys.stdout.write(json.dumps({"header": {"finish_reason": "stop"},
                             "messages": [{"origin": "assistant", "type": "text", "text": ans}]}))
"#;

/// The same brain with one tool round in front of it: iteration 0 asks two
/// tools, iteration 1 answers and reports what it was given.
const TOOL_BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)
ctx = (d.get("header") or {}).get("context") or {}
it = int(ctx.get("iter", 0) or 0)
msgs = d.get("messages", [])
if it == 0:
    out = {"header": {"finish_reason": "tool_calls"},
           "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "alpha"},
                        {"origin": "assistant", "type": "tool_call", "id": "c2", "text": "beta"}]}
else:
    users = [str(m.get("text", "")) for m in msgs if m.get("origin") == "user"]
    res = [str(m.get("text", "")) for m in msgs if m.get("type") == "tool_result"]
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text",
                         "text": "seen=%d|first=%s|tools=%d|%s" % (
                             len(msgs), users[0] if users else "<none>", len(res),
                             ",".join(sorted(res)))}]}
sys.stdout.write(json.dumps(out))
"#;

/// A brain that never stops asking for tools. Nothing in the topology bounds
/// it; the seam has to.
const RUNAWAY_BRAIN: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps(
    {"header": {"finish_reason": "tool_calls"},
     "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "again"}]}))
"#;

/// A brain for the GH #103 cases: it opens a two-tool round exactly once (on
/// the literal turn "look it up" at iteration 0) and otherwise REPORTS its
/// context -- every user turn, every tool result, and the two #103 hop flags.
const ROBUST_BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)
h = d.get("header") or {}
ctx = h.get("context") or {}
hop = h.get("hop") or {}
it = int(ctx.get("iter", 0) or 0)
msgs = d.get("messages", [])
users = [str(m.get("text", "")) for m in msgs if m.get("origin") == "user"]
res = [str(m.get("text", "")) for m in msgs if m.get("type") == "tool_result"]
if it == 0 and users and users[-1] == "look it up":
    out = {"header": {"finish_reason": "tool_calls"},
           "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "alpha"},
                        {"origin": "assistant", "type": "tool_call", "id": "c2", "text": "beta"}]}
else:
    ans = "users=%s|tools=%d|res=%s|stale=%s|deferred=%s" % (
        ";".join(users), len(res), ";".join(sorted(res)),
        hop.get("round_stale", ""), hop.get("round_deferred", ""))
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text", "text": ans}]}
sys.stdout.write(json.dumps(out))
"#;

/// A tool that answers the `alpha` call and swallows every other one -- the
/// lost message of GH #103, reproduced deterministically.
const DROPPY_TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
if str(c.get("text", "")) != "alpha":
    sys.stdout.write(json.dumps([]))
    sys.exit(0)
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "result-alpha"}]}))
"#;

/// A brain that reports the SIZE of what it was handed: the only thing a cap
/// can be measured by from the outside.
const MEASURE_BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)
ctx = (d.get("header") or {}).get("context") or {}
it = int(ctx.get("iter", 0) or 0)
msgs = d.get("messages", [])
if it == 0:
    out = {"header": {"finish_reason": "tool_calls"},
           "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "alpha"}]}
else:
    res = [len(str(m.get("text", ""))) for m in msgs if m.get("type") == "tool_result"]
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text",
                         "text": "tools=%d|chars=%s" % (len(res),
                                                        ",".join(str(x) for x in res))}]}
sys.stdout.write(json.dumps(out))
"#;

/// A tool whose answer is larger than any context window it could enter.
const BIG_TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "z" * 100000}]}))
"#;

const DISPATCH: &str = r#"
import sys, json
d = json.load(sys.stdin)
calls = [m for m in d.get("messages", []) if m.get("type") == "tool_call"]
out = [{"header": {"route": "asst"}, "messages": calls}]
for c in calls:
    out.append({"header": {"route": "tool"}, "messages": [c]})
sys.stdout.write(json.dumps(out))
"#;

const TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "result-" + str(c.get("text", ""))}]}))
"#;

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The port wiring a parent draws around the collector. Five entry lanes exist;
/// a tree that has no tools wires three of them.
fn main_config(with_tools: bool) -> Value {
    let mut edges = vec![
        json!({"from": "./probe", "to": "./collector/assemble",
               "condition": "hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"}}}),
        // The close port and the batch it produces. A tree that never closes a
        // session simply never takes these two edges.
        json!({"from": "./probe", "to": "./collector/assemble",
               "condition": "hop.route == 'close'",
               "modifier": {"set_hop": {"route": "'in_close'"}}}),
        json!({"from": "./collector/assemble", "to": "/sink",
               "condition": "hop.route == 'write'"}),
        // The prune port and its report (GH #76). The template never fires
        // this itself; here the probe stands in for the timer a parent tree
        // would wire to the lane.
        json!({"from": "./probe", "to": "./collector/assemble",
               "condition": "hop.route == 'prune'",
               "modifier": {"set_hop": {"route": "'in_prune'"}}}),
        // The round sweep port (GH #103): the same timer stand-in asks
        // whether any tool round is stuck behind the idle window.
        json!({"from": "./probe", "to": "./collector/assemble",
               "condition": "hop.route == 'sweep'",
               "modifier": {"set_hop": {"route": "'in_round_sweep'"}}}),
        json!({"from": "./collector/assemble", "to": "/sink",
               "condition": "hop.route == 'prune'"}),
        json!({"from": "./collector/assemble", "to": "./brain",
               "condition": "hop.route == 'brain'",
               "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                            "session_id": "hop.session_id",
                                            "iter": "hop.iter"}}}),
        json!({"from": "./brain", "to": "./collector/assemble",
               "condition": "hop.finish_reason == 'stop'",
               "modifier": {"set_hop": {"route": "'in_answer'"}}}),
        json!({"from": "./collector/assemble", "to": "/sink",
               "condition": "hop.route == 'answer'"}),
    ];
    if with_tools {
        edges.push(json!({"from": "./brain", "to": "./dispatch",
                          "condition": "hop.finish_reason == 'tool_calls'"}));
        edges.push(json!({"from": "./dispatch", "to": "./collector/assemble",
                          "condition": "hop.route == 'asst'",
                          "modifier": {"set_hop": {"route": "'in_calls'"}}}));
        edges.push(json!({"from": "./dispatch", "to": "./tool",
                          "condition": "hop.route == 'tool'"}));
        edges.push(json!({"from": "./tool", "to": "./collector/assemble",
                          "condition": "hop.route == 'res'",
                          "modifier": {"set_hop": {"route": "'in_tool'"}}}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, env: &str, with_tools: bool) {
    if with_tools {
        build_tool_tree(td, env, TOOL_BRAIN, TOOL);
        return;
    }
    build_base(td, env, false);
    write(
        td.path(),
        "main/brain/config.json",
        &code_cell(BRAIN, &[], finish_hop()),
    );
}

/// The same tree with a tool lane, but with the brain and the tool named by
/// the case: what a cap does is only visible against a specific pair.
fn build_tool_tree(td: &tempfile::TempDir, env: &str, brain: &str, tool: &str) {
    build_base(td, env, true);
    let root = td.path();
    write(
        root,
        "main/brain/config.json",
        &code_cell(brain, &[], finish_hop()),
    );
    write(
        root,
        "main/dispatch/config.json",
        &code_cell(DISPATCH, &["asst", "tool"], json!({})),
    );
    write(
        root,
        "main/tool/config.json",
        &code_cell(tool, &["res"], json!({})),
    );
}

fn finish_hop() -> Value {
    json!({"finish_reason": {"type": "string",
                             "values": ["stop", "tool_calls"], "required": true}})
}

fn build_base(td: &tempfile::TempDir, env: &str, with_tools: bool) {
    let root = td.path();
    std::fs::write(root.join(".env"), env).unwrap();
    write(root, "main/config.json", &main_config(with_tools));
    copy_cells(&template_dir(), &root.join("main/collector"));
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["turn", "close", "prune", "sweep"], json!({})),
    );
}

fn turn_in(session: &str, text: &str) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("session_id".into(), json!(session));
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(64)
        .build()
}

fn answer_text(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// A hop key of the message as the sink received it -- the hop is the
/// collector's, refined by the edge that delivered it.
fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One message in, one message out of the sink.
async fn round_trip(
    h: &meclaw_testing::ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    text: &str,
) -> Message {
    round_trip_in(h, rx, "s1", text).await
}

async fn round_trip_in(
    h: &meclaw_testing::ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    session: &str,
    text: &str,
) -> Message {
    h.send(turn_in(session, text)).await;
    recv_bounded(rx)
        .await
        .unwrap_or_else(|| panic!("nothing came back for {text:?}"))
}

/// One turn in, one answer out. The receipt synchronises the conversation, so
/// the next turn is sent only once the previous one is in the window.
async fn say(
    h: &meclaw_testing::ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    text: &str,
) -> String {
    answer_text(&round_trip(h, rx, text).await)
}

async fn say_in(
    h: &meclaw_testing::ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    session: &str,
    text: &str,
) -> String {
    answer_text(&round_trip_in(h, rx, session, text).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_can_reference_its_own_first_turn() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "", false);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let a1 = say(&h, &mut sink_rx, "my editor is helix").await;
    assert!(
        a1.starts_with("seen=1|"),
        "the first turn stands alone: {a1}"
    );

    let a2 = say(&h, &mut sink_rx, "and my shell is fish").await;
    assert!(
        a2.starts_with("seen=3|first=my editor is helix|"),
        "turn 2 sees turn 1 AND the answer to it: {a2}"
    );

    // The question the issue is about. There is no memory hive in this colony,
    // so a correct answer can only have come from the window.
    let a3 = say(&h, &mut sink_rx, "what did i say first?").await;
    assert!(
        a3.contains("first=my editor is helix"),
        "turn 3 must be able to name turn 1 without retrieval: {a3}"
    );
    assert!(
        a3.starts_with("seen=5|"),
        "three user turns and the two answers to them: {a3}"
    );
    assert!(a3.ends_with("|last=what did i say first?"), "{a3}");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_window_evicts_the_oldest_turns_when_the_turn_cap_is_reached() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "COLLECTOR_WINDOW_TURNS=3\n", false);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    say(&h, &mut sink_rx, "alpha").await;
    say(&h, &mut sink_rx, "beta").await;
    say(&h, &mut sink_rx, "gamma").await;
    let a4 = say(&h, &mut sink_rx, "delta").await;

    // Seven rows exist by now (four user turns, three answers). The window is
    // three, and it is the NEWEST three: the oldest turns left, whole.
    assert!(a4.starts_with("seen=3|"), "the cap holds: {a4}");
    assert!(
        a4.contains("first=gamma"),
        "the oldest survivor is a whole turn, not a fragment: {a4}"
    );
    assert!(
        !a4.contains("alpha"),
        "the evicted turn is gone, not truncated: {a4}"
    );
    assert!(
        a4.ends_with("|last=delta"),
        "the turn being answered never leaves: {a4}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_window_evicts_the_oldest_turns_when_the_byte_cap_is_reached() {
    let td = tempfile::TempDir::new().unwrap();
    // Generous turn cap, tight byte cap: the two policies are independent and
    // this colony proves the second one alone.
    build_tree(
        &td,
        "COLLECTOR_WINDOW_TURNS=50\nCOLLECTOR_WINDOW_BYTES=40\n",
        false,
    );
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    say(&h, &mut sink_rx, "aaaaaaaaaaaaaaaaaaaa").await;
    let a2 = say(&h, &mut sink_rx, "bbbbbbbbbbbbbbbbbbbb").await;

    // Rows: the 20-byte user turn, an answer of its own length, the new 20-byte
    // turn. Forty bytes buy the newest turn and nothing that would exceed them.
    assert!(
        a2.starts_with("seen=1|"),
        "the byte cap cut the older turns whole: {a2}"
    );
    assert!(a2.contains("first=bbbbbbbbbbbbbbbbbbbb"), "{a2}");
    assert!(!a2.contains("aaaaaaaaaaaaaaaaaaaa"), "{a2}");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_result_larger_than_the_window_reaches_the_brain_capped() {
    let td = tempfile::TempDir::new().unwrap();
    build_tool_tree(&td, "COLLECTOR_TOOL_CHARS=50\n", MEASURE_BRAIN, BIG_TOOL);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    // The tool answers with 100 KB. Without the cap that is what the brain
    // gets, and every window knob above it is decoration.
    let a1 = say(&h, &mut sink_rx, "look it up").await;
    assert_eq!(
        a1, "tools=1|chars=50",
        "the seam handed on a bounded preview, not the whole environment: {a1}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runaway_tool_round_is_ended_by_the_seam_that_opened_it() {
    let td = tempfile::TempDir::new().unwrap();
    // This brain answers `tool_calls` forever. Nothing in this topology says
    // stop: no iteration condition on an edge, no dispatcher, no error lane.
    build_tool_tree(&td, "COLLECTOR_MAX_ITER=1\n", RUNAWAY_BRAIN, TOOL);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let got = round_trip(&h, &mut sink_rx, "spin forever").await;
    assert_eq!(
        hop_of(&got, "route"),
        "answer",
        "the turn left through the answer lane instead of asking again"
    );
    assert_eq!(hop_of(&got, "round_capped"), "1");
    let texts: Vec<String> = body_of(&got)["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        texts[0], "spin forever",
        "what the turn collected travels with it: {texts:?}"
    );

    // And it is over: a capped turn asks nothing more, so nothing follows.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), sink_rx.recv())
            .await
            .is_err(),
        "the loop stopped, it did not merely pause"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_leaves_the_collector_as_one_batch() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "", false);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    say(&h, &mut sink_rx, "alpha").await;
    say(&h, &mut sink_rx, "beta").await;
    say(&h, &mut sink_rx, "gamma").await;

    // The keeper's close request. The collector reads its own store back --
    // the window store IS the durable record of the session (R-OS-6).
    let batch = round_trip(&h, &mut sink_rx, "/close").await;
    assert_eq!(hop_of(&batch, "route"), "write");
    assert_eq!(hop_of(&batch, "session_id"), "s1");
    assert_eq!(
        hop_of(&batch, "turn_count"),
        "6",
        "three questions and the three answers to them"
    );
    let body = body_of(&batch);
    let texts: Vec<String> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        texts.len(),
        6,
        "the whole day, not a window of it: {texts:?}"
    );
    assert_eq!(texts[0], "alpha", "in the order it happened");
    assert_eq!(texts[4], "gamma");
    assert_eq!(body["messages"][1]["origin"], "assistant");
    // The rounds ride along raw: with no tools in this tree they are the
    // per-turn assembly legs, which is what an eviction report is.
    let rounds = body["rounds"].as_array().expect("rounds slot");
    assert_eq!(rounds.len(), 3, "one slate per turn");
    assert_eq!(hop_of(&batch, "round_count"), "3");
    assert!(
        rounds.iter().all(|r| r["role"] == "leg-window"),
        "the collector's own bookkeeping row is not part of the batch: {rounds:?}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_round_re_enters_the_brain_through_the_same_seam() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "", true);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let a1 = say(&h, &mut sink_rx, "look it up").await;
    // The re-entry is not a fresh prompt: it carries the conversation window in
    // front of the tool round, which is the whole point of doing the fan-in
    // HERE rather than in a cell that only knows the round.
    assert!(
        a1.contains("first=look it up"),
        "the window leads the re-entry: {a1}"
    );
    assert!(
        a1.contains("|tools=2|"),
        "both parallel results fanned in: {a1}"
    );
    assert!(
        a1.contains("result-alpha,result-beta"),
        "both tools answered, exactly once each: {a1}"
    );
    assert!(
        a1.starts_with("seen=5|"),
        "window turn + the assistant turn that asked (2 calls) + 2 results: {a1}"
    );

    // And the round did not leak into the rolling window: the next turn sees
    // the conversation, not the tool traffic.
    let a2 = say(&h, &mut sink_rx, "thanks").await;
    assert!(
        a2.contains("|tools=0|") || a2.contains("tools=2|"),
        "second turn ran its own round: {a2}"
    );
    assert!(
        !a2.contains("result-alpha,result-beta,result-alpha"),
        "{a2}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_with_a_lost_result_is_closed_by_a_sweep_after_the_idle_window() {
    // GH #103, block 1. The brain asks for two tools; the tool answers one
    // call and swallows the other -- a result lost in flight. The round MUST
    // park first (that is the pre-#103 pin), and a sweep after the idle
    // window must close it: synthetic stand-in, regular fire, round_stale=1.
    let td = tempfile::TempDir::new().unwrap();
    build_tool_tree(
        &td,
        "COLLECTOR_ROUND_IDLE_MS=300\n",
        ROBUST_BRAIN,
        DROPPY_TOOL,
    );
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    // The round parks: one call is open, nothing reaches the sink.
    h.send(turn_in("s1", "look it up")).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), sink_rx.recv())
            .await
            .is_err(),
        "an incomplete round parks -- the deterministic exit needs an occasion"
    );

    // The occasion: a parent timer's sweep, well past the 300 ms window.
    let got = round_trip(&h, &mut sink_rx, "/sweep").await;
    assert_eq!(hop_of(&got, "route"), "answer");
    let ans = answer_text(&got);
    assert!(
        ans.contains("users=look it up|"),
        "the round fired with the context it collected: {ans}"
    );
    assert!(
        ans.contains("|tools=2|"),
        "the stand-in completed the fan-in: {ans}"
    );
    assert!(
        ans.contains("result-alpha"),
        "the real result still travels: {ans}"
    );
    assert!(
        ans.contains("tool result lost"),
        "the lost call was answered synthetically, id kept: {ans}"
    );
    assert!(
        ans.contains("|stale=1|"),
        "the seam reported the stale close on its hop: {ans}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mid_round_turn_defers_and_rides_with_the_next_assembly() {
    // GH #103, block 2 -- and block 1's other occasion in the same run: the
    // mid-round turn both defers itself AND closes the stale round it ran
    // into. At most ONE open brain call per session (telephone model R-OS-3).
    let td = tempfile::TempDir::new().unwrap();
    build_tool_tree(
        &td,
        "COLLECTOR_ROUND_IDLE_MS=300\n",
        ROBUST_BRAIN,
        DROPPY_TOOL,
    );
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn_in("s1", "look it up")).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), sink_rx.recv())
            .await
            .is_err(),
        "the round parks until an occasion arrives"
    );

    // The mid-round turn IS the occasion. It closes the stale round -- and
    // the answer that comes back belongs to the ROUND's turn, with the
    // round's own window: the deferred turn did not leak into it.
    let got = round_trip(&h, &mut sink_rx, "second question").await;
    let ans = answer_text(&got);
    assert!(
        ans.contains("users=look it up|"),
        "the running round answers from ITS window, not the new turn's: {ans}"
    );
    assert!(ans.contains("|stale=1|"), "{ans}");

    // And no second assembly follows: the deferred turn produced no second
    // brain call and therefore no second answer.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), sink_rx.recv())
            .await
            .is_err(),
        "a deferred turn asks nothing while it waits"
    );

    // The next regular turn carries the deferred one in its window and says
    // so on the seam.
    let a3 = say(&h, &mut sink_rx, "third question").await;
    assert!(
        a3.contains("users=look it up;second question;third question|"),
        "the deferred turn rides with the next assembly, in order: {a3}"
    );
    assert!(
        a3.contains("|deferred=1"),
        "the arrival is marked round_deferred=1: {a3}"
    );

    // The stamp cleared with that arrival: the next assembly is ordinary.
    let a4 = say(&h, &mut sink_rx, "fourth").await;
    assert!(
        a4.contains("|deferred=0"),
        "round_deferred marks the arrival, not every later window: {a4}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batched_session_is_pruned_and_the_living_session_is_not() {
    // GH #76 end to end. Two sessions share the store; one is closed and
    // pruned, the other must stay byte for byte -- its answers are the proof,
    // because this tree has no memory hive to hide a loss behind.
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "COLLECTOR_PRUNE_AFTER_MS=0\n", false);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    say_in(&h, &mut sink_rx, "s1", "alpha").await;
    say_in(&h, &mut sink_rx, "s1", "beta").await;
    say_in(&h, &mut sink_rx, "s2", "gamma").await;

    // A prune BEFORE any close finds no evidence and cuts nothing -- and says
    // so, instead of dying in silence.
    let r0 = round_trip_in(&h, &mut sink_rx, "s1", "/prune").await;
    assert_eq!(hop_of(&r0, "route"), "prune");
    assert_eq!(hop_of(&r0, "pruned_turns"), "0", "no ledger, no cut");
    assert_eq!(hop_of(&r0, "pruned_rounds"), "0");

    // Nothing fell: s1 still remembers its first turn.
    let a = say_in(&h, &mut sink_rx, "s1", "still there?").await;
    assert!(a.contains("first=alpha"), "no evidence, no loss: {a}");

    // The keeper closes s1: the batch leaves, the ledger row is the evidence.
    let batch = round_trip_in(&h, &mut sink_rx, "s1", "/close").await;
    assert_eq!(hop_of(&batch, "route"), "write");
    assert_eq!(hop_of(&batch, "turn_count"), "6");

    // Now the prune has evidence AND, with a zero gate, age.
    let r1 = round_trip_in(&h, &mut sink_rx, "s1", "/prune").await;
    assert_eq!(hop_of(&r1, "route"), "prune");
    assert_eq!(hop_of(&r1, "session_id"), "s1");
    assert_eq!(
        hop_of(&r1, "pruned_turns"),
        "6",
        "three questions and their answers -- exactly what the batch carried"
    );
    assert_eq!(
        hop_of(&r1, "pruned_rounds"),
        "4",
        "three assembly legs and the backdated parked day"
    );

    // The pruned session starts empty: its durable record left with the batch.
    let a1 = say_in(&h, &mut sink_rx, "s1", "anyone home?").await;
    assert!(
        a1.starts_with("seen=1|"),
        "the batched history left the window store: {a1}"
    );

    // The living session lost NOTHING -- it was never named by the ledger.
    let a2 = say_in(&h, &mut sink_rx, "s2", "what did i say first?").await;
    assert!(
        a2.starts_with("seen=3|first=gamma|"),
        "a session without a close batch is never touched: {a2}"
    );

    // And the evidence does not fire twice: the mark makes prune idempotent.
    let r2 = round_trip_in(&h, &mut sink_rx, "s1", "/prune").await;
    assert_eq!(
        hop_of(&r2, "pruned_turns"),
        "0",
        "used evidence is marked, not re-spent"
    );

    h.shutdown().await;
}
