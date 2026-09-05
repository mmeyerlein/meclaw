//! GH #553 — the mutation door leaves a receipt, and the boot is the first one.
//!
//! A committed mutation used to be an event only its own caller could see: the
//! verdict travels back on `reply_to`, and `--apply` and `POST
//! /colony/mutations` set no `reply_to` at all. Everything else in the tree that
//! wanted to know "the graph moved" had to ask on a timer, which is a poll in an
//! event-driven substrate — availability spent on a question that already had an
//! answer.
//!
//! What is built here: the door itself emits ONE terminal message per committed
//! knock, addressed to a hive named in `colony.json`
//! (`mutation_receipts.to`). Because the target is a hive the message enters as
//! a HiveTransit, so the hive's own `{"from": "."}` edges fan it out — the
//! substrate hands over an event, the topology decides who hears it.
//!
//! Four properties are pinned:
//!
//! - **Opt-in.** No `mutation_receipts` entry, no receipt, nothing else changes.
//! - **Committed means committed.** A refusal leaves no receipt.
//! - **The boot is the first receipt.** After `InitialApply` the colony emits
//!   exactly one `form: "boot"` receipt, so a restart fills a menu and a screen
//!   without waiting for the first mutation of the day.
//! - **No feedback.** A receipt is not a mutation, so `n` mutations plus the
//!   boot leave exactly `n + 1` receipts — never a cascade.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, MutationOutcome,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::wait::wait_for_message_log_count;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let dir = root.join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), body).unwrap();
}

/// The colony under test: a hive `/hive` whose single out-edge carries the
/// receipt lane down to `/hive/capture`, plus two ordinary cells `/a` and `/b`
/// for the edge mutations to move.
///
/// The condition on the hive edge is the point of the fixture: it is the shape
/// a real listener draws, and it only fires if the substrate really stamps
/// `hop.route` on the receipt.
fn write_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        root,
        "main/hive",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./capture",
             "condition":"has(hop.route) && hop.route == 'mutation_committed'"}
        ]}}}"#,
    );
    write(
        root,
        "main/a",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/b",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Boot a colony over [`write_topology`], with `colony.json` written first so
/// the handle picks it up.
async fn boot(
    td: &tempfile::TempDir,
    colony_json: Option<&str>,
) -> (ColonyHandle, mpsc::Receiver<Message>) {
    write_topology(td.path());
    if let Some(doc) = colony_json {
        std::fs::write(td.path().join("colony.json"), doc).unwrap();
    }
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());

    // Anti-cascade: the terminal exists before anything is sent towards it, and
    // it is a live registry entry the boot's edge check can resolve.
    let (cap_tx, cap_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/hive/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");

    (h, cap_rx)
}

/// The `colony.json` that opts in.
const OPTED_IN: &str = r#"{"schema_version": 1, "mutation_receipts": {"to": "/hive"}}"#;

/// One committing single mutation: swap the `/a -> /b` edge for a fresh one.
fn edge_mutation(condition: &str) -> Value {
    json!({
        "scope": "/",
        "diff": {"add_edges": [{"from": "a", "to": "b", "condition": condition}]}
    })
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

/// Send a mutation through the door and return both the verdict and the trace
/// id it travelled under.
async fn knock(h: &ColonyHandle, payload: Value) -> (MutationDoorOutcome, Uuid) {
    let trace_id = Uuid::now_v7();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id,
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    (ack_rx.await.unwrap(), trace_id)
}

/// Wait for the next message at the capture cell. Generous by the 30 s
/// failure-marker convention.
async fn next_receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no receipt arrived at the capture cell")
        .expect("capture channel closed")
}

/// Assert nothing arrives within a short, deliberate window. Kept tight on
/// purpose: this is a semantic discriminator ("silence"), not a failure marker.
async fn stays_quiet(rx: &mut mpsc::Receiver<Message>) {
    match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        Err(_) => {}
        Ok(other) => panic!("expected silence, got {other:?}"),
    }
}

/// The five keys, sorted, of a receipt's `hop` compartment.
fn hop_keys(msg: &Message) -> Vec<String> {
    let mut k: Vec<String> = msg.headers.hop.keys().cloned().collect();
    k.sort();
    k
}

fn hop_str(msg: &Message, key: &str) -> String {
    msg.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_boot_is_the_first_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    let (_h, mut cap_rx) = boot(&td, Some(OPTED_IN)).await;

    let boot_receipt = next_receipt(&mut cap_rx).await;
    assert_eq!(
        hop_keys(&boot_receipt),
        vec!["form", "mutation_id", "outcome", "route", "scope"],
        "the boot receipt carries exactly the five contract keys"
    );
    assert_eq!(hop_str(&boot_receipt, "route"), "mutation_committed");
    assert_eq!(hop_str(&boot_receipt, "form"), "boot");
    assert_eq!(hop_str(&boot_receipt, "outcome"), "committed");
    assert_eq!(hop_str(&boot_receipt, "scope"), "/");
    assert_eq!(
        hop_str(&boot_receipt, "mutation_id"),
        Uuid::nil().to_string(),
        "a boot is nobody's mutation, so the id is nil"
    );
    assert_eq!(
        boot_receipt.target,
        Path::new("/hive/capture"),
        "the hive's own edge is what carried it the last hop"
    );
    assert!(
        boot_receipt.reply_to.is_none(),
        "a receipt is terminal — it answers nobody"
    );
    // The boot receipt continues nothing, so `/colony` emitted it without a
    // parent — and `route_with_log` books a parentless message as `@external`,
    // the booking of every source emission. The message that ARRIVES here is
    // the hive transit's onward hop and therefore does carry a parent, so the
    // claim has to be read where it is made: in the audit.
    wait_for_message_log_count(
        &td.path().join("colony.db"),
        &boot_receipt.trace_id.to_string(),
        1,
        Duration::from_secs(30),
    )
    .await;
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony.db");
    let first_from: String = conn
        .query_row(
            "SELECT from_path FROM message_log WHERE trace_id = ? ORDER BY rowid LIMIT 1",
            [boot_receipt.trace_id.to_string()],
            |r| r.get(0),
        )
        .expect("the boot receipt is in the audit");
    assert_eq!(
        first_from, "@external",
        "a boot receipt has no parent message, so the audit books it as a source emission"
    );
    assert_eq!(
        boot_receipt.body,
        meclaw_core::Body::Inline(json!({"messages": []})),
        "the body is the empty UBF document; everything the receipt says is in the hop"
    );
    stays_quiet(&mut cap_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_single_mutation_leaves_one_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = boot(&td, Some(OPTED_IN)).await;
    let boot_receipt = next_receipt(&mut cap_rx).await;
    assert_eq!(hop_str(&boot_receipt, "form"), "boot");

    let (outcome, trace_id) = knock(&h, edge_mutation("hop.route == 'go'")).await;
    let MutationDoorOutcome::Single(MutationOutcome::Committed { id }) = &outcome else {
        panic!("precondition: the edge mutation commits; got {outcome:?}");
    };

    let receipt = next_receipt(&mut cap_rx).await;
    assert_eq!(
        hop_keys(&receipt),
        vec!["form", "mutation_id", "outcome", "route", "scope"],
    );
    assert_eq!(hop_str(&receipt, "form"), "single");
    assert_eq!(
        &hop_str(&receipt, "mutation_id"),
        id,
        "the receipt names the mutation that was committed"
    );
    assert_eq!(hop_str(&receipt, "scope"), "/");
    assert_eq!(
        receipt.trace_id, trace_id,
        "the receipt continues the mutation's trace"
    );
    stays_quiet(&mut cap_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_manifest_names_every_id() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = boot(&td, Some(OPTED_IN)).await;
    let _boot = next_receipt(&mut cap_rx).await;

    let (outcome, _) = knock(
        &h,
        json!({"manifest": [
            edge_mutation("hop.route == 'one'"),
            edge_mutation("hop.route == 'two'"),
        ]}),
    )
    .await;
    let MutationDoorOutcome::Manifest(meclaw_colony::ManifestOutcome::Committed { ids }) = &outcome
    else {
        panic!("precondition: the manifest commits; got {outcome:?}");
    };
    assert_eq!(ids.len(), 2);

    let receipt = next_receipt(&mut cap_rx).await;
    assert_eq!(
        hop_keys(&receipt),
        vec!["form", "mutation_ids", "outcome", "route", "scope"],
        "a manifest names its ids in the plural key, and nothing else changes"
    );
    assert_eq!(hop_str(&receipt, "form"), "manifest");
    let got: Vec<String> = receipt.headers.hop["mutation_ids"]
        .as_array()
        .expect("mutation_ids is a list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(&got, ids, "one id per entry, in manifest order");
    stays_quiet(&mut cap_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_mutation_leaves_no_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = boot(&td, Some(OPTED_IN)).await;
    let _boot = next_receipt(&mut cap_rx).await;

    let refused = mutate(
        &h,
        json!({"scope": "/", "diff": {"add_edges": [{"from": "a", "to": "nowhere"}]}}),
    )
    .await;
    assert!(
        matches!(refused, MutationOutcome::Rejected { .. }),
        "precondition: an edge to a path nobody holds is refused; got {refused:?}"
    );
    stays_quiet(&mut cap_rx).await;

    // The positive half: the same colony still emits for a mutation that DOES
    // commit, so the silence above is the refusal and not a broken fixture.
    let committed = mutate(&h, edge_mutation("hop.route == 'go'")).await;
    assert!(matches!(committed, MutationOutcome::Committed { .. }));
    let receipt = next_receipt(&mut cap_rx).await;
    assert_eq!(hop_str(&receipt, "form"), "single");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_that_does_not_ask_gets_no_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = boot(&td, None).await;

    let committed = mutate(&h, edge_mutation("hop.route == 'go'")).await;
    assert!(
        matches!(committed, MutationOutcome::Committed { .. }),
        "precondition: the mutation commits; got {committed:?}"
    );
    stays_quiet(&mut cap_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn n_mutations_and_one_boot_leave_n_plus_one_receipts() {
    const N: usize = 4;
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut cap_rx) = boot(&td, Some(OPTED_IN)).await;

    for i in 0..N {
        let outcome = mutate(&h, edge_mutation(&format!("hop.route == 'r{i}'"))).await;
        assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    }

    let mut forms = Vec::new();
    for _ in 0..=N {
        forms.push(hop_str(&next_receipt(&mut cap_rx).await, "form"));
    }
    assert_eq!(
        forms,
        vec!["boot", "single", "single", "single", "single"],
        "the boot, then one receipt per mutation — and no receipt begets a receipt"
    );
    stays_quiet(&mut cap_rx).await;
}
