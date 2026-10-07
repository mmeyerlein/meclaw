//! GH #1061 (#801), lock T6 — a standalone template runs after one deposit.
//!
//! Since 0.62 a standalone template that spends a provider key carries its
//! own broker: `summarizer` ships `access/config.json` (a `ref` to
//! `access@2.5.1`) plus a SEED OVERLAY `access/store/seed/{grants,
//! grant_events}.jsonl` granting `./writer` the credential `cred:openrouter`
//! under `grant:openrouter@template-summarizer/writer`, the handle the writer's
//! `credential_grant_id` names. The promise to an operator is: instantiate it,
//! deposit the credential ONCE (`meclaw --vault-add cred:openrouter`), and it
//! runs — no `.env` key, no hand-written grant, no edge to draw.
//!
//! The lock instantiates the SHIPPED template through the mutation door (so
//! the overlay is laid by `overlay_ref_seeds`, not by the test), points the
//! writer at a loopback provider, deposits once through the user channel
//! (`vault::user_channel::add`, what `--vault-add` calls) and sends one batch
//! into the hive's `in_batch` lane. The measure is at the receiver: the bearer
//! on the writer's provider call.
//!
//! OR-VG.V4.D2 — the vault self-unlocks from `key_source: "systemd-cred"`
//! (`$CREDENTIALS_DIRECTORY/vault_key`), not from `plainfile`: `key_file` is
//! not a param of the shipped `access/vault`, so the mutation door refuses an
//! `override_params` entry that names it (GH #294). `key_source` is shipped,
//! so the systemd source is what the template's shape allows.
//!
//! No stub value may reach a log line or a `message_log` row.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::vault::{VaultCellFactory, user_channel};
use meclaw_cells::{LlmCellFactory, TimerCellFactory};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::oneshot;

const SECRET: &str = "stub-secret-6";
const PASSPHRASE: &str = "stub-secret-passphrase-1061-t6";
const CRED_REF: &str = "cred:openrouter";
const GRANT: &str = "grant:openrouter@template-summarizer/writer";
const VAULT: &str = "/main/summarizer/access/vault";

// ─────────────────────────────────────────────────────────── log capture

/// Every tracing event of this test process, formatted with its field values.
/// Global, because the cells run on worker threads; nextest gives every test
/// its own process.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut f = Fields(event.metadata().target().to_string());
        event.record(&mut f);
        self.0.lock().unwrap().push(f.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn capture() -> Capture {
    static C: OnceLock<Capture> = OnceLock::new();
    C.get_or_init(|| {
        let c = Capture::default();
        let _ = tracing::subscriber::set_global_default(c.clone());
        c
    })
    .clone()
}

// ─────────────────────────────────────────────────────────── the material

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/summarizer/template.json",
        "templates/summarizer/access/config.json",
        "templates/summarizer/access/store/seed/grants.jsonl",
        "templates/access/template.json",
    ]
    .iter()
    .all(|f| repo(f).is_file())
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// An empty root hive, the two templates in the library, and a `.env` that
/// names nothing a cell could spend.
fn build_root(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    for name in ["summarizer", "access"] {
        copy_tree(
            &repo("templates").join(name),
            &root.join("templates").join(name),
        );
    }
    std::fs::write(root.join(".env"), "GH1061_UNRELATED=1\n").unwrap();
}

/// The systemd credential the vault opens itself from: `vault_key`, owner-only.
/// `set_var` is unsafe in edition 2024; sound here because nextest runs this
/// test in its own process and the variable is written before any vault exists.
fn arm_credentials_directory(dir: &std::path::Path) {
    let key = dir.join("vault_key");
    std::fs::write(&key, format!("{PASSPHRASE}\n")).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", dir) };
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        ("vault".to_string(), Arc::new(VaultCellFactory)),
    ]
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories();
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the colony boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan aborted");
    h
}

/// The one instantiation: the shipped template, its writer on the stub, its
/// vault on the systemd source, its batch port wired. No grant, no credential
/// edge, no store row in it.
async fn instantiate(h: &ColonyHandle, base_url: &str) {
    let payload = json!({
        "scope": "/",
        "ctx": {"model": "stub-model"},
        "diff": {"add_nodes": [{
            "name": "summarizer", "template": "summarizer",
            "override_params": {
                "writer": {"base_url": base_url},
                "access/vault": {"key_source": "systemd-cred"}
            }
        }],
        // The hive's own port, wired in the same mutation as its README asks:
        // an island without a crossing edge derives inactive and never spawns
        // (`CellInactive`). It is the batch's way in, not a credential edge.
        "add_edges": [{"from": ".", "to": "./summarizer",
                       "condition": "has(hop.route) && hop.route == 'in_batch'"}]}
    });
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
    let outcome = ack_rx.await.expect("mutation ack");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the shipped template instantiates: {outcome:?}"
    );
}

/// One closed session in the collector's write-batch form, into `in_batch`.
async fn send_batch(h: &ColonyHandle) {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_batch"));
    hop.insert("session_id".into(), json!("s1"));
    hop.insert("turn_count".into(), json!("2"));
    hop.insert("round_count".into(), json!("0"));
    h.send(
        MessageBuilder::new(Path::new("/summarizer"))
            .hop(hop)
            .body(Body::Inline(json!({
                "messages": [
                    {"origin": "user", "type": "text", "text": "hello"},
                    {"origin": "assistant", "type": "text", "text": "hi"}
                ],
                "rounds": []
            })))
            .ttl(400)
            .build(),
    )
    .await;
}

/// The grant rows the placed tree's broker store will seed on its first wake.
/// A `store` spawns lazily, so before a message reaches it the overlay is a
/// claim about the SEED FILE in the placed tree (as in `gh452_*`).
fn seeded_grants(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("main/summarizer/access/store/seed/grants.jsonl"))
        .unwrap_or_default()
}

fn log_rows(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
        .expect("message_log");
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "stub-model",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "a short summary"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

// ═══════════════════════════════════════════════════════════════════ the lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_a_standalone_template_runs_after_one_deposit() {
    if !shipped() {
        eprintln!("skipped: templates/summarizer or templates/access did not ship (GH #49)");
        return;
    }
    let logs = capture();
    let creds = tempfile::TempDir::new().unwrap();
    arm_credentials_directory(creds.path());
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());

    // 1. Instantiate. The grant comes with the template — the overlay.
    let h = boot(&td).await;
    instantiate(&h, &format!("http://{addr}/v1")).await;
    assert!(
        seeded_grants(td.path()).contains(GRANT),
        "the overlay laid the template's grant into the placed broker store"
    );
    h.shutdown().await;

    // 2. The one deposit, with no colony running.
    user_channel::add(
        td.path(),
        VAULT,
        CRED_REF,
        SECRET.as_bytes(),
        PASSPHRASE.as_bytes(),
    )
    .expect("the one deposit");

    // 3. Boot again, one batch in, and the writer authenticates.
    let h = boot(&td).await;
    send_batch(&h).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !captured.try_lock().map(|c| !c.is_empty()).unwrap_or(false) {
        if tokio::time::Instant::now() >= deadline {
            // What the road did instead, so a red run names its own cause:
            // the routed rows (headers only) and the log lines of the hive.
            let rows: Vec<String> = log_rows(td.path())
                .into_iter()
                .map(|r| r.chars().take(400).collect())
                .collect();
            let lines: Vec<String> = logs
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("meclaw"))
                .map(|l| l.chars().take(400).collect())
                .collect();
            let dead: Vec<String> = h
                .drain_dead_letters()
                .await
                .iter()
                .map(|d| {
                    format!(
                        "{:?} -> {:?}: {:?}",
                        d.sender_path, d.resolved_target, d.reason
                    )
                    .chars()
                    .take(900)
                    .collect()
                })
                .collect();
            panic!(
                "the writer's provider call never happened\nrows (last 40):\n{}\nlog (last 60):\n{}\ndead letters:\n{}",
                rows[rows.len().saturating_sub(40)..].join("\n"),
                lines[lines.len().saturating_sub(60)..].join("\n"),
                dead.join("\n")
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let auth: Vec<String> = captured
        .lock()
        .await
        .iter()
        .map(|r| r.headers.get("authorization").cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        auth.first().map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the writer presents the deposited credential: {auth:?}"
    );
    h.shutdown().await;

    let hits: Vec<String> = log_rows(td.path())
        .into_iter()
        .filter(|r| r.contains(SECRET) || r.contains(PASSPHRASE))
        .collect();
    assert!(hits.is_empty(), "the credential is on record: {hits:?}");
    let lines = logs.0.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .all(|l| !l.contains(SECRET) && !l.contains(PASSPHRASE)),
        "the credential is in a log line"
    );
}
