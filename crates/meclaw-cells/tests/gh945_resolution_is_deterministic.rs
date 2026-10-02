//! GH #945 -- resolution is a pure function of the edge and the indexed
//! sources, one rule per kind (graph-space README § Resolution).
//!
//! A table over the shipped `index` script's own `target_of` and `resolve`
//! (its pure half, loaded under the cell's params): python `import` (module,
//! member, submodule, package, an ambiguous and a unique dotted suffix,
//! relative), python `call` (through an import, to its own source, through an
//! alias, qualified, through a module alias, unbound), rust `use` (a `crate::`
//! path, a glob, an external crate, two crates with the same module, a
//! `mod.rs`), markdown `link` (anchor there, anchor missing, file missing, a
//! code file, a URL; a fragment slugged by the heading rule of the file
//! space) and `url`. The same table run with the sources in reverse
//! order answers the same -- an order of rows is not an input.
//!
//! The module rule (python path without `.py`, `__init__` = package; rust
//! path under the nearest `src/` as `crate::a::b`) is pinned on its own.
//! No colony boots. Guarded like every template-reading test (GH #49).

#[path = "support/graph_space_pure.rs"]
mod pure;

use meclaw_core::serde_json::{Value, json};

const PROBE: &str = r#"
def world_of(spec):
    sources, nodes = [], {}
    for s in spec:
        lang = lang_of("", s["path"])
        sources.append({"source": s["id"], "path": s["path"], "lang": lang,
                        "module": module_of(s["path"], lang), "version": "v1", "tomb": ""})
        nodes[s["id"]] = s.get("nodes", [])
    return sources, nodes


def run_cases(spec, cases, reverse=False):
    sources, nodes = world_of(spec)
    if reverse:
        sources = list(reversed(sources))
    by = {s["source"]: s for s in sources}
    out = []
    for c in cases:
        src = by[c["from"]]
        e = {"kind": c["kind"], "target_name": c["target"], "alias": c.get("alias", ""),
             "from_anchor": ""}
        links = clean_links(c.get("imports", []) + [e])
        t = target_of(clean_links([e])[0], src, links, nodes[src["source"]])
        if t["state"] == "external":
            out.append(["external", ""])
            continue
        row = {"kind": c["kind"], "lang": src["lang"], "from_source": src["source"],
               "to_module": t["to_module"], "to_member": t["to_member"]}
        r = resolve(row, sources, lambda s: nodes.get(s, []))
        out.append([r["state"], r["to_addr"]])
    return out
"#;

fn world() -> Value {
    json!([
        {"id": "fh-a00000000001", "path": "/pkg/a.py", "nodes": ["def:g"]},
        {"id": "fh-b00000000002", "path": "/pkg/b.py", "nodes": ["def:f", "class:K", "class:K/def:m"]},
        {"id": "fh-c00000000003", "path": "/pkg/__init__.py", "nodes": []},
        {"id": "fh-c00000000004", "path": "/pkg/sub/c.py", "nodes": ["def:h"]},
        {"id": "fh-d00000000005", "path": "/x/util.py", "nodes": []},
        {"id": "fh-d00000000006", "path": "/y/util.py", "nodes": []},
        {"id": "fh-d00000000007", "path": "/lib/deep/mod1.py", "nodes": []},
        {"id": "fh-e00000000008", "path": "/one/src/a/b.rs", "nodes": ["impl:C", "struct:C", "fn:x"]},
        {"id": "fh-e00000000009", "path": "/one/src/lib.rs", "nodes": []},
        {"id": "fh-e0000000000a", "path": "/one/src/m/mod.rs", "nodes": ["fn:y"]},
        {"id": "fh-e0000000000b", "path": "/two/src/a/b.rs", "nodes": ["struct:D"]},
        {"id": "fh-e0000000000c", "path": "/two/src/lib.rs", "nodes": []},
        {"id": "fh-f0000000000d", "path": "/docs/guide.md", "nodes": ["sec:setup", "sec:getting-started"]},
        {"id": "fh-f0000000000e", "path": "/docs/index.md", "nodes": []}
    ])
}

/// `(name, from, kind, target, imports of the same source, state, to_addr)`.
#[allow(clippy::type_complexity)]
fn cases() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
)> {
    let none = json!([]);
    let imp = |t: &str, alias: &str| json!([{"kind": "import", "target_name": t, "alias": alias}]);
    vec![
        (
            "module",
            "fh-a00000000001",
            "import",
            "pkg.b",
            none.clone(),
            "resolved",
            "fh-b00000000002",
        ),
        (
            "member def",
            "fh-a00000000001",
            "import",
            "pkg.b:f",
            none.clone(),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            "member class",
            "fh-a00000000001",
            "import",
            "pkg.b:K",
            none.clone(),
            "resolved",
            "fh-b00000000002#class:K",
        ),
        (
            // Never resolved is `unresolved`; `broken` is kept for an edge that
            // pointed somewhere once (OR-BC-55 (4), gh945 rename lock).
            "member never there",
            "fh-a00000000001",
            "import",
            "pkg.b:gone",
            none.clone(),
            "unresolved",
            "",
        ),
        (
            "submodule",
            "fh-a00000000001",
            "import",
            "pkg.sub:c",
            none.clone(),
            "resolved",
            "fh-c00000000004",
        ),
        (
            "package",
            "fh-a00000000001",
            "import",
            "pkg",
            none.clone(),
            "resolved",
            "fh-c00000000003",
        ),
        (
            "ambiguous suffix",
            "fh-a00000000001",
            "import",
            "util",
            none.clone(),
            "unresolved",
            "",
        ),
        (
            "unique suffix",
            "fh-a00000000001",
            "import",
            "mod1",
            none.clone(),
            "resolved",
            "fh-d00000000007",
        ),
        (
            "relative",
            "fh-c00000000004",
            "import",
            "..b:f",
            none.clone(),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            "relative sibling",
            "fh-c00000000004",
            "import",
            ".c:h",
            none.clone(),
            "resolved",
            "fh-c00000000004#def:h",
        ),
        (
            "nowhere",
            "fh-a00000000001",
            "import",
            "nowhere.at.all",
            none.clone(),
            "unresolved",
            "",
        ),
        (
            "call via import",
            "fh-a00000000001",
            "call",
            "f",
            imp("pkg.b:f", ""),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            "call to its own source",
            "fh-a00000000001",
            "call",
            "g",
            none.clone(),
            "resolved",
            "fh-a00000000001#def:g",
        ),
        // The file space's form of `from pkg.b import f as ff`: the import
        // edge carries `alias` (exact, from `ast`), the call names `ff`.
        (
            "alias call",
            "fh-a00000000001",
            "call",
            "ff",
            imp("pkg.b:f", "ff"),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            "qualified call",
            "fh-a00000000001",
            "call",
            "pkg.b:f",
            none.clone(),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            // `import pkg.b as bb` then `bb.f()`: the file space writes the
            // call against the module the alias binds, `pkg.b:f`.
            "module alias call",
            "fh-a00000000001",
            "call",
            "pkg.b:f",
            imp("pkg.b", "bb"),
            "resolved",
            "fh-b00000000002#def:f",
        ),
        (
            "unbound call",
            "fh-a00000000001",
            "call",
            "print",
            none.clone(),
            "unresolved",
            "",
        ),
        (
            "crate path",
            "fh-e00000000009",
            "import",
            "crate::a::b::C",
            none.clone(),
            "resolved",
            "fh-e00000000008#struct:C",
        ),
        (
            "glob",
            "fh-e00000000009",
            "import",
            "crate::a::b::*",
            none.clone(),
            "resolved",
            "fh-e00000000008",
        ),
        (
            "external crate",
            "fh-e00000000009",
            "import",
            "std::collections::HashMap",
            none.clone(),
            "external",
            "",
        ),
        (
            "the same crate wins",
            "fh-e0000000000c",
            "import",
            "crate::a::b::D",
            none.clone(),
            "resolved",
            "fh-e0000000000b#struct:D",
        ),
        (
            "mod.rs",
            "fh-e00000000009",
            "import",
            "crate::m::y",
            none.clone(),
            "resolved",
            "fh-e0000000000a#fn:y",
        ),
        (
            "markdown anchor",
            "fh-f0000000000e",
            "link",
            "guide.md#setup",
            none.clone(),
            "resolved",
            "fh-f0000000000d#sec:setup",
        ),
        (
            "markdown anchor slugged",
            "fh-f0000000000e",
            "link",
            "guide.md#Setup",
            none.clone(),
            "resolved",
            "fh-f0000000000d#sec:setup",
        ),
        (
            "markdown anchor percent-encoded",
            "fh-f0000000000e",
            "link",
            "guide.md#Getting%20Started",
            none.clone(),
            "resolved",
            "fh-f0000000000d#sec:getting-started",
        ),
        (
            "markdown anchor missing",
            "fh-f0000000000e",
            "link",
            "guide.md#missing",
            none.clone(),
            "broken",
            "fh-f0000000000d#sec:missing",
        ),
        (
            "markdown file missing",
            "fh-f0000000000e",
            "link",
            "missing.md",
            none.clone(),
            "unresolved",
            "",
        ),
        (
            "markdown to code",
            "fh-f0000000000e",
            "link",
            "../pkg/b.py",
            none.clone(),
            "resolved",
            "fh-b00000000002",
        ),
        (
            "markdown link to a URL",
            "fh-f0000000000e",
            "link",
            "https://example.org/x",
            none.clone(),
            "external",
            "",
        ),
        (
            "url",
            "fh-f0000000000e",
            "url",
            "https://example.org/y",
            none,
            "external",
            "",
        ),
    ]
}

fn run(reverse: bool) -> Vec<(String, String)> {
    let table: Vec<Value> = cases()
        .iter()
        .map(|(_, from, kind, target, imports, ..)| {
            json!({"from": from, "kind": kind, "target": target, "imports": imports})
        })
        .collect();
    let probe = if reverse {
        "run_cases(ARGS['world'], ARGS['cases'], True)"
    } else {
        "run_cases(ARGS['world'], ARGS['cases'])"
    };
    let got = pure::pure_with(
        "index",
        PROBE,
        probe,
        json!({"world": world(), "cases": table}),
    );
    got.as_array()
        .expect("one result per case")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                r[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn every_kind_resolves_by_its_rule() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = run(false);
    let want = cases();
    assert_eq!(got.len(), want.len());
    let wrong: Vec<String> = want
        .iter()
        .zip(&got)
        .filter(|((.., state, addr), (s, a))| s != state || a != addr)
        .map(|((name, ..), (s, a))| format!("{name}: got ({s}, {a})"))
        .collect();
    assert!(
        wrong.is_empty(),
        "resolution off its rule:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn an_order_of_rows_is_not_an_input() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    assert_eq!(
        run(false),
        run(true),
        "the same sources in reverse order resolve differently"
    );
}

#[test]
fn a_module_is_named_by_its_path() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let table = json!([
        ["/pkg/a.py", "pkg.a"],
        ["/pkg/__init__.py", "pkg"],
        ["/top.py", "top"],
        ["/one/src/a/b.rs", "crate::a::b"],
        ["/one/src/a/mod.rs", "crate::a"],
        ["/one/src/lib.rs", "crate"],
        ["/one/src/main.rs", "crate"],
        ["/x/src/y/src/z.rs", "crate::z"],
        ["/docs/guide.md", "/docs/guide.md"],
        ["/conf/app.toml", "/conf/app.toml"]
    ]);
    let got = pure::pure(
        "index",
        "[[p, module_of(p, lang_of('', p))] for p, _ in ARGS]",
        table.clone(),
    );
    assert_eq!(
        got, table,
        "the module rule of graph-space README § Resolution"
    );
    let q = pure::pure(
        "index",
        "[qualified('pkg.b', 'class:K/def:m', 'python'), qualified('crate::a', 'impl:T/fn:m', 'rust'), ident('def:f~2'), ident('sec:intro')]",
        json!(null),
    );
    assert_eq!(q, json!(["pkg.b:K.m", "crate::a::T::m", "f", "intro"]));
}

/// A link's `#frag` names the section the way the file space anchors its
/// heading (OR-BC-55 (3)): the graph's slug of a heading's text is the file
/// space's own anchor of that heading, run from both shipped scripts.
#[test]
fn a_fragment_is_slugged_by_the_rule_of_its_source() {
    if !pure::shipped() || !pure::repo("templates/file-space/read/config.json").is_file() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let heads = json!([
        "Setup",
        "Getting Started",
        "What's new?",
        "Caf\u{e9} menu",
        "a_b-c  d",
        "Version 1.2 (beta)"
    ]);
    let source = pure::pure_at(
        "templates/file-space/read/config.json",
        "",
        "[anchor_part('sec', md_slug(h)) for h in ARGS]",
        heads.clone(),
    );
    let graph = pure::pure("index", "[frag_member(h) for h in ARGS]", heads);
    assert_eq!(
        graph, source,
        "the graph slugs a fragment off its source's rule"
    );
}
