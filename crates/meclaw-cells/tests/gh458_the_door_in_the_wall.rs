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
//! 1. an accepted slot is READ BACK out of the agent's own durable state —
//!    since GH #889 the ledger of the composite's `./curator` (`slots`, owner
//!    `pack`), which holds the pack and hands it to the brain as a `$replace`
//!    root with the next call — never an empty dead-letter queue;
//! 2. the pack costs the agent a write and not an inference: the provider is
//!    never called on this lane;
//! 3. a slot outside the closed list refuses the WHOLE pack — asserted on the
//!    ledger, not only on the receipt, because a test that reads the ack alone
//!    would pass over a half write;
//! 4. an empty pack is a refusal, not a no-op;
//! 5. the receipt answers on success too;
//! 6. the single-slot body shape is the same door;
//! 7. the owner comes off the envelope and a body cannot move it;
//! 8. a `cogny` tells its curator who it is and still answers exactly once.
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
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
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
    // Every open generation is a candidate the moment the sweep runs. It was a
    // `KEEPER_IDLE_MS=0` line in the `.env` above until GH #138; the knob is a
    // param of `./close` now, so such a line would be read by NOTHING -- the
    // sweep would keep the shipped two hours and find no candidate.
    if keeper.exists() {
        patch(
            root,
            &format!("main/{composite}/session-keeper/close/config.json"),
            |v| v["params"]["idle_ms"] = json!(0),
        );
    }
    for cell in llm_cells_of(composite) {
        patch(root, &format!("main/{composite}/{cell}/config.json"), |v| {
            v["params"]["base_url"] = json!(base_url);
            v["params"]["model"] = json!("gpt-4o-mock");
        });
    }
}

/// Every `llm` cell the composite carries: its brain, and since GH #889 the
/// summarizer of the `./curator` in front of it. Neither is expected to talk on
/// this lane; both point at the mock so the tree is provider-free by
/// construction and not by luck.
fn llm_cells_of(composite: &str) -> Vec<String> {
    let mut cells: Vec<String> = brains_of(composite).iter().map(|b| b.to_string()).collect();
    cells.push("curator/summarizer".to_string());
    cells
}

/// Which brains the composite carries. Every shipped one has exactly one
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

/// Failure-marker timeout: 30s is the convention in this tree.
async fn recv_ack(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the pack lane answers unconditionally — no receipt arrived at all")
}

/// The agent's OWN durable state for this lane: the pack-owned slots of its
/// curator's ledger, as `(path, body)` pairs.
///
/// GH #889: the pack no longer lands in the brain's `cell.db` on arrival.
/// `./curator` holds it in its ledger — table `slots`, owner `pack`
/// (`curator@1.0.0`) — and hands it to the brain as a `$replace` root with the
/// NEXT call; the body is read from `blocks` by the slot's hash. This is what
/// the next system prompt is built from, which is what made the brain's
/// `system` table the honest signal before
/// (`gh258_the_push_lane_reaches_the_prompt.rs`).
fn ledger_slots(td: &tempfile::TempDir, composite: &str) -> Vec<(String, String)> {
    let p = td
        .path()
        .join(format!("main/{composite}/curator/ledger/cell.db"));
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.path, COALESCE(b.body, '') FROM slots s \
         LEFT JOIN blocks b ON b.hash = s.hash \
         WHERE s.owner = 'pack' ORDER BY s.path",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// A ledger path belongs to a family when it IS the family or lies under it.
/// The ledger may hold a family whole (`identity`) or leaf by leaf
/// (`identity.text`); what is pinned is the family, not the granularity.
fn in_family(path: &str, family: &str) -> bool {
    path == family
        || path
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('/'))
}

/// Every body the ledger holds under one family, joined.
fn family_body(slots: &[(String, String)], family: &str) -> String {
    slots
        .iter()
        .filter(|(p, _)| in_family(p, family))
        .map(|(_, v)| v.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Poll the curator's ledger until a slot of `family` appears. The write is the
/// LAST thing that happens on this lane and it may land off the receipt's
/// thread, so a positive read needs a window; 30s is the failure marker, the
/// 20ms step only decides how fast a green test finishes.
async fn await_slot(
    td: &tempfile::TempDir,
    composite: &str,
    family: &str,
) -> Vec<(String, String)> {
    for _ in 0..1500 {
        let slots = ledger_slots(td, composite);
        if slots.iter().any(|(p, _)| in_family(p, family)) {
            return slots;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "`{family}` never reached {composite}/curator's ledger (`slots`, owner `pack`); \
         it holds {:?}",
        ledger_slots(td, composite)
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
/// state of the agent's OWN brain — since GH #889 in its curator's ledger,
/// from which the brain's next call is built.
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

    let slots = await_slot(&td, "talky", "identity").await;
    let identity = family_body(&slots, "identity");
    assert!(
        identity.contains("You are Ada, the ledger keeper."),
        "the slot must land with the sender's own text, because that text is \
         what the curator hands the brain for its next system prompt; the \
         family holds {identity:?} and the ledger holds {slots:?}"
    );

    h.shutdown().await;
}

/// Claim 2. A changed identity costs the agent a write and never an inference.
///
/// Until GH #889 this was measured twice over: the SHAPE off the `pack` message
/// a bare collector emitted (no `messages[]`, so the brain upserted and
/// returned), and the CONSEQUENCE off the sealed tree. The shape half left with
/// the collector's `pack` route — `./curator` holds the pack and sends it to
/// the brain only with the next call (`curator@1.0.0`, route `brain`) — so the
/// consequence is the claim: the provider was never called.
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
    await_slot(&td, "talky", "persona").await;

    // The write has landed, so the composite has done everything this lane
    // asks of it. A provider call would already have been recorded.
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

/// Claim 3. One unknown slot refuses the WHOLE pack — and the proof is the
/// agent's state (since GH #889 its curator's ledger), not the receipt: a half
/// write would ack exactly the same way if the ack were all this test read.
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
    await_slot(&td, "talky", "identity").await;

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
    let slots = ledger_slots(&td, "talky");
    let identity = family_body(&slots, "identity");
    assert!(
        identity.contains("the first identity"),
        "the refused pack wrote its understood half anyway: `identity` holds \
         {identity:?}, and the whole ledger is {slots:?}"
    );
    assert!(
        !slots.iter().any(|(p, _)| in_family(p, "channel")),
        "and the unknown slot reached the curator's ledger: {slots:?}"
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
/// boundary and never reaches the pack lane. The single slot is a CONVENIENCE
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

    let slots = await_slot(&td, "talky", "persona").await;
    let persona = family_body(&slots, "persona");
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

/// Claim 8. The pack reaches the core and answers ONCE, so a caller counts
/// packs and not cores.
///
/// Until `cogny@4.4.0` this claim had a second half: the core was two brains and
/// one agent, the pack reached BOTH — a core whose thinking lane knew who it was
/// while its lookup lane did not would answer as two different people — and one
/// receipt still came back, because the collector answers before the fan-out.
/// The lookup lane is gone ([#528](https://github.com/mmeyerlein/meclaw/issues/528))
/// and the receipt half is the half that survives it: the count is what says
/// so. Since GH #889 the ack comes from the core's `./curator`, which holds
/// the pack for its one brain.
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

    let slots = await_slot(&td, "cogny", "identity").await;
    let identity = family_body(&slots, "identity");
    assert!(
        identity.contains("You are Ada, one core."),
        "the core's curator must hold the identity the pack named; its ledger \
         holds {slots:?}"
    );

    // ONE receipt. The curator answers once per pack (GH #889 moved the receipt
    // from the collector to it), never once per slot or per brain.
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
