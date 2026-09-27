//! GH #864 — a brief carries each persona once.
//!
//! Measured at `affinity@3.6.0`: `answer()` rendered the pack once (every person
//! slot got a `text` of `path: value` lines beside its structure, GH #258) and
//! serialised that SAME rendered object into the tool lane's `tool_result`
//! (GH #242). Every released value therefore stood in the tool result twice --
//! once as a field, once inside the slot's `text` -- and `json.dumps`'s default
//! `ensure_ascii` turned every umlaut into a six-character escape in both copies.
//! With a 1,600-character channel persona and `slots: ["peer", "channel"]` the
//! fixture below measured 4,301 characters (4,471 with a German persona). The
//! collector caps a tool result at `tool_chars` (4,000) and keys are sorted, so
//! the `peer` slot came last and its `trust_rank` (character 4,284) never reached
//! the model. Afterwards: 2,169 characters in both languages, `trust_rank` at
//! 2,152, and the largest persona that still fits grows from 1,449 to 3,431.
//!
//! What is pinned here, through the SHIPPED `brief` script over stdin, one phase
//! per run, with the store's answers handed back the way the hive's internal
//! `./store -> ./brief` edge hands them back (the driver of
//! `gh848_affinity_names_who_is_speaking.rs`, trimmed to what this lock reads):
//!
//! 1. the tool lane carries the persona once;
//! 2. the brief of the fixture stays at or below 2,400 characters and under the
//!    collector's `tool_chars`, read from the collector's own config;
//! 3. the `peer` slot's `trust_rank` sits before that cap;
//! 4. the JSON below the receipt line is `system` with the `text` rendering
//!    removed from every person slot -- no field missing, none added;
//! 5. a German persona travels unescaped;
//! 6. the push lane keeps its rendering (guard: it was right before);
//! 7. an `mx.brain` family keeps its `{"text": ...}` leaves (guard);
//! 8. the README says the tool lane carries no rendering (§ 2d drift lock);
//! 9. the seam: the collector hands the model the whole brief, uncut.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{assemble, bundle_reply, calls_of, config_of, in_phase, lane, repo, run_cell};
use serde_json::{Value, json};

const BRIEF: &str = "templates/affinity/brief/config.json";
const README: &str = "templates/affinity/README.md";
const COLLECTOR: &str = "templates/collector/assemble/config.json";

const SUBJECT: &str = "peer:colA/org1/jonas/-";
const CHANNEL: &str = "peer-friend";
const ASKER: &str = "agent:alpha";
const PERSONA_CHARS: usize = 1600;
/// The fixture measured 2,169 characters after the fix; the lock allows ~10 %.
const BRIEF_CEILING: usize = 2400;

const EN: &str = "You talk to Jonas like an old friend who happens to live in another \
colony. Keep it warm, short and concrete; ask about his week, his garden and the choir. ";
const DE: &str = "Du sprichst mit Jonas wie mit einem alten Freund, der zufällig in einer \
anderen Kolonie wohnt. Bleib herzlich, kurz und konkret; frag nach seiner Woche, dem \
Garten, dem Chor und den Blüten. ";

/// A channel persona of exactly `PERSONA_CHARS` characters.
fn persona(base: &str) -> String {
    base.chars().cycle().take(PERSONA_CHARS).collect()
}

/// The collector's cap on a tool result -- read, never restated.
fn tool_chars() -> usize {
    let v = &config_of(COLLECTOR)["params"]["tool_chars"];
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or_else(|| panic!("{COLLECTOR}: params.tool_chars is not a number: {v}"))
}

// ─────────────────────────────────────────────────────────────── the lane

/// What the hive's door hands `./brief`: a tool call, the asker and the round
/// promoted onto context, the channel node beside them. `subscriber` set = the
/// push lane (only `./push`'s edge sets it).
fn door(call_id: &str, slots: Value, subscriber: &str) -> Value {
    let round = json!([ASKER, SUBJECT]).to_string();
    json!({
        "header": {"hop": {"route": "in_brief"},
                   "context": {"asker": ASKER, "channel_node": CHANNEL,
                               "audience_set": round, "aff_phase": "", "aff_carry": "",
                               "aff_subscriber": subscriber}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": call_id,
                      "text": json!({"subject": SUBJECT, "slots": slots}).to_string()}]
    })
}

fn op_of(m: &Value) -> Value {
    serde_json::from_str(m["messages"][0]["text"].as_str().unwrap()).unwrap()
}

/// The store's answer, the way `./store -> ./brief` delivers it.
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

/// One entity, trust `known`, everything released to everyone.
struct Store {
    entity: Value,
}

impl Store {
    fn answer(&self, op: &Value) -> Value {
        match (
            op["operation"].as_str().unwrap_or_default(),
            op["table"].as_str().unwrap_or_default(),
        ) {
            ("select", "entities") => {
                let cols = op["columns"].as_array().expect("columns");
                let mut row = json!({});
                for c in cols {
                    let c = c.as_str().unwrap();
                    row[c] = self.entity.get(c).cloned().unwrap_or(Value::Null);
                }
                json!([row])
            }
            ("select", "trust") => {
                json!([{"level": "known", "decided_at": "2026-09-25T00:00:00.000000Z"}])
            }
            ("select", "disclosure") => json!([{
                "field_path": "*", "mode": "share", "decided_at": "2026-09-25T00:00:00Z",
                "audience": "*", "audience_set": null}]),
            ("traverse", _) => json!([]),
            other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
        }
    }
}

fn entity(persona: &str, brain: Option<Value>) -> Value {
    let aieos = json!({
        "identity": {"names": {"first": "Jonas", "last": "Berg"}},
        "interests": {"hobbies": ["choir", "gardening", "chess"],
                      "topics": ["local history", "trains"]},
        "psychology": {"values": ["loyalty", "curiosity"]},
        "linguistics": {"text_style": {"formality": "casual", "emoji": "rare"}}
    });
    let mut mx = json!({"peer": {"name": "colA", "url": "https://colony-a.example/peer"},
                        "channel_personas": {CHANNEL: persona}});
    if let Some(b) = brain {
        mx["brain"] = b;
    }
    json!({"entity_id": SUBJECT, "kind": "person", "display_name": "Jonas Berg",
           "status": "active", "aieos": aieos.to_string(), "mx": mx.to_string()})
}

/// One brief, run to its answer.
fn brief(first: Value, store: &Store) -> Value {
    let mut doc = first;
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, &[], doc.clone());
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return ans.clone();
        }
        let next: Vec<&Value> = out
            .iter()
            .filter(|m| {
                m["header"]["route"] == "astore" && m["header"]["phase"].as_str() != Some("audit")
            })
            .collect();
        assert_eq!(next.len(), 1, "one store read per phase: {out:?}");
        doc = store_reply(next[0], store.answer(&op_of(next[0])));
    }
    panic!("the lane never answered");
}

/// The tool-lane brief about the fixture subject with a persona of `base`.
fn tool_brief(base: &str) -> (Value, String) {
    let p = persona(base);
    let ans = brief(
        door("call-864", json!(["peer", "channel"]), ""),
        &Store {
            entity: entity(&p, None),
        },
    );
    (ans, p)
}

fn tool_text(ans: &Value) -> String {
    let msgs = ans["messages"]
        .as_array()
        .expect("the tool lane carries messages");
    assert_eq!(msgs[0]["type"], "tool_result", "{ans}");
    msgs[0]["text"]
        .as_str()
        .expect("tool_result text")
        .to_string()
}

/// The JSON below the receipt line.
fn pack_of(text: &str) -> Value {
    let (_, payload) = text
        .split_once('\n')
        .unwrap_or_else(|| panic!("the receipt line, then the pack: {text}"));
    serde_json::from_str(payload).unwrap_or_else(|e| panic!("the pack is JSON ({e}): {payload}"))
}

/// `system` with the rendering taken off every slot that has one.
fn without_renderings(system: &Value) -> Value {
    let mut v = system.clone();
    for slot in v.as_object_mut().expect("system is an object").values_mut() {
        if let Some(doc) = slot.as_object_mut() {
            doc.remove("text");
        }
    }
    v
}

/// The lock of case 4, used twice (case 4 and the README drift lock).
fn assert_tool_pack_is_system_without_renderings(ans: &Value, text: &str) {
    let pack = pack_of(text);
    let system = &ans["system"];
    assert!(
        system["peer"]["text"].is_string() && system["channel"]["text"].is_string(),
        "every person slot on `system` still carries its rendering: {system}"
    );
    for slot in ["peer", "channel"] {
        assert!(
            pack[slot].get("text").is_none(),
            "the tool lane's `{slot}` slot carries no rendering: {pack}"
        );
    }
    assert_eq!(
        pack,
        without_renderings(system),
        "the tool lane carries the same disclosure decision as `system`, key for key, \
         without the rendering"
    );
}

// ═══════════════════════════════════════════════════════════ 1. once

#[test]
fn the_tool_lane_carries_the_persona_once() {
    let (ans, p) = tool_brief(EN);
    let text = tool_text(&ans);
    assert_eq!(
        text.matches(p.as_str()).count(),
        1,
        "the persona stands in the tool result exactly once (it stood there twice at \
         affinity@3.6.0, once as a field and once in the slot's rendering): {text}"
    );
}

// ═══════════════════════════════════════════════════════════ 2. it fits

#[test]
fn a_brief_with_a_1600_char_persona_fits_under_the_collectors_cap() {
    let cap = tool_chars();
    for base in [EN, DE] {
        let (ans, _) = tool_brief(base);
        let n = tool_text(&ans).chars().count();
        assert!(
            n <= BRIEF_CEILING && n <= cap,
            "the brief with a {PERSONA_CHARS}-character persona is {n} characters; \
             it has to stay at or below {BRIEF_CEILING} and under the collector's \
             tool_chars ({cap}) -- measured 4,301 / 4,471 before GH #864, 2,169 after"
        );
    }
}

// ═══════════════════════════════════════════════════════════ 3. peer whole

#[test]
fn the_peer_slot_reaches_the_model_whole() {
    let cap = tool_chars();
    let (ans, _) = tool_brief(EN);
    let text = tool_text(&ans);
    let at = text
        .find("\"trust_rank\"")
        .unwrap_or_else(|| panic!("the peer slot carries trust_rank: {text}"));
    let at_chars = text[..at].chars().count();
    assert!(
        at_chars < cap,
        "`trust_rank` sits at character {at_chars}, the collector cuts at {cap}: the \
         model would never see it (4,284 before GH #864)"
    );
}

// ═══════════════════════════════════════════════════════════ 4. same decision

#[test]
fn the_tool_lane_pack_is_the_system_pack_without_its_renderings() {
    let (ans, _) = tool_brief(EN);
    let text = tool_text(&ans);
    assert_tool_pack_is_system_without_renderings(&ans, &text);
    let pack = pack_of(&text);
    assert_eq!(pack["peer"]["trust_level"], "known", "{pack}");
    assert_eq!(pack["peer"]["trust_rank"], 2, "{pack}");
    assert_eq!(
        pack["peer"]["address"]["url"], "https://colony-a.example/peer",
        "{pack}"
    );
    assert!(
        ans["who"]["ref"].is_string(),
        "`who` still rides the tool lane (GH #848): {ans}"
    );
}

// ═══════════════════════════════════════════════════════════ 5. umlauts

#[test]
fn an_umlaut_travels_unescaped() {
    let (ans, p) = tool_brief(DE);
    let text = tool_text(&ans);
    assert!(
        text.contains('ü') && !text.contains("\\u00fc"),
        "an umlaut is one character in the tool result, not a six-character escape: {text}"
    );
    assert_eq!(
        text.matches(p.as_str()).count(),
        1,
        "the German persona stands there once, verbatim: {text}"
    );
}

// ═══════════════════════════════════════════════════════════ 6. push lane

#[test]
fn the_push_lane_keeps_its_rendering() {
    let p = persona(EN);
    let ans = brief(
        door("", json!(["peer", "channel"]), "assistants/talky/brain"),
        &Store {
            entity: entity(&p, None),
        },
    );
    assert!(
        ans.get("messages").is_none(),
        "the push lane carries `system` and nothing else (GH #263): {ans}"
    );
    for slot in ["peer", "channel"] {
        let t = ans["system"][slot]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`system.{slot}` keeps its rendering (GH #258): {ans}"));
        assert!(
            t.starts_with(&format!(
                "affinity brief on Jonas Berg (person) -- {slot}\n"
            )),
            "{t}"
        );
    }
    assert!(
        ans["system"]["channel"]["text"]
            .as_str()
            .unwrap()
            .contains(p.as_str()),
        "the persona is in the rendering an `llm` cell reads: {ans}"
    );
}

// ═══════════════════════════════════════════════════════════ 7. brain leaves

#[test]
fn a_brain_family_keeps_its_text_leaves() {
    let brain = json!({"identity": {"soul": "I am the agent."},
                       "instructions": {"reply": "Be brief."}});
    let ans = brief(
        door("call-brain", json!(["brain"]), ""),
        &Store {
            entity: entity(&persona(EN), Some(brain)),
        },
    );
    let want = json!({"identity": {"soul": {"text": "I am the agent."}},
                      "instructions": {"reply": {"text": "Be brief."}}});
    assert_eq!(
        pack_of(&tool_text(&ans)),
        want,
        "an `mx.brain` family is content, not a rendering: its `text` leaves stay (GH #488)"
    );
    assert_eq!(ans["system"], want, "and `system` carries the same leaves");
}

// ═══════════════════════════════════════════════════════════ 8. README drift

#[test]
fn the_readme_says_the_tool_lane_carries_no_rendering() {
    let raw = std::fs::read_to_string(repo(README)).expect("affinity README");
    let prose = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    for sentence in [
        "every slot's structure, key for key, without the `text` rendering the push lane needs",
        "(GH #864)",
    ] {
        assert!(
            prose.contains(sentence),
            "{README} says what the tool lane carries: {sentence:?}"
        );
    }
    // ...and the sentence is true.
    let (ans, _) = tool_brief(EN);
    assert_tool_pack_is_system_without_renderings(&ans, &tool_text(&ans));
}

// ═══════════════════════════════════════════════════════════ 9. the seam

const TURN: &str = "peer-channel#0864";

/// The collector's own turn, on a channel whose entry edge stamped the
/// counterpart; the knob the assistants ship (`brief_slots`).
fn knob() -> Vec<(&'static str, Value)> {
    vec![("brief_slots", json!(["peer", "channel"]))]
}

fn collector_turn() -> Value {
    lane(
        "in_turn",
        json!({"turn_id": TURN}),
        json!({"turn_id": TURN, "channel": CHANNEL,
               "audience_set": json!([ASKER, SUBJECT]).to_string(),
               "assistant": "alpha", "counterpart": SUBJECT}),
        json!([{"origin": "user", "type": "text", "text": "hello from colony A"}]),
    )
}

#[test]
fn the_collector_hands_the_model_the_whole_brief() {
    // 1. The turn opens and asks affinity; the call id is the collector's.
    let opened = assemble(&knob(), collector_turn());
    let call = opened
        .iter()
        .find(|m| m["header"]["route"] == "brief")
        .unwrap_or_else(|| panic!("the turn asks for a brief: {opened:?}"));
    let id = call["messages"][0]["id"]
        .as_str()
        .expect("call id")
        .to_string();

    // 2. affinity answers it -- the SHIPPED brief, with the 1,600-character persona.
    let ans = brief(
        door(&id, json!(["peer", "channel"]), ""),
        &Store {
            entity: entity(&persona(EN), None),
        },
    );

    // 3. The answer arrives on `in_briefing`, the way the member's edge restamps it.
    let mut back = lane(
        "in_briefing",
        json!({"brief_outcome": "answer", "subject": SUBJECT,
               "slots": "[\"peer\",\"channel\"]", "subscriber": ""}),
        json!({"turn_id": TURN, "counterpart": SUBJECT}),
        ans["messages"].clone(),
    );
    back["system"] = ans["system"].clone();
    back["who"] = ans["who"].clone();
    let parked = assemble(&knob(), back);
    let row = calls_of(in_phase(&parked, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|row| row["role"] == "leg-brief")
        .unwrap_or_else(|| panic!("in_briefing parked no leg-brief row: {parked:?}"));

    // 4. The round collects, and the brain gets its prompt.
    let window = json!({"turn_id": TURN, "iter": 0, "role": "leg-window",
                        "turn": json!({"turns": [{"role": "user", "text": "hello from colony A",
                                                  "consult_id": ""}],
                                       "bytes": 19, "dropped": 0, "capped": 0,
                                       "deferred": 0}).to_string(),
                        "fired": 0});
    let reply = bundle_reply("collect", TURN, &[("c-collect-read", json!([window, row]))]);
    let out = assemble(&knob(), reply);
    let seam = out
        .iter()
        .find(|m| {
            matches!(
                m["header"]["route"].as_str(),
                Some("brain") | Some("answer")
            )
        })
        .unwrap_or_else(|| panic!("the turn opens: {out:?}"));
    let got = seam["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["type"] == "tool_result" && m["id"] == id.as_str())
        .and_then(|m| m["text"].as_str())
        .unwrap_or_else(|| panic!("no brief result in the prompt: {seam}"))
        .to_string();
    assert!(
        !got.contains("[truncated"),
        "the collector cut the brief at tool_chars ({}) -- the tail of the `peer` slot \
         never reaches the model: {got}",
        tool_chars()
    );
    assert!(
        got.contains("\"trust_rank\""),
        "the model reads the peer slot's trust_rank: {got}"
    );
}
