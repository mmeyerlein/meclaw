//! GH #728, lock 6 — an advice without a departure behaves as before.
//!
//! Two shapes reach `in_advice` with nothing to look up: a return whose departure was
//! never written (a round from before this build, a tree whose dispatcher does not
//! classify the consult as a handoff) and an advice with no `consult_id` at all (the
//! gh420 replay shape). Both open their round as they did before the build — a fresh
//! id, no label, `late` empty — and the first one says so on stderr.
//!
//! And the adoption of a channel's stamp disarms `~` beside `|`: a channel id that read
//! like a round key would hand an answer a label nobody gave it.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

#[test]
fn an_empty_lookup_opens_the_round_under_the_provisional_key() {
    let a = assemble(&[], advice("k-7", "berlin: 21C"));
    let look = in_phase(&a, "advice-look");
    let prov = hop_str(look, "turn_id");
    let rid = call(look, "c-look-turn")["row"]["id"]
        .as_str()
        .expect("rid")
        .to_string();
    let (b, stderr) = run_cell(
        ASSEMBLE,
        &[],
        advice_looked(&prov, json!([]), &rid, "berlin: 21C", "k-7"),
    );
    let key = hop_str(in_phase(&b, "turn-open"), "turn_id");
    assert_eq!(key, prov, "no departure, no label: the key it already had");
    assert!(
        stderr.contains("k-7"),
        "and it says which consult: {stderr:?}"
    );
    let out = assemble(&[], answer_in(&key, "x"));
    let ans = on_route(&out, "answer");
    assert_eq!(hop_str(ans, "turn_id"), key);
    assert_eq!(hop_str(ans, "late"), "");
}

#[test]
fn an_advice_without_a_consult_id_is_one_stage_as_before() {
    let out = assemble(
        &[],
        lane(
            "in_advice",
            json!({}),
            json!({}),
            json!([{"origin": "assistant", "type": "text", "text": "berlin: 21C"}]),
        ),
    );
    let open = in_phase(&out, "turn-open");
    let key = hop_str(open, "turn_id");
    assert!(!key.contains('~'), "{key}");
    assert_eq!(call(open, "c-open-turn")["row"]["turn_id"], key.as_str());
}

#[test]
fn a_channel_stamp_with_a_tilde_is_disarmed_on_adoption() {
    let out = assemble(
        &[],
        lane(
            "in_turn",
            json!({"turn_id": "chat#a~1~deadbeef"}),
            json!({}),
            json!([{"origin": "user", "type": "text", "text": "hi"}]),
        ),
    );
    let key = hop_str(in_phase(&out, "turn-open"), "turn_id");
    assert_eq!(key, "chat#a_1_deadbeef");
    let ans = on_route(&assemble(&[], answer_in(&key, "hello")), "answer").clone();
    assert_eq!(hop_str(&ans, "turn_id"), key, "no label was invented");
    assert_eq!(hop_str(&ans, "late"), "");
}

/// T5 review M5: stage B re-keys the advice row it wrote in stage A. When that
/// row does not come back there is nothing to re-key and the advice ends here —
/// which it used to do without a sound. It says so on stderr now, naming the
/// provisional key the row was filed under.
#[test]
fn a_stage_b_without_its_own_row_says_so_on_stderr() {
    let (out, stderr) = run_cell(
        ASSEMBLE,
        &[],
        bundle_reply(
            "advice-look",
            "prov-9",
            &[
                ("c-look-depart", json!([])),
                ("c-look-turn", serde_json::Value::Null),
                ("c-look-self", json!([])),
            ],
        ),
    );
    assert!(
        out.is_empty(),
        "nothing to re-key, nothing to emit: {out:?}"
    );
    assert!(
        stderr.contains("prov-9"),
        "the dropped advice is named on stderr: {stderr:?}"
    );
}
