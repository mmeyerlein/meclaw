//! GH #1106: a `webhook` proxy takes its verification secret from a credential
//! grant (`verify.secret_grant_id`) instead of the environment.
//!
//! Driven through both real halves: the handler asks in `on_start`, opens the
//! sealed box and hands the secret over its reconfigure channel; the very frame
//! it sends is fed to the mount half, which checks the signature with it. Before
//! the box the mount is closed (`503`, `credential_pending`); after it, a
//! correctly signed POST is `202` and a wrong signature still `401` with the
//! `webhook_unverified` refusal -- the boundary is the same, only the source of
//! the secret moved. Stub values only.

// harness_fold: #[path = "support/gh1059_grants.rs"]
#[allow(unused_imports)]
use crate::gh1059_grants; // harness_fold: mod gh1059_grants;

use gh1059_grants::{delivery, foreign_delivery, message};
use hmac::{Hmac, Mac};
use meclaw_cells::proxy::webhook::{
    cell::WebhookCell,
    mount::{HookEvent, HookReconfig, WebhookIo, run_io},
    params::WebhookParams,
    verify::Unverified,
};
use meclaw_colony::{LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Headers, OriginSink, OutputSink, Path, Uuid};
use meclaw_testing::surface_listener::{surface_listener, wait_for_mount};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::mpsc;

const GRANT: &str = "grant:probe-hook@t/hook";
const SECRET: &str = "stub-1106-hook-secret";
const MOUNT: &str = "probe-hook";
const SIG: &str = "X-Probe-Signature";

fn params() -> Value {
    json!({"platform": "webhook", "mount": MOUNT, "route": "probe", "emit_to": "/sink",
        "verify": {"kind": "hmac_sha256", "secret_grant_id": GRANT,
                   "header": SIG, "prefix": "sha256="},
        "headers_allow": ["X-Probe-Event"]})
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

async fn post(addr: std::net::SocketAddr, signature: Option<String>, body: &str) -> u16 {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/{MOUNT}/"))
        .header("content-type", "application/json")
        .header("X-Probe-Event", "push")
        .body(body.to_string());
    if let Some(s) = signature {
        req = req.header(SIG, s);
    }
    req.send()
        .await
        .expect("the mount answers")
        .status()
        .as_u16()
}

async fn next_event(rx: &mut mpsc::Receiver<HookEvent>) -> HookEvent {
    meclaw_testing::deadline::hang("the next mount event", rx.recv())
        .await
        .expect("the event channel is open")
}

/// The mount half of the grant's cell, served on a loopback listener.
async fn mounted(
    p: &WebhookParams,
) -> (
    std::net::SocketAddr,
    mpsc::Receiver<HookEvent>,
    mpsc::Sender<HookReconfig>,
) {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, events_rx) = mpsc::channel(16);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(4);
    tokio::spawn(run_io(
        WebhookIo::new(p, "/hook", Arc::clone(&surfaces)),
        events_tx,
        reconfig_rx,
    ));
    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, _join) = surface_listener(Arc::clone(&surfaces)).await;
    (addr, events_rx, reconfig_tx)
}

/// The handler's `on_start` question: its content, emitted towards `emit_to`.
async fn ask(cell: &mut WebhookCell) -> Value {
    let (tx, mut out) = mpsc::channel(8);
    let sink = OriginSink::new(tx, Path::new("/hook"), 64);
    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.on_start(&sink, &mut db).await;
    let e = meclaw_testing::deadline::hang("the on_start question", out.recv())
        .await
        .expect("a question");
    assert_eq!(
        e.target.as_str(),
        "/sink",
        "towards emit_to; the edge decides"
    );
    e.content
}

/// Hand one message to the handler; return what it emitted and what it sent
/// to the mount half.
async fn hand(cell: &mut WebhookCell, body: Value) -> (Vec<Value>, Vec<HookReconfig>) {
    let (tx, mut out) = mpsc::channel(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/hook"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    let (rc_tx, mut rc_rx) = mpsc::channel(4);
    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.handle(message("/hook", body), &sink, &mut db, &rc_tx)
        .await;
    drop(rc_tx);
    // `handle` awaited every push before it returned: what it emitted is in
    // the channel now, so a zero-wait drain is the whole of it (dev-rules
    // § 11, absence by a zero-wait check after the marker).
    drop(sink);
    let mut emitted = Vec::new();
    while let Ok(e) = out.try_recv() {
        emitted.push(e.content);
    }
    let mut frames = Vec::new();
    while let Some(f) = rc_rx.recv().await {
        frames.push(f);
    }
    (emitted, frames)
}

/// The red line of #1106: signature check with the secret from the grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1106_the_signature_is_checked_with_the_secret_from_the_grant() {
    let p = WebhookParams::parse(&params()).expect("a grant-only mount parses");
    let (addr, mut events, io_tx) = mounted(&p).await;
    let body = r#"{"action":"opened"}"#;

    // Before the box: closed, booked, nothing arrives -- even signed right.
    assert_eq!(
        post(addr, Some(sign(SECRET, body.as_bytes())), body).await,
        503
    );
    assert!(matches!(
        next_event(&mut events).await,
        HookEvent::CredentialPending
    ));

    // The handler asks once, under its grant, and opens the vault's box.
    let mut cell = WebhookCell::new(&p);
    let question = ask(&mut cell).await;
    assert_eq!(question["header"]["route"], "credential_request");
    assert_eq!(question["header"]["grant_id"], GRANT);
    let (emitted, mut frames) = hand(&mut cell, delivery(GRANT, &question, SECRET)).await;
    assert!(emitted.is_empty(), "a box is never answered: {emitted:?}");
    assert_eq!(frames.len(), 1, "exactly one secret goes to the mount half");
    let frame = frames.remove(0);
    assert!(
        !format!("{frame:?}").contains(SECRET),
        "the frame prints <sealed>"
    );
    io_tx.send(frame).await.expect("the mount half listens");

    // After the box: a right signature crosses, a wrong one is the same 401.
    // The mount half takes the frame on its own task and reports nothing back,
    // so this is the condition with a deadline (dev-rules § 11, no hook): post
    // until the mount is open, each closed answer consumed by its own event.
    let status =
        meclaw_testing::deadline::hang("the mount opens with the delivered secret", async {
            loop {
                let status = post(addr, Some(sign(SECRET, body.as_bytes())), body).await;
                if status != 503 {
                    return status;
                }
                assert!(matches!(
                    next_event(&mut events).await,
                    HookEvent::CredentialPending
                ));
                tokio::task::yield_now().await;
            }
        })
        .await;
    assert_eq!(status, 202, "signed with the delivered secret");
    match next_event(&mut events).await {
        HookEvent::Arrived(b) => {
            assert_eq!(b["webhook"]["json"], json!({"action": "opened"}));
            assert_eq!(b["webhook"]["headers"], json!({"x-probe-event": "push"}));
        }
        other => panic!("expected the arrival, got {other:?}"),
    }
    assert_eq!(
        post(addr, Some(sign("stub-wrong-secret", body.as_bytes())), body).await,
        401
    );
    assert!(matches!(
        next_event(&mut events).await,
        HookEvent::Refused(Unverified::Mismatch)
    ));
    assert_eq!(post(addr, None, body).await, 401, "unsigned stays 401");
}

/// A box sealed to a stranger does not open: no frame reaches the mount half,
/// so it stays closed; and a mount without a grant still refuses a "box" like
/// any other message (nothing new is accepted there).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1106_a_box_that_does_not_open_keeps_the_mount_closed() {
    let p = WebhookParams::parse(&params()).expect("params");
    let mut cell = WebhookCell::new(&p);
    let question = ask(&mut cell).await;
    let (emitted, frames) = hand(&mut cell, foreign_delivery(GRANT, &question, SECRET)).await;
    assert!(emitted.is_empty(), "{emitted:?}");
    assert!(frames.is_empty(), "a refused box hands nothing over");

    let plain = WebhookParams::parse(&json!({"platform": "webhook", "mount": MOUNT,
        "route": "probe", "emit_to": "/sink",
        "verify": {"kind": "hmac_sha256", "secret": SECRET, "header": SIG}}))
    .expect("params");
    let mut cell = WebhookCell::new(&plain);
    let (emitted, frames) = hand(&mut cell, delivery(GRANT, &question, SECRET)).await;
    assert!(frames.is_empty());
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert_eq!(emitted[0]["header"]["error_code"], "invalid_input");
    assert!(!emitted[0].to_string().contains(SECRET));
}

/// Both sources at once is a configuration error naming the grant key.
#[test]
fn gh1106_a_grant_beside_a_secret_is_refused() {
    let mut v = params();
    v["verify"]["secret"] = json!(SECRET);
    let e = WebhookParams::parse(&v).expect_err("both");
    assert!(e.starts_with("verify.secret_grant_id"), "{e}");
    assert!(!e.contains(SECRET), "{e}");
}
