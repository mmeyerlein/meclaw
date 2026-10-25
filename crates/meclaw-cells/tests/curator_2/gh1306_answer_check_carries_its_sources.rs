//! GH #1306 -- the sent answer reaches its checker with the words it answered.
//!
//! The lane `answer_check` (GH #1255) carries the sent answer alone
//! (`verify_answer`): not a word of the person and not the recall of the round
//! (`templates/talky/README.md`, "The sent answer reaches its declared
//! checker"). A conscience that checks what the answer claims against what the
//! person said and what the memory gave has no source to check against. The
//! words are held in one place, the talky's curator, which already checks a
//! section's quotes against them (`sidecar_verify` `turn+recall`, GH #1079).
//! So the splitter's knob `answer_check_sources` (`""` by default, or
//! `turn+recall`) stamps each check on its hop, the talky routes a stamped
//! check through its curator on `in_answer_check`, and the curator's intake
//! reads the round's rows and hands the check on on `answer_check` with
//! `verify_turn` (the words of this turn's participants) and `verify_recall`
//! (`[{ref, text}]`, the recall answers that stood in the window of this
//! answer, with their short block id).
//!
//! What is pinned here: without the knob the splitter's output is byte for
//! byte today's and the check leaves the rim straight from the splitter; with
//! it the check carries the turn and the recall of its window, in order, and
//! leaves the rim through the curator; the words are the turn's own (an
//! earlier turn's stay out), the recall is what the window showed (an earlier
//! turn's while the window plan shows it, another round's never -- owner
//! ruling on OR-agent-2.C5.1), nothing of them rides a section on `sidecar`,
//! and a read that failed is said, never an empty source.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const INTAKE: &str = "templates/curator/intake/config.json";
const TALKY: &str = "templates/talky/config.json";
const CURATOR: &str = "templates/curator/config.json";
const SOURCES: &str = "turn+recall";

fn abs(rel: &str) -> String {
    repo(rel).to_string_lossy().to_string()
}

/// The splitter's params as shipped, `answer_check` ["claims"] and
/// `answer_check_sources` set to `sources` or left out.
fn splitter_params(sources: Option<Value>) -> Value {
    let mut p = read_json(&repo(SPLITTER))["params"].clone();
    let o = p.as_object_mut().expect("params");
    o.remove("script_inline");
    o.insert("answer_check".into(), json!(["claims"]));
    match sources {
        Some(s) => {
            o.insert("answer_check_sources".into(), s);
        }
        None => {
            o.remove("answer_check_sources");
        }
    }
    p
}

fn split_raw(finish: &str, turns: &Value, sources: Option<Value>) -> String {
    let doc = json!({
        "header": {"hop": {"finish_reason": finish},
                   "context": {"session_id": "s1", "turn_id": "t2"}},
        "messages": turns,
        "params": splitter_params(sources),
    });
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&abs(SPLITTER)),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "the splitter exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

fn split(finish: &str, turns: &Value, sources: Option<Value>) -> Vec<Value> {
    match sj::from_str::<Value>(&split_raw(finish, turns, sources)).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

fn the_check(out: &[Value]) -> Value {
    let c: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == json!("answer_check"))
        .collect();
    assert_eq!(c.len(), 1, "one check: {out:#?}");
    c[0].clone()
}

fn answer_with(prose: &str, block: &str) -> Value {
    json!([said(&format!("{prose}\n\n```sidecar\n{block}\n```"))])
}

fn table(base: &str, rel: &str) -> EdgeTable {
    let hp: HiveParams = sj::from_value(read_json(&repo(rel))["params"].clone())
        .unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let at = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&at(&spec.from)),
            to: Path::new(&at(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("{rel}: condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

/// `(target, hop out)` of every edge out of `from` the hop takes.
fn deliveries(t: &EdgeTable, from: &str, hop: &Value) -> Vec<(String, Map<String, Value>)> {
    let hs = Headers::from_parts(
        obj(json!({"session_id": "s1", "turn_id": "t2", "curator_call": "k"})),
        obj(hop.clone()),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out.hop.clone()))
        .collect()
}

fn targets(d: &[(String, Map<String, Value>)]) -> Vec<String> {
    d.iter().map(|(to, _)| to.clone()).collect()
}

/// The check on `in_answer_check` as the talky's edge hands it to the curator:
/// the splitter's hop with the lane set, the context of the call.
fn hand_in(h: &mut Hive, call: &Msg, check: &Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    let mut hop = check["header"].clone();
    hop.as_object_mut().expect("hop").remove("route");
    let mut body = check.clone();
    body.as_object_mut().expect("body").remove("header");
    h.out.clear();
    h.lane("in_answer_check", Value::Object(ctx), hop, body);
}

const PROSE: &str = "Your tomatoes on the balcony should be ripe soon.";
const ASK: &str = "Will my tomatoes be ripe for the party?";
const RECALL: &str = "The person grows tomatoes on the balcony every summer.";
const EARLIER_RECALL: &str = "The person lives on the fourth floor.";

/// The second round of session `s1` (turn `t2`): the person's words and one
/// recall pair. The first round (`t1`) said other words and had its own recall.
fn two_rounds(h: &mut Hive) -> Msg {
    let first = h.curate(
        "s1",
        "t1",
        0,
        json!([
            user("I bought a new watering can today."),
            tool_call("r0", "memory_recall"),
            tool_result("r0", EARLIER_RECALL)
        ]),
        mode("Be brief."),
    );
    h.tap(&first, "stop", json!({}), json!([said("Nice, enjoy it.")]));
    h.curate(
        "s1",
        "t2",
        0,
        json!([
            user(ASK),
            tool_call("r1", "memory_recall"),
            tool_result("r1", RECALL)
        ]),
        mode("Be brief."),
    )
}

// ------------------------------------------------------------ 1. no knob, today's bytes

#[test]
fn gh1306_answer_check_without_sources_is_todays_bytes() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(SPLITTER));
    assert_eq!(cfg["params"]["answer_check_sources"], json!(""));
    let setting = &cfg["contract"]["settings"]["answer_check_sources"];
    assert_eq!(setting["type"], "string");
    assert_eq!(setting["default"], json!(""));
    let fixtures: Vec<(&str, Value)> = vec![
        ("stop", json!([said(PROSE)])),
        ("stop", answer_with(PROSE, r#"{"claims": {"claims": []}}"#)),
        ("length", json!([said(PROSE)])),
        (
            "tool_calls",
            json!([said("Moment."), tool_call("c1", "web_search")]),
        ),
    ];
    for (finish, turns) in &fixtures {
        let without = split_raw(finish, turns, None);
        for knob in [json!(""), json!("  "), json!(null)] {
            assert_eq!(
                split_raw(finish, turns, Some(knob.clone())),
                without,
                "{finish}: the knob {knob} is no knob"
            );
        }
        assert!(!without.contains("answer_check_sources"), "{without}");
    }
    // A value the knob does not know stamps nothing either -- and says so.
    let turns = json!([said(PROSE)]);
    assert_eq!(
        split_raw("stop", &turns, Some(json!("turn+gossip"))),
        split_raw("stop", &turns, None)
    );
    // Unstamped, the check leaves the rim straight from the splitter, as before.
    let t = table("/t", TALKY);
    let plain = the_check(&split("stop", &turns, None));
    assert_eq!(
        targets(&deliveries(&t, "/t/splitter", &plain["header"])),
        vec!["/t"]
    );
    // Stamped, it goes to the curator alone, on its own lane.
    let stamped = the_check(&split("stop", &turns, Some(json!(SOURCES))));
    assert_eq!(stamped["header"]["answer_check_sources"], json!(SOURCES));
    let d = deliveries(&t, "/t/splitter", &stamped["header"]);
    assert_eq!(targets(&d), vec!["/t/curator"], "{d:?}");
    assert_eq!(d[0].1["route"], json!("in_answer_check"));
    // The splitter itself carries no source: they are the curator's.
    assert!(stamped.get("verify_turn").is_none(), "{stamped}");
    assert!(stamped.get("verify_recall").is_none(), "{stamped}");
}

// ------------------------------------------------------------ 2. turn and recall

#[test]
fn gh1306_answer_check_carries_turn_and_recall() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.check_emits = true;
    let call = two_rounds(&mut h);
    let out = split("stop", &json!([said(PROSE)]), Some(json!(SOURCES)));
    let check = the_check(&out);
    hand_in(&mut h, &call, &check);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
    let got = h.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let m = &got[0];
    assert_eq!(m.hop["section"], "claims");
    assert_eq!(m.hop["turn_id"], "t2");
    assert_eq!(m.hop["section_written"], "0");
    assert_eq!(m.hop["answer_check_sources"], SOURCES);
    assert!(m.hop.get("sources_error").is_none(), "{m:?}");
    assert_eq!(m.body["section"], "claims");
    assert_eq!(
        m.body["verify_answer"], PROSE,
        "the answer rides on unchanged"
    );
    assert_eq!(
        m.body["verify_turn"], ASK,
        "the person's words of this turn"
    );
    let earlier = tool_result("r0", EARLIER_RECALL);
    assert_eq!(
        m.body["verify_recall"],
        json!([
            {"ref": format!("#{}", short_id(&earlier)), "text": EARLIER_RECALL},
            {"ref": format!("#{}", short_id(&tool_result("r1", RECALL))), "text": RECALL}
        ]),
        "the recall answers the window showed, in wall order, with their short ids, \
         never their questions"
    );
    assert_eq!(m.body["messages"], json!([]));
    // The curator hands it out of its rim, and the talky out of its own.
    let c = table("/t/curator", CURATOR);
    assert_eq!(
        targets(&deliveries(&c, "/t/curator/intake", &json!(m.hop))),
        vec!["/t/curator"]
    );
    let t = table("/t", TALKY);
    assert_eq!(
        targets(&deliveries(&t, "/t/curator", &json!(m.hop))),
        vec!["/t"],
        "the check leaves the talky's rim, and goes nowhere else"
    );
    // Declared where it leaves: the intake's emits and the curator's rim.
    let emits = &read_json(&repo(INTAKE))["contract"]["emits"];
    let routes = emits["hop"]["route"]["values"].as_array().expect("routes");
    assert!(routes.contains(&json!("answer_check")), "{routes:?}");
    for k in ["verify_turn", "verify_recall", "verify_answer"] {
        assert!(emits["body"][k].is_object(), "body.{k} is not declared");
    }
    let rim = &read_json(&repo(CURATOR))["params"]["contract"];
    for (side, route) in [("emits", "answer_check"), ("accepts", "in_answer_check")] {
        assert!(
            rim[side]
                .as_array()
                .expect("lanes")
                .iter()
                .any(|l| l["route"] == json!(route)),
            "the curator's rim {side} `{route}`"
        );
    }
}

// ------------------------------------------------------------ 3. the round's own

#[test]
fn gh1306_answer_check_sources_stay_in_the_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = two_rounds(&mut h);
    let block = r#"{"claims": {"claims": [{"said": "ripe soon"}]}, "memory": {"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}}"#;
    let out = split("stop", &answer_with(PROSE, block), Some(json!(SOURCES)));
    // No section on `sidecar` carries the sources: only the lane does.
    for m in out
        .iter()
        .filter(|m| m["header"]["route"] == json!("sidecar"))
    {
        let s = m.to_string();
        assert!(
            !s.contains("verify_turn") && !s.contains("verify_recall"),
            "{s}"
        );
        assert!(!s.contains(ASK) && !s.contains(RECALL), "{s}");
    }
    let check = the_check(&out);
    assert_eq!(check["header"]["section_written"], json!("1"));
    hand_in(&mut h, &call, &check);
    let got = h.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let all = json!({"hop": got[0].hop, "body": got[0].body}).to_string();
    for earlier in ["I bought a new watering can today.", "Nice, enjoy it."] {
        assert!(
            !all.contains(earlier),
            "`{earlier}` of turn t1 in t2: {all}"
        );
    }
    assert_eq!(got[0].body["verify_turn"], ASK);
    assert_eq!(
        recall_texts(&got[0]),
        vec![EARLIER_RECALL, RECALL],
        "t1's recall stood in the window of t2's answer"
    );
    // Nothing else left the curator beside the check.
    assert_eq!(h.out.len(), 1, "{:?}", h.out);

    // Another round's recall never stands in it, wherever its `seq` falls.
    let elsewhere = "The person's neighbour keeps bees.";
    h.row_under(
        Some("[\"member:x\"]"),
        chrono::Utc::now(),
        "s1",
        "tx",
        "recall",
        &tool_result("rx", elsewhere),
        1,
    );
    hand_in(&mut h, &call, &check);
    let got = h.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(recall_texts(&got[0]), vec![EARLIER_RECALL, RECALL]);

    // A window plan whose cover lies past t1 shows t1's recall no more ...
    let t1_last = h.rows("SELECT max(seq) FROM wall WHERE turn_id = 't1'")[0][0]
        .as_i64()
        .expect("t1 on the wall");
    let t1_recall = h.rows(&format!(
        "SELECT seq FROM wall WHERE turn_id = 't1' AND kind = 'recall' AND hash LIKE '{}%'",
        short_id(&tool_result("r0", EARLIER_RECALL))
    ))[0][0]
        .as_i64()
        .expect("t1's recall answer on the wall");
    let key = format!("window_plan:{}", round_key(ROUND_E));
    for (plan, want) in [
        (json!({"cover": t1_last}), vec![RECALL]),
        // ... and one that keeps the row shows it again.
        (
            json!({"cover": t1_last, "keep": [t1_recall]}),
            vec![EARLIER_RECALL, RECALL],
        ),
    ] {
        h.db.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
            [&key, &canonical(&plan)],
        )
        .expect("the plan");
        hand_in(&mut h, &call, &check);
        let got = h.routed("answer_check");
        assert_eq!(got.len(), 1, "{plan}: {:?} {:?}", h.out, h.stderr);
        assert_eq!(recall_texts(&got[0]), want, "{plan}");
        assert_eq!(got[0].body["verify_turn"], ASK, "{plan}");
    }

    // A read that failed -- the turn, the round's recall, the plan or the
    // cover -- is said, never an empty source.
    for leg in ["a-src", "a-wrec", "a-plan", "a-cover"] {
        let mut h = Hive::new();
        let call = two_rounds(&mut h);
        h.ledger_may_refuse = true;
        h.refuse_legs = vec![(leg.to_string(), "query_timeout".to_string())];
        hand_in(&mut h, &call, &check);
        assert!(h.refuse_legs.is_empty(), "{leg}: the read failed");
        let got = h.routed("answer_check");
        assert_eq!(got.len(), 1, "{leg}: {:?} {:?}", h.out, h.stderr);
        assert_eq!(got[0].hop["sources_error"], "store_error", "{leg}");
        assert!(
            got[0].body.get("verify_turn").is_none(),
            "{leg}: {:?}",
            got[0]
        );
        assert!(
            got[0].body.get("verify_recall").is_none(),
            "{leg}: {:?}",
            got[0]
        );
        assert_eq!(got[0].body["verify_answer"], PROSE, "{leg}");
    }
}

/// The texts of `verify_recall`, in order.
fn recall_texts(m: &Msg) -> Vec<String> {
    m.body["verify_recall"]
        .as_array()
        .expect("verify_recall")
        .iter()
        .map(|r| r["text"].as_str().expect("text").to_string())
        .collect()
}

// ------------------------------------------------------------ 5. the window ends at the answer

const LATE_RECALL: &str = "The person's party is on Saturday.";
const NEXT_RECALL: &str = "The person likes basil with tomatoes.";

/// The check reaches the curator a hop after its answer; by then the wall may
/// hold recall the answer never saw: the result of the tool round whose
/// interim answer is checked (iteration 1 of the same turn) and the recall of
/// the next turn. Neither stood in the window of the answer, so neither is a
/// source of its check -- the window ends at the highest `seq` of the checked
/// call's own turn up to its iteration. The check of the later call sees the
/// tool round's recall and still not the next turn's.
#[test]
fn gh1306_answer_check_recall_ends_at_the_answer() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.check_emits = true;
    let call = two_rounds(&mut h);
    let check = the_check(&split("stop", &json!([said(PROSE)]), Some(json!(SOURCES))));
    let later = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user(ASK),
            tool_call("r1", "memory_recall"),
            tool_result("r1", RECALL),
            tool_call("r2", "memory_recall"),
            tool_result("r2", LATE_RECALL)
        ]),
        mode("Be brief."),
    );
    h.curate(
        "s1",
        "t3",
        0,
        json!([
            user("And what goes with them?"),
            tool_call("r3", "memory_recall"),
            tool_result("r3", NEXT_RECALL)
        ]),
        mode("Be brief."),
    );
    for (asked, want) in [
        (&call, vec![EARLIER_RECALL, RECALL]),
        (&later, vec![EARLIER_RECALL, RECALL, LATE_RECALL]),
    ] {
        hand_in(&mut h, asked, &check);
        let got = h.routed("answer_check");
        assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
        assert!(got[0].hop.get("sources_error").is_none(), "{:?}", got[0]);
        assert_eq!(
            recall_texts(&got[0]),
            want,
            "iter {}: recall that came after the answer is no source of its check",
            asked.hop["iter"]
        );
        assert_eq!(got[0].body["verify_turn"], ASK);
        assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
    }
}

// ------------------------------------------------------------ 4. every word, once, in order

fn pending(h: &Hive) -> i64 {
    h.rows("SELECT count(*) FROM state WHERE key LIKE 'pending:%'")[0][0]
        .as_i64()
        .expect("a count")
}

/// A turn with two words of the person and one of a peer gives all three, in
/// the order the round wrote them; a turn with nothing on the wall gives no
/// words but the window's recall, an empty wall an empty source and no error; a missing block and a refused block read are
/// said; and no path leaves a parked check behind.
#[test]
fn gh1306_answer_check_reads_every_word_of_its_turn_in_order() {
    if !shipped() {
        return;
    }
    let peer = json!({"origin": "peer", "type": "text", "speaker": "Anna",
                      "speaker_ref": "peer:anna", "text": "Bring the big bowl."});
    let round = json!([
        user("First the salad."),
        peer,
        user("Then the cake."),
        tool_call("r1", "memory_recall"),
        tool_result("r1", RECALL)
    ]);
    let check = the_check(&split("stop", &json!([said(PROSE)]), Some(json!(SOURCES))));

    let mut h = Hive::new();
    h.check_emits = true;
    let call = h.curate("s1", "t2", 0, round.clone(), mode("Be brief."));
    let before = pending(&h);
    hand_in(&mut h, &call, &check);
    let got = h.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        got[0].body["verify_turn"], "First the salad.\nBring the big bowl.\nThen the cake.",
        "the person's and the peer's words, in wall order, one per line"
    );
    assert_eq!(pending(&h), before, "the parked check is dropped");
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // A turn the wall holds nothing of: no words, but the round's recall
    // still stands in the window.
    let mut other = call.clone();
    other.hop.insert("turn_id".into(), json!("t-none"));
    let mut quiet = check.clone();
    quiet["header"]["turn_id"] = json!("t-none");
    hand_in(&mut h, &other, &quiet);
    let got = h.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(got[0].hop.get("sources_error").is_none(), "{:?}", got[0]);
    assert_eq!(got[0].body["verify_turn"], "");
    assert_eq!(recall_texts(&got[0]), vec![RECALL]);
    assert_eq!(pending(&h), before, "nothing parked stays behind");
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // A wall that holds nothing at all: an empty source, no error.
    let mut empty = Hive::new();
    empty.check_emits = true;
    hand_in(&mut empty, &call, &check);
    let got = empty.routed("answer_check");
    assert_eq!(got.len(), 1, "{:?} {:?}", empty.out, empty.stderr);
    assert!(got[0].hop.get("sources_error").is_none(), "{:?}", got[0]);
    assert_eq!(got[0].body["verify_turn"], "");
    assert_eq!(got[0].body["verify_recall"], json!([]));
    assert_eq!(pending(&empty), 0, "nothing parked stays behind");
    assert!(
        empty.emit_violations.is_empty(),
        "{:?}",
        empty.emit_violations
    );

    // A row whose block is gone, and a block read the store refused: said.
    let recall_hash = short_id(&tool_result("r1", RECALL));
    for broken in ["missing", "refused"] {
        let mut h = Hive::new();
        h.check_emits = true;
        let call = h.curate("s1", "t2", 0, round.clone(), mode("Be brief."));
        let before = pending(&h);
        if broken == "missing" {
            h.sql_at_phase = Some((
                "v".to_string(),
                format!("DELETE FROM blocks WHERE hash LIKE '{recall_hash}%';"),
            ));
        } else {
            h.ledger_may_refuse = true;
            h.refuse_legs = vec![("v-blocks".to_string(), "query_timeout".to_string())];
        }
        hand_in(&mut h, &call, &check);
        assert!(
            h.sql_at_phase.is_none() && h.refuse_legs.is_empty(),
            "{broken}: struck"
        );
        let got = h.routed("answer_check");
        assert_eq!(got.len(), 1, "{broken}: {:?} {:?}", h.out, h.stderr);
        assert_eq!(got[0].hop["sources_error"], "store_error", "{broken}");
        assert!(got[0].body.get("verify_turn").is_none(), "{broken}");
        assert!(got[0].body.get("verify_recall").is_none(), "{broken}");
        assert_eq!(pending(&h), before, "{broken}: the parked check is dropped");
        assert!(
            h.emit_violations.is_empty(),
            "{broken}: {:?}",
            h.emit_violations
        );
    }
}
