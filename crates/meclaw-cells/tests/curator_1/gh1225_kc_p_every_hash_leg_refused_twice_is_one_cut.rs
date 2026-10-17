//! GH #1225 (A3, policy, delta check of KR) -- every hash read of
//! `win-hashes` refused twice is one cut and one `store_error` mark.
//!
//! WHY this file exists: review I-1 of KR-P2 found a hash read refused twice
//! read as empty -- the parked turn (`b-slimq`) as no plan, the kept rows
//! (`b-keeph`), the pins (`b-pins`) and the newest cover (`b-cover`) silently
//! out of the window. The fix maps each leg to the part it was read for
//! (`B_CUT_OF`), but only `b-wallh` and `b-slotsh` were pinned: a leg dropped
//! from that map would read as empty again with every test green. One test
//! per leg now, and the one case where a refused leg cuts nothing -- a cover
//! read under a plan that holds its own cover.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-kcp";
const WORDS: &str = "My daughter Anna moves to Hamburg in May.";

fn cuts_of(call: &Msg, what: &str) -> Vec<Value> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["what"] == what)
        .collect()
}

fn marks(h: &Hive, kind: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
    .collect()
}

fn first_turn(h: &mut Hive) {
    turn(h, S, "r1", "I grew up in Leipzig.", "Nice.", json!({}));
}

/// The plan of the round written as `plan` (`None`: the ledger holds none).
fn set_plan(h: &Hive, plan: Option<Value>) {
    let key = format!("window_plan:{}", round_key(ROUND_E));
    h.db.execute("DELETE FROM state WHERE key = ?1", [&key])
        .expect("plan dropped");
    if let Some(p) = plan {
        h.db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            [&key, &p.to_string()],
        )
        .expect("plan written");
    }
}

fn seqs(h: &Hive) -> Vec<i64> {
    h.rows(&format!(
        "SELECT seq FROM wall WHERE session_id = '{S}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_i64().unwrap_or(0))
    .collect()
}

/// Step B refused whole, and the read again refuses `leg`. `plan`: the plan
/// step B parked with the turn stands although the bundle was refused -- its
/// write of the parked turn landed, its reads did not (a whole refusal takes
/// the plan with it, and the read again would then hold no kept rows and no
/// cover of a plan to cut).
fn refused_twice(h: &mut Hive, leg: &str, plan: Option<&Value>) -> Msg {
    h.ledger_may_refuse = true;
    if let Some(p) = plan {
        h.sql_at_phase = Some((
            "win-hashes".to_string(),
            format!(
                "UPDATE state SET value = json_set(value, '$.plan', '{p}') \
                 WHERE key LIKE 'pending:%' AND json_extract(value, '$.lane') = 'in_curate'"
            ),
        ));
    }
    h.refuse_phase = Some("win-hashes".to_string());
    h.refuse_legs = vec![(leg.to_string(), "store_error".to_string())];
    let call = h.curate(S, "r2", 0, json!([user(WORDS)]), mode("Be brief."));
    assert!(h.refuse_phase.is_none(), "{leg}: step B was refused");
    assert!(h.sql_at_phase.is_none(), "{leg}: the plan stood");
    assert!(
        h.refuse_legs.is_empty(),
        "{leg}: the read again was refused"
    );
    call
}

/// One cut of `what` naming `leg`, one `store_error` mark of `win` naming it,
/// and never a `missing_block`.
fn one_cut(h: &Hive, call: &Msg, leg: &str, what: &str) {
    let cut = cuts_of(call, what);
    assert_eq!(
        cut.len(),
        1,
        "{leg}: one {what} cut: {:?} {:?}",
        call.hop,
        h.stderr
    );
    assert_eq!(cut[0]["legs"], json!([leg]), "{leg}: {cut:?}");
    assert_eq!(cut[0]["error_code"], "store_error", "{leg}: {cut:?}");
    assert!(
        cuts_of(call, "missing_block").is_empty(),
        "{leg}: a block that stands is no missing block: {:?}",
        call.hop
    );
    assert!(marks(h, "missing_block").is_empty(), "{leg}");
    assert!(
        marks(h, "store_error")
            .iter()
            .any(|v| v.contains("\"step\":\"win\"") && v.contains(leg)),
        "{leg}: one store_error mark names the leg: {:?}",
        marks(h, "store_error")
    );
}

#[test]
fn a_parked_turn_read_refused_twice_is_one_wall_cut() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    first_turn(&mut h);
    let call = refused_twice(&mut h, "b-slimq", None);
    one_cut(&h, &call, "b-slimq", "wall");
    assert!(
        texts(&call).iter().all(|t| !t.contains("Leipzig")),
        "{:?}",
        texts(&call)
    );
}

#[test]
fn a_pin_hash_read_refused_twice_is_a_pins_cut() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    first_turn(&mut h);
    let call = refused_twice(&mut h, "b-pins", None);
    one_cut(&h, &call, "b-pins", "pins");
    assert!(cuts_of(&call, "wall").is_empty(), "{:?}", call.hop);
    assert!(
        texts(&call).iter().any(|t| t.contains("Leipzig")),
        "the wall still goes: {:?}",
        texts(&call)
    );
}

#[test]
fn a_kept_row_hash_read_refused_twice_is_one_wall_cut() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    first_turn(&mut h);
    let s = seqs(&h);
    assert!(s.len() >= 2, "the first turn stands on the wall: {s:?}");
    // A plan that covers the first turn and keeps its first row: step B
    // reads the kept rows' hashes (`b-keeph`).
    let plan = json!({"v": 1, "cover": s[s.len() - 1], "keep": [s[0]]});
    set_plan(&h, Some(plan.clone()));
    let call = refused_twice(&mut h, "b-keeph", Some(&plan));
    one_cut(&h, &call, "b-keeph", "wall");
    assert!(
        texts(&call).iter().all(|t| !t.contains("Leipzig")),
        "{:?}",
        texts(&call)
    );
}

#[test]
fn a_cover_read_refused_twice_without_a_plan_is_one_wall_cut() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    first_turn(&mut h);
    set_plan(&h, None);
    let call = refused_twice(&mut h, "b-cover", None);
    one_cut(&h, &call, "b-cover", "wall");
}

#[test]
fn a_cover_read_refused_twice_under_a_plan_with_its_own_cover_cuts_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    first_turn(&mut h);
    let plan = json!({"v": 1, "cover": 0, "keep": []});
    set_plan(&h, Some(plan.clone()));
    let call = refused_twice(&mut h, "b-cover", Some(&plan));
    assert!(
        cuts_of(&call, "wall").is_empty(),
        "the plan holds its own cover: {:?} {:?}",
        call.hop,
        h.stderr
    );
    assert!(
        texts(&call).iter().any(|t| t.contains("Leipzig")),
        "the wall goes: {:?}",
        texts(&call)
    );
}
