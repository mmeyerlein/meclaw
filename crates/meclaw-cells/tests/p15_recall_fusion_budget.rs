//! Ruling O-7 -- separate-universe fusion with an episode budget, probed as a
//! pure function inside the shipped recall script.
//!
//! Same probe pattern as `p15_recall_chain.rs`: the REAL `params.script_inline`
//! runs, never a copy -- the `${VAR:-default}` literals are resolved the way the
//! colony resolves them at instantiation, the module body runs against a stub
//! stdin, and the `park()` exit at its end is swallowed so the probe can call
//! `fuse_rank` directly with synthetic leg lists.

use std::io::Write;
use std::process::{Command, Stdio};

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
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

fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
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

/// Shared preamble: a compact writer for synthetic leg entries.
const MK: &str = r#"
mkf = lambda i: {"kind": "fact", "id": i}
mke = lambda i: {"kind": "episode", "id": i}
def shape(ranked):
    ids = [k[1] for k in ranked]
    return ids, sum(1 for k in ranked if k[0] == "episode"), \
           sum(1 for k in ranked if k[0] == "fact")
"#;

#[test]
fn an_episode_flood_no_longer_cuts_the_carrying_fact() {
    // The 2026-08-11 incident, rebuilt: the keyword leg answers with 12 verbatim
    // copies of earlier askings plus 2 question-echo facts, and the fact that
    // actually carries the answer is reachable ONLY through the temporal leg --
    // on positions 13/14, behind 10 junk facts. Under the plain [:TOPK] cut the
    // episodes ate the slots and the answer fell off the end.
    let probe = format!(
        "{}{}",
        MK,
        r#"
kw = [mkf("F01"), mke("E01"), mkf("F02"), mke("E02")] + \
     [mke("E%02d" % i) for i in range(3, 13)]
temporal = [mkf("F01"), mkf("F02")] + [mkf("T%02d" % i) for i in range(3, 13)] + \
           [mkf("FVSCODE"), mkf("FHELIX")]
ranked, _, _, _ = fuse_rank({"keyword": kw, "temporal": temporal})
ids, eps, facts = shape(ranked)
print(len(ranked), eps, facts)
print("FVSCODE" in ids, "FHELIX" in ids)
"#
    );
    assert_eq!(run_probe(&probe), "20 6 14\nTrue True");
}

#[test]
fn a_thin_episode_side_keeps_its_budget_against_a_full_fact_side() {
    // The hunger side of the same posting (registered miss 89527b6b): a fact can
    // collect three legs, an episode at most two, so a fact wall outranks every
    // episode by construction. The budget is a FLOOR -- 3 episodes stay in,
    // however many facts crowd in front of them.
    let probe = format!(
        "{}{}",
        MK,
        r#"
kw = [mkf("F%02d" % i) for i in range(1, 18)] + [mke("E%d" % i) for i in range(1, 4)]
sem = [mkf("F%02d" % i) for i in range(1, 31)]
temporal = [mkf("F%02d" % i) for i in range(1, 31)]
ranked, _, _, _ = fuse_rank({"keyword": kw, "semantic": sem, "temporal": temporal})
ids, eps, facts = shape(ranked)
print(len(ranked), eps, facts)
print(all("E%d" % i in ids for i in (1, 2, 3)))
"#
    );
    assert_eq!(run_probe(&probe), "20 3 17\nTrue");
}

#[test]
fn the_budget_backfills_when_the_fact_side_cannot_fill_the_bundle() {
    // The budget is a floor, not a ceiling: 2 facts cannot fill 20 slots, so the
    // episodes take the rest instead of leaving 12 slots empty. A hard cap at
    // EPISODE_BUDGET would ship an 8-item bundle here.
    let probe = format!(
        "{}{}",
        MK,
        r#"
kw = [mkf("F1"), mke("E01"), mkf("F2")] + [mke("E%02d" % i) for i in range(2, 13)]
temporal = [mkf("F1"), mkf("F2")]
ranked, _, _, _ = fuse_rank({"keyword": kw, "temporal": temporal})
ids, eps, facts = shape(ranked)
print(len(ranked), eps, facts)
print("F1" in ids, "F2" in ids)
"#
    );
    assert_eq!(run_probe(&probe), "14 12 2\nTrue True");
}

#[test]
fn the_composition_cut_keeps_the_ranked_order_untouched() {
    // O-2 tie-breaks and the O-4b first-winner semantics live in the SORT; O-7
    // only decides membership. The surviving keys must therefore appear in the
    // same relative order they had before the cut.
    let probe = format!(
        "{}{}",
        MK,
        r#"
kw = [mkf("F01"), mke("E01"), mkf("F02"), mke("E02")] + \
     [mke("E%02d" % i) for i in range(3, 13)]
temporal = [mkf("F01"), mkf("F02")] + [mkf("T%02d" % i) for i in range(3, 13)] + \
           [mkf("FVSCODE"), mkf("FHELIX")]
ranked, scores, _, _ = fuse_rank({"keyword": kw, "temporal": temporal})
# monotone non-increasing scores == no reordering happened, and no key was
# duplicated by the membership filter.
print(all(scores[a] >= scores[b] for a, b in zip(ranked, ranked[1:])))
print(len(ranked) == len(set(ranked)))
"#
    );
    assert_eq!(run_probe(&probe), "True\nTrue");
}
