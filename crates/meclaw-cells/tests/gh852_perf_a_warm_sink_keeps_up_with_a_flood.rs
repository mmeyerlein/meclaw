//! GH #852, the perf stage: the real-time claims of the flood test, measured
//! instead of judged (rule 11 -- a rate or a cost ratio is never a verdict in
//! a functional test). The events behind them -- every message of a flood
//! answered once, from the pool's children and from nothing else -- are
//! pinned in `misc_7/gh852_a_warm_sink_keeps_up_with_a_flood.rs`.
//!
//! Run by `scripts/test-tier.sh perf` only (`#[ignore]`, profile `perf`), so
//! this file stays its own binary (`scripts/harness_fold.py`: an ignore proof
//! is never folded into a harness).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::deadline;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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

/// A sink that reads its document and answers with one text naming the
/// interpreter that ran it and the message it ran: `<pid>:<n>`. The pid is the
/// event the flood is judged on -- a warm cell answers from its pool, a cold
/// cell starts one interpreter (and, behind the default-deny profile, one user
/// and network namespace) per message.
fn pid_sink_params(mode: &str, sandbox: Option<Value>) -> Value {
    let mut p = json!({
        "runner": "python3",
        "script_inline": "import json, os, sys\n\
            d = json.load(sys.stdin)\n\
            n = d['body'].get('n', -1)\n\
            sys.stdout.write(json.dumps({'messages': [{'origin': 'assistant', 'type': 'text', \
            'text': '%d:%d' % (os.getpid(), n)}]}))\n",
        "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
        "runner_mode": mode,
    });
    if let Some(s) = sandbox {
        p["sandbox"] = s;
    }
    p
}

/// Read the output channel until every message of `ids` is marked consumed
/// (GH #1015: a delivery's mark rides behind every emission of that
/// delivery), and return the answers in between. No count is guessed and no
/// window waited out: when the last mark arrived, every answer is in.
async fn drain(
    orx: &mut mpsc::Receiver<CellEmission>,
    ids: &[Uuid],
    what: &str,
) -> Vec<CellEmission> {
    let mut open: HashSet<Uuid> = ids.iter().copied().collect();
    let mut answers = Vec::new();
    deadline::hang(what, async {
        while !open.is_empty() {
            let em = orx
                .recv()
                .await
                .unwrap_or_else(|| panic!("{what}: the output channel closed"));
            match em.consumed_mark_id() {
                Some(id) => {
                    open.remove(&id);
                }
                None => answers.push(em),
            }
        }
    })
    .await;
    answers
}

/// Send `n` messages (numbered) into `tx` with backpressure -- a full mailbox
/// makes the sender wait, it never drops -- while a second task drains the
/// answers; return those answers.
async fn flood(
    tx: mpsc::Sender<Message>,
    mut orx: mpsc::Receiver<CellEmission>,
    n: usize,
    what: &'static str,
) -> Vec<CellEmission> {
    let msgs: Vec<Message> = (0..n)
        .map(|i| msg(json!({"messages": [], "n": i})))
        .collect();
    let ids: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    let reader = tokio::spawn(async move { drain(&mut orx, &ids, what).await });
    for m in msgs {
        tx.send(m).await.expect("the sink's mailbox stays open");
    }
    deadline::hang("the reader of the flood", reader)
        .await
        .expect("the reader task")
}

/// The perf stage of GH #852 (`scripts/test-tier.sh perf`, never a gate):
/// the old real-time claims, measured instead of judged -- a 5 s flood of 200
/// messages a second, `try_send` into a mailbox of 64 behind the default-deny
/// profile, counted for `Full`; and the time 100 messages take `cold` against
/// `warm`. Written as JSON to `target/perf/`.
#[ignore = "perf stage"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_gh852_a_warm_sink_keeps_up_with_a_flood() {
    if meclaw_cells::sandbox::landlock_abi().is_none() {
        eprintln!("SKIP: no Landlock on this kernel");
        return;
    }
    const RATE_HZ: u64 = 200;
    const SECONDS: u64 = 5;
    const TOTAL: usize = (RATE_HZ * SECONDS) as usize;
    let (tx, mut orx, _td) = spawn(pid_sink_params("warm", Some(default_deny())), 64, false);
    let msgs: Vec<Message> = (0..TOTAL)
        .map(|i| msg(json!({"messages": [], "n": i})))
        .collect();
    let mut sent = Vec::with_capacity(TOTAL);
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / RATE_HZ));
    let mut full = 0usize;
    for m in msgs {
        tick.tick().await;
        let id = m.id;
        match tx.try_send(m) {
            Ok(()) => sent.push(id),
            Err(mpsc::error::TrySendError::Full(_)) => full += 1,
            Err(mpsc::error::TrySendError::Closed(_)) => panic!("the sink's mailbox closed"),
        }
    }
    let answered = drain(&mut orx, &sent, "the perf flood is marked consumed")
        .await
        .len();

    let mut round_ms = Map::new();
    for mode in ["cold", "warm"] {
        let (tx, mut orx, _td) = spawn(pid_sink_params(mode, None), 200, false);
        // The pool is warmed first so the round measures the runner, not the
        // first spawn of each child.
        let warm_up: Vec<Message> = (0..8).map(|_| msg(json!({"messages": []}))).collect();
        let warm_ids: Vec<Uuid> = warm_up.iter().map(|m| m.id).collect();
        for m in warm_up {
            tx.send(m).await.expect("send");
        }
        drain(&mut orx, &warm_ids, "the warm-up is marked consumed").await;
        let started = tokio::time::Instant::now();
        flood(tx, orx, 100, "the perf round is marked consumed").await;
        round_ms.insert(mode.into(), json!(started.elapsed().as_millis() as u64));
    }
    let report = json!({
        "test": "perf_gh852_a_warm_sink_keeps_up_with_a_flood",
        "flood": {"rate_hz": RATE_HZ, "seconds": SECONDS, "mailbox": 64,
                  "full": full, "answered": answered},
        "round_100_ms": round_ms,
    });
    println!("PERF {report}");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("target/tmp has a parent")
        .join("perf");
    std::fs::create_dir_all(&dir).expect("target/perf");
    std::fs::write(
        dir.join("perf_gh852_a_warm_sink_keeps_up_with_a_flood.json"),
        report.to_string(),
    )
    .expect("write the perf report");
}
