//! GH #453 / GH #458 — the push lane is EARNED by a subscribe, and the member
//! owns the subscription.
//!
//! `affinity`'s push lane was built and pinned before anything shipped that
//! used it: `push` hashed what a subscriber would get, `brief` rendered it,
//! `clock` ticked it, and every test that exercised the lane drew the two edges
//! by hand inside the test. Two things were missing and neither of them was a
//! bug in the mechanism.
//!
//! 1. **No `subscribers` seed.** A fresh hive started with an empty
//!    subscription table, so a tick was silent for a STRUCTURAL reason, and
//!    from outside that is indistinguishable from a tick that found nothing
//!    changed. An empty result and a forgotten call must never look alike
//!    (`docs/development-rules.md` § 2c). The template ships exactly one row
//!    since #453 — the seeded agent's own AIeOS document into the seeded
//!    agent's own brain, on every channel, in the `identity` slot — cut by the
//!    three `disc:aiden-self-*` releases that were already there.
//! 2. **Nobody owned the subscription.** `templates/affinity/README.md` said
//!    the question landed with GH #302, and #302 shipped the four composition
//!    levels without answering it, which left `member` carrying a flat
//!    cancellation ("No push edge into a subscribing brain") that read as a
//!    decision and was an open question. The answer is the **member**: a
//!    subscription is a row in the record, the record is the member's, and an
//!    assistant is replaced per generation — a subscription owned one level
//!    down would have to be re-written on every swap.
//!
//! **And the seeded row ships INACTIVE (#458).** That is the correction this
//! file's name carries. #453 shipped the row `active`, which claimed a delivery
//! the tree cannot make: the `cell_path` names a brain no shipped graph draws
//! an edge to, and until #458 gave `talky` and `cogny` an `in_pack` lane no
//! shipped brain even HAD a lane that accepts an identity. A row is half a
//! subscription; the other half is an edge, and writing an edge is a mutation,
//! so the shipped half can never be the whole one. The row is therefore an
//! EXAMPLE of the shape, and the push is earned by a `subscribe` op rather than
//! granted at birth.
//!
//! Silence keeps its reason either way, and that is the point of the flip
//! rather than a cost of it: `./push` selects `where status = 'active'`, so an
//! untouched hive is silent because nothing has SUBSCRIBED, and that is a state
//! anybody can read off the `status` column. § 2c is satisfied by the column
//! instead of by a fiction.
//!
//! What a template can and cannot ship is the sharp half, and it is asserted
//! here rather than only written: the ROW ships, the EDGE cannot. Writing an
//! edge is a mutation and mutation authority is the colony's, so the seeded
//! `cell_path` is a birth-state token exactly the way `entity:alex` is, and the
//! mutation that instantiates the member either conditions the push edge on
//! that token or rewrites the row through `./gate`.
//!
//! This file is the drift lock (`docs/development-rules.md` § 2d) for that
//! promise: it greps the sentences on the three public template surfaces AND
//! drives the mechanism through a real colony. Either half alone would rot —
//! grepping pins a string, asserting alone lets the prose walk away from it.
//!
//! **R2b guard (GH #49 form).** Every read is guarded by [`shipped_affinity`] /
//! [`shipped_member`], so a tree that does not carry the template skips instead
//! of failing on a dead `templates/` reference.

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

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The address the seeded row hands `./push`, and therefore the literal the
/// delivering edge has to compare `hop.subscriber` against. It is a TOKEN, not
/// a live path: no instance is called this until a mutation says so.
const SEEDED_SUBSCRIBER: &str = "./assistants/aiden/brain";

/// Everything the seeded lane needs to exist at all. A tree missing any of it
/// skips (GH #49) rather than failing on a template it does not carry.
const REQUIRED: &[&str] = &[
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

fn shipped_affinity() -> Option<std::path::PathBuf> {
    let root = repo("templates/affinity");
    REQUIRED
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

fn shipped_member() -> Option<std::path::PathBuf> {
    let root = repo("templates/member");
    ["config.json", "template.json", "README.md"]
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The seed file as `(schema, rows)`.
fn seeded_subscribers(root: &std::path::Path) -> (Value, Vec<Value>) {
    let raw = std::fs::read_to_string(root.join("store/seed/subscribers.jsonl"))
        .expect("store/seed/subscribers.jsonl");
    let mut schema = Value::Null;
    let mut rows = Vec::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = meclaw_core::serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("a seed line is not JSON: {e} -- {line}"));
        if v.as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key("schema"))
        {
            schema = v["schema"].clone();
        } else {
            rows.push(v);
        }
    }
    (schema, rows)
}

// ═══════════════════════════════════════════════ (a) the row, and only one row

/// **Exactly one INACTIVE row, and every column the store declares.**
///
/// One, because a seed is a birth state and not a fixture: a second row would
/// be a second promise about a topology nobody has drawn yet. Inactive (#458),
/// because the shipped half of a subscription is the row and the other half is
/// an edge only a mutation may draw — an `active` seed promises a delivery the
/// tree cannot make, which is a worse fiction than the structural silence #453
/// removed. And every column, because the store's `subscribers` schema is what
/// `./push` selects: a seed row missing `pack_hash` would make the first tick
/// after a subscribe compare a hash against a column that is not there.
#[test]
fn the_record_ships_exactly_one_inactive_subscription() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let (schema, rows) = seeded_subscribers(&root);
    assert_eq!(
        rows.len(),
        1,
        "the seed is a BIRTH state: one subscription, so the push lane has \
         traffic on the first tick and no more topology is promised than the \
         template can keep -- got {rows:?}"
    );
    let row = &rows[0];
    assert_eq!(
        row["status"].as_str(),
        Some("inactive"),
        "the seeded row is an EXAMPLE of the shape, not a live subscription: an \
         active seed claims a delivery no shipped edge can make, and `./push` \
         selects on status = 'active' precisely so that an unsubscribed hive is \
         silent for a reason this column states (GH #458): {row}"
    );
    assert_eq!(row["cell_path"].as_str(), Some(SEEDED_SUBSCRIBER), "{row}");
    assert_eq!(
        row["subject"].as_str(),
        Some("entity:aiden"),
        "the birth subscription is the agent's own document -- the persona is a \
         projection, not a copy: {row}"
    );
    assert_eq!(
        row["audience"].as_str(),
        Some("agent:aiden"),
        "and it is cut for the agent itself, which is what the three \
         disc:aiden-self-* releases already disclose: {row}"
    );
    assert_eq!(row["channel"].as_str(), Some("*"), "{row}");
    assert_eq!(
        row["slots"].as_array().map(|a| a.len()),
        Some(1),
        "one slot, `identity`: {row}"
    );
    assert_eq!(row["slots"][0].as_str(), Some("identity"), "{row}");
    assert_eq!(
        row["pack_hash"].as_str(),
        Some(""),
        "an empty hash IS the change the first tick after a subscribe finds -- a \
         seeded hash would make the lane silent even once somebody subscribed: \
         {row}"
    );

    // The row against the store's own schema, both ways: a column the store
    // does not know is refused on insert, and a column the seed omits is one
    // `./push` selects and does not get.
    let store: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("store/config.json")).expect("store/config.json"),
    )
    .expect("store/config.json parses");
    let declared = store["params"]["schema"]["subscribers"]
        .as_object()
        .expect("the store declares a subscribers table");
    let seeded = row.as_object().expect("the seed row is an object");
    for col in declared.keys() {
        assert!(
            seeded.contains_key(col),
            "the seed row omits `{col}`, which ./push selects every tick: {row}"
        );
    }
    for col in seeded.keys() {
        assert!(
            declared.contains_key(col),
            "the seed row carries `{col}`, which the store's subscribers table \
             does not declare: {row}"
        );
    }
    assert_eq!(
        schema.as_object().map(|o| o.len()),
        Some(declared.len()),
        "the seed's own schema line and the store's table have to be the same \
         table: {schema}"
    );
}

// ══════════════════════════════════════ (b) the row ships, the edge cannot

/// **The drift lock on the three public template surfaces.**
///
/// The promise is two-sided and the sides are easy to collapse into each other:
/// the ROW ships (as the shape a `subscribe` fills in, which is why it ships
/// INACTIVE — GH #458), the EDGE does not and cannot (so a parent still owes
/// one per subscribing cell). A surface that states only the first invites a
/// reader to expect delivery; one that states only the second is the
/// cancellation this issue removed.
#[test]
fn no_public_surface_still_cancels_the_push_edge() {
    let Some(member) = shipped_member() else {
        return;
    };
    let Some(affinity) = shipped_affinity() else {
        return;
    };
    let surfaces = [
        member.join("config.json"),
        member.join("template.json"),
        member.join("README.md"),
        affinity.join("README.md"),
    ];
    for path in &surfaces {
        let raw = std::fs::read_to_string(path).expect("a shipped surface");
        for cancelled in [
            "No push edge into a subscribing brain",
            "This template only offers the lane",
        ] {
            assert!(
                !raw.contains(cancelled),
                "{} still cancels the push edge ({cancelled:?}). It is not \
                 cancelled: the member owns the subscription and the \
                 instantiating mutation draws the edge (GH #453)",
                path.display()
            );
        }
    }

    // The half that ships: the member says the row exists and says who owns it.
    let readme = std::fs::read_to_string(member.join("README.md")).expect("member README.md");
    assert!(
        readme.contains("seed/subscribers.jsonl"),
        "the member's README names no seed file, so a reader cannot find the \
         row that makes the lane live"
    );
    assert!(
        readme.contains("453"),
        "the member's README does not point at the issue that decided the owner"
    );

    // The half that cannot: mutation authority, stated where the reader is.
    assert!(
        readme.contains("authority is the colony's"),
        "the member's README no longer says WHY the edge is not shipped -- \
         without that sentence the missing edge reads as an oversight"
    );

    // And the mechanism behind the second half, not just the sentence: the
    // record's own answer exit is conditioned on the empty subscriber, so a
    // push can never leave on the brief lane (GH #289).
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(member.join("config.json")).expect("member config.json"),
    )
    .expect("member config.json parses");
    let guarded = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("the member declares edges")
        .iter()
        .filter(|e| e["from"] == "./affinity" && e["to"] == ".")
        .any(|e| {
            e["condition"].as_str().is_some_and(|c| {
                c.contains("hop.route == 'answer'") && c.contains("hop.subscriber == ''")
            })
        });
    assert!(
        guarded,
        "the member's answer exit does not carry `hop.subscriber == ''` -- \
         without it the brief edge collects every push as well, which is the \
         instance defect GH #289 measured"
    );
}

// ═══════════════════════════════════════════════════ (c) the mechanism, live

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

/// The tick, driven by hand. `./clock` fires `./push` on a cron and that wiring
/// is pinned by `push_fires_on_a_changed_pack_and_stays_silent_without_one`
/// (`affinity_template.rs`); what THIS file needs is a tick at a moment it
/// chose, so the store can be read between the subscribe and the push without
/// racing a schedule. The edge below hands `./push` exactly the context the
/// clock edge hands it.
const TICKER: &str = r#"
import sys, json
sys.stdout.write(json.dumps({
    "header": {"route": "ptick"},
    "messages": [{"origin": "user", "type": "text", "id": "t1", "text": "tick"}]}))
"#;

/// The writing side. It stamps the two identity facts of a `subscribe` on the
/// HOP so the propose edge can promote them into `context` -- which is the only
/// place `./gate` reads them from (`subscriber_not_on_edge`,
/// `identity_from_body`). A shipped producer takes both from its own wiring;
/// this stand-in takes them out of the request JSON so one test can choose.
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
    "header": {"route": "propose",
               "actor": str(a.get("actor") or ""),
               "subscriber": str(a.get("subscriber") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w1",
                  "text": raw}]}))
"#;

/// The shipped template, copied cell by cell -- `config.json` files and the
/// seeds next to them, which is what instantiation copies.
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

/// The ticker's `config.json`.
fn ticker_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": TICKER, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": ["ptick"], "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that fires one push tick at a chosen moment.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The writer stand-in's `config.json` -- the two hop keys the propose edge
/// promotes are declared here, because an undeclared hop key does not survive
/// the contract check.
fn writer_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": WRITER, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["propose"], "required": false},
                    "actor": {"type": "string", "required": false},
                    "subscriber": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that proposes through the shipped write port.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn probe_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": PROBE, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": ["pstore"], "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that reads what the shipped ports wrote.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The topology a PARENT owes the seeded row: one edge per subscribing cell,
/// conditioned on the token that row carries. The push edge is drawn FIRST and
/// stands here from boot -- which is the harmless half of the two-halves order
/// (#458): an edge with no active row behind it carries nothing, because
/// `./push` selects on `status = 'active'` and never sees the subscriber.
///
/// The write lane is the second half. It promotes the two identity facts of a
/// `subscribe` into context, and it is the ONLY way anything in this colony can
/// turn the seeded example into a live subscription -- no asker is wired at
/// all, so a push that arrives at `/pushsink` can only have come from a row
/// somebody subscribed.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./affinity", "to": "/pushsink",
         "condition": format!(
             "has(hop.route) && hop.route == 'answer' && hop.subscriber == '{SEEDED_SUBSCRIBER}'")},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == ''"},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./writer", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'propose'",
         "modifier": {"set_hop": {"route": "'in_propose'"},
                      "set_context": {
                          "actor": "hop.actor",
                          "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}},
        {"from": "./ticker", "to": "./affinity/push",
         "condition": "has(hop.route) && hop.route == 'ptick'",
         "modifier": {"set_context": {"affinity_origin": "'push'",
                                      "aff_phase": "''", "aff_carry": "''"}}},
        {"from": "./probe", "to": "./affinity/store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"affinity_origin": "'probe'"}}},
        {"from": "./affinity/store", "to": "/sink",
         "condition": "context.affinity_origin == 'probe'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, cron: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    write(root, "main/probe/config.json", &probe_cell());
    write(root, "main/writer/config.json", &writer_cell());
    write(root, "main/ticker/config.json", &ticker_cell());
    copy_cells(root_template, &root.join("main/affinity"));
    let p = root.join("main/affinity/clock/config.json");
    let mut v: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000000b5");
    // The cadence used to come out of the `.env` through a
    // `${AFFINITY_PUSH_CRON:-…}` token. Since GH #138 it is a literal of
    // `./clock`'s own params, so it is written here beside the schedule_id --
    // an `.env` line would be read by nothing at all and would say nothing
    // about it.
    v["params"]["schedules"][0]["cron"] = json!(cron);
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
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
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (push_tx, push_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/pushsink"), move || {
        CaptureCell::new(push_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, push_rx)
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

fn turn_json(m: &Message) -> Value {
    let text = body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default();
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..12 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                panic!("nothing more arrived while waiting for route {route:?}; saw {seen:?}")
            });
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(hop_of(&m, "route"));
    }
    panic!("route {route:?} never arrived; saw {seen:?}");
}

async fn probe(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(to(
        "/probe",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    turn_json(&recv_route(rx, "").await)
}

/// **Silence first, delivery second -- and a `subscribe` is what separates
/// them.**
///
/// One colony, two assertions, in the order that makes them mean something.
///
/// 1. A tick over the SHIPPED tree pushes nothing at all, and the store still
///    shows the seeded row `inactive`. That is silence with a readable reason:
///    `./push` selects `where status = 'active'` and finds no subscriber, which
///    is a different observation from finding one whose hash did not move
///    (`docs/development-rules.md` § 2c is satisfied by the column, not by a
///    seed that claims a delivery the tree cannot make -- GH #458).
/// 2. Then one `subscribe` travels the shipped write port over an edge that
///    promotes `context.actor` and `context.subscriber`, the row lands `active`
///    with an empty `pack_hash`, and the NEXT tick delivers: the pack arrives at
///    the wired sink under the subscribed address, carrying `system.*` and no
///    turn beside it, and the hash and the timestamp are written back.
///
/// The two halves also prove each other. The tick in (1) is the same mechanism
/// as the tick in (2), so the silence is not the silence of a dead lane -- and
/// the delivery in (2) is not a lane that would have fired anyway, because
/// nothing but the `subscribe` changed between them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_seeded_subscription_is_silent_until_something_subscribes() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    // The clock stays quiet: every tick in this test is one this test sent, so
    // the store can be read between the subscribe and the push.
    build_tree(&td, &root, "0 0 4 * * *");
    let (h, mut rx, mut push_rx) = boot(&td).await;

    // ── 1. the shipped tree, ticked, says nothing ──────────────────────────
    h.send(to("/ticker", "tick")).await;
    let quiet = tokio::time::timeout(Duration::from_secs(8), push_rx.recv()).await;
    assert!(
        quiet.is_err(),
        "the shipped tree has nobody subscribed, so a tick must deliver nothing \
         at all -- the seeded row is an EXAMPLE of the shape, and `./push` \
         selects on status = 'active' (GH #458): {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    let subs = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "subscribers",
               "columns": ["id", "cell_path", "pack_hash", "status"], "limit": 5}),
    )
    .await;
    assert_eq!(
        subs.as_array().map(|a| a.len()),
        Some(1),
        "the seed ships exactly one row and the tick wrote none: {subs}"
    );
    assert_eq!(
        subs[0]["status"].as_str(),
        Some("inactive"),
        "and it is still inactive -- that column is WHY the tick was silent, \
         which is what makes the silence readable instead of ambiguous: {subs}"
    );
    assert_eq!(subs[0]["id"].as_str(), Some("sub:aiden-self"), "{subs}");
    assert_eq!(
        subs[0]["pack_hash"].as_str(),
        Some(""),
        "nothing was served, so nothing was hashed: {subs}"
    );

    // ── 2. one subscribe, and the same tick delivers ───────────────────────
    // The body says WHAT (the subject, the slots); WHERE the pushes go and WHO
    // they are cut for come off the edge (GH #288), which is why the writer
    // stand-in stamps them on the hop and the port edge promotes them. The
    // address it names is the token the push edge was drawn on at boot -- the
    // edge first, the row second (GH #458).
    let op = json!({"op": "subscribe", "subject": "entity:aiden",
                    "slots": ["identity"], "actor": "agent:aiden",
                    "subscriber": SEEDED_SUBSCRIBER});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(
        payload["outcome"].as_str(),
        Some("accepted"),
        "the subscribe carries no identity of its own, which is a COMPLETE \
         request: the edge supplies the rest: {payload}"
    );

    let subs = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "subscribers",
               "columns": ["cell_path", "subject", "audience", "pack_hash", "status"],
               "where": {"status": "active"}, "limit": 5}),
    )
    .await;
    assert_eq!(
        subs.as_array().map(|a| a.len()),
        Some(1),
        "the subscribe minted the colony's first ACTIVE subscription: {subs}"
    );
    assert_eq!(
        subs[0]["cell_path"].as_str(),
        Some(SEEDED_SUBSCRIBER),
        "at the address the EDGE named, which only the colony writes: {subs}"
    );
    assert_eq!(subs[0]["subject"].as_str(), Some("entity:aiden"), "{subs}");
    assert_eq!(
        subs[0]["audience"].as_str(),
        Some("agent:aiden"),
        "cut for the actor the edge named: {subs}"
    );
    assert_eq!(
        subs[0]["pack_hash"].as_str(),
        Some(""),
        "and with an empty hash, which is what makes the very next tick a \
         change: {subs}"
    );

    // The next tick. Nothing else moved -- same tree, same ticker, same edge.
    h.send(to("/ticker", "tick")).await;
    let pushed = recv_route(&mut push_rx, "answer").await;
    assert_eq!(
        hop_of(&pushed, "subscriber"),
        SEEDED_SUBSCRIBER,
        "the pack names the address ./push read off the row, which is how the \
         parent's edge finds the brain it belongs to: {:?}",
        pushed.headers.hop
    );
    assert!(
        body_of(&pushed).get("messages").is_none(),
        "a push carries system.* and nothing else; a turn beside it makes the \
         subscriber's llm cell call a provider (GH #263): {:?}",
        body_of(&pushed)
    );
    assert_eq!(
        body_of(&pushed)["system"]["identity"]["names"]["first"].as_str(),
        Some("Aiden"),
        "and it is the AGENT's own document, through the same audience filter \
         every read goes through: {:?}",
        body_of(&pushed)["system"]
    );
    assert!(
        body_of(&pushed)["system"].get("peer").is_none(),
        "through the same slot selection too -- the subscription asked for one \
         slot: {:?}",
        body_of(&pushed)["system"]
    );

    // The receipt on the row itself: what was sent, and when.
    let subs = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "subscribers",
               "columns": ["cell_path", "pack_hash", "sent_at", "status"],
               "where": {"status": "active"}, "limit": 5}),
    )
    .await;
    assert_eq!(subs.as_array().map(|a| a.len()), Some(1), "subs: {subs}");
    assert_eq!(
        subs[0]["pack_hash"].as_str().map(|s| s.len()),
        Some(64),
        "the tick wrote the sha256 of what it sent: {subs}"
    );
    assert!(
        subs[0]["pack_hash"]
            .as_str()
            .is_some_and(|s| s.chars().all(|c| c.is_ascii_hexdigit())),
        "and it is a hash, not a marker: {subs}"
    );
    assert!(
        subs[0]["sent_at"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z') && s.len() >= 20),
        "and when it sent it: {subs}"
    );

    h.shutdown().await;
}
