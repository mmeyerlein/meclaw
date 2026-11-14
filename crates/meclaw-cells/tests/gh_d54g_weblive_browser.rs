//! d54g weblive -- the display page behind a live model, in a real browser.
//!
//! The duplex harness (`duplex/gh_d54g_weblive.rs`) proves the cell's half on
//! the page's own door with a Rust client. This file proves the PAGE's half:
//! headless Chromium loads the display, the page's own script joins its topic
//! on a `voice` cell whose provider hears and speaks on one socket (the
//! shipped loopback `echo`, which costs nothing and answers every frame with
//! itself), and the page does what contract point 5 and the hold section of
//! the display description (§ 5.4) ask of it:
//!
//! - **The early word.** The pointer comes onto the mark, and the page pushes
//!   `ready` -- before any press, so the `ready` leaves ahead of the `hold`
//!   (the cell may open the next session on it, `duplex.prewarm_ttl_ms`).
//! - **The model's voice.** What the model says comes back as audio on the
//!   page's own socket, and the page plays it.
//! - **Its line.** The colony gives the model a sentence to say into the page's
//!   session; the page's mark follows the model's spoken section
//!   (`speak_start`, `speak_end`), and the turn that carries the model's words
//!   leaves the cell under the page session's name, as a call's turn does --
//!   the line the chat application draws.
//!
//! A browser lock (OR-54g-21 j): `#[ignore]`d in the tree and named in
//! `BROWSER_LOCKS` of `scripts/gate_plan.py`, so it runs in the station that
//! books a browser and nowhere else. Without node or a browser the driver says
//! `SKIP` and the test passes, except under `MECLAW_REQUIRE_BROWSERS=1`, where
//! the station has booked the engine and a skip is red.
//!
//! The boot is the one of `gh643_audio_in_the_display_window_browser.rs`, with
//! the voice cell switched to the duplex loopback; copied rather than shared
//! for the reason that file gives.

#[path = "support/browser_run.rs"]
mod browser_run;

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
use meclaw_testing::deadline::{HANG, scaled};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// The name the screen answers to on the colony's one listener.
const SCREEN_MOUNT: &str = "display";
/// The output the proof drives (never the switch at `/<mount>/`, GH #763).
const EXIT: &str = "monitor";
/// The context key that opens the colony's egress door for this file's listener.
const MARK: &str = "voice_out";
/// What the colony gives the model to say into the page's session.
const ANSWER: &str = "It is noon.";
/// The driver's run marker in HANG marks: load, ring, take, the model's
/// section and its close -- each wait under one HANG mark of its own.
const RUN_HANGS: u32 = 6;
/// How long the take holds the mark, in ms of the page's own 20 ms frames
/// (the driver's `MECLAW_MIC_HOLD_MS`). The loopback meters its session once
/// per 50 frames it heard (`duplex_echo.rs` `ITEMS_PER_USAGE`), and that
/// `usage` emission is how this file learns the page session's name: the
/// loopback takes no `seed_wait_ms`, so no `seed_ask` ever leaves it. 1600 ms
/// is 80 frames, one reading with room to spare.
const HOLD_MS: u32 = 1_600;

/// Every template this file reads, spelled out so the export's R2b check sees
/// the names (GH #9).
const NEEDED: [&str; 3] = ["templates/display", "templates/web", "templates/voice"];

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

fn page_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/{SCREEN_MOUNT}/{EXIT}")
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

/// The colony this file measures, and the doors the test holds onto it.
struct Live {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    /// Everything the `voice` cell emitted that reached the root.
    egress: mpsc::Receiver<Message>,
    /// The port of the one listener in front of the whole colony.
    port: u16,
    _listener: tokio::task::JoinHandle<()>,
}

/// Boot the display and a duplex `voice` cell behind the loopback into one
/// colony: the shipped voice cell's own `config.json`, its stt/tts pair taken
/// out and `duplex` set to `echo`, connections in `hold`.
async fn boot() -> Live {
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
                {"from": "./voice", "to": ".", "modifier": {"set_context": mark}}
            ]}}
        }),
    );

    copy_tree(&repo("templates/display"), &root.join("main/screen"));
    copy_tree(&repo("templates/web"), &root.join("templates/web"));
    patch(&root.join("main/screen/web/config.json"), |v| {
        v["override_params"][""]["mount"] = json!(SCREEN_MOUNT)
    });
    // A screen with a finger and an ear (display-hive.md § 6.4): the shipped
    // output is a television, which takes no press.
    patch(&root.join("main/screen/compose/config.json"), |v| {
        v["params"]["screens"] = json!({"monitor": {
            "display_type": "monitor", "viewing_distance_m": 0.7,
            "inputs": ["audio", "pointer", "keyboard"],
            "dock_default": "shown", "dock_max": 8,
        }});
        v["params"]["default_screen"] = json!("monitor");
    });

    let mut voice = read_json(&repo("templates/voice/config.json"));
    {
        let params = voice["params"].as_object_mut().expect("params");
        params.remove("stt");
        params.remove("tts");
        params.insert("mount".into(), json!("voice"));
        params.insert("default_mode".into(), json!("hold"));
        params.insert(
            "duplex".into(),
            json!({"provider": "echo", "sample_rate": 16_000}),
        );
        params.insert("emit_usage".into(), json!(true));
    }
    write_json(&root.join("main/voice/config.json"), &voice);

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
    }
}

fn hop(m: &Message, key: &str) -> Option<String> {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
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

/// Wait until the screen serves a page with the microphone on it.
async fn wait_for_the_microphone(port: u16) {
    let url = page_url(port);
    meclaw_testing::wait::until_some(
        "the screen publishes a page with the microphone on it",
        || {
            let url = url.clone();
            async move {
                let r = reqwest::get(&url).await.ok()?;
                if !r.status().is_success() {
                    return None;
                }
                r.text()
                    .await
                    .ok()?
                    .contains("phx-hook=\"DisplayMic\"")
                    .then_some(())
            }
        },
    )
    .await;
}

/// What the colony saw of the page's session: its name (from `usage`) and
/// the first `turn` under that name that carries the model's words.
#[derive(Debug, Default)]
struct Seen {
    session: Option<String>,
    turn: Option<Value>,
}

/// The colony's side of the run: when the page's session reports its first
/// `usage`, give the model a sentence to say into that session; then wait for the
/// turn that carries it. Ends at the turn, at a closed egress or after
/// `within` (a hang mark, never a verdict: the test reads `Seen`).
async fn speak_into_the_session(
    inbox: mpsc::Sender<ColonyMsg>,
    mut egress: mpsc::Receiver<Message>,
    within: std::time::Duration,
) -> Seen {
    let until = Instant::now() + within;
    let mut seen = Seen::default();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let m = match tokio::time::timeout(left, egress.recv()).await {
            Ok(Some(m)) => m,
            Ok(None) | Err(_) => return seen,
        };
        let route = hop(&m, "route").unwrap_or_default();
        let call = hop(&m, "call_id");
        if route == "usage" && seen.session.is_none() {
            let Some(call) = call else { continue };
            seen.session = Some(call.clone());
            let mut context = meclaw_core::serde_json::Map::new();
            context.insert("call_id".to_string(), json!(call));
            let mut speak_hop = meclaw_core::serde_json::Map::new();
            speak_hop.insert("route".to_string(), json!("in_speak"));
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
        } else if route == "turn" && call.is_some() && call == seen.session {
            let Body::Inline(body) = &m.body else {
                continue;
            };
            let said = body["messages"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter(|t| t["origin"] == json!("assistant"))
                        .filter_map(|t| t["text"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default();
            if said.contains(ANSWER) {
                let mut turn = body.clone();
                turn["hop"] = json!(m.headers.hop);
                seen.turn = Some(turn);
                return seen;
            }
        }
    }
}

/// One number out of a `BROWSER …` line.
fn counter(line: &str, key: &str) -> i64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

/// The page behind a live model: it says `ready` before the hold, plays the
/// model's audio, and its mark follows the model's spoken section; the turn
/// with the model's words leaves the cell under the page session's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser lock: runs in the station browser:display (scripts/gate_plan.py BROWSER_LOCKS)"]
async fn gh_d54g_weblive_the_page_plays_the_models_audio_and_follows_its_line_browser() {
    let required = std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() == Ok("1");
    if !library_ships() {
        assert!(
            !required,
            "SKIP under the switch: the library does not ship in this tree"
        );
        return;
    }
    let mut live = boot().await;
    put_a_view_up(&live.h).await;
    wait_for_the_microphone(live.port).await;

    let script = repo("workshop/tools/display-mic-browser.mjs");
    if !script.is_file() {
        println!("SKIP the driver does not ship in this tree");
        assert!(
            !required,
            "SKIP under the switch: the driver does not ship in this tree"
        );
        return;
    }
    let run = scaled(HANG) * RUN_HANGS;
    let outside = browser_run::outside();
    let colony_side = tokio::spawn(speak_into_the_session(
        live.h.inbox_tx.clone(),
        std::mem::replace(&mut live.egress, mpsc::channel(1).1),
        run + outside,
    ));

    let mut command = tokio::process::Command::new("node");
    command
        .arg(&script)
        .arg(page_url(live.port))
        .arg(ANSWER)
        .arg("duplex")
        .env("MECLAW_MIC_HOLD_MS", HOLD_MS.to_string());
    let out = browser_run::run_marked(&mut command, "the duplex mic driver", run, outside)
        .await
        .unwrap_or_else(|hung| panic!("{hung}"));
    let out = match out {
        Err(e) => {
            colony_side.abort();
            println!("SKIP node is not on this host: {e}");
            assert!(!required, "SKIP under the switch: node is not on this host");
            return;
        }
        Ok(out) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    match out.status.code() {
        Some(3) => {
            colony_side.abort();
            assert!(
                !required,
                "a skip under MECLAW_REQUIRE_BROWSERS=1 is red:\n{stderr}"
            );
            println!("{}", stderr.trim());
            return;
        }
        Some(0) => {}
        other => {
            colony_side.abort();
            panic!("the browser driver failed ({other:?}):\n{stdout}\n{stderr}")
        }
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with("BROWSER "))
        .unwrap_or_else(|| panic!("the driver printed no counters:\n{stdout}\n{stderr}"))
        .to_string();
    println!("{line}");
    assert_eq!(
        counter(&line, "nav="),
        1,
        "the page navigated after it had loaded: {line}"
    );
    assert!(
        counter(&line, "readies=") >= 1,
        "the pointer on the mark is the page's early word: {line}"
    );
    assert_eq!(
        counter(&line, "ready_before_hold="),
        1,
        "the early word leaves ahead of the hold: {line}"
    );
    assert!(
        counter(&line, "played=") >= 1,
        "the page plays the model's audio: {line}"
    );
    assert!(
        counter(&line, "speak_start=") >= 1 && counter(&line, "speak_end=") >= 1,
        "the mark follows the model's spoken section: {line}"
    );

    let seen = colony_side.await.expect("the colony side ran");
    let session = seen
        .session
        .unwrap_or_else(|| panic!("HANG: the page's session never reported its usage: {line}"));
    let turn = seen.turn.unwrap_or_else(|| {
        panic!(
            "HANG: no turn with the model's words left the cell under {session} within {} s",
            (run + outside).as_secs()
        )
    });
    assert_eq!(
        turn["hop"]["engine"],
        json!("duplex"),
        "a page session's turn is a duplex turn: {turn}"
    );
    assert!(
        turn["hop"]["turn_id"]
            .as_str()
            .is_some_and(|id| id.starts_with(&format!("{session}#"))),
        "the turn is numbered within the page session: {turn}"
    );
    live.h.shutdown().await;
}
