//! GH #1059: the `voice` cell takes the keys of its provider slots from the
//! vault.
//!
//! Each slot (`stt`, `tts`, `duplex`) that names a `credential_grant_id` asks
//! once in `on_start`. The cell takes sessions from the first second, but a
//! slot without its key connects to nobody: a session waits at most the slot's
//! `credential_wait_ms` for the box and is then refused by name. The box opens
//! in the handler and the key reaches the I/O half, which builds the slot's
//! adapter with it. A box that does not open leaves the slot waiting; a lost
//! question is asked again after the wait (doubling); a respawn asks again
//! (key pair and key are RAM-only). Slots are independent. Stub values only.
//!
//! The provider stub is the capturing HTTP mock: the Deepgram adapter's
//! WebSocket upgrade is an HTTP `GET` carrying `Authorization: Token <key>`,
//! so the stub records exactly the key a session connected with — and answers
//! `404`, which is all these tests need from a provider.

#[path = "support/gh1059_grants.rs"]
mod gh1059_grants;

use gh1059_grants::{
    LogCapture, delivery, eventually, foreign_delivery, message, next_request, recipient_of,
    requests_within,
};
use meclaw_cells::voice::VoiceCellFactory;
use meclaw_cells::voice::params::{CredentialSlot, VoiceParams};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Message, Path};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const STT_GRANT: &str = "grant:deepgram_api_key@agent/voice";
const TTS_GRANT: &str = "grant:openai_api_key@agent/voice";
const MOUNT: &str = "voice-grant";

struct Spawned {
    sender: mpsc::Sender<Message>,
    join: JoinHandle<()>,
    out: mpsc::Receiver<CellEmission>,
    /// Where a client reaches the cell's mount.
    addr: SocketAddr,
    _listener: JoinHandle<()>,
    /// The colony inbox the cell was handed — held so the channel stays open.
    _inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
}

type Seen = Arc<Mutex<Vec<CapturedRequest>>>;

/// A provider stub that records every request (headers included) and answers
/// `404`.
async fn provider_stub() -> (String, Seen) {
    let (addr, _h, seen) = start_mock_server_capturing(vec![MockResponse::not_found()]).await;
    (addr.to_string(), seen)
}

/// How many requests reached the stub carrying `value` in a header.
fn carried(seen: &Seen, value: &str) -> usize {
    gh1059_grants::snapshot(seen)
        .iter()
        .filter(|r| r.headers.values().any(|v| v.contains(value)))
        .count()
}

/// A cascade: Deepgram behind `stt_host`, OpenAI speech behind `tts_host`.
/// `stt`/`tts` are merged into the two blocks (grant, wait, literal key).
fn params(stt_host: &str, stt: Value, tts_host: &str, tts: Value, top: Value) -> Value {
    let mut p = json!({
        "mount": MOUNT,
        "stt": {"provider": "deepgram", "base_url": format!("ws://{stt_host}")},
        "tts": {"provider": "openai", "base_url": format!("http://{tts_host}")},
    });
    for (k, v) in stt.as_object().unwrap() {
        p["stt"][k] = v.clone();
    }
    for (k, v) in tts.as_object().unwrap() {
        p["tts"][k] = v.clone();
    }
    for (k, v) in top.as_object().unwrap() {
        p[k] = v.clone();
    }
    p
}

async fn spawn(params: Value, dir: &tempfile::TempDir) -> Spawned {
    let cell_dir = dir.path().join("voice");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (out_tx, out) = mpsc::channel::<CellEmission>(256);
    let (inbox_tx, inbox) = mpsc::channel(64);
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let f = Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces)));
    let spawned = f
        .spawn_cell(
            Path::new("/voice"),
            params,
            out_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    let meclaw_colony::SpawnedCellKind::Active { sender, join, .. } = spawned else {
        unreachable!("a voice cell is spawned active");
    };
    let (addr, listener) = surface_listener(surfaces).await;
    Spawned {
        sender,
        join,
        out,
        addr,
        _listener: listener,
        _inbox: inbox,
    }
}

/// Connect a client in `auto` mode (a recognition session starts with the
/// connection), retrying while the mount is still registering. The `hello`
/// is the receipt that the cell took the session.
async fn connect(s: &Spawned) -> VoiceClient {
    let url = format!("ws://{}/{MOUNT}/ws?mode=auto", s.addr);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match VoiceClient::connect(&url).await {
            Ok((client, hello)) => {
                assert_eq!(hello["type"], "hello", "the cell took the session: {hello}");
                return client;
            }
            Err(e) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the voice cell never accepted a connection on {url}: {e}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn send(s: &Spawned, body: Value) {
    s.sender.send(message("/voice", body)).await.unwrap();
}

/// T1: one question per grant, and only one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_asks_once_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, _) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT}),
            &tts_host,
            json!({"credential_grant_id": TTS_GRANT}),
            json!({}),
        ),
        &dir,
    )
    .await;
    let stt = next_request(&mut s.out, Some(STT_GRANT)).await;
    assert_eq!(stt["header"]["grant_id"], STT_GRANT);
    let tts = next_request(&mut s.out, Some(TTS_GRANT)).await;
    assert_eq!(tts["header"]["grant_id"], TTS_GRANT);
    let more = requests_within(&mut s.out, Duration::from_millis(500), None).await;
    assert!(more.is_empty(), "exactly one question per grant: {more:?}");
}

/// T2: a session is taken, but its slot connects to nobody before the box —
/// not even with the literal that stands beside the grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_does_not_poll_or_connect_before_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT, "api_key": "stub-literal-2"}),
            &tts_host,
            json!({"api_key": "stub-literal-2t"}),
            json!({}),
        ),
        &dir,
    )
    .await;
    let _ = next_request(&mut s.out, Some(STT_GRANT)).await;
    let _client = connect(&s).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "no request to the recogniser before the box, though a session is open"
    );
}

/// T3 (and OR-VG-4: the literal next to a grant is never used).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_opens_the_box_and_starts() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT, "api_key": "stub-literal-3"}),
            &tts_host,
            json!({"api_key": "stub-literal-3t"}),
            json!({}),
        ),
        &dir,
    )
    .await;
    let request = next_request(&mut s.out, Some(STT_GRANT)).await;
    // The session opens first and waits for the key (default wait 10 s).
    let _client = connect(&s).await;
    send(&s, delivery(STT_GRANT, &request, "stub-secret-3")).await;
    eventually("a recognition session with the vaulted key", || {
        carried(&seen, "Token stub-secret-3") > 0
    })
    .await;
    assert_eq!(
        carried(&seen, "stub-literal-3"),
        0,
        "the literal is never used"
    );
    assert!(
        !logs
            .containing(
                "voice: params.stt.api_key is ignored because params.stt.credential_grant_id is set"
            )
            .is_empty(),
        "the ignored literal is named once"
    );
}

/// T4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_refuses_on_a_box_that_does_not_open() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT}),
            &tts_host,
            json!({"api_key": "stub-literal-4t"}),
            json!({}),
        ),
        &dir,
    )
    .await;
    let request = next_request(&mut s.out, Some(STT_GRANT)).await;
    send(&s, foreign_delivery(STT_GRANT, &request, "stub-secret-4")).await;
    eventually("the refusal's log line", || {
        !logs
            .containing("voice: the sealed credential was refused")
            .is_empty()
    })
    .await;
    let _client = connect(&s).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        gh1059_grants::snapshot(&seen).is_empty(),
        "a box that did not open starts nothing"
    );
}

/// T5.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_asks_again_after_a_respawn() {
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, _) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let p = params(
        &stt_host,
        json!({"credential_grant_id": STT_GRANT}),
        &tts_host,
        json!({"api_key": "stub-literal-5t"}),
        json!({}),
    );
    let mut first = spawn(p.clone(), &dir).await;
    let r1 = next_request(&mut first.out, Some(STT_GRANT)).await;
    send(&first, delivery(STT_GRANT, &r1, "stub-secret-5")).await;
    let Spawned { sender, join, .. } = first;
    drop(sender);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .unwrap()
        .unwrap();
    // The same cell directory, a new life: nothing of the key survived.
    let mut second = spawn(p, &dir).await;
    let r2 = next_request(&mut second.out, Some(STT_GRANT)).await;
    assert_ne!(
        recipient_of(&r1),
        recipient_of(&r2),
        "a new life, a new recipient"
    );
}

/// T6.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_asks_again_when_no_box_arrives() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT, "credential_wait_ms": 300}),
            &tts_host,
            json!({"api_key": "stub-literal-6t"}),
            json!({"credential_backoff_max_ms": 60_000}),
        ),
        &dir,
    )
    .await;
    let r1 = next_request(&mut s.out, Some(STT_GRANT)).await;
    let r2 = next_request(&mut s.out, Some(STT_GRANT)).await;
    assert_ne!(
        recipient_of(&r1),
        recipient_of(&r2),
        "a new round, a new recipient"
    );
    assert!(
        !logs
            .containing("voice: no sealed stt credential arrived in time — asking again")
            .is_empty(),
        "one line per round"
    );
    // The first round's box, late: discarded, the rounds go on.
    send(&s, delivery(STT_GRANT, &r1, "stub-secret-6-late")).await;
    eventually("the late box is discarded", || {
        !logs.containing("arrived late and was discarded").is_empty()
    })
    .await;
    assert!(gh1059_grants::snapshot(&seen).is_empty());
    // The question in flight now (round 2, or a later one if the doubling wait
    // ran out meanwhile) is the one whose box opens.
    let newer = requests_within(&mut s.out, Duration::from_millis(20), Some(STT_GRANT)).await;
    let current = newer.last().unwrap_or(&r2).clone();
    send(&s, delivery(STT_GRANT, &current, "stub-secret-6")).await;
    let _client = connect(&s).await;
    eventually("a recognition session with the current round's key", || {
        carried(&seen, "Token stub-secret-6") > 0
    })
    .await;
    assert_eq!(carried(&seen, "stub-secret-6-late"), 0);
}

/// T7: the stt box starts recognition; the tts slot keeps waiting — and keeps
/// asking — on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_voice_slots_ask_independently() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, stt_seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT}),
            &tts_host,
            json!({"credential_grant_id": TTS_GRANT, "credential_wait_ms": 300}),
            json!({"credential_backoff_max_ms": 60_000}),
        ),
        &dir,
    )
    .await;
    // `on_start` asks stt first, then tts — taken in that order.
    let r_stt = next_request(&mut s.out, Some(STT_GRANT)).await;
    let r_tts = next_request(&mut s.out, Some(TTS_GRANT)).await;
    send(&s, delivery(STT_GRANT, &r_stt, "stub-secret-7")).await;
    let _client = connect(&s).await;
    eventually("recognition runs on the stt key alone", || {
        carried(&stt_seen, "Token stub-secret-7") > 0
    })
    .await;
    // The tts slot is still without a key: its round ends and it asks again,
    // while the stt slot — which has its key — asks nothing more.
    let again = next_request(&mut s.out, Some(TTS_GRANT)).await;
    assert_ne!(
        recipient_of(&r_tts),
        recipient_of(&again),
        "a new round, a new recipient"
    );
    assert!(
        !logs
            .containing("voice: no sealed tts credential arrived in time — asking again")
            .is_empty(),
        "the tts slot's round line"
    );
    let later = requests_within(&mut s.out, Duration::from_millis(800), None).await;
    assert!(
        later.iter().all(|r| r["header"]["grant_id"] == TTS_GRANT),
        "only the slot without a key asks again: {later:?}"
    );
}

/// Review V2 M2 (GH #1061): two slots spending ONE grant run their rounds in
/// step, so both clocks expire together. The question is the grant's, not the
/// slot's: one round asks once. Before the fix every later round sent two
/// `credential_request`s within the same instant, and the first box came back
/// `Late` because the second question had already replaced its recipient.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_voice_slots_sharing_a_grant_ask_once_per_round() {
    const SHARED: &str = "grant:speech@agent/voice";
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, _) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": SHARED, "credential_wait_ms": 300}),
            &tts_host,
            json!({"credential_grant_id": SHARED, "credential_wait_ms": 300}),
            json!({"credential_backoff_max_ms": 60_000}),
        ),
        &dir,
    )
    .await;
    // `on_start` already asks once per grant.
    let _first = next_request(&mut s.out, Some(SHARED)).await;
    // Rounds end at 300 ms and at 900 ms (the wait doubles); the next one only
    // at 2100 ms. One question per round is two in this window, the doubled
    // asking of the old handler four.
    let later = requests_within(&mut s.out, Duration::from_millis(1_400), Some(SHARED)).await;
    assert!(
        !later.is_empty(),
        "the shared grant's slots still ask again when no box arrives"
    );
    assert!(
        later.len() <= 2,
        "one question per round for a grant two slots share, got {}",
        later.len()
    );
}

/// A6: a slot whose provider needs a key and has neither a grant nor a usable
/// literal is refused by the param's name, with the way out in the message.
#[test]
fn gh1059_a_voice_provider_without_grant_is_refused_by_name() {
    let cases = [
        (
            json!({"mount": "v", "stt": {"provider": "deepgram"},
                   "tts": {"provider": "openai", "api_key": "stub-literal-a"}}),
            "stt",
        ),
        (
            json!({"mount": "v", "stt": {"provider": "deepgram", "api_key": ""},
                   "tts": {"provider": "openai", "api_key": "stub-literal-a"}}),
            "stt",
        ),
        (
            // An unresolved `${…}` counts as no literal.
            json!({"mount": "v", "stt": {"provider": "deepgram", "api_key": "${DG_KEY}"},
                   "tts": {"provider": "openai", "api_key": "stub-literal-a"}}),
            "stt",
        ),
        (
            json!({"mount": "v",
                   "stt": {"provider": "deepgram", "credential_grant_id": STT_GRANT},
                   "tts": {"provider": "cartesia", "voice": "v1"}}),
            "tts",
        ),
        (
            json!({"mount": "v",
                   "duplex": {"provider": "gpt_live", "instructions": "x"}}),
            "duplex",
        ),
    ];
    for (raw, slot) in cases {
        let err = VoiceParams::parse(&raw).unwrap_err();
        assert!(err.starts_with(&format!("{slot}.api_key:")), "{err}");
        assert!(err.contains("name a credential_grant_id"), "{err}");
        assert!(
            err.contains(&format!("params.{slot}.credential_grant_id")),
            "{err}"
        );
    }
    // A grant alone is enough, per slot.
    let ok = VoiceParams::parse(&json!({
        "mount": "v",
        "stt": {"provider": "deepgram", "credential_grant_id": STT_GRANT},
        "tts": {"provider": "openai", "credential_grant_id": TTS_GRANT},
    }))
    .unwrap();
    assert_eq!(ok.slot_grant(CredentialSlot::Stt), Some(STT_GRANT));
    assert_eq!(ok.slot_grant(CredentialSlot::Tts), Some(TTS_GRANT));
    let live = VoiceParams::parse(&json!({
        "mount": "v",
        "duplex": {"provider": "gpt_live", "instructions": "x",
                   "credential_grant_id": "grant:openai_api_key@agent/live"},
    }))
    .unwrap();
    assert_eq!(
        live.slot_grant(CredentialSlot::Duplex),
        Some("grant:openai_api_key@agent/live")
    );
    // The loopbacks need nothing.
    VoiceParams::parse(&json!({"mount": "v", "stt": {"provider": "echo"}, "tts": null})).unwrap();
}

/// The grants and their schedule are settled at birth: the provider blocks
/// (which carry the grants) are no updatable key, the ceiling is immutable —
/// and a mount-only update keeps every grant through the merge.
#[test]
fn gh1059_voice_grants_are_settled_at_birth() {
    use meclaw_cells::params_overlay::{OverlayParams, apply_update};
    use meclaw_cells::voice::params::VoiceOverlay;
    use meclaw_core::serde_json::Map;
    let doc = json!({
        "mount": "v",
        "stt": {"provider": "deepgram", "credential_grant_id": STT_GRANT},
        "tts": {"provider": "openai", "credential_grant_id": TTS_GRANT},
        "credential_backoff_max_ms": 60_000,
    });
    let current = VoiceOverlay::parse(&doc).unwrap();
    for (key, value) in [
        ("credential_backoff_max_ms", json!(1_000)),
        (
            "stt",
            json!({"provider": "deepgram", "credential_grant_id": "grant:other"}),
        ),
    ] {
        let mut update = Map::new();
        update.insert(key.to_string(), value);
        let e = apply_update(&current, &update)
            .err()
            .unwrap_or_else(|| panic!("`{key}` is settled at birth"));
        assert!(e.detail().contains(key), "{}", e.detail());
    }
    let mut update = Map::new();
    update.insert("mount".to_string(), json!("w"));
    let (merged, _) = apply_update(&current, &update).unwrap();
    assert_eq!(merged.credential_backoff_max_ms, 60_000);
    let reparsed =
        VoiceParams::parse(&meclaw_core::serde_json::to_value(&merged).unwrap()).unwrap();
    assert_eq!(reparsed.slot_grant(CredentialSlot::Stt), Some(STT_GRANT));
    assert_eq!(reparsed.slot_grant(CredentialSlot::Tts), Some(TTS_GRANT));
}

/// No grant → the literal still works (the one-release transition), and the
/// cell asks nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_a_literal_key_still_works_for_one_release() {
    let dir = tempfile::tempdir().unwrap();
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"api_key": "stub-literal-9"}),
            &tts_host,
            json!({"api_key": "stub-literal-9t"}),
            json!({}),
        ),
        &dir,
    )
    .await;
    let _client = connect(&s).await;
    eventually("a recognition session with the literal key", || {
        carried(&seen, "Token stub-literal-9") > 0
    })
    .await;
    let asked = requests_within(&mut s.out, Duration::from_millis(300), None).await;
    assert!(asked.is_empty(), "no grant, no question: {asked:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1059_no_secret_in_the_log() {
    let logs = LogCapture::global();
    let dir = tempfile::tempdir().unwrap();
    // The stub answers 404: every session fails at the upgrade, the path where
    // an error text could carry what the request carried.
    let (stt_host, seen) = provider_stub().await;
    let (tts_host, _) = provider_stub().await;
    let mut s = spawn(
        params(
            &stt_host,
            json!({"credential_grant_id": STT_GRANT, "api_key": "stub-literal-10"}),
            &tts_host,
            json!({"credential_grant_id": TTS_GRANT, "api_key": "stub-literal-10t"}),
            json!({}),
        ),
        &dir,
    )
    .await;
    // `on_start` asks stt first, then tts — taken in that order.
    let r_stt = next_request(&mut s.out, Some(STT_GRANT)).await;
    let r_tts = next_request(&mut s.out, Some(TTS_GRANT)).await;
    let _client = connect(&s).await;
    send(&s, delivery(STT_GRANT, &r_stt, "stub-secret-10")).await;
    eventually("the box opened", || {
        !logs
            .containing("voice: credential received sealed and opened in RAM")
            .is_empty()
    })
    .await;
    eventually("a failed recognition session with the key", || {
        carried(&seen, "Token stub-secret-10") > 0
    })
    .await;
    // A refused box too, while the key is in RAM (the tts slot's default
    // wait keeps its first question current).
    send(&s, foreign_delivery(TTS_GRANT, &r_tts, "stub-secret-10t")).await;
    eventually("the tts refusal", || {
        !logs
            .containing("voice: the sealed credential was refused")
            .is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for needle in ["stub-secret-10", "stub-literal-10"] {
        let hits = logs.containing(needle);
        assert!(hits.is_empty(), "{needle} in the log: {hits:?}");
    }
}
