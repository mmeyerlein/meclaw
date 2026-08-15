//! Issue #9 -- the embed lane's token accounting.
//!
//! The `llm` cells put `usage.prompt_tokens` / `usage.completion_tokens` on the
//! hop compartment (`crates/meclaw-cells/src/llm/output.rs`), and the cost
//! rollup sums `hop.tokens_*` over the whole message log. The embed lane is a
//! `code` cell that calls the embeddings endpoint itself, so it has to put its
//! own usage on the SAME hop field -- otherwise the books are short by every
//! embedding call, and a backfill that re-embeds the same rows every night
//! stays invisible.
//!
//! The REAL `params.script_inline` runs here, never a copy (P5 pattern, see
//! `workshop/evals/p5-longmemeval/tools/cellrun.py`): the `${VAR:-default}`
//! literals are resolved the way the colony resolves them at instantiation,
//! with only the endpoint pointed at a local mock. The script therefore speaks
//! real HTTP and its stdout is the multi-send the colony would wire.

use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use serde_json::{Value, json};
use std::process::Stdio;
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

/// A UBF doc carrying `args` as the tool-call text the embed cell consumes.
fn tool_call_doc(args: Value) -> String {
    meclaw_testing::code_stdin(
        &json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "e-in",
                              "text": args.to_string()}]}),
    )
    .to_string()
}

/// An OpenAI-compatible embeddings response. `usage` is `None` for providers
/// that omit the block entirely.
fn embeddings_response(vectors: &[Vec<f64>], usage: Option<Value>) -> MockResponse {
    let data: Vec<Value> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| json!({"object": "embedding", "index": i, "embedding": v}))
        .collect();
    let mut body = json!({"object": "list", "data": data, "model": "mock-embed"});
    if let Some(u) = usage {
        body["usage"] = u;
    }
    MockResponse::ok_json(body.to_string().as_bytes())
}

/// Run the shipped script against `stdin_doc`; returns the parsed multi-send.
async fn run_embed(script: &str, stdin_doc: &str) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(stdin_doc.as_bytes()).await.expect("write");
    drop(stdin);
    let out = child.wait_with_output().await.expect("python3 output");
    assert!(
        out.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a multi-send ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Sum of `header.tokens_prompt` over a multi-send, plus how many messages
/// carry the field at all (a batch must bill exactly once).
fn prompt_tokens(msgs: &[Value]) -> (u64, usize) {
    let mut sum = 0;
    let mut carriers = 0;
    for m in msgs {
        if let Some(t) = m["header"].get("tokens_prompt") {
            sum += t
                .as_u64()
                .unwrap_or_else(|| panic!("tokens_prompt not a number: {t}"));
            carriers += 1;
        }
    }
    (sum, carriers)
}

/// Recall's query embedding: one text in, one vector back -- and the prompt
/// tokens the provider billed for it on the hop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_embedding_surfaces_its_prompt_tokens_on_the_hop() {
    let resp = embeddings_response(
        &[vec![0.5, -0.5, 0.5, -0.5]],
        Some(json!({"prompt_tokens": 7, "total_tokens": 7})),
    );
    let (addr, _join, _cap) = start_mock_server_capturing(vec![resp]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));

    let msgs = run_embed(
        &script,
        &tool_call_doc(json!({"query": {"text": "what does the user eat", "recall_id": "r1"}})),
    )
    .await;

    assert_eq!(msgs.len(), 1, "read lane always answers exactly once");
    assert_eq!(msgs[0]["header"]["route"], "equery");
    let body: Value = serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().unwrap())
        .expect("query body json");
    assert_eq!(body["degraded"], false, "mock answered, so not degraded");
    assert_eq!(prompt_tokens(&msgs), (7, 1));
}

/// The backfill form: one batched call for N rows. The batch is billed ONCE,
/// so exactly one of the N updates may carry the usage -- spreading it over
/// all of them would multiply the cost by the batch size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backfill_batch_bills_its_prompt_tokens_exactly_once() {
    let resp = embeddings_response(
        &[
            vec![1.0, -1.0, 1.0, -1.0],
            vec![-1.0, 1.0, -1.0, 1.0],
            vec![1.0, 1.0, -1.0, -1.0],
        ],
        Some(json!({"prompt_tokens": 33, "total_tokens": 33})),
    );
    let (addr, _join, _cap) = start_mock_server_capturing(vec![resp]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));

    let msgs = run_embed(
        &script,
        &tool_call_doc(json!({"items": [
            {"embedding_id": "e1", "text": "first row"},
            {"embedding_id": "e2", "text": "second row"},
            {"embedding_id": "e3", "text": "third row"},
        ]})),
    )
    .await;

    assert_eq!(msgs.len(), 3, "one store update per embedded row");
    for m in &msgs {
        assert_eq!(m["header"]["route"], "estore");
    }
    assert_eq!(
        prompt_tokens(&msgs),
        (33, 1),
        "the batch bills once, not once per row"
    );
}

/// A provider that omits `usage` must not change anything else: the vectors
/// still land, the hop field is simply absent (same shape as the llm lane,
/// which only inserts `tokens_*` when the response carried them).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_usage_leaves_the_hop_field_absent_on_both_lanes() {
    let (addr, _join, _cap) = start_mock_server_capturing(vec![
        embeddings_response(&[vec![0.5, -0.5, 0.5, -0.5]], None),
        embeddings_response(&[vec![1.0, -1.0, 1.0, -1.0]], None),
    ])
    .await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));

    let q = run_embed(
        &script,
        &tool_call_doc(json!({"query": {"text": "anything", "recall_id": "r1"}})),
    )
    .await;
    assert_eq!(q.len(), 1);
    let qbody: Value =
        serde_json::from_str(q[0]["messages"][0]["text"].as_str().unwrap()).expect("query body");
    assert_eq!(qbody["degraded"], false, "no usage is not a failure");
    assert!(qbody["vector"].is_string(), "the vector still comes back");
    assert_eq!(prompt_tokens(&q), (0, 0));

    let w = run_embed(
        &script,
        &tool_call_doc(json!({"items": [{"embedding_id": "e1", "text": "row"}]})),
    )
    .await;
    assert_eq!(w.len(), 1, "the update is still emitted");
    assert_eq!(w[0]["header"]["route"], "estore");
    assert_eq!(prompt_tokens(&w), (0, 0));
}

/// A dead endpoint stays a degraded answer and books nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_endpoint_books_nothing_and_still_answers_the_read_lane() {
    let (addr, _join, _cap) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));

    let msgs = run_embed(
        &script,
        &tool_call_doc(json!({"query": {"text": "anything", "recall_id": "r1"}})),
    )
    .await;

    assert_eq!(msgs.len(), 1);
    let body: Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().unwrap()).expect("query body");
    assert_eq!(body["degraded"], true);
    assert_eq!(prompt_tokens(&msgs), (0, 0));
}

/// The accounting field is declared, not just emitted: the contract names it
/// with the same type the llm-cell templates use for `hop.tokens_prompt`.
#[test]
fn contract_declares_the_accounting_field() {
    let spec = embed_config()["contract"]["emits"]["hop"]["tokens_prompt"].clone();
    assert_eq!(spec["type"], "number", "declared as number: {spec}");
    assert_eq!(spec["required"], false, "optional -- usage may be absent");
}
