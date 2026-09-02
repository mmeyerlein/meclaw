//! GH #561 — the identity pack rides a v-lane: `affinity` reaches the brains
//! directly, and the level between them stops carrying it.
//!
//! Until now the pack travelled as a per-level chain: `affinity` -> the member's
//! own rim -> the `assistants` container -> the assistant level -> the two
//! sealed brains, with the assistant declaring `in_pack` / `pack_ack` purely as
//! a PASS-THROUGH. Under the v-lane ruling (GH #559, rule 2) a level that
//! declares a lane has said it takes part in it — it stamps, filters, guards —
//! so a declaration that only forwards reads as an influence claim nobody makes.
//!
//! The migration is therefore whole or not at all, and this file measures both
//! halves of it:
//!
//!   * **The road.** Two v-lanes — one edge each, `lane: "in_pack"` — carry the
//!     push from the affinity hive to the `talky` and `cogny` RIMS of a
//!     generation two levels down, and the receipts ride the same road back on
//!     `lane: "pack_ack"`. The fan-out is expressed as two edges, exactly as it
//!     was at the assistant level before; what changed is WHERE the two edges
//!     are drawn.
//!   * **What is left at the level.** The assistant's `in_pack` / `pack_ack`
//!     pass-through edges are gone, and what remains in its contract is the
//!     at-corridor declaration and nothing else: *whoever sends me this lane may
//!     draw the edge as far as my occupants `talky` and `cogny`, and no
//!     further.* That is the ruled R-V1 shape — the connect point is declared by
//!     the ENDPOINT'S PARENT, and `docs/config.md` carries this very JSON as its
//!     worked example.
//!
//! The mechanism BEHIND each rim is not this file's question: what a `talky`
//! does with an arriving pack is pinned in `gh458_the_door_in_the_wall.rs`, and
//! that a push has somewhere to land at all in
//! `gh458_a_push_lands_after_the_edge_exists.rs`. Here the question is whether
//! it still lands when the chain in between is gone.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RespawnFn, SpawnedCellKind, WakeFn,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
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

/// Every file the generation is made of, through the two refs that carry the
/// occupants this lane ends at.
const ASSISTANT_FILES: &[&str] = &["config.json", "talky/config.json", "cogny/config.json"];

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

/// Quiesce the generation the way every test of a shipped composite does, but
/// walked rather than listed: `${uuid7:…}` is an INSTANTIATION substitution, so
/// a tree written straight to disk carries the literal, and a menu or keeper
/// tick during a run would ask for a hive this colony has not got. Every `llm`
/// gets a provider that is not reachable and a model that is not real — a pack
/// costs a write and no inference (GH #263), so no brain has to answer here.
fn quiesce(dir: &std::path::Path, counter: &mut u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            quiesce(&p, counter);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let mut v = read_json(&p);
        let kind = v["cell"]["type"].as_str().unwrap_or_default().to_string();
        if kind == "timer" {
            let Some(schedules) = v["params"]["schedules"].as_array_mut() else {
                continue;
            };
            for s in schedules.iter_mut() {
                *counter += 1;
                s["schedule_id"] = json!(format!("0190a3f2-0000-7000-8000-{counter:012}"));
                s["cron"] = json!(NEVER);
            }
        } else if kind == "llm" {
            v["params"]["base_url"] = json!("http://127.0.0.1:1/v1");
            v["params"]["model"] = json!("gpt-4o-mock");
        } else {
            continue;
        }
        std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
}

// ────────────────────────────────────────────────────────── the test-only cells

/// The writing side, copied in shape from `gh458`: the actor and the SUBSCRIBER
/// ride on the hop so the port edge can promote them into context. A
/// subscription names a cell that will be handed somebody's briefs — that
/// address is a routing decision, so it belongs to an edge, never to a body.
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
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w561",
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
            "purpose": "Test stand-in that subscribes a generation to an affinity.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The generation, and the two rims the pack now ends at.
/// `bootstrap_from_filesystem` roots the tree at `main/`, so `main/assistants`
/// answers to `/assistants`.
const GENERATION: &str = "/assistants/scribe";
const SURFACE_RIM: &str = "/assistants/scribe/talky";
const CORE_RIM: &str = "/assistants/scribe/cogny";

/// The colony around the two hives, and the four edges that are the whole point
/// of GH #561: two v-lanes in, two out, and NOTHING in between. The container
/// `./assistants` is crossed and declares nothing, so it is transparent; the
/// generation is crossed and declares the lane WITH a matching `at`, so it
/// vouches for the corridor instead of hopping on it.
fn main_config() -> Value {
    let push = |rim: &str| {
        json!({"from": "./affinity", "to": format!(".{rim}"),
               "lane": "in_pack",
               "condition": format!(
                   "has(hop.route) && hop.route == 'answer' && hop.subscriber == '{GENERATION}'"),
               "modifier": {"set_hop": {"route": "'in_pack'"}}})
    };
    let receipt = |rim: &str| {
        json!({"from": format!(".{rim}"), "to": "/sink",
               "lane": "pack_ack",
               "condition": "has(hop.route) && hop.route == 'pack_ack'"})
    };
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
        // ── THE TWO V-LANES, and the two receipts that pair with them ──
        push(SURFACE_RIM),
        push(CORE_RIM),
        receipt(SURFACE_RIM),
        receipt(CORE_RIM),
        // ── everything else either hive may say, so no capture is a closed
        //    channel and a push meant for somebody else is still observable ──
        {"from": "./assistants", "to": "/park", "condition": "has(hop.route)"},
        {"from": "./affinity", "to": "/park",
         "condition": format!(
             "has(hop.route) && hop.route == 'answer' && hop.subscriber != '{GENERATION}'")}
    ]}}})
}

/// A fixed schedule id. `${uuid7:…}` is an INSTANTIATION-side substitution.
const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000561";
const NEVER: &str = "0 0 0 1 1 *";

fn build_tree(td: &tempfile::TempDir, affinity: &std::path::Path, assistant: &std::path::Path) {
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
    // The container the generation stands in is a HIVE — a scope marker, which
    // is what makes `./assistants` an edge endpoint at all, and what the two
    // v-lanes cross without touching.
    write(
        root,
        "main/assistants/config.json",
        &json!({"cell": {"type": "hive"}}),
    );
    copy_cells(affinity, &root.join("main/affinity"));
    copy_cells(assistant, &root.join("main/assistants/scribe"));
    let mut counter = 0;
    quiesce(&root.join("main/assistants"), &mut counter);
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
    });
}

// ─────────────────────────────────────────────────────────────── the boot

/// A lazy factory that accepts every params block and never runs anything —
/// copied in shape from `gh302_the_stack_grows_from_templates.rs`. The tool
/// surface of a generation carries five cell types no pack ever reaches; they
/// are registered rather than deleted so the tree that boots is the tree that
/// ships.
struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);

        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });

        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });

        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    let mut fs: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ];
    for tool in ["bash", "edit", "file", "web_fetch", "web_search"] {
        fs.push((tool.to_string(), Arc::new(InertCellFactory)));
    }
    fs
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
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

/// A brain's OWN durable state: the `system` table of one `llm` cell's
/// `cell.db`. Nothing else in this colony can write a row into it.
fn brain_slots(td: &tempfile::TempDir, rel: &str) -> Vec<(String, String)> {
    let p = td.path().join(rel).join("cell.db");
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

/// Poll one brain's `system` table until the pushed slot stands in it. The write
/// happens off the receipt's thread, so this is a wait and not a race; 30s is
/// the failure marker.
async fn await_identity(td: &tempfile::TempDir, rel: &str) -> String {
    for _ in 0..1500 {
        let slots = brain_slots(td, rel);
        if let Some((_, v)) = slots.iter().find(|(p, _)| p == "identity") {
            return v.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the pushed identity never reached {rel}'s own cell.db; it holds {:?}",
        brain_slots(td, rel)
    )
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The claim of GH #561, measured end to end: one push, two v-lanes, BOTH
/// brains of one generation, and two receipts — with no level in between
/// carrying any of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pack_reaches_both_brains_over_two_v_lanes() {
    let (Some(affinity), Some(assistant)) = (
        shipped("affinity", AFFINITY_FILES),
        shipped("assistant", ASSISTANT_FILES),
    ) else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &affinity, &assistant);
    let (h, mut sink, _park) = boot(&td).await;

    // 1. The subscription. ONE row, naming the GENERATION — the fan-out is the
    //    two edges and not two subscriptions, exactly as it was one edge and a
    //    fan-out inside the level before.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "channel": "telegram", "slots": ["identity"],
                    "subscriber": GENERATION});
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

    // 2. TWO receipts, one per rim the v-lanes end at. Counting occupants and
    //    not packs is the lane's own arithmetic (`assistant` § pack_ack) — what
    //    moved is who answers, not how many.
    let mut owners = Vec::new();
    for _ in 0..2 {
        let receipt = recv_route(&mut sink, "pack_ack").await;
        assert_eq!(
            hop_of(&receipt, "error_code"),
            "",
            "a pushed pack must be ACCEPTED at the rim the v-lane ends at; the \
             receipt says otherwise: {:?}",
            receipt.headers.hop
        );
        assert_eq!(
            hop_of(&receipt, "pack_slots"),
            "identity",
            "the pack the affinity rendered is the pack the rim wrote: {:?}",
            receipt.headers.hop
        );
        let owner = hop_of(&receipt, "pack_owner");
        assert!(
            owner.starts_with("/affinity"),
            "the receipt must name the AFFINITY side as the pack's owner — the \
             subscription's other end — and it named {owner:?}: {:?}",
            receipt.headers.hop
        );
        owners.push(owner);
    }

    // 3. And the proof that outlives the messages: the slot stands in the OWN
    //    `cell.db` of both brains of the generation. One push, two durable
    //    writes, no hop in between — a generation whose surface knew who it was
    //    while its core did not would answer as two different people.
    for rel in [
        "main/assistants/scribe/talky/brain",
        "main/assistants/scribe/cogny/brain",
    ] {
        let identity = await_identity(&td, rel).await;
        assert!(
            identity.contains("Alex"),
            "the slot at {rel} must carry what the affinity DISCLOSED — the pack \
             went through the audience filter and came out readable: {identity:?}"
        );
    }

    h.shutdown().await;
}

/// The other half of the same decision, read off the shipped template: what is
/// left at the level is the at-corridor and nothing else.
///
/// A level that declares a lane has said it takes part in it, so a declaration
/// that only forwards is an influence claim nobody makes (GH #559 rule 2). The
/// pass-through EDGES are what made it one; the declaration stays, but only in
/// the form that says where the lane may dock — `at`, which is the target's own
/// permission and never a caller's right.
#[test]
fn what_is_left_at_the_level_is_the_at_corridor() {
    let Some(assistant) = shipped("assistant", ASSISTANT_FILES) else {
        return;
    };
    let tpl = read_json(&assistant.join("config.json"));
    let rims = json!(["./talky", "./cogny"]);

    for (side, route) in [("accepts", "in_pack"), ("emits", "pack_ack")] {
        let lane = tpl["params"]["contract"][side]
            .as_array()
            .unwrap_or_else(|| panic!("the assistant declares {side}"))
            .iter()
            .find(|l| l["route"] == json!(route))
            .unwrap_or_else(|| {
                panic!(
                    "the assistant must still DECLARE `{route}`: without it the \
                     generation is not the corridor's target and a v-lane ending \
                     at one of its occupants is refused `v_lane_no_connect_point`"
                )
            });
        assert_eq!(
            lane["at"], rims,
            "`{route}` must name both rims as its connect points and nothing \
             else — the enumeration is the boundary, not an example: {lane}"
        );
    }

    let edges = tpl["params"]["graph"]["edges"]
        .as_array()
        .expect("the assistant ships a graph");
    let carried: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or_default();
            c.contains("in_pack") || c.contains("pack_ack")
        })
        .collect();
    assert!(
        carried.is_empty(),
        "the pass-through is what the migration removes: a level that both \
         declares the lane AND carries it hop by hop is the shape GH #559 rule 2 \
         calls an influence claim. Still drawn: {carried:?}"
    );
}

/// And the corridor has to SURVIVE the next mutation, which is the half a grow
/// alone never measures.
///
/// The lane-door check (GH #173) asks of every contracted hive in the colony:
/// does a message arriving at your path on this lane reach a cell inside you?
/// For a lane a level CARRIES that is the right question and the answer is the
/// door edge. For a lane it VOUCHES for it is the wrong one — the connect point
/// IS the door, it stands one storey down at the rim the `at` names, and the
/// edge that ends there is drawn by the sender when and if the corridor is
/// opened at all (the identity door is opt-in, GH #473). A check that demanded a
/// rim door here would refuse every later mutation in a colony that holds a
/// generation, and refuse it for a contract that is exactly right.
///
/// A hive the diff CREATES is not in the collected list, so the grow itself says
/// nothing about this; the mutation after it does.
#[test]
fn a_vouching_level_keeps_a_lane_door_check_it_can_pass() {
    const HIVE: &str = "/h";
    const CALLER: &str = "/caller";

    let Some(assistant) = shipped("assistant", ASSISTANT_FILES) else {
        return;
    };
    let contract = meclaw_colony::mutation::hive_contract::contract_from_cell_dir(&assistant, HIVE)
        .expect("the shipped assistant declares a contract");

    // The post-state a colony really stands in once a generation is grown: the
    // level's OWN graph, resolved at its path, plus the one lane a parent wires
    // it with. Nothing here draws the identity corridor — that is the point.
    let abs = |rel: &str| {
        if rel == "." {
            Path::new(HIVE)
        } else {
            Path::new(&format!("{HIVE}/{}", rel.trim_start_matches("./")))
        }
    };
    let tpl = read_json(&assistant.join("config.json"));
    let mut edges: Vec<meclaw_colony::edge_table::Edge> = tpl["params"]["graph"]["edges"]
        .as_array()
        .expect("the level ships a graph")
        .iter()
        .map(|e| meclaw_colony::edge_table::Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: abs(e["from"].as_str().expect("a from")),
            to: abs(e["to"].as_str().expect("a to")),
            condition: e["condition"].as_str().map(|c| {
                meclaw_colony::cel_eval::parse_condition(c).expect("a shipped condition parses")
            }),
            modifier: None,
            is_default: false,
            lane: None,
        })
        .collect();
    edges.push(meclaw_colony::edge_table::Edge {
        id: meclaw_core::Uuid::now_v7(),
        from: Path::new(CALLER),
        to: Path::new(HIVE),
        condition: Some(
            meclaw_colony::cel_eval::parse_condition("has(hop.route) && hop.route == 'in_turn'")
                .expect("test condition parses"),
        ),
        modifier: None,
        is_default: false,
        lane: None,
    });
    let mut table = meclaw_colony::edge_table::EdgeTable::new();
    for e in edges {
        table.insert(e);
    }

    meclaw_colony::mutation::hive_contract::check_lane_doors(
        std::slice::from_ref(&contract),
        &table,
    )
    .unwrap_or_else(|e| {
        panic!(
            "a level that declares a CONNECT POINT for a lane owes no door of \
                 its own: the door is the rim the `at` names, and the edge ending \
                 there belongs to whoever opens the corridor. Refusing it here \
                 would refuse every mutation that comes after a generation was \
                 grown. The check said: {e:?}"
        )
    });
}
