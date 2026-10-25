//! GH #1276 -- the runner-mode inventory (`templates/RUNNER-MODES.md`) and the
//! shipped `code` cells it describes, for the lock and the road tests.
//!
//! A road test boots shipped templates under its own mount points; a
//! delivery's `to_path` is an INSTANCE path. [`shipped_cell`] maps it back to
//! the template cell it was grown from -- the template root mounted above it
//! plus the relative rest, following `ref` cells into the template they name
//! -- so the check reads the SHIPPED config, never the one a road bent with
//! `override_params_on_disk`.
#![allow(dead_code)]

use meclaw_core::serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const MODES: [&str; 3] = ["cold", "warm", "resident"];
pub const PASSES: [&str; 5] = ["turn", "tool", "session", "after", "rare"];
/// What a cell on a turn road may say it passes.
pub const ON_A_TURN: [&str; 3] = ["turn", "tool", "session"];

pub fn templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Whether a row with this `mode` and `passes` must say why in writing:
/// `cold` and `resident` always (warm is the default); `warm` when the cell
/// runs `rare` -- a warm child stands 30 minutes after its last job (GH
/// #1276), and the apply broadcast wakes every warm cell that answers it
/// (a member colony on meclaw 0.70.2 held 91 python children for 48 code cells, 36 of them
/// schema answers, measured 22.10., GH #1294). A rarely called cell runs
/// `cold` unless its row says why not.
pub fn needs_a_reason(mode: &str, passes: &str) -> bool {
    mode == "cold" || mode == "resident" || (mode == "warm" && passes == "rare")
}

/// Whether this is the private tree: `plans/` never travels with the public
/// one (the marker `gh80_shipped_conditions_are_guarded` uses).
pub fn private_tree() -> bool {
    let root = templates().join("..");
    root.join("plans").is_dir()
}

/// Whether the lock judges a row whose template root is (`present`) or is not
/// in this tree. The public tree ships a subset of the library, so there a
/// row of an absent template has nothing to be held against; in the private
/// tree it is a row whose template was deleted or renamed.
pub fn judges_the_row(present: bool, private: bool) -> bool {
    present || private
}

fn read_json(p: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(p).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// The shipped runner mode of a config (absent reads `cold`, the parser's
/// default).
pub fn mode_of(cfg: &Value) -> String {
    cfg["params"]["runner_mode"]
        .as_str()
        .unwrap_or("cold")
        .to_string()
}

/// Every `code` cell under `dir`: path relative to `root` -> its mode.
pub fn code_cells(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    let cfg = dir.join("config.json");
    if cfg.is_file() {
        let v = read_json(&cfg).unwrap_or_else(|| panic!("{cfg:?}: unreadable JSON"));
        if v["cell"]["type"] == "code" {
            let rel = dir
                .strip_prefix(root)
                .expect("below the root")
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, mode_of(&v));
        }
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir:?}: {e}"))
        .map(|e| e.expect("a dir entry").path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "seed"))
        // A frozen version `templates/<name>@<x.y.z>/` (development-rules § 4)
        // is immutable and keeps the modes of its release; its directory
        // names a version, which the inventory never does.
        .filter(|p| {
            dir != root
                || !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains('@'))
        })
        .collect();
    subs.sort();
    for sub in subs {
        code_cells(root, &sub, out);
    }
}

/// One row of the inventory.
#[derive(Clone, Debug)]
pub struct Row {
    pub line: usize,
    pub cell: String,
    pub mode: String,
    pub passes: String,
    pub reason: String,
}

/// The table rows of `text`, and the lines whose column count is not four
/// (a `|` inside a reason splits it).
pub fn parse(text: &str) -> (Vec<Row>, Vec<String>) {
    let (mut rows, mut broken) = (Vec::new(), Vec::new());
    for (i, line) in text.lines().enumerate() {
        let s = line.trim();
        if !s.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = s.trim_matches('|').split('|').map(str::trim).collect();
        if cols.len() != 4 {
            broken.push(format!("line {}: {} columns, not 4", i + 1, cols.len()));
            continue;
        }
        if cols[0] == "cell" || cols[0].chars().all(|c| matches!(c, '-' | ':' | ' ')) {
            continue;
        }
        rows.push(Row {
            line: i + 1,
            cell: cols[0].trim_matches('`').to_string(),
            mode: cols[1].to_string(),
            passes: cols[2].to_string(),
            reason: cols[3].to_string(),
        });
    }
    (rows, broken)
}

/// The inventory file's text.
pub fn inventory_text() -> String {
    let file = templates().join("RUNNER-MODES.md");
    std::fs::read_to_string(&file).unwrap_or_else(|e| {
        panic!("{file:?}: {e} -- every shipped code cell needs its row there (GH #1276)")
    })
}

/// The inventory rows by cell.
pub fn inventory() -> HashMap<String, Row> {
    parse(&inventory_text())
        .0
        .into_iter()
        .map(|r| (r.cell.clone(), r))
        .collect()
}

/// `name@x.y.z` anywhere in `text` (gh408 sweeps every `.md` under
/// `templates/` for template versions).
pub fn names_a_version(text: &str) -> Option<String> {
    for word in text.split(|c: char| c.is_whitespace() || c == '`' || c == '|') {
        if let Some((name, ver)) = word.split_once('@') {
            let parts: Vec<&str> = ver
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .split('.')
                .collect();
            if !name.is_empty()
                && parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            {
                return Some(word.to_string());
            }
        }
    }
    None
}

/// The shipped template cell an instance path was grown from, when it is a
/// `code` cell. `mounts` maps an instance prefix (`/firewall`) to the template
/// root booted there (`firewall`); the longest prefix wins.
pub fn shipped_cell(mounts: &[(&str, &str)], to_path: &str) -> Option<String> {
    let (prefix, template) = mounts
        .iter()
        .filter(|(p, _)| to_path == *p || to_path.starts_with(&format!("{p}/")))
        .max_by_key(|(p, _)| p.len())?;
    let rest = to_path[prefix.len()..].trim_start_matches('/');
    let root = templates();
    let mut at = root.join(template);
    let mut depth = 0;
    for seg in std::iter::once("").chain(rest.split('/').filter(|s| !s.is_empty())) {
        if !seg.is_empty() {
            at = at.join(seg);
        }
        // A `ref` cell stands for the template it names: follow it.
        while let Some(cfg) = read_json(&at.join("config.json")) {
            if cfg["cell"]["type"] != "ref" {
                break;
            }
            let name = cfg["cell"]["template"].as_str()?.split('@').next()?;
            at = root.join(name);
            depth += 1;
            assert!(depth < 16, "{to_path}: a ref chain that does not end");
        }
    }
    let cfg = read_json(&at.join("config.json"))?;
    if cfg["cell"]["type"] != "code" {
        return None;
    }
    Some(
        at.strip_prefix(&root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

/// The rule for a code cell a turn road passed: warm or resident, or a
/// written reason; and its row says it sits on a turn. One line per breach.
pub fn judge(passed: &BTreeSet<String>) -> Vec<String> {
    let rows = inventory();
    let mut out = Vec::new();
    for cell in passed {
        let cfg = read_json(&templates().join(cell).join("config.json"))
            .unwrap_or_else(|| panic!("{cell}: a shipped config"));
        let mode = mode_of(&cfg);
        let Some(row) = rows.get(cell) else {
            out.push(format!(
                "{cell}: on the turn road, but no row in RUNNER-MODES.md"
            ));
            continue;
        };
        if mode == "cold" && row.reason.is_empty() {
            out.push(format!(
                "{cell}: on the turn road and `cold` without a reason -- rule: a cell a turn \
                 passes runs warm or resident, or says why not"
            ));
        }
        if !ON_A_TURN.contains(&row.passes.as_str()) {
            out.push(format!(
                "{cell}: on the turn road, but its row says passes `{}` -- expected one of \
                 turn|tool|session",
                row.passes
            ));
        }
    }
    out
}
