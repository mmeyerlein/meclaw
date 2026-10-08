//! GH #1085 (R-IG-1, OR-IG-8) -- a fallback window is a catalogue number.
//!
//! A producer that is asked before any model answered it, or on a road no
//! window travels, falls back to a param named `input_soft_fallback`, or
//! ending in it for a second reader of the same producer
//! (`embed_input_soft_fallback`, the window of the embedding row): the
//! `input_soft` of the catalogue row of the model its reader is BORN on
//! (OR-IG-9: the model the template names, or for a birth token the
//! llm-registry tier of it; the smallest chat row only where the reader has
//! no born model). Written into a `config.json` it would be a number by hand
//! like any other, so this file finds EVERY param whose name ends in
//! `input_soft_fallback` under `templates/` -- a `params` key, an
//! `override_params` key, the `default` of a declared setting, or the literal
//! a script reads it with (`_int("input_soft_fallback", N)`,
//! `_int("embed_input_soft_fallback", N)`) -- and holds each value to THE row
//! the same config names beside it, `<name>_row` (`input_soft_fallback_row`,
//! `embed_input_soft_fallback_row`): equal to that row's `input_soft`, not
//! merely to some row's (OR-IG-9, GH #1085). A row that moves, a fallback
//! typed beside it, a fallback that names no row or a row the catalogue does
//! not have fails here and not in a colony.
//!
//! Zero, empty or null is "no fallback" (the producer then delivers whole) and
//! passes. The scan passes with no hit at all as well, and says how many it
//! held, so a strand that adds the param is covered the moment it lands.

use meclaw_core::serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The suffix every fallback window's name ends in (OR-IG-8).
const KEY: &str = "input_soft_fallback";

/// `name` is a fallback window: `input_soft_fallback` itself, or a name that
/// ends in `_input_soft_fallback`.
fn is_fallback(name: &str) -> bool {
    name == KEY
        || name
            .strip_suffix(KEY)
            .is_some_and(|head| head.ends_with('_'))
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The suffix of the param that names a fallback's catalogue row (OR-IG-9).
const ROW: &str = "_row";

/// Every row of the shipped catalogue with its `input_soft`, whatever the
/// row's status: a retired row is still a number the catalogue states.
fn catalogue_windows() -> BTreeMap<String, u64> {
    let p = repo("templates/llm-registry/store/seed/models.jsonl");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let rows: BTreeMap<String, u64> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter_map(|r| {
            let id = r.get("model_id")?.as_str()?.to_string();
            let soft = r.get("input_soft")?.as_u64()?;
            (soft > 0).then_some((id, soft))
        })
        .collect();
    assert!(
        !rows.is_empty(),
        "the catalogue states windows: {}",
        p.display()
    );
    rows
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

/// A fallback as written: `Some(n)` for a number (or the text of one), `None`
/// for "no fallback" (0, "", null). Anything else is no window at all and is
/// reported as such.
fn window_of(v: &Value) -> Result<Option<u64>, String> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => match n.as_u64() {
            Some(0) => Ok(None),
            Some(n) => Ok(Some(n)),
            None => Err(format!("{v} is no whole number of tokens")),
        },
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => match s.trim().parse::<u64>() {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(_) => Err(format!("{v} is no whole number of tokens")),
        },
        Value::Object(o) if o.contains_key("default") => window_of(&o["default"]),
        _ => Err(format!("{v} is no window")),
    }
}

/// One fallback as written: where, under which name, and the value.
type Hit = (String, String, Value);

/// Every place `KEY` is written in one config: JSON keys anywhere in the
/// document, and the literal a script reads the param with.
fn hits(doc: &Value, at: &str, out: &mut Vec<Hit>) {
    match doc {
        Value::Object(o) => {
            for (k, v) in o {
                let here = format!("{at}.{k}");
                if is_fallback(k) {
                    out.push((here.clone(), k.clone(), v.clone()));
                }
                if let Value::String(s) = v {
                    script_literals(s, &here, out);
                }
                hits(v, &here, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                hits(v, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// `"input_soft_fallback", 237500` or `"embed_input_soft_fallback", 2048` (a
/// knob read with its default, the form of `_int(key, default)` in the
/// shipped scripts) inside a script string.
fn script_literals(s: &str, at: &str, out: &mut Vec<Hit>) {
    for quote in ['"', '\''] {
        let needle = format!("{KEY}{quote}");
        let mut from = 0;
        while let Some(i) = s[from..].find(&needle) {
            let end = from + i + needle.len();
            let head = &s[..from + i];
            from = end;
            // The whole quoted name: identifier characters back to its quote.
            let name_start = head
                .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_')
                .len();
            let name = format!("{}{KEY}", &head[name_start..]);
            if !head[..name_start].ends_with(quote) || !is_fallback(&name) {
                continue;
            }
            let Some(rest) = s[end..].trim_start().strip_prefix(',') else {
                continue;
            };
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            // Review M-2: a default that is no literal (`_int(key, CONST)`)
            // would pass unseen -- it is reported as no window instead.
            let value = if digits.is_empty() {
                Value::from(format!(
                    "<not a literal: {}>",
                    rest.trim_start().chars().take(24).collect::<String>()
                ))
            } else {
                Value::from(digits.parse::<u64>().unwrap_or(u64::MAX))
            };
            out.push((format!("{at} (script literal)"), name, value));
        }
    }
}

/// Every value `<name>_row` takes anywhere in one config (a param, an
/// override, a setting's `default`): the row a fallback names.
fn rows_named(doc: &Value, name: &str, out: &mut BTreeSet<String>) {
    match doc {
        Value::Object(o) => {
            for (k, v) in o {
                if k.strip_suffix(ROW) == Some(name) {
                    let v = v.get("default").unwrap_or(v);
                    if let Some(id) = v.as_str().filter(|s| !s.trim().is_empty()) {
                        out.insert(id.trim().to_string());
                    }
                }
                rows_named(v, name, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| rows_named(v, name, out)),
        _ => {}
    }
}

/// Holds every fallback of one config to the row it names (OR-IG-9): the
/// number of fallbacks held, and what is wrong.
fn check(doc: &Value, catalogue: &BTreeMap<String, u64>) -> (usize, Vec<String>) {
    let mut found = Vec::new();
    hits(doc, "", &mut found);
    let mut held = 0usize;
    let mut wrong = Vec::new();
    for (at, name, v) in found {
        let n = match window_of(&v) {
            Ok(None) => continue,
            Ok(Some(n)) => n,
            Err(why) => {
                wrong.push(format!("{at}: {why}"));
                continue;
            }
        };
        let mut named = BTreeSet::new();
        rows_named(doc, &name, &mut named);
        let row = match named.len() {
            1 => named.into_iter().next().unwrap_or_default(),
            0 => {
                wrong.push(format!(
                    "{at} = {n}: names no catalogue row (`{name}{ROW}`, OR-IG-9)"
                ));
                continue;
            }
            _ => {
                wrong.push(format!(
                    "{at} = {n}: `{name}{ROW}` names two rows {named:?}"
                ));
                continue;
            }
        };
        match catalogue.get(&row) {
            Some(soft) if *soft == n => held += 1,
            Some(soft) => wrong.push(format!(
                "{at} = {n}: the row it names, {row}, states input_soft {soft}"
            )),
            None => wrong.push(format!(
                "{at} = {n}: names {row}, which the catalogue does not have -- a gap in \
                 the catalogue: add the row (OR-IG-9)"
            )),
        }
    }
    (held, wrong)
}

#[test]
fn every_fallback_window_is_the_row_it_names() {
    let catalogue = catalogue_windows();
    let mut files = Vec::new();
    configs(&repo("templates"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "the scan sees the templates");
    let mut held = 0usize;
    let mut wrong = Vec::new();
    for f in &files {
        let Ok(doc) = meclaw_core::serde_json::from_str::<Value>(
            &std::fs::read_to_string(f).unwrap_or_default(),
        ) else {
            continue;
        };
        let rel = f.strip_prefix(repo("")).unwrap_or(f).display().to_string();
        let (n, bad) = check(&doc, &catalogue);
        held += n;
        wrong.extend(bad.into_iter().map(|w| format!("{rel} {w}")));
    }
    println!(
        "{KEY}: {held} fallback window(s) held to the row they name ({} rows), \
         {} config files scanned",
        catalogue.len(),
        files.len()
    );
    assert!(
        wrong.is_empty(),
        "a fallback window must be the input_soft of the catalogue row it names (OR-IG-9):\n{}",
        wrong.join("\n")
    );
    // Review M-2: a scan that finds nothing is no proof. Nine producers ship
    // a fallback (firewall, prep, recall, policy, handover, coder-pipeline and
    // research-assistant dispatch, derive twice), each as param and setting,
    // most also as a script literal.
    assert!(
        held >= MIN_HELD,
        "the scan held only {held} fallback window(s), at least {MIN_HELD} ship"
    );
}

/// The fewest fallback windows the shipped templates hold (Review M-2).
const MIN_HELD: usize = 20;

/// The scan finds what it claims to find: each written form, and a value that
/// is no catalogue row.
#[test]
fn the_scan_sees_every_form_of_the_param() {
    use meclaw_core::serde_json::json;
    let doc = json!({
        "params": {KEY: 237500,
                   "embed_input_soft_fallback": 2048,
                   "not_input_soft_fallbackish": 5,
                   "xinput_soft_fallback": 6,
                   "script_inline": "N = _int(\"input_soft_fallback\", 8000)\n\
                                     E = _int('embed_input_soft_fallback', 4096)\n\
                                     X = _int(\"xinput_soft_fallback\", 7)\n"},
        "override_params": {"x": {KEY: "250000"}},
        "contract": {"settings": {KEY: {"type": "number", "default": 0}}}
    });
    let mut found = Vec::new();
    hits(&doc, "", &mut found);
    let got: Vec<Option<u64>> = found
        .iter()
        .map(|(_, _, v)| window_of(v).unwrap())
        .collect();
    assert_eq!(found.len(), 6, "{found:?}");
    for want in [
        Some(237500),
        Some(2048),
        Some(8000),
        Some(4096),
        Some(250000),
        None,
    ] {
        assert!(got.contains(&want), "{want:?} in {found:?}");
    }
    assert!(
        !got.contains(&Some(5)) && !got.contains(&Some(6)) && !got.contains(&Some(7)),
        "a name that only contains the suffix, or glues onto it, is no fallback: {found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(_, n, _)| n == "embed_input_soft_fallback"),
        "a hit keeps its name: {found:?}"
    );
    assert!(window_of(&json!(1.5)).is_err());
    assert!(window_of(&json!("lots")).is_err());
}

/// OR-IG-9: a fallback is held to the row it NAMES, not to any row -- a value
/// of the wrong row, a fallback naming no row, a row the catalogue lacks and
/// two rows for one fallback are each wrong; settings and script agree.
#[test]
fn a_fallback_is_held_to_the_row_it_names() {
    use meclaw_core::serde_json::json;
    let catalogue: BTreeMap<String, u64> = [
        ("big/model".to_string(), 250000),
        ("small/model".to_string(), 17500),
        ("embed/model".to_string(), 2048),
    ]
    .into_iter()
    .collect();
    let good = json!({
        "params": {KEY: 17500, "input_soft_fallback_row": "small/model",
                   "embed_input_soft_fallback": 2048,
                   "embed_input_soft_fallback_row": "embed/model",
                   "script_inline": "N = _int(\"input_soft_fallback\", 17500)\n"},
        "contract": {"settings": {
            KEY: {"type": "number", "default": 17500},
            "input_soft_fallback_row": {"type": "string", "default": "small/model"}}}
    });
    let (held, wrong) = check(&good, &catalogue);
    // params (2), the script literal, the setting default: four held.
    assert_eq!((held, wrong.len()), (4, 0), "{wrong:?}");

    // Some row states 250000, but not the one this fallback names.
    let other = json!({"params": {KEY: 250000, "input_soft_fallback_row": "small/model"}});
    let (_, wrong) = check(&other, &catalogue);
    assert!(wrong[0].contains("states input_soft 17500"), "{wrong:?}");

    let unnamed = json!({"params": {KEY: 17500}});
    let (_, wrong) = check(&unnamed, &catalogue);
    assert!(wrong[0].contains("names no catalogue row"), "{wrong:?}");

    let gap = json!({"params": {KEY: 17500, "input_soft_fallback_row": "lost/model"}});
    let (_, wrong) = check(&gap, &catalogue);
    assert!(wrong[0].contains("add the row"), "{wrong:?}");

    let two = json!({
        "params": {KEY: 17500, "input_soft_fallback_row": "small/model"},
        "contract": {"settings": {"input_soft_fallback_row": {"default": "big/model"}}}
    });
    let (_, wrong) = check(&two, &catalogue);
    assert!(wrong[0].contains("names two rows"), "{wrong:?}");

    // The script literal drifting from the row is caught like the param.
    let drift = json!({"params": {KEY: 17500, "input_soft_fallback_row": "small/model",
                       "script_inline": "N = _int(\"input_soft_fallback\", 237500)\n"}});
    let (held, wrong) = check(&drift, &catalogue);
    assert_eq!(held, 1);
    assert!(wrong[0].contains("(script literal) = 237500"), "{wrong:?}");
}
