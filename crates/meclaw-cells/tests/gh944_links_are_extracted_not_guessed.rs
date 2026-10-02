//! GH #944, R-BC-1 condition 2 -- a link is what the syntax says, never a
//! guess from a name. Python (`ast`): `import a.b` -> `a.b`, `from a import
//! c` -> `a:c`, relative imports keep their dots, a call is an edge only for
//! a bare name or a module bound by `import` (`np.zeros` -> `numpy:zeros`);
//! `obj.meth()` says nothing about `obj` and is no edge. Markdown: written
//! inline and reference links, resolved against the file's directory into
//! absolute space paths (a link above the root is no edge), external URLs as
//! `url`, nothing from a fence or inline code. Rust: only `use` (braces
//! expanded, globs kept, `as` dropped, `self` folded into its path), no call
//! edges. Data files and pages link nowhere. Repeats of one link from one
//! node are one link, at its first position. A python import names what
//! `as` binds in `alias` (OR-BC-55), exactly as `ast` reads it; an import
//! without `as` has no `alias` key, and two imports of one module under two
//! names are two links.
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

/// Every link as `kind from -> target @pos` (`-`: the file itself).
fn links(r: &Value) -> Vec<String> {
    r["links"]
        .as_array()
        .expect("links")
        .iter()
        .map(|l| {
            let from = l["from_anchor"].as_str().expect("from_anchor");
            format!(
                "{} {} -> {} @{}",
                l["kind"].as_str().expect("kind"),
                if from.is_empty() { "-" } else { from },
                l["target_name"].as_str().expect("target_name"),
                l["pos"]
            )
        })
        .collect()
}

/// Imports of every form, alias calls, method calls that are no edge.
const PY_LINKS: &str = r#"import os
import os.path as osp
import numpy as np
from json import dumps, loads as parse
from . import sibling
from ..pkg.mod import thing
import a.b


def load(path):
    data = open(path).read()
    rows = parse(data)
    rows = parse(data)
    np.zeros(3)
    osp.join(path, "x")
    a.b.run()
    rows.append(1)
    self_like = Holder()
    self_like.method()
    return dumps(rows)


class Holder:
    def method(self):
        import shutil
        shutil.copy("a", "b")
        self.other()
        return os.getcwd()
"#;

/// Relative, reference, rooted, too-far, fenced and inline-code links, URLs.
const MD_LINKS: &str = r#"See [the plan](plan.md) first.

# Setup
Install per [guide](./install/guide.md#linux) and [readme](../README.md).
The [root file](/top.md) and [too far](../../../../etc/passwd).
Back to [usage](#usage) or [ref][r].
External: https://example.org/page. Auto: <https://example.com/a>.
Also [site](https://example.net "Site") and [mail](mailto:team@example.org).
Code: `[not a link](code.md)` stays code.

[r]: refs/table.md "The table"

```
[fenced](fenced.md)
https://fenced.example.org
```

## Usage
![logo](img/logo.png)
"#;

/// `use` with braces, `self`, `as`, a glob, a leading `::`; calls.
const RS_LINKS: &str = r#"use std::collections::{HashMap, HashSet};
use crate::store::{self, Row as StoreRow};
use super::*;
use ::serde::Serialize;
pub use self::inner::Thing;

mod inner {
    use super::Thing as Alias;
    pub struct Thing;
}

fn run() {
    let m: HashMap<u8, u8> = HashMap::new();
    helper(m.len());
    store::open();
}
"#;

/// A data file: keys, no links.
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

/// A data file: keys, no links.
const TOML: &str = r#"title = "demo"

[server]
host = "h"

[server.tls.cert]
path = "/c"

[[bin]]
name = "a"
"#;

#[test]
fn python_links_are_imports_and_known_calls() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/links.py", "data": PY_LINKS}]));
        assert_eq!(
            links(&got[0]),
            [
                "import - -> os @1",
                "import - -> os.path @2",
                "import - -> numpy @3",
                "import - -> json:dumps @4",
                "import - -> json:loads @4",
                "import - -> .:sibling @5",
                "import - -> ..pkg.mod:thing @6",
                "import - -> a.b @7",
                "call def:load -> open @11",
                "call def:load -> parse @12",
                "call def:load -> numpy:zeros @14",
                "call def:load -> os.path:join @15",
                "call def:load -> a.b:run @16",
                "call def:load -> Holder @18",
                "call def:load -> dumps @20",
                "import class:Holder/def:method -> shutil @25",
                "call class:Holder/def:method -> shutil:copy @26",
                "call class:Holder/def:method -> os:getcwd @28",
            ],
            "{cell}"
        );
    }
}

/// `as` on both import forms, repeats, a call through the alias.
const PY_ALIAS: &str = r#"import a.b as m
import os
import os as o
import os as o
import os as p
from json import dumps as dd, loads


def f():
    return m.f()
"#;

#[test]
fn a_python_import_carries_its_alias() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/alias.py", "data": PY_ALIAS}]));
        assert_eq!(
            got[0]["links"],
            json!([
                {"kind": "import", "from_anchor": "", "target_name": "a.b", "pos": 1, "alias": "m"},
                {"kind": "import", "from_anchor": "", "target_name": "os", "pos": 2},
                {"kind": "import", "from_anchor": "", "target_name": "os", "pos": 3, "alias": "o"},
                {"kind": "import", "from_anchor": "", "target_name": "os", "pos": 5, "alias": "p"},
                {"kind": "import", "from_anchor": "", "target_name": "json:dumps", "pos": 6, "alias": "dd"},
                {"kind": "import", "from_anchor": "", "target_name": "json:loads", "pos": 6},
                {"kind": "call", "from_anchor": "def:f", "target_name": "a.b:f", "pos": 10}
            ]),
            "{cell}: `alias` only where `as` stands, the call through it names the module"
        );
        // Rust drops `as` (`use x as y` names `x`): no alias key there.
        let rs = run(cell, json!([{"path": "/src/links.rs", "data": RS_LINKS}]));
        assert!(
            rs[0]["links"]
                .as_array()
                .expect("links")
                .iter()
                .all(|l| l.get("alias").is_none()),
            "{cell}: rust links carry no alias"
        );
    }
}

#[test]
fn markdown_links_resolve_against_the_file() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([{"path": "/docs/guide/links.md", "data": MD_LINKS}]),
        );
        let got = links(&got[0]);
        assert_eq!(
            got,
            [
                "link - -> /docs/guide/plan.md @1",
                "link sec:setup -> /docs/guide/install/guide.md#linux @4",
                "link sec:setup -> /docs/README.md @4",
                "link sec:setup -> /top.md @5",
                "link sec:setup -> /docs/guide/links.md#usage @6",
                "url sec:setup -> https://example.org/page @7",
                "url sec:setup -> https://example.com/a @7",
                "url sec:setup -> https://example.net @8",
                "url sec:setup -> mailto:team@example.org @8",
                "link sec:setup -> /docs/guide/refs/table.md @11",
                "link sec:usage -> /docs/guide/img/logo.png @19",
            ],
            "{cell}"
        );
        assert!(
            !got.iter()
                .any(|l| l.contains("fenced") || l.contains("code.md") || l.contains("passwd")),
            "{cell}: nothing from a fence, inline code or above the root"
        );
    }
}

#[test]
fn rust_links_are_its_uses_only() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(cell, json!([{"path": "/src/links.rs", "data": RS_LINKS}]));
        assert_eq!(
            links(&got[0]),
            [
                "import - -> std::collections::HashMap @1",
                "import - -> std::collections::HashSet @1",
                "import - -> crate::store @2",
                "import - -> crate::store::Row @2",
                "import - -> super::* @3",
                "import - -> ::serde::Serialize @4",
                "import - -> self::inner::Thing @5",
                "import mod:inner -> super::Thing @8",
            ],
            "{cell}: no call is an edge"
        );
    }
}

#[test]
fn data_files_and_pages_link_nowhere() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        let got = run(
            cell,
            json!([
                {"path": "/conf/app.json", "data": JSON},
                {"path": "/conf/app.toml", "data": TOML},
                {"path": "/inbox/report.pdf", "mime": "application/pdf", "data": [37, 80, 68, 70],
                 "pages": [{"part": 2, "text": "Results\nmore"}, {"part": 1, "text": "\n Title page"}]}
            ]),
        );
        for r in &got {
            assert!(!r["nodes"].as_array().unwrap().is_empty(), "{cell}");
            assert!(links(r).is_empty(), "{cell}: {}", r["fmt"]);
        }
    }
}

#[test]
fn a_relative_link_becomes_an_absolute_space_path() {
    if !shipped() {
        return;
    }
    for cell in CELLS {
        assert_eq!(
            pure(
                cell,
                "[resolve_link(p, t) for p, t in ARGS]",
                json!([
                    ["/docs/guide/a.md", "b.md"],
                    ["/docs/guide/a.md", "./x/../c.md#top"],
                    ["/docs/guide/a.md", "../../up.md"],
                    ["/docs/guide/a.md", "../../../gone.md"],
                    ["/docs/guide/a.md", "#intro"],
                    ["/a.md", "/abs/b.md"],
                    ["/a.md", "sub/"],
                    ["/a.md", "../x.md"]
                ])
            ),
            json!([
                "/docs/guide/b.md",
                "/docs/guide/c.md#top",
                "/up.md",
                null,
                "/docs/guide/a.md#intro",
                "/abs/b.md",
                "/sub",
                null
            ]),
            "{cell}: `..` folded, `#frag` kept, above the root is no edge"
        );
    }
}
