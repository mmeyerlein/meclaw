//! meclaw-os -- per-turn episodes, LLM-free (wave 9, track A).
//!
//! Until now an episode came into being at the NIGHT CLOSE: the collector
//! handed its day out as one batch, `memory-drain` decomposed it, and the
//! memory hive wrote it. That is a freshness hole of up to 24 hours -- a
//! question about the last turn can be answered wrongly, which is a direct
//! breach of the north star ("no question may be answered wrongly").
//!
//! This track closes the hole without a single model call: the collector hands
//! the day out AGAIN after every stored turn, on its own route, into the
//! drain's EXISTING batch entry. The seam is deliberate (v8 review, E3): the
//! `turn_id` formula `"<session_id>#<index>"` must be the same one the close
//! drain mints, or a filled memory (61 productive episodes) doubles. It is the
//! same formula here because it is the same CODE -- one minter, one ledger, one
//! high-water mark. Nothing of `memory-drain` changes.
//!
//! Four claims are pinned:
//!
//! 1. PER-TURN -- a stored turn asks for the day right away and hands it out on
//!    route `turn_write`; switched off, nothing about the collector moves.
//! 2. IDEMPOTENCE -- the same turn delivered twice writes no second episode,
//!    through the ledger that already guards the close lane.
//! 3. COMPLETENESS -- the close drain over a partly written session writes
//!    exactly the missing turns, and the count gate proves it.
//! 4. REPLAY -- the episode of the per-turn lane and the episode of the close
//!    lane are byte-identical, or the close lane is not a replay at all.
//!
//! Everything runs the shipped `params.script_inline` of both cells against
//! real stdin documents. No mock, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";
const DRAIN_CONFIG: &str = "../../templates/memory-drain/drain/config.json";

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
/// merged over them. The collector's knobs live here since `collector@1.2.0`;
/// the memory-drain's are still `${VAR}` literals and go through [`script_of`].
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

/// The collector with the per-turn lane switched ON.
fn collector(doc: Value) -> Vec<Value> {
    collector_with(&[("turn_write", "1")], doc)
}

/// The collector exactly as it ships -- the lane is off until somebody turns
/// it on.
fn collector_default(doc: Value) -> Vec<Value> {
    collector_with(&[], doc)
}

fn drain(doc: Value) -> Vec<Value> {
    run(&script_of(DRAIN_CONFIG, &[]), doc)
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

/// One row of the collector's `turns` table, as the store hands it back.
fn turn_row(id: &str, session: &str, role: &str, content: &str) -> Value {
    json!({"id": id, "session_id": session, "turn_id": "t-".to_string() + id,
           "role": role, "content": content, "recorded_at": id})
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

fn day_rows(session: &str, upto: usize) -> Value {
    Value::Array(
        DAY.iter()
            .take(upto)
            .enumerate()
            .map(|(i, (role, text))| turn_row(&format!("2026-08-15-{i:02}"), session, role, text))
            .collect(),
    )
}

/// The stored turn is the occasion: the moment a turn is in the table, the
/// collector asks for the day it belongs to. No timer, no gate, no model --
/// the same select the close lane fires, one turn earlier.
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
        "the session is the scope, exactly like the close lane's read"
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

/// The lane ships OFF. A collector whose parent wired no drain must not emit a
/// route nobody consumes -- that is a dead letter per turn, not a memory.
#[test]
fn the_per_turn_lane_is_off_until_somebody_turns_it_on() {
    let turn = collector_default(insert_echo("s1", "turn-w", "t1"));
    assert!(
        phase_msgs(&turn, "tw-scan").is_empty(),
        "no day select by default -- {turn:?}"
    );
    assert_eq!(
        phase_msgs(&turn, "turn-open").len(),
        1,
        "and the round check is untouched -- {turn:?}"
    );
    assert!(
        collector_default(insert_echo("s1", "ans-w", "t1")).is_empty(),
        "the answer echo stays the dead end it was"
    );
}

/// What leaves is the day in the order it happened, on its OWN route: the
/// `write` route already feeds the summarizer inside the talky, and firing
/// that per turn would be an LLM call per turn -- the exact opposite of an
/// LLM-free write path.
#[test]
fn the_per_turn_batch_is_the_day_in_the_order_it_happened() {
    let out = collector(select_reply("s1", "tw-scan", "t1", day_rows("s1", 3)));
    assert_eq!(out.len(), 1, "one emission: the day -- {out:?}");
    assert_eq!(hop(&out[0], "route"), "turn_write");
    assert_eq!(hop(&out[0], "session_id"), "s1");
    assert_eq!(hop(&out[0], "turn_count"), "3");
    assert_eq!(
        out[0]["messages"],
        json!([
            {"origin": "user", "type": "text", "text": "my editor is helix"},
            {"origin": "assistant", "type": "text", "text": "noted"},
            {"origin": "user", "type": "text", "text": "and i cook keto"}
        ])
    );
}

/// An empty session is not a batch. Nothing to write, nothing emitted.
#[test]
fn a_session_without_a_single_turn_hands_out_nothing() {
    assert!(collector(select_reply("s1", "tw-scan", "t1", json!([]))).is_empty());
}

// ══════════════════════════════════════════════ the same day, the two lanes

/// The close batch, produced by the shipped close chain over the same rows:
/// `close-turns` parks the day as its own leg, `close-fire` hands it out.
fn close_batch(session: &str, rows: Value) -> Value {
    let parked = collector(select_reply(session, "close-turns", "close-x", rows));
    let leg = args(&parked[0])["row"].clone();
    let out = collector(select_reply(
        session,
        "close-fire",
        "close-x",
        json!([{"turn_id": leg["turn_id"], "iter": 0, "role": "leg-close",
                "turn": leg["turn"], "fired": 0, "recorded_at": "2026-08-15"}]),
    ));
    out.into_iter()
        .find(|m| hop(m, "route") == "write")
        .expect("the close batch")
}

/// Both lanes read the same table with the same order and map roles the same
/// way -- so the day they hand out is the same document. If it ever were not,
/// the indexes would differ and the close lane would write a SECOND memory
/// instead of a replay.
#[test]
fn the_per_turn_batch_and_the_close_batch_carry_the_same_day() {
    let rows = day_rows("s1", 4);
    let per_turn = collector(select_reply("s1", "tw-scan", "t1", rows.clone()));
    let close = close_batch("s1", rows);

    assert_eq!(
        per_turn[0]["messages"], close["messages"],
        "the day is one document, whichever occasion asks for it"
    );
    assert_eq!(hop(&per_turn[0], "turn_count"), hop(&close, "turn_count"));
}

// ═══════════════════════════════════════════════════════════ through the drain

/// One drain chain over a batch, replayed exactly as the colony runs it: the
/// day is parked, the ledger answers with everything this session left behind,
/// and what comes back is what the drain fires at the hive's writer port.
fn drain_run(session: &str, batch: &Value, marks: &[u64]) -> Vec<Value> {
    let doc = json!({
        "header": {
            "hop": {"route": "in_batch", "session_id": session,
                    "turn_count": hop(batch, "turn_count"), "round_count": "0"},
            // The round the day was spoken in (#244/#269): the drain refuses a
            // batch that does not carry one rather than consuming turns it
            // could not deliver. Named participants, never `["*"]`.
            "context": {"session_id": session,
                        "audience_set": r#"["member:alex","agent:scribe"]"#,
                        "channel": "tg:4711"}
        },
        "messages": batch["messages"].clone()
    });
    let parked = drain(doc);
    if parked.is_empty() {
        return vec![];
    }
    let payload = args(&parked[0])["row"]["payload"].clone();
    let mut rows = vec![json!({"id": "2026-08-15-batch", "kind": "batch",
                               "payload": payload, "drained_upto": 0})];
    for (i, upto) in marks.iter().enumerate() {
        rows.push(json!({"id": format!("2026-08-15-mark{i}"), "kind": "mark",
                         "payload": "", "drained_upto": upto}));
    }
    drain(json!({
        "header": {
            "hop": {"operation": "select", "rows_affected": rows.len()},
            "context": {"session_id": session, "drain_phase": "probe",
                        "drain_origin": "drain"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "d-probe",
                      "text": Value::Array(rows).to_string()}]
    }))
}

fn episodes(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| hop(m, "route") == "episode")
        .collect()
}

fn marked_upto(out: &[Value]) -> Option<u64> {
    out.iter()
        .find(|m| hop(m, "route") == "lstore" && hop(m, "phase") == "mark")
        .and_then(|m| args(m)["row"]["drained_upto"].as_u64())
}

/// The point of the whole track: the turn that just happened is an episode
/// NOW, under the id the close drain would have minted for it tonight.
#[test]
fn a_stored_turn_is_an_episode_under_the_drains_own_id() {
    let batch = collector(select_reply("s1", "tw-scan", "t1", day_rows("s1", 1)));
    let out = drain_run("s1", &batch[0], &[]);
    let eps = episodes(&out);

    assert_eq!(eps.len(), 1, "one turn, one episode -- {out:?}");
    assert_eq!(hop(eps[0], "turn_id"), "s1#0");
    assert_eq!(eps[0]["messages"][0]["text"], "my editor is helix");
    assert_eq!(marked_upto(&out), Some(1), "and the ledger says so");
}

/// Idempotence, the half this track adds: the SAME turn arrives again (a
/// retry, a replay, the next turn's day that still contains it) and writes
/// nothing a second time. The guard is the ledger the close lane already had.
#[test]
fn the_same_turn_twice_writes_no_second_episode() {
    let batch = collector(select_reply("s1", "tw-scan", "t1", day_rows("s1", 1)));
    let again = drain_run("s1", &batch[0], &[1]);
    assert!(
        again.is_empty(),
        "a drained turn is a skip, not a second insert -- {again:?}"
    );
}

/// The per-turn cadence in full: every turn hands out the day it belongs to,
/// and every turn adds exactly ONE episode -- the ones before it are already
/// marked. Six deliveries, four episodes, four ids.
#[test]
fn every_turn_adds_exactly_its_own_episode() {
    let mut marks: Vec<u64> = vec![];
    let mut ids: Vec<String> = vec![];
    for n in 1..=DAY.len() {
        let batch = collector(select_reply("s1", "tw-scan", "t1", day_rows("s1", n)));
        let out = drain_run("s1", &batch[0], &marks);
        let eps = episodes(&out);
        assert_eq!(eps.len(), 1, "turn {n} adds one episode -- {out:?}");
        ids.push(hop(eps[0], "turn_id"));
        marks.push(marked_upto(&out).expect("a mark"));
    }
    assert_eq!(ids, vec!["s1#0", "s1#1", "s1#2", "s1#3"]);
    assert_eq!(marks, vec![1, 2, 3, 4]);
}

// ═══════════════════════════════════════════════════ the close lane, replayed

/// The role change the review asks for (§ 8.2): the close drain is the safety
/// net. Over a day the per-turn lane already carried, it writes NOTHING -- and
/// the count gate is what proves it: `turn_count` of the batch equals the
/// episodes the session has, so a drain that writes zero is the SUCCESS
/// signal, not a broken one.
#[test]
fn the_close_over_an_already_written_day_writes_nothing() {
    let rows = day_rows("s1", 4);
    let mut marks: Vec<u64> = vec![];
    let mut written = 0usize;
    for n in 1..=DAY.len() {
        let batch = collector(select_reply("s1", "tw-scan", "t1", day_rows("s1", n)));
        let out = drain_run("s1", &batch[0], &marks);
        written += episodes(&out).len();
        marks.push(marked_upto(&out).expect("a mark"));
    }

    let close = close_batch("s1", rows);
    let out = drain_run("s1", &close, &marks);
    assert!(
        out.is_empty(),
        "the safety net had nothing to catch -- {out:?}"
    );
    assert_eq!(
        hop(&close, "turn_count").parse::<usize>().expect("count"),
        written,
        "count gate: every turn of the day is an episode, written by the per-turn lane"
    );
}

/// And when the per-turn lane missed the end of the day -- a lost message, a
/// restart, a lane switched on late -- the close drain writes exactly the
/// missing turns and nothing else. That is what makes it a net.
#[test]
fn the_close_writes_only_the_turns_the_per_turn_lane_missed() {
    let close = close_batch("s1", day_rows("s1", 4));
    let out = drain_run("s1", &close, &[2]);
    let eps = episodes(&out);

    assert_eq!(eps.len(), 2, "two of four are missing -- {out:?}");
    assert_eq!(hop(eps[0], "turn_id"), "s1#2");
    assert_eq!(hop(eps[1], "turn_id"), "s1#3");
    assert_eq!(eps[0]["messages"][0]["text"], "and i cook keto");
    assert_eq!(marked_upto(&out), Some(4));
}

/// Replay means IDENTICAL, not similar. The episode the per-turn lane fires
/// and the episode the close lane fires for the same turn are the same
/// message, byte for byte -- id, index, sender, text, and the empty
/// `happened_at` that lets the writer stamp the event time itself. Anything
/// less and the two lanes are two memories.
#[test]
fn the_two_lanes_mint_byte_identical_episodes() {
    let rows = day_rows("s1", 4);
    let per_turn = collector(select_reply("s1", "tw-scan", "t1", rows.clone()));
    let from_turns = drain_run("s1", &per_turn[0], &[]);
    let from_close = drain_run("s1", &close_batch("s1", rows), &[]);

    let a: Vec<&Value> = episodes(&from_turns);
    let b: Vec<&Value> = episodes(&from_close);
    assert_eq!(a.len(), 4);
    assert_eq!(a, b, "the same day mints the same episodes on both lanes");
}
