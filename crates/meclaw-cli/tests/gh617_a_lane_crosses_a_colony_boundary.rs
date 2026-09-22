//! GH #617: a declared lane crosses a colony boundary, and both sides book it.
//!
//! Two real colonies run in this test process, each through `run_with_hooks`
//! on `127.0.0.1:0`; in front of the south colony's peer mount sits a reverse
//! proxy that writes the sender into `X-Meclaw-Peer`, the only place the mount
//! reads it from. `setup_subscriber` runs only in `entrypoint`, and the root
//! lease hangs off `--root`, so two TempDirs are two colonies. The file is
//! deliberately not named `*_e2e`: `scripts/gate_plan.py` subtracts
//! `binary(/e2e/)` as the scenario class, and a strand gate would then never
//! run the test that measures the change it is gating. Every test boots south
//! first, then the proxy, then north, because north's edge must name an
//! address that already answers.
//!
//! The fixtures are `tests/fixtures/peer-north` and `tests/fixtures/peer-south`:
//!
//! ```text
//!   (a turn over HTTP) -> north /writer -> north /friend --POST--> proxy
//!        --POST + X-Meclaw-Peer--> south /peer/ -> south /friend -> /sink
//!   north /friend -> north /receipts        south /friend -> south /receipts
//! ```
//!
//! Deterministic by construction: no provider, no wall clock. Every read is
//! filtered on the turn's `trace_id`, asks for `limit=1000` and refuses a
//! truncated scan, because a truncated scan reads as "nothing matched".

use meclaw_cli::{Cli, run_with_hooks};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The 30-second convention: a failure marker, never a budget.
const RECV: Duration = Duration::from_secs(30);

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies the committed fixture tree into a TempDir -- never boot in place
/// (`colony.db`/`cell.db` are created at runtime).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

/// Rewrites every `config.json` under `root` as text: the literal stands in
/// the fixture as a JSON string and comes out as one, so nothing is parsed.
fn rewrite_configs(root: &std::path::Path, pairs: &[(&str, &str)]) {
    for entry in std::fs::read_dir(root).expect("read_dir").flatten() {
        let path = entry.path();
        if entry.file_type().expect("file_type").is_dir() {
            rewrite_configs(&path, pairs);
        } else if path.file_name().is_some_and(|n| n == "config.json") {
            let mut text = std::fs::read_to_string(&path).expect("read config");
            for (from, to) in pairs {
                text = text.replace(from, to);
            }
            std::fs::write(&path, text).expect("write config");
        }
    }
}

fn cli_for(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind")),
        daemon: false,
        validate: false,
        validate_strict: false,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6688,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
        command: None,
    }
}

/// A booted fixture: its bind address, and the handles that stop it again.
struct Colony {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    _td: tempfile::TempDir,
}

impl Colony {
    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(Duration::from_secs(30), self.join).await;
    }
}

/// Boots one fixture tree in a TempDir, rewriting the literals a run fills in first.
async fn boot(fixture: &str, rewrite: &[(&str, &str)]) -> Colony {
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path(fixture), td.path());
    rewrite_configs(td.path(), rewrite);
    let cli = cli_for(td.path());
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = tokio::time::timeout(RECV, addr_rx)
        .await
        .expect("the colony must bind HTTP within 30s")
        .expect("addr hook");
    Colony {
        addr,
        shutdown,
        join,
        _td: td,
    }
}

/// The same three steps in every test: south, the proxy in front of it, north.
async fn pair() -> (Colony, SocketAddr, Colony) {
    let south = boot("peer-south", &[]).await;
    let fwd = peer_forwarder(south.addr, "north").await;
    let url = format!("http://{fwd}/peer/");
    let north = boot("peer-north", &[("__PEER_URL__", url.as_str())]).await;
    (south, fwd, north)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The `Content-Length` of a request head, case-insensitive; `0` without one.
fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .split("\r\n")
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// The head without the named headers: the request line stays, every kept
/// line gets its `\r\n` back.
fn strip_headers(head: &[u8], names: &[&str]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut out = Vec::new();
    for (i, line) in text.split("\r\n").enumerate() {
        if line.is_empty() {
            continue;
        }
        let drop = i > 0
            && line
                .split_once(':')
                .is_some_and(|(k, _)| names.iter().any(|n| k.trim().eq_ignore_ascii_case(n)));
        if !drop {
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

/// A reverse proxy in front of a peer mount: it authenticates (here: by
/// constant) and writes the identity as a header. The mount reads ONLY this
/// header -- R-26-18 -- so any version a client sent along is deleted first.
async fn peer_forwarder(upstream: SocketAddr, sender: &'static str) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut down, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let head = loop {
                    let mut b = [0u8; 4096];
                    match down.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&b[..n]),
                    }
                    if let Some(i) = find(&buf, b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let want = head + content_length(&buf[..head]);
                while buf.len() < want {
                    let mut b = [0u8; 4096];
                    match down.read(&mut b).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&b[..n]),
                    }
                }
                let mut out = strip_headers(&buf[..head - 2], &["x-meclaw-peer", "connection"]);
                out.extend_from_slice(
                    format!("X-Meclaw-Peer: {sender}\r\nConnection: close\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(&buf[head..]);
                let Ok(mut up) = tokio::net::TcpStream::connect(upstream).await else {
                    return;
                };
                if up.write_all(&out).await.is_err() {
                    return;
                }
                let mut back = Vec::new();
                let _ = up.read_to_end(&mut back).await;
                let _ = down.write_all(&back).await;
                let _ = down.shutdown().await;
            });
        }
    });
    addr
}

/// One row of `GET /colony/messages`, both compartments parsed out.
struct Row {
    hop: Value,
    body: Value,
    ttl: i64,
}

/// The page carries its rows under `messages`. `body_payload` is a STRING holding JSON, `headers_json` the raw string of
/// BOTH compartments; the hop is read the way `meclaw ask` reads it.
async fn rows(addr: &SocketAddr, trace: &str, prefix: &str) -> Vec<Row> {
    let url = format!(
        "http://{addr}/colony/messages?trace_id={trace}&to_path_prefix={prefix}&limit=1000"
    );
    let page: Value = reqwest::get(&url)
        .await
        .expect("GET")
        .json()
        .await
        .expect("json");
    assert_eq!(
        page["scan_truncated"],
        json!(false),
        "a truncated scan reads as 'nothing matched' -- {url}"
    );
    page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|e| {
            let h: Value =
                serde_json::from_str(e["headers_json"].as_str().unwrap_or("{}")).expect("headers");
            let body = e["body_payload"]
                .as_str()
                .map(|s| serde_json::from_str(s).expect("body"))
                .unwrap_or(Value::Null);
            Row {
                hop: h.get("hop").cloned().unwrap_or(json!({})),
                body,
                ttl: e["ttl"].as_i64().expect("ttl"),
            }
        })
        .collect()
}

/// Reads `rows` every 50 ms until at least `n` are there. The test is a client
/// outside the substrate, not a cell inside it, so polling is its to do.
async fn wait_for_rows(addr: &SocketAddr, trace: &str, prefix: &str, n: usize) -> Vec<Row> {
    let deadline = tokio::time::Instant::now() + RECV;
    loop {
        let r = rows(addr, trace, prefix).await;
        if r.len() >= n {
            return r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{n} row(s) under {prefix} within 30s -- saw {}",
            r.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The driver's POST: one turn at north's `/writer`, the lane in `hop.route`.
/// Fire-and-forget, so `202`; the returned `message_id` IS the turn's
/// `trace_id`, which every read filters on.
async fn drive(addr: &SocketAddr, route: &str, body: Value, ttl: u32) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/messages"))
        .json(&json!({"target": "/writer", "hop": {"route": route}, "body": body, "ttl": ttl}))
        .send()
        .await
        .expect("POST /messages");
    assert_eq!(
        resp.status().as_u16(),
        202,
        "a turn is accepted, never answered"
    );
    let v: Value = resp.json().await.expect("json");
    v["message_id"].as_str().expect("message_id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_lane_crosses_a_colony_boundary_and_leaves_a_receipt() {
    let (south, _fwd, north) = pair().await;
    let t = drive(
        &north.addr,
        "topic",
        json!({"messages": [], "topic": "gardening"}),
        64,
    )
    .await;
    let seen = wait_for_rows(&south.addr, &t, "/sink", 1).await;
    assert_eq!(
        seen.len(),
        1,
        "exactly one arrival, not one of several the window gave up"
    );
    assert_eq!(
        seen[0].body,
        json!({"messages": [], "topic": "gardening"}),
        "only the named field crossed, next to the turn list, projected turn by turn and empty"
    );
    assert_eq!(
        seen[0].hop["route"],
        json!("topic"),
        "the lane is the route it arrived on"
    );
    assert_eq!(
        seen[0].hop["peer"],
        json!("north"),
        "the sender came from the header, not the frame"
    );
    assert!(
        seen[0].hop.get("peer_url").is_none(),
        "an address never crosses"
    );
    let r = wait_for_rows(&north.addr, &t, "/receipts", 1).await;
    assert_eq!(
        r[0].hop["route"],
        json!("receipt"),
        "a receipt travels on its own route"
    );
    assert_eq!(
        r[0].hop["peer_event"],
        json!("crossed"),
        "the near side booked a crossing"
    );
    assert_eq!(
        r[0].hop["boundary"],
        json!("north"),
        "a receipt names the side that wrote it"
    );
    assert_eq!(
        r[0].hop["fields"],
        json!(["topic"]),
        "and what it let through"
    );
    assert_eq!(
        r[0].body["messages"],
        json!([]),
        "ADR-0025: a receipt never lifts a turn"
    );
    let sr = wait_for_rows(&south.addr, &t, "/receipts", 1).await;
    assert_eq!(
        sr.len(),
        1,
        "the far side booked exactly one receipt for the one crossing"
    );
    assert_eq!(
        sr[0].hop["peer_event"],
        json!("crossed"),
        "the far side booked the same verdict"
    );
    assert_eq!(sr[0].hop["boundary"], json!("south"), "in its own name");
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_the_far_side_does_not_accept_is_refused_on_both_sides() {
    let (south, _fwd, north) = pair().await;
    let t = drive(
        &north.addr,
        "gossip",
        json!({"messages": [], "topic": "gardening"}),
        64,
    )
    .await;
    let s = wait_for_rows(&south.addr, &t, "/receipts", 1).await;
    assert_eq!(
        s[0].hop["peer_event"],
        json!("refused"),
        "the far side booked a refusal"
    );
    assert_eq!(
        s[0].hop["error_code"],
        json!("lane_undeclared"),
        "the far side judged its own edge"
    );
    assert_eq!(s[0].hop["boundary"], json!("south"), "in its own name");
    let n = wait_for_rows(&north.addr, &t, "/receipts", 1).await;
    assert_eq!(
        n[0].hop["error_code"],
        json!("peer_refused"),
        "the near side heard the verdict"
    );
    assert!(
        n[0].body["messages"][0]["text"]
            .as_str()
            .expect("detail")
            .contains("south"),
        "the detail names the boundary that refused and the code it gave"
    );
    assert!(
        rows(&south.addr, &t, "/sink").await.is_empty(),
        "nothing arrived -- asserted NEXT TO the two receipts above, never alone"
    );
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_field_outside_the_allow_list_is_refused_rather_than_stripped() {
    let (south, _fwd, north) = pair().await;
    let t = drive(
        &north.addr,
        "topic",
        json!({"messages": [], "topic": "gardening", "who": "the owner"}),
        64,
    )
    .await;
    let at = wait_for_rows(&north.addr, &t, "/friend", 1).await;
    assert_eq!(
        at[0].body["who"],
        json!("the owner"),
        "the unnamed field did reach the peer cell, so the refusal below is its verdict"
    );
    let n = wait_for_rows(&north.addr, &t, "/receipts", 1).await;
    assert_eq!(
        n[0].hop["error_code"],
        json!("lane_field_denied"),
        "a field the lane does not name is refused"
    );
    let detail = n[0].body["messages"][0]["text"].as_str().expect("detail");
    assert!(detail.contains("who"), "the detail names the FIELD");
    assert!(!detail.contains("the owner"), "and never its value");
    assert!(
        rows(&south.addr, &t, "/sink").await.is_empty(),
        "no partially projected body arrived"
    );
    // The refusal falls on north, so south never saw this trace at all: an
    // `all()` over its rows would hold on an empty list and prove nothing.
    assert!(
        rows(&south.addr, &t, "/").await.is_empty(),
        "and the far side never saw this trace at all, under any path"
    );
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trace_survives_the_boundary_and_the_ttl_falls_by_one() {
    let (south, _fwd, north) = pair().await;
    let t = drive(
        &north.addr,
        "topic",
        json!({"messages": [], "topic": "gardening"}),
        64,
    )
    .await;
    // North's input at the peer cell against south's input at the sink: both
    // rows are deliveries, so both carry the post-decrement `ttl`.
    let out = wait_for_rows(&north.addr, &t, "/friend", 1).await;
    let inn = wait_for_rows(&south.addr, &t, "/sink", 1).await;
    assert_eq!(
        inn[0].ttl,
        out[0].ttl - 1,
        "like against like: `ttl` is post-decrement at every delivery, so comparing unlike \
         hops measures the decrement twice or not at all"
    );
    // Both reads above filter on the same trace_id, so their non-empty answers
    // ARE the sentence: one conversation, one trace, two message logs.
    // Three hops from the driver: the delivery to /writer, the delivery to
    // /friend, and the crossing, which leaves the frame with zero. A turn at
    // /friend with no budget at all could not even book its receipt: the
    // colony dead-letters every emission whose input had no hops left.
    let t0 = drive(
        &north.addr,
        "topic",
        json!({"messages": [], "topic": "gardening"}),
        3,
    )
    .await;
    let s0 = wait_for_rows(&south.addr, &t0, "/receipts", 1).await;
    assert_eq!(
        s0[0].hop["error_code"],
        json!("ttl_exhausted"),
        "a crossing at zero is refused, not made"
    );
    let z = wait_for_rows(&north.addr, &t0, "/receipts", 1).await;
    assert_eq!(
        z[0].hop["error_code"],
        json!("peer_refused"),
        "the near side heard the verdict"
    );
    assert!(
        z[0].body["messages"][0]["text"]
            .as_str()
            .expect("detail")
            .contains("ttl_exhausted"),
        "and the detail names the code the far side gave"
    );
    assert!(
        rows(&south.addr, &t0, "/sink").await.is_empty(),
        "and nothing arrived"
    );
    north.stop().await;
    south.stop().await;
}

/// A frame on lane `topic`, with a fresh trace, merged with `extra`. The body
/// is the one a meclaw sender projects: the turn list always rides along, and
/// without it the arrival would be no valid body at all.
fn frame_with(extra: Value) -> Value {
    frame_with_trace_and(extra).0
}

/// A frame on lane `topic` and the trace it carries.
fn frame_with_trace() -> (Value, String) {
    frame_with_trace_and(json!({}))
}

fn frame_with_trace_and(extra: Value) -> (Value, String) {
    let trace = uuid::Uuid::now_v7().to_string();
    let mut f = json!({
        "v": 1, "type": "message", "lane": "topic", "trace_id": trace, "ttl": 8,
        "context": {}, "body": {"messages": [], "topic": "gardening"}
    });
    if let (Some(obj), Some(more)) = (f.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    (f, trace)
}

/// One frame straight at a peer mount; the answer IS the receipt.
async fn post_frame(addr: &SocketAddr, frame: Value, headers: &[(&str, &str)]) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/peer/"))
        .header("Content-Type", "application/json")
        .body(frame.to_string());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.expect("POST /peer/");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a verdict is an answer, refused or not"
    );
    resp.json().await.expect("receipt json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sender_field_in_the_frame_is_refused() {
    let (south, fwd, north) = pair().await;
    // (a) A sender field in the frame: the identity is never what the sender claims.
    let a = post_frame(&fwd, frame_with(json!({"sender": "north"})), &[]).await;
    assert_eq!(a["result"], json!("refused"), "the frame did not cross");
    assert_eq!(
        a["error_code"],
        json!("invalid_frame"),
        "a frame does not get to say who it is"
    );
    // (b) A header sent along is deleted by the proxy, not passed on.
    let (f, t) = frame_with_trace();
    let b = post_frame(&fwd, f, &[("X-Meclaw-Peer", "impostor")]).await;
    assert_eq!(
        b["result"],
        json!("crossed"),
        "the same frame without the field crosses"
    );
    assert_eq!(
        wait_for_rows(&south.addr, &t, "/sink", 1).await[0].hop["peer"],
        json!("north"),
        "the mount saw the name the proxy checked, and no other"
    );
    // (c) Without the proxy in front nobody gets in: fail-closed.
    let c = post_frame(&south.addr, frame_with(json!({})), &[]).await;
    assert_eq!(
        c["result"],
        json!("refused"),
        "a bare mount crosses nothing"
    );
    assert_eq!(
        c["error_code"],
        json!("invalid_frame"),
        "no authenticated sender header on this mount"
    );
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_whose_body_has_no_central_slot_is_refused_not_lost() {
    let (south, fwd, north) = pair().await;
    // (d) A foreign sender that leaves out the turn list: the lane lets
    // `topic` through, but `{"topic": ...}` alone is no UBF body, and the
    // arrival could reach nobody. Accepting it would answer `crossed` for a
    // message that is then lost.
    let (mut f, t) = frame_with_trace();
    f["body"] = json!({"topic": "gardening"});
    let d = post_frame(&fwd, f, &[]).await;
    assert_eq!(
        d["result"],
        json!("refused"),
        "a body that could not be delivered is not accepted"
    );
    assert_eq!(
        d["error_code"],
        json!("invalid_frame"),
        "the frame is what is wrong, not the lane"
    );
    let detail = d["detail"].as_str().expect("detail");
    assert!(
        detail.contains("messages"),
        "the detail names the central slot that is missing -- {detail}"
    );
    assert!(!detail.contains("gardening"), "and never a value");
    let s = wait_for_rows(&south.addr, &t, "/receipts", 1).await;
    assert_eq!(
        s[0].hop["peer_event"],
        json!("refused"),
        "the far side booked the refusal itself, on the frame's trace"
    );
    assert_eq!(
        s[0].hop["error_code"],
        json!("invalid_frame"),
        "under the same code it answered"
    );
    assert!(
        rows(&south.addr, &t, "/sink").await.is_empty(),
        "nothing arrived -- asserted NEXT TO the two receipts above, never alone"
    );
    north.stop().await;
    south.stop().await;
}
