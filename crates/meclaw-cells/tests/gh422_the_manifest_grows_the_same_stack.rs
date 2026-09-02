//! GH #422 — the five shipped declarations grow the same stack as ONE manifest.
//!
//! WHAT THIS FILE IS
//! =================
//! `examples/organism/` is a colony with zero cells checked in and five
//! declarations beside it. Ruling R5 says those five may travel as one body:
//! `examples/organism/grow.manifest.json` is that body, and it is the five
//! files verbatim, in the same order, wrapped in one `manifest` array.
//!
//! Four measurements:
//!
//! * **The two ways build the same tree.** Same registry rows (path, template,
//!   version, chain), same edge set. If they ever differ, the manifest is not
//!   "the same mutations in one body" any more.
//! * **The file is the five, verbatim.** Read off disk and compared entry by
//!   entry, so the manifest cannot drift away from the declarations it bundles.
//! * **No rollback.** A manifest broken at entry 3 leaves entries 1 and 2
//!   standing, says `failed_at: 3`, and never looks at 4 and 5.
//! * **Resumable.** Dropping the applied entries and sending the rest arrives at
//!   exactly the tree the whole manifest would have built.
//!
//! WHY THE CELLS ARE INERT — and the guard
//! =======================================
//! Same device and same reason as `gh302_the_stack_grows_from_templates.rs`:
//! every claim here is structural, and two of the cell types this stack names
//! would reach outward the moment they were spawned for real. And like every
//! template-reading test (GH #49), a tree that did not ship the example or the
//! library is SKIPPED, never judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, MutationOutcome, RespawnFn,
    SpawnedCellKind, WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The six declarations, in the order the manifest bundles them.
///
/// The sixth is GH #560: the two credential v-lanes per surface plus the
/// grants they spend. It is not a LEVEL — no node is grown by it — which is
/// why it stands apart from the five the `grow_level` recipe renders.
const DECLARATIONS: [&str; 6] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
    "examples/organism/grow-channel.json",
    "examples/organism/grow-credentials.json",
];

const MANIFEST: &str = "examples/organism/grow.manifest.json";

/// `examples/organism`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> bool {
    repo("examples/organism/grow-os.json").is_file() && repo(MANIFEST).is_file()
}

/// Whether the whole library this example instantiates travelled with the tree.
fn library_is_complete() -> bool {
    [
        "meclaw-os",
        "org",
        "member",
        "assistant",
        "talky",
        "telegram-connector",
        "cogny",
        "collector",
        "access",
    ]
    .iter()
    .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ──────────────────────────────────────────────────────────────────────────────
// the inert factory (the device of gh302, copied deliberately: gh302 is the
// measured reference and must not move because this file exists)
// ──────────────────────────────────────────────────────────────────────────────

struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);

        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });

        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });

        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// growing the tree
// ──────────────────────────────────────────────────────────────────────────────

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn cell_types_in(root: &std::path::Path) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json")
                && let Ok(raw) = std::fs::read_to_string(&p)
                && let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw)
                && let Some(t) = v["cell"]["type"].as_str()
            {
                out.insert(t.to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, &mut out);
    out.remove("hive");
    out.remove("ref");
    out
}

/// The colony root: the example's own seed, the real library, and an `.env`
/// whose every value is a placeholder.
fn build_root(root: &std::path::Path) {
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         MODEL_SURFACE=gpt-4o-mock-surface\n\
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n\
         KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
    )
    .unwrap();
}

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect()
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories(td.path());
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the empty seed of examples/organism must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    h
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
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

/// One body at the door, form unknown to the caller — exactly what `--apply`
/// and `POST /colony/mutations` do.
async fn knock(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send body");
    ack_rx.await.expect("door ack")
}

/// One registry row, as the persisted `colony.db` holds it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    path: String,
    template: Option<String>,
    template_version: Option<String>,
    template_chain: Option<String>,
}

/// Everything one run leaves behind.
struct Grown {
    _td: tempfile::TempDir,
    rows: Vec<Row>,
    /// The hive scopes the run registered. A hive is a scope marker, not an
    /// actor, so it leaves no registry row — and a declaration whose subtree is
    /// all containers would otherwise look like it did nothing.
    hives: Vec<String>,
    edges: Vec<(String, String)>,
    receipt: Value,
}

impl Grown {
    fn paths(&self) -> Vec<&str> {
        self.rows.iter().map(|r| r.path.as_str()).collect()
    }
}

/// Read the graph, shut down cleanly (which flushes the write buffer) and read
/// the registry back.
///
/// NOTE on the direct SQL: test-side reading of `colony.db`, not cell code. A
/// fixture that measures what the substrate persisted has to read it.
async fn harvest(td: tempfile::TempDir, h: ColonyHandle, receipt: Value) -> Grown {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut edges: Vec<(String, String)> = ack_rx
        .await
        .unwrap()
        .edges
        .iter()
        .map(|e| (e.from.to_string(), e.to.to_string()))
        .collect();
    edges.sort();
    h.shutdown().await;

    let conn = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare(
            "SELECT path, template, template_version, template_chain \
             FROM registry ORDER BY path",
        )
        .unwrap();
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                path: r.get(0)?,
                template: r.get(1)?,
                template_version: r.get(2)?,
                template_chain: r.get(3)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    let mut stmt = conn
        .prepare("SELECT path FROM hive_scopes ORDER BY path")
        .unwrap();
    let hives: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    drop(conn);

    Grown {
        _td: td,
        rows,
        hives,
        edges,
        receipt,
    }
}

/// Boot and apply the six declarations, one mutation each.
async fn grow_with_the_declarations() -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    for file in DECLARATIONS {
        let outcome = mutate(&h, read_json(&repo(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file} was not committed: {outcome:?}"
        );
    }
    harvest(td, h, Value::Null).await
}

/// Boot and apply ONE body: the manifest, as it lies on disk (or as the caller
/// bent it).
async fn grow_with_a_manifest(body: Value) -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    let outcome = knock(&h, body).await;
    let receipt = meclaw_colony::mutation_door_reply(&outcome);
    harvest(td, h, receipt["manifest"].clone()).await
}

/// The shipped manifest body.
fn manifest_body() -> Value {
    read_json(&repo(MANIFEST))
}

// ──────────────────────────────────────────────────────────────────────────────
// the four measurements
// ──────────────────────────────────────────────────────────────────────────────

/// One body builds the tree six bodies build.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_grows_the_same_stack_as_the_six_declarations() {
    if !shipped() || !library_is_complete() {
        eprintln!("skipped: examples/organism or the template library did not ship (GH #49)");
        return;
    }
    let by_five = grow_with_the_declarations().await;
    let by_manifest = grow_with_a_manifest(manifest_body()).await;

    assert_eq!(
        by_five.rows, by_manifest.rows,
        "same registry, row by row: path, template, version, chain"
    );
    assert_eq!(by_five.edges, by_manifest.edges, "same edge set");
    assert_eq!(by_five.hives, by_manifest.hives, "same hive scopes");
    assert_eq!(
        by_manifest.receipt["applied"], 6,
        "the receipt counts all six entries: {}",
        by_manifest.receipt
    );
    assert_eq!(by_manifest.receipt["outcome"], "committed");
    // Not a literal from a plan: the size of the tree the two ways agree on,
    // written down so a drift in either is visible.
    assert!(
        by_manifest.rows.len() > 40,
        "the grown stack is the whole organism, not a stub: {} rows",
        by_manifest.rows.len()
    );
}

/// The manifest file is the six declarations, verbatim and in order.
///
/// Without this the bundle could drift away from what it bundles and every
/// other assertion here would still pass.
#[test]
fn the_manifest_file_is_the_six_declarations_verbatim() {
    if !shipped() {
        eprintln!("skipped: examples/organism did not ship (GH #49)");
        return;
    }
    let manifest = manifest_body();
    let entries = manifest["manifest"]
        .as_array()
        .expect("`manifest` is an array");
    assert_eq!(entries.len(), 6, "one entry per declaration");
    for (i, file) in DECLARATIONS.iter().enumerate() {
        assert_eq!(
            entries[i],
            read_json(&repo(file)),
            "manifest entry {} is {file} verbatim",
            i + 1
        );
    }
}

/// A manifest broken at entry 3 leaves entries 1 and 2 standing.
///
/// THIS IS THE NO-ROLLBACK STATEMENT. The two committed entries are committed
/// and stay committed; entries 4 and 5 were never looked at. The receipt names
/// the position, which is the whole reason there is no rollback: an operator
/// can fix entry 3 and send the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_that_fails_at_step_three_leaves_the_first_two_standing() {
    if !shipped() || !library_is_complete() {
        eprintln!("skipped: examples/organism or the template library did not ship (GH #49)");
        return;
    }
    let mut body = manifest_body();
    // A typo in the third entry's template name — bent in the test, never on
    // disk: the shipped file has to stay the one an operator applies.
    body["manifest"][2]["diff"]["add_nodes"][0]["template"] = json!("membre@9.9.9");
    let broken = grow_with_a_manifest(body).await;

    assert_eq!(broken.receipt["outcome"], "rejected", "{}", broken.receipt);
    assert_eq!(broken.receipt["applied"], 2);
    assert_eq!(broken.receipt["failed_at"], 3);
    assert_eq!(broken.receipt["remaining"], 3, "4, 5 and 6 were never seen");
    assert_eq!(broken.receipt["error_code"], "template_missing");

    let paths = broken.paths();
    assert!(
        paths.iter().any(|p| p.starts_with("/os/access/")),
        "entry 1 stands: {paths:?}"
    );
    assert!(
        broken.hives.iter().any(|p| p == "/os/orgs/acme"),
        "entry 2 stands — its subtree is all containers, so it lives in the \
         hive scopes, not the registry: {:?}",
        broken.hives
    );
    assert!(
        !paths.iter().any(|p| p.contains("/members/alex")),
        "and nothing from entry 3 onwards does: {paths:?}"
    );
}

/// The manifest is resumable from where it stopped.
///
/// Apply the first two, then a manifest holding only entries 3–6, and arrive at
/// exactly the tree the whole manifest builds. "Resumable" is measured, not
/// promised.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_manifest_is_resumable_from_where_it_stopped() {
    if !shipped() || !library_is_complete() {
        eprintln!("skipped: examples/organism or the template library did not ship (GH #49)");
        return;
    }
    let whole = grow_with_a_manifest(manifest_body()).await;

    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    let entries = manifest_body()["manifest"].as_array().unwrap().clone();
    let first_two = knock(&h, json!({"manifest": entries[..2]})).await;
    assert!(first_two.is_committed(), "the head of the manifest commits");
    let rest = knock(&h, json!({"manifest": entries[2..]})).await;
    assert!(
        rest.is_committed(),
        "and the tail picks up where it stopped"
    );
    let receipt = meclaw_colony::mutation_door_reply(&rest);
    let resumed = harvest(td, h, receipt["manifest"].clone()).await;

    assert_eq!(resumed.receipt["applied"], 4, "{}", resumed.receipt);
    assert_eq!(
        whole.rows, resumed.rows,
        "two halves arrive where one whole does"
    );
    assert_eq!(whole.edges, resumed.edges, "same edge set");
    assert_eq!(whole.hives, resumed.hives, "same hive scopes");
}
