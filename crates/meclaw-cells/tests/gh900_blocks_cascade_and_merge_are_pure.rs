//! GH #900, the pure half of `./write` -- blocks, the replace cascade, the
//! line ops, patch and the three-way merge, locked as tables.
//!
//! Every value here is computed by the SHIPPED `script_inline` of
//! `templates/file-space/write`: its imports, defs and upper-case constants
//! are loaded without the I/O half (`support/file_space_hive.rs` `pure`), and
//! a small python probe over that scope comes back as JSON. Large inputs (the
//! 5 000-line corpus) are made inside the probe from a fixed seed, so the
//! table stays readable and the run stays deterministic.
//!
//! The contract: README § 2.4 (blocks: cut rule, binary blocks, the empty
//! file), README § 2.8 (the write ops, patch, merge3) and S-27-1 (the replace
//! cascade and how its count is judged).
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::{pure, shipped};
use meclaw_core::serde_json::{Value, json};

const CELL: &str = "write";

/// `expr` evaluated in the pure scope of `./write`, `ARGS` = `args`.
fn probe(expr: &str, args: Value) -> Value {
    pure(CELL, expr, args)
}

/// A block of python statements run in the pure scope; its `OUT` comes back.
fn script(src: &str) -> Value {
    pure(
        CELL,
        r#"exec(ARGS["src"], globals()) or OUT"#,
        json!({ "src": src }),
    )
}

/// 5 000 random lines from a fixed seed, with umlauts, a euro sign, an emoji
/// and a dash in the alphabet: multi-byte characters sit everywhere.
const CORPUS: &str = r#"
R = __import__("random").Random(7)
ALPHA = "abcdefghij klmnop äöüß €😀—"
L = ["%d %s\n" % (i, "".join(R.choice(ALPHA) for _ in range(R.randint(0, 60))))
     for i in range(5000)]
"#;

/// `tail` run after [`CORPUS`], its `OUT` returned.
fn on_corpus(tail: &str) -> Value {
    script(&[CORPUS, tail].concat())
}

// ------------------------------------------------------------------ blocks

/// README § 2.4: a version is its blocks; the cut is a function of the bytes
/// alone (two cuts of the same bytes are the same blocks) and loses nothing
/// (the blocks, joined, are the input), for text and binary alike.
#[test]
fn blocks_are_deterministic_and_join_back_to_the_input() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str, i64)] = &[
        ("one line", "a\n", 1),
        ("no final newline", "a", 1),
        ("unterminated last line", "a\nb", 1),
        ("three lines", "one\ntwo\nthree\n", 1),
        ("only newlines", "\n\n\n", 1),
        ("multi-byte", "grüße\n😀\n", 1),
    ];
    let expr = r#"(lambda d: [b"".join(chunk(d, True)) == d,
        [block_hash(b) for b in chunk(d, True)] == [block_hash(b) for b in chunk(d, True)],
        version_of(d) == version_of(d), len(chunk(d, True))])(ARGS.encode())"#;
    for (name, text, blocks) in cases {
        let got = probe(expr, json!(text));
        assert_eq!(
            got,
            json!([true, true, true, blocks]),
            "{name}: [joins back, same hashes twice, same version twice, block count]"
        );
    }
    let bin = probe(
        r#"(lambda d: [b"".join(chunk(d, False)) == d, chunk(d, False) == chunk(d, False),
            version_of(d) == block_hash(d)])(bytes(range(256)) * 3)"#,
        Value::Null,
    );
    assert_eq!(bin, json!([true, true, true]), "binary: joins back, stable");
    let corpus = on_corpus(
        r#"
data = "".join(L).encode()
bs = chunk(data, True)
OUT = [b"".join(bs) == data, chunk(data, True) == bs]
"#,
    );
    assert_eq!(corpus, json!([true, true]), "corpus: joins back, stable");
}

/// README § 2.4: the content-defined cut is what makes a small edit cheap --
/// one changed line, or one inserted line, touches at most two blocks of a
/// large text, so at most two new blocks are stored. 50 random samples each.
/// Lines here are short (<= 60 characters): the promise holds where the
/// 16 KiB cap cannot cut before the 256-line cap; past it the chain
/// resynchronises instead (OR-FH-84,
/// `past_the_cap_the_blocks_resynchronise_at_the_next_boundary`).
#[test]
fn one_changed_or_inserted_line_makes_at_most_two_new_blocks() {
    if !shipped() {
        return;
    }
    let got = on_corpus(
        r#"
def fresh(lines):
    return [block_hash(b) for b in chunk("".join(lines).encode(), True)]
old = set(fresh(L))
edits, inserts = [], []
for k in range(50):
    i = R.randrange(5000)
    M = list(L)
    M[i] = "changed %d\n" % k
    edits.append(len([h for h in fresh(M) if h not in old]))
    M = list(L)
    M.insert(i, "inserted %d\n" % k)
    inserts.append(len([h for h in fresh(M) if h not in old]))
OUT = [len(old), edits, inserts]
"#,
    );
    let blocks = got[0].as_u64().expect("block count");
    assert!(
        blocks > 100,
        "the corpus must span many blocks, got {blocks}"
    );
    let samples: &[(&str, usize)] = &[("changed line", 1), ("inserted line", 2)];
    for (what, col) in samples {
        let counts = got[*col].as_array().expect("counts");
        assert_eq!(counts.len(), 50, "{what}: 50 samples");
        for (k, n) in counts.iter().enumerate() {
            let n = n.as_u64().expect("count");
            assert!(
                (1..=2).contains(&n),
                "{what}, sample {k}: {n} new blocks, the contract allows 1..=2"
            );
        }
    }
}

/// README § 2.4: without a hash boundary a text block still ends after 256
/// lines. 300 lines none of which is a boundary -> 256 + 44.
#[test]
fn a_text_block_without_a_boundary_ends_after_256_lines() {
    if !shipped() {
        return;
    }
    let got = probe(
        r#"(lambda L: [len(L), [b.count(b"\n") for b in chunk(b"".join(L), True)]])(
            [x for x in (("x%d\n" % i).encode() for i in range(400)) if not _boundary(x)][:300])"#,
        Value::Null,
    );
    assert_eq!(got, json!([300, [256, 44]]), "[lines fed, lines per block]");
}

/// README § 2.4: a block also ends with the line that brings it to 16 KiB
/// or more, and only at a line end. 40 lines of exactly 1 000 bytes, none a
/// boundary: 16 lines hold 16 000 B, the 17th reaches 17 000 B -> 17, 17, 6.
#[test]
fn a_text_block_ends_with_the_line_that_reaches_16_kib() {
    if !shipped() {
        return;
    }
    let got = probe(
        r#"(lambda L: [len(L), [len(b) for b in chunk(b"".join(L), True)],
            all(b.endswith(b"\n") for b in chunk(b"".join(L), True))])(
            [x for x in ((("y%d" % i).ljust(999, "-") + "\n").encode() for i in range(80))
             if not _boundary(x)][:40])"#,
        Value::Null,
    );
    assert_eq!(
        got,
        json!([40, [17000, 17000, 6000], true]),
        "[lines fed, bytes per block, every cut at a line end]"
    );
}

/// README § 2.4: binary content is cut in fixed 192 KiB blocks, the last one
/// holding the rest.
#[test]
fn binary_blocks_are_fixed_192_kib() {
    if !shipped() {
        return;
    }
    let cases: &[(i64, Value)] = &[
        (500_000, json!([196_608, 196_608, 106_784])),
        (196_608, json!([196_608])),
        (196_609, json!([196_608, 1])),
        (1, json!([1])),
    ];
    for (size, want) in cases {
        let got = probe(
            r#"[len(b) for b in chunk(bytes(i % 251 for i in range(ARGS)), False)]"#,
            json!(size),
        );
        assert_eq!(&got, want, "{size} bytes: block sizes");
    }
}

/// README § 2.4 / GH #900 task 1: the empty file has NO block (the one form
/// chosen), text or binary, and its version is the sha256 of nothing.
#[test]
fn an_empty_file_has_no_block_and_the_empty_hash_as_version() {
    if !shipped() {
        return;
    }
    let got = probe(
        r#"[chunk(b"", True), chunk(b"", False), version_of(b"")]"#,
        Value::Null,
    );
    assert_eq!(
        got,
        json!([
            [],
            [],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ]),
        "[text blocks, binary blocks, version]"
    );
}

/// README § 2.4: cuts fall only at line ends, so every text block decodes as
/// UTF-8 on its own -- over a corpus full of multi-byte characters.
#[test]
fn every_text_block_is_valid_utf8() {
    if !shipped() {
        return;
    }
    let got = on_corpus(
        r#"
bad = []
bs = chunk("".join(L).encode(), True)
for k, b in enumerate(bs):
    try:
        b.decode("utf-8")
    except UnicodeDecodeError:
        bad.append(k)
OUT = [len(bs) > 1, bad, max(b.count(b"\n") for b in bs) <= 256,
       max(len(b) for b in bs) < 16384 + 400]
"#,
    );
    assert_eq!(
        got,
        json!([true, [], true, true]),
        "[many blocks, blocks not UTF-8, <= 256 lines, bounded bytes]"
    );
}

// ----------------------------------------------------------------- cascade

/// S-27-1: the first stage with a hit wins (1 exact, 2 trailing whitespace,
/// 3 indentation, 4 punctuation) and is reported; on stage 3/4 `new` moves by
/// the indentation difference; blank lines in `old` match blank lines; and
/// what is written is `new` as given -- normalisation is for comparing only.
#[test]
fn the_cascade_reports_its_stage_and_writes_new_as_given() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str, &str, &str, Value, i64, &str)] = &[
        (
            "stage 1, exact",
            "alpha\nbeta\ngamma\n",
            "beta",
            "BETA",
            json!(1),
            1,
            "alpha\nBETA\ngamma\n",
        ),
        (
            "stage 2, trailing whitespace on both sides",
            "a = 1   \nb = 2\t\nc = 3\n",
            "a = 1\nb = 2  \n",
            "a = 10\nb = 20\n",
            json!(1),
            2,
            "a = 10\nb = 20\nc = 3\n",
        ),
        (
            "stage 2, blank line of spaces matches a blank line",
            "a\n  \nb\n",
            "a\n\nb\n",
            "A\n\nB\n",
            json!(1),
            2,
            "A\n\nB\n",
        ),
        (
            "stage 3, deeper indentation, multi-line new shifted",
            "def f():\n    if x:\n        y()\n    z()\n",
            "if x:\n    y()\n",
            "if x:\n    y()\n    w()\n",
            json!(1),
            3,
            "def f():\n    if x:\n        y()\n        w()\n    z()\n",
        ),
        (
            "stage 3, shift keeps the relative indentation of new",
            "class A:\n        run()\n        stop()\n",
            "  run()\n  stop()",
            "  go()\n    deeper()\n  halt()",
            json!(1),
            3,
            "class A:\n        go()\n          deeper()\n        halt()\n",
        ),
        (
            "stage 3, blank line in old matches a blank line",
            "    a\n\n    b\n",
            "a\n\nb\n",
            "A\n\nB\n",
            json!(1),
            3,
            "    A\n\n    B\n",
        ),
        (
            "stage 3, all, each span shifted to its own indentation",
            "   foo()\n  foo()\n",
            "    foo()\n",
            "    bar()\n",
            json!("all"),
            3,
            "   bar()\n  bar()\n",
        ),
        (
            "stage 4, quotes, dash, nbsp, ellipsis",
            "say \u{201c}hello\u{201d} \u{2014} now\u{a0}then\u{2026}\n",
            "say \"hello\" - now then...",
            "SAID",
            json!(1),
            4,
            "SAID\n",
        ),
        (
            "stage 4, typographic apostrophe",
            "it\u{2019}s\n",
            "it's",
            "it is",
            json!(1),
            4,
            "it is\n",
        ),
        (
            "stage 4, new written as given (not folded)",
            "x = \u{201c}q\u{201d}\n",
            "x = \"q\"",
            "x = \u{201c}r\u{201d}",
            json!(1),
            4,
            "x = \u{201c}r\u{201d}\n",
        ),
        (
            "expected all replaces every hit",
            "x\ny\nx\ny\nx\n",
            "x",
            "X",
            json!("all"),
            1,
            "X\ny\nX\ny\nX\n",
        ),
        (
            "expected n = the exact count",
            "x\ny\nx\ny\nx\n",
            "x",
            "X",
            json!(3),
            1,
            "X\ny\nX\ny\nX\n",
        ),
    ];
    let expr = r#"(lambda f: [f.get("stage"),
        apply_cascade(ARGS["t"], f, ARGS["o"], ARGS["n"]) if "spans" in f else f])(
        cascade_find(ARGS["t"], ARGS["o"], ARGS["e"]))"#;
    for (name, text, old, new, expected, stage, result) in cases {
        let got = probe(
            expr,
            json!({ "t": text, "o": old, "n": new, "e": expected }),
        );
        assert_eq!(got, json!([stage, result]), "{name}: [stage, text]");
    }
}

/// S-27-1: the count is judged on the stage that matched -- `expected` 1 =
/// exactly one, `all` = at least one, n = exactly n, else `ambiguous` with the
/// count and the lines; an exact single hit wins even where stage 3 would see
/// two; nothing anywhere is `not_found`; an empty `old` is `bad_request`.
#[test]
fn the_cascade_counts_on_the_stage_that_matched() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str, &str, Value, Value)] = &[
        (
            "three hits, one expected",
            "x\ny\nx\ny\nx\n",
            "x",
            json!(1),
            json!({"error": "ambiguous", "stage": 1, "count": 3, "lines": [1, 3, 5]}),
        ),
        (
            "three hits, two expected",
            "x\ny\nx\ny\nx\n",
            "x",
            json!(2),
            json!({"error": "ambiguous", "stage": 1, "count": 3, "lines": [1, 3, 5]}),
        ),
        (
            "three hits, three expected",
            "x\ny\nx\ny\nx\n",
            "x",
            json!(3),
            json!({"error": null, "stage": 1, "count": 3, "lines": [1, 3, 5]}),
        ),
        (
            "three hits, all",
            "x\ny\nx\ny\nx\n",
            "x",
            json!("all"),
            json!({"error": null, "stage": 1, "count": 3, "lines": [1, 3, 5]}),
        ),
        (
            "exact single hit wins over two on stage 3",
            "    foo()\n  foo()\n",
            "    foo()\n",
            json!(1),
            json!({"error": null, "stage": 1, "count": 1, "lines": [1]}),
        ),
        (
            "no exact hit, two on stage 3",
            "   foo()\n  foo()\n",
            "    foo()\n",
            json!(1),
            json!({"error": "ambiguous", "stage": 3, "count": 2, "lines": [1, 2]}),
        ),
        (
            "not found on any stage",
            "abc\n",
            "zzz",
            json!(1),
            json!({"error": "not_found", "stage": null, "count": 0, "lines": []}),
        ),
        (
            "empty old",
            "abc",
            "",
            json!(1),
            json!({"error": "bad_request", "stage": null, "count": 0, "lines": []}),
        ),
    ];
    let expr = r#"(lambda f: {k: f.get(k) for k in ("error", "stage", "count", "lines")})(
        cascade_find(ARGS["t"], ARGS["o"], ARGS["e"]))"#;
    for (name, text, old, expected, want) in cases {
        let got = probe(expr, json!({ "t": text, "o": old, "e": expected }));
        assert_eq!(&got, want, "{name}");
    }
}

// ---------------------------------------------------------------- line ops

/// README § 2.1/2.5: `h4` is the first four hex digits of the sha256 of a
/// line without its ending -- `\n` and `\r\n` give the same hash as none.
#[test]
fn line_hash_is_four_hex_of_the_line_without_its_end() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str)] = &[
        ("two", "3fc4"),
        ("two\n", "3fc4"),
        ("two\r\n", "3fc4"),
        ("", "e3b0"),
        ("grüße", "8285"),
    ];
    for (line, want) in cases {
        let got = probe("line_hash(ARGS)", json!(line));
        assert_eq!(got, json!(want), "line_hash({line:?})");
    }
}

const FOUR: &str = "one\ntwo\nthree\nfour\n";

/// `(name, from, to, hashes, new, answer)` of one `replace_lines` row.
type ReplaceLinesCase = (
    &'static str,
    i64,
    i64,
    &'static [&'static str],
    &'static str,
    Value,
);

/// README § 2.8: `replace_lines` writes only while every `h4` still names its
/// line; a stale hash answers with the CURRENT lines in the read form
/// `n:h4|text`, a range outside the file with the line count.
#[test]
fn replace_lines_checks_its_hashes_and_its_range() {
    if !shipped() {
        return;
    }
    // h4: one 7692, two 3fc4, three 8b5b, four 04ef
    let cases: &[ReplaceLinesCase] = &[
        (
            "two lines by one",
            2,
            3,
            &["3fc4", "8b5b"],
            "TWO\n",
            json!({"text": "one\nTWO\nfour\n"}),
        ),
        (
            "one line by two",
            1,
            1,
            &["7692"],
            "a\nb\n",
            json!({"text": "a\nb\ntwo\nthree\nfour\n"}),
        ),
        (
            "last line by nothing",
            4,
            4,
            &["04ef"],
            "",
            json!({"text": "one\ntwo\nthree\n"}),
        ),
        (
            "one stale hash",
            2,
            3,
            &["3fc4", "0000"],
            "x\n",
            json!({"error": "stale_lines", "lines": ["2:3fc4|two", "3:8b5b|three"]}),
        ),
        (
            "from 0",
            0,
            1,
            &[],
            "x",
            json!({"error": "out_of_range", "lines": 4}),
        ),
        (
            "to past the end",
            3,
            5,
            &[],
            "x",
            json!({"error": "out_of_range", "lines": 4}),
        ),
        (
            "to before from",
            3,
            2,
            &[],
            "x",
            json!({"error": "out_of_range", "lines": 4}),
        ),
    ];
    let expr = r#"apply_replace_lines(ARGS["t"], ARGS["f"], ARGS["to"], ARGS["h"], ARGS["n"])"#;
    for (name, from, to, hashes, new, want) in cases {
        let got = probe(
            expr,
            json!({ "t": FOUR, "f": from, "to": to, "h": hashes, "n": new }),
        );
        assert_eq!(&got, want, "{name}");
    }
}

/// README § 2.8: `insert` puts its text before or after a line of the base;
/// after 0 is the top, after the last line (or before n+1) the end; anything
/// else is out of range.
#[test]
fn insert_goes_before_after_to_the_top_and_to_the_end() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str, i64, &str, &str, Value)] = &[
        (
            "before 2",
            FOUR,
            2,
            "before",
            "X",
            json!({"text": "one\nX\ntwo\nthree\nfour\n"}),
        ),
        (
            "after 2",
            FOUR,
            2,
            "after",
            "X\n",
            json!({"text": "one\ntwo\nX\nthree\nfour\n"}),
        ),
        (
            "after 0 = top",
            FOUR,
            0,
            "after",
            "TOP",
            json!({"text": "TOP\none\ntwo\nthree\nfour\n"}),
        ),
        (
            "after last = end",
            FOUR,
            4,
            "after",
            "END",
            json!({"text": "one\ntwo\nthree\nfour\nEND\n"}),
        ),
        (
            "before n+1 = end",
            FOUR,
            5,
            "before",
            "END",
            json!({"text": "one\ntwo\nthree\nfour\nEND\n"}),
        ),
        (
            "into an empty file",
            "",
            0,
            "after",
            "first",
            json!({"text": "first\n"}),
        ),
        (
            "after n+1",
            FOUR,
            5,
            "after",
            "X",
            json!({"error": "out_of_range", "lines": 4}),
        ),
        (
            "before 0",
            FOUR,
            0,
            "before",
            "X",
            json!({"error": "out_of_range", "lines": 4}),
        ),
        (
            "unknown where",
            FOUR,
            1,
            "middle",
            "X",
            json!({"error": "out_of_range", "lines": 4}),
        ),
    ];
    let expr = r#"apply_insert(ARGS["t"], ARGS["l"], ARGS["w"], ARGS["n"])"#;
    for (name, text, line, place, new, want) in cases {
        let got = probe(expr, json!({ "t": text, "l": line, "w": place, "n": new }));
        assert_eq!(&got, want, "{name}");
    }
}

/// README § 2.8: `delete` removes lines `from..to` inclusive; the bounds are
/// 1..=n with from <= to.
#[test]
fn delete_keeps_to_its_bounds() {
    if !shipped() {
        return;
    }
    let cases: &[(i64, i64, Value)] = &[
        (1, 1, json!({"text": "two\nthree\nfour\n"})),
        (4, 4, json!({"text": "one\ntwo\nthree\n"})),
        (2, 3, json!({"text": "one\nfour\n"})),
        (1, 4, json!({"text": ""})),
        (0, 1, json!({"error": "out_of_range", "lines": 4})),
        (4, 5, json!({"error": "out_of_range", "lines": 4})),
        (3, 2, json!({"error": "out_of_range", "lines": 4})),
    ];
    for (from, to, want) in cases {
        let got = probe(
            r#"apply_delete(ARGS["t"], ARGS["f"], ARGS["to"])"#,
            json!({ "t": FOUR, "f": from, "to": to }),
        );
        assert_eq!(&got, want, "delete {from}..{to}");
    }
}

// ------------------------------------------------------------------- patch

/// README § 2.8: a hunk applies at its stated line, or -- found exactly once
/// -- up to 50 lines off; hunks apply in order and a later hunk is looked for
/// after the offset the earlier one was found at.
#[test]
fn a_patch_applies_exactly_off_by_some_lines_and_in_order() {
    if !shipped() {
        return;
    }
    let twenty: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let fixed = |edits: &[(usize, &str)]| -> String {
        (1..=20)
            .map(|i| match edits.iter().find(|(n, _)| *n == i) {
                Some((_, s)) => s.to_string(),
                None => format!("line {i}\n"),
            })
            .collect()
    };
    let cases: &[(&str, &str, String)] = &[
        (
            "exact, with headers",
            "--- a\n+++ b\n@@ -4,3 +4,3 @@\n line 4\n-line 5\n+LINE 5\n line 6\n",
            fixed(&[(5, "LINE 5\n")]),
        ),
        (
            "stated ten lines late, found once",
            "@@ -14,3 +14,3 @@\n line 4\n-line 5\n+LINE 5\n line 6\n",
            fixed(&[(5, "LINE 5\n")]),
        ),
        (
            "two hunks",
            concat!(
                "@@ -2,2 +2,4 @@\n line 2\n+new a\n+new b\n line 3\n",
                "@@ -10,3 +10,3 @@\n line 10\n-line 11\n+LINE 11\n line 12\n"
            ),
            fixed(&[(2, "line 2\nnew a\nnew b\n"), (11, "LINE 11\n")]),
        ),
    ];
    for (name, diff, want) in cases {
        let got = probe(
            r#"apply_patch(ARGS["t"], ARGS["d"])"#,
            json!({ "t": twenty, "d": diff }),
        );
        assert_eq!(got, json!({ "text": want }), "{name}");
    }

    // The offset carries: hunk 2 stated 40 lines early is ambiguous alone
    // (its context stands at lines 100 and 160), and exact once hunk 1 was
    // found 40 lines late.
    let got = script(
        r#"
L = ["l%d" % i for i in range(1, 201)]
L[99:101] = ["dupa", "dupb"]
L[159:161] = ["dupa", "dupb"]
t = "\n".join(L) + "\n"
h2 = "@@ -120,2 +120,2 @@\n dupa\n-dupb\n+DUPB\n"
r = apply_patch(t, "@@ -10,1 +10,1 @@\n-l50\n+L50\n" + h2)
OUT = [[split_lines(r["text"])[0][k] for k in (49, 100, 160)], apply_patch(t, h2)]
"#,
    );
    assert_eq!(
        got,
        json!([
            ["L50", "dupb", "DUPB"],
            {"error": "patch_failed", "hunk": 1, "message": "context matches 2 places"}
        ]),
        "[lines 50/101/161 after both hunks, hunk 2 alone]"
    );

    // Slack is 50 lines: 50 off is found, 95 off is not.
    let far = script(
        r#"
t = "".join("l%d\n" % i for i in range(1, 121))
OUT = [apply_patch(t, "@@ -55,1 +55,1 @@\n-l5\n+L5\n").get("text", "").split("\n")[4],
       apply_patch(t, "@@ -100,1 +100,1 @@\n-l5\n+L5\n")]
"#,
    );
    assert_eq!(
        far,
        json!([
            "L5",
            {"error": "patch_failed", "hunk": 1, "message": "context not found near line 100"}
        ]),
        "[50 lines off, 95 lines off]"
    );
}

/// README § 2.8: a patch is all or nothing -- a failing hunk is named
/// (1-based) and no text comes back; ambiguous context fails too.
#[test]
fn a_failing_patch_names_its_hunk_and_returns_no_text() {
    if !shipped() {
        return;
    }
    let twenty: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let cases: &[(&str, &str, &str, Value)] = &[
        (
            "hunk 1 fine, hunk 2 not found",
            twenty.as_str(),
            "@@ -2,2 +2,2 @@\n line 2\n-line 3\n+L3\n@@ -10,2 +10,2 @@\n nope\n-line 11\n+x\n",
            json!({"error": "patch_failed", "hunk": 2,
                   "message": "context not found near line 10"}),
        ),
        (
            "context found twice",
            "a\nx\nb\na\nx\nb\n",
            "@@ -3,3 +3,3 @@\n a\n-x\n+Y\n b\n",
            json!({"error": "patch_failed", "hunk": 1, "message": "context matches 2 places"}),
        ),
        (
            "no hunk at all",
            twenty.as_str(),
            "garbage\n",
            json!({"error": "patch_failed", "hunk": 0, "message": "no hunk in the diff"}),
        ),
    ];
    for (name, text, diff, want) in cases {
        let got = probe(
            r#"apply_patch(ARGS["t"], ARGS["d"])"#,
            json!({ "t": text, "d": diff }),
        );
        assert_eq!(&got, want, "{name}");
        assert!(
            got.get("text").is_none(),
            "{name}: a failed patch has no text"
        );
    }
}

// ------------------------------------------------------- merge3 and unified

/// README § 2.8: `merge3` takes hunks that do not overlap from both sides,
/// the same change once, a change of one side alone as it is; an overlap is
/// a conflict written with the four marks in order, and `from`/`to` are the
/// lines of the marked region in the result.
#[test]
fn merge3_takes_separate_hunks_and_marks_an_overlap() {
    if !shipped() {
        return;
    }
    let base = "a\nb\nc\nd\ne\nf\ng\n";
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "separate hunks",
            "A\nb\nc\nd\ne\nf\ng\n",
            "a\nb\nc\nd\ne\nf\nG\n",
            "A\nb\nc\nd\ne\nf\nG\n",
        ),
        (
            "the same change",
            "a\nB\nc\nd\ne\nf\ng\n",
            "a\nB\nc\nd\ne\nf\ng\n",
            "a\nB\nc\nd\ne\nf\ng\n",
        ),
        (
            "ours alone",
            "a\nb\nc\nX\ne\nf\ng\n",
            base,
            "a\nb\nc\nX\ne\nf\ng\n",
        ),
        (
            "theirs alone",
            base,
            "a\nb\nc\nX\ne\nf\ng\n",
            "a\nb\nc\nX\ne\nf\ng\n",
        ),
    ];
    let expr = r#"(lambda r: [r, has_marks(r["text"])])(merge3(ARGS["b"], ARGS["o"], ARGS["t"]))"#;
    for (name, ours, theirs, want) in cases {
        let got = probe(expr, json!({ "b": base, "o": ours, "t": theirs }));
        assert_eq!(
            got,
            json!([{"text": want, "clean": true, "conflicts": []}, false]),
            "{name}: [merge, has marks]"
        );
    }

    let got = probe(
        r#"(lambda r: [r, [ln for ln in r["text"].split("\n") if ln in MARKS],
            r["text"].split("\n")[r["conflicts"][0]["from"] - 1],
            r["text"].split("\n")[r["conflicts"][0]["to"] - 1],
            has_marks(r["text"])])(merge3(ARGS["b"], ARGS["o"], ARGS["t"]))"#,
        json!({
            "b": base,
            "o": "a\nb\nOURS\nd\ne\nf\ng\n",
            "t": "a\nb\nTHEIRS\nd\ne\nf\ng\n"
        }),
    );
    assert_eq!(
        got,
        json!([
            {
                "text": concat!(
                    "a\nb\n<<<<<<< ours\nOURS\n||||||| base\nc\n=======\nTHEIRS\n",
                    ">>>>>>> theirs\nd\ne\nf\ng\n"
                ),
                "clean": false,
                "conflicts": [{"from": 3, "to": 9, "theirs_from": 3, "theirs_to": 3}]
            },
            ["<<<<<<< ours", "||||||| base", "=======", ">>>>>>> theirs"],
            "<<<<<<< ours",
            ">>>>>>> theirs",
            true
        ]),
        "overlap: [merge, marks in order, line `from`, line `to`, has marks]"
    );
}

/// README § 2.8: `has_marks` sees the two NAMED conflict marks (also with a
/// suffix) and nothing else -- `=======` alone underlines a Markdown heading
/// as often as it separates a conflict, so a file with one must not stay
/// `conflict` (review W m-3).
#[test]
fn has_marks_sees_every_mark_and_nothing_else() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, bool)] = &[
        ("a\n<<<<<<< ours\nb\n", true),
        ("=======\n", false),
        ("Title\n=======\n", false),
        ("||||||| base\n", false),
        ("a\n>>>>>>> theirs x\n", true),
        ("a == b\n", false),
        ("plain\n", false),
    ];
    for (text, want) in cases {
        let got = probe("has_marks(ARGS)", json!(text));
        assert_eq!(got, json!(want), "has_marks({text:?})");
    }
}

/// README § 2.8: `unified` writes the diff `patch` reads -- header
/// `--- base` / `+++ new`, three lines of context -- and applying it to `a`
/// gives back `b`.
#[test]
fn unified_diffs_round_trip_through_apply_patch() {
    if !shipped() {
        return;
    }
    let ten: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    let got = probe(
        r#"unified(ARGS["a"], ARGS["b"])"#,
        json!({ "a": ten, "b": ten.replace("l5\n", "L5\n") }),
    );
    assert_eq!(
        got,
        json!("--- base\n+++ new\n@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5\n+L5\n l6\n l7\n l8"),
        "unified, context 3"
    );

    let thirty: String = (0..30).map(|i| format!("l{i}\n")).collect();
    let every_ninth: String = (0..30)
        .map(|i| {
            if i % 9 == 0 {
                format!("X{i}\n")
            } else {
                format!("l{i}\n")
            }
        })
        .collect();
    let pairs: &[(&str, &str, &str)] = &[
        ("from empty", "", "x\n"),
        ("to empty", "x\ny\n", ""),
        ("a line removed", "a\nb\nc\n", "a\nc\n"),
        ("a line appended", "a\nb\nc\n", "a\nb\nc\nd\n"),
        ("four hunks", thirty.as_str(), every_ninth.as_str()),
        // lines that look like file headers inside a hunk (review W I-2)
        ("a removed `-- ` line", "x\n-- c\ny\n", "x\ny\n"),
        ("an added `++ ` line", "x\ny\n", "x\n++ c\ny\n"),
        (
            "header-like lines swapped",
            "--- a\n+++ b\n",
            "+++ b\n--- a\n",
        ),
    ];
    for (name, a, b) in pairs {
        let got = probe(
            r#"apply_patch(ARGS["a"], unified(ARGS["a"], ARGS["b"]))"#,
            json!({ "a": a, "b": b }),
        );
        assert_eq!(
            got,
            json!({ "text": b }),
            "{name}: apply_patch(a, unified(a, b))"
        );
    }
}

// ----------------------------------------------------------------- compute

/// README § 2.8: `replace_regex` counts its hits like the cascade counts --
/// 1 by default, `all` = at least one, n = exactly n, else `count_mismatch`
/// with the count; a pattern longer than 500 characters is `bad_request`.
#[test]
fn replace_regex_counts_its_hits() {
    if !shipped() {
        return;
    }
    let long = "a".repeat(501);
    let edge = "a".repeat(500);
    let cases: &[(&str, &str, Value, Value)] = &[
        (
            "default one, three hits",
            "^x",
            Value::Null,
            json!({"error": "count_mismatch", "count": 3,
                   "message": "pattern matched 3 times"}),
        ),
        ("all", "^x", json!("all"), json!({"text": "y1\ny2\ny3\n"})),
        (
            "exactly three",
            "^x",
            json!(3),
            json!({"text": "y1\ny2\ny3\n"}),
        ),
        (
            "two of three",
            "^x",
            json!(2),
            json!({"error": "count_mismatch", "count": 3,
                   "message": "pattern matched 3 times"}),
        ),
        (
            "all, no hit",
            "q",
            json!("all"),
            json!({"error": "count_mismatch", "count": 0,
                   "message": "pattern matched 0 times"}),
        ),
        (
            "501 characters",
            long.as_str(),
            Value::Null,
            json!({"error": "bad_request", "message": "pattern (<= 500 chars) and repl"}),
        ),
        (
            "500 characters is allowed",
            edge.as_str(),
            Value::Null,
            json!({"error": "count_mismatch", "count": 0,
                   "message": "pattern matched 0 times"}),
        ),
    ];
    for (name, pattern, expected, want) in cases {
        let mut args = json!({ "pattern": pattern, "repl": "y" });
        if !expected.is_null() {
            args["expected"] = expected.clone();
        }
        let got = probe(
            r#"compute("replace_regex", ARGS["a"], ARGS["t"])"#,
            json!({ "a": args, "t": "x1\nx2\nx3\n" }),
        );
        assert_eq!(&got, want, "{name}");
    }
}

/// README § 2.8: `changed_share` is the share of lines a change touches (the
/// `use_replace` hint): one line of 100 is 0.01.
#[test]
fn changed_share_is_the_share_of_touched_lines() {
    if !shipped() {
        return;
    }
    let hundred: String = (0..100).map(|i| format!("l{i}\n")).collect();
    let cases: &[(&str, &str, String, f64)] = &[
        (
            "one of 100",
            hundred.as_str(),
            hundred.replace("l42\n", "X\n"),
            0.01,
        ),
        ("unchanged", "a\n", "a\n".to_string(), 0.0),
        ("both empty", "", String::new(), 0.0),
        ("all of one", "a\n", "b\n".to_string(), 1.0),
    ];
    for (name, a, b, want) in cases {
        let got = probe(
            r#"changed_share(ARGS["a"], ARGS["b"])"#,
            json!({ "a": a, "b": b }),
        );
        let got = got.as_f64().expect("a number");
        assert!((got - want).abs() < 1e-12, "{name}: {got} != {want}");
    }
}

/// README § 2.9 `patch`: a context line matched loosely (trailing whitespace,
/// cascade stage 2) is the FILE's line in the result -- the diff's copy of it
/// would strip whitespace off a line the patch never meant to touch.
#[test]
fn a_loose_context_line_keeps_the_files_own_text() {
    if !shipped() {
        return;
    }
    let got = probe(
        r#"apply_patch(ARGS["t"], ARGS["d"])"#,
        json!({"t": "a  \nb\n", "d": "@@ -1,2 +1,2 @@\n a\n-b\n+B\n"}),
    );
    assert_eq!(
        got,
        json!({"text": "a  \nB\n"}),
        "the untouched line keeps its spaces"
    );
}

/// Review W I-2: a hunk's body is read by the counts of its head. While lines
/// are owed, `--- x` is the removed line `-- x` and `+++ x` the added `++ x`
/// (SQL and Lua comments), and an empty line is an empty context line; the
/// prefix guess dropped them and the patch was a silent no-op. Headers before
/// the first hunk stay optional.
#[test]
fn a_hunk_is_read_by_the_counts_of_its_head() {
    if !shipped() {
        return;
    }
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "a removed `-- note`",
            "a\n-- note\nb\nc\n",
            "@@ -1,4 +1,3 @@\n a\n--- note\n b\n c\n",
            "a\nb\nc\n",
        ),
        (
            "an added `++ x`",
            "a\nb\n",
            "@@ -1,2 +1,3 @@\n a\n+++ x\n b\n",
            "a\n++ x\nb\n",
        ),
        (
            "file headers before the hunk",
            "a\nb\n",
            "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n",
            "a\nB\n",
        ),
        (
            "an empty context line",
            "a\n\nb\n",
            "@@ -1,3 +1,3 @@\n a\n\n-b\n+B\n",
            "a\n\nB\n",
        ),
        (
            "counts omitted mean one",
            "a\n",
            "@@ -1 +1 @@\n-a\n+A\n",
            "A\n",
        ),
    ];
    for (name, text, diff, want) in cases {
        let got = probe(
            r#"apply_patch(ARGS["t"], ARGS["d"])"#,
            json!({"t": text, "d": diff}),
        );
        assert_eq!(got, json!({ "text": want }), "{name}");
    }
}

/// OR-FH-84 (review W I-4): the 16 KiB cap counts from the block's start, so
/// it is no content boundary -- a line that grows or shrinks moves every
/// capped cut up to the next line whose hash is a boundary. What holds for
/// ANY line length is the resynchronisation: every new block lies between
/// the block of the edited line and the next boundary line at or after it;
/// the blocks before and behind are shared. Measured on this corpus (50
/// changed + 50 inserted lines each, most new blocks of one edit): lines of
/// 0..60 B -> 1, 300..600 B -> 3, 1 200..1 600 B -> 10; fixed 600 B lines
/// reached 5 on another seed. `<= 2` is only promised where the byte cap
/// cannot come before the line cap (lines under 64 B = 16 KiB / 256 lines),
/// locked by `one_changed_or_inserted_line_makes_at_most_two_new_blocks`.
#[test]
fn past_the_cap_the_blocks_resynchronise_at_the_next_boundary() {
    if !shipped() {
        return;
    }
    let got = script(
        r#"
R = __import__("random").Random(7)
A = "abcdefghij klmnop"
OUT = []
for lo, hi in ((0, 60), (300, 600), (1200, 1600)):
    L = ["%d %s\n" % (i, "".join(R.choice(A) for _ in range(R.randint(lo, hi))))
         for i in range(2000)]
    old = set(block_hash(b) for b in chunk("".join(L).encode(), True))
    stray, most = 0, 0
    for k in range(50):
        i = R.randrange(2000)
        for M in ([*L[:i], "changed %d\n" % k, *L[i + 1:]], [*L[:i], "inserted %d\n" % k, *L[i:]]):
            blocks = chunk("".join(M).encode(), True)
            j = next((x for x in range(i, len(M)) if _boundary(M[x].encode())), len(M) - 1)
            lo_b, hi_b = len("".join(M[:i]).encode()), len("".join(M[:j + 1]).encode())
            pos, new = 0, 0
            for b in blocks:
                if block_hash(b) not in old:
                    new += 1
                    if pos + len(b) <= lo_b or pos >= hi_b:
                        stray += 1
                pos += len(b)
            most = max(most, new)
    OUT.append([hi, len(old), stray, most])
"#,
    );
    let rows = got.as_array().expect("one row per line length");
    assert_eq!(rows.len(), 3, "{got}");
    for r in rows {
        let (hi, blocks, stray, most) =
            (r[0].as_u64(), r[1].as_u64(), r[2].as_u64(), r[3].as_u64());
        assert!(
            blocks.unwrap_or(0) > 30,
            "lines up to {hi:?} B: many blocks: {r}"
        );
        assert_eq!(
            stray,
            Some(0),
            "lines up to {hi:?} B: a new block outside the edited block .. next boundary: {r}"
        );
        assert!(most.unwrap_or(0) >= 1, "every edit makes a block: {r}");
    }
}
