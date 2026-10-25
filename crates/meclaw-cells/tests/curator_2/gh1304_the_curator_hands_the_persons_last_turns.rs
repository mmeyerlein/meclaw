//! GH #1304 (OR-EP-4) -- the curator hands `talky/route` the person's last
//! turns, so the decide call reads a turn in the light of the ones before it.
//!
//! WHY this file exists: `route` is stateless and its decide call saw only the
//! running turn (`text_of`). With `context_turns` above 0 it now asks the
//! curator on the internal lane `in_turns_ask` and gets exactly one `turns`
//! back. The source is the curator's own wall -- no second copy of the text in
//! `route` -- under the round rule of `history_read` (GH #925): only rows the
//! round may see, only the running generation (`context.session_id`), and a
//! question without a round reads nothing and is answered `no_round`.
//!
//! What is handed on is the PERSON's words alone: no assistant turn, no
//! advice, delegation, peer, app, timer or tool row; never the running turn
//! (`context.turn_id` -- whether it already stands on the wall is a race);
//! oldest first; within `budget_tokens` by the one token estimate of the
//! templates (`token-estimate v1`), the oldest turns dropped first and
//! `truncated` said. The budget is at most 4000 tokens (the decide model's
//! window is small), and a budget at or below 0, or none, is 1500.
//!
//! The history cell runs here the way `gh1225_kh_history_reads_the_whole_wall`
//! runs it: the shipped script, its store messages answered by the store
//! cell's own dispatcher on the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
use curator_hive::*;

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// One cell's step, the way the hive's code runner hands it a message.
fn step(script: &str, params: &Map<String, Value>, msg: &Msg) -> Vec<Msg> {
    let doc = json!({
        "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                     "target": "/x/curator/history",
                     "reply_to": msg.reply_to},
        "body": msg.body,
        "params": params,
    });
    let out = run_python(script, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "history exited non-zero: {err}");
    let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "history: output is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let list = match emitted {
        Value::Array(a) => a,
        other => vec![other],
    };
    list.into_iter()
        .map(|m| {
            let mut body = obj(m);
            let hop = body
                .remove("header")
                .and_then(|h| h.as_object().cloned())
                .unwrap_or_default();
            Msg {
                context: msg.context.clone(),
                hop,
                body,
                reply_to: String::new(),
            }
        })
        .collect()
}

/// The ledger's answer to one store message, in the store cell's own reply
/// shape -- or, with `refuse`, the answer of a store that refused it.
fn ledger_answer(db: &rusqlite::Connection, msg: &Msg, refuse: bool) -> Msg {
    use meclaw_cells::store::ops::dispatch;
    use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
    if refuse {
        return Msg {
            context: msg.context.clone(),
            hop: obj(json!({"operation": "select", "error_code": "store_error"})),
            body: obj(json!({"messages": []})),
            reply_to: String::new(),
        };
    }
    let calls: Vec<Value> = msg
        .messages()
        .into_iter()
        .filter(|m| m["type"] == "tool_call")
        .collect();
    let op =
        |c: &Value| -> Value { sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json") };
    let (body, hop) = if calls.len() == 1 {
        let c = &calls[0];
        let outcome = dispatch(db, &op(c))
            .unwrap_or_else(|e| panic!("a ledger op was refused ({e}): {}", c["text"]));
        build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
    } else {
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let outcome = dispatch(db, &op(c))
                    .unwrap_or_else(|e| panic!("a ledger op was refused ({e}): {}", c["text"]));
                BundleLeg::from_outcome(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
            })
            .collect();
        build_bundle_result(&legs, 0)
    };
    Msg {
        context: msg.context.clone(),
        hop,
        body: obj(body),
        reply_to: String::new(),
    }
}

/// The answer to one `in_turns_ask` and the number of ledger reads it took.
struct Answer {
    hop: Map<String, Value>,
    context: Map<String, Value>,
    body: Map<String, Value>,
    reads: usize,
}

impl Answer {
    fn turns(&self) -> Vec<(String, String)> {
        self.body["turns"]
            .as_array()
            .unwrap_or_else(|| panic!("`turns` is a list: {:?}", self.body))
            .iter()
            .map(|t| {
                (
                    t["turn_id"].as_str().expect("turn_id").to_string(),
                    t["text"].as_str().expect("text").to_string(),
                )
            })
            .collect()
    }
    fn ids(&self) -> Vec<String> {
        self.turns().into_iter().map(|(id, _)| id).collect()
    }
    fn truncated(&self) -> bool {
        self.body["truncated"]
            .as_bool()
            .expect("`truncated` is a bool")
    }
}

/// One question on `in_turns_ask` during turn `t-now` of session `s-now`, in
/// the round `round` (`None`: the context names none), asking `ask` (the
/// body's `turns_ask`). With `refuse_store`, every ledger read is refused.
fn ask(h: &Hive, round: Option<&str>, ask: Value, refuse_store: bool) -> Answer {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    let mut context = json!({"session_id": "s-now", "turn_id": "t-now", "channel": "phone",
                             "route_state": "asked", "route_asked_at": "17"});
    if let Some(r) = round {
        context["audience_set"] = json!(r);
    }
    let first = Msg {
        context: obj(context),
        hop: obj(json!({"route": "in_turns_ask"})),
        body: obj(json!({"messages": [], "turns_ask": ask})),
        reply_to: String::new(),
    };
    let mut queue = VecDeque::from([first]);
    let mut answers = Vec::new();
    let mut reads = 0;
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 20, "the question does not come to rest");
        for out in step(&script, &params, &msg) {
            match out.route() {
                "lstore" => {
                    reads += 1;
                    for m in out.messages() {
                        let op: Value = sj::from_str(m["text"].as_str().unwrap_or("")).unwrap();
                        assert_eq!(op["operation"], "select", "a read writes nothing: {op}");
                    }
                    let mut req = out.clone();
                    for (key, from) in [
                        ("cur_origin", "reply_cell"),
                        ("cur_phase", "phase"),
                        ("cur_call", "cur_call"),
                        ("cur_reason", "cur_reason"),
                    ] {
                        let v = out.hop.get(from).cloned().unwrap_or_else(|| {
                            panic!("a store message without `hop.{from}`: {:?}", out.hop)
                        });
                        req.context.insert(key.to_string(), v);
                    }
                    queue.push_back(ledger_answer(&h.db, &req, refuse_store));
                }
                "turns" => answers.push(out),
                other => panic!("history emitted on `{other}`: {:?}", out.hop),
            }
        }
    }
    assert_eq!(
        answers.len(),
        1,
        "one question, exactly one answer: {answers:?}"
    );
    let a = answers.remove(0);
    assert_eq!(
        a.body["messages"],
        json!([]),
        "the answer carries no turn of its own: {:?}",
        a.body
    );
    Answer {
        hop: a.hop,
        context: a.context,
        body: a.body,
        reads,
    }
}

fn at(minute: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("2026-10-24T09:{minute:02}:00Z"))
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

/// A person's text of `tokens` tokens by the token-estimate block: ASCII, so
/// three characters a token.
fn words(tag: &str, tokens: usize) -> String {
    let mut s = format!("{tag} ");
    while s.len() < tokens * 3 {
        s.push('w');
    }
    s
}

/// The hive's edges and contract carry the lane: in onto `./history`, out on
/// `turns` with the interior keys cleared (GH #494).
fn the_hive_carries_the_lane() {
    let hive = read_json(&repo("templates/curator/config.json"));
    let routes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(routes("accepts").contains(&"in_turns_ask".to_string()));
    assert!(routes("emits").contains(&"turns".to_string()));
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap();
    let cond = |e: &Value| e["condition"].as_str().unwrap_or("").to_string();
    assert!(
        edges.iter().any(|e| e["from"] == "."
            && e["to"] == "./history"
            && cond(e).contains("hop.route == 'in_turns_ask'")),
        "the door onto ./history"
    );
    let exit = edges
        .iter()
        .find(|e| e["from"] == "./history" && e["to"] == "." && cond(e).contains("'turns'"))
        .expect("the exit of `turns`");
    let cleared: Vec<&str> = exit["modifier"]["delete_context"]
        .as_array()
        .expect("delete_context")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(cleared.contains(&k), "the exit clears {k}: {cleared:?}");
    }
}

#[test]
fn gh1304_context_turns_feed_only_the_persons_words() {
    the_hive_carries_the_lane();
    let mut h = Hive::new();
    // Five turns of the person, 1000 tokens each, around them every other
    // kind of row the wall holds -- and the running turn, already written.
    let mut m = 0;
    let mut next = || {
        m += 1;
        at(m)
    };
    for i in 1..=5 {
        let t = format!("t{i}");
        h.row(
            next(),
            "s-now",
            &t,
            "user",
            &user(&words(&format!("person-{i}"), 1000)),
            1,
        );
        h.row(
            next(),
            "s-now",
            &t,
            "assistant",
            &said(&format!("answer-{i}")),
            1,
        );
    }
    h.row(
        next(),
        "s-now",
        "t3",
        "advice",
        &user("[advice] advice-3"),
        1,
    );
    h.row(
        next(),
        "s-now",
        "t3",
        "delegation",
        &user("[delegation] deleg-3"),
        1,
    );
    h.row(
        next(),
        "s-now",
        "t4",
        "peer",
        &json!({"origin": "peer", "type": "text",
                                                 "text": "peer-4"}),
        1,
    );
    h.row(
        next(),
        "s-now",
        "t4",
        "tool_call",
        &tool_call("c4", "memory_recall"),
        1,
    );
    h.row(
        next(),
        "s-now",
        "t4",
        "tool_result",
        &tool_result("c4", "tool-4"),
        1,
    );
    h.row(next(), "s-now", "t5", "app", &user("[app] app-5"), 1);
    h.row(
        next(),
        "s-now",
        "t-now",
        "user",
        &user("the running turn"),
        1,
    );

    let names = |a: &Answer| a.ids();
    // Within a wide budget: the newest n, oldest first, the person's alone.
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 2, "budget_tokens": 2500}),
        false,
    );
    assert_eq!(a.hop["route"], "turns");
    assert_eq!(names(&a), ["t4", "t5"], "{:?}", a.body);
    assert!(!a.truncated(), "n, not the budget, cut it: {:?}", a.body);
    for (_, text) in a.turns() {
        assert!(
            text.starts_with("person-"),
            "only the person's words: {text}"
        );
    }
    assert_eq!(a.turns()[1].1, words("person-5", 1000), "a turn whole");
    // The context rides on untouched, the interior keys are the edge's.
    assert_eq!(a.context["route_state"], "asked");
    assert_eq!(a.context["route_asked_at"], "17");
    assert_eq!(a.context["turn_id"], "t-now");
    assert!(!a.hop.contains_key("refused") && !a.hop.contains_key("degraded"));

    // More turns asked than the budget holds: the oldest go first.
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 10, "budget_tokens": 2500}),
        false,
    );
    assert_eq!(names(&a), ["t4", "t5"], "{:?}", a.body);
    assert!(a.truncated(), "the budget cut it: {:?}", a.body);
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 10, "budget_tokens": 5000}),
        false,
    );
    assert_eq!(names(&a), ["t2", "t3", "t4", "t5"], "{:?}", a.body);
    assert!(a.truncated(), "a budget over 4000 is 4000: {:?}", a.body);
    for b in [json!(0), json!(-3), Value::Null] {
        let mut q = json!({"n": 10});
        if !b.is_null() {
            q["budget_tokens"] = b.clone();
        }
        let a = ask(&h, Some(ROUND_E), q, false);
        assert_eq!(names(&a), ["t5"], "budget {b} is 1500: {:?}", a.body);
        assert!(a.truncated(), "{:?}", a.body);
    }
    // Everything fits: every turn of the person, the running one never.
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 3, "budget_tokens": 4000}),
        false,
    );
    assert_eq!(names(&a), ["t3", "t4", "t5"]);
    assert!(!a.truncated());

    // Two messages of the person in one turn are one turn, in their order.
    let mut h = Hive::new();
    h.row(at(1), "s-now", "t1", "user", &user("first line"), 1);
    h.row(at(2), "s-now", "t1", "assistant", &said("answer"), 1);
    h.row(at(3), "s-now", "t1", "user", &user("second line"), 1);
    h.row(at(4), "s-now", "t-now", "user", &user("now"), 1);
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 1, "budget_tokens": 100}),
        false,
    );
    assert_eq!(
        a.turns(),
        [("t1".to_string(), "first line\nsecond line".to_string())]
    );
    // A wall that holds nothing of the person yet answers with nothing.
    let h = Hive::new();
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 3, "budget_tokens": 100}),
        false,
    );
    assert!(a.turns().is_empty() && !a.truncated(), "{:?}", a.body);
}

#[test]
fn gh1304_context_turns_respect_the_round() {
    let mut h = Hive::new();
    let other = "[\"member:x\"]";
    let wider = "[\"member:e\",\"member:x\"]";
    h.row(
        at(1),
        "s-old",
        "o1",
        "user",
        &user("an older generation"),
        1,
    );
    h.row(at(2), "s-now", "t1", "user", &user("in the round"), 1);
    h.row_under(
        Some(other),
        at(3),
        "s-now",
        "x1",
        "user",
        &user("another round"),
        1,
    );
    h.row_under(
        None,
        at(4),
        "s-now",
        "n1",
        "user",
        &user("before the rule"),
        1,
    );
    h.row_under(
        Some(wider),
        at(5),
        "s-now",
        "w1",
        "user",
        &user("a wider audience"),
        1,
    );
    h.row_under(
        Some(other),
        at(6),
        "s-now",
        "x2",
        "user",
        &user("another round again"),
        1,
    );
    h.row(at(7), "s-now", "t-now", "user", &user("now"), 1);

    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 10, "budget_tokens": 4000}),
        false,
    );
    assert_eq!(
        a.turns(),
        [
            ("t1".to_string(), "in the round".to_string()),
            ("w1".to_string(), "a wider audience".to_string())
        ],
        "only the round's rows of the running generation"
    );
    assert!(!a.truncated());
    // The other round reads its own and nothing of ours.
    let a = ask(
        &h,
        Some(other),
        json!({"n": 10, "budget_tokens": 4000}),
        false,
    );
    assert_eq!(a.ids(), ["x1", "w1", "x2"]);

    // Without a round nothing is read: an empty answer, refused, never silence.
    let a = ask(&h, None, json!({"n": 10, "budget_tokens": 4000}), false);
    assert_eq!(a.hop["route"], "turns");
    assert_eq!(a.hop["refused"], "no_round", "{:?}", a.hop);
    assert!(a.turns().is_empty() && !a.truncated(), "{:?}", a.body);
    assert_eq!(a.reads, 0, "not one ledger read without a round");
    let a = ask(&h, Some(""), json!({"n": 10, "budget_tokens": 4000}), false);
    assert_eq!(a.hop["refused"], "no_round", "an empty round is none");
    assert_eq!(a.reads, 0);

    // A refused store read is said, never an answer that looks empty.
    let a = ask(
        &h,
        Some(ROUND_E),
        json!({"n": 10, "budget_tokens": 4000}),
        true,
    );
    assert_eq!(a.hop["route"], "turns");
    assert_eq!(a.hop["degraded"], "store", "{:?}", a.hop);
    assert!(a.turns().is_empty(), "{:?}", a.body);
}
