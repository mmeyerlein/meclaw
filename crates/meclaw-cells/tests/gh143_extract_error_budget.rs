//! The extraction lane stops re-sending a batch the provider keeps refusing
//! (GitHub #143).
//!
//! Measured against a dead endpoint while preparing an eval run: the drain
//! flushes about every five seconds, and a batch whose extractor call came back
//! as a provider error was handed straight back to the queue -- where the next
//! flush claimed and re-sent it. 107 of 111 extraction responses inside nine
//! minutes were retries of ONE failing batch, on a seven-turn session.
//!
//! Against a local model that is free noise. Against a paid provider it is a
//! colony that looks idle while it spends, and nothing in the hive counted
//! consecutive failures.
//!
//! So the lane now carries two numbers on the queue row itself: `attempts`, and
//! the instant `not_before` from which the batch may be claimed again. The
//! window doubles per attempt, and after `MEMORY_EXTRACT_ERROR_BUDGET`
//! consecutive errors the batch is parked instead of re-sent -- loudly, with a
//! mark in `scratch`, because a queue row that silently stops moving is the
//! failure mode this path exists to remove.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against a small in-test queue, so nothing costs anything and no model is
//! called.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template.
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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// The declared default of one of the lane's settings, read from the contract
/// rather than repeated here -- a test that hard-codes 3 keeps passing after
/// somebody changes the budget to 5.
fn setting_default(name: &str) -> i64 {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    v["contract"]["settings"][name]["default"]
        .as_i64()
        .unwrap_or_else(|| panic!("contract.settings.{name}.default"))
}

struct Emission {
    msgs: Vec<serde_json::Value>,
    stderr: String,
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

/// Run the real script with a real stdin document and return what it emitted,
/// stderr included: the parking decision is supposed to be audible.
fn emit(script: &str, doc: serde_json::Value) -> Emission {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {stderr}"
    );
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    Emission { msgs, stderr }
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// Every store op of an emission, with the phase the edge would promote.
fn ops(msgs: &[serde_json::Value]) -> Vec<(String, serde_json::Value)> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(|m| {
            (
                m["header"]["phase"].as_str().unwrap_or("").to_string(),
                args_of(m),
            )
        })
        .collect()
}

fn only_op(msgs: &[serde_json::Value]) -> Option<(String, serde_json::Value)> {
    ops(msgs).into_iter().next()
}

/// A document as the colony delivers it to the lane: the phase in the context,
/// the store's answer in the hop.
fn doc(
    phase: &str,
    operation: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": phase},
            "hop": {"operation": operation, "rows_affected": rows_affected}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload.to_string()}]
    })
}

type Row = BTreeMap<String, serde_json::Value>;

/// Just enough of a store to drive the lane's state machine: the three ops the
/// extraction queue uses over one table, plus the scratch table the parking
/// mark lands in.
#[derive(Default)]
struct Queue {
    rows: Vec<Row>,
    scratch: Vec<Row>,
}

impl Queue {
    fn matches(row: &Row, filters: &serde_json::Value) -> bool {
        let Some(map) = filters.as_object() else {
            return true;
        };
        map.iter().all(|(col, want)| {
            let have = row.get(col).cloned().unwrap_or(serde_json::Value::Null);
            match want {
                serde_json::Value::Object(o) if o.contains_key("in") => {
                    o["in"].as_array().expect("in list").contains(&have)
                }
                serde_json::Value::Object(o) if o.contains_key("lt") => {
                    let bound = o["lt"].as_str().expect("lt is a string bound");
                    have.as_str().is_some_and(|h| h < bound)
                }
                other => *other == have,
            }
        })
    }

    fn apply(&mut self, op: &serde_json::Value) -> (i64, Vec<Row>) {
        let table = op["table"].as_str().unwrap_or("pending_extraction");
        match op["operation"].as_str().expect("operation") {
            "insert" => {
                let row: Row = serde_json::from_value(op["row"].clone()).expect("row object");
                if table == "scratch" {
                    self.scratch.push(row);
                } else {
                    self.rows.push(row);
                }
                (1, vec![])
            }
            "select" => {
                let hits: Vec<Row> = self
                    .rows
                    .iter()
                    .filter(|r| Self::matches(r, &op["where"]))
                    .cloned()
                    .collect();
                (hits.len() as i64, hits)
            }
            "update" => {
                let set = op["set"].as_object().expect("set object");
                let mut n = 0;
                for row in self.rows.iter_mut() {
                    if Self::matches(row, &op["where"]) {
                        for (k, v) in set {
                            row.insert(k.clone(), v.clone());
                        }
                        n += 1;
                    }
                }
                (n, vec![])
            }
            other => panic!("the extraction queue never emits {other}"),
        }
    }

    fn with_status(&self, status: &str) -> Vec<&Row> {
        self.rows.iter().filter(|r| r["status"] == status).collect()
    }
}

fn step(
    script: &str,
    q: &mut Queue,
    phase: &str,
    operation: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> Option<(String, serde_json::Value, i64, Vec<Row>)> {
    let e = emit(script, doc(phase, operation, rows_affected, payload));
    let (next_phase, op) = only_op(&e.msgs)?;
    let (n, rows) = q.apply(&op);
    Some((next_phase, op, n, rows))
}

/// Enqueue one turn and run the gate. Returns the batch id the gate claimed, or
/// `None` when the gate parked -- which is exactly the question this file asks.
fn ingest_turn(script: &str, q: &mut Queue, episode: &str, tokens: i64) -> Option<String> {
    let item = serde_json::json!({
        "episode_id": episode, "session_id": "s1", "sender": "user",
        "content": "a turn", "token_est": tokens
    });
    let (phase, _op, _n, _rows) = step(script, q, "enqueue", "", 0, item)?;
    assert_eq!(phase, "gate");

    let (phase, _op, n, rows) = step(script, q, "gate", "insert", 1, serde_json::json!({}))?;
    assert_eq!(phase, "gate-eval");

    let (_phase, op, _n, _rows) =
        step(script, q, "gate-eval", "select", n, serde_json::json!(rows))?;
    Some(
        op["set"]["batch_id"]
            .as_str()
            .expect("batch id")
            .to_string(),
    )
}

/// A flush that does not reclaim: "drain the queue now". Returns the batch id it
/// claimed, or `None` when it found nothing it was allowed to take.
fn flush(script: &str, q: &mut Queue) -> Option<String> {
    let e = emit(
        script,
        serde_json::json!({
            "header": {"context": {"mem_phase": "flush"}, "hop": {}},
            "messages": [{"origin": "user", "type": "text", "text": "{}"}]
        }),
    );
    let (phase, op) = only_op(&e.msgs)?;
    assert_eq!(phase, "flush-eval");
    let (n, rows) = q.apply(&op);
    let (_phase, op, _n, _rows) = step(
        script,
        q,
        "flush-eval",
        "select",
        n,
        serde_json::json!(rows),
    )?;
    Some(
        op["set"]["batch_id"]
            .as_str()
            .expect("batch id")
            .to_string(),
    )
}

/// One failed extractor call for `batch`: the provider error arrives in the
/// parse phase, the lane reads what the batch has already cost, and decides.
/// Returns the stderr of the deciding step.
fn provider_error(script: &str, q: &mut Queue, batch: &str) -> String {
    let e = emit(
        script,
        serde_json::json!({
            "header": {
                "context": {"mem_phase": "parse", "batch_id": batch},
                "hop": {"finish_reason": "error"}
            },
            "messages": [{"origin": "assistant", "type": "text", "text": ""}]
        }),
    );
    let (phase, op) = only_op(&e.msgs).expect("a provider error is answered by an op");
    assert_eq!(
        phase, "error-eval",
        "the error path reads the batch's cost before it decides"
    );
    assert_eq!(
        op["operation"], "select",
        "and it reads, it does not requeue"
    );
    let (n, rows) = q.apply(&op);

    let e = emit(
        script,
        serde_json::json!({
            "header": {
                "context": {"mem_phase": "error-eval", "batch_id": batch},
                "hop": {"operation": "select", "rows_affected": n}
            },
            "messages": [{"origin": "user", "type": "text",
                          "text": serde_json::json!(rows).to_string()}]
        }),
    );
    for (_phase, op) in ops(&e.msgs) {
        q.apply(&op);
    }
    e.stderr
}

// -------------------------------------------------------------------- tests

#[test]
fn a_provider_error_no_longer_hands_the_batch_straight_back() {
    // The measured failure: the rows went back to `pending` with nothing to say
    // they had just failed, so the next flush -- five seconds later -- claimed
    // and re-sent them. Now they come back carrying an attempt count and a
    // window, and the window is in the future.
    let script = glue_script();
    let mut q = Queue::default();
    let batch = ingest_turn(&script, &mut q, "e1", 999).expect("one turn over the gate is a claim");

    provider_error(&script, &mut q, &batch);

    let pending = q.with_status("pending");
    assert_eq!(pending.len(), 1, "the turn is still on the queue");
    assert_eq!(
        pending[0]["attempts"], 1,
        "the failure is counted on the row, so it survives the requeue"
    );
    let nb = pending[0]["not_before"]
        .as_str()
        .expect("the requeued row carries its backoff window");
    let now = std::process::Command::new("python3")
        .args(["-c", "import datetime;print(datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S.%fZ'))"])
        .output()
        .expect("python3");
    let now = String::from_utf8_lossy(&now.stdout).trim().to_string();
    assert!(
        nb > now.as_str(),
        "the retry window has to be in the future, got {nb} against {now}"
    );
}

#[test]
fn the_next_flush_does_not_claim_a_batch_inside_its_backoff_window() {
    // The assertion #143 is actually about. The flush cadence used to BE the
    // retry cadence; a held-back row must be invisible to the claim and to the
    // gate alike.
    let script = glue_script();
    let mut q = Queue::default();
    let batch = ingest_turn(&script, &mut q, "e1", 999).expect("claim");
    provider_error(&script, &mut q, &batch);

    assert!(
        flush(&script, &mut q).is_none(),
        "a flush inside the backoff window must find nothing it may take"
    );
    assert!(
        ingest_turn(&script, &mut q, "e2", 1).is_none(),
        "and so must the gate: counting tokens it may not claim is how a lane \
         spins on park() forever"
    );
    assert_eq!(
        q.with_status("claimed").len(),
        0,
        "nothing was claimed a second time"
    );
}

#[test]
fn the_budget_parks_a_batch_that_keeps_failing() {
    // Three consecutive provider errors (the declared default) and the lane
    // stops paying. The turns are not deleted -- they carry a terminal status
    // and a mark, so an operator can find them.
    let script = glue_script();
    let budget = setting_default("extract_error_budget");
    let mut q = Queue::default();
    let mut batch = ingest_turn(&script, &mut q, "e1", 999).expect("claim");

    let mut last_stderr = String::new();
    for attempt in 1..=budget {
        last_stderr = provider_error(&script, &mut q, &batch);
        if attempt < budget {
            // Clear the window by hand: this test is about the budget, not
            // about waiting out a real minute.
            for row in q.rows.iter_mut() {
                row.insert("not_before".into(), serde_json::json!(""));
            }
            batch =
                flush(&script, &mut q).expect("outside its window the batch is claimable again");
        }
    }

    assert_eq!(
        q.with_status("error_parked").len(),
        1,
        "after {budget} consecutive provider errors the batch is parked, not re-sent"
    );
    assert!(
        flush(&script, &mut q).is_none(),
        "and a parked batch is not picked up by the next flush"
    );
    assert!(
        last_stderr.contains("parked"),
        "the parking decision has to be audible, got stderr {last_stderr:?}"
    );
    assert_eq!(
        q.scratch
            .iter()
            .filter(|r| r["kind"] == "extract_error_parked")
            .count(),
        1,
        "and it leaves a mark an operator can find"
    );
}
