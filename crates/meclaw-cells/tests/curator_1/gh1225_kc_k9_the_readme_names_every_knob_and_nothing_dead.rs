//! GH #1225 (K-9) -- the curator README's § Knobs and § Pure functions say
//! what the template ships, no more and no less.
//!
//! WHY this file exists: review K-9 found the README naming constants the
//! policy script had long dropped (`QUALITY_CAP`, `SUMMARY_BUDGET`,
//! `SIDECAR_MAX_CHARS`), an old `window_plan` signature, and knobs of the
//! code cells (`max_round_plans`, `round_plan_idle_s`, `candidate_cooldown`,
//! ...) missing from the knobs table while every stream of the wave added
//! rows of its own. Two locks now: every param of a code cell of the curator
//! stands in the table under that cell and every knob of the table stands in
//! that cell's `config.json`; every function and constant § Pure functions
//! names is defined at the top level of `./policy`, with exactly the
//! parameters it lists.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use std::collections::BTreeSet;

/// Params of a code cell that wire the cell, not the curator's behaviour.
const WIRING: [&str; 5] = [
    "script_inline",
    "runner",
    "runner_mode",
    "external_timeout_ms",
    "sandbox",
];

fn readme() -> String {
    std::fs::read_to_string(repo("templates/curator/README.md")).expect("curator README")
}

/// The section of the README from `## <title>` to the next `## `.
fn section(text: &str, title: &str) -> String {
    let head = format!("## {title}\n");
    let at = text
        .find(&head)
        .unwrap_or_else(|| panic!("README has no `{head}`"));
    let rest = &text[at + head.len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].to_string()
}

/// Every `name` in backticks of a text.
fn ticked(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

/// A set of (cell, knob) pairs.
type Knobs = BTreeSet<(String, String)>;

/// (cell, knob) for every row of the knobs table.
fn table_knobs() -> Knobs {
    let mut out = BTreeSet::new();
    for line in section(&readme(), "Knobs").lines() {
        let cols: Vec<&str> = line.split('|').collect();
        if cols.len() < 4 || !cols[1].contains('`') {
            continue;
        }
        for cell in ticked(cols[1]) {
            for knob in ticked(cols[2]) {
                out.insert((cell.clone(), knob));
            }
        }
    }
    out
}

/// (cell, knob) for every param of the template's cells: the first set holds
/// the code cells only (each of them must stand in the table), the second
/// every cell, code and llm (a row is dead only when no cell carries its
/// param; OR-KF-53: an llm cell's `x_title` is contract surface too).
fn shipped_knobs() -> (Knobs, Knobs) {
    let mut code = BTreeSet::new();
    let mut every = BTreeSet::new();
    let dir = repo("templates/curator");
    for entry in std::fs::read_dir(&dir).expect("templates/curator") {
        let path = entry.expect("entry").path().join("config.json");
        if !path.is_file() {
            continue;
        }
        let cell = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .expect("cell name")
            .to_string();
        let cfg = read_json(&path);
        let is_code = cfg["cell"]["type"] == "code";
        let params = match cfg["params"].as_object() {
            Some(params) => params,
            None if !is_code => continue,
            None => panic!("{cell}: a code cell without params"),
        };
        for knob in params.keys() {
            if WIRING.contains(&knob.as_str()) {
                continue;
            }
            every.insert((cell.clone(), knob.clone()));
            if is_code {
                code.insert((cell.clone(), knob.clone()));
            }
        }
    }
    (code, every)
}

#[test]
fn every_knob_of_a_code_cell_is_in_the_table_and_every_row_is_shipped() {
    if !shipped() {
        return;
    }
    let table = table_knobs();
    let (code, every) = shipped_knobs();
    let missing: Vec<_> = code.difference(&table).collect();
    let dead: Vec<_> = table.difference(&every).collect();
    assert!(
        missing.is_empty() && dead.is_empty(),
        "README § Knobs drifts from the config.json files -- shipped but not in the table: \
         {missing:?}; in the table but not shipped: {dead:?}"
    );
}

/// The parameter names of `def <name>(...)` at the top level of a script,
/// defaults dropped; None when there is no such def.
fn params_of(script: &str, name: &str) -> Option<Vec<String>> {
    let head = format!("\ndef {name}(");
    let at = script.find(&head)? + head.len();
    let end = at + script[at..].find("):")?;
    Some(
        script[at..end]
            .split(',')
            .map(|p| p.split('=').next().unwrap_or("").trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
    )
}

#[test]
fn every_pure_function_and_constant_the_readme_names_is_in_the_policy_script() {
    if !shipped() {
        return;
    }
    let text = section(&readme(), "Pure functions");
    let policy_part = text
        .split("`./schemas`")
        .next()
        .expect("the ./policy part of § Pure functions");
    let script = format!("\n{}", script_of("policy"));
    let mut wrong = Vec::new();
    for item in ticked(policy_part) {
        if let Some((name, args)) = item.strip_suffix(')').and_then(|s| s.split_once('(')) {
            let listed: Vec<String> = args
                .split(',')
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
            match params_of(&script, name) {
                Some(code) if code == listed => {}
                code => wrong.push(format!("{name}: README {listed:?}, script {code:?}")),
            }
        } else if !item.is_empty()
            && item
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !script.contains(&format!("\n{item} = "))
        {
            wrong.push(format!("{item}: no top-level constant"));
        }
    }
    assert!(
        wrong.is_empty(),
        "README § Pure functions names what ./policy does not define: {wrong:?}"
    );
}
