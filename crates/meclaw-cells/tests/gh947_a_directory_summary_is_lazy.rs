//! GH #947 -- a directory's one-line summary is made when it is read, never
//! when something under it is written. A write marks every ancestor `dirty`
//! (and moves its `changed_seq`); `dir_summary {path}` answers the stored
//! line while the row is clean (`fresh: false`, no model), and otherwise
//! makes ONE summarizer call over the directory's children (their own one
//! lines) and stores it, clean, under the `changed_seq` it read. It never
//! cascades: the parent stays dirty.
//!
//! Why: a write-triggered summary would cost one model call per ancestor on
//! every write, for directories nobody may ever look at. Lazy, a burst of
//! writes costs nothing and the next reader pays exactly once.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder (file summaries and directory
//! summaries told apart by the directory prompt). No embedding endpoint
//! (`embed` off), no provider, no net.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

fn plain() -> Space {
    Space::with("/x/files", &[("derive", "embed", json!("0"))])
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn head_of(sp: &Space, file: &str) -> String {
    sp.rows(&format!("SELECT head FROM files WHERE file = '{file}'"))[0][0]
        .as_str()
        .unwrap()
        .to_string()
}

/// The prompt `./derive` sends with a directory summary.
fn dir_prompt() -> Value {
    pure("derive", "DIR_SUMMARY_PROMPT", json!(null))
}

/// Answer every FILE summary the recorder holds; a directory summary here
/// would be one a write asked for.
fn settle(sp: &mut Space, prompt: &Value) {
    while let Some((cell, m)) = sp.llm.front() {
        assert_eq!(cell, "summarizer", "only the summarizer is asked");
        assert_ne!(
            m.body.get("system").map(|s| &s["instructions"]["text"]),
            Some(prompt),
            "a write never asks for a directory summary"
        );
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
}

/// Every directory summary the summarizer was ever handed.
fn dir_calls(sp: &Space, prompt: &Value) -> Vec<Value> {
    sp.sent
        .iter()
        .filter(|m| {
            m["to"] == json!("./summarizer")
                && m["body"]["system"]["instructions"]["text"] == *prompt
        })
        .cloned()
        .collect()
}

fn write(sp: &mut Space, file: &str, body: &str, prompt: &Value) {
    let base = head_of(sp, file);
    ok(sp.request(
        "in_write",
        "overwrite",
        Some(file),
        json!({"text": body, "base": &base[..12]}),
        json!({}),
    ));
    settle(sp, prompt);
}

/// `dir_summary` on `in_read`; a directory summary the recorder holds on the
/// way is answered with `line`. Returns the one answer's body.
fn dir_summary(sp: &mut Space, path: &str, line: &str, prompt: &Value) -> Value {
    let op_id = sp.next_op_id();
    let before = sp.out.len();
    sp.lane(
        "in_read",
        json!({}),
        json!({"op": "dir_summary", "op_id": op_id}),
        json!({"op": "dir_summary", "args": {"path": path}}),
    );
    loop {
        let mine: Vec<Msg> = sp.out[before..]
            .iter()
            .filter(|m| m.route() == "answer" && m.hop.get("op_id") == Some(&json!(op_id)))
            .cloned()
            .collect();
        if !mine.is_empty() || sp.llm.is_empty() {
            assert_eq!(mine.len(), 1, "one answer; stderr {:?}", sp.stderr);
            let mut b = mine[0].body.clone();
            b.remove("messages");
            return Value::Object(b);
        }
        let (_, m) = sp.llm.front().unwrap();
        assert_eq!(
            m.body.get("system").map(|s| &s["instructions"]["text"]),
            Some(prompt),
            "only the directory's summary is asked"
        );
        sp.llm_answer(line, "stop");
    }
}

fn dirty(sp: &Space) -> Vec<Vec<Value>> {
    sp.rows("SELECT path, dirty FROM dirs ORDER BY path")
}

#[test]
fn a_directory_summary_is_made_when_read_and_only_then() {
    if !shipped() {
        return;
    }
    let prompt = dir_prompt();
    let mut sp = plain();
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/a/b/x.py", "text": "def f():\n    return 1\n"}),
        json!({}),
    ));
    let f = a["file"].as_str().unwrap().to_string();
    settle(&mut sp, &prompt);
    // Ten writes in all: no directory summary is made.
    for i in 2..=10 {
        write(&mut sp, &f, &format!("def f():\n    return {i}\n"), &prompt);
    }
    assert_eq!(
        dir_calls(&sp, &prompt).len(),
        0,
        "ten writes, no model call"
    );
    let all_dirty = vec![
        vec![json!("/"), json!(1)],
        vec![json!("/a"), json!(1)],
        vec![json!("/a/b"), json!(1)],
    ];
    assert_eq!(dirty(&sp), all_dirty, "every ancestor is dirty");

    // The first read: exactly one call over the children's one lines.
    let got = ok(dir_summary(&mut sp, "/a/b", "Holds the sources.", &prompt));
    assert_eq!(
        (
            got["path"].clone(),
            got["summary"].clone(),
            got["fresh"].clone()
        ),
        (json!("/a/b"), json!("Holds the sources."), json!(true))
    );
    let calls = dir_calls(&sp, &prompt);
    assert_eq!(calls.len(), 1, "one model call");
    assert_eq!(
        calls[0]["body"]["messages"][0]["text"],
        json!("Directory: /a/b\n\nx.py: One line of the file."),
        "the children's one lines, never their contents"
    );
    assert_eq!(
        sp.rows("SELECT dirty, summary, summary_seq = changed_seq FROM dirs WHERE path = '/a/b'"),
        vec![vec![json!(0), json!("Holds the sources."), json!(1)]],
        "stored, clean, under the changed_seq it read"
    );

    // Twice more without a change: the stored line, no model.
    for _ in 0..2 {
        let got = ok(dir_summary(&mut sp, "/a/b", "unused", &prompt));
        assert_eq!(
            (got["summary"].clone(), got["fresh"].clone()),
            (json!("Holds the sources."), json!(false))
        );
    }
    assert_eq!(dir_calls(&sp, &prompt).len(), 1, "unchanged: no model");

    // One change: dirty up to the root again, still no model call.
    write(&mut sp, &f, "def f():\n    return 99\n", &prompt);
    assert_eq!(dirty(&sp), all_dirty, "a write marks every ancestor");
    assert_eq!(dir_calls(&sp, &prompt).len(), 1, "a write asks no model");
    let got = ok(dir_summary(
        &mut sp,
        "/a/b",
        "Holds the changed sources.",
        &prompt,
    ));
    assert_eq!(
        (got["summary"].clone(), got["fresh"].clone()),
        (json!("Holds the changed sources."), json!(true))
    );
    assert_eq!(dir_calls(&sp, &prompt).len(), 2, "exactly one more call");
    assert_eq!(
        dirty(&sp),
        vec![
            vec![json!("/"), json!(1)],
            vec![json!("/a"), json!(1)],
            vec![json!("/a/b"), json!(0)],
        ],
        "never cascading: the parents stay dirty"
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE op_id LIKE 'y-%' OR op_id LIKE 's-%'"),
        vec![vec![json!(0)]],
        "no job left parked"
    );
}
