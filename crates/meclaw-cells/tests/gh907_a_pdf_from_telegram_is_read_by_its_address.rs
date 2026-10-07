//! GH #907 -- a PDF sent to the bot is read by its address.
//!
//! The acceptance of the road, on ONE booted colony: a person sends a PDF to
//! the Telegram bot, and the assistant receives a line that names the file,
//! never its bytes. The stub bot delivers three updates in one batch: a small
//! PDF (`two_pages.pdf`, the fixture), a photo, and a large PDF (`large.pdf`,
//! built here: 30 pages of about 3 MiB, each page carrying a marker
//! `zebra-<page>-<hex>` unique to this run, the text of all pages over the
//! 64 KiB at which the colony moves a body into a blob). What is measured, each
//! at the receiver:
//!
//! 1. the member's file space (`<member>/file-space`) holds each document under
//!    `/inbox/<UTC date>/<name>` with exactly its bytes (`files.bytes`, and the
//!    head version is their sha256), one `derived` row per page, a one-line
//!    summary and embeddings (read out of the space's own store, and once more
//!    through the space's `in_read` door by the file's address);
//! 2. each turn that reaches the assistant carries
//!    `[file fh-<12 hex>@<12 hex> "<name>", <n> pages: <one line>]` right after
//!    the caption, no `attachments` slot, and nowhere the bytes or the text of
//!    the PDF;
//! 3. the assistant reads page two of each by that address with the
//!    `file_read` tool (the member's `file_*` tool edges, GH #908), and the
//!    `tool_result` it gets back is page two;
//! 4. the next `getUpdates` acknowledges all three updates (the photo
//!    included), and the photo reaches no assistant;
//! 5. the document exists behind the connector only as a blob (R-FJ-1): the
//!    connector's turn carries a reference (`attachments[]` with size and
//!    sha256, the blob in the colony's `blobs/` holding exactly the bytes), and
//!    in the whole message log no header is over 8 KiB or carries the bytes or
//!    any page's text, no `pending` write to the space's store carries them,
//!    and at the end `pending` holds nothing of the two documents. One line
//!    `GH907-MEASURE` on stderr prints what was measured.
//!
//! # What is booted
//!
//! The SHIPPED `member`, `firewall`, `file-space` and `telegram-connector`
//! templates, cell for cell; the member's `./file-space` ref is resolved the
//! way the mutation door lays it out. Every `ref` holder this round never
//! reaches (`access`, `affinity`, `memory-hive`) is replaced by an inert `code`
//! double (a hive door pointing at an absent directory leaves the inside
//! unroutable, GH #286). The firewall is the real one: the turn has to pass
//! it, and it must pass it ONCE -- a screen that emitted the body twice would
//! show up here as a third turn at the assistant. `./assistants` is replaced
//! by a double standing where the assistant level stands: it answers every
//! `in_turn` with what it saw and, like a model that read the line, calls
//! `file_read` with `page: 2` on the address the line names -- a `tool` on the
//! member's own edge, which is what the assistant level emits for any
//! `file_*` call (GH #908 pins that a talky does emit it). The connector is the
//! real `proxy` cell, pointed at a stub of the Bot API on 127.0.0.1; the file
//! space's `./embed` and `./summarizer` speak real HTTP to stubs on the same
//! listener. No paid provider is called. The colony runs with its blob store
//! at `<root>/blobs`, the one the connector writes the document into.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! member's `./file-space` node or the space's `./tools` is skipped, never
//! judged -- this file measures the road those two open.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, SurfaceRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ══════════════════════════════════════════════════════════════ the fixture

const BOT_TOKEN: &str = "4242:stub-token";
const CHAT_ID: i64 = 100;
const USER_ID: i64 = 200;
const DOC_UPDATE: i64 = 501;
const PHOTO_UPDATE: i64 = 502;
const DOC_FILE_ID: &str = "DOC-two-pages";
const DOC_FILE_PATH: &str = "documents/file_1.pdf";
const BIG_UPDATE: i64 = 503;
const BIG_FILE_ID: &str = "DOC-large";
const BIG_FILE_PATH: &str = "documents/file_2.pdf";
const CAPTION: &str = "here are the two pages";
const NAME: &str = "two_pages.pdf";
const BIG_NAME: &str = "large.pdf";
/// The large PDF: pages, filler words per page (the text of all pages is
/// ~88 KB, over the 64 KiB inline limit), and a padding stream of 3 MiB that
/// no page shows, so the file is as large as a real scan-sized document.
const BIG_PAGES: usize = 30;
const FILLER_WORDS: usize = 420;
const WORDS_PER_LINE: usize = 12;
const PAD_BYTES: usize = 3 * 1024 * 1024;
const FILLER: [&str; 16] = [
    "river", "stone", "lamp", "orchard", "signal", "harbor", "meadow", "copper", "lantern",
    "window", "garden", "thunder", "velvet", "canyon", "marble", "silver",
];
/// Every page marker of the large PDF starts with this: a header, a context or
/// a `pending` row that carried any page's text would carry it.
const MARKER: &str = "zebra-";
/// The most a message_log header row may weigh (R-FJ-1, measured).
const MAX_HEADER_BYTES: usize = 8192;
/// What the stub summarizer answers: the first line is the one line.
const SUMMARY: &str = "Two pages of a test.\n\nPage one and page two.";
const ONELINE: &str = "Two pages of a test.";
const EMBED_DIM: usize = 64;
/// The base64 of `%PDF-`, the first bytes of every PDF: a turn that carried
/// the document's bytes in any base64 form would carry this.
const PDF_B64_MAGIC: &str = "JVBERi0";

/// Failure budgets, generous under cargo's parallel load: the road from the
/// update to the assistant crosses a dozen cold `code` cells and three store
/// round trips before the summary, the embeddings and `pdftotext`, and the
/// large document's ~3 MiB are read out of the blob store and base64-encoded
/// for two scripts on the way.
const ROAD_SECS: u64 = 120;
const PROBE_SECS: u64 = 30;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The templates this file boots, or `false` when the tree under test did not
/// ship them (the public export ships a subset of the library, GH #49).
fn shipped() -> bool {
    [
        "templates/file-space/ingest/config.json",
        "templates/file-space/tools/config.json",
        "templates/member/file-space/config.json",
        "templates/file-space/config.json",
        "templates/member/config.json",
        "templates/telegram-connector/config.json",
        "templates/firewall/config.json",
    ]
    .iter()
    .all(|p| repo(p).is_file())
}

fn pdf_bytes() -> Vec<u8> {
    let p = repo("crates/meclaw-cells/tests/fixtures/two_pages.pdf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A tag unique to this run, so a page marker can only come from this run's PDF.
fn run_tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    sha256_hex(format!("{nanos}:{}", std::process::id()).as_bytes())
}

/// The marker of each page, `zebra-<page>-<12 hex>`, page 1 first.
fn page_markers(tag: &str) -> Vec<String> {
    (1..=BIG_PAGES)
        .map(|p| {
            format!(
                "{MARKER}{p}-{}",
                &sha256_hex(format!("{tag}:{p}").as_bytes())[..12]
            )
        })
        .collect()
}

/// A valid PDF of [`BIG_PAGES`] pages: page `n` shows its marker and
/// [`FILLER_WORDS`] words, and one unreferenced stream of [`PAD_BYTES`] hex
/// lines makes the file ~3 MiB without adding text (the hex alphabet cannot
/// spell a marker, `endstream` or `%PDF`). The cross-reference table is
/// computed, so no reader has to repair it: `pdftotext -layout` reads 30
/// pages, each with exactly its own marker, and warns nothing (checked with a
/// line-for-line port of this function). Returns the bytes and the markers.
fn big_pdf(tag: &str) -> (Vec<u8>, Vec<String>) {
    let markers = page_markers(tag);
    let n = BIG_PAGES;
    let font = 3 + 2 * n;
    let stream = |data: &[u8]| {
        let mut o = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream");
        o
    };
    let page = |contents: usize| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {contents} 0 R >>"
        )
        .into_bytes()
    };
    let content = |page_no: usize, marker: &str| {
        let words: Vec<&str> = (0..FILLER_WORDS)
            .map(|i| FILLER[(page_no * 31 + i * 7) % FILLER.len()])
            .collect();
        let mut lines = vec![marker.to_string()];
        lines.extend(words.chunks(WORDS_PER_LINE).map(|c| c.join(" ")));
        let mut out = b"BT /F1 7 Tf 9 TL 36 756 Td\n".to_vec();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(b"T* ");
            }
            out.extend_from_slice(format!("({l}) Tj\n").as_bytes());
        }
        out.extend_from_slice(b"ET\n");
        out
    };
    let mut pad = Vec::with_capacity(PAD_BYTES + 128);
    let mut row = 0usize;
    while pad.len() < PAD_BYTES {
        let h = sha256_hex(format!("pad:{row}").as_bytes());
        pad.extend_from_slice(h.as_bytes());
        pad.extend_from_slice(&h.as_bytes()[..63]);
        pad.push(b'\n');
        row += 1;
    }
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes(),
    ];
    objects.extend((0..n).map(|i| page(3 + n + i)));
    objects.extend((0..n).map(|i| stream(&content(i + 1, &markers[i]))));
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.push(stream(&pad));
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    (out, markers)
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

// ══════════════════════════════════════════════════════════════ the upstream

/// One request the stub saw: method, request target (path and query), body.
#[derive(Clone, Debug)]
struct Hit {
    method: String,
    target: String,
    body: Vec<u8>,
}

/// Everything the colony talks to over HTTP, on one listener on 127.0.0.1,
/// told apart by path:
///
/// - the Bot API (`/bot<token>/getUpdates|getFile|…`, `/file/bot<token>/…`).
///   `getUpdates` answers the way Telegram does: every update whose id is at
///   least `offset`, so an update that is not acknowledged comes back; an empty
///   answer waits a little first, the way a long poll does;
/// - an OpenAI-compatible embeddings endpoint (`/v1/embeddings`);
/// - an OpenAI-compatible chat endpoint (`/v1/chat/completions`) that answers
///   every request with [`SUMMARY`].
struct Upstream {
    base: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Upstream {
    fn start(pdf: Vec<u8>, big: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the upstream stub");
        let base = format!("http://{}", listener.local_addr().expect("a local address"));
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::new(Mutex::new(Vec::new()));
        let log = hits.clone();
        let docs = Arc::new((pdf, big));
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let log = log.clone();
                let docs = docs.clone();
                // One thread per connection: a long poll that waits must never
                // hold up the download or a model call behind it.
                std::thread::spawn(move || serve_one(conn, &docs, &log));
            }
        });
        Upstream { base, hits }
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().expect("the hit log").clone()
    }

    /// The `offset` of every `getUpdates`, in the order they arrived.
    fn poll_offsets(&self) -> Vec<i64> {
        self.hits()
            .iter()
            .filter(|h| path_of(&h.target).ends_with("/getUpdates"))
            .map(|h| {
                query_of(&h.target, "offset")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            })
            .collect()
    }

    fn count(&self, path_suffix: &str) -> usize {
        self.hits()
            .iter()
            .filter(|h| path_of(&h.target).ends_with(path_suffix))
            .count()
    }

    /// A short, byte-free account of the traffic, for failure messages.
    fn trace(&self) -> Vec<String> {
        self.hits()
            .iter()
            .map(|h| format!("{} {} ({} bytes)", h.method, h.target, h.body.len()))
            .collect()
    }
}

fn path_of(target: &str) -> &str {
    target.split('?').next().unwrap_or("")
}

fn query_of(target: &str, key: &str) -> Option<String> {
    let q = target.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn document_update() -> Value {
    json!({"update_id": DOC_UPDATE, "message": {
        "message_id": 5, "date": 1_790_000_000,
        "chat": {"id": CHAT_ID, "type": "private"},
        "from": {"id": USER_ID, "is_bot": false, "first_name": "Test"},
        "caption": CAPTION,
        "document": {"file_id": DOC_FILE_ID, "file_unique_id": "u-doc",
                     "file_name": NAME, "mime_type": "application/pdf",
                     "file_size": pdf_bytes().len()}}})
}

fn big_update(size: usize) -> Value {
    json!({"update_id": BIG_UPDATE, "message": {
        "message_id": 7, "date": 1_790_000_002,
        "chat": {"id": CHAT_ID, "type": "private"},
        "from": {"id": USER_ID, "is_bot": false, "first_name": "Test"},
        "caption": CAPTION,
        "document": {"file_id": BIG_FILE_ID, "file_unique_id": "u-big",
                     "file_name": BIG_NAME, "mime_type": "application/pdf",
                     "file_size": size}}})
}

fn photo_update() -> Value {
    json!({"update_id": PHOTO_UPDATE, "message": {
        "message_id": 6, "date": 1_790_000_001,
        "chat": {"id": CHAT_ID, "type": "private"},
        "from": {"id": USER_ID, "is_bot": false, "first_name": "Test"},
        "photo": [{"file_id": "PHOTO-1", "file_unique_id": "u-photo",
                   "width": 90, "height": 90, "file_size": 1000}]}})
}

/// A vector of `dim` signs, a pure function of the text: never all zero.
fn vector(text: &str, dim: usize) -> Vec<f64> {
    let bytes = text.as_bytes();
    (0..dim)
        .map(|i| {
            let b = bytes.get(i % bytes.len().max(1)).copied().unwrap_or(0) as usize;
            if (b + i).is_multiple_of(2) { 1.0 } else { -1.0 }
        })
        .collect()
}

/// `(status, content type, payload, delay before answering)`.
fn answer(
    method: &str,
    target: &str,
    body: &[u8],
    docs: &(Vec<u8>, Vec<u8>),
) -> (u16, &'static str, Vec<u8>, Duration) {
    let (pdf, big) = (&docs.0, &docs.1);
    let path = path_of(target);
    let json_ok = |v: Value| {
        (
            200,
            "application/json",
            v.to_string().into_bytes(),
            Duration::ZERO,
        )
    };
    if path.ends_with("/getUpdates") {
        let offset: i64 = query_of(target, "offset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let due: Vec<Value> = [document_update(), photo_update(), big_update(big.len())]
            .into_iter()
            .filter(|u| u["update_id"].as_i64().unwrap_or(0) >= offset)
            .collect();
        let wait = if due.is_empty() {
            Duration::from_millis(400)
        } else {
            Duration::ZERO
        };
        return (
            200,
            "application/json",
            json!({"ok": true, "result": due}).to_string().into_bytes(),
            wait,
        );
    }
    if path.ends_with("/getFile") {
        if query_of(target, "file_id").as_deref() == Some(DOC_FILE_ID) {
            return json_ok(json!({"ok": true, "result": {
                "file_id": DOC_FILE_ID, "file_unique_id": "u-doc",
                "file_size": pdf.len(), "file_path": DOC_FILE_PATH}}));
        }
        if query_of(target, "file_id").as_deref() == Some(BIG_FILE_ID) {
            return json_ok(json!({"ok": true, "result": {
                "file_id": BIG_FILE_ID, "file_unique_id": "u-big",
                "file_size": big.len(), "file_path": BIG_FILE_PATH}}));
        }
        return (
            400,
            "application/json",
            json!({"ok": false, "error_code": 400, "description": "Bad Request: invalid file_id"})
                .to_string()
                .into_bytes(),
            Duration::ZERO,
        );
    }
    if path.starts_with("/file/bot") && path.ends_with(DOC_FILE_PATH) {
        return (200, "application/pdf", pdf.to_vec(), Duration::ZERO);
    }
    if path.starts_with("/file/bot") && path.ends_with(BIG_FILE_PATH) {
        return (200, "application/pdf", big.to_vec(), Duration::ZERO);
    }
    if path == "/v1/embeddings" && method == "POST" {
        let req: Value = sj::from_slice(body).unwrap_or(Value::Null);
        let dim = req["dimensions"]
            .as_u64()
            .map(|d| d as usize)
            .unwrap_or(EMBED_DIM);
        let inputs: Vec<String> = req["input"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|t| t.as_str().unwrap_or("").to_string())
            .collect();
        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"object": "embedding", "index": i, "embedding": vector(t, dim)}))
            .collect();
        return json_ok(
            json!({"object": "list", "data": data, "model": "stub-embed",
                              "usage": {"prompt_tokens": 3, "total_tokens": 3}}),
        );
    }
    if path == "/v1/chat/completions" && method == "POST" {
        return json_ok(json!({
            "id": "chatcmpl-stub", "object": "chat.completion", "created": 0,
            "model": "stub-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": SUMMARY},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18}}));
    }
    // sendMessage, sendChatAction and anything else the connector may call.
    json_ok(json!({"ok": true, "result": true}))
}

fn serve_one(conn: TcpStream, docs: &(Vec<u8>, Vec<u8>), log: &Mutex<Vec<Hit>>) {
    let Ok(read_half) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    if len > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    log.lock().expect("the hit log").push(Hit {
        method: method.clone(),
        target: target.clone(),
        body: body.clone(),
    });
    let (status, ctype, payload, wait) = answer(&method, &target, &body, docs);
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let mut conn = conn;
    let _ = write!(
        conn,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        payload.len()
    );
    let _ = conn.write_all(&payload);
    let _ = conn.flush();
}

// ══════════════════════════════════════════════════════════════ the tree

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write");
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

/// Copy a template cell by cell: `config.json` files and seed files
/// (`*.jsonl`) travel, nothing else, so the tree under test IS the template.
fn copy_template(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_template(&from, &dst.join(&name));
        } else if name == "config.json" || name.to_string_lossy().ends_with(".jsonl") {
            std::fs::copy(&from, dst.join(&name)).expect("copy a template file");
        }
    }
}

/// Set `params.<key>` of the cell at `rel`, the way `override_params` does at
/// instantiation. A key the cell does not have is a fixture error, not a
/// silent addition.
fn set_params(root: &std::path::Path, rel: &str, over: &[(&str, Value)]) {
    let p = root.join(rel).join("config.json");
    let mut cfg = read_json(&p);
    for (k, v) in over {
        assert!(
            cfg["params"].get(*k).is_some(),
            "{rel} has no param {k} to override"
        );
        cfg["params"][*k] = v.clone();
    }
    std::fs::write(&p, sj::to_string_pretty(&cfg).expect("serialise")).expect("write");
}

/// A cell that answers nothing: the holders of the member this round never
/// reaches.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's assistants, doubled: the RECEIVER of the turn, and the
/// model that reads it. Every `in_turn` is answered on `answer` with exactly
/// what arrived -- body, context and hop -- so the assertions read the turn
/// where it landed rather than infer it; and for the address line in it, the
/// double calls `file_read` with `page: 2` on the address, as a `tool` with
/// `tool_name` on the hop and the call in `messages` -- the form the assistant
/// level sends up for every `file_*` call (GH #908). The `tool_result` that
/// comes back on `in_tool` is answered on `answer` as well, marked
/// `saw_tool`.
const ASSISTANT: &str = r#"
import sys, json, re
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
body = doc.get("body") or {}
route = str(hop.get("route") or "")
if route == "in_tool":
    sys.stdout.write(json.dumps({
        "header": {"route": "answer", "saw_tool": "1",
                   "tool_call_id": str(hop.get("tool_call_id") or ""),
                   "error_code": str(hop.get("error_code") or "")},
        "messages": [{"origin": "assistant", "type": "text", "text": "read"}],
        "saw_body": body}))
    sys.exit(0)
if route != "in_turn":
    sys.stdout.write(json.dumps([]))
    sys.exit(0)
out = [{"header": {"route": "answer", "saw_turn": "1"},
        "messages": [{"origin": "assistant", "type": "text", "text": "seen"}],
        "saw_body": body, "saw_context": ctx, "saw_hop": hop}]
for m in body.get("messages") or []:
    hit = re.match(r'\[file (fh-[0-9a-f]{12}@[0-9a-f]{12}) "([^"]*)"', str(m.get("text") or ""))
    if hit:
        call = "read-" + hit.group(2)
        out.append({"header": {"route": "tool", "tool_name": "file_read", "tool_call_id": call},
                    "messages": [{"origin": "assistant", "type": "tool_call", "id": call,
                                  "text": json.dumps({"file": hit.group(1), "page": 2})}]})
sys.stdout.write(json.dumps(out))
"#;

/// A `code` double with a fixed script. `emits` is left wide on purpose: what
/// a double may say is decided by the assertions.
fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        // `contract.multi_send_capable`: the assistant double answers a turn AND calls
        // `file_read` in one invocation (a JSON array). Without the flag the
        // code cell turns the array into a route-less error emission, which
        // matches no member edge and dead-letters as `no_route` (the first
        // run of this lock, OR-FJ.I.20).
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

/// The name the connector is instantiated under in the member's `./channels`.
const CHANNEL: &str = "telegram";

/// The edges the instantiating mutations would draw into the member's graph:
///
/// - the connector's pairing edges (telegram-connector README § *Wiring it*):
///   the turn goes up as `turn` with the chat promoted into context, a failure
///   as `error`. `channel_node` is NOT stamped: the way back into the chat is
///   not under test, and without it the member's own guarded default carries
///   the assistant's answer out of the level to the sink this file reads;
/// - one probe edge out of `./file-space`: the space answers an outside request on
///   `answer` at its own path, and this carries the answers of THIS file's
///   probes (and nothing else) out of the member.
fn instantiation_edges() -> Vec<Value> {
    vec![
        json!({
            "from": format!("./channels/{CHANNEL}"), "to": "./channels",
            "condition": "!has(hop.error_code)",
            "modifier": {
                "set_hop": {"route": "'turn'"},
                "set_context": {
                    "channel": "has(hop.chat_id) ? hop.chat_id : ''",
                    "chat_id": "has(hop.chat_id) ? hop.chat_id : ''",
                    "user_id": "has(hop.user_id) ? hop.user_id : ''"
                }
            }
        }),
        json!({
            "from": format!("./channels/{CHANNEL}"), "to": "./channels",
            "condition": "has(hop.error_code)",
            "modifier": {"set_hop": {"route": "'error'"}}
        }),
        // The way back into the chat (README § *Wiring it*, third edge). The
        // connector requires `context.chat_id`, and the boot check wants a
        // setter reachable backwards over an edge INTO it (validate.rs, context
        // reachability) -- without this edge the member does not boot. It never
        // fires here: `channel_node` is not stamped, so the answer takes the
        // member's guarded default out to the sink this file reads.
        json!({
            "from": "./channels", "to": format!("./channels/{CHANNEL}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'answer' && has(context.channel_node) \
                 && context.channel_node == '{CHANNEL}'")
        }),
        json!({
            "from": "./file-space", "to": ".",
            "condition": "has(hop.route) && hop.route == 'answer' && has(hop.op_id) \
                          && hop.op_id.startsWith('probe-')"
        }),
    ]
}

/// The colony around the member: a drain for every lane the member emits (an
/// undrained lane is a dead letter, and this file would then read a silence).
fn main_config() -> Value {
    let edges: Vec<Value> = [
        "answer",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "build",
        "close_report",
        "export_done",
    ]
    .iter()
    .map(|lane| {
        json!({"from": "./person", "to": "/sink",
               "condition": format!("has(hop.route) && hop.route == '{lane}'")})
    })
    .collect();
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// The member's `./file-space` as instantiation lays it out: its `ref`
/// marker replaced by the referenced template's tree, the marker's
/// `override_params` applied to the cells they name (the reader of
/// `gh908_a_model_reads_a_file_through_its_tools.rs`).
fn resolve_file_space(root: &std::path::Path) {
    let node = root.join("main/person/file-space");
    assert_eq!(
        read_json(&node.join("config.json"))["cell"]["type"],
        "ref",
        "the member ships ./file-space as a ref to the template"
    );
    resolve_ref(root, "main/person/file-space", 0);
}

/// Replace the ref marker at `rel` by its template's tree, apply the marker's
/// `override_params`, and do the same for every ref below it -- the file
/// space carries its own child ref, `./projection` (GH #905).
fn resolve_ref(root: &std::path::Path, rel: &str, depth: usize) {
    assert!(depth < 8, "template ref chain does not terminate at {rel}");
    let node = root.join(rel);
    let marker = read_json(&node.join("config.json"));
    if marker["cell"]["type"] == "ref" {
        let reference = marker["cell"]["template"]
            .as_str()
            .expect("a ref names a template");
        let name = reference.split('@').next().unwrap_or_default();
        let _ = std::fs::remove_dir_all(&node);
        copy_template(&repo("templates").join(name), &node);
        resolve_children(root, rel, depth + 1);
        if let Some(over) = marker["override_params"].as_object() {
            for (cell, params) in over {
                let pairs: Vec<(&str, Value)> = params
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
                    .unwrap_or_default();
                set_params(root, &format!("{rel}/{cell}"), &pairs);
            }
        }
    } else {
        resolve_children(root, rel, depth);
    }
}

fn resolve_children(root: &std::path::Path, rel: &str, depth: usize) {
    let mut subdirs: Vec<String> = std::fs::read_dir(root.join(rel))
        .expect("readable")
        .filter_map(Result::ok)
        .filter(|e| e.path().join("config.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    subdirs.sort();
    for sub in subdirs {
        resolve_ref(root, &format!("{rel}/{sub}"), depth);
    }
}

/// Every `${VAR}` any config below `dir` names, so the test `.env` can give
/// each a stub value and no boot reaches for a real credential.
fn env_names(dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let from = entry.expect("entry").path();
        if from.is_dir() {
            env_names(&from, out);
        } else if from.file_name().is_some_and(|n| n == "config.json") {
            let raw = std::fs::read_to_string(&from).unwrap_or_default();
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let name = &rest[..end];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    out.insert(name.to_string());
                }
                rest = &rest[end + 1..];
            }
        }
    }
}

fn build_tree(root: &std::path::Path, up: &Upstream) {
    write(root, "main/config.json", &main_config());
    copy_template(&repo("templates/member"), &root.join("main/person"));

    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "graph-space",
        "librarian",
        "objects",
    ] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this round never reaches."),
        );
    }
    write(
        root,
        "main/person/assistants/config.json",
        &double(
            ASSISTANT,
            "Test double for the member's assistants: the receiver of the turn.",
        ),
    );

    // The real firewall: a turn has to pass it once, and only once.
    let fw = root.join("main/person/firewall");
    let _ = std::fs::remove_dir_all(&fw);
    copy_template(&repo("templates/firewall"), &fw);

    resolve_file_space(root);
    set_params(
        root,
        "main/person/file-space/embed",
        &[
            ("endpoint", json!(format!("{}/v1/embeddings", up.base))),
            ("model", json!("stub-embed")),
            ("dim", json!(EMBED_DIM.to_string())),
        ],
    );
    set_params(
        root,
        "main/person/file-space/summarizer",
        &[
            ("model", json!("stub-model")),
            ("base_url", json!(format!("{}/v1", up.base))),
            ("api_key", json!("stub-key")),
        ],
    );

    // The connector, as the channel's instantiation grows it.
    let conn_rel = format!("main/person/channels/{CHANNEL}");
    write(
        root,
        &format!("{conn_rel}/config.json"),
        &read_json(&repo("templates/telegram-connector/config.json")),
    );
    set_params(
        root,
        &conn_rel,
        &[
            ("bot_token", json!(BOT_TOKEN)),
            // GH #1061: the template names a grant, and a grant wins over the
            // literal -- this tree grows no broker, so the stub token rides
            // the one-release literal road with the grant emptied.
            ("bot_token_grant_id", json!("")),
            ("base_url", json!(up.base)),
            ("long_poll_request_secs", json!(1)),
            ("long_poll_timeout_ms", json!(5000)),
        ],
    );

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph")
        .extend(instantiation_edges());
    std::fs::write(&cfg_path, sj::to_string_pretty(&cfg).expect("serialise"))
        .expect("write the member config");

    // Every `${VAR}` a shipped config still names (a contract default, the
    // embed cell's fallback key, the space's model role) resolves to a stub
    // value; no real credential, no real endpoint.
    let mut names = std::collections::BTreeSet::new();
    env_names(&root.join("main"), &mut names);
    let mut env: std::collections::BTreeMap<String, String> = names
        .into_iter()
        .map(|n| (n.clone(), format!("stub-{n}")))
        .collect();
    env.insert("TELEGRAM_BOT_TOKEN".into(), BOT_TOKEN.into());
    env.insert("OPENROUTER_API_KEY".into(), "stub-key".into());
    env.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", up.base),
    );
    let body: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the test .env");
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    let proxy: Arc<dyn CellFactory> =
        Arc::new(ProxyCellFactory::new(Arc::new(SurfaceRegistry::new())));
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "store".to_string(),
            Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "llm".to_string(),
            Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "timer".to_string(),
            Arc::new(TimerCellFactory) as Arc<dyn CellFactory>,
        ),
        ("proxy".to_string(), proxy),
    ]
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let fs = factories();
    // The colony's blob store at `<root>/blobs`: the connector writes the
    // document there, and a body over 64 KiB moves there (R-FJ-1).
    let h = ColonyHandle::new_with_blobs_at(td, fs.clone());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member, firewall, file space and connector must boot");
    (h, sink_rx)
}

// ══════════════════════════════════════════════════════════════ reading

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn body_of(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(id) => panic!("an inline body was expected, got blob {id}"),
    }
}

fn is_turn(m: &Message) -> bool {
    hop_str(m, "saw_turn") == "1"
}

fn is_tool(m: &Message) -> bool {
    hop_str(m, "saw_tool") == "1"
}

/// Everything the sink received, kept, so a later wait sees an earlier arrival.
struct Sink {
    rx: mpsc::Receiver<Message>,
    seen: Vec<Message>,
    /// The colony root, read on a timeout for what the colony did instead.
    root: std::path::PathBuf,
}

impl Sink {
    fn account(&self) -> Vec<String> {
        self.seen
            .iter()
            .map(|m| format!("{:?}", m.headers.hop))
            .collect()
    }

    async fn until(
        &mut self,
        what: &str,
        secs: u64,
        up: &Upstream,
        pred: impl Fn(&Message) -> bool,
    ) -> Message {
        if let Some(m) = self.seen.iter().find(|m| pred(m)) {
            return m.clone();
        }
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(m)) => {
                    let hit = pred(&m);
                    self.seen.push(m.clone());
                    if hit {
                        return m;
                    }
                }
                _ => panic!(
                    "{what} did not reach the sink within {secs}s.\nsink: {:#?}\n\
                     colony: {:#?}\nupstream: {:#?}",
                    self.account(),
                    colony_account(&self.root),
                    up.trace()
                ),
            }
        }
    }

    /// Take in whatever arrives within `d`.
    async fn settle(&mut self, d: Duration) {
        let deadline = Instant::now() + d;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(m)) => self.seen.push(m),
                _ => return,
            }
        }
    }
}

/// One request into the file space's `in_read` door from outside.
fn probe(op: &str, op_id: &str, file: &str, args: Value) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_read"));
    hop.insert("op".into(), json!(op));
    hop.insert("op_id".into(), json!(op_id));
    MessageBuilder::new(Path::new("/person/file-space"))
        .body(Body::Inline(
            json!({"op": op, "file": file, "args": args, "messages": []}),
        ))
        .hop(hop)
        .ttl(200)
        .build()
}

fn store_db(root: &std::path::Path) -> std::path::PathBuf {
    root.join("main/person/file-space/store/cell.db")
}

fn query_strings(db: &std::path::Path, sql: &str, param: &str) -> Vec<String> {
    let conn =
        rusqlite::Connection::open(db).unwrap_or_else(|e| panic!("open {}: {e}", db.display()));
    let mut st = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    st.query_map([param], |r| r.get::<_, String>(0))
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .map(|r| r.expect("a text column"))
        .collect()
}

fn query_count(db: &std::path::Path, sql: &str, param: &str) -> i64 {
    let conn =
        rusqlite::Connection::open(db).unwrap_or_else(|e| panic!("open {}: {e}", db.display()));
    conn.query_row(sql, [param], |r| r.get::<_, i64>(0))
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// `[file fh-<12 hex>@<12 hex> "<name>", <pages> pages: <ONELINE>]` → (`fh-…`, v12).
fn parse_address_line(line: &str, name: &str, pages: usize) -> Option<(String, String)> {
    let rest = line.strip_prefix("[file fh-")?;
    let (id, rest) = rest.split_at_checked(12)?;
    let rest = rest.strip_prefix('@')?;
    let (v12, rest) = rest.split_at_checked(12)?;
    let tail = format!(" \"{name}\", {pages} pages: {ONELINE}]");
    (is_hex(id, 12) && is_hex(v12, 12) && rest == tail)
        .then(|| (format!("fh-{id}"), v12.to_string()))
}

/// One row of the colony's message log, as far as this file reads it.
struct Logged {
    id: String,
    parent: Option<String>,
    from: String,
    to: String,
    ttl: i64,
    headers: String,
    body_kind: String,
    body: String,
}

/// On a timeout: every dead letter, then the last hops of the message log
/// (from, to, route) -- where the turn went instead of the sink.
fn colony_account(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(conn) = rusqlite::Connection::open(root.join("colony.db"))
        && let Ok(mut st) = conn.prepare(
            "SELECT sender_path, resolved_target, error_code, \
             COALESCE(detail, '') || ' ' || substr(message_json, 1, 1500) \
             FROM dead_letters ORDER BY id",
        )
        && let Ok(rows) = st.query_map([], |r| {
            Ok(format!(
                "DEAD {} -> {}: {} {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })
    {
        out.extend(rows.filter_map(Result::ok));
    }
    let log = message_log(root);
    let skip = log.len().saturating_sub(60);
    for r in log.iter().skip(skip) {
        let route = sj::from_str::<Value>(&r.headers)
            .ok()
            .and_then(|h| {
                h.pointer("/hop/route")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        out.push(format!("{} -> {} [{route}] ttl={}", r.from, r.to, r.ttl));
    }
    out
}

/// OR-FJ.I.21, narrowed by GH #929: a document turn reaches the assistants
/// with at least the floor below, and the door of the generation it enters
/// restores it to 63 again. The ingest
/// detour (store, extract, write, derive with its embedding and summary
/// rounds) spends the hop budget: measured before the fix, both turns (2 and
/// 30 pages) left the firewall with ttl=55 and reached the assistants with
/// ttl=10 of 64, and every emission of the assistant inherits that rest (the
/// double's `file_read` died `ttl_expired` at `store -> read`). The two
/// requests out of `ingest` restore it for the detour itself; since GH #929
/// the finished turn leaves the space without a restore, because the door of
/// the generation it enters (`in_turn`, two decisions behind the container
/// this double stands for) restores it again.
fn check_turn_ttl(root: &std::path::Path) {
    let log = message_log(root);
    let route = |r: &Logged| {
        sj::from_str::<Value>(&r.headers)
            .ok()
            .and_then(|h| {
                h.pointer("/hop/route")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    };
    let left_firewall: Vec<i64> = log
        .iter()
        .filter(|r| r.from == "/person/firewall" && r.to == "/person/file-space")
        .map(|r| r.ttl)
        .collect();
    let arrived: Vec<i64> = log
        .iter()
        .filter(|r| r.to == "/person/assistants" && route(r) == "in_turn")
        .map(|r| r.ttl)
        .collect();
    // m-2 of the closing review: the detour itself lives on the budget too.
    // `./ingest -> ./extract` and `./ingest -> ./write` restore it, so a
    // `path_taken` retry or a turn that came in short cannot starve the chain
    // inside the space. (A hive's own out-edge restores in the transit too,
    // GH #929 `s0_a_hive_out_edge_restores_in_transit`.) Only the detour
    // counts: what the double sends once the turn is in (its `file_read`)
    // starts, in a real member, behind the generation's `in_turn` door, which
    // restores (GH #929) -- the double stands where the container stands and
    // has no such door.
    let by_id: std::collections::HashMap<&str, &Logged> =
        log.iter().map(|r| (r.id.as_str(), r)).collect();
    let after_the_turn = |r: &Logged| {
        let mut at = Some(r);
        while let Some(x) = at {
            if x.to == "/person/assistants" {
                return true;
            }
            at = x.parent.as_deref().and_then(|p| by_id.get(p).copied());
        }
        false
    };
    let in_space = log
        .iter()
        .filter(|r| r.to.starts_with("/person/file-space/") && !after_the_turn(r))
        .map(|r| r.ttl)
        .min()
        .unwrap_or(0);
    eprintln!(
        "GH907-TTL left_firewall={left_firewall:?} at_assistants={arrived:?} \
         min_in_space={in_space}"
    );
    // Measured: 10 without the two restores (the whole detour on the 55 the
    // turn left the firewall with), 22 with them -- the write -> derive leg
    // alone spends some 42 hops, and each `path_taken` retry now starts it
    // on a full budget. The floor is a quarter of the budget.
    assert!(
        in_space >= 16,
        "no hop inside the space runs below a quarter of the budget: min {in_space}"
    );
    assert_eq!(
        left_firewall.len(),
        2,
        "two document turns left the firewall"
    );
    assert_eq!(
        arrived.len(),
        2,
        "two document turns reached the assistants"
    );
    // GH #929: the turn carries what the segment from the space's last
    // restoring edge (63 behind it) to the generation's door may leave -- at
    // most 48 spent (reserve 16), plus 2: the two decisions are `TO_THE_DOOR`
    // of the chain lock, counted here from the arrival itself, so this floor is
    // one decision stricter than the lock (which counts them from the arrival's
    // parent) -- it can be red where the lock is green, never the other way
    // round. The lock measures the small document; this floor holds both, the
    // 30-page one included.
    const FLOOR: i64 = 63 - 48 + 2;
    assert!(
        arrived.iter().all(|t| *t >= FLOOR),
        "a document turn reaches the assistants with at least {FLOOR} of its budget \
         (left the firewall with {left_firewall:?}); arrived with {arrived:?}"
    );
}

fn message_log(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT from_path, to_path, headers, body_kind, body_payload, ttl, id, \
             parent_message_id FROM message_log ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        Ok(Logged {
            from: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            to: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            headers: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            body_kind: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            body: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ttl: r.get::<_, Option<i64>>(5)?.unwrap_or_default(),
            id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            parent: r.get::<_, Option<String>>(7)?,
        })
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

/// A logged body as JSON text: inline as stored, a blob body read back from
/// the colony's store (`blobs/<uuid>.json`, the offload's file). A blob that
/// is gone reads as the empty string -- the callers then find nothing in it,
/// so they also check that the bodies they judge were read.
fn body_text(root: &std::path::Path, r: &Logged) -> String {
    if r.body_kind == "blob" {
        return std::fs::read_to_string(root.join("blobs").join(format!("{}.json", r.body)))
            .unwrap_or_default();
    }
    r.body.clone()
}

/// The content of the blob `id` in the colony's store: the one file
/// `blobs/<id>.<ext>` that is neither a sidecar nor a temporary.
fn blob_bytes(root: &std::path::Path, id: &str) -> Vec<u8> {
    let dir = root.join("blobs");
    let hits: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.starts_with(&format!("{id}.")) && !n.ends_with(".meta.json") && !n.ends_with(".tmp")
        })
        .collect();
    assert_eq!(hits.len(), 1, "one content file for blob {id}: {hits:?}");
    std::fs::read(&hits[0]).unwrap_or_else(|e| panic!("{}: {e}", hits[0].display()))
}

/// What must never stand where it is scanned: the PDF in any base64 form, the
/// PDF raw, any page's text of the large one.
fn carries_doc_bytes(s: &str) -> bool {
    s.contains(PDF_B64_MAGIC) || s.contains("%PDF")
}

fn carries_text_view(s: &str) -> bool {
    s.contains(MARKER)
}

/// The connector's turn for `name`, the first row it wrote to the message log:
/// ONE attachment, the reference to the blob holding exactly `bytes`, and no
/// bytes in the body.
fn check_emission(root: &std::path::Path, log: &[Logged], name: &str, bytes: &[u8]) {
    // The connector's emission is a source message (no parent): the substrate
    // logs its first hop -- into `/person/channels`, the hive the pairing edge
    // leaves the connector through -- with the `@external` sender sentinel
    // (colony.rs, `from_path` of a row without `parent_message_id`).
    let is_conn = |r: &Logged| r.from == "@external" && r.to == "/person/channels";
    let row = log
        .iter()
        .find(|r| is_conn(r) && body_text(root, r).contains(&format!("\"{name}\"")))
        .unwrap_or_else(|| {
            let from_conn: Vec<String> = log
                .iter()
                .filter(|r| is_conn(r))
                .map(|r| {
                    let t = body_text(root, r);
                    format!("[{}] {}", r.body_kind, &t[..t.len().min(400)])
                })
                .collect();
            panic!("the connector's turn for {name} is in the message log; from it: {from_conn:#?}")
        });
    let text = body_text(root, row);
    assert!(
        !text.contains("data_b64") && !carries_doc_bytes(&text),
        "{name}: the connector's turn carries a reference, never the bytes"
    );
    let body: Value = sj::from_str(&text).unwrap_or_else(|e| panic!("{name}: body: {e}"));
    let att = body["attachments"]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: the turn carries `attachments`: {body}"));
    assert_eq!(att.len(), 1, "{name}: one attachment: {body}");
    let a = &att[0];
    assert_eq!(a["filename"], json!(name), "{name}: {a}");
    assert_eq!(a["mime_type"], json!("application/pdf"), "{name}: {a}");
    assert_eq!(
        a["size_bytes"],
        json!(bytes.len()),
        "{name}: size_bytes: {a}"
    );
    assert_eq!(a["sha256"], json!(sha256_hex(bytes)), "{name}: sha256: {a}");
    let id = a["blob_id"].as_str().unwrap_or_default();
    assert!(
        meclaw_core::Uuid::parse_str(id).is_ok(),
        "{name}: blob_id is a uuid: {a}"
    );
    assert!(
        blob_bytes(root, id) == bytes,
        "{name}: the blob holds exactly the bytes that were sent"
    );
    assert!(
        !body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["type"] == "file"),
        "{name}: no file element beside the reference: {body}"
    );
}

/// The turn at the assistant that names `name`, checked: no `attachments`
/// slot, no file element, the caption followed by ONE address line, no bytes
/// and no page text anywhere. Returns (`fh-…`, v12).
fn check_turn(turn: &Message, name: &str, pages: usize) -> (String, String) {
    let saw = body_of(turn);
    assert!(
        saw["saw_body"].get("attachments").is_none(),
        "{name}: the turn reaches the assistant without an `attachments` slot: {}",
        saw["saw_body"]
    );
    let msgs = saw["saw_body"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the assistant saw a turn without messages: {saw}"));
    let texts: Vec<String> = msgs
        .iter()
        .filter(|m| m["type"] == "text")
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        msgs.iter().all(|m| m["type"] != "file"),
        "no file element reaches the assistant: {texts:?}"
    );
    let caption = texts
        .iter()
        .position(|t| t == CAPTION)
        .unwrap_or_else(|| panic!("the caption travels on unchanged: {texts:?}"));
    let lines: Vec<&String> = texts.iter().filter(|t| t.starts_with("[file ")).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one address line in the turn: {texts:?}"
    );
    assert!(
        texts.get(caption + 1) == Some(lines[0]),
        "the address line follows the caption: {texts:?}"
    );
    let parsed = parse_address_line(lines[0], name, pages).unwrap_or_else(|| {
        panic!(
            "the line has the form [file fh-<12 hex>@<12 hex> \"{name}\", {pages} pages: \
             {ONELINE}], got {:?}",
            lines[0]
        )
    });
    let everything = format!(
        "{}{}{}",
        saw["saw_body"], saw["saw_context"], saw["saw_hop"]
    );
    assert!(
        !everything.contains("data_b64") && !carries_doc_bytes(&everything),
        "the PDF's bytes reached the assistant in some form"
    );
    assert!(
        !carries_text_view(&everything),
        "the PDF's text reached the assistant with the turn"
    );
    parsed
}

/// The file behind an address, read out of the space's own store: born under
/// `/inbox/<date>/<name>` with exactly `bytes`, the line naming its head, one
/// derived row per entry of `pages` (each containing its entry), the one
/// line, embeddings. Returns the length of all page texts together.
fn check_file(
    db: &std::path::Path,
    file: &str,
    v12: &str,
    name: &str,
    days: &[String],
    bytes: &[u8],
    pages: &[String],
) -> usize {
    let paths = query_strings(db, "SELECT path FROM files WHERE file = ?1 LIMIT 5", file);
    let expected: Vec<String> = days.iter().map(|d| format!("/inbox/{d}/{name}")).collect();
    assert!(
        paths.len() == 1 && expected.contains(&paths[0]),
        "{name} was born under /inbox/<UTC date>/{name}: {paths:?}"
    );
    assert_eq!(
        query_count(db, "SELECT bytes FROM files WHERE file = ?1 LIMIT 1", file),
        bytes.len() as i64,
        "{name}: the space holds exactly the bytes that were sent"
    );
    let head = query_strings(db, "SELECT head FROM files WHERE file = ?1 LIMIT 1", file);
    assert_eq!(
        head,
        vec![sha256_hex(bytes)],
        "{name}: the head version is the sha256 of the bytes that were sent"
    );
    assert!(
        head[0].starts_with(v12),
        "{name}: the line names the head: @{v12}"
    );
    let got = query_strings(
        db,
        "SELECT body FROM derived WHERE file = ?1 AND kind = 'text' ORDER BY part LIMIT 100",
        file,
    );
    assert_eq!(got.len(), pages.len(), "{name}: one derived row per page");
    for (i, (text, needle)) in got.iter().zip(pages).enumerate() {
        assert!(
            text.contains(needle.as_str()),
            "{name}: page {} reads {needle:?}: {:?}",
            i + 1,
            text.chars().take(200).collect::<String>()
        );
    }
    assert_eq!(
        query_strings(
            db,
            "SELECT text FROM summaries WHERE file = ?1 AND level = 'oneline' LIMIT 5",
            file
        ),
        vec![ONELINE.to_string()],
        "{name}: the one-line summary is written"
    );
    assert!(
        query_count(db, "SELECT COUNT(*) FROM embeddings WHERE file = ?1", file) > 0,
        "{name}: the file has embeddings"
    );
    got.iter().map(String::len).sum()
}

/// Every row of the space's `pending`, as `cell|op_id|phase|body`.
fn pending_rows(db: &std::path::Path) -> Vec<String> {
    let conn =
        rusqlite::Connection::open(db).unwrap_or_else(|e| panic!("open {}: {e}", db.display()));
    let mut st = conn
        .prepare(
            "SELECT COALESCE(cell,''), COALESCE(op_id,''), COALESCE(phase,''), \
             COALESCE(body,'') FROM pending",
        )
        .expect("pending");
    st.query_map([], |r| {
        Ok(format!(
            "{}|{}|{}|{}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?
        ))
    })
    .expect("query pending")
    .filter_map(Result::ok)
    .collect()
}

/// The `pending` writes (`insert`/`update`/`upsert` on table `pending`) in one
/// body sent to the space's store: the tool_call texts, each a store op.
fn pending_writes(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["text"].as_str())
        .filter_map(|t| sj::from_str::<Value>(t).ok().map(|op| (t.to_string(), op)))
        .filter(|(_, op)| {
            op["table"] == "pending"
                && ["insert", "update", "upsert"]
                    .iter()
                    .any(|w| op["operation"] == *w)
        })
        .map(|(t, _)| t)
        .collect()
}

// ══════════════════════════════════════════════════════════════ the measurement

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pdf_sent_to_the_bot_reaches_the_assistant_as_an_address_and_is_read_by_it() {
    if !shipped() {
        eprintln!(
            "member/file-space, file-space/tools, firewall or telegram-connector did not \
             travel into this tree -- skipped (GH #49)"
        );
        return;
    }
    assert!(
        std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_ok(),
        "`pdftotext` (poppler-utils) is required: the file space's ./extract runs it"
    );

    let pdf = pdf_bytes();
    let (big, markers) = big_pdf(&run_tag());
    assert!(
        big.len() > PAD_BYTES,
        "the large PDF is a few MiB: {}",
        big.len()
    );
    let up = Upstream::start(pdf.clone(), big.clone());
    let td = tempfile::tempdir().expect("a temporary directory");
    let day_before = today();
    build_tree(td.path(), &up);
    let (h, rx) = boot(&td).await;
    let mut sink = Sink {
        rx,
        seen: Vec::new(),
        root: td.path().to_path_buf(),
    };
    let named = |name: &'static str| {
        move |m: &Message| is_turn(m) && body_of(m).to_string().contains(&format!("\\\"{name}\\\""))
    };

    // ── 2. both turns at the assistant ───────────────────────────────────────
    let small_turn = sink
        .until("the small document's turn", ROAD_SECS, &up, named(NAME))
        .await;
    let big_turn = sink
        .until("the large document's turn", ROAD_SECS, &up, named(BIG_NAME))
        .await;
    let days = [day_before, today()];
    check_turn_ttl(td.path());
    let (small_id, small_v12) = check_turn(&small_turn, NAME, 2);
    let (big_id, big_v12) = check_turn(&big_turn, BIG_NAME, BIG_PAGES);
    assert_ne!(small_id, big_id, "two documents, two files");

    // ── 1. the files in the space, out of its own store ──────────────────────
    let db = store_db(td.path());
    let small_pages = vec!["page one".to_string(), "page two".to_string()];
    check_file(&db, &small_id, &small_v12, NAME, &days, &pdf, &small_pages);
    let text_view = check_file(&db, &big_id, &big_v12, BIG_NAME, &days, &big, &markers);
    assert!(
        text_view > 64 * 1024,
        "the large PDF's text is over the 64 KiB blob limit: {text_view} B"
    );
    assert!(
        up.count("/v1/embeddings") > 0,
        "the embeddings came from the endpoint"
    );
    assert!(
        up.count("/v1/chat/completions") > 0,
        "the summary came from the model"
    );

    // ── 1 again, through the address: `info` at the space's door ────────────
    h.send(probe("info", "probe-info", &small_id, json!({})))
        .await;
    let info = sink
        .until("the answer to `info`", PROBE_SECS, &up, |m| {
            hop_str(m, "op_id") == "probe-info"
        })
        .await;
    let info = body_of(&info);
    assert_eq!(info["ok"], true, "`info` on the address answers: {info}");
    assert!(
        days.iter()
            .any(|d| info["path"] == json!(format!("/inbox/{d}/{NAME}"))),
        "`info` names the inbox path: {info}"
    );
    assert_eq!(info["pages"], json!(2), "`info` counts two pages: {info}");
    assert_eq!(
        info["oneline"],
        json!(ONELINE),
        "`info` carries the one line: {info}"
    );

    // ── 3. page two of each, read by the model's `file_read` ─────────────────
    for (name, two, one) in [
        (NAME, small_pages[1].as_str(), small_pages[0].as_str()),
        (BIG_NAME, markers[1].as_str(), markers[0].as_str()),
    ] {
        let call = format!("read-{name}");
        let result = sink
            .until(
                &format!("the tool_result of {call}"),
                PROBE_SECS,
                &up,
                |m| is_tool(m) && hop_str(m, "tool_call_id") == call,
            )
            .await;
        assert_eq!(
            hop_str(&result, "error_code"),
            "",
            "{call} answers without error"
        );
        let text = body_of(&result)["saw_body"]["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            text.contains(two),
            "{call}: page 2 reads {two:?}: {}",
            text.chars().take(200).collect::<String>()
        );
        assert!(
            !text.contains(one),
            "{call}: and only page 2, never page 1 ({one:?})"
        );
    }

    // ── 4. all three updates acknowledged, the photo reached nobody ──────────
    let deadline = Instant::now() + Duration::from_secs(PROBE_SECS);
    while up.poll_offsets().len() < 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let offsets = up.poll_offsets();
    assert!(
        offsets.len() >= 2,
        "the connector polled again after the batch: {:?}",
        up.trace()
    );
    assert!(
        offsets[0] <= DOC_UPDATE,
        "the first poll fetched the batch: {offsets:?}"
    );
    assert_eq!(
        offsets[1],
        BIG_UPDATE + 1,
        "the next getUpdates acknowledges ALL three updates -- the photo is \
         acknowledged, not fetched again: {offsets:?}"
    );
    assert!(
        offsets[1..].iter().all(|o| *o == BIG_UPDATE + 1),
        "no later poll steps back behind the batch: {offsets:?}"
    );
    let mut get_files: Vec<String> = up
        .hits()
        .iter()
        .filter(|h| path_of(&h.target).ends_with("/getFile"))
        .filter_map(|h| query_of(&h.target, "file_id"))
        .collect();
    get_files.sort();
    assert_eq!(
        get_files,
        vec![BIG_FILE_ID.to_string(), DOC_FILE_ID.to_string()],
        "only the documents were fetched, each once, never the photo"
    );
    sink.settle(Duration::from_secs(3)).await;
    let turns = sink.seen.iter().filter(|m| is_turn(m)).count();
    assert_eq!(
        turns,
        2,
        "exactly TWO turns at the assistant: one per document, each passed by the \
         firewall once, and none for the photo. sink: {:#?}",
        sink.account()
    );

    // ── 5. the document behind the connector: a blob, nowhere else (R-FJ-1) ─
    let root = td.path();
    let log = message_log(root);
    check_emission(root, &log, NAME, &pdf);
    check_emission(root, &log, BIG_NAME, &big);

    // Every header in the log: small, no bytes, no page text. Counted first,
    // printed, then judged -- the line is the measurement even when it fails.
    let max_header = log.iter().map(|r| r.headers.len()).max().unwrap_or(0);
    let heavy: Vec<&str> = log
        .iter()
        .filter(|r| r.headers.len() > MAX_HEADER_BYTES)
        .map(|r| r.to.as_str())
        .collect();
    let with_bytes: Vec<&str> = log
        .iter()
        .filter(|r| carries_doc_bytes(&r.headers))
        .map(|r| r.to.as_str())
        .collect();
    let with_text: Vec<&str> = log
        .iter()
        .filter(|r| carries_text_view(&r.headers))
        .map(|r| r.to.as_str())
        .collect();

    // Every `pending` write the space's store was asked for.
    let store_path = "/person/file-space/store";
    let mut unread = 0usize;
    let mut pending_with_doc: Vec<String> = Vec::new();
    for r in log.iter().filter(|r| r.to == store_path) {
        let text = body_text(root, r);
        let Ok(body) = sj::from_str::<Value>(&text) else {
            unread += 1;
            continue;
        };
        for op in pending_writes(&body) {
            if carries_doc_bytes(&op) || carries_text_view(&op) {
                let at = op
                    .find("%PDF")
                    .or_else(|| op.find(PDF_B64_MAGIC))
                    .or_else(|| op.find(MARKER))
                    .unwrap_or(0);
                let lo = op.floor_char_boundary(at.saturating_sub(160));
                let hi = op.ceil_char_boundary((at + 80).min(op.len()));
                pending_with_doc.push(format!("{} -> {}: …{}…", r.from, r.body_kind, &op[lo..hi]));
            }
        }
    }

    eprintln!(
        "GH907-MEASURE doc_bytes={} max_header_bytes={max_header} headers_with_doc_bytes={} \
         headers_with_text_view={} pending_ops_with_doc={} rows_scanned={}",
        big.len(),
        with_bytes.len(),
        with_text.len(),
        pending_with_doc.len(),
        log.len()
    );
    assert!(
        log.iter().any(|r| r.to == store_path),
        "the space's store was written to (the scan below judged something)"
    );
    assert_eq!(unread, 0, "every body sent to the store could be read back");
    assert!(
        heavy.is_empty(),
        "no message_log header over {MAX_HEADER_BYTES} B (max {max_header} B): {heavy:?}"
    );
    assert!(
        with_bytes.is_empty(),
        "no header carries the document's bytes: {with_bytes:?}"
    );
    assert!(
        with_text.is_empty(),
        "no header carries any page's text: {with_text:?}"
    );
    assert!(
        pending_with_doc.is_empty(),
        "no `pending` write carries the document's bytes or text: {pending_with_doc:?}"
    );
    for r in log.iter().filter(|r| {
        r.to.starts_with("/person/assistants") || r.to.starts_with("/person/file-space/tools")
    }) {
        let all = format!("{}{}", r.headers, body_text(root, r));
        assert!(
            !all.contains("data_b64") && !carries_doc_bytes(&all),
            "bytes reached {} in a header or body",
            r.to
        );
    }

    // At the end `pending` holds nothing of the two documents, and no staged
    // content was left behind.
    let pending = pending_rows(&db);
    let open: Vec<&String> = pending
        .iter()
        .filter(|r| r.starts_with("ingest|") || r.starts_with("write"))
        .collect();
    assert!(
        open.is_empty(),
        "`pending` holds no row of ingest or write once both documents are through: {:?}",
        open.iter()
            .map(|r| r.chars().take(80).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        !pending
            .iter()
            .any(|r| carries_doc_bytes(r) || carries_text_view(r)),
        "no `pending` row carries the bytes or the text of a document"
    );
    for table in ["blocks", "derived"] {
        assert_eq!(
            query_count(
                &db,
                &format!("SELECT COUNT(*) FROM {table} WHERE file LIKE ?1"),
                "stage:%"
            ),
            0,
            "`{table}` keeps no staged rows"
        );
    }

    h.shutdown().await;
}
