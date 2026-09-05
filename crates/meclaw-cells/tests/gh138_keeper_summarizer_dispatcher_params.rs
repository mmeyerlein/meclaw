//! GH #138 -- the keeper, the summarizer and the dispatcher take their knobs
//! from `params`, not from the environment.
//!
//! Until `session-keeper@2.1.0`, `summarizer@2.0.2` and `dispatcher@1.1.2` ten
//! behaviour knobs of these three templates were `${KEEPER_*}` /
//! `${SUMMARIZER_*}` / `${DISPATCHER_*}` substitution tokens: colony-global by
//! construction, so two agents in one colony could not have their idle window,
//! their recency weighting or their call budget tuned apart, and every knob NAME
//! was a public configuration contract that could only be renamed by breaking a
//! running colony. Ruling R-0904-6 moves them onto the `params` surface of the
//! cells that read them, defaults bit-identical.
//!
//! `crates/meclaw-cells/tests/w13_collector_params.rs` is the pattern (GH #136,
//! `collector@1.2.0`) and `gh138_memory_hive_params.rs` is the same form one
//! strand earlier; this file is the same three claims for the three templates
//! whose knobs were written into a `.env` by thirty-nine test setups:
//!
//! 1. **THE ENVIRONMENT ROUTE IS GONE FOR BEHAVIOUR.** Not "deprecated", not
//!    "fallback": the only `${...}` tokens left anywhere under
//!    `templates/session-keeper/`, `templates/summarizer/` and
//!    `templates/dispatcher/` are the PROVIDER LANE, and they are named here one
//!    by one. A clean cut is only clean if nothing reads the old surface.
//! 2. **ONE VALUE, THREE PLACES, NO DRIFT.** A knob's default exists as a literal
//!    in the script, as a value under `params`, and as
//!    `contract.settings.<knob>.default`; all three are compared per knob.
//! 3. **THE BEHAVIOUR COMES FROM THE PARAM.** Each knob class is measured twice
//!    against the SHIPPED script: once with no override, where the shipped
//!    default must stand, and once with one, where the override must be what the
//!    cell acts on. That is the assertion "the tests are green" cannot make.
//!
//! The third claim is why this file exists at all. A `.env` line that stops
//! meaning anything is SILENT: a `KEEPER_NIGHT_CRON` line in eighteen test setups
//! pushed the nightly close sweep out of a run's way, and after the migration
//! such a line would be read by nothing -- the night would fire into those runs
//! and nobody would say so, as a flake rather than as a red assert (research note
//! E § 2.5b, open point 5). Every one of those lines is an `override_params`
//! entry now, and the two halves below are the proof that the new form is what
//! the cells actually act on.

use meclaw_cells::timer::params::TimerParams;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn templates() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// One cell's shipped `config.json`, addressed the way the tree spells it:
/// `session-keeper/close`, `summarizer/prep`, `dispatcher` (a single-cell
/// template, whose cell IS the template root).
fn config(cell: &str) -> Value {
    let path = templates().join(cell).join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ───────────────────────────────────────────────────────────── the inventory

/// Every knob of one scripted cell, with the accessor its script reads it
/// through. Restated here on purpose: this is the inventory the migration
/// claims to be complete, and a knob that quietly leaves a config fails here.
struct Scripted {
    cell: &'static str,
    knobs: &'static [(&'static str, &'static str)],
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        cell: "session-keeper/close",
        knobs: &[("idle_ms", "_int"), ("close_limit", "_int")],
    },
    Scripted {
        cell: "summarizer/prep",
        knobs: &[
            ("recent_turns", "_int"),
            ("phaseout_chars", "_int"),
            ("tool_chars", "_int"),
            ("round_lines", "_int"),
        ],
    },
    Scripted {
        cell: "dispatcher",
        knobs: &[
            ("max_calls", "_int"),
            ("async_tools", "_list"),
            ("handoff_tools", "_list"),
            // `interim` reached the params surface a wave early (GH #539) and is
            // listed here because it is the same claim: one value, three places.
            // This migration only made it read through the same accessor as its
            // three neighbours instead of an open-coded `P.get`.
            ("interim", "_str"),
        ],
    },
];

/// The `code` cell's own param surface (`CodeParams::parse`), kept complete on
/// purpose: a substrate param this list forgets shows up below as a phantom
/// "undeclared knob", which is a false accusation against the template.
const SUBSTRATE_CODE_PARAMS: &[&str] = &[
    "runner",
    "script_path",
    "script_inline",
    "external_timeout_ms",
    "max_concurrency",
    "sandbox",
    "runner_mode",
];

/// The provider lane, name by name -- the ONLY `${...}` tokens these three
/// templates may still carry after the migration (ruling R-0904-6; the gate reads
/// the same classes off the name in `scripts/check_tree_rules.py` § R6).
///
/// One entry, and it is the summarizer's writer: an `llm` cell needs a key.
const ENV_LANE: &[&str] = &["OPENROUTER_API_KEY"];

/// The three template roots this file rules over.
const ROOTS: &[&str] = &["session-keeper", "summarizer", "dispatcher"];

// ─────────────────────────────────────────────────────────────── the harness

/// Hand a program to python3 **on stdin**, never in argv: a single argv string is
/// capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped scripts have grown to
/// within a few KB of that line (GH #279, GH #349).
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run one cell's SHIPPED script over a real stdin document whose `params` object
/// is `params`, and return the messages it emits.
///
/// No substitution step: after the migration there is nothing left in these
/// scripts to substitute, and a harness that still resolved `${...}` would pass
/// for a template that kept a token.
fn emit_with_params(cell: &str, params: Value, doc: Value) -> Vec<Value> {
    let script = config(cell)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    assert!(
        !script.contains("${"),
        "{cell}: a substitution token survived in the shipped script"
    );
    let mut flat = doc;
    flat["params"] = params;
    let stdin_doc = meclaw_testing::code_stdin(&flat).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(&script).unwrap(),
        meclaw_core::serde_json::to_string(&stdin_doc).unwrap(),
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell}: output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Every `${NAME}` / `${NAME:-default}` a value carries, `contract` skipped -- a
/// settings description may still name a variable in prose.
fn substitutions(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                let tail = &rest[start + 2..];
                let Some(end) = tail.find('}') else { return };
                let inner = &tail[..end];
                if !inner.starts_with("uuid7:") && !inner.starts_with("ctx.") {
                    out.push(inner.split(":-").next().unwrap_or(inner).to_string());
                }
                rest = &tail[end + 1..];
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key == "contract" {
                    continue;
                }
                substitutions(item, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                substitutions(item, out);
            }
        }
        _ => {}
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The old surface is not deprecated, it is absent: every token left in
/// the three templates is a provider-lane name, and the sweep is over the whole
/// subtree so a cell nobody was thinking about cannot keep one.
#[test]
fn nothing_in_the_shipped_three_reads_a_behaviour_knob_out_of_the_environment() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                out.push(p);
            }
        }
    }

    let root = templates();
    let mut files = Vec::new();
    for t in ROOTS {
        walk(&root.join(t), &mut files);
    }
    assert!(
        files.len() >= 10,
        "the sweep found {} config(s) under the three templates -- it swept \
         almost nothing and would have passed for a tree with a token in every \
         cell",
        files.len()
    );

    let mut strays: Vec<String> = Vec::new();
    for p in &files {
        let raw = std::fs::read_to_string(p).expect("config");
        let val: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
        let mut names = Vec::new();
        substitutions(&val, &mut names);
        for name in names {
            if !ENV_LANE.contains(&name.as_str()) {
                let rel = p.strip_prefix(&root).unwrap_or(p).display();
                strays.push(format!("  {rel}: ${{{name}}}"));
            }
        }
    }
    assert!(
        strays.is_empty(),
        "{} behaviour knob(s) still read out of the environment (GH #138):\n{}",
        strays.len(),
        strays.join("\n")
    );
}

/// Claim 2. Every knob exists in all three places, with the same value.
///
/// The script literal is read out of the source text rather than exercised,
/// because that literal IS the fallback: `_int("idle_ms", …)` is the value a cell
/// uses when its config says nothing, and comparing the text is the complete
/// check over all ten knobs.
#[test]
fn every_knob_is_a_param_a_setting_and_a_script_literal_with_one_value() {
    let mut total = 0usize;
    for group in SCRIPTED {
        let cfg = config(group.cell);
        let params = cfg["params"].as_object().expect("params");
        let settings = cfg["contract"]["settings"]
            .as_object()
            .expect("contract.settings");
        let src = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline");

        for (knob, kind) in group.knobs {
            total += 1;
            let cell = group.cell;
            let param = params
                .get(*knob)
                .unwrap_or_else(|| panic!("{cell}: params.{knob} is missing"));
            let default = settings
                .get(*knob)
                .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob} is missing"))
                .get("default")
                .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob}.default is missing"));
            assert_eq!(
                param, default,
                "{cell}: params.{knob} and contract.settings.{knob}.default disagree"
            );

            // `NAME = _int("close_limit", 50)` -- the literal after the comma.
            let needle = format!("{kind}(\"{knob}\", ");
            let at = src
                .find(&needle)
                .unwrap_or_else(|| panic!("{cell}: the script does not read {knob} with {kind}"));
            let rest = &src[at + needle.len()..];
            let lit = &rest[..rest.find(')').expect("closing paren")];
            let lit: Value = meclaw_core::serde_json::from_str(lit).unwrap_or_else(|e| {
                panic!("{cell}/{knob}: script literal {lit:?} is not json ({e})")
            });
            assert_eq!(
                lit, *param,
                "{cell}: the script's own fallback for {knob} drifted from the shipped param"
            );
        }

        // No knob may hide: every non-substrate param is one of the declared ones.
        for key in params.keys() {
            assert!(
                SUBSTRATE_CODE_PARAMS.contains(&key.as_str())
                    || group.knobs.iter().any(|(k, _)| k == key),
                "{}: params.{key} is neither a substrate param nor a declared knob",
                group.cell
            );
        }
        assert_eq!(
            settings.len(),
            group.knobs.len(),
            "{}: contract.settings and the knob inventory disagree in size",
            group.cell
        );
    }
    assert_eq!(
        total, 10,
        "the scripted half of this migration is ten knobs"
    );
}

// ══════════════════════════════════════════ claim 3 -- one proof per knob class

/// A firing as the night timer delivers it: the auto headers of the schedule, no
/// context at all (an `emit_to` message is minted, not routed).
fn firing() -> Value {
    json!({
        "header": {"context": {},
                   "hop": {"event_id": "e1", "schedule_id": "s1",
                           "schedule_name": "night-close",
                           "scheduled_at": "2026-09-04T22:00:00Z",
                           "fired_at": "2026-09-04T22:00:00Z", "iteration_n": 0}},
        "messages": [{"origin": "user", "type": "text", "text": "night-close"}]
    })
}

/// The store args of an emitted `kstore` message.
fn op_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    meclaw_core::serde_json::from_str(text).expect("op json")
}

fn seconds_back(cutoff: &str) -> i64 {
    let parsed = chrono::DateTime::parse_from_rfc3339(cutoff)
        .unwrap_or_else(|e| panic!("cutoff {cutoff} is not RFC-3339: {e}"));
    (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_seconds()
}

/// The keeper's idle window and its runaway guard. Negative half: with no
/// override the shipped two hours and the shipped fifty are what the sweep asks
/// the store for. Positive half: an `override_params` entry moves both, and the
/// question the store is asked changes with it.
///
/// This is the pin under the seventeen setups that used to write a
/// `KEEPER_IDLE_MS` line into a `.env` to make every open generation a candidate.
#[test]
fn the_idle_window_and_the_close_limit_come_from_the_params() {
    let shipped = emit_with_params("session-keeper/close", json!({}), firing());
    assert_eq!(shipped.len(), 1, "one question, asked of the store");
    let op = op_of(&shipped[0]);
    assert_eq!(op["limit"], 50, "the shipped runaway guard is fifty");
    let back = seconds_back(op["where"]["last_seen"]["lt"].as_str().expect("lt cutoff"));
    assert!(
        (7100..=7300).contains(&back),
        "the shipped idle window is two hours, got {back}s back"
    );

    let tuned = emit_with_params(
        "session-keeper/close",
        json!({"idle_ms": 600000, "close_limit": 7}),
        firing(),
    );
    let op = op_of(&tuned[0]);
    assert_eq!(
        op["limit"], 7,
        "the guard an override names is not the one the sweep asks for"
    );
    let back = seconds_back(op["where"]["last_seen"]["lt"].as_str().expect("lt cutoff"));
    assert!(
        (500..=700).contains(&back),
        "the window an override names is not the one the sweep cuts on: {back}s back"
    );

    // And zero is a VALUE, not "unset": it is what makes every open generation a
    // candidate, which is exactly what the colony tests need of it.
    let now = emit_with_params("session-keeper/close", json!({"idle_ms": 0}), firing());
    let back = seconds_back(
        op_of(&now[0])["where"]["last_seen"]["lt"]
            .as_str()
            .expect("lt cutoff"),
    );
    assert!(
        (-1..=1).contains(&back),
        "an idle window of zero must cut at NOW, got {back}s back"
    );
}

/// GH #138 for the night: the sweep's schedule is a literal the timer's own
/// `params` carry, and an override is what the REAL parser plans on.
///
/// Negative half first: with no override the shipped cron is what
/// `TimerParams::parse` plans, and it is the same string the declaration carries
/// -- a timer has no `night_cron` param, so the declaration documents a value
/// that lives inside `params.schedules[0].cron` and the two are compared here
/// rather than typed twice. Then the positive half, which is the one that
/// matters: the schedule a mutation pushes down through `override_params` is the
/// one the timer plans on -- so the eighteen setups that used to write a
/// `KEEPER_NIGHT_CRON` line into a `.env` still move the nightly sweep out of
/// their way. A `.env` line that stopped meaning anything would have been silent:
/// green tests with a close sweep firing into them.
#[test]
fn the_night_fires_on_the_cron_its_params_carry() {
    let mut cfg = config("session-keeper/night");
    let declared = cfg["contract"]["settings"]["night_cron"]["default"]
        .as_str()
        .expect("the declared nightly cron")
        .to_string();
    assert_eq!(
        declared.split_whitespace().count(),
        6,
        "the declared schedule is a six-field Quartz cron: {declared}"
    );
    let params = cfg["params"].as_object_mut().expect("params");
    // The one substitution staging performs on this file: `${uuid7:...}` is not a
    // UUID until the colony mints one.
    params["schedules"][0]["schedule_id"] =
        Value::String("0190a3f2-0000-7000-8000-000000000138".into());

    let shipped = TimerParams::parse(&Value::Object(params.clone())).expect("shipped params");
    assert_eq!(shipped.schedules.len(), 1, "the keeper ticks one night");
    let ScheduleKind::Cron(shipped_cron) = &shipped.schedules[0].kind else {
        panic!("the nightly schedule is a repeating cron, not a one-shot `at`")
    };
    assert_eq!(
        *shipped_cron, declared,
        "the shipped nightly schedule is a literal and it is the declared one"
    );

    // What a mutation writes: `override_params` replaces the whole `schedules`
    // key (last-write-wins, `mutation::stage::patch_and_substitute_config`), and
    // GH #294 lets it name that key precisely because it EXISTS under `params`.
    let mut overridden = params.clone();
    overridden["schedules"][0]["cron"] = Value::String("0 0 4 1 1 *".into());
    let tuned = TimerParams::parse(&Value::Object(overridden)).expect("overridden params");
    let ScheduleKind::Cron(tuned_cron) = &tuned.schedules[0].kind else {
        panic!("the overridden schedule stopped being a cron")
    };
    assert_eq!(
        tuned_cron, "0 0 4 1 1 *",
        "the schedule an override names is not the one the timer plans on"
    );
}

fn turn(origin: &str, text: &str) -> Value {
    json!({"origin": origin, "type": "text", "text": text})
}

/// The write batch as the collector's close lane emits it.
fn batch_doc(turns: Vec<Value>) -> Value {
    json!({
        "header": {"context": {},
                   "hop": {"route": "in_batch", "session_id": "s1",
                           "turn_count": turns.len().to_string(), "round_count": "0"}},
        "messages": turns,
        "rounds": []
    })
}

/// The summarizer's recency weighting. Negative half: the shipped twelve keeps
/// four turns verbatim, so nothing is condensed. Positive half: an override of
/// two cuts the two oldest to a preview and COUNTS them, which is the whole
/// behaviour of the knob.
#[test]
fn the_recency_cut_comes_from_the_params() {
    let day = || {
        vec![
            turn("user", "alpha-0123456789-ALPHA-TAIL"),
            turn("assistant", "beta-0123456789-BETA-TAIL"),
            turn("user", "gamma-recent"),
            turn("assistant", "delta-recent"),
        ]
    };
    let prompt_of = |out: &[Value]| -> String {
        assert_eq!(out.len(), 1, "one batch, one prompt");
        out[0]["messages"][0]["text"]
            .as_str()
            .expect("prompt text")
            .to_string()
    };

    let shipped = prompt_of(&emit_with_params(
        "summarizer/prep",
        json!({}),
        batch_doc(day()),
    ));
    assert!(
        shipped.contains("ALPHA-TAIL"),
        "under the shipped twelve, four turns travel whole: {shipped}"
    );
    assert!(
        !shipped.contains("older turn"),
        "and nothing is phased out: {shipped}"
    );

    let tuned = prompt_of(&emit_with_params(
        "summarizer/prep",
        json!({"recent_turns": 2, "phaseout_chars": 10}),
        batch_doc(day()),
    ));
    assert!(
        tuned.contains("gamma-recent") && tuned.contains("delta-recent"),
        "the newest two still travel whole: {tuned}"
    );
    assert!(
        tuned.contains("alpha-0123") && !tuned.contains("ALPHA-TAIL"),
        "the older two are cut to the preview the param names: {tuned}"
    );
    assert!(
        tuned.contains("2 older turn"),
        "what was condensed is counted, not hidden: {tuned}"
    );
}

fn call_turn(id: &str, name: &str, args: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": args}).to_string()})
}

fn bundle(calls: Vec<Value>) -> Value {
    json!({"header": {"context": {}, "hop": {"finish_reason": "tool_calls"}},
           "messages": calls})
}

fn route_of(msg: &Value) -> &str {
    msg["header"]["route"].as_str().unwrap_or_default()
}

/// The dispatcher's call budget and its two tool classes. Negative half: with no
/// override a three-call bundle runs and no call is classified. Positive half: an
/// override of two refuses the whole bundle with synthetic results, and an
/// override naming a tool marks exactly that call.
///
/// This is the pin under the setups that used to write `DISPATCHER_MAX_CALLS`,
/// `DISPATCHER_ASYNC_TOOLS` and `DISPATCHER_HANDOFF_TOOLS` into a `.env`.
#[test]
fn the_call_budget_and_the_tool_classes_come_from_the_params() {
    let three = || {
        vec![
            call_turn("c1", "consult_cogny", "{}"),
            call_turn("c2", "web_search", "{}"),
            call_turn("c3", "web_fetch", "{}"),
        ]
    };

    let shipped = emit_with_params("dispatcher", json!({}), bundle(three()));
    assert_eq!(
        shipped.len(),
        4,
        "under the shipped sixteen a three-call bundle runs: {shipped:?}"
    );
    assert!(
        shipped[1..].iter().all(|m| route_of(m) == "tool"),
        "every call leaves as a tool call: {shipped:?}"
    );
    assert_eq!(
        shipped[0]["header"]["async_calls"], "",
        "nothing is async until a param says so: {shipped:?}"
    );
    assert_eq!(shipped[0]["header"]["handoff_calls"], "");

    let capped = emit_with_params("dispatcher", json!({"max_calls": 2}), bundle(three()));
    assert_eq!(
        capped.len(),
        4,
        "one over the cap: the whole bundle is refused: {capped:?}"
    );
    assert!(
        capped[1..].iter().all(|m| route_of(m) == "result"),
        "the budget an override names is not the one the cell counts to: {capped:?}"
    );

    let classed = emit_with_params(
        "dispatcher",
        json!({"async_tools": "consult_cogny", "handoff_tools": "web_fetch"}),
        bundle(three()),
    );
    assert_eq!(
        classed[0]["header"]["async_calls"], "c1,c3",
        "a handoff declares itself async, and the class comes from the param: {classed:?}"
    );
    assert_eq!(
        classed[0]["header"]["handoff_calls"], "c3",
        "and only the handoff takes the turn with it: {classed:?}"
    );

    // The same declaration as a JSON array, which is what a manifest writes when
    // it names more than one tool: `override_params` reaches this knob as a
    // config value, not as a line somebody types into a shell.
    let listed = emit_with_params(
        "dispatcher",
        json!({"async_tools": ["consult_cogny", "web_search"]}),
        bundle(three()),
    );
    assert_eq!(listed[0]["header"]["async_calls"], "c1,c2");
}

/// A knob blanked by an operator means "not configured", not a dead cell -- and a
/// number may arrive as a string, which is what an operator typing a config line
/// writes.
#[test]
fn a_blank_knob_falls_back_and_a_string_number_is_read() {
    for blank in [json!(null), json!(""), json!("   ")] {
        let out = emit_with_params(
            "session-keeper/close",
            json!({"close_limit": blank}),
            firing(),
        );
        assert_eq!(
            op_of(&out[0])["limit"],
            50,
            "a close_limit of {blank} must fall back to the shipped default"
        );
    }
    let out = emit_with_params(
        "session-keeper/close",
        json!({"close_limit": "7"}),
        firing(),
    );
    assert_eq!(
        op_of(&out[0])["limit"],
        7,
        "a numeric knob may arrive as a string"
    );

    // The `_list` half: a blank declaration is the empty class, which is the
    // shipped default and the reason a dispatcher without a consult lane beside
    // it is silent rather than noisy.
    for blank in [json!(null), json!(""), json!([])] {
        let out = emit_with_params(
            "dispatcher",
            json!({"async_tools": blank}),
            bundle(vec![call_turn("c1", "consult_cogny", "{}")]),
        );
        assert_eq!(
            out[0]["header"]["async_calls"], "",
            "a blank async_tools declares nothing: {blank}"
        );
    }
}

/// Claim 3 in its plainest form: one shipped script, two `params` objects, two
/// behaviours, in the same process. That is the property the environment form
/// could not have, and it is what an `override_params` entry buys (GH #294: only
/// a key the cell carries under `params` may be named at all).
#[test]
fn two_instances_of_the_same_close_script_are_tuned_apart() {
    let one = emit_with_params("session-keeper/close", json!({"close_limit": 3}), firing());
    let other = emit_with_params("session-keeper/close", json!({"close_limit": 90}), firing());
    assert_eq!(op_of(&one[0])["limit"], 3);
    assert_eq!(op_of(&other[0])["limit"], 90);
}

/// The declarations carry no leftover env NAME in their prose either. A
/// description that still said "(KEEPER_IDLE_MS)" would send the next operator to
/// a file nothing reads any more -- and a `${...}` inside one is worse than stale
/// prose: `substitute_full` walks contracts too, so an instantiation would fail
/// on a missing variable (measured in this wave, task 11).
#[test]
fn no_settings_description_still_points_at_the_environment() {
    let mut strays = Vec::new();
    for cell in [
        "session-keeper/close",
        "session-keeper/night",
        "summarizer/prep",
        "dispatcher",
    ] {
        let cfg = config(cell);
        let Some(settings) = cfg["contract"]["settings"].as_object() else {
            continue;
        };
        for (knob, decl) in settings {
            let text = decl["description"].as_str().unwrap_or_default();
            for name in ["KEEPER_", "SUMMARIZER_", "DISPATCHER_"] {
                assert!(
                    !text.contains(name),
                    "{cell}: settings.{knob} still names {name}*"
                );
            }
        }
        let mut names = Vec::new();
        substitutions(&cfg["contract"], &mut names);
        for name in names {
            if !ENV_LANE.contains(&name.as_str()) {
                strays.push(format!("  {cell}: contract carries ${{{name}}}"));
            }
        }
    }
    assert!(
        strays.is_empty(),
        "a contract still carries a behaviour token (GH #138):\n{}",
        strays.join("\n")
    );
}
