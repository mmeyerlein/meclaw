//! GH #657 — in `hold` mode the recognition session lives per hold.
//!
//! A page that joins a voice topic and is then left alone used to lose its
//! key. The session was opened when the connection came up, the provider's own
//! idle deadline ended it after `provider_idle_timeout_ms` of silence, and the
//! connection read that ending as a fault: an `stt_failed` frame, one retry,
//! the same ending again, `1011`. In `hold` mode no audio flows between two
//! holds by design, so the deadline was certain to fire and the button was
//! certain to die — measured on a live screen, twice, 32 s apart.
//!
//! So the session now begins with the first `hold` and an ending with no hold
//! open is not a fault. Seven cases, and the set is the claim:
//!
//! (a) `hold` mode: connect, wait past the idle deadline, hold, speak, release
//!     — a turn comes back, and no `stt_failed` and no close went past on the
//!     way;
//! (b) `auto` mode: the same silent provider still ends the call with
//!     `stt_failed` and `1011`, because there the session IS the call and a
//!     provider that stops listening is news;
//! (c) a connection that SWITCHES to `hold` gets the new arrangement, and
//!     (f) one that switches to `auto` gets the session that mode always has —
//!     the mode of a connection is not frozen at the handshake;
//! (d) a `cancel` does not close the boundary, so a failure under a held key is
//!     still reported;
//! (e) a session that ended with the key up costs no retry;
//! (g) a `hold` the turn machine REFUSED moves nothing here either.
//!
//! **The clock here is a semantic discriminator, not a budget.** The cell's
//! idle deadline is set to [`IDLE_MS`] and the test waits three times that
//! before it holds, so the ending the old code produced has happened by then
//! rather than probably-happened. Everything after the hold is waited for on a
//! frame, never on a sleep.

use meclaw_cells::voice::VoiceCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::{Frame, VoiceClient};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker window (30 s convention), never a budget.
const DEADLINE: Duration = Duration::from_secs(30);
/// The mount this file's cell answers under.
const MOUNT: &str = "voice";
/// Context key that opens the colony's egress door for this file's listener.
const MARK: &str = "voice_out";
/// The cell's provider idle deadline. Small on purpose: it is the thing the
/// test has to outwait, and the wait is the only sleep in this file.
const IDLE_MS: u64 = 200;
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the frame length the wire
/// recommends.
const FRAME_BYTES: usize = 640;
/// How many of those the held key sends.
const FRAMES: usize = 3;
/// What the scripted recogniser says once that audio really arrived.
const SAID: &str = "the key still works";

/// A booted colony holding exactly one `voice` cell, plus the door its
/// emissions leave through.
struct Fixture {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    egress: mpsc::Receiver<Message>,
    /// `ws://<listener>/<mount>`.
    ws_base: String,
    listener: tokio::task::JoinHandle<()>,
}

impl Fixture {
    /// Boot a colony whose only cell is a `voice` cell with these `params`.
    ///
    /// The root hive routes `./voice -> .` and stamps [`MARK`] on the way, so
    /// every emission of the cell dies at the root and lands on `egress` — the
    /// `gh163` pattern `voice_t5_behaviour.rs` uses for the same purpose.
    async fn boot(params: Value) -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path().join("main");
        std::fs::create_dir_all(root.join("voice")).expect("create the cell dir");
        let mut mark = meclaw_core::serde_json::Map::new();
        mark.insert(MARK.to_string(), json!("'1'"));
        std::fs::write(
            root.join("config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "hive"},
                "params": {"graph": {"edges": [{
                    "from": "./voice",
                    "to": ".",
                    "modifier": {"set_context": mark}
                }]}}
            }))
            .expect("serialise the hive"),
        )
        .expect("write the root hive");

        std::fs::write(
            root.join("voice/config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "voice", "timeout": -1},
                "params": params,
                "contract": {
                    "version": "1.0.0",
                    "settings": {},
                    // The cell mints `session_id` itself — a connection is
                    // where a session begins — so it is the setter the header
                    // check looks for.
                    "ingress": {"context": ["session_id"]},
                    "emits": {"body": {"messages": {"type": "array", "required": true}}},
                    "consumes": {
                        "body": {"messages": {"type": "array", "required": true}},
                        "context": {"session_id": {"type": "string", "required": true}}
                    }
                }
            }))
            .expect("serialise the cell"),
        )
        .expect("write the voice cell");

        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let voice: Arc<dyn CellFactory> = Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces)));
        let factories: Vec<(String, Arc<dyn CellFactory>)> =
            vec![("voice".to_string(), Arc::clone(&voice))];
        let mut registry = CellFactoryRegistry::new();
        registry.insert("voice".into(), voice);
        let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories, MARK);
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .expect("the colony boots with a voice cell");

        let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
        Self {
            _td: td,
            h,
            egress,
            ws_base: format!("ws://{addr}/{MOUNT}"),
            listener,
        }
    }

    /// Connect, retrying while the cell is still registering its mount.
    async fn connect(&self, query: &str) -> (VoiceClient, Value) {
        let url = format!("{}/ws?{query}", self.ws_base);
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

    /// Wait until `n` emissions carry `hop.route == route`, then return them
    /// together with everything else that arrived.
    async fn wait_for_route(&mut self, route: &str, n: usize) -> Vec<Message> {
        let deadline = Instant::now() + DEADLINE;
        let mut out: Vec<Message> = Vec::new();
        loop {
            if out.iter().filter(|m| hop_route(m) == Some(route)).count() >= n {
                return out;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "no `{route}` emission within {DEADLINE:?}; saw routes {:?}",
                out.iter()
                    .map(|m| hop_route(m).map(str::to_string))
                    .collect::<Vec<_>>()
            );
            if let Ok(Some(m)) = tokio::time::timeout(left, self.egress.recv()).await {
                out.push(m);
            }
        }
    }

    async fn shutdown(self) {
        self.listener.abort();
        self.h.shutdown().await;
    }
}

fn hop_route(m: &Message) -> Option<&str> {
    m.headers.hop.get("route").and_then(Value::as_str)
}

/// The `text` of the first turn in an emission's body.
fn body_text(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v.get("messages")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `code` of an `error` frame, if this is one.
fn error_code(frame: &Frame) -> Option<&str> {
    frame
        .as_text()
        .filter(|_| frame.is_type("error"))
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str)
}

/// A cell with both provider fakes and an idle deadline of [`IDLE_MS`].
///
/// `release_grace_ms` is left at the shipped 1500, on purpose: the released
/// boundary then waits for the recogniser's own end of turn instead of cutting
/// at the frame, and the WORDS are what this file's receipt is made of — an
/// empty turn would also be a turn. They arrive when the fake has the audio,
/// not when a cap expires, so no assertion here waits on a clock.
fn params(stt: &MockDeepgram, tts: &MockCartesia) -> Value {
    json!({
        "mount": MOUNT,
        // Small on purpose: this is the deadline the tests have to outwait, and
        // the only sleep in this file is the one that outwaits it.
        "provider_idle_timeout_ms": IDLE_MS,
        "stt": {
            "provider": "deepgram",
            "api_key": "fake-key",
            "language": "de",
            "sample_rate": 16000,
            "base_url": stt.base_url()
        },
        "tts": {
            "provider": "cartesia",
            "api_key": "fake-key",
            "voice": "fake-voice",
            "language": "de",
            "sample_rate": 24000,
            "base_url": tts.base_url()
        }
    })
}

/// (a) A page held open longer than the provider's idle deadline still gets a
/// turn out of its first hold — and was told nothing alarming in between.
///
/// The recogniser says nothing until the audio of the hold has really arrived,
/// so the turn proves the path rather than the script's timing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hold_after_the_idle_deadline_still_gets_its_turn() {
    let stt = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES * FRAMES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let mut fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, hello) = fx.connect("session=hold-idle&mode=hold").await;
    assert_eq!(hello["mode"], "hold");

    // Three idle deadlines with nobody speaking: the old code had sent its
    // first `stt_failed` after one of them.
    tokio::time::sleep(Duration::from_millis(IDLE_MS * 3)).await;

    client.hold().await.expect("open the boundary");
    for _ in 0..FRAMES {
        client
            .send_audio(&vec![0u8; FRAME_BYTES])
            .await
            .expect("the held key sends audio");
    }
    client.release().await.expect("let the key go");

    let seen = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    let turns: Vec<&Frame> = seen
        .iter()
        .map(|(_, f)| f)
        .filter(|f| f.is_type("turn"))
        .collect();
    assert_eq!(
        turns.len(),
        1,
        "the first hold of a page that waited must end in exactly one turn; saw {seen:?}"
    );
    assert_eq!(
        turns[0]
            .as_text()
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str),
        Some(SAID),
        "and it carries what was said into the held key"
    );
    assert!(
        seen.iter().all(|(_, f)| error_code(f).is_none()),
        "a session that ended with no hold open is not a fault the client hears about: {seen:?}"
    );
    assert!(
        seen.iter().all(|(_, f)| f.as_close().is_none()),
        "and it never closes the socket under the page: {seen:?}"
    );

    // The same turn on the lane, so the claim is not only about the socket.
    let lane = fx.wait_for_route("turn", 1).await;
    let emitted: Vec<&Message> = lane
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .collect();
    assert_eq!(emitted.len(), 1, "one hold, one turn on the lane");
    assert_eq!(body_text(emitted[0]), SAID);

    fx.shutdown().await;
}

/// (b) `auto` mode is untouched: there the session is the call, and a provider
/// that goes quiet ends it — one `stt_failed` on the first failure, one retry,
/// then `1011`.
///
/// Telephony runs this mode, and a caller on a line nobody is recognising must
/// be hung up on rather than left talking into it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_auto_mode_a_silent_provider_still_ends_the_call() {
    let stt = MockDeepgram::start(DeepgramScript::new())
        .await
        .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, hello) = fx.connect("session=auto-idle&mode=auto").await;
    assert_eq!(hello["mode"], "auto");

    let seen = client
        .collect_until(|f| f.as_close() == Some(1011), DEADLINE)
        .await;
    assert!(
        seen.iter()
            .any(|(_, f)| error_code(f) == Some("stt_failed")),
        "the client is told on the first failure, not only on the last: {seen:?}"
    );
    assert_eq!(
        seen.last().and_then(|(_, f)| f.as_close()),
        Some(1011),
        "and the second failure in a row ends the connection: {seen:?}"
    );

    fx.shutdown().await;
}

/// (c) A connection that SWITCHES to `hold` gets the new arrangement.
///
/// The mode of a connection is not frozen at the handshake: the built-in test
/// page offers both as radio buttons on a live socket. Before the review of
/// this task the connection decided on the word in the query string alone, so a
/// page that started in `auto` and switched carried the old rule with it — and
/// that rule is #657.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_that_switches_to_hold_gets_the_hold_arrangement() {
    let stt = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES * FRAMES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, hello) = fx.connect("session=switch&mode=auto").await;
    assert_eq!(hello["mode"], "auto", "it starts as a telephone would");

    client
        .set_mode("hold")
        .await
        .expect("the page switches the running connection");
    let switched = client.collect_until(|f| f.is_type("mode"), DEADLINE).await;
    assert!(
        switched.iter().any(|(_, f)| f
            .as_text()
            .and_then(|v| v.get("mode"))
            .and_then(Value::as_str)
            == Some("hold")),
        "the cell confirms the switch before the test leans on it: {switched:?}"
    );

    // The session the `auto` handshake opened now runs out — with the key up,
    // which after the switch is nobody's silence rather than a fault.
    tokio::time::sleep(Duration::from_millis(IDLE_MS * 3)).await;

    client.hold().await.expect("open the boundary");
    for _ in 0..FRAMES {
        client
            .send_audio(&vec![0u8; FRAME_BYTES])
            .await
            .expect("the held key sends audio");
    }
    client.release().await.expect("let the key go");

    let seen = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    assert_eq!(
        seen.iter()
            .filter(|(_, f)| f.is_type("turn"))
            .filter_map(|(_, f)| f.as_text())
            .filter_map(|v| v.get("text"))
            .filter_map(Value::as_str)
            .next(),
        Some(SAID),
        "the hold after the switch produces its turn: {seen:?}"
    );
    assert!(
        seen.iter().all(|(_, f)| error_code(f).is_none()),
        "and the switched connection is told nothing about a session that ended \
         with the key up: {seen:?}"
    );

    fx.shutdown().await;
}

/// (d) A `cancel` does not close the boundary, so a real failure under a held
/// key is still reported.
///
/// The turn machine leaves the boundary standing on `cancel` — it drops the
/// queue and the running synthesis and nothing else. A connection that treated
/// the frame as a release would go blind for as long as the key stays down: the
/// next provider death reads as nobody's silence while the client is waiting
/// for a turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_does_not_make_the_connection_blind() {
    let stt = MockDeepgram::start(DeepgramScript::new())
        .await
        .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, _) = fx.connect("session=cancel-hold&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client.cancel().await.expect("drop what is queued");

    // The key is still down, so the session that dies now is one the client is
    // waiting on.
    let seen = client
        .collect_until(|f| error_code(f) == Some("stt_failed"), DEADLINE)
        .await;
    assert!(
        seen.iter()
            .any(|(_, f)| error_code(f) == Some("stt_failed")),
        "a provider that dies under a held key is a fault the client hears \
         about, `cancel` or no `cancel`: {seen:?}"
    );

    fx.shutdown().await;
}

/// (e) A session that ended with the key up costs no retry.
///
/// The one retry exists for a provider that refuses twice in a row. A `hold`
/// connection lives as long as the page does, so a failure in the morning must
/// not turn the next failure in the evening into a `1011` — that is the dead
/// button again, one day later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quiet_ending_costs_no_retry() {
    let stt = MockDeepgram::start(DeepgramScript::new())
        .await
        .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, _) = fx.connect("session=budget&mode=hold").await;

    // 1. A real failure under a held key: reported, and the one retry is armed.
    //    One failure reaches this socket TWICE — the connection sends its own
    //    `error` frame and the handler answers the same event with one of its
    //    own — so the wait is for both, or the second would land in the quiet
    //    window below and look like a new fault.
    client.hold().await.expect("open the boundary");
    // `collect_until` takes an `Fn`, so the count lives beside it rather than
    // in it — the same shape `voice_t5_behaviour.rs` uses for its fifteenth
    // frame.
    let failures = AtomicUsize::new(0);
    let first = client
        .collect_until(
            |f| {
                error_code(f) == Some("stt_failed")
                    && failures.fetch_add(1, Ordering::SeqCst) + 1 == 2
            },
            DEADLINE,
        )
        .await;
    assert_eq!(
        first
            .iter()
            .filter(|(_, f)| error_code(f) == Some("stt_failed"))
            .count(),
        2,
        "the failure under the held key is reported: {first:?}"
    );

    // 2. The key goes up, and the session the retry would have opened is not
    //    opened at all. Nothing may reach the client out of that, and the
    //    budget goes back.
    client.release().await.expect("let the key go");
    let quiet = client.drain_for(Duration::from_millis(3000)).await;
    assert!(
        quiet
            .iter()
            .all(|(_, f)| error_code(f).is_none() && f.as_close().is_none()),
        "a session that ended with the key up is not a fault and closes nothing: \
         {quiet:?}"
    );

    // 3. And the next failure buys a retry again. The discriminator is the
    //    CLOCK and only from below: a connection whose budget was still spent
    //    from the first failure closes in the same breath as it reports, and a
    //    full one waits out `STT_RETRY_DELAY` (1 s) before it gives up.
    client.hold().await.expect("hold again");
    let seen = client
        .collect_until(|f| f.as_close().is_some(), DEADLINE)
        .await;
    let told = seen
        .iter()
        .find(|(_, f)| error_code(f) == Some("stt_failed"))
        .map(|(at, _)| *at)
        .unwrap_or_else(|| panic!("the second round reports its failure: {seen:?}"));
    let closed = seen
        .last()
        .filter(|(_, f)| f.as_close() == Some(1011))
        .map(|(at, _)| *at)
        .unwrap_or_else(|| panic!("and ends on 1011: {seen:?}"));
    assert!(
        closed.duration_since(told) >= Duration::from_secs(1),
        "the retry has to happen between the report and the close; a spent \
         budget closes at once: {:?}",
        closed.duration_since(told)
    );

    fx.shutdown().await;
}

/// (f) And the way back: a connection that switches to `auto` gets a session.
///
/// In `auto` the session IS the connection — there is no `hold` to open one,
/// and the client streams continuously. A connection that arrived in `hold` and
/// never held has no session at all, so the switch is the moment one has to be
/// opened: without it the audio of a page whose radio says `auto` is dropped in
/// silence, for ever, with no `error` and no turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_that_switches_to_auto_gets_a_session() {
    let stt = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES * FRAMES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, hello) = fx.connect("session=to-auto&mode=hold").await;
    assert_eq!(hello["mode"], "hold", "it starts as a screen would");

    client
        .set_mode("auto")
        .await
        .expect("the page switches the running connection");
    let switched = client.collect_until(|f| f.is_type("mode"), DEADLINE).await;
    assert!(
        switched.iter().any(|(_, f)| f
            .as_text()
            .and_then(|v| v.get("mode"))
            .and_then(Value::as_str)
            == Some("auto")),
        "the cell confirms the switch before the test leans on it: {switched:?}"
    );

    // No hold, no release: in `auto` the provider draws the boundary, and the
    // audio only has somewhere to go if the switch opened a session.
    for _ in 0..FRAMES {
        client
            .send_audio(&vec![0u8; FRAME_BYTES])
            .await
            .expect("the microphone switch streams");
    }

    let seen = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    assert_eq!(
        seen.iter()
            .filter(|(_, f)| f.is_type("turn"))
            .filter_map(|(_, f)| f.as_text())
            .filter_map(|v| v.get("text"))
            .filter_map(Value::as_str)
            .next(),
        Some(SAID),
        "the provider's own end of turn reached the client, so the audio \
         reached a session: {seen:?}"
    );

    fx.shutdown().await;
}

/// (g) A `hold` the turn machine refused must not move the connection either.
///
/// In `auto` a `hold` frame is answered with `wrong_mode` and the machine draws
/// no boundary. The connection used to take the frame at face value and mark
/// itself as holding — and a `mode` frame after it was then ignored here while
/// the machine switched, which is the two halves in different modes and #657
/// back with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_hold_leaves_the_connection_where_it_was() {
    let stt = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES * FRAMES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");
    let fx = Fixture::boot(params(&stt, &tts)).await;
    let (mut client, _) = fx.connect("session=refused-hold&mode=auto").await;

    client.hold().await.expect("the frame goes out either way");
    let refused = client
        .collect_until(|f| error_code(f) == Some("wrong_mode"), DEADLINE)
        .await;
    assert!(
        refused
            .iter()
            .any(|(_, f)| error_code(f) == Some("wrong_mode")),
        "a hold in `auto` is refused by name: {refused:?}"
    );

    client.set_mode("hold").await.expect("now switch for real");
    let switched = client.collect_until(|f| f.is_type("mode"), DEADLINE).await;
    assert!(
        switched.iter().any(|(_, f)| f
            .as_text()
            .and_then(|v| v.get("mode"))
            .and_then(Value::as_str)
            == Some("hold")),
        "and the switch is confirmed: {switched:?}"
    );

    // The session the `auto` handshake opened runs out with the key up. If the
    // refused frame had left this connection thinking it was holding, the
    // switch would have been ignored here and this is where the old story
    // starts again: `stt_failed`, a retry, `1011`.
    tokio::time::sleep(Duration::from_millis(IDLE_MS * 3)).await;

    client.hold().await.expect("open the boundary");
    for _ in 0..FRAMES {
        client
            .send_audio(&vec![0u8; FRAME_BYTES])
            .await
            .expect("the held key sends audio");
    }
    client.release().await.expect("let the key go");

    let seen = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    assert_eq!(
        seen.iter()
            .filter(|(_, f)| f.is_type("turn"))
            .filter_map(|(_, f)| f.as_text())
            .filter_map(|v| v.get("text"))
            .filter_map(Value::as_str)
            .next(),
        Some(SAID),
        "the hold after the switch produces its turn: {seen:?}"
    );
    assert!(
        seen.iter().all(|(_, f)| error_code(f).is_none()),
        "and nothing was reported about a session that ended with the key up: \
         {seen:?}"
    );

    fx.shutdown().await;
}
