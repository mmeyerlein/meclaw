//! GH #973 -- a count is ASCII digits. `expected` of `replace` and
//! `replace_regex` was checked with `str.isdigit()`, which takes a superscript
//! two, and `int()` of it raised: the write died without an answer. The same
//! flaw as the librarian's cursor (M-7), found beside it: now a count that is
//! not ASCII digits is no match, and the write answers with its code.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::{Space, shipped};
use meclaw_core::serde_json::json;

#[test]
fn a_superscript_count_is_answered_not_raised() {
    if !shipped() {
        return;
    }
    let mut s = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    let a = s.request(
        "in_write",
        "create",
        None,
        json!({"path": "/n.txt", "text": "l1\nl2\n"}),
        json!({}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let f = a["file"].as_str().unwrap().to_string();
    let v = a["version"].as_str().unwrap().to_string();
    for (op, args) in [
        (
            "replace",
            json!({"old": "l1\n", "new": "x\n", "expected": "\u{b2}", "base": v}),
        ),
        (
            "replace_regex",
            json!({"pattern": "^l", "repl": "x", "expected": "\u{b2}", "base": v}),
        ),
    ] {
        let got = s.request("in_write", op, Some(&f), args, json!({}));
        assert_eq!(got["ok"], json!(false), "{op}: {got}");
        assert!(
            !s.stderr.iter().any(|e| e.contains("Traceback")),
            "{op}: {:?}",
            s.stderr
        );
    }
}
