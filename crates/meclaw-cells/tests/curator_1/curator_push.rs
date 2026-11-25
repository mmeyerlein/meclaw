//! GH #895 -- the curator's recall push, cell by cell, against a ledger that is a real store.
//!
//! `curator/push` builds the question of the collector's per-turn memory ask
//! out of the wall and looks a `gap` of an answer up after the answer. What is
//! pinned here is plan P § 1 and the review focus of § 5:
//!
//! * the ask leaves with EXACTLY the collector's hop keys and a question that
//!   carries the session's topic, what the last three rounds named and what a
//!   short id points at -- in front of the person's words, within
//!   `recall_budget`; `recall_push` off hands it on untouched, and both knobs
//!   are the policy's, resolved by the role the instance runs (GH #892);
//! * a gap is looked up once, on its own ask marked `gap_ask`, and nothing else
//!   leaves the hive for it -- the answer is not waited on; one gap is one mark;
//! * a find becomes a `recall` pair and an `addendum` mark, and the next round
//!   of the same session begins with that pair in its window, once, written
//!   into the wall by `./intake` -- the wall's one writer; nothing found,
//!   nothing written; a turn of a running session (`live`) also says it as a
//!   `fact`;
//! * the prompt hygiene stands in the model's `system.instructions`;
//! * what leaves the hive is only what the round was present for (GH #925):
//!   the entities, the topic and a short id's words in the question come from
//!   rows whose audience holds the round, a find is handed to a round only if
//!   the round it was found for holds it, and every mark the push writes
//!   carries the round that caused it (§ 6).
//!
//! The harness is `support/curator_hive.rs` (GH #892): the shipped
//! `script_inline` programs run under python3, the edges are the hive's own
//! `params.graph` evaluated by the colony's CEL, and every store operation runs
//! through the store cell's own dispatcher against an in-memory SQLite with the
//! shipped schema. The hive runs the role its talky instance runs
//! (`templates/talky/curator`), so the window carries short ids. The road
//! through talky, the member and the memory hive is
//! `gh895_a_fact_from_session_one_reaches_session_three.rs`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::collections::BTreeSet;

/// R2b / GH #49: a tree without the cell skips.
fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

/// The curator of a talky: the shipped hive under the role its instance runs
/// and with the `memory` section its marker registers (GH #1473) and its
/// `things` with the mark `thing` (GH #1523), with `(cell, param, value)`
/// overrides on top.
fn talky(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("intake", "sections", sections())];
    all.extend(over.iter().cloned());
    Hive::with(&all).reporting_composite("talky")
}

/// The sections a talky's curator registers: `memory` (GH #1473) and
/// `things` with the mark `thing`, whose names the push asks with (GH #1523).
fn sections() -> Value {
    json!({"memory": {"lane": "sidecar", "mark": "topic"},
           "things": {"lane": "thing_seen", "cap_share": 0.1, "mark": "thing"}})
}

/// The section `things` of the answer of `turn` (GH #1523): the names the
/// model saw. Since GH #1523 `mentioned:` holds only these names, where the
/// person said them -- no capital, date or quote is taken for one.
fn things(h: &mut Hive, session: &str, turn: &str, names: &[&str]) {
    things_as(h, &test_round(), session, turn, names);
}

/// The same section of an answer spoken in `round`.
fn things_as(h: &mut Hive, round: &Value, session: &str, turn: &str, names: &[&str]) {
    let items: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "kind": "thing"}))
        .collect();
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "curator_call": "c-1", "channel": "test",
               "audience_set": round}),
        json!({"section": "things"}),
        json!({"messages": [], "section": "things", "payload": {"items": items}}),
    );
}

/// The hop of the collector's ask (collector `head` + `recall_ask`).
fn ask_hop(session: &str, turn: &str, text: &str) -> Value {
    json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
           "recall_query": text, "memory_tier": "1",
           "recall_window_from": "", "recall_window_to": ""})
}

/// The round every turn of §§ 1-5 is spoken in (GH #925): the harness's
/// standard round, declared here by every helper itself, so that what one turn
/// writes the next one may read -- whatever a lane of the harness declares
/// when a test declares nothing.
const TEST_ROUND: &str = r#"["member:e"]"#;

fn test_round() -> Value {
    json!(TEST_ROUND)
}

/// The collector's ask of one turn, as its `recall_ask` spells it; returns
/// every `recall` that left the hive for it.
fn ask(h: &mut Hive, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    ask_as(h, &test_round(), session, turn, text)
}

/// The same ask spoken in `round` (`null`: a round nobody declared).
fn ask_as(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": session, "channel": "test", "audience_set": round}),
        ask_hop(session, turn, text),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// One round on `in_curate` spoken in `round`, the way the harness's `curate`
/// sends one; returns the call that left for the model.
fn curate_as(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> Msg {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": round}),
        json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(text)], "system": mode("")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// A whole turn the way talky runs it: the ask first, the round after its
/// bundle came home, the answer on the tap. Returns the call.
fn round(h: &mut Hive, session: &str, turn: &str, text: &str, reply: &str) -> Msg {
    round_as(h, &test_round(), session, turn, text, reply)
}

/// The same turn spoken in `round`: the ask, the call and the answer on the
/// tap all carry it, as the member's door stamps every message of a turn.
fn round_as(
    h: &mut Hive,
    round: &Value,
    session: &str,
    turn: &str,
    text: &str,
    reply: &str,
) -> Msg {
    let asks = ask_as(h, round, session, turn, text);
    assert_eq!(
        asks.len(),
        1,
        "every ask leaves exactly once: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = curate_as(h, round, session, turn, text);
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

/// The section `gap` of the answer of `turn`, as the splitter cuts it and the
/// parent hands it in (`in_section`); `payload` is the splitter's slot.
fn gap_of(h: &mut Hive, session: &str, turn: &str, payload: Value, live: &str) -> Vec<Msg> {
    gap_in(h, &test_round(), session, turn, payload, live)
}

/// The same section of an answer spoken in `round`.
fn gap_in(
    h: &mut Hive,
    round: &Value,
    session: &str,
    turn: &str,
    payload: Value,
    live: &str,
) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "curator_call": "c-1", "channel": "test", "live": live,
               "audience_set": round}),
        json!({"section": "gap"}),
        json!({"messages": [], "section": "gap", "payload": payload}),
    );
    h.routed("recall")
}

/// A gap the model wrote as a string: the splitter hands it on as
/// `{payload: <text>}`.
fn gap(h: &mut Hive, session: &str, turn: &str, text: &str, live: &str) -> Vec<Msg> {
    gap_of(h, session, turn, json!({"payload": text}), live)
}

/// The same gap in an answer spoken in `round`.
fn gap_as(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    gap_in(h, round, session, turn, json!({"payload": text}), "")
}

/// The section `memory` of the answer of `turn`, naming `topic`, as the
/// splitter cuts it and the parent hands it in (`in_section`). `./intake`
/// files the `topic` mark out of it -- the one way a live mark comes to be
/// (OR-KY-71); a mark sown by SQL would only pin the form the test assumes
/// (review I-1).
fn memory_section(h: &mut Hive, session: &str, turn: &str, topic: Value) {
    memory_section_as(h, &test_round(), session, turn, topic);
}

/// The same section of an answer spoken in `round`.
fn memory_section_as(h: &mut Hive, round: &Value, session: &str, turn: &str, topic: Value) {
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "curator_call": "c-1", "channel": "test",
               "audience_set": round}),
        json!({"section": "memory"}),
        json!({"messages": [], "section": "memory", "payload": {"topic": topic}}),
    );
}

/// The memory's answer to a gap's ask, as the parent's edge delivers it: the
/// ask's context with `gap_ask` lifted into it.
fn gap_bundle(h: &mut Hive, ask: &Msg, hop: Value, text: &str) {
    h.out.clear();
    let mut ctx = ask.context.clone();
    ctx.insert("gap_ask".into(), ask.hop["gap_ask"].clone());
    h.lane(
        "in_gap_bundle",
        Value::Object(ctx),
        hop,
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                             "text": text}]}),
    );
}

/// `(session, turn, value)` of every mark of `kind`, oldest first; a value
/// written as JSON comes back parsed.
fn marks(h: &Hive, kind: &str) -> Vec<(String, String, Value)> {
    h.rows(&format!(
        "SELECT session_id, turn_id, value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| {
        let v = match &r[2] {
            Value::String(s) => sj::from_str(s).unwrap_or(Value::String(s.clone())),
            other => other.clone(),
        };
        (
            r[0].as_str().unwrap_or("").to_string(),
            r[1].as_str().unwrap_or("").to_string(),
            v,
        )
    })
    .collect()
}

fn query_of(m: &Msg) -> String {
    m.hop["recall_query"].as_str().unwrap_or("").to_string()
}

/// A window text without its short id: with the talky's `short_ids` on,
/// somebody else's words and a tool result begin with `[#<12 hex>] ` (GH #892).
fn bare(text: &str) -> &str {
    let b = text.as_bytes();
    let id = b.len() >= 16
        && b.starts_with(b"[#")
        && b[2..14].iter().all(u8::is_ascii_hexdigit)
        && &b[14..16] == b"] ";
    if id { &text[16..] } else { text }
}

/// The texts of a call's window, each without its short id.
fn window(call: &Msg) -> Vec<String> {
    texts(call).iter().map(|t| bare(t).to_string()).collect()
}

const ADDENDUM: &str = "[addendum to your last answer";

// ============================================================ 1. the shape

#[test]
fn the_push_is_a_cell_of_the_hive_behind_the_policy() {
    if !push_shipped() {
        return;
    }
    let push = cell_config("push");
    assert_eq!(push["cell"]["type"], "code");
    assert_eq!(
        push["params"]["runner_mode"], "resident",
        "one child, strictly serial"
    );
    let routes: Vec<&str> = push["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("the push declares what it emits")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(routes, ["lstore", "recall", "sidecar", "in_addendum"]);
    let policy = cell_config("policy");
    let emits: Vec<&str> = policy["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for r in ["push_ask", "push_gap"] {
        assert!(emits.contains(&r), "policy declares `{r}`");
    }
    let hive = read_json(&repo("templates/curator/config.json"));
    let lanes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    for l in ["in_recall_ask", "in_gap_bundle"] {
        assert!(lanes("accepts").contains(&l.to_string()), "accepts {l}");
    }
    for l in ["recall", "sidecar"] {
        assert!(lanes("emits").contains(&l.to_string()), "emits {l}");
    }
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap();
    // The addendum goes into the wall through its one writer (OR-KX.K.1).
    assert!(
        edges.iter().any(|e| e["from"] == "./push"
            && e["to"] == "./intake"
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("in_addendum"))),
        "the push hands an addendum to `./intake`"
    );
    // Every exit of the push clears the hive's interior keys (GH #494).
    let exits: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == "./push" && e["to"] == ".")
        .collect();
    assert!(!exits.is_empty(), "the push leaves through the hive path");
    for e in exits {
        let gone: BTreeSet<&str> = e["modifier"]["delete_context"]
            .as_array()
            .expect("an exit clears context")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
            assert!(gone.contains(k), "{e}: {k} survives the rim");
        }
    }
}

/// The fence rule and the hash rule are one text in every script that uses
/// them (`curator_cells.rs`, same rule).
#[test]
fn the_push_shares_the_helpers_word_for_word() {
    if !push_shipped() {
        return;
    }
    let def = |script: &str, name: &str| -> String {
        let at = script
            .find(&format!("\ndef {name}("))
            .unwrap_or_else(|| panic!("no {name}"));
        let rest = &script[at + 1..];
        let end = rest[1..].find("\ndef ").map_or(rest.len(), |e| e + 1);
        rest[..end].trim_end().to_string()
    };
    let (i, p) = (script_of("intake"), script_of("push"));
    for name in ["canonical", "block_hash", "block_span", "stripped_text"] {
        assert_eq!(def(&i, name), def(&p, name), "{name}: intake vs push");
    }
}

// ============================================================ 2. the question

#[test]
fn the_push_emits_the_collectors_key_set() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "My sister Hannah moved to Porto.",
        "Porto, how lovely.",
    );
    let asks = ask(&mut h, "s1", "t2", "Where does she live now?");
    assert_eq!(asks.len(), 1, "exactly one ask leaves: {:?}", h.out);
    let got: BTreeSet<&str> = asks[0].hop.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = [
        "route",
        "phase",
        "turn_id",
        "session_id",
        "iter",
        "recall_query",
        "memory_tier",
        "recall_window_from",
        "recall_window_to",
        // GH #1040: the answering model's package, which only this hive knows.
        "recall_input_soft",
    ]
    .into_iter()
    .collect();
    assert_eq!(got, want, "the collector's keys and no other");
    let sent = obj(ask_hop("s1", "t2", "Where does she live now?"));
    for (k, v) in &sent {
        if k != "recall_query" {
            assert_eq!(&asks[0].hop[k], v, "{k} is the collector's");
        }
    }
    // The body is the question the hop names, as the collector's ask has it.
    assert_eq!(
        asks[0].messages()[0]["text"].as_str().unwrap_or(""),
        query_of(&asks[0])
    );
    // Nothing of the hive's interior leaves with it.
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(!asks[0].context.contains_key(k), "{k} left the hive");
    }
}

#[test]
fn the_query_carries_topic_entities_and_refs() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "My sister Hannah moved to Porto on 2026-03-14.",
        "Nice. Did \"Casa Verde\" work out for her?",
    );
    round(
        &mut h,
        "s1",
        "t2",
        "yes, she loves \"Casa Azul\"",
        "Good to hear that Hannah is happy.",
    );
    // GH #1523: the names the model saw in each answer.
    things(&mut h, "s1", "t1", &["Hannah", "Porto", "Casa Verde"]);
    things(&mut h, "s1", "t2", &["Casa Azul", "Hannah"]);
    // The topic the memory section of the first answer named, filed by
    // `./intake` the way every section's marks are.
    memory_section(
        &mut h,
        "s1",
        "t1",
        json!({"movement": "start", "name": "the move"}),
    );
    assert_eq!(
        marks(&h, "topic"),
        [(
            "s1".to_string(),
            "t1".to_string(),
            json!({"movement": "start", "name": "the move"})
        )],
        "the intake files the topic"
    );
    let first_user: String = h.rows("SELECT hash FROM wall WHERE kind = 'user' ORDER BY seq")[0][0]
        .as_str()
        .unwrap()
        .to_string();
    let text = format!("And what did I say in [#{}]?", &first_user[..12]);
    let q = query_of(&ask(&mut h, "s1", "t3", &text)[0]);
    assert!(
        q.ends_with(&text),
        "the person's words come last, whole: {q}"
    );
    assert!(q.contains("topic: the move"), "the topic: {q}");
    let named: Vec<&str> = q
        .split("mentioned: ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("no entities: {q}"))
        .split(", ")
        .collect();
    // The person's words only (GH #1342): a name the model named and the
    // person said, newest round first; "Casa Verde" the model alone said
    // names nothing. GH #1523: a date is no name the model named -- the date
    // the word heuristic took ("2026-03-14") stays out.
    assert_eq!(named, ["Casa Azul", "Hannah", "Porto"], "{q}");
    assert!(
        !q.contains("Casa Verde"),
        "a quote in the model's answer is no anchor: {q}"
    );
    assert!(
        q.contains("refers to: My sister Hannah"),
        "a short id in the person's words is what it points at: {q}"
    );
    assert!(q.len() <= 200, "the talky's budget: {} characters", q.len());
}

/// OR-KY-71: the open topic is the name of the newest topic mark that carries
/// one, unless an `end` came after it -- a turn that names no topic
/// (`continue` with an empty name, the nothing form of every quiet turn) hides
/// nothing, and a movement is never a name. Every mark a live curator holds is
/// filed by `./intake` out of a `memory` section, and so is every mark here
/// (review I-1): the push's page of topic marks (`a-topic`, 16 rows) leaves
/// out the nameless `continue` by its exact bytes, which only the intake's
/// own serialisation can prove -- so more quiet turns stand behind the name
/// than one page holds.
#[test]
fn the_topic_is_the_newest_named_mark_no_end_closed() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "we are moving", "Good luck with it.");
    let question = |h: &mut Hive, turn: &str| query_of(&ask(h, "s1", turn, "and then?")[0]);
    memory_section(
        &mut h,
        "s1",
        "t1",
        json!({"movement": "start", "name": "the move"}),
    );
    for n in 2..=18 {
        memory_section(
            &mut h,
            "s1",
            &format!("t{n}"),
            json!({"movement": "continue"}),
        );
    }
    let quiet = marks(&h, "topic")
        .iter()
        .filter(|(_, _, v)| *v == json!({"movement": "continue", "name": ""}))
        .count();
    assert_eq!(
        quiet,
        17,
        "one nameless continue per quiet turn: {:?}",
        marks(&h, "topic")
    );
    // The bare movement a curator before OR-KY-71 wrote: a movement, no name.
    // No writer of this form is left, so it is the one mark sown by SQL.
    h.mark(chrono::Utc::now(), "s1", "t18", "topic", "continue");
    // Another session's topic is not this one's.
    memory_section(
        &mut h,
        "s2",
        "t9",
        json!({"movement": "start", "name": "elsewhere"}),
    );
    let q = question(&mut h, "t19");
    assert!(q.contains("topic: the move"), "{q}");
    assert!(
        !q.contains("topic: continue") && !q.contains("elsewhere"),
        "{q}"
    );
    memory_section(
        &mut h,
        "s1",
        "t19",
        json!({"movement": "end", "name": "the move"}),
    );
    let q = question(&mut h, "t20");
    assert!(!q.contains("topic:"), "an ended topic is no topic: {q}");
    memory_section(
        &mut h,
        "s1",
        "t20",
        json!({"movement": "start", "name": "the flat"}),
    );
    let q = question(&mut h, "t21");
    assert!(q.contains("topic: the flat"), "{q}");
}

#[test]
fn the_query_respects_recall_budget() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[("policy", "recall_budget", json!(60))]);
    round(
        &mut h,
        "s1",
        "t1",
        "My sister Hannah lives in Porto with Ana, Bea, Carla and Dora.",
        "ok",
    );
    things(
        &mut h,
        "s1",
        "t1",
        &["Hannah", "Porto", "Ana", "Bea", "Carla", "Dora"],
    );
    let q = query_of(&ask(&mut h, "s1", "t2", "where does she live?")[0]);
    assert!(q.len() <= 60, "{} > 60: {q}", q.len());
    assert!(q.ends_with("where does she live?"), "{q}");
    assert_eq!(
        q, "[mentioned: Hannah, Porto, Ana, Bea] where does she live?",
        "the oldest entities go first"
    );
    // A question longer than the budget is never cut here: the memory's own
    // hygiene guard is the one place a question is cut (GH #88).
    let long = "where exactly does my sister live these days, and with whom?";
    let q = query_of(&ask(&mut h, "s1", "t3", long)[0]);
    assert_eq!(q, long);
}

#[test]
fn recall_push_off_passes_the_collectors_query_through() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[("policy", "recall_push", json!("0"))]);
    round(&mut h, "s1", "t1", "Hannah lives in Porto.", "ok");
    let asks = ask(&mut h, "s1", "t2", "where again?");
    assert_eq!(asks.len(), 1);
    let mut want = obj(ask_hop("s1", "t2", "where again?"));
    want.insert("route".into(), json!("recall"));
    // GH #1044: the question as it came, sized by the model's package like a
    // pushed one -- no answer named a package here, so the key is empty.
    want.insert("recall_input_soft".into(), json!(""));
    assert_eq!(asks[0].hop, want, "the ask as it came");
    assert_eq!(asks[0].messages(), vec![user("where again?")]);
}

/// The two knobs are the policy's and the role says what they are (GH #892):
/// a consulted core and a curator without a role hand the collector's ask on
/// as it came -- only the talky pushes.
#[test]
fn the_push_follows_the_role() {
    if !push_shipped() {
        return;
    }
    for (role, pushes) in [("talky", true), ("consult", false), ("", false)] {
        let mut h = Hive::with(&[("intake", "sections", sections())]);
        if !role.is_empty() {
            h.report_profile(curator_profiles::profile(role));
        }
        let call = round(
            &mut h,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, how lovely.",
        );
        things(&mut h, "s1", "t1", &["Hannah", "Porto"]);
        let q = query_of(&ask(&mut h, "s1", "t2", "where again?")[0]);
        if pushes {
            assert_eq!(q, "[mentioned: Hannah, Porto] where again?", "{role:?}");
        } else {
            assert_eq!(q, "where again?", "role {role:?} pushes nothing");
            let system = call.body.get("system").cloned().unwrap_or(Value::Null);
            assert!(
                system["instructions"]["hygiene"].is_null(),
                "role {role:?}: no hygiene without the push: {system}"
            );
        }
    }
}

/// The memory keeps a query whole only up to its `query_safe_chars` (GH #88):
/// above it, only the last question survives and the enrichment in front of it
/// is gone. The talky's budget is held to that number here, where both are
/// shipped (§ 2d) -- the colony lock measures the same at the memory.
#[test]
fn the_talky_budget_keeps_the_query_whole_at_the_memory() {
    if !push_shipped() || !repo("templates/memory-hive/recall/config.json").is_file() {
        return;
    }
    let recall = read_json(&repo("templates/memory-hive/recall/config.json"));
    let script = recall["params"]["script_inline"].as_str().expect("script");
    // GH #1040: a knob the model package sizes; its literal is the value
    // without a package, which is what the role's preset is held to.
    let lit = "_pkg(\"query_safe_chars\", ";
    let at = script.find(lit).expect("the memory's query_safe_chars") + lit.len();
    let safe: f64 = script[at..at + script[at..].find(')').unwrap()]
        .trim()
        .parse()
        .expect("a number");
    let (budget, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "number_of(P, 'recall_budget')",
        Value::Null,
    );
    assert!(err.is_empty(), "{err}");
    let budget = budget.as_f64().expect("a number");
    assert!(
        budget > 0.0 && budget <= safe,
        "the talky's recall_budget {budget} is over the memory's query_safe_chars {safe}: \
         every enriched query longer than that loses its enrichment at the memory"
    );
    // And the push uses it: a long question with a wall full of names.
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "Can you recommend a book about sailing, something like Moby Dick or Treasure Island?",
        "Try The Cruel Sea by Nicholas Monsarrat.",
    );
    things(
        &mut h,
        "s1",
        "t1",
        &[
            "Moby Dick",
            "Treasure Island",
            "The Cruel Sea",
            "Nicholas Monsarrat",
        ],
    );
    let long = "Sorry, my memory is a sieve these days and I keep mixing up the family \
                news I told you about, so where does my sister live now?";
    let q = query_of(&ask(&mut h, "s1", "t2", long)[0]);
    assert!(q.len() as f64 <= budget, "{} > {budget}: {q}", q.len());
    assert!(q.contains("mentioned: ") && q.ends_with(long), "{q}");
}

#[test]
fn a_lower_case_wall_names_no_entity() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "my sister lives in porto since march.",
        "porto sounds nice. she will love it.",
    );
    let q = query_of(&ask(&mut h, "s1", "t2", "and her flat?")[0]);
    assert_eq!(q, "and her flat?", "nothing to add is nothing added");
}

// ============================================================ 3. the gap

#[test]
fn a_gap_searches_once_and_does_not_block() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let asks = gap(&mut h, "s1", "t1", "the name of the ferry to Porto", "");
    assert_eq!(asks.len(), 1, "one lookup: {:?}", h.out);
    assert_eq!(
        h.out.len(),
        1,
        "and nothing else leaves -- no call, no answer waits for it: {:?}",
        h.out
    );
    let a = &asks[0];
    assert_eq!(a.hop["recall_query"], "the name of the ferry to Porto");
    assert_eq!(
        a.hop["memory_tier"], "1",
        "the tier of this surface's ambient ask"
    );
    assert_eq!(a.hop["turn_id"], "t1");
    assert!(
        a.hop["gap_ask"].as_str().is_some_and(|s| !s.is_empty()),
        "marked for the way back"
    );
    let kept = marks(&h, "gap");
    assert_eq!(kept.len(), 1, "one gap, one mark: {kept:?}");
    assert_eq!(kept[0].2["text"], "the name of the ferry to Porto");
    // The same section again is the same gap.
    assert!(gap(&mut h, "s1", "t1", "the name of the ferry to Porto", "").is_empty());
    assert_eq!(marks(&h, "gap").len(), 1);
}

/// One gap is one mark, in one form, whatever form the model gave it: the
/// door of `./intake` does not take the section, `./push` does (review I-5).
#[test]
fn a_gap_is_one_mark_whatever_its_form() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    let call = round(&mut h, "s1", "t1", "hi", "hello");
    h.section(&call, "gap", json!({"query": "the owner's birthday"}));
    let kept = marks(&h, "gap");
    assert_eq!(kept.len(), 1, "one gap, one mark: {kept:?}");
    assert_eq!(kept[0].2["text"], "the owner's birthday", "{kept:?}");
    assert!(
        !h.ledger_ops.iter().any(|(who, op)| who == "intake"
            && op["table"] == "marks"
            && op["operation"] == "insert"),
        "the intake marks no gap: {:?}",
        h.ledger_ops
    );
}

#[test]
fn a_hit_becomes_an_addendum_pair_in_the_next_round() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let ask = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        "- Hannah took the ferry Estrela do Norte",
    );
    assert!(
        h.out.is_empty(),
        "a kept find says nothing now: {:?}",
        h.out
    );
    let kept = marks(&h, "addendum");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].1, "t1", "under the gap's own round");
    let call = round(&mut h, "s1", "t2", "and when does it leave?", "At nine.");
    let t = window(&call);
    let at = t
        .iter()
        .position(|x| x.starts_with(ADDENDUM))
        .unwrap_or_else(|| panic!("no addendum in the window: {t:?}"));
    assert!(t[at].contains("Estrela do Norte"));
    assert_eq!(
        call.messages()[at - 1]["type"],
        "tool_call",
        "a pair: the call in front of its result"
    );
    let asked = t
        .iter()
        .position(|x| x == "and when does it leave?")
        .expect("the round's own turn");
    assert!(at < asked, "the round begins with it");
    assert!(
        t.iter()
            .position(|x| x == "hello")
            .expect("the last answer")
            < at - 1,
        "after the answer it adds to"
    );
    let done = marks(&h, "addendum_done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].1, "t2", "shown in the round after");
    assert_eq!(done[0].2["delivered"], 1);
    // Shown once: the round after carries the same pair once, not twice.
    let call = round(&mut h, "s1", "t3", "thanks", "You're welcome.");
    assert_eq!(
        window(&call)
            .iter()
            .filter(|x| x.starts_with(ADDENDUM))
            .count(),
        1
    );
    assert_eq!(marks(&h, "addendum_done").len(), 1);
}

/// OR-KX.K.1 (review I-1): the wall has ONE writer, `./intake`, whose
/// sequence numbers are the order of arrival. The addendum pair reaches the
/// wall through it, ahead of the round it opens -- measured at the store.
#[test]
fn the_wall_has_one_writer() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let ask = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        "- Hannah took the ferry Estrela do Norte",
    );
    round(&mut h, "s1", "t2", "and when does it leave?", "At nine.");
    let writers: BTreeSet<&str> = h
        .ledger_ops
        .iter()
        .filter(|(_, op)| op["table"] == "wall" && op["operation"] != "select")
        .map(|(who, _)| who.as_str())
        .collect();
    assert_eq!(
        writers,
        BTreeSet::from(["intake"]),
        "the wall's writers: {writers:?}"
    );
    let rows = h
        .rows("SELECT kind, seq FROM wall WHERE session_id = 's1' AND turn_id = 't2' ORDER BY seq");
    let kinds: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap_or("")).collect();
    assert_eq!(
        kinds,
        ["recall", "recall", "user", "assistant"],
        "the pair first, then the round: {rows:?}"
    );
}

#[test]
fn no_hit_no_addendum() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let ask = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    for (hop, text) in [
        (
            json!({"recall_empty": "1"}),
            "Nothing in this memory answers this question (as of 29 Sep 2026).",
        ),
        (json!({}), "Nothing in this memory answers this question."),
        (json!({"reject_reason": "missing_audience"}), "rejected"),
    ] {
        gap_bundle(&mut h, &ask, hop, text);
        assert!(h.out.is_empty(), "{text}: {:?}", h.out);
    }
    assert!(marks(&h, "addendum").is_empty());
    let call = round(&mut h, "s1", "t2", "next", "ok");
    assert!(!window(&call).iter().any(|x| x.starts_with(ADDENDUM)));
}

/// A running session's find (`live` in the context) is said at once, as a `fact` -- and a `fact` leaves
/// the colony: the member hands it to the channel as advice and the voice
/// reads it out, over edges only (`a_duplex_find_is_said_once_on_its_channel`
/// in `gh895_…` walks that road; no cell on it could clean the words). So the
/// short ids a gap or a find quotes stay inside (R-27-3, OR-KY-68, review
/// I-2): the words that leave carry none, the window's addendum -- which the
/// model reads, and which `history_read` needs -- keeps them.
#[test]
fn a_duplex_turn_speaks_the_addendum() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let ask = gap(
        &mut h,
        "s1",
        "t1",
        "whether [#0123456789ab] was the ferry",
        "call-9",
    )[0]
    .clone();
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        // A find quotes ids in every form the colony makes one -- the window's
        // bracket, and the bare id a history tool answers with, up to the
        // sixteen digits of an ambiguous one (OR-KY-81, OR-KY-83).
        "- Hannah took the ferry Estrela do Norte [#abcdef012345] #abcdef0123456789\n\
         - [#fedcba987654] It left at nine",
    );
    let spoken = h.routed("sidecar");
    assert_eq!(spoken.len(), 1, "{:?}", h.out);
    assert_eq!(spoken[0].hop["section"], "fact");
    assert_eq!(spoken[0].body["section"], "fact");
    let words = spoken[0].body["payload"]["payload"].as_str().unwrap_or("");
    assert!(
        words.contains("Estrela do Norte") && words.contains("It left at nine"),
        "{words}"
    );
    assert!(
        !words.contains('#'),
        "a short block id, bracketed or bare, reaches the voice: {words}"
    );
    assert!(
        words.contains("(whether was the ferry)") && words.contains("Norte It left"),
        "an id goes with the space in front of it: {words}"
    );
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(!spoken[0].context.contains_key(k), "{k} left the hive");
    }
    // And it is kept for the next round all the same, ids and all.
    let kept = marks(&h, "addendum");
    assert_eq!(kept.len(), 1);
    assert!(
        kept[0].2["gap"]
            .as_str()
            .unwrap_or("")
            .contains("[#0123456789ab]"),
        "the window's addendum keeps its ids: {kept:?}"
    );
}

#[test]
fn a_find_for_an_ended_session_is_not_shown() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let ask = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        "- the ferry is called Estrela do Norte",
    );
    let call = round(&mut h, "s2", "t1", "a new call", "hello again");
    assert!(
        !window(&call).iter().any(|x| x.starts_with(ADDENDUM)),
        "an addendum to an answer of another session is no addendum"
    );
    assert!(marks(&h, "addendum_done").is_empty());
}

#[test]
fn a_find_the_window_already_carries_is_not_carried_twice() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    for turn in ["t1", "t2"] {
        if turn == "t2" {
            round(&mut h, "s1", "t2", "go on", "ok");
        }
        let ask = gap(&mut h, "s1", turn, "the ferry", "")[0].clone();
        gap_bundle(
            &mut h,
            &ask,
            json!({}),
            "- the ferry is called Estrela do Norte",
        );
    }
    // Two gaps, the same find: the pair is one block and stands once.
    let call = round(&mut h, "s1", "t3", "and then?", "then home");
    assert_eq!(
        window(&call)
            .iter()
            .filter(|x| x.starts_with(ADDENDUM))
            .count(),
        1,
        "{:?}",
        window(&call)
    );
}

// ============================================================ 4. the hygiene

#[test]
fn the_hygiene_stands_in_the_talky_instructions() {
    if !push_shipped() {
        return;
    }
    let hygiene_of = |h: &mut Hive| -> Value {
        let call = round(h, "s1", "t1", "hi", "hello");
        let system = call.body.get("system").cloned().unwrap_or(Value::Null);
        system["instructions"].clone()
    };
    let instructions = hygiene_of(&mut talky(&[]));
    let hygiene = instructions["hygiene"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no hygiene in {instructions}"));
    assert!(
        hygiene.contains("ask them instead of guessing"),
        "{hygiene}"
    );
    assert!(hygiene.contains("[#<12 hex digits>]"), "{hygiene}");
    assert!(hygiene.contains("history_read"), "{hygiene}");
    // The family is sent whole: the collector's leaf stands beside it.
    assert!(instructions["mode"].is_object());
    // Without short ids there is no id to read: the second sentence is not said.
    let instructions = hygiene_of(&mut talky(&[("policy", "short_ids", json!("0"))]));
    let hygiene = instructions["hygiene"]["text"].as_str().unwrap_or("");
    assert!(
        hygiene.contains("ask them instead of guessing"),
        "{hygiene}"
    );
    assert!(!hygiene.contains("history_read"), "{hygiene}");
    // Off with the push: a consulted core is told nothing of the kind.
    let instructions = hygiene_of(&mut talky(&[("policy", "recall_push", json!("0"))]));
    assert!(instructions["hygiene"].is_null(), "{instructions}");
}

// ============================================================ 5. the offer

/// OR-KY-G1: the hive answers the collector's menu question like a tools hive,
/// and the section `gap` is its entry in that answer (`CURATOR_OFFER` in
/// `./schemas`, OR-KY.T.1).
#[test]
fn the_menu_offers_the_gap_section() {
    if !push_shipped() {
        return;
    }
    let mut h = Hive::new();
    h.out.clear();
    h.lane(
        "in_schemas",
        json!({"session_id": "s1"}),
        json!({"operation": ""}),
        json!({"messages": [], "tools": ["*"]}),
    );
    let offers: Vec<Value> = h
        .out
        .iter()
        .filter_map(|m| m.body.get("sidecar").and_then(Value::as_array).cloned())
        .flatten()
        .collect();
    let gap = offers
        .iter()
        .find(|o| o["section"] == "gap")
        .unwrap_or_else(|| panic!("no gap in the offer: {:?}", h.out));
    assert_eq!(
        gap["instruction"],
        "write what you were unsure about or missed; it will be looked up after this answer"
    );
    assert_eq!(gap["required"], false);
}

// ============================================================ 6. the audience

/// GH #925 rounds in the affinity vocabulary, in the form the colony carries
/// them: TEXT, a JSON array in a string -- deliberately out of the canonical
/// order the ledger's column keeps.
const EAB: &str = r#"["member:e","member:a","member:b"]"#;
const EA: &str = r#"["member:e","member:a"]"#;
const EB: &str = r#"["member:e","member:b"]"#;
const EABC: &str = r#"["member:e","member:a","member:b","member:c"]"#;
/// `EA` as the ledger keeps it.
const EA_CANON: &str = r#"["member:a","member:e"]"#;

fn declared(round: &str) -> Value {
    json!(round)
}

/// `(kind, audience_set)` of every mark the push writes, oldest first; a
/// NULL column is `None`.
fn push_marks(h: &Hive) -> Vec<(String, Option<String>)> {
    h.rows(
        "SELECT kind, audience_set FROM marks \
         WHERE kind IN ('gap', 'addendum', 'addendum_done') ORDER BY seq",
    )
    .into_iter()
    .map(|r| {
        (
            r[0].as_str().unwrap_or("").to_string(),
            r[1].as_str().map(str::to_string),
        )
    })
    .collect()
}

/// The question leaves the hive -- toward the memory, and in the memory pair
/// back into a window -- so the entities in it come only from rows whose
/// audience holds the round: {e,a} was present for what {e,a,b} said, not for
/// what {e,b} said, and {e,a,b,c} was present for neither (GH #925, the
/// affinity rule). A hidden row leaves no trace in the question. All in one
/// session: since GH #1342 only the running session's words anchor it.
#[test]
fn the_question_names_only_what_the_round_was_present_for() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round_as(
        &mut h,
        &declared(EAB),
        "s1",
        "t1",
        "My sister Hannah moved to Porto.",
        "Porto, how lovely.",
    );
    things_as(&mut h, &declared(EAB), "s1", "t1", &["Hannah", "Porto"]);
    round_as(
        &mut h,
        &declared(EB),
        "s1",
        "t2",
        "I met Bea and Carlos in Lisbon.",
        "Lisbon in spring, lovely.",
    );
    things_as(
        &mut h,
        &declared(EB),
        "s1",
        "t2",
        &["Bea", "Carlos", "Lisbon"],
    );
    let q = query_of(&ask_as(&mut h, &declared(EA), "s1", "t3", "where again?")[0]);
    assert_eq!(
        q, "[mentioned: Hannah, Porto] where again?",
        "{{e,a}} is asked with what {{e,a,b}} said and nothing {{e,b}} said"
    );
    let q = query_of(&ask_as(&mut h, &declared(EABC), "s1", "t4", "where again?")[0]);
    assert_eq!(q, "where again?", "{{e,a,b,c}} was present for neither");
}

/// A running session's find is said at once only to a round that may hear it (review
/// M-10, R2-I-14). The gap's round rides the ask out and home, but the call on
/// the line may have widened since: the find goes to the voice as a `fact`
/// only when the round of the session's newest call may see the addendum --
/// read in the bundle that keeps it, no round trip more -- and it is kept as
/// an addendum all the same.
#[test]
fn a_duplex_find_is_said_only_to_a_round_that_may_hear_it() {
    if !push_shipped() {
        return;
    }
    for (live, spoken) in [(EA, true), (EAB, false), (EB, false)] {
        let mut h = talky(&[]);
        round_as(&mut h, &declared(EA), "s1", "t1", "hi", "hello");
        let ask = gap_in(
            &mut h,
            &declared(EA),
            "s1",
            "t1",
            json!({"payload": "whether the ferry left"}),
            "call-9",
        )[0]
        .clone();
        if live != EA {
            round_as(
                &mut h,
                &declared(live),
                "s1",
                "t2",
                "who is there?",
                "Everyone.",
            );
        }
        gap_bundle(&mut h, &ask, json!({}), "- The ferry left at nine");
        assert_eq!(
            h.routed("sidecar").len(),
            usize::from(spoken),
            "live round {live}: {:?}",
            h.out
        );
        assert!(
            push_marks(&h).contains(&("addendum".to_string(), Some(EA_CANON.to_string()))),
            "live round {live}: the find is kept as an addendum: {:?}",
            push_marks(&h)
        );
    }
}

/// OR-BD-4 / OR-BD.A.2: a round nobody declared is asked with what its own
/// session said WITHOUT a round, and nothing of another session -- and its
/// ask leaves all the same. What a declared round said in the same session
/// stays (review M-7): nobody declared who hears the round-less call.
#[test]
fn a_round_nobody_declared_is_asked_with_its_own_session_only() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round_as(
        &mut h,
        &declared(EAB),
        "s1",
        "t1",
        "My sister Hannah moved to Porto.",
        "Porto, how lovely.",
    );
    round_as(
        &mut h,
        &declared(EA),
        "s2",
        "t1",
        "We sailed to Madeira with Tomas.",
        "Madeira, wonderful.",
    );
    things_as(&mut h, &declared(EA), "s2", "t1", &["Madeira", "Tomas"]);
    let asks = ask_as(&mut h, &Value::Null, "s2", "t2", "and then?");
    assert_eq!(asks.len(), 1, "the ask always leaves: {:?}", h.out);
    assert_eq!(
        query_of(&asks[0]),
        "and then?",
        "what {{e,a}} said in the session is not asked with"
    );
    round_as(
        &mut h,
        &Value::Null,
        "s2",
        "t3",
        "We flew to Faro with Ines.",
        "Faro, wonderful.",
    );
    things_as(&mut h, &Value::Null, "s2", "t3", &["Faro", "Ines"]);
    let asks = ask_as(&mut h, &Value::Null, "s2", "t4", "and then?");
    assert_eq!(asks.len(), 1, "the ask always leaves: {:?}", h.out);
    assert_eq!(
        query_of(&asks[0]),
        "[mentioned: Faro, Ines] and then?",
        "what the session said without a round is"
    );
    let asks = ask_as(&mut h, &Value::Null, "s9", "t1", "and then?");
    assert_eq!(asks.len(), 1, "the ask always leaves: {:?}", h.out);
    assert_eq!(
        query_of(&asks[0]),
        "and then?",
        "a session with nothing of its own is asked with nothing"
    );
}

/// A short id in the person's words is said in the question only where the
/// round may see a row that carries it: an id copied out of a conversation of
/// {e,b} names nothing to {e,a} (GH #925).
#[test]
fn a_short_id_is_resolved_only_for_a_row_the_round_may_see() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round_as(
        &mut h,
        &declared(EB),
        "s1",
        "t1",
        "my sister moved to porto.",
        "ok",
    );
    round_as(
        &mut h,
        &declared(EAB),
        "s2",
        "t1",
        "we sailed to madeira.",
        "ok",
    );
    let id = |h: &Hive, s: &str| -> String {
        h.rows(&format!(
            "SELECT hash FROM wall WHERE session_id = '{s}' AND kind = 'user'"
        ))[0][0]
            .as_str()
            .expect("a hash")[..12]
            .to_string()
    };
    let (hidden, seen) = (id(&h, "s1"), id(&h, "s2"));
    let text = format!("what about [#{hidden}] and [#{seen}]?");
    let q = query_of(&ask_as(&mut h, &declared(EA), "s3", "t1", &text)[0]);
    assert!(q.ends_with(&text), "the person's words, whole: {q}");
    assert!(q.contains("refers to: we sailed to madeira."), "{q}");
    assert!(
        !q.contains("porto"),
        "what {{e,b}} said is not named to {{e,a}}: {q}"
    );
}

/// The topic is a row like any other: a round that was not present when it
/// was named -- somebody joined the call -- is not asked with it.
#[test]
fn a_topic_is_asked_with_only_by_a_round_present_for_it() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round_as(
        &mut h,
        &declared(EA),
        "s1",
        "t1",
        "we are moving",
        "Good luck with it.",
    );
    memory_section_as(
        &mut h,
        &declared(EA),
        "s1",
        "t1",
        json!({"movement": "start", "name": "the move"}),
    );
    let q = query_of(&ask_as(&mut h, &declared(EAB), "s1", "t2", "and then?")[0]);
    assert_eq!(q, "and then?", "{{e,a,b}} was not present for the topic");
    let q = query_of(&ask_as(&mut h, &declared(EA), "s1", "t3", "and then?")[0]);
    assert_eq!(q, "[topic: the move] and then?");
}

/// A find is handed to a round only if the round its gap was asked in holds
/// it: {e,a,b,c} -- a wider round of the same session -- is not handed what
/// {e,a} was told, and the find is not marked done for it; it waits, and the
/// next round of {e,a} begins with it.
#[test]
fn an_addendum_is_handed_only_to_a_round_its_find_was_for() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round_as(&mut h, &declared(EA), "s1", "t1", "hi", "hello");
    let ask = gap_as(&mut h, &declared(EA), "s1", "t1", "the ferry")[0].clone();
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        "- Hannah took the ferry Estrela do Norte",
    );
    assert_eq!(marks(&h, "addendum").len(), 1, "the find is kept");
    let call = round_as(
        &mut h,
        &declared(EABC),
        "s1",
        "t2",
        "and when does it leave?",
        "At nine.",
    );
    assert!(
        !window(&call).iter().any(|x| x.contains("Estrela do Norte")),
        "{:?}",
        window(&call)
    );
    assert!(
        marks(&h, "addendum_done").is_empty(),
        "a find not handed over is not done: {:?}",
        marks(&h, "addendum_done")
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM wall WHERE kind = 'recall'"),
        vec![vec![json!(0)]],
        "and not in the wall"
    );
    let call = round_as(&mut h, &declared(EA), "s1", "t3", "and then?", "Home.");
    let t = window(&call);
    assert!(
        t.iter()
            .any(|x| x.starts_with(ADDENDUM) && x.contains("Estrela do Norte")),
        "the round it was found for begins with it: {t:?}"
    );
    let done = marks(&h, "addendum_done");
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(done[0].1, "t3");
    assert_eq!(done[0].2["delivered"], 1);
}

/// Every mark the push writes carries the round of the message that caused it
/// in the ledger's one form (sorted, no duplicates, no whitespace): the gap's
/// round, which rides the gap's ask out and its answer home (the addendum),
/// and the round the find is handed to (`addendum_done`). A round nobody
/// declared writes `[]` (PP-BD-12, GH #932) and the lookup goes on: the mark
/// is a round-less row of its session, found by the round-less reads of that
/// session alone -- NULL is left to rows from before the rule.
#[test]
fn every_mark_of_the_push_carries_its_round() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    let messy = json!(r#"[ "member:e", "member:a", "member:e" ]"#);
    round_as(&mut h, &messy, "s1", "t1", "hi", "hello");
    let ask = gap_as(&mut h, &messy, "s1", "t1", "the ferry")[0].clone();
    assert_eq!(
        ask.context.get("audience_set"),
        Some(&messy),
        "the gap's ask leaves in the round it was asked in"
    );
    gap_bundle(
        &mut h,
        &ask,
        json!({}),
        "- Hannah took the ferry Estrela do Norte",
    );
    round_as(
        &mut h,
        &messy,
        "s1",
        "t2",
        "and when does it leave?",
        "At nine.",
    );
    let canon = Some(EA_CANON.to_string());
    assert_eq!(
        push_marks(&h),
        vec![
            ("gap".to_string(), canon.clone()),
            ("addendum".to_string(), canon.clone()),
            ("addendum_done".to_string(), canon),
        ]
    );
    let asks = gap_as(&mut h, &Value::Null, "s2", "t1", "the bridge");
    assert_eq!(asks.len(), 1, "looked up all the same: {:?}", h.out);
    assert_eq!(
        push_marks(&h).pop(),
        // PP-BD-12 (GH #932): round-less is `[]`, no longer NULL.
        Some(("gap".to_string(), Some("[]".to_string()))),
        "no round, the empty set"
    );
}

// ======================================================= GH #1600 the gap's echo

/// The memory's answer to a gap's ask as the memory hive builds it at tier 1:
/// the readable text with its frame (the header, the section heads and the
/// caveat line), the payload JSON the model reads (`system.memory.bundle`) and
/// the record of the same candidates in the same order (`recall_diagnostic`),
/// which carries the session each row was said in. A row is
/// `(kind, session, who, text)`.
fn recall_body(rows: &[(&str, &str, &str, &str)]) -> Value {
    let mut lines = vec!["WHAT THIS MEMORY HOLDS (as of 2026-11-23)".to_string()];
    let facts: Vec<_> = rows.iter().filter(|r| r.0 == "fact").collect();
    let said: Vec<_> = rows.iter().filter(|r| r.0 != "fact").collect();
    if !facts.is_empty() {
        lines.push("FACTS (extracted, canonical, dated)".into());
        for r in &facts {
            lines.push(format!("  {}", r.3));
        }
    }
    if !said.is_empty() {
        lines.push("WHAT WAS SAID (verbatim, not interpreted)".into());
        lines.push(
            "  (an earlier answer claiming something is NOT stored is evidence about \
             that answer and nothing else -- never a measurement of this memory)"
                .into(),
        );
        for r in &said {
            lines.push(format!("  {} on 2026-11-23: \"{}\"", r.2, r.3));
        }
    }
    let payload: Vec<Value> = rows
        .iter()
        .map(|r| {
            if r.0 == "fact" {
                json!({"kind": "fact", "text": r.3})
            } else {
                json!({"kind": r.0, "text": r.3, "who": r.2, "when": "2026-11-23"})
            }
        })
        .collect();
    let record: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({"kind": r.0, "id": format!("row-{i}"), "session_id": r.1,
                   "sender": r.2, "text": r.3, "legs": ["keyword"], "rank": i + 1})
        })
        .collect();
    let bundle = json!({"answers": "direct", "as_of": "2026-11-23T04:29:42Z",
                        "beliefs": [], "candidates": payload, "complete": true,
                        "query": "q", "tier": 1});
    json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": lines.join("\n")}],
        "recall_diagnostic": {"as_of": "2026-11-23T04:29:42Z", "candidates": record,
                              "query": "q", "tier": 1},
        "system": {"memory": {"bundle": {"text": bundle.to_string()}}},
    })
}

/// A gap of a running session's turn, as `gap_in` hands it in, with
/// `extra` on its context (a delegation's id).
fn live_gap(h: &mut Hive, session: &str, turn: &str, text: &str, extra: Value) -> Msg {
    h.out.clear();
    let mut ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                         "curator_call": "c-1", "channel": "test", "live": "call-9",
                         "audience_set": test_round()});
    if let (Some(c), Value::Object(e)) = (ctx.as_object_mut(), extra) {
        c.extend(e);
    }
    h.lane(
        "in_section",
        ctx,
        json!({"section": "gap"}),
        json!({"messages": [], "section": "gap", "payload": {"payload": text}}),
    );
    h.routed("recall")[0].clone()
}

/// The memory's whole answer to a gap's ask, as the parent's edge delivers it.
/// `extra` is put on its context too: the memory's answer comes home in the
/// context the ask left with, which carries a delegation's id (F17 run).
fn gap_bundle_body(h: &mut Hive, ask: &Msg, extra: Value, body: Value) {
    h.out.clear();
    let mut ctx = ask.context.clone();
    ctx.insert("gap_ask".into(), ask.hop["gap_ask"].clone());
    if let Value::Object(e) = extra {
        ctx.extend(e);
    }
    h.lane("in_gap_bundle", Value::Object(ctx), json!({}), body);
}

/// The words a running session's find is said with, one per `fact` sent.
fn said_facts(h: &Hive) -> Vec<String> {
    h.routed("sidecar")
        .iter()
        .filter(|m| m.hop["section"] == "fact")
        .map(|m| {
            m.body["payload"]["payload"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// GH #1600 (F17): a gap's recall that finds only turns of the running session
/// -- the answer of this very round, written a moment before the lookup -- found
/// nothing the window does not hold already: no addendum, nothing said.
#[test]
fn a_gap_that_finds_only_its_own_session_adds_nothing() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let ask = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        json!({}),
    );
    gap_bundle_body(
        &mut h,
        &ask,
        json!({}),
        recall_body(&[(
            "episode",
            "s1",
            "assistant",
            "I asked the garage whether the car is ready.",
        )]),
    );
    assert!(
        said_facts(&h).is_empty(),
        "the session's own turn is said: {:?}",
        h.out
    );
    assert!(
        marks(&h, "addendum").is_empty(),
        "{:?}",
        marks(&h, "addendum")
    );
    // Review 5: the positive signal -- the push parked it as a recall that
    // found nothing, and wrote no pending find for it.
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("found nothing") && l.contains("only turns of the running session")),
        "{:?}",
        h.stderr
    );
    assert_eq!(pending_finds(&h), 0, "a pending find is written");
}

/// GH #1600 (F17), the counter-test: a find from another session is still
/// said in a running session's turn -- and only its rows are said, never the
/// frame of the memory's answer (its header, its section heads, its caveat);
/// a row of the running session beside it is not said either.
#[test]
fn a_find_from_another_session_is_said_without_its_frame() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let ask = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        json!({}),
    );
    gap_bundle_body(
        &mut h,
        &ask,
        json!({}),
        recall_body(&[
            ("fact", "s0", "", "the car is at the Kessler garage"),
            ("episode", "s0", "user", "The garage said Thursday."),
            ("episode", "s1", "assistant", "I asked the garage."),
        ]),
    );
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?}", h.out);
    let words = &said[0];
    assert!(
        words.contains("the car is at the Kessler garage")
            && words.contains("The garage said Thursday."),
        "{words}"
    );
    for frame in [
        "WHAT THIS MEMORY HOLDS",
        "FACTS (extracted",
        "WHAT WAS SAID",
        "an earlier answer claiming",
    ] {
        assert!(
            !words.contains(frame),
            "the frame {frame:?} is said: {words}"
        );
    }
    assert!(
        !words.contains("I asked the garage."),
        "the own turn is said: {words}"
    );
    assert_eq!(marks(&h, "addendum").len(), 1);
}

/// GH #1600 (F17): a gap in a delegated turn (`context.delegation_id`) keeps
/// its find as an addendum, but says no `fact`: the delegated answer belongs
/// to the delegated cell.
#[test]
fn a_delegated_gap_keeps_its_find_and_says_no_fact() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let ask = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        json!({"delegation_id": "dl-1"}),
    );
    gap_bundle_body(
        &mut h,
        &ask,
        json!({"delegation_id": "dl-1"}),
        recall_body(&[("episode", "s0", "user", "The garage said Thursday.")]),
    );
    assert!(
        said_facts(&h).is_empty(),
        "a delegated find is said: {:?}",
        h.out
    );
    assert_eq!(
        marks(&h, "addendum").len(),
        1,
        "the find is kept: {:?}",
        h.out
    );
}

/// The pending finds in the push's state (`pending:push:gap-*`).
fn pending_finds(h: &Hive) -> i64 {
    h.rows("SELECT count(*) FROM state WHERE key LIKE 'pending:push:gap-%'")[0][0]
        .as_i64()
        .unwrap_or(-1)
}

/// `body` with `keys` put on the payload candidate `i` of its bundle: the
/// annotations the memory hands along with a row (`payload_candidate`).
fn with_payload(mut body: Value, i: usize, keys: Value) -> Value {
    let text = body["system"]["memory"]["bundle"]["text"]
        .as_str()
        .unwrap_or("{}")
        .to_string();
    let mut doc: Value = sj::from_str(&text).unwrap_or(json!({}));
    if let (Some(c), Value::Object(k)) = (doc["candidates"][i].as_object_mut(), keys) {
        c.extend(k);
    }
    body["system"]["memory"]["bundle"]["text"] = Value::String(doc.to_string());
    body
}

/// A running session's gap of `t1` in `s1`, asked; the gap's answer is
/// `body` on the context `extra`. Returns the hive.
fn live_find(body: Value, extra: Value) -> Hive {
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let ask = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        extra.clone(),
    );
    gap_bundle_body(&mut h, &ask, extra, body);
    h
}

/// GH #1600, review 1: a row's annotations are the row, not the frame -- a
/// superseded statement, an intention, one whose currency is unknown and one
/// that replaced another are said with their marks, in the memory's words.
#[test]
fn a_superseded_or_intended_find_is_said_with_its_mark() {
    if !push_shipped() {
        return;
    }
    let body = recall_body(&[
        ("fact", "s0", "", "at the Kessler garage"),
        ("fact", "s0", "", "sell the car"),
        ("fact", "s0", "", "blue"),
    ]);
    let body = with_payload(
        body,
        0,
        json!({"subject": "car", "predicate": "location", "since": "2026-11-01",
               "superseded": "at home",
               "previously": [{"claim": "at home", "until": "2026-11-01"}]}),
    );
    let body = with_payload(body, 1, json!({"intent": "intention", "superseded": ""}));
    let body = with_payload(body, 2, json!({"supersession_unknown": true}));
    let h = live_find(body, json!({}));
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let words = &said[0];
    for mark in [
        "car location: at the Kessler garage since 2026-11-01",
        "(superseded by: at home)",
        "(previously: at home until 2026-11-01)",
        "sell the car (intention) (superseded: yes)",
        "blue (currency unknown: cannot vouch that this still holds)",
    ] {
        assert!(words.contains(mark), "{mark:?} is not said: {words}");
    }
}

/// GH #1600, review 2: a delegated gap whose marks could not be read is held;
/// the next message of its round -- not delegated -- keeps the find and says
/// no `fact`: the gap's own turn decides, never the later message's context.
#[test]
fn a_held_delegated_find_stays_unsaid_in_a_later_round() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let extra = json!({"delegation_id": "dl-1"});
    let asked = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        extra.clone(),
    );
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("h-marks".into(), "store_error".into())];
    gap_bundle_body(
        &mut h,
        &asked,
        extra,
        recall_body(&[("episode", "s0", "user", "The garage said Thursday.")]),
    );
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert_eq!(marks(&h, "held").len(), 1, "held: {:?}", h.stderr);
    ask(&mut h, "s1", "t2", "and when?");
    assert_eq!(marks(&h, "addendum").len(), 1, "kept: {:?}", h.stderr);
    assert!(
        said_facts(&h).is_empty(),
        "a held delegated find is said: {:?}",
        h.out
    );
}

/// GH #1600, review 2, the other way: a held find of a turn that was not
/// delegated is said, though the next message of its round is delegated.
#[test]
fn a_held_find_is_said_though_a_later_message_is_delegated() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "is the car ready?",
        "I asked the garage.",
    );
    let asked = live_gap(
        &mut h,
        "s1",
        "t1",
        "the garage's answer is pending",
        json!({}),
    );
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("h-marks".into(), "store_error".into())];
    gap_bundle_body(
        &mut h,
        &asked,
        json!({}),
        recall_body(&[("episode", "s0", "user", "The garage said Thursday.")]),
    );
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": test_round(),
               "delegation_id": "dl-2"}),
        ask_hop("s1", "t2", "and when?"),
        json!({"messages": [user("and when?")]}),
    );
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(said[0].contains("The garage said Thursday."), "{}", said[0]);
}

/// GH #1600, review 3: a find whose kept rows carry no words is kept, and
/// nothing is said -- never the readable text with its frame and the
/// session's own turn in it.
#[test]
fn a_find_without_words_is_kept_and_not_said() {
    if !push_shipped() {
        return;
    }
    let h = live_find(
        recall_body(&[
            ("fact", "s0", "", ""),
            ("episode", "s1", "assistant", "I asked the garage."),
        ]),
        json!({}),
    );
    assert!(said_facts(&h).is_empty(), "said: {:?}", said_facts(&h));
    assert_eq!(marks(&h, "addendum").len(), 1, "kept: {:?}", h.stderr);
}

/// GH #1600, review 4: a record that does not line up with the bundle is
/// said in a line, and the rows are paired by their words -- the session's
/// own turn is still not said, the other session's row is.
#[test]
fn a_misaligned_record_is_named_and_still_filters_the_own_turn() {
    if !push_shipped() {
        return;
    }
    let mut body = recall_body(&[
        ("fact", "s0", "", "the car is at the Kessler garage"),
        ("episode", "s0", "user", "The garage said Thursday."),
        ("episode", "s1", "assistant", "I asked the garage."),
    ]);
    if let Some(recs) = body["recall_diagnostic"]["candidates"].as_array_mut() {
        recs.remove(0);
    }
    let h = live_find(body, json!({}));
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        said[0].contains("The garage said Thursday.")
            && said[0].contains("the car is at the Kessler garage"),
        "{}",
        said[0]
    );
    assert!(
        !said[0].contains("I asked the garage."),
        "the own turn is said: {}",
        said[0]
    );
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("WARN") && l.contains("record")),
        "{:?}",
        h.stderr
    );
}

/// GH #1600, review 4: a tier-0 bundle names no session; its episodes are
/// the asking session's own (the tier reads that session alone), so they are
/// not said -- what else it holds is, without its frame, and a line says so.
#[test]
fn a_tier_zero_find_says_its_rows_without_the_own_turns() {
    if !push_shipped() {
        return;
    }
    let bundle = json!({"tier": 0, "beliefs": [{"id": "b1", "statement": "the car is old"}],
                        "foresight": [{"id": "f1", "subject": "car", "predicate": "ready_on",
                                       "claim": "Thursday"}],
                        "episodes": [{"id": "e1", "sender": "assistant",
                                      "content": "I asked the garage."}],
                        "query": "q", "token_estimate": 9});
    let text = "MEMORY (tier 0, deterministic bundle)\n- belief: the car is old\n\
                - open: Thursday\n- assistant: I asked the garage.";
    let body = json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall", "text": text}],
        "system": {"memory": {"bundle": {"text": bundle.to_string()}}},
    });
    let h = live_find(body, json!({}));
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let words = &said[0];
    assert!(
        words.contains("the car is old") && words.contains("car ready_on: Thursday"),
        "{words}"
    );
    assert!(
        !words.contains("MEMORY (tier 0"),
        "the frame is said: {words}"
    );
    assert!(
        !words.contains("I asked the garage."),
        "the own turn is said: {words}"
    );
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("WARN") && l.contains("tier-0")),
        "{:?}",
        h.stderr
    );
}

/// GH #1600, review 5: a tier-2 answer is said as the one row it is, never
/// with the frame of the readable text around it.
#[test]
fn a_tier_two_answer_is_said_as_its_row() {
    if !push_shipped() {
        return;
    }
    let body = json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": "WHAT THIS MEMORY HOLDS (as of 2026-11-23)\n  The garage said Thursday."}],
        "system": {"memory": {"answer": {"text": json!({"answer": "The garage said Thursday."}).to_string()}}},
    });
    let h = live_find(body, json!({}));
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(said[0].contains("The garage said Thursday."), "{}", said[0]);
    assert!(!said[0].contains("WHAT THIS MEMORY HOLDS"), "{}", said[0]);
}

/// GH #1600, review 5: a running session's find without a session on its
/// record is no echo -- the filter needs a session to compare with -- and is
/// said.
#[test]
fn a_find_whose_record_names_no_session_is_said() {
    if !push_shipped() {
        return;
    }
    let h = live_find(
        recall_body(&[("episode", "", "user", "The garage said Thursday.")]),
        json!({}),
    );
    let said = said_facts(&h);
    assert_eq!(said.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(said[0].contains("The garage said Thursday."), "{}", said[0]);
}
