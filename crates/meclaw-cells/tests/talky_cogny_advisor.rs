//! meclaw-os -- the bilateral advisor connection talky <-> cogny (GH #28, #123).
//!
//! The advisor split (R-CG-1) gives an agent two brains: a fast **talky** that
//! owns the channel and a slower **cogny** that owns the thinking. R-CG-3 says
//! the connection between them is asynchronous by construction, and that three
//! template deltas are enough to build it. This file is the end-to-end proof of
//! exactly that claim, in a running colony:
//!
//! 1. ONE brain response carries an interim answer AND a consult call. The
//!    sentence reaches the channel and the turn ENDS -- no second inference is
//!    spent on "one moment".
//! 2. The round that asked closes without a consult fan-in. Nothing waits for
//!    the advisor, so `round_idle_ms` never races its thinking time:
//!    a sweep at a one-millisecond idle window finds no open round, and the
//!    next turn assembles instead of being deferred.
//! 3. The advisor -- a sibling hive built from the SAME `collector` and
//!    `dispatcher` templates plus a brain, without keeper, summarizer or proxy
//!    (R-CG-2) -- runs its own tool round and returns on the `in_advice` lane.
//!    A fresh talky round verbalises the result in the channel.
//! 4. Bilateral: the advisor may ask BACK on the same lane, the user answers in
//!    the channel, and the reply travels to the advisor under the SAME
//!    `consult_id` -- which the model can only do because the collector showed
//!    it the open ids in `system.consult`.
//!
//! Free of a real provider by construction: the talky brain is the mock OpenAI
//! wire (which is what proves the `llm` cell hands `content` + `tool_calls` to
//! the dispatcher together), the cogny brain and both tools are `code` cells.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped trees

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
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

/// GH #277: a directory whose `config.json` declares `cell.type: "ref"` is a
/// REFERENCE, not a cell -- the referenced template's tree belongs in its
/// place. `talky` names its four sub-units that way, so a tree copied straight
/// off the library follows the same hop the substrate's staging path follows.
fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        let name = reference.split('@').next().unwrap_or_default();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// A fixed schedule id -- `${uuid7:*}` is an INSTANTIATION-side substitution.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1128";
/// Never during a test run.
const NEVER: &str = "0 0 0 1 1 *";

// ────────────────────────────────────────────────────────── the test-only cells

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({});
    if !routes.is_empty() {
        hop["route"] = json!({"type": "string", "values": routes, "required": false});
    }
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
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
            "purpose": "Test stand-in around the talky/cogny advisor split.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The channel surface: a plain turn, or the operator's stuck-round sweep.
const SURFACE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
last = str(msgs[-1].get("text", "")) if msgs else ""
route = "sweep" if last == "/sweep" else "turn"
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-42"},
                             "messages": msgs}))
"#;

/// The agent core's brain. It is not the subject of this file -- the WIRING is
/// -- so it is a `code` cell that decides on the text it was handed:
///
/// * a tool result in the round  -> the advice is ready
/// * an errand with `answer`     -> the reply to its own question, answered
/// * an errand mentioning a trip -> it needs to ask BACK
/// * anything else               -> it runs its own tool round first
const COGNY_BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
users = [str(m.get("text", "")) for m in msgs if m.get("origin") == "user"]
res = [str(m.get("text", "")) for m in msgs if m.get("type") == "tool_result"]
ask = users[-1] if users else ""
try:
    args = json.loads(ask)
except Exception:
    args = {}
if not isinstance(args, dict):
    args = {}


def answer(text):
    sys.stdout.write(json.dumps({"header": {"finish_reason": "stop"},
                                 "messages": [{"origin": "assistant", "type": "text",
                                               "text": text}]}))
    sys.exit(0)


if res:
    answer("ADVICE|" + res[-1])
if args.get("answer"):
    answer("ADVICE|it will be sunny in " + str(args.get("answer")))
question = str(args.get("question", "") or "")
if "trip" in question:
    answer("ASK|which city are you travelling to?")
call = {"name": "lookup", "arguments": json.dumps({"q": question})}
sys.stdout.write(json.dumps({"header": {"finish_reason": "tool_calls"},
                             "messages": [{"origin": "assistant", "type": "tool_call",
                                           "id": "k1", "text": json.dumps(call)}]}))
"#;

/// The agent core's own tool -- an ordinary, synchronous one.
const LOOKUP: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
try:
    a = json.loads(c.get("text") or "{}")
except Exception:
    a = {}
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "21C and sunny (%s)" % str(a.get("q", ""))}]}))
"#;

/// An advisor that never answers -- the worst case the async class exists for.
const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

// ─────────────────────────────────────────────────────────────── the topology

/// The agent core (R-CG-2): a SIBLING hive of the talkies, built from the same
/// proven tool-loop core -- collector + dispatcher + a brain -- and from
/// nothing else. No keeper, no summarizer, no proxy: cogny has no channel, no
/// sessions and no night. Its "conversation" is the talky's errands.
fn cogny_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // its own seam, with its own iteration ceiling and its own restore_ttl
        {"from": "./collector", "to": "./brain",
         "condition": "has(hop.route) && hop.route == 'brain' && has(hop.iter) && int(hop.iter) < 6",
         "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                      "session_id": "hop.session_id",
                                      "iter": "hop.iter"},
                      "restore_ttl": true}},
        {"from": "./brain", "to": "./dispatcher",
         "condition": "has(hop.finish_reason) && (hop.finish_reason == 'stop' || hop.finish_reason == 'tool_calls')"},
        {"from": "./dispatcher", "to": "./collector",
         "condition": "has(hop.route) && hop.route == 'calls'",
         "modifier": {"set_hop": {"route": "'in_calls'"}}},
        {"from": "./dispatcher", "to": "./collector",
         "condition": "has(hop.route) && hop.route == 'result'",
         "modifier": {"set_hop": {"route": "'in_tool'"}}},
        {"from": "./dispatcher", "to": "./collector",
         "condition": "has(hop.route) && hop.route == 'answer'",
         "modifier": {"set_hop": {"route": "'in_answer'"}}},
        {"from": "./dispatcher", "to": "./lookup",
         "condition": "has(hop.tool_name) && hop.tool_name == 'lookup'"},
        {"from": "./lookup", "to": "./collector",
         "condition": "has(hop.route) && hop.route == 'res'",
         "modifier": {"set_hop": {"route": "'in_tool'"}}}
    ]}}})
}

/// The ports around both hives. Two of them are the advisor connection.
fn main_config(silent_advisor: bool) -> Value {
    let advisor = if silent_advisor {
        "./void"
    } else {
        "./cogny/collector"
    };
    let mut edges = vec![
        json!({"from": "./surface", "to": "./talky/session-keeper",
               "condition": "has(hop.route) && hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"},
                            "set_context": {"channel": "hop.chat_id"}}}),
        // the operator's stuck-round re-check, straight at the collector
        json!({"from": "./surface", "to": "./talky/collector",
               "condition": "has(hop.route) && hop.route == 'sweep'",
               "modifier": {"set_hop": {"route": "'in_round_sweep'"}}}),
        // the channel
        json!({"from": "./talky/collector", "to": "/sink",
               "condition": "has(hop.route) && hop.route == 'answer'"}),
        json!({"from": "./talky/errors", "to": "/park",
               "condition": "has(hop.route) && hop.route == 'error'"}),
        // ── the advisor connection, port 1: the errand leaves the talky ──
        // A consult is a tool call like any other from the dispatcher's side:
        // the NAME selects the cell. What is special travels with it --
        // `consult_id` becomes context so it survives the advisor's own chain,
        // and `col_phase` is cleared because this message comes out of ANOTHER
        // collector's chain and would otherwise arrive mid-assembly.
        json!({"from": "./talky/dispatcher", "to": advisor,
               "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
               "modifier": {"set_hop": {"route": "'in_turn'"},
                            "set_context": {"consult_id": "hop.consult_id",
                                            "col_phase": "''"},
                            "restore_ttl": true}}),
        // a second edge on the same condition: the probe that lets this test
        // read what the correlation actually was
        json!({"from": "./talky/dispatcher", "to": "/park",
               "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'"}),
    ];
    if !silent_advisor {
        // ── port 2: the answer (or the question back) comes home ──
        edges.push(json!({"from": "./cogny/collector",
                          "to": "./talky/collector",
                          "condition": "has(hop.route) && hop.route == 'answer'",
                          "modifier": {"set_hop": {"route": "'in_advice'"},
                                       "set_context": {"col_phase": "''"},
                                       "restore_ttl": true}}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str, silent_advisor: bool, idle_ms: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        // A consult is a HANDOFF, not merely async (GH #372): the advisor's
        // answer comes back as its own turn, so the round the call leaves
        // behind is over even when no sentence stood beside it. The handoff
        // list declares the async class too -- the dispatcher unions the two.
        "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=3600000\n\
         DISPATCHER_HANDOFF_TOOLS=consult_cogny\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config(silent_advisor));
    write(
        root,
        "main/surface/config.json",
        &code_cell(
            SURFACE,
            &["turn", "sweep"],
            json!({"chat_id": {"type": "string", "required": false}}),
        ),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));
    // The idle window is a collector PARAM since `collector@1.2.0`, so it is set
    // per collector rather than once in the colony's `.env` -- and this tree has
    // two of them (the talky's and, below, the core's).
    patch(root, "main/talky/collector/assemble/config.json", |v| {
        v["params"]["round_idle_ms"] = json!(idle_ms);
    });
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // GH #464 -- the second timer of a shipped composite, and the same two
    // patches for the same two reasons: `${uuid7:*}` is an INSTANTIATION
    // substitution and a tree written straight to disk carries a literal, and a
    // menu tick during a test run would ask a tools hive this colony does not
    // have.
    patch(root, "main/talky/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
    if silent_advisor {
        write(
            root,
            "main/void/config.json",
            &code_cell(VOID, &[], json!({})),
        );
        return;
    }
    // The agent core, wired out of the SAME two templates the talky carries.
    write(root, "main/cogny/config.json", &cogny_config());
    copy_cells(
        &templates_root().join("collector"),
        &root.join("main/cogny/collector"),
    );
    patch(root, "main/cogny/collector/assemble/config.json", |v| {
        v["params"]["round_idle_ms"] = json!(idle_ms);
    });
    patch(root, "main/cogny/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    std::fs::create_dir_all(root.join("main/cogny/dispatcher")).unwrap();
    std::fs::copy(
        templates_root().join("dispatcher/config.json"),
        root.join("main/cogny/dispatcher/config.json"),
    )
    .unwrap();
    write(
        root,
        "main/cogny/brain/config.json",
        &code_cell(
            COGNY_BRAIN,
            &[],
            json!({"finish_reason": {"type": "string",
                                     "values": ["stop", "tool_calls"], "required": true}}),
        ),
    );
    write(
        root,
        "main/cogny/lookup/config.json",
        &code_cell(LOOKUP, &["res"], json!({})),
    );
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
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
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
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
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn answer_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The whole wire conversation of one provider call, as one string.
fn wire_of(req: &mock_openai::OpenAiRequestSnapshot) -> String {
    meclaw_core::serde_json::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The end-to-end claim of R-CG-3, in one turn: ONE brain response carries the
/// interim answer AND the consult call, the sentence reaches the channel, the
/// turn ends -- and later, without anything having waited, the advisor's result
/// arrives on `in_advice` and a FRESH talky round says it out loud.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_answers_the_channel_at_once_and_the_advice_follows_later() {
    let mock = MockOpenAI::start(vec![
        canned_content_and_tool_calls(
            "one moment, i am asking",
            vec![(
                "call-1",
                "consult_cogny",
                r#"{"question":"weather in berlin","eta":"seconds"}"#,
            )],
        ),
        canned_chat_completion("the advisor says it is 21C and sunny.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url, false, "120000");
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn("what is the weather in berlin?")).await;

    // 1. The interim answer, in the same turn that asked.
    let interim = recv_bounded(&mut sink_rx)
        .await
        .expect("the interim answer");
    assert_eq!(answer_text(&interim), "one moment, i am asking");
    assert_eq!(
        hop_of(&interim, "interim"),
        "1",
        "and the channel is told that it IS an interim: {:?}",
        interim.headers.hop
    );

    // 2. The errand left with its correlation and its (unread) estimate.
    let errand = recv_bounded(&mut park_rx).await.expect("the consult call");
    assert_eq!(hop_of(&errand, "tool_name"), "consult_cogny");
    assert_eq!(hop_of(&errand, "async"), "1");
    assert_eq!(hop_of(&errand, "consult_id"), "call-1");
    assert_eq!(
        hop_of(&errand, "consult_eta"),
        "seconds",
        "GH #123: the model's own estimate is in the message flow -- and nothing reads it"
    );

    // 3. The follow-up, verbalised in the channel by a FRESH talky round.
    let follow_up = recv_bounded(&mut sink_rx).await.expect("the follow-up");
    assert_eq!(
        answer_text(&follow_up),
        "the advisor says it is 21C and sunny."
    );
    assert!(
        follow_up.headers.hop.get("interim").is_none(),
        "the follow-up is a real answer: {:?}",
        follow_up.headers.hop
    );

    // 4. Two talky inferences for the whole exchange, and the second one saw
    //    the advice as an inbound event -- not as its own words.
    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        2,
        "one to ask, one to say it: {} calls",
        reqs.len()
    );
    let second = wire_of(&reqs[1]);
    assert!(
        second.contains("ADVICE|21C and sunny (weather in berlin)"),
        "the advisor ran its OWN tool round and the result came home: {second}"
    );
    assert!(
        second.contains(
            r#"content":"[advice from your reasoning core, consult call-1]\nADVICE|21C and sunny (weather in berlin)","role":"user"#
        ),
        "an advisor event is INBOUND on the wire, not the agent's own words -- and since \
         GH #540 it says which of the two inbound voices it is: {second}"
    );
    assert!(
        second.contains("one moment, i am asking"),
        "and the interim answer is part of the conversation: {second}"
    );

    h.shutdown().await;
}

/// The race that R-CG-3 abolishes, measured: with a ONE MILLISECOND idle window
/// and an advisor that never answers, nothing is ever swept, nothing invents a
/// "tool result lost", and the next turn assembles instead of being deferred.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_idle_window_ever_waits_for_the_advisor() {
    let mock = MockOpenAI::start(vec![
        canned_content_and_tool_calls(
            "one moment, i am asking",
            vec![("call-1", "consult_cogny", r#"{"question":"anything"}"#)],
        ),
        canned_chat_completion("still here.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url, true, "1");
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn("what is the weather in berlin?")).await;
    let interim = recv_bounded(&mut sink_rx)
        .await
        .expect("the interim answer");
    assert_eq!(answer_text(&interim), "one moment, i am asking");
    recv_bounded(&mut park_rx).await.expect("the consult call");

    // The occasion a stuck round would be closed on -- twice, so a first sweep
    // that arrived too early cannot be the reason nothing happened.
    h.send(turn("/sweep")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.send(turn("/sweep")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // POSITIVE receipt: the next turn assembles. A still-open round would have
    // deferred it (one open brain call per session), and this answer would
    // never come.
    h.send(turn("are you still there?")).await;
    let second = recv_bounded(&mut sink_rx).await.expect("the second answer");
    assert_eq!(answer_text(&second), "still here.");

    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        2,
        "no third inference: nothing re-entered the brain on the advisor's behalf"
    );
    let wire = wire_of(&reqs[1]);
    assert!(
        !wire.contains("tool result lost"),
        "no synthetic stand-in was ever manufactured for the consult: {wire}"
    );
    assert!(
        !wire.contains("tool_call"),
        "the closed round is not re-opened by the next turn: {wire}"
    );
    assert!(
        wire.contains("are you still there?") && wire.contains("one moment, i am asking"),
        "the new turn assembled with the interim answer in its window: {wire}"
    );

    h.shutdown().await;
}

/// Bilateral: the advisor asks BACK on the same lane, the talky puts the
/// question into the channel, the user answers there, and the reply reaches the
/// advisor under the SAME `consult_id` -- which the model can only supply
/// because the collector showed it the open ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_advisor_can_ask_back_and_the_users_answer_finds_the_same_thread() {
    let mock = MockOpenAI::start(vec![
        canned_content_and_tool_calls(
            "one moment, i am asking",
            vec![(
                "call-1",
                "consult_cogny",
                r#"{"question":"plan a trip","eta":"minutes"}"#,
            )],
        ),
        canned_chat_completion(
            "the advisor asks: which city are you travelling to?",
            "stop",
        ),
        canned_content_and_tool_calls(
            "passing that on",
            vec![(
                "call-2",
                "consult_cogny",
                r#"{"consult_id":"call-1","answer":"berlin"}"#,
            )],
        ),
        canned_chat_completion("the advisor says it will be sunny in berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url, false, "120000");
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn("plan me a trip")).await;
    assert_eq!(
        answer_text(&recv_bounded(&mut sink_rx).await.expect("interim 1")),
        "one moment, i am asking"
    );
    let first_errand = recv_bounded(&mut park_rx).await.expect("errand 1");
    assert_eq!(hop_of(&first_errand, "consult_id"), "call-1");

    // The advisor's QUESTION arrives on the same return lane and is verbalised.
    let question = recv_bounded(&mut sink_rx).await.expect("the question back");
    assert_eq!(
        answer_text(&question),
        "the advisor asks: which city are you travelling to?"
    );

    // The user answers in the channel; the model passes the id it was shown.
    h.send(turn("berlin")).await;
    assert_eq!(
        answer_text(&recv_bounded(&mut sink_rx).await.expect("interim 2")),
        "passing that on"
    );
    let second_errand = recv_bounded(&mut park_rx).await.expect("errand 2");
    assert_eq!(
        hop_of(&second_errand, "consult_id"),
        "call-1",
        "the SAME consultation: {:?}",
        second_errand.headers.hop
    );
    assert_eq!(
        hop_of(&second_errand, "tool_call_id"),
        "call-2",
        "a different call -- correlation and call id are different keys"
    );

    let closing = recv_bounded(&mut sink_rx)
        .await
        .expect("the closing answer");
    assert_eq!(
        answer_text(&closing),
        "the advisor says it will be sunny in berlin."
    );

    // And the id was OFFERED, not guessed: the third inference saw it in the
    // system prompt, because the advice turn was still in the window.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 4, "two rounds of two inferences each");
    let third = reqs[2].messages().expect("wire messages");
    let system = third
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system message on the wire");
    let sys_text = system["content"].as_str().unwrap_or_default();
    assert!(
        sys_text.contains("open consults: call-1"),
        "the collector showed the model which thread it may answer: {sys_text}"
    );

    h.shutdown().await;
}
