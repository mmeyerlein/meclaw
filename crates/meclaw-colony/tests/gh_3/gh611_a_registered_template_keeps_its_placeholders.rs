//! GH #611: registering a template must not substitute its files.
//!
//! `add_templates` files a CLASS in the instance-local library. Until this
//! issue the whole diff went through the full substitution pass first, so the
//! file bodies of a declaration were rewritten on the way in: a `config.json`
//! that referenced the colony's API key as `${SECRET_API_KEY}` arrived under
//! `{templates_root}/local/<name>@<version>/` with the key in CLEAR TEXT, and every
//! further registration of that derived class copied it again. The mirror
//! image of the same bug refused a registration outright — a README that only
//! MENTIONS `${…}` in prose was read as a placeholder and answered
//! `env_var_missing`, for a variable the registering colony had no reason to
//! own.
//!
//! What the fix asserts here, in the order the issue names it:
//!
//! 1. the file under `local/<name>@<version>/` is byte-identical to the declaration, and
//!    no sentinel appears anywhere under the root (the dumb, literal sweep);
//! 2. a README whose prose carries `${…}` registers, untouched;
//! 3. an instance grown from such a class still gets the environment's value —
//!    the token stands on disk and binds at read time, exactly as GH #20 has it
//!    for every other template.

use meclaw_colony::mutation::{ManifestOutcome, MutationDoorOutcome, MutationOutcome};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    ColonyTaskConfig, colony_task,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::{mpsc, oneshot};

/// The value in the colony's `.env`. It must never reach a file.
const SENTINEL: &str = "sk-do-not-materialize-me";
/// The environment token every fixture below references.
const ENV_TOKEN: &str = "${SECRET_API_KEY}";

const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

/// The class under test: one persist cell whose `params` reference the key the
/// environment owns.
fn cell_config() -> String {
    format!(
        r#"{{"cell":{{"type":"persist_mock","idle_timeout_ms":60000}},"params":{{"terminal":true,"api_key":"{ENV_TOKEN}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
    )
}

/// Prose, not a placeholder: the variable named here belongs to whoever
/// INSTANTIATES the class, and the registering colony need not know it at all.
const README: &str = "# note-unit\n\nSet `${NOBODY_HAS_THIS_ONE}` in your `.env` before you instantiate this class.\n";

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
}

/// A booted colony plus the channels a test talks to it through. Copied from
/// the gh443 suite rather than shared, for the same reason it was copied there:
/// a test helper two suites own is a third thing to keep in step with the door.
struct Colony {
    inbox_tx: mpsc::Sender<ColonyMsg>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    colony_config: ColonyConfig,
    join: tokio::task::JoinHandle<()>,
}

impl Colony {
    fn runtime(&self) -> ColonyRuntime {
        ColonyRuntime {
            inbox_tx: self.inbox_tx.clone(),
            outputs_tx: self.outputs_tx.clone(),
            colony_config: self.colony_config.clone(),
            blob_store: None,
        }
    }

    async fn shutdown(self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.inbox_tx
            .send(ColonyMsg::Shutdown { ack: ack_tx })
            .await
            .expect("send shutdown");
        ack_rx.await.expect("shutdown ack");
        self.join.await.expect("colony task");
    }
}

/// Boot a colony whose library is `templates_root` and whose `.env` carries the
/// sentinel — the very setup the issue was found on.
async fn boot_colony(root: &std::path::Path, templates_root: &std::path::Path) -> Colony {
    write(root, "main/config.json", HIVE_CONFIG);
    std::fs::write(root.join(".env"), format!("SECRET_API_KEY={SENTINEL}\n")).expect("write .env");
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let mut factories = CellFactoryRegistry::new();
    factories.insert("persist_mock".into(), persist_factory());
    let colony_config = ColonyConfig::default();
    let cfg = ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        factories,
        root.to_path_buf(),
        colony_config.clone(),
        None,
        None,
    )
    .with_templates_root(templates_root.to_path_buf());
    let join = tokio::spawn(colony_task(cfg));
    let colony = Colony {
        inbox_tx,
        outputs_tx,
        colony_config,
        join,
    };
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), persist_factory());
    meclaw_colony::bootstrap_from_filesystem(root, &reg, &colony.runtime())
        .await
        .expect("bootstrap");
    colony
}

async fn send_mutation(c: &Colony, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::MutationDoor {
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

fn assert_committed(outcome: &MutationDoorOutcome) {
    match outcome {
        MutationDoorOutcome::Single(MutationOutcome::Committed { .. })
        | MutationDoorOutcome::Manifest(ManifestOutcome::Committed { .. }) => {}
        other => panic!("the mutation must commit: {other:?}"),
    }
}

/// One `add_templates[]` entry: the class and its `template.json`. Without the
/// README, so that the leak half of the issue is proven on its own — with the
/// prose file in it, an unfixed colony refuses the whole mutation before it
/// ever writes the key anywhere.
fn declaration(name: &str) -> Value {
    json!({"name": name,
    "files": {
        "template.json": format!(r#"{{"name": "{name}", "version": "1.0.0"}}"#),
        "config.json": cell_config(),
    }})
}

/// The same entry plus the prose README — the second half of the issue.
fn declaration_with_readme(name: &str) -> Value {
    let mut entry = declaration(name);
    entry["files"]["README.md"] = json!(README);
    entry
}

/// Literal grep over every file under `dir`: the sentinel may appear nowhere.
///
/// Deliberately dumber than a JSON inspection (same reason as the GH #20
/// sweep): a key baked into a README, a seed file or a nested script string is
/// still a leak, and only a byte-level sweep catches all of them. The colony's
/// own `.env` is the one file that legitimately holds it.
fn assert_no_sentinel_under(dir: &std::path::Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        for entry in std::fs::read_dir(&p).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().is_some_and(|n| n == ".env") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read file");
            assert!(
                !String::from_utf8_lossy(&bytes).contains(SENTINEL),
                "the environment's key materialized into {}",
                path.display(),
            );
        }
    }
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The issue's first half: the registered `config.json` keeps its token, and
/// nothing anywhere carries the key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_config_keeps_its_environment_placeholder() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {"add_templates": [declaration("note-unit")]}}),
    )
    .await;
    assert_committed(&outcome);

    let registered = templates.join("local/note-unit@1.0.0/config.json");
    assert_eq!(
        read(&registered),
        cell_config(),
        "a registered template is byte-identical to the declaration",
    );
    assert_no_sentinel_under(td.path());

    colony.shutdown().await;
}

/// The issue's second half: a README that only MENTIONS a placeholder is not a
/// placeholder. Registering must neither fail nor rewrite the prose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_readme_that_only_mentions_a_placeholder_registers_untouched() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration_with_readme("note-unit")],
        }}),
    )
    .await;
    assert_committed(&outcome);
    assert_eq!(
        read(&templates.join("local/note-unit@1.0.0/README.md")),
        README,
        "prose about a variable is prose, not a binding",
    );

    colony.shutdown().await;
}

/// The third condition: an instance of such a class still binds the
/// environment. The token stands on disk (GH #20), and the late pass — the one
/// the boot runs on every read — resolves it to the environment's value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instance_of_a_registered_class_still_binds_the_environment() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration("note-unit")],
            "add_nodes": [{"name": "notes", "template": "note-unit@1.0.0"}],
        }}),
    )
    .await;
    assert_committed(&outcome);

    let instance = td.path().join("main/notes/config.json");
    let parsed: Value = meclaw_core::serde_json::from_str(&read(&instance)).expect("instance json");
    assert_eq!(
        parsed["params"]["api_key"], ENV_TOKEN,
        "the instance carries the token, not the value",
    );
    assert_no_sentinel_under(td.path());

    let env = [("SECRET_API_KEY".to_string(), SENTINEL.to_string())].into();
    let bound = meclaw_colony::mutation::substitute::substitute_env_only(&parsed, &env)
        .expect("the late pass resolves what the file kept");
    assert_eq!(
        bound["params"]["api_key"], SENTINEL,
        "reading the instance binds the environment's value, as at boot",
    );

    colony.shutdown().await;
}
