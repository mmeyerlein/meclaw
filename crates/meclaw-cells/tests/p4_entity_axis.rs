//! 0.2.0 P4 -- the chain derivation reads on the CANONICAL SUBJECT as well
//! (ruling Q5, GitHub #23; the entity half of the axis P2 built for relations).
//!
//! The defect this pins is the one P1's receipt named and C1 already carries as
//! an assertion: the extractor mints `user` in one turn and `user:alex` in the
//! next, the relation is identical, and the axis falls apart anyway -- so the
//! version chain never fires and a knowledge update supersedes nothing.
//!
//! Two things have to be true for the union to reach a reader, and each fails on
//! its own:
//!   1. the chain groups on the canonical subject (this file's probes), and
//!   2. the axis SELECT filters on that column, or the store never hands the
//!      other spelling's rows over in the first place (the text locks below).

use std::process::Command;

fn script_of(config: &str) -> String {
    let raw = std::fs::read_to_string(config).expect("config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

fn recall_script() -> String {
    script_of("../../templates/memory-hive/recall/config.json")
}

fn dream_glue_script() -> String {
    script_of("../../templates/memory-hive/dream-glue/config.json")
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Run a probe against the module body of a real script. The `park()` exit at its
/// end is swallowed so the probe can call the helpers the script defines.
fn run_probe(script: &str, name: &str, probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, '{}', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(script).unwrap(),
        name,
        probe
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(src)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "{name} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Two rows of one relation under two spellings of one subject. The store has
/// already derived the identity (this is what a mint after `set_alias` writes),
/// so what is measured here is purely the chain's grouping key.
///
/// One person under two spellings, one relation, and
/// since W2 (#13, ruling Q1) one STATEMENT asserted twice -- the value has to be
/// held constant, or the entity merge under test would be invisible behind the
/// statement rule that now decides what supersedes what.
const DRIFTED_SUBJECT_ROWS: &str = r#"
rows = [
 {"id":"a","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"helix","canonical_claim":"helix",
  "valid_from":"2026-08-01T09:00:00Z","valid_until":None,"recorded_at":"2026-08-01T09:00:03Z"},
 {"id":"b","subject":"user:alex","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"helix","canonical_claim":"helix",
  "valid_from":"2026-08-09T18:30:00Z","valid_until":None,"recorded_at":"2026-08-09T18:30:04Z"},
]
"#;

#[test]
fn two_spellings_of_one_subject_form_a_single_axis() {
    // Ruling Q5 in one probe: the axis key is the canonical subject, so the two
    // rows are ONE chain and the version chain can fire across them at all. On
    // the written subject they were two axes and nothing ever fired -- with the
    // answer carrying both the old and the new value side by side.
    let probe = format!(
        "{DRIFTED_SUBJECT_ROWS}\
         print(sorted(build_chains(rows).keys()))\n\
         c = project_fact_candidate(\"a\", rows)\n\
         print(c[\"id\"], c[\"claim\"], c[\"history\"][0][\"claim\"], \
         c[\"history\"][0][\"until\"])\n"
    );
    assert_eq!(
        run_probe(&recall_script(), "recall", &probe),
        "[('user', 'favorite_editor')]\nb helix helix 2026-08-09T18:30:00Z"
    );
}

#[test]
fn the_supersession_arithmetic_sees_the_same_single_axis() {
    // The write side of the same union: the dream lane derives the supersession
    // from the chain, so a split axis does not merely hide history in a bundle --
    // it never materialises the closed span either.
    let probe = format!(
        "{DRIFTED_SUBJECT_ROWS}\
         print([p for p in derive_supersessions(rows) if p[1] or p[2]])\n"
    );
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "[('a', '2026-08-09T18:30:00Z', 'b')]"
    );
}

#[test]
fn a_row_without_a_derived_subject_keeps_an_axis_of_its_own() {
    // Same fallback discipline as the predicate dimension (P2): a row from before
    // the migration, or a store with no entity binding at all, keeps its written
    // subject as the key instead of collapsing every such row into one None
    // bucket. A missing canonical value may cost recall, never correctness.
    let probe = r#"
rows = [
 {"id":"a","subject":"user","predicate":"phone","canonical_predicate":"phone","claim":"030-1",
  "valid_from":"2020-01-01T00:00:00Z","valid_until":None,"recorded_at":"2020-01-01T00:00:00Z"},
 {"id":"b","subject":"user:alex","predicate":"phone","canonical_predicate":"phone",
  "claim":"030-2","valid_from":"2021-01-01T00:00:00Z","valid_until":None,
  "recorded_at":"2021-01-01T00:00:00Z"},
]
print(sorted(build_chains(rows).keys()))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", probe),
        "[('user', 'phone'), ('user:alex', 'phone')]"
    );
}

#[test]
fn both_chain_scripts_key_the_entity_dimension_the_same_way() {
    // One ordering rule for the whole lane: read path and write path must agree
    // on what an axis IS, or the materialised cache disagrees with the derivation
    // that produced it.
    let probe = r#"
print(subject_key({"subject": "user:alex", "canonical_subject": "user"}))
print(subject_key({"subject": "user:alex"}))
print(subject_key({}))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, probe),
            "user\nuser:alex\nNone",
            "{name}: the entity key drifted"
        );
    }
}

#[test]
fn the_axis_selects_filter_on_the_canonical_subject() {
    // The grouping key alone unifies nothing: the axis round trip asks the store
    // for "every row of these subjects", and with the WRITTEN column in that
    // filter the other spelling's rows never arrive. Text lock rather than a
    // probe, because the failure is in a payload the script only ever emits.
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert!(
            script.contains("\"canonical_subject\": {\"in\": subjects}"),
            "{name}: the axis select does not filter on the canonical subject"
        );
        assert!(
            !script.contains("\"subject\": {\"in\": subjects}"),
            "{name}: an axis select still filters on the written subject"
        );
        assert!(
            script.contains("subjects = sorted({subject_key(r) for r in rows if subject_key(r)})"),
            "{name}: the subject list is not built from the canonical identity"
        );
    }
}
