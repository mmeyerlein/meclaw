//! #1402 -- a renewed seed waits, and a seed knows the name it speaks under.
//!
//! WHY this file exists: after a `renew` or `recover` the voice cell opens the
//! next model session without a greeting (`greet=false`) on the seed of
//! `./history`, and that seed ended on the persona anchor -- the reason of the
//! ask was never read, so the model spoke first after two of three renewals
//! (plan d53 T2 § V5, reader B § 3). `seed_renew_line` closes a renewed seed
//! on the set's own wait line; `seed_anchor_first` moves the anchor in front
//! of the turns. A seed knew no runtime name either: `in_seed_line` of kind
//! `identity` holds one line in the head right after the facts, until a newer
//! one replaces it -- per member, not per call: a name belongs to the member,
//! and the set's character op sends it on a rename without any call (S5
//! review I-1). The texts are the set's; the defaults
//! leave the seed byte-identical (`default_seed_bytes_do_not_move`).
//!
//! The history cell runs as in `fu2_h_the_judge_context_comes_from_the_wall`:
//! the shipped script, its store messages answered by the store cell's own
//! dispatcher on the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
use curator_hive::*;

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// The fixed instant of every seed here (`now_utc`), a Friday.
const NOW: &str = "2026-10-30T01:00:00Z";
const ANCHOR: &str = "You are the companion: warm, brief, curious.";
const WAIT: &str = "The call goes on; wait for the person to speak.";

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
        let outcome = dispatch(db, &op(c)).expect("a ledger op");
        build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
    } else {
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let outcome = dispatch(db, &op(c)).expect("a ledger op");
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

/// One message on `route` into `./history` with `over` set over the shipped
/// params, run to rest against the hive's ledger; what it answered.
fn drive(
    h: &Hive,
    over: &[(&str, Value)],
    route: &str,
    context: Value,
    hop: Value,
    body: Value,
) -> Vec<Msg> {
    let (script, mut params) = params();
    for (k, v) in over {
        params.insert((*k).to_string(), v.clone());
    }
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
    let mut out = Vec::new();
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 20, "the message does not come to rest");
        for m in step(&script, &params, &msg) {
            if m.route() != "lstore" {
                out.push(m);
                continue;
            }
            let mut req = m.clone();
            for (key, from) in [
                ("cur_origin", "reply_cell"),
                ("cur_phase", "phase"),
                ("cur_call", "cur_call"),
                ("cur_reason", "cur_reason"),
            ] {
                req.context.insert(
                    key.to_string(),
                    m.hop.get(from).cloned().expect("a store hop"),
                );
            }
            queue.push_back(ledger_answer(&h.db, &req));
        }
    }
    out
}

/// The one `seed` of an `in_seed_ask` of `call` for `reason`.
fn seed(h: &Hive, over: &[(&str, Value)], call: &str, reason: &str, renewal_n: u64) -> Msg {
    let out = drive(
        h,
        over,
        "in_seed_ask",
        json!({"live": call}),
        json!({"live": call, "session_id": call, "reason": reason, "renewal_n": renewal_n}),
        json!({"messages": []}),
    );
    assert_eq!(out.len(), 1, "exactly one answer: {out:?}");
    assert_eq!(out[0].route(), "seed", "{:?}", out[0].hop);
    out[0].clone()
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
    let out = drive(h, &[], "in_seed_line", json!({}), json!({}), body);
    assert!(out.is_empty(), "nothing answers a seed line");
}

fn at(minute: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("2026-10-24T09:{minute:02}:00Z"))
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

/// A wall of one exchange, both roles.
fn garden() -> Hive {
    let mut h = Hive::new();
    h.row(at(1), "s-now", "t1", "user", &user("the garden"), 1);
    h.row(
        at(2),
        "s-now",
        "t1",
        "assistant",
        &said("the roses bloom"),
        1,
    );
    h
}

/// `(role, text)` of every seed message.
fn lines(seed: &Msg) -> Vec<(String, String)> {
    seed.messages()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap().to_string(),
                m["text"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn dev(text: &str) -> (String, String) {
    ("developer".to_string(), text.to_string())
}

fn est(seed: &Msg) -> Value {
    json!(
        seed.messages()
            .iter()
            .map(|m| m["text"].as_str().unwrap().len().div_ceil(3))
            .sum::<usize>()
    )
}

/// A `renew` or `recover` seed ends on the set's wait line -- after the
/// anchor and after a held `wind_down`, the last developer line.
#[test]
fn a_renewed_seed_ends_with_the_wait_line_when_set() {
    let over = [
        ("seed_anchor", json!(ANCHOR)),
        ("seed_renew_line", json!(WAIT)),
    ];
    for reason in ["renew", "recover"] {
        let h = garden();
        let renewed = seed(&h, &over, "c-1", reason, 1);
        let got = lines(&renewed);
        assert_eq!(
            got[got.len() - 2..],
            [dev(ANCHOR), dev(WAIT)],
            "{reason}: the wait line after the anchor"
        );
        assert_eq!(
            renewed.body["est_tokens"],
            est(&renewed),
            "{reason}: counted"
        );

        seed_line(&h, "c-1", "wind_down", "The call ends in two minutes.");
        let ending = seed(&h, &over, "c-1", reason, 2);
        let got = lines(&ending);
        assert_eq!(
            got[got.len() - 3..],
            [dev(ANCHOR), dev("The call ends in two minutes."), dev(WAIT)],
            "{reason}: the wait line after the end of the call"
        );
    }
}

/// A `start` seed never carries the wait line, set or not.
#[test]
fn a_fresh_seed_never_carries_the_wait_line() {
    let h = garden();
    let plain = seed(&h, &[("seed_anchor", json!(ANCHOR))], "c-1", "start", 0);
    let set = seed(
        &h,
        &[
            ("seed_anchor", json!(ANCHOR)),
            ("seed_renew_line", json!(WAIT)),
        ],
        "c-1",
        "start",
        0,
    );
    assert_eq!(
        set.body, plain.body,
        "a start seed is the seed without the param"
    );
    assert!(lines(&set).iter().all(|(_, t)| t != WAIT));
}

/// An `identity` line stands in the head right after the facts line, before
/// the notes and the turns -- and stays for every next seed, of any call.
#[test]
fn an_identity_line_leads_the_seed_after_the_facts() {
    let h = garden();
    let plain = seed(&h, &[], "c-1", "start", 0);
    seed_line(&h, "c-1", "identity", "You are Lotte.");
    let named = seed(&h, &[], "c-1", "start", 0);
    let mut want = lines(&plain);
    want.insert(1, dev("You are Lotte."));
    assert_eq!(lines(&named), want, "one line after the facts");
    assert!(lines(&plain)[0].1.starts_with("Facts: "));
    assert_eq!(named.body["est_tokens"], est(&named), "counted");
    let again = seed(&h, &[], "c-1", "renew", 1);
    assert_eq!(lines(&again)[1], dev("You are Lotte."), "it holds");
    let other = seed(&h, &[], "c-2", "start", 0);
    assert_eq!(
        lines(&other)[1],
        dev("You are Lotte."),
        "the member's, not the call's"
    );
}

/// The set's character op sends the identity line on a rename WITHOUT a call;
/// it reaches the next seed of every call (S5 review I-1: keyed per call, it
/// was dropped and never reached a seed).
#[test]
fn an_identity_line_without_a_call_reaches_the_next_seed() {
    let h = garden();
    let out = drive(&h, &[], "in_seed_line", json!({}), json!({}), {
        let mut body = place("identity");
        body["text"] = json!("You are Lotte.");
        body
    });
    assert!(out.is_empty(), "nothing answers a seed line");
    let named = seed(&h, &[], "c-7", "start", 0);
    assert_eq!(lines(&named)[1], dev("You are Lotte."), "after the facts");
    assert_eq!(named.body["est_tokens"], est(&named), "counted");
    // A hint still needs its call: without one it reaches no seed.
    drive(&h, &[], "in_seed_line", json!({}), json!({}), {
        let mut body = place("hint");
        body["text"] = json!("Ask about the roses.");
        body
    });
    let again = seed(&h, &[], "c-7", "renew", 1);
    assert!(
        lines(&again)
            .iter()
            .all(|(_, t)| t != "Ask about the roses.")
    );
    assert_eq!(lines(&again)[1], dev("You are Lotte."), "it holds");
}

/// A newer identity line replaces the older, whichever call sent it; `seed_anchor_first`
/// puts the anchor at the end of the head, before the turns.
#[test]
fn a_newer_identity_line_replaces_the_older() {
    let h = garden();
    seed_line(&h, "c-1", "identity", "You are Lotte.");
    seed_line(&h, "c-9", "identity", "You are Greta.");
    let named = seed(&h, &[], "c-1", "renew", 1);
    let got = lines(&named);
    assert_eq!(got[1], dev("You are Greta."));
    assert!(
        got.iter().all(|(_, t)| t != "You are Lotte."),
        "the older one is gone"
    );

    let first = seed(
        &h,
        &[
            ("seed_anchor", json!(ANCHOR)),
            ("seed_anchor_first", json!("1")),
        ],
        "c-1",
        "start",
        0,
    );
    let got = lines(&first);
    assert_eq!(
        got[..3],
        [got[0].clone(), dev("You are Greta."), dev(ANCHOR)]
    );
    assert_eq!(
        got[3],
        ("user".to_string(), "the garden".to_string()),
        "before the turns"
    );
    assert_eq!(got.iter().filter(|(_, t)| t == ANCHOR).count(), 1, "once");
}

/// The defaults move no byte of the seed, whatever the reason: the golden of
/// FU3 (facts, turns, held hint, anchor, held wind_down).
#[test]
fn default_seed_bytes_do_not_move() {
    let golden = |s: &Msg| {
        assert_eq!(
            s.body["messages"],
            json!([
                {"role": "developer",
                 "text": "Facts: now 2026-10-30T02:00:00+01:00 (Friday, Europe/Berlin); place: Berlin."},
                {"role": "user", "text": "the garden"},
                {"role": "assistant", "text": "the roses bloom"},
                {"role": "developer", "text": "Mention the roses."},
                {"role": "developer", "text": ANCHOR},
                {"role": "developer", "text": "The call ends in two minutes."},
            ]),
            "the seed of FU3, byte for byte"
        );
        assert_eq!(s.body["est_tokens"], est(s));
    };
    for reason in ["start", "renew", "recover"] {
        let h = garden();
        seed_line(&h, "c-1", "hint", "Mention the roses.");
        seed_line(&h, "c-1", "wind_down", "The call ends in two minutes.");
        let s = seed(&h, &[("seed_anchor", json!(ANCHOR))], "c-1", reason, 1);
        let defaults = [
            ("seed_anchor", json!(ANCHOR)),
            ("seed_renew_line", json!("")),
            ("seed_anchor_first", json!("0")),
        ];
        golden(&s);
        let h2 = garden();
        seed_line(&h2, "c-1", "hint", "Mention the roses.");
        seed_line(&h2, "c-1", "wind_down", "The call ends in two minutes.");
        assert_eq!(
            seed(&h2, &defaults, "c-1", reason, 1).body,
            s.body,
            "{reason}"
        );
    }
}
