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
//! `KEEPER_CLOSE_LIMIT`).
//!
//! ## Both forms, since GH #138
//!
//! A setting with no env twin used to be SKIPPED, on the argument that this is
//! the shape of a `params`-class knob (#136) and there is nothing for it to
//! disagree with. That was true of one template and it is about to be false of
//! every template: ruling R-0904-6 moves ~140 behaviour knobs onto the params
//! surface in one wave, and a check that only understands the OLD form would
//! have counted itself down to nothing while the tree got better — the floor
//! below would have had to be lowered at every strand until it meant nothing.
//!
//! So a knob is compared in whichever form its template has, and counted once:
//!
//! * **the environment form** — `contract.settings.<k>.default` against the
//!   `${VAR:-default}` beside it, as before;
//! * **the params form** (#136, the pattern
//!   `crates/meclaw-cells/tests/w13_collector_params.rs` pins for the collector
//!   alone) — `params.<k>` against `contract.settings.<k>.default`, and against
//!   the script's own fallback literal wherever the script reads the knob
//!   through an accessor. Three copies of one value, all three compared.
//!
//! A param that is still a `${...}` token is NOT the params form: it is the
//! environment form written one place further out, and it is skipped here so
//! that half a migration cannot look finished. The floor at the end is what
//! keeps every skip from swallowing the whole check; it now has room to GROW
//! with the wave instead of shrinking with it.

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

/// The literal a script hands its own accessor for `key`, if it reads it that
/// way: `_int("window_turns", 12)` -> `12`.
///
/// The accessor's NAME is left open — `_int`, `_float`, `_str`, `_list` are the
/// collector's four and a later template may read a knob some other way. A knob
/// this finds nothing for is not accused of anything: the two copies that
/// always exist (param and declaration) are still compared, and this is the
/// third when it is there.
fn script_literal(src: &str, key: &str) -> Option<Value> {
    let needle = format!("(\"{key}\", ");
    let rest = &src[src.find(&needle)? + needle.len()..];
    meclaw_core::serde_json::from_str(&rest[..rest.find(')')?]).ok()
}

/// Every declared default of ONE config, compared against whichever half of the
/// template it has. Returns how many knobs were compared and what disagreed.
///
/// The two forms are asked in order and a knob is counted ONCE: a setting with
/// an env twin is the environment form and is judged there; everything else with
/// a param beside it is the params form.
fn compare_config(rel: &str, val: &Value) -> (usize, Vec<String>) {
    let src = val["params"]["script_inline"].as_str().unwrap_or("");
    let inline = inline_defaults(src);
    let empty = meclaw_core::serde_json::Map::new();
    let params = val["params"].as_object().unwrap_or(&empty);
    let mut compared = 0usize;
    let mut findings = Vec::new();
    let Some(settings) = val["contract"]["settings"].as_object() else {
        return (compared, findings);
    };
    for (key, spec) in settings {
        let description = spec["description"].as_str().unwrap_or("");

        // --- the environment form ------------------------------------------
        if let Some(var) = env_var_of(key, description, &inline) {
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
            continue;
        }

        // --- the params form (#136) ----------------------------------------
        let (Some(param), Some(declared)) = (params.get(key), spec.get("default")) else {
            continue;
        };
        // A param that is STILL a substitution token has not migrated. It is
        // the environment form written one place further out, and comparing it
        // against a declaration that carries the same token would be a knob
        // agreeing with itself.
        if param.as_str().is_some_and(|s| s.contains("${")) {
            continue;
        }
        compared += 1;
        if param != declared {
            findings.push(format!(
                "{rel}: contract.settings.{key}.default is {declared}, but params.{key} is \
                 {param}. Since the knob is a param, the declaration is a SECOND copy of the \
                 shipped value and not a description of it; a reader of the contract and the \
                 cell itself would be told different things."
            ));
        }
        if let Some(literal) = script_literal(src, key)
            && &literal != param
        {
            findings.push(format!(
                "{rel}: the script's own fallback for {key} is {literal}, but params.{key} is \
                 {param}. The literal is what the cell uses when its config says nothing, so a \
                 default moved in the params block and forgotten in the script is a cell that \
                 runs on the old value the moment the knob is left unset."
            ));
        }
    }
    (compared, findings)
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
        let (n, found) = compare_config(rel, val);
        compared += n;
        findings.extend(found);
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
    //
    // The floor is UNCHANGED by GH #138 and stays measured. What changed is the
    // direction it has to survive: with both forms compared the private tree
    // measures 97 on 2026-09-04 (62 before), and each template the wave
    // migrates converts its env comparisons into params comparisons rather than
    // losing them -- memory-hive alone turns 15 into 49. A floor that had to be
    // lowered at every strand would have been a floor that measured nothing.
    assert!(
        compared >= 40,
        "the check compared almost no defaults: {compared}"
    );
}

/// The generalisation, on a fixture rather than on the tree (GH #138).
///
/// The tree sweep above cannot prove WHICH form it compared: a green run and a
/// run that skipped everything look the same from outside, which is what the
/// floor exists for. This one names both forms and asserts each is counted
/// once and each is caught when it drifts.
#[test]
fn both_forms_of_a_knob_are_compared() {
    // The env form: the knob lives in a `${VAR:-default}` and is declared
    // beside it. The params form (#136): the knob is a param, the declared
    // default is that same value, and the script's own fallback literal is the
    // third copy.
    let agreeing = meclaw_core::serde_json::json!({
        "params": {
            "script_inline": "A = \"${KEEPER_IDLE_MS:-600000}\"\nB = _int(\"window_turns\", 12)\n",
            "window_turns": 12
        },
        "contract": {"settings": {
            "idle_ms": {"description": "KEEPER_IDLE_MS -- idle cut", "default": "600000"},
            "window_turns": {"description": "rolling window", "default": 12}
        }}
    });
    let (compared, findings) = compare_config("fixture/config.json", &agreeing);
    assert!(
        findings.is_empty(),
        "the agreeing fixture disagrees: {findings:?}"
    );
    assert_eq!(
        compared, 2,
        "both forms must be counted: the env twin AND the params form"
    );

    // The params form drifting: the declared default says one thing, the
    // param says another. Before this test the whole form was invisible.
    let drifting = meclaw_core::serde_json::json!({
        "params": {
            "script_inline": "B = _int(\"window_turns\", 12)\n",
            "window_turns": 12
        },
        "contract": {"settings": {
            "window_turns": {"description": "rolling window", "default": 20}
        }}
    });
    let (compared, findings) = compare_config("fixture/config.json", &drifting);
    assert_eq!(compared, 1);
    assert_eq!(
        findings.len(),
        1,
        "a params-form knob whose declared default drifted must be a finding"
    );

    // The script's own fallback drifting from the shipped param -- the third
    // copy, the one `w13_collector_params.rs` pins for the collector alone.
    let stale_literal = meclaw_core::serde_json::json!({
        "params": {
            "script_inline": "B = _int(\"window_turns\", 8)\n",
            "window_turns": 12
        },
        "contract": {"settings": {
            "window_turns": {"description": "rolling window", "default": 12}
        }}
    });
    let (compared, findings) = compare_config("fixture/config.json", &stale_literal);
    assert_eq!(compared, 1);
    assert_eq!(
        findings.len(),
        1,
        "a script fallback that drifted from the shipped param must be a finding"
    );

    // A param that is STILL a substitution token has not migrated: it is the
    // environment form written one place further out, and comparing it against
    // a declaration carrying the same token would be a knob agreeing with
    // itself. Half a migration must not look finished.
    let unmigrated = meclaw_core::serde_json::json!({
        "params": {
            "script_inline": "TABLE = P.get(\"table\")\n",
            "table": "${ARCHIVE_TABLE:-rows}"
        },
        "contract": {"settings": {
            "table": {"description": "the table", "default": "${ARCHIVE_TABLE:-rows}"}
        }}
    });
    let (compared, findings) = compare_config("fixture/config.json", &unmigrated);
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(compared, 0, "an unmigrated param is not a params-form knob");
}
