//! GH #375 -- the night sweeps the lane scratch.
//!
//! `scratch` and `recall_scratch` are the parking places every glue lane of the
//! `memory-hive` carries state across a store round trip in: `extract-glue`
//! stages its payload and its known-facts set, `dream-glue` stages the night's
//! working sets, `porter` stages a transfer part, `close-glue` stages the close
//! pass's four read sets and its parked verdict, and `recall` writes one
//! `recall_scratch` row per retrieval leg plus the fired-once guard the fan-in
//! reads. Every one of them was written and then never removed: the ONLY delete
//! anywhere in the hive was `dream-glue`'s on `predicate_cardinality`, which is
//! a re-judgement upsert and has nothing to do with lane state. Both tables
//! therefore grew monotonically with the traffic, and since `3.0.0` one closed
//! SESSION parks four rows plus the meeting that reads them back -- the growth
//! turned from per-batch into per-conversation.
//!
//! The nightly run is the sweeper the issue asks for, because it is the one lane
//! with a clock (`delta_to`), a window and a receipt. It deletes what is older
//! than `MEMORY_SCRATCH_TTL_DAYS` days before its own window end and touches
//! nothing else -- no memory table, no provenance, no durable row. The No-Delete
//! policy keeps standing where it belongs.
//!
//! **A TTL sweep can never break the close pass's meeting.** That read is the
//! one that looks like it might, so it is pinned here rather than argued: the
//! meeting orders `created_at desc` and takes the FIRST row it sees per kind, so
//! it consumes the NEWEST parking -- exactly the head of an ordering a cutoff
//! only ever cuts the tail of. A row young enough to be read back is younger
//! than the cutoff by orders of magnitude (a recall lives seconds, an extraction
//! a round trip, a close pass one model call), and a sweep that removes older
//! rows cannot change which row is at the head.
//!
//! Everything here runs the REAL `params.script_inline` of `dream-glue` against
//! injected store replies, so no model is called and nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const CLOSE_CONFIG: &str = "../../templates/memory-hive/close-glue/config.json";
const HIVE_README: &str = "../../templates/memory-hive/README.md";

const RUN: &str = "run-375";
/// The window end of the run below. `delta_to` is the ONLY clock this lane has,
/// so the cutoff is derived from it and a replay computes the same one.
const TO: &str = "2026-08-23T03:00:00Z";
/// `TO` minus the shipped default of seven days.
const CUTOFF: &str = "2026-08-16T03:00:00Z";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn config_of(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("config");
    serde_json::from_str(&raw).expect("config json")
}

fn script_of(path: &str) -> String {
    resolve_vars(
        config_of(path)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown past that line, so `python3 -c <whole script>` is a
/// harness that breaks on size rather than on behaviour (GH #279).
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

/// Run the real dream-glue script with a real stdin document and return the
/// emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &script_of(GLUE_CONFIG),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One store reply as the edge delivers it: the run keys in the context, the
/// operation in the hop, the projected rows as the first message's text.
fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The scratch rows of a run, as the `apply-run` select hands them back: the two
/// mandatory kinds and nothing else, so the phase reaches its op list.
fn run_scratch() -> serde_json::Value {
    serde_json::json!([
        {"key": RUN, "kind": "verdicts", "payload": "{}"},
        {"key": RUN, "kind": "beliefs", "payload": "[]"},
    ])
}

/// Every op the closing phase of a run emits, already parsed.
fn apply_ops() -> Vec<serde_json::Value> {
    emit(store_reply("apply-run", "select", run_scratch()))
        .iter()
        .map(args_of)
        .collect()
}

/// The delete this run emits for one table.
fn sweep_of(ops: &[serde_json::Value], table: &str) -> serde_json::Value {
    ops.iter()
        .find(|a| a["operation"] == "delete" && a["table"] == table)
        .unwrap_or_else(|| panic!("the night did not sweep `{table}`: {ops:?}"))
        .clone()
}

// ---------------------------------------------------------------- the two ops

#[test]
fn the_night_deletes_the_aged_lane_scratch_of_both_bookkeeping_tables() {
    // The whole of GH #375. Against the old script this finds no delete at all:
    // the only one in the hive stood on `predicate_cardinality`, which is a
    // re-judgement and not lane state.
    let ops = apply_ops();
    for table in ["scratch", "recall_scratch"] {
        let sweep = sweep_of(&ops, table);
        assert_eq!(
            sweep["where"]["created_at"]["lt"], CUTOFF,
            "the sweep of `{table}` is not bounded on the cutoff this window derives: {sweep}"
        );
    }
    assert!(
        !ops.iter().any(|a| a["operation"] == "delete"
            && a["table"] != "scratch"
            && a["table"] != "recall_scratch"),
        "the sweep reached a table that is not lane state: {ops:?}"
    );
}

#[test]
fn the_cutoff_is_derived_from_the_window_end_and_not_from_the_clock() {
    // `delta_to` is the only clock this lane has -- every value a dream run writes
    // is derived from it, which is what makes a replay byte-identical. A sweep
    // that read "now" would make two replays of one window delete different rows.
    let ops = apply_ops();
    let sweep = sweep_of(&ops, "scratch");
    let cutoff = sweep["where"]["created_at"]["lt"]
        .as_str()
        .expect("a string cutoff");
    assert!(
        cutoff < TO,
        "the cutoff is not before the window end: {cutoff}"
    );
    assert_eq!(
        cutoff, CUTOFF,
        "the cutoff is not `delta_to` minus MEMORY_SCRATCH_TTL_DAYS"
    );
}

#[test]
fn a_row_younger_than_the_cutoff_is_outside_the_where() {
    // The property the sweep rests on, asserted on the filter rather than argued:
    // the bound is a STRICT `lt` on `created_at`, so everything the cutoff does
    // not strictly precede survives -- including every row THIS run just parked,
    // which carries `delta_to` itself as its `created_at`.
    let sweep = sweep_of(&apply_ops(), "scratch");
    let filter = &sweep["where"]["created_at"];
    assert!(
        filter.get("lt").is_some(),
        "the bound must be a strict `lt`, got {filter}"
    );
    for forbidden in ["gt", "gte", "is_null", "or_null"] {
        assert!(
            filter.get(forbidden).is_none(),
            "the sweep must not widen its own window: {filter}"
        );
    }
    let cutoff = filter["lt"].as_str().expect("a string cutoff");
    // The three ages that must survive: this night's own parking, yesterday's,
    // and a close pass parked an hour before the run fired. All of them are read
    // back inside their own run, and none of them is older than the cutoff.
    for young in [
        TO,
        "2026-08-22T03:00:00Z",
        "2026-08-23T02:00:00.123456Z",
        "2026-08-16T03:00:01Z",
    ] {
        assert!(
            young >= cutoff,
            "a row parked at {young} would be swept by a cutoff of {cutoff}"
        );
    }
    // And the one that must not: a fortnight-old parking nobody will ever read.
    assert!("2026-08-09T03:00:00Z" < cutoff, "the sweep reaches nothing");
}

#[test]
fn the_close_passs_meeting_reads_the_head_of_the_order_a_ttl_only_cuts_the_tail_of() {
    // Stated as a test because it is the one read that looks like the sweep could
    // break it. The close lane parks four rows plus a verdict under `close:
    // <session>` and reads them back ordered `created_at desc`, taking the FIRST
    // row it sees per kind -- so what it consumes is the NEWEST parking, which is
    // the head of the ordering. A TTL cutoff removes the oldest rows, i.e. the
    // tail. Cutting the tail of a descending order cannot change its head, so no
    // retention window -- however short -- can make the meeting read a different
    // row than it read before.
    let close = script_of(CLOSE_CONFIG);
    assert!(
        close.contains("\"order_by\": [{\"col\": \"created_at\", \"dir\": \"desc\"}"),
        "the close pass's meeting no longer reads newest first; the sweep's \
         safety argument rests on that ordering"
    );
    let sweep = sweep_of(&apply_ops(), "scratch");
    assert!(
        sweep["where"]["created_at"]["lt"].is_string(),
        "and the sweep still bounds itself on the same column: {sweep}"
    );
}

// ------------------------------------------------------------------- the knob

#[test]
fn the_retention_window_is_a_declared_knob_with_a_generous_default() {
    // Three places have to agree, the way every other knob of this lane does:
    // the script (where the default is spent), `contract.settings` (the public
    // surface -- a knob nobody declared is not discoverable from outside) and the
    // README's variable table (where an operator looks it up).
    let raw = config_of(GLUE_CONFIG)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    assert!(
        raw.contains("_int(\"scratch_ttl_days\", 7)"),
        "the script does not spend the knob with its default"
    );
    let setting = &config_of(GLUE_CONFIG)["contract"]["settings"]["scratch_ttl_days"];
    assert_eq!(setting["type"], "number", "undeclared knob: {setting}");
    assert_eq!(setting["default"], 7);
    assert_eq!(setting["secret"], false);
    assert_eq!(
        config_of(GLUE_CONFIG)["params"]["scratch_ttl_days"],
        7,
        "the knob is not a param, so no instance could be given its own window \
         (GH #138: an override may only name a key that exists under `params`)"
    );
    let readme = std::fs::read_to_string(HIVE_README).expect("hive README");
    assert!(
        readme.contains("| `scratch_ttl_days` | `7` |"),
        "the knob is missing from the README's params table"
    );
}

#[test]
fn an_unparseable_window_end_sweeps_nothing_at_all() {
    // The one way this op could hurt: a cutoff the lane could not compute would
    // render as an empty `where`, and a delete without a where is a table drop
    // with extra steps. The same fallback direction `extract_cutoff` already
    // takes -- do less, never more.
    let doc = serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "apply-run",
                        "dream_run": RUN, "dream_to": "nightly"},
            "hop": {"operation": "select", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": run_scratch().to_string()}]
    });
    let ops: Vec<serde_json::Value> = emit(doc).iter().map(args_of).collect();
    assert!(
        !ops.iter().any(|a| a["operation"] == "delete"),
        "an unparseable window end still emitted a delete: {ops:?}"
    );
}
