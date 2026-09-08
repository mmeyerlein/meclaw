//! GH #620 (a) — **a media message names its call.**
//!
//! `context.session_id` had two owners. This cell selects a connection by it,
//! and a member's `session-keeper` mints one of its own on the same key for
//! every turn that passes it — so an answer came back carrying the keeper's
//! generation, no connection held it, and the caller heard nothing while the
//! cell answered `unknown_session` (GH #603 § 3). The tree patched it with a
//! second context key and both READMEs called that a workaround waiting for a
//! ruling.
//!
//! The ruling is R-0908-6: **the call key belongs to the channel.** Every
//! emission of this cell names its call on `hop.call_id`, and an `in_speak`
//! picks the connection by `context.call_id`. Nothing about `context.session_id`
//! moves — its owner is unchanged and it is still read where the call key is
//! absent, so a colony wired against `voice@1.3.0` keeps working.
//!
//! Every assertion here is a POSITIVE receipt: a frame arrived at one client,
//! an emission reached the door, a header carries a value. The one silence —
//! the connection that must NOT be spoken into — carries its control in the
//! same measurement: the other client's frame proves the order was carried out
//! at all.
//!
//! The provider is `echo` and there is no synthesis provider, so this file
//! spends nothing at a vendor. A `Speak` with no TTS configured ends at once as
//! `speak_end` / `failed` — which is exactly the receipt this file needs: it
//! names the connection the order was delivered to.

use meclaw_cells::voice::VoiceCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::free_port;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo: generous, so a loaded runner
/// cannot turn a slow answer into a red test.
const DEADLINE: Duration = Duration::from_secs(30);

/// The semantic window for "and the other connection heard nothing". Short on
/// purpose: it is paid on every green run, and its control (the frame the other
/// client DID get) has already arrived by the time it starts.
const QUIET: Duration = Duration::from_millis(750);

/// Context key that opens the colony's egress door for this file's listener.
const MARK: &str = "voice_out";

struct Fixture {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    egress: mpsc::Receiver<Message>,
    port: u16,
}

impl Fixture {
    /// A colony whose only cell is a `voice` cell on the echo provider, with
    /// the `speak_end` lane on so that a delivered order leaves a receipt.
    ///
    /// The contract is the SHIPPED shape of `voice@1.4.0`: both context keys
    /// optional. Required, the substrate would refuse a message that names only
    /// the call before the cell ever saw it, and the precedence below could not
    /// be measured.
    async fn boot() -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path().join("main");
        std::fs::create_dir_all(root.join("voice")).expect("create the cell dir");
        let mut set_context = meclaw_core::serde_json::Map::new();
        set_context.insert(MARK.to_string(), json!("'1'"));
        std::fs::write(
            root.join("config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "hive"},
                "params": {"graph": {"edges": [
                    {"from": "./voice", "to": ".", "modifier": {"set_context": set_context}}
                ]}}
            }))
            .expect("serialise the hive"),
        )
        .expect("write the root hive");

        let port = free_port();
        std::fs::write(
            root.join("voice/config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "voice", "timeout": -1},
                "params": {
                    "port": port,
                    "bind": "127.0.0.1",
                    "emit_speak_end": true,
                    "stt": {"provider": "echo"}
                },
                "contract": {
                    "version": "1.1.0",
                    "settings": {},
                    // `session_id` alone: `ingress.context` is the standard
                    // header convention and its list is closed (`turn_id`,
                    // `session_id`, `user_id`, `chat_id`, `locale`, GH #185).
                    // `call_id` is not a standard header, and it does not need
                    // to be one — it reaches context through the channel's own
                    // ingress edge, which is what the list's own refusal says.
                    "ingress": {"context": ["session_id"]},
                    "emits": {"body": {"messages": {"type": "array", "required": true}}},
                    "consumes": {
                        "body": {"messages": {"type": "array", "required": true}},
                        "context": {
                            "call_id": {"type": "string", "required": false},
                            "session_id": {"type": "string", "required": false}
                        }
                    }
                }
            }))
            .expect("serialise the cell"),
        )
        .expect("write the voice cell");

        let factories: Vec<(String, Arc<dyn CellFactory>)> =
            vec![("voice".to_string(), Arc::new(VoiceCellFactory))];
        let mut registry = CellFactoryRegistry::new();
        registry.insert(
            "voice".into(),
            Arc::new(VoiceCellFactory) as Arc<dyn CellFactory>,
        );
        let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories, MARK);
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .expect("the colony boots with a voice cell");

        Self {
            _td: td,
            h,
            egress,
            port,
        }
    }

    /// Connect, retrying while the listener is still coming up: the I/O half
    /// binds when the cell's task starts, so the first attempt can lose that
    /// race and get `ConnectionRefused` — the absence of an answer, not one.
    async fn connect(&self, session: &str) -> (VoiceClient, Value) {
        let url = format!("ws://127.0.0.1:{}/ws?session={session}", self.port);
        let deadline = Instant::now() + DEADLINE;
        loop {
            match VoiceClient::connect(&url).await {
                Ok(pair) => return pair,
                Err(e) if Instant::now() >= deadline => {
                    panic!("the voice cell never accepted a connection on {url}: {e}")
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    /// Hand the cell an assistant turn to speak, addressed by whatever context
    /// the caller passes — which is the whole subject of this file.
    async fn speak(&self, context: Value, text: &str) {
        let context = context.as_object().expect("an object").clone();
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".into(), json!("in_speak"));
        self.h
            .send(
                MessageBuilder::new(Path::new("/voice"))
                    .context(context)
                    .hop(hop)
                    .body(Body::Inline(json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": text}]
                    })))
                    .build(),
            )
            .await;
    }

    /// Wait until one emission satisfies `want`, then return everything that
    /// arrived. The deadline is the repo's 30-second failure marker, never a
    /// guess at how long a colony takes: a fixed short cap is a flake on a
    /// loaded runner and tells a reader nothing about what was expected.
    async fn emissions_until(
        &mut self,
        what: &str,
        want: impl Fn(&Message) -> bool,
    ) -> Vec<Message> {
        let deadline = Instant::now() + DEADLINE;
        let mut out = Vec::new();
        loop {
            if out.iter().any(&want) {
                return out;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!(
                    "no emission was {what} within {DEADLINE:?}; saw {:?}",
                    out.iter()
                        .map(|m| (hop_of(m, "route"), hop_of(m, "error_code")))
                        .collect::<Vec<_>>()
                );
            }
            match tokio::time::timeout(left, self.egress.recv()).await {
                Ok(Some(m)) => out.push(m),
                Ok(None) => panic!("the egress channel closed while waiting for {what}"),
                Err(_) => {}
            }
        }
    }

    async fn shutdown(self) {
        self.h.shutdown().await;
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The connection that was spoken into answers with the pair every `Speak`
/// produces. With no synthesis provider that pair is a `speak_end` carrying
/// `failed`, which is a receipt of DELIVERY and nothing else — and delivery is
/// what this file measures.
async fn heard_the_order(client: &mut VoiceClient) -> Value {
    client
        .next_frame_of_type("speak_end", DEADLINE)
        .await
        .expect("the addressed connection is told what happened to the order")
}

/// **The call is the key, and the keeper's session is not.**
///
/// One message carries both, and they name two different live connections. The
/// defect this pins: with `context.session_id` as the only selector, the answer
/// went wherever a member's `session-keeper` had last stamped — which is never
/// the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_speak_prefers_the_call_over_the_keepers_session() {
    let fx = Fixture::boot().await;
    let (mut call_a, hello_a) = fx.connect("call-a").await;
    let (mut call_b, _hello_b) = fx.connect("call-b").await;

    assert_eq!(
        hello_a["call_id"], "call-a",
        "the first frame names the call this connection is: {hello_a}"
    );
    assert_eq!(
        hello_a["session_id"], "call-a",
        "both names, one value — a connection IS the session: {hello_a}"
    );

    fx.speak(
        json!({"call_id": "call-a", "session_id": "call-b"}),
        "this belongs to the first call",
    )
    .await;

    let heard = heard_the_order(&mut call_a).await;
    assert_eq!(
        heard["reason"], "failed",
        "the order was delivered and ended where an unconfigured synthesis ends: {heard}"
    );
    let quiet = call_b.drain_for(QUIET).await;
    assert!(
        quiet.iter().all(|(_, f)| !f.is_type("speak_end")),
        "the connection the keeper's generation names is NOT the one spoken into: {quiet:?}"
    );

    fx.shutdown().await;
}

/// **The old key is still read, and its owner has not changed** (R-G2).
///
/// A colony wired against `voice@1.3.0` promotes `context.session_id` and
/// nothing else. It keeps working: the call key is ADDED beside the session
/// key, never in place of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_speak_still_reads_the_session_key() {
    let fx = Fixture::boot().await;
    let (mut only_one, _hello) = fx.connect("older-wiring").await;

    fx.speak(
        json!({"session_id": "older-wiring"}),
        "the old wiring still speaks",
    )
    .await;

    let heard = heard_the_order(&mut only_one).await;
    assert!(
        heard["speak_id"].is_string(),
        "the connection named by the old key is the one that was spoken into: {heard}"
    );

    fx.shutdown().await;
}

/// **Every emission of the media half names its call.**
///
/// Two of the four lanes are reachable without a provider: `speak_end` and
/// `error`. The other two — `turn` and `partial` — are pinned one level down,
/// on the builder itself, in `voice::cell`'s own unit tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_speak_end_and_error_lanes_name_the_call() {
    let mut fx = Fixture::boot().await;
    let (mut client, _hello) = fx.connect("named-call").await;

    fx.speak(json!({"call_id": "named-call"}), "say something")
        .await;
    let _ = heard_the_order(&mut client).await;

    let emissions = fx
        .emissions_until("a speak_end on the lane", |m| {
            hop_of(m, "route") == "speak_end"
        })
        .await;
    let speak_end = emissions
        .iter()
        .find(|m| hop_of(m, "route") == "speak_end")
        .unwrap_or_else(|| panic!("the speak_end lane is on: {emissions:#?}"));
    assert_eq!(hop_of(speak_end, "call_id"), "named-call");
    assert_eq!(
        hop_of(speak_end, "session_id"),
        hop_of(speak_end, "call_id"),
        "one value under both names"
    );

    let error = emissions
        .iter()
        .find(|m| hop_of(m, "route") == "error")
        .unwrap_or_else(|| panic!("a synthesis with no provider is reported: {emissions:#?}"));
    assert_eq!(hop_of(error, "error_code"), "speak_failed");
    assert_eq!(
        hop_of(error, "call_id"),
        "named-call",
        "a failure that happened inside a call carries that call: {error:#?}"
    );

    fx.shutdown().await;
}

/// **A message that names neither key is refused, and the refusal says both.**
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speak_that_names_no_call_is_refused_by_name() {
    let mut fx = Fixture::boot().await;
    let (_client, _hello) = fx.connect("anybody").await;

    fx.speak(json!({}), "into which line?").await;

    let emissions = fx
        .emissions_until("a refusal naming no session", |m| {
            hop_of(m, "error_code") == "missing_session"
        })
        .await;
    let refusal = emissions
        .iter()
        .find(|m| hop_of(m, "error_code") == "missing_session")
        .unwrap_or_else(|| panic!("a speak with no address is refused: {emissions:#?}"));
    let Body::Inline(body) = &refusal.body else {
        panic!("a refusal of this cell is an inline body");
    };
    let detail = body
        .get("meta")
        .and_then(|m| m.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        detail.contains("context.call_id") && detail.contains("context.session_id"),
        "the refusal names both keys a caller may address a connection with: {detail}"
    );

    fx.shutdown().await;
}

/// **An empty `call_id` is not an address, and the shipped edge writes one.**
///
/// This is the case the tree actually produces, not a hypothetical: the install
/// manifest promotes `"call_id": "has(hop.call_id) ? hop.call_id : ''"`, because
/// a modifier that fails to evaluate skips the whole edge — so every emission
/// that carries no call (a `tool_schemas` answer, an older channel, a turn from
/// a cell that predates 1.4.0) arrives with `context.call_id` PRESENT and
/// EMPTY. Reading that as an address would look for a connection called `""`
/// and answer `unknown_session`, which is the GH #603 failure again with a new
/// key. The empty string is skipped and `context.session_id` decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_call_id_falls_through_to_the_session() {
    let fx = Fixture::boot().await;
    let (mut only_one, _hello) = fx.connect("still-here").await;

    fx.speak(
        json!({"call_id": "", "session_id": "still-here"}),
        "the guarded promotion writes an empty string, not nothing",
    )
    .await;

    let heard = heard_the_order(&mut only_one).await;
    assert!(
        heard["speak_id"].is_string(),
        "an empty call key is no key at all, and the session behind it is the \
         address: {heard}"
    );

    fx.shutdown().await;
}
