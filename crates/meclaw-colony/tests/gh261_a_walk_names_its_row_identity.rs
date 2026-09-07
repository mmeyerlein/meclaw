//! GH #261 — a walk over more than one table can say what makes a row THE SAME
//! row.
//!
//! `key` belongs to ONE table's identity, so the substrate only carried it when
//! the call named one table. That left the whole-directory form — the form that
//! exists so a broken file writes *nothing* — unusable for the cells that need
//! it most: a `store` declares its tables in `params.schema`, and that
//! declaration cannot express a PRIMARY KEY. Every such table therefore has an
//! empty key, and an `import` refuses it by name rather than duplicate rows.
//!
//! The only way out was one call per table, and that trades the property away:
//! a malformed file halfway down the walk leaves every table before it standing.
//!
//! `keys` closes it. `{"<table>": ["<col>", …]}` beside `tables`, one call, one
//! parse of the whole directory before the first write — and one TRANSACTION
//! over every table of that call, so a refusal anywhere leaves nothing behind.

use meclaw_colony::{DbConn, build_stateful_task_with_peace};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, TransferBounds};
use std::sync::Arc;
use tokio::sync::mpsc;

const CELL: &str = "/main/memory/store";
const SENDER: &str = "/main/memory/porter";

/// Two tables the way a `store` declares them: no PRIMARY KEY anywhere, because
/// `params.schema` carries column names and a coarse type and nothing else.
const KEYLESS: &str = "CREATE TABLE episodes (id TEXT, audience_set TEXT, content TEXT);
     CREATE TABLE facts (id TEXT, audience_set TEXT, claim TEXT);
     INSERT INTO episodes VALUES ('e1', 'a,b', 'first'), ('e2', '', 'second');
     INSERT INTO facts VALUES ('f1', 'a,b', 'sky is blue');";

struct Live {
    mailbox: mpsc::Sender<Message>,
    out: mpsc::Receiver<CellEmission>,
    #[allow(dead_code)]
    join: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}

impl Live {
    async fn start(bounds: TransferBounds, ddl: &str) -> Live {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.path().join("cell.db"))
            .expect("cell.db");
        conn.execute_batch(ddl).expect("fixture DDL");
        let db = DbConn::wrap(conn, None);

        let (mailbox, mb_rx) = mpsc::channel::<Message>(8);
        let (otx, out) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel::<meclaw_colony::ColonyMsg>(8);
        let cell = meclaw_testing::mocks::PersistMockCell::from_params(&json!({"terminal": true}))
            .expect("mock cell");

        let (join, _peace, _stop, _ack, _backstop) = build_stateful_task_with_peace(
            Path::new(CELL),
            mb_rx,
            otx,
            inbox_tx,
            None,
            None,
            0,
            cell,
            db,
            None,
            None,
            bounds,
        );
        Live {
            mailbox,
            out,
            join,
            dir,
        }
    }

    async fn send(&mut self, slot: Value) -> Value {
        self.mailbox
            .send(
                MessageBuilder::new(Path::new(CELL))
                    .reply_to(Path::new(SENDER))
                    .body(Body::Inline(json!({"transfer": slot})))
                    .build(),
            )
            .await
            .expect("mailbox open");
        tokio::time::timeout(std::time::Duration::from_secs(30), self.out.recv())
            .await
            .expect("the substrate must answer a transfer slot within 30s")
            .expect("the substrate must answer a transfer slot")
            .content
    }

    async fn finish(self) -> tempfile::TempDir {
        drop(self.mailbox);
        drop(self.out);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), self.join).await;
        self.dir
    }
}

fn payload(reply: &Value) -> Value {
    let text = reply["messages"][0]["text"].as_str().unwrap_or_default();
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

fn code(reply: &Value) -> Option<String> {
    reply["header"]["error_code"]
        .as_str()
        .map(|s| s.to_string())
}

fn fenced(base: &std::path::Path) -> TransferBounds {
    TransferBounds {
        base_path: Some(Arc::from(base)),
        ..TransferBounds::default()
    }
}

const WALK: [&str; 2] = ["episodes", "facts"];

fn keys() -> Value {
    json!({"episodes": ["id"], "facts": ["id"]})
}

/// One directory out, one call back in, and every row identified by what the
/// caller said identifies it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_call_carries_the_order_and_the_row_identity_of_a_whole_walk() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut src = Live::start(fenced(fence.path()), KEYLESS).await;
    let out = src
        .send(json!({"operation": "export", "to": "run", "tables": WALK}))
        .await;
    assert_eq!(code(&out), None, "the export must not be refused: {out}");
    src.finish().await;

    let mut dst = Live::start(fenced(fence.path()), KEYLESS_EMPTY).await;
    let reply = dst
        .send(json!({"operation": "import", "from": "run", "tables": WALK, "keys": keys()}))
        .await;
    assert_eq!(
        code(&reply),
        None,
        "a walk that names its row identity must not be refused: {reply}"
    );
    let got = payload(&reply);
    assert_eq!(got["rows_written"], 3);
    assert_eq!(got["rows_skipped"], 0);

    // and applying the same directory twice writes nothing the second time —
    // which is the whole reason the key had to travel.
    let again = dst
        .send(json!({"operation": "import", "from": "run", "tables": WALK, "keys": keys()}))
        .await;
    assert_eq!(code(&again), None);
    assert_eq!(payload(&again)["rows_written"], 0);
    assert_eq!(payload(&again)["rows_skipped"], 3);

    let dir = dst.finish().await;
    let conn = rusqlite::Connection::open(dir.path().join("cell.db")).unwrap();
    let episodes: i64 = conn
        .query_row("SELECT count(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    let audiences: Vec<String> = conn
        .prepare("SELECT audience_set FROM episodes ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(episodes, 2);
    assert_eq!(
        audiences,
        vec!["a,b".to_string(), String::new()],
        "an audience that is present but EMPTY travels as it stands"
    );
}

const KEYLESS_EMPTY: &str = "CREATE TABLE episodes (id TEXT, audience_set TEXT, content TEXT);
     CREATE TABLE facts (id TEXT, audience_set TEXT, claim TEXT);";

/// Without the argument the walk is refused, by the name of the first table it
/// cannot identify a row in — and nothing at all is written, not even for the
/// tables that came before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_walk_over_keyless_tables_without_keys_is_refused_by_name() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut src = Live::start(fenced(fence.path()), KEYLESS).await;
    let out = src
        .send(json!({"operation": "export", "to": "run", "tables": WALK}))
        .await;
    assert_eq!(code(&out), None, "{out}");
    src.finish().await;

    let mut dst = Live::start(fenced(fence.path()), KEYLESS_EMPTY).await;
    let reply = dst
        .send(json!({"operation": "import", "from": "run", "tables": WALK}))
        .await;
    assert_eq!(code(&reply).as_deref(), Some("invalid_input"), "{reply}");
    assert!(
        reply["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("has no primary key"),
        "the refusal has to say WHICH table it could not identify a row in: {reply}"
    );
    dst.finish().await;
}

/// Three typos a caller wants to hear about, each by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keys_that_cannot_mean_anything_are_refused_before_a_row_is_written() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut src = Live::start(fenced(fence.path()), KEYLESS).await;
    assert_eq!(
        code(
            &src.send(json!({"operation": "export", "to": "run", "tables": WALK}))
                .await
        ),
        None
    );
    src.finish().await;

    let mut dst = Live::start(fenced(fence.path()), KEYLESS_EMPTY).await;
    for (slot, says) in [
        (
            json!({"operation": "import", "from": "run", "tables": WALK,
                   "key": ["id"], "keys": keys()}),
            "key and keys cannot both be given",
        ),
        (
            json!({"operation": "import", "from": "run", "tables": WALK,
                   "keys": {"episodes": "id", "facts": ["id"]}}),
            "must be an array of column names",
        ),
        (
            json!({"operation": "import", "from": "run", "tables": WALK,
                   "keys": {"episodes": ["id"], "facts": ["id"], "topics": ["id"]}}),
            "which this call does not address",
        ),
    ] {
        let reply = dst.send(slot).await;
        assert_eq!(code(&reply).as_deref(), Some("invalid_input"), "{reply}");
        assert!(
            reply["messages"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains(says),
            "the refusal must say {says:?}: {reply}"
        );
    }

    let dir = dst.finish().await;
    let conn = rusqlite::Connection::open(dir.path().join("cell.db")).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "a refused argument may not have written a row");
}

/// The property the one-call form exists for: a malformed file anywhere in the
/// directory writes nothing at all — not even for the tables ahead of it in the
/// walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_file_in_the_set_leaves_no_prefix_behind() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut src = Live::start(fenced(fence.path()), KEYLESS).await;
    assert_eq!(
        code(
            &src.send(json!({"operation": "export", "to": "run", "tables": WALK}))
                .await
        ),
        None
    );
    src.finish().await;
    // `facts` is the SECOND table of the walk; `episodes` ahead of it is whole.
    std::fs::write(fence.path().join("run/seed/facts.jsonl"), "not json\n").unwrap();

    let mut dst = Live::start(fenced(fence.path()), KEYLESS_EMPTY).await;
    let reply = dst
        .send(json!({"operation": "import", "from": "run", "tables": WALK, "keys": keys()}))
        .await;
    assert_eq!(
        code(&reply).as_deref(),
        Some("transfer_seed_malformed"),
        "{reply}"
    );
    let dir = dst.finish().await;
    let conn = rusqlite::Connection::open(dir.path().join("cell.db")).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        n, 0,
        "the table ahead of the broken one must not have landed — that is the \
         whole difference between one call and one call per table"
    );
}

/// The other half of "whole or nothing", and the half a parse cannot buy: the
/// column-set gate runs while the rows are being applied, so a document whose
/// ninth table disagrees with the target must roll back the eight before it.
///
/// Until this issue it was one transaction PER TABLE. The prefix that left
/// behind was invisible to the caller, because the receipt of a refused import says
/// `rows_affected = 0` — the worst shape a partial write can have: a document
/// that reports it did nothing, and a target that carries half of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_table_refused_late_rolls_back_the_tables_before_it() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut src = Live::start(fenced(fence.path()), KEYLESS).await;
    assert_eq!(
        code(
            &src.send(json!({"operation": "export", "to": "run", "tables": WALK}))
                .await
        ),
        None
    );
    src.finish().await;

    // `facts` is the SECOND table of the walk, and its header now declares a
    // column this target does not have — the source is newer, which is the
    // refusal an import may never resolve by guessing. `episodes`, ahead of it,
    // is whole and would apply cleanly on its own.
    let path = fence.path().join("run/seed/facts.jsonl");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let mut header: Value = meclaw_core::serde_json::from_str(&lines[0]).unwrap();
    header["schema"]["a_column_from_the_future"] = json!("text");
    lines[0] = meclaw_core::serde_json::to_string(&header).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let mut dst = Live::start(fenced(fence.path()), KEYLESS_EMPTY).await;
    let reply = dst
        .send(json!({"operation": "import", "from": "run", "tables": WALK, "keys": keys()}))
        .await;
    assert_eq!(
        code(&reply).as_deref(),
        Some("import_schema_drift"),
        "the refusal must name the case: {reply}"
    );
    assert!(
        reply["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("a_column_from_the_future"),
        "and the column that could not travel: {reply}"
    );

    let dir = dst.finish().await;
    let conn = rusqlite::Connection::open(dir.path().join("cell.db")).unwrap();
    for table in WALK {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            n, 0,
            "{table} carries rows after a refused document. A document applies whole \
             or not at all -- and the receipt of this refusal says `rows_affected = 0`, \
             so a prefix here is a prefix nobody can see"
        );
    }
}
