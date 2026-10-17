//! GH #1105: the `meclaw` peer proxy takes its OAuth client secret from the
//! vault.
//!
//! A peer proxy with `auth.client_secret_grant_id` asks once in `on_start`.
//! Until the sealed box opened, a message is booked and deferred as
//! `credential_pending`, and the token endpoint sees no request at all; once
//! the box opened, the waiting message leaves at once, with a token fetched
//! by the delivered secret. A `client_secret` next to the grant is a
//! configuration error naming both keys. Stub values only; the measurement is
//! at the receiving end (the token endpoint's form body, the peer's POST).

// harness_fold: #[path = "support/gh1059_grants.rs"]
#[allow(unused_imports)]
use crate::gh1059_grants; // harness_fold: mod gh1059_grants;

use gh1059_grants::{delivery, is_request, message, next_request};
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const GRANT: &str = "grant:peer_client_secret@agent/peer";
const SECRET: &str = "stub-secret-1105";

type Seen = Arc<Mutex<Vec<CapturedRequest>>>;

struct Spawned {
    sender: mpsc::Sender<Message>,
    _join: JoinHandle<()>,
    out: mpsc::Receiver<CellEmission>,
    /// The colony inbox the cell was handed — held so the channel stays open.
    _inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
}

/// The token endpoint (one token, five minutes) and the peer (a `crossed`
/// receipt), each recording every request.
async fn endpoints() -> ((String, Seen), (String, Seen)) {
    let token = json!({"access_token": "tok-1105", "token_type": "Bearer", "expires_in": 300});
    let (t_addr, _t, t_seen) =
        start_mock_server_capturing(vec![MockResponse::ok_json(token.to_string().as_bytes())])
            .await;
    let receipt = meclaw_cells::proxy::meclaw::wire::crossed_receipt("proposal", "south", &[]);
    let (p_addr, _p, p_seen) =
        start_mock_server_capturing(vec![MockResponse::ok_json(receipt.to_string().as_bytes())])
            .await;
    (
        (format!("http://{t_addr}/token"), t_seen),
        (format!("http://{p_addr}"), p_seen),
    )
}

fn params(token_url: &str, peer: &str, auth_extra: Value) -> Value {
    let mut auth = json!({"token_url": token_url, "client_id": "north"});
    for (k, v) in auth_extra.as_object().unwrap() {
        auth[k] = v.clone();
    }
    json!({"platform": "meclaw", "mount": "peer-1105", "identity_header": "X-Meclaw-Peer",
        "boundary": "north", "emit_to": "/agent", "external_timeout_ms": 5000,
        "egress": [peer],
        "lanes": {"accepts": [],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]},
        "auth": auth})
}

fn spawn(params: Value, dir: &tempfile::TempDir) -> Result<Spawned, String> {
    let cell_dir = dir.path().join("peer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (out_tx, out) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, inbox) = mpsc::channel(64);
    let f = Arc::new(ProxyCellFactory::new(Arc::new(
        meclaw_colony::SurfaceRegistry::new(),
    )));
    let spawned = f.spawn_cell(
        Path::new("/peer"),
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
    )?;
    let meclaw_colony::SpawnedCellKind::Active { sender, join, .. } = spawned else {
        unreachable!("a proxy is spawned active");
    };
    Ok(Spawned {
        sender,
        _join: join,
        out,
        _inbox: inbox,
    })
}

/// One outgoing message on the `proposal` lane to `peer`.
fn proposal(peer: &str) -> Message {
    let mut hop = serde_json::Map::new();
    hop.insert("route".into(), json!("proposal"));
    hop.insert("peer".into(), json!("south"));
    hop.insert("peer_url".into(), json!(peer));
    MessageBuilder::new(Path::new("/peer"))
        .ttl(5)
        .hop(hop)
        .body(Body::Inline(json!({"proposal": "a walk"})))
        .build()
}

/// The next peer receipt (`hop.route == "peer_receipt"`-style header with a
/// `peer_event`) among the emissions; requests are skipped.
async fn next_receipt(rx: &mut mpsc::Receiver<CellEmission>) -> Value {
    meclaw_testing::deadline::hang("the next peer receipt", async {
        loop {
            let em = rx.recv().await.expect("the cell is gone");
            if em.content["header"].get("peer_event").is_some() {
                return em.content["header"].clone();
            }
        }
    })
    .await
}

fn bodies(seen: &Seen) -> Vec<String> {
    gh1059_grants::snapshot(seen)
        .iter()
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect()
}

/// Every `credential_request` emitted before the next peer receipt, and that
/// receipt. The receipt is the marker of absence (dev-rules § 11): it comes
/// from `handle`, after `on_start` has emitted all its questions, and a re-ask
/// only comes from an outbox pass over a row already booked.
async fn requests_before_receipt(rx: &mut mpsc::Receiver<CellEmission>) -> (Vec<Value>, Value) {
    meclaw_testing::deadline::hang("the peer receipt after the questions", async {
        let mut requests = Vec::new();
        loop {
            let em = rx.recv().await.expect("the cell is gone");
            if em.content["header"].get("peer_event").is_some() {
                return (requests, em.content["header"].clone());
            }
            if is_request(&em.content, None) {
                requests.push(em.content);
            }
        }
    })
    .await
}

/// T1: exactly one question per grant on start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1105_the_peer_proxy_asks_once_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let ((token_url, _t), (peer, _p)) = endpoints().await;
    let mut s = spawn(
        params(&token_url, &peer, json!({"client_secret_grant_id": GRANT})),
        &dir,
    )
    .expect("spawns");
    let request = next_request(&mut s.out, Some(GRANT)).await;
    assert_eq!(request["header"]["grant_id"], GRANT);
    s.sender.send(proposal(&peer)).await.unwrap();
    let (more, receipt) = requests_before_receipt(&mut s.out).await;
    assert_eq!(receipt["error_code"], "credential_pending", "{receipt}");
    assert!(more.is_empty(), "exactly one question per grant: {more:?}");
}

/// T2: before the box a message is deferred `credential_pending` and nobody
/// is asked for a token; the box releases it at once, with a token fetched by
/// the delivered secret.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1105_the_client_secret_comes_from_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ((token_url, token_seen), (peer, peer_seen)) = endpoints().await;
    let mut s = spawn(
        params(&token_url, &peer, json!({"client_secret_grant_id": GRANT})),
        &dir,
    )
    .expect("spawns");
    let request = next_request(&mut s.out, Some(GRANT)).await;
    s.sender.send(proposal(&peer)).await.unwrap();
    let r = next_receipt(&mut s.out).await;
    assert_eq!(r["peer_event"], "deferred", "{r}");
    assert_eq!(r["error_code"], "credential_pending", "{r}");
    assert!(
        gh1059_grants::snapshot(&token_seen).is_empty()
            && gh1059_grants::snapshot(&peer_seen).is_empty(),
        "no token request and no POST before the box"
    );
    s.sender
        .send(message("/peer", delivery(GRANT, &request, SECRET)))
        .await
        .unwrap();
    let r = next_receipt(&mut s.out).await;
    assert_eq!(r["peer_event"], "crossed", "the waiting message left: {r}");
    let token_bodies = bodies(&token_seen);
    assert!(
        token_bodies
            .iter()
            .any(|b| b.contains(&format!("client_secret={SECRET}"))),
        "the token was fetched with the delivered secret: {token_bodies:?}"
    );
    // The `crossed` receipt is the marker: the stub records a request before
    // it answers (mock_http), so the POST is already captured here.
    let posts = gh1059_grants::snapshot(&peer_seen);
    assert!(
        posts.iter().any(|r| r
            .headers
            .get("authorization")
            .is_some_and(|v| v == "Bearer tok-1105")),
        "the POST carried the bearer: {posts:?}"
    );
}

/// T3: a `client_secret` next to its grant is a configuration error naming
/// both keys, never the value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1105_secret_and_grant_together_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let ((token_url, _t), (peer, _p)) = endpoints().await;
    let e = match spawn(
        params(
            &token_url,
            &peer,
            json!({"client_secret_grant_id": GRANT, "client_secret": "stub-literal-1105"}),
        ),
        &dir,
    ) {
        Ok(_) => panic!("both sources at once must be refused"),
        Err(e) => e,
    };
    assert!(
        e.contains("client_secret") && e.contains("client_secret_grant_id"),
        "{e}"
    );
    assert!(!e.contains("stub-literal-1105"), "{e}");
}
