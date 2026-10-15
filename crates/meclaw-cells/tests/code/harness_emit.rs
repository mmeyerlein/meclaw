//! P8 block 5 — the five emission shapes.
//!
//! Every one of them is checked against the UBF schema, because the turn object
//! forbids additional properties: anything structural (task id, session id,
//! cost) belongs in the header, and a body that puts it in the turn would be
//! rejected at the delivery boundary rather than here.
//!
//! The result emission carries a second rule that is easy to lose: it reports
//! only what was OBSERVED — the workspace we handed out, the status we
//! decided, the numbers the harness reported about itself. It never claims a
//! branch or a commit; verifying those is a follow-up topology step.

use meclaw_cells::harness::emit;
use meclaw_cells::harness::emit::TaskOutcome;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OriginSink, OutputSink, Path};
use tokio::sync::mpsc;

fn output_sink() -> (
    OutputSink,
    mpsc::Receiver<CellEmission>,
    meclaw_core::Message,
) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let msg = MessageBuilder::new(Path::new("/harness"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages": []})))
        .build();
    let sink = OutputSink::new(
        tx,
        Path::new("/harness"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    );
    (sink, rx, msg)
}

fn origin_sink() -> (OriginSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    (OriginSink::new(tx, Path::new("/harness"), 16), rx)
}

fn body_of(em: &CellEmission) -> meclaw_core::serde_json::Value {
    let body = em.content.clone();
    meclaw_core::validate_ubf_body(&body).expect("emission must be UBF-valid");
    body
}

/// The one synchronous emission: it answers the request that started the task
/// and hands back the task id, which is the correlation key for everything
/// that follows on the origin lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_answers_in_the_trace_and_carries_the_task_id() {
    let (sink, mut rx, msg) = output_sink();
    emit::accepted(&sink, &msg, "t-1", "call-9").await;

    let em = rx.recv().await.expect("emission");
    assert_eq!(em.target, Path::new("/sink"));
    let body = body_of(&em);
    assert_eq!(body["header"]["harness_event"], "accepted");
    assert_eq!(body["header"]["task_id"], "t-1");
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][0]["id"], "call-9",
        "the tool_result must echo the call id it answers"
    );
    assert_eq!(body["messages"][0]["text"], "{\"task_id\":\"t-1\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_reports_what_the_harness_is_doing() {
    let (sink, mut rx) = origin_sink();
    emit::progress(
        &sink,
        Path::new("/main/coord"),
        "t-1",
        Some("s-1"),
        "tool",
        Some("Bash"),
        Some("ls -la"),
    )
    .await;

    let em = rx.recv().await.expect("emission");
    assert_eq!(em.target, Path::new("/main/coord"));
    assert_eq!(
        em.parent_message_id, None,
        "origin emissions start their own trace (overview § source cells)"
    );
    let body = body_of(&em);
    assert_eq!(body["header"]["harness_event"], "progress");
    assert_eq!(body["header"]["task_id"], "t-1");
    assert_eq!(body["header"]["session_id"], "s-1");
    assert_eq!(body["header"]["phase"], "tool");
    assert_eq!(body["header"]["tool_name"], "Bash");
    assert_eq!(body["messages"][0]["origin"], "assistant");
    assert_eq!(body["messages"][0]["type"], "text");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_carries_the_request_id_the_answer_must_quote() {
    let (sink, mut rx) = origin_sink();
    emit::question(
        &sink,
        Path::new("/main/coord"),
        "t-1",
        Some("s-1"),
        "r-7",
        "Bash",
        &json!({"command": "rm -rf /"}),
    )
    .await;

    let body = body_of(&rx.recv().await.expect("emission"));
    assert_eq!(body["header"]["harness_event"], "question");
    assert_eq!(body["header"]["request_id"], "r-7");
    assert_eq!(body["header"]["tool_name"], "Bash");
    assert!(
        body["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("rm -rf /"),
        "the question must show what it is about to run"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_reports_observations_not_claims() {
    let (sink, mut rx) = origin_sink();
    emit::result(
        &sink,
        Path::new("/main/coord"),
        &TaskOutcome {
            task_id: "t-1".to_string(),
            session_id: Some("s-1".to_string()),
            status: "ok",
            workspace: "/ws/one".to_string(),
            duration_ms: 4200,
            num_turns: Some(3),
            cost_usd: Some(0.25),
            model: Some("m-1".to_string()),
            error_code: None,
            text: "I created hello.txt on branch feature/x".to_string(),
        },
    )
    .await;

    let body = body_of(&rx.recv().await.expect("emission"));
    let header = &body["header"];
    assert_eq!(header["harness_event"], "result");
    assert_eq!(header["status"], "ok");
    assert_eq!(header["workspace"], "/ws/one");
    assert_eq!(header["num_turns"], 3);
    assert_eq!(header["cost_usd"], 0.25);
    assert_eq!(header["model"], "m-1");
    assert_eq!(header["duration_ms"], 4200);
    assert!(header.get("error_code").is_none(), "a success carries none");

    // The self-report travels as prose in the turn — and nowhere else. A
    // `branch` or `commit` header would turn an unverified claim into
    // structured data the topology would trust.
    assert!(
        body["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("feature/x")
    );
    for claimed in ["branch", "commit", "files_changed"] {
        assert!(
            header.get(claimed).is_none(),
            "the header must not carry {claimed}: it would be the harness's claim, not our observation"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_result_names_its_error_code() {
    let (sink, mut rx) = origin_sink();
    emit::result(
        &sink,
        Path::new("/main/coord"),
        &TaskOutcome {
            task_id: "t-1".to_string(),
            session_id: None,
            status: "unknown",
            workspace: "/ws/one".to_string(),
            duration_ms: 0,
            num_turns: None,
            cost_usd: None,
            model: None,
            error_code: Some("unknown_outcome"),
            text: "interrupted by a cell restart; inspect the workspace".to_string(),
        },
    )
    .await;

    let body = body_of(&rx.recv().await.expect("emission"));
    assert_eq!(body["header"]["status"], "unknown");
    assert_eq!(body["header"]["error_code"], "unknown_outcome");
    assert!(
        body["header"].get("num_turns").is_none(),
        "absent numbers must be absent, not zero — zero would be a claim"
    );
}

/// The error emission exists for the message-driven failures (bad input, busy
/// cell). It answers in the trace, so a tool loop is never left waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_answers_the_request_that_caused_it() {
    let (sink, mut rx, msg) = output_sink();
    emit::error(&sink, &msg, "call-9", "harness_busy", "a task is running").await;

    let em = rx.recv().await.expect("emission");
    assert_eq!(em.target, Path::new("/sink"));
    let body = body_of(&em);
    assert_eq!(body["header"]["harness_event"], "error");
    assert_eq!(body["header"]["error_code"], "harness_busy");
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["id"], "call-9");
    assert_eq!(body["messages"][0]["text"], "a task is running");
}
