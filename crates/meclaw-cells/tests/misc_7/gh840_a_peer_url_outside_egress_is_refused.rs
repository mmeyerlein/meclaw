//! GH #840: a peer cell sends only to an origin its own `params.egress` lists,
//! and without a list it sends nothing (fail-closed).
//!
//! `hop.peer_url` is written by whoever wrote the message: any edge
//! (`set_hop`), any `header` slot, any caller of `/messages`. Before this lock
//! the cell posted the frame there unchecked and hung the credential of
//! `params.auth` on the POST, with the OAuth form after a token request. The
//! measurement is at the receiving end: two counting listeners, the would-be
//! peer and the token endpoint, and a refusal must leave BOTH at zero
//! connections — the check falls before the token request and the connect.
use meclaw_cells::proxy::meclaw::{cell::MeclawCell, params::MeclawParams};
use meclaw_colony::LongRunningCell;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, Headers, MessageBuilder, OutputSink, Path, Uuid};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// A listener that counts every connection it accepts and answers each one
/// with `body` as JSON after reading the request.
async fn counting(body: Value) -> (SocketAddr, Arc<AtomicUsize>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    let n = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&n);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
            let body = body.to_string();
            tokio::spawn(async move {
                read_request(&mut s).await;
                let msg = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(msg.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    (addr, n)
}

/// Reads the head and as much body as `Content-Length` says.
async fn read_request(s: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let head = loop {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let text = String::from_utf8_lossy(&buf[..head]).to_ascii_lowercase();
    let len = text
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head + len {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
    }
}

fn params(egress: Option<Value>, auth: Option<Value>) -> Value {
    let mut v = json!({"platform": "meclaw", "mount": "peer", "identity_header": "X-Meclaw-Peer",
        "boundary": "north", "emit_to": "/sink", "external_timeout_ms": 5000, "lanes": {
            "accepts": [],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}});
    if let Some(e) = egress {
        v["egress"] = e;
    }
    if let Some(a) = auth {
        v["auth"] = a;
    }
    v
}

/// Drives `handle()` once with `peer_url` on the hop and returns the emitted
/// `header`, with the refusal's detail (the one turn's text, `emit.rs`) copied
/// in as `detail` so each case reads one value.
async fn cross(v: &Value, peer_url: &str) -> Value {
    let p = MeclawParams::parse(v).expect("params");
    let mut cell = MeclawCell::new(&p).expect("cell");
    let (tx, mut rx) = mpsc::channel(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/friend"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    let mut hop = serde_json::Map::new();
    hop.insert("route".into(), json!("proposal"));
    hop.insert("peer".into(), json!("south"));
    hop.insert("peer_url".into(), json!(peer_url));
    let msg = MessageBuilder::new(Path::new("/friend"))
        .ttl(5)
        .hop(hop)
        .body(Body::Inline(json!({"proposal": "a walk"})))
        .build();
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("a receipt within 30s")
        .expect("a receipt");
    let mut h = em.content["header"].clone();
    h["detail"] = em.content["messages"][0]["text"].clone();
    h
}

/// The two listeners: a peer answering `crossed`, a token endpoint issuing one token.
async fn pair() -> (
    (SocketAddr, Arc<AtomicUsize>),
    (SocketAddr, Arc<AtomicUsize>),
) {
    let receipt = meclaw_cells::proxy::meclaw::wire::crossed_receipt("proposal", "south", &[]);
    let peer = counting(receipt).await;
    let token =
        counting(json!({"access_token": "tok-840", "token_type": "Bearer", "expires_in": 300}))
            .await;
    (peer, token)
}

fn oauth(token: SocketAddr) -> Value {
    json!({"token_url": format!("http://{token}/token"), "client_id": "north",
        "client_secret": "s3cret-840"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_origin_outside_the_list_is_refused_before_the_token_and_the_connect() {
    let ((peer, peer_n), (token, token_n)) = pair().await;
    let v = params(Some(json!(["https://gateway.example"])), Some(oauth(token)));
    let h = cross(&v, &format!("http://{peer}/peer/")).await;
    assert_eq!(
        (h["peer_event"].clone(), h["error_code"].clone()),
        (json!("refused"), json!("egress_denied")),
        "{h}"
    );
    let detail = h["detail"].as_str().expect("detail");
    assert!(
        detail.contains(&format!("http://{peer}")) && detail.contains("params.egress"),
        "the detail names the origin and the list -- {detail}"
    );
    assert_eq!(peer_n.load(Ordering::SeqCst), 0, "no connect to the peer");
    assert_eq!(token_n.load(Ordering::SeqCst), 0, "no token request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_list_nothing_goes_out() {
    let ((peer, peer_n), (token, token_n)) = pair().await;
    for auth in [
        None,
        Some(json!({"header": "X-Peer-Credential", "value": "s3cret-840"})),
        Some(oauth(token)),
    ] {
        let h = cross(&params(None, auth), &format!("http://{peer}/peer/")).await;
        assert_eq!(
            h["error_code"],
            json!("egress_denied"),
            "fail-closed -- {h}"
        );
        let detail = h["detail"].as_str().expect("detail");
        assert!(
            detail.contains("\"egress\": [\"https://<gateway-origin>\"]"),
            "the refusal is the migration text -- {detail}"
        );
    }
    assert_eq!(peer_n.load(Ordering::SeqCst), 0, "no connect to the peer");
    assert_eq!(token_n.load(Ordering::SeqCst), 0, "no token request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listed_origin_is_posted_to() {
    let ((peer, peer_n), (token, token_n)) = pair().await;
    // Anonymous: one connection to the peer and a crossed receipt.
    let v = params(Some(json!([format!("http://{peer}")])), None);
    let h = cross(&v, &format!("http://{peer}/peer/")).await;
    assert_eq!(h["peer_event"], json!("crossed"), "{h}");
    assert_eq!(peer_n.load(Ordering::SeqCst), 1);
    // OAuth: the token first, then the POST; a trailing `/` on the entry is the same origin.
    let v = params(Some(json!([format!("http://{peer}/")])), Some(oauth(token)));
    let h = cross(&v, &format!("http://{peer}/peer/")).await;
    assert_eq!(h["peer_event"], json!("crossed"), "{h}");
    assert_eq!(
        (
            peer_n.load(Ordering::SeqCst),
            token_n.load(Ordering::SeqCst)
        ),
        (2, 1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_in_the_peer_url_and_a_foreign_scheme_are_refused_without_a_connect() {
    let ((peer, peer_n), (token, token_n)) = pair().await;
    let v = params(Some(json!([format!("http://{peer}")])), Some(oauth(token)));
    let h = cross(&v, &format!("http://id:pw-840@{peer}/peer/")).await;
    assert_eq!(h["error_code"], json!("egress_denied"), "{h}");
    let detail = h["detail"].as_str().expect("detail");
    assert!(
        !detail.contains("pw-840"),
        "the URL is not echoed -- {detail}"
    );
    for bad in [format!("ftp://{peer}/peer/"), "not a url".to_string()] {
        let h = cross(&v, &bad).await;
        assert_eq!(h["error_code"], json!("egress_denied"), "{bad}: {h}");
    }
    // A foreign scheme or an unparsable URL with credentials in it is not
    // echoed either: the receipt and the log line would carry them (review G,
    // Minor 1).
    for bad in [
        format!("ftp://id:pw-840@{peer}/peer/"),
        "http://id:pw-840@[x".to_string(),
        "foo:id:pw-840@x".to_string(),
    ] {
        let h = cross(&v, &bad).await;
        assert_eq!(h["error_code"], json!("egress_denied"), "{bad}: {h}");
        let detail = h["detail"].as_str().expect("detail");
        assert!(!detail.contains("pw-840"), "{bad}: not echoed -- {detail}");
    }
    assert_eq!(peer_n.load(Ordering::SeqCst), 0, "no connect to the peer");
    assert_eq!(token_n.load(Ordering::SeqCst), 0, "no token request");
}

/// The peer_url side of the comparison is normalised the way the egress side
/// is (`Url::origin().ascii_serialization()`): scheme and host lower-case, an
/// IDN host as punycode, the default port dropped. The refusal names the
/// origin it compared, so the form is pinned without a DNS lookup.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_peer_url_is_compared_as_its_normalised_origin() {
    let v = params(Some(json!(["https://other.example"])), None);
    for (peer_url, origin) in [
        (
            "HTTPS://GATEWAY.Example:443/peer/",
            "https://gateway.example",
        ),
        (
            "https://b\u{fc}cher.example/peer/",
            "https://xn--bcher-kva.example",
        ),
        ("http://gateway.example:80/peer/", "http://gateway.example"),
        (
            "https://gateway.example:8443/peer/",
            "https://gateway.example:8443",
        ),
    ] {
        let h = cross(&v, peer_url).await;
        assert_eq!(h["error_code"], json!("egress_denied"), "{peer_url}: {h}");
        let detail = h["detail"].as_str().expect("detail");
        assert!(
            detail.contains(&format!("the origin {origin} of hop.peer_url")),
            "{peer_url} compares as {origin} -- {detail}"
        );
    }
    // Both sides normalised: an entry with an upper-case scheme and a
    // trailing `/` admits the lower-case URL.
    let ((peer, peer_n), _) = pair().await;
    let v = params(Some(json!([format!("HTTP://{peer}/")])), None);
    let h = cross(&v, &format!("http://{peer}/peer/")).await;
    assert_eq!(h["peer_event"], json!("crossed"), "{h}");
    assert_eq!(peer_n.load(Ordering::SeqCst), 1);
}
