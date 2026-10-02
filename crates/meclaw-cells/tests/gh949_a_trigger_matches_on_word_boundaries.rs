//! GH #949 -- a trigger of a push candidate matches the person's words on word
//! boundaries.
//!
//! A candidate that names triggers rides in front of the question of
//! `curator/push` only on a turn whose words say one of them. What "says"
//! means is pinned here, on the shipped functions alone (no hive, no store):
//! the token rule of the push's `entities` (dates, numbers, words, Unicode),
//! every token in NFC and casefolded, a possessive `'s` dropped, a trigger of
//! several words a run of consecutive tokens -- and never a part of a token:
//! `Firewall X` is not said by `Firewall Xenon`, `Firewall` not by
//! `firewalls`. A decomposed accent (NFD) is the same word as the composed one
//! (NFC): python's `\w` does not match a combining mark, so NFD text read
//! without normalising first would cut `café` in two.
//!
//! The technique is the gate table's (`curator_history.rs`,
//! `the_audience_gate_is_one_rule_in_every_cell`): the shipped
//! `script_inline` is read with `ast`, the named functions and the constants
//! they read are run, nothing else of the script -- its top level reads stdin.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::{script_of, shipped};
use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

/// Runs `trigger_hits(trigger, tokens_of(turn))` of the shipped push for each
/// case and prints `{missing, hits}`. Input: JSON `{src, cases}` on stdin.
const PURE: &str = r#"
import ast, json, sys
inp = json.load(sys.stdin)
NAMES = ("match_form", "tokens_of", "trigger_hits")
CONSTS = ("DATE", "NUMBER", "WORD", "POSSESSIVE", "CANDIDATE_TOKEN")
scope, found = {}, set()
for n in ast.parse(inp["src"]).body:
    if isinstance(n, ast.FunctionDef):
        if n.name not in NAMES:
            continue
        found.add(n.name)
    elif isinstance(n, ast.Assign):
        if not any(isinstance(t, ast.Name) and t.id in CONSTS for t in n.targets):
            continue
    elif not isinstance(n, (ast.Import, ast.ImportFrom)):
        continue
    exec(compile(ast.Module(body=[n], type_ignores=[]), "push", "exec"), scope)
missing = [x for x in NAMES if x not in found]
if missing:
    print(json.dumps({"missing": missing, "hits": []}))
else:
    hits = [scope["trigger_hits"](t, scope["tokens_of"](s)) for t, s in inp["cases"]]
    print(json.dumps({"missing": [], "hits": hits}))
"#;

/// `(trigger, the person's words, said?, why)`.
const CASES: &[(&str, &str, bool, &str)] = &[
    (
        "Firewall X",
        "\u{2026} die firewall x h\u{e4}ngt",
        true,
        "two words, in another case, mid-sentence",
    ),
    (
        "Firewall X",
        "die Firewall Xenon h\u{e4}ngt",
        false,
        "`X` is no part of `Xenon`",
    ),
    (
        "Firewall X",
        "zwei firewalls x",
        false,
        "`firewalls` is no `firewall`",
    ),
    (
        "Firewall",
        "zwei firewalls h\u{e4}ngen",
        false,
        "a one-word trigger is a whole token too",
    ),
    (
        "Firewall X",
        "X und die Firewall",
        false,
        "the words of a trigger in its order",
    ),
    (
        "Firewall X",
        "die firewall h\u{e4}ngt, x nicht",
        false,
        "the words of a trigger next to each other",
    ),
    // NFC and NFD are the same words, either side.
    (
        "Caf\u{e9} Gr\u{f6}\u{df}e",
        "das cafe\u{301} gro\u{308}\u{df}e passt",
        true,
        "an NFC trigger in NFD words",
    ),
    (
        "Cafe\u{301} Gro\u{308}\u{df}e",
        "das Caf\u{e9} Gr\u{f6}\u{df}e passt",
        true,
        "an NFD trigger in NFC words",
    ),
    (
        "Gr\u{f6}\u{df}e",
        "GR\u{d6}SSE",
        true,
        "casefolded, not lower-cased: \u{df} is ss",
    ),
    // A possessive is the word it belongs to.
    (
        "Firewall X",
        "Firewall X's update failed",
        true,
        "a possessive 's",
    ),
    (
        "Firewall X",
        "Firewall X\u{2019}s update failed",
        true,
        "a possessive with a typographic apostrophe",
    ),
    (
        "Anna's laptop",
        "Anna\u{2019}s Laptop is gone",
        true,
        "a possessive in the trigger itself",
    ),
    (
        "Anna",
        "Annas Laptop",
        false,
        "no apostrophe, no possessive: another word",
    ),
    // Punctuation is no token, on either side.
    (
        "Firewall X",
        "Was ist mit Firewall X?",
        true,
        "a question mark after the trigger",
    ),
    (
        "Firewall X!",
        "(firewall, x)",
        true,
        "punctuation in the trigger and between the words",
    ),
    (
        "Firewall X",
        "\u{201e}Firewall X\u{201c} h\u{e4}ngt",
        true,
        "a quoted name is matched word by word",
    ),
    (
        "Version 3.5",
        "es l\u{e4}uft version 3.5.",
        true,
        "a number the sentence ends on",
    ),
    (
        "!!!",
        "!!! alarm",
        false,
        "a trigger of punctuation alone hits nothing",
    ),
];

#[test]
fn a_trigger_matches_on_word_boundaries() {
    if !shipped() {
        return;
    }
    // The NFD cases are NFD: the bytes differ from their NFC twins.
    assert_ne!("Caf\u{e9}", "Cafe\u{301}");
    assert_ne!("Gr\u{f6}\u{df}e", "Gro\u{308}\u{df}e");
    let cases: Vec<Value> = CASES.iter().map(|(t, s, _, _)| json!([t, s])).collect();
    let doc = json!({"src": script_of("push"), "cases": cases});
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(PURE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("write the script");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "the pure half ran: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = sj::from_slice(&out.stdout).expect("one JSON document");
    assert_eq!(
        report["missing"],
        json!([]),
        "curator/push defines the trigger rule (GH #949)"
    );
    let hits = report["hits"].as_array().expect("one verdict per case");
    assert_eq!(hits.len(), CASES.len());
    for ((trigger, turn, want, why), got) in CASES.iter().zip(hits) {
        assert_eq!(got, &json!(want), "trigger {trigger:?} in {turn:?}: {why}");
    }
}
