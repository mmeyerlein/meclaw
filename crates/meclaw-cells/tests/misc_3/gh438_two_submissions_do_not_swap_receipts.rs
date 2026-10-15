//! GH #438 — two submissions in flight, two receipts, each with ITS own
//! `tool_call_id`.
//!
//! The correlation is not a hope, it is a row: a real tree, the shipped `submit`
//! hive with its own store, the one privileged edge onto the mutation door in
//! the birth topology, and a `CaptureCell` that takes the receipts. Two rounds
//! run back to back, and the second one is the measurement: it can only carry
//! `c-second` if the first round actually FORGOT its row — a delete that never
//! ran would hand round two the same id again.
//!
//! The digest assertion is the load-bearing half: a swap between two receipts
//! would be invisible if both manifests had the same digest, so the two
//! manifests declare DIFFERENT edges and each receipt is checked against the
//! digest of the manifest that asked for it.
//!
//! WHAT THIS FILE DOES NOT CLAIM, because it is not true
//! ====================================================
//! Two submissions **in flight at the same time** are NOT correlated correctly,
//! and this was measured rather than reasoned about: with both submitted before
//! either answer comes back, both receipts came out carrying the FIRST round's
//! id and digest. The reason is a window the FIFO argument misses. The door is
//! serial and the gate is serial, so the inserts and the answers keep their
//! order — but the gate emits round two's `select` as soon as round two's
//! answer arrives, which can be BEFORE the store has run round one's `delete`.
//! Both selects then read the same oldest row.
//!
//! Closing that window needs a claim that happens inside the read itself, and
//! the `store` has no operation that removes what it returns (`delete` takes no
//! `order_by`/`limit`, and a bundle's arguments are all fixed before the first
//! operation runs — `docs/cell-types.md` § store). So the correlation holds for
//! one submission in flight at a time, which is the shape the apply lane has,
//! and the limit is written down in `templates/submit/README.md` rather than
//! papered over here.
//!
//! Why this file lives in `meclaw-cells` and not in `meclaw-colony`: it needs
//! the real `code` and `store` cells, and `meclaw-colony` has no dependency on
//! the crate that holds them.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::code_wire::run_shipped_script;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Generous: the colony boots a tree, runs two round trips through the mutation
/// door and two store round trips (30 s convention).
const WAIT: Duration = Duration::from_secs(30);

/// The requester the shipped policy below permits. It is the path of the cell
/// that emits, because the substrate stamps `reply_to` — never a body field.
const REQUESTER: &str = "/request";

fn submit_template() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/submit")
}

// ─────────────────────────────────────────────────────────────── the test tree

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json" {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The asking side. The colony's ingress puts a caller's headers into the
/// CONTEXT compartment, so this cell re-mints them as `hop` one hop later and
/// passes the manifest through untouched — the digest is over bytes, and a cell
/// that re-serialised them would break its own submission.
const REQUEST: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = ((doc["envelope"].get("header") or {}).get("context") or {})
hop = {}
for k, v in ctx.items():
    if k.startswith("hop_"):
        hop[k[4:]] = v
body = doc["body"]
out = {"header": hop, "messages": body.get("messages") or []}
if body.get("manifest") is not None:
    out["manifest"] = body["manifest"]
sys.stdout.write(json.dumps(out))
"#;

/// The far end of the edges the two manifests draw. It exists so the mutations
/// are REAL ones that commit, rather than refusals that would also produce a
/// receipt and prove nothing about the lane.
const PARK: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps({"messages": []}))
"#;

/// A stand-in for the capability broker. Since GH #435 the submitter does not
/// decide who may submit — it asks, and the answer comes back on `in_verdict`.
/// The shell wires that pair; here it is wired by hand, so this file keeps
/// measuring the CORRELATION and not the broker.
///
/// It says yes to everything on purpose: what a real `access` would add is a
/// comparison, and the comparison has its own tests. What it must reproduce is
/// the SHAPE — a `tool_result` whose payload carries a `status`, on route
/// `grant`, which is exactly what `access/policy`'s `answer()` emits.
const BROKER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
msgs = doc["body"].get("messages") or []
call_id = ""
for m in msgs:
    if m.get("type") == "tool_call":
        call_id = str(m.get("id") or "")
sys.stdout.write(json.dumps({
    "header": {"route": "grant", "verdict": "allowed", "grant_id": ""},
    "messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
                  "text": json.dumps({"status": "allowed",
                                      "capability": "colony.mutate",
                                      "reason_code": ""})}]}))
"#;

fn code_cell(script: &str, hop: Value) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "manifest": {"type": "array", "required": false}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped submit template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // The ONE edge a mutation can never draw: `/colony/mutations` is not
        // among the endpoints a mutation may address on any scope, so the lane
        // onto the door lives in the birth topology and nowhere else.
        {"from": "./submit", "to": "/colony/mutations",
         "condition": "has(hop.route) && hop.route == 'mutate'"},
        {"from": "./request", "to": "./submit",
         "condition": "has(hop.route) && hop.route == 'in_apply'"},
        // The required drain: a caller that hands this hive a manifest and does
        // not take the receipt back has arranged to never learn what happened.
        {"from": "./submit", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'receipt'"},
        // GH #435: the question and its answer. The same pair `meclaw-os` draws
        // between its two occupants, including the two things that carry it —
        // `context.sub_sha`, because `hop.*` lives for exactly one hop and the
        // verdict has to be matched back against the manifest it was asked
        // about, and `context.sub_ask`, in the submitter's OWN key space.
        {"from": "./submit", "to": "./broker",
         "condition": "has(hop.route) && hop.route == 'ask'",
         "modifier": {"set_hop": {"route": "'in_request'"},
                      "set_context": {"requester": "'/submit'", "sub_ask": "'1'",
                                      "sub_sha": "hop.manifest_sha256"}}},
        {"from": "./broker", "to": "./submit",
         "condition": "context.sub_ask == '1' && has(hop.route) && hop.route == 'grant'",
         "modifier": {"set_hop": {"route": "'in_verdict'"}}}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/request/config.json",
        &code_cell(
            REQUEST,
            json!({"route": {"type": "string", "values": ["in_apply"], "required": false},
                   "manifest_sha256": {"type": "string", "required": false},
                   "tool_call_id": {"type": "string", "required": false}}),
        ),
    );
    write(root, "main/park/config.json", &code_cell(PARK, json!({})));
    write(
        root,
        "main/broker/config.json",
        &code_cell(
            BROKER,
            json!({"route": {"type": "string", "values": ["grant"], "required": false},
                   "verdict": {"type": "string", "required": false},
                   "grant_id": {"type": "string", "required": false}}),
        ),
    );
    // The shipped tree, verbatim. Nothing is injected into it any more: since
    // `submit@2.0.0` the cell carries no policy of its own, and what decides is
    // wired beside it.
    copy_cells(&submit_template(), &root.join("main/submit"));
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
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

// ────────────────────────────────────────────────────────── the two submissions

/// One declaration drawing a distinct edge, so the two manifests differ in
/// their BYTES and therefore in their digests.
fn manifest(route: &str) -> Value {
    json!([{
        "scope": "/",
        "ctx": {},
        "diff": {"add_edges": [{
            "from": "./request", "to": "./park",
            "condition": format!("has(hop.route) && hop.route == '{route}'")
        }]}
    }])
}

/// The canonical digest, drawn the way the shipped helper draws it.
fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

fn submission(call_id: &str, decls: &Value, sha: &str) -> Message {
    let mut context = Map::new();
    context.insert("hop_route".into(), json!("in_apply"));
    context.insert("hop_manifest_sha256".into(), json!(sha));
    context.insert("hop_tool_call_id".into(), json!(call_id));
    MessageBuilder::new(Path::new(REQUESTER))
        .body(Body::Inline(json!({
            "manifest": decls,
            "messages": [{"origin": "assistant", "type": "tool_call",
                          "id": call_id, "text": "{}"}]
        })))
        .context(context)
        .ttl(64)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(id) => panic!("the receipt arrived as a blob body ({id})"),
    }
}

/// The next `receipt` to reach the capture, or a panic naming what did arrive.
async fn next_receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    let mut seen: Vec<String> = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, rx.recv()).await {
            Ok(Some(m)) => {
                if m.headers.hop.get("route").and_then(Value::as_str) == Some("receipt") {
                    return m;
                }
                seen.push(format!("{:?}", m.headers.hop));
            }
            Ok(None) => {
                panic!("the capture channel closed before a receipt arrived; seen: {seen:?}")
            }
            Err(_) => panic!("no receipt within {WAIT:?}; seen: {seen:?}"),
        }
    }
}

/// Two submissions, back to back, each answered with ITS own correlation.
///
/// The second is sent once the first receipt is in hand — one in flight at a
/// time, which is what the pop is sound for (see the file header). What that
/// still measures is everything #438 is about: the id and the digest come off
/// the remembered row rather than off the answer, and round two proves the row
/// of round one was deleted rather than read a second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_submissions_do_not_swap_receipts() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx) = boot(&td).await;

    let first = manifest("probe_a");
    let second = manifest("probe_b");
    let sha_first = digest_of(&first);
    let sha_second = digest_of(&second);
    assert_ne!(
        sha_first, sha_second,
        "two manifests with the same digest would make a swap invisible"
    );

    h.send(submission("c-first", &first, &sha_first)).await;
    let r1 = next_receipt(&mut sink_rx).await;
    h.send(submission("c-second", &second, &sha_second)).await;
    let r2 = next_receipt(&mut sink_rx).await;

    for (r, call_id, sha) in [(&r1, "c-first", &sha_first), (&r2, "c-second", &sha_second)] {
        let body = body_of(r);
        assert_eq!(
            body["messages"][0]["id"],
            json!(call_id),
            "the receipt closes the round that asked for it: {body}"
        );
        assert_eq!(
            r.headers.hop.get("tool_call_id").and_then(Value::as_str),
            Some(call_id),
            "and says so on the hop too: {:?}",
            r.headers.hop
        );
        assert_eq!(
            r.headers.hop.get("manifest_sha256").and_then(Value::as_str),
            Some(sha.as_str()),
            "each receipt carries the digest of the manifest that asked for it — \
             the assertion a swap could not survive: {:?}",
            r.headers.hop
        );
        assert!(
            body["messages"][0]["text"]
                .as_str()
                .expect("a turn")
                .contains("manifest applied"),
            "both manifests committed: {body}"
        );
    }

    h.shutdown().await;
}
