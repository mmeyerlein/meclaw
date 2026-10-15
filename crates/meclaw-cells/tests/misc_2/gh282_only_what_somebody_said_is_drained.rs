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
//! GH #889 (R-27-1) moved both writers out of the collector: the per-turn
//! episodes (`turn_write`) and the close batch (`write`) are written by the
//! curator's writer now, out of its own ledger. The half of #282 that is about
//! the MEMORY -- the interim and the advisor's answer are neither an episode
//! nor a line of the close batch, and take no episode index -- went with them
//! and is the curator's to pin (`curator@1.0.0`, writer). The collector no
//! longer stores an answer at all, so there is no `interim` column left to
//! stamp.
//!
//! What stays here is the negative control, because it is a promise of the
//! collector's own seam: the fix must not blind the PROMPT. An advice turn of
//! the running round still reaches `curate` as something said, framed, with
//! its correlation id exposed.
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

/// The collector with whatever the case needs to turn. The per-turn write lane
/// it used to switch on first (`turn_write`) left the collector with GH #889 --
/// the curator's writer carries that knob now.
fn collector_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut doc = doc;
    doc["params"] = params_of(ASSEMBLE_CONFIG, over);
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
}

fn hop(m: &Value, key: &str) -> String {
    m["header"][key].as_str().unwrap_or_default().to_string()
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

/// The advisor's answer as it comes back on `in_advice` -- itself a rendering
/// of a previous recall, which is what makes it the closed loop of #282.
const ADVICE: &str = "Reliably stored are four things: name, editor, city, timezone.";

/// The store's reply to the `collect` read-back. GH #419: the phase that used
/// to be a fan-in is ONE bundle -- the leg parks and the table is read back in
/// the same message, so the fixture puts its rows under the read-back's
/// `tool_call_id`.
fn collect_reply(turn: &str, rows: Value) -> Value {
    let cid = "c-collect-read";
    json!({
        "header": {
            "hop": {"operation": "bundle", "bundle_errors": 0,
                    "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": "s1", "turn_id": turn,
                        "col_phase": "collect", "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": cid, "operation": "select", "duration_ms": 0,
                     "rows_affected": rows.as_array().map_or(0, Vec::len)}]
    })
}

// ═════════════════════════════════════════════════════ THE NEGATIVE CONTROL

/// A `leg-window` row as the `win` step writes it -- the prompt's own reading
/// of the round, which is NOT the memory's. Since GH #889 the leg carries the
/// round's turns and the deferral marks only: no byte count, no drop or cap
/// marks, because the collector no longer cuts anything.
fn leg_window_row(turns: Value) -> Value {
    let payload = json!({"turns": turns, "deferred": 0, "deferred_turns": []});
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
    let out = collector_with(&[("memory_tier", "0")], collect_reply("t1", rows));
    // GH #889: the seam is route `curate` (to the curator, which owns the
    // window) where it was `brain`; the round it carries is the same.
    let seam = out
        .iter()
        .find(|m| hop(m, "route") == "curate" || hop(m, "route") == "answer")
        .expect("the seam");
    assert!(
        texts(seam).iter().any(|t| t.ends_with(ADVICE)),
        "the advice turn stays in the prompt -- {seam:?}"
    );
    // Since GH #540 it stays there SAYING what it is: the frame is what the
    // model reads instead of the role, and the role alone was what made an
    // advisor's answer indistinguishable from a new sentence by the person.
    assert!(
        texts(seam)
            .iter()
            .any(|t| t == &format!("[advice from your reasoning core, consult c-42]\n{ADVICE}")),
        "and it is framed as an event of the agent's own machinery -- {seam:?}"
    );
    assert_eq!(
        seam["system"]["consult"]["open"],
        json!(["c-42"]),
        "and it still exposes its correlation id -- {seam:?}"
    );
}
