//! Wave 13 -- the collector's knobs are params, not environment (GH #136).
//!
//! Until `collector@1.1.0` every knob was a `${COLLECTOR_*}` substitution
//! token: colony-global by construction, so two collectors in one colony could
//! not be tuned apart, and every knob NAME was a public config contract that
//! could only be renamed by breaking existing colonies. Since 1.2.0 they are
//! params of `./assemble`, read off the `params` object `build_stdin_json`
//! always puts on a `code` cell's stdin.
//!
//! Three claims are pinned here:
//!
//! 1. **THE ENVIRONMENT ROUTE IS GONE.** Not "deprecated", not "fallback":
//!    there is no `${...}` token left in the shipped script and no
//!    `COLLECTOR_` anywhere in the config. A clean cut is only clean if
//!    nothing reads the old surface.
//! 2. **ONE VALUE, THREE PLACES, NO DRIFT.** A knob's default now exists as a
//!    literal in the script, as a value under `params`, and as
//!    `contract.settings.<knob>.default`. Three copies is one more than
//!    before, so the pin below compares all three per knob -- a default moved
//!    in one place and forgotten in another fails here rather than in an
//!    instance.
//! 3. **TUNED APART.** The actual point of the issue: the same shipped script,
//!    two different `params` objects, two different behaviours in the same
//!    process. That is the property the environment form could not have.

use std::io::Write;
use std::process::{Command, Stdio};

const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";

/// The twenty-six knobs, with the kind of accessor the script reads each one with.
/// Restated here on purpose: this is the inventory the migration claims to be
/// complete, and a knob that quietly leaves the config should fail the pin.
const KNOBS: &[(&str, &str)] = &[
    ("window_turns", "_int"),
    ("window_bytes", "_int"),
    ("turn_chars", "_int"),
    ("tool_chars", "_int"),
    ("round_bytes", "_int"),
    ("memory_chars", "_int"),
    ("memory_tier", "_str"),
    ("memory_form", "_str"),
    ("memory_call_tier", "_str"),
    ("max_iter", "_int"),
    ("round_idle_ms", "_int"),
    ("prune_after_ms", "_int"),
    ("turn_write", "_str"),
    // GH #525 -- the inline extraction contract. It sits beside `turn_write`
    // because the two are one sentence: this one asks the brain for the
    // annotation, that one mints the episode the annotation is bound to, and a
    // block whose turn is not yet an episode is rejected by design.
    ("inline_extraction", "_str"),
    ("context_window", "_int"),
    ("curate_soft", "_float"),
    ("curate_hard", "_float"),
    ("keep_rounds", "_int"),
    ("recoverability", "_str"),
    ("thread_recall", "_str"),
    ("thread_recall_budget", "_float"),
    // GH #451 -- the curator's widened contract. `tool_menu` is the one that
    // moves an OWNERSHIP: with it set, the collector writes `system.tools` and
    // the tool declarations become curatable; empty, the menu stays in the
    // brain and nothing about this cell changes.
    ("tool_menu", "_str"),
    ("tool_desc_chars", "_int"),
    ("curate_slot_chars", "_int"),
    ("curate_budget_line", "_str"),
    // GH #464 -- the DECLARATION. It is the only knob whose value is a list,
    // and the only one whose effect is a QUESTION rather than a number: the
    // names in it are what the menu tick asks a tools hive for. Empty is the
    // shipped default and asks nothing, which is why a collector that ships
    // without a tools hive beside it is silent rather than noisy.
    ("tools", "_list"),
];

fn config() -> serde_json::Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    serde_json::from_str(&raw).expect("config json")
}

fn script() -> String {
    config()["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The shipped `params`, minus the script's own source -- the object the
/// substrate hands the script (`build_stdin_json` withholds `script_inline`).
fn shipped_params() -> serde_json::Value {
    let mut p = config()["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    serde_json::Value::Object(p)
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
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

fn emit(params: serde_json::Value, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut doc = doc;
    doc["params"] = params;
    let out = run_script_on_stdin(&script(), &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "assemble exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("emissions are json")
}

/// The arriving turn -- the occasion the window read is built on. Since
/// GH #419 the window is a CALL of the ONE message a turn opens with, so the
/// knob is observed where the op is written rather than one hop later.
fn window_read() -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1"},
                   "hop": {"route": "in_turn"}},
        "messages": [{"origin": "user", "type": "text", "text": "hello"}]
    })
}

/// The `limit` of the window select -- i.e. what `window_turns` actually did.
fn window_limit(out: &[serde_json::Value]) -> i64 {
    assert_eq!(out.len(), 1, "the turn opens with one message");
    let op = out[0]["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(t["text"].as_str()?).ok())
        .find(|a| a["table"] == "turns" && a["operation"] == "select" && a["limit"].is_i64())
        .expect("the window call");
    op["limit"].as_i64().expect("limit")
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The old surface is not deprecated, it is absent -- in the script
/// AND in every string of the config that used to name it.
#[test]
fn nothing_in_the_shipped_collector_reads_the_environment_any_more() {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    assert!(
        !raw.contains("COLLECTOR_"),
        "a COLLECTOR_* name survived in the shipped config"
    );
    assert!(
        !script().contains("${"),
        "a ${{...}} substitution token survived in the shipped script"
    );
}

/// Claim 2. Every knob exists in all three places, with the same value.
///
/// The script literal is read out of the source text rather than exercised,
/// because that literal IS the fallback: `_int("window_turns", 12)` is the
/// value a cell uses when its config says nothing, and comparing the text is
/// the complete check over all twenty-five knobs.
#[test]
fn every_knob_is_a_param_a_setting_and_a_script_literal_with_one_value() {
    let cfg = config();
    let src = script();
    let params = cfg["params"].as_object().expect("params");
    let settings = cfg["contract"]["settings"]
        .as_object()
        .expect("contract.settings");

    for (knob, kind) in KNOBS {
        let param = params
            .get(*knob)
            .unwrap_or_else(|| panic!("params.{knob} is missing"));
        let default = settings
            .get(*knob)
            .unwrap_or_else(|| panic!("contract.settings.{knob} is missing"))
            .get("default")
            .unwrap_or_else(|| panic!("contract.settings.{knob}.default is missing"));
        assert_eq!(
            param, default,
            "params.{knob} and contract.settings.{knob}.default disagree"
        );

        // `NAME = _int("window_turns", 12)` -- the literal after the comma.
        let needle = format!("{kind}(\"{knob}\", ");
        let at = src
            .find(&needle)
            .unwrap_or_else(|| panic!("the script does not read {knob} with {kind}"));
        let rest = &src[at + needle.len()..];
        let lit = &rest[..rest.find(')').expect("closing paren")];
        let lit: serde_json::Value = serde_json::from_str(lit)
            .unwrap_or_else(|e| panic!("{knob}: script literal {lit:?} is not json ({e})"));
        assert_eq!(
            lit, *param,
            "the script's own fallback for {knob} drifted from the shipped param"
        );
    }

    // No knob may hide: every non-substrate param is one of the twenty-five above.
    //
    // The allow-list is the `code` cell's OWN param surface, i.e. every key
    // `CodeParams::parse` reads (crates/meclaw-cells/src/code/params.rs) --
    // not the subset this template happens to set today. Kept complete on
    // purpose: a substrate param the list forgets shows up here as a phantom
    // "undeclared knob", which is a false accusation against the template.
    let substrate = [
        "runner",
        "script_path",
        "script_inline",
        "external_timeout_ms",
        "max_concurrency",
        "sandbox",
        "runner_mode",
    ];
    for key in params.keys() {
        assert!(
            substrate.contains(&key.as_str()) || KNOBS.iter().any(|(k, _)| k == key),
            "params.{key} is neither a substrate param nor a declared knob"
        );
    }
    assert_eq!(
        settings.len(),
        KNOBS.len(),
        "contract.settings and the knob inventory disagree in size"
    );
}

/// Claim 2, behaviourally: a cell whose `params` say nothing behaves exactly
/// like the shipped one. The literal check above is the complete one; this is
/// the one that runs the script.
#[test]
fn an_empty_params_object_behaves_like_the_shipped_defaults() {
    let shipped = window_limit(&emit(shipped_params(), window_read()));
    let empty = window_limit(&emit(serde_json::json!({}), window_read()));
    assert_eq!(
        shipped, empty,
        "the script's fallback and the shipped param produce different windows"
    );
    assert_eq!(shipped, 12, "the shipped window is still twelve turns");
}

/// A knob blanked by an operator means "not configured", not a dead cell.
#[test]
fn a_blank_or_null_knob_falls_back_to_the_shipped_default() {
    for blank in [
        serde_json::json!(""),
        serde_json::json!("   "),
        serde_json::json!(null),
    ] {
        let mut p = shipped_params();
        p["window_turns"] = blank.clone();
        assert_eq!(
            window_limit(&emit(p, window_read())),
            12,
            "a window_turns of {blank} must fall back to the shipped default"
        );
    }
}

/// A numeric knob may arrive as a string -- which is what a `${VAR}`-carrying
/// param resolves to, and the reason the accessors coerce instead of assuming.
#[test]
fn a_numeric_knob_may_arrive_as_a_string() {
    let mut p = shipped_params();
    p["window_turns"] = serde_json::json!("5");
    assert_eq!(window_limit(&emit(p, window_read())), 5);
}

/// Claim 3 -- the whole point of GH #136. One shipped script, two params
/// objects, two windows. Under the environment form both instances read the
/// same key and this test could not be written at all.
#[test]
fn two_instances_of_the_same_script_are_tuned_apart() {
    let mut chatty = shipped_params();
    chatty["window_turns"] = serde_json::json!(40);
    let mut terse = shipped_params();
    terse["window_turns"] = serde_json::json!(3);

    assert_eq!(window_limit(&emit(chatty, window_read())), 40);
    assert_eq!(window_limit(&emit(terse, window_read())), 3);
}
