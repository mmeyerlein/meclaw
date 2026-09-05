//! GH #401 — every `timer` the library ships carries params the boot accepts.
//!
//! The companion file (`gh401_a_grown_argus_survives_a_reboot.rs`) proves one
//! grown colony restarts. This one proves the defect was singular: a template
//! whose `params` the boot rejects commits happily through `add_nodes` and kills
//! the NEXT boot, so a second such template would be a second colony that runs
//! once and never starts again, discovered the same expensive way.
//!
//! WHY THIS CALLS THE REAL PARSER
//! =============================
//! Asserting "the object has these five keys" would be a copy of the schema
//! living next to the schema, and the two would drift the first time a
//! requirement moved. `TimerCellFactory::validate_params` IS the call the boot
//! makes, so the test makes it. The only thing done to the shipped JSON first is what staging
//! does anyway: resolve the two substitution forms, because a `${uuid7:…}` token
//! is not a UUID until the colony mints one.
//!
//! It is deliberately a sweep over the tree rather than a list of names. A list
//! is a thing somebody has to remember to extend, and the template that gets
//! forgotten is the one nobody was thinking about.

use meclaw_cells::TimerCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::Value;
use std::path::{Path, PathBuf};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every `config.json` under `dir`, in path order.
fn config_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            config_files(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// What staging does to a token, done here so the parser sees the shape a booted
/// cell sees:
///
/// * `${uuid7:anything}` — the colony mints a UUID v7. Any valid one proves the
///   same thing, so a fixed one keeps the test deterministic.
/// * `${VAR:-default}` — the default, which is the value a tree with no `.env`
///   entry gets and therefore the one that has to parse.
/// * `${VAR}` — no default to fall back on; left alone, and a schedule that
///   depends on one for a *structural* field would fail here, which is correct:
///   a required key that only exists when an env var is set is a boot that
///   depends on the environment.
fn resolve(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        if token.starts_with("uuid7:") {
            out.push_str("0190a3f2-0000-7000-8000-000000000001");
        } else if let Some((_, default)) = token.split_once(":-") {
            out.push_str(default);
        } else {
            out.push_str("${");
            out.push_str(token);
            out.push('}');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[test]
fn every_shipped_timer_carries_params_the_boot_accepts() {
    let root = templates_root();
    if !root.exists() {
        return;
    }
    let mut files = Vec::new();
    config_files(&root, &mut files);

    let mut checked = 0usize;
    let mut broken: Vec<String> = Vec::new();

    for p in &files {
        let Ok(raw) = std::fs::read_to_string(p) else {
            continue;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if v["cell"]["type"].as_str() != Some("timer") {
            continue;
        }
        checked += 1;

        let resolved: Value = meclaw_core::serde_json::from_str(&resolve(&raw))
            .unwrap_or_else(|e| panic!("{} does not parse after substitution: {e}", p.display()));
        if let Err(reason) = TimerCellFactory.validate_params(&resolved["params"]) {
            let rel = p.strip_prefix(&root).unwrap_or(p).display();
            broken.push(format!("  templates/{rel}: {reason}"));
        }
    }

    assert!(
        checked > 0,
        "the sweep found no `timer` cell in templates/ — it swept nothing and \
         would have passed for a library with the defect in every clock. Check \
         the walk before trusting a green run."
    );
    assert!(
        broken.is_empty(),
        "{} of {checked} shipped timer(s) carry params the boot REJECTS. Each \
         one is a colony that grows fine and refuses to start next time (GH \
         #401):\n{}",
        broken.len(),
        broken.join("\n")
    );
}

/// Names the ruling of GH #551 § 2 leaves alone: a tick whose NAME says which
/// tick it is (`night`), plus the survivor itself. The ruling named
/// `menu-clock` here too; it left the tree with GH #553, and the menu is asked
/// for on the mutation receipt now.
const RULED_TIMER_NAMES: &[&str] = &["clock", "night"];

/// Directories that still carry an unruled timer name and are known to leave
/// during the 2026-09-04 wave. Same dated-transition shape the tree gate uses
/// (`scripts/check_tree_rules.py` § TRANSITIONAL): a row leaves with the commit
/// that lands its issue, and a row that no longer matches anything is a finding
/// of its own, so a tolerated exception cannot turn into sediment.
///
/// EMPTY since GH #551 landed: `canvy/refresh`, `daily-digest/cron` and
/// `memory-hive/cron` are all called `clock` now, so the sweep below is the
/// whole rule again and nothing is excused from it.
///
///   (path relative to `templates/`, issue, what lands)
const PENDING_TIMER_RENAMES: &[(&str, u32, &str)] = &[];

/// A schedule that only ticks: it carries no payload at all, or the one turn it
/// carries is the schedule's own name spelled out again.
fn is_pure_tick(schedule: &Value) -> bool {
    let messages = &schedule["emit_body"]["messages"];
    let Some(turns) = messages.as_array() else {
        return false;
    };
    match turns.len() {
        0 => true,
        1 => {
            let text = turns[0]["text"].as_str().unwrap_or_default();
            !text.is_empty() && Some(text) == schedule["schedule_name"].as_str()
        }
        _ => false,
    }
}

/// GH #551 § 2, ruling R-0904-5 — the pure tick is called `clock`.
///
/// The library shipped six spellings of the same cell (`cron`, `refresh`,
/// `night`, `menu-clock`, `tick`, `clock`), which is why a reader could not tell
/// a cell's job from its name. The ruling settles the plainest pair: a `timer`
/// whose schedules carry nothing but the tick is named `clock`; a tick whose
/// NAME carries the semantics keeps it.
///
/// This is a sweep and not a list, for the reason the file's header gives: a
/// list is a thing somebody has to remember to extend. The exceptions are the
/// list, and each one is either ruled or dated.
#[test]
fn every_pure_tick_is_called_clock() {
    let root = templates_root();
    if !root.exists() {
        return;
    }
    for (rel, issue, what) in PENDING_TIMER_RENAMES {
        assert!(
            root.join(rel).exists(),
            "stale exception: templates/{rel} is gone (GH #{issue}, {what}), so \
             its row in PENDING_TIMER_RENAMES no longer matches anything. \
             Delete the row in the same commit."
        );
    }

    let mut files = Vec::new();
    config_files(&root, &mut files);

    let mut ticks = 0usize;
    let mut misnamed: Vec<String> = Vec::new();

    for p in &files {
        let Ok(raw) = std::fs::read_to_string(p) else {
            continue;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&resolve(&raw)) else {
            continue;
        };
        if v["cell"]["type"].as_str() != Some("timer") {
            continue;
        }
        let Some(schedules) = v["params"]["schedules"].as_array() else {
            continue;
        };
        if schedules.is_empty() || !schedules.iter().all(is_pure_tick) {
            continue;
        }
        ticks += 1;

        let dir = p.parent().unwrap_or(&root);
        let rel = dir.strip_prefix(&root).unwrap_or(dir);
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if RULED_TIMER_NAMES.contains(&name.as_str()) {
            continue;
        }
        if PENDING_TIMER_RENAMES
            .iter()
            .any(|(pending, _, _)| Path::new(pending) == rel)
        {
            continue;
        }
        misnamed.push(format!("  templates/{}: named `{name}`", rel.display()));
    }

    assert!(
        ticks > 0,
        "the sweep found no pure tick in templates/ — it swept nothing and would \
         have passed for a library that spells the clock six ways. Check the walk \
         before trusting a green run."
    );
    assert!(
        misnamed.is_empty(),
        "{} of {ticks} pure tick(s) are not called `clock` (GH #551 § 2, ruling \
         R-0904-5). A timer that carries no payload beyond its own schedule name \
         is a clock, and the library spells it one way:\n{}",
        misnamed.len(),
        misnamed.join("\n")
    );
}
