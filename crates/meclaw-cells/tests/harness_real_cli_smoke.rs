//! P8 — the ONE paid run: the cell against the real Claude Code CLI.
//!
//! **This test costs money and is `#[ignore]`d.** `cargo test` never runs it;
//! it exists so the claim "the fixture speaks the same dialect as the real
//! thing" stays checkable instead of being a one-off assertion in a report.
//!
//! Run it deliberately:
//!
//! ```text
//! HARNESS_SMOKE_MODEL=sonnet \
//!   cargo test -p meclaw-cells --test harness_real_cli_smoke -- --ignored --nocapture
//! ```
//!
//! The model comes from `HARNESS_SMOKE_MODEL` and is never defaulted in code
//! (standing rule: models live in `${VAR}`). The run is bounded by
//! `--max-turns` and the smallest sensible task, and the receipt it prints —
//! session id, effective model, turns, cost — is what goes into the package
//! report. Nothing here is asserted from configuration: every number is read
//! back out of the harness's own result event.
//!
//! Three smokes live here. The first two differ only in what the child was
//! allowed to do: the 0.1.8 acceptance run (grant `Write`, mode `acceptEdits`,
//! no approval channel), and the GH #46.1 probe that grants nothing and
//! switches the approval channel on to find out whether the real CLI asks. The
//! third runs two tasks on one `persistent` cell and checks that they share a
//! session. All are `#[ignore]`d; all spend money. The #46.1 probe was red until
//! the adapter put the control lane on stdio; it is **expected green** since
//! then — see its doc comment for both measurements.
#![cfg(unix)]

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A live colony with a `harness` cell wired to the REAL `claude` binary.
///
/// Everything that decides what the child is *allowed* to do is a parameter,
/// because that is exactly what the two smokes below disagree about. The
/// containment triple travels together on purpose: a permission mode without
/// its tool list (or without the approval channel that answers a question)
/// says nothing on its own.
async fn topology(
    model: &str,
    permission_mode: &str,
    allowed_tools: serde_json::Value,
    approval: &str,
    session: &str,
) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("harness");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let workspaces = td.path().join("workspaces");
    std::fs::create_dir_all(workspaces.join("wt-1")).unwrap();

    let factory = Arc::new(HarnessCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/harness"),
            json!({
                "adapter": "claude-code",
                "command": "claude",
                "emit_to": "/sink",
                "workspace_root": workspaces.display().to_string(),
                "model": model,
                // Containment, decided by the caller — see the doc comment.
                "allowed_tools": allowed_tools,
                "permission_mode": permission_mode,
                "approval": approval,
                "session": session,
                "max_turns": 3,
                // Every LLM request names where it comes from.
                "x_title": "meclaw-lab harness-smoke",
                "http_referer": "https://meclaw.ai",
                // A real harness needs its own credentials and PATH; nothing
                // else of this process's environment travels with it. An
                // Anthropic-compatible endpoint may come from the environment
                // (names only); unset, the CLI keeps its own login.
                "env_passthrough": [
                    "PATH", "HOME", "USER", "LANG", "TERM",
                    "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"
                ],
                "startup_timeout_ms": 120000,
                "external_timeout_ms": 30000,
                "query_timeout_ms": 5000,
                "kill_grace_ms": 5000
            }),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .expect("spawn harness cell");
    h.register_spawned(Path::new("/harness"), spawned).await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/harness"),
        Path::new("/sink"),
    )
    .await;

    (h, recv_rx, td)
}

/// The permission mode the second smoke runs under. Changing this one constant
/// is the whole knob: two runs against CLI 2.1.237 differed in nothing else, so
/// the transcripts are comparable.
///
/// **Measured 2026-08-21, `sonnet`, both runs `status: ok`, no question in
/// either:** `"default"` (session `22bd82dc…`, 2 turns, $0.1256) and `"plan"`
/// (session `ba0c605f…`, 2 turns, $0.1500) each ran `Bash` and returned
/// `hello`, with `--allowedTools` omitted entirely. See the test's doc comment
/// for what that answers.
///
/// No question in either because `echo hello` is read-only, and the CLI
/// approves those by itself (measured 2026-11-11, #1489).
///
/// `"default"` since the control lane went to stdio: it is the mode the
/// 2026-11-08 measurement asked under.
const RESTRICTIVE_MODE: &str = "default";

fn tool_call(name: &str, arguments: serde_json::Value, call_id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/harness"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[
            {"origin":"assistant","type":"tool_call","text": inner, "id": call_id}
        ]})))
        .build()
}

#[ignore = "costs money: runs the real claude CLI"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_the_real_cli_runs_a_task_and_reports_its_cost() {
    let model = std::env::var("HARNESS_SMOKE_MODEL").expect(
        "set HARNESS_SMOKE_MODEL (no model default in code — standing rule: models come from ${VAR})",
    );
    // The values the acceptance smoke ran with, verbatim: file writing only,
    // `acceptEdits`, and no approval channel (the parse default this smoke
    // relied on when the key was absent). The task below also asks for a
    // shell command, but a read-only one: the CLI approves those by itself
    // (measured 2026-11-11, CLI 2.1.322, #1489), so this run asks nobody. The
    // case of a tool that was not granted is the next smoke's.
    let (h, mut sink, td) =
        topology(&model, "acceptEdits", json!(["Write"]), "off", "per_task").await;

    // Two things in one task: the first is the granted `Write`, the second a
    // read-only `ls -la` the CLI runs without asking, though `Bash` is not
    // granted. A `can_use_tool` question is only printed here, never asserted.
    h.send(tool_call(
        "start_task",
        json!({
            "task_id": "smoke-1",
            "prompt": "Create a file named hello.txt containing exactly the word hello. \
                       Then run `ls -la` to verify it. Then stop.",
            "workspace": "wt-1"
        }),
        "call_1",
    ))
    .await;

    let mut result = None;
    let mut saw_question = false;
    // Generous: a real model call takes as long as it takes.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    while std::time::Instant::now() < deadline {
        let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(300), sink.recv()).await else {
            break;
        };
        let hop = &m.headers.hop;
        let event = hop
            .get("harness_event")
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        println!("receipt: {event} {hop:?}");
        if event == "question" {
            saw_question = true;
        }
        if event == "result" {
            result = Some(m);
            break;
        }
    }

    let result = result.expect("no result emission from the real CLI within 300s");
    let hop = &result.headers.hop;

    // The receipt. Every value is read back from the harness's own events —
    // none of it is echoed configuration.
    println!("\n=== SMOKE RECEIPT ===");
    println!("status:      {}", hop["status"]);
    println!("session_id:  {:?}", hop.get("session_id"));
    println!("model:       {:?}", hop.get("model"));
    println!("num_turns:   {:?}", hop.get("num_turns"));
    println!("cost_usd:    {:?}", hop.get("cost_usd"));
    println!("duration_ms: {:?}", hop.get("duration_ms"));
    println!("saw a can_use_tool question: {saw_question}");
    if let Body::Inline(body) = &result.body {
        println!("summary:     {}", body["messages"][0]["text"]);
    }
    println!("=====================\n");

    assert_eq!(hop["status"], "ok", "the real run did not succeed: {hop:?}");
    assert!(
        hop.get("session_id").is_some(),
        "no session id — the audit trail depends on it: {hop:?}"
    );
    assert!(
        hop.get("model").is_some(),
        "no effective model reported: {hop:?}"
    );

    // The point of the whole cell type: the artifact is on disk, not in prose.
    let hello = td.path().join("workspaces/wt-1/hello.txt");
    let content = std::fs::read_to_string(&hello)
        .unwrap_or_else(|e| panic!("the harness reported success but wrote no {hello:?}: {e}"));
    assert!(
        content.to_lowercase().contains("hello"),
        "unexpected file content: {content:?}"
    );
}

/// GH #46.1 — does the REAL CLI ever ask?
///
/// The acceptance smoke never reached the `can_use_tool` path: it granted
/// `Write` under `acceptEdits`, and the model reached for `Bash`, which that
/// mode hands out anyway. So the control protocol stayed proven against the
/// fixture only. This run removes every grant instead of adding one — no
/// `--allowedTools` at all, the least permissive `--permission-mode` the CLI
/// takes, an approval channel that is switched ON (so the cell does not
/// auto-deny before the vendor can show its hand), and a prompt whose only way
/// forward is a shell command.
///
/// Two things are measured, both of them unproven before this test existed:
///   1. whether the real CLI sends `control_request`/`can_use_tool` at all
///      without the SDK's `initialize` handshake, and
///   2. whether it accepts our `control_response` on stdin when the process was
///      started without `--input-format stream-json` — which is why the answer
///      below is a real `deny` sent back through the cell rather than a
///      silently dropped one.
///
/// The assertions are on receipts, never on configuration. A red run here is
/// itself the finding; the transcript printed under `--nocapture` is the
/// deliverable either way.
///
/// **Outcome (2026-08-21, CLI 2.1.237, `sonnet`, both permission modes): RED,
/// and that is the answer.** Question 1 is answered in the negative — the real
/// CLI never sent a `control_request`, under `default` or under `plan`, with no
/// `--allowedTools` granted; it simply ran `Bash` and finished `ok`. Question 2
/// is therefore not reachable by this route at all: with no `request_id` ever
/// issued, there is nothing to answer, so whether a bare stdin would carry a
/// `control_response` stays untested. The one adjacent fact the run did produce
/// is the child's own warning, `no stdin data received in 3s, proceeding
/// without it` — stdin is read once at startup as a prompt lane and then
/// abandoned, not held open as a control lane.
///
/// **Expected green since the adapter speaks stream-json in both directions
/// and passes `--permission-prompt-tool stdio`.** Measured 2026-11-08 against
/// CLI 2.1.319 and again 2026-11-11 against CLI 2.1.322 (`haiku`, outside this
/// test, same flags): the CLI sends
/// `control_request`/`can_use_tool` with a `request_id`, and a
/// `control_response` in the cell's shape works both ways — `deny` keeps the
/// tool from running, `allow` (without `updatedInput`) lets it run. Without the
/// prompt-tool flag the CLI refuses on its own and nobody is asked. A red run
/// now means the CLI changed. Do not "fix" it by weakening the assertion.
///
/// **The task asks for a writing command.** The CLI approves read-only
/// commands such as `echo hello` by itself, with or without user settings
/// (measured 2026-11-11, CLI 2.1.322, `haiku`, #1489): no question is sent and
/// the run proves nothing about the channel. `echo hi > probe.txt` is asked
/// every time; after the `deny` the file must not exist.
#[ignore = "costs money: runs the real claude CLI — expected green, see doc comment"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_the_real_cli_asks_before_a_tool_it_was_not_granted() {
    let model = std::env::var("HARNESS_SMOKE_MODEL").expect(
        "set HARNESS_SMOKE_MODEL (no model default in code — standing rule: models come from ${VAR})",
    );
    let (h, mut sink, td) =
        topology(&model, RESTRICTIVE_MODE, json!([]), "channel", "per_task").await;

    h.send(tool_call(
        "start_task",
        json!({
            "task_id": "smoke-2",
            "prompt": "Run `echo hi > probe.txt` with Bash and report its output verbatim. \
                       Do nothing else.",
            "workspace": "wt-1"
        }),
        "call_1",
    ))
    .await;

    let mut question: Option<(String, String)> = None;
    let mut result = None;
    let mut receipts = 0usize;
    // Bounded on purpose: if the child asks and nothing ever answers, the run
    // would otherwise hang forever, and "it hung" is a finding we want to reach
    // in finite time rather than a test that never returns.
    let deadline = std::time::Instant::now() + Duration::from_secs(240);
    while std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, sink.recv()).await else {
            break;
        };
        receipts += 1;
        let hop = &m.headers.hop;
        let event = hop
            .get("harness_event")
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        println!("receipt {receipts}: {event} {hop:?}");
        if event == "question" && question.is_none() {
            let request_id = hop
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let tool_name = hop
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            question = Some((request_id.clone(), tool_name.clone()));
            // Answer it for real. This is the only way to learn whether the
            // child reads a `control_response` off a plain stdin.
            println!("--> answering deny for request_id={request_id:?} tool={tool_name:?}");
            h.send(tool_call(
                "answer",
                json!({
                    "task_id": "smoke-2",
                    "request_id": request_id,
                    "behavior": "deny",
                    "message": "smoke run: the topology denies every tool"
                }),
                "call_2",
            ))
            .await;
        }
        if event == "result" {
            result = Some(m);
            break;
        }
    }

    println!("\n=== SMOKE RECEIPT (GH #46.1) ===");
    println!("permission_mode: {RESTRICTIVE_MODE}");
    println!("allowed_tools:   [] (flag omitted entirely)");
    println!("approval:        channel");
    println!("receipts seen:   {receipts}");
    println!("question:        {question:?}");
    match &result {
        Some(m) => {
            let hop = &m.headers.hop;
            println!("status:      {}", hop["status"]);
            println!("error_code:  {:?}", hop.get("error_code"));
            println!("session_id:  {:?}", hop.get("session_id"));
            println!("model:       {:?}", hop.get("model"));
            println!("num_turns:   {:?}", hop.get("num_turns"));
            println!("cost_usd:    {:?}", hop.get("cost_usd"));
            println!("duration_ms: {:?}", hop.get("duration_ms"));
            if let Body::Inline(body) = &m.body {
                println!("summary:     {}", body["messages"][0]["text"]);
            }
        }
        None => println!("status:      <no terminal result within 240s>"),
    }
    println!("================================\n");

    let (request_id, tool_name) = question.expect(
        "the real CLI ran a writing Bash command to its end without ever sending \
         control_request/can_use_tool — the approval channel is not reached in this permission mode",
    );
    assert!(
        !request_id.is_empty(),
        "a question without a request_id cannot be answered"
    );
    assert!(
        !tool_name.is_empty(),
        "a question without a tool_name cannot be judged"
    );

    let result = result.expect("a question was asked but the run never reached a terminal result");
    let hop = &result.headers.hop;
    assert!(
        hop.get("harness_event").and_then(|v| v.as_str()) == Some("result"),
        "not a terminal receipt: {hop:?}"
    );
    assert!(
        hop.get("session_id").is_some(),
        "no session id — the audit trail depends on it: {hop:?}"
    );
    let probe = td.path().join("workspaces/wt-1/probe.txt");
    assert!(
        !probe.exists(),
        "the question was denied, yet the writing command ran: {probe:?} exists"
    );
}

/// D57: a `persistent` cell keeps ONE session across two tasks. The second
/// task is a turn of the same session (same `session_id`), and its answer knows
/// a word only the first task said.
#[ignore = "costs money: runs the real claude CLI"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_the_real_cli_keeps_one_session_across_two_tasks() {
    let model = std::env::var("HARNESS_SMOKE_MODEL").expect(
        "set HARNESS_SMOKE_MODEL (no model default in code — standing rule: models come from ${VAR})",
    );
    let (h, mut sink, _td) = topology(&model, "default", json!([]), "off", "persistent").await;

    let prompts = [
        "Remember the word PELICAN. Reply with exactly: OK",
        "Which word did I ask you to remember? Reply with that word only.",
    ];
    let mut results = Vec::new();
    for (i, prompt) in prompts.iter().enumerate() {
        let task_id = format!("smoke-3-{}", i + 1);
        h.send(tool_call(
            "start_task",
            json!({"task_id": task_id, "prompt": prompt, "workspace": "wt-1"}),
            &format!("call_{}", i + 1),
        ))
        .await;
        let deadline = std::time::Instant::now() + Duration::from_secs(240);
        let mut result = None;
        while std::time::Instant::now() < deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(Some(m)) = tokio::time::timeout(left, sink.recv()).await else {
                break;
            };
            let hop = &m.headers.hop;
            let event = hop
                .get("harness_event")
                .and_then(|v| v.as_str())
                .unwrap_or("<none>");
            println!("receipt {task_id}: {event} {hop:?}");
            if event == "result" {
                result = Some(m);
                break;
            }
        }
        results.push(result.unwrap_or_else(|| panic!("no result for {task_id} within 240s")));
    }

    println!("\n=== SMOKE RECEIPT (persistent) ===");
    for m in &results {
        let hop = &m.headers.hop;
        println!("task_id:     {}", hop["task_id"]);
        println!("status:      {}", hop["status"]);
        println!("session_id:  {:?}", hop.get("session_id"));
        println!("model:       {:?}", hop.get("model"));
        println!("num_turns:   {:?}", hop.get("num_turns"));
        println!("cost_usd:    {:?}", hop.get("cost_usd"));
        println!("duration_ms: {:?}", hop.get("duration_ms"));
        if let Body::Inline(body) = &m.body {
            println!("summary:     {}", body["messages"][0]["text"]);
        }
    }
    println!("==================================\n");

    for m in &results {
        assert_eq!(m.headers.hop["status"], "ok", "{:?}", m.headers.hop);
    }
    let first = results[0].headers.hop.get("session_id").cloned();
    assert!(first.is_some(), "no session id on the first task");
    assert_eq!(
        results[1].headers.hop.get("session_id").cloned(),
        first,
        "the second task ran in another session"
    );
    let Body::Inline(body) = &results[1].body else {
        panic!("no inline body on the second result")
    };
    let text = body["messages"][0]["text"].to_string().to_uppercase();
    assert!(
        text.contains("PELICAN"),
        "the second task does not know the first: {text}"
    );
}
