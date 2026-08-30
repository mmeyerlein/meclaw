//! GH #456 — `seed_rows`: rows that are permissions and keys go through the
//! door topology goes through.
//!
//! Seven diff operations changed topology and none of them wrote a row. A
//! policy row, a grant, a firewall rule, an affinity subscriber — the class
//! that most needs the door — reached a RUNNING colony as a bare store message,
//! and that path has three holes: no digest, no access verdict before the
//! write, and no `mutation_log` entry. `seed_rows` is the eighth operation, and
//! this file proves the four things the issue asks for:
//!
//! 1. **Positive.** A declaration writes into a LIVE store and the store's very
//!    next `select` answers with the row; the mutation appears in `mutation_log`
//!    as `committed`.
//! 2. **Refusals.** A target that is not a `store` and a table the store does
//!    not declare are refused by name, and nothing is written.
//! 3. **The gate.** A `seed_rows` manifest passes the submitter exactly like a
//!    topology manifest — one `colony.mutate` question, no capability of its
//!    own — and a denied verdict stops it before the door.
//! 4. **Idempotence.** The same declaration applied twice leaves one row, and a
//!    changed row changes the digest.
//!
//! Why the assertions read the DATABASE and the store's own answer rather than
//! the receipt: an operation that stopped being rejected but wrote nothing would
//! pass an outcome-only assertion (the gh169 lesson).

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30 s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The node name of the store this file grows, and therefore its directory.
const NODE: &str = "keeper";

const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);
const REQUESTER: &str = "/os/operator/submit";

// ── the colony half ─────────────────────────────────────────────────────────

/// A store that declares one table, in the shape a permissions row has: an id,
/// a subject and a verdict.
fn store_config() -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {"schema": {"policy": {
            "rule_id": "text", "subject": "text", "verdict": "text", "priority": "int"
        }}},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

fn policy_row(rule_id: &str) -> Value {
    json!({"rule_id": rule_id, "subject": "*", "verdict": "allow", "priority": 100})
}

/// One `tool_call` turn: the args go on the wire as the turn's `text`, exactly
/// as an `llm` cell writes them.
fn call(id: &str, args: Value) -> Value {
    json!({"origin":"assistant","type":"tool_call","text": args.to_string(), "id": id})
}

/// A booted colony with a `store` factory, a capture sink and the `keeper`
/// template registered — everything up to (but not including) `seed_rows`.
struct Rig {
    td: tempfile::TempDir,
}

async fn boot(rig: &Rig) -> (ColonyHandle, mpsc::Receiver<Message>) {
    // The production filesystem layout: ONE top-level cell directory under
    // `{root}`, whose name is stripped from every logical path. Without it the
    // logical→fs resolver has no anchor to strip, and `/keeper` would mean two
    // different directories before and after the node exists.
    std::fs::create_dir_all(rig.td.path().join("main")).expect("root cell dir");
    std::fs::write(
        rig.td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("root cell config");
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(&rig.td, vec![("store".to_string(), factory)]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let templates_root = rig.td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(&tpl).expect("template dir");
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).expect("write");
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&store_config()).expect("config json"),
    )
    .expect("write");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("the rescan must not have aborted");

    let out = mutate(
        &h,
        json!({"scope": "/", "diff": {"add_nodes": [{"name": NODE, "template": NODE}]}}),
    )
    .await;
    assert!(
        matches!(out, MutationOutcome::Committed { .. }),
        "growing the store must commit: {out:?}"
    );
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("/{NODE}")),
        Path::new("/sink"),
    )
    .await;
    (h, sink_rx)
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    ack_rx.await.expect("mutation ack")
}

/// A `seed_rows` declaration against the grown store.
fn seed_rows(table: &str, rows: Value) -> Value {
    json!({"scope": "/", "diff": {"seed_rows": [
        {"target": NODE, "table": table, "rows": rows}
    ]}})
}

/// Ask the LIVE store what it holds, through its own port. The answer comes
/// from the running cell's own connection, which is what makes it evidence
/// that the row reached the database the cell reads.
async fn rule_ids(h: &ColonyHandle, sink_rx: &mut mpsc::Receiver<Message>) -> Vec<String> {
    let msg = MessageBuilder::new(Path::new(&format!("/{NODE}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [call(
            "c-sel",
            json!({"operation": "select", "table": "policy",
                   "columns": ["rule_id", "subject", "verdict", "priority"]})
        )]})))
        .build();
    h.send(msg).await;
    let reply = tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .expect("no store reply within 30s")
        .expect("sink channel closed");
    let body = match &reply.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("inline expected"),
    };
    let turn = &body["messages"][0];
    let rows: Value = meclaw_core::serde_json::from_str(turn["text"].as_str().unwrap_or("[]"))
        .unwrap_or_else(|e| panic!("select turn carries no row array: {body} ({e})"));
    rows.as_array()
        .unwrap_or_else(|| panic!("select turn is not an array: {body}"))
        .iter()
        .map(|r| r["rule_id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The `mutation_log` row of one mutation id, read the way an operator reads it.
fn mutation_log_row(colony_db: &std::path::Path, id: &str) -> (String, String) {
    let conn = rusqlite::Connection::open_with_flags(
        colony_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT status, payload_json FROM mutation_log WHERE id=?",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or_else(|e| panic!("no mutation_log row for {id}: {e}"))
}

fn committed_id(out: &MutationOutcome) -> String {
    match out {
        MutationOutcome::Committed { id } => id.clone(),
        other => panic!("expected a committed mutation, got {other:?}"),
    }
}

// ── 1. positive ─────────────────────────────────────────────────────────────

/// A row enters a store that is already AWAKE, and the store's very next
/// `select` answers with it. The mutation is in `mutation_log` as `committed`,
/// carrying the declaration — which is the whole point: the change is visible
/// to the steward and to the operator, which a bare store message never was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_enters_a_running_store_and_the_ledger_says_so() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    // Wake the store first, so the write below meets a cell that owns an open
    // `cell.db` — the running-colony case, not the fresh-directory one.
    assert!(
        rule_ids(&h, &mut sink_rx).await.is_empty(),
        "the store starts empty"
    );

    let out = mutate(&h, seed_rows("policy", json!([policy_row("r1")]))).await;
    let id = committed_id(&out);

    assert_eq!(
        rule_ids(&h, &mut sink_rx).await,
        vec!["r1".to_string()],
        "the live store must answer with the row that came through the door"
    );

    let (status, payload) = mutation_log_row(&rig.td.path().join("colony.db"), &id);
    assert_eq!(status, "committed", "the door recorded the write");
    assert!(
        payload.contains("seed_rows") && payload.contains("r1"),
        "the ledger row carries the declaration that was applied: {payload}"
    );
    h.shutdown().await;
}

/// The other half of the running-colony case: a store that has never been awake
/// takes rows too, and wakes up holding them. The table it stands up is the one
/// its `params.schema` declares, so the store's own DDL finds it and leaves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_that_never_woke_wakes_up_holding_its_rows() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    let out = mutate(&h, seed_rows("policy", json!([policy_row("r1")]))).await;
    committed_id(&out);

    assert_eq!(
        rule_ids(&h, &mut sink_rx).await,
        vec!["r1".to_string()],
        "the first wake must find the rows already there"
    );
    h.shutdown().await;
}

// ── 2. refusals ─────────────────────────────────────────────────────────────

/// A target that is not a `store` is refused by name, and the mutation carries
/// no other effect: only a store owns declared tables.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_that_is_not_a_store_is_refused_by_name() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    let out = mutate(
        &h,
        json!({"scope": "/", "diff": {"seed_rows": [
            {"target": "sink", "table": "policy", "rows": [policy_row("r1")]}
        ]}}),
    )
    .await;
    match out {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "seed_target_not_a_store")
        }
        other => panic!("a non-store target must be refused, got {other:?}"),
    }
    assert!(
        rule_ids(&h, &mut sink_rx).await.is_empty(),
        "a refused declaration writes nothing"
    );
    h.shutdown().await;
}

/// A table the store does not declare is refused by name. Inventing the table
/// would create a shape the store never agreed to — and the store's own DDL
/// would then be the second author of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_table_the_store_does_not_declare_is_refused_by_name() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    let out = mutate(&h, seed_rows("grants", json!([{"rule_id": "r1"}]))).await;
    match out {
        MutationOutcome::Rejected {
            error_code,
            ref details,
            ..
        } => {
            assert_eq!(error_code, "seed_table_undeclared");
            assert!(
                details.contains("policy"),
                "the refusal names the tables that DO exist: {details}"
            );
        }
        other => panic!("an undeclared table must be refused, got {other:?}"),
    }
    assert!(
        rule_ids(&h, &mut sink_rx).await.is_empty(),
        "a refused declaration writes nothing"
    );
    h.shutdown().await;
}

/// A column the table does not declare is a `schema` refusal — the token the
/// door has always used for "the diff says something the grammar does not know".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_column_the_table_does_not_declare_is_refused() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    let out = mutate(
        &h,
        seed_rows("policy", json!([{"rule_id": "r1", "secret": "x"}])),
    )
    .await;
    match out {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(error_code, "schema"),
        other => panic!("an undeclared column must be refused, got {other:?}"),
    }
    assert!(rule_ids(&h, &mut sink_rx).await.is_empty());
    h.shutdown().await;
}

// ── 4. idempotence ──────────────────────────────────────────────────────────

/// The same declaration applied twice leaves ONE row. `seed_rows` says *these
/// rows are present*, not *append these rows*: a store's declared tables carry
/// no primary key, so nothing else would make a re-applied manifest a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_declaration_applied_twice_leaves_one_row() {
    let rig = Rig {
        td: tempfile::TempDir::new().expect("tempdir"),
    };
    let (h, mut sink_rx) = boot(&rig).await;

    for _ in 0..2 {
        let out = mutate(&h, seed_rows("policy", json!([policy_row("r1")]))).await;
        committed_id(&out);
    }
    assert_eq!(
        rule_ids(&h, &mut sink_rx).await,
        vec!["r1".to_string()],
        "re-applying the same manifest must be a no-op, not a duplicate"
    );

    // A DIFFERENT row is a different declaration and lands beside the first.
    let out = mutate(&h, seed_rows("policy", json!([policy_row("r2")]))).await;
    committed_id(&out);
    let mut ids = rule_ids(&h, &mut sink_rx).await;
    ids.sort();
    assert_eq!(ids, vec!["r1".to_string(), "r2".to_string()]);
    h.shutdown().await;
}

// ── 3. the gate, the digest, and the drafting vocabulary ────────────────────

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

/// A manifest whose only operation is `seed_rows`.
fn rows_manifest(rule_id: &str) -> Value {
    json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": {"seed_rows": [{
            "target": "./access/store", "table": "policy",
            "rows": [{"rule_id": rule_id, "subject": "*", "verdict": "allow"}]
        }]}
    }])
}

/// Phase A of the submitter, mirroring `gh446`'s rig.
fn submit(decls: &Value) -> Vec<Value> {
    let sha = digest_of(decls);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": REQUESTER,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": sha,
                                 "tool_call_id": "op:c1" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [],
            "params": {}
        }),
    )
}

fn unpark(phase: &str, decls: &Value) -> Vec<Value> {
    let sha = digest_of(decls);
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": REQUESTER,
                        "tool_call_id": "op:c1", "manifest_sha256": sha }]);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": phase,
                             "sub_carry": "{\"status\":\"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

fn verdict(capability: &str, status: &str, sha: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": { "route": "in_verdict" },
                        "context": { "sub_ask": "1", "sub_sha": sha } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "q1",
                "text": json!({ "status": status, "capability": capability,
                                "reason_code": "" }).to_string() }],
            "params": {}
        }),
    )
}

/// `seed_rows` asks the question topology asks, and no other. The rows a
/// manifest carries are not executable behaviour, so `code.author` does not
/// fire; and the reach of the write is already bounded by the scope the
/// `colony.mutate` question is asked over — the same bound that lets the same
/// requester `swap_nodes` the store outright.
///
/// A drift lock (development-rules § 2d): the sentence on the submitter's public
/// README that makes this promise is grepped here, AND the mechanism it describes
/// is asserted below. Either half alone would let the prose outlive it.
#[test]
fn a_rows_manifest_asks_the_one_question_topology_asks() {
    let readme = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/submit/README.md"
    ))
    .expect("the submitter's README ships");
    assert!(
        readme.contains("A `seed_rows` manifest asks no question of its own"),
        "the README must carry the promise this test pins"
    );

    let decls = rows_manifest("r1");
    let a = submit(&decls);
    assert_eq!(a.len(), 2, "park, then ONE question: {a:?}");
    assert_eq!(op_of(&a[1])["capability"], "colony.mutate");
    assert_eq!(op_of(&a[1])["check_only"], true);
    assert_eq!(op_of(&a[1])["resource"]["scope"], "/os/orgs/acme");

    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 3, "forget the park, remember the flight, submit");
    assert_eq!(out[2]["header"]["route"], "mutate");
}

/// The access verdict is what stands between the rows and the door. Denied, the
/// manifest is forgotten and refused — and nothing goes to `mutate`.
#[test]
fn a_denied_verdict_stops_the_rows_before_the_door() {
    let sha = digest_of(&rows_manifest("r1"));
    let out = verdict("colony.mutate", "denied", &sha);
    assert_eq!(out.len(), 2, "forget the park, then refuse");
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("mutate")),
        "a denied manifest never reaches the door: {out:?}"
    );
    assert_eq!(out[1]["header"]["route"], "receipt");
    assert_eq!(out[1]["header"]["error_code"], "requester_not_permitted");
}

/// The manifest digest is over the declarations, so a changed ROW changes it.
/// That is the acceptance criterion the issue names, and it holds without the
/// digest helper knowing the key exists.
#[test]
fn a_changed_row_changes_the_digest() {
    assert_ne!(
        digest_of(&rows_manifest("r1")),
        digest_of(&rows_manifest("r2")),
        "a manifest whose row changed must not carry the digest of the old one"
    );
}

/// The normaliser knows the key. Without this the drafting lane would let
/// `seed_rows` through as an invented operation only if it happened to be
/// spelled like one of the seven — and refuse it otherwise, at position k.
#[test]
fn the_normaliser_admits_the_eighth_operation() {
    let out = emit_all(
        &shipped_script(NORMALISE),
        &json!({
            "target": "/os/builder",
            "header": { "hop": {}, "context": {} },
            "ttl": 64,
            "messages": [{ "origin": "assistant", "type": "message",
                           "text": json!({"declarations": rows_manifest("r1")}).to_string() }],
            "params": {}
        }),
    );
    assert!(
        out.iter()
            .all(|m| m["header"]["error_code"] != json!("declaration_malformed")),
        "the normaliser must not call the eighth operation unknown: {out:?}"
    );
}
