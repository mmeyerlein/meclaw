//! GH #950 -- the catalog follows its sources.
//!
//! The librarian keeps one row per source of its member's file space in the
//! table `entries` of its own store, written from the source's announcements
//! (`source_changed`), from the two answers it pulls for each (`info` for the
//! kind, `outline` for the names) and from the source's descriptions
//! (`source_described`, the summary line and the tags). A space announces a
//! head before its model has summarised it, so an `info` pulled on the
//! announcement carries the summary of the head before; the description of
//! a head comes later, on its own lane (GH #950, OR-BC-68). These locks play
//! the file space against the shipped `templates/librarian`, booted by
//! itself (`support/librarian_hive.rs`):
//!
//! 1. **create, replace, move, remove** of one python, one markdown and one
//!    pdf file: the row follows -- path, dir, fmt, kind, oneline (at most 300
//!    characters), tags (a list of at most eight strings, `[]` for anything
//!    else), the names of the top-level items only, nodes, version, tomb.
//!    Exactly two pulls per announcement, the outline's `limit` the announced
//!    node count (at least 1, at most 2000); none for a removal, none for an
//!    announcement without a version, none for a description. A grave is
//!    never deleted and never found by `find`.
//! 2. **a stale answer never overwrites**: the answers to v1 arrive after v2
//!    was announced -- once before v2's own answers, once after them -- and
//!    the row holds v2. The compare-and-set on `announced` is the whole rule
//!    (GH #950 § 3: no table of pulls).
//! 3. **a move takes its path from the event**: same version, new path; a
//!    late `info` of the earlier announcement and a description under the old
//!    path reset nothing (events are ordered, answers are not).
//! 4. **names**: one per top-level anchor -- the part after the first `:` of
//!    the last segment, `%XX` decoded, `impl:T+Tr` naming both -- once each,
//!    at most 64; a page names nothing.
//! 5. **a description is kept only for the announced version**: an `info`
//!    that carries a summary line and tags writes neither; the description of
//!    the announced version writes both; a late description of a version no
//!    longer announced, an unversioned one, one for a source never announced
//!    and one said over a grave change nothing and lay no row; the path of a
//!    description is never taken.
//! 6. **the hive and its store** are what the build spec names: its lanes, its
//!    five cells, the columns of `entries`, its full-text index, its indexes.
//!
//! Every emission of a code cell carries what its contract declares, and no
//! run leaves a dead letter. Signals, not windows: a write is awaited in the
//! colony's own log (the store answered the index's bundle for exactly that
//! delivery); the end of a run is the colony going quiet. Free of a paid
//! provider by construction. Guarded like every template-reading test (GH #49).

#[path = "support/librarian_hive.rs"]
mod librarian;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use librarian::{Doc, Row, col, list, node, terms};
use meclaw_core::Message;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use std::collections::{BTreeMap, BTreeSet};

const V1: &str = "111111111111";
const V2: &str = "222222222222";
/// A full-length version: the catalog and every pull name its first twelve.
const V64: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

const PY: &str = "fh-a00000000001";
const MD: &str = "fh-b00000000002";
const PDF: &str = "fh-c00000000003";
const LATE: &str = "fh-d00000000004";
const EARLY: &str = "fh-e00000000005";
const MOVED: &str = "fh-f00000000006";
const RUST: &str = "fh-0a0000000007";
const SCAN: &str = "fh-0b0000000008";
const MANY: &str = "fh-0c0000000009";
const STRANGER: &str = "fh-0d000000000a";
const UNVERSIONED: &str = "fh-0e000000000b";
const TOLD: &str = "fh-0f000000000c";
const UNTOLD: &str = "fh-10000000000d";
const NAMELESS: &str = "fh-11000000000e";

/// Ten tags: the catalog keeps eight of them.
const TEN: [&str; 10] = [
    "cli", "server", "app", "entry", "main", "tool", "http", "api", "port", "run",
];

/// One file through its four moves.
struct Life {
    create: Doc,
    replace: Doc,
    /// Where the move takes it (the version of `replace`).
    to: &'static str,
    /// The top-level names after `create` and after `replace`.
    names: [&'static [&'static str]; 2],
    /// The tags after `create` and after `replace`; more than eight given
    /// means eight of them.
    tags: [&'static [&'static str]; 2],
    /// A word only this file carries once moved, for `find`.
    word: &'static str,
}

fn lives() -> Vec<Life> {
    let long = "a long line ".repeat(40);
    vec![
        Life {
            create: Doc::new(PY, V1, "/pkg/app.py")
                .with_oneline("Command line entry of the app.")
                .with_nodes(vec![
                    node("def:main", ""),
                    node("class:App", ""),
                    node("class:App/def:run", "class:App"),
                ]),
            replace: Doc::new(PY, V2, "/pkg/app.py")
                .with_oneline("Command line entry and server of the app.")
                .with_tags(json!(TEN))
                .with_nodes(vec![
                    node("def:main", ""),
                    node("def:serve", ""),
                    node("class:App", ""),
                    node("class:App/def:stop", "class:App"),
                ]),
            to: "/srv/main_app.py",
            names: [&["main", "App"], &["main", "serve", "App"]],
            tags: [&[], &TEN],
            word: "serve",
        },
        Life {
            create: Doc::new(MD, V1, "/docs/guide.md")
                .with_oneline("How to set the tool up.")
                .with_tags(json!(["guide", "setup"]))
                .with_nodes(vec![
                    node("sec:setup", ""),
                    node("sec:setup/sec:linux", "sec:setup"),
                    node("sec:usage", ""),
                ]),
            replace: Doc::new(MD, V2, "/docs/guide.md")
                .with_oneline(&long)
                .with_tags(json!(["guide"]))
                .with_nodes(vec![
                    node("sec:setup", ""),
                    node("sec:usage", ""),
                    node("sec:faq", ""),
                ]),
            to: "/handbook/guide.md",
            names: [&["setup", "usage"], &["setup", "usage", "faq"]],
            tags: [&["guide", "setup"], &["guide"]],
            word: "handbook",
        },
        Life {
            create: Doc::new(PDF, V64, "/papers/report.pdf")
                .with_oneline("Quarterly report.")
                .with_tags(json!(["report"]))
                .with_nodes(vec![
                    node("page:1", ""),
                    node("page:2", ""),
                    node("page:3", ""),
                ])
                .with_count(4000),
            replace: Doc::new(PDF, V2, "/papers/report.pdf")
                .with_oneline("Quarterly report, revised.")
                .with_tags(json!("not a list"))
                .with_nodes(vec![node("page:1", ""), node("page:2", "")]),
            to: "/archive/2026/report.pdf",
            names: [&[], &[]],
            tags: [&["report"], &[]],
            word: "archive",
        },
    ]
}

/// The words of a path as the catalog splits it (`[^\W_]+`, lower case);
/// none of the paths here has a case boundary.
fn words(path: &str) -> Vec<String> {
    path.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The row of `doc` once both its answers and its description are written:
/// what the event, the two answers and the description carry (GH #950 § 2
/// and § 3, OR-BC-68), and nothing older -- least of all the summary of the
/// head before that an `info` answer carries ([`librarian::STALE_ONELINE`]).
fn assert_row(row: &Row, doc: &Doc, names: &[&str], tags: &[&str], stage: &str) {
    let at = format!("{} after {stage}", doc.source);
    assert_eq!(col(row, "path"), doc.path, "{at}: {row:#?}");
    assert_eq!(
        col(row, "dir"),
        librarian::dir_of(&doc.path),
        "{at}: {row:#?}"
    );
    assert_eq!(col(row, "fmt"), doc.fmt, "{at}: {row:#?}");
    assert_eq!(col(row, "kind"), doc.kind, "{at}: {row:#?}");
    let line = col(row, "oneline");
    if doc.oneline.chars().count() <= 300 {
        assert_eq!(line, doc.oneline, "{at}: {row:#?}");
    } else {
        assert!(
            !line.is_empty()
                && line.chars().count() <= 300
                && doc.oneline.starts_with(line.trim_end()),
            "{at}: the one line is cut to at most 300 characters: {line:?}"
        );
    }
    let v = librarian::v12(&doc.version);
    assert_eq!(col(row, "version"), v, "{at}: {row:#?}");
    assert_eq!(col(row, "announced"), v, "{at}: {row:#?}");
    assert_eq!(
        col(row, "nodes"),
        doc.announced_nodes().to_string(),
        "{at}: the node count of the event: {row:#?}"
    );
    assert_eq!(col(row, "tomb"), "", "{at}: {row:#?}");

    let got = list(row, "names");
    let have: BTreeSet<&str> = got.iter().map(String::as_str).collect();
    let want: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(have, want, "{at}: the names of the top-level items only");
    assert_eq!(got.len(), have.len(), "{at}: every name once: {got:?}");
    let nterms = terms(row, "nterms");
    for n in names {
        assert!(
            nterms.contains(&n.to_lowercase()),
            "{at}: `nterms` holds the name {n}: {nterms:?}"
        );
    }
    let pterms = terms(row, "pterms");
    for w in words(&doc.path) {
        assert!(
            pterms.contains(&w),
            "{at}: `pterms` holds the path word {w}: {pterms:?}"
        );
    }

    let got = list(row, "tags");
    if tags.len() <= 8 {
        assert_eq!(got, tags, "{at}: {row:#?}");
    } else {
        assert_eq!(got.len(), 8, "{at}: at most eight tags: {got:?}");
        assert!(
            got.iter().all(|t| tags.contains(&t.as_str())),
            "{at}: the tags are the source's own: {got:?}"
        );
    }
}

/// The sources a `find` answer names.
fn hits(a: &Value) -> Vec<String> {
    a["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i["file"].as_str().map(str::to_string))
        .collect()
}

/// Both answers of one announcement, as the file space sends them.
async fn answer(h: &ColonyHandle, pulls: &(Message, Message), doc: &Doc) {
    h.send(librarian::answer_pull(&pulls.0, doc.info())).await;
    h.send(librarian::answer_pull(&pulls.1, doc.outline()))
        .await;
}

/// Both `op_id`s of each doc, each to be written `n` times.
fn ids(docs: &[&Doc], n: usize) -> Vec<(String, usize)> {
    docs.iter()
        .flat_map(|d| [d.op_id("info"), d.op_id("outline")])
        .map(|id| (id, n))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_catalog_follows_create_replace_move_and_remove() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;
    let lives = lives();

    let created: Vec<Doc> = lives.iter().map(|l| l.create.clone()).collect();
    librarian::index_files(&h, &mut ports, &root, &created).await;
    for l in &lives {
        let row = librarian::catalog_row(&root, &l.create.source).expect("a row per new file");
        assert_row(&row, &l.create, l.names[0], l.tags[0], "create");
    }

    let replaced: Vec<Doc> = lives.iter().map(|l| l.replace.clone()).collect();
    librarian::index_files(&h, &mut ports, &root, &replaced).await;
    for l in &lives {
        let row = librarian::catalog_row(&root, &l.create.source).expect("the row stays");
        assert_row(&row, &l.replace, l.names[1], l.tags[1], "replace");
    }

    let moved: Vec<Doc> = lives
        .iter()
        .map(|l| l.replace.clone().moved_to(l.to))
        .collect();
    librarian::index_files(&h, &mut ports, &root, &moved).await;
    for (l, d) in lives.iter().zip(&moved) {
        let row = librarian::catalog_row(&root, &d.source).expect("the row stays");
        assert_row(&row, d, l.names[1], l.tags[1], "move");
        let pterms = terms(&row, "pterms");
        let now = words(&d.path);
        for w in words(&l.replace.path) {
            assert!(
                now.contains(&w) || !pterms.contains(&w),
                "{}: `pterms` forgot the old path's {w}: {pterms:?}",
                d.source
            );
        }
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("live-{}", l.word),
            json!({"q": l.word}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{a}");
        assert!(
            hits(&a).contains(&d.source),
            "a living file is found by its word `{}`: {a}",
            l.word
        );
    }

    for d in &moved {
        h.send(librarian::removal(&d.source, &d.path)).await;
    }
    let sources: Vec<String> = lives.iter().map(|l| l.create.source.clone()).collect();
    librarian::wait_for(&root, "every removed file is a grave", || {
        sources
            .iter()
            .all(|s| librarian::catalog_row(&root, s).is_some_and(|r| col(&r, "tomb") == "1"))
    })
    .await;
    for (l, d) in lives.iter().zip(&moved) {
        let row = librarian::catalog_row(&root, &d.source).expect("a grave is never deleted");
        assert_eq!(col(&row, "tomb"), "1", "{row:#?}");
        assert_eq!(
            col(&row, "announced"),
            "",
            "a grave is announced no more: {row:#?}"
        );
        assert_eq!(
            col(&row, "path"),
            d.path,
            "a grave keeps where it lay: {row:#?}"
        );
        assert_eq!(
            col(&row, "version"),
            librarian::v12(V2),
            "a grave keeps its last version: {row:#?}"
        );
        let a = librarian::ask(
            &h,
            &mut ports,
            &root,
            "find",
            &format!("gone-{}", l.word),
            json!({"q": l.word}),
        )
        .await;
        assert_eq!(a["ok"], json!(true), "{a}");
        assert!(!hits(&a).contains(&d.source), "a grave is never found: {a}");
    }

    // A removal of a source the catalog never held writes nothing and is no
    // error; an announcement without a version is parked (GH #950 § 3).
    h.send(librarian::removal(STRANGER, "/nowhere/at/all.py"))
        .await;
    h.send(librarian::message(
        librarian::HIVE,
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": UNVERSIONED, "path": "/odd/unversioned.py", "fmt": "python",
               "parser": "python", "mark": "", "nodes": 1, "links": 0, "tomb": false,
               "messages": []}),
    ))
    .await;
    space::quiet(&root).await;

    let log = space::message_log(&root);
    let rows = librarian::catalog_rows(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    let held: BTreeSet<String> = rows.iter().map(|r| col(r, "source").to_string()).collect();
    let want: BTreeSet<String> = sources.iter().cloned().collect();
    assert_eq!(
        held, want,
        "one row per source, never deleted, nothing else: {rows:#?}"
    );
    let pulls = librarian::pulls_in(&log);
    for s in &sources {
        for part in ["info", "outline"] {
            let prefix = format!("lib:f:i:{part}:{s}:");
            let n = pulls
                .iter()
                .filter(|r| {
                    r.hop["op_id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with(&prefix))
                })
                .count();
            assert_eq!(
                n, 3,
                "{s}: one `{part}` pull per announcement (create, replace, move), none for \
                 its removal"
            );
        }
    }
    assert_eq!(
        pulls.len(),
        2 * 3 * sources.len(),
        "two pulls per announcement and nothing else: {:#?}",
        pulls.iter().map(|r| r.say()).collect::<Vec<_>>()
    );
    let doors = log
        .iter()
        .filter(|r| r.to == librarian::INDEX && r.route() == "source_changed")
        .count();
    assert_eq!(
        doors,
        3 * 3 + 3 + 2,
        "every event of the run reached the index"
    );
    let told = log
        .iter()
        .filter(|r| r.to == librarian::INDEX && r.route() == "source_described")
        .count();
    assert_eq!(
        told,
        3 * 3,
        "one description per announcement of the run reached the index (the move's \
         as well -- it writes the same line again)"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_answer_never_overwrites_a_newer_announcement() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let version = |s: &str, v: &str, path: &str, line: &str, name: &str| {
        Doc::new(s, v, path).with_oneline(line).with_names(&[name])
    };
    let late = [
        version(LATE, V1, "/pkg/late.py", "the old line", "old"),
        version(LATE, V2, "/pkg/late.py", "the new line", "new"),
    ];
    let early = [
        version(EARLY, V1, "/pkg/early.py", "the old line", "old"),
        version(EARLY, V2, "/pkg/early.py", "the new line", "new"),
    ];
    for d in late.iter().chain(&early) {
        h.send(d.announcement()).await;
    }
    let mut held = Vec::new();
    let mut pulls = Vec::new();
    for d in late.iter().chain(&early) {
        pulls.push(librarian::take_pulls(&mut ports, &root, d, &mut held).await);
    }
    // Both files are described at v2, the announced version: the summary
    // line and the tags come from there alone (GH #950, OR-BC-68). What
    // follows is about the pulled answers, which write neither.
    h.send(late[1].description()).await;
    h.send(early[1].description()).await;
    librarian::wait_described(
        &root,
        "v2 of both files is described",
        &[(late[1].described_key(), 1), (early[1].described_key(), 1)],
    )
    .await;

    // The late file: v2 is answered first. The early file: the answers to v1
    // arrive while v2 is announced and not yet answered.
    answer(&h, &pulls[1], &late[1]).await;
    answer(&h, &pulls[2], &early[0]).await;
    librarian::wait_written(
        &root,
        "v2 of the late file and v1 of the early one are handled",
        &ids(&[&late[1], &early[0]], 1),
    )
    .await;
    let row = librarian::catalog_row(&root, EARLY).expect("the early row");
    assert_eq!(
        col(&row, "announced"),
        V2,
        "the newer event stands: {row:#?}"
    );
    assert_eq!(
        col(&row, "version"),
        "",
        "an answer to v1 after v2 was announced writes nothing: {row:#?}"
    );
    assert_eq!(
        col(&row, "oneline"),
        "the new line",
        "the description of the announced version stands; a stale answer touches it \
         not: {row:#?}"
    );
    assert_eq!(list(&row, "names"), Vec::<String>::new(), "{row:#?}");
    let row = librarian::catalog_row(&root, LATE).expect("the late row");
    assert_row(&row, &late[1], &["new"], &[], "v2");

    // The late file: the answers to v1 arrive after v2 is written. The early
    // file: v2's own answers arrive.
    answer(&h, &pulls[0], &late[0]).await;
    answer(&h, &pulls[3], &early[1]).await;
    librarian::wait_written(
        &root,
        "v1 of the late file and v2 of the early one are handled",
        &ids(&[&late[0], &early[1]], 1),
    )
    .await;
    space::quiet(&root).await;
    let rows = [
        librarian::catalog_row(&root, LATE).expect("the late row"),
        librarian::catalog_row(&root, EARLY).expect("the early row"),
    ];
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_row(&rows[0], &late[1], &["new"], &[], "a late answer to v1");
    assert_row(
        &rows[1],
        &early[1],
        &["new"],
        &[],
        "v2 after an early answer to v1",
    );
    assert!(
        held.is_empty(),
        "no pull beyond two per announcement: {held:?}"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_takes_its_path_from_the_event_and_a_late_answer_resets_nothing() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let first = Doc::new(MOVED, V1, "/docs/old/notes.md")
        .with_oneline("Notes that moved.")
        .with_tags(json!(["notes"]))
        .with_nodes(vec![node("sec:notes", "")]);
    let second = first.clone().moved_to("/docs/new/place.md");
    let mut held = Vec::new();
    h.send(first.announcement()).await;
    let early = librarian::take_pulls(&mut ports, &root, &first, &mut held).await;
    h.send(second.announcement()).await;
    let now = librarian::take_pulls(&mut ports, &root, &second, &mut held).await;

    // The head's description arrives after the move, under the path the head
    // had when its model summarised it. Same version: its summary line and
    // its tags are written, its path is not taken -- the path comes from
    // `source_changed` alone (GH #950, OR-BC-68).
    h.send(first.description()).await;
    librarian::wait_described(
        &root,
        "the description under the old path is handled",
        &[(first.described_key(), 1)],
    )
    .await;
    answer(&h, &now, &second).await;
    librarian::wait_written(&root, "the move's answers are handled", &ids(&[&second], 1)).await;
    let row = librarian::catalog_row(&root, MOVED).expect("the moved row");
    assert_row(&row, &second, &["notes"], &["notes"], "the move");

    // The earlier announcement's answers arrive last; its `info` names the
    // old path. Both pass the compare-and-set (same version) and write the
    // same kind and names -- the path stays where the event put it, and the
    // summary line its `info` carries (the head before's) is not taken.
    answer(&h, &early, &first).await;
    librarian::wait_written(
        &root,
        "the earlier announcement's answers are handled",
        &ids(&[&first], 2),
    )
    .await;
    space::quiet(&root).await;
    let row = librarian::catalog_row(&root, MOVED).expect("the moved row");
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_row(&row, &second, &["notes"], &["notes"], "a late answer");
    let pterms = terms(&row, "pterms");
    for gone in ["old", "notes"] {
        assert!(
            !pterms.contains(gone),
            "`pterms` is the new path's, not the old one's ({gone}): {pterms:?}"
        );
    }
    assert_eq!(
        librarian::pulls_in(&log).len(),
        4,
        "two pulls per announcement, the move included"
    );
    assert!(
        held.is_empty(),
        "no pull beyond two per announcement: {held:?}"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_top_level_items_name_a_file() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let many: Vec<String> = (0..70).map(|i| format!("f{i:02}")).collect();
    let docs = [
        Doc::new(RUST, V1, "/pkg/store.rs").with_nodes(vec![
            node("struct:Store", ""),
            node("impl:Store", ""),
            node("impl:Store/fn:new", "impl:Store"),
            node("impl:Store+Display", ""),
            node("impl:Store+Display/fn:fmt", "impl:Store+Display"),
            node("fn:open", ""),
            // A name with `#` travels as `%23` (the file space's `anchor_part`).
            node("fn:r%23match", ""),
        ]),
        Doc::new(SCAN, V1, "/papers/scan.pdf")
            .with_nodes(vec![node("page:1", ""), node("page:2", "")]),
        Doc::new(MANY, V1, "/pkg/many.py")
            .with_nodes(many.iter().map(|n| node(&format!("def:{n}"), "")).collect()),
    ];
    librarian::index_files(&h, &mut ports, &root, &docs).await;
    space::quiet(&root).await;
    let rust = librarian::catalog_row(&root, RUST).expect("the rust row");
    let scan = librarian::catalog_row(&root, SCAN).expect("the pdf row");
    let lots = librarian::catalog_row(&root, MANY).expect("the long row");
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    let got = list(&rust, "names");
    let have: BTreeSet<&str> = got.iter().map(String::as_str).collect();
    let want: BTreeSet<&str> = ["Store", "Display", "open", "r#match"]
        .into_iter()
        .collect();
    assert_eq!(
        have, want,
        "a struct, an impl and an impl of a trait name their type (and the trait), a nested \
         item names nothing, `%23` is `#`"
    );
    assert_eq!(
        got.len(),
        4,
        "`Store` is named once although three items carry it: {got:?}"
    );
    assert_eq!(
        list(&scan, "names"),
        Vec::<String>::new(),
        "a page names nothing: {scan:#?}"
    );
    let got = list(&lots, "names");
    assert_eq!(got.len(), 64, "at most 64 names: {got:?}");
    assert!(
        got.iter().all(|n| many.contains(n)),
        "every name is one of the file's: {got:?}"
    );
    assert_eq!(
        got.iter().collect::<BTreeSet<_>>().len(),
        64,
        "every name once: {got:?}"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_description_is_kept_only_for_the_announced_version() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let v1 = Doc::new(TOLD, V1, "/pkg/told.py")
        .with_oneline("The first summary.")
        .with_tags(json!(["first", "draft"]))
        .with_names(&["told"]);
    let v2 = Doc::new(TOLD, V2, "/pkg/told.py")
        .with_oneline("The second summary.")
        .with_tags(json!(["second", "", "  ", "final"]))
        .with_names(&["told"]);
    let row = || librarian::catalog_row(&root, TOLD).expect("the row of the told file");
    let mut held = Vec::new();

    // v1 is announced and answered. Its `info` carries a summary line and
    // tags -- even v1's own -- and the catalog takes neither: on an
    // announcement a space's `info` holds the head before's (GH #950,
    // OR-BC-68), so the kind is all an `info` writes.
    h.send(v1.announcement()).await;
    let p1 = librarian::take_pulls(&mut ports, &root, &v1, &mut held).await;
    h.send(librarian::answer_pull(
        &p1.0,
        librarian::info_body(&v1.path, &v1.kind, &v1.oneline, v1.tags.clone()),
    ))
    .await;
    h.send(librarian::answer_pull(&p1.1, v1.outline())).await;
    librarian::wait_written(&root, "v1's answers are handled", &ids(&[&v1], 1)).await;
    let answered = row();
    assert_eq!(
        col(&answered, "version"),
        V1,
        "the `info` of v1 landed: {answered:#?}"
    );
    assert_eq!(col(&answered, "kind"), "text", "{answered:#?}");
    assert_eq!(
        col(&answered, "oneline"),
        "",
        "an `info` answer writes no summary line, whatever it carries: {answered:#?}"
    );
    assert_eq!(
        list(&answered, "tags"),
        Vec::<String>::new(),
        "an `info` answer writes no tags, whatever it carries: {answered:#?}"
    );

    // v1 is described: the summary line and the tags stand.
    h.send(v1.description()).await;
    librarian::wait_described(
        &root,
        "v1's description is handled",
        &[(v1.described_key(), 1)],
    )
    .await;
    assert_row(
        &row(),
        &v1,
        &["told"],
        &["first", "draft"],
        "its description",
    );

    // v2 is announced and answered; then a description of v1 arrives once
    // more, late and with another line. v1 is no longer the announced
    // version: nothing of the row changes, not even `changed_at`.
    h.send(v2.announcement()).await;
    let p2 = librarian::take_pulls(&mut ports, &root, &v2, &mut held).await;
    answer(&h, &p2, &v2).await;
    librarian::wait_written(&root, "v2's answers are handled", &ids(&[&v2], 1)).await;
    let before = row();
    assert_eq!(col(&before, "announced"), V2, "{before:#?}");
    h.send(librarian::described(
        TOLD,
        V1,
        &v1.path,
        "A late word on the first head.",
        json!(["late"]),
    ))
    .await;
    librarian::wait_described(
        &root,
        "the late description of v1 is handled",
        &[(v1.described_key(), 2)],
    )
    .await;
    assert_eq!(
        row(),
        before,
        "the description of a version no longer announced changes nothing"
    );

    // v2 is described under a path no event ever named: the summary line and
    // the tags are written (blank tags dropped), the path is not taken -- it
    // comes from `source_changed` alone.
    h.send(librarian::described(
        TOLD,
        V2,
        "/elsewhere/other.py",
        &v2.oneline,
        v2.tags.clone().expect("v2's tags"),
    ))
    .await;
    librarian::wait_described(
        &root,
        "v2's description is handled",
        &[(v2.described_key(), 1)],
    )
    .await;
    let told = row();
    assert_row(
        &told,
        &v2,
        &["told"],
        &["second", "final"],
        "its description",
    );
    let pterms = terms(&told, "pterms");
    for foreign in ["elsewhere", "other"] {
        assert!(
            !pterms.contains(foreign),
            "the path of a description is never taken ({foreign}): {told:#?}"
        );
    }

    // Three descriptions that lay no row and change none: one for a source
    // the catalog never held (its compare-and-set matches nothing), one with
    // no version at all and one with an empty version -- both parked. The
    // parked ones send no bundle, so the colony going quiet is their end.
    h.send(librarian::described(
        UNTOLD,
        V1,
        "/pkg/untold.py",
        "Never announced.",
        json!(["ghost"]),
    ))
    .await;
    h.send(librarian::message(
        librarian::HIVE,
        json!({"route": "source_described"}),
        json!({}),
        json!({"source": NAMELESS, "path": "/pkg/nameless.py", "oneline": "No version.",
               "tags": ["none"], "messages": []}),
    ))
    .await;
    h.send(librarian::described(
        TOLD,
        "",
        &v2.path,
        "An empty version.",
        json!(["none"]),
    ))
    .await;
    librarian::wait_described(
        &root,
        "the description of a source never announced is handled",
        &[(librarian::described_key(UNTOLD, V1), 1)],
    )
    .await;
    space::quiet(&root).await;
    let held_rows: BTreeSet<String> = librarian::catalog_rows(&root)
        .iter()
        .map(|r| col(r, "source").to_string())
        .collect();
    assert_eq!(
        held_rows,
        BTreeSet::from([TOLD.to_string()]),
        "a description lays no row of its own"
    );
    assert_eq!(
        row(),
        told,
        "an unversioned description changes nothing of a known row"
    );

    // A grave stays a grave: v2's description said once more after the
    // removal finds no announced version (`announced` emptied).
    h.send(librarian::removal(TOLD, &v2.path)).await;
    librarian::wait_for(&root, "the removed file is a grave", || {
        librarian::catalog_row(&root, TOLD).is_some_and(|r| col(&r, "tomb") == "1")
    })
    .await;
    let grave = row();
    h.send(librarian::described(
        TOLD,
        V2,
        &v2.path,
        "Said over a grave.",
        json!(["grave"]),
    ))
    .await;
    librarian::wait_described(
        &root,
        "the description over the grave is handled",
        &[(v2.described_key(), 2)],
    )
    .await;
    space::quiet(&root).await;
    let last = row();
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_eq!(last, grave, "a description never wakes a grave");
    assert_eq!(col(&last, "tomb"), "1", "{last:#?}");
    assert_eq!(col(&last, "oneline"), v2.oneline, "{last:#?}");

    // The lane, measured at its receivers: every description reached
    // `./index` and no other cell of the hive, and the index answered each
    // with one `described` store bundle or with nothing -- a description
    // pulls nothing and sends nothing out of the hive.
    let inside = format!("{}/", librarian::HIVE);
    let inbound: Vec<_> = log
        .iter()
        .filter(|r| r.route() == "source_described" && r.to.starts_with(&inside))
        .collect();
    assert!(
        inbound.iter().all(|r| r.to == librarian::INDEX),
        "a description goes to `./index` only: {:#?}",
        inbound.iter().map(|r| r.say()).collect::<Vec<_>>()
    );
    assert_eq!(
        inbound.len(),
        7,
        "every description of the run reached the index"
    );
    for d in &inbound {
        let out: Vec<_> = log
            .iter()
            .filter(|r| r.parent.as_deref() == Some(d.id.as_str()))
            .collect();
        assert!(
            out.len() <= 1
                && out.iter().all(|r| r.from == librarian::INDEX
                    && r.to == librarian::STORE
                    && r.hop["phase"] == json!("described")),
            "a description is answered by one `described` store bundle or by nothing: {:#?}",
            out.iter().map(|r| r.say()).collect::<Vec<_>>()
        );
    }
    let written = librarian::described_written(&log);
    assert_eq!(
        written.values().sum::<usize>(),
        5,
        "five descriptions carried a source and a version and were handled, the two \
         without a version were parked: {written:?}"
    );
    assert_eq!(
        librarian::pulls_in(&log).len(),
        4,
        "two pulls per announcement, none for a description"
    );
    assert!(
        held.is_empty(),
        "no pull beyond two per announcement: {held:?}"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

fn set(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|x| x.to_string()).collect()
}

/// The index writes an announcement's path and version in arrival order,
/// last one wins (`a_stale_answer_never_overwrites_a_newer_announcement`
/// sends v1 and v2 of one source back to back). A code cell runs four
/// messages at once by default (`CodeParams::effective_max_concurrency`), and
/// on a build host the older bundle was written last; one at a time is what
/// makes "the newer event stands" true (the curator's pattern, GH #765).
#[test]
fn the_index_runs_one_message_at_a_time() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let cfg = librarian::cell_config("index");
    assert_eq!(
        cfg["params"]["max_concurrency"],
        json!(1),
        "templates/librarian/index: `params.max_concurrency` is 1: {}",
        cfg["params"]["max_concurrency"]
    );
}

#[test]
fn the_hive_and_its_store_are_what_the_build_spec_names() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let hive = space::read_json(&space::repo("templates/librarian/config.json"));
    assert_eq!(hive["cell"]["type"], json!("hive"));
    assert_eq!(
        hive["params"]["ports"],
        json!([]),
        "the hive path is the address: no port (GH #950 § 1)"
    );
    let lanes = |side: &str| -> BTreeSet<String> {
        hive["params"]["contract"][side]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|l| l["route"].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(
        lanes("accepts"),
        set(&[
            "source_changed",
            "source_described",
            "in_pulled",
            "in_lib",
            "in_tool",
            "in_schemas"
        ]),
        "the lanes into the hive (GH #950 § 1, `source_described` OR-BC-68)"
    );
    assert_eq!(
        lanes("emits"),
        set(&["pull", "answer", "tool_result", "tool_schemas"]),
        "the lanes out of the hive (GH #950 § 1)"
    );

    let mut cells = BTreeMap::new();
    for e in std::fs::read_dir(space::repo("templates/librarian")).expect("the template") {
        let p = e.expect("an entry").path();
        let cfg = p.join("config.json");
        if p.is_dir() && cfg.is_file() {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let ty = space::read_json(&cfg)["cell"]["type"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            cells.insert(name, ty);
        }
    }
    let want: BTreeMap<String, String> = [
        ("index", "code"),
        ("query", "code"),
        ("tools", "code"),
        ("schemas", "code"),
        ("store", "store"),
    ]
    .iter()
    .map(|(c, t)| (c.to_string(), t.to_string()))
    .collect();
    assert_eq!(
        cells, want,
        "five cells, no model, no embedder (GH #950 § 1)"
    );

    let store = librarian::cell_config("store");
    let schema = store["params"]["schema"]["entries"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let columns: BTreeSet<&str> = schema.keys().map(String::as_str).collect();
    assert_eq!(
        columns,
        librarian::COLUMNS.into_iter().collect::<BTreeSet<&str>>(),
        "the columns of `entries` (GH #950 § 2)"
    );
    for (c, t) in &schema {
        assert_eq!(t, &json!("text"), "entries.{c} is text");
    }
    assert_eq!(
        store["params"]["fts"],
        json!({"entries": ["pterms", "nterms", "oneline", "tags"]}),
        "the full-text index `find` searches"
    );
    let indexes: Vec<Value> = store["params"]["indexes"]
        .as_object()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    assert!(
        indexes.iter().any(|i| i["table"] == json!("entries")
            && i["on"] == json!(["source"])
            && i["unique"] == json!(true)),
        "one row per source: a unique index on `entries(source)`: {indexes:#?}"
    );
    assert!(
        indexes
            .iter()
            .any(|i| i["table"] == json!("entries") && i["on"] == json!(["path"])),
        "an index on `entries(path)`: {indexes:#?}"
    );
    let internal = |v: &Value| v["write_surface"] == json!("internal");
    assert!(
        internal(&store["contract"]) || internal(&store["params"]),
        "only the hive's own cells write the catalog"
    );
}
