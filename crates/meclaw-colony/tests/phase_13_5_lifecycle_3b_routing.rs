//! Phase-13.5 Lifecycle-3b Task 5: the inactive-routing short-circuit (F3,
//! SCOPE 3).
//!
//! `route_with_log` short-circuits BEFORE the `route()` call: if the resolved
//! target exists in the registry AND is `active == false` (and is not a colony
//! endpoint and TTL > 0), the message is dead-lettered as `cell_inactive` and
//! routing stops — the inactive cell is NEVER woken and NEVER sent to.
//!
//! Trennschärfe (demo (c)): in ONE test we prove three DISTINCT DLQ reasons:
//!   - inactive cell        → `cell_inactive`
//!   - unknown path         → `unresolved_path`   (no cell, no hive)
//!   - hive, no out-edge    → `hive_no_route`     (reachable hive, graph silent)
//!
//! The inactive cell is produced via the realistic persisted-status path
//! (mirrors `phase_13_5_lifecycle_3b_demo::reboot_maps_persisted_status_into_active`):
//! Boot 1 registers `/a` as `status='active'`, we flip it to `'inactive'` in
//! colony.db, Boot 2 rehydrates `/a` with `active == false`. The positive proof
//! is the `cell_inactive` DLQ entry: the short-circuit fires before `route()`,
//! so the inactive target is dead-lettered rather than (wrongly) routed. Without
//! the short-circuit the echo cell `/a` re-routes to itself until TTL expiry, so
//! the inactive case is observably distinct from the correct behaviour.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// `(name, factory)` list with `echo` registered.
fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive `/` with one out-edge gated on `headers.kind == 'text'`, plus an
/// echo cell `/a`. The gated edge lets the same hive both route (kind=text) and
/// silently drop (kind=other → `hive_no_route`); the cell `/a` is the inactive
/// target.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./a","condition":"headers.kind == 'text'"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// One header entry shorthand.
fn headers_with(pairs: &[(&str, &str)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), json!(*v));
    }
    m
}

/// Build a probe addressed at `target` carrying the given headers and an
/// `origin`-format UBF body (Phase-6 lesson: never `role`/`content`).
fn probe(target: &str, headers: Map<String, Value>) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"origin":"user","type":"text","text":"ping"}),
        ))
        .hop(headers)
        .ttl(8)
        .build()
}

/// Poll-then-drain the DLQ until an entry with `resolved_target == target`
/// appears (bounded). Returns ALL drained entries (the snapshot at first hit, or
/// at the deadline), so co-resident DLQ entries can be asserted too.
async fn drain_until_target(h: &ColonyHandle, target: &str) -> Vec<meclaw_colony::DeadLetter> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = h.drain_dead_letters().await;
        let hit = snapshot
            .iter()
            .any(|d| d.resolved_target.as_str() == target);
        if hit || std::time::Instant::now() >= deadline {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Find the single DLQ entry whose `resolved_target` matches `target`.
fn dlq_entry_for<'a>(
    snapshot: &'a [meclaw_colony::DeadLetter],
    target: &str,
) -> &'a meclaw_colony::DeadLetter {
    let matches: Vec<_> = snapshot
        .iter()
        .filter(|d| d.resolved_target.as_str() == target)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one DLQ entry for resolved_target={target}; snapshot={snapshot:?}"
    );
    matches[0]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inactive_cell_dead_letters_distinct_from_unresolved_and_hive_no_route() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1: registers /a as status='active'. ────────────────────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    h.shutdown().await;

    // ── Flip /a to 'inactive' directly in colony.db between the two boots. ───
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        let n = conn
            .execute("UPDATE registry SET status='inactive' WHERE path='/a'", [])
            .unwrap();
        assert_eq!(n, 1, "exactly one /a row must be flipped to inactive");
    }

    // ── Boot 2 (same colony.db): /a rehydrates with active == false. ─────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");

    // (1) Probe at the INACTIVE cell /a → cell_inactive (route() short-circuit).
    h2.send(probe("/a", headers_with(&[("kind", "text")])))
        .await;
    let snap_a = drain_until_target(&h2, "/a").await;
    let dl_a = dlq_entry_for(&snap_a, "/a");
    assert_eq!(
        dl_a.reason.as_code(),
        "cell_inactive",
        "message to inactive cell /a → cell_inactive; got {:?}",
        dl_a.reason
    );

    // (2) Probe at an unknown path /nope (no cell, no hive) → unresolved_path.
    h2.send(probe("/nope", Map::new())).await;
    let snap_n = drain_until_target(&h2, "/nope").await;
    let dl_n = dlq_entry_for(&snap_n, "/nope");
    assert_eq!(
        dl_n.reason.as_code(),
        "unresolved_path",
        "non-existent path /nope → unresolved_path; got {:?}",
        dl_n.reason
    );

    // (3) Probe at the reachable hive / with kind=other → out-edge condition
    // false → no out-edge matches → hive_no_route.
    h2.send(probe("/", headers_with(&[("kind", "other")])))
        .await;
    let snap_h = drain_until_target(&h2, "/").await;
    let dl_h = dlq_entry_for(&snap_h, "/");
    assert_eq!(
        dl_h.reason.as_code(),
        "hive_no_route",
        "reachable hive / with no matching out-edge → hive_no_route; got {:?}",
        dl_h.reason
    );

    // Trennschärfe: the three reasons are pairwise distinct.
    assert_ne!(dl_a.reason.as_code(), dl_n.reason.as_code());
    assert_ne!(dl_a.reason.as_code(), dl_h.reason.as_code());
    assert_ne!(dl_n.reason.as_code(), dl_h.reason.as_code());

    h2.shutdown().await;
}
