//! GH #1089 -- German writes every noun with a capital; a capital is no name.
//!
//! The locks, with invented people only:
//! - a capitalised word run is cut at the words of the language lists
//!   (`entity_stop_lang.<lang>`: forms of address, pronouns, polite forms,
//!   articles, common nouns) before it becomes a subject key (entity-glue
//!   `name_windows`) or a graph anchor (recall `name_anchors`): on the three
//!   review sentences the German runs are at most 1.5 times the English ones,
//!   and no run or anchor carries a word of the lists;
//! - a proper name spelled like a word of the lists ("Oma") is no mention on
//!   its capital alone (entity-glue `_scan`);
//! - one list: recall, entity-glue, the curator push and the fixture
//!   `fixtures/entity_stop_de.jsonl` carry the same words (drift lock).

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::shipped_script;
use std::io::Write;
use support::*;

const ENTITY: &str = "../../templates/memory-hive/entity-glue/config.json";
const PUSH: &str = "../../templates/curator/push/config.json";
const FIXTURE: &str = "tests/fixtures/entity_stop_de.jsonl";

const DE: [&str; 3] = [
    "Am Montag hat Herrn Beckers Tochter Anna den Termin beim Hausarzt Doktor Weber abgesagt.",
    "Die Bank hat wegen der Überweisung an Frau Müller angerufen, Montag Vormittag.",
    "Der Garten Blumen Duft war schön, sagt die Nachbarin Rose.",
];
const EN: [&str; 3] = [
    "On Monday Mr Becker's daughter Anna cancelled the appointment with the GP Doctor Weber.",
    "The bank called about the transfer to Mrs Mueller, Monday morning.",
    "The garden flowers smelled nice, says the neighbour Rose.",
];

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

/// A Python program on stdin (the scripts are larger than one argument).
fn py(src: &str) -> Value {
    let mut child = std::process::Command::new("python3")
        .arg("-I")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(src.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("json out")
}

fn lit(v: &Value) -> String {
    serde_json::to_string(&v.to_string()).expect("literal")
}

/// entity-glue as a library (`_ENTITY_LIB_ONLY`), its params, then `tail`.
fn entity(tail: &str) -> Value {
    py(&format!(
        "import json\nP = json.loads({})\n_ENTITY_LIB_ONLY = True\nexec(compile(json.loads({}), 'entity-glue', 'exec'))\n{tail}\n",
        lit(&json!({"entity_stop_lang": config(ENTITY)["params"]["entity_stop_lang"].clone()})),
        lit(&json!(shipped_script(ENTITY))),
    ))
}

/// recall's name blocks, its params, then `tail`.
fn recall(tail: &str) -> Value {
    let script = shipped_script(RECALL);
    let blocks: Vec<String> = [
        "CAP_RUN = ",
        "ENTITY_STOP = ",
        "def name_runs",
        "def name_anchors",
    ]
    .iter()
    .map(|h| block_of(&script, h))
    .collect();
    py(&format!(
        "import json, re\nP = json.loads({})\n{}\n{tail}\n",
        lit(&json!({"entity_stop_lang": config(RECALL)["params"]["entity_stop_lang"].clone()})),
        blocks.join("\n\n")
    ))
}

fn listed() -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (_, words) in config(RECALL)["params"]["entity_stop_lang"]
        .as_object()
        .expect("recall: params.entity_stop_lang is per language")
    {
        for w in words.as_array().expect("a list") {
            out.insert(w.as_str().expect("a word").to_lowercase());
        }
    }
    out
}

fn words_of(runs: &Value) -> Vec<String> {
    runs.as_array()
        .expect("runs")
        .iter()
        .flat_map(|r| {
            r.as_str()
                .unwrap_or_default()
                .split_whitespace()
                .map(|w| {
                    w.trim_end_matches("'s")
                        .trim_end_matches("\u{2019}s")
                        .to_lowercase()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_german_noun_is_no_part_of_a_name() {
    let lists = listed();
    for (cell, runs_of) in [
        ("entity-glue", entity as fn(&str) -> Value),
        ("recall", recall as fn(&str) -> Value),
    ] {
        let runs = |xs: &[&str]| {
            runs_of(&format!(
                "print(json.dumps([r for x in json.loads({}) for r in name_runs(x)]))",
                lit(&json!(xs))
            ))
        };
        let (de, en) = (runs(&DE), runs(&EN));
        let (nd, ne) = (
            de.as_array().expect("de").len(),
            en.as_array().expect("en").len(),
        );
        assert!(
            nd * 2 <= ne * 3,
            "{cell}: {nd} German runs against {ne}: {de} / {en}"
        );
        for w in words_of(&de).into_iter().chain(words_of(&en)) {
            assert!(
                !lists.contains(&w),
                "{cell}: a run carries `{w}`: {de} {en}"
            );
        }
    }
    // the subject keys entity-glue looks up
    let windows = entity(&format!(
        "print(json.dumps(sorted(set().union(*[name_windows(x) for x in json.loads({})]))))",
        lit(&json!(DE))
    ));
    for w in words_of(&windows) {
        assert!(
            !lists.contains(&w),
            "a subject key carries `{w}`: {windows}"
        );
    }
    // the graph anchors recall reads off claims
    let anchors = recall(&format!(
        "print(json.dumps(sorted(name_anchors('', json.loads({})))))",
        lit(&json!(DE))
    ));
    for w in words_of(&anchors) {
        assert!(!lists.contains(&w), "an anchor carries `{w}`: {anchors}");
    }
}

#[test]
fn a_name_spelled_like_a_noun_is_no_mention_on_its_capital() {
    let found = entity(
        "t = {'oma': ('oma-key', True), 'anna': ('anna-key', True)}\n\
         print(json.dumps([sorted(mentions(x, t)) for x in \
         ['Oma hat angerufen.', 'Anna hat angerufen.', 'die oma hat angerufen.']]))",
    );
    assert_eq!(found, json!([[], ["anna-key"], []]), "{found}");
}

#[test]
fn one_list_of_words_that_are_never_a_name() {
    let (recall, entity) = (
        config(RECALL)["params"]["entity_stop_lang"].clone(),
        config(ENTITY)["params"]["entity_stop_lang"].clone(),
    );
    assert!(recall.is_object(), "recall: params.entity_stop_lang");
    assert_eq!(recall, entity, "recall and entity-glue drifted");
    for (cell, cfg) in [("recall", RECALL), ("entity-glue", ENTITY)] {
        assert_eq!(
            config(cfg)["contract"]["settings"]["entity_stop_lang"]["default"],
            config(cfg)["params"]["entity_stop_lang"],
            "{cell}: the declared default is the param"
        );
    }
    // the curator push asks with the same words (GH #1089, KP)
    let push = config(PUSH)["params"]["entity_stop_lang"].clone();
    assert_eq!(push, recall, "the push and the memory hive drifted");
    // the German list is the fixture, word for word, in its order
    let path = format!("{}/{FIXTURE}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e} -- the fixture of the German list"));
    let words: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("fixture line")["word"].clone())
        .collect();
    assert_eq!(
        recall["de"],
        json!(words),
        "the German list drifted from the fixture"
    );
    // both scripts cut a run by the same bytes
    let (a, b) = (shipped_script(RECALL), shipped_script(ENTITY));
    for head in ["ENTITY_STOP = ", "def name_runs"] {
        assert_eq!(block_of(&a, head), block_of(&b, head), "{head} drifted");
    }
}
