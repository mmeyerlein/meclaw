//! GH #1024: every WebSocket route of the colony's listener offers
//! permessage-deflate (RFC 7692), and the unchanged LiveView client in the
//! browser negotiates it on its own.
//!
//! Why: the join tree of a large page is the first thing a viewer waits for.
//! The lab page of the web-cell hardening wave answered with ~755 KB of join
//! frames uncompressed (H4 receipt, A5/A6), and on Slow 3G that is 17.8 s to a
//! first picture. JSON of a packed LiveView tree is highly repetitive, so the
//! wire is where it shrinks — without touching the client (R-VL-6) or the
//! frame contract.
//!
//! The client here is a raw one on purpose: the locks are about bytes and the
//! RSV1 bit on the wire, which a WebSocket library hides. It speaks just enough
//! RFC 6455 to join: a handshake, masked client frames, and frame headers read
//! one by one.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress};
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use web_fixture::{CELL, Lab, MOUNT, Shape};

/// The threshold the server compresses from (`DEFLATE_MIN_BYTES`, 4 KiB).
const THRESHOLD: usize = meclaw_cells::websocket::DEFLATE_MIN_BYTES;

/// One frame as it travelled.
struct WireFrame {
    rsv1: bool,
    opcode: u8,
    /// The payload as it was on the wire (compressed if `rsv1`).
    wire: Vec<u8>,
}

impl WireFrame {
    /// The message text: inflated when the frame was compressed.
    fn text(&self) -> String {
        let bytes = if self.rsv1 {
            inflate(&self.wire)
        } else {
            self.wire.clone()
        };
        String::from_utf8(bytes).expect("a text frame is UTF-8")
    }
}

/// Inflate one permessage-deflate payload (RFC 7692 § 7.2.2: append the tail).
fn inflate(wire: &[u8]) -> Vec<u8> {
    let mut input = wire.to_vec();
    input.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
    let mut d = Decompress::new(false);
    let mut out = Vec::with_capacity(input.len() * 8);
    loop {
        let consumed = d.total_in() as usize;
        if consumed >= input.len() {
            break;
        }
        out.reserve(64 * 1024);
        let before = d.total_out();
        d.decompress_vec(&input[consumed..], &mut out, FlushDecompress::Sync)
            .expect("the server's deflate stream inflates");
        if d.total_out() == before && d.total_in() as usize == consumed {
            break;
        }
    }
    out
}

/// Deflate one message as a browser does (RFC 7692 § 7.2.1: drop the tail).
fn deflate(text: &str) -> Vec<u8> {
    let mut c = Compress::new(Compression::fast(), false);
    let mut out = Vec::with_capacity(text.len() + 64);
    c.compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync)
        .expect("deflate");
    while out.len() == out.capacity() {
        out.reserve(1024);
        c.compress_vec(&[], &mut out, FlushCompress::Sync)
            .expect("deflate");
    }
    assert!(out.ends_with(&[0x00, 0x00, 0xff, 0xff]), "sync flush tail");
    out.truncate(out.len() - 4);
    out
}

/// A raw RFC 6455 client: enough to handshake, join and read frames.
struct Raw {
    tcp: TcpStream,
    /// The response head of the upgrade, lower-cased header names.
    head: String,
    /// Bytes read past the head.
    buf: Vec<u8>,
}

impl Raw {
    async fn connect(port: u16, path: &str, extensions: Option<&str>) -> Raw {
        let mut tcp = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let mut req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n"
        );
        if let Some(ext) = extensions {
            req.push_str(&format!("Sec-WebSocket-Extensions: {ext}\r\n"));
        }
        req.push_str("\r\n");
        tcp.write_all(req.as_bytes()).await.expect("write upgrade");
        let mut buf = Vec::new();
        let end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(Duration::from_secs(30), tcp.read(&mut chunk))
                .await
                .expect("the upgrade is answered")
                .expect("read");
            assert!(n > 0, "the server closed during the handshake");
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
        let rest = buf[end..].to_vec();
        Raw {
            tcp,
            head,
            buf: rest,
        }
    }

    /// The value of a response header (lower-cased), if present.
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    }

    async fn fill(&mut self, n: usize) {
        while self.buf.len() < n {
            let mut chunk = vec![0u8; 64 * 1024];
            let got = tokio::time::timeout(Duration::from_secs(30), self.tcp.read(&mut chunk))
                .await
                .expect("a frame within the failure-marker window")
                .expect("read");
            assert!(got > 0, "the server closed the socket");
            self.buf.extend_from_slice(&chunk[..got]);
        }
    }

    async fn read_frame(&mut self) -> WireFrame {
        self.fill(2).await;
        let b0 = self.buf[0];
        let b1 = self.buf[1];
        assert_eq!(b0 & 0x80, 0x80, "the server sends whole messages (FIN)");
        assert_eq!(b1 & 0x80, 0, "a server frame is never masked");
        let (len, hdr) = match b1 & 0x7f {
            126 => {
                self.fill(4).await;
                (u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize, 4)
            }
            127 => {
                self.fill(10).await;
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.buf[2..10]);
                (u64::from_be_bytes(b) as usize, 10)
            }
            n => (n as usize, 2),
        };
        self.fill(hdr + len).await;
        let wire = self.buf[hdr..hdr + len].to_vec();
        self.buf.drain(..hdr + len);
        WireFrame {
            rsv1: b0 & 0x40 != 0,
            opcode: b0 & 0x0f,
            wire,
        }
    }

    /// Send one masked text frame; compressed (RSV1) when `compress`.
    async fn send_text(&mut self, text: &str, compress: bool) {
        let payload = if compress {
            deflate(text)
        } else {
            text.as_bytes().to_vec()
        };
        let mut frame = vec![if compress { 0xC1 } else { 0x81 }];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.tcp.write_all(&frame).await.expect("send frame");
    }

    /// The next text frame whose decoded JSON matches `pred`.
    async fn next_where(&mut self, pred: impl Fn(&Value) -> bool) -> (WireFrame, Value) {
        loop {
            let f = self.read_frame().await;
            if f.opcode != 1 {
                continue;
            }
            let v: Value = meclaw_core::serde_json::from_str(&f.text()).expect("JSON frame");
            if pred(&v) {
                return (f, v);
            }
        }
    }
}

fn socket_path() -> String {
    format!("/{MOUNT}/live/websocket")
}

fn token_in(page: &str) -> String {
    let marker = "data-phx-session=\"";
    let start = page.find(marker).expect("the shell carries a token") + marker.len();
    let end = start + page[start..].find('"').expect("quoted");
    page[start..end].to_string()
}

fn join_frame(lab: &Lab, token: &str) -> String {
    let topic = format!("lv:{}", meclaw_surface::session::container_id(CELL));
    json!(["1", "1", topic, "phx_join", {
        "session": token,
        "url": format!("http://127.0.0.1:{}/{MOUNT}/", lab.port)
    }])
    .to_string()
}

/// What Chrome and Firefox send (`client_max_window_bits` without a value).
const BROWSER_OFFER: &str = "permessage-deflate; client_max_window_bits";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offer_is_answered_with_permessage_deflate() {
    let lab = Lab::start(Shape::with_figures(3)).await;
    let raw = Raw::connect(lab.port, &socket_path(), Some(BROWSER_OFFER)).await;
    assert!(
        raw.head.starts_with("http/1.1 101"),
        "the upgrade succeeds: {}",
        raw.head
    );
    let ext = raw
        .header("sec-websocket-extensions")
        .unwrap_or_else(|| panic!("an offer is answered: {}", raw.head));
    assert!(
        ext.starts_with("permessage-deflate"),
        "the answer names the extension: {ext}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_an_offer_the_socket_stays_plain() {
    let lab = Lab::start(Shape::default()).await;
    let token = token_in(&lab.get_page().await);
    let mut raw = Raw::connect(lab.port, &socket_path(), None).await;
    assert!(raw.head.starts_with("http/1.1 101"), "{}", raw.head);
    assert_eq!(
        raw.header("sec-websocket-extensions"),
        None,
        "no offer, no extension"
    );
    raw.send_text(&join_frame(&lab, &token), false).await;
    let (f, reply) = raw.next_where(|v| v[3] == json!("phx_reply")).await;
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
    assert!(!f.rsv1, "nothing is compressed without the extension");
}

/// Lock (2) of the wave: the join frame of a large page is at most a third of
/// its own size on the wire — and inflates to exactly what a plain socket
/// receives, so the frame contract is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_join_frame_travels_at_most_a_third_of_its_size() {
    let lab = Lab::start(Shape::default()).await;
    let token = token_in(&lab.get_page().await);

    let mut plain = Raw::connect(lab.port, &socket_path(), None).await;
    plain.send_text(&join_frame(&lab, &token), false).await;
    let (pf, _) = plain.next_where(|v| v[3] == json!("phx_reply")).await;
    let uncompressed = pf.text();

    let mut raw = Raw::connect(lab.port, &socket_path(), Some(BROWSER_OFFER)).await;
    raw.send_text(&join_frame(&lab, &token), false).await;
    let (f, reply) = raw.next_where(|v| v[3] == json!("phx_reply")).await;
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
    let text = f.text();
    assert!(
        text.len() >= 4 * THRESHOLD,
        "precondition: the lab join is large ({} B)",
        text.len()
    );
    assert!(f.rsv1, "a frame above the threshold is compressed");
    eprintln!(
        "gh1024: join frame {} B uncompressed, {} B on the wire ({:.1} %)",
        text.len(),
        f.wire.len(),
        100.0 * f.wire.len() as f64 / text.len() as f64
    );
    assert!(
        f.wire.len() * 3 <= text.len(),
        "the join travels at most a third: {} of {} B",
        f.wire.len(),
        text.len()
    );
    assert_eq!(
        text, uncompressed,
        "the frame inflates to exactly what a plain socket receives"
    );
}

/// Lock (3): a frame below the threshold is sent as it is, byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_small_frame_is_not_compressed() {
    let lab = Lab::start(Shape::with_figures(3)).await;
    let mut raw = Raw::connect(lab.port, &socket_path(), Some(BROWSER_OFFER)).await;
    let beat = json!(["1", "2", "phoenix", "heartbeat", {}]).to_string();
    raw.send_text(&beat, false).await;
    let (f, reply) = raw.next_where(|v| v[1] == json!("2")).await;
    assert_eq!(reply[4]["status"], json!("ok"));
    assert!(!f.rsv1, "a small frame is not compressed");
    assert!(f.wire.len() < THRESHOLD);
    assert_eq!(
        f.wire.len(),
        reply.to_string().len(),
        "the payload on the wire is the frame's own text"
    );
}

/// A browser compresses what IT sends once the extension is agreed. The server
/// must read that, or the first click after the join is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compressed_client_frame_is_read() {
    let lab = Lab::start(Shape::with_figures(3)).await;
    let token = token_in(&lab.get_page().await);
    let mut raw = Raw::connect(lab.port, &socket_path(), Some(BROWSER_OFFER)).await;
    assert!(raw.header("sec-websocket-extensions").is_some());
    raw.send_text(&join_frame(&lab, &token), true).await;
    let (_, reply) = raw.next_where(|v| v[3] == json!("phx_reply")).await;
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
    // And a second compressed message: no context is carried between messages
    // (the server asked for `client_no_context_takeover`).
    let beat = json!(["1", "9", "phoenix", "heartbeat", {}]).to_string();
    raw.send_text(&beat, true).await;
    let (_, reply) = raw.next_where(|v| v[1] == json!("9")).await;
    assert_eq!(reply[4]["status"], json!("ok"));
}
