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
        env_report: false,
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
    let origin = origin_of(&url);
    let north = boot(
        "peer-north",
        &[
            ("__PEER_URL__", url.as_str()),
            ("__PEER_ORIGIN__", origin.as_str()),
        ],
    )
    .await;
    (south, fwd, north)
}

/// GH #840: the origin of a peer URL, the one entry of north's
/// `params.egress` (`__PEER_ORIGIN__` in the fixture).
fn origin_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .expect("a peer URL")
        .origin()
        .ascii_serialization()
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
    post_frame_from(None, addr, frame, headers).await
}

/// [`post_frame`] from a chosen source address (GH #833). Linux routes all of
/// `127.0.0.0/8` over `lo`, so `127.0.0.2` is a second host on the same box —
/// the cheapest real "client that is not the proxy" there is (OR-AG-21).
async fn post_frame_from(
    source: Option<std::net::IpAddr>,
    addr: &SocketAddr,
    frame: Value,
    headers: &[(&str, &str)],
) -> Value {
    let client = reqwest::Client::builder()
        .local_address(source)
        .build()
        .expect("a client");
    let mut req = client
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

/// The direct client of the GH #833 pin: a second loopback address, never the
/// forwarder's `127.0.0.1`.
const DIRECT: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 2));

/// GH #833, A-f(2): a client that reaches the colony's port directly, past the
/// proxy, and writes the sender header itself. With the default list (loopback
/// only, R-AG-1) the header counts from `127.0.0.2` too — the pin is what the
/// SOURCE decides, so the south fixture lists the forwarder's address and the
/// direct client's is outside: refused as `invalid_frame`, nothing arrives,
/// and the forwarder on `127.0.0.1` still crosses. A second south that trusts
/// only `127.0.0.2` turns both outcomes round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh833_a_forged_sender_header_from_a_direct_client_is_refused_while_the_forwarder_crosses()
{
    const HDR: &str = r#""identity_header": "X-Meclaw-Peer","#;
    let trusting = |list: &str| format!(r#"{HDR} "trusted_proxies": {list},"#);

    // (1) South believes the forwarder's address and no other.
    let only_fwd = trusting(r#"["127.0.0.1/32"]"#);
    let south = boot("peer-south", &[(HDR, only_fwd.as_str())]).await;
    let fwd = peer_forwarder(south.addr, "north").await;
    let (f, forged) = frame_with_trace();
    let r = post_frame_from(Some(DIRECT), &south.addr, f, &[("X-Meclaw-Peer", "north")]).await;
    assert_eq!(
        r["result"],
        json!("refused"),
        "a forged sender does not cross"
    );
    assert_eq!(
        r["error_code"],
        json!("invalid_frame"),
        "no new code (OR-AG-5)"
    );
    let detail = r["detail"].as_str().expect("detail");
    assert!(
        detail.contains("trusted_proxies"),
        "the receipt names the reason -- {detail}"
    );
    let (f, honest) = frame_with_trace();
    let c = post_frame(&fwd, f, &[]).await;
    assert_eq!(c["result"], json!("crossed"), "the forwarder still crosses");
    assert_eq!(
        wait_for_rows(&south.addr, &honest, "/sink", 1).await[0].hop["peer"],
        json!("north")
    );
    assert!(
        rows(&south.addr, &forged, "/sink").await.is_empty(),
        "0 arrivals from the direct client -- read NEXT TO the forwarder's arrival"
    );
    south.stop().await;

    // (2) The same two requests against a south that trusts only 127.0.0.2.
    let only_direct = trusting(r#"["127.0.0.2/32"]"#);
    let south = boot("peer-south", &[(HDR, only_direct.as_str())]).await;
    let fwd = peer_forwarder(south.addr, "north").await;
    let (f, direct) = frame_with_trace();
    let r = post_frame_from(Some(DIRECT), &south.addr, f, &[("X-Meclaw-Peer", "north")]).await;
    assert_eq!(
        r["result"],
        json!("crossed"),
        "a listed source is believed -- {r}"
    );
    assert_eq!(
        wait_for_rows(&south.addr, &direct, "/sink", 1).await[0].hop["peer"],
        json!("north")
    );
    let (f, _) = frame_with_trace();
    let c = post_frame(&fwd, f, &[]).await;
    assert_eq!(
        (c["result"].clone(), c["error_code"].clone()),
        (json!("refused"), json!("invalid_frame")),
        "and the forwarder, now unlisted, is not -- {c}"
    );
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

// ---------------------------------------------------------------------------
// GH #828: the outgoing POST carries a credential.
//
// North's peer cell gains `params.auth` in the TempDir copy, its secrets come
// from a `.env` written next to it, and the proxy in front of south now
// authenticates for real: it reads the credential, answers `401` itself when
// it does not know it, and strips it before the request reaches the mount --
// the mount still reads the sender from `X-Meclaw-Peer` and from nowhere else.
// For the OAuth form a token endpoint runs in this test process, like the
// proxy. Every test ends by looking for the secret and every token it handed
// out in all files of both colonies and in every log line the process wrote.
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The static credential; test-only, and deliberately unlike anything else in
/// the trees, so a hit can only be a leak.
const STATIC_SECRET: &str = "stat1c-cred-828-quux";
/// The OAuth client secret, same reasoning.
const CLIENT_SECRET: &str = "cl1ent-secret-828-zork";
/// Every token the test endpoint hands out ends in this, so one needle finds
/// all of them.
const TOKEN_SALT: &str = "t0ken-828-frob";

/// Every `tracing` event this test process emits at `debug` and above -- the
/// colony's own logging, which is where a credential could slip. Not caught:
/// `trace` events, and records of the `log` crate (reqwest, hyper), because no
/// `LogTracer` bridge is installed here; production installs one via
/// `try_init`. Today neither writes a header value. `nextest` runs each test in
/// a process of its own, so the global subscriber is this test's alone; under
/// `cargo test` the buffer is shared, which only makes the search wider.
fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buf);
        let sub = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
            .with_ansi(false)
            .with_writer(move || LogWriter(Arc::clone(&sink)))
            .finish();
        let _ = tracing::subscriber::set_global_default(sub);
        buf
    })
    .clone()
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(b);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reads one request off `s`: the head (without the blank line) and the body.
async fn read_request(s: &mut tokio::net::TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let head = loop {
        let mut b = [0u8; 4096];
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
    };
    let want = head + content_length(&buf[..head]);
    while buf.len() < want {
        let mut b = [0u8; 4096];
        match s.read(&mut b).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
    }
    Some((buf[..head - 2].to_vec(), buf[head..].to_vec()))
}

/// The header lines of a request head, names lower-cased, values trimmed.
fn header_lines(head: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(head)
        .split("\r\n")
        .skip(1)
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect()
}

fn header<'a>(lines: &'a [(String, String)], name: &str) -> Option<&'a str> {
    lines
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

async fn answer(s: &mut tokio::net::TcpStream, status: &str, ctype: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(msg.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// What the authenticating proxy decides for one request: `None` passes it on,
/// `Some(detail)` answers `401` with that detail as the body.
type Verdict = Arc<dyn Fn(&[(String, String)]) -> Option<String> + Send + Sync>;

/// A reverse proxy that authenticates by credential: it records the headers
/// of every request, asks `verdict`, and on a pass strips the credential
/// headers and writes the sender, as `peer_forwarder` does.
async fn auth_forwarder(
    upstream: SocketAddr,
    sender: &'static str,
    verdict: Verdict,
) -> (SocketAddr, Arc<Mutex<Vec<Vec<(String, String)>>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut down, _)) = l.accept().await {
            let verdict = Arc::clone(&verdict);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Some((head, body)) = read_request(&mut down).await else {
                    return;
                };
                let lines = header_lines(&head);
                log.lock().expect("seen").push(lines.clone());
                if let Some(detail) = verdict(lines.as_slice()) {
                    answer(&mut down, "401 Unauthorized", "text/plain", &detail).await;
                    return;
                }
                let mut out = strip_headers(
                    &head,
                    &[
                        "x-meclaw-peer",
                        "connection",
                        "authorization",
                        "x-peer-credential",
                    ],
                );
                out.extend_from_slice(
                    format!("X-Meclaw-Peer: {sender}\r\nConnection: close\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(&body);
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
    (addr, seen)
}

/// How the test token endpoint answers.
#[derive(Clone, Copy)]
enum TokenMode {
    /// A fresh token per request, valid for this many seconds.
    Issue { expires_in: u64 },
    /// `401` with the OAuth error `invalid_client`.
    Refuse,
    /// Accepts the connection and never answers.
    Silent,
}

/// An OAuth 2.0 token endpoint in the test process. The n-th request gets
/// `tok-<n>-<salt>`; the form bodies are recorded.
async fn token_endpoint(mode: TokenMode) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let n = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let log = Arc::clone(&log);
            let n = Arc::clone(&n);
            tokio::spawn(async move {
                let Some((_head, body)) = read_request(&mut s).await else {
                    return;
                };
                log.lock()
                    .expect("seen")
                    .push(String::from_utf8_lossy(&body).into_owned());
                match mode {
                    TokenMode::Issue { expires_in } => {
                        let i = n.fetch_add(1, Ordering::SeqCst) + 1;
                        let token = json!({
                            "access_token": format!("tok-{i}-{TOKEN_SALT}"),
                            "token_type": "Bearer",
                            "expires_in": expires_in,
                        });
                        answer(&mut s, "200 OK", "application/json", &token.to_string()).await;
                    }
                    TokenMode::Refuse => {
                        let e = json!({"error": "invalid_client"}).to_string();
                        answer(&mut s, "401 Unauthorized", "application/json", &e).await;
                    }
                    TokenMode::Silent => {
                        // Holds the connection open and says nothing.
                        std::future::pending::<()>().await;
                        drop(s);
                    }
                }
            });
        }
    });
    (addr, seen)
}

/// Boots north with `auth` in its peer cell's params, `extra` merged beside
/// it, and `env` as its `.env`.
async fn boot_north_with(
    peer_url: &str,
    auth: Value,
    extra: Value,
    env: &[(&str, &str)],
) -> Colony {
    let td = prepare_north(peer_url, auth, extra, env);
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

/// The north tree in a TempDir, its peer cell patched and its `.env` written.
fn prepare_north(
    peer_url: &str,
    auth: Value,
    extra: Value,
    env: &[(&str, &str)],
) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path("peer-north"), td.path());
    let origin = origin_of(peer_url);
    rewrite_configs(
        td.path(),
        &[
            ("__PEER_URL__", peer_url),
            ("__PEER_ORIGIN__", origin.as_str()),
        ],
    );
    let cfg_path = td.path().join("main/friend/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("read")).expect("json");
    cfg["params"]["auth"] = auth;
    if let Some(more) = extra.as_object() {
        for (k, v) in more {
            cfg["params"][k] = v.clone();
        }
    }
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).expect("ser")).expect("write");
    let lines: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(td.path().join(".env"), lines).expect("write env");
    td
}

/// South, the authenticating proxy in front of it, and north with `auth`.
async fn auth_pair(
    verdict: Verdict,
    auth: Value,
    extra: Value,
    env: &[(&str, &str)],
) -> (Colony, Arc<Mutex<Vec<Vec<(String, String)>>>>, Colony) {
    let _ = captured_logs();
    let south = boot("peer-south", &[]).await;
    let (fwd, seen) = auth_forwarder(south.addr, "north", verdict).await;
    let url = format!("http://{fwd}/peer/");
    let north = boot_north_with(&url, auth, extra, env).await;
    (south, seen, north)
}

fn oauth_block(token: SocketAddr) -> Value {
    json!({
        "token_url": format!("http://{token}/realms/test/protocol/openid-connect/token"),
        "client_id": "${PEER_CLIENT_ID}",
        "client_secret": "${PEER_CLIENT_SECRET}",
        "scope": "peer",
        "audience": "south",
    })
}

const OAUTH_ENV: &[(&str, &str)] = &[
    ("PEER_CLIENT_ID", "north-colony"),
    ("PEER_CLIENT_SECRET", CLIENT_SECRET),
];

/// Every file under both colonies' roots except the `.env` that holds the
/// secret on purpose, and every captured log line: none contains a needle.
fn assert_nowhere(colonies: &[&Colony], needles: &[&str]) {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let p = e.path();
            if e.file_type().expect("file_type").is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n != ".env") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for c in colonies {
        walk(c._td.path(), &mut files);
    }
    assert!(!files.is_empty(), "the search has something to search");
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        for n in needles {
            assert!(
                find(&bytes, n.as_bytes()).is_none(),
                "a credential stands in {} -- it must live in .env and in memory only",
                f.display()
            );
        }
    }
    let logs = captured_logs().lock().expect("logs").clone();
    assert!(
        !logs.is_empty(),
        "the log capture saw lines; an empty one would prove nothing"
    );
    for n in needles {
        assert!(
            find(&logs, n.as_bytes()).is_none(),
            "a credential stands in a log line"
        );
    }
}

/// North's one receipt on `trace`.
async fn north_receipt(north: &Colony, t: &str) -> Row {
    let mut r = wait_for_rows(&north.addr, t, "/receipts", 1).await;
    assert_eq!(r.len(), 1, "one receipt per crossing");
    r.remove(0)
}

fn topic() -> Value {
    json!({"messages": [], "topic": "gardening"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_static_credential_rides_every_post_and_a_proxy_that_strips_it_still_delivers() {
    let verdict: Verdict = Arc::new(|h: &[(String, String)]| {
        (header(h, "x-peer-credential") != Some(STATIC_SECRET))
            .then(|| "south-proxy: no known credential".to_string())
    });
    let auth = json!({"header": "X-Peer-Credential", "value": "${PEER_CREDENTIAL}"});
    let (south, seen, north) = auth_pair(
        verdict,
        auth,
        json!({}),
        &[("PEER_CREDENTIAL", STATIC_SECRET)],
    )
    .await;
    for _ in 0..2 {
        let t = drive(&north.addr, "topic", topic(), 64).await;
        assert_eq!(
            north_receipt(&north, &t).await.hop["peer_event"],
            json!("crossed"),
            "the proxy knew the credential and let the frame through"
        );
        assert_eq!(
            wait_for_rows(&south.addr, &t, "/sink", 1).await[0].hop["peer"],
            json!("north"),
            "the mount delivered without the header, which the proxy stripped: it is additive"
        );
    }
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 2, "one POST per crossing");
    for h in &seen {
        assert_eq!(
            header(h, "x-peer-credential"),
            Some(STATIC_SECRET),
            "every POST carries the header with the resolved value"
        );
    }
    assert_nowhere(&[&north, &south], &[STATIC_SECRET]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_the_oauth_form_fetches_one_token_for_two_crossings() {
    let (tok, grants) = token_endpoint(TokenMode::Issue { expires_in: 3600 }).await;
    let verdict: Verdict = Arc::new(|h: &[(String, String)]| {
        (!header(h, "authorization").is_some_and(|v| v.starts_with("Bearer tok-")))
            .then(|| "south-proxy: no bearer".to_string())
    });
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), json!({}), OAUTH_ENV).await;
    for _ in 0..2 {
        let t = drive(&north.addr, "topic", topic(), 64).await;
        assert_eq!(
            north_receipt(&north, &t).await.hop["peer_event"],
            json!("crossed"),
            "the bearer was accepted"
        );
    }
    let grants = grants.lock().expect("grants").clone();
    assert_eq!(grants.len(), 1, "one token request for two crossings");
    let form: Vec<(String, String)> = grants[0]
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let get = |k: &str| form.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(
        get("grant_type"),
        Some("client_credentials"),
        "RFC 6749 § 4.4"
    );
    assert_eq!(get("client_id"), Some("north-colony"));
    assert_eq!(
        get("client_secret"),
        Some(CLIENT_SECRET),
        "the secret goes in the form body"
    );
    assert_eq!(get("scope"), Some("peer"));
    assert_eq!(get("audience"), Some("south"));
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 2);
    for h in &seen {
        assert_eq!(
            header(h, "authorization"),
            Some(format!("Bearer tok-1-{TOKEN_SALT}").as_str()),
            "both crossings carry the one cached token"
        );
    }
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET, TOKEN_SALT]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_token_past_expires_in_is_fetched_again() {
    let (tok, grants) = token_endpoint(TokenMode::Issue { expires_in: 1 }).await;
    let verdict: Verdict = Arc::new(|_: &[(String, String)]| None);
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), json!({}), OAUTH_ENV).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    assert_eq!(
        north_receipt(&north, &t).await.hop["peer_event"],
        json!("crossed")
    );
    // One second is the token's whole life; past it the cache must not hold.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    assert_eq!(
        north_receipt(&north, &t).await.hop["peer_event"],
        json!("crossed")
    );
    assert_eq!(
        grants.lock().expect("grants").len(),
        2,
        "an expired token is fetched again"
    );
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(
        header(&seen[1], "authorization"),
        Some(format!("Bearer tok-2-{TOKEN_SALT}").as_str()),
        "and the second crossing carries the new one"
    );
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET, TOKEN_SALT]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_401_fetches_once_more_and_retries_once() {
    let (tok, grants) = token_endpoint(TokenMode::Issue { expires_in: 3600 }).await;
    // The proxy has revoked the first token; the second is good.
    let verdict: Verdict = Arc::new(|h: &[(String, String)]| {
        (header(h, "authorization") != Some(format!("Bearer tok-2-{TOKEN_SALT}").as_str()))
            .then(|| "south-proxy: token revoked".to_string())
    });
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), json!({}), OAUTH_ENV).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    assert_eq!(
        north_receipt(&north, &t).await.hop["peer_event"],
        json!("crossed"),
        "the retry with a fresh token crossed"
    );
    assert_eq!(
        grants.lock().expect("grants").len(),
        2,
        "exactly one re-fetch"
    );
    assert_eq!(seen.lock().expect("seen").len(), 2, "exactly one retry");
    assert_eq!(
        wait_for_rows(&south.addr, &t, "/sink", 1).await.len(),
        1,
        "one arrival"
    );
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET, TOKEN_SALT]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_second_401_is_peer_refused_with_the_far_sides_detail() {
    let (tok, grants) = token_endpoint(TokenMode::Issue { expires_in: 3600 }).await;
    let verdict: Verdict = Arc::new(|_: &[(String, String)]| {
        Some("south-proxy: this client is not admitted".to_string())
    });
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), json!({}), OAUTH_ENV).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    let r = north_receipt(&north, &t).await;
    assert_eq!(r.hop["peer_event"], json!("refused"));
    assert_eq!(
        r.hop["error_code"],
        json!("peer_refused"),
        "the far side refused twice"
    );
    assert_eq!(r.hop["boundary"], json!("north"), "booked on this side");
    let detail = r.body["messages"][0]["text"].as_str().expect("detail");
    assert!(
        detail.contains("south-proxy: this client is not admitted"),
        "the detail carries the far side's word -- {detail}"
    );
    assert_eq!(
        grants.lock().expect("grants").len(),
        2,
        "one re-fetch, no more"
    );
    assert_eq!(seen.lock().expect("seen").len(), 2, "one retry, no more");
    assert!(
        rows(&south.addr, &t, "/").await.is_empty(),
        "nothing crossed"
    );
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET, TOKEN_SALT]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_token_endpoint_that_does_not_answer_refuses_on_the_sending_side() {
    let (tok, grants) = token_endpoint(TokenMode::Silent).await;
    let verdict: Verdict = Arc::new(|_: &[(String, String)]| None);
    let extra = json!({"external_timeout_ms": 1000});
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), extra, OAUTH_ENV).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    let r = north_receipt(&north, &t).await;
    assert_eq!(r.hop["peer_event"], json!("refused"), "a receipt, refused");
    assert_eq!(r.hop["error_code"], json!("auth_unavailable"));
    assert_eq!(r.hop["boundary"], json!("north"), "on the sending side");
    assert_eq!(
        grants.lock().expect("grants").len(),
        1,
        "the endpoint was asked"
    );
    assert!(
        seen.lock().expect("seen").is_empty(),
        "and the peer never was"
    );
    assert!(
        rows(&south.addr, &t, "/").await.is_empty(),
        "nothing crossed half"
    );
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET, TOKEN_SALT]);
    north.stop().await;
    south.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_token_endpoint_answering_non_2xx_refuses_on_the_sending_side() {
    let (tok, _grants) = token_endpoint(TokenMode::Refuse).await;
    let verdict: Verdict = Arc::new(|_: &[(String, String)]| None);
    let (south, seen, north) = auth_pair(verdict, oauth_block(tok), json!({}), OAUTH_ENV).await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    let r = north_receipt(&north, &t).await;
    assert_eq!(r.hop["error_code"], json!("auth_unavailable"));
    let detail = r.body["messages"][0]["text"].as_str().expect("detail");
    assert!(
        detail.contains("401"),
        "the detail names the status -- {detail}"
    );
    assert!(
        detail.contains("invalid_client"),
        "and the OAuth error code -- {detail}"
    );
    assert!(
        seen.lock().expect("seen").is_empty(),
        "the peer was never asked"
    );
    assert_nowhere(&[&north, &south], &[CLIENT_SECRET]);
    north.stop().await;
    south.stop().await;
}

/// Boots north and expects the boot to be refused; returns the refusal.
async fn boot_refused(auth: Value, env: &[(&str, &str)]) -> String {
    let td = prepare_north("http://127.0.0.1:9/peer/", auth, json!({}), env);
    let cli = cli_for(td.path());
    let (addr_tx, _addr_rx) = tokio::sync::oneshot::channel();
    let (_shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let res = tokio::time::timeout(RECV, run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)))
        .await
        .expect("a refused boot ends within 30s");
    format!("{:#}", res.expect_err("the boot must be refused"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_literal_secret_or_both_forms_at_once_do_not_boot() {
    let literal = "a-literal-in-config-json";
    let e = boot_refused(
        json!({"header": "X-Peer-Credential", "value": literal}),
        &[],
    )
    .await;
    assert!(e.contains("auth.value"), "the refusal names the key -- {e}");
    assert!(!e.contains(literal), "and never echoes the literal");
    let e = boot_refused(
        json!({"token_url": "http://127.0.0.1:9/token", "client_id": "north",
               "client_secret": literal}),
        &[],
    )
    .await;
    assert!(
        e.contains("auth.client_secret"),
        "the refusal names the key -- {e}"
    );
    assert!(!e.contains(literal), "and never echoes the literal");
    let e = boot_refused(
        json!({"header": "X-Peer-Credential", "value": "${PEER_CREDENTIAL}",
               "token_url": "http://127.0.0.1:9/token"}),
        &[("PEER_CREDENTIAL", STATIC_SECRET)],
    )
    .await;
    assert!(e.contains("auth"), "the refusal names the block -- {e}");
    assert!(
        e.contains("token_url"),
        "and the key of the second form -- {e}"
    );
    assert!(!e.contains(STATIC_SECRET), "and never the resolved value");
}

/// GH #828 fix round 1 (review I2): the static form does not retry. A `401`
/// to a static credential is `peer_refused` at once -- the same value sent
/// again would be refused again -- with the far side's word in the detail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_401_to_the_static_form_is_peer_refused_after_one_post() {
    let verdict: Verdict = Arc::new(|_: &[(String, String)]| {
        Some("south-proxy: this credential is revoked".to_string())
    });
    let auth = json!({"header": "X-Peer-Credential", "value": "${PEER_CREDENTIAL}"});
    let (south, seen, north) = auth_pair(
        verdict,
        auth,
        json!({}),
        &[("PEER_CREDENTIAL", STATIC_SECRET)],
    )
    .await;
    let t = drive(&north.addr, "topic", topic(), 64).await;
    let r = north_receipt(&north, &t).await;
    assert_eq!(r.hop["peer_event"], json!("refused"));
    assert_eq!(r.hop["error_code"], json!("peer_refused"));
    assert_eq!(r.hop["boundary"], json!("north"), "booked on this side");
    let detail = r.body["messages"][0]["text"].as_str().expect("detail");
    assert!(
        detail.contains("south-proxy: this credential is revoked"),
        "the detail carries the far side's word -- {detail}"
    );
    assert_eq!(
        seen.lock().expect("seen").len(),
        1,
        "exactly one POST: the static form has nothing fresh to retry with"
    );
    assert!(
        rows(&south.addr, &t, "/").await.is_empty(),
        "nothing crossed"
    );
    assert_nowhere(&[&north, &south], &[STATIC_SECRET]);
    north.stop().await;
    south.stop().await;
}

/// Every `config.json` under `root` with its bytes: the tree as the colony
/// would boot it next time.
fn config_snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let p = e.path();
            if e.file_type().expect("file_type").is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                out.push((p.clone(), std::fs::read(&p).expect("read")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

/// GH #828 fix round 1 (review I1): the mutation door asks the same question
/// as the boot. A `meclaw` proxy grown by `add_nodes` with a literal
/// `auth.value` is refused with `invalid_params`, naming the key and never the
/// literal, and not one `config.json` of the tree has moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh828_a_literal_secret_is_refused_at_the_mutation_door_too() {
    let literal = "a-literal-grown-by-a-mutation";
    let td = prepare_north(
        "http://127.0.0.1:9/peer/",
        json!({"header": "X-Peer-Credential", "value": "${PEER_CREDENTIAL}"}),
        json!({}),
        &[("PEER_CREDENTIAL", STATIC_SECRET)],
    );
    // A second peer proxy as a template: north's own, `auth` as a token, on a
    // mount of its own. `override_params` may only address a param the
    // template has (GH #198), so the mutation replaces `auth`, not adds it.
    let mut tpl: Value = serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/friend/config.json")).expect("read"),
    )
    .expect("json");
    tpl["params"]["mount"] = json!("peer-two");
    let tpl_dir = td.path().join("templates/peer-two");
    std::fs::create_dir_all(&tpl_dir).expect("mkdir");
    std::fs::write(tpl_dir.join("template.json"), br#"{"name":"peer-two"}"#).expect("write");
    std::fs::write(
        tpl_dir.join("config.json"),
        serde_json::to_string_pretty(&tpl).expect("ser"),
    )
    .expect("write");
    let mut cli = cli_for(td.path());
    cli.rescan_templates = true;
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = tokio::time::timeout(RECV, addr_rx)
        .await
        .expect("the colony must bind HTTP within 30s")
        .expect("addr hook");
    let before = config_snapshot(td.path());

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/colony/mutations"))
        .json(&json!({
            "scope": "/",
            "ctx": {},
            "diff": {"add_nodes": [{
                "name": "friend-two",
                "template": "peer-two",
                "override_params": {
                    "auth": {"header": "X-Peer-Credential", "value": literal}
                }
            }]}
        }))
        .send()
        .await
        .expect("post");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.expect("json");
    assert_eq!(status, 422, "a refused mutation -- {body}");
    assert_eq!(body["mutation"]["outcome"], json!("rejected"), "{body}");
    assert_eq!(
        body["mutation"]["error_code"],
        json!("invalid_params"),
        "{body}"
    );
    let details = body["mutation"]["details"].as_str().expect("details");
    assert!(
        details.contains("auth.value"),
        "the refusal names the key -- {details}"
    );
    assert!(
        !body.to_string().contains(literal),
        "and never echoes the literal -- {body}"
    );
    assert_eq!(
        config_snapshot(td.path()),
        before,
        "the tree is unchanged: no new node, no staging rest, no rewritten config"
    );
    assert!(!td.path().join("main/friend-two").exists());

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(RECV, join).await;
}
