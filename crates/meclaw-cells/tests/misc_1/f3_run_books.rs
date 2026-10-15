//! 0.3.x follow-up F3 -- the books of a night count every model call (GitHub #64).
//!
//! Observed on the first canonical dream run of a production colony: the night
//! made TWO model calls, the dreamer and the canonicalisation judge, and
//! `consolidation_log.llm_calls` said 1. The judge's call sat in the provider
//! usage with its prompt and completion tokens known, and never reached the
//! books -- so the one number a colony has for "what did last night cost" was
//! wrong by exactly the most expensive model of the hive.
//!
//! The fix follows the shape GH #9 gave the embedding lane: the accounting rides
//! the fields the llm cells already emit (`hop.tokens_prompt` /
//! `hop.tokens_completion`), so the message log and the run's books count the
//! same thing. What is new here is only WHERE they land -- one parked receipt
//! per model call under the run key, read back by the phase that closes the run,
//! because a night closes several store round trips after the model answered and
//! the hop is long gone by then.
//!
//! Everything below runs the REAL `params.script_inline` of the `code` cell
//! against injected replies, so no model is called and nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const STORE_CONFIG: &str = "../../templates/memory-hive/store/config.json";

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

fn glue_script() -> String {
    resolve_vars(
        config_of(GLUE_CONFIG)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
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

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let script = glue_script();
    let out = run_script_on_stdin(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// A model reply as the return edge delivers it: the answer in the body, the
/// usage on the hop. `usage` is `None` for a provider that reported none.
fn reply(phase: &str, text: &str, usage: Option<(u64, u64)>) -> serde_json::Value {
    let mut hop = serde_json::json!({"finish_reason": "stop"});
    if let Some((prompt, completion)) = usage {
        hop["tokens_prompt"] = serde_json::json!(prompt);
        hop["tokens_completion"] = serde_json::json!(completion);
    }
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": hop
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

/// One store reply as the edge delivers it.
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

/// Every call receipt a phase parked, in order, already parsed.
fn receipts(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["table"] == "scratch" && a["row"]["kind"] == "llm-call")
        .map(|a| {
            serde_json::from_str(a["row"]["payload"].as_str().expect("payload")).expect("json")
        })
        .collect()
}

/// The `consolidation_log` update that closes a run.
fn close_op(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run has to close")
}

/// The scratch rows of a run, as the apply-run select hands them back. The two
/// mandatory kinds plus whatever call receipts the night parked.
fn run_scratch(calls: &[serde_json::Value]) -> serde_json::Value {
    let mut rows = vec![
        serde_json::json!({"key": RUN, "kind": "verdicts", "payload": "{}"}),
        serde_json::json!({"key": RUN, "kind": "beliefs", "payload": "[]"}),
    ];
    for call in calls {
        rows.push(serde_json::json!({"key": RUN, "kind": "llm-call",
                                     "payload": call.to_string()}));
    }
    serde_json::Value::Array(rows)
}

// ------------------------------------------------------------------ the column

#[test]
fn the_store_declares_the_two_token_columns_next_to_the_call_count() {
    // The books are one row per run, so the tokens belong in that row rather than
    // in the `verdicts` payload: everything folded in there is a JUDGEMENT of the
    // night (closures, reopenings, cardinality, rewordings), and what a call cost
    // is not a judgement. The store grows an existing table additively
    // (`ALTER TABLE ADD COLUMN` at boot), so a colony that has been running since
    // 0.2.0 gains the columns without a migration and reads NULL until its next
    // night.
    let table = &config_of(STORE_CONFIG)["params"]["schema"]["consolidation_log"];
    for column in ["llm_calls", "tokens_prompt", "tokens_completion"] {
        assert_eq!(
            table[column], "int",
            "the run books are missing {column}: {table}"
        );
    }
}

// --------------------------------------------------------- one receipt per call

#[test]
fn the_dreamer_call_is_parked_with_the_usage_of_its_reply() {
    // The dreamer's answer opens the apply chain and the run closes six store
    // round trips later, so the hop that carries the usage has to be written down
    // where the close can find it: under the run key, like every other artefact
    // of this lane.
    let msgs = emit(reply("verdict", "{\"beliefs\": []}", Some((1200, 340))));
    assert_eq!(
        receipts(&msgs),
        vec![serde_json::json!({"call": "dreamer",
                                "tokens_prompt": 1200, "tokens_completion": 340})],
        "the dreamer's call was not booked: {msgs:?}"
    );
}

#[test]
fn the_judge_call_is_parked_too_and_that_is_the_whole_issue() {
    // The call the books used to hide (GH #64). The round is a second model call,
    // made once a night on the most expensive model of the hive, and until this
    // receipt existed nothing downstream of the provider dashboard could see it.
    let msgs = emit(reply(
        "canon-judged",
        r#"{"predicates":[{"alias":"Lieblingseditor","canonical":"favorite_editor"}]}"#,
        Some((8400, 260)),
    ));
    assert_eq!(
        receipts(&msgs),
        vec![serde_json::json!({"call": "judge",
                                "tokens_prompt": 8400, "tokens_completion": 260})],
        "the judgement did its work without booking its call: {msgs:?}"
    );
}

#[test]
fn a_reply_without_usage_books_the_call_and_no_tokens() {
    // The rule GH #9 wrote for the embedding lane, one lane over: a response
    // without a usage block leaves the field out rather than booking a zero. A
    // zero would read as a call that was free, and no reader could tell the two
    // apart afterwards.
    let msgs = emit(reply("verdict", "{\"beliefs\": []}", None));
    assert_eq!(
        receipts(&msgs),
        vec![serde_json::json!({"call": "dreamer"})],
        "an unreported usage became a number: {msgs:?}"
    );
}

#[test]
fn a_judge_that_answered_badly_is_booked_all_the_same() {
    // Both skip paths of the round. A judge that errored or answered in prose was
    // asked, answered and billed -- and a night whose books drop exactly those
    // calls reports the cheapest possible version of itself.
    let mut dead = reply("canon-judged", "irrelevant", Some((8400, 12)));
    dead["header"]["hop"]["finish_reason"] = serde_json::json!("error");
    assert_eq!(
        receipts(&emit(dead)),
        vec![serde_json::json!({"call": "judge",
                                "tokens_prompt": 8400, "tokens_completion": 12})]
    );
    assert_eq!(
        receipts(&emit(reply(
            "canon-judged",
            "I could not decide, sorry.",
            Some((8400, 9))
        ))),
        vec![serde_json::json!({"call": "judge",
                                "tokens_prompt": 8400, "tokens_completion": 9})],
        "an unparseable answer is the same class: skip the round, book the call"
    );
}

// ------------------------------------------------------------------- the close

#[test]
fn a_night_that_asked_the_judge_books_two_calls_and_the_sum_of_both() {
    // The pin of GitHub #64. Two model calls, two receipts, and the run closes
    // saying two -- with the tokens of BOTH replies summed into the row, which is
    // the second half the issue asks for. Against the old code this reads 1 and
    // no tokens at all.
    let msgs = emit(store_reply(
        "apply-run",
        "select",
        run_scratch(&[
            serde_json::json!({"call": "dreamer", "tokens_prompt": 1200,
                               "tokens_completion": 340}),
            serde_json::json!({"call": "judge", "tokens_prompt": 8400,
                               "tokens_completion": 260}),
        ]),
    ));
    let closed = close_op(&msgs);
    assert_eq!(
        closed["set"]["llm_calls"], 2,
        "the judge is still not in the books: {closed}"
    );
    assert_eq!(closed["set"]["tokens_prompt"], 9600);
    assert_eq!(closed["set"]["tokens_completion"], 600);
    assert_eq!(
        closed["set"]["status"], "done",
        "and the close is otherwise the one this lane always wrote"
    );
}

#[test]
fn a_night_without_a_round_books_the_one_call_it_made() {
    // The counter-direction, and the reason the count is derived from the
    // receipts rather than hardcoded: a night where the round was switched off,
    // had no question or was never reached books exactly one call.
    let closed = close_op(&emit(store_reply(
        "apply-run",
        "select",
        run_scratch(
            &[serde_json::json!({"call": "dreamer", "tokens_prompt": 1200,
                                         "tokens_completion": 340})],
        ),
    )));
    assert_eq!(closed["set"]["llm_calls"], 1);
    assert_eq!(closed["set"]["tokens_prompt"], 1200);
    assert_eq!(closed["set"]["tokens_completion"], 340);
}

#[test]
fn tokens_nobody_reported_stay_out_of_the_row_instead_of_becoming_zero() {
    // NULL in a token column means "no reply reported usage". Writing 0 instead
    // would make a colony whose provider omits the usage block look free, and the
    // free scenario suite -- where every model reply is INJECTED and therefore
    // carries no usage at all -- would be the loudest liar of them all.
    let closed = close_op(&emit(store_reply(
        "apply-run",
        "select",
        run_scratch(&[
            serde_json::json!({"call": "dreamer"}),
            serde_json::json!({"call": "judge"}),
        ]),
    )));
    assert_eq!(
        closed["set"]["llm_calls"], 2,
        "the calls happened even where their usage did not arrive"
    );
    assert!(
        closed["set"]["tokens_prompt"].is_null() && closed["set"]["tokens_completion"].is_null(),
        "an unknown token count was booked as a number: {closed}"
    );
}

#[test]
fn a_run_older_than_the_books_still_reports_its_dreamer() {
    // A night that was already in flight when the lane learned to book calls has
    // no receipt at all. The dreamer's answer is what got the run to this phase,
    // so its call is certain -- the floor keeps such a run at the number the old
    // code wrote instead of claiming it was free.
    let closed = close_op(&emit(store_reply("apply-run", "select", run_scratch(&[]))));
    assert_eq!(closed["set"]["llm_calls"], 1);
    assert!(closed["set"]["tokens_prompt"].is_null());
}

#[test]
fn a_dreamer_that_failed_books_its_call_at_the_close_it_writes_itself() {
    // The two paths that close the run inside the verdict phase never reach the
    // parking mechanism, and they hold the hop in their hands -- so they book the
    // call directly. A dreamer that errored or answered in a shape the lane
    // cannot read was still called and still billed.
    for (text, error) in [
        ("irrelevant", "dreamer_failed"),
        ("not json at all", "verdict_not_json"),
    ] {
        let mut doc = reply("verdict", text, Some((1200, 7)));
        if error == "dreamer_failed" {
            doc["header"]["hop"]["finish_reason"] = serde_json::json!("error");
        }
        let closed = close_op(&emit(doc));
        assert_eq!(closed["set"]["llm_calls"], 1, "on the {error} path");
        assert_eq!(closed["set"]["tokens_prompt"], 1200, "on the {error} path");
        assert_eq!(closed["set"]["tokens_completion"], 7, "on the {error} path");
        assert!(
            closed["set"]["verdicts"]
                .as_str()
                .expect("verdicts")
                .contains(error),
            "the close lost its reason: {closed}"
        );
    }
}

#[test]
fn an_empty_window_books_nothing_because_nothing_was_called() {
    // The one close that must stay at zero: a window with no facts in it never
    // reaches the dreamer, so the night made no model call and the books say so.
    let closed = close_op(&emit(store_reply("scope", "select", serde_json::json!([]))));
    assert_eq!(closed["set"]["llm_calls"], 0);
    assert!(
        closed["set"]["tokens_prompt"].is_null(),
        "a run that called nothing booked tokens: {closed}"
    );
}
