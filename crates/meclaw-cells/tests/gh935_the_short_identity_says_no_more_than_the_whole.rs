//! GH #935 -- the short identity says no more than the whole.
//!
//! `affinity/brief` serves a fifth person slot, `identity_short`: names and kind
//! in one line, capped at `identity_short_max` characters. It is cut from the
//! `identity` document the SAME request already built under the disclosure rows
//! of the SAME round, and from nothing else, so it can never name what the full
//! slot of that round would not.
//!
//! What is pinned here, through the SHIPPED `brief` script over stdin, one phase
//! per run, with the store's answers handed back the way the hive's internal
//! `./store -> ./brief` edge hands them back (the driver of
//! `gh864_a_brief_carries_each_persona_once.rs`):
//!
//! (a) a round that was released the first name only finds neither the last
//!     name, the middle name nor the nickname in `identity_short` (nor in
//!     `identity`), and every word of the short line stands in the full slot of
//!     the same answer;
//! (b) no released name, no `identity_short` slot -- a bare kind names nobody;
//! (c) the cap `identity_short_max` (default 400) cuts at a word boundary;
//! (d) a request for `["identity"]` alone is answered byte for byte as before
//!     the short slot existed: `fixtures/gh935_brief_identity_3_6_2.json` is the
//!     answer the `affinity@3.6.2` brief script produced for this very request
//!     (script_inline as of commit e3f9f0e2c, sha256 prefix cfd94758adf7a39c),
//!     generated once by running that script under python3 with the same
//!     store answers this driver hands back;
//! (e) `brain` together with `identity_short` is refused whole as
//!     `slots_conflict`, like `brain` together with `identity` (GH #488).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{repo, run_cell};
use serde_json::{Value, json};

const BRIEF: &str = "templates/affinity/brief/config.json";

const SUBJECT: &str = "peer:colA/org1/jonas/-";
const CHANNEL: &str = "peer-friend";
const ASKER: &str = "agent:alpha";

/// The answer of `affinity@3.6.2` to the `["identity"]` request of case (d).
const IDENTITY_3_6_2: &str = include_str!("fixtures/gh935_brief_identity_3_6_2.json");

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(BRIEF).is_file()
}

// ─────────────────────────────────────────────────────────────── the lane

/// What the hive's door hands `./brief` on the tool lane: a tool call, the
/// asker and the round promoted onto context.
fn door(slots: Value) -> Value {
    let round = json!([ASKER, SUBJECT]).to_string();
    json!({
        "header": {"hop": {"route": "in_brief"},
                   "context": {"asker": ASKER, "channel_node": CHANNEL,
                               "audience_set": round, "aff_phase": "", "aff_carry": "",
                               "aff_subscriber": ""}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-935",
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

/// One entity, trust `known`, and the field paths released to everyone.
struct Store {
    entity: Value,
    released: Vec<&'static str>,
}

impl Store {
    fn answer(&self, op: &Value) -> Value {
        match (
            op["operation"].as_str().unwrap_or_default(),
            op["table"].as_str().unwrap_or_default(),
        ) {
            ("select", "entities") => {
                let mut row = json!({});
                for c in op["columns"].as_array().expect("columns") {
                    let c = c.as_str().unwrap();
                    row[c] = self.entity.get(c).cloned().unwrap_or(Value::Null);
                }
                json!([row])
            }
            ("select", "trust") => {
                json!([{"level": "known", "decided_at": "2026-09-25T00:00:00.000000Z"}])
            }
            ("select", "disclosure") => Value::Array(
                self.released
                    .iter()
                    .map(|p| {
                        json!({"field_path": p, "mode": "share",
                               "decided_at": "2026-09-25T00:00:00Z",
                               "audience": "*", "audience_set": null})
                    })
                    .collect(),
            ),
            ("traverse", _) => json!([]),
            other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
        }
    }
}

/// Jonas Peter Berg, called Jo: four name parts, so a released part and a
/// withheld one can be told apart.
fn jonas(released: &[&'static str]) -> Store {
    let aieos = json!({
        "identity": {"names": {"first": "Jonas", "middle": "Peter", "last": "Berg",
                               "nickname": "Jo"}},
        "motivations": {"core_drive": "keep the choir singing"},
        "interests": {"hobbies": ["choir"]}
    });
    Store {
        entity: json!({"entity_id": SUBJECT, "kind": "person", "display_name": "Jonas Berg",
                       "status": "active", "aieos": aieos.to_string(), "mx": "{}"}),
        released: released.to_vec(),
    }
}

/// One brief, run to its answer.
fn brief(slots: Value, store: &Store, over: &[(&str, Value)]) -> Value {
    let mut doc = door(slots);
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, over, doc.clone());
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

/// The JSON below the receipt line of the tool result.
fn tool_pack(ans: &Value) -> Value {
    let text = ans["messages"][0]["text"]
        .as_str()
        .expect("tool_result text");
    let (_, payload) = text
        .split_once('\n')
        .unwrap_or_else(|| panic!("the receipt line, then the pack: {text}"));
    serde_json::from_str(payload).unwrap_or_else(|e| panic!("the pack is JSON ({e}): {payload}"))
}

/// The released content of an answer -- the tool lane's pack and every
/// `system` slot WITH its rendering: since GH #939 the rendering's head names
/// only what the round was released, so the `text` is checked like the rest.
fn released_text(ans: &Value) -> String {
    format!("{}{}", tool_pack(ans), ans["system"])
}

fn short_of(ans: &Value) -> String {
    ans["system"]["identity_short"]["short"]
        .as_str()
        .unwrap_or_else(|| panic!("`identity_short.short` is served: {ans}"))
        .to_string()
}

/// Every word of the short line, quotes and parentheses taken off, stands in
/// the full `identity` slot of the same answer.
fn assert_short_inside_full(ans: &Value) {
    let short = short_of(ans);
    let full = ans["system"]["identity"].to_string();
    for word in short.split_whitespace() {
        let w = word.trim_matches(|c| c == '(' || c == ')' || c == '"');
        assert!(
            full.contains(w),
            "`{w}` of the short line `{short}` stands in the full slot: {full}"
        );
    }
}

// ═══════════════════════════════════════ (a) no more than the full slot

#[test]
fn a_name_the_round_was_not_given_is_not_in_the_short_line() {
    if !shipped() {
        return;
    }
    let ans = brief(
        json!(["identity", "identity_short"]),
        &jonas(&["aieos.identity.names.first", "aieos.motivations"]),
        &[],
    );
    assert_eq!(short_of(&ans), "Jonas (person)", "{ans}");
    let released = released_text(&ans);
    for withheld in ["Berg", "Peter", "\"Jo\""] {
        assert!(
            !released.contains(withheld),
            "{withheld} was released to nobody, so neither slot names it: {released}"
        );
    }
    assert_short_inside_full(&ans);
}

#[test]
fn the_whole_name_released_is_the_whole_short_line_and_no_more() {
    if !shipped() {
        return;
    }
    let ans = brief(json!(["identity", "identity_short"]), &jonas(&["*"]), &[]);
    assert_eq!(short_of(&ans), "Jonas Peter Berg \"Jo\" (person)", "{ans}");
    assert_short_inside_full(&ans);
}

// ═══════════════════════════════════════════════════ (b) no name, no slot

#[test]
fn without_a_released_name_there_is_no_short_slot() {
    if !shipped() {
        return;
    }
    let ans = brief(
        json!(["identity", "identity_short"]),
        &jonas(&["aieos.motivations"]),
        &[],
    );
    assert!(
        ans["system"]["identity"].is_object(),
        "the full slot carries what was released: {ans}"
    );
    assert!(
        ans["system"].get("identity_short").is_none()
            && tool_pack(&ans).get("identity_short").is_none(),
        "a bare kind names nobody -- no `identity_short` slot: {ans}"
    );
    // Asked for alone, there is nothing to serve: the brief refuses.
    let alone = brief(
        json!(["identity_short"]),
        &jonas(&["aieos.motivations"]),
        &[],
    );
    assert!(
        alone
            .get("system")
            .is_none_or(|s| s.get("identity_short").is_none()),
        "{alone}"
    );
    assert_eq!(
        alone["messages"][0]["text"], "nothing is disclosed to this audience",
        "{alone}"
    );
}

// ═══════════════════════════════════════════ (c) the cap, at a word boundary

#[test]
fn the_cap_cuts_the_short_line_at_a_word_boundary() {
    if !shipped() {
        return;
    }
    // `Jonas Peter Berg "Jo" (person)`: 14 characters end inside `Berg`, so the
    // line stops after `Peter`; 16 end exactly after `Berg`.
    for (cap, want) in [
        (json!(14), "Jonas Peter"),
        (json!("16"), "Jonas Peter Berg"),
        (json!(400), "Jonas Peter Berg \"Jo\" (person)"),
    ] {
        let ans = brief(
            json!(["identity_short"]),
            &jonas(&["*"]),
            &[("identity_short_max", cap.clone())],
        );
        assert_eq!(short_of(&ans), want, "identity_short_max {cap}: {ans}");
        assert_eq!(
            tool_pack(&ans)["identity_short"]["short"],
            want,
            "identity_short_max {cap}: the tool lane carries the same line: {ans}"
        );
    }
    let ans = brief(json!(["identity_short"]), &jonas(&["*"]), &[]);
    assert_eq!(
        short_of(&ans),
        "Jonas Peter Berg \"Jo\" (person)",
        "the shipped cap (400) leaves a short line whole: {ans}"
    );
}

// ════════════════════════════════════════ (d) `["identity"]` as before

#[test]
fn an_identity_request_is_answered_byte_for_byte_as_before() {
    if !shipped() {
        return;
    }
    let ans = brief(
        json!(["identity"]),
        &jonas(&["aieos.identity.names", "aieos.motivations"]),
        &[],
    );
    let before: Value = serde_json::from_str(IDENTITY_3_6_2).expect("fixture JSON");
    assert_eq!(
        ans["messages"][0]["text"].as_str(),
        before["messages"][0]["text"].as_str(),
        "the tool result of an `[\"identity\"]` request is the 3.6.2 text, byte for byte"
    );
    assert_eq!(
        ans, before,
        "the whole answer of an `[\"identity\"]` request is the 3.6.2 answer"
    );
}

// ════════════════════════════════════ (e) `brain` and the short slot

#[test]
fn brain_with_the_short_slot_is_a_slots_conflict() {
    if !shipped() {
        return;
    }
    let (out, _) = run_cell(BRIEF, &[], door(json!(["brain", "identity_short"])));
    let audit = out
        .iter()
        .find(|m| m["header"]["route"] == "astore" && m["header"]["phase"] == "audit")
        .unwrap_or_else(|| panic!("the refusal is audited: {out:?}"));
    let row = &op_of(audit)["row"];
    assert_eq!(row["outcome"], "denied", "{row}");
    assert_eq!(row["reason_code"], "slots_conflict", "{row}");
    let ans = out
        .iter()
        .find(|m| m["header"]["route"] == "answer")
        .unwrap_or_else(|| panic!("the call is answered: {out:?}"));
    assert!(
        ans.get("system").is_none(),
        "refused whole, nothing served: {ans}"
    );
    assert!(
        out.iter()
            .all(|m| m["header"]["route"] != "astore" || m["header"]["phase"] == "audit"),
        "refused before anything is read: {out:?}"
    );
}
