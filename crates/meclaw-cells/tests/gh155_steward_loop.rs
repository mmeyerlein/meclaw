//! The steward's loop, run against its real scripts (GitHub #155).
//!
//! A colony that can be mutated by an agent is not yet a colony that improves
//! itself. What is missing is the loop: measure, decide, act through the same
//! gates everybody else uses, verify, measure the effect, and then keep the
//! change or take it back — with a record of each step.
//!
//! The tests below hold up the parts of that claim that can be wrong quietly:
//!
//! - the mutation **radius** (model choice and numeric params, nothing else),
//! - the **charter rule** that a cycle without a pre-authored revert plan is
//!   invalid, and that the plan must actually restore the original,
//! - the **significance floor**, so noise never triggers action,
//! - the **probe**, which fails closed when it cannot look,
//! - and that every one of those outcomes is a receipt rather than a silence.
//!
//! # GH #304 — the accepted change is measured at the CELL, not at the script
//!
//! For the whole life of `steward@2.0.x` the decide path emitted a diff the
//! validator has always refused (`{"swap_nodes": [{"name": …, "params": …}]}`
//! against a validator that requires `match.name` + `with`), and every test in
//! this file agreed with it, because the assertion was a literal copy of the
//! script's own output. A self-assertion cannot fail; that is what let a loop
//! which has never committed once read as covered.
//!
//! So the accepted change is now checked the only way that can be wrong: the
//! emitted body is handed to a REAL `llm` cell in front of a counting mock
//! provider, and the claim is that the cell's live param moved — persisted in
//! its own `cell.db`, and visible on the wire of the next inference. Nothing in
//! this file re-derives the params-update path; the cell is the witness.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

#[path = "mock_openai.rs"]
mod mock_openai;

const MUTATOR: &str = "../../templates/steward/mutator/config.json";
const METER: &str = "../../templates/steward/meter/config.json";
const PROBE: &str = "../../templates/steward/probe/config.json";
const CHARTER: &str = "../../templates/steward/charter/config.json";
const HIVE: &str = "../../templates/steward/config.json";

fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn config(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("template config");
    serde_json::from_str(&raw).expect("config json")
}

fn script(path: &str) -> String {
    resolve_vars(
        config(path)["params"]["script_inline"]
            .as_str()
            .expect("script"),
    )
}

fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_in(std::path::Path::new("."), script, doc)
}

/// `emit`, but from a chosen working directory.
///
/// The probe resolves `${STEWARD_COLONY_DB:-colony.db}` to a **relative** name,
/// so a test that wants it to find a ledger has to run the script where the
/// fixture is. Without this the only reachable probe verdict is
/// `probe_unavailable`, which is why the mechanism below went unpinned.
fn emit_in(dir: &std::path::Path, script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

/// A `colony.db` carrying the three tables the probe reads.
///
/// A row is `(table, key, headers)`: for `mutation_log` the key is the row's
/// `status`, for `message_log` it is `to_path` and `headers` is the header the
/// probe parses, for `dead_letters` both are ignored. Every row is stamped
/// `created_at` = now, so all of them fall inside the probe's window.
fn colony_db_with(dir: &std::path::Path, rows: &[(&str, &str, serde_json::Value)]) {
    let conn = rusqlite::Connection::open(dir.join("colony.db")).expect("colony.db");
    conn.execute_batch(
        "CREATE TABLE mutation_log (status TEXT NOT NULL, created_at INTEGER NOT NULL);
         CREATE TABLE message_log (headers TEXT NOT NULL, to_path TEXT NOT NULL,
                                   from_path TEXT NOT NULL, created_at INTEGER NOT NULL);
         CREATE TABLE dead_letters (created_at INTEGER NOT NULL);",
    )
    .expect("the three tables the probe reads");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    for (table, key, headers) in rows {
        let done = match *table {
            "mutation_log" => conn.execute(
                "INSERT INTO mutation_log (status, created_at) VALUES (?1, ?2)",
                rusqlite::params![key, now],
            ),
            "message_log" => conn.execute(
                "INSERT INTO message_log (headers, to_path, from_path, created_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![headers.to_string(), key, "/main/steward/mutator", now],
            ),
            "dead_letters" => conn.execute(
                "INSERT INTO dead_letters (created_at) VALUES (?1)",
                rusqlite::params![now],
            ),
            other => panic!("the probe reads no table called {other}"),
        };
        done.expect("fixture row");
    }
}

/// A judge decision arriving at the mutator.
fn decision(args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string()
        }],
        "header": {"hop": {}, "context": {"st_cycle": "cycle:1", "st_goal": "goal:llm-cost"}}
    })
}

/// The row a `store` insert would write, if there is one.
fn inserted(out: &[serde_json::Value], table: &str) -> Option<serde_json::Value> {
    out.iter().find_map(|m| {
        let text = m["messages"][0]["text"].as_str()?;
        let args: serde_json::Value = serde_json::from_str(text).ok()?;
        (args["operation"] == "insert" && args["table"] == table).then(|| args["row"].clone())
    })
}

/// The change the cell decided to make, if it decided to make one — the one
/// message that leaves on the `mutate` lane.
fn mutation(out: &[serde_json::Value]) -> Option<&serde_json::Value> {
    out.iter().find(|m| m["header"]["route"] == "mutate")
}

/// The body of an emitted message: everything beside the `header`, which the
/// substrate splits off into hop keys (`code::wire::split_content_header`).
fn body_of(m: &serde_json::Value) -> serde_json::Value {
    let mut v = m.clone();
    v.as_object_mut()
        .expect("an emitted message is a JSON object")
        .remove("header");
    v
}

/// The cell the steward is allowed to change in these tests. A brain, because
/// the radius is model choice first — and because an `llm` cell is the one that
/// can be asked afterwards what it actually runs on.
const BRAIN: &str = "/main/talky/brain";

/// A real `llm` cell on `model`, talking to `base_url`, with its own `cell.db`.
fn brain(td: &TempDir, base_url: &str, model: &str) -> (LlmCell, DbConn) {
    let params = LlmParams::parse(&serde_json::json!({
        "provider": "openai",
        "model": model,
        "api_key": "sk-test-gh304",
        "base_url": format!("{base_url}/v1"),
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db opens");
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// Deliver one body into the cell exactly as the colony would.
async fn deliver(cell: &mut LlmCell, db: &mut DbConn, body: serde_json::Value) {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(BRAIN),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(BRAIN))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let _ = rx.recv().await;
}

/// The model overlay the cell persisted in its OWN db, JSON-encoded as stored.
async fn stored_model(db: &mut DbConn) -> Option<String> {
    db.call(|conn| {
        conn.query_row("SELECT value FROM params WHERE key='model'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
    })
    .await
}

/// One inference turn, so the wire can be asked which model actually served it.
fn an_inference() -> serde_json::Value {
    serde_json::json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]})
}

/// Every message of a run, judged by the cell's OWN declared `emits` contract —
/// compiled and applied by the substrate, never re-derived here.
///
/// The `code` cell validates its emissions in-cell, always on, so a body the
/// declaration does not admit never leaves the cell at all. That makes the
/// contract part of the change rather than documentation beside it: a `params`
/// slot nobody declared, or a `messages[]` still marked `required`, would turn
/// the fix into a different silent failure.
fn assert_the_declaration_admits(path: &str, out: &[serde_json::Value]) {
    let cfg = config(path);
    let block: meclaw_colony::config::ContractBlock =
        serde_json::from_value(cfg["contract"].clone()).expect("the contract block parses");
    let compiled =
        meclaw_core::CompiledEmits::compile(&block.emits).expect("the emits schemas compile");
    assert!(!out.is_empty(), "nothing was emitted at all");
    for m in out {
        meclaw_core::validate_emits(m, &compiled)
            .unwrap_or_else(|e| panic!("the cell's own contract refuses what it emits: {e} — {m}"));
    }
}

fn a_valid_change() -> serde_json::Value {
    serde_json::json!({
        "cycle_id": "cycle:1",
        "action": "change",
        "reasoning": "opus served 40 calls at 2.1M prompt tokens; sonnet at the same counts costs 0.41 EUR against 1.90 EUR, and the quality gate held last window.",
        "simulated": {"counterfactual_eur": 0.41, "actual_eur": 1.90},
        "change": {"target": "/main/talky/brain", "kind": "model",
                   "from": "anthropic/claude-opus-4", "to": "anthropic/claude-sonnet-4"},
        "revert_plan": {"target": "/main/talky/brain", "kind": "model",
                        "to": "anthropic/claude-opus-4"}
    })
}

// ---------------------------------------------------------------------------
// The charter rule
// ---------------------------------------------------------------------------

#[test]
fn a_cycle_without_a_revert_plan_is_invalid() {
    let mut args = a_valid_change();
    args["revert_plan"] = serde_json::json!({});
    let out = emit(&script(MUTATOR), decision(args));
    assert!(
        mutation(&out).is_none(),
        "nothing may be changed without a way back: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(row["outcome"], "refused");
    assert_eq!(row["reason_code"], "no_revert_plan");
}

#[test]
fn a_revert_plan_that_does_not_lead_back_is_refused() {
    // Structurally perfect, semantically useless: it "reverts" to the value we
    // are moving TO. Every field check would pass.
    let mut args = a_valid_change();
    args["revert_plan"]["to"] = serde_json::json!("anthropic/claude-sonnet-4");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(row["reason_code"], "revert_plan_is_not_inverse");
}

#[test]
fn a_revert_plan_pointing_at_another_cell_is_refused() {
    let mut args = a_valid_change();
    args["revert_plan"]["target"] = serde_json::json!("/main/cogny/brain");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "revert_plan_wrong_target"
    );
}

#[test]
fn a_revert_plan_that_restores_something_else_is_refused() {
    let mut args = a_valid_change();
    args["revert_plan"]["to"] = serde_json::json!("anthropic/claude-haiku-3");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "revert_plan_does_not_restore_the_original"
    );
}

// ---------------------------------------------------------------------------
// The radius
// ---------------------------------------------------------------------------

#[test]
fn a_topology_change_is_refused_however_well_argued() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/brain", "kind": "topology",
        "from": "", "to": "rewire the collector"
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/brain", "kind": "topology", "to": "rewire it back"
    });
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none(), "radius v1 executes no topology");
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "outside_radius"
    );
}

#[test]
fn a_proposal_is_recorded_rather_than_executed() {
    // The judge's legitimate way to raise a topology idea. It has to be
    // findable later — a human who cannot find the proposal has not been sent
    // one.
    let out = emit(
        &script(MUTATOR),
        decision(serde_json::json!({
            "cycle_id": "cycle:1",
            "action": "propose",
            "reasoning": "the real win is moving the summariser off the hot path",
            "simulated": {},
            "change": {},
            "revert_plan": {}
        })),
    );
    assert!(mutation(&out).is_none());
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(row["outcome"], "proposed");
    assert!(
        row["judged"]["reasoning"]
            .as_str()
            .unwrap()
            .contains("summariser"),
        "the proposal's content survives into the record: {row}"
    );
}

#[test]
fn a_numeric_step_beyond_the_limit_is_refused() {
    // An in-radius key on a cell that can receive it, so the step limit is what
    // refuses this and not the key set (the radius is checked first, on
    // purpose).
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": BRAIN, "kind": "numeric_param",
        "key": "max_tokens", "from": 4096, "to": 40960
    });
    args["revert_plan"] = serde_json::json!({
        "target": BRAIN, "kind": "numeric_param",
        "key": "max_tokens", "to": 4096
    });
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert!(
        inserted(&out, "cycles").unwrap()["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("step_too_large"),
        "one cycle's mistake has to stay small enough to measure out of"
    );
}

// ---------------------------------------------------------------------------
// What a good cycle does
// ---------------------------------------------------------------------------

#[test]
fn an_accepted_change_names_the_cell_it_changes() {
    let out = emit(&script(MUTATOR), decision(a_valid_change()));
    let m = mutation(&out).expect("a change leaves the cell");
    // The address is a hop key, the way `llm-registry/hand` names a subscriber:
    // the parent draws one edge per cell the loop may reach, so which cells are
    // in range is a fact about the seed rather than about the judge's output.
    assert_eq!(m["header"]["target"], "/main/talky/brain");
    // The ordinary shape, with no operator flag anywhere near it.
    let dump = serde_json::to_string(m).unwrap();
    assert!(!dump.contains("operator"), "no operator lane: {dump}");
    assert!(!dump.contains("force"), "no override: {dump}");
    // GH #304: the shape the validator has always refused must not come back.
    assert!(
        !dump.contains("swap_nodes"),
        "the refused diff is gone: {dump}"
    );
    // And the whole run passes the cell's own declaration, both paths.
    assert_the_declaration_admits(MUTATOR, &out);
}

#[test]
fn the_revert_run_also_passes_the_cells_own_declaration() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({
                    "op": "revert",
                    "cycle_id": "cycle:1",
                    "plan": {"target": "/main/talky/brain", "kind": "model",
                             "to": "anthropic/claude-opus-4"}
                }).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    assert_the_declaration_admits(MUTATOR, &out);
}

/// GH #304 — the decide path, measured at the cell.
///
/// The mutator's own output is handed to a real `llm` cell. What is asserted is
/// not the shape of that output but its EFFECT: the provider is not called, the
/// overlay lands in the cell's own db, and the next inference goes out on the
/// new model. Any of the three can fail on a body that looks right.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decided_change_moves_the_brains_live_model() {
    let out = emit(&script(MUTATOR), decision(a_valid_change()));
    let m = mutation(&out).expect("a change leaves the cell").clone();

    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = brain(&td, &mock.base_url, "anthropic/claude-opus-4");

    deliver(&mut cell, &mut db, body_of(&m)).await;

    assert!(
        mock.recorded_requests().await.is_empty(),
        "a params update buys a write, not an inference: {m}"
    );
    assert_eq!(
        stored_model(&mut db).await.as_deref(),
        Some(r#""anthropic/claude-sonnet-4""#),
        "the decided model has to reach the cell's own params: {m}"
    );

    deliver(&mut cell, &mut db, an_inference()).await;
    let requests = mock.recorded_requests().await;
    assert_eq!(requests.len(), 1, "one call, and it is the next turn's");
    assert_eq!(
        requests[0].model(),
        Some("anthropic/claude-sonnet-4"),
        "the change is only real if the wire carries it"
    );
}

#[test]
fn an_applied_cycle_stays_open_and_fires_the_probe() {
    let out = emit(&script(MUTATOR), decision(a_valid_change()));
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(
        row["status"], "applied",
        "open until its effect is measured"
    );
    assert_eq!(row["outcome"], "");
    assert_eq!(row["revert_plan"]["to"], "anthropic/claude-opus-4");
    assert!(
        row["simulated"]["counterfactual_eur"].is_number(),
        "what it simulated is part of the record: {row}"
    );

    let probe = out
        .iter()
        .find(|m| m["header"]["route"] == "probe")
        .expect("the health check is fired at once, not on the next tick");
    let args: serde_json::Value =
        serde_json::from_str(probe["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["target"], "/main/talky/brain");
}

/// A numeric cap on a cell that can actually receive one, measured at the cell.
///
/// The shape half of this used to be the whole test, and a shape assertion on
/// the numeric half is worth exactly as little as it was on the model half: the
/// key it used (`max_iter`) lives on a `code` cell, which has no params lane at
/// all, so the body was well formed and nothing anywhere would ever have merged
/// it. Asked of a cell that HAS the lane, the claim can fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_numeric_param_within_the_limit_moves_the_cells_live_cap() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": BRAIN, "kind": "numeric_param",
        "key": "max_tokens", "from": 4096, "to": 3072
    });
    args["revert_plan"] = serde_json::json!({
        "target": BRAIN, "kind": "numeric_param",
        "key": "max_tokens", "to": 4096
    });
    let out = emit(&script(MUTATOR), decision(args));
    let m = mutation(&out).expect("a change").clone();
    assert_eq!(m["header"]["target"], BRAIN);
    assert_eq!(
        m["params"]["max_tokens"],
        serde_json::json!(3072),
        "an integer stays an integer on the wire"
    );
    // A body carries a `system`, a `messages[]` or an `attachments[]` slot or it
    // is not a UBF body at all (`crates/meclaw-core/schemas/ubf-body.json`), and
    // a params update has nothing to say in any of them. The empty `system` is
    // the ticket — the same one `llm-registry/hand` buys for its push — and the
    // ABSENT `messages[]` is what keeps the delivery free of an inference.
    assert_eq!(m["system"], serde_json::json!({}));
    assert!(
        m.get("messages").is_none(),
        "a params update carries no turn: {m}"
    );

    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = brain(&td, &mock.base_url, "anthropic/claude-opus-4");

    deliver(&mut cell, &mut db, body_of(&m)).await;
    assert!(mock.recorded_requests().await.is_empty());

    deliver(&mut cell, &mut db, an_inference()).await;
    let requests = mock.recorded_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["max_tokens"],
        serde_json::json!(3072),
        "the cap is only real if the wire carries it"
    );
}

/// GH #304, fix round 1 — the honest edge of the numeric radius.
///
/// `max_iter` is a real shipped cap (`templates/collector/assemble/config.json`)
/// and it sits on a `code` cell. There is no `OverlayParams` impl for the code
/// cell's params and no params lane in `code/cell.rs`: an overlay addressed
/// there is not merged, not persisted, and not refused either — the cell would
/// take the body as ordinary input. A cycle that sent one and receipted
/// `applied` would be the #304 defect a second time, on the half of the radius
/// nobody was looking at.
///
/// So the cell refuses it, and the refusal names the key. What is asserted is
/// the whole outcome: nothing leaves on the `mutate` lane, and the receipt says
/// `refused` rather than `applied`.
#[test]
fn a_numeric_cap_with_no_receiver_is_refused_rather_than_receipted_as_applied() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "from": 4, "to": 5
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "to": 4
    });
    let out = emit(&script(MUTATOR), decision(args));
    assert!(
        mutation(&out).is_none(),
        "nothing may go out to a cell that cannot merge it: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        row["outcome"], "refused",
        "a receipt saying `applied` over a colony where nothing moved is the \
         defect, not the fix: {row}"
    );
    assert_eq!(row["reason_code"], "key_outside_radius_max_iter");
    assert_eq!(row["status"], "closed");
}

/// The set is a declaration, not a constant: an operator who has wired a target
/// that CAN receive another key widens it, and the same decision then travels.
#[test]
fn the_numeric_key_set_is_an_operator_declaration() {
    let widened = script(MUTATOR).replace(
        "temperature,max_tokens,external_timeout_ms,attachment_timeout_ms",
        "max_iter",
    );
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "from": 4, "to": 5
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "to": 4
    });
    let out = emit(&widened, decision(args));
    let m = mutation(&out).expect("the widened set lets it through");
    assert_eq!(m["params"]["max_iter"], serde_json::json!(5));
}

// ---------------------------------------------------------------------------
// The revert
// ---------------------------------------------------------------------------

#[test]
fn a_revert_uses_the_plan_that_was_authored_beforehand() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({
                    "op": "revert",
                    "cycle_id": "cycle:1",
                    "plan": {"target": "/main/talky/brain", "kind": "model",
                             "to": "anthropic/claude-opus-4"}
                }).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    let m = mutation(&out).expect("the inverse change");
    assert_eq!(m["header"]["target"], "/main/talky/brain");
    assert_eq!(m["params"]["model"], "anthropic/claude-opus-4");
    assert_eq!(m["header"]["outcome"], "reverted");
}

/// GH #304 — the revert path, measured at the cell.
///
/// The half that matters most: a way back that does not actually arrive is
/// worse than no way back, because the receipt says `reverted` either way. So
/// the same witness is asked in the other direction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revert_moves_the_brains_live_model_back() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({
                    "op": "revert",
                    "cycle_id": "cycle:1",
                    "plan": {"target": "/main/talky/brain", "kind": "model",
                             "to": "anthropic/claude-opus-4"}
                }).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    let m = mutation(&out).expect("the inverse change").clone();

    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    // The colony as the failed cycle left it: on the model that did not prove
    // itself.
    let (mut cell, mut db) = brain(&td, &mock.base_url, "anthropic/claude-sonnet-4");

    deliver(&mut cell, &mut db, body_of(&m)).await;

    assert!(
        mock.recorded_requests().await.is_empty(),
        "taking a change back must not cost a call either: {m}"
    );
    assert_eq!(
        stored_model(&mut db).await.as_deref(),
        Some(r#""anthropic/claude-opus-4""#),
        "the pre-authored plan has to land, not merely be well formed: {m}"
    );

    deliver(&mut cell, &mut db, an_inference()).await;
    let requests = mock.recorded_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model(), Some("anthropic/claude-opus-4"));
}

#[test]
fn a_revert_without_a_plan_is_recorded_rather_than_improvised() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({"op": "revert", "cycle_id": "cycle:1", "plan": {}}).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    assert!(mutation(&out).is_none(), "nothing is invented here");
    let text = out[0]["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("revert_plan_missing_at_revert_time"),
        "{text}"
    );
}

/// The row a `store` update would write, if there is one.
fn updated(out: &[serde_json::Value], table: &str) -> Option<serde_json::Value> {
    out.iter().find_map(|m| {
        let text = m["messages"][0]["text"].as_str()?;
        let args: serde_json::Value = serde_json::from_str(text).ok()?;
        (args["operation"] == "update" && args["table"] == table).then(|| args["set"].clone())
    })
}

/// A revert order for `plan`, as the meter sends one.
fn revert_order(plan: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
            serde_json::json!({"op": "revert", "cycle_id": "cycle:1", "plan": plan}).to_string()}],
        "header": {"hop": {}, "context": {}}
    })
}

/// GH #304, fix round 2 — the guard binds the way back too.
///
/// The decide path refuses a key outside the radius; the revert path used to
/// emit unconditionally. A plan stored while the key was in the set, reverted
/// after an operator narrowed it (or a row that predates the set), produced a
/// `mutate` message nobody merges plus a receipt reading `reverted` — the same
/// untruthful-receipt class, on the other half.
///
/// What is asserted is the whole outcome, not just the absence of the message:
/// the receipt has to say `revert_refused`, and it has to put the cycle back to
/// `applied`, because that is the state the colony is genuinely in — the change
/// the loop failed to prove is still running.
#[test]
fn a_revert_whose_plan_left_the_radius_is_refused_rather_than_receipted_as_reverted() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "target": "/main/talky/collector", "kind": "numeric_param",
            "key": "max_iter", "to": 4
        })),
    );
    assert!(
        mutation(&out).is_none(),
        "nothing may go out on a plan the radius no longer carries: {out:?}"
    );
    let set = updated(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        set["outcome"], "revert_refused",
        "a way back nobody took must not read as taken: {set}"
    );
    assert_eq!(set["reason_code"], "key_outside_radius_max_iter");
    assert_eq!(
        set["status"], "applied",
        "the cycle stays the open one: the change is still standing, and the \
         meter scans exactly this status: {set}"
    );
}

/// The other direction of the same guard: an in-set plan still lands, and the
/// receipt still closes as `reverted`. A guard that refused everything would
/// pass the test above and break the loop.
#[test]
fn an_in_radius_revert_still_closes_the_cycle_as_reverted() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "target": BRAIN, "kind": "numeric_param",
            "key": "max_tokens", "to": 4096
        })),
    );
    let m = mutation(&out).expect("the inverse change goes out");
    assert_eq!(m["params"]["max_tokens"], serde_json::json!(4096));
    let set = updated(&out, "cycles").expect("a receipt");
    assert_eq!(set["status"], "closed");
    assert_eq!(set["outcome"], "reverted");
}

/// GH #304, fix round 3 — the oldest branch of the same defect class.
///
/// A revert order that arrives with no plan takes nothing, so the cycle was not
/// reverted. It used to write only its reason code, which left the meter's
/// `closed` / `reverted` standing: a receipt claiming a way back that was never
/// taken, with the reason muttering otherwise underneath it. The meter writes
/// that verdict in the same emission as the revert order (`meter` script
/// l.291-307), so there is no moment at which the row is not already closed.
///
/// Same mechanic as the radius refusal: the row is put back into the state the
/// colony is genuinely in, and the outcome names the refusal.
#[test]
fn a_revert_with_no_plan_leaves_a_row_that_does_not_claim_a_revert() {
    let out = emit(&script(MUTATOR), revert_order(serde_json::json!({})));
    assert!(mutation(&out).is_none(), "nothing is invented here");
    let set = updated(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        set["outcome"], "revert_refused",
        "no plan means no revert, and the row has to say so: {set}"
    );
    assert_eq!(set["reason_code"], "revert_plan_missing_at_revert_time");
    assert_eq!(
        set["status"], "applied",
        "the change the cycle failed to prove is still running: {set}"
    );
}

/// GH #326 — the way back is bound by the whole check, not by half of it.
///
/// The radius was only one of the decide path's bounds. A stored plan whose
/// `to` is empty passed the revert branch untouched and left as a params
/// update carrying `null`: nothing merges, and the receipt read `reverted`
/// over a colony still running the value the cycle failed to prove. Same
/// untruthful-receipt class as #304, one bound further in.
#[test]
fn a_revert_whose_plan_carries_no_value_is_refused_rather_than_sent_as_null() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "target": BRAIN, "kind": "numeric_param",
            "key": "max_tokens", "to": ""
        })),
    );
    assert!(
        mutation(&out).is_none(),
        "a params update carrying no value merges nothing: {out:?}"
    );
    let set = updated(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        set["outcome"], "revert_refused",
        "a way back nobody took must not read as taken: {set}"
    );
    assert_eq!(set["reason_code"], "no_new_value");
    assert_eq!(
        set["status"], "applied",
        "the change the cycle failed to prove is still running: {set}"
    );
}

/// GH #326 — a relative target addresses nothing.
///
/// `hop.target` is matched by edge conditions against absolute paths; a plan
/// stored with `talky/brain` produced a message no edge carries, under a
/// receipt reading `reverted`. The decide path has refused this shape since
/// the first version of `check()`.
#[test]
fn a_revert_whose_plan_names_a_relative_target_is_refused() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "target": "talky/brain", "kind": "numeric_param",
            "key": "max_tokens", "to": 4096
        })),
    );
    assert!(
        mutation(&out).is_none(),
        "no edge condition matches a relative target: {out:?}"
    );
    let set = updated(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        set["outcome"], "revert_refused",
        "a way back nobody took must not read as taken: {set}"
    );
    assert_eq!(set["reason_code"], "target_not_absolute");
    assert_eq!(
        set["status"], "applied",
        "the change the cycle failed to prove is still running: {set}"
    );
}

/// GH #326 — the step limit judges the way there, and must not re-judge the
/// way back.
///
/// The limit is a percentage of the value being left, so it is not symmetric:
/// 100 → 60 is a 40% step and passes, while 60 → 100 is a 67% step and would
/// not. A revert plan carries no `from` in the shape the judge is asked for,
/// which is why the limit reads as inert on this path — but "inert" was a fact
/// about the prompt, not about the code: the tool schema declares
/// `revert_plan` as a free-form object and this cell replays it verbatim. One
/// stray `from` and a legitimate revert would be refused as
/// `step_too_large_67_pct`, leaving exactly the unproven value standing that
/// the revert exists to remove — the guard turning on the thing it guards.
///
/// So the revert branch drops `from` before it validates, and this pin is the
/// construction.
#[test]
fn a_revert_plan_carrying_from_is_not_re_judged_against_the_step_limit() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "target": BRAIN, "kind": "numeric_param",
            "key": "max_tokens", "from": 60, "to": 100
        })),
    );
    let m = mutation(&out).expect("the way back is not a step to be judged");
    assert_eq!(m["params"]["max_tokens"], serde_json::json!(100));
    let set = updated(&out, "cycles").expect("a receipt");
    assert_eq!(set["status"], "closed");
    assert_eq!(
        set["outcome"], "reverted",
        "a revert refused for reversing a change the limit already allowed \
         would leave the unproven value running: {set}"
    );
}

/// GH #326 — a plan with no target at all is the same refusal.
///
/// It is not the missing-plan case: the row carries a plan, it just names
/// nobody. It used to leave with `hop.target` absent, which is a message the
/// colony cannot address, under a `reverted` receipt.
#[test]
fn a_revert_whose_plan_names_no_target_is_refused() {
    let out = emit(
        &script(MUTATOR),
        revert_order(serde_json::json!({
            "kind": "numeric_param", "key": "max_tokens", "to": 4096
        })),
    );
    assert!(
        mutation(&out).is_none(),
        "a params update addressed to nobody is not a way back: {out:?}"
    );
    let set = updated(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(
        set["outcome"], "revert_refused",
        "a way back nobody took must not read as taken: {set}"
    );
    assert_eq!(set["reason_code"], "target_not_absolute");
    assert_eq!(
        set["status"], "applied",
        "the change the cycle failed to prove is still running: {set}"
    );
}

// ---------------------------------------------------------------------------
// The measurement side
// ---------------------------------------------------------------------------

#[test]
fn an_inert_charter_makes_the_steward_do_nothing_at_all() {
    // The resting state of a freshly grown steward: every goal disabled. It
    // must be silent, not an error — nobody has said what to pursue yet.
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "goals", "st_carry": "{}"}}
        }),
    );
    assert!(out.is_empty(), "an inert charter emits nothing: {out:?}");
}

#[test]
fn a_tick_reads_the_charter_before_anything_else() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"schedule_name": "steward-cycle"}, "context": {}}
        }),
    );
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["table"], "goals");
    assert_eq!(args["where"]["enabled"], 1, "only enabled goals");
    assert_eq!(out[0]["header"]["route"], "cstore");
}

#[test]
fn too_little_traffic_closes_the_cycle_as_skipped_with_the_count() {
    // "We did not look" and "we looked and saw nothing" are different facts.
    let carry = serde_json::json!({
        "goals": [{"id": "goal:llm-cost", "metric": "llm_cost", "direction": "lower",
                   "window_minutes": 60, "min_samples": 30, "min_delta_pct": 10,
                   "quality_gate": "answer_quality", "enabled": 1}],
        "rules": []
    });
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "open", "st_carry": carry.to_string()}}
        }),
    );
    let row = inserted(&out, "cycles").expect("a receipt even when nothing happened");
    assert_eq!(row["outcome"], "skipped");
    assert!(
        row["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("below_min_samples_"),
        "the count is in the reason: {row}"
    );
    assert_eq!(row["status"], "closed");
}

#[test]
fn an_observe_only_goal_never_proposes_anything() {
    let carry = serde_json::json!({
        "goals": [{"id": "goal:dlq-watch", "metric": "dlq_rate", "direction": "observe",
                   "window_minutes": 60, "min_samples": 0, "min_delta_pct": 0,
                   "quality_gate": "", "enabled": 1}],
        "rules": []
    });
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "open", "st_carry": carry.to_string()}}
        }),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"),
        "an observe-only goal must not reach the judge: {out:?}"
    );
    assert_eq!(inserted(&out, "cycles").unwrap()["outcome"], "observed");
}

#[test]
fn no_model_is_reachable_from_the_measuring_path() {
    // The property that makes the numbers trustworthy: the meter and the probe
    // are code cells, and nothing in them talks to a provider.
    for path in [METER, PROBE] {
        let cfg = config(path);
        assert_eq!(cfg["cell"]["type"], "code", "{path}");
        // A `code` cell cannot call a provider by construction — it has no
        // api_key, no base_url and no model param, and there is no param that
        // would give it one.
        let params = cfg["params"].as_object().expect("params");
        for forbidden in ["api_key", "base_url", "provider", "model", "tools"] {
            assert!(
                !params.contains_key(forbidden),
                "{path} carries {forbidden}, which is llm-cell shaped"
            );
        }
        // …and the script itself reaches nothing over the network. Checked on
        // the imports rather than on the prose, so a comment mentioning a
        // provider does not fail a test about capability.
        let script = params["script_inline"].as_str().expect("script");
        for line in script.lines() {
            let l = line.trim_start();
            if l.starts_with("import ") || l.starts_with("from ") {
                for net in ["http", "urllib", "socket", "requests"] {
                    assert!(
                        !l.contains(net),
                        "{path} imports {net}: the measuring path must reach nothing"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------

/// The store call the probe wrote, if it wrote one of that operation.
fn probe_call(out: &[serde_json::Value], operation: &str) -> Option<serde_json::Value> {
    out.iter().find_map(|m| {
        let t = m["messages"][0]["text"].as_str()?;
        let a: serde_json::Value = serde_json::from_str(t).ok()?;
        (a["operation"] == operation).then_some(a)
    })
}

/// The order that puts the probe to work on one cycle.
fn probe_order(cycle: &str, target: &str) -> serde_json::Value {
    serde_json::json!({
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                      "text": serde_json::json!({"op": "probe", "cycle_id": cycle,
                                                 "target": target}).to_string()}],
        "header": {"hop": {}, "context": {}}
    })
}

#[test]
fn a_completed_params_update_cycle_reads_healthy() {
    // GH #338: the evidence a post-#304 cycle actually leaves behind is the
    // params update the mutator sent — on the `mutate` lane, carrying the
    // cycle's id, addressed at the cell the cycle names. The `mutation_log`
    // row here belongs to somebody else's rejected mutation; this hive authors
    // no diffs at all any more, so no steward row will ever say `committed`.
    let dir = TempDir::new().expect("tempdir");
    colony_db_with(
        dir.path(),
        &[
            ("mutation_log", "rejected", serde_json::Value::Null),
            (
                "message_log",
                "/main/talky/brain",
                serde_json::json!({"hop": {"route": "mutate", "cycle_id": "cycle:1",
                                           "outcome": "applied"}}),
            ),
        ],
    );
    let out = emit_in(
        dir.path(),
        &script(PROBE),
        probe_order("cycle:1", "/main/talky/brain"),
    );
    let update = probe_call(&out, "update").expect("the verdict is written to the receipt");
    assert_eq!(update["set"]["verified"]["verdict"], "healthy");
    assert_eq!(update["set"]["verified"]["reason"], "ok");
    assert!(
        probe_call(&out, "select").is_none(),
        "a healthy probe fetches no revert plan: {out:?}"
    );
}

#[test]
fn a_loop_whose_update_never_arrived_reads_unhealthy() {
    // Same ledger minus the one row that proves the change landed. The cell
    // the cycle names was never written to, so the loop does not know whether
    // it changed anything — and that is exactly when the way back is taken.
    let dir = TempDir::new().expect("tempdir");
    colony_db_with(
        dir.path(),
        &[("mutation_log", "rejected", serde_json::Value::Null)],
    );
    let out = emit_in(
        dir.path(),
        &script(PROBE),
        probe_order("cycle:1", "/main/talky/brain"),
    );
    let update = probe_call(&out, "update").expect("the verdict is written to the receipt");
    assert_eq!(update["set"]["verified"]["verdict"], "unhealthy");
    assert_eq!(
        update["set"]["verified"]["reason"],
        "params_update_not_seen"
    );
    let select = probe_call(&out, "select").expect("it fetches the plan authored beforehand");
    assert_eq!(select["table"], "cycles");
    assert_eq!(select["columns"][1], "revert_plan");
}

#[test]
fn a_probe_that_cannot_look_reports_unhealthy_rather_than_fine() {
    // Fail closed: "found nothing" and "found it healthy" must never read the
    // same. The colony.db path resolves to nothing in this working directory.
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({"op":"probe","cycle_id":"cycle:1",
                                   "target":"/main/talky/brain"}).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    let update = out
        .iter()
        .find_map(|m| {
            let t = m["messages"][0]["text"].as_str()?;
            let a: serde_json::Value = serde_json::from_str(t).ok()?;
            (a["operation"] == "update").then_some(a)
        })
        .expect("the verdict is written to the receipt");
    assert_eq!(update["set"]["verified"]["verdict"], "unhealthy");
    assert_eq!(update["set"]["verified"]["reason"], "probe_unavailable");

    // …and it goes looking for the revert plan rather than inventing one.
    let select = out
        .iter()
        .find_map(|m| {
            let t = m["messages"][0]["text"].as_str()?;
            let a: serde_json::Value = serde_json::from_str(t).ok()?;
            (a["operation"] == "select").then_some(a)
        })
        .expect("it fetches the plan that was authored beforehand");
    assert_eq!(select["columns"][1], "revert_plan");
}

#[test]
fn an_unhealthy_cycle_without_a_stored_plan_closes_for_a_human() {
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                          "text": "[{\"id\":\"cycle:1\",\"revert_plan\":{}}]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "plan", "st_cycle": "cycle:1",
                                   "st_reason": "unhealthy"}}
        }),
    );
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["set"]["outcome"], "unhealthy_no_plan");
    assert_eq!(args["set"]["status"], "closed");
}

#[test]
fn an_unhealthy_cycle_with_a_plan_reverts_at_once() {
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                "text": serde_json::json!([{
                    "id": "cycle:1",
                    "revert_plan": {"target":"/main/talky/brain","kind":"model",
                                    "to":"anthropic/claude-opus-4"}
                }]).to_string()}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "plan", "st_cycle": "cycle:1",
                                   "st_reason": "errors_3"}}
        }),
    );
    assert_eq!(out[0]["header"]["route"], "revert");
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["op"], "revert");
    assert_eq!(args["plan"]["to"], "anthropic/claude-opus-4");
}

// ---------------------------------------------------------------------------
// The shape of the hive itself
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_charter_pursues_nothing() {
    let seed = std::fs::read_to_string("../../templates/steward/charter/seed/goals.jsonl")
        .expect("the goals seed ships with the template");
    for line in seed.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line).expect("seed row");
        assert_eq!(
            row["enabled"], 0,
            "a freshly grown steward must change nothing until somebody means it: {row}"
        );
    }
}

#[test]
fn the_charter_carries_the_radius_and_the_revert_rule_as_data() {
    let seed = std::fs::read_to_string("../../templates/steward/charter/seed/rules.jsonl")
        .expect("the rules seed ships with the template");
    let rows: Vec<serde_json::Value> = seed
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("seed row"))
        .collect();
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    for required in [
        "radius",
        "require_revert_plan",
        "budget_eur_per_cycle",
        "quality_floor_pct",
    ] {
        assert!(
            kinds.contains(&required),
            "the charter is missing {required}"
        );
    }
    let radius = rows.iter().find(|r| r["kind"] == "radius").unwrap();
    assert_eq!(
        radius["value"], "model,numeric_params",
        "the radius widens by editing this row, never by editing code"
    );
}

#[test]
fn the_vault_of_this_hive_is_its_charter_and_its_stores_are_internal() {
    // Both stores declare an internal write surface: nothing outside the hive
    // writes the charter it is governed by, or the receipts it is judged on.
    for path in [CHARTER, "../../templates/steward/receipts/config.json"] {
        assert_eq!(
            config(path)["params"]["write_surface"],
            "internal",
            "{path} must not be writable from outside the hive"
        );
    }
}

#[test]
fn the_hive_is_sealed_to_its_own_path_and_states_its_lanes() {
    // GH #197: this used to pin `ports == ["meter", "mutator"]`, which spelled
    // out two CELL names — exactly what the boundary ruling of 2026-08-18 took
    // away. What it was protecting is the property below: nothing outside can
    // name anything inside, and what a caller may ask for is said in lanes.
    let hive = config(HIVE);
    let ports = hive["params"]["ports"]
        .as_array()
        .expect("the hive declares a port list");
    assert!(
        ports.is_empty(),
        "the hive path is the address and the lane is the port: {ports:?}"
    );

    let contract = hive["params"]["contract"]
        .as_object()
        .expect("a sealed hive owes a contract");
    let cells = [
        "charter", "clock", "judge", "meter", "mutator", "probe", "receipts",
    ];
    let mut lanes = 0usize;
    for side in ["accepts", "emits"] {
        for lane in contract[side].as_array().expect("accepts/emits is a list") {
            let route = lane["route"].as_str().expect("a lane names a route");
            assert!(
                !cells.contains(&route),
                "'{route}' is a cell of this hive — a lane says what a caller wants, never where \
                 it lands"
            );
            lanes += 1;
        }
    }
    assert!(
        lanes >= 3,
        "the contract says almost nothing: {lanes} lanes"
    );
}

#[test]
fn every_edge_of_the_hive_stays_inside_it() {
    let hive = config(HIVE);
    for edge in hive["params"]["graph"]["edges"].as_array().unwrap() {
        for role in ["from", "to"] {
            let ep = edge[role].as_str().unwrap();
            // `.` is the hive itself — the door and the exit of the sealed form
            // (GH #197), and the one endpoint that is still inside this subtree
            // without being below it. Everything else must be a child.
            assert!(
                ep == "." || ep.starts_with("./"),
                "a template has no edges leaving its own subtree: {ep}"
            );
        }
    }
}
