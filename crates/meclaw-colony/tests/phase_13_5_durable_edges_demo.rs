//! Phase-13.5 Durable-Edges Demo (Task 7): CEL edges added via **mutation**
//! survive a colony reboot.
//!
//! This closes the spec hole in PROGRESS.md § "Edge-Persistenz": before this
//! slice, mutation-added CEL edges were rehydrated as identity edges after a
//! reboot, silently corrupting routing. Bootstrap edges are NOT sufficient to
//! prove the fix — every test here adds the edge via an `add_edges` mutation,
//! reboots, and shows the `condition`/`modifier` still applies.
//!
//! Topology:
//!   td/main/config.json       — root hive (persists the hive_scope `/`, so a
//!                               populated colony.db classifies as Reboot, not
//!                               Inconsistent — boot_state needs all three
//!                               tables non-empty).
//!   td/main/a/config.json     — `/a`: echo cell, `echo_to = /a` (self-loop).
//!                               The mutation edge /a -> /sink overrides the
//!                               emit target when it fires; the self-loop is
//!                               only the no-match fallback, and probes carry a
//!                               low `ttl` so a no-match hop dies within a
//!                               couple of hops (bounded — never an infinite
//!                               cascade).
//!   /sink                     — CaptureCell (registry-only via `h.spawn`), the
//!                               terminal positive-receipt signal.
//!
//! Anti-Cascade discipline (Phase-6.5 lesson): /sink is registered via
//! `h.spawn(...)` BEFORE bootstrap so the edge target is resolved on /a's first
//! emit. On reboot, the in-memory registry resets to empty and bootstrap is NOT
//! re-run (the DB is authoritative), so /a + /sink are re-registered via
//! `h.spawn`. The persisted EdgeTable is what carries the CEL across the reboot
//! — that is exactly what we are proving.
//!
//! Demo discipline: positive proof is a POSITIVE capture receipt on /sink;
//! "not received" is a BOUNDED timeout (never an unbounded `recv` that hangs).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, spawn_colony_task_at};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// `(name, factory)` list with `echo` registered — required because a rebooted
/// colony opens `colony.db` with the same factory registry as the first boot.
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

/// Write the FS topology under `td/main`: a root hive (so the hive_scope `/`
/// gets persisted) + `/a` as a self-echoing source cell.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    // Echoes to itself; the mutation edge /a -> /sink overrides the target when
    // it fires, otherwise the self-loop dies via the probe's low ttl.
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Register /sink (CaptureCell) BEFORE bootstrap (anti-cascade), then run
/// bootstrap so the root hive_scope + `/a` are persisted. Returns the /sink
/// capture receiver. Used for the first boot.
async fn boot1_topology(h: &ColonyHandle, td: &TempDir) -> mpsc::Receiver<Message> {
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");
    sink_rx
}

/// Re-register /sink + /a after a reboot (registry status resets; bootstrap is
/// NOT re-run — the persisted EdgeTable carries the CEL). Returns the fresh
/// /sink capture receiver.
async fn reboot_topology(h: &ColonyHandle) -> mpsc::Receiver<Message> {
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/a"), || {
        EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/a"))
    })
    .await;
    sink_rx
}

/// Send a mutation and await the outcome (copied from
/// `phase_11_contract_via_mutation.rs`).
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

/// Build a probe to /a carrying the given headers in `context`. The probe is a
/// Source — it establishes the survive-keys in `context` (Ingress-at-birth) so
/// they survive the /a echo-cell hop (input.hop decays on cell emission, only
/// context travels through — `carry_context_with_hop`). The edge condition /
/// modifier — evaluated at /a's emit — then reads/writes `context.*`. Low ttl
/// bounds the no-match self-loop so a "not received" case dies in the DLQ within
/// a couple of hops.
fn probe_to_a(headers: Map<String, Value>) -> Message {
    MessageBuilder::new(Path::new("/a"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .context(headers)
        .ttl(4)
        .build()
}

/// One header entry shorthand.
fn headers_with(pairs: &[(&str, &str)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), json!(*v));
    }
    m
}

/// Bounded wait for a /sink receipt. Returns `Some(msg)` if received within
/// the timeout, `None` on timeout (the bounded "not received" signal).
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Assert /sink does NOT receive within a bounded window (no unbounded recv).
async fn assert_not_received(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: /sink must NOT receive (condition/modifier guard), got {got:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Step 1 — condition edge survives reboot.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_durable_edges_cel_condition_survives_reboot() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1 ──────────────────────────────────────────────────────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let mut sink_rx = boot1_topology(&h, &td).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {"from": "a", "to": "sink", "condition": "context.kind == 'text'"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges mutation must commit, got {outcome:?}"
    );

    // Positive: kind=text → edge fires → /sink receives.
    h.send(probe_to_a(headers_with(&[("kind", "text")]))).await;
    let got = recv_bounded(&mut sink_rx).await;
    assert!(
        got.is_some(),
        "Boot 1: kind=text must route to /sink via CEL condition"
    );
    // Negative: kind=other → edge condition false → /sink must NOT receive.
    h.send(probe_to_a(headers_with(&[("kind", "other")]))).await;
    assert_not_received(&mut sink_rx, "Boot 1 kind=other").await;

    h.shutdown().await;

    // ── Boot 2 (reboot at the SAME directory) ───────────────────────────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let mut sink_rx2 = reboot_topology(&h2).await;

    // PROOF: condition survived the reboot — NOT silently dropped to identity.
    h2.send(probe_to_a(headers_with(&[("kind", "text")]))).await;
    let got = recv_bounded(&mut sink_rx2).await;
    assert!(
        got.is_some(),
        "Boot 2: kind=text must route to /sink — condition survived reboot"
    );
    h2.send(probe_to_a(headers_with(&[("kind", "other")])))
        .await;
    assert_not_received(&mut sink_rx2, "Boot 2 kind=other").await;

    h2.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Step 2 — modifier edge (set + delete) survives reboot.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_durable_edges_cel_modifier_survives_reboot() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // Probe carries `removed_field` (to be deleted) + `keep_me` (intact).
    let probe_headers = || headers_with(&[("removed_field", "gone"), ("keep_me", "stay")]);

    // ── Boot 1 ──────────────────────────────────────────────────────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let mut sink_rx = boot1_topology(&h, &td).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {
                        "from": "a",
                        "to": "sink",
                        "modifier": {
                            "set_hop": {"injected_tag": "'v1'"},
                            "delete_context": ["removed_field"]
                        }
                    }
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges modifier mutation must commit, got {outcome:?}"
    );

    h.send(probe_to_a(probe_headers())).await;
    let msg = recv_bounded(&mut sink_rx)
        .await
        .expect("Boot 1: modifier edge must route to /sink");
    assert_modifier_applied(&msg, "Boot 1");

    h.shutdown().await;

    // ── Boot 2 (reboot) ─────────────────────────────────────────────────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let mut sink_rx2 = reboot_topology(&h2).await;

    // PROOF: modifier set + delete both still active after reboot.
    h2.send(probe_to_a(probe_headers())).await;
    let msg = recv_bounded(&mut sink_rx2)
        .await
        .expect("Boot 2: modifier edge must route to /sink — modifier survived reboot");
    assert_modifier_applied(&msg, "Boot 2");

    h2.shutdown().await;
}

/// Assert the modifier's set + delete both took effect on the received headers.
///
/// `injected_tag` is set on `hop` by the edge — there is no cell between the
/// edge and /sink, so an immediate hop write is the correct surface. The
/// survive-keys (`removed_field`/`keep_me`) live in `context` because they had
/// to outlive the /a Echo-Cell hop: `delete_context` drops `removed_field`,
/// the untouched `keep_me` rides `context` through to /sink.
fn assert_modifier_applied(msg: &Message, ctx: &str) {
    assert_eq!(
        msg.headers.hop.get("injected_tag"),
        Some(&Value::String("v1".into())),
        "{ctx}: modifier.set_hop injected_tag=v1 must be present; got {:?}",
        msg.headers.hop
    );
    assert!(
        msg.headers.context.get("removed_field").is_none(),
        "{ctx}: delete_context must drop removed_field; got {:?}",
        msg.headers.context
    );
    assert_eq!(
        msg.headers.context.get("keep_me"),
        Some(&Value::String("stay".into())),
        "{ctx}: untouched context headers must survive the cell hop; got {:?}",
        msg.headers.context
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Steps 3 & 4 — reboot HARD-FAILS on corrupt persisted CEL.
//
// After Boot 1 + mutation-add-edge + shutdown, corrupt the persisted edge
// directly in colony.db, then reboot via a raw `colony_task` JoinHandle. The
// boot-init panics (no `catch_unwind`); the panic surfaces as a JoinError.
// ───────────────────────────────────────────────────────────────────────────

/// Boot 1 + add a CEL edge via mutation + graceful shutdown. Leaves a populated
/// colony.db at `td.path()`. Returns nothing — the directory is the artifact.
async fn boot1_with_cel_edge(td: &TempDir, edge_diff: Value) {
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    let _sink_rx = boot1_topology(&h, td).await;
    let outcome = send_mutation(&h, json!({"scope": "/", "diff": edge_diff})).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "setup add_edges must commit, got {outcome:?}"
    );
    h.shutdown().await;
}

/// Await the rebooted colony_task JoinHandle and assert it panicked with a
/// message containing `needle`.
async fn assert_reboot_panics_with(td: &TempDir, needle: &str) {
    let join = spawn_colony_task_at(td.path(), echo_factories());
    let err = join
        .await
        .expect_err("corrupt persisted CEL must hard-fail the boot");
    assert!(err.is_panic(), "boot failure must be a panic, got {err:?}");
    let msg = err.into_panic();
    let s = msg
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| msg.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(s.contains(needle), "panic msg was: {s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_durable_edges_reboot_hard_fails_on_corrupt_condition() {
    let td = TempDir::new().unwrap();
    boot1_with_cel_edge(
        &td,
        json!({
            "add_edges": [{"from": "a", "to": "sink", "condition": "hop.kind == 'text'"}]
        }),
    )
    .await;

    // Corrupt the persisted condition source (the mutation generated a UUID id;
    // there is exactly one edge, so SELECT id needs no filter).
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let id: String = conn
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "UPDATE edges SET condition = 'not valid cel ++' WHERE id = ?",
        [&id],
    )
    .unwrap();
    drop(conn);

    assert_reboot_panics_with(&td, "ConditionParseFailed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_durable_edges_reboot_hard_fails_on_corrupt_modifier_json() {
    let td = TempDir::new().unwrap();
    boot1_with_cel_edge(
        &td,
        json!({
            "add_edges": [{
                "from": "a", "to": "sink",
                "modifier": {"set_hop": {"injected_tag": "'v1'"}}
            }]
        }),
    )
    .await;

    // Corrupt the persisted modifier into invalid JSON.
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let id: String = conn
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        r#"UPDATE edges SET modifier = '{"set": invalid' WHERE id = ?"#,
        [&id],
    )
    .unwrap();
    drop(conn);

    assert_reboot_panics_with(&td, "ModifierJsonInvalid").await;
}
