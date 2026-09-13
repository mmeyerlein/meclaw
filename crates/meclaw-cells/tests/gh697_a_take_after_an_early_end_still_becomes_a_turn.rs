//! GH #697 — a take whose boundary the cap closed with the provider idle must
//! not eat the take that follows it.
//!
//! The unit tests in `turns.rs` prove the automaton; this proves the
//! composition, because the incident of 2026-09-13 needed both halves: a
//! provider that is NOT inside a turn when the key comes up, and a cap that
//! closes the boundary afterwards. What used to happen then was that the NEXT
//! take's own end of turn was swallowed as payment for a debt nobody owed, so
//! that boundary was closed by its cap with nothing in it — `turn_seq` moved,
//! the lane saw nothing, and the number of the missing turn is the only trace.
//!
//! **Why the idle provider here is one that never started rather than one that
//! finished early.** In the incident the provider delivered its `EndOfTurn`
//! half a second before the key came up. On the wire that order is a race the
//! test cannot pin: an `EndOfTurn` inside an open, undrained boundary produces
//! no frame the client could wait for, and measured on this path the provider's
//! second message reaches the cell ~5 ms after its first while the client's
//! `release` round trip takes ~4.7 ms — six runs out of six lost it. The turn
//! machine cannot tell the two idle providers apart (`provider_in_turn` is
//! `false` either way), so the boundary is made idle by construction instead:
//! the key is held and nothing is said. That is also a take of its own — the
//! silent press that happens on every screen — and on `voice@2.0.1` it ate the
//! next real take just the same. Three holds, and only the middle one is silent.
//!
//! The fixture is the trimmed copy `gh657_a_hold_page_outlives_the_provider_idle.rs`
//! also carries: integration tests share no helpers, and only what this file
//! needs was copied.

use meclaw_cells::voice::VoiceCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker window (30 s convention), never a budget.
const DEADLINE: Duration = Duration::from_secs(30);
/// The mount this file's cell answers under.
const MOUNT: &str = "voice";
/// Context key that opens the colony's egress door for this file's listener.
const MARK: &str = "voice_out";
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the frame length the wire
/// recommends.
const FRAME: usize = 640;

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
                "only {} of {n} `{route}` emission(s) within {DEADLINE:?}; saw routes {:?}",
                out.iter().filter(|m| hop_route(m) == Some(route)).count(),
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

/// Wait until the client has seen a `partial` whose text contains `needle`.
///
/// Every provider message produces a partial of its own, so "a partial arrived"
/// would close a boundary somewhere in the middle of a script and make a test a
/// race. Waiting for the one that carries the whole take is what makes the next
/// step of the test a fact.
async fn wait_for_partial(client: &mut VoiceClient, needle: &str) {
    let seen = client
        .collect_until(
            |f| {
                f.as_text()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
                    .map(|t| t.contains(needle))
                    .unwrap_or(false)
            },
            DEADLINE,
        )
        .await;
    assert!(
        seen.iter().any(|(_, f)| f.is_type("partial")),
        "the script must have played before the boundary closes, got {seen:?}"
    );
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

mod fakes {
    use super::*;

    pub async fn deepgram(script: DeepgramScript) -> MockDeepgram {
        MockDeepgram::start(script)
            .await
            .expect("the fake deepgram binds")
    }

    pub async fn cartesia(script: CartesiaScript) -> MockCartesia {
        MockCartesia::start(script)
            .await
            .expect("the fake cartesia binds")
    }

    /// A synthesis provider that is configured and never asked to speak. The
    /// params contract refuses a cell that transcribes but cannot speak, so
    /// even a test about turns has to name one.
    pub fn cartesia_silent() -> CartesiaScript {
        CartesiaScript::new()
    }

    /// The shipped defaults with both fakes behind them and `emit_partials`
    /// said out loud (R-V8' makes it `false` by default).
    pub fn both_params(mount: &str, stt: &MockDeepgram, tts: &MockCartesia) -> Value {
        json!({
            "mount": mount,
            "emit_partials": true,
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_take_survives_a_first_one_the_provider_ended_early() {
    // Take one: a whole take, its end gated on the audio the client sends AFTER
    // the release, so the boundary is closed by the provider and the `turn`
    // that reaches the topology is the rendezvous for everything after it.
    // Take two: the key held over silence — the provider never starts a turn,
    // and the cap closes the boundary with the provider idle. Exactly the
    // condition under which the old code recorded a debt nobody owed.
    // Take three: another whole take, its end again gated behind the release.
    // With the debt in place that end was eaten and the cap cut with nothing.
    let script = DeepgramScript::new()
        .require_audio_bytes(FRAME)
        .turn_info("Update", "first take")
        .require_audio_bytes(FRAME * 2)
        .turn_info("EndOfTurn", "first take whole")
        .require_audio_bytes(FRAME * 4)
        .turn_info("Update", "third take")
        .require_audio_bytes(FRAME * 5)
        .turn_info("EndOfTurn", "third take whole");
    let fake = fakes::deepgram(script).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    // Short, because take two is DELIBERATELY closed by the cap: the provider
    // has nothing to say about it.
    params["release_grace_ms"] = json!(200);
    let mut fx = Fixture::boot(params).await;
    let (mut client, _) = fx.connect("session=early-end&mode=hold").await;

    client.hold().await.expect("open the first boundary");
    client.send_audio(&vec![0u8; FRAME]).await.expect("audio");
    wait_for_partial(&mut client, "first take").await;
    client.release().await.expect("let the key go");
    client
        .send_audio(&vec![0u8; FRAME])
        .await
        .expect("the audio still in the capture chain when the key came up");
    let mut turns: Vec<String> = fx
        .wait_for_route("turn", 1)
        .await
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .map(body_text)
        .collect();
    assert_eq!(
        turns,
        vec!["first take whole".to_string()],
        "the first take is whole, closed by the provider's own end"
    );
    let mirrored = client
        .next_frame_of_type("turn", DEADLINE)
        .await
        .expect("the client sees its first boundary close");
    assert_eq!(
        mirrored.get("text").and_then(Value::as_str),
        Some("first take whole"),
        "and sees the same words the lane saw: {mirrored}"
    );

    // The silent press: one frame of audio, nothing recognised, the key up —
    // and the client waits for the cap to close it, so the next hold opens
    // after the cut rather than being the cut. That is the incident's path:
    // the boundary closed by `release_grace_ms`, with the provider idle.
    client.hold().await.expect("open the second boundary");
    client.send_audio(&vec![0u8; FRAME]).await.expect("audio");
    client.release().await.expect("let the key go over silence");
    let empty = client
        .next_frame_of_type("turn", DEADLINE)
        .await
        .expect("the cap closes the silent boundary");
    assert_eq!(
        empty.get("text").and_then(Value::as_str),
        Some(""),
        "a boundary with nothing in it is still closed, for the client: {empty}"
    );

    client.hold().await.expect("open the third boundary");
    client.send_audio(&vec![0u8; FRAME]).await.expect("audio");
    wait_for_partial(&mut client, "third take").await;
    client.release().await.expect("let the key go again");
    client
        .send_audio(&vec![0u8; FRAME])
        .await
        .expect("the audio still in the capture chain when the key came up");

    // The lane sees one more turn and not two: the silent boundary produces
    // its `turn` frame for the client and no lane emission (the pipecat rule,
    // R-V11). Before the fix it saw none — the third take's end paid the debt
    // the silent one left, and the cap cut with nothing.
    turns.extend(
        fx.wait_for_route("turn", 1)
            .await
            .iter()
            .filter(|m| hop_route(m) == Some("turn"))
            .map(body_text),
    );
    assert_eq!(
        turns,
        vec![
            "first take whole".to_string(),
            "third take whole".to_string()
        ],
        "both takes with words reach the topology, in order, whole: {turns:?}"
    );
    assert_eq!(
        fake.connections(),
        1,
        "and on ONE recognition session — the debt that ate the next take \
         only exists while the session lives"
    );

    fx.shutdown().await;
}

/// The provider's idle deadline, for the test below. Small, because it is the
/// ending the test needs to have happened, and it happens with the key up —
/// but not smaller than this: the deadline runs from the moment a session is
/// opened and is reset only by frames the provider sends, so on the SECOND
/// session `hold` → accept → audio → `Update` has to fit inside it, or the
/// ending fires under a held key and takes the retry path with an `error`
/// frame. Locally that handshake is a few milliseconds; under the gate, where
/// 247 binaries share four threads beside a build, it is not. 500 ms is a
/// multiple of what was measured, and the test pays half a second for it.
const IDLE_MS: u64 = 500;
/// The cap of the test below — longer than [`IDLE_MS`] by a margin `tokio`'s
/// timers respect (a sleep never wakes early), so the `turn` the cap produces
/// is a receipt that the idle deadline was due before it.
const GRACE_MS: u64 = 1000;

/// Wait until the fake has accepted `n` connections. A rendezvous on a fact
/// the fake records, never a sleep: the loop yields to the runtime between
/// looks and gives up at the failure marker.
async fn wait_for_connections(fake: &MockDeepgram, n: usize) {
    let deadline = Instant::now() + DEADLINE;
    while fake.connections() < n {
        assert!(
            Instant::now() < deadline,
            "the fake saw {} connection(s), not {n}, within {DEADLINE:?}",
            fake.connections()
        );
        tokio::task::yield_now().await;
    }
}

/// GH #697, the other half of the fix: a recognition session that ends
/// between two holds tells the turn machine so, and the debt it owed is
/// written off with it.
///
/// The debt is real here — the provider WAS inside a turn when the cap cut the
/// first boundary — and it is owed on a socket that then dies of the provider's
/// idle deadline with the key up. That ending used to be dropped in silence
/// (GH #657), and `Closed` is the only thing that writes a debt off, so the
/// debt survived onto the next session and was paid by the end of the next
/// take, which then never became a turn. The idle path is exactly the one on
/// which the adapter sends no `Closed` of its own, so this is the branch A3
/// changed and nothing else pins.
///
/// The script is per connection, so the second session plays the same lines:
/// on the first it gets one frame and then goes quiet, on the second it gets
/// the frame that releases its end of turn only after the key came up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_ends_between_two_holds_writes_its_debt_off() {
    let script = DeepgramScript::new()
        .require_audio_bytes(FRAME)
        .turn_info("Update", "take")
        .require_audio_bytes(FRAME * 2)
        .turn_info("EndOfTurn", "take whole");
    let fake = fakes::deepgram(script).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    params["provider_idle_timeout_ms"] = json!(IDLE_MS);
    params["release_grace_ms"] = json!(GRACE_MS);
    let mut fx = Fixture::boot(params).await;
    let (mut client, _) = fx.connect("session=idle-debt&mode=hold").await;

    // Take one: the provider starts a turn and never ends it. The cap cuts
    // with the interim and the session is left owing an end of turn.
    client.hold().await.expect("open the first boundary");
    client.send_audio(&vec![0u8; FRAME]).await.expect("audio");
    wait_for_partial(&mut client, "take").await;
    client.release().await.expect("let the key go");
    // Everything the client sees from here to the second take's partial is
    // kept, not skipped: the idle ending happens inside this stretch, and the
    // claim that it reaches the client as NOTHING (GH #657) is only a claim
    // about frames somebody looked at.
    let mut heard = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    let cut = heard
        .iter()
        .find(|(_, f)| f.is_type("turn"))
        .and_then(|(_, f)| f.as_text().cloned())
        .expect("the cap closes the first boundary");
    assert_eq!(
        cut.get("text").and_then(Value::as_str),
        Some("take"),
        "the cap cuts with what the provider had said so far: {cut}"
    );
    let first: Vec<String> = fx
        .wait_for_route("turn", 1)
        .await
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .map(body_text)
        .collect();
    assert_eq!(first, vec!["take".to_string()]);

    // By now the provider's idle deadline was due (it is shorter than the cap
    // by a margin) and it fired with the key up: the quiet ending of GH #657.
    // The next hold therefore opens a NEW session, which the fake records.
    client.hold().await.expect("open the second boundary");
    wait_for_connections(&fake, 2).await;
    client.send_audio(&vec![0u8; FRAME]).await.expect("audio");
    heard.extend(
        client
            .collect_until(
                |f| {
                    f.as_text()
                        .and_then(|v| v.get("text"))
                        .and_then(Value::as_str)
                        .map(|t| t.contains("take"))
                        .unwrap_or(false)
                },
                DEADLINE,
            )
            .await,
    );
    assert!(
        heard.iter().any(|(_, f)| f.is_type("partial")),
        "the fresh session played its script: {heard:?}"
    );
    assert!(
        heard.iter().all(|(_, f)| !f.is_type("error")),
        "the quiet ending between the two holds was nobody's failure — no \
         `error` frame from the first release to the second take's partial: \
         {heard:?}"
    );
    client.release().await.expect("let the key go again");
    client
        .send_audio(&vec![0u8; FRAME])
        .await
        .expect("the audio still in the capture chain when the key came up");

    // Before A3 this end of turn paid the debt the dead session had left
    // behind, the cap then cut with nothing, and the lane saw no turn at all.
    let second: Vec<String> = fx
        .wait_for_route("turn", 1)
        .await
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .map(body_text)
        .collect();
    assert_eq!(
        second,
        vec!["take whole".to_string()],
        "the take on the fresh session is whole; a debt owed on a socket that \
         is gone must not be paid on this one"
    );
    fx.shutdown().await;
}
