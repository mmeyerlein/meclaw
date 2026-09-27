//! GH #867 — no page the tree ships carries an inline script that is not listed in a
//! published `csp.json`, and none loads from `blob:`.
//!
//! A proxy in front of a colony may set a Content-Security-Policy of `script-src 'self'`
//! without `'unsafe-inline'`, `'unsafe-eval'` or `blob:`. Under that policy an inline
//! script runs only when its sha256 is listed, and a `blob:` URL never loads. Before this
//! issue three surfaces broke it:
//!
//! * the `web` cell's page shell booted LiveView from an inline block whose text held the
//!   socket URL -- proxy prefix plus mount -- so its hash was different on every
//!   deployment path and nobody could list it once;
//! * the display microphone and the voice test page installed their `AudioWorklet` from
//!   a `blob:` URL made from a string;
//! * the voice test page was one inline module script and one inline style.
//!
//! What this file pins, over HTTP against the real listener: the shell alone carries no
//! inline script; a display page carries only the inline scripts whose hashes
//! `templates/display/csp.json` lists; the voice test page carries none; and the hashes in
//! `csp.json` are the sha256 of the two constants `compose.py` ships (`SCENE_CLIENT_JS`,
//! `OS_CLIENT_JS`). Whether a real engine runs the page under that policy is the browser
//! lock beside this one (`gh867_a_display_runs_under_a_strict_csp_browser.rs`).
//!
//! The hashes are computed by `python3` (`hashlib`): no crate in the workspace provides a
//! sha256, and this test adds no dependency to get one. Without python or without the
//! template library the display half says `SKIP` and passes (R2b); the shell half needs
//! neither and always runs.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::sync::Arc;
use std::time::{Duration, Instant};

use display_colony::{Boot, boot_with_voice_echo, have_python, library_ships, repo};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
use meclaw_testing::{surface_listener, wait_for_mount};
use tokio::sync::mpsc;

/// The app whose window makes the screen compose its page.
const APP: &str = "/alex/apps/note";

/// The inline scripts of a page: the text of every `<script>` whose opening tag carries
/// no `src`. A `<script src=…>` is a file from the page's own origin and `'self'` admits
/// it; everything else needs a hash.
fn inline_scripts(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("<script") {
        let after = &rest[at..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let open = &after[..open_end];
        let body_start = open_end + 1;
        let Some(close) = after[body_start..].find("</script>") else {
            break;
        };
        if !open.contains(" src=") {
            out.push(after[body_start..body_start + close].to_string());
        }
        rest = &after[body_start + close..];
    }
    out
}

/// The CSP source expression for each text: `'sha256-<base64>'`, as a browser hashes an
/// inline script (the UTF-8 bytes of its text).
fn hashes(texts: &[String]) -> Vec<String> {
    let script = "import base64, hashlib, json, sys\n\
                  print(json.dumps([\"'sha256-\" + base64.b64encode(hashlib.sha256(t.encode('utf-8')).digest()).decode() + \"'\" for t in json.load(sys.stdin)]))\n";
    run_python(script, &json!(texts))
        .as_array()
        .expect("a list of hashes")
        .iter()
        .map(|v| v.as_str().expect("a hash").to_string())
        .collect()
}

/// Run `script` with `input` as JSON on stdin and read JSON back from stdout.
fn run_python(script: &str, input: &Value) -> Value {
    use std::io::Write;
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .current_dir(repo("templates/display/compose"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3 starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("python3 ends");
    assert!(
        out.status.success(),
        "python3 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("python3 printed JSON")
}

/// `templates/display/csp.json`, the policy fragment the display publishes.
fn csp_json() -> Value {
    let p = repo("templates/display/csp.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "{}: {e} -- the display publishes no policy fragment, so a proxy has no \
             hashes to list for its inline hook scripts",
            p.display()
        )
    });
    meclaw_core::serde_json::from_str(&raw).expect("csp.json is JSON")
}

fn listed(csp: &Value, directive: &str) -> Vec<String> {
    csp[directive]
        .as_array()
        .unwrap_or_else(|| panic!("csp.json has no `{directive}` list: {csp}"))
        .iter()
        .map(|v| v.as_str().expect("a source expression").to_string())
        .collect()
}

/// GET until the listener answers with something other than `503 starting`.
async fn get_text(url: &str) -> (u16, String, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(r) = reqwest::get(url).await
            && r.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE
        {
            let status = r.status().as_u16();
            let ctype = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            return (status, ctype, r.text().await.expect("a body"));
        }
        assert!(Instant::now() < deadline, "{url} never answered");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Seed one plain page, so the shell has something to wrap (the `pages` table is the
/// only route source).
fn seed_one_page(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"page","template":"<h1>{{body}}</h1>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("write components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root","parent":null,"component":"page","ord":0,"props":"{\"body\":\"hello\"}"}"#,
            "\n"
        ),
    )
    .expect("write objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root","title":"Home"}"#,
            "\n"
        ),
    )
    .expect("write pages");
}

/// The shell alone: a `web` cell with a plain page and nothing of a display in it.
///
/// Driven through the factory the way `web_cell_serves.rs` does, so it needs neither the
/// template library nor python and runs in every tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_web_shell_carries_no_inline_script() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({"mount": "screen"}),
            out_tx,
            cell_dir.clone(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a web cell spawns");
    let SpawnedCellKind::Active {
        join,
        sender: _sender,
        stop_tx: _stop_tx,
        ..
    } = spawned
    else {
        panic!("a web cell spawns Active");
    };
    wait_for_mount(&surfaces, "screen").await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;

    let (status, _, page) = get_text(&format!("http://{addr}/screen/")).await;
    assert_eq!(status, 200, "the page answers: {page}");
    assert!(
        page.contains("<h1>hello</h1>"),
        "and it is the seeded page: {page}"
    );
    let inline = inline_scripts(&page);
    assert!(
        inline.is_empty(),
        "the shell carries inline script, whose hash would have to be listed per \
         deployment path: {inline:?}"
    );
    assert!(
        !page.contains("blob:") && !page.contains("createObjectURL"),
        "and nothing on it loads from an object URL: {page}"
    );
    assert!(
        page.contains("<meta name=\"meclaw-live\" content=\"/screen/live\">")
            && page.contains("src=\"/screen/@client/boot.js\""),
        "the socket URL is data the boot file reads: {page}"
    );

    // The two client files of our own, from the page's origin, with a script type.
    for (file, needle) in [
        ("boot.js", "meta[name=meclaw-live]"),
        ("display-mic-worklet.js", "registerProcessor('mic',P)"),
    ] {
        let (status, ctype, body) = get_text(&format!("http://{addr}/screen/@client/{file}")).await;
        assert_eq!(status, 200, "{file} is served");
        assert!(
            ctype.starts_with("text/javascript"),
            "{file} is a script: {ctype}"
        );
        assert!(body.contains(needle), "{file} is the file it names: {body}");
    }

    join.abort();
    listener.abort();
}

/// The display and the voice test page, on one colony, and the constants behind them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_inline_script_of_the_display_is_listed_and_the_voice_page_has_none() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 in this tree");
        return;
    }
    let csp = csp_json();
    let script_src = listed(&csp, "script-src");

    let colony = boot_with_voice_echo(Boot::default()).await;
    // One window, so the screen composes and publishes its own page with both hooks.
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "system", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;

    // Every exit and the switch: the switch carries the scene hook too, because the
    // switch IS the scene hook's first decision (display-hive.md § 6.5).
    let mut seen = Vec::new();
    for exit in ["monitor", "phone", "tv", ""] {
        let page = colony.page(exit).await;
        assert!(
            page.contains("phx-hook=\"DisplayScene\"") || page.contains("data-switch"),
            "the {exit:?} exit serves the composed screen: {page}"
        );
        assert!(
            !page.contains("blob:") && !page.contains("createObjectURL"),
            "the {exit:?} exit loads nothing from an object URL"
        );
        let inline = inline_scripts(&page);
        for (text, hash) in inline.iter().zip(hashes(&inline)) {
            assert!(
                script_src.contains(&hash),
                "the {exit:?} exit carries an inline script whose hash {hash} is not in \
                 templates/display/csp.json ({script_src:?}); it starts: {:?}",
                text.chars().take(120).collect::<String>()
            );
            seen.push(hash);
        }
    }
    // Both published hashes are really on a page: a list that names a script nobody ships
    // would admit whatever happens to hash to it.
    let monitor = colony.page("monitor").await;
    let on_monitor = hashes(&inline_scripts(&monitor));
    assert_eq!(
        on_monitor.len(),
        2,
        "the monitor page carries exactly the scene hook and the microphone hook inline"
    );
    for hash in script_src.iter().filter(|s| s.starts_with("'sha256-")) {
        assert!(
            on_monitor.contains(hash),
            "csp.json lists {hash}, and no script on the monitor page hashes to it"
        );
    }
    assert!(
        !seen.is_empty(),
        "the display pages carry their hooks at all"
    );

    // The voice test page on the same listener: no inline script, no object URL, and its
    // three files beside it.
    let voice = format!("http://127.0.0.1:{}/voice", colony.port);
    let (status, _, page) = get_text(&format!("{voice}/")).await;
    assert_eq!(status, 200, "the voice test page answers");
    assert!(
        inline_scripts(&page).is_empty(),
        "the voice test page carries inline script: {page}"
    );
    assert!(
        !page.contains("<style"),
        "and an inline style block: {page}"
    );
    for file in ["test.js", "test.css", "worklet.js"] {
        let (status, _, body) = get_text(&format!("{voice}/{file}")).await;
        assert_eq!(status, 200, "{file} is served beside the page");
        assert!(
            !body.contains("createObjectURL") && !body.contains("new Blob"),
            "{file} builds no object URL"
        );
    }

    colony.shutdown().await;
}

/// The hashes `csp.json` publishes are the sha256 of the constants `compose.py` ships,
/// and the fragment is the one a proxy can take as it stands.
#[test]
fn the_published_hashes_are_the_shipped_constants() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 in this tree");
        return;
    }
    let consts = run_python(
        "import importlib.util, json, sys\n\
         spec = importlib.util.spec_from_file_location('compose', 'compose.py')\n\
         m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)\n\
         print(json.dumps([m.SCENE_CLIENT_JS, m.OS_CLIENT_JS]))\n",
        &json!(null),
    );
    let consts: Vec<String> = consts
        .as_array()
        .expect("two constants")
        .iter()
        .map(|v| v.as_str().expect("a script").to_string())
        .collect();
    let want = hashes(&consts);
    let csp = csp_json();
    assert_eq!(
        listed(&csp, "script-src"),
        vec!["'self'".to_string(), want[0].clone(), want[1].clone()],
        "script-src is 'self' plus the hashes of SCENE_CLIENT_JS and OS_CLIENT_JS, in that \
         order -- recompute them after every change of either hook script"
    );
    assert_eq!(listed(&csp, "style-src"), vec!["'self'", "'unsafe-inline'"]);
    assert_eq!(listed(&csp, "img-src"), vec!["'self'", "data:"]);
    assert_eq!(listed(&csp, "connect-src"), vec!["'self'"]);
    assert_eq!(listed(&csp, "base-uri"), vec!["'self'"]);
    for (i, directive) in csp.as_object().expect("an object").iter().enumerate() {
        let sources = directive.1.as_array().expect("a list");
        for s in sources {
            let s = s.as_str().expect("a string");
            assert!(
                s != "'unsafe-eval'"
                    && s != "blob:"
                    && !(directive.0 == "script-src" && s == "'unsafe-inline'"),
                "directive {i} ({}) admits {s}",
                directive.0
            );
        }
    }
    // And the microphone hook loads its worklet by URL, from the web cell's client.
    let os = &consts[1];
    assert!(
        os.contains("@client/display-mic-worklet.js") && os.contains("document.baseURI"),
        "the microphone hook loads its worklet from the web cell's client"
    );
    assert!(
        !os.contains("createObjectURL") && !os.contains("new Blob"),
        "and builds no object URL for it"
    );
}
