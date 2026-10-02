//! GH #947 -- what a file is about leaves the space once per stored summary.
//!
//! A catalog of the space's files needs the one line and the topic tags of
//! every living head, and it must not have to ask each file for them: so
//! `./derive`, once it has stored the summary of a version (`files.oneline`
//! and the `tags` row written), sends exactly ONE
//! `source_described {source, version, path, oneline, tags}` out of the hive,
//! the head carrying the route alone (no `caller`: it leaves whoever wrote).
//! Only for a living head: the `d-write` leg that lays the one line on the
//! head (`update files ... where {file, head: v}`) must have hit its row, and
//! the file must not be removed -- a slow summary of an older version, or of a
//! file removed in between, describes nothing, and the job of the newer head
//! does it. No stored summary (a cut or empty answer, or none asked for under
//! `summary_on_commit` "0"), no event. `source_changed` stays exactly one per
//! head move; the description is a second event, never a second
//! `source_changed`. The path is the one the file has when it is described (a
//! move keeps the head and changes the path).
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder, answered by this file in the order
//! each case needs.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The summarizer's answer: a one line, a paragraph, and a tag line with a
/// repeat and a case the store folds.
const SUMMARY: &str =
    "One line of the file.\n\nA short paragraph.\n\nTAGS: Notes, Plans  Today, notes";

/// The shipped space without embeddings (no endpoint needed), plus `over`.
fn plain(over: &[(&str, &str, Value)]) -> Space {
    let mut all: Vec<(&str, &str, Value)> = vec![("derive", "embed", json!("0"))];
    all.extend(over.iter().cloned());
    Space::with("/x/files", &all)
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn col(sp: &Space, sql: &str) -> Vec<String> {
    sp.rows(sql)
        .into_iter()
        .map(|r| {
            r[0].as_str()
                .map(str::to_string)
                .unwrap_or_else(|| r[0].to_string())
        })
        .collect()
}

fn head_of(sp: &Space, file: &str) -> String {
    col(sp, &format!("SELECT head FROM files WHERE file = '{file}'"))[0].clone()
}

/// Answer every summary the recorder holds, oldest first, with `text`.
fn settle_with(sp: &mut Space, text: &str, finish: &str) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer(text, finish);
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn settle(sp: &mut Space) {
    settle_with(sp, SUMMARY, "stop");
}

fn create(sp: &mut Space, path: &str, text: &str) -> (String, String) {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": text}),
        json!({}),
    ));
    let file = a["file"].as_str().expect("a file id").to_string();
    let head = head_of(sp, &file);
    (file, head)
}

fn replace(sp: &mut Space, file: &str, old: &str, new: &str) -> String {
    let base = head_of(sp, file);
    ok(sp.request(
        "in_write",
        "replace",
        Some(file),
        json!({"old": old, "new": new, "expected": 1, "base": &base[..12]}),
        json!({}),
    ));
    head_of(sp, file)
}

/// The events on `route` that left since `before` (an index into `out`),
/// every head checked: the route alone, no working context.
fn events(sp: &Space, route: &str, before: usize) -> Vec<Value> {
    sp.out[before..]
        .iter()
        .filter(|m| m.route() == route)
        .map(|m| {
            assert_eq!(
                Value::Object(m.hop.clone()),
                json!({"route": route}),
                "the head carries the route alone -- no caller, no op, no job"
            );
            for k in ["cur_origin", "cur_phase", "cur_job", "cur_call"] {
                assert!(!m.context.contains_key(k), "{k} leaves the space");
            }
            let mut b = m.body.clone();
            b.remove("messages");
            Value::Object(b)
        })
        .collect()
}

fn described(sp: &Space, before: usize) -> Vec<Value> {
    events(sp, "source_described", before)
}

fn changed(sp: &Space, before: usize) -> usize {
    events(sp, "source_changed", before).len()
}

/// The one description of a living head: the whole form, the values the
/// rows say -- `files.oneline` and the head's `tags` row.
fn assert_described(sp: &Space, e: &Value, file: &str, path: &str) {
    let head = head_of(sp, file);
    let one = col(
        sp,
        &format!("SELECT oneline FROM files WHERE file = '{file}'"),
    );
    let tags = col(
        sp,
        &format!(
            "SELECT text FROM summaries WHERE file = '{file}' AND version = '{head}' \
             AND level = 'tags'"
        ),
    );
    assert_eq!(tags.len(), 1, "one tags row of the head: {tags:?}");
    let tags: Value = sj::from_str(&tags[0]).expect("the tags row is a JSON list");
    assert_eq!(
        *e,
        json!({"source": file, "version": &head[..12], "path": path,
               "oneline": one[0], "tags": tags}),
    );
}

/// Every route `./derive`, `./write` and `./ws` sent is one its contract
/// declares (the colony's `validate_emits` would dead-letter any other),
/// `./derive` declares the new route, and the hive's rim names it beside
/// `source_changed`.
fn assert_contracts(sp: &Space) {
    for cell in ["derive", "write", "ws"] {
        let declared: Vec<String> =
            cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]
                .as_array()
                .unwrap_or_else(|| panic!("{cell} declares its routes"))
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
        assert!(declared.contains(&"source_changed".to_string()), "{cell}");
        if cell == "derive" {
            assert!(declared.contains(&"source_described".to_string()));
        }
        let from = format!("./{cell}");
        for m in sp.sent.iter().filter(|m| m["from"] == json!(from)) {
            let route = m["route"].as_str().unwrap_or("");
            assert!(
                declared.iter().any(|d| d == route),
                "{cell} sent {route}, its contract does not declare it"
            );
        }
    }
    let rim: Vec<String> = hive_config()["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect();
    for route in ["source_changed", "source_described"] {
        assert!(rim.contains(&route.to_string()), "{route}: {rim:?}");
    }
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
}

#[test]
fn a_stored_summary_of_the_head_is_described_once() {
    if !shipped() {
        return;
    }
    let mut sp = plain(&[]);

    // --- birth: nothing before the summary is stored, then exactly one ------
    let n0 = sp.out.len();
    let (f, v1) = create(&mut sp, "/notes/a.md", "# A\n\n## One\nx\n");
    assert_eq!(changed(&sp, n0), 1, "the head move's own event");
    assert_eq!(described(&sp, n0), Vec::<Value>::new(), "no summary yet");
    assert!(!sp.llm.is_empty(), "the summary is asked for");
    settle(&mut sp);
    let ev = described(&sp, n0);
    assert_eq!(ev.len(), 1, "one per stored summary: {ev:?}");
    assert_described(&sp, &ev[0], &f, "/notes/a.md");
    assert_eq!(
        (ev[0]["oneline"].clone(), ev[0]["tags"].clone()),
        (
            json!("One line of the file."),
            json!(["notes", "plans today"])
        )
    );
    assert_eq!(changed(&sp, n0), 1, "no second source_changed");

    // --- a write: one more of each ------------------------------------------
    let n1 = sp.out.len();
    let v2 = replace(&mut sp, &f, "x", "y");
    assert_ne!(v1, v2);
    assert_eq!(changed(&sp, n1), 1);
    settle(&mut sp);
    let ev = described(&sp, n1);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_described(&sp, &ev[0], &f, "/notes/a.md");
    assert_eq!(ev[0]["version"], json!(&v2[..12]));

    // --- a summary without a tag line: described, with no tags --------------
    let n2 = sp.out.len();
    let v3 = replace(&mut sp, &f, "y", "z");
    settle_with(&mut sp, "Plain one line.\n\nMore of it.", "stop");
    let ev = described(&sp, n2);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_described(&sp, &ev[0], &f, "/notes/a.md");
    assert_eq!(
        (ev[0]["oneline"].clone(), ev[0]["tags"].clone()),
        (json!("Plain one line."), json!([]))
    );
    assert_eq!(ev[0]["version"], json!(&v3[..12]));

    // Three head moves, three summaries: three of each.
    assert_eq!(sp.routed("source_changed").len(), 3);
    assert_eq!(sp.routed("source_described").len(), 3);
    assert_contracts(&sp);
}

#[test]
fn a_version_that_is_no_longer_head_is_not_described() {
    if !shipped() {
        return;
    }
    // Two writes before either summary arrives: oldest answered first (the
    // older job's one line finds no head row) and newest first (the older
    // job finds a newer version derived). Either way only the head is.
    for newest_first in [false, true] {
        let mut sp = plain(&[]);
        let (f, _) = create(&mut sp, "/s.md", "# S\n\nx\n");
        settle(&mut sp);
        let n = sp.out.len();
        replace(&mut sp, &f, "x", "y");
        let v3 = replace(&mut sp, &f, "y", "z");
        assert_eq!(changed(&sp, n), 2, "one source_changed per head move");
        assert_eq!(sp.llm.len(), 2, "two summaries asked for");
        if newest_first {
            sp.llm.rotate_right(1);
            sp.llm_answer(SUMMARY, "stop");
            assert_eq!(described(&sp, n).len(), 1, "the head, first");
        } else {
            sp.llm_answer(SUMMARY, "stop");
            assert_eq!(
                described(&sp, n),
                Vec::<Value>::new(),
                "the older version's summary describes nothing"
            );
        }
        settle(&mut sp);
        let ev = described(&sp, n);
        assert_eq!(ev.len(), 1, "newest first {newest_first}: {ev:?}");
        assert_eq!(ev[0]["version"], json!(&v3[..12]));
        assert_described(&sp, &ev[0], &f, "/s.md");
        assert_eq!(changed(&sp, n), 2, "no second source_changed");
        assert_contracts(&sp);
    }

    // A file removed before its summary arrives: the head stays, `tomb` is set.
    let mut sp = plain(&[]);
    let n = sp.out.len();
    let (f, v) = create(&mut sp, "/r.md", "# R\n\nx\n");
    ok(sp.request(
        "in_write",
        "remove",
        Some(&f),
        json!({"base": &v[..12]}),
        json!({}),
    ));
    settle(&mut sp);
    assert_eq!(
        described(&sp, n),
        Vec::<Value>::new(),
        "a removed file is not described"
    );
    assert_contracts(&sp);

    // A file moved before its summary arrives: described under its new path.
    let mut sp = plain(&[]);
    let n = sp.out.len();
    let (f, _) = create(&mut sp, "/m/a.md", "# M\n\nx\n");
    let m = ok(sp.request(
        "in_write",
        "move",
        Some(&f),
        json!({"to": "/n/b.md"}),
        json!({}),
    ));
    assert_eq!(m["moved"], json!(true), "{m}");
    settle(&mut sp);
    let ev = described(&sp, n);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_described(&sp, &ev[0], &f, "/n/b.md");
    assert_contracts(&sp);
}

#[test]
fn no_stored_summary_no_description() {
    if !shipped() {
        return;
    }
    // A cut answer and an empty one: the job stops, the head's own event stays.
    let mut sp = plain(&[]);
    let n = sp.out.len();
    let (f, _) = create(&mut sp, "/f.md", "# F\n\nx\n");
    settle_with(&mut sp, SUMMARY, "length");
    assert_eq!(described(&sp, n), Vec::<Value>::new(), "a cut summary");
    assert_eq!(changed(&sp, n), 1);
    let n = sp.out.len();
    replace(&mut sp, &f, "x", "y");
    settle_with(&mut sp, "  \n\n", "stop");
    assert_eq!(described(&sp, n), Vec::<Value>::new(), "an empty summary");
    assert_eq!(changed(&sp, n), 1);
    assert_eq!(
        col(
            &sp,
            &format!("SELECT COUNT(*) FROM summaries WHERE file = '{f}'")
        ),
        vec!["0"],
        "nothing was stored"
    );
    assert_contracts(&sp);

    // `summary_on_commit` "0": the birth is summarized and described, a later
    // write asks no model and is not described.
    let mut sp = plain(&[("derive", "summary_on_commit", json!("0"))]);
    let n = sp.out.len();
    let (f, _) = create(&mut sp, "/g.md", "# G\n\nx\n");
    settle(&mut sp);
    let ev = described(&sp, n);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_described(&sp, &ev[0], &f, "/g.md");
    let n = sp.out.len();
    replace(&mut sp, &f, "x", "y");
    assert!(sp.llm.is_empty(), "no model is asked");
    assert_eq!(described(&sp, n), Vec::<Value>::new());
    assert_eq!(changed(&sp, n), 1);
    assert_contracts(&sp);
}
