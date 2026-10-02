//! GH #944, R-BC-1 condition 3 -- the rust scanner is a standard-library
//! tokenizer, not a parser, so it must not be fooled by what only looks like
//! an item: `fn` inside a string, a raw string `r#"..."#` (any number of `#`),
//! a byte string, a char literal next to a lifetime, line, block, nested
//! block and doc comments. A `macro_rules!` body is one `macro:` node, never
//! the `fn`s it would expand to. Modules nest; methods hang on the type, not
//! on the block, and a second inherent block of a type adds no node. A file
//! the scanner cannot balance is `unparsable` with no nodes -- a guess past
//! a broken bracket would invent items. Python that `ast` refuses falls back
//! to the indentation scanner; a binary without a known name has no extractor.
//!
//! Pure: the extractor block of `./derive` and `./read`, loaded via `ast`,
//! both cells asked the same.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The two cells that carry the extractor block, a twin.
const CELLS: [&str; 2] = ["derive", "read"];

/// `extract(...)` of every case in `ARGS`: `{path, mime?, data, pages?,
/// limits?}`, `data` a text or a list of bytes.
const EXTRACT: &str = "[extract(a['path'], a.get('mime', ''), \
    a['data'].encode() if isinstance(a['data'], str) else bytes(a['data']), \
    a.get('pages'), a.get('limits') or {}) for a in ARGS]";

fn run(cell: &str, cases: Value) -> Vec<Value> {
    pure(cell, EXTRACT, cases)
        .as_array()
        .expect("a list")
        .clone()
}

/// Every node as `anchor (kind, from-to)`.
fn items(r: &Value) -> Vec<String> {
    r["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| {
            format!(
                "{} ({}, {}-{})",
                n["anchor"].as_str().expect("anchor"),
                n["kind"].as_str().expect("kind"),
                n["from"],
                n["to"]
            )
        })
        .collect()
}

/// `fmt/parser/mark <n>n <l>l` of one result.
fn verdict(r: &Value) -> String {
    format!(
        "{}/{}/{} {}n {}l",
        r["fmt"].as_str().expect("fmt"),
        r["parser"].as_str().expect("parser"),
        r["mark"].as_str().expect("mark"),
        r["nodes"].as_array().expect("nodes").len(),
        r["links"].as_array().expect("links").len()
    )
}

/// Every place an item hides that is not an item.
const RS_TRICKY: &str = r###"//! Crate doc: fn in_inner_doc() {}
/// Outer doc: fn in_doc() {
// Line comment: fn in_line() {
/* Block comment: fn in_block() { */
/* Outer /* nested fn in_nested() { */ still a comment } */
const A: &str = "fn in_string() { \" }";
const B: &str = r#"fn in_raw() { "quoted" }"#;
const C: &str = r##"fn in_raw2() "# {"##;
const D: &[u8] = b"fn in_bytes() {";
const E: char = '{';
const F: char = '\'';
const G: char = '"';

fn real<'a>(x: &'a str) -> &'a str {
    let _brace = '}';
    let _s = "}";
    x
}

macro_rules! make {
    ($name:ident) => {
        fn $name() {}
        fn in_macro() {}
    };
}

fn after_macro() {}
"###;

/// Nested modules, an inherent and a trait `impl`, a second inherent block.
const RS_NEST: &str = r#"mod outer {
    pub mod inner {
        pub fn deep() {}
    }
    fn shallow() {}
}

pub struct Wrapper<T>(T);

impl<T: Clone> Wrapper<T> {
    pub fn new(v: T) -> Self {
        Wrapper(v)
    }
}

impl<T> std::fmt::Debug for Wrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("w")
    }
}

impl<T: Clone> Wrapper<T> {
    pub fn get(&self) -> T {
        self.0.clone()
    }
}
"#;

/// A parenthesis closed by a brace.
const RS_UNBALANCED: &str = r#"fn ok() {}

fn broken() {
    let x = (1, 2;
}
"#;

/// A brace never closed.
const RS_UNCLOSED: &str = r#"fn ok() {}

fn open() {
    let x = 1;
"#;

/// A nested block comment never closed.
const RS_OPEN_COMMENT: &str = r#"fn ok() {}
/* never closed /* nested */
fn hidden() {}
"#;

/// A raw string whose `"#` never comes.
const RS_OPEN_RAW: &str = r##"fn ok() {}
const R: &str = r#"never closed "# missing";
"##;

/// Python `ast` refuses: an open brace, a `def` without its colon.
const PY_BROKEN: &str = r#"import os


class Config:
    def load(self):
        return {

    def save(self) -> None
        pass


def main():
    print("ok")
"#;

#[test]
fn strings_comments_and_a_macro_body_hide_no_items() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/tricky.rs", "data": RS_TRICKY}]));
        assert_eq!(
            items(&got[0]),
            [
                "fn:real (fn, 14-18)",
                "macro:make (macro, 20-25)",
                "fn:after_macro (fn, 27-27)",
            ],
            "{cell}"
        );
        assert_eq!(got[0]["parser"], "scan", "{cell}");
        assert!(
            got[0]["links"].as_array().unwrap().is_empty(),
            "{cell}: no call is an edge"
        );
    }
}

#[test]
fn modules_nest_and_methods_hang_on_their_type() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/nest.rs", "data": RS_NEST}]));
        assert_eq!(
            items(&got[0]),
            [
                "mod:outer (mod, 1-6)",
                "mod:outer/mod:inner (mod, 2-4)",
                "mod:outer/mod:inner/fn:deep (fn, 3-3)",
                "mod:outer/fn:shallow (fn, 5-5)",
                "struct:Wrapper (struct, 8-8)",
                "impl:Wrapper (impl, 10-14)",
                "impl:Wrapper/fn:new (fn, 11-13)",
                "impl:Wrapper+Debug (impl, 16-20)",
                "impl:Wrapper+Debug/fn:fmt (fn, 17-19)",
                "impl:Wrapper/fn:get (fn, 23-25)",
            ],
            "{cell}"
        );
        let parents: Vec<(String, String)> = got[0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                (
                    n["anchor"].as_str().unwrap().to_string(),
                    n["parent"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("mod:outer", ""),
            ("mod:outer/mod:inner", "mod:outer"),
            ("mod:outer/mod:inner/fn:deep", "mod:outer/mod:inner"),
            ("mod:outer/fn:shallow", "mod:outer"),
            ("struct:Wrapper", ""),
            ("impl:Wrapper", ""),
            ("impl:Wrapper/fn:new", "impl:Wrapper"),
            ("impl:Wrapper+Debug", ""),
            ("impl:Wrapper+Debug/fn:fmt", "impl:Wrapper+Debug"),
            ("impl:Wrapper/fn:get", "impl:Wrapper"),
        ]
        .iter()
        .map(|(a, p)| (a.to_string(), p.to_string()))
        .collect();
        assert_eq!(parents, want, "{cell}");
    }
}

#[test]
fn a_file_the_scanner_cannot_balance_is_unparsable() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/a.rs", "data": RS_UNBALANCED},
                {"path": "/src/b.rs", "data": RS_UNCLOSED},
                {"path": "/src/c.rs", "data": RS_OPEN_COMMENT},
                {"path": "/src/d.rs", "data": RS_OPEN_RAW}
            ]),
        );
        let marks: Vec<String> = got.iter().map(verdict).collect();
        assert_eq!(
            marks,
            [
                "rust//unparsable 0n 0l",
                "rust//unparsable 0n 0l",
                "rust//unparsable 0n 0l",
                "rust//unparsable 0n 0l",
            ],
            "{cell}: no nodes, no parser"
        );
    }
}

#[test]
fn python_that_ast_refuses_is_scanned() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/broken.py", "data": PY_BROKEN}]));
        assert_eq!(verdict(&got[0]), "python/scan/ 4n 0l", "{cell}");
        assert_eq!(
            items(&got[0]),
            [
                "class:Config (class, 4-9)",
                "class:Config/def:load (def, 5-6)",
                "class:Config/def:save (def, 8-9)",
                "def:main (def, 12-13)",
            ],
            "{cell}"
        );
        assert!(
            got[0]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n["parser"] == "scan"),
            "{cell}: every node names its parser"
        );
    }
}

#[test]
fn a_binary_or_an_unknown_format_has_no_extractor() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/inbox/blob", "data": [0, 159, 146, 150, 255]},
                {"path": "/inbox/blob.py", "data": [0, 159, 146, 150, 255]},
                {"path": "/inbox/notes.yaml", "data": "a: 1\n"}
            ]),
        );
        let marks: Vec<String> = got.iter().map(verdict).collect();
        assert_eq!(
            marks,
            [
                "//no_extractor 0n 0l",
                "python//unparsable 0n 0l",
                "//no_extractor 0n 0l"
            ],
            "{cell}: a binary named .py is unparsable, not guessed"
        );
    }
}
