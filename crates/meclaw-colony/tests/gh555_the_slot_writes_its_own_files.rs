//! GH #555 — the `transfer` slot writes and reads its own files, and only
//! inside its own fence.
//!
//! Until now a document left a cell as a message and arrived as a message; the
//! file half was done by an interim script cell standing beside the store. The
//! ruling (R-0904-3, the owner: *"cells manage their own files, nobody else does"*)
//! moved it where it belongs: `{"operation": "export", "to": …}` writes
//! `<dir>/seed/<table>.jsonl` itself, `{"operation": "import", "from": …}` reads
//! the same directory back, and both are bounded by ONE fence the cell declares
//! for itself — `params.transfer.base_path`.
//!
//! Everything here drives the real seam: `build_stateful_task_with_peace` →
//! `cell_task_stateful` → `db_transfer::handle_transfer_slot`, on a real
//! `cell.db`, with a real directory on disk. The message form is untouched and
//! stays the default when no path is named — that is pinned in
//! `gh253_db_transfer.rs` and in this module's own unit tests.

use meclaw_colony::{DbConn, build_stateful_task_with_peace};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, TransferBounds};
use std::sync::Arc;
use tokio::sync::mpsc;

const CELL: &str = "/main/memory/store";
const INSIDE: &str = "/main/memory/curator";
const OUTSIDE: &str = "/main/intruder";

/// One live cell on a real `cell.db`, spawned through the substrate's own path
/// and kept open, so a test can send more than one slot to the same cell.
struct Live {
    mailbox: mpsc::Sender<Message>,
    out: mpsc::Receiver<CellEmission>,
    join: tokio::task::JoinHandle<()>,
    /// The cell's own directory — held so the `cell.db` outlives the task.
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

    /// Send one `transfer` slot from `sender` and return the substrate's reply.
    async fn send_from(&mut self, sender: Option<&str>, slot: Value) -> Value {
        let mut b = MessageBuilder::new(Path::new(CELL)).body(Body::Inline(json!({
            "transfer": slot
        })));
        if let Some(s) = sender {
            b = b.reply_to(Path::new(s));
        }
        self.mailbox.send(b.build()).await.expect("mailbox open");
        tokio::time::timeout(std::time::Duration::from_secs(30), self.out.recv())
            .await
            .expect("the substrate must answer a transfer slot within 30s")
            .expect("the substrate must answer a transfer slot")
            .content
    }

    async fn send(&mut self, slot: Value) -> Value {
        self.send_from(Some(INSIDE), slot).await
    }

    /// Close the mailbox and wait for the task, so `cell.db` is closed before a
    /// test reads the tree.
    async fn finish(self) -> tempfile::TempDir {
        drop(self.mailbox);
        drop(self.out);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), self.join).await;
        self.dir
    }
}

/// The reply's `tool_result` payload, parsed back out of its text.
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

const NOTES: &str = "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);
                     INSERT INTO notes VALUES ('a', 'first', 1), ('b', 'second', 2);";

// ---------------------------------------------------------------------------
// The fence
// ---------------------------------------------------------------------------

/// A cell that named no fence has no place to write, and says so by name. It
/// does not fall back to the cell's own tree, to a temp directory or to the
/// working directory — every one of those would be a second output channel no
/// edge carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_without_a_declared_base_path_refuses_every_named_path() {
    let mut live = Live::start(TransferBounds::default(), NOTES).await;
    let reply = live
        .send(json!({"operation": "export", "to": "anywhere"}))
        .await;
    assert_eq!(code(&reply).as_deref(), Some("transfer_path_out_of_bounds"));
    assert_eq!(reply["header"]["rows_affected"], 0);

    let import = live
        .send(json!({"operation": "import", "from": "anywhere"}))
        .await;
    assert_eq!(
        code(&import).as_deref(),
        Some("transfer_path_out_of_bounds")
    );

    let dir = live.finish().await;
    assert!(
        !dir.path().join("anywhere").exists(),
        "a refused path must leave no directory behind"
    );
    assert!(!dir.path().join("seed").exists());
}

/// Every way out of the fence answers the same way — and it answers the same
/// whether the target outside exists or not. That identity IS the closed
/// oracle (the shape GH #107 built for the `file` cell's boundary).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_climbs_out_of_the_bound_is_refused_by_name() {
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("fence");
    std::fs::create_dir(&base).unwrap();
    std::fs::create_dir(outer.path().join("exists")).unwrap();

    let mut live = Live::start(fenced(&base), NOTES).await;
    for rel in [
        "../elsewhere",
        "/etc",
        "sub/../../x",
        // The oracle pair: one target outside exists, one does not.
        "../exists",
        "../missing",
    ] {
        let reply = live
            .send(json!({"operation": "export", "to": rel, "table": "notes"}))
            .await;
        assert_eq!(
            code(&reply).as_deref(),
            Some("transfer_path_out_of_bounds"),
            "{rel} must be refused by name: {reply}"
        );
        let import = live.send(json!({"operation": "import", "from": rel})).await;
        assert_eq!(
            code(&import).as_deref(),
            Some("transfer_path_out_of_bounds"),
            "{rel} must be refused on the read side too: {import}"
        );
    }
    live.finish().await;

    assert!(
        std::fs::read_dir(outer.path().join("exists"))
            .unwrap()
            .next()
            .is_none(),
        "nothing may be written outside the fence"
    );
    assert!(!outer.path().join("elsewhere").exists());
    assert!(!outer.path().join("missing").exists());
}

/// The hole a check that stops one component short leaves open: `<fence>/x`
/// stays inside, and `<fence>/x/seed` is a symlink pointing out. The bytes land
/// in `<dir>/seed`, so `<dir>/seed` is what the fence has to resolve — checking
/// `<dir>` alone let `create_dir_all` follow the link and write every file,
/// marker included, outside a fence the check had just called intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_directory_that_is_a_symlink_out_of_the_fence_is_refused() {
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("fence");
    let outside = outer.path().join("outside");
    std::fs::create_dir(&base).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::create_dir(base.join("x")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, base.join("x/seed")).unwrap();
    #[cfg(not(unix))]
    return;

    let mut live = Live::start(fenced(&base), NOTES).await;
    let export = live
        .send(json!({"operation": "export", "to": "x", "table": "notes"}))
        .await;
    assert_eq!(
        code(&export).as_deref(),
        Some("transfer_path_out_of_bounds"),
        "a seed directory that resolves out of the fence is out of the fence: {export}"
    );
    let import = live.send(json!({"operation": "import", "from": "x"})).await;
    assert_eq!(
        code(&import).as_deref(),
        Some("transfer_path_out_of_bounds"),
        "and the read side answers the same: {import}"
    );
    live.finish().await;

    assert!(
        std::fs::read_dir(&outside).unwrap().next().is_none(),
        "not one file may appear at the symlink's target"
    );
}

/// The `--validate` promise, as a test: a cell whose fence is not there yet
/// boots and validates cleanly (`gh555_the_base_path_reaches_the_view`), and the
/// refusal arrives on the MESSAGE, as an I/O error naming the directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_base_path_that_is_not_there_is_an_io_error_at_the_message_not_at_the_boot() {
    let outer = tempfile::TempDir::new().unwrap();
    let absent = outer.path().join("never-created");

    let mut live = Live::start(fenced(&absent), NOTES).await;
    let reply = live
        .send(json!({"operation": "export", "to": "", "table": "notes"}))
        .await;
    assert_eq!(
        code(&reply).as_deref(),
        Some("transfer_io_error"),
        "a fence that is not there is an I/O fact, not a boundary violation: {reply}"
    );
    live.finish().await;
    assert!(
        !absent.exists(),
        "the substrate creates the fence itself as little as it creates a colony root"
    );
}

// ---------------------------------------------------------------------------
// The write half
// ---------------------------------------------------------------------------

/// The round trip that makes this a substrate feature rather than a script: an
/// `export … to:` writes a `seed/<table>.jsonl`, the EXISTING seed loader births
/// a fresh cell from it, and the message-form `export` of both cells carries the
/// same rows. Compared as PARSED values — byte identity with the interim
/// Python sink was never the goal, format identity is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_to_a_path_writes_a_seed_file_the_loader_reads() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut live = Live::start(fenced(fence.path()), NOTES).await;

    let here = payload(
        &live
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    let reply = live
        .send(json!({"operation": "export", "to": "gen-1", "table": "notes"}))
        .await;
    assert_eq!(code(&reply), None, "{reply}");
    assert_eq!(reply["header"]["rows_affected"], 2);
    let receipt = payload(&reply);
    assert_eq!(receipt["format"], "meclaw-cell-export/1");
    assert_eq!(receipt["tables"], json!(["notes"]));
    assert_eq!(receipt["rows"]["notes"], 2);
    assert_eq!(
        receipt["seed_dir"], "gen-1/seed",
        "the receipt names the path the CALLER named — a host prefix travels \
         further than the fence does"
    );
    live.finish().await;

    // The file, exactly as the loader wants it: the schema object on line 1,
    // one row per line after it, a trailing newline.
    let file = fence.path().join("gen-1/seed/notes.jsonl");
    let text = std::fs::read_to_string(&file).expect("the slot must have written the file");
    assert!(text.ends_with('\n'), "a seed file ends with a newline");
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 3, "one header plus one line per row: {text}");
    let header: Value = meclaw_core::serde_json::from_str(lines[0]).unwrap();
    assert_eq!(header["schema"], here["schema"]);
    let row: Value = meclaw_core::serde_json::from_str(lines[1]).unwrap();
    assert_eq!(row["id"], "a");

    // And the loader reads it — the birth path and the transfer path speak one
    // format, now literally: the same process wrote the file the loader takes.
    let born = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(born.path().join("seed")).unwrap();
    std::fs::copy(&file, born.path().join("seed/notes.jsonl")).unwrap();
    let seeded = seed_and_export(born.path(), "notes");
    assert_eq!(
        seeded["rows"], here["rows"],
        "what left the source is what a cell born from the file holds"
    );
}

/// Birth a cell from `dir/seed/*.jsonl` with the substrate's own loader and
/// read one table back out of it.
fn seed_and_export(dir: &std::path::Path, table: &str) -> Value {
    meclaw_colony::mutation::stage::seed_cell_db_if_present(dir, "store", &Default::default())
        .expect("the existing seed loader must read what the slot wrote");
    let conn = rusqlite::Connection::open(dir.join("cell.db")).unwrap();
    let args = json!({"operation": "export", "table": table, "key": ["id"]});
    match meclaw_colony::db_transfer::dispatch(&conn, args.as_object().unwrap()).unwrap() {
        meclaw_colony::db_transfer::TransferOutcome::Done { payload, .. } => payload,
        other => panic!("export must succeed: {other:?}"),
    }
}

/// Whole, or not at all — and the marker is the last thing written. A reader
/// that watches `export_final.json` never sees a directory that is still
/// filling, and no `.part` file is left standing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_leaves_no_part_file_and_writes_the_marker_last() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut live = Live::start(
        fenced(fence.path()),
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);
         CREATE TABLE tags  (id TEXT PRIMARY KEY, tag TEXT);
         INSERT INTO notes VALUES ('a', 'first');
         INSERT INTO tags  VALUES ('t', 'red');",
    )
    .await;

    // No `table` at all: every content table of this cell, in one directory.
    let reply = live
        .send(json!({"operation": "export", "to": "whole"}))
        .await;
    assert_eq!(code(&reply), None, "{reply}");
    live.finish().await;

    let seed = fence.path().join("whole/seed");
    let mut names: Vec<String> = std::fs::read_dir(&seed)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(
        names.iter().all(|n| !n.ends_with(".part")),
        "a staged write that was renamed leaves nothing behind: {names:?}"
    );
    assert!(names.contains(&"notes.jsonl".to_string()));
    assert!(names.contains(&"tags.jsonl".to_string()));
    assert!(names.contains(&"export_final.json".to_string()));
    assert!(
        names.contains(&"system.jsonl".to_string()),
        "`system` is content and travels (GH #99): {names:?}"
    );

    let marker: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(seed.join("export_final.json")).unwrap(),
    )
    .expect("the marker must be parseable JSON");
    assert_eq!(marker["format"], "meclaw-cell-export/1");
    assert_eq!(marker["cell"], CELL);
    assert!(marker["exported_at"].is_i64(), "{marker}");
    let tables: Vec<&str> = marker["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(tables.contains(&"notes") && tables.contains(&"tags"));
    assert_eq!(marker["rows"]["notes"], 1);
    assert_eq!(marker["rows"]["tags"], 1);
}

// ---------------------------------------------------------------------------
// The read half
// ---------------------------------------------------------------------------

/// A directory written by one cell is read by another, and the second
/// application of the same directory writes nothing: the target wins every
/// collision, and re-applying is idempotent — the same three decisions the
/// message form encodes, reached through a file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_from_a_path_merges_and_the_target_wins() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut source = Live::start(fenced(fence.path()), NOTES).await;
    let reply = source
        .send(json!({"operation": "export", "to": "gen-1"}))
        .await;
    assert_eq!(code(&reply), None, "{reply}");
    let doc = payload(
        &source
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    source.finish().await;

    let mut target = Live::start(
        fenced(fence.path()),
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);
         INSERT INTO notes VALUES ('a', 'the target already decided this', 9);",
    )
    .await;

    let first = target
        .send(json!({"operation": "import", "from": "gen-1", "table": "notes", "key": ["id"]}))
        .await;
    assert_eq!(code(&first), None, "{first}");
    let r1 = payload(&first);
    assert_eq!(r1["rows_written"], 1, "only 'b' is new: {r1}");
    assert_eq!(r1["rows_skipped"], 1, "'a' belongs to the target: {r1}");
    assert_eq!(r1["tables"][0]["table"], "notes");
    assert_eq!(r1["tables"][0]["rows_in_part"], 2);
    assert_eq!(first["header"]["rows_affected"], 1);

    let second = target
        .send(json!({"operation": "import", "from": "gen-1", "table": "notes", "key": ["id"]}))
        .await;
    let r2 = payload(&second);
    assert_eq!(r2["rows_written"], 0, "re-applying writes nothing: {r2}");
    assert_eq!(r2["rows_skipped"], 2);

    let after = payload(
        &target
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    target.finish().await;
    assert_eq!(
        after["rows"][0]["body"], "the target already decided this",
        "the target wins, on the file path too"
    );
    assert_eq!(after["rows"][1], doc["rows"][1], "and 'b' arrived whole");
}

/// A seed file without a header describes nothing. The refusal names the file
/// and writes not one row — the same sentence `apply_seed_jsonl` says on the
/// birth path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_file_without_a_header_is_refused_and_nothing_is_written() {
    let fence = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(fence.path().join("broken/seed")).unwrap();
    std::fs::write(
        fence.path().join("broken/seed/notes.jsonl"),
        "{\"id\":\"z\",\"body\":\"no header above me\",\"n\":1}\n",
    )
    .unwrap();

    let mut live = Live::start(
        fenced(fence.path()),
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);",
    )
    .await;
    let reply = live
        .send(json!({"operation": "import", "from": "broken", "key": ["id"]}))
        .await;
    assert_eq!(code(&reply).as_deref(), Some("transfer_seed_malformed"));
    let text = reply["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("notes.jsonl"),
        "the refusal must name the file: {text}"
    );

    let after = payload(
        &live
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    live.finish().await;
    assert_eq!(after["rows"].as_array().unwrap().len(), 0);
}

/// And a data line that is not an object names the LINE, because that is the
/// part an operator repairs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_file_whose_row_is_not_an_object_names_the_line() {
    let fence = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(fence.path().join("broken/seed")).unwrap();
    std::fs::write(
        fence.path().join("broken/seed/notes.jsonl"),
        "{\"schema\":{\"id\":\"text\",\"body\":\"text\",\"n\":\"int\"}}\n\
         {\"id\":\"a\",\"body\":\"fine\",\"n\":1}\n\
         \"not an object at all\"\n",
    )
    .unwrap();

    let mut live = Live::start(
        fenced(fence.path()),
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);",
    )
    .await;
    let reply = live
        .send(json!({"operation": "import", "from": "broken", "key": ["id"]}))
        .await;
    assert_eq!(code(&reply).as_deref(), Some("transfer_seed_malformed"));
    let text = reply["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("notes.jsonl") && text.contains("line 3"),
        "the refusal must name the file AND the line: {text}"
    );

    let after = payload(
        &live
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    live.finish().await;
    assert_eq!(
        after["rows"].as_array().unwrap().len(),
        0,
        "a part that fails anywhere writes nothing"
    );
}

// ---------------------------------------------------------------------------
// The two boundaries beside the fence
// ---------------------------------------------------------------------------

/// `contract.write_surface: "internal"` bounds the write half of the seam, and
/// a `from:` is a write. Reading the document off a disk does not make it a
/// different operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_import_is_bounded_by_the_write_surface_like_a_message_import() {
    let fence = tempfile::TempDir::new().unwrap();
    let mut source = Live::start(fenced(fence.path()), NOTES).await;
    drop(
        source
            .send(json!({"operation": "export", "to": "gen-1"}))
            .await,
    );
    source.finish().await;

    let bounds = TransferBounds {
        write_surface: meclaw_core::WriteSurface::Internal,
        base_path: Some(Arc::from(fence.path())),
        ..TransferBounds::default()
    };
    let mut live = Live::start(
        bounds,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);",
    )
    .await;
    let refused = live
        .send_from(
            Some(OUTSIDE),
            json!({"operation": "import", "from": "gen-1", "table": "notes", "key": ["id"]}),
        )
        .await;
    assert_eq!(code(&refused).as_deref(), Some("write_denied"));

    let after = payload(
        &live
            .send(json!({"operation": "export", "table": "notes"}))
            .await,
    );
    assert_eq!(
        after["rows"].as_array().unwrap().len(),
        0,
        "a refused import must leave the table untouched"
    );

    // And the export half stays a read: an outside sender may still ask for one.
    let allowed = live
        .send_from(
            Some(OUTSIDE),
            json!({"operation": "export", "to": "read-is-a-read", "table": "notes"}),
        )
        .await;
    assert_eq!(
        code(&allowed),
        None,
        "no write surface has ever bounded a read: {allowed}"
    );
    live.finish().await;
}

/// `transfer_exempt` still strikes FIRST — before the arguments are read, and
/// therefore before a path could be resolved or a directory created. A refusal
/// that behaved differently for a reachable path than for an unreachable one
/// would be an oracle about the filesystem.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exempt_cell_refuses_a_path_before_it_reads_it() {
    let fence = tempfile::TempDir::new().unwrap();
    let bounds = TransferBounds {
        base_path: Some(Arc::from(fence.path())),
        ..TransferBounds::exempt()
    };
    let mut live = Live::start(bounds, NOTES).await;

    let reply = live
        .send(json!({"operation": "export", "to": "gen-1", "table": "notes"}))
        .await;
    assert_eq!(code(&reply).as_deref(), Some("transfer_exempt"));
    let import = live
        .send(json!({"operation": "import", "from": "gen-1"}))
        .await;
    assert_eq!(code(&import).as_deref(), Some("transfer_exempt"));
    live.finish().await;

    assert!(
        !fence.path().join("gen-1").exists(),
        "an exempt cell touches the filesystem not at all"
    );
    assert!(
        std::fs::read_dir(fence.path()).unwrap().next().is_none(),
        "not one directory may appear inside the fence of an exempt cell"
    );
}
