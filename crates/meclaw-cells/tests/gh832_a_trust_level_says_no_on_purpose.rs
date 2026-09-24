//! GH #832 — a trust level that says no on purpose, an order among the levels, and
//! the newest trust row winning by construction.
//!
//! Measured at `affinity@3.4.0`: `LEVELS` knew four words and no order; the only way
//! to shut a counterpart out was a newer `stranger` row -- the same word as "nobody
//! ever decided" -- and `brief` read `order_by decided_at desc limit 1` over a
//! second-precise stamp with no tie-break, so of two verdicts in one second the store
//! served whichever it scanned first. Measured with the store's own SQLite: the FIRST
//! row inserted wins a tie, so a `blocked` written right after a `known` was not what
//! the next brief read.
//!
//! What is pinned here:
//!
//! 1. `set_trust blocked` is accepted, and a brief for that pair carries
//!    `trust_level blocked`, `trust_rank 0` in the `peer` slot and in the receipt line
//!    -- through a real colony, a real store and the shipped edges;
//! 2. an unknown level is still `trust_level_unknown`;
//! 3. of two trust rows written for one pair within one second, the brief reads the
//!    later one, in both orders;
//! 4. every level has a rank, the order is one literal in `gate`, the same literal in
//!    `brief`, and the README table (drift lock, `docs/development-rules.md` § 2d);
//! 5. the level and its rank ride in an ANSWERED brief only: a pair with nothing
//!    released to the round is refused, and the refusal carries neither -- a
//!    `blocked` pair reads like a `known` one, and the README says so (OR-AG-38).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{repo, run_cell};
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

const GATE: &str = "templates/affinity/gate/config.json";
const BRIEF: &str = "templates/affinity/brief/config.json";
const README: &str = "templates/affinity/README.md";

// ───────────────────────────────────────────────────────────── the colony

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

/// The asking side: one tool call, the asker and a declared solo round on the hop.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
req = {"subject": a.get("subject"), "channel": "*", "slots": a.get("slots") or ["peer"]}
sys.stdout.write(json.dumps({
    "header": {"route": "brief", "audience": str(a.get("audience") or ""),
               "audience_set": json.dumps([])},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "b1",
                  "text": json.dumps(req)}]}))
"#;

/// The writing side, the actor on the hop for the edge to promote.
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": "member:alex"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w1", "text": raw}]}))
"#;

/// The test's own read channel into the store, to see the stamps the gate wrote.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "pstore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "p1", "text": raw}]}))
"#;

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
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
            "emits": {"body": {"messages": {"type": "array", "required": true}}, "hop": hop},
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped affinity template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./asker", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_hop": {"route": "'in_brief'"},
                      "set_context": {"asker": "hop.audience"}}},
        {"from": "./writer", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'propose'",
         "modifier": {"set_hop": {"route": "'in_propose'"},
                      "set_context": {"actor": "hop.actor"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'ack' || hop.route == 'error' || (hop.route == 'answer' && hop.subscriber == ''))"},
        {"from": "./probe", "to": "./affinity/store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"affinity_origin": "'probe'"}}},
        {"from": "./affinity/store", "to": "/sink",
         "condition": "context.affinity_origin == 'probe'"}
    ]}}})
}

async fn boot() -> (tempfile::TempDir, ColonyHandle, mpsc::Receiver<Message>) {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["brief"],
            json!({"audience": {"type": "string", "required": false},
                   "audience_set": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            &["propose"],
            json!({"actor": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["pstore"], json!({})),
    );
    copy_cells(&repo("templates/affinity"), &root.join("main/affinity"));
    // `${uuid7:…}` is minted on the instantiation path; a raw bootstrap is handed a
    // literal, and a cron far enough away that no tick fires during the test.
    let clock = root.join("main/affinity/clock/config.json");
    let mut v: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&clock).unwrap()).unwrap();
    v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-000000000832");
    v["params"]["schedules"][0]["cron"] = json!("0 0 4 * * *");
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

fn to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
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

fn route_of(m: &Message) -> String {
    m.headers
        .hop
        .get("route")
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

async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..12 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("nothing arrived while waiting for {route}; saw {seen:?}"));
        if route_of(&m) == route {
            return m;
        }
        seen.push(format!("{}: {}", route_of(&m), turn_text(&m)));
    }
    panic!("route {route} never arrived; saw {seen:?}");
}

/// One write through the door, answered by its ack.
async fn propose(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(to("/writer", &op.to_string())).await;
    let ack = recv_route(rx, "ack").await;
    meclaw_core::serde_json::from_str(&turn_text(&ack)).unwrap_or(Value::Null)
}

async fn set_trust(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, aud: &str, level: &str) {
    let a = propose(
        h,
        rx,
        json!({"op": "set_trust", "entity_id": "entity:alex", "audience": aud, "level": level}),
    )
    .await;
    assert_eq!(a["outcome"], "accepted", "set_trust {level}: {a}");
}

/// The whole answer body of one brief on `entity:alex`, asking for `peer`.
async fn ask(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, aud: &str) -> Value {
    h.send(to(
        "/asker",
        &json!({"audience": aud, "subject": "entity:alex", "slots": ["peer"]}).to_string(),
    ))
    .await;
    body_of(&recv_route(rx, "answer").await).clone()
}

/// The `peer` slot and the receipt line of one brief on `entity:alex`.
async fn brief(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, aud: &str) -> (Value, String) {
    let body = ask(h, rx, aud).await;
    let peer = body["system"]["peer"].clone();
    let receipt = body["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    (peer, receipt)
}

/// The trust rows of one pair, newest insert last, as the store holds them.
async fn stamps(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, aud: &str) -> Vec<Value> {
    h.send(to(
        "/probe",
        &json!({"operation": "select", "table": "trust",
                "columns": ["level", "decided_at"],
                "where": {"entity_id": "entity:alex", "audience": aud}, "limit": 50})
        .to_string(),
    ))
    .await;
    let m = recv_route(rx, "").await;
    meclaw_core::serde_json::from_str::<Value>(&turn_text(&m))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// A release for the asking audience, so the brief has a `peer` slot to carry.
async fn release_names(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, aud: &str) {
    let a = propose(
        h,
        rx,
        json!({"op": "set_disclosure", "entity_id": "entity:alex", "audience": aud,
               "field_path": "aieos.identity.names", "mode": "share"}),
    )
    .await;
    assert_eq!(a["outcome"], "accepted", "{a}");
}

/// The reason codes the brief wrote to the audit table for one asking audience.
async fn brief_denials(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    aud: &str,
) -> Vec<String> {
    h.send(to(
        "/probe",
        &json!({"operation": "select", "table": "audit",
                "columns": ["outcome", "reason_code"],
                "where": {"actor": aud, "action": "brief"}, "limit": 50})
        .to_string(),
    ))
    .await;
    let m = recv_route(rx, "").await;
    meclaw_core::serde_json::from_str::<Value>(&turn_text(&m))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|r| r["outcome"] == "denied")
        .filter_map(|r| r["reason_code"].as_str().map(str::to_string))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════ pins

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_trust_blocked_is_accepted_and_the_brief_reports_rank_zero() {
    let (_td, h, mut rx) = boot().await;
    release_names(&h, &mut rx, "agent:x").await;
    set_trust(&h, &mut rx, "agent:x", "blocked").await;

    let (peer, receipt) = brief(&h, &mut rx, "agent:x").await;
    assert_eq!(peer["trust_level"], "blocked", "{peer}");
    assert_eq!(
        peer["trust_rank"], 0,
        "blocked is the bottom of the order: {peer}"
    );
    assert!(
        receipt.contains("trust blocked (0)"),
        "the receipt line carries the level and its rank: {receipt}"
    );
    // `brief` compares nothing: a blocked pair still gets what was released, and
    // the caller decides what the rank means.
    assert_eq!(peer["names"]["first"], "Alex", "{peer}");

    // The seeded pair keeps its meaning and gains its number.
    let (peer, receipt) = brief(&h, &mut rx, "agent:aiden").await;
    assert_eq!(peer["trust_level"], "trusted", "{peer}");
    assert_eq!(peer["trust_rank"], 3, "{peer}");
    assert!(receipt.contains("trust trusted (3)"), "{receipt}");

    // No trust row at all is the default, and the default has a rank too.
    release_names(&h, &mut rx, "agent:nobody").await;
    let (peer, receipt) = brief(&h, &mut rx, "agent:nobody").await;
    assert_eq!(peer["trust_level"], "stranger", "{peer}");
    assert_eq!(peer["trust_rank"], 1, "{peer}");
    assert!(receipt.contains("trust stranger (1)"), "{receipt}");

    h.shutdown().await;
}

/// OR-AG-38: `trust_level` and `trust_rank` ride in an ANSWERED brief. With not one
/// field of the subject released to the round, `brief` refuses before it reads the
/// record, and the refusal carries neither -- so a `blocked` pair with nothing
/// released reads exactly like a `known` one with nothing released. The wave keeps
/// the behaviour and makes it visible: the README says it, and says that a caller
/// deriving a permission from the rank reads a missing rank as no. Measured at the
/// receiver: the answer body on the sink, and the reason code the refusal wrote to
/// the audit table -- which the README sentence has to name (drift lock, § 2d).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_pair_without_a_disclosure_is_refused_without_a_level() {
    let (_td, h, mut rx) = boot().await;
    set_trust(&h, &mut rx, "agent:z", "blocked").await;
    set_trust(&h, &mut rx, "agent:w", "known").await;

    let blocked = ask(&h, &mut rx, "agent:z").await;
    let known = ask(&h, &mut rx, "agent:w").await;
    for (who, body) in [("blocked", &blocked), ("known", &known)] {
        assert!(
            body["system"]["peer"].is_null(),
            "{who}: a refusal carries no peer slot: {body}"
        );
        let all = body.to_string();
        assert!(
            !all.contains("trust_level") && !all.contains("trust_rank"),
            "{who}: a refusal carries neither the level nor its rank: {body}"
        );
        let text = body["messages"][0]["text"].as_str().unwrap_or_default();
        assert!(
            !text.contains("trust"),
            "{who}: the refusal's text names no level: {text}"
        );
    }
    assert_eq!(
        blocked["messages"][0]["text"], known["messages"][0]["text"],
        "blocked and known read alike while nothing is released"
    );

    // The release is the whole precondition: one field released to the same pair,
    // and the same brief answers with the level and its rank.
    release_names(&h, &mut rx, "agent:z").await;
    let (peer, receipt) = brief(&h, &mut rx, "agent:z").await;
    assert_eq!(peer["trust_level"], "blocked", "{peer}");
    assert_eq!(peer["trust_rank"], 0, "{peer}");
    assert!(receipt.contains("trust blocked (0)"), "{receipt}");

    // What the refusal wrote, read back from the store.
    let denials = brief_denials(&h, &mut rx, "agent:z").await;
    assert_eq!(
        denials.len(),
        1,
        "one refused brief for agent:z: {denials:?}"
    );
    let reason = denials[0].clone();
    h.shutdown().await;

    // Drift lock: README § The write ops states the precondition in one sentence
    // naming the measured code, and what a caller makes of a missing rank; the
    // `peer` row of the slot table says it where the slot is described.
    let readme = std::fs::read_to_string(repo(README)).expect("README");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    let sentence = flat
        .split(". ")
        .find(|s| s.contains("carries neither `trust_level` nor `trust_rank`"))
        .expect("README states that a refusal carries neither the level nor its rank");
    assert!(
        sentence.contains(&format!("`{reason}`")),
        "the sentence names the refusal the brief wrote ({reason}): {sentence}"
    );
    assert!(
        flat.contains("reads a missing rank as no"),
        "README says what a caller makes of a missing rank"
    );
    let row = readme
        .lines()
        .find(|l| l.starts_with("| `peer` |"))
        .expect("the slot table has a peer row");
    assert!(
        row.contains("a refusal carries neither"),
        "the peer row names the precondition: {row}"
    );
}

#[test]
fn an_unknown_level_is_still_refused() {
    let (out, _) = run_cell(
        GATE,
        &[],
        json!({
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": json!({"op": "set_trust", "entity_id": "entity:alex",
                                         "audience": "agent:x", "level": "enemy"}).to_string()}],
            "header": {"hop": {}, "context": {"actor": "member:alex"}}
        }),
    );
    let ack = out
        .iter()
        .find(|m| m["header"]["route"] == "ack")
        .expect("an ack");
    assert_eq!(
        ack["header"]["reason_code"], "trust_level_unknown",
        "{out:?}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "astore"
            && m["messages"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("\"trust\"")),
        "nothing reaches the trust table: {out:?}"
    );
}

/// OR-AG-3: the newest trust row wins by construction, also inside one second. Two
/// writes are sent back to back, in both orders; the brief after each pair must read
/// the second one. The pairs are repeated until at least one of them was measured
/// inside one second at the store, so the case the fix is for is really exercised.
/// The first two pairs use levels every version knew, so the tie is what fails on a
/// tree without the fix -- not the new word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_newest_trust_row_wins_within_one_second() {
    let (_td, h, mut rx) = boot().await;
    release_names(&h, &mut rx, "agent:y").await;

    let mut same_second = 0;
    for attempt in 0..5 {
        for (first, second) in [
            ("known", "trusted"),
            ("trusted", "known"),
            ("known", "blocked"),
            ("blocked", "known"),
        ] {
            set_trust(&h, &mut rx, "agent:y", first).await;
            set_trust(&h, &mut rx, "agent:y", second).await;
            let rows = stamps(&h, &mut rx, "agent:y").await;
            let n = rows.len();
            assert!(n >= 2, "{rows:?}");
            let a = rows[n - 2]["decided_at"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let b = rows[n - 1]["decided_at"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if a.get(..19) == b.get(..19) {
                same_second += 1;
            }
            let (peer, receipt) = brief(&h, &mut rx, "agent:y").await;
            assert_eq!(
                peer["trust_level"], second,
                "attempt {attempt}: {first} then {second} ({a} / {b}) must read {second}: \
                 {peer} / {receipt}"
            );
        }
        if same_second > 0 {
            break;
        }
    }
    assert!(
        same_second > 0,
        "no pair ever landed inside one second, so the tie was never exercised"
    );

    h.shutdown().await;
}

/// The literal `LEVELS = (...)` of one shipped script.
fn levels_literal(rel: &str) -> Vec<String> {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(repo(rel)).expect("config"))
            .expect("json");
    let src = cfg["params"]["script_inline"]
        .as_str()
        .expect("script")
        .to_string();
    let line = src
        .lines()
        .find(|l| l.starts_with("LEVELS = ("))
        .unwrap_or_else(|| panic!("{rel} carries no LEVELS literal"));
    line.trim_start_matches("LEVELS = (")
        .trim_end_matches(')')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Drift lock: the order is one literal, the same in the writer and the reader, and
/// the README table states it -- and the mechanism behind each row is run: the gate
/// accepts the level, and the brief reports the rank the table names.
#[test]
fn every_level_has_a_rank_and_the_order_is_locked() {
    let gate = levels_literal(GATE);
    let brief = levels_literal(BRIEF);
    assert_eq!(
        gate, brief,
        "the writer and the reader order the levels alike"
    );
    assert_eq!(
        gate.first().map(String::as_str),
        Some("blocked"),
        "{gate:?}"
    );
    assert_eq!(gate.len(), 5, "{gate:?}");

    let readme = std::fs::read_to_string(repo(README)).expect("README");
    let table: Vec<(i64, String)> = readme
        .lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            if cells.len() < 4 {
                return None;
            }
            let rank = cells[1].parse::<i64>().ok()?;
            let level = cells[2].strip_prefix('`')?.strip_suffix('`')?;
            Some((rank, level.to_string()))
        })
        .collect();
    let want: Vec<(i64, String)> = gate
        .iter()
        .enumerate()
        .map(|(i, l)| (i as i64, l.clone()))
        .collect();
    assert_eq!(
        table, want,
        "the README table of trust levels is the literal"
    );

    for (rank, level) in &want {
        let (out, _) = run_cell(
            GATE,
            &[],
            json!({
                "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                              "text": json!({"op": "set_trust", "entity_id": "entity:alex",
                                             "audience": "agent:x", "level": level}).to_string()}],
                "header": {"hop": {}, "context": {"actor": "member:alex"}}
            }),
        );
        let ack = out
            .iter()
            .find(|m| m["header"]["route"] == "ack")
            .expect("ack");
        assert_eq!(ack["header"]["outcome"], "accepted", "{level}: {out:?}");

        // The brief's last phase, handed the level the trust read found.
        let carry = json!({
            "trust": level, "call_id": "b1", "relations": [],
            "allow": [{"field_path": "aieos.identity.names", "mode": "share"}]
        });
        let entity = json!([{"entity_id": "entity:alex", "kind": "person",
                             "display_name": "Alex Kern",
                             "aieos": {"identity": {"names": {"first": "Alex"}}}, "mx": {}}]);
        let (out, _) = run_cell(
            BRIEF,
            &[],
            json!({
                "messages": [{"origin": "tool", "type": "tool_result", "id": "p",
                              "text": entity.to_string()}],
                "header": {"hop": {"operation": "select"}, "context": {
                    "aff_phase": "entity", "aff_subject": "entity:alex",
                    "aff_audience": "agent:x", "aff_channel": "*",
                    "aff_slots": "[\"peer\"]", "aff_carry": carry.to_string()}}
            }),
        );
        let answer = out
            .iter()
            .find(|m| m["header"]["route"] == "answer")
            .unwrap_or_else(|| panic!("{level}: no answer: {out:?}"));
        assert_eq!(
            answer["system"]["peer"]["trust_level"],
            json!(level),
            "{answer}"
        );
        assert_eq!(
            answer["system"]["peer"]["trust_rank"],
            json!(rank),
            "{answer}"
        );
        let receipt = answer["messages"][0]["text"].as_str().unwrap_or_default();
        assert!(
            receipt.contains(&format!("trust {level} ({rank})")),
            "{level}: {receipt}"
        );
    }
}
