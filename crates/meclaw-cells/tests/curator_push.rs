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
//!   nothing written; a duplex turn also says it as a `fact`;
//! * the prompt hygiene stands in the model's `system.instructions`.
//!
//! The harness is `support/curator_hive.rs` (GH #892): the shipped
//! `script_inline` programs run under python3, the edges are the hive's own
//! `params.graph` evaluated by the colony's CEL, and every store operation runs
//! through the store cell's own dispatcher against an in-memory SQLite with the
//! shipped schema. The hive runs the role its talky instance runs
//! (`templates/talky/curator`), so the window carries short ids. The road
//! through talky, the member and the memory hive is
//! `gh895_a_fact_from_session_one_reaches_session_three.rs`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::collections::BTreeSet;

/// R2b / GH #49: a tree without the cell skips.
fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

/// The curator of a talky: the shipped hive under the role its instance runs,
/// with `(cell, param, value)` overrides on top.
fn talky(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("policy", "role", json!("talky"))];
    all.extend(over.iter().cloned());
    Hive::with(&all)
}

/// The hop of the collector's ask (collector `head` + `recall_ask`).
fn ask_hop(session: &str, turn: &str, text: &str) -> Value {
    json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
           "recall_query": text, "memory_tier": "1",
           "recall_window_from": "", "recall_window_to": ""})
}

/// The collector's ask of one turn, as its `recall_ask` spells it; returns
/// every `recall` that left the hive for it.
fn ask(h: &mut Hive, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": session, "channel": "test",
               "audience_set": "[\"member:test\"]"}),
        ask_hop(session, turn, text),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// A whole turn the way talky runs it: the ask first, the round after its
/// bundle came home, the answer on the tap. Returns the call.
fn round(h: &mut Hive, session: &str, turn: &str, text: &str, reply: &str) -> Msg {
    let asks = ask(h, session, turn, text);
    assert_eq!(
        asks.len(),
        1,
        "every ask leaves exactly once: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = h.curate(session, turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

/// The section `gap` of the answer of `turn`, as the splitter cuts it and the
/// parent hands it in (`in_section`); `payload` is the splitter's slot.
fn gap_of(h: &mut Hive, session: &str, turn: &str, payload: Value, engine: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "curator_call": "c-1", "channel": "test", "engine": engine,
               "audience_set": "[\"member:test\"]"}),
        json!({"section": "gap"}),
        json!({"messages": [], "section": "gap", "payload": payload}),
    );
    h.routed("recall")
}

/// A gap the model wrote as a string: the splitter hands it on as
/// `{payload: <text>}`.
fn gap(h: &mut Hive, session: &str, turn: &str, text: &str, engine: &str) -> Vec<Msg> {
    gap_of(h, session, turn, json!({"payload": text}), engine)
}

/// The section `memory` of the answer of `turn`, naming `topic`, as the
/// splitter cuts it and the parent hands it in (`in_section`). `./intake`
/// files the `topic` mark out of it -- the one way a live mark comes to be
/// (OR-KY-71); a mark sown by SQL would only pin the form the test assumes
/// (review I-1).
fn memory_section(h: &mut Hive, session: &str, turn: &str, topic: Value) {
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "curator_call": "c-1", "channel": "test",
               "audience_set": "[\"member:test\"]"}),
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
        "Nice. Did \"Casa Azul\" work out for her?",
    );
    round(
        &mut h,
        "s1",
        "t2",
        "yes, she loves it",
        "Good to hear that Hannah is happy.",
    );
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
    // Newest round first; `Nice`, `Did` and `Good` open a sentence and name
    // nothing, `My` too; `Hannah` and `Porto` twice are each once.
    assert_eq!(named, ["Hannah", "Casa Azul", "Porto", "2026-03-14"], "{q}");
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
        let mut h = Hive::with(&[("policy", "role", json!(role))]);
        let call = round(
            &mut h,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, how lovely.",
        );
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
    let lit = "_int(\"query_safe_chars\", ";
    let at = script.find(lit).expect("the memory's query_safe_chars") + lit.len();
    let safe: f64 = script[at..at + script[at..].find(')').unwrap()]
        .trim()
        .parse()
        .expect("a number");
    let (budget, err) = policy_scope(
        json!({"role": "talky"}),
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

/// A duplex call's find is said at once, as a `fact` -- and a `fact` leaves
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
        "duplex",
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
