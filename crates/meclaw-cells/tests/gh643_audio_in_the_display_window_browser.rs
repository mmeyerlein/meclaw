//! GH #643 — the same call, driven by a real browser.
//!
//! The colony beside this one (`gh643_audio_in_the_display_window.rs`) proves the
//! wiring with a Rust client. This file proves the PAGE: headless Chromium loads
//! the display, the page's own script joins the topic on the socket it already
//! holds, a fake microphone is opened through `getUserMedia`, an `AudioWorklet`
//! cuts the frames, and the transcript appears on the button's own line.
//!
//! Nothing is installed for it. `workshop/tools/display-mic-browser.mjs` speaks
//! CDP over the WebSocket Node has had since 22, and the browser is the one
//! Playwright already put in this host's cache. Without either, the script says
//! `SKIP` and this test passes — the same tool guard every other one in this
//! tree uses (R2b).
//!
//! The boot is copied from the file beside it rather than shared: two proofs that
//! drift apart are two findings, and a helper both of them lean on is one place
//! where a mistake hides from both.

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

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the frame length the wire recommends.
const FRAME_BYTES: usize = 640;
/// Enough audio for the scripted recogniser to answer: the button is held for
/// 600 ms, so a browser at 16 kHz sends about thirty 20 ms frames.
const FRAMES: usize = 20;
/// What the scripted recogniser says once the audio really arrived.
const SAID: &str = "what time is it";
/// What the test speaks back into the call.
const ANSWER: &str = "It is noon.";
/// How long the browser driver may take, all in.
const DRIVER_LIMIT: Duration = Duration::from_secs(90);
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
    _stt: MockDeepgram,
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
    // The display refs `web@2.0.0`, and a ref resolves against the templates
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
        _stt: stt_fake,
        _tts: tts_fake,
    }
}

fn hop(m: &Message, key: &str) -> Option<String> {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Wait until the screen serves a page with the microphone on it.
///
/// A `web` cell answers its own seeded demo page from the moment it binds, and
/// that page has no microphone: the button is an object the compose cell writes,
/// so the screen has to have composed once. One `in_view` is what makes it.
async fn wait_for_the_microphone(port: u16) {
    let url = format!("http://127.0.0.1:{port}/{SCREEN_MOUNT}/");
    let deadline = Instant::now() + MARKER;
    loop {
        if let Ok(r) = reqwest::get(&url).await
            && r.status().is_success()
            && r.text()
                .await
                .unwrap_or_default()
                .contains("phx-hook=\"DisplayMic\"")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the screen never published a page with the microphone on it"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One view, so the screen composes and publishes its own page.
async fn put_a_view_up(h: &ColonyHandle) {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".to_string(), json!("in_view"));
    h.send(
        MessageBuilder::new(Path::new("/screen"))
            .hop(hop)
            .reply_to(Path::new("/somebody"))
            .body(Body::Inline(json!({
                "messages": [],
                "view_id": "hello",
                "kind": "prose",
                "content": {"title": "Hello", "body": "the screen is up"}
            })))
            .build(),
    )
    .await;
}

/// The one thing the browser cannot do for itself: speak the answer into the
/// call, from the topology side, as soon as the turn is on the lane.
///
/// It runs beside the driver rather than before it, because the turn only exists
/// once the page has actually sent its audio.
async fn answer_when_the_turn_lands(
    inbox: mpsc::Sender<ColonyMsg>,
    mut egress: mpsc::Receiver<Message>,
) {
    let deadline = Instant::now() + DRIVER_LIMIT;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        // Three outcomes, three answers. Folding them into one `continue` made a
        // closed channel a hot loop -- `recv()` on one returns `None` at once, so
        // the loop would peg a core until the deadline, and a spinning core on
        // this host is what trips a live colony's watchdog.
        let m = match tokio::time::timeout(left, egress.recv()).await {
            Ok(Some(m)) => m,
            // The colony closed its egress; nothing more will ever come.
            Ok(None) => return,
            // The window is over.
            Err(_) => return,
        };
        if hop(&m, "route").as_deref() != Some("turn") {
            continue;
        }
        let Some(call) = hop(&m, "call_id") else {
            continue;
        };
        let mut context = meclaw_core::serde_json::Map::new();
        context.insert("call_id".to_string(), meclaw_core::serde_json::json!(call));
        let mut speak_hop = meclaw_core::serde_json::Map::new();
        speak_hop.insert(
            "route".to_string(),
            meclaw_core::serde_json::json!("in_speak"),
        );
        // The same injection `ColonyHandle::send` makes: a `Route` from the
        // root, because nothing here resolves a relative target.
        let _ = inbox
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/voice"))
                    .context(context)
                    .hop(speak_hop)
                    .body(Body::Inline(json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": ANSWER}]
                    })))
                    .build(),
            })
            .await;
        return;
    }
}

/// Drive the page once and hand back the driver's counter line, or `None` when
/// this host has nothing to drive it with.
///
/// `mode` is the driver's third argument: `None` for the plain run, `"rejoin"`
/// for the one that lets the cell close the topic in between.
async fn drive(live: &mut Live, mode: Option<&str>) -> Option<String> {
    put_a_view_up(&live.h).await;
    wait_for_the_microphone(live.port).await;

    let script = repo("workshop/tools/display-mic-browser.mjs");
    if !script.is_file() {
        println!("SKIP the driver does not ship in this tree");
        return None;
    }

    // The answer goes in as soon as the turn comes out, so the page has a
    // `speak_end` to count before the driver reads its counters.
    let inbox = live.h.inbox_tx.clone();
    let egress = std::mem::replace(&mut live.egress, mpsc::channel(1).1);
    let speaking = tokio::spawn(answer_when_the_turn_lands(inbox, egress));

    let mut command = tokio::process::Command::new("node");
    command
        .arg(&script)
        .arg(format!("http://127.0.0.1:{}/{SCREEN_MOUNT}/", live.port))
        .arg(SAID);
    if let Some(mode) = mode {
        command.arg(mode);
    }
    // The timeout path panics and drops this future, which does NOT kill the
    // child on its own -- node would keep running and the browser with it. That
    // is the leak the 90 s bound exists to prevent, so the bound has to take the
    // process with it. Node kills the browser itself, by PID, on every exit path
    // it now has.
    let run = tokio::time::timeout(DRIVER_LIMIT, command.kill_on_drop(true).output()).await;
    speaking.abort();

    let out = match run {
        Err(_) => panic!("the browser driver did not finish within {DRIVER_LIMIT:?}"),
        // No node on this host is the same kind of absence as no browser.
        Ok(Err(e)) => {
            println!("SKIP node is not on this host: {e}");
            return None;
        }
        Ok(Ok(out)) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    match out.status.code() {
        // The script already writes its own `SKIP <reason>`; repeating the word
        // would print it twice.
        Some(3) => {
            println!("{}", stderr.trim());
            return None;
        }
        Some(0) => {}
        other => panic!("the browser driver failed ({other:?}):\n{stdout}\n{stderr}"),
    }

    let line = stdout
        .lines()
        .find(|l| l.starts_with("BROWSER "))
        .unwrap_or_else(|| panic!("the driver printed no counters:\n{stdout}\n{stderr}"))
        .to_string();
    println!("{line}");
    Some(line)
}

/// One number out of a `BROWSER …` line.
fn counter(line: &str, key: &str) -> u64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_holds_the_button_and_the_colony_answers() {
    if !library_ships() || !have_python() {
        return;
    }
    let mut live = boot().await;
    let Some(line) = drive(&mut live, None).await else {
        return;
    };
    assert!(
        counter(&line, "sent=") > 0,
        "the page's own worklet cut frames and pushed them: {line}"
    );
    assert!(
        counter(&line, "turns=") >= 1,
        "the transcript the page shows came back over its own socket: {line}"
    );
}

/// GH #658 — and the same page, after the cell closed its topic underneath it.
///
/// A `4409` is a real close from the cell: a second link claiming the same call
/// displaces the first. The page used to disable its button on it, for the life
/// of the page. Now the next press joins a NEW call, and this is the only place
/// that can prove it: the vendored Phoenix client refuses a second `join()` on
/// the same channel instance, so the rejoin is a browser fact, not a string in
/// a rendered page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_rejoins_after_the_cell_closed_the_topic() {
    if !library_ships() || !have_python() {
        return;
    }
    let mut live = boot().await;
    let Some(line) = drive(&mut live, Some("rejoin")).await else {
        return;
    };
    assert!(
        line.contains("closed=4409"),
        "the cell really closed the first topic, and the page saw the code: {line}"
    );
    assert!(
        counter(&line, "turns=") >= 2,
        "one turn before the close and one after it, on a call that did not \
         exist when the page was loaded: {line}"
    );
}
