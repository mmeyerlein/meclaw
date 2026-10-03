//! GH #972 (PE-DP-13, ruling R-NL-4) — an app's pin carries the round of its
//! row only on proof from the context.
//!
//! Before: the builder's edge of an app's pin lane stamped the MEMBER round on
//! every `in_pin` message and the curator wrote every pin in it, so a group
//! goal never reached its group (fail-closed, but narrowed: orga
//! `test_h_expected_narrowing`). Now each pin may name its own `audience_set`,
//! and the curator keeps it when it is
//!
//!   (a) the round of the turn this message belongs to -- `context.turn_round`,
//!       stamped by the edge that raised the turn, which no config's own edge
//!       may set (`STAMPED_CONTEXT_KEYS`, refused as `edge_schema` at the door
//!       and in the boot pass), or
//!   (b) inside the ceiling -- `context.audience_set`, still the member round
//!       the builder's pin edge stamps after every edge of the app.
//!
//! Anything else is not placed: `pin:held` with its reason on the journal, and
//! the app pins it again in a turn of that round. A pin without a round takes
//! the ceiling, as before. Measured at the receiver: the curator's `pins` rows.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The member's round, as the builder's pin edge stamps it (the ceiling).
const MEMBER: &str = r#"["agent:scribe","member:e"]"#;
/// A group round with a guest: wider than the member's round.
const GROUP: &str = r#"["agent:scribe","member:b","member:e"]"#;
/// Only the person: narrower than the member's round.
const PRIVATE: &str = r#"["member:e"]"#;

fn canon(round: &str) -> String {
    round_canon(round).expect("a round")
}

/// One `in_pin` as it reaches the curator: the ceiling on `audience_set`, the
/// turn's round on `turn_round` when a turn raised the chain.
fn pin_door(h: &mut Hive, turn_round: Option<&str>, pins: Value) {
    let mut ctx = json!({"audience_set": MEMBER});
    if let Some(t) = turn_round {
        ctx["turn_round"] = json!(t);
    }
    h.lane("in_pin", ctx, json!({}), json!({"pins": pins}));
}

/// `(text, audience_set)` of every live pin of `goal`.
fn rounds(h: &Hive) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = h
        .rows(
            "SELECT b.body, p.audience_set FROM pins p JOIN blocks b ON b.hash = p.hash \
             WHERE p.source = 'goal' AND (p.until IS NULL OR p.until = '')",
        )
        .into_iter()
        .map(|r| {
            let body: Value =
                meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}")).unwrap();
            (
                body["text"].as_str().unwrap_or("").to_string(),
                r[1].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn held(h: &Hive) -> bool {
    h.stderr.join("").contains("pin:held")
}

#[test]
fn a_pin_without_round_takes_the_edge_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    pin_door(
        &mut h,
        Some(GROUP),
        json!([{"text": "swim", "source": "goal"}]),
    );
    assert_eq!(
        rounds(&h),
        [("swim".into(), canon(MEMBER))],
        "{:?}",
        h.stderr
    );
}

#[test]
fn a_pin_narrower_than_the_member_round_keeps_its_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // No turn at all (the app's own timer): a narrower round needs no proof.
    pin_door(
        &mut h,
        None,
        json!([{"text": "diary", "source": "goal", "audience_set": PRIVATE}]),
    );
    assert_eq!(
        rounds(&h),
        [("diary".into(), canon(PRIVATE))],
        "{:?}",
        h.stderr
    );
    assert!(!held(&h));
}

#[test]
fn a_pin_in_the_round_of_its_turn_keeps_it() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // A turn of the group with a guest; the substrate stamped its round.
    pin_door(
        &mut h,
        Some(GROUP),
        json!([{"text": "paint the fence", "source": "goal", "audience_set": GROUP}]),
    );
    assert_eq!(
        rounds(&h),
        [("paint the fence".into(), canon(GROUP))],
        "{:?}",
        h.stderr
    );
}

/// Red before the fix: every pin stood in the member round whatever it said.
/// Now a group round claimed outside a turn of that group is never placed --
/// not on the app's timer, not in the member's own turn, and not in a turn of
/// another group -- and the journal says why.
#[test]
fn a_private_goal_never_lands_in_a_group_round_with_a_guest() {
    if !shipped() {
        return;
    }
    let other = r#"["agent:scribe","member:c","member:e"]"#;
    for turn in [None, Some(MEMBER), Some(PRIVATE), Some(other)] {
        let mut h = Hive::new();
        pin_door(
            &mut h,
            turn,
            json!([{"text": "my secret goal", "source": "goal", "audience_set": GROUP}]),
        );
        assert!(rounds(&h).is_empty(), "{turn:?}: placed: {:?}", rounds(&h));
        assert!(held(&h), "{turn:?}: no journal line: {:?}", h.stderr);
        assert!(
            h.rows("SELECT hash FROM pins WHERE audience_set LIKE '%member:b%'")
                .is_empty(),
            "{turn:?}: a row in the guest's round"
        );
    }
}

#[test]
fn a_held_pin_appears_on_the_next_turn_of_its_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let pin = json!([{"text": "paint the fence", "source": "goal", "audience_set": GROUP}]);
    pin_door(&mut h, Some(MEMBER), pin.clone());
    assert!(rounds(&h).is_empty() && held(&h), "{:?}", h.stderr);
    // The next turn of that group: the app pins it again, the turn proves it.
    pin_door(&mut h, Some(GROUP), pin);
    assert_eq!(
        rounds(&h),
        [("paint the fence".into(), canon(GROUP))],
        "{:?}",
        h.stderr
    );
}

/// The context half of R-NL-4: no config's own edge -- an app's inner edges
/// are exactly that -- may set a stamped key, at the door and in the boot
/// pass (the builder's wiring stamps it: `gh967` reads it off the rendered
/// ingress edge).
#[test]
fn an_app_cannot_set_the_turn_round() {
    use meclaw_colony::cel_eval::STAMPED_CONTEXT_KEYS;
    use meclaw_colony::mutation::substitute::substitute_env_only;
    assert!(STAMPED_CONTEXT_KEYS.contains(&"turn_round"));
    let app = json!({"name": "apps/x", "params": {"graph": {"edges": [
        {"from": "./pin", "to": ".",
         "modifier": {"set_context": {"turn_round": format!("'{GROUP}'")}}}
    ]}}});
    let err = substitute_env_only(&app, &std::collections::HashMap::new()).unwrap_err();
    assert_eq!(err.error_code(), "edge_schema", "{err:?}");
    assert!(err.message().contains("turn_round"), "{err:?}");
}
