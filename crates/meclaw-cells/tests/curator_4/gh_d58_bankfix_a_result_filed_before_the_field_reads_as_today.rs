//! GH #1468 (D58 bankfix, F2) -- a result filed before the field reads as
//! today.
//!
//! WHY this file exists: since GH #1468 `./intake` writes its verdict on a
//! tool result beside the wall row (`wall.failed`) and the rebuild reads it.
//! A wall that was filed before knows no verdict: its rows hold NULL there
//! (the column is declared without a default, so growing the table backfills
//! nothing). For such a row the rebuild judges as it always did, by the text
//! alone (`is_error_result`): an error object stays whole, and a failure in
//! prose -- whose header code nobody kept -- shrinks like a fine result. The
//! second half is no promise worth keeping, it is the line the field draws:
//! no verdict is made up for a row after the fact.
//!
//! The old rows are written the way a ledger from before holds them (a row
//! without the column); the rebuild runs on the real way.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn prose() -> String {
    format!(
        "The service did not answer in time, so nothing was fetched. {}",
        "It may be back later today. ".repeat(24)
    )
}

fn fine(i: usize) -> String {
    format!("result {i}: {}", "m".repeat(700))
}

fn error_object() -> String {
    json!({"error": "timeout", "detail": "e".repeat(700)}).to_string()
}

fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
}

/// A tool round of before: its four rows straight in the ledger, none with a
/// verdict. Returns the short id of the result.
fn old_round(h: &mut Hive, i: i64, text: &str) -> String {
    let base = backdated(LOOKBACK_S) + chrono::Duration::seconds(10 * i);
    let (turn, id) = (format!("r{i}"), format!("c{i}"));
    h.row(
        base,
        "s",
        &turn,
        "user",
        &user(&format!("look up {turn}")),
        0,
    );
    h.row(
        base + chrono::Duration::seconds(1),
        "s",
        &turn,
        "tool_call",
        &tool_call(&id, "lookup"),
        0,
    );
    let el = tool_result(&id, text);
    h.row(
        base + chrono::Duration::seconds(2),
        "s",
        &turn,
        "tool_result",
        &el,
        0,
    );
    h.row(
        base + chrono::Duration::seconds(3),
        "s",
        &turn,
        "assistant",
        &said(&format!("answered {turn}")),
        1,
    );
    short_id(&el)
}

#[test]
fn a_result_filed_before_the_field_reads_as_today() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for role in ["talky", "coding"] {
        let mut h = Hive::with(&[
            ("policy", "gc", json!("1")),
            ("policy", "keep_rounds", json!(1)),
            ("policy", "keep_recent", json!(40)),
            ("policy", "window_target", json!(2000)),
        ])
        .reporting(curator_profiles::profile(role));
        let prose_id = old_round(&mut h, 0, &prose());
        let error_id = old_round(&mut h, 1, &error_object());
        let fine_id = old_round(&mut h, 2, &fine(2));
        let kept_id = old_round(&mut h, 3, &fine(3));

        // A round of today crosses the target and orders the rebuild.
        let call = h.curate(
            "s",
            "r4",
            0,
            json!([user(&"x".repeat(12_000))]),
            mode("Be brief."),
        );
        h.out.clear();
        h.tap(&call, "stop", usage(3000), json!([said("a")]));
        let order = h
            .clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("{role}: the target orders a rebuild: {:?}", h.stderr));
        h.fire(&order);
        while !h.summ.is_empty() {
            h.answer("What came before, in short.", "stop");
        }

        let plan = h.plan_in(ROUND_E);
        let mut shrunk: Vec<String> = plan["shrunk"]
            .as_array()
            .unwrap_or_else(|| panic!("{role}: a plan with `shrunk`: {plan} {:?}", h.stderr))
            .iter()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .collect();
        shrunk.sort();
        let mut want = vec![prose_id.clone(), fine_id.clone()];
        want.sort();
        assert_eq!(
            shrunk, want,
            "{role}: judged by the text alone, as before -- the error object ({error_id}) and \
             the kept iteration ({kept_id}) stay, the prose shrinks: {plan}"
        );

        // The rows of before hold no verdict, and the rebuild wrote none.
        let verdicts = h
            .rows("SELECT failed FROM wall WHERE turn_id IN ('r0', 'r1', 'r2', 'r3') ORDER BY seq");
        assert_eq!(verdicts.len(), 16, "{role}: {verdicts:?}");
        assert!(
            verdicts.iter().all(|r| r[0].is_null()),
            "{role}: no verdict is made up after the fact: {verdicts:?}"
        );

        // The next call shows the error object whole.
        let call = h.curate("s", "r5", 0, json!([user("and now?")]), mode("Be brief."));
        let shown = texts(&call).join("\n");
        assert!(
            shown.contains(&error_object()),
            "{role}: the error object is shown as it is: {shown}"
        );
        assert!(
            !shown.contains(&prose()),
            "{role}: the prose of before takes the one-line form: {shown}"
        );
    }
}
