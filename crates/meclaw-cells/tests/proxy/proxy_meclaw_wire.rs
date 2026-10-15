//! The trace is carried, the TTL falls by one, and nothing else crosses.
use meclaw_cells::proxy::meclaw::client::PeerClient;
use meclaw_cells::proxy::meclaw::lanes::Refusal;
use meclaw_cells::proxy::meclaw::params::Lane;
use meclaw_cells::proxy::meclaw::wire::{
    PROTOCOL_VERSION, crossed_receipt, message_frame, parse_message_frame, read_receipt,
    refused_receipt,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
fn lane() -> Lane {
    Lane {
        route: "topic".into(),
        fields: vec!["topic".into()],
        context: vec![],
        because: "a subject this side may consider, never who said it".into(),
    }
}
fn msg(ttl: u32) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/friend"))
        .ttl(ttl)
        .body(Body::Inline(json!({"topic": "gardening"})))
        .build()
}
fn inbound() -> Value {
    json!({"v": 1, "type": "message", "lane": "topic",
        "trace_id": Uuid::now_v7().to_string(), "ttl": 5, "context": {}, "body": {}})
}
#[test]
fn the_trace_is_carried_and_the_ttl_falls_by_one() {
    let m = msg(64);
    let f = message_frame(&lane(), &m, json!({"topic": "gardening"}), Map::new())
        .expect("a declared crossing");
    assert_eq!(f["v"], json!(PROTOCOL_VERSION));
    assert_eq!(f["lane"], json!("topic"));
    assert_eq!(f["ttl"], json!(63));
    assert_eq!(f["trace_id"], json!(m.trace_id.to_string()));
    assert_eq!(f["body"], json!({"topic": "gardening"}));
    assert!(
        f.get("hop").is_none(),
        "hop never crosses, in either direction"
    );
    let r = message_frame(&lane(), &msg(0), json!({}), Map::new()).expect_err("no hops left");
    assert_eq!(r.error_code, "ttl_exhausted");
}
#[test]
fn a_foreign_or_missing_protocol_integer_is_refused_by_name() {
    let mut two = inbound();
    two["v"] = json!(2);
    let mut none = inbound();
    none.as_object_mut().expect("object").remove("v");
    for v in [two, none] {
        let r = parse_message_frame(&v).expect_err("this build speaks one version");
        assert_eq!(r.error_code, "protocol_mismatch");
    }
}
#[test]
fn a_frame_without_a_lane_is_not_a_lane_frame() {
    let mut v = inbound();
    v.as_object_mut().expect("object").remove("lane");
    let r = parse_message_frame(&v)
        .expect_err("a crossing without a lane has no contract to be judged by");
    assert_eq!(r.error_code, "invalid_frame");
}
#[test]
fn a_frame_that_names_its_own_sender_is_refused() {
    // R-26-18: identity comes from the edge, never from the body.
    for extra in ["sender", "peer", "from"] {
        let mut v = inbound();
        v[extra] = json!("north");
        let r = parse_message_frame(&v).expect_err("a frame carries a lane and never an address");
        assert_eq!(r.error_code, "invalid_frame", "{extra}");
        assert!(
            r.detail.contains(extra),
            "the refusal names the key: {}",
            r.detail
        );
    }
}
#[test]
fn a_frame_at_zero_parses_so_the_mount_can_say_ttl_exhausted() {
    let mut v = inbound();
    v["ttl"] = json!(0);
    assert_eq!(
        parse_message_frame(&v)
            .expect("the parser does not judge the budget")
            .ttl,
        0
    );
}
#[test]
fn both_receipt_verdicts_are_built_and_read_back() {
    let ok = crossed_receipt("topic", "north", &["topic".to_string()]);
    assert_eq!(ok["result"], json!("crossed"));
    assert_eq!(ok["fields"], json!(["topic"]));
    read_receipt(&ok).expect("a crossing reads back as one");
    let r = Refusal::new("lane_field_denied", "who".to_string());
    let bad = refused_receipt("topic", "south", &r, Some("a subject, never who said it"));
    assert_eq!(bad["boundary"], json!("south"));
    assert_eq!(bad["error_code"], json!("lane_field_denied"));
    let back = read_receipt(&bad).expect_err("a refusal is not a crossing");
    assert_eq!(
        back.error_code, "peer_refused",
        "the writer learns it failed, in this colony's vocabulary"
    );
    assert!(
        back.detail.contains("south")
            && back.detail.contains("lane_field_denied")
            && back.detail.contains("never who said it"),
        "detail: {}",
        back.detail
    );
    assert_eq!(
        read_receipt(&json!({"v": 1, "type": "receipt", "result": "maybe"}))
            .expect_err("two verdicts, and no third")
            .error_code,
        "invalid_frame"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_answers_is_read_and_a_peer_nobody_holds_is_unreachable() {
    let frame = crossed_receipt("topic", "south", &["topic".to_string()]);
    let (addr, join) = meclaw_testing::mock_http::start_mock_server(
        meclaw_testing::mock_http::MockResponse::ok_json(frame.to_string().as_bytes()),
    )
    .await;
    let client = PeerClient::new().expect("the client builds");
    let answer = client
        .post_frame(&format!("http://{addr}/peer/"), &json!({"v": 1}), 5000)
        .await
        .expect("the peer answered");
    read_receipt(&answer).expect("and it answered a crossing");
    join.abort();
    let dead = meclaw_testing::ports::free_port();
    let r = client
        .post_frame(
            &format!("http://127.0.0.1:{dead}/peer/"),
            &json!({"v": 1}),
            5000,
        )
        .await
        .expect_err("nobody is there");
    assert_eq!(r.error_code, "peer_unreachable");
}
#[test]
fn a_protocol_field_that_is_no_integer_is_named_as_such() {
    for bad in [json!("1"), json!(1.5), json!(-1)] {
        let mut v = inbound();
        v["v"] = bad.clone();
        let r = parse_message_frame(&v).expect_err("only an integer is a protocol version");
        assert_eq!(r.error_code, "protocol_mismatch", "{bad}");
        assert!(
            r.detail.contains("not a protocol integer") && !r.detail.contains("without"),
            "{bad}: the detail names the bad value, not a missing one: {}",
            r.detail
        );
    }
}
#[test]
fn a_bare_refusal_reads_back_without_empty_quotes() {
    let r = read_receipt(&json!({"v": 1, "type": "receipt", "result": "refused"}))
        .expect_err("a refusal, however thin, is not a crossing");
    assert_eq!(r.error_code, "peer_refused");
    assert!(
        !r.detail.contains("``") && r.detail.contains("no error_code"),
        "detail: {}",
        r.detail
    );
}
/// Reads one HTTP request (headers plus a `Content-Length` body) so the answer
/// is not a reset.
async fn read_request(stream: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.expect("read");
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + len {
                return;
            }
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_is_unreachable_and_its_target_is_never_contacted() {
    use tokio::io::AsyncWriteExt;
    // The target a 3xx points at: a third party no operator declared. It
    // answers a crossing, so a client that follows would come back with Ok.
    let target = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let target_addr = target.local_addr().expect("addr");
    let (hit_tx, mut hit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let receipt = crossed_receipt("topic", "elsewhere", &["topic".to_string()]).to_string();
    let target_task = tokio::spawn(async move {
        while let Ok((mut s, _)) = target.accept().await {
            let _ = hit_tx.send(());
            read_request(&mut s).await;
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{receipt}",
                receipt.len()
            );
            let _ = s.write_all(answer.as_bytes()).await;
        }
    });
    let redirector = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let redirector_addr = redirector.local_addr().expect("addr");
    let redirector_task = tokio::spawn(async move {
        while let Ok((mut s, _)) = redirector.accept().await {
            read_request(&mut s).await;
            let answer = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_addr}/peer/\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = s.write_all(answer.as_bytes()).await;
        }
    });
    let client = PeerClient::new().expect("the client builds");
    let r = client
        .post_frame(
            &format!("http://{redirector_addr}/peer/"),
            &json!({"v": 1}),
            5000,
        )
        .await
        .expect_err("a 3xx is not a receipt");
    assert_eq!(r.error_code, "peer_unreachable", "detail: {}", r.detail);
    assert!(
        hit_rx.try_recv().is_err(),
        "the redirect target must never see a connection"
    );
    redirector_task.abort();
    target_task.abort();
}
