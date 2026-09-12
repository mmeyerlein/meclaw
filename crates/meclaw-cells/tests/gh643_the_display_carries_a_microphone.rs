//! GH #643 — `display@2.1.0` carries a microphone, on the shipped bytes.
//!
//! The whole template boots: the compose cell out of `params.script_inline`, the
//! store beside it, and a real `web` cell serving the page under its mount. One
//! `in_view` is enough to make the screen publish, and what this file reads is
//! the page a browser would get.
//!
//! Three things have to be on it, and none of them is a screenshot: the hook the
//! LiveView boot looks for, the mount the button will join, and the topic prefix
//! the client builds its channel name from. Everything the button then DOES is
//! measured where it can be measured — over the socket in
//! `gh643_audio_in_the_display_window.rs`, and in a real browser in the file
//! beside it.
//!
//! The mount is asserted against a NON-default name. Checking for `voice` would
//! pass a compose cell that never read its params at all, because the module
//! default is that same word — so this screen is given `voice_mount: "phone"` and
//! the page has to carry it.
//!
//! Guarded like every template-reading test (GH #49): a tree without the library
//! is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

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
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("json"),
    )
    .expect("write");
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

fn factories(
    surfaces: &Arc<meclaw_colony::SurfaceRegistry>,
) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        (
            "web".to_string(),
            Arc::new(WebCellFactory::new(Arc::clone(surfaces))),
        ),
    ]
}

/// GET the screen's route until it answers with a page carrying `wanted`.
async fn wait_for_page(port: u16, wanted: &str) -> String {
    let url = format!("http://127.0.0.1:{port}/{SCREEN_MOUNT}/");
    let deadline = Instant::now() + MARKER;
    let mut last = String::new();
    loop {
        if let Ok(r) = reqwest::get(&url).await {
            let status = r.status();
            last = r.text().await.unwrap_or_default();
            if status.is_success() && last.contains(wanted) {
                return last;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the screen never served a page carrying {wanted:?}; last body was:\n{last}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The mount this screen is pointed at: deliberately not the default.
const MOUNT: &str = "phone";

/// The name the screen itself answers to on the colony's one listener. Not the
/// same word as `MOUNT` above, and deliberately so: the screen's own name and
/// the name of the voice cell it speaks to are two different things.
const SCREEN_MOUNT: &str = "display";

/// The shipped screen, and the page it publishes when somebody writes on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_carries_the_hook_the_mount_and_the_topic() {
    if !library_ships() || !have_python() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());

    write_json(&root.join("colony.json"), &json!({"schema_version": 1}));
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./screen",
                 "condition": "has(hop.route) && hop.route == 'in_view'"}
            ]}}
        }),
    );
    copy_tree(&repo("templates/display"), &root.join("main/screen"));
    // The display refs `web@2.0.1`, so the template it grows from has to be on
    // disk before the boot resolves the ref (GH #424).
    copy_tree(&repo("templates/web"), &root.join("templates/web"));
    patch(&root.join("main/screen/web/config.json"), |v| {
        v["override_params"][""]["mount"] = json!(SCREEN_MOUNT)
    });
    // The one new mechanism of the task, pointed somewhere else on purpose: a
    // screen may be wired to a `voice` cell that was mounted under another name.
    patch(&root.join("main/screen/compose/config.json"), |v| {
        v["params"]["voice_mount"] = json!(MOUNT)
    });

    let h = ColonyHandle::new_with_factories_at(&td, factories(&surfaces));
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories(&surfaces) {
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
        .expect("the screen boots");

    // One view, so the screen has something to publish. The microphone does not
    // depend on it — it hangs under the root — but a page does.
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

    // Waited for the VIEW and not for the button, though the button is what this
    // test is about: the screen publishes as its bundle is applied, so a `GET`
    // between two legs of one bundle can see the microphone without the view that
    // made it publish. The view is the last object written, so waiting for it
    // waits for the whole page. (Measured under a loaded gate runner: the other
    // order is a flake.)
    wait_for_mount(&surfaces, SCREEN_MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let page = wait_for_page(addr.port(), "the screen is up").await;
    assert!(
        page.contains("phx-hook=\"DisplayMic\""),
        "the hook the LiveView boot looks for is on the page:\n{page}"
    );
    assert!(
        page.contains(&format!("data-mount=\"{MOUNT}\"")),
        "the button names the mount the compose cell was CONFIGURED with, which \
         is the only thing that proves the param is read at all:\n{page}"
    );
    assert!(
        !page.contains("data-mount=\"voice\""),
        "and not the module default it would fall back to:\n{page}"
    );
    assert!(
        page.contains("voice:"),
        "the client builds its topic from the call and this prefix:\n{page}"
    );
    assert!(
        page.contains("SurfaceHooks"),
        "the hook has to be on `window.SurfaceHooks` before the socket boot \
         reads it, which is why the script rides in the dead render:\n{page}"
    );
    assert!(
        page.contains("hold to talk"),
        "a person has to be able to see what the button is for:\n{page}"
    );

    // GH #658 — the three states the button owes a person, on the shipped
    // bytes. Each one is a moment that used to pass in silence, and silence on
    // a screen is indistinguishable from a screen that is broken.
    assert!(
        page.contains("asking for the microphone"),
        "the permission prompt opens INSIDE the gesture, so the page has to say \
         that it is waiting for an answer:\n{page}"
    );
    assert!(
        page.contains("press again"),
        "a press that ended before the permission arrived is a press that did \
         nothing, and the person is the only one who can repeat it:\n{page}"
    );
    assert!(
        page.contains("\"closed \" + c.code; joined = false;"),
        "a close ends the CALL: the hook notes it and lets the next press join \
         a new one:\n{page}"
    );
    assert!(
        !page.contains("\"closed \" + c.code; btn.disabled = true;"),
        "and the close handler must not be what kills the button — that is the \
         one-minute fuse a screen nobody spoke into used to run into:\n{page}"
    );
    assert!(
        page.contains("if (!joined && !(await join()))"),
        "the rejoin hangs on the press, not on a timer — and it waits for the \
         join to be acknowledged before the hold goes out: a text push is \
         buffered until then and a raw binary push is not, so audio sent in \
         that window is dropped or arrives ahead of its own hold:\n{page}"
    );
    assert!(
        page.contains("|| \"refused\"; btn.disabled = true;"),
        "a REFUSED join stays final: that answer is about this screen and this \
         mount, and pressing again cannot change it:\n{page}"
    );
    assert!(
        page.contains("state.textContent = \"listening"),
        "and the waiting sentence goes when the hold begins — a screen that \
         still says it is asking for a microphone it already has is the same \
         lie as one that said nothing:\n{page}"
    );
    assert!(
        page.contains("function sendAudio(ab) { if (!joined) return;"),
        "and no frame leaves for a topic this page is not on:\n{page}"
    );
    listener.abort();
    h.shutdown().await;
}

/// The version the screen ships as, and the sentence that says what changed.
#[test]
fn the_template_says_it_carries_a_microphone() {
    if !library_ships() {
        return;
    }
    let template = read_json(&repo("templates/display/template.json"));
    assert_eq!(
        template["version"], "2.1.0",
        "the screen shipped the microphone at 1.2.0 — a new component is a \
         minor version — moved to 2.0.0 when its own port went with \
         `web@2.0.0`, to 2.0.1 for what the button says while it waits \
         (GH #658), which is a repair, and to 2.1.0 for the design language, \
         the catalogue and `params.font_base` (GH #669, #670, #672), which a \
         caller can name — a minor version again"
    );
    let purpose = template["description"]["purpose"]
        .as_str()
        .expect("a purpose");
    assert!(
        purpose.contains("Since 1.2.0") && purpose.contains("voice_mount"),
        "the purpose says what a caller has to configure for the button to \
         reach anything: {purpose}"
    );
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(
        readme.starts_with("# `display@2.1.0`"),
        "the README heads with the version it describes"
    );
    assert!(
        readme.contains("Talking to the screen"),
        "and says how a person speaks to it"
    );
}

/// The source and the runtime copy of it are the same bytes.
///
/// `gh609_a_standing_view_keeps_its_place.rs` pins this too. It is repeated here
/// because this strand is the one that regenerates the copy, and a strand that
/// forgets to would otherwise ship a screen whose page has no microphone on it
/// while `compose.py` says it does.
#[test]
fn the_shipped_script_is_the_file_beside_it() {
    if !library_ships() {
        return;
    }
    let src =
        std::fs::read_to_string(repo("templates/display/compose/compose.py")).expect("compose.py");
    let cfg = read_json(&repo("templates/display/compose/config.json"));
    assert_eq!(
        cfg["params"]["script_inline"].as_str().expect("inline"),
        src,
        "script_inline has drifted from compose.py"
    );
    assert_eq!(
        cfg["params"]["voice_mount"], "voice",
        "the mount the button joins is a param of the compose cell, so one \
         screen can be pointed at another colony's voice cell"
    );
}
