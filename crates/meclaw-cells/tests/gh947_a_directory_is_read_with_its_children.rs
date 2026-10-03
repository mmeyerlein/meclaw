//! GH #947 -- a directory is read with its counts and its children. The
//! `dirs` rows `./derive`'s sync keeps (files, bytes, nodes, tags, dirty,
//! summary) are what `dir_info {path, cursor?, limit?}` answers, with ONE page
//! of the living children by path -- subdirectories and files, each in its
//! own form -- and the `next` cursor; the root reads as zeros before it has a
//! row, an unknown or removed directory is `not_found`. `list` carries the
//! same counts on its directory entries and shows an empty directory too;
//! `info` names the tags of the version it answers for.
//!
//! Two ledger lines of the wave on the read side: an anchor whose percent
//! escapes decode to no valid UTF-8 (the bytes of a lone surrogate) answers
//! with an error code and never with a Python exception (ledger 15; the lone
//! surrogate as a Python string itself is held at the function by
//! `gh947_the_directory_twins_are_one`, it cannot travel in a message of this
//! harness), and a `node_runs` row that counts more nodes than there are rows
//! never cuts the outline short -- read computes it from the blocks instead
//! (ledger 13).
//!
//! The shipped space in one process (`support/file_space_hive.rs`) with the
//! store's declared indexes applied as the store factory applies them.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const A_MD: &str = "# A\n\n## One\nx\n";
const C_MD: &str = "# C\n";
const B_MD: &str = "# B\n\n## Two\ny\n";
const TOP: &str = "top\n";
const SUMMARY: &str = "One line of the file.\n\nA short paragraph.";
const SUMMARY_TAGS: &str = "One line of the file.\n\nA short paragraph.\n\nTAGS: River, mill";

/// The unique indexes whose `unique_violation` is an answer, not a fault.
const ANSWERS: [&str; 3] = ["claims_path", "dirs_path", "contrib_file"];

/// The shipped space without embeddings, its store carrying the declared
/// indexes (GH #915: the store factory applies them right after the schema).
fn space() -> Space {
    let sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    let store = meclaw_cells::store::StoreParams::parse(&cell_config("store")["params"])
        .expect("the store params parse");
    meclaw_cells::store::ddl::apply_index_ddl(&sp.db, &store.indexes).expect("index ddl");
    sp
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn refused(v: Value, code: &str) -> Value {
    assert_eq!(v["ok"], json!(false), "expected {code}: {v}");
    assert_eq!(v["error"]["code"], json!(code), "{v}");
    v
}

/// The answer without the keys every answer carries.
fn bare(v: Value) -> Value {
    let mut b = ok(v).as_object().cloned().unwrap();
    for k in ["ok", "op", "op_id"] {
        b.remove(k);
    }
    Value::Object(b)
}

fn int(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

fn text(sp: &Space, sql: &str) -> String {
    sp.rows(sql)[0][0].as_str().unwrap().to_string()
}

/// Answer every summary the recorder holds with `answer`.
fn settle(sp: &mut Space, answer: &str) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer(answer, "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn create(sp: &mut Space, path: &str, body: &str, summary: &str) -> String {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": body}),
        json!({}),
    ));
    settle(sp, summary);
    a["file"].as_str().expect("a file id").to_string()
}

fn create_dir(sp: &mut Space, path: &str) {
    ok(sp.request(
        "in_write",
        "create_dir",
        None,
        json!({"path": path}),
        json!({}),
    ));
}

/// Every route `./write`, `./derive` and `./ws` sent is one its contract
/// declares; no store fault, no unscoped store op, no traceback.
fn assert_clean(sp: &Space) {
    for cell in ["write", "derive", "ws"] {
        let declared: Vec<String> =
            strings(&cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]);
        let from = format!("./{cell}");
        for m in sp.sent.iter().filter(|m| m["from"] == json!(from)) {
            let route = m["route"].as_str().unwrap_or("");
            assert!(
                declared.iter().any(|d| d == route),
                "{cell} sent {route}, its contract does not declare it"
            );
        }
    }
    let faults: Vec<&String> = sp
        .store_errors
        .iter()
        .filter(|e| !(e.contains("unique_violation") && ANSWERS.iter().any(|i| e.contains(i))))
        .collect();
    assert!(faults.is_empty(), "store faults: {faults:?}");
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert!(
        !sp.stderr.iter().any(|e| e.contains("Traceback")),
        "{:?}",
        sp.stderr
    );
}

/// `/docs/a.md`, `/docs/c.md` (both tagged river and mill), `/docs/deep/b.md`,
/// `/top.txt`, and the empty directories `/docs/empty` and `/void`.
struct Tree {
    sp: Space,
    a: String,
    c: String,
    b: String,
    t: String,
}

fn tree() -> Tree {
    let mut sp = space();
    let a = create(&mut sp, "/docs/a.md", A_MD, SUMMARY_TAGS);
    let c = create(&mut sp, "/docs/c.md", C_MD, SUMMARY_TAGS);
    let b = create(&mut sp, "/docs/deep/b.md", B_MD, SUMMARY);
    let t = create(&mut sp, "/top.txt", TOP, SUMMARY);
    create_dir(&mut sp, "/docs/empty");
    create_dir(&mut sp, "/void");
    Tree { sp, a, c, b, t }
}

/// The nodes of the heads of `files`, as their run rows count them.
fn nodes_of(sp: &Space, files: &[&str]) -> i64 {
    files
        .iter()
        .map(|f| {
            int(
                sp,
                &format!("SELECT nodes FROM node_runs WHERE file = '{f}'"),
            )
        })
        .sum()
}

/// A file child of `dir_info`, from the `files` row.
fn file_child(sp: &Space, file: &str) -> Value {
    let rows = sp.rows(&format!(
        "SELECT path, kind, mime, bytes, lines, head, head_seq, oneline FROM files \
         WHERE file = '{file}'"
    ));
    let r = &rows[0];
    let path = r[0].as_str().unwrap();
    json!({"name": path.rsplit('/').next().unwrap(), "path": path, "type": "file",
           "file": file, "kind": r[1], "mime": r[2], "bytes": r[3], "lines": r[4],
           "version": &r[5].as_str().unwrap()[..12], "changed": stamp(r[6].as_i64().unwrap()),
           "oneline": r[7]})
}

fn paths(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn dir_info_answers_the_row_and_one_page_of_children() {
    if !shipped() {
        return;
    }
    let Tree {
        mut sp, a, c, b, ..
    } = tree();
    let bytes = (A_MD.len() + C_MD.len() + B_MD.len()) as i64;
    let nodes = nodes_of(&sp, &[a.as_str(), c.as_str(), b.as_str()]);
    let deep = json!({"name": "deep", "path": "/docs/deep", "type": "dir", "files": 1,
                      "bytes": B_MD.len(), "nodes": nodes_of(&sp, &[b.as_str()]),
                      "dirty": true, "summary": ""});
    let empty = json!({"name": "empty", "path": "/docs/empty", "type": "dir", "files": 0,
                       "bytes": 0, "nodes": 0, "dirty": true, "summary": ""});
    let got = bare(sp.read_space("dir_info", json!({"path": "/docs/"})));
    let born = text(&sp, "SELECT born FROM dirs WHERE path = '/docs'");
    assert!(!born.is_empty());
    assert_eq!(
        got,
        json!({"path": "/docs", "parent": "/", "files": 3, "bytes": bytes, "nodes": nodes,
               "subdirs": 2, "dirty": true,
               "tags": [{"tag": "mill", "n": 2}, {"tag": "river", "n": 2}],
               "summary": "", "born": born, "next": "",
               "children": [file_child(&sp, &a), file_child(&sp, &c), deep, empty]})
    );

    // Pages by path: `next` is the last path of a page, '' at the end.
    let p1 = ok(sp.read_space("dir_info", json!({"path": "/docs", "limit": 2})));
    assert_eq!(paths(&p1["children"]), vec!["/docs/a.md", "/docs/c.md"]);
    assert_eq!(p1["next"], json!("/docs/c.md"));
    let p2 = ok(sp.read_space(
        "dir_info",
        json!({"path": "/docs", "limit": 2, "cursor": p1["next"]}),
    ));
    assert_eq!(paths(&p2["children"]), vec!["/docs/deep", "/docs/empty"]);
    assert_eq!(p2["next"], json!(""));

    // The root, with its row.
    let root = ok(sp.read_space("dir_info", json!({"path": "/"})));
    assert_eq!(
        (
            root["parent"].clone(),
            root["files"].clone(),
            root["subdirs"].clone()
        ),
        (json!(""), json!(4), json!(2))
    );
    assert_eq!(paths(&root["children"]), vec!["/docs", "/top.txt", "/void"]);

    // Unknown, removed, not a directory path.
    refused(
        sp.read_space("dir_info", json!({"path": "/nope"})),
        "not_found",
    );
    ok(sp.request(
        "in_write",
        "remove_dir",
        None,
        json!({"path": "/docs/empty"}),
        json!({}),
    ));
    refused(
        sp.read_space("dir_info", json!({"path": "/docs/empty"})),
        "not_found",
    );
    let docs = ok(sp.read_space("dir_info", json!({"path": "/docs"})));
    assert_eq!(
        paths(&docs["children"]),
        vec!["/docs/a.md", "/docs/c.md", "/docs/deep"]
    );
    refused(
        sp.read_space("dir_info", json!({"path": "docs"})),
        "bad_request",
    );
    assert_clean(&sp);

    // The root of an empty space has no row and reads as zeros.
    let mut empty = space();
    assert_eq!(
        bare(empty.read_space("dir_info", json!({"path": "/"}))),
        json!({"path": "/", "parent": "", "files": 0, "bytes": 0, "nodes": 0,
               "subdirs": 0, "dirty": false, "tags": [], "summary": "", "born": "",
               "children": [], "next": ""})
    );
    assert_eq!(int(&empty, "SELECT COUNT(*) FROM dirs"), 0);
    assert_clean(&empty);
}

#[test]
fn a_listing_carries_the_directory_rows() {
    if !shipped() {
        return;
    }
    let Tree { mut sp, a, c, b, t } = tree();
    let nodes = nodes_of(&sp, &[a.as_str(), c.as_str(), b.as_str()]);
    let got = ok(sp.read_space("list", json!({"prefix": "/", "depth": 1})));
    assert_eq!(
        got["entries"],
        json!([
            {"path": "/docs/", "dir": true, "files": 3,
             "bytes": A_MD.len() + C_MD.len() + B_MD.len(), "nodes": nodes, "subdirs": 2,
             "dirty": true, "tags": [{"tag": "mill", "n": 2}, {"tag": "river", "n": 2}]},
            {"path": "/top.txt", "file": &t, "bytes": TOP.len(), "lines": 1},
            {"path": "/void/", "dir": true, "files": 0, "bytes": 0, "nodes": 0,
             "subdirs": 0, "dirty": true, "tags": []},
        ]),
        "a directory entry carries its row; an empty one is listed too"
    );
    let got = ok(sp.read_space("list", json!({"prefix": "/docs", "depth": 1})));
    assert_eq!(
        paths(&got["entries"]),
        vec!["/docs/a.md", "/docs/c.md", "/docs/deep/", "/docs/empty/"]
    );
    assert_clean(&sp);
}

#[test]
fn info_names_the_tags_of_its_version() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let a = create(&mut sp, "/n/a.md", A_MD, SUMMARY_TAGS);
    assert_eq!(
        ok(sp.read("info", &a, json!({})))["tags"],
        json!(["river", "mill"])
    );
    let b = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/n/b.md", "text": B_MD}),
        json!({}),
    ))["file"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        ok(sp.read("info", "/n/b.md", json!({})))["tags"],
        json!([]),
        "no summary yet, no tags"
    );
    settle(&mut sp, SUMMARY);
    assert_eq!(
        ok(sp.read("info", &b, json!({})))["tags"],
        json!([]),
        "a summary without a TAGS line"
    );
    assert_clean(&sp);
}

/// The anchors of an `outline` answer, in its order.
fn anchors(got: &Value) -> Vec<String> {
    got["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["anchor"].as_str().unwrap().to_string())
        .collect()
}

/// Five functions, lines 1-2, 5-6, 9-10, 13-14, 17-18.
fn five() -> String {
    (1..=5)
        .map(|i| format!("def f{i}():\n    return {i}\n\n\n"))
        .collect()
}

#[test]
fn a_short_run_row_never_cuts_the_outline() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let file = "fh-0000000000d1";
    let v = sp.seed_text(file, "/m.py", &[five().as_str()], &[])[0].clone();
    // A run row that counts five nodes, and only two node rows: a refused
    // bundle left the version half written (ledger 13).
    sp.db
        .execute(
            "INSERT INTO node_runs (file, version, fmt, parser, mark, nodes, links, fvec, seq) \
             VALUES (?1, ?2, 'python', 'ast', '', 5, 0, '', 1000)",
            [file, v.as_str()],
        )
        .expect("seed the run row");
    for (anchor, from, to) in [("def:f1", 1, 2), ("def:f2", 5, 6)] {
        sp.db
            .execute(
                "INSERT INTO nodes (file, version, anchor, kind, parent, unit, from_pos, to_pos, \
                 oneline, parser, seq) VALUES (?1, ?2, ?3, 'def', '', 'line', ?4, ?5, ?3, 'ast', \
                 1000)",
                rusqlite::params![file, v, anchor, from, to],
            )
            .expect("seed a node row");
    }
    let got = ok(sp.read("outline", file, json!({})));
    assert_eq!(
        anchors(&got),
        vec!["def:f1", "def:f2", "def:f3", "def:f4", "def:f5"],
        "the whole outline, computed from the blocks"
    );
    assert_eq!(got["next"], json!(""));
    let got = ok(sp.read("outline", file, json!({"limit": 2, "cursor": "2"})));
    assert_eq!(anchors(&got), vec!["def:f3", "def:f4"]);
    assert_eq!(got["next"], json!("4"));
    assert_clean(&sp);
}

#[test]
fn an_anchor_of_no_valid_text_answers_with_a_code() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let f = create(&mut sp, "/s.py", &five(), SUMMARY);
    let head = text(&sp, &format!("SELECT head FROM files WHERE file = '{f}'"));
    // `%ED%A0%80` is U+D800 written as UTF-8 -- a lone surrogate's bytes.
    for addr in [
        format!("{f}#def:%ED%A0%80"),
        format!("{f}#def:f1%ED%A0%80"),
        "/s.py#def:%ED%A0%80x".to_string(),
    ] {
        refused(sp.read("read", &addr, json!({})), "unknown_anchor");
    }
    refused(
        sp.request(
            "in_write",
            "write_node",
            Some(&format!("{f}#def:%ED%A0%80")),
            json!({"text": "x\n", "base": &head[..12]}),
            json!({}),
        ),
        "unknown_anchor",
    );
    assert_clean(&sp);
}

/// One tool call on the space's `in_tool` lane, its arguments as the model
/// wrote them (`raw`, JSON text): the `tool_result`s that left for it.
fn tool_call(sp: &mut Space, name: &str, id: &str, raw: &str) -> Vec<Msg> {
    let before = sp.out.len();
    sp.lane(
        "in_tool",
        json!({"tool_caller": "cogny"}),
        json!({"tool_name": name, "tool_call_id": id}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": raw}]}),
    );
    sp.out[before..]
        .iter()
        .filter(|m| m.route() == "tool_result")
        .cloned()
        .collect()
}

/// Ledger line 15 of wave C (review I-2 of GH #947): a `\udXXX` escape in a
/// tool call's text is a lone surrogate after `json.loads`, and every cell
/// wrote it back as the same escape -- an emission no strict parser takes, so
/// the colony refused it and the model never got its `tool_result` (the
/// harness refuses it the same way: `output is not JSON`). Now every
/// emission of `./tools`, `./read` and `./write` leaves strict JSON
/// (`strict_json`, U+FFFD for the lone surrogate): a read and an anchored
/// write by such an address each get exactly one answer, a refusal.
#[test]
fn a_lone_surrogate_in_a_tool_call_still_gets_its_answer() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let f = create(&mut sp, "/s.py", &five(), SUMMARY);
    let head = text(&sp, &format!("SELECT head FROM files WHERE file = '{f}'"));
    let calls = [
        ("file_read", format!(r#"{{"file": "{f}#def:\ud800x"}}"#)),
        (
            "file_write_node",
            format!(
                r#"{{"file": "{f}#def:f1\udfff", "text": "x\n", "base": "{}"}}"#,
                &head[..12]
            ),
        ),
    ];
    for (i, (name, raw)) in calls.iter().enumerate() {
        let id = format!("c-{i}");
        let got = tool_call(&mut sp, name, &id, raw);
        assert_eq!(
            got.len(),
            1,
            "{name}: one tool_result, got {got:?}; {:?}",
            sp.stderr
        );
        assert_eq!(got[0].hop["tool_call_id"], json!(id));
        let msgs = got[0].messages();
        let v: Value = meclaw_core::serde_json::from_str(msgs[0]["text"].as_str().unwrap())
            .expect("the result is strict JSON");
        refused(v.clone(), "unknown_anchor");
        assert!(
            v.to_string().contains('\u{fffd}'),
            "{name}: the lone surrogate comes back as U+FFFD: {v}"
        );
    }
    assert_clean(&sp);
}

/// GH #973 M-2: a `files` row written before `files.dir` existed has none, and
/// nothing backfills it. Its folder finds it all the same -- every reader and
/// writer takes the children by path (`path/` <= p < `path0`), for old and new
/// rows alike: `dir_info` lists it, `dir_summary` reads its one line,
/// `remove_dir` refuses its folder, and a file is never created on top of it.
#[test]
fn old_rows_without_dir_are_children_of_their_folder() {
    if !shipped() {
        return;
    }
    let Tree {
        mut sp, a, c, b, ..
    } = tree();
    sp.db
        .execute_batch(&format!(
            "UPDATE files SET dir = NULL WHERE file IN ('{c}', '{b}');"
        ))
        .expect("two rows of the old stock");
    let docs = ok(sp.read_space("dir_info", json!({"path": "/docs"})));
    assert_eq!(
        docs["children"].as_array().unwrap()[..2].to_vec(),
        vec![file_child(&sp, &a), file_child(&sp, &c)],
        "an old row is a child of its folder"
    );
    let deep = ok(sp.read_space("dir_info", json!({"path": "/docs/deep"})));
    assert_eq!(deep["children"], json!([file_child(&sp, &b)]));
    let listed = ok(sp.read_space("list", json!({"prefix": "/docs/deep"})));
    assert!(listed.to_string().contains("/docs/deep/b.md"), "{listed}");

    // The summary of /docs reads the old row's line.
    let op_id = sp.next_op_id();
    sp.lane(
        "in_read",
        json!({}),
        json!({"op": "dir_summary", "op_id": op_id}),
        json!({"op": "dir_summary", "args": {"path": "/docs"}}),
    );
    let (_, asked) = sp.llm.front().expect("the directory's summary is asked");
    let text = Value::Object(asked.body.clone()).to_string();
    assert!(text.contains("c.md: "), "the old row's one line: {text}");
    settle(&mut sp, SUMMARY);

    // A folder whose counter missed it still holds it.
    sp.db
        .execute_batch("UPDATE dirs SET files = 0 WHERE path = '/docs/deep';")
        .expect("a counter that never counted the old row");
    refused(
        sp.request(
            "in_write",
            "remove_dir",
            None,
            json!({"path": "/docs/deep"}),
            json!({}),
        ),
        "not_empty",
    );
    // Without its directory row the old row still marks its folder taken.
    sp.db
        .execute_batch("DELETE FROM dirs WHERE path = '/docs/deep';")
        .expect("no row of the folder");
    refused(
        sp.request(
            "in_write",
            "create",
            None,
            json!({"path": "/docs/deep", "text": "x\n"}),
            json!({}),
        ),
        "path_taken",
    );
    assert_clean(&sp);
}

/// GH #973 fix round 1 (review M-4): the files of `dir_info` are read from
/// the cursor on, but never from below `path/`. A cursor before the folder
/// (`/` for `/docs`, with more than a window of foreign files between the
/// two) answers the first page, not an empty one without `next`.
#[test]
fn a_cursor_before_the_folder_reads_the_folder() {
    if !shipped() {
        return;
    }
    let Tree { mut sp, .. } = tree();
    sp.db
        .execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 2099) \
             INSERT INTO files (file, path, kind, mime, head, head_seq, tomb, oneline, bytes, \
             lines, dir) \
             SELECT 'fh-b' || i, printf('/b/k%04d.md', i), 'text', 'text/plain', 'h', 1, '', \
             'Foreign.', 1, 1, '/b' FROM n;",
        )
        .expect("a window of foreign rows between / and /docs/");
    let first = ok(sp.read_space("dir_info", json!({"path": "/docs"})));
    let early = ok(sp.read_space("dir_info", json!({"path": "/docs", "cursor": "/"})));
    assert_eq!(
        (early["children"].clone(), early["next"].clone()),
        (first["children"].clone(), first["next"].clone()),
        "a cursor before the folder reads the folder from its start"
    );
    assert_eq!(first["children"].as_array().map(|c| c.len()), Some(4));
}
