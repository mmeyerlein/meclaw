//! meclaw-os -- per-turn episodes, LLM-free (wave 9, track A).
//!
//! Until wave 9 an episode came into being at the NIGHT CLOSE: the collector
//! handed its day out as one batch, `memory-drain` decomposed it, and the
//! memory hive wrote it. That is a freshness hole of up to 24 hours -- a
//! question about the last turn can be answered wrongly, which is a direct
//! breach of the north star ("no question may be answered wrongly"). This track
//! closed the hole without a single model call.
//!
//! **Re-decided by ruling Q11 (2026-08-21, GH #298).** The hole stays closed;
//! the mechanism behind it is a different one, and this header says which of the
//! file's four original claims survived that and in what form:
//!
//! 1. PER-TURN -- SURVIVES, re-formed, and is now the whole subject of this
//!    file. A stored turn still asks for the day right away and still hands it
//!    out on route `turn_write`. What it hands out is no longer the day as one
//!    batch for a decomposer, but ONE message per turn, in the shape the memory
//!    hive's writer port reads. The switch survives too and has flipped: the
//!    lane is the only path into the episodes table, so it ships ON.
//! 2. IDEMPOTENCE -- SURVIVES, moved. It was the drain's ledger and its
//!    high-water mark; it is now the collector's own `turns.episode_written`
//!    column, guarded in the same emission as the episode it covers. It is
//!    therefore asserted HERE, against the collector, instead of behind a drain.
//! 3. COMPLETENESS (the close drain writes the missing turns) -- **RETIRED by
//!    Q11**, not failed. There is no close drain on the live path any more: the
//!    per-turn lane is not a fast path in front of a safety net, it is the whole
//!    path, and a net that re-reads a day would be the second writer Q11
//!    removed.
//! 4. REPLAY (the two lanes mint byte-identical episodes) -- **RETIRED by Q11**,
//!    not failed. `write` and `turn_write` hand out different documents on
//!    purpose now: a closed session with its rounds for whoever archives a day,
//!    one turn per message for whoever keeps episodes. There is nothing left for
//!    the two to be identical about.
//!
//! The end-to-end shape of the surviving two -- the exact hop keys, the
//! deterministic id, the guarded mark, the filter's effect on the index -- is
//! pinned in `gh298_the_turn_writes_its_own_episode.rs`. What is pinned here is
//! the CADENCE: which occasions ask, what the knob does, and what a session
//! looks like turn after turn.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No mock, no provider, nothing spent.

use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one) -- the same substitution the colony performs at boot.
fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let inner = &tail[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, d),
            None => (inner, ""),
        };
        let value = over
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(default);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str, over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
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

/// Runs a shipped script against a real stdin document and returns its
/// emissions (an empty multi-send is an empty vector).
fn run(script: &str, doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The `params` object the substrate puts on a `code` cell's stdin: the values
/// the config ships, minus the script's own source, with the case's overrides
/// merged over them. The collector's knobs live here since `collector@1.2.0`.
fn params_of(path: &str, over: &[(&str, &str)]) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    let mut p = v["params"].as_object().cloned().expect("params object");
    p.remove("script_inline");
    for (k, val) in over {
        assert!(p.contains_key(*k), "no such param: {k}");
        p.insert((*k).to_string(), json!(val));
    }
    Value::Object(p)
}

fn collector_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut doc = doc;
    doc["params"] = params_of(ASSEMBLE_CONFIG, over);
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
}

/// The collector exactly as it ships -- and since Q11 that means the per-turn
/// lane is on.
fn collector(doc: Value) -> Vec<Value> {
    collector_with(&[], doc)
}

fn hop(m: &Value, key: &str) -> String {
    m["header"][key].as_str().unwrap_or_default().to_string()
}

/// The store args of an emission that goes to a store cell.
fn args(m: &Value) -> Value {
    let text = m["messages"][0]["text"].as_str().expect("tool_call text");
    meclaw_core::serde_json::from_str(text).expect("store args are json")
}

fn phase_msgs<'a>(out: &'a [Value], phase: &str) -> Vec<&'a Value> {
    out.iter().filter(|m| hop(m, "phase") == phase).collect()
}

/// The episodes of an emission, in the order they leave.
fn episodes(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| hop(m, "route") == "turn_write")
        .collect()
}

// ═══════════════════════════════════════════════════ the collector's own side

/// The reply a store cell sends back after an insert: the operation on the
/// hop, the phase in the context (the internal edge promoted it there).
fn insert_echo(session: &str, phase: &str, turn: &str) -> Value {
    json!({
        "header": {
            "hop": {"operation": "insert", "rows_affected": 1},
            "context": {"session_id": session, "turn_id": turn,
                        "col_phase": phase, "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-turn-w",
                      "text": "{\"rows_affected\": 1}"}]
    })
}

/// One row of the collector's `turns` table, as the store hands it back --
/// `episode_written` included, because since Q11 that column is what the scan
/// decides on.
fn turn_row(id: &str, session: &str, role: &str, content: &str, written: i64) -> Value {
    json!({"id": id, "session_id": session, "turn_id": "t-".to_string() + id,
           "role": role, "content": content, "recorded_at": id,
           "interim": 0, "episode_written": written})
}

/// The store's reply to a select, under the phase the request carried out.
fn select_reply(session: &str, phase: &str, turn: &str, rows: Value) -> Value {
    json!({
        "header": {
            "hop": {"operation": "select",
                    "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": session, "turn_id": turn,
                        "col_phase": phase, "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-sel",
                      "text": rows.to_string()}]
    })
}

const DAY: [(&str, &str); 4] = [
    ("user", "my editor is helix"),
    ("assistant", "noted"),
    ("user", "and i cook keto"),
    ("assistant", "noted that too"),
];

fn row_id(i: usize) -> String {
    format!("2026-08-15-{i:02}")
}

/// The session as the store holds it after `upto` turns, with `written` naming
/// the rows whose `episode_written` the collector has already set.
fn day_rows(session: &str, upto: usize, written: &HashSet<String>) -> Value {
    Value::Array(
        DAY.iter()
            .take(upto)
            .enumerate()
            .map(|(i, (role, text))| {
                let id = row_id(i);
                let mark = i64::from(written.contains(&id));
                turn_row(&id, session, role, text, mark)
            })
            .collect(),
    )
}

// ══════════════════════════════════════════════════════ 1. PER-TURN (re-formed)

/// The stored turn is the occasion: the moment a turn is in the table, the
/// collector asks for the day it belongs to. No timer, no gate, no model.
#[test]
fn a_stored_turn_asks_for_the_whole_day_right_away() {
    let out = collector(insert_echo("s1", "turn-w", "t1"));
    let scans = phase_msgs(&out, "tw-scan");
    assert_eq!(
        scans.len(),
        1,
        "one day select on top of the round check -- {out:?}"
    );

    let a = args(scans[0]);
    assert_eq!(a["operation"], "select");
    assert_eq!(a["table"], "turns");
    assert_eq!(
        a["where"],
        json!({"session_id": "s1"}),
        "the session is the scope: an episode index counts the session"
    );
    assert_eq!(
        a["order_by"],
        json!([{"col": "id", "dir": "asc"}]),
        "the order of the day IS the index the turn_id is minted from"
    );
    assert!(
        a.get("limit").is_none(),
        "a window is capped, a memory is not -- a limit here would shift every index"
    );

    // The round check the turn always fired keeps firing: the per-turn lane is
    // added next to the machine, it does not replace a step of it.
    assert_eq!(phase_msgs(&out, "turn-open").len(), 1, "{out:?}");
}

/// The answer is a turn of the day like any other -- and it is the one a
/// question about "what did you just say" needs. Its own insert echo was a
/// dead end before this track; now it is the second occasion.
#[test]
fn the_stored_answer_asks_for_the_whole_day_too() {
    let out = collector(insert_echo("s1", "ans-w", "t1"));
    assert_eq!(phase_msgs(&out, "tw-scan").len(), 1, "{out:?}");
}

/// The knob flipped with Q11 and the direction is asserted, not assumed: a
/// shipped collector writes episodes, because after this wave nothing else
/// does. Off is still reachable, in both spellings, and off means the
/// conversation reaches no memory at all.
#[test]
fn the_per_turn_lane_is_on_unless_somebody_turns_it_off() {
    assert_eq!(
        phase_msgs(&collector(insert_echo("s1", "turn-w", "t1")), "tw-scan").len(),
        1,
        "the shipped collector scans"
    );

    for off in ["", "0"] {
        let turn = collector_with(&[("turn_write", off)], insert_echo("s1", "turn-w", "t1"));
        assert!(
            phase_msgs(&turn, "tw-scan").is_empty(),
            "turn_write={off:?}: no day select -- {turn:?}"
        );
        assert_eq!(
            phase_msgs(&turn, "turn-open").len(),
            1,
            "and the round check is untouched -- {turn:?}"
        );
        assert!(
            collector_with(&[("turn_write", off)], insert_echo("s1", "ans-w", "t1")).is_empty(),
            "turn_write={off:?}: the answer echo stays the dead end it was"
        );
    }
}

/// What leaves is the day in the order it happened, on its OWN route -- but as
/// one message per turn, because the hive's writer takes the first
/// user/assistant turn of a body and ignores the rest. The `write` route stays
/// what it was: it feeds the summarizer inside the talky, and firing that per
/// turn would be an LLM call per turn -- the exact opposite of an LLM-free
/// write path.
#[test]
fn the_day_leaves_as_one_message_per_turn_in_the_order_it_happened() {
    let out = collector(select_reply(
        "s1",
        "tw-scan",
        "t1",
        day_rows("s1", 3, &HashSet::new()),
    ));
    let eps = episodes(&out);
    assert_eq!(eps.len(), 3, "three turns, three messages -- {out:?}");
    for (i, ep) in eps.iter().enumerate() {
        assert_eq!(hop(ep, "route"), "turn_write");
        assert_eq!(hop(ep, "session_id"), "s1");
        assert_eq!(hop(ep, "turn_id"), format!("s1#{i}"));
        assert_eq!(ep["messages"].as_array().expect("messages").len(), 1);
    }
    assert_eq!(
        eps.iter()
            .map(|e| e["messages"][0].clone())
            .collect::<Vec<_>>(),
        vec![
            json!({"origin": "user", "type": "text", "text": "my editor is helix"}),
            json!({"origin": "assistant", "type": "text", "text": "noted"}),
            json!({"origin": "user", "type": "text", "text": "and i cook keto"}),
        ]
    );
}

/// An empty session is not a batch. Nothing to write, nothing emitted.
#[test]
fn a_session_without_a_single_turn_hands_out_nothing() {
    assert!(collector(select_reply("s1", "tw-scan", "t1", json!([]))).is_empty());
}

// ══════════════════════════════════════════════════ 2. IDEMPOTENCE (moved)

/// The marks one scan asked for, as the store would have applied them: the row
/// ids of its guarded updates.
fn marked_by(out: &[Value]) -> Vec<String> {
    phase_msgs(out, "tw-mark")
        .iter()
        .map(|m| {
            args(m)["where"]["id"]
                .as_str()
                .expect("the guard names its row")
                .to_string()
        })
        .collect()
}

/// Idempotence, the half this track added -- now guarded by a column of the
/// collector's own table instead of by a ledger behind a drain. The SAME day
/// arrives again (a retry, the next turn's scan that still contains it) and
/// writes nothing a second time.
#[test]
fn the_same_turn_twice_writes_no_second_episode() {
    let first = collector(select_reply(
        "s1",
        "tw-scan",
        "t1",
        day_rows("s1", 1, &HashSet::new()),
    ));
    assert_eq!(episodes(&first).len(), 1, "{first:?}");
    let written: HashSet<String> = marked_by(&first).into_iter().collect();
    assert_eq!(written, HashSet::from([row_id(0)]), "{first:?}");

    let again = collector(select_reply(
        "s1",
        "tw-scan",
        "t1",
        day_rows("s1", 1, &written),
    ));
    assert!(
        again.is_empty(),
        "a written turn is a skip, not a second episode -- {again:?}"
    );
}

/// The per-turn cadence in full: every turn hands out the day it belongs to,
/// and every turn adds exactly ONE episode -- the ones before it carry the
/// column already. Four deliveries, four episodes, four ids, and the mark of
/// each rides in the emission of the episode it covers.
#[test]
fn every_turn_adds_exactly_its_own_episode() {
    let mut written: HashSet<String> = HashSet::new();
    let mut ids: Vec<String> = vec![];
    for n in 1..=DAY.len() {
        let out = collector(select_reply(
            "s1",
            "tw-scan",
            "t1",
            day_rows("s1", n, &written),
        ));
        let eps = episodes(&out);
        assert_eq!(eps.len(), 1, "turn {n} adds one episode -- {out:?}");
        ids.push(hop(eps[0], "turn_id"));

        let marks = marked_by(&out);
        assert_eq!(
            marks,
            vec![row_id(n - 1)],
            "the mark covers the turn that just left -- {out:?}"
        );
        written.extend(marks);
    }
    assert_eq!(ids, vec!["s1#0", "s1#1", "s1#2", "s1#3"]);
    assert_eq!(written.len(), DAY.len());
}

/// And the id of a turn does not depend on WHEN it was handed out: a session
/// whose lane was switched on late writes the turns it missed under the same
/// ids it would have minted turn by turn. That is what makes the formula an
/// identity rather than a counter -- a bulk import mints exactly this.
#[test]
fn a_late_scan_mints_the_ids_the_turn_by_turn_run_would_have() {
    let late = collector(select_reply(
        "s1",
        "tw-scan",
        "t1",
        day_rows("s1", DAY.len(), &HashSet::new()),
    ));
    assert_eq!(
        episodes(&late)
            .iter()
            .map(|e| hop(e, "turn_id"))
            .collect::<Vec<_>>(),
        vec!["s1#0", "s1#1", "s1#2", "s1#3"]
    );
}
