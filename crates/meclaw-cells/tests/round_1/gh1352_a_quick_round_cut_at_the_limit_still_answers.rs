//! GH #1352 -- a quick round cut at its `tool_call_limit` still answers the
//! person.
//!
//! Measured in a lab run of the owner's product: the quick voice of a
//! `simple` turn streamed a 17th tool call over the dispatcher's budget of 16;
//! the `llm` cell stopped at the limit (GH #1350) and answered with
//! `provider_error` (`meta.error.kind` `tool_call_limit`). talky sent that to
//! its error drain and nothing else happened: the session closed without one
//! word to the person.
//!
//! This file boots the SHIPPED talky with `./quick` at a local stream stub
//! that sends 20 tool calls, `./brain` at a local stub that answers in
//! words, and every other `llm` cell at a closed port. The curator takes the
//! cut round to the brain (one `escalate` mark, reason `no_answer`), and the
//! brain's words leave talky on `answer` -- measured at the receiver, the
//! parent's sink.
//!
//! Free of a real provider and of the network by construction. Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DeadLetter, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, deadline, override_params_on_disk, wait};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// An endpoint nothing listens on: a call there fails at once and costs nothing.
const CLOSED_PORT: &str = "http://127.0.0.1:9/v1";
const STUB_MODEL: &str = "stub-model";
const CHANNEL: &str = "chat:1352";
const AUDIENCE: &str = r#"["member:a","member:e"]"#;
/// The brain's words: what the person must hear.
const BRAIN_TEXT: &str = "Of course -- what is your daughter's name?";
/// More calls than the dispatcher's budget (16), as the lab run's quick voice
/// wrote them.
const RUNAWAY_CALLS: usize = 20;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The shipped template, copied the way instantiation lays it out: every
/// `config.json` and the seeds beside them, a `cell.type: "ref"` directory
/// replaced by the referenced template with the marker's `override_params`
/// applied (GH #140, GH #277).
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

/// Every `config.json` below `dir`, seeds excluded.
fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every timer of the tree out of the run's way.
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x1352_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// `talky/quick` at the stream stub, `talky/brain` at the words stub, every
/// other `llm` cell at a closed port. The shipped params stay otherwise --
/// `./quick`'s `tool_call_limit` among them.
fn point_llms(main: &std::path::Path, quick_url: &str, brain_url: &str) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(main)
            .expect("under main")
            .to_string_lossy()
            .replace('\\', "/");
        let url = match rel.as_str() {
            "talky/quick" => quick_url,
            "talky/brain" => brain_url,
            _ => CLOSED_PORT,
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(STUB_MODEL);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// Every `${VAR}` the tree names bound to a dummy, so nothing reads the host's.
fn write_env(root: &std::path::Path, main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
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
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes the talky template declares at its own path.
fn rim_emits() -> Vec<String> {
    let cfg = read_json(&repo("templates/talky/config.json"));
    cfg["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The parent: the answer to the sink, the error drain to its own capture,
/// every other talky lane to the park.
fn parent_edges() -> Vec<Value> {
    let mut edges = vec![];
    for lane in rim_emits() {
        let to = match lane.as_str() {
            "answer" => "/sink",
            "error" => "/drain",
            _ => "/park",
        };
        edges.push(json!({"from": "./talky", "to": to,
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    edges
}

fn build(td: &tempfile::TempDir, quick_url: &str, brain_url: &str) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/talky"), &main.join("talky"), 0);
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": parent_edges()}}}),
    );
    quiet_timers(&main);
    point_llms(&main, quick_url, brain_url);
    // The decider sits at the closed port, so the route cell stamps its
    // fallback class at once -- `simple`, the class the curator sends to
    // `./quick`, with no tool needed (the lab turn's shape). The bound is a
    // hang mark, never reached.
    override_params_on_disk(
        &main.join("talky/route"),
        &json!({"fallback_class": "simple"}),
    );
    override_params_on_disk(
        &main.join("talky/collector/assemble"),
        &json!({"route_wait_ms": 600000}),
    );
    write_env(root, &main);
}

/// One SSE frame of a chat-completions stream.
fn frame(chunk: &Value) -> String {
    format!("data: {chunk}\n\n")
}

/// A sentence and `n` tool calls, each split over two chunks.
fn runaway_frames(n: usize) -> Vec<String> {
    let mut out = vec![frame(&json!({
        "id": "chatcmpl-q", "object": "chat.completion.chunk", "model": STUB_MODEL,
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": "One moment."}}]
    }))];
    for i in 0..n {
        out.push(frame(&json!({
            "id": "chatcmpl-q", "object": "chat.completion.chunk", "model": STUB_MODEL,
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": i, "id": format!("call_{i}"), "type": "function",
                "function": {"name": "needs", "arguments": ""}
            }]}}]
        })));
        out.push(frame(&json!({
            "id": "chatcmpl-q", "object": "chat.completion.chunk", "model": STUB_MODEL,
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": i, "function": {"arguments": format!("{{\"n\": {i}}}")}
            }]}}]
        })));
    }
    out.push("data: [DONE]\n\n".to_string());
    out
}

/// A provider on a raw socket that answers every request with the event
/// stream of `frames` and closes. Hands back the base url and a receiver of
/// one unit per request it served.
async fn stream_provider(frames: Vec<String>) -> (String, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = mpsc::unbounded_channel::<()>();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let frames = frames.clone();
            let seen = seen_tx.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, len) = loop {
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    match sock.read(&mut tmp).await {
                        Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]),
                        _ => return,
                    }
                }
                let _ = seen.send(());
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                            connection: close\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for f in frames {
                    if sock.write_all(f.as_bytes()).await.is_err() {
                        return;
                    }
                }
                let _ = sock.flush().await;
            });
        }
    });
    (format!("http://{addr}/v1"), seen_rx)
}

struct Ports {
    answer: mpsc::Receiver<Message>,
    drain: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    _park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut rx = Vec::new();
    for path in ["/sink", "/drain", "/park"] {
        let (tx, r) = mpsc::channel::<Message>(256);
        h.spawn(Path::new(path), move || CaptureCell::new(tx.clone()))
            .await;
        rx.push(r);
    }
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped talky must boot");
    let park = rx.pop().expect("park");
    let drain = rx.pop().expect("drain");
    let answer = rx.pop().expect("sink");
    (
        h,
        Ports {
            answer,
            drain,
            _park: park,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn talky_turn() -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "Please call my daughter."}]})))
        .ttl(400)
        .build()
}

/// Every dead letter the colony recorded, taken through the colony.
async fn dead_letters(h: &ColonyHandle) -> Vec<(String, String, String)> {
    deadline::hang(
        "the colony hands over its dead letters",
        h.drain_dead_letters(),
    )
    .await
    .into_iter()
    .map(|d: DeadLetter| {
        (
            d.reason.as_code().to_string(),
            d.resolved_target.as_str().to_string(),
            d.detail().unwrap_or_default().to_string(),
        )
    })
    .collect()
}

fn shipped() -> bool {
    [
        "talky",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => meclaw_core::serde_json::to_string(&v["messages"]).unwrap_or_default(),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quick_round_cut_at_its_tool_call_limit_answers_the_person() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (quick_url, mut quick_seen) = stream_provider(runaway_frames(RUNAWAY_CALLS)).await;
    let brain = MockOpenAI::start(vec![canned_chat_completion(BRAIN_TEXT, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &quick_url, &brain.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(talky_turn()).await;
    // The quick voice was asked: the round is the lab turn's shape.
    deadline::hang("the quick voice is asked", quick_seen.recv())
        .await
        .expect("the quick stub serves the quick voice");
    let answer = wait::until_some_with(
        "talky: an answer reaches the person after the quick round was cut",
        || {
            let got = ports.answer.try_recv().ok();
            async move { got }
        },
        || {
            let drained = ports.drain.try_recv().ok().map(|m| text_of(&m));
            let hr = &h;
            async move {
                format!(
                    "drained errors: {drained:?}; dead letters so far: {:#?}",
                    dead_letters(hr).await
                )
            }
        },
    )
    .await;
    h.quiesce().await;
    let brain_reqs = brain.recorded_requests().await;
    h.shutdown().await;

    let said = text_of(&answer);
    assert!(
        said.contains(BRAIN_TEXT),
        "the brain's words reach the person: {said}"
    );
    assert_eq!(
        brain_reqs.len(),
        1,
        "the cut round went to the brain exactly once"
    );
    let window = meclaw_core::serde_json::to_string(brain_reqs[0].messages().expect("wire"))
        .unwrap_or_default();
    assert!(
        window.contains("call my daughter"),
        "the brain sees the person's turn: {window}"
    );
}
