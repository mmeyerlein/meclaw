//! GH #1225 (K-5, A-K5) -- every row `./policy` keeps is written in ONE step.
//!
//! WHY this file exists: `put` wrote a state row as a `delete` and an
//! `insert`, and the window forms, the summary blocks, the identity snapshot
//! and the summary's audience the same way. A bundle is no transaction: a
//! reader between the two legs missed the row, and an insert that failed lost
//! it -- the round's plan, its summary binding, its last call. Now a state row
//! is one `insert` with `replace` on its key, a block one `insert` with
//! `if_absent` on its hash, a model pin one `insert` with `if_absent` on
//! (hash, source, audience) (form of `misc_2::gh1199_a_row_is_replaced_in_one_step`).
//! The behaviour: every insert of the class refused by a trigger, the whole
//! cycle of turns and a rebuild run -- no row that stood before is lost.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-k5";

fn collapsed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_policy_writes_its_rows_in_one_step() {
    if !shipped() {
        return;
    }
    let text = collapsed(&script_of("policy"));
    for gone in [
        r#"(cid + "-del", {"operation": "delete", "table": "state""#,
        r#""operation": "delete", "table": "blocks""#,
        r#"("i-sd", {"operation": "delete""#,
        r#"("i-md", {"operation": "delete""#,
        r#"("s-clear", {"operation": "delete""#,
        r#"("s-sdel", {"operation": "delete""#,
        r#"("s-unaud", {"operation": "delete""#,
    ] {
        assert!(
            !text.contains(&collapsed(gone)),
            "still a delete and an insert: {gone}"
        );
    }
    for one_step in [
        r#""row": {"key": key, "value": value}, "replace": {"key": key}}"#,
        r#""if_absent": {"hash": h}})"#,
        r#""if_absent": {"hash": h, "source": MODEL_SOURCE, "audience_set": row["audience_set"]"#,
        r#"block_put("s-b1", h, "summary""#,
        r#"block_put("s-b2", sh, "system""#,
        r#"block_put("i-bi", fh, IDENTITY_FULL_KIND"#,
        r#"block_put("y-bi", sh, "system""#,
        r#"put("s-aud", rkey("summary_audience")"#,
        r#"put("s-put", rkey("pending:summary")"#,
        r#"put("i-si", "identity_full:" + session"#,
    ] {
        assert!(
            text.contains(&collapsed(one_step)),
            "the one-step form is missing: {one_step}"
        );
    }
}

/// The `state` rows and block hashes the ledger holds.
fn snapshot(h: &Hive) -> (Vec<(String, String)>, Vec<String>) {
    let state = h
        .rows("SELECT key, value FROM state ORDER BY key")
        .into_iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or("").to_string(),
                r[1].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let blocks = h
        .rows("SELECT hash FROM blocks ORDER BY hash")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    (state, blocks)
}

#[test]
fn no_key_of_the_class_loses_its_row_when_its_insert_is_refused() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    let cold = json!({"cache_expires_at": "2099-01-01T00:00:00Z"});
    turn(
        &mut h,
        S,
        "t-1",
        "cwK5 1: the parcel left.",
        "cwK5 1 reply.",
        json!({}),
    );
    turn(
        &mut h,
        S,
        "t-2",
        "cwK5 2: the lease is signed.",
        "cwK5 2 reply.",
        cold.clone(),
    );
    let order = h.clock_order_for(ROUND_E).expect("a strike");
    h.fire(&order);
    h.answer("cwK5 summary: a parcel and a lease.", "stop");
    let plan = h.state_in("window_plan", ROUND_E);
    assert!(plan.contains("summary"), "a rebuild made a plan: {plan}");
    let (state, blocks) = snapshot(&h);

    // From here on the store refuses every insert of the class: every state
    // row this hive keeps (the parked turns of `./intake` pass, they are
    // another cell's) and every block.
    h.ledger_may_refuse = true;
    h.db.execute_batch(
        "CREATE TRIGGER k5_state BEFORE INSERT ON state \
           WHEN NEW.key NOT LIKE 'pending:%' OR NEW.key LIKE 'pending:summary%' \
           BEGIN SELECT RAISE(ABORT, 'refused by the test'); END; \
         CREATE TRIGGER k5_blocks BEFORE INSERT ON blocks \
           BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;",
    )
    .expect("the triggers");
    turn(
        &mut h,
        S,
        "t-3",
        "cwK5 3: the keys are back.",
        "cwK5 3 reply.",
        cold,
    );
    if let Some(order) = h.clock_order_for(ROUND_E) {
        h.fire(&order);
        if !h.summ.is_empty() {
            h.answer("cwK5 second summary.", "stop");
        }
    }

    let (state_after, blocks_after) = snapshot(&h);
    let keys_after: Vec<&String> = state_after.iter().map(|(k, _)| k).collect();
    // A projection of an earlier call and a backoff fall by design (the next
    // call takes the projection down, a plan ends the backoff); the parked
    // rows are the steps' own.
    let lost: Vec<&(String, String)> = state
        .iter()
        .filter(|(k, _)| {
            !k.starts_with("pending:")
                && !k.starts_with("window_est:")
                && !k.starts_with("rebuild_backoff:")
        })
        .filter(|(k, _)| !keys_after.contains(&k))
        .collect();
    assert!(lost.is_empty(), "rows lost to a refused insert: {lost:?}");
    for key in ["window_plan", "summary_audience"] {
        let full = format!("{key}:{}", round_key(ROUND_E));
        let before = state
            .iter()
            .find(|(k, _)| *k == full)
            .map(|(_, v)| v.clone());
        let after = state_after
            .iter()
            .find(|(k, _)| *k == full)
            .map(|(_, v)| v.clone());
        assert!(before.is_some(), "{full} stood before");
        assert_eq!(before, after, "{full} keeps its old value");
    }
    let gone: Vec<&String> = blocks
        .iter()
        .filter(|b| !blocks_after.contains(b))
        .collect();
    assert!(gone.is_empty(), "blocks lost to a refused insert: {gone:?}");
}

/// Two store answers in ONE child, as the `code` runner keeps it resident:
/// the window-form cache `_VIEWS` (a cache of the child's own writes, never
/// the truth) survives an answer that took the form, and is emptied by one
/// whose view write the store refused -- the next call puts every form again
/// instead of trusting a block that is not there (GH #1225, RAM class).
#[test]
fn a_refused_view_write_empties_the_form_cache() {
    if !shipped() {
        return;
    }
    const DRIVER: &str = r#"
import io, json, sys
job = json.load(sys.stdin)
g = {"__name__": "__main__", "_VIEWS": {"h-cached"}}
seen, out = [], sys.stdout
for doc in job["docs"]:
    sys.stdin, sys.stdout = io.StringIO(json.dumps(doc)), io.StringIO()
    try:
        exec(compile(job["script"], "cell", "exec"), g)
    except SystemExit:
        pass
    sys.stdout = out
    seen.append(sorted(g["_VIEWS"]))
out.write(json.dumps(seen))
"#;
    let mut params = cell_config("policy")["params"].clone();
    params.as_object_mut().unwrap().remove("script_inline");
    let doc = |code: &str| {
        json!({
            "envelope": {"header": {"context": {"cur_phase": "done", "cur_call": "c-1",
                                                "cur_reason": ""},
                                    "hop": {"operation": "bundle"}},
                         "target": "/x/curator/policy", "reply_to": ""},
            "body": {"messages": [{"origin": "tool", "type": "tool_result", "id": "v-i0",
                                   "text": "null"}],
                     "results": [{"tool_call_id": "v-i0", "error_code": code,
                                  "rows_affected": if code.is_empty() { 1 } else { 0 }}]},
            "params": params,
        })
    };
    let job = json!({"script": script_of("policy"), "docs": [doc(""), doc("store_error")]});
    let out = run_python(DRIVER, &job.to_string());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let seen: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("driver output");
    assert_eq!(
        seen,
        json!([["h-cached"], []]),
        "a taken view write keeps the cache, a refused one empties it: {err}"
    );
}
