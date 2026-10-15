//! Test-harness poll helpers for phase-5 quiescence.
//!
//! Three helpers: message_log COUNT polling (cascade barrier), cell.db value
//! polling (restore barrier), Arc<AtomicU32> polling (restart counter).
//!
//! All of them: 10 ms interval, fail-timeout with a count dump in the panic.
//! Their deadlines are `tokio::time::Instant`: virtual under a paused clock
//! like the poll sleep, so no helper spins on a real clock while the virtual
//! one races ahead.

//! New waits use [`until`] / [`until_some`]: the deadline is the hang mark
//! [`crate::deadline::HANG`] on `tokio::time::Instant` (virtual under a paused
//! clock) and the failure reads `HANG: <what> (waited <n> s)`.

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Waits until `pred` holds; polls every 10 ms under the [`crate::deadline::HANG`]
/// mark and panics with `HANG: <desc> (waited <n> s)` if it never does.
///
/// Topology tests only: under a paused clock every poll sleep moves the clock,
/// so time-logic tests wait on events, never on a polled condition (TV-Z7).
pub async fn until<F, Fut>(desc: &str, mut pred: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    until_some(desc, || {
        let fut = pred();
        async move { fut.await.then_some(()) }
    })
    .await
}

/// Like [`until`]; on a hang the panic also carries the last observed state
/// (`HANG: <desc> (waited <n> s); last state: <state:?>`, TV-Z3).
///
/// Nesting [`until`] in [`crate::deadline::hang_with`] does not do this: both
/// carry the same mark and the inner poll check wins (the outer timer rounds
/// up to the next millisecond) -- measured in tv-br M2, the state was lost.
pub async fn until_with<F, Fut, S, L>(desc: &str, mut pred: F, last_state: L)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
    S: std::fmt::Debug,
    L: FnOnce() -> S,
{
    poll(
        desc,
        || {
            let fut = pred();
            async move { fut.await.then_some(()) }
        },
        Some(last_state),
    )
    .await
}

/// [`until_with`] under the [`crate::deadline::BOOT_HANG`] mark: a boot wait.
pub async fn boot_until_with<F, Fut, S, L>(desc: &str, mut pred: F, last_state: L)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
    S: std::fmt::Debug,
    L: FnOnce() -> S,
{
    poll_under(
        desc,
        crate::deadline::BOOT_HANG,
        || {
            let fut = pred();
            async move { fut.await.then_some(()) }
        },
        Some(last_state),
    )
    .await
}

/// [`until_with`] under the [`crate::deadline::REPLAY_HANG`] mark: work a
/// stop left open, replayed after a boot (GH #1216).
pub async fn replay_until_with<F, Fut, S, L>(desc: &str, mut pred: F, last_state: L)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
    S: std::fmt::Debug,
    L: FnOnce() -> S,
{
    poll_under(
        desc,
        crate::deadline::REPLAY_HANG,
        || {
            let fut = pred();
            async move { fut.await.then_some(()) }
        },
        Some(last_state),
    )
    .await
}

/// Waits until `f` yields `Some(value)` and returns the value; same poll and
/// hang mark as [`until`].
pub async fn until_some<T, F, Fut>(desc: &str, f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    poll(desc, f, None::<fn() -> ()>).await
}

async fn poll<T, F, Fut, S, L>(desc: &str, f: F, last_state: Option<L>) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
    S: std::fmt::Debug,
    L: FnOnce() -> S,
{
    poll_under(desc, crate::deadline::HANG, f, last_state).await
}

/// The one poll loop: `mark` (scaled) is its only deadline, so the last state
/// always reaches the panic -- no outer mark races an inner one.
async fn poll_under<T, F, Fut, S, L>(
    desc: &str,
    mark: std::time::Duration,
    mut f: F,
    last_state: Option<L>,
) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
    S: std::fmt::Debug,
    L: FnOnce() -> S,
{
    let limit = crate::deadline::scaled(mark);
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        if tokio::time::Instant::now() >= deadline {
            match last_state {
                Some(state) => panic!(
                    "HANG: {desc} (waited {} s); last state: {:?}",
                    limit.as_secs(),
                    state()
                ),
                None => panic!("HANG: {desc} (waited {} s)", limit.as_secs()),
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Polls cell.db system.value for `slot_path` until == `expected`, under the
/// [`crate::deadline::HANG`] mark. Read-only connection; opened and closed per
/// poll. A hang names the value read last (`<none>` before the first row).
///
/// tv-br M10q-a (re-check R K5): no deadline of the caller any more. The
/// callers passed literals (5 s, 10 s, 30 s) the time factor never reached,
/// so under a quota they cut a slow wait short with a timeout instead of a
/// scaled `HANG:`; the helper now polls under the one mark, scaled in
/// `poll_under` (OR-T12-13).
pub async fn wait_for_cell_db_value(cell_dir: &std::path::Path, slot_path: &str, expected: &str) {
    let db_path = cell_dir.join("cell.db");
    let desc = format!(
        "wait_for_cell_db_value: {slot_path} == {expected:?} in {}",
        cell_dir.display()
    );
    let (seen_tx, seen_rx) = tokio::sync::watch::channel(String::from("<none>"));
    let read = || {
        let hit = read_cell_db_value(&db_path, slot_path, expected, &seen_tx);
        async move { hit }
    };
    poll_under(
        &desc,
        crate::deadline::HANG,
        read,
        Some(move || seen_rx.borrow().clone()),
    )
    .await;
}

/// One poll of [`wait_for_cell_db_value`]: `Some(())` on a match, else the
/// value read (if any) goes to `seen`.
fn read_cell_db_value(
    db_path: &std::path::Path,
    slot_path: &str,
    expected: &str,
    seen: &tokio::sync::watch::Sender<String>,
) -> Option<()> {
    // Phase-13-K-2: cell.db may not exist yet — the stateful factory now
    // returns `Dormant` (mailbox pair only), and the DB open lives in the
    // WakeFn closure. First message → Wake-Pre-Send → DB-Open. Tolerate
    // pre-Wake polls by treating Open-failure as "not-yet-existing".
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let v: Result<String, _> = conn.query_row(
        "SELECT value FROM system WHERE slot_path = ?",
        [slot_path],
        |r| r.get(0),
    );
    drop(conn);
    match v {
        Ok(s) if s == expected => Some(()),
        Ok(s) => {
            seen.send_replace(s);
            None
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        // Phase-13-K-2: between `Connection::open` and `setup_cell_db`, the
        // file exists but tables don't yet. Poll until setup finishes.
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg))) if msg.contains("no such table") => {
            None
        }
        Err(e) => panic!("wait_for_cell_db_value: SQL error: {e:?}"),
    }
}

/// Polls an `Arc<AtomicU32>` counter until >= `expected`, under the
/// [`crate::deadline::HANG`] mark (tv-br M10q-a, re-check R K5: no deadline of
/// the caller, see [`wait_for_cell_db_value`]). NO DB operation; a direct
/// atomic read. A hang names the count read last.
pub async fn wait_for_spawn_count(
    counter: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    expected: u32,
) {
    let desc = format!("wait_for_spawn_count: the spawn count reaches {expected}");
    let read = || {
        let actual = counter.load(std::sync::atomic::Ordering::Relaxed);
        async move { (actual >= expected).then_some(()) }
    };
    poll_under(
        &desc,
        crate::deadline::HANG,
        read,
        Some(|| counter.load(std::sync::atomic::Ordering::Relaxed)),
    )
    .await;
}

/// Waits until the message log holds at least `expected` rows of `trace_id`.
///
/// GH #1208 (TV-A3): event-driven over the writer's commit counter (`log_seq`)
/// and read through the colony (`ReadMessages`) — no poll, no database file.
/// The hang mark is [`crate::deadline::HANG`].
pub async fn wait_for_message_log_count(
    colony: &crate::ColonyHandle,
    trace_id: &str,
    expected: usize,
) {
    let mut seq = colony.subscribe_log_seq();
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        trace_id: Some(trace_id.to_owned()),
        ..Default::default()
    };
    let desc = format!("the message log holds >= {expected} rows of trace {trace_id}");
    // TV-Z3: a hang names the row count read last (re-check tv-p1a M4).
    let (seen_tx, seen_rx) = tokio::sync::watch::channel(0_usize);
    let waited = async {
        loop {
            // Mark the counter seen BEFORE the read: a commit landing during
            // the read flips `changed()` and the loop reads again.
            seq.borrow_and_update();
            let n = colony.read_log(filter.clone()).await.len();
            if n >= expected {
                return;
            }
            seen_tx.send_replace(n);
            if seq.changed().await.is_err() {
                // The writer is gone: nothing more will be committed, so the
                // read after its last commit decides.
                let n = colony.read_log(filter.clone()).await.len();
                assert!(n >= expected, "the colony writer ended at {n} rows");
                return;
            }
        }
    };
    crate::deadline::hang_with(&desc, waited, move || *seen_rx.borrow()).await;
}

/// Phase-6 T23: waits until `mutation_log.status` of `mutation_id` reads
/// `expected_status`.
///
/// GH #1208 (TV-A3): read through the colony (`ReadMutationsAudit`), never the
/// database file; polls under the [`crate::deadline::HANG`] mark (topology only).
/// A hang names the status read last, `None` while the row is absent (TV-Z3).
pub async fn wait_for_mutation_status(
    colony: &crate::ColonyHandle,
    mutation_id: &str,
    expected_status: &str,
) {
    let desc = format!("mutation {mutation_id} reaches status {expected_status}");
    let (seen_tx, seen_rx) = tokio::sync::watch::channel(None::<String>);
    let waited = async {
        loop {
            let (ack, rx) = tokio::sync::oneshot::channel();
            let msg = meclaw_colony::ColonyMsg::ReadMutationsAudit {
                since: None,
                limit: crate::AUDIT_PAGE,
                ack,
            };
            let entries = colony
                .ask("the colony answers a mutations-audit read", msg, rx)
                .await
                .entries;
            // K8 (tv-bn M7b17): one read serves AUDIT_PAGE rows, oldest first, no cursor;
            // a miss in a full read is a window, not an absence.
            assert!(
                entries.len() < crate::AUDIT_PAGE || entries.iter().any(|m| m.id == mutation_id),
                "{desc}: a full audit read of {} rows without it is a window",
                crate::AUDIT_PAGE
            );
            let status = entries
                .into_iter()
                .find(|m| m.id == mutation_id)
                .map(|m| m.status);
            if status.as_deref() == Some(expected_status) {
                return;
            }
            seen_tx.send_replace(status);
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    crate::deadline::hang_with(&desc, waited, move || seen_rx.borrow().clone()).await;
}

/// Like [`until_with`] for a value: waits until `f` yields `Some(value)` and returns it; on a
/// hang the panic carries the last observed state as
/// `HANG: <desc> (waited <n> s); last state: <state>`.
///
/// `last_state` is async and runs once, after the mark and never while the wait still polls,
/// so it may read what a poll would not (a fixture's dead letters, a hop chain). Re-check
/// tv-bn M5 M1: the display fixture's waits kept a deadline of their own to be able to say
/// this. Built on [`within_hang`], so on the one poll loop of this module.
pub async fn until_some_with<T, F, Fut, L, LFut>(desc: &str, f: F, last_state: L) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
    L: FnOnce() -> LFut,
    LFut: std::future::Future<Output = String>,
{
    match within_hang(f).await {
        Some(v) => v,
        None => {
            let state = last_state().await;
            panic!(
                "HANG: {desc} (waited {} s); last state: {state}",
                crate::deadline::scaled(crate::deadline::HANG).as_secs()
            )
        }
    }
}

/// Polls `f` like [`until_some`] and returns `None` at the [`crate::deadline::HANG`] mark
/// instead of panicking, for a caller that names what is left itself (a process tree that
/// outlived its SIGKILL, `meclaw-cells` `tests/support/browser_run.rs`).
///
/// The one poll loop is `poll_under`: the mark is read here, before the loop starts, and
/// the poll hands `Some(None)` back once it passed. The loop's own mark is a backstop one
/// whole mark later -- two equal marks race and the loop's check would win (tv-br M2), so
/// it only fires for a thread that stood still for 30 s between a poll and its check.
pub async fn within_hang<T, F, Fut>(mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + crate::deadline::scaled(crate::deadline::HANG);
    poll_under(
        "the hang mark of a wait that answers None",
        crate::deadline::HANG * 2,
        move || {
            let fut = f();
            async move {
                match fut.await {
                    Some(v) => Some(Some(v)),
                    None if tokio::time::Instant::now() >= deadline => Some(None),
                    None => None,
                }
            }
        },
        None::<fn() -> ()>,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deadline::{BOOT_HANG, HANG, REPLAY_HANG, scaled};

    /// The panic text of a wait that hangs. The pins compare it against the
    /// mark scaled by this process's factor, so they hold in a run that sets
    /// `MECLAW_TEST_TIME_FACTOR` too (OR-T12-13, tv-br M10m): a fixed
    /// "(waited 30 s)" was a factor-1 assertion.
    async fn hang_text<F>(wait: F) -> String
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let err = tokio::spawn(wait).await.err().expect("the wait must hang");
        let payload = err.into_panic();
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .expect("a hang panics with a text")
    }

    #[tokio::test(start_paused = true)]
    async fn until_reports_hang_with_its_description() {
        let text = hang_text(until("the flag flips", || async { false })).await;
        let want = format!("HANG: the flag flips (waited {} s)", scaled(HANG).as_secs());
        assert_eq!(text, want);
    }

    /// GH #1209 (H8): the state reaches the panic -- an inner mark of the
    /// poll that fired first would drop it.
    #[tokio::test(start_paused = true)]
    async fn until_with_hangs_with_the_last_state() {
        let text = hang_text(until_with(
            "the bearer arrives",
            || async { false },
            || "daemon gone",
        ))
        .await;
        let want = format!(
            "HANG: the bearer arrives (waited {} s); last state: \"daemon gone\"",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(start_paused = true)]
    async fn boot_until_with_waits_the_boot_mark() {
        let text = hang_text(boot_until_with(
            "the listener accepts",
            || async { false },
            || "booting",
        ))
        .await;
        let want = format!(
            "HANG: the listener accepts (waited {} s); last state: \"booting\"",
            scaled(BOOT_HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(start_paused = true)]
    async fn replay_until_with_waits_the_replay_mark() {
        let text = hang_text(replay_until_with(
            "the turn is replayed",
            || async { false },
            || "replaying",
        ))
        .await;
        let want = format!(
            "HANG: the turn is replayed (waited {} s); last state: \"replaying\"",
            scaled(REPLAY_HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(start_paused = true)]
    async fn until_with_returns_once_the_predicate_holds() {
        let mut polls = 0_u32;
        until_with(
            "the second poll",
            || {
                polls += 1;
                let done = polls == 2;
                async move { done }
            },
            || "never read",
        )
        .await;
        assert_eq!(polls, 2, "no poll after the predicate held");
    }

    #[tokio::test(start_paused = true)]
    async fn until_with_carries_the_last_observed_state() {
        let text = hang_text(until_with(
            "three letters",
            || async { false },
            || vec![1, 2],
        ))
        .await;
        let want = format!(
            "HANG: three letters (waited {} s); last state: [1, 2]",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(start_paused = true)]
    async fn until_returns_as_soon_as_the_predicate_holds() {
        let mut polls = 0_u32;
        let t0 = tokio::time::Instant::now();
        until("the third poll", || {
            polls += 1;
            let done = polls == 3;
            async move { done }
        })
        .await;
        assert_eq!(polls, 3, "no poll after the predicate held");
        assert_eq!(t0.elapsed(), 2 * POLL_INTERVAL, "two sleeps, then done");
        let v = until_some("a value", || async { Some(7) }).await;
        assert_eq!(v, 7);
    }

    /// The legacy helpers poll under the virtual HANG mark under a paused
    /// clock: the value only lands after twice the scaled mark, so the mark
    /// fires first and names the value read last (tv-br M10q-a, re-check R K5:
    /// formerly a 10 s caller deadline, never scaled).
    #[tokio::test(start_paused = true)]
    async fn the_wait_helpers_run_under_a_paused_clock() {
        let td = tempfile::TempDir::new().unwrap();
        let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE system (slot_path TEXT PRIMARY KEY, value TEXT);
             INSERT INTO system VALUES ('n', 'early');",
        )
        .unwrap();
        let dir = td.path().to_path_buf();
        let late = tokio::spawn(async move {
            tokio::time::sleep(scaled(HANG) * 2).await;
            conn.execute("UPDATE system SET value = 'late' WHERE slot_path = 'n'", [])
                .unwrap();
        });
        let shown = dir.display().to_string();
        let text = hang_text(async move { wait_for_cell_db_value(&dir, "n", "late").await }).await;
        let want = format!(
            "HANG: wait_for_cell_db_value: n == \"late\" in {shown} (waited {} s); last state: \"early\"",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
        late.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_message_log_count_returns_when_expected_reached() {
        let h = crate::ColonyHandle::new();
        h.spawn(meclaw_core::Path::new("/a"), || {
            crate::mocks::EchoMockCell::new(meclaw_core::Path::new("/a"))
        })
        .await;
        let first = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/a")).build();
        let trace = first.trace_id;
        h.send(first).await;
        for _ in 0..2 {
            h.send(
                meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/a"))
                    .trace_id(trace)
                    .build(),
            )
            .await;
        }
        wait_for_message_log_count(&h, &trace.to_string(), 3).await;
        h.shutdown().await;
    }

    /// Paused clock: the hang mark is virtual, so the colony's idle loop lets
    /// it run out at once (TV-Z7: the writer is an async front, ADR-0051).
    #[tokio::test(start_paused = true)]
    async fn wait_for_message_log_count_panics_on_timeout() {
        let h = crate::ColonyHandle::new();
        let text = hang_text(async move { wait_for_message_log_count(&h, "T1", 1).await }).await;
        let want = format!(
            "HANG: the message log holds >= 1 rows of trace T1 (waited {} s); last state: 0",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_cell_db_value_returns_when_expected_value_matches() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES ('counter', '42', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        wait_for_cell_db_value(&cell_dir, "counter", "42").await;
    }

    /// Paused clock: the HANG mark is virtual (tv-br M10q-a, re-check R K5:
    /// formerly a 100 ms caller deadline in real time).
    #[tokio::test(start_paused = true)]
    async fn wait_for_cell_db_value_panics_on_timeout() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        drop(conn);
        let shown = cell_dir.display().to_string();
        let text =
            hang_text(async move { wait_for_cell_db_value(&cell_dir, "counter", "99").await })
                .await;
        let want = format!(
            "HANG: wait_for_cell_db_value: counter == \"99\" in {shown} (waited {} s); last state: \"<none>\"",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_spawn_count_returns_when_expected_reached() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let counter = Arc::new(AtomicU32::new(3));
        wait_for_spawn_count(&counter, 3).await;
    }

    /// Paused clock: the HANG mark is virtual (tv-br M10q-a, re-check R K5:
    /// formerly a 100 ms caller deadline in real time).
    #[tokio::test(start_paused = true)]
    async fn wait_for_spawn_count_panics_on_timeout() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let counter = Arc::new(AtomicU32::new(2));
        let text = hang_text(async move { wait_for_spawn_count(&counter, 5).await }).await;
        let want = format!(
            "HANG: wait_for_spawn_count: the spawn count reaches 5 (waited {} s); last state: 2",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    /// The row is the colony's own: a mutation it refuses at the validate stage
    /// books a `rejected` row, read back through the colony (TV-A3: no database
    /// file in test code).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_mutation_status_returns_when_status_matches() {
        let h = crate::ColonyHandle::new();
        let (ack, rx) = tokio::sync::oneshot::channel();
        let msg = meclaw_colony::ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "x", "template": "doesnotexist"}]}
            }),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack,
        };
        let id = match h.ask("the colony answers a mutation", msg, rx).await {
            meclaw_colony::mutation::MutationOutcome::Rejected { id: Some(id), .. } => id,
            other => panic!("a mutation of a missing template is refused with an id: {other:?}"),
        };
        wait_for_mutation_status(&h, &id, "rejected").await;
        h.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_mutation_status_panics_on_timeout() {
        let h = crate::ColonyHandle::new();
        let text =
            hang_text(async move { wait_for_mutation_status(&h, "absent", "committed").await })
                .await;
        let want = format!(
            "HANG: mutation absent reaches status committed (waited {} s); last state: None",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    /// Re-check tv-bn M5 M1: a fixture wait that knows what it was looking at says so
    /// on a hang, read once after the mark, never while it still polls.
    #[tokio::test(start_paused = true)]
    async fn until_some_with_names_the_last_state_on_a_hang() {
        let text = hang_text(async {
            until_some_with(
                "the window stands",
                || async { None::<()> },
                || async { "ids [a, b]".to_string() },
            )
            .await
        })
        .await;
        let want = format!(
            "HANG: the window stands (waited {} s); last state: ids [a, b]",
            scaled(HANG).as_secs()
        );
        assert_eq!(text, want);
    }

    #[tokio::test(start_paused = true)]
    async fn until_some_with_returns_the_value_and_never_reads_the_state() {
        let v = until_some_with(
            "a value",
            || async { Some(7) },
            || async { panic!("the state of a wait that held is never read") },
        )
        .await;
        assert_eq!(v, 7);
    }

    /// Re-check tv-bn M7 (merge condition wait.rs): `within_hang` gives up at the hang mark
    /// itself, not at the one-mark-later backstop of the poll loop beneath it.
    #[tokio::test(start_paused = true)]
    async fn within_hang_gives_up_at_the_hang_mark() {
        let t0 = tokio::time::Instant::now();
        let got = within_hang(|| async { None::<()> }).await;
        assert!(got.is_none(), "nothing came, so nothing is handed back");
        assert_eq!(
            t0.elapsed(),
            crate::deadline::scaled(crate::deadline::HANG),
            "exactly the mark, virtual"
        );
    }
}
