//! GH #204 — a knob has ONE default, whoever asks.
//!
//! `RECEPTIONIST_INGRESS` had three values and no two agreed: the
//! `${VAR:-default}` in `greet`'s `script_inline` said `session-keeper`, the
//! `contract.settings.ingress.default` next to it said `session-keeper/stamp`
//! (a sealed address, refused since the boundary seal), and the README said
//! `` `session- session-keeper` ``, which is a rename substitution that ran over
//! its own output and is not a value at all.
//!
//! The declared half is the one that matters most and the one nothing was
//! reading. `contract.settings.<x>.default` is the MACHINE-READABLE statement of
//! what a knob defaults to — it is what a reader inspecting the contract sees,
//! and what any tooling reading defaults would take. The inline literal is what
//! the cell actually resolves at every read. When they disagree the template
//! answers "what is the default" differently depending on who asks, and the
//! disagreement is invisible: both halves are well-formed, both parse, and no
//! run exercises the declared one at all.
//!
//! **The two halves are compared, never re-derived.** The inline default is read
//! with the same `${VAR:-default}` substitution the colony performs at
//! instantiation, and nothing else about the value is interpreted.
//!
//! **Which setting is which env var** is decided by the two conventions the
//! shipped tree already follows, in that order: the description opens with the
//! env var's name (`"RECEPTIONIST_INGRESS -- entry port of …"`), or the env var
//! ends in `_` plus the setting's key upper-cased (`close_limit` ↔
//! `KEEPER_CLOSE_LIMIT`). A setting that links to neither is SKIPPED, because
//! that is the shape of a `params`-class knob (#136) — the collector's twenty
//! knobs are params and carry no `${VAR}` at all, and there is nothing for them
//! to disagree with. The floor at the end is what keeps that skip from
//! swallowing the whole check.

use meclaw_core::serde_json::Value;
use std::collections::HashMap;

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every `${VAR:-default}` a script carries, as `VAR -> default`.
///
/// A bare `${VAR}` is deliberately absent: it declares no default, so it is not
/// a second opinion about one. Same substitution rule as
/// `gh299_the_contract_asks_for_both_parts` and as the colony's own at
/// instantiation.
fn inline_defaults(script: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        let tail = &rest[start + 2..];
        let Some(end) = tail.find('}') else { break };
        if let Some((var, default)) = tail[..end].split_once(":-") {
            out.insert(var.to_string(), default.to_string());
        }
        rest = &tail[end + 1..];
    }
    out
}

/// The env var a contract setting speaks for, by the two shipped conventions.
fn env_var_of<'a>(
    key: &str,
    description: &'a str,
    inline: &'a HashMap<String, String>,
) -> Option<&'a String> {
    if let Some(first) = description.split_whitespace().next()
        && let Some((k, _)) = inline.get_key_value(first)
    {
        return Some(k);
    }
    let suffix = format!("_{}", key.to_uppercase());
    let upper = key.to_uppercase();
    inline.keys().find(|v| v.ends_with(&suffix) || **v == upper)
}

/// Every shipped `config.json` that carries BOTH an inline script and a
/// declared contract — the only files where the two halves can disagree.
fn scripted_contracts() -> Vec<(String, Value)> {
    let root = templates_root();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Value)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            walk(root, &p, out);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let val: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let has_script = val["params"]["script_inline"].is_string();
        let has_settings = val["contract"]["settings"].is_object();
        if has_script && has_settings {
            out.push((p.strip_prefix(root).unwrap().display().to_string(), val));
        }
    }
}

/// The declared default as the string the inline half would have produced.
/// `7200000` and `"7200000"` are the same default written two ways; anything
/// structural (an object, an array) is not a default a `${VAR}` can carry.
fn as_literal(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[test]
fn a_declared_default_is_the_default_the_script_resolves() {
    let files = scripted_contracts();
    assert!(
        files.len() >= 10,
        "the sweep found almost no scripted contracts: {}",
        files.len()
    );
    let mut compared = 0usize;
    let mut findings = Vec::new();
    for (rel, val) in &files {
        let inline = inline_defaults(val["params"]["script_inline"].as_str().unwrap());
        for (key, spec) in val["contract"]["settings"].as_object().unwrap() {
            let description = spec["description"].as_str().unwrap_or("");
            // No env twin: a `params`-class knob (#136), nothing to disagree with.
            let Some(var) = env_var_of(key, description, &inline) else {
                continue;
            };
            let Some(declared) = spec.get("default").and_then(as_literal) else {
                continue;
            };
            let resolved = &inline[var];
            compared += 1;
            if &declared != resolved {
                findings.push(format!(
                    "{rel}: contract.settings.{key}.default is {declared:?}, but the script \
                     resolves ${{{var}:-…}} to {resolved:?}. The declared half is what a reader \
                     of the contract and any tooling take; the inline half is what the cell \
                     actually runs on. One of them is wrong and nothing else can tell which."
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "a knob answers differently depending on who asks:\n  {}",
        findings.join("\n  ")
    );
    // "Nothing disagrees" and "nothing was compared" look the same from outside.
    //
    // The floor is MEASURED, in both tree shapes, and it has to hold in the
    // smaller one. Measured 2026-08-22: 61 comparisons over the private
    // `templates/`, 49 over the subset the export publishes
    // (`PUBLIC_TEMPLATES` in the maintainers' export script).
    //
    // Both numbers moved on 2026-08-27 (S5): `bot-basic` and `llm-unit` were
    // deleted, so the private pool shrank, and `daily-digest` was released, so
    // the public pool grew towards it. The two trees are closer together than
    // the measurement above describes, and the direction is the safe one for a
    // floor set on the SMALLER tree. The floor itself is unchanged and still
    // measured, not declared.
    //
    // It used to read 50, measured on 2026-08-18 at 69 private / 57 public --
    // and it went red in the public CI, not here. GH #277's W3 conversion
    // replaced the byte-copied sub-units inside `talky` and `cogny` with
    // six-line `cell.type: "ref"` markers, and those copies were carrying
    // declared defaults: `talky/summarizer/prep` (4), `talky/dispatcher` (2),
    // `talky/session-keeper/close` (2), `cogny/dispatcher` (2). The standalone
    // templates they now reference are in the pool already, so the ten
    // comparisons are not lost, they were DOUBLE-COUNTED before. Resolving the
    // refs here to win the number back would re-tell that lie; the honest move
    // is a floor that matches what is really compared.
    //
    // 40 keeps roughly a fifth of the public corpus as headroom for the next
    // composite that stops byte-copying, while still demanding four fifths of
    // it be present. The failure this guard exists for -- the env-var
    // convention matching nothing, so every setting is skipped -- collapses the
    // count towards zero, not by twenty percent.
    assert!(
        compared >= 40,
        "the check compared almost no defaults: {compared}"
    );
}
