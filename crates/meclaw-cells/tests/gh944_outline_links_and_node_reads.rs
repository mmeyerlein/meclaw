//! GH #944: the node reads of a file space -- `outline`, `links`, `read` of
//! one node (`fh-<12hex>#<anchor>`) and `near` -- answered by `./read`.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): the
//! `read` script under python3, the hive's own edges under the colony's CEL,
//! the store through the store cell's own dispatcher.
//!
//! The node rows of a head (`nodes`, `links`, `node_runs`) are written here
//! the way `./derive` writes them -- by the shipped extractor itself (the
//! pure half of `./read`, word for word the twin of `./derive`'s) -- so this
//! lock does not hang on the derive job: the derive side (one bundle per head,
//! `source_changed`) is `gh944_a_head_move_emits_one_source_changed`.
//!
//! The sentences of the acceptance (plan N.6, section 3):
//!   (a) `outline` and `links` answer page by page (`cursor`/`next`), and the
//!       pages put together are the whole list;
//!   (b) `read` of `fh-...#<anchor>` answers the node's lines in the form of
//!       a read by range; an unknown anchor answers `unknown_anchor` with at
//!       most five candidates of the same name;
//!   (c) the `outline` of an older version, computed from its blocks, is the
//!       answer the space gave while that version was head (out of the rows),
//!       an import's `alias` included;
//!   (d) `near` is deterministic: Hamming over the file vectors, a tie by
//!       path, live files only, never the file itself, `[]` without a vector.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const A: &str = "fh-aaaaaaaaaaa1";

const V1: &str = "import os\nfrom json import dumps as to_json\n\n\nclass Alpha:\n    def run(self):\n        return os.getcwd()\n\n\ndef helper():\n    return to_json(1)\n";
const V2: &str = "import os\nfrom json import dumps as to_json\n\n\nclass Alpha:\n    def run(self):\n        return os.getcwd()\n\n    def stop(self):\n        pass\n\n\ndef helper():\n    return to_json(2)\n";

/// After every test: no query left its file out (bar the exempt reads) or
/// its limit off, no store op failed, and `read` cleaned its working rows up.
fn clean(s: &Space) {
    assert_eq!(
        s.unscoped(),
        Vec::<String>::new(),
        "R-FH-1 Auflage 2 at the store"
    );
    assert_eq!(s.store_errors, Vec::<String>::new(), "a store op failed");
    assert_eq!(
        s.rows("SELECT COUNT(*) FROM pending")[0][0],
        json!(0),
        "read leaves no pending row behind"
    );
}

/// What `./derive` writes for a new head: the rows of the file go, the head's
/// come in (the head only), with the file vector `fvec`.
fn index(s: &Space, file: &str, version: &str, path: &str, text: &str, fvec: &str) {
    let ex = pure(
        "read",
        "extract(ARGS['path'], 'text/plain', ARGS['text'].encode('utf-8'), None, LIMITS)",
        json!({"path": path, "text": text}),
    );
    for t in ["nodes", "links", "node_runs"] {
        s.db.execute(&format!("DELETE FROM {t} WHERE file = ?1"), [file])
            .unwrap();
    }
    let nodes = ex["nodes"].as_array().unwrap();
    for n in nodes {
        s.db.execute(
            "INSERT INTO nodes (file, version, anchor, kind, parent, unit, from_pos, to_pos, \
             oneline, parser) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                file,
                version,
                n["anchor"].as_str().unwrap(),
                n["kind"].as_str().unwrap(),
                n["parent"].as_str().unwrap(),
                n["unit"].as_str().unwrap(),
                n["from"].as_i64().unwrap(),
                n["to"].as_i64().unwrap(),
                n["oneline"].as_str().unwrap(),
                n["parser"].as_str().unwrap(),
            ],
        )
        .unwrap();
    }
    let links = ex["links"].as_array().unwrap();
    for l in links {
        s.db.execute(
            "INSERT INTO links (file, version, from_anchor, kind, target_name, pos, alias) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                file,
                version,
                l["from_anchor"].as_str().unwrap(),
                l["kind"].as_str().unwrap(),
                l["target_name"].as_str().unwrap(),
                l["pos"].as_i64().unwrap(),
                // like `./derive`: '' for a link without `as`
                l.get("alias").and_then(Value::as_str).unwrap_or(""),
            ],
        )
        .unwrap();
    }
    s.db.execute(
        "INSERT INTO node_runs (file, version, fmt, parser, mark, nodes, links, fvec) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            file,
            version,
            ex["fmt"].as_str().unwrap(),
            ex["parser"].as_str().unwrap(),
            ex["mark"].as_str().unwrap(),
            nodes.len() as i64,
            links.len() as i64,
            fvec,
        ],
    )
    .unwrap();
}

fn anchors(v: &Value) -> Vec<String> {
    v["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("no nodes: {v}"))
        .iter()
        .map(|n| n["anchor"].as_str().unwrap().to_string())
        .collect()
}

fn without_op_id(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("op_id");
    v
}

/// The selects one request sent to `table`.
fn selects_on(s: &Space, from: usize, table: &str) -> usize {
    s.store_ops[from..]
        .iter()
        .filter(|(_, a)| a["operation"] == "select" && a["table"] == table)
        .count()
}

#[test]
fn outline_and_links_answer_page_by_page() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(A, "/src/m.py", &[V1], &[]);
    index(&s, A, &v[0], "/src/m.py", V1, "");

    let all = s.read("outline", A, json!({}));
    assert_eq!(all["ok"], true, "{all}");
    assert_eq!(all["file"], A);
    assert_eq!(
        all["version"],
        json!(&v[0][..12]),
        "the version token, always"
    );
    assert_eq!(all["parser"], "ast");
    assert!(all.get("mark").is_none(), "no mark when nothing went wrong");
    assert_eq!(all["next"], "", "one page holds it all");
    assert_eq!(
        anchors(&all),
        vec!["class:Alpha", "class:Alpha/def:run", "def:helper"],
        "source order, the class before its method"
    );
    assert_eq!(
        all["nodes"][1],
        json!({"anchor": "class:Alpha/def:run", "kind": "def", "parent": "class:Alpha",
               "unit": "line", "from": 6, "to": 7, "oneline": "def run(self):",
               "parser": "ast"})
    );

    // Pages of two: `next` is the cursor of the next page, '' at the end, and
    // the pages put together are the whole list.
    let p1 = s.read("outline", A, json!({"limit": 2}));
    assert_eq!(anchors(&p1), anchors(&all)[..2].to_vec());
    assert_eq!(p1["next"], "2");
    let p2 = s.read("outline", A, json!({"limit": 2, "cursor": p1["next"]}));
    assert_eq!(anchors(&p2), anchors(&all)[2..].to_vec());
    assert_eq!(p2["next"], "");
    assert_eq!(p2["version"], json!(&v[0][..12]));
    assert_eq!(
        s.read("outline", A, json!({"cursor": "two"}))["error"]["code"],
        "bad_request",
        "a cursor is a number"
    );

    let links = s.read("links", A, json!({}));
    assert_eq!(links["version"], json!(&v[0][..12]));
    assert_eq!(
        links["links"],
        json!([
            {"kind": "import", "from_anchor": "", "target_name": "os", "pos": 1},
            {"kind": "import", "from_anchor": "", "target_name": "json:dumps", "pos": 2, "alias": "to_json"},
            {"kind": "call", "from_anchor": "class:Alpha/def:run", "target_name": "os:getcwd", "pos": 7},
            {"kind": "call", "from_anchor": "def:helper", "target_name": "to_json", "pos": 11},
        ]),
        "`alias` only on the import that has one (OR-BC-55)"
    );
    let mut paged = Vec::new();
    let mut cursor = json!("");
    for _ in 0..10 {
        let page = s.read("links", A, json!({"limit": 3, "cursor": cursor}));
        paged.extend(page["links"].as_array().unwrap().clone());
        cursor = page["next"].clone();
        if cursor == "" {
            break;
        }
    }
    assert_eq!(json!(paged), links["links"], "the pages are the whole list");
    clean(&s);
}

#[test]
fn a_node_is_read_by_its_anchor_and_an_unknown_one_names_candidates() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(A, "/src/m.py", &[V2], &[]);
    index(&s, A, &v[0], "/src/m.py", V2, "");

    // The node's lines, in the form of a read by range.
    let got = s.read("read", &format!("{A}#class:Alpha/def:stop"), json!({}));
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(got["version"], json!(&v[0][..12]));
    assert_eq!(
        (got["from"].clone(), got["to"].clone()),
        (json!(9), json!(10))
    );
    assert_eq!(
        got["text"],
        json!(format!(
            "9:{}|    def stop(self):\n10:{}|        pass",
            h4("    def stop(self):"),
            h4("        pass")
        ))
    );
    assert_eq!(
        got["text"],
        s.read("read", A, json!({"from": 9, "to": 10}))["text"],
        "the same text as the range read"
    );

    // Unknown: the anchors of the same name, a bare name finds them too.
    let got = s.read("read", &format!("{A}#def:stop"), json!({}));
    assert_eq!(got["ok"], false);
    assert_eq!(got["error"]["code"], "unknown_anchor");
    assert_eq!(got["error"]["candidates"], json!(["class:Alpha/def:stop"]));
    let got = s.read("read", &format!("{A}#nothing"), json!({}));
    assert_eq!(got["error"]["candidates"], json!([]));

    // At most five candidates, in source order; a `~n` of a duplicate is no
    // part of the name.
    let many: String = (1..=7)
        .map(|i| format!("class K{i}:\n    def run(self):\n        pass\n\n"))
        .collect::<String>()
        + "def dup():\n    pass\n\n\ndef dup():\n    pass\n";
    let b = "fh-bbbbbbbbbbb2";
    let vb = s.seed_text(b, "/many.py", &[many.as_str()], &[]);
    index(&s, b, &vb[0], "/many.py", &many, "");
    let got = s.read("read", &format!("{b}#run"), json!({}));
    assert_eq!(
        got["error"]["candidates"],
        json!(
            (1..=5)
                .map(|i| format!("class:K{i}/def:run"))
                .collect::<Vec<_>>()
        )
    );
    let got = s.read("read", &format!("{b}#def:dup~3"), json!({}));
    assert_eq!(got["error"]["candidates"], json!(["def:dup", "def:dup~2"]));
    assert_eq!(
        s.read("read", &format!("{b}#def:dup~2"), json!({}))["from"],
        json!(33)
    );

    // Only `read` takes an anchor here; a write by node is a later step.
    assert_eq!(
        s.read(
            "search",
            &format!("{A}#def:helper"),
            json!({"pattern": "x"})
        )["error"]["code"],
        "anchor_unsupported"
    );
    clean(&s);
}

#[test]
fn the_outline_of_an_older_version_is_the_answer_it_gave_as_head() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(A, "/src/m.py", &[V1], &[]);
    index(&s, A, &v[0], "/src/m.py", V1, "");
    let at = s.store_ops.len();
    let head_outline = s.read("outline", A, json!({}));
    let head_links = s.read("links", A, json!({}));
    assert_eq!(
        head_links["links"][1]["alias"], "to_json",
        "the head's rows answer the alias"
    );
    assert!(
        head_links["links"][0].get("alias").is_none(),
        "an empty alias column is no key"
    );
    assert_eq!(
        selects_on(&s, at, "blocks"),
        0,
        "the head answers out of its rows, no block is read"
    );

    // The head moves on; the derive job swaps the rows for the new head's.
    let v2 = s.seed_version(A, V2.as_bytes(), &[], &v[0]);
    s.seed_head(A, &v2, 2000, "overwrite");
    index(&s, A, &v2, "/src/m.py", V2, "");
    assert_eq!(
        s.rows(&format!(
            "SELECT COUNT(*) FROM nodes WHERE file = '{A}' AND version = '{}'",
            v[0]
        ))[0][0],
        json!(0),
        "only the head has rows"
    );
    assert!(anchors(&s.read("outline", A, json!({}))).contains(&"class:Alpha/def:stop".into()));

    let at = s.store_ops.len();
    let old = s.read("outline", A, json!({"version": &v[0][..8]}));
    assert!(
        selects_on(&s, at, "blocks") > 0,
        "an older version is computed from its blocks"
    );
    assert_eq!(
        without_op_id(old),
        without_op_id(head_outline.clone()),
        "the outline of the older version is the answer it gave as head"
    );
    let old_links = s.read("links", &format!("{A}@{}", &v[0][..8]), json!({}));
    assert_eq!(without_op_id(old_links), without_op_id(head_links));
    // A node of the older version reads from its blocks as well.
    let got = s.read("read", &format!("{A}@{}#def:helper", &v[0][..8]), json!({}));
    assert_eq!(got["version"], json!(&v[0][..12]));
    assert_eq!(
        (got["from"].clone(), got["to"].clone()),
        (json!(10), json!(11))
    );
    clean(&s);
}

#[test]
fn near_is_deterministic_over_live_heads_only() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    // Four bytes of sign bits per file: 32 bits, so a score is 1 - d / 32.
    let files: [(&str, &str, [u8; 4]); 5] = [
        (A, "/me.py", [0xff, 0, 0, 0]),
        ("fh-bbbbbbbbbbb2", "/b.py", [0xff, 0, 0, 0x0f]), // d 4
        ("fh-ccccccccccc3", "/a.py", [0xff, 0, 0, 0x0f]), // d 4, an earlier path
        ("fh-ddddddddddd4", "/d.py", [0xff, 0, 0, 0]),    // d 0, but removed
        ("fh-fffffffffff6", "/f.py", [0xff, 0xff, 0, 0]), // d 8
    ];
    for (f, path, bits) in files {
        let v = s.seed_text(f, path, &["x = 1\n"], &[]);
        index(&s, f, &v[0], path, "x = 1\n", &b64(&bits));
    }
    s.seed_tomb("fh-ddddddddddd4");
    // A file without a vector (its embedding failed) is never a neighbour.
    let e = "fh-eeeeeeeeeee5";
    let ve = s.seed_text(e, "/e.py", &["y = 2\n"], &[]);
    index(&s, e, &ve[0], "/e.py", "y = 2\n", "");

    let llm_before = s.llm.len();
    let got = s.read("near", A, json!({}));
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(got["file"], A);
    assert_eq!(
        got["near"],
        json!([
            {"file": "fh-ccccccccccc3", "path": "/a.py", "score": 0.875},
            {"file": "fh-bbbbbbbbbbb2", "path": "/b.py", "score": 0.875},
            {"file": "fh-fffffffffff6", "path": "/f.py", "score": 0.75},
        ]),
        "Hamming, a tie by path, no removed file, never itself"
    );
    assert_eq!(
        without_op_id(s.read("near", A, json!({}))),
        without_op_id(got),
        "the same answer every time"
    );
    assert_eq!(
        s.read("near", A, json!({"k": 1}))["near"],
        json!([{"file": "fh-ccccccccccc3", "path": "/a.py", "score": 0.875}])
    );
    assert_eq!(s.llm.len(), llm_before, "no model is asked");
    assert_eq!(
        s.read("near", e, json!({}))["near"],
        json!([]),
        "no vector, no neighbours"
    );
    assert_eq!(
        s.read("near", &format!("{A}@ws:draft"), json!({}))["error"]["code"],
        "bad_request",
        "near compares heads"
    );
    clean(&s);
}
