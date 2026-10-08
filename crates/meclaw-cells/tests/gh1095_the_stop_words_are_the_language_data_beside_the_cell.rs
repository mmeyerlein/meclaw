//! GH #1095 (release audit of 0.62.2) — the recall's stop words are language
//! data with one home.
//!
//! The keyword leg of `memory-hive/recall` drops function words from a query
//! before it reaches the full-text index. The words used to be a literal in
//! the shipped script, so the public tree carried a second language inside
//! code and the export audit had to buy it free with a declared block. They
//! now live beside the cell in `recall/lang/<code>.json` (key `stopwords`),
//! copied byte for byte into `params.stopwords_lang` and its declared default.
//! This file pins the three copies against each other and keeps the script
//! free of the list.

use meclaw_core::serde_json::{self, Value};

fn cell_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/memory-hive/recall")
}

fn config() -> Option<Value> {
    let path = cell_dir().join("config.json");
    // GH #49: a tree without the template skips instead of failing.
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).expect("recall config.json parses"))
}

#[test]
fn the_stop_words_are_the_language_data_beside_the_cell() {
    let Some(cfg) = config() else { return };
    let shipped = &cfg["params"]["stopwords_lang"];
    let declared = &cfg["contract"]["settings"]["stopwords_lang"]["default"];
    assert_eq!(
        shipped, declared,
        "params.stopwords_lang and its declared default are two copies of one value"
    );
    let mut codes = Vec::new();
    for entry in std::fs::read_dir(cell_dir().join("lang")).expect("recall/lang") {
        let path = entry.expect("entry").path();
        let code = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("code")
            .to_string();
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("lang file"))
            .expect("lang json");
        assert_eq!(
            shipped[&code], data,
            "params.stopwords_lang.{code} drifted from lang/{code}.json -- copy the file \
             into the param and its declared default"
        );
        let words = data["stopwords"].as_str().expect("stopwords is a string");
        assert!(
            words.split_whitespace().count() >= 20,
            "lang/{code}.json carries a real list"
        );
        codes.push(code);
    }
    codes.sort();
    assert_eq!(codes, ["de", "en"], "both shipped languages have a file");
    assert_eq!(
        shipped.as_object().map(|o| o.len()),
        Some(codes.len()),
        "the param names no language without a file"
    );
}

#[test]
fn the_script_carries_no_stop_word_list() {
    let Some(cfg) = config() else { return };
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        script.contains("STOP = _stop_words(P.get(\"stopwords_lang\"))"),
        "STOP is built from the param"
    );
    for code in ["en", "de"] {
        let words = cfg["params"]["stopwords_lang"][code]["stopwords"]
            .as_str()
            .expect("stopwords");
        let head: Vec<&str> = words.split_whitespace().take(6).collect();
        assert!(
            !script.contains(&head.join(" ")),
            "the {code} list is data, not script text"
        );
    }
}
