//! Paket-6 A3 (Demo a): restart-exhaustion → `failed` persisted, no respawn.
//!
//! Full-topology proof through the REAL `handle_cell_died` corridor + the
//! `colony_task` call-site wiring added in A3. A `hang` cell with
//! `restart_limit: 0` is woken by a probe; it hangs in `handle()` past its small
//! `cell.message_timeout`; the substrate backstop fires `DeathKind::Backstop`;
//! `handle_cell_died` bumps `restart_count` to 1 > limit 0 → returns
//! `CellDiedOutcome::Failed { path }` WITHOUT respawning. The call-site then
//! (1) persists `SetRegistryStatus { status: "failed" }` via the writer and
//! (2) parks the entry non-running.
//!
//! Two positive assertions after exhaustion:
//!   (1) A FRESH `rusqlite::Connection` on the on-disk `colony.db` reads
//!       `registry.status == "failed"` for `/hang` (polled, 30s-convention).
//!   (2) `spawn_count` stays at 1 (the Wake build) — no further respawn fires.

use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::Value;
use meclaw_core::{MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::{EchoCellFactory, HangCellFactory};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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

/// Poll `spawn_count` until it reaches `target` (or time out). Returns the last
/// observed value.
async fn wait_for_spawn_count(counter: &Arc<AtomicU32>, target: u32) -> u32 {
    for _ in 0..300 {
        let n = counter.load(Ordering::Relaxed);
        if n >= target {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    counter.load(Ordering::Relaxed)
}

/// Fresh-read the `registry.status` for `path` via a brand-new read-only
/// `rusqlite::Connection` on the on-disk `colony.db`. Returns `None` while the
/// row is absent (boot race). Polls until the status equals `want` or the
/// 30s-convention timeout elapses; returns the last observed status.
async fn wait_for_registry_status(db_path: &std::path::Path, path: &str, want: &str) -> String {
    let db = db_path.join("colony.db");
    let mut last = String::from("<absent>");
    for _ in 0..600 {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("fresh read-only colony.db connection");
        let status: Option<String> = conn
            .query_row("SELECT status FROM registry WHERE path = ?", [path], |r| {
                r.get(0)
            })
            .ok();
        if let Some(s) = status {
            last = s;
            if last == want {
                return last;
            }
        }
        drop(conn);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    last
}

/// Read the `RegistryEntryDto` for `path` via the `/colony/registry`
/// (`ReadRegistry`) endpoint. `active: None` so failed/inactive entries are
/// included.
async fn read_entry(h: &ColonyHandle, path: &str) -> Option<RegistryEntryDto> {
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
    let reply = ack_rx.await.unwrap();
    reply.entries.into_iter().find(|e| e.path == path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhaustion_marks_failed_persisted_no_respawn() {
    let td = tempfile::TempDir::new().unwrap();

    // Root hive maps to `/`; `/anchor` (echo) provides the activating edge for
    // `/hang`.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // The hang cell: hangs forever, small message_timeout so the B-backstop
    // fires quickly, AND `restart_limit: 0` so the FIRST backstop already
    // exceeds the limit → exhaustion (Failed), not restart.
    write(
        td.path(),
        "main/hang/config.json",
        r#"{"cell":{"type":"hang","message_timeout":300,"restart_limit":0},"params":{"hang_forever":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
    assert_eq!(report.cell_count, 2, "anchor + hang boot");

    // Activate `/hang` via an edge anchor->hang.
    let act = send_mutation(
        &h,
        json!({
            "diff": { "add_edges": [{"from": "anchor", "to": "hang"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->hang must commit, got {act:?}"
    );

    // Probe `/hang` → wakes it (Dormant → Awake). spawn_count: 0 → 1.
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    let after_wake = wait_for_spawn_count(&spawn_count, 1).await;
    assert!(after_wake >= 1, "hang cell must wake (spawn_count >= 1)");

    // handle() hangs → after ~300ms the B-backstop fires → watcher sees
    // DeathKind::Backstop → handle_cell_died bumps restart_count to 1 > limit 0
    // → Failed{path} (NO respawn). The A3 call-site persists status="failed".
    let status = wait_for_registry_status(&h.tempdir_path(), "/hang", "failed").await;
    assert_eq!(
        status, "failed",
        "exhausted hang cell must persist registry.status == \"failed\" (fresh colony.db read)"
    );

    // No further respawn: spawn_count must NOT have advanced past the Wake build.
    let final_count = spawn_count.load(Ordering::Relaxed);
    assert_eq!(
        final_count, 1,
        "exhausted cell must NOT respawn — spawn_count must stay at 1 (Wake only), was {final_count}"
    );

    h.shutdown().await;
}

/// Paket-6 C2 (Demo): a FULLY WIRED cell that exhausts its restart-limit
/// persists `status="failed"` and, after a REBOOT on the same `{root}`/
/// `colony.db`, rehydrates `failed=true, active=false` — even though its
/// activating edge is still present (no `remove_edges`). The persisted `failed`
/// status wins over the edge-derived activity (overview Z.1426): no boot
/// recompute reactivates it.
///
/// Reboot harness (same pattern as `paket_2_swap.rs`
/// `reboot_durability_swung_edge_and_inactive_t2_survive`): `h1.shutdown()` →
/// fresh `ColonyHandle::new_with_factories_at(&td, …)` on the SAME caller-owned
/// `td` → second `bootstrap_from_filesystem`.
///
/// Post-reboot assertions:
///   (a) `/colony/registry` (ReadRegistry) shows `/hang` with
///       `failed=true, active=false`.
///   (b) NO cell-task is spawned for `/hang` — proven POSITIVELY: a probe to
///       `/hang` lands in the DLQ (a live cell would consume it).
///   (c) Routing to `/hang` dead-letters with `error_code == "cell_inactive"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_cell_survives_reboot_fully_wired_no_spawn() {
    let td = tempfile::TempDir::new().unwrap();

    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/hang/config.json",
        r#"{"cell":{"type":"hang","message_timeout":300,"restart_limit":0},"params":{"hang_forever":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // ── BOOT 1: drive /hang into `failed`. ───────────────────────────────────
    let spawn_count1 = Arc::new(AtomicU32::new(0));
    let hang_factory1: Arc<dyn CellFactory> = Arc::new(HangCellFactory {
        spawn_count: spawn_count1.clone(),
    });
    let echo_factory: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);

    let h1 = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("hang".to_string(), hang_factory1.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    let mut reg1 = CellFactoryRegistry::new();
    reg1.insert("hang".to_string(), hang_factory1);
    reg1.insert("echo".to_string(), echo_factory.clone());

    bootstrap_from_filesystem(td.path(), &reg1, &h1.runtime())
        .await
        .expect("boot1 bootstrap must succeed");

    // Activate /hang via edge anchor->hang. This edge is NEVER removed — the
    // cell stays fully wired across the whole test.
    let act = send_mutation(
        &h1,
        json!({
            "diff": { "add_edges": [{"from": "anchor", "to": "hang"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->hang must commit, got {act:?}"
    );

    // Wake /hang via a probe; it hangs → backstop → exhaustion → `failed`.
    h1.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    let woke = wait_for_spawn_count(&spawn_count1, 1).await;
    assert!(woke >= 1, "boot1: /hang must wake (spawn_count >= 1)");

    let status = wait_for_registry_status(&h1.tempdir_path(), "/hang", "failed").await;
    assert_eq!(
        status, "failed",
        "boot1: exhausted /hang must persist registry.status == \"failed\""
    );

    // ── REBOOT: shutdown colony1, start colony2 on the SAME {root}. ──────────
    h1.shutdown().await;

    let spawn_count2 = Arc::new(AtomicU32::new(0));
    let hang_factory2: Arc<dyn CellFactory> = Arc::new(HangCellFactory {
        spawn_count: spawn_count2.clone(),
    });
    let h2 = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("hang".to_string(), hang_factory2.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    let mut reg2 = CellFactoryRegistry::new();
    reg2.insert("hang".to_string(), hang_factory2);
    reg2.insert("echo".to_string(), echo_factory);

    bootstrap_from_filesystem(td.path(), &reg2, &h2.runtime())
        .await
        .expect("boot2 (reboot) bootstrap must succeed");

    // ── POST-REBOOT ASSERTIONS ───────────────────────────────────────────────

    // (a) /colony/registry shows /hang failed:true, active:false. The anchor
    //     edge is still present → edge-derived activity would be `true`, but the
    //     persisted `failed` status wins (overview Z.1426).
    let entry = read_entry(&h2, "/hang")
        .await
        .expect("post-reboot: /hang must remain visible in /colony/registry (No-Delete)");
    assert!(
        entry.failed,
        "post-reboot: /hang must rehydrate failed=true; got {entry:?}"
    );
    assert!(
        !entry.active,
        "post-reboot: /hang must rehydrate active=false (failed wins over edge-derived activity); got {entry:?}"
    );

    // (b)+(c) Probe /hang → no live cell → DLQ `cell_inactive`. A spawned cell
    //     would have consumed the probe; the DLQ entry proves NO spawn happened
    //     and routing dead-letters the message.
    h2.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;

    let mut hit = None;
    for _ in 0..600 {
        let dlq = h2.drain_dead_letters().await;
        if let Some(dl) = dlq
            .into_iter()
            .find(|dl| dl.resolved_target == Path::new("/hang"))
        {
            hit = Some(dl);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let dl = hit.expect("post-reboot: probe to failed /hang must dead-letter");
    assert_eq!(
        dl.reason.as_code(),
        "cell_inactive",
        "post-reboot: routing to a failed cell must DLQ with cell_inactive; got {dl:?}"
    );

    // No spawn for /hang after reboot — the failed cell never got a task.
    let post_reboot_spawns = spawn_count2.load(Ordering::Relaxed);
    assert_eq!(
        post_reboot_spawns, 0,
        "post-reboot: failed /hang must NOT be spawned; spawn_count was {post_reboot_spawns}"
    );

    h2.shutdown().await;
}

/// Paket-6 E1 (regression pin): a message routed to a LIVE `failed` cell (no
/// reboot) dead-letters with `error_code == "cell_inactive"`, distinct from
/// `unresolved_path` (path does not exist).
///
/// WHY this needs NO production change: a `failed` cell carries `active=false`
/// in the registry (No-Delete — the entry stays, see overview § No-Delete-
/// Policy + § Routing-Algorithmus). An inactive target hits the SAME canonical
/// inactive-routing short-circuit as any other inactive cell (`route_with_log`
/// pre-check, NOT `route()` itself — see colony.rs Z.1771). No new `error_code`,
/// no new code path: `failed` reuses `cell_inactive`. This test only pins that
/// the live-failed corridor keeps resolving to `cell_inactive`.
///
/// Same `failed`-driver as `exhaustion_marks_failed_persisted_no_respawn`
/// above (hang-mock + `restart_limit:0` + B-backstop), but WITHOUT a reboot —
/// the probe is sent against the still-running colony1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_cell_routing_dead_letters_cell_inactive_distinct() {
    let td = tempfile::TempDir::new().unwrap();

    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/hang/config.json",
        r#"{"cell":{"type":"hang","message_timeout":300,"restart_limit":0},"params":{"hang_forever":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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

    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // Activate /hang via edge anchor->hang.
    let act = send_mutation(
        &h,
        json!({
            "diff": { "add_edges": [{"from": "anchor", "to": "hang"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->hang must commit, got {act:?}"
    );

    // Wake /hang via a probe; it hangs → B-backstop → exhaustion → `failed`.
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    let woke = wait_for_spawn_count(&spawn_count, 1).await;
    assert!(woke >= 1, "/hang must wake (spawn_count >= 1)");

    // Wait for the live registry to settle on failed/active=false (No-Delete:
    // the entry stays, just flipped inactive). NO reboot — colony1 is still up.
    let status = wait_for_registry_status(&h.tempdir_path(), "/hang", "failed").await;
    assert_eq!(
        status, "failed",
        "live /hang must persist registry.status == \"failed\" (active=false)"
    );
    let entry = read_entry(&h, "/hang")
        .await
        .expect("live: /hang must stay visible in /colony/registry (No-Delete)");
    assert!(
        entry.failed,
        "live: /hang must be failed=true; got {entry:?}"
    );
    assert!(
        !entry.active,
        "live: /hang must be active=false (failed → inactive); got {entry:?}"
    );

    // (1) Probe at the LIVE failed cell /hang → cell_inactive. `failed` carries
    //     active=false → same canonical inactive-routing short-circuit, no new
    //     error_code, no production change.
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    let dl_failed = drain_dlq_for(&h, "/hang").await;
    assert_eq!(
        dl_failed.reason.as_code(),
        "cell_inactive",
        "message to live failed /hang → cell_inactive; got {dl_failed:?}"
    );

    // (2) Discriminating power: a probe at a NON-EXISTENT path /nope (no cell, no hive)
    //     → unresolved_path, NOT cell_inactive.
    h.send(MessageBuilder::new(Path::new("/nope")).build())
        .await;
    let dl_unresolved = drain_dlq_for(&h, "/nope").await;
    assert_eq!(
        dl_unresolved.reason.as_code(),
        "unresolved_path",
        "message to non-existent /nope → unresolved_path; got {dl_unresolved:?}"
    );

    // The two reasons are distinct — failed is NOT conflated with unresolved.
    assert_ne!(
        dl_failed.reason.as_code(),
        dl_unresolved.reason.as_code(),
        "cell_inactive (failed) must be distinct from unresolved_path"
    );

    h.shutdown().await;
}

/// Poll the DLQ until an entry whose `resolved_target == path` appears, then
/// return it (30s-convention timeout).
async fn drain_dlq_for(h: &ColonyHandle, path: &str) -> meclaw_colony::DeadLetter {
    for _ in 0..600 {
        let dlq = h.drain_dead_letters().await;
        if let Some(dl) = dlq
            .into_iter()
            .find(|dl| dl.resolved_target == Path::new(path))
        {
            return dl;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no DLQ entry for resolved_target={path} within timeout");
}
