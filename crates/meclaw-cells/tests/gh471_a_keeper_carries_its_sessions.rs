//! GH #471 — a session ledger crosses from one keeper to another, and the
//! receiving keeper CONTINUES the conversation instead of starting it again.
//!
//! `sessions` decides whether a turn belongs to the generation that is already
//! open or opens a new one. A keeper reborn empty greets somebody it has been
//! talking to for a year as a stranger, and nothing anywhere says so — the new
//! generation is a perfectly ordinary event. That is why the row has to travel.
//!
//! Two keepers stand in one colony here, wired the way `memory-hive`'s README
//! prescribes for a hive-to-hive transfer: ONE edge, `old -> new` on
//! `hop.route == 'dump'`, renaming the lane to `in_import` on the way. Nothing
//! is mocked; both hives are the shipped `templates/session-keeper` tree.
//!
//! Three properties, and the third is the one a row count cannot reach:
//!
//! 1. **The walk leaves.** One part, `sessions`, carrying the schema the store
//!    declares and the rows it holds — and it is the FINAL part, so the receipt
//!    the sink of a real member waits for can exist at all.
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
    assert!(db.is_file(), "no cell.db at {}", db.display());
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = conn.prepare(sql).expect("prepare");
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
fn keeper(root: &std::path::Path, name: &str, seeded: bool) {
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
fn flag_cell(dir: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "flag_dir": dir, "sandbox": {"trust": "trusted"},
                   "script_inline": r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
name = str(hop.get("dump_kind") or hop.get("route") or "unknown")
path = os.path.join(doc["params"]["flag_dir"], name + ".json")
seen = []
if os.path.exists(path):
    with open(path) as fh:
        seen = json.load(fh)
seen.append({"hop": hop, "text": (doc["body"].get("messages") or [{}])[0].get("text", "")})
with open(path, "w") as fh:
    fh.write(json.dumps(seen))
sys.stdout.write(json.dumps([]))
"#},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {}}
    })
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

async fn wait_for(
    p: &std::path::Path,
    what: &str,
    h: &ColonyHandle,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(p)
            && let Ok(v) = from_str::<Value>(&raw)
            && predicate(&v)
        {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "{what} never arrived at {} -- dead letters: {:?}",
        p.display(),
        h.drain_dead_letters()
            .await
            .iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    )
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

    keeper(root, "old", true);
    keeper(root, "new", false);
    write_json(
        &root.join("main/flag/config.json"),
        &flag_cell(flag_dir.to_str().unwrap()),
    );
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                // The migration recipe, verbatim: one edge, renaming the lane.
                {"from": "./old", "to": "./new",
                 "condition": "has(hop.route) && hop.route == 'dump' && has(hop.dump_kind) && hop.dump_kind == 'export_part'",
                 "modifier": {"set_hop": {"route": "'in_import'"}}},
                // ... and the two lanes a caller has to subscribe to, on both
                // sides, because `required_drains` is not a suggestion.
                {"from": "./old", "to": "./flag",
                 "condition": "has(hop.route) && (hop.route == 'dump' || hop.route == 'reject')"},
                {"from": "./new", "to": "./flag",
                 "condition": "has(hop.route) && (hop.route == 'dump' || hop.route == 'reject' || hop.route == 'turn')"}
            ]}}
        }),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // ── 1. the walk leaves ──────────────────────────────────────────────────
    nudge(&h, "/old", "in_export", &[]).await;
    let parts = wait_for(
        &flag_dir.join("export_part.json"),
        "the export part",
        &h,
        |v| !v.as_array().unwrap_or(&vec![]).is_empty(),
    )
    .await;
    let parts = parts.as_array().unwrap().clone();
    assert_eq!(
        parts.len(),
        1,
        "the keeper holds ONE content table, so its walk is one part: {parts:?}"
    );
    assert_eq!(parts[0]["hop"]["export_final"], "1");
    assert_eq!(parts[0]["hop"]["port_hive"], "session-keeper");
    let part: Value = from_str(parts[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(part["format"], "meclaw-session-export/1");
    assert_eq!(part["table"], "sessions");
    assert_eq!(part["rows"][0]["session_id"], SESSION);
    assert_eq!(
        part["schema"],
        shipped_config("templates/session-keeper/sessions/config.json")["params"]["schema"]["sessions"],
        "line 1 of a seed file is the store's own declaration, verbatim -- that \
         is what makes the part a birth format rather than a row dump"
    );
    assert!(
        !flag_dir.join("reject.json").exists(),
        "the walk refused something: {:?}",
        std::fs::read_to_string(flag_dir.join("reject.json"))
    );

    // ── 2. the row arrives, and a second application changes nothing ────────
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
    let receipts = wait_for(
        &flag_dir.join("import_receipt.json"),
        "the import receipt",
        &h,
        |v| !v.as_array().unwrap_or(&vec![]).is_empty(),
    )
    .await;
    assert_eq!(receipts[0]["hop"]["rows_written"], 1);

    // the same document again
    nudge(&h, "/old", "in_export", &[]).await;
    let receipts = wait_for(
        &flag_dir.join("import_receipt.json"),
        "the second import receipt",
        &h,
        |v| v.as_array().map(Vec::len).unwrap_or(0) >= 2,
    )
    .await;
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
    let turns = wait_for(&flag_dir.join("turn.json"), "the stamped turn", &h, |v| {
        !v.as_array().unwrap_or(&vec![]).is_empty()
    })
    .await;
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
