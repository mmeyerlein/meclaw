//! d58-3 wort (GH #1523) -- the push holds no word list.
//!
//! WHY this file exists: the owner's rule "no word lists: meaning by the
//! model, code only for structure". Until d58-3 `curator/push` shipped one
//! list per language of the words that are never a name
//! (`entity_stop_lang`), of the words a noun follows (`entity_det_lang`), of
//! noun endings (`entity_suffix_lang`) and of weekdays and months
//! (`entity_date_lang`), with `entity_language` picking among them. The names
//! of `mentioned:` come from the model's section `things` now, so the lists,
//! the knob that picks among them and the code that read them leave the push:
//! not in `params`, not in `contract.settings`, not in the script. The memory
//! hive's own `entity_stop_lang` is another hive's and not pinned here.
//!
//! Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;

/// The params of the word lists and the one that picks their languages.
const WORD_LIST_PARAMS: [&str; 5] = [
    "entity_stop_lang",
    "entity_det_lang",
    "entity_suffix_lang",
    "entity_date_lang",
    "entity_language",
];

#[test]
fn no_list_in_params_settings_or_script() {
    let path = repo("templates/curator/push/config.json");
    if !path.is_file() {
        return;
    }
    let cfg = read_json(&path);
    let params = cfg["params"].as_object().cloned().unwrap_or_default();
    let settings = cfg["contract"]["settings"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let script = cfg["params"]["script_inline"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(!script.is_empty(), "the push ships its script inline");
    for key in WORD_LIST_PARAMS {
        assert!(
            !params.contains_key(key),
            "curator/push params carry no word list: `{key}` is still shipped"
        );
        assert!(
            !settings.contains_key(key),
            "curator/push contract.settings name no word list: `{key}` is still declared"
        );
        assert!(
            !script.contains(key),
            "the push script reads no word list: it still names `{key}`"
        );
    }
    // The sets the lists were read into, and the guess by a word's form.
    for name in [
        "ENTITY_STOP",
        "ENTITY_DET",
        "ENTITY_DATE",
        "ENTITY_SUFFIX",
        "a_noun_by_form",
        "NOT_NAMES",
    ] {
        assert!(
            !script.contains(name),
            "the push script guesses no name from a list or a word's form: it still holds `{name}`"
        );
    }
}
