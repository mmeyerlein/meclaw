//! P8 block 4 — the Claude Code adapter: argv construction and frame
//! classification.
//!
//! Both are pure functions, and deliberately so: everything vendor-specific
//! about this cell type is a translation between one JSON dialect and this
//! package's own vocabulary. Nothing here spawns, waits or emits.

use meclaw_cells::harness::claude_code::{self, HarnessSignal};
use meclaw_cells::harness::{HarnessParams, TaskRequest};
use serde_json::json;

fn params(extra: serde_json::Value) -> (tempfile::TempDir, HarnessParams) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = json!({
        "adapter": "claude-code",
        "emit_to": "/main/coordinator",
        "workspace_root": dir.path().display().to_string(),
    });
    for (k, v) in extra.as_object().expect("object") {
        cfg[k] = v.clone();
    }
    let p = HarnessParams::parse(&cfg).expect("parse");
    (dir, p)
}

fn task(prompt: &str) -> TaskRequest {
    TaskRequest {
        task_id: "t1".to_string(),
        prompt: prompt.to_string(),
        workspace: "ws".to_string(),
        resume_session_id: None,
        model: None,
        max_turns: None,
    }
}

/// The minimum every run needs: print mode, the streaming dialect, and the
/// prompt. `--verbose` comes along because the streaming format is only fully
/// populated with it.
#[test]
fn the_baseline_argv_asks_for_print_mode_and_the_streaming_dialect() {
    let (_dir, p) = params(json!({}));
    let argv = claude_code::build_argv(&p, &task("do the thing"));

    assert_eq!(argv[0], "-p");
    assert_eq!(argv[1], "do the thing");
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--output-format" && w[1] == "stream-json")
    );
    assert!(argv.contains(&"--verbose".to_string()));

    // Nothing is invented: no model, no budget, no permission mode unless the
    // operator configured one.
    assert!(!argv.contains(&"--model".to_string()));
    assert!(!argv.contains(&"--max-turns".to_string()));
    assert!(!argv.contains(&"--permission-mode".to_string()));
    assert!(!argv.contains(&"--resume".to_string()));
}

#[test]
fn configured_limits_and_the_model_are_passed_through() {
    let (_dir, p) = params(json!({
        "model": "some-model-id",
        "permission_mode": "acceptEdits",
        "max_turns": 7,
        "max_budget_usd": 0.5,
        "allowed_tools": ["Bash", "Write"],
        "extra_args": ["--strict-mcp-config"],
    }));
    let argv = claude_code::build_argv(&p, &task("go"));

    let pairs: Vec<(String, String)> = argv
        .windows(2)
        .map(|w| (w[0].clone(), w[1].clone()))
        .collect();
    assert!(pairs.contains(&("--model".to_string(), "some-model-id".to_string())));
    assert!(pairs.contains(&("--permission-mode".to_string(), "acceptEdits".to_string())));
    assert!(pairs.contains(&("--max-turns".to_string(), "7".to_string())));
    assert!(pairs.contains(&("--max-budget-usd".to_string(), "0.5".to_string())));
    assert!(pairs.contains(&("--allowedTools".to_string(), "Bash,Write".to_string())));
    assert!(argv.contains(&"--strict-mcp-config".to_string()));
}

/// Per-task overrides beat the cell's configuration — a topology may run a
/// cheap model for one task and a strong one for the next.
#[test]
fn the_task_overrides_the_cell_configuration() {
    let (_dir, p) = params(json!({"model": "cell-model", "max_turns": 7}));
    let mut t = task("go");
    t.model = Some("task-model".to_string());
    t.max_turns = Some(2);
    t.resume_session_id = Some("sess-9".to_string());

    let argv = claude_code::build_argv(&p, &t);
    let pairs: Vec<(String, String)> = argv
        .windows(2)
        .map(|w| (w[0].clone(), w[1].clone()))
        .collect();
    assert!(pairs.contains(&("--model".to_string(), "task-model".to_string())));
    assert!(pairs.contains(&("--max-turns".to_string(), "2".to_string())));
    assert!(pairs.contains(&("--resume".to_string(), "sess-9".to_string())));
    assert!(
        !pairs.contains(&("--model".to_string(), "cell-model".to_string())),
        "the cell default must not be passed alongside the override"
    );
}

// ---- frame classification ----

#[test]
fn the_init_event_becomes_a_start_signal_with_session_and_model() {
    let f = json!({"type": "system", "subtype": "init",
                   "session_id": "s1", "model": "m1", "tools": []});
    match claude_code::classify(&f) {
        HarnessSignal::Started { session_id, model } => {
            assert_eq!(session_id, "s1");
            assert_eq!(model.as_deref(), Some("m1"));
        }
        other => panic!("expected Started, got {other:?}"),
    }
}

#[test]
fn an_assistant_text_turn_becomes_progress() {
    let f = json!({"type": "assistant",
                   "message": {"content": [{"type": "text", "text": "thinking"}]}});
    match claude_code::classify(&f) {
        HarnessSignal::Progress {
            phase,
            tool_name,
            text,
        } => {
            assert_eq!(phase, "text");
            assert_eq!(tool_name, None);
            assert_eq!(text.as_deref(), Some("thinking"));
        }
        other => panic!("expected Progress, got {other:?}"),
    }
}

/// A tool call is the interesting kind of progress: it names what the harness
/// is about to do to the workspace.
#[test]
fn a_tool_call_becomes_progress_that_names_the_tool() {
    let f = json!({"type": "assistant",
                   "message": {"content": [
                       {"type": "tool_use", "id": "tu1", "name": "Bash",
                        "input": {"command": "ls"}}]}});
    match claude_code::classify(&f) {
        HarnessSignal::Progress {
            phase, tool_name, ..
        } => {
            assert_eq!(phase, "tool");
            assert_eq!(tool_name.as_deref(), Some("Bash"));
        }
        other => panic!("expected Progress, got {other:?}"),
    }
}

#[test]
fn the_result_event_carries_the_outcome_and_what_it_cost() {
    let f = json!({"type": "result", "subtype": "success", "is_error": false,
                   "num_turns": 3, "total_cost_usd": 0.125,
                   "result": "created hello.txt", "session_id": "s1"});
    match claude_code::classify(&f) {
        HarnessSignal::Finished {
            ok,
            num_turns,
            cost_usd,
            text,
        } => {
            assert!(ok);
            assert_eq!(num_turns, Some(3));
            assert_eq!(cost_usd, Some(0.125));
            assert_eq!(text.as_deref(), Some("created hello.txt"));
        }
        other => panic!("expected Finished, got {other:?}"),
    }
}

/// `is_error` decides, not the subtype string: a harness that failed still
/// writes a result event, and reporting it as success would be the worst kind
/// of wrong.
#[test]
fn a_failed_result_is_not_reported_as_success() {
    let f = json!({"type": "result", "subtype": "error_max_turns", "is_error": true,
                   "num_turns": 9, "result": "ran out of turns"});
    match claude_code::classify(&f) {
        HarnessSignal::Finished { ok, .. } => assert!(!ok),
        other => panic!("expected Finished, got {other:?}"),
    }
}

#[test]
fn a_permission_request_becomes_a_question() {
    let f = json!({"type": "control_request", "request_id": "r1",
                   "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                               "input": {"command": "rm -rf /"}}});
    match claude_code::classify(&f) {
        HarnessSignal::Question {
            request_id,
            tool_name,
            input,
        } => {
            assert_eq!(request_id, "r1");
            assert_eq!(tool_name, "Bash");
            assert_eq!(input["command"], "rm -rf /");
        }
        other => panic!("expected Question, got {other:?}"),
    }
}

/// Anything unrecognised is ignored rather than guessed at — the vendor adds
/// event types faster than this adapter can grow arms for them.
#[test]
fn unknown_frames_are_ignored() {
    for f in [
        json!({"type": "stream_event", "event": {"delta": {"text": "x"}}}),
        json!({"type": "system", "subtype": "api_retry"}),
        json!({"type": "control_request", "request_id": "r", "request": {"subtype": "initialize"}}),
        json!({"hello": "world"}),
    ] {
        assert!(
            matches!(claude_code::classify(&f), HarnessSignal::Ignored),
            "expected Ignored for {f}"
        );
    }
}

#[test]
fn a_control_response_is_built_in_the_vendors_shape() {
    let allow = claude_code::build_control_response("r1", true, "");
    assert_eq!(allow["type"], "control_response");
    assert_eq!(allow["response"]["request_id"], "r1");
    assert_eq!(allow["response"]["subtype"], "success");
    assert_eq!(allow["response"]["response"]["behavior"], "allow");

    let deny = claude_code::build_control_response("r2", false, "not allowed here");
    assert_eq!(deny["response"]["response"]["behavior"], "deny");
    assert_eq!(deny["response"]["response"]["message"], "not allowed here");
}
