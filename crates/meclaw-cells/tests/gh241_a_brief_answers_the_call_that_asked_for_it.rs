//! GH #241 and GH #242 — a brief has to reach the caller that asked for it.
//!
//! Two defects of `affinity@2.0.0`, one strand: the answer carried the wrong
//! id, and it carried the payload on a lane the caller cannot read. Both make
//! a brief that the audit table calls `ok` arrive as nothing.
//!
//! # Why this file boots a colony instead of driving the script
//!
//! `crates/meclaw-cells/tests/gh154_audience_is_a_set.rs` runs the shipped
//! `brief` script phase by phase and writes the echo context by hand — and it
//! writes `"aff_call": "c1"` into it. The script was always correct for that
//! input. What was missing is the wiring that would ever produce it: nothing in
//! the hive writes `aff_call`, and nothing could — `emit()` puts no `call_id`
//! on the hop, so no edge modifier has one to promote. A script-level pin
//! cannot tell "the script is wrong" from "no edge feeds the script", so this
//! file drives the SHIPPED template through a real colony, with the shipped
//! internal edges doing the promotion. Same guard form as
//! `affinity_template.rs` (GH #49): `affinity` is private and does not travel
//! with the export, so in a tree without it these tests skip.
//!
//! # The claims
//!
//! 1. **Every answer names the call it answers** (#241). Not only the two early
//!    refusals that never reach the store — the served brief and the
//!    post-store denial as well, which is every answer a caller actually
//!    waits on. An empty id closes no fan-in: the asking round keeps waiting
//!    for the call it opened and the turn ends in its idle window while the
//!    audit row says `ok`.
//! 2. **The disclosed pack survives a lane that carries `messages[]` alone**
//!    (#242). A sealed agent hive delivers a tool answer on its `in_tool`
//!    lane, and that lane keeps one message and drops `system`. So the proof
//!    throws the `system` slot away and asks whether the payload is still
//!    there — because that is precisely what the recipient behind the seal
//!    sees.
//! 3. **The denial did not become chattier for it.** A refused audience still
//!    gets the one sentence and no pack, on either lane. A fix that widens the
//!    answer must not widen the *denial*.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

/// Every file the hive is made of; a missing one makes this file skip rather
/// than fail (GH #49 — `affinity` is not in `PUBLIC_TEMPLATES`).
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "store/seed/entities.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/relations.jsonl",
];

fn shipped_affinity() -> Option<std::path::PathBuf> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/affinity");
    AFFINITY_FILES
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The shipped template, copied the way instantiation copies it: the
/// `config.json` files and the seed tables next to them.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

// ────────────────────────────────────────────────────────── the asking side

/// The caller. It opens ONE tool call with a known id and declares who is
/// asking on the hop — the port edge turns that into `context.asker`, which is
/// the only way `brief` ever learns an audience.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
req = {"subject": a.get("subject"), "channel": a.get("channel") or "*"}
if a.get("slots") is not None:
    req["slots"] = a.get("slots")
sys.stdout.write(json.dumps({
    "header": {"route": "brief", "audience": str(a.get("audience") or ""),
               # GH #306: the ROUND is edge truth too, and the door refuses a
               # request that declares none. This lane is a 1:1, so it says so.
               "participants": json.dumps([str(a.get("audience") or "")])},
    "messages": [{"origin": "assistant", "type": "tool_call",
                  "id": str(a.get("call_id") or "call-241"),
                  "text": json.dumps(req)}]}))
"#;

fn asker_config() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": ASKER, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["brief"], "required": false},
                    "audience": {"type": "string", "required": false},
                    "participants": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for whoever asks the affinity for a brief.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The lanes around the hive, all of them at the hive PATH — `params.ports` is
/// empty, so naming a cell inside it is refused.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./asker", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_hop": {"route": "'in_brief'"},
                      "set_context": {"asker": "hop.audience"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"}
    ]}}})
}

/// Far enough away that the push tick never fires during a test that is not
/// about ticks.
const QUIET_CRON: &str = "0 0 4 * * *";

async fn boot(
    root_template: &std::path::Path,
) -> (tempfile::TempDir, ColonyHandle, mpsc::Receiver<Message>) {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!("AFFINITY_PUSH_CRON={QUIET_CRON}\n"),
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(root, "main/asker/config.json", &asker_config());
    copy_cells(root_template, &root.join("main/affinity"));
    // `${uuid7:…}` is minted on the instantiation path; a raw filesystem
    // bootstrap has to be handed a literal.
    let clock = root.join("main/affinity/clock/config.json");
    let mut v: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&clock).unwrap()).unwrap();
    v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-00000000024a");
    std::fs::write(
        &clock,
        meclaw_core::serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (td, h, sink_rx)
}

fn ask(request: &str) -> Message {
    MessageBuilder::new(Path::new("/asker"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": request}]}),
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

/// The one turn an answer carries, as the caller's fan-in sees it: an id to
/// correlate on and a text to read.
fn tool_result(m: &Message) -> (String, String) {
    let turn = &body_of(m)["messages"][0];
    assert_eq!(
        turn["type"].as_str(),
        Some("tool_result"),
        "an answer to a tool call is a tool_result: {turn}"
    );
    (
        turn["id"].as_str().unwrap_or_default().to_string(),
        turn["text"].as_str().unwrap_or_default().to_string(),
    )
}

async fn recv_answer(rx: &mut mpsc::Receiver<Message>) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..12 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("no answer arrived; saw {seen:?}"));
        if hop_of(&m, "route") == "answer" {
            return m;
        }
        seen.push(hop_of(&m, "route"));
    }
    panic!("no answer among 12 messages; saw {seen:?}");
}

const DENIED: &str = "nothing is disclosed to this audience";

/// What a sealed agent hive's `in_tool` lane delivers: `messages[0]` and
/// nothing else. Modelling the drop explicitly is the point of claim 2 — a
/// test that reads `system` proves the wrong lane.
fn as_a_sealed_hive_delivers_it(m: &Message) -> Value {
    json!({"messages": [body_of(m)["messages"][0].clone()]})
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// GH #241. The served brief and the post-store denial both answer the call
/// they were asked with. Before the fix both left with `id: ""`, because the
/// echo branch read the id from `context.aff_call` — a key the hive's edges
/// never write and, since `emit()` puts no `call_id` on the hop, never could.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_answer_carries_the_id_of_the_call_it_answers() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let (_td, h, mut rx) = boot(&root).await;

    // 1. A brief that is actually served: four store reads happen before the
    //    answer is built, so the id has to survive all four.
    h.send(ask(
        r#"{"call_id":"call-a","audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let (id, text) = tool_result(&recv_answer(&mut rx).await);
    assert_ne!(text, DENIED, "the seeded audience IS disclosed to: {text}");
    assert_eq!(
        id, "call-a",
        "a served brief must answer the call it was asked with, not the empty id"
    );

    // 2. A denial that happens AFTER the store round trip — the disclosure
    //    read found nothing for this audience. This one is a denial, but it is
    //    still an answer somebody is waiting for.
    h.send(ask(
        r#"{"call_id":"call-b","audience":"agent:someone-else","subject":"entity:alex"}"#,
    ))
    .await;
    let (id, text) = tool_result(&recv_answer(&mut rx).await);
    assert_eq!(text, DENIED, "fail-closed: {text}");
    assert_eq!(
        id, "call-b",
        "a denial is an answer too, and it has to correlate"
    );

    // 3. The refusal that never reaches the store, for completeness: it was
    //    always right, and it stays right.
    h.send(ask(r#"{"call_id":"call-c","audience":"agent:aiden"}"#))
        .await;
    let (id, text) = tool_result(&recv_answer(&mut rx).await);
    assert_eq!(text, DENIED);
    assert_eq!(id, "call-c", "the early refusal never regressed");

    h.shutdown().await;
}

/// GH #242. The pack has to arrive where the caller reads. `system.*` stays —
/// it is right for the push lane, which addresses an `llm` cell — but it is
/// not the only place any more, because behind a sealed agent hive there is no
/// such recipient and the slot is dropped at the boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_disclosed_pack_reaches_a_caller_that_can_only_read_messages() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let (_td, h, mut rx) = boot(&root).await;

    h.send(ask(
        r#"{"call_id":"call-d","audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let answer = recv_answer(&mut rx).await;

    // The push lane is untouched: an `llm` cell still gets its four slots.
    let system = body_of(&answer)
        .get("system")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        system["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the system slot is still the push lane's answer: {system}"
    );

    // And now the lane that drops it. Everything below reads ONLY what a
    // sealed hive forwards.
    let delivered = as_a_sealed_hive_delivers_it(&answer);
    assert!(
        delivered.get("system").is_none(),
        "the model of this test is that `system` does not survive"
    );
    let text = delivered["messages"][0]["text"]
        .as_str()
        .expect("a tool_result carries text")
        .to_string();

    // The receipt line is still the first thing a reader sees.
    let (receipt, payload) = text
        .split_once('\n')
        .expect("the receipt line, then the pack — GH #242");
    assert!(
        receipt.starts_with("affinity brief on Alex Kern (person) for agent:aiden"),
        "the receipt line survived the fix: {receipt}"
    );

    // The pack below it is the SAME pack, parseable rather than prose.
    let pack: Value = meclaw_core::serde_json::from_str(payload)
        .unwrap_or_else(|e| panic!("the pack must be JSON a caller can parse ({e}): {payload}"));
    assert_eq!(
        pack, system,
        "the tool lane and the push lane carry the same disclosure decision"
    );
    assert_eq!(
        pack["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the disclosed material reached a caller that reads messages[] only: {pack}"
    );

    // Disclosure is unchanged by the new lane: what no row named is still
    // absent, in the text as well as in the slot.
    for undisclosed in ["INTP", "neutral good", "Example City", "1980-04-12"] {
        assert!(
            !text.contains(undisclosed),
            "an undisclosed value rode along on the tool lane ({undisclosed}): {text}"
        );
    }

    h.shutdown().await;
}

/// The fix widens the ANSWER, never the denial. A refused audience gets one
/// sentence, no newline, no pack and no slot — on both lanes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_audience_still_gets_one_sentence_and_nothing_else() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let (_td, h, mut rx) = boot(&root).await;

    h.send(ask(
        r#"{"call_id":"call-e","audience":"agent:someone-else","subject":"entity:alex"}"#,
    ))
    .await;
    let denied = recv_answer(&mut rx).await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "no system slot at all: {:?}",
        body_of(&denied)
    );
    let text = as_a_sealed_hive_delivers_it(&denied)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        text, DENIED,
        "the denial carries the sentence and nothing appended to it"
    );

    h.shutdown().await;
}
