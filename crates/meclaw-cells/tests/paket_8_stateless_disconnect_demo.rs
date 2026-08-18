//! Paket-8 C — end-to-end demos for the paket-8 stateless-disconnect substrate
//! (Plan C1 + C7).
//!
//! Lives in `meclaw-cells/tests/` (NOT `meclaw-colony/tests/`) because it drives
//! a real `CodeCellFactory` under a live Colony topology — the cross-cutting demo
//! pattern established in Phase 9 V1 (a colony test that needs a cell-factory
//! belongs here, so `meclaw-colony` keeps NO `meclaw-cells` dependency).
//!
//! ## Background (the A+B fix this demo proves end-to-end)
//!
//! A real stateless `code` cell is spawned by `CodeCellFactory::spawn_cell` via
//! `build_stateless_task`, which wires a REAL `stop_tx` into the dispatcher's
//! `stop_rx`. Before Paket 8, `stateless_dispatcher` did NOT fire `peace_tx` on a
//! colony peace-stop: `build_stateless_task` held `peace_tx` as `_peace_keep` and
//! dropped it on task end WITHOUT sending. The watcher (`spawn_watcher`) then saw
//! `peace_rx.await == Err` → `CellDied{DeathKind::Normal}` → `handle_cell_died`
//! did `registry.remove(&path)`. The registry entry VANISHED on disconnect.
//!
//! Paket-8 A+B fixed this: `build_stateless_task` now passes `peace_tx` INTO the
//! dispatcher, and `stateless_dispatcher` fires it on its colony peace-stop arm
//! (→ watcher Ok → NO `CellDied` → entry PRESERVED, `active=false`). It still lets
//! `peace_tx` drop unsent on a genuine mailbox-close (→ watcher Err →
//! `DeathKind::Normal` → remove), which Demo (e) pins below.
//!
//! ## Demo (a) — Disconnect parks the stateless entry; mailbox-rest → DLQ
//!
//! `demo_a_disconnect_parks_stateless_entry_and_dlqs_remainder` proves E2E that
//! disconnecting a stateless `code` cell (remove its last edge → recompute
//! deactivates it → dispatcher peace-stopped) LEAVES the registry entry in place
//! (`active=false`, `cell_id` stable — No-Delete, Z.1397) and routes any further
//! message addressed to the inactive cell into the DLQ as `cell_inactive`
//! (Mailbox-rest → DLQ, Z.1488). Before the Paket-8 fix the entry was removed on
//! disconnect (`registry.remove`), so `cell_id`-after would be `None` and the
//! No-Delete assert would fail — i.e. this demo is exactly the acceptance demo the
//! paket_7 (a3) note deferred to Paket 8.
//!
//! ## Demo (e) — Normal-Exit-Pin (C7, verification only, no new test body)
//!
//! C7 is NOT an E2E test but a pin against scope-creep: the genuine normal exit
//! (mailbox-close) → `DeathKind::Normal` → `registry.remove` must stay BYTE-FROZEN.
//! It is pinned by four existing guards (run + verified at Paket-8-C time):
//!   1. `cargo test -p meclaw-colony spawn_watcher_no_peace_cell_exit_emits_cell_died_normal`
//!      — the watcher still emits `CellDied{Normal}` when no peace is fired.
//!   2. `cargo test -p meclaw-colony handle_cell_died`
//!      — the `handle_cell_died` corridor unit-suite stays green.
//!   3. `diff <(sed -n '/^async fn handle_cell_died(/,/^}$/p' colony.rs)
//!         plans/paket-6-fixtures/expected_handle_cell_died_body.txt` — MUST be empty
//!      (the `#[rustfmt::skip]`-frozen corridor: `Normal` → remove unchanged).
//!   4. `cargo test -p meclaw-colony
//!         cell_task::tests::stateless_dispatcher_fires_peace_on_stop_not_on_mailbox_close`
//!      — the A4 unit pins that the dispatcher fires peace ONLY on the stop arm,
//!      NEVER on a plain mailbox-close (so the Normal-exit→remove path is intact).
//!
//! These four pin the Normal-exit corridor without a dedicated test body here.
//!
//! ## Anti-cascade (phase-6.5/7 demo discipline)
//!
//! `/sink` is a terminal CaptureCell, spawned and resolved BEFORE any probe goes
//! out. A persistent `/anchor -> /sink` edge keeps `/sink` connected after `/code`
//! is disconnected, so the disconnect only deactivates `/code` (not the
//! no-stop-wiring `/sink`). Content probes use the `origin` UBF message form; the
//! inactive-routing remainder probe (Demo a) is an empty UBF envelope
//! (`{"messages": []}`), whose body the `cell_inactive` short-circuit never
//! inspects (it dead-letters before any cell sees the body). Failure-marker
//! timeouts are generous (30 s).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{
    CellFactory, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind, StatelessCell,
    bootstrap_from_filesystem, set_term_timeout_ms_for_test, stateless_dispatcher,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, CellOutput, JsonValue, Message, MessageBuilder, OutputSink, Path, Uuid,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};

/// Root hive + a persistent `/anchor` echo cell. `/anchor` exists only to keep
/// the `/sink` CaptureCell connected via an edge the test never removes, so that
/// disconnecting `/code` does NOT also strand `/sink` (an Awake test sink with no
/// stop-wiring → would otherwise trip the disconnect's stop-wiring guard).
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/anchor")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/anchor/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/anchor"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

fn echo_registry() -> meclaw_colony::CellFactoryRegistry {
    let mut r = meclaw_colony::CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
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

/// Read the RAM registry entry DTO for `path` (or `None` if absent — the
/// pre-Paket-8 disconnect would make this `None` for `/code`).
async fn ram_entry(h: &ColonyHandle, path: &str) -> Option<RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
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

/// Install a trivial `code` template `name` (python3, writes a fixed conforming
/// UBF body), then load it via `RescanTemplates`.
async fn install_code_template(td: &tempfile::TempDir, h: &ColonyHandle, name: &str) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    let script = r#"import sys; sys.stdout.write('{"messages":[{"origin":"assistant","type":"text","text":"ok"}]}')"#;
    let config = json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// `add_nodes` a single code cell from `template` at logical `/<node>`.
async fn add_code_node(h: &ColonyHandle, node: &str, template: &str) {
    let outcome = send_mutation(
        h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":node,"template":template}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /{node} from {template} must commit; got {outcome:?}"
    );
}

/// Collect the `cell_inactive` dead-letters resolved to `path`.
async fn inactive_dlq_for(h: &ColonyHandle, path: &str) -> Vec<Message> {
    let dlq = h.drain_dead_letters().await;
    dlq.into_iter()
        .filter(|d| {
            matches!(
                d.reason,
                meclaw_colony::dead_letter::DeadLetterReason::CellInactive
            ) && d.resolved_target == Path::new(path)
        })
        .map(|d| d.message)
        .collect()
}

/// W2 (Ruling A1): after the disconnect removes the only out-edge of `/gated`,
/// the detached worker's late emission matches NO out-edge → it dead-letters as
/// `no_route` (the routing edge it would have travelled is gone), never reaching
/// the registry `cell_inactive` check. The "lands in DLQ / no zombie delivery"
/// invariant is unchanged; only the reason is now `no_route`.
async fn no_route_dlq_for(h: &ColonyHandle, path: &str) -> Vec<Message> {
    let dlq = h.drain_dead_letters().await;
    dlq.into_iter()
        .filter(|d| {
            matches!(
                d.reason,
                meclaw_colony::dead_letter::DeadLetterReason::NoRoute
            ) && d.resolved_target == Path::new(path)
        })
        .map(|d| d.message)
        .collect()
}

/// Demo (a): disconnecting a real stateless `code` cell PARKS its registry entry
/// (No-Delete) and routes the mailbox-rest into the DLQ as `cell_inactive`.
///
/// Beweist Z.1397 No-Delete (entry stays, `cell_id` stable) + Z.1488
/// Mailbox remainder → DLQ for a stateless cell. Before the paket-8 fix the
/// entry disappeared (`registry.remove` from `handle_cell_died{Normal}`),
/// `cell_id` after would be `None` and the no-delete assert would fire (red-proof
/// note).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_disconnect_parks_stateless_entry_and_dlqs_remainder() {
    // Generous term-timeout budget so a load-slow death-ack never spuriously
    // rejects the disconnect mutation (the dispatcher death-acks fast in
    // practice; this only guards against cargo-parallel scheduling jitter).
    set_term_timeout_ms_for_test(30_000);

    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);

    // /sink BEFORE bootstrap (anti-cascade): terminal CaptureCell.
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_code_template(&td, &h, "worker").await;
    add_code_node(&h, "code", "worker").await;

    // Connect /code -> /sink so /code is active, plus a persistent /anchor ->
    // /sink edge that keeps /sink connected after /code's edge is removed.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"code","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // cell_id BEFORE disconnect — active, real stateless dispatcher running.
    let before = ram_entry(&h, "/code").await.expect("/code registered");
    assert!(before.active, "/code must be active after add_edges");
    let cell_id_before = before.cell_id.clone();

    // Disconnect: remove the last edge of /code → recompute deactivates it →
    // dispatcher peace-stopped (Paket-8: peace fired → watcher Ok → NO CellDied
    // → entry preserved). This commits because the dispatcher death-acks well
    // under TERM_TIMEOUT.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"code","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack < TERM_TIMEOUT), got {outcome:?}"
    );

    // No-Delete (Z.1397): entry STAYS, active == false, cell_id STABLE.
    let after = ram_entry(&h, "/code")
        .await
        .expect("/code entry must STAY per No-Delete (pre-Paket-8 it was removed)");
    assert!(
        !after.active,
        "/code must be inactive after its last edge is removed"
    );
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be stable across disconnect (No-Delete: same entry, not re-created)"
    );

    // Mailbox-rest → DLQ (Z.1488): a probe routed to the now-inactive /code is
    // short-circuited to the DLQ as `cell_inactive` (the inactive-routing guard
    // in route_with_log; deterministic, no timing race).
    let corr = Uuid::now_v7();
    let remainder = MessageBuilder::new(Path::new("/code"))
        .correlation_id(corr)
        .body(Body::Inline(json!({"messages": []})))
        .build();
    h.send_from(Path::new("/"), remainder).await;

    let inactive = inactive_dlq_for(&h, "/code").await;
    assert_eq!(
        inactive.len(),
        1,
        "the post-disconnect probe to inactive /code must hit the DLQ as cell_inactive, got {inactive:?}"
    );
    assert_eq!(
        inactive[0].correlation_id,
        Some(corr),
        "DLQ entry must be the probe we routed after the disconnect"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (a-late) — late worker emission after parking → DLQ / inactive
// ───────────────────────────────────────────────────────────────────────────
//
// ## pin: "workers run to completion" is tested behaviour
//
// `stateless_dispatcher` spawns ONE detached `tokio::task` per message (Z.635 in
// `cell_task.rs`): the worker holds a per-message `OutputSink` + semaphore permit
// and runs INDEPENDENTLY of the dispatcher loop. On a colony peace-stop the
// dispatcher's biased stop-arm wins (Z.598), fires `peace_tx` and returns —
// parking the registry entry (`active=false`, No-Delete, Z.1484/Z.540 "detached
// workers run to completion"). A worker that was already in-flight at the
// disconnect KEEPS RUNNING after the park; when it finally emits, the emission
// rides `outputs_tx` → the colony outputs-arm → `route_with_log`.
//
// This demo pins that such a late emission lands deterministically in the DLQ /
// inactive path — no panic, no zombie delivery to a swung-away/inactive target,
// colony stable.
//
// ## E2 reason observation (against the FROZEN `route()` / `route_with_log` actual)
//
// The worker emits to `/late-sink`. At emission time `/late-sink` STILL EXISTS in
// the registry but is `active == false` (the single edge `/gated -> /late-sink`
// was removed, which the activity-recompute derives as inactive for BOTH
// endpoints). The inactive-routing short-circuit in `route_with_log` (Z.1781-1808
// in `colony.rs`) matches exactly this — `registry.get(resolved).is_some() &&
// !entry.active` — and dead-letters as `DeadLetterReason::CellInactive`
// (`cell_inactive`). It does NOT reach `route()` (which is byte-frozen against
// `expected_route_body.txt`), and `route()` itself would NOT discriminate active
// here anyway (it only does `registry.get(&resolved)` and sends to a live
// handle). The observed reason is therefore `cell_inactive`, NOT `unresolved_path`
// (which is the registry-MISS case — the target never existed). This is an
// OBSERVATION of the frozen routing actual, asserted exactly as measured; `route()`
// is NOT touched.
//
// ## Determinism (no sleeps as a correctness discriminator)
//
//   - `entered`: a `Semaphore(0)`; the worker `add_permits(1)` on entry, the test
//     `acquire` it (30 s failure-marker) → the test KNOWS the worker is in-flight
//     and parked on the gate before it disconnects.
//   - `gate`: a `Semaphore(0)`; the worker `acquire`s it and blocks; the test
//     `add_permits(1)` to release the worker → it runs out and emits.
//   - DLQ-arrival: a bounded poll-until-converged loop over `drain_dead_letters`
//     (30 s failure-marker). The emission→outputs_rx→route→DLQ path is async; the
//     poll is a convergence wait, not a fixed sleep — the failure-marker bounds
//     it, the loop body yields between drains.
//   - Stability: a follow-up probe to the always-active `/live-sink` is awaited on
//     its CaptureCell receipt (30 s failure-marker) — a POSITIVE delivery proof
//     (CaptureCell receives ⟺ cell live), per the demo-discipline lesson.

/// Test-local stateless cell that, on `handle()`: (1) signals "entered" to the
/// test (`entered.add_permits(1)`), (2) blocks until the test releases the
/// `gate` (`gate.acquire()`), then (3) emits EXACTLY ONE valid-UBF message to
/// `/late-sink`. Read-only config only (the two `Arc<Semaphore>`) — no mutable
/// cell state, per the `StatelessCell` contract.
struct GatedEmitCell {
    entered: Arc<Semaphore>,
    gate: Arc<Semaphore>,
}

impl StatelessCell for GatedEmitCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        _msg: Message,
        sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // (1) tell the test the worker is in-flight.
            self.entered.add_permits(1);
            // (2) block until the test releases the gate (deterministic, not a
            //     timer). `forget()` so the permit is not returned on drop.
            if let Ok(p) = self.gate.acquire().await {
                p.forget();
            }
            // (3) the late emission — valid origin-UBF body (`assistant`).
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/late-sink"),
                    content: json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": "late"}]
                    }),
                })
                .await;
        }
    }
}

/// Factory for `/gated`: spawns the `stateless_dispatcher` with REAL stop-wiring
/// (Active kind, live `stop_tx`) so the disconnect can peace-stop it cleanly.
/// `max_concurrency = 4` (the worker is a detached task regardless). Mirrors the
/// committed `CodeCellFactory::spawn_cell` shape (build the dispatcher task, wire
/// peace/stop/death_ack/backstop, return `SpawnedCellKind::Active`).
struct GatedEmitFactory {
    entered: Arc<Semaphore>,
    gate: Arc<Semaphore>,
}

impl CellFactory for GatedEmitFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let cell = Arc::new(GatedEmitCell {
            entered: self.entered.clone(),
            gate: self.gate.clone(),
        });
        let join = tokio::spawn(async move {
            stateless_dispatcher(
                path,
                rx,
                outputs_tx,
                cell,
                4,              // max_concurrency
                None,           // message_timeout (no B-backstop)
                Some(peace_tx), // Paket-8: dispatcher fires peace on the stop arm.
                Some(stop_rx),
                Some(colony_inbox_tx),
                Some(death_ack_tx),
                None,
                None,
            )
            .await;
        });
        let respawn: RespawnFn = Box::new(|| unreachable!("GatedEmit: no restart in this test"));
        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Test-local stateless capture cell: taps every received message into an mpsc
/// `tx` (for the "received NOTHING" / "received the stability probe" asserts).
/// Used for `/late-sink` (must be deactivatable WITH live stop-wiring, so a
/// `h.spawn` no-stop-wiring sink — which the permanent stop-wiring guard would
/// reject on disconnect — is not usable here).
struct CaptureStatelessCell {
    tx: mpsc::Sender<Message>,
}

impl StatelessCell for CaptureStatelessCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        msg: Message,
        _sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        let tx = self.tx.clone();
        async move {
            let _ = tx.send(msg).await;
        }
    }
}

/// Factory for `/late-sink`: a `CaptureStatelessCell` dispatcher with REAL
/// stop-wiring (Active kind) so the disconnect can deactivate it without tripping
/// the permanent stop-wiring guard (`stop_wiring_unavailable`).
struct CaptureStatelessFactory {
    tx: mpsc::Sender<Message>,
}

impl CellFactory for CaptureStatelessFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let cell = Arc::new(CaptureStatelessCell {
            tx: self.tx.clone(),
        });
        let join = tokio::spawn(async move {
            stateless_dispatcher(
                path,
                rx,
                outputs_tx,
                cell,
                4,
                None,
                Some(peace_tx),
                Some(stop_rx),
                Some(colony_inbox_tx),
                Some(death_ack_tx),
                None,
                None,
            )
            .await;
        });
        let respawn: RespawnFn =
            Box::new(|| unreachable!("CaptureStateless: no restart in this test"));
        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Root hive + a persistent `/live-anchor` echo cell. `/live-anchor` exists only
/// to keep the `/live-sink` CaptureCell connected via an edge the test never
/// removes, so `/live-sink` stays active for the positive stability proof.
fn write_topology_a_late(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/live-anchor")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/live-anchor/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/live-anchor"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Poll `drain_dead_letters` until at least one `cell_inactive` dead-letter
/// resolved to `path` appears, or the 30 s failure-marker elapses. Returns the
/// matching messages. The poll is a convergence wait (emission→route→DLQ is
/// async), bounded by the marker; it is NOT a fixed sleep used as a correctness
/// discriminator — the correctness signal is "a `cell_inactive` DLQ entry exists",
/// the marker only bounds how long we wait for the async pipeline to converge.
async fn await_no_route_dlq(h: &ColonyHandle, path: &str) -> Vec<Message> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let hit = no_route_dlq_for(h, path).await;
        if !hit.is_empty() {
            return hit;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("30s failure-marker: no no_route DLQ entry for {path} appeared");
        }
        tokio::task::yield_now().await;
    }
}

/// Demo (a-late): a detached stateless worker that was in-flight at disconnect
/// emits AFTER the park; the emission lands in the DLQ as `cell_inactive` (E2:
/// observed against the frozen routing actual), no panic, and the colony stays
/// stable (a follow-up probe to the always-active `/live-sink` is delivered).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_late_detached_worker_emission_after_park_lands_in_dlq() {
    set_term_timeout_ms_for_test(30_000);

    let td = tempfile::TempDir::new().unwrap();
    write_topology_a_late(td.path());

    // Deterministic signals: `entered` (worker → test), `gate` (test → worker).
    let entered = Arc::new(Semaphore::new(0));
    let gate = Arc::new(Semaphore::new(0));

    let gated_factory: Arc<dyn CellFactory> = Arc::new(GatedEmitFactory {
        entered: entered.clone(),
        gate: gate.clone(),
    });

    // /late-sink tap (must capture NOTHING — the late emission is DLQ'd).
    let (late_tx, mut late_rx) = mpsc::channel::<Message>(8);
    let late_factory: Arc<dyn CellFactory> = Arc::new(CaptureStatelessFactory { tx: late_tx });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("echo".to_string(), {
            Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn CellFactory>
        })],
    );

    // /live-sink BEFORE bootstrap (anti-cascade): always-active CaptureCell for
    // the positive stability proof. Kept active by the persistent /live-anchor
    // edge, so it is never deactivated → never trips the stop-wiring guard.
    let (live_tx, mut live_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/live-sink"), move || {
        CaptureCell::new(live_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Register /gated (Active, live stop-wiring) via the gated factory.
    let gated_dir = td.path().join("main/gated");
    std::fs::create_dir_all(&gated_dir).unwrap();
    let gated_spawned = gated_factory
        .spawn_cell(
            Path::new("/gated"),
            json!({}),
            h.runtime().outputs_tx,
            gated_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/gated"), gated_spawned).await;

    // Register /late-sink (Active, live stop-wiring) via the capture factory.
    let late_dir = td.path().join("main/late-sink");
    std::fs::create_dir_all(&late_dir).unwrap();
    let late_spawned = late_factory
        .spawn_cell(
            Path::new("/late-sink"),
            json!({}),
            h.runtime().outputs_tx,
            late_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/late-sink"), late_spawned)
        .await;

    // Edges: /gated -> /late-sink (THE single edge of both — removing it
    // deactivates both); /live-anchor -> /live-sink (persistent, kept active).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"gated","to":"late-sink"},
            {"from":"live-anchor","to":"live-sink"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges must commit, got {outcome:?}"
    );

    // /gated active, cell_id captured (No-Delete stability check later).
    let before = ram_entry(&h, "/gated").await.expect("/gated registered");
    assert!(before.active, "/gated must be active after add_edges");
    let gated_cell_id = before.cell_id.clone();

    // Probe /gated → dispatcher recvs → spawns a DETACHED worker → worker signals
    // "entered" and blocks on the gate. Empty-UBF body is fine: GatedEmitCell does
    // not read it. The probe goes via the inbox Route arm (not outputs).
    let probe = MessageBuilder::new(Path::new("/gated"))
        .body(Body::Inline(json!({"messages": []})))
        .build();
    h.send_from(Path::new("/"), probe).await;

    // Deterministic wait for the worker to be in-flight (30 s failure-marker).
    tokio::time::timeout(Duration::from_secs(30), entered.acquire())
        .await
        .expect("30s failure-marker: /gated worker never entered handle()")
        .expect("entered semaphore closed")
        .forget();

    // Disconnect /gated: remove the single edge → recompute deactivates BOTH
    // /gated and /late-sink → /gated dispatcher peace-stopped (the detached worker
    // keeps running, still blocked on the gate). Commits because both cells have
    // live stop-wiring (death-ack < TERM_TIMEOUT).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"gated","to":"late-sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (both endpoints have live stop-wiring), got {outcome:?}"
    );

    // Park confirmed: /gated stays (No-Delete), inactive, cell_id stable.
    let after = ram_entry(&h, "/gated")
        .await
        .expect("/gated entry must STAY per No-Delete");
    assert!(!after.active, "/gated must be inactive after disconnect");
    assert_eq!(
        after.cell_id, gated_cell_id,
        "cell_id must be stable across disconnect (No-Delete)"
    );
    // /late-sink is also parked inactive (the single edge deactivated it).
    let late_after = ram_entry(&h, "/late-sink")
        .await
        .expect("/late-sink entry must STAY per No-Delete");
    assert!(
        !late_after.active,
        "/late-sink must be inactive after the shared edge is removed"
    );

    // Release the gate → the detached worker runs out and emits to /late-sink.
    gate.add_permits(1);

    // E2 OBSERVATION: the late emission is DLQ'd (NOT delivered to /late-sink).
    // W2 (Ruling A1): the disconnect removed /gated's only out-edge, so the late
    // emission matches no edge → `no_route` (it never reaches the registry
    // `cell_inactive` check). "Lands in DLQ / no zombie delivery" is unchanged.
    let dlq = await_no_route_dlq(&h, "/late-sink").await;
    assert_eq!(
        dlq.len(),
        1,
        "the late detached-worker emission after the edge was removed must hit the \
         DLQ as no_route, got {dlq:?}"
    );

    // No zombie delivery: /late-sink's CaptureCell received NOTHING.
    assert!(
        late_rx.try_recv().is_err(),
        "/late-sink (inactive) must NOT have received the late emission (no zombie delivery)"
    );

    // Colony stable (positive proof): a follow-up probe to the always-active
    // /live-sink is delivered cleanly to its CaptureCell.
    let live_probe = MessageBuilder::new(Path::new("/live-sink"))
        .body(Body::Inline(json!({"messages": []})))
        .build();
    h.send_from(Path::new("/"), live_probe).await;
    tokio::time::timeout(Duration::from_secs(30), live_rx.recv())
        .await
        .expect("30s failure-marker: /live-sink never received the stability probe")
        .expect("/live-sink tap closed");

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (b) — Reconnect → eager Respawn, stateless code cell works again
// ───────────────────────────────────────────────────────────────────────────
//
// The A+B fix this demo proves E2E: after a disconnect a stateless `code` entry
// parks as `NotYetSpawned` with `entry.respawn` preserved and
// `eager_on_reconnect=true` (handle_stopped). An `add_edges` reconnect hits the
// reconnect-eager arm → `(entry.respawn)()` → an IMMEDIATE re-spawn of the
// dispatcher. The respawned cell processes a fresh probe and emits to `/sink`
// again — a POSITIVE receipt proves it is live (CaptureCell receives ⟺ cell
// live; demo discipline). The `cell_id` is stable across the whole
// disconnect→reconnect cycle (No-Delete: same entry, never re-created).
//
// Proves: Reconnect (Z.1404) + eager-Respawn for a real stateless cell.
//
// ## Determinism (no sleeps as a correctness discriminator)
//
//   - `/sink` is a terminal CaptureCell resolved BEFORE any probe (anti-cascade),
//     kept active by a persistent `/anchor -> /sink` edge so the disconnect only
//     deactivates `/code`.
//   - Probes carry `reply_to=/sink`; the `code` cell emits to its `reply_to`, so
//     a sink receipt is the positive liveness signal.
//   - Each liveness check awaits the sink CaptureCell receipt (30 s
//     failure-marker) — the receipt is the barrier, NOT a fixed sleep. The
//     post-reconnect probe is only sent AFTER the committed `add_edges` reconnect
//     (the eager re-spawn happens synchronously inside that committed mutation).

/// A probe addressed to `/code` with `reply_to=/sink`; the `code` cell emits its
/// fixed script output to `reply_to`. Valid (empty) UBF input body — the cell
/// ignores the input body and runs its fixed script.
fn probe_to_code_reply_sink() -> Message {
    MessageBuilder::new(Path::new("/code"))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// Demo (b): reconnecting a disconnected stateless `code` cell (`add_edges`
/// reconnect → eager `(entry.respawn)()`) re-spawns it IMMEDIATELY; a fresh probe
/// produces an emission at `/sink` again (positive receipt ⟹ cell live), and the
/// `cell_id` is stable across the whole disconnect→reconnect cycle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_b_reconnect_eager_respawns_stateless_cell() {
    set_term_timeout_ms_for_test(30_000);

    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);

    // /sink BEFORE bootstrap (anti-cascade): terminal CaptureCell that taps every
    // emission for the positive liveness receipt.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_code_template(&td, &h, "worker").await;
    add_code_node(&h, "code", "worker").await;

    // /code -> /sink keeps /code active; persistent /anchor -> /sink keeps /sink
    // active after /code's edge is removed.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"code","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Pre-proof: /code is live BEFORE the disconnect — a probe lands at /sink.
    let before = ram_entry(&h, "/code").await.expect("/code registered");
    assert!(before.active, "/code must be active after add_edges");
    let cell_id_before = before.cell_id.clone();

    h.send_from(Path::new("/"), probe_to_code_reply_sink())
        .await;
    tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("30s failure-marker: /sink never received the pre-disconnect emission")
        .expect("/sink tap closed");

    // Disconnect: remove /code's last edge → recompute deactivates it → dispatcher
    // peace-stopped → entry PARKED (No-Delete, active=false, respawn preserved,
    // eager_on_reconnect=true). Commits because the dispatcher death-acks fast.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"code","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack < TERM_TIMEOUT), got {outcome:?}"
    );

    let parked = ram_entry(&h, "/code")
        .await
        .expect("/code entry must STAY per No-Delete");
    assert!(!parked.active, "/code must be inactive after disconnect");
    assert_eq!(
        parked.cell_id, cell_id_before,
        "cell_id must be stable across disconnect (No-Delete: same parked entry)"
    );

    // Reconnect: re-add /code -> /sink → recompute reactivates /code → the
    // reconnect-eager arm fires `(entry.respawn)()` → IMMEDIATE re-spawn of the
    // stateless dispatcher (eager_on_reconnect=true).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"code","to":"sink"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "reconnect must commit, got {outcome:?}"
    );

    // /code active again, cell_id STILL stable across the full cycle.
    let after = ram_entry(&h, "/code")
        .await
        .expect("/code entry present after reconnect");
    assert!(
        after.active,
        "/code must be active again after the reconnect (eager respawn)"
    );
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be STABLE across the whole disconnect→reconnect cycle"
    );

    // Positive liveness proof: a probe sent AFTER the reconnect produces an
    // emission at /sink again ⟹ the eager-respawned dispatcher is live. The sink
    // receipt is the barrier (30 s failure-marker), not a sleep.
    h.send_from(Path::new("/"), probe_to_code_reply_sink())
        .await;
    let receipt = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("30s failure-marker: eager-respawned /code never emitted to /sink")
        .expect("/sink tap closed");
    // The receipt is a regular (non-error) emission from the conforming worker.
    assert!(
        receipt.headers.hop.get("error_code").is_none(),
        "post-reconnect emission must be a regular receipt, not an error; headers: {:?}",
        receipt.headers.hop
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (c=a3) — reconnect-respawned code cell STILL enforces contract.emits
// ───────────────────────────────────────────────────────────────────────────
//
// This is the Paket-7-(a3) acceptance demo deferred to Paket 8. In Paket 7
// it could NOT be driven for a real
// stateless `code` cell, because a disconnect made the registry entry VANISH
// (`stateless_dispatcher` did not fire `peace_tx` → `CellDied{Normal}` →
// `registry.remove`), so there was no entry to reconnect/respawn. Paket-8 A+B
// fixed the substrate (the entry now parks instead of dying), so the
// disconnect→reconnect eager-respawn path is finally drivable.
//
// GOAL: a `code` cell with `contract.emits` + the resolved `validate_emits` flag
// rejects a VIOLATING emission as `contract_violation`. After a disconnect→
// reconnect (eager `(entry.respawn)()` re-spawn) it must STILL reject the SAME
// violating emission as `contract_violation` — proving the respawn closure carries
// the compiled `emits` + `validate_emits` across the reconnect (the
// `RespawnFn` in `CodeCellFactory::spawn_cell` reuses the SAME `Arc<CodeCell>`,
// built once with `contract.emits` + `contract.validate_emits`).
//
// ## Contract setup (mirrored EXACTLY from paket_7_contract_demo.rs)
//
// The contract requires `body.messages:array required`; the script writes an
// object WITHOUT `messages` (`{"foo":"bar"}`) → `inject_standard_headers` only
// touches the header → the body stays violated on the final content → the cell
// emits a `contract_violation` error to `reply_to` (paket_7's `demo_c_body_
// violation` shape). The colony's mutation-spawn path resolves `validate_emits =
// cfg!(debug_assertions) || strict_validation`, which is TRUE in test builds —
// so the contract genuinely enforces here without direct flag injection (same as
// paket_7's colony-driven demo_c, which is why a template config carrying
// `contract.emits` is the canonical, not the direct-cell, vehicle).
//
// ## Determinism — sink receipt as the barrier (no sleeps)
//
// `/sink` (CaptureCell) is resolved before any probe (anti-cascade), kept active
// by a persistent `/anchor -> /sink` edge. The violating emission rides
// `reply_to=/sink`; the violation receipt at /sink is the barrier (30 s
// failure-marker). The post-reconnect violating probe is sent only AFTER the
// committed `add_edges` reconnect (the eager re-spawn is synchronous in it).

/// Install a `code` template `name` whose script writes `script_stdout` verbatim
/// and declares `contract.emits = emits_json`, then load it via `RescanTemplates`.
/// Mirrors `paket_7_contract_demo.rs::install_code_template` (the canonical
/// `contract.emits` template shape). `validate_emits` is resolved by the colony
/// (`cfg!(debug_assertions) || strict_validation`, true in test builds).
async fn install_contract_code_template(
    td: &tempfile::TempDir,
    h: &ColonyHandle,
    name: &str,
    emits_json: &str,
    script_stdout: &str,
) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();

    let literal = meclaw_core::serde_json::to_string(script_stdout).unwrap();
    let script = format!("import sys; sys.stdout.write({literal})");
    let config = json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "0.1.0",
            "settings": {},
            "consumes": {},
            "multi_send_capable": false,
            "emits": meclaw_core::serde_json::from_str::<Value>(emits_json).unwrap()
        }
    });
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// Send a VIOLATING probe to `/code` (`reply_to=/sink`) and assert the receipt at
/// `/sink` is a `contract_violation` (`finish_reason:"error"`, message preserved,
/// i.e. delivered). 30 s failure-marker; the sink receipt is the barrier.
async fn assert_violation_at_sink(
    h: &ColonyHandle,
    sink_rx: &mut mpsc::Receiver<Message>,
    what: &str,
) {
    h.send_from(Path::new("/"), probe_to_code_reply_sink())
        .await;
    let m = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("30s failure-marker: no violation receipt at /sink ({what})"))
        .unwrap_or_else(|| panic!("/sink tap closed ({what})"));
    assert_eq!(
        m.headers.hop["error_code"], "contract_violation",
        "{what}: violating emission must yield contract_violation; headers: {:?}",
        m.headers.hop
    );
    assert_eq!(
        m.headers.hop["finish_reason"], "error",
        "{what}: contract_violation receipt must carry finish_reason=error"
    );
}

/// Demo (c=a3): a reconnect-respawned `code` cell STILL enforces `contract.emits`
/// — the same violating emission yields `contract_violation` BOTH before the
/// disconnect (baseline) and after the reconnect (eager respawn), with the
/// `cell_id` stable across the cycle. Proves the respawn closure carries the
/// compiled `emits` + resolved `validate_emits` across reconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_c_a3_reconnected_code_cell_still_enforces_contract() {
    set_term_timeout_ms_for_test(30_000);

    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);

    // /sink BEFORE bootstrap (anti-cascade): terminal CaptureCell taps the
    // contract_violation receipts (reply_to=/sink).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Contract requires body.messages:array required; the script writes an object
    // WITHOUT messages → the final content stays body-violated → contract_violation
    // (paket_7 demo_c_body_violation shape). validate_emits resolves true in tests.
    install_contract_code_template(
        &td,
        &h,
        "violworker",
        r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
        r#"{"foo":"bar"}"#,
    )
    .await;
    add_code_node(&h, "code", "violworker").await;

    // /code -> /sink keeps /code active; persistent /anchor -> /sink keeps /sink
    // active after /code's edge is removed.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"code","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    let before = ram_entry(&h, "/code").await.expect("/code registered");
    assert!(before.active, "/code must be active after add_edges");
    let cell_id_before = before.cell_id.clone();

    // Baseline: BEFORE the disconnect the violating emission is rejected.
    assert_violation_at_sink(&h, &mut sink_rx, "baseline (pre-disconnect)").await;

    // Disconnect: remove /code's last edge → park (No-Delete, active=false,
    // respawn + contract preserved, eager_on_reconnect=true).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"code","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit, got {outcome:?}"
    );
    let parked = ram_entry(&h, "/code")
        .await
        .expect("/code entry must STAY per No-Delete");
    assert!(!parked.active, "/code must be inactive after disconnect");
    assert_eq!(
        parked.cell_id, cell_id_before,
        "cell_id must be stable across disconnect (No-Delete)"
    );

    // Reconnect: re-add /code -> /sink → reactivates → reconnect-eager arm fires
    // `(entry.respawn)()` → IMMEDIATE re-spawn. The respawn closure reuses the SAME
    // Arc<CodeCell> (built once with contract.emits + validate_emits).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"code","to":"sink"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "reconnect must commit, got {outcome:?}"
    );
    let after = ram_entry(&h, "/code")
        .await
        .expect("/code entry present after reconnect");
    assert!(after.active, "/code must be active again after reconnect");
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be STABLE across the whole disconnect→reconnect cycle"
    );

    // A3 PROOF: after the eager respawn the SAME violating emission is STILL
    // rejected as contract_violation — the contract survived the reconnect.
    assert_violation_at_sink(&h, &mut sink_rx, "post-reconnect (a3 acceptance)").await;

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (d) — stateless graph-swap is REVERSIBLE (Plan C5)
// ───────────────────────────────────────────────────────────────────────────
//
// Repairs the Paket-2 rollback ("blue-green" swing-back) promise FOR STATELESS
// cells. A `swap_nodes(match=t2, with=t3)` swings ALL external edges of the
// stateless `code` cell `/impl` (t2) atomically onto `/impl2` (t3) → `/impl`
// becomes edge-less → recompute deactivates it → its stateless dispatcher is
// peace-stopped → the entry PARKS (No-Delete, active=false, cell_id stable).
//
// BEFORE the Paket-8 A+B fix this was impossible for a stateless cell: the first
// disconnect made the registry entry VANISH (`stateless_dispatcher` did not fire
// `peace_tx` → `CellDied{Normal}` → `registry.remove`), so there was nothing to
// swing BACK to — t2 was gone, not retained. With the fix t2 parks instead of
// dying, so the canonical Paket-2 rollback `swap_nodes(match=t3, with=t2)`
// (existing-target form) swings the edges BACK onto the retained `/impl`, which
// the reconnect-eager arm immediately re-spawns.
//
// PROOF (positive-and-deep):
//   1. `/impl`'s `cell_id` is STABLE across BOTH swing directions (== baseline)
//      — the same parked entry is reused, never re-created (No-Delete).
//   2. After the swing-back `/impl` processes an origin-probe again and emits to
//      `/sink` (positive CaptureCell receipt ⟺ cell live), exactly the Paket-2
//      rollback liveness guarantee, now satisfied for a stateless cell.
//
// ## Determinism (no sleeps as a correctness discriminator)
//
//   - `/sink` is a terminal CaptureCell resolved BEFORE any probe (anti-cascade),
//     kept active by a persistent `/anchor -> /sink` edge so neither swing
//     strands it.
//   - The structural swing is asserted via the deterministic `edges` DTO read;
//     the liveness probe carries `reply_to=/sink` and is awaited on the sink
//     receipt (30 s failure-marker), sent only AFTER the committed swing-back
//     (the eager re-spawn is synchronous inside that committed mutation).

/// Read the persisted edge list (from, to) from the colony graph DTO.
async fn swap_edges(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .edges
        .into_iter()
        .map(|e| (e.from, e.to))
        .collect()
}

/// An origin-probe addressed to `path` with `reply_to=/sink`; the `code` cell
/// emits its fixed conforming script output to `reply_to`. Valid (empty) UBF
/// input body — the cell ignores the input body and runs its fixed script.
fn probe_to_reply_sink(path: &str) -> Message {
    MessageBuilder::new(Path::new(path))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// Demo (d): a stateless `code` graph-swap is REVERSIBLE — swinging the external
/// edges of `/impl` (t2) onto `/impl2` (t3) parks `/impl` (No-Delete), and the
/// canonical Paket-2 rollback swing-back re-activates the RETAINED `/impl`. The
/// `cell_id` of `/impl` is stable across BOTH swing directions, and after the
/// swing-back `/impl` is live again (positive `/sink` receipt). This repairs the
/// Paket-2 rollback promise for stateless cells (pre-Paket-8 `/impl` vanished on
/// the first disconnect → un-swing-back-able).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_d_stateless_swap_is_reversible() {
    set_term_timeout_ms_for_test(30_000);

    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);

    // /sink BEFORE bootstrap (anti-cascade): terminal CaptureCell for the
    // positive liveness receipt.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_code_template(&td, &h, "worker").await;
    // t2 = /impl (the stateless cell under swap). t3 (the swap target) is
    // instantiated from the same `worker` template by the swap itself.
    add_code_node(&h, "impl", "worker").await;

    // Wire /impl -> /sink so /impl is active, plus a persistent /anchor -> /sink
    // edge that keeps /sink connected after /impl's edge is swung away.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"impl","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Baseline: /impl active with a real stateless dispatcher; capture cell_id.
    let before = ram_entry(&h, "/impl").await.expect("/impl registered");
    assert!(before.active, "/impl must be active after add_edges");
    let cell_id_before = before.cell_id.clone();

    // Pre-swap sanity: the edge is /impl -> /sink.
    let pre = swap_edges(&h).await;
    assert!(
        pre.contains(&("/impl".to_string(), "/sink".to_string())),
        "pre-swap edge must be /impl -> /sink; got {pre:?}"
    );

    // ── SWAP: t2 (/impl) -> fresh t3 (/impl2, template `worker`). ────────────
    // Swings the external /impl -> /sink edge onto /impl2 -> /sink. /impl goes
    // edge-less → recompute deactivates it → its stateless dispatcher is
    // peace-stopped → the entry PARKS (No-Delete; pre-Paket-8 it would VANISH).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"impl"},"with":{"template":"worker","name":"impl2","params":{}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "stateless graph-swap must commit; got {outcome:?}"
    );

    // Structural: edge swung to /impl2 -> /sink, old /impl -> /sink gone.
    let after_swap = swap_edges(&h).await;
    assert!(
        after_swap.contains(&("/impl2".to_string(), "/sink".to_string())),
        "edge must swing to /impl2 -> /sink; got {after_swap:?}"
    );
    assert!(
        !after_swap.contains(&("/impl".to_string(), "/sink".to_string())),
        "old /impl -> /sink edge must be gone after swap; got {after_swap:?}"
    );

    // No-Delete: /impl entry STAYS, inactive, cell_id STABLE (the Paket-8 fix —
    // pre-fix the stateless entry was removed here, breaking swing-back).
    let parked = ram_entry(&h, "/impl")
        .await
        .expect("/impl entry must STAY per No-Delete (pre-Paket-8 it vanished on swap)");
    assert!(
        !parked.active,
        "/impl must be inactive after its edge is swung away"
    );
    assert_eq!(
        parked.cell_id, cell_id_before,
        "cell_id must be stable across the forward swing (No-Delete: same parked entry)"
    );

    // ── SWING BACK: t3 (/impl2) -> t2 (/impl), existing-target form. ─────────
    // Canonical Paket-2 rollback: swings the edges BACK onto the RETAINED /impl
    // → recompute reactivates /impl → the reconnect-eager arm fires
    // `(entry.respawn)()` → IMMEDIATE re-spawn of the stateless dispatcher.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"impl2"},"with":{"name":"impl"}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "swing-back swap must commit; got {outcome:?}"
    );

    // Structural: edge swung BACK to /impl -> /sink, /impl2 -> /sink gone.
    let after_back = swap_edges(&h).await;
    assert!(
        after_back.contains(&("/impl".to_string(), "/sink".to_string())),
        "edge must swing back to /impl -> /sink; got {after_back:?}"
    );
    assert!(
        !after_back.contains(&("/impl2".to_string(), "/sink".to_string())),
        "old /impl2 -> /sink edge must be gone after swing-back; got {after_back:?}"
    );

    // /impl active again, cell_id STILL stable across BOTH swing directions.
    let after = ram_entry(&h, "/impl")
        .await
        .expect("/impl entry present after swing-back");
    assert!(
        after.active,
        "/impl must be active again after the swing-back (eager respawn)"
    );
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be STABLE across BOTH swing directions (No-Delete: never re-created)"
    );

    // Positive liveness proof: an origin-probe sent AFTER the swing-back produces
    // an emission at /sink again ⟹ the eager-respawned /impl is live. The sink
    // receipt is the barrier (30 s failure-marker), not a sleep.
    h.send_from(Path::new("/"), probe_to_reply_sink("/impl"))
        .await;
    let receipt = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("30s failure-marker: swing-back /impl never emitted to /sink")
        .expect("/sink tap closed");
    assert!(
        receipt.headers.hop.get("error_code").is_none(),
        "post-swing-back emission must be a regular receipt, not an error; headers: {:?}",
        receipt.headers.hop
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (f) — parked stateless cell SURVIVES a reboot and RECONNECTS eagerly
// ───────────────────────────────────────────────────────────────────────────
//
// Proves "half B" (the REAL boot-inactive respawn from `CodeCellFactory`)
// end-to-end: a disconnected (parked) stateless `code` cell survives a colony
// reboot AND is reconnect-able afterwards WITHOUT a further reboot.
//
// On a fresh boot the rehydrated `/code` entry is INACTIVE (its only edge was
// removed before shutdown; the inactive status lives in `colony.db`). The boot
// path calls the code-factory's `build_boot_inactive_respawn` → a REAL respawn
// hook, so the entry parks as `NotYetSpawned` with `eager_on_reconnect=true`.
// An `add_edges` reconnect therefore eager-respawns the dispatcher IMMEDIATELY
// (exactly the in-process Demo (b) behaviour, now across a reboot).
//
// Counter-check (why half B matters): without it, `build_boot_inactive_respawn`
// would return the default `None` → the rehydrated entry would carry NO respawn
// hook → an `add_edges` reconnect could not eager-spawn it, and a stateless cell
// has NO wake-on-message path (no `Dormant`/wake closure like a stateful cell) →
// the cell would be permanently un-reconnectable post-reboot. This demo passing
// is the proof that half B supplies the hook.
//
// PROOF (positive-and-deep):
//   1. `/code`'s `cell_id` is STABLE across the reboot (read from `colony.db` in
//      the SECOND colony == the value read in the first colony).
//   2. The post-reboot `add_edges` reconnect behaves like an in-process reconnect
//      (Demo b): `/code` is active again and processes an origin-probe → emits to
//      `/sink` (positive CaptureCell receipt ⟺ cell live after reboot+reconnect).
//
// ## Reboot-on-same-root harness (same as paket_2_swap.rs Demo (f))
//
//   `ColonyHandle::new_with_factories_at(&td, factories)` opens `colony.db` under
//   the caller-owned `td` (TempDir). The caller owns the dir, so after
//   `h1.shutdown().await` a SECOND `new_with_factories_at(&td, factories)` on the
//   SAME `td` re-hydrates from the SAME `{root}/colony.db`. Reusing the same
//   `&td` (NOT a new TempDir) is what makes the reboot durable; `CodeCellFactory`
//   is stateless, so a fresh factory `Arc` per boot is equivalent.
//
// ## Determinism (no sleeps as a correctness discriminator)
//
//   - `/sink` is a terminal CaptureCell resolved BEFORE any probe in BOTH boots
//     (anti-cascade), kept active by a persistent `/anchor -> /sink` edge.
//   - Probes carry `reply_to=/sink`; each liveness check awaits the sink receipt
//     (30 s failure-marker). The post-reboot probe is sent only AFTER the
//     committed `add_edges` reconnect (the eager re-spawn is synchronous in it).

/// Demo (f): a disconnected (parked) stateless `code` cell survives a colony
/// reboot and reconnects eagerly. After a disconnect→reboot the rehydrated
/// `/code` entry is inactive with a REAL boot-inactive respawn hook (half B);
/// an `add_edges` reconnect eager-respawns it WITHOUT a further reboot, and a
/// fresh origin-probe emits to `/sink` again. The `cell_id` is stable across the
/// reboot (read from `colony.db`). Counter-check: without half B the entry would
/// carry no respawn hook and (stateless = no wake-on-message path) stay
/// permanently un-reconnectable post-reboot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_f_parked_stateless_cell_survives_reboot_and_reconnects() {
    set_term_timeout_ms_for_test(30_000);

    // FIXED root: the caller-owned TempDir survives shutdown so the SECOND colony
    // re-hydrates from the SAME {root}/colony.db (reboot-on-same-root).
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1 ──────────────────────────────────────────────────────────────
    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h1 = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);

    // /sink BEFORE bootstrap (anti-cascade): terminal CaptureCell.
    let (sink_tx1, mut sink_rx1) = mpsc::channel::<Message>(8);
    h1.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx1.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h1.runtime())
        .await
        .expect("boot1 bootstrap");

    install_code_template(&td, &h1, "worker").await;
    add_code_node(&h1, "code", "worker").await;

    // /code -> /sink keeps /code active; persistent /anchor -> /sink keeps /sink
    // active after /code's edge is removed.
    let outcome = send_mutation(
        &h1,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"code","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Pre-proof: /code is live in boot1 — a probe lands at /sink. Capture cell_id.
    let before = ram_entry(&h1, "/code").await.expect("/code registered");
    assert!(before.active, "/code must be active after add_edges");
    let cell_id_before = before.cell_id.clone();

    h1.send_from(Path::new("/"), probe_to_reply_sink("/code"))
        .await;
    tokio::time::timeout(Duration::from_secs(30), sink_rx1.recv())
        .await
        .expect("30s failure-marker: /sink never received the boot1 emission")
        .expect("/sink tap closed (boot1)");

    // Disconnect: remove /code's last edge → recompute deactivates it → dispatcher
    // peace-stopped → entry PARKED (No-Delete, active=false). The inactive status
    // is persisted into colony.db, so it survives the reboot.
    let outcome = send_mutation(
        &h1,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"code","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack < TERM_TIMEOUT), got {outcome:?}"
    );
    let parked = ram_entry(&h1, "/code")
        .await
        .expect("/code entry must STAY per No-Delete");
    assert!(!parked.active, "/code must be inactive after disconnect");
    assert_eq!(
        parked.cell_id, cell_id_before,
        "cell_id must be stable across disconnect (No-Delete)"
    );

    // ── REBOOT: shut down colony1, start colony2 from the SAME root. ─────────
    h1.shutdown().await;

    let factory2: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h2 = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory2)]);

    // Anti-cascade for the reboot: register the NEW /sink BEFORE bootstrap.
    let (sink_tx2, mut sink_rx2) = mpsc::channel::<Message>(8);
    h2.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx2.clone())
    })
    .await;
    // Boot 2 re-hydrates /code from disk (main/code/config.json), so the
    // bootstrap registry needs the `code` factory in addition to `echo`.
    let mut boot2_registry = echo_registry();
    boot2_registry.insert(
        "code".into(),
        Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &boot2_registry, &h2.runtime())
        .await
        .expect("boot2 (reboot) bootstrap must succeed");

    // (1) /code rehydrated INACTIVE (its edge was removed before shutdown); the
    // boot path called `build_boot_inactive_respawn` (half B) → a REAL respawn
    // hook + eager_on_reconnect=true. cell_id is STABLE across the reboot.
    let post_reboot = ram_entry(&h2, "/code")
        .await
        .expect("/code rehydrated after reboot (from colony.db)");
    assert!(
        !post_reboot.active,
        "/code must be inactive after reboot (edge-less, parked status from colony.db)"
    );
    assert_eq!(
        post_reboot.cell_id, cell_id_before,
        "cell_id must be STABLE across the reboot (rehydrated from colony.db)"
    );

    // (2) Reconnect after reboot: add_edges /code -> /sink → recompute reactivates
    // /code → the reconnect-eager arm fires the boot-inactive respawn hook →
    // IMMEDIATE re-spawn of the stateless dispatcher (no further reboot needed).
    // This is the exact in-process Demo (b) behaviour, now across a reboot.
    let outcome = send_mutation(
        &h2,
        json!({"scope":"/","diff":{"add_edges":[{"from":"code","to":"sink"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "post-reboot reconnect must commit, got {outcome:?}"
    );
    let reconnected = ram_entry(&h2, "/code")
        .await
        .expect("/code present after post-reboot reconnect");
    assert!(
        reconnected.active,
        "/code must be active again after the post-reboot reconnect (eager respawn)"
    );
    assert_eq!(
        reconnected.cell_id, cell_id_before,
        "cell_id must be STABLE across reboot + reconnect (never re-created)"
    );

    // Positive liveness proof: an origin-probe sent AFTER the post-reboot
    // reconnect produces an emission at /sink ⟹ the eager-respawned /code is live
    // after reboot+reconnect. The sink receipt is the barrier (30 s marker).
    h2.send_from(Path::new("/"), probe_to_reply_sink("/code"))
        .await;
    let receipt = tokio::time::timeout(Duration::from_secs(30), sink_rx2.recv())
        .await
        .expect("30s failure-marker: reconnected post-reboot /code never emitted to /sink")
        .expect("/sink tap closed (boot2)");
    assert!(
        receipt.headers.hop.get("error_code").is_none(),
        "post-reboot-reconnect emission must be a regular receipt, not an error; headers: {:?}",
        receipt.headers.hop
    );

    h2.shutdown().await;
}
