//! GH #944 -- a node is named by what it is, not by where it stands. Every
//! format the space reads (python, rust, markdown, json, toml, pdf) gives its
//! items anchors from the item's kind and name (`class:Store/def:load`,
//! `impl:Point+Display/fn:fmt`, `sec:budget`, `key:/server/tls`, `page:2`),
//! so an edit that only moves lines keeps every anchor and a rename changes
//! exactly one: graph edges and node reads survive a reformat. Twins in one
//! file are numbered in source order (`~2`; markdown the GitHub way, `-1`).
//! The marks say why a file has fewer nodes than items, or none.
//!
//! Pure: the extractor block of `./derive` and `./read`, loaded via `ast`
//! (README § 3 of wave File Hive B1), both cells asked the same.

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

fn anchors(r: &Value) -> Vec<String> {
    r["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| n["anchor"].as_str().expect("anchor").to_string())
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

/// A python module: classes, methods, async, a nested def, a decorator.
const PY: &str = r#""""A module."""
import os


class Store:
    """Holds rows."""

    def __init__(self, path):
        self.path = path

    async def load(self):
        def parse(line):
            return line.split()
        return [parse(l) for l in open(self.path)]

    @property
    def size(self):
        return 0


def helper(x):
    return x


async def main():
    store = Store(os.getcwd())
    await store.load()
"#;

/// `PY` with blank lines and comments pushed in: every line moves.
const PY_MOVED: &str = r#""""A module."""
# A comment that moves everything below.

import os



class Store:
    """Holds rows."""

    # Another comment.

    def __init__(self, path):
        self.path = path


    async def load(self):
        # Inner helper.
        def parse(line):
            return line.split()
        return [parse(l) for l in open(self.path)]

    @property
    def size(self):
        return 0


def helper(x):
    return x


async def main():
    store = Store(os.getcwd())
    await store.load()
"#;

/// A rust file: every item kind the scanner names.
const RS: &str = r#"use std::fmt;

pub struct Point {
    x: i32,
}

pub enum Shape {
    Dot,
}

pub trait Area {
    fn area(&self) -> f64;
}

mod geo {
    pub fn origin() -> i32 {
        0
    }
}

impl Point {
    pub fn new(x: i32) -> Self {
        Point { x }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.x)
    }
}

macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

pub fn run() {}
"#;

/// `RS` with a comment and doc lines pushed in.
const RS_MOVED: &str = r#"use std::fmt;

// A comment.


pub struct Point {
    x: i32,
}

pub enum Shape {
    Dot,
}

pub trait Area {
    fn area(&self) -> f64;
}

/// Geometry.
///
/// More.
mod geo {
    pub fn origin() -> i32 {
        0
    }
}

impl Point {
    pub fn new(x: i32) -> Self {
        Point { x }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.x)
    }
}

macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

pub fn run() {}
"#;

/// Markdown with twin headings and a fenced non-heading.
const MD: &str = r#"# Plan
Intro.

## Budget
Forty.

## Team
Three.

## Budget
Again.

# Plan
Twice.

```
# Not a heading
```
"#;

/// `MD` with a preface and a longer section.
const MD_MOVED: &str = r#"Preface.

# Plan
Intro.

## Budget
Forty.
More.


## Team
Three.

## Budget
Again.

# Plan
Twice.

```
# Not a heading
```
"#;

/// JSON deeper than three keys, an array, a key with a slash.
const JSON: &str = r#"{
  "name": "demo",
  "server": {
    "tls": {
      "cert": {
        "path": "/c"
      }
    },
    "ports": [80, 443]
  },
  "a/b": 1
}
"#;

/// TOML with a table, a deeper table and an array of tables.
const TOML: &str = r#"title = "demo"

[server]
host = "h"

[server.tls.cert]
path = "/c"

[[bin]]
name = "a"
"#;

/// Three functions of one name, two with an inner twin.
const PY_TWINS: &str = r#"def f():
    pass


def f():
    def g():
        pass


def f():
    def g():
        pass
"#;

fn every_format() -> Value {
    json!([
        {"path": "/src/store.py", "data": PY},
        {"path": "/src/lib.rs", "data": RS},
        {"path": "/docs/plan.md", "data": MD},
        {"path": "/conf/app.json", "data": JSON},
        {"path": "/conf/app.toml", "data": TOML},
        {"path": "/inbox/report.pdf", "mime": "application/pdf", "data": [37, 80, 68, 70, 0, 255], "pages": [{"part": 2, "text": "Results\nmore"}, {"part": 1, "text": "\n Title page"}]}
    ])
}

#[test]
fn every_format_names_its_items_not_their_lines() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, every_format());
        assert_eq!(
            items(&got[0]),
            [
                "class:Store (class, 5-18)",
                "class:Store/def:__init__ (def, 8-9)",
                "class:Store/def:load (def, 11-14)",
                "class:Store/def:load/def:parse (def, 12-13)",
                "class:Store/def:size (def, 17-18)",
                "def:helper (def, 21-22)",
                "def:main (def, 25-27)",
            ],
            "{cell}: python"
        );
        assert_eq!(
            items(&got[1]),
            [
                "struct:Point (struct, 3-5)",
                "enum:Shape (enum, 7-9)",
                "trait:Area (trait, 11-13)",
                "trait:Area/fn:area (fn, 12-12)",
                "mod:geo (mod, 15-19)",
                "mod:geo/fn:origin (fn, 16-18)",
                "impl:Point (impl, 21-25)",
                "impl:Point/fn:new (fn, 22-24)",
                "impl:Point+Display (impl, 27-31)",
                "impl:Point+Display/fn:fmt (fn, 28-30)",
                "macro:square (macro, 33-37)",
                "fn:run (fn, 39-39)",
            ],
            "{cell}: rust"
        );
        assert_eq!(
            items(&got[2]),
            [
                "sec:plan (sec, 1-12)",
                "sec:budget (sec, 4-6)",
                "sec:team (sec, 7-9)",
                "sec:budget-1 (sec, 10-12)",
                "sec:plan-1 (sec, 13-18)",
            ],
            "{cell}: markdown"
        );
        assert_eq!(
            items(&got[3]),
            [
                "key:/name (key, 2-2)",
                "key:/server (key, 3-10)",
                "key:/server/tls (key, 4-8)",
                "key:/server/tls/cert (key, 5-7)",
                "key:/server/ports (key, 9-9)",
                "key:/a~1b (key, 11-11)",
            ],
            "{cell}: json"
        );
        assert_eq!(
            items(&got[4]),
            [
                "key:/title (key, 1-1)",
                "key:/server (key, 3-4)",
                "key:/server/host (key, 4-4)",
                "key:/server/tls (key, 6-7)",
                "key:/server/tls/cert (key, 6-7)",
                "key:/bin (key, 9-10)",
            ],
            "{cell}: toml"
        );
        assert_eq!(
            items(&got[5]),
            ["page:1 (page, 1-1)", "page:2 (page, 2-2)"],
            "{cell}: pdf"
        );
        let parsers: Vec<&str> = got.iter().map(|r| r["parser"].as_str().unwrap()).collect();
        assert_eq!(
            parsers,
            ["ast", "scan", "markdown", "json", "toml", "pdf"],
            "{cell}"
        );
        assert!(
            got.iter().all(|r| r["mark"] == ""),
            "{cell}: a readable file carries no mark"
        );
    }
}

#[test]
fn an_edit_that_only_moves_lines_keeps_every_anchor() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/store.py", "data": PY},
                {"path": "/src/store.py", "data": PY_MOVED},
                {"path": "/src/lib.rs", "data": RS},
                {"path": "/src/lib.rs", "data": RS_MOVED},
                {"path": "/docs/plan.md", "data": MD},
                {"path": "/docs/plan.md", "data": MD_MOVED}
            ]),
        );
        for pair in got.chunks(2) {
            assert_eq!(anchors(&pair[0]), anchors(&pair[1]), "{cell}");
            assert!(!anchors(&pair[0]).is_empty(), "{cell}");
            assert_ne!(
                items(&pair[0]),
                items(&pair[1]),
                "{cell}: the lines did move"
            );
        }
    }
}

#[test]
fn a_rename_changes_exactly_one_anchor() {
    if !shipped() {
        return;
    }
    let renamed = PY.replace("def helper(x):", "def assist(x):");
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/store.py", "data": PY},
                {"path": "/src/store.py", "data": renamed.as_str()}
            ]),
        );
        let (before, after) = (anchors(&got[0]), anchors(&got[1]));
        assert_eq!(before.len(), after.len(), "{cell}");
        let changed: Vec<(&str, &str)> = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        assert_eq!(changed, [("def:helper", "def:assist")], "{cell}");
    }
}

#[test]
fn twins_in_one_file_are_numbered_in_source_order() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/twins.py", "data": PY_TWINS},
                {"path": "/docs/plan.md", "data": MD}
            ]),
        );
        assert_eq!(
            anchors(&got[0]),
            [
                "def:f",
                "def:f~2",
                "def:f~2/def:g",
                "def:f~3",
                "def:f~3/def:g"
            ],
            "{cell}"
        );
        assert_eq!(
            anchors(&got[1]),
            [
                "sec:plan",
                "sec:budget",
                "sec:team",
                "sec:budget-1",
                "sec:plan-1"
            ],
            "{cell}: markdown numbers its twins the GitHub way"
        );
    }
}

#[test]
fn an_anchor_part_escapes_what_an_address_splits_on() {
    if !shipped() {
        return;
    }
    let probe = "[anchor_part('key', '/a b#c@d%e'), anchor_part('def', 'f\\tg'), \
                 anchor_of([['class', 'A'], ['def', 'b']]), \
                 (lambda s: [unique_anchor(s, 'def:f'), unique_anchor(s, 'def:g'), \
                 unique_anchor(s, 'def:f'), unique_anchor(s, 'def:f')])({}), \
                 [md_slug(t) for t in ['Getting Started!', 'C# & .NET @ 100%', \
                 'snake_case-and-dash', '  Two  Spaces ']]]";
    for cell in CELLS {
        assert_eq!(
            pure(cell, probe, json!(null)),
            json!([
                "key:/a%20b%23c%40d%25e",
                "def:f%09g",
                "class:A/def:b",
                ["def:f", "def:g", "def:f~2", "def:f~3"],
                [
                    "getting-started",
                    "c--net--100",
                    "snake_case-and-dash",
                    "two--spaces"
                ]
            ]),
            "{cell}"
        );
    }
}

#[test]
fn a_node_carries_its_whole_record() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/a.py", "data": "class A:\n    def b(self):\n        return 1\n"},
                {"path": "/inbox/report.pdf", "mime": "application/pdf", "data": [37, 80, 68, 70],
                 "pages": [{"part": 1, "text": "\n  Title page  \nbody"}]}
            ]),
        );
        assert_eq!(
            got[0]["nodes"][1],
            json!({"anchor": "class:A/def:b", "kind": "def", "parent": "class:A", "unit": "line", "from": 2, "to": 3, "oneline": "def b(self):", "parser": "ast"}),
            "{cell}"
        );
        assert_eq!(
            got[1]["nodes"][0],
            json!({"anchor": "page:1", "kind": "page", "parent": "", "unit": "page", "from": 1, "to": 1, "oneline": "Title page", "parser": "pdf"}),
            "{cell}"
        );
        assert_eq!(got[1]["fmt"], "pdf", "{cell}");
    }
}

#[test]
fn the_marks_say_why_a_file_has_fewer_nodes() {
    if !shipped() {
        return;
    }
    let many: String = (0..6)
        .map(|i| format!("def f{i}():\n    g{i}()\n"))
        .collect();
    let big = "def a():\n    pass\n".repeat(4);
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/src/empty.py", "data": ""},
                {"path": "/conf/empty.json", "data": ""},
                {"path": "/src/big.py", "data": big, "limits": {"extract_max_bytes": 40}},
                {"path": "/src/many.py", "data": many, "limits": {"nodes_max": 4}},
                {"path": "/src/many.py", "data": many, "limits": {"links_max": 2}},
                {"path": "/src/many.py", "data": many, "limits": {"nodes_max": 6, "links_max": 6}},
                {"path": "/notes/todo.txt", "data": "def a(): pass\n"},
                {"path": "/conf/broken.json", "data": "{\"a\": }"},
                {"path": "/conf/deep.json", "data": JSON, "limits": {"key_depth": 1}},
                {"path": "/conf/deep.json", "data": JSON, "limits": {"key_depth": 4}}
            ]),
        );
        let marks: Vec<String> = got.iter().map(verdict).collect();
        assert_eq!(
            marks,
            [
                "python// 0n 0l",
                "json// 0n 0l",
                "python//too_large 0n 0l",
                "python/ast/truncated 4n 6l",
                "python/ast/truncated 6n 2l",
                "python/ast/ 6n 6l",
                "//no_extractor 0n 0l",
                "json//unparsable 0n 0l",
                "json/json/ 3n 0l",
                "json/json/ 7n 0l",
            ],
            "{cell}"
        );
        // A cap keeps the first nodes in source order.
        assert_eq!(
            items(&got[3]),
            [
                "def:f0 (def, 1-2)",
                "def:f1 (def, 3-4)",
                "def:f2 (def, 5-6)",
                "def:f3 (def, 7-8)",
            ],
            "{cell}"
        );
    }
}
