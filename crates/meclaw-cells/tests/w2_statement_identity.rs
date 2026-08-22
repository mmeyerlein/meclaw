//! Statement identity W2 -- the VALUE becomes part of the identity (GitHub #13,
//! ruling Q1).
//!
//! Until W2 a fact's identity was its axis `(canonical_subject,
//! canonical_predicate)`, and everything that arrived later on that axis
//! superseded everything that arrived earlier. W1 measured what that costs once
//! the session guard thaws the axes the degenerate coexistence signal had frozen:
//! on the eight P8a stores a dream run would have materialised 928 closures
//! instead of 49, 636 of them on foresight facts, because a BUCKET axis
//! (`planned_activity` with 71 values, `plans`, `interested_in`) counted as
//! functional as a whole.
//!
//! Ruling Q1 moves the supersession unit down to the statement
//! `(canonical_subject, canonical_predicate, canonical_claim)` and leaves the
//! axis as the retrieval grouping. `canonical_claim` is derived by the same
//! generic binding the other two dimensions use, so the declaration is one row in
//! `params.canonical` and there is no new Rust anywhere.
//!
//! This file pins the DECLARATION and the identity it derives. The chain rule
//! that rests on it is `w2_statement_chain.rs`.

use std::io::Write;
use std::process::{Command, Stdio};

fn store_config() -> serde_json::Value {
    let raw = std::fs::read_to_string("../../templates/memory-hive/store/config.json")
        .expect("store config");
    serde_json::from_str(&raw).unwrap()
}

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

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
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
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "{name} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_store_binds_the_claim_as_a_third_canonical_dimension() {
    // Ruling Q1 in one declaration line. The binding is generic since P4 (a list
    // of specs per table), so the claim dimension costs a row and no code: the
    // insert path derives the target on every write, `canonicalize` re-derives
    // it, and the alias plus rejected tables come from the same DDL the other two
    // dimensions use.
    let cfg = store_config();
    let bindings = cfg["params"]["canonical"]["facts"]
        .as_array()
        .expect("facts carries a LIST of canonical bindings");
    let claim = bindings
        .iter()
        .find(|b| b["source"] == "claim")
        .expect("no canonical binding on the claim dimension");
    assert_eq!(claim["target"], "canonical_claim");
    assert_eq!(claim["aliases"], "claim_aliases");
    assert_eq!(claim["rejected"], "claim_rejected_pairs");
    assert!(
        claim.get("normalize").is_none(),
        "byte identity is the day-one canonical value (ruling Q1): normalisation \
         beyond it is the judged claim aliases, which are their own package"
    );
    let schema = &cfg["params"]["schema"]["facts"];
    assert_eq!(
        schema["canonical_claim"], "text",
        "the derived column has to stand in params.schema -- the binding is \
         closed-set validated against it"
    );
}

#[test]
fn a_closure_names_its_author_or_it_is_arithmetic() {
    // Ruling Q2 guard rail 2, made a column: "every closure carries its reason".
    // It is what lets a re-derive tell an EXPLICIT closure (judge, extractor)
    // from one the old axis arithmetic left behind -- the two are otherwise the
    // same two columns, and ruling Q4 asks the re-derive to withdraw the second
    // kind. W2 writes nothing here; it is the slot W3 fills.
    let cfg = store_config();
    assert_eq!(
        cfg["params"]["schema"]["facts"]["closure_source"], "text",
        "facts carries no closure attribution column"
    );
}

#[test]
fn both_chain_scripts_read_the_claim_identity_the_same_way() {
    // The third dimension needs the same fallback discipline as the other two:
    // the written claim stands in for a row from before the migration, so such a
    // row keeps a statement of its OWN instead of collapsing into a single None
    // bucket with every other unmigrated row.
    let probe = r#"
print(statement_key({"claim":"a","canonical_claim":"A"}),
      statement_key({"claim":"a"}),
      statement_key({"claim":"a","canonical_claim":""}),
      statement_key({}))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, probe),
            "A a a None",
            "{name}: the claim identity does not fall back onto the written claim"
        );
    }
}

#[test]
fn every_axis_page_select_carries_the_claim_identity() {
    // The statement rule can only read what the select fetched, and a missing
    // column would not fail loudly -- every claim would fall back onto its
    // written spelling, which is byte identity again and therefore invisible
    // until the judged aliases land. The three selects whose rows reach
    // `build_chains` are the AXIS PAGES, each bounded by AXIS_LIMIT.
    for (script, name, anchors) in [
        (
            recall_script(),
            "recall",
            vec![
                "AXIS_LIMIT}, \"t1-hyd-axis\"",
                "AXIS_LIMIT}, \"t1-temporal\"",
            ],
        ),
        (
            dream_glue_script(),
            "dream-glue",
            vec!["AXIS_LIMIT}, \"sup-axes\""],
        ),
    ] {
        for anchor in anchors {
            let start = script
                .find(anchor)
                .unwrap_or_else(|| panic!("{name}: no axis page select at `{anchor}`"));
            let head = &script[..start];
            let select = &head[head
                .rfind("\"operation\": \"select\"")
                .unwrap_or_else(|| panic!("{name}: `{anchor}` is not fed by a select"))..];
            assert!(
                select.contains("\"canonical_claim\""),
                "{name}: the axis page at `{anchor}` does not fetch canonical_claim"
            );
        }
    }
}

#[test]
fn the_supersession_page_carries_the_closure_attribution() {
    // Only the dream page needs it: the read path reads a closure, the WRITE
    // path has to decide whether it may withdraw one (ruling Q4). A page without
    // the column would look like an unattributed store and clear every judged
    // closure on the next round -- the loudest possible version of the revert
    // path firing when it should not.
    let script = dream_glue_script();
    let start = script
        .find("AXIS_LIMIT}, \"sup-axes\"")
        .expect("sup-axes page");
    let head = &script[..start];
    let select = &head[head.rfind("\"operation\": \"select\"").expect("select")..];
    assert!(
        select.contains("\"closure_source\""),
        "the supersession page does not fetch closure_source"
    );
}
