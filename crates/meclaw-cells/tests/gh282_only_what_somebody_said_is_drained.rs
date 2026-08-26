//! meclaw-os -- an interim sentence is not a turn of the conversation (GH #282).
//!
//! "One moment, I'm thinking about that." is an ANSWER on the wire: the advisor
//! split (R-CG-3) sends it so the channel has something to say while the real
//! reply is still being worked out, and it enters the context window like every
//! other turn, because the model must know it said it. What it is NOT is
//! something anybody told anybody -- and since wave 9 the write path hands the
//! day out after every stored turn, so that sentence became an EPISODE, once
//! per deferred turn, in a memory that is supposed to hold what was said.
//!
//! The marker existed already, but only on the wire (`hop.interim=1` on the
//! outgoing answer). The stored row did not carry it, and the stored row is
//! what both drain lanes read. This test pins the whole path:
//!
//! 1. STAMPED -- the `in_answer` lane writes `interim` into the `turns` row,
//!    from the same condition that marks the outgoing answer.
//! 2. FILTERED -- the per-turn lane (`tw-scan`) leaves the interim row behind.
//!    Since ruling Q11 (GH #298) that lane hands out ONE message per turn
//!    instead of the day as one batch, so what is asserted below is that the
//!    refused rows produce no message AND take no index: an index that shifted
//!    would write the same turn twice under two names.
//! 3. CARRIED -- the close lane selects the column and parks it with the leg,
//!    so the close batch filters on the same evidence.
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

/// The collector with the per-turn write lane switched ON -- the lane this
/// issue is about -- plus whatever else the case needs to turn.
fn collector_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut doc = doc;
    let mut all = vec![("turn_write", "1")];
    all.extend_from_slice(over);
    doc["params"] = params_of(ASSEMBLE_CONFIG, &all);
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
}

/// The collector with the per-turn write lane switched ON -- the lane this
/// issue is about.
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

/// Texts of a batch, in the order they leave.
fn texts(m: &Value) -> Vec<String> {
    m["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|t| t["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Origins of a batch, in the order they leave -- the attribution #282 is
/// about: who, in the memory's reading, said the thing.
fn origins(m: &Value) -> Vec<String> {
    m["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|t| t["origin"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The episodes of an emission, in the order they leave -- one message per turn
/// since Q11.
fn episodes(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| hop(m, "route") == "turn_write")
        .collect()
}

/// The conversation the per-turn lane hands out, read back off the one-turn
/// messages: what a memory made of these episodes would hold.
fn episode_texts(out: &[Value]) -> Vec<String> {
    episodes(out).iter().flat_map(|m| texts(m)).collect()
}

/// Who each of those episodes is attributed to.
fn episode_origins(out: &[Value]) -> Vec<String> {
    episodes(out).iter().flat_map(|m| origins(m)).collect()
}

/// The deterministic ids the lane minted, in order -- the assertion that says
/// a refused row took no index.
fn episode_ids(out: &[Value]) -> Vec<String> {
    episodes(out).iter().map(|m| hop(m, "turn_id")).collect()
}

const INTERIM: &str = "One moment, I'm thinking about that.";

/// The advisor's answer as it comes back on `in_advice` -- itself a rendering
/// of a previous recall, which is what makes it the closed loop of #282.
const ADVICE: &str = "Reliably stored are four things: name, editor, city, timezone.";

/// An answer arriving on the answer lane, with or without the interim marker
/// the advisor split puts on the hop.
fn answer_doc(interim: bool) -> Value {
    let mut hop = json!({"route": "in_answer"});
    if interim {
        hop["interim"] = json!("1");
    }
    json!({
        "header": {"hop": hop, "context": {"session_id": "s1", "turn_id": "t1"}},
        "messages": [{"origin": "assistant", "type": "text",
                      "text": if interim { INTERIM } else { "berlin, 14 degrees" }}]
    })
}

/// One row of the collector's `turns` table, as the store hands it back.
fn turn_row(id: &str, role: &str, content: &str, interim: i64) -> Value {
    json!({"id": id, "session_id": "s1", "turn_id": "t-".to_string() + id,
           "role": role, "content": content, "recorded_at": id, "interim": interim})
}

/// The store's reply to a select, under the phase the request carried out.
fn select_reply(phase: &str, turn: &str, rows: Value) -> Value {
    // GH #419: the phases that used to be a fan-in are ONE bundle -- the leg
    // parks and the table is read back in the same message. So a fixture for one
    // of them puts its rows under the read-back's `tool_call_id`; every other
    // phase keeps the single-op shape it always had.
    let cid = match phase {
        "collect" => "c-collect-read",
        "round-check" => "c-round-check-read",
        "close-fire" => "c-close-read",
        _ => "c-sel",
    };
    if cid == "c-sel" {
        return json!({
            "header": {
                "hop": {"operation": "select",
                        "rows_affected": rows.as_array().map_or(0, Vec::len)},
                "context": {"session_id": "s1", "turn_id": turn,
                            "col_phase": phase, "store_origin": "collector"}
            },
            "messages": [{"origin": "tool", "type": "tool_result", "id": "c-sel",
                          "text": rows.to_string()}]
        });
    }
    json!({
        "header": {
            "hop": {"operation": "bundle", "bundle_errors": 0,
                    "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": "s1", "turn_id": turn,
                        "col_phase": phase, "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": cid, "operation": "select", "duration_ms": 0,
                     "rows_affected": rows.as_array().map_or(0, Vec::len)}]
    })
}

/// A day whose second row is the interim sentence: two real turns around it,
/// and the answer that eventually came.
fn day_with_interim() -> Value {
    json!([
        turn_row("2026-08-23-00", "user", "whats the weather", 0),
        turn_row("2026-08-23-01", "assistant", INTERIM, 1),
        turn_row("2026-08-23-02", "assistant", "berlin, 14 degrees", 0),
    ])
}

/// A day whose second row is the advisor's answer: the `in_advice` lane writes
/// it with `role = "advice"`, and it sits in the same table between two things
/// somebody actually said.
fn day_with_advice() -> Value {
    json!([
        turn_row("2026-08-23-00", "user", "what do you remember about me", 0),
        turn_row("2026-08-23-01", "advice", ADVICE, 0),
        turn_row("2026-08-23-02", "assistant", "four things, apparently", 0),
    ])
}

// ═════════════════════════════════════════════════════════════════ 1. STAMPED

/// The marker was on the wire and nowhere else. It belongs in the ROW, because
/// the row is what both drain lanes read -- that gap is the whole issue.
#[test]
fn an_interim_answer_is_stored_as_an_interim_row() {
    let out = collector(answer_doc(true));
    let w = phase_msgs(&out, "ans-w");
    assert_eq!(
        w.len(),
        1,
        "the answer is stored, interim or not -- {out:?}"
    );
    assert_eq!(
        args(w[0])["row"]["interim"],
        json!(1),
        "the stored row says what the wire says -- {out:?}"
    );

    // Unchanged: the answer still leaves on the answer lane, still marked.
    let answer = out
        .iter()
        .find(|m| hop(m, "route") == "answer")
        .expect("the answer");
    assert_eq!(hop(answer, "interim"), "1");
}

/// And a real answer is stamped clear -- explicitly, not by absence: the drain
/// filter compares against `0`, and a column that is only ever written on the
/// interim path would make every real answer depend on a default.
#[test]
fn a_real_answer_is_stored_with_the_mark_cleared() {
    let out = collector(answer_doc(false));
    let w = phase_msgs(&out, "ans-w");
    assert_eq!(w.len(), 1, "{out:?}");
    assert_eq!(args(w[0])["row"]["interim"], json!(0), "{out:?}");
}

// ════════════════════════════════════════════════════════════════ 2. FILTERED

/// The point of the issue: what the per-turn lane hands out is the two things
/// somebody said, and not the sentence that only bought time.
#[test]
fn the_per_turn_lane_leaves_the_interim_sentence_behind() {
    let out = collector(select_reply("tw-scan", "t1", day_with_interim()));
    assert_eq!(
        episode_texts(&out),
        vec!["whats the weather", "berlin, 14 degrees"],
        "an interim sentence is not a turn of the conversation -- {out:?}"
    );
    assert_eq!(
        episode_ids(&out),
        vec!["s1#0", "s1#1"],
        "and it takes no index either: the index counts turns, never rows -- {out:?}"
    );
}

// ═════════════════════════════════════════════════════════════════ 3. CARRIED

/// The close lane reads the same table and must read the same evidence: the
/// column travels in the select and rides on into the parked leg, because the
/// script keeps no state between hops.
#[test]
fn the_close_lane_selects_the_column_and_parks_it() {
    let ask = collector(json!({
        "header": {"hop": {"route": "in_close"},
                   "context": {"session_id": "s1"}},
        "messages": []
    }));
    assert_eq!(ask.len(), 1, "{ask:?}");
    let cols = args(&ask[0])["columns"].clone();
    assert!(
        cols.as_array()
            .expect("columns")
            .contains(&json!("interim")),
        "the close read carries the evidence it will filter on -- {cols:?}"
    );

    let parked = collector(select_reply("close-turns", "close-x", day_with_interim()));
    let leg: Value = meclaw_core::serde_json::from_str(
        args(&parked[0])["row"]["turn"].as_str().expect("the leg"),
    )
    .expect("the leg is json");
    assert_eq!(
        leg.as_array()
            .expect("turns")
            .iter()
            .map(|t| t["interim"].clone())
            .collect::<Vec<_>>(),
        vec![json!(0), json!(1), json!(0)],
        "each parked turn keeps its own mark -- {leg:?}"
    );
}

/// And the close batch -- the safety net over the whole day -- hands out the
/// same conversation the per-turn lane does. If it did not, the net would put
/// back exactly what the per-turn lane refused.
#[test]
fn the_close_batch_hands_out_the_same_conversation() {
    let parked = collector(select_reply("close-turns", "close-x", day_with_interim()));
    let leg = args(&parked[0])["row"].clone();
    let out = collector(select_reply(
        "close-fire",
        "close-x",
        json!([{"turn_id": leg["turn_id"], "iter": 0, "role": "leg-close",
                "turn": leg["turn"], "fired": 0, "recorded_at": "2026-08-23"}]),
    ));
    let batch = out
        .iter()
        .find(|m| hop(m, "route") == "write")
        .expect("the close batch");
    assert_eq!(
        texts(batch),
        vec!["whats the weather", "berlin, 14 degrees"],
        "the net catches turns, not filler -- {batch:?}"
    );
    assert_eq!(hop(batch, "turn_count"), "2");
}

// ═══════════════════════════════════════════════════════════════ 4. ATTRIBUTED

/// The other half of #282, measured as `sender=user` on a row reading
/// "Reliably stored are four things: ...": the advisor's answer is not the user
/// speaking. It is the hive's own recall, rendered by the advisor and handed
/// back on `in_advice` -- a memory that drains it ingests its own answers and
/// signs them with the wrong name.
#[test]
fn the_advisors_answer_is_not_a_turn_of_the_conversation() {
    let out = collector(select_reply("tw-scan", "t1", day_with_advice()));
    assert_eq!(
        episode_texts(&out),
        vec!["what do you remember about me", "four things, apparently"],
        "the advisor's answer is not something somebody said -- {out:?}"
    );
    assert_eq!(episode_ids(&out), vec!["s1#0", "s1#1"], "{out:?}");
}

/// And what does leave carries the attribution of its own row -- read from an
/// explicit mapping, never from a fallback that calls everything it does not
/// recognise `user`.
#[test]
fn the_per_turn_lane_attributes_every_turn_to_its_own_role() {
    let out = collector(select_reply("tw-scan", "t1", day_with_advice()));
    assert_eq!(
        episode_origins(&out),
        vec!["user", "assistant"],
        "origin is the row's role, not a fallback -- {out:?}"
    );
}

/// The close lane reads the same table and must hand out the same
/// conversation: a safety net that puts back what the per-turn lane refused is
/// not a net.
#[test]
fn the_close_batch_leaves_the_advisors_answer_behind() {
    let parked = collector(select_reply("close-turns", "close-x", day_with_advice()));
    let leg = args(&parked[0])["row"].clone();
    let out = collector(select_reply(
        "close-fire",
        "close-x",
        json!([{"turn_id": leg["turn_id"], "iter": 0, "role": "leg-close",
                "turn": leg["turn"], "fired": 0, "recorded_at": "2026-08-23"}]),
    ));
    let batch = out
        .iter()
        .find(|m| hop(m, "route") == "write")
        .expect("the close batch");
    assert_eq!(
        texts(batch),
        vec!["what do you remember about me", "four things, apparently"],
        "the net catches turns, not the advisor's answer -- {batch:?}"
    );
    assert_eq!(
        origins(batch),
        vec!["user", "assistant"],
        "same mapping on both lanes -- {batch:?}"
    );
    assert_eq!(hop(batch, "turn_count"), "2");
}

/// A day whose middle row carries NO role at all -- a row nobody writes today,
/// which is exactly why the two lanes were free to disagree about it.
fn day_with_a_roleless_row() -> Value {
    json!([
        turn_row("2026-08-23-00", "user", "what do you remember about me", 0),
        {"id": "2026-08-23-01", "session_id": "s1", "turn_id": "t-01",
         "content": "a row nobody wrote", "recorded_at": "2026-08-23-01",
         "interim": 0},
        turn_row("2026-08-23-02", "assistant", "four things, apparently", 0),
    ])
}

/// The invention sat UPSTREAM of the mapping: the close lane built its parked
/// leg with `role or "user"`, so a roleless row reached the filter already
/// labelled `user`, passed it, and was drained as something the user said --
/// while the per-turn lane, which filters the raw rows, dropped the same row.
/// Two lanes, two answers, over a row nobody wrote. Both lanes now ask the one
/// filter, and the filter sees what the table holds.
#[test]
fn a_row_without_a_role_is_dropped_by_both_lanes() {
    let out = collector(select_reply("tw-scan", "t1", day_with_a_roleless_row()));
    assert_eq!(
        episode_texts(&out),
        vec!["what do you remember about me", "four things, apparently"],
        "the per-turn lane never invented a speaker -- {out:?}"
    );

    let parked = collector(select_reply(
        "close-turns",
        "close-x",
        day_with_a_roleless_row(),
    ));
    let leg = args(&parked[0])["row"].clone();
    let parsed: Value =
        meclaw_core::serde_json::from_str(leg["turn"].as_str().expect("the leg")).expect("json");
    assert_eq!(
        parsed[1]["role"],
        json!(""),
        "the parked leg carries the role the row had, empty included -- {parsed:?}"
    );

    let out = collector(select_reply(
        "close-fire",
        "close-x",
        json!([{"turn_id": leg["turn_id"], "iter": 0, "role": "leg-close",
                "turn": leg["turn"], "fired": 0, "recorded_at": "2026-08-23"}]),
    ));
    let batch = out
        .iter()
        .find(|m| hop(m, "route") == "write")
        .expect("the close batch");
    assert_eq!(
        texts(batch),
        vec!["what do you remember about me", "four things, apparently"],
        "and neither does the close lane -- {batch:?}"
    );
    assert_eq!(
        origins(batch),
        vec!["user", "assistant"],
        "nothing is signed with a name the row did not carry -- {batch:?}"
    );
    assert_eq!(hop(batch, "turn_count"), "2");
}

// ══════════════════════════════════════════════════ 4b. THE NEGATIVE CONTROL

/// A `leg-window` row as the `win` step writes it -- the prompt's own reading
/// of the day, which is NOT the memory's.
fn leg_window_row(turns: Value) -> Value {
    let payload = json!({"turns": turns, "bytes": 0, "dropped": 0, "capped": 0});
    json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
           "turn": payload.to_string(), "fired": 0})
}

/// The window is not the memory. Inside the PROMPT an advice turn should read
/// as something said -- the model has to see what the advisor came back with --
/// and its correlation id is the reply half of the bilateral lane (R-CG-3).
/// The fix must not blind the prompt.
#[test]
fn the_prompt_window_still_shows_the_advice_turn_and_its_consult_id() {
    // Both declared legs, because the assembly waits for both: with a memory
    // tier configured, a window-only slate is an incomplete round and terminal.
    // Before GH #419 this fixture reached the rendering phase directly, past the
    // gate; now the rendering IS the reply the completeness was read out of.
    let rows = json!([
        leg_window_row(json!([
            {"role": "user", "text": "what do you remember about me", "consult_id": ""},
            {"role": "advice", "text": ADVICE, "consult_id": "c-42"},
        ])),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "fired": 0,
         "turn": json!({"system": {}, "messages": []}).to_string()}
    ]);
    let out = collector_with(&[("memory_tier", "0")], select_reply("collect", "t1", rows));
    let seam = out
        .iter()
        .find(|m| hop(m, "route") == "brain" || hop(m, "route") == "answer")
        .expect("the seam");
    assert!(
        texts(seam).contains(&ADVICE.to_string()),
        "the advice turn stays in the prompt -- {seam:?}"
    );
    assert_eq!(
        seam["system"]["consult"]["open"],
        json!(["c-42"]),
        "and it still exposes its correlation id -- {seam:?}"
    );
}
