//! GH #944, R-BC-1 condition 1 -- python has two extractors, `ast` and the
//! indentation scanner behind it, and an anchor must not depend on which one
//! read the file: the scanner is the fallback for a file `ast` refuses, and a
//! node read by its anchor must find the same item after the file is fixed.
//! For valid python both name the same items (anchor, kind, parent, first
//! line); only `ast` knows links. The register is the fallback chain, one
//! entry per format. The block is a twin: `./derive` extracts the head,
//! `./read` an older version, so the text between the marker lines is the
//! same bytes in both cells. The file vector of `near` (a bitwise majority
//! over the section bits) lives in the same block.
//!
//! Pure: the extractor block of `./derive` and `./read`, loaded via `ast`.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The two cells that carry the extractor block, a twin.
const CELLS: [&str; 2] = ["derive", "read"];

const BEGIN: &str = "# ---- extractors begin (twin of ./derive and ./read; GH #944)\n";
const END: &str = "# ---- extractors end\n";

/// Per fixture: `[ast, scan]`, each `[[anchor, kind, parent, from]..., parser,
/// link count]`.
const BOTH: &str = "[[[[n['anchor'], n['kind'], n['parent'], n['from']] for n in r['nodes']] \
    + [r['parser'], len(r['links'])] \
    for r in (extract_python_ast(a.encode(), None), extract_python_scan(a.encode(), None))] \
    for a in ARGS]";

fn block(cell: &str) -> String {
    let src = script_of(cell);
    assert_eq!(src.matches(BEGIN).count(), 1, "{cell}: one begin line");
    assert_eq!(src.matches(END).count(), 1, "{cell}: one end line");
    let a = src.find(BEGIN).unwrap();
    let b = src.find(END).unwrap();
    assert!(a < b, "{cell}: begin before end");
    src[a..b + END.len()].to_string()
}

/// A module: classes, methods, async, a nested def, a decorator.
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

/// Strings, comments and continuations that look like items.
const PY_TRICKY: &str = r#"import json
# def commented(): pass
TEXT = """
def in_a_docstring():
    pass
"""


@decorator(
    "def not_here()",
)
def decorated(a,
              b):
    s = 'class InString: pass'
    return json.dumps([a, b])


class Outer:
    class Inner:
        async def deep(self):
            pass

    def after(self):
        x = (
            1,
        )
        return x


def tail(): return 1
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

#[test]
fn ast_and_the_scanner_name_the_same_items() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = pure(cell, BOTH, json!([PY, PY_TRICKY, PY_TWINS]));
        for (k, pair) in got.as_array().unwrap().iter().enumerate() {
            let (by_ast, by_scan) = (pair[0].as_array().unwrap(), pair[1].as_array().unwrap());
            let n = by_ast.len() - 2;
            assert!(n > 0, "{cell}: fixture {k} has items");
            assert_eq!(
                by_ast[..n],
                by_scan[..by_scan.len() - 2],
                "{cell}: fixture {k}"
            );
            assert_eq!(by_ast[n], "ast", "{cell}: fixture {k}");
            assert_eq!(by_scan[n], "scan", "{cell}: fixture {k}");
            assert_eq!(by_scan[n + 1], 0, "{cell}: the scanner knows no links");
        }
        let tricky: Vec<&str> = got[1][0]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i.as_array().and_then(|a| a.first()).and_then(Value::as_str))
            .collect();
        assert_eq!(
            tricky,
            [
                "def:decorated",
                "class:Outer",
                "class:Outer/class:Inner",
                "class:Outer/class:Inner/def:deep",
                "class:Outer/def:after",
                "def:tail",
            ],
            "{cell}: no item from a string, a comment or a decorator argument"
        );
        let links: Vec<Value> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| pair[0].as_array().unwrap().last().unwrap().clone())
            .collect();
        assert_eq!(Value::Array(links), json!([5, 3, 0]), "{cell}: ast links");
    }
}

#[test]
fn the_register_is_the_fallback_chain() {
    if !shipped() {
        return;
    }
    let probe = "{k: [f.__name__ for f in v] for k, v in EXTRACTORS.items()}";
    for cell in CELLS {
        assert_eq!(
            pure(cell, probe, json!(null)),
            json!({"python": ["extract_python_ast", "extract_python_scan"], "rust": ["extract_rust_scan"], "markdown": ["extract_markdown"], "json": ["extract_json"], "toml": ["extract_toml"], "pdf": ["extract_pdf"]}),
            "{cell}"
        );
    }
}

#[test]
fn derive_and_read_carry_one_block() {
    if !shipped() {
        return;
    }
    let derive = block("derive");
    assert!(
        derive.contains("\ndef extract("),
        "the block holds the entry point"
    );
    assert!(
        derive.contains("\nEXTRACTORS = {"),
        "the block holds the register"
    );
    assert_eq!(
        derive,
        block("read"),
        "the extractor block of ./derive and ./read is one text, byte for byte"
    );
}

#[test]
fn the_file_vector_is_a_bitwise_majority() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        assert_eq!(
            pure(
                cell,
                "[file_vector(c) for c in ARGS]",
                json!([
                    ["wAE=", "gAA="],
                    ["8A==", "AA==", "wA=="],
                    [],
                    ["AQ==", "AQI="],
                    ["not base64!"]
                ])
            ),
            json!(["wAE=", "wA==", "", "", ""]),
            "{cell}: a tie is 1; nothing, unequal lengths or broken base64 is no vector"
        );
        assert_eq!(
            pure(
                cell,
                "[hamming_b64(a, b) for a, b in ARGS]",
                json!([
                    ["8A==", "Dw=="],
                    ["AQI=", "AQI="],
                    ["AQ==", "AQI="],
                    ["!", "AQ=="]
                ])
            ),
            json!([8, 0, null, null]),
            "{cell}"
        );
    }
}
