//! P8 block 4 — parsing what a message asks of a harness cell.
//!
//! Every reject here prevents a badly specified agent run. The strictest rule
//! is that `task_id` is mandatory: it is the dedup key, and a cell that
//! invented one would run a retried request twice.

use meclaw_cells::harness::{HarnessCommand, TaskRequest};
use serde_json::json;

#[test]
fn a_full_start_task_parses_into_a_task_request() {
    let cmd = HarnessCommand::parse(
        "start_task",
        &json!({
            "task_id": "t-1", "prompt": "fix the bug", "workspace": "wt-1",
            "resume_session_id": "s-9", "model": "m", "max_turns": 5
        }),
    )
    .expect("parse");

    assert_eq!(
        cmd,
        HarnessCommand::Start(TaskRequest {
            task_id: "t-1".to_string(),
            prompt: "fix the bug".to_string(),
            workspace: "wt-1".to_string(),
            resume_session_id: Some("s-9".to_string()),
            model: Some("m".to_string()),
            max_turns: Some(5),
        })
    );
}

/// The three keys without which a run cannot be specified — including the
/// dedup key, which is deliberately NOT generated when absent.
#[test]
fn start_task_rejects_a_missing_id_prompt_or_workspace_by_name() {
    let full = json!({"task_id": "t-1", "prompt": "p", "workspace": "w"});
    for key in ["task_id", "prompt", "workspace"] {
        let mut args = full.clone();
        args.as_object_mut().expect("object").remove(key);
        let err = HarnessCommand::parse("start_task", &args).expect_err("must reject");
        assert!(err.contains(key), "the reject must name {key}, got: {err}");

        // Blank is as absent as missing.
        let mut args = full.clone();
        args[key] = json!("   ");
        let err = HarnessCommand::parse("start_task", &args).expect_err("must reject blanks");
        assert!(err.contains(key), "got: {err}");
    }
}

#[test]
fn an_answer_needs_a_decision_it_can_act_on() {
    let ok = HarnessCommand::parse(
        "answer",
        &json!({"task_id": "t-1", "request_id": "r-1", "behavior": "deny", "message": "no"}),
    )
    .expect("parse");
    match ok {
        HarnessCommand::Answer(a) => {
            assert!(!a.allow);
            assert_eq!(a.message, "no");
            assert_eq!(a.request_id, "r-1");
        }
        other => panic!("expected Answer, got {other:?}"),
    }

    let err = HarnessCommand::parse(
        "answer",
        &json!({"task_id": "t-1", "request_id": "r-1", "behavior": "maybe"}),
    )
    .expect_err("must reject");
    assert!(err.contains("behavior"), "got: {err}");
}

#[test]
fn cancel_and_status_need_the_task_they_are_about() {
    match HarnessCommand::parse("cancel", &json!({"task_id": "t-1", "reason": "obsolete"}))
        .expect("parse")
    {
        HarnessCommand::Cancel { task_id, reason } => {
            assert_eq!(task_id, "t-1");
            assert_eq!(reason, "obsolete");
        }
        other => panic!("expected Cancel, got {other:?}"),
    }

    // A cancel without a reason still works — stopping is more important than
    // explaining.
    match HarnessCommand::parse("cancel", &json!({"task_id": "t-1"})).expect("parse") {
        HarnessCommand::Cancel { reason, .. } => assert_eq!(reason, "cancelled"),
        other => panic!("expected Cancel, got {other:?}"),
    }

    for name in ["cancel", "status"] {
        let err = HarnessCommand::parse(name, &json!({})).expect_err("must reject");
        assert!(err.contains("task_id"), "got: {err}");
    }
}

#[test]
fn an_unknown_tool_name_lists_the_known_ones() {
    let err = HarnessCommand::parse("do_something", &json!({})).expect_err("must reject");
    assert!(err.contains("start_task"), "got: {err}");
    assert!(err.contains("cancel"), "got: {err}");
}
