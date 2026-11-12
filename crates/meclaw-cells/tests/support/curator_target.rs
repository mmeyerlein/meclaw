//! D58 target (GH #1483) -- a model cell tells the curator what it is and
//! what it is used for, the way an `llm` cell with `package_line` does.
//!
//! WHY this file exists: the curator no longer knows its targets by name and
//! keeps no table of roles. A model cell puts one message on the curator's
//! `in_catalog` lane -- its catalogue row (`package`), the profile its
//! composite wrote on it (`window_profile`) and the order of its system part
//! (`system_order`), each in its own body slot -- and the curator cuts every
//! window for the target a curate names (`hop.target`, the name the composite
//! wires the cell under). The tests of the strand and the bank speak to the
//! curator through these helpers instead of naming a role.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The composite the cells of these tests are wired in: a target's name is
/// the last segment of the path its line comes from.
pub const COMPOSITE: &str = "/comp";

/// The kind of the mark for a curate the curator could not cut for a known
/// target: none named (`missing` "target") or a name without a reported
/// profile (`missing` "profile").
pub const TARGET_MARK: &str = "target_unknown";

/// A catalogue row as the registry pushes it into a model cell.
pub fn row(model: &str, input_soft: i64, input_hard: i64) -> Value {
    json!({"model": model, "input_soft": input_soft, "input_hard": input_hard,
           "max_output": 4096, "price_tiers": null, "cache_mode": "breakpoints",
           "cache_ttl_s": 300, "wire_dialect": "chat_completions",
           "supported_params": [], "chars_per_token": 3.5})
}

/// A row large enough that no window of these tests touches its lines.
pub fn roomy(model: &str) -> Value {
    row(model, 5_000_000, 10_000_000)
}

/// The cell `target` of [`COMPOSITE`] tells the curator its package: the
/// row, the profile and the order of its system part, on `in_catalog`.
pub fn announce(h: &mut Hive, target: &str, row: Value, profile: Value, system_order: Value) {
    h.out.clear();
    h.lane(
        "in_catalog",
        json!({}),
        json!({"package_of": format!("{COMPOSITE}/{target}")}),
        json!({"messages": [], "package": row, "window_profile": profile,
               "system_order": system_order}),
    );
}

/// [`announce`] with a roomy row and no order of the system part.
pub fn announce_profile(h: &mut Hive, target: &str, profile: Value) {
    announce(
        h,
        target,
        roomy(&format!("test/{target}")),
        profile,
        json!([]),
    );
}

/// The system part every curate of these tests brings: three families, one
/// of them the tool menu.
pub fn system_tree() -> Value {
    json!({"instructions": {"mode": {"text": "Be brief."}},
           "persona": {"name": {"text": "A patient companion."}},
           "tools": {"web_search": {"text": "{\"name\":\"web_search\",\"description\":\"Search.\"}"}}})
}

/// One curate of `turn` (call `iter`) in session `s1`: `round` the elements
/// the composite brings, `target` the name it gives (none: no `hop.target`),
/// `extra` further hop keys (the route stamp `path`, `needs`, `families`).
/// Every message to a model since the lane, bridges included.
pub fn curate_for(
    h: &mut Hive,
    target: Option<&str>,
    turn: &str,
    iter: u32,
    round: Value,
    extra: Value,
) -> Vec<Msg> {
    curate_with_system(h, target, turn, iter, round, extra, system_tree())
}

/// [`curate_for`] with the system part `system` instead of [`system_tree`].
pub fn curate_with_system(
    h: &mut Hive,
    target: Option<&str>,
    turn: &str,
    iter: u32,
    round: Value,
    extra: Value,
    system: Value,
) -> Vec<Msg> {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                         "phase": ""});
    if let Some(t) = target {
        hop["target"] = json!(t);
    }
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": round, "system": system}),
    );
    model_calls(h)
}

/// Every message the hive sent to a model since `out` was cleared: one with
/// a `curator_call` and a `messages` slot, whatever its route.
pub fn model_calls(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| {
            m.hop.get("curator_call").is_some_and(Value::is_string)
                && m.body.contains_key("messages")
        })
        .cloned()
        .collect()
}

/// The one call of a curate that is not a bridge.
pub fn the_call(h: &Hive, calls: &[Msg]) -> Msg {
    let main: Vec<&Msg> = calls
        .iter()
        .filter(|m| {
            !m.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(
        main.len(),
        1,
        "one call to a model: {:?} {:?}",
        h.out,
        h.stderr
    );
    main[0].clone()
}

/// [`curate_for`] with the person's words, the one call.
pub fn say(h: &mut Hive, target: Option<&str>, turn: &str, words: &str) -> Msg {
    let calls = curate_for(h, target, turn, 0, json!([user(words)]), json!({}));
    the_call(h, &calls)
}

/// The answer of the model to `call`, as the composite's edge brings it
/// back: the call's keys and its target (`context.curator_target`) lifted
/// into the context.
pub fn answer(h: &mut Hive, call: &Msg, finish: &str, usage: Value, messages: Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    // talky's edge lifts a side call's `round_open` "1" onto `interim`
    // (D58 moveout, contract point 4), else the call's own `interim`.
    if call.hop.get("round_open").is_some_and(|v| *v == "1") {
        ctx.insert("interim".into(), json!("1"));
    } else if let Some(interim) = call.hop.get("interim") {
        ctx.insert("interim".into(), interim.clone());
    }
    if let Some(t) = call.hop.get("target") {
        ctx.insert("curator_target".into(), t.clone());
    }
    // D58 moveout (contract point 3): the kind of a call rides in the
    // context as well.
    if let Some(k) = call.hop.get("call_kind") {
        ctx.insert("call_kind".into(), k.clone());
    }
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
    for (k, v) in obj(usage) {
        hop.insert(k, v);
    }
    h.lane(
        "in_llm",
        Value::Object(ctx),
        Value::Object(hop),
        json!({"messages": messages}),
    );
}

/// A whole turn for `target`: the words, the final answer.
pub fn turn_for(h: &mut Hive, target: &str, turn: &str, words: &str, reply: &str) -> Msg {
    let c = say(h, Some(target), turn, words);
    answer(h, &c, "stop", warm_usage(), json!([said(reply)]));
    c
}

/// The values of every mark of `kind`, oldest first.
pub fn marks_of_kind(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

/// What the curator holds for `target` (`catalog:<target>`), parsed.
pub fn held(h: &Hive, target: &str) -> Value {
    sj::from_str(&h.state(&format!("catalog:{target}"))).unwrap_or(Value::Null)
}

/// Whether the call stands under a hard line it was cut to.
pub fn has_overflow_cut(call: &Msg) -> bool {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().any(|x| x["what"] == "overflow_cut"))
}

/// The paths of the system part in the order the snapshot row of `call`
/// lists them (`sent`, entries whose block is a leaf of the system part).
pub fn sent_system_paths(h: &Hive, call: &Msg) -> Vec<String> {
    let sent = sent_of(h, call).unwrap_or_else(|| {
        panic!(
            "the snapshot row of call {} carries `sent`: {:?}",
            call.hop["curator_call"], h.stderr
        )
    });
    let mut out = Vec::new();
    for e in sent {
        let hash = e["hash"].as_str().unwrap_or_default();
        let rows = h.rows(&format!("SELECT body FROM blocks WHERE hash = '{hash}'"));
        let Some(body) = rows.first().and_then(|r| r[0].as_str()) else {
            continue;
        };
        let el: Value = sj::from_str(body).unwrap_or(Value::Null);
        if el.get("origin").is_none()
            && let Some(p) = el.get("path").and_then(Value::as_str)
        {
            out.push(p.to_string());
        }
    }
    out
}

/// The `cut` object of the snapshot row of `call`.
pub fn cut_of(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    rows.first()
        .and_then(|r| r[0].as_str())
        .and_then(|t| sj::from_str(t).ok())
        .unwrap_or(Value::Null)
}

/// The corpus clock of the C0 corpus, so that a window is the same bytes on
/// every run: every cell runs behind [`PRELUDE`], and a test sets the clock
/// with [`Hive::set_clock`] before each step.
pub const T0_US: i64 = 1_772_445_600_000_000;

/// The C0 corpus prelude (`gh_d38_c0_policy_corpus_is_byte_identical.rs`):
/// `datetime.now`, `time.time` and `uuid4` read the instant the harness
/// writes as the first key of the stdin document.
pub const PRELUDE: &str = r#"import datetime as _c0_dt, time as _c0_time, uuid as _c0_uuid, sys as _c0_sys, io as _c0_io, re as _c0_re
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

/// The words and answers of the conversation [`fixed_window`] runs.
pub const TALK: [(&str, &str); 4] = [
    ("Good morning.", "Good morning to you."),
    ("My sister visits today.", "That sounds lovely."),
    ("She brings her dog.", "A dog, how nice."),
    ("We will walk by the river.", "Enjoy the walk."),
];

/// The window of the fifth call of a fixed conversation, on the corpus
/// clock: `before` runs on the fresh hive (an announcement, say), then four
/// turns for `target` and the words of a fifth; returns that call's body
/// parts `system`, `messages` and `tool_scope`.
pub fn fixed_window(
    over: &[(&str, &str, Value)],
    target: Option<&str>,
    before: impl FnOnce(&mut Hive),
) -> Value {
    let mut h = Hive::with_prelude(over, PRELUDE);
    h.set_clock(T0_US);
    before(&mut h);
    for (i, (words, reply)) in TALK.iter().enumerate() {
        h.set_clock(T0_US + 5_000_000 * (2 * i as i64 + 1));
        let c = say(&mut h, target, &format!("t{i}"), words);
        h.set_clock(T0_US + 5_000_000 * (2 * i as i64 + 2));
        answer(&mut h, &c, "stop", warm_usage(), json!([said(reply)]));
    }
    h.set_clock(T0_US + 5_000_000 * 9);
    let c = say(&mut h, target, "t9", "What should we cook tonight?");
    json!({"system": c.body.get("system").cloned().unwrap_or(Value::Null),
           "messages": c.body.get("messages").cloned().unwrap_or(Value::Null),
           "tool_scope": c.body.get("tool_scope").cloned().unwrap_or(Value::Null)})
}

/// The names no script of the curator, no edge and no contract of its hive
/// may name (contract point 14). Every later strand of the stage adds its
/// own names here; there are no exceptions.
pub const LOCKED_NAMES: [&str; 31] = [
    "brain",
    "quick",
    "quick_spec",
    "quick_bridge",
    "strong",
    "consult",
    "research",
    "talky",
    "coding",
    "cogny",
    "consult_cogny",
    "sa_stage",
    // D58 lead (GH #1474): the switch and its store left the curator.
    "lead_state",
    "curator-lead",
    // D58 live (GH #1482): the curator knows a running session only by its
    // handle `live`; the voice's engine and its lanes' names stay outside.
    "duplex",
    "in_call_end",
    "call_facts",
    // D58 judge (GH #1480): the judge is a model like any other, its window a
    // view of a target the caller names; a screen, a wind-down line and the
    // person's hints are contributions and seed lines without a kind.
    "judge",
    "screen",
    "wind_down",
    "hints",
    // D58 note (GH #1477): the compositions that hand `note_receipt` out at
    // their rim (talky and cogny stand above); where it goes is theirs.
    "simple-agent",
    "figure",
    // D58 frame (GH #1478): what a turn of a stamp class gets is the
    // composition's `class_notes`; the curator names no class of its own.
    // The lead hive (`templates/lead`) is not read by this lock.
    "clarify",
    // D58 feed (GH #1481): the curator sends a feed on its own route
    // `live_feed` and passes the section on uninterpreted; the receiving
    // cell and the lane its composition translates the feed into stay
    // outside.
    "voice",
    "in_advise",
    // D58 moveout (GH #1476): the bridge, the waiting-time advise and the
    // early interrupt left the curator -- it offers a conditional side call
    // and a conditional feed; what they say and when is the lead's.
    "bridge",
    "horizon_advise",
    "interrupt",
    // D58 proof (GH #1475): the name simple-agent carries its target under.
    "sa_target",
    // D58 followup (GH #1479): the person's wellbeing is the hive
    // `assistant`'s; the curator carries a registered class unnamed.
    "wellbeing",
];

/// The lock's reader: a whole string literal of the code (a comment, a
/// docstring or a bare string statement is no code; a part of a longer
/// literal is no name), a quoted literal or a whole string value in an edge,
/// a whole string value in the contract. A literal in the form of a phase
/// (words joined by `-`, `view-rows`) or of a lane (`in_<words>`) is read by
/// its words as well (split at `-` and `_`): review I2 of the D58 judge
/// strand found the phase `judge-write` in new curator code, which the
/// whole-literal reader let through. Other literals stay whole, so a param
/// or key that only starts with a name (`judge_context`, `hints_of`) is none.
const LOCK_READER: &str = r#"
import ast, json, re, sys
d = json.load(sys.stdin)
names = set(d["names"])
PHASE = re.compile(r"^[a-z][a-z0-9_]*(?:-[a-z0-9_]+)+$")
LANE = re.compile(r"^in_[a-z0-9_]+$")
def hits_of(s):
    words = set(re.split(r"[-_]", s)) if PHASE.match(s) or LANE.match(s) else set()
    return sorted(({s} | words) & names)
out = []
for src in d["sources"]:
    try:
        tree = ast.parse(src["code"])
    except SyntaxError as e:
        out.append("%s: does not parse: %s" % (src["where"], e))
        continue
    prose = set(id(n.value) for n in ast.walk(tree) if isinstance(n, ast.Expr))
    for n in ast.walk(tree):
        if isinstance(n, ast.Constant) and isinstance(n.value, str) and id(n) not in prose:
            for h in hits_of(n.value):
                out.append("%s:%d: %r" % (src["where"], n.lineno, h if h == n.value
                                          else "%s in %s" % (h, n.value)))
QUOTED = re.compile(r"'([^']*)'|\"([^\"]*)\"")
def strings(v):
    if isinstance(v, str):
        yield v
    elif isinstance(v, list):
        for x in v:
            yield from strings(x)
    elif isinstance(v, dict):
        for x in v.values():
            yield from strings(x)
for i, e in enumerate(d["edges"]):
    for s in strings({k: v for k, v in e.items() if k not in ("from", "to")}):
        hits = set()
        for q in {s} | {a or b for a, b in QUOTED.findall(s)}:
            hits.update(hits_of(q))
        for h in sorted(hits):
            out.append("edge %d (%s -> %s): %r" % (i, e.get("from"), e.get("to"), h))
for s in strings(d["contract"]):
    for h in hits_of(s):
        out.append("contract: %r" % h)
print(json.dumps(out))
"#;

/// Every name of `names` the lock finds in `sources` (`(where, code)`),
/// `edges` and `contract`.
pub fn lock_findings(
    names: &[&str],
    sources: &[(String, String)],
    edges: &Value,
    contract: &Value,
) -> Vec<String> {
    let doc = json!({
        "names": names,
        "sources": sources.iter().map(|(w, c)| json!({"where": w, "code": c})).collect::<Vec<_>>(),
        "edges": edges, "contract": contract,
    });
    let out = run_python(LOCK_READER, &sj::to_string(&doc).expect("json"));
    assert!(
        out.status.success(),
        "the lock's reader runs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice::<Vec<String>>(&out.stdout)
        .unwrap_or_else(|e| panic!("not a list ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

/// The script of every cell under `templates/curator/`, `(where, code)`.
pub fn curator_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![repo("templates/curator")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                let cfg = read_json(&p);
                if let Some(code) = cfg["params"]["script_inline"].as_str() {
                    let rel = p
                        .strip_prefix(repo(""))
                        .map(|r| r.display().to_string())
                        .unwrap_or_else(|_| p.display().to_string());
                    out.push((rel, code.to_string()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Every order the clock received since the first `since` messages, `add`s
/// only, newest last.
pub fn adds_since(h: &Hive, since: usize) -> Vec<Msg> {
    h.clock[since..]
        .iter()
        .filter(|m| m.body.get("op") == Some(&json!("add")))
        .cloned()
        .collect()
}

/// The answer of a call whose window stands at 2000 tokens, under a soft
/// line far above it: only a target of the wall (`window_target`) orders a
/// rebuild of its own (`curate_mark` "target").
pub fn wall_usage() -> Value {
    warm_usage()
}
