//! P7 block 2 — the serve loop of the stdio-child core, driven against the
//! real `line_json_test_server` fixture.

use meclaw_cells::stdio_child::{
    ChildCommand, ChildEvent, ChildExit, ChildSpec, CorrelationKey, ServeConfig, StdioChild,
    serve::{serve_child, serve_child_until_exit},
};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

fn fixture_spec(args: &[&str]) -> ChildSpec {
    ChildSpec {
        program: FIXTURE.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: Vec::new(),
        cwd: None,
        kill_grace_ms: 500,
        ..ChildSpec::default()
    }
}

fn serve_config() -> ServeConfig {
    ServeConfig {
        write_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(500),
    }
}

/// Correlate on a top-level `id` field, the JSON-RPC shape.
fn by_id(v: &JsonValue) -> Option<CorrelationKey> {
    match &v["id"] {
        JsonValue::Null => None,
        other => Some(other.to_string()),
    }
}

/// Start the fixture under `serve_child` on its own task.
fn start(args: &[&str]) -> (mpsc::Sender<ChildCommand>, mpsc::Receiver<ChildEvent>) {
    let child = StdioChild::spawn(&fixture_spec(args)).expect("spawn fixture");
    let (cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, ev_rx) = mpsc::channel::<ChildEvent>(64);
    tokio::spawn(serve_child(child, by_id, serve_config(), cmd_rx, ev_tx));
    (cmd_tx, ev_rx)
}

/// Next event or a loud timeout — a hung test must name itself, not stall.
async fn next_event(rx: &mut mpsc::Receiver<ChildEvent>) -> ChildEvent {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no event within 30s")
        .expect("event channel closed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uncorrelated_answer_arrives_as_a_frame_event() {
    let (cmds, mut events) = start(&["echo"]);
    cmds.send(ChildCommand::Send {
        line: json!({"hello": "world"}),
        correlate: None,
        reply: None,
    })
    .await
    .expect("send");

    match next_event(&mut events).await {
        ChildEvent::Frame(v) => assert_eq!(v["hello"], "world"),
        other => panic!("expected a frame event, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_correlated_answer_bypasses_the_event_channel() {
    let (cmds, mut events) = start(&["echo"]);

    // A foreign, uncorrelated frame first, then the correlated request. The
    // answer must reach the oneshot, and only the foreign frame the events.
    cmds.send(ChildCommand::Send {
        line: json!({"note": "push"}),
        correlate: None,
        reply: None,
    })
    .await
    .expect("send push");

    let (tx, rx) = oneshot::channel();
    cmds.send(ChildCommand::Send {
        line: json!({"id": 42, "v": "answer"}),
        correlate: Some("42".to_string()),
        reply: Some(tx),
    })
    .await
    .expect("send request");

    let answer = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .expect("no answer within 30s")
        .expect("oneshot dropped")
        .expect("child error");
    assert_eq!(answer["v"], "answer");

    match next_event(&mut events).await {
        ChildEvent::Frame(v) => assert_eq!(v["note"], "push"),
        other => panic!("expected the push frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_json_line_is_reported_and_does_not_end_the_loop() {
    let (cmds, mut events) = start(&["echo", "--banner"]);

    match next_event(&mut events).await {
        ChildEvent::Malformed { raw } => assert_eq!(raw, "line_json_test_server ready"),
        other => panic!("expected the banner as a malformed frame, got {other:?}"),
    }

    // The blank line must have been skipped silently, and the loop must still
    // serve: a normal exchange still works.
    cmds.send(ChildCommand::Send {
        line: json!({"still": "alive"}),
        correlate: None,
        reply: None,
    })
    .await
    .expect("send");
    match next_event(&mut events).await {
        ChildEvent::Frame(v) => assert_eq!(v["still"], "alive"),
        other => panic!("expected a frame after the banner, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dying_child_answers_the_in_flight_request_before_announcing_its_death() {
    let (cmds, mut events) = start(&["die-on-request", "3"]);

    let (tx, rx) = oneshot::channel();
    cmds.send(ChildCommand::Send {
        line: json!({"id": 1}),
        correlate: Some("1".to_string()),
        reply: Some(tx),
    })
    .await
    .expect("send");

    // Ratified liveness slice D1, first half: the pending request resolves
    // with the child's fate instead of hanging forever.
    let err = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .expect("no resolution within 30s")
        .expect("oneshot dropped without a value")
        .expect_err("a dead child cannot answer");
    assert_eq!(
        err.detail(),
        "child gone: exit code 3",
        "wrong fate reported"
    );

    // Second half: only then does the death reach the handler.
    match next_event(&mut events).await {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "exit code 3"),
        other => panic!("expected the exit event, got {other:?}"),
    }
}

/// P8: a consumer's command enum may carry commands that are NOT for the child.
///
/// The `harness` cell type sends "start a task" over the same channel it sends
/// child writes over, because the I/O sub-task owns both the idle wait and the
/// running child. While a child is running such a command cannot be honoured —
/// and must not end the loop either. It is dropped with a warning, and the
/// loop keeps serving.
#[derive(Debug)]
enum MixedCommand {
    ForTheChild(ChildCommand),
    ForTheIoTaskItself,
}

impl TryFrom<MixedCommand> for ChildCommand {
    type Error = ();
    fn try_from(c: MixedCommand) -> Result<Self, ()> {
        match c {
            MixedCommand::ForTheChild(c) => Ok(c),
            MixedCommand::ForTheIoTaskItself => Err(()),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_that_is_not_for_the_child_is_skipped_not_fatal() {
    let child = StdioChild::spawn(&fixture_spec(&["echo"])).expect("spawn fixture");
    let (cmd_tx, cmd_rx) = mpsc::channel::<MixedCommand>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<ChildEvent>(64);
    tokio::spawn(serve_child(child, by_id, serve_config(), cmd_rx, ev_tx));

    cmd_tx
        .send(MixedCommand::ForTheIoTaskItself)
        .await
        .expect("send the foreign command");
    cmd_tx
        .send(MixedCommand::ForTheChild(ChildCommand::Send {
            line: json!({"still": "alive"}),
            correlate: None,
            reply: None,
        }))
        .await
        .expect("send a real command");

    // Had the foreign command been treated as a shutdown (or a conversion
    // panic), this frame would never arrive.
    match next_event(&mut ev_rx).await {
        ChildEvent::Frame(v) => assert_eq!(v["still"], "alive"),
        other => panic!("the loop did not survive the foreign command: {other:?}"),
    }
}

/// P8: the same loop, but as the borrowing variant that HANDS BACK the fate.
///
/// `mcp` needs the parking form (a dead child ends the cell); a `harness` needs
/// this one, because there a dead child is the normal end of one task and the
/// I/O task has to go back to waiting for the next one. Both share the loop —
/// only the epilogue differs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_child_until_exit_hands_the_fate_back_to_its_caller() {
    let child = StdioChild::spawn(&fixture_spec(&["exit", "3"])).expect("spawn fixture");
    let (_cmd_tx, mut cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<ChildEvent>(64);
    let join = tokio::spawn(async move {
        serve_child_until_exit(child, by_id, serve_config(), &mut cmd_rx, &ev_tx).await
    });

    let exit = tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("serve_child_until_exit did not return within 30s")
        .expect("task panicked");
    assert_eq!(exit, ChildExit::Code(3), "wrong fate handed back");

    // The event is still emitted — the caller learns the fate twice, once for
    // its own control flow and once through the handler's event channel.
    match ev_rx.recv().await.expect("event channel closed") {
        ChildEvent::Exited(x) => assert_eq!(x, ChildExit::Code(3)),
        other => panic!("expected the exit event, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_child_parks_after_the_child_died_and_never_returns() {
    let child = StdioChild::spawn(&fixture_spec(&["exit", "0"])).expect("spawn fixture");
    let (_cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<ChildEvent>(64);
    let join = tokio::spawn(serve_child(child, by_id, serve_config(), cmd_rx, ev_tx));

    match next_event(&mut ev_rx).await {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "exit code 0"),
        other => panic!("expected the exit event, got {other:?}"),
    }

    // A1' (overview § Long-running cells: double task): a clean return here
    // would let the outer select! abort a healthy handler. The loop must park.
    let parked = tokio::time::timeout(Duration::from_millis(300), join).await;
    assert!(parked.is_err(), "serve_child returned instead of parking");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_terminates_the_child_and_reports_the_exit() {
    let (cmds, mut events) = start(&["echo"]);
    cmds.send(ChildCommand::Shutdown)
        .await
        .expect("send shutdown");

    match next_event(&mut events).await {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "exit code 0"),
        other => panic!("expected the exit event, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_command_channel_shuts_the_child_down_too() {
    let (cmds, mut events) = start(&["echo"]);
    drop(cmds);

    match next_event(&mut events).await {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "exit code 0"),
        other => panic!("expected the exit event, got {other:?}"),
    }
}

/// The ordering proof for the ratified liveness slice D1.
///
/// The buffered variant above cannot discriminate: with a roomy events channel
/// both orderings look identical from outside. Here the events channel is
/// capacity 1 and is deliberately left FULL (the banner's malformed frame sits
/// in it). If the loop announced the death BEFORE resolving the in-flight
/// request, `events.send(Exited)` would block on the full channel and the
/// pending request would never be answered — the assert below would time out.
/// Mutation-checked: swapping the two lines in `serve_child` turns this red.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_in_flight_answer_does_not_wait_on_event_channel_capacity() {
    let child = StdioChild::spawn(&fixture_spec(&["die-on-request", "3", "--banner"]))
        .expect("spawn fixture");
    let (cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<ChildEvent>(1);
    tokio::spawn(serve_child(child, by_id, serve_config(), cmd_rx, ev_tx));

    // Let the banner fill the single event slot, and do NOT drain it.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let (tx, rx) = oneshot::channel();
    cmd_tx
        .send(ChildCommand::Send {
            line: json!({"id": 1}),
            correlate: Some("1".to_string()),
            reply: Some(tx),
        })
        .await
        .expect("send");

    let err = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("in-flight request was not answered before the death was announced")
        .expect("oneshot dropped without a value")
        .expect_err("a dead child cannot answer");
    assert_eq!(err.detail(), "child gone: exit code 3");

    // The channel really was full: the banner is still sitting in it.
    match ev_rx.recv().await.expect("event channel closed") {
        ChildEvent::Malformed { raw } => assert_eq!(raw, "line_json_test_server ready"),
        other => panic!("expected the buffered banner, got {other:?}"),
    }
}

/// Regression: a request that arrives AFTER the child died must be answered
/// immediately, not left to time out.
///
/// Found while wiring the mcp cell: the handler's `select!` is biased towards
/// its mailbox over its event channel, so it can accept one more message
/// before it has even seen `Exited`. When the serve loop parked silently after
/// the death, that late request sat in the command channel until its A-timeout
/// elapsed — a known death surfacing as a spurious `provider_timeout`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_after_the_child_died_fails_fast_instead_of_timing_out() {
    let (cmds, mut events) = start(&["die-on-request", "3"]);

    cmds.send(ChildCommand::Send {
        line: json!({"id": 1}),
        correlate: Some("1".to_string()),
        reply: None,
    })
    .await
    .expect("send the killing request");

    match next_event(&mut events).await {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "exit code 3"),
        other => panic!("expected the exit event, got {other:?}"),
    }

    let (tx, rx) = oneshot::channel();
    cmds.send(ChildCommand::Send {
        line: json!({"id": 2}),
        correlate: Some("2".to_string()),
        reply: Some(tx),
    })
    .await
    .expect("send the late request");

    // Tight on purpose: "fast" is the whole point. A parked loop would need
    // the caller's full A-timeout instead.
    let err = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("late request was not answered within 2s")
        .expect("oneshot dropped without a value")
        .expect_err("a dead child cannot answer");
    assert_eq!(err.detail(), "child gone: exit code 3");
}
