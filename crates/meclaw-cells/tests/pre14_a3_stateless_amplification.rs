//! Pre-14 Pass-2 backstop A3 — stateless-dispatcher amplification + supervision.
//!
//! Substrate contract (docs § Stateless-Cell-Dispatcher, CLAUDE.md Phase-7
//! lesson): `stateless_dispatcher` is the supervised actor task; it spawns ONE
//! ephemeral detached worker per message (capped at `params.max_concurrency`).
//! A worker panic ends silently with its message and is NOT seen by the
//! supervisor — the ONLY supervised unit is the dispatcher task. If the
//! dispatcher itself dies (panic/backstop), the supervisor restarts it.
//!
//! This backstop pins three facets that no single existing test pins together:
//!   * (a) AMPLIFICATION: N messages → up to `max_concurrency` concurrent workers,
//!     never more, and ALL N run. (A deterministic, non-flaky counterpart to the
//!     quarantined `cell_task` concurrency unit test.)
//!   * (b) WORKER-PANIC ISOLATION: a panicking worker does NOT kill the
//!     dispatcher — a follow-up message is still served.
//!   * (c) DISPATCHER RESTART: when the dispatcher task dies, the colony
//!     supervisor restarts it (real `RespawnFn`), and the restarted dispatcher
//!     serves messages again.
//!
//! (a)+(b) drive `stateless_dispatcher` directly (no colony) for deterministic
//! rendezvous; (c) drives the real colony supervisor.

use meclaw_colony::{
    ColonyMsg, DeathKind, RespawnFn, SpawnedCellKind, StatelessCell, stateless_dispatcher,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, CellOutput, Message, MessageBuilder, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};

fn probe(target: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(body))
        .build()
}

// ── (a) amplification — peak concurrency caps at max_concurrency, all N run ───

/// Stateless cell instrumented for the amplification rendezvous: on entry it
/// bumps a live counter (tracking the running peak), signals `entered`, then
/// blocks on `gate` until the test releases it; on exit it bumps `completed`.
struct AmpCell {
    live: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    gate: Arc<Semaphore>,
    completed: Arc<Semaphore>,
}

impl StatelessCell for AmpCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        _msg: Message,
        _sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.entered.add_permits(1);
            if let Ok(p) = self.gate.acquire().await {
                p.forget();
            }
            self.live.fetch_sub(1, Ordering::SeqCst);
            self.completed.add_permits(1);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn n_messages_amplify_to_max_concurrency_workers_and_all_run() {
    const MAX: usize = 3;
    const N: usize = 7;

    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let completed = Arc::new(Semaphore::new(0));

    let cell = Arc::new(AmpCell {
        live: live.clone(),
        peak: peak.clone(),
        entered: entered.clone(),
        gate: gate.clone(),
        completed: completed.clone(),
    });

    let (mbx_tx, mbx_rx) = mpsc::channel::<Message>(64);
    let (outputs_tx, _outputs_rx) = mpsc::channel::<CellEmission>(4);
    let dispatcher = tokio::spawn(stateless_dispatcher(
        Path::new("/disp"),
        mbx_rx,
        outputs_tx,
        cell,
        MAX,
        None, // message_timeout
        None, // peace_tx
        None, // stop_rx
        None, // colony_inbox_tx
        None, // death_ack
        None, // blob_store
        None,
    ));

    // Inject N messages — more than MAX, so the dispatcher must queue the surplus.
    for _ in 0..N {
        mbx_tx
            .send(probe("/disp", json!({"messages": []})))
            .await
            .unwrap();
    }

    // Wait until MAX workers are concurrently in-flight (all blocked on the gate).
    tokio::time::timeout(Duration::from_secs(30), entered.acquire_many(MAX as u32))
        .await
        .expect("30s failure-marker: fewer than MAX workers ever entered")
        .expect("entered semaphore closed")
        .forget();

    // CAP: with MAX workers parked on the gate, the dispatcher's semaphore is
    // exhausted → NO further worker may enter until one releases. The peak the
    // counter saw is exactly MAX — amplification reached the cap, never exceeded.
    assert_eq!(
        live.load(Ordering::SeqCst),
        MAX,
        "exactly max_concurrency workers run concurrently"
    );
    assert_eq!(
        entered.available_permits(),
        0,
        "no (MAX+1)th worker may enter while the cap is saturated"
    );
    assert_eq!(peak.load(Ordering::SeqCst), MAX, "peak concurrency == cap");

    // Release everyone → all N workers run to completion (amplification: none lost).
    gate.add_permits(N);
    tokio::time::timeout(Duration::from_secs(30), completed.acquire_many(N as u32))
        .await
        .expect("30s failure-marker: not all N workers completed")
        .expect("completed semaphore closed")
        .forget();

    assert_eq!(
        peak.load(Ordering::SeqCst),
        MAX,
        "peak NEVER exceeded the cap across the whole run"
    );

    // Close the mailbox → the dispatcher drains and exits cleanly.
    drop(mbx_tx);
    tokio::time::timeout(Duration::from_secs(30), dispatcher)
        .await
        .expect("30s failure-marker: dispatcher did not exit after mailbox close")
        .expect("dispatcher join");
}

// ── (b) worker-panic isolation — a panicking worker does not kill the dispatcher ─

/// Stateless cell that PANICS when the message body carries `{"panic": true}`,
/// otherwise signals completion on `served`. The panic unwinds in the detached
/// worker task — the dispatcher must keep serving.
struct PanicOrServeCell {
    served: Arc<Semaphore>,
}

impl StatelessCell for PanicOrServeCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        msg: Message,
        _sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let should_panic = matches!(&msg.body, Body::Inline(Value::Object(m))
                if m.get("panic").and_then(|v| v.as_bool()).unwrap_or(false));
            if should_panic {
                panic!("intentional worker panic (isolation test)");
            }
            self.served.add_permits(1);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_panic_does_not_kill_dispatcher() {
    let served = Arc::new(Semaphore::new(0));
    let cell = Arc::new(PanicOrServeCell {
        served: served.clone(),
    });

    let (mbx_tx, mbx_rx) = mpsc::channel::<Message>(8);
    let (outputs_tx, _outputs_rx) = mpsc::channel::<CellEmission>(4);
    let dispatcher = tokio::spawn(stateless_dispatcher(
        Path::new("/disp"),
        mbx_rx,
        outputs_tx,
        cell,
        4,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    // First message PANICS its worker (detached → isolated, not joined here).
    mbx_tx
        .send(probe("/disp", json!({"messages": [], "panic": true})))
        .await
        .unwrap();

    // Follow-up message must still be served → the dispatcher survived the panic.
    mbx_tx
        .send(probe("/disp", json!({"messages": []})))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), served.acquire())
        .await
        .expect("30s failure-marker: dispatcher did NOT serve the follow-up — it died with the worker panic")
        .expect("served semaphore closed")
        .forget();

    // And it keeps serving a third message too (still alive, not wedged).
    mbx_tx
        .send(probe("/disp", json!({"messages": []})))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), served.acquire())
        .await
        .expect("30s failure-marker: dispatcher stopped serving after the panic")
        .expect("served semaphore closed")
        .forget();

    drop(mbx_tx);
    let _ = tokio::time::timeout(Duration::from_secs(30), dispatcher).await;
}

// ── (c) dispatcher death → supervisor restarts it ────────────────────────────
//
// The dispatcher is robust by design (worker panics + backstops do NOT kill it),
// so a genuine dispatcher death is only reachable via an actual task panic, which
// production code never does from cell logic. We therefore inject the death the
// watcher WOULD emit on a real dispatcher panic — `ColonyMsg::CellDied{Panic}` —
// for a dispatcher registered with a REAL `RespawnFn`, and assert the colony
// supervisor restarts it into a live dispatcher. (The watcher→`CellDied`-on-panic
// emission is separately unit-pinned in `spawn_watcher` tests; the
// `handle_cell_died{Panic}→respawn` restart in `handle_cell_died_panic_restarts`.
// This composes both halves for a real stateless dispatcher end-to-end.)

/// Stateless cell that emits one fixed UBF message to `/sink` per message — the
/// positive liveness signal (a `/sink` receipt ⟺ the dispatcher is live).
struct EmitToSinkCell;

impl StatelessCell for EmitToSinkCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        _msg: Message,
        sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/sink"),
                    content: json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": "alive"}]
                    }),
                })
                .await;
        }
    }
}

/// Spawn a fresh `EmitToSinkCell` dispatcher and return the Active wiring. The
/// `RespawnFn` re-invokes this same builder, so the colony supervisor can restart
/// the dispatcher on death (Phase-5 corridor: the respawn closure is await-free).
/// `spawn_count` is bumped synchronously on every (re)spawn — the deterministic
/// proof that the supervisor actually called the `RespawnFn` (without it, the
/// still-alive old task could serve the post-death probe and mask a no-op).
fn spawn_emit_dispatcher(
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<ColonyMsg>,
    spawn_count: Arc<AtomicUsize>,
) -> SpawnedCellKind {
    let (tx, rx) = mpsc::channel::<Message>(64);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let cell = Arc::new(EmitToSinkCell);
    let disp_outputs = outputs_tx.clone();
    let disp_inbox = colony_inbox_tx.clone();
    let disp_path = path.clone();
    spawn_count.fetch_add(1, Ordering::SeqCst);
    let join = tokio::spawn(async move {
        stateless_dispatcher(
            disp_path,
            rx,
            disp_outputs,
            cell,
            4,
            None,
            Some(peace_tx),
            Some(stop_rx),
            Some(disp_inbox),
            Some(death_ack_tx),
            None,
            None,
        )
        .await;
    });

    // RespawnFn: re-spawn the dispatcher with fresh channels. Restarted cells lose
    // the external stop wiring (not needed here — the test only probes liveness).
    let respawn: RespawnFn = Box::new(move || {
        spawn_count.fetch_add(1, Ordering::SeqCst);
        let (n_tx, n_rx) = mpsc::channel::<Message>(64);
        let (_n_stop_tx, n_stop_rx) = oneshot::channel::<()>();
        let (n_death_ack_tx, _n_death_ack_rx) = oneshot::channel::<()>();
        let (n_peace_tx, n_peace_rx) = oneshot::channel::<()>();
        let (_n_backstop_tx, n_backstop_rx) = oneshot::channel::<()>();
        let n_cell = Arc::new(EmitToSinkCell);
        let n_outputs = outputs_tx.clone();
        let n_inbox = colony_inbox_tx.clone();
        let n_path = path.clone();
        let n_join = tokio::spawn(async move {
            stateless_dispatcher(
                n_path,
                n_rx,
                n_outputs,
                n_cell,
                4,
                None,
                Some(n_peace_tx),
                Some(n_stop_rx),
                Some(n_inbox),
                Some(n_death_ack_tx),
                None,
                None,
            )
            .await;
        });
        (n_tx, n_join, n_peace_rx, n_backstop_rx)
    });

    SpawnedCellKind::Active {
        sender: tx,
        join,
        peace_rx,
        stop_tx,
        death_ack_rx,
        backstop_rx,
        respawn,
    }
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>, what: &str) {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("30s failure-marker: {what}"))
        .unwrap_or_else(|| panic!("/sink tap closed: {what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatcher_death_is_restarted_by_supervisor() {
    let h = ColonyHandle::new();

    // /sink BEFORE the probe (anti-cascade): terminal CaptureCell for receipts.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Register /disp as an Active stateless dispatcher with a REAL respawn.
    let spawn_count = Arc::new(AtomicUsize::new(0));
    let disp = spawn_emit_dispatcher(
        Path::new("/disp"),
        h.runtime().outputs_tx,
        h.inbox_tx.clone(),
        spawn_count.clone(),
    );
    h.register_spawned(Path::new("/disp"), disp).await;
    // W2 (A1): /disp emission to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/disp"),
        Path::new("/sink"),
    )
    .await;
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        1,
        "dispatcher spawned once at registration"
    );

    // Baseline liveness: a probe to /disp emits to /sink.
    h.send_from(Path::new("/"), probe("/disp", json!({"messages": []})))
        .await;
    recv_bounded(&mut sink_rx, "baseline /disp emission never reached /sink").await;

    // Inject the dispatcher death the watcher would emit on a real panic.
    h.inbox_tx
        .send(ColonyMsg::CellDied {
            path: Path::new("/disp"),
            death_kind: DeathKind::Panic,
        })
        .await
        .expect("colony inbox closed");

    // After the supervisor restart, /disp serves again: a fresh probe emits to
    // /sink → positive receipt ⟹ the restarted dispatcher is live.
    h.send_from(Path::new("/"), probe("/disp", json!({"messages": []})))
        .await;
    recv_bounded(
        &mut sink_rx,
        "restarted /disp never emitted to /sink — supervisor did not restart the dispatcher",
    )
    .await;

    // The receipt above is served by the NEW dispatcher (the colony processes the
    // CellDied — swapping in the respawned sender — before the FIFO-later Route).
    // `spawn_count == 2` is the unambiguous proof the supervisor actually invoked
    // the RespawnFn (a no-op restart would leave it at 1, served by the old task).
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        2,
        "supervisor must have respawned the dispatcher exactly once after its death"
    );

    h.shutdown().await;
}
