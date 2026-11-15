//! GH #1526 (d58-3 grenzen, OR-583.grenzen.12) -- no shipped set and no
//! composition sizes the window in tokens.
//!
//! WHY this file exists: `keep_recent_tokens` and `window_target` were the
//! last fixed token numbers of a window: talky's profile kept 80000 tokens
//! raw and ordered a rebuild at 100000, whatever the model's window was --
//! under a model of 17500 tokens the raw budget alone was four windows, under
//! one of a million a twelfth of the soft line. They are now shares of the
//! target's window (`keep_recent_share`, `window_target_share`, the window
//! from the target's catalog row). The token knobs stay as an override for
//! tests and instances; a set or a composition shipped here that typed a
//! number into one of them would bring the fixed number back unnoticed. This
//! lock reads every `config.json` under `templates/` (frozen `@` versions
//! aside: they are what an older release shipped) and the example-set table
//! of the curator's README.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::Value;

const TOKEN_KNOBS: [&str; 2] = ["keep_recent_tokens", "window_target"];

/// Every `config.json` below `dir`, frozen `@` versions left out.
fn configs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !name.contains('@') {
                configs(&path, out);
            }
        } else if name == "config.json" {
            out.push(path);
        }
    }
}

/// `where.key = number` for every token knob set to a number in `v` -- a
/// numeric string too: the policy reads it as a number.
fn numbers_in(v: &Value, at: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let here = format!("{at}.{k}");
                let numeric =
                    x.is_number() || x.as_str().is_some_and(|s| s.trim().parse::<f64>().is_ok());
                if TOKEN_KNOBS.contains(&k.as_str()) && numeric {
                    out.push(format!("{here} = {x}"));
                }
                numbers_in(x, &here, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                numbers_in(x, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn gh_d583_grenzen_no_set_or_composition_sets_a_token_knob() {
    if !shipped() {
        return;
    }
    let mut files = Vec::new();
    configs(&repo("templates"), &mut files);
    files.sort();
    assert!(
        files.len() > 50,
        "check of the setup: the templates are there ({} configs)",
        files.len()
    );
    let mut found = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(repo("")).unwrap_or(f).display().to_string();
        numbers_in(&read_json(f), &rel, &mut found);
    }
    // The example sets of the curator's README are sets too.
    let readme =
        std::fs::read_to_string(repo("templates/curator/README.md")).expect("the curator's README");
    for line in readme.lines() {
        for knob in TOKEN_KNOBS {
            let row = format!("| `{knob}` |");
            if line.starts_with(&row) && line[row.len()..].chars().any(|c| c.is_ascii_digit()) {
                found.push(format!("templates/curator/README.md: {line}"));
            }
        }
    }
    assert_eq!(
        found,
        Vec::<String>::new(),
        "a shipped set or composition sizes the window by a share of the target's window \
         (`keep_recent_share`, `window_target_share`), never by a token number"
    );
    // The control: the reader does find a number where one stands.
    let mut probe = Vec::new();
    numbers_in(
        &meclaw_core::serde_json::json!({"params": {"window_profile": {"window_target": 1}}}),
        "probe",
        &mut probe,
    );
    assert_eq!(probe.len(), 1, "check of the setup: {probe:?}");
}
