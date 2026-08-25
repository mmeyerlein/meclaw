//! GH #398 — a template instantiated by mutation must be WRITABLE afterwards.
//!
//! The gap this file closes is a shape, not a cell type. Every `web` test spawns
//! the cell through its factory on a hand-built directory, where the factory's
//! own DDL runs first and is therefore right. `gh163_web_cell_into_a_running_colony`
//! does instantiate by mutation — but it asserts a **GET** of the seeded page,
//! and a SELECT needs none of the constraints a schema carries. So nothing in
//! the suite instantiated a template by mutation and then wrote to it, and a
//! display shipped that could not take a `page.set` at all:
//!
//! ```text
//! error_code: invalid_input, operation: page.set
//! "ON CONFLICT clause does not match any PRIMARY KEY or UNIQUE constraint"
//! ```
//!
//! The cause was the mutation staging seeder building the cell's tables from the
//! header line of `seed/<table>.jsonl` alone — column names and a coarse type,
//! and nothing else a schema means: no keys, no NOT NULL, no defaults, no
//! indexes, alphabetical column order. For the `store` cell that is harmless,
//! because a store's schema genuinely IS declared per instance. The `web` cell's
//! schema is fixed in code on purpose (a display's tables are its contract with
//! its renderer), so staging got there first and the cell's own
//! `CREATE TABLE IF NOT EXISTS` then found the constraint-free tables standing
//! and left them exactly as they were.
//!
//! Two claims, and the second is what makes the first non-accidental:
//!
//! 1. **A display grown by mutation takes a write.** `page.set` is the op that
//!    needs the key, so it is the op that is sent.
//! 2. **Its schema is the one its factory declares, byte for byte** — compared
//!    against a database built the way the factory builds one, through
//!    `sqlite_master`, which carries the keys, the defaults and the index that a
//!    column list does not.

use meclaw_cells::web::WebCellFactory;
use meclaw_cells::web::db::setup_web_schema;
use meclaw_colony::persist::open_or_create_cell_db;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The node name of the instantiated display, and therefore its directory.
const NODE: &str = "display";

/// The repository root, so the test can install the **shipped** template rather
/// than a fixture that only looks like it.
fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A port nothing is listening on, obtained by binding and letting go.
///
/// `params.port` is required and immutable, so it is chosen before the cell
/// exists and written into the template the mutation instantiates.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    let port = l.local_addr().expect("read the bound address").port();
    drop(l);
    port
}

/// Copy a directory tree — the staging path copies a template's whole directory
/// into the new cell's, so the seed travels with it here too.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create the destination");
    for entry in std::fs::read_dir(from).expect("read the template dir") {
        let entry = entry.expect("read a directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a template file");
        }
    }
}

/// Install the shipped `templates/web/` into a temporary library, with one value
/// changed: the port. Everything else — the contract block, the seed, the
/// stylesheet — is the shipped file, because a fixture cannot ship a defect.
async fn install_web_template(td: &tempfile::TempDir, h: &ColonyHandle, port: u16) {
    let shipped = core_root().join("templates/web");
    assert!(
        shipped.join("config.json").is_file(),
        "templates/web must ship — it is a public template (W8 Task 11)"
    );
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join("web");
    copy_tree(&shipped, &tpl);

    let mut cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(tpl.join("config.json")).expect("read the shipped config"),
    )
    .expect("the shipped config is JSON");
    cfg["params"]["port"] = json!(port);
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the ported config");

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx.await.expect("rescan acked");
}

/// A colony with the `web` factory registered and a capture cell to answer into.
async fn colony_with_sink(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factory: Arc<dyn CellFactory> = Arc::new(WebCellFactory);
    let h = ColonyHandle::new_with_factories_at(td, vec![("web".to_string(), factory)]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    (h, sink_rx)
}

/// `add_nodes` from the shipped template — the step that runs the staging
/// seeder, and therefore the step under test.
async fn grow_the_display(h: &ColonyHandle) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": NODE, "template": "web@1.0.0"}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /{NODE} must commit; got {outcome:?}"
    );
    // The display answers over its out-edge, so the answer needs somewhere to go.
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("/{NODE}")),
        Path::new("/sink"),
    )
    .await;
}

/// GET the URL until it answers 200 or the deadline passes.
///
/// A fresh display has two start-up windows — nothing listening yet, and
/// listening with an empty page map — and a `page.set` publishes its new route
/// asynchronously behind the reply. So both uses here poll: what fails a claim
/// is a LASTING absence, and the 30 s failure-marker convention bounds it.
async fn wait_until_200(url: &str, what: &str) {
    let deadline = Instant::now() + RECV_TIMEOUT;
    loop {
        if let Ok(r) = reqwest::get(url).await
            && r.status().is_success()
        {
            return;
        }
        assert!(Instant::now() < deadline, "{what} ({url})");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One op as a `tool_call` turn, answered back to `/sink`.
fn op(args: Value) -> Message {
    MessageBuilder::new(Path::new(&format!("/{NODE}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": meclaw_core::serde_json::to_string(&args).unwrap(),
            "id": "call_1"
        }]})))
        .build()
}

/// Send one op and return the display's own answer: the `error_code` header (if
/// any) and the text of the `tool_result` turn.
async fn answer(
    h: &ColonyHandle,
    sink_rx: &mut mpsc::Receiver<Message>,
    args: Value,
) -> (Option<String>, String) {
    h.send(op(args.clone())).await;
    let m = tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("sink recv timeout: {args}"))
        .unwrap_or_else(|| panic!("sink channel closed: {args}"));
    let code = m
        .headers
        .hop
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let text = match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
        Body::Blob(_) => panic!("inline expected"),
    };
    (code, text)
}

/// Everything SQLite itself says about a database's schema: every table, index
/// and trigger it holds, with the statement that made it. This is the comparison
/// a column list cannot make — a key, a `NOT NULL`, a `DEFAULT` and an index all
/// live in this text and nowhere else.
///
/// The `sqlite_%` names are SQLite's own bookkeeping (an implicit index of a
/// `TEXT PRIMARY KEY` among them); they are excluded because they are derived
/// from the statements that ARE compared.
fn schema_of(db: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(db).expect("open the database");
    let mut st = conn
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .expect("prepare");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// A `cell.db` built the way [`WebCellFactory`] builds one: the substrate's own
/// tables, then the cell type's DDL. No seed — this is the schema reference, and
/// rows do not appear in `sqlite_master`.
fn factory_reference(dir: &std::path::Path) -> std::path::PathBuf {
    let db = dir.join("cell.db");
    let conn = open_or_create_cell_db(&db).expect("the substrate's own cell.db");
    setup_web_schema(&conn).expect("the web schema applies");
    drop(conn);
    db
}

// ─────────────────────────────────────────────────────────────── the claims

/// **The claim the shipped display failed.** A `page.set` against a display that
/// was instantiated by mutation is an upsert on `pages.route`, and SQLite
/// refuses an `ON CONFLICT` target that matches no key — so a display whose
/// table was built by the staging seeder could not be given a page at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_display_instantiated_by_mutation_accepts_a_page_set() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;
    let port = free_port();
    install_web_template(&td, &h, port).await;
    grow_the_display(&h).await;
    wait_until_200(
        &format!("http://127.0.0.1:{port}/"),
        "the instantiated display never served its seeded page",
    )
    .await;

    // `demo` is a root object the shipped seed carries, so the only thing that
    // can refuse this call is the table.
    let (code, text) = answer(
        &h,
        &mut sink_rx,
        json!({"op": "page.set", "route": "/second", "root": "demo", "title": "Second"}),
    )
    .await;
    assert_eq!(
        code, None,
        "page.set was refused by the display's own database: {text}"
    );

    // Twice, because an upsert that only works once is a table without a key
    // answering an INSERT.
    let (code, text) = answer(
        &h,
        &mut sink_rx,
        json!({"op": "page.set", "route": "/second", "root": "root", "title": "Second, again"}),
    )
    .await;
    assert_eq!(
        code, None,
        "a repeated page.set is a correction, not a second row: {text}"
    );

    wait_until_200(
        &format!("http://127.0.0.1:{port}/second"),
        "the route a page.set created never served",
    )
    .await;

    h.shutdown().await;
}

/// The general form: what a mutation builds is what the cell type declares.
/// Compared through `sqlite_master`, so a lost PRIMARY KEY, a lost `NOT NULL`, a
/// lost `DEFAULT`, a lost index and a reordered column list all fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_display_instantiated_by_mutation_has_the_schema_its_factory_declares() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, _sink_rx) = colony_with_sink(&td).await;
    let port = free_port();
    install_web_template(&td, &h, port).await;
    grow_the_display(&h).await;
    wait_until_200(
        &format!("http://127.0.0.1:{port}/"),
        "the instantiated display never served its seeded page",
    )
    .await;

    let reference_dir = tempfile::TempDir::new().unwrap();
    let expected = schema_of(&factory_reference(reference_dir.path()));
    let grown = schema_of(&td.path().join(NODE).join("cell.db"));

    assert_eq!(
        grown, expected,
        "a display grown by mutation must hold the schema its factory declares, \
         keys, defaults and index included"
    );

    h.shutdown().await;
}
