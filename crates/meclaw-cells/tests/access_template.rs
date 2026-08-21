//! meclaw-os -- the shipped `access@1` template, the capability broker (V8
//! spec § 2, rulings of 2026-08-15).
//!
//! What is pinned here is what the template PROMISES, in the order the README
//! promises it:
//!
//! 1. **The inventory, and that it starts inert.** Five cells, none of them an
//!    `llm`; not one seeded policy row is enabled; not one seeded credential
//!    row carries a value rather than a variable NAME. A broker that shipped
//!    switched on would be a broker nobody decided about.
//! 2. **A request becomes a deterministic grant.** One enabled rule, one
//!    request, and out comes a handle with an `expires_at`, the rule's
//!    constraints and an audit line -- while the answer to the model carries no
//!    address at all.
//! 3. **R-AC-1: the requester comes from the EDGE.** The body of the request
//!    claims a different requester, and the rule that decides only matches the
//!    edge's one. The grant is issued anyway, to the edge's identity: proof
//!    over the wire rather than over trust.
//! 4. **R-AC-2: the address comes from the GRANT.** An invoke whose payload
//!    names a different chat reaches the connector addressed at the granted
//!    chat, without the payload key, and the attempt is in the audit detail.
//! 5. **The TTL bites on the call, the sweep only writes it down.** An expired
//!    grant is refused by `invoke` itself; the `expired` row in `grant_events`
//!    arrives afterwards, on the tick.
//! 6. **Revocation is a row, and it is instant.** A `revoked` event refuses the
//!    next invoke although `expires_at` is still in the future -- because the
//!    effective state is the NEWEST event, not a column.
//! 7. **An unknown capability is denied and audited.** Fail closed, with a
//!    reason_code a caller can explain to a human.
//!
//! Free of a provider by construction: this hive holds no model at all, and the
//! connector below is a test cell that sends nowhere.
//!
//! **R2b guard (GH #49 form).** `access@1` is PRIVATE -- it is not in
//! `PUBLIC_TEMPLATES`, so it does not travel with the export. Every read below
//! is guarded per file by [`shipped_access`]; in the public clone the guard
//! exits cleanly and these tests skip instead of failing on a dead `templates/`
//! reference. Same form as `affinity_template.rs`, and the matching
//! `ALLOWED_HITS` entry lives in `plans/export-fixtures/make_export.py`.

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

/// Every cell the hive is made of. The list is the guard AND the inventory: a
/// cell that silently disappears makes these tests skip rather than pass.
const ACCESS_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "policy/config.json",
    "invoke/config.json",
    "sweep/config.json",
    "clock/config.json",
    // GH #151: the vault joined as an INTERIOR cell — not a port, so the
    // generic hive-port boundary refuses any edge into it from outside the
    // scope. It is the sixth cell and still no model.
    "vault/config.json",
];

/// The two seed tables. Both are catalogue, neither is a secret: `policy` ships
/// disabled and `cred_refs` ships variable names.
const ACCESS_SEEDS: &[&str] = &["store/seed/policy.jsonl", "store/seed/cred_refs.jsonl"];

/// The template root, or `None` where it does not ship (the documented R2b
/// exception form, GH #49).
fn shipped_access() -> Option<std::path::PathBuf> {
    let root = templates_root().join("access");
    for rel in ACCESS_FILES.iter().chain(ACCESS_SEEDS) {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// The shipped template, copied cell by cell: `config.json` files and the seeds
/// next to them travel, which is what instantiation copies.
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

fn collect_configs(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            collect_configs(root, &p, out);
        } else if entry.file_name() == "config.json" {
            out.push(p.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

// ────────────────────────────────────────────────────────── the test-only cells

/// The asking side of the request port. It declares WHO is asking on the hop --
/// which the port edge then promotes to `context.requester`. Everything else it
/// is handed becomes the tool_call `arguments` verbatim, INCLUDING a forged
/// `requester` when a test wants to plant one.
const REQUESTER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
who = str(a.pop("who", ""))
sys.stdout.write(json.dumps({
    "header": {"route": "request", "who": who},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "r1",
                  "text": json.dumps(a)}]}))
"#;

/// The spending side of the invoke port, same shape and same promotion.
const SPENDER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
who = str(a.pop("who", ""))
sys.stdout.write(json.dumps({
    "header": {"route": "invoke", "who": who},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "i1",
                  "text": json.dumps(a)}]}))
"#;

/// The MOCK connector. It is deliberately as dumb as the README says a real one
/// is: it knows no grant, it reads the address off the hop, and it reaches no
/// network of any kind. Nothing in this test suite ever calls a channel.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = (envelope.get("header") or {}).get("hop") or {}
msgs = d.get("messages") or [{}]
sys.stdout.write(json.dumps({
    "header": {"route": "sent", "channel": str(hop.get("channel") or ""),
               "address": str(hop.get("address") or "")},
    "messages": [{"origin": "assistant", "type": "text",
                  "text": json.dumps({"payload": d.get("payload"),
                                      "text": msgs[-1].get("text")})}]}))
"#;

/// The probe. It reaches PAST the boundary straight into `./store` -- exactly
/// the bypass the README calls out as possible from a boot graph. Here it is
/// used the two ways that are legitimate: to read what the broker wrote, and to
/// play the operator who turns a policy row on or writes a revocation.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
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

/// The lanes around the hive -- every one a literal copy of what
/// `templates/access/README.md` documents, plus the probe pair that is the
/// test's own read channel. The template draws no edge that appears here.
///
/// **Every endpoint that belongs to the broker is the HIVE path** (GH #197):
/// `params.ports` is empty, so a caller names `./access` and a lane on
/// `hop.route`, and which cell answers is decided by the hive's own door edge.
/// That is what makes the two invariants below invariants of an INTERFACE
/// rather than of an arrangement — an inside rebuilt differently keeps them.
///
/// Note the shape of `connect`: exactly ONE edge leaves `./access` towards
/// `./connector`, and nothing else in this tree can reach it. That single edge
/// is what the whole broker is worth, and the README says so.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── in_request: the caller's identity becomes EDGE truth, and only here ──
        {"from": "./requester", "to": "./access",
         "condition": "has(hop.route) && hop.route == 'request'",
         "modifier": {"set_hop": {"route": "'in_request'"},
                      "set_context": {"requester": "hop.who"}}},
        // ── grant ──
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'grant'"},
        // ── in_invoke: the same promotion, because a grant belongs to somebody ──
        {"from": "./spender", "to": "./access",
         "condition": "has(hop.route) && hop.route == 'invoke'",
         "modifier": {"set_hop": {"route": "'in_invoke'"},
                      "set_context": {"requester": "hop.who"}}},
        // ── ack ──
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        // ── connect: THE one edge into the connector ──
        {"from": "./access", "to": "./connector",
         "condition": "has(hop.route) && hop.route == 'connect'"},
        {"from": "./connector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'sent'"},
        // ── error: the drain the parent MUST wire; ONE edge, because which
        //    cell inside failed is not the caller's business ──
        {"from": "./access", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── the test's own read channel, straight into the store. This is the
        //    bypass the README's `The honest limit` names: legal here only
        //    because a BOOT graph is the sovereign birth draft and the seal is
        //    warned about rather than enforced there. A mutation drawing it is
        //    refused with `hive_port_boundary`, which is asserted separately. ──
        {"from": "./probe", "to": "./access/store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"access_origin": "'probe'"}}},
        {"from": "./access/store", "to": "/sink",
         "condition": "context.access_origin == 'probe'"}
    ]}}})
}

/// A cron far enough away that no sweep happens during a test that is not about
/// sweeping. The TTL test overrides it.
const QUIET_CRON: &str = "0 0 4 * * *";

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, cron: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), format!("ACCESS_SWEEP_CRON={cron}\n")).unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/requester/config.json",
        &code_cell(
            REQUESTER,
            &["request"],
            json!({"who": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/spender/config.json",
        &code_cell(
            SPENDER,
            &["invoke"],
            json!({"who": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/connector/config.json",
        &code_cell(
            CONNECTOR,
            &["sent"],
            json!({"channel": {"type": "string", "required": false},
                   "address": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["pstore"], json!({})),
    );
    copy_cells(root_template, &root.join("main/access"));
    // `${uuid7:…}` is an INSTANTIATION-side substitution (the mutation path
    // mints it); a raw directory copy bootstrapped from the filesystem has to
    // be handed a literal. The cron itself is NOT patched -- it comes out of the
    // `.env` above through the shipped `${ACCESS_SWEEP_CRON:-…}` default, so the
    // late-binding form is under test rather than bypassed.
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
            // GH #151: the hive carries a vault since the credentials moved off
            // the wire, so the boot needs its factory like any other.
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

/// The JSON payload of the one turn a store or a port answer carries.
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
    for _ in 0..16 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("nothing more arrived while waiting for route {route}; saw {seen:?}");
        });
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(format!("{}: {}", hop_of(&m, "route"), turn_text(&m)));
    }
    panic!("route {route} never arrived; saw {seen:?}");
}

/// The first message on each of several routes, in the order the routes are
/// named -- regardless of the order they actually arrive in. `invoke` emits its
/// connector message and its acknowledgement in one pass, so which of the two
/// reaches the sink first is a scheduling detail and not a promise.
async fn recv_routes(rx: &mut mpsc::Receiver<Message>, routes: &[&str]) -> Vec<Message> {
    let mut got: Vec<Option<Message>> = routes.iter().map(|_| None).collect();
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..16 {
        if got.iter().all(|g| g.is_some()) {
            break;
        }
        let m = recv_bounded(rx)
            .await
            .unwrap_or_else(|| panic!("nothing more arrived; want {routes:?}, saw {seen:?}"));
        let route = hop_of(&m, "route");
        seen.push(format!("{route}: {}", turn_text(&m)));
        if let Some(i) = routes.iter().position(|r| *r == route)
            && got[i].is_none()
        {
            got[i] = Some(m);
        }
    }
    got.into_iter()
        .enumerate()
        .map(|(i, g)| {
            g.unwrap_or_else(|| panic!("route {} never arrived; saw {seen:?}", routes[i]))
        })
        .collect()
}

/// A store op through the probe, returned as its rows.
async fn probe(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(send_json("/probe", &op)).await;
    let m = recv_route(rx, "").await;
    turn_json(&m)
}

/// The operator's one gesture: a rule turned ON. Every seeded row ships
/// `enabled: 0`, so a test that wants a verdict has to write the row it wants --
/// which is precisely how a real colony grants a capability.
fn allow_rule(rule_id: &str, requester: &str, capability: &str) -> Value {
    json!({"operation": "insert", "table": "policy", "row": {
        "rule_id": rule_id, "requester": requester, "capability": capability,
        "subject": "member:example",
        "scope_match": {"channel": "example-chat", "chat_id": "*",
                        "actions": ["send_message"]},
        "verdict": "allow", "max_ttl_ms": 900000,
        "constraints": {"max_invocations": 20},
        "cred_ref": "cred:example-chat:primary",
        "enabled": 1, "priority": 100, "note": "test rule"}})
}

fn request(who: &str, chat: &str, ttl_ms: i64) -> Value {
    json!({"who": who, "capability": "chat.send", "subject": "member:example",
           "resource": {"channel": "example-chat", "chat_id": chat},
           "purpose": "answer the incoming message", "ttl_ms": ttl_ms})
}

/// Ask once and return the granted handle, failing loudly on any other verdict.
async fn grant_for(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    who: &str,
    chat: &str,
    ttl_ms: i64,
) -> String {
    h.send(send_json("/requester", &request(who, chat, ttl_ms)))
        .await;
    let m = recv_route(rx, "grant").await;
    let payload = turn_json(&m);
    assert_eq!(
        payload["status"].as_str(),
        Some("granted"),
        "expected a grant: {payload}"
    );
    payload["grant_id"].as_str().unwrap_or_default().to_string()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Five cells, none of them a model -- and a broker that grants nothing until
/// somebody says so. The seed is pinned in the same test because "ships inert"
/// is a security property, not a style choice: a template that arrived with an
/// enabled rule would hand out a capability nobody in the receiving colony ever
/// decided about.
#[test]
fn the_hive_carries_six_cells_no_model_and_starts_inert() {
    let Some(root) = shipped_access() else {
        return;
    };
    let mut found = Vec::new();
    collect_configs(&root, &root, &mut found);
    let mut found: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    let mut want: Vec<String> = ACCESS_FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        found, want,
        "access@1 is store + policy + invoke + sweep + clock + vault: no brain, no judge"
    );
    for rel in ACCESS_FILES {
        let cfg = read_json(&root.join(rel));
        assert_ne!(
            cfg["cell"]["type"].as_str().unwrap_or_default(),
            "llm",
            "{rel} is an llm cell -- the verdict of this hive is a comparison"
        );
    }

    // 1. Every seeded policy row is disabled.
    let raw = std::fs::read_to_string(root.join("store/seed/policy.jsonl")).unwrap();
    let mut rules = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).unwrap();
        // Line 1 of a seed file is the column-type header, not a row.
        if row.get("schema").is_some() {
            continue;
        }
        assert_eq!(
            row["enabled"].as_i64(),
            Some(0),
            "a seeded policy row is ENABLED: {row}"
        );
        rules += 1;
    }
    assert!(
        rules >= 2,
        "the policy seed pin swept almost nothing: {rules}"
    );

    // 2. Every seeded credential row is a NAME, not a value. `${VAR}` binds
    //    late, so the template can only ever ship the catalogue -- and this is
    //    the test that says the catalogue stayed one.
    let raw = std::fs::read_to_string(root.join("store/seed/cred_refs.jsonl")).unwrap();
    let mut creds = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).unwrap();
        if row.get("schema").is_some() {
            continue;
        }
        let env_var = row["env_var"].as_str().unwrap_or_default();
        assert!(
            !env_var.is_empty()
                && env_var
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
            "cred_refs.env_var must be a variable NAME: {row}"
        );
        for key in ["token", "value", "secret", "key"] {
            assert!(
                row.get(key).is_none(),
                "cred_refs must never carry a {key} column: {row}"
            );
        }
        creds += 1;
    }
    assert!(creds >= 1, "the cred_refs seed pin swept nothing");

    // 3. And no cell of this template declares a secret setting at all -- the
    //    vault is .env plus the connector, and this hive is neither.
    for rel in ACCESS_FILES {
        let cfg = read_json(&root.join(rel));
        if let Some(settings) = cfg["contract"]["settings"].as_object() {
            for (name, spec) in settings {
                assert_ne!(
                    spec["secret"].as_bool(),
                    Some(true),
                    "{rel} declares a secret setting {name} -- the broker holds no credential"
                );
            }
        }
    }
}

/// The happy path, end to end: one enabled rule, one request, one grant. What
/// the model is handed back is a handle, an expiry and a channel-level summary
/// -- and NOT the address, which is the half of R-AC-2 that lives on the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_becomes_a_deterministic_grant_with_a_ttl_and_an_audit_line() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    probe(
        &h,
        &mut rx,
        allow_rule("t-chat-send", "agent:example", "chat.send"),
    )
    .await;

    h.send(send_json(
        "/requester",
        &request("agent:example", "chat-1", 900_000),
    ))
    .await;
    let m = recv_route(&mut rx, "grant").await;
    let payload = turn_json(&m);
    assert_eq!(payload["status"].as_str(), Some("granted"), "{payload}");
    let grant_id = payload["grant_id"].as_str().unwrap_or_default().to_string();
    assert!(grant_id.starts_with("grant:"), "handle: {grant_id}");
    assert!(
        !payload["expires_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "a grant without an expiry is not a grant: {payload}"
    );
    assert_eq!(
        payload["constraints"]["max_invocations"].as_i64(),
        Some(20),
        "the rule's constraints ride back with the handle: {payload}"
    );
    let dump = meclaw_core::serde_json::to_string(&payload).unwrap();
    assert!(
        dump.contains("chat.send") && dump.contains("example-chat"),
        "the summary names the capability and the channel: {dump}"
    );
    assert!(
        !dump.contains("chat-1") && !dump.contains("cred:"),
        "the answer must carry no address and no credential reference: {dump}"
    );
    assert_eq!(
        hop_of(&m, "verdict"),
        "granted",
        "the verdict rides on the hop so a parent can route on it"
    );

    // The grant row carries what the answer withheld: the frozen address, the
    // credential reference and the rule that decided.
    let rows = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "grants",
               "columns": ["grant_id", "requester", "capability", "scope",
                           "cred_ref", "rule_id", "expires_at", "purpose"],
               "where": {"grant_id": grant_id.clone()}, "limit": 5}),
    )
    .await;
    assert_eq!(rows.as_array().map(|a| a.len()), Some(1), "grants: {rows}");
    assert_eq!(rows[0]["capability"].as_str(), Some("chat.send"));
    assert_eq!(rows[0]["rule_id"].as_str(), Some("t-chat-send"));
    assert_eq!(
        rows[0]["cred_ref"].as_str(),
        Some("cred:example-chat:primary"),
        "the grant is bound to a credential REFERENCE, never to a value: {rows}"
    );
    let scope: Value =
        meclaw_core::serde_json::from_str(rows[0]["scope"].as_str().unwrap_or_default())
            .expect("the stored scope is a JSON document");
    assert_eq!(
        scope["chat_id"].as_str(),
        Some("chat-1"),
        "the wildcard coordinate was taken from the request and FROZEN here: {scope}"
    );
    assert_eq!(scope["channel"].as_str(), Some("example-chat"));
    assert_eq!(scope["actions"][0].as_str(), Some("send_message"));

    // The grant is usable because a `granted` event exists -- the effective
    // state of a grant is its newest event, so this row is not decoration.
    let events = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "grant_events",
               "columns": ["event", "actor"],
               "where": {"grant_id": grant_id.clone()}, "limit": 5}),
    )
    .await;
    assert_eq!(events.as_array().map(|a| a.len()), Some(1), "{events}");
    assert_eq!(events[0]["event"].as_str(), Some("granted"));

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "action", "capability", "outcome", "reason_code"],
               "where": {"action": "request"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    assert_eq!(audit[0]["outcome"].as_str(), Some("granted"));
    assert_eq!(
        audit[0]["actor"].as_str(),
        Some("agent:example"),
        "the actor came from context, which only an edge can write: {audit}"
    );

    h.shutdown().await;
}

/// **R-AC-1**, proved over the wire rather than over trust.
///
/// The body claims `requester: agent:forged`, the edge promotes
/// `agent:example`, and the single enabled rule matches ONLY `agent:example`.
/// A cell that read the body would therefore have to deny. This one grants --
/// and the grant, the event actor and the audit line all name the edge's
/// identity. There is no assertion here about what the cell "ignores": the
/// evidence is that the rule fired at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_requester_comes_from_the_edge_and_never_from_the_body() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    probe(
        &h,
        &mut rx,
        allow_rule("t-edge-only", "agent:example", "chat.send"),
    )
    .await;

    let mut forged = request("agent:example", "chat-1", 900_000);
    forged["requester"] = json!("agent:forged");
    h.send(send_json("/requester", &forged)).await;

    let payload = turn_json(&recv_route(&mut rx, "grant").await);
    assert_eq!(
        payload["status"].as_str(),
        Some("granted"),
        "the rule matches the EDGE's requester; a body-reading policy would have \
         denied here: {payload}"
    );
    let grant_id = payload["grant_id"].as_str().unwrap_or_default().to_string();

    let rows = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "grants",
               "columns": ["grant_id", "requester"],
               "where": {"grant_id": grant_id.clone()}, "limit": 5}),
    )
    .await;
    assert_eq!(
        rows[0]["requester"].as_str(),
        Some("agent:example"),
        "the grant belongs to the edge's identity: {rows}"
    );

    // And nothing anywhere in the broker's own tables ever learned the forged
    // name -- it was not stored, not audited and not echoed.
    for table in ["grants", "audit"] {
        let hit = probe(
            &h,
            &mut rx,
            json!({"operation": "select", "table": table,
                   "columns": ["actor"] , "where": {"actor": "agent:forged"},
                   "limit": 5}),
        )
        .await;
        let hit = if hit.is_array() { hit } else { json!([]) };
        assert_eq!(
            hit.as_array().map(|a| a.len()),
            Some(0),
            "the forged requester reached {table}: {hit}"
        );
    }

    h.shutdown().await;
}

/// **R-AC-2**: the address comes from the grant, the content from the payload.
///
/// A legitimate grant for `chat-1` plus a payload naming `chat-999`. The
/// connector -- the only cell in this tree that would ever talk to a channel --
/// is addressed at `chat-1`, never sees the key, and the attempt is on record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_address_comes_from_the_grant_and_never_from_the_payload() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    probe(
        &h,
        &mut rx,
        allow_rule("t-addr", "agent:example", "chat.send"),
    )
    .await;
    let grant_id = grant_for(&h, &mut rx, "agent:example", "chat-1", 900_000).await;

    h.send(send_json(
        "/spender",
        &json!({"who": "agent:example", "grant_id": grant_id,
                "operation": "send_message",
                "payload": {"chat_id": "chat-999", "channel": "other-chat",
                            "text": "on my way"}}),
    ))
    .await;

    let pair = recv_routes(&mut rx, &["sent", "ack"]).await;
    let (sent, ack) = (&pair[0], &pair[1]);
    let address: Value =
        meclaw_core::serde_json::from_str(&hop_of(sent, "address")).unwrap_or(Value::Null);
    assert_eq!(
        address["chat_id"].as_str(),
        Some("chat-1"),
        "the connector was addressed from the GRANT: {address}"
    );
    assert_eq!(address["channel"].as_str(), Some("example-chat"));
    assert_eq!(hop_of(sent, "channel"), "example-chat");

    let seen = turn_json(sent);
    assert_eq!(
        seen["text"].as_str(),
        Some("on my way"),
        "the content still came from the payload: {seen}"
    );
    assert!(
        seen["payload"].get("chat_id").is_none() && seen["payload"].get("channel").is_none(),
        "an address key must be REMOVED before the connector, not merged: {seen}"
    );
    let dump = meclaw_core::serde_json::to_string(&seen).unwrap();
    assert!(
        !dump.contains("chat-999") && !dump.contains("other-chat"),
        "the payload's own address must not reach the connector at all: {dump}"
    );

    assert_eq!(turn_json(ack)["outcome"].as_str(), Some("ok"), "{ack:?}");

    // The refused steering attempt is a row, because an operator wants to see a
    // model that keeps trying to address its own sends.
    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["outcome", "detail"], "where": {"action": "invoke"},
               "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    let detail: Value =
        meclaw_core::serde_json::from_str(audit[0]["detail"].as_str().unwrap_or_default())
            .expect("the audit detail is a JSON document");
    let ignored: Vec<&str> = detail["payload_address_ignored"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        ignored.contains(&"chat_id") && ignored.contains(&"channel"),
        "both address keys are named in the audit: {detail}"
    );

    // One usage row, so the count that feeds `max_invocations` is real.
    let usage = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "usage",
               "columns": ["outcome", "operation"], "limit": 5}),
    )
    .await;
    assert_eq!(usage.as_array().map(|a| a.len()), Some(1), "usage: {usage}");
    assert_eq!(usage[0]["outcome"].as_str(), Some("ok"));

    h.shutdown().await;
}

/// The TTL, both halves. `invoke` refuses on its own comparison, so the door
/// closes on time whether or not the clock ticks; the `expired` row in
/// `grant_events` is what the sweep adds afterwards, and it is bookkeeping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_grant_is_refused_and_the_sweep_writes_the_event() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    // Two seconds, so this test sees a tick inside its own budget.
    build_tree(&td, &root, "*/2 * * * * *");
    let (h, mut rx) = boot(&td).await;

    probe(
        &h,
        &mut rx,
        allow_rule("t-ttl", "agent:example", "chat.send"),
    )
    .await;
    // One millisecond of life: the rule's max_ttl is the ceiling, the request
    // asks for less, and the smaller of the two wins.
    let grant_id = grant_for(&h, &mut rx, "agent:example", "chat-1", 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    h.send(send_json(
        "/spender",
        &json!({"who": "agent:example", "grant_id": grant_id,
                "operation": "send_message", "payload": {"text": "too late"}}),
    ))
    .await;
    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(payload["outcome"].as_str(), Some("denied"), "{payload}");
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("grant_expired"),
        "the TTL bites on the CALL, not on the tick: {payload}"
    );

    // Nothing reached the connector. The sink is drained for a moment, and any
    // `sent` in that window would be the defect this whole cell exists against.
    let quiet = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    if let Ok(Some(m)) = quiet {
        assert_ne!(
            hop_of(&m, "route"),
            "sent",
            "an expired grant reached the connector: {:?}",
            m.headers.hop
        );
    }

    // And the sweep catches up: within a few two-second ticks the newest event
    // of this grant is `expired`, written by the sweep and not by the caller.
    let mut event = String::new();
    for _ in 0..12 {
        let rows = probe(
            &h,
            &mut rx,
            json!({"operation": "select", "table": "grant_events",
                   "columns": ["event", "actor", "reason_code"],
                   "where": {"grant_id": grant_id.clone(), "event": "expired"},
                   "limit": 5}),
        )
        .await;
        if rows.as_array().map(|a| a.len()).unwrap_or(0) > 0 {
            assert_eq!(rows[0]["actor"].as_str(), Some("access/sweep"));
            assert_eq!(rows[0]["reason_code"].as_str(), Some("ttl_elapsed"));
            event = "expired".to_string();
            break;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    assert_eq!(
        event, "expired",
        "the sweep never wrote the expired event for {grant_id}"
    );

    h.shutdown().await;
}

/// Revocation without an update and without a delete: one `revoked` row lands
/// in `grant_events`, and the very next invoke is refused although `expires_at`
/// is still fifteen minutes away. The effective state of a grant is its NEWEST
/// event -- which is why the history stays complete and the withdrawal is
/// instant at the same time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_grant_is_refused_although_it_has_not_expired() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    probe(
        &h,
        &mut rx,
        allow_rule("t-revoke", "agent:example", "chat.send"),
    )
    .await;
    let grant_id = grant_for(&h, &mut rx, "agent:example", "chat-1", 900_000).await;

    // The operator's revocation: an APPEND, dated so that it is unambiguously
    // the newest row. Nothing is updated and nothing is deleted -- the `granted`
    // row it overrules stays readable, which is the point of an audit trail.
    probe(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "grant_events", "row": {
            "id": "ev-revoke-1", "grant_id": grant_id.clone(), "event": "revoked",
            "at": "2099-01-01T00:00:00.000000Z", "actor": "member:example",
            "reason_code": "operator", "detail": {}}}),
    )
    .await;

    h.send(send_json(
        "/spender",
        &json!({"who": "agent:example", "grant_id": grant_id.clone(),
                "operation": "send_message", "payload": {"text": "still there?"}}),
    ))
    .await;
    let payload = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(payload["outcome"].as_str(), Some("denied"), "{payload}");
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("grant_revoked"),
        "a revoked grant is dead at once, expires_at notwithstanding: {payload}"
    );

    // The `granted` row is still there. A revocation that erased its own history
    // would answer 'who was allowed to do what on the third' with silence.
    let events = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "grant_events",
               "columns": ["event"], "where": {"grant_id": grant_id.clone()},
               "order_by": [{"col": "at", "dir": "asc"}], "limit": 10}),
    )
    .await;
    let kinds: Vec<&str> = events
        .as_array()
        .map(|a| a.iter().filter_map(|r| r["event"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(
        kinds,
        vec!["granted", "revoked"],
        "the history is append-only and complete: {events}"
    );

    h.shutdown().await;
}

/// Fail closed, and say which door was knocked on. A capability no enabled rule
/// mentions is denied with `capability_unknown` -- distinguishable from an
/// explicit `deny`, because a caller that cannot tell a closed door from a wall
/// will guess, and guessing is what a broker exists to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_capability_is_denied_and_audited() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    // A rule exists -- for something else entirely. The seeded rows are all
    // disabled, so `fs.write` is genuinely unmentioned.
    probe(
        &h,
        &mut rx,
        allow_rule("t-other", "agent:example", "chat.send"),
    )
    .await;

    h.send(send_json(
        "/requester",
        &json!({"who": "agent:example", "capability": "fs.write",
                "subject": "member:example", "resource": {"path": "/etc/passwd"},
                "purpose": "no", "ttl_ms": 60000}),
    ))
    .await;

    let m = recv_route(&mut rx, "grant").await;
    let payload = turn_json(&m);
    assert_eq!(payload["status"].as_str(), Some("denied"), "{payload}");
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("capability_unknown"),
        "an unmentioned capability is 'unknown', not 'forbidden': {payload}"
    );
    assert!(
        payload.get("grant_id").is_none(),
        "a denial mints no handle: {payload}"
    );

    let grants = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "grants",
               "columns": ["grant_id"], "limit": 5}),
    )
    .await;
    assert_eq!(
        grants.as_array().map(|a| a.len()),
        Some(0),
        "a denied request writes no grant row at all: {grants}"
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "capability", "outcome", "reason_code"],
               "where": {"outcome": "denied"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    assert_eq!(audit[0]["capability"].as_str(), Some("fs.write"));
    assert_eq!(audit[0]["reason_code"].as_str(), Some("capability_unknown"));
    assert_eq!(
        audit[0]["actor"].as_str(),
        Some("agent:example"),
        "a refusal is the more interesting half of the log: {audit}"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────── the boundary (GH #197, #200)

/// The hive is sealed to its own path, and what it offers is stated in lanes.
///
/// `access@1.x` declared `params.ports: ["policy", "invoke", "store"]`. Two of
/// those are cell names standing in for lanes, and the third was the bypass
/// this template's own README calls out as its honest limit — a declared port
/// straight into the store is an invitation to write a policy row without
/// asking anybody.
#[test]
fn the_hive_is_sealed_to_its_own_path_and_states_its_lanes() {
    let Some(root) = shipped_access() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    let ports = cfg["params"]["ports"]
        .as_array()
        .expect("params.ports is declared");
    assert!(
        ports.is_empty(),
        "the hive path is the only address, got {ports:?}"
    );

    let lanes = |key: &str| -> Vec<String> {
        cfg["params"]["contract"][key]
            .as_array()
            .unwrap_or_else(|| panic!("params.contract.{key} is declared"))
            .iter()
            .map(|l| {
                assert!(
                    !l["because"].as_str().expect("a lane says why").is_empty(),
                    "a lane without a sentence is a lane nobody can wire"
                );
                l["route"].as_str().expect("a lane is a route").to_string()
            })
            .collect()
    };
    assert_eq!(lanes("accepts"), vec!["in_request", "in_invoke"]);
    assert_eq!(lanes("emits"), vec!["grant", "ack", "connect", "error"]);
    // Requirement 2: not one of them is the name of a cell in here.
    for lane in lanes("accepts").iter().chain(lanes("emits").iter()) {
        for cell in ["policy", "invoke", "store", "sweep", "clock", "vault"] {
            assert_ne!(lane, cell, "'{lane}' is a cell of this hive, not a lane");
        }
    }

    // Requirement 3: the mapping lane -> cell exists exactly once, and it is an
    // edge of this hive's own graph.
    let edges = cfg["params"]["graph"]["edges"].as_array().unwrap();
    let door_to = |lane: &str| -> Vec<&str> {
        edges
            .iter()
            .filter(|e| {
                e["from"] == "."
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("'{lane}'")))
            })
            .map(|e| e["to"].as_str().unwrap())
            .collect()
    };
    assert_eq!(door_to("in_request"), vec!["./policy"]);
    assert_eq!(door_to("in_invoke"), vec!["./invoke"]);
    assert_eq!(
        edges.iter().filter(|e| e["from"] == ".").count(),
        2,
        "two doors, one per accepted lane: {edges:?}"
    );
}

/// And the seal is the substrate's, not this test's opinion of it: the real
/// port-boundary validation refuses a mutation that reaches inside, and admits
/// the hive path itself.
#[test]
fn a_mutation_reaching_inside_the_broker_is_refused_by_the_real_validator() {
    use meclaw_colony::config::HiveParams;
    use meclaw_colony::mutation::port_boundary::{SealedHive, validate_hive_port_boundary};

    let Some(root) = shipped_access() else {
        return;
    };
    let params: HiveParams =
        meclaw_core::serde_json::from_value(read_json(&root.join("config.json"))["params"].clone())
            .expect("the shipped params parse as HiveParams");
    let sealed = vec![SealedHive {
        path: "/access".to_string(),
        ports: params.ports.clone().expect("declared"),
    }];

    // The store was a declared port in 1.x. It is the one that mattered.
    for endpoint in ["./access/store", "./access/policy", "./access/invoke"] {
        let err = validate_hive_port_boundary(
            &json!({"add_edges": [{"from": "./agent", "to": endpoint}]}),
            "/",
            &sealed,
        )
        .expect_err("an interior endpoint must be refused");
        assert_eq!(err.error_code(), "hive_port_boundary", "for {endpoint}");
    }

    validate_hive_port_boundary(
        &json!({"add_edges": [
            {"from": "./agent", "to": "./access",
             "modifier": {"set_hop": {"route": "'in_request'"}}},
            {"from": "./access", "to": "./agent",
             "condition": "has(hop.route) && hop.route == 'grant'"}
        ]}),
        "/",
        &sealed,
    )
    .expect("the documented wiring names the hive and must stay legal");
}

/// GH #307 — an interior edge that carries a route no cell in this hive ever
/// emits is dead wiring, and dead wiring reads as a channel that carries
/// something.
///
/// `access@2.0.0` shipped `./invoke -> ./vault` on `hop.route == 'vault'` plus
/// the reply edge. `invoke`'s script calls `emit()` with four literal routes
/// (`astore`, `ack`, `error`, `connect`), none of them computed, so neither edge
/// could ever fire; the hive is sealed (`params.ports` is empty), so `./vault`
/// was unaddressable from anywhere else too. The vault does not need them —
/// `meclaw_cells::vault::attest`'s `a_vault_with_no_inbound_edges_attests` is
/// the pin that removing them changes no behaviour, and the credential reaches
/// the connector by the late-bound `.env` path the README documents.
///
/// The assertion is the general one rather than a ban on the string `vault`:
/// for every edge leaving a CELL of this hive on a `hop.route` comparison, the
/// route has to appear as a literal in that cell's own script. A door edge
/// (`from: "."`) is exempt — its route is minted outside, by the caller's edge.
#[test]
fn every_route_edge_out_of_a_cell_names_a_route_that_cell_emits() {
    let Some(root) = shipped_access() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    let edges = cfg["params"]["graph"]["edges"].as_array().unwrap();

    let mut checked = 0usize;
    for e in edges {
        let Some(src) = e["from"].as_str().and_then(|f| f.strip_prefix("./")) else {
            continue; // the hive's own door: the route was minted outside
        };
        let cond = e["condition"].as_str().unwrap_or_default();
        for route in route_literals(cond) {
            let src_cfg = read_json(&root.join(src).join("config.json"));
            let script = src_cfg["params"]["script_inline"]
                .as_str()
                .unwrap_or_else(|| panic!("./{src} carries a route edge but runs no script"));
            assert!(
                script.contains(&format!("\"{route}\"")),
                "./{src} -> {} fires on hop.route == '{route}', and ./{src} never \
                 emits that route: an edge nothing can traverse",
                e["to"]
            );
            checked += 1;
        }
    }
    assert!(checked >= 7, "the route-edge sweep found almost nothing");
}

/// Every `hop.route == '<x>'` comparison in a CEL condition, in order.
fn route_literals(cond: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = cond;
    while let Some(at) = rest.find("hop.route == '") {
        rest = &rest[at + "hop.route == '".len()..];
        match rest.find('\'') {
            Some(end) => {
                out.push(rest[..end].to_string());
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// GH #307 — the policy store's write surface has two halves, this template
/// shipped neither, and exactly ONE of them can be closed here.
///
/// `contract.write_surface` (GH #260) bounds the `import` of the `transfer` body
/// slot, which the SUBSTRATE answers before `handle()` is ever reached. An
/// absent key means `open`, and `open` bounds nothing — `meclaw_colony`'s
/// `an_open_write_surface_bounds_no_import_at_all` is the negative pin. Without
/// it an `import` writes `policy` rows in bulk, from any sender, straight past
/// every comparison this hive is built on. It is declared.
///
/// `params.write_surface` (GH #132) bounds the ops the store's own `handle()`
/// runs, and it is deliberately NOT declared — which is the second half of what
/// this test pins, because an omission that is not asserted reads as an
/// oversight. The reason is a property of the template: **nothing inside the
/// hive ever writes `policy` or `cred_refs`**. The three `code` cells only
/// `select` from those two tables; what they write is `grants`, `grant_events`,
/// `usage` and `audit`. Enabling a rule is the operator's gesture and comes from
/// outside the scope by construction (the `PROBE` above plays exactly that), so
/// a cell-level seal would not tighten the boundary — it would leave a freshly
/// instantiated broker inert forever, with no path to ever turn a rule on.
///
/// The `select`-only assertion below is what makes this revisable rather than a
/// standing excuse: the day a cell in here writes `policy`, it goes red and the
/// seal becomes possible.
///
/// The three `code` cells declare no contract half on purpose — this hive keeps
/// a lane's state on the wire (the store round trip IS its memory), so their
/// `cell.db` holds nothing a boundary would protect.
#[test]
fn the_policy_store_bounds_the_import_and_says_why_the_cell_surface_stays_open() {
    let Some(root) = shipped_access() else {
        return;
    };
    let store = read_json(&root.join("store/config.json"));
    assert_eq!(
        store["contract"]["write_surface"], "internal",
        "GH #260: without the substrate half an import writes policy rows past \
         every comparison this hive is built on"
    );
    assert!(
        store["params"].get("write_surface").is_none(),
        "GH #132 stays open here on purpose: the operator who enables a rule is \
         outside this scope, and a sealed handle() would leave the broker inert"
    );

    // And that reason, asserted rather than asserted-once-in-prose: every touch
    // of `policy` or `cred_refs` inside the hive is a read.
    let mut reads = 0usize;
    for rel in [
        "policy/config.json",
        "invoke/config.json",
        "sweep/config.json",
    ] {
        let cfg = read_json(&root.join(rel));
        let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
        for line in script.lines() {
            for table in ["policy", "cred_refs"] {
                if line.contains(&format!("table=\"{table}\"")) {
                    assert!(
                        line.contains("\"select\""),
                        "{rel} touches `{table}` with something other than a \
                         select -- this hive now HAS an internal writer, so \
                         params.write_surface can and should be sealed: {line}"
                    );
                    reads += 1;
                }
            }
        }
    }
    assert!(reads >= 1, "the select-only sweep found nothing to check");
    for rel in [
        "policy/config.json",
        "invoke/config.json",
        "sweep/config.json",
    ] {
        let cfg = read_json(&root.join(rel));
        assert!(
            cfg["contract"].get("write_surface").is_none(),
            "{rel} is a code cell whose cell.db this template never uses; a \
             boundary around it would be decoration, not a promise"
        );
    }
}

/// GH #307, the same boundary proved at runtime rather than at the declaration:
/// a `transfer` `import` addressed straight at the policy store writes no row.
///
/// This is the half that made the omission load-bearing. The slot is answered by
/// the SUBSTRATE in `cell_task`, before the `consumes` gate and before
/// `handle()` — so it walks past everything the hive checks, and it writes in
/// bulk. The message below carries no sender at all, which the rule treats as
/// outside (fail-closed), and it plants an ENABLED rule granting `chat.send` to
/// a requester nobody ever brokered. With `contract.write_surface` absent it
/// lands; with `"internal"` it is refused with `write_denied` before the first
/// row.
///
/// The evidence is the store's own content, read back through the probe: an
/// import that was refused is invisible in every other way (its reply matches no
/// out-edge of the store, because a source message carries no `access_origin`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_import_from_outside_plants_no_policy_row() {
    let Some(root) = shipped_access() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    let count = |v: &Value| v.as_array().map(|a| a.len()).unwrap_or(0);
    let read = json!({"operation": "select", "table": "policy",
                      "columns": ["rule_id", "enabled"], "limit": 50});
    let before = probe(&h, &mut rx, read.clone()).await;

    h.send(
        MessageBuilder::new(Path::new("/access/store"))
            .body(Body::Inline(json!({"transfer": {
                "operation": "import",
                "table": "policy",
                "key": ["rule_id"],
                "schema": {
                    "rule_id": "text", "requester": "text", "capability": "text",
                    "subject": "text", "scope_match": "json", "verdict": "text",
                    "max_ttl_ms": "int", "constraints": "json", "cred_ref": "text",
                    "enabled": "int", "priority": "int", "note": "text"
                },
                "rows": [{
                    "rule_id": "smuggled", "requester": "agent:nobody",
                    "capability": "chat.send", "subject": "member:example",
                    "scope_match": {"channel": "example-chat", "chat_id": "*",
                                    "actions": ["send_message"]},
                    "verdict": "allow", "max_ttl_ms": 900000,
                    "constraints": {}, "cred_ref": "cred:example-chat:primary",
                    "enabled": 1, "priority": 1, "note": "planted"
                }]
            }})))
            .ttl(400)
            .build(),
    )
    .await;
    // The import travels ONE hop; the read below travels two. The wait is the
    // discriminator, not the ordering: without it a green result would only mean
    // the import had not arrived yet.
    tokio::time::sleep(Duration::from_millis(600)).await;

    let after = probe(&h, &mut rx, read).await;
    assert_eq!(
        count(&after),
        count(&before),
        "an import from outside the scope planted a policy row: {after}"
    );
    assert!(
        after
            .as_array()
            .is_none_or(|a| a.iter().all(|r| r["rule_id"] != "smuggled")),
        "the planted rule is in the policy table: {after}"
    );

    h.shutdown().await;
}
