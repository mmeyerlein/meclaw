//! GH #907 (R-FJ-1) -- `file-space/write` takes a third content source,
//! `args.attachment`: an index into `body.attachments`, whose bytes the code
//! cell hands to the script's stdin only. A file made that way is the same
//! file a `b64` request makes -- same version, same blocks, same `files` row
//! -- but neither its bytes nor its text view (the `derived` pages, the text
//! of a text file) ever sit in a `pending` row: `write` stages the blocks in
//! the store under a key of the request and re-keys them onto the file, and
//! `derive` keeps only section borders between its phases and reads the text
//! again for each model step.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script the shipped one, every edge under the colony's CEL, the store the
//! store cell's own dispatcher, `Space::blobs` the colony's blob store. The
//! summarizer is the harness's `llm` recorder; `./embed` is the shipped code
//! cell against a stub endpoint on 127.0.0.1 run by this file.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

const DIM: usize = 16;
const TEXT_BLOB: &str = "0192f0a0-0000-7000-8000-000000000001";
const BIN_BLOB: &str = "0192f0a0-0000-7000-8000-000000000002";
const NEW_BLOB: &str = "0192f0a0-0000-7000-8000-000000000003";
const MOVED_BLOB: &str = "0192f0a0-0000-7000-8000-000000000004";
const GONE_BLOB: &str = "0192f0a0-0000-7000-8000-00000000dead";
/// A line of the text file that is not its first line (the first line is
/// the `files.oneline` a birth writes).
const TEXT_MARK: &str = "TEXTMARK-7c1e the budget line of the notes";
const PAGE_MARK: &str = "PAGEMARK-4a09 first page of the scan";

fn shipped_all() -> bool {
    shipped()
        && repo("templates/file-space/embed/config.json").is_file()
        && repo("templates/file-space/summarizer/config.json").is_file()
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1: the same vector for
/// every input -- `derive` only has to get one vector per section back.
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
                .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
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

fn space(url: &str) -> Space {
    let mut sp = Space::with(
        "/x/files",
        &[
            ("embed", "endpoint", json!(url)),
            ("embed", "model", json!("stub-embed")),
            ("embed", "dim", json!(DIM.to_string())),
        ],
    );
    sp.blobs.insert(TEXT_BLOB.into(), text_bytes());
    sp.blobs.insert(BIN_BLOB.into(), binary_bytes());
    sp
}

/// A Markdown file of a few sections; the marker line sits in the middle.
fn text_bytes() -> Vec<u8> {
    let mut t = String::from("# Notes of the week\n");
    for h in ["Plan", "Budget", "Team"] {
        t.push_str(&format!("\n## {h}\n"));
        for i in 0..20 {
            t.push_str(&format!("{h} line {i} of the notes.\n"));
        }
        if h == "Budget" {
            t.push_str(TEXT_MARK);
            t.push('\n');
        }
    }
    t.into_bytes()
}

/// 200 KiB that are no UTF-8 (a NUL first): two blocks of 192 KiB and 8 KiB.
fn binary_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let mut n = 0u32;
    while out.len() < 200 * 1024 {
        let h = sha256_hex(format!("scan/{n}").as_bytes());
        out.extend((0..32).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()));
        n += 1;
    }
    out.truncate(200 * 1024);
    out[0] = 0;
    out
}

fn pages() -> Value {
    json!([{"part": 1, "text": PAGE_MARK}, {"part": 2, "text": "the second page"}])
}

fn att(blob: &str) -> Value {
    json!([{"blob_id": blob, "mime_type": "application/octet-stream", "filename": "f",
            "size_bytes": 1, "sha256": sha256_hex(b"any")}])
}

/// One `in_write` request with `attachments` in its body (the harness's
/// `request` sends none); returns its one answer.
fn write_req(sp: &mut Space, op: &str, file: Option<&str>, args: Value, atts: Value) -> Value {
    let op_id = sp.next_op_id();
    let mut body = json!({"op": op, "args": args, "attachments": atts});
    if let Some(f) = file {
        body["file"] = json!(f);
    }
    let before = sp.out.len();
    sp.lane(
        "in_write",
        json!({}),
        json!({"op": op, "op_id": op_id}),
        body,
    );
    let mine: Vec<Msg> = sp.out[before..]
        .iter()
        .filter(|m| m.route() == "answer" && m.hop.get("op_id") == Some(&json!(op_id)))
        .cloned()
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "{op}: one answer, got {mine:?}; stderr {:?}",
        sp.stderr
    );
    Value::Object(mine[0].body.clone())
}

fn count(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

fn staged(sp: &Space) -> i64 {
    count(
        sp,
        "SELECT (SELECT COUNT(*) FROM blocks WHERE file LIKE 'stage:%') + \
         (SELECT COUNT(*) FROM derived WHERE file LIKE 'stage:%')",
    )
}

/// The `insert`/`update` operations on `pending` among everything the space
/// delivered to `./store`, as their JSON text.
fn pending_ops(sp: &Space) -> Vec<String> {
    sp.sent
        .iter()
        .filter(|m| m["to"] == json!("./store"))
        .flat_map(|m| {
            m["body"]["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|c| c["text"].as_str().map(str::to_string))
        .filter(|t| {
            let a: Value = sj::from_str(t).unwrap_or(Value::Null);
            a["table"] == json!("pending")
                && (a["operation"] == json!("insert") || a["operation"] == json!("update"))
        })
        .collect()
}

/// What a file row, its version and its blocks say, for the comparison.
fn picture(sp: &Space) -> Vec<Vec<Vec<Value>>> {
    vec![
        sp.rows("SELECT kind, mime, bytes, lines, oneline FROM files"),
        sp.rows("SELECT version, blocks, bytes, lines FROM versions"),
        sp.rows("SELECT hash, enc, size, body FROM blocks ORDER BY hash"),
        sp.rows("SELECT version, kind, part, body FROM derived ORDER BY part"),
    ]
}

#[test]
fn an_attachment_makes_the_file_a_b64_request_makes() {
    if !shipped_all() {
        return;
    }
    let url = stub();
    for (blob, path, bytes, derived) in [
        (TEXT_BLOB, "/notes.md", text_bytes(), None),
        (BIN_BLOB, "/scan.pdf", binary_bytes(), Some(pages())),
    ] {
        let mut by_att = space(&url);
        let mut args = json!({"path": path, "attachment": 0});
        if let Some(p) = &derived {
            args["derived"] = p.clone();
        }
        let a = write_req(&mut by_att, "create", None, args, att(blob));
        assert_eq!(a["ok"], json!(true), "{path} by attachment: {a}");
        assert_eq!(
            a["version"],
            json!(&sha256_hex(&bytes)[..12]),
            "the version is the content's"
        );

        let mut by_b64 = space(&url);
        let mut args = json!({"path": path, "b64": b64(&bytes)});
        if let Some(p) = &derived {
            args["derived"] = p.clone();
        }
        let b = write_req(&mut by_b64, "create", None, args, json!([]));
        assert_eq!(b["ok"], json!(true), "{path} by b64: {b}");

        // Compared before the summarizer answers: `./derive` lays its own
        // one line over `files.oneline` afterwards.
        assert_eq!(picture(&by_att), picture(&by_b64), "{path}");
        if path == "/scan.pdf" {
            assert_eq!(
                count(&by_att, "SELECT COUNT(*) FROM blocks"),
                2,
                "200 KiB of binary are two blocks"
            );
            let file = a["file"].as_str().unwrap();
            assert_eq!(
                count(
                    &by_att,
                    &format!(
                        "SELECT COUNT(*) FROM derived WHERE file = '{file}' AND version = '{}'",
                        sha256_hex(&bytes)
                    )
                ),
                2,
                "the pages land under the file and its version"
            );
        }
        assert_eq!(staged(&by_att), 0, "nothing stays staged");
        for sp in [&by_att, &by_b64] {
            assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
            assert!(sp.unscoped().is_empty(), "{:#?}", sp.unscoped());
        }
    }
}

#[test]
fn neither_bytes_nor_text_view_sit_in_pending() {
    if !shipped_all() {
        return;
    }
    let url = stub();
    for (blob, path, bytes, derived, mark) in [
        (TEXT_BLOB, "/notes.md", text_bytes(), None, TEXT_MARK),
        (
            BIN_BLOB,
            "/scan.pdf",
            binary_bytes(),
            Some(pages()),
            PAGE_MARK,
        ),
    ] {
        let mut sp = space(&url);
        let mut args = json!({"path": path, "attachment": true});
        if let Some(p) = &derived {
            args["derived"] = p.clone();
        }
        let a = write_req(&mut sp, "create", None, args, att(blob));
        assert_eq!(a["ok"], json!(true), "{a}");
        sp.llm_answer("What the file is.\n\nWhat it holds.", "stop");
        let file = a["file"].as_str().unwrap();
        // The derive job ran through: sections embedded, summaries written.
        assert!(
            count(
                &sp,
                &format!("SELECT COUNT(*) FROM embeddings WHERE file = '{file}'")
            ) > 0,
            "{path}: embedded; stderr {:?}",
            sp.stderr
        );
        assert_eq!(
            count(
                &sp,
                &format!("SELECT COUNT(*) FROM summaries WHERE file = '{file}'")
            ),
            // oneline, short and, since GH #947, the head's tags
            3,
            "{path}: summarized"
        );

        let ops = pending_ops(&sp);
        assert!(
            ops.iter().any(|o| o.contains("\"cell\": \"write\""))
                && ops.iter().any(|o| o.contains("\"cell\": \"derive\"")),
            "both cells parked something"
        );
        // A slice of the base64 of each block (192 KiB is a multiple of 3,
        // so the second block's base64 is a slice of the file's), and the
        // marker of the text view.
        let n = 3000.min(bytes.len() / 3 * 3);
        let mut probes = vec![b64(&bytes[..n]), mark.to_string()];
        if bytes.len() > 196_608 + 3000 {
            probes.push(b64(&bytes[196_608..196_608 + 3000]));
        }
        for o in &ops {
            for p in &probes {
                assert!(
                    !o.contains(p.as_str()),
                    "{path}: a pending op holds {}...: {}",
                    &p[..20.min(p.len())],
                    &o[..200.min(o.len())]
                );
            }
        }
        // Nor a hop or a context anywhere in the space.
        for m in &sp.sent {
            let h = m["header"].to_string();
            for p in &probes {
                assert!(!h.contains(p.as_str()), "{path}: a header holds content");
            }
        }
        assert_eq!(staged(&sp), 0);
        assert_eq!(
            count(
                &sp,
                "SELECT COUNT(*) FROM pending WHERE op_id NOT LIKE 'done:%'"
            ),
            0
        );
        assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
        assert!(sp.unscoped().is_empty(), "{:#?}", sp.unscoped());
    }
}

#[test]
fn a_refused_attachment_leaves_nothing_staged() {
    if !shipped_all() {
        return;
    }
    let url = stub();
    let mut sp = space(&url);
    sp.seed_file("fh-00000000000a", "/scan.pdf", "binary", "application/pdf");
    let a = write_req(
        &mut sp,
        "create",
        None,
        json!({"path": "/scan.pdf", "attachment": 0, "derived": pages()}),
        att(BIN_BLOB),
    );
    assert_eq!(a["error"]["code"], json!("path_taken"), "{a}");
    assert_eq!(
        staged(&sp),
        0,
        "path_taken drops the staged blocks and pages"
    );
    assert_eq!(count(&sp, "SELECT COUNT(*) FROM blocks"), 0);
    assert_eq!(count(&sp, "SELECT COUNT(*) FROM pending"), 0);

    let a = write_req(
        &mut sp,
        "create",
        None,
        json!({"path": "/other.pdf", "attachment": 0, "b64": "AAAA"}),
        att(BIN_BLOB),
    );
    assert_eq!(
        a["error"]["code"],
        json!("bad_request"),
        "one content source: {a}"
    );
    let a = write_req(
        &mut sp,
        "create",
        None,
        json!({"path": "/other.pdf", "attachment": 0}),
        att(GONE_BLOB),
    );
    assert_eq!(
        a["error"]["code"],
        json!("not_found"),
        "the entry's error: {a}"
    );
    let a = write_req(
        &mut sp,
        "create",
        None,
        json!({"path": "/other.pdf", "attachment": 3}),
        att(BIN_BLOB),
    );
    assert_eq!(
        a["error"]["code"],
        json!("bad_request"),
        "no such entry: {a}"
    );
    assert_eq!(
        count(&sp, "SELECT COUNT(*) FROM files"),
        1,
        "nothing was born"
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
}

#[test]
fn an_overwrite_from_an_attachment_replaces_the_file_whole() {
    if !shipped_all() {
        return;
    }
    let url = stub();
    let mut sp = space(&url);
    let new = b"three\nfour\n".to_vec();
    sp.blobs.insert(NEW_BLOB.into(), new.clone());
    sp.blobs.insert(MOVED_BLOB.into(), b"five\n".to_vec());
    let a = sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/o.txt", "text": "one\ntwo\n"}),
        json!({}),
    );
    sp.llm_answer("A file.", "stop");
    let file = a["file"].as_str().unwrap().to_string();
    let v1 = a["version"].as_str().unwrap().to_string();

    let a = write_req(
        &mut sp,
        "overwrite",
        Some(&file),
        json!({"base": v1, "attachment": 0}),
        att(NEW_BLOB),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(a["version"], json!(&sha256_hex(&new)[..12]));
    assert_eq!(
        a["hook"],
        json!("none"),
        "an upload is not sent to the hook"
    );
    assert_eq!(
        sp.rows(&format!("SELECT head FROM files WHERE file = '{file}'"))[0][0],
        json!(sha256_hex(&new))
    );
    sp.llm_answer("A file.", "stop");

    // The head moved since `v1`: an upload is refused, not merged.
    let a = write_req(
        &mut sp,
        "overwrite",
        Some(&file),
        json!({"base": v1, "attachment": 0}),
        att(MOVED_BLOB),
    );
    assert_eq!(a["error"]["code"], json!("base_moved"), "{a}");
    assert_eq!(
        sp.rows(&format!("SELECT head FROM files WHERE file = '{file}'"))[0][0],
        json!(sha256_hex(&new)),
        "the head stays"
    );
    assert_eq!(staged(&sp), 0);
    for o in pending_ops(&sp) {
        assert!(!o.contains("three") && !o.contains("five"), "{o}");
    }
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert!(sp.unscoped().is_empty(), "{:#?}", sp.unscoped());
}

/// OR-FJ.I.22: a PDF can be pure ASCII (no binary comment line, no compressed
/// stream). A create that brings `derived` pages is binary all the same --
/// otherwise its first line (`%PDF-1.4`) became the file's one line and was
/// parked in `pending`, and its blocks were its PDF source as text. Measured
/// in the seam lock before the fix: `"kind": "text"`, `"oneline": "%PDF-1.4"`
/// in 18 pending ops of write and derive.
#[test]
fn an_ascii_pdf_with_pages_is_born_binary() {
    if !shipped_all() {
        return;
    }
    const ASCII_PDF_BLOB: &str = "0192f0a0-0000-7000-8000-000000000005";
    let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n".to_vec();
    let url = stub();
    let mut sp = space(&url);
    sp.blobs.insert(ASCII_PDF_BLOB.into(), pdf.clone());
    let a = write_req(
        &mut sp,
        "create",
        None,
        json!({"path": "/ascii.pdf", "attachment": 0, "derived": pages()}),
        att(ASCII_PDF_BLOB),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(
        sp.rows("SELECT kind, oneline FROM files"),
        vec![vec![json!("binary"), json!("")]],
        "born binary, no first line of PDF source"
    );
    assert_eq!(
        sp.rows("SELECT DISTINCT enc FROM blocks"),
        vec![vec![json!("b64")]],
        "its blocks are bytes"
    );
    for o in pending_ops(&sp) {
        assert!(
            !o.contains("%PDF"),
            "a pending op holds the PDF's first line: {o}"
        );
    }
}
