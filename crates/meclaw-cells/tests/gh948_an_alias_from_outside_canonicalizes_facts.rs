//! GH #948 -- an alias from outside canonicalizes the facts.
//!
//! Another part of the colony (a hive that keeps rows about things) tells the
//! memory hive under which spellings a thing is talked about and which id it
//! carries there: `in_alias {aliases: [{alias, canonical}]}`. The facts stay
//! where they are and carry the id as their canonical subject -- the ones said
//! before the binding (one `canonicalize` re-derives the column) and the ones
//! said after it (the store derives the column on every write from the alias
//! table). Measured at the receiver: the answer on the hive's rim, the rows in
//! the real `cell.db`.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const AT: [&str; 3] = [
    "2026-09-01T10:00:00Z",
    "2026-09-02T10:00:00Z",
    "2026-09-03T10:00:00Z",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh948_an_alias_from_outside_canonicalizes_facts() {
    if !shipped() {
        eprintln!("gh948: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        &[
            fact("f-1", "Firewall X", "firewall x", ROUND_EA, AT[0]),
            fact("f-2", "firewall  x", "firewall x", ROUND_EB, AT[1]),
            fact("f-3", "Router", "router", ROUND_EA, AT[2]),
        ],
    );
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();

    h.send(in_alias(
        json!({"aliases": [{"alias": "Firewall X", "canonical": "ob-1a2b3c4d5e6f"}]}),
        "t-bind",
    ))
    .await;
    let answer = ack(&mut rx, &mut seen, "t-bind").await;
    assert_eq!(
        hop_str(&answer, "error_code"),
        "",
        "bound: {:?}",
        answer.headers.hop
    );
    assert_eq!(body_of(&answer)["done"], json!(1));
    assert_eq!(body_of(&answer)["refused"], json!([]));
    // The hive's own round-trip markers end at its rim (GH #494).
    for key in ["alias_req", "mem_phase", "store_origin"] {
        assert!(
            !answer.headers.context.contains_key(key),
            "{key} left the hive on the answer: {:?}",
            answer.headers.context
        );
    }

    // Both spellings already on record now answer to the id; the third
    // subject is untouched. The binding sits under the store's normal form.
    let db = db(&td);
    let held = canonical_subjects(&db);
    assert_eq!(held["f-1"], "ob-1a2b3c4d5e6f");
    assert_eq!(held["f-2"], "ob-1a2b3c4d5e6f");
    assert_eq!(held["f-3"], "router");
    assert_eq!(
        rows(&db, "SELECT alias, canonical FROM subject_aliases"),
        vec![vec![
            "firewall x".to_string(),
            "ob-1a2b3c4d5e6f".to_string()
        ]]
    );
    h.shutdown().await;

    // A fact said AFTER the binding, written the way the store cell writes
    // every row (its own dispatcher, its own bindings): it carries the id too.
    let mut later = fact("f-4", "FIREWALL X", "", ROUND_EA, "2026-09-04T10:00:00Z");
    later
        .as_object_mut()
        .expect("row")
        .remove("canonical_subject");
    let out = store_op(
        &db,
        json!({"operation": "insert", "table": "facts", "row": later}),
    );
    assert_eq!(out.error_code, None, "the insert is taken");
    let held = canonical_subjects(&db);
    assert_eq!(
        held["f-4"], "ob-1a2b3c4d5e6f",
        "a later fact carries the id"
    );
    let mut other = fact("f-5", "Router", "", ROUND_EA, "2026-09-05T10:00:00Z");
    other
        .as_object_mut()
        .expect("row")
        .remove("canonical_subject");
    store_op(
        &db,
        json!({"operation": "insert", "table": "facts", "row": other}),
    );
    assert_eq!(canonical_subjects(&db)["f-5"], "router");
}

/// The writer's three steps on injected store replies, each emission judged by
/// the writer's own `contract.emits` -- the unit half of the lane.
#[test]
fn gh948_every_step_of_the_lane_is_inside_the_writers_contract() {
    if !shipped() {
        return;
    }
    let script = meclaw_testing::shipped_script(WRITER);
    let door = meclaw_testing::emit_all(
        &script,
        &json!({"header": {"context": {"mem_phase": "alias", "store_origin": "alias"},
                           "hop": {"route": "in_alias", "alias_tag": "t"}},
                "aliases": [{"alias": "Firewall X", "canonical": "ob-1"}],
                "messages": []}),
    );
    assert_eq!(door.len(), 1);
    assert_eq!(door[0]["header"]["route"], "alstore");
    assert_eq!(door[0]["header"]["phase"], "alias-read");
    let read: Value =
        meclaw_core::serde_json::from_str(door[0]["messages"][0]["text"].as_str().expect("op"))
            .expect("op json");
    assert_eq!(read["operation"], "select");
    assert_eq!(read["table"], "subject_aliases");
    assert_eq!(read["where"], json!({"alias": {"in": ["firewall x"]}}));
    let req = door[0]["header"]["alias_req"].clone();

    let write = meclaw_testing::emit_all(
        &script,
        &json!({"header": {"context": {"mem_phase": "alias-read", "store_origin": "alias",
                                       "alias_req": req},
                           "hop": {"operation": "select", "rows_affected": 0}},
                "messages": [{"origin": "tool", "type": "tool_result", "id": "al-read",
                              "text": "[]"}]}),
    );
    assert_eq!(write.len(), 1);
    assert_eq!(write[0]["header"]["phase"], "alias-write");
    let ops: Vec<Value> = write[0]["messages"]
        .as_array()
        .expect("ops")
        .iter()
        .map(|m| meclaw_core::serde_json::from_str(m["text"].as_str().expect("op")).expect("json"))
        .collect();
    // One insert on the alias table's key (GH #948 review I-1: never the
    // upsert without `force`), then ONE canonicalize, then the select that
    // settles the spelling.
    assert_eq!(
        ops.len(),
        3,
        "one insert, ONE canonicalize, one select: {ops:?}"
    );
    assert_eq!(ops[0]["operation"], "insert");
    assert_eq!(ops[0]["table"], "subject_aliases");
    assert_eq!(ops[0]["row"]["alias"], "firewall x");
    assert_eq!(ops[0]["row"]["canonical"], "ob-1");
    assert_eq!(
        ops[1],
        json!({"operation": "canonicalize", "table": "facts", "column": "subject"})
    );
    assert_eq!(
        ops[2],
        json!({"operation": "select", "table": "subject_aliases",
               "columns": ["alias", "canonical"],
               "where": {"alias": {"in": ["firewall x"]}}})
    );
    let req = write[0]["header"]["alias_req"].clone();

    let done = meclaw_testing::emit_all(
        &script,
        &json!({"header": {"context": {"mem_phase": "alias-write", "store_origin": "alias",
                                       "alias_req": req},
                           "hop": {"operation": "bundle", "rows_affected": 3, "bundle_errors": 0}},
                "messages": [{"origin": "tool", "type": "tool_result", "id": "al-check",
                              "text": r#"[{"alias": "firewall x", "canonical": "ob-1"}]"#}],
                "results": [{"tool_call_id": "al-0", "operation": "insert", "rows_affected": 1},
                            {"tool_call_id": "al-canon", "operation": "canonicalize",
                             "rows_affected": 2},
                            {"tool_call_id": "al-check", "operation": "select",
                             "rows_affected": 0}]}),
    );
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["header"]["route"], "alias_ack");
    assert_eq!(done[0]["header"]["error_code"], "");
    assert_eq!(done[0]["header"]["alias_tag"], "t");
    assert_eq!(done[0]["done"], json!(1));
    assert_eq!(done[0]["refused"], json!([]));

    let mut all = door;
    all.extend(write);
    all.extend(done);
    assert_the_declaration_admits(WRITER, &all);
}
