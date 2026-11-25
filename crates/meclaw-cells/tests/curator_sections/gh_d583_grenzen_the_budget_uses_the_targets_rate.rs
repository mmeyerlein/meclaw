//! GH #1526 (d58-3 grenzen, N5) -- a budget in characters is measured at the
//! rate of the model it is measured for.
//!
//! WHY this file exists: every cell of the curator turned a window of tokens
//! into characters at its own constant three (`CHARS_PER_TOKEN` of the copied
//! content-budget block), even where the catalog row of the target names its
//! own rate (`chars_per_token`) and the policy already knew it. A model that
//! reads four characters a token got three quarters of the share its row
//! allows. The rule (contract point 1): where `./policy` hands a window on,
//! the target's rate travels beside it as `chars_per_token`; every cell
//! measures with that rate and falls back to the core's one estimate (3)
//! only without one. Each test below runs one cell with a rate of 4 against
//! the same case at 3.
//!
//! The hive runs in one process (`support/curator_hive.rs`); the brief runs
//! its shipped script. Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_testing::{emit_all, shipped_script};

/// The window every case names, in tokens.
const WINDOW: u64 = 1000;
/// A tenth of it -- the share of the sidecar, the read and the section.
const SHARE: f64 = 0.10;

fn chars_at(rate: f64) -> u64 {
    (WINDOW as f64 * SHARE * rate) as u64
}

/// The map of `m` (hop first, then context) that carries `recall_input_soft`.
fn head_with_window(m: &Msg) -> Map<String, Value> {
    for map in [&m.hop, &m.context] {
        if map.contains_key("recall_input_soft") {
            return map.clone();
        }
    }
    panic!(
        "the message carries the window: {:?} {:?}",
        m.hop, m.context
    )
}

fn rate_of(map: &Map<String, Value>) -> Option<f64> {
    match map.get("chars_per_token") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────── ./policy

/// The talky hive with the main target's catalog row at `rate`, one answer
/// whose sidecar block is 350 characters long, and the next call.
fn policy_case(rate: u64) -> (Hive, Msg) {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let main = h.target.clone().expect("a main target");
    h.report_row(
        "talky",
        &main,
        json!({"model": "test/rate", "input_soft": WINDOW, "input_hard": 4 * WINDOW,
               "chars_per_token": rate}),
    );
    let block = format!("{{\"a\": \"{}\"}}", "x".repeat(341));
    assert_eq!(block.len(), 350);
    let answer = format!("noted.\n\n```sidecar\n{block}\n```");
    let call = h.curate("s1", "t1", 0, json!([user("note it")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"input_soft": WINDOW, "chars_per_token": rate}),
        json!([said(&answer)]),
    );
    let call = h.curate("s1", "t2", 0, json!([user("and now")]), mode("Be brief."));
    (h, call)
}

#[test]
fn policy() {
    if !shipped() {
        return;
    }
    // At 3 characters a token the share is 300 characters: the block of 350
    // leaves cut.
    let (_, call) = policy_case(3);
    assert!(
        texts(&call).iter().any(|t| t.contains("[dropped: sidecar")),
        "at the rate 3 the block is over its {} characters: {:?}",
        chars_at(3.0),
        texts(&call)
    );
    // At 4 it is 400: the block goes whole.
    let (h, call) = policy_case(4);
    assert!(
        !texts(&call).iter().any(|t| t.contains("[dropped: sidecar")),
        "at the row's rate 4 the block fits its {} characters: {:?} {:?}",
        chars_at(4.0),
        texts(&call),
        h.stderr
    );
    // The head that carries the window carries the rate of the same target.
    let head = head_with_window(&call);
    assert_eq!(
        rate_of(&head),
        Some(4.0),
        "the window leaves with its rate: {head:?}"
    );
}

// ─────────────────────────────────────────────────────────── ./push

/// One round of the model, answered on the tap with `usage`.
fn answer_round(h: &mut Hive, turn: &str, usage: Value) {
    let call = h.curate("s1", turn, 0, json!([user("Who is my son?")]), mode(""));
    h.tap(&call, "stop", usage, json!([said("noted")]));
}

/// The collector's ask of one turn; every `recall` that left for it.
fn recall_ask(h: &mut Hive, turn: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
               "recall_query": "And what does he do?", "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user("And what does he do?")]}),
    );
    h.routed("recall")
}

#[test]
fn push() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    answer_round(
        &mut h,
        "t1",
        json!({"input_soft": 250000, "input_hard": 1000000, "chars_per_token": 4}),
    );
    let asks = recall_ask(&mut h, "t2");
    assert_eq!(asks.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let head = head_with_window(&asks[0]);
    assert_eq!(
        head.get("recall_input_soft"),
        Some(&json!("250000")),
        "{head:?}"
    );
    assert_eq!(
        rate_of(&head),
        Some(4.0),
        "the ask carries the window's rate beside it: {head:?}"
    );
}

// ───────────────────────────────────────────── from the catalog row alone

/// Contract point 1 from the source: only the target's catalog row (the
/// package line on `in_catalog`) names the rate 4; no answer names one. The
/// window `./policy` hands on and the window of the collector's recall ask
/// leave with 4 -- the rate of the row, not one an answer said.
#[test]
fn the_rate_comes_from_the_catalog_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let main = h.target.clone().expect("a main target");
    h.report_row(
        "talky",
        &main,
        json!({"model": "test/rate", "input_soft": WINDOW, "input_hard": 4 * WINDOW,
               "chars_per_token": 4}),
    );
    let call = h.curate("s1", "t1", 0, json!([user("note it")]), mode("Be brief."));
    let head = head_with_window(&call);
    assert_eq!(
        rate_of(&head),
        Some(4.0),
        "the call's window leaves with the row's rate: {head:?} {:?}",
        h.stderr
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": WINDOW}),
        json!([said("noted")]),
    );
    let asks = recall_ask(&mut h, "t2");
    assert_eq!(asks.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let head = head_with_window(&asks[0]);
    assert_eq!(
        rate_of(&head),
        Some(4.0),
        "the recall ask carries the row's rate, not the core's: {head:?}"
    );
}

// ─────────────────────────────────────────────────────────── ./history

/// A first turn of 2 000 characters on the wall, then a `history_read` of it
/// by a call whose model names `WINDOW` and `rate`; the read's answer.
fn history_read(rate: u64) -> Value {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user(&"y".repeat(2000))]),
        mode("Be brief."),
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": 250000}),
        json!([said("ok")]),
    );
    h.out.clear();
    h.lane(
        "in_history_call",
        json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
               "audience_set": ROUND_E}),
        json!({"tool_name": "history_read", "tool_call_id": "hc-1",
               "input_soft": WINDOW, "chars_per_token": rate}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "hc-1",
                             "text": json!({"from_turn": "t1"}).to_string()}]}),
    );
    let res = h.routed("tool_result");
    assert_eq!(res.len(), 1, "one answer: {:?} {:?}", h.out, h.stderr);
    let text = res[0].messages()[0]["text"]
        .as_str()
        .expect("a text")
        .to_string();
    sj::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

#[test]
fn history_read_budget() {
    if !shipped() {
        return;
    }
    let at3 = history_read(3);
    assert_eq!(at3["budget"], json!(chars_at(3.0)), "{at3}");
    let at4 = history_read(4);
    assert_eq!(
        at4["budget"],
        json!(chars_at(4.0)),
        "the read is measured at the asking model's rate: {at4}"
    );
}

// ─────────────────────────────────────────────────────────── ./reader

/// The text of the one row a `turns` read returns for a turn of 2 000
/// characters, asked by a model with `WINDOW` and `rate`.
fn reader_text(rate: u64) -> String {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user(&"z".repeat(2000))]),
        mode("Be brief."),
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": 250000}),
        json!([said("ok")]),
    );
    h.out.clear();
    h.lane(
        "in_read",
        json!({"session_id": "s1", "audience_set": ROUND_E}),
        json!({"read_tag": "r-1", "input_soft": WINDOW, "chars_per_token": rate}),
        json!({"messages": [], "what": "turns", "session_id": "s1"}),
    );
    let read = h.routed("read");
    assert_eq!(read.len(), 1, "one answer: {:?} {:?}", h.out, h.stderr);
    let rows = read[0].body["rows"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("an answer carries rows: {:?}", read[0].body));
    rows.iter()
        .map(|r| r["text"].as_str().unwrap_or("").to_string())
        .find(|t| t.starts_with("zzz"))
        .unwrap_or_else(|| panic!("the long turn is read: {rows:?}"))
}

#[test]
fn reader() {
    if !shipped() {
        return;
    }
    let at3 = reader_text(3);
    assert!(
        at3.contains(&format!("...[cut: {} of 2000", chars_at(3.0))),
        "{at3}"
    );
    let at4 = reader_text(4);
    assert!(
        at4.contains(&format!("...[cut: {} of 2000", chars_at(4.0))),
        "the turns are cut at the asking model's rate: {at4}"
    );
}

// ─────────────────────────────────────────────────────────── ./intake

/// The shown characters of a registered section of 60 entries (about 600
/// characters as the intake measures them), cut at
/// `WINDOW` x `SHARE` by an answer whose hop names `rate`.
fn intake_shown(rate: Option<u64>) -> u64 {
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({"notes": {"lane": "sidecar", "cap_share": SHARE}}),
    )]);
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("Note down what I still have to do.")]),
        mode("Be brief."),
    );
    let mut ctx = call.context.clone();
    ctx.insert("input_soft".into(), json!(WINDOW));
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        if let Some(v) = call.hop.get(k) {
            ctx.insert(k.into(), v.clone());
        }
    }
    ctx.insert("audience_set".into(), json!(ROUND_E));
    let mut hop = json!({"section": "notes"});
    if let Some(r) = rate {
        hop["chars_per_token"] = json!(r);
    }
    let items: Vec<Value> = (1..=60)
        .map(|i| json!({"name": format!("errand-{i:03}")}))
        .collect();
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        hop,
        json!({"messages": [], "section": "notes", "payload": {"items": items}}),
    );
    let side = h.routed("sidecar");
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cut = side[0]
        .hop
        .get("cuts")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c["what"] == json!("notes")).cloned())
        .unwrap_or_else(|| panic!("60 entries are over the share: {:?}", side[0].hop));
    cut["shown"].as_u64().expect("shown")
}

#[test]
fn intake() {
    if !shipped() {
        return;
    }
    let at3 = intake_shown(None);
    assert!(at3 <= chars_at(3.0), "without a rate: three a token, {at3}");
    let at4 = intake_shown(Some(4));
    assert!(
        at4 > chars_at(3.0) && at4 <= chars_at(4.0),
        "the section is cut at the answering model's rate 4: shown {at4}, \
         budget {} (at 3: {})",
        chars_at(4.0),
        chars_at(3.0)
    );
}

// ─────────────────────────────────────────────────────────── objects/brief

#[test]
fn objects_brief() {
    let brief = "../../templates/objects/brief/config.json";
    let what = format!("{}WHAT-TAIL", "x".repeat(400));
    let row = json!({"id": "ob-0123456789ab", "rev": 4, "type": "thing",
           "aliases": json!(["Blue Bike"]).to_string(), "state": "active",
           "audience_set": r#"["member:p"]"#,
           "slots": json!({"what": what, "where": "", "who": [],
                           "when": "", "why": "", "how": ""}).to_string(),
           "refs": json!({"doc": [], "related": []}).to_string()});
    let render = |hop: Value| {
        let out = emit_all(
            &shipped_script(brief),
            &json!({"header": {"context": {"input_soft": 500}, "hop": hop},
                    "row": row, "facts": ["a fact"], "messages": []}),
        );
        out[0]["text"].as_str().expect("text").to_string()
    };
    // input_soft 500: a tenth is 50 tokens -- 150 characters at 3, 200 at 4.
    let at3 = render(json!({"route": "brief_render", "reply_cell": "tools"}));
    assert!(at3.contains("...[cut: 150 of "), "{at3}");
    let at4 = render(json!({"route": "brief_render", "reply_cell": "tools",
                            "chars_per_token": 4}));
    assert!(
        at4.contains("...[cut: 200 of "),
        "the brief is cut at the asking model's rate: {at4}"
    );
}

// ─────────────────────────── from the catalog row alone, cell by cell

/// The talky hive whose main target's catalog row (and only that row) names
/// `WINDOW` and the rate 4, with one long turn on the wall answered without
/// any rate: the state of the wall after it.
fn row_rate_hive(turn: &str) -> Hive {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let main = h.target.clone().expect("a main target");
    h.report_row(
        "talky",
        &main,
        json!({"model": "test/rate", "input_soft": WINDOW, "input_hard": 4 * WINDOW,
               "chars_per_token": 4}),
    );
    let call = h.curate("s1", "t1", 0, json!([user(turn)]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"input_soft": WINDOW}),
        json!([said("ok")]),
    );
    h
}

/// `./handover` sizes the block a renewed or new session starts with by the
/// window and the rate `./policy` keeps for the wall (`served_rate`, the
/// state key `chars_per_token` beside `input_soft`): after an answer that
/// names no rate the key is the rate of the wall target's catalog row.
#[test]
fn handover_reads_the_rate_of_the_catalog_row() {
    if !shipped() {
        return;
    }
    let h = row_rate_hive("note it");
    assert_eq!(
        h.state("chars_per_token"),
        "4.0",
        "the wall's rate is the row's, no answer named one: {:?}",
        h.stderr
    );
    assert_eq!(h.state("input_soft"), WINDOW.to_string());
}

/// `./history` measures a read whose call names no window by the window and
/// the rate `./policy` keeps for the wall: the row's rate, no answer's.
#[test]
fn history_read_counts_at_the_rate_of_the_catalog_row() {
    if !shipped() {
        return;
    }
    let mut h = row_rate_hive(&"y".repeat(2000));
    h.out.clear();
    h.lane(
        "in_history_call",
        json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
               "audience_set": ROUND_E}),
        json!({"tool_name": "history_read", "tool_call_id": "hc-1"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "hc-1",
                             "text": json!({"from_turn": "t1"}).to_string()}]}),
    );
    let res = h.routed("tool_result");
    assert_eq!(res.len(), 1, "one answer: {:?} {:?}", h.out, h.stderr);
    let turns = res[0].messages();
    let text = turns[0]["text"].as_str().expect("a text");
    let got: Value = sj::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"));
    assert_eq!(
        got["budget"],
        json!(chars_at(4.0)),
        "the read is measured at the row's rate: {got}"
    );
}

/// The view of a judge (`./history` on `in_view_ask`) is counted at the rate
/// of the judge's catalog row (its package line on `in_limits_row`), and the
/// view says that rate back beside its budget (`chars_per_token`), so the
/// asker (talky's `./route`, pinned in
/// `assemble/gh1279_a_how_question_comes_back_to_the_stamp.rs`) counts what it
/// adds at the same rate. At the core's rate the view names none, as before.
fn judge_view(rate: u64) -> Msg {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.report_row(
        "talky",
        "decide",
        json!({"model": "test/judge", "input_soft": 40000, "input_hard": 80000,
               "chars_per_token": rate}),
    );
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user(&"v".repeat(3000))]),
        mode("Be brief."),
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": 250000}),
        json!([said("ok")]),
    );
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t9", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": 4}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    views.into_iter().next().expect("a view")
}

#[test]
fn the_judge_view_says_the_rate_of_the_catalog_row() {
    if !shipped() {
        return;
    }
    let at4 = judge_view(4);
    assert!(at4.body["budget_tokens"].is_u64(), "{:?}", at4.body);
    assert_eq!(
        at4.body["chars_per_token"],
        json!(4.0),
        "the view names the row's rate beside its budget: {:?}",
        at4.body
    );
    let at3 = judge_view(3);
    assert_eq!(at3.body["budget_tokens"], at4.body["budget_tokens"]);
    assert!(
        at3.body.get("chars_per_token").is_none(),
        "at the core's rate the view is the bytes of before: {:?}",
        at3.body
    );
}

// ───────────── intake and objects/brief: the rate the composite lifts

/// The shipped edges of the composition `comp`.
fn edges_of(comp: &str) -> Vec<Value> {
    read_json(&repo(&format!("templates/{comp}/config.json")))["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// `h` with the modifier of `edge` applied, under the colony's own CEL
/// evaluator.
fn lifted(edge: &Value, h: &Headers) -> Headers {
    let spec: ModifierSpec = sj::from_value(edge["modifier"].clone()).expect("modifier spec");
    let m = parse_modifier(&spec).expect("the modifier compiles");
    apply_modifier(&m, h).expect("the modifier evaluates")
}

/// The message `h` crosses the one shipped edge `from -> to` of `comp` whose
/// condition takes it.
fn cross(comp: &str, from: &str, to: &str, h: &Headers) -> Headers {
    let edges = edges_of(comp);
    let hit: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && e["condition"].as_str().is_none_or(|c| {
                    let cond = parse_condition(c).expect("the condition compiles");
                    evaluate_condition(&cond, &h.context, &h.hop).unwrap_or(false)
                })
        })
        .collect();
    assert_eq!(hit.len(), 1, "{comp}: one edge {from} -> {to} takes {h:?}");
    lifted(hit[0], h)
}

/// The context the main model of talky is called with, when only its catalog
/// row names a rate (4) and no answer does: the curator's model call (whose
/// head carries the window and the rate of the wall target) over the shipped
/// edge to the model. `over` are overrides of the hive.
fn model_call_context(over: &[(&str, &str, Value)]) -> (Hive, Headers) {
    let mut h = Hive::with(over).reporting_composite("talky");
    let main = h.target.clone().expect("a main target");
    h.report_row(
        "talky",
        &main,
        json!({"model": "test/rate", "input_soft": WINDOW, "input_hard": 4 * WINDOW,
               "chars_per_token": 4}),
    );
    let call = h.curate("s1", "t1", 0, json!([user("note it")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"input_soft": WINDOW}),
        json!([said("ok")]),
    );
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Note down what I still have to do.")]),
        mode("Be brief."),
    );
    let target = call
        .hop
        .get("target")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .unwrap_or("brain")
        .to_string();
    let outer: Map<String, Value> = json!({"audience_set": ROUND_E, "channel": "test",
                                           "session_id": "s1"})
    .as_object()
    .cloned()
    .expect("an object");
    let h_in = Headers::from_parts(outer, call.hop.clone());
    let lifted = cross("talky", "./curator", &format!("./{target}"), &h_in);
    let soft = match lifted.context.get("recall_input_soft") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    };
    assert_eq!(
        soft,
        Some(WINDOW as f64),
        "the window of the row rides into the call: {lifted:?}"
    );
    (h, lifted)
}

/// Every edge of a composition that lifts the window of the model package
/// into the context (`recall_input_soft`) lifts its rate beside it, from the
/// same head (contract point 1, oc1 ruling on the rate of intake and brief).
#[test]
fn every_lift_of_the_window_lifts_its_rate() {
    if !shipped() {
        return;
    }
    let head: Map<String, Value> = json!({"recall_input_soft": "1000", "chars_per_token": 4})
        .as_object()
        .cloned()
        .expect("an object");
    let mut seen = 0;
    for comp in ["talky", "cogny", "simple-agent", "member"] {
        for e in edges_of(comp) {
            if e["modifier"]["set_context"]["recall_input_soft"].is_null() {
                continue;
            }
            seen += 1;
            // The pair alone: the other keys of the edge read other heads.
            let sc = &e["modifier"]["set_context"];
            let mut pair = json!({"recall_input_soft": sc["recall_input_soft"]});
            if !sc["chars_per_token"].is_null() {
                pair["chars_per_token"] = sc["chars_per_token"].clone();
            }
            let out = lifted(
                &json!({"modifier": {"set_context": pair}}),
                &Headers::from_parts(Map::new(), head.clone()),
            );
            assert_eq!(
                out.context.get("chars_per_token"),
                Some(&json!(4)),
                "{comp}: {} -> {} lifts the window without its rate",
                e["from"],
                e["to"]
            );
        }
    }
    // GH #1584: talky's two lifts onto `./quick_spec` went with the cell.
    assert_eq!(seen, 15, "talky 6, cogny 1, simple-agent 5, member 3");
}

/// `./intake` cuts a registered section by the window and the rate the
/// composite lifted with the model call: the rate of the catalog row, no
/// answer named one.
#[test]
fn intake_counts_at_the_rate_of_the_catalog_row() {
    if !shipped() {
        return;
    }
    let (mut h, lifted) = model_call_context(&[(
        "intake",
        "sections",
        json!({"notes": {"lane": "sidecar", "cap_share": SHARE}}),
    )]);
    let items: Vec<Value> = (1..=60)
        .map(|i| json!({"name": format!("errand-{i:03}")}))
        .collect();
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(lifted.context.clone()),
        json!({"section": "notes"}),
        json!({"messages": [], "section": "notes", "payload": {"items": items}}),
    );
    let side = h.routed("sidecar");
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let shown = side[0]
        .hop
        .get("cuts")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c["what"] == json!("notes")).cloned())
        .unwrap_or_else(|| panic!("60 entries are over the share: {:?}", side[0].hop))["shown"]
        .as_u64()
        .expect("shown");
    assert!(
        shown > chars_at(3.0) && shown <= chars_at(4.0),
        "the section is cut at the row's rate 4: shown {shown}, budget {} (at 3: {})",
        chars_at(4.0),
        chars_at(3.0)
    );
}

/// `objects/brief` renders a brief the model asked for by tool at the rate
/// the composite lifted with the model call: the call leaves talky over the
/// shipped tool exit (the answer names its window, no rate) and the brief
/// is cut at the row's rate 4.
#[test]
fn objects_brief_counts_at_the_rate_of_the_catalog_row() {
    if !shipped() {
        return;
    }
    let (_, lifted) = model_call_context(&[]);
    let back = Headers::from_parts(
        lifted.context.clone(),
        json!({"route": "tool", "tool_name": "object_read", "tool_call_id": "x",
               "turn_id": "t2", "input_soft": WINDOW})
        .as_object()
        .cloned()
        .expect("an object"),
    );
    let out = cross("talky", "./dispatcher", ".", &back);
    let what = format!("{}WHAT-TAIL", "x".repeat(600));
    let row = json!({"id": "ob-0123456789ab", "rev": 4, "type": "thing",
           "aliases": json!(["Blue Bike"]).to_string(), "state": "active",
           "audience_set": r#"["member:p"]"#,
           "slots": json!({"what": what, "where": "", "who": [],
                           "when": "", "why": "", "how": ""}).to_string(),
           "refs": json!({"doc": [], "related": []}).to_string()});
    let doc = emit_all(
        &shipped_script("../../templates/objects/brief/config.json"),
        &json!({"header": {"context": Value::Object(out.context.clone()),
                           "hop": {"route": "brief_render", "reply_cell": "tools"}},
                "row": row, "facts": ["a fact"], "messages": []}),
    );
    let text = doc[0]["text"].as_str().expect("text");
    // A tenth of 1000 tokens: 300 characters at 3, 400 at the row's 4.
    assert!(
        text.contains(&format!("...[cut: {} of ", chars_at(4.0))),
        "the brief is cut at the row's rate: {text} (context {:?})",
        out.context
    );
}

/// The bare rate (`chars_per_token`) is the wall's, paired with its bare
/// window (`input_soft`, written only by an answer of the target that holds
/// the wall): an answer of another target -- here a side call the
/// projection names no target for -- never writes it, so `./push`,
/// `./history` and `./handover` never count the wall's window at another
/// model's rate.
#[test]
fn a_foreign_answer_leaves_the_rate_of_the_wall() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let main = h.target.clone().expect("a main target");
    h.report_row(
        "talky",
        &main,
        json!({"model": "test/wall", "input_soft": WINDOW, "input_hard": 4 * WINDOW}),
    );
    let call = h.curate("s1", "t1", 0, json!([user("note it")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"input_soft": WINDOW, "chars_per_token": 4}),
        json!([said("ok")]),
    );
    assert_eq!(h.state("chars_per_token"), "4.0", "{:?}", h.stderr);
    let mut side = call.clone();
    side.hop.insert("curator_call".into(), json!("side-1"));
    side.context.insert("call_kind".into(), json!("side"));
    h.tap(
        &side,
        "stop",
        json!({"input_soft": 8 * WINDOW, "chars_per_token": 2}),
        json!([said("aside")]),
    );
    assert_eq!(
        h.state("chars_per_token"),
        "4.0",
        "a side answer leaves the wall's rate: {:?}",
        h.stderr
    );
    assert_eq!(h.state("input_soft"), WINDOW.to_string());
}
