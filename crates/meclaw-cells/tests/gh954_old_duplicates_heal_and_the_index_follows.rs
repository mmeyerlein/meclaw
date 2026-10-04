//! GH #954 (Review I-1) -- the old duplicates heal, and the index follows.
//!
//! WHY this file exists: since GH #954 the keeper's store declares the unique
//! index `sessions_open_round` on (channel, audience_set, closed_at), and the
//! open of a generation is a claim under it. The store builds a declared index
//! at WAKE, `CREATE UNIQUE INDEX IF NOT EXISTS`, and the build fails -- softly,
//! with one log line -- while two rows already collide. A keeper that ran
//! before #954 can hold exactly such rows:
//!
//! * two OPEN generations of one round on one channel (the race of two first
//!   turns, before the claim existed) -- both `closed_at` empty;
//! * two SEALED generations of one round on one channel with the same
//!   `closed_at` (an old seal or sweep pass sealed both with one `now`).
//!
//! As long as one pair stands, the index is never built, the claim is never
//! enforced, and the race #954 closed stays open on exactly the live colonies
//! that had it. Nothing may be deleted to get there (the store is a log).
//!
//! Asked of a running colony with the shipped keeper and a real store whose
//! `cell.db` is seeded, BEFORE the first boot, in the schema a pre-#954 keeper
//! wrote (no `owed_turn`, no index): the store wakes on it and cannot build the
//! index; the night (`in_sweep`, fired by hand) re-stamps the sealed pair, and
//! a turn on the channel of the open pair runs in the newest of them and seals
//! the other; then no two rows collide, every seeded row still stands, and
//! every sealed row was visited (`owed_turn` NOT NULL). Woken again on that
//! `cell.db`, the store builds the index, and two first turns of a new round
//! sent together open ONE generation.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// A failure marker, not a timing: the night and a turn are a handful of
/// python and store round trips.
const DEADLINE: Duration = Duration::from_secs(30);

/// A fixed schedule id, so the test can trigger the night (an instantiation
/// substitution does not run on a tree written straight to disk).
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-00000000c954";
/// Never during a test run: the shipped night must not race the test.
const NEVER: &str = "0 0 0 1 1 *";

/// The channel of the sealed pair and its round.
const OLD: &str = "tg:old";
const ROUND_OLD: &str = r#"["member:alex","agent:scribe"]"#;
/// The one `closed_at` an old pass stamped on both rows of the sealed pair.
const SEALED_AT: &str = "2026-09-01T22:00:00.000000Z";
/// The channel of the open pair and its round.
const DUP: &str = "tg:dup";
const ROUND_DUP: &str = r#"["member:robin","agent:scribe"]"#;
/// A fresh channel and round for the race after the index stands.
const NEW: &str = "tg:new";
const ROUND_NEW: &str = r#"["member:kim","agent:scribe"]"#;

const SESSIONS_DB: &str = "main/session-keeper/sessions/cell.db";

// ═══════════════════════════════════════════════════════════════ the tree

/// The shipped template, copied cell by cell: only `config.json` files travel.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// Everything under `src` -- the colony's tree as a restart finds it.
///
/// GH #991: a SQLite database is copied as one consistent snapshot
/// (`VACUUM INTO`, which reads through the WAL under SQLite's own locks), and
/// its `-wal` / `-shm` / `-journal` sidecars are not copied as files. The
/// connections of the stopped colony close on blocking threads after
/// `shutdown` returns; the last one checkpoints and deletes the WAL. A
/// file-by-file copy met that as `NotFound` (release gate, 4.10.) -- or,
/// worse, took the main file before the checkpoint and lost the WAL frames.
/// Every other file is copied as before, and a missing one stays an error.
fn copy_all(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if from.is_dir() {
            copy_all(&from, &dst.join(&name));
        } else if ["-wal", "-shm", "-journal"]
            .iter()
            .any(|sidecar| name_str.ends_with(sidecar))
        {
            // Part of its database's snapshot below.
        } else if name_str.ends_with(".db") {
            let conn = rusqlite::Connection::open(&from).unwrap();
            conn.busy_timeout(Duration::from_secs(10)).unwrap();
            conn.execute("VACUUM INTO ?1", [dst.join(&name).to_string_lossy()])
                .unwrap();
        } else {
            std::fs::copy(&from, dst.join(&name)).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn code_cell(script: &str, routes: &[&str]) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": routes, "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that exercises the keeper ports.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A harness message becomes an inbound turn on the keeper's port.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"}, "messages": d.get("messages", [])}))
"#;

/// Everything downstream of the stamp: it answers with the session id the
/// turn was stamped with.
const REPORT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "report"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("session_id", ""))}]}))
"#;

/// The close lane downstream: it names the generation and the channel.
const CLOSED: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "closed"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "closed:" + str(ctx.get("session_id", "")) +
                                                   "|" + str(ctx.get("channel", ""))}]}))
"#;

/// The parent around the keeper, as in `session_keeper.rs`: one ingress, the
/// stamped turn out, the close out, and the door the night is fired through.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": ".", "to": "./session-keeper/night",
         "condition": "hop.route == 'fire_night'"},
        {"from": "./probe", "to": "./session-keeper",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"}}},
        {"from": "./session-keeper", "to": "./report",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_context": {"session_id": "hop.session_id"}}},
        {"from": "./report", "to": "/sink"},
        {"from": "./session-keeper", "to": "./closed",
         "condition": "hop.route == 'close'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "channel": "hop.channel"}}},
        {"from": "./closed", "to": "/park"}
    ]}}})
}

fn build_tree(root: &std::path::Path) {
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    let template =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/session-keeper");
    copy_cells(&template, &root.join("main/session-keeper"));
    let night_path = root.join("main/session-keeper/night/config.json");
    let mut night: Value =
        serde_json::from_str(&std::fs::read_to_string(&night_path).unwrap()).unwrap();
    night["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
    night["params"]["schedules"][0]["cron"] = json!(NEVER);
    std::fs::write(&night_path, serde_json::to_string_pretty(&night).unwrap()).unwrap();
    write(root, "main/probe/config.json", &code_cell(PROBE, &["turn"]));
    write(
        root,
        "main/report/config.json",
        &code_cell(REPORT, &["report"]),
    );
    write(
        root,
        "main/closed/config.json",
        &code_cell(CLOSED, &["closed"]),
    );
}

/// The pre-#954 store of the keeper, written before the colony ever woke: the
/// table a 2.x keeper created (every column text but `closed`, no `owed_turn`,
/// no index) and the two colliding pairs. The file is a real cell database
/// (`open_or_create_cell_db`: schema_version 1, the substrate's own tables),
/// so the boot probe resumes it like any other.
fn seed_old_store(root: &std::path::Path) {
    let db = root.join(SESSIONS_DB);
    let conn = meclaw_colony::persist::open_or_create_cell_db(&db).expect("seed cell.db");
    conn.execute_batch(
        "CREATE TABLE sessions (channel TEXT, session_id TEXT, opened_at TEXT, \
         last_seen TEXT, closed INTEGER, closed_at TEXT, audience_set TEXT)",
    )
    .expect("old sessions table");
    // The open pair is still talking (last seen now), so the night's idle
    // cut leaves it to the next turn of its round.
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string();
    let rows = [
        (
            OLD,
            "tg:old-1",
            "2026-09-01T08:00:00.000000Z",
            "2026-09-01T09:00:00.000000Z",
            1,
            SEALED_AT,
            ROUND_OLD,
        ),
        (
            OLD,
            "tg:old-2",
            "2026-09-01T10:00:00.000000Z",
            "2026-09-01T11:00:00.000000Z",
            1,
            SEALED_AT,
            ROUND_OLD,
        ),
        (
            DUP,
            "tg:dup-1",
            "2026-10-01T10:00:00.000000Z",
            now.as_str(),
            0,
            "",
            ROUND_DUP,
        ),
        (
            DUP,
            "tg:dup-2",
            "2026-10-01T11:00:00.000000Z",
            now.as_str(),
            0,
            "",
            ROUND_DUP,
        ),
    ];
    for r in rows {
        conn.execute(
            "INSERT INTO sessions (channel, session_id, opened_at, last_seen, closed, \
             closed_at, audience_set) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![r.0, r.1, r.2, r.3, r.4, r.5, r.6],
        )
        .expect("old row");
    }
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

fn turn_in(channel: &str, text: &str, round: &str) -> Message {
    let mut ctx = serde_json::Map::new();
    ctx.insert("channel".into(), json!(channel));
    ctx.insert("audience_set".into(), json!(round));
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(64)
        .build()
}

fn fire() -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("fire_night"));
    MessageBuilder::new(Path::new("/"))
        .hop(hop)
        .body(Body::Inline(
            json!({"messages": [], "op": "trigger", "schedule_id": SCHEDULE_ID}),
        ))
        .ttl(64)
        .build()
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(DEADLINE, rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what}: nothing arrived within {DEADLINE:?}"))
}

// ═══════════════════════════════════════════════════════════ the store

/// The rows of `sql` over the keeper's `cell.db`, every column as text
/// (NULL as `<null>`). Read-only and never created here (the `gh893`
/// lesson); a query the schema cannot answer (no `owed_turn` yet) is no row.
fn rows(root: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    let db = root.join(SESSIONS_DB);
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
                Ok(rusqlite::types::ValueRef::Integer(n)) => n.to_string(),
                Ok(rusqlite::types::ValueRef::Null) => "<null>".to_string(),
                _ => String::new(),
            })
            .collect::<Vec<String>>())
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Until `ok` holds over the rows of `sql`, or the deadline says what did not
/// happen and what the store held instead.
async fn until(
    root: &std::path::Path,
    sql: &str,
    ok: impl Fn(&[Vec<String>]) -> bool,
    what: &str,
) -> Vec<Vec<String>> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let got = rows(root, sql);
        if ok(&got) {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "{what} within {DEADLINE:?}: `{sql}` -> {got:?}; the store holds {:?}",
            rows(root, EVERYTHING)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

const ALL: &str = "SELECT session_id, channel, COALESCE(audience_set, ''), closed, \
                   COALESCE(closed_at, ''), COALESCE(owed_turn, '<null>') FROM sessions \
                   ORDER BY session_id";

/// Every row as it stands, whatever the schema (a diagnostic).
const EVERYTHING: &str = "SELECT * FROM sessions ORDER BY session_id";

/// Every group of rows that would break the unique index.
const COLLISIONS: &str = "SELECT channel, audience_set, closed_at, COUNT(*) FROM sessions \
                          GROUP BY channel, audience_set, closed_at HAVING COUNT(*) > 1";

fn has_index(root: &std::path::Path) -> bool {
    !rows(
        root,
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'sessions_open_round'",
    )
    .is_empty()
}

fn row_of(all: &[Vec<String>], sid: &str) -> Vec<String> {
    all.iter()
        .find(|r| r[0] == sid)
        .unwrap_or_else(|| panic!("no row {sid}: {all:?}"))
        .clone()
}

// ═══════════════════════════════════════════════════════════ the lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_duplicates_heal_and_the_index_follows() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    build_tree(root);
    seed_old_store(root);
    let seeded: Vec<String> = rows(root, "SELECT session_id FROM sessions ORDER BY session_id")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(seeded.len(), 4, "the old store is seeded: {seeded:?}");
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    // The night: the sealed pair is visited and re-stamped, nothing else.
    h.send(fire()).await;
    until(
        root,
        &format!(
            "SELECT session_id FROM sessions WHERE channel = '{OLD}' AND closed = 1 \
             AND owed_turn IS NOT NULL"
        ),
        |r| r.len() == 2,
        "the night visits both rows of the old sealed pair",
    )
    .await;
    // The store woke on colliding rows: the index could not be built, and it
    // is not built behind the store's back while it runs.
    assert!(
        !has_index(root),
        "the unique index stands over colliding rows -- the seed did not collide"
    );

    // A turn of the open pair's round: it runs in the NEWEST of the two and
    // seals the other, which is handed over under its own round.
    h.send(turn_in(DUP, "the open pair speaks", ROUND_DUP))
        .await;
    let ran_in = text_of(&recv_bounded(&mut sink_rx, "the turn of the open pair").await);
    assert_eq!(
        ran_in,
        "tg:dup-2",
        "the newest open generation of the round runs on: {:?}",
        rows(root, EVERYTHING)
    );
    let closed = recv_bounded(&mut park_rx, "the older open generation is handed over").await;
    assert_eq!(text_of(&closed), format!("closed:tg:dup-1|{DUP}"));

    until(
        root,
        COLLISIONS,
        |r| r.is_empty(),
        "no two rows collide on (channel, audience_set, closed_at)",
    )
    .await;
    let healed = rows(root, ALL);
    // Nothing deleted: every seeded generation still stands.
    for sid in &seeded {
        row_of(&healed, sid);
    }
    assert!(healed.len() >= seeded.len(), "{healed:?}");
    // Every sealed row was visited -- `owed_turn` NULL is the mark of a row
    // the heal pass has not seen yet.
    for r in healed.iter().filter(|r| r[3] == "1") {
        assert_ne!(r[5], "<null>", "a sealed row was never visited: {healed:?}");
    }
    // The sealed pair: the smaller session id keeps its stamp, the other one
    // becomes `<stamp>~<session_id>` -- unique by the session id, and it
    // still sorts right after the old instant.
    let (old1, old2) = (row_of(&healed, "tg:old-1"), row_of(&healed, "tg:old-2"));
    assert_eq!(
        old1[4], SEALED_AT,
        "the first of the pair keeps its stamp: {healed:?}"
    );
    assert!(
        old2[4] == format!("{SEALED_AT}~tg:old-2") && old2[4] > old1[4],
        "the second of the pair is re-stamped right after the first: {healed:?}"
    );
    let (dup1, dup2) = (row_of(&healed, "tg:dup-1"), row_of(&healed, "tg:dup-2"));
    assert_eq!(
        (dup1[2].as_str(), dup1[3].as_str()),
        (ROUND_DUP, "1"),
        "the older open generation is sealed under its own round: {healed:?}"
    );
    assert_eq!(dup2[3], "0", "the newest runs on: {healed:?}");
    h.shutdown().await;

    // The restart: the same tree and the same `cell.db`, woken by a new
    // colony. The store resumes the file and builds the index now.
    let td2 = tempfile::TempDir::new().unwrap();
    let root2 = td2.path();
    std::fs::write(root2.join(".env"), "").unwrap();
    copy_all(&root.join("main"), &root2.join("main"));
    let (h, mut sink_rx, _park_rx) = boot(&td2).await;
    // Two first turns of a new round, together: the claim under the index
    // lets one of them open the generation and the other join it.
    h.send(turn_in(NEW, "the new round, once", ROUND_NEW)).await;
    h.send(turn_in(NEW, "the new round, twice", ROUND_NEW))
        .await;
    let one = text_of(&recv_bounded(&mut sink_rx, "the first racing turn").await);
    let two = text_of(&recv_bounded(&mut sink_rx, "the second racing turn").await);
    assert!(
        has_index(root2),
        "the woken store did not build `sessions_open_round` over the healed rows: {:?}",
        rows(root2, EVERYTHING)
    );
    let opened = rows(
        root2,
        &format!("SELECT session_id FROM sessions WHERE channel = '{NEW}' AND closed = 0"),
    );
    assert_eq!(
        (opened.len(), one.as_str()),
        (1, two.as_str()),
        "two first turns of a new round open ONE generation: {opened:?}"
    );
    assert_eq!(opened[0][0], one);
    h.shutdown().await;
}
