//! GH #856 -- the three judge templates default to a model the provider lists.
//!
//! `argus/judge` and `steward/judge` defaulted to `anthropic/claude-opus-4`, which
//! the hosted provider no longer lists, so a judge left on its default ran into
//! nothing; `memory-hive/judge` defaulted to `anthropic/claude-opus-5`. All three
//! carry the default twice -- once in the `${VAR:-default}` substitution of
//! `params.model` and once in `contract.settings` -- and the two copies are what
//! drifted apart from the provider's catalogue without anybody noticing. This is
//! the drift lock: both copies, all three judges, one value.

const JUDGES: [(&str, &str); 3] = [
    (
        "../../templates/argus/judge/config.json",
        "ARGUS_JUDGE_MODEL",
    ),
    (
        "../../templates/steward/judge/config.json",
        "STEWARD_JUDGE_MODEL",
    ),
    (
        "../../templates/memory-hive/judge/config.json",
        "MODEL_JUDGE",
    ),
];

const LISTED: &str = "anthropic/claude-opus-5.5";

fn config_of(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The `settings` entry of the one knob that names the model, wherever the
/// contract keeps it (a map keyed by name or a list of `{name, default}`).
fn settings_default(cfg: &serde_json::Value, var: &str) -> Option<String> {
    let settings = &cfg["contract"]["settings"];
    if let Some(map) = settings.as_object() {
        for (key, entry) in map {
            let named = key == var
                || entry["description"]
                    .as_str()
                    .is_some_and(|d| d.starts_with(var));
            if named {
                return entry["default"].as_str().map(str::to_string);
            }
        }
    }
    if let Some(list) = settings.as_array() {
        for entry in list {
            let named = entry["name"].as_str() == Some(var)
                || entry["description"]
                    .as_str()
                    .is_some_and(|d| d.starts_with(var));
            if named {
                return entry["default"].as_str().map(str::to_string);
            }
        }
    }
    None
}

#[test]
fn every_judge_defaults_to_the_listed_model_in_both_places() {
    for (path, var) in JUDGES {
        let cfg = config_of(path);
        let model = cfg["params"]["model"].as_str().expect("params.model");
        assert_eq!(
            model,
            format!("${{{var}:-{LISTED}}}"),
            "{path}: the substitution default of params.model is not {LISTED}"
        );
        assert_eq!(
            settings_default(&cfg, var).as_deref(),
            Some(LISTED),
            "{path}: contract.settings does not default {var} to {LISTED}"
        );
    }
}

#[test]
fn no_shipped_judge_names_a_model_the_provider_dropped() {
    for (path, _) in JUDGES {
        let raw = std::fs::read_to_string(path).expect("config");
        for dropped in [
            "anthropic/claude-opus-4\"",
            "anthropic/claude-opus-4}",
            "anthropic/claude-opus-5\"",
            "anthropic/claude-opus-5}",
        ] {
            assert!(!raw.contains(dropped), "{path} still names {dropped}");
        }
    }
}
