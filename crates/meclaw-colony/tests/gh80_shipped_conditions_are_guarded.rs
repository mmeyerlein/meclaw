//! GH #80: the shipped topologies teach the guarded form, and this sweep says so.
//!
//! Half of #80 is a substrate change (the log level follows the error class);
//! the other half is that every reader who copies `examples/telegram-research`,
//! and every builder that generates from a template, inherited the noisy form.
//! A doc sentence does not hold that: the configs themselves have to be the
//! reference, so the sweep walks every shipped `config.json` and requires each
//! `hop.<key>` an edge condition reads to be guarded by `has()`.
//!
//! Exempt: `builder/templates/collector/**` is frozen pending design review, so
//! it is named here rather than silently skipped -- an exemption nobody can see
//! is an exemption nobody removes.
//!
//! `context.*` is deliberately NOT swept. Context keys are the promoted,
//! carried-along compartment (`builder/cookbook/cel-condition-guards.md`); the
//! per-message absence that produces #80's noise is a `hop` property.

use meclaw_colony::cel_eval::parse_condition;
use std::path::{Path, PathBuf};

/// `builder/` and `examples/` sit two levels above this crate.
fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The one frozen tree (pending design review), by path fragment.
const EXEMPT: &[&str] = &["builder/templates/collector/"];

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let p = entry.path();
        if p.is_dir() {
            json_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
}

/// Every `hop.<ident>` an expression reads, in order of first appearance.
fn hop_keys(expr: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let bytes = expr.as_bytes();
    let mut i = 0;
    while let Some(rel) = expr[i..].find("hop.") {
        let at = i + rel;
        // `hop` must not be the tail of a longer identifier (`myhop.x`).
        let boundary = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let start = at + 4;
        let end = start
            + expr[start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(expr.len() - start);
        if boundary && end > start {
            let key = expr[start..end].to_string();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        i = start.max(at + 1);
    }
    keys
}

#[test]
fn every_shipped_edge_condition_guards_the_hop_keys_it_reads() {
    let mut files = Vec::new();
    json_files(&repo_path("examples"), &mut files);
    // builder/ is not part of the export subset; in the public tree the sweep
    // holds the examples alone (same guard class as proxy_promotion_edge_e2e's
    // templates_root(), GH #49).
    let builder_templates = repo_path("builder/templates");
    if builder_templates.exists() {
        json_files(&builder_templates, &mut files);
    }
    assert!(files.len() > 20, "the sweep found almost nothing to read");

    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    for file in files {
        let shown = file.to_string_lossy().replace('\\', "/");
        if EXEMPT.iter().any(|e| shown.contains(e)) {
            continue;
        }
        let raw = std::fs::read_to_string(&file).expect("read");
        let doc: meclaw_core::serde_json::Value = match meclaw_core::serde_json::from_str(&raw) {
            Ok(v) => v,
            // A non-config json (seed data, fixtures) is not this sweep's business.
            Err(_) => continue,
        };
        let mut conditions = Vec::new();
        collect_conditions(&doc, &mut conditions);
        for cond in conditions {
            checked += 1;
            parse_condition(&cond)
                .unwrap_or_else(|e| panic!("{shown}: condition does not parse: {cond} ({e})"));
            for key in hop_keys(&cond) {
                if !cond.contains(&format!("has(hop.{key})")) {
                    offenders.push(format!("{shown}: {cond} (unguarded hop.{key})"));
                }
            }
        }
    }

    // The floor tracks what the tree offers: with builder/templates present
    // (private tree) the sweep must see the lot; the public export subset
    // carries the examples alone.
    let floor = if builder_templates.exists() { 50 } else { 15 };
    assert!(
        checked > floor,
        "only {checked} conditions swept, expected more than {floor}"
    );
    assert!(
        offenders.is_empty(),
        "GH #80: a shipped edge condition reads a hop key without has():\n  {}",
        offenders.join("\n  ")
    );
}

fn collect_conditions(node: &meclaw_core::serde_json::Value, out: &mut Vec<String>) {
    match node {
        meclaw_core::serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "condition"
                    && let Some(s) = v.as_str()
                {
                    out.push(s.to_string());
                } else {
                    collect_conditions(v, out);
                }
            }
        }
        meclaw_core::serde_json::Value::Array(items) => {
            for v in items {
                collect_conditions(v, out);
            }
        }
        _ => {}
    }
}

/// The guard has to keep the routing it guards: `||` alternatives must not lose
/// their branches to `&&` binding tighter than `||`.
#[test]
fn a_guarded_alternative_still_matches_every_branch() {
    use meclaw_colony::cel_eval::evaluate_condition;
    use meclaw_core::serde_json::{Map, json};

    let c = parse_condition(
        "has(hop.finish_reason) && (hop.finish_reason == 'stop' \
         || hop.finish_reason == 'length' || hop.finish_reason == 'error')",
    )
    .expect("parse");
    for value in ["stop", "length", "error"] {
        let mut hop = Map::new();
        hop.insert("finish_reason".into(), json!(value));
        assert!(
            evaluate_condition(&c, &Map::new(), &hop).expect("eval"),
            "branch {value} must still route"
        );
    }
    let mut hop = Map::new();
    hop.insert("finish_reason".into(), json!("tool_calls"));
    assert!(!evaluate_condition(&c, &Map::new(), &hop).expect("eval"));
    // No key at all: the guard makes it a plain `false`, not an eval error.
    assert!(!evaluate_condition(&c, &Map::new(), &Map::new()).expect("eval"));
}
