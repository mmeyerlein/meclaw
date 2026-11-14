//! GH #1436 (d54g) -- without `carry_notes` every window is byte-identical.
//!
//! WHY this file exists: `carry_notes` (curator/policy) ships `{}`, and a
//! colony whose set never names a source must run the curator of before:
//! the same windows, byte for byte, and no row written. The class notes of
//! the route stamp (`class_notes`, the strand `stamp`) are turn notes as
//! well, but they are never carried: `carry_notes` reads only the turn notes
//! of the context, the sources a set names -- even a set that names the
//! class notes' source leaves them alone. This pin runs one conversation (a
//! crisis turn, then a later session) with `class_notes` on, three times:
//! the param unset, `{}`, and naming the class notes' source; the windows
//! and the `notes` table are equal. The declaration ships `{}`, in `params`
//! and in the setting.
//!
//! The clock is the corpus's (`PRELUDE`, the text of
//! `gh_d38_c0_policy_corpus_is_byte_identical.rs`): the three hives read the
//! same instants, so their bytes can be compared. No model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const T0_S: i64 = 1_772_445_600;
const STEP_S: i64 = 5;
const NOTE: &str = "The person may be in danger: answer with care.";
const CLASS: &str = "Answer this turn with care and stay with the person.";

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

fn crisis() -> Value {
    json!([{"id": "route.risk", "source": "route.risk", "text": NOTE, "priority": 0}])
}

fn talk(h: &mut Hive, session: &str, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("I am here with you.")]),
    );
    call
}

/// The conversation under `carry` (None: the param unset): the bodies of
/// its three calls and the `notes` table after it.
fn conversation(carry: Option<Value>) -> (Vec<String>, Vec<Vec<Value>>) {
    let mut over = vec![
        ("policy", "notes_share", json!(0.04)),
        ("policy", "class_notes", json!({"talk": {"text": CLASS}})),
    ];
    if let Some(c) = carry {
        over.push(("policy", "carry_notes", c));
    }
    let mut h = Hive::with_prelude(&over, PRELUDE);
    let mut calls = Vec::new();
    for (step, (session, turn, notes)) in [
        ("s1", "t1", crisis()),
        ("s1", "t2", json!([])),
        ("s2", "t1", json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        h.set_clock((T0_S + STEP_S * (step as i64 + 1)) * 1_000_000);
        let call = talk(&mut h, session, turn, notes);
        assert!(
            texts(&call).iter().any(|t| t.contains(CLASS)),
            "the class note stands in every window: {:?}",
            texts(&call)
        );
        calls.push(canonical(&json!({"body": call.body})));
    }
    let notes = h.rows("SELECT * FROM notes ORDER BY source, note_id");
    (calls, notes)
}

#[test]
fn the_param_ships_empty() {
    let policy = read_json(&repo("templates/curator/policy/config.json"));
    assert_eq!(
        policy["params"]["carry_notes"],
        json!({}),
        "templates/curator/policy/config.json declares carry_notes {{}}"
    );
    assert_eq!(
        policy["contract"]["settings"]["carry_notes"]["default"],
        json!({}),
        "the setting says the same"
    );
}

#[test]
fn without_carry_notes_every_window_is_byte_identical() {
    if !shipped() {
        return;
    }
    let (unset, unset_notes) = conversation(None);
    let (empty, empty_notes) = conversation(Some(json!({})));
    let (class, class_notes) = conversation(Some(
        json!({"curator": {"text": "never carried", "until": 3600}}),
    ));
    assert_eq!(unset, empty, "{{}} is the curator of before");
    assert_eq!(unset_notes, empty_notes, "and writes no row");
    assert!(
        unset_notes.is_empty(),
        "a conversation of turn notes leaves no row: {unset_notes:?}"
    );
    assert_eq!(unset, class, "the class notes' source is never carried");
    assert_eq!(unset_notes, class_notes, "and leaves no row");
}
