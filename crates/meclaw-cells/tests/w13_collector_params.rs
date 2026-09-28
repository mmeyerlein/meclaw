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

/// The nine knobs, with the kind of accessor the script reads each one with.
/// Restated here on purpose: this is the inventory the migration claims to be
/// complete, and a knob that quietly leaves the config should fail the pin.
///
/// GH #889 (`collector@5.0.0`) took nineteen knobs out -- the window and its
/// caps (`window_turns`, `window_bytes`, `turn_chars`, `tool_chars`,
/// `round_bytes`, `memory_chars`), the curation stages (`context_window`,
/// `curate_*`, `keep_rounds`, `recoverability`, `tool_menu`,
/// `tool_desc_chars`), `thread_recall*`, `turn_write` and `prune_after_ms`:
/// the curator owns the window now (R-27-1), so this inventory shrinks with it.
const KNOBS: &[(&str, &str)] = &[
    ("memory_tier", "_str"),
    ("memory_form", "_str"),
    // `memory_call_tier` stood here until GH #552. It was the tier of the memory
    // TOOL, and the tool is not this cell's any more: the member's memory hive
    // declares the name and answers the call, with a tier of its own on the cell
    // that serves it. The AMBIENT leg above is what is left, and it keeps its own
    // three knobs.
    ("max_iter", "_int"),
    ("round_idle_ms", "_int"),
    // GH #728 -- the deadline of a consult or a delegation: inside it the answer
    // is a leg of the member's turn, past it a straggler (`hop.late`).
    ("late_after_ms", "_int"),
    // GH #525 -- the block contract, `inline_extraction` until GH #606. What it
    // asks FOR is no longer a literal of this cell: the sections are offered on
    // the menu lane and this knob decides whether they are composed into a
    // contract at all. (Its partner `turn_write`, which minted the episode the
    // annotation is bound to, moved to the curator's writer with GH #889.)
    ("sidecar", "_str"),
    // GH #606 -- the ceiling of that composed contract. It is the only bound
    // this cell puts on words it did not write, which is why it lives here and
    // not as a promise each offering template has to keep.
    ("sidecar_max_chars", "_int"),
    // GH #464 -- the DECLARATION. It is the only knob whose value is a list,
    // and the only one whose effect is a QUESTION rather than a number: the
    // names in it are what the menu tick asks a tools hive for. Empty is the
    // shipped default and asks nothing, which is why a collector that ships
    // without a tools hive beside it is silent rather than noisy.
    ("tools", "_list"),
    // GH #834 -- the brief leg. A LIST like `tools`, and a question like it too:
    // the names are affinity's slot vocabulary, asked about the counterpart of a
    // turn. Empty is the shipped default and asks nothing.
    ("brief_slots", "_list"),
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

/// A tool round of iteration `iter` that has just completed: one call and its
/// result, read back by the round-check bundle -- the occasion the iteration
/// cap is judged on.
///
/// GH #889: the behavioural pins below used to observe `window_turns` as the
/// `limit` of the window read. That knob left with the window (the curator owns
/// it, R-27-1), so they observe `max_iter` instead -- the same accessor, a knob
/// that stays, and a decision (`curate` or `answer`) the seam writes itself.
fn round_done_at(iter: i64) -> serde_json::Value {
    let call = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"}
    ]);
    let res = serde_json::json!(
        {"origin": "tool", "type": "tool_result", "id": "c1", "text": "ok"}
    );
    let rows = serde_json::json!([
        {"turn_id": "t1", "iter": iter, "role": "assistant",
         "turn": call.to_string(), "fired": 0},
        {"turn_id": "t1", "iter": iter, "role": "tool",
         "turn": res.to_string(), "fired": 0}
    ]);
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1",
                               "iter": iter.to_string(), "col_phase": "round-check",
                               "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 2,
                           "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": "c-round-check-read", "text": rows.to_string()}],
        "results": [{"tool_call_id": "c-round-check-read", "operation": "select",
                     "rows_affected": 2, "duration_ms": 0}]
    })
}

/// Whether the round of iteration `iter` leaves on `answer` instead of going on
/// to `curate` -- i.e. what `max_iter` actually did.
fn capped_at(params: serde_json::Value, iter: i64) -> bool {
    let out = emit(params, round_done_at(iter));
    let seam = out
        .iter()
        .find(|m| m["header"]["route"] == "curate" || m["header"]["route"] == "answer")
        .unwrap_or_else(|| panic!("the completed round leaves on a seam: {out:?}"));
    seam["header"]["route"] == "answer"
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
/// because that literal IS the fallback: `_int("max_iter", 8)` is the
/// value a cell uses when its config says nothing, and comparing the text is
/// the complete check over all nine knobs.
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

        // `NAME = _int("max_iter", 8)` -- the literal after the comma.
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

    // No knob may hide: every non-substrate param is one of the nine above.
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
    // GH #889: observed on `max_iter` (8) instead of the removed `window_turns`.
    for iter in [7, 8] {
        assert_eq!(
            capped_at(shipped_params(), iter),
            capped_at(serde_json::json!({}), iter),
            "the script's fallback and the shipped param end the round apart at \
             iteration {iter}"
        );
    }
    assert!(
        !capped_at(shipped_params(), 7) && capped_at(shipped_params(), 8),
        "the shipped round still ends at eight iterations"
    );
}

/// A knob blanked by an operator means "not configured", not a dead cell.
#[test]
fn a_blank_or_null_knob_falls_back_to_the_shipped_default() {
    for blank in [
        serde_json::json!(""),
        serde_json::json!("   "),
        serde_json::json!(null),
    ] {
        // GH #889: `max_iter` stands in for the removed `window_turns`.
        let mut p = shipped_params();
        p["max_iter"] = blank.clone();
        assert!(
            !capped_at(p.clone(), 7) && capped_at(p, 8),
            "a max_iter of {blank} must fall back to the shipped default"
        );
    }
}

/// A numeric knob may arrive as a string -- which is what a `${VAR}`-carrying
/// param resolves to, and the reason the accessors coerce instead of assuming.
#[test]
fn a_numeric_knob_may_arrive_as_a_string() {
    // GH #889: `max_iter` stands in for the removed `window_turns`.
    let mut p = shipped_params();
    p["max_iter"] = serde_json::json!("5");
    assert!(!capped_at(p.clone(), 4), "under the cap the round goes on");
    assert!(
        capped_at(p, 5),
        "the string \"5\" is read as the number five"
    );
}

/// Claim 3 -- the whole point of GH #136. One shipped script, two params
/// objects, two behaviours. Under the environment form both instances read the
/// same key and this test could not be written at all.
///
/// GH #889: the two instances differ in `max_iter` now, since `window_turns`
/// left with the window -- the same round ends in one and goes on in the other.
#[test]
fn two_instances_of_the_same_script_are_tuned_apart() {
    let mut patient = shipped_params();
    patient["max_iter"] = serde_json::json!(40);
    let mut terse = shipped_params();
    terse["max_iter"] = serde_json::json!(3);

    assert!(
        !capped_at(patient, 3),
        "forty iterations: the round goes on"
    );
    assert!(capped_at(terse, 3), "three iterations: the same round ends");
}
