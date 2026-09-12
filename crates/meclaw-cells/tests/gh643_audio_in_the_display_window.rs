//! GH #643 — audio in the window a person is already looking at.
//!
//! One colony: the shipped `display` under a mount, a `voice` cell with a
//! mount and NO port at all, and the two scripted provider fakes. A Rust client
//! opens the display's own LiveView socket, joins `voice:call-1` on it, holds,
//! sends 20 ms PCM frames as binary pushes, releases — and reads the turn on the
//! topic. Then the test speaks an answer into the cell from the topology side and
//! reads the speech back on the same topic.
//!
//! The proof is that both halves of one call travel over a socket that was opened
//! for a page. Nothing binds a second port: the `voice` cell has none, and every
//! frame reaches it through the mount table.
//!
//! It also PRINTS a number and asserts nothing about it: the milliseconds from the
//! `release` frame going out to the `turn` push coming back. A threshold there
//! would measure the runner's load; a number a person reads beside the run is what
//! the wave's receipt quotes. `release_grace_ms` is `0` here so the number is the
//! path and not the cap -- see the fixture.
//!
//! Nothing here is timed by a sleep. With the cap at zero the release cuts the
//! turn with whatever the recogniser has ALREADY said, so the release has to come
//! after the provider heard the audio -- and that is waited for on the fake's own
//! byte count rather than hoped for behind a delay.
//!
//! Guarded like every template-reading test (GH #49): a tree without the library
//! is skipped, never judged.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::voice::VoiceCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::{surface_listener, wait_for_mount};

/// The name the screen answers to on the colony's one listener. The port in
/// the URLs below is the LISTENER's: a `web` cell has none since `web@2.0.0`.
const SCREEN_MOUNT: &str = "display";
use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the frame length the wire recommends.
const FRAME_BYTES: usize = 640;
/// Half a second of speech, sent at the pace a browser sends it.
const FRAMES: usize = 25;
/// What the scripted recogniser says once the audio really arrived.
const SAID: &str = "what time is it";
/// What the test speaks back into the call.
const ANSWER: &str = "It is noon.";
/// The call this test holds.
const CALL: &str = "call-1";
/// The context key that opens the colony's egress door for this file's listener.
const MARK: &str = "voice_out";

/// Every template this file reads, spelled out so the export's R2b check sees
/// the names (GH #9).
const NEEDED: [&str; 2] = ["templates/display", "templates/web"];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn library_ships() -> bool {
    NEEDED
        .iter()
        .all(|rel| repo(rel).join("template.json").is_file())
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("dirs");
    let text = meclaw_core::serde_json::to_string_pretty(v).expect("json");
    std::fs::write(p, text).expect("write");
}

fn patch(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    write_json(p, &v);
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("dirs");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// The contract the shipped `voice` template declares, minus what a fixture has
/// no use for. Read off `templates/voice/config.json` rather than invented.
fn voice_contract() -> Value {
    json!({
        "version": "1.0.0",
        "settings": {},
        // The cell mints `session_id` itself -- a connection is where a session
        // begins -- so it is the setter the header check looks for.
        "ingress": {"context": ["session_id"]},
        "emits": {
            "body": {"messages": {"type": "array", "required": true}},
            "hop": {
                "route": {"type": "string", "required": false},
                "session_id": {"type": "string", "required": false},
                "call_id": {"type": "string", "required": false},
                "turn_id": {"type": "string", "required": false},
                "platform": {"type": "string", "required": false},
                "mode": {"type": "string", "required": false},
                "eager": {"type": "boolean", "required": false},
                "error_code": {"type": "string", "required": false}
            }
        },
        "consumes": {
            "body": {"messages": {"type": "array", "required": true}},
            "context": {
                "call_id": {"type": "string", "required": false},
                "session_id": {"type": "string", "required": false}
            }
        }
    })
}

/// The colony this file measures, and the doors the test holds onto it.
struct Live {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    /// Everything the `voice` cell emitted that reached the root.
    egress: mpsc::Receiver<Message>,
    /// Where the display serves its page.
    /// The port of the one listener in front of the whole colony.
    port: u16,
    _listener: tokio::task::JoinHandle<()>,
    /// Read, not just held: the release below waits until this fake has the bytes
    /// that unlock its scripted end of turn.
    stt: MockDeepgram,
    _tts: MockCartesia,
}

/// Boot the display, the voice cell and the two fakes into one colony.
///
/// The two factories share ONE registry, which is the whole point: the `web`
/// cell's socket loop finds the `voice` cell in it, and neither of them knows
/// anything about the other.
async fn boot() -> Live {
    // The recogniser says nothing until ALL the audio has arrived. That is what
    // makes the printed latency a number about the path rather than about
    // `release_grace_ms`: the provider's end of turn lands next to the release
    // instead of half a second before it, which is where a real one lands.
    let stt_fake = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES * FRAMES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts_fake = MockCartesia::start(CartesiaScript::new().with_chunks(vec![
        vec![0u8; 960],
        vec![1u8; 960],
        vec![2u8; 480],
    ]))
    .await
    .expect("the fake cartesia binds");

    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();

    write_json(&root.join("colony.json"), &json!({"schema_version": 1}));
    let mut mark = meclaw_core::serde_json::Map::new();
    mark.insert(MARK.to_string(), json!("'1'"));
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./screen",
                 "condition": "has(hop.route) && hop.route == 'in_view'"},
                {"from": ".", "to": "./voice",
                 "condition": "has(hop.route) && hop.route == 'in_speak'"},
                // Whatever the voice cell says leaves the colony onto this
                // file's own channel, marked on the way (the `MARK` pattern).
                {"from": "./voice", "to": ".", "modifier": {"set_context": mark}}
            ]}}
        }),
    );

    copy_tree(&repo("templates/display"), &root.join("main/screen"));
    // The display refs `web@2.0.1`, and a ref resolves against the templates
    // table, which is empty until somebody fills it (GH #424).
    copy_tree(&repo("templates/web"), &root.join("templates/web"));
    patch(&root.join("main/screen/web/config.json"), |v| {
        v["override_params"][""]["mount"] = json!(SCREEN_MOUNT)
    });

    // A voice cell on its mount. Nothing of this cell listens anywhere; every
    // frame it sees came through the display's socket.
    write_json(
        &root.join("main/voice/config.json"),
        &json!({
            "cell": {"type": "voice", "timeout": -1},
            "params": {
                "mount": "voice",
                // `0` cuts the turn on the release frame with whatever the
                // recogniser has already said. The shipped default is 1500 ms,
                // and with it the number printed below would be that cap
                // rather than the path this strand built -- the same choice
                // `voice_t5_behaviour.rs` makes at `hold_release_boundary`, and
                // for the same reason.
                "release_grace_ms": 0,
                "stt": {
                    "provider": "deepgram",
                    "api_key": "fake-key",
                    "language": "de",
                    "sample_rate": 16000,
                    "base_url": stt_fake.base_url()
                },
                "tts": {
                    "provider": "cartesia",
                    "api_key": "fake-key",
                    "voice": "fake-voice",
                    "language": "de",
                    "sample_rate": 24000,
                    "base_url": tts_fake.base_url()
                }
            },
            "contract": voice_contract()
        }),
    );

    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        (
            "web".to_string(),
            Arc::new(WebCellFactory::new(Arc::clone(&surfaces))),
        ),
        (
            "voice".to_string(),
            Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces))),
        ),
    ];
    let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories.clone(), MARK);
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the template table fills");
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the colony boots");

    wait_for_mount(&surfaces, SCREEN_MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    Live {
        _td: td,
        h,
        egress,
        port: addr.port(),
        _listener: listener,
        stt: stt_fake,
        _tts: tts_fake,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// GET the screen until it serves a page, and hand back its session token.
async fn token_of(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/{SCREEN_MOUNT}/");
    let deadline = Instant::now() + MARKER;
    loop {
        if let Ok(r) = reqwest::get(&url).await
            && r.status().is_success()
        {
            let body = r.text().await.expect("text");
            let marker = "data-phx-session=\"";
            if let Some(at) = body.find(marker) {
                let start = at + marker.len();
                let end = start + body[start..].find('"').expect("the token is quoted");
                return body[start..end].to_string();
            }
        }
        assert!(Instant::now() < deadline, "the screen never served a page");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn send(ws: &mut Ws, frame: Value) {
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("send");
}

/// What `phoenix.min.js` `binaryEncode` writes for a client push.
fn binary_push(join_ref: &str, msg_ref: &str, topic: &str, event: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![
        0u8,
        join_ref.len() as u8,
        msg_ref.len() as u8,
        topic.len() as u8,
        event.len() as u8,
    ];
    out.extend_from_slice(join_ref.as_bytes());
    out.extend_from_slice(msg_ref.as_bytes());
    out.extend_from_slice(topic.as_bytes());
    out.extend_from_slice(event.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// One frame off the socket: text as JSON, binary as bytes.
enum Seen {
    Text(Value),
    Binary(Vec<u8>),
}

async fn next_frame(ws: &mut Ws) -> Seen {
    let msg = tokio::time::timeout(MARKER, ws.next())
        .await
        .expect("the socket answers within the failure-marker window")
        .expect("the stream stays open")
        .expect("a frame");
    match msg {
        WsMessage::Text(t) => {
            Seen::Text(meclaw_core::serde_json::from_str(&t).expect("the frame is JSON"))
        }
        WsMessage::Binary(b) => Seen::Binary(b.into()),
        other => panic!("expected a frame, got {other:?}"),
    }
}

/// Read until a text frame on `topic` satisfies `want`, keeping what went past.
async fn until(
    ws: &mut Ws,
    topic: &str,
    want: impl Fn(&Value) -> bool,
) -> (Vec<Value>, Vec<Vec<u8>>) {
    let mut texts = Vec::new();
    let mut binaries = Vec::new();
    let deadline = Instant::now() + MARKER;
    loop {
        match next_frame(ws).await {
            Seen::Binary(bytes) => binaries.push(bytes),
            Seen::Text(v) => {
                let hit = v[2] == json!(topic) && want(&v[4]);
                texts.push(v);
                if hit {
                    return (texts, binaries);
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "the frame never arrived; saw {texts:?}"
        );
    }
}

/// Hand the cell an assistant turn for `call`, the way a topology does.
async fn speak_into(h: &ColonyHandle, call: &str, text: &str) {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".to_string(), json!(call));
    let mut speak_hop = meclaw_core::serde_json::Map::new();
    speak_hop.insert("route".to_string(), json!("in_speak"));
    h.send(
        MessageBuilder::new(Path::new("/voice"))
            .context(context)
            .hop(speak_hop)
            .body(Body::Inline(json!({
                "messages": [{"origin": "assistant", "type": "text", "text": text}]
            })))
            .build(),
    )
    .await;
}

fn hop(m: &Message, key: &str) -> Option<String> {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Wait until `n` emissions on `route` reached the door, and answer with all of
/// them.
async fn wait_for_route(
    egress: &mut mpsc::Receiver<Message>,
    route: &str,
    n: usize,
) -> Vec<Message> {
    let deadline = Instant::now() + MARKER;
    let mut seen = Vec::new();
    loop {
        if seen
            .iter()
            .filter(|m| hop(m, "route").as_deref() == Some(route))
            .count()
            >= n
        {
            return seen;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "no `{route}` emission within {MARKER:?}; saw {:?}",
            seen.iter().map(|m| hop(m, "route")).collect::<Vec<_>>()
        );
        if let Ok(Some(m)) = tokio::time::timeout(left, egress.recv()).await {
            seen.push(m);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_speaks_into_the_page_and_hears_the_answer_on_it() {
    if !library_ships() || !have_python() {
        return;
    }
    let mut live = boot().await;

    // 1. The page, and its own socket.
    let token = token_of(live.port).await;
    let page_topic = format!(
        "lv:{}",
        meclaw_surface::session::container_id("/screen/web")
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/{SCREEN_MOUNT}/live/websocket",
        live.port
    ))
    .await
    .expect("the display accepts a websocket");
    send(
        &mut ws,
        json!(["1", "1", page_topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/{SCREEN_MOUNT}/", live.port)
        }]),
    )
    .await;
    let Seen::Text(joined) = next_frame(&mut ws).await else {
        panic!("a join is answered with text")
    };
    assert_eq!(joined[4]["status"], json!("ok"), "{joined}");

    // 2. The call, on the same socket. `hello` is the first frame on the topic.
    let topic = format!("voice:{CALL}");
    send(
        &mut ws,
        json!(["1", "2", topic, "phx_join",
               {"mount": "voice", "mode": "hold", "sample_rate": 16000}]),
    )
    .await;
    let (texts, _) = until(&mut ws, &topic, |p| {
        p["status"] == "ok" || p["type"] == "hello"
    })
    .await;
    assert!(
        texts
            .iter()
            .any(|v| v[3] == json!("phx_reply") && v[4]["status"] == json!("ok")),
        "the join is answered: {texts:?}"
    );
    let (texts, _) = if texts.iter().any(|v| v[4]["type"] == json!("hello")) {
        (texts, Vec::new())
    } else {
        until(&mut ws, &topic, |p| p["type"] == "hello").await
    };
    let hello = texts
        .iter()
        .find(|v| v[4]["type"] == json!("hello"))
        .expect("hello");
    assert_eq!(
        hello[3],
        json!("frame"),
        "hello travels as a frame: {hello}"
    );
    assert_eq!(hello[4]["session_id"], json!(CALL));
    assert_eq!(hello[4]["audio_in"]["sample_rate"], json!(16000));

    // 3. Hold, half a second of audio at the pace a browser sends it, release.
    send(&mut ws, json!(["1", "3", topic, "frame", {"type": "hold"}])).await;
    for _ in 0..FRAMES {
        ws.send(WsMessage::Binary(
            binary_push("1", "", &topic, "audio", &vec![0u8; FRAME_BYTES]).into(),
        ))
        .await
        .expect("send audio");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The release cuts the turn with whatever the recogniser has already said
    // (`release_grace_ms: 0`), so it must not overtake the audio. The fake counts
    // the bytes it received and its scripted end of turn is unlocked by exactly
    // that count, so waiting for the count is waiting for the transcript to be on
    // its way. The 20 ms sleep in the loop above paces the send like a browser; it
    // decides nothing.
    let want = FRAME_BYTES * FRAMES;
    let audio_deadline = Instant::now() + MARKER;
    while live.stt.received_audio_bytes() < want {
        assert!(
            Instant::now() < audio_deadline,
            "the recogniser only ever saw {} of {want} bytes",
            live.stt.received_audio_bytes()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let released_at = Instant::now();
    send(
        &mut ws,
        json!(["1", "4", topic, "frame", {"type": "release"}]),
    )
    .await;

    // 4. The turn, on the topic. The number is printed, not asserted on.
    let (texts, _) = until(&mut ws, &topic, |p| p["type"] == "turn").await;
    let latency = released_at.elapsed();
    let turn = texts
        .iter()
        .find(|v| v[4]["type"] == json!("turn"))
        .expect("a turn");
    assert_eq!(turn[4]["text"], json!(SAID), "{turn}");
    assert_eq!(turn[4]["turn_id"], json!(format!("{CALL}#1")));
    println!("LATENCY release->turn {} ms", latency.as_millis());

    // 5. The same turn on the cell's own lane, which is what a colony reads --
    //    and EXACTLY one of them, which needs something behind it. Waiting for the
    //    first `turn` and then counting turns can only ever find that one.
    //
    //    So a second `in_speak` goes in, addressed to a call that does not exist.
    //    The cell answers that at once on its ERROR lane (`unknown_session`), out
    //    of the same handler and down the same road as the turn -- so by the time
    //    it is here, a second turn would already be in front of it.
    let seen = wait_for_route(&mut live.egress, "turn", 1).await;
    speak_into(&live.h, "no-such-call", "nobody is listening").await;
    let mut after = wait_for_route(&mut live.egress, "error", 1).await;
    let mut all = seen;
    all.append(&mut after);
    let turns: Vec<&Message> = all
        .iter()
        .filter(|m| hop(m, "route").as_deref() == Some("turn"))
        .collect();
    assert_eq!(
        turns.len(),
        1,
        "one turn on the topic is one turn on the lane: {:?}",
        all.iter().map(|m| hop(m, "route")).collect::<Vec<_>>()
    );
    assert_eq!(hop(turns[0], "call_id").as_deref(), Some(CALL));
    let Body::Inline(body) = &turns[0].body else {
        panic!("an inline body")
    };
    assert_eq!(body["messages"][0]["text"], json!(SAID));

    // 6. The answer, spoken into the call from the topology side, heard on the
    //    same topic: a start, at least one binary broadcast, an end.
    speak_into(&live.h, CALL, ANSWER).await;

    let (texts, binaries) = until(&mut ws, &topic, |p| p["type"] == "speak_end").await;
    assert!(
        texts.iter().any(|v| v[4]["type"] == json!("speak_start")),
        "the client is told before the audio, not after: {texts:?}"
    );
    let ended = texts
        .iter()
        .find(|v| v[4]["type"] == json!("speak_end"))
        .expect("speak_end");
    assert_eq!(ended[4]["reason"], json!("done"), "{ended}");
    assert!(
        !binaries.is_empty(),
        "speech comes back as binary broadcasts on the same topic"
    );
    for encoded in &binaries {
        assert_eq!(encoded[0], 2u8, "a broadcast, so nobody has to reply to it");
        assert_eq!(&encoded[1..3], &[topic.len() as u8, 5u8]);
        assert_eq!(&encoded[3..3 + topic.len()], topic.as_bytes());
        assert_eq!(&encoded[3 + topic.len()..8 + topic.len()], b"audio");
    }

    // 7. Leaving the topic ends the call, and nothing follows it.
    send(&mut ws, json!(["1", "6", topic, "phx_leave", {}])).await;
    let Seen::Text(left) = next_frame(&mut ws).await else {
        panic!("a leave is answered with text")
    };
    assert_eq!(left[4]["status"], json!("ok"), "{left}");
    send(&mut ws, json!(["1", "7", "phoenix", "heartbeat", {}])).await;
    loop {
        match next_frame(&mut ws).await {
            Seen::Binary(_) => continue,
            Seen::Text(v) => {
                assert_ne!(
                    v[2],
                    json!(topic.clone()),
                    "a frame on a topic that was left: {v}"
                );
                assert_eq!(v[2], json!("phoenix"), "{v}");
                assert_eq!(v[4]["status"], json!("ok"));
                break;
            }
        }
    }

    live.h.shutdown().await;
}
