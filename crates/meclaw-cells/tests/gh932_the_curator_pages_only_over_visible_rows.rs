//! GH #932 -- a curator pages, counts and windows only over the rows the
//! round may see.
//!
//! Since GH #925 every row of the curator's ledger carries the audience of the
//! round that caused it, and every reader drops a row its round may not see
//! before it uses it. That gate ran AFTER the store had answered: a reader
//! asked for "the newest N rows", got rows of other rounds among them and threw
//! those away. The words stayed hidden, but the arithmetic did not -- where a
//! page of a history search ended, how many rows a renewal or a push window
//! held, whether a read hit its row bound and answered in the cut form, and
//! the turn index the writer stamped (`<session>#<n>` counted every round of a
//! session, so its gaps told a round how many turns of others it had not
//! seen). Each of these is a side channel: a round could count what it was
//! not present for. Since GH #932 the store filters first (the `covers`
//! operator in every round's read), so a hidden row never enters a read at
//! all, and the turn index counts per round under a tag of the round
//! (`<session>#<tag>-<n>`, `tag` = the first 8 hex digits of the sha256 of the
//! canonical round).
//!
//! The rule every test here holds: what a round is answered is a function of
//! the rows it may see and of nothing else. Two tracks of one session and one
//! channel are driven through the curator's own lanes -- track A with the
//! turns of one round only, track B with the SAME turns and turns of another
//! round between them, plus a block of that other round's rows after them
//! that is larger than any bound a reader puts on one read (the renewal's 600
//! rows are the largest). Then the same question is put to both, in the first
//! round, and the answers must be equal field by field:
//!
//! 1. the pages of `history_search` and `history_outline` (hits, `scanned`,
//!    `total_hits`, where `limit` or `scan_budget` stopped, the cut form),
//! 2. the push window (the question that leaves on `recall`),
//! 3. the renewal block of a renewed live session (`in_renewed`),
//! 4. the window that goes to the model (`brain`) -- measured without a
//!    rebuild, which is honest about what it measures: the window as the
//!    turns build it,
//! 5. the window after a rebuild in the round (review I-1, ruling (b)): the
//!    shape of a rebuild -- cover, kept rows, sizes, releases, shrunk results,
//!    stubs -- follows from the rows and marks of the round the rebuild is
//!    made in alone; the marks of another round stay ahead of that round's
//!    cursor, never lost, and never shape this round's window. One plan per
//!    hive stays: where a rebuild that ANOTHER round caused cuts the window
//!    is a residual channel (a plan per round is a later item),
//! 6. the turn index (`turn_write`: `hop.turn_id`, `hop.turn_index`), which
//!    also has the per-round form and runs 0, 1, 2 in both tracks.
//!
//! And PP-BD-12: a row written without a round carries `[]`, not NULL, so a
//! call without a round finds the rows of its own session that declare none
//! by value in the store -- and still never a row from before the audience
//! rule (NULL), which reaches no round at all.
//!
//! What is normalised is only what differs between two runs whatever the
//! ledger holds: the wall's `seq` (microseconds of the run's clock), its `at`
//! stamps and the episode's `happened_at` (wall-clock time), and the random
//! id of a model call. Nothing a round could count is dropped.
//!
//! The hive runs in one process (`support/curator_hive.rs`): the shipped
//! `script_inline` programs under python3, the shipped edges under the
//! colony's own CEL, and an in-memory ledger whose every operation runs
//! through the store cell's own dispatcher -- so the store's filter is the
//! real one. The history cell is not part of that harness's hive; its calls
//! run the shipped script against the same ledger through the same
//! dispatcher, with the two edges between it and the ledger applied here as
//! the hive's `params.graph` declares them. Guarded like every
//! template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

// ═══════════════════════════════════════════════════════════════ the rounds

/// The session and channel both tracks run in.
const SESSION: &str = "s-932";

/// The round every question is asked in, as a channel stamps it (unsorted,
/// with spaces), and as the ledger stores it (canonical).
const ROUND_EA: &str = r#"["member:e", "member:a"]"#;
const CANON_EA: &str = r#"["member:a","member:e"]"#;
/// The other round of the same session.
const ROUND_EB: &str = r#"["member:e", "member:b"]"#;
const CANON_EB: &str = r#"["member:b","member:e"]"#;

/// Every sentence carries the token `cw932`, so a search for it would show a
/// row of the other round the moment one crossed the gate; the visible turns
/// name people and places a push turns into entities, the hidden ones name
/// others.
const EA_TURNS: [(&str, &str, &str); 3] = [
    (
        "t-ea-1",
        "cw932 ea one: the parcel for Bruno left Lisbon on time.",
        "cw932 ea one reply: noted, the parcel reaches Bruno in Porto.",
    ),
    (
        "t-ea-2",
        "cw932 ea two: the lease is signed by Clara and Marta.",
        "cw932 ea two reply: noted, Marta keeps the signed lease.",
    ),
    (
        "t-ea-3",
        "cw932 ea three: the market opens early at Rossio.",
        "cw932 ea three reply: noted, the market at Rossio opens early.",
    ),
];
const EB_TURNS: [(&str, &str, &str); 3] = [
    (
        "t-eb-1",
        "cw932 eb one: the spare key lies under the stone by Zora.",
        "cw932 eb one reply: noted, Zora knows where the key lies.",
    ),
    (
        "t-eb-2",
        "cw932 eb two: the boat waits at Kestrel until Yarrow comes.",
        "cw932 eb two reply: noted, Yarrow takes the boat at Kestrel.",
    ),
    (
        "t-eb-3",
        "cw932 eb three: the code of the shed is the year of Quill.",
        "cw932 eb three reply: noted, the shed opens with the year of Quill.",
    ),
];
/// What only the other round said: no answer to the first round may carry it.
const EB_MARK: &str = "cw932 eb";

/// The block of the other round's rows written after the turns: more than the
/// largest bound a reader puts on one read of the wall (a renewal reads the
/// newest 600 rows, `curator/handover` RENEW_WALL_ROWS; a push 48; a history
/// read at the knobs below 8), so before GH #932 every bounded read of track B
/// met only hidden rows where track A met the visible ones.
const FLOOD: usize = 640;

/// The history knobs of the history tests: a budget smaller than the visible
/// wall, so `scanned`, `cut_by` and the page where `limit` stops all say
/// something.
const HISTORY_KNOBS: [(&str, i64); 2] = [("scan_budget", 4), ("page_rows", 2)];

const PROBE_TURN: &str = "t-ea-probe";
const PROBE_SAYS: &str = "cw932 probe: what is still open for the two of us?";
const HISTORY_CALL: &str = "call-932-history";
const RENEWED_CALL: &str = "call-932-renewed";

// ═══════════════════════════════════════════════════════════════ the tracks

/// The curator of a talky (the role its instance runs): the push enriches the
/// question and the window shows short ids.
fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

/// One round on `in_curate` spoken in `round` (`Value::Null`: a round nobody
/// declared); returns the call that left for the model.
fn curate_in(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> Msg {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": round}),
        json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(text)], "system": mode("Be brief.")}),
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

/// A whole turn in `round`: the person's words in, the model's final answer
/// back on the tap (which carries the call's context, the round included).
/// Returns every `turn_write` the turn produced, in order.
fn turn_in(
    h: &mut Hive,
    round: &Value,
    session: &str,
    turn: &str,
    says: &str,
    reply: &str,
) -> Vec<Msg> {
    let call = curate_in(h, round, session, turn, says);
    let mut written = h.routed("turn_write");
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    written.extend(h.routed("turn_write"));
    written
}

/// The other round's rows after everything said so far, in the same session:
/// written straight into the ledger as `./intake` writes a row, each with a
/// block of its own and at `seq`s right above the newest row -- the lanes
/// cannot grow 640 rows in a test's time. Every lane write after it is newer.
fn flood(h: &mut Hive) {
    let newest = h.rows("SELECT MAX(seq) FROM wall")[0][0]
        .as_i64()
        .expect("a wall with rows");
    for i in 0..FLOOD {
        let at = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(newest + 1 + i as i64)
            .expect("a stamp");
        let (kind, el, final_) = if i % 2 == 0 {
            (
                "user",
                user(&format!(
                    "{EB_MARK} flood {i}: Quill and Yarrow wait at Kestrel."
                )),
                0,
            )
        } else {
            (
                "assistant",
                said(&format!("{EB_MARK} flood {i}: Zora keeps the shed shut.")),
                1,
            )
        };
        h.row_under(
            Some(CANON_EB),
            at,
            SESSION,
            &format!("t-eb-flood-{}", i / 2),
            kind,
            &el,
            final_,
        );
    }
}

/// Track A (`hidden` false): the three turns of {e,a}. Track B (`hidden`
/// true): the same three turns, a turn of {e,b} after each, then the flood of
/// {e,b} rows.
fn track(hidden: bool) -> Hive {
    let mut h = talky();
    for (i, (turn, says, reply)) in EA_TURNS.iter().enumerate() {
        turn_in(&mut h, &json!(ROUND_EA), SESSION, turn, says, reply);
        if hidden {
            let (turn, says, reply) = EB_TURNS[i];
            turn_in(&mut h, &json!(ROUND_EB), SESSION, turn, says, reply);
        }
    }
    if hidden {
        flood(&mut h);
        let n = h.rows(&format!(
            "SELECT COUNT(*) FROM wall WHERE audience_set = '{CANON_EB}'"
        ))[0][0]
            .as_i64()
            .unwrap_or(0);
        assert!(
            n >= (FLOOD + 2 * EB_TURNS.len()) as i64,
            "track B holds the other round's rows: {n}"
        );
    }
    let ea = h.rows(&format!(
        "SELECT COUNT(*) FROM wall WHERE audience_set = '{CANON_EA}'"
    ))[0][0]
        .as_i64()
        .unwrap_or(0);
    assert_eq!(
        ea,
        2 * EA_TURNS.len() as i64,
        "both tracks hold the same {{e,a}} rows"
    );
    h
}

/// A value with every key named in `drop` taken out, at any depth.
fn without(v: &Value, drop: &[&str]) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !drop.contains(&k.as_str()))
                .map(|(k, x)| (k.clone(), without(x, drop)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| without(x, drop)).collect()),
        other => other.clone(),
    }
}

/// The wall's own clock in an answer: `seq` is microseconds of the run that
/// wrote the row, `at`/`first_at`/`last_at` its wall-clock stamps -- they
/// differ between any two runs, hidden rows or not. Everything else stays.
const RUN_CLOCK: [&str; 4] = ["seq", "at", "first_at", "last_at"];

fn all_text(v: &Value) -> String {
    sj::to_string(v).unwrap_or_default()
}

// ════════════════════════════════════════════════════════════ the history

/// One cell's step, the way the hive's code runner hands it a message.
fn step(script: &str, params: &Map<String, Value>, name: &str, msg: &Msg) -> Vec<Msg> {
    let doc = json!({
        "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                     "target": format!("/x/curator/{name}"),
                     "reply_to": msg.reply_to},
        "body": msg.body,
        "params": params,
    });
    let out = run_python(script, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{name} exited non-zero: {err}");
    let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{name}: output is not JSON ({e}): {}",
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
/// shape (the harness's `store`, with the store's `covers` filter as real as
/// every other operator). A refused op fails the test where it happens.
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
    for r in body["results"].as_array().cloned().unwrap_or_default() {
        assert!(
            r["error_code"].as_str().is_none(),
            "a ledger op failed: {r}"
        );
    }
    assert!(
        hop.get("error_code").and_then(Value::as_str).is_none(),
        "a ledger op failed: {body}"
    );
    Msg {
        context: msg.context.clone(),
        hop,
        body: obj(body),
        reply_to: String::new(),
    }
}

/// One history call on `in_history_call` in the dispatcher's shape, in
/// `round`, during the probe turn of the session; returns the answer's
/// payload. The shipped script runs with HISTORY_KNOBS; its store messages
/// take the edge `./history -> ./ledger` (the `set_context` of `cur_*` out of
/// the hop, as the hive declares it) and the answer comes back on
/// `./ledger -> ./history` with that context.
fn history(h: &Hive, round: &str, tool: &str, args: Value) -> Value {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    for (k, v) in HISTORY_KNOBS {
        assert!(params.contains_key(k), "no such history knob: {k}");
        params.insert(k.to_string(), json!(v));
    }
    let first = Msg {
        context: obj(
            json!({"session_id": SESSION, "turn_id": PROBE_TURN, "iter": "1",
                            "curator_call": "cc-932", "audience_set": round}),
        ),
        hop: obj(json!({"route": "in_history_call", "tool_name": tool,
                        "tool_call_id": HISTORY_CALL})),
        body: obj(
            json!({"messages": [{"origin": "assistant", "type": "tool_call",
                                       "id": HISTORY_CALL, "text": args.to_string()}]}),
        ),
        reply_to: String::new(),
    };
    let mut queue = VecDeque::from([first]);
    let mut answers = Vec::new();
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 200, "the history call does not come to rest");
        for out in step(&script, &params, "history", &msg) {
            match out.route() {
                "lstore" => {
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
                    queue.push_back(ledger_answer(&h.db, &req));
                }
                "tool_result" => answers.push(out),
                other => panic!("history emitted on `{other}`: {:?}", out.hop),
            }
        }
    }
    assert_eq!(answers.len(), 1, "one call, one answer");
    let a = &answers[0];
    assert!(
        a.hop
            .get("error_code")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty),
        "{tool} refused: {:?}",
        a.hop
    );
    let turns = a.messages();
    assert_eq!(turns.len(), 1, "one tool_result turn: {turns:?}");
    sj::from_str(turns[0]["text"].as_str().expect("a text")).expect("one JSON object")
}

/// The pages of a history search and an outline: hits (ids, sessions, turns,
/// kinds, excerpts, neighbours), `scanned`, `total_hits`, `truncated_scan`,
/// `cut_by` and `stopped_at_limit` are those of the same wall without the
/// other round's rows -- where a page ends and what was counted hang on the
/// visible rows alone. Before GH #932 track B answered in the cut form: the
/// bounded read met only hidden rows.
#[test]
fn a_history_page_of_a_round_is_the_same_beside_rows_it_may_not_see() {
    if !shipped() {
        return;
    }
    let asks = [
        (
            "history_search",
            json!({"query": "cw932", "mode": "exact", "limit": 2, "context": 1}),
        ),
        (
            "history_search",
            json!({"query": "cw932", "mode": "exact", "limit": 20}),
        ),
        ("history_outline", json!({})),
    ];
    let a = track(false);
    let b = track(true);
    for (tool, args) in asks {
        let pa = without(&history(&a, ROUND_EA, tool, args.clone()), &RUN_CLOCK);
        let pb = without(&history(&b, ROUND_EA, tool, args.clone()), &RUN_CLOCK);
        if tool == "history_search" {
            assert!(
                pa["hits"].as_array().is_some_and(|h| !h.is_empty()),
                "{tool} {args}: track A finds the round's own words: {pa}"
            );
            assert!(
                pa["scanned"].as_i64().is_some_and(|n| n > 0),
                "{tool} {args}: track A says how far it read: {pa}"
            );
        } else {
            assert!(
                pa["sessions"].as_array().is_some_and(|s| s.len() == 1),
                "{tool}: track A outlines its one session: {pa}"
            );
        }
        assert_eq!(
            pb, pa,
            "{tool} {args}: the other round's rows moved the answer of {{e,a}}"
        );
        assert!(
            !all_text(&pb).contains(EB_MARK),
            "{tool} {args}: a word of the other round: {pb}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════ the push

/// The hop of the collector's ask (collector `head` + `recall_ask`).
fn ask_hop(session: &str, turn: &str, text: &str) -> Value {
    json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
           "recall_query": text, "memory_tier": "1",
           "recall_window_from": "", "recall_window_to": ""})
}

/// The collector's ask of the probe turn in `round`; the one `recall` that
/// left the hive for it.
fn push_ask(h: &mut Hive, round: &str) -> Msg {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": SESSION, "channel": "test", "audience_set": round}),
        ask_hop(SESSION, PROBE_TURN, PROBE_SAYS),
        json!({"messages": [user(PROBE_SAYS)]}),
    );
    let asks = h.routed("recall");
    assert_eq!(asks.len(), 1, "every ask leaves exactly once: {:?}", h.out);
    asks[0].clone()
}

/// The push window -- the newest rounds of the wall the question's entities
/// come from -- is the window of the same wall without the other round's
/// rows: the same `recall` hop, the same question, word for word. Before GH
/// #932 the push read the newest 48 rows and gated them afterwards, so in
/// track B it met only hidden rows and asked without a single entity.
#[test]
fn the_push_window_of_a_round_is_the_same_beside_rows_it_may_not_see() {
    if !shipped() {
        return;
    }
    let mut a = track(false);
    let mut b = track(true);
    let qa = push_ask(&mut a, ROUND_EA);
    let qb = push_ask(&mut b, ROUND_EA);
    let query = qa.hop["recall_query"].as_str().unwrap_or("").to_string();
    assert!(
        query.contains("Rossio") && query.ends_with(PROBE_SAYS),
        "track A's question names what its round said: {query}"
    );
    assert_eq!(
        (&qb.hop, qb.messages()),
        (&qa.hop, qa.messages()),
        "the other round's rows moved the push window of {{e,a}}"
    );
    for word in ["Zora", "Yarrow", "Kestrel", "Quill"] {
        assert!(
            !all_text(&Value::Object(qb.hop.clone())).contains(word),
            "the question names `{word}` of the other round: {:?}",
            qb.hop
        );
    }
}

// ═════════════════════════════════════════════════════════════ the renewal

/// The member's renewal of the duplex session `SESSION` in `round`; the one
/// `sidecar` section that left the hive for it.
fn renewed(h: &mut Hive, round: &str) -> Msg {
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": SESSION, "call_id": RENEWED_CALL, "channel": "test",
               "audience_set": round}),
        json!({"call_id": RENEWED_CALL, "session_id": SESSION, "renewal_n": 1}),
        json!({"messages": []}),
    );
    let blocks = h.routed("sidecar");
    assert_eq!(blocks.len(), 1, "one renewal, one block: {:?}", h.out);
    blocks[0].clone()
}

/// The renewal block -- the newest `renew_rounds` rounds as text, built
/// without a model -- is the block of the same wall without the other
/// round's rows. Before GH #932 the renewal read the newest 600 rows and gated
/// them afterwards, so in track B its rounds were all hidden ones and the
/// block came out without a single turn.
#[test]
fn the_renewal_block_of_a_round_is_the_same_beside_rows_it_may_not_see() {
    if !shipped() {
        return;
    }
    let mut a = track(false);
    let mut b = track(true);
    let ra = renewed(&mut a, ROUND_EA);
    let rb = renewed(&mut b, ROUND_EA);
    let text = ra.body["payload"].as_str().unwrap_or("").to_string();
    for (_, says, reply) in EA_TURNS {
        assert!(
            text.contains(says) && text.contains(reply),
            "track A's block carries the round's turns: {text}"
        );
    }
    assert_eq!(
        (&rb.hop, &rb.body),
        (&ra.hop, &ra.body),
        "the other round's rows moved the renewal block of {{e,a}}"
    );
    assert!(
        !all_text(&Value::Object(rb.body.clone())).contains(EB_MARK),
        "a word of the other round in the block: {:?}",
        rb.body
    );
}

// ═══════════════════════════════════════════════════════════════ the window

/// The window that goes to the model -- its messages and its system part --
/// is the window of the same wall without the other round's rows. The model
/// call's own id is random per run and is left out; everything the model
/// reads is compared.
#[test]
fn the_window_of_a_round_is_the_same_beside_rows_it_may_not_see() {
    if !shipped() {
        return;
    }
    let mut a = track(false);
    let mut b = track(true);
    let wa = curate_in(&mut a, &json!(ROUND_EA), SESSION, PROBE_TURN, PROBE_SAYS);
    let wb = curate_in(&mut b, &json!(ROUND_EA), SESSION, PROBE_TURN, PROBE_SAYS);
    let seen = texts(&wa).join("\n");
    for (_, says, reply) in EA_TURNS {
        assert!(
            seen.contains(says) && seen.contains(reply),
            "track A's window carries the round's turns: {seen}"
        );
    }
    assert_eq!(
        (wb.messages(), wb.body.get("system")),
        (wa.messages(), wa.body.get("system")),
        "the other round's rows moved the window of {{e,a}}"
    );
    assert!(
        !all_text(&Value::Object(wb.body.clone())).contains(EB_MARK),
        "a word of the other round in the window"
    );
}

// ═════════════════════════════════════════════════════════════ the rebuild

/// A round both rounds of the session see: a row under it is a block {e,a}
/// and {e,b} share, so a mark of either round can name it.
const SHARED: &str = r#"["member:a","member:b","member:e"]"#;
const SHARED_TURN: &str = "t-shared";
const SHARED_SAYS: &str = "cw932 shared: the ferry to Faro leaves at noon.";
const SHARED_REPLY: &str = "cw932 shared reply: noted, the ferry leaves at noon.";
/// The turn of {e,b} the other round's marks are said in -- not the turn of
/// the shared rows, so a release of them is no release of the round's own.
const EB_MARK_TURN: &str = "t-eb-2";
const CROSS_TURN: &str = "t-ea-cross";
const CROSS_SAYS: &str = "cw932 ea four: please sum up what we settled.";
const CROSS_REPLY: &str = "cw932 ea four reply: the parcel, the lease and the market.";

/// The rows both rounds see, in the order they are sown.
fn shared_rows() -> [(&'static str, Value, i64); 2] {
    [
        ("user", user(SHARED_SAYS), 0),
        ("assistant", said(SHARED_REPLY), 1),
    ]
}

/// The newest `seq` of the wall and the marks.
fn newest_seq(h: &Hive) -> i64 {
    h.rows(
        "SELECT MAX(s) FROM (SELECT MAX(seq) AS s FROM wall \
         UNION ALL SELECT MAX(seq) AS s FROM marks)",
    )[0][0]
        .as_i64()
        .expect("a ledger with rows")
}

/// The shared rows, written as `./intake` writes a row, right after the
/// newest row.
fn sow_shared(h: &mut Hive) {
    let newest = newest_seq(h);
    for (i, (kind, el, final_)) in shared_rows().into_iter().enumerate() {
        let at = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(newest + 1 + i as i64)
            .expect("a stamp");
        h.row_under(Some(SHARED), at, SESSION, SHARED_TURN, kind, &el, final_);
    }
}

/// A mark of the model of {e,b} (`release` or `pin`) on block `id`, written
/// as `./intake` writes the mark a section of the model leaves, under the
/// round of {e,b}, right after the newest row.
fn eb_mark(h: &mut Hive, kind: &str, id: &str) {
    let seq = newest_seq(h) + 1;
    let at = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(seq).expect("a stamp");
    h.db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            seq,
            SESSION,
            EB_MARK_TURN,
            kind,
            format!("#{id}"),
            at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
            CANON_EB
        ],
    )
    .expect("a mark");
}

/// The block ids of the shared rows: the one the other round releases, the
/// one it pins.
fn shared_ids() -> (String, String) {
    let [(_, x, _), (_, y, _)] = shared_rows();
    (short_id(&x), short_id(&y))
}

/// A talky with a small window, so one report crosses `compress_at` (the
/// pattern of `gh935` and `curator_cells.rs`, `compress_at_crossed_rebuilds`).
fn small_talky() -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
        ("policy", "context_window", json!(1000)),
    ])
}

/// Track A (`hidden` false): the three turns of {e,a} and the shared rows
/// after the first. Track B (`hidden` true): the same, a turn of {e,b} after
/// each turn of {e,a}, and the model of {e,b} releasing the shared question
/// and pinning the shared answer.
fn rebuild_track(hidden: bool) -> Hive {
    let mut h = small_talky();
    for (i, (turn, says, reply)) in EA_TURNS.iter().enumerate() {
        turn_in(&mut h, &json!(ROUND_EA), SESSION, turn, says, reply);
        if hidden {
            let (turn, says, reply) = EB_TURNS[i];
            turn_in(&mut h, &json!(ROUND_EB), SESSION, turn, says, reply);
        }
        if i == 0 {
            sow_shared(&mut h);
        }
    }
    if hidden {
        let (x, y) = shared_ids();
        eb_mark(&mut h, "release", &x);
        eb_mark(&mut h, "pin", &y);
    }
    h
}

/// A rebuild in {e,a}: a turn of {e,a} whose report crosses `compress_at`,
/// the clock's strike (it carries the round of that call), the summarizer
/// answered with the same words in every track.
fn rebuild_in_ea(h: &mut Hive) {
    let call = curate_in(h, &json!(ROUND_EA), SESSION, CROSS_TURN, CROSS_SAYS);
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 600}),
        json!([said(CROSS_REPLY)]),
    );
    let add = last_add(h);
    assert_eq!(add.body["emit_body"]["reason"], "compress", "{add:?}");
    h.fire(&add);
    while !h.summ.is_empty() {
        h.answer("cw932 summary: the parcel, the lease, the market.", "stop");
    }
    assert_ne!(h.plan(), json!({}), "the rebuild finished: {:?}", h.stderr);
}

/// The shape of the stored plan in terms of the rows {e,a} sees: `cover`
/// and `keep` are `seq`s -- microseconds of the run that wrote the row,
/// different between any two runs -- so each is named by the rows it stands
/// for (`turn_id`, `kind`, `hash` of every visible row at or below the cover,
/// of each kept row); `released`, `shrunk` and `stubs` are block ids and
/// tool names and are taken as they are.
fn plan_shape(h: &Hive) -> Value {
    let plan = h.plan();
    let visible = format!("audience_set IN ('{CANON_EA}', '{SHARED}', '[\"*\"]')");
    let cover = plan["cover"].as_i64().unwrap_or(0);
    let below = h.rows(&format!(
        "SELECT turn_id, kind, hash FROM wall WHERE {visible} AND seq <= {cover} ORDER BY seq"
    ));
    let keep: Vec<Value> = plan["keep"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            json!(h.rows(&format!(
                "SELECT turn_id, kind, hash, audience_set FROM wall WHERE seq = {}",
                s.as_i64().unwrap_or(0)
            )))
        })
        .collect();
    json!({"cover": below, "keep": keep, "released": plan["released"],
           "shrunk": plan["shrunk"], "stubs": plan["stubs"]})
}

/// Review I-1, ruling (b): the shape of a rebuild follows from the rows of
/// the round it is made in and nothing else. Track B holds, beside the same
/// {e,a} turns, the turns of {e,b} and two marks the model of {e,b} left on
/// blocks {e,a} sees too (a release, a pin); a rebuild in {e,a} then cuts the
/// same window in both: the next {e,a} call reads the same messages and the
/// same system part, and the stored plan has the same `cover`, `keep`,
/// `released`, `shrunk` and `stubs`. Before the ruling the rebuild read every
/// row and every mark, so {e,a} saw a block released by a round it was not
/// in -- and the window's size hung on rows it could not see.
#[test]
fn the_window_shape_of_a_round_is_the_same_with_and_without_another_rounds_rows_and_marks() {
    if !shipped() {
        return;
    }
    let mut a = rebuild_track(false);
    let mut b = rebuild_track(true);
    rebuild_in_ea(&mut a);
    rebuild_in_ea(&mut b);
    assert_eq!(
        plan_shape(&b),
        plan_shape(&a),
        "the other round's rows or marks moved the plan of a rebuild in {{e,a}}"
    );
    let wa = curate_in(&mut a, &json!(ROUND_EA), SESSION, PROBE_TURN, PROBE_SAYS);
    let wb = curate_in(&mut b, &json!(ROUND_EA), SESSION, PROBE_TURN, PROBE_SAYS);
    let seen = all_text(&json!({"messages": wa.messages(), "system": wa.body.get("system")}));
    assert!(
        seen.contains(SHARED_SAYS) || seen.contains(&shared_ids().0),
        "track A's window knows the shared question: {seen}"
    );
    assert_eq!(
        (wb.messages(), wb.body.get("system")),
        (wa.messages(), wa.body.get("system")),
        "the other round's rows or marks moved the window of {{e,a}} after a rebuild"
    );
    assert!(
        !all_text(&Value::Object(wb.body.clone())).contains(EB_MARK),
        "a word of the other round in the window"
    );
}

/// Review I-1, ruling (b), the other half: the marks of a round a rebuild is
/// not made in are kept, never lost. The rebuild in {e,a} reads only marks
/// its round may see (`rebuild_where`), so it neither records the release
/// and the pin of {e,b} (`marks_on` holds no {e,b} entry) nor moves past
/// them: the cursor of {e,a} moves (`marks_to_by`), the floor `marks_to`
/// and the cursor of {e,b} stay below both marks, so the next rebuild in
/// {e,b} still reads them (OR-S3.K.13). What {e,a} sees released
/// (`released`) does not carry them. Read off the plan; the harness's single
/// plan cannot be rebuilt in {e,b} without moving the cover past the shared
/// rows first.
#[test]
fn another_rounds_marks_survive_a_rebuild_under_this_one() {
    if !shipped() {
        return;
    }
    let mut b = rebuild_track(true);
    rebuild_in_ea(&mut b);
    let plan = b.plan();
    let (x, y) = shared_ids();
    assert_eq!(plan["round"], CANON_EA, "the round of the rebuild: {plan}");
    let eb_seqs: Vec<i64> = b
        .rows(&format!(
            "SELECT seq FROM marks WHERE audience_set = '{CANON_EB}' \
             AND kind IN ('release', 'pin')"
        ))
        .iter()
        .map(|r| r[0].as_i64().expect("an integer seq"))
        .collect();
    let first_eb = *eb_seqs.iter().min().expect("the marks of {e,b} were said");
    let floor = plan["marks_to"].as_i64().unwrap_or(0);
    let eb_cursor = plan["marks_to_by"][CANON_EB].as_i64().unwrap_or(floor);
    assert!(
        floor < first_eb && eb_cursor < first_eb,
        "the rebuild in {{e,a}} moved the cursor of {{e,b}} past its marks \
         {eb_seqs:?}: {plan}"
    );
    assert!(
        plan["marks_to_by"][CANON_EA].is_number(),
        "the cursor of {{e,a}} is its own: {plan}"
    );
    for (id, kind) in [(&x, "release"), (&y, "pin")] {
        assert!(
            plan["marks_on"][id.as_str()][CANON_EB].is_null(),
            "the {kind} of {{e,b}} on #{id} is not read by a rebuild in {{e,a}}: {plan}"
        );
    }
    assert!(
        !plan["released"]
            .as_array()
            .is_some_and(|r| r.iter().any(|i| i.as_str() == Some(x.as_str()))),
        "{{e,a}} sees a block released by {{e,b}}: {plan}"
    );
    let marks = b.rows(&format!(
        "SELECT COUNT(*) FROM marks WHERE audience_set = '{CANON_EB}' \
         AND kind IN ('release', 'pin')"
    ));
    assert_eq!(marks[0][0], json!(2), "the marks of {{e,b}} stand");
}

// ═══════════════════════════════════════════════════════════ the turn index

/// `<session>#<tag>-<n>` for `canon`: the tag is the first 8 hex digits of
/// the sha256 of the canonical round.
fn turn_id_of(canon: &str, n: usize) -> String {
    format!("{SESSION}#{}-{n}", &sha256_hex(canon)[..8])
}

/// A `turn_write` without the episode's wall-clock stamp (`happened_at`),
/// which differs between two runs whatever the ledger holds.
fn episode(m: &Msg) -> (Value, Vec<Value>) {
    let mut hop = m.hop.clone();
    hop.remove("happened_at");
    (Value::Object(hop), m.messages())
}

/// The turn index counts the participant turns of a session IN ITS ROUND,
/// from 0 and without gaps, under the round's tag: the episodes of {e,a} --
/// the person's words, the model's answer, the person's next words -- are
/// `<session>#<tag>-0`, `-1`, `-2` with `turn_index` "0", "1", "2" whether or
/// not a turn of {e,b} stood between them, and that turn's episodes count on
/// their own under a tag of their own. Before GH #932 the id was
/// `<session>#<n>` over every round, so track B's {e,a} episodes ran 0, 1, 4
/// -- a gap of exactly the turns the round did not see.
#[test]
fn the_turn_index_counts_only_the_turns_of_its_own_round() {
    if !shipped() {
        return;
    }
    let ea = json!(ROUND_EA);
    let (t1, says1, reply1) = EA_TURNS[0];
    let (t2, says2, _) = EA_TURNS[1];
    let mut tracks = Vec::new();
    let mut eb_written = Vec::new();
    for hidden in [false, true] {
        let mut h = talky();
        let mut written = turn_in(&mut h, &ea, SESSION, t1, says1, reply1);
        if hidden {
            let (tb, saysb, replyb) = EB_TURNS[0];
            eb_written = turn_in(&mut h, &json!(ROUND_EB), SESSION, tb, saysb, replyb);
        }
        curate_in(&mut h, &ea, SESSION, t2, says2);
        written.extend(h.routed("turn_write"));
        tracks.push(written);
    }
    for written in &tracks {
        let ids: Vec<&str> = written
            .iter()
            .map(|m| m.hop["turn_id"].as_str().unwrap_or(""))
            .collect();
        let want: Vec<String> = (0..3).map(|n| turn_id_of(CANON_EA, n)).collect();
        assert_eq!(ids, want, "the {{e,a}} episodes in their round's form");
        let index: Vec<&str> = written
            .iter()
            .map(|m| m.hop["turn_index"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(index, ["0", "1", "2"], "counted in the round, from 0");
    }
    assert_eq!(
        tracks[1].iter().map(episode).collect::<Vec<_>>(),
        tracks[0].iter().map(episode).collect::<Vec<_>>(),
        "the turn of {{e,b}} moved the episodes of {{e,a}}"
    );
    let eb_ids: Vec<&str> = eb_written
        .iter()
        .map(|m| m.hop["turn_id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        eb_ids,
        [turn_id_of(CANON_EB, 0), turn_id_of(CANON_EB, 1)],
        "the other round counts on its own, under its own tag"
    );
    assert_ne!(
        &sha256_hex(CANON_EA)[..8],
        &sha256_hex(CANON_EB)[..8],
        "two rounds of one session never share an id"
    );
}

// ═════════════════════════════════════════════════════ the call without a round

const NONE_SESSION: &str = "s-932-none";
const NONE_ONE_SAYS: &str = "cw932 none one: the bread is in the blue tin.";
const NONE_ONE_REPLY: &str = "cw932 none one reply: noted, the blue tin.";
const NONE_TWO_SAYS: &str = "cw932 none two: and where was the bread again?";
/// A row from before the audience rule, in the same session.
const OLD_SAYS: &str = "cw932 old: the spare battery sits behind the clock.";

/// PP-BD-12: a call without a round writes its rows with `[]`, and the next
/// call of the same session without a round sees them in the window that
/// goes to the model -- found by value in the store -- while a row from
/// before the audience rule (NULL), sown into the same session, reaches it
/// never. Before GH #932 such rows were written NULL and a round-less call
/// let every NULL row of its session through, the old one included.
#[test]
fn a_call_without_a_round_sees_its_sessions_empty_rows_and_never_a_null_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn_in(
        &mut h,
        &Value::Null,
        NONE_SESSION,
        "t-none-1",
        NONE_ONE_SAYS,
        NONE_ONE_REPLY,
    );
    let stored: Vec<Value> = h
        .rows(&format!(
            "SELECT audience_set FROM wall WHERE session_id = '{NONE_SESSION}' \
             AND turn_id = 't-none-1' ORDER BY seq"
        ))
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert!(stored.len() >= 2, "the turn left its rows: {stored:?}");
    assert!(
        stored.iter().all(|a| a == "[]"),
        "a row without a round carries `[]`, never NULL: {stored:?}"
    );
    h.row_under(
        None,
        chrono::Utc::now() - chrono::Duration::minutes(5),
        NONE_SESSION,
        "t-none-old",
        "user",
        &user(OLD_SAYS),
        0,
    );
    let call = curate_in(
        &mut h,
        &Value::Null,
        NONE_SESSION,
        "t-none-2",
        NONE_TWO_SAYS,
    );
    let seen = texts(&call).join("\n");
    assert!(
        !seen.contains(OLD_SAYS),
        "a row from before the rule reached a call without a round: {seen}"
    );
    assert!(
        seen.contains(NONE_ONE_SAYS) && seen.contains(NONE_ONE_REPLY),
        "the session's own rows without a round reach its next call: {seen}"
    );
    assert!(seen.contains(NONE_TWO_SAYS), "the running turn: {seen}");
}
