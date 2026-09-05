//! GH #553 — the two shipped examples that HAVE a consumer follow the receipt.
//!
//! `gh553_a_committed_mutation_refreshes_menu_and_screen.rs` measures the
//! mechanism on a fixture of its own: one hive, one agent, one app. What it
//! cannot measure is the part that only exists in the tree a reader boots — the
//! CHAIN, drawn one level at a time through four shipped levels, and the
//! example that puts a picture on a screen without a timer.
//!
//! Two runs, one claim each:
//!
//! 1. **`examples/organism`** — the whole stack, grown from the shipped
//!    manifest, with `mutation_receipts` opted in at its own seed. The receipt
//!    of the growing mutation has to travel `/os -> ./orgs -> acme ->
//!    ./members -> alex -> ./assistants -> scribe -> ./talky -> ./collector`
//!    and be VISIBLE there: the audit is asked for the delivery, so the claim
//!    is a positive receipt and not the absence of a dead letter.
//! 2. **`examples/display-colony-view`** — the app example, booted for real and
//!    asked over HTTP. Nothing in that colony ticks for the app any more: the
//!    mutation that grows the screen leaves the receipt that draws the first
//!    picture, and the screen serves it with `200`.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! example or the library is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RespawnFn, SpawnedCellKind, WakeFn,
    bootstrap_from_filesystem,
};
use meclaw_core::JsonValue;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::free_port;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
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

async fn rescan(h: &ColonyHandle, root: &std::path::Path) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
}

async fn mutate(h: &ColonyHandle, payload: Value) -> meclaw_colony::mutation::MutationOutcome {
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

// ═══════════════════════════════════════════ 1. the chain, over four levels

/// A cell that accepts everything and emits nothing.
///
/// Same device and same reason as `gh422_the_manifest_grows_the_same_stack.rs`:
/// the claim here is a ROUTING claim, and two of the cell types this stack
/// names would reach outward the moment they were spawned for real. What is
/// measured — where a message was delivered — is the substrate's own audit and
/// does not depend on what the cell did with it.
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

/// Every delivery the audit booked at `to_path`, with the lane it arrived on.
fn deliveries(root: &std::path::Path, to_path: &str) -> Vec<String> {
    let db = root.join("colony.db");
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&db) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT headers FROM message_log WHERE to_path = ?") else {
        return Vec::new();
    };
    let rows = stmt.query_map([to_path], |r| r.get::<_, String>(0));
    let Ok(rows) = rows else { return Vec::new() };
    rows.filter_map(|r| r.ok())
        .filter_map(|h| meclaw_core::serde_json::from_str::<Value>(&h).ok())
        .filter_map(|h| h["hop"]["route"].as_str().map(str::to_string))
        .collect()
}

/// Every path the receipt lane was ever delivered to — the diagnostic a broken
/// chain needs: it says which level dropped it.
fn where_the_lane_went(root: &std::path::Path) -> Vec<String> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT to_path FROM message_log WHERE headers LIKE '%mutation_committed%' ORDER BY rowid",
    ) else {
        return Vec::new();
    };
    match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

const SCRIBE_COLLECTOR: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/collector";

/// Claim 1. The receipt of the growing mutation reaches the collector inside
/// the assistant — four levels and two containers below the hive it entered at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_organism_receipt_reaches_the_collector_inside_the_assistant() {
    if !repo("examples/organism/grow.manifest.json").is_file() || !repo("templates").is_dir() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    // The keeper's nightly close sweep, pushed to a date this run cannot reach.
    // It was a `KEEPER_NIGHT_CRON` line in the `.env` below until GH #138: the
    // schedule is a LITERAL of `session-keeper/night`'s own params now, so such
    // a line is read by nothing at all -- the sweep would fire into this run and
    // nobody would say so. The library copy is this tree's own, so writing the
    // key into it is what an `override_params` entry does to a staged config
    // (`crates/meclaw-cells/tests/gh138_keeper_summarizer_dispatcher_params.rs`
    // is the proof that the timer plans on what it finds there).
    meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nMODEL_BRAIN=gpt-4o-mock\nMODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock\nMODEL_SURFACE=gpt-4o-mock\nMODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\nMODEL_DREAMER=gpt-4o-mock\nTELEGRAM_BOT_TOKEN=t\n\
         TELEGRAM_BOT_TOKEN_2=t2\nTELEGRAM_ALLOWED_USER_ID=0\nEXAMPLE_CHAT_TOKEN=c\n",
    )
    .unwrap();

    // The example opts in at its own seed, which is the point of the run: the
    // key is read off the shipped file rather than written here.
    assert_eq!(
        read_json(&root.join("colony.json"))["mutation_receipts"]["to"],
        json!("/os"),
        "examples/organism/seed/colony.json has to opt into the receipt: its \
         colony-view app has no other producer since colony-view@1.1.0"
    );

    let fs: Vec<(String, Arc<dyn CellFactory>)> = cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect();
    let h = ColonyHandle::new_with_factories_at(&td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the empty seed of examples/organism must boot");
    rescan(&h, root).await;

    // The five shipped declarations, in the order the example applies them --
    // each one a mutation of its own, so each one leaves a receipt of its own.
    for file in [
        "grow-os.json",
        "grow-org.json",
        "grow-member.json",
        "grow-assistant.json",
        "grow-screen.json",
    ] {
        let payload = read_json(&repo("examples/organism").join(file));
        let entries = payload
            .get("manifest")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_else(|| vec![payload.clone()]);
        for entry in entries {
            let outcome = mutate(&h, entry).await;
            assert!(
                matches!(
                    outcome,
                    meclaw_colony::mutation::MutationOutcome::Committed { .. }
                ),
                "precondition: {file} commits; got {outcome:?}"
            );
        }
    }

    // Positive receipt: the audit booked a delivery AT the collector's own hive
    // path, and it arrived on the lane the mutation door stamps.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let lanes = deliveries(root, SCRIBE_COLLECTOR);
        if lanes.iter().any(|l| l == "mutation_committed") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the receipt never reached {SCRIBE_COLLECTOR}; what the audit booked \
             there is {lanes:?}. Everywhere the lane WAS booked: {:?}. The chain \
             is os -> ./orgs -> acme -> ./members -> alex -> ./assistants -> \
             scribe -> ./talky -> ./collector, and every level of it has to \
             declare `mutation_committed` and draw one edge for it",
            where_the_lane_went(root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The app half of the same fan-out: the member carries it into `./apps` too.
    let app = "/os/orgs/acme/members/alex/apps/colony-view";
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if deliveries(root, app)
            .iter()
            .any(|l| l == "mutation_committed")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the receipt never reached the member's app at {app}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ══════════════════════════════════ 2. the app example, booted and asked

/// Claim 2. `examples/display-colony-view` really puts a picture on its screen,
/// and nothing in it ticks for the app.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_display_example_draws_its_first_picture_from_the_grow_receipt() {
    let ex = repo("examples/display-colony-view");
    if !ex.join("grow.json").is_file() || !repo("templates/colony-view").is_dir() {
        return;
    }
    assert!(
        !repo("templates/colony-view/refresh").exists(),
        "colony-view has a timer again; this example's claim is that it does not need one"
    );

    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&ex.join("seed"), root);
    for name in [
        "display",
        "colony-view",
        "terminal",
        "web",
        "canvy",
        "clock",
    ] {
        let src = repo(&format!("templates/{name}"));
        if src.is_dir() {
            copy_tree(&src, &root.join("templates").join(name));
        }
    }
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();

    // The example opts in, and the key is read off the shipped file.
    assert_eq!(
        read_json(&root.join("colony.json"))["mutation_receipts"]["to"],
        json!("/"),
        "examples/display-colony-view/seed/colony.json has to opt in: the app has \
         no other producer"
    );

    // The one thing the run has to bend: the shipped port is a fixed 7899, and a
    // test that took it would collide with a second run on the same box.
    let port = free_port();
    let mut grow = read_json(&ex.join("grow.json"));
    for node in grow["manifest"][0]["diff"]["add_nodes"]
        .as_array_mut()
        .expect("add_nodes")
    {
        if node["name"] == json!("display") {
            node["override_params"]["web"]["port"] = json!(port);
        }
    }

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("web".to_string(), Arc::new(WebCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the seed of examples/display-colony-view must boot");
    rescan(&h, root).await;

    for entry in grow["manifest"]
        .as_array()
        .expect("the shipped manifest")
        .clone()
    {
        let outcome = mutate(&h, entry).await;
        assert!(
            matches!(
                outcome,
                meclaw_colony::mutation::MutationOutcome::Committed { .. }
            ),
            "precondition: the shipped declaration commits; got {outcome:?}"
        );
    }

    // The receipt of THAT mutation is what draws the first picture. No tick, no
    // `in_refresh`, nothing sent by hand.
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    loop {
        if let Ok(resp) = reqwest::get(&url).await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            if status == 200 && body.contains("colony-view") {
                return;
            }
            last = format!("status {status}, {} bytes", body.len());
        }
        assert!(
            Instant::now() < deadline,
            "the screen never served the topology picture on {url}: {last}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
