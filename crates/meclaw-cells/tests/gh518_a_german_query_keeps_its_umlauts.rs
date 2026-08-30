//! GH #518 — the recall lane's query tokenizer must cut words, not fold them.
//!
//! The defect: `tokens_of` in `templates/memory-hive/recall/config.json` split
//! on `[^0-9A-Za-zaeoeueAEOEUEss_-]+`. The tail of that class is the ASCII
//! *spelling* of the German umlauts — `ae`, `oe`, `ue`, `ss` — every letter of
//! which `A-Za-z` already covers, so the class added nothing and the real
//! characters `ä ö ü ß` landed in the SEPARATOR half. A German word was cut
//! into fragments (`Söhne` → `hne`, `Straße` → `Stra`) or, when both fragments
//! fell under the three-character floor, disappeared entirely (`Größe` → –).
//!
//! Both consumers of `tokens_of` degrade from that: the keyword leg asks FTS5
//! for a term the index does not hold, and the graph anchors — matched exactly
//! against entity names — start from a fragment that names nothing.
//!
//! The repair is the division of labour the store already documents: folding
//! belongs to ONE place, the store's FTS5 tokenizer
//! (`crates/meclaw-cells/src/store/query/fts_tokenizer.rs`, which wraps
//! `unicode61` and therefore case-folds and strips diacritics on index text and
//! query text alike). The recall lane only cuts words out of a sentence, and it
//! does that with the Unicode-aware word class — which is exactly the set the
//! broken class was reaching for, minus the ASCII restriction.
//!
//! The probe harness is deliberately duplicated from `q2_recall_query_guard.rs`
//! for the reason stated there: the REAL `params.script_inline` runs, with
//! `${VAR:-default}` resolved the way the colony resolves it at instantiation.

use std::io::Write;
use std::process::{Command, Stdio};

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the same substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Hand a probe program to python3 **on stdin**, never in argv: the probe
/// embeds the whole shipped script, and one argv string is capped at 128 KiB.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Runs the module body against an empty stdin (it parks) and then evaluates
/// `probe` against the module globals.
fn run_probe(probe: &str) -> String {
    let stdin = serde_json::json!({"envelope": {}, "body": {}, "params": {}}).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        serde_json::to_string(&stdin).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The three words the defect was measured on, each a different shape of the
/// same failure: a split that leaves one usable fragment, a split that leaves
/// none, and a split that shatters a hyphenated name into two.
#[test]
fn a_german_word_survives_tokenisation_whole() {
    let probe = r#"
for w in ("Söhne", "Größe", "Straße", "Müller-Lüdenscheid",
          "Überweisung", "weiß"):
    print(w, tokens_of(w))
"#;
    assert_eq!(
        run_probe(probe),
        "Söhne ['Söhne']\n\
         Größe ['Größe']\n\
         Straße ['Straße']\n\
         Müller-Lüdenscheid ['Müller-Lüdenscheid']\n\
         Überweisung ['Überweisung']\n\
         weiß ['weiß']"
    );
}

/// The consequence the issue is about: the FTS5 expression asks for the word
/// the index holds. The store's tokenizer folds both sides (`unicode61` strips
/// the diacritic on index text and on this query text alike), so the term this
/// lane must hand over is the WORD — folding it here would fold it twice.
#[test]
fn the_keyword_leg_asks_for_the_whole_german_word() {
    let probe = r#"
print(fts_match("Wie heißen die Söhne von Marcus?"))
print(fts_match("Welche Größe hat die Wohnung?"))
"#;
    assert_eq!(
        run_probe(probe),
        "\"heißen\"* OR \"Söhne\"* OR \"Marcus\"*\n\
         \"Welche\"* OR \"Größe\"* OR \"Wohnung\"*"
    );
}

/// A query whose every word carries an umlaut used to produce an EMPTY match
/// expression — FTS5 gets `""`, the leg comes back with nothing, and nothing
/// distinguishes that from a memory which does not hold the answer.
#[test]
fn an_all_umlaut_query_is_no_longer_an_empty_match_expression() {
    let probe = r#"
m = fts_match("Größe Höhe Fläche")
print(repr(m))
print(len(tokens_of("Größe Höhe Fläche")))
"#;
    assert_eq!(
        run_probe(probe),
        "'\"Größe\"* OR \"Höhe\"* OR \"Fläche\"*'\n3"
    );
}

/// The graph anchors are matched EXACTLY against entity names, so a fragment
/// anchors nothing. After the repair the three case variants of the real word
/// are offered — and `capitalize()` on a word that already starts with an
/// umlaut is the word itself.
#[test]
fn the_graph_anchors_carry_the_real_name() {
    let probe = r#"
names = set()
for t in tokens_of("Wo wohnt Söhne Überweisung?"):
    names.update([t, t.lower(), t.capitalize()])
print(sorted(n for n in names if n))
"#;
    assert_eq!(
        run_probe(probe),
        "['Söhne', 'Wohnt', 'söhne', 'wohnt', 'Überweisung', 'überweisung']"
    );
}

/// The repair must not move an all-ASCII query by a single token: the stop-word
/// filter, the three-character floor, the hyphen and the underscore all keep
/// behaving exactly as they did.
#[test]
fn an_ascii_query_is_byte_identical_to_before() {
    let probe = r#"
print(tokens_of("Welchen Lieblingseditor nutzt alex?"))
print(tokens_of("re-entry snake_case ab abc 42 x1"))
print(fts_match("welchen Lieblingseditor nutzt alex"))
print(tokens_of(""), tokens_of("...---..."))
"#;
    assert_eq!(
        run_probe(probe),
        "['Welchen', 'Lieblingseditor', 'nutzt', 'alex']\n\
         ['re-entry', 'snake_case', 'abc']\n\
         \"welchen\"* OR \"Lieblingseditor\"* OR \"nutzt\"* OR \"alex\"*\n\
         [] ['---']"
    );
}

/// The drift lock for the sentence this repair puts on a public template
/// surface (development-rules § 2d): the README says the lane cuts words and
/// leaves the folding to the store, and the shipped script must not contain the
/// transliterated class again.
#[test]
fn the_lane_does_not_fold_and_says_so() {
    let script = recall_script();
    // The class is still NAMED in the docstring — that is the retraction the
    // repair leaves behind — so what must be gone is the SPLIT that used it.
    assert!(
        !script.contains(r#"re.split(r"[^0-9A-Za-zaeoeueAEOEUEss_-]+""#),
        "the transliterated character class is splitting queries again"
    );
    assert!(
        script.contains(r#"re.split(r"[^\w-]+", text, flags=re.UNICODE)"#),
        "the splitter must be the Unicode-aware word class"
    );
    let readme = std::fs::read_to_string("../../templates/memory-hive/README.md").expect("README");
    assert!(
        readme.contains("cuts words, it does not fold them"),
        "the README must name the division of labour this test pins"
    );
}
