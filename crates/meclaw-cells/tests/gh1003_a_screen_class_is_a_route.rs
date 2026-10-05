//! GH #1003: one picture per member, every screen of a route gets the same bytes,
//! and the app may hear which screens are looking.
//!
//! The owner's rule (R-H4-1): every screen shows the same content; screens differ
//! only in size and orientation, and per screen class in window orientation and
//! menu arrangement (display-hive § 6). The class is the ROUTE a screen opened
//! (`/<mount>/phone`, `/<mount>/tv`); the cell renders one tree per route for all
//! of its screens. What was missing: the cell threw the client's join `params`
//! away, so an app could not know which screens were looking. The client now
//! sends `params._screen` = `{w, h, dpr, orientation, coarse}`; the `viewers`
//! op carries it per row, and with `viewer_events: ["screen"]` the app hears
//! `viewer:screen` on join and on leave.
//!
//! These locks hold function and bytes only, so they stand under the gate's
//! parallel suite; the cost of the fan-out (1 vs. 10 screens) is measured once,
//! alone, by `gh1003_fan_out_cost_is_flat_in_viewers` in `src/web/io.rs`
//! (`#[ignore]`).

#[path = "support/web_fixture.rs"]
mod web_fixture;

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use web_fixture::backlog_lab::{Live, MOUNT, row_of, start, start_routes, token};

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// One screen's socket, joined.
struct Screen {
    ws: Ws,
    /// Its session id, as rows and events name it.
    session: String,
    /// The join reply, verbatim.
    reply: String,
}

/// A phone held upright.
fn phone() -> Value {
    json!({"w": 390, "h": 844, "dpr": 3, "orientation": "portrait", "coarse": true})
}

/// A television on the wall.
fn tv() -> Value {
    json!({"w": 1920, "h": 1080, "dpr": 1, "orientation": "landscape", "coarse": false})
}

/// A desk monitor.
fn monitor() -> Value {
    json!({"w": 2560, "h": 1440, "dpr": 1.5, "orientation": "landscape", "coarse": false})
}

/// Join `url` like the client does, with `params` as the join params (`None`
/// is a client that sends none, as before the key existed).
async fn open(port: u16, url: &str, params: Option<Value>) -> Screen {
    let token = token(port).await;
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let (mut ws, _) = tokio_tungstenite::client_async(
        format!("ws://127.0.0.1:{port}/{MOUNT}/live/websocket"),
        stream,
    )
    .await
    .expect("handshake");
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let mut payload = json!({"session": token, "url": url});
    if let Some(p) = params {
        payload["params"] = p;
    }
    ws.send(WsMessage::Text(
        json!(["1", "1", topic, "phx_join", payload])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let reply = next_text(&mut ws).await;
    let parsed: Value = meclaw_core::serde_json::from_str(&reply).expect("reply JSON");
    assert_eq!(parsed[3], json!("phx_reply"), "a reply: {reply}");
    assert_eq!(
        parsed[4]["status"],
        json!("ok"),
        "the join is accepted: {reply}"
    );
    Screen {
        ws,
        session: token.split('.').next().expect("nonce").to_string(),
        reply,
    }
}

/// The next text frame, within the failure-marker timeout.
async fn next_text(ws: &mut Ws) -> String {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("a frame within 30 s")
            .expect("open")
            .expect("frame");
        if let WsMessage::Text(t) = m {
            return t.to_string();
        }
    }
}

/// Wait until `live` has heard `n` `viewer:screen` events (failure marker 30 s).
async fn screens_heard(live: &Live, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let heard = live.screens().await;
        if heard.len() >= n || Instant::now() >= deadline {
            return heard;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until the `viewers` op lists `n` rows (failure marker 30 s).
async fn rows(live: &mut Live, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let rows = live.viewers().await;
        if rows.len() == n || Instant::now() >= deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The cell's params with `viewer:screen` on.
fn opted_in() -> Value {
    json!({"mount": MOUNT, "viewer_events": ["screen"]})
}

/// T3: a diff frame is the frame `frames::push` builds from its own payload —
/// byte for byte, so the display cannot tell the once-encoded fan-out from the
/// one before (the unit half: `push_raw_is_push_byte_for_byte`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_frames_are_byte_identical_to_before() {
    let mut live = start(json!({"mount": MOUNT})).await;
    let mut s = open(live.port, "/", None).await;
    live.pass(1, 8).await;
    let text = next_text(&mut s.ws).await;
    let frame: Value = meclaw_core::serde_json::from_str(&text).expect("frame JSON");
    assert_eq!(frame[3], json!("diff"), "a diff: {text}");
    let topic = frame[2].as_str().expect("topic").to_string();
    assert_eq!(
        text,
        meclaw_surface::frames::push(&frame[0], &topic, "diff", frame[4].clone()),
        "the frame is the one `frames::push` built"
    );
    live.join.abort();
}

/// T4: a phone joins `/phone` with its `_screen` → exactly one `viewer:screen`
/// with the route and the measured values; when it goes, one with
/// `joined: false`. The `viewers` op carries the same `screen` in its row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_a_join_reports_its_screen_class() {
    let mut live = start_routes(opted_in(), &["/", "/phone"]).await;
    let s = open(live.port, "/phone", Some(json!({"_screen": phone()}))).await;
    let heard = screens_heard(&live, 1).await;
    assert_eq!(
        heard,
        vec![
            json!({"session_id": s.session, "route": "/phone", "screen": phone(), "joined": true})
        ],
        "one report of the join"
    );
    let rows = rows(&mut live, 1).await;
    let row = row_of(&rows, &s.session).expect("the phone's row");
    assert_eq!(row["route"], json!("/phone"));
    assert_eq!(row["screen"], phone(), "the op carries the screen");

    drop(s.ws);
    let heard = screens_heard(&live, 2).await;
    assert_eq!(heard.len(), 2, "a join and a leave: {heard:?}");
    assert_eq!(
        heard[1],
        json!({"session_id": s.session, "route": "/phone", "screen": phone(), "joined": false}),
        "and the leave names the same screen"
    );
    live.join.abort();
}

/// T5: three screens of one route with different `_screen` — a television in
/// landscape, a monitor, a phone upright — are sent the same bytes: the join
/// reply and every diff. Content never depends on the screen (R-H4-1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_every_screen_of_a_route_gets_the_same_bytes() {
    let mut live = start(opted_in()).await;
    let mut screens = Vec::new();
    for screen in [tv(), monitor(), phone()] {
        screens.push(open(live.port, "/", Some(json!({"_screen": screen}))).await);
    }
    let _ = rows(&mut live, 3).await;
    for round in 1..=5 {
        live.pass(round, 16).await;
    }
    let mut seen: Vec<Vec<String>> = Vec::new();
    for s in &mut screens {
        let mut frames = vec![s.reply.clone()];
        for _ in 0..5 {
            frames.push(next_text(&mut s.ws).await);
        }
        seen.push(frames);
    }
    assert!(
        seen[0][1..].iter().all(|f| f.contains("\"diff\"")),
        "five diffs: {:?}",
        seen[0]
    );
    assert_eq!(seen[0], seen[1], "the television and the monitor");
    assert_eq!(seen[0], seen[2], "the television and the phone");
    live.join.abort();
}

/// T6: without `viewer_events` no `viewer:screen` is emitted and the log grows
/// by nothing — the display behaves byte for byte as before. The op still
/// carries the screen: it reads, it does not report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_no_screen_event_without_opt_in() {
    let mut live = start_routes(json!({"mount": MOUNT}), &["/", "/phone"]).await;
    let s = open(live.port, "/phone", Some(json!({"_screen": phone()}))).await;
    let rows_now = rows(&mut live, 1).await;
    assert_eq!(
        row_of(&rows_now, &s.session).expect("row")["screen"],
        phone()
    );
    drop(s.ws);
    let gone = rows(&mut live, 0).await;
    assert!(gone.is_empty(), "the phone left: {gone:?}");
    // One more answered call, so a leave the socket task sent after the
    // registry let go has been read by the handler before the count.
    let _ = live.viewers().await;
    assert!(live.screens().await.is_empty(), "no screen report");
    assert_eq!(
        live.emitted.load(Ordering::Relaxed),
        0,
        "no emission besides the replies"
    );
    live.join.abort();
}

/// T7: a join without `_screen` — an older client — and joins with junk in it
/// still join; their screen is `null`, in the op and in the report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_a_join_without_screen_params_still_joins() {
    let mut live = start(opted_in()).await;
    let junk = [
        None,
        Some(json!({"_csrf_token": "x"})),
        Some(json!({"_screen": "tall"})),
        Some(
            json!({"_screen": {"w": -1, "h": 800, "dpr": 1, "orientation": "portrait", "coarse": true}}),
        ),
        Some(
            json!({"_screen": {"w": 390, "h": 844, "dpr": 3, "orientation": "sideways", "coarse": true}}),
        ),
        Some(
            json!({"_screen": {"w": 390, "h": 844, "dpr": 0, "orientation": "portrait", "coarse": true}}),
        ),
        Some(json!({"_screen": {"w": 390, "h": 844, "dpr": 3, "orientation": "portrait"}})),
    ];
    let mut joined = Vec::new();
    for params in junk.iter().cloned() {
        joined.push(open(live.port, "/", params).await);
    }
    let rows = rows(&mut live, junk.len()).await;
    assert_eq!(rows.len(), junk.len(), "every one joined: {rows:?}");
    for s in &joined {
        let row = row_of(&rows, &s.session).expect("row");
        assert!(row["screen"].is_null(), "no screen: {row}");
    }
    let heard = screens_heard(&live, junk.len()).await;
    assert_eq!(heard.len(), junk.len(), "one report per join: {heard:?}");
    assert!(
        heard
            .iter()
            .all(|h| h["screen"].is_null() && h["joined"] == json!(true)),
        "{heard:?}"
    );
    live.join.abort();
}

/// OR-H4.W3.4 (W3 review M1): a second `phx_join` on the same socket is a new
/// join of that socket — the app hears the first join, its leave and the new
/// join, in that order (`true, false, true`), and the last one names the new
/// route and screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1003_a_second_join_on_one_socket_is_a_leave_and_a_join() {
    let live = start_routes(opted_in(), &["/", "/phone"]).await;
    let mut s = open(live.port, "/phone", Some(json!({"_screen": phone()}))).await;
    assert_eq!(screens_heard(&live, 1).await.len(), 1, "the first join");
    let token = token(live.port).await;
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    s.ws.send(WsMessage::Text(
        json!(["2", "2", topic, "phx_join", {"session": token, "url": "/", "params": {"_screen": tv()}}])
            .to_string()
            .into(),
    ))
    .await
    .expect("second join");
    let reply = next_text(&mut s.ws).await;
    let parsed: Value = meclaw_core::serde_json::from_str(&reply).expect("reply JSON");
    assert_eq!(parsed[3], json!("phx_reply"), "a reply: {reply}");
    let heard = screens_heard(&live, 3).await;
    let joined: Vec<&Value> = heard.iter().map(|h| &h["joined"]).collect();
    assert_eq!(
        joined,
        [&json!(true), &json!(false), &json!(true)],
        "join, leave, join: {heard:?}"
    );
    assert_eq!(heard[2]["route"], json!("/"), "{heard:?}");
    assert_eq!(heard[2]["screen"], tv(), "{heard:?}");
    live.join.abort();
}
