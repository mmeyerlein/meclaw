//! GH #899: the twins of the file space answer one table. `parse_addr`,
//! `line_hash` and `text_view` recur word for word in every code cell of
//! `templates/file-space` that needs them -- `script_inline` knows no library
//! (OR-FH-G1) -- so this lock loads EVERY cell that defines one of them (the
//! pure half, AST loader of `support/file_space_hive.rs`) and holds each copy
//! to the same table. A strand that adds a cell with a twin runs through here
//! without an edit.
//!
//! Beside the twins, the table of the pure reading functions of `./read`:
//! the range rule, the window with its limits, the search, the listing, the
//! assembly of a version from its blocks.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

fn twins(func: &str) -> Vec<String> {
    let got = cells_defining(func);
    assert!(
        got.contains(&"read".to_string()),
        "./read defines {func}: {got:?}"
    );
    got
}

#[test]
fn every_copy_of_parse_addr_parses_one_address_the_same() {
    if !shipped() {
        return;
    }
    let id = "fh-0123456789ab";
    let table: Vec<(String, Value)> = vec![
        (
            id.into(),
            json!({"file": id, "path": "", "at": "head", "ref": "", "anchor": ""}),
        ),
        (
            format!("{id}@a1b2"),
            json!({"file": id, "path": "", "at": "v", "ref": "a1b2", "anchor": ""}),
        ),
        (
            format!("{id}@{}", "f".repeat(64)),
            json!({"file": id, "path": "", "at": "v", "ref": "f".repeat(64), "anchor": ""}),
        ),
        (
            format!("{id}@ws:feature-1"),
            json!({"file": id, "path": "", "at": "ws", "ref": "feature-1", "anchor": ""}),
        ),
        (
            format!("{id}@snap:v1.0"),
            json!({"file": id, "path": "", "at": "snap", "ref": "v1.0", "anchor": ""}),
        ),
        (
            format!("{id}#intro"),
            json!({"file": id, "path": "", "at": "head", "ref": "", "anchor": "intro"}),
        ),
        (
            format!("{id}@a1b2#intro"),
            json!({"file": id, "path": "", "at": "v", "ref": "a1b2", "anchor": "intro"}),
        ),
        (
            "/a/b.md".into(),
            json!({"file": "", "path": "/a/b.md", "at": "head", "ref": "", "anchor": ""}),
        ),
        (
            "/a/b.md@abcdef".into(),
            json!({"file": "", "path": "/a/b.md", "at": "v", "ref": "abcdef", "anchor": ""}),
        ),
        (
            "/mail/a@b.txt".into(),
            json!({"file": "", "path": "/mail/a@b.txt", "at": "head", "ref": "", "anchor": ""}),
        ),
        (
            "  /x.md  ".into(),
            json!({"file": "", "path": "/x.md", "at": "head", "ref": "", "anchor": ""}),
        ),
        // Refused: no id, short or upper hex, an empty anchor, a directory,
        // a doubled slash, a bad name, an unknown suffix on an id.
        ("".into(), json!({"error": "bad_address"})),
        ("fh-0123".into(), json!({"error": "bad_address"})),
        ("fh-0123456789AB".into(), json!({"error": "bad_address"})),
        (format!("{id}@a1b"), json!({"error": "bad_address"})),
        (format!("{id}#"), json!({"error": "bad_address"})),
        ("/a/".into(), json!({"error": "bad_address"})),
        ("/".into(), json!({"error": "bad_address"})),
        ("/a//b".into(), json!({"error": "bad_address"})),
        ("a/b.md".into(), json!({"error": "bad_address"})),
        (format!("{id}@ws:bad name"), json!({"error": "bad_address"})),
        (format!("{id}@tag:x"), json!({"error": "bad_address"})),
    ];
    let inputs: Vec<Value> = table.iter().map(|(a, _)| json!(a)).collect();
    let want: Vec<Value> = table.iter().map(|(_, w)| w.clone()).collect();
    for cell in twins("parse_addr") {
        let got = pure(&cell, "[parse_addr(a) for a in ARGS]", json!(inputs));
        let got = got.as_array().unwrap();
        for (i, (addr, w)) in table.iter().enumerate() {
            assert_eq!(&got[i], w, "{cell}: parse_addr({addr:?})");
        }
        assert_eq!(got, &want);
    }
    // Not a string at all.
    for cell in twins("parse_addr") {
        let got = pure(
            &cell,
            "[parse_addr(a) for a in ARGS]",
            json!([null, 7, {"file": id}]),
        );
        assert_eq!(
            got,
            json!([{"error": "bad_address"}, {"error": "bad_address"}, {"error": "bad_address"}]),
            "{cell}"
        );
    }
}

#[test]
fn every_copy_of_line_hash_hashes_a_line_without_its_ending() {
    if !shipped() {
        return;
    }
    let lines = [
        "",
        "alpha",
        "alpha\n",
        "alpha\r\n",
        "alpha\r",
        "  indented\t",
        "ümlaut ß",
        "a\rb",
    ];
    let want: Vec<Value> = lines
        .iter()
        .map(|l| {
            let l: &str = l;
            let bare = l.strip_suffix('\n').unwrap_or(l);
            let bare = bare.strip_suffix('\r').unwrap_or(bare);
            json!(h4(bare))
        })
        .collect();
    for cell in twins("line_hash") {
        let got = pure(&cell, "[line_hash(l) for l in ARGS]", json!(lines));
        assert_eq!(got, json!(want), "{cell}");
    }
}

#[test]
fn every_copy_of_text_view_reads_text_or_pages_or_nothing() {
    if !shipped() {
        return;
    }
    let probe = "[text_view(__import__('base64').b64decode(c), d) for c, d in ARGS]";
    let pages = json!([{"part": 2, "text": "two\n"}, {"part": 1, "text": "one\nuno"}]);
    let cases = json!([
        [b64(b"a\nb\n"), []],
        [b64(b"a\nb"), []],
        [b64(b"a\r\nb\r\n"), []],
        [b64(b""), []],
        [b64(b"\n\n"), []],
        [b64(b"bin\x00ary"), pages],
        [b64(b"bin\x00ary"), []],
        [b64(&[0xff, 0xfe, b'x']), []],
        // OR-FJ.I.22 (GH #907): a PDF can be pure ASCII (no binary comment
        // line, no compressed stream -- the seam lock's `two_pages.pdf` is
        // one). Its extracted pages are its text, not its PDF source: the
        // address line counts 2 pages from them and `page: 2` must find one.
        [b64(b"%PDF-1.4\n1 0 obj\n"), pages],
    ]);
    let want = json!([
        {"kind": "text", "lines": ["a", "b"], "pages": {}},
        {"kind": "text", "lines": ["a", "b"], "pages": {}},
        {"kind": "text", "lines": ["a\r", "b\r"], "pages": {}},
        {"kind": "text", "lines": [], "pages": {}},
        {"kind": "text", "lines": ["", ""], "pages": {}},
        {"kind": "derived",
         "lines": ["--- page 1 ---", "one", "uno", "--- page 2 ---", "two"],
         "pages": {"1": [1, 3], "2": [4, 5]}},
        {"error": "no_text"},
        {"error": "no_text"},
        {"kind": "derived",
         "lines": ["--- page 1 ---", "one", "uno", "--- page 2 ---", "two"],
         "pages": {"1": [1, 3], "2": [4, 5]}},
    ]);
    for cell in twins("text_view") {
        let got = pure(&cell, probe, cases.clone());
        assert_eq!(got, want, "{cell}");
    }
}

#[test]
fn the_pure_reading_functions_of_read_hold_their_table() {
    if !shipped() {
        return;
    }
    // span: 1-based inclusive, negative from the end, no end = to the end.
    let got = pure(
        "read",
        "[list(span(a, t)) for a, t in ARGS]",
        json!([[{}, 10], [{"from": 3}, 10], [{"from": -2}, 10], [{"from": 2, "to": -2}, 10],
               [{"from": 0, "to": 99}, 10], [{"from": 12}, 10]]),
    );
    assert_eq!(
        got,
        json!([[1, 10], [3, 10], [9, 10], [2, 9], [1, 10], [12, 10]])
    );

    // window: formatted lines, cut at `max_lines`, `more` names the rest of
    // the asked range.
    let got = pure(
        "read",
        "[list(window(ARGS, a, b)) for a, b in [(1, 3), (2, 5)]]",
        json!(["x", "y\r", "z", "w", "v"]),
    );
    assert_eq!(
        got,
        json!([
            [
                format!("1:{}|x\n2:{}|y\n3:{}|z", h4("x"), h4("y"), h4("z")),
                1,
                3,
                null
            ],
            [
                format!(
                    "2:{}|y\n3:{}|z\n4:{}|w\n5:{}|v",
                    h4("y"),
                    h4("z"),
                    h4("w"),
                    h4("v")
                ),
                2,
                5,
                null
            ]
        ])
    );
    // Empty lines, so 2000 of them stay under `max_chars`.
    let many: Vec<String> = vec![String::new(); 2500];
    let got = pure("read", "list(window(ARGS, 1, len(ARGS)))[1:]", json!(many));
    assert_eq!(
        got,
        json!([1, 2000, {"from": 2001, "to": 2500}]),
        "max_lines"
    );
    let wide: Vec<String> = (1..=100).map(|_| "w".repeat(1000)).collect();
    let got = pure("read", "list(window(ARGS, 1, len(ARGS)))[1:]", json!(wide));
    assert_eq!(got, json!([1, 24, {"from": 25, "to": 100}]), "max_chars");

    // search: exact and regex, context, limit against total, long lines cut.
    let long = format!("{}needle", "x".repeat(5000));
    let got = pure(
        "read",
        "[list(search_lines(ARGS, p, m, c, l)) for p, m, c, l in \
         [('b', 'exact', 0, 20), ('^[ab]', 'regex', 1, 1), ('needle', 'exact', 0, 20), ('x', 'exact', 0, 20)]]",
        json!(["ab", "b\r", "c", long]),
    );
    assert_eq!(
        got[0],
        json!([[{"line": 1, "h4": h4("ab"), "text": "ab"},
                                {"line": 2, "h4": h4("b"), "text": "b"}], 2, false])
    );
    assert_eq!(
        got[1],
        json!([[{"line": 1, "h4": h4("ab"), "text": "ab", "before": [],
                                "after": [format!("2:{}|b", h4("b"))]}], 2, false])
    );
    assert_eq!(
        got[2],
        json!([[], 0, false]),
        "a needle past 4000 characters is not searched"
    );
    assert_eq!(got[3][1], 1);
    assert_eq!(got[3][0][0]["long_line"], true);
    assert_eq!(got[3][0][0]["text"].as_str().unwrap().len(), 4000);

    // search under `max_chars` (review S M-1): a context line is cut at 4000
    // characters (its h4 is of the whole line), and the hits stop before they
    // pass 25 000 characters -- `cut` says so; `total` still counts them all.
    let got = pure(
        "read",
        "list(search_lines(ARGS, 'hit', 'exact', 1, 20))",
        json!(["hit", long]),
    );
    assert_eq!(
        got[0][0]["after"],
        json!([format!("2:{}|{}", h4(&long), &long[..4000])])
    );
    let wide: Vec<String> = (0..20)
        .map(|i| format!("hit{i}{}", "y".repeat(3990)))
        .collect();
    let got = pure(
        "read",
        "(lambda r: [len(r[0]), r[1], r[2]])(search_lines(ARGS, 'hit', 'exact', 0, 20))",
        json!(wide),
    );
    assert_eq!(got, json!([6, 20, true]), "max_chars across hits");

    // unified under `max_chars` as well (review S M-1).
    let got = pure(
        "read",
        "(lambda r: [len(r[0]) <= 25000, r[1]])(unified([], ARGS, 'a', 'b'))",
        json!(vec!["z".repeat(4000); 10]),
    );
    assert_eq!(got, json!([true, true]), "a diff stops at max_chars");

    // history_rows (OR-FH-85): newest first, a commit once -- its smallest seq.
    let got = pure(
        "read",
        "[r['seq'] for r in history_rows(ARGS, 10)]",
        json!([{"seq": 5, "commit": "c1"}, {"seq": 4, "commit": "c1"}, {"seq": 3, "commit": ""},
               {"seq": 2, "commit": "c0"}, {"seq": 1, "commit": ""}]),
    );
    assert_eq!(got, json!([4, 3, 2, 1]));
    let got = pure(
        "read",
        "[r['seq'] for r in history_rows(ARGS, 1)]",
        json!([{"seq": 5, "commit": "c1"}, {"seq": 4, "commit": "c1"}]),
    );
    assert_eq!(got, json!([4]));

    // list_entries: files within `depth`, deeper ones counted per directory.
    let rows = json!([
        {"path": "/a/x.md", "file": "fh-1", "bytes": 1, "lines": 1},
        {"path": "/a/b/y.md", "file": "fh-2", "bytes": 2, "lines": 1},
        {"path": "/a/b/c/z.md", "file": "fh-3", "bytes": 3, "lines": 1},
        {"path": "/ab.md", "file": "fh-4", "bytes": 4, "lines": 1},
    ]);
    let got = pure("read", "list_entries(ARGS, '/a', 1)", rows.clone());
    assert_eq!(
        got,
        json!([{"path": "/a/b/", "dir": true, "files": 2},
                           {"path": "/a/x.md", "file": "fh-1", "bytes": 1, "lines": 1}])
    );
    let got = pure(
        "read",
        "[e['path'] for e in list_entries(ARGS, '/', 2)]",
        rows,
    );
    assert_eq!(got, json!(["/a/b/", "/a/x.md", "/ab.md"]));

    // assemble: blocks in list order, `b64` decoded, a missing block = None.
    let (h1, h2) = (sha256_hex(b"ab"), sha256_hex(b"\x00c"));
    let probe = "[None if (r := assemble(v, bl)) is None else r.hex() for v, bl in ARGS]";
    let blocks = json!([{"hash": &h2, "enc": "b64", "body": b64(b"\x00c")},
                        {"hash": &h1, "enc": "utf8", "body": "ab"}]);
    let got = pure(
        "read",
        probe,
        json!([
            [{"blocks": format!("[\"{h1}\", \"{h2}\", \"{h1}\"]")}, blocks.clone()],
            [{"blocks": [&h1]}, blocks.clone()],
            [{"blocks": [&h1, "0".repeat(64)]}, blocks],
        ]),
    );
    assert_eq!(got, json!(["616200636162", "6162", null]));
}
