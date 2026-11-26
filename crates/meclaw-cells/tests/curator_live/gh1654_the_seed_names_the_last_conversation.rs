//! GH #1654 -- the seed of a new call names the last conversation.
//!
//! WHY this file exists: every call starts as a new conversation without the
//! turns of earlier ones (R-W25fix-1), and the greeting stays a developer
//! instruction. To greet in the light of what came before, the start window
//! carries ONE developer line about the last conversation, made only of the
//! curator's own ledger: when it ended (the close's time in the zone and how
//! long ago), its channel, its close and whether it ended open (the person's
//! words or an errand without an answer -- structure, never the words), and
//! the handover note of that session through the audience rule (GH #925).
//! The line stands right after the facts and before a held greeting, counts
//! in the seed's budget and is never cut before turns go; a renewal gets
//! none, and without an earlier conversation there is none. An answer is a
//! final assistant row that was not withdrawn: the sentence beside a tool
//! call and a withdrawn answer leave the conversation open. A close or a
//! note of a round the new call may not see stays out; a note longer than
//! its share of the start budget is left out whole.
//!
//! The hive in one process (`support/curator_hive.rs`); no model; the clock
//! is the param `now_utc`, the close's time the keeper's `closed_at`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

const HEAD: &str = "Last conversation: ";
const NOW: &str = "2026-11-26T10:35:00Z";
const CLOSED_AT: &str = "2026-11-26T10:00:00.000000Z";
const NODE: &str = "channels/freeswitch/voice";
const NOTE: &str = "The person asked about the weather; a call back was agreed for 18:00.";
const GREETING: &str = "Greet the person by name.";
const FOREIGN: &str = r#"["member:x"]"#;
/// The time the rows of the old conversation stand at, before its close.
const SAID_AT: &str = "2026-11-26T09:55:00Z";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

/// The hive of the pins: the clock fixed, the zone Berlin.
fn hive() -> Hive {
    live_hive(&[
        ("history", "now_utc", json!(NOW)),
        ("history", "timezone", json!("Europe/Berlin")),
    ])
}

/// The keeper's close of `session`, with the facts it says since GH #1305.
fn close(h: &mut Hive, session: &str) {
    close_under(h, session, ROUND_E, "call_ended");
}

/// [`close`] in the round `aud` with the reason `reason`.
fn close_under(h: &mut Hive, session: &str, aud: &str, reason: &str) {
    h.lane(
        "in_close",
        json!({"session_id": session, "audience_set": aud, "channel": "voice"}),
        json!({"session_id": session, "close_reason": reason, "closed_at": CLOSED_AT,
               "opened_at": "2026-11-26T09:50:00.000000Z", "channel_node": NODE}),
        json!({"messages": []}),
    );
}

/// The instant `SAID_AT` plus `s` seconds.
fn said_at(s: i64) -> chrono::DateTime<chrono::Utc> {
    SAID_AT.parse::<chrono::DateTime<chrono::Utc>>().unwrap() + chrono::Duration::seconds(s)
}

/// The person's words of turn `t1` of `s-old`, straight into the wall.
fn asked(h: &mut Hive) {
    h.row(
        said_at(0),
        "s-old",
        "t1",
        "user",
        &user("what will the weather be?"),
        1,
    );
}

/// The open end of the line: the person's words got no answer.
const OPEN: &str = "it ended open: the person's last words got no answer";
/// The open end of the line: an errand handed on got no answer.
const ERRAND_OPEN: &str = "it ended open: an errand handed on got no answer";
/// The line's word for an answered end.
const ANSWERED: &str = "the last word was the assistant's";

/// The handover note of `session` straight into the ledger, under `aud`.
fn sow_note(h: &Hive, session: &str, aud: &str) {
    let note = json!({"type": "summary", "text": NOTE});
    let hash = sha256_hex(&canonical(&note));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'summary', ?2, ?3, 'x')",
        rusqlite::params![hash, NOTE.len() as i64, canonical(&note)],
    )
    .unwrap();
    let prepared = json!({"state": "prepared", "hash": hash, "from_session": session});
    h.db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
         VALUES (1, ?1, '', 'handover', ?2, '2026-11-26T10:00:01.000000Z', ?3)",
        rusqlite::params![session, prepared.to_string(), aud],
    )
    .unwrap();
}

fn seed(h: &mut Hive, reason: &str) -> Vec<Value> {
    let seeds = ask_seed(h, &live_keys(LIVE), reason);
    assert_eq!(seeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    seeds[0].messages()
}

fn last_line(msgs: &[Value]) -> Option<(usize, String)> {
    msgs.iter()
        .position(|m| m["role"] == "developer" && text(m).starts_with(HEAD))
        .map(|i| (i, text(&msgs[i]).to_string()))
}

#[test]
fn gh1654_the_seed_names_when_where_and_how_the_last_conversation_ended() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    // The person's words, and the call ended before an answer reached the wall.
    seam(&mut h, "s-old", "s-old#0", "talk", &json!({}), json!({}));
    close(&mut h, "s-old");
    // A held head line, the greeting of the set.
    h.lane(
        "in_seed_line",
        json!({}),
        json!({}),
        json!({"messages": [], "text": GREETING, "position": "head", "lasts": "always"}),
    );
    let msgs = seed(&mut h, "start");
    let (at, line) = last_line(&msgs).unwrap_or_else(|| panic!("no line: {msgs:#?}"));
    assert_eq!(at, 1, "the line stands right after the facts: {msgs:#?}");
    assert!(text(&msgs[0]).starts_with("Facts: "), "{msgs:#?}");
    assert_eq!(
        text(&msgs[2]),
        GREETING,
        "the line stands before the held greeting: {msgs:#?}"
    );
    for part in [
        "2026-11-26T11:00:00+01:00",
        "35 minutes ago",
        NODE,
        "call_ended",
        "no answer",
    ] {
        assert!(line.contains(part), "the line names {part:?}: {line}");
    }
    assert_eq!(
        msgs.iter().filter(|m| text(m).starts_with(HEAD)).count(),
        1,
        "exactly one line: {msgs:#?}"
    );
}

#[test]
fn gh1654_an_answered_end_is_no_open_end() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(!line.contains("no answer"), "{line}");
    assert!(
        line.contains("the assistant"),
        "the last word is named: {line}"
    );
}

#[test]
fn gh1654_the_line_carries_the_handover_note_the_round_may_see() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    sow_note(&h, "s-old", ROUND_E);
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(line.contains(NOTE), "the note of that session: {line}");
}

#[test]
fn gh1654_a_note_of_another_round_stays_out() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    sow_note(&h, "s-old", r#"["member:x"]"#);
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(!line.contains(NOTE), "a note the round may not see: {line}");
}

#[test]
fn gh1654_a_renewal_gets_no_line() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    for reason in ["renew", "recover"] {
        let msgs = seed(&mut h, reason);
        assert!(last_line(&msgs).is_none(), "{reason}: {msgs:#?}");
    }
}

#[test]
fn gh1654_without_an_earlier_conversation_there_is_no_line() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    let msgs = seed(&mut h, "start");
    assert!(last_line(&msgs).is_none(), "no close, no line: {msgs:#?}");
}

/// The turns of a seed (`user`/`assistant` lines).
fn turn_lines(msgs: &[Value]) -> usize {
    msgs.iter()
        .filter(|m| m["role"] == "user" || m["role"] == "assistant")
        .count()
}

/// A hive with one answered and closed conversation, `over` its params.
fn conversation_hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut p = vec![("history", "now_utc", json!(NOW))];
    p.extend_from_slice(over);
    let mut h = live_hive(&p);
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    h
}

#[test]
fn gh1654_the_line_counts_in_the_budget() {
    if !shipped() {
        return;
    }
    // Without the line, the whole seed -- facts, turns, tail -- at its own
    // count: the budget that just holds every turn.
    let mut off = conversation_hive(&[("history", "seed_last_head", json!(""))]);
    let seeds = ask_seed(&mut off, &live_keys(LIVE), "start");
    assert_eq!(seeds.len(), 1, "{:?}", off.stderr);
    let all = turn_lines(&seeds[0].messages());
    assert!(
        all > 0,
        "the control holds turns: {:?}",
        seeds[0].messages()
    );
    let fits = seeds[0].body["est_tokens"].as_i64().expect("est_tokens");
    // Control: at exactly that budget every turn stays without the line.
    let mut exact = conversation_hive(&[
        ("history", "seed_last_head", json!("")),
        ("history", "seed_budget", json!(fits)),
    ]);
    let ctl = seed(&mut exact, "start");
    assert!(last_line(&ctl).is_none(), "{ctl:#?}");
    assert_eq!(
        turn_lines(&ctl),
        all,
        "every turn fits the budget: {ctl:#?}"
    );
    // With the line at the same budget, the line stands and a turn goes:
    // the line is counted in the budget.
    let mut on = conversation_hive(&[("history", "seed_budget", json!(fits))]);
    let msgs = seed(&mut on, "start");
    assert!(last_line(&msgs).is_some(), "the line stands: {msgs:#?}");
    assert!(
        turn_lines(&msgs) < all,
        "the line takes the budget of the oldest turn: {msgs:#?}"
    );
}

#[test]
fn gh1654_an_interim_answer_beside_a_tool_call_is_no_answer() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    asked(&mut h);
    // The sentence beside a tool call (`final` 0), and the call ended in
    // the tool round.
    h.row(
        said_at(1),
        "s-old",
        "t1",
        "assistant",
        &said("let me look"),
        0,
    );
    close(&mut h, "s-old");
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(line.contains(OPEN), "an interim answer is none: {line}");
    assert!(!line.contains(ANSWERED), "{line}");
}

#[test]
fn gh1654_a_withdrawn_answer_is_no_answer() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    asked(&mut h);
    h.row(
        said_at(1),
        "s-old",
        "t1",
        "assistant",
        &said("sunny all day"),
        1,
    );
    // d58-3 T2: the answer was taken back as wrong.
    h.mark(said_at(2), "s-old", "t1", "withdrawn", "{}");
    close(&mut h, "s-old");
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(line.contains(OPEN), "a withdrawn answer is none: {line}");
    assert!(!line.contains(ANSWERED), "{line}");
}

#[test]
fn gh1654_an_errand_without_an_answer_ends_open() {
    if !shipped() {
        return;
    }
    for last in ["delegation", "outcome"] {
        let mut h = hive();
        asked(&mut h);
        h.row(
            said_at(1),
            "s-old",
            "t1",
            "delegation",
            &said("the weather service"),
            1,
        );
        if last == "outcome" {
            h.row(
                said_at(2),
                "s-old",
                "t1",
                "outcome",
                &said("sunny, 21 degrees"),
                1,
            );
        }
        close(&mut h, "s-old");
        let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
        assert!(line.contains(ERRAND_OPEN), "{last} last: {line}");
        assert!(!line.contains(ANSWERED), "{last} last: {line}");
    }
}

#[test]
fn gh1654_a_close_of_another_round_gives_no_line() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    close_under(&mut h, "s-x", FOREIGN, "foreign_close");
    let msgs = seed(&mut h, "start");
    assert!(
        last_line(&msgs).is_none(),
        "a close the round may not see: {msgs:#?}"
    );
}

#[test]
fn gh1654_a_close_of_another_round_leaves_the_last_one_it_may_see() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    // A newer close, of a round the new call may not see.
    close_under(&mut h, "s-x", FOREIGN, "foreign_close");
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(line.contains("call_ended"), "the close it may see: {line}");
    assert!(!line.contains("foreign_close"), "{line}");
}

#[test]
fn gh1654_a_note_over_its_share_is_left_out_whole() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[
        ("history", "now_utc", json!(NOW)),
        ("history", "seed_last_note_share", json!(0.0001)),
    ]);
    closed_turn(&mut h, "s-old", "s-old#0", &json!({}));
    close(&mut h, "s-old");
    sow_note(&h, "s-old", ROUND_E);
    let (_, line) = last_line(&seed(&mut h, "start")).expect("a line");
    assert!(
        !line.contains("handover note"),
        "no part of the note: {line}"
    );
    assert!(
        line.contains("call_ended"),
        "the rest of the line stands: {line}"
    );
    assert!(
        h.stderr.iter().any(|e| e.contains("seed_last_note_share")),
        "said on stderr: {:?}",
        h.stderr
    );
}

mod handover_prompt {
    //! (2) The handover note names what was agreed -- a phone appointment
    //! (a call back, its time) included -- and whether something stayed open.
    //! No model: the constant is read from the shipped script with `ast`.
    use super::curator_hive::{read_json, repo, run_python};
    use meclaw_core::serde_json::{self as sj, Value, json};

    const HANDOVER: &str = "templates/curator/handover/config.json";
    const AST_LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
scope = {}
for n in ast.parse(inp["src"]).body:
    if isinstance(n, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == "HANDOVER_PROMPT" for t in n.targets):
        exec(compile(ast.Module(body=[n], type_ignores=[]), "handover", "exec"), scope)
print(json.dumps({"prompt": scope.get("HANDOVER_PROMPT")}))
"#;

    fn prompt() -> String {
        let cfg = read_json(&repo(HANDOVER));
        let src = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline");
        let out = run_python(AST_LOADER, &json!({ "src": src }).to_string());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got: Value = sj::from_slice(&out.stdout).expect("JSON");
        got["prompt"]
            .as_str()
            .expect("HANDOVER_PROMPT")
            .to_lowercase()
    }

    #[test]
    fn gh1654_the_note_names_what_was_agreed_and_what_stayed_open() {
        if !repo(HANDOVER).is_file() {
            return;
        }
        let p = prompt();
        for pin in ["what was agreed", "call back", "its time", "stayed open"] {
            assert!(p.contains(pin), "the prompt must say {pin:?}: {p}");
        }
    }
}
