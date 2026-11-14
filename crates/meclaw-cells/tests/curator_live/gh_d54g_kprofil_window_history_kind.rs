//! D54g kprofil (GH #1507, OR-54g-83) -- an entry of a delegation's history
//! keeps its own voice: one that is neither the person's nor the model's is
//! never filed as the person's words.
//!
//! WHY this file exists: the history a delegation carries (OR-54g-31) went
//! through two readers that knew two voices only. The collector's
//! `history_of` (`collector/assemble`) and the curator's `history_of`
//! (`curator/intake`) made every origin other than `assistant` a `user`, so a
//! `system` frame, a `tool` output and a `peer`'s words of the sender's
//! conversation landed on the wall as rows of kind `user`. Every reader of
//! the person's words reads that kind or that origin -- the anchors of the
//! memory (GH #1342, `./push` `recent_rounds`), the turn of a side window
//! (`./policy` `turn_window`), the transcript of a summary -- and took them
//! for something the person said. Now such an entry keeps its origin on the
//! seam and is filed under a kind of its own; it is kept, never dropped.
//! Told apart by the field `origin` only, never by the words.
//!
//! Every knob as shipped. The hive in one process (`support/curator_hive.rs`,
//! `support/curator_live.rs`), the collector's script through
//! `support/assemble_cell.rs`; no model.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

const OPEN_UNTIL: u64 = 4_102_444_800_000;
/// The member turn whose delegation round carries the history.
const LABEL: &str = "voice-turn-3";
/// The session the delegation round runs under.
const DELEGATION_SESSION: &str = "deleg-session-83";
/// The task the delegation carries last.
const TASK: &str = "book the table for seven";
/// One text of each voice that is neither the person's nor the model's.
const FRAME: &str = "frame of the sender: the caller is on a train";
const OUTPUT: &str = "result of the sender's lookup: three tables free";
const PEER: &str = "the other speaker in the room asks for the terrace";
/// The person's and the model's words of the sender's conversation.
const ASKED: &str = "is there a table tonight";
const SPOKEN: &str = "let me look that up";

fn event_key(label: &str) -> String {
    format!("{label}~{OPEN_UNTIL}~ab12cd83")
}

fn text_of(origin: &str, text: &str) -> Value {
    json!({"origin": origin, "type": "text", "text": text})
}

/// The history in front of the task: one entry of each voice.
fn history() -> Vec<Value> {
    vec![
        text_of("system", FRAME),
        text_of("tool", OUTPUT),
        text_of("peer", PEER),
        user(ASKED),
        said(SPOKEN),
    ]
}

/// The origins the history above names, in its order.
const ORIGINS: [&str; 5] = ["system", "tool", "peer", "user", "assistant"];

/// A delegation with `messages` through the shipped collector, hop by hop
/// (arrival, turn-open reply, fan-in); returns the seam (`curate`).
fn collected(messages: Value) -> Value {
    use assemble_cell as ac;
    let out = ac::assemble(
        &[],
        ac::lane(
            "in_delegation",
            json!({"turn_id": LABEL}),
            json!({"delegation_id": "item_d83", "engine": "duplex"}),
            messages,
        ),
    );
    let open = ac::in_phase(&out, "turn-open").clone();
    let key = ac::hop_str(&open, "turn_id");
    let own = ac::call(&open, "c-open-turn")["row"].clone();
    let opened = ac::bundle_reply(
        "turn-open",
        &key,
        &[
            ("c-open-round", json!([])),
            ("c-open-win", json!([own])),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
        ],
    );
    let out = ac::assemble(&[], opened);
    let collect = out
        .iter()
        .find(|m| m["header"]["phase"].as_str() == Some("collect"))
        .unwrap_or_else(|| panic!("the round opens: {out:?}"))
        .clone();
    let stored = |row: &Value| {
        json!({"turn_id": key, "iter": 0, "role": row["role"], "turn": row["turn"],
               "fired": 0, "recorded_at": row["recorded_at"]})
    };
    let mut rows = vec![stored(&ac::call(&collect, "c-collect-row")["row"])];
    for (id, args) in ac::calls_of(&open) {
        if id == "c-open-history" {
            rows.push(stored(&args["row"]));
        }
    }
    let out = ac::assemble(
        &[],
        ac::bundle_reply("collect", &key, &[("c-collect-read", Value::Array(rows))]),
    );
    ac::on_route(&out, "curate").clone()
}

/// The delegation round of turn [`LABEL`] with its task and `history`.
fn delegation_with(h: &mut Hive, history: Value) {
    h.out.clear();
    let key = event_key(LABEL);
    let mut hop = json!({"session_id": DELEGATION_SESSION, "turn_id": key, "iter": "0",
                         "phase": "", "path": "talk", "needs": "", "families": "",
                         "delegation_id": "item_d83"});
    let mut ctx = json!({"session_id": DELEGATION_SESSION, "turn_id": key, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    for (k, v) in obj(live_keys(LIVE)) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(TASK)], "history": history}),
    );
}

/// The kind of every wall row the history was filed as, oldest first.
fn filed_kinds(h: &Hive) -> Vec<String> {
    h.rows(&format!(
        "SELECT kind FROM wall WHERE session_id = '{DELEGATION_SESSION}' \
         AND turn_id <> '{}' ORDER BY seq",
        event_key(LABEL)
    ))
    .into_iter()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
    .collect()
}

#[test]
fn gh_d54g_kprofil_window_the_collector_keeps_the_voice_of_every_history_entry() {
    if !shipped() {
        return;
    }
    let mut messages = history();
    messages.push(user(TASK));
    let seam = collected(Value::Array(messages));
    let carried = seam["history"].as_array().cloned().unwrap_or_default();
    let origins: Vec<&str> = carried
        .iter()
        .map(|e| e["origin"].as_str().unwrap_or(""))
        .collect();
    // Kept, all of them, in order -- and each in its own voice: a frame, a
    // tool's output and a peer's words are not the person's.
    assert_eq!(
        origins,
        ORIGINS.to_vec(),
        "every entry rides the seam in its own voice: {carried:?}"
    );
    for (entry, text) in carried.iter().zip([FRAME, OUTPUT, PEER, ASKED, SPOKEN]) {
        assert_eq!(entry["text"].as_str(), Some(text), "whole: {carried:?}");
    }
}

#[test]
fn gh_d54g_kprofil_window_a_history_entry_of_another_voice_is_never_the_persons() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[]);
    delegation_with(&mut h, Value::Array(history()));
    // (a) Kept, each under a kind of its own: nothing is dropped, and the
    // three other voices are filed as what they are.
    assert_eq!(
        filed_kinds(&h),
        ORIGINS.to_vec(),
        "each entry is a round of the wall in its own kind"
    );
    // (b) The readers of the person's words go by the row's kind (GH #1342:
    // `user`, and a peer's turn as `peer`) and by the element's origin (the
    // person's turn of a window): only the person's own entry is `user`.
    let users = h.rows(&format!(
        "SELECT count(*) FROM wall WHERE session_id = '{DELEGATION_SESSION}' \
         AND turn_id <> '{}' AND kind = 'user'",
        event_key(LABEL)
    ));
    assert_eq!(
        users[0][0].as_i64(),
        Some(1),
        "one row of the person's words: the person's own"
    );
    let call = h
        .out
        .iter()
        .find(|m| m.route() == "model_call")
        .cloned()
        .expect("the delegation round calls its target");
    for (text, origin) in [(FRAME, "system"), (OUTPUT, "tool"), (PEER, "peer")] {
        let shown: Vec<Value> = call
            .messages()
            .iter()
            .filter(|m| m["text"].as_str().unwrap_or("").contains(text))
            .cloned()
            .collect();
        assert_eq!(shown.len(), 1, "`{text}` is shown once: {shown:?}");
        assert_ne!(
            shown[0]["origin"].as_str(),
            Some("user"),
            "the {origin} entry is no word of the person: {shown:?}"
        );
        // And in a form the wire of every provider takes for a text
        // (`llm` `map_turn`: user, peer, assistant, system).
        assert!(
            ["peer", "assistant", "system"].contains(&shown[0]["origin"].as_str().unwrap_or("")),
            "the {origin} entry is a text the wire takes: {shown:?}"
        );
    }
}
