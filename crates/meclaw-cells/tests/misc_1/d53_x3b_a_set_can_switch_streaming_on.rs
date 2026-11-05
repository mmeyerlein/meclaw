//! GH #1412 (OR-D53-100) — a set can switch the streaming params on.
//!
//! GH #1410 and #1411 built the streaming knobs into the cells: `partial_unit`
//! and `partial_min_chars` on the `llm` cell, `partial_delivery` on the proxy,
//! `speak_partials` on the voice cell. No set could reach them: the mutation
//! door admits an `override_params` key only when the addressed cell's
//! `config.json` carries it under `params` ([`check_override_params`],
//! GH #294), and the shipped templates did not declare them, so a set that
//! asked for streaming was refused with `names no param of ...`.
//!
//! Two locks:
//!
//! 1. the door admits each key on the cell that reads it, in the template a
//!    set addresses (`talky/quick`, `talky/brain`, `telegram-connector`,
//!    `voice`);
//! 2. the declared default is the cell's own default: the shipped params parse
//!    to the same cell params with and without the new key, so an instance
//!    that sets nothing behaves byte for byte as before.

use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::proxy::params::ProxyParams;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_colony::mutation::subtree::{CellNode, check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    let path = core_root().join(rel);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The shipped `templates/` directory as a registry, so `ref` cells resolve the
/// way a real mutation resolves them (GH #277).
fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn cell_of(template: &str, rel: &str) -> CellNode {
    let parsed = parse_subtree(
        &core_root().join("templates").join(template),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped template '{template}' parses: {e:?}"));
    parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == rel)
        .unwrap_or_else(|| panic!("template '{template}' has no cell '{rel}'"))
}

/// (template, cell rel_path, the key, a value that switches streaming on).
const STREAMING: &[(&str, &str, &str, &str)] = &[
    ("talky", "quick", "partial_unit", "sentence"),
    ("talky", "quick", "partial_min_chars", "20"),
    ("talky", "brain", "partial_unit", "sentence"),
    ("talky", "brain", "partial_min_chars", "20"),
    (
        "telegram-connector",
        "",
        "partial_delivery",
        "first_sentence",
    ),
    ("voice", "", "speak_partials", "1"),
];

fn on_value(key: &str, raw: &str) -> Value {
    if key == "partial_min_chars" {
        json!(raw.parse::<u32>().expect("a number"))
    } else {
        json!(raw)
    }
}

#[test]
fn the_door_admits_every_streaming_param_on_the_cell_that_reads_it() {
    for (template, rel, key, on) in STREAMING {
        let cell = cell_of(template, rel);
        let cell_key = if rel.is_empty() { None } else { Some(*rel) };
        let over = json!({ (*key): on_value(key, on) });
        if let Err(why) = check_override_params(&cell, cell_key, template, &over) {
            panic!("a set's {key} on '{template}/{rel}' must pass the door: {why:?}");
        }
    }
}

/// The shipped params of a template cell with `key` removed.
fn without(params: &Value, key: &str) -> Value {
    let mut p = params.clone();
    p.as_object_mut().expect("params object").remove(key);
    p
}

#[test]
fn the_declared_default_is_the_cells_own_default() {
    for (cfg, keys) in [
        (
            "templates/talky/quick/config.json",
            &["partial_unit", "partial_min_chars"][..],
        ),
        (
            "templates/talky/brain/config.json",
            &["partial_unit", "partial_min_chars"][..],
        ),
    ] {
        let params = read_json(cfg)["params"].clone();
        assert_eq!(
            params["partial_unit"],
            json!("off"),
            "{cfg}: partial_unit ships off"
        );
        assert_eq!(
            params["partial_min_chars"],
            json!(12),
            "{cfg}: partial_min_chars ships 12"
        );
        let mut bare = params.clone();
        for k in keys {
            bare = without(&bare, k);
        }
        let with = LlmParams::parse(&params).expect("shipped llm params parse");
        let none = LlmParams::parse(&bare).expect("llm params without the keys parse");
        assert_eq!(
            meclaw_core::serde_json::to_value(&with).unwrap(),
            meclaw_core::serde_json::to_value(&none).unwrap(),
            "{cfg}: the declared defaults change nothing in the cell"
        );
    }

    let cfg = "templates/telegram-connector/config.json";
    let params = read_json(cfg)["params"].clone();
    assert_eq!(
        params["partial_delivery"],
        json!("off"),
        "{cfg}: partial_delivery ships off"
    );
    let with = ProxyParams::parse(&params).expect("shipped proxy params parse");
    let none =
        ProxyParams::parse(&without(&params, "partial_delivery")).expect("proxy params parse");
    assert_eq!(
        format!("{with:?}"),
        format!("{none:?}"),
        "{cfg}: off is the proxy's default"
    );

    let cfg = "templates/voice/config.json";
    let params = read_json(cfg)["params"].clone();
    assert_eq!(
        params["speak_partials"],
        json!("0"),
        "{cfg}: speak_partials ships \"0\""
    );
    let with = VoiceParams::parse(&params).expect("shipped voice params parse");
    let none = VoiceParams::parse(&without(&params, "speak_partials")).expect("voice params parse");
    assert_eq!(
        format!("{with:?}"),
        format!("{none:?}"),
        "{cfg}: \"0\" is the voice default"
    );
}
