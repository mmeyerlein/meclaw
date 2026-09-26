//! GH #852 — the shipped high-frequency sinks and gates run `warm`.
//!
//! `terminal` sits at the end of every lane it is wired on, `daily-digest/void`
//! swallows every inbound chat of a push-only bot, and `affinity/gate` sits
//! under every door of a member. All three ran `cold`: a fresh interpreter per
//! message and, under the default-deny profile every instantiated `code` cell
//! gets (`network: deny`), a fresh user and network namespace as well. A burst
//! of messages was a burst of namespace setups, and the mailbox ran full while
//! they ran.
//!
//! What is pinned here, each at the receiver:
//!
//! 1. **the declarations** — the three shipped configs say `runner_mode: "warm"`,
//!    their scripts read stdin as text only (the warm harness hands them an
//!    `io.StringIO`, which has no `buffer` and no `fileno()`) and declare no
//!    `global` state, and a cell that says nothing still runs `cold`;
//! 2. **the reason** — a warm sink behind the default-deny profile takes a
//!    5 s flood of 200 messages a second through a mailbox of 64 without one
//!    `Full`, and answers all of them; 100 messages cost `warm` at most a fifth
//!    of what they cost `cold`;
//! 3. **the move changes nothing but cost** — the shipped gate answers the
//!    same message stream with the same emissions under `warm` as under `cold`.

use meclaw_cells::code::{CodeCellFactory, CodeParams, RunnerMode};
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo: generous, never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// The three cells #852 moved, as they ship.
const WARM_CELLS: [&str; 3] = [
    "templates/terminal/config.json",
    "templates/daily-digest/void/config.json",
    "templates/affinity/gate/config.json",
];

fn shipped(rel: &str) -> Value {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The block a cell instantiated from a template gets when it declares none
/// (`meclaw-colony` `default_sandbox_block`, GH #85).
fn default_deny() -> Value {
    json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}})
}

type Cell = (
    mpsc::Sender<Message>,
    mpsc::Receiver<CellEmission>,
    tempfile::TempDir,
);

fn spawn(params: Value, mailbox: usize, multi_send: bool) -> Cell {
    let (otx, orx) = mpsc::channel(4096);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/sink"),
            params,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView {
                multi_send_capable: multi_send,
                ..Default::default()
            },
            itx,
            None,
            0,
            None,
            None,
            mailbox,
        )
        .expect("the params spawn");
    match spawned {
        SpawnedCellKind::Active { sender, .. } => (sender, orx, td),
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    }
}

fn msg(body: Value) -> Message {
    MessageBuilder::new(Path::new("/sink"))
        .body(Body::Inline(body))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A sink that reads its document and answers with one empty message: the
/// cheapest script that still produces a receipt per message.
fn sink_params(mode: &str, sandbox: Option<Value>) -> Value {
    let mut p = json!({
        "runner": "python3",
        "script_inline": "import json, sys\njson.load(sys.stdin)\nsys.stdout.write('{\"messages\": []}')\n",
        "external_timeout_ms": 10000,
        "runner_mode": mode,
    });
    if let Some(s) = sandbox {
        p["sandbox"] = s;
    }
    p
}

/// Wait until `n` emissions arrived, and return them.
async fn collect(orx: &mut mpsc::Receiver<CellEmission>, n: usize) -> Vec<CellEmission> {
    let mut out = Vec::with_capacity(n);
    let deadline = Instant::now() + MARKER;
    while out.len() < n {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, orx.recv()).await {
            Ok(Some(em)) => out.push(em),
            Ok(None) => panic!("the output channel closed after {} of {n}", out.len()),
            Err(_) => panic!("only {} of {n} answers within {MARKER:?}", out.len()),
        }
    }
    out
}

// ── 1. the declarations ──────────────────────────────────────────────────

#[test]
fn the_three_hot_cells_ship_warm_and_are_safe_to() {
    for rel in WARM_CELLS {
        let cfg = shipped(rel);
        let params = &cfg["params"];
        assert_eq!(
            params["runner_mode"],
            json!("warm"),
            "{rel}: a high-frequency sink or gate runs warm (GH #852)"
        );
        let parsed = CodeParams::parse(params).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(parsed.runner_mode, RunnerMode::Warm, "{rel}");
        let script = params["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{rel}: an inline script"));
        // The warm harness gives the body an `io.StringIO` as stdin and a fresh
        // namespace per message. A script reading bytes off the real fd, or
        // keeping module-level state on purpose, would notice both.
        for forbidden in [
            "stdin.buffer",
            "fileno(",
            "os.read(0",
            "\nglobal ",
            " global ",
        ] {
            assert!(
                !script.contains(forbidden),
                "{rel}: `{forbidden}` does not survive the warm harness"
            );
        }
    }
}

#[test]
fn a_cell_that_says_nothing_still_runs_cold() {
    let parsed =
        CodeParams::parse(&json!({"runner": "python3", "script_inline": "pass"})).expect("parses");
    assert_eq!(
        parsed.runner_mode,
        RunnerMode::Cold,
        "cold stays the default"
    );
}

// ── 2. the reason ────────────────────────────────────────────────────────

/// 5 s at 200 messages a second into a mailbox of 64, `try_send` like a burst
/// that does not wait: not one `Full`, and every message answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_sink_keeps_up_with_a_flood() {
    // A kernel without Landlock cannot apply the restricted profile, and a
    // restricted cell never falls back to unsandboxed; skip visibly rather
    // than go red (the convention of `sandbox_isolation.rs`).
    if meclaw_cells::sandbox::landlock_abi().is_none() {
        eprintln!("SKIP: no Landlock on this kernel");
        return;
    }
    const RATE_HZ: u64 = 200;
    const SECONDS: u64 = 5;
    const TOTAL: usize = (RATE_HZ * SECONDS) as usize;
    let (tx, mut orx, _td) = spawn(sink_params("warm", Some(default_deny())), 64, false);

    let counter = tokio::spawn(async move { collect(&mut orx, TOTAL).await.len() });
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / RATE_HZ));
    let mut full = 0usize;
    for n in 0..TOTAL {
        tick.tick().await;
        match tx.try_send(msg(json!({"messages": [], "n": n}))) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => full += 1,
            Err(mpsc::error::TrySendError::Closed(_)) => panic!("the sink's mailbox closed"),
        }
    }
    assert_eq!(full, 0, "a warm sink never lets its mailbox of 64 run full");
    let answered = counter.await.expect("the counter task");
    assert_eq!(
        answered, TOTAL,
        "every message is answered, at the receiver"
    );
}

/// Time for `n` messages, sent at once, to come back answered.
async fn round(mode: &str, n: usize) -> Duration {
    let (tx, mut orx, _td) = spawn(sink_params(mode, None), 2 * n, false);
    // Warm children start on the first job that needs one; the pool is warmed
    // first so the round measures the runner, not the first spawn of each child.
    for _ in 0..8 {
        tx.send(msg(json!({"messages": []}))).await.expect("send");
    }
    collect(&mut orx, 8).await;
    let started = Instant::now();
    for _ in 0..n {
        tx.send(msg(json!({"messages": []}))).await.expect("send");
    }
    collect(&mut orx, n).await;
    started.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_messages_cost_warm_at_most_a_fifth_of_cold() {
    let cold = round("cold", 100).await;
    let warm = round("warm", 100).await;
    eprintln!("100 messages: cold {cold:?}, warm {warm:?}");
    assert!(
        warm * 5 <= cold,
        "warm must be at least five times cheaper than cold: cold {cold:?}, warm {warm:?}"
    );
}

// ── 3. the move changes nothing but cost ─────────────────────────────────

/// One proposal as the gate receives it: a tool call from the member's side,
/// with the actor on the edge.
fn proposal(args: Value) -> Message {
    let mut ctx = Map::new();
    ctx.insert("actor".into(), json!("member:alex"));
    MessageBuilder::new(Path::new("/sink"))
        .context(ctx)
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string(),
        }]})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A store answer as it comes back to the gate: `hop.operation`, and an error
/// code when the write failed.
fn store_answer(error_code: Option<&str>) -> Message {
    let mut hop = Map::new();
    hop.insert("operation".into(), json!("insert"));
    if let Some(code) = error_code {
        hop.insert("error_code".into(), json!(code));
    }
    MessageBuilder::new(Path::new("/sink"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A stream that crosses every branch shape of the gate — accepted writes, a
/// refusal, a silent echo that leaves through `sys.exit(0)`, a store error,
/// unparsable arguments — and how many emissions it produces in total.
fn gate_stream() -> (Vec<Message>, usize) {
    let aieos = json!({
        "standard": {"protocol": "AIEOS", "version": "1.1.0"},
        "metadata": {"instance_id": "i-1"},
        "identity": {"names": {"first": "Ada"}}
    });
    let stream = vec![
        // 2 store ops + audit + ack
        proposal(
            json!({"op": "upsert_entity", "kind": "person", "display_name": "Ada", "aieos": aieos}),
        ),
        // audit + ack (refused)
        proposal(
            json!({"op": "upsert_entity", "kind": "person", "display_name": "Ada", "aieos": {"bogus": 1}}),
        ),
        // nothing: the echo of a write that worked
        store_answer(None),
        // one error
        store_answer(Some("query_failed")),
        // 1 store op + audit + ack
        proposal(
            json!({"op": "set_trust", "entity_id": "entity:1", "audience": "*", "level": "known"}),
        ),
        // audit + ack (refused)
        {
            let mut ctx = Map::new();
            ctx.insert("actor".into(), json!("member:alex"));
            MessageBuilder::new(Path::new("/sink"))
                .context(ctx)
                .body(Body::Inline(json!({"messages": [{
                    "origin": "assistant", "type": "tool_call", "id": "c2", "text": "not json"
                }]})))
                .reply_to(Path::new("/drain"))
                .build()
        },
        // audit + ack (refused: no subscriber on the edge)
        proposal(json!({"op": "subscribe", "subject": "entity:1"})),
        // 1 store op + audit + ack
        proposal(
            json!({"op": "propose", "source_ref": "turn:1", "entity_ref": "entity:1",
                        "field_path": "aieos.identity", "audience": "agent:x"}),
        ),
    ];
    (stream, 4 + 2 + 1 + 3 + 2 + 2 + 3)
}

/// One emission with everything random (ids, timestamps) taken out: the lane,
/// the verdict, and per turn what it asks the store to do or what it answers.
fn shape(em: &CellEmission) -> String {
    let h = &em.content["header"];
    let mut parts = vec![format!(
        "{}|{}|{}",
        h["route"].as_str().unwrap_or(""),
        h["outcome"].as_str().unwrap_or(""),
        h["reason_code"].as_str().unwrap_or("")
    )];
    for m in em.content["messages"].as_array().into_iter().flatten() {
        let text: Value = m["text"]
            .as_str()
            .and_then(|t| meclaw_core::serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        parts.push(format!(
            "{}:{}:{}:{}:{}",
            m["type"].as_str().unwrap_or(""),
            text["operation"].as_str().unwrap_or(""),
            text["table"].as_str().unwrap_or(""),
            text["outcome"].as_str().unwrap_or(""),
            text["reason_code"].as_str().unwrap_or("")
        ));
    }
    parts.join(" ")
}

async fn gate_answers(mode: &str) -> Vec<String> {
    let mut params = shipped("templates/affinity/gate/config.json")["params"].clone();
    params["runner_mode"] = json!(mode);
    params["external_timeout_ms"] = json!(30000);
    let (tx, mut orx, _td) = spawn(params, 64, true);
    let (stream, expected) = gate_stream();
    for m in stream {
        tx.send(m).await.expect("send");
    }
    let got = collect(&mut orx, expected).await;
    // Nothing beyond the count: a silence with its control (the count above).
    let extra = tokio::time::timeout(Duration::from_millis(300), orx.recv()).await;
    assert!(extra.is_err(), "{mode}: one emission too many: {extra:?}");
    for em in &got {
        assert!(
            em.content["header"]["exit_code"] == json!(0),
            "{mode}: the gate never fails a message: {}",
            em.content
        );
    }
    let mut shapes: Vec<String> = got.iter().map(shape).collect();
    shapes.sort();
    shapes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_gate_answers_the_same_warm_as_cold() {
    let cold = gate_answers("cold").await;
    let warm = gate_answers("warm").await;
    assert_eq!(warm, cold, "warm changes latency, never an answer");
}
