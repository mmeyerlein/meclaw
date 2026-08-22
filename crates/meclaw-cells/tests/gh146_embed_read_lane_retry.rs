//! GH #146 -- the read lane of the memory-hive embedder retries before it
//! degrades, and it is bounded while doing so.
//!
//! # The defect
//!
//! Measured during a 50-question eval under ten parallel colonies: one question
//! fused three legs instead of four (`semantic: 0`, `degraded: true`) while its
//! store held 356 `ready` embedding rows. The corpus was fine -- what failed was
//! embedding the QUERY. Timing is the evidence: the degraded question spent
//! 21.56 s in tier-1 against a median of 3.15 s, i.e. it walked into the 20 s
//! bound, while a single call against the same provider took 0.26 s. That is CPU
//! contention on the box, not a dead endpoint -- and the read lane had no retry,
//! so one slow moment cost the most expensive leg of the fan.
//!
//! # What is fixed, and what must NOT change
//!
//! Two things, and they pull in opposite directions:
//!
//! - the read lane gets a bounded retry and its OWN, more generous timeout,
//!   separate from bulk corpus embedding (throughput vs. latency);
//! - the fail-open contract survives it. A retry is not a replacement for
//!   answering: silence from this cell hangs recall's fan-in forever, so after
//!   the last attempt the lane still answers `vector: null, degraded: true`.
//!
//! The third property is the one that makes the first two safe: the retry may
//! never push the lane past the cell's own operation timeout, because a killed
//! process IS silence. The lane therefore reads its own `external_timeout_ms`
//! and skips an attempt it cannot finish.
//!
//! Everything here runs the REAL `params.script_inline` against a local mock
//! (P5 pattern): no paid call, and the property under test -- retry, budget,
//! fail-open -- is the same whoever answers.

use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use serde_json::{Map, Value, json};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const EMBED_CONFIG: &str = "../../templates/memory-hive/embed/config.json";

fn embed_config() -> Value {
    let raw = std::fs::read_to_string(EMBED_CONFIG).expect("embed config");
    serde_json::from_str(&raw).expect("embed config json")
}

/// The shipped script with `MEMORY_EMBED_ENDPOINT` bound to `endpoint`; every
/// other `${VAR:-default}` collapses to its default and every bare `${VAR}` to
/// the empty string -- the substitution the colony performs at instantiation.
/// (Same helper as `embed_token_accounting.rs`: the point of both files is that
/// the SHIPPED script runs, never a copy of it.)
fn embed_script(endpoint: &str) -> String {
    let cfg = embed_config();
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let (name, default) = match tail[..end].split_once(":-") {
            Some((n, d)) => (n, d),
            None => (&tail[..end], ""),
        };
        if name == "MEMORY_EMBED_ENDPOINT" {
            out.push_str(endpoint);
        } else {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The SHIPPED params with `over` merged on top, minus the script source --
/// exactly the object `build_stdin_json` hands the child. A case that names no
/// knob therefore exercises the shipped default, and a typo in a knob name is
/// an assertion failure instead of a silently ignored line.
fn embed_params(over: Value) -> Value {
    let cfg = embed_config();
    let mut p: Map<String, Value> = cfg["params"].as_object().expect("params").clone();
    p.remove("script_inline");
    for (k, v) in over.as_object().expect("overrides are an object") {
        assert!(
            p.contains_key(k),
            "unknown knob `{k}` -- the shipped params do not carry it"
        );
        p.insert(k.clone(), v.clone());
    }
    Value::Object(p)
}

/// A stdin document carrying `args` as the tool-call text plus this cell's
/// configuration, split into the three top-level objects the substrate builds.
fn tool_call_doc(args: Value, params: Value) -> String {
    meclaw_testing::code_stdin(&json!({
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "e-in",
                      "text": args.to_string()}],
        "params": params,
    }))
    .to_string()
}

fn query_doc(text: &str, params: Value) -> String {
    tool_call_doc(json!({"query": {"text": text, "recall_id": "r1"}}), params)
}

/// An OpenAI-compatible embeddings response, optionally answered late enough to
/// walk into the caller's timeout.
fn embeddings_response(delay: Option<Duration>) -> MockResponse {
    let body = json!({"object": "list", "model": "mock-embed",
                      "data": [{"object": "embedding", "index": 0,
                                "embedding": [0.5, -0.5, 0.5, -0.5]}],
                      "usage": {"prompt_tokens": 7, "total_tokens": 7}});
    let resp = MockResponse::ok_json(body.to_string().as_bytes());
    match delay {
        Some(d) => resp.with_delay(d),
        None => resp,
    }
}

struct Run {
    msgs: Vec<Value>,
    stderr: String,
}

impl Run {
    /// The read lane's answer body (it always emits exactly one message).
    fn query_body(&self) -> Value {
        assert_eq!(self.msgs.len(), 1, "the read lane answers exactly once");
        assert_eq!(self.msgs[0]["header"]["route"], "equery");
        serde_json::from_str(self.msgs[0]["messages"][0]["text"].as_str().unwrap())
            .expect("query body json")
    }
}

/// Runs the shipped script against `stdin_doc`. A non-zero exit would itself be
/// a contract break (the cell must never die on a bad embedder), so it is
/// asserted here rather than in every case.
async fn run_embed(script: &str, stdin_doc: &str) -> Run {
    // The script travels on stdin, never in argv: a single argv string is capped
    // at 128 KiB (`MAX_ARG_STRLEN`) and the shipped scripts have grown to within
    // a few KB of it (GH #279, precedent 89a522e4). The document rides inside the
    // program and is put under `sys.stdin` before the script runs, so the script
    // itself sees exactly what `python3 -c` gave it.
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
        .expect("spawn python3");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(src.as_bytes()).await.expect("write");
    drop(stdin);
    let out = child.wait_with_output().await.expect("python3 output");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "the cell must not die: {stderr}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a multi-send ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    Run { msgs, stderr }
}

/// The number the script itself carries as a knob's default, e.g. the `30000`
/// in `_int("query_timeout_ms", 30000)`.
fn script_default(knob: &str) -> i64 {
    let cfg = embed_config();
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let needle = format!("_int(\"{knob}\", ");
    let at = script
        .find(&needle)
        .unwrap_or_else(|| panic!("the script reads `{knob}` off its params"))
        + needle.len();
    let rest = &script[at..];
    let end = rest
        .find(')')
        .unwrap_or_else(|| panic!("unterminated _int call for `{knob}`"));
    rest[..end]
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("`{knob}` default is not a number: {e}"))
}

// ---------------------------------------------------------------- the retry

/// The issue's exact shape: the first attempt walks into the timeout, the second
/// one answers -- and the semantic leg stands instead of being lost for the rest
/// of the question. A TIMEOUT is used deliberately rather than an HTTP error,
/// because the timeout is what was measured in production.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timed_out_first_attempt_is_retried_and_the_semantic_leg_stands() {
    let (addr, _join, cap) = start_mock_server_capturing(vec![
        embeddings_response(Some(Duration::from_secs(5))),
        embeddings_response(None),
    ])
    .await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let params = embed_params(json!({"query_timeout_ms": 400, "query_retries": 1,
                                     "query_retry_backoff_ms": 50}));

    let run = run_embed(&script, &query_doc("what does the user eat", params)).await;
    let body = run.query_body();

    assert_eq!(
        body["degraded"], false,
        "the second attempt succeeded, so nothing is degraded: {body}"
    );
    assert!(
        body["vector"].is_string(),
        "the query vector is there -- that IS the semantic leg: {body}"
    );
    assert_eq!(
        cap.lock().await.len(),
        2,
        "exactly two attempts: the retry fired once, and only once"
    );
    assert!(
        run.stderr.contains("query attempt 1/2 failed"),
        "a recovered attempt is still stated, or the retry's value is unmeasurable: {:?}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("query degraded"),
        "nothing degraded here: {:?}",
        run.stderr
    );
}

/// Fail-open survives the retry. After the last attempt the lane still ANSWERS
/// -- degraded, with a reason, at exit code 0. Silence would hang recall's
/// fan-in forever, which is the failure the whole cell is shaped around.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exhausted_retry_still_fails_open_with_a_reason() {
    let (addr, _join, cap) = start_mock_server_capturing(vec![
        MockResponse::server_error(),
        MockResponse::server_error(),
    ])
    .await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let params = embed_params(json!({"query_retries": 1, "query_retry_backoff_ms": 20}));

    let run = run_embed(&script, &query_doc("anything", params)).await;
    let body = run.query_body();

    assert_eq!(
        body["degraded"], true,
        "the endpoint never answered: {body}"
    );
    assert!(body["vector"].is_null(), "no vector to hand over: {body}");
    let err = body["error"].as_str().expect("a stated reason");
    assert!(
        err.contains("HTTP 500") && err.contains("2 attempt"),
        "the reason names the cause AND how often it was tried: {err}"
    );
    assert_eq!(
        cap.lock().await.len(),
        2,
        "one retry means two attempts, not an unbounded loop"
    );
    assert!(
        run.stderr.contains("embed: query degraded"),
        "giving up is a WARN line, not only a flag in the body: {:?}",
        run.stderr
    );
}

/// The retry is opt-out, and switching it off changes nothing else: one attempt,
/// still an answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retries_zero_is_one_attempt_and_still_an_answer() {
    let (addr, _join, cap) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let params = embed_params(json!({"query_retries": 0}));

    let run = run_embed(&script, &query_doc("anything", params)).await;

    assert_eq!(run.query_body()["degraded"], true);
    assert_eq!(cap.lock().await.len(), 1, "no retry was asked for");
}

// -------------------------------------------------------------- the bound

/// The retry never outlives the cell's own operation timeout. Configured
/// absurdly -- a 30 s per-attempt timeout inside a 4 s operation budget -- the
/// lane spends the budget it has, skips the attempt it cannot finish, and still
/// answers. Being killed here would be silence, which is strictly worse than the
/// degraded answer the retry exists to avoid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_read_lane_answers_inside_its_own_operation_timeout() {
    let (addr, _join, cap) =
        start_mock_server_capturing(vec![embeddings_response(Some(Duration::from_secs(30)))]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let params = embed_params(
        json!({"external_timeout_ms": 4000, "query_timeout_ms": 30000,
                                     "query_retries": 3, "query_retry_backoff_ms": 20}),
    );

    let started = Instant::now();
    let run = run_embed(&script, &query_doc("anything", params)).await;
    let elapsed = started.elapsed();

    assert_eq!(run.query_body()["degraded"], true);
    assert!(
        elapsed < Duration::from_millis(4000),
        "the lane answered after {elapsed:?}, i.e. its own operation timeout would have killed it"
    );
    assert_eq!(
        cap.lock().await.len(),
        1,
        "three retries were configured and the budget allowed one attempt -- \
         an attempt that cannot finish is not started"
    );
}

/// The shipped numbers are consistent with each other: the whole worst case of
/// the read lane fits inside the operation timeout, which in turn stays well
/// under the substrate backstop. Raising one knob without the others is the
/// mistake this pin exists to catch.
#[test]
fn the_shipped_budget_fits_inside_the_shipped_timeouts() {
    let cfg = embed_config();
    let p = &cfg["params"];
    let per_attempt = p["query_timeout_ms"].as_i64().expect("query_timeout_ms");
    let retries = p["query_retries"].as_i64().expect("query_retries");
    let backoff = p["query_retry_backoff_ms"]
        .as_i64()
        .expect("query_retry_backoff_ms");
    let external = p["external_timeout_ms"]
        .as_i64()
        .expect("external_timeout_ms");
    // The script keeps 2 s of the operation budget for spawn plus the final
    // write; anything the attempts want on top has to fit below that.
    let worst_case = (retries + 1) * per_attempt + retries * backoff + 2000;
    assert!(
        worst_case <= external,
        "worst case {worst_case} ms does not fit in external_timeout_ms {external} ms -- \
         the lane would lose attempts it was configured to make"
    );
    let backstop = cfg["cell"]["message_timeout"]
        .as_i64()
        .expect("the cell declares its own backstop instead of taking the colony default");
    assert!(
        backstop > external,
        "the B-backstop ({backstop} ms) must stay above the A-timeout ({external} ms), \
         or the substrate kills the cell before its own timeout can answer"
    );
}

/// The read path is the generous one and the write path is not: two knobs with
/// two defaults is the entire point of the separation.
#[test]
fn the_read_lane_is_configured_more_generously_than_the_write_lane() {
    let p = embed_config()["params"].clone();
    let write = p["timeout_ms"].as_i64().expect("timeout_ms");
    let read = p["query_timeout_ms"].as_i64().expect("query_timeout_ms");
    assert!(
        read > write,
        "read lane {read} ms is not more generous than the write lane {write} ms"
    );
    assert_eq!(
        write, 20000,
        "the bulk corpus lane keeps the bound it shipped with"
    );
}

/// A knob's default is the SAME number in all three places it lives: the
/// template params, `contract.settings`, and the script's own literal. Two of
/// them agreeing is not enough -- the script is what runs.
#[test]
fn every_knob_has_one_default_in_all_three_places() {
    let cfg = embed_config();
    for knob in [
        "timeout_ms",
        "query_timeout_ms",
        "query_retries",
        "query_retry_backoff_ms",
    ] {
        let from_params = cfg["params"][knob]
            .as_i64()
            .unwrap_or_else(|| panic!("params carry `{knob}`"));
        let from_contract = cfg["contract"]["settings"][knob]["default"]
            .as_i64()
            .unwrap_or_else(|| panic!("contract.settings declares `{knob}`"));
        assert_eq!(
            from_params, from_contract,
            "`{knob}`: params say {from_params}, the contract says {from_contract}"
        );
        assert_eq!(
            from_params,
            script_default(knob),
            "`{knob}`: params say {from_params}, the script's own literal disagrees"
        );
    }
}

/// The environment route is absent, not deprecated: a `.env` line for the old
/// knob is read by nothing, so leaving the spelling in the script would be a
/// promise the cell no longer keeps.
#[test]
fn the_timeout_knob_left_the_environment_surface() {
    let raw = std::fs::read_to_string(EMBED_CONFIG).expect("embed config");
    assert!(
        !raw.contains("MEMORY_EMBED_TIMEOUT_MS"),
        "the timeout knob still names its old environment variable"
    );
}

// -------------------------------------------------------------- write lane

/// The write lane keeps the opposite rule: no in-process retry, silence, and the
/// rows stay queued for the nightly backfill -- which IS that lane's retry.
/// Repeating a batch here would pay twice for rows a later run picks up anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_write_lane_does_not_retry_in_process() {
    let (addr, _join, cap) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let params = embed_params(json!({}));

    let run = run_embed(
        &script,
        &tool_call_doc(
            json!({"items": [{"embedding_id": "e1", "text": "a row"}]}),
            params,
        ),
    )
    .await;

    assert!(
        run.msgs.is_empty(),
        "a failed batch is silence, not a store write: {:?}",
        run.msgs
    );
    assert_eq!(
        cap.lock().await.len(),
        1,
        "the write lane sent its batch once"
    );
    assert!(
        run.stderr.contains("rows stay queued"),
        "the no-op is logged with its reason: {:?}",
        run.stderr
    );
}
