//! GH #1225 (K-4, A-K4) -- fifty slot pushes at once leave no duplicate.
//!
//! WHY this file exists: on soul-col01 every curator ledger held copies of its
//! slot paths (`tools.*` up to 56 rows of one path) -- the collector and the
//! identity pusher write the same paths on every assembly, and two messages in
//! flight both read "missing" and both inserted. The fix is a write that is
//! one step (`insert` + `replace` under the unique index `slots_path`); this
//! lock asks it of a booted colony with the shipped curator, its real store and
//! its resident intake: fifty `in_slots`/`in_pack` messages on five paths sent
//! together, then a marker push behind them -- the intake and the ledger are
//! each one mailbox, so the marker's row landing means every write before it
//! has landed. Not one path may stand twice, and every path ends with the
//! collector's text where the collector wrote it.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::{ColonyHandle, NEVER_CRON};
use std::sync::Arc;

const PATHS: [&str; 5] = [
    "tools.search",
    "tools.calendar",
    "instructions.sidecar",
    "identity",
    "persona",
];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).expect("write");
}

/// The shipped curator, cell by cell: `config.json` files and seeds travel;
/// every `llm` at an address nobody answers (no call is made), every cron
/// out of the run's way, every `${VAR}` bound to a dummy.
fn copy_quiet(src: &std::path::Path, dst: &std::path::Path, vars: &mut Vec<String>) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_quiet(&from, &dst.join(&name), vars);
        } else if src.file_name().is_some_and(|d| d == "seed") {
            std::fs::copy(&from, dst.join(&name)).expect("copy a seed");
        } else if name == "config.json" {
            let raw = std::fs::read_to_string(&from).expect("read");
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let var = &rest[..end];
                if !var.is_empty()
                    && var
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    vars.push(var.to_string());
                }
                rest = &rest[end + 1..];
            }
            let mut cfg = read_json(&from);
            if cfg["cell"]["type"] == "llm" {
                cfg["params"]["base_url"] = json!("http://127.0.0.1:9/v1");
                cfg["params"]["model"] = json!("stub");
                cfg["params"]["api_key"] = json!("sk-test");
            } else if cfg["cell"]["type"] == "timer" {
                // The curator's clock ships without schedules (its parent
                // names them, a talky none): an empty array, as the talky's.
                if cfg["params"]["schedules"].is_null() {
                    cfg["params"]["schedules"] = json!([]);
                }
                for s in cfg["params"]["schedules"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                {
                    if s.get("cron").is_some() {
                        s["cron"] = json!(NEVER_CRON);
                    }
                }
            }
            write_json(&dst.join("config.json"), &cfg);
        }
    }
}

fn build(root: &std::path::Path) {
    let main = root.join("main");
    let mut vars = Vec::new();
    copy_quiet(&repo("templates/curator"), &main.join("c"), &mut vars);
    write_json(
        &main.join("config.json"),
        // The curator's rim is sealed (`ports: []`): a push enters it by an
        // edge of its parent, as the talky's collector does.
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./c", "condition": "has(hop.route)"}
        ]}}}),
    );
    vars.sort();
    vars.dedup();
    let body: String = vars.iter().map(|k| format!("{k}=dummy-{k}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

fn as_map(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// One slot push at the curator: `lane` `in_slots` (the collector) or
/// `in_pack` (the identity pusher), one leaf on `path`.
fn push(lane: &str, path: &str, text: &str) -> Message {
    let mut tree = json!({});
    let mut node = &mut tree;
    for seg in path.split('.') {
        node = &mut node[seg];
    }
    *node = json!({"text": text});
    MessageBuilder::new(Path::new("/"))
        .hop(as_map(json!({"route": lane})))
        .context(as_map(json!({"audience_set": "[\"member:e\"]"})))
        .body(Body::Inline(json!({"messages": [], "system": tree})))
        .build()
}

fn rows(db: &rusqlite::Connection, sql: &str) -> Vec<(String, i64)> {
    let mut st = db.prepare(sql).expect(sql);
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .expect(sql)
        .map(Result::unwrap)
        .collect()
}

/// Whether the marker push has its row in the ledger. The file stands before
/// the store has laid out its tables: a read that fails is "not yet".
fn marker_landed(db_path: &std::path::Path) -> bool {
    if !db_path.is_file() {
        return false;
    }
    let db = rusqlite::Connection::open(db_path).expect("ledger");
    db.busy_timeout(std::time::Duration::from_secs(5))
        .expect("busy timeout");
    db.query_row(
        "SELECT count(*) FROM slots WHERE path = 'marker'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_concurrent_slot_pushes_leave_no_duplicate() {
    if !repo("templates/curator/config.json").is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(td.path());
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the curator must boot");

    // Fifty pushes at once: the collector on every path, the pusher on its
    // own families and on the collector's.
    let mut sends = Vec::new();
    for i in 0..50 {
        let path = PATHS[i % PATHS.len()];
        let lane = if i % 2 == 0 { "in_slots" } else { "in_pack" };
        let who = if lane == "in_slots" {
            "collector"
        } else {
            "pack"
        };
        sends.push(push(lane, path, &format!("{who} text {i}")));
    }
    for m in sends {
        h.send(m).await;
    }
    // The marker, behind all of them in both mailboxes.
    h.send(push("in_slots", "marker", "the last push")).await;

    let db_path = td.path().join("main/c/ledger/cell.db");
    // An event, not a guess: the marker's row is the last write of both
    // mailboxes, so its landing means every push before it has landed (the
    // hang mark of `wait::until` is no verdict).
    meclaw_testing::wait::until("the marker push reaches the ledger", || {
        let db_path = db_path.clone();
        async move { marker_landed(&db_path) }
    })
    .await;
    let db = rusqlite::Connection::open(&db_path).expect("ledger");
    db.busy_timeout(std::time::Duration::from_secs(5))
        .expect("busy timeout");
    assert_eq!(
        rows(
            &db,
            "SELECT path, count(*) FROM slots GROUP BY path HAVING count(*) > 1"
        ),
        Vec::<(String, i64)>::new(),
        "no path stands twice"
    );
    let held = rows(
        &db,
        "SELECT path, count(*) FROM slots GROUP BY path ORDER BY path",
    );
    assert_eq!(held.len(), PATHS.len() + 1, "{held:?}");
    let owners = rows(
        &db,
        "SELECT owner, count(*) FROM slots WHERE path <> 'marker' GROUP BY owner",
    );
    assert_eq!(
        owners,
        [("collector".to_string(), PATHS.len() as i64)],
        "the collector wrote every path, so it holds every path"
    );
    let index: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'slots_path' \
             AND sql LIKE 'CREATE UNIQUE INDEX%'",
            [],
            |r| r.get(0),
        )
        .expect("sqlite_master");
    assert_eq!(index, 1, "the ledger was born with its unique index");
    h.shutdown().await;
}
