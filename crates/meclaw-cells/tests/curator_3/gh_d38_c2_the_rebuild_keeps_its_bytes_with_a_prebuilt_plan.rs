//! GH #1448 (D38 C2, Lehre 4 of C1, OR-d38-3.o9.6) -- the rebuild keeps its
//! bytes beside a prebuilt plan, and leaves a plan that follows its cover.
//!
//! WHY this file exists: C1 broke the rebuild path (`rb-data`) four times
//! while it promised to change nothing (the consult line behind the
//! transcript, GH #1440, OR-d38-3.c1.33/.40-.42). C2 writes the window plan
//! ahead of the turn -- after the answer, at a rebuild, at the clock -- in the
//! one plan form (`state.window_plan:<rk>`, `blocks` beside the rebuild's
//! keys). Two promises hold that path:
//!
//! 1. The building blocks never feed the rebuild: the same conversation, once
//!    with its prebuilt blocks and once with every `blocks` taken out of the
//!    plan rows before the strike, sends the summarizer the same request,
//!    byte for byte, leaves the same rebuild keys and the same next call --
//!    on the branches the corpus (C0) does not walk: `summary_form` brief,
//!    `summary_state_cond` and the state line.
//! 2. After the rebuild the blocks follow the cover: no block of a condensed
//!    wall row stays, and the condensate is one block of kind `brief`.
//!
//! The clock is the corpus's (`PRELUDE` below, the text of
//! `gh_d38_c0_policy_corpus_is_byte_identical.rs`): both hives read the same
//! instants, so their bytes can be compared. No model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const T0_S: i64 = 1_772_445_600;
const STEP_S: i64 = 5;

/// The clock in front of every shipped program (python). It replaces
/// `datetime.datetime.now`/`utcnow`, `time.time`/`time_ns` and `uuid.uuid4`
/// by readings of the instant the harness writes as the first key of the
/// stdin document (`c0_clock_us`), one microsecond further per reading. A
/// `uuid4` is the run's instant and its own count of uuids in the run, not a
/// reading: a clock reading added before a mint (`handle_ms`) moves no id.
const PRELUDE: &str = r#"import datetime as _c0_dt, time as _c0_time, uuid as _c0_uuid, sys as _c0_sys, io as _c0_io, re as _c0_re
if not hasattr(_c0_dt, "_c0_state"):
    _c0_dt._c0_state = {"us": 0, "k": 0, "u": 0}
    _c0_real = _c0_dt.datetime
    _c0_epoch = _c0_real(1970, 1, 1, tzinfo=_c0_dt.timezone.utc)

    def _c0_tick():
        st = _c0_dt._c0_state
        st["k"] += 1
        return st["us"] + st["k"]

    class _C0DateTime(_c0_real):
        @classmethod
        def now(cls, tz=None):
            t = _c0_epoch + _c0_dt.timedelta(microseconds=_c0_tick())
            t = cls.combine(t.date(), t.timetz())
            return t.astimezone(tz) if tz is not None else t.replace(tzinfo=None)

        @classmethod
        def utcnow(cls):
            return cls.now(None)

    _c0_dt.datetime = _C0DateTime
    _c0_time.time = lambda: _c0_tick() / 1e6
    _c0_time.time_ns = lambda: _c0_tick() * 1000
    def _c0_uuid4():
        st = _c0_dt._c0_state
        st["u"] += 1
        return _c0_uuid.UUID(int=st["us"] << 64 | st["u"] << 8 | 0xc0, version=4)

    _c0_uuid.uuid4 = _c0_uuid4
_c0_text = _c0_sys.stdin.read()
_c0_sys.stdin = _c0_io.StringIO(_c0_text)
_c0_m = _c0_re.match(r'\{"c0_clock_us":(\d+),', _c0_text)
if _c0_m:
    _c0_dt._c0_state["us"] = int(_c0_m.group(1))
    _c0_dt._c0_state["k"] = 0
    _c0_dt._c0_state["u"] = 0
del _c0_text, _c0_m"#;

fn knobs() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        ("policy", "state_lines", json!(8)),
    ]
}

/// Takes `blocks` out of the round's plan row and out of every parked
/// turn's copy of it (`pending:*`, `plan`), as a ledger of before C2 holds
/// them.
fn strip_blocks(h: &Hive) {
    let rows = h.rows(
        "SELECT key, value FROM state WHERE key LIKE 'window_plan:%' OR key LIKE 'pending:%'",
    );
    for r in rows {
        let (key, text) = (r[0].as_str().unwrap_or(""), r[1].as_str().unwrap_or(""));
        let Ok(mut v) = sj::from_str::<Map<String, Value>>(text) else {
            continue;
        };
        let changed = if key.starts_with("window_plan:") {
            v.remove("blocks").is_some()
        } else if let Some(Ok(mut p)) = v
            .get("plan")
            .and_then(Value::as_str)
            .map(sj::from_str::<Map<String, Value>>)
        {
            let had = p.remove("blocks").is_some();
            v.insert("plan".into(), json!(sj::to_string(&p).expect("plan")));
            had
        } else {
            false
        };
        if changed {
            h.db.execute(
                "UPDATE state SET value = ?1 WHERE key = ?2",
                rusqlite::params![sj::to_string(&v).expect("row"), key],
            )
            .expect("an update");
        }
    }
}

struct Seen {
    request: String,
    plan: Value,
    next: Vec<Value>,
}

fn conversation(strip: bool) -> Seen {
    let mut h = Hive::with_prelude(&knobs(), PRELUDE);
    let mut step = 0;
    let mut tick = |h: &mut Hive| {
        step += 1;
        h.set_clock((T0_S + STEP_S * step) * 1_000_000);
    };
    for (t, q, a) in [("t1", "q1", "a1"), ("t2", "q2", "a2")] {
        tick(&mut h);
        turn(&mut h, "s1", t, q, a, json!({}));
    }
    tick(&mut h);
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2026-03-02T12:00:00Z"}),
    );
    if strip {
        strip_blocks(&h);
    }
    tick(&mut h);
    h.fire(&last_add(&h));
    let q1 = short_id(&user("q1"));
    tick(&mut h);
    let req = h.answer(
        &format!("- person (2026-03-02) garden/state: The garden is in bloom. [asserted] [#{q1}]"),
        "stop",
    );
    let mut plan = h.plan_in(ROUND_E);
    if let Some(m) = plan.as_object_mut() {
        m.remove("blocks");
    }
    tick(&mut h);
    let next = h.curate("s1", "t4", 0, json!([user("q4")]), mode("Be brief."));
    Seen {
        request: canonical(&json!({"body": req.body, "hop": req.hop})),
        plan,
        next: next.messages(),
    }
}

#[test]
fn the_rebuild_reads_as_before_beside_a_prebuilt_plan() {
    if !shipped() {
        return;
    }
    let with = conversation(false);
    let without = conversation(true);
    assert!(
        with.request.contains("[state -- ") || with.request.contains("q1"),
        "the rebuild condensed the old rounds: {}",
        with.request
    );
    assert_eq!(
        with.request, without.request,
        "the summarizer's request does not see the blocks"
    );
    assert_eq!(with.plan, without.plan, "the rebuild keys are the same");
    assert_eq!(with.next, without.next, "the next call is the same");
}

#[test]
fn after_the_rebuild_the_blocks_follow_the_cover() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&knobs());
    for (t, q, a) in [("t1", "q1", "a1"), ("t2", "q2", "a2")] {
        turn(&mut h, "s1", t, q, a, json!({}));
    }
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let q1 = short_id(&user("q1"));
    h.answer(
        &format!("- person (2026-03-02) garden/state: The garden is in bloom. [asserted] [#{q1}]"),
        "stop",
    );
    let plan = h.plan_in(ROUND_E);
    let cover = plan["cover"].as_i64().expect("the rebuild's cover");
    let covered: Vec<String> = h
        .rows(&format!("SELECT hash FROM wall WHERE seq <= {cover}"))
        .into_iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .collect();
    assert!(!covered.is_empty(), "the rebuild condensed rows: {plan}");
    let blocks = plan_blocks(&h, ROUND_E);
    assert!(
        !blocks.is_empty(),
        "the rebuild leaves the plan's blocks: {plan}"
    );
    for b in &blocks {
        let hash = b["origin"]["hash"].as_str().unwrap_or_default();
        assert!(
            !(b["origin"]["source"] == json!("wall") && covered.iter().any(|c| c == hash)),
            "a condensed row keeps no block: {b}"
        );
    }
    let briefs = blocks
        .iter()
        .filter(|b| b["kind"] == json!("brief"))
        .count();
    assert_eq!(briefs, 1, "the condensate is one block: {blocks:?}");
}

/// One turn with a tool round: the call asks `lookup`, the result comes
/// back on iteration 1, the answer closes the turn.
fn tool_turn(h: &mut Hive, t: &str, found: &str, extra: Value) -> Value {
    let id = format!("call_lookup_{t}");
    let first = h.curate(
        "s1",
        t,
        0,
        json!([user(&format!("ask {t}"))]),
        mode("Be brief."),
    );
    h.tap(
        &first,
        "tool_calls",
        json!({}),
        json!([tool_call(&id, "lookup")]),
    );
    let result = tool_result(&id, found);
    let second = h.curate(
        "s1",
        t,
        1,
        json!([tool_call(&id, "lookup"), result.clone()]),
        mode("Be brief."),
    );
    h.tap(
        &second,
        "stop",
        extra,
        json!([said(&format!("answered {t}"))]),
    );
    result
}

/// GH #1448 (Z1.9): the garbage collection works on the plan -- an old tool
/// result the rebuild shrinks is a `stub` block of the new plan, the newest
/// one stays a block of its kind, and the `gc` mark counts as many
/// `shrunk_tools` as the plan holds stubs.
#[test]
fn a_shrunk_tool_result_is_a_stub_block() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "gc", json!("1")),
        ("policy", "keep_rounds", json!(1)),
        ("policy", "keep_recent", json!(9)),
    ]);
    let old = tool_turn(&mut h, "t1", "lookup one: the lake is cold", json!({}));
    tool_turn(&mut h, "t2", "lookup two: the lake is warm", json!({}));
    let newest = tool_turn(
        &mut h,
        "t3",
        "lookup three: the lake is frozen",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let blocks = plan_blocks(&h, ROUND_E);
    let kind_of = |el: &Value| {
        let tail = format!(":{}", short_id(el));
        blocks
            .iter()
            .find(|b| b["id"].as_str().is_some_and(|id| id.ends_with(&tail)))
            .map(|b| b["kind"].as_str().unwrap_or_default().to_string())
    };
    assert_eq!(
        kind_of(&old).as_deref(),
        Some("stub"),
        "the shrunk result is a stub block: {blocks:?}"
    );
    assert_eq!(
        kind_of(&newest).as_deref(),
        Some("tool_result"),
        "the newest result stays whole: {blocks:?}"
    );
    let stubs = blocks.iter().filter(|b| b["kind"] == json!("stub")).count();
    let gc: Vec<Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'gc' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
        .collect();
    let last = gc.last().expect("the rebuild's gc mark");
    let counted: i64 = [
        "shrunk_tools",
        "dup_recall",
        "scaffold",
        "released",
        "transient",
    ]
    .iter()
    .map(|c| last["classes"][c].as_i64().unwrap_or(0))
    .sum();
    assert_eq!(
        counted as usize, stubs,
        "every collected block is a stub of the plan: {last} {blocks:?}"
    );
}
