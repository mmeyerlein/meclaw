//! GH #947 -- a file changes its place, or gets a twin, and keeps one path.
//! A `move` keeps the head and its nodes and changes the path: exactly ONE
//! `source_changed` with the new path and the unchanged version leaves the
//! space, straight from `./write` without a derive job, the `files` row
//! carries the new path and directory, the main line notes a `move`, and ONE
//! `in_dirs` hands the directory counts to `./derive`'s sync, which moves them
//! from the old directories to the new ones. A `copy` is a new file of the
//! same version: the same block list on rows of its own (R-FH-1), its own main
//! line starting with `copy`, its nodes and event from `./derive`. A place
//! that is taken refuses (`path_taken`), a move onto itself moves nothing and
//! says so, a removed file is not copied, and a directory goes only empty --
//! as a tombstone, never the root.
//!
//! The shipped space in one process (`support/file_space_hive.rs`) with the
//! store's declared indexes applied as the store factory applies them.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const A_PY: &str = "import os\n\nclass A:\n    def m(self):\n        return os.getcwd()\n\n\
def f():\n    return g()\n";
const O_PY: &str = "x = 1\n";

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

fn write(sp: &mut Space, op: &str, file: Option<&str>, args: Value) -> Value {
    sp.request("in_write", op, file, args, json!({}))
}

fn col(sp: &Space, sql: &str) -> Vec<String> {
    sp.rows(sql)
        .into_iter()
        .map(|r| {
            r[0].as_str()
                .map(str::to_string)
                .unwrap_or_else(|| r[0].to_string())
        })
        .collect()
}

fn head_of(sp: &Space, file: &str) -> String {
    col(sp, &format!("SELECT head FROM files WHERE file = '{file}'"))[0].clone()
}

fn nodes_of(sp: &Space, file: &str) -> i64 {
    sp.rows(&format!(
        "SELECT nodes FROM node_runs WHERE file = '{file}'"
    ))[0][0]
        .as_i64()
        .unwrap()
}

/// Answer every summary the recorder holds.
fn settle(sp: &mut Space) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn create(sp: &mut Space, path: &str, text: &str) -> (String, String) {
    let a = ok(write(
        sp,
        "create",
        None,
        json!({"path": path, "text": text}),
    ));
    settle(sp);
    let file = a["file"].as_str().expect("a file id").to_string();
    let head = head_of(sp, &file);
    (file, head)
}

/// The living directories: `[path, files, bytes, nodes]`, by path.
fn dirs(sp: &Space) -> Vec<Vec<Value>> {
    sp.rows("SELECT path, files, bytes, nodes FROM dirs WHERE tomb = '' ORDER BY path")
}

fn claims(sp: &Space) -> Vec<Vec<Value>> {
    sp.rows("SELECT path, op_id FROM claims ORDER BY path")
}

/// The `source_changed` events that left since `before`, each with the route
/// alone in its head.
fn events_since(sp: &Space, before: usize) -> Vec<Value> {
    sp.out[before..]
        .iter()
        .filter(|m| m.route() == "source_changed")
        .map(|m| {
            assert_eq!(
                Value::Object(m.hop.clone()),
                json!({"route": "source_changed"}),
                "the head carries the route alone"
            );
            let mut b = m.body.clone();
            b.remove("messages");
            Value::Object(b)
        })
        .collect()
}

/// What `./write` sent on `route` since `before` (an index into `sent`).
fn from_write(sp: &Space, before: usize, route: &str) -> Vec<Value> {
    sp.sent[before..]
        .iter()
        .filter(|m| m["from"] == json!("./write") && m["route"] == json!(route))
        .cloned()
        .collect()
}

/// Every route `./write`, `./derive` and `./ws` sent is one its contract
/// declares and the rim names `source_changed`; no store fault, no unscoped
/// store op, no traceback, no claim left.
fn assert_clean(sp: &Space) {
    for cell in ["write", "derive", "ws"] {
        let declared: Vec<String> =
            strings(&cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]);
        assert!(declared.contains(&"source_changed".to_string()), "{cell}");
        let from = format!("./{cell}");
        for m in sp.sent.iter().filter(|m| m["from"] == json!(from)) {
            let route = m["route"].as_str().unwrap_or("");
            assert!(
                declared.iter().any(|d| d == route),
                "{cell} sent {route}, its contract does not declare it"
            );
        }
    }
    let rim: Vec<String> = hive_config()["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect();
    assert!(rim.contains(&"source_changed".to_string()), "{rim:?}");
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
    assert_eq!(claims(sp), Vec::<Vec<Value>>::new(), "every claim dropped");
}

#[test]
fn a_move_sends_one_event_and_the_counts_follow() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let (a, h) = create(&mut sp, "/lib/a.py", A_PY);
    let (o, _) = create(&mut sp, "/lib/o.py", O_PY);
    let (na, no) = (nodes_of(&sp, &a), nodes_of(&sp, &o));
    let both = (A_PY.len() + O_PY.len()) as i64;
    assert_eq!(
        dirs(&sp),
        vec![
            vec![json!("/"), json!(2), json!(both), json!(na + no)],
            vec![json!("/lib"), json!(2), json!(both), json!(na + no)],
        ]
    );

    let (n, s) = (sp.out.len(), sp.sent.len());
    let r = ok(write(&mut sp, "move", Some(&a), json!({"to": "/src/b.py"})));
    for (k, v) in [
        ("file", json!(&a)),
        ("version", json!(&h[..12])),
        ("path", json!("/src/b.py")),
        ("from", json!("/lib/a.py")),
        ("moved", json!(true)),
    ] {
        assert_eq!(r[k], v, "{k}: {r}");
    }
    assert_eq!(head_of(&sp, &a), h, "the head stays");
    let run = sp.rows(&format!(
        "SELECT fmt, parser, mark, nodes, links FROM node_runs WHERE file = '{a}'"
    ));
    assert_eq!(run.len(), 1, "one run row for the head: {run:?}");
    assert_eq!(
        events_since(&sp, n),
        vec![
            json!({"source": &a, "version": &h[..12], "path": "/src/b.py",
                    "fmt": run[0][0].clone(), "parser": run[0][1].clone(),
                    "mark": run[0][2].clone(), "nodes": run[0][3].clone(),
                    "links": run[0][4].clone(), "tomb": false})
        ],
        "exactly one event: the new path, the unchanged version"
    );
    assert_eq!(
        sp.rows(&format!("SELECT path, dir FROM files WHERE file = '{a}'")),
        vec![vec![json!("/src/b.py"), json!("/src")]]
    );
    assert_eq!(
        sp.rows(&format!(
            "SELECT op, version, prev, note FROM line WHERE file = '{a}' \
             ORDER BY seq DESC LIMIT 1"
        )),
        vec![vec![
            json!("move"),
            json!(&h),
            json!(&h),
            json!("/lib/a.py")
        ]]
    );
    let to_dirs = from_write(&sp, s, "in_dirs");
    assert_eq!(to_dirs.len(), 1, "one in_dirs: {to_dirs:?}");
    assert_eq!(to_dirs[0]["to"], json!("./derive"));
    assert_eq!(to_dirs[0]["body"]["file"], json!(&a));
    assert!(
        from_write(&sp, s, "in_derive").is_empty(),
        "a move derives nothing"
    );
    assert!(sp.llm.is_empty(), "a move asks no model");

    // The sync of `./derive` moved the counts with the file.
    assert_eq!(
        dirs(&sp),
        vec![
            vec![json!("/"), json!(2), json!(both), json!(na + no)],
            vec![json!("/lib"), json!(1), json!(O_PY.len() as i64), json!(no)],
            vec![json!("/src"), json!(1), json!(A_PY.len() as i64), json!(na)],
        ]
    );
    assert_eq!(
        sp.rows(&format!(
            "SELECT path, version FROM contrib WHERE file = '{a}'"
        )),
        vec![vec![json!("/src/b.py"), json!(&h)]]
    );

    // The old place is free again.
    create(&mut sp, "/lib/a.py", "y = 2\n");
    assert_clean(&sp);
}

#[test]
fn a_copy_is_a_new_file_of_the_same_version() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let (f, h) = create(&mut sp, "/c/a.py", A_PY);
    let line_of_f = format!("SELECT seq, op FROM line WHERE file = '{f}' ORDER BY seq");
    let lines_before = sp.rows(&line_of_f);

    let (n, s) = (sp.out.len(), sp.sent.len());
    let r = ok(write(
        &mut sp,
        "copy",
        Some("/c/a.py"),
        json!({"to": "/c/b.py"}),
    ));
    let g = r["file"].as_str().expect("the copy's id").to_string();
    assert!(
        g != f && g.starts_with("fh-") && g.len() == 15,
        "a new file: {g}"
    );
    assert_eq!(
        (r["version"].clone(), r["path"].clone(), r["from"].clone()),
        (json!(&h[..12]), json!("/c/b.py"), json!(&f))
    );
    assert_eq!(head_of(&sp, &g), h, "the same version");
    assert_eq!(
        sp.rows(&format!(
            "SELECT made_by, blocks FROM versions WHERE file = '{g}'"
        )),
        vec![vec![
            json!("copy"),
            sp.rows(&format!("SELECT blocks FROM versions WHERE file = '{f}'"))[0][0].clone()
        ]],
        "the same block list"
    );
    let blocks = |file: &str| {
        let mut b = sp.rows(&format!(
            "SELECT hash, enc, body FROM blocks WHERE file = '{file}'"
        ));
        b.sort_by_key(|r| r[0].to_string());
        b
    };
    assert_eq!(blocks(&g), blocks(&f), "rows of its own, the same hashes");
    assert_eq!(
        sp.rows(&format!(
            "SELECT op, version, prev, note FROM line WHERE file = '{g}' ORDER BY seq"
        )),
        vec![vec![json!("copy"), json!(&h), json!(""), json!(&f)]],
        "its own main line, starting with the copy"
    );
    assert_eq!(sp.rows(&line_of_f), lines_before, "the source's line stays");
    assert_eq!(
        sp.rows(&format!(
            "SELECT path, dir, kind, mime, bytes, lines FROM files WHERE file = '{g}'"
        )),
        vec![vec![
            json!("/c/b.py"),
            json!("/c"),
            json!("text"),
            json!("text/x-python"),
            json!(A_PY.len() as i64),
            json!(line_count(A_PY.as_bytes()))
        ]]
    );
    let derive: Vec<Value> = from_write(&sp, s, "in_derive")
        .iter()
        .map(|m| m["body"]["file"].clone())
        .collect();
    assert_eq!(
        derive,
        vec![json!(&g)],
        "the copy's nodes are ./derive's job"
    );
    let ev: Vec<Value> = events_since(&sp, n)
        .iter()
        .map(|e| json!([e["source"], e["path"], e["version"]]))
        .collect();
    assert_eq!(
        ev,
        vec![json!([&g, "/c/b.py", &h[..12]])],
        "one event, from ./derive"
    );
    let raw = ok(sp.read("raw", &g, json!({})));
    assert_eq!(raw["b64"], json!(b64(A_PY.as_bytes())));
    settle(&mut sp);
    assert_eq!(
        dirs(&sp).last().cloned().unwrap()[..3].to_vec(),
        vec![json!("/c"), json!(2), json!(2 * A_PY.len() as i64)]
    );
    assert_clean(&sp);
}

#[test]
fn a_taken_place_refuses_and_an_empty_directory_goes() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    let (a, h) = create(&mut sp, "/lib/a.py", A_PY);
    let (o, ho) = create(&mut sp, "/lib/o.py", O_PY);

    // A move onto a living file.
    let e = refused(
        write(&mut sp, "move", Some(&a), json!({"to": "/lib/o.py"})),
        "path_taken",
    );
    assert_eq!(e["error"]["file"], json!(&o));

    // A move onto itself moves nothing and says so, without an event.
    let (n, s) = (sp.out.len(), sp.sent.len());
    let r = ok(write(&mut sp, "move", Some(&a), json!({"to": "/lib/a.py"})));
    for (k, v) in [
        ("file", json!(&a)),
        ("version", json!(&h[..12])),
        ("path", json!("/lib/a.py")),
        ("from", json!("/lib/a.py")),
        ("moved", json!(false)),
    ] {
        assert_eq!(r[k], v, "{k}: {r}");
    }
    assert!(events_since(&sp, n).is_empty(), "no move, no event");
    assert!(from_write(&sp, s, "in_dirs").is_empty());

    // A removed file is not copied.
    ok(write(
        &mut sp,
        "remove",
        Some(&o),
        json!({"base": &ho[..12]}),
    ));
    refused(
        write(&mut sp, "copy", Some(&o), json!({"to": "/lib/z.py"})),
        "not_found",
    );

    // A directory over a file, a directory with content, the root.
    let e = refused(
        write(&mut sp, "create_dir", None, json!({"path": "/lib/a.py"})),
        "path_taken",
    );
    assert_eq!(e["error"]["file"], json!(&a));
    refused(
        write(&mut sp, "remove_dir", None, json!({"path": "/lib"})),
        "not_empty",
    );
    refused(
        write(&mut sp, "remove_dir", None, json!({"path": "/"})),
        "bad_request",
    );

    // An empty directory goes as a tombstone; its parent then goes too.
    let r = ok(write(&mut sp, "create_dir", None, json!({"path": "/e/f"})));
    assert_eq!(r["created"], json!(true));
    refused(
        write(&mut sp, "remove_dir", None, json!({"path": "/e"})),
        "not_empty",
    );
    let r = ok(write(&mut sp, "remove_dir", None, json!({"path": "/e/f"})));
    assert_eq!(r["removed"], json!(true));
    let tomb = col(&sp, "SELECT tomb FROM dirs WHERE path = '/e/f'");
    assert!(
        tomb.len() == 1 && !tomb[0].is_empty(),
        "a tombstone: {tomb:?}"
    );
    refused(
        sp.read_space("dir_info", json!({"path": "/e/f"})),
        "not_found",
    );
    refused(
        write(&mut sp, "remove_dir", None, json!({"path": "/e/f"})),
        "not_found",
    );
    let r = ok(write(&mut sp, "remove_dir", None, json!({"path": "/e"})));
    assert_eq!(r["removed"], json!(true));
    assert_eq!(
        col(&sp, "SELECT path FROM dirs WHERE tomb = '' ORDER BY path"),
        vec!["/", "/lib"]
    );
    settle(&mut sp);
    assert_clean(&sp);
}
