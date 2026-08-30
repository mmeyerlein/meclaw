//! GH #481 — the broker answers the question it was asked, not the one before.
//!
//! `submit/gate` asks the broker up to three check-only questions per manifest,
//! sequentially, on ONE message chain: `colony.mutate` always, `code.author`
//! when the diff carries executable behaviour, `affinity.subscribe` when it
//! draws an identity door. Measured on a running colony, the SECOND question
//! was always refused — `capability_unknown`, stamped with the capability and
//! the `call_id` of the FIRST question, and the `policy` table was never read
//! for it at all.
//!
//! The cause is one context marker that outlives its round trip. `./policy`
//! recognises its own store echo by `context.access_origin`, promoted by its
//! own `./policy -> ./store` edge together with `ac_phase` and `ac_carry`.
//! `context` is persistent for the life of the chain and nothing removed those
//! three keys again, so they survived the store round trip, the `grant`
//! emission, the trip out of the hive and the caller's next move — and the
//! second request arrived already looking like the first one's echo. `policy`
//! read `phase == "rules"` off the stale marker, found no rows in a body that
//! carries a `tool_call`, and answered "no enabled rule mentions this
//! capability" to the question in the stale carry.
//!
//! What is pinned here, over the hive's REAL edges rather than by driving the
//! script directly — every existing `access` test drives the script or hands
//! the gate a finished verdict, and neither can see an edge:
//!
//! 1. **Two questions in a row get two answers.** The second answer carries the
//!    second capability and the second `call_id`.
//! 2. **The hive's bookkeeping does not leave the hive.** The message on the
//!    `grant` lane carries none of `access_origin`, `access_lane`, `ac_phase`,
//!    `ac_carry` — an interior state key crossing a sealed boundary is leakage
//!    whether or not anybody reads it.
//! 3. **`in_invoke` is a request too.** The same defect class one cell over:
//!    `./invoke` read `hop.operation` as the mark of its own echo, and a caller
//!    that carries one for reasons of its own was answered with silence.
//! 4. **Every exit edge clears the markers** — the structural half of 2, for the
//!    two lanes a test cannot cheaply drive (`./invoke -> .` after a vault round
//!    trip, `./sweep -> .`).
//!
//! This test grants NOTHING that is refused today. It only makes the question
//! get asked; `code.author.default` still ships `enabled: 0`, and the enabled
//! row below is the operator gesture a real colony has to make.
//!
//! **R2b guard (GH #49 form).** `access` is PRIVATE — it does not travel with
//! the export, so in the public clone these tests skip.

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

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

const ACCESS_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "policy/config.json",
    "invoke/config.json",
    "sweep/config.json",
    "clock/config.json",
    "vault/config.json",
];

const ACCESS_SEEDS: &[&str] = &["store/seed/policy.jsonl", "store/seed/cred_refs.jsonl"];

/// The template root, or `None` where it does not ship (GH #49 R2b form).
fn shipped_access() -> Option<std::path::PathBuf> {
    let root = templates_root().join("access");
    for rel in ACCESS_FILES.iter().chain(ACCESS_SEEDS) {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

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

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ────────────────────────────────────────────────────────── the test-only cells

/// The asking side: the shape `submit/gate` has, reduced to what this defect
/// needs. It asks ONCE, and when the answer comes back it asks a SECOND time on
/// the same chain — which is the whole of the reproduction. Which round it is
/// in is read off `context.round`, promoted by its own outbound edge from a hop
/// key it wrote itself; the broker's answer is not consulted for that, because
/// the broker's answer is the thing under test.
const ASKER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
header = (doc["envelope"].get("header") or {})
hop = header.get("hop") or {}
context = header.get("context") or {}


def ask(round_no, capability, call_id):
    return {"header": {"route": "ask", "round": round_no},
            "messages": [{"origin": "assistant", "type": "tool_call",
                          "id": call_id,
                          "text": json.dumps({"capability": capability,
                                              "check_only": True,
                                              "subject": "*",
                                              "resource": {"scope": "/os/orgs/acme",
                                                           "actions": ["apply"]}})}]}


if str(hop.get("route") or "") != "grant":
    sys.stdout.write(json.dumps([ask("1", "colony.mutate", "call-one")]))
    sys.exit(0)

msgs = d.get("messages") or []
last = msgs[-1] if msgs else {}
round_no = str(context.get("round") or "")
out = [{"header": {"route": "seen", "round": round_no,
                   "answer_id": str(last.get("id") or ""),
                   "verdict": str(hop.get("verdict") or "")},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": str(last.get("id") or ""),
                      "text": str(last.get("text") or "")}]}]
if round_no == "1":
    out.append(ask("2", "code.author", "call-two"))
sys.stdout.write(json.dumps(out))
"#;

/// The spending side. It carries `hop.operation` — a word of its OWN, about its
/// own business — which is exactly what the broker may not read as the mark of
/// its own store echo.
const SPENDER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages") or []
raw = str(msgs[-1].get("text") or "{}") if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "spend", "operation": "spend-a-grant"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-spend",
                  "text": raw}]}))
"#;

/// The operator's read/write channel straight into `./store`, as in
/// `access_template.rs`: legal from a BOOT graph, which is the sovereign birth
/// draft, and the only way a test can turn a rule on.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages") or []
raw = str(msgs[-1].get("text") or "{}") if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "pstore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "p1",
                  "text": raw}]}))
"#;

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
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped access template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ─────────────────────────────────────────────────────────────── the topology

/// The lanes around the hive. `./access -> ./asker` and `./access -> /sink`
/// both carry the `grant` lane: the asker needs the answer to ask again, and
/// the sink needs the message ITSELF, because assertion 2 is about the context
/// a message carries out of the hive.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./asker", "to": "./access",
         "condition": "has(hop.route) && hop.route == 'ask'",
         "modifier": {"set_hop": {"route": "'in_request'"},
                      "set_context": {"requester": "'/os/submit'",
                                      "round": "hop.round"}}},
        {"from": "./access", "to": "./asker",
         "condition": "has(hop.route) && hop.route == 'grant'"},
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'grant'"},
        {"from": "./asker", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'seen'"},
        {"from": "./spender", "to": "./access",
         "condition": "has(hop.route) && hop.route == 'spend'",
         "modifier": {"set_hop": {"route": "'in_invoke'"},
                      "set_context": {"requester": "'/os/submit'"}}},
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./probe", "to": "./access/store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"access_origin": "'probe'"}}},
        {"from": "./access/store", "to": "/sink",
         "condition": "context.access_origin == 'probe'"}
    ]}}})
}

/// Far enough away that no sweep happens during a test that is not about it.
const QUIET_CRON: &str = "0 0 4 * * *";

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!("ACCESS_SWEEP_CRON={QUIET_CRON}\n"),
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["ask", "seen"],
            json!({"round": {"type": "string", "required": false},
                   "answer_id": {"type": "string", "required": false},
                   "verdict": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/spender/config.json",
        &code_cell(
            SPENDER,
            &["spend"],
            json!({"operation": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["pstore"], json!({})),
    );
    copy_cells(root_template, &root.join("main/access"));
    patch(root, "main/access/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000000ac");
    });
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            (
                "vault".to_string(),
                Arc::new(meclaw_cells::vault::VaultCellFactory),
            ),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
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
    (h, sink_rx)
}

fn to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn send_json(cell: &str, v: &Value) -> Message {
    to(cell, &meclaw_core::serde_json::to_string(v).unwrap())
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

fn turn_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn turn_json(m: &Message) -> Value {
    meclaw_core::serde_json::from_str(&turn_text(m)).unwrap_or(Value::Null)
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The next message on `route`, skipping whatever else the sink collects.
async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("nothing more arrived while waiting for route {route}; saw {seen:?}")
        });
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(format!("{}: {}", hop_of(&m, "route"), turn_text(&m)));
    }
    panic!("route {route} never arrived; saw {seen:?}");
}

/// The `seen` message of one round, skipping the rounds before it.
async fn recv_round(rx: &mut mpsc::Receiver<Message>, round: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("round {round} never arrived; saw {seen:?}");
        });
        if hop_of(&m, "route") == "seen" && hop_of(&m, "round") == round {
            return m;
        }
        seen.push(format!(
            "{}/{}: {}",
            hop_of(&m, "route"),
            hop_of(&m, "round"),
            turn_text(&m)
        ));
    }
    panic!("round {round} never arrived; saw {seen:?}");
}

async fn probe(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(send_json("/probe", &op)).await;
    let m = recv_route(rx, "").await;
    turn_json(&m)
}

/// The operator gesture: `code.author` gets an enabled row. The shipped
/// `code.author.default` stays `enabled: 0` — turning a capability on is an
/// instance decision, and this test asserts nothing about the seed.
fn code_author_rule() -> Value {
    json!({"operation": "insert", "table": "policy", "row": {
        "rule_id": "code.author.test", "requester": "/os/submit",
        "capability": "code.author", "subject": "*",
        "scope_match": {"scope_prefix": "/os/orgs", "actions": ["apply"]},
        "verdict": "allow", "max_ttl_ms": 900000, "constraints": {},
        "cred_ref": "", "enabled": 1, "priority": 200, "note": "test rule"}})
}

/// The four keys the hive keeps for itself.
const INTERIOR_KEYS: &[&str] = &["access_origin", "access_lane", "ac_phase", "ac_carry"];

// ────────────────────────────────────────────────────────────────────── tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_second_question_is_answered_as_the_second_question() {
    let Some(template) = shipped_access() else {
        return;
    };
    let td = tempfile::tempdir().unwrap();
    build_tree(&td, &template);
    let (h, mut rx) = boot(&td).await;

    // `colony.mutate.default` ships enabled; `code.author` needs the operator's
    // row. Both are then answerable, which is the premise of the whole test:
    // whatever the second answer is, it is not "no such rule".
    probe(&h, &mut rx, code_author_rule()).await;

    h.send(send_json("/asker", &json!({"go": true}))).await;

    let first = turn_json(&recv_round(&mut rx, "1").await);
    assert_eq!(
        first["status"],
        json!("allowed"),
        "the first question is answerable and must be answered: {first}"
    );
    assert_eq!(first["capability"], json!("colony.mutate"));

    let second_msg = recv_round(&mut rx, "2").await;
    let second = turn_json(&second_msg);
    assert_eq!(
        second["capability"],
        json!("code.author"),
        "the second question asked about `code.author`; the broker answered about \
         `{}` — it read the FIRST question out of a carry that outlived its round \
         trip, and never looked at the policy table at all: {second}",
        second["capability"].as_str().unwrap_or("<none>")
    );
    assert_eq!(
        hop_of(&second_msg, "answer_id"),
        "call-two",
        "a `tool_result` belongs to the call that asked for it; this one is stamped \
         with the first question's call id: {second}"
    );
    assert_eq!(
        second["status"],
        json!("allowed"),
        "an enabled `code.author` rule matches this request, so the answer is a yes: {second}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hive_keeps_its_own_bookkeeping_inside() {
    let Some(template) = shipped_access() else {
        return;
    };
    let td = tempfile::tempdir().unwrap();
    build_tree(&td, &template);
    let (h, mut rx) = boot(&td).await;

    h.send(send_json("/asker", &json!({"go": true}))).await;
    let grant = recv_route(&mut rx, "grant").await;

    let leaked: Vec<&str> = INTERIOR_KEYS
        .iter()
        .copied()
        .filter(|k| grant.headers.context.contains_key(*k))
        .collect();
    assert!(
        leaked.is_empty(),
        "the store round trip is the broker's own memory and it belongs inside the \
         hive; these keys rode out on the `grant` lane and are still in the context \
         when the caller asks its next question: {leaked:?} in {:?}",
        grant.headers.context
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invoke_that_carries_its_own_word_is_still_a_request() {
    let Some(template) = shipped_access() else {
        return;
    };
    let td = tempfile::tempdir().unwrap();
    build_tree(&td, &template);
    let (h, mut rx) = boot(&td).await;

    // No `grant_id`, so the honest answer is a refusal — the point is that
    // there IS one. `./invoke` used to read the caller's own `hop.operation` as
    // the mark of its own store echo and fall silent on a perfectly good
    // request, which is the defect of `./policy` one cell over.
    h.send(send_json("/spender", &json!({"operation": "send_message"})))
        .await;
    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(
        payload["outcome"],
        json!("denied"),
        "an invoke without a grant is refused, not swallowed: {payload}"
    );
    assert_eq!(
        payload["reason_code"],
        json!("grant_id_missing"),
        "the refusal names what was missing: {payload}"
    );
}

#[test]
fn every_exit_edge_clears_the_hive_s_own_markers() {
    let Some(template) = shipped_access() else {
        return;
    };
    let config = read_json(&template.join("config.json"));
    let edges = config["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive declares its edges");
    let mut checked = 0;
    for edge in edges {
        if edge["to"] != json!(".") {
            continue;
        }
        checked += 1;
        let cleared: Vec<String> = edge["modifier"]["delete_context"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for key in INTERIOR_KEYS {
            assert!(
                cleared.iter().any(|c| c == key),
                "the exit edge {} -> . lets `{key}` out of the hive; an interior state \
                 key that crosses a sealed boundary comes back on the caller's next \
                 message and is read as this hive's own echo (GH #481). It clears {cleared:?}",
                edge["from"].as_str().unwrap_or("?")
            );
        }
    }
    assert_eq!(
        checked, 3,
        "three lanes leave this hive (policy, invoke, sweep); the count is part of the \
         assertion, because an exit added without the cleanup is exactly the leak"
    );
}
