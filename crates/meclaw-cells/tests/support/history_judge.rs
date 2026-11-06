//! `./history` of the curator hive alone, driven through its judge reads
//! (GH #1436): the shipped `script_inline` under python3, its store messages
//! answered by the store cell's own dispatcher on the hive's ledger -- the way
//! `fu2_h_the_judge_context_comes_from_the_wall` runs it, written once here
//! for the three locks of the topic's age instead of once per file.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// The fixed instant of every judge here (`now_utc`).
pub const NOW: &str = "2026-10-30T01:00:00Z";

/// The script of the shipped history cell and its params without it, the
/// clock fixed; `over` sets params over the shipped ones, `drop` removes them.
pub fn history_params(over: &[(&str, Value)], drop: &[&str]) -> (String, Map<String, Value>) {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    params.insert("now_utc".into(), json!(NOW));
    for (k, v) in over {
        params.insert((*k).to_string(), v.clone());
    }
    for k in drop {
        params.remove(*k);
    }
    (script, params)
}

fn step(script: &str, params: &Map<String, Value>, msg: &Msg) -> Vec<Msg> {
    let doc = json!({
        "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                     "target": "/x/curator/history",
                     "reply_to": msg.reply_to},
        "body": msg.body,
        "params": params,
    });
    let out = run_python(script, &doc.to_string());
    assert!(
        out.status.success(),
        "history exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let emitted: Value = sj::from_slice(&out.stdout).expect("history: output is JSON");
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

fn ledger_answer(db: &rusqlite::Connection, msg: &Msg) -> Msg {
    use meclaw_cells::store::ops::dispatch;
    use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
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

/// What one lane message left: the answers and every store message, in order.
pub struct Run {
    pub out: Vec<Msg>,
    pub store: Vec<Msg>,
}

/// One message on `route` into `./history` with `params`, run to rest
/// against the hive's ledger, under the standard round.
pub fn drive(
    h: &Hive,
    params: &(String, Map<String, Value>),
    route: &str,
    context: Value,
    hop: Value,
    body: Value,
) -> Run {
    let (script, params) = params;
    let mut ctx = obj(json!({"audience_set": ROUND_E}));
    ctx.extend(obj(context));
    let mut hop = obj(hop);
    hop.insert("route".into(), json!(route));
    let mut queue = VecDeque::from([Msg {
        context: ctx,
        hop,
        body: obj(body),
        reply_to: String::new(),
    }]);
    let mut run = Run {
        out: Vec::new(),
        store: Vec::new(),
    };
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 20, "the message does not come to rest");
        for out in step(script, params, &msg) {
            if out.route() != "lstore" {
                run.out.push(out);
                continue;
            }
            let mut req = out.clone();
            for (key, from) in [
                ("cur_origin", "reply_cell"),
                ("cur_phase", "phase"),
                ("cur_call", "cur_call"),
                ("cur_reason", "cur_reason"),
            ] {
                let v = out
                    .hop
                    .get(from)
                    .cloned()
                    .expect("a store message's hop key");
                req.context.insert(key.to_string(), v);
            }
            run.store.push(out);
            queue.push_back(ledger_answer(&h.db, &req));
        }
    }
    run
}

/// The one answer of a run.
pub fn only(run: &Run, route: &str) -> Msg {
    assert_eq!(run.out.len(), 1, "exactly one answer: {:?}", run.out);
    assert_eq!(run.out[0].route(), route, "{:?}", run.out[0].hop);
    run.out[0].clone()
}

/// A judge ask of the route in `session` for `turn` (the turn running).
pub fn judge_ask(
    h: &Hive,
    params: &(String, Map<String, Value>),
    session: &str,
    turn: &str,
) -> Run {
    drive(
        h,
        params,
        "in_turns_ask",
        json!({"session_id": session, "turn_id": turn}),
        json!({}),
        json!({"messages": [], "turns_ask": {"n": 2, "budget_tokens": 1500, "judge": true}}),
    )
}

/// A judge ask of a voice DELTA: `context.session_id` is the call's id, the
/// turns stand under the generation's session (live-2 K3) -- the round-wide
/// read the route stamp of a call gets.
pub fn delta_judge_ask(
    h: &Hive,
    params: &(String, Map<String, Value>),
    call: &str,
    turn: &str,
) -> Run {
    drive(
        h,
        params,
        "in_turns_ask",
        json!({"session_id": call, "turn_id": turn, "delta": "1"}),
        json!({}),
        json!({"messages": [], "turns_ask": {"n": 2, "budget_tokens": 1500, "judge": true}}),
    )
}

/// The seed of a call opening.
pub fn seed_ask(h: &Hive, params: &(String, Map<String, Value>), call: &str) -> Run {
    seed_ask_for(h, params, call, "start", 0)
}

/// The seed of `call` asked for `reason` (`start` when the call opens,
/// `renew` when the voice renews its provider session mid-call).
pub fn seed_ask_for(
    h: &Hive,
    params: &(String, Map<String, Value>),
    call: &str,
    reason: &str,
    renewal_n: u64,
) -> Run {
    drive(
        h,
        params,
        "in_seed_ask",
        json!({"call_id": call}),
        json!({"call_id": call, "session_id": call, "reason": reason,
               "renewal_n": renewal_n}),
        json!({"messages": []}),
    )
}

/// A minute of the test day.
pub fn at(minute: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("2026-10-24T09:{minute:02}:00Z"))
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

/// A `topic` mark's value in the form `./intake` writes it.
pub fn topic(name: &str, movement: &str) -> String {
    sj::to_string(&json!({"movement": movement, "name": name})).unwrap()
}

/// One turn on the wall: the person's words, the answer, and the turn's
/// `topic` mark (`name` "" = the nameless `continue` of a quiet turn).
pub fn said_turn(h: &mut Hive, minute: u32, session: &str, turn: &str, words: &str, name: &str) {
    let later = |s: i64| at(minute) + chrono::Duration::seconds(s);
    h.row(at(minute), session, turn, "user", &user(words), 1);
    h.row(
        later(20),
        session,
        turn,
        "assistant",
        &said(&format!("about {words}")),
        1,
    );
    let movement = if name.is_empty() { "continue" } else { "start" };
    h.mark(later(40), session, turn, "topic", &topic(name, movement));
}

/// One turn on the wall that leaves NO `topic` mark: a tool round without
/// its sidecar contract, an `error`/`content_filter` answer, a turn an early
/// interrupt ended, or an answer whose model left the `memory` block out
/// (`./intake` writes the mark only from that block). It is a turn all the
/// same -- the person spoke.
pub fn markless_turn(h: &mut Hive, minute: u32, session: &str, turn: &str, words: &str) {
    h.row(at(minute), session, turn, "user", &user(words), 1);
    h.row(
        at(minute) + chrono::Duration::seconds(20),
        session,
        turn,
        "assistant",
        &said("the tool did not answer"),
        1,
    );
}

/// The topic a judge answer carries.
pub fn judged_topic(a: &Msg) -> String {
    a.body["judge"]["topic"]
        .as_str()
        .unwrap_or_else(|| panic!("judge.topic is a string: {:?}", a.body))
        .to_string()
}

/// The fact line a seed opens with.
pub fn fact_line(seed: &Msg) -> String {
    seed.body["messages"][0]["text"]
        .as_str()
        .expect("a seed's first line")
        .to_string()
}

/// The aging on, with the bound `n`, the clock of the wall itself (no fixed
/// `now_utc`): for a ledger the full hive writes at the real clock.
pub fn aging_live(n: u64) -> (String, Map<String, Value>) {
    history_params(
        &[("topic_aging", json!("1")), ("topic_age_turns", json!(n))],
        &["now_utc"],
    )
}

/// The aging on, with the bound `n`.
pub fn aging(n: u64) -> (String, Map<String, Value>) {
    history_params(
        &[("topic_aging", json!("1")), ("topic_age_turns", json!(n))],
        &[],
    )
}
