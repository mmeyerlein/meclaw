//! Issue #49: the `chat_id` promotion edge, driven through a real colony, once
//! per proxy platform.
//!
//! Every other proxy test either builds the `context` compartment by hand
//! (`MessageBuilder::context`) or calls `handle()` directly, so the mechanism
//! the whole reply leg hangs on — an out-edge with `modifier.set_context`
//! lifting `chat_id` out of the decaying hop — was only pinned generically,
//! without a proxy in the loop. Here a bot loop boots from the filesystem,
//! a platform event arrives at the proxy, the promotion carries the address
//! across the relay, and the answer leaves through the platform client with
//! that address on it.
//!
//! The promotion is not restated in this file. It is READ OUT OF THE SHIPPED
//! TEMPLATE (`builder/templates/bot-basic`, `builder/templates/slack-agent`):
//! delete `set_context` there and the positive arms below lose their promotion
//! and fail.
//!
//! Three shapes are pinned per platform, and the middle one is the one the
//! issue did not expect:
//!
//! 1. **Promotion present** — the reply reaches the platform with the promoted
//!    `chat_id` (`sendMessage.chat_id` / `chat.postMessage.channel+thread_ts`).
//! 2. **Promotion gone, `consumes.context.chat_id` required as shipped** — the
//!    colony does not boot at all. The 14-B locality check walks back from the
//!    proxy over its in-edges looking for a `set_context` setter, and a bot loop
//!    is closed, so there is no ingress-at-birth entry to fall back on. The
//!    reply leg cannot die silently because nothing starts.
//! 3. **Promotion gone AND the contract relaxed to `required: false`** — this is
//!    the silent shape the issue describes, and it needs both mistakes: the
//!    reply reaches the cell, finds no `context.chat_id`, and dies as
//!    `missing_chat_id` at the `reply_to` leg while the bot looks healthy.

use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, RespawnFn, SpawnedCellKind, bootstrap_from_filesystem,
    cell_task,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Cell, CellEmission, CellOutput, JsonValue, Message, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use meclaw_testing::mock_slack::{
    CapturedPost, MockSlack, SlackScript, app_mention, event_callback,
};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The text the relay puts on the wire. The platform payload has to carry it,
/// which proves the answer travelled the loop rather than being echoed by a
/// mock.
const RELAY_ANSWER: &str = "the relay answers";

/// Generic credentials. Never a real token — the mocks accept anything.
const TELEGRAM_BOT_TOKEN: &str = "test-telegram-token";
const SLACK_APP_TOKEN: &str = "xapp-test-promotion";
const SLACK_BOT_TOKEN: &str = "xoxb-test-promotion";

/// Telegram ingress. `chat_id` is a number on this platform.
const TELEGRAM_CHAT_ID: i64 = 4242;
const TELEGRAM_UPDATE_ID: i64 = 71;

/// Slack ingress. A mention in a channel root opens a thread at its own `ts`,
/// so the composite address the proxy emits is `<channel>:<ts>`.
const SLACK_CHANNEL: &str = "C0PROMOTION";
const SLACK_TS: &str = "1700000000.000100";

/// Telegram's answer to `getUpdates` when nothing is waiting.
const EMPTY_UPDATES: &[u8] = br#"{"ok":true,"result":[]}"#;

/// Failure-marker deadline (30 s convention, robust under cargo-parallel load).
const MARKER: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// The relay: the one non-proxy cell in the loop
// ---------------------------------------------------------------------------

/// Stand-in for the persona+worker leg of a bot template.
///
/// A user turn gets an assistant answer; every other input is captured and left
/// unanswered. That asymmetry is load-bearing rather than cosmetic: the proxy's
/// inbound-error shape carries an empty `messages[]` and is addressed at the
/// input's `reply_to`, which the substrate sets to the emitting cell — this one.
/// Answering it would put the same error straight back on the loop.
///
/// An `llm` cell would need a provider key and would cost money per run, and
/// neither is needed to prove routing.
struct ReplyRelayCell {
    /// Emission target. The out-edges override it, but the params still name it.
    emit_to: Path,
    /// Every inbound message, in arrival order — the test's window into the loop.
    seen_tx: mpsc::Sender<Message>,
}

impl Cell for ReplyRelayCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        msg: Message,
        sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let emit_to = self.emit_to.clone();
        let seen_tx = self.seen_tx.clone();
        let sink = sink.clone();
        async move {
            let has_user_turn = match &msg.body {
                Body::Inline(v) => {
                    v.get("messages")
                        .and_then(|m| m.as_array())
                        .is_some_and(|arr| {
                            arr.iter()
                                .any(|t| t.get("origin").and_then(|o| o.as_str()) == Some("user"))
                        })
                }
                _ => false,
            };
            let _ = seen_tx.send(msg).await;
            if !has_user_turn {
                return;
            }
            let _ = sink
                .push(CellOutput {
                    target: emit_to,
                    content: json!({
                        "messages": [
                            { "origin": "assistant", "type": "text", "text": RELAY_ANSWER }
                        ]
                    }),
                })
                .await;
        }
    }
}

/// Factory for [`ReplyRelayCell`], registered under the cell type `reply_relay`
/// so the filesystem bootstrap can spawn it from a `config.json` like any other
/// cell.
struct ReplyRelayFactory {
    /// Handed to every spawned instance (and to every respawn).
    seen_tx: mpsc::Sender<Message>,
}

/// What a (re)spawn hands back: mailbox sender, join handle, peace end,
/// backstop end.
type RelaySpawn = (
    mpsc::Sender<Message>,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
);

impl ReplyRelayFactory {
    /// Shared spawn path for the initial spawn and the `RespawnFn`.
    fn spawn_once(
        &self,
        path: Path,
        emit_to: Path,
        outputs_tx: mpsc::Sender<CellEmission>,
        mailbox_capacity: usize,
    ) -> RelaySpawn {
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
        let cell = ReplyRelayCell {
            emit_to,
            seen_tx: self.seen_tx.clone(),
        };
        let join = tokio::spawn(async move {
            let _peace_keep = peace_tx;
            cell_task(path, rx, outputs_tx, cell, None, None).await;
        });
        (tx, join, peace_rx, backstop_rx)
    }

    /// Shared parse path for `validate_params` and `spawn_cell` (parser
    /// invariant per the `CellFactory` docs).
    fn parse_emit_to(raw: &JsonValue) -> Result<Path, String> {
        raw.get("emit_to")
            .and_then(|v| v.as_str())
            .map(Path::new)
            .ok_or_else(|| "params.emit_to missing or not a string".to_string())
    }
}

impl CellFactory for ReplyRelayFactory {
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        Self::parse_emit_to(raw).map(|_| ())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let emit_to = Self::parse_emit_to(&raw)?;
        let (sender, join, peace_rx, backstop_rx) = self.spawn_once(
            path.clone(),
            emit_to.clone(),
            outputs_tx.clone(),
            mailbox_capacity,
        );
        let factory = self.clone();
        let respawn: RespawnFn = Box::new(move || {
            factory.spawn_once(
                path.clone(),
                emit_to.clone(),
                outputs_tx.clone(),
                mailbox_capacity,
            )
        });
        // Inert placeholder ends: this stateless test cell has no live
        // peace-stop wiring, exactly like `EchoCellFactory`.
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

// ---------------------------------------------------------------------------
// Tree construction — the shipped templates are the source
// ---------------------------------------------------------------------------

/// Where the shipped bot templates live, relative to this crate.
fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builder/templates")
}

/// Reads and parses a JSON file, naming the file in every failure.
fn read_json(path: std::path::PathBuf) -> Value {
    let txt =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&txt)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Writes a JSON file, creating nothing — the caller owns the directories.
fn write_json(path: std::path::PathBuf, value: &Value) {
    let txt = meclaw_core::serde_json::to_string_pretty(value).expect("serialize config");
    std::fs::write(&path, txt).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// The `modifier` the SHIPPED template puts on its proxy out-edge.
///
/// Reading it instead of restating it is the entire point of the pin: this is
/// the one edge the reply leg of a chat bot depends on, and a template that
/// loses it must fail a test rather than a user.
fn shipped_promotion_modifier(template: &str) -> Value {
    let cfg = read_json(templates_root().join(template).join("config.json"));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{template}: the hive must declare params.graph.edges"))
        .clone();
    let out_edge = edges
        .iter()
        .find(|e| e.get("from").and_then(|f| f.as_str()) == Some("./proxy"))
        .unwrap_or_else(|| panic!("{template}: there must be an out-edge from ./proxy"));
    let modifier = out_edge.get("modifier").cloned().unwrap_or_else(|| {
        panic!(
            "{template}: the proxy out-edge MUST carry a modifier — cells never write \
             context themselves, so without it chat_id dies at the next hop"
        )
    });
    assert_eq!(
        modifier
            .get("set_context")
            .and_then(|s| s.get("chat_id"))
            .and_then(|v| v.as_str()),
        Some("hop.chat_id"),
        "{template}: the promotion must lift chat_id out of the hop the proxy just emitted"
    );
    modifier
}

/// The SHIPPED Telegram proxy config with only what a hermetic run must change:
/// the credential, the API base, the emission target and the poll timings.
///
/// `require_chat_id` flips `consumes.context.chat_id.required` — `true` is the
/// shipped value, `false` is the relaxed shape in which a lost promotion reaches
/// the cell at all.
fn telegram_proxy_config(base_url: &str, require_chat_id: bool) -> Value {
    let mut cfg = read_json(templates_root().join("bot-basic/proxy/config.json"));
    cfg["params"]["bot_token"] = json!(TELEGRAM_BOT_TOKEN);
    cfg["params"]["base_url"] = json!(base_url);
    cfg["params"]["emit_to"] = json!("../relay");
    // W7 tripwire: the client deadline stays above the server-side wait.
    cfg["params"]["long_poll_request_secs"] = json!(1);
    cfg["params"]["long_poll_timeout_ms"] = json!(2000);
    cfg["params"]["send_timeout_ms"] = json!(5000);
    // The ${VAR} substitution walks `contract.settings.*.default` too, so an
    // unset variable fails the whole boot even when `params` supplies a literal.
    cfg["contract"]["settings"]["bot_token"]["default"] = json!(TELEGRAM_BOT_TOKEN);
    cfg["contract"]["consumes"]["context"]["chat_id"]["required"] = json!(require_chat_id);
    cfg
}

/// The SHIPPED Slack proxy config, patched the same way. Slack's `chat_id` is
/// the composite string, so the contract type differs — which is exactly why
/// both platforms are pinned instead of one standing in for the other.
fn slack_proxy_config(base_url: &str, require_chat_id: bool) -> Value {
    let mut cfg = read_json(templates_root().join("slack-agent/proxy/config.json"));
    cfg["params"]["app_token"] = json!(SLACK_APP_TOKEN);
    cfg["params"]["bot_token"] = json!(SLACK_BOT_TOKEN);
    cfg["params"]["base_url"] = json!(base_url);
    cfg["params"]["emit_to"] = json!("../relay");
    cfg["params"]["send_timeout_ms"] = json!(5000);
    cfg["contract"]["settings"]["app_token"]["default"] = json!(SLACK_APP_TOKEN);
    cfg["contract"]["settings"]["bot_token"]["default"] = json!(SLACK_BOT_TOKEN);
    cfg["contract"]["consumes"]["context"]["chat_id"]["required"] = json!(require_chat_id);
    cfg
}

/// Writes the bot loop: `proxy → relay → proxy`, plus one edge that leaves the
/// hive.
///
/// That last edge is not decoration. A hive whose edges are all internal is an
/// island and boots INACTIVE (bootstrap A7 activity derivation), so a
/// self-contained loop needs exactly one boundary-crossing edge to come up at
/// all. `/observer` is that anchor and doubles as an independent receipt of the
/// reply leg — the same message the proxy is about to consume, observable
/// outside the loop.
///
/// `promotion` is the modifier for the `proxy → relay` edge; `None` writes the
/// same topology with the promotion removed.
fn write_bot_tree(root: &std::path::Path, proxy_cfg: Value, promotion: Option<Value>) {
    let main = root.join("main");
    std::fs::create_dir_all(main.join("proxy")).expect("proxy dir");
    std::fs::create_dir_all(main.join("relay")).expect("relay dir");

    let mut promotion_edge = json!({ "from": "./proxy", "to": "./relay" });
    if let Some(modifier) = promotion {
        promotion_edge["modifier"] = modifier;
    }
    write_json(
        main.join("config.json"),
        &json!({
            "cell": { "type": "hive" },
            "params": { "graph": { "edges": [
                promotion_edge,
                { "from": "./relay", "to": "./proxy" },
                { "from": "./relay", "to": "/observer" }
            ]}}
        }),
    );
    write_json(main.join("proxy/config.json"), &proxy_cfg);
    write_json(
        main.join("relay/config.json"),
        &json!({
            "cell": { "type": "reply_relay" },
            "params": { "emit_to": "../proxy" },
            "contract": { "version": "1.0.0", "settings": {}, "consumes": {} }
        }),
    );
}

/// A booted bot loop plus the two windows into it.
struct BotRun {
    /// The live colony.
    colony: ColonyHandle,
    /// Every message the relay received, in arrival order.
    relay_rx: mpsc::Receiver<Message>,
    /// Every message that left the hive over the `./relay → /observer` edge.
    observer_rx: mpsc::Receiver<Message>,
    /// Boot outcome, flattened to a string so the failure arms can assert on it.
    boot: Result<(), String>,
}

/// Boots the tree written at `td`, with the `proxy` and `reply_relay` factories
/// registered and `/observer` alive before anything can emit at it
/// (anti-cascade).
async fn boot_bot_tree(td: &TempDir) -> BotRun {
    let (relay_tx, relay_rx) = mpsc::channel::<Message>(16);
    let (observer_tx, observer_rx) = mpsc::channel::<Message>(16);

    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "proxy".to_string(),
            Arc::new(ProxyCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "reply_relay".to_string(),
            Arc::new(ReplyRelayFactory { seen_tx: relay_tx }) as Arc<dyn CellFactory>,
        ),
    ];
    let colony = ColonyHandle::new_with_factories_at(td, factories.clone());
    colony
        .spawn(Path::new("/observer"), move || {
            CaptureCell::new(observer_tx.clone())
        })
        .await;

    let mut registry = CellFactoryRegistry::new();
    for (name, factory) in factories {
        registry.insert(name, factory);
    }
    let boot = bootstrap_from_filesystem(td.path(), &registry, &colony.runtime())
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:?}"));

    BotRun {
        colony,
        relay_rx,
        observer_rx,
        boot,
    }
}

/// Next message on a capture channel, with the 30 s failure marker.
async fn recv(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(MARKER, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} — nothing arrived within 30s"))
        .unwrap_or_else(|| panic!("{what} — the capture channel closed"))
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

/// The mock's capture vector type.
type Captured = Arc<tokio::sync::Mutex<Vec<CapturedRequest>>>;

/// One mock server for both Telegram calls the cell makes.
///
/// The long poll answers the single canned update once and stays empty
/// afterwards (the cell advances its cursor, so a repeat would be a second
/// emission); `sendMessage` always succeeds. Everything is served from the
/// validator hook, which is path-aware — the canned sequence is a formality and
/// is never consumed.
async fn start_mock_telegram() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, Captured) {
    let update = json!({
        "ok": true,
        "result": [{
            "update_id": TELEGRAM_UPDATE_ID,
            "message": {
                "message_id": 9,
                "chat": { "id": TELEGRAM_CHAT_ID },
                "from": { "id": 777 },
                "text": "does the promotion edge work?"
            }
        }]
    })
    .to_string()
    .into_bytes();
    let polls = Arc::new(AtomicUsize::new(0));
    let validator: RequestValidator = Arc::new(move |req: &CapturedRequest| {
        if req.path.contains("/getUpdates") {
            if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                Some(MockResponse::ok_json(&update))
            } else {
                // Keeps the idle lane from spinning against the mock.
                Some(MockResponse::ok_json(EMPTY_UPDATES).with_delay(Duration::from_millis(100)))
            }
        } else if req.path.contains("/sendMessage") {
            Some(MockResponse::ok_json(br#"{"ok":true,"result":{}}"#))
        } else {
            Some(MockResponse::not_found())
        }
    });
    start_mock_server_capturing_with_validator(
        vec![MockResponse::ok_json(EMPTY_UPDATES)],
        Some(validator),
    )
    .await
}

/// Every `sendMessage` body the mock saw so far.
async fn telegram_sends(captured: &Captured) -> Vec<Value> {
    captured
        .lock()
        .await
        .iter()
        .filter(|r| r.method == "POST" && r.path.contains("/sendMessage"))
        .map(|r| {
            meclaw_core::serde_json::from_slice(&r.body)
                .unwrap_or_else(|e| panic!("sendMessage body must be JSON: {e}"))
        })
        .collect()
}

/// Waits for the first `sendMessage` body (30 s failure marker).
async fn await_telegram_send(captured: &Captured) -> Value {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        if let Some(body) = telegram_sends(captured).await.into_iter().next() {
            return body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no sendMessage reached the mock within 30s; paths seen: {:?}",
            captured
                .lock()
                .await
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// THE TELEGRAM PIN. A real colony, the shipped proxy config, the shipped
/// promotion — and a reply that lands in the chat it came from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telegram_promotion_edge_carries_the_reply_back_to_the_right_chat() {
    let (addr, _mock_join, captured) = start_mock_telegram().await;
    let td = TempDir::new().expect("tempdir");
    write_bot_tree(
        td.path(),
        telegram_proxy_config(&format!("http://{addr}"), true),
        Some(shipped_promotion_modifier("bot-basic")),
    );
    let mut run = boot_bot_tree(&td).await;
    assert!(
        run.boot.is_ok(),
        "the bot tree must boot green; got {:?}",
        run.boot.as_ref().err()
    );

    // Mid-loop receipt: the edge modifier put chat_id into the PERSISTENT
    // compartment. The cell emitted it as a hop key, and only an edge can move
    // it here — a hop would already have decayed at the relay's own emission.
    let user_turn = recv(&mut run.relay_rx, "the relay must receive the user turn").await;
    assert_eq!(
        user_turn
            .headers
            .context
            .get("chat_id")
            .and_then(|v| v.as_i64()),
        Some(TELEGRAM_CHAT_ID),
        "context.chat_id must carry the address at the relay; context = {:?}",
        user_turn.headers.context
    );

    // The reply leg, observed outside the loop: the same promoted address rides
    // the relay's answer back towards the proxy.
    let reply = recv(
        &mut run.observer_rx,
        "the relay's answer must leave the hive",
    )
    .await;
    assert_eq!(
        reply
            .headers
            .context
            .get("chat_id")
            .and_then(|v| v.as_i64()),
        Some(TELEGRAM_CHAT_ID),
        "the reply must still carry context.chat_id; context = {:?}",
        reply.headers.context
    );

    // End receipt: the answer left through the Telegram client, addressed.
    let sent = await_telegram_send(&captured).await;
    assert_eq!(
        sent["chat_id"].as_i64(),
        Some(TELEGRAM_CHAT_ID),
        "sendMessage must address the chat the promotion named; body = {sent}"
    );
    assert_eq!(
        sent["text"].as_str(),
        Some(RELAY_ANSWER),
        "the text must be the relay's answer, not a mock echo; body = {sent}"
    );

    let dls = run.colony.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a clean chat round trip must leave the DLQ empty; got {dls:?}"
    );
    run.colony.shutdown().await;
}

/// The silent shape, and what it takes to reach it: promotion gone AND the
/// contract relaxed. Then the reply arrives at the cell with nothing to address
/// and dies as `missing_chat_id` at its `reply_to` leg.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telegram_reply_leg_dies_as_missing_chat_id_without_the_promotion() {
    let (addr, _mock_join, captured) = start_mock_telegram().await;
    let td = TempDir::new().expect("tempdir");
    write_bot_tree(
        td.path(),
        telegram_proxy_config(&format!("http://{addr}"), false),
        None,
    );
    let mut run = boot_bot_tree(&td).await;
    assert!(
        run.boot.is_ok(),
        "a relaxed contract lets the promotion-less tree boot; got {:?}",
        run.boot.as_ref().err()
    );

    // Sanity on the same message the positive arm asserts against: without the
    // modifier the address never leaves the hop, and the hop is gone by now.
    let user_turn = recv(&mut run.relay_rx, "the relay must receive the user turn").await;
    assert!(
        user_turn.headers.context.get("chat_id").is_none(),
        "nothing promoted chat_id, so context must be empty of it; context = {:?}",
        user_turn.headers.context
    );

    // The proxy's error reply comes back at `reply_to`, which the substrate set
    // to the emitting cell — the relay. Positive receipt of the failure class.
    let error = recv(
        &mut run.relay_rx,
        "the proxy's inbound error must come back to the relay",
    )
    .await;
    assert_eq!(
        error.headers.hop.get("error_code").and_then(|v| v.as_str()),
        Some("missing_chat_id"),
        "the reply leg must die as missing_chat_id; hop = {:?}",
        error.headers.hop
    );
    assert_eq!(
        error.headers.hop.get("msg_type").and_then(|v| v.as_str()),
        Some("proxy_inbound_error"),
        "the error reply must announce its shape; hop = {:?}",
        error.headers.hop
    );

    // The error is the ordering anchor: the proxy has decided by the time it
    // emitted, so a send would already be in the capture.
    let sends = telegram_sends(&captured).await;
    assert!(
        sends.is_empty(),
        "nothing may reach the chat when the address is missing; got {sends:?}"
    );

    run.colony.shutdown().await;
}

/// With the contract as shipped, the promotion cannot go missing quietly: the
/// boot-time locality check walks back from the proxy over its in-edges looking
/// for a `set_context` setter, and a bot loop is closed — there is no
/// ingress-at-birth entry to fall back on. The colony refuses to start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_telegram_bot_tree_without_the_promotion_edge_does_not_boot() {
    let td = TempDir::new().expect("tempdir");
    // Unreachable base: the boot fails during planning, before any spawn.
    write_bot_tree(
        td.path(),
        telegram_proxy_config("http://127.0.0.1:1", true),
        None,
    );
    let run = boot_bot_tree(&td).await;
    let BotRun { colony, boot, .. } = run;
    let err = boot.expect_err("a bot loop without the promotion must not boot");
    assert!(
        err.contains("HeaderContractViolation"),
        "the boot must fail as a header-contract violation; got {err}"
    );
    assert!(
        err.contains("chat_id"),
        "the boot error must name the key that is unreachable; got {err}"
    );
    colony.shutdown().await;
}

// ---------------------------------------------------------------------------
// Slack
// ---------------------------------------------------------------------------

/// Waits for the first `chat.postMessage` the fake Slack saw (30 s marker).
async fn await_slack_post(server: &MockSlack) -> CapturedPost {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        if let Some(post) = server.posts().await.into_iter().next() {
            return post;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no chat.postMessage reached the fake Slack within 30s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Registers the one mention the run is about.
async fn script_one_mention(server: &MockSlack) {
    server
        .script_for(
            SLACK_APP_TOKEN,
            SlackScript::new("A_PROMOTION").envelope(
                "env-promotion",
                event_callback(
                    "A_PROMOTION",
                    app_mention(
                        SLACK_CHANNEL,
                        "U_HUMAN",
                        "<@BOT> does the promotion edge work?",
                        SLACK_TS,
                        None,
                    ),
                ),
            ),
        )
        .await;
}

/// THE SLACK PIN. Same substrate mechanism, a composite string address, and a
/// reply that returns to the exact thread the mention opened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slack_promotion_edge_carries_the_reply_back_to_the_right_thread() {
    let server = MockSlack::start().await.expect("fake slack");
    script_one_mention(&server).await;

    let td = TempDir::new().expect("tempdir");
    write_bot_tree(
        td.path(),
        slack_proxy_config(&server.base_url(), true),
        Some(shipped_promotion_modifier("slack-agent")),
    );
    let mut run = boot_bot_tree(&td).await;
    assert!(
        run.boot.is_ok(),
        "the bot tree must boot green; got {:?}",
        run.boot.as_ref().err()
    );

    let composite = format!("{SLACK_CHANNEL}:{SLACK_TS}");
    let user_turn = recv(&mut run.relay_rx, "the relay must receive the user turn").await;
    assert_eq!(
        user_turn
            .headers
            .context
            .get("chat_id")
            .and_then(|v| v.as_str()),
        Some(composite.as_str()),
        "context.chat_id must carry the composite address; context = {:?}",
        user_turn.headers.context
    );

    let reply = recv(
        &mut run.observer_rx,
        "the relay's answer must leave the hive",
    )
    .await;
    assert_eq!(
        reply
            .headers
            .context
            .get("chat_id")
            .and_then(|v| v.as_str()),
        Some(composite.as_str()),
        "the reply must still carry context.chat_id; context = {:?}",
        reply.headers.context
    );

    // The composite is split back into channel + thread on the way out, so the
    // answer lands in the thread the mention opened rather than at the root.
    let post = await_slack_post(&server).await;
    assert_eq!(
        post.channel(),
        Some(SLACK_CHANNEL),
        "the post must address the originating channel; body = {:?}",
        post.body
    );
    assert_eq!(
        post.thread_ts(),
        Some(SLACK_TS),
        "the post must stay in the originating thread; body = {:?}",
        post.body
    );
    assert_eq!(
        post.text(),
        Some(RELAY_ANSWER),
        "the text must be the relay's answer; body = {:?}",
        post.body
    );

    let dls = run.colony.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a clean chat round trip must leave the DLQ empty; got {dls:?}"
    );
    run.colony.shutdown().await;
}

/// Slack's silent shape, same two mistakes as the Telegram one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slack_reply_leg_dies_as_missing_chat_id_without_the_promotion() {
    let server = MockSlack::start().await.expect("fake slack");
    script_one_mention(&server).await;

    let td = TempDir::new().expect("tempdir");
    write_bot_tree(
        td.path(),
        slack_proxy_config(&server.base_url(), false),
        None,
    );
    let mut run = boot_bot_tree(&td).await;
    assert!(
        run.boot.is_ok(),
        "a relaxed contract lets the promotion-less tree boot; got {:?}",
        run.boot.as_ref().err()
    );

    let user_turn = recv(&mut run.relay_rx, "the relay must receive the user turn").await;
    assert!(
        user_turn.headers.context.get("chat_id").is_none(),
        "nothing promoted chat_id, so context must be empty of it; context = {:?}",
        user_turn.headers.context
    );

    let error = recv(
        &mut run.relay_rx,
        "the proxy's inbound error must come back to the relay",
    )
    .await;
    assert_eq!(
        error.headers.hop.get("error_code").and_then(|v| v.as_str()),
        Some("missing_chat_id"),
        "the reply leg must die as missing_chat_id; hop = {:?}",
        error.headers.hop
    );

    let posts = server.posts().await;
    assert!(
        posts.is_empty(),
        "nothing may reach Slack when the address is missing; got {} post(s)",
        posts.len()
    );

    run.colony.shutdown().await;
}

/// The Slack half of the boot gate — the check is substrate-level, so the
/// composite string address is caught by exactly the same rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slack_bot_tree_without_the_promotion_edge_does_not_boot() {
    let td = TempDir::new().expect("tempdir");
    write_bot_tree(
        td.path(),
        slack_proxy_config("http://127.0.0.1:1/api", true),
        None,
    );
    let run = boot_bot_tree(&td).await;
    let BotRun { colony, boot, .. } = run;
    let err = boot.expect_err("a bot loop without the promotion must not boot");
    assert!(
        err.contains("HeaderContractViolation"),
        "the boot must fail as a header-contract violation; got {err}"
    );
    assert!(
        err.contains("chat_id"),
        "the boot error must name the key that is unreachable; got {err}"
    );
    colony.shutdown().await;
}
