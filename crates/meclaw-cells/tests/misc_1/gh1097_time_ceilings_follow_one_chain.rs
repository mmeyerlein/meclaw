//! GH #1097 -- the time ceilings of the shipped templates follow ONE chain.
//!
//! Before this file every `llm` cell of the library carried an operation
//! timeout and a backstop typed by hand (120/240 s, 300/400 s, thirteen times
//! 180 s, 60/90 s), none of them derived from the model it runs on: a long
//! answer (`max_output` 128000) or a slow provider was cut by a number nobody
//! could explain. The chain (`docs/cell-types.md` § `llm`, "The timeout chain
//! of a shipped template") is:
//!
//! - `external_timeout_ms` = the completion budget (`max_tokens` when the cell
//!   names one, never above the row's `max_output`; else `max_output`) over
//!   the row's measured `output_tps`, plus [`FIRST_TOKEN_MS`], rounded up to
//!   the second. A row without `output_tps` counts with the slowest row that
//!   states one.
//! - `cell.message_timeout` >= that x (1 + `length_continuations`) plus the
//!   backstop margin (`BACKSTOP_MARGIN_FLOOR_MS`, at least a tenth), rounded up
//!   to the second.
//!
//! The row is the one the cell is BORN on (OR-IG-9): the literal `model`, the
//! default of a `${VAR:-row}` token, else the row the cell names in
//! `contract.settings.external_timeout_ms_row` -- and where both exist, they
//! agree. The numbers that wait for such a cell (the curator's stale rebuild
//! claim, the registry's open question), the presenter's verdict deadline, the
//! tool round of cogny and the embedders are held to their own links of the
//! same chain below. A row that moves, a cell born on another row or a number
//! typed by hand fails here and not in a colony.

use meclaw_cells::llm::params::{
    BACKSTOP_MARGIN_DIVISOR, BACKSTOP_MARGIN_FLOOR_MS, DEFAULT_MAX_TOKENS,
};
use meclaw_core::serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The allowance for the first token on top of the output time: three times
/// the measured 99th percentile of a short call (3.0 s, `openai/gpt-6-luna`,
/// calls under 100 completion tokens), rounded up to ten seconds.
const FIRST_TOKEN_MS: u64 = 10_000;

/// Cells whose operation timeout is a DEADLINE, not an output time: an answer
/// after it is worth nothing to the reader, so the output chain does not
/// apply. `presenter/decide` is held to the decisions link below;
/// `display/judge` keeps its 8 s (a screen verdict later than that is about a
/// screen the member no longer sees, commit e8add4ead).
const DEADLINE_CELLS: &[&str] = &["presenter/decide", "display/judge"];

/// The slowest measured decision of the decisions road (its measurement,
/// eight in parallel, 2026-10-02, `E_parallel_8.max` 1006.8 ms; the live wire with 41
/// questions answered in 495 ms).
const DECIDE_MEASURED_MAX_MS: u64 = 1_007;

/// The stage's allowance for a verdict's way back after the decider's own
/// deadline (measured below 1 ms: t_verdict equals t_window on the live wire).
const VERDICT_WAY_BACK_MS: u64 = 1_000;

/// The idle window of a tool round outlasts its longest call by this much
/// (cogny `round_idle_ms`, GH #980).
const ROUND_MARGIN_MS: u64 = 30_000;

/// The reserve an embedder keeps for spawn plus the final write
/// (`DEADLINE_RESERVE_S` in both embed scripts).
const EMBED_RESERVE_MS: u64 = 2_000;

/// The registry keeps an open question this long past the translator's
/// backstop (`OPEN_QUESTION_SECONDS`).
const OPEN_QUESTION_MARGIN_S: u64 = 30;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn config(rel: &str) -> Value {
    let p = repo(&format!("templates/{rel}/config.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// One catalogue row as the chain reads it.
#[derive(Clone, Copy, Debug)]
struct Row {
    max_output: u64,
    output_tps: u64,
}

fn catalogue() -> BTreeMap<String, Row> {
    let p = repo("templates/llm-registry/store/seed/models.jsonl");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let rows: BTreeMap<String, Row> = raw
        .lines()
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter_map(|r| {
            let id = r.get("model_id")?.as_str()?.to_string();
            let row = Row {
                max_output: r.get("max_output").and_then(Value::as_u64).unwrap_or(0),
                output_tps: r.get("output_tps").and_then(Value::as_u64).unwrap_or(0),
            };
            Some((id, row))
        })
        .collect();
    assert!(!rows.is_empty(), "the catalogue has rows: {}", p.display());
    rows
}

/// The slowest row that states a measured rate: what an unmeasured row
/// counts with.
fn slowest_tps(rows: &BTreeMap<String, Row>) -> u64 {
    rows.values()
        .map(|r| r.output_tps)
        .filter(|t| *t > 0)
        .min()
        .expect("at least one catalogue row states a measured output_tps (GH #1097)")
}

fn ceil_s(ms: u64) -> u64 {
    ms.div_ceil(1000) * 1000
}

fn margin(ms: u64) -> u64 {
    BACKSTOP_MARGIN_FLOOR_MS.max(ms / BACKSTOP_MARGIN_DIVISOR)
}

/// The operation timeout of a cell born on `row` with its own cap `max_tokens`.
fn ext_of(row: Row, floor_tps: u64, max_tokens: u64) -> u64 {
    let budget = match (max_tokens, row.max_output) {
        (0, 0) => u64::from(DEFAULT_MAX_TOKENS),
        (0, listed) => listed,
        (own, 0) => own,
        (own, listed) => own.min(listed),
    };
    let tps = if row.output_tps > 0 {
        row.output_tps
    } else {
        floor_tps
    };
    ceil_s((budget * 1000).div_ceil(tps) + FIRST_TOKEN_MS)
}

/// The least backstop over `longest` ms of legitimate handling, `calls` times.
fn backstop_of(longest: u64, calls: u64) -> u64 {
    ceil_s(longest * calls + margin(longest))
}

fn configs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            configs(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// The row a `model` param names by itself: a literal, or the non-empty
/// default of a `${VAR:-row}` token. `None` for a token without one.
fn model_row(model: &str) -> Option<String> {
    if !model.contains("${") {
        return Some(model.to_string()).filter(|m| !m.is_empty());
    }
    let inner = model.strip_prefix("${")?.strip_suffix('}')?;
    let (_, default) = inner.split_once(":-")?;
    Some(default.trim().to_string()).filter(|d| !d.is_empty())
}

fn u64_at(v: &Value, path: &[&str]) -> Option<u64> {
    path.iter().try_fold(v, |v, k| v.get(*k))?.as_u64()
}

/// A `NAME = N` literal of a script.
fn script_literal(script: &str, name: &str) -> u64 {
    let line = script
        .lines()
        .find(|l| l.starts_with(&format!("{name} = ")))
        .unwrap_or_else(|| panic!("{name} is not a literal of the script"));
    line.split('=')
        .nth(1)
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{line}"))
}

#[test]
fn every_shipped_llm_cell_follows_the_timeout_chain() {
    let rows = catalogue();
    let floor = slowest_tps(&rows);
    let mut files = Vec::new();
    configs(&repo("templates"), &mut files);
    files.sort();
    let mut held = 0usize;
    let mut wrong = Vec::new();
    for f in &files {
        let Ok(doc) = meclaw_core::serde_json::from_str::<Value>(
            &std::fs::read_to_string(f).unwrap_or_default(),
        ) else {
            continue;
        };
        if doc["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .strip_prefix(repo("templates"))
            .unwrap_or(f)
            .parent()
            .unwrap_or(Path::new(""))
            .display()
            .to_string();
        if DEADLINE_CELLS.contains(&rel.as_str()) {
            continue;
        }
        let p = &doc["params"];
        let by_model = model_row(p["model"].as_str().unwrap_or_default());
        let named = doc["contract"]["settings"]["external_timeout_ms_row"]["default"]
            .as_str()
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        let row_id = match (&by_model, &named) {
            (Some(m), Some(n)) if m != n => {
                wrong.push(format!(
                    "{rel}: born on {m}, but external_timeout_ms_row names {n}"
                ));
                continue;
            }
            (Some(m), _) => m.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => {
                wrong.push(format!(
                    "{rel}: names no born row (a literal model, a `${{VAR:-row}}` default \
                     or contract.settings.external_timeout_ms_row)"
                ));
                continue;
            }
        };
        let Some(row) = rows.get(&row_id).copied() else {
            wrong.push(format!(
                "{rel}: born on {row_id}, which the catalogue does not have -- a gap in the \
                 catalogue: add the row (OR-IG-9)"
            ));
            continue;
        };
        let want = ext_of(row, floor, p["max_tokens"].as_u64().unwrap_or(0));
        let ext = p["external_timeout_ms"].as_u64().unwrap_or(0);
        if ext != want {
            wrong.push(format!(
                "{rel}: external_timeout_ms {ext}, the chain over {row_id} gives {want}"
            ));
            continue;
        }
        let calls = 1 + p["length_continuations"].as_u64().unwrap_or(0);
        let need = backstop_of(ext, calls);
        match doc["cell"]["message_timeout"].as_u64() {
            Some(msg) if msg >= need => held += 1,
            got => wrong.push(format!(
                "{rel}: message_timeout {got:?} under the chain's {need} ({calls} call(s) of {ext})"
            )),
        }
    }
    assert!(
        wrong.is_empty(),
        "every shipped llm cell follows the timeout chain (GH #1097):\n{}",
        wrong.join("\n")
    );
    // The floor is counted for the SMALLER tree: the public export carries 13
    // of the llm cells this scan holds (the private templates are not in it),
    // the full tree more. A floor over the full tree is red in the export
    // tree (GH #1101).
    assert!(held >= 13, "the scan held only {held} llm cell(s)");
}

#[test]
fn the_cells_that_wait_for_a_backstop_derive_from_it() {
    // The curator's stale rebuild claim waits for `curator/summarizer`.
    let summarizer = u64_at(&config("curator/summarizer"), &["cell", "message_timeout"])
        .expect("the curator's summarizer has a backstop");
    let policy = config("curator/policy");
    let stale = script_literal(
        policy["params"]["script_inline"]
            .as_str()
            .unwrap_or_default(),
        "REBUILD_STALE_S",
    );
    assert_eq!(
        stale * 1000,
        ceil_s(summarizer + margin(summarizer)),
        "REBUILD_STALE_S is the summarizer's backstop plus the chain's margin"
    );
    // The registry's open question waits for `llm-registry/translate`.
    let translate = u64_at(
        &config("llm-registry/translate"),
        &["cell", "message_timeout"],
    )
    .expect("the translator has a backstop");
    let hand = config("llm-registry/hand");
    let open = script_literal(
        hand["params"]["script_inline"].as_str().unwrap_or_default(),
        "OPEN_QUESTION_SECONDS",
    );
    assert_eq!(
        open,
        translate.div_ceil(1000) + OPEN_QUESTION_MARGIN_S,
        "OPEN_QUESTION_SECONDS is the translator's backstop plus {OPEN_QUESTION_MARGIN_S} s"
    );
}

#[test]
fn no_verdict_the_decider_may_still_deliver_is_late() {
    let decide = config("presenter/decide");
    let ext = u64_at(&decide, &["params", "external_timeout_ms"]).unwrap_or(0);
    assert_eq!(
        ext,
        ceil_s(2 * DECIDE_MEASURED_MAX_MS),
        "the decider's deadline is twice the slowest measured decision, to the second"
    );
    let msg = u64_at(&decide, &["cell", "message_timeout"]).unwrap_or(0);
    assert!(msg >= backstop_of(ext, 1), "decide backstop {msg}");
    let stage = config("presenter/stage");
    let want = ext + VERDICT_WAY_BACK_MS;
    assert_eq!(u64_at(&stage, &["params", "budget_ms"]), Some(want));
    assert_eq!(
        u64_at(&stage, &["contract", "settings", "budget_ms", "default"]),
        Some(want)
    );
    let script = stage["params"]["script_inline"]
        .as_str()
        .unwrap_or_default();
    assert!(
        script.contains(&format!("\"budget_ms\": {want},")),
        "the stage's DEFAULTS carry budget_ms {want}"
    );
}

#[test]
fn a_tool_round_runs_a_full_fan_out_in_one_wave() {
    let max_calls = u64_at(&config("dispatcher"), &["params", "max_calls"]).unwrap_or(0);
    let exec = u64_at(&config("projection/run"), &["params", "exec_timeout_ms"]).unwrap_or(0);
    assert!(max_calls > 0 && exec > 0);
    let round = u64_at(
        &config("cogny/collector"),
        &["override_params", "assemble", "round_idle_ms"],
    )
    .unwrap_or(0);
    assert_eq!(
        round,
        exec + ROUND_MARGIN_MS,
        "cogny's round window is a program run plus {ROUND_MARGIN_MS}"
    );
    let default_wait_max = meclaw_cells::credential::default_credential_wait_max() as u64;
    for tool in ["bash", "web_fetch", "web_search"] {
        let c = config(&format!("tools/{tool}"));
        let p = &c["params"];
        let mc = p["max_concurrency"].as_u64().unwrap_or(0);
        assert!(
            mc >= max_calls,
            "tools/{tool}: max_concurrency {mc} under the dispatcher's max_calls {max_calls}"
        );
        let ext = p["external_timeout_ms"].as_u64().unwrap_or(0);
        assert!(
            ext + ROUND_MARGIN_MS <= round,
            "tools/{tool}: a call of {ext} does not fit cogny's round of {round}"
        );
        // GH #1061: a granted cell waits for its ticket inside the backstop.
        let longest = if p.get("credential_grant_id").is_some() {
            let w = p["credential_wait_max"]
                .as_u64()
                .unwrap_or(default_wait_max);
            ext.max((w + 1).div_ceil(mc) * ext + ext)
        } else {
            ext
        };
        let msg = c["cell"]["message_timeout"].as_u64().unwrap_or(0);
        assert!(
            msg >= backstop_of(longest, 1),
            "tools/{tool}: message_timeout {msg} under {}",
            backstop_of(longest, 1)
        );
    }
    assert_eq!(
        u64_at(&config("tools/bash"), &["params", "external_timeout_ms"]),
        Some(exec),
        "a shell command runs as long as a projection's program"
    );
}

#[test]
fn an_embedder_bounds_its_own_worst_case() {
    let default_wait_max = meclaw_cells::credential::default_credential_wait_max() as u64;
    let mh = config("memory-hive/embed");
    let p = &mh["params"];
    let tries = p["query_retries"].as_u64().unwrap_or(1);
    let worst = (tries + 1) * p["query_timeout_ms"].as_u64().unwrap_or(30_000)
        + tries * p["query_retry_backoff_ms"].as_u64().unwrap_or(250)
        + EMBED_RESERVE_MS;
    let fs = config("file-space/embed");
    let q = &fs["params"];
    let fs_worst = (q["retries"].as_u64().unwrap_or(1) + 1)
        * q["timeout_ms"].as_u64().unwrap_or(20_000)
        + EMBED_RESERVE_MS;
    for (name, c, worst) in [
        ("memory-hive/embed", &mh, worst),
        ("file-space/embed", &fs, fs_worst),
    ] {
        let p = &c["params"];
        let ext = p["external_timeout_ms"].as_u64().unwrap_or(0);
        assert_eq!(
            ext,
            ceil_s(worst),
            "{name}: the operation timeout is its worst case"
        );
        let script = p["script_inline"].as_str().unwrap_or_default();
        assert!(
            script.contains(&format!("\"external_timeout_ms\", {ext})")),
            "{name}: the script reads the same default"
        );
        let mc = p["max_concurrency"].as_u64().unwrap_or(1);
        let w = p["credential_wait_max"]
            .as_u64()
            .unwrap_or(default_wait_max);
        // GH #1061: a run ahead holds its ticket for its whole operation (every
        // attempt), so the ticket wait counts in operation timeouts, not attempts.
        let longest = (w + 1).div_ceil(mc) * ext + ext;
        let msg = c["cell"]["message_timeout"].as_u64().unwrap_or(0);
        assert_eq!(
            msg,
            backstop_of(longest, 1),
            "{name}: the backstop is the chain's"
        );
    }
    let derive = config("file-space/derive");
    let ext = u64_at(&derive, &["params", "external_timeout_ms"]).unwrap_or(0);
    assert_eq!(
        u64_at(&derive, &["cell", "message_timeout"]),
        Some(backstop_of(ext, 1)),
        "file-space/derive: the backstop is the chain's"
    );
}

#[test]
fn the_chain_reads_the_row_and_borrows_the_slowest_rate() {
    let rows: BTreeMap<String, Row> = [
        (
            "fast".to_string(),
            Row {
                max_output: 128_000,
                output_tps: 104,
            },
        ),
        (
            "slow".to_string(),
            Row {
                max_output: 128_000,
                output_tps: 42,
            },
        ),
        (
            "unmeasured".to_string(),
            Row {
                max_output: 16_384,
                output_tps: 0,
            },
        ),
    ]
    .into_iter()
    .collect();
    let floor = slowest_tps(&rows);
    assert_eq!(floor, 42);
    assert_eq!(ext_of(rows["fast"], floor, 0), 1_241_000);
    assert_eq!(ext_of(rows["slow"], floor, 0), 3_058_000);
    assert_eq!(ext_of(rows["unmeasured"], floor, 0), 401_000);
    assert_eq!(
        ext_of(rows["fast"], floor, 1_040),
        20_000,
        "an own cap counts"
    );
    assert_eq!(backstop_of(1_241_000, 1), 1_366_000);
    assert_eq!(backstop_of(3_058_000, 3), 9_480_000, "two continuations");
    assert_eq!(backstop_of(30_000, 1), 40_000, "the floor of ten seconds");
    assert_eq!(model_row("${X:-a/b}").as_deref(), Some("a/b"));
    assert_eq!(model_row("${X:-}"), None);
    assert_eq!(model_row("${ctx.model}"), None);
    assert_eq!(model_row("a/b").as_deref(), Some("a/b"));
}
