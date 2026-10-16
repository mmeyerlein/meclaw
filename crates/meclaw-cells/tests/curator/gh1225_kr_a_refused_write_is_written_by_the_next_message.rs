//! GH #1225 (A1) -- a write the ledger refused is written by the next message.
//!
//! WHY this file exists: `./intake` kept what step B was to write as written
//! in its RAM (`_MEM["wall"]`, `_MEM["blocks"]`), though the B bundle answers
//! `./policy` and never this cell. One B the store refused (a full disk, GH
//! #1082 `store_degraded`) left the RAM saying "written": every later re-send
//! of the round skipped the person's row and its block for the life of the
//! child -- the model never saw what the person said. Now every write of B is
//! ONE step by its own key (`if_absent`), and the RAM guards only the writes
//! still in flight. The behaviour, with the child alive the whole time: the
//! first B is refused, the round goes on, and the next message writes the
//! person's row once, with its block, into the next call.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-ea1";
const T: &str = "t-ea1";
const WORDS: &str = "My name is Herta and my son is called Jan.";

#[test]
fn a_refused_write_is_written_by_the_next_message() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    // The first B of the round is refused whole; the child lives on.
    h.refuse_phase = Some("win-hashes".into());
    let ctx = json!({"session_id": S, "turn_id": T, "iter": "0", "channel": "test",
                     "audience_set": ROUND_E});
    let hop = json!({"session_id": S, "turn_id": T, "iter": "0", "phase": ""});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(WORDS)], "system": mode("Be brief.")}),
    );
    assert!(
        h.refuse_phase.is_none(),
        "the B was refused: {:?}",
        h.stderr
    );
    assert!(
        h.rows("SELECT 1 FROM wall WHERE kind = 'user'").is_empty(),
        "nothing of the refused B stands"
    );
    // The next message of the same round: the person's words again, with a
    // tool call and its result.
    let call = h.curate(
        S,
        T,
        1,
        json!([
            user(WORDS),
            tool_call("c1", "lookup"),
            tool_result("c1", "{\"ok\": true}")
        ]),
        mode("Be brief."),
    );
    let person = h.rows(&format!(
        "SELECT seq FROM wall WHERE session_id = '{S}' AND turn_id = '{T}' AND kind = 'user'"
    ));
    assert_eq!(
        person.len(),
        1,
        "the person's row exactly once: {:?}",
        h.stderr
    );
    let orphans = h.rows(
        "SELECT w.hash FROM wall w LEFT JOIN blocks b ON b.hash = w.hash WHERE b.hash IS NULL",
    );
    assert!(
        orphans.is_empty(),
        "every wall row has its block: {orphans:?}"
    );
    let id = call.hop["curator_call"].as_str().expect("a call id");
    let shown: Vec<String> = h
        .rows(&format!(
            "SELECT b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
             WHERE cb.call_id = '{id}'"
        ))
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        shown.iter().any(|b| b.contains("Herta")),
        "the person's words are in the next call: {shown:?}"
    );
}
