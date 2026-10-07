//! GH #943 -- a curator keeps one window plan per round.
//!
//! Until GH #943 the curator kept ONE plan of the window per hive
//! (`state.window_plan`), and beside it one rebuild claim, one list of pending
//! actions, one pending summary, one summary leaf and one standing order on
//! the clock. GH #932 made a rebuild read only the rows and marks of the round
//! whose call started it, and named what was left in its module doc (point
//! 5): where a rebuild that ANOTHER round caused cuts the window is a
//! residual channel. It was more than a channel. A rebuild of round B set the
//! cover, the releases, the shrunk results and the stubs of the one plan, and
//! the next call of round A was cut there -- A lost rows of its own on B's
//! account, and the shape of A's window told A that B had been active. The
//! one clock order was re-armed by B's call, so A's rebuild was dropped as
//! stale; one claim meant B's rebuild waited for A's summarizer; one action
//! list handed A's rebuild report to B's next call. Each of these moves what
//! a round is shown on the account of a round it may not see -- the
//! information flow the audience rule (GH #925) exists to stop.
//!
//! Since GH #943 every one of those rows is kept per round, under the round's
//! key `round_key` = the first 12 hex digits of the sha256 of the canonical
//! round (a call without a round has the key of `[]`): `window_plan:<rk>`,
//! `rebuild_running:<rk>`, `last_call:<rk>`, `armed_call:<rk>`,
//! `actions_pending:<rk>`, `pending:summary:<rk>`, `summary_audience:<rk>`,
//! one clock order per round. The plans are bounded: the registry
//! `window_plans` (`{rk: last_used_ms}`, "used" = the round's last rebuild)
//! drops a round's plan and rows when more than `max_round_plans` rounds hold
//! one (the least recently rebuilt first) or when a round has not been
//! rebuilt for `round_plan_idle_s`; a dropped round builds its next window
//! from its own rows again.
//!
//! The locks here:
//!
//! 1. the key: one key per canonical round (order, duplicates, whitespace
//!    do not count; `[]` and a round nobody declared share one), the same in
//!    the policy and in the harness; the bound's defaults and its pure
//!    pruning (`prune_plans`) by table;
//! 2. a rebuild never cuts another round's window: a track with only the
//!    turns of {e,a} and a track with the same turns between turns of {e,b}
//!    and a forced rebuild of {e,b} answer the next call of {e,a} with the
//!    same plan, the same turn index and the same window, byte for byte; and
//!    the marks of {e,b} survive a rebuild of {e,a} and shape the next
//!    rebuild of {e,b};
//! 3. two rounds rebuild side by side: each claims its own row (the first
//!    claim of a round creates it), each waits for its own summary, a second
//!    trigger of the same round merges, a call of B between A's call and A's
//!    rebuild does not discard A's, and each round's report reaches only
//!    that round's next call -- and each call fetches only its own round's
//!    summary leaf (review I-1); the handover leaf of a session falls with
//!    the first rebuild of ITS round only (review I-2); intake keys a round
//!    as the policy does (review M-2);
//! 4. the plans are bounded: past `max_round_plans` the least recently
//!    rebuilt round loses its plan, an idle round loses its plan, and the
//!    round that lost it is answered from its own rows exactly as a ledger
//!    that never held another round.
//! 5. GH #949 (the findings of review W on GH #943): everything a dropped
//!    round owns falls with its plan -- `summary_audience:<rk>` and the slot
//!    `history.summary:<rk>` too (M-1) -- and the round that comes back sees
//!    its own summary on its first call (the leaf laid again); a strike of
//!    its old clock order is stale; nothing falls beside a registry put that
//!    lost (M-3); a handover leaf bound to no audience falls at the next
//!    rebuild of any round (M-4); the ledger seed holds no bare key of a
//!    round (M-7).
//!
//! The hive runs in one process (`support/curator_hive.rs`): the shipped
//! `script_inline` programs under python3, the shipped edges under the
//! colony's own CEL, an in-memory ledger through the store's own dispatcher.
//! Only the clock and the summarizer are answered by hand. No provider, no
//! fixed time window: where the day of a row counts (the talky role keeps
//! today raw), a test stays clear of midnight UTC. Guarded like every
//! template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

// ═══════════════════════════════════════════════════════════════ the rounds

/// The session every round speaks in.
const SESSION: &str = "s-943";

/// The round of the first track, as a channel stamps it (unsorted, with
/// spaces), and as the ledger stores it (canonical).
const ROUND_EA: &str = r#"["member:e", "member:a"]"#;
const CANON_EA: &str = r#"["member:a","member:e"]"#;
/// The other round of the same session.
const ROUND_EB: &str = r#"["member:e", "member:b"]"#;
const CANON_EB: &str = r#"["member:b","member:e"]"#;

/// A round both rounds see: a row under it is a block {e,a} and {e,b} share,
/// so a mark of {e,b} can name a block {e,a} shows too.
const SHARED: &str = r#"["member:a","member:b","member:e"]"#;

const EA_TURNS: [(&str, &str, &str); 3] = [
    (
        "t-ea-1",
        "cw943 ea one: the parcel left the depot on time.",
        "cw943 ea one reply: noted, the parcel is on its way.",
    ),
    (
        "t-ea-2",
        "cw943 ea two: the lease is signed by both parties.",
        "cw943 ea two reply: noted, the signed lease is filed.",
    ),
    (
        "t-ea-3",
        "cw943 ea three: the market opens early on the square.",
        "cw943 ea three reply: noted, the market opens early.",
    ),
];
const EB_TURNS: [(&str, &str, &str); 3] = [
    (
        "t-eb-1",
        "cw943 eb one: the spare key lies under the grey stone.",
        "cw943 eb one reply: noted, the key is under the stone.",
    ),
    (
        "t-eb-2",
        "cw943 eb two: the boat waits at the north pier.",
        "cw943 eb two reply: noted, the boat is at the north pier.",
    ),
    (
        "t-eb-3",
        "cw943 eb three: the shed code is the year it was built.",
        "cw943 eb three reply: noted, the shed opens with that year.",
    ),
];
/// What only {e,b} said: no window of {e,a} may carry it.
const EB_MARK: &str = "cw943 eb";

const SHARED_TURN: &str = "t-shared";
const SHARED_SAYS: &str = "cw943 shared: the ferry leaves at noon.";
const SHARED_REPLY: &str = "cw943 shared reply: noted, the ferry leaves at noon.";
/// The turn of {e,b} its marks are said in -- not the turn of the shared
/// rows, so a release of them is no release of the round's own block.
const EB_MARK_TURN: &str = "t-eb-2";

const PROBE_TURN: &str = "t-probe";
const PROBE_SAYS: &str = "cw943 probe: what is still open?";

/// The words every summary is answered with where a test does not care.
const SUMMARY_WORDS: &str = "cw943 summary: what was said.";

/// A report on the tap that crosses `compress_at` of the small window.
const OVER_THE_LINE: i64 = 600;

// ═══════════════════════════════════════════════════════════════ the hives

/// A talky with a small window, so one report crosses `compress_at` (the
/// pattern of gh932 `small_talky`), with `extra` overrides.
fn small_talky(extra: &[(&str, &str, Value)]) -> Hive {
    let mut over = vec![
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
        ("policy", "context_window", json!(1000)),
    ];
    over.extend(extra.iter().cloned());
    Hive::with(&over)
}

/// The window of 1.0.0 (no role) keeping one round raw: a rebuild condenses
/// every older round of its round and so asks the summarizer (the pattern of
/// `curator_policy.rs`, `a_rebuild_with_a_round_summarises_what_the_round_may_see`).
fn summarising() -> Hive {
    Hive::with(&[("policy", "keep_recent", json!(1))])
}

/// A tap that stamps a cache expiry far ahead: the clock is ordered for the
/// cold cache, and the test strikes it by hand.
fn cold() -> Value {
    json!({"cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// One round on `in_curate` spoken in `round` (a JSON text); returns the call
/// that left for the model.
fn curate_in(h: &mut Hive, round: &str, turn: &str, text: &str) -> Msg {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": SESSION, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": round}),
        json!({"session_id": SESSION, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(text)], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call in {round}: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// A whole turn in `round`: the words in, the model's final answer back on
/// the tap with `extra` usage. Returns the call.
fn turn_in(h: &mut Hive, round: &str, turn: &str, says: &str, reply: &str, extra: Value) -> Msg {
    let call = curate_in(h, round, turn, says);
    h.out.clear();
    h.tap(&call, "stop", extra, json!([said(reply)]));
    call
}

/// A rebuild in `round`, forced: a turn whose report crosses `compress_at`,
/// the strike of the round's own clock order, every summary answered alike.
/// Returns the call that crossed the line.
fn rebuild_in(h: &mut Hive, round: &str, turn: &str, says: &str, reply: &str) -> Msg {
    let call = turn_in(
        h,
        round,
        turn,
        says,
        reply,
        json!({"tokens_prompt": OVER_THE_LINE}),
    );
    let order = h
        .clock_order_for(round)
        .unwrap_or_else(|| panic!("the call in {round} ordered no strike: {:?}", h.clock));
    assert_eq!(
        order.body["emit_body"]["reason"], "compress",
        "the report in {round} crossed compress_at: {order:?}"
    );
    assert_eq!(
        order.body["emit_body"]["curator_call"], call.hop["curator_call"],
        "the order of {round} is for its own call: {order:?}"
    );
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer(SUMMARY_WORDS, "stop");
    }
    let plan = h.plan_in(round);
    assert_ne!(
        plan,
        json!({}),
        "the rebuild in {round} wrote no plan of its round: {:?}",
        h.stderr
    );
    assert_eq!(
        plan["round"].as_str(),
        round_canon(round).as_deref(),
        "the plan of {round} names its round: {plan}"
    );
    call
}

/// The number of `state` rows under `key`.
fn state_rows(h: &Hive, key: &str) -> i64 {
    h.rows(&format!("SELECT COUNT(*) FROM state WHERE key = '{key}'"))[0][0]
        .as_i64()
        .unwrap_or(0)
}

/// The registry of the plans, `{rk: last_used_ms}`; empty without one.
fn registry(h: &Hive) -> Map<String, Value> {
    sj::from_str(&h.state("window_plans")).unwrap_or_default()
}

/// The actions the record of `call` carries (`calls.actions`).
fn actions_of(h: &Hive, call: &Msg) -> Vec<String> {
    let id = call.hop["curator_call"].as_str().expect("a call id");
    let rows = h.rows(&format!("SELECT actions FROM calls WHERE call_id = '{id}'"));
    let text = rows
        .first()
        .and_then(|r| r[0].as_str())
        .unwrap_or_else(|| panic!("no record of call {id}"));
    let list: Value = sj::from_str(text).unwrap_or_else(|e| panic!("actions of {id}: {e}"));
    list.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| x.as_str().unwrap_or("").to_string())
        .collect()
}

/// What the model reads of a call: its messages and its system part. The
/// call's own id is random per run and is left out.
fn window(call: &Msg) -> (Vec<Value>, Option<Value>) {
    (call.messages(), call.body.get("system").cloned())
}

fn all_text(v: &Value) -> String {
    sj::to_string(v).unwrap_or_default()
}

/// The block hashes the policy fetched in `ops` (its `blocks` reads by
/// `hash in [...]`), as the store saw them.
fn fetched_blocks(ops: &[(String, Value)]) -> Vec<String> {
    ops.iter()
        .filter(|(from, op)| from == "policy" && op["table"] == "blocks")
        .filter_map(|(_, op)| op["where"]["hash"]["in"].as_array().cloned())
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

// ════════════════════════════════════════════════════════════════ 1. the key

/// The key of a round is the key of its canonical form: the same round in
/// another order, with duplicates, with whitespace, as text or as a list has
/// one key; `[]` and a round nobody declared (`null`, `""`) share the key of
/// `[]`; `["*"]` and another round have their own; every key is 12 hex
/// digits, the first of the sha256 of the canonical round, and the harness
/// computes the same. The bound's defaults are 64 plans and 30 days, and its
/// pure half (`prune_plans`) drops the least recently used past the bound,
/// what was unused for longer than `idle_s`, and never the round it runs
/// for.
#[test]
fn gh943_round_key_is_canonical() {
    if !shipped() {
        return;
    }
    let rounds = json!([
        ROUND_EA,
        CANON_EA,
        r#"["member:a", "member:e", "member:a"]"#,
        r#"  [ "member:e" ,"member:a" ]  "#,
        ["member:e", "member:a", "member:e"],
        "[]",
        [],
        null,
        "",
        r#"["*"]"#,
        ROUND_EB,
    ]);
    let (got, err) = policy_scope(json!({}), "[round_key(r) for r in ARGS]", rounds);
    let keys: Vec<String> = got
        .as_array()
        .unwrap_or_else(|| panic!("round_key answered no list: {got} {err}"))
        .iter()
        .map(|k| k.as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(keys.len(), 11, "one key per round: {keys:?}");
    for k in &keys {
        assert!(
            k.len() == 12
                && k.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "a round key is 12 lower-case hex digits: {k:?}"
        );
    }
    for (i, k) in keys.iter().enumerate().take(5).skip(1) {
        assert_eq!(
            k, &keys[0],
            "the same round in another order, with duplicates or whitespace (#{i}) has \
             another key"
        );
    }
    for (i, k) in keys.iter().enumerate().take(9).skip(6) {
        assert_eq!(
            k, &keys[5],
            "an empty or undeclared round (#{i}) does not have the key of `[]`"
        );
    }
    assert_ne!(keys[5], keys[0], "`[]` shares a key with {{e,a}}");
    assert_ne!(keys[9], keys[5], "`[\"*\"]` shares a key with `[]`");
    assert_ne!(keys[9], keys[0], "`[\"*\"]` shares a key with {{e,a}}");
    assert_ne!(keys[10], keys[0], "{{e,b}} shares a key with {{e,a}}");
    assert_eq!(
        keys[0],
        &sha256_hex(CANON_EA)[..12],
        "the key is not the first 12 hex digits of the sha256 of the canonical round"
    );
    for (i, text) in [
        (0, ROUND_EA),
        (5, "[]"),
        (8, ""),
        (9, r#"["*"]"#),
        (10, ROUND_EB),
    ] {
        assert_eq!(
            keys[i],
            round_key(text),
            "the policy and the harness disagree on the key of {text:?}"
        );
    }

    // The bound's defaults: the shipped params leave both unset.
    let (got, err) = policy_scope(
        json!({}),
        "[MAX_ROUND_PLANS, ROUND_PLAN_IDLE_S]",
        Value::Null,
    );
    assert_eq!(
        (got[0].as_f64(), got[1].as_f64()),
        (Some(64.0), Some(2_592_000.0)),
        "the defaults of max_round_plans and round_plan_idle_s moved: {got} {err}"
    );
    let (got, err) = policy_scope(
        json!({"max_round_plans": "5", "round_plan_idle_s": ""}),
        "[MAX_ROUND_PLANS, ROUND_PLAN_IDLE_S]",
        Value::Null,
    );
    assert_eq!(
        (got[0].as_f64(), got[1].as_f64()),
        (Some(5.0), Some(2_592_000.0)),
        "a numeric string is a number and empty is not set: {got} {err}"
    );
    let (got, err) = policy_scope(
        json!({"max_round_plans": 0, "round_plan_idle_s": -1}),
        "[MAX_ROUND_PLANS, ROUND_PLAN_IDLE_S]",
        Value::Null,
    );
    assert_eq!(
        (got[0].as_f64(), got[1].as_f64()),
        (Some(64.0), Some(2_592_000.0)),
        "a bound below 1 is no bound: the defaults stand: {got} {err}"
    );

    // `prune_plans(reg, rk, now_ms, max_plans, idle_s)` -> (reg, dropped).
    let day = 86_400;
    let cases = [
        (
            "the least recently used plan falls at max + 1",
            json!({"aa": 1000, "bb": 2000, "cc": 3000}),
            "dd",
            4000_i64,
            3,
            day,
            json!({"bb": 2000, "cc": 3000, "dd": 4000}),
            json!(["aa"]),
        ),
        (
            "at max nothing falls",
            json!({"aa": 1000, "bb": 2000}),
            "cc",
            3000,
            3,
            day,
            json!({"aa": 1000, "bb": 2000, "cc": 3000}),
            json!([]),
        ),
        (
            "the round it runs for is stamped now",
            json!({"aa": 1000, "bb": 2000}),
            "aa",
            3000,
            3,
            day,
            json!({"aa": 3000, "bb": 2000}),
            json!([]),
        ),
        (
            "a plan unused for longer than idle_s falls",
            json!({"aa": 1000, "bb": 3_000_000}),
            "cc",
            3_500_000,
            64,
            1000,
            json!({"bb": 3_000_000, "cc": 3_500_000}),
            json!(["aa"]),
        ),
        (
            "unused for exactly idle_s is not over it",
            json!({"aa": 2_500_000}),
            "cc",
            3_500_000,
            64,
            1000,
            json!({"aa": 2_500_000, "cc": 3_500_000}),
            json!([]),
        ),
        (
            "several fall, and are named sorted",
            json!({"zz": 1, "yy": 2, "xx": 3_000_000}),
            "ww",
            3_000_001,
            64,
            1000,
            json!({"xx": 3_000_000, "ww": 3_000_001}),
            json!(["yy", "zz"]),
        ),
        (
            "of two equally old plans the smaller key falls",
            json!({"bb": 1000, "aa": 1000}),
            "cc",
            2000,
            2,
            day,
            json!({"bb": 1000, "cc": 2000}),
            json!(["aa"]),
        ),
        (
            "the round it runs for never falls, not even as the oldest",
            json!({"aa": 5000, "bb": 6000}),
            "aa",
            1000,
            1,
            day,
            json!({"aa": 1000}),
            json!(["bb"]),
        ),
        (
            "the round it runs for never falls as idle",
            json!({"aa": 1}),
            "aa",
            10_000_000_000,
            1,
            1,
            json!({"aa": 10_000_000_000_i64}),
            json!([]),
        ),
    ];
    let args: Vec<Value> = cases
        .iter()
        .map(|(_, reg, rk, now, max, idle, _, _)| json!([reg, rk, now, max, idle]))
        .collect();
    let (got, err) = policy_scope(
        json!({}),
        "[list(prune_plans(*c)) for c in ARGS]",
        json!(args),
    );
    for (i, (name, _, _, _, _, _, reg, dropped)) in cases.iter().enumerate() {
        assert_eq!(
            (&got[i][0], &got[i][1]),
            (reg, dropped),
            "prune_plans: {name} -- {err}"
        );
    }
}

// ═══════════════════════════════════════════════ 2. never another's window

/// The rows both rounds see, in the order they are sown.
fn shared_rows() -> [(&'static str, Value, i64); 2] {
    [
        ("user", user(SHARED_SAYS), 0),
        ("assistant", said(SHARED_REPLY), 1),
    ]
}

/// The block ids of the shared rows: the one {e,b} releases, the one it pins.
fn shared_ids() -> (String, String) {
    let [(_, x, _), (_, y, _)] = shared_rows();
    (short_id(&x), short_id(&y))
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

/// Track A (`hidden` false): the three turns of {e,a}, the shared rows after
/// the first, a rebuild of {e,a}. Track B (`hidden` true): the same, a turn
/// of {e,b} after each turn of {e,a}, and after the rebuild of {e,a} a forced
/// rebuild of {e,b} -- the rebuild that, with one plan per hive, set the
/// cover of the next window of {e,a} past the first turn of {e,a}.
fn rebuild_track(hidden: bool) -> Hive {
    let mut h = small_talky(&[]);
    for (i, (turn, says, reply)) in EA_TURNS.iter().enumerate() {
        turn_in(&mut h, ROUND_EA, turn, says, reply, json!({}));
        if hidden {
            let (turn, says, reply) = EB_TURNS[i];
            turn_in(&mut h, ROUND_EB, turn, says, reply, json!({}));
        }
        if i == 0 {
            sow_shared(&mut h);
        }
    }
    rebuild_in(
        &mut h,
        ROUND_EA,
        "t-ea-cross",
        "cw943 ea four: please sum up what we settled.",
        "cw943 ea four reply: the parcel, the lease and the market.",
    );
    if hidden {
        let before = h.state_in("window_plan", ROUND_EA);
        rebuild_in(
            &mut h,
            ROUND_EB,
            "t-eb-cross",
            "cw943 eb four: please sum up what we settled.",
            "cw943 eb four reply: the key, the boat and the shed.",
        );
        let ea = h.plan_in(ROUND_EA);
        let eb = h.plan_in(ROUND_EB);
        assert!(
            eb["cover"].as_i64().unwrap_or(0) > ea["cover"].as_i64().unwrap_or(0),
            "the rebuild of {{e,b}} cuts later than the plan of {{e,a}} -- the cut that \
             one plan per hive put on {{e,a}}: {eb} / {ea}"
        );
        assert_eq!(
            h.state_in("window_plan", ROUND_EA),
            before,
            "the rebuild of {{e,b}} rewrote the plan of {{e,a}}"
        );
    }
    h
}

/// The shape of the plan of {e,a} in terms of the rows {e,a} sees (gh932
/// `plan_shape`): `cover` and `keep` are `seq`s -- microseconds of the run
/// that wrote the row -- so each is named by the rows it stands for; the
/// block ids and tool names of `released`, `shrunk` and `stubs`, the
/// summary's presence and the plan's round are taken as they are.
fn plan_shape(h: &Hive) -> Value {
    let plan = h.plan_in(ROUND_EA);
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
           "shrunk": plan["shrunk"], "stubs": plan["stubs"],
           "summary": plan["summary"].as_str().is_some_and(|s| !s.is_empty()),
           "round": plan["round"]})
}

/// A `turn_write` without the episode's wall-clock stamp (`happened_at`):
/// the turn id (`<session>#<tag>-<n>`), the turn index and the words.
fn episode(m: &Msg) -> (Value, Vec<Value>) {
    let mut hop = m.hop.clone();
    hop.remove("happened_at");
    (Value::Object(hop), m.messages())
}

/// A rebuild of one round never cuts the window of another. Track B holds,
/// beside the turns of {e,a} that track A holds, turns of {e,b} and -- after
/// the rebuild of {e,a} and before the next call of {e,a} -- a forced
/// rebuild of {e,b}, which cuts {e,b}'s window later than {e,a}'s. The next
/// call of {e,a} is then the same in both: the plan of {e,a} (cover, kept
/// rows, releases, shrunk results, stubs), the turn index and the whole
/// window that leaves for the model, messages and system part, byte for
/// byte. With one plan per hive, the rebuild of {e,b} set the cover of that
/// window past the first turn of {e,a}.
///
/// And the marks of {e,b} survive a rebuild of {e,a}: a release and a pin
/// {e,b} said on blocks both rounds see leave the plan of {e,b} as it was
/// through a rebuild of {e,a}, which does not apply them, and the next
/// rebuild of {e,b} applies them.
#[test]
fn gh943_a_rebuild_never_cuts_another_rounds_window() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut a = rebuild_track(false);
    let mut b = rebuild_track(true);
    assert_eq!(
        plan_shape(&b),
        plan_shape(&a),
        "the rows or the rebuild of {{e,b}} moved the plan of {{e,a}}"
    );
    let wa = curate_in(&mut a, ROUND_EA, PROBE_TURN, PROBE_SAYS);
    let ta: Vec<_> = a.routed("turn_write").iter().map(episode).collect();
    let wb = curate_in(&mut b, ROUND_EA, PROBE_TURN, PROBE_SAYS);
    let tb: Vec<_> = b.routed("turn_write").iter().map(episode).collect();
    let seen = texts(&wa).join("\n");
    let (_, first_says, first_reply) = EA_TURNS[0];
    assert!(
        seen.contains(first_says) && seen.contains(first_reply),
        "track A's window holds the first turn of {{e,a}}: {seen}"
    );
    assert!(
        !ta.is_empty(),
        "the probe turn wrote its episode in track A"
    );
    assert_eq!(
        tb, ta,
        "the turns or the rebuild of {{e,b}} moved the turn index of {{e,a}}"
    );
    assert_eq!(
        window(&wb),
        window(&wa),
        "the rebuild of {{e,b}} cut the window of {{e,a}}"
    );
    assert!(
        !all_text(&Value::Object(wb.body.clone())).contains(EB_MARK),
        "a word of {{e,b}} in the window of {{e,a}}"
    );

    // The marks of {e,b} through a rebuild of {e,a}.
    let (x, y) = shared_ids();
    eb_mark(&mut b, "release", &x);
    eb_mark(&mut b, "pin", &y);
    let eb_before = b.state_in("window_plan", ROUND_EB);
    assert!(
        !eb_before.is_empty(),
        "{{e,b}} holds a plan of its own before the marks"
    );
    rebuild_in(
        &mut b,
        ROUND_EA,
        "t-ea-again",
        "cw943 ea five: anything new?",
        "cw943 ea five reply: nothing new.",
    );
    assert_eq!(
        b.state_in("window_plan", ROUND_EB),
        eb_before,
        "the rebuild of {{e,a}} rewrote the plan of {{e,b}}"
    );
    let ea = b.plan_in(ROUND_EA);
    assert!(
        !ea["released"]
            .as_array()
            .is_some_and(|r| r.iter().any(|i| i.as_str() == Some(x.as_str()))),
        "{{e,a}} applied a release {{e,b}} said: {ea}"
    );
    assert!(
        ea["marks_on"][x.as_str()][CANON_EB].is_null(),
        "the rebuild of {{e,a}} read the release of {{e,b}}: {ea}"
    );
    rebuild_in(
        &mut b,
        ROUND_EB,
        "t-eb-again",
        "cw943 eb five: anything new?",
        "cw943 eb five reply: nothing new.",
    );
    let eb = b.plan_in(ROUND_EB);
    assert!(
        eb["released"]
            .as_array()
            .is_some_and(|r| r.iter().any(|i| i.as_str() == Some(x.as_str()))),
        "the next rebuild of {{e,b}} did not apply its release of #{x}: {eb}"
    );
    assert_eq!(
        eb["marks_on"][x.as_str()][CANON_EB][3],
        "release",
        "the plan of {{e,b}} does not keep its release of #{x}: {eb}"
    );
    let pinned = b.rows(&format!(
        "SELECT COUNT(*) FROM pins WHERE audience_set = '{CANON_EB}'"
    ))[0][0]
        .as_i64()
        .unwrap_or(0);
    assert_eq!(
        pinned, 1,
        "the next rebuild of {{e,b}} did not apply its pin of #{y}"
    );
}

// ═══════════════════════════════════════════════════ 3. side by side

/// Two rebuilds of two rounds overlap, and neither is lost or merged into the
/// other. A call and a tap of {e,b} between the call and tap of {e,a} and the
/// strike of {e,a}'s order do not make {e,a}'s strike stale (one order and
/// one last call per round). The first rebuild of a round takes its claim
/// although its claim row did not exist before. Both rebuilds wait for their
/// summaries at once -- two claims, two requests; a second strike of {e,a}
/// while its rebuild runs merges into it and asks nothing. Answered, each
/// writes its own plan and frees its own claim, and each round's report --
/// `rebuild:<why>` and its summary -- reaches only that round's next call.
#[test]
fn gh943_two_rounds_rebuild_side_by_side() {
    if !shipped() {
        return;
    }
    let mut h = summarising();
    turn_in(
        &mut h,
        ROUND_EA,
        "t-a-1",
        "cw943 a one: the parcel left the depot.",
        "cw943 a one reply: noted.",
        json!({}),
    );
    turn_in(
        &mut h,
        ROUND_EB,
        "t-b-1",
        "cw943 b one: the key lies under the stone.",
        "cw943 b one reply: noted.",
        json!({}),
    );
    let call_a = turn_in(
        &mut h,
        ROUND_EA,
        "t-a-2",
        "cw943 a two: the lease is signed.",
        "cw943 a two reply: noted.",
        cold(),
    );
    // B's call and tap between A's call and tap and A's rebuild.
    let call_b = turn_in(
        &mut h,
        ROUND_EB,
        "t-b-2",
        "cw943 b two: the boat waits at the pier.",
        "cw943 b two reply: noted.",
        cold(),
    );
    let order_a = h
        .clock_order_for(ROUND_EA)
        .unwrap_or_else(|| panic!("{{e,a}} ordered no strike: {:?}", h.clock));
    let order_b = h
        .clock_order_for(ROUND_EB)
        .unwrap_or_else(|| panic!("{{e,b}} ordered no strike: {:?}", h.clock));
    assert_ne!(
        order_a.body["schedule_id"], order_b.body["schedule_id"],
        "the order of {{e,b}} took the place of the order of {{e,a}}"
    );
    assert_eq!(
        order_a.body["emit_body"]["curator_call"], call_a.hop["curator_call"],
        "the order of {{e,a}} is not for the call of {{e,a}}: {order_a:?}"
    );
    assert_eq!(
        h.state_in("last_call", ROUND_EA),
        call_a.hop["curator_call"].as_str().unwrap_or("?"),
        "the call of {{e,b}} took the place of the last call of {{e,a}}"
    );
    let claim_a = format!("rebuild_running:{}", round_key(ROUND_EA));
    let claim_b = format!("rebuild_running:{}", round_key(ROUND_EB));
    assert_eq!(
        state_rows(&h, &claim_a),
        0,
        "{{e,a}} has a claim row before its first rebuild"
    );

    h.fire(&order_a);
    assert_eq!(
        state_rows(&h, &claim_a),
        1,
        "the first rebuild of {{e,a}} did not create its claim row: {:?}",
        h.stderr
    );
    assert!(
        !h.state_in("rebuild_running", ROUND_EA).is_empty(),
        "the rebuild of {{e,a}} was discarded or took no claim -- the call of {{e,b}} \
         between made it stale? {:?}",
        h.stderr
    );
    assert_eq!(
        h.summ.len(),
        1,
        "the rebuild of {{e,a}} asks for its summary: {:?}",
        h.stderr
    );

    // A second trigger of the same round while its rebuild runs.
    h.fire(&order_a);
    assert_eq!(
        h.summ.len(),
        1,
        "a second trigger of {{e,a}} during its rebuild asked again instead of merging"
    );

    h.fire(&order_b);
    assert_eq!(
        h.summ.len(),
        2,
        "the rebuild of {{e,b}} waited for the one of {{e,a}} instead of running beside it: \
         {:?}",
        h.stderr
    );
    assert_eq!(
        state_rows(&h, &claim_b),
        1,
        "the first rebuild of {{e,b}} did not create its claim row"
    );
    for round in [ROUND_EA, ROUND_EB] {
        assert!(
            !h.state_in("rebuild_running", round).is_empty(),
            "the claim of {round} is not held while its summary is out"
        );
    }
    let asked: Vec<Option<String>> = h
        .summ
        .iter()
        .map(|m| {
            m.context
                .get("audience_set")
                .and_then(Value::as_str)
                .and_then(round_canon)
        })
        .collect();
    assert_eq!(
        asked,
        [Some(CANON_EA.to_string()), Some(CANON_EB.to_string())],
        "the two requests are not one of each round, in order"
    );

    let sum_a = "cw943 summary of a: the parcel left the depot.";
    let sum_b = "cw943 summary of b: the key lies under the stone.";
    h.answer(sum_a, "stop");
    let plan_a = h.plan_in(ROUND_EA);
    assert!(
        plan_a["summary"].as_str().is_some_and(|s| !s.is_empty()),
        "the answer of {{e,a}} wrote no plan with its summary: {plan_a} {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_EA),
        "",
        "the claim of {{e,a}} is not freed"
    );
    assert!(
        !h.state_in("rebuild_running", ROUND_EB).is_empty(),
        "the answer of {{e,a}} freed the claim of {{e,b}}"
    );
    assert_eq!(
        h.plan_in(ROUND_EB),
        json!({}),
        "the answer of {{e,a}} wrote a plan of {{e,b}}"
    );
    h.answer(sum_b, "stop");
    let plan_b = h.plan_in(ROUND_EB);
    assert_eq!(
        h.state_in("rebuild_running", ROUND_EB),
        "",
        "the claim of {{e,b}} is not freed"
    );
    let sid = |canon: &str| -> String {
        let r = h.rows(&format!(
            "SELECT id FROM summaries WHERE audience_set = '{canon}'"
        ));
        assert_eq!(r.len(), 1, "one summary of {canon}: {r:?} {:?}", h.stderr);
        r[0][0].as_str().unwrap_or("").to_string()
    };
    let (sid_a, sid_b) = (sid(CANON_EA), sid(CANON_EB));
    assert_eq!(
        plan_a["summary"], sid_a,
        "the plan of {{e,a}} does not name the summary of {{e,a}}: {plan_a}"
    );
    assert_eq!(
        plan_b["summary"], sid_b,
        "the plan of {{e,b}} does not name the summary of {{e,b}}: {plan_b}"
    );
    assert_eq!(
        h.plan_in(ROUND_EA),
        plan_a,
        "the answer of {{e,b}} rewrote the plan of {{e,a}}"
    );

    // The block of each round's summary leaf, by its slot (OR-BC.W.2).
    let leaf_hash = |round: &str| -> String {
        let r = h.rows(&format!(
            "SELECT hash FROM slots WHERE path = '{}'",
            summary_slot(round)
        ));
        assert_eq!(r.len(), 1, "one summary slot of {round}: {r:?}");
        r[0][0].as_str().unwrap_or("").to_string()
    };
    let (leaf_a, leaf_b) = (leaf_hash(ROUND_EA), leaf_hash(ROUND_EB));

    // Each report reaches only its own round's next call -- B's first.
    h.ledger_ops.clear();
    let next_b = curate_in(&mut h, ROUND_EB, "t-b-3", "cw943 b three: and now?");
    let fetched_b = fetched_blocks(&std::mem::take(&mut h.ledger_ops));
    let next_a = curate_in(&mut h, ROUND_EA, "t-a-3", "cw943 a three: and now?");
    let fetched_a = fetched_blocks(&std::mem::take(&mut h.ledger_ops));
    for (name, next, own, other, own_words, other_words, fetched, own_leaf, other_leaf) in [
        (
            "{e,b}", &next_b, &sid_b, &sid_a, sum_b, sum_a, &fetched_b, &leaf_b, &leaf_a,
        ),
        (
            "{e,a}", &next_a, &sid_a, &sid_b, sum_a, sum_b, &fetched_a, &leaf_a, &leaf_b,
        ),
    ] {
        // Review I-1: the call fetches the block of its own summary leaf and
        // never the other round's -- what a call reads says nothing of
        // another round's summaries (GH #932), not only what it shows.
        assert!(
            fetched.contains(own_leaf),
            "the call of {name} did not fetch its own summary leaf: {fetched:?}"
        );
        assert!(
            !fetched.contains(other_leaf),
            "the call of {name} fetched the summary leaf of the other round: {fetched:?}"
        );
        let acts = actions_of(&h, next);
        assert_eq!(
            acts.iter().filter(|x| x.starts_with("rebuild:")).count(),
            1,
            "the next call of {name} does not carry exactly its own rebuild: {acts:?}"
        );
        assert!(
            acts.contains(&format!("summary:{own}")),
            "the next call of {name} misses its own summary: {acts:?}"
        );
        assert!(
            !acts.contains(&format!("summary:{other}")),
            "the next call of {name} carries the other round's report: {acts:?}"
        );
        let leaf = next.body["system"]["history"]["summary"]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            leaf.ends_with(own_words),
            "the window of {name} does not show its own summary: {:?}",
            next.body.get("system")
        );
        assert!(
            !all_text(&Value::Object(next.body.clone())).contains(other_words),
            "the window of {name} shows the other round's summary"
        );
    }
    assert!(
        !actions_of(&h, &call_b)
            .iter()
            .any(|x| x.starts_with("rebuild:")),
        "a call before any rebuild carries a rebuild report"
    );
}

/// The handover leaf of a session of {e,b} (OR-BC.W.3, review I-2): a
/// rebuild of {e,a} leaves it standing, the first rebuild of {e,b} takes it
/// down. Until OR-BC.W.3 the first rebuild of ANY round took the one leaf
/// down -- {e,b} lost its handover on the account of {e,a}, the channel
/// GH #943 closes. Red without the claim's `unleaf` (every rebuild deletes).
#[test]
fn gh943_a_handover_leaf_falls_only_with_its_round() {
    if !shipped() {
        return;
    }
    let mut h = small_talky(&[]);
    let (t, says, reply) = EA_TURNS[0];
    turn_in(&mut h, ROUND_EA, t, says, reply, json!({}));
    let (t, says, reply) = EB_TURNS[0];
    turn_in(&mut h, ROUND_EB, t, says, reply, json!({}));
    // The leaf `./handover` writes for a new session of {e,b}, bound to it.
    let leaf = json!({"path": "history.handover", "text": "cw943 eb handover: the key."});
    let hash = sha256_hex(&canonical(&leaf));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', 27, ?2, 'x')",
        rusqlite::params![hash, canonical(&leaf)],
    )
    .expect("the leaf's block");
    h.db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES ('history.handover', ?1, 'curator', 'x')",
        [&hash],
    )
    .expect("the leaf's slot");
    let bound = canonical(&json!({"audience": CANON_EB, "hash": hash}));
    h.db.execute("DELETE FROM state WHERE key = 'handover_audience'", [])
        .expect("no binding");
    h.db.execute(
        "INSERT INTO state (key, value) VALUES ('handover_audience', ?1)",
        [&bound],
    )
    .expect("the binding");
    let leaf_rows = |h: &Hive| -> Vec<Vec<Value>> {
        h.rows("SELECT hash FROM slots WHERE path = 'history.handover'")
    };

    rebuild_in(
        &mut h,
        ROUND_EA,
        "t-ea-cross",
        "cw943 ea four: please sum up what we settled.",
        "cw943 ea four reply: the parcel.",
    );
    assert_eq!(
        leaf_rows(&h),
        vec![vec![json!(hash)]],
        "the rebuild of {{e,a}} took down the handover leaf of {{e,b}}: {:?}",
        h.stderr
    );
    rebuild_in(
        &mut h,
        ROUND_EB,
        "t-eb-cross",
        "cw943 eb four: please sum up what we settled.",
        "cw943 eb four reply: the key.",
    );
    assert!(
        leaf_rows(&h).is_empty(),
        "the first rebuild of {{e,b}} left its own handover leaf standing: {:?}",
        h.stderr
    );
}

/// `./intake` reads the plan under the key `./policy` writes it under
/// (OR-BC.W.1, review M-2): the same rounds, the same keys.
#[test]
fn gh943_intake_keys_a_round_as_the_policy_does() {
    if !shipped() {
        return;
    }
    let rounds = json!([ROUND_EA, CANON_EA, "[]", [], null, "", r#"["*"]"#, ROUND_EB]);
    let (policy, _) = policy_scope(json!({}), "[round_key(r) for r in ARGS]", rounds.clone());
    // The defs of intake alone (a def whose default names a run-time value
    // does not load and is not needed).
    const LOAD: &str = "import ast, json, sys\ninp = json.load(sys.stdin)\nsc = {}\n\
        for n in ast.parse(inp['src']).body:\n    if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef)):\n        \
        try:\n            exec(compile(ast.Module(body=[n], type_ignores=[]), 'intake', 'exec'), sc)\n        \
        except NameError:\n            pass\nprint(json.dumps([sc['round_key'](r) for r in inp['args']]))\n";
    let out = run_python(
        LOAD,
        &json!({"src": script_of("intake"), "args": rounds}).to_string(),
    );
    let intake: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "intake round_key ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(intake, policy, "intake keys a round other than the policy");
}

// ════════════════════════════════════════════════════════ 4. the bound

/// Four rounds of one session, canonical.
const ROUNDS: [&str; 4] = [
    r#"["member:a","member:e"]"#,
    r#"["member:b","member:e"]"#,
    r#"["member:c","member:e"]"#,
    r#"["member:d","member:e"]"#,
];

/// The turn that rebuilds round `i`: the same words in every track.
fn round_turn(i: usize) -> (String, String, String) {
    (
        format!("t-r{i}"),
        format!("cw943 round {i}: the question of this round."),
        format!("cw943 round {i} reply: the answer of this round."),
    )
}

/// A ledger bounded to three plans (instead of 65 rounds at the default 64:
/// the default is test 1's), each round rebuilt once.
fn bounded() -> Hive {
    small_talky(&[("policy", "max_round_plans", json!(3))])
}

/// The plans are bounded. With `max_round_plans` 3, four rounds rebuilt one
/// after the other leave the plans of the last three: the plan of the first
/// is gone (and with it its claim, its last call, its order, its actions and
/// its pending summary), and the registry holds three rounds. A round whose
/// registry entry looks older than `round_plan_idle_s` (the row rewritten
/// by hand -- no waiting) loses its plan at the next rebuild of another
/// round. And the round that lost its plan builds its next window from its
/// own rows: byte for byte the window of a ledger that never held another
/// round and lost the plan the same way (its `window_plan` row deleted).
#[test]
fn gh943_round_plans_are_bounded() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = bounded();
    for (i, round) in ROUNDS.iter().enumerate() {
        let (turn, says, reply) = round_turn(i);
        rebuild_in(&mut h, round, &turn, &says, &reply);
    }
    let rk: Vec<String> = ROUNDS.iter().map(|r| round_key(r)).collect();
    assert_eq!(
        h.plan_in(ROUNDS[0]),
        json!({}),
        "the least recently rebuilt round kept its plan past max_round_plans"
    );
    for round in &ROUNDS[1..] {
        assert_ne!(
            h.plan_in(round),
            json!({}),
            "a round within max_round_plans lost its plan: {round}"
        );
    }
    let mut held: Vec<String> = registry(&h).keys().cloned().collect();
    held.sort();
    let mut want = rk[1..].to_vec();
    want.sort();
    assert_eq!(
        held, want,
        "the registry does not hold exactly the three newest rounds"
    );
    for key in [
        "window_plan",
        "rebuild_running",
        "last_call",
        "armed_call",
        "actions_pending",
        "pending:summary",
    ] {
        assert_eq!(
            state_rows(&h, &format!("{key}:{}", rk[0])),
            0,
            "the dropped round kept its `{key}` row"
        );
    }

    // An idle round: its registry entry rewritten to look 30 days and more
    // unused, then another round rebuilds.
    let mut reg = registry(&h);
    reg.insert(rk[2].clone(), json!(1));
    let changed =
        h.db.execute(
            "UPDATE state SET value = ?1 WHERE key = 'window_plans'",
            rusqlite::params![sj::to_string(&reg).expect("a registry")],
        )
        .expect("the registry rewritten");
    assert_eq!(changed, 1, "the registry row to rewrite exists");
    rebuild_in(
        &mut h,
        ROUNDS[1],
        "t-r1-again",
        "cw943 round 1 again: anything new?",
        "cw943 round 1 again reply: nothing new.",
    );
    assert_eq!(
        h.plan_in(ROUNDS[2]),
        json!({}),
        "a round unused for longer than round_plan_idle_s kept its plan"
    );
    for i in [1, 3] {
        assert_ne!(
            h.plan_in(ROUNDS[i]),
            json!({}),
            "a round in use lost its plan with the idle one: {}",
            ROUNDS[i]
        );
    }
    let mut held: Vec<String> = registry(&h).keys().cloned().collect();
    held.sort();
    let mut want = vec![rk[1].clone(), rk[3].clone()];
    want.sort();
    assert_eq!(held, want, "the registry kept the idle round");

    // The round that lost its plan, beside a ledger that only ever held it.
    let mut solo = bounded();
    let (turn, says, reply) = round_turn(0);
    rebuild_in(&mut solo, ROUNDS[0], &turn, &says, &reply);
    let gone = solo
        .db
        .execute(
            "DELETE FROM state WHERE key = ?1",
            rusqlite::params![format!("window_plan:{}", rk[0])],
        )
        .expect("the plan deleted");
    assert_eq!(gone, 1, "the lone round had a plan to delete");
    assert_eq!(solo.plan_in(ROUNDS[0]), json!({}), "the lone plan is gone");
    let fallen = curate_in(&mut h, ROUNDS[0], PROBE_TURN, PROBE_SAYS);
    let alone = curate_in(&mut solo, ROUNDS[0], PROBE_TURN, PROBE_SAYS);
    let seen = texts(&fallen).join("\n");
    assert!(
        seen.contains(&says) && seen.contains(&reply),
        "the round that lost its plan does not see its own rows: {seen}"
    );
    assert_eq!(
        window(&fallen),
        window(&alone),
        "the window of the round that lost its plan hangs on the other rounds"
    );
    for i in 1..ROUNDS.len() {
        let (_, other, _) = round_turn(i);
        assert!(
            !seen.contains(&other),
            "the window of round 0 shows a word of round {i}: {seen}"
        );
    }
}

// ═══════════════════════════════════════════ 5. what a round owns (GH #949)

/// A ledger bounded to three plans that summarises: the window of 1.0.0 (no
/// role) keeping one round raw, so a rebuild of a round with two turns
/// condenses the older one.
fn bounded_summarising() -> Hive {
    Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "max_round_plans", json!(3)),
    ])
}

/// A turn of `round` with a cold cache, the strike of the round's own order
/// and every summary answered alike: a rebuild through the clock (the
/// pattern of `gh943_two_rounds_rebuild_side_by_side`). Returns the call.
fn struck_in(h: &mut Hive, round: &str, turn: &str, says: &str, reply: &str) -> Msg {
    let call = turn_in(h, round, turn, says, reply, cold());
    let order = h
        .clock_order_for(round)
        .unwrap_or_else(|| panic!("the call in {round} ordered no strike: {:?}", h.clock));
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer(SUMMARY_WORDS, "stop");
    }
    assert_ne!(
        h.plan_in(round),
        json!({}),
        "the strike in {round} rebuilt nothing: {:?}",
        h.stderr
    );
    call
}

/// The `state` rows of the round key `rk`, `[key, value]`, sorted.
fn rows_of_round(h: &Hive, rk: &str) -> Vec<Vec<Value>> {
    h.rows(&format!(
        "SELECT key, value FROM state WHERE key LIKE '%:{rk}' ORDER BY key"
    ))
}

/// Everything a dropped round owns falls with its plan (review W M-1 of
/// GH #943). Round 0 makes a summary -- its slot `history.summary:<rk>` and
/// its binding `summary_audience:<rk>` -- and three rounds rebuilt after it
/// push it out of a registry bounded to three. Until GH #949 only six state
/// rows fell and the binding and the slot stayed, a pair per round for good.
/// Red before the fix: `the dropped round kept state rows` (the binding) and
/// `the dropped round kept its summary slot`.
///
/// The round's clock order struck already (disarmed, `completed` at the
/// timer), so no `remove` is sent for it -- the timer would refuse one; the
/// armed case is `gh949_a_dropped_rounds_armed_order_falls_with_it`. A strike
/// of the old order finds no `last_call` and is stale. And the round that
/// comes back is answered from its own rows -- cut at its newest summary,
/// that summary's leaf laid again from the `summaries` row before the call
/// leaves, never forgotten.
#[test]
fn gh949_what_a_dropped_round_owns_falls_with_its_plan() {
    if !shipped() {
        return;
    }
    let mut h = bounded_summarising();
    let r0 = ROUNDS[0];
    let rk0 = round_key(r0);
    let slot_rows = |h: &Hive| -> usize {
        h.rows(&format!(
            "SELECT hash FROM slots WHERE path = '{}'",
            summary_slot(r0)
        ))
        .len()
    };
    turn_in(
        &mut h,
        r0,
        "t-949-0a",
        "cw949 zero one: the parcel left the depot.",
        "cw949 zero one reply: noted.",
        json!({}),
    );
    struck_in(
        &mut h,
        r0,
        "t-949-0b",
        "cw949 zero two: the lease is signed.",
        "cw949 zero two reply: noted.",
    );
    assert_eq!(
        slot_rows(&h),
        1,
        "round 0 made no summary slot: {:?}",
        h.stderr
    );
    assert_eq!(
        state_rows(&h, &format!("summary_audience:{rk0}")),
        1,
        "round 0 bound no summary leaf"
    );
    let stale = h.clock_order_for(r0).expect("the order of round 0");
    for (i, round) in ROUNDS.iter().enumerate().skip(1) {
        struck_in(
            &mut h,
            round,
            &format!("t-949-{i}"),
            &format!("cw949 round {i}: the question of this round."),
            &format!("cw949 round {i} reply: the answer of this round."),
        );
    }
    assert!(
        !registry(&h).contains_key(&rk0),
        "round 0 is still registered past max_round_plans: {:?}",
        registry(&h)
    );
    assert_eq!(
        rows_of_round(&h, &rk0),
        Vec::<Vec<Value>>::new(),
        "the dropped round kept state rows"
    );
    assert_eq!(slot_rows(&h), 0, "the dropped round kept its summary slot");
    assert!(
        !h.clock
            .iter()
            .any(|m| m.body.get("op") == Some(&json!("remove"))),
        "a struck order was removed: {:?}",
        h.clock
    );

    // The old order of the dropped round strikes: stale, nothing written.
    let asked = h.summ.len();
    h.fire(&stale);
    assert_eq!(
        h.summ.len(),
        asked,
        "a strike of the dropped round's order asked for a summary"
    );
    assert_eq!(
        h.plan_in(r0),
        json!({}),
        "a strike of the dropped round's order rebuilt it: {:?}",
        h.stderr
    );
    assert_eq!(
        rows_of_round(&h, &rk0),
        Vec::<Vec<Value>>::new(),
        "a stale strike wrote state of the dropped round"
    );

    // The round comes back: its first call shows its own summary.
    let back = curate_in(&mut h, r0, PROBE_TURN, PROBE_SAYS);
    let leaf = back.body["system"]["history"]["summary"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        leaf.ends_with(SUMMARY_WORDS),
        "the round that lost its plan forgot its summary: {:?} {:?}",
        back.body.get("system"),
        h.stderr
    );
    assert!(
        !texts(&back).join("\n").contains("cw949 zero one"),
        "the window of the round that came back is not cut at its summary: {:?}",
        texts(&back)
    );
    assert_eq!(
        slot_rows(&h),
        1,
        "the leaf laid again is not the round's slot"
    );
    let bound: Value = sj::from_str(&h.state(&format!("summary_audience:{rk0}")))
        .expect("the leaf laid again is bound");
    assert_eq!(
        bound["audience"], r0,
        "the leaf laid again is bound to another audience: {bound}"
    );
}

/// Nothing a dropped round owns falls beside a registry put that lost (review
/// W M-3 of GH #943). The registry row refuses every write here (a trigger
/// that ignores it: the CAS of `f-reg` affects no row, as when another
/// round's rebuild put it first), so the rebuild of round 3 retries three
/// times and leaves the registry as it is -- round 0 is still in it. Until
/// GH #949 the deletes of round 0 rode in the same bundle as each `f-reg`
/// and ran whatever it did. Red before the fix: `a lost registry put dropped
/// the rows of round 0`.
#[test]
fn gh949_a_lost_registry_put_drops_nothing() {
    if !shipped() {
        return;
    }
    let mut h = bounded();
    for (i, round) in ROUNDS.iter().enumerate().take(3) {
        let (turn, says, reply) = round_turn(i);
        rebuild_in(&mut h, round, &turn, &says, &reply);
    }
    let rk0 = round_key(ROUNDS[0]);
    let before = rows_of_round(&h, &rk0);
    assert!(
        before
            .iter()
            .any(|r| r[0] == json!(format!("window_plan:{rk0}"))),
        "round 0 has no plan to lose: {before:?}"
    );
    let reg = h.state("window_plans");
    h.db.execute_batch(
        "CREATE TRIGGER gh949_lose_update BEFORE UPDATE ON state \
           WHEN OLD.key = 'window_plans' BEGIN SELECT RAISE(IGNORE); END; \
         CREATE TRIGGER gh949_lose_insert BEFORE INSERT ON state \
           WHEN NEW.key = 'window_plans' BEGIN SELECT RAISE(IGNORE); END;",
    )
    .expect("the triggers");
    let (turn, says, reply) = round_turn(3);
    rebuild_in(&mut h, ROUNDS[3], &turn, &says, &reply);
    assert_eq!(
        h.state("window_plans"),
        reg,
        "the registry was put after all"
    );
    assert!(
        h.stderr.iter().any(|e| e.contains("window_plans not put")),
        "the rebuild of round 3 did not give the registry up: {:?}",
        h.stderr
    );
    assert_eq!(
        rows_of_round(&h, &rk0),
        before,
        "a lost registry put dropped the rows of round 0"
    );
}

/// A handover leaf bound to no audience (`null`: `./handover` met an
/// undeclared round, or a source from before the rule) reaches no round and
/// so belongs to none -- until GH #949 it never fell (review W M-4 of
/// GH #943: `unleaf` matched the round of the rebuild only). It falls at the
/// next rebuild of any round, by its hash: another leaf at the same path
/// stays. Red before the fix: `the leaf bound to no audience stands`.
#[test]
fn gh949_a_handover_leaf_bound_to_nobody_falls_at_any_rebuild() {
    if !shipped() {
        return;
    }
    let mut h = small_talky(&[]);
    let (t, says, reply) = EA_TURNS[0];
    turn_in(&mut h, ROUND_EA, t, says, reply, json!({}));
    let mut hashes = Vec::new();
    for text in ["cw949 handover of nobody.", "cw949 a leaf laid since."] {
        let leaf = json!({"path": "history.handover", "text": text});
        let hash = sha256_hex(&canonical(&leaf));
        h.db.execute(
            "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
             VALUES (?1, 'system', 25, ?2, 'x')",
            rusqlite::params![hash, canonical(&leaf)],
        )
        .expect("the leaf's block");
        h.db.execute(
            "INSERT INTO slots (path, hash, owner, at) VALUES ('history.handover', ?1, 'curator', 'x')",
            [&hash],
        )
        .expect("the leaf's slot");
        hashes.push(hash);
    }
    let bound = canonical(&json!({"audience": null, "hash": hashes[0]}));
    h.db.execute("DELETE FROM state WHERE key = 'handover_audience'", [])
        .expect("no binding");
    h.db.execute(
        "INSERT INTO state (key, value) VALUES ('handover_audience', ?1)",
        [&bound],
    )
    .expect("the binding");
    rebuild_in(
        &mut h,
        ROUND_EA,
        "t-949-ho",
        "cw949 ea: please sum up what we settled.",
        "cw949 ea reply: the parcel.",
    );
    let left: Vec<String> = h
        .rows("SELECT hash FROM slots WHERE path = 'history.handover'")
        .iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        !left.contains(&hashes[0]),
        "the leaf bound to no audience stands: {:?}",
        h.stderr
    );
    assert_eq!(
        left,
        vec![hashes[1].clone()],
        "the rebuild took down a leaf its claim did not name"
    );
}

/// The ledger seed holds no bare key of a round (review W M-7 of GH #943):
/// since GH #943 every one of them is a row per round, `<key>:<rk>`, put or
/// inserted under the unique `state(key)` when the round first needs it, and
/// the bare rows were read by nobody. The two pointers a leg only UPDATES
/// stay (`f-sent`, `t-cw`). A ledger born from the seed runs a round to its
/// rebuild, and no cell writes a bare key. Red before the fix: `the seed
/// carries the bare key`.
#[test]
fn gh949_the_ledger_seed_holds_no_bare_round_key() {
    if !shipped() {
        return;
    }
    const BARE: [&str; 9] = [
        "window_plan",
        "rebuild_running",
        "last_call",
        "armed_call",
        "actions_pending",
        "pending:summary",
        "summary_audience",
        "curate",
        "aim",
    ];
    let seed = std::fs::read_to_string(repo("templates/curator/ledger/seed/state.jsonl"))
        .expect("the state seed");
    let keys: Vec<String> = seed
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .filter_map(|v| v["key"].as_str().map(str::to_string))
        .collect();
    for bare in BARE {
        assert!(
            !keys.iter().any(|k| k == bare),
            "the seed carries the bare key `{bare}`: {keys:?}"
        );
    }
    for kept in ["system_hash_sent", "context_window"] {
        assert!(
            keys.iter().any(|k| k == kept),
            "the seed lost `{kept}`, which a leg only updates: {keys:?}"
        );
    }
    let mut h = small_talky(&[]);
    let (t, says, reply) = EA_TURNS[0];
    turn_in(&mut h, ROUND_EA, t, says, reply, json!({}));
    rebuild_in(
        &mut h,
        ROUND_EA,
        "t-949-seed",
        "cw949 seed: please sum up what we settled.",
        "cw949 seed reply: the parcel.",
    );
    for bare in BARE {
        assert_eq!(
            state_rows(&h, bare),
            0,
            "a cell wrote the bare key `{bare}`"
        );
    }
    assert_ne!(
        h.state("system_hash_sent"),
        "{}",
        "the update of the seeded `system_hash_sent` did not land"
    );
}

/// The clock order of a dropped round falls with its plan when it is still
/// ARMED (ledger § 2a line 6 of GH #949, review M-1): round 0 rebuilds, then
/// calls again with a cold cache -- an order armed, not struck -- and three
/// rounds rebuilt after it push it out of a registry bounded to three. The
/// timer is told `remove` for exactly the round's id, once; no other order
/// falls. The timer marks the row and keeps it (it deletes none).
///
/// Red before the fix: no `remove` reached the clock -- the armed order of a
/// round nobody holds a plan for stayed active.
#[test]
fn gh949_a_dropped_rounds_armed_order_falls_with_it() {
    if !shipped() {
        return;
    }
    let mut h = bounded_summarising();
    let r0 = ROUNDS[0];
    let rk0 = round_key(r0);
    struck_in(
        &mut h,
        r0,
        "t-949-a0",
        "cw949 zero one: the parcel left the depot.",
        "cw949 zero one reply: noted.",
    );
    turn_in(
        &mut h,
        r0,
        "t-949-a1",
        "cw949 zero two: the lease is signed.",
        "cw949 zero two reply: noted.",
        cold(),
    );
    let armed = h.clock_order_for(r0).expect("round 0 armed an order");
    assert_ne!(
        h.state(&format!("armed_call:{rk0}")),
        "",
        "round 0 holds no armed order before it is dropped"
    );
    for (i, round) in ROUNDS.iter().enumerate().skip(1) {
        struck_in(
            &mut h,
            round,
            &format!("t-949-a{}", i + 1),
            &format!("cw949 round {i}: the question of this round."),
            &format!("cw949 round {i} reply: the answer of this round."),
        );
    }
    assert!(
        !registry(&h).contains_key(&rk0),
        "round 0 is still registered past max_round_plans: {:?}",
        registry(&h)
    );
    let removed: Vec<&Value> = h
        .clock
        .iter()
        .filter(|m| m.body.get("op") == Some(&json!("remove")))
        .map(|m| &m.body["schedule_id"])
        .collect();
    assert_eq!(
        removed,
        [&armed.body["schedule_id"]],
        "the armed order of the dropped round, and only it, falls: {:?}",
        h.clock
    );
    assert_eq!(
        rows_of_round(&h, &rk0),
        Vec::<Vec<Value>>::new(),
        "the dropped round kept state rows"
    );
}
