//! meclaw-os -- the summarizer hive turns a closed session into a handover (GH #100).
//!
//! When a session generation closes, the collector hands the whole day out as
//! ONE batch on route `write` (R-OS-6). The summarizer is the step behind that
//! batch: a glue cell shapes the day into a recency-weighted prompt, an llm
//! cell writes the summary, and the same glue cell forms the answer into ONE
//! `system.handover` update -- the fresh memory the NEXT generation wakes up
//! with (R-OS-3: more from the last turns, phase-out before, hard facts kept).
//!
//! Three claims are pinned here, all against the SHIPPED `script_inline` and
//! real stdin documents (nothing mocked, nothing spent):
//!
//! 1. PROMPT -- the batch becomes exactly one prompt for the writer, and the
//!    recency weighting is structure, not hope: the newest turns travel
//!    verbatim, older turns are cut to a phase-out preview and counted, tool
//!    rounds enter as capped context, bookkeeping rows do not enter at all.
//! 2. UPDATE -- the writer's answer becomes exactly one emission on route
//!    `summary`, whose body is nothing but the `system.handover` update an
//!    llm cell consumes without a provider call. The summarizer is the ONLY
//!    writer of that slot (R-OS-1 slot discipline).
//! 3. DEGRADATION -- a failed call leaves on `summary_error` instead of being
//!    swallowed, and an empty answer is an error too: an empty update would
//!    REPLACE a real handover with nothing.

use std::io::Write;
use std::process::{Command, Stdio};

const PREP_CONFIG: &str = "../../templates/summarizer/prep/config.json";

/// The shipped script, verbatim.
///
/// There is nothing left to substitute: since `summarizer@2.1.0` the four
/// weighting knobs of `./prep` are params of that cell rather than substitution
/// tokens (GH #138), so a case that wants a different weighting hands one down
/// on the stdin document's `params` object -- the same object an
/// `override_params` entry fills at instantiation.
fn prep_script() -> String {
    let raw = std::fs::read_to_string(PREP_CONFIG).expect("prep config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
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

/// Run the real script against a real stdin document and return the emitted
/// messages.
fn emit_with(params: serde_json::Value, mut doc: serde_json::Value) -> Vec<serde_json::Value> {
    doc["params"] = params;
    let out = run_script_on_stdin(
        &prep_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "prep exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_with(serde_json::json!({}), doc)
}

fn turn(origin: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"origin": origin, "type": "text", "text": text})
}

/// The write batch exactly as the collector's close lane emits it and the
/// parent port edge renames it: `messages[]` all turns of the day in order,
/// the raw round rows in the top-level slot `rounds`, the session and its
/// sizes on the hop (c3 batch form).
fn batch_doc(turns: Vec<serde_json::Value>, rounds: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {},
                   "hop": {"route": "in_batch", "session_id": "s1",
                           "turn_count": turns.len().to_string(),
                           "round_count": rounds.as_array().map(|a| a.len()).unwrap_or(0).to_string()}},
        "messages": turns,
        "rounds": rounds
    })
}

/// The writer's answer as the internal edge delivers it back: the lane on the
/// hop, the session in context (promoted by the prep -> writer edge).
fn answer_doc(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1"},
                   "hop": {"route": "in_answer", "finish_reason": "stop"}},
        "messages": [turn("assistant", text)]
    })
}

fn prompt_of(msg: &serde_json::Value) -> String {
    msg["messages"][0]["text"]
        .as_str()
        .expect("prompt text")
        .to_string()
}

// ====================================================================== PROMPT

#[test]
fn a_write_batch_becomes_one_prompt_for_the_writer() {
    let out = emit(batch_doc(
        vec![
            turn("user", "my editor is helix"),
            turn("assistant", "noted: helix"),
            turn("user", "and my shell is fish"),
        ],
        serde_json::json!([]),
    ));
    assert_eq!(out.len(), 1, "one batch, one prompt: {out:?}");
    assert_eq!(out[0]["header"]["route"], "llm");
    assert_eq!(out[0]["header"]["session_id"], "s1");
    assert_eq!(out[0]["header"]["turn_count"], "3");
    // The prompt lives in the glue phase, not in the llm config (P15 lesson):
    // the instructions ride as a system update next to the user document.
    let instructions = out[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions text");
    assert!(!instructions.is_empty());
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 1, "one prompt document");
    assert_eq!(msgs[0]["origin"], "user");
    let prompt = prompt_of(&out[0]);
    assert!(
        prompt.contains("Session s1 closed with 3 turns."),
        "{prompt}"
    );
    for text in ["my editor is helix", "noted: helix", "and my shell is fish"] {
        assert!(
            prompt.contains(text),
            "every turn is in the prompt: {prompt}"
        );
    }
}

#[test]
fn the_prompt_keeps_recent_turns_verbatim_and_condenses_older_ones() {
    let out = emit_with(
        serde_json::json!({"recent_turns": 2, "phaseout_chars": 10}),
        batch_doc(
            vec![
                turn("user", "alpha-0123456789-ALPHA-TAIL"),
                turn("assistant", "beta-0123456789-BETA-TAIL"),
                turn("user", "gamma-recent"),
                turn("assistant", "delta-recent"),
            ],
            serde_json::json!([]),
        ),
    );
    assert_eq!(out.len(), 1);
    let prompt = prompt_of(&out[0]);
    // The newest turns travel whole -- they are what the next generation must
    // wake up remembering best (R-OS-3).
    assert!(prompt.contains("gamma-recent"), "{prompt}");
    assert!(prompt.contains("delta-recent"), "{prompt}");
    // Older turns phase out: a bounded preview, counted, never the full text.
    assert!(
        prompt.contains("alpha-0123"),
        "the preview survives: {prompt}"
    );
    assert!(
        !prompt.contains("ALPHA-TAIL"),
        "the tail of an old turn is cut: {prompt}"
    );
    assert!(!prompt.contains("BETA-TAIL"), "{prompt}");
    assert!(
        prompt.contains("2 older turn"),
        "what was condensed is counted, not hidden: {prompt}"
    );
    // And the weighting is an ORDER: old material first, recent material last,
    // closest to where the model starts writing.
    let older = prompt.find("alpha-0123").expect("older section");
    let recent = prompt.find("gamma-recent").expect("recent section");
    assert!(
        older < recent,
        "phase-out before the verbatim tail: {prompt}"
    );
}

#[test]
fn the_instructions_demand_honesty_instead_of_invention() {
    // The empty session is the trap the instructions must disarm: a summarizer
    // that invents a day poisons the next generation's memory (Nordstern:
    // never wrong, degraded is fine).
    let out = emit(batch_doc(vec![], serde_json::json!([])));
    assert_eq!(out.len(), 1, "an empty session still gets its summary call");
    let instructions = out[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        instructions.contains("never invent"),
        "the honesty sentence is part of the shipped prompt: {instructions}"
    );
    assert!(
        instructions.contains("short, honest summary"),
        "an empty or short session yields a short honest summary: {instructions}"
    );
    assert!(
        instructions.contains("hard facts"),
        "hard facts are kept even from the oldest turns: {instructions}"
    );
    let prompt = prompt_of(&out[0]);
    assert!(
        prompt.contains("Session s1 closed with 0 turns."),
        "the prompt does not pretend there was a day: {prompt}"
    );
}

#[test]
fn tool_rounds_enter_the_prompt_capped_and_bookkeeping_rows_do_not() {
    let rounds = serde_json::json!([
        {"turn_id": "t1", "iter": 0, "role": "leg-window",
         "turn": {"turns": [{"role": "user", "text": "EVICTION-MARKER"}]}, "fired": 1},
        {"turn_id": "t1", "iter": 0, "role": "assistant",
         "turn": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                   "text": "look-0123456789-CALL-TAIL"}], "fired": 1},
        {"turn_id": "t1", "iter": 0, "role": "tool",
         "turn": {"origin": "tool", "type": "tool_result", "id": "c1",
                  "text": "result-0123456789-RESULT-TAIL"}, "fired": 1}
    ]);
    let out = emit_with(
        serde_json::json!({"tool_chars": 15}),
        batch_doc(vec![turn("user", "look it up")], rounds),
    );
    assert_eq!(out.len(), 1);
    let prompt = prompt_of(&out[0]);
    assert!(prompt.contains("call c1"), "the round's question: {prompt}");
    assert!(prompt.contains("result c1"), "the round's answer: {prompt}");
    assert!(
        prompt.contains("result-01234567"),
        "a capped preview of the result: {prompt}"
    );
    assert!(
        !prompt.contains("RESULT-TAIL"),
        "the cap holds on tool texts: {prompt}"
    );
    assert!(
        !prompt.contains("EVICTION-MARKER"),
        "leg-* rows are the collector's bookkeeping, not session content: {prompt}"
    );
}

#[test]
fn the_session_rides_on_the_hop_or_in_context() {
    // The collector stamps the session on the hop of the write batch; a parent
    // edge MAY additionally promote it to context. Either one is enough.
    let mut doc = batch_doc(vec![turn("user", "hi")], serde_json::json!([]));
    doc["header"]["hop"]
        .as_object_mut()
        .unwrap()
        .remove("session_id");
    doc["header"]["context"]["session_id"] = serde_json::json!("s7");
    let out = emit(doc);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["session_id"], "s7");
    assert!(
        prompt_of(&out[0]).contains("Session s7"),
        "context fallback"
    );
}

// ====================================================================== UPDATE

#[test]
fn the_answer_becomes_one_system_handover_update() {
    let out = emit(answer_doc("The user prefers helix and fish."));
    assert_eq!(out.len(), 1, "one summary, one emission: {out:?}");
    assert_eq!(out[0]["header"]["route"], "summary");
    assert_eq!(out[0]["header"]["session_id"], "s1");
    assert_eq!(out[0]["header"]["summary_chars"], "32");
    // The body IS the update: exactly what an llm cell consumes as a
    // `system.*` accumulation without a provider call, on the one slot this
    // hive owns (R-OS-1: one writer per system path).
    assert_eq!(
        out[0]["system"]["handover"]["text"],
        "The user prefers helix and fish."
    );
    assert!(
        out[0].get("messages").is_none(),
        "a system update carries no messages[] -- messages would trigger \
         an inference: {:?}",
        out[0]
    );
}

// ================================================================= DEGRADATION

#[test]
fn a_provider_error_leaves_on_summary_error() {
    let out = emit(serde_json::json!({
        "header": {"context": {"session_id": "s1"},
                   "hop": {"route": "in_error", "finish_reason": "error",
                           "error_code": "provider_error"}},
        "messages": [turn("user", "the prompt travels back unchanged")]
    }));
    assert_eq!(out.len(), 1, "a failure is handed on, never swallowed");
    assert_eq!(out[0]["header"]["route"], "summary_error");
    assert_eq!(out[0]["header"]["session_id"], "s1");
    assert_eq!(out[0]["header"]["error_code"], "provider_error");
    let text = out[0]["messages"][0]["text"].as_str().expect("detail");
    assert!(text.contains("s1"), "the report names the session: {text}");
    assert!(text.contains("provider_error"), "and the cause: {text}");
}

#[test]
fn an_empty_answer_is_an_error_not_an_empty_handover() {
    // system.* is accumulate-replace per path: an empty update would REPLACE
    // a real handover with nothing. Rather no update and a loud lane.
    let out = emit(answer_doc("   "));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "summary_error");
    assert_eq!(out[0]["header"]["error_code"], "empty_summary");
    assert!(
        out[0].get("system").is_none(),
        "no handover slot leaves on the error lane: {:?}",
        out[0]
    );
}

#[test]
fn an_unrelated_document_emits_nothing() {
    let out = emit(serde_json::json!({
        "header": {"context": {}, "hop": {"route": "bogus"}},
        "messages": [turn("user", "stray")]
    }));
    assert!(
        out.is_empty(),
        "terminal by design: an unknown lane parks, it does not guess: {out:?}"
    );
}
