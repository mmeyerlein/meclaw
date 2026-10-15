//! Statement identity W1 -- the learned cardinality rule needs CROSS-SESSION
//! evidence (GitHub #13, ruling Q3, delivery clause "the session guard ships
//! first").
//!
//! The rule said: two facts of one axis starting at the same instant prove the
//! axis enumerates, so it never derives supersession again. The 0.2.0 P8a run
//! measured that signal as degenerate -- LongMemEval stamps one instant per
//! SESSION and every turn of a session inherits it, so two facts from ONE
//! conversation are indistinguishable from two values that genuinely started
//! together. 143 of 187 multi-version axes were frozen that way, at least 103 of
//! them by the learned half alone.
//!
//! Since W1 coexistence counts only when the two facts arrived in two DIFFERENT
//! sessions. The session is read from `facts.session_id`, stamped at write time
//! from the episode the fact was extracted from, because the store has no joins
//! and the read path may not pay a round trip per axis.
//!
//! Both chain scripts are probed: the block is byte-identical by drift lock
//! (`p15_dream_glue.rs`), and a guard that fires in only one of them would make
//! the materialised `expired_at` disagree with the derived one.

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

/// Two facts of one unlisted axis at the SAME instant, with the session of each
/// as a parameter, plus an optional third value arriving strictly later.
fn two_at_one_instant(left_session: &str, right_session: &str, third: bool) -> String {
    let tail = if third {
        r#",
 {"id":"c","subject":"user:u1","predicate":"practices","canonical_predicate":"practices",
  "claim":"yoga three times a week","valid_from":"2023-11-30T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-03T00:00:00Z","episode_id":"e3","session_id":"s9"}"#
    } else {
        ""
    };
    format!(
        r#"
rows = [
 {{"id":"a","subject":"user:u1","predicate":"practices","canonical_predicate":"practices",
  "claim":"yoga twice a week","valid_from":"2023-08-11T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","episode_id":"e1","session_id":"{left_session}"}},
 {{"id":"b","subject":"user:u1","predicate":"practices","canonical_predicate":"practices",
  "claim":"the user practices yoga","valid_from":"2023-08-11T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-02T00:00:00Z","episode_id":"e2","session_id":"{right_session}"}}{tail}
]
"#
    )
}

#[test]
fn one_session_stamping_one_instant_is_no_coexistence_evidence() {
    // The measured defect, in one probe. Both facts come from the same
    // conversation, so the axis keeps the plain state behaviour and the learned
    // rule contributes nothing. Before W1 this pair alone froze the axis.
    let probe = two_at_one_instant("s1", "s1", false)
        + "print(axis_is_multivalued(build_chains(rows)[('user:u1','practices')]))\n";
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "False",
            "{name}: two facts of ONE session still count as an enumeration proof"
        );
    }
}

#[test]
fn two_sessions_stamping_one_instant_are_coexistence_evidence() {
    // The evidence the rule was written for, now with the origin it always
    // needed: two conversations state two values that start together. That is
    // coexistence a write order cannot fake, and the axis enumerates from then
    // on -- exactly as before W1, only on proof instead of on noise.
    let probe = two_at_one_instant("s1", "s2", false)
        + "print(axis_is_multivalued(build_chains(rows)[('user:u1','practices')]))\n";
    for (script, name) in [
        (recall_script(), "recall"),
        (dream_glue_script(), "dream-glue"),
    ] {
        assert_eq!(
            run_probe(&script, name, &probe),
            "True",
            "{name}: coexistence across two sessions was not read as an enumeration"
        );
    }
}

#[test]
fn a_missing_session_id_is_not_evidence() {
    // A row from before the migration carries no session. Absence of an origin
    // is not proof of a different origin -- the conservative direction, and the
    // one the seed list stands in front of.
    for (left, right) in [("", "s2"), ("s1", ""), ("", "")] {
        let probe = two_at_one_instant(left, right, false)
            + "print(axis_is_multivalued(build_chains(rows)[('user:u1','practices')]))\n";
        assert_eq!(
            run_probe(&recall_script(), "recall", &probe),
            "False",
            "an unknown session origin ({left:?}/{right:?}) was read as evidence"
        );
    }
}

#[test]
fn the_frozen_axis_thaws_without_the_later_value_ending_anything() {
    // The 143-of-187 shape end to end, on the specimen from the P8a receipt: two
    // yoga statements minted from one conversation at one instant, then a third
    // value months later. The axis THAWS -- that is W1's claim and it is intact.
    //
    // What the third value does on the thawed axis is W2's (#13, ruling Q1), and
    // this pin is where the two packages meet. W1 shipped with the arithmetic
    // still attached, and section 4 of its receipt measured what that would have
    // cost: 928 materialised closures instead of 49 across the eight P8a stores,
    // 636 of them on foresight facts, because a thawed bucket axis closed every
    // plan the moment the next plan was mentioned. Since W2 three different
    // values are three statements and none of them ends another.
    let probe = two_at_one_instant("s1", "s1", true)
        + r#"
chain = build_chains(rows)[('user:u1','practices')]
print(axis_is_multivalued(chain),
      [p for p in derive_supersessions(rows) if p[1] or p[2]])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "False []"
    );
}

#[test]
fn cross_session_coexistence_still_blocks_the_later_value() {
    // The protection direction of the same shape: once the coexistence is
    // PROVEN, a later third value ends nothing. This is the case the ruling
    // calls the dangerous error direction, and it is the reason the guard asks
    // for evidence rather than dropping the learned rule.
    //
    // Since W2 an empty triple is emitted per fact rather than nothing at all --
    // that list is the channel through which a closure the old arithmetic left
    // behind is withdrawn (ruling Q4) -- so the assertion counts CLOSURES.
    let probe = two_at_one_instant("s1", "s2", true)
        + "print([p for p in derive_supersessions(rows) if p[1] or p[2]])\n";
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", &probe),
        "[]",
        "a proven enumeration derived a supersession"
    );
}

#[test]
fn the_seed_list_stands_in_front_of_the_guard() {
    // Precedence, pinned: seed before learned (ruling Q3, option D). Two
    // children written in one breath in ONE session are no learned evidence any
    // more -- and `has_child` is on the curated multi list, so the axis
    // enumerates anyway and the later child ends nothing.
    let probe = r#"
rows = [
 {"id":"a","subject":"user:u1","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Robin","valid_from":"2002-03-04T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","episode_id":"e1","session_id":"s1"},
 {"id":"b","subject":"user:u1","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Noa","valid_from":"2002-03-04T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","episode_id":"e1","session_id":"s1"},
 {"id":"c","subject":"user:u1","predicate":"has_child","canonical_predicate":"has_child",
  "claim":"Mika","valid_from":"2019-03-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-06-01T00:00:00Z","episode_id":"e2","session_id":"s2"},
]
print(axis_is_multivalued(build_chains(rows)[('user:u1','has_child')]),
      [p for p in derive_supersessions(rows) if p[1] or p[2]])
"#;
    assert_eq!(
        run_probe(&dream_glue_script(), "dream-glue", probe),
        "True []"
    );
}

#[test]
fn the_read_path_agrees_with_the_dream_on_the_thawed_axis() {
    // The other half of the same rule: what the dream materialises, a recall
    // derives. On the thawed axis that is now NOTHING, so each of the three
    // statements stays an answer of its own and carries no predecessors -- the
    // read-path twin of the case above, and the reason the currency marker
    // (ruling Q6) is what tells a reader which values the store has closed.
    let probe = two_at_one_instant("s1", "s1", true)
        + r#"
for hit in ("a", "b", "c"):
    c = project_fact_candidate(hit, rows)
    print(c["id"], c["claim"], "|", len(c["history"]))
"#;
    assert_eq!(
        run_probe(&recall_script(), "recall", &probe),
        "a yoga twice a week | 0\n\
         b the user practices yoga | 0\n\
         c yoga three times a week | 0"
    );
}

/// The store call written under `marker` — from its own `"operation"` up to the
/// next one in the script.
///
/// A `tool_call_id` marker opens its call, so the args follow it; every other
/// marker (a phase name behind `store_op(...)`, or a predicate inside the args)
/// sits inside or behind them, so the args start at the `"operation"` in front.
fn store_call<'a>(script: &'a str, marker: &str) -> &'a str {
    let at = script
        .find(marker)
        .unwrap_or_else(|| panic!("no store call marked `{marker}`"));
    let (head, tail) = script.split_at(at);
    let start = if marker.starts_with("\"r-") {
        at + tail
            .find("\"operation\"")
            .unwrap_or_else(|| panic!("`{marker}` opens no store call"))
    } else {
        head.rfind("\"operation\"")
            .unwrap_or_else(|| panic!("`{marker}` closes no store call"))
    };
    let body = &script[start..];
    match body[1..].find("\"operation\"") {
        Some(n) => &body[..n + 1],
        None => body,
    }
}

#[test]
fn every_axis_page_select_carries_the_session_column() {
    // The rule can only read what the select fetched, and a missing column would
    // not fail loudly -- it would silently turn every axis functional again.
    // The three selects whose rows reach `build_chains` are exactly the AXIS
    // PAGES: recall's axis hydration, recall's window-mode temporal leg, and
    // dream-glue's supersession page. Each is bounded by AXIS_LIMIT, which is
    // what distinguishes them from the leg selects tagged with the same phase.
    for (script, name, anchors) in [
        (
            recall_script(),
            "recall",
            // #418: the axis page is a CALL of the hydration bundle, found by
            // its `tool_call_id`; the window-mode temporal page shares its id
            // with the point-mode one, so it is found by the predicate only IT
            // carries. What both have to fetch is unchanged.
            vec!["\"r-hyd-axis\"", "{\"lte\": window[1]}"],
        ),
        (
            dream_glue_script(),
            "dream-glue",
            vec!["AXIS_LIMIT}, \"sup-axes\""],
        ),
    ] {
        for anchor in anchors {
            let select = store_call(&script, anchor);
            assert!(
                select.contains("\"session_id\""),
                "{name}: the axis page at `{anchor}` does not fetch session_id"
            );
        }
    }
}
