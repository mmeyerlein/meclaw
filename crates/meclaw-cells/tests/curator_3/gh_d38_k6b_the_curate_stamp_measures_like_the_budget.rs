//! GH #1434 (D38 K6b, OR-38-29 B3) -- the `curate` stamp's `raw_tokens` is the
//! budget's own figure of the same rounds.
//!
//! WHY this file exists: two numbers said "raw". The raw token budget (GH
//! #1427) measured a round by its blocks' characters over the row's rate; the
//! turn's `curate` stamp measured the same rows by `estimate_tokens` of their
//! message elements, JSON form and all, at a fixed three bytes a token -- the
//! bench of the wave read the stamp at 1.37-1.40 times the budget with the
//! collection off (receipt `fensterbank`, B3). The watcher (`curator/stats`
//! kind `window`, GH #1426) reads the stamp, the knob is set in the budget's
//! unit: one estimate for both, or the knob and the watch never meet. Now the
//! stamp measures the raw part as the plan does: the wall rows above the
//! plan's cover, a shrunk or released row at `FORM_CHARS`, every other at its
//! block's characters, over the rate the plan counts with.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); deterministic,
//! no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{
    Hive, Msg, ROUND_E, last_add, mode, said, shipped, short_id, texts, tool_call, tool_result,
    turn, user,
};
use meclaw_core::serde_json;
use meclaw_core::serde_json::{Value, json};

/// The policy's rate without a measured one, characters a token.
const CHARS_PER_TOKEN: i64 = 3;
/// `FORM_CHARS` of the policy: the one-line form of a shrunk, released or
/// expired block.
const FORM_CHARS: i64 = 90;

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

/// The wall as the ledger holds it, oldest first: `(turn_id, kind, chars)` of
/// every row's block.
fn wall_rows(h: &Hive) -> Vec<(String, String, i64)> {
    h.rows(
        "SELECT w.turn_id, w.kind, b.chars FROM wall w JOIN blocks b ON b.hash = w.hash \
         ORDER BY w.seq",
    )
    .into_iter()
    .map(|r| {
        (
            r[0].as_str().unwrap().to_string(),
            r[1].as_str().unwrap().to_string(),
            r[2].as_i64().unwrap(),
        )
    })
    .collect()
}

/// Five rounds of today, then a call: the stamp's `raw_tokens` is the sum of
/// the characters of the wall rows the window sent raw over the policy's rate
/// -- the five rounds and the running round's rows that stood on the wall at
/// the call (its answer comes later), `raw_rounds` 6. No rebuild has run, so the cover is 0 and
/// nothing is shrunk or released: the figure is the budget's figure of the
/// same rounds, byte for byte, and not `estimate_tokens` of their elements.
#[test]
fn the_stamps_raw_tokens_is_the_budgets_figure() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..5 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        // Rows of 600 and 900 characters: the JSON form of an element adds a
        // fixed share the budget never counted.
        let asked = format!("{i} {}", "q".repeat(598));
        let answered = format!("{i} {}", "a".repeat(898));
        h.row(at, "s", &format!("r{i}"), "user", &user(&asked), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&answered),
            1,
        );
    }
    let call = h.curate("s", "t1", 0, json!([user("and now?")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 6_000, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said("done")]),
    );
    let s = stamp(&h);
    let raw_rounds = s["raw_rounds"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_rounds in the stamp: {s} {:?}", h.stderr));
    let raw_tokens = s["raw_tokens"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_tokens in the stamp: {s}"));
    assert!(
        h.plan().get("cover").is_none(),
        "no rebuild ran, the cover is 0: {}",
        h.plan()
    );

    let rows = wall_rows(&h);
    assert!(
        rows.len() >= 10,
        "the five rounds stand on the wall: {rows:?}"
    );
    let earlier: i64 = rows
        .iter()
        .filter(|(turn, _, _)| turn != "t1")
        .map(|(_, _, chars)| chars)
        .sum();
    let running: i64 = rows
        .iter()
        .filter(|(turn, kind, _)| turn == "t1" && kind != "assistant")
        .map(|(_, _, chars)| chars)
        .sum();
    // The running round's words stand on the wall at the call and go out
    // with it: six rounds, one figure (review M-3 of GH #1434).
    assert_eq!(raw_rounds, 6, "the five rounds and the running one: {s}");
    let expected = (earlier + running) / CHARS_PER_TOKEN;
    assert_eq!(
        raw_tokens, expected,
        "the stamp measures the raw part as the budget does (characters of the wall rows \
         sent raw over the rate): stamp {s}, wall {rows:?}"
    );
}

/// The quick seam of a simple turn: the round in on `in_curate` with the
/// path the router named.
fn quick_seam(h: &mut Hive, session: &str, turn: &str, messages: Value) {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "simple", "families": "weather"});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

/// A quick call sends the running round and the last `keep_recent` (4)
/// elements before it -- here the two newest of six earlier rounds. The raw
/// part of its window (`window_est:<call>`, which the tap copies into the
/// `curate` stamp) counts what that call sends: the rows of those two rounds
/// and the running round's words, by the budget's estimate, and so it never
/// exceeds the projection of the same call. Before the fix of review I-1
/// (GH #1434) the figure summed every wall row above the cover -- six rounds
/// the quick call never sent, above its own `window_estimate`.
#[test]
fn a_quick_calls_raw_part_counts_what_it_sends() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..6 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        let asked = format!("round {i} {}", "q".repeat(592));
        let answered = format!("round {i} {}", "a".repeat(892));
        h.row(at, "s", &format!("r{i}"), "user", &user(&asked), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&answered),
            1,
        );
    }
    quick_seam(&mut h, "s", "t9", json!([user("and the weather?")]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "the turn goes on quick: {:?} {:?}",
        h.out,
        h.stderr
    );
    let sent: Vec<String> = quick[0]
        .messages()
        .iter()
        .filter(|el| el["origin"] != json!("system"))
        .map(|el| el["text"].as_str().unwrap_or("").to_string())
        .collect();
    for i in 0..4 {
        assert!(
            !sent.iter().any(|t| t.contains(&format!("round {i} "))),
            "round {i} lies before the quick window: {sent:?}"
        );
    }
    for i in 4..6 {
        assert_eq!(
            sent.iter()
                .filter(|t| t.contains(&format!("round {i} ")))
                .count(),
            2,
            "both rows of round {i} are in the quick window: {sent:?}"
        );
    }

    let id = quick[0].hop["curator_call"].as_str().expect("a call id");
    let est: Value = serde_json::from_str(&h.state(&format!("window_est:{id}")))
        .unwrap_or_else(|_| panic!("the call's projection: {:?}", h.stderr));
    let raw_tokens = est["raw_tokens"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_tokens in the projection: {est}"));
    let projected = est["est"].as_i64().expect("the projection of the call");
    assert_eq!(
        est["raw_rounds"],
        json!(3),
        "two earlier rounds and the running one: {est}"
    );

    let sent_chars: i64 = wall_rows(&h)
        .iter()
        .filter(|(turn, _, _)| ["r4", "r5", "t9"].contains(&turn.as_str()))
        .map(|(_, _, chars)| chars)
        .sum();
    assert_eq!(
        raw_tokens,
        sent_chars / CHARS_PER_TOKEN,
        "the raw part is the budget's figure of the rows the quick call sends: {est}"
    );
    assert!(
        raw_tokens <= projected,
        "the raw part of a call never exceeds the call: {est}"
    );
}

/// The projection of `call` (`window_est:<call>`): `(raw_tokens, raw_rounds,
/// est)`.
fn projection(h: &Hive, call: &Msg) -> (i64, i64, i64) {
    let id = call.hop["curator_call"].as_str().expect("a call id");
    let est: Value = serde_json::from_str(&h.state(&format!("window_est:{id}")))
        .unwrap_or_else(|_| panic!("the call's projection: {:?}", h.stderr));
    (
        est["raw_tokens"]
            .as_i64()
            .unwrap_or_else(|| panic!("raw_tokens in the projection: {est}")),
        est["raw_rounds"]
            .as_i64()
            .unwrap_or_else(|| panic!("raw_rounds in the projection: {est}")),
        est["est"]
            .as_i64()
            .unwrap_or_else(|| panic!("the projection of the call: {est}")),
    )
}

/// Tokens of `chars` at the rate the policy measures with: the answering
/// model's `chars_per_token` the tap kept, else the policy's own.
fn at_rate(h: &Hive, chars: i64) -> i64 {
    let rate = h
        .state("chars_per_token")
        .parse::<f64>()
        .unwrap_or(CHARS_PER_TOKEN as f64);
    (chars as f64 / rate) as i64
}

/// Every wall row above `cover`, oldest first: `(short id, kind, chars)` of
/// its block.
fn rows_above(h: &Hive, cover: i64) -> Vec<(String, String, i64)> {
    h.rows(&format!(
        "SELECT w.hash, w.kind, b.chars FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.seq > {cover} ORDER BY w.seq"
    ))
    .into_iter()
    .map(|r| {
        (
            r[0].as_str().unwrap()[..12].to_string(),
            r[1].as_str().unwrap().to_string(),
            r[2].as_i64().unwrap(),
        )
    })
    .collect()
}

/// A transient tool result whose `valid_until` ran out before the plan is
/// sent in its one-line form (`EXPIRED_FORM`), and the raw part of the call
/// counts it at `FORM_CHARS` as the budget does (review m-1 of GH #1434):
/// after a rebuild, the call's `raw_tokens` is the characters of the rows
/// above the cover over the measured rate, the expired result at its form.
#[test]
fn the_raw_part_counts_an_expired_result_at_its_form() {
    if !shipped() {
        return;
    }
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let result = {
        let mut r = tool_result("c1", &format!("the weather now: {}", "sunny ".repeat(200)));
        r["valid_until"] = json!(past);
        r
    };
    let rid = short_id(&result);
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let call = h.curate(
        "s",
        "t1",
        1,
        json!([user("weather"), tool_call("c1", "weather"), result.clone()]),
        mode("x"),
    );
    h.tap(&call, "stop", json!({}), json!([said("sunny")]));
    let call = h.curate("s", "t2", 0, json!([user("and now")]), mode("x"));
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("?")]),
    );
    h.fire(&last_add(&h));
    let call = h.curate("s", "t3", 0, json!([user("x")]), mode("x"));
    assert!(
        texts(&call).contains(&format!("[#{rid} expired]")),
        "the window sends the result in its one-line form: {:?}",
        texts(&call)
    );
    let (raw_tokens, raw_rounds, _) = projection(&h, &call);
    let cover = h.plan()["cover"].as_i64().unwrap_or(0);
    let rows = rows_above(&h, cover);
    assert!(
        rows.iter()
            .any(|(id, _, chars)| *id == rid && *chars > FORM_CHARS),
        "the expired result stands above the cover, longer than its form: {rows:?}"
    );
    let chars: i64 = rows
        .iter()
        .map(|(id, _, chars)| if *id == rid { FORM_CHARS } else { *chars })
        .sum();
    assert_eq!(raw_rounds, 3, "{rows:?}");
    assert_eq!(
        raw_tokens,
        at_rate(&h, chars),
        "the expired result counts at its one-line form: {rows:?} {:?}",
        texts(&call)
    );
}

/// An earlier answer whose sidecar block is over the window's share of it
/// (`SIDECAR_SHARE` of `input_soft`) is sent cut -- the answer and the mark
/// where the block stood (`SIDECAR_DROPPED`) --, and the raw part of the call
/// counts it at the characters sent, so it never exceeds the projection of
/// the same call (the rest list of the review of GH #1434). Before, the raw
/// part counted the whole block.
#[test]
fn the_raw_part_counts_a_cut_sidecar_answer_as_sent() {
    if !shipped() {
        return;
    }
    let big = format!(
        "long.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```",
        "x".repeat(30_000)
    );
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s", "t1", "q1", &big, json!({"input_soft": 1000}));
    let call = h.curate("s", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let cut = texts(&call)
        .into_iter()
        .find(|t| t.contains("[dropped: sidecar"))
        .unwrap_or_else(|| panic!("the answer is sent cut: {:?}", texts(&call)));
    let (raw_tokens, raw_rounds, est) = projection(&h, &call);
    let rows = rows_above(&h, 0);
    let chars: i64 = rows
        .iter()
        .map(|(_, kind, chars)| {
            if kind == "assistant" {
                cut.chars().count() as i64
            } else {
                *chars
            }
        })
        .sum();
    assert_eq!(raw_rounds, 2, "{rows:?}");
    assert_eq!(
        raw_tokens,
        at_rate(&h, chars),
        "the cut answer counts at the characters sent: {rows:?}"
    );
    assert!(
        raw_tokens <= est,
        "the raw part of a call never exceeds the call: raw {raw_tokens}, est {est}"
    );
}
