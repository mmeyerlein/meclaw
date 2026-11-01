//! #1279, the read-back side -- a how-question the person answered
//! comes back on that person's next turns.
//!
//! WHY this file exists: the curator proposes every clarify pair (the
//! assistant's how-question, the person's answer, the class the next stamp
//! resolved to) to the affinity as `mx.hints.clarify` (#1279), and
//! nothing read it back -- the affinity brief had no slot for it, so the stamp
//! of the person's next ambiguous turn guessed from scratch again. Now:
//!
//! 1. `affinity/brief` serves the slot `hints` (asked for, never by default):
//!    the accepted `mx.hints.clarify` proposals about the subject, newest
//!    first, each usable only in a round that is a subset of the round it was
//!    made in (R-AF-3 on `origin_round`), with no disclosure row needed -- the
//!    proposal is the agent's own observation in that round, not a release;
//! 2. `collector/assemble` with `brief_hints` "1" asks for it on every turn:
//!    about the counterpart when the turn names one, else about the one person
//!    of the round (the same rule the curator proposes under), and carries
//!    what came back as the `curate` slot `hints`;
//! 3. `talky/route` renders `judge.hints` -- which the curator fills from that
//!    slot -- in the decide state, as facts;
//! 4. shipped, nothing of it moves.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const BRIEF: &str = "templates/affinity/brief/config.json";
const ROUTE: &str = "templates/talky/route/config.json";

const PERSON: &str = "member:e";
const ASKER: &str = "agent:alpha";
const TURN: &str = "tg:1#0007";

fn round() -> Vec<&'static str> {
    vec![ASKER, PERSON]
}

// ═══════════════════════════════════════════════════════ 1. affinity/brief

fn door(slots: Value, round: &[&str]) -> Value {
    json!({
        "header": {"hop": {"route": "in_brief"},
                   "context": {"asker": ASKER, "channel_node": "telegram",
                               "audience_set": json!(round).to_string(), "aff_phase": "",
                               "aff_carry": "", "aff_subscriber": ""}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-hints",
                      "text": json!({"subject": PERSON, "slots": slots}).to_string()}]
    })
}

fn op_of(m: &Value) -> Value {
    serde_json::from_str(m["messages"][0]["text"].as_str().unwrap()).unwrap()
}

fn store_reply(asked: &Value, rows: Value) -> Value {
    let h = &asked["header"];
    json!({
        "header": {
            "hop": {"operation": op_of(asked)["operation"]},
            "context": {
                "affinity_origin": "brief",
                "aff_phase": h["phase"], "aff_subject": h["subject"],
                "aff_audience": h["audience"], "aff_channel": h["channel"],
                "aff_slots": h["slots"], "aff_carry": h["carry"],
                "aff_subscriber": h["subscriber"]
            }
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "s1",
                      "text": rows.to_string()}]
    })
}

fn proposal(question: &str, answer: &str, class: &str, origin: &[&str], at: &str) -> Value {
    json!({"id": format!("clarify:{question}"), "entity_ref": PERSON,
           "field_path": "mx.hints.clarify", "status": "accepted",
           "origin_round": json!(origin).to_string(), "recorded_at": at,
           "value": json!({"question": question, "answer": answer, "class": class,
                           "session_id": "s1", "asked_turn": "t1",
                           "answered_turn": "t2"}).to_string()})
}

/// The store: no entity row, a known trust, nothing disclosed -- and the
/// proposals the hints read finds, newest first as asked.
fn answer_of(op: &Value) -> Value {
    match (
        op["operation"].as_str().unwrap_or_default(),
        op["table"].as_str().unwrap_or_default(),
    ) {
        ("select", "entities") => json!([]),
        ("select", "trust") => json!([{"level": "known", "decided_at": "2026-10-01T00:00:00Z"}]),
        ("select", "disclosure") => json!([]),
        ("select", "proposals") => json!([
            proposal(
                "Its concerts or its website?",
                "its concerts",
                "simple",
                &[ASKER, PERSON],
                "2026-10-29T10:00:00Z"
            ),
            // made in front of another agent: this round is no subset of it
            proposal(
                "Which choir?",
                "the senior choir",
                "research",
                &["agent:beta", PERSON],
                "2026-10-28T10:00:00Z"
            ),
            proposal(
                "Today or tomorrow?",
                "tomorrow",
                "simple",
                &[ASKER, PERSON, "member:f"],
                "2026-10-27T10:00:00Z"
            ),
        ]),
        other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
    }
}

/// One brief, run to its answer; also every store op it asked.
fn brief(slots: Value) -> (Value, Vec<Value>) {
    brief_in(slots, &round())
}

/// [`brief`] in `round`.
fn brief_in(slots: Value, round: &[&str]) -> (Value, Vec<Value>) {
    let mut doc = door(slots, round);
    let mut asked = Vec::new();
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, &[], doc.clone());
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return (ans.clone(), asked);
        }
        let next: Vec<&Value> = out
            .iter()
            .filter(|m| {
                m["header"]["route"] == "astore" && m["header"]["phase"].as_str() != Some("audit")
            })
            .collect();
        assert_eq!(next.len(), 1, "one store read per phase: {out:?}");
        let op = op_of(next[0]);
        asked.push(op.clone());
        doc = store_reply(next[0], answer_of(&op));
    }
    panic!("the lane never answered");
}

#[test]
fn the_brief_serves_the_persons_hints_by_their_round() {
    let (ans, asked) = brief(json!(["hints"]));
    let read = asked
        .iter()
        .find(|op| op["table"] == "proposals")
        .unwrap_or_else(|| panic!("no proposals read: {asked:?}"));
    assert_eq!(read["operation"], "select");
    assert_eq!(
        read["where"],
        json!({"entity_ref": PERSON, "field_path": "mx.hints.clarify", "status": "accepted"}),
        "{read}"
    );
    assert_eq!(
        read["order_by"],
        json!([{"col": "recorded_at", "dir": "desc"}])
    );
    assert!(
        asked.iter().all(
            |op| op["table"] != "disclosure" && op["table"] != "entities"
                || op["columns"] == json!(["entity_id", "display_name", "status"])
        ),
        "a hints-only brief reads no disclosure and no document: {asked:?}"
    );
    assert_eq!(
        ans["hints"],
        json!([
            {"question": "Its concerts or its website?", "answer": "its concerts",
             "class": "simple"},
            {"question": "Today or tomorrow?", "answer": "tomorrow", "class": "simple"}
        ]),
        "newest first, only what this round may use (R-AF-3 on origin_round): {ans}"
    );
    let text = ans["messages"][0]["text"]
        .as_str()
        .expect("tool_result text");
    assert!(
        text.contains("its concerts"),
        "the receipt carries the pack: {text}"
    );
    assert_eq!(
        ans["messages"][0]["id"], "call-hints",
        "it answers the call"
    );
}

/// The asker is in the round it answers even when the declared set leaves it
/// out, as in the three sibling checks: a pair made in front of another agent
/// stays out of a round that names the person alone.
#[test]
fn the_hints_count_the_asker_into_the_round() {
    let (ans, _) = brief_in(json!(["hints"]), &[PERSON]);
    assert_eq!(
        ans["hints"],
        json!([
            {"question": "Its concerts or its website?", "answer": "its concerts",
             "class": "simple"},
            {"question": "Today or tomorrow?", "answer": "tomorrow", "class": "simple"}
        ]),
        "{ans}"
    );
}

#[test]
fn a_brief_without_the_hints_slot_reads_no_proposal() {
    let (ans, asked) = brief(json!(["peer", "channel"]));
    assert!(
        asked.iter().all(|op| op["table"] != "proposals"),
        "no slot, no read: {asked:?}"
    );
    assert!(ans.get("hints").is_none(), "no slot, no field: {ans}");
    let cfg = config_of(BRIEF);
    assert_eq!(cfg["params"]["hint_rows"], json!(20));
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("DEFAULT_SLOTS = (\"identity\", \"peer\", \"relationship\", \"channel\")"),
        "a request naming no slots never gets `hints`"
    );
}

// ═════════════════════════════════════════════════ 2. collector/assemble

fn knob() -> Vec<(&'static str, Value)> {
    vec![("brief_hints", json!("1"))]
}

fn turn(round: &[&str], counterpart: Option<&str>) -> Value {
    let mut ctx = json!({"turn_id": TURN, "channel": "tg:1",
                         "audience_set": json!(round).to_string(), "assistant": "alpha"});
    if let Some(c) = counterpart {
        ctx["counterpart"] = json!(c);
    }
    lane(
        "in_turn",
        json!({"turn_id": TURN}),
        ctx,
        json!([{"origin": "user", "type": "text", "text": "the choir, search please"}]),
    )
}

fn briefs(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("brief"))
        .collect()
}

fn args_of(brief: &Value) -> Value {
    serde_json::from_str(brief["messages"][0]["text"].as_str().expect("call text")).expect("json")
}

fn parked_brief_rows(out: &[Value]) -> Vec<Value> {
    calls_of(in_phase(out, "turn-open"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "round")
        .map(|(_, a)| a["row"].clone())
        .filter(|row| row["role"] == "leg-brief")
        .collect()
}

fn leg_window_row() -> Value {
    let payload = json!({"turns": [{"role": "user", "text": "the choir, search please",
                                    "consult_id": ""}],
                         "deferred": 0, "deferred_turns": []});
    json!({"turn_id": TURN, "iter": 0, "role": "leg-window",
           "turn": payload.to_string(), "fired": 0})
}

fn hints() -> Value {
    json!([{"question": "Its concerts or its website?", "answer": "its concerts",
            "class": "simple"}])
}

fn in_briefing(call_id: &str, slots: &str, body_hints: Option<Value>) -> Value {
    in_briefing_about(PERSON, call_id, slots, body_hints)
}

fn in_briefing_about(
    subject: &str,
    call_id: &str,
    slots: &str,
    body_hints: Option<Value>,
) -> Value {
    let mut doc = lane(
        "in_briefing",
        json!({"brief_outcome": "answer", "subject": subject, "slots": slots,
               "subscriber": ""}),
        json!({"turn_id": TURN}),
        json!([{"origin": "tool", "type": "tool_result", "id": call_id,
                "text": "affinity hints on member:e for agent:alpha: 1 hint(s)\n{}"}]),
    );
    if let Some(h) = body_hints {
        doc["hints"] = h;
    }
    doc
}

fn leg_brief_row_of(out: &[Value]) -> Value {
    calls_of(in_phase(out, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|row| row["role"] == "leg-brief")
        .unwrap_or_else(|| panic!("in_briefing parked no leg-brief row: {out:?}"))
}

fn curate_of(over: &[(&str, Value)], rows: Value) -> Value {
    let out = assemble(
        over,
        bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    );
    out.into_iter()
        .find(|m| m["header"]["route"] == "curate")
        .expect("the turn opens on curate")
}

#[test]
fn with_brief_hints_a_turn_asks_for_the_persons_hints() {
    let out = assemble(&knob(), turn(&round(), None));
    let b = briefs(&out);
    assert_eq!(b.len(), 1, "one brief per turn: {out:?}");
    let args = args_of(b[0]);
    assert_eq!(
        args["subject"], PERSON,
        "the one person of the round: {args}"
    );
    assert_eq!(args["slots"], json!(["hints"]), "{args}");
    assert!(
        parked_brief_rows(&out).is_empty(),
        "a brief that left is not parked"
    );

    // A counterpart is the subject, and the hints ride beside its slots.
    let over = [
        ("brief_hints", json!("1")),
        ("brief_slots", json!(["peer"])),
    ];
    let args = args_of(
        briefs(&assemble(
            &over,
            turn(&[ASKER, "peer:north"], Some("peer:north")),
        ))[0],
    );
    assert_eq!(args["subject"], "peer:north");
    assert_eq!(args["slots"], json!(["peer", "hints"]));

    // Two people and no counterpart: nobody to ask about, the leg parks empty.
    let out = assemble(&knob(), turn(&[ASKER, PERSON, "member:f"], None));
    assert!(briefs(&out).is_empty(), "{out:?}");
    assert_eq!(parked_brief_rows(&out).len(), 1, "{out:?}");
}

#[test]
fn the_hints_come_back_as_a_slot_of_the_curate() {
    let out = assemble(&knob(), turn(&round(), None));
    let id = briefs(&out)[0]["messages"][0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let row = leg_brief_row_of(&assemble(
        &knob(),
        in_briefing(&id, "[\"hints\"]", Some(hints())),
    ));
    let msg = curate_of(&knob(), json!([leg_window_row(), row]));
    assert_eq!(msg["hints"], hints(), "{msg}");
    assert_eq!(msg["hints_of"], PERSON, "whom the brief was about: {msg}");
    assert!(
        !msg.to_string().contains("affinity_brief"),
        "a hints-only brief is no evidence for the model: {msg}"
    );

    // A brief that answered without the field (a refusal) carries no slot.
    let row = leg_brief_row_of(&assemble(&knob(), in_briefing(&id, "[\"hints\"]", None)));
    let msg = curate_of(&knob(), json!([leg_window_row(), row]));
    assert!(msg.get("hints").is_none(), "{msg}");
    assert!(msg.get("hints_of").is_none(), "{msg}");

    // A counterpart's hints name the counterpart: the curator keeps them only
    // for the person of the round they are about.
    let row = leg_brief_row_of(&assemble(
        &knob(),
        in_briefing_about("peer:north", &id, "[\"hints\"]", Some(hints())),
    ));
    let msg = curate_of(&knob(), json!([leg_window_row(), row]));
    assert_eq!(msg["hints_of"], "peer:north", "{msg}");
}

#[test]
fn brief_hints_ships_off_and_changes_nothing() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["brief_hints"], json!("0"));
    assert_eq!(
        cfg["contract"]["settings"]["brief_hints"]["default"],
        json!("0")
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"brief_hints\", \"0\")"),
        "the script's own fallback"
    );
    // Shipped, a turn without a counterpart asks nothing and waits for nothing,
    // and its curate is the bytes it was.
    let out = assemble(&[], turn(&round(), None));
    assert!(briefs(&out).is_empty(), "{out:?}");
    assert!(parked_brief_rows(&out).is_empty(), "{out:?}");
    let shipped = curate_of(&[], json!([leg_window_row()]));
    let off = curate_of(&[("brief_hints", json!("0"))], json!([leg_window_row()]));
    assert_eq!(shipped, off);
    assert!(shipped.get("hints").is_none(), "{shipped}");
    // A parked empty brief leg keeps the slots it always had.
    let out = assemble(&[("brief_slots", json!(["peer"]))], turn(&round(), None));
    let rows = parked_brief_rows(&out);
    assert_eq!(rows.len(), 1, "{out:?}");
    let parked: Value =
        serde_json::from_str(rows[0]["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(parked["slots"], json!(["peer"]), "{parked}");
}

// ═══════════════════════════════════════════════════════ 3. talky/route

fn judged_turns(judge: Value) -> Value {
    let mut doc = lane(
        "in_turns",
        json!({"state": "the choir, search please",
               "asked_at": (now_ms() + 3_600_000).to_string()}),
        json!({"turn_id": TURN, "channel": "tg:1", "audience_set": "[\"member:e\"]"}),
        json!([]),
    );
    doc["turns"] = json!([]);
    doc["judge"] = judge;
    doc
}

fn decide_state(judge: Value) -> String {
    let out = run_cell(ROUTE, &[("judge_context", json!("1"))], judged_turns(judge)).0;
    let d = out
        .iter()
        .find(|m| m["header"]["route"] == "decide")
        .unwrap_or_else(|| panic!("no decide: {out:?}"));
    d["decide"]["state"].as_str().expect("state").to_string()
}

#[test]
fn the_route_renders_the_persons_hints() {
    let base = json!({"turns": [], "topic": "", "screen": {"topics": [], "state": []},
                      "now": {"iso": "2026-10-30T09:00:00+01:00", "weekday": "Friday",
                              "timezone": "Europe/Berlin"},
                      "place": ""});
    let mut with = base.clone();
    with["hints"] = hints();
    let state = decide_state(with);
    assert!(
        state.contains("How this person meant earlier requests"),
        "{state}"
    );
    assert!(
        state.contains("- asked \"Its concerts or its website?\", the person answered \"its concerts\" (class: simple)"),
        "{state}"
    );
    assert!(
        state.ends_with("The request now:\nthe choir, search please"),
        "{state}"
    );
    // No hints, no block: the state is the bytes it was.
    let without = decide_state(base);
    assert!(!without.contains("How this person"), "{without}");
}

// ═══════════════════════════════════ 4. the hints in the window's budget (#1363)
//
// WHY: the route rendered every hint the brief served (only `hint_rows` of the
// affinity bounds the count, each answer in full) outside every token budget of
// the decide window. Now the hints count against the window's own budget
// (`context_budget_tokens`, the one the wall cut the turns to) beside the turns
// already in it: the newest hints that fit stay whole, the older ones go whole
// (never half a sentence), and a cut says so with a mark naming what was shown
// and the total, like every other curated block (R-IG-1).

/// Three hints, newest first, each line about 34 tokens.
fn long_hints() -> Value {
    json!([
        {"question": "The newest one: the concerts of the choir or its website, which did you mean?",
         "answer": "the concerts", "class": "simple"},
        {"question": "The middle one: the weather of today or the forecast of the whole week then?",
         "answer": "the forecast", "class": "simple"},
        {"question": "The oldest one: the bakery around the corner or the one next to the station?",
         "answer": "the station", "class": "talk"}
    ])
}

/// The line the route renders for a hint and its tokens by the token-estimate
/// block (UTF-8 bytes over three, rounded up).
fn hint_line(h: &Value) -> (String, usize) {
    let line = format!(
        "- asked \"{}\", the person answered \"{}\" (class: {})",
        h["question"].as_str().unwrap(),
        h["answer"].as_str().unwrap(),
        h["class"].as_str().unwrap()
    );
    let tokens = line.len().div_ceil(3);
    (line, tokens)
}

fn judge_with(turns: Value, hints: Value) -> Value {
    json!({"turns": turns, "topic": "", "screen": {},
           "now": {}, "place": "", "hints": hints})
}

fn decide_state_within(judge: Value, budget: u64) -> String {
    let out = run_cell(
        ROUTE,
        &[
            ("judge_context", json!("1")),
            ("context_budget_tokens", json!(budget)),
        ],
        judged_turns(judge),
    )
    .0;
    let d = out
        .iter()
        .find(|m| m["header"]["route"] == "decide")
        .unwrap_or_else(|| panic!("no decide: {out:?}"));
    d["decide"]["state"].as_str().expect("state").to_string()
}

#[test]
fn the_hints_count_against_the_windows_budget_newest_kept_whole() {
    let hints = long_hints();
    let lines: Vec<(String, usize)> = hints.as_array().unwrap().iter().map(hint_line).collect();
    let total: usize = lines.iter().map(|(_, t)| t).sum();
    // Room for exactly the two newest hints, no turn in the window.
    let budget = (lines[0].1 + lines[1].1 + lines[2].1 / 2) as u64;
    let state = decide_state_within(judge_with(json!([]), hints.clone()), budget);
    assert!(state.contains(&lines[0].0), "{state}");
    assert!(state.contains(&lines[1].0), "{state}");
    assert!(!state.contains("The oldest one"), "no half hint: {state}");
    let mark = format!(
        "...[cut: 2 of 3 hints shown, {} of {} tokens; budget of the window]",
        lines[0].1 + lines[1].1,
        total
    );
    assert!(state.contains(&mark), "{mark}\n{state}");
    assert!(
        state.ends_with("The request now:\nthe choir, search please"),
        "{state}"
    );

    // A turn in the window takes its tokens from the same budget: one hint less.
    let said = "x".repeat(lines[1].1 * 3);
    let turns = json!([{"turn_id": "t1", "role": "user", "text": said}]);
    let state = decide_state_within(judge_with(turns, hints.clone()), budget);
    assert!(state.contains(&lines[0].0), "{state}");
    assert!(!state.contains("The middle one"), "{state}");
    let mark = format!(
        "...[cut: 1 of 3 hints shown, {} of {} tokens; budget of the window]",
        lines[0].1, total
    );
    assert!(state.contains(&mark), "{mark}\n{state}");

    // No room at all: the block says it held hints and showed none.
    let state = decide_state_within(judge_with(json!([]), hints), 1);
    assert!(!state.contains("- asked"), "{state}");
    let mark = format!("...[cut: 0 of 3 hints shown, 0 of {total} tokens; budget of the window]");
    assert!(state.contains(&mark), "{mark}\n{state}");
}

#[test]
fn hints_inside_the_budget_render_as_before() {
    let hints = long_hints();
    let lines: Vec<(String, usize)> = hints.as_array().unwrap().iter().map(hint_line).collect();
    let state = decide_state_within(judge_with(json!([]), hints), 1500);
    let block = format!(
        "How this person meant earlier requests when asked (newest first):\n{}\n{}\n{}\n\nThe request now:",
        lines[0].0, lines[1].0, lines[2].0
    );
    assert!(state.contains(&block), "{state}");
    assert!(!state.contains("[cut:"), "{state}");
}
