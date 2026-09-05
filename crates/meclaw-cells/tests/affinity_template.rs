//! meclaw-os -- the shipped `affinity` template, the first hive with a
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
//! **R2b guard (GH #49 form).** `affinity` is PRIVATE -- it is not in
//! `PUBLIC_TEMPLATES`, so it does not travel with the export. Every read below
//! is guarded per file by [`shipped_affinity`]; in the public clone the guard
//! exits cleanly and these tests skip instead of failing on a dead
//! `templates/` reference. Same form as `cogny_template.rs` carried BEFORE its
//! public switch, and the matching `ALLOWED_HITS` entry lives in the
//! maintainers' export script.

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
    "porter/config.json",
];

/// The non-`config.json` files the template ships: five seed tables and the
/// vendored schema. The seed is data, the schema is a pinned copy of a foreign
/// spec -- neither is read at runtime by a cell, and the schema is never
/// fetched from the network.
///
/// `subscribers.jsonl` is the fifth and the only one that is a WIRING birth
/// state rather than a data one (GH #453): one active row, so the push lane
/// carries traffic on the first tick of a fresh hive and silence over unchanged
/// data is silence rather than emptiness. Every test in this file that ticks
/// therefore has that row in it as well -- it is addressed at a token no graph
/// here names, which is the documented undeliverable case (GH #289) and is why
/// the assertions below count subscriptions instead of assuming one.
const AFFINITY_SEEDS: &[&str] = &[
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/subscribers.jsonl",
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
///
/// GH #306: the ROUND rides on the hop the same way and for the same reason.
/// A caller that names no round is a caller whose room nobody knows, so the
/// default here is the one a 1:1 lane means -- the asker alone, spelled out --
/// and the literal string `"unset"` is the test-only way to send a request that
/// declares nothing at all.
///
/// GH #330: the round has ONE name now, `audience_set`, and this asker writes
/// it under that name and no other -- the request field is spelled the same way
/// the hop key is. The `spelling` override exists for exactly one test: the
/// retired key has to be sendable for its death to be provable, and no shipped
/// producer has a reason to write it.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
req = {"subject": a.get("subject"), "channel": a.get("channel") or "*"}
if a.get("slots") is not None:
    req["slots"] = a.get("slots")
h = {"route": "brief", "audience": str(a.get("audience") or "")}
round_ = a.get("audience_set")
if round_ != "unset":
    if round_ is None:
        round_ = [str(a.get("audience") or "")]
    h["participants" if a.get("spelling") == "participants" else "audience_set"] = json.dumps(round_)
if a.get("pinned") is not None:
    # A round the PORT EDGE pins into context, beside whatever the hop says.
    # One context key, because there is one spelling: `pinned_aud` is what the
    # door edge promotes to `context.audience_set`.
    h["pinned_aud"] = json.dumps(a.get("pinned"))
sys.stdout.write(json.dumps({
    "header": h,
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "b1",
                  "text": json.dumps(req)}]}))
"#;

/// The writing side: the same shape for the write port, with the actor on the
/// hop so the port edge can make it edge truth.
///
/// GH #288: the SUBSCRIBER rides on the hop the same way and for the same
/// reason. A subscription names a cell that will be handed briefs about a
/// person -- that address is a routing decision, so it belongs to the edge, not
/// to a body an llm may have written. The request JSON picks it here only so a
/// test can choose what the hop says; a shipped producer stamps it from its own
/// wiring.
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

/// The ports around the hive -- every one at the hive PATH, plus the probe pair
/// that is the test's own read channel. The template draws no edge that appears
/// here, and this graph names no cell inside it bar the probe's store read.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // Every contract lane names the HIVE (overview § Die Hive-Grenze). The
        // caller asserts the lane with a set_hop; which cell behind the door
        // serves it is not written into this graph, which is what makes the
        // template replaceable.
        // ── in_brief: the asker's identity becomes EDGE truth, and only here ──
        // GH #306: this edge also PINS a round into context when the request
        // asked for one. That is the caller-edge spelling of the round, and
        // the door must prefer it over anything on the hop -- an edge is
        // written by the colony, a hop key by whatever cell it passed. GH #330
        // leaves ONE context key to pin it under: `audience_set` is the name
        // every shipped producer promotes (ADR-0002 E8), and a second spelling
        // beside it only ever bought a precedence rule nobody could state.
        // Empty otherwise, so the door falls through to the hop.
        {"from": "./asker", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_hop": {"route": "'in_brief'"},
                      "set_context": {
                          "asker": "hop.audience",
                          "audience_set": "has(hop.pinned_aud) ? hop.pinned_aud : ''"}}},
        // ── in_propose: the writer's identity, likewise from the edge ──
        // GH #288: and the SUBSCRIBER address alongside it, for the same
        // reason the actor is here -- a subscription hands somebody's briefs
        // to a cell path, so that path has to be edge truth. The `has()` guard
        // is not decoration: an unresolvable `set_context` expression makes the
        // modifier fail and the edge is SKIPPED, so a request without the key
        // would vanish instead of being refused.
        {"from": "./writer", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'propose'",
         "modifier": {"set_hop": {"route": "'in_propose'"},
                      "set_context": {
                          "actor": "hop.actor",
                          "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}},
        // ── out_brief / out_push: TWO exits, told apart by hop.subscriber ──
        // GH #289: the hive answers on one route with two meanings. A
        // `tool_result` answers a call somebody opened and belongs to the
        // caller that opened it; a push carries `system.*` for the subscriber
        // `./push` chose and belongs to THAT address. One unconditioned
        // `hop.route == 'answer'` edge collects both -- which is the instance
        // defect this issue measured in the wild: the push lands wherever the
        // tool lane happens to point, and the subscriber's llm cell never sees
        // its own slot update. So the discriminator is written into the graph,
        // not left to the reader: the empty subscriber is the tool lane, a
        // named one is the push lane, and an answer whose subscriber matches
        // NEITHER is delivered nowhere rather than to the wrong place.
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == ''"},
        {"from": "./affinity", "to": "/pushsink",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == '/main/consumer'"},
        // ── out_ack ──
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        // ── out_error: ONE edge. Three of them -- one per code cell inside --
        //    was the old shape, and once all three ends name the hive the same
        //    failure is delivered three times.
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── the test's own read channel, straight into the store. The one
        //    reach past the doors this file keeps: a test that may only see the
        //    contract cannot check what the contract wrote.
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
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["brief"],
            json!({"audience": {"type": "string", "required": false},
                   "audience_set": {"type": "string", "required": false},
                   "pinned_aud": {"type": "string", "required": false}}),
        ),
    );
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
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["pstore"], json!({})),
    );
    copy_cells(root_template, &root.join("main/affinity"));
    // `${uuid7:…}` is an INSTANTIATION-side substitution (the mutation path
    // mints it); a raw directory copy bootstrapped from the filesystem has to
    // be handed a literal, exactly as `cogny_template.rs` hands its brains a
    // base_url. The cron is written here too: since GH #138 it is a literal of
    // `./clock`'s own params, and this is the form `override_params` takes at
    // instantiation. An `AFFINITY_PUSH_CRON=` line in the `.env` would be read
    // by nothing at all and would say nothing about it.
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000000af");
        v["params"]["schedules"][0]["cron"] = json!(cron);
    });
}

/// The colony, plus the TWO answer sinks the split exit (GH #289) needs: the
/// tool lane lands at `/sink`, the push lane at `/pushsink`. Every test takes
/// both, so a message that picks the wrong door is a message that goes missing
/// in the test that owns it rather than one that quietly arrives anyway.
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

/// Six cells, no more and no less. Pinned as a SET, not as a floor: a hive
/// that grew an `llm` would still pass every round below -- it would just stop
/// being the thing whose whole cost argument is that it holds no model. The
/// sixth arrived with 3.2.0 (GH #471): `porter`, which walks the record out as
/// a document and takes one back, and it is a `code` cell like the other four.
#[test]
fn the_hive_carries_six_cells_and_no_model() {
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
        "affinity is store + brief + gate + push + clock + porter: no curator, \
         no brain"
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
    let (h, mut rx, _push_rx) = boot(&td).await;

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
    let (h, mut rx, _push_rx) = boot(&td).await;

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
    let (h, mut rx, _push_rx) = boot(&td).await;

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

/// GH #306 -- the audience-SET rule (R-AF-3) on the SHIPPED topology, both
/// halves of it.
///
/// Until this issue the refusal half was dead code in every colony that ever
/// booted: `brief` read the round out of context, and **no edge anywhere
/// promoted that key**. So `present` was always `{asker}`, the widest reading a
/// subset test can be given, and a four-person round reading a row released to
/// three of them was served with the fourth participant unreportable.
///
/// GH #330: the round is spelled `audience_set` here and everywhere -- one
/// canonical key, on the hop and in context alike.
///
/// Two things are pinned here, and neither of them is reachable through a
/// script-level test: the DOOR (`. -> ./brief`) has to carry the promotion, and
/// a round nobody declared has to be refused rather than narrowed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_wider_than_the_release_is_refused_at_the_shipped_door() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // The seeded rows for `entity:alex` are released to `{agent:aiden}` alone.
    // Four people are in this room. {aiden,alex,robin,sam} ⊄ {aiden} -- so the
    // release the agent normally reads does not cover this round.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "audience_set":["agent:aiden","member:alex","member:robin","member:sam"]}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "a round the release never saw must get NO pack: {:?}",
        body_of(&denied)
    );
    assert_eq!(
        turn_text(&denied),
        "nothing is disclosed to this audience",
        "and the same one sentence every other denial gets"
    );

    // The audit says WHICH denial it was: released to a smaller round, not
    // never released at all. That difference is the whole reason the two codes
    // exist.
    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "outcome", "reason_code"],
               "where": {"actor": "agent:aiden"}, "limit": 5}),
    )
    .await;
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("audience_not_subset"),
        "the newcomer must close the release: {audit}"
    );

    // The same question in the room the release WAS given in still works --
    // otherwise the gate above would be a lock, not a rule.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let served = recv_route(&mut rx, "answer").await;
    assert_eq!(
        body_of(&served)["system"]["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the declared 1:1 round is inside the release: {:?}",
        body_of(&served)
    );

    h.shutdown().await;
}

/// GH #306 -- and a round nobody declared is REFUSED, never narrowed.
///
/// "The asker alone" is the widest reading a subset test can be handed: every
/// release that names the asker passes it, whoever else is actually in the
/// room. So an undeclared round cannot be a default -- it is the one state in
/// which the rule cannot be applied at all, and the honest answer to that is a
/// denial with its own reason code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_nobody_declared_is_refused_rather_than_narrowed_to_the_asker() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // `"unset"` makes the test asker stamp NO round on the hop at all -- the
    // shipped wiring of every colony before this issue.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "audience_set":"unset"}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "an undeclared round must not be served the asker's own releases: {:?}",
        body_of(&denied)
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["actor", "outcome", "reason_code"],
               "where": {"actor": "agent:aiden"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit[0]["outcome"].as_str(), Some("denied"), "{audit}");
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("no_round"),
        "a missing round is its own denial -- not `not_disclosed`, which would \
         claim nobody released anything: {audit}"
    );

    h.shutdown().await;
}

/// GH #330 -- the RETIRED spelling is not a round.
///
/// Q12 rules `audience_set` the one canonical name for the round; `participants`
/// is retired. Retired has to mean *inert*, not *deprecated*: as long as the
/// door still reads the old key, every cell that stamps it keeps deciding who
/// is in the room, and the migration is a rename nobody has to follow.
///
/// So the check is the fail-closed one and not a warning. A request whose ONLY
/// round is `hop.participants` -- four people, no `audience_set` anywhere -- is
/// a request that declared no round at all, and the honest answer to that is
/// the `no_round` denial, not the asker alone (which passes every 1:1 release)
/// and not the four-person set the dead key names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_retired_spelling_is_not_a_round() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // `spelling` puts the round under the dead key and nowhere else. Nothing
    // pins a context round, so `hop.participants` is the only candidate in
    // play -- and after the migration it is not a candidate at all.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "spelling":"participants",
            "audience_set":["agent:aiden","member:alex","member:robin","member:sam"]}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "the retired key must not be read as a round -- and must not be read as \
         the asker alone either: {:?}",
        body_of(&denied)
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["outcome", "reason_code"],
               "where": {"actor": "agent:aiden"}, "limit": 5}),
    )
    .await;
    assert_eq!(audit[0]["outcome"].as_str(), Some("denied"), "{audit}");
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("no_round"),
        "a request carrying only the retired key declared NO round -- \
         `audience_not_subset` would mean the dead key had been read as one, \
         and `not_disclosed` would claim nobody released anything: {audit}"
    );

    h.shutdown().await;
}

/// GH #306 / GH #330 -- an edge-pinned round outranks a hop key.
///
/// A `set_context` value on an edge is written by the colony; a hop key is
/// written by whatever cell the message passed, up to and including one that
/// runs a model's output. So where both exist the edge wins and the hop only
/// fills the gap -- otherwise a cell downstream of the pinning edge could widen
/// its own room by stamping a hop key, which is the body-writes-its-own-identity
/// hazard the whole "identity comes from the edge" rule exists to prevent.
///
/// With one spelling left this is the ONLY precedence claim there is to pin:
/// the four-step chain (`context.participants` → `context.audience_set` →
/// `hop.participants` → `hop.audience_set`) collapses to two steps, context
/// before hop. That collapse is also what makes the old tier-beats-spelling
/// warning moot, and the warning is worth keeping as a doc note: while two
/// spellings existed, ranking them per-spelling instead of per-TIER let a cell
/// stamping the hop outrank an edge-pinned context round -- the hazard would
/// have survived untouched on the very spelling every real colony carries,
/// behind a fix that read correct. One name per fact is what removes that
/// failure mode rather than papering over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_pinned_round_outranks_the_hop_key() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // The port edge pins a four-person round into `context.audience_set`. The
    // cell stamps a cosy 1:1 on `hop.audience_set` -- the audience the seed
    // released to. The pinned round must win, so this is refused, not served.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "audience_set":["agent:aiden"],
            "pinned":["agent:aiden","member:alex","member:robin","member:sam"]}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "a hop key must not be able to shrink the room an edge pinned: {:?}",
        body_of(&denied)
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["outcome", "reason_code"],
               "where": {"actor": "agent:aiden", "outcome": "denied"}, "limit": 5}),
    )
    .await;
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("audience_not_subset"),
        "{audit}"
    );

    h.shutdown().await;
}

/// GH #306 / GH #330 -- `audience_set`, promoted where it actually lives.
///
/// Every shipped producer promotes the round to **context**
/// (`session-keeper`, `memory-drain`, the receptionist ingress per ADR-0002 E8,
/// talky's own port edge). So the leg that carries every real colony is
/// `context.audience_set`, and it gets its own round trip through both
/// directions of the rule -- because a key that only ever passes is a key that
/// disabled the rule.
///
/// GH #330 merged the hop-side twin (`the_audience_set_spelling_is_a_round_in_
/// both_directions`) into this one. While two spellings existed the pair
/// proved the alias was honoured on both legs; with one canonical name the hop
/// leg is what `a_round_wider_than_the_release_is_refused_at_the_shipped_door`
/// already exercises in both directions, and a second copy of it proves
/// nothing the door test does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_edge_pinned_audience_set_is_a_round_in_both_directions() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // `"audience_set":"unset"` keeps the hop key off the wire, so the pinned
    // context round is the ONLY round in play.
    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "audience_set":"unset","pinned":["agent:aiden"]}"#,
    ))
    .await;
    let served = recv_route(&mut rx, "answer").await;
    assert_eq!(
        body_of(&served)["system"]["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "an edge-pinned audience_set must BE the round: {:?}",
        body_of(&served)
    );

    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram",
            "audience_set":"unset",
            "pinned":["agent:aiden","member:alex","member:robin","member:sam"]}"#,
    ))
    .await;
    let denied = recv_route(&mut rx, "answer").await;
    assert!(
        body_of(&denied).get("system").is_none(),
        "and it must carry the refusal half: {:?}",
        body_of(&denied)
    );

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit",
               "columns": ["outcome", "reason_code"],
               "where": {"actor": "agent:aiden", "outcome": "denied"}, "limit": 5}),
    )
    .await;
    assert_eq!(
        audit[0]["reason_code"].as_str(),
        Some("audience_not_subset"),
        "{audit}"
    );

    h.shutdown().await;
}

/// GH #306 -- the door resets the lane's own state keys.
///
/// `context` travels colony-wide: every cell emission carries the context it
/// was handed. A caller that once spoke to this hive (or to anything that left
/// `aff_*` behind) would hand a FRESH `in_brief` an inherited `aff_phase`, and
/// `brief` would then read it as a mid-lane echo -- entering the disclosure
/// phase with a stale carry, or answering on the push lane because
/// `aff_subscriber` still named somebody. The internal `./push -> ./brief` edge
/// has always reset these; the door had not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door_resets_an_inherited_lane_state() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // The probe's edge leaves `context.affinity_origin` behind, and the store
    // answer that comes back through it carries a full `aff_*` set on the way
    // to ./brief -- so a colony where a request follows a store round trip is
    // exactly where an inherited phase comes from. Here the inheritance is
    // asserted directly: the request travels with `aff_phase` already set.
    let mut m = to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    );
    m.headers
        .context
        .insert("aff_phase".into(), json!("disclosure"));
    m.headers
        .context
        .insert("aff_carry".into(), json!("{\"trust\":\"intimate\"}"));
    m.headers
        .context
        .insert("aff_subscriber".into(), json!("/main/somebody-else"));
    h.send(m).await;

    let answer = recv_route(&mut rx, "answer").await;
    assert_eq!(
        hop_of(&answer, "subscriber"),
        "",
        "an inherited subscriber must not turn a tool answer into somebody \
         else's push: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        body_of(&answer)["system"]["identity"]["names"]["first"].as_str(),
        Some("Alex"),
        "the request ran as a FRESH lane, not as an echo of an inherited \
         phase: {:?}",
        body_of(&answer)
    );

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
    let (h, mut rx, _push_rx) = boot(&td).await;

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

/// GH #288 -- the write port's identity rule, on the one op that routes.
///
/// `upsert_entity` already takes its actor from the edge, and this file pins
/// that. `subscribe` did not: it read `cell_path` and `audience` out of the
/// body, so anything that could write a tool_call could name the cell that
/// receives somebody's briefs and the audience filter they are cut to. That is
/// the same hole the `actor` rule closes, one op further along -- and worse,
/// because the value is an ADDRESS: the row it writes is what `./push` reads
/// every tick to decide where a pack goes.
///
/// A body that asserts an address is not narrowed to the edge's -- it is
/// REFUSED, with a code of its own. Narrowing would let a caller keep guessing
/// until something stuck; a refusal says the request was shaped wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_asserted_subscriber_is_refused() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // The edge says `/main/consumer` and `member:alex`. The body says somebody
    // else, on both counts -- exactly what an llm-written tool_call can do.
    let op = json!({"op": "subscribe", "cell_path": "/main/somebody-else",
                    "subject": "entity:alex", "audience": "agent:aiden",
                    "subscriber": "/main/consumer"});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;

    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(
        payload["outcome"].as_str(),
        Some("rejected"),
        "a body that names its own subscription address is refused, not \
         silently corrected: {payload}"
    );
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("identity_from_body"),
        "the refusal names WHY -- not `subscription_target_empty`, which would \
         claim the request was incomplete: {payload}"
    );

    let subs = probe(
        &h,
        &mut rx,
        // `columns` is not optional on a select: without it the store refuses
        // with `missing columns array` and the assertion below cannot be
        // reached at all, so the store-side half measures nothing.
        json!({"operation": "select", "table": "subscribers",
               "columns": ["id", "cell_path", "audience"], "limit": 5}),
    )
    .await;
    // The table is not empty at birth (GH #453 seeds one row), so "nothing was
    // written" is asserted as "the birth state, unchanged" rather than as a
    // zero -- a zero here would have stopped being a measurement the day the
    // seed landed.
    assert_eq!(
        subs.as_array().map(|a| a.len()),
        Some(1),
        "a refused subscription reaches the store nowhere -- `./push` must not \
         find a row it would then serve: {subs}"
    );
    assert_eq!(
        subs[0]["id"].as_str(),
        Some("sub:aiden-self"),
        "and the one row that IS there is the seeded one: {subs}"
    );

    h.shutdown().await;
}

/// GH #288 -- the accepting half of the same rule.
///
/// A request that asserts nothing gets the row the EDGE describes: the cell
/// path the port promoted, and the actor the port promoted as the audience the
/// pack is cut to. Both halves matter -- pinning the refusal alone would be
/// satisfied by an op that refuses everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_subscription_row_carries_the_edge_identity() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    // No `cell_path`, no `audience`: the body says WHAT to subscribe to, the
    // edge says WHO is subscribing and where it lives.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "subscriber": "/main/consumer"});
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
        "a request that asserts no identity is a COMPLETE request -- the edge \
         supplies the rest: {payload}"
    );

    let subs = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "subscribers",
               "columns": ["cell_path", "subject", "audience", "status"],
               "limit": 5}),
    )
    .await;
    // Two rows: the one this subscribe wrote, and the seeded birth state the
    // template ships (GH #453). The written one is picked by the address the
    // EDGE named -- which is the fact under test.
    assert_eq!(subs.as_array().map(|a| a.len()), Some(2), "subs: {subs}");
    let written = subs
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|r| r["cell_path"].as_str() == Some("/main/consumer"))
        })
        .unwrap_or_else(|| panic!("no row for the address the edge named: {subs}"));
    assert_eq!(
        written["cell_path"].as_str(),
        Some("/main/consumer"),
        "the address came from the edge, which only the colony writes: {subs}"
    );
    assert_eq!(
        written["audience"].as_str(),
        Some("member:alex"),
        "and the audience is the actor the edge named, not anything the body \
         said: {subs}"
    );
    assert_eq!(written["subject"].as_str(), Some("entity:alex"));

    h.shutdown().await;
}

/// GH #288 -- and no edge subscriber means no subscription.
///
/// The port edge promotes `''` when the hop carries nothing, so the gate has a
/// value that says "nobody named an address". Falling back to the body there
/// would reopen the hole the test above closes, and defaulting to the sender
/// would invent an address nobody asked for. Fail closed, the same way an
/// undeclared round is refused with `no_round` rather than narrowed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscribe_without_an_edge_subscriber_is_refused() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, QUIET_CRON);
    let (h, mut rx, _push_rx) = boot(&td).await;

    let op = json!({"op": "subscribe", "subject": "entity:alex"});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;

    let ack = recv_route(&mut rx, "ack").await;
    let payload = turn_json(&ack);
    assert_eq!(
        payload["reason_code"].as_str(),
        Some("subscriber_not_on_edge"),
        "an undeclared subscriber is its own refusal, so the caller learns \
         which half of the request was missing: {payload}"
    );
    assert_eq!(payload["outcome"].as_str(), Some("rejected"), "{payload}");

    let subs = probe(
        &h,
        &mut rx,
        // `columns` is not optional on a select: without it the store refuses
        // with `missing columns array` and the assertion below cannot be
        // reached at all, so the store-side half measures nothing.
        json!({"operation": "select", "table": "subscribers",
               "columns": ["id", "cell_path", "audience"], "limit": 5}),
    )
    .await;
    assert_eq!(
        subs.as_array().map(|a| a.len()),
        Some(1),
        "and nothing was written on the way to that refusal -- the one row in \
         the table is the seeded birth state (GH #453): {subs}"
    );
    assert_eq!(subs[0]["id"].as_str(), Some("sub:aiden-self"), "{subs}");

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
    // GH #289: the push lane has an exit of its own now, so the re-brief below
    // is drained where it actually lands. The `ack` still comes back at `/sink`
    // -- it is not an answer and takes neither of the two answer edges.
    let (h, mut rx, mut push_rx) = boot(&td).await;

    // GH #288: the body says WHAT is subscribed to, on which channel and in how
    // many slots; WHERE the pushes go and WHO they are cut for come off the
    // edge, so the `subscriber` key here is read by the WRITER stand-in and
    // stamped on the hop, not by the gate. The audience the row lands with is
    // therefore `member:alex`, the actor the port edge named -- and the seeded
    // owner release (`disc:alex-owner`, field_path `*`, audience_set
    // `["member:alex"]`) is what the filter finds for it. That keeps both pack
    // assertions below measuring what they measured: the pack still had to pass
    // the audience filter to exist at all, and with everything disclosed a
    // missing `relationship` slot can ONLY be the slot selection.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "channel": "telegram", "slots": ["identity", "channel"],
                    "subscriber": "/main/consumer"});
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
    let pushed = recv_route(&mut push_rx, "answer").await;
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
    // The push exit is the one a tick can reach, so silence is measured there.
    let quiet = tokio::time::timeout(Duration::from_secs(7), push_rx.recv()).await;
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
               "where": {"cell_path": "/main/consumer", "status": "active"}, "limit": 5}),
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

/// GH #289 -- the answer route carries TWO lanes, and only `hop.subscriber`
/// tells them apart.
///
/// `./affinity` speaks `route: answer` for both a tool call somebody opened and
/// a push nobody asked for. The two are not variants of one message: a
/// `tool_result` closes a fan-in at the caller that opened the call, while a
/// push is `system.*` alone, addressed at the subscriber `./push` read out of
/// its own table. Deliver either one at the other's door and both promises
/// break -- the caller's call never closes, and the subscriber's `llm` cell
/// gets a `tool_result` under a call id it never opened (GH #263).
///
/// The instance defect this issue measured was exactly that: a topology with
/// ONE unconditioned `hop.route == 'answer'` edge. Nothing in it is wrong to
/// read -- it simply cannot express the difference, so the push follows the
/// tool lane. The graph below expresses it, and this test is what keeps it
/// expressed: both lanes are provoked in the same colony, in the same window,
/// and each sink is drained to the end to prove it never saw the other's
/// message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_answer_lanes_are_told_apart_by_the_subscriber_key() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    // A short cadence, so the push lane speaks inside the same window the tool
    // lane does -- the point is the two lanes CROSSING, not either alone.
    build_tree(&td, &root, "*/2 * * * * *");
    let (h, mut rx, mut push_rx) = boot(&td).await;

    // 1. A subscription, with its identity off the edge (GH #288): the body
    //    says WHAT, the `subscriber` key is read by the WRITER stand-in and
    //    stamped on the hop, and the port edge promotes it into context. The
    //    row therefore lands with audience `member:alex` -- the actor the edge
    //    named -- and the seeded `disc:alex-owner` release (field_path `*`,
    //    audience_set `["member:alex"]`) is what lets a pack exist for it.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "channel": "telegram", "slots": ["identity", "channel"],
                    "subscriber": "/main/consumer"});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(&mut rx, "ack").await;
    assert_eq!(
        turn_json(&ack)["outcome"].as_str(),
        Some("accepted"),
        "the subscription has to exist before the push lane can carry anything: {:?}",
        turn_json(&ack)
    );

    // 2. And a brief over the TOOL lane, as the same actor -- so the two
    //    messages are cut from the same disclosure decision and differ in
    //    nothing but their lane.
    h.send(to(
        "/asker",
        r#"{"audience":"member:alex","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;

    // 3. The tool answer at `/sink`: no subscriber, and a `tool_result` under
    //    the id of the call it answers.
    let answer = recv_route(&mut rx, "answer").await;
    assert_eq!(
        hop_of(&answer, "subscriber"),
        "",
        "the tool lane is the lane with NO subscriber -- that emptiness is the \
         discriminator, not a missing value: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        body_of(&answer)["messages"][0]["type"].as_str(),
        Some("tool_result"),
        "a call somebody opened is answered, or the asking fan-in never \
         closes: {:?}",
        body_of(&answer)
    );

    // 4. The push at `/pushsink`: it names the subscriber, and it carries NO
    //    turn at all -- which is the whole reason a slot update costs a write
    //    instead of an inference (GH #263).
    let pushed = recv_route(&mut push_rx, "answer").await;
    assert_eq!(
        hop_of(&pushed, "subscriber"),
        "/main/consumer",
        "the push lane names the address `./push` read out of its own table: {:?}",
        pushed.headers.hop
    );
    assert!(
        body_of(&pushed).get("messages").is_none(),
        "a push carries `system.*` and nothing else; a turn beside it makes \
         the subscriber's llm cell call a provider: {:?}",
        body_of(&pushed)
    );
    assert!(
        body_of(&pushed)["system"]["identity"].is_object(),
        "and it is a real pack, not an empty envelope that would let the \
         assertion above pass for the wrong reason: {:?}",
        body_of(&pushed)
    );

    // 5. Neither door ever saw the other's message. Drain both to the end of a
    //    bounded window and count: every answer at `/sink` is a tool answer,
    //    every answer at `/pushsink` is a push, and there is at least one of
    //    each -- a test that counted zero on both sides would pass trivially.
    let mut tool_answers = 1usize;
    while let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(4), rx.recv()).await {
        if hop_of(&m, "route") != "answer" {
            continue;
        }
        tool_answers += 1;
        assert_eq!(
            hop_of(&m, "subscriber"),
            "",
            "a push reached the TOOL sink -- this is the instance defect of \
             #289, reproduced: {:?}",
            m.headers.hop
        );
    }
    let mut pushes = 1usize;
    while let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(4), push_rx.recv()).await {
        pushes += 1;
        assert_eq!(
            hop_of(&m, "subscriber"),
            "/main/consumer",
            "a tool answer reached the PUSH sink: {:?}",
            m.headers.hop
        );
    }
    assert!(
        tool_answers >= 1 && pushes >= 1,
        "both lanes have to have spoken for the separation to mean anything: \
         {tool_answers} tool answer(s), {pushes} push(es)"
    );

    h.shutdown().await;
}

/// GH #260 — the sovereignty sentence has two halves, and a store that seals
/// one is still writable through the other.
///
/// `params.write_surface: "internal"` (ruling F3 / GH #132) bounds the ops the
/// store's own `handle()` runs. The `transfer` body slot is answered by the
/// SUBSTRATE, in `cell_task`, before `handle()` is ever reached — so an
/// `import` from outside walks past that declaration without ever meeting it.
/// The type-neutral `contract.write_surface` is the half that closes it.
///
/// `clock` carries the contract half as well, and for a reason that is not
/// symmetry: a timer keeps its schedules in its own `cell.db`
/// (`timer::db::load_active_filter_past`), so an imported row is a firing with
/// an `emit_to` of the writer's choosing. The three `code` cells deliberately
/// declare nothing — this hive keeps a lane's state on the wire, so their
/// `cell.db` holds nothing a boundary would protect.
#[test]
fn the_two_cells_with_state_seal_both_write_surfaces() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let store = read_json(&root.join("store/config.json"));
    assert_eq!(
        store["params"]["write_surface"], "internal",
        "only affinity/gate writes into affinity/store"
    );
    assert_eq!(
        store["contract"]["write_surface"], "internal",
        "GH #260: without the substrate half an import writes rows past the one \
         sentence this hive is built on"
    );
    assert_eq!(
        read_json(&root.join("clock/config.json"))["contract"]["write_surface"],
        "internal",
        "GH #260: a timer's cell.db IS its schedule list -- an imported row fires"
    );
    for rel in ["brief/config.json", "gate/config.json", "push/config.json"] {
        let cfg = read_json(&root.join(rel));
        assert!(
            cfg["contract"].get("write_surface").is_none(),
            "{rel} is a code cell whose cell.db this template never uses; a \
             boundary around it would be decoration, not a promise"
        );
    }
}

// ──────────────────────────────── GH #288: the prose and the mechanism, locked

/// The four refusals of `subscribe`, in the order the shipped `gate` checks
/// them. The order is load-bearing prose: a body carrying `cell_path` but no
/// `subject` answers `subscription_target_empty`, not `identity_from_body`, so
/// a README that lists the identity refusals first would describe a gate that
/// does not exist.
const SUBSCRIBE_REFUSALS: &[&str] = &[
    "subscription_target_empty",
    "identity_from_body",
    "subscriber_not_on_edge",
    "actor_not_on_edge",
];

/// The `subscribe` branch of the shipped `gate` script, as source text.
///
/// Sliced between its own `if op ==` line and the next one, so a code that
/// belongs to a neighbouring op cannot be counted as this branch's.
fn subscribe_branch(root: &std::path::Path) -> String {
    let cfg = read_json(&root.join("gate/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("affinity/gate carries its script as `params.script_inline`")
        .to_string();
    let start = script
        .find("if op == \"subscribe\":")
        .expect("the gate script has a `subscribe` branch");
    let rest = &script[start..];
    let end = rest
        .find("if op == \"propose\":")
        .expect("the `propose` branch follows `subscribe` and ends its slice");
    rest[..end].to_string()
}

/// The order in which `needles` appear in `haystack`. A needle that is absent
/// panics -- an unfound string must never look like an ordering result.
fn appearance_order(haystack: &str, needles: &[&str]) -> Vec<String> {
    let mut found: Vec<(usize, String)> = needles
        .iter()
        .map(|n| {
            let at = haystack
                .find(n)
                .unwrap_or_else(|| panic!("`{n}` appears nowhere in:\n{haystack}"));
            (at, (*n).to_string())
        })
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, n)| n).collect()
}

/// GH #288 (§ 2d drift lock) -- the README's `subscribe` row lists the refusals
/// in the order the gate checks them.
///
/// Both halves: the sentence is read out of the shipped README, and the order
/// it claims is compared against the order derived from the shipped script. A
/// reordered branch is red here, and so is a row that renames a code.
#[test]
fn the_subscribe_refusals_are_documented_in_check_order() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let Ok(readme) = std::fs::read_to_string(root.join("README.md")) else {
        return;
    };
    let row = readme
        .lines()
        .find(|l| l.trim_start().starts_with("| `subscribe` |"))
        .expect("the write-ops table has a `subscribe` row");

    let checked = appearance_order(&subscribe_branch(&root), SUBSCRIBE_REFUSALS);
    assert_eq!(
        checked, SUBSCRIBE_REFUSALS,
        "the gate checks the four refusals in a different order than this test \
         records -- move the constant WITH the branch, and the README row with both"
    );
    assert_eq!(
        appearance_order(row, SUBSCRIBE_REFUSALS),
        checked,
        "the README lists the `subscribe` refusals in an order the gate does not \
         check. A body with `cell_path` and no `subject` answers \
         `subscription_target_empty`; a row that puts the identity refusals first \
         tells the reader they outrank it.\n  row: {row}"
    );
}

/// GH #288 (§ 2d drift lock) -- every public surface of this template says
/// where `subscribe`'s two identity facts come from, and the gate takes them
/// from there.
///
/// The prose half sweeps the three public surfaces (README, `template.json`,
/// the `description` block of `gate/config.json`); the mechanism half asserts
/// that the branch reads `context.subscriber` / `context.actor` and reads
/// NEITHER out of the body.
#[test]
fn the_identity_prose_names_the_edge_keys_on_every_public_surface() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let Ok(readme) = std::fs::read_to_string(root.join("README.md")) else {
        return;
    };

    // ── the prose, surface by surface
    let ports_row = readme
        .lines()
        .find(|l| l.trim_start().starts_with("| `in_propose` |"))
        .expect("the ports table has an `in_propose` row");
    for key in ["context.actor", "context.subscriber"] {
        assert!(
            ports_row.contains(key),
            "the `in_propose` port row must name `{key}` -- an edge that does not \
             promote it makes every `subscribe` fail closed:\n  {ports_row}"
        );
    }
    let identity_section = readme
        .split("### Identity comes from the edge, never from the body")
        .nth(1)
        .expect("§ Identity comes from the edge is where this rule lives");
    let identity_section = identity_section
        .split("\n### ")
        .next()
        .expect("a section ends at the next one");
    assert!(
        identity_section.contains("context.subscriber"),
        "§ Identity comes from the edge names `brief`'s two keys and `gate`'s \
         actor but not the subscriber address -- the third case of the same rule"
    );

    let manifest = read_json(&root.join("template.json"));
    let use_when = manifest["description"]["use_when"]
        .as_str()
        .expect("template.json has a string `use_when`");
    let ports_example = manifest["description"]["examples"][0]
        .as_str()
        .expect("examples[0] is the PORTS paragraph");
    for (what, text) in [("use_when", use_when), ("examples[0]", ports_example)] {
        assert!(
            text.contains("context.subscriber"),
            "template.json `description.{what}` describes the propose ingress \
             without naming `context.subscriber`"
        );
    }

    let gate = read_json(&root.join("gate/config.json"));
    for what in ["use_when", "consumes_meaning"] {
        let text = gate["description"][what]
            .as_str()
            .unwrap_or_else(|| panic!("gate/config.json has a string `description.{what}`"));
        assert!(
            text.contains("context.subscriber"),
            "gate/config.json `description.{what}` does not name `context.subscriber`"
        );
    }
    let subscribe_example = gate["description"]["examples"]
        .as_array()
        .expect("gate/config.json has a `description.examples` array")
        .iter()
        .filter_map(Value::as_str)
        .find(|e| e.contains("\"op\":\"subscribe\""))
        .expect("one example shows the `subscribe` call shape");
    for banned in ["cell_path", "audience"] {
        assert!(
            !subscribe_example.contains(&format!("\"{banned}\"")),
            "the documented `subscribe` call still passes `{banned}` -- a caller \
             copying it gets `identity_from_body`:\n  {subscribe_example}"
        );
    }

    // ── the mechanism the prose describes
    assert_eq!(
        gate["contract"]["consumes"]["context"]["subscriber"]["type"], "string",
        "the key the prose promises must be DECLARED, or the edge promoting it \
         is a key nobody asked for (GH #292)"
    );
    let branch = subscribe_branch(&root);
    assert!(
        branch.contains("\"cell_path\": subscriber") && branch.contains("\"audience\": actor"),
        "the written row must carry the edge's two values:\n{branch}"
    );
    for body_read in ["a.get(\"cell_path\")", "a.get(\"audience\")"] {
        assert!(
            !branch.contains(body_read),
            "`{body_read}` is back in the `subscribe` branch -- the body is \
             model-writable and the row it would write is the address ./push \
             delivers to"
        );
    }
}

/// GH #330 (R6 drift lock) -- the round has exactly ONE name on every public
/// surface of this template, and the retraction is stated rather than implied.
///
/// Two sentences of the README are behaviour-describing public promises: that
/// `context.participants` is *retired, not aliased* (a request spelling the
/// round that way is refused `no_round`), and that no template may ever
/// introduce a second name for it. Both halves are locked here.
///
/// The prose half sweeps README and `template.json` for the retraction and for
/// the second-name sentence, and asserts that no public surface still tells a
/// builder to send the dead key. The mechanism half asserts the door: its
/// `set_context` promotes `audience_set` and mentions `participants` nowhere in
/// a modifier expression, and the declared `contract.accepts[0].context` is
/// exactly the two surviving keys.
#[test]
fn the_round_has_one_name_on_every_public_surface() {
    let Some(root) = shipped_affinity() else {
        return;
    };
    let Ok(readme) = std::fs::read_to_string(root.join("README.md")) else {
        return;
    };
    let manifest = read_json(&root.join("template.json"));
    let use_when = manifest["description"]["use_when"]
        .as_str()
        .expect("template.json has a string `use_when`");
    let ports_example = manifest["description"]["examples"][0]
        .as_str()
        .expect("examples[0] is the PORTS paragraph");

    // ── the two promises
    assert!(
        readme.contains("it is retired, not aliased"),
        "the README no longer states the retraction. `participants` is not an \
         alias and never became one -- a reader who is not told that wires the \
         dead key and gets `no_round`"
    );
    assert!(
        readme.contains("No template may ever introduce a second name for it"),
        "the README dropped the second-name rule (GH #330). It is the reason the \
         retraction is worth anything: one more spelling is one more gate that \
         can stand open while the first reads shut"
    );
    for (what, text) in [("use_when", use_when), ("examples[0]", ports_example)] {
        assert!(
            text.contains("retired, not aliased"),
            "template.json `description.{what}` describes the brief ingress \
             without the retraction -- this text is what a builder reads"
        );
    }

    // ── and nothing still asks a caller for the dead key
    for (what, text) in [
        ("README.md", readme.as_str()),
        ("template.json use_when", use_when),
        ("template.json examples[0]", ports_example),
    ] {
        // `hop.participants` alone carries this guard: it is the one spelling that
        // appears in NO surviving sentence, so any resurrection of the four-step
        // chain -- whichever arrow or fence it is drawn with -- has to name it.
        // `context.participants` cannot be banned here, the README names it to say
        // it is retired.
        let dead = "hop.participants";
        assert!(
            !text.contains(dead),
            "`{dead}` is back on {what} -- the four-step precedence chain is \
             gone and a surface that still documents it directs a builder at \
             a key the door does not read"
        );
    }

    // ── the mechanism: the door promotes exactly one spelling
    let cfg = read_json(&root.join("config.json"));
    let door = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive config carries its BOOT graph")
        .iter()
        .find(|e| e["from"] == "." && e["to"] == "./brief")
        .expect("the door edge `. -> ./brief` is where the read lane is promoted")
        .clone();
    let set_context = door["modifier"]["set_context"]
        .as_object()
        .expect("the door edge promotes the read lane's identity keys");
    assert!(
        set_context.contains_key("audience_set"),
        "the door stopped promoting `audience_set` -- {set_context:?}"
    );
    for (key, expr) in set_context {
        assert!(
            !key.contains("participants")
                && !expr.as_str().unwrap_or_default().contains("participants"),
            "the door edge names `participants` again in `{key}` -- retired means \
             the door does not read it, not that it reads it last"
        );
    }
    assert_eq!(
        cfg["params"]["contract"]["accepts"][0]["context"],
        json!(["asker", "audience_set"]),
        "`contract.accepts[0].context` is the DOCUMENTED key set of the brief \
         ingress and the reason this release is Breaking -- it carries the two \
         surviving keys and nothing else"
    );
}
