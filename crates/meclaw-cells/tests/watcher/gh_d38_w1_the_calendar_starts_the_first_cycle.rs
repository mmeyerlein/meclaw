//! GH #1444 -- a freshly born argus ran no cycle, ever: its clock carries no
//! schedule (R-AG-1, GH #1229), the meter arms the next cycle only from inside
//! a cycle, and nothing in a grown colony sends `in_cycle`. The watcher sat in
//! its resting state for as long as the colony lived (measured on a production
//! colony: zero cycles in sixteen hours, the charter with five goals enabled).
//!
//! The way in needs no new param and no code in the timer: `params.schedules`
//! of `./clock` is a birth seed, so the parent that grows the hive can hand it
//! one calendar line under `override_params` -- `schedule_name` `argus-cycle`,
//! the name the hive's edge to `./meter` recognises. Three claims:
//!
//! 1. THE DEFAULT IS UNCHANGED. The shipped clock still carries `schedules: []`:
//!    an argus grown without the line rests until `in_cycle`, as before.
//! 2. THE LINE STARTS A CYCLE. An argus grown with the line and an inert
//!    charter writes its `idle` receipt without anybody calling `in_cycle`.
//! 3. THE LINE HANDS OVER. The documented line uses the schedule id the meter's
//!    own watchdog uses, so the first cycle that finds an enabled goal REPLACES
//!    the calendar line with its one-shot (`rearm`) instead of standing beside
//!    it: one order in the clock, never a calendar and a chain ticking twice.
//!
//! Real time, multi-thread: the cycle runs through `code` cells (child
//! processes), which a paused clock cannot carry (development-rules § 11).
//! The line strikes every second here so the test measures the wiring, not
//! the wall clock; `HANG` is a failure marker, never a verdict.

use meclaw_cells::TimerCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use tokio::sync::oneshot;

/// The schedule id of the meter's cycle watchdog: `uuid5` of the name
/// `argus-cycle` in the watchdog namespace of `argus/meter` (`watchdog_id`).
/// The documented calendar line carries exactly this id.
const CYCLE_ID: &str = "b758d96e-fb9b-5dff-b93a-837c8ce33b99";

/// The role goal the hand-over scenario switches on.
const GOAL: &str = "goal:talky-gap_rate";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn json_at(rel: &str) -> Value {
    let raw = std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn skip() -> bool {
    if !repo("templates/argus/config.json").is_file() {
        eprintln!("argus did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

/// The calendar line in the form the template README documents, with the
/// cadence turned down to one second.
fn calendar_line(cron: &str) -> Value {
    json!({"schedule_id": CYCLE_ID, "schedule_name": "argus-cycle", "cron": cron,
           "emit_to": "../meter",
           "emit_body": {"messages": [{"origin": "user", "type": "text",
                                       "text": "argus-cycle"}]},
           "emit_headers": {}})
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the source is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// A colony with an empty root hive and a library that holds argus; with
/// `enable`, the library's charter ships the one role goal switched on.
fn tree(enable: bool) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main")).expect("root hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");
    let lib = root.join("templates/argus");
    copy_tree(&repo("templates/argus"), &lib);
    if enable {
        let goals = lib.join("charter/seed/goals.jsonl");
        let raw = std::fs::read_to_string(&goals).expect("the shipped goals");
        let mut found = false;
        let lines: Vec<String> = raw
            .lines()
            .map(|l| {
                let mut v: Value = meclaw_core::serde_json::from_str(l).expect("a seed row");
                if v.get("id").and_then(Value::as_str) == Some(GOAL) {
                    v["enabled"] = json!(1);
                    found = true;
                }
                v.to_string()
            })
            .collect();
        assert!(found, "the charter ships the role goal {GOAL}");
        std::fs::write(&goals, lines.join("\n") + "\n").expect("write the goals seed");
    }
    td
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
    ]
}

/// Boots the empty colony, registers the library and grows one argus with
/// the calendar line in `override_params`, the way a set grows it at birth.
async fn grow(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("an empty colony boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the library must register");
    let version = json_at("templates/argus/template.json")["version"]
        .as_str()
        .expect("a version")
        .to_string();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {
                "add_nodes": [{"name": "argus", "template": format!("argus@{version}"),
                    "override_params": {"clock": {"schedules": [
                        calendar_line("* * * * * *")]}}}]
            }}),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "an argus with a calendar line must be growable by an ordinary declaration: \
         {outcome:?}"
    );
    h
}

fn read_only(db: std::path::PathBuf) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// The outcomes of the receipts written so far.
fn outcomes(td: &tempfile::TempDir) -> Vec<String> {
    let Some(conn) = read_only(td.path().join("main/argus/receipts/cell.db")) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT outcome FROM cycles ORDER BY at ASC") else {
        return Vec::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The clock's orders: `(schedule_id, kind, status)` of every row.
fn orders(td: &tempfile::TempDir) -> Vec<(String, String, String)> {
    let Some(conn) = read_only(td.path().join("main/argus/clock/cell.db")) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT schedule_id, kind, status FROM schedules") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Waits for `pred` over a reading of the hive's own stores, through the
/// shared waiter (`meclaw_testing::wait::until_with`, the `HANG` mark is a
/// hang, never a verdict on the time it took; the last reading rides the panic).
async fn until<T: std::fmt::Debug>(what: &str, read: impl Fn() -> T, pred: impl Fn(&T) -> bool) {
    meclaw_testing::wait::until_with(
        what,
        || {
            let ok = pred(&read());
            async move { ok }
        },
        &read,
    )
    .await
}

#[test]
fn gh_d38_w1_the_default_clock_still_carries_no_schedule() {
    let clock = json_at("templates/argus/clock/config.json");
    assert_eq!(
        clock["params"]["schedules"],
        json!([]),
        "the shipped clock stays without a schedule (R-AG-1): the calendar line is \
         the parent's birth seed, not a default"
    );
    // The documented line is the one this file grows, and its id is the
    // meter's own watchdog id, read out of the shipped script's namespace.
    let readme = std::fs::read_to_string(repo("templates/argus/README.md")).expect("README");
    assert!(
        readme.contains(CYCLE_ID) && readme.contains("\"schedule_name\": \"argus-cycle\""),
        "templates/argus/README.md must document the calendar line with the \
         watchdog id {CYCLE_ID}"
    );
    let meter = json_at("templates/argus/meter/config.json");
    let script = meter["params"]["script_inline"].as_str().expect("a script");
    assert!(
        script.contains("WATCHDOG_NS = _wd_uuid.UUID(\"6f1c2a8e-3b7d-5e90-9a41-0c2d7e5b8f13\")")
            && script.contains("CYCLE_WATCHDOG = \"argus-cycle\""),
        "the meter's watchdog namespace or name moved: recompute CYCLE_ID \
         (uuid5 of `argus-cycle` in that namespace)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d38_w1_the_calendar_starts_the_first_cycle() {
    if skip() {
        return;
    }
    let td = tree(false);
    let h = grow(&td).await;
    // Nobody calls `in_cycle`: the calendar line is the only way in.
    until(
        "an idle receipt off the calendar line",
        || outcomes(&td),
        |o| o.iter().any(|s| s == "idle"),
    )
    .await;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d38_w1_the_first_cycle_replaces_the_calendar_line() {
    if skip() {
        return;
    }
    let td = tree(true);
    let h = grow(&td).await;
    // A cycle that found the enabled goal arms its one-shot under the same id:
    // the calendar row becomes that one-shot, and it is the only order.
    until(
        "the calendar line replaced by the meter's one-shot",
        || orders(&td),
        |o| o.len() == 1 && o[0].0 == CYCLE_ID && o[0].1 == "at" && o[0].2 == "active",
    )
    .await;
    h.shutdown().await;
}
