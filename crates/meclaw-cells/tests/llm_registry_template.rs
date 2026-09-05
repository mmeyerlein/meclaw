//! meclaw-os -- the shipped `llm-registry@1` template: the catalog behind the
//! tier names, plus a write hand that moves them ON CALL (V8 spec § 3, ruling L6
//! of 2026-08-15).
//!
//! What is pinned here is what the template PROMISES, in the order the README
//! promises it:
//!
//! 1. **The inventory, and the absence of a clock.** Three cells and a hive,
//!    no `timer`, no `schedules`, no cron string anywhere -- and a booted tree
//!    that says nothing at all until it is asked. v1 is the catalog plus an
//!    on-call hand; the closed loop is the target picture (GH #130), not
//!    this release, and an absence that is not pinned arrives by accident.
//! 2. **Resolution is deterministic.** The same request over the same catalog
//!    resolves to the same model twice, because the rank ends on a unique
//!    column -- and each round leaves one `resolutions` line.
//! 3. **A refusal is not a guess.** Requirements nothing satisfies come back
//!    `resolved: false` with `model_id: ""` -- no nearest match, no default --
//!    and a named tier whose model cannot serve the request is refused rather
//!    than silently replaced.
//! 4. **The hand moves on call, and reaches exactly the right cells.** One
//!    remap command supersedes the tier row and pushes `{system:{},
//!    params:{model}}` at every UNPINNED subscriber of that tier. The push is
//!    proved on the WIRE: the receivers are real `llm` cells against a mock
//!    provider, the params message triggers no provider call at all (accepted
//!    and silent -- the 202 form), and the next inference of each cell shows
//!    which ones actually moved.
//! 5. **`incidents` is a journal.** An incident row changes no other table,
//!    does not move the tier index, does not alter the next resolution, and
//!    emits nothing. That is the ruling: no automation crept in.
//!
//! Free of a paid call by construction: the only provider on the wire is the
//! in-process mock, and the registry itself holds no model.
//!
//! **R2b guard (GH #49 form).** `llm-registry@1` is PRIVATE -- it is not in
//! `PUBLIC_TEMPLATES`, so it does not travel with the export. Every read below
//! is guarded per file by [`shipped_registry`]; in the public clone the guard
//! exits cleanly and these tests skip instead of failing on a dead
//! `templates/` reference. Same form as `affinity_template.rs`, and the
//! matching `ALLOWED_HITS` entry lives in the maintainers' export script.

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
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every cell the hive is made of. The list is the guard AND the inventory: a
/// cell that silently appears or disappears is caught by the set comparison in
/// [`the_hive_carries_three_cells_and_no_clock`].
const REGISTRY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "select/config.json",
    "hand/config.json",
];

const REGISTRY_SEEDS: &[&str] = &["store/seed/models.jsonl", "store/seed/tiers.jsonl"];

/// The template root, or `None` where it does not ship (the documented R2b
/// exception form, GH #49).
fn shipped_registry() -> Option<std::path::PathBuf> {
    let root = templates_root().join("llm-registry");
    for rel in REGISTRY_FILES.iter().chain(REGISTRY_SEEDS) {
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

/// The asking side of the read port: one request in, the documented `tool_call`
/// turn out, and WHO is asking declared on the hop -- which the port edge then
/// promotes to `context.asker`. `select` never reads an asker out of a body.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "select", "asker": "member:alex"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "s1",
                  "text": raw}]}))
"#;

/// The commanding side of the hand port. The actor rides the hop so the port
/// edge can make it edge truth; `ACTOR` in the raw request switches it off, so
/// the refusal path has a way in.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    cmd = json.loads(raw)
except Exception:
    cmd = {}
actor = "" if cmd.pop("_no_actor", False) else "member:alex"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": actor},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                  "text": json.dumps(cmd)}]}))
"#;

/// The maintenance cell at the far end of the BOOT-GRAPH edge into `./store`
/// (GH #310). It is not a port and not a bypass: `params.ports` is empty, so no
/// runtime mutation could ever draw this edge (`hive_port_boundary`) -- but the
/// bootstrap is deliberately outside that check, and the birth topology of the
/// parent is where `models`, `subscribers` and `incidents` come from, because no
/// lane of the hive writes them. The test uses the same edge to read.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1",
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
            "purpose": "Test stand-in around the shipped llm-registry template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A subscriber: a REAL `llm` cell pointed at the in-process mock. Using the
/// real cell is the whole point of round 4 -- a capture cell would prove that a
/// message was shaped right, and this proves that the shape does what the
/// README says it does.
fn llm_cell(base_url: &str, model: &str) -> Value {
    json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai", "model": model,
            "api_key": "test-key-lr", "base_url": base_url
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "meta": {"type": "object", "required": false}},
                "hop": {"finish_reason": {"type": "string", "required": true}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false},
                                  "system": {"type": "object", "required": false}}},
            "capabilities": ["network:llm", "db:own"]
        },
        "description": {
            "purpose": "A subscribed brain, standing in for any llm cell a tier reaches.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ─────────────────────────────────────────────────────────────── the topology

/// The ports around the hive -- every one a literal copy of what
/// `templates/llm-registry/README.md` documents. The template draws no edge
/// that appears here.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── in_select: the asker's identity becomes EDGE truth, and only here ──
        {"from": "./asker", "to": "./llm_registry/select",
         "condition": "has(hop.route) && hop.route == 'select'",
         "modifier": {"set_context": {"asker": "hop.asker"}}},
        // ── out_select ──
        {"from": "./llm_registry/select", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // ── in_hand: likewise, and a command without one is refused ──
        {"from": "./operator", "to": "./llm_registry/hand",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_context": {"actor": "hop.actor"}}},
        // ── out_ack ──
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        // ── out_update: ONE edge per subscriber, by hand. That is the honest
        //    cost of a substrate in which cells cannot be enumerated. ──
        {"from": "./llm_registry/hand", "to": "./sub_a",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_a'"},
        {"from": "./llm_registry/hand", "to": "./sub_b",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_b'"},
        {"from": "./llm_registry/hand", "to": "./sub_c",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_c'"},
        // ── the test's own observer of the push, so its BODY can be read ──
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'update'"},
        // ── out_error: the drain the parent MUST wire, both code cells ──
        {"from": "./llm_registry/select", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── the BOOT-GRAPH edge into ./store and its reply: not a port of the
        //    hive (params.ports is empty) and not drawable by a mutation, but
        //    the only way models/subscribers/incidents are ever written ──
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/sink",
         "condition": "context.registry_origin == 'admin'"},
        // ── what the subscribers answer, so an inference is observable ──
        {"from": "./sub_a", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_b", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_c", "to": "/sink", "condition": "has(hop.finish_reason)"}
    ]}}})
}

/// The models the three subscribers are BORN on. Distinct on purpose: after a
/// remap, the model on the wire is the only thing that says which cell moved.
const BIRTH_A: &str = "birth/sub-a";
const BIRTH_B: &str = "birth/sub-b";
const BIRTH_C: &str = "birth/sub-c";
/// What the `mid` tier is remapped to. It is a seeded, ACTIVE catalog row.
const REMAP_TO: &str = "provider-b/model-large";

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, base_url: &str) {
    let root = td.path();
    // No `.env`. Until `llm-registry@2.1.0` this wrote
    // `LLM_REGISTRY_SUBSCRIBER_ROWS=200` -- the shipped default, spelled out so
    // the fan-out bound was visible in the setup. Since GH #138 (ruling
    // R-0904-6) the bound is `params.subscriber_rows` of `./hand`, and such a
    // line would be read by nothing and would say nothing about it. The copied
    // template carries the value, and
    // `crates/meclaw-cells/tests/gh138_long_tail_params.rs` pins that the cells
    // act on their params rather than on an environment.
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["select"],
            json!({"asker": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/operator/config.json",
        &code_cell(
            OPERATOR,
            &["command"],
            json!({"actor": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/admin/config.json",
        &code_cell(ADMIN, &["astore"], json!({})),
    );
    write(root, "main/sub_a/config.json", &llm_cell(base_url, BIRTH_A));
    write(root, "main/sub_b/config.json", &llm_cell(base_url, BIRTH_B));
    write(root, "main/sub_c/config.json", &llm_cell(base_url, BIRTH_C));
    copy_cells(root_template, &root.join("main/llm_registry"));
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
            ("llm".to_string(), Arc::new(LlmCellFactory)),
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

/// The next message matching `want`, skipping whatever else the sink collects.
async fn recv_matching(
    rx: &mut mpsc::Receiver<Message>,
    label: &str,
    want: impl Fn(&Message) -> bool,
) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("nothing more arrived while waiting for {label}; saw {seen:?}");
        });
        if want(&m) {
            return m;
        }
        seen.push(format!("{:?}: {}", m.headers.hop, turn_text(&m)));
    }
    panic!("{label} never arrived; saw {seen:?}");
}

async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let owned = route.to_string();
    recv_matching(rx, route, move |m| hop_of(m, "route") == owned).await
}

/// One store op over the boot-graph edge into `./store`, returned as its rows.
async fn admin(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(to(
        "/admin",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let m = recv_matching(rx, "admin answer", |m| !hop_of(m, "operation").is_empty()).await;
    turn_json(&m)
}

/// One lookup through the read port, returned as the resolution payload.
async fn resolve(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, req: Value) -> Value {
    h.send(to(
        "/asker",
        &meclaw_core::serde_json::to_string(&req).unwrap(),
    ))
    .await;
    turn_json(&recv_route(rx, "answer").await)
}

/// One inference on a subscriber, answered as the model that served it. This is
/// the only place the wire says which cell a remap actually moved.
async fn inference_model(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    cell: &str,
    mock: &MockOpenAI,
) -> String {
    let before = mock.recorded_requests().await.len();
    h.send(to(cell, "ping")).await;
    let _ = recv_matching(rx, "an inference answer", |m| {
        !hop_of(m, "finish_reason").is_empty()
    })
    .await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        before + 1,
        "exactly one provider call per inference"
    );
    snaps[before].model().unwrap_or_default().to_string()
}

fn now_iso() -> &'static str {
    "2026-08-15T00:00:00Z"
}

/// The three subscriber rows: two on `mid`, one of them pinned, and one on
/// `light`. Written over the boot-graph edge, because NO lane of this hive
/// writes `subscribers` -- `select` and `hand` only read it, and the seed does
/// not carry it either (GH #310). This is the one path there is.
async fn wire_subscribers(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) {
    for (path, tier, pinned) in [
        ("/sub_a", "mid", 0),
        ("/sub_b", "mid", 1),
        ("/sub_c", "light", 0),
    ] {
        admin(
            h,
            rx,
            json!({"operation": "insert", "table": "subscribers",
                   "row": {"cell_path": path, "tier": tier, "pinned": pinned,
                           "wired_at": now_iso()}}),
        )
        .await;
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Three cells and a hive, and NO clock. Pinned as a set, because the shape is
/// the ruling: v1 is the catalog plus a write hand on call. A `timer` here, a
/// `schedules` block, or a cron string in any config would be the control loop
/// arriving by accident -- and the loop is GH #130's business, not v1's.
#[test]
fn the_hive_carries_three_cells_and_no_clock() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mut found = Vec::new();
    collect_configs(&root, &root, &mut found);
    let mut found: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    let mut want: Vec<String> = REGISTRY_FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        found, want,
        "llm-registry@1 is store + select + hand: no probe, no clock"
    );
    for rel in REGISTRY_FILES {
        let cfg = read_json(&root.join(rel));
        let ty = cfg["cell"]["type"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            ty, "llm",
            "{rel} is an llm cell -- the registry that repairs a model holds none"
        );
        assert_ne!(
            ty, "timer",
            "{rel} is a timer -- v1 has no tick, and that is the ruling"
        );
        let raw = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(
            !raw.contains("schedules"),
            "{rel} declares schedules -- nothing in v1 fires on its own"
        );
        assert!(
            !raw.contains("cron"),
            "{rel} names a cron -- nothing in v1 fires on its own"
        );
    }
}

/// The other half of the same ruling, at runtime: a booted registry with a full
/// catalog and three wired subscribers says NOTHING until somebody asks. No
/// probe tick, no health sweep, no unsolicited push.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_booted_registry_emits_nothing_on_its_own() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    wire_subscribers(&h, &mut rx).await;

    // Long enough that any plausible tick would have fired -- and the shortest
    // cron this substrate can express is one second.
    let quiet = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "an idle registry must emit nothing at all, got {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "and it must reach no provider: this hive holds no model"
    );

    h.shutdown().await;
}

/// The read port, resolved twice. The claim is not that some model comes back
/// -- it is that the SAME one comes back, because the rank ends on `model_id`
/// and no two active rows share it. And each round leaves its journal line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_resolves_the_same_way_twice_and_is_journalled() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    // 1. A tier is an INDEX LOOKUP: the answer is the row the index points at,
    //    with the catalog facts that make it usable without a second question.
    let by_tier = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(by_tier["resolved"].as_bool(), Some(true), "{by_tier}");
    assert_eq!(by_tier["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(by_tier["reason_code"].as_str(), Some("tier_active"));
    assert_eq!(by_tier["provider"].as_str(), Some("gateway"));
    assert_eq!(by_tier["wire_dialect"].as_str(), Some("chat_completions"));
    assert_eq!(
        by_tier["cost_out"].as_i64(),
        Some(400),
        "the price rides along as a CENT INTEGER, which is why it can be ordered"
    );

    // 2. Without a tier it is a ranked search -- and the rank is part of the
    //    answer, so a caller can see WHY this row won.
    let ranked = resolve(
        &h,
        &mut rx,
        json!({"capability": ["tools", "vision"], "max_cost": 500}),
    )
    .await;
    assert_eq!(ranked["resolved"].as_bool(), Some(true), "{ranked}");
    assert_eq!(
        ranked["model_id"].as_str(),
        Some("provider-a/model-mid"),
        "the cheapest ACTIVE model that has both capabilities: {ranked}"
    );
    assert_eq!(ranked["reason_code"].as_str(), Some("ranked"));
    assert_eq!(
        ranked["rank"].as_str(),
        Some("cost_out asc, cost_in asc, context_window desc, model_id asc")
    );

    // 3. The same request again. This is the determinism claim, and it is the
    //    reason `select` holds no model: a comparison repeats, a judgement does
    //    not.
    let again = resolve(
        &h,
        &mut rx,
        json!({"capability": ["tools", "vision"], "max_cost": 500}),
    )
    .await;
    assert_eq!(
        again["model_id"], ranked["model_id"],
        "the same request over the same catalog must resolve identically"
    );
    assert_eq!(again["reason_code"], ranked["reason_code"]);

    // 4. The retired row is the CHEAPEST in the seed, and it was never in the
    //    running: `status` is a filter, not a hint.
    let cheapest = resolve(&h, &mut rx, json!({})).await;
    assert_eq!(cheapest["resolved"].as_bool(), Some(true), "{cheapest}");
    assert_ne!(
        cheapest["model_id"].as_str(),
        Some("provider-b/model-legacy"),
        "a retired model must never be resolved, however cheap it is"
    );

    // 5. Four lookups, four journal lines -- and the asker on each of them is
    //    the one the EDGE promoted, not one a body claimed.
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["tier", "model_id", "cell_path", "reason"],
               "order_by": [{"col": "at", "dir": "asc"}], "limit": 50}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 4, "one line per lookup: {rows:?}");
    assert!(
        rows.iter()
            .all(|r| r["cell_path"].as_str() == Some("member:alex")),
        "the journal names the asker the edge wrote: {rows:?}"
    );
    assert_eq!(rows[0]["reason"].as_str(), Some("tier_active"));
    assert_eq!(rows[1]["reason"].as_str(), Some("ranked"));

    h.shutdown().await;
}

/// The refusal, in both of its shapes. Nothing satisfiable comes back with a
/// reason code and NO model id; and a named tier whose model cannot serve the
/// request is refused rather than quietly swapped for one that can -- which
/// would move a cell off the tier its operator chose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsatisfiable_requirements_are_refused_and_never_guessed() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    // 1. A budget nothing meets. Not the nearest model, not the cheapest, not a
    //    default -- an empty model id.
    let broke = resolve(&h, &mut rx, json!({"capability": "vision", "max_cost": 1})).await;
    assert_eq!(broke["resolved"].as_bool(), Some(false), "{broke}");
    assert_eq!(broke["reason_code"].as_str(), Some("no_candidate"));
    assert_eq!(
        broke["model_id"].as_str(),
        Some(""),
        "a refusal names NO model at all: {broke}"
    );
    assert!(
        broke["considered"].as_i64().unwrap_or(0) > 0,
        "and it looked -- the refusal is a result, not an empty catalog: {broke}"
    );

    // 2. A tier that exists, and a requirement its model does not meet. The
    //    catalog HAS a model with vision under this ceiling; it is not offered.
    let below = resolve(
        &h,
        &mut rx,
        json!({"tier": "light", "capability": "vision"}),
    )
    .await;
    assert_eq!(below["resolved"].as_bool(), Some(false), "{below}");
    assert_eq!(
        below["reason_code"].as_str(),
        Some("tier_below_requirement"),
        "a named tier is a lookup, not a search: {below}"
    );
    assert_eq!(below["model_id"].as_str(), Some(""));
    assert_eq!(
        below["detail"].as_str(),
        Some("capability_missing"),
        "and the refusal says WHICH requirement failed: {below}"
    );

    // 3. A tier nobody has decided about.
    let unknown = resolve(&h, &mut rx, json!({"tier": "does-not-exist"})).await;
    assert_eq!(unknown["resolved"].as_bool(), Some(false), "{unknown}");
    assert_eq!(unknown["reason_code"].as_str(), Some("unknown_tier"));

    // 4. Refusals are journalled too. A log that only records successes cannot
    //    answer "why did nothing get a model last Tuesday".
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["model_id", "reason"], "limit": 50}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 3, "three refusals, three lines: {rows:?}");
    assert!(
        rows.iter().all(|r| r["model_id"].as_str() == Some("")),
        "a refused line carries no model either: {rows:?}"
    );

    h.shutdown().await;
}

/// The write hand, on call. One command, and the wire shows all three halves of
/// the promise: the message form an `llm` cell accepts in silence, the tier
/// index superseded, and exactly the unpinned subscribers of that tier moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remap_command_pushes_params_to_exactly_the_unpinned_subscribers() {
    let Some(root) = shipped_registry() else {
        return;
    };
    // Three inferences, one per subscriber, and not one more: the params push
    // itself must reach no provider.
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("a", "stop"),
        canned_chat_completion("b", "stop"),
        canned_chat_completion("c", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": REMAP_TO,
            "reason": "provider-a degraded"
        }))
        .unwrap(),
    ))
    .await;

    // 1. The push, as the observer sees it: exactly ONE update message, and it
    //    is addressed at the unpinned mid subscriber. The pinned one and the
    //    light one produce no message at all.
    let pushed = recv_route(&mut rx, "update").await;
    assert_eq!(
        hop_of(&pushed, "subscriber"),
        "/sub_a",
        "the push names its subscriber, which is how the parent's edge finds \
         the llm cell: {:?}",
        pushed.headers.hop
    );
    assert_eq!(hop_of(&pushed, "tier"), "mid");
    assert_eq!(hop_of(&pushed, "model_id"), REMAP_TO);

    // 2. The FORM. An empty `system` slot, a `params` overlay, and no
    //    `messages[]` -- which is precisely what makes an llm cell persist the
    //    overlay and answer with nothing: accepted, silent, free.
    let body = body_of(&pushed);
    assert_eq!(
        body["params"]["model"].as_str(),
        Some(REMAP_TO),
        "the overlay carries the model and nothing else: {body}"
    );
    assert!(
        body["system"].is_object() && body["system"].as_object().is_some_and(|o| o.is_empty()),
        "the system slot is present and EMPTY: {body}"
    );
    assert!(
        body.get("messages").is_none(),
        "no messages slot -- a turn here would cost a provider call: {body}"
    );
    assert!(
        !body.as_object().is_some_and(|o| o.contains_key("provider"))
            && !body.as_object().is_some_and(|o| o.contains_key("api_key")),
        "and the overlay never touches the immutable auth dimension: {body}"
    );

    // 3. The acknowledgement, which is emitted AFTER the pushes -- so by the
    //    time it reaches this sink, every update is already in its mailbox.
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("accepted"), "{ack}");
    assert_eq!(ack["pushed"].as_i64(), Some(1), "{ack}");
    assert_eq!(
        ack["skipped_pinned"].as_i64(),
        Some(1),
        "the pinned subscriber is counted, not silently dropped: {ack}"
    );
    assert_eq!(
        ack["decided_by"].as_str(),
        Some("member:alex"),
        "the decider is edge truth: {ack}"
    );

    // 4. Nothing reached a provider. The whole remap cost zero tokens.
    let calls: Vec<String> = mock
        .recorded_requests()
        .await
        .iter()
        .map(|r| r.model().unwrap_or_default().to_string())
        .collect();
    assert!(
        calls.is_empty(),
        "a params push must not trigger inference: {calls:?}"
    );

    // 5. What each cell RUNS now, read off the wire one inference at a time.
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &mock).await,
        REMAP_TO,
        "the unpinned mid subscriber moved"
    );
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_b", &mock).await,
        BIRTH_B,
        "the PINNED mid subscriber did not: a pin is a decision written down"
    );
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_c", &mock).await,
        BIRTH_C,
        "and another tier is another question entirely"
    );

    // 6. The index moved by supersede, not by overwrite: what `mid` used to
    //    mean is still readable.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["tier", "model_id", "active", "decided_by"],
               "where": {"tier": "mid"},
               "order_by": [{"col": "active", "dir": "desc"}], "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 2, "the old row is still there: {tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some(REMAP_TO));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));
    assert_eq!(tiers[0]["decided_by"].as_str(), Some("member:alex"));
    assert_eq!(tiers[1]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[1]["active"].as_i64(), Some(0));

    // 7. And the journal separates the two outcomes by name.
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["cell_path", "model_id", "reason"],
               "where": {"reason": {"in": ["hand_remap", "hand_skipped_pinned"]}},
               "order_by": [{"col": "cell_path", "dir": "asc"}], "limit": 20}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 2, "one line per mid subscriber: {rows:?}");
    assert_eq!(rows[0]["cell_path"].as_str(), Some("/sub_a"));
    assert_eq!(rows[0]["reason"].as_str(), Some("hand_remap"));
    assert_eq!(rows[1]["cell_path"].as_str(), Some("/sub_b"));
    assert_eq!(rows[1]["reason"].as_str(), Some("hand_skipped_pinned"));

    h.shutdown().await;
}

/// The two ways a remap is refused, and both leave the world untouched. A
/// command with no actor is the interesting one: a remap changes what other
/// cells run, so it needs an identity an edge wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remap_without_an_actor_or_onto_an_unknown_model_is_refused() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    // 1. No actor on the edge.
    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": REMAP_TO, "_no_actor": true
        }))
        .unwrap(),
    ))
    .await;
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("rejected"), "{ack}");
    assert_eq!(ack["reason_code"].as_str(), Some("no_actor"), "{ack}");

    // 2. A model the catalog does not carry as active -- the retired one is the
    //    sharper case: the row EXISTS, and it is still not a legal target.
    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": "provider-b/model-legacy"
        }))
        .unwrap(),
    ))
    .await;
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("rejected"), "{ack}");
    assert_eq!(ack["reason_code"].as_str(), Some("unknown_model"), "{ack}");

    // 3. The index never moved, and nothing was pushed at anybody.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["model_id", "active"], "where": {"tier": "mid"},
               "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 1, "a refusal writes no index row: {tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &mock).await,
        BIRTH_A,
        "and the subscriber is still on the model it was born with"
    );

    h.shutdown().await;
}

/// `incidents` is a journal and nothing more. This is the ruling of 2026-08-15
/// pinned as behaviour: a row lands, and the world does not move. The day
/// somebody wires an incident rule, this test is what notices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incident_row_changes_nothing_and_triggers_nothing() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("a", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    let before = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(before["model_id"].as_str(), Some("provider-a/model-mid"));

    // The worst incident the schema can express, against the very model the
    // `mid` tier points at.
    admin(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "incidents",
               "row": {"id": "inc-1", "model_id": "provider-a/model-mid",
                       "kind": "outage", "at": now_iso(),
                       "detail": {"note": "provider returned 503 for ten minutes"}}}),
    )
    .await;

    // 1. Nothing leaves the hive. No auto-remap, no push, no alert.
    let quiet = tokio::time::timeout(Duration::from_secs(4), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "an incident row must trigger NOTHING in v1, got {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    // 2. The tier index is untouched -- still one active row, still the model
    //    the incident was filed against.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["model_id", "active"], "where": {"tier": "mid"},
               "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 1, "{tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));

    // 3. And `models` is untouched too: an incident does not degrade a status.
    //    Deciding that a model is degraded is a human act, and it looks like an
    //    UPDATE somebody made on purpose.
    let models = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "models",
               "columns": ["status"], "where": {"model_id": "provider-a/model-mid"},
               "limit": 5}),
    )
    .await;
    assert_eq!(models[0]["status"].as_str(), Some("active"), "{models}");

    // 4. The next lookup answers exactly as the first one did.
    let after = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(
        after["model_id"], before["model_id"],
        "an incident is a line in a log, not an input to the resolver"
    );
    assert_eq!(after["reason_code"], before["reason_code"]);

    // 5. And the subscriber never heard a thing.
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &mock).await, BIRTH_A);

    h.shutdown().await;
}

/// GH #310 — the catalog store's write surface has two halves, this template
/// shipped neither, and exactly ONE of them can be closed here.
///
/// `contract.write_surface` (GH #260) bounds the `import` of the `transfer`
/// body slot, which the SUBSTRATE answers before `handle()` is ever reached. An
/// absent key means `open`, and `open` bounds nothing — `meclaw_colony`'s
/// `an_open_write_surface_bounds_no_import_at_all` is the negative pin. Without
/// it an `import` writes `models` rows in bulk, from any sender, straight past
/// the comparison that `select` IS: the rank literal, the `status` column and
/// the refusal. It is declared.
///
/// `params.write_surface` (GH #132) bounds the ops the store's own `handle()`
/// runs, and it is deliberately NOT declared — which is the second half of what
/// this test pins, because an omission that is not asserted reads as an
/// oversight. The reason is a property of the template: **no cell in this hive
/// ever writes `models`, `subscribers` or `incidents`**. `select` and `hand`
/// only select from those three; what they write is `tiers` and `resolutions`.
/// The three operator tables are maintained over a parent's BOOT-graph edge
/// straight into `./store` (the bootstrap is deliberately outside the port
/// seal's scope, `mutation::port_boundary`), and that sender lies outside the
/// hive by construction — [`build_tree`]'s own `./admin -> ./llm_registry/store`
/// edge is exactly it, and it is how `subscribers` and `incidents` get their
/// rows in the tests below. A cell-level seal would not tighten the boundary; it
/// would leave a freshly instantiated registry with no way to ever name a
/// subscriber.
///
/// The two sweeps below are what make this revisable rather than a standing
/// excuse: the day a cell in here writes `models`, `subscribers` or
/// `incidents`, the first goes red and the seal becomes possible — and the day
/// nothing in here writes `tiers`/`resolutions` any more, the second goes red
/// and the hive has stopped being its own writer at all.
#[test]
fn the_catalog_store_bounds_the_import_and_says_why_the_cell_surface_stays_open() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let store = read_json(&root.join("store/config.json"));
    assert_eq!(
        store["contract"]["write_surface"], "internal",
        "GH #260: without the substrate half an import writes catalog rows in \
         bulk past the comparison this hive is built on"
    );
    assert!(
        store["params"].get("write_surface").is_none(),
        "GH #132 stays open here on purpose: models, subscribers and incidents \
         have no writer inside the hive, and a sealed handle() would leave a \
         fresh registry unable to name a single subscriber"
    );

    // Half one: every touch of an operator table inside the hive is a read.
    let mut reads = 0usize;
    // Half two: the hive DOES write its own two tables — which is why the
    // contract half above is true rather than merely harmless.
    let mut writes = 0usize;
    for rel in ["select/config.json", "hand/config.json"] {
        let cfg = read_json(&root.join(rel));
        let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
        for line in script.lines() {
            for table in ["models", "subscribers", "incidents"] {
                if line.contains(&format!("table=\"{table}\"")) {
                    assert!(
                        line.contains("\"select\""),
                        "{rel} touches `{table}` with something other than a \
                         select -- this hive now HAS an internal writer for an \
                         operator table, so params.write_surface can and should \
                         be sealed: {line}"
                    );
                    reads += 1;
                }
            }
            for table in ["tiers", "resolutions"] {
                if line.contains(&format!("table=\"{table}\""))
                    && (line.contains("\"insert\"") || line.contains("\"update\""))
                {
                    writes += 1;
                }
            }
        }
    }
    assert!(reads >= 2, "the select-only sweep found nothing to check");
    assert!(
        writes >= 2,
        "no cell in this hive writes tiers or resolutions any more -- the \
         registry has stopped being its own writer"
    );
}

/// GH #310, the same boundary proved at runtime rather than at the declaration:
/// a `transfer` `import` addressed straight at the catalog store writes no row.
///
/// This is the half that makes the omission load-bearing. The slot is answered
/// by the SUBSTRATE in `cell_task`, before the `consumes` gate and before
/// `handle()` — so it walks past everything this hive is: past `select`'s rank
/// literal, past `hand`'s single op, past the actor the edge has to promote. And
/// it writes in BULK. The message below carries no sender at all, which the rule
/// treats as outside (fail-closed), and it plants two rows that matter more than
/// they look: a `subscribers` row decides who a remap reaches, and an
/// `incidents` row is the field record a human reads before moving a tier.
/// With `contract.write_surface` absent both land; with `"internal"` they are
/// refused with `write_denied` before the first row.
///
/// The evidence is the store's own content, read back over the boot-graph edge:
/// a refused import is invisible in every other way, because the reply to a
/// source message carries no `registry_origin` and therefore matches no out-edge
/// of the store.
///
/// RED receipt, taken by dropping the declaration from the shipped template and
/// running this test: `an import from outside the scope planted a subscriber --
/// the row that decides who a remap reaches: [{"cell_path":"/smuggled",
/// "tier":"strong"}]`. So this pin fails for the reason it names, and not
/// because a shape assertion happened to read `Null`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_import_from_outside_plants_no_subscriber_and_no_incident() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    let count = |v: &Value| v.as_array().map(|a| a.len()).unwrap_or(0);
    let read_subs = json!({"operation": "select", "table": "subscribers",
                           "columns": ["cell_path", "tier"], "limit": 50});
    let read_inc = json!({"operation": "select", "table": "incidents",
                          "columns": ["id", "model_id"], "limit": 50});
    let subs_before = admin(&h, &mut rx, read_subs.clone()).await;
    let inc_before = admin(&h, &mut rx, read_inc.clone()).await;

    for transfer in [
        json!({
            "operation": "import", "table": "subscribers", "key": ["cell_path"],
            "schema": {"cell_path": "text", "tier": "text", "pinned": "int",
                       "wired_at": "text"},
            "rows": [{"cell_path": "/smuggled", "tier": "strong", "pinned": 0,
                      "wired_at": "2026-08-15T00:00:00Z"}]
        }),
        json!({
            "operation": "import", "table": "incidents", "key": ["id"],
            "schema": {"id": "text", "model_id": "text", "kind": "text",
                       "at": "text", "detail": "json"},
            "rows": [{"id": "smuggled", "model_id": "provider-a/model-mid",
                      "kind": "outage", "at": "2026-08-15T00:00:00Z",
                      "detail": {"note": "planted"}}]
        }),
    ] {
        h.send(
            MessageBuilder::new(Path::new("/llm_registry/store"))
                .body(Body::Inline(json!({ "transfer": transfer })))
                .ttl(400)
                .build(),
        )
        .await;
    }
    // The import travels ONE hop; the read below travels two. The wait is the
    // discriminator, not the ordering: without it a green result would only mean
    // the import had not arrived yet.
    tokio::time::sleep(Duration::from_millis(600)).await;

    let subs_after = admin(&h, &mut rx, read_subs).await;
    let inc_after = admin(&h, &mut rx, read_inc).await;
    assert_eq!(
        count(&subs_after),
        count(&subs_before),
        "an import from outside the scope planted a subscriber -- the row that \
         decides who a remap reaches: {subs_after}"
    );
    assert!(
        subs_after
            .as_array()
            .is_none_or(|a| a.iter().all(|r| r["cell_path"] != "/smuggled")),
        "the planted subscriber is in the table: {subs_after}"
    );
    assert_eq!(
        count(&inc_after),
        count(&inc_before),
        "an import from outside the scope planted an incident: {inc_after}"
    );
    assert!(
        inc_after
            .as_array()
            .is_none_or(|a| a.iter().all(|r| r["id"] != "smuggled")),
        "the planted incident is in the field log: {inc_after}"
    );

    h.shutdown().await;
}
