//! GH #424 — the seed that grows itself, measured against the mutation that
//! grows the same thing.
//!
//! WHAT THIS FILE IS
//! =================
//! Ruling R4 asks for a root tree that references a composite template and
//! grows itself on the first `meclaw --root` start, "through the same
//! resolution the mutation path takes". `examples/organism/seed-ref/` is that
//! tree: a `colony.json`, one empty root hive, and ONE `cell.type: "ref"`
//! marker naming the shipped `meclaw-os` template. Zero cells checked in.
//!
//! The acceptance is a comparison, because "the same resolution" is only
//! provable against the other resolution:
//!
//! * **A** — `seed-ref` booted. The first boot fulfils the marker.
//! * **B** — `seed` booted, then `examples/organism/grow-os.json` applied as a
//!   mutation.
//!
//! Three ways they must agree: the registry rows (path, template, version,
//! chain), the edge set, and the `config.json` BYTES after UUID blanking. The
//! byte comparison uses the same device
//! `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs`
//! uses, so "byte-identical" means here what it means there.
//!
//! WHAT THIS FILE IS NOT
//! =====================
//! It is NOT the claim that bootstrap refs can build the whole organism. A
//! `ref` marker declares a NODE and never an EDGE, and the shipped example
//! carries 48 hand-written transit edges hanging off hives the templates
//! materialise — addresses that do not exist until the growth has happened. The
//! whole stack from one file is the OTHER half of this lane:
//! `meclaw --root <seed> --apply examples/organism/grow.manifest.json`
//! (`gh422_the_manifest_grows_the_same_stack.rs`,
//! `gh423_apply_one_shot.rs`). Read `examples/organism/README.md` § "The seed that
//! grows itself" for the same sentence in prose.
//!
//! Guarded like every template-reading test (GH #49): a tree that did not ship
//! the example or the library is SKIPPED, never judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::Value;
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

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Did the two seeds and the library travel with this tree (GH #49)?
fn shipped() -> bool {
    repo("examples/organism/seed-ref/main/os/config.json").is_file()
        && repo("examples/organism/grow-os.json").is_file()
        && repo("templates/meclaw-os/template.json").is_file()
}

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

// ──────────────────────────────────────────────────────────────────────────────
// the inert factory (the device of gh302 / gh277, copied for the same reason)
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
// two roots
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

/// A colony root built from one of the two shipped seeds, plus the real
/// library and a placeholder `.env`.
fn build_root_from(seed: &str, root: &std::path::Path) {
    copy_tree(&repo(seed), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
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

/// Scan the templates FIRST, then boot — the order production keeps, and the
/// reason a growth has a registry to resolve against.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories(td.path());
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack");
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the seed must boot");
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

/// One registry row, as the persisted `colony.db` holds it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    path: String,
    template: Option<String>,
    template_version: Option<String>,
    template_chain: Option<String>,
}

struct Grown {
    _td: tempfile::TempDir,
    root: std::path::PathBuf,
    rows: Vec<Row>,
    edges: Vec<(String, String)>,
    hives: Vec<String>,
}

async fn harvest(td: tempfile::TempDir, h: ColonyHandle) -> Grown {
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

    // NOTE on the direct SQL: test-side reading of `colony.db`, not cell code.
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

    let root = td.path().join("main");
    Grown {
        _td: td,
        root,
        rows,
        edges,
        hives,
    }
}

/// **A** — the ref seed, grown by its own first boot.
async fn grown_by_the_boot() -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    build_root_from("examples/organism/seed-ref", td.path());
    let h = boot(&td).await;
    harvest(td, h).await
}

/// **B** — the plain seed, grown by the shipped mutation.
async fn grown_by_the_mutation() -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    build_root_from("examples/organism/seed", td.path());
    let h = boot(&td).await;
    let outcome = mutate(&h, read_json(&repo("examples/organism/grow-os.json"))).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "grow-os.json must commit: {outcome:?}"
    );
    harvest(td, h).await
}

// ──────────────────────────────────────────────────────────────────────────────
// the byte fingerprint (the device of gh277)
// ──────────────────────────────────────────────────────────────────────────────

/// Every `config.json` under `root`, keyed by its relative path, normalised:
/// the identity a fresh instantiation mints is blanked, everything else stands.
fn config_fingerprint(root: &std::path::Path) -> Vec<(String, String)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                let raw = std::fs::read(&p).unwrap();
                out.push((rel, normalise_config(&raw)));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// `cell.id` and `cell.provenance` are per-instantiation identity (`provenance`
/// carries `instantiated_at`), so both are dropped; every remaining UUID string
/// becomes `<uuid>`. Exactly `gh277_composite_instantiation_is_byte_identical`'s
/// rule, so "byte-identical" means the same thing in both files.
fn normalise_config(raw: &[u8]) -> String {
    let mut v: Value = meclaw_core::serde_json::from_slice(raw).expect("config.json is JSON");
    if let Some(cell) = v.get_mut("cell").and_then(|c| c.as_object_mut()) {
        cell.remove("id");
        cell.remove("provenance");
    }
    blank_uuids(&mut v);
    meclaw_core::serde_json::to_string_pretty(&v).expect("re-serialise")
}

fn blank_uuids(v: &mut Value) {
    match v {
        Value::String(s) if Uuid::parse_str(s).is_ok() => *s = "<uuid>".to_string(),
        Value::Array(a) => a.iter_mut().for_each(blank_uuids),
        Value::Object(o) => o.values_mut().for_each(blank_uuids),
        _ => {}
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// the acceptance
// ──────────────────────────────────────────────────────────────────────────────

/// The boot-grown ref is byte-identical to the mutation-grown tree.
///
/// **The pin for `SNB-graph-bootstrap-{de,en}`** in
/// `plans/spec-claims/claims.tsv`: a declaration in the root tree provides the
/// initial desired state for its position on first instantiation, and it does
/// so through the very chain a mutation takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_boot_grown_ref_is_byte_identical_to_the_mutation_grown_tree() {
    if !shipped() || !library_is_complete() {
        eprintln!("skipped: examples/organism or the template library did not ship (GH #49)");
        return;
    }
    let a = grown_by_the_boot().await;
    let b = grown_by_the_mutation().await;

    assert_eq!(
        a.rows, b.rows,
        "same registry rows: path, template, version, chain"
    );
    assert_eq!(a.edges, b.edges, "same edge set");
    assert_eq!(a.hives, b.hives, "same hive scopes");
    assert_eq!(
        config_fingerprint(&a.root),
        config_fingerprint(&b.root),
        "same config.json bytes after UUID blanking"
    );
    // Anti-vacuity: a comparison of two empty trees proves nothing.
    assert!(
        a.rows.len() > 5,
        "the grown shell is a real tree, not a stub: {} rows",
        a.rows.len()
    );
}

/// The seed names a version the shipped library actually holds.
///
/// Without this the seed could fall behind a template bump unnoticed — the same
/// role `gh302`'s `a_the_five_declarations_name_the_whole_stack_by_pinned_reference`
/// plays for the five declarations.
#[test]
fn the_seed_ref_names_a_version_the_library_holds() {
    if !shipped() {
        eprintln!("skipped: examples/organism did not ship (GH #49)");
        return;
    }
    let marker = read_json(&repo("examples/organism/seed-ref/main/os/config.json"));
    let reference = marker["cell"]["template"]
        .as_str()
        .expect("the marker declares cell.template");
    let declared = read_json(&repo("templates/meclaw-os/template.json"));
    let version = declared["version"]
        .as_str()
        .expect("meclaw-os declares a version");
    assert_eq!(
        reference,
        format!("meclaw-os@{version}"),
        "the seed's reference must name the version the library ships"
    );
}

/// And it names the SAME version the shipped mutation names.
///
/// The two acceptances of this lane grow the same shell; a seed that drifted
/// away from `grow-os.json` would make them incomparable without saying so.
#[test]
fn the_seed_ref_and_the_shipped_mutation_name_one_version() {
    if !shipped() {
        eprintln!("skipped: examples/organism did not ship (GH #49)");
        return;
    }
    let marker = read_json(&repo("examples/organism/seed-ref/main/os/config.json"));
    let grow = read_json(&repo("examples/organism/grow-os.json"));
    assert_eq!(
        marker["cell"]["template"], grow["diff"]["add_nodes"][0]["template"],
        "one shell, one version, whichever way it is grown"
    );
}
