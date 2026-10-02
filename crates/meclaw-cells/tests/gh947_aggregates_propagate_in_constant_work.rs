//! GH #947 -- a directory is the sum of the files under it, and a change of
//! one file moves exactly its own ancestors. Every `dirs` row holds counters
//! (`files`, `bytes`, `nodes`, the bit counts of the file vectors `vec_counts`
//! over `vec_n` files, the topic counts `tags`); a file's contribution is what
//! the store says of it NOW (path, head, bytes, the head's node run and tags),
//! `contrib` holds the one last counted in, and the sync job of
//! `file-space/derive` moves only the difference: one claim on
//! `contrib.seq`, then ONE bundle with exactly one compare-and-swap `update`
//! per ancestor. A sibling is never read.
//!
//! Why: a directory listing that recounts its subtree costs work in the size
//! of the tree on every write; the delta costs work in the depth of the path.
//! The counters must still be exact, so every lock here recomputes them from
//! the files' own rows (`recount`) and compares row by row.
//!
//! The pure half (`agg_delta`, `agg_apply`) runs as tables over the shipped
//! `script_inline`; the road runs through the shipped space in one process
//! (`support/file_space_hive.rs`): every script and edge the shipped one, the
//! store the store cell's dispatcher, the summarizer the harness's recorder,
//! `./embed` the shipped code cell against an HTTP stub on 127.0.0.1 run by
//! this file (no provider, no net).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

const DIM: usize = 16;
/// The summarizer's answer: its last line names two topics, one twice.
const SUM: &str = "One line of the file.\n\nA short paragraph.\n\nTAGS: Parser, tree, parser";
const X1: &str = "import os\n\ndef f():\n    return os.getcwd()\n";
const X2: &str = "import os\n\ndef f():\n    return os.getcwd()\n\ndef g():\n    return f()\n";
const Z1: &str = "def z():\n    return 0\n";
const W1: &str = "def w():\n    return 1\n";

fn all_shipped() -> bool {
    shipped()
        && repo("templates/file-space/embed/config.json").is_file()
        && repo("templates/file-space/summarizer/config.json").is_file()
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1: the same vector
/// for every input, so a file's vector (the bitwise majority of its
/// sections') is exactly those bits.
fn stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
    let url = format!("http://{}/v1/embeddings", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let req: Value = sj::from_slice(&body).unwrap_or(Value::Null);
            let n = req["input"].as_array().map(Vec::len).unwrap_or(0);
            let vector: Vec<f64> = (0..DIM)
                .map(|i| if i % 3 == 0 { 1.0 } else { -1.0 })
                .collect();
            let data: Vec<Value> = (0..n)
                .map(|i| json!({"index": i, "embedding": vector}))
                .collect();
            let out = json!({"data": data, "usage": {"prompt_tokens": 3}}).to_string();
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    url
}

/// The shipped space with `./embed` at the stub.
fn space(url: &str) -> Space {
    Space::with(
        "/x/files",
        &[
            ("embed", "endpoint", json!(url)),
            ("embed", "model", json!("stub-embed")),
            ("embed", "dim", json!(DIM.to_string())),
        ],
    )
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn head_of(sp: &Space, file: &str) -> String {
    text(&sp.rows(&format!("SELECT head FROM files WHERE file = '{file}'"))[0][0])
}

fn create(sp: &mut Space, path: &str, body: &str) -> String {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": body}),
        json!({}),
    ));
    text(&a["file"])
}

fn overwrite(sp: &mut Space, file: &str, body: &str) {
    let base = head_of(sp, file);
    ok(sp.request(
        "in_write",
        "overwrite",
        Some(file),
        json!({"text": body, "base": &base[..12]}),
        json!({}),
    ));
}

/// Answer every summary the recorder holds, in order.
fn settle(sp: &mut Space, finish: &str) {
    while sp.llm.front().is_some_and(|(c, _)| c == "summarizer") {
        sp.llm_answer(SUM, finish);
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

/// The store's refusals, less the unique keys the sync job and the path
/// claim are built to meet: a lost insert there says "somebody was first",
/// it is no failure (GH #947). The harness records every leg's code.
fn refusals(sp: &Space) -> Vec<String> {
    sp.store_errors
        .iter()
        .filter(|e| {
            let Some((_, leg)) = e.split_once(": unique_violation: ") else {
                return true;
            };
            let leg: Value = sj::from_str(leg).unwrap_or(Value::Null);
            let id = leg["tool_call_id"].as_str().unwrap_or("");
            !(id.starts_with("s-claim") || id.starts_with("di-") || id == "claim")
        })
        .cloned()
        .collect()
}

fn assert_clean(sp: &Space) {
    assert_eq!(refusals(sp), Vec::<String>::new(), "no store refusal");
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE op_id LIKE 's-%' OR op_id LIKE 'd-%'"),
        vec![vec![json!(0)]],
        "no job left parked"
    );
    for e in &sp.stderr {
        for bad in ["Traceback", "dropped", "lost its", "given up", "drift"] {
            assert!(!e.contains(bad), "{e}");
        }
    }
}

// ------------------------------------------------------------ the recount

fn dir_of(path: &str) -> String {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(i) if i > 0 => p[..i].to_string(),
        _ => "/".to_string(),
    }
}

fn ancestors(dir: &str) -> Vec<String> {
    let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    let mut out = vec!["/".to_string()];
    for i in 1..=segs.len() {
        out.push(format!("/{}", segs[..i].join("/")));
    }
    out
}

/// The sign bits of a file vector (standard base64), most significant first.
fn bits(encoded: &str) -> Vec<i64> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut acc, mut n, mut bytes) = (0u32, 0u32, Vec::new());
    for c in encoded.bytes().filter(|c| *c != b'=') {
        let v = A.iter().position(|x| *x == c).expect("a base64 digit") as u32;
        acc = (acc << 6) | v;
        n += 6;
        if n >= 8 {
            n -= 8;
            bytes.push((acc >> n) as u8);
            acc &= (1 << n) - 1;
        }
    }
    bytes
        .iter()
        .flat_map(|b| (0..8u32).map(move |k| i64::from((b >> (7 - k)) & 1)))
        .collect()
}

#[derive(Default)]
struct Sum {
    files: i64,
    bytes: i64,
    nodes: i64,
    vec_n: i64,
    vec_counts: Vec<i64>,
    tags: BTreeMap<String, i64>,
}

/// The counter-check: every directory recomputed from the files' own rows
/// -- `files` (path, head, bytes), the head's `node_runs` (nodes, file
/// vector) and its `summaries` of level `tags` -- the sum the sync job must
/// reach without ever reading a sibling.
fn recount(sp: &Space) -> BTreeMap<String, Value> {
    let mut want: BTreeMap<String, Sum> = BTreeMap::new();
    for r in sp.rows("SELECT file, path, head, tomb, bytes FROM files ORDER BY file") {
        let (file, path, head) = (text(&r[0]), text(&r[1]), text(&r[2]));
        if head.is_empty() || !text(&r[3]).is_empty() {
            continue;
        }
        let run = sp.rows(&format!(
            "SELECT nodes, fvec FROM node_runs WHERE file = '{file}' AND version = '{head}'"
        ));
        let (nodes, fvec) = run
            .first()
            .map(|x| (x[0].as_i64().unwrap_or(0), text(&x[1])))
            .unwrap_or((0, String::new()));
        let tags: Vec<String> = sp
            .rows(&format!(
                "SELECT text FROM summaries WHERE file = '{file}' AND version = '{head}' \
                 AND level = 'tags' ORDER BY at DESC LIMIT 1"
            ))
            .first()
            .map(|x| sj::from_str::<Vec<String>>(&text(&x[0])).expect("a tag list"))
            .unwrap_or_default();
        let b = bits(&fvec);
        for p in ancestors(&dir_of(&path)) {
            let e = want.entry(p).or_default();
            e.files += 1;
            e.bytes += r[4].as_i64().unwrap_or(0);
            e.nodes += nodes;
            if !b.is_empty() {
                e.vec_n += 1;
                if e.vec_counts.len() < b.len() {
                    e.vec_counts.resize(b.len(), 0);
                }
                for (c, x) in e.vec_counts.iter_mut().zip(&b) {
                    *c += x;
                }
            }
            for t in &tags {
                *e.tags.entry(t.clone()).or_default() += 1;
            }
        }
    }
    want.into_iter()
        .map(|(p, s)| {
            (
                p,
                json!({"files": s.files, "bytes": s.bytes, "nodes": s.nodes, "vec_n": s.vec_n,
                       "vec_counts": s.vec_counts, "tags": s.tags}),
            )
        })
        .collect()
}

/// Every `dirs` row as its counters.
fn dirs(sp: &Space) -> BTreeMap<String, Value> {
    sp.rows("SELECT path, files, bytes, nodes, vec_n, vec_counts, tags FROM dirs")
        .into_iter()
        .map(|r| {
            let vc: Value = sj::from_str(&text(&r[5])).expect("vec_counts is a JSON list");
            let tags: Value = sj::from_str(&text(&r[6])).expect("tags is a JSON object");
            (
                text(&r[0]),
                json!({"files": r[1].clone(), "bytes": r[2].clone(), "nodes": r[3].clone(),
                       "vec_n": r[4].clone(), "vec_counts": vc, "tags": tags}),
            )
        })
        .collect()
}

/// Every directory row is the recount; a row with no living file under it
/// (directories never vanish) holds zeros.
fn assert_dirs(sp: &Space, when: &str) -> BTreeMap<String, Value> {
    let want = recount(sp);
    let got = dirs(sp);
    let zero = json!({"files": 0, "bytes": 0, "nodes": 0, "vec_n": 0, "vec_counts": [],
                      "tags": {}});
    for p in want.keys().chain(got.keys()) {
        assert_eq!(
            got.get(p),
            Some(want.get(p).unwrap_or(&zero)),
            "{when}: directory {p}"
        );
    }
    got
}

/// Every store bundle `./derive` sent since `since`: `(job, legs)`.
fn derive_bundles(sp: &Space, since: usize) -> Vec<(String, Vec<(String, Value)>)> {
    sp.sent[since..]
        .iter()
        .filter(|m| m["from"] == json!("./derive") && m["to"] == json!("./store"))
        .map(|m| {
            let legs = m["body"]["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|c| c["type"] == json!("tool_call"))
                .map(|c| {
                    (
                        text(&c["id"]),
                        sj::from_str::<Value>(c["text"].as_str().unwrap_or("null"))
                            .unwrap_or(Value::Null),
                    )
                })
                .collect();
            (text(&m["header"]["context"]["cur_job"]), legs)
        })
        .collect()
}

fn dirs_update(a: &Value) -> bool {
    a["operation"] == json!("update") && a["table"] == json!("dirs")
}

// ------------------------------------------------------------ pure tables

#[test]
fn the_delta_of_a_contribution_names_every_ancestor_and_nothing_else() {
    if !shipped() {
        return;
    }
    let c = |path: &str, v: &str, size: i64, nodes: i64, vec: &str, tags: Value| {
        json!({"file": "fh-000000000001", "path": path, "version": v, "bytes": size,
               "nodes": nodes, "vec": vec, "tags": tags})
    };
    // Two bytes of sign bits: 1001 0010 0100 1001.
    let one = b64(&[0b1001_0010, 0b0100_1001]);
    let a = c("/a/b/x.py", "v1", 10, 2, "", json!(["p"]));
    let got = pure(
        "derive",
        "{k: agg_delta(o, n) for k, (o, n) in ARGS.items()}",
        json!({
            "equal": [a, a],
            "change": [a, c("/a/b/x.py", "v2", 15, 3, "", json!(["p"]))],
            "move": [a, c("/a/z/x.py", "v1", 10, 2, "", json!(["p"]))],
            "remove": [a, null],
            "birth": [null, a],
            "vector": [a, c("/a/b/x.py", "v1", 10, 2, one.as_str(), json!(["p"]))],
            "tags": [a, c("/a/b/x.py", "v1", 10, 2, "", json!(["q", "r"]))],
            "version": [a, c("/a/b/x.py", "v9", 10, 2, "", json!(["p"]))],
        }),
    );
    let z = |over: Value| {
        let mut d = json!({"files": 0, "bytes": 0, "nodes": 0, "vec": [], "vec_n": 0, "tags": {}});
        for (k, v) in obj(over) {
            d[k] = v;
        }
        d
    };
    assert_eq!(got["equal"], json!({}), "equal contributions move nothing");
    for p in ["/", "/a", "/a/b"] {
        assert_eq!(
            got["change"][p],
            z(json!({"bytes": 5, "nodes": 1})),
            "change {p}"
        );
        assert_eq!(
            got["remove"][p],
            z(json!({"files": -1, "bytes": -10, "nodes": -2, "tags": {"p": -1}})),
            "remove {p}"
        );
        assert_eq!(
            got["birth"][p],
            z(json!({"files": 1, "bytes": 10, "nodes": 2, "tags": {"p": 1}})),
            "birth {p}"
        );
        assert_eq!(
            got["vector"][p],
            z(json!({"vec": [1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1], "vec_n": 1})),
            "vector {p}"
        );
        assert_eq!(
            got["tags"][p],
            z(json!({"tags": {"p": -1, "q": 1, "r": 1}})),
            "tags {p}"
        );
        // A new version with the same counts still names every ancestor:
        // each of them is dirty from now on (its summary is stale).
        assert_eq!(got["version"][p], z(json!({})), "version {p}");
    }
    let keys = |k: &str| -> Vec<String> { obj(got[k].clone()).keys().cloned().collect() };
    assert_eq!(
        keys("change"),
        vec!["/", "/a", "/a/b"],
        "only the ancestors"
    );
    assert_eq!(keys("move"), vec!["/", "/a", "/a/b", "/a/z"]);
    assert_eq!(got["move"]["/"], z(json!({})), "a move under / nets out");
    assert_eq!(got["move"]["/a"], z(json!({})));
    assert_eq!(
        got["move"]["/a/b"],
        z(json!({"files": -1, "bytes": -10, "nodes": -2, "tags": {"p": -1}}))
    );
    assert_eq!(
        got["move"]["/a/z"],
        z(json!({"files": 1, "bytes": 10, "nodes": 2, "tags": {"p": 1}}))
    );
}

#[test]
fn a_delta_applies_to_a_row_and_never_below_zero() {
    if !shipped() {
        return;
    }
    let got = pure(
        "derive",
        "[agg_apply(r, d) for r, d in ARGS]",
        json!([
            [{"files": 2, "bytes": 30, "nodes": 4, "vec_counts": "[1, 0]", "vec_n": 1,
              "tags": "{\"p\": 2}"},
             {"files": -1, "bytes": -10, "nodes": -2, "vec": [-1, 0], "vec_n": -1,
              "tags": {"p": -1}}],
            [null,
             {"files": 1, "bytes": 10, "nodes": 2, "vec": [1, 0, 1], "vec_n": 1,
              "tags": {"q": 1}}],
            [{"files": 1, "bytes": 5, "nodes": 0, "vec_counts": "[]", "vec_n": 0, "tags": "{}"},
             {"files": -3, "bytes": -9, "nodes": -1, "vec": [], "vec_n": -1, "tags": {"x": -1}}],
            [{"files": 3, "bytes": 3, "nodes": 3, "vec_counts": "[2, 1]", "vec_n": 2,
              "tags": "{\"a\": 1}"},
             {"files": 0, "bytes": 0, "nodes": 0, "vec": [1, 1, 1, 1], "vec_n": 1,
              "tags": {"b": 2}}]
        ]),
    );
    assert_eq!(
        got[0],
        json!({"files": 1, "bytes": 20, "nodes": 2, "vec_counts": "[]", "vec_n": 0,
               "tags": "{\"p\": 1}"}),
        "the last vector leaves: no counts"
    );
    assert_eq!(
        got[1],
        json!({"files": 1, "bytes": 10, "nodes": 2, "vec_counts": "[1, 0, 1]", "vec_n": 1,
               "tags": "{\"q\": 1}"}),
        "a missing row is the empty one"
    );
    assert_eq!(
        got[2],
        json!({"files": 0, "bytes": 0, "nodes": 0, "vec_counts": "[]", "vec_n": 0,
               "tags": "{}"}),
        "never below zero, a counted-out tag goes"
    );
    assert_eq!(
        got[3],
        json!({"files": 3, "bytes": 3, "nodes": 3, "vec_counts": "[3, 2, 1, 1]", "vec_n": 3,
               "tags": "{\"a\": 1, \"b\": 2}"}),
        "a wider vector pads the counts"
    );
}

// ------------------------------------------------------------ through the hive

#[test]
fn a_change_moves_one_bundle_of_four_counters_and_reads_no_sibling() {
    if !all_shipped() {
        return;
    }
    let url = stub();
    let mut sp = space(&url);
    let x = create(&mut sp, "/a/b/c/x.py", X1);
    settle(&mut sp, "stop");
    let z = create(&mut sp, "/a/z.py", Z1);
    settle(&mut sp, "stop");
    let got = assert_dirs(&sp, "born");
    assert_eq!(
        got.keys().cloned().collect::<Vec<_>>(),
        vec!["/", "/a", "/a/b", "/a/b/c"],
        "a directory is born with the first file under it"
    );
    assert_eq!(
        (got["/"]["files"].clone(), got["/"]["vec_n"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(got["/"]["tags"], json!({"parser": 2, "tree": 2}));
    assert_eq!(got["/a/b/c"]["tags"], json!({"parser": 1, "tree": 1}));
    assert_eq!(
        got["/a/b/c"]["vec_counts"].as_array().map(Vec::len),
        Some(DIM),
        "one count per bit of the file vector"
    );

    // The change: the sync round sends ONE bundle with exactly four updates,
    // one per ancestor of x, each a compare-and-swap on its own row.
    let n = sp.sent.len();
    overwrite(&mut sp, &x, X2);
    settle(&mut sp, "stop");
    let got = assert_dirs(&sp, "changed");
    assert_eq!(
        (got["/a/b/c"]["nodes"].clone(), got["/"]["nodes"].clone()),
        (json!(2), json!(3))
    );
    let all = derive_bundles(&sp, n);
    let applies: Vec<&Vec<(String, Value)>> = all
        .iter()
        .filter(|(job, legs)| job.starts_with("s-") && legs.iter().any(|(_, a)| dirs_update(a)))
        .map(|(_, legs)| legs)
        .collect();
    assert_eq!(applies.len(), 1, "one bundle moves the counters: {all:?}");
    let mut moved: Vec<String> = Vec::new();
    for (_, a) in applies[0].iter().filter(|(_, a)| dirs_update(a)) {
        let mut keys: Vec<String> = obj(a["where"].clone()).keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["changed_seq", "path"], "a compare-and-swap: {a}");
        moved.push(text(&a["where"]["path"]));
    }
    moved.sort();
    assert_eq!(
        moved,
        vec!["/", "/a", "/a/b", "/a/b/c"],
        "exactly the ancestors"
    );

    // No leg of the derive and sync jobs names the sibling or reaches past
    // the ancestors: constant work in the depth of the path.
    let anc = ["/", "/a", "/a/b", "/a/b/c"];
    for (_, legs) in &all {
        for (id, a) in legs {
            assert!(
                !strings(a)
                    .iter()
                    .any(|s| s.contains(&z) || s.contains("/a/z.py")),
                "{id} names the sibling: {a}"
            );
            if a["table"] == json!("files") && a["operation"] == json!("select") {
                assert_eq!(
                    a["where"]["file"],
                    json!(x),
                    "{id}: `files` by the file: {a}"
                );
            }
            if a["table"] == json!("dirs") {
                let p = if a["operation"] == json!("insert") {
                    &a["row"]["path"]
                } else {
                    &a["where"]["path"]
                };
                let named: Vec<String> = match p.get("in") {
                    Some(list) => list.as_array().unwrap().iter().map(text).collect(),
                    None => vec![text(p)],
                };
                assert!(
                    named.iter().all(|q| anc.contains(&q.as_str())),
                    "{id} reaches past the ancestors: {a}"
                );
            }
        }
    }
    // Every touched row is dirty, its `changed_seq` one up per move.
    assert_eq!(
        sp.rows("SELECT path, dirty, changed_seq FROM dirs ORDER BY path"),
        vec![
            vec![json!("/"), json!(1), json!(3)],
            vec![json!("/a"), json!(1), json!(3)],
            vec![json!("/a/b"), json!(1), json!(2)],
            vec![json!("/a/b/c"), json!(1), json!(2)],
        ]
    );
    assert_clean(&sp);
}

#[test]
fn a_failed_summary_counts_without_tags_and_a_removal_counts_out() {
    if !all_shipped() {
        return;
    }
    let url = stub();
    let mut sp = space(&url);
    let x = create(&mut sp, "/a/b/c/x.py", X1);
    settle(&mut sp, "stop");
    create(&mut sp, "/a/z.py", Z1);
    settle(&mut sp, "stop");

    // The summary of w fails (finish != stop): the job stops after its node
    // rows, and the file still counts -- without tags, and without the
    // vector its failed job never wrote.
    let w = create(&mut sp, "/a/b/w.py", W1);
    settle(&mut sp, "error");
    assert_eq!(
        sp.rows(&format!(
            "SELECT COUNT(*) FROM summaries WHERE file = '{w}'"
        )),
        vec![vec![json!(0)]],
        "the summary was refused"
    );
    assert_eq!(
        sp.rows(&format!("SELECT tags, vec FROM contrib WHERE file = '{w}'")),
        vec![vec![json!("[]"), json!("")]]
    );
    let got = assert_dirs(&sp, "a failed summary");
    assert_eq!(
        (
            got["/a/b"]["files"].clone(),
            got["/a/b"]["tags"].clone(),
            got["/a/b"]["vec_n"].clone()
        ),
        (json!(2), json!({"parser": 1, "tree": 1}), json!(1)),
        "w counts as a file, its topics and vector do not"
    );

    // A removal: the counters of every ancestor go down, the rows stay.
    let before = got;
    let base = head_of(&sp, &x);
    ok(sp.request(
        "in_write",
        "remove",
        Some(&x),
        json!({"base": &base[..12]}),
        json!({}),
    ));
    let got = assert_dirs(&sp, "removed");
    assert_eq!(
        got["/a/b/c"],
        json!({"files": 0, "bytes": 0, "nodes": 0, "vec_n": 0, "vec_counts": [], "tags": {}}),
        "a directory never vanishes; its counters go to zero"
    );
    assert_eq!(
        got["/"]["files"].as_i64().unwrap(),
        before["/"]["files"].as_i64().unwrap() - 1
    );
    assert_eq!(
        got["/"]["tags"],
        json!({"parser": 1, "tree": 1}),
        "the sibling's topics alone"
    );
    assert_eq!(
        sp.rows(&format!("SELECT COUNT(*) FROM contrib WHERE file = '{x}'")),
        vec![vec![json!(0)]],
        "the counted contribution goes with the file"
    );
    assert_clean(&sp);
}

// ------------------------------------------------------------ the door (review I-1)

/// The routing decisions one segment may spend (GH #929): the colony budget
/// less the reserve for nesting and stages still to come.
const SEGMENT_MAX: i64 = TTL - 16;

fn ws(sp: &mut Space, op: &str, name: &str, args: Value) -> Value {
    let hop = if name.is_empty() {
        json!({})
    } else {
        json!({"ws": name})
    };
    ok(sp.request("in_ws", op, None, args, hop))
}

/// Review I-1 of GH #947: the directory sync runs behind the derive job of a
/// workspace commit, and that road (commit, derive, embedding, summary) leaves
/// the sync almost no budget -- 47 of 48 routing decisions measured in the
/// strand's model, and every lost compare-and-swap on `/` (the one row every
/// sync of the space shares) costs two more. So the sync enters `./derive`
/// again on `in_dirs` over a restoring edge, a door: one sync per file and
/// head move, its rounds bounded by its script. Measured on the harness's
/// budget: a commit whose sync loses `/` as often as `agg_retries` allows
/// still counts in exactly, no delivery spends more than one segment, none
/// would die of its ttl. No time window: the losses are a trigger's count.
#[test]
fn a_commit_whose_sync_keeps_losing_the_root_still_counts_in() {
    if !all_shipped() {
        return;
    }
    let url = stub();
    let mut sp = space(&url);
    create(&mut sp, "/a/b/x.py", X1);
    settle(&mut sp, "stop");
    create(&mut sp, "/a/z.py", Z1);
    settle(&mut sp, "stop");
    assert_dirs(&sp, "born");
    let losses = cell_config("derive")["params"]["agg_retries"]
        .as_i64()
        .expect("agg_retries");
    assert!(losses > 0, "a sync retries a lost row");
    // A writer outside the sync moves `/` first, every time: the trigger skips
    // the update the way a lost `changed_seq` does (rows_affected 0).
    sp.db
        .execute_batch(&format!(
            "CREATE TABLE lose_root (n INTEGER); INSERT INTO lose_root VALUES ({losses});
             CREATE TRIGGER lose_root_cas BEFORE UPDATE ON dirs
             WHEN NEW.path = '/' AND NEW.changed_seq <> OLD.changed_seq
                  AND (SELECT n FROM lose_root) > 0
             BEGIN UPDATE lose_root SET n = n - 1; SELECT RAISE(IGNORE); END;"
        ))
        .expect("the rival on /");
    ws(&mut sp, "ws_open", "", json!({"name": "W", "root": "/"}));
    ws(
        &mut sp,
        "ws_patch",
        "W",
        json!({"diff": "--- a/a/b/x.py\n+++ b/a/b/x.py\n@@ -1,4 +1,4 @@\n import os\n \n \
                        def f():\n-    return os.getcwd()\n+    return os.getcwd() + '/'\n"}),
    );
    let n = sp.sent.len();
    sp.worst_segment = (0, String::new());
    ws(&mut sp, "ws_commit", "W", json!({}));
    settle(&mut sp, "stop");
    assert_eq!(
        sp.rows("SELECT n FROM lose_root"),
        vec![vec![json!(0)]],
        "the sync lost / {losses} times"
    );
    assert_eq!(
        sp.ttl_dead,
        Vec::<String>::new(),
        "no delivery dies of its ttl"
    );
    assert!(
        sp.worst_segment.0 <= SEGMENT_MAX,
        "a segment of the commit spends {:?} of {SEGMENT_MAX} routing decisions",
        sp.worst_segment
    );
    assert_dirs(&sp, "committed through a lost root");
    assert!(
        sp.sent[n..].iter().any(|m| m["from"] == json!("./derive")
            && m["to"] == json!("./derive")
            && m["route"] == json!("in_dirs")),
        "the sync crosses its door: in_dirs from ./derive to ./derive"
    );
    assert_clean(&sp);
}
