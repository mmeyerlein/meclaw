//! GH #401 — every `timer` the library ships carries params the boot accepts.
//!
//! The companion file (`gh401_a_grown_steward_survives_a_reboot.rs`) proves one
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
