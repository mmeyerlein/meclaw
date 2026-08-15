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

/// The twenty knobs, with the kind of accessor the script reads each one with.
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
    ("context_window", "_int"),
    ("curate_soft", "_float"),
    ("curate_hard", "_float"),
    ("keep_rounds", "_int"),
    ("recoverability", "_str"),
    ("thread_recall", "_str"),
    ("thread_recall_budget", "_float"),
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

fn emit(params: serde_json::Value, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut doc = doc;
    doc["params"] = params;
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "assemble exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("emissions are json")
}

/// The store reply that makes the collector read its rolling window -- the one
/// emission that carries a knob (`window_turns`) verbatim into a store op, and
/// therefore the cheapest OBSERVABLE probe of a configured value.
fn window_read() -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "turn-open", "store_origin": "collector"},
                   "hop": {"operation": "select", "rows_affected": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x", "text": "[]"}]
    })
}

/// The `limit` of the window select -- i.e. what `window_turns` actually did.
fn window_limit(out: &[serde_json::Value]) -> i64 {
    assert_eq!(out.len(), 1, "the window read is one emission");
    let text = out[0]["messages"][0]["text"].as_str().expect("op text");
    let op: serde_json::Value = serde_json::from_str(text).expect("op json");
    assert_eq!(op["table"], "turns");
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
/// the complete check over all twenty knobs.
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

    // No knob may hide: every non-substrate param is one of the twenty above.
    let substrate = ["runner", "external_timeout_ms", "script_inline"];
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
