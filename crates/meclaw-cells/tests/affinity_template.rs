//! meclaw-os -- the shipped `affinity@1` template, the first hive with a
//! domain of its own (V8 spec, rulings of 2026-08-15).
//!
//! What is pinned here is what the template PROMISES, in the order the README
//! promises it:
//!
//! 1. **The inventory and the vendored schema.** Five cells and one vendored
//!    `aieos.schema.json`, pinned at 1.1.0 -- and the mandatory-path list the
//!    `gate` script carries as a literal is pinned AGAINST that file, so a
//!    schema swap that does not move the validator fails here instead of in a
//!    write six months later. The seed documents are pinned the same way: a
//!    seeded AIeOS document cannot carry a section the vendored skeleton does
//!    not know. That is the byte-drift lock of a template whose contract is a
//!    foreign spec.
//! 2. **Only the gate writes, and it validates.** A valid entity reaches the
//!    store and leaves an audit line; a document missing a mandatory path
//!    reaches nothing and leaves an audit line saying so.
//! 3. **The read port is fail-closed.** A disclosed audience gets exactly the
//!    fields its rows named; an audience nobody decided about gets NOTHING --
//!    not a redacted pack, not an empty object, no `system` slot at all.
//! 4. **Relations are a store op.** "Who is Sam to Alex" comes back over two
//!    hops with its path, out of ONE `traverse`, because the table is cut on
//!    that op's signature.
//! 5. **Push is silent by default.** A changed subject produces exactly one
//!    re-brief; the tick after it produces none, because the hash it computes
//!    is the hash the first one stored.
//!
//! Free of a provider by construction: this hive holds no model at all.
//!
//! **R2b guard (GH #49 form).** `affinity@1` is PRIVATE -- it is not in
//! `PUBLIC_TEMPLATES`, so it does not travel with the export. Every read below
//! is guarded per file by [`shipped_affinity`]; in the public clone the guard
//! exits cleanly and these tests skip instead of failing on a dead
//! `templates/` reference. Same form as `cogny_template.rs` carried BEFORE its
//! public switch, and the matching `ALLOWED_HITS` entry lives in
//! `plans/export-fixtures/make_export.py`.

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
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
];

/// The non-`config.json` files the template ships: four seed tables and the
/// vendored schema. The seed is data, the schema is a pinned copy of a foreign
/// spec -- neither is read at runtime by a cell, and the schema is never
/// fetched from the network.
const AFFINITY_SEEDS: &[&str] = &[
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
];
const VENDORED_SCHEMA: &str = "aieos.schema.json";

/// The template root, or `None` where it does not ship (the documented R2b
/// exception form, GH #49).
fn shipped_affinity() -> Option<std::path::PathBuf> {
    let root = templates_root().join("affinity");
    for rel in AFFINITY_FILES
        .iter()
        .chain(AFFINITY_SEEDS)
        .chain(std::iter::once(&VENDORED_SCHEMA))
    {
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

/// The asking side of the read port: it turns one request into the tool_call
/// the hive documents, and declares WHO is asking on the hop -- which the port
/// edge then promotes to `context.asker`. That promotion is the whole point:
/// `brief` never reads an audience out of a body.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
req = {"subject": a.get("subject"), "channel": a.get("channel") or "*"}
if a.get("slots") is not None:
    req["slots"] = a.get("slots")
sys.stdout.write(json.dumps({
    "header": {"route": "brief", "audience": str(a.get("audience") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "b1",
                  "text": json.dumps(req)}]}))
"#;

/// The writing side: the same shape for the write port, with the actor on the
/// hop so the port edge can make it edge truth.
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": "member:alex"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w1",
                  "text": raw}]}))
"#;

/// The probe. It reaches PAST the ports straight into `./store`, which is
/// exactly the bypass the README calls out as possible -- here it is used the
/// one way it is legitimate: to read what the ports wrote.
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

// ─────────────────────────────────────────────────────────────── the topology

/// The ports around the hive -- every one a literal copy of what
/// `templates/affinity/README.md` documents, plus the probe pair that is the
/// test's own read channel. The template draws no edge that appears here.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── in_brief: the asker's identity becomes EDGE truth, and only here ──
        {"from": "./asker", "to": "./affinity/brief",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_context": {"asker": "hop.audience"}}},
        // ── out_brief / out_push: the same exit, told apart by hop.subscriber ──
        {"from": "./affinity/brief", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // ── in_propose: the writer's identity, likewise from the edge ──
        {"from": "./writer", "to": "./affinity/gate",
         "condition": "has(hop.route) && hop.route == 'propose'",
         "modifier": {"set_context": {"actor": "hop.actor"}}},
        // ── out_ack ──
        {"from": "./affinity/gate", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        // ── out_error: the drain the parent MUST wire, all three code cells ──
        {"from": "./affinity/gate", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./affinity/brief", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./affinity/push", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── the test's own read channel, straight into the store ──
        {"from": "./probe", "to": "./affinity/store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"affinity_origin": "'probe'"}}},
        {"from": "./affinity/store", "to": "/sink",
         "condition": "context.affinity_origin == 'probe'"}
    ]}}})
}

/// A cron far enough away that no tick happens during a test that is not about
/// ticks. The push test overrides it.
const QUIET_CRON: &str = "0 0 4 * * *";

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, cron: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), format!("AFFINITY_PUSH_CRON={cron}\n")).unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["brief"],
            json!({"audience": {"type": "string", "required": false}}),
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
    copy_cells(root_template, &root.join("main/affinity"));
    // `${uuid7:…}` is an INSTANTIATION-side substitution (the mutation path
    // mints it); a raw directory copy bootstrapped from the filesystem has to
    // be handed a literal, exactly as `cogny_template.rs` hands its brains a
    // base_url. The cron itself is NOT patched -- it comes out of the `.env`
    // above through the shipped `${AFFINITY_PUSH_CRON:-…}` default, so the
    // late-binding form is under test rather than bypassed.
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000000af");
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
    for _ in 0..12 {
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

/// A store read through the probe, returned as its rows.
async fn probe(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(to(
        "/probe",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let m = recv_route(rx, "").await;
    turn_json(&m)
}

/// A minimal but VALID AIeOS 1.1.0 document -- the four mandatory paths and
/// nothing else, which is exactly what `gate` promises to accept.
fn valid_aieos(instance: &str, first: &str) -> Value {
    json!({
        "standard": {"protocol": "AIEOS", "version": "1.1.0"},
        "metadata": {"instance_id": instance, "created_at": "2026-08-15"},
        "identity": {"names": {"first": first, "last": "Vale"}}
    })
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Five cells, no more and no less. Pinned as a SET, not as a floor: a hive
/// that grew an `llm` would still pass every round below -- it would just stop
/// being the thing whose whole cost argument is that it holds no model.
#[test]
fn the_hive_carries_five_cells_and_no_model() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let mut found = Vec::new();
    collect_configs(&root, &root, &mut found);
    let mut found: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    let mut want: Vec<String> = AFFINITY_FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        found, want,
        "affinity@1 is store + brief + gate + push + clock: no curator, no brain"
    );
    for rel in AFFINITY_FILES {
        let cfg = read_json(&root.join(rel));
        let ty = cfg["cell"]["type"].as_str().unwrap_or_default().to_string();
        assert_ne!(ty, "llm", "{rel} is an llm cell -- the hive holds no model");
    }
}

/// The vendored schema is the contract, so the contract has to be pinned in
/// three places at once: the file says 1.1.0, the `gate` script's literal
/// section list IS that file's top-level shape, and every seeded document
/// stays inside it. Swap the file without moving the validator and this test
/// is the thing that notices.
#[test]
fn the_vendored_aieos_schema_pins_the_validator_and_the_seed() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let schema = read_json(&root.join(VENDORED_SCHEMA));
    assert_eq!(
        schema["standard"]["protocol"].as_str(),
        Some("AIEOS"),
        "the vendored copy must be an AIeOS document"
    );
    assert_eq!(
        schema["standard"]["version"].as_str(),
        Some("1.1.0"),
        "the vendored schema is version-pinned; moving it is a deliberate act"
    );
    let mut sections: Vec<String> = schema
        .as_object()
        .expect("schema object")
        .keys()
        .cloned()
        .collect();
    sections.sort();

    // 1. The gate's literal list is the file's shape.
    let gate = read_json(&root.join("gate/config.json"));
    let script = gate["params"]["script_inline"]
        .as_str()
        .expect("gate script")
        .to_string();
    let start = script.find("SECTIONS = (").expect("gate declares SECTIONS");
    let end = start + script[start..].find(')').expect("SECTIONS closes");
    let mut declared: Vec<String> = script[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.to_string())
        .collect();
    declared.sort();
    assert_eq!(
        declared, sections,
        "the gate's mandatory-section list drifted from the vendored schema"
    );

    // 2. No seeded document leaves the vendored shape, and every one of them
    //    satisfies the four mandatory paths the gate enforces.
    let raw = std::fs::read_to_string(root.join("store/seed/entities.jsonl")).unwrap();
    let mut checked = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).unwrap();
        // Line 1 of a seed file is the column-type header, not a row.
        if row.get("schema").is_some() {
            continue;
        }
        let doc = &row["aieos"];
        for key in doc.as_object().expect("aieos object").keys() {
            assert!(
                sections.contains(key),
                "seed entity {} carries section {key}, which the vendored 1.1.0 schema does not know",
                row["entity_id"]
            );
        }
        assert_eq!(doc["standard"]["protocol"].as_str(), Some("AIEOS"));
        assert!(
            doc["standard"]["version"]
                .as_str()
                .unwrap_or_default()
                .starts_with("1."),
            "seed entity {} is not a 1.x document",
            row["entity_id"]
        );
        assert!(
            !doc["metadata"]["instance_id"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "seed entity {} has no instance_id",
            row["entity_id"]
        );
        assert!(
            doc["identity"]["names"].is_object(),
            "seed entity {} has no identity.names",
            row["entity_id"]
        );
        checked += 1;
    }
    assert!(checked >= 2, "the seed pin swept almost nothing: {checked}");

    // 3. SCHEMA-ONLY (ruling L4): no cell of this template reaches aieos.org.
    //    The only place the domain may appear is the vendored document itself.
    for rel in AFFINITY_FILES {
        let raw = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(
            !raw.contains("aieos.org"),
            "{rel} names aieos.org -- the schema is VENDORED, there is no network path"
        );
    }
}

/// The write half of the sovereignty contract, in one proposal: a valid
/// document enters through the ONE write port, lands in the store, and leaves
/// an audit line naming the actor the EDGE promoted -- not one the body could
/// have claimed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gate_writes_a_valid_entity_and_audits_it() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    let op = json!({"op": "upsert_entity", "kind": "person",
                    "display_name": "Kim Vale", "owner_member": "member:alex",
                    "aieos": valid_aieos("aaaa-1111", "Kim"),
                    "mx": {"provenance": "test"}});
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
        "a document with all mandatory paths is accepted: {payload}"
    );
    let eid = payload["entity_id"]
        .as_str()
        .expect("the gate mints and returns the entity_id")
        .to_string();
    assert!(eid.starts_with("entity:"), "minted id: {eid}");

    // 1. The row is in the store, active, with the document unchanged.
    let rows = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "entities",
               "columns": ["entity_id", "display_name", "aieos", "status", "aieos_version"],
               "where": {"entity_id": eid.clone()}, "limit": 5}),
    )
    .await;
    assert_eq!(rows.as_array().map(|a| a.len()), Some(1), "rows: {rows}");
    assert_eq!(rows[0]["display_name"].as_str(), Some("Kim Vale"));
    assert_eq!(rows[0]["status"].as_str(), Some("active"));
    assert_eq!(rows[0]["aieos_version"].as_str(), Some("1.1.0"));
    // A `json` column comes back as the TEXT the store kept, so the document is
    // parsed here -- and that it parses AND still says "Kim" is the claim: the
    // gate stored the foreign document unchanged.
    let stored: Value =
        meclaw_core::serde_json::from_str(rows[0]["aieos"].as_str().unwrap_or_default())
            .expect("the stored aieos column is a JSON document");
    assert_eq!(
        stored["identity"]["names"]["first"].as_str(),
        Some("Kim"),
        "the AIeOS document is stored UNCHANGED, which is what makes it a foreign spec"
    );
    assert_eq!(stored["standard"]["protocol"].as_str(), Some("AIEOS"));

    // 2. And the audit line exists, with the actor the edge promoted.
    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "action", "subject", "outcome", "reason_code"],
               "where": {"subject": eid.clone()}, "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    assert_eq!(audit[0]["outcome"].as_str(), Some("ok"));
    assert_eq!(audit[0]["action"].as_str(), Some("upsert_entity"));
    assert_eq!(
        audit[0]["actor"].as_str(),
        Some("member:alex"),
        "the actor came from context, which only an edge can write: {audit}"
    );

    h.shutdown().await;
}

/// The other half: the store enforces no schema, so if the gate lets a broken
/// document through, nothing else will stop it. A missing `metadata.instance_id`
/// writes NO entity row and one audit line that says which path failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gate_refuses_a_mandatory_path_violation_and_writes_nothing() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    let mut doc = valid_aieos("will-be-removed", "Noor");
    doc["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("instance_id");
    let op = json!({"op": "upsert_entity", "kind": "person",
                    "display_name": "Noor Vale", "aieos": doc});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;

    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(payload["outcome"].as_str(), Some("rejected"), "{payload}");
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("aieos_metadata_instance_id"),
        "the refusal names the path that failed, not just 'invalid': {payload}"
    );

    let rows = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "entities",
               "columns": ["entity_id"], "where": {"display_name": "Noor Vale"},
               "limit": 5}),
    )
    .await;
    assert_eq!(
        rows.as_array().map(|a| a.len()),
        Some(0),
        "a refused document reaches the store nowhere: {rows}"
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["outcome", "reason_code", "action"],
               "where": {"outcome": "invalid"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("aieos_metadata_instance_id"),
        "a refusal is the more interesting half of the log: {audit}"
    );

    h.shutdown().await;
}

/// The read port, both directions of its one rule. A disclosed audience gets
/// exactly the fields its rows named -- and an audience nobody decided about
/// gets nothing at all, without the document ever being read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_brief_answers_a_disclosed_audience_and_a_stranger_gets_nothing() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    // 1. The seeded agent asks about the seeded person. Its disclosure rows
    //    name the names, the text style, the idiolect and a SUMMARY of the
    //    favourites -- and nothing else.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let answer = recv_route(&mut rx, "answer").await;
    let system = body_of(&answer)
        .get("system")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        system.is_object(),
        "a disclosed audience gets a pack: {system}"
    );
    assert_eq!(
        system["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the disclosed field arrived: {system}"
    );
    assert_eq!(
        system["channel"]["channel"].as_str(),
        Some("telegram"),
        "the channel slot carries the channel it was asked for: {system}"
    );
    assert!(
        system["channel"]["text_style"].is_object(),
        "linguistics.text_style is disclosed to this audience: {system}"
    );
    let dump = meclaw_core::serde_json::to_string(&system).unwrap();
    // The seeded document HAS these; no disclosure row names them, so they are
    // absent. Not redacted, not summarised -- absent.
    for undisclosed in ["INTP", "neutral good", "Example City", "1980-04-12"] {
        assert!(
            !dump.contains(undisclosed),
            "an undisclosed value reached the pack ({undisclosed}): {dump}"
        );
    }
    assert!(
        dump.contains("<summary of"),
        "the `summarize` mode arrived as a summary rather than as the values: {dump}"
    );

    // 2. The same subject, an audience nobody wrote a row for. Fail-closed
    //    means NOTHING -- no system slot at all, not an empty one.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:someone-else","subject":"entity:alex"}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "an unknown audience must not get a system slot at all: {:?}",
        body_of(&denied)
    );
    assert_eq!(
        turn_text(&denied),
        "nothing is disclosed to this audience",
        "and the tool_result says so instead of leaking the reason"
    );

    // 3. The denial is in the log, and it is a DENIAL, not an error.
    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "outcome", "reason_code"],
               "where": {"actor": "agent:someone-else"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit.as_array().map(|a| a.len()), Some(1), "audit: {audit}");
    assert_eq!(audit[0]["outcome"].as_str(), Some("denied"));
    assert_eq!(audit[0]["reason_code"].as_str(), Some("not_disclosed"));

    h.shutdown().await;
}

/// Relations are a table cut on the `traverse` signature, so "who is Sam to
/// Alex" is answered by the store. Both hops come back from ONE op, each with
/// the path that produced it -- which is the difference between a graph and a
/// list of neighbours.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hops_of_relations_come_back_from_one_store_op() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx) = boot(&td).await;

    // The owner holds a `*` disclosure row on itself, so this is the audience
    // for which the relationship slot is released.
    h.send(to(
        "/asker",
        r#"{"audience":"member:alex","subject":"entity:alex","slots":["relationship"]}"#,
    ))
    .await;
    let answer = recv_route(&mut rx, "answer").await;
    let system = body_of(&answer)
        .get("system")
        .cloned()
        .unwrap_or(Value::Null);
    let rels = system["relationship"]["relations"]
        .as_array()
        .unwrap_or_else(|| panic!("the relationship slot: {system}"))
        .clone();

    let hop1 = rels
        .iter()
        .find(|r| r["to"].as_str() == Some("entity:robin"))
        .unwrap_or_else(|| panic!("hop 1 missing: {rels:?}"));
    assert_eq!(hop1["depth"].as_i64(), Some(1));
    assert_eq!(hop1["kind"].as_str(), Some("parent_of"));

    let hop2 = rels
        .iter()
        .find(|r| r["to"].as_str() == Some("entity:sam"))
        .unwrap_or_else(|| panic!("hop 2 missing -- the second hop is the whole point: {rels:?}"));
    assert_eq!(
        hop2["depth"].as_i64(),
        Some(2),
        "Sam is two parent_of hops from Alex: {hop2}"
    );
    assert_eq!(
        hop2["path"].as_array().map(|a| a.len()),
        Some(3),
        "and the path says HOW, which is what makes 'who is Sam to Alex' answerable: {hop2}"
    );
    assert_eq!(hop2["path"][1].as_str(), Some("entity:robin"));

    // Both depths in one answer means one traverse produced them: the brief
    // emits exactly one relations read per request, and it is this op.
    assert!(
        rels.iter().any(|r| r["depth"].as_i64() == Some(1))
            && rels.iter().any(|r| r["depth"].as_i64() == Some(2)),
        "one op, two depths: {rels:?}"
    );

    h.shutdown().await;
}

/// Push-on-change, both halves. A subscription whose subject has never been
/// rendered is a change, so the first tick re-briefs it; the second tick
/// computes the hash the first one stored and therefore says nothing at all.
/// That silence is what makes a short cadence affordable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_fires_on_a_changed_pack_and_stays_silent_without_one() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    // Two seconds, so the test sees several ticks inside its own budget.
    build_tree(&td, &root, "*/2 * * * * *");
    let (h, mut rx) = boot(&td).await;

    let op = json!({"op": "subscribe", "cell_path": "/main/consumer",
                    "subject": "entity:alex", "audience": "agent:aiden",
                    "channel": "telegram", "slots": ["identity", "channel"]});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(&mut rx, "ack").await;
    assert_eq!(turn_json(&ack)["outcome"].as_str(), Some("accepted"));

    // 1. The next tick finds a subscriber whose stored hash is empty -- a
    //    change by definition -- and exactly one re-brief comes out, rendered
    //    by ./brief and addressed at the subscriber.
    let pushed = recv_route(&mut rx, "answer").await;
    assert_eq!(
        hop_of(&pushed, "subscriber"),
        "/main/consumer",
        "a push-driven brief names its subscriber, which is how the parent's \
         edge finds the llm cell: {:?}",
        pushed.headers.hop
    );
    let system = body_of(&pushed)
        .get("system")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        system["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the pushed pack went through the SAME audience filter: {system}"
    );
    assert!(
        system.get("relationship").is_none(),
        "and through the same slot selection -- the subscription asked for two \
         slots: {system}"
    );

    // 2. The stored hash moved, so every further tick is silent. Several
    //    two-second ticks fit in this window; not one of them may speak.
    let quiet = tokio::time::timeout(Duration::from_secs(7), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "a tick over unchanged data must emit nothing at all, got {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    // 3. And the hash really is what stopped it, not a dead lane.
    let subs = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "subscribers",
               "columns": ["cell_path", "pack_hash", "status"],
               "where": {"status": "active"}, "limit": 5}),
    )
    .await;
    assert_eq!(subs.as_array().map(|a| a.len()), Some(1), "subs: {subs}");
    assert_eq!(
        subs[0]["pack_hash"].as_str().map(|s| s.len()),
        Some(64),
        "the first tick wrote the sha256 of what it sent: {subs}"
    );

    h.shutdown().await;
}
