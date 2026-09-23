//! GH #728, lock 2 — an advice answer carries the turn that asked.
//!
//! Before: `in_advice` minted a `uuid4` for the round it opened, and the answer that
//! left that round carried it — "a `turn_id` that belongs to no turn of the member", so
//! § 4.13 (the chat closes when a canvas window carries the id of the last turn) could
//! never fire for a window opened from it.
//!
//! After (OR-T11): the return is TWO-STAGED. Stage A parks the advice row under a
//! provisional key and, in the same bundle, looks the departure up by its correlation.
//! Stage B opens the round under a key that carries the member's turn as its LABEL —
//! `"<label>~<deadline_ms>~<hex8>"` — and `head()` hands the label out on the `answer`
//! route alone. The round keeps a key of its own (ruling point 3); the answer carries
//! the member's turn (point 1).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

const T: &str = "chat#0f1e2d3c4b5a6978";

fn future_depart() -> serde_json::Value {
    json!([{"turn_id": T, "deadline_ms": now_ms() + 600_000}])
}

#[test]
fn stage_a_parks_the_advice_and_asks_for_its_departure_in_one_bundle() {
    let out = assemble(&[], advice("k-7", "berlin: 21C"));
    assert_eq!(out.len(), 1, "one bundle, no memory leg yet: {out:?}");
    let look = in_phase(&out, "advice-look");
    let prov = hop_str(look, "turn_id");
    let depart = call(look, "c-look-depart");
    assert_eq!(depart["table"], "round");
    assert_eq!(
        depart["where"],
        json!({"session_id": SESSION, "role": "depart", "correlation": "k-7"}),
        "looked up by the correlation the advisor answered under, in this session"
    );
    assert_eq!(depart["limit"], 1);
    let row = call(look, "c-look-turn");
    assert_eq!(row["row"]["role"], "advice");
    assert_eq!(row["row"]["consult_id"], "k-7");
    assert_eq!(row["row"]["content"], "berlin: 21C");
    assert_eq!(row["row"]["turn_id"], prov.as_str());
    assert_ne!(prov, "cogny-round-1", "never the id on the advisor's hop");
    let me = call(look, "c-look-self");
    assert_eq!(me["where"], json!({"turn_id": prov}));
}

#[test]
fn stage_b_opens_the_round_under_a_key_that_carries_the_members_turn() {
    let (key, out) = open_advice("k-7", future_depart());
    assert!(
        key.starts_with(&format!("{T}~")),
        "the round key carries the member's turn as its label: {key}"
    );
    assert_ne!(
        key, T,
        "and it is NOT the member's turn itself — the round keeps its own"
    );
    let open = in_phase(&out, "turn-open");
    let (first, rekey) = calls_of(open)[0].clone();
    assert_eq!(first, "c-open-rekey");
    assert_eq!(rekey["operation"], "update");
    assert_eq!(rekey["set"], json!({"turn_id": key}));
    // The rest of the chain is the chain of every turn.
    let ids: Vec<String> = calls_of(open).into_iter().map(|(i, _)| i).collect();
    assert!(ids.contains(&"c-open-round".to_string()), "{ids:?}");
    assert!(ids.contains(&"c-open-win".to_string()), "{ids:?}");
}

#[test]
fn the_answer_of_that_round_carries_the_members_turn_and_its_own_round() {
    let (key, _) = open_advice("k-7", future_depart());
    let out = assemble(&[], answer_in(&key, "It is 21C and sunny in Berlin."));
    let ans = on_route(&out, "answer");
    assert_eq!(
        hop_str(ans, "turn_id"),
        T,
        "the answer carries the member's turn — the id every window from it writes"
    );
    assert_eq!(hop_str(ans, "round_id"), key, "and names the round it left");
    assert_eq!(hop_str(ans, "late"), "0", "inside the deadline it is a leg");
    // The stored answer row stays on the round key.
    let w = in_phase(&out, "ans-w");
    assert_eq!(call(w, "c-ans-w")["row"]["turn_id"], key.as_str());
}

#[test]
fn every_round_row_of_that_round_stays_on_its_key() {
    let (key, _) = open_advice("k-7", future_depart());
    // The turn-open reply: nothing open, a window of one row. The leg it parks is keyed
    // on the round, not on the member's turn.
    let reply = bundle_reply(
        "turn-open",
        &key,
        &[
            ("c-open-round", json!([])),
            (
                "c-open-win",
                json!([{"id": "1", "role": "advice", "content": "berlin: 21C",
                                    "turn_id": key, "session_id": SESSION}]),
            ),
        ],
    );
    let out = assemble(&[], reply);
    let collect = in_phase(&out, "collect");
    assert_eq!(
        call(collect, "c-collect-row")["row"]["turn_id"],
        key.as_str()
    );
    assert_eq!(
        call(collect, "c-collect-read")["where"],
        json!({"turn_id": key}),
        "the fan-in reads its own round and nobody else's"
    );
    // Every header on the way stays on the key — only `answer` hands out the label.
    assert_eq!(hop_str(collect, "turn_id"), key);
}
