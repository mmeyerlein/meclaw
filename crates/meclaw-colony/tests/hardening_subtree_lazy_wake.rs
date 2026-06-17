//! F1-KH2 hardening: mutation-instantiated lazy/stateful SUBTREE cells must
//! wake on first delivery (Hot/Cold model), exactly like single-cell
//! `add_nodes` and the bootstrap path.
//!
//! Finding (ruling):
//! the subtree registration path (colony.rs apply-Schritt 9c) installed an
//! INERT WakeFn for EVERY subtree cell — a pre-R12 assumption ("subtree stays
//! inactive until a later add_edges; routing dead-letters it") that R12's
//! same-mutation activation invalidated. First routed delivery to a lazy
//! (Dormant-kind) subtree cell invoked the inert wake: the parked mailbox
//! receiver was dropped (message silently lost), the status flipped to a false
//! `Awake` with NO task, and every later delivery vanished without a DLQ
//! trace. Severity class: silent loss + false-positive liveness.
//!
//! These tests pin the fixed contract POSITIVELY (CaptureCell receipt ⟺ the
//! lazy cells actually ran — CLAUDE.md demo discipline):
//! - (a) K-H2 repro shape: ONE mutation (`add_nodes` subtree with two lazy
//!   cells + activating `add_edges` port) → first delivery arrives end-to-end,
//!   status is truthful, DLQ empty;
//! - (b) follow-up delivery arrives too (the woken task stays live);
//! - (d) boot-path counter-probe: a boot-instantiated ACTIVE lazy cell wakes
//!   on first message (never affected — pinned as regression guard);
//! - (e) inventory finding #3 (same pre-R12 class): a boot-INACTIVE lazy cell
//!   (persisted `inactive`, rebooted, then reconnected via `add_edges`) must
//!   wake on first message — the bootstrap `register_inactive_non_spawned`
//!   path used to install the same inert wake.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers (mirror the paket_5_p9_demo harness)
// ──────────────────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
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

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    })
}

/// Factory list for `ColonyHandle::new_with_factories_at` (mutation path).
fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![("persist_mock".to_string(), persist_factory())]
}

/// Factory registry for `bootstrap_from_filesystem` (boot path).
fn factory_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("persist_mock".into(), persist_factory());
    r
}

/// Write the SUBTREE template `unit`: root hive (internal edge ./s -> ./e) plus
/// TWO lazy stateful cells (`persist_mock` — Dormant kind, no
/// `build_boot_inactive_respawn` hook), mirroring the K-H2 llm+store shape.
/// `s` forwards to `e` (matching the internal edge), `e` forwards to the
/// external `/capture` sink.
fn write_unit_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("unit");
    write(&tpl, "template.json", r#"{"name":"unit"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./s","to":"./e"}]}}}"#,
    );
    write(
        &tpl,
        "s/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/u1/e"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "e/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A probe addressed at `target` carrying an origin-format UBF body.
fn probe_to(target: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(8)
        .build()
}

/// Bounded wait for a receipt (30s failure-marker convention).
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Set up the K-H2 repro colony: root hive, `unit` template, /capture sink,
/// /anchor entry, then ONE mutation (`add_nodes` subtree + activating
/// `add_edges` port onto the lazy cell — the R12 one-mutation form).
async fn setup_kh2_colony(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_unit_template(td.path());

    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let (cap_tx, cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    // K-H2 one-mutation form: subtree instantiation + activating port edge in
    // the SAME diff (R12 same-mutation activation — deep endpoint `u1/s`).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"u1","template":"unit"}],
            // W2b (Ruling A1): the terminal lazy cell /u1/e echoes to /capture —
            // that hop needs a wired catch-all out-edge now (identity-fallback gone).
            // /u1/e is a post_state subtree node (R12 deep resolution), /capture is
            // live (spawned above). The internal s->e edge ships with the template.
            "add_edges":[{"from":"anchor","to":"u1/s"},{"from":"u1/e","to":"capture"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "one-mutation subtree apply must commit; got {outcome:?}"
    );

    // Hot/Cold form after the apply (K-H2 Gate 3): lazy cells are active but
    // parked NotYetSpawned — wake-on-message.
    for p in ["/u1/s", "/u1/e"] {
        let e = registry_entry(&h, p).await.unwrap();
        assert!(e.active, "{p} must be active after the one-mutation apply");
        assert_eq!(
            e.lifecycle_status, "NotYetSpawned",
            "{p} must be parked NotYetSpawned (hot/cold) before first delivery"
        );
    }
    (h, cap_rx)
}

// ──────────────────────────────────────────────────────────────────────────────
// (a) K-H2 repro: first delivery to a mutation-instantiated lazy subtree cell
// ──────────────────────────────────────────────────────────────────────────────

/// FIRST routed delivery to the lazy subtree cell `/u1/s` must ARRIVE: the
/// cell wakes (real Hot/Cold wake), processes, and the emission chains through
/// the internal edge to the second lazy cell and out to `/capture` (positive
/// receipt ⟺ BOTH lazy cells ran). Status is truthful (`Awake` WITH a task)
/// and the DLQ stays empty — no silent loss, no false-positive liveness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_subtree_lazy_cell_first_delivery_arrives() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = setup_kh2_colony(&td).await;

    h.send(probe_to("/u1/s")).await;
    let got = recv_bounded(&mut cap_rx).await;
    assert!(
        got.is_some(),
        "first delivery must wake /u1/s and chain s->e->/capture \
         (K-H2 F1: message was silently lost on the inert subtree WakeFn)"
    );

    // Status truth: Awake AND alive (the receipt above proves the task runs).
    let s = registry_entry(&h, "/u1/s").await.unwrap();
    assert_eq!(
        s.lifecycle_status, "Awake",
        "/u1/s must be truthfully Awake after the first delivery"
    );

    // No silent loss anywhere: DLQ empty.
    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "DLQ must be empty after a clean wake round; got {dls:?}"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// (b) follow-up delivery: the woken subtree cell stays live
// ──────────────────────────────────────────────────────────────────────────────

/// A SECOND probe after the wake round must arrive too — pre-fix, every
/// delivery after the first vanished into the dropped throwaway mailbox
/// (unbounded loss surface, one-shot error trace).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_subtree_lazy_cell_follow_up_delivery_arrives() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = setup_kh2_colony(&td).await;

    h.send(probe_to("/u1/s")).await;
    assert!(
        recv_bounded(&mut cap_rx).await.is_some(),
        "first delivery must arrive (wake round)"
    );

    h.send(probe_to("/u1/s")).await;
    assert!(
        recv_bounded(&mut cap_rx).await.is_some(),
        "follow-up delivery must arrive (woken task stays live; pre-fix it \
         vanished spurlessly into the dropped throwaway mailbox)"
    );

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "DLQ must be empty after two clean rounds; got {dls:?}"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// (d) boot-path counter-probe: boot-instantiated ACTIVE lazy cells were never
//     affected — pin it
// ──────────────────────────────────────────────────────────────────────────────

/// Write the BOOT tree: root hive `main` with edges ./a -> ./s and ./w -> ./v
/// (all lazy cells). `s` forwards to `/capture`; the unrelated `w -> v` pair
/// keeps the persisted edge table non-empty when test (e) disconnects `s`
/// (the boot-state heuristic classifies registry>0 + edges==0 as
/// Inconsistent — out of scope here).
fn write_boot_tree(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./s"},{"from":"./w","to":"./v"}]}}}"#,
    );
    write(
        root,
        "main/a/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/s"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/s/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/w/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/v"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/v/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/v"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Boot path (bootstrap_apply ACTIVE lazy registration): first message to a
/// boot-instantiated lazy cell wakes it via the REAL factory wake. This was
/// never broken (the K-H2 workaround was exactly "reboot — the boot path
/// registers the real wake"); pinned here as the counter-probe.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_active_lazy_cell_wakes_on_first_message() {
    let td = tempfile::TempDir::new().unwrap();
    write_boot_tree(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    let s = registry_entry(&h, "/s").await.unwrap();
    assert!(s.active, "/s must boot active (edge ./a -> ./s)");
    assert_eq!(
        s.lifecycle_status, "NotYetSpawned",
        "/s must boot parked NotYetSpawned (lazy kind)"
    );

    // W2b (Ruling A1): /s echoes to /capture — wire that hop's catch-all out-edge
    // (identity-fallback gone). /s is boot-active, /capture is live; both endpoints
    // resolve, so it goes via add_edges (the shared write_boot_tree can't carry a
    // boot edge to /capture — test (e)'s boot 1 doesn't spawn /capture → A8 fail).
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"s","to":"capture"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /s -> /capture must commit"
    );

    h.send(probe_to("/s")).await;
    assert!(
        recv_bounded(&mut cap_rx).await.is_some(),
        "boot-instantiated lazy cell must wake on first message (boot path \
         was never affected — regression pin)"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// (e) inventory finding #3: boot-INACTIVE lazy cell + add_edges reconnect
// ──────────────────────────────────────────────────────────────────────────────

/// Same pre-R12 assumption class on the BOOTSTRAP path
/// (`register_inactive_non_spawned`): a lazy cell persisted `inactive`
/// (disconnect → reboot) reconnects via `add_edges` as wake-on-message — the
/// boot-inactive registration must therefore install the REAL factory wake,
/// not the inert one. First delivery after the reconnect must arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_inactive_lazy_cell_reconnect_wakes_on_message() {
    let td = tempfile::TempDir::new().unwrap();
    write_boot_tree(td.path());

    // ── Boot 1: tree active; disconnect /s (remove its only edge) → persisted
    //    `inactive`. ──
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime())
        .await
        .expect("boot 1 must succeed");
    assert!(registry_entry(&h, "/s").await.unwrap().active);
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"a","to":"s"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit; got {outcome:?}"
    );
    assert!(
        !registry_entry(&h, "/s").await.unwrap().active,
        "/s must be inactive after the disconnect"
    );
    h.shutdown().await; // flushes colony.db (persisted status `inactive`)

    // ── Boot 2: /s rehydrates INACTIVE (register_inactive_non_spawned), then
    //    reconnects via add_edges → wake-on-message. ──
    let h2 = ColonyHandle::new_with_factories_at(&td, factories());
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h2.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    h2.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;
    bootstrap_from_filesystem(td.path(), &factory_registry(), &h2.runtime())
        .await
        .expect("boot 2 must succeed");
    let s = registry_entry(&h2, "/s").await.unwrap();
    assert!(!s.active, "/s must rehydrate inactive on boot 2");
    assert_eq!(s.lifecycle_status, "NotYetSpawned");

    let outcome = send_mutation(
        &h2,
        // W2b (Ruling A1): reconnect anchor->s AND wire s->/capture in the same
        // mutation — /s's echo to /capture needs the catch-all out-edge now.
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"s"},{"from":"s","to":"capture"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "reconnect must commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h2, "/s").await.unwrap().active,
        "/s must be active after the reconnect"
    );

    h2.send(probe_to("/s")).await;
    assert!(
        recv_bounded(&mut cap_rx).await.is_some(),
        "first delivery after a boot-inactive reconnect must wake the lazy \
         cell (inventory finding #3: the boot-inactive path installed the \
         same inert wake)"
    );

    h2.shutdown().await;
}
