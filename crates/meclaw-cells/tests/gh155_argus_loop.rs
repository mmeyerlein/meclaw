//! The argus's loop, run against its real scripts (GitHub #155).
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
//! For the whole life of `argus@1.0.x` the decide path emitted a diff the
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

const MUTATOR: &str = "../../templates/argus/mutator/config.json";
const METER: &str = "../../templates/argus/meter/config.json";
const PROBE: &str = "../../templates/argus/probe/config.json";
const CHARTER: &str = "../../templates/argus/charter/config.json";
const HIVE: &str = "../../templates/argus/config.json";
const README: &str = "../../templates/argus/README.md";

/// The argus's shipped page, as bytes.
///
/// It is read here rather than trusted because § *Honest limits* and § *Lanes*
/// make **countable** promises about the two scripts beside it — how many ways
/// an answer can fail, which colony endpoints this hive may address, what
/// happens to a truncated one. `docs/development-rules.md` § R6 wants a promise
/// like that pinned to the mechanism that keeps it, so the tests below grep the
/// sentence and then assert the thing it claims.
fn readme() -> String {
    std::fs::read_to_string(README).expect("the argus ships a README")
}

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

/// Run one shipped script over one document and collect what it emitted.
///
/// The script travels to python3 **on stdin**, never in argv: a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped scripts have
/// grown to within a few KB of it (GH #279, precedent 89a522e4). stdin carries
/// the program, so the document rides inside it and is put under `sys.stdin`
/// before the script runs — same `__main__` globals, same stdout, same exit
/// status as `python3 -c` gave it.
///
/// There is no working-directory parameter any more: since GH #267 neither the
/// meter nor the probe opens a database, so no script in this file can see a
/// fixture on disk, and a test that wants one to read something has to hand it
/// the message that carries it.
fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

/// A judge decision arriving at the mutator.
/// A decision in the FLAT form — arguments straight in `text`.
///
/// This is what a tool call authored inside the hive looks like (the meter's
/// and the probe's `revert`/`probe` orders): a `code` cell writes what it
/// means. It is **not** what the judge's answer looks like, which is why
/// [`decision_off_the_wire`] exists beside it and why GH #462 found a defect
/// every test in this file had been agreeing with.
fn decision(args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string()
        }],
        "header": {"hop": {}, "context": {"ar_cycle": "cycle:1", "ar_goal": "goal:llm-cost"}}
    })
}

/// The same decision in the form the `llm` cell really emits.
///
/// `crates/meclaw-cells/src/llm/translate.rs` puts the provider's whole
/// FUNCTION OBJECT into the turn's `text` — `{"name": …, "arguments":
/// "<json string>"}` — so the decision sits one level in. Built here by hand
/// rather than taken from a fixture, because the point is the shape, and the
/// booted proof (`gh462_argus_runs_a_full_cycle.rs`) asserts the same thing
/// against the real cell.
fn decision_off_the_wire(args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": serde_json::json!({
                "name": "argus_change",
                "arguments": args.to_string()
            }).to_string()
        }],
        "header": {"hop": {}, "context": {"ar_cycle": "cycle:1", "ar_goal": "goal:llm-cost"}}
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

/// The cell the argus is allowed to change in these tests. A brain, because
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

/// GH #462 — **the judge's answer, in the shape the judge's answer actually
/// has.** The defect this pins was invisible to every other test in this file.
///
/// The mutator parsed the tool-call turn's `text` and read `action` out of it.
/// A turn off the wire carries `{"name": "argus_change", "arguments": "<json>"}`,
/// so `action` was absent, defaulted to `none`, and the cycle closed as
/// `no_action` over a colony where a model had in fact decided to change
/// something. The loop had never applied a change end to end — and could not
/// have, because the one shape it meets in production was the one shape nobody
/// fed it.
#[test]
fn a_decision_off_the_wire_is_read_the_same_as_one_authored_inside_the_hive() {
    let out = emit(&script(MUTATOR), decision_off_the_wire(a_valid_change()));
    let m = mutation(&out)
        .unwrap_or_else(|| panic!("the wire form has to reach the mutate lane too: {out:?}"));
    assert_eq!(m["header"]["target"], "/main/talky/brain", "{m}");
    assert_eq!(m["params"]["model"], "anthropic/claude-sonnet-4", "{m}");
    let row = inserted(&out, "cycles").expect("an applied cycle is a row");
    assert_eq!(row["status"], "applied", "{row}");
    assert_eq!(row["judged"]["action"], "change", "{row}");
    assert_the_declaration_admits(MUTATOR, &out);
}

/// The unwrapping must not eat a decision that was never wrapped: the meter's
/// and the probe's own orders travel flat, and one form must not be read as the
/// other.
#[test]
fn the_flat_form_still_decides_the_same_thing() {
    let wire = emit(&script(MUTATOR), decision_off_the_wire(a_valid_change()));
    let flat = emit(&script(MUTATOR), decision(a_valid_change()));
    let (w, f) = (
        mutation(&wire).expect("wire"),
        mutation(&flat).expect("flat"),
    );
    assert_eq!(w["params"], f["params"], "one decision, two envelopes");
    assert_eq!(w["header"]["target"], f["header"]["target"]);
}

/// GH #462 — an ERROR answered back to this cell is not a decision.
///
/// A target that refuses the params update replies straight to the mutator, and
/// that reply carries a `finish_reason` and no turns at all. It used to fall
/// through to the decide path, parse `{}` and write a cycle row for a decision
/// nobody took. The refusal is not lost — it is a colony-wide error inside the
/// probe's window, so the health check rules the cycle unhealthy and the revert
/// plan is taken — but it must not mint a receipt of its own.
#[test]
fn an_error_answered_back_to_the_mutator_is_not_a_decision() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"finish_reason": "error",
                               "error_code": "consumes_violation"},
                       "context": {}}
        }),
    );
    assert!(
        out.is_empty(),
        "a refusal is not a ruling and writes no row: {out:?}"
    );
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

/// GH #462 — the empty tick is written down.
///
/// The resting state of a freshly grown argus is every goal disabled, and it is
/// still not an error: nobody has said what to pursue yet. What changed is that
/// it is no longer SILENT. It used to emit nothing at all, which meant the
/// receipts table held evidence only of the moments something was decided — so
/// a watcher whose clock had stopped and a watcher with an empty charter wrote
/// the same nothing, and no reader could tell them apart. The chain is the
/// whole product here; a hole in it is the defect.
#[test]
fn an_inert_charter_leaves_an_idle_receipt_and_nothing_else() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"ar_phase": "goals", "ar_carry": "{}"}}
        }),
    );
    assert_eq!(
        out.len(),
        1,
        "an inert tick writes its receipt and reaches nowhere else: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("the idle tick inserts a cycles row");
    assert_eq!(row["outcome"], "idle", "{row}");
    assert_eq!(row["reason_code"], "no_enabled_goal", "{row}");
    assert_eq!(
        row["status"], "closed",
        "an idle tick opens nothing the next tick would have to close: {row}"
    );
    assert_eq!(
        row["goal"], "",
        "there was no goal — naming one would be an invention: {row}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"
            || m["header"]["route"] == "ledger"
            || m["header"]["route"] == "error"),
        "an inert charter reaches no model, no ledger and no error lane: {out:?}"
    );
}

/// GH #462 — a cycle that dies at a store reject leaves a receipt too, and
/// exactly one.
///
/// The error lane alone was what this branch used to do, which left the chain
/// with a hole precisely where a cycle had been. The bound on the new write is
/// its own phase: the receipt goes out on `serr`, so a reject OF THE RECEIPT
/// comes back naming that phase and only the error lane fires. One step, never
/// two — this cell has no memory to keep a counter in, so the phase IS the
/// counter.
#[test]
fn a_store_reject_leaves_a_receipt_and_the_error_lane() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"operation": "insert", "error_code": "schema"},
                       "context": {"ar_phase": "written"}}
        }),
    );
    assert_eq!(out.len(), 2, "a receipt and the error lane: {out:?}");
    let row = inserted(&out, "cycles").expect("the reject inserts a cycles row");
    assert_eq!(row["outcome"], "store_error", "{row}");
    assert_eq!(
        row["reason_code"], "store_rejected_insert_schema",
        "the row names the operation and the code that refused it: {row}"
    );
    let err = out
        .iter()
        .find(|m| m["header"]["route"] == "error")
        .expect("the error lane still fires");
    assert_eq!(
        err["header"]["phase"], "written",
        "the error names the phase the cycle died in: {err}"
    );
}

/// The recursion bound, measured rather than argued: the reject of the
/// store-error receipt itself writes no second receipt.
#[test]
fn a_reject_of_the_store_error_receipt_does_not_write_another() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"operation": "insert", "error_code": "schema"},
                       "context": {"ar_phase": "serr"}}
        }),
    );
    assert_eq!(
        out.len(),
        1,
        "only the error lane — a second receipt would chase its own tail: {out:?}"
    );
    assert_eq!(out[0]["header"]["route"], "error", "{out:?}");
    assert!(inserted(&out, "cycles").is_none(), "no second row: {out:?}");
}

#[test]
fn a_tick_reads_the_charter_before_anything_else() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"schedule_name": "argus-cycle"}, "context": {}}
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
    //
    // Since GH #267 the count arrives from `/colony/ledger` rather than from a
    // database this cell has no business opening, so the verdict is reached one
    // hop later — on the answer, not on the tick. What is asserted is unchanged:
    // the receipt, and the number in its reason code.
    let out = resume(
        "baseline",
        serde_json::json!({"goal": a_goal(), "prices": {}, "rules": [],
                           "cycle_id": "cycle:1"}),
        ledger_answer("wait:abc", "anthropic/claude-opus-4", 1, 100),
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
    let goal = serde_json::json!({"id": "goal:dlq-watch", "metric": "dlq_rate",
                                  "direction": "observe", "window_minutes": 60,
                                  "min_samples": 0, "min_delta_pct": 0,
                                  "quality_gate": "", "enabled": 1});
    let out = resume(
        "baseline",
        serde_json::json!({"goal": goal, "prices": {}, "rules": [],
                           "cycle_id": "cycle:1"}),
        ledger_answer("wait:abc", "anthropic/claude-opus-4", 4, 100),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"),
        "an observe-only goal must not reach the judge: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("an observation is a row");
    assert_eq!(row["outcome"], "observed");
    // GH #462: a clean watch and a watch that saw something must not read the
    // same. The ledger answer above carries no dead letters, so this one is
    // clean — and it says so in the metric's own name.
    assert_eq!(
        row["reason_code"], "observed_dlq_rate_clean",
        "the reason code names the metric and what it saw: {row}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "alert"),
        "nothing was seen, so nothing is alerted: {out:?}"
    );
}

/// GH #462 — the deterministic reaction path, and the point of the whole
/// `observe` direction: the count reaches the receipt AND leaves the hive, with
/// no model consulted anywhere along it.
///
/// Before this, every observation closed as `observe_only`, so a colony losing
/// letters and a colony losing none produced identical rows and nobody outside
/// the hive heard about either. The judge stays out of it on purpose — an error
/// rate is a symptom, and a loop that reacts to symptoms without a hypothesis is
/// a random walk with receipts.
#[test]
fn an_observed_symptom_is_counted_and_alerted_without_a_model() {
    let goal = serde_json::json!({"id": "goal:dlq-watch", "metric": "dlq_rate",
                                  "direction": "observe", "window_minutes": 60,
                                  "min_samples": 0, "min_delta_pct": 0,
                                  "quality_gate": "", "enabled": 1});
    let mut answer = ledger_answer("wait:abc", "anthropic/claude-opus-4", 4, 100);
    answer["dead_letters"] = serde_json::json!({"total": 3});
    let out = resume(
        "baseline",
        serde_json::json!({"goal": goal, "prices": {}, "rules": [],
                           "cycle_id": "cycle:1"}),
        answer,
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"),
        "a symptom is never handed to a model: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("an observation is a row");
    assert_eq!(row["outcome"], "observed", "{row}");
    assert_eq!(
        row["reason_code"], "observed_dlq_rate_3",
        "the count is IN the reason code — that is what makes two watches \
         distinguishable: {row}"
    );
    let alert = out
        .iter()
        .find(|m| m["header"]["route"] == "alert")
        .unwrap_or_else(|| panic!("the symptom leaves the hive: {out:?}"));
    let text = alert["messages"][0]["text"]
        .as_str()
        .expect("the alert carries a body");
    let payload: serde_json::Value = serde_json::from_str(text).expect("the alert body is json");
    assert_eq!(payload["metric"], "dlq_rate", "{payload}");
    assert_eq!(payload["value"], 3.0, "{payload}");
    assert_eq!(payload["goal"], "goal:dlq-watch", "{payload}");
    assert_eq!(payload["cycle_id"], "cycle:1", "{payload}");
}

// ---------------------------------------------------------------------------
// The ask/resume lane (GH #267, Wave D): the meter measures BY MESSAGE
//
// The numbers used to come out of `colony.db`, opened read-only by the script
// itself. A cell reading a database it does not own is forbidden with no
// exception left (`docs/meclaw-overview.md` § Datenbank-Isolation, GH #160), so
// the read became an ask at `/colony/ledger` — and an ask has to be waited for.
// The three properties below are the ones that can be wrong quietly: that the
// ask leaves a memory, that an answer finds the memory it belongs to, and that
// a refusal closes it rather than leaving the loop waiting for an answer it has
// already had.
// ---------------------------------------------------------------------------

/// A goal for the meter to pursue, with a window it can be asked about.
fn a_goal() -> serde_json::Value {
    serde_json::json!({"id": "goal:llm-cost", "metric": "llm_cost", "direction": "lower",
                       "window_minutes": 60, "min_samples": 30, "min_delta_pct": 10,
                       "quality_gate": "answer_quality", "enabled": 1})
}

/// The `open` phase with one enabled goal and no cycle still open: the pass
/// that used to measure in one breath and now has to ask.
fn a_fresh_cycle(goal: serde_json::Value, rules: serde_json::Value) -> Vec<serde_json::Value> {
    let carry = serde_json::json!({"goals": [goal], "rules": rules});
    emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"ar_phase": "open", "ar_carry": carry.to_string()}}
        }),
    )
}

/// The colony's answer to one ledger ask, in the shape `ReadLedgerReply`
/// serialises to (`crates/meclaw-colony/src/api_dto.rs`).
fn ledger_answer(tag: &str, model: &str, calls: u64, tokens: i64) -> serde_json::Value {
    serde_json::json!({
        "query": {"since": 1, "until": 2, "path_prefix": null, "cycle_id": null,
                  "group_by": "model", "tag": tag, "scan_budget": 50000},
        "messages": {"total": calls, "errors": 0, "path_prefix_total": 0,
                     "path_prefix_cycle_total": 0,
                     "by_model": {model: {"calls": calls, "tokens_prompt": tokens,
                                          "tokens_completion": tokens}}},
        "dead_letters": {"total": 0},
        "mutations": {"total": 0, "by_status": {}},
        "scan_truncated": false
    })
}

/// The meter resumed on an answer: the `waits` row comes back in the rows, the
/// answer itself travels in `ar_carry` (the `./meter -> ./receipts` edge puts
/// `hop.carry` there).
fn resume(
    kind: &str,
    wait_carry: serde_json::Value,
    ledger: serde_json::Value,
) -> Vec<serde_json::Value> {
    let rows = serde_json::json!([{
        "id": "wait:abc", "at": "2026-01-01T00:00:00.000000Z",
        "kind": kind, "carry": wait_carry, "status": "open"
    }]);
    emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                          "text": rows.to_string()}],
            "header": {"hop": {"operation": "select"},
                       "context": {"ar_phase": "wait", "ar_carry": ledger.to_string()}}
        }),
    )
}

/// The store call of that operation on that table, if the meter wrote one.
fn store_call(
    out: &[serde_json::Value],
    operation: &str,
    table: &str,
) -> Option<serde_json::Value> {
    out.iter().find_map(|m| {
        let text = m["messages"][0]["text"].as_str()?;
        let args: serde_json::Value = serde_json::from_str(text).ok()?;
        (args["operation"] == operation && args["table"] == table).then_some(args)
    })
}

#[test]
fn the_meter_asks_the_ledger_instead_of_opening_the_database() {
    let out = a_fresh_cycle(a_goal(), serde_json::json!([]));

    let row = inserted(&out, "waits").expect("the ask leaves a memory behind");
    assert_eq!(row["status"], "open");
    assert_eq!(row["kind"], "baseline");

    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "ledger")
        .expect("the window is asked for, not read: {out:?}");
    assert!(
        ask["query"]["since"].is_number(),
        "the ask names its window: {ask}"
    );
    assert_eq!(ask["query"]["group_by"], "model");
    assert_eq!(
        ask["query"]["tag"], row["id"],
        "the tag IS the wait row's id — that is how the answer finds it back"
    );

    // And the database read is gone, not merely unused: a script that can still
    // open `colony.db` is one edit away from doing it again.
    assert!(
        !script(METER).contains("sqlite3"),
        "the meter must not be able to open a database it does not own"
    );
}

#[test]
fn a_ledger_answer_resumes_the_cycle_it_belongs_to() {
    // A `/colony/*` reply is a FRESH message: no context, no hop of ours. So the
    // only thing that says which ask it answers is the tag it echoes.
    let out = emit(
        &script(METER),
        serde_json::json!({
            "ledger": ledger_answer("wait:abc", "anthropic/claude-opus-4", 12, 1000),
            "header": {"hop": {}, "context": {}}
        }),
    );
    assert_eq!(out.len(), 1, "one lookup, nothing else: {out:?}");
    let args = store_call(&out, "select", "waits").expect("the memory is read back");
    assert_eq!(
        args["where"],
        serde_json::json!({"id": "wait:abc"}),
        "the echoed tag selects the ask: {args}"
    );
    assert_eq!(out[0]["header"]["phase"], "wait");
    assert_eq!(out[0]["header"]["route"], "rstore");
}

#[test]
fn the_wait_row_carries_the_prices_and_the_goal() {
    // Whatever the answer needs to be turned into a verdict has to survive the
    // round trip, because nothing else does: prices are charter data and the
    // goal is what the number will be judged against.
    let rules = serde_json::json!([
        {"id": "r1", "kind": "price_per_mtok", "value": "anthropic/claude-opus-4=15/75"}
    ]);
    let out = a_fresh_cycle(a_goal(), rules);
    let row = inserted(&out, "waits").expect("a memory");
    assert_eq!(row["carry"]["goal"]["id"], "goal:llm-cost");
    assert_eq!(
        row["carry"]["prices"]["anthropic/claude-opus-4"],
        serde_json::json!([15.0, 75.0]),
        "the prices travel with the ask: {row}"
    );
}

#[test]
fn a_refused_ledger_read_closes_the_wait_instead_of_hanging() {
    // A filter the colony cannot read is refused without a `query` echo at all
    // (GH #341/#359) — so the tag is missing and the oldest open ask is the one
    // that was refused.
    let refusal = serde_json::json!({
        "status": "error", "error_code": "invalid_query",
        "details": "`query.group_by` must be \"model\""
    });
    let lookup = emit(
        &script(METER),
        serde_json::json!({"ledger": refusal.clone(), "header": {"hop": {}, "context": {}}}),
    );
    let args = store_call(&lookup, "select", "waits").expect("a refusal is still an answer");
    assert_eq!(
        args["where"],
        serde_json::json!({"status": "open"}),
        "with no tag, the open ask is the one that was refused: {args}"
    );

    let out = resume(
        "baseline",
        serde_json::json!({"goal": a_goal(), "prices": {}, "rules": [], "cycle_id": "cycle:1"}),
        refusal,
    );
    let receipt = inserted(&out, "cycles").expect("that nobody looked is a fact worth keeping");
    assert_eq!(receipt["outcome"], "skipped");
    assert_eq!(
        receipt["reason_code"], "unmeasured",
        "a refusal is not a measurement of zero: {receipt}"
    );
    assert!(
        receipt["measured"]["unavailable"]
            .as_str()
            .is_some_and(|s| s.contains("invalid_query")),
        "the refusal says why: {receipt}"
    );

    let closed = store_call(&out, "update", "waits").expect("the wait must be closed");
    assert_eq!(closed["set"]["status"], "done");
    assert_eq!(closed["where"]["id"], "wait:abc");
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "ledger"),
        "a refused read is not retried in the same breath: {out:?}"
    );
}

#[test]
fn a_truncated_ledger_answer_is_not_a_measurement() {
    // GH #385 — the third way an answer can fail to be an answer, and the only
    // one that used to fail OPEN. `scan_truncated` says a windowed sub-query
    // hit its budget, so the counts beside it are a PART of the window
    // (`api_dto.rs`, `ReadLedgerReply::scan_truncated`). A part of a cost is
    // not a small cost; it is no cost at all, and a keep/revert ruling taken
    // against it is taken against a number nobody measured.
    let mut answer = ledger_answer("wait:abc", "anthropic/claude-opus-4", 12, 1000);
    answer["scan_truncated"] = serde_json::json!(true);

    let out = resume(
        "baseline",
        serde_json::json!({"goal": a_goal(), "prices": {}, "rules": [], "cycle_id": "cycle:1"}),
        answer,
    );
    let receipt = inserted(&out, "cycles").expect("that nobody looked is a fact worth keeping");
    assert_eq!(receipt["outcome"], "skipped");
    assert_eq!(
        receipt["reason_code"], "unmeasured",
        "a partial window is not a measurement of anything: {receipt}"
    );
    assert!(
        receipt["measured"]["unavailable"]
            .as_str()
            .is_some_and(|s| s.contains("scan_truncated")),
        "the receipt says which of the three refusals it was: {receipt}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"),
        "a truncated baseline never reaches the judge: {out:?}"
    );

    // The ask is closed all the same — an answer that has already arrived must
    // not leave its memory open forever.
    let closed = store_call(&out, "update", "waits").expect("the wait must be closed");
    assert_eq!(closed["set"]["status"], "done");
    assert_eq!(closed["where"]["id"], "wait:abc");
}

/// The `effect` half of the wait: an applied cycle whose window has passed, in
/// the shape the `open` phase remembers it (`wait_row("effect", …)`).
fn an_applied_cycle(before_cost_eur: f64) -> serde_json::Value {
    serde_json::json!({
        "goal": a_goal(),
        "prices": {"anthropic/claude-opus-4": [15.0, 75.0]},
        "rules": [],
        "cycle_id": "cycle:7",
        "row": {
            "id": "cycle:7",
            "goal": "goal:llm-cost",
            "at": "2026-01-01T00:00:00.000000Z",
            "status": "applied",
            "measured": {"cost_eur": before_cost_eur},
            "change": {"target": BRAIN, "kind": "model", "to": "anthropic/claude-haiku-4"},
            "revert_plan": {"target": BRAIN, "kind": "model",
                            "to": "anthropic/claude-opus-4"}
        }
    })
}

#[test]
fn an_effect_answer_that_proves_the_improvement_keeps_the_change() {
    // The `baseline` half of the wait lane is pinned above; this is the other
    // half, and it is the one that decides. A cycle sits `applied` until an
    // answer to the SECOND ask arrives, and the ruling taken on that answer is
    // the loop's whole point: 100 prompt + 100 completion tokens at 15/75 per
    // Mtok is 0.009 EUR against a baseline of 0.02, which clears the goal's
    // 10 % floor.
    let out = resume(
        "effect",
        an_applied_cycle(0.02),
        ledger_answer("wait:abc", "anthropic/claude-opus-4", 12, 100),
    );

    let update = store_call(&out, "update", "cycles").expect("the applied cycle is ruled on");
    assert_eq!(
        update["where"]["id"], "cycle:7",
        "the ruling names its cycle"
    );
    assert_eq!(update["set"]["status"], "closed");
    assert_eq!(update["set"]["outcome"], "kept");
    assert!(
        update["set"]["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("improved_"),
        "the margin is in the reason: {update}"
    );
    assert_eq!(
        update["set"]["effect"]["before"],
        serde_json::json!(0.02),
        "both numbers are on the receipt, not just the verdict: {update}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "revert"),
        "a proven improvement is not taken back: {out:?}"
    );
    let closed = store_call(&out, "update", "waits").expect("the wait must be closed");
    assert_eq!(closed["set"]["status"], "done");
    assert_eq!(closed["where"]["id"], "wait:abc");
}

#[test]
fn an_effect_answer_that_proves_nothing_reverts_by_the_stored_plan() {
    // The same lane, the other verdict. 0.009 EUR against a baseline of 0.001
    // is worse, so the change goes back — and it goes back by the plan that was
    // authored BEFORE it was applied, which is the charter rule the whole loop
    // is built around.
    let out = resume(
        "effect",
        an_applied_cycle(0.001),
        ledger_answer("wait:abc", "anthropic/claude-opus-4", 12, 100),
    );

    let update = store_call(&out, "update", "cycles").expect("the applied cycle is ruled on");
    assert_eq!(update["set"]["outcome"], "reverted");
    assert!(
        update["set"]["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("not_proven_"),
        "the margin is in the reason: {update}"
    );

    let order = out
        .iter()
        .find(|m| m["header"]["route"] == "revert")
        .expect("a reverted cycle sends the revert order");
    let args: serde_json::Value =
        serde_json::from_str(order["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["op"], "revert");
    assert_eq!(args["cycle_id"], "cycle:7");
    assert_eq!(
        args["plan"]["to"], "anthropic/claude-opus-4",
        "the way back is the stored one, never one invented now: {args}"
    );
    assert_eq!(order["header"]["cycle_id"], "cycle:7");

    let closed = store_call(&out, "update", "waits").expect("the wait must be closed");
    assert_eq!(closed["set"]["status"], "done");
    assert_eq!(closed["where"]["id"], "wait:abc");
}

// ---------------------------------------------------------------------------
// The page and the mechanism (GH #267, rule R6)
//
// Three sentences on the shipped README count something: the three shapes an
// answer can fail in, the two colony endpoints this hive may address, and what
// a truncated answer is worth. A README is a public surface; a countable claim
// on one is a promise, and a promise nothing holds is the stale documentation
// `docs/development-rules.md` forbids. So each of the three is greped here and
// then checked against the scripts that are supposed to keep it.
// ---------------------------------------------------------------------------

#[test]
fn the_readme_names_the_three_failure_forms_and_both_scripts_map_them() {
    let doc = readme();
    for form in ["`unavailable`", "`invalid_query`", "`scan_truncated`"] {
        assert!(
            doc.contains(form),
            "§ Honest limits promises three failure forms and must name {form}"
        );
    }
    assert!(
        doc.contains("**three** ways"),
        "the count is the promise: three, not 'several'"
    );

    // And each of the three is a shape the scripts actually recognise. The
    // meter turns them into `unmeasured`, the probe into `probe_unavailable`;
    // what is asserted here is only that neither can meet one and read on.
    for path in [METER, PROBE] {
        let s = script(path);
        for form in ["unavailable", "invalid_query", "scan_truncated"] {
            assert!(s.contains(form), "{path} does not know about {form}");
        }
    }
}

#[test]
fn the_readme_promises_a_truncated_answer_is_discarded_and_it_is() {
    let doc = readme();
    assert!(
        doc.contains("**discarded**"),
        "§ Configuration promises a truncated answer is discarded (GH #385)"
    );
    assert!(
        doc.contains("scan_truncated: partial counts"),
        "§ Honest limits names the verdict a busy colony sees"
    );

    // The promise, kept: the same string, out of the shipped scripts.
    for path in [METER, PROBE] {
        assert!(
            script(path).contains("scan_truncated: partial counts"),
            "{path} must refuse a partial window in the words the README uses"
        );
    }
}

#[test]
fn the_readme_carve_out_is_the_pair_the_edge_test_holds() {
    let doc = readme();
    for endpoint in COLONY_READS {
        assert!(
            doc.contains(endpoint),
            "§ Lanes names the sanctioned colony reads and must name {endpoint}"
        );
    }
    assert!(
        doc.contains("The two of them are the whole list"),
        "the carve-out is a closed list on the page as well as in the test"
    );
    assert_eq!(COLONY_READS.len(), 2, "two, and the page says two");

    // The retracted knob, and the retraction that says so out loud.
    assert!(
        doc.contains("`ARGUS_COLONY_DB` is gone"),
        "a deleted setting is retracted on the page, never quietly dropped"
    );
    for path in [METER, PROBE] {
        let s = script(path);
        assert!(
            !s.contains("ARGUS_COLONY_DB"),
            "{path} still reads the knob"
        );
        assert!(!s.contains("sqlite3"), "{path} can still open a database");
    }
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

/// The colony's answer to one probe ask, in the shape `ReadLedgerReply`
/// serialises to (`crates/meclaw-colony/src/api_dto.rs`).
///
/// `path_prefix_cycle_total` is the whole health question in one number: how
/// many messages of this cycle reached that path. The tag comes back verbatim,
/// and it is the only thing in the answer that says which cycle — and which
/// attempt — it belongs to.
fn probe_answer(tag: &str, target: &str, of_this_cycle: u64, errors: u64) -> serde_json::Value {
    serde_json::json!({
        "query": {"since": 1, "until": 2, "path_prefix": target, "cycle_id": "cycle:1",
                  "group_by": null, "tag": tag, "scan_budget": 50000},
        "messages": {"total": 7, "errors": errors, "path_prefix_total": 3,
                     "path_prefix_cycle_total": of_this_cycle, "by_model": {}},
        "dead_letters": {"total": 0},
        "mutations": {"total": 0, "by_status": {}},
        "scan_truncated": false
    })
}

/// The probe resumed on an answer. A `/colony/*` reply is a FRESH message: no
/// `context`, no `hop` of ours, which is exactly why the tag has to carry the
/// cycle.
fn probe_resume(ledger: serde_json::Value) -> Vec<serde_json::Value> {
    emit(
        &script(PROBE),
        serde_json::json!({"ledger": ledger, "header": {"hop": {}, "context": {}}}),
    )
}

/// The one ask the probe emitted, if it emitted one.
fn probe_ask(out: &[serde_json::Value]) -> Option<&serde_json::Value> {
    out.iter().find(|m| m["header"]["route"] == "ledger")
}

#[test]
fn the_probe_asks_the_ledger_and_carries_its_cycle_in_the_tag() {
    // The probe has no `cell.db`, so it cannot remember an ask the way the
    // meter does. What it has instead is the echo: the tag names the cycle AND
    // the attempt, and comes back verbatim, so the answer is self-describing.
    let out = emit(&script(PROBE), probe_order("cycle:1", BRAIN));
    assert_eq!(out.len(), 1, "one ask, nothing else: {out:?}");
    assert_the_declaration_admits(PROBE, &out);

    let ask = probe_ask(&out).expect("the window is asked for, not read: {out:?}");
    assert_eq!(ask["query"]["tag"], "cycle:1#1", "first attempt: {ask}");
    assert_eq!(ask["query"]["cycle_id"], "cycle:1");
    assert_eq!(ask["query"]["path_prefix"], BRAIN);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let since = ask["query"]["since"]
        .as_i64()
        .expect("the ask names a window");
    // 120 s is the shipped `probe_window_sec`; the slack is for the seconds
    // this test spends spawning a python.
    assert!(
        (now - 120 - since).abs() <= 5,
        "the window is now minus probe_window_sec: {since} against {now}"
    );
}

#[test]
fn a_completed_params_update_cycle_reads_healthy() {
    // GH #338: the evidence a post-#304 cycle leaves behind is the params
    // update the mutator sent, carrying the cycle's id, addressed at the cell
    // the cycle names. Since GH #267 that evidence is counted by the colony
    // and arrives as a number rather than being read out of a database this
    // cell has no business opening.
    let out = probe_resume(probe_answer("cycle:1#1", BRAIN, 1, 0));
    let update = probe_call(&out, "update").expect("the verdict is written to the receipt");
    assert_eq!(update["set"]["verified"]["verdict"], "healthy");
    assert_eq!(update["set"]["verified"]["reason"], "ok");
    assert!(
        probe_call(&out, "select").is_none(),
        "a healthy probe fetches no revert plan: {out:?}"
    );
    assert!(
        probe_ask(&out).is_none(),
        "an answered question is not asked again: {out:?}"
    );
}

#[test]
fn a_ledger_answer_verdicts_the_cycle_the_tag_names() {
    // The receipt has to name a cycle, and with an empty context there is
    // nowhere to get one from except the echo. A verdict written against the
    // wrong id — or against none — is worse than no verdict at all.
    let out = probe_resume(probe_answer("cycle:1#1", BRAIN, 1, 0));
    assert_the_declaration_admits(PROBE, &out);
    let update = probe_call(&out, "update").expect("the verdict is written to the receipt");
    assert_eq!(update["table"], "cycles");
    assert_eq!(
        update["where"],
        serde_json::json!({"id": "cycle:1"}),
        "the tag is the only thing that says which cycle: {update}"
    );
    assert_eq!(update["set"]["verified"]["verdict"], "healthy");
}

#[test]
fn a_loop_whose_update_never_arrived_reads_unhealthy() {
    // The load-bearing verdict of the whole health check. A count of zero is a
    // real answer, but it is also the answer a write still in flight gives, so
    // the first zero buys a re-ask rather than a revert…
    let again = probe_resume(probe_answer("cycle:1#1", BRAIN, 0, 0));
    assert_eq!(again.len(), 1, "one re-ask, nothing else: {again:?}");
    let ask = probe_ask(&again).expect("a zero at attempt 1 is asked again");
    assert_eq!(
        ask["query"]["tag"], "cycle:1#2",
        "the attempt is incremented in the echo, because nothing else remembers it: {ask}"
    );
    assert!(
        probe_call(&again, "update").is_none(),
        "no verdict is written while the question is still open: {again:?}"
    );

    // …and at the bound the cell the cycle names was never written to, so the
    // loop does not know whether it changed anything — which is exactly when
    // the way back is taken.
    let out = probe_resume(probe_answer("cycle:1#3", BRAIN, 0, 0));
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
fn the_re_ask_is_bounded_by_the_tries_setting() {
    // The bound lives in the script, not in the caller's patience: at
    // `probe_ledger_tries` the probe verdicts instead of asking a fourth time.
    // Without this the lane is a loop with no exit — the answer that would end
    // it is the one that never comes.
    let out = probe_resume(probe_answer("cycle:1#3", BRAIN, 0, 0));
    assert!(
        probe_ask(&out).is_none(),
        "three tries are three asks, not four: {out:?}"
    );
    assert!(
        probe_call(&out, "update").is_some(),
        "the bound is reached by verdicting, not by falling silent: {out:?}"
    );
}

#[test]
fn a_probe_that_cannot_look_reports_unhealthy_rather_than_fine() {
    // Fail closed: "found nothing" and "found it healthy" must never read the
    // same. Two shapes can fail to be an answer (GH #341/#359) — a read that
    // could not happen answers `unavailable` beside its echo, and a filter that
    // could not be read is refused with `status == "error"` and carries no
    // counts and no echo at all. Neither is a healthy colony.
    let unavailable = serde_json::json!({
        "query": {"since": 1, "until": 2, "path_prefix": BRAIN, "cycle_id": "cycle:1",
                  "group_by": null, "tag": "cycle:1#1", "scan_budget": 50000},
        "unavailable": "disk I/O error"
    });
    let refused = serde_json::json!({
        "status": "error", "error_code": "invalid_query",
        "details": "`query.cycle_id` must be at most 64 characters"
    });
    for answer in [unavailable, refused] {
        let out = probe_resume(answer.clone());
        let update = probe_call(&out, "update")
            .unwrap_or_else(|| panic!("the verdict is written to the receipt: {out:?}"));
        assert_eq!(
            update["set"]["verified"]["verdict"], "unhealthy",
            "on {answer}"
        );
        assert_eq!(
            update["set"]["verified"]["reason"], "probe_unavailable",
            "on {answer}"
        );

        // …and it goes looking for the revert plan rather than inventing one.
        let select = probe_call(&out, "select")
            .unwrap_or_else(|| panic!("it fetches the plan authored beforehand: {out:?}"));
        assert_eq!(select["columns"][1], "revert_plan");

        // A refusal is not a slow answer: asking again would be asking the
        // same broken question.
        assert!(probe_ask(&out).is_none(), "not retried: {out:?}");
    }
}

#[test]
fn a_truncated_scan_is_not_a_healthy_colony() {
    // GH #385 — the fail-open hole in a loop that is fail-closed everywhere
    // else. A budget-exhausted scan counts a PART of the window, and the part
    // it counts can only ever be missing errors, missing dead letters and
    // missing silence. Read as countable, it turns unhealthy into healthy —
    // the one direction this verdict must never fail in.
    let mut answer = probe_answer("cycle:1#1", BRAIN, 1, 0);
    answer["scan_truncated"] = serde_json::json!(true);

    let out = probe_resume(answer);
    let update = probe_call(&out, "update").expect("the verdict is written to the receipt");
    assert_eq!(
        update["set"]["verified"]["verdict"], "unhealthy",
        "a partial count of zero errors is not zero errors: {update}"
    );
    assert_eq!(update["set"]["verified"]["reason"], "probe_unavailable");
    assert!(
        update["set"]["verified"]["unavailable"]
            .as_str()
            .is_some_and(|s| s.contains("scan_truncated")),
        "the receipt says which of the three refusals it was: {update}"
    );

    // …and, like every other unhealthy verdict, it goes for the plan the judge
    // authored beforehand rather than inventing one.
    let select = probe_call(&out, "select").expect("it fetches the plan authored beforehand");
    assert_eq!(select["columns"][1], "revert_plan");

    // Truncation is not a race: the same question would truncate again.
    assert!(probe_ask(&out).is_none(), "not retried: {out:?}");
}

#[test]
fn the_probe_names_the_scan_budget_it_wants() {
    // Once truncation is fail-closed, a small budget stops being a slow answer
    // and becomes a spurious revert. So the ask names the ceiling the colony
    // allows instead of riding on whatever the endpoint defaults to.
    let out = emit(&script(PROBE), probe_order("cycle:1", BRAIN));
    let ask = probe_ask(&out).expect("the window is asked for, not read");
    assert_eq!(
        ask["query"]["scan_budget"],
        serde_json::json!(200000),
        "the health check asks for as much as the colony will read: {ask}"
    );
}

#[test]
fn no_sqlite_survives_in_the_probe() {
    // Removed, not merely unused: a script that can still open `colony.db` is
    // one edit away from doing it again.
    assert!(
        !script(PROBE).contains("sqlite3"),
        "the probe must not be able to open a database it does not own"
    );
}

#[test]
fn an_unhealthy_cycle_without_a_stored_plan_closes_for_a_human() {
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                          "text": "[{\"id\":\"cycle:1\",\"revert_plan\":{}}]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"ar_phase": "plan", "ar_cycle": "cycle:1",
                                   "ar_reason": "unhealthy"}}
        }),
    );
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["set"]["outcome"], "unhealthy_no_plan");
    assert_eq!(args["set"]["status"], "closed");
    // GH #462: and it tells somebody. `./probe -> .` on `error` was declared
    // from the first version of this hive and no branch had ever emitted it, so
    // a loop stuck in exactly this state — colony unhealthy, change standing, no
    // plan to take it back — was indistinguishable from a quiet one out at the
    // rim. This is the one place in the cycle where only a human can act, which
    // is what makes it the place the lane belongs to.
    let err = out
        .iter()
        .find(|m| m["header"]["route"] == "error")
        .unwrap_or_else(|| panic!("the stuck cycle has to leave the hive: {out:?}"));
    assert_eq!(err["header"]["cycle_id"], "cycle:1", "{err}");
    assert_eq!(err["header"]["verdict"], "unhealthy", "{err}");
    let text = err["messages"][0]["text"]
        .as_str()
        .expect("an error says why");
    assert!(
        text.contains("no revert plan"),
        "the error names what is wrong, not merely that something is: {text}"
    );
}

/// GH #462 — every lane this hive declares on the way OUT has a shipped emitter
/// and an edge that carries it off the hive path.
///
/// The dead-lane class (`docs/development-rules.md` § 2c): a declared, unmarked
/// lane with no traffic is prose, and prose outlives its mechanism silently.
/// `error` on `./probe` was exactly that for the whole life of this template's
/// predecessor — declared in the contract, wired as an edge, emitted by nothing.
/// Grepping the emitters alone would pin a string; asserting the edges alone
/// would pin a table. This asks for both, per declared lane.
#[test]
fn every_declared_out_lane_has_an_emitter_and_an_edge() {
    let hive = config(HIVE);
    // The scripts with their comment lines dropped and their whitespace
    // collapsed. Both halves are needed: all three scripts MENTION every lane in
    // their prose, so a raw substring match would call a paragraph an emitter,
    // and a real emission is regularly wrapped across lines.
    let code_of = |path: &str| -> String {
        script(path)
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<String>()
    };
    let scripts = [
        ("meter", code_of(METER)),
        ("mutator", code_of(MUTATOR)),
        ("probe", code_of(PROBE)),
    ];
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive draws a graph");

    let out_lanes = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("the contract declares what leaves");
    assert!(
        out_lanes.len() >= 3,
        "an empty result and a forgotten call must never look alike: this hive \
         declares mutate, alert and error, and a run over fewer lanes than that \
         is the test passing vacuously, not the tree being clean"
    );
    for lane in out_lanes {
        let route = lane["route"].as_str().expect("a lane names a route");
        assert!(
            edges.iter().any(|e| e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{route}'")))),
            "no edge carries `{route}` off the hive path"
        );
        // `emit("<route>"` and not a bare substring: all three scripts MENTION
        // every lane in their comments, so a substring match would call a
        // paragraph an emitter. What is asked for here is the emission call
        // itself, which is the only thing that puts a message on the lane.
        let emitters: Vec<&str> = scripts
            .iter()
            .filter(|(_, src)| src.contains(&format!("emit(\"{route}\"")))
            .map(|(name, _)| *name)
            .collect();
        assert!(
            !emitters.is_empty(),
            "no shipped script raises `{route}` — a lane nothing emits is prose"
        );
    }
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
                       "context": {"ar_phase": "plan", "ar_cycle": "cycle:1",
                                   "ar_reason": "errors_3"}}
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
    let seed = std::fs::read_to_string("../../templates/argus/charter/seed/goals.jsonl")
        .expect("the goals seed ships with the template");
    for line in seed.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line).expect("seed row");
        assert_eq!(
            row["enabled"], 0,
            "a freshly grown argus must change nothing until somebody means it: {row}"
        );
    }
}

#[test]
fn the_charter_carries_the_radius_and_the_revert_rule_as_data() {
    let seed = std::fs::read_to_string("../../templates/argus/charter/seed/rules.jsonl")
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
    for path in [CHARTER, "../../templates/argus/receipts/config.json"] {
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

/// The read-only colony endpoints a sealed template may address, and the whole
/// list of them.
///
/// These are not cells and they are not outside the seal in the sense the seal
/// is about: they are the substrate answering a question about itself, over an
/// ordinary message, to a caller that gets counts rather than reach.
/// `/colony/graph` has shipped as a mutation-drawn edge out of a sealed hive
/// since [GH #163](https://github.com/mmeyerlein/meclaw/issues/163)
/// (`templates/canvy/config.json` draws `./probe -> /colony/graph`), so the
/// argus's `/colony/ledger` edges follow a precedent rather than set one.
///
/// The list is literal on purpose. A foreign **cell** path is still forbidden,
/// and so is every `/colony/*` endpoint that is not one of these two —
/// `/colony/mutations` above all, which would hand this hive every cell in the
/// tree at once.
///
/// **This is the STEWARD's list, not the substrate's, and since 2026-08-27 the
/// two differ.** The check below used to assert equality with
/// `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`; it now asserts containment, and the
/// difference is the point. The substrate gained `/colony/registry` for the
/// agentic builder's second eye — a name this hive has no use for. Equality
/// would have forced the argus's page and its carve-out to grow a lane the
/// argus does not draw, which is the opposite of what a carve-out is for.
/// Containment keeps the only direction that can go wrong: this list may never
/// vouch for an endpoint the substrate refuses, so a name added HERE without
/// being sanctioned THERE is red on the next run.
const COLONY_READS: &[&str] = &["/colony/graph", "/colony/ledger"];

#[test]
fn every_edge_of_the_hive_stays_inside_it() {
    // The carve-out is only as true as the substrate's own list. Containment,
    // not equality (see `COLONY_READS`): a shorter local list is a stricter
    // local rule and stays legal, but a name this test vouches for that the
    // substrate would refuse is a promise no mutation could keep.
    for endpoint in COLONY_READS {
        assert!(
            meclaw_colony::mutation::MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS.contains(endpoint),
            "{endpoint} is not one the substrate lets a mutation draw"
        );
    }
    let hive = config(HIVE);
    for edge in hive["params"]["graph"]["edges"].as_array().unwrap() {
        for role in ["from", "to"] {
            let ep = edge[role].as_str().unwrap();
            // `.` is the hive itself — the door and the exit of the sealed form
            // (GH #197), and the one endpoint that is still inside this subtree
            // without being below it. Everything else must be a child, or one
            // of the two sanctioned read-only colony endpoints.
            assert!(
                ep == "." || ep.starts_with("./") || COLONY_READS.contains(&ep),
                "a template has no edges leaving its own subtree, and a foreign \
                 cell path is not made one by living under /colony: {ep}"
            );
        }
    }
}
