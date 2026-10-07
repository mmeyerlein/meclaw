//! GH #1021 — `affinity/brief` runs warm.
//!
//! One brief request costs six runs of the brief cell in series (the request,
//! then one run per store phase: who, trust, disclosure, entity, audit). Cold,
//! each of them paid an interpreter start plus the compile of a 1 100-line
//! inline script; the issue measured a comparable cell at p50 ≈ 200 ms cold
//! against ≈ 0.5 ms warm. The script is stateless — it imports only
//! `sys, json, uuid, hashlib, datetime`, opens no file, reads no environment,
//! keeps no cache — so `warm` (a fresh globals dict per message,
//! `crates/meclaw-cells/src/code/params.rs` `RunnerMode::Warm`) is safe, the
//! same move #852 made for `affinity/gate`.
//!
//! Two locks:
//! 1. **the declaration** — the shipped config says `runner_mode: "warm"`, it
//!    parses as `RunnerMode::Warm`, and the script carries none of the shapes
//!    that would notice the warm harness or keep state across messages;
//! 2. **no shared state** — one warm child (`max_concurrency: 1`) answers a
//!    stream of consecutive messages exactly as one cold process per message
//!    does, and a request repeated after other requests gets the same answer
//!    as the first time.

use meclaw_cells::code::{CodeCellFactory, CodeParams, RunnerMode};
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use meclaw_testing::EmissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const BRIEF: &str = "templates/affinity/brief/config.json";
const MARKER: Duration = Duration::from_secs(60);

fn shipped() -> Value {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(BRIEF),
    )
    .unwrap_or_else(|e| panic!("{BRIEF}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{BRIEF}: {e}"))
}

// ── 1. the declaration ───────────────────────────────────────────────────

#[test]
fn the_brief_ships_warm_and_is_safe_to() {
    let cfg = shipped();
    let params = &cfg["params"];
    assert_eq!(
        params["runner_mode"],
        json!("warm"),
        "{BRIEF}: six serial runs per brief request run warm (GH #1021)"
    );
    let parsed = CodeParams::parse(params).unwrap_or_else(|e| panic!("{BRIEF}: {e}"));
    assert_eq!(parsed.runner_mode, RunnerMode::Warm);
    let script = params["script_inline"]
        .as_str()
        .expect("affinity/brief ships an inline script");
    // The warm harness hands the body an `io.StringIO` as stdin and a fresh
    // namespace per message: a script reading bytes off the real fd, keeping
    // module-level state on purpose, holding a file or starting a thread would
    // notice the one or defeat the other.
    for forbidden in [
        "stdin.buffer",
        "fileno(",
        "os.read(0",
        "\nglobal ",
        " global ",
        "open(",
        "import os",
        "threading",
        "lru_cache",
        "os.environ",
    ] {
        assert!(
            !script.contains(forbidden),
            "{BRIEF}: `{forbidden}` does not survive the warm harness"
        );
    }
}

// ── 2. no shared state between consecutive messages ──────────────────────

type Cell = (
    mpsc::Sender<Message>,
    mpsc::Receiver<CellEmission>,
    tempfile::TempDir,
);

fn spawn(params: Value) -> Cell {
    let (otx, orx) = mpsc::channel(4096);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/brief"),
            params,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView {
                multi_send_capable: true,
                ..Default::default()
            },
            itx,
            None,
            0,
            None,
            None,
            64,
        )
        .expect("the params spawn");
    match spawned {
        SpawnedCellKind::Active { sender, .. } => (sender, orx, td),
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    }
}

/// A brief request as it arrives on `in_brief`: the asker on the edge, the
/// subject and slots in the tool call.
fn request(asker: &str, args: Value, id: &str) -> Message {
    let mut ctx = Map::new();
    ctx.insert("asker".into(), json!(asker));
    ctx.insert("channel_node".into(), json!("telegram"));
    MessageBuilder::new(Path::new("/brief"))
        .context(ctx)
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": id,
            "text": args.to_string(),
        }]})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A store answer that failed, and the echo of an audit write: the two
/// branches that leave through `sys.exit(0)` before the request lanes.
fn echo(phase: &str, error_code: Option<&str>) -> Message {
    let mut hop = Map::new();
    hop.insert("operation".into(), json!("query"));
    if let Some(code) = error_code {
        hop.insert("error_code".into(), json!(code));
    }
    let mut ctx = Map::new();
    ctx.insert("aff_phase".into(), json!(phase));
    MessageBuilder::new(Path::new("/brief"))
        .hop(hop)
        .context(ctx)
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// Consecutive messages that differ in every input the script reads: asker,
/// subject, slots, op, phase. The first request comes back as the last one.
/// The flag marks the one message that answers with silence: the audit echo
/// writes `[]` (its lane already answered).
fn stream() -> Vec<(Message, bool)> {
    let silent = |m: Message| (m, true);
    let speaks = |m: Message| (m, false);
    vec![
        speaks(request("peer:ada", json!({"subject": "entity:1"}), "c1")),
        speaks(request(
            "member:alex",
            json!({"subject": "entity:2", "slots": ["brain", "identity_short"]}),
            "c2",
        )),
        speaks(echo("who", Some("query_failed"))),
        speaks(request("agent:x", json!({"op": "list"}), "c3")),
        silent(echo("audit", None)),
        speaks(request("", json!({}), "c4")),
        speaks(request("peer:ada", json!({"subject": "entity:1"}), "c1")),
    ]
}

/// One emission with nothing random in it: the header, and per message its
/// type plus the store operation, table and filter it asks for.
fn shape(em: &CellEmission) -> String {
    let mut h = em.content["header"].clone();
    if let Some(o) = h.as_object_mut() {
        // Process facts of the run, not answers of the script.
        for k in ["duration_ms", "exit_code", "pid", "run_id", "started_at"] {
            o.remove(k);
        }
    }
    let mut parts = vec![h.to_string()];
    for m in em.content["messages"].as_array().into_iter().flatten() {
        let text: Value = m["text"]
            .as_str()
            .and_then(|t| meclaw_core::serde_json::from_str(t).ok())
            .unwrap_or_else(|| m["text"].clone());
        parts.push(format!(
            "{}:{}:{}:{}",
            m["type"].as_str().unwrap_or(""),
            text["operation"].as_str().unwrap_or(""),
            text["table"].as_str().unwrap_or(""),
            text["filter"]
        ));
    }
    parts.join(" | ")
}

/// Send every message, wait for its answer before the next, and return one
/// shape list per message — the emissions of one message, in arrival order.
async fn answers(mode: &str) -> Vec<Vec<String>> {
    let mut params = shipped()["params"].clone();
    params["runner_mode"] = json!(mode);
    params["max_concurrency"] = json!(1);
    params["external_timeout_ms"] = json!(30000);
    let (tx, mut orx, _td) = spawn(params);
    let mut out = Vec::new();
    for (m, silent) in stream() {
        tx.send(m).await.expect("send");
        let deadline = Instant::now() + MARKER;
        let first_wait = if silent {
            Duration::from_secs(2)
        } else {
            MARKER
        };
        let mut ems = Vec::new();
        if let Ok(first) = tokio::time::timeout(first_wait, orx.recv_answer()).await {
            ems.push(first.expect("the output channel stays open"));
            // Everything this message produced: a short silence closes it.
            while let Ok(Some(em)) = tokio::time::timeout(
                Duration::from_millis(300).min(deadline.saturating_duration_since(Instant::now())),
                orx.recv_answer(),
            )
            .await
            {
                ems.push(em);
            }
        }
        assert_eq!(
            ems.is_empty(),
            silent,
            "{mode}: only the audit echo is silent ({} emissions)",
            ems.len()
        );
        for em in &ems {
            assert_eq!(
                em.content["header"]["exit_code"],
                json!(0),
                "{mode}: the brief never fails a message: {}",
                em.content
            );
        }
        out.push(ems.iter().map(shape).collect());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_consecutive_messages_share_no_state_warm() {
    let cold = answers("cold").await;
    let warm = answers("warm").await;
    assert_eq!(cold.len(), warm.len());
    for (i, (c, w)) in cold.iter().zip(&warm).enumerate() {
        assert_eq!(w, c, "message {i}: warm changes latency, never an answer");
    }
    // The repeated request, after five other messages through the same child,
    // answers exactly as it did first.
    assert_eq!(
        warm.first(),
        warm.last(),
        "a request answers the same after other messages ran in the warm child"
    );
    // Control: the stream really differs message by message, so equality above
    // is not the equality of a cell that answers everything alike.
    assert_ne!(
        warm[0], warm[1],
        "two different requests, two different answers"
    );
}
