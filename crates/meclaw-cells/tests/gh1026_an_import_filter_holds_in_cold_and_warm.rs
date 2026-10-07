//! GH #1026 -- an import restriction a `code` script puts in place holds the
//! same in `cold`, `warm` and `resident`.
//!
//! A script may restrict what it imports: a finder on `sys.meta_path` that
//! refuses a name, or a `sys.path` it cleans before its first import (a
//! gate script drops `''`, the working directory, so a stray `json.py`
//! there cannot win). Both only see an import that actually reaches the import
//! system. In `warm` and `resident` the runner is the shared harness
//! (`crates/meclaw-cells/src/code/harness.py`), and it used to import `io`,
//! `json`, `traceback` -- 21 modules in all, `json`, `re`, `enum`, `tokenize`
//! among them (measured with python 3.12.3) -- before the script ran, with `''`
//! first on `sys.path`. Those modules then sat in `sys.modules`, an `import
//! json` in the script was answered from that cache without asking any finder,
//! and the script's own restriction was skipped: cold denied, warm allowed.
//!
//! The pins drive the production spawn path (`CodeCellFactory`) in all three
//! modes with the same script and compare what the script reports from inside
//! the interpreter:
//! 1. **the verdict** -- a script that refuses `json` is refused in every mode
//!    and on every message, and a script that does not refuse it is allowed in
//!    every mode (the control);
//! 2. **the start** -- the script starts with the very `sys.modules` and the
//!    very `sys.path[0]` a cold run starts with, on every message;
//! 3. **the oversized script** -- an inline script above the `argv` cap runs
//!    cold as `python3 -I <file>` (GH #844); warm and resident run its harness
//!    isolated too, so `''` is not on its path in any mode.
//!
//! The report is built from `sys` alone: the script must not import the very
//! module it is testing in order to say what it saw.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::EmissionsExt;
use std::sync::Arc;
use std::time::Duration;

/// Messages per warm/resident child: the first shows the boot, the rest show
/// that nothing a previous message did (or the harness did) leaks into the next.
const MESSAGES: usize = 3;

/// The Linux per-argv-string cap (`script_file::MAX_ARG_STRLEN`).
const MAX_ARG_STRLEN: usize = 32 * 4096;

/// The script's report, written with `sys` only. `deny` installs the finder
/// that refuses `json`; without it the same script is the control.
fn reporting_script(deny: bool) -> String {
    let filter = if deny {
        "class _Deny:\n\
         \x20   def find_spec(self, name, path=None, target=None):\n\
         \x20       if name == 'json' or name.startswith('json.'):\n\
         \x20           raise ImportError('refused by the script: ' + name)\n\
         \x20       return None\n\
         sys.meta_path.insert(0, _Deny())\n"
    } else {
        // A finder that refuses nothing, so the `finally` below is the same.
        "sys.meta_path.insert(0, type('_Pass', (), {'find_spec': lambda *a, **k: None})())\n"
    };
    format!(
        "import sys\n\
         start = sorted(sys.modules)\n\
         path0 = 'EMPTY' if sys.path and sys.path[0] == '' else 'OTHER'\n\
         empty_on_path = '' in sys.path\n\
         {filter}\
         try:\n\
         \x20   import json\n\
         \x20   verdict = 'allowed'\n\
         except ImportError:\n\
         \x20   verdict = 'denied'\n\
         finally:\n\
         \x20   sys.meta_path.pop(0)\n\
         text = 'verdict=%s|path0=%s|empty_on_path=%s|isolated=%d|modules=%s' % (\n\
         \x20   verdict, path0, empty_on_path, sys.flags.isolated, ','.join(start))\n\
         sys.stdout.write('{{\"messages\":[{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"\",\"text\":\"' + text + '\"}}]}}')\n"
    )
}

/// The same script padded above the `argv` cap with a comment.
fn oversized(script: String) -> String {
    let padded = format!("{}\n{script}", "#".repeat(MAX_ARG_STRLEN + 10_000));
    assert!(padded.len() > MAX_ARG_STRLEN, "only a pin above the cap");
    padded
}

/// One script report, split into its fields.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    verdict: String,
    path0: String,
    empty_on_path: String,
    isolated: String,
    modules: Vec<String>,
}

fn parse(text: &str) -> Report {
    let mut fields = std::collections::BTreeMap::new();
    for part in text.split('|') {
        let (k, v) = part.split_once('=').unwrap_or_else(|| panic!("{text}"));
        fields.insert(k.to_string(), v.to_string());
    }
    Report {
        verdict: fields["verdict"].clone(),
        path0: fields["path0"].clone(),
        empty_on_path: fields["empty_on_path"].clone(),
        isolated: fields["isolated"].clone(),
        modules: fields["modules"].split(',').map(str::to_string).collect(),
    }
}

/// Spawn one cell in `mode` around `script`, send `n` messages to it and return
/// the `n` reports in order. `max_concurrency: 1` keeps a warm/resident cell on
/// ONE child, so message 2 runs in the interpreter message 1 left behind.
async fn run(mode: &str, script: &str, n: usize) -> Vec<Report> {
    let raw = json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 30_000,
        "runner_mode": mode,
        "max_concurrency": 1
    });
    let (otx, mut orx) = tokio::sync::mpsc::channel(64);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/code"),
            raw,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("the params spawn");
    let tx = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };
    let mut reports = Vec::with_capacity(n);
    for _ in 0..n {
        tx.send(
            MessageBuilder::new(Path::new("/code"))
                .body(Body::Inline(json!({"messages": []})))
                .reply_to(Path::new("/sink"))
                .build(),
        )
        .await
        .expect("the cell takes the message");
        let em = tokio::time::timeout(Duration::from_secs(60), orx.recv_answer())
            .await
            .unwrap_or_else(|_| panic!("{mode}: no answer within 60 s"))
            .unwrap_or_else(|| panic!("{mode}: the cell ended without an answer"));
        let header: &Value = &em.content["header"];
        assert!(
            header["error_code"].is_null(),
            "{mode}: the script must not fail: {header}"
        );
        let text = em.content["messages"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{mode}: the report is a text turn: {}", em.content));
        reports.push(parse(text));
    }
    reports
}

/// Cold once, warm and resident `MESSAGES` times each, same script.
async fn all_modes(script: &str) -> (Report, Vec<Report>, Vec<Report>) {
    let cold = run("cold", script, 1).await.remove(0);
    let warm = run("warm", script, MESSAGES).await;
    let resident = run("resident", script, MESSAGES).await;
    (cold, warm, resident)
}

// ── 1. the verdict ───────────────────────────────────────────────────────

/// The acceptance of GH #1026: the script refuses `json`, a module the warm
/// harness itself uses. Cold refuses it; so must warm and resident, on the
/// first message and on every one after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_preloaded_module_is_refused_in_every_mode() {
    let (cold, warm, resident) = all_modes(&reporting_script(true)).await;
    assert_eq!(
        cold.verdict, "denied",
        "the cold baseline refuses: {cold:?}"
    );
    for (mode, reports) in [("warm", &warm), ("resident", &resident)] {
        for (i, r) in reports.iter().enumerate() {
            assert_eq!(
                r.verdict,
                cold.verdict,
                "{mode} message {}: the script's own filter must decide, not the \
                 harness's module cache",
                i + 1
            );
        }
    }
}

/// The control: the same script without the refusal imports `json` in every
/// mode. The fix must not turn warm into "refuses everything".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unrefused_module_is_allowed_in_every_mode() {
    let (cold, warm, resident) = all_modes(&reporting_script(false)).await;
    assert_eq!(cold.verdict, "allowed", "{cold:?}");
    for r in warm.iter().chain(resident.iter()) {
        assert_eq!(r.verdict, "allowed", "{r:?}");
    }
}

// ── 2. the start ─────────────────────────────────────────────────────────

/// What a script finds when it starts is what a cold run finds: the same
/// `sys.modules` (no module the harness needed for itself) and the same first
/// `sys.path` entry. The refusing script imports nothing that survives, so
/// this holds on every message, not only the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_mode_starts_the_script_where_a_cold_run_starts_it() {
    let (cold, warm, resident) = all_modes(&reporting_script(true)).await;
    assert!(
        !cold.modules.iter().any(|m| m == "json"),
        "cold starts without json: {cold:?}"
    );
    for (mode, reports) in [("warm", &warm), ("resident", &resident)] {
        for (i, r) in reports.iter().enumerate() {
            let extra: Vec<&String> = r
                .modules
                .iter()
                .filter(|m| !cold.modules.contains(m))
                .collect();
            let missing: Vec<&String> = cold
                .modules
                .iter()
                .filter(|m| !r.modules.contains(m))
                .collect();
            assert!(
                extra.is_empty() && missing.is_empty(),
                "{mode} message {}: sys.modules at the start differs from cold \
                 -- extra {extra:?}, missing {missing:?}",
                i + 1
            );
            assert_eq!(
                (&r.path0, &r.isolated),
                (&cold.path0, &cold.isolated),
                "{mode} message {}: the script's search path starts as cold's does",
                i + 1
            );
        }
    }
}

// ── 3. the oversized script ──────────────────────────────────────────────

/// Above the `argv` cap cold runs `python3 -I <file>` (GH #844): no `''` on the
/// path, isolated flags. A warm or resident child of the same script used to
/// run `python3 -c <harness>` with `''` first -- the very entry a gate script
/// that drops `''` (an oversized warm inline script) filters by hand. Now every mode
/// runs it isolated, and the verdict is the same as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversized_script_runs_isolated_in_every_mode() {
    let (cold, warm, resident) = all_modes(&oversized(reporting_script(true))).await;
    assert_eq!(
        (cold.isolated.as_str(), cold.empty_on_path.as_str()),
        ("1", "False"),
        "the cold baseline (GH #844): {cold:?}"
    );
    for (mode, reports) in [("warm", &warm), ("resident", &resident)] {
        for (i, r) in reports.iter().enumerate() {
            assert_eq!(
                (&r.isolated, &r.empty_on_path, &r.verdict),
                (&cold.isolated, &cold.empty_on_path, &cold.verdict),
                "{mode} message {}: an oversized script runs as its cold run does",
                i + 1
            );
        }
    }
}
