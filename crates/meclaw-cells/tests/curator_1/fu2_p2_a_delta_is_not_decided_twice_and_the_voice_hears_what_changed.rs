//! #1279 -- the voice hears what changed, a delta's stamp is
//! not decided twice, and a failed tool is never spoken by the quick voice.
//!
//! WHY this file exists: a duplex call runs on a provider session with its
//! own context; after each closed turn the curator tells the voice cell, as
//! ONE `live_feed` section (way `renew`), only the facts that changed since
//! the last one -- nothing when nothing changed, nothing for a text turn.
//! A voice turn is stamped while the person still speaks (`in_delta_stamp`):
//! when the final text is the delta's text, the turn's spec curate IS its
//! normal curate under the delta's stamp and the later normal curate makes
//! no second call; another text fires anew as in #1275. And a tool result
//! whose structure says it failed (its header code, named by the collector's
//! `tool_errors` slot of the curate, or an error object as its text) gives
//! the round to the brain with the note "tool not reachable" -- the quick
//! voice never speaks the raw error.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ASK: &str = "What is the weather in Berlin?";

fn seam(h: &mut Hive, turn: &str, path: &str, ask: &str, extra: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "needs": "", "families": ""});
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
}

fn duplex() -> Value {
    json!({"engine": "duplex", "call_id": "call-9"})
}

/// One closed turn: its call and the model's final answer on the tap.
fn closed_turn(h: &mut Hive, turn: &str, extra: Value) {
    seam(h, turn, "talk", "and then?", extra);
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
}

fn topic(h: &mut Hive, back_s: i64, name: &str) {
    h.mark(
        backdated(back_s),
        "s1",
        "t0",
        "topic",
        &json!({"movement": "start", "name": name}).to_string(),
    );
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

fn keys_like(h: &Hive, prefix: &str) -> Vec<Vec<Value>> {
    h.rows(&format!(
        "SELECT key, value FROM state WHERE key LIKE '{prefix}%'"
    ))
}

#[test]
fn live_feed_sends_one_fact_section_after_duplex_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "live_feed", json!("1")),
    ]);
    h.check_emits = true;
    // The topic marks lie behind the clock: one day for all of them.
    clear_of_midnight();
    topic(&mut h, 50, "choir");

    // The first closed duplex turn: ONE section, `renew`, facts only.
    closed_turn(&mut h, "t1", duplex());
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let f = &feeds[0];
    assert_eq!(f.hop["section"], json!("renew"));
    // the voice cell takes a renew at the next turn boundary only.
    assert_eq!(f.hop["at_turn_end"], json!("1"));
    assert_eq!(f.hop["call_id"], json!("call-9"));
    assert_eq!(f.body["topic"], json!("choir"), "{:?}", f.body);
    assert!(
        f.messages().is_empty(),
        "no words, only facts: {:?}",
        f.body
    );
    for k in f.body.keys() {
        assert!(
            ["messages", "notes", "inner_turns", "topic", "screen_topics"].contains(&k.as_str()),
            "a fact field only, never an instruction: {k}"
        );
    }

    // Nothing changed since: nothing is sent.
    closed_turn(&mut h, "t2", duplex());
    assert!(h.routed("live_feed").is_empty(), "unchanged: {:?}", h.out);

    // The topic moves, but the turn is a text turn: nothing.
    topic(&mut h, 30, "weather");
    closed_turn(&mut h, "t3", json!({}));
    assert!(h.routed("live_feed").is_empty(), "text turn: {:?}", h.out);

    // The next duplex turn says the move, and only the move.
    closed_turn(&mut h, "t4", duplex());
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(feeds[0].body["topic"], json!("weather"));
    assert!(!feeds[0].body.contains_key("notes"), "{:?}", feeds[0].body);

    // A turn that ends in a tool call is not closed: nothing.
    seam(&mut h, "t5", "talk", "look it up", duplex());
    let call = h.routed("brain")[0].clone();
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("x1", "web_search")]),
    );
    assert!(h.routed("live_feed").is_empty(), "open turn: {:?}", h.out);

    // A nameless `continue` mark is no topic (the seed's rule, `topic_now`):
    // the seed would say the same, so nothing is fed.
    h.mark(
        backdated(20),
        "s1",
        "t5",
        "topic",
        &json!({"movement": "continue", "name": ""}).to_string(),
    );
    closed_turn(&mut h, "t6", duplex());
    assert!(
        h.routed("live_feed").is_empty(),
        "nameless mark: {:?}",
        h.out
    );

    // A new session note: one feed that names it, and the next seed of the
    // call carries it -- what is fed is what the seed says.
    h.lane(
        "in_note",
        json!({"session_id": "s1", "audience_set": ROUND_E}),
        json!({}),
        json!({"notes": [{"id": "n1", "source": "probe", "scope": "session",
                          "text": "The daughter visits this afternoon."}]}),
    );
    closed_turn(&mut h, "t7", duplex());
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        feeds[0].body["notes"],
        json!([{"source": "probe", "id": "n1",
                "text": "The daughter visits this afternoon."}])
    );
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"call_id": "call-9", "audience_set": ROUND_E}),
        json!({"call_id": "call-9", "session_id": "call-9", "reason": "renew",
               "renewal_n": 1}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let seed_texts: Vec<String> = seeds[0]
        .messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        seed_texts
            .iter()
            .any(|t| t == "Notes: The daughter visits this afternoon."),
        "{seed_texts:?}"
    );
    // The seed wrote the facts it carried: the next closed turn feeds nothing.
    closed_turn(&mut h, "t8", duplex());
    assert!(
        h.routed("live_feed").is_empty(),
        "after the seed: {:?}",
        h.out
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

fn delta_hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!("")),
        ("policy", "delta_speculation", json!("1")),
    ]);
    h.check_emits = true;
    h
}

fn delta_stamp(h: &mut Hive, turn: &str, text_norm: &str) {
    h.out.clear();
    h.lane(
        "in_delta_stamp",
        json!({"session_id": "s1", "turn_id": turn, "delta": "1"}),
        json!({"turn_id": turn, "path": "simple"}),
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "simple", "p": 0.9,
                                         "intent": "", "needs": [], "families": ["web"],
                                         "duration": 5, "text_norm": text_norm}}),
    );
}

fn spec_curate(h: &mut Hive, turn: &str, ask: &str) {
    seam(h, turn, "", ask, json!({"spec": "1"}));
}

#[test]
fn delta_result_reused_when_final_text_equal() {
    if !shipped() {
        return;
    }
    let mut h = delta_hive();
    delta_stamp(&mut h, "t1", "what is the weather in berlin");
    assert_eq!(keys_like(&h, "delta:").len(), 1, "the delta is kept");

    // The spec curate's final text normalises to the delta's: the turn's call.
    spec_curate(&mut h, "t1", ASK);
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        !quick[0].hop.contains_key("spec"),
        "a normal call: {:?}",
        quick[0].hop
    );
    assert_eq!(quick[0].hop["path"], json!("simple"));
    assert_eq!(quick[0].hop["families"], json!("web"));
    let m = marks(&h, "delta");
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("reused"));
    assert_eq!(m[0]["turn_id"], json!("t1"));

    // The normal curate of the turn parks: no second call.
    seam(&mut h, "t1", "simple", ASK, json!({}));
    assert!(
        h.routed("quick").is_empty() && h.routed("brain").is_empty(),
        "no second call: {:?}",
        h.out
    );
    assert!(
        keys_like(&h, "spec:").is_empty(),
        "{:?}",
        keys_like(&h, "spec:")
    );
    assert!(
        keys_like(&h, "delta").is_empty(),
        "{:?}",
        keys_like(&h, "delta")
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn delta_result_refired_when_text_differs() {
    if !shipped() {
        return;
    }
    let mut h = delta_hive();
    delta_stamp(&mut h, "t1", "what is the weather");
    spec_curate(&mut h, "t1", ASK);
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        quick[0].hop["spec"],
        json!("1"),
        "#1275 as before: {:?}",
        quick[0].hop
    );
    let m = marks(&h, "delta");
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("refired"));
    assert!(
        keys_like(&h, "delta:").is_empty(),
        "the delta went with its turn"
    );

    // Without a delta: #1275 unchanged, no mark.
    spec_curate(&mut h, "t2", ASK);
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1);
    assert_eq!(quick[0].hop["spec"], json!("1"));
    assert_eq!(marks(&h, "delta").len(), 1, "no mark without a delta");
}

/// One quick round with a tool result: the curate of iteration 1 carries the
/// result `text` and, when given, the collector's `tool_errors` slot. Returns
/// the hive after it.
fn quick_round(text: &str, errors: Option<Value>) -> Hive {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    seam(&mut h, "t1", "simple", ASK, json!({"families": "web"}));
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "1", "phase": "",
                     "path": "simple", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "1",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let mut body = json!({"messages": [tool_call("c1", "web_search"), tool_result("c1", text)]});
    if let Some(e) = errors {
        body["tool_errors"] = e;
    }
    h.lane("in_curate", ctx, hop, body);
    h
}

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate'")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).unwrap())
        .collect()
}

/// GH #1279: a tool that failed is never spoken by the quick voice. The
/// failure told in PROSE ALONE -- no JSON hull, nothing in the words that says
/// "error" -- is named by the collector's `tool_errors` slot (the result's
/// header code, per call id), and the round goes to the brain with the note
/// "tool not reachable", never the raw words. An error object as the text
/// does the same without the slot; the same prose without the slot is a
/// result the quick voice speaks.
#[test]
fn quick_tool_error_never_reaches_speech() {
    if !shipped() {
        return;
    }
    const PROSE: &str = "The service is down for maintenance until noon.";
    for (case, text, errors) in [
        (
            "prose with its code",
            PROSE,
            Some(json!({"c1": "unavailable"})),
        ),
        ("an error object", "{\"error\": \"timeout\"}", None),
    ] {
        let h = quick_round(text, errors);
        assert!(
            h.routed("quick").is_empty(),
            "{case}: the quick voice never speaks a failed tool: {:?}",
            h.out
        );
        let brain = h.routed("brain");
        assert_eq!(brain.len(), 1, "{case}: {:?} {:?}", h.out, h.stderr);
        let all = texts(&brain[0]).join("\n");
        assert!(
            all.contains("tool not reachable"),
            "{case}: the note: {all}"
        );
        let esc = escalations(&h);
        assert_eq!(esc.len(), 1, "{case}: {esc:?}");
        assert_eq!(esc[0]["reason"], json!("unspeakable"), "{case}");
    }
    // The same words without the code: a result, spoken by the quick voice.
    let h = quick_round(PROSE, None);
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(h.routed("brain").is_empty(), "{:?}", h.out);
    assert!(escalations(&h).is_empty());
}

#[test]
fn fu2_p2_defaults_byte_identical() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    for (k, v) in [
        ("live_feed", json!("0")),
        ("live_feed_section", json!("renew")),
        ("delta_speculation", json!("0")),
    ] {
        assert_eq!(policy["params"][k], v, "{k}");
        assert_eq!(policy["contract"]["settings"][k]["default"], v, "{k}");
    }
    // A run with duplex turns and a delta stamp: the same calls with the knobs
    // shipped and with the knobs absent, and nothing of P2 in the ledger.
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over);
        let mut seen = Vec::new();
        delta_stamp(&mut h, "t1", "and then");
        assert!(
            h.stderr
                .iter()
                .any(|l| l.contains("delta_speculation is off")),
            "parked: {:?}",
            h.stderr
        );
        for (turn, path) in [("t1", "talk"), ("t2", "simple"), ("t3", "")] {
            seam(&mut h, turn, path, "and then?", duplex());
            for m in &h.out {
                seen.push(json!({"route": m.route(), "system": m.body.get("system"),
                                 "tool_scope": m.body.get("tool_scope"),
                                 "messages": m.body.get("messages")}));
            }
            for call in h.routed("brain").into_iter().chain(h.routed("quick")) {
                h.out.clear();
                h.tap(&call, "stop", json!({}), json!([said("fine")]));
                seen.extend(h.out.iter().map(|m| json!(m.route())));
            }
        }
        assert!(h.routed("live_feed").is_empty());
        for p in ["delta", "live_"] {
            assert!(keys_like(&h, p).is_empty(), "{p}: {:?}", keys_like(&h, p));
        }
        assert!(marks(&h, "delta").is_empty());
        seen
    };
    let shipped_run = run(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
    ]);
    let absent_run = run(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "live_feed", Value::Null),
        ("policy", "live_feed_section", Value::Null),
        ("policy", "delta_speculation", Value::Null),
    ]);
    assert_eq!(shipped_run, absent_run);
}

/// live-2 #1370: the set writes `live_feed`, `delta_speculation` (and
/// `quick_speculative`) as JSON `true` (soul2me 0.45 `04-assistant.json`);
/// `str(True)` is "True", so the feed and the delta stayed off without a word.
/// One rule for every switch of the wave, the presenter's `knob()`: JSON true,
/// "1", "true" or "on" is on.
#[test]
fn live_feed_and_delta_switches_read_json_true() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "live_feed", json!(true)),
    ]);
    clear_of_midnight();
    topic(&mut h, 50, "choir");
    closed_turn(&mut h, "t1", duplex());
    assert_eq!(
        h.routed("live_feed").len(),
        1,
        "true is on: {:?} {:?}",
        h.out,
        h.stderr
    );

    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!(true)),
        ("policy", "spec_tools", json!("")),
        ("policy", "delta_speculation", json!(true)),
    ]);
    delta_stamp(&mut h, "t1", "what is the weather in berlin");
    assert_eq!(
        keys_like(&h, "delta:").len(),
        1,
        "true is on: the delta is kept"
    );
    spec_curate(&mut h, "t1", ASK);
    let m = marks(&h, "delta");
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("reused"));
}

/// live-2 #1370 (K5): only the way `renew` is built. A `context` section
/// carries no `payload`/`text`, so the voice cell answered it `bad_section`;
/// the contract names `renew` alone and any other value leaves as `renew`,
/// said on stderr.
#[test]
fn live_feed_section_is_renew_only() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(
        policy["contract"]["settings"]["live_feed_section"]["values"],
        json!(["renew"])
    );
    let curator = read_json(&repo("templates/curator/config.json"));
    // A hive's contract lies under `params` (config.md: the hive's wiring surface).
    let feed = curator["params"]["contract"]["emits"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["route"] == "live_feed"))
        .cloned()
        .expect("the rim emits `live_feed`");
    let because = feed["because"].as_str().unwrap_or_default();
    assert!(
        because.contains("`renew`") && !because.contains("`context`"),
        "the rim names the built way only: {because}"
    );
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "live_feed", json!("1")),
        ("policy", "live_feed_section", json!("context")),
    ]);
    clear_of_midnight();
    topic(&mut h, 50, "choir");
    closed_turn(&mut h, "t1", duplex());
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(feeds[0].hop["section"], json!("renew"));
    assert_eq!(feeds[0].hop["at_turn_end"], json!("1"));
    assert!(
        h.stderr.iter().any(|l| l.contains("live_feed_section")),
        "{:?}",
        h.stderr
    );
}
