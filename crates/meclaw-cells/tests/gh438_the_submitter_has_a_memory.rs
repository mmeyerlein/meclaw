//! GH #438 — the submitter keeps one row per submission in flight.
//!
//! A `code` cell has no `cell.db` (`docs/cell-types.md` § code), so the memory
//! of the submit hive is a `store` beside the gate and the round trip to it.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/config.json"
);
const STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/store/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const REQUESTER: &str = "/os/orgs/acme/members/alex/assistants/scribe/tools/apply";

fn read(path: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(path).expect(path)).expect(path)
}

/// The canonical digest of a declaration list, drawn by the same two lines the
/// shipped helper draws it with.
fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    let out = run_shipped_script(program, &decls.to_string());
    String::from_utf8(out.stdout).expect("hex")
}

fn one_declaration() -> Value {
    json!([{ "scope": "/", "ctx": {}, "diff": { "add_edges": [] } }])
}

/// Phase A: a submission arriving on `in_apply`.
fn submit(decls: &Value, claimed: &str, reply_to: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": reply_to,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": claimed,
                                 "tool_call_id": "c2" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [{ "origin": "assistant", "type": "tool_call",
                           "id": "c2", "text": "{}" }],
            "params": {}
        }),
    )
}

/// The one operation a store message carries, parsed out of its `tool_call`.
fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

#[test]
fn the_hive_wires_the_gate_to_a_store_and_back() {
    let hive = read(HIVE);
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");

    let out = edges
        .iter()
        .find(|e| e["from"] == "./gate" && e["to"] == "./store")
        .expect("gate -> store");
    assert_eq!(out["condition"], "has(hop.route) && hop.route == 'sstore'");
    let m = &out["modifier"]["set_context"];
    // The key space is the submitter's OWN: `access_origin`/`ac_*` are
    // overwritten on the broker's internal edges, so a marker under those
    // names would not survive the phase-2 round trip.
    assert_eq!(m["sub_origin"], "'gate'");
    assert_eq!(m["sub_phase"], "hop.phase");
    assert_eq!(m["sub_carry"], "hop.carry");

    let back = edges
        .iter()
        .find(|e| e["from"] == "./store" && e["to"] == "./gate")
        .expect("store -> gate");
    assert_eq!(back["condition"], "context.sub_origin == 'gate'");
}

#[test]
fn the_store_is_sealed_against_writers_from_outside_the_scope() {
    let store = read(STORE);
    assert_eq!(store["cell"]["type"], "store");
    assert_eq!(store["contract"]["write_surface"], "internal");
    let cols = &store["params"]["schema"]["submissions"];
    for (col, ty) in [
        ("id", "text"),
        ("at", "text"),
        ("tool_call_id", "text"),
        ("manifest_sha256", "text"),
        ("requester", "text"),
        ("kind", "text"),
        ("manifest", "json"),
    ] {
        assert_eq!(cols[col], ty, "column {col}");
    }
}

#[test]
fn phase_a_writes_the_row_before_it_does_anything_else() {
    let decls = one_declaration();
    let sha = digest_of(&decls);
    let out = submit(&decls, &sha, REQUESTER);

    assert_eq!(out.len(), 2, "one remembering, one asking");

    // 1. the row
    let h0 = &out[0]["header"];
    assert_eq!(h0["route"], "sstore");
    assert_eq!(h0["phase"], "written");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "submissions");
    // GH #435 moved the decision to the broker, so what phase A remembers is
    // the manifest itself, PARKED under its digest — the round trip to the
    // broker replaces the body it travelled in. The `flight` row that carries
    // the receipt correlation is written one phase later, when the verdict
    // un-parks it, and it is the same row with the same columns.
    assert_eq!(op["row"]["kind"], "parked");
    assert_eq!(op["row"]["tool_call_id"], "c2");
    assert_eq!(op["row"]["manifest_sha256"], sha);
    assert_eq!(op["row"]["requester"], REQUESTER);
    assert_eq!(op["row"]["manifest"], decls);
    // fixed-width microseconds: `at` is ordered as TEXT, and a tie in a FIFO
    // is a lost correlation. Same lesson as access/policy's now().
    let at = op["row"]["at"].as_str().expect("at");
    assert_eq!(at.len(), 27, "YYYY-MM-DDTHH:MM:SS.ffffffZ");

    // 2. the question
    assert_eq!(out[1]["header"]["route"], "ask");
    assert_eq!(out[1]["header"]["manifest_sha256"], sha);
}

#[test]
fn the_call_id_falls_back_to_the_hop_when_the_turn_has_none() {
    let decls = one_declaration();
    let sha = digest_of(&decls);
    let out = emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": REQUESTER,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": sha,
                                 "tool_call_id": "c9" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            // No turn at all — the id has nowhere else to come from.
            "messages": [],
            "params": {}
        }),
    );
    assert_eq!(out.len(), 2);
    assert_eq!(op_of(&out[0])["row"]["tool_call_id"], "c9");
}

#[test]
fn a_refusal_remembers_nothing() {
    // A row without an answer coming would shift every following correlation
    // by one — the fault that is worse than the empty id it would heal.
    let decls = one_declaration();
    // The honest digest, with one character moved: close enough to look right
    // and wrong enough to be refused.
    let sha = digest_of(&decls);
    let near_miss = format!(
        "{}{}",
        &sha[..sha.len() - 1],
        if sha.ends_with('a') { 'b' } else { 'a' }
    );
    // Since GH #435 only the checks that survived the move refuse HERE. The two
    // permission refusals are the broker's answer now and land one phase later,
    // where they delete the parked row rather than never writing one —
    // measured in `gh435_the_submitter_asks_the_broker`.
    for (claimed, code) in [
        ("deadbeef", "manifest_digest_mismatch"),
        (near_miss.as_str(), "manifest_digest_mismatch"),
        ("", "manifest_digest_mismatch"),
    ] {
        let out = submit(&decls, claimed, REQUESTER);
        assert_eq!(out.len(), 1, "{code}: a refusal is one message");
        assert_eq!(out[0]["header"]["route"], "receipt");
        assert_eq!(out[0]["header"]["error_code"], code);
    }
}

#[test]
fn a_submission_without_a_requester_remembers_nothing() {
    let decls = one_declaration();
    let sha = digest_of(&decls);
    let out = submit(&decls, &sha, "");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["error_code"], "requester_unknown");
}

/// Phase B: the colony's answer, in the fresh trace `emit_reply_or_done` built
/// — no `reply_to`, no context, no headers.
fn colony_answer(receipt: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": {}, "context": {} },
            "ttl": 64,
            "manifest": receipt,
            "messages": [],
            "params": {}
        }),
    )
}

#[test]
fn phase_b_asks_the_store_before_it_renders() {
    let out = colony_answer(json!({ "outcome": "committed", "applied": 2,
                                    "ids": ["m1", "m2"] }));

    assert_eq!(out.len(), 1);
    let h = &out[0]["header"];
    assert_eq!(h["route"], "sstore");
    assert_eq!(h["phase"], "pop");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "submissions");
    // `columns` is mandatory for a store select (docs/cell-types.md § store),
    // and `limit` without `order_by` returns an UNSPECIFIED row.
    assert_eq!(
        op["columns"],
        json!(["id", "tool_call_id", "manifest_sha256"])
    );
    assert_eq!(op["where"], json!({ "kind": "flight" }));
    assert_eq!(
        op["order_by"],
        json!([{ "col": "at", "dir": "asc" }, { "col": "id", "dir": "asc" }])
    );
    assert_eq!(op["limit"], 1);

    // the receipt facts ride in the carry -- the /colony reply carries no
    // context of its own, so this is the only place they can wait.
    let carry: Value =
        meclaw_core::serde_json::from_str(h["carry"].as_str().expect("carry")).expect("json");
    assert_eq!(carry["outcome"], "committed");
    assert_eq!(carry["applied"], 2);
}

/// The store's answer to the FIFO `select`, in the shape the hive's own edge
/// hands it back: the marker in context, the rows as the text of one turn.
fn pop(rows: Value, carry: &str, error_code: Option<&str>) -> Vec<Value> {
    let mut hop = json!({ "operation": "select", "rows_affected": 1 });
    if let Some(code) = error_code {
        hop["error_code"] = json!(code);
    }
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": hop,
                "context": { "sub_origin": "gate", "sub_phase": "pop",
                             "sub_carry": carry }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

const COMMITTED: &str = r#"{"outcome":"committed","applied":2,"ids":["m1","m2"]}"#;

#[test]
fn the_pop_stamps_the_receipt_and_forgets_the_row() {
    let rows = json!([{ "id": "r1", "tool_call_id": "c2",
                        "manifest_sha256": "abc123" }]);
    let out = pop(rows, COMMITTED, None);

    assert_eq!(out.len(), 2, "forget the row, then answer");

    let del = op_of(&out[0]);
    assert_eq!(del["operation"], "delete");
    assert_eq!(del["where"], json!({ "id": "r1" }));
    assert_eq!(out[0]["header"]["phase"], "written");

    let h = &out[1]["header"];
    assert_eq!(h["route"], "receipt");
    assert_eq!(h["applied"], 2);
    assert_eq!(h["mutation_id"], "m2");
    // GH #438, the whole point:
    assert_eq!(h["tool_call_id"], "c2");
    assert_eq!(h["manifest_sha256"], "abc123");
    assert_eq!(out[1]["messages"][0]["id"], "c2");
    assert!(
        out[1]["messages"][0]["text"]
            .as_str()
            .expect("a turn")
            .contains("manifest applied")
    );
}

#[test]
fn a_rejected_manifest_is_correlated_too() {
    // `rejected` keeps error_code, failed_at and remaining verbatim from the
    // colony, PLUS the id. A refusal a fan-in cannot close on is still a
    // refusal nobody hears.
    let rows = json!([{ "id": "r7", "tool_call_id": "c5",
                        "manifest_sha256": "beef01" }]);
    let carry = r#"{"outcome":"rejected","applied":2,"ids":["m1","m2"],
                    "failed_at":3,"remaining":2,"error_code":"scope_containment"}"#;
    let out = pop(rows, carry, None);

    assert_eq!(out.len(), 2);
    let h = &out[1]["header"];
    assert_eq!(h["route"], "receipt");
    assert_eq!(h["error_code"], "scope_containment");
    assert_eq!(h["failed_at"], 3);
    assert_eq!(h["remaining"], 2);
    assert_eq!(h["applied"], 2);
    assert_eq!(h["tool_call_id"], "c5");
    assert_eq!(h["manifest_sha256"], "beef01");
    assert_eq!(out[1]["messages"][0]["id"], "c5");
}

#[test]
fn no_row_still_answers() {
    // A cell restart, an `--apply` mutation from outside, a row somebody swept:
    // the correlation is gone, the FACT is not. A lost receipt is the one exit
    // there is no recovery from — it is why this hive declares a required drain.
    let out = pop(json!([]), COMMITTED, None);
    assert_eq!(out.len(), 1, "nothing to forget, but something to say");
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["header"]["tool_call_id"], "");
    assert_eq!(out[0]["messages"][0]["id"], "");
    assert!(
        out[0]["messages"][0]["text"]
            .as_str()
            .expect("a turn")
            .contains("manifest applied")
    );
}

#[test]
fn a_store_error_still_answers() {
    // `hop.error_code` on the store's answer leads into the same branch: the
    // colony already applied the manifest, and a swallowed answer would make a
    // committed change look like a lost one.
    let rows = json!([{ "id": "r1", "tool_call_id": "c2",
                        "manifest_sha256": "abc123" }]);
    let out = pop(rows, COMMITTED, Some("sql_error"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(
        out[0]["header"]["tool_call_id"], "",
        "a row that could not be read is a row that is not there"
    );
}

#[test]
fn the_hive_still_has_no_ports() {
    // The hive path stays the only address. A store reachable from outside
    // would be the audit trail with a second door.
    assert_eq!(
        read(HIVE)["params"]["ports"]
            .as_array()
            .expect("ports")
            .len(),
        0
    );
}
