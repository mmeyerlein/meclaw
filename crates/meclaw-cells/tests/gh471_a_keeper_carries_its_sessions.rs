//! GH #471 — a session ledger crosses from one keeper to another, and the
//! receiving keeper CONTINUES the conversation instead of starting it again.
//!
//! `sessions` decides whether a turn belongs to the generation that is already
//! open or opens a new one. A keeper reborn empty greets somebody it has been
//! talking to for a year as a stranger, and nothing anywhere says so — the new
//! generation is a perfectly ordinary event. That is why the row has to travel.
//!
//! Two keepers stand in one colony here, and since GH #555 the transfer between
//! them goes through a DIRECTORY rather than through an edge: the sending
//! keeper's own store writes `<fence>/session-keeper/seed/sessions.jsonl`
//! itself, and the document is carried into the receiving keeper as an
//! `in_import` part built out of that file — the same document read the other
//! way round, and exactly what `examples/memory-import/build_import.py`
//! (`--after-boot`) writes for a keeper that cannot be seeded at birth. Nothing
//! is mocked; both hives are the shipped `templates/session-keeper` tree.
//!
//! Three properties, and the third is the one a row count cannot reach:
//!
//! 1. **The walk leaves, as a file.** One table, `sessions`, written with the
//!    schema the store declares on line 1 and one row per line after it, plus
//!    the completeness marker beside it — and the keeper says `export_done`
//!    itself, naming the directory relative to its own fence.
//! 2. **The row arrives, once.** The same document applied twice leaves the
//!    same state: that is what the probe before the insert buys, and applying
//!    it twice is how a partial transfer is repaired.
//! 3. **The imported row is LIVE.** A turn on the transferred channel is
//!    stamped with the session the source keeper had open, not with a fresh
//!    one. A ledger that arrives and is not read is a table, not a session.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;

const RECV_TIMEOUT: Duration = Duration::from_secs(30);
const CHANNEL: &str = "tg:471";
const SESSION: &str = "tg:471-2026-08-28T09:00:00.000000Z";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/session-keeper/config.json",
        "templates/session-keeper/porter/config.json",
        "templates/session-keeper/sessions/config.json",
        "templates/session-keeper/stamp/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else if from.extension().and_then(|e| e.to_str()) != Some("md") {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

fn shipped_config(rel: &str) -> Value {
    from_str(&std::fs::read_to_string(repo(rel)).expect(rel)).expect("shipped config is json")
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = match conn.prepare(sql) {
        Ok(st) => st,
        // The store creates its tables when it first wakes; a keeper nobody has
        // spoken to yet has a file and no schema, and that is an empty ledger
        // rather than a defect (GH #565).
        Err(_) => return Vec::new(),
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// One shipped keeper at `main/<name>`, with the night timer left out: this
/// file is about the transfer, and a cron that fires mid-test would close the
/// very generation the third property reads.
fn keeper(root: &std::path::Path, name: &str, seeded: bool, fence: &std::path::Path) {
    let dst = root.join("main").join(name);
    copy_tree(&repo("templates/session-keeper"), &dst);
    std::fs::remove_file(dst.join("template.json")).unwrap();
    std::fs::remove_dir_all(dst.join("night")).unwrap();
    let mut hive = shipped_config("templates/session-keeper/config.json");
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["from"] != "./night")
        .cloned()
        .collect::<Vec<_>>();
    hive["params"]["graph"]["edges"] = json!(edges);
    write_json(&dst.join("config.json"), &hive);

    // The one thing an instance says about files (GH #555): the absolute
    // directory this keeper's own store writes and reads inside.
    let mut store_cfg = shipped_config("templates/session-keeper/sessions/config.json");
    store_cfg["params"]["transfer"]["base_path"] = json!(fence.to_str().unwrap());
    write_json(&dst.join("sessions/config.json"), &store_cfg);

    if seeded {
        let store = shipped_config("templates/session-keeper/sessions/config.json");
        let header = json!({"schema": store["params"]["schema"]["sessions"]});
        let row = json!({"channel": CHANNEL, "session_id": SESSION,
                         "opened_at": "2026-08-28T09:00:00.000000Z",
                         "last_seen": "2026-08-28T09:30:00.000000Z",
                         "closed": 0, "closed_at": "",
                         "audience_set": "member:alex,agent:aiden"});
        std::fs::create_dir_all(dst.join("sessions/seed")).unwrap();
        std::fs::write(
            dst.join("sessions/seed/sessions.jsonl"),
            format!("{header}\n{row}\n"),
        )
        .unwrap();
    }
}

/// A code cell that writes one file per lane it is handed, so a wait can be a
/// wait for something that HAD to arrive rather than for the absence of a dead
/// letter.
///
/// **One line per message, appended — never a rewritten document.** A `code`
/// cell is a stateless dispatcher whose default `max_concurrency` is 4
/// (`CodeParams::effective_max_concurrency`), so two arrivals close enough
/// together run as two overlapping subprocesses. The earlier body read the
/// whole lane file, appended to what it had read and wrote it back — the last
/// writer then erased every receipt that had arrived while it was working, and
/// a wait on that lane spent its whole window on a lane that had in fact
/// delivered (GH #587, measured there; GH #588, the same form here).
///
/// A single `O_APPEND` write places each line whole and at the end instead, so
/// no execution can observe — let alone overwrite — another one's, and the
/// return value is checked so a short write falls loudly rather than leaving
/// half a line lying there. A reader that meets a line mid-flight fails to
/// parse it and retries, which is why an arrival can never be miscounted.
fn flag_cell(dir: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "flag_dir": dir, "sandbox": {"trust": "trusted"},
                   "script_inline": r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
name = str(hop.get("dump_kind") or hop.get("route") or "unknown")
path = os.path.join(doc["params"]["flag_dir"], name + ".jsonl")
entry = {"hop": hop, "text": (doc["body"].get("messages") or [{}])[0].get("text", "")}
blob = (json.dumps(entry) + "\n").encode("utf-8")
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
try:
    written = os.write(fd, blob)
finally:
    os.close(fd)
if written != len(blob):
    sys.exit("short write: %d of %d bytes" % (written, len(blob)))
sys.stdout.write(json.dumps([]))
"#},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {}}
    })
}

/// One `in_import` part, addressed at the receiving keeper's own path.
async fn import(h: &ColonyHandle, part: &Value) {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_import"));
    h.send(
        MessageBuilder::new(Path::new("/new"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "text",
                "text": to_string_pretty(part).unwrap()}]})))
            .build(),
    )
    .await;
}

async fn nudge(h: &ColonyHandle, target: &str, route: &str, hop_extra: &[(&str, Value)]) {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!(route));
    for (k, v) in hop_extra {
        hop.insert((*k).to_string(), v.clone());
    }
    h.send(
        MessageBuilder::new(Path::new(target))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;
}

/// The recorder's append log for one lane.
fn lane_file(flags: &std::path::Path, lane: &str) -> std::path::PathBuf {
    flags.join(format!("{lane}.jsonl"))
}

/// Every message the recorder has finished placing on that lane. A line that is
/// still being placed does not parse and is simply not there yet.
fn lane_entries(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| from_str::<Value>(l).ok())
        .collect()
}

/// Poll one lane's append log until `want` messages have arrived on it.
///
/// What ends the wait is the count of messages that reached the lane, and
/// nothing that arrived can go missing again — so reaching the marker means the
/// lane did not deliver, never that the record of a delivery was overwritten.
async fn wait_lane(
    flags: &std::path::Path,
    lane: &str,
    want: usize,
    what: &str,
    h: &ColonyHandle,
) -> Vec<Value> {
    let p = lane_file(flags, lane);
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let seen = lane_entries(&p);
        if seen.len() >= want {
            return seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: only {} of {want} reached `{lane}` -- dead letters: {:?}",
            seen.len(),
            h.drain_dead_letters()
                .await
                .iter()
                .map(|d| (
                    d.sender_path.as_str().to_string(),
                    d.resolved_target.as_str().to_string(),
                    d.reason.as_code()
                ))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_ledger_crosses_and_the_new_keeper_continues_the_conversation() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let flag_dir = root.join("flags");
    std::fs::create_dir_all(&flag_dir).unwrap();

    let fence = root.join("exports");
    std::fs::create_dir_all(&fence).unwrap();
    keeper(root, "old", true, &fence);
    keeper(root, "new", false, &fence);
    write_json(
        &root.join("main/flag/config.json"),
        &flag_cell(flag_dir.to_str().unwrap()),
    );
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                // No edge carries the document any more: the sending store
                // writes it, and whoever moves the directory carries it in.
                // What each side still owes is a subscription to what it says
                // about the transfer, because `required_drains` is not a
                // suggestion.
                {"from": "./old", "to": "./flag",
                 "condition": "has(hop.route) && (hop.route == 'export_done' || hop.route == 'dump' || hop.route == 'reject')"},
                {"from": "./new", "to": "./flag",
                 "condition": "has(hop.route) && (hop.route == 'export_done' || hop.route == 'dump' || hop.route == 'reject' || hop.route == 'turn')"}
            ]}}
        }),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // ── 1. the walk leaves, as a file ───────────────────────────────────────
    nudge(&h, "/old", "in_export", &[]).await;
    let done = wait_lane(
        &flag_dir,
        "export_done",
        1,
        "the keeper's own completion word",
        &h,
    )
    .await;
    assert_eq!(done[0]["hop"]["export_hive"], "session-keeper");
    assert_eq!(
        done[0]["hop"]["seed_dir"], "session-keeper/seed",
        "the completion word names the directory RELATIVE to the fence the store \
         declares -- a receipt travels further than the fence does: {done:?}"
    );
    assert_eq!(done[0]["hop"]["rows_written"], 1);
    let seed = fence.join("session-keeper/seed");
    let text = std::fs::read_to_string(seed.join("sessions.jsonl"))
        .expect("the store must have written its own seed file");
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 2, "one schema line plus one row: {text}");
    let header: Value = from_str(lines[0]).unwrap();
    assert_eq!(
        header["schema"],
        shipped_config("templates/session-keeper/sessions/config.json")["params"]["schema"]["sessions"],
        "line 1 of a seed file is the store's own declaration, verbatim -- that \
         is what makes the file a birth format rather than a row dump"
    );
    let row: Value = from_str(lines[1]).unwrap();
    assert_eq!(row["session_id"], SESSION);
    assert!(
        seed.join("export_final.json").is_file(),
        "the completeness marker is what tells a whole document from a prefix"
    );
    assert!(
        !lane_file(&flag_dir, "reject").exists(),
        "the walk refused something: {:?}",
        lane_entries(&lane_file(&flag_dir, "reject"))
    );

    // ── 2. the row arrives, and a second application changes nothing ────────
    // The document, read the other way round: the header line is the part's
    // schema and the rest are its rows. `examples/memory-import/build_import.py`
    // builds exactly this out of exactly these bytes.
    let part = json!({
        "format": "meclaw-session-export/1", "hive_template": "session-keeper",
        "export_id": "gh471", "exported_at": "2026-09-04T00:00:00Z",
        "table": "sessions", "part": 1, "of": 1, "final": true, "absent": false,
        "schema": header["schema"], "rows": [row],
    });
    import(&h, &part).await;

    let new_db = root.join("main/new/sessions/cell.db");
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline
        && (!new_db.is_file() || rows(&new_db, "SELECT session_id FROM sessions").is_empty())
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        rows(
            &new_db,
            "SELECT channel, session_id, CAST(closed AS TEXT) FROM sessions"
        ),
        vec![vec![
            CHANNEL.to_string(),
            SESSION.to_string(),
            "0".to_string()
        ]],
        "the transferred generation is not in the receiving keeper"
    );
    let receipts = wait_lane(&flag_dir, "dump", 1, "the import receipt", &h).await;
    assert_eq!(receipts[0]["hop"]["rows_written"], 1);

    // the same document again
    import(&h, &part).await;
    let receipts = wait_lane(&flag_dir, "dump", 2, "the second import receipt", &h).await;
    assert_eq!(
        receipts[1]["hop"]["rows_written"], 0,
        "applying the same document twice wrote a second row -- the probe \
         before the insert is what makes a repeated transfer a repair rather \
         than a duplication"
    );
    assert_eq!(
        rows(&new_db, "SELECT session_id FROM sessions").len(),
        1,
        "two rows for one generation is a conversation that forked"
    );

    // ── 3. the imported row is LIVE ─────────────────────────────────────────
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_turn"));
    let mut ctx = Map::new();
    ctx.insert("channel".to_string(), json!(CHANNEL));
    ctx.insert("audience_set".to_string(), json!("member:alex,agent:aiden"));
    h.send(
        MessageBuilder::new(Path::new("/new"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "still there?"}]})))
            .build(),
    )
    .await;
    let turns = wait_lane(&flag_dir, "turn", 1, "the stamped turn", &h).await;
    assert_eq!(
        turns[0]["hop"]["session_id"], SESSION,
        "the turn opened a NEW generation. The ledger arrived and was not read: \
         a keeper that starts every imported conversation at zero is exactly \
         the state GH #471 measured, one table further in"
    );
    assert_eq!(
        rows(&new_db, "SELECT session_id FROM sessions").len(),
        1,
        "a turn on a transferred channel minted a second generation beside the \
         one it should have continued"
    );

    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "a transfer that dead-letters is the state this lane was built to end; \
         got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    h.shutdown().await;
}
