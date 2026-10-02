//! The file road of GH #929's chain-length locks (S5): a PDF sent to the bot
//! becomes a file in the member's file space, and the turn that names it
//! reaches the member's assistants. Taken over from
//! `gh907_a_pdf_from_telegram_is_read_by_its_address.rs`, slimmed to ONE small
//! document (the fixture `two_pages.pdf`): the SHIPPED `member`, `firewall`,
//! `file-space` and `telegram-connector` templates, cell for cell. The
//! connector is the real `proxy` cell against a Bot API stub on 127.0.0.1, the
//! space's `./embed` and `./summarizer` speak HTTP to stubs on the same
//! listener, `./assistants` is a `code` double that answers every `in_turn`,
//! and the holders this road never reaches (`access`, `affinity`,
//! `memory-hive`) are inert doubles. The colony runs with its blob store at
//! `<root>/blobs`, the one the connector writes the document into.
//!
//! What it adds is the measurement, read off the test colony's own
//! `message_log` along the parent chain of the turn's arrival at the double
//! (`crate::road::segment_before`). A seam is where the budget starts again:
//! the fresh root (the connector's emission, a source without a parent) and
//! the delivery right behind every edge that restores in the variant under
//! test. A variant rewrites the COPY of the space's graph in the temp tree,
//! never the template:
//!
//! - `Shipped`: the graph as shipped -- `./ingest -> ./extract`
//!   (`in_extract`) and `./ingest -> ./write` (`in_write`, caller `ingest`)
//!   restore, `./ingest -> .` (`turn`) does not: the generation's `in_turn`
//!   door restores a few decisions later (OR-BD-65);
//! - `NoRestores`: none of the space's four road edges restores;
//! - `DoorOnly`: only the space's door `. -> ./ingest` (`in_ingest`) restores,
//!   a hive out-edge that restores in the transit (S0).
//!
//! A run may also RACE the document's path (`raced`): the copy of the
//! space's `./write` finds the first `raced` candidate paths of the document
//! taken, the way a writer outside `./ingest` would have taken them between
//! its search and its `create`. `./write` answers `path_taken`, `./ingest`
//! searches again (at most `TRIES` = 3 times), and the whole leg -- a second
//! `./extract`, the `create` -- runs again. Two races are the longest road on
//! which the document is still stored.
//!
//! The including test file declares `#[path = "support/gh929_member_road.rs"]
//! mod road;` at its root.
#![allow(dead_code)]

use crate::road::{
    Delivery, Segment, chain_of, dead_letters, deliveries, read_json, repo, segment_before,
    write_json,
};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, SurfaceRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{MESSAGE_DEFAULT_TTL, Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
const DOC_FILE_ID: &str = "DOC-two-pages";
const DOC_FILE_PATH: &str = "documents/file_1.pdf";
const CAPTION: &str = "here are the two pages";
const NAME: &str = "two_pages.pdf";
/// What the stub summarizer answers: the first line is the one line.
const SUMMARY: &str = "Two pages of a test.\n\nPage one and page two.";
const EMBED_DIM: usize = 64;

/// Failure budget, generous under cargo's parallel load: the road from the
/// update to the assistants crosses a dozen cold `code` cells and several
/// store round trips, `pdftotext`, the summary and the embeddings. Not a
/// timing discriminator.
const ROAD_SECS: u64 = 120;

/// The name the connector is instantiated under in the member's `./channels`.
const CHANNEL: &str = "telegram";

/// Where the member stands in the test colony, and the nodes the road names.
pub const CHANNELS: &str = "/person/channels";
pub const SPACE: &str = "/person/file-space";
pub const ASSISTANTS: &str = "/person/assistants";

/// The routing decisions the turn still spends between its arrival at the
/// double and the door of a generation, which the measured chain cannot see.
/// The double stands where the member's CONTAINER `./assistants` stands; in a
/// real member the turn arrives at the container (1), the container forwards
/// it into the generation (2), and the generation's door edge (`. -> ./talky`,
/// `in_turn`) restores -- the door itself costs this segment nothing.
/// `segment_before` ends at the PARENT of the arrival, so neither of the two
/// is in its `used`; the last segment of the turn gets them added on its `end`
/// side (`hops` stays the count of logged deliveries it walked).
pub const TO_THE_DOOR: i64 = 2;

/// Which edges of the space restore in a run (see the file header).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum B2Variant {
    Shipped,
    NoRestores,
    DoorOnly,
}

/// What one S5 run leaves to read.
pub struct FileTurn {
    /// Every delivery the colony logged, in order.
    pub log: Vec<Delivery>,
    /// `(sender, target, error_code)` of every dead letter of the run.
    pub dead: Vec<(String, String, String)>,
    /// Every segment of the document turn between two seams, oldest first,
    /// named `S5 <opening seam> -> <closing seam>`: `root` (the connector's
    /// emission), `ingest` (the space's door, `DoorOnly`), `extract`, `write`,
    /// `turn` (the restoring edges out of `./ingest`), `door` (the
    /// generation's door, [`TO_THE_DOOR`] behind the arrival at the double).
    pub segments: Vec<(String, Segment)>,
    /// The ttl of every arrival at the double on `in_turn`.
    pub arrived: Vec<i64>,
    /// The document's chain died of its budget (a `ttl_expired` dead letter)
    /// and the turn never reached the double: no segment to measure, the road
    /// does not fit at all (review T, M-3). A turn that arrives and dies later
    /// (the double's answer on its way out) is measured, not `died`.
    pub died: bool,
    /// How often the document's leg ran through `./ingest -> ./extract`: one
    /// plus the races.
    pub extracts: usize,
}

impl FileTurn {
    pub fn expired(&self) -> Vec<(String, String, String)> {
        self.dead
            .iter()
            .filter(|(_, _, c)| c == "ttl_expired")
            .cloned()
            .collect()
    }
}

/// The templates this road boots, or `false` when the tree under test did not
/// ship them (the public export ships a subset of the library, GH #49).
pub fn shipped() -> bool {
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

// ══════════════════════════════════════════════════════════════ the upstream

/// Everything the colony talks to over HTTP, on one listener on 127.0.0.1,
/// told apart by path: the Bot API (`getUpdates` answers every update whose
/// id is at least `offset`, an empty answer waits a little first, the way a
/// long poll does), an OpenAI-compatible embeddings endpoint and a chat
/// endpoint that answers every request with [`SUMMARY`].
struct Upstream {
    base: String,
    /// `METHOD target (n bytes)` of every request, for failure messages.
    hits: Arc<Mutex<Vec<String>>>,
}

impl Upstream {
    fn start(pdf: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the upstream stub");
        let base = format!("http://{}", listener.local_addr().expect("a local address"));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log = hits.clone();
        let pdf = Arc::new(pdf);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let log = log.clone();
                let pdf = pdf.clone();
                // One thread per connection: a long poll that waits must never
                // hold up the download or a model call behind it.
                std::thread::spawn(move || serve_one(conn, &pdf, &log));
            }
        });
        Upstream { base, hits }
    }

    fn trace(&self) -> Vec<String> {
        self.hits.lock().expect("the hit log").clone()
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

fn document_update(size: usize) -> Value {
    json!({"update_id": DOC_UPDATE, "message": {
        "message_id": 5, "date": 1_790_000_000,
        "chat": {"id": CHAT_ID, "type": "private"},
        "from": {"id": USER_ID, "is_bot": false, "first_name": "Test"},
        "caption": CAPTION,
        "document": {"file_id": DOC_FILE_ID, "file_unique_id": "u-doc",
                     "file_name": NAME, "mime_type": "application/pdf",
                     "file_size": size}}})
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
    pdf: &[u8],
) -> (u16, &'static str, Vec<u8>, Duration) {
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
        let due: Vec<Value> = if DOC_UPDATE >= offset {
            vec![document_update(pdf.len())]
        } else {
            Vec::new()
        };
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
    if path == "/v1/embeddings" && method == "POST" {
        let req: Value = sj::from_slice(body).unwrap_or(Value::Null);
        let dim = req["dimensions"]
            .as_u64()
            .map(|d| d as usize)
            .unwrap_or(EMBED_DIM);
        let data: Vec<Value> = req["input"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, t)| {
                json!({"object": "embedding", "index": i,
                       "embedding": vector(t.as_str().unwrap_or(""), dim)})
            })
            .collect();
        return json_ok(json!({
            "object": "list", "data": data, "model": "stub-embed",
            "usage": {"prompt_tokens": 3, "total_tokens": 3}
        }));
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

fn serve_one(conn: TcpStream, pdf: &[u8], log: &Mutex<Vec<String>>) {
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
    log.lock()
        .expect("the hit log")
        .push(format!("{method} {target} ({} bytes)", body.len()));
    let (status, ctype, payload, wait) = answer(&method, &target, &body, pdf);
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
    write_json(&p, &cfg);
}

/// A cell that answers nothing: the holders of the member this road never
/// reaches.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's assistants, doubled: the RECEIVER of the turn. Every
/// `in_turn` is answered on `answer`, marked `saw_turn`, which the member's
/// default edge carries out to the sink this road waits on; everything else
/// is answered with nothing.
const ASSISTANT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
if str(hop.get("route") or "") != "in_turn":
    sys.stdout.write(json.dumps([]))
    sys.exit(0)
sys.stdout.write(json.dumps([{"header": {"route": "answer", "saw_turn": "1"},
                              "messages": [{"origin": "assistant", "type": "text", "text": "seen"}]}]))
"#;

/// A `code` double with a fixed script (the shape of gh907's doubles: an
/// answer is a JSON array, `emits` is left wide on purpose).
fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
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

/// The edges the instantiating mutations would draw into the member's graph:
/// the connector's pairing edges (telegram-connector README § *Wiring it*),
/// the turn up as `turn` with the chat promoted into context, a failure as
/// `error`, and the way back into the chat. The last one never fires here
/// (`channel_node` is not stamped, so the double's answer takes the member's
/// guarded default out to the sink), but the boot check wants a setter of
/// `context.chat_id` reachable backwards over an edge INTO the connector.
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
        json!({
            "from": "./channels", "to": format!("./channels/{CHANNEL}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'answer' && has(context.channel_node) \
                 && context.channel_node == '{CHANNEL}'")
        }),
    ]
}

/// The colony around the member: a drain for every lane the member emits (an
/// undrained lane is a dead letter).
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

/// Every `${VAR}` any config below `dir` names, so the test env file can give
/// each a stub value and no boot reaches for a real credential.
fn env_names(dir: &std::path::Path, out: &mut BTreeSet<String>) {
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

/// The member as gh907 lays it out (see the file header), every endpoint on
/// the stub at `base`.
fn build_tree(root: &std::path::Path, base: &str) {
    write_json(&root.join("main/config.json"), &main_config());
    copy_template(&repo("templates/member"), &root.join("main/person"));
    // `graph-space` joined the member with GH #945: its index hears the
    // space's `source_changed`, never this road's turn.
    for holder in ["access", "affinity", "memory-hive", "graph-space"] {
        write_json(
            &root.join(format!("main/person/{holder}/config.json")),
            &double(INERT, "Inert double for a holder this road never reaches."),
        );
    }
    write_json(
        &root.join("main/person/assistants/config.json"),
        &double(
            ASSISTANT,
            "Test double for the member's assistants: the receiver of the turn.",
        ),
    );

    // The real firewall: the turn has to pass it.
    let fw = root.join("main/person/firewall");
    let _ = std::fs::remove_dir_all(&fw);
    copy_template(&repo("templates/firewall"), &fw);

    // The member's `./file-space` as instantiation lays it out.
    assert_eq!(
        read_json(&root.join("main/person/file-space/config.json"))["cell"]["type"],
        "ref",
        "the member ships ./file-space as a ref to the template"
    );
    resolve_ref(root, "main/person/file-space", 0);
    set_params(
        root,
        "main/person/file-space/embed",
        &[
            ("endpoint", json!(format!("{base}/v1/embeddings"))),
            ("model", json!("stub-embed")),
            ("dim", json!(EMBED_DIM.to_string())),
            ("api_key", json!("stub-key")),
        ],
    );
    set_params(
        root,
        "main/person/file-space/summarizer",
        &[
            ("model", json!("stub-model")),
            ("base_url", json!(format!("{base}/v1"))),
            ("api_key", json!("stub-key")),
        ],
    );

    // The connector, as the channel's instantiation grows it.
    let conn_rel = format!("main/person/channels/{CHANNEL}");
    write_json(
        &root.join(format!("{conn_rel}/config.json")),
        &read_json(&repo("templates/telegram-connector/config.json")),
    );
    set_params(
        root,
        &conn_rel,
        &[
            ("bot_token", json!(BOT_TOKEN)),
            ("base_url", json!(base)),
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
    write_json(&cfg_path, &cfg);

    // Every `${VAR}` a shipped config still names resolves to a stub value;
    // no real credential, no real endpoint.
    let mut names = BTreeSet::new();
    env_names(&root.join("main"), &mut names);
    let mut env: BTreeMap<String, String> = names
        .into_iter()
        .map(|n| (n.clone(), format!("stub-{n}")))
        .collect();
    env.insert("TELEGRAM_BOT_TOKEN".into(), BOT_TOKEN.into());
    env.insert("OPENROUTER_API_KEY".into(), "stub-key".into());
    env.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{base}/v1/embeddings"),
    );
    let body: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the test env file");
}

// ══════════════════════════════════════════════════════════════ the variants

/// The four edges of the space's graph on the document road that can carry a
/// seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpaceEdge {
    /// `. -> ./ingest` on `in_ingest`: the space's door, a hive out-edge.
    Door,
    /// `./ingest -> ./extract` on `in_extract`.
    Extract,
    /// `./ingest -> ./write` on `in_write` (caller `ingest`). The same lane
    /// out of `./extract` is a plain edge.
    Write,
    /// `./ingest -> .` on `turn`: the released turn leaves the space.
    Turn,
}

impl SpaceEdge {
    const ALL: [SpaceEdge; 4] = [
        SpaceEdge::Door,
        SpaceEdge::Extract,
        SpaceEdge::Write,
        SpaceEdge::Turn,
    ];

    /// `from`, `to` and route of the edge as the space's graph declares it.
    fn declared(self) -> (&'static str, &'static str, &'static str) {
        match self {
            SpaceEdge::Door => (".", "./ingest", "in_ingest"),
            SpaceEdge::Extract => ("./ingest", "./extract", "in_extract"),
            SpaceEdge::Write => ("./ingest", "./write", "in_write"),
            SpaceEdge::Turn => ("./ingest", ".", "turn"),
        }
    }

    /// The seam's word in a segment's name.
    fn label(self) -> &'static str {
        match self {
            SpaceEdge::Door => "ingest",
            SpaceEdge::Extract => "extract",
            SpaceEdge::Write => "write",
            SpaceEdge::Turn => "turn",
        }
    }

    /// Whether `d` is the delivery through this edge. The message log names
    /// the sender of the hop: the hive itself for a hive out-edge (`. ->
    /// ./ingest`, the transit), the emitting cell for a cell's edge; `.` as a
    /// target is the hive's own path.
    fn carried(self, d: &Delivery) -> bool {
        let (from, to, route) = self.declared();
        let at = |rel: &str| match rel {
            "." => SPACE.to_string(),
            _ => format!("{SPACE}/{}", rel.strip_prefix("./").unwrap_or(rel)),
        };
        d.from == at(from) && d.to == at(to) && d.route == route
    }
}

/// The index of `e` in the space's edge list: exactly one edge with its
/// `from` and `to`, conditioned on its route. Anything else means the shipped
/// graph changed shape under this road.
fn edge_index(edges: &[Value], e: SpaceEdge) -> usize {
    let (from, to, route) = e.declared();
    let hits: Vec<usize> = edges
        .iter()
        .enumerate()
        .filter(|(_, x)| x["from"].as_str() == Some(from) && x["to"].as_str() == Some(to))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "the file space declares exactly one edge {from} -> {to}: {hits:?}"
    );
    let condition = edges[hits[0]]["condition"].as_str().unwrap_or_default();
    assert!(
        condition.contains(&format!("hop.route == '{route}'")),
        "the file space's edge {from} -> {to} carries `{route}`: {condition}"
    );
    hits[0]
}

fn restores(edge: &Value) -> bool {
    edge["modifier"]["restore_ttl"].as_bool() == Some(true)
}

/// Rewrite the COPY of the space's graph for `variant` and return the edges
/// that restore in it, read back off the rewritten file -- the seams of the
/// run are those of the tree that boots.
fn apply_variant(root: &std::path::Path, variant: B2Variant) -> Vec<SpaceEdge> {
    let p = root.join("main/person/file-space/config.json");
    let mut cfg = read_json(&p);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the file space ships a graph");
    for e in SpaceEdge::ALL {
        let i = edge_index(edges, e);
        let edge = edges[i].as_object_mut().expect("an edge is an object");
        if variant != B2Variant::Shipped {
            if let Some(m) = edge.get_mut("modifier").and_then(Value::as_object_mut) {
                m.remove("restore_ttl");
            }
            if edge
                .get("modifier")
                .and_then(Value::as_object)
                .is_some_and(|m| m.is_empty())
            {
                edge.remove("modifier");
            }
        }
        if variant == B2Variant::DoorOnly && e == SpaceEdge::Door {
            edge.entry("modifier").or_insert_with(|| json!({}))["restore_ttl"] = json!(true);
        }
    }
    write_json(&p, &cfg);
    let edges = read_json(&p)["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    SpaceEdge::ALL
        .into_iter()
        .filter(|e| restores(&edges[edge_index(&edges, *e)]))
        .collect()
}

/// The line of the space's `./write` that finds a path taken (`born0`, the
/// only `path_taken` of a `create`).
const TAKEN_LINE: &str = "    taken = [r for r in rows(lg, \"paths\") if not r.get(\"tomb\")]\n";

/// The candidate names of the document `./ingest` tries, in order (its
/// `with_suffix`): the name itself, then `stem (n).ext`.
fn candidate(n: usize) -> String {
    if n <= 1 {
        return NAME.to_string();
    }
    let (stem, ext) = NAME
        .rsplit_once('.')
        .expect("the fixture name has an extension");
    format!("{stem} ({n}).{ext}")
}

/// Race the document's first `raced` candidate paths in the COPY of the
/// space's `./write`: its path check finds each of them taken by a file
/// `fh-raced`, which is what a writer outside `./ingest` leaves between the
/// search and the `create`. The template is never touched.
fn apply_race(root: &std::path::Path, raced: usize) {
    assert!(
        raced < 3,
        "./ingest tries a document at most three times (TRIES); {raced} races leave none"
    );
    if raced == 0 {
        return;
    }
    let p = root.join("main/person/file-space/write/config.json");
    let mut cfg = read_json(&p);
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("./write is a code cell")
        .to_string();
    assert_eq!(
        script.matches(TAKEN_LINE).count(),
        1,
        "./write checks a create's path in exactly one line; the race fixture needs it"
    );
    let names: Vec<String> = (1..=raced).map(candidate).collect();
    let raced_line = format!(
        "{}    taken = taken or ([{{\"file\": \"fh-raced\", \"tomb\": \"\"}}] \
         if st[\"path\"].rpartition(\"/\")[2] in {} else [])\n",
        TAKEN_LINE,
        meclaw_core::serde_json::to_string(&names).expect("names serialise")
    );
    cfg["params"]["script_inline"] = json!(script.replace(TAKEN_LINE, &raced_line));
    write_json(&p, &cfg);
}

// ══════════════════════════════════════════════════════════════ the run

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
    // document there.
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

/// How a wait for the turn ended.
enum Reached {
    /// The double answered the turn (`saw_turn`).
    Turn,
    /// A message of the run died of its budget first.
    Died,
    /// Neither within [`ROAD_SECS`]: the hops of whatever reached the sink.
    Lost(Vec<String>),
}

/// Whether the colony under `root` has dead-lettered a message for its
/// budget yet. Read while the colony runs; a read that fails (the writer
/// holds the file) says no and is asked again.
fn died_of_its_budget(root: &std::path::Path) -> bool {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return false;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM dead_letters WHERE error_code = 'ttl_expired'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .is_ok_and(|n| n > 0)
}

/// After the first death of the run, how long the wait still listens for
/// the turn: a message off the document's chain may die first. Not a timing
/// discriminator -- whether the turn arrived is read off the log afterwards.
const AFTER_A_DEATH: Duration = Duration::from_secs(5);

/// Wait for the double's answer to the turn, and stop early once a message
/// of the run died of its budget (plus [`AFTER_A_DEATH`]) -- a dead chain
/// sends nothing on, so the wait would otherwise sit out the whole
/// [`ROAD_SECS`].
async fn until_turn(root: &std::path::Path, rx: &mut mpsc::Receiver<Message>) -> Reached {
    let mut deadline = Instant::now() + Duration::from_secs(ROAD_SECS);
    let mut dead = false;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return if dead {
                Reached::Died
            } else {
                Reached::Lost(seen)
            };
        }
        match tokio::time::timeout(left.min(Duration::from_millis(500)), rx.recv()).await {
            Ok(Some(m)) => {
                if m.headers.hop.get("saw_turn").and_then(Value::as_str) == Some("1") {
                    return Reached::Turn;
                }
                seen.push(format!("{:?}", m.headers.hop));
            }
            Ok(None) => return Reached::Lost(seen),
            Err(_) if !dead && died_of_its_budget(root) => {
                dead = true;
                deadline = deadline.min(Instant::now() + AFTER_A_DEATH);
            }
            Err(_) => {}
        }
    }
}

/// Every dead letter, then the last hops of the message log -- where the
/// turn went instead of the double.
fn account(log: &[Delivery], dead: &[(String, String, String)]) -> String {
    let mut out: Vec<String> = dead
        .iter()
        .map(|(s, t, c)| format!("DEAD {s} -> {t}: {c}"))
        .collect();
    let skip = log.len().saturating_sub(60);
    out.extend(log.iter().skip(skip).map(Delivery::say));
    out.join("\n")
}

// ══════════════════════════════════════════════════════════════ the measurement

/// The deliveries of `d`'s parent chain, `d` first.
fn chain<'a>(log: &'a [Delivery], d: &'a Delivery) -> Vec<&'a Delivery> {
    let by_id: HashMap<&str, &Delivery> = log.iter().map(|x| (x.id.as_str(), x)).collect();
    let mut out = vec![d];
    let mut at = d;
    while let Some(p) = at.parent.as_deref().and_then(|p| by_id.get(p)).copied() {
        out.push(p);
        at = p;
        assert!(
            out.len() <= 4 * MESSAGE_DEFAULT_TTL as usize,
            "the parent chain of {} does not end",
            d.say()
        );
    }
    out
}

/// Every segment of the turn that arrived with `arrival`, oldest first (see
/// [`FileTurn::segments`]). The seams are read off the arrival's parent
/// chain: its oldest delivery (a source, the connector's emission) and every
/// delivery through an edge of `restoring`. Every plain hop costs one; a
/// delivery on the chain whose ttl is above its parent's restored the budget,
/// and one that is no seam of the variant means the tree restores somewhere
/// this road does not count and the numbers would be wrong -- it fails the
/// run.
fn measure(
    log: &[Delivery],
    arrival: &Delivery,
    restoring: &[SpaceEdge],
) -> Vec<(String, Segment)> {
    let links = chain(log, arrival);
    let say_chain = || chain_of(log, arrival, 4 * MESSAGE_DEFAULT_TTL as usize).join("\n");
    let oldest = links
        .last()
        .copied()
        .expect("a chain holds its own delivery");
    assert!(
        oldest.parent.is_none() && oldest.from == "@external" && oldest.to == CHANNELS,
        "the turn's chain starts at the connector's emission into {CHANNELS} (a source, \
         logged from @external); it starts at {}:\n{}",
        oldest.say(),
        say_chain()
    );
    let seam = |d: &Delivery| -> Option<&'static str> {
        if d.parent.is_none() {
            return Some("root");
        }
        restoring.iter().find(|e| e.carried(d)).map(|e| e.label())
    };
    // The seams on the chain, newest first (the arrival itself is none).
    let mut seams: Vec<(&Delivery, &'static str)> = Vec::new();
    for (i, &d) in links.iter().enumerate() {
        let word = if i == 0 { None } else { seam(d) };
        if let Some(p) = links.get(i + 1)
            && d.ttl > p.ttl
            && word.is_none()
        {
            panic!(
                "the turn's budget rises at {} (after {}), which is no seam of this \
                 variant (restoring: {restoring:?}):\n{}",
                d.say(),
                p.say(),
                say_chain()
            );
        }
        if let Some(w) = word {
            seams.push((d, w));
        }
    }
    let (open, open_word) = *seams
        .first()
        .unwrap_or_else(|| panic!("the turn's chain finds no seam:\n{}", say_chain()));
    let mut last = segment_before(log, arrival, |d| d.id == open.id).unwrap_or_else(|| {
        panic!(
            "the parent chain of the arrival reaches its seam {}:\n{}",
            open.say(),
            say_chain()
        )
    });
    last.end -= TO_THE_DOOR;
    let mut out = vec![(format!("S5 {open_word} -> door"), last)];
    for w in seams.windows(2) {
        let ((close, close_word), (open, open_word)) = (w[0], w[1]);
        let seg = segment_before(log, close, |d| d.id == open.id).unwrap_or_else(|| {
            panic!(
                "the parent chain of the seam {} reaches the seam {}:\n{}",
                close.say(),
                open.say(),
                say_chain()
            )
        });
        out.push((format!("S5 {open_word} -> {close_word}"), seg));
    }
    out.reverse();
    // A seam crossed twice (a retry) names its segments apart.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (name, _) in &mut out {
        let n = seen.entry(name.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            *name = format!("{name} #{n}");
        }
    }
    out
}

/// S5 -- one document turn, from the connector's emission to the member's
/// assistants, on the file space `variant` lays out, with the document's
/// first `raced` paths taken under it. Boots the road, waits for the turn at
/// the double, reads the log after shutdown, and measures every segment of
/// the turn's parent chain. It does not judge the segments against the
/// reserve (the including test does). A chain that dies of its budget comes
/// back as `died` with no segment; it fails loudly when the turn neither
/// arrives nor dies, or its chain finds no seam.
pub async fn s5_file_turn(variant: B2Variant, raced: usize) -> FileTurn {
    assert!(
        std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_ok(),
        "`pdftotext` (poppler-utils) is required: the file space's ./extract runs it"
    );
    let up = Upstream::start(pdf_bytes());
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(td.path(), &up.base);
    let restoring = apply_variant(td.path(), variant);
    apply_race(td.path(), raced);
    let (h, mut rx) = boot(&td).await;
    let reached = until_turn(td.path(), &mut rx).await;
    h.shutdown().await;
    let log = deliveries(td.path());
    let dead = dead_letters(td.path());
    let extracts = log.iter().filter(|d| SpaceEdge::Extract.carried(d)).count();
    let landed = log
        .iter()
        .any(|d| d.to == ASSISTANTS && d.route == "in_turn");
    if matches!(reached, Reached::Died) && !landed {
        return FileTurn {
            log,
            dead,
            segments: Vec::new(),
            arrived: Vec::new(),
            died: true,
            extracts,
        };
    }
    if let (Reached::Lost(seen), false) = (reached, landed) {
        let hits = up.trace();
        let tail = hits.len().saturating_sub(40);
        panic!(
            "S5 {variant:?}: the document turn did not reach the double within {ROAD_SECS}s \
             (restoring: {restoring:?}).\nsink: {seen:#?}\ncolony:\n{}\nupstream: {:#?}",
            account(&log, &dead),
            &hits[tail..]
        );
    }
    let arrived: Vec<i64> = log
        .iter()
        .filter(|d| d.to == ASSISTANTS && d.route == "in_turn")
        .map(|d| d.ttl)
        .collect();
    let first = log
        .iter()
        .find(|d| d.to == ASSISTANTS && d.route == "in_turn")
        .unwrap_or_else(|| {
            panic!(
                "S5 {variant:?}: the double answered, but the log holds no arrival at \
                 {ASSISTANTS} on in_turn:\n{}",
                account(&log, &dead)
            )
        });
    if std::env::var("GH929_TRACE").is_ok() {
        for line in chain_of(&log, first, 4 * MESSAGE_DEFAULT_TTL as usize)
            .iter()
            .rev()
        {
            eprintln!("gh929 trace S5 {variant:?} {line}");
        }
    }
    let segments = measure(&log, first, &restoring);
    FileTurn {
        log,
        dead,
        segments,
        arrived,
        died: false,
        extracts,
    }
}
