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
//!
//! **GH #889.** The question above is no longer the collector's to answer. It
//! hands on the running round only, whole, on `curate`; the conversation window,
//! the close batch and the pruning belong to the curator (R-27-1). The trees
//! below wire `curate` straight into the reporting brain, so what they pin is the
//! round itself: its tool fan-in, its iteration cap, its idle exit, its deferred
//! turns, the memory tool and the hive boundary.

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::time::Duration;
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
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/collector")
}

/// Retunes the collector knobs of an ALREADY COPIED instance, in that
/// instance's own `assemble/config.json`.
///
/// That is one of the two per-instance mechanisms since `collector@1.2.0`: the
/// knobs are params, and a tree writer that already owns the tree sets them in
/// the tree. The other is `add_nodes[].override_params`, which addresses a
/// subtree template's sub-cells by path since GH #140 (`{"assemble": {…}}`);
/// this helper retunes an ALREADY COPIED instance, where birth is long past.
/// The key assertion is deliberate -- a knob name that does not exist used to
/// be a silently ignored `.env` line and is now a failing test.
fn tune(root: &std::path::Path, knobs: &[(&str, &str)]) {
    if knobs.is_empty() {
        return;
    }
    let p = root.join("main/collector/assemble/config.json");
    let raw = std::fs::read_to_string(&p).unwrap();
    let mut v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    let params = v["params"].as_object_mut().expect("params object");
    for (k, val) in knobs {
        assert!(params.contains_key(*k), "no such collector param: {k}");
        params.insert((*k).to_string(), json!(val));
    }
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
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

/// Turns a harness message into an inbound turn -- or, on the magic text, into
/// the round sweep a timer would send. The lane name is set by the PORT EDGE,
/// which is what makes this a port test and not a script test. (GH #889: the
/// close and prune requests left with the collector's `in_close`/`in_prune`.)
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
last = str(msgs[-1].get("text", "")) if msgs else ""
route = {"/sweep": "sweep"}.get(last, "turn")
sys.stdout.write(json.dumps({"header": {"route": route}, "messages": msgs}))
"#;

/// A brain that answers by REPORTING its context: how many turns it was given,
/// what the oldest user turn in it said, what the newest said. A model would
/// have to be believed; this cell can be measured.
const BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
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
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
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
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
h = envelope.get("header") or {}
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
d = json.load(sys.stdin)["body"]
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

const DISPATCH: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
calls = [m for m in d.get("messages", []) if m.get("type") == "tool_call"]
out = [{"header": {"route": "asst"}, "messages": calls}]
for c in calls:
    out.append({"header": {"route": "tool"}, "messages": [c]})
sys.stdout.write(json.dumps(out))
"#;

const TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "result-" + str(c.get("text", ""))}]}))
"#;

/// A dispatcher that hands the WHOLE call bundle to one tool cell in one
/// message, instead of splitting it into one message per call. Nothing in the
/// substrate forbids that shape -- a batch tool is a tool.
const BATCH_DISPATCH: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
calls = [m for m in d.get("messages", []) if m.get("type") == "tool_call"]
sys.stdout.write(json.dumps([{"header": {"route": "asst"}, "messages": calls},
                             {"header": {"route": "tool"}, "messages": calls}]))
"#;

/// The tool at the other end of that bundle: every call answered, in ONE
/// message, each turn under the id of the call it answers (GH #252).
const BATCH_TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
res = [{"origin": "tool", "type": "tool_result", "id": c.get("id", ""),
        "text": "result-" + str(c.get("text", ""))}
       for c in d.get("messages", []) if c.get("type") == "tool_call"]
sys.stdout.write(json.dumps({"header": {"route": "res"}, "messages": res}))
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
        json!({"from": "./probe", "to": "./collector",
               "condition": "hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"}}}),
        // GH #889: the close port with its batch and the prune port with its
        // report left the collector (R-27-1) -- the curator batches a closed
        // session, and there is no window left to prune.
        //
        // The round sweep port (GH #103): the timer stand-in asks whether any
        // tool round is stuck behind the idle window.
        json!({"from": "./probe", "to": "./collector",
               "condition": "hop.route == 'sweep'",
               "modifier": {"set_hop": {"route": "'in_round_sweep'"}}}),
        // GH #889: the seam is `curate` now; with no curator in this tree it
        // goes straight to the reporting brain, which then sees the round alone.
        json!({"from": "./collector", "to": "./brain",
               "condition": "hop.route == 'curate'",
               "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                            "session_id": "hop.session_id",
                                            "iter": "hop.iter"}}}),
        json!({"from": "./brain", "to": "./collector",
               "condition": "hop.finish_reason == 'stop'",
               "modifier": {"set_hop": {"route": "'in_answer'"}}}),
        json!({"from": "./collector", "to": "/sink",
               "condition": "hop.route == 'answer'"}),
    ];
    if with_tools {
        edges.push(json!({"from": "./brain", "to": "./dispatch",
                          "condition": "hop.finish_reason == 'tool_calls'"}));
        edges.push(json!({"from": "./dispatch", "to": "./collector",
                          "condition": "hop.route == 'asst'",
                          "modifier": {"set_hop": {"route": "'in_calls'"}}}));
        edges.push(json!({"from": "./dispatch", "to": "./tool",
                          "condition": "hop.route == 'tool'"}));
        edges.push(json!({"from": "./tool", "to": "./collector",
                          "condition": "hop.route == 'res'",
                          "modifier": {"set_hop": {"route": "'in_tool'"}}}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, knobs: &[(&str, &str)], with_tools: bool) {
    if with_tools {
        build_tool_tree(td, knobs, TOOL_BRAIN, TOOL);
        return;
    }
    build_base(td, knobs, false);
    write(
        td.path(),
        "main/brain/config.json",
        &code_cell(BRAIN, &[], finish_hop()),
    );
}

/// The same tree with a tool lane, but with the brain and the tool named by
/// the case: what a cap does is only visible against a specific pair.
fn build_tool_tree(td: &tempfile::TempDir, knobs: &[(&str, &str)], brain: &str, tool: &str) {
    build_base(td, knobs, true);
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

/// The same tree again, but with a dispatcher that batches and a tool that
/// answers the whole batch in one message -- the shape GH #252 is about.
fn build_batch_tree(td: &tempfile::TempDir, knobs: &[(&str, &str)]) {
    build_base(td, knobs, true);
    let root = td.path();
    write(
        root,
        "main/brain/config.json",
        &code_cell(TOOL_BRAIN, &[], finish_hop()),
    );
    write(
        root,
        "main/dispatch/config.json",
        &code_cell(BATCH_DISPATCH, &["asst", "tool"], json!({})),
    );
    write(
        root,
        "main/tool/config.json",
        &code_cell(BATCH_TOOL, &["res"], json!({})),
    );
}

fn finish_hop() -> Value {
    json!({"finish_reason": {"type": "string",
                             "values": ["stop", "tool_calls"], "required": true}})
}

fn build_base(td: &tempfile::TempDir, knobs: &[(&str, &str)], with_tools: bool) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config(with_tools));
    copy_cells(&template_dir(), &root.join("main/collector"));
    tune(root, knobs);
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["turn", "sweep"], json!({})),
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

/// The idle window the GH #103 colony cases configure. It is the one SEMANTIC
/// discriminator of that block -- a round is closed by an occasion only when
/// its last progress lies BEHIND this window -- and every wait in those cases
/// is derived from it, never written out a second time (`idle_knob` puts the
/// same number into the instance's own params, so the two cannot drift).
///
/// Why two seconds and not the 300 ms this block was written with (GH #114):
/// the window is not only the gate the OCCASION has to pass, it is also the
/// deadline the tree's OWN chain has to beat. The round-check that follows
/// every new round row measures the round against the same window, so when a
/// hop of this tree -- a python subprocess spawn plus a store round trip --
/// takes longer than the window, the round declares ITSELF idle and closes
/// without any occasion at all. Measured under eight-way parallel load: hops
/// of 500-700 ms, and a round that closed itself 306 ms after its own tool
/// result (3/24 binary runs red at the "a deferred turn asks nothing" pin).
/// Two seconds sits above that hop latency with room to spare while staying a
/// window a wall clock can still tell apart. The BOUNDARY behaviour -- fresh
/// round waits, stale round closes -- is pinned deterministically and without
/// a clock at script level in `collector_window.rs` (`STALE`/`FRESH`), so
/// nothing semantic rides on the absolute number here.
const ROUND_IDLE: Duration = Duration::from_millis(2000);

/// The collector param that configures [`ROUND_IDLE`] in a tree under test.
fn idle_ms() -> String {
    ROUND_IDLE.as_millis().to_string()
}

fn idle_knob(ms: &str) -> [(&str, &str); 1] {
    [("round_idle_ms", ms)]
}

/// How long a case waits before it hands the round its occasion: the idle
/// window plus a slack of half a second. The clock starts at the OBSERVED
/// slate (see [`await_parked_round`]) and the newest row was written no later
/// than that, so the round's last progress is at least `ROUND_IDLE` old when
/// the occasion mints its own cut; the slack absorbs timestamp granularity.
/// Scheduler latency can only delay the occasion further, which makes the
/// round staler, never fresher -- the discriminator has no upper edge to lose.
const PAST_IDLE: Duration = Duration::from_millis(2500);

/// Waits for the POSITIVE receipt that a tool round is open and its fan-in is
/// stuck, and returns the slate it read for the failure messages.
///
/// The receipt is the collector's own state surface: the `round` table in the
/// store cell's `cell.db` carries an unfired `assistant` row (the round IS
/// open, the guard has not fired) next to at least one real `tool` result row
/// (one call answered, the other lost in flight). Nothing else in these trees
/// writes there, so the two rows together are the parked round -- an
/// observation, not an inference from elapsed time.
///
/// GH #114: the cases used to wait a wall-clock second instead and read
/// "nothing arrived at the sink" as "the round parked". Under parallel cargo
/// load that reading is ambiguous -- the three python cells of this tree can
/// need seconds to walk probe -> brain -> dispatch -> tool, and "not started
/// yet" produces exactly the same silence. Measured: 12/36 red at ~33 % under
/// twelve-way load. Waiting for the slate removes the ambiguity: what follows
/// is timed against an OBSERVED round, not against a hopeful sleep.
///
/// The 30 s bound is a failure marker, not a discriminator, and follows the
/// 30 s convention of `recv_bounded`.
async fn await_parked_round(td: &tempfile::TempDir) -> String {
    let db = td.path().join("main/collector/window/cell.db");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut slate = "<no round table yet>".to_string();
    loop {
        if let Ok(conn) = rusqlite::Connection::open(&db)
            && let Ok(mut stmt) =
                conn.prepare("SELECT role, fired, recorded_at FROM round ORDER BY recorded_at")
        {
            let rows: Vec<(String, i64, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map(|it| it.flatten().collect())
                .unwrap_or_default();
            if !rows.is_empty() {
                slate = rows
                    .iter()
                    .map(|(role, fired, at)| format!("{role}/fired={fired}@{at}"))
                    .collect::<Vec<_>>()
                    .join(" ; ");
            }
            let open = rows
                .iter()
                .any(|(role, fired, _)| role == "assistant" && *fired == 0);
            let answered = rows.iter().any(|(role, _, _)| role == "tool");
            if open && answered {
                return slate;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no parked round in the collector's slate within 30 s -- slate: {slate}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// GH #889: the conversation window (a turn naming an earlier one, the turn and
// byte caps) and the preview cap on a tool result left the collector -- the
// window is the curator's, and R-27-1 hands the round on uncut.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runaway_tool_round_is_ended_by_the_seam_that_opened_it() {
    let td = tempfile::TempDir::new().unwrap();
    // This brain answers `tool_calls` forever. Nothing in this topology says
    // stop: no iteration condition on an edge, no dispatcher, no error lane.
    build_tool_tree(&td, &[("max_iter", "1")], RUNAWAY_BRAIN, TOOL);
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

// GH #889: the close batch (`in_close` -> `write`) is the curator's now; it
// builds the batch from its own ledger.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_round_re_enters_the_brain_through_the_same_seam() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &[], true);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let a1 = say(&h, &mut sink_rx, "look it up").await;
    // The re-entry is not a fresh prompt: it carries the round's opening turn in
    // front of the tool round. GH #889: earlier turns are the curator's to add,
    // so the opening turn is all of the conversation this seam carries.
    assert!(
        a1.contains("first=look it up"),
        "the opening turn leads the re-entry: {a1}"
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
        "the opening turn + the assistant turn that asked (2 calls) + 2 results: {a1}"
    );

    // And the round did not leak into the next one. GH #889: a round's rows
    // drop when its answer leaves, so the second turn's round carries its own
    // opening turn and its own two results -- nothing of the first round.
    let a2 = say(&h, &mut sink_rx, "thanks").await;
    assert!(
        a2.starts_with("seen=5|first=thanks|tools=2|"),
        "second turn ran its own round, alone: {a2}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_message_answering_two_calls_completes_the_round() {
    // GH #252, at the port. The tool answers BOTH calls in one message, which
    // the `in_tool` lane used to reduce to `messages[0]`: the second call
    // stayed open, the round parked, and the turn was only ever closed by the
    // idle exit with a synthetic stand-in for a result that had arrived. The
    // observable form of the defect is that no answer comes back at all inside
    // the failure marker, which is exactly what this case would hit.
    let td = tempfile::TempDir::new().unwrap();
    build_batch_tree(&td, &[]);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let a1 = say(&h, &mut sink_rx, "look it up").await;
    assert!(
        a1.contains("|tools=2|"),
        "one message answered both calls: {a1}"
    );
    assert!(
        a1.contains("result-alpha,result-beta"),
        "both results reached the brain, each under its own call id: {a1}"
    );
    assert!(
        a1.starts_with("seen=5|"),
        "the opening turn + the assistant turn that asked (2 calls) + 2 results: {a1}"
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
    build_tool_tree(&td, &idle_knob(&idle_ms()), ROBUST_BRAIN, DROPPY_TOOL);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    // The round parks: one call is open, nothing reaches the sink. The slate
    // is the receipt that it IS parked (GH #114) -- an unfired assistant row
    // beside the one real result.
    h.send(turn_in("s1", "look it up")).await;
    let slate = await_parked_round(&td).await;

    // One construct, two duties. It is the pre-#103 pin -- an incomplete round
    // fires for nobody, not even once its window has passed, and nothing in
    // this tree closes it on its own -- and it is the wait that puts the sweep
    // PAST that window (see PAST_IDLE for why the arithmetic holds under
    // load). That order is what makes the sweep the PROVEN occasion: were the
    // round to close itself, the answer would land inside this silence and
    // fail the pin instead of passing for the sweep's work.
    assert!(
        tokio::time::timeout(PAST_IDLE, sink_rx.recv())
            .await
            .is_err(),
        "an incomplete round parks -- the deterministic exit needs an occasion (slate: {slate})"
    );

    // The occasion: a parent timer's sweep, well past the idle window.
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
    build_tool_tree(&td, &idle_knob(&idle_ms()), ROBUST_BRAIN, DROPPY_TOOL);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn_in("s1", "look it up")).await;
    // Same discipline as the sweep case (GH #114): the parked round is read
    // off the slate, and the wait that follows both pins "no occasion, no
    // fire" and carries the round past its idle window.
    let slate = await_parked_round(&td).await;
    assert!(
        tokio::time::timeout(PAST_IDLE, sink_rx.recv())
            .await
            .is_err(),
        "the round parks until an occasion arrives (slate: {slate})"
    );

    // The mid-round turn IS the occasion. It closes the stale round -- and
    // the answer that comes back belongs to the ROUND's turn, with the
    // round's own turns: the deferred turn did not leak into it.
    let got = round_trip(&h, &mut sink_rx, "second question").await;
    let ans = answer_text(&got);
    assert!(
        ans.contains("users=look it up|"),
        "the running round answers from ITS turns, not the new turn's: {ans}"
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

    // The next regular turn carries the deferred one and says so on the seam.
    // GH #889: only the running round rides on `curate` -- the deferred turn
    // and the new one, in order; the earlier round's turn is the curator's.
    let a3 = say(&h, &mut sink_rx, "third question").await;
    assert!(
        a3.contains("users=second question;third question|"),
        "the deferred turn rides with the next assembly, in order: {a3}"
    );
    assert!(
        !a3.contains("look it up"),
        "an ended round's turn does not ride again: {a3}"
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

// GH #889: pruning a batched session (`in_prune`, GH #76) left with the window
// store it cut; the curator's ledger is append-only (R-27-2 point 1).

// ==================================================== THE MEMORY TOOL (GH #78)
//
// The per-turn recall leg is fired before the model has seen the turn, so no
// agent can ever DECIDE to ask memory about a time RANGE. The tool closes that
// half -- and the two trees below ask the two questions that decide whether it
// is really wiring: does the round come back complete when the port is there,
// and does it END when the port is not?
//
// The router in both trees is the SHIPPED `dispatcher@1` template, unchanged.
// If the dispatcher had to learn one word about memory, the claim of the issue
// would be false.

/// The shipped fan-out half, byte for byte as the template ships it.
fn dispatcher_config() -> Value {
    let raw = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/dispatcher/config.json"),
    )
    .expect("dispatcher template");
    meclaw_core::serde_json::from_str(&raw).expect("dispatcher json")
}

/// A brain that asks for two tools at once -- one ordinary tool and one
/// `memory_recall` with a TIME RANGE -- and then reports what came back.
const MEMORY_BRAIN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
h = envelope.get("header") or {}
ctx = h.get("context") or {}
hop = h.get("hop") or {}
it = int(ctx.get("iter", 0) or 0)
msgs = d.get("messages", [])
if it == 0:
    args = json.dumps({"query": "what did we decide?",
                       "window_from": "2026-08-01T00:00:00Z",
                       "window_to": "2026-08-02T00:00:00Z"})
    out = {"header": {"finish_reason": "tool_calls"},
           "messages": [
               {"origin": "assistant", "type": "tool_call", "id": "c1",
                "text": json.dumps({"name": "fake_tool", "arguments": "alpha"})},
               {"origin": "assistant", "type": "tool_call", "id": "m1",
                "text": json.dumps({"name": "memory_recall", "arguments": args})}]}
else:
    users = [str(m.get("text", "")) for m in msgs if m.get("origin") == "user"]
    res = [str(m.get("text", "")) for m in msgs if m.get("type") == "tool_result"]
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text",
                         "text": "first=%s|tools=%d|%s|stale=%s" % (
                             users[0] if users else "<none>", len(res),
                             " ;; ".join(sorted(res)), hop.get("round_stale", ""))}]}
sys.stdout.write(json.dumps(out))
"#;

/// A memory hive's TOOL adapter, reduced to the one thing this test asks of it:
/// it REPORTS the call it received, as a `tool_result` under the original id.
/// Nothing is retrieved, so nothing has to be believed -- what the result says
/// is what the model asked for.
///
/// Since GH #552 this is where a `memory_recall` call is served: the dispatcher
/// names the tool, an edge knows the cell, and the cell is in the member's
/// memory rather than in this collector. From the collector's side the answer is
/// an ordinary `in_tool` result, which is the whole point of the change.
const MEMO: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
msgs = d.get("messages") or []
call = msgs[0] if msgs else {}
args = json.loads(call.get("text") or "{}")
sys.stdout.write(json.dumps(
    {"header": {"route": "bundle"},
     "messages": [{"origin": "tool", "type": "tool_result",
                   "id": call.get("id", ""),
                   "text": "MEMORY[tier=1,from=%s,to=%s,q=%s]" % (
                       args.get("window_from", ""), args.get("window_to", ""),
                       args.get("query", ""))}]}))
"#;

/// The wiring a parent draws for an agent with a memory tool. Since GH #552 it
/// is exactly what an ORDINARY tool costs -- the dispatcher names the tool, an
/// edge knows the cell, and the result comes back on `in_tool`. The collector
/// has no second lane and no correlation key of its own any more.
fn memory_main_config(with_memo: bool) -> Value {
    let mut edges = vec![
        json!({"from": "./probe", "to": "./collector",
               "condition": "hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"}}}),
        json!({"from": "./probe", "to": "./collector",
               "condition": "hop.route == 'sweep'",
               "modifier": {"set_hop": {"route": "'in_round_sweep'"}}}),
        // GH #889: the seam is `curate`, straight into the brain here.
        json!({"from": "./collector", "to": "./brain",
               "condition": "hop.route == 'curate'",
               "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                            "session_id": "hop.session_id",
                                            "iter": "hop.iter"}}}),
        json!({"from": "./brain", "to": "./collector",
               "condition": "hop.finish_reason == 'stop'",
               "modifier": {"set_hop": {"route": "'in_answer'"}}}),
        json!({"from": "./collector", "to": "/sink",
               "condition": "hop.route == 'answer'"}),
        json!({"from": "./brain", "to": "./dispatcher",
               "condition": "hop.finish_reason == 'tool_calls'"}),
        json!({"from": "./dispatcher", "to": "./collector",
               "condition": "hop.route == 'calls'",
               "modifier": {"set_hop": {"route": "'in_calls'"}}}),
        // An ordinary tool: the dispatcher names it, this edge knows the cell.
        json!({"from": "./dispatcher", "to": "./tool",
               "condition": "hop.route == 'tool' && hop.tool_name == 'fake_tool'"}),
        json!({"from": "./tool", "to": "./collector",
               "condition": "hop.route == 'res'",
               "modifier": {"set_hop": {"route": "'in_tool'"}}}),
    ];
    if with_memo {
        // The memory tool, in the shape GH #552 gave it: the SAME two edges an
        // ordinary tool costs, pointing at a cell that is not this collector.
        edges.push(json!({"from": "./dispatcher", "to": "./memo",
                          "condition": "hop.route == 'tool' && hop.tool_name == 'memory_recall'"}));
        edges.push(json!({"from": "./memo", "to": "./collector",
                          "condition": "hop.route == 'bundle'",
                          "modifier": {"set_hop": {"route": "'in_tool'"}}}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_memory_tree(td: &tempfile::TempDir, knobs: &[(&str, &str)], with_memo: bool) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &memory_main_config(with_memo));
    copy_cells(&template_dir(), &root.join("main/collector"));
    tune(root, knobs);
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["turn", "sweep"], json!({})),
    );
    write(
        root,
        "main/brain/config.json",
        &code_cell(MEMORY_BRAIN, &[], finish_hop()),
    );
    write(root, "main/dispatcher/config.json", &dispatcher_config());
    write(
        root,
        "main/tool/config.json",
        &code_cell(TOOL, &["res"], json!({})),
    );
    if with_memo {
        write(
            root,
            "main/memo/config.json",
            &code_cell(MEMO, &["bundle"], json!({})),
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_recall_call_is_answered_elsewhere_and_completes_the_round() {
    let td = tempfile::TempDir::new().unwrap();
    build_memory_tree(&td, &[], true);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let ans = say(&h, &mut sink_rx, "what did we decide on the first?").await;
    assert!(
        ans.contains("|tools=2|"),
        "one ordinary tool and one memory call, both fanned back in: {ans}"
    );
    assert!(
        ans.contains("result-alpha"),
        "the ordinary tool travelled its ordinary path: {ans}"
    );
    // The window the MODEL asked for reached the recall port -- the first
    // producer of the recall window the memory hive has understood since P15.
    assert!(
        ans.contains(
            "MEMORY[tier=1,from=2026-08-01T00:00:00Z,to=2026-08-02T00:00:00Z,q=what did we decide?]"
        ),
        "the call's arguments reached the memory as the model named them: {ans}"
    );
    assert!(
        ans.contains("first=what did we decide on the first?"),
        "and the round re-entered through the seam, its opening turn first: {ans}"
    );
    assert!(
        ans.ends_with("|stale=0"),
        "a complete round is not a stale one: {ans}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_call_without_a_wired_port_ends_in_the_rounds_idle_exit() {
    // The documented failure path. Without the tool edge the call is
    // unroutable and nothing ever answers it -- but the round must not
    // park forever: the idle exit of GH #103 owns this case exactly as it owns
    // a tool that died mid-flight. No new machinery for a memory tool.
    let td = tempfile::TempDir::new().unwrap();
    build_memory_tree(&td, &idle_knob(&idle_ms()), false);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn_in("s1", "what did we decide on the first?"))
        .await;
    // The parked round off the slate, then the wait that is both the pin and
    // the passage of the idle window (GH #114).
    let slate = await_parked_round(&td).await;
    assert!(
        tokio::time::timeout(PAST_IDLE, sink_rx.recv())
            .await
            .is_err(),
        "the round parks: the memory call has no port to answer it (slate: {slate})"
    );

    let got = round_trip(&h, &mut sink_rx, "/sweep").await;
    let ans = answer_text(&got);
    assert!(ans.contains("|tools=2|"), "the fan-in completed: {ans}");
    assert!(
        ans.contains("result-alpha"),
        "the tool that DID answer still travels: {ans}"
    );
    assert!(
        ans.contains("tool result lost"),
        "the memory call was answered synthetically, under its own id: {ans}"
    );
    assert!(
        ans.ends_with("|stale=1"),
        "and the seam says the round was closed by the idle exit: {ans}"
    );

    h.shutdown().await;
}

// ── The hive boundary (meclaw-overview § Die Hive-Grenze) ────────────────────
//
// Everything above wires to `./collector` — the shape every caller in
// this repo grew up with, and the shape the boundary rule retires. What follows
// is the same conversation with every edge addressed to the HIVE, so a caller
// never names a cell inside it.

/// The same wiring as `main_config`, with one difference that is the whole
/// point: `./collector` instead of `./collector`, in both directions.
fn main_config_via_hive() -> Value {
    json!({
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": "./probe", "to": "./collector",
             "condition": "hop.route == 'turn'",
             "modifier": {"set_hop": {"route": "'in_turn'"}}},
            // GH #889: the seam is `curate`.
            {"from": "./collector", "to": "./brain",
             "condition": "hop.route == 'curate'",
             "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                          "session_id": "hop.session_id",
                                          "iter": "hop.iter"}}},
            {"from": "./brain", "to": "./collector",
             "condition": "hop.finish_reason == 'stop'",
             "modifier": {"set_hop": {"route": "'in_answer'"}}},
            {"from": "./collector", "to": "/sink",
             "condition": "hop.route == 'answer'"}
        ]}}
    })
}

/// **A caller talks to the hive, not into it.** The turn enters at the collector's
/// own path, the hive hands it to whatever is behind the boundary, and the answer
/// leaves the same way — so the caller's topology contains no name from inside the
/// template and survives any rearrangement of it.
///
/// Both directions are exercised on purpose: an inbound-only test would pass on a
/// hive whose exit still reaches out of an interior cell, which is the same breach
/// seen from the other end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_crosses_the_collector_at_its_hive_path() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config_via_hive());
    copy_cells(&template_dir(), &root.join("main/collector"));
    tune(root, &[]);
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["turn", "sweep"], json!({})),
    );
    write(
        root,
        "main/brain/config.json",
        &code_cell(BRAIN, &[], finish_hop()),
    );

    let (h, mut rx, _park_rx) = boot(&td).await;
    let answer = say(&h, &mut rx, "hello through the door").await;
    assert!(
        answer.contains("hello through the door"),
        "the turn has to come back through the hive, got {answer:?}"
    );

    // And the caller's own topology names nothing from inside the template.
    let wiring = meclaw_core::serde_json::to_string(&main_config_via_hive()).unwrap();
    assert!(
        !wiring.contains("collector/"),
        "a caller that names a cell inside the hive has written its layout down"
    );
}
