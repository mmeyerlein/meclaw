//! GH #1276 (R-WK-4): a warm or resident runner child stops after the cell's
//! idle timeout, and the next message boots a fresh one lazily.
//!
//! Before #1276 the factory threw the idle timeout away (`_idle_timeout`), so
//! every child a warm cell ever started lived until the colony restarted. The
//! test goes through the production spawn path (`CodeCellFactory::spawn_cell`)
//! and hands the idle duration in exactly where `resolve_idle_timeout` hands it
//! in production; that resolution is pinned in `meclaw-colony`.
//!
//! Time is SUT configuration here, never a verdict (rule 11): the idle value is
//! small so the test does not sit in its hang mark, and the claim holds for any
//! value. The stop itself is observed as an EVENT -- the first child's pid
//! disappears (`kill(pid, 0)`) -- under the hang mark of `wait::until`.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::EmissionsExt;
use std::sync::Arc;
use std::time::Duration;

/// The idle timeout the cell is configured with (SUT time, not a verdict).
const IDLE: Duration = Duration::from_millis(300);

/// Answers its own pid and echoes `n`, so a reply proves which process ran it.
const SCRIPT: &str = "import os,sys,json\n\
    d = json.load(sys.stdin)\n\
    sys.stdout.write(json.dumps({'messages': [], 'pid': os.getpid(), 'n': d['body']['n']}))\n";

/// Spawn the cell through the factory, send two messages with a full idle stop
/// in between, and check the second one ran in a NEW process.
async fn the_idle_child_is_replaced(mode: &str) {
    let raw = json!({
        "runner": "python3",
        "script_inline": SCRIPT,
        "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
        "runner_mode": mode,
        "max_concurrency": 1
    });
    let (otx, mut orx) = tokio::sync::mpsc::channel(8);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/code"),
            raw,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            Some(IDLE),
            0,
            None,
            None,
            1000,
        )
        .expect("the cell spawns");
    let sender = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };
    let mut ask = async |n: i64| -> Value {
        sender
            .send(
                MessageBuilder::new(Path::new("/code"))
                    .body(Body::Inline(json!({"messages": [], "n": n})))
                    .reply_to(Path::new("/sink"))
                    .build(),
            )
            .await
            .expect("the cell takes the message");
        let em = meclaw_testing::deadline::hang(
            &format!("{mode}: message {n} answers"),
            orx.recv_answer(),
        )
        .await
        .expect("the cell is alive");
        assert_eq!(
            em.content["header"]["exit_code"], 0,
            "{mode}: {}",
            em.content
        );
        em.content
    };
    let first = ask(1).await;
    let pid_a = first["pid"].as_i64().expect("a pid") as i32;
    meclaw_testing::wait::until(
        &format!("the idle {mode} child stops on its own"),
        || async move {
            // SAFETY: signal 0 only probes whether the pid still exists.
            unsafe { libc::kill(pid_a, 0) != 0 }
        },
    )
    .await;
    let second = ask(2).await;
    assert_eq!(second["n"], 2, "{mode}: the next message is answered");
    assert_ne!(
        second["pid"].as_i64(),
        Some(i64::from(pid_a)),
        "{mode}: the next message booted a new child"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1276_an_idle_warm_child_stops_and_the_next_message_boots_a_new_one() {
    the_idle_child_is_replaced("warm").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1276_an_idle_resident_child_stops_and_the_next_message_boots_a_new_one() {
    the_idle_child_is_replaced("resident").await;
}
