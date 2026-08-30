//! GH #458 — the `in_pack` lane, driven through the SHIPPED agent composites.
//!
//! A `talky` is sealed: an edge naming `./brain` is refused with
//! `hive_port_boundary`, and the collector behind the door drops `system.*` on
//! every lane that could have carried one. Until 4.4.0 that meant a shipped
//! agent had no entrance for its own identity at all — `affinity` could push,
//! and there was nowhere to push to.
//!
//! `in_pack` is that entrance. Every claim below is measured on the SHIPPED
//! tree in a running colony, and every one of them is positive:
//!
//! 1. an accepted slot is READ BACK out of the brain's own `cell.db` — the
//!    durable state an `llm` cell upserts (the honest signal
//!    `gh258_the_push_lane_reaches_the_prompt.rs` established), never an empty
//!    dead-letter queue;
//! 2. the `pack` message the collector emits carries NO `messages[]`, so the
//!    brain upserts and returns without calling a provider;
//! 3. a slot outside the closed list refuses the WHOLE pack — asserted on the
//!    brain's state, not only on the receipt, because a test that reads the ack
//!    alone would pass over a half write;
//! 4. an empty pack is a refusal, not a no-op;
//! 5. the receipt answers on success too;
//! 6. the single-slot body shape is the same door;
//! 7. the owner comes off the envelope and a body cannot move it;
//! 8. a `cogny` tells BOTH of its brains and still answers exactly once.
//!
//! Free of a real provider by construction: the brain talks to a mock OpenAI
//! wire, and on this lane it is expected never to talk at all.

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
use mock_openai::MockOpenAI;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the template SKIPS instead of failing. The
/// composites under test are made of refs, so the whole reachable closure has
/// to exist before this file has anything to measure.
fn shipped(name: &str) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    root.join("config.json").exists().then_some(root)
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
/// REFERENCE, not a cell — the referenced template's tree belongs in its place.
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

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION-side substitution.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000458";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

// ────────────────────────────────────────────────────────── the test-only cells

/// The sender. It reads ONE json document out of the harness turn and emits it
/// as the pack body verbatim — `system`, `slot`/`content`, an owner key it is
/// not allowed to be believed about, or nothing at all.
///
/// It emits a body with NO `messages[]` when the case says so, because that is
/// the shape `affinity`'s push lane emits and the shape the lane's contract
/// documents: the slots and no turn beside them.
const SENDER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    spec = json.loads(raw or "{}")
except Exception:
    spec = {}
if not isinstance(spec, dict):
    spec = {}
out = {"header": {"route": "pack_out"}}
out.update(spec)
sys.stdout.write(json.dumps(out))
"#;

/// The sender's contract. `messages` is NOT required on the way out: this cell
/// exists precisely to produce the body shape the `in_pack` lane takes.
fn sender_config() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": SENDER, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {
                    "messages": {"type": "array", "required": false},
                    "system": {"type": "object", "required": false},
                    "slot": {"type": "string", "required": false},
                    "content": {"type": "object", "required": false}
                },
                "hop": {"route": {"type": "string", "values": ["pack_out"], "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for whoever pushes an identity at an agent.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The wiring a parent draws around a sealed composite for this lane, and
/// nothing else: the door edge that stamps `in_pack`, and the receipt drain the
/// composite's `required_drains` obliges. `/park` collects everything else the
/// composite may say so no capture is ever a closed channel.
fn main_config(composite: &str) -> Value {
    let hive = format!("./{composite}");
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./sender", "to": hive,
         "condition": "has(hop.route) && hop.route == 'pack_out'",
         "modifier": {"set_hop": {"route": "'in_pack'"}}},
        {"from": hive, "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'pack_ack'"},
        {"from": hive, "to": "/park",
         "condition": "has(hop.route) && hop.route != 'pack_ack'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, composite: &str, src: &std::path::Path, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config(composite));
    write(root, "main/sender/config.json", &sender_config());
    copy_cells(src, &root.join(format!("main/{composite}")));

    let keeper = root.join(format!("main/{composite}/session-keeper/night/config.json"));
    if keeper.exists() {
        patch(
            root,
            &format!("main/{composite}/session-keeper/night/config.json"),
            |v| {
                v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
                v["params"]["schedules"][0]["cron"] = json!(NEVER);
            },
        );
    }
    // GH #464 -- the collector's own timer, quiesced the same way and for the
    // same two reasons: `${uuid7:*}` is an INSTANTIATION substitution, and a
    // menu tick during a test run would ask a tools hive this colony has not
    // got. Every composite in this file carries one, because every one of them
    // carries a collector.
    patch(
        root,
        &format!("main/{composite}/collector/menu-clock/config.json"),
        |v| {
            v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
            v["params"]["schedules"][0]["cron"] = json!(NEVER);
        },
    );
    for brain in brains_of(composite) {
        patch(
            root,
            &format!("main/{composite}/{brain}/config.json"),
            |v| {
                v["params"]["base_url"] = json!(base_url);
                v["params"]["model"] = json!("gpt-4o-mock");
            },
        );
    }
}

/// Which `llm` cells the composite carries. Every shipped one has exactly one
/// since `cogny@4.4.0` ([#528](https://github.com/mmeyerlein/meclaw/issues/528))
/// took the core's lookup lane out; the indirection stays because the door is a
/// FAN-OUT by construction and a composite that grows a second brain must not
/// need a second test to notice.
fn brains_of(_composite: &str) -> &'static [&'static str] {
    &["brain"]
}

// ──────────────────────────────────────────────────────────────── the harness

struct Ports {
    ack: mpsc::Receiver<Message>,
    /// Everything the composite says that is not a receipt. Held rather than
    /// dropped: a capture whose receiver is gone turns every delivery into a
    /// send error.
    _park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
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
    let (ack_tx, ack_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(ack_tx.clone()))
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
    (
        h,
        Ports {
            ack: ack_rx,
            _park: park_rx,
        },
    )
}

/// One pack, handed to the sender as the json it should emit verbatim.
fn pack(spec: &Value) -> Message {
    MessageBuilder::new(Path::new("/sender"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text",
             "text": meclaw_core::serde_json::to_string(spec).unwrap()}
        ]})))
        .ttl(200)
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

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// Failure-marker timeout: 30s is the convention in this tree.
async fn recv_ack(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the pack lane answers unconditionally — no receipt arrived at all")
}

/// The brain's OWN durable state: the `system` table of its `cell.db`, as
/// `(slot_path, value)` pairs. This is the signal
/// `gh258_the_push_lane_reaches_the_prompt.rs` established as honest — what an
/// `llm` cell upserted and what it will concatenate into its next prompt.
fn brain_slots(td: &tempfile::TempDir, composite: &str, brain: &str) -> Vec<(String, String)> {
    let p = td.path().join(format!("main/{composite}/{brain}/cell.db"));
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

/// Poll the brain's `cell.db` until a slot path appears. The write is the
/// LAST thing that happens on this lane and it happens off the receipt's
/// thread, so a positive read needs a window; 30s is the failure marker, the
/// 20ms step only decides how fast a green test finishes.
async fn await_slot(
    td: &tempfile::TempDir,
    composite: &str,
    brain: &str,
    slot_path: &str,
) -> Vec<(String, String)> {
    for _ in 0..1500 {
        let slots = brain_slots(td, composite, brain);
        if slots.iter().any(|(p, _)| p == slot_path) {
            return slots;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "`{slot_path}` never reached {composite}/{brain}'s own cell.db; it holds {:?}",
        brain_slots(td, composite, brain)
    );
}

/// The reason of the FIRST dead letter the colony recorded, polled off its own
/// `colony.db`. 30s is the failure-marker convention.
async fn await_dead_letter(td: &tempfile::TempDir) -> String {
    let p = td.path().join("colony.db");
    for _ in 0..1500 {
        if let Ok(conn) = rusqlite::Connection::open(&p)
            && let Ok(reason) = conn.query_row(
                "SELECT error_code FROM dead_letters ORDER BY id LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
        {
            return reason;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no dead letter was ever recorded in {p:?}");
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. A whitelisted slot travels the door edges and lands as durable
/// state of the agent's OWN brain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whitelisted_slot_lands_in_the_brains_own_prompt() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(
        &json!({"system": {"identity": {"text": "You are Ada, the ledger keeper."}}}),
    ))
    .await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(
        hop_of(&ack, "error_code"),
        "",
        "a pack of one whitelisted slot must be accepted: {:?}",
        ack.headers.hop
    );

    let slots = await_slot(&td, "talky", "brain", "identity").await;
    let identity = slots
        .iter()
        .find(|(p, _)| p == "identity")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        identity.contains("You are Ada, the ledger keeper."),
        "the slot must land with the sender's own text, because that text is \
         what the brain concatenates into its next system prompt; the row \
         holds {identity:?} and the table holds {slots:?}"
    );

    h.shutdown().await;
}

/// Claim 2. The `pack` message the collector emits carries no `messages[]`, so
/// the brain upserts and returns: a changed identity costs the agent a write
/// and never an inference.
///
/// Measured twice over. The SHAPE is read off the message a bare collector
/// emits on its `pack` route — the one place it is observable, because the
/// agent composites are sealed and no edge may name their `./brain`. The
/// CONSEQUENCE is read off the sealed tree: the provider was never called.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pack_costs_the_agent_a_write_and_not_an_inference() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(
        &json!({"system": {"persona": {"text": "dry, brief"}}}),
    ))
    .await;
    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);
    await_slot(&td, "talky", "brain", "persona").await;

    // The write has landed, so the brain has seen everything this lane sends
    // it. A provider call would already have been recorded.
    let calls = mock.recorded_requests().await;
    assert!(
        calls.is_empty(),
        "a pack must cost a write and not an inference; the brain called the \
         provider {} time(s)",
        calls.len()
    );

    // And no answer left the composite. The lane produced a receipt and a
    // durable write, and nothing that looks like a turn.
    let stray = tokio::time::timeout(Duration::from_secs(2), ports.ack.recv()).await;
    assert!(
        stray.is_err(),
        "the pack lane answers ONCE; a second message arrived: {:?}",
        stray.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}

/// Claim 2, the shape half — asserted where it is observable: a bare
/// `collector`, wired by this file, whose `pack` route is drained into a sink.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_emitted_pack_carries_slots_and_no_turn() {
    let Some(src) = shipped("collector") else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(root.join(".env"), "KEEPER_IDLE_MS=0\n").unwrap();
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./sender", "to": "./collector",
             "condition": "has(hop.route) && hop.route == 'pack_out'",
             "modifier": {"set_hop": {"route": "'in_pack'"}}},
            {"from": "./collector", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'pack'"},
            {"from": "./collector", "to": "/park",
             "condition": "has(hop.route) && hop.route != 'pack'"}
        ]}}}),
    );
    write(root, "main/sender/config.json", &sender_config());
    copy_cells(&src, &root.join("main/collector"));
    patch(root, "main/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    let (h, mut ports) = boot(&td).await;

    h.send(pack(
        &json!({"system": {"handover": {"text": "the night shift note"}}}),
    ))
    .await;

    let emitted = tokio::time::timeout(Duration::from_secs(30), ports.ack.recv())
        .await
        .ok()
        .flatten()
        .expect("an accepted pack must leave the collector on the `pack` route");
    let body = body_of(&emitted);
    assert_eq!(
        body["system"]["handover"]["text"].as_str(),
        Some("the night shift note"),
        "the pack carries the slots it was given: {body}"
    );
    assert!(
        body.get("messages").is_none(),
        "the pack must carry NO messages[] — an `llm` cell handed a turn beside \
         the slots calls the provider instead of upserting and returning: {body}"
    );
    assert_eq!(
        hop_of(&emitted, "route"),
        "pack",
        "and it travels its own route, not the turn-bounded `brain` one: {:?}",
        emitted.headers.hop
    );

    h.shutdown().await;
}

/// Claim 3. One unknown slot refuses the WHOLE pack — and the proof is the
/// brain's state, not the receipt: a half write would ack exactly the same way
/// if the ack were all this test read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_outside_the_list_refuses_the_whole_pack() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    // First a pack that IS accepted, so the assertion below is about a write
    // that did not happen rather than about a lane that never worked.
    h.send(pack(
        &json!({"system": {"identity": {"text": "the first identity"}}}),
    ))
    .await;
    let first = recv_ack(&mut ports.ack).await;
    assert_eq!(hop_of(&first, "error_code"), "", "{:?}", first.headers.hop);
    await_slot(&td, "talky", "brain", "identity").await;

    // Now the mixed pack: one slot the list knows, one it does not.
    h.send(pack(&json!({"system": {
        "identity": {"text": "the second identity"},
        "channel": {"text": "telegram"}
    }})))
    .await;
    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(
        hop_of(&ack, "error_code"),
        "slot_unknown",
        "a slot outside the closed list refuses the pack: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_unknown"),
        "channel",
        "and the receipt names WHICH slot it refused, or a sender cannot fix \
         it: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_slots"),
        "channel,identity",
        "the receipt names everything that was asked for, sorted: {:?}",
        ack.headers.hop
    );

    // All or nothing. The identity slot still carries the FIRST text — the
    // understood half of a refused pack must not have been written.
    let slots = brain_slots(&td, "talky", "brain");
    let identity = slots
        .iter()
        .find(|(p, _)| p == "identity")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        identity.contains("the first identity"),
        "the refused pack wrote its understood half anyway: `identity` holds \
         {identity:?}, and the whole table is {slots:?}"
    );
    assert!(
        !slots.iter().any(|(p, _)| p.starts_with("channel")),
        "and the unknown slot reached the brain: {slots:?}"
    );

    h.shutdown().await;
}

/// Claim 4. An empty pack is refused with its own code — a sender that
/// addressed the wrong body key must not read silence as success.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_pack_is_refused_and_not_ignored() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(&json!({"system": {}}))).await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(
        hop_of(&ack, "error_code"),
        "pack_empty",
        "an empty pack answers with its own code: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_slots"),
        "",
        "nothing was named: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_unknown"),
        "",
        "and nothing was unknown either — an empty pack is not an unknown \
         slot: {:?}",
        ack.headers.hop
    );

    h.shutdown().await;
}

/// Claim 5. The receipt answers on success too, and it names the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_receipt_answers_on_success_too() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(&json!({"system": {
        "identity": {"text": "Ada"}, "persona": {"text": "dry"}
    }})))
    .await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(
        hop_of(&ack, "error_code"),
        "",
        "an accepted pack is acked with an EMPTY code, present and empty \
         rather than absent: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_slots"),
        "identity,persona",
        "the receipt names what it wrote, sorted: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        hop_of(&ack, "pack_owner"),
        "/sender",
        "the receipt names the sender the SUBSTRATE recorded on the envelope \
         (`bootstrap_from_filesystem` roots the tree at `main/`, so \
         `main/sender` answers to `/sender`): {:?}",
        ack.headers.hop
    );

    // Exactly one receipt, not one per slot.
    let second = tokio::time::timeout(Duration::from_secs(2), ports.ack.recv()).await;
    assert!(
        second.is_err(),
        "one pack, one receipt; a second arrived: {:?}",
        second.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}

/// Claim 6. `slot`/`content` is the same door, for a caller writing one slot by
/// hand.
///
/// The empty `"system": {}` beside it is NOT cosmetic and must not be tidied
/// away. `meclaw-core`'s `validate_ubf_body` requires a body to carry
/// `messages` OR `system`, so `{"slot": …, "content": …}` on its own is not a
/// UBF body at all: it is dead-lettered as `invalid_ubf_body` at the delivery
/// boundary and never reaches the collector. The single slot is a CONVENIENCE
/// over a pack, not a body shape of its own — it is merged over whatever
/// `system` carried, and an empty tree is what "nothing to merge over" looks
/// like. The test below this one pins that rule from the other side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_single_slot_form_is_the_same_door() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(&json!({
        "system": {},
        "slot": "persona",
        "content": {"text": "terse, never chatty"}
    })))
    .await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);
    assert_eq!(
        hop_of(&ack, "pack_slots"),
        "persona",
        "the single slot is the whole pack: {:?}",
        ack.headers.hop
    );

    let slots = await_slot(&td, "talky", "brain", "persona").await;
    let persona = slots
        .iter()
        .find(|(p, _)| p == "persona")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        persona.contains("terse, never chatty"),
        "the single-slot form lands under `system.persona` like the tree form \
         does: {slots:?}"
    );

    h.shutdown().await;
}

/// Claim 7. The owner comes off the envelope. A body may name whatever it
/// likes — bodies are written by whatever produced the message, up to and
/// including a model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_comes_off_the_envelope_and_not_out_of_the_body() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(&json!({
        "system": {"identity": {"text": "Ada"}},
        "owner": "/somebody-else",
        "reply_to": "/somebody-else",
        "pack_owner": "/somebody-else"
    })))
    .await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(
        hop_of(&ack, "pack_owner"),
        "/sender",
        "the owner is what the SUBSTRATE wrote on the envelope; three body keys \
         claiming otherwise must change nothing: {:?}",
        ack.headers.hop
    );
    assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);

    h.shutdown().await;
}

/// Claim 8. The pack reaches the core's brain and answers ONCE, so a caller
/// counts packs and not cores.
///
/// Until `cogny@4.4.0` this claim had a second half: the core was two brains and
/// one agent, the pack reached BOTH — a core whose thinking lane knew who it was
/// while its lookup lane did not would answer as two different people — and one
/// receipt still came back, because the collector answers before the fan-out.
/// The lookup lane is gone ([#528](https://github.com/mmeyerlein/meclaw/issues/528))
/// and the receipt half is the half that survives it: the ack is emitted where
/// it always was, and the count is what says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_core_tells_its_brain_who_it_is_and_answers_once() {
    let Some(src) = shipped("cogny") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "cogny", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(
        &json!({"system": {"identity": {"text": "You are Ada, one core."}}}),
    ))
    .await;

    let ack = recv_ack(&mut ports.ack).await;
    assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);

    for brain in brains_of("cogny") {
        let slots = await_slot(&td, "cogny", brain, "identity").await;
        let identity = slots
            .iter()
            .find(|(p, _)| p == "identity")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        assert!(
            identity.contains("You are Ada, one core."),
            "{brain} must carry the identity the pack named; it holds {slots:?}"
        );
    }

    // ONE receipt. The collector answers BEFORE any fan-out, so a composite
    // with two brains would still not produce two acks.
    let second = tokio::time::timeout(Duration::from_secs(2), ports.ack.recv()).await;
    assert!(
        second.is_err(),
        "one pack, one receipt — never one per brain: {:?}",
        second.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}

/// The rule the test above rides on, pinned from the other side: a body that is
/// ONLY `slot`/`content` never reaches the lane at all.
///
/// `validate_ubf_body` (meclaw-core) accepts a body that carries `messages` or
/// `system`, and nothing else counts. So the convenience form is a convenience
/// over a pack and not a second body shape, and a caller who drops the empty
/// `system` gets a dead letter rather than a refusal on the lane — a different
/// failure, at a different boundary, with a different place to look. Left
/// unpinned, the next reader tidies the empty tree out of the test above and
/// the lane goes silent for a reason nothing in this file explains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_is_only_a_slot_never_reaches_the_lane() {
    let Some(src) = shipped("talky") else {
        return;
    };
    let mock = MockOpenAI::start(Vec::new()).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "talky", &src, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(pack(
        &json!({"slot": "persona", "content": {"text": "terse, never chatty"}}),
    ))
    .await;

    // The positive receipt is the dead letter itself, with the reason the
    // substrate recorded — not the absence of an ack.
    let reason = await_dead_letter(&td).await;
    assert_eq!(
        reason, "invalid_ubf_body",
        "a body carrying neither `messages` nor `system` is refused at the \
         DELIVERY boundary and never reaches the pack lane; the substrate \
         recorded {reason:?} instead"
    );

    // And the lane really did stay silent, so the dead letter is the whole
    // story rather than one of two outcomes.
    let ack = tokio::time::timeout(Duration::from_secs(2), ports.ack.recv()).await;
    assert!(
        ack.is_err(),
        "the message was dead-lettered, so no receipt may exist: {:?}",
        ack.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}
