//! #1279 -- the judge context comes out of the curator's wall.
//!
//! WHY this file exists: the route's decide call and the voice cell's seed need
//! the same picture of the conversation -- the last turns of both roles, the
//! topic, the screen topics, the time and the place. `./history` builds it on
//! demand out of the ledger, so it is current after every turn the wall holds
//! without a timer: on `in_view_ask` with a target as one `view` (D58 GH
//! #1480: `messages` of both sides and the `system` tree -- `frame`, `topic`,
//! the target's contributions), and on `in_seed_ask` as one `seed` (the read
//! context under `context`). `in_part` (and the `parts` of a curate) and
//! `in_seed_line` keep what the domains and the colony tell the hive. A view without a target reads
//! as the turns lane of GH #1304 did.
//!
//! The history cell runs here the way `gh1304_the_curator_hands_the_persons_last_turns`
//! runs it: the shipped script, its store messages answered by the store
//! cell's own dispatcher on the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
use curator_hive::*;
// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;
// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// The fixed instant of every judge here (`now_utc`), a Friday.
const NOW: &str = "2026-10-30T01:00:00Z";

/// The params of the shipped history cell, with `place`/`timezone` set and
/// the clock fixed.
fn params() -> (String, Map<String, Value>) {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    params.insert("place".into(), json!("Berlin"));
    params.insert("timezone".into(), json!("Europe/Berlin"));
    params.insert("now_utc".into(), json!(NOW));
    (script, params)
}

/// One cell's step, the way the hive's code runner hands it a message.
fn step(script: &str, params: &Map<String, Value>, msg: &Msg) -> (Vec<Msg>, String) {
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
    let msgs = list
        .into_iter()
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
        .collect();
    (msgs, err)
}

/// The ledger's answer to one store message, in the store cell's own reply
/// shape.
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

/// What one lane message left: every message history emitted that is not a
/// store message, and the store messages themselves (in order).
struct Run {
    out: Vec<Msg>,
    store: Vec<Msg>,
    /// What every step of the run said on stderr, in order.
    stderr: String,
}

/// One message on `route` into `./history`, run to rest against the hive's
/// ledger. `context` over the standard one (`audience_set` the standard
/// round; a `null` removes the key).
fn drive(h: &Hive, route: &str, context: Value, hop: Value, body: Value) -> Run {
    drive_with(h, &[], route, context, hop, body)
}

/// [`drive`] with params of the cell set over the shipped ones.
fn drive_with(
    h: &Hive,
    over: &[(&str, Value)],
    route: &str,
    context: Value,
    hop: Value,
    body: Value,
) -> Run {
    let (script, mut params) = params();
    for (k, v) in over {
        params.insert((*k).to_string(), v.clone());
    }
    let mut ctx = obj(json!({"audience_set": ROUND_E}));
    for (k, v) in obj(context) {
        if v.is_null() {
            ctx.remove(&k);
        } else {
            ctx.insert(k, v);
        }
    }
    let mut hop = obj(hop);
    hop.insert("route".into(), json!(route));
    let first = Msg {
        context: ctx,
        hop,
        body: obj(body),
        reply_to: String::new(),
    };
    let mut queue = VecDeque::from([first]);
    let mut run = Run {
        out: Vec::new(),
        store: Vec::new(),
        stderr: String::new(),
    };
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 20, "the message does not come to rest");
        let (msgs, err) = step(&script, &params, &msg);
        run.stderr.push_str(&err);
        for out in msgs {
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
                let v = out.hop.get(from).cloned().unwrap_or_else(|| {
                    panic!("a store message without `hop.{from}`: {:?}", out.hop)
                });
                req.context.insert(key.to_string(), v);
            }
            run.store.push(out);
            queue.push_back(ledger_answer(&h.db, &req));
        }
    }
    run
}

/// The one answer of a run, on `route`.
fn the_answer(run: &Run, route: &str) -> Msg {
    assert_eq!(run.out.len(), 1, "exactly one answer: {:?}", run.out);
    let a = run.out[0].clone();
    assert_eq!(a.route(), route, "{:?}", a.hop);
    a
}

/// The view the route stamp asks for its judge (target `decide`) of `turn`.
fn ask_judge(h: &Hive, turn: &str, n: u64) -> Msg {
    let run = drive(
        h,
        "in_view_ask",
        json!({"session_id": "s-now", "turn_id": turn}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": n, "budget_tokens": 1500}}),
    );
    the_answer(&run, "view")
}

/// `(origin, text)` of a view's messages, in their order.
fn said_of(view: &Msg) -> Vec<(String, String)> {
    view.body["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("a view carries `messages`: {:?}", view.body))
        .iter()
        .map(|m| {
            assert_eq!(m["type"], "text", "{m}");
            (
                m["origin"].as_str().unwrap().to_string(),
                m["text"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// One expected message of a view.
fn s(origin: &str, text: &str) -> (String, String) {
    (origin.to_string(), text.to_string())
}

/// The text of the leaf `name` of a view's system tree, "" when it holds none.
fn leaf_text(view: &Msg, name: &str) -> String {
    view.body["system"]
        .get(name)
        .map(|l| l["text"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

fn seed_ask(h: &Hive, call: &str, renewal_n: u64, round: Value) -> Msg {
    seed_ask_with(h, &[], call, renewal_n, round)
}

/// [`seed_ask`] with params of the cell set over the shipped ones.
fn seed_ask_with(
    h: &Hive,
    over: &[(&str, Value)],
    call: &str,
    renewal_n: u64,
    round: Value,
) -> Msg {
    let run = drive_with(
        h,
        over,
        "in_seed_ask",
        json!({"live": call, "audience_set": round}),
        json!({"live": call, "session_id": call, "reason": "start", "renewal_n": renewal_n}),
        json!({"messages": []}),
    );
    the_answer(&run, "seed")
}

/// `(turn_id, role, text)` of a seed's read context's turns.
fn turns_of(judge: &Value) -> Vec<(String, String, String)> {
    judge["turns"]
        .as_array()
        .unwrap_or_else(|| panic!("context.turns is a list: {judge}"))
        .iter()
        .map(|t| {
            (
                t["turn_id"].as_str().unwrap().to_string(),
                t["role"].as_str().unwrap().to_string(),
                t["text"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn at(minute: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("2026-10-24T09:{minute:02}:00Z"))
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

fn topic(name: &str, movement: &str) -> String {
    sj::to_string(&json!({"movement": movement, "name": name})).unwrap()
}

/// An answer with its sidecar block, the way the intake writes it.
fn answered(text: &str) -> Value {
    said(&format!(
        "{text}\n\n```sidecar\n{{\"memory\": {{\"topic\": \"x\"}}}}\n```"
    ))
}

/// A turn of both roles on the wall.
fn exchange(h: &mut Hive, minute: u32, session: &str, turn: &str, person: &str, reply: &str) {
    h.row(at(minute), session, turn, "user", &user(person), 1);
    h.row(
        at(minute + 1),
        session,
        turn,
        "assistant",
        &answered(reply),
        1,
    );
}

fn judge_keys(judge: &Value) -> Vec<String> {
    let mut k: Vec<String> = judge.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn judge_context_updates_after_each_turn() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "how is the weather", "sunny");
    exchange(&mut h, 3, "s-now", "t2", "and tomorrow", "rain");
    h.mark(at(4), "s-now", "t2", "topic", &topic("weather", "start"));
    h.row(at(5), "s-now", "t3", "user", &user("the running turn"), 1);

    let a = ask_judge(&h, "t3", 2);
    assert!(
        a.body.get("judge").is_none() && a.body.get("turns").is_none(),
        "the package `judge` and the list `turns` are gone: {:?}",
        a.body
    );
    assert_eq!(
        said_of(&a),
        [
            s("user", "how is the weather"),
            s("assistant", "sunny"),
            s("user", "and tomorrow"),
            s("assistant", "rain"),
        ],
        "both roles, oldest first, the sidecar block cut, never the running turn"
    );
    assert_eq!(leaf_text(&a, "topic"), "weather");
    assert_eq!(
        leaf_text(&a, "frame"),
        "now: 2026-10-30T02:00:00+01:00\nweekday: Friday\ntimezone: Europe/Berlin\n\
         place: Berlin",
        "the moment: one clock, one place, one fact per line"
    );
    // No contribution, no family of the target; no hint source, no leaf.
    assert_eq!(
        judge_keys(&a.body["system"]),
        ["frame", "topic"],
        "{:?}",
        a.body
    );

    // Turn 3 closes, turn 4 asks: the judge moves on by one turn, no timer.
    h.row(
        at(6),
        "s-now",
        "t3",
        "assistant",
        &answered("an umbrella"),
        1,
    );
    h.mark(at(7), "s-now", "t3", "topic", &topic("garden", "start"));
    h.row(at(8), "s-now", "t4", "user", &user("next"), 1);
    let view = ask_judge(&h, "t4", 2);
    assert_eq!(
        said_of(&view),
        [
            s("user", "and tomorrow"),
            s("assistant", "rain"),
            s("user", "the running turn"),
            s("assistant", "an umbrella"),
        ],
        "the last k = 2 turns after turn n + 1"
    );
    assert_eq!(leaf_text(&view, "topic"), "garden");
    // A topic that ended is no topic: the view names none.
    h.mark(at(9), "s-now", "t3", "topic", &topic("garden", "end"));
    assert_eq!(leaf_text(&ask_judge(&h, "t4", 2), "topic"), "");
}

#[test]
fn judge_context_answers_seed_ask() {
    let mut h = Hive::new();
    // A call has no generation yet: the seed reads the round, any session.
    exchange(&mut h, 1, "s-old", "o1", "hello there", "hello Marta");
    exchange(&mut h, 3, "s-now", "t1", "the garden", "the roses bloom");
    h.mark(at(4), "s-now", "t1", "topic", &topic("garden", "start"));
    h.row_under(
        Some("[\"member:x\"]"),
        at(5),
        "s-now",
        "x1",
        "user",
        &user("another round"),
        1,
    );

    // A seed line waits for the call's next seed.
    let run = drive(
        &h,
        "in_seed_line",
        json!({}),
        json!({}),
        json!({"live": "c-1", "position": "tail", "order": 2, "lasts": "session",
               "text": "The call ends in two minutes."}),
    );
    assert!(run.out.is_empty(), "nothing answers a seed line");

    let a = seed_ask(&h, "c-1", 2, json!(ROUND_E));
    assert_eq!(a.hop["renewal_n"], 2, "the renewal_n given back");
    assert_eq!(a.hop["live"], "c-1");
    // live, session_id and renewal_n come back unchanged: the composition
    // addresses the seed by them, and the voice cell refuses a seed without
    // its call and renewal_n (`missing_renewal_n`).
    assert_eq!(a.hop["session_id"], "c-1", "the session_id given back");
    assert!(!a.hop.contains_key("refused") && !a.hop.contains_key("degraded"));
    let msgs = a.messages();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        [
            "developer",
            "user",
            "assistant",
            "user",
            "assistant",
            "developer"
        ]
    );
    let fact = msgs[0]["text"].as_str().unwrap();
    assert_eq!(
        fact,
        "Facts: now 2026-10-30T02:00:00+01:00 (Friday, Europe/Berlin); place: Berlin; \
         topic: garden."
    );
    let texts: Vec<&str> = msgs[1..5]
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "hello there",
            "hello Marta",
            "the garden",
            "the roses bloom"
        ]
    );
    assert_eq!(
        msgs[5]["text"], "The call ends in two minutes.",
        "the line last"
    );
    // The seed's own budget rule: bytes / 3, rounded up, per
    // message -- not a second rule beside it.
    let est: usize = msgs
        .iter()
        .map(|m| m["text"].as_str().unwrap().len().div_ceil(3))
        .sum();
    assert_eq!(a.body["est_tokens"], json!(est));
    // The read context the view is built out of, under a neutral key.
    assert!(a.body.get("judge").is_none(), "{:?}", a.body);
    let judge = &a.body["context"];
    assert_eq!(
        judge_keys(judge),
        ["now", "parts", "place", "topic", "turns"]
    );
    assert_eq!(turns_of(judge).len(), 4);

    // The line went with that seed.
    assert!(
        h.state(r#"seed_line:["c-1","tail",2]"#).is_empty(),
        "the line is dropped"
    );
    let again = seed_ask(&h, "c-1", 3, json!(ROUND_E));
    let last = again.messages().last().cloned().unwrap();
    assert_eq!(last["role"], "assistant", "no line any more: {last}");
    assert_eq!(again.hop["renewal_n"], 3);

    // The budget: 8192 tokens, the oldest turns go first, the facts stay.
    let mut big = Hive::new();
    for i in 0..12u32 {
        let mut text = format!("turn-{i} ");
        while text.len() < 3000 {
            text.push('w');
        }
        big.row(at(i), "s-now", &format!("t{i:02}"), "user", &user(&text), 1);
    }
    let a = seed_ask(&big, "c-2", 0, json!(ROUND_E));
    let msgs = a.messages();
    assert_eq!(msgs[0]["role"], "developer");
    let turns = &msgs[1..];
    assert!(
        turns.len() < 12 && !turns.is_empty(),
        "cut by the budget: {}",
        turns.len()
    );
    assert!(
        turns.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .starts_with("turn-11 "),
        "the newest stays"
    );
    let tokens: usize = msgs
        .iter()
        .map(|m| m["text"].as_str().unwrap().len().div_ceil(3))
        .sum();
    assert!(tokens <= 8192, "within 8192 tokens: {tokens}");
    assert_eq!(a.hop["renewal_n"], 0);

    // Without a round nothing is read, and the answer still comes.
    let run = drive(
        &h,
        "in_seed_ask",
        json!({"live": "c-1", "audience_set": null}),
        json!({"live": "c-1", "renewal_n": 4}),
        json!({"messages": []}),
    );
    let a = the_answer(&run, "seed");
    assert!(run.store.is_empty(), "not one ledger read without a round");
    assert_eq!(a.hop["refused"], "no_round");
    assert_eq!(a.hop["renewal_n"], 4);
    assert_eq!(a.body["messages"], json!([]));

    // The hive's rim: in on the history door, out on `seed` with the call in
    // the context and the hive's bookkeeping cleared.
    let hive = read_json(&repo("templates/curator/config.json"));
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap().clone();
    let pass = |e: &Value, ctx: &Map<String, Value>, hop: &Map<String, Value>| {
        e["condition"].as_str().is_none_or(|c| {
            matches!(
                evaluate_condition(&parse_condition(c).unwrap(), ctx, hop),
                Ok(true)
            )
        })
    };
    for lane in ["in_seed_ask", "in_seed_line", "in_part"] {
        let hop = obj(json!({"route": lane}));
        let to: Vec<&str> = edges
            .iter()
            .filter(|e| e["from"] == "." && pass(e, &Map::new(), &hop))
            .map(|e| e["to"].as_str().unwrap())
            .collect();
        assert_eq!(to, ["./history"], "{lane}");
    }
    let ctx = obj(
        json!({"cur_origin": "curator-history", "cur_phase": "seed-blocks",
                         "audience_set": ROUND_E}),
    );
    let hop = obj(json!({"route": "seed", "live": "c-9", "renewal_n": 1}));
    let exits: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == "./history" && pass(e, &ctx, &hop))
        .collect();
    assert_eq!(exits.len(), 1, "one exit for `seed`");
    assert_eq!(exits[0]["to"], ".");
    let spec: ModifierSpec = sj::from_value(exits[0]["modifier"].clone()).unwrap();
    let out = apply_modifier(
        &parse_modifier(&spec).unwrap(),
        &Headers::from_parts(ctx.clone(), hop.clone()),
    )
    .expect("the modifier applies");
    assert_eq!(out.context["live"], "c-9");
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(!out.context.contains_key(k), "{k} never leaves the hive");
    }
    let routes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    for lane in ["in_seed_ask", "in_seed_line", "in_part", "in_delta_stamp"] {
        assert!(routes("accepts").contains(&lane.to_string()), "{lane}");
    }
    for route in ["seed", "live_feed", "propose"] {
        assert!(routes("emits").contains(&route.to_string()), "{route}");
        assert!(
            edges.iter().any(|e| e["to"] == "."
                && e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains(&format!("hop.route == '{route}'"))),
            "an exit for {route}"
        );
    }
}

/// The place a line of today's senders names since D58 judge (GH #1480):
/// `in_seed_line` knows no kind any more; the set's `identity` sender is a head
/// line that lasts, its `hint` and `wind_down` senders are tail lines of the
/// running session at orders 1 and 2 (the anchor's shipped order 1.5 between).
fn place(kind: &str) -> Value {
    match kind {
        "identity" => json!({"position": "head", "lasts": "always"}),
        "hint" => json!({"position": "tail", "order": 1, "lasts": "session"}),
        "wind_down" => json!({"position": "tail", "order": 2, "lasts": "session"}),
        other => panic!("no sender of kind {other}"),
    }
}

fn seed_line(h: &Hive, call: &str, kind: &str, text: &str) {
    let mut body = place(kind);
    body["live"] = json!(call);
    body["text"] = json!(text);
    let run = drive(h, "in_seed_line", json!({}), json!({}), body);
    assert!(run.out.is_empty(), "nothing answers a seed line");
}

/// a seed line is held per call AND kind. `wind_down` and
/// `hint` stand at once, a new line replaces only its own kind, the next seed
/// of the call ends on every held line in a fixed order (`hint`, then
/// `wind_down`) and drops them all.
#[test]
fn seed_lines_are_kept_per_kind() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");

    seed_line(&h, "c-1", "wind_down", "The call ends in two minutes.");
    seed_line(&h, "c-1", "hint", "Mention the roses.");
    seed_line(&h, "c-1", "hint", "Ask about the tulips.");
    seed_line(&h, "c-2", "wind_down", "Another call ends.");
    assert_eq!(
        h.state(r#"seed_line:["c-1","tail",1]"#),
        r#"{"lasts":"session","order":1,"position":"tail","text":"Ask about the tulips."}"#,
        "the newer hint replaced the older one"
    );

    let a = seed_ask(&h, "c-1", 1, json!(ROUND_E));
    let msgs = a.messages();
    let texts: Vec<(&str, &str)> = msgs
        .iter()
        .map(|m| (m["role"].as_str().unwrap(), m["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        texts[1..],
        [
            ("user", "the garden"),
            ("assistant", "the roses bloom"),
            ("developer", "Ask about the tulips."),
            ("developer", "The call ends in two minutes."),
        ],
        "both kinds last, each once, the hint before the wind-down"
    );
    // The seed's own budget rule: bytes / 3, rounded up, per
    // message -- not a second rule beside it.
    let est: usize = msgs
        .iter()
        .map(|m| m["text"].as_str().unwrap().len().div_ceil(3))
        .sum();
    assert_eq!(a.body["est_tokens"], json!(est));

    // Both went with that seed; another call's line stays for its own seed.
    assert!(h.state(r#"seed_line:["c-1","tail",1]"#).is_empty());
    assert!(h.state(r#"seed_line:["c-1","tail",2]"#).is_empty());
    assert!(!h.state(r#"seed_line:["c-2","tail",2]"#).is_empty());
    let again = seed_ask(&h, "c-1", 2, json!(ROUND_E));
    let last = again.messages().last().cloned().unwrap();
    assert_eq!(last["role"], "assistant", "no line any more: {last}");

    // One kind alone closes the seed by itself.
    seed_line(&h, "c-1", "hint", "Only a hint.");
    let one = seed_ask(&h, "c-1", 3, json!(ROUND_E));
    let msgs = one.messages();
    assert_eq!(msgs.last().unwrap()["text"], "Only a hint.");
    assert_eq!(msgs[msgs.len() - 2]["role"], "assistant");
}

/// One contribution on `in_part` (D58 GH #1480): `value` None leaves the
/// field out (a body out of form), `subject` "" none.
fn put_part(h: &Hive, source: &str, value: Option<Value>, subject: &str) -> Run {
    let mut body = json!({"messages": [], "target": "decide", "source": source});
    if let Some(v) = value {
        body["value"] = v;
    }
    if !subject.is_empty() {
        body["subject"] = json!(subject);
    }
    drive(h, "in_part", json!({}), json!({}), body)
}

/// The leaf of `source` in the family `decide` of a view, Null when it holds
/// none.
fn decide_leaf(view: &Msg, source: &str) -> Value {
    view.body["system"]["decide"]
        .get(source)
        .cloned()
        .unwrap_or(Value::Null)
}

/// A structured leaf read back from its JSON text (the curator keeps the
/// value as it came and hands it on as one text, never read).
fn leaf_json(leaf: &Value) -> Value {
    let text = leaf["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a leaf is {{\"text\": ...}}: {leaf}"));
    sj::from_str(text).unwrap_or_else(|_| json!(text))
}

/// D58 GH #1480 (was the judge package's `screen.topics`): the presenter's
/// screen topics reach the judge as the presenter's contribution to the
/// target `decide` -- one leaf `system.decide.presenter` of the view, kept
/// as told (the value is never read: a nameless topic is the route's to
/// leave out, `kept_topics`), a new list replaces the old one whole, and the
/// seed of a session whose target is `decide` names them as facts.
#[test]
fn judge_context_holds_the_screen_topics() {
    let mut h = Hive::new();
    exchange(
        &mut h,
        1,
        "s-now",
        "t1",
        "show me the photos",
        "here they are",
    );
    let topics = |h: &Hive| decide_leaf(&ask_judge(h, "t2", 3), "presenter");
    assert_eq!(topics(&h), Value::Null, "nothing told, nothing held");

    let first = json!([
        {"name": "weather", "purpose": "the forecast"},
        {"name": "photos", "purpose": "family pictures",
         "candidates": [{"key": "p1", "describe": "the lake"},
                        {"key": "p2", "describe": "the garden"}]},
        {"purpose": "a topic without a name"}
    ]);
    let run = put_part(&h, "presenter", Some(first.clone()), "");
    assert!(run.out.is_empty(), "nothing answers the topics");
    assert_eq!(
        leaf_json(&topics(&h)),
        first,
        "kept as told, whole and unread: {:?}",
        run.stderr
    );
    // A new list replaces the old one whole.
    let calendar = json!([{"name": "calendar", "purpose": "the week"}]);
    put_part(&h, "presenter", Some(calendar.clone()), "");
    assert_eq!(leaf_json(&topics(&h)), calendar);
    // The seed of a session for the target names them as facts.
    let seed_of = |h: &Hive| {
        seed_ask_with(
            h,
            &[("seed_target", json!("decide"))],
            "c-1",
            0,
            json!(ROUND_E),
        )
    };
    let seed = seed_of(&h);
    let fact = seed.messages()[0]["text"].as_str().unwrap().to_string();
    assert!(fact.contains("presenter: calendar"), "{fact}");
    assert_eq!(
        leaf_json(&seed.body["context"]["parts"]["presenter"]),
        calendar
    );
    // A body without a value keeps the list; an empty list empties it.
    put_part(&h, "presenter", None, "");
    assert_eq!(leaf_json(&topics(&h)), calendar);
    put_part(&h, "presenter", Some(json!([])), "");
    assert_eq!(leaf_json(&topics(&h)), json!([]));
    let fact = seed_of(&h).messages()[0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!fact.contains("presenter:"), "no topic, no fact: {fact}");
}

#[test]
fn a_view_without_a_target_reads_as_the_turns_ask_did() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "first", "one");
    exchange(&mut h, 3, "s-now", "t2", "second", "two");
    h.mark(at(4), "s-now", "t2", "topic", &topic("weather", "start"));
    h.row(at(5), "s-now", "t3", "user", &user("now"), 1);
    put_part(
        &h,
        "presenter",
        Some(json!([{"name": "weather", "purpose": "the forecast"}])),
        "",
    );

    let ask = |q: Value| {
        drive(
            &h,
            "in_view_ask",
            json!({"session_id": "s-now", "turn_id": "t3"}),
            json!({}),
            json!({"messages": [], "view_ask": q}),
        )
    };
    let plain = ask(json!({"turns": 2, "budget_tokens": 1500}));
    let off = ask(json!({"turns": 2, "budget_tokens": 1500, "target": ""}));
    let text = |r: &Run| -> Vec<String> {
        r.store
            .iter()
            .chain(r.out.iter())
            .map(|m| {
                format!(
                    "{} {}",
                    Value::Object(m.hop.clone()),
                    Value::Object(m.body.clone())
                )
            })
            .collect()
    };
    assert_eq!(text(&plain), text(&off), "an empty target is none");
    // The reads and the answer of GH #1304, nothing beside them.
    assert_eq!(plain.store.len(), 2, "rows, then blocks");
    let first = plain.store[0].messages();
    assert_eq!(first.len(), 1, "one read: {first:?}");
    assert_eq!(first[0]["id"], "t-rows");
    let op: Value = sj::from_str(first[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        op,
        json!({"operation": "select", "table": "wall",
               "columns": ["seq", "session_id", "turn_id", "kind", "hash", "audience_set"],
               "where": {"session_id": "s-now", "kind": "user", "turn_id": {"neq": "t3"},
                         "audience_set": {"covers": ["member:e"]}},
               "order_by": [{"col": "seq", "dir": "desc"}], "limit": 8})
    );
    assert_eq!(
        plain.store[0].hop["cur_reason"],
        json!(r#"{"budget": 1500, "limit": 8, "n": 2}"#)
    );
    let a = the_answer(&plain, "view");
    let mut keys: Vec<&String> = a.body.keys().collect();
    keys.sort();
    assert_eq!(keys, ["messages", "truncated"], "no system part");
    assert_eq!(
        Value::Object(a.hop.clone()),
        json!({"route": "view"}),
        "the hop of a view"
    );
    assert_eq!(
        a.body["messages"],
        json!([{"origin": "user", "type": "text", "text": "first"},
               {"origin": "user", "type": "text", "text": "second"}]),
        "the person's words alone, as GH #1304 gave them"
    );
    // With a target both sides speak and the system tree stands beside them.
    let with = ask(json!({"turns": 2, "budget_tokens": 1500, "target": "decide"}));
    let b = the_answer(&with, "view");
    assert_eq!(
        said_of(&b),
        [
            s("user", "first"),
            s("assistant", "one"),
            s("user", "second"),
            s("assistant", "two")
        ]
    );
    assert_eq!(b.body["truncated"], a.body["truncated"]);
    assert_eq!(b.hop, a.hop);
    assert!(b.body["system"].is_object());
}

/// One curate of `turn` into the hive, `hints` the collector's contribution
/// when given (#1279: what the affinity brief brought back for the round's
/// person; D58 GH #1480: one entry of the curate's `parts`, source
/// `affinity`, for the target `decide`, about the round's person).
fn curate_with_hints(h: &mut Hive, turn: &str, iter: &str, hints: Option<Value>) {
    curate_with_hints_of(h, turn, iter, hints, "member:e");
}

/// [`curate_with_hints`] with the subject the brief was about.
fn curate_with_hints_of(h: &mut Hive, turn: &str, iter: &str, hints: Option<Value>, of: &str) {
    let hop = json!({"session_id": "s-now", "turn_id": turn, "iter": iter, "phase": ""});
    let ctx = json!({"session_id": "s-now", "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E});
    let mut body = json!({"messages": [user("the choir, search please")]});
    if let Some(v) = hints {
        body["parts"] = json!([{"target": "decide", "source": "affinity", "value": v,
                                "subject": of}]);
    }
    h.lane("in_curate", ctx, hop, body);
}

fn clarify_hints() -> Value {
    json!([{"question": "Its concerts or its website?", "answer": "its concerts",
            "class": "simple"}])
}

/// The person's kept hints row: the contribution of `affinity` to `decide`
/// about `member:e`, with the round it was brought in.
fn kept_hints(h: &Hive) -> Value {
    sj::from_str::<Value>(&h.state(r#"part:["decide","member:e","affinity"]"#))
        .unwrap_or(Value::Null)
}

/// The judge's view (target `decide`) of turn t3 in `round`.
fn judge_in(h: &Hive, round: &str) -> Msg {
    let run = drive(
        h,
        "in_view_ask",
        json!({"session_id": "s-now", "turn_id": "t3", "audience_set": round}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": 2,
                                            "budget_tokens": 1500}}),
    );
    the_answer(&run, "view")
}

/// D58 GH #1480 (was the judge package's `hints` and the rows
/// `judge:hints:<person>`): the person's how-questions ride on the curate as
/// the collector's contribution about the round's person; the curator keeps
/// them with the round, and the view of that person's next turn carries them
/// as the leaf `system.decide.affinity` -- the view of another person, or of
/// a round the hints were not brought in, does not.
#[test]
fn judge_context_carries_the_persons_hints() {
    let mut h = Hive::new();
    exchange(
        &mut h,
        1,
        "s-now",
        "t1",
        "the choir in Dresden",
        "Concerts or website?",
    );
    // The curate of turn t2 carries the hints the brief brought for member:e.
    curate_with_hints(&mut h, "t2", "0", Some(clarify_hints()));
    assert_eq!(
        kept_hints(&h),
        json!({"source": "affinity", "subject": "member:e", "round": ["member:e"],
               "value": clarify_hints()}),
        "the curator keeps them for the person, with their round: {:?}",
        h.stderr
    );
    // The next turn of the same person: the view carries them.
    let view = ask_judge(&h, "t3", 2);
    assert_eq!(
        leaf_json(&decide_leaf(&view, "affinity")),
        clarify_hints(),
        "{:?}",
        view.body
    );
    // Another person's turn: none of them.
    let other = judge_in(&h, "[\"member:f\"]");
    assert_eq!(
        decide_leaf(&other, "affinity"),
        Value::Null,
        "{:?}",
        other.body
    );
    // The same person in a wider round (another agent beside): the hints were
    // brought for a round this one is no subset of, so none of them (R-AF-3).
    let wider = judge_in(&h, "[\"agent:x\",\"member:e\"]");
    assert_eq!(
        decide_leaf(&wider, "affinity"),
        Value::Null,
        "{:?}",
        wider.body
    );
    // A later iteration of a round writes nothing again; a curate without the
    // contribution (the knob off) keeps what stands and writes nothing.
    curate_with_hints(&mut h, "t3", "1", Some(json!([])));
    curate_with_hints(&mut h, "t4", "0", None);
    assert_eq!(kept_hints(&h)["value"], clarify_hints());
    // A brief about somebody else than the round's person (a counterpart
    // outside the round) is kept under nobody.
    curate_with_hints_of(&mut h, "t4", "0", Some(json!([])), "peer:north");
    assert_eq!(kept_hints(&h)["value"], clarify_hints());
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'part:%peer:north%'")
            .is_empty(),
        "nothing kept about the counterpart"
    );
    // An empty list from the brief replaces them: no hint reaches the judge.
    curate_with_hints(&mut h, "t5", "0", Some(json!([])));
    let view = ask_judge(&h, "t6", 2);
    assert_eq!(leaf_json(&decide_leaf(&view, "affinity")), json!([]));
    assert!(
        !chain_decide_state(&view).contains("How this person"),
        "an empty list renders no hint"
    );
}

/// D58 GH #1480 (was: the judge package holds exactly its five fields of
/// before): a curate without the contribution keeps no row, and the view of
/// the next turn carries the tree of before -- the moment, no topic without
/// a topic mark, no family without a contribution -- so the route's decide
/// text has no hint block.
#[test]
fn judge_without_hints_reads_as_before() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "hello", "hi");
    curate_with_hints(&mut h, "t2", "0", None);
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'part:%'")
            .is_empty(),
        "no contribution, no row"
    );
    let view = ask_judge(&h, "t3", 2);
    let system = view.body["system"]
        .as_object()
        .unwrap_or_else(|| panic!("a view for a target carries a system tree: {:?}", view.body));
    let mut keys: Vec<&String> = system.keys().collect();
    keys.sort();
    assert_eq!(keys, ["frame"], "{:?}", view.body);
    assert!(
        !chain_decide_state(&view).contains("How this person"),
        "no hint block"
    );
}

// ═══════════════════════════════════ #1279: one hint, the whole way round

const CHAIN_ROUND: &str = "[\"agent:alpha\",\"member:e\"]";
const CHAIN_TURN: &str = "tg:1#0007";

fn chain_args(m: &Value) -> Value {
    sj::from_str(m["messages"][0]["text"].as_str().expect("call text")).expect("json")
}

/// The affinity store the brief reads: no entity row, a known trust, nothing
/// disclosed, and two accepted clarify proposals about `member:e` -- one made
/// in this round, one in front of another agent (no subset: never usable).
fn chain_store(op: &Value) -> Value {
    let proposal = |q: &str, a: &str, origin: &str, at: &str| {
        json!({"id": format!("clarify:{q}"), "entity_ref": "member:e",
               "field_path": "mx.hints.clarify", "status": "accepted",
               "origin_round": origin, "recorded_at": at,
               "value": json!({"question": q, "answer": a, "class": "simple",
                               "session_id": "s1", "asked_turn": "t1",
                               "answered_turn": "t2"}).to_string()})
    };
    match (
        op["operation"].as_str().unwrap_or(""),
        op["table"].as_str().unwrap_or(""),
    ) {
        ("select", "entities") | ("select", "disclosure") => json!([]),
        ("select", "trust") => json!([{"level": "known", "decided_at": "2026-10-01T00:00:00Z"}]),
        ("select", "proposals") => json!([
            proposal(
                "Its concerts or its website?",
                "its concerts",
                CHAIN_ROUND,
                "2026-10-29T10:00:00Z"
            ),
            proposal(
                "Which choir?",
                "the senior choir",
                "[\"agent:beta\",\"member:e\"]",
                "2026-10-28T10:00:00Z"
            ),
        ]),
        other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
    }
}

/// The `brief` the collector raised, answered by the shipped affinity brief
/// against `chain_store`, run to its answer.
fn chain_brief(raised: &Value) -> Value {
    let mut doc = json!({
        "header": {"hop": {"route": "in_brief"},
                   "context": {"asker": "agent:alpha", "channel_node": "telegram",
                               "audience_set": CHAIN_ROUND, "aff_phase": "",
                               "aff_carry": "", "aff_subscriber": ""}},
        "messages": raised["messages"].clone()
    });
    for _ in 0..12 {
        let (out, _) = assemble_cell::run_cell("templates/affinity/brief/config.json", &[], doc);
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return ans.clone();
        }
        let asked: Vec<&Value> = out
            .iter()
            .filter(|m| {
                m["header"]["route"] == "astore" && m["header"]["phase"].as_str() != Some("audit")
            })
            .collect();
        assert_eq!(asked.len(), 1, "one store read per phase: {out:?}");
        let (q, op) = (asked[0], chain_args(asked[0]));
        let h = &q["header"];
        doc = json!({
            "header": {"hop": {"operation": op["operation"]},
                       "context": {"affinity_origin": "brief", "aff_phase": h["phase"],
                                   "aff_subject": h["subject"], "aff_audience": h["audience"],
                                   "aff_channel": h["channel"], "aff_slots": h["slots"],
                                   "aff_carry": h["carry"], "aff_subscriber": h["subscriber"]}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "s1",
                          "text": chain_store(&op).to_string()}]
        });
    }
    panic!("the brief never answered");
}

/// The collector's whole turn with `brief_hints` "1" and the hints a
/// contribution to the judge's view (`part_target` `decide`, as talky sets
/// it; D58 GH #1480): the turn raises the brief, the brief's answer comes home
/// on `in_briefing`, and the fan-in opens the turn on `curate`.
fn chain_curate() -> Value {
    use assemble_cell::{assemble, bundle_reply, calls_of, in_phase, lane};
    let knob = [
        ("brief_hints", json!("1")),
        ("part_target", json!("decide")),
    ];
    let ctx = json!({"turn_id": CHAIN_TURN, "channel": "tg:1",
                     "audience_set": CHAIN_ROUND, "assistant": "alpha"});
    let said = json!([{"origin": "user", "type": "text", "text": "the choir, search please"}]);
    let out = assemble(
        &knob,
        lane("in_turn", json!({"turn_id": CHAIN_TURN}), ctx, said),
    );
    let raised = out
        .iter()
        .find(|m| m["header"]["route"] == "brief")
        .unwrap_or_else(|| panic!("the turn raised no brief: {out:?}"))
        .clone();
    let args = chain_args(&raised);
    let ans = chain_brief(&raised);
    let mut home = lane(
        "in_briefing",
        json!({"brief_outcome": "answer", "subject": args["subject"],
               "slots": args["slots"].to_string(), "subscriber": ""}),
        json!({"turn_id": CHAIN_TURN}),
        ans["messages"].clone(),
    );
    home["hints"] = ans["hints"].clone();
    let parked = assemble(&knob, home);
    let leg = calls_of(in_phase(&parked, "collect"))
        .into_iter()
        .map(|(_, a)| a["row"].clone())
        .find(|row| row["role"] == "leg-brief")
        .unwrap_or_else(|| panic!("in_briefing parked no leg-brief row: {parked:?}"));
    let window = json!({"turn_id": CHAIN_TURN, "iter": 0, "role": "leg-window", "fired": 0,
                        "turn": json!({"turns": [{"role": "user", "consult_id": "",
                                                  "text": "the choir, search please"}],
                                       "deferred": 0, "deferred_turns": []}).to_string()});
    let reply = bundle_reply(
        "collect",
        CHAIN_TURN,
        &[("c-collect-read", json!([window, leg]))],
    );
    assemble(&knob, reply)
        .into_iter()
        .find(|m| m["header"]["route"] == "curate")
        .expect("the turn opens on curate")
}

/// `./route`'s decide state for the view `./history` answered with (D58 GH
/// #1480: the view comes back on `in_view`, its `messages` as the lane's and
/// its `system` tree beside them, as the curator hive hands it on).
fn chain_decide_state(view: &Msg) -> String {
    let mut doc = assemble_cell::lane(
        "in_view",
        json!({"state": "the choir, search please",
               "asked_at": (assemble_cell::now_ms() + 3_600_000).to_string()}),
        json!({"turn_id": "t3", "channel": "tg:1", "audience_set": CHAIN_ROUND}),
        view.body.get("messages").cloned().unwrap_or(json!([])),
    );
    doc["system"] = view.body["system"].clone();
    let (out, err) = assemble_cell::run_cell(
        "templates/talky/route/config.json",
        &[("judge_context", json!("1"))],
        doc,
    );
    let d = out
        .iter()
        .find(|m| m["header"]["route"] == "decide")
        .unwrap_or_else(|| panic!("no decide: {out:?} {err}"));
    d["decide"]["state"].as_str().expect("state").to_string()
}

/// The whole way of one hint, each step the shipped script and each hop the
/// message the step before emitted: the collector raises a hints brief for the
/// round's one person, the affinity brief serves the accepted clarify pair of
/// this round (not the one made in front of another agent), the collector
/// carries it on `curate`, the intake keeps it for the person, the next judge
/// of that person carries it and the route renders it -- the judge of another
/// person does not. Each half is pinned on its own above and in the assemble
/// harness; this pins that the halves fit.
#[test]
fn one_hint_travels_from_the_brief_to_the_route() {
    let curate = chain_curate();
    let expected = json!([{"question": "Its concerts or its website?",
                           "answer": "its concerts", "class": "simple"}]);
    // D58 GH #1480: the hints ride on the curate as a contribution to the
    // judge's view, about the round's one person.
    assert_eq!(
        curate["parts"],
        json!([{"target": "decide", "source": "affinity", "value": expected,
                "subject": "member:e"}]),
        "{curate}"
    );

    let mut h = Hive::new();
    exchange(
        &mut h,
        1,
        "s-now",
        "t1",
        "the choir in Dresden",
        "Concerts or website?",
    );
    let mut body = curate.clone();
    body.as_object_mut()
        .expect("curate object")
        .remove("header");
    let ctx = json!({"session_id": "s-now", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": CHAIN_ROUND});
    let hop = json!({"session_id": "s-now", "turn_id": "t2", "iter": "0", "phase": ""});
    h.lane("in_curate", ctx, hop, body);

    let ask = |round: &str| {
        let run = drive(
            &h,
            "in_view_ask",
            json!({"session_id": "s-now", "turn_id": "t3", "audience_set": round}),
            json!({}),
            json!({"messages": [], "view_ask": {"target": "decide", "turns": 2,
                                                "budget_tokens": 1500}}),
        );
        the_answer(&run, "view")
    };
    let same = ask(CHAIN_ROUND);
    assert_eq!(
        same.body["system"]["decide"]["affinity"]["text"],
        json!(sj::to_string(&expected).expect("json")),
        "the view of the person's next turn carries the hints as its leaf: {:?} {:?}",
        same.body,
        h.stderr
    );
    let state = chain_decide_state(&same);
    assert!(
        state.contains(
            "- asked \"Its concerts or its website?\", the person answered \"its concerts\" (class: simple)"
        ),
        "{state}"
    );
    assert!(!state.contains("Which choir?"), "{state}");

    let other = ask("[\"agent:alpha\",\"member:f\"]");
    assert!(
        other.body["system"]["decide"].get("affinity").is_none(),
        "{:?}",
        other.body
    );
    assert!(!chain_decide_state(&other).contains("How this person"));
}

/// A `session` note of the standard round straight into the ledger, the way
/// `./intake` keeps one (`in_note`), at minute `minute` with `priority`.
fn note(h: &Hive, id: &str, text: &str, priority: i64, minute: u32) {
    let stamp = at(minute).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    h.db.execute(
        "INSERT INTO notes (source, note_id, text, scope, until, priority, audience_set, \
         session_id, at, used_turn) VALUES ('probe', ?1, ?2, 'session', '', ?3, ?4, '', ?5, '')",
        rusqlite::params![id, text, priority, ROUND_E, stamp],
    )
    .unwrap();
}

/// The `Notes:` line of a seed, "" when it carries none.
fn notes_line(seed: &Msg) -> String {
    seed.messages()
        .iter()
        .filter_map(|m| m["text"].as_str())
        .find(|t| t.starts_with("Notes: "))
        .unwrap_or("")
        .to_string()
}

/// `seed_entries_max` bounds the notes a seed carries to the
/// best ranked -- `priority` first (0 first), then the youngest -- and they
/// stand in the order they always stood (oldest first). Unset, every note
/// stands, byte for byte as before.
#[test]
fn seed_entries_max_keeps_the_best_ranked_notes() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");
    // (id, priority, minute): n6 and n7 are the youngest of priority 5.
    for (id, prio, minute) in [
        ("n1", 5, 10),
        ("n2", 1, 11),
        ("n3", 5, 12),
        ("n4", 9, 13),
        ("n5", 0, 14),
        ("n6", 5, 15),
        ("n7", 5, 16),
    ] {
        note(&h, id, &format!("note {id}"), prio, minute);
    }
    let whole = seed_ask(&h, "c-1", 0, json!(ROUND_E));
    assert_eq!(
        notes_line(&whole),
        "Notes: note n1; note n2; note n3; note n4; note n5; note n6; note n7",
        "without the param every note stands"
    );
    let zero = seed_ask_with(
        &h,
        &[("seed_entries_max", json!(0))],
        "c-1",
        0,
        json!(ROUND_E),
    );
    assert_eq!(
        zero.body, whole.body,
        "0 is no bound: the seed is byte-identical"
    );

    let run = drive_with(
        &h,
        &[("seed_entries_max", json!(5))],
        "in_seed_ask",
        json!({"live": "c-1", "audience_set": ROUND_E}),
        json!({"live": "c-1", "session_id": "c-1", "reason": "start", "renewal_n": 0}),
        json!({"messages": []}),
    );
    // The cut is said, not silent (R-IG-1): how many notes stood out.
    assert!(
        run.stderr
            .contains("seed of c-1 carries 5 of 7 notes (seed_entries_max 5), the best ranked"),
        "{}",
        run.stderr
    );
    let five = the_answer(&run, "seed");
    assert_eq!(
        notes_line(&five),
        "Notes: note n2; note n3; note n5; note n6; note n7",
        "priority first, then the youngest; shown oldest first"
    );
    // The fact line and the turns are no notes: they stay.
    assert_eq!(five.messages()[0], whole.messages()[0]);
    let msgs = five.messages();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["developer", "developer", "user", "assistant"]);
}

/// `seed_anchor` is the persona's anchor -- unset nothing, set
/// the last developer item of the seed, after the turns and a held hint and
/// right before the end of the call (`wind_down`), never in the head.
#[test]
fn seed_anchor_closes_the_seed_before_the_call_ends() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");
    let plain = seed_ask(&h, "c-1", 0, json!(ROUND_E));
    let empty = seed_ask_with(&h, &[("seed_anchor", json!(""))], "c-1", 0, json!(ROUND_E));
    assert_eq!(
        empty.body, plain.body,
        "an empty anchor leaves the seed byte-identical"
    );

    const ANCHOR: &str = "You are the companion: warm, brief, curious.";
    let anchored = seed_ask_with(
        &h,
        &[("seed_anchor", json!(ANCHOR))],
        "c-1",
        1,
        json!(ROUND_E),
    );
    let msgs = anchored.messages();
    assert_eq!(
        msgs.last().unwrap(),
        &json!({"role": "developer", "text": ANCHOR}),
        "the anchor last"
    );
    assert_eq!(
        msgs[..msgs.len() - 1],
        plain.messages()[..],
        "nothing else moved"
    );
    let est: usize = msgs
        .iter()
        .map(|m| m["text"].as_str().unwrap().len().div_ceil(3))
        .sum();
    assert_eq!(
        anchored.body["est_tokens"],
        json!(est),
        "the anchor is counted"
    );

    seed_line(&h, "c-1", "wind_down", "The call ends in two minutes.");
    seed_line(&h, "c-1", "hint", "Mention the roses.");
    let ending = seed_ask_with(
        &h,
        &[("seed_anchor", json!(ANCHOR))],
        "c-1",
        2,
        json!(ROUND_E),
    );
    let texts: Vec<(String, String)> = ending
        .messages()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap().to_string(),
                m["text"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let tail: Vec<(&str, &str)> = texts[texts.len() - 3..]
        .iter()
        .map(|(r, t)| (r.as_str(), t.as_str()))
        .collect();
    assert_eq!(
        tail,
        [
            ("developer", "Mention the roses."),
            ("developer", ANCHOR),
            ("developer", "The call ends in two minutes."),
        ],
        "the anchor right before the end of the call"
    );
    assert_ne!(texts[0].1, ANCHOR, "never in the head");
}

/// The edge out of `./history` on `seed` (templates/curator/config.json), applied
/// to the answer as the colony applies it.
fn through_the_seed_exit(a: &Msg) -> Headers {
    let hive = read_json(&repo("templates/curator/config.json"));
    let exit = hive["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["from"] == "./history"
                && e["to"] == "."
                && e["condition"].as_str().is_some_and(|c| {
                    matches!(
                        evaluate_condition(&parse_condition(c).unwrap(), &a.context, &a.hop),
                        Ok(true)
                    )
                })
        })
        .cloned()
        .expect("an exit for `seed`");
    let spec: ModifierSpec = sj::from_value(exit["modifier"].clone()).unwrap();
    apply_modifier(
        &parse_modifier(&spec).unwrap(),
        &Headers::from_parts(a.context.clone(), a.hop.clone()),
    )
    .expect("the modifier applies")
}

/// live-2 #1370 (K6): the `seed` seam end to end. The voice cell asks at the
/// ring, `./history` answers that ask (its hop as the voice wrote it), the
/// answer leaves the curator through its rim edge and reaches the voice
/// unchanged on `in_seed` -- and the parked session takes exactly the lines
/// history wrote. Before, each half was pinned on its own
/// (`gpt_live_seed_lanes.rs` built the seed by hand), so a key one side
/// renamed would have passed both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_from_history_reaches_the_voice_session_unchanged() {
    use meclaw_cells::voice::contract::{DuplexControl, SeedMessage};
    use meclaw_core::{Body, MessageBuilder, Path};

    let mut h = Hive::new();
    exchange(&mut h, 3, "s-now", "t1", "the garden", "the roses bloom");
    h.mark(at(4), "s-now", "t1", "topic", &topic("garden", "start"));

    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 10_000, 300, 60_000, 400).await;
    let mut ring = Map::new();
    ring.insert("route".into(), json!("call_ringing"));
    ring.insert("call_uuid".into(), json!("ring-1"));
    live.send(
        MessageBuilder::new(Path::new("/voice"))
            .hop(ring)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;
    let ask = live.emission("seed_ask").await;
    let mut ask_hop = ask["header"].as_object().cloned().expect("a header");
    ask_hop.remove("route");
    let call = ask_hop["call_id"].clone();
    // D58 live (GH #1482): the composition translates the voice's call into
    // the handle `live` on the way in (plan `u-live.md` contract point 2) --
    // the curator reads no `call_id`.
    ask_hop.insert("live".into(), call.clone());

    // The set lifts the handle and the round into the context (its edge #11).
    let a = tokio::task::block_in_place(|| {
        let run = drive(
            &h,
            "in_seed_ask",
            json!({"live": call, "audience_set": ROUND_E}),
            Value::Object(ask_hop.clone()),
            json!({"messages": []}),
        );
        the_answer(&run, "seed")
    });
    assert!(!a.hop.contains_key("degraded"), "{:?}", a.hop);
    let out = through_the_seed_exit(&a);
    let mut hop = out.hop.clone();
    hop.insert("route".into(), json!("in_seed"));
    // ... and back on the way out: the voice cell keeps `call_id` (contract
    // point 2), the composition sets it from the handle the seed carries.
    assert_eq!(hop.get("live"), Some(&call), "the seed carries the handle");
    hop.insert("call_id".into(), call.clone());
    let mut back = out.context.clone();
    back.insert("call_id".into(), call.clone());
    live.send(
        MessageBuilder::new(Path::new("/voice"))
            .hop(hop)
            .context(back)
            .body(Body::Inline(Value::Object(a.body.clone())))
            .build(),
    )
    .await;

    let wrote: Vec<SeedMessage> = a
        .messages()
        .iter()
        .map(|m| SeedMessage {
            role: m["role"].as_str().unwrap().to_string(),
            text: m["text"].as_str().unwrap().to_string(),
        })
        .collect();
    assert!(!wrote.is_empty(), "history wrote a seed: {:?}", a.body);
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: no seed reached a session")
                .expect("the fake is running");
        if let DuplexControl::Seed { messages } = control {
            assert_eq!(at, 1, "the parked session of the ring");
            assert_eq!(messages, wrote, "the lines history wrote, unchanged");
            break;
        }
    }
}

/// live-2 #1370 (K6): the `live_feed` seam. The collector's `curate` head carries
/// neither `engine`, `call_id` nor `live`; the composition's edges set the
/// handle `live` into the context (D58 live: from `engine` "duplex" and the
/// call), and the policy reads it from there. The collector's emission goes
/// unchanged onto `in_curate`; after the closed turn of the running session
/// ONE `live_feed` leaves for the handle.
#[test]
fn live_feed_from_a_collector_curate_with_the_members_context() {
    use assemble_cell::{assemble, bundle_reply};
    const TURN: &str = "feed-channel#0001";
    let window = json!({"turn_id": TURN, "iter": 0, "role": "leg-window", "fired": 0,
                        "recorded_at": "2999-01-01T00:00:00.000000Z",
                        "turn": json!({"turns": [{"role": "user", "consult_id": "",
                                                  "text": "and then?"}],
                                       "deferred": 0, "deferred_turns": []}).to_string()});
    let mut reply = bundle_reply("collect", TURN, &[("c-collect-read", json!([window]))]);
    reply["header"]["context"]["live"] = json!("call-9");
    let ctx_in = reply["header"]["context"].clone();
    let curate = assemble(&[], reply)
        .into_iter()
        .find(|m| m["header"]["route"] == "curate")
        .expect("the turn opens on curate");
    let mut hop = curate["header"].as_object().cloned().expect("a header");
    let carried = hop.remove("context");
    for k in ["engine", "call_id", "live"] {
        assert!(
            !hop.contains_key(k),
            "the curate head carries no {k}: {hop:?}"
        );
    }
    // The context travels with the message (the substrate keeps it); the
    // cell's own context keys, if it wrote any, lie over it.
    let mut ctx = obj(ctx_in);
    if let Some(Value::Object(c)) = carried {
        ctx.extend(c);
    }
    ctx.insert("channel".into(), json!("test"));
    ctx.insert("audience_set".into(), json!(ROUND_E));
    let session = hop["session_id"].as_str().unwrap_or_default().to_string();
    let mut body = curate.clone();
    body.as_object_mut().unwrap().remove("header");

    let mut h = Hive::with(&[("policy", "live_feed", json!("1"))]).reporting_composite("talky");
    clear_of_midnight();
    h.mark(
        backdated(50),
        &session,
        "t0",
        "topic",
        &topic("choir", "start"),
    );
    h.lane("in_curate", Value::Object(ctx), Value::Object(hop), body);
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(feeds[0].hop["live"], json!("call-9"));
    assert_eq!(feeds[0].hop["section"], json!("renew"));
}

// ═══════════════════ live-2 fx-call (#1369): what a call carried, at its end

/// The end of a call (`in_live_end` under the handle `live`, D58 GH #1482;
/// the final `usage` of the voice cell the set draws here) run to its one
/// answer.
fn call_end(h: &Hive, call: &str, round: Value) -> Msg {
    let run = drive(
        h,
        "in_live_end",
        json!({"live": call, "audience_set": round}),
        json!({"live": call, "session_id": call, "final": true, "seconds": 312}),
        json!({"messages": []}),
    );
    the_answer(&run, "live_facts")
}

/// A seed went out for `c-1` with a note and a topic, the voice delegated
/// once in that call and once in another: the end of `c-1` says the seed was
/// used, the facts it gave (the note `{source, id, text}`, the topic) and only
/// its own delegation. Read only -- the facts row of the call stays.
#[test]
fn call_facts_tell_the_seed_and_the_delegations_of_the_call() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");
    h.mark(at(2), "s-now", "t1", "topic", &topic("garden", "start"));
    note(&h, "n1", "the person waters at seven", 5, 3);
    let seed = seed_ask(&h, "c-1", 0, json!(ROUND_E));
    assert!(!seed.hop.contains_key("refused"), "{:?}", seed.hop);
    let held = h.state("seed_facts:c-1");
    assert!(!held.is_empty(), "the seed wrote the facts of the call");
    h.row(
        at(5),
        "c-1",
        "c-1#2",
        "delegation",
        &user("[delegation d-1] book the table"),
        1,
    );
    h.row(
        at(6),
        "c-2",
        "c-2#1",
        "delegation",
        &user("[delegation d-9] another call"),
        1,
    );

    let a = call_end(&h, "c-1", json!(ROUND_E));
    assert_eq!(a.hop["live"], "c-1");
    assert_eq!(a.hop["session_id"], "c-1");
    assert_eq!(a.body["live"], "c-1");
    assert_eq!(a.body["session_id"], "c-1");
    assert_eq!(a.body["seed_used"], true);
    assert_eq!(
        a.body["facts"],
        json!([{"source": "probe", "id": "n1", "text": "the person waters at seven"},
               {"source": "topic", "id": "", "text": "garden"}]),
        "{:?}",
        a.body
    );
    assert_eq!(
        a.body["delegations"],
        json!([{"turn_id": "c-1#2", "text": "[delegation d-1] book the table"}]),
        "only the delegations of this call"
    );
    assert_eq!(a.messages(), Vec::<Value>::new());
    assert_eq!(
        h.state("seed_facts:c-1"),
        held,
        "the end of a call only reads"
    );
}

/// A call that got no seed and delegated nothing: `seed_used` false, both
/// lists empty -- one answer all the same, never silence.
#[test]
fn call_facts_without_a_seed_are_empty() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");
    let a = call_end(&h, "c-1", json!(ROUND_E));
    assert_eq!(a.body["seed_used"], false);
    assert_eq!(a.body["facts"], json!([]));
    assert_eq!(a.body["delegations"], json!([]));
    assert!(!a.hop.contains_key("refused") && !a.hop.contains_key("degraded"));

    // Without a round nothing of the wall is read: the answer says so.
    let refused = call_end(&h, "c-1", json!(null));
    assert_eq!(refused.hop["refused"], "no_round");
    assert_eq!(refused.body["seed_used"], false);

    // Without the handle `live` it is no running session (D58 GH #1482,
    // OR-d58-2.live.1): the voice's `call_id` alone counts for nothing, the
    // session is no fallback -- refused, nothing read, one answer all the same.
    let run = drive(
        &h,
        "in_live_end",
        json!({"call_id": "c-1", "audience_set": ROUND_E}),
        json!({"call_id": "c-1", "session_id": "c-1", "final": true}),
        json!({"messages": []}),
    );
    let a = the_answer(&run, "live_facts");
    assert!(
        run.store.is_empty(),
        "not one ledger read without the handle"
    );
    assert_eq!(a.hop["refused"], "no_live");
    assert_eq!(a.body["seed_used"], false);
}

/// The end of ANOTHER call reads nothing of `c-1`: neither its seed nor its
/// delegation.
#[test]
fn call_facts_of_a_foreign_call_carry_nothing_of_this_one() {
    let mut h = Hive::new();
    exchange(&mut h, 1, "s-now", "t1", "the garden", "the roses bloom");
    note(&h, "n1", "a note", 5, 3);
    let _ = seed_ask(&h, "c-1", 0, json!(ROUND_E));
    h.row(
        at(5),
        "c-1",
        "c-1#2",
        "delegation",
        &user("[delegation d-1] book the table"),
        1,
    );
    let a = call_end(&h, "c-9", json!(ROUND_E));
    assert_eq!(a.body["live"], "c-9");
    assert_eq!(a.body["seed_used"], false);
    assert_eq!(a.body["facts"], json!([]));
    assert_eq!(a.body["delegations"], json!([]));
}

/// live-2 fx-call (K3): a DELTA of a voice turn (`context.delta` "1") is
/// stamped under the call's id as `context.session_id`, while the turns of the
/// conversation stand under the generation's (`<channel>-<now>`). Its judge
/// reads the round's newest rows whatever their session, the way the seed does;
/// without the delta the session filter stands as before.
#[test]
fn a_delta_reads_the_round_not_the_calls_session() {
    let mut h = Hive::new();
    exchange(
        &mut h,
        1,
        "phone-1761800000",
        "t1",
        "how is the weather",
        "sunny",
    );
    h.mark(
        at(2),
        "phone-1761800000",
        "t1",
        "topic",
        &topic("weather", "start"),
    );
    let ask = |delta: bool| {
        let mut ctx = json!({"session_id": "call-1", "turn_id": "call-1#3"});
        if delta {
            ctx["delta"] = json!("1");
        }
        let run = drive(
            &h,
            "in_view_ask",
            ctx,
            json!({}),
            json!({"messages": [], "view_ask": {"target": "decide", "turns": 4, "budget_tokens": 1500}}),
        );
        the_answer(&run, "view")
    };
    let d = ask(true);
    assert_eq!(
        said_of(&d),
        [s("user", "how is the weather"), s("assistant", "sunny")],
        "{:?}",
        d.body
    );
    assert_eq!(leaf_text(&d, "topic"), "weather");
    let plain = ask(false);
    assert_eq!(
        plain.body["messages"],
        json!([]),
        "no delta: the session filter stands"
    );
    assert_eq!(leaf_text(&plain, "topic"), "");
}
