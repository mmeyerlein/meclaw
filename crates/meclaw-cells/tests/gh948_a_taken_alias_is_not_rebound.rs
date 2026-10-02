//! GH #948 -- a taken alias is not rebound.
//!
//! `set_alias` is an upsert without a check: written twice, the second id wins
//! and every fact of that spelling moves with it. A caller binding a spelling
//! that already means another id must learn that instead of moving the facts
//! of someone else's thing -- `alias_taken`, nothing written -- and only an
//! explicit `force` overwrites. Binding the same id again is no conflict.
//!
//! The answer never names the previous target. A spelling the night bound to a
//! subject it derived from conversations points at a string that is content of
//! rounds the caller was never part of -- whatever shape that string has, even
//! the shape of an id -- and the alias table carries no audience to ask.
//!
//! Checking and writing are two store round trips, so the check alone holds
//! only while requests arrive one at a time. The write itself is therefore a
//! plain `insert` on the table's primary key, and a `select` closes the same
//! store bundle: two requests in flight for one spelling cannot both win.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh948_a_taken_alias_is_not_rebound() {
    if !shipped() {
        eprintln!("gh948: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        &[
            fact(
                "f-1",
                "Firewall X",
                "firewall x",
                ROUND_EA,
                "2026-09-01T10:00:00Z",
            ),
            fact(
                "f-2",
                "the box",
                "the box",
                ROUND_EB,
                "2026-09-02T10:00:00Z",
            ),
        ],
    );
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();
    let db = db(&td);
    let bind = |tag: &str, alias: &str, canonical: &str, force: bool| {
        in_alias(
            json!({"aliases": [{"alias": alias, "canonical": canonical}], "force": force}),
            tag,
        )
    };

    h.send(bind("t-1", "Firewall X", "ob-1", false)).await;
    let a = ack(&mut rx, &mut seen, "t-1").await;
    assert_eq!(body_of(&a)["done"], json!(1));
    assert_eq!(canonical_subjects(&db)["f-1"], "ob-1");

    // A second id for the same spelling: refused, nothing moves.
    h.send(bind("t-2", "Firewall X", "ob-2", false)).await;
    let a = ack(&mut rx, &mut seen, "t-2").await;
    assert_eq!(hop_str(&a, "error_code"), "");
    assert_eq!(body_of(&a)["done"], json!(0));
    assert_eq!(
        body_of(&a)["refused"],
        json!([{"alias": "Firewall X", "error_code": "alias_taken"}])
    );
    assert_eq!(canonical_subjects(&db)["f-1"], "ob-1");
    assert_eq!(
        rows(
            &db,
            "SELECT canonical FROM subject_aliases WHERE alias = 'firewall x'"
        ),
        vec![vec!["ob-1".to_string()]]
    );

    // The same id again is no conflict.
    h.send(bind("t-3", "Firewall X", "ob-1", false)).await;
    let a = ack(&mut rx, &mut seen, "t-3").await;
    assert_eq!(body_of(&a)["done"], json!(1));
    assert_eq!(body_of(&a)["refused"], json!([]));

    // `force` overwrites, and the facts follow.
    h.send(bind("t-4", "Firewall X", "ob-2", true)).await;
    let a = ack(&mut rx, &mut seen, "t-4").await;
    assert_eq!(body_of(&a)["done"], json!(1));
    assert_eq!(body_of(&a)["refused"], json!([]));
    assert_eq!(canonical_subjects(&db)["f-1"], "ob-2");

    // A binding the night made, to a subject it derived from conversations:
    // still refused, and the answer does not name that subject.
    let night = store_op(
        &db,
        json!({"operation": "set_alias", "table": "facts", "column": "subject",
               "alias": "The Box", "canonical": "a box in the hall"}),
    );
    assert_eq!(night.error_code, None);
    h.send(bind("t-5", "the box", "ob-3", false)).await;
    let a = ack(&mut rx, &mut seen, "t-5").await;
    assert_eq!(
        body_of(&a)["refused"],
        json!([{"alias": "the box", "error_code": "alias_taken"}]),
        "a previous target is never named"
    );

    // Two entries of ONE request that claim one spelling for two ids: the
    // first wins, the second is the same conflict as a stored one.
    h.send(in_alias(
        json!({"aliases": [{"alias": "Lamp", "canonical": "ob-4"},
                           {"alias": "lamp", "canonical": "ob-5"}]}),
        "t-6",
    ))
    .await;
    let a = ack(&mut rx, &mut seen, "t-6").await;
    assert_eq!(body_of(&a)["done"], json!(1));
    assert_eq!(
        body_of(&a)["refused"],
        json!([{"alias": "lamp", "error_code": "alias_taken"}])
    );
    assert_eq!(
        rows(
            &db,
            "SELECT canonical FROM subject_aliases WHERE alias = 'lamp'"
        ),
        vec![vec!["ob-4".to_string()]]
    );
    h.shutdown().await;
}

/// The judgement on its own, over the table as the store answers it.
#[test]
fn gh948_the_judgement_reads_the_table_in_request_order() {
    if !shipped() {
        return;
    }
    let program = r#"
held = {"firewall x": "ob-1", "the box": "a box in the hall", "jean-paul": "jean-paul",
        "ob/gyn": "ob-gyn", "fw x": "ob-0123456789ab"}
def run(entries, force):
    es, f, bad = parse_aliases({"aliases": entries, "force": force})
    assert bad is None, bad
    binds, refused = judge_aliases(es, f, held)
    return [[b["alias"] for b in binds], refused]
print(json.dumps([
  run([{"alias": "Firewall X", "canonical": "ob-2"}], False),
  run([{"alias": "Firewall X", "canonical": "ob-1"}], False),
  run([{"alias": "Firewall X", "canonical": "ob-2"}], True),
  run([{"alias": "the box", "canonical": "ob-3"}], False),
  run([{"alias": "Jean-Paul", "canonical": "ob-3"}], False),
  run([{"alias": "Lamp", "canonical": "ob-4"}, {"alias": "LAMP", "canonical": "ob-5"}], False),
  run([{"alias": "Lamp", "canonical": "ob-4"}, {"alias": "LAMP", "canonical": "ob-5"}], True),
  run([{"alias": "new", "canonical": "ob-9"}], False),
  run([{"alias": "OB/GYN", "canonical": "ob-1a2b3c4d5e6f"}], False),
  run([{"alias": "FW X", "canonical": "ob-1a2b3c4d5e6f"}], False),
], sort_keys=True))
"#;
    let out = probe(WRITER, program);
    let mut lines = out.lines();
    assert_eq!(
        lines.next().expect("judgements"),
        concat!(
            r#"[[[], [{"alias": "Firewall X", "error_code": "alias_taken"}]], "#,
            r#"[["Firewall X"], []], "#,
            r#"[["Firewall X"], []], "#,
            r#"[[], [{"alias": "the box", "error_code": "alias_taken"}]], "#,
            r#"[[], [{"alias": "Jean-Paul", "error_code": "alias_taken"}]], "#,
            r#"[["Lamp"], [{"alias": "LAMP", "error_code": "alias_taken"}]], "#,
            r#"[["Lamp", "LAMP"], []], "#,
            r#"[["new"], []], "#,
            // A night subject in the caller's own prefix, and one in the very
            // shape of a source id: neither is named (GH #948 review I-2).
            r#"[[], [{"alias": "OB/GYN", "error_code": "alias_taken"}]], "#,
            r#"[[], [{"alias": "FW X", "error_code": "alias_taken"}]]]"#
        )
    );
}

/// Two bindings of one spelling in flight at once (GH #948 review I-1): both
/// may read the empty table before either writes. The write is an `insert` on
/// the primary key of the alias table and the store runs one bundle without a
/// foreign step in between, so exactly one of them binds and the other learns
/// `alias_taken` -- whichever arrives first. No time window is assumed: the
/// invariant holds whether the two reads interleave or not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh948_two_bindings_in_flight_never_overwrite_each_other() {
    if !shipped() {
        eprintln!("gh948: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        &[fact(
            "f-shed",
            "Garden Shed",
            "garden shed",
            ROUND_EA,
            "2026-09-01T10:00:00Z",
        )],
    );
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();
    let db = db(&td);

    // Sent back to back, neither waits for the other's answer.
    h.send(in_alias(
        json!({"aliases": [{"alias": "Garden Shed", "canonical": "ob-6"}]}),
        "r-1",
    ))
    .await;
    h.send(in_alias(
        json!({"aliases": [{"alias": "garden  shed", "canonical": "ob-7"}]}),
        "r-2",
    ))
    .await;
    let first = ack(&mut rx, &mut seen, "r-1").await;
    let second = ack(&mut rx, &mut seen, "r-2").await;
    for a in [&first, &second] {
        assert_eq!(hop_str(a, "error_code"), "", "{:?}", body_of(a));
    }
    let done = |a: &meclaw_core::Message| body_of(a)["done"].as_i64().expect("done");
    assert_eq!(
        done(&first) + done(&second),
        1,
        "exactly one binding wins: {:?} / {:?}",
        body_of(&first),
        body_of(&second)
    );
    let (winner, loser, alias) = if done(&first) == 1 {
        ("ob-6", &second, "garden  shed")
    } else {
        ("ob-7", &first, "Garden Shed")
    };
    assert_eq!(
        body_of(loser)["refused"],
        json!([{"alias": alias, "error_code": "alias_taken"}])
    );
    assert_eq!(
        rows(
            &db,
            "SELECT canonical FROM subject_aliases WHERE alias = 'garden shed'"
        ),
        vec![vec![winner.to_string()]]
    );
    assert_eq!(canonical_subjects(&db)["f-shed"], winner);
    h.shutdown().await;
}

/// The write as the writer hands it to the store, and the store's verdict as
/// the writer reads it back (GH #948 review I-1): without `force` an unbound
/// spelling is an `insert` on the table's key, a spelling bound to the same id
/// writes nothing, `force` is the upsert, and a `select` of the keys closes the
/// bundle. An insert the key refused is `alias_taken` when another id stands
/// there now, and bound when the same id does.
#[test]
fn gh948_the_write_is_an_insert_the_closing_select_settles() {
    if !shipped() {
        return;
    }
    let script = meclaw_testing::shipped_script(WRITER);
    let run = |ctx: Value, hop: Value, body: Value| {
        let mut flat = as_map(&body);
        flat.insert("header".into(), json!({"context": ctx, "hop": hop}));
        let out = meclaw_testing::emit_all(&script, &Value::Object(flat));
        assert_the_declaration_admits(WRITER, &out);
        out
    };
    // One request through the read: the table holds `held` for its keys.
    let to_write = |aliases: Value, force: bool, held: Value| {
        let out = run(
            json!({"mem_phase": "alias", "store_origin": "alias"}),
            json!({"route": "in_alias", "alias_tag": "t"}),
            json!({"aliases": aliases, "force": force, "messages": []}),
        );
        let req = out[0]["header"]["alias_req"].clone();
        let out = run(
            json!({"mem_phase": "alias-read", "store_origin": "alias", "alias_req": req}),
            json!({"operation": "select", "rows_affected": 0}),
            json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "al-read",
                                 "text": held.to_string()}]}),
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0]["header"]["route"], "alstore");
        let calls: Vec<Value> = out[0]["messages"]
            .as_array()
            .expect("calls")
            .iter()
            .map(|m| {
                let mut c: Value =
                    meclaw_core::serde_json::from_str(m["text"].as_str().expect("text"))
                        .expect("call json");
                c["id"] = m["id"].clone();
                c
            })
            .collect();
        (out[0]["header"]["alias_req"].clone(), calls)
    };
    let ops = |calls: &[Value]| -> Vec<String> {
        calls
            .iter()
            .map(|c| c["operation"].as_str().expect("op").to_string())
            .collect()
    };
    // The store's bundle answer: `refuse` names the legs the key refused,
    // `table` is what the closing select reads.
    let settle = |req: &Value, calls: &[Value], refuse: &[&str], table: Value| {
        let mut messages = Vec::new();
        let mut results = Vec::new();
        for c in calls {
            let id = c["id"].as_str().expect("id");
            let op = c["operation"].as_str().expect("op");
            let mut r = json!({"tool_call_id": id, "operation": op, "rows_affected": 1,
                               "duration_ms": 0});
            let mut text = "null".to_string();
            if refuse.contains(&id) {
                r["rows_affected"] = json!(0);
                r["error_code"] = json!("constraint_violation");
                text = "UNIQUE constraint failed: subject_aliases.alias".into();
            } else if op == "select" {
                text = table.to_string();
            }
            messages.push(json!({"origin": "tool", "type": "tool_result", "id": id,
                                 "text": text}));
            results.push(r);
        }
        let out = run(
            json!({"mem_phase": "alias-write", "store_origin": "alias", "alias_req": req}),
            json!({"operation": "bundle", "rows_affected": 1, "bundle_errors": refuse.len()}),
            json!({"messages": messages, "results": results}),
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0]["header"]["route"], "alias_ack");
        assert_eq!(out[0]["header"]["alias_tag"], "t");
        out[0].clone()
    };

    // An unbound spelling: an insert on the key, then canonicalize, then the
    // closing select of the keys.
    let (req, calls) = to_write(
        json!([{"alias": "Firewall X", "canonical": "ob-2"}]),
        false,
        json!([]),
    );
    assert_eq!(ops(&calls), ["insert", "canonicalize", "select"]);
    assert_eq!(calls[0]["table"], "subject_aliases");
    assert_eq!(calls[0]["row"]["alias"], "firewall x");
    assert_eq!(calls[0]["row"]["canonical"], "ob-2");
    assert_eq!(calls[2]["table"], "subject_aliases");
    assert_eq!(calls[2]["where"], json!({"alias": {"in": ["firewall x"]}}));
    let ins = calls[0]["id"].as_str().expect("id").to_string();

    // The insert landed.
    let a = settle(
        &req,
        &calls,
        &[],
        json!([{"alias": "firewall x", "canonical": "ob-2"}]),
    );
    assert_eq!(a["done"], json!(1));
    assert_eq!(a["refused"], json!([]));
    // Another request bound the spelling between this read and this write.
    let a = settle(
        &req,
        &calls,
        &[ins.as_str()],
        json!([{"alias": "firewall x", "canonical": "ob-1"}]),
    );
    assert_eq!(a["header"]["error_code"], "");
    assert_eq!(a["done"], json!(0));
    assert_eq!(
        a["refused"],
        json!([{"alias": "Firewall X", "error_code": "alias_taken"}])
    );
    // ... to the same id: bound all the same.
    let a = settle(
        &req,
        &calls,
        &[ins.as_str()],
        json!([{"alias": "firewall x", "canonical": "ob-2"}]),
    );
    assert_eq!(a["done"], json!(1));
    assert_eq!(a["refused"], json!([]));

    // The same id again writes nothing and is bound while the table says so.
    let (req, calls) = to_write(
        json!([{"alias": "Firewall X", "canonical": "ob-1"}]),
        false,
        json!([{"alias": "firewall x", "canonical": "ob-1"}]),
    );
    assert_eq!(ops(&calls), ["canonicalize", "select"]);
    let a = settle(
        &req,
        &calls,
        &[],
        json!([{"alias": "firewall x", "canonical": "ob-1"}]),
    );
    assert_eq!(a["done"], json!(1));

    // `force` is the upsert, and its own leg is its verdict.
    let (req, calls) = to_write(
        json!([{"alias": "Firewall X", "canonical": "ob-2"}]),
        true,
        json!([{"alias": "firewall x", "canonical": "ob-1"}]),
    );
    assert_eq!(ops(&calls), ["set_alias", "canonicalize", "select"]);
    let a = settle(
        &req,
        &calls,
        &[],
        json!([{"alias": "firewall x", "canonical": "ob-2"}]),
    );
    assert_eq!(a["done"], json!(1));
}
