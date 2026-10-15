//! Statement identity W2 -- supersession moves from the AXIS onto the STATEMENT
//! (GitHub #13, rulings Q1/Q2/Q4).
//!
//! The old rule: everything that started later on an axis ended everything that
//! started earlier, unless the axis was classified as an enumeration. W1 measured
//! the price of that rule once its coexistence guard stopped firing on noise --
//! on the eight P8a stores a dream run would have materialised 928 closures
//! instead of 49, 636 of them on foresight facts, because a bucket axis like
//! `planned_activity` (71 values) counted as one functional relation.
//!
//! The new rule, in two sentences. Ordering arithmetic closes ONLY a
//! re-assertion of the same statement, which is the half that was always safe.
//! Across statement boundaries nothing closes but an EXPLICIT closure someone
//! wrote down, and `span_end` reads it instead of computing one.
//!
//! Both scripts are probed wherever the block is shared: the chain helpers are
//! byte-identical by drift lock, and a rule that fired in only one of them would
//! make the materialised `expired_at` disagree with the derived one.

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

/// One fact row, in the shape the axis page selects hand to `build_chains`.
fn fact(id: &str, claim: &str, from: &str, session: &str) -> String {
    format!(
        r#" {{"id":"{id}","subject":"user:u1","canonical_subject":"user:u1",
  "predicate":"practices","canonical_predicate":"practices",
  "claim":"{claim}","canonical_claim":"{claim}","fact_kind":"world",
  "valid_from":"{from}","valid_until":None,"recorded_at":"{from}",
  "expired_at":None,"superseded_by":None,"closure_source":None,
  "episode_id":"e-{id}","session_id":"{session}"}}"#
    )
}

fn rows(items: &[String]) -> String {
    format!("rows = [\n{}\n]\n", items.join(",\n"))
}

#[test]
fn two_different_statements_on_one_axis_close_nothing_by_themselves() {
    // The core of ruling Q1. Two values, two instants, two conversations, one
    // axis, and the store has said nothing about their relation -- so neither
    // ends. Before W2 the later one closed the earlier one purely because it was
    // later, which is the arithmetic the bucket axes drowned in.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
chain = build_chains(rows)[('user:u1','practices')]
print(span_end(chain, 0), span_end(chain, 1))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "None None",
            "{name}: axis arithmetic still ends a statement nobody closed"
        );
    }
}

#[test]
fn a_re_assertion_of_the_same_statement_still_supersedes_by_order() {
    // The half of the old arithmetic that was always safe and stays: the SAME
    // statement asserted again later is one statement with two assertions, so the
    // older assertion becomes history and the newer one is the live version.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "yoga twice a week", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
chain = build_chains(rows)[('user:u1','practices')]
print(span_end(chain, 0), span_successor(chain, 0), span_end(chain, 1))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "2023-11-30T00:00:00Z 1 None",
            "{name}: a re-assertion of one statement no longer closes its predecessor"
        );
    }
}

#[test]
fn the_claim_identity_decides_what_counts_as_the_same_statement() {
    // The discriminating pair: the rows differ ONLY in their canonical claim. The
    // day-one canonical value is byte identity, so two wordings are two
    // statements today and the judged claim aliases are what will merge them --
    // in the same alias table, with the same revert path, one package later.
    let probe = r#"
def row(i, claim, canon, start):
    return {"id": i, "subject": "user:u1", "canonical_subject": "user:u1",
            "predicate": "practices", "canonical_predicate": "practices",
            "claim": claim, "canonical_claim": canon, "valid_from": start,
            "valid_until": None, "recorded_at": start, "expired_at": None,
            "superseded_by": None, "closure_source": None, "session_id": "s1"}

merged = [row("a", "yoga twice a week", "practices yoga", "2023-08-11T00:00:00Z"),
          row("b", "the user practices yoga", "practices yoga", "2023-11-30T00:00:00Z")]
split = [row("a", "yoga twice a week", "yoga twice a week", "2023-08-11T00:00:00Z"),
         row("b", "the user practices yoga", "the user practices yoga",
             "2023-11-30T00:00:00Z")]
for name, rs in (("merged", merged), ("split", split)):
    chain = build_chains(rs)[('user:u1','practices')]
    print(name, span_end(chain, 0))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, probe),
            "merged 2023-11-30T00:00:00Z\nsplit None",
            "{name}: the claim identity is not what separates a re-assertion from a change"
        );
    }
}

#[test]
fn an_explicit_closure_is_read_never_recomputed() {
    // Ruling Q2: across statement boundaries a span ends when an invalidation
    // says so. `span_end` therefore READS what a writer materialised instead of
    // deriving a second, possibly different answer next to it -- the read path
    // and the stored column cannot disagree if only one of them decides.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["expired_at"] = "2023-11-30T00:00:00Z"
rows[0]["superseded_by"] = "b"
rows[0]["closure_source"] = "judge"
chain = build_chains(rows)[('user:u1','practices')]
print(span_end(chain, 0), span_successor(chain, 0), span_end(chain, 1))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "2023-11-30T00:00:00Z 1 None",
            "{name}: the materialised closure is not what the read path reports"
        );
    }
}

#[test]
fn an_explicit_valid_until_still_wins_over_everything() {
    // Unchanged and deliberately so: a fact that says when it stops being true
    // says it about itself, and no closure and no re-assertion overrules that.
    // This is a statement about the READ path only -- since GH #65 the write
    // path stores nothing for it (see below), and the two agree precisely
    // because `span_end` reads `valid_until` first and always did.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "yoga twice a week", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["valid_until"] = "2023-09-01T00:00:00Z"
chain = build_chains(rows)[('user:u1','practices')]
print(span_end(chain, 0), span_successor(chain, 0))
"#;
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "2023-09-01T00:00:00Z None",
            "{name}: an explicitly ended fact was ended by something else"
        );
    }
}

#[test]
fn a_declared_end_of_validity_is_not_a_closure() {
    // GH #65. `valid_until` used to be the FIRST source of
    // `derive_supersessions`, mirrored straight into `expired_at`. But the two
    // columns answer two different questions, and the tier-0 foresight leg asks
    // both of them: `expired_at is_null` for "nobody closed this" and
    // `valid_until or_null gt <recall instant>` for "the deadline has not passed
    // yet". The mirror answered the first with the second one's data, so a plan
    // whose deadline lies in the FUTURE dropped out of the foresight bundle on
    // the first night after it was written -- a foresight leg losing exactly the
    // facts it exists for.
    //
    // The two rows are T5's corpus: one deadline ahead of the recall instant,
    // one behind it. NEITHER is a closure, and the difference between them is a
    // read-time comparison rather than a stored column. `span_end` is untouched
    // by the removal and reports the declared end for both, which is why the
    // read path answers the same before and after this change.
    let probe = rows(&[
        fact(
            "a",
            "will run the Berlin marathon",
            "2026-01-10T00:00:00Z",
            "s1",
        ),
        fact("b", "will renew the passport", "2026-01-10T00:00:00Z", "s1"),
    ]) + r#"
rows[0]["valid_until"] = "2030-01-01T00:00:00Z"
rows[1]["valid_until"] = "2026-02-01T00:00:00Z"
print(derive_supersessions(rows))
chain = build_chains(rows)[('user:u1','practices')]
print(span_end(chain, 0), span_end(chain, 1))
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "[('a', None, None), ('b', None, None)]\n\
         2030-01-01T00:00:00Z 2026-02-01T00:00:00Z",
        "a stated end date is materialised as an expiry again"
    );
}

#[test]
fn a_deadline_does_not_out_compute_a_judgement() {
    // The second thing the mirror did, and the reason its removal is a repair
    // rather than a trade (GH #65): it stood ABOVE the attributed closure in the
    // precedence, so a fact carrying both a `valid_until` and a judgement had
    // its `superseded_by` cleared and its `expired_at` overwritten every single
    // night. Ruling Q2 forbids exactly that -- a judgement is reverted by
    // withdrawing it, never by out-computing it.
    let probe = rows(&[
        fact(
            "a",
            "will run the Berlin marathon",
            "2026-01-10T00:00:00Z",
            "s1",
        ),
        fact("b", "gave up on the marathon", "2026-06-05T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["valid_until"] = "2030-01-01T00:00:00Z"
rows[0]["expired_at"] = "2026-06-05T00:00:00Z"
rows[0]["superseded_by"] = "b"
rows[0]["closure_source"] = "judge"
print(derive_supersessions(rows)[0])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "('a', '2026-06-05T00:00:00Z', 'b')",
        "the stated end date overwrote a judged closure again"
    );
}

#[test]
fn the_bucket_axis_of_the_w1_finding_materialises_nothing() {
    // The W1 section 4 counter-proof, as a unit. A bucket axis carrying many
    // different plans from many conversations -- the exact shape that projected
    // 636 foresight closures across the eight P8a stores -- now yields not one
    // closure. The tier-0 foresight leg filters `expired_at is_null`, so every
    // one of those closures was an answer the store would have stopped giving.
    let items: Vec<String> = [
        ("p1", "trip to the coast", "2023-01-05T00:00:00Z", "s1"),
        ("p2", "learn sourdough", "2023-02-05T00:00:00Z", "s2"),
        ("p3", "run a half marathon", "2023-03-05T00:00:00Z", "s3"),
        (
            "p4",
            "visit the science museum",
            "2023-04-05T00:00:00Z",
            "s4",
        ),
    ]
    .iter()
    .map(|(i, c, f, s)| fact(i, c, f, s).replace("\"practices\"", "\"planned_activity\""))
    .collect();
    let probe = rows(&items)
        + r#"
print([p for p in derive_supersessions(rows) if p[1] or p[2]])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "[]",
        "a dream run still closes plans because a later plan was mentioned"
    );
}

#[test]
fn a_dream_run_materialises_the_re_assertion_and_nothing_else() {
    // The positive half of the same claim: the arithmetic did not disappear, it
    // narrowed. One axis carrying a re-assertion AND a genuine second value --
    // the yoga shape of the design brief -- writes exactly one closure.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-09-11T00:00:00Z", "s2"),
        fact("c", "yoga twice a week", "2023-11-30T00:00:00Z", "s3"),
    ]) + r#"
for p in derive_supersessions(rows):
    print(p)
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "('a', '2023-11-30T00:00:00Z', 'c')\n('b', None, None)\n('c', None, None)"
    );
}

#[test]
fn a_re_derive_withdraws_a_closure_the_new_rule_does_not_find() {
    // Ruling Q4, relabelled in review as an OPERATING property rather than a
    // migration clause: a materialised closure the rule no longer finds falls
    // back to NULL. Without it every closure the old axis arithmetic wrote
    // survives the fix invisibly, and a reverted judgement could never be undone.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["expired_at"] = "2023-11-30T00:00:00Z"
rows[0]["superseded_by"] = "b"
print(derive_supersessions(rows)[0])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "('a', None, None)",
        "the old arithmetic closure survived the re-derive"
    );
}

#[test]
fn a_re_derive_keeps_a_closure_that_names_its_author() {
    // And the counter-direction, which is what makes the withdrawal safe: an
    // ATTRIBUTED closure is not re-derivable from the chain, so a round that
    // cleared it would silently delete a judgement. Ruling Q2 makes the reason
    // part of the ruling for exactly this reason; W3 is what fills it.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["expired_at"] = "2023-11-30T00:00:00Z"
rows[0]["superseded_by"] = "b"
rows[0]["closure_source"] = "judge"
print(derive_supersessions(rows)[0])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "('a', '2023-11-30T00:00:00Z', 'b')",
        "a judged closure did not survive the re-derive"
    );
}

#[test]
fn the_read_path_lets_two_open_statements_coexist() {
    // The presentation half of ruling Q1: an unclosed statement is an answer of
    // its own, not the history of whatever was said last on the axis. This is
    // the deliberate error direction -- the bundle shows more coexistence until
    // a judgement links two statements, and it never hides a value that is true.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
c = project_fact_candidate("a", rows)
print(c["claim"], "|", len(c["history"]))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", &probe),
        "yoga twice a week | 0",
        "an unclosed statement was collapsed onto a later one"
    );
}

#[test]
fn the_read_path_collapses_a_closed_statement_onto_its_successor() {
    // R4 keeps its guarantee, on the new definition of "superseded": a hit whose
    // statement the store CLOSED is a field on the statement that closed it, so
    // no stale claim ever looks like the value in force.
    let probe = rows(&[
        fact("a", "yoga twice a week", "2023-08-11T00:00:00Z", "s1"),
        fact("b", "sourdough on weekends", "2023-11-30T00:00:00Z", "s2"),
    ]) + r#"
rows[0]["expired_at"] = "2023-11-30T00:00:00Z"
rows[0]["superseded_by"] = "b"
rows[0]["closure_source"] = "judge"
c = project_fact_candidate("a", rows)
print(c["claim"], "|", "; ".join("%s->%s" % (h["claim"], h["until"]) for h in c["history"]))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", &probe),
        "sourdough on weekends | yoga twice a week->2023-11-30T00:00:00Z"
    );
}
