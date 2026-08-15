//! P12 S19/S21: two Slack bots in one colony — the done criterion.
//!
//! The claim under test is that two `proxy` cells with two different Slack
//! identities coexist without leaking into each other: an event addressed to
//! bot A reaches A's agent tree and nothing else, and A's reply goes out under
//! A's own bot token.
//!
//! Receipts are positive: each assertion names a CaptureCell that RECEIVED the
//! message it should have, and the silence of the other one is checked
//! separately rather than being the whole proof.
//!
//! Note on what makes this honest: the fake keeps its scripts keyed by app
//! token, so neither connection can even observe the other's frames. A fake
//! that broadcast to all sockets would make this test pass no matter what the
//! implementation did.

use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_slack::{MockSlack, SlackScript, app_mention, event_callback};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const APP_A: &str = "xapp-alpha";
const BOT_A: &str = "xoxb-alpha";
const APP_B: &str = "xapp-beta";
const BOT_B: &str = "xoxb-beta";

/// One bot identity: its cell directory, its address in the colony, where it
/// emits, and the two Slack tokens that make it that bot and no other.
struct BotFixture<'a> {
    name: &'a str,
    path: Path,
    emit_to: &'a str,
    app_token: &'a str,
    bot_token: &'a str,
}

/// Spawns one Slack-variant proxy cell for the given identity.
async fn spawn_slack_cell(h: &ColonyHandle, td: &TempDir, bot: BotFixture<'_>, base_url: &str) {
    let BotFixture {
        name,
        path,
        emit_to,
        app_token,
        bot_token,
    } = bot;
    let cell_dir = td.path().join(name);
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    let factory = Arc::new(ProxyCellFactory);
    let spawned = factory
        .spawn_cell(
            path.clone(),
            json!({
                "platform": "slack",
                "app_token": app_token,
                "bot_token": bot_token,
                "emit_to": emit_to,
                "base_url": base_url,
                "connect_timeout_ms": 3000,
                "send_timeout_ms": 3000
            }),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("slack cell spawns");
    h.register_spawned(path, spawned).await;
}

/// Waits for one message on a capture channel.
async fn recv_one(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no message within 30s")
        .expect("capture channel closed")
}

fn user_text(m: &Message) -> String {
    match &m.body {
        meclaw_core::Body::Inline(v) => v
            .get("messages")
            .and_then(|x| x.as_array())
            .and_then(|a| a.first())
            .and_then(|t| t.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_bots_receive_only_their_own_events_and_reply_with_their_own_token() {
    let server = MockSlack::start().await.expect("fake slack");
    // Each identity is scripted separately; neither socket can see the other's.
    server
        .script_for(
            APP_A,
            SlackScript::new("A_ALPHA").envelope(
                "env-for-alpha",
                event_callback(
                    "A_ALPHA",
                    app_mention(
                        "C1",
                        "U_HUMAN",
                        "<@ALPHA> question for alpha",
                        "10.0001",
                        None,
                    ),
                ),
            ),
        )
        .await;
    server
        .script_for(
            APP_B,
            SlackScript::new("A_BETA").envelope(
                "env-for-beta",
                event_callback(
                    "A_BETA",
                    app_mention(
                        "C1",
                        "U_HUMAN",
                        "<@BETA> question for beta",
                        "20.0002",
                        None,
                    ),
                ),
            ),
        )
        .await;

    let h = ColonyHandle::new();
    let (cap_a_tx, mut cap_a_rx) = mpsc::channel::<Message>(8);
    let (cap_b_tx, mut cap_b_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/agent-a"), move || {
        CaptureCell::new(cap_a_tx.clone())
    })
    .await;
    h.spawn(Path::new("/agent-b"), move || {
        CaptureCell::new(cap_b_tx.clone())
    })
    .await;

    // Wire the routing BEFORE the bots connect. A Socket Mode cell starts
    // receiving within milliseconds of spawning, and an emission that arrives
    // before its out-edge exists dead-letters as `no_route` — which is exactly
    // what happened on the first run of this test. In production the colony
    // boots the edge table from config.json before any cell task starts, so
    // wiring first is the faithful order, not a convenience.
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/slack-a"),
        Path::new("/agent-a"),
    )
    .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/slack-b"),
        Path::new("/agent-b"),
    )
    .await;

    let td = TempDir::new().expect("tempdir");
    spawn_slack_cell(
        &h,
        &td,
        BotFixture {
            name: "alpha",
            path: Path::new("/slack-a"),
            emit_to: "/agent-a",
            app_token: APP_A,
            bot_token: BOT_A,
        },
        &server.base_url(),
    )
    .await;
    spawn_slack_cell(
        &h,
        &td,
        BotFixture {
            name: "beta",
            path: Path::new("/slack-b"),
            emit_to: "/agent-b",
            app_token: APP_B,
            bot_token: BOT_B,
        },
        &server.base_url(),
    )
    .await;

    // POSITIVE receipt on both sides: each agent got its own question.
    let a = recv_one(&mut cap_a_rx).await;
    let b = recv_one(&mut cap_b_rx).await;
    assert!(
        user_text(&a).contains("question for alpha"),
        "alpha's agent got: {}",
        user_text(&a)
    );
    assert!(
        user_text(&b).contains("question for beta"),
        "beta's agent got: {}",
        user_text(&b)
    );

    // And neither received a second message — no cross-delivery.
    assert!(
        cap_a_rx.try_recv().is_err(),
        "alpha's agent must not see beta's traffic"
    );
    assert!(
        cap_b_rx.try_recv().is_err(),
        "beta's agent must not see alpha's traffic"
    );

    // Identity on the way out: alpha's reply must carry alpha's bot token.
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("chat_id".into(), json!("C1:10.0001"));
    h.send(
        MessageBuilder::new(Path::new("/slack-a"))
            .context(ctx)
            .body(meclaw_core::Body::Inline(json!({
                "messages": [
                    { "origin": "assistant", "type": "text", "text": "alpha answers" }
                ]
            })))
            .build(),
    )
    .await;

    let mut posted = None;
    for _ in 0..300 {
        let posts = server.posts().await;
        if let Some(p) = posts.first() {
            posted = Some(p.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let post = posted.expect("alpha's reply must reach Slack");
    assert_eq!(post.text(), Some("alpha answers"));
    assert_eq!(post.channel(), Some("C1"));
    assert_eq!(
        post.thread_ts(),
        Some("10.0001"),
        "the reply belongs in the thread the mention opened"
    );
    assert_eq!(
        post.authorization.as_deref(),
        Some(format!("Bearer {BOT_A}").as_str()),
        "alpha must answer as alpha — a leaked beta token here would be a \
         cross-identity bug invisible to every single-bot test"
    );

    h.shutdown().await;
}
