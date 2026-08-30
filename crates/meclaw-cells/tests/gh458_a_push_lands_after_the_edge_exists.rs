//! GH #458 — the mechanism half of the end-to-end promise: an `affinity` push
//! now has somewhere to land.
//!
//! Before `in_pack` the two halves of this sentence were both shipped and did
//! not meet. `affinity` rendered a pack, chose a subscriber and emitted it on
//! its push lane as `system.*` and nothing else; a `talky` was sealed, its
//! `./brain` unreachable by any edge, and its collector dropped `system.*` on
//! every lane it had. The subscription named a brain no graph could reach.
//!
//! This file wires the two together with EXACTLY the two edges the talky
//! README's "door in the wall" section prints — no third edge, no shortcut past
//! a door — and asks whether an identity that was pushed ends up in the
//! subscriber's own durable state. That is the whole issue in one message.
//!
//! The OPERATOR half — who may draw that edge, and what the gate and the broker
//! check before they let it be drawn — is not this file's question; it is asked
//! in `gh458_a_brain_may_only_draw_its_own_push_edge.rs`. Here the edge simply
//! exists, and the question is whether the mechanism behind it works.
//!
//! The second pin is the pairing: `in_pack` and `pack_ack` are one decision, so
//! a mutation that wires the lane without its receipt drain is refused by the
//! substrate's own check — the same shape `gh202_shipped_drain_requirements.rs`
//! measures for the `in_prune` / `prune` pair.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::required_drains::{DrainRequirement, check_required_drains};
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

/// Every file the affinity hive is made of; a missing one makes this file skip
/// rather than fail (R2b / GH #49 — `affinity` is not in `PUBLIC_TEMPLATES`).
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/subscribers.jsonl",
];

fn shipped(name: &str, files: &[&str]) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    files
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The shipped template, copied the way instantiation copies it: `config.json`
/// files, the seed tables next to them, and a `ref` resolved to the tree it
/// names (GH #277).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
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
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
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

// ────────────────────────────────────────────────────────── the test-only cells

/// The writing side, copied in shape from `affinity_template.rs`: the actor and
/// the SUBSCRIBER ride on the hop so the port edge can promote them into
/// context. A subscription names a cell that will be handed somebody's briefs —
/// that address is a routing decision, so it belongs to an edge, never to a body.
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    a = json.loads(raw or "{}")
except Exception:
    a = {}
if not isinstance(a, dict):
    a = {}
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": "member:alex",
               "subscriber": str(a.get("subscriber") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w458",
                  "text": raw}]}))
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
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that subscribes a talky to an affinity.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Where the subscriber lives. It is what `./push` reads out of its own table
/// and what the door edge below matches on. `bootstrap_from_filesystem` roots
/// the tree at `main/`, so the hive `main/talky` answers to `/talky`.
const TALKY: &str = "/talky";

/// The two edges the talky README's "door in the wall" section prints, and the
/// three the affinity side needs to be driven at all. Nothing here names a cell
/// INSIDE either hive: the point of the issue is that the door is the hive path.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── affinity's write port: actor and subscriber become EDGE truth ──
        {"from": "./writer", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'propose'",
         "modifier": {"set_hop": {"route": "'in_propose'"},
                      "set_context": {
                          "actor": "hop.actor",
                          "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'ack' || hop.route == 'error')"},
        // ── THE DOOR. Verbatim the README's form: the push lane of the
        //    affinity HIVE (not `./affinity/push` — its ports are empty too),
        //    told apart from the tool lane by `hop.subscriber`, restamped as
        //    the subscriber's own `in_pack` lane.
        {"from": "./affinity", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == '/talky'",
         "modifier": {"set_hop": {"route": "'in_pack'"}}},
        // ── and the receipt drain the pairing obliges, PLAIN, as documented ──
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'pack_ack'"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'pack_ack'"},
        {"from": "./affinity", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber != '/talky'"}
    ]}}})
}

/// A fixed schedule id. `${uuid7:…}` is an INSTANTIATION-side substitution.
const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000458";
const KEEPER_ID: &str = "0190a3f2-0000-7000-8000-000000000458";
const NEVER: &str = "0 0 0 1 1 *";

fn build_tree(td: &tempfile::TempDir, affinity: &std::path::Path, talky: &std::path::Path) {
    let root = td.path();
    // Two seconds, so the push tick fires several times inside the test's own
    // budget. The cron comes out of the `.env` through the shipped
    // `${AFFINITY_PUSH_CRON:-…}` default, so late binding is under test too.
    std::fs::write(
        root.join(".env"),
        "AFFINITY_PUSH_CRON=*/2 * * * * *\nOPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            &["propose"],
            json!({"actor": {"type": "string", "required": false},
                   "subscriber": {"type": "string", "required": false}}),
        ),
    );
    copy_cells(affinity, &root.join("main/affinity"));
    copy_cells(talky, &root.join("main/talky"));
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
    });
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(KEEPER_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // GH #464 -- the second timer of a shipped composite, and the same two
    // patches for the same two reasons: `${uuid7:*}` is an INSTANTIATION
    // substitution and a tree written straight to disk carries a literal, and a
    // menu tick during a test run would ask a tools hive this colony does not
    // have.
    patch(root, "main/talky/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(KEEPER_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // The brain never has to answer on this lane — a pack costs a write and no
    // inference — but a `base_url` pointing at the real provider is a network
    // call this test would rather refuse than make.
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!("http://127.0.0.1:1/v1");
        v["params"]["model"] = json!("gpt-4o-mock");
    });
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

fn to(target: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next message on `rx` whose `hop.route` matches. 30s is the failure
/// marker convention; several two-second push ticks fit inside it.
async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("no `{route}` arrived within 30s; saw {seen:?}");
        };
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(hop_of(&m, "route"));
    }
}

/// The subscriber's OWN durable state: the `system` table of the talky brain's
/// `cell.db`. Nothing else in this colony can write a row into it.
fn brain_slots(td: &tempfile::TempDir) -> Vec<(String, String)> {
    let p = td.path().join("main/talky/brain/cell.db");
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path, value FROM system ORDER BY slot_path")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The single most important claim of GH #458: `affinity`'s push has somewhere
/// to land. One colony, two shipped hives, the two README edges between them —
/// and an identity that was rendered on one side is durable state on the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_affinity_push_reaches_a_talkys_prompt_through_in_pack() {
    let (Some(affinity), Some(talky)) = (
        shipped("affinity", AFFINITY_FILES),
        shipped("talky", &["config.json", "brain/config.json"]),
    ) else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &affinity, &talky);
    let (h, mut sink, _park) = boot(&td).await;

    // 1. The subscription. The body says WHAT is subscribed to and in how many
    //    slots; WHERE the pushes go comes off the edge. Exactly ONE slot, and
    //    it is one the `in_pack` lane may write: a subscription that asked for
    //    `channel` too would be refused WHOLE at the far door, which is the
    //    all-or-nothing promise doing its job rather than this test failing.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "channel": "telegram", "slots": ["identity"],
                    "subscriber": TALKY});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(&mut sink, "ack").await;
    let outcome = match &ack.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => String::new(),
    };
    assert!(
        outcome.contains("accepted"),
        "the subscription has to exist before the push lane can carry anything: {outcome}"
    );

    // 2. The next tick finds a subscriber whose stored hash is empty — a change
    //    by definition — renders the brief, and the door edge restamps it as
    //    the talky's own `in_pack`. The receipt is the first proof it arrived.
    let receipt = recv_route(&mut sink, "pack_ack").await;
    assert_eq!(
        hop_of(&receipt, "error_code"),
        "",
        "the pushed pack must be ACCEPTED by the door; the receipt says \
         otherwise: {:?}",
        receipt.headers.hop
    );
    assert_eq!(
        hop_of(&receipt, "pack_slots"),
        "identity",
        "the pack the affinity rendered is the pack the door wrote: {:?}",
        receipt.headers.hop
    );
    // The owner is what the SUBSTRATE wrote on the envelope: the cell whose
    // emission this message is, on the affinity side of the subscription. Not
    // the writer that asked for the subscription, and not the talky itself —
    // which is the whole reason the key is read off the envelope and never out
    // of a body. The exact cell behind the affinity's door is that template's
    // business, so the pin is the side, not the sub-path.
    let owner = hop_of(&receipt, "pack_owner");
    assert!(
        owner.starts_with("/affinity"),
        "the receipt must name the AFFINITY side as the pack's owner — the \
         subscription's other end — and it named {owner:?}: {:?}",
        receipt.headers.hop
    );

    // 3. And the proof that outlives the message: the slot is in the
    //    subscriber's own cell.db, which is what its next system prompt is
    //    concatenated from. Polled, because the write happens off the
    //    receipt's thread; 30s is the failure marker.
    let mut slots = Vec::new();
    for _ in 0..1500 {
        slots = brain_slots(&td);
        if slots.iter().any(|(p, _)| p == "identity") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let identity = slots
        .iter()
        .find(|(p, _)| p == "identity")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| {
            panic!(
                "the pushed identity never reached the talky brain's own cell.db; \
                 it holds {slots:?}"
            )
        });
    assert!(
        identity.contains("Alex"),
        "the slot must carry what the affinity DISCLOSED — the pack went \
         through the audience filter and came out readable: {identity:?}"
    );

    h.shutdown().await;
}

/// The pairing. `in_pack` and `pack_ack` are one decision, and the substrate's
/// own check is what enforces it: a mutation that wires the lane without its
/// receipt drain is refused with `required_drain_missing`, the same way the
/// `in_prune` / `prune` pair is refused (`gh202_shipped_drain_requirements.rs`).
///
/// No colony: the question is about a mutation that never commits.
#[test]
fn the_pack_ack_drain_is_required() {
    const HIVE: &str = "/h";
    const CALLER: &str = "/caller";
    const SINK: &str = "/sink";

    let Some(talky) = shipped("talky", &["config.json"]) else {
        return;
    };

    // The talky's `config.json` planted as the hive `/h` of a throwaway root:
    // that file IS the declaration, and reading it per mutation is how a live
    // colony learns what a hive insists on.
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::create_dir_all(root.join("main/h")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::copy(talky.join("config.json"), root.join("main/h/config.json")).unwrap();
    let paths = [Path::new(HIVE)];
    let reqs: Vec<DrainRequirement> =
        meclaw_colony::mutation::required_drains::collect_required_drains(root, paths.iter());

    let pairing = reqs
        .iter()
        .find(|r| format!("{:?}", r.kind).contains("in_pack"))
        .unwrap_or_else(|| {
            panic!(
                "the shipped talky must still declare the in_pack/pack_ack pairing; \
                 the substrate read {reqs:?}"
            )
        });

    let edge = |from: &str, to: &str, condition: Option<&str>| Edge {
        id: meclaw_core::Uuid::now_v7(),
        from: Path::new(from),
        to: Path::new(to),
        condition: condition
            .map(|c| meclaw_colony::cel_eval::parse_condition(c).expect("test condition parses")),
        modifier: None,
        is_default: false,
    };
    // How a caller says which lane of a SEALED hive it is sending into: it
    // stamps the route on the edge (GH #237) — exactly what the door edge in
    // the test above does.
    let door = {
        let mut spec = meclaw_colony::config::ModifierSpec::default();
        spec.set_hop
            .insert("route".to_string(), "'in_pack'".to_string());
        Edge {
            modifier: Some(
                meclaw_colony::cel_eval::parse_modifier(&spec).expect("test modifier parses"),
            ),
            ..edge(CALLER, HIVE, None)
        }
    };
    let table = |edges: Vec<Edge>| {
        let mut t = EdgeTable::new();
        for e in edges {
            t.insert(e);
        }
        t
    };

    let undrained = table(vec![door.clone()]);
    let err = check_required_drains(std::slice::from_ref(pairing), &undrained).unwrap_err();
    assert_eq!(
        err.error_code(),
        "required_drain_missing",
        "wiring `in_pack` without a `pack_ack` drain must be refused: every push \
         would dead-letter its own receipt and the sender's `sent_at` stamp would \
         record a delivery nobody can confirm. The check said: {err:?}"
    );

    let drained = table(vec![
        door,
        edge(
            HIVE,
            SINK,
            Some("has(hop.route) && hop.route == 'pack_ack'"),
        ),
    ]);
    assert!(
        check_required_drains(std::slice::from_ref(pairing), &drained).is_ok(),
        "and the SAME wiring with a plain `pack_ack` drain must commit — \
         otherwise the check is refusing something other than what the hive \
         declared"
    );
}
