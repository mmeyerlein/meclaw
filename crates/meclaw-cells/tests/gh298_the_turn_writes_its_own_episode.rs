//! meclaw-os -- the turn writes its own episode (GH #298, #282; RULED Q11).
//!
//! Until this wave the per-turn lane handed the WHOLE DAY out again after every
//! stored turn, as one batch, for a decomposer to cut into episodes and for that
//! decomposer's ledger to deduplicate. Q11 removes the decomposer from the live
//! path, and with it the batch: what leaves on route `turn_write` is now ONE
//! message per turn, in the shape the memory hive's writer port accepts (it
//! takes the FIRST `user`/`assistant` text turn of a body and ignores the rest,
//! so one turn per message is not a preference but the port's own arithmetic).
//!
//! Three claims are pinned here:
//!
//! 1. **ONE MESSAGE PER TURN.** Every drainable turn of the session leaves as
//!    its own message, under the deterministic id `"<session_id>#<index>"` and
//!    with `turn_index` and `happened_at` beside it -- the same id a bulk
//!    import mints, because a live session and a replay must name the same turn
//!    the same way.
//! 2. **IDEMPOTENCE IS A COLUMN.** It moved out of the drain's ledger and into
//!    the collector: `turns.episode_written`. A row is handed out while the
//!    column says "not written", and the guarded `update ... where {id,
//!    episode_written: {or_null: {eq: 0}}}` rides in the SAME multi-send as the
//!    episode it covers -- what left and what says it left are one emission,
//!    never two decisions. `or_null` because NO insert of this cell names the
//!    column: it arrives by `ALTER TABLE ADD COLUMN`, SQLite fills no default,
//!    so every row reads NULL until the guard sets it -- and NULL is not `0`.
//! 3. **THE LANE SHIPS ON.** After this wave it is the only path from a
//!    conversation into the episodes table, so a shipped default of "off" would
//!    be a shipped agent that remembers nothing. Both spellings of off (`""`
//!    and `"0"`) still work, and the flipped default is asserted rather than
//!    assumed.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No mock, no provider, nothing spent.

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
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts are within a few KB of it).
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
/// merged over them.
fn params_of(over: &[(&str, &str)]) -> Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).unwrap_or_else(|e| panic!("config: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    let mut p = v["params"].as_object().cloned().expect("params object");
    p.remove("script_inline");
    for (k, val) in over {
        assert!(p.contains_key(*k), "no such param: {k}");
        p.insert((*k).to_string(), json!(val));
    }
    Value::Object(p)
}

/// The collector exactly as it ships -- since this wave that means the per-turn
/// lane is ON without anybody switching it.
fn collector(doc: Value) -> Vec<Value> {
    collector_with(&[], doc)
}

fn collector_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut doc = doc;
    doc["params"] = params_of(over);
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
}

/// The collector whose `params` do not mention the knob at all -- the case that
/// falls through to the script's own literal, which is the third of the three
/// places the default lives.
fn collector_without_the_knob(doc: Value) -> Vec<Value> {
    let mut doc = doc;
    let mut p = params_of(&[]);
    p.as_object_mut().expect("params").remove("turn_write");
    doc["params"] = p;
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
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

/// The guarded marks of an emission, in the order they leave.
fn marks(out: &[Value]) -> Vec<&Value> {
    phase_msgs(out, "tw-mark")
}

/// One row of the collector's `turns` table, as the store hands it back.
fn turn_row(id: &str, role: &str, content: &str) -> Value {
    json!({"id": id, "session_id": "s1", "turn_id": "t-".to_string() + id,
           "role": role, "content": content, "recorded_at": "2026-08-23T09:00:0".to_string() + id,
           "interim": 0, "episode_written": 0})
}

/// The store's reply to a select, under the phase the request carried out.
fn select_reply(phase: &str, rows: Value) -> Value {
    json!({
        "header": {
            "hop": {"operation": "select",
                    "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": "s1", "turn_id": "t1",
                        "col_phase": phase, "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-sel",
                      "text": rows.to_string()}]
    })
}

/// The reply a store cell sends back after an insert: the occasion on which the
/// scan is asked for at all.
fn insert_echo(phase: &str) -> Value {
    json!({
        "header": {
            "hop": {"operation": "insert", "rows_affected": 1},
            "context": {"session_id": "s1", "turn_id": "t1",
                        "col_phase": phase, "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-turn-w",
                      "text": "{\"rows_affected\": 1}"}]
    })
}

/// Three things somebody said, in the order of the day.
fn three_turns() -> Value {
    json!([
        turn_row("0", "user", "my editor is helix"),
        turn_row("1", "assistant", "noted"),
        turn_row("2", "user", "and i cook keto"),
    ])
}

// ═════════════════════════════════════════════════════ (a) one message per turn

/// The shape change this task IS: three drainable rows leave as three messages,
/// each carrying exactly one turn, each named by the index it holds in the day.
#[test]
fn every_unwritten_turn_leaves_as_its_own_episode() {
    let out = collector(select_reply("tw-scan", three_turns()));
    let eps = episodes(&out);
    assert_eq!(
        eps.len(),
        3,
        "one message per turn, not one batch -- {out:?}"
    );

    for (i, ep) in eps.iter().enumerate() {
        assert_eq!(
            ep["messages"].as_array().expect("messages").len(),
            1,
            "the writer port reads the FIRST turn and ignores the rest -- {ep:?}"
        );
        assert_eq!(hop(ep, "turn_id"), format!("s1#{i}"));
        assert_eq!(hop(ep, "turn_index"), i.to_string());
        assert_eq!(hop(ep, "session_id"), "s1");
    }

    assert_eq!(
        eps.iter()
            .map(|e| e["messages"][0]["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["my editor is helix", "noted", "and i cook keto"],
        "the day in the order it happened -- {out:?}"
    );

    // The event time of the row, so the writer's bi-temporal split survives the
    // hop instead of collapsing onto the writer's own clock.
    assert_eq!(hop(eps[0], "happened_at"), "2026-08-23T09:00:00");

    // Every hop key present, empty included: a missing key makes the port
    // edge's CEL modifier fail, and a failed modifier SKIPS the edge.
    for key in [
        "route",
        "phase",
        "session_id",
        "iter",
        "turn_id",
        "turn_index",
        "happened_at",
    ] {
        assert!(
            eps[0]["header"].get(key).is_some(),
            "hop key {key} is absent rather than empty -- {:?}",
            eps[0]
        );
    }
}

// ══════════════════════════════════════════════ (b) the mark rides along

/// Idempotence is a column now, and it is written in the SAME emission as the
/// episode it covers: one decision, not two. The guard is what makes two turns
/// in flight safe -- the reason this phase does not simply take the newest row.
#[test]
fn each_episode_carries_its_own_guarded_mark() {
    let out = collector(select_reply("tw-scan", three_turns()));
    let m = marks(&out);
    assert_eq!(m.len(), 3, "one mark per episode -- {out:?}");
    assert_eq!(
        out.len(),
        6,
        "and both halves are ONE multi-send -- {out:?}"
    );

    for (i, mark) in m.iter().enumerate() {
        let a = args(mark);
        assert_eq!(a["operation"], "update");
        assert_eq!(a["table"], "turns");
        assert_eq!(a["set"], json!({"episode_written": 1}));
        // `or_null`, not a plain 0. No insert of this cell names the column,
        // it arrives by `ALTER TABLE ADD COLUMN` and SQLite fills no default,
        // so every row reads NULL until this guard sets it -- and NULL is not
        // 0. A guard comparing to 0 alone matches NO row, and the scan would
        // hand every turn out again for ever (measured: the same turn twice in
        // a live colony, before the guard read or_null).
        assert_eq!(
            a["where"],
            json!({"id": i.to_string(), "episode_written": {"or_null": {"eq": 0}}}),
            "the mark is guarded on the row it covers, NULL included -- {a:?}"
        );
    }
}

// ═══════════════════════════════════════════ (c) a written turn is a skip

/// The second delivery of a scan whose rows are all marked writes nothing at
/// all -- not an empty batch, not a mark, nothing.
#[test]
fn a_day_that_is_written_hands_out_nothing() {
    let mut rows = three_turns();
    for r in rows.as_array_mut().expect("rows") {
        r["episode_written"] = json!(1);
    }
    assert!(
        collector(select_reply("tw-scan", rows)).is_empty(),
        "a written turn is a skip, not a second episode"
    );
}

/// And a session without a single turn is not an empty batch, it is nothing.
#[test]
fn a_session_without_a_single_turn_hands_out_nothing() {
    assert!(collector(select_reply("tw-scan", json!([]))).is_empty());
}

/// The mixed case, which is the one a live session is always in: the rows
/// before the newest turn are marked, the newest is not, and exactly it leaves
/// -- under the index it holds in the WHOLE day, never a fresh count.
#[test]
fn only_the_unwritten_turn_leaves_and_it_keeps_its_index() {
    let mut rows = three_turns();
    rows[0]["episode_written"] = json!(1);
    rows[1]["episode_written"] = json!(1);

    let out = collector(select_reply("tw-scan", rows));
    let eps = episodes(&out);
    assert_eq!(eps.len(), 1, "one turn was unwritten -- {out:?}");
    assert_eq!(
        hop(eps[0], "turn_id"),
        "s1#2",
        "the index counts the day, not this delivery -- {out:?}"
    );
    assert_eq!(marks(&out).len(), 1, "{out:?}");
}

// ══════════════════════════════════ (d) what is not a turn is not counted

/// The subtlest regression this task can produce: an interim sentence and an
/// advice row are neither handed out NOR counted, so the turn after them keeps
/// the index it would have had without them. An index that shifts here means
/// the same turn is written twice under two names -- once live, once by a
/// replay that counts differently.
#[test]
fn an_interim_and_an_advice_row_are_neither_emitted_nor_counted() {
    let mut interim = turn_row("1", "assistant", "One moment, I'm thinking about that.");
    interim["interim"] = json!(1);
    let rows = json!([
        turn_row("0", "user", "whats the weather"),
        interim,
        turn_row("2", "advice", "Reliably stored are four things."),
        turn_row("3", "assistant", "berlin, 14 degrees"),
    ]);

    let out = collector(select_reply("tw-scan", rows));
    let eps = episodes(&out);
    assert_eq!(eps.len(), 2, "two of four are turns -- {out:?}");
    assert_eq!(
        eps.iter()
            .map(|e| e["messages"][0]["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["whats the weather", "berlin, 14 degrees"]
    );
    assert_eq!(hop(eps[0], "turn_id"), "s1#0");
    assert_eq!(
        hop(eps[1], "turn_id"),
        "s1#1",
        "the index counts turns, never rows -- {out:?}"
    );

    // And nothing is marked that never left: the two refused rows keep their
    // column at 0 for ever, which is correct -- they are not episodes.
    assert_eq!(
        marks(&out)
            .iter()
            .map(|m| args(m)["where"]["id"].clone())
            .collect::<Vec<_>>(),
        vec![json!("0"), json!("3")],
        "{out:?}"
    );
}

// ═════════════════════════════════════════════ (e) who said it, explicitly

/// `origin` comes from the explicit `{"user": "user", "assistant": "assistant"}`
/// mapping of GH #282, applied after the filter has already refused everything
/// else -- so there is no role an episode can attribute to a speaker who did
/// not speak.
#[test]
fn every_episode_is_attributed_to_the_role_of_its_own_row() {
    let out = collector(select_reply("tw-scan", three_turns()));
    assert_eq!(
        episodes(&out)
            .iter()
            .map(|e| e["messages"][0]["origin"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "user"],
        "origin is the row's role, not a fallback -- {out:?}"
    );
}

// ══════════════════════════════════════════════════ (f) the flipped default

/// Off is still off, in both spellings -- an instance that wires no episode
/// consumer must not emit a route nobody routes.
#[test]
fn both_spellings_of_off_keep_the_lane_silent() {
    for off in ["", "0"] {
        let out = collector_with(&[("turn_write", off)], insert_echo("turn-w"));
        assert!(
            phase_msgs(&out, "tw-scan").is_empty(),
            "turn_write={off:?} must not scan -- {out:?}"
        );
        assert_eq!(
            phase_msgs(&out, "turn-open").len(),
            1,
            "and the round check is untouched -- {out:?}"
        );
        assert!(
            collector_with(&[("turn_write", off)], insert_echo("ans-w")).is_empty(),
            "turn_write={off:?}: the answer echo stays a dead end"
        );
    }
}

/// And the default is ON -- asserted, not assumed. After this wave the per-turn
/// lane is the only path from a conversation into the episodes table, so a
/// shipped "off" is a shipped agent that remembers nothing.
#[test]
fn the_shipped_default_writes_episodes() {
    for occasion in ["turn-w", "ans-w"] {
        let shipped = collector(insert_echo(occasion));
        assert_eq!(
            phase_msgs(&shipped, "tw-scan").len(),
            1,
            "the shipped params scan on {occasion} -- {shipped:?}"
        );
        let bare = collector_without_the_knob(insert_echo(occasion));
        assert_eq!(
            phase_msgs(&bare, "tw-scan").len(),
            1,
            "and so does a params object that never mentions the knob -- {bare:?}"
        );
    }
}

/// The scan asks for the columns it decides on. A column that is not selected
/// is a column that is not there -- the filter and the guard would both read
/// their evidence as absent.
#[test]
fn the_scan_selects_the_evidence_it_decides_on() {
    let out = collector(insert_echo("turn-w"));
    let scan = phase_msgs(&out, "tw-scan");
    assert_eq!(scan.len(), 1, "{out:?}");
    let a = args(scan[0]);
    assert_eq!(a["operation"], "select");
    assert_eq!(a["table"], "turns");
    assert_eq!(a["where"], json!({"session_id": "s1"}));
    assert_eq!(
        a["order_by"],
        json!([{"col": "id", "dir": "asc"}]),
        "the order of the day IS the index an episode id is minted from"
    );
    assert!(
        a.get("limit").is_none(),
        "a window is capped, a memory is not -- a limit here would shift every index"
    );
    let cols = a["columns"].as_array().expect("columns").clone();
    for col in [
        "id",
        "role",
        "content",
        "interim",
        "recorded_at",
        "episode_written",
    ] {
        assert!(
            cols.contains(&json!(col)),
            "the scan does not select {col} -- {cols:?}"
        );
    }
}
