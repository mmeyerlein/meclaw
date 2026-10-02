//! GH #947 -- a node is an edit target. `write_node {file: <addr>#<anchor>,
//! args: {text, base}}` replaces exactly the span of one node of the head,
//! and `insert` at an anchor puts text before or after it; both take the span
//! the head's `nodes` rows give (as `read` of `fh-...#<anchor>` reads it), so
//! `base` must be the head (`base_moved` otherwise) and an anchor the head does
//! not have answers `unknown_anchor` with its candidates. After that the write
//! takes the road every write takes -- hook, blocks, swing, derive -- so a
//! function rewritten in place leaves its neighbour byte for byte, a broken
//! one is refused like any broken write, and the anchor is there again once
//! `./derive` has run for the new head.
//!
//! The shipped space in one process (`support/file_space_hive.rs`) with the
//! store's declared indexes applied as the store factory applies them.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const F_PART: &str = "def f():\n    return 1\n";
const G_PART: &str = "\n\ndef g():\n    # the neighbour\n    return 2\n";
const S_MD: &str = "# T\n\n## A\nx\n\n## B\ny\n";
const C_PY: &str = "class A:\n    def m(self):\n        return 0\n";

/// The unique indexes whose `unique_violation` is an answer, not a fault.
const ANSWERS: [&str; 3] = ["claims_path", "dirs_path", "contrib_file"];

/// The shipped space without embeddings, its store carrying the declared
/// indexes (GH #915: the store factory applies them right after the schema).
fn space() -> Space {
    let sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    let store = meclaw_cells::store::StoreParams::parse(&cell_config("store")["params"])
        .expect("the store params parse");
    meclaw_cells::store::ddl::apply_index_ddl(&sp.db, &store.indexes).expect("index ddl");
    sp
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn refused(v: Value, code: &str) -> Value {
    assert_eq!(v["ok"], json!(false), "expected {code}: {v}");
    assert_eq!(v["error"]["code"], json!(code), "{v}");
    v
}

fn write(sp: &mut Space, op: &str, file: &str, args: Value) -> Value {
    sp.request("in_write", op, Some(file), args, json!({}))
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

fn anchors(sp: &Space, file: &str) -> Vec<String> {
    col(
        sp,
        &format!("SELECT anchor FROM nodes WHERE file = '{file}' ORDER BY anchor"),
    )
}

/// The head's content as `raw` answers it (standard base64).
fn raw_of(sp: &mut Space, file: &str) -> Value {
    ok(sp.read("raw", file, json!({})))["b64"].clone()
}

/// `text` as `raw` answers it.
fn as_raw(text: &str) -> Value {
    json!(b64(text.as_bytes()))
}

/// The changed lines of a unified diff that start with `sign`, its file
/// headers left out.
fn changed(diff: &str, sign: char) -> Vec<String> {
    let header = sign.to_string().repeat(3);
    diff.lines()
        .filter(|l| l.starts_with(sign) && !l.starts_with(&header))
        .map(str::to_string)
        .collect()
}

/// Answer every summary the recorder holds.
fn settle(sp: &mut Space) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn create(sp: &mut Space, path: &str, text: &str) -> (String, String) {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": text}),
        json!({}),
    ));
    settle(sp);
    let file = a["file"].as_str().expect("a file id").to_string();
    let head = head_of(sp, &file);
    (file, head)
}

/// What `./write` sent on `route` since `before` (an index into `sent`).
fn from_write(sp: &Space, before: usize, route: &str) -> Vec<Value> {
    sp.sent[before..]
        .iter()
        .filter(|m| m["from"] == json!("./write") && m["route"] == json!(route))
        .cloned()
        .collect()
}

/// Every route `./write`, `./derive` and `./ws` sent is one its contract
/// declares; no store fault, no unscoped store op, no traceback, no claim
/// left.
fn assert_clean(sp: &Space) {
    for cell in ["write", "derive", "ws"] {
        let declared: Vec<String> =
            strings(&cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]);
        let from = format!("./{cell}");
        for m in sp.sent.iter().filter(|m| m["from"] == json!(from)) {
            let route = m["route"].as_str().unwrap_or("");
            assert!(
                declared.iter().any(|d| d == route),
                "{cell} sent {route}, its contract does not declare it"
            );
        }
    }
    let faults: Vec<&String> = sp
        .store_errors
        .iter()
        .filter(|e| !(e.contains("unique_violation") && ANSWERS.iter().any(|i| e.contains(i))))
        .collect();
    assert!(faults.is_empty(), "store faults: {faults:?}");
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert!(
        !sp.stderr.iter().any(|e| e.contains("Traceback")),
        "{:?}",
        sp.stderr
    );
    assert!(
        sp.rows("SELECT path FROM claims").is_empty(),
        "every claim dropped"
    );
}

#[test]
fn a_function_is_rewritten_and_its_neighbour_stays() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let py = format!("{F_PART}{G_PART}");
    let (f, h) = create(&mut sp, "/m/n.py", &py);
    assert_eq!(anchors(&sp, &f), vec!["def:f", "def:g"]);

    let (n, s) = (sp.out.len(), sp.sent.len());
    let r = ok(write(
        &mut sp,
        "write_node",
        &format!("{f}#def:f"),
        json!({"text": "def f():\n    return 42\n", "base": &h[..12]}),
    ));
    let h2 = head_of(&sp, &f);
    assert_ne!(h2, h, "a new head");
    assert_eq!(
        (r["anchor"].clone(), r["version"].clone(), r["base"].clone()),
        (json!("def:f"), json!(&h2[..12]), json!(&h[..12]))
    );
    // f's lines are new, g's are the same bytes.
    assert_eq!(
        raw_of(&mut sp, &f),
        as_raw(&format!("def f():\n    return 42\n{G_PART}"))
    );
    let diff = r["diff"].as_str().expect("a diff");
    assert_eq!(changed(diff, '-'), vec!["-    return 1"], "{diff}");
    assert_eq!(changed(diff, '+'), vec!["+    return 42"], "{diff}");
    assert_eq!(
        col(
            &sp,
            &format!("SELECT op FROM line WHERE file = '{f}' ORDER BY seq DESC LIMIT 1")
        ),
        vec!["write_node"]
    );
    let derive: Vec<Value> = from_write(&sp, s, "in_derive")
        .iter()
        .map(|m| m["body"]["file"].clone())
        .collect();
    assert_eq!(derive, vec![json!(&f)], "the normal road: one derive job");
    let ev: Vec<Value> = sp.out[n..]
        .iter()
        .filter(|m| m.route() == "source_changed")
        .map(|m| json!([m.body["source"], m.body["version"]]))
        .collect();
    assert_eq!(
        ev,
        vec![json!([&f, &h2[..12]])],
        "one event for the new head"
    );
    settle(&mut sp);
    assert_eq!(
        anchors(&sp, &f),
        vec!["def:f", "def:g"],
        "def:f again after the derive of the new head"
    );
    assert_eq!(
        col(
            &sp,
            &format!("SELECT DISTINCT version FROM nodes WHERE file = '{f}'")
        ),
        vec![h2]
    );
    assert_clean(&sp);
}

#[test]
fn a_broken_function_is_refused_like_any_broken_write() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let py = format!("{F_PART}{G_PART}");
    let (f, h) = create(&mut sp, "/m/n.py", &py);
    let s = sp.sent.len();
    refused(
        write(
            &mut sp,
            "write_node",
            &format!("{f}#def:f"),
            json!({"text": "def f(:\n    return 1\n", "base": &h[..12]}),
        ),
        "syntax",
    );
    assert_eq!(head_of(&sp, &f), h, "nothing written");
    assert_eq!(raw_of(&mut sp, &f), as_raw(&py));
    assert!(from_write(&sp, s, "in_derive").is_empty());
    // As a broken overwrite of the whole file is.
    refused(
        write(
            &mut sp,
            "overwrite",
            &f,
            json!({"text": format!("def f(:\n    return 1\n{G_PART}"), "base": &h[..12]}),
        ),
        "syntax",
    );
    assert_eq!(head_of(&sp, &f), h);
    assert_clean(&sp);
}

#[test]
fn an_anchor_takes_an_insert_after_its_section() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let (f, h) = create(&mut sp, "/s.md", S_MD);
    let r = ok(write(
        &mut sp,
        "insert",
        &format!("{f}#sec:a"),
        json!({"where": "after", "text": "more\n", "base": &h[..12]}),
    ));
    assert_eq!(r["anchor"], json!("sec:a"));
    assert_eq!(
        raw_of(&mut sp, &f),
        as_raw("# T\n\n## A\nx\n\nmore\n## B\ny\n")
    );
    settle(&mut sp);
    // By path, and before a section.
    let h2 = head_of(&sp, &f);
    ok(write(
        &mut sp,
        "insert",
        "/s.md#sec:b",
        json!({"where": "before", "text": "<!-- b -->", "base": &h2[..12]}),
    ));
    assert_eq!(
        raw_of(&mut sp, &f),
        as_raw("# T\n\n## A\nx\n\nmore\n<!-- b -->\n## B\ny\n")
    );
    settle(&mut sp);
    assert_clean(&sp);
}

#[test]
fn an_unknown_anchor_or_a_moved_base_is_refused() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let (f, h) = create(&mut sp, "/k.py", C_PY);
    // The same last name elsewhere is a candidate; no such name, none.
    let e = refused(
        write(
            &mut sp,
            "write_node",
            &format!("{f}#def:m"),
            json!({"text": "x\n", "base": &h[..12]}),
        ),
        "unknown_anchor",
    );
    assert_eq!(e["error"]["candidates"], json!(["class:A/def:m"]));
    let e = refused(
        write(
            &mut sp,
            "write_node",
            &format!("{f}#def:zz"),
            json!({"text": "x\n", "base": &h[..12]}),
        ),
        "unknown_anchor",
    );
    assert_eq!(e["error"]["candidates"], json!([]));
    refused(
        write(
            &mut sp,
            "insert",
            &format!("{f}#def:zz"),
            json!({"text": "x\n", "base": &h[..12]}),
        ),
        "unknown_anchor",
    );

    // The head moves; the old base is refused, with the head it moved to.
    ok(write(
        &mut sp,
        "write_node",
        &format!("{f}#class:A/def:m"),
        json!({"text": "    def m(self):\n        return 1\n", "base": &h[..12]}),
    ));
    settle(&mut sp);
    let h2 = head_of(&sp, &f);
    let e = refused(
        write(
            &mut sp,
            "write_node",
            &format!("{f}#class:A/def:m"),
            json!({"text": "    def m(self):\n        return 2\n", "base": &h[..12]}),
        ),
        "base_moved",
    );
    assert_eq!(e["error"]["current"], json!(&h2[..12]));
    // A version in an anchored address: the version travels as `base` only.
    refused(
        write(
            &mut sp,
            "write_node",
            &format!("{f}@{}#class:A/def:m", &h2[..8]),
            json!({"text": "x\n", "base": &h2[..12]}),
        ),
        "bad_request",
    );
    assert_eq!(
        raw_of(&mut sp, &f),
        as_raw("class A:\n    def m(self):\n        return 1\n")
    );
    assert_clean(&sp);
}
