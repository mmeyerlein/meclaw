//! Issue #37 — import and export of colonies: the dedicated test pass.
//!
//! Every path that moves a colony or a part of it is exercised here as ONE
//! sweep, so the round trips, the partial failures and the interaction with the
//! No-Delete-Policy and the birth-snapshot semantics are visible in one place.
//! The matrix below is the contract; every cell either names the test that pins
//! it or states why no test is needed.
//!
//! # Terms
//!
//! * **live-read** — the artifact is read fresh on every boot; an edit takes
//!   effect at the next boot.
//! * **birth snapshot** — the artifact was copied into a database ONCE (at the
//!   node's birth) and the database wins from then on; a later edit of the
//!   source is silently ineffective.
//! * **carried** — a copy/restore moves the artifact and its content stays in
//!   effect.
//! * **reborn** — the artifact is absent after the move and is recreated from
//!   the live-read source.
//!
//! # Matrix — move path × artifact
//!
//! Move paths:
//!
//! | id  | path                                                                   |
//! |-----|------------------------------------------------------------------------|
//! | (a) | template instantiation (`add_nodes` with `template`, `templates/` → tree)|
//! | (b) | reboot of the same tree in place                                        |
//! | (c) | full directory copy to a fresh root (backup/restore incl. all `*.db`)   |
//! | (d) | config-only copy to a fresh root (no `colony.db`, no `cell.db`)         |
//! | (e) | `--validate` against a fresh / copied root                              |
//! | (f) | partial restore (`config.json` restored, `cell.db` kept)                |
//!
//! | artifact | (a) instantiation | (b) reboot | (c) full copy | (d) config-only copy | (e) validate | (f) partial restore |
//! |---|---|---|---|---|---|---|
//! | `config.json` — node header + params | **written once**, `${VAR}` baked in, `cell.id` minted, `template.json` stripped → `a1`, `a2` | **live-read** every boot, never rewritten → `b1` | carried verbatim → `c1` | carried verbatim → `d1` | live-read, no side effect → `e1` | carried verbatim, but see the two `cell.db` rows → `f1` |
//! | `cell.db` — params overlay (`params` table) | not created unless the template ships `seed/` (no test needed: `a1` already asserts the instance has no `cell.db` without `seed/`) | **survives**, wins over `config.json` for overlay keys → `b4` | carried → `c2` | **reborn** empty → `d2` | not touched (`--validate` spawns nothing; no test needed) | **kept** → overlay still wins over the restored `config.json` → `f2` |
//! | `cell.db` — birth-snapshot tables (`timer.schedules`, `store` seed) | seeded from `seed/` only, else at first spawn (covered by `a1` + `d2`) | **birth snapshot** — a `config.json` schedule edit is IGNORED → `b2`, `b3` | carried → live schedule state survives → `c2`; a DAMAGED one fails the boot loudly → `c6` | **reborn** — re-seeded from `config.json` → `d2` | probed, never opened for writing → `c6` (second half) | **kept** — a config-level restore does NOT restore behavior → `f2`; only a `cell.db` restore does → `f3` |
//! | `colony.db` — `registry` (`cell_id`, status) | fresh `cell_id` per instantiated node (covered by `a1`) | **carried**, `cell_id` stable across reboot → `b1` | carried, `cell_id` identical to the source → `c1` | **reborn**, `cell_id` re-minted → `d1` | read-only overlay probe → `e1`, `e2` | carried; a restored node the registry does not know is **never adopted** → `f4` |
//! | `colony.db` — `edges` + `hive_scopes` | mutation-applied (out of scope for this sweep; the `add_edges` path has its own suites) | **birth snapshot** — `params.graph` hints are ignored on reboot (pinned in `meclaw-testing/src/bootstrap_apply.rs::reboot_hydrates_edges_from_colony_db_and_ignores_hints`; no duplicate here) | carried → `c1` | **reborn** from `params.graph` → `d1` | plan-only static check → `e1`, `e2` | carried (same row as (c); no separate test needed) |
//! | `colony.db` — `templates` (scan index) | read at resolve time (covered by `a1`) | scanned ONCE; re-scanned only when the table is empty or on `--rescan-templates` → `b5` | **carried with the SOURCE's `filesystem_path`** — stale after a restore until a rescan → `c3` | **reborn** by the boot scan → `d1` covers the boot; `c3` states the contrast | not rescanned by `--validate` (no test needed — `--validate` returns before the colony spawns; the scan runs in `run()` ahead of it and is already covered by `c3`) | same as (c) (no separate test needed) |
//! | `blobs/` (`DiskBlobStore`) | not involved (no test needed — instantiation never writes blobs) | live directory, never a snapshot (no test needed — the store is content-addressed by UUID and has no boot-time index) | carried as plain files → `c4` | **lost** — `BlobRef`s in a carried `message_log` would dangle; a config-only copy carries no `message_log` either, so the pair stays consistent → `c4` documents the carried case | not touched (no test needed) | same as (c) (no separate test needed) |
//! | `templates/` + provenance of the copy | tree copied minus `template.json`; **no provenance is recorded anywhere** → `a1` | blacklisted from the bootstrap walk (no test needed — `walk_cell_directories` skips `templates/` at top level, covered by the whole sweep booting cleanly with a `templates/` dir present) | carried as plain files → `c3` | carried as plain files (same row) | blacklisted (no test needed) | same as (c) (no separate test needed) |
//! | `.env` | substituted into `config.json` ONCE; `$${VAR}` survives as a literal `${VAR}` → `a2` | live-read every boot, in memory only → `b6` | carried; the baked values do NOT re-bind, the escaped ones DO → `a2` + `b6` cover both arms | same as (c) | live-read → `e1` | same as (c) |
//! | SQLite WAL sidecars (`*.db-wal` / `*.db-shm`) | n/a (no test needed — instantiation leaves no open connection behind, so nothing can sit in a WAL) | checkpointed by a clean shutdown, so a cold reboot never depends on them (no test needed — every reboot cell in column (b) exercises it) | **part of the artifact whenever the colony was live** — a `*.db`-only copy silently restores stale state → `c5` | excluded on purpose → `d1`, `d2` | not touched (no test needed) | the restore unit is the whole directory → `f3` |
//!
//! # The sharp edges this sweep exists for
//!
//! 1. **Birth snapshot.** `timer` schedules live in `cell.db`
//!    (`TimerCellFactory` seeds only on `OpenStatus::Created`). Editing
//!    `params.schedules` in `config.json` after the first boot changes NOTHING
//!    — `cell.db` wins. A restore that puts only `config.json` files back
//!    restores the *declaration* but not the *behavior*. `b2`/`b3` pin the
//!    asymmetry, `f2`/`f3` pin what a restore actually has to move, and `d2`
//!    pins that a fresh root (no `cell.db`) does re-seed from `config.json`.
//! 2. **WAL sidecars.** Both `colony.db` and `cell.db` run in
//!    `journal_mode=WAL`. A clean shutdown checkpoints them, but a backup taken
//!    while the colony is live (or after a crash) finds the newest writes in
//!    `*.db-wal`. A backup pattern of `*.db` alone then restores a colony that
//!    boots perfectly and runs the PREVIOUS state — silent, not loud. `c5`
//!    pins it against a control copy that carries the sidecars.
//! 3. **Partial failures are asymmetric.** A damaged `cell.db` is caught in
//!    the plan phase and fails the boot before a single spawn (`c6`), but a
//!    *plausible* `cell.db` carrying stale content boots silently (`c5`). The
//!    substrate defends against corruption, not against staleness.
//! 4. **No provenance.** An instantiated node records nothing about the
//!    template it came from (`a1`), and the carried template index keeps the
//!    SOURCE machine's absolute paths until an explicit rescan (`c3`).

use meclaw_cli::{Cli, StdioFormat, built_in_factories, run};
use meclaw_colony::{
    BootState, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    MutationOutcome, bootstrap_from_filesystem, colony_task, probe_boot_state,
    read_registry_overlay,
};
use meclaw_core::Uuid;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (30 s convention — robust under cargo's
/// parallel test load).
const MARKER: std::time::Duration = std::time::Duration::from_secs(30);

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

/// Stable schedule id so both boots address the SAME row (the point of the
/// birth-snapshot pin: same id, different cron in `config.json`).
const SCHED_ID: &str = "0190a3f2-0000-7000-8000-0000000000f1";

// ─────────────────────────────── fixtures ────────────────────────────────

/// A minimal, self-contained colony: root hive with one graph edge, a `timer`
/// cell (birth-snapshot carrier) and a stateless `bash` sink.
///
/// Logical layout: `/` (hive) → `/tick` (timer) → `/sink` (bash). On disk the
/// root cell dir is `main/`, whose name is stripped from the logical paths.
/// A `templates/` dir with one versioned template is present so the template
/// paths of the matrix have something to resolve.
///
/// Only built-in cell types are used, so the library boot harness in this file
/// and `meclaw_cli::run` (`--validate`) see the SAME factory registry — an
/// import/export sweep must not diverge from the production one. The sink is
/// `bash` because it is stateless (no `cell.db` noise) and never runs a
/// subprocess: nothing is ever routed to it, and the schedules below fire at
/// fixed early-morning times, so no test in this file depends on timing.
fn write_tree(root: &std::path::Path, cron: &str) {
    let main = root.join("main");
    std::fs::create_dir_all(main.join("tick")).unwrap();
    std::fs::create_dir_all(main.join("sink")).unwrap();
    std::fs::write(
        main.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./tick","to":"./sink"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        main.join("tick/config.json"),
        timer_config(&[(SCHED_ID, cron)]),
    )
    .unwrap();
    std::fs::write(main.join("sink/config.json"), sink_config(7000)).unwrap();
    write_template(root);
}

/// The `bash` sink's `config.json`. `external_timeout_ms` is a really-parsed
/// param, so an edit of it is a genuine live-read probe (not a free-form key).
fn sink_config(external_timeout_ms: u64) -> String {
    format!(
        r#"{{"cell":{{"type":"bash"}},"params":{{"external_timeout_ms":{external_timeout_ms}}},{CONTRACT}}}"#
    )
}

/// A `timer` `config.json` carrying the given `(schedule_id, cron)` seed rows.
/// All crons used in this file fire at fixed early-morning times, so no
/// schedule ever actually fires during a test run — every assertion is a
/// database read, never a timing race.
fn timer_config(schedules: &[(&str, &str)]) -> String {
    let rows: Vec<String> = schedules
        .iter()
        .map(|(id, cron)| {
            format!(
                r#"{{"schedule_id":"{id}","schedule_name":"s-{id}","cron":"{cron}","emit_to":"/sink","emit_body":{{"messages":[]}}}}"#
            )
        })
        .collect();
    format!(
        r#"{{"cell":{{"type":"timer"}},"params":{{"schedules":[{}]}},{CONTRACT}}}"#,
        rows.join(",")
    )
}

/// A versioned template plus one extra payload file, so the copy semantics of
/// instantiation (whole tree minus `template.json`) are visible.
fn write_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("sink-tpl@1.0.0");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(
        tpl.join("template.json"),
        r#"{"name":"sink-tpl","version":"1.0.0","author":"suite"}"#,
    )
    .unwrap();
    std::fs::write(tpl.join("config.json"), sink_config(7000)).unwrap();
    std::fs::write(tpl.join("payload.txt"), b"template payload").unwrap();
}

/// The production factory registry — identical to what `meclaw_cli::run` uses.
fn factories() -> CellFactoryRegistry {
    built_in_factories()
}

// ────────────────────────────── boot harness ─────────────────────────────

/// One boot against `root`: `colony_task` plus the filesystem bootstrap, both
/// as join handles. Mirrors `meclaw-colony/tests/bootstrap_crash_recovery.rs`;
/// `colony.db` deliberately lives INSIDE the tree, because that is what a
/// backup/restore of `{root}` moves.
fn boot(
    root: &std::path::Path,
) -> (
    mpsc::Sender<ColonyMsg>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<meclaw_colony::BootstrapReport>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let f = factories();
    let colony_join = tokio::spawn(colony_task(meclaw_colony::ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        f.clone(),
        root.to_path_buf(),
        ColonyConfig::default(),
        None,
        None,
    )));
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx,
        colony_config: ColonyConfig::default(),
        blob_store: None,
    };
    let root_owned = root.to_path_buf();
    let apply_join = tokio::spawn(async move {
        bootstrap_from_filesystem(&root_owned, &f, &runtime)
            .await
            .expect("bootstrap must succeed")
    });
    (inbox_tx, colony_join, apply_join)
}

/// Clean shutdown — the hard barrier before any direct rusqlite read (the
/// colony.db writer thread only flushes on shutdown).
async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    tokio::time::timeout(MARKER, colony_join)
        .await
        .expect("colony shutdown hung")
        .expect("colony task must not panic");
}

/// Boot, run the bootstrap to completion, shut down again. The workhorse of
/// every reboot / restore assertion in this file.
async fn boot_and_shutdown(root: &std::path::Path) -> meclaw_colony::BootstrapReport {
    let (inbox, colony, apply) = boot(root);
    let report = apply.await.expect("bootstrap apply join");
    shutdown(inbox, colony).await;
    report
}

/// Boot expecting the bootstrap to REJECT, and return the rendered error. Used
/// by the restore cells whose contract is "fail loudly", not "come up wrong".
async fn boot_expect_bootstrap_error(root: &std::path::Path) -> String {
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let f = factories();
    let colony_join = tokio::spawn(colony_task(meclaw_colony::ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        f.clone(),
        root.to_path_buf(),
        ColonyConfig::default(),
        None,
        None,
    )));
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx,
        colony_config: ColonyConfig::default(),
        blob_store: None,
    };
    let errors = bootstrap_from_filesystem(root, &f, &runtime)
        .await
        .expect_err("the bootstrap was expected to reject this root");
    let rendered = format!("{errors:?}");
    shutdown(inbox_tx, colony_join).await;
    rendered
}

async fn send_mutation(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("mutation ack timed out")
        .unwrap()
}

async fn rescan_templates(inbox_tx: &mpsc::Sender<ColonyMsg>, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("rescan ack timed out")
        .unwrap();
}

// ────────────────────────────── copy helpers ─────────────────────────────

/// Move path (c): a full `cp -r` of `{root}` — configs, `colony.db`, every
/// `cell.db`, `templates/`, `blobs/`.
fn copy_full(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_full(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// Recursive copy that keeps only the files `keep(file_name)` accepts.
fn copy_filtered(src: &std::path::Path, dst: &std::path::Path, keep: &dyn Fn(&str) -> bool) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name_s = name.to_string_lossy().to_string();
        let to = dst.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_filtered(&entry.path(), &to, keep);
        } else if keep(&name_s) {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

fn is_sqlite_artifact(name: &str) -> bool {
    name.ends_with(".db") || name.ends_with(".db-wal") || name.ends_with(".db-shm")
}

/// Move path (d): a config-only copy — every file EXCEPT the SQLite artifacts
/// (`*.db` plus the WAL/SHM sidecars). This is what "restore the declarations
/// onto a clean machine" looks like.
fn copy_configs_only(src: &std::path::Path, dst: &std::path::Path) {
    copy_filtered(src, dst, &|n| !is_sqlite_artifact(n));
}

// ───────────────────────────── db probes ─────────────────────────────────

/// `(schedule_id, cron_expr, status)` for every row in a timer's `cell.db`.
fn schedule_rows(cell_dir: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT schedule_id, COALESCE(cron_expr,''), status FROM schedules ORDER BY schedule_id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// `(path, cell_id)` for every registry row in a `colony.db`.
fn registry_ids(db_path: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT path, cell_id FROM registry ORDER BY path")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// `(from_path, to_path)` for every edge row in a `colony.db`.
fn edge_rows(db_path: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT from_path, to_path FROM edges ORDER BY from_path, to_path")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// `(name, filesystem_path)` for every template row in a `colony.db`.
fn template_rows(db_path: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT name, filesystem_path FROM templates ORDER BY name")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn cli_validate(root: &std::path::Path, strict: bool) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        strict,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: StdioFormat::Text,
    }
}

// ══════════════════════════════════════════════════════════════════════════
// (a) template instantiation — templates/ → tree
// ══════════════════════════════════════════════════════════════════════════

/// **a1** — what an instantiated node carries, and what it does NOT carry.
///
/// The whole template tree is copied (`payload.txt` lands in the instance),
/// `template.json` is deliberately stripped, a fresh `cell.id` is minted into
/// the instance's `config.json` — and **no provenance is recorded**: neither
/// the template name nor its version appears anywhere in the instance, and the
/// `colony.db` `registry` row has no template column either. Instantiating
/// without a `seed/` directory creates NO `cell.db`; that only happens on the
/// first spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a1_instantiated_copy_strips_template_json_and_records_no_provenance() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");

    let (inbox, colony, apply) = boot(td.path());
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, td.path().join("templates")).await;

    let outcome = send_mutation(
        &inbox,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{
                    "name": "clone",
                    "template": "sink-tpl@1.0.0",
                    "override_params": { "external_timeout_ms": 4242 }
                }],
                "add_edges": [{"from": "clone", "to": "sink"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "instantiation must commit, got {outcome:?}"
    );
    shutdown(inbox, colony).await;

    let instance = td.path().join("main/clone");
    assert!(
        instance.join("config.json").is_file(),
        "the instance must carry a config.json"
    );
    assert!(
        instance.join("payload.txt").is_file(),
        "the whole template tree is copied — payload.txt must come along"
    );
    assert!(
        !instance.join("template.json").exists(),
        "template.json is template metadata and must NEVER land in the instance"
    );
    assert!(
        !instance.join("cell.db").exists(),
        "a template without seed/ must not produce a cell.db at instantiation \
         (cell.db is created lazily at first spawn)"
    );

    let raw = std::fs::read_to_string(instance.join("config.json")).unwrap();
    let cfg: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    assert!(
        cfg["cell"]["id"].is_string(),
        "instantiation mints a fresh cell.id into the instance config: {raw}"
    );
    // Provenance receipt: the instance is a detached copy. Nothing in it names
    // the template it came from — this is the CURRENT contract, pinned here so
    // a future provenance field is a deliberate, visible change.
    assert!(
        !raw.contains("sink-tpl"),
        "no template name is recorded in the instantiated config.json: {raw}"
    );
    assert!(
        !raw.contains("1.0.0"),
        "no template version is recorded in the instantiated config.json: {raw}"
    );
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('registry')")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !cols.iter().any(|c| c.contains("template")),
        "the registry table records no template provenance either, got {cols:?}"
    );
}

/// **a2** — `${VAR}` is baked into the instance ONCE; `$${VAR}` stays a literal
/// `${VAR}` on disk and is therefore the only late-bound (portable) form.
///
/// This is the export-hygiene cell of the matrix: an exported `{root}` carries
/// the *resolved* value of every plain `${VAR}` — secrets included — while the
/// escaped form re-binds per environment at boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a2_instantiation_bakes_env_and_keeps_escaped_token_late_bound() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    std::fs::write(td.path().join(".env"), "GREETING=baked-at-birth\n").unwrap();
    // Template params carry both forms of the SAME variable.
    let tpl = td.path().join("templates/sink-tpl@1.0.0");
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"bash"}},"params":{{"external_timeout_ms":7000,"baked":"${{GREETING}}","late":"$${{GREETING}}"}},{CONTRACT}}}"#
        ),
    )
    .unwrap();

    let (inbox, colony, apply) = boot(td.path());
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, td.path().join("templates")).await;
    let outcome = send_mutation(
        &inbox,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{ "name": "clone", "template": "sink-tpl@1.0.0" }],
                "add_edges": [{"from": "clone", "to": "sink"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "instantiation must commit, got {outcome:?}"
    );
    shutdown(inbox, colony).await;

    let raw = std::fs::read_to_string(td.path().join("main/clone/config.json")).unwrap();
    let cfg: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    assert_eq!(
        cfg["params"]["baked"], "baked-at-birth",
        "a plain ${{VAR}} is substituted ONCE and written to disk: {raw}"
    );
    assert_eq!(
        cfg["params"]["late"], "${GREETING}",
        "the escaped $${{VAR}} form survives to disk as a literal token: {raw}"
    );

    // Late binding proof: change .env, re-plan. Only the escaped one moves.
    std::fs::write(td.path().join(".env"), "GREETING=rebound-elsewhere\n").unwrap();
    let overlay = read_registry_overlay(&td.path().join("colony.db")).unwrap();
    let boot_state = probe_boot_state(&td.path().join("colony.db")).unwrap();
    let plan =
        meclaw_colony::plan_bootstrap_with_env(td.path(), &factories(), &overlay, boot_state, None)
            .expect("plan must succeed");
    let clone = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/clone")
        .expect("/clone planned");
    assert_eq!(
        clone.params["baked"], "baked-at-birth",
        "the baked value does NOT re-bind to the new .env — it is plain data now"
    );
    assert_eq!(
        clone.params["late"], "rebound-elsewhere",
        "the escaped token IS re-resolved from .env at every boot"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// (b) reboot of the same tree
// ══════════════════════════════════════════════════════════════════════════

/// **b1** — `config.json` is live-read on every boot, `cell_id` is not.
///
/// Editing `params` between two boots takes effect at the next boot, while the
/// node's identity comes from the `colony.db` registry overlay and stays put.
/// The two halves together are the reboot contract: declarations move with the
/// file, identity moves with the database.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b1_reboot_rereads_config_json_but_keeps_cell_id_from_colony_db() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let db_path = td.path().join("colony.db");

    boot_and_shutdown(td.path()).await;
    let ids_before = registry_ids(&db_path);
    assert!(
        ids_before.iter().any(|(p, _)| p == "/sink"),
        "/sink registered on the first boot, got {ids_before:?}"
    );

    // Edit a live-read field of the sink between the two boots.
    std::fs::write(td.path().join("main/sink/config.json"), sink_config(9999)).unwrap();

    boot_and_shutdown(td.path()).await;

    let overlay = read_registry_overlay(&db_path).unwrap();
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &factories(),
        &overlay,
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("plan must succeed");
    let sink = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/sink")
        .expect("/sink planned");
    assert_eq!(
        sink.params["external_timeout_ms"], 9999,
        "config.json is live-read: the edited params must be in effect after the reboot"
    );
    assert_eq!(
        registry_ids(&db_path),
        ids_before,
        "cell_ids come from the colony.db registry and stay stable across a reboot"
    );
}

/// **b2** — THE sharp edge, arm 1: editing an existing schedule's cron in
/// `config.json` is silently ineffective after the first boot.
///
/// `TimerCellFactory` seeds `cell.db.schedules` only on `OpenStatus::Created`.
/// Once the file exists, `cell.db` is the truth and `config.json` is a stale
/// declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b2_timer_schedule_is_a_birth_snapshot_config_edit_is_ignored() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let tick = td.path().join("main/tick");

    boot_and_shutdown(td.path()).await;
    let seeded = schedule_rows(&tick);
    assert_eq!(seeded.len(), 1, "first boot seeds exactly one row");
    assert_eq!(
        seeded[0].1, "0 0 5 * * *",
        "the birth cron is the config one"
    );

    // Operator edits the schedule in config.json and reboots.
    std::fs::write(
        tick.join("config.json"),
        timer_config(&[(SCHED_ID, "0 0 6 * * *")]),
    )
    .unwrap();
    boot_and_shutdown(td.path()).await;

    let after = schedule_rows(&tick);
    assert_eq!(after.len(), 1, "still exactly one row");
    assert_eq!(
        after[0].1, "0 0 5 * * *",
        "cell.db WINS: the config.json cron edit has NO effect after the first boot \
         (birth-snapshot semantics — the schedule table is seeded only on \
          OpenStatus::Created)"
    );
}

/// **b3** — THE sharp edge, arm 2: adding a schedule to `config.json` after the
/// first boot does not add it either. The seed is all-or-nothing at birth, not
/// an additive merge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b3_timer_schedule_added_to_config_after_first_boot_is_not_seeded() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let tick = td.path().join("main/tick");

    boot_and_shutdown(td.path()).await;
    assert_eq!(schedule_rows(&tick).len(), 1);

    std::fs::write(
        tick.join("config.json"),
        timer_config(&[
            (SCHED_ID, "0 0 5 * * *"),
            ("0190a3f2-0000-7000-8000-0000000000f2", "0 30 7 * * *"),
        ]),
    )
    .unwrap();
    boot_and_shutdown(td.path()).await;

    let after = schedule_rows(&tick);
    assert_eq!(
        after.len(),
        1,
        "the second schedule is NOT seeded on a resume — the seed runs only on \
         OpenStatus::Created, it never merges additively, got {after:?}"
    );
}

/// **b4** — a runtime params overlay in `cell.db` survives a reboot and is
/// never written back into `config.json`.
///
/// `config.json` stays the birth declaration; the `params` table is the live
/// override. A restore that only moves `config.json` therefore cannot undo a
/// runtime params change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b4_cell_db_params_overlay_survives_reboot_and_config_json_is_not_rewritten() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let tick = td.path().join("main/tick");

    boot_and_shutdown(td.path()).await;
    let config_before = std::fs::read_to_string(tick.join("config.json")).unwrap();

    // A runtime params-update persists into cell.db.params (the same table the
    // production overlay path writes).
    {
        let conn = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
        conn.execute(
            "INSERT INTO params (key, value, updated_at) VALUES ('query_timeout_ms','1234',1) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
    }

    boot_and_shutdown(td.path()).await;

    let conn = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM params WHERE key='query_timeout_ms'",
            [],
            |r| r.get(0),
        )
        .expect("the overlay row must survive the reboot");
    assert_eq!(
        v, "1234",
        "the cell.db params overlay is carried across reboots"
    );
    assert_eq!(
        std::fs::read_to_string(tick.join("config.json")).unwrap(),
        config_before,
        "config.json is NEVER rewritten at boot — instantiation is its only writer"
    );
}

/// **b5** — the `colony.db` templates index is a scan snapshot, not a live
/// read: a boot re-scans only when the table is empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b5_templates_index_is_scanned_once_and_not_refreshed_by_a_plain_boot() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let db_path = td.path().join("colony.db");

    let (inbox, colony, apply) = boot(td.path());
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, td.path().join("templates")).await;
    shutdown(inbox, colony).await;
    assert_eq!(
        template_rows(&db_path).len(),
        1,
        "the initial scan indexes the one template"
    );

    // A second template appears on disk, but nothing forces a rescan.
    let tpl2 = td.path().join("templates/second@2.0.0");
    std::fs::create_dir_all(&tpl2).unwrap();
    std::fs::write(
        tpl2.join("template.json"),
        r#"{"name":"second","version":"2.0.0"}"#,
    )
    .unwrap();

    boot_and_shutdown(td.path()).await;
    assert_eq!(
        template_rows(&db_path).len(),
        1,
        "a plain boot does not re-scan a non-empty templates table — the index is \
         a snapshot until an explicit rescan"
    );

    let (inbox, colony, apply) = boot(td.path());
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, td.path().join("templates")).await;
    shutdown(inbox, colony).await;
    assert_eq!(
        template_rows(&db_path).len(),
        2,
        "an explicit rescan picks the new template up"
    );
}

/// **b6** — `.env` is live-read at every boot and substituted in memory only;
/// the on-disk `config.json` keeps its token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b6_env_is_live_read_at_every_boot_and_config_json_keeps_the_token() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    std::fs::write(td.path().join(".env"), "TARGET=/first\n").unwrap();
    std::fs::write(
        td.path().join("main/sink/config.json"),
        format!(
            r#"{{"cell":{{"type":"bash"}},"params":{{"external_timeout_ms":7000,"probe":"${{TARGET}}"}},{CONTRACT}}}"#
        ),
    )
    .unwrap();
    let db_path = td.path().join("colony.db");

    boot_and_shutdown(td.path()).await;
    std::fs::write(td.path().join(".env"), "TARGET=/second\n").unwrap();
    boot_and_shutdown(td.path()).await;

    let overlay = read_registry_overlay(&db_path).unwrap();
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &factories(),
        &overlay,
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("plan must succeed");
    let sink = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/sink")
        .expect("/sink planned");
    assert_eq!(
        sink.params["probe"], "/second",
        ".env is re-read and re-substituted at every boot"
    );
    assert!(
        std::fs::read_to_string(td.path().join("main/sink/config.json"))
            .unwrap()
            .contains("${TARGET}"),
        "the substitution is in-memory only — the on-disk config.json keeps its token"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// (c) full directory copy to a fresh root (backup / restore)
// ══════════════════════════════════════════════════════════════════════════

/// **c1** — a full copy of `{root}` reboots as a Reboot on the new machine and
/// is identity-preserving: `cell_id`s, edges and hive scopes are byte-identical
/// to the source.
///
/// Registry paths are logical (`/tick`), not filesystem paths, which is exactly
/// why the tree relocates cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c1_full_copy_to_a_fresh_root_reboots_and_preserves_identity_and_edges() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    let src_ids = registry_ids(&src.path().join("colony.db"));
    let src_edges = edge_rows(&src.path().join("colony.db"));
    assert!(!src_ids.is_empty() && !src_edges.is_empty());

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);

    assert_eq!(
        probe_boot_state(&restored.join("colony.db")).unwrap(),
        BootState::Reboot,
        "a full copy classifies as a Reboot before anything is booted"
    );

    let report = boot_and_shutdown(&restored).await;
    assert_eq!(
        report.cell_count, 2,
        "both cells come up on the restored root"
    );
    assert_eq!(
        registry_ids(&restored.join("colony.db")),
        src_ids,
        "cell_ids are carried by colony.db — a full restore is identity-preserving"
    );
    assert_eq!(
        edge_rows(&restored.join("colony.db")),
        src_edges,
        "edges are carried by colony.db and hydrated on the reboot"
    );
}

/// **c2** — a full copy carries live `cell.db` state, including state that was
/// changed at runtime and never existed in any `config.json`.
///
/// The runtime change is applied through the production write path
/// (`timer::db::modify_schedule_fields`, the same function the `modify` op
/// calls), so the assertion is about the artifact, not about a hand-rolled row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c2_full_copy_carries_live_cell_db_state() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    // Runtime op: retune the schedule. Nothing in config.json knows about it.
    {
        let conn = rusqlite::Connection::open(src.path().join("main/tick/cell.db")).unwrap();
        let changed = meclaw_cells::timer::db::modify_schedule_fields(
            &conn,
            Uuid::parse_str(SCHED_ID).unwrap(),
            Some("0 45 9 * * *"),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(changed, 1, "the runtime modify must hit exactly one row");
    }

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);
    boot_and_shutdown(&restored).await;

    let rows = schedule_rows(&restored.join("main/tick"));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1, "0 45 9 * * *",
        "the runtime-modified schedule is carried by cell.db — a full copy \
         restores behavior, not just declarations"
    );
}

/// **c3** — the carried `templates` index still points at the SOURCE's
/// filesystem path, and a plain boot does not correct it.
///
/// This is the one artifact in the matrix whose content is machine-specific:
/// `templates.filesystem_path` is an absolute path captured at scan time. After
/// a restore to a different location the index is stale until an explicit
/// rescan re-anchors it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c3_full_copy_carries_stale_template_paths_until_a_rescan() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    let (inbox, colony, apply) = boot(src.path());
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, src.path().join("templates")).await;
    shutdown(inbox, colony).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);
    boot_and_shutdown(&restored).await;

    let rows = template_rows(&restored.join("colony.db"));
    assert_eq!(rows.len(), 1, "the template index is carried, got {rows:?}");
    assert!(
        rows[0].1.starts_with(src.path().to_str().unwrap()),
        "the carried filesystem_path still points at the SOURCE root — a plain \
         boot does not re-anchor it (rows: {rows:?})"
    );
    assert!(
        !rows[0].1.starts_with(restored.to_str().unwrap()),
        "and it does NOT point at the restored root (rows: {rows:?})"
    );

    // The repair is an explicit rescan against the new root.
    let (inbox, colony, apply) = boot(&restored);
    apply.await.expect("bootstrap apply join");
    rescan_templates(&inbox, restored.join("templates")).await;
    shutdown(inbox, colony).await;
    let rows = template_rows(&restored.join("colony.db"));
    assert!(
        rows.iter()
            .all(|(_, p)| p.starts_with(restored.to_str().unwrap())),
        "an explicit rescan re-anchors the index to the new root, got {rows:?}"
    );
}

/// **c4** — `blobs/` is a plain directory of files: a full copy carries it and
/// the boot never indexes or validates it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c4_full_copy_carries_the_blob_directory_verbatim() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    let blobs = src.path().join("blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(
        blobs.join("0190a3f2-0000-7000-8000-0000000000b1.bin"),
        b"payload",
    )
    .unwrap();
    boot_and_shutdown(src.path()).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);
    let report = boot_and_shutdown(&restored).await;

    assert_eq!(
        report.cell_count, 2,
        "blobs/ is blacklisted from the bootstrap walk and never becomes a cell"
    );
    assert_eq!(
        std::fs::read(restored.join("blobs/0190a3f2-0000-7000-8000-0000000000b1.bin")).unwrap(),
        b"payload",
        "blob files are carried verbatim by a directory copy"
    );
}

/// **c5** — the SQLite artifacts are `*.db` **plus** their `-wal`/`-shm`
/// sidecars. A backup that matches only `*.db` is not a backup: it silently
/// restores STALE state.
///
/// Both `colony.db` and `cell.db` run in `journal_mode=WAL`. A clean shutdown
/// does checkpoint them, but a backup taken while the colony is LIVE (or after
/// a crash) finds the newest writes in the `-wal` sidecar, not in the base
/// file. This test reproduces exactly that state deterministically: a second
/// connection is held open across the write, so SQLite does not checkpoint
/// (only the LAST connection to close a WAL database checkpoints it).
///
/// The failure mode is the dangerous kind — the restored colony boots
/// perfectly and runs the schedule the operator believed they had replaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c5_db_copy_without_wal_sidecars_silently_restores_stale_state() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;
    let tick = src.path().join("main/tick");
    assert_eq!(schedule_rows(&tick)[0].1, "0 0 5 * * *");

    // A holder connection stands in for the live colony: while it is open, a
    // closing writer connection does NOT checkpoint, so the new value stays in
    // cell.db-wal — the state any hot backup encounters.
    let keep = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
    // Touch it so SQLite really attaches to the WAL/shm — a freshly opened but
    // unused connection does not yet count as "a connection on this database"
    // for checkpoint-on-close purposes.
    let _: i64 = keep
        .query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
        .unwrap();
    {
        let w = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
        meclaw_cells::timer::db::modify_schedule_fields(
            &w,
            Uuid::parse_str(SCHED_ID).unwrap(),
            Some("0 20 4 * * *"),
            None,
            None,
            None,
        )
        .unwrap();
    }
    assert_eq!(
        schedule_rows(&tick)[0].1,
        "0 20 4 * * *",
        "the new schedule is live (readable through the WAL)"
    );
    let wal = tick.join("cell.db-wal");
    assert!(
        wal.is_file() && wal.metadata().unwrap().len() > 0,
        "receipt: the write sits in the un-checkpointed cell.db-wal"
    );

    // Backup 1: `*.db` only — the sidecars are left behind.
    let dst1 = tempfile::TempDir::new().unwrap();
    let no_wal = dst1.path().join("restored");
    copy_filtered(src.path(), &no_wal, &|n| {
        !is_sqlite_artifact(n) || n.ends_with(".db")
    });
    // Backup 2 (control): the whole directory, sidecars included.
    let dst2 = tempfile::TempDir::new().unwrap();
    let with_wal = dst2.path().join("restored");
    copy_full(src.path(), &with_wal);

    drop(keep);

    let report = boot_and_shutdown(&no_wal).await;
    assert_eq!(
        report.cell_count, 2,
        "the WAL-less restore boots without a single complaint"
    );
    assert_eq!(
        schedule_rows(&no_wal.join("main/tick"))[0].1,
        "0 0 5 * * *",
        "SILENT data loss: the `*.db`-only backup restored the PRE-WAL schedule.          The restore looks healthy and runs the wrong schedule"
    );

    boot_and_shutdown(&with_wal).await;
    assert_eq!(
        schedule_rows(&with_wal.join("main/tick"))[0].1,
        "0 20 4 * * *",
        "control: carrying the -wal/-shm sidecars restores the actual state"
    );
}

/// **c6** — a restore that carries a DAMAGED `cell.db` is rejected loudly, and
/// nothing is spawned. The partial-failure counterpart to `c5`.
///
/// `c5` shows the dangerous case (a plausible-looking database with stale
/// content boots silently). This is the other end: a truncated / garbled
/// `cell.db` — the classic half-written backup — must never boot into an empty
/// cell that quietly loses its state. `plan_bootstrap` probes every `cell.db`
/// before the apply phase, so the whole boot fails before a single spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c6_restore_with_a_damaged_cell_db_is_rejected_before_any_spawn() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);

    // Half-written backup: the cell.db arrived truncated.
    let damaged = restored.join("main/tick/cell.db");
    std::fs::write(&damaged, b"not a sqlite file at all").unwrap();
    let _ = std::fs::remove_file(restored.join("main/tick/cell.db-wal"));
    let _ = std::fs::remove_file(restored.join("main/tick/cell.db-shm"));

    let rendered = boot_expect_bootstrap_error(&restored).await;
    assert!(
        rendered.contains("CorruptCellDb"),
        "a damaged cell.db must fail the boot LOUDLY in the plan phase, never \
         start an empty cell, got: {rendered}"
    );
    assert!(
        rendered.contains("tick"),
        "the diagnostic must name the offending cell directory, got: {rendered}"
    );

    // `--validate` gives the operator the same verdict without booting.
    assert!(
        run(cli_validate(&restored, false)).await.is_err(),
        "--validate must reject a damaged cell.db too (nginx -t role)"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// (d) config-only copy to a fresh root
// ══════════════════════════════════════════════════════════════════════════

/// **d1** — a config-only copy is a FirstBoot: the tree comes up, but every
/// identity is re-minted. Declarations move, identity does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d1_config_only_copy_is_a_first_boot_with_fresh_cell_ids() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;
    let src_ids = registry_ids(&src.path().join("colony.db"));

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_configs_only(src.path(), &restored);
    assert!(
        !restored.join("colony.db").exists(),
        "the config-only copy must carry no colony.db"
    );
    assert_eq!(
        probe_boot_state(&restored.join("colony.db")).unwrap(),
        BootState::FirstBoot
    );

    let report = boot_and_shutdown(&restored).await;
    assert_eq!(
        report.cell_count, 2,
        "both cells are adopted on a FirstBoot"
    );

    let new_ids = registry_ids(&restored.join("colony.db"));
    let paths: Vec<&String> = new_ids.iter().map(|(p, _)| p).collect();
    let src_paths: Vec<&String> = src_ids.iter().map(|(p, _)| p).collect();
    assert_eq!(paths, src_paths, "the same logical paths come up");
    assert!(
        new_ids.iter().zip(src_ids.iter()).all(|(a, b)| a.1 != b.1),
        "every cell_id is re-minted — a config-only copy is a NEW colony, not \
         the same one (src {src_ids:?}, new {new_ids:?})"
    );
    assert_eq!(
        edge_rows(&restored.join("colony.db")),
        edge_rows(&src.path().join("colony.db")),
        "edges are reborn from params.graph and end up identical"
    );
}

/// **d2** — a config-only copy re-seeds the birth-snapshot tables: with no
/// `cell.db` present, `config.json` is the source again.
///
/// This is the counterpart to `b2`/`b3`: the ONLY way a `config.json` schedule
/// edit takes effect is a cell.db-less start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d2_config_only_copy_reseeds_timer_schedules_from_config_json() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;
    assert_eq!(
        schedule_rows(&src.path().join("main/tick"))[0].1,
        "0 0 5 * * *"
    );

    // The operator fixes the schedule in config.json — ineffective in place (b2),
    // effective on a fresh root.
    std::fs::write(
        src.path().join("main/tick/config.json"),
        timer_config(&[(SCHED_ID, "0 0 6 * * *")]),
    )
    .unwrap();

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_configs_only(src.path(), &restored);
    assert!(
        !restored.join("main/tick/cell.db").exists(),
        "the config-only copy must carry no cell.db"
    );

    boot_and_shutdown(&restored).await;
    let rows = schedule_rows(&restored.join("main/tick"));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1, "0 0 6 * * *",
        "with no cell.db present the timer re-seeds from config.json — the \
         declaration wins exactly once, at birth"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// (e) --validate against a fresh / copied root
// ══════════════════════════════════════════════════════════════════════════

/// **e1** — `--validate` (also `--strict`) is clean on a config-only copy and
/// leaves no side effects: no `colony.db` is created, no `cell.db` is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_validate_passes_on_a_config_only_copy_without_side_effects() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_configs_only(src.path(), &restored);

    run(cli_validate(&restored, true))
        .await
        .expect("--validate --strict must pass on a clean config-only restore");
    assert!(
        !restored.join("main/tick/cell.db").exists(),
        "--validate must not spawn anything — no cell.db may appear"
    );
}

/// **e2** — `--validate` is clean on a full copy too: the carried `colony.db`
/// classifies as a Reboot and every FS node is known to its registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2_validate_passes_on_a_full_copy_as_a_reboot() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);
    assert_eq!(
        probe_boot_state(&restored.join("colony.db")).unwrap(),
        BootState::Reboot
    );

    run(cli_validate(&restored, true))
        .await
        .expect("--validate --strict must pass on a full restore");
}

/// **e3** — `--validate --strict` catches the half restore: a cell directory
/// that came back from a backup but is unknown to the carried `colony.db` is
/// reported, never adopted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e3_validate_strict_flags_a_restored_cell_dir_the_registry_does_not_know() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);

    // Half restore: an extra cell dir from an older backup lands in the tree,
    // but the carried colony.db never knew it.
    std::fs::create_dir_all(restored.join("main/ghost")).unwrap();
    std::fs::write(restored.join("main/ghost/config.json"), sink_config(7000)).unwrap();

    run(cli_validate(&restored, false))
        .await
        .expect("plain --validate WARNS about the unknown node (nginx -t role, exit 0)");
    assert!(
        run(cli_validate(&restored, true)).await.is_err(),
        "--validate --strict must FAIL: a restored dir the registry does not know \
         is never adopted on a reboot"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// (f) partial restore — config.json restored, cell.db kept
// ══════════════════════════════════════════════════════════════════════════

/// **f1** — restoring a `config.json` DOES restore everything that is live-read
/// (the control arm for `f2`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f1_config_restore_does_restore_live_read_fields() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let db_path = td.path().join("colony.db");
    let backup = std::fs::read_to_string(td.path().join("main/sink/config.json")).unwrap();

    boot_and_shutdown(td.path()).await;
    // Drift, then restore the backed-up config.json.
    std::fs::write(td.path().join("main/sink/config.json"), sink_config(1111)).unwrap();
    boot_and_shutdown(td.path()).await;
    std::fs::write(td.path().join("main/sink/config.json"), &backup).unwrap();
    boot_and_shutdown(td.path()).await;

    let overlay = read_registry_overlay(&db_path).unwrap();
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &factories(),
        &overlay,
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("plan must succeed");
    let sink = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/sink")
        .expect("/sink planned");
    assert_eq!(
        sink.params["external_timeout_ms"], 7000,
        "a live-read field IS restored by putting the config.json back"
    );
}

/// **f2** — THE headline asymmetry: restoring the `config.json` does NOT
/// restore behavior that lives in `cell.db`.
///
/// Backup the tree, drift the schedule at runtime, restore only `config.json`,
/// reboot — the drifted schedule is still in force. `f3` shows what the restore
/// actually has to move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f2_config_only_restore_does_not_restore_timer_behavior() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let tick = td.path().join("main/tick");
    let config_backup = std::fs::read_to_string(tick.join("config.json")).unwrap();

    boot_and_shutdown(td.path()).await;
    assert_eq!(schedule_rows(&tick)[0].1, "0 0 5 * * *");

    // Runtime drift via the production write path.
    {
        let conn = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
        meclaw_cells::timer::db::modify_schedule_fields(
            &conn,
            Uuid::parse_str(SCHED_ID).unwrap(),
            Some("0 15 3 * * *"),
            None,
            None,
            None,
        )
        .unwrap();
    }
    assert_eq!(schedule_rows(&tick)[0].1, "0 15 3 * * *", "drift applied");

    // "Restore from backup" — the operator puts config.json back and reboots.
    std::fs::write(tick.join("config.json"), &config_backup).unwrap();
    boot_and_shutdown(td.path()).await;

    assert_eq!(
        schedule_rows(&tick)[0].1,
        "0 15 3 * * *",
        "the config-level restore did NOT restore behavior: cell.db still holds \
         the drifted schedule. Restoring config.json alone restores the \
         DECLARATION, never the birth-snapshot state"
    );
}

/// **f3** — restoring the whole cell DIRECTORY (config + `cell.db` + its WAL
/// sidecars) DOES restore behavior. The restore unit for a stateful cell is the
/// directory, not the config file — and not the `.db` file on its own either
/// (see `c5`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f3_restoring_the_whole_cell_directory_restores_timer_behavior() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree(td.path(), "0 0 5 * * *");
    let tick = td.path().join("main/tick");

    boot_and_shutdown(td.path()).await;
    // Take the backup AFTER the first boot — that is when cell.db exists.
    let backup = tempfile::TempDir::new().unwrap();
    let cell_backup = backup.path().join("tick");
    copy_full(&tick, &cell_backup);

    {
        let conn = rusqlite::Connection::open(tick.join("cell.db")).unwrap();
        meclaw_cells::timer::db::modify_schedule_fields(
            &conn,
            Uuid::parse_str(SCHED_ID).unwrap(),
            Some("0 15 3 * * *"),
            None,
            None,
            None,
        )
        .unwrap();
    }
    assert_eq!(schedule_rows(&tick)[0].1, "0 15 3 * * *", "drift applied");

    // Directory-level restore of the cell.
    std::fs::remove_dir_all(&tick).unwrap();
    copy_full(&cell_backup, &tick);
    boot_and_shutdown(td.path()).await;

    assert_eq!(
        schedule_rows(&tick)[0].1,
        "0 0 5 * * *",
        "restoring the whole cell directory restores the behavior — the cell's \
         restore unit is its directory"
    );
}

/// **f4** — a restored cell directory the carried `colony.db` does not know is
/// never adopted at boot; and No-Delete cuts the other way too: a cell dir left
/// OUT of the restore keeps its registry row and does not break the boot.
///
/// Both halves of the half-restore failure mode in one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f4_half_restore_neither_adopts_a_new_dir_nor_drops_a_missing_one() {
    let src = tempfile::TempDir::new().unwrap();
    write_tree(src.path(), "0 0 5 * * *");
    boot_and_shutdown(src.path()).await;
    let src_ids = registry_ids(&src.path().join("colony.db"));

    let dst = tempfile::TempDir::new().unwrap();
    let restored = dst.path().join("restored");
    copy_full(src.path(), &restored);

    // (i) an extra dir the registry never knew.
    std::fs::create_dir_all(restored.join("main/ghost")).unwrap();
    std::fs::write(restored.join("main/ghost/config.json"), sink_config(7000)).unwrap();
    // (ii) a dir the restore forgot, whose registry row is still carried.
    std::fs::remove_dir_all(restored.join("main/sink")).unwrap();

    let report = boot_and_shutdown(&restored).await;
    assert_eq!(
        report.cell_count, 1,
        "only /tick is planned: /ghost is not adopted and /sink is gone from the FS"
    );

    let ids = registry_ids(&restored.join("colony.db"));
    assert!(
        !ids.iter().any(|(p, _)| p == "/ghost"),
        "a restored dir unknown to the registry is NEVER adopted on a reboot, got {ids:?}"
    );
    assert!(
        ids.iter().any(|(p, _)| p == "/sink"),
        "No-Delete: the registry row of a directory the restore forgot is kept \
         (its cell_id survives for a later repair), got {ids:?}"
    );
    assert_eq!(
        ids, src_ids,
        "the registry is exactly the source's — nothing added, nothing removed"
    );
}
