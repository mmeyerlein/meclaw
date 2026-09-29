//! GH #903 -- the pure half of `file-space/derive`: how a text view is cut into
//! the sections that are embedded one by one, how a version's blocks join and
//! read as the twin `text_view`, and how the summarizer's two answers are
//! taken apart. Tables over the shipped `script_inline`, loaded via `ast`
//! (README § 3 of wave File Hive B1): no colony, no model, no endpoint.
//!
//! The road through the hive -- birth, head move, working edit, semantic
//! search, `ask` -- is `gh903_a_file_knows_itself_from_birth.rs`.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// R2b / GH #49: a tree without the cell skips. Presence only: a placeholder
/// `derive` without `sections` must fail here, not pass silently.
fn derive_shipped() -> bool {
    repo("templates/file-space/derive/config.json").is_file()
}

/// `(section, from_line, to_line)` of `sections(text, kind, ...)`.
fn cut(text: &str, kind: &str, extra: Value) -> Vec<(String, i64, i64)> {
    let got = pure(
        "derive",
        "[(s['section'], s['from_line'], s['to_line']) for s in \
          sections(ARGS['text'], ARGS['kind'], **ARGS['kw'])]",
        json!({"text": text, "kind": kind, "kw": extra}),
    );
    got.as_array()
        .expect("a list")
        .iter()
        .map(|t| {
            (
                t[0].as_str().unwrap().to_string(),
                t[1].as_i64().unwrap(),
                t[2].as_i64().unwrap(),
            )
        })
        .collect()
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn spans(v: &[(String, i64, i64)]) -> Vec<(i64, i64)> {
    v.iter().map(|(_, a, b)| (*a, *b)).collect()
}

#[test]
fn plain_text_is_cut_into_windows_of_sixty_lines_overlapping_by_ten() {
    if !derive_shipped() {
        return;
    }
    let got = cut(&numbered(150), "text", json!({}));
    assert_eq!(spans(&got), vec![(1, 60), (51, 110), (101, 150)]);
    assert_eq!(got[0].0, "lines 1-60", "a window is named by its lines");
    // A text that fits one window is one section; one line past it is two.
    assert_eq!(spans(&cut(&numbered(60), "text", json!({}))), vec![(1, 60)]);
    assert_eq!(
        spans(&cut(&numbered(61), "text", json!({}))),
        vec![(1, 60), (51, 61)]
    );
    // The knob moves the window.
    assert_eq!(
        spans(&cut(&numbered(50), "text", json!({"window": 20}))),
        vec![(1, 20), (11, 30), (21, 40), (31, 50)]
    );
}

#[test]
fn markdown_is_cut_at_its_headings_and_never_inside_a_fence() {
    if !derive_shipped() {
        return;
    }
    let md = "intro\n# Budget\nforty\n```\n# not a heading\n```\n## Team\nthree\nfour\n";
    let got = cut(md, "markdown", json!({}));
    assert_eq!(
        got,
        vec![
            ("lines 1-1".to_string(), 1, 1),
            ("Budget".to_string(), 2, 6),
            ("Team".to_string(), 7, 9),
        ]
    );
    // A heading section longer than the window is windowed under its heading.
    let long = format!("# Long\n{}", numbered(100));
    let got = cut(&long, "markdown", json!({}));
    assert_eq!(spans(&got), vec![(1, 60), (51, 101)]);
    assert!(got.iter().all(|(s, _, _)| s == "Long"), "{got:?}");
    // The same text as plain text knows no headings.
    assert_eq!(cut(md, "text", json!({})).len(), 1);
}

#[test]
fn a_page_is_a_hard_border() {
    if !derive_shipped() {
        return;
    }
    let text = format!(
        "--- page 1 ---\n{}--- page 2 ---\nsecond\n--- page 3 ---\nthird\n",
        "x\n".repeat(70)
    );
    let got = cut(&text, "pages", json!({}));
    assert_eq!(
        got,
        vec![
            ("page 1".to_string(), 1, 60),
            ("page 1".to_string(), 51, 71),
            ("page 2".to_string(), 72, 73),
            ("page 3".to_string(), 74, 75),
        ]
    );
    let heads = [1, 72, 74];
    for (_, a, b) in &got {
        assert!(
            !heads.iter().any(|h| a < h && h <= b),
            "section {a}-{b} crosses a page"
        );
    }
}

#[test]
fn a_file_never_has_more_than_four_hundred_sections() {
    if !derive_shipped() {
        return;
    }
    // 30 000 lines are 600 windows of 60: the windows grow instead, and the
    // sections still cover the whole text without a gap.
    let got = cut(&numbered(30_000), "text", json!({}));
    assert!(got.len() <= 400, "{} sections", got.len());
    assert_eq!(got.first().unwrap().1, 1);
    assert_eq!(got.last().unwrap().2, 30_000);
    for w in got.windows(2) {
        assert!(
            w[1].1 <= w[0].2 + 1,
            "a gap between {:?} and {:?}",
            w[0],
            w[1]
        );
    }
    // 1 000 headings are 1 000 borders: neighbours are joined, and every
    // section still starts at a heading.
    let md: String = (0..1000).map(|i| format!("# h{i}\nbody\n")).collect();
    let got = cut(&md, "markdown", json!({}));
    assert!(got.len() <= 400, "{} sections", got.len());
    assert!(
        got.iter().all(|(_, a, _)| a % 2 == 1),
        "a section starts off a heading"
    );
    assert_eq!(got.last().unwrap().2, 2000);
}

#[test]
fn an_empty_text_has_no_section() {
    if !derive_shipped() {
        return;
    }
    assert!(cut("", "text", json!({})).is_empty());
    assert_eq!(spans(&cut("one", "text", json!({}))), vec![(1, 1)]);
    assert_eq!(
        spans(&cut("a\nb\n", "text", json!({}))),
        vec![(1, 2)],
        "a final line end opens no line"
    );
}

#[test]
fn markdown_is_known_by_its_name_or_its_type() {
    if !derive_shipped() {
        return;
    }
    let got = pure(
        "derive",
        "[kind_of(p, m) for p, m in ARGS]",
        json!([
            ["/a/plan.md", ""],
            ["/a/x", "text/markdown"],
            ["/a/lib.rs", "text/x-rust"],
            ["/a/README.MARKDOWN", ""]
        ]),
    );
    assert_eq!(got, json!(["markdown", "markdown", "text", "markdown"]));
}

#[test]
fn derive_carries_the_three_twins_of_read() {
    if !derive_shipped() {
        return;
    }
    // C-1 of the contract review: `parse_addr`, `line_hash` and `text_view`
    // are `read`'s copies word for word, so `gh899_the_twins_parse_one_address`
    // holds derive's copies to the same table as `read`'s.
    for func in ["parse_addr", "line_hash", "text_view"] {
        let got = cells_defining(func);
        assert!(
            got.contains(&"derive".to_string()),
            "derive defines {func}: {got:?}"
        );
        assert!(
            got.contains(&"read".to_string()),
            "read defines {func}: {got:?}"
        );
    }
}

#[test]
fn the_blocks_join_into_the_bytes_of_a_version() {
    if !derive_shipped() {
        return;
    }
    let probe = "[(lambda b: None if b is None else __import__('base64').b64encode(b).decode())\
                 (assemble_blocks(h, r)) for h, r in ARGS]";
    let e = "\u{e4}\n".as_bytes();
    let cases = json!([
        // Text blocks, in the version's order (the same block may repeat).
        [["a", "b", "a"], [{"hash": "a", "enc": "utf8", "body": "x\n"},
                           {"hash": "b", "enc": "utf8", "body": "y\n"}]],
        // A block cut inside a character is b64; the joined bytes are text.
        [["c", "d"], [{"hash": "c", "enc": "b64", "body": b64(&e[..1])},
                      {"hash": "d", "enc": "b64", "body": b64(&e[1..])}]],
        // A block the store does not have: a broken version, never read around.
        [["gone"], []],
        // Broken base64 is a broken block.
        [["z"], [{"hash": "z", "enc": "b64", "body": "not base64!"}]],
        [[], []]
    ]);
    let got = pure("derive", probe, cases);
    assert_eq!(got, json!([b64(b"x\ny\nx\n"), b64(e), null, null, ""]));
}

#[test]
fn the_text_view_of_a_version_is_the_twin_s_lines() {
    if !derive_shipped() {
        return;
    }
    let probe = "[view_text(None if c is None else __import__('base64').b64decode(c), g) \
                 for c, g in ARGS]";
    let cases = json!([
        [b64(b"x\ny\n"), []],
        [b64("\u{e4}\n".as_bytes()), []],
        // A `\r` stays in the line (the twin keeps it; `read_form` drops it).
        [b64(b"a\r\nb\r\n"), []],
        // A last empty line survives the round trip through `split_lines`.
        [b64(b"a\n\n"), []],
        // Binary with pages: the pages, in part order, under their heads.
        [b64(b"%PDF\x00\xff"), [{"part": 2, "body": "two"}, {"part": 1, "body": "one\n"}]],
        // Binary without pages, and a broken version: no text view.
        [b64(b"\x00\xff"), []],
        [null, []]
    ]);
    let got = pure("derive", probe, cases);
    assert_eq!(
        got,
        json!([
            ["x\ny\n", false],
            ["\u{e4}\n", false],
            ["a\r\nb\r\n", false],
            ["a\n\n", false],
            ["--- page 1 ---\none\n--- page 2 ---\ntwo\n", true],
            [null, false],
            [null, false]
        ])
    );
}

#[test]
fn the_read_form_carries_the_hash_replace_lines_checks() {
    if !derive_shipped() {
        return;
    }
    // `read`'s `fmt_line`: the `\r` of a CRLF line is neither shown nor hashed.
    let got = pure(
        "derive",
        "[read_form(7, l) for l in ARGS]",
        json!(["forty", "", "forty\r"]),
    );
    assert_eq!(
        got,
        json!([
            format!("7:{}|forty", h4("forty")),
            format!("7:{}|", h4("")),
            format!("7:{}|forty", h4("forty"))
        ])
    );
}

#[test]
fn a_stamp_has_the_space_s_one_form() {
    if !derive_shipped() {
        return;
    }
    // I-1 of the contract review: `summaries.at` and `pending.at` in S's and
    // W's `FMT`, which `summary` orders by.
    assert_eq!(
        pure("derive", "FMT", json!(null)),
        json!("%Y-%m-%dT%H:%M:%S.%fZ")
    );
    let now = pure("derive", "now()", json!(null));
    let s = now.as_str().unwrap();
    assert_eq!(s.len(), 27, "{s}");
    assert!(
        s.ends_with('Z') && &s[10..11] == "T" && &s[19..20] == ".",
        "{s}"
    );
}

#[test]
fn the_summary_answer_is_one_line_and_a_short_one() {
    if !derive_shipped() {
        return;
    }
    let long = "w".repeat(300);
    let got = pure(
        "derive",
        "[parse_summary(t) for t in ARGS]",
        json!([
            "The plan of the file hive.\n\nIt holds the budget\nand the team.",
            "# Only a title",
            "",
            format!("{long}\n\n{}", "s".repeat(2000)),
        ]),
    );
    assert_eq!(
        got[0],
        json!([
            "The plan of the file hive.",
            "It holds the budget and the team."
        ])
    );
    assert_eq!(
        got[1],
        json!(["Only a title", "Only a title"]),
        "no short one: the line again"
    );
    assert_eq!(got[2], Value::Null, "an empty answer is no summary");
    assert_eq!(got[3][0].as_str().unwrap().chars().count(), 120);
    assert_eq!(got[3][1].as_str().unwrap().chars().count(), 1200);
}

#[test]
fn ask_cites_only_the_lines_it_was_given() {
    if !derive_shipped() {
        return;
    }
    let got = pure(
        "derive",
        "[parse_sources(a, [tuple(c) for c in ARGS['chunks']]) for a in ARGS['answers']]",
        json!({"chunks": [[1, 10], [40, 45]],
               "answers": ["Forty.\nSOURCES: 3-5, 90-99, 41",
                           "No range named.",
                           "Both.\nsources: 1\u{2013}2"]}),
    );
    assert_eq!(
        got,
        json!([
            ["Forty.", [{"from_line": 3, "to_line": 5}, {"from_line": 41, "to_line": 41}]],
            ["No range named.", [{"from_line": 1, "to_line": 10}, {"from_line": 40, "to_line": 45}]],
            ["Both.", [{"from_line": 1, "to_line": 2}]]
        ])
    );
    // The exact hit: the line holding most of the question's words, with two
    // lines around it -- unless a chosen section already covers it.
    let lines: Vec<String> = (1..=20).map(|i| format!("filler {i}")).collect();
    let mut with_hit = lines.clone();
    with_hit[11] = "The budget for travel is small".into();
    let got = pure(
        "derive",
        "[exact_hit(ARGS['q'], ARGS['lines'], [tuple(c) for c in cov]) for cov in ARGS['covered']]",
        json!({"q": "What is the travel budget?", "lines": with_hit,
               "covered": [[[1, 5]], [[10, 14]]]}),
    );
    assert_eq!(got, json!([[10, 14], null]));
}
