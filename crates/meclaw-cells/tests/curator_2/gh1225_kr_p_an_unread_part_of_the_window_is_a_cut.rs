//! GH #1225 (A3, policy) -- a part of the window that could not be read is
//! one cut, never a row of missing blocks.
//!
//! WHY this file exists: `./policy` builds a call's window from the hashes of
//! step B (`b-wallh`, `b-keeph`, `b-slotsh`, `b-pins`). A hash read refused
//! twice dropped its rows' blocks from the fetch: every wall row became a
//! `missing_block` cut and mark (loud, but false -- the block stands), slots
//! and pins fell out with nothing said. And a parked turn whose plan
//! `./intake` could not read (`wall_unread`) was read by `./policy` as if the
//! plan stood: the whole wall after cover 0, each older row a false
//! `missing_block`. Now each is one cut `{what, error_code, legs}` with one
//! `store_error` mark, and an unread plan shows the call's own turn only.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-krp";
const WORDS: &str = "My daughter Anna moves to Hamburg in May.";

fn cuts(call: &Msg) -> Vec<Value> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn cuts_of(call: &Msg, what: &str) -> Vec<Value> {
    cuts(call)
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

/// Step B refused whole, and the read again refuses `leg`: the call goes
/// without that part of the window and says so once -- not as one
/// `missing_block` per wall row.
fn refused_twice(leg: &str) -> (Hive, Msg) {
    let mut h = Hive::new();
    first_turn(&mut h);
    h.ledger_may_refuse = true;
    h.refuse_phase = Some("win-hashes".to_string());
    h.refuse_legs = vec![(leg.to_string(), "store_error".to_string())];
    let call = h.curate(S, "r2", 0, json!([user(WORDS)]), mode("Be brief."));
    assert!(h.refuse_phase.is_none(), "{leg}: step B was refused");
    assert!(
        h.refuse_legs.is_empty(),
        "{leg}: the read again was refused"
    );
    (h, call)
}

#[test]
fn a_wall_hash_read_refused_twice_is_one_wall_cut() {
    if !shipped() {
        return;
    }
    let (h, call) = refused_twice("b-wallh");
    let wall = cuts_of(&call, "wall");
    assert_eq!(wall.len(), 1, "one wall cut: {:?} {:?}", call.hop, h.stderr);
    assert_eq!(wall[0]["legs"], json!(["b-wallh"]), "{wall:?}");
    assert_eq!(wall[0]["error_code"], "store_error", "{wall:?}");
    assert!(
        cuts_of(&call, "missing_block").is_empty(),
        "a block that stands is no missing block: {:?}",
        call.hop
    );
    assert!(
        marks(&h, "missing_block").is_empty(),
        "{:?}",
        marks(&h, "missing_block")
    );
    assert!(
        marks(&h, "store_error")
            .iter()
            .any(|v| v.contains("\"step\":\"win\"") && v.contains("b-wallh")),
        "one store_error mark names the leg: {:?}",
        marks(&h, "store_error")
    );
    assert!(
        texts(&call).iter().all(|t| !t.contains("Leipzig")),
        "{:?}",
        texts(&call)
    );
}

#[test]
fn a_slot_hash_read_refused_twice_is_a_slots_cut() {
    if !shipped() {
        return;
    }
    let (h, call) = refused_twice("b-slotsh");
    let slots = cuts_of(&call, "slots");
    assert_eq!(
        slots.len(),
        1,
        "one slots cut: {:?} {:?}",
        call.hop,
        h.stderr
    );
    assert_eq!(slots[0]["legs"], json!(["b-slotsh"]), "{slots:?}");
    assert!(cuts_of(&call, "wall").is_empty(), "{:?}", call.hop);
    assert!(
        texts(&call).iter().any(|t| t.contains("Leipzig")),
        "the wall still goes: {:?}",
        texts(&call)
    );
}

#[test]
fn an_unread_plan_shows_the_calls_own_turn_as_one_cut() {
    if !shipped() {
        return;
    }
    for leg in ["a-plan", "a-cover"] {
        let mut h = Hive::new();
        first_turn(&mut h);
        h.ledger_may_refuse = true;
        h.refuse_legs = vec![(leg.to_string(), "store_error".to_string())];
        let call = h.curate(S, "r2", 0, json!([user(WORDS)]), mode("Be brief."));
        assert!(h.refuse_legs.is_empty(), "{leg}: the read failed");
        let wall = cuts_of(&call, "wall");
        assert_eq!(
            wall.len(),
            1,
            "{leg}: one wall cut: {:?} {:?}",
            call.hop,
            h.stderr
        );
        assert_eq!(wall[0]["legs"], json!([leg]), "{leg}: {wall:?}");
        assert_eq!(wall[0]["error_code"], "store_error", "{leg}: {wall:?}");
        assert!(
            cuts_of(&call, "missing_block").is_empty(),
            "{leg}: a row never read is no missing block: {:?}",
            call.hop
        );
        assert!(
            marks(&h, "missing_block").is_empty(),
            "{leg}: {:?}",
            marks(&h, "missing_block")
        );
        assert!(
            !marks(&h, "store_error").is_empty(),
            "{leg}: said with a mark"
        );
        assert!(
            texts(&call).iter().all(|t| !t.contains("Leipzig")),
            "{leg}: never an older turn: {:?}",
            texts(&call)
        );
        assert!(
            texts(&call).iter().any(|t| t.contains("Hamburg")),
            "{leg}: the call carries its own turn"
        );
    }
}
