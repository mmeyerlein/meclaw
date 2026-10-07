//! GH #1037 — output caps are model-sized, not inherited from 2023.
//!
//! Measured before: 19 `llm` cells under `templates/` named a fixed
//! `max_tokens` between 512 and 8 192 (the memory hive's dream step 4 096,
//! its close step 8 192, the brains 2 048 and 4 096), and the catalogue knew
//! no output limit at all, so a cell that named nothing got the cell default
//! of 4 096. Three pins:
//!
//! 1. no shipped template names a completion cap below 16 384 -- not in an
//!    `llm` cell's params, not as a declared setting's default, not in a
//!    catalogue row's package;
//! 2. every reachable chat row of the shipped catalogue states `max_output`
//!    (a store column, so the hand pushes it with the model), and says where
//!    the figure comes from;
//! 3. the memory hive's consolidation cells continue a cut answer
//!    (`length_continuations` ≥ 1) instead of keeping half of it.

use serde_json::Value;
use std::path::{Path, PathBuf};

const FLOOR: u64 = 16_384;

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn configs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            configs(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A fixed cap is a number, a digit string or the default of an env
/// reference (`${X:-4096}`) below the floor; 0 is "none".
fn below_floor(v: &Value) -> Option<u64> {
    let n = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => {
            let s = s.trim();
            match s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
                Some(inner) => inner.split_once(":-")?.1.trim().parse::<u64>().ok(),
                None => s.parse::<u64>().ok(),
            }
        }
        _ => None,
    }?;
    (n > 0 && n < FLOOR).then_some(n)
}

#[test]
fn gh1037_the_sweep_reads_numbers_digit_strings_and_env_defaults() {
    use serde_json::json;
    assert_eq!(below_floor(&json!(4096)), Some(4096));
    assert_eq!(below_floor(&json!("8192")), Some(8192));
    assert_eq!(below_floor(&json!("${MAX_TOKENS:-4096}")), Some(4096));
    assert_eq!(below_floor(&json!("${MAX_TOKENS:-32768}")), None);
    assert_eq!(
        below_floor(&json!("${MAX_TOKENS}")),
        None,
        "no default, no cap"
    );
    assert_eq!(below_floor(&json!(0)), None, "0 is none");
}

#[test]
fn gh1037_no_shipped_template_caps_output_below_16k() {
    let root = templates_root();
    if !root.join("memory-hive").is_dir() {
        return; // R2b: no template tree in this build
    }
    let mut files = Vec::new();
    configs(&root, &mut files);
    assert!(files.len() > 50, "the sweep sees the tree: {}", files.len());
    let mut found: Vec<String> = Vec::new();
    for f in &files {
        let c = read(f);
        if c["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f.strip_prefix(&root).unwrap().display().to_string();
        for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if let Some(n) = below_floor(&c["params"][key]) {
                found.push(format!("{rel}: params.{key} = {n}"));
            }
            if let Some(n) = below_floor(&c["contract"]["settings"][key]["default"]) {
                found.push(format!("{rel}: contract.settings.{key}.default = {n}"));
            }
            if let Some(n) = below_floor(&c["params"]["provider_extra"][key]) {
                found.push(format!("{rel}: params.provider_extra.{key} = {n}"));
            }
        }
    }
    let models = root.join("llm-registry/store/seed/models.jsonl");
    for line in std::fs::read_to_string(&models).unwrap().lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        if let Some(n) = below_floor(&row["package"]["max_tokens"]) {
            found.push(format!(
                "models.jsonl {}: package.max_tokens = {n}",
                row["model_id"]
            ));
        }
    }
    assert!(
        found.is_empty(),
        "fixed output caps below {FLOOR} (GH #1037: the cell takes the model's \
         max_output from its package, or names a cap of at least {FLOOR}):\n{}",
        found.join("\n")
    );
}

#[test]
fn gh1037_every_reachable_chat_row_states_its_max_output() {
    let root = templates_root().join("llm-registry");
    if !root.join("store/seed/models.jsonl").is_file() {
        return;
    }
    let store = read(&root.join("store/config.json"));
    // A column of its own, so the hand pushes it with the model -- declared
    // with default 0, so a seed or an export written before it still loads.
    let col = &store["params"]["schema"]["models"]["max_output"];
    assert_eq!(col["type"], "int", "{col}");
    assert_eq!(col["default"], 0, "{col}");
    let text = std::fs::read_to_string(root.join("store/seed/models.jsonl")).unwrap();
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(header["schema"]["max_output"], "int", "the seed header");
    let mut seen = 0;
    for line in lines {
        let row: Value = serde_json::from_str(line).unwrap();
        let reachable = matches!(row["status"].as_str(), Some("active" | "explicit"));
        if !reachable || row["wire_dialect"] == "decisions" {
            continue;
        }
        seen += 1;
        let id = row["model_id"].as_str().unwrap_or("?");
        let n = row["max_output"].as_u64().unwrap_or(0);
        assert!(n >= FLOOR, "{id}: max_output {n}");
        assert!(
            row["note"]
                .as_str()
                .is_some_and(|n| n.contains("max_output")),
            "{id}: the note says where max_output comes from"
        );
    }
    assert!(seen > 0, "the catalogue has chat rows");
}

#[test]
fn gh1037_the_consolidation_cells_continue_a_cut_answer() {
    let root = templates_root().join("memory-hive");
    if !root.is_dir() {
        return;
    }
    for cell in ["closer", "dreamer", "judge"] {
        let c = read(&root.join(cell).join("config.json"));
        let n = c["params"]["length_continuations"].as_u64().unwrap_or(0);
        assert!(
            n >= 1,
            "memory-hive/{cell}: a cut consolidation is continued, never kept half"
        );
    }
}

/// R-HK-15/16 (2026-10-06): every reachable row states its input bounds --
/// a soft one from which a rebuild of the window pays, a hard one above which
/// the cell refuses the call -- and what a cached prompt token costs, so the
/// curator can weigh a cached window against a rebuild. Columns with default
/// 0 (a seed or an export written before them still loads); the prices are
/// `json` columns because a cached token of a cheap row costs a fraction of a
/// cent per million, and the store has no float type.
#[test]
fn gh1037_every_reachable_row_states_its_input_bounds_and_cache_price() {
    let root = templates_root().join("llm-registry");
    if !root.join("store/seed/models.jsonl").is_file() {
        return;
    }
    let store = read(&root.join("store/config.json"));
    let cols = &store["params"]["schema"]["models"];
    for (key, ty) in [
        ("input_soft", "int"),
        ("input_hard", "int"),
        ("cost_cached_in", "json"),
        ("cost_cache_write", "json"),
    ] {
        assert_eq!(cols[key]["type"], ty, "{key}: {}", cols[key]);
        assert_eq!(cols[key]["default"], 0, "{key}: {}", cols[key]);
    }
    let text = std::fs::read_to_string(root.join("store/seed/models.jsonl")).unwrap();
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    for (key, ty) in [
        ("input_soft", "int"),
        ("input_hard", "int"),
        ("cost_cached_in", "json"),
        ("cost_cache_write", "json"),
    ] {
        assert_eq!(header["schema"][key], ty, "the seed header names {key}");
    }
    let mut seen = 0;
    for line in lines {
        let row: Value = serde_json::from_str(line).unwrap();
        if !matches!(row["status"].as_str(), Some("active" | "explicit")) {
            continue;
        }
        seen += 1;
        let id = row["model_id"].as_str().unwrap_or("?");
        let soft = row["input_soft"].as_u64().unwrap_or(0);
        let hard = row["input_hard"].as_u64().unwrap_or(0);
        let window = row["context_window"].as_u64().unwrap_or(0);
        assert!(
            soft > 0 && soft < hard,
            "{id}: input_soft {soft} < input_hard {hard}"
        );
        assert!(
            hard <= window,
            "{id}: input_hard {hard} within the window {window}"
        );
        let note = row["note"].as_str().unwrap_or("");
        assert!(
            note.contains("input_soft") && note.contains("input_hard"),
            "{id}: note"
        );
        if row["wire_dialect"] == "decisions" {
            continue;
        }
        let cached = row["cost_cached_in"].as_f64().unwrap_or(0.0);
        let cost_in = row["cost_in"].as_f64().unwrap_or(0.0);
        assert!(
            cached > 0.0 && cached < cost_in,
            "{id}: cost_cached_in {cached} is a price below cost_in {cost_in}"
        );
        assert!(
            note.contains("input_cache_read"),
            "{id}: the note names the cache price source"
        );
    }
    assert!(seen > 0, "the catalogue has reachable rows");
}

/// KD diagnosis (fix 2a): the consolidation cells think on a budget of their
/// own, a cell param -- not on an effort the provider maps to whatever it
/// likes. The closer spent 6.6k of 8k on thinking at day 0.
#[test]
fn gh1037_the_consolidation_cells_think_on_a_budget() {
    let root = templates_root().join("memory-hive");
    if !root.is_dir() {
        return;
    }
    for (cell, want) in [("dreamer", 1024), ("closer", 3072)] {
        let c = read(&root.join(cell).join("config.json"));
        let p = &c["params"];
        assert_eq!(
            p["thinking_budget"], want,
            "memory-hive/{cell}: thinking_budget"
        );
        assert!(
            p["provider_extra"].get("reasoning").is_none() && p.get("reasoning_effort").is_none(),
            "memory-hive/{cell}: no effort beside the budget (provider_extra wins whole)"
        );
    }
}

/// KD diagnosis (fix 2b): the closer's schema said `"confidence": 0`, models
/// answered fractions, and `int(0.9)` filed every fact at 0 or 1 of 100. The
/// schema now names the scale, and a fraction still arriving is read as one.
#[test]
fn gh1037_the_closer_asks_confidence_on_0_to_100() {
    let root = templates_root().join("memory-hive");
    if !root.is_dir() {
        return;
    }
    let c = read(&root.join("close-glue/config.json"));
    let script = c["params"]["script_inline"].as_str().unwrap();
    assert!(
        script.contains(r#"\"confidence\": \"0-100\""#),
        "the closer schema names the 0-100 scale"
    );
    let start = script.find("def confidence_of(").expect("one scale reader");
    let end = script[start..].find("\n\n\n").map(|e| start + e).unwrap();
    let probe = format!(
        "{}\nprint([confidence_of(v) for v in (0.9, 1, 1.0, 85, '72', None, 'x', 250, -3)])",
        &script[start..end]
    );
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(&probe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "[90, 1, 100, 85, 72, 70, 70, 100, 0]"
    );
}
