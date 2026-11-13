//! GH #1463 (D58 form) -- `recall_in_wall` off is byte-identical.
//!
//! WHY this file exists: the form changes how a recall block is decided, and
//! it changes that only for a hive that turned the knob on. Shipped "0", the
//! window is the window of before: every recall pair stays on every call,
//! no call says a `recall_in_wall` cut, the snapshot lists no element left
//! out, the plan keeps the block, and the rows of a recall result hash as
//! they did (the garbage collection's `dup_recall` reads them with `gc` on,
//! whatever this knob says).
//!
//! Three arms of the same run. `plain` sets no knob, so it runs the shipped
//! default -- since the form was built that is the knob at "0", no longer a
//! tree that knows no such knob (this pin was written red before the build,
//! when it was). `off` sets "0" by hand: the byte-for-byte comparison with
//! `plain` (calls, cuts, `sent`, plan, row counts, ledger operations) holds
//! that the explicit value and the default are one and the same, not that
//! the window equals the one of a tree before the form. That "before" is
//! held by what the window of `off` shows: the pair on every call, no cut of
//! this kind on the hop or in the snapshot, no element left out, the plan
//! keeps the recall block. The arm `on` is the control: without it every
//! statement on `off` would pass on a tree where the knob does nothing.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_000000000formoff";
const SAID: &str = "My sister Anna moved to Hamburg.";
const WHY: &str = "recall_in_wall";

/// Four turns: [`SAID`], a recall pair that quotes it, and two more calls.
/// Returns the hive and the calls of `t2`, `t3` and `t4`.
fn run(over: &[(&str, &str, Value)]) -> (Hive, Vec<Msg>) {
    let mut all = vec![("policy", "snapshot_window", json!("1"))];
    all.extend(over.iter().cloned());
    let mut h = Hive::with(&all);
    turn(&mut h, "s1", "t1", SAID, "Hamburg is lovely.", json!({}));
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("She likes it there.")]));
    let c3 = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    h.tap(&c3, "stop", json!({}), json!([said("She moved in March.")]));
    let c4 = h.curate("s1", "t4", 0, json!([user("Go on.")]), mode("Be brief."));
    (h, vec![c2, c3, c4])
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

fn cuts(c: &Msg) -> Value {
    c.hop.get("cuts").cloned().unwrap_or(Value::Null)
}

/// The `cut` column of the call's snapshot row.
fn snapshot_cut(h: &Hive, c: &Msg) -> String {
    let id = c.hop["curator_call"].as_str().unwrap_or_default();
    h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r.first().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default()
}

/// The kinds of the plan's blocks, in their order.
fn plan_kinds(h: &Hive) -> Vec<String> {
    plan_blocks(h, ROUND_E)
        .iter()
        .map(|b| b["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn off_is_the_window_of_before() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(policy["params"]["recall_in_wall"], json!("0"), "ships off");
    assert_eq!(
        policy["contract"]["settings"]["recall_in_wall"]["default"],
        json!("0"),
        "the setting says the same"
    );

    let (plain, p) = run(&[]);
    let (off, o) = run(&[("policy", "recall_in_wall", json!("0"))]);
    let (on, n) = run(&[("policy", "recall_in_wall", json!("1"))]);

    for (i, name) in ["t2", "t3", "t4"].iter().enumerate() {
        // Off set by hand is the shipped default, byte for byte.
        assert_eq!(o[i].body, p[i].body, "the call of {name}");
        assert_eq!(cuts(&o[i]), cuts(&p[i]), "the cuts of {name}");
        assert_eq!(
            sent_of(&off, &o[i]),
            sent_of(&plain, &p[i]),
            "the list `sent` of {name}"
        );
        // And it is the window of before: the pair on every call, nothing
        // left out, nothing said about it.
        let t = text(&o[i]);
        assert!(t.contains(RECALL_ID), "{name}: the pair stays: {t}");
        assert_eq!(
            t.matches(SAID).count(),
            2,
            "{name}: the turn and the quote: {t}"
        );
        assert!(
            !cuts(&o[i]).to_string().contains(WHY),
            "{name}: no cut of this kind on the hop: {}",
            cuts(&o[i])
        );
        assert!(
            !snapshot_cut(&off, &o[i]).contains(WHY),
            "{name}: none in the snapshot: {}",
            snapshot_cut(&off, &o[i])
        );
        let sent = sent_of(&off, &o[i]).expect("the snapshot row carries `sent`");
        let w = sent_window(&off, &sent);
        assert!(
            w.left_out.is_empty(),
            "{name}: off, the list names no element left out: {sent:?}"
        );
        assert_eq!(
            w.messages,
            o[i].messages(),
            "{name}: the list is the window"
        );
    }
    assert_eq!(plan_kinds(&off), plan_kinds(&plain), "the plan of before");
    assert!(
        plan_kinds(&off).iter().any(|k| k == "recall"),
        "off, the plan keeps the recall block: {:?}",
        plan_kinds(&off)
    );
    for table in ["blocks", "snapshots", "call_blocks", "wall", "marks"] {
        let count = |h: &Hive| h.rows(&format!("SELECT count(*) FROM {table}"))[0][0].as_i64();
        assert_eq!(count(&off), count(&plain), "{table}");
    }
    assert_eq!(
        off.ledger_ops.len(),
        plain.ledger_ops.len(),
        "off runs no ledger operation more"
    );

    // The control: on, the same run leaves the pair out after its round.
    assert!(text(&n[0]).contains(RECALL_ID), "on, the running round");
    assert!(
        !text(&n[1]).contains(RECALL_ID),
        "with `recall_in_wall` \"1\" the pair leaves the call of t3: {}",
        text(&n[1])
    );
    assert_ne!(n[1].body, o[1].body, "on and off differ");
    drop(on);
}

/// The rows of a recall result as the script of before hashed them: every
/// line at an indent of exactly two that is no parenthesised caveat, each
/// stripped, the first 16 hex of its sha256.
fn rows_of_before(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let third = line.chars().nth(2);
            let body = line.trim_end();
            line.starts_with("  ")
                && !matches!(third, None | Some(' ') | Some('\t'))
                && !(body[2..].starts_with('(') && body.ends_with(')'))
        })
        .map(|line| sha256_hex(line.trim())[..16].to_string())
        .collect()
}

/// Lock: off, the rows of a recall result hash as before -- also for a
/// quote over several lines, whose head row carries the opening mark only.
/// The class `dup_recall` of the garbage collection reads these hashes with
/// `gc` on, so a change here would change a rebuild of a hive that never
/// turned `recall_in_wall` on.
#[test]
fn off_the_rows_of_a_recall_result_hash_as_before() {
    if !shipped() {
        return;
    }
    let result = "WHAT WAS SAID\n  (a caveat in parentheses)\n  user on 2026-11-08: \"Where could we go?\"\n  assistant on 2026-11-08: \"A week at the Baltic Sea.\n\n- Usedom or Ruegen.\n\nShall I look up the trains?\" (seen: 2)";
    let want = rows_of_before(result);
    assert_eq!(want.len(), 2, "check of the setup: two rows: {want:?}");
    for over in [json!({}), json!({"recall_in_wall": "0"})] {
        let (got, err) = policy_scope(
            over.clone(),
            "row_lines(ARGS['text'])",
            json!({"text": result}),
        );
        assert!(!err.contains("Traceback"), "{err}");
        assert_eq!(got, json!(want), "with {over}");
    }
}
