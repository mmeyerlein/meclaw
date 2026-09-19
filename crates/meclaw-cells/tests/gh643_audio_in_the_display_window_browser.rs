//! GH #643 — the same call, driven by a real browser.
//!
//! The colony beside this one (`gh643_audio_in_the_display_window.rs`) proves the
//! wiring with a Rust client. This file proves the PAGE: headless Chromium loads
//! the display, the page's own script joins the topic on the socket it already
//! holds, a fake microphone is opened through `getUserMedia`, an `AudioWorklet`
//! cuts the frames, and the turn comes back on the page's own socket. Since
//! 2.3.0 the words are not written into the page (D-17); the hook keeps the
//! last turn's text on its counter object, and that is what the driver reads.
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

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
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
    /// The scripted recogniser, kept readable: the drain proof counts the
    /// bytes it saw against the milliseconds the button was held.
    stt: MockDeepgram,
    _tts: MockCartesia,
}

/// Boot the display, the voice cell and the two fakes into one colony.
///
/// The two factories share ONE registry, which is the whole point: the `web`
/// cell's socket loop finds the `voice` cell in it, and neither of them knows
/// anything about the other.
async fn boot() -> Live {
    boot_gated_on(FRAME_BYTES * FRAMES, 0).await
}

/// How much audio the scripted recogniser waits for before it ends the turn,
/// and the `release_grace_ms` the voice cell is armed with.
///
/// The existing proofs use the exact count they send and a grace of `0`; the
/// drain proof uses a share of what the button was held for, which is the loss
/// it measures, and needs the shipped grace because its end of turn comes only
/// AFTER the release.
async fn boot_gated_on(bytes: usize, grace_ms: u64) -> Live {
    // The recogniser says nothing until ALL the audio has arrived. That is what
    // makes the printed latency a number about the path rather than about
    // `release_grace_ms`: the provider's end of turn lands next to the release
    // instead of half a second before it, which is where a real one lands.
    let stt_fake = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(bytes)
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
    // The display refs `web@2.0.4`, and a ref resolves against the templates
    // table, which is empty until somebody fills it (GH #424).
    copy_tree(&repo("templates/web"), &root.join("templates/web"));
    patch(&root.join("main/screen/web/config.json"), |v| {
        v["override_params"][""]["mount"] = json!(SCREEN_MOUNT)
    });
    // A screen with a finger and an ear. The template ships ONE output, a
    // television, and a television has `inputs: []` at the door
    // (display-hive.md § 6.4, § 4.7: it is an output device -- hold and press
    // come from the phone or the laptop). Since display 2.5.0 the client binds
    // only what the profile names, so a driver that presses the mark has to
    // press it on an output that can be pressed.
    patch(&root.join("main/screen/compose/config.json"), |v| {
        v["params"]["screens"] = json!({"monitor": {
            "display_type": "monitor", "viewing_distance_m": 0.7,
            "inputs": ["audio", "pointer", "keyboard"],
            "dock_default": "shown", "dock_max": 8,
        }});
        v["params"]["default_screen"] = json!("monitor");
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
                "release_grace_ms": grace_ms,
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
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
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
    drive_with(live, mode, &[]).await
}

/// `drive`, with extra environment for the `node` process: the driver reads
/// `MECLAW_MIC_AUDIO` (a wav for the fake microphone) and `MECLAW_MIC_HOLD_MS`
/// from there, so a proof can choose what is spoken and for how long.
async fn drive_with(live: &mut Live, mode: Option<&str>, env: &[(&str, &str)]) -> Option<String> {
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
    for (key, value) in env {
        command.env(key, value);
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
        "the turn came back over the page's own socket: {line}"
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

/// GH #698 — the last words of a take leave the browser.
///
/// The fake microphone plays a four second file instead of a tone, the button
/// is held over its whole length and a little past it, and the recogniser ends
/// the turn only once it has seen 95 % of the milliseconds the button was down.
/// Before the drain window that count was never reached: `up()` lowered the
/// audio gate first, so everything still in the worklet's accumulator and in the
/// message port's queue was dropped — 300 to 600 ms, measured on a live screen
/// in 15 of 15 takes.
///
/// The structural assertion is `flushed`: it counts the frames that went out
/// AFTER the key came up, and it cannot be positive without the window,
/// whatever the host's load is. The byte bracket sits in the driver: it waits
/// for the turn to carry the scripted text, which the recogniser only says once
/// the 95 % have arrived, and without them the driver times out and this test
/// fails there. The share itself is printed either way.
///
/// The bracket only means anything while the bytes it counts belong to the
/// take. The ring in front of the press is exactly such a foreign source: a
/// driver that touches the mark and waits before pressing hands the colony
/// two thirds of a second of audio the button was never down for, and the
/// share went from 98.6 % to 114.3 % -- past the loss this test looks for,
/// which is 300 to 600 ms of 4200, or 7 to 14 %. So the driver does not touch
/// the mark when it is playing a file, and the ceiling below says so: what a
/// press with nothing in front of it arms is the threshold window alone, about
/// twelve 20 ms frames, where a touch 400 ms early makes it about thirty-three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_release_drains_before_it_lets_go() {
    /// 16 kHz mono PCM16: 32 bytes per millisecond.
    const BYTES_PER_MS: usize = 32;
    /// The file is 4.0 s; held 200 ms past its end, as a person does.
    const HOLD_MS: usize = 4_200;
    /// What has to arrive for the turn to be released. The loss this test is
    /// about is 300–600 ms, so 95 % is above the noise and below the defect.
    const WANT: usize = HOLD_MS * BYTES_PER_MS * 95 / 100;

    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 on this host");
        return;
    }

    // Derive the 16 kHz fixture into a temp dir: the tree carries the 8 kHz
    // master only, and everything else is derived from it (`make_fixtures.py`).
    let td = tempfile::TempDir::new().expect("tempdir");
    let made = std::process::Command::new("python3")
        .arg(repo("workshop/voice-smoke/make_fixtures.py"))
        .arg("--raise-only")
        .arg("--only")
        .arg("f01")
        .arg("--out-dir")
        .arg(td.path())
        .output();
    let wav = td.path().join("f01-8k-up16k.wav");
    match made {
        Ok(out) if out.status.success() && wav.is_file() => {}
        // No python, or a fixture that cannot be derived, is the same kind of
        // absence as no browser: reported, not failed.
        other => {
            println!("SKIP the 16 kHz fixture could not be derived: {other:?}");
            return;
        }
    }

    let mut live = boot_gated_on(WANT, 1_500).await;
    let Some(line) = drive_with(
        &mut live,
        None,
        &[
            ("MECLAW_MIC_AUDIO", wav.to_string_lossy().as_ref()),
            ("MECLAW_MIC_HOLD_MS", &HOLD_MS.to_string()),
        ],
    )
    .await
    else {
        return; // SKIP: no node, no browser
    };

    // The measurement first, so it is on record whichever way the verdict goes.
    let seen = live.stt.received_audio_bytes();
    println!(
        "DRAIN held={HOLD_MS}ms want={WANT}B seen={seen}B ({:.1} %)",
        seen as f64 * 100.0 / (HOLD_MS * BYTES_PER_MS) as f64
    );
    assert!(
        counter(&line, "flushed=") >= 1,
        "the drain window sent nothing after the key came up: {line}"
    );
    assert!(
        counter(&line, "prebuffered=") <= 20,
        "nothing but the threshold window sits in front of this take, or the \
         bytes above are not a measurement of the hold any more: {line}"
    );
}

/// Welle F: the threshold, in a real engine. A press under 250 ms sends no
/// `hold` and switches the dock once; a press over it sends the hold and the
/// touch on the chat, and leaves the dock alone -- including through the
/// `click` a browser appends to the gesture.
///
/// That last clause is a second way into the dock, and the handles alone never
/// reach it: they are what a `pointerup` calls, and a click is a separate
/// event with its own listener. So the driver dispatches a real one on the mark
/// after the long press, inside the grace the check is armed for, and
/// `after_click` is what the dock counter says afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_press_opens_the_dock_and_a_long_one_speaks() {
    if !library_ships() || !have_python() {
        return;
    }
    let mut live = boot().await;
    let Some(line) = drive_with(&mut live, Some("tap"), &[]).await else {
        return;
    };
    // `drive_with` already printed the line; one copy is the record.
    assert_eq!(
        counter(&line, "taps="),
        1,
        "the long press did not switch the dock a second time: {line}"
    );
    assert!(
        counter(&line, "touches=") >= 1,
        "and it touched the chat (R-23-5): {line}"
    );
    assert!(
        counter(&line, "hold_ms=") >= 250,
        "the take started at the threshold, not before it: {line}"
    );
    assert_eq!(
        counter(&line, "after_click="),
        1,
        "and the click the browser appends to a press that spoke switched \
         nothing on top of it: {line}"
    );
}

/// Welle F: what was said before the hold reaches the colony (E-3, OR-F25).
///
/// The driver touches the mark, waits, and only then presses: that is what a
/// finger does, and it is the one way to make the ring hold anything. What
/// comes back has to show both halves — a ring that had frames in it, and a
/// take that still became a turn with those frames in front of it.
///
/// Both halves are measured rather than asserted away. `prebuffered` is read
/// on the page BEFORE the ring is emptied, so any positive number would also
/// be reached by a press that armed nothing but its own threshold window: 250
/// ms of 20 ms frames is about twelve. The touch is 400 ms ahead of the press,
/// which makes 650 ms, about thirty-two, and thirty-three measured — so the
/// threshold below is 15, above what the press alone can reach and well under
/// what the touch produces. And a count on the page is not an arrival, so the
/// second half asks the recogniser how much audio it really saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ring_sends_what_it_kept_before_the_hold() {
    /// 16 kHz mono PCM16: 32 bytes per millisecond.
    const BYTES_PER_MS: usize = 32;
    /// How long the button is held, and therefore what the take alone is worth.
    const HOLD_MS: usize = 900;

    if !library_ships() || !have_python() {
        return;
    }
    let mut live = boot().await;
    let Some(line) = drive_with(
        &mut live,
        None,
        &[("MECLAW_MIC_HOLD_MS", &HOLD_MS.to_string())],
    )
    .await
    else {
        return;
    };
    assert!(
        counter(&line, "prebuffered=") >= 15,
        "the ring held more than a press could have put in it: {line}"
    );
    assert!(
        counter(&line, "turns=") >= 1,
        "and the take still became a turn: {line}"
    );
    // The arrival, and it is on record whichever way the verdict goes. Live
    // audio cannot start before the threshold, so a take with nothing kept in
    // front of it is worth LESS than the press: 650 ms of the 900, plus a
    // drain window. More than the whole press reached the colony only because
    // what the ring heard before the press went with it.
    let seen = live.stt.received_audio_bytes();
    println!(
        "RING held={HOLD_MS}ms press_worth={}B seen={seen}B",
        HOLD_MS * BYTES_PER_MS
    );
    assert!(
        seen > HOLD_MS * BYTES_PER_MS,
        "more audio reached the colony than the press itself is worth: \
         {seen} B for {HOLD_MS} ms"
    );
}
