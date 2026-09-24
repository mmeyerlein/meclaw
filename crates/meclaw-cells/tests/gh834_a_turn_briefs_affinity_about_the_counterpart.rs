//! GH #834, lock 1 -- a turn on a channel with many counterparts briefs affinity
//! about the counterpart, and the brief is a LEG of the turn.
//!
//! Measured before this lock (collector@4.2.1): in three turns on a peer channel
//! zero messages reached the member's `affinity`. Nothing in the collector asked:
//! no knob, no emission, no lane. The one door into the brain that carries durable
//! slots (`in_pack`) refuses `peer`/`channel` as `slot_unknown` (gh458), so the
//! counterpart's permission and persona had no way into a turn at all.
//!
//! The brief is built the way the memory leg is (#535/#278): asked ONCE, at the
//! turn's opening, beside `recall`; waited for by the fan-in because the
//! configuration says so (`EXPECT`), never because something happened to arrive;
//! and handed to the brain as a synthetic tool pair `affinity_brief` in
//! `messages[]` -- never as `system.*`, which is durable state and would keep one
//! counterpart's brief in the prompt of the next one.
//!
//! What is pinned here, over the SHIPPED `script_inline` on stdin:
//!
//! 1. with the knob and a `counterpart` the turn raises `brief` (subject = the
//!    counterpart, the turn on the hop) and the fan-in waits for `leg-brief`;
//! 2. without a `counterpart` nothing leaves and the leg is parked EMPTY in the
//!    turn-open bundle, so no turn waits for an answer nobody was asked for;
//! 3. without the knob there is no leg at all -- the shipped default;
//! 4. `in_briefing` parks the answer, and the pair reaches the prompt without the
//!    `system` of the answer;
//! 5. an `error` is an empty leg and the turn still opens;
//! 6. the call id is derived from the turn and the subject, never drawn.
//!
//! The colony half -- the member stamps, the answer finds the generation that
//! asked, nothing leaks to the level above -- is
//! `gh834_the_member_stamps_the_brief_and_the_answer_finds_the_generation.rs`.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "peer-channel#0001";
const COUNTERPART: &str = "peer:north";
const ROOM: &str = "peer-channel:north";

fn knob() -> Vec<(&'static str, Value)> {
    vec![("brief_slots", json!(["peer", "channel"]))]
}

/// A turn as the talky's session keeper hands it to the collector, on a channel
/// whose entry edge stamped the counterpart (or did not).
fn turn(counterpart: Option<&str>) -> Value {
    let mut ctx = json!({
        "turn_id": TURN,
        "channel": ROOM,
        "audience_set": "[\"agent:alpha\",\"peer:north\"]",
        "assistant": "alpha"
    });
    if let Some(c) = counterpart {
        ctx["counterpart"] = json!(c);
    }
    lane(
        "in_turn",
        json!({"turn_id": TURN}),
        ctx,
        json!([{"origin": "user", "type": "text", "text": "hello from the north"}]),
    )
}

fn briefs(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("brief"))
        .collect()
}

/// The `leg-brief` rows a turn-open bundle writes itself -- the empty leg.
fn parked_brief_rows(out: &[Value]) -> Vec<Value> {
    let open = in_phase(out, "turn-open");
    calls_of(open)
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "round")
        .map(|(_, a)| a["row"].clone())
        .filter(|row| row["role"] == "leg-brief")
        .collect()
}

fn leg_window_row() -> Value {
    let payload = json!({"turns": [{"role": "user", "text": "hello from the north",
                                    "consult_id": ""}],
                         "bytes": 20, "dropped": 0, "capped": 0, "deferred": 0});
    json!({"turn_id": TURN, "iter": 0, "role": "leg-window",
           "turn": payload.to_string(), "fired": 0})
}

/// The collect phase over the rows the round table read back.
fn collect(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    let reply = bundle_reply("collect", TURN, &[("c-collect-read", rows)]);
    assemble(over, reply)
}

fn seam(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        let r = m["header"]["route"].as_str();
        r == Some("brain") || r == Some("answer")
    })
}

/// What `affinity/brief` answers a tool-lane brief with (brief script 187-193):
/// the receipt line, the pack below it, and the same pack as `system`.
fn affinity_answer(call_id: &str) -> Value {
    let pack = json!({"peer": {"subject": COUNTERPART, "trust_level": "known",
                               "names": {"first": "North"},
                               "text": "affinity brief on North (peer) -- peer\nnames.first: North"},
                      "channel": {"channel": ROOM,
                                  "text": "affinity brief on North (peer) -- channel"}});
    let text = format!(
        "affinity brief on North (peer) for agent:alpha: slots channel, peer, trust known, \
         0 relation(s)\n{}",
        pack
    );
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
                         "text": text}],
           "system": pack})
}

/// The answer arriving back on `in_briefing`, as the member's edge B restamps it:
/// the turn in context (#535), affinity's own hop keys, and the outcome.
fn in_briefing(outcome: &str, body: Value) -> Value {
    let mut doc = lane(
        "in_briefing",
        json!({"brief_outcome": outcome, "subject": COUNTERPART,
               "slots": "[\"peer\",\"channel\"]", "subscriber": ""}),
        json!({"turn_id": TURN, "counterpart": COUNTERPART}),
        body["messages"].clone(),
    );
    if let Some(sys) = body.get("system") {
        doc["system"] = sys.clone();
    }
    doc
}

/// The leg row an `in_briefing` arrival parks, out of its own emission.
fn leg_brief_row_of(out: &[Value]) -> Value {
    let park = in_phase(out, "collect");
    calls_of(park)
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|row| row["role"] == "leg-brief")
        .unwrap_or_else(|| panic!("in_briefing parked no leg-brief row: {out:?}"))
}

fn the_call(out: &[Value]) -> Value {
    let b = briefs(out);
    assert_eq!(b.len(), 1, "exactly one brief per turn: {out:?}");
    b[0]["messages"][0].clone()
}

// ══════════════════════════════════════════════════════ 1. knob + counterpart

#[test]
fn with_the_knob_and_a_counterpart_the_turn_expects_a_brief_leg() {
    let out = assemble(&knob(), turn(Some(COUNTERPART)));
    let b = briefs(&out);
    assert_eq!(b.len(), 1, "the turn raises exactly one brief: {out:?}");
    let brief = b[0];
    assert_eq!(
        hop_str(brief, "turn_id"),
        TURN,
        "the turn rides on the hop, where the member's edge promotes it from (#535)"
    );
    let call = &brief["messages"][0];
    assert_eq!(call["type"], "tool_call", "{brief}");
    let args: Value = serde_json::from_str(call["text"].as_str().expect("call text"))
        .expect("the request is json");
    assert_eq!(
        args["subject"], COUNTERPART,
        "subject = the counterpart: {args}"
    );
    assert_eq!(args["slots"], json!(["peer", "channel"]), "{args}");
    assert_eq!(args["channel"], ROOM, "the room the turn is in: {args}");
    assert!(
        call["id"].as_str().expect("id").starts_with("call_brief_"),
        "the call carries an id the answer comes back under: {call}"
    );
    assert!(
        parked_brief_rows(&out).is_empty(),
        "a brief that LEFT is not also parked empty -- the fan-in would complete \
         without its answer"
    );

    // The fan-in waits for the leg: the window alone does not open the turn.
    let only_window = collect(&knob(), json!([leg_window_row()]));
    assert!(
        seam(&only_window).is_none(),
        "with the knob set the turn waits for leg-brief: {only_window:?}"
    );
}

// ═══════════════════════════════════════════════════ 2. no counterpart, no wait

#[test]
fn without_a_counterpart_no_brief_leaves_and_the_leg_is_parked_empty() {
    let out = assemble(&knob(), turn(None));
    assert!(
        briefs(&out).is_empty(),
        "no counterpart, no brief -- there is no fallback subject (OR-AG-10): {out:?}"
    );
    let parked = parked_brief_rows(&out);
    assert_eq!(
        parked.len(),
        1,
        "the leg is parked EMPTY in the turn-open bundle, so the fan-in completes: {out:?}"
    );
    assert_eq!(parked[0]["turn_id"], TURN);

    let rows = json!([leg_window_row(), parked[0].clone()]);
    let opened = collect(&knob(), rows);
    let msg = seam(&opened).unwrap_or_else(|| panic!("the turn opens: {opened:?}"));
    let text = msg.to_string();
    assert!(
        !text.contains("affinity_brief"),
        "an empty leg renders nothing: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════ 3. no knob, no leg

#[test]
fn without_the_knob_there_is_no_leg() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(
        cfg["params"]["brief_slots"],
        json!([]),
        "the shipped default is the leg switched off"
    );
    let out = assemble(&[], turn(Some(COUNTERPART)));
    assert!(briefs(&out).is_empty(), "no knob, no brief: {out:?}");
    assert!(
        parked_brief_rows(&out).is_empty(),
        "and no empty leg either: {out:?}"
    );
    let opened = collect(&[], json!([leg_window_row()]));
    assert!(
        seam(&opened).is_some(),
        "the window alone opens the turn, as before 4.3.0: {opened:?}"
    );
}

// ════════════════════════════════════════════ 4. the answer is a pair, no system

#[test]
fn in_briefing_parks_the_leg_and_the_pair_reaches_the_prompt_without_system() {
    let call = the_call(&assemble(&knob(), turn(Some(COUNTERPART))));
    let id = call["id"].as_str().expect("id").to_string();

    let parked = assemble(&knob(), in_briefing("answer", affinity_answer(&id)));
    let row = leg_brief_row_of(&parked);
    assert_eq!(
        row["turn_id"], TURN,
        "the leg is filed under the turn (#535)"
    );
    let leg: Value = serde_json::from_str(row["turn"].as_str().expect("turn json")).expect("json");
    assert!(
        leg.get("system").is_none(),
        "the system of the answer is dropped at the lane, not carried: {leg}"
    );

    let opened = collect(&knob(), json!([leg_window_row(), row]));
    let msg = seam(&opened).unwrap_or_else(|| panic!("the turn opens: {opened:?}"));
    let msgs = msg["messages"].as_array().expect("messages");
    let (ci, c) = msgs
        .iter()
        .enumerate()
        .find(|(_, m)| {
            m["type"] == "tool_call"
                && m["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("\"affinity_brief\""))
        })
        .unwrap_or_else(|| panic!("no affinity_brief call in the prompt: {msg}"));
    assert_eq!(
        c["id"],
        id.as_str(),
        "the pair answers the call that left: {msg}"
    );
    let fun: Value = serde_json::from_str(c["text"].as_str().unwrap()).expect("json");
    assert_eq!(fun["name"], "affinity_brief");
    let args: Value = serde_json::from_str(fun["arguments"].as_str().expect("arguments"))
        .expect("arguments json");
    assert_eq!(args["subject"], COUNTERPART, "{args}");
    assert_eq!(args["slots"], json!(["peer", "channel"]), "{args}");
    let r = &msgs[ci + 1];
    assert_eq!(
        r["type"], "tool_result",
        "the result follows its call: {msg}"
    );
    assert_eq!(r["id"], id.as_str());
    let rt = r["text"].as_str().expect("result text");
    assert!(
        rt.starts_with("affinity brief on North (peer)"),
        "the receipt line heads the result: {rt}"
    );
    assert!(
        rt.contains("\"trust_level\": \"known\"") || rt.contains("\"trust_level\":\"known\""),
        "the peer slot rides in the result: {rt}"
    );
    let sys = msg.get("system").cloned().unwrap_or(json!({}));
    assert!(
        sys.get("peer").is_none() && sys.get("channel").is_none(),
        "no brief slot lands in system.* -- the pair is evidence of THIS turn: {sys}"
    );
}

// ═════════════════════════════════════════════ 5. an error is an empty leg

#[test]
fn an_error_answer_is_an_empty_leg_and_the_turn_still_opens() {
    let error = json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                                     "text": "affinity/brief: store rejected op select \
                                              (unknown_table)"}]});
    let parked = assemble(&knob(), in_briefing("error", error));
    let row = leg_brief_row_of(&parked);
    let opened = collect(&knob(), json!([leg_window_row(), row]));
    let msg = seam(&opened).unwrap_or_else(|| panic!("the turn still opens: {opened:?}"));
    assert!(
        !msg.to_string().contains("affinity_brief"),
        "an error renders no pair: {msg}"
    );
}

// ═══════════════════════════════════════════════ 6. a derived id, never drawn

#[test]
fn the_call_id_is_the_same_for_the_same_turn_and_subject() {
    let once = the_call(&assemble(&knob(), turn(Some(COUNTERPART))));
    let twice = the_call(&assemble(&knob(), turn(Some(COUNTERPART))));
    assert_eq!(
        once["id"], twice["id"],
        "a re-assembly asks the same question"
    );
    let other = the_call(&assemble(&knob(), turn(Some("peer:south"))));
    assert_ne!(once["id"], other["id"], "another subject is another call");
}

/// The pair answers the call that LEFT, read off the answer rather than rebuilt
/// from its echo. affinity answers a tool-lane brief under `req["call_id"]`, i.e.
/// `messages[0].id` of the request (brief script, `answer()`); the subject it
/// echoes on the hop is its own reading of the request, and a reading may be
/// normalised. So the parked leg takes the id the answer carries, and derives one
/// only when the answer carries none -- affinity's store-error path answers with
/// `id: ""`, and an error is an empty leg that renders no pair anyway.
#[test]
fn the_pair_carries_the_id_the_answer_came_back_under() {
    let call = the_call(&assemble(&knob(), turn(Some(COUNTERPART))));
    let id = call["id"].as_str().expect("id").to_string();
    // The same answer, but the echo spells the subject another way.
    let mut back = in_briefing("answer", affinity_answer(&id));
    back["header"]["hop"]["subject"] = json!("peer:North");
    let row = leg_brief_row_of(&assemble(&knob(), back));
    let leg: Value = serde_json::from_str(row["turn"].as_str().expect("turn json")).expect("json");
    assert_eq!(
        leg["id"],
        id.as_str(),
        "the leg is filed under the id the request left with, not one rebuilt from \
         the echo: {leg}"
    );
}

/// A brief longer than `tool_chars` is cut where every tool result is cut, and
/// the cut is SAID: a pack with many relations would otherwise end mid-JSON and
/// neither the model nor a reader of the prompt could tell a cut from an end. The
/// marker is the house form of a visible cut (`code/cell.rs`, `llm/wire.rs`,
/// `web_fetch.rs`: `… [truncated, N bytes total]`), counted in characters here
/// because the knob is. The full text stays in the round table.
#[test]
fn a_brief_over_the_cap_is_cut_visibly() {
    let call = the_call(&assemble(&knob(), turn(Some(COUNTERPART))));
    let id = call["id"].as_str().expect("id").to_string();
    let long = format!(
        "affinity brief on North (peer) for agent:alpha: slots peer\n{}",
        "x".repeat(300)
    );
    let total = long.chars().count();
    let body = json!({"messages": [{"origin": "tool", "type": "tool_result", "id": id,
                                    "text": long}]});
    let over = [
        ("brief_slots", json!(["peer", "channel"])),
        ("tool_chars", json!(120)),
    ];
    let row = leg_brief_row_of(&assemble(&over, in_briefing("answer", body)));
    let opened = collect(&over, json!([leg_window_row(), row]));
    let msg = seam(&opened).unwrap_or_else(|| panic!("the turn opens: {opened:?}"));
    let rt = msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["type"] == "tool_result" && m["id"] == id.as_str())
        .and_then(|m| m["text"].as_str())
        .unwrap_or_else(|| panic!("no brief result in the prompt: {msg}"))
        .to_string();
    assert!(
        rt.ends_with(&format!("… [truncated, {total} chars total]")),
        "the cut is marked with the full length: {rt:?}"
    );
    assert!(
        rt.starts_with("affinity brief on North (peer)"),
        "the head of the brief survives the cut: {rt:?}"
    );
    let kept = rt.split('…').next().unwrap_or_default().chars().count();
    assert_eq!(kept, 120, "the kept part is exactly tool_chars: {rt:?}");
}

// ═══════════════════════════════════════════ 7. the knob and the road, together

/// The w13 triplet for the new knob, stated where the knob is: a param, a
/// `contract.settings` entry and the script's own literal, with one value.
#[test]
fn param_setting_and_literal_agree() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["brief_slots"], json!([]));
    let s = &cfg["contract"]["settings"]["brief_slots"];
    assert_eq!(s["default"], json!([]));
    assert_eq!(s["type"], "array");
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_list(\"brief_slots\", [])"),
        "the script's own fallback is the shipped default"
    );
}

/// THE HANGING TRAP, pinned shut from the knob's side. The brief leg has no
/// deadline (OR-AG-12): a surface whose knob is on waits for its brief. So the
/// knob is set on BOTH surface ref markers of the assistant -- the recipe draws
/// the brief road for both -- and to the same slots, because a typed channel
/// that briefed differently from the spoken one would be a different assistant
/// to the same counterpart. The #728 pattern (`gh728_the_deadline_is_one_knob.rs`).
#[test]
fn both_surfaces_of_the_assistant_set_brief_slots() {
    for rel in [TALKY_REF, TALKY_CHAT_REF] {
        let v = &config_of(rel)["override_params"]["collector/assemble"]["brief_slots"];
        assert_eq!(
            *v,
            json!(["peer", "channel"]),
            "{rel}: brief_slots is not set on this surface"
        );
    }
    let asst = config_of(ASSISTANT);
    let c = &asst["params"]["contract"];
    for (list, route) in [("emits", "brief"), ("accepts", "in_briefing")] {
        let lane = c[list]
            .as_array()
            .expect(list)
            .iter()
            .find(|l| l["route"] == route)
            .unwrap_or_else(|| panic!("the assistant declares no `{route}`"));
        assert_eq!(
            lane["at"],
            json!(["./talky", "./talky-chat"]),
            "`{route}` docks at both surfaces and only there -- the core never asks"
        );
    }
}
