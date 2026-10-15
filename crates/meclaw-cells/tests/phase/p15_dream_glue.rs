//! P15 task 7 -- the dream lane materialises the chain instead of judging it (R9).
//!
//! Two guarantees are pinned here. First the DRIFT LOCK: `recall` and `dream-glue`
//! are separate `code` cells and a script cannot import, so the chain helpers exist
//! twice. Byte-equality of the duplicated block is the only thing that keeps the
//! materialised `expired_at` identical to the derived one -- without it the two
//! copies drift apart silently and the invariance criterion rots from below.
//!
//! Second the DERIVATION itself: on a functional axis every fact gets its span end
//! and its successor, on a multivalued one nothing at all (O-3).
//!
//! Both run the REAL `params.script_inline`, never a copy (P5 pattern).

use std::io::Write;
use std::process::{Command, Stdio};

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

/// The shared chain block, cut out by the two `def` lines that bracket it in each
/// script. Anchors instead of line numbers: a marker comment would have to be added
/// to `recall` for the sake of a test, and anchors that are real code cannot rot
/// unnoticed -- if either boundary moves, the extraction fails loudly.
fn chain_block(script: &str, end_anchor: &str) -> String {
    let start = script
        .find("def build_chains(rows):")
        .unwrap_or_else(|| panic!("no build_chains in script"));
    let end = script[start..]
        .find(end_anchor)
        .unwrap_or_else(|| panic!("no {end_anchor} after build_chains"));
    script[start..start + end].to_string()
}

#[test]
fn chain_helpers_are_byte_identical_in_both_scripts() {
    let from_recall = chain_block(&recall_script(), "def project_fact_candidate(");
    let from_dream = chain_block(&dream_glue_script(), "def derive_supersessions(");
    // The helper list is the W2 one (statement identity, #13 ruling Q1): the
    // two-argument `effective_until` and the value-blind `next_later` are gone
    // with the axis arithmetic they implemented, and the three helpers that
    // replaced them decide the same question one level down. `axis_is_multivalued`
    // took a second parameter in W5 (ruling Q3): the judged cardinality the
    // caller looked up, which only the read path has and passes.
    assert!(
        from_recall.contains("def span_end(chain, i):")
            && from_recall.contains("def axis_is_multivalued(chain, judged=None):")
            && from_recall.contains("def statement_key(fact):")
            && from_recall.contains("def next_reassertion(chain, i):")
            && from_recall.contains("def span_successor(chain, i):")
            && from_recall.contains("def closure_is_explicit(fact):")
            && from_recall.contains("def chain_target(chain, i):"),
        "the extracted block lost a helper -- the anchors moved"
    );
    assert_eq!(
        from_recall, from_dream,
        "recall and dream-glue disagree on the chain helpers: what the read path \
         derives and what the dream run writes would drift apart"
    );
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

fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'dream-glue', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&dream_glue_script()).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_re_asserted_statement_yields_span_end_and_successor() {
    // The dream run writes exactly what a read derives. Since W2 (#13, ruling
    // Q1) the arithmetic that produces such a pair is the RE-ASSERTION of one
    // statement: said again months later, the older assertion becomes history
    // and the newer one carries the value. Two DIFFERENT values of one axis are
    // two statements and close nothing (`w2_statement_chain.rs`).
    let probe = r#"
rows = [
 {"id":"a","subject":"user:alex","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,"recorded_at":"2026-08-08T19:18:39Z"},
 {"id":"b","subject":"user:alex","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,"recorded_at":"2026-08-08T19:28:03Z"},
]
for p in derive_supersessions(rows):
    print(p)
"#;
    assert_eq!(
        run_probe(probe),
        "('a', '2026-08-08T19:28:00Z', 'b')\n('b', None, None)"
    );
}

#[test]
fn a_multivalued_axis_yields_no_closure_at_all() {
    // O-3: an enumeration supersedes nothing, so the dream run writes not one
    // closure for it. Since W2 the triples are EMITTED and empty rather than
    // absent, because emitting them is the channel through which a closure the
    // old axis arithmetic left behind is withdrawn (ruling Q4). The caller
    // writes differences only, so an empty triple against an empty row is
    // still no op at all.
    //
    // The two sessions on the coexisting pair are the W1 session guard (#13,
    // ruling Q3): since then the same instant is evidence only when it was
    // stated in two DIFFERENT conversations. The behaviour under test is O-3's
    // and unchanged; what the fixture had to gain is the origin it always
    // implied.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"hat Sohn","claim":"Mika","session_id":"s1",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"b","subject":"s","predicate":"hat Sohn","claim":"Noa","session_id":"s2",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:02Z"},
 {"id":"c","subject":"s","predicate":"hat Sohn","claim":"Nova",
  "valid_from":"2026-06-01T00:00:00Z","valid_until":None,"recorded_at":"2026-06-01T00:00:00Z"},
]
print([p for p in derive_supersessions(rows) if p[1] or p[2]])
"#;
    assert_eq!(run_probe(probe), "[]");
}

#[test]
fn a_mixed_row_set_derives_per_axis_and_in_a_stable_order() {
    // The window select hands over several axes at once. Each is judged on its
    // OWN chain, and the order is total so two runs emit the same op sequence.
    // The two sessions on the child axis are the W1 session guard, same as
    // above: that axis has to stay an enumeration for the case to test what it
    // says it tests.
    //
    // The editor axis carries a RE-ASSERTION since W2 -- two different values
    // would be two statements and would print an empty triple like every other
    // row, which would make the per-axis claim of this case unobservable.
    //
    // Since GH #65 the probe prints EVERY triple instead of only the non-empty
    // ones. It has to: the `lives in` row used to appear because its
    // `valid_until` was mirrored into `expired_at`, and with the mirror gone a
    // filtered print would show a single axis and prove neither half of the
    // claim. The unfiltered form is the stronger pin anyway -- the grouping and
    // the total order are visible in the output rather than inferred from two
    // surviving rows.
    let probe = r#"
rows = [
 {"id":"m2","subject":"s","predicate":"hat Sohn","claim":"Noa","session_id":"s2",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:02Z"},
 {"id":"f2","subject":"s","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,"recorded_at":"2026-08-08T19:28:03Z"},
 {"id":"m1","subject":"s","predicate":"hat Sohn","claim":"Mika","session_id":"s1",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"f1","subject":"s","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,"recorded_at":"2026-08-08T19:18:39Z"},
 {"id":"w1","subject":"s","predicate":"lives in","claim":"Berlin",
  "valid_from":"2020-01-01T00:00:00Z","valid_until":"2024-01-01T00:00:00Z",
  "recorded_at":"2020-01-01T00:00:00Z"},
]
for p in derive_supersessions(rows):
    print(p)
one = derive_supersessions(rows)
two = derive_supersessions(list(reversed(rows)))
print(one == two)
"#;
    assert_eq!(
        run_probe(probe),
        "('f1', '2026-08-08T19:28:00Z', 'f2')\n\
         ('f2', None, None)\n\
         ('m1', None, None)\n\
         ('m2', None, None)\n\
         ('w1', None, None)\n\
         True"
    );
}

#[test]
fn a_declared_end_of_validity_wins_on_the_read_path_and_writes_nothing() {
    // A fact that says when it ended ends then, and on the READ path it keeps
    // the top of the precedence (#13, rulings Q1/Q2): neither a judgement nor a
    // re-assertion overrules a statement about itself. `span_end` therefore
    // still reports the declared end, which is the half of this case that never
    // moved.
    //
    // The WRITE path no longer copies it (GH #65). `valid_until` used to be the
    // first source of `derive_supersessions`, mirrored straight into
    // `expired_at` -- and the tier-0 foresight leg filters `expired_at is_null`,
    // so a plan whose deadline lay in the FUTURE left the foresight bundle on
    // the first night after it was written. This case was the write-side pin of
    // that mirror and is therefore the one that had to invert: the assertion is
    // no longer weaker for it, because it now pins BOTH paths at once and that
    // is exactly the property the issue asks for. Which of two deadlines an
    // answer still contains is decided at READ time against the recall instant
    // (`valid_until or_null gt <as_of>` in the leg), never by a stored copy.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"p","claim":"old",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":"2026-01-15T00:00:00Z",
  "recorded_at":"2026-01-01T00:00:00Z"},
 {"id":"b","subject":"s","predicate":"p","claim":"new",
  "valid_from":"2026-02-01T00:00:00Z","valid_until":None,"recorded_at":"2026-02-01T00:00:00Z"},
]
for p in derive_supersessions(rows):
    print(p)
print(span_end(build_chains(rows)[('s','p')], 0))
"#;
    assert_eq!(
        run_probe(probe),
        "('a', None, None)\n('b', None, None)\n2026-01-15T00:00:00Z"
    );
}
