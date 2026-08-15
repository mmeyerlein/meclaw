//! 0.2.0 P2 -- the chain derivation reads on the CANONICAL predicate and knows a
//! seeded cardinality (rulings Q3 and Q4, GitHub #22 / #24 / the #13 interim).
//!
//! Two defects are pinned here, both measured before this package:
//!
//! 1. One relation under two spellings was two axes, so the version chain never
//!    fired. The axis key is now the store-owned `canonical_predicate`.
//! 2. Coexistence learning cannot see a multivalued predicate whose second value
//!    arrives STRICTLY LATER -- it had never coexisted in a write, so the second
//!    child superseded the first. A curated `CORE_MULTI` list now makes such an
//!    axis an enumeration from its first value.
//!
//! And the drift lock underneath both: `predicate-core.json` is the authority,
//! and `extract-glue` / `recall` / `dream-glue` each carry a copy because a script
//! cannot import. This file compares what the two chain scripts actually EVALUATE
//! against that file -- the same guarantee `extract_canonicalization.rs` gives for
//! the rendered prompt.

use std::process::Command;

const CORE_LIST: &str = "../../templates/memory-hive/predicate-core.json";

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

/// Predicates of one cardinality group, as the authority file declares them.
fn core_group(kind: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(CORE_LIST).expect("predicate-core.json");
    let list: serde_json::Value = serde_json::from_str(&raw).expect("core list json");
    let mut out: Vec<String> = list["predicates"]
        .as_array()
        .expect("predicates array")
        .iter()
        .filter(|p| p["cardinality"] == kind)
        .map(|p| p["predicate"].as_str().expect("predicate").to_string())
        .collect();
    out.sort();
    out
}

fn scripts() -> [(String, &'static str); 2] {
    [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ]
}

#[test]
fn both_chain_scripts_evaluate_the_core_list_of_predicate_core_json() {
    // The drift lock. `predicate-core.json` is the authority; the literal in each
    // script is its copy, and this compares the VALUES the scripts actually hold,
    // not the text they contain. Whoever edits the list edits it in the JSON file
    // and in every script literal -- otherwise this goes red.
    let probe = "print('|'.join(sorted(CORE_SINGLE)))\nprint('|'.join(sorted(CORE_MULTI)))\n";
    for (script, name) in scripts() {
        let out = run_probe(&script, name, probe);
        let mut lines = out.lines();
        assert_eq!(
            lines.next().unwrap_or_default(),
            core_group("single").join("|"),
            "{name}: the single-valued group drifted from predicate-core.json"
        );
        assert_eq!(
            lines.next().unwrap_or_default(),
            core_group("multi").join("|"),
            "{name}: the multivalued group drifted from predicate-core.json"
        );
    }
}

#[test]
fn the_core_list_block_is_byte_identical_in_both_chain_scripts() {
    // Values alone would still let the two copies drift in what they SAY. The
    // block is lifted byte for byte, anchors included, so the next package can
    // lift it again from any of the three scripts.
    const START: &str =
        "# --------------------------------------------------- PREDICATE CORE LIST (P1)";
    const END: &str =
        "# -------------------------------------------------- /PREDICATE CORE LIST (P1)";
    let block = |script: &str, name: &str| -> String {
        let s = script
            .find(START)
            .unwrap_or_else(|| panic!("{name}: no core-list start anchor"));
        let e = script
            .find(END)
            .unwrap_or_else(|| panic!("{name}: no core-list end anchor"));
        script[s..e + END.len()].to_string()
    };
    let from_extract = block(
        &script_of("../../templates/memory-hive/extract-glue/config.json"),
        "extract-glue",
    );
    assert!(from_extract.contains("has_child"), "the anchors moved");
    for (script, name) in scripts() {
        assert_eq!(
            block(&script, name),
            from_extract,
            "{name} carries a different core-list block than extract-glue"
        );
    }
}

#[test]
fn a_seeded_multi_predicate_coexists_when_the_second_value_arrives_later() {
    // Ruling Q4, the #13 interim and the whole reason the seed exists: the two
    // children were never written at the same instant, so coexistence learning is
    // blind here. Before the seed the second child ENDED the first -- a wrong
    // answer, not a missing one.
    let probe = r#"
rows = [
 {"id":"a","subject":"user:m","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Mika","valid_from":"2002-03-04T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"b","subject":"user:m","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Noa","valid_from":"2007-11-12T00:00:00Z","valid_until":None,
  "recorded_at":"2026-06-01T00:00:00Z"},
]
print([p for p in derive_supersessions(rows) if p[1] or p[2]])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", probe),
        "[]",
        "an enumeration supersedes nothing -- not even with a strictly later value"
    );

    // and the read path agrees: both spans stay open, neither claim is history
    let read = r#"
rows = [
 {"id":"a","subject":"user:m","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Mika","valid_from":"2002-03-04T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"b","subject":"user:m","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Noa","valid_from":"2007-11-12T00:00:00Z","valid_until":None,
  "recorded_at":"2026-06-01T00:00:00Z"},
]
for hit in ("a", "b"):
    c = project_fact_candidate(hit, rows)
    print(c["claim"], c["history"])
chain = build_chains(rows)[("user:m","has_child")]
print(span_end(chain, 0), span_end(chain, 1))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", read),
        "Mika []\nNoa []\nNone None"
    );
}

#[test]
fn a_single_predicate_still_supersedes_its_own_re_assertion() {
    // The protection direction of the epic constraint, and it runs BOTH ways: the
    // multivalue seed must not invent supersessions, and it must not swallow the
    // ones a functional axis has. `favorite_color` is on the curated SINGLE list.
    //
    // W2 (#13, ruling Q1) narrows what "the ones a functional axis has" means, and
    // both arms are here because the difference IS the package: the same colour
    // stated twice is one statement with two assertions and still supersedes; a
    // different colour is a different statement and now waits for a judgement.
    // Being on the single list buys presentation, not a closure.
    let probe = r#"
def rows_for(second):
    return [
     {"id":"a","subject":"user:m","predicate":"favorite_color",
      "canonical_predicate":"favorite_color","claim":"green",
      "valid_from":"2020-01-01T00:00:00Z","valid_until":None,
      "recorded_at":"2020-01-01T00:00:00Z"},
     {"id":"b","subject":"user:m","predicate":"favorite_color",
      "canonical_predicate":"favorite_color","claim":second,
      "valid_from":"2026-01-01T00:00:00Z","valid_until":None,
      "recorded_at":"2026-01-01T00:00:00Z"},
    ]

for second in ("green", "blue"):
    print(second, [p for p in derive_supersessions(rows_for(second)) if p[1] or p[2]])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", probe),
        "green [('a', '2026-01-01T00:00:00Z', 'b')]\nblue []"
    );
}

#[test]
fn an_unlisted_predicate_keeps_the_learned_rule_only() {
    // The residual gap ruling Q4 names, pinned as behaviour rather than left to be
    // discovered later: a multivalued predicate that is NOT on the list and never
    // coexisted was read as a state and superseded. That is the case the follow-up
    // track (statement identity, #13) closes.
    //
    // W1 (ruling Q3) put the learned half behind cross-session evidence; W2
    // (ruling Q1) took the closure away from it entirely. What the rule decides
    // now is the PRESENTATION verdict -- does this axis enumerate -- and the
    // three arms below pin exactly that, next to the constant that made the whole
    // package necessary: a different value never closes another one, in any arm.
    let corpus = |a: &str, b: &str, c: &str| {
        format!(
            r#"
rows = [
 {{"id":"a","subject":"user:m","predicate":"collects","canonical_predicate":"collects",
  "claim":"Vinyl","canonical_claim":"Vinyl","valid_from":"{a}","valid_until":None,
  "recorded_at":"2020-01-01T00:00:00Z","session_id":"s1"}},
 {{"id":"b","subject":"user:m","predicate":"collects","canonical_predicate":"collects",
  "claim":"Cacti","canonical_claim":"Cacti","valid_from":"{b}","valid_until":None,
  "recorded_at":"2020-01-01T00:00:01Z","session_id":"{c}"}},
 {{"id":"z","subject":"user:m","predicate":"collects","canonical_predicate":"collects",
  "claim":"Bonsai","canonical_claim":"Bonsai","valid_from":"2026-01-01T00:00:00Z",
  "valid_until":None,"recorded_at":"2026-01-01T00:00:00Z","session_id":"s9"}},
]
print(axis_is_multivalued(build_chains(rows)[("user:m","collects")]),
      [p for p in derive_supersessions(rows) if p[1] or p[2]])
"#
        )
    };

    // Never coexisted: the axis still reads as a state, and since W2 it closes
    // nothing anyway -- which is the whole point. Before the track, `z` ended
    // both collections.
    assert_eq!(
        run_probe(
            &dream_glue_script(),
            "dream-glue",
            &corpus("2020-01-01T00:00:00Z", "2021-01-01T00:00:00Z", "s2")
        ),
        "False []"
    );

    // Both values at ONE instant from ONE conversation. Before W1 this froze the
    // axis on the spot; now the origin decides, and the origin is one talk.
    assert_eq!(
        run_probe(
            &dream_glue_script(),
            "dream-glue",
            &corpus("2020-01-01T00:00:00Z", "2020-01-01T00:00:00Z", "s1")
        ),
        "False []"
    );

    // And the evidence the rule was always meant to read: two conversations, one
    // instant. The axis enumerates.
    assert_eq!(
        run_probe(
            &dream_glue_script(),
            "dream-glue",
            &corpus("2020-01-01T00:00:00Z", "2020-01-01T00:00:00Z", "s2")
        ),
        "True []"
    );
}

#[test]
fn two_spellings_of_one_relation_form_a_single_axis() {
    // Ruling Q3 in one probe: the axis key is the canonical predicate, so the two
    // rows the extractor minted under different spellings are ONE chain and the
    // version chain can fire across them at all. On the written predicate they
    // were two axes and nothing ever fired.
    //
    // Since W2 (#13, ruling Q1) the value decides WHAT fires, so the fixture
    // states the same thing twice under two predicate spellings -- one statement,
    // two assertions. That is the smallest corpus on which the merged axis is
    // still observable: on two different values the merged axis would show one
    // chain with two open statements, which is a weaker signal.
    let probe = r#"
rows = [
 {"id":"a","subject":"user:m","predicate":"Lieblingseditor",
  "canonical_predicate":"favorite_editor","claim":"Helix","canonical_claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,"recorded_at":"2026-08-08T19:18:39Z"},
 {"id":"b","subject":"user:m","predicate":"favorite editor",
  "canonical_predicate":"favorite_editor","claim":"Helix","canonical_claim":"Helix",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,"recorded_at":"2026-08-08T19:28:03Z"},
]
print(sorted(build_chains(rows).keys()))
c = project_fact_candidate("a", rows)
print(c["id"], c["claim"], c["history"][0]["claim"], c["history"][0]["until"])
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", probe),
        "[('user:m', 'favorite_editor')]\nb Helix Helix 2026-08-08T19:28:00Z"
    );
}

#[test]
fn a_row_from_before_the_migration_keeps_an_axis_of_its_own() {
    // The fallback is not cosmetic: without it every unmigrated row would land in
    // one `None` bucket and answer for relations it has nothing to do with. A
    // missing canonical value must cost recall, never correctness.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"phone","claim":"030-1",
  "valid_from":"2020-01-01T00:00:00Z","valid_until":None,"recorded_at":"2020-01-01T00:00:00Z"},
 {"id":"b","subject":"s","predicate":"editor","claim":"Helix",
  "valid_from":"2021-01-01T00:00:00Z","valid_until":None,"recorded_at":"2021-01-01T00:00:00Z"},
]
print(sorted(build_chains(rows).keys()))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", probe),
        "[('s', 'editor'), ('s', 'phone')]"
    );
}
