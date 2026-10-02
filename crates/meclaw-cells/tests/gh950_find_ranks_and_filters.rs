//! GH #950 -- `find` ranks and filters the catalog, and no question breaks it.
//!
//! `find {q, kind?, fmt?, under?, limit?, cursor?}` on the librarian's `in_lib`
//! lane searches the catalog's full-text index (path terms, name terms, the
//! one line, the tags) and answers at most 20 hits a page. These locks index
//! small corpora through the shipped `templates/librarian`, booted by itself
//! (`support/librarian_hive.rs`), the test playing the file space: each file
//! is announced, its two pulls answered and its summary line and tags
//! described on `source_described` -- never taken from `info`, which on an
//! announcement carries the head before's (GH #950, OR-BC-68):
//!
//! 1. **one splitter** (no colony): `terms` of the shipped `index` and `query`
//!    scripts split a text alike -- alphanumeric runs, cut at case
//!    boundaries, the whole run kept beside its parts, lower case, in order,
//!    once each (GH #950 § 2). Without it a word indexed one way is searched
//!    another, and `foo bar` misses `fooBar.py`.
//! 2. **ranking**: a hit by name before one by path before one by tag before
//!    one by the one line, other things equal, every hit saying `why`, the
//!    tag and the one line being the ones described; `foo bar` finds
//!    `fooBar.py` and `foo_bar.py`; a file without an extension and an empty
//!    file (no nodes) stand in the catalog and are found; a word only an
//!    `info` answer carried is found nowhere.
//! 3. **filters**: `kind`, `fmt`, `under` -- a prefix of whole folders, so
//!    `/pkg` does not reach `/pkgx` -- and a relative `under` is refused.
//! 4. **pages**: 25 hits answer 20 and a `next`; the `next` answers the
//!    other 5 and an empty `next`; `limit` above 20 is 20.
//! 5. **input classes**: a question carrying `"`, `*`, `-`, `:`, `AND`, `OR`,
//!    `NEAR` -- the full-text syntax of the store -- is never answered
//!    `ok: false`, never a `store_error`: every word is a quoted prefix
//!    literal (OR-BC.B.2). Punctuation alone finds nothing; an empty or blank
//!    `q`, a `q` that is no string, a question without `op_id` are
//!    `invalid_input`; an unknown op is `unknown_op`.
//! 6. **the other ops** refuse a bad argument with `invalid_input` and ask no
//!    source.
//!
//! Every emission of a code cell carries what its contract declares, and no
//! run leaves a dead letter. Free of a paid provider by construction. Guarded
//! like every template-reading test (GH #49).

#[path = "support/librarian_hive.rs"]
mod librarian;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_pure.rs"]
mod pure;
#[path = "support/graph_space_colony.rs"]
mod space;

use librarian::{Doc, col, list};
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

const V: &str = "0123456789ab";

/// A source id of this file's corpora.
fn src(i: u64) -> String {
    format!("fh-{:012x}", 0x0950_0000 + i)
}

fn items(a: &Value) -> Vec<Value> {
    a["items"].as_array().cloned().unwrap_or_default()
}

fn field(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn num(v: &Value) -> f64 {
    v.as_f64()
        .unwrap_or_else(|| panic!("a score is a number: {v}"))
}

fn paths(a: &Value) -> BTreeSet<String> {
    items(a).iter().map(|i| field(i, "path")).collect()
}

/// Sorted by score, best first, then by path (GH #950 § 4).
fn in_order(a: &Value) {
    let items = items(a);
    for w in items.windows(2) {
        let (x, y) = (num(&w[0]["score"]), num(&w[1]["score"]));
        let tie = (x - y).abs() < 1e-9;
        assert!(
            (x > y && !tie) || (tie && field(&w[0], "path") <= field(&w[1], "path")),
            "sorted by score, then path: {a}"
        );
    }
}

/// Four files that each carry the word `widget` in exactly one field, and
/// the rest of the first lock's corpus. The one line and the tags are what
/// each file's `source_described` carries (`librarian::index_files`).
fn rank_corpus() -> Vec<Doc> {
    vec![
        Doc::new(&src(1), V, "/rank/alpha.py")
            .with_oneline("alpha module")
            .with_names(&["widget"]),
        Doc::new(&src(2), V, "/rank/widget.py")
            .with_oneline("beta module")
            .with_names(&["beta"]),
        Doc::new(&src(3), V, "/rank/gamma.py")
            .with_oneline("gamma module")
            .with_tags(json!(["widget"]))
            .with_names(&["gamma"]),
        Doc::new(&src(4), V, "/rank/delta.py")
            .with_oneline("a widget helper")
            .with_names(&["delta"]),
    ]
}

#[test]
fn the_index_and_the_query_split_words_by_one_rule() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let cases = json!([
        "fooBar",
        "HTTPServer",
        "/pkg/foo_bar.py",
        "/case/fooBar.py",
        "item01",
        "fooBar fooBar",
        "Gr\u{f6}\u{df}e",
        "a-b:c\"d*e",
        ""
    ]);
    // `terms` answers a list of words or the words as one text; both read as
    // the text the catalog stores.
    let probe = "[(lambda r: r if isinstance(r, str) else ' '.join(r))(terms(x)) for x in ARGS]";
    let index = pure::pure_at(
        "templates/librarian/index/config.json",
        "",
        probe,
        cases.clone(),
    );
    let query = pure::pure_at("templates/librarian/query/config.json", "", probe, cases);
    assert_eq!(
        index, query,
        "`index` and `query` split a text alike (GH #950 § 2)"
    );
    assert_eq!(
        index,
        json!([
            "foobar foo bar",
            "httpserver http server",
            "pkg foo bar py",
            "case foobar foo bar py",
            "item01",
            "foobar foo bar",
            "gr\u{f6}\u{df}e",
            "a b c d e",
            ""
        ]),
        "runs cut at case boundaries, the whole run kept, lower case, in order, once each"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_ranks_name_before_path_before_tag_before_text() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let mut docs = rank_corpus();
    docs.extend([
        Doc::new(&src(5), V, "/case/fooBar.py")
            .with_oneline("camel case")
            .with_names(&["camel"]),
        Doc::new(&src(6), V, "/case/foo_bar.py")
            .with_oneline("snake case")
            .with_names(&["snake"]),
        Doc::new(&src(7), V, "/case/foo.py")
            .with_oneline("only the first word")
            .with_names(&["lone"]),
        Doc::new(&src(8), V, "/tools/runner").with_oneline("Runs the nightly job."),
        Doc::new(&src(9), V, "/empty/blank.txt"),
    ]);
    librarian::index_files(&h, &mut ports, &root, &docs).await;

    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "rank",
        json!({"q": "widget"}),
    )
    .await;
    assert_eq!(a["ok"], json!(true), "{a}");
    let got: Vec<(String, Value)> = items(&a)
        .iter()
        .map(|i| (field(i, "path"), i["why"].clone()))
        .collect();
    let want = [
        ("/rank/alpha.py".to_string(), json!(["name"])),
        ("/rank/widget.py".to_string(), json!(["path"])),
        ("/rank/gamma.py".to_string(), json!(["tag"])),
        ("/rank/delta.py".to_string(), json!(["text"])),
    ];
    assert_eq!(
        got, want,
        "name before path before tag before text, each hit saying why: {a}"
    );
    in_order(&a);
    let hits = items(&a);
    let top = &hits[0];
    assert_eq!(top["file"], json!(src(1)), "{a}");
    assert_eq!(top["kind"], json!("text"), "{a}");
    assert_eq!(top["fmt"], json!("python"), "{a}");
    assert_eq!(top["oneline"], json!("alpha module"), "{a}");
    assert_eq!(a["next"], json!(""), "{a}");
    // The tag hit and the text hit came in on `source_described`: every
    // file's description reached the index.
    let log = space::message_log(&root);
    let told = log
        .iter()
        .filter(|r| r.to == librarian::INDEX && r.route() == "source_described")
        .count();
    assert_eq!(
        told,
        docs.len(),
        "one description per file reached the index"
    );

    // Every `info` answer of the corpus carried the summary line and the
    // tags of a head before (`librarian::STALE_ONELINE`): none of their
    // words is searchable, in the one line or in the tags.
    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "stale",
        json!({"q": "superseded"}),
    )
    .await;
    assert!(
        librarian::STALE_ONELINE.contains("superseded")
            && librarian::stale_tags() == json!(["superseded"]),
        "the probe word stands in what every `info` answer carries"
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(
        a["items"],
        json!([]),
        "what an `info` answer carries is never searched (GH #950, OR-BC-68): {a}"
    );

    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "two",
        json!({"q": "foo bar"}),
    )
    .await;
    assert_eq!(a["ok"], json!(true), "{a}");
    let want: BTreeSet<String> = ["/case/fooBar.py", "/case/foo_bar.py"]
        .map(String::from)
        .into_iter()
        .collect();
    assert_eq!(
        paths(&a),
        want,
        "both words, wherever the path cuts them, and only both: {a}"
    );
    for i in items(&a) {
        assert!(
            i["why"]
                .as_array()
                .is_some_and(|w| w.contains(&json!("path"))),
            "a hit in the path says so: {i}"
        );
    }

    for (q, path) in [("runner", "/tools/runner"), ("blank", "/empty/blank.txt")] {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("plain-{q}"),
            json!({"q": q}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{a}");
        assert_eq!(
            paths(&a),
            BTreeSet::from([path.to_string()]),
            "{path} is found by its name: {a}"
        );
    }
    for (s, path) in [(src(8), "/tools/runner"), (src(9), "/empty/blank.txt")] {
        let row = librarian::catalog_row(&root, &s).expect("a row for a file without items");
        assert_eq!(col(&row, "path"), path, "{row:#?}");
        assert_eq!(col(&row, "fmt"), "", "no extractor reads it: {row:#?}");
        assert_eq!(col(&row, "nodes"), "0", "{row:#?}");
        assert_eq!(list(&row, "names"), Vec::<String>::new(), "{row:#?}");
        assert_eq!(col(&row, "tomb"), "", "{row:#?}");
    }

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_filters_by_kind_fmt_and_folder() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let docs = [
        Doc::new(&src(21), V, "/docs/report.md").with_oneline("The numbers in words."),
        Doc::new(&src(22), V, "/papers/report.pdf").with_oneline("The numbers on paper."),
        Doc::new(&src(23), V, "/notes/report.txt").with_oneline("The numbers in short."),
        Doc::new(&src(24), V, "/pkg/core.py").with_names(&["core"]),
        Doc::new(&src(25), V, "/pkg/sub/core.py").with_names(&["core"]),
        Doc::new(&src(26), V, "/pkgx/core.py").with_names(&["core"]),
    ];
    librarian::index_files(&h, &mut ports, &root, &docs).await;

    let cases: [(Value, &[&str]); 11] = [
        (
            json!({"q": "report"}),
            &["/docs/report.md", "/notes/report.txt", "/papers/report.pdf"],
        ),
        (
            json!({"q": "report", "kind": "binary"}),
            &["/papers/report.pdf"],
        ),
        (
            json!({"q": "report", "kind": "text"}),
            &["/docs/report.md", "/notes/report.txt"],
        ),
        (
            json!({"q": "report", "fmt": "markdown"}),
            &["/docs/report.md"],
        ),
        (
            json!({"q": "core", "under": "/pkg"}),
            &["/pkg/core.py", "/pkg/sub/core.py"],
        ),
        (
            json!({"q": "core", "under": "/"}),
            &["/pkg/core.py", "/pkg/sub/core.py", "/pkgx/core.py"],
        ),
        (json!({"q": "core", "under": "/pkgx"}), &["/pkgx/core.py"]),
        (
            json!({"q": "core", "under": "/pkg/sub"}),
            &["/pkg/sub/core.py"],
        ),
        (
            json!({"q": "core", "under": "/pkg/core.py"}),
            &["/pkg/core.py"],
        ),
        (json!({"q": "core", "under": "/nowhere"}), &[]),
        (
            json!({"q": "core", "fmt": "python", "under": "/pkg"}),
            &["/pkg/core.py", "/pkg/sub/core.py"],
        ),
    ];
    for (i, (args, want)) in cases.iter().enumerate() {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("filter-{i}"),
            args.clone(),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{args}: {a}");
        let want: BTreeSet<String> = want.iter().map(|p| p.to_string()).collect();
        assert_eq!(paths(&a), want, "{args}: {a}");
    }
    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "filter-relative",
        json!({"q": "core", "under": "pkg"}),
    )
    .await;
    assert_eq!(
        a["ok"],
        json!(false),
        "`under` is a folder of the space: {a}"
    );
    assert_eq!(a["error"]["code"], json!("invalid_input"), "{a}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_answers_twenty_at_a_time_and_says_where_to_go_on() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let docs: Vec<Doc> = (1..=25)
        .map(|i| {
            Doc::new(&src(100 + i), V, &format!("/bulk/item{i:02}.py")).with_oneline("bulk entry")
        })
        .collect();
    librarian::index_files(&h, &mut ports, &root, &docs).await;

    let first = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "page-1",
        json!({"q": "bulk"}),
    )
    .await;
    assert_eq!(first["ok"], json!(true), "{first}");
    assert_eq!(items(&first).len(), 20, "at most 20 hits a page: {first}");
    in_order(&first);
    let next = field(&first, "next");
    assert!(
        !next.is_empty(),
        "a full page says where the next begins: {first}"
    );
    let second = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "page-2",
        json!({"q": "bulk", "cursor": next}),
    )
    .await;
    assert_eq!(second["ok"], json!(true), "{second}");
    assert_eq!(items(&second).len(), 5, "the rest: {second}");
    assert_eq!(second["next"], json!(""), "the last page says so: {second}");
    let seen: BTreeSet<String> = items(&first)
        .iter()
        .chain(items(&second).iter())
        .map(|i| field(i, "file"))
        .collect();
    let want: BTreeSet<String> = docs.iter().map(|d| d.source.clone()).collect();
    assert_eq!(seen, want, "two pages, every file once");

    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "page-7",
        json!({"q": "bulk", "limit": 7}),
    )
    .await;
    assert_eq!(items(&a).len(), 7, "{a}");
    assert!(!field(&a, "next").is_empty(), "{a}");
    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "find",
        "page-over",
        json!({"q": "bulk", "limit": 100}),
    )
    .await;
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(items(&a).len(), 20, "a limit above 20 is 20: {a}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_question_breaks_the_search() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;
    librarian::index_files(&h, &mut ports, &root, &rank_corpus()).await;

    // The word `widget` dressed in the store's full-text syntax: every one is
    // the literal word, so every one finds the file named `widget`.
    let dressed = [
        "\"widget\"",
        "widget*",
        "-widget",
        "widget:",
        "(widget",
        "widget)",
        "^widget",
        "+widget",
        "{widget}",
    ];
    for (i, q) in dressed.iter().enumerate() {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("dressed-{i}"),
            json!({"q": q}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{q:?}: {a}");
        assert!(
            paths(&a).contains("/rank/alpha.py"),
            "{q:?} is the word `widget`: {a}"
        );
    }
    // Operators and broken syntax: answered, never refused by the store.
    let syntax = [
        "widget AND beta",
        "widget OR beta",
        "widget NOT beta",
        "NEAR(widget beta)",
        "NEAR(widget beta, 2)",
        "AND",
        "OR",
        "NEAR",
        "NOT",
        "name:widget",
        "a\"b",
        "\"unbalanced",
        "widget\"*",
        "[widget]",
        "widget^2",
        "'widget'",
        "widget; DROP TABLE entries",
    ];
    for (i, q) in syntax.iter().enumerate() {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("syntax-{i}"),
            json!({"q": q}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{q:?} is answered, not refused: {a}");
        assert!(a.get("error").is_none(), "{q:?}: {a}");
    }
    // Punctuation alone carries no word: nothing is found, nothing fails.
    for (i, q) in ["\"", "*", "-", ":", "\"\"", "()", "^", "* - : \""]
        .iter()
        .enumerate()
    {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("bare-{i}"),
            json!({"q": q}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{q:?}: {a}");
        assert_eq!(a["items"], json!([]), "{q:?}: {a}");
        assert_eq!(a["next"], json!(""), "{q:?}: {a}");
    }
    // No question at all.
    let empty = [
        json!({"q": ""}),
        json!({"q": "   "}),
        json!({"q": "\t\n"}),
        json!({}),
        json!({"q": 42}),
    ];
    for (i, args) in empty.iter().enumerate() {
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("empty-{i}"),
            args.clone(),
        )
        .await;
        assert_eq!(a["ok"], json!(false), "{args}: {a}");
        assert_eq!(a["error"]["code"], json!("invalid_input"), "{args}: {a}");
    }
    let a = librarian::ask(
        &h,
        &mut ports,
        &root,
        "frobnicate",
        "no-such-op",
        json!({"q": "widget"}),
    )
    .await;
    assert_eq!(a["ok"], json!(false), "{a}");
    assert_eq!(a["error"]["code"], json!("unknown_op"), "{a}");
    // A question without `op_id` cannot be answered by its id; it is answered
    // `invalid_input` all the same, under an empty one.
    h.send(librarian::message(
        librarian::HIVE,
        json!({"route": "in_lib", "op": "find"}),
        json!({}),
        json!({"op": "find", "args": {"q": "widget"}, "messages": []}),
    ))
    .await;
    let a = librarian::answer_of(&mut ports, &root, "find", "").await;
    assert_eq!(a["ok"], json!(false), "{a}");
    assert_eq!(a["error"]["code"], json!("invalid_input"), "{a}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_op_refuses_a_bad_argument_and_asks_no_source() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let cases = [
        ("symbol", json!({})),
        ("symbol", json!({"name": ""})),
        ("symbol", json!({"name": "x".repeat(513)})),
        (
            "related",
            json!({"addr": "fh-0123456789ab", "how": "sideways"}),
        ),
        (
            "related",
            json!({"addr": "not-an-address", "how": "callers"}),
        ),
        (
            "related",
            json!({"addr": "fh-0123456789ab", "how": "dependents", "depth": 4}),
        ),
        ("related", json!({"how": "callers"})),
        ("tree", json!({"under": "pkg"})),
        ("tree", json!({"depth": 4})),
        ("tree", json!({"depth": 0})),
        ("tree", json!({"cursor": "next"})),
        ("tree", json!({"limit": 0})),
    ];
    for (i, (op, args)) in cases.iter().enumerate() {
        let a = librarian::ask(&h, &mut ports, &root, op, &format!("bad-{i}"), args.clone()).await;
        assert_eq!(a["ok"], json!(false), "{op} {args}: {a}");
        assert_eq!(
            a["error"]["code"],
            json!("invalid_input"),
            "{op} {args}: {a}"
        );
    }

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    let pulls: Vec<String> = librarian::pulls_in(&log).iter().map(|r| r.say()).collect();
    assert!(
        pulls.is_empty(),
        "a refused question asks no source: {pulls:#?}"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
