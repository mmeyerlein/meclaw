//! GH #422 — the SINGLE mutation body form does not move.
//!
//! WHAT THIS FILE IS
//! =================
//! Ruling R5 of the 2026-08-26 wave adds a SECOND body form to
//! `/colony/mutations` — the manifest, an ordered list of mutation bodies in
//! one body — and says of the first one: "die Einzel-Mutations-Form bewegt
//! sich byte-genau nicht". A pin written AFTER that change would pin the
//! change. So this file is written FIRST, against the tree as it stands, and
//! it measures the exact shape the single form has today:
//!
//! * a committed single mutation replies with exactly `id` + `outcome`;
//! * a rejected one with exactly `details` + `error_code` + `id` + `outcome`,
//!   and `violations` is NOT on the wire (GH #293);
//! * a body carrying an unknown top-level key still takes the single path —
//!   only the key `manifest` may ever discriminate.
//!
//! Every assertion here is a BEHAVIOUR of the shipped door, read off a green
//! run before any manifest code existed. A number or a key that moves later is
//! a contract break, not a refactor.
//!
//! HOW THE DOOR IS REACHED
//! =======================
//! Through the EDA dispatch path, not through `ColonyMsg::Mutation`: the reply
//! body this file pins is built by `colony_dispatch::build_mutation_reply`, and
//! only the dispatch path builds it. A probe cell emits one `CellOutput` at
//! `/colony/mutations` and captures what comes back — the `reply_to` is the
//! auto-stamp the outputs arm performs (spec Z.891), never set by hand, so the
//! measurement rides the same mechanism a real cell would.
//!
//! THE DEFECT THIS FILE FOUND (GH #432)
//! ====================================
//! Measuring the door turned up one: a `Body::Blob` arriving at
//! `/colony/mutations` was reported `committed` and changed nothing, because
//! `body_value` mapped it to `Value::Null`. The four tests at the end of this
//! file are that find, fixed and pinned — including the verification R5 asked
//! for, that a manifest over the inline threshold travels as a blob and works.
//!
//! WHY THE CELLS ARE INERT
//! =======================
//! The claim under test is the shape of a control-plane reply. The probe is
//! `meclaw_testing::EmitOnceMockCellFactory`; nothing else in these colonies
//! ever runs.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::{ColonyHandle, EmitOnceMockCellFactory};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Generous: these colonies boot a filesystem tree and wake a dormant cell.
const REPLY_WAIT: Duration = Duration::from_secs(30);

/// The one instantiable cell the blob cases use, as a template `config.json`.
const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

/// The root tree every case in this file boots: one hive, no edges, no cells.
/// The probe is registered beside it, never inside it — the bootstrap walk must
/// not see a cell directory it has no factory for.
fn write_root(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
}

/// One knock at `/colony/mutations` and the reply body that came back.
///
/// The `TempDir`s are returned so the caller keeps both trees alive: the colony
/// root, and the probe's own cell directory (which lives OUTSIDE the root, so
/// the boot walk never adopts it).
struct Knock {
    root: tempfile::TempDir,
    _probe_dir: tempfile::TempDir,
    handle: ColonyHandle,
    /// The dispatcher's own reply — the body `build_mutation_reply` produced.
    reply: Value,
    /// Everything else that reached the probe before it, in arrival order.
    ///
    /// A REFUSED mutation with a `reply_to` produces TWO messages at that
    /// address: `handle_mutation`'s own EDA reject (a UBF body with a
    /// `system.header.error_code`) and, behind it, the dispatcher's
    /// `{"mutation": …}` slot. That double answer is a property of the single
    /// form and is pinned below — it is also the reason a manifest hands
    /// `reply_to: None` to every entry it rolls off (R5: one manifest, one
    /// verdict).
    before: Vec<Value>,
}

/// Boot a colony, let one probe emit `payload` at `/colony/mutations`, and
/// return what the door replied.
///
/// The payload gains an empty `messages[]` on the way out: every CELL emission
/// is UBF-validated in the outputs arm (debug builds), and a body without one
/// of the UBF alternatives is dead-lettered as `invalid_ubf_body` before it
/// ever reaches a door. That is a property of the emit path, not of the
/// mutation door — a `curl` at the HTTP door needs no such key — so it is added
/// here once rather than written into every case below.
async fn knock(payload: Value) -> Knock {
    let mut payload = payload;
    if payload.get("messages").is_none()
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("messages".to_string(), json!([]));
    }
    let root = tempfile::TempDir::new().unwrap();
    write_root(root.path());
    let probe_dir = tempfile::TempDir::new().unwrap();

    let handle = ColonyHandle::new_with_factories_at(&root, Vec::new());

    let (capture_tx, mut capture_rx) = mpsc::channel(8);
    let factory = Arc::new(EmitOnceMockCellFactory::new(
        Path::new("/colony/mutations"),
        payload,
        capture_tx,
    ));
    let spawned = factory
        .spawn_cell(
            Path::new("/probe"),
            json!({}),
            handle.outputs_sender(),
            probe_dir.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            handle.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            64,
        )
        .expect("probe spawn");
    // Anti-cascade (phase-6.5 lesson): the probe is registered BEFORE the boot,
    // so the reply path to `/probe` resolves the moment the door answers.
    handle.register_spawned(Path::new("/probe"), spawned).await;

    let factories = CellFactoryRegistry::new();
    bootstrap_from_filesystem(root.path(), &factories, &handle.runtime())
        .await
        .expect("the root tree must boot");

    handle
        .send(
            MessageBuilder::new(Path::new("/probe"))
                .body(Body::Inline(json!({"messages": []})))
                .build(),
        )
        .await;

    let mut before = Vec::new();
    let reply = loop {
        let got = match tokio::time::timeout(REPLY_WAIT, capture_rx.recv()).await {
            Ok(Some(m)) => m,
            Ok(None) => panic!("/probe capture channel closed before the door answered"),
            Err(_) => {
                let dlq = handle.drain_dead_letters().await;
                panic!(
                    "the mutation door did not answer within {REPLY_WAIT:?}; \
                     seen so far: {before:?}; DLQ: {dlq:?}"
                )
            }
        };
        let body = match got.body {
            Body::Inline(v) => v,
            Body::Blob(id) => panic!("the door replied with a blob body ({id})"),
        };
        if body.get("mutation").is_some() {
            break body;
        }
        before.push(body);
    };
    Knock {
        root,
        _probe_dir: probe_dir,
        handle,
        reply,
        before,
    }
}

/// The same knock, but the body reaches the door as a `Body::Blob`.
///
/// The substrate's oversized-body offload (`colony::offload_oversized`) is
/// gated on `should_log`, and `should_log` deliberately excludes `/colony/*` —
/// so a cell emitting straight at the door never offloads. It offloads one hop
/// EARLIER: this fixture puts a hive `/door` in front, whose single out-edge is
/// `/colony/mutations`. The hop INTO the hive is an ordinary logged hop, the
/// body crosses the (here: 1-byte) threshold and becomes a blob, and the hive
/// transit hands that blob on to the door. That is the shape a manifest large
/// enough to be offloaded really arrives in.
async fn knock_through_a_hive(payload: Value) -> Knock {
    let mut payload = payload;
    if payload.get("messages").is_none()
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("messages".to_string(), json!([]));
    }
    let root = tempfile::TempDir::new().unwrap();
    write_root(root.path());
    // Every body crosses this threshold: the offload is `>=`, so 1 offloads all.
    std::fs::write(
        root.path().join("colony.json"),
        r#"{"blob_inline_max_bytes":1}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("main/door")).unwrap();
    std::fs::write(
        root.path().join("main/door/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
             {"from":".","to":"/colony/mutations"}
           ]}}}"#,
    )
    .unwrap();
    // One template, so a body that really was read can leave a registry row.
    let tpl = root.path().join("templates/persist_mock");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    std::fs::write(tpl.join("config.json"), CELL_CONFIG).unwrap();
    let probe_dir = tempfile::TempDir::new().unwrap();

    let cell_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    });
    let handle = ColonyHandle::new_with_blobs_at(
        &root,
        vec![("persist_mock".to_string(), cell_factory.clone())],
    );
    // The probe's own view of the same store the colony wired (`<root>/blobs`),
    // so its delivery boundary resolves the blob replies it is sent.
    let probe_blobs = Arc::new(
        meclaw_colony::DiskBlobStore::new(root.path().join("blobs")).expect("probe blob store"),
    );

    let (capture_tx, mut capture_rx) = mpsc::channel(8);
    let factory = Arc::new(EmitOnceMockCellFactory::new(
        Path::new("/door"),
        payload,
        capture_tx,
    ));
    let spawned = factory
        .spawn_cell(
            Path::new("/probe"),
            json!({}),
            handle.outputs_sender(),
            probe_dir.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            handle.inbox_tx.clone(),
            None,
            0,
            None,
            Some(probe_blobs),
            64,
        )
        .expect("probe spawn");
    handle.register_spawned(Path::new("/probe"), spawned).await;

    let mut factories = CellFactoryRegistry::new();
    factories.insert("persist_mock".into(), cell_factory);
    bootstrap_from_filesystem(root.path(), &factories, &handle.runtime())
        .await
        .expect("the root tree must boot");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    handle
        .inbox_tx
        .send(meclaw_colony::ColonyMsg::RescanTemplates {
            templates_root: root.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    // Ruling A1: a cell emission that matches no out-edge dead-letters as
    // `no_route`. Only `/colony/*` is dispatched edge-free, and `/door` is a
    // hive — so the probe needs a real lane to it.
    handle
        .add_edge(
            meclaw_core::Uuid::now_v7(),
            Path::new("/probe"),
            Path::new("/door"),
        )
        .await;

    handle
        .send(
            MessageBuilder::new(Path::new("/probe"))
                .body(Body::Inline(json!({"messages": []})))
                .build(),
        )
        .await;

    let mut before = Vec::new();
    let reply = loop {
        let got = match tokio::time::timeout(REPLY_WAIT, capture_rx.recv()).await {
            Ok(Some(m)) => m,
            Ok(None) => panic!("/probe capture channel closed before the door answered"),
            Err(_) => {
                let dlq = handle.drain_dead_letters().await;
                panic!(
                    "the mutation door did not answer within {REPLY_WAIT:?}; \
                     seen so far: {before:?}; DLQ: {dlq:?}"
                )
            }
        };
        let body = match got.body {
            Body::Inline(v) => v,
            Body::Blob(id) => panic!("the reply reached the cell unresolved ({id})"),
        };
        if body.get("mutation").is_some() || body.get("manifest").is_some() {
            break body;
        }
        before.push(body);
    };
    Knock {
        root,
        _probe_dir: probe_dir,
        handle,
        reply,
        before,
    }
}

/// The keys of the `mutation` slot of a reply, in the order `serde_json` holds
/// them (its `Value::Object` is a `BTreeMap`, so this is alphabetical and
/// stable).
fn mutation_keys(reply: &Value) -> Vec<&str> {
    reply
        .get("mutation")
        .unwrap_or_else(|| panic!("reply carries the top-level slot `mutation`: {reply}"))
        .as_object()
        .unwrap_or_else(|| panic!("the `mutation` slot is an object: {reply}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Every path the persisted registry holds after the run.
fn registry_paths(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT path FROM registry ORDER BY path")
        .unwrap();
    let out: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// the pins
// ──────────────────────────────────────────────────────────────────────────────

/// A committed single mutation replies with EXACTLY two keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_single_mutation_replies_exactly_two_keys() {
    let k = knock(json!({"scope": "/", "diff": {}})).await;
    assert_eq!(
        mutation_keys(&k.reply),
        vec!["id", "outcome"],
        "a committed reply carries exactly these two keys: {}",
        k.reply
    );
    assert_eq!(k.reply["mutation"]["outcome"], "committed");
    k.handle.shutdown().await;
}

/// A rejected single mutation replies with EXACTLY four keys — and `violations`
/// is deliberately not one of them (GH #293: the rendered `details` already
/// carries every violation, one per line).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_single_mutation_replies_exactly_four_keys() {
    let k = knock(json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "nope", "template": "no-such-template@1.0.0"}]}
    }))
    .await;
    assert_eq!(
        mutation_keys(&k.reply),
        vec!["details", "error_code", "id", "outcome"],
        "a rejected reply carries exactly these four keys: {}",
        k.reply
    );
    assert_eq!(k.reply["mutation"]["outcome"], "rejected");
    assert!(
        k.reply["mutation"].get("violations").is_none(),
        "GH #293: `violations` is not on this wire: {}",
        k.reply
    );
    // The double answer of the single form, measured: the EDA reject comes
    // first, the dispatcher's slot behind it.
    assert_eq!(
        k.before.len(),
        1,
        "a refused single mutation answers TWICE at one `reply_to`: {:?}",
        k.before
    );
    assert_eq!(
        k.before[0]["system"]["header"]["error_code"], "template_missing",
        "the first of the two is `handle_mutation`'s own EDA reject: {}",
        k.before[0]
    );
    k.handle.shutdown().await;
}

/// A body with an unknown top-level key takes the single path, unchanged.
///
/// This is the assertion the manifest must not break: after R5 exactly ONE
/// top-level key discriminates, and it is `manifest`. Everything else — a
/// comment, a client's correlation field, a typo — is ignored exactly as it is
/// ignored today.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_without_manifest_key_takes_the_single_path() {
    let k = knock(json!({"scope": "/", "diff": {}, "comment": "x"})).await;
    assert_eq!(
        mutation_keys(&k.reply),
        vec!["id", "outcome"],
        "an unknown top-level key changes nothing: {}",
        k.reply
    );
    assert_eq!(k.reply["mutation"]["outcome"], "committed");
    k.handle.shutdown().await;
}

/// The registry a refused mutation leaves behind is the one it found.
///
/// Sibling of the two shape pins above and the baseline Task 2 measures
/// against: a refusal writes nothing. `/probe` is the fixture's own cell, hand-
/// registered before the boot — it is what "the registry it found" means here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_single_mutation_leaves_the_registry_empty() {
    let k = knock(json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "nope", "template": "no-such-template@1.0.0"}]}
    }))
    .await;
    assert_eq!(k.reply["mutation"]["outcome"], "rejected");
    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(
        registry_paths(&root),
        vec!["/probe".to_string()],
        "a refusal registers nothing beyond the probe the fixture placed"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the defect this lane found and fixed (GH #432)
// ──────────────────────────────────────────────────────────────────────────────

/// A blob-bodied mutation is RESOLVED before the door dispatches it.
///
/// The find, and the shape it had: `colony_dispatch::body_value` mapped
/// `Body::Blob(_)` to `Value::Null`, so `handle_mutation` found no `diff`, fell
/// back to an empty one and replied `{"mutation":{"outcome":"committed"}}` for a
/// body it had never read. The message door did not inherit the
/// delivery-boundary resolution (`cell_task::resolve_blob_for_delivery`) that
/// every cell gets — and a body large enough for the substrate's own offload is
/// exactly such a body. R5 asked that this be verified; the verification was
/// red, and this test is the record of it being green.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_bodied_mutation_message_is_resolved_before_dispatch() {
    let k = knock_through_a_hive(json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "grown", "template": "persist_mock"}]}
    }))
    .await;
    assert_eq!(
        k.reply["mutation"]["outcome"], "committed",
        "the blob-borne diff was read: {}",
        k.reply
    );
    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(
        registry_paths(&root),
        vec!["/grown".to_string(), "/probe".to_string()],
        "…and the node it asked for really stands"
    );
}

/// And a blob-borne diff that is WRONG is refused, not waved through.
///
/// The old behaviour reported `committed` for exactly this body. That it now
/// refuses is the sharper half of the fix: the door reads what it judges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_bodied_mutation_that_is_wrong_is_refused() {
    let k = knock_through_a_hive(json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "nope", "template": "no-such-template@1.0.0"}]}
    }))
    .await;
    assert_eq!(
        k.reply["mutation"]["outcome"], "rejected",
        "a diff naming a template that does not exist is refused: {}",
        k.reply
    );
    assert_eq!(k.reply["mutation"]["error_code"], "template_missing");
    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(registry_paths(&root), vec!["/probe".to_string()]);
}

/// A body whose blob is gone dead-letters instead of committing nothing.
///
/// Same rule as at the cell boundary: a failed resolution delivers the message
/// NOT AT ALL, never half — and certainly never as a success report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_whose_blob_is_gone_dead_letters_instead_of_committing_nothing() {
    let root = tempfile::TempDir::new().unwrap();
    write_root(root.path());
    let handle = ColonyHandle::new_with_blobs_at(&root, Vec::new());
    bootstrap_from_filesystem(root.path(), &CellFactoryRegistry::new(), &handle.runtime())
        .await
        .expect("boot");

    // A pointer at an id the store never held.
    handle
        .send(
            MessageBuilder::new(Path::new("/colony/mutations"))
                .body(Body::Blob(meclaw_core::Uuid::now_v7()))
                .build(),
        )
        .await;

    let mut dlq = Vec::new();
    for _ in 0..100 {
        dlq = handle.drain_dead_letters().await;
        if !dlq.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        dlq.len(),
        1,
        "the unresolvable body is dead-lettered: {dlq:?}"
    );
    assert_eq!(
        dlq[0].resolved_target.as_str(),
        "/colony/mutations",
        "{dlq:?}"
    );
    let root_path = root.path().to_path_buf();
    handle.shutdown().await;
    assert!(
        registry_paths(&root_path).is_empty(),
        "and nothing was committed"
    );
}

/// A manifest larger than the inline threshold travels as a blob — and works.
///
/// This is the verification R5 asked for, in the form R5 named it: "grosse
/// Bodies reisen per vorhandenem Blob-Offload". Three entries, a 1-byte inline
/// threshold, and the receipt says `applied 3`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_larger_than_the_inline_threshold_travels_as_a_blob() {
    let node = |name: &str| json!({"scope": "/", "diff": {"add_nodes": [{"name": name, "template": "persist_mock"}]}});
    let k = knock_through_a_hive(json!({"manifest": [node("a"), node("b"), node("c")]})).await;
    let m = &k.reply["manifest"];
    assert_eq!(m["outcome"], "committed", "{}", k.reply);
    assert_eq!(m["applied"], 3);
    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(
        registry_paths(&root),
        vec![
            "/a".to_string(),
            "/b".to_string(),
            "/c".to_string(),
            "/probe".to_string()
        ]
    );
}
