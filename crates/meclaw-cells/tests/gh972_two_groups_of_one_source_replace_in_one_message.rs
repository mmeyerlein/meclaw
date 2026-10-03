//! GH #972 (PE-OG-O4-3 a, curator side) — every audience group of one source
//! travels in ONE `in_pin` message with ONE `replace_sources`.
//!
//! Before: a pin had no round of its own, so a source with goals in two rounds
//! sent one message per round, and only the first could carry
//! `replace_sources` -- the second would have ended the first's pins. With a
//! round per pin (`gh972_a_pin_carries_the_round_its_row_proves`) the whole
//! set is one message, and the replacement ends every live pin of the source
//! except the ones in it, across all rounds. A pin of the set that waits
//! (`pin:held`) is still part of it: its live row is not ended, or every turn
//! of another round would end it until its own round came back.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const MEMBER: &str = r#"["agent:scribe","member:e"]"#;
const GROUP_B: &str = r#"["agent:scribe","member:b","member:e"]"#;
const PRIVATE: &str = r#"["member:e"]"#;

fn door(h: &mut Hive, turn_round: &str, pins: Value) {
    h.lane(
        "in_pin",
        json!({"audience_set": MEMBER, "turn_round": turn_round}),
        json!({}),
        json!({"pins": pins, "replace_sources": ["goal"]}),
    );
}

/// `(text, audience_set)` of every live pin of `goal`.
fn live(h: &Hive) -> Vec<(String, String)> {
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

fn a() -> Value {
    json!({"text": "diary", "source": "goal", "audience_set": PRIVATE})
}

fn b() -> Value {
    json!({"text": "paint the fence", "source": "goal", "audience_set": GROUP_B})
}

#[test]
fn two_groups_of_one_source_replace_in_one_message() {
    if !shipped() {
        return;
    }
    let both = vec![
        ("diary".to_string(), round_canon(PRIVATE).unwrap()),
        ("paint the fence".to_string(), round_canon(GROUP_B).unwrap()),
    ];
    // Order does not matter: A then B, or B then A, in a turn of group B.
    for set in [json!([a(), b()]), json!([b(), a()])] {
        let mut h = Hive::new();
        door(&mut h, GROUP_B, set.clone());
        assert_eq!(live(&h), both, "{set}: {:?}", h.stderr);
        // Both again in the member's turn: B waits (no proof in this turn)
        // but stays alive, A stays.
        door(&mut h, MEMBER, set);
        assert_eq!(live(&h), both, "a held pin was ended: {:?}", h.stderr);
        assert!(h.stderr.join("").contains("pin:held"), "{:?}", h.stderr);
        // The next message names only A: B ends, nothing is deleted.
        door(&mut h, MEMBER, json!([a()]));
        assert_eq!(live(&h), both[..1].to_vec(), "{:?}", h.stderr);
        assert_eq!(
            h.rows("SELECT hash FROM pins WHERE source = 'goal'").len(),
            2,
            "an ended pin keeps its row"
        );
    }
}
