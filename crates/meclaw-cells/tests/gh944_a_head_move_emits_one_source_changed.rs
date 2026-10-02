//! GH #944 -- the node source contract at the rim of a file space: every
//! head move of a file (birth, write, workspace commit, one per file) sends
//! exactly ONE `source_changed {source, version, path, fmt, parser, mark,
//! nodes, links, tomb: false}` out of the hive, and a removal one with
//! `tomb: true`; only the head has rows in `nodes`/`links`/`node_runs`. The
//! event does not depend on who wrote (`in_write`, a tool call, an ingested
//! document) nor on a model: a refused summary leaves nodes and event in
//! place. Every mark is honest -- an empty file has 0 nodes and no mark, a
//! file without a known ending `no_extractor`, a file over
//! `extract_max_bytes` `too_large`, more items than `nodes_max` `truncated`
//! with exactly `nodes_max` nodes, a file no extractor reads `unparsable`
//! and is written all the same. Once the sections are embedded, `node_runs`
//! carries the file vector `fvec`.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder, `./embed` the shipped code cell
//! against an HTTP stub on 127.0.0.1 run by this file (no provider, no net).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

const DIM: usize = 16;
const NOTES_BLOB: &str = "0192f0a0-0000-7000-8000-000000000944";

/// Line 7 and 8 are what the workspace patch below changes.
const A_PY: &str = "import os\n\nclass A:\n    def m(self):\n        return os.getcwd()\n\n\
def f():\n    return g()\n";
/// Line 5 and 6 are what the workspace patch below changes.
const B_MD: &str = "# Title\n\nSee [a](a.py).\n\n## Part\ntext\n";

fn all_shipped() -> bool {
    shipped()
        && repo("templates/file-space/embed/config.json").is_file()
        && repo("templates/file-space/summarizer/config.json").is_file()
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1: the same vector
/// for every input, so every section of a file has the same bits and the
/// file vector (their bitwise majority) is exactly those bits.
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

/// The shipped space with `./embed` at the stub, plus `over`.
fn space_at(url: &str, over: &[(&str, &str, Value)]) -> Space {
    let mut all: Vec<(&str, &str, Value)> = vec![
        ("embed", "endpoint", json!(url)),
        ("embed", "model", json!("stub-embed")),
        ("embed", "dim", json!(DIM.to_string())),
    ];
    all.extend(over.iter().cloned());
    Space::with("/x/files", &all)
}

/// The shipped space without embeddings (no endpoint needed), plus `over`.
fn plain(over: &[(&str, &str, Value)]) -> Space {
    let mut all: Vec<(&str, &str, Value)> = vec![("derive", "embed", json!("0"))];
    all.extend(over.iter().cloned());
    Space::with("/x/files", &all)
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
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

fn count(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

fn head_of(sp: &Space, file: &str) -> String {
    col(sp, &format!("SELECT head FROM files WHERE file = '{file}'"))[0].clone()
}

/// Answer every summary the recorder holds, in order (`stop` or not).
fn settle(sp: &mut Space, finish: &str) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", finish);
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn create(sp: &mut Space, path: &str, text: &str) -> (String, String) {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": text}),
        json!({}),
    ));
    let file = a["file"].as_str().expect("a file id").to_string();
    let head = head_of(sp, &file);
    (file, head)
}

/// The `source_changed` events that left since `before` (an index into
/// `out`), every one checked for the contract's form.
fn events_since(sp: &Space, before: usize) -> Vec<Value> {
    sp.out[before..]
        .iter()
        .filter(|m| m.route() == "source_changed")
        .map(|m| {
            assert_eq!(
                Value::Object(m.hop.clone()),
                json!({"route": "source_changed"}),
                "the head carries the route alone -- no caller, no op, no job"
            );
            for k in ["cur_origin", "cur_phase", "cur_job", "cur_call"] {
                assert!(!m.context.contains_key(k), "{k} leaves the space");
            }
            let mut b = m.body.clone();
            b.remove("messages");
            Value::Object(b)
        })
        .collect()
}

/// The one event of a living head: the whole form, the counts the rows say.
fn assert_head_event(sp: &Space, e: &Value, file: &str, path: &str) {
    let head = head_of(sp, file);
    let run = sp.rows(&format!(
        "SELECT fmt, parser, mark, nodes, links FROM node_runs WHERE file = '{file}'"
    ));
    assert_eq!(run.len(), 1, "one run row for the head: {run:?}");
    assert_eq!(
        *e,
        json!({"source": file, "version": &head[..12], "path": path,
               "fmt": run[0][0].clone(), "parser": run[0][1].clone(),
               "mark": run[0][2].clone(), "nodes": run[0][3].clone(),
               "links": run[0][4].clone(), "tomb": false}),
    );
    assert_eq!(
        count(
            sp,
            &format!("SELECT COUNT(*) FROM nodes WHERE file = '{file}'")
        ),
        run[0][3].as_i64().unwrap(),
        "the event counts the rows"
    );
    assert_eq!(
        count(
            sp,
            &format!("SELECT COUNT(*) FROM links WHERE file = '{file}'")
        ),
        run[0][4].as_i64().unwrap()
    );
    for t in ["nodes", "links", "node_runs"] {
        assert_eq!(
            col(
                sp,
                &format!("SELECT DISTINCT version FROM {t} WHERE file = '{file}'")
            )
            .into_iter()
            .filter(|v| *v != head)
            .collect::<Vec<_>>(),
            Vec::<String>::new(),
            "{t}: rows of the head only"
        );
    }
}

/// Every route `./derive`, `./write` and `./ws` sent is one its contract
/// declares (the colony's `validate_emits` would dead-letter any other), and
/// the hive's rim names `source_changed`.
fn assert_contracts(sp: &Space) {
    for cell in ["derive", "write", "ws"] {
        let declared: Vec<String> =
            cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]
                .as_array()
                .unwrap_or_else(|| panic!("{cell} declares its routes"))
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
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
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
}

fn ws(sp: &mut Space, op: &str, name: &str, args: Value) -> Value {
    let hop = if name.is_empty() {
        json!({})
    } else {
        json!({"ws": name})
    };
    ok(sp.request("in_ws", op, None, args, hop))
}

#[test]
fn a_head_move_emits_one_source_changed() {
    if !all_shipped() {
        return;
    }
    let url = stub();
    let mut sp = space_at(&url, &[]);

    // --- birth: one event before any model answered ------------------------
    let n0 = sp.out.len();
    let (a, a1) = create(&mut sp, "/lib/a.py", A_PY);
    let ev = events_since(&sp, n0);
    assert_eq!(ev.len(), 1, "one event per birth: {ev:?}");
    assert_head_event(&sp, &ev[0], &a, "/lib/a.py");
    assert_eq!(
        (
            ev[0]["fmt"].clone(),
            ev[0]["parser"].clone(),
            ev[0]["mark"].clone()
        ),
        (json!("python"), json!("ast"), json!(""))
    );
    assert_eq!(
        col(
            &sp,
            &format!("SELECT anchor FROM nodes WHERE file = '{a}' ORDER BY from_pos, anchor")
        ),
        vec!["class:A", "class:A/def:m", "def:f"]
    );
    assert_eq!(
        sp.rows(&format!(
            "SELECT from_anchor, kind, target_name FROM links WHERE file = '{a}' \
             AND kind = 'import'"
        )),
        vec![vec![json!(""), json!("import"), json!("os")]]
    );
    assert!(!sp.llm.is_empty(), "the summary is still asked for");
    settle(&mut sp, "stop");

    // `fvec`: the bitwise majority of identical section bits is those bits.
    let blobs = col(
        &sp,
        &format!("SELECT DISTINCT blob FROM embeddings WHERE file = '{a}' AND version = '{a1}'"),
    );
    assert_eq!(blobs.len(), 1, "the stub gives every section one vector");
    assert_eq!(
        col(
            &sp,
            &format!("SELECT fvec FROM node_runs WHERE file = '{a}'")
        ),
        blobs,
        "node_runs carries the file vector once the sections are embedded"
    );

    // --- a write: one event, the rows move to the new head -----------------
    let n1 = sp.out.len();
    ok(sp.request(
        "in_write",
        "replace",
        Some(&a),
        json!({"old": "return g()", "new": "return h()", "expected": 1, "base": &a1[..12]}),
        json!({}),
    ));
    let a2 = head_of(&sp, &a);
    assert_ne!(a1, a2);
    let ev = events_since(&sp, n1);
    assert_eq!(ev.len(), 1, "one event per write: {ev:?}");
    assert_head_event(&sp, &ev[0], &a, "/lib/a.py");
    assert_eq!(
        col(
            &sp,
            &format!(
                "SELECT target_name FROM links WHERE file = '{a}' AND kind = 'call' \
                 AND from_anchor = 'def:f'"
            )
        ),
        vec!["h"]
    );
    settle(&mut sp, "stop");

    // --- a workspace commit over two files: one event per file -------------
    let n2 = sp.out.len();
    let (b, _) = create(&mut sp, "/lib/b.md", B_MD);
    settle(&mut sp, "stop");
    assert_eq!(events_since(&sp, n2).len(), 1);
    assert_eq!(
        sp.rows(&format!(
            "SELECT kind, target_name FROM links WHERE file = '{b}'"
        )),
        vec![vec![json!("link"), json!("/lib/a.py")]],
        "a relative link is resolved against the file's directory"
    );
    ws(&mut sp, "ws_open", "", json!({"name": "W", "root": "/"}));
    ws(
        &mut sp,
        "ws_patch",
        "W",
        json!({"diff": "--- a/lib/a.py\n+++ b/lib/a.py\n@@ -7,2 +7,2 @@\n-def f():\n+def f2():\n     return h()\n\
                       --- a/lib/b.md\n+++ b/lib/b.md\n@@ -5,2 +5,2 @@\n-## Part\n+## Part two\n text\n"}),
    );
    assert_eq!(
        events_since(&sp, n2).len(),
        1,
        "a working version sends nothing"
    );
    let n3 = sp.out.len();
    let c = ws(&mut sp, "ws_commit", "W", json!({}));
    assert_eq!(c["files"].as_array().unwrap().len(), 2, "{c}");
    let ev = events_since(&sp, n3);
    assert_eq!(ev.len(), 2, "one event per committed file: {ev:?}");
    let mut sources: Vec<String> = ev
        .iter()
        .map(|e| e["source"].as_str().unwrap().to_string())
        .collect();
    sources.sort();
    let mut want = vec![a.clone(), b.clone()];
    want.sort();
    assert_eq!(sources, want);
    for e in &ev {
        let (f, p) = if e["source"] == json!(&a) {
            (&a, "/lib/a.py")
        } else {
            (&b, "/lib/b.md")
        };
        assert_head_event(&sp, e, f, p);
    }
    assert!(
        col(&sp, &format!("SELECT anchor FROM nodes WHERE file = '{a}'"))
            .contains(&"def:f2".to_string()),
        "the committed head's nodes"
    );
    assert!(
        col(&sp, &format!("SELECT anchor FROM nodes WHERE file = '{b}'"))
            .contains(&"sec:part-two".to_string())
    );
    settle(&mut sp, "stop");

    // --- remove: one tomb event, the rows are gone -------------------------
    let n4 = sp.out.len();
    let head = head_of(&sp, &a);
    ok(sp.request(
        "in_write",
        "remove",
        Some(&a),
        json!({"base": &head[..12]}),
        json!({}),
    ));
    let ev = events_since(&sp, n4);
    assert_eq!(
        ev,
        vec![json!({"source": &a, "path": "/lib/a.py", "tomb": true})],
        "one tomb event per removal"
    );
    for t in ["nodes", "links", "node_runs"] {
        assert_eq!(
            count(&sp, &format!("SELECT COUNT(*) FROM {t} WHERE file = '{a}'")),
            0,
            "{t} of a removed file"
        );
    }
    assert!(sp.llm.is_empty(), "a removal asks no model");

    // Five head moves, one removal, six events in all.
    assert_eq!(sp.routed("source_changed").len(), 6);
    assert_contracts(&sp);
}

#[test]
fn every_writer_sends_the_same_event() {
    if !shipped() {
        return;
    }
    // A tool call of the core (`./tools` -> `./write`, caller `tools`).
    let mut sp = plain(&[]);
    let n0 = sp.out.len();
    sp.lane(
        "in_tool",
        json!({"tool_caller": "cogny"}),
        json!({"tool_name": "file_create", "tool_call_id": "c-1"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c-1",
                             "text": json!({"path": "/t/n.md", "text": "# N\n\n## One\nx\n"})
                                 .to_string()}]}),
    );
    let file = col(&sp, "SELECT file FROM files WHERE path = '/t/n.md'");
    assert_eq!(
        file.len(),
        1,
        "the tool wrote the file; stderr {:?}",
        sp.stderr
    );
    let ev = events_since(&sp, n0);
    assert_eq!(ev.len(), 1, "a tool's write sends the one event: {ev:?}");
    assert_head_event(&sp, &ev[0], &file[0], "/t/n.md");
    assert_eq!(ev[0]["nodes"], json!(2));
    settle(&mut sp, "stop");
    assert_eq!(sp.routed("source_changed").len(), 1);
    assert_contracts(&sp);

    // An ingested document (`./ingest` -> `./write`, caller `ingest`).
    if !repo("templates/file-space/ingest/config.json").is_file() {
        return;
    }
    let mut sp = plain(&[]);
    sp.blobs
        .insert(NOTES_BLOB.into(), b"# Notes\n\n## One\nx\n".to_vec());
    let n0 = sp.out.len();
    sp.lane(
        "in_ingest",
        json!({"chat_id": 100, "channel": "telegram"}),
        json!({"engine": "text"}),
        json!({"messages": [{"origin": "user", "type": "text", "text": "keep this"}],
               "attachments": [{"blob_id": NOTES_BLOB, "mime_type": "text/markdown",
                                "filename": "notes.md", "size_bytes": 19,
                                "sha256": sha256_hex(b"any")}]}),
    );
    let born = sp.rows("SELECT file, path FROM files WHERE path LIKE '/inbox/%'");
    assert_eq!(
        born.len(),
        1,
        "the document is a file; stderr {:?}",
        sp.stderr
    );
    let (file, path) = (
        born[0][0].as_str().unwrap().to_string(),
        born[0][1].as_str().unwrap().to_string(),
    );
    let ev = events_since(&sp, n0);
    assert_eq!(
        ev.len(),
        1,
        "an ingested document sends the one event: {ev:?}"
    );
    assert_head_event(&sp, &ev[0], &file, &path);
    assert_eq!(ev[0]["fmt"], json!("markdown"));
    settle(&mut sp, "stop");
    assert_eq!(sp.routed("turn").len(), 1, "the turn still goes on");
    assert_eq!(sp.routed("source_changed").len(), 1);
    assert_contracts(&sp);
}

#[test]
fn nodes_never_hang_on_a_model_and_every_mark_is_honest() {
    if !shipped() {
        return;
    }
    // A refused summary: the job stops, the nodes and the event stay.
    let mut sp = plain(&[]);
    let n0 = sp.out.len();
    let (f, _) = create(&mut sp, "/s.md", "# S\n\n## A\nx\n\n## B\ny\n");
    settle(&mut sp, "error");
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM summaries WHERE file = '{f}'")
        ),
        0,
        "the summary was refused"
    );
    let ev = events_since(&sp, n0);
    assert_eq!(ev.len(), 1);
    assert_head_event(&sp, &ev[0], &f, "/s.md");
    assert_eq!(ev[0]["nodes"], json!(3));

    // (path, text, fmt, parser, mark, nodes): an empty file, a file without
    // an ending, a file no extractor reads (written all the same -- Rust has
    // no syntax hook, so `./guard` lets it through).
    for (path, text, fmt, parser, mark, nodes) in [
        ("/e.md", "", "markdown", "", "", 0),
        ("/Makefile", "all:\n\techo hi\n", "", "", "no_extractor", 0),
        ("/x.rs", "fn a() {\n", "rust", "", "unparsable", 0),
    ] {
        let n = sp.out.len();
        let (f, _) = create(&mut sp, path, text);
        settle(&mut sp, "stop");
        let ev = events_since(&sp, n);
        assert_eq!(ev.len(), 1, "{path}: {ev:?}; stderr {:?}", sp.stderr);
        assert_head_event(&sp, &ev[0], &f, path);
        assert_eq!(
            (
                ev[0]["fmt"].clone(),
                ev[0]["parser"].clone(),
                ev[0]["mark"].clone(),
                ev[0]["nodes"].clone()
            ),
            (json!(fmt), json!(parser), json!(mark), json!(nodes)),
            "{path}"
        );
    }
    assert_contracts(&sp);

    // Over `extract_max_bytes`: no nodes, `too_large`.
    let mut sp = plain(&[("derive", "extract_max_bytes", json!(10))]);
    let n = sp.out.len();
    let (f, _) = create(&mut sp, "/big.md", "# A\n\n## B\nmore than ten bytes\n");
    let ev = events_since(&sp, n);
    assert_eq!(ev.len(), 1);
    assert_head_event(&sp, &ev[0], &f, "/big.md");
    assert_eq!(
        (ev[0]["mark"].clone(), ev[0]["nodes"].clone()),
        (json!("too_large"), json!(0))
    );
    settle(&mut sp, "stop");
    assert_contracts(&sp);

    // More items than `nodes_max`: the first ones in source order, `truncated`.
    let mut sp = plain(&[("derive", "nodes_max", json!(2))]);
    let n = sp.out.len();
    let (f, _) = create(
        &mut sp,
        "/m.py",
        "def a():\n    pass\n\ndef b():\n    pass\n\ndef c():\n    pass\n\ndef d():\n    pass\n",
    );
    let ev = events_since(&sp, n);
    assert_eq!(ev.len(), 1);
    assert_head_event(&sp, &ev[0], &f, "/m.py");
    assert_eq!(
        (ev[0]["mark"].clone(), ev[0]["nodes"].clone()),
        (json!("truncated"), json!(2))
    );
    assert_eq!(
        col(
            &sp,
            &format!("SELECT anchor FROM nodes WHERE file = '{f}' ORDER BY from_pos")
        ),
        vec!["def:a", "def:b"]
    );
    settle(&mut sp, "stop");
    assert_contracts(&sp);
}
