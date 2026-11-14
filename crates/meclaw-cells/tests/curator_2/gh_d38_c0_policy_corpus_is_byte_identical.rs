//! GH #1446 (D38 C0) -- the curator's behaviour, recorded before the cut.
//!
//! WHY this file exists: the strands after it take the curator apart (the
//! way decision into a cell of its own, the window plan built ahead of the
//! turn, the cut per target model) and every one of them promises to change
//! no behaviour. This corpus is the oracle for that promise: each case drives
//! the shipped curator hive (`support/curator_hive.rs`) at its rim -- the
//! messages a talky or cogny composite hands it (`in_curate`, the taps of
//! the model's answers on `in_llm`, the spec lanes, the clock's strikes, the
//! summarizer's answers) -- and records, step by step, what leaves the hive
//! (route, the selector keys of the hop (`HOP_KEYS`) and body of every
//! message to the parent, the body of every request to the summarizer) and
//! every mark it writes. The fixtures under `tests/fixtures/d38_c0/` were
//! written from the hive as it stood at the basis of the strand; a later run
//! must give the same messages byte for byte (canonical JSON) and every
//! recorded mark again, unchanged -- each of the three as a multiset per
//! step: the order of two deliveries is the depth of their chains inside the
//! hive, not behaviour. A mark of a NEW kind listed in `EXTRA_MARKS` may come
//! in addition -- the list is empty at the recording, a strand that adds a
//! station with a mark kind of its own names it there.
//!
//! The clock: the shipped programs read the wall clock (`datetime.now`,
//! `time.time`) and mint ids from it (`uuid4`, wall sequence numbers). Each
//! program runs behind `PRELUDE`, which replaces the three with a clock the
//! test sets per step (`Hive::set_clock`, `T0_S` + `STEP_S` per step, plus
//! `CLOCK_STEP_US` per cell run); nothing of the shipped text changes.
//!
//! GH #1474: the lead is a hive of its own. talky's cases run with it wired
//! beside the curator as talky ships it (the harness's bridge from talky's
//! edges; what the bridge takes is no message to the parent), cogny's cases
//! without one -- they ask no lead and write no `lead` mark.
//! Envelope ids are not compared, only the hop keys an edge selects on; the
//! one normalisation is `sub_ms` (the microseconds of a reading below its
//! millisecond).
//!
//! `MECLAW_D38_C0_BLESS=1` writes the fixtures instead of comparing (the
//! switch travels to a lane and the fixtures come back like the gh277
//! goldens, `scripts/pipeline/lane.py` `GOLDEN_SWITCHES`). No model is
//! called: every answer is the test's.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const BLESS: &str = "MECLAW_D38_C0_BLESS";
const FIXTURES: &str = "crates/meclaw-cells/tests/fixtures/d38_c0";

/// Mark kinds a later strand may add beside the recorded ones -- only kinds
/// the corpus never recorded (C1: a `lead` mark of its own, C2: the plan
/// snapshot). A recorded kind here (`route`, say) turns its cases red: a
/// mark that moves to another cell must come back unchanged, and a new field
/// such as `handle_ms` belongs in a new kind. A recording leaves these kinds
/// out (`recorded`): `lead` carries `handle_ms`, the lead's own time, which no
/// corpus clock fixes.
const EXTRA_MARKS: &[&str] = &["lead"];

/// talky's marker of the lead hive it wires beside its curator (GH #1474).
const TALKY_LEAD_MARKER: &str = "templates/talky/lead/config.json";

/// The knobs C1 moved from `curator/policy` to `curator/lead` (GH #1447, the
/// lead hive `templates/lead` since GH #1474): a
/// set or case override of `policy.<knob>` lands on `lead.<knob>`. The place
/// moves, the value does not.
const LEAD_KNOBS: &[&str] = &[
    "path_routes",
    "memory_to_brain",
    "quick_speculative",
    "spec_tools",
    "quick_max_iter",
    "consult_pending_ms",
    "departures",
    "fallback",
    "spec_target",
    "bridge_target",
    "errand_tool",
    "errand_family",
    "delta_speculation",
    "interrupt_on_delta",
    "interrupt_questions",
];

/// The hop keys of a message leaving the hive that the corpus compares, byte
/// for byte (OR-d38-3.o9.1): the keys the edges out of `./curator` in
/// `templates/talky/config.json` and `templates/cogny/config.json` select on
/// (`route` -- recorded beside the hop --, `spec`, `iter`, `curator_call`,
/// `finish_reason`, `gap_ask`, `refused_subscriber`) or lift into the
/// context by their modifiers, where the next cell's edges select on them
/// (`interim`, `summarize`, `summary_call`, `summary_token`,
/// `episode_turn_id`, `recall_input_soft`), the keys talky's port
/// names for the errand (`consult_id`, `consult_class`) and for the live feed
/// (`section`, `at_turn_end`, `live`, `fact_source`; D58 live: the handle
/// `live` in place of `call_id`), and the turn's
/// identity the hop repeats (`session_id`, `turn_id`, `audience_set`). A key
/// a message does not carry is not recorded; any other hop key is free.
const HOP_KEYS: &[&str] = &[
    "spec",
    "iter",
    "curator_call",
    "finish_reason",
    "gap_ask",
    "refused_subscriber",
    "interim",
    "summarize",
    "summary_call",
    "summary_token",
    "episode_turn_id",
    "recall_input_soft",
    "consult_id",
    "consult_class",
    // D58 target (contract point 8): the target of a model call; free in the
    // corpus comparison of the strand (`corpus-free-target.json`).
    "target",
    "section",
    "at_turn_end",
    "live",
    "fact_source",
    "session_id",
    "turn_id",
    "audience_set",
];

/// The corpus clock: 2026-03-02T10:00:00Z, one step every five seconds.
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

/// Which composite the curator sits in, with which knobs.
#[derive(Clone, Copy, PartialEq)]
enum Knobs {
    /// The template's own overrides (`templates/<composite>/curator`).
    Template,
    /// Those plus the curator overrides of set 0.56 for the channel
    /// (`talky/curator/*` for the phone, `talky-chat/curator/*` for text).
    Set056,
}

#[derive(Clone, Copy, PartialEq)]
enum Channel {
    Text,
    Phone,
}

#[derive(Clone, Copy, PartialEq)]
enum Composite {
    Talky,
    Cogny,
}

/// Set 0.56, `assistant` overrides of `talky/curator/policy` (the phone).
fn set_policy_phone() -> Value {
    json!({"quick_tools": "web,weather,story_read,orga", "quick_max_iter": 3,
           "delta_speculation": true, "live_feed": true, "live_feed_section": "renew",
           "memory_to_brain": "1", "window_target": 100000, "keep_recent_tokens": 80000,
           "keep_rounds": 1, "rebuild_to": 0.45, "notes_share": 0.04, "topic_aging": "1",
           "state_block": "1", "state_lines": 8, "summarizer_self": "0"})
}

/// Set 0.56, `talky-chat/curator/policy` (text).
fn set_policy_text() -> Value {
    json!({"quick_tools": "web,weather,story_read,orga", "quick_max_iter": 3,
           "memory_to_brain": "1", "topic_aging": "1", "state_block": "1", "state_lines": 8,
           "summarizer_self": "0"})
}

/// Set 0.56, `talky/curator/history` (phone) -- `seed_anchor` as the set
/// file writes it, before the set builder fills it in.
fn set_history_phone() -> Value {
    json!({"timezone": "Europe/Berlin", "seed_entries_max": 5,
           "seed_anchor": "{seed_anchor}", "topic_aging": "1"})
}

/// Set 0.56, `talky-chat/curator/history` (text).
fn set_history_text() -> Value {
    json!({"topic_aging": "1"})
}

/// The overrides of one case: the composite's marker, the set's knobs, then
/// the case's own (the last word wins).
fn overrides(
    composite: Composite,
    knobs: Knobs,
    channel: Channel,
    own: &[(&str, &str, Value)],
) -> Vec<(String, String, Value)> {
    let marker = match composite {
        Composite::Talky => "templates/talky/curator/config.json",
        Composite::Cogny => "templates/cogny/curator/config.json",
    };
    let mut over: Vec<(String, String, Value)> = Vec::new();
    let mut put = |cell: &str, params: Value| {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            let cell = if cell == "policy" && LEAD_KNOBS.contains(&k.as_str()) {
                "lead"
            } else {
                cell
            };
            over.retain(|(c, key, _)| !(c == cell && *key == k));
            over.push((cell.to_string(), k, v));
        }
    };
    for (cell, params) in read_json(&repo(marker))["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        put(&cell, params);
    }
    match composite {
        // GH #1474: talky wires a lead hive beside its curator (its curator
        // marker sets `policy.lead_wired`) and names the lead's knobs at its
        // own marker `talky/lead` -- the targets of every way, `*:brain` for
        // a turn without a path entry of its own (OR-d58-2.target.39 / .41).
        // The harness builds the lead hive and the bridge from talky's
        // shipped edges.
        Composite::Talky => {
            for (cell, params) in read_json(&repo(TALKY_LEAD_MARKER))["override_params"]
                .as_object()
                .cloned()
                .unwrap_or_default()
            {
                put(&cell, params);
            }
        }
        // GH #1474: cogny wires no lead; its curator keeps the curator's own
        // `lead_wired` (the harness otherwise wires the lead as talky does).
        Composite::Cogny => put(
            "policy",
            json!({"lead_wired": cell_config("policy")["params"]["lead_wired"].clone()}),
        ),
    }
    if knobs == Knobs::Set056 && composite == Composite::Talky {
        let (policy, history) = match channel {
            Channel::Phone => (set_policy_phone(), set_history_phone()),
            Channel::Text => (set_policy_text(), set_history_text()),
        };
        put("policy", policy);
        put("history", history);
    }
    for (cell, k, v) in own {
        put(cell, json!({ *k: v }));
    }
    over
}

/// The window values a set or a case puts on the curator instance.
const VALUE_KEYS: [&str; 18] = [
    "compress_at",
    "rebuild_to",
    "keep_recent",
    "keep_recent_tokens",
    "horizon",
    "tiers",
    "keep_rounds",
    "gc",
    "gc_scaffold",
    "stub_tools_after",
    "broadcast_mode",
    "recall_push",
    "recall_budget",
    "short_ids",
    "identity_budget",
    "window_target",
    "notes_share",
    "state_lines",
];

/// The values a call's cut reads beside the short window and the notes
/// (contract point 3, Nachtrag 11.11. Nr. 1).
const CALL_KEYS: [&str; 5] = [
    "window_target",
    "state_lines",
    "short_ids",
    "identity_budget",
    "broadcast_mode",
];

/// The profiles of the composite's model cells as the composite writes
/// them, with what the case puts on the curator moved in (contract point 3,
/// "precedence": every window value a composite sets on its curator moves
/// into the profile of its main model; OR-d58-2.target.60): the main cell
/// takes every window value, a fast cell the call's values its cut read
/// from the instance before, and `quick_tools` becomes its list `allow`.
/// Returns the overrides left for the instance and `(cell, profile, order)`.
#[allow(clippy::type_complexity)]
fn moved_into_profiles<'a>(
    composite: Composite,
    over: Vec<(&'a str, &'a str, Value)>,
) -> (Vec<(&'a str, &'a str, Value)>, Vec<(String, Value, Value)>) {
    let mut moved = json!({});
    let mut allow: Option<Value> = None;
    let mut left = Vec::new();
    for (cell, k, v) in over {
        if cell == "policy" && k == "quick_tools" {
            allow = Some(json!(
                v.as_str()
                    .unwrap_or("")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            ));
            continue;
        }
        if cell == "policy" && VALUE_KEYS.contains(&k) {
            moved[k] = v.clone();
        }
        left.push((cell, k, v));
    }
    let (comp, cells): (&str, &[&str]) = match composite {
        Composite::Talky => ("talky", &["brain", "quick", "quick_spec", "quick_bridge"]),
        Composite::Cogny => ("cogny", &["brain"]),
    };
    let mut out = Vec::new();
    for cell in cells {
        let params =
            read_json(&repo(&format!("templates/{comp}/{cell}/config.json")))["params"].clone();
        let mut profile = params["window_profile"].clone();
        let main = profile["system"] == json!("delta");
        for (k, v) in moved.as_object().cloned().unwrap_or_default() {
            if main || CALL_KEYS.contains(&k.as_str()) {
                profile[k] = v;
            }
        }
        if let (false, Some(a)) = (main, &allow) {
            profile["allow"] = a.clone();
        }
        out.push((cell.to_string(), profile, params["system_order"].clone()));
    }
    (left, out)
}

/// One case of the corpus: the hive, what it said so far, the steps.
struct Run {
    h: Hive,
    name: String,
    channel: Channel,
    steps: Vec<Value>,
    n: i64,
    last_seq: i64,
    last_out: Vec<Msg>,
    /// The rebuild orders the last step armed (`add` with a `curate_mark`
    /// other than `none`).
    last_adds: usize,
}

const SUMMARY: &str = "The person and the companion talked about the week ahead.";

fn system() -> Value {
    json!({"instructions": {"mode": {"text": "Be brief."},
                            "sidecar": {"text": "End with a sidecar block."}}})
}

fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "tokens_cached": 0,
           "input_soft": 90000, "input_hard": 200000})
}

/// The target the composite's curate edge (`./collector -> ./curator`, route
/// `in_curate`) sets on `hop.target`, without the CEL quotes.
fn curate_target(composite: Composite) -> String {
    let root = match composite {
        Composite::Talky => "templates/talky/config.json",
        Composite::Cogny => "templates/cogny/config.json",
    };
    let cfg = read_json(&repo(root));
    let targets: Vec<String> = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| {
            e["from"] == "./collector"
                && e["to"] == "./curator"
                && e["modifier"]["set_hop"]["route"] == "'in_curate'"
        })
        .filter_map(|e| e["modifier"]["set_hop"]["target"].as_str())
        .map(|t| t.trim_matches('\'').to_string())
        .collect();
    assert_eq!(
        targets.len(),
        1,
        "{root}: one curate edge names hop.target: {targets:?}"
    );
    targets[0].clone()
}

impl Run {
    fn new(
        name: &str,
        composite: Composite,
        knobs: Knobs,
        channel: Channel,
        own: &[(&str, &str, Value)],
    ) -> Self {
        let over = overrides(composite, knobs, channel, own);
        let over: Vec<(&str, &str, Value)> = over
            .iter()
            .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
            .collect();
        let (over, profiles) = moved_into_profiles(composite, over);
        let mut h = Hive::with_prelude(&over, PRELUDE);
        // GH #1474 (u-lead contract point 3 and 5): a curate names its target
        // the way the composite's curate edge sets it (`hop.target`); without
        // a lead nothing else names one. Read off the shipped edge, not
        // written here.
        h.target = Some(curate_target(composite));
        // OR-d58-2.target.60 (c): every model cell of the composite reports
        // its package with its profile before the first step, as the
        // registry's push after the birth does -- the corpus measures the
        // profile path. The row names its model only: the window stays the
        // one the answers report.
        for (cell, profile, order) in profiles {
            h.lane(
                "in_catalog",
                json!({}),
                json!({"package_of": format!("/comp/{cell}")}),
                json!({"messages": [], "package": {"model": format!("test/{cell}")},
                       "window_profile": profile, "system_order": order}),
            );
        }
        h.out.clear();
        Run {
            h,
            name: name.to_string(),
            channel,
            steps: Vec::new(),
            n: 0,
            last_seq: 0,
            last_out: Vec::new(),
            last_adds: 0,
        }
    }

    /// One step: the clock set, the action run to rest, the summarizer's
    /// requests answered, the whole step recorded.
    fn step(&mut self, label: &str, act: impl FnOnce(&mut Hive)) {
        self.h.set_clock((T0_S + STEP_S * self.n) * 1_000_000);
        self.n += 1;
        self.h.out.clear();
        let clock_before = self.h.clock.len();
        act(&mut self.h);
        let mut summarizer = Vec::new();
        while let Some(req) = self.h.summ.front().cloned() {
            assert!(
                summarizer.len() < 8,
                "{}: the summarizer never rests",
                self.name
            );
            summarizer.push(Value::Object(req.body.clone()));
            self.h.answer(SUMMARY, "stop");
        }
        let out: Vec<Value> = self
            .h
            .out
            .iter()
            .map(|m| {
                let hop: sj::Map<String, Value> = HOP_KEYS
                    .iter()
                    .filter_map(|k| m.hop.get(*k).map(|v| (k.to_string(), v.clone())))
                    .collect();
                json!({"route": m.route(), "hop": Value::Object(hop),
                       "body": Value::Object(m.body.clone())})
            })
            .collect();
        let marks: Vec<Value> = self
            .h
            .rows(&format!(
                "SELECT seq, kind, session_id, turn_id, audience_set, value FROM marks \
                 WHERE seq > {} ORDER BY seq",
                self.last_seq
            ))
            .into_iter()
            .map(|r| {
                if let Some(s) = r[0].as_i64() {
                    self.last_seq = self.last_seq.max(s);
                }
                json!({"kind": r[1], "session_id": r[2], "turn_id": r[3],
                       "audience_set": r[4], "value": r[5]})
            })
            .collect();
        assert!(
            !self.h.stderr.iter().any(|e| e.contains("Traceback")),
            "{}: {label}: {:?}",
            self.name,
            self.h.stderr
        );
        self.steps
            .push(json!({"step": format!("{:02} {label}", self.n),
                               "out": out, "summarizer": summarizer, "marks": marks}));
        self.last_out = self.h.out.clone();
        self.last_adds = self.h.clock[clock_before..]
            .iter()
            .filter(|m| m.body.get("op") == Some(&json!("add")))
            .filter(|m| {
                m.body["emit_body"]["curate_mark"]
                    .as_str()
                    .is_some_and(|k| !k.is_empty() && k != "none")
            })
            .count();
    }

    fn seam_parts(&self, turn: &str, iter: &str, path: &str, extra: Value) -> (Value, Value) {
        let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                             "path": path, "needs": "", "families": ""});
        let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                             "audience_set": ROUND_E, "path": path});
        match self.channel {
            Channel::Phone => {
                // D58 live: the edges of the composition translate the
                // voice's keys into the handle; the curator reads `live`.
                hop["live"] = json!("call-1");
                ctx["channel"] = json!("phone");
            }
            Channel::Text => ctx["channel"] = json!("chat"),
        }
        for (k, v) in obj(extra) {
            hop[k] = v;
        }
        (ctx, hop)
    }

    /// The person's words of `turn` on `in_curate`, stamped `path`, with the
    /// stamp's `needs`/`families` and further hop keys in `extra`.
    fn say(&mut self, turn: &str, path: &str, extra: Value, text: &str) {
        let (ctx, hop) = self.seam_parts(turn, "0", path, extra);
        let body = json!({"messages": [user(text)], "system": system()});
        self.step(&format!("say {turn} {path} {text}"), |h| {
            h.lane("in_curate", ctx, hop, body)
        });
    }

    /// The next iteration of a tool round: the call and its result.
    fn tool_result(&mut self, turn: &str, iter: &str, path: &str, extra: Value, result: &str) {
        let (ctx, hop) = self.seam_parts(turn, iter, path, extra);
        let body = json!({"messages": [tool_call("c1", "web_search"),
                                       tool_result("c1", result)],
                          "system": system()});
        self.step(&format!("tool result {turn} {iter}"), |h| {
            h.lane("in_curate", ctx, hop, body)
        });
    }

    /// The call of the last step on `route` (not a bridge call).
    fn call_on(&self, route: &str) -> Msg {
        let calls: Vec<&Msg> = self
            .last_out
            .iter()
            // D58 target (contract point 8): a call is found under its way
            // of before too (`way_of`, read off `hop.target`).
            .filter(|m| m.route() == route || way_of(m) == route)
            .filter(|m| {
                m.hop.get("call_kind").is_none_or(|v| *v != "side")
                    && !m.hop["curator_call"]
                        .as_str()
                        .unwrap_or("")
                        .ends_with("-bridge")
            })
            .collect();
        assert_eq!(
            calls.len(),
            1,
            "{}: one call on {route}: {:?} {:?}",
            self.name,
            self.last_out,
            self.h.stderr
        );
        calls[0].clone()
    }

    /// The model's answer to the last step's call on `route`.
    fn reply(&mut self, route: &str, finish: &str, extra: Value, messages: Value) {
        let call = self.call_on(route);
        self.step(&format!("reply {route} {finish}"), |h| {
            h.tap(&call, finish, extra, messages)
        });
    }

    fn reply_text(&mut self, route: &str, text: &str) {
        self.reply(route, "stop", usage(1200), json!([said(text)]));
    }

    /// The collector's early curate of `turn` (`quick_speculative`): no path.
    fn spec(&mut self, turn: &str, text: &str) {
        let (mut ctx, mut hop) = self.seam_parts(turn, "0", "", json!({"spec": "1"}));
        ctx["path"] = json!("");
        hop["path"] = json!("");
        let body = json!({"messages": [user(text)]});
        self.step(&format!("spec {turn}"), |h| {
            h.lane("in_curate", ctx, hop, body)
        });
    }

    /// The quick voice's answer to the spec call, held by the policy.
    fn spec_answer(&mut self, text: &str) {
        let call = self.call_on("quick");
        let mut ctx = call.context.clone();
        for k in ["curator_call", "turn_id", "session_id", "iter"] {
            ctx.insert(k.into(), call.hop[k].clone());
        }
        ctx.insert("spec".into(), json!("1"));
        let body = json!({"messages": [said(text)]});
        self.step("spec answer", |h| {
            h.lane(
                "in_spec_llm",
                Value::Object(ctx),
                json!({"finish_reason": "stop", "model": "test-model", "tokens_prompt": 10}),
                body,
            )
        });
    }

    /// The memory names a topic of `turn` (a `topic` mark in the ledger,
    /// between this step and the next): input, not a mark of the hive, so
    /// the next step does not record it.
    fn name_topic(&mut self, turn: &str, name: &str) {
        let at = chrono::DateTime::from_timestamp(T0_S + STEP_S * (self.n - 1) + 4, 0)
            .expect("a corpus instant");
        let value = canonical(&json!({"movement": "start", "name": name}));
        self.h.mark(at, "s1", turn, "topic", &value);
        self.last_seq = self.last_seq.max(at.timestamp_micros());
    }

    /// The strike of the newest order the clock holds (a rebuild).
    fn fire(&mut self) {
        let order = last_add(&self.h);
        self.step("fire", |h| h.fire(&order));
    }

    /// The consult id the last step's errand carries.
    fn errand_id(&self, route: &str) -> String {
        self.call_on(route).hop["consult_id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn routes(&self) -> Vec<String> {
        let mut r: Vec<String> = Vec::new();
        for s in &self.steps {
            for o in s["out"].as_array().into_iter().flatten() {
                r.push(o["route"].as_str().unwrap_or("").to_string());
                // D58 target (contract point 8): the way of before of a model
                // call or an errand, as `way_of` reads it.
                let mut hop = o["hop"].as_object().cloned().unwrap_or_default();
                hop.insert("route".into(), o["route"].clone());
                r.push(way_of(&Msg {
                    context: Default::default(),
                    hop,
                    body: Default::default(),
                    reply_to: String::new(),
                }));
            }
            if !s["summarizer"].as_array().is_none_or(Vec::is_empty) {
                r.push("summarize".to_string());
            }
        }
        r
    }

    /// Compare with the fixture, or write it under the bless switch. `ways`
    /// are routes the case must show (so a fixture never records a case that
    /// lost its way).
    fn finish(self, ways: &[&str]) {
        let routes = self.routes();
        for w in ways {
            assert!(
                routes.iter().any(|r| r == w),
                "{}: the case takes the way {w}: {routes:?} {:?}",
                self.name,
                self.h.stderr
            );
        }
        let got = json!({"case": self.name, "steps": self.steps});
        let path = repo(&format!("{FIXTURES}/{}.json", self.name));
        if std::env::var(BLESS).as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().expect("a fixture dir")).expect("fixture dir");
            let text = sj::to_string_pretty(&recorded(&got)).expect("json") + "\n";
            std::fs::write(&path, text).expect("the fixture is written");
            return;
        }
        assert!(
            path.is_file(),
            "{}: no fixture at {} -- record it with {BLESS}=1 on the basis",
            self.name,
            path.display()
        );
        compare(&self.name, &read_json(&path), &got);
    }
}

/// The one normalisation of the corpus, on both sides: the microseconds
/// below the millisecond of every corpus-clock reading in a text -- the last
/// three fraction digits of an ISO stamp (`10:00:10.000033Z` reads
/// `10:00:10.000uuuZ`) and of a sixteen-digit wall sequence number. They
/// count only the cell runs and clock readings of a step before the reading
/// (`CLOCK_STEP_US`, one microsecond per reading), which a strand that moves
/// code between cells changes without changing behaviour; the millisecond,
/// which carries the step, stays exact. Seen at the recording in the brain
/// call's `system.consult.pending.text` ("since ...") and
/// `system.history.summary.text` ("ledger rows up to ...", the row range).
/// Both are texts of a body, so the mask cannot keep to stamp fields: the
/// readings it must hide stand inside the prose. What it hides there too --
/// a row bound one row off within the same millisecond -- stays visible in
/// the summarizer's request, whose text of the condensed rows is compared.
fn sub_ms(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = b.to_vec();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let mut i = 0;
    while i < b.len() {
        // `hh:mm:ss.ffffffZ`: the fraction's last three digits.
        if i + 16 <= b.len()
            && (0..16).all(|k| match k {
                2 | 5 => b[i + k] == b':',
                8 => b[i + k] == b'.',
                15 => b[i + k] == b'Z',
                _ => digit(i + k),
            })
            && !digit(i.wrapping_sub(1))
        {
            out[i + 12..i + 15].copy_from_slice(b"uuu");
            i += 16;
            continue;
        }
        // A run of exactly sixteen digits starting with the corpus epoch.
        if digit(i) && !digit(i.wrapping_sub(1)) {
            let mut j = i;
            while digit(j) {
                j += 1;
            }
            if j - i == 16 && b[i..].starts_with(b"17724") {
                out[j - 3..j].copy_from_slice(b"uuu");
            }
            i = j;
            continue;
        }
        i += 1;
    }
    String::from_utf8(out).expect("ascii replaced by ascii")
}

fn masked(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(sub_ms(s)),
        Value::Array(a) => Value::Array(a.iter().map(masked).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), masked(x))).collect()),
        other => other.clone(),
    }
}

/// A run as the fixture records it: every step without the marks of a kind in
/// `EXTRA_MARKS`, which the comparison never asks of the recording.
fn recorded(run: &Value) -> Value {
    let mut run = run.clone();
    if let Some(steps) = run["steps"].as_array_mut() {
        for step in steps {
            if let Some(marks) = step["marks"].as_array_mut() {
                marks.retain(|m| !EXTRA_MARKS.contains(&m["kind"].as_str().unwrap_or("")));
            }
        }
    }
    run
}

fn sorted_marks(step: &Value, drop_extra: bool) -> Vec<String> {
    let mut v: Vec<String> = step["marks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|m| !(drop_extra && EXTRA_MARKS.contains(&m["kind"].as_str().unwrap_or(""))))
        .map(|m| canonical(&masked(m)))
        .collect();
    v.sort();
    v
}

/// The recorded run against this one: step for step, every message to the
/// parent and every summarizer request byte for byte in canonical JSON, in
/// any order within its step (a multiset: the order is the hop depth of the
/// chains inside the hive, which a cut moves without changing behaviour),
/// every recorded mark again (likewise), and no other mark than one of
/// `EXTRA_MARKS`; both sides through `sub_ms`.
fn compare(name: &str, want: &Value, got: &Value) {
    let ws = want["steps"].as_array().cloned().unwrap_or_default();
    let gs = got["steps"].as_array().cloned().unwrap_or_default();
    assert_eq!(ws.len(), gs.len(), "{name}: the number of steps");
    for (w, g) in ws.iter().zip(gs.iter()) {
        let label = w["step"].as_str().unwrap_or("");
        assert_eq!(w["step"], g["step"], "{name}: the step");
        for part in ["out", "summarizer"] {
            let mut a: Vec<String> = w[part]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| canonical(&masked(x)))
                .collect();
            let mut b: Vec<String> = g[part]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| canonical(&masked(x)))
                .collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{name}: {label}: {part}");
        }
        assert_eq!(
            sorted_marks(w, false),
            sorted_marks(g, true),
            "{name}: {label}: marks"
        );
    }
}

fn talky(name: &str, knobs: Knobs, channel: Channel, own: &[(&str, &str, Value)]) -> Run {
    Run::new(name, Composite::Talky, knobs, channel, own)
}

fn stamp(needs: &str, families: &str) -> Value {
    json!({"needs": needs, "families": families})
}

// ---- the ways, one scenario each; the cases below run them per channel and knobs.

/// quick: a `simple` turn with a family, answered by the fast model; then a
/// `talk` turn sees the quick round.
fn way_quick(mut r: Run) {
    r.say("t1", "talk", stamp("", ""), "Good morning!");
    r.reply_text("brain", "Good morning to you.");
    r.say("t2", "simple", stamp("", "web"), "What day is it today?");
    r.reply_text("quick", "It is Monday.");
    r.say("t3", "talk", stamp("", ""), "Thanks, that helps.");
    r.reply_text("brain", "Glad to help.");
    r.finish(&["quick", "brain"]);
}

/// brain: three `talk` turns, the window growing round by round.
fn way_brain(mut r: Run) {
    for (t, ask, answer) in [
        ("t1", "Hello there.", "Hello! How are you?"),
        ("t2", "I slept badly.", "I am sorry to hear that."),
        ("t3", "My sister visits today.", "That sounds lovely."),
    ] {
        r.say(t, "talk", stamp("", ""), ask);
        r.reply_text("brain", answer);
    }
    r.finish(&["brain"]);
}

/// consult: a `complex` turn goes to cogny with a bridge beside it; a turn
/// while it is pending; the advice comes back under its consult id.
fn way_consult(mut r: Run) {
    r.say("t0", "talk", stamp("", ""), "Hello.");
    r.reply_text("brain", "Hello!");
    r.say(
        "t1",
        "complex",
        stamp("reasoning", ""),
        "Plan my week around the move.",
    );
    let cid = r.errand_id("consult");
    r.say("t2", "talk", stamp("", ""), "And what about tomorrow?");
    r.reply_text("brain", "Tomorrow is still open.");
    r.say(
        "t3",
        "talk",
        json!({"consult_id": cid}),
        "The plan for the week.",
    );
    r.reply_text("brain", "Here is the plan for your week.");
    r.finish(&["consult", "quick", "brain"]);
}

/// research: the deep search leaves on `research`, bridged by the quick voice.
fn way_research(mut r: Run) {
    r.say("t0", "talk", stamp("", ""), "Hello.");
    r.reply_text("brain", "Hello!");
    r.say(
        "t1",
        "research",
        stamp("research", "web"),
        "Find everything on the choir.",
    );
    r.say("t2", "talk", stamp("", ""), "Is it done yet?");
    r.reply_text("brain", "Not yet, it takes a moment.");
    r.finish(&["research", "quick", "brain"]);
}

/// quick_spec kept: the early curate, the held answer, a `simple` stamp
/// without a tool keeps it.
fn way_spec_kept(mut r: Run) {
    r.say("t0", "talk", stamp("", ""), "Hello.");
    r.reply_text("brain", "Hello!");
    r.spec("t1", "How do you spell rhythm?");
    r.spec_answer("R-H-Y-T-H-M.");
    r.say("t1", "simple", stamp("", ""), "How do you spell rhythm?");
    r.finish(&["quick", "spec_release"]);
}

/// quick_spec dropped: the held answer called no tool, the stamp needs one;
/// the normal tool round runs instead.
fn way_spec_dropped(mut r: Run) {
    r.say("t0", "talk", stamp("", ""), "Hello.");
    r.reply_text("brain", "Hello!");
    r.spec("t1", "What is the weather in Berlin?");
    r.spec_answer("It is sunny.");
    r.say(
        "t1",
        "simple",
        stamp("tool", "weather"),
        "What is the weather in Berlin?",
    );
    r.reply(
        "quick",
        "tool_calls",
        usage(900),
        json!([tool_call("c1", "web_search")]),
    );
    r.tool_result(
        "t1",
        "1",
        "simple",
        stamp("", "weather"),
        "12 degrees, cloudy",
    );
    r.reply_text("quick", "It is 12 degrees and cloudy.");
    r.finish(&["quick"]);
}

/// clarify: the stamp says the words mean more than one thing; the brain
/// asks back, no tool, the held guess dropped.
fn way_clarify(mut r: Run) {
    r.say("t0", "talk", stamp("", ""), "Hello.");
    r.reply_text("brain", "Hello!");
    r.spec("t1", "The choir in Dresden, search please.");
    r.spec_answer("SPECGUESS the choir sings on Friday.");
    r.say(
        "t1",
        "clarify",
        stamp("", ""),
        "The choir in Dresden, search please.",
    );
    r.reply_text("brain", "Do you mean the concert or the rehearsal?");
    r.finish(&["brain"]);
}

/// tool iteration: a `simple` turn that needs a tool -- the call, the
/// result, the answer -- and the next turn after it.
fn way_tool(mut r: Run) {
    r.say(
        "t1",
        "simple",
        stamp("tool", "web"),
        "Who won the match yesterday?",
    );
    r.reply(
        "quick",
        "tool_calls",
        usage(900),
        json!([tool_call("c1", "web_search")]),
    );
    r.tool_result(
        "t1",
        "1",
        "simple",
        stamp("", "web"),
        "The home team won 2:1.",
    );
    r.reply_text("quick", "The home team won two to one.");
    r.say("t2", "talk", stamp("", ""), "Great news!");
    r.reply_text("brain", "It is!");
    r.finish(&["quick", "brain"]);
}

/// live feed: closed turns of a running session (phone, `live_feed` on) send
/// the live feed; a text session and the template (`live_feed` off) send
/// none -- the same steps record that. Before the last turn the memory names
/// a topic (a `topic` mark in the ledger, the form OR-KY-71 writes), so the
/// feed after it carries a moved fact, not only the empty opening.
fn way_live(r: Run) {
    live_steps(r, &["brain", "quick"]);
}

/// [`way_live`] where the feed must leave: on `live_feed`, with the named
/// topic in its body.
fn way_live_fed(r: Run) {
    live_steps(r, &["brain", "quick", "live_feed"]);
}

fn live_steps(mut r: Run, ways: &[&str]) {
    r.say("t1", "talk", stamp("", ""), "Hello, it is me.");
    r.reply_text("brain", "Hello! Nice to hear you.");
    r.say(
        "t2",
        "simple",
        stamp("tool", "weather"),
        "Will it rain today?",
    );
    r.reply(
        "quick",
        "tool_calls",
        usage(900),
        json!([tool_call("c1", "web_search")]),
    );
    r.tool_result("t2", "1", "simple", stamp("", "weather"), "Rain from noon.");
    r.reply_text("quick", "Yes, from noon on.");
    r.name_topic("t2", "the rain today");
    r.say("t3", "talk", stamp("", ""), "Then I stay in.");
    r.reply_text("brain", "A good plan.");
    if ways.contains(&"live_feed") {
        let topics: Vec<&Value> = r
            .steps
            .iter()
            .flat_map(|s| s["out"].as_array().into_iter().flatten())
            .filter(|o| o["route"] == "live_feed")
            .map(|o| &o["body"]["topic"])
            .collect();
        assert!(
            topics.iter().any(|t| *t == "the rain today"),
            "{}: the feed carries the named topic: {topics:?}",
            r.name
        );
    }
    r.finish(ways);
}

/// summarize / rebuild: rounds over a small window, a tap over the target
/// orders the rebuild, the strike condenses the old rounds through the
/// summarizer, and the next turn gets the rebuilt window.
fn way_rebuild(mut r: Run, route: &str) {
    let long = |i: usize| {
        format!(
            "Round {i}: {}",
            "we talked about the garden and the weather. ".repeat(40)
        )
    };
    for i in 1..=4 {
        let t = format!("t{i}");
        r.say(&t, "talk", stamp("", ""), &long(i));
        r.reply(
            route,
            "stop",
            usage(1500 * i as i64),
            json!([said(&long(i + 10))]),
        );
        if r.last_adds > 0 {
            r.fire();
        }
    }
    r.say("t5", "talk", stamp("", ""), "And now?");
    r.reply(route, "stop", usage(9000), json!([said("Now we rest.")]));
    if r.last_adds > 0 {
        r.fire();
    }
    r.say("t6", "talk", stamp("", ""), "What did we talk about?");
    r.reply(
        route,
        "stop",
        usage(1200),
        json!([said("About the garden.")]),
    );
    r.finish(&[route, "summarize"]);
}

/// The knobs a rebuild case adds: a small window so four rounds overflow it.
const REBUILD: [(&str, &str, i64); 3] = [
    ("policy", "window_target", 3000),
    ("policy", "keep_recent_tokens", 600),
    ("policy", "keep_recent", 1),
];

fn rebuild_knobs() -> Vec<(&'static str, &'static str, Value)> {
    REBUILD.iter().map(|(c, k, v)| (*c, *k, json!(v))).collect()
}

/// A turn without a stamp keeps the road of before (`brain`).
fn way_unstamped(mut r: Run) {
    r.say("t1", "", stamp("", ""), "Hello, anyone there?");
    r.reply_text("brain", "Yes, I am here.");
    r.say("t2", "", stamp("", ""), "Good.");
    r.reply_text("brain", "Good.");
    r.finish(&["brain"]);
}

const SPEC: [(&str, &str, &str); 2] = [
    ("policy", "quick_speculative", "1"),
    ("policy", "spec_tools", ""),
];

fn spec_knobs() -> Vec<(&'static str, &'static str, Value)> {
    SPEC.iter().map(|(c, k, v)| (*c, *k, json!(v))).collect()
}

macro_rules! cases {
    ($($name:ident: $way:ident, $knobs:ident, $channel:ident, $own:expr;)*) => {
        /// The names of the cases of `cases!`.
        const CASES: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() {
                if !shipped() {
                    return;
                }
                $way(talky(stringify!($name), Knobs::$knobs, Channel::$channel, &$own));
            }
        )*
    };
}

cases! {
    quick_text_template: way_quick, Template, Text, [];
    quick_text_set056: way_quick, Set056, Text, [];
    quick_phone_template: way_quick, Template, Phone, [];
    quick_phone_set056: way_quick, Set056, Phone, [];
    brain_text_template: way_brain, Template, Text, [];
    brain_text_set056: way_brain, Set056, Text, [];
    brain_phone_template: way_brain, Template, Phone, [];
    brain_phone_set056: way_brain, Set056, Phone, [];
    consult_text_template: way_consult, Template, Text, [];
    consult_text_set056: way_consult, Set056, Text, [];
    consult_phone_template: way_consult, Template, Phone, [];
    consult_phone_set056: way_consult, Set056, Phone, [];
    research_text_template: way_research, Template, Text, [];
    research_text_set056: way_research, Set056, Text, [];
    research_phone_template: way_research, Template, Phone, [];
    research_phone_set056: way_research, Set056, Phone, [];
    spec_kept_text_template: way_spec_kept, Template, Text, spec_knobs();
    spec_kept_text_set056: way_spec_kept, Set056, Text, spec_knobs();
    spec_kept_phone_template: way_spec_kept, Template, Phone, spec_knobs();
    spec_kept_phone_set056: way_spec_kept, Set056, Phone, spec_knobs();
    spec_dropped_text_template: way_spec_dropped, Template, Text, spec_knobs();
    spec_dropped_text_set056: way_spec_dropped, Set056, Text, spec_knobs();
    spec_dropped_phone_template: way_spec_dropped, Template, Phone, spec_knobs();
    spec_dropped_phone_set056: way_spec_dropped, Set056, Phone, spec_knobs();
    clarify_text_template: way_clarify, Template, Text, spec_knobs();
    clarify_text_set056: way_clarify, Set056, Text, spec_knobs();
    clarify_phone_template: way_clarify, Template, Phone, spec_knobs();
    clarify_phone_set056: way_clarify, Set056, Phone, spec_knobs();
    tool_text_template: way_tool, Template, Text, [];
    tool_text_set056: way_tool, Set056, Text, [];
    tool_phone_template: way_tool, Template, Phone, [];
    tool_phone_set056: way_tool, Set056, Phone, [];
    live_text_set056: way_live, Set056, Text, [];
    live_phone_template: way_live, Template, Phone, [];
    live_phone_set056: way_live_fed, Set056, Phone, [];
    unstamped_text_template: way_unstamped, Template, Text, [];
    unstamped_phone_set056: way_unstamped, Set056, Phone, [];
}

macro_rules! rebuild_cases {
    ($($name:ident: $composite:ident, $knobs:ident, $channel:ident, $route:expr;)*) => {
        /// The names of the cases of `rebuild_cases!`.
        const REBUILD_CASES: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() {
                if !shipped() {
                    return;
                }
                let r = Run::new(stringify!($name), Composite::$composite, Knobs::$knobs,
                                 Channel::$channel, &rebuild_knobs());
                way_rebuild(r, $route);
            }
        )*
    };
}

rebuild_cases! {
    rebuild_text_template: Talky, Template, Text, "brain";
    rebuild_text_set056: Talky, Set056, Text, "brain";
    rebuild_phone_template: Talky, Template, Phone, "brain";
    rebuild_phone_set056: Talky, Set056, Phone, "brain";
    rebuild_cogny_template: Cogny, Template, Text, "brain";
}

/// The cases written out below the macros.
const COGNY_CASES: &[&str] = &["cogny_brain_text_template", "cogny_tool_text_template"];

/// cogny's curator (role consult): the errand of a turn and its answer, the
/// next turn after it.
#[test]
fn cogny_brain_text_template() {
    if !shipped() {
        return;
    }
    let mut r = Run::new(
        "cogny_brain_text_template",
        Composite::Cogny,
        Knobs::Template,
        Channel::Text,
        &[],
    );
    r.say("t1", "", stamp("", ""), "Plan the move for the person.");
    r.reply_text("brain", "Step one: boxes. Step two: the van.");
    r.say("t2", "", stamp("", ""), "And the costs?");
    r.reply_text("brain", "About 800 euros.");
    r.finish(&["brain"]);
}

/// cogny's tool round: the call, the result, the answer.
#[test]
fn cogny_tool_text_template() {
    if !shipped() {
        return;
    }
    let mut r = Run::new(
        "cogny_tool_text_template",
        Composite::Cogny,
        Knobs::Template,
        Channel::Text,
        &[],
    );
    r.say("t1", "", stamp("", ""), "Find a moving company in Berlin.");
    r.reply(
        "brain",
        "tool_calls",
        usage(900),
        json!([tool_call("c1", "web_search")]),
    );
    r.tool_result("t1", "1", "", stamp("", ""), "Three companies found.");
    r.reply_text("brain", "I found three companies.");
    r.finish(&["brain"]);
}

/// The normalisation takes the microseconds and nothing else: the
/// millisecond, a thirteen-digit millisecond stamp and other numbers stay.
#[test]
fn the_mask_takes_only_the_microseconds() {
    assert_eq!(
        sub_ms("since 2026-03-02T10:00:10.000033Z, rows up to 1772445605000010."),
        "since 2026-03-02T10:00:10.000uuuZ, rows up to 1772445605000uuu."
    );
    assert_eq!(
        sub_ms("at 1772445610000 ms, 10:00:10.123Z, 12345678901234567, 9:00:00.000001Z"),
        "at 1772445610000 ms, 10:00:10.123Z, 12345678901234567, 9:00:00.000001Z"
    );
}

/// Every fixture belongs to a case: a file without its test is a case the
/// corpus lost without anyone seeing it (a renamed or deleted test).
#[test]
fn every_fixture_has_its_case() {
    let dir = repo(FIXTURES);
    if !dir.is_dir() {
        return;
    }
    let cases: std::collections::BTreeSet<String> = CASES
        .iter()
        .chain(REBUILD_CASES)
        .chain(COGNY_CASES)
        .map(|c| format!("{c}.json"))
        .collect();
    let orphans: Vec<String> = std::fs::read_dir(&dir)
        .expect("the fixture dir")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|f| !cases.contains(f))
        .collect();
    assert!(orphans.is_empty(), "fixtures without a case: {orphans:?}");
}

/// The comparison takes the messages of a step as a multiset: a recorded
/// step with two of its messages to the parent swapped is the same step, one
/// changed byte in a body is not.
#[test]
fn the_order_within_a_step_is_free_the_bytes_are_not() {
    let path = repo(&format!("{FIXTURES}/spec_kept_text_template.json"));
    if !path.is_file() {
        return;
    }
    let want = read_json(&path);
    let at = want["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .position(|s| s["out"].as_array().is_some_and(|o| o.len() >= 2))
        .expect("a step with two messages to the parent");
    let mut swapped = want.clone();
    swapped["steps"][at]["out"]
        .as_array_mut()
        .expect("out")
        .swap(0, 1);
    assert_ne!(swapped, want, "the swap moved something");
    compare("swapped", &want, &swapped);
    let mut changed = want.clone();
    let body = canonical(&changed["steps"][at]["out"][0]["body"]);
    let one_byte = body.replacen('a', "b", 1);
    assert_ne!(body, one_byte, "the body has a byte to change");
    changed["steps"][at]["out"][0]["body"] = sj::from_str(&one_byte).expect("json");
    let red = std::panic::catch_unwind(|| compare("changed", &want, &changed));
    assert!(red.is_err(), "one changed byte is red");
}
