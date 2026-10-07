//! GH #1059: the Telegram `proxy` takes its bot token from the vault.
//!
//! A connector with `bot_token_grant_id` asks once in `on_start`, polls
//! nothing until the sealed box opened, then polls with the delivered token
//! (it stands in the URL: `/bot<token>/getUpdates`). A box that does not open
//! leaves it asleep; a lost question is asked again after the wait; a respawn
//! asks again (key pair and token are RAM-only). Stub values only.

#[path = "support/gh1059_grants.rs"]
mod gh1059_grants;

use gh1059_grants::{
    LogCapture, delivery, eventually, foreign_delivery, message, next_request, recipient_of,
    requests_within,
};
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::proxy::params::ProxyParams;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Message, Path};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const GRANT: &str = "grant:telegram_bot_token@agent/proxy";

struct Spawned {
    sender: mpsc::Sender<Message>,
    join: JoinHandle<()>,
    out: mpsc::Receiver<CellEmission>,
    /// The colony inbox the cell was handed — held so the channel stays open.
    _inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
}

/// A Telegram stub that answers every `getUpdates` with an empty batch and
/// records every request (path includes the token).
async fn telegram_stub() -> (String, Arc<Mutex<Vec<CapturedRequest>>>) {
    let (addr, _h, seen) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":[]}"#)])
            .await;
    (format!("http://{addr}"), seen)
}

fn params(base_url: &str, extra: Value) -> Value {
    let mut p = json!({
        "emit_to": "/agent",
        "base_url": base_url,
        "long_poll_request_secs": 1,
        "long_poll_timeout_ms": 2000,
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
            Path::new("/tg"),
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

fn polls_with(seen: &Arc<Mutex<Vec<CapturedRequest>>>, token: &str) -> usize {
    let needle = format!("/bot{token}/getUpdates");
    gh1059_grants::snapshot(seen)
        .iter()
        .filter(|r| r.path.contains(&needle))
        .count()
}

async fn send(s: &Spawned, body: Value) {
    s.sender.send(message("/tg", body)).await.unwrap();
}

/// T1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_telegram_asks_once_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _seen) = telegram_stub().await;
    let mut s = spawn(params(&base, json!({"bot_token_grant_id": GRANT})), &dir);
    let request = next_request(&mut s.out, Some(GRANT)).await;
    assert_eq!(request["header"]["grant_id"], GRANT);
    let more = requests_within(&mut s.out, Duration::from_millis(500), None).await;
    assert!(more.is_empty(), "exactly one question per grant: {more:?}");
}

/// T2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_telegram_does_not_poll_or_connect_before_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = telegram_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"bot_token_grant_id": GRANT, "bot_token": "stub-literal-1"}),
        ),
        &dir,
    );
    let _ = next_request(&mut s.out, Some(GRANT)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "no request to Telegram before the box — not even with the ignored literal"
    );
}

/// T3 (and OR-VG-4: the literal next to a grant is never used).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_telegram_opens_the_box_and_starts() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = telegram_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"bot_token_grant_id": GRANT, "bot_token": "stub-literal-3"}),
        ),
        &dir,
    );
    let request = next_request(&mut s.out, Some(GRANT)).await;
    send(&s, delivery(GRANT, &request, "stub-secret-3")).await;
    eventually("a poll with the vaulted token", || {
        polls_with(&seen, "stub-secret-3") > 0
    })
    .await;
    assert_eq!(
        polls_with(&seen, "stub-literal-3"),
        0,
        "the literal is never used"
    );
    assert!(
        !logs
            .containing("params.bot_token is ignored because params.bot_token_grant_id is set")
            .is_empty(),
        "the ignored literal is named once"
    );
}

/// T4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_telegram_refuses_on_a_box_that_does_not_open() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = telegram_stub().await;
    let mut s = spawn(params(&base, json!({"bot_token_grant_id": GRANT})), &dir);
    let request = next_request(&mut s.out, Some(GRANT)).await;
    send(&s, foreign_delivery(GRANT, &request, "stub-secret-4")).await;
    eventually("the refusal's log line", || {
        !logs
            .containing("proxy: the sealed bot token was refused")
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
async fn gh1059_telegram_asks_again_after_a_respawn() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _seen) = telegram_stub().await;
    let mut first = spawn(params(&base, json!({"bot_token_grant_id": GRANT})), &dir);
    let r1 = next_request(&mut first.out, Some(GRANT)).await;
    send(&first, delivery(GRANT, &r1, "stub-secret-5")).await;
    let Spawned { sender, join, .. } = first;
    drop(sender);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .unwrap()
        .unwrap();
    // The same cell directory, a new life: nothing of the token survived.
    let mut second = spawn(params(&base, json!({"bot_token_grant_id": GRANT})), &dir);
    let r2 = next_request(&mut second.out, Some(GRANT)).await;
    assert_ne!(
        recipient_of(&r1),
        recipient_of(&r2),
        "a new life, a new recipient"
    );
}

/// T6.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_telegram_asks_again_when_no_box_arrives() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = telegram_stub().await;
    let mut s = spawn(
        params(
            &base,
            json!({"bot_token_grant_id": GRANT, "credential_wait_ms": 300,
                   "credential_backoff_max_ms": 60_000}),
        ),
        &dir,
    );
    let r1 = next_request(&mut s.out, Some(GRANT)).await;
    let r2 = next_request(&mut s.out, Some(GRANT)).await;
    assert_ne!(
        recipient_of(&r1),
        recipient_of(&r2),
        "a new round, a new recipient"
    );
    assert!(
        !logs
            .containing("proxy: no sealed bot token arrived in time")
            .is_empty(),
        "one line per round"
    );
    // The first round's box, late: discarded, the second round goes on.
    send(&s, delivery(GRANT, &r1, "stub-secret-6-late")).await;
    eventually("the late box is discarded", || {
        !logs.containing("arrived late and was discarded").is_empty()
    })
    .await;
    assert!(gh1059_grants::snapshot(&seen).is_empty());
    send(&s, delivery(GRANT, &r2, "stub-secret-6")).await;
    eventually("a poll with the second round's token", || {
        polls_with(&seen, "stub-secret-6") > 0
    })
    .await;
    assert_eq!(polls_with(&seen, "stub-secret-6-late"), 0);
}

#[test]
fn gh1059_a_telegram_proxy_without_grant_is_refused_by_name() {
    for p in [
        json!({"emit_to": "/x"}),
        json!({"emit_to": "/x", "bot_token": ""}),
    ] {
        let err = ProxyParams::parse(&p).unwrap_err();
        assert!(err.starts_with("bot_token:"), "{err}");
        assert!(err.contains("name a credential_grant_id"), "{err}");
        assert!(err.contains("bot_token_grant_id"), "{err}");
    }
    // A grant alone is enough.
    let ok = ProxyParams::parse(&json!({"emit_to": "/x", "bot_token_grant_id": GRANT})).unwrap();
    assert_eq!(ok.grant(), Some(GRANT));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_a_literal_key_still_works_for_one_release() {
    let dir = tempfile::tempdir().unwrap();
    let (base, seen) = telegram_stub().await;
    let mut s = spawn(params(&base, json!({"bot_token": "stub-literal-8"})), &dir);
    eventually("a poll with the literal token", || {
        polls_with(&seen, "stub-literal-8") > 0
    })
    .await;
    let asked = requests_within(&mut s.out, Duration::from_millis(300), None).await;
    assert!(asked.is_empty(), "no grant, no question: {asked:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_no_secret_in_the_log() {
    let logs = LogCapture::global();
    // A closed port: every poll fails at `send`, the path where reqwest's
    // error text would carry the URL — and the URL carries the token.
    let dir = tempfile::tempdir().unwrap();
    let mut s = spawn(
        params(
            "http://127.0.0.1:1",
            json!({"bot_token_grant_id": GRANT, "bot_token": "stub-literal-9"}),
        ),
        &dir,
    );
    let request = next_request(&mut s.out, Some(GRANT)).await;
    send(&s, delivery(GRANT, &request, "stub-secret-9")).await;
    eventually("the box opened", || {
        !logs
            .containing("proxy: bot token received sealed and opened in RAM")
            .is_empty()
    })
    .await;
    eventually("a failed poll was logged", || {
        !logs.containing("get_updates transient").is_empty()
    })
    .await;
    // A reply while the token is in RAM, with an unreachable Telegram: the
    // send error path too.
    let reply = message(
        "/tg",
        json!({"messages": [{"origin": "assistant", "type": "text", "text": "hi"}]}),
    );
    let mut reply = reply;
    reply.headers.context.insert("chat_id".into(), json!(42));
    s.sender.send(reply).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    for needle in ["stub-secret-9", "stub-literal-9"] {
        let hits = logs.containing(needle);
        assert!(hits.is_empty(), "{needle} in the log: {hits:?}");
    }
}

/// Review V2 M3 (GH #1061): a credential param of the wrong type is refused by
/// its name, like voice refuses it. A silent fall back to the default cost a
/// respawn with a manifest, because the params are immutable.
#[test]
fn gh1061_a_telegram_credential_param_of_the_wrong_type_is_refused_by_name() {
    for (key, bad) in [
        ("credential_wait_ms", json!("300")),
        ("credential_wait_ms", json!(-1)),
        ("credential_backoff_max_ms", json!("60000")),
        ("bot_token_grant_id", json!(7)),
    ] {
        let mut p = json!({"emit_to": "/x", "bot_token_grant_id": GRANT});
        p[key] = bad;
        let err = match ProxyParams::parse(&p) {
            Ok(_) => panic!("{key}: a wrong type must be refused, not defaulted"),
            Err(e) => e,
        };
        assert!(err.starts_with(&format!("{key}:")), "{err}");
    }
    // `null` is the absent value, as in voice.
    let ok = ProxyParams::parse(
        &json!({"emit_to": "/x", "bot_token_grant_id": GRANT, "credential_wait_ms": null}),
    );
    assert!(ok.is_ok(), "{ok:?}");
}
