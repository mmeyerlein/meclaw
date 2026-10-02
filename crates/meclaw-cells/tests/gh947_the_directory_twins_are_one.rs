//! GH #947: the directory twins of the file space answer one table. A
//! directory is a path without a trailing `/` (the root is `/`), and three
//! cells name directories: `./write` sets `files.dir` and makes the rows of
//! `create_dir`, `./derive`'s sync counts into every ancestor, `./read`
//! answers `dir_info` and `list`. `script_inline` knows no library
//! (OR-FH-G1), so `dir_of`, `dir_norm`, `ancestors`, `dir_parent`, `dir_depth`
//! and `tags_norm` stand word for word in each of them, between the markers
//! `# ---- directories (GH #947)` and `# ---- directories end`. This lock
//! loads EVERY cell that defines `dir_of` (the pure half, AST loader of
//! `support/file_space_hive.rs`), holds each copy to one table and the three
//! texts to one text -- a cell whose copy drifts would file a directory under
//! a path the others never read.
//!
//! The same for `pct_decode`, the anchor decoding `./write` (an anchored
//! write) and `./read` share: one text, and it never raises -- a lone
//! surrogate once raised in `./read` and the request died without its answer
//! (ledger 15 of the wave).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const HEAD: &str =
    "# ---- directories (GH #947) -- word for word in ./write, ./derive and ./read\n";
const TAIL: &str = "# ---- directories end\n";

/// The cells that define `func`, at least `want`.
fn twins(func: &str, want: &[&str]) -> Vec<String> {
    let got = cells_defining(func);
    for w in want {
        assert!(
            got.contains(&w.to_string()),
            "./{w} defines {func}: {got:?}"
        );
    }
    got
}

#[test]
fn every_copy_of_the_directory_functions_answers_one_table() {
    if !shipped() {
        return;
    }
    let paths = json!([
        "/a/b/x.py",
        "/x",
        "/a/b/",
        "/",
        "",
        "a/b",
        "/a//b/c",
        "/a/b/c/d"
    ]);
    let table: [(&str, Value); 5] = [
        (
            "dir_of",
            json!(["/a/b", "/", "/a", "/", "/", "a", "/a//b", "/a/b/c"]),
        ),
        (
            "dir_norm",
            json!([
                "/a/b/x.py",
                "/x",
                "/a/b",
                "/",
                "/",
                "/a/b",
                "/a/b/c",
                "/a/b/c/d"
            ]),
        ),
        (
            "ancestors",
            json!([
                ["/", "/a", "/a/b", "/a/b/x.py"],
                ["/", "/x"],
                ["/", "/a", "/a/b"],
                ["/"],
                ["/"],
                ["/", "/a", "/a/b"],
                ["/", "/a", "/a/b", "/a/b/c"],
                ["/", "/a", "/a/b", "/a/b/c", "/a/b/c/d"]
            ]),
        ),
        (
            "dir_parent",
            json!(["/a/b", "/", "/a", "", "", "/a", "/a/b", "/a/b/c"]),
        ),
        ("dir_depth", json!([3, 1, 2, 0, 0, 2, 3, 4])),
    ];
    for (func, want) in &table {
        for cell in twins(func, &["write", "derive", "read"]) {
            let got = pure(&cell, &format!("[{func}(p) for p in ARGS]"), paths.clone());
            for (i, p) in paths.as_array().unwrap().iter().enumerate() {
                assert_eq!(got[i], want[i], "{cell}: {func}({p})");
            }
        }
    }

    // A tag list as it is stored: lower case, inner whitespace folded, cut to
    // 32 characters, no empty one, no repeat, at most 8 (the first win); a
    // JSON string of a list is read too, anything else is [].
    let long = "x".repeat(40);
    let cases = json!([
        ["Alpha", " beta  gamma ", "alpha", "", 7, long, "end.", null],
        "[\"A\", \"b\"]",
        "not json",
        null,
        5,
        {"a": 1},
        (0..12).map(|i| format!("t{i}")).collect::<Vec<_>>()
    ]);
    let want = json!([
        ["alpha", "beta gamma", "x".repeat(32), "end"],
        ["a", "b"],
        [],
        [],
        [],
        [],
        (0..8).map(|i| format!("t{i}")).collect::<Vec<_>>()
    ]);
    for cell in twins("tags_norm", &["write", "derive", "read"]) {
        assert_eq!(
            pure(&cell, "[tags_norm(v) for v in ARGS]", cases.clone()),
            want,
            "{cell}"
        );
        assert_eq!(
            pure(&cell, "[TAGS_MAX, TAG_CHARS]", json!(null)),
            json!([8, 32]),
            "{cell}"
        );
    }
}

#[test]
fn the_directory_functions_are_one_text() {
    if !shipped() {
        return;
    }
    let mut texts: Vec<(String, String)> = Vec::new();
    for cell in twins("dir_of", &["write", "derive", "read"]) {
        let src = script_of(&cell);
        assert_eq!(src.matches(HEAD).count(), 1, "{cell}: one opening marker");
        assert_eq!(src.matches(TAIL).count(), 1, "{cell}: one closing marker");
        let from = src.find(HEAD).unwrap();
        let to = src.find(TAIL).unwrap() + TAIL.len();
        assert!(from < to, "{cell}: the markers in order");
        texts.push((cell, src[from..to].to_string()));
    }
    let (first, text) = &texts[0];
    for func in [
        "dir_of",
        "dir_norm",
        "ancestors",
        "dir_parent",
        "dir_depth",
        "tags_norm",
    ] {
        assert!(
            text.contains(&format!("\ndef {func}(")),
            "{first}: {func} between the markers"
        );
    }
    for (cell, other) in &texts[1..] {
        assert_eq!(other, text, "./{cell} and ./{first}: one text");
    }
}

/// The text of the top-level `def func(` in `src`, up to the next line that
/// starts in column one, trailing blank lines cut.
fn def_text(src: &str, func: &str) -> String {
    let at = src
        .find(&format!("\ndef {func}("))
        .unwrap_or_else(|| panic!("no def {func}"))
        + 1;
    let mut lines = src[at..].lines();
    let mut out = vec![lines.next().unwrap()];
    for l in lines {
        if !l.is_empty() && !l.starts_with(char::is_whitespace) {
            break;
        }
        out.push(l);
    }
    out.join("\n").trim_end().to_string()
}

#[test]
fn pct_decode_is_one_text_and_never_raises() {
    if !shipped() {
        return;
    }
    let cells = twins("pct_decode", &["write", "read"]);
    let read = def_text(&script_of("read"), "pct_decode");
    assert!(
        read.contains("surrogatepass"),
        "a lone surrogate passes the encoding: {read}"
    );
    for cell in &cells {
        assert_eq!(
            def_text(&script_of(cell), "pct_decode"),
            read,
            "./{cell} and ./read: one text"
        );
    }
    // `\ud800` / `\udfff` in a Python literal is a lone surrogate -- the form
    // that once raised. Each decodes to replacement characters, as do the
    // percent-escaped bytes of one; a stray `%` stays as it is.
    let probe = "[pct_decode(s) for s in ['def:\\ud800x', '\\udfff', 'a%20b', 'caf%C3%A9', \
                 '%e2%82%ac', '%ED%A0%80', '%zz', '%C3']]";
    let r = "\u{fffd}";
    let want = json!([
        format!("def:{r}{r}{r}x"),
        format!("{r}{r}{r}"),
        "a b",
        "caf\u{e9}",
        "\u{20ac}",
        format!("{r}{r}{r}"),
        "%zz",
        r
    ]);
    for cell in &cells {
        assert_eq!(pure(cell, probe, json!(null)), want, "{cell}");
    }
}

/// Ledger line 15 of wave C (review I-2): `strict_json` is how `./tools`,
/// `./read` and `./write` write their emissions -- one text, and it never
/// leaves a lone surrogate's escape a strict parser refuses: the lone one
/// becomes U+FFFD, a pair stays its character, a backslash stays text.
#[test]
fn strict_json_is_one_text_and_leaves_no_lone_surrogate() {
    if !shipped() {
        return;
    }
    let cells = twins("strict_json", &["tools", "read", "write"]);
    let read = def_text(&script_of("read"), "strict_json");
    for cell in &cells {
        assert_eq!(
            def_text(&script_of(cell), "strict_json"),
            read,
            "./{cell} and ./read: one text"
        );
        let script = script_of(cell);
        let emit = def_text(&script, "emit");
        assert!(
            emit.contains("strict_json(msgs)") && !emit.contains("json.dumps"),
            "./{cell} emits through strict_json: {emit}"
        );
    }
    // `\ud800` in a Python literal is a lone surrogate; the probe returns the
    // emitted TEXT, and the test parses it strictly.
    let probe = "[strict_json([{'t': s}]) for s in ['def:\\ud800x', '\\udfff', \
                 '\\ud83d\\ude00', 'a\\\\ud800', 'plain']]";
    let r = "\u{fffd}";
    let want = [
        format!("def:{r}x"),
        r.to_string(),
        "\u{1f600}".to_string(),
        "a\\ud800".to_string(),
        "plain".to_string(),
    ];
    for cell in &cells {
        let got = pure(cell, probe, json!(null));
        let texts = got.as_array().expect("a list of texts");
        assert_eq!(texts.len(), want.len());
        for (t, w) in texts.iter().zip(&want) {
            let v: Value = meclaw_core::serde_json::from_str(t.as_str().unwrap())
                .unwrap_or_else(|e| panic!("./{cell}: no strict JSON ({e}): {t}"));
            assert_eq!(v, json!([{"t": w}]), "./{cell}");
        }
    }
}
