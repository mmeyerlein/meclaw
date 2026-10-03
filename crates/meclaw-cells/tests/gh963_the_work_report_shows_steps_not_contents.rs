//! GH #963 -- the work report shows steps, files and the test verdict, never a file's
//! content or a command's output.
//!
//! Three observed calls of the generation -- a file read, a red test run, a green test
//! run -- each with its result, then a turn the decider is sure about as `work`: the
//! window shows `display-steps` with the three steps and the list of touched files. The
//! file's text and both outputs travelled in the results; none of them reaches the screen.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, boot, decision, guard, warm};

fn ctx() -> Value {
    json!({"audience_set": r#"["agent:g1","member:alex"]"#, "tool_caller": "talky",
           "assistant": "g1", "session_id": "s1"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_work_report_shows_steps_not_contents() {
    if !guard("the_work_report_shows_steps_not_contents") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;

    s.tool_call(
        "file",
        "a1",
        json!({"op": "read", "path": "src/lib.rs"}),
        ctx(),
    )
    .await;
    s.tool_result(
        "a1",
        "fn hidden_file_content() {}",
        json!({"operation": "read", "bytes": 27}),
        ctx(),
    )
    .await;
    s.tool_call(
        "bash",
        "a2",
        json!({"command": "cargo test -p probe"}),
        ctx(),
    )
    .await;
    s.tool_result(
        "a2",
        "test result: FAILED red_output_line",
        json!({"operation": "bash", "exit_code": 101}),
        ctx(),
    )
    .await;
    s.tool_call(
        "bash",
        "a3",
        json!({"command": "cargo test -p probe"}),
        ctx(),
    )
    .await;
    s.tool_result(
        "a3",
        "test result: ok green_output_line",
        json!({"operation": "bash", "exit_code": 0}),
        ctx(),
    )
    .await;

    s.turn_in(
        "w1",
        "what are you working on right now",
        json!(["agent:g1", "member:alex"]),
    )
    .await;
    s.ask().await.release(decision(&[
        ("topic", "work", 0.9),
        ("work.lead", "steps", 0.9),
        ("work.also", "files", 0.9),
    ]));
    s.wait_drawn("show-work-files").await;
    let steps = s.ids("show-work-steps-").await;
    assert_eq!(steps.len(), 3, "{steps:?}");
    let files = s.ids("show-work-files-").await;
    assert_eq!(files.len(), 1, "{files:?}");
    let tree = s.c.tree().await.to_string();
    assert!(tree.contains("src/lib.rs"), "the touched file is named");
    for secret in [
        "hidden_file_content",
        "red_output_line",
        "green_output_line",
        "cargo test",
    ] {
        assert!(!tree.contains(secret), "`{secret}` reached the screen");
    }
    let row = s.journal_of("w1").await;
    assert_eq!(
        (row["topic"].clone(), row["fallback"].clone()),
        (json!("work"), json!("none")),
        "{row}"
    );
    s.c.shutdown().await;
}
