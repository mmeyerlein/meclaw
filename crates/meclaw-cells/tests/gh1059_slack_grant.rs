//! GH #1059: the Slack `proxy` takes its two tokens from the vault.
//!
//! A connector with `app_token_grant_id` / `bot_token_grant_id` asks once per
//! grant in `on_start`, opens no Socket Mode connection and calls no Web API
//! until EVERY vaulted token arrived, then connects with the delivered app
//! token (it stands in the `Authorization` header of `apps.connections.open`).
//! A box that does not open leaves it asleep; a lost question is asked again
//! after the wait; a respawn asks again (key pairs and tokens are RAM-only).
//! Stub values only.

#[path = "support/gh1059_grants.rs"]
mod gh1059_grants;

use gh1059_grants::{
    LogCapture, delivery, eventually, foreign_delivery, message, next_request, recipient_of,
    requests_within,
};
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Message, Path};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const APP: &str = "grant:slack_app_token@agent/proxy";
const BOT: &str = "grant:slack_bot_token@agent/proxy";

type Seen = Arc<Mutex<Vec<CapturedRequest>>>;

struct Spawned {
    sender: mpsc::Sender<Message>,
    join: JoinHandle<()>,
    out: mpsc::Receiver<CellEmission>,
    /// The colony inbox the cell was handed — held so the channel stays open.
    _inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
}

/// A Slack Web API stub that answers every call with a transient Slack error
/// (the connector backs off and tries again) and records every request with
/// its headers — the `Authorization` header is the receipt of which token was
/// sent.
async fn slack_stub() -> (String, Seen) {
    let (addr, _h, seen) = start_mock_server_capturing(vec![MockResponse::ok_json(
        br#"{"ok":false,"error":"stub_unavailable"}"#,
    )])
    .await;
    (format!("http://{addr}"), seen)
}

fn params(base_url: &str, extra: Value) -> Value {
    let mut p = json!({
        "platform": "slack",
        "emit_to": "/agent",
        "base_url": base_url,
        "connect_timeout_ms": 2000,
    });
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    p
}

fn spawn(params: Value, dir: &tempfile::TempDir) -> Spawned {
    let cell_dir = dir.path().join("proxy");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (out_tx, out) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, inbox) = mpsc::channel(64);
    let f = Arc::new(ProxyCellFactory::new(Arc::new(
        meclaw_colony::SurfaceRegistry::new(),
    )));
    let spawned = f
        .spawn_cell(
            Path::new("/slack"),
            params,
            out_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    let meclaw_colony::SpawnedCellKind::Active { sender, join, .. } = spawned else {
        unreachable!("a proxy is spawned active");
    };
    Spawned {
        sender,
        join,
        out,
        _inbox: inbox,
    }
}

/// `apps.connections.open` calls that carried `Bearer <token>`.
fn opens_with(seen: &Seen, token: &str) -> usize {
    let bearer = format!("Bearer {token}");
    gh1059_grants::snapshot(seen)
        .iter()
        .filter(|r| r.path.contains("apps.connections.open"))
        .filter(|r| r.headers.get("authorization") == Some(&bearer))
        .count()
}

/// Requests that carry `needle` anywhere (path, any header, body).
fn requests_carrying(seen: &Seen, needle: &str) -> usize {
    gh1059_grants::snapshot(seen)
        .iter()
        .filter(|r| {
            r.path.contains(needle)
                || r.headers.values().any(|v| v.contains(needle))
                || String::from_utf8_lossy(&r.body).contains(needle)
        })
        .count()
}

/// The next `n` credential requests, whatever their grant.
async fn asked(out: &mut mpsc::Receiver<CellEmission>, n: usize) -> Vec<Value> {
    let mut v = Vec::new();
    for _ in 0..n {
        v.push(next_request(out, None).await);
    }
    v
}

/// The request of `grant` among `requests`.
fn of<'a>(requests: &'a [Value], grant: &str) -> &'a Value {
    requests
        .iter()
        .find(|r| r["header"]["grant_id"] == grant)
        .unwrap_or_else(|| panic!("no question for {grant}: {requests:?}"))
}

async fn send(s: &Spawned, body: Value) {
    s.sender.send(message("/slack", body)).await.unwrap();
}

/// An agent answer addressed to a Slack channel.
fn reply() -> Message {
    let mut m = message(
        "/slack",
        json!({"messages": [{"origin": "assistant", "type": "text", "text": "hi"}]}),
    );
    m.headers.context.insert("chat_id".into(), json!("C0STUB"));
    m
}

/// T1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_asks_once_on_start() {
    // Two grants → two questions, one each.
    let dir = tempfile::tempdir().unwrap();
    let (base, _seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT}),
        ),
        &dir,
    );
    let requests = asked(&mut s.out, 2).await;
    assert_eq!(of(&requests, APP)["header"]["grant_id"], APP);
    assert_eq!(of(&requests, BOT)["header"]["grant_id"], BOT);
    let more = requests_within(&mut s.out, Duration::from_millis(500), None).await;
    assert!(more.is_empty(), "exactly one question per grant: {more:?}");

    // One grant (the bot token a literal) → one question.
    let dir = tempfile::tempdir().unwrap();
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token": "stub-literal-1"}),
        ),
        &dir,
    );
    let _ = next_request(&mut s.out, Some(APP)).await;
    let more = requests_within(&mut s.out, Duration::from_millis(500), None).await;
    assert!(more.is_empty(), "no question for a literal token: {more:?}");
}

/// T2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_does_not_poll_or_connect_before_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT,
                   "app_token": "stub-literal-2a", "bot_token": "stub-literal-2b"}),
        ),
        &dir,
    );
    let _ = asked(&mut s.out, 2).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "no request to Slack before the boxes — not even with the ignored literals"
    );
}

/// T3 (and OR-VG-4: a literal next to a grant is never used).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_opens_the_box_and_starts() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT,
                   "app_token": "stub-literal-3a", "bot_token": "stub-literal-3b"}),
        ),
        &dir,
    );
    let requests = asked(&mut s.out, 2).await;
    send(&s, delivery(APP, of(&requests, APP), "stub-secret-3a")).await;
    send(&s, delivery(BOT, of(&requests, BOT), "stub-secret-3b")).await;
    eventually("apps.connections.open with the vaulted app token", || {
        opens_with(&seen, "stub-secret-3a") > 0
    })
    .await;
    assert_eq!(
        requests_carrying(&seen, "stub-literal-3"),
        0,
        "the literals are never used"
    );
    for param in ["app_token", "bot_token"] {
        let line = format!("params.{param} is ignored because params.{param}_grant_id is set");
        assert!(
            !logs.containing(&line).is_empty(),
            "the ignored literal {param} is named once"
        );
    }
}

/// T4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_refuses_on_a_box_that_does_not_open() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT}),
        ),
        &dir,
    );
    let requests = asked(&mut s.out, 2).await;
    // The bot token opens, the app token's box was sealed to a stranger.
    send(&s, delivery(BOT, of(&requests, BOT), "stub-secret-4b")).await;
    send(
        &s,
        foreign_delivery(APP, of(&requests, APP), "stub-secret-4a"),
    )
    .await;
    eventually("the refusal's log line", || {
        !logs
            .containing("proxy: the sealed slack token was refused")
            .is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "a box that did not open starts nothing"
    );
}

/// T5.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_asks_again_after_a_respawn() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _seen) = slack_stub().await;
    let p = params(
        &base,
        json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT}),
    );
    let mut first = spawn(p.clone(), &dir);
    let r1 = asked(&mut first.out, 2).await;
    send(&first, delivery(APP, of(&r1, APP), "stub-secret-5a")).await;
    send(&first, delivery(BOT, of(&r1, BOT), "stub-secret-5b")).await;
    let Spawned { sender, join, .. } = first;
    drop(sender);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .unwrap()
        .unwrap();
    // The same cell directory, a new life: nothing of the tokens survived.
    let mut second = spawn(p, &dir);
    let r2 = asked(&mut second.out, 2).await;
    for grant in [APP, BOT] {
        assert_ne!(
            recipient_of(of(&r1, grant)),
            recipient_of(of(&r2, grant)),
            "a new life, a new recipient ({grant})"
        );
    }
}

/// T6.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_asks_again_when_no_box_arrives() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token": "stub-literal-6b",
                   "credential_wait_ms": 300, "credential_backoff_max_ms": 60_000}),
        ),
        &dir,
    );
    let r1 = next_request(&mut s.out, Some(APP)).await;
    let r2 = next_request(&mut s.out, Some(APP)).await;
    assert_ne!(
        recipient_of(&r1),
        recipient_of(&r2),
        "a new round, a new recipient"
    );
    assert!(
        !logs
            .containing("proxy: no sealed slack token arrived in time")
            .is_empty(),
        "one line per round"
    );
    // The first round's box, late: discarded, the second round goes on.
    send(&s, delivery(APP, &r1, "stub-secret-6-late")).await;
    eventually("the late box is discarded", || {
        !logs.containing("arrived late and was discarded").is_empty()
    })
    .await;
    assert!(gh1059_grants::snapshot(&seen).is_empty());
    send(&s, delivery(APP, &r2, "stub-secret-6")).await;
    eventually(
        "apps.connections.open with the second round's token",
        || opens_with(&seen, "stub-secret-6") > 0,
    )
    .await;
    assert_eq!(requests_carrying(&seen, "stub-secret-6-late"), 0);
}

/// T7: the socket waits for BOTH vaulted tokens — a bot that listens but
/// cannot answer would take events in only to fail them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_holds_two_grants() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT}),
        ),
        &dir,
    );
    let requests = asked(&mut s.out, 2).await;
    send(&s, delivery(APP, of(&requests, APP), "stub-secret-7a")).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "the app token alone opens no connection"
    );
    send(&s, delivery(BOT, of(&requests, BOT), "stub-secret-7b")).await;
    eventually("apps.connections.open once both tokens arrived", || {
        opens_with(&seen, "stub-secret-7a") > 0
    })
    .await;
}

/// Plan V2 § 2.4: an answer before the bot token is refused at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_slack_send_before_the_bot_token_is_credential_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token": "stub-literal-10a", "bot_token_grant_id": BOT}),
        ),
        &dir,
    );
    let _ = next_request(&mut s.out, Some(BOT)).await;
    s.sender.send(reply()).await.unwrap();
    let pending = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let em = s.out.recv().await.expect("the cell is gone");
            if em.content["header"]["error_code"] == "credential_pending" {
                return em.content;
            }
        }
    })
    .await
    .expect("no credential_pending within 30 s");
    assert_eq!(pending["header"]["msg_type"], "proxy_inbound_error");
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "nothing was posted"
    );
}

#[test]
fn gh1059_a_slack_proxy_without_grant_is_refused_by_name() {
    for (missing, other) in [("app_token", "bot_token"), ("bot_token", "app_token")] {
        for literal in [None, Some("")] {
            let mut p = json!({"emit_to": "/x"});
            p[other] = json!("stub-literal-11");
            if let Some(l) = literal {
                p[missing] = json!(l);
            }
            let err = SlackParams::parse(&p).unwrap_err();
            assert!(err.starts_with(&format!("{missing}:")), "{err}");
            assert!(err.contains("name a credential_grant_id"), "{err}");
            assert!(err.contains(&format!("{missing}_grant_id")), "{err}");
        }
    }
    // A grant alone is enough, per token.
    let ok = SlackParams::parse(&json!({
        "emit_to": "/x", "app_token_grant_id": APP, "bot_token_grant_id": BOT
    }))
    .unwrap();
    assert_eq!(ok.app_token_grant_id.as_deref(), Some(APP));
    assert_eq!(ok.bot_token_grant_id.as_deref(), Some(BOT));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_a_literal_key_still_works_for_one_release() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = slack_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"app_token": "stub-literal-8a", "bot_token": "stub-literal-8b"}),
        ),
        &dir,
    );
    eventually("apps.connections.open with the literal app token", || {
        opens_with(&seen, "stub-literal-8a") > 0
    })
    .await;
    let asked = requests_within(&mut s.out, Duration::from_millis(300), None).await;
    assert!(asked.is_empty(), "no grant, no question: {asked:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_no_secret_in_the_log() {
    let logs = LogCapture::global();
    // A closed port: every Web API call fails at `send`, the path where
    // reqwest's error text would carry the request.
    let dir = tempfile::tempdir().unwrap();
    let mut s = spawn(
        params(
            "http://127.0.0.1:1",
            json!({"app_token_grant_id": APP, "bot_token_grant_id": BOT,
                   "app_token": "stub-literal-9a", "bot_token": "stub-literal-9b"}),
        ),
        &dir,
    );
    let requests = asked(&mut s.out, 2).await;
    send(&s, delivery(APP, of(&requests, APP), "stub-secret-9a")).await;
    send(&s, delivery(BOT, of(&requests, BOT), "stub-secret-9b")).await;
    eventually("both boxes opened", || {
        logs.containing("proxy: slack token received sealed and opened in RAM")
            .len()
            >= 2
    })
    .await;
    eventually("a failed connect was logged", || {
        !logs
            .containing("slack: connection failed, backing off")
            .is_empty()
    })
    .await;
    // An answer while the bot token is in RAM, with an unreachable Slack: the
    // post's error path too.
    s.sender.send(reply()).await.unwrap();
    let mut emitted = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
    while let Ok(Some(em)) = tokio::time::timeout_at(deadline, s.out.recv()).await {
        emitted.push(em.content.to_string());
    }
    assert!(
        emitted.iter().any(|e| e.contains("send_failed")),
        "the failed post was reported: {emitted:?}"
    );
    for needle in [
        "stub-secret-9a",
        "stub-secret-9b",
        "stub-literal-9a",
        "stub-literal-9b",
    ] {
        let hits = logs.containing(needle);
        assert!(hits.is_empty(), "{needle} in the log: {hits:?}");
        assert!(
            !emitted.iter().any(|e| e.contains(needle)),
            "{needle} in an emission"
        );
    }
}

/// Review V2 M3 (GH #1061): a credential param of the wrong type is refused by
/// its name instead of falling back to the default.
#[test]
fn gh1061_a_slack_credential_param_of_the_wrong_type_is_refused_by_name() {
    for (key, bad) in [
        ("credential_wait_ms", json!("300")),
        ("credential_backoff_max_ms", json!(1.5)),
        ("app_token_grant_id", json!(["x"])),
        ("bot_token_grant_id", json!(true)),
    ] {
        let mut p = json!({"emit_to": "/x", "app_token_grant_id": APP, "bot_token_grant_id": BOT});
        p[key] = bad;
        let err = match SlackParams::parse(&p) {
            Ok(_) => panic!("{key}: a wrong type must be refused, not defaulted"),
            Err(e) => e,
        };
        assert!(err.starts_with(&format!("{key}:")), "{err}");
    }
}
