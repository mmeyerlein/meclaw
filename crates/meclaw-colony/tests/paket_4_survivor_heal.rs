//! Paket-4 P4-B1: the `term_timeout`-survivor self-heals via the Paket-3 P6
//! `cell.message_timeout` backstop → a RETRY disconnect commits.
//!
//! This is the P4 (survivor) closure for a GENUINELY WEDGED stateful cell — the
//! make-or-break counterpart to the A1/A2 backpressure pin-demos
//! (`paket_4_backpressure_disconnect.rs`). There a backpressured cell unblocked
//! NATURALLY once the colony drained; here the cell hangs FOREVER (`hang_forever`)
//! and does NOT naturally unblock. The heal MUST come from the substrate
//! B-backstop, not from a drain.
//!
//! ## What a `term_timeout`-survivor is
//! A cell left `Awake` with `stop_tx == None` after a disconnect whose
//! death-ack-wait timed out: the disconnect `take()`s the cell's `stop_tx` and
//! fires the peace-stop, but the wedged `handle()` never returns → `death_ack`
//! never fires → after the tight `term_timeout` the mutation is
//! `Rejected{term_timeout}` (edges rolled back, cell stays `active`/`Awake`), and
//! the consumed `stop_tx` is gone. A subsequent disconnect would now hit the
//! interim `stop_wiring_unavailable` guard — UNLESS the cell self-heals.
//!
//! ## The self-heal chain (stateful, has a backstop)
//! 1. Bootstrap a `hang` cell (`HangMockCell`, `hang_forever`) with a small
//!    `cell.message_timeout` (800 ms). `set_term_timeout_ms_for_test` is set
//!    TIGHTER (400 ms) and SMALLER than the backstop, holding
//!    `TERM_TIMEOUT_TEST_LOCK`. Ordering point: term_timeout fires FIRST (→
//!    survivor), THEN the backstop fires (→ heal).
//! 2. Probe → the cell wakes (`spawn_count` 0→1) and enters `handle()`, which
//!    hangs forever. The backstop clock is anchored at this `handle()` entry.
//! 3. Disconnect (remove the last edge) → the colony fires the peace-stop +
//!    enters death-ack-await; the wedged cell never acks → after 400 ms →
//!    `Rejected{term_timeout}`. The cell is now the survivor (`Awake`, `active`,
//!    `stop_tx` consumed).
//! 4. The still-wedged `handle()` crosses 800 ms → the `cell.message_timeout`
//!    backstop fires → `CellDied{Backstop}` → `handle_cell_died` RESTARTS (Paket-3
//!    P3-B-restart) → the factory's `renotify_stop_wiring` restores a fresh
//!    `(stop_tx, death_ack_rx)` pair. Positive receipt: `spawn_count` 1→2.
//! 5. RETRY disconnect → asserts `MutationOutcome::Committed` (the restored
//!    `stop_tx` lets the cell be peace-stopped — NO `stop_wiring_unavailable`
//!    reject), the cell ends inactive, and the edge is gone in colony.db.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::{EchoCellFactory, HangCellFactory};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Serialize the term-timeout-sensitive test(s) in this binary — they drive the
/// process-global `set_term_timeout_ms_for_test` static. Held across `.await`, so
/// `tokio::sync::Mutex` (yields, never blocks a worker thread).
static TERM_TIMEOUT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Tight death-ack term-timeout (semantic timing discriminator — kept short +
/// justified per CLAUDE.md). It is SMALLER than `MESSAGE_TIMEOUT_MS` so the
/// survivor is created BEFORE the backstop heals it.
const TERM_TIMEOUT_MS: u64 = 400;

/// The `cell.message_timeout` B-backstop budget. LARGER than `TERM_TIMEOUT_MS`
/// so the heal (backstop → restart) happens AFTER the survivor exists.
const MESSAGE_TIMEOUT_MS: u64 = 800;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Read the RAM registry entry DTO for `path` (or `None` if absent).
async fn ram_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Count edge rows in colony.db with the given from/to (fresh read-only conn).
fn db_edge_count(db_dir: &std::path::Path, from: &str, to: &str) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT count(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
        [from, to],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

/// Poll `spawn_count` until it reaches `target` (or time out). Generous 30 s
/// failure marker (robust against cargo-parallel load). Returns the last value.
async fn wait_for_spawn_count(counter: &Arc<AtomicU32>, target: u32) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let n = counter.load(Ordering::Relaxed);
        if n >= target || std::time::Instant::now() >= deadline {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// P4-B1: a wedged STATEFUL `term_timeout`-survivor self-heals via the Paket-3
/// P6 `cell.message_timeout` backstop, and a RETRY disconnect then COMMITS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stateful_survivor_heals_via_backstop_then_retry_disconnect_commits() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    // Tight + SMALLER than the backstop: the wedged cell's death-ack never fires,
    // so the FIRST disconnect rejects via `term_timeout` (→ survivor) at 400 ms,
    // BEFORE the 800 ms backstop fires (→ heal).
    set_term_timeout_ms_for_test(TERM_TIMEOUT_MS);

    let td = tempfile::TempDir::new().unwrap();
    let db_dir = td.path().to_path_buf();

    // Root hive (`main/` → `/`); `/anchor` provides the inbound edge that
    // activates `/hang`. `/hang` hangs forever with a small `message_timeout` so
    // the B-backstop fires shortly after the term_timeout survivor exists.
    //
    // `/keep -> /anchor` is a persistent keep-alive edge the test never removes,
    // so removing `/anchor -> /hang` deactivates ONLY `/hang` — never `/anchor`.
    // Without it, the first term_timeout disconnect would ALSO deactivate
    // `/anchor` (its only edge gone) + consume its stop_tx, leaving `/anchor` a
    // second survivor whose later disconnect would trip the guard (mirrors the
    // A1/A2 demos' anti-strand `/anchor -> /sink` rationale).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/keep/config.json",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/keep"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/hang/config.json",
        &format!(
            r#"{{"cell":{{"type":"hang","message_timeout":{MESSAGE_TIMEOUT_MS}}},"params":{{"hang_forever":true}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );

    let spawn_count = Arc::new(AtomicU32::new(0));
    let hang_factory: Arc<dyn CellFactory> = Arc::new(HangCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let echo_factory: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("hang".to_string(), hang_factory.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    let mut registry = CellFactoryRegistry::new();
    registry.insert("hang".to_string(), hang_factory);
    registry.insert("echo".to_string(), echo_factory);

    let report = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    assert_eq!(report.cell_count, 3, "keep + anchor + hang boot");

    // Activate /hang via anchor->hang + keep /anchor alive via keep->anchor
    // (bootstrap edge set was empty).
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"keep","to":"anchor"},
            {"from":"anchor","to":"hang"}
        ]}}),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges must commit, got {act:?}"
    );
    assert!(ram_entry(&h, "/hang").await.expect("/hang").active);
    assert_eq!(db_edge_count(&db_dir, "/anchor", "/hang"), 1);

    // Probe /hang → wakes it (Dormant → Awake, spawn_count 0→1) with the
    // message_timeout backstop wired. handle() now hangs forever; the backstop
    // clock is anchored at this entry.
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    assert!(
        wait_for_spawn_count(&spawn_count, 1).await >= 1,
        "hang cell must wake (spawn_count >= 1)"
    );
    assert_eq!(
        ram_entry(&h, "/hang")
            .await
            .expect("/hang")
            .lifecycle_status,
        "Awake",
        "/hang must be Awake after wake-pre-send"
    );

    // ── FIRST disconnect → term_timeout survivor. ──────────────────────────────
    // remove_edges deactivates /hang → fires the peace-stop → death-ack-await;
    // the wedged handle() never acks → after 400 ms → Rejected{term_timeout}.
    // The cell is now the survivor: Awake, active, stop_tx consumed (None).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"anchor","to":"hang"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "term_timeout",
                "wedged disconnect must reject with term_timeout (survivor created), got {error_code}"
            );
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }
    // Full rollback: edge restored, /hang stays active + Awake (the survivor).
    assert_eq!(
        db_edge_count(&db_dir, "/anchor", "/hang"),
        1,
        "edge must remain after term_timeout rollback"
    );
    let surv = ram_entry(&h, "/hang")
        .await
        .expect("/hang survivor must stay");
    assert!(surv.active, "/hang survivor must stay active (no zombie)");
    assert_eq!(surv.lifecycle_status, "Awake", "/hang survivor still Awake");

    // ── SELF-HEAL: the still-wedged handle() crosses message_timeout (800 ms) →
    // backstop fires → CellDied{Backstop} → handle_cell_died RESTARTS → the
    // factory's renotify_stop_wiring restores a fresh stop_tx. Positive receipt:
    // spawn_count 1 → 2 (the restart actually happened). ──────────────────────
    let after_restart = wait_for_spawn_count(&spawn_count, 2).await;
    assert!(
        after_restart >= 2,
        "B-backstop must RESTART the wedged survivor (spawn_count >= 2, was {after_restart}) — \
         this is the self-heal; without it the survivor stays un-stoppable"
    );

    // ── RETRY disconnect → MUST COMMIT (restored stop_tx → peace-stoppable, NO
    // stop_wiring_unavailable reject). ─────────────────────────────────────────
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"anchor","to":"hang"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!(
            "MAKE-OR-BREAK: retry disconnect must COMMIT after backstop self-heal \
             (no stop_wiring_unavailable), got {other:?}"
        ),
    }
    let e = ram_entry(&h, "/hang").await.expect("/hang entry must STAY");
    assert!(
        !e.active,
        "/hang must be inactive after the committed retry"
    );

    h.shutdown().await;
    assert_eq!(
        db_edge_count(&db_dir, "/anchor", "/hang"),
        0,
        "edge must be gone in colony.db after the committed retry disconnect"
    );
}
