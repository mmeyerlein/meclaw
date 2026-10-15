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
//! inline script; a display page -- with an application window AND the code view it shows
//! by default (`colony-view`) -- carries only the inline scripts whose hashes
//! `templates/display/csp.json` lists; the voice test page carries none. And, without a
//! listener, over the tree: every inline script any template ships is found (not named)
//! and its hash stands in the `csp.json` of the page that carries it --
//! `templates/display/csp.json` for the display's hooks and its code views,
//! `templates/canvy/csp.json` for the canvas. That reading of the source is the early
//! warning; the guarantee is the render lock: every template that composes a page (a `web`
//! cell) renders it the way a colony does -- the display with its code views, canvy's layout
//! into a real `web` cell, the `web` template on its seed -- and every inline script an HTML
//! parser finds on those pages has its hash in that template's `csp.json`. Whether a real
//! engine runs the display under that policy is the browser lock beside this one
//! (`gh867_a_display_runs_under_a_strict_csp_browser.rs`).
//!
//! The hashes are computed by `python3` (`hashlib`): no crate in the workspace provides a
//! sha256, and this test adds no dependency to get one. Without python or without the
//! template library the display half says `SKIP` and passes (R2b); the shell half needs
//! neither and always runs.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::sync::Arc;

use display_colony::{
    Boot, COLONY_VIEW, Site, boot_with_voice_echo, colony_view_emission, have_python,
    library_ships, repo,
};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
// Every page this file reads goes through ONE reader, `meclaw_testing::page_scripts`:
// python's HTML parser and a scan of the text, red where the two disagree, and red without
// python (re-check tv-bn M9f, A-M1). The string search that stood here (`find("<script")`)
// missed `<SCRIPT>`, `on*` handlers, `javascript:` and `<script/>` (re-check tv-bn M9e,
// B-M1).
use meclaw_testing::page_scripts::{assert_no_inline_code, read_pages};
use meclaw_testing::{surface_listener, wait_for_mount};
use tokio::sync::mpsc;

/// The app whose window makes the screen compose its page.
const APP: &str = "/alex/apps/note";

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

/// The display colony of the two page locks: linger and fade far past every hang mark.
///
/// These locks read the pages a window composes, not the curator's fade. With the helper's
/// defaults (2 s + 3 s, made for locks that wait FOR the fade) the note's presence ended
/// five seconds after its first pass; the stroke the clock orders for that moment took it
/// into its leaving pass and the next pass out of the tree. Under the 80 % quota at F=3 in
/// the full set (scan S3 on build03, again on build01) the settle between the store write
/// and the first tree read outlasted that, the window was never seen in the tree, and `put`
/// failed with `HANG: ... the window is not in the tree` (welle-val2 v2-fix-scan). The
/// shape of `gh867_a_display_runs_under_a_strict_csp_browser.rs` and gh767.
fn past_the_fade() -> Boot {
    Boot {
        linger_ms: 900_000,
        fade_ms: 900_000,
        ..Boot::default()
    }
}

/// The windows of the page locks outlive every wait they sit in: linger and fade each hold
/// three hang marks at the largest time factor, so no settle under any quota reaches the
/// fade (welle-val2 v2-fix-scan).
#[test]
fn the_page_locks_read_their_windows_before_any_fade() {
    let boot = past_the_fade();
    let longest = (meclaw_testing::deadline::HANG_MS as f64
        * meclaw_testing::deadline::MAX_TIME_FACTOR) as u64;
    assert!(
        boot.linger_ms >= 3 * longest && boot.fade_ms >= 3 * longest,
        "linger {} ms and fade {} ms must each hold three hang marks at the largest time \
         factor ({} ms): a shorter fade ends the window inside a settle under load",
        boot.linger_ms,
        boot.fade_ms,
        3 * longest
    );
}

/// GET until the listener answers with something other than `503 starting`.
///
/// Re-check tv-bn M7 M1 (TV-S3, TV-Z3): the kit's wait under the scaled hang mark (it was a
/// std `Instant` plus a literal 30 s and "never answered"); a hang names the last answer.
async fn get_text(url: &str) -> (u16, String, String) {
    meclaw_testing::wait::until_some_with(
        &format!("{url} answers past `503 starting`"),
        || async {
            let r = reqwest::get(url).await.ok()?;
            if r.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                return None;
            }
            let status = r.status().as_u16();
            let ctype = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            Some((status, ctype, r.text().await.expect("a body")))
        },
        || async {
            match reqwest::get(url).await {
                Ok(r) => format!("it answers {}", r.status()),
                Err(e) => format!("it does not answer: {e}"),
            }
        },
    )
    .await
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
    // An inline script on the shell would need its hash listed per deployment path.
    assert_no_inline_code("the web shell", &page);
    assert!(
        !page.contains("blob:") && !page.contains("createObjectURL"),
        "and nothing on it loads from an object URL: {page}"
    );
    assert!(
        page.contains("<meta name=\"meclaw-live\" content=\"/screen/live\">")
            && page.contains("src=\"/screen/@client/boot.js?v="),
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

/// The display, on one colony, and the constants behind them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_inline_script_of_the_display_is_listed() {
    if !library_ships() {
        println!("SKIP no template library in this tree");
        return;
    }
    // python3 reads every page here; a host without it is red, not a skip (re-check tv-bn
    // M9f, A-M1).
    assert!(
        have_python(),
        "python3 is required: a reader that cannot read is red"
    );
    let csp = csp_json();
    let script_src = listed(&csp, "script-src");

    let colony = boot_with_voice_echo(past_the_fade()).await;
    // One window, so the screen composes and publishes its own page with both hooks.
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "system", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    // And the code view the screen shows by default, as its layout emits it: its shell
    // brings colony-view's client as a third inline script (re-check tv-bn M8, M1).
    colony
        .put_emission(COLONY_VIEW, &colony_view_emission())
        .await;

    // Every exit and the switch: the switch carries the scene hook too, because the
    // switch IS the scene hook's first decision (display-hive.md § 6.5).
    let exits = ["monitor", "phone", "tv", ""];
    let mut pages = Vec::new();
    for exit in exits {
        pages.push(colony.page(exit).await);
    }
    let mut seen = Vec::new();
    for ((exit, page), read) in exits.iter().zip(&pages).zip(read_pages(&pages)) {
        assert!(
            page.contains("phx-hook=\"DisplayScene\"") || page.contains("data-switch"),
            "the {exit:?} exit serves the composed screen: {page}"
        );
        assert!(
            !page.contains("blob:") && !page.contains("createObjectURL"),
            "the {exit:?} exit loads nothing from an object URL"
        );
        assert!(
            read.inline_code.is_empty() && read.closed == 0,
            "the {exit:?} exit carries inline code no hash admits: {:?}, self-closed {}",
            read.inline_code,
            read.closed
        );
        for (text, hash) in read.scripts.iter().zip(read.hashes) {
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
    let monitor = &pages[0];
    let on_monitor = read_pages(std::slice::from_ref(monitor)).remove(0).hashes;
    assert!(
        monitor.contains("phx-hook=\"ColonyView\""),
        "the monitor page carries the code view: {monitor}"
    );
    assert_eq!(
        on_monitor.len(),
        3,
        "the monitor page carries exactly the scene hook, the microphone hook and the \
         colony-view client inline"
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

    colony.shutdown().await;
}

/// The voice test page the colony's listener serves: no inline script, no inline style, no
/// object URL, and its three files beside it. A fn of its own, so the voice lock is proven
/// by this one read and by no display read beside it (re-check tv-bn M9g, B-M1 (a)).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_voice_test_page_carries_no_inline_script() {
    if !library_ships() {
        println!("SKIP no template library in this tree");
        return;
    }
    let colony = boot_with_voice_echo(Boot::default()).await;
    // The fixture GETs them (re-check tv-bn M7b12 A-M3): the site gets the files, not the
    // address of the listener that serves the colony's screens as well.
    let voice = colony.voice_files(Site::Gh867VoiceFiles).await;
    let (_, status, page) = &voice[0];
    assert_eq!(*status, 200, "the voice test page answers");
    let read = assert_no_inline_code("the voice test page", page);
    assert!(
        !page.to_ascii_lowercase().contains("<style") && read.styles == 0,
        "and no inline style block: {page}"
    );
    assert_eq!(
        voice[1..].iter().map(|(f, _, _)| *f).collect::<Vec<_>>(),
        ["test.js", "test.css", "worklet.js"],
        "the three files beside the page"
    );
    for (file, status, body) in &voice[1..] {
        assert_eq!(*status, 200, "{file} is served beside the page");
        assert!(
            !body.contains("createObjectURL") && !body.contains("new Blob"),
            "{file} builds no object URL"
        );
    }

    colony.shutdown().await;
}

/// The inline scripts the tree ships, found rather than named (re-check tv-bn M8, M1).
///
/// The lock before this one named `SCENE_CLIENT_JS` and `OS_CLIENT_JS` and compared their
/// hashes with `templates/display/csp.json`. It caught a changed hook, and it could never
/// catch a NEW one: `colony-view`, the code view the display shows by default
/// (`params.code_views`), draws its own client into the display page through the same
/// `<script>{{&client_js}}</script>` carrier, and its hash stood in no policy -- under the
/// published `script-src` its hook would have been blocked, the symptom of acceptance N
/// (`unknown hook`). `canvy` carries the same form on its own page.
///
/// So the python below reads what the templates SHIP, the way a page gets it: every string
/// and bytes constant of each cell's code (`params.script_inline` in `templates/**/config.json`,
/// the bytes a `code` cell runs, parsed with python's own `ast`) and every string of every
/// JSON and JSONL file under `templates/`. Each `<script>` without `src` in them is a
/// carrier. A carrier whose text is one raw prop (`{{&client_js}}`) is filled by the module
/// constant the code writes into that prop; a carrier with literal text is its own source;
/// anything else -- a prop filled by an expression, a template inside the script -- is a
/// refusal, because no published hash could cover it. So is what the discovery cannot read
/// (re-check tv-bn M9, M1; M9c, M1-Rest): code that does not parse, a `script_inline` that is
/// not a string or stands anywhere but in `params` of a `config.json`, and a carrier the code
/// assembles from parts in the forms it recognises. Those are refused at the part: every code
/// constant and every JSON string that is a bare script tag, has an opening or closing script
/// tag (or a cut one, `<scr`/`ipt>`) at its start or end -- read through the markup there, so
/// `"<div><script>a=1;"` counts -- or has an opening script tag right after markup or a closing
/// one right before markup anywhere in it (`"<title>Canvy</title><script>window.CFG="`) is a
/// refusal with its file, line or key and name. Exempt are only the two sanitizer needles of
/// the display's compose code, `FORBIDDEN_MARKUP` and `OWN_CODE_MARKUP` (`"<script"`), pinned
/// by name; a pin whose constant is gone is a refusal as well. Where the assembly is spelled
/// out (f-string, `+`, `%`, `.format`, `.join`, `.replace`), it is also read whole, literal
/// parts in place and every part it cannot read an unknown: a `<script` without `src` in it, a
/// `</script` with no readable opening before it, or an unknown next to a tag bracket is a
/// refusal.
///
/// This lock is the early warning, not the guarantee: it reads source, and source can spell
/// a script in forms no reader of it knows. What stays out of its reach: a tag spelled from a
/// lone `<` and a tag NAME the code holds as data -- through `+=`, a list, or a placeholder of
/// `%` or `.format` (whose keyword arguments it does not read at all) -- a script tag in the
/// middle of a part with neither markup nor the part's edge beside it (`"x<script>a="`), and
/// any use of a pinned needle beyond its constant. The guarantee is the render lock below
/// (`every_page_a_template_composes_carries_only_listed_inline_scripts`), which reads the
/// pages themselves, whatever the code that built them looks like.
/// Each source belongs to one policy: a view the display draws (its `params.code_views`,
/// else the default its compose code names in `CODE_VIEWS`, read from that code) is drawn
/// into the display page and so into `templates/display/csp.json`; any other template
/// serves its own page and publishes its own `templates/<name>/csp.json`.
const DISCOVER: &str = r#"
import ast, json, os, re, sys, types, pathlib
root = pathlib.Path(json.load(sys.stdin))
carrier = re.compile(r'<script\b([^>]*)>(.*?)</script\s*>', re.I | re.S)
raw_prop = re.compile(r'^\{\{&(\w+)\}\}$')
carriers, sources, refusals = [], [], []
# The only code constants that are a script tag and no tag of any page: the sanitizer
# needles of the display's compose code (the `<script` it refuses in a view's markup,
# and the one a code view may bring). Pinned by file and name; a pin whose constant
# is gone is a refusal too, so the exemption cannot outlive its reason.
PINNED = {('templates/display/compose/config.json', 'FORBIDDEN_MARKUP'),
          ('templates/display/compose/config.json', 'OWN_CODE_MARKUP')}
pinned_seen = set()

def as_text(value):
    # `bytes` reach a page as well as `str` (re-check tv-bn M9c, M1-Rest G).
    return value.decode('utf-8', 'replace') if isinstance(value, bytes) else value

# The rule at the part, for every code constant and every JSON string alike (re-check
# tv-bn M9c, M1-Rest B-K; M9d, M1 N1). A carrier written whole is the scan's; what is
# left must not be a script tag that only an assembly around it would close. "At the
# edge" reads through markup: `"<div><script>a=1;"` opens a script at its start as much
# as `"<script>"` does, `"b=2;</script></div>"` closes one at its end.
tags = r'(?:<[^<>]*>\s*)*'
edges = (
    ('is a bare script tag', re.compile(r'^\s*</?script\b[^>]*>?\s*$', re.I)),
    ('begins with an opening script tag',
     re.compile(r'^\s*' + tags + r'<script\b(?![^>]*\bsrc\s*=)[^>]*>', re.I)),
    ('ends in an opening script tag',
     re.compile(r'<script\b(?![^>]*\bsrc\s*=)[^>]*>?\s*' + tags + '$', re.I)),
    ('begins with a closing script tag', re.compile(r'^\s*' + tags + r'</script\b', re.I)),
    ('ends in a closing script tag', re.compile(r'</script\b[^>]*>?\s*' + tags + '$', re.I)),
    ('ends in a cut script tag', re.compile(r'(?:</?s(?:c(?:r(?:i(?:p)?)?)?)?|</)$', re.I)),
    ('begins with a cut script tag',
     re.compile(r'^(?:/script\b|script\b[^>\n]*>|(?:cript|ript|ipt)\b|pt\s*>)', re.I)),
    # And around the tag, anywhere in the part (re-check tv-bn M9d, M1): a page frame
    # puts text between the edge and the tag (`"<title>Canvy</title><script>window.CFG="`,
    # `";</script><p>Loading</p>"`), so an opening tag right after markup and a closing
    # one right before markup are a script cut open at the part as well.
    ('has an opening script tag right after markup',
     re.compile(r'>\s*<script\b(?![^>]*\bsrc\s*=)', re.I)),
    ('has a closing script tag right before markup', re.compile(r'</script\b[^<>]*>?\s*<', re.I)),
)

def cut_open(text):
    # Why `text` is a part of an assembled script, or None.
    # A carrier written whole stays markup where it stood (`<\0>`, not a bare `\0`):
    # a script cut open right beside it is a script after or before markup, as on the
    # page (re-check tv-bn M9e, A-H1: `'<script src="/c.js"></script><script>window.CFG='`
    # went through while the cut-out carrier read as no markup at all).
    rest = carrier.sub('<\0>', text)
    for why, rx in edges:
        if rx.search(rest):
            return why, (rest[:60] if len(rest) <= 60 else rest[:30] + '...' + rest[-30:])
    return None

def scan(at, template, text, out):
    for m in carrier.finditer(text):
        if re.search(r'\bsrc\s*=', m.group(1), re.I):
            continue
        out.append({'at': at, 'template': template, 'body': m.group(2)})

def walk_json(at, template, value, out, keys=()):
    if isinstance(value, str):
        scan(at, template, value, out)
        cut = cut_open(value)
        if cut:
            refusals.append('%s: the string at `%s` %s (%r), so a page may carry an inline '
                            'script assembled around it that no published hash stands for'
                            % (at, '.'.join(keys) or '.', cut[0], cut[1]))
    elif isinstance(value, list):
        for v in value:
            walk_json(at, template, v, out, keys + ('[]',))
    elif isinstance(value, dict):
        for k, v in value.items():
            if k != 'script_inline':
                walk_json(at, template, v, out, keys + (k,))
            elif not (at.endswith('/config.json') and keys == ('params',)):
                # Code is read below only where a `code` cell runs it from. Anywhere
                # else nothing here would read it: a refusal, not a skip (M1-Rest 5).
                refusals.append('%s: a `script_inline` at `%s`, not in `params` of a '
                                '`config.json`, so the code it carries is not read'
                                % (at, '.'.join(keys + (k,))))

# Every page text a template ships: the string constants of each cell's code
# (`params.script_inline`, the bytes the `code` cell runs) and every string of its
# JSON and JSONL files (seeds, catalogues, params).
for path in sorted(root.glob('templates/**/*')):
    if path.suffix not in ('.json', '.jsonl') or not path.is_file():
        continue
    rel = path.relative_to(root).as_posix()
    template = rel.split('/')[1]
    text = path.read_text(encoding='utf-8')
    docs = ([json.loads(l) for l in text.splitlines() if l.strip()]
            if path.suffix == '.jsonl' else [json.loads(text)])
    for doc in docs:
        walk_json(rel, template, doc, carriers)
    if path.name != 'config.json':
        continue
    params = docs[0].get('params') if isinstance(docs[0], dict) else None
    if not isinstance(params, dict) or 'script_inline' not in params:
        continue
    code = params['script_inline']
    if not isinstance(code, str):
        refusals.append('%s: `params.script_inline` is not a string, so the code it '
                        'ships cannot be read' % rel)
        continue
    try:
        tree = ast.parse(code)
    except SyntaxError as e:
        # Code the lock cannot read may carry anything: a refusal, never a skip.
        refusals.append('%s: `script_inline` does not parse (%s), so the inline scripts '
                        'it ships cannot be read' % (rel, e))
        continue
    found = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)):
            scan('%s:%d' % (rel, node.lineno), template, as_text(node.value), found)
    carriers.extend(found)
    # A carrier is written as ONE constant. Assembled from parts, by whatever
    # expression, the tag itself still has to stand in some constant: bare, or at
    # the start or end of a part. Each such constant is a refusal, read at the part
    # and so independent of the form that joins it (`+=`, `append` + `join`, a
    # generator, an attribute, a dict entry, a local name; re-check tv-bn M9c,
    # M1-Rest B-K). The long constants (a client, a stylesheet, a message) mention
    # `<script>` in prose and comments, never at their edges.
    owner = {}
    for node in ast.walk(tree):
        targets = (node.targets if isinstance(node, ast.Assign) else
                   [node.target] if isinstance(node, (ast.AugAssign, ast.AnnAssign)) else [])
        if targets and node.value is not None:
            for c in ast.walk(node.value):
                owner.setdefault(id(c), ', '.join(ast.unparse(t) for t in targets))
    exempt = set()
    for n in tree.body:
        if (isinstance(n, ast.Assign) and len(n.targets) == 1
                and isinstance(n.targets[0], ast.Name) and (rel, n.targets[0].id) in PINNED):
            parts = (n.value.elts if isinstance(n.value, (ast.Tuple, ast.List, ast.Set))
                     else [n.value])
            for c in parts:
                if (isinstance(c, ast.Constant) and isinstance(c.value, str)
                        and c.value.lower() == '<script'):
                    exempt.add(id(c))
                    pinned_seen.add((rel, n.targets[0].id))
    for node in ast.walk(tree):
        if not (isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes))):
            continue
        if id(node) in exempt:
            continue
        cut = cut_open(as_text(node.value))
        if cut:
            name = owner.get(id(node))
            refusals.append('%s:%d: the constant%s %s (%r), so a page may carry an '
                            'inline script assembled around it that no published hash '
                            'stands for' % (rel, node.lineno,
                                            ' in `%s`' % name if name else '', cut[0], cut[1]))
    # Where the assembly is spelled out, it is also read whole: put back together
    # from its literal parts, every part it cannot read (a name, a call, a value)
    # an unknown `\x01` -- never an empty hole -- and list items and call
    # arguments apart by `\0`.
    def assembled(node):
        if isinstance(node, ast.JoinedStr):
            return True
        if isinstance(node, ast.BinOp):
            return isinstance(node.op, (ast.Add, ast.Mod))
        return (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and node.func.attr in ('format', 'format_map', 'join', 'replace'))
    def flat(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)):
            # A whole carrier inside one part is the scan's above; what is left
            # is a tag that only the assembly closes.
            return carrier.sub('\0', as_text(node.value))
        if isinstance(node, ast.JoinedStr):
            return ''.join(flat(v) for v in node.values)
        if isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Add, ast.Mod)):
            return flat(node.left) + flat(node.right)
        if isinstance(node, (ast.List, ast.Tuple)):
            return '\0'.join(flat(e) for e in node.elts)
        if isinstance(node, ast.Call) and assembled(node):
            return flat(node.func.value) + '\0' + '\0'.join(flat(a) for a in node.args)
        return '\x01'
    whole = (
        ('an opening script tag', re.compile(r'<script\b(?![^>]*\bsrc\s*=)[^>]*', re.I)),
        ('a closing script tag with no opening before it',
         re.compile(r'^[^<]*(?:<(?!script\b)[^<]*)*</script\b', re.I)),
        ('a part it cannot read next to a tag bracket',
         re.compile(r'<(?:/?(?:s(?:c(?:r(?:i(?:pt?)?)?)?)?)?)\x01|\x01\s*/?>'
                    r'|\x01(?:/?script|cript|ript|ipt|pt)', re.I)),
    )
    told = set()
    for node in ast.walk(tree):
        if not assembled(node):
            continue
        f = flat(node)
        for why, rx in whole:
            m = rx.search(f)
            if not m or (node.lineno, why) in told:
                continue
            told.add((node.lineno, why))
            refusals.append('%s:%d: an inline script assembled from parts (%s, %s), not '
                            'written as one constant, so the hash a page needs for it is not '
                            'in the code: %r' % (rel, node.lineno, type(node).__name__, why,
                                                 m.group(0)[-80:]))
    if not found:
        continue
    # What fills each raw carrier prop, read where the code writes it: a dict
    # entry or an item assignment naming the prop. A `prop_schema` says only the
    # type (`"html"`), so its dict is not a value.
    props = {}
    for c in found:
        m = raw_prop.match(c['body'])
        if m:
            props[m.group(1)] = c['at']
        elif '{{' in c['body']:
            refusals.append('%s: an inline script whose text is a template, not a '
                            'constant or one raw prop: %r' % (c['at'], c['body'][:80]))
        else:
            sources.append({'template': template, 'at': c['at'], 'line': 0,
                            'name': '<literal>', 'text': c['body']})
    schemas = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Dict):
            for k, v in zip(node.keys, node.values):
                if isinstance(k, ast.Constant) and k.value == 'prop_schema' and isinstance(v, ast.Dict):
                    schemas.add(id(v))
    module = None
    for node in ast.walk(tree):
        pairs = []
        if isinstance(node, ast.Dict) and id(node) not in schemas:
            pairs = [(k.value, v, node.lineno) for k, v in zip(node.keys, node.values)
                     if isinstance(k, ast.Constant) and k.value in props]
        elif isinstance(node, ast.Assign):
            pairs = [(t.slice.value, node.value, node.lineno) for t in node.targets
                     if isinstance(t, ast.Subscript) and isinstance(t.slice, ast.Constant)
                     and t.slice.value in props]
        for prop, value, line in pairs:
            if not isinstance(value, ast.Name):
                refusals.append('%s:%d: `%s` is filled with an expression, not a module '
                                'constant, so its hash cannot be published' % (rel, line, prop))
                continue
            if module is None:
                module = types.ModuleType('shipped')
                module.__file__ = str(path.parent / 'shipped.py')
                here = os.getcwd()
                os.chdir(path.parent)
                try:
                    exec(compile(code, module.__file__, 'exec'), module.__dict__)
                finally:
                    os.chdir(here)
            text = module.__dict__.get(value.id)
            if not isinstance(text, str):
                refusals.append('%s:%d: `%s` = %s is not a module-level string'
                                % (rel, line, prop, value.id))
                continue
            sources.append({'template': template, 'at': '%s:%d' % (rel, line),
                            'line': line, 'name': value.id, 'text': text})
    for prop, at in props.items():
        if not any(s['at'].startswith(rel + ':') and s['name'] != '<literal>' for s in sources):
            refusals.append('%s: the raw prop `%s` carries an inline script and nothing '
                            'in the code fills it' % (at, prop))

for rel, name in sorted(PINNED - pinned_seen):
    refusals.append('%s: the pinned sanitizer needle `%s` (`"<script"`) is gone, so its '
                    'exemption in this lock names nothing; drop the pin' % (rel, name))

# Which published policy each source belongs to: a code view (`code_views` of the
# display, shipped default) is drawn INTO the display page, so the display's
# `csp.json` lists it; any other template serves its own page and publishes its own.
# The views are what the display runs with: `params.code_views` of its compose
# cell when it ships one, else the default its code names (`CODE_VIEWS`, every
# literal assigned to it, one value) -- read from the code, not copied here.
compose = root / 'templates/display/compose/config.json'
knobs = json.loads(compose.read_text(encoding='utf-8')).get('params') or {}
defaults = set()
try:
    for n in ast.walk(ast.parse(knobs.get('script_inline') or '')):
        if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'CODE_VIEWS'
                                             for t in n.targets):
            for v in ast.walk(n.value):
                if isinstance(v, (ast.Tuple, ast.List)) and all(
                        isinstance(e, ast.Constant) and isinstance(e.value, str) for e in v.elts):
                    defaults.add(tuple(e.value for e in v.elts))
except SyntaxError:
    pass
views = knobs.get('code_views')
if len(defaults) != 1:
    refusals.append('templates/display/compose/config.json: the compose code names no one '
                    'default `CODE_VIEWS` as a literal (found %r), so which views draw '
                    'into the display page is unknown' % sorted(defaults))
    views = views if isinstance(views, list) else []
views = [str(v) for v in views] if isinstance(views, list) else list(next(iter(defaults)))
for s in sources:
    owner = 'display' if s['template'] in views else s['template']
    s['policy'] = 'templates/%s/csp.json' % owner
    s['rank'] = 0 if s['template'] == owner else 1 + views.index(s['template'])
sources.sort(key=lambda s: (s['policy'], s['rank'], s['at'].split(':')[0], s['line']))
print(json.dumps({'carriers': carriers, 'sources': sources, 'refusals': refusals,
                  'code_views': views,
                  'policies': sorted(p.relative_to(root).as_posix()
                                     for p in root.glob('templates/*/csp.json'))}))
"#;

/// What the discovery finds today, as a count beside the real check: the display's two
/// carriers (`compose.py` `SCENE_TEMPLATE`, `OS_TEMPLATE`), colony-view's shell and
/// canvy's shell; their four sources are `SCENE_CLIENT_JS`, `OS_CLIENT_JS` and the two
/// `CLIENT_JS`. The protection is the policy equality below; the count only makes a new
/// carrier say itself by name in the failure.
const CARRIERS: usize = 4;
const SOURCES: usize = 4;

/// One published policy fragment, by its path under the repository.
fn policy(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    Some(meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}")))
}

fn text_of(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("no `{key}` in {v}"))
        .to_string()
}

/// Every inline script the tree ships has its hash in the `csp.json` of the page that
/// carries it, every published hash belongs to one of them, and no fragment admits what a
/// strict policy forbids.
#[test]
fn the_published_hashes_are_the_shipped_constants() {
    if !library_ships() {
        println!("SKIP no template library in this tree");
        return;
    }
    // python3 reads every page here; a host without it is red, not a skip (re-check tv-bn
    // M9f, A-M1).
    assert!(
        have_python(),
        "python3 is required: a reader that cannot read is red"
    );
    let root = repo("");
    let found = run_python(DISCOVER, &json!(root.to_str().expect("a utf-8 path")));
    let refusals = found["refusals"].as_array().expect("refusals");
    assert!(
        refusals.is_empty(),
        "an inline script whose hash cannot be published: {refusals:#?}"
    );
    let carriers: Vec<String> = found["carriers"]
        .as_array()
        .expect("carriers")
        .iter()
        .map(|c| text_of(c, "at"))
        .collect();
    let sources = found["sources"].as_array().expect("sources");

    // Per policy: `'self'` and exactly the hashes of its sources, in discovery order (the
    // template's own sources in code order, then the code views in `code_views` order).
    let mut by_policy: std::collections::BTreeMap<String, Vec<&Value>> = Default::default();
    for s in sources {
        by_policy.entry(text_of(s, "policy")).or_default().push(s);
    }
    for (rel, of) in &by_policy {
        let names: Vec<String> = of
            .iter()
            .map(|s| format!("{} at {}", text_of(s, "name"), text_of(s, "at")))
            .collect();
        let csp = policy(rel).unwrap_or_else(|| {
            panic!(
                "{rel} does not exist, and these inline scripts are on its page: {names:#?} \
                 -- a proxy with `script-src 'self'` blocks every one of them"
            )
        });
        let texts: Vec<String> = of.iter().map(|s| text_of(s, "text")).collect();
        let mut want = vec!["'self'".to_string()];
        want.extend(hashes(&texts));
        assert_eq!(
            listed(&csp, "script-src"),
            want,
            "{rel} `script-src` is 'self' plus the hashes of {names:#?}, in that order -- \
             recompute them after every change of one of these scripts"
        );
    }
    // A fragment nobody's script is in would admit whatever happens to hash to it.
    for rel in found["policies"].as_array().expect("policies") {
        let rel = rel.as_str().expect("a path");
        assert!(
            by_policy.contains_key(rel),
            "{rel} is published, and no inline script the tree ships belongs to it"
        );
    }
    for rel in by_policy.keys() {
        let csp = policy(rel).expect("read above");
        for (directive, sources) in csp.as_object().expect("an object") {
            for s in sources.as_array().expect("a list") {
                let s = s.as_str().expect("a string");
                assert!(
                    s != "'unsafe-eval'"
                        && s != "blob:"
                        && !(directive == "script-src" && s == "'unsafe-inline'"),
                    "{rel} `{directive}` admits {s}"
                );
            }
        }
    }
    assert_eq!(
        (carriers.len(), sources.len()),
        (CARRIERS, SOURCES),
        "the tree ships another set of inline scripts than the one this lock was written \
         for: carriers {carriers:#?}; give each new one its hash and move the count"
    );
    // The display fragment beyond `script-src`, as README § strict CSP explains it.
    let csp = csp_json();
    assert_eq!(listed(&csp, "style-src"), vec!["'self'", "'unsafe-inline'"]);
    assert_eq!(listed(&csp, "img-src"), vec!["'self'", "data:"]);
    assert_eq!(listed(&csp, "connect-src"), vec!["'self'"]);
    assert_eq!(listed(&csp, "base-uri"), vec!["'self'"]);
    // And the microphone hook loads its worklet by URL, from the web cell's client.
    let os = sources
        .iter()
        .find(|s| s["name"] == "OS_CLIENT_JS")
        .map(|s| text_of(s, "text"))
        .expect("the microphone hook is one of the sources");
    assert!(
        os.contains("@client/display-mic-worklet.js") && os.contains("document.baseURI"),
        "the microphone hook loads its worklet from the web cell's client"
    );
    assert!(
        !os.contains("createObjectURL") && !os.contains("new Blob"),
        "and builds no object URL for it"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The render lock: the pages themselves, as the templates compose them
// ─────────────────────────────────────────────────────────────────────────────

/// Every template that composes a page, with the mount it serves it on: each `config.json`
/// under `templates/` whose cell is a `web` cell or a `ref` to one (`web@…`), owned by the
/// template it stands in. Read from the tree, so a new one is a page this lock renders or
/// a red line saying it cannot.
fn page_surfaces() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                out.push(p);
            }
        }
    }
    let base = repo("templates");
    let mut configs = Vec::new();
    walk(&base, &mut configs);
    let configs: Vec<(std::path::PathBuf, Value)> = configs
        .into_iter()
        .map(|p| {
            let v = display_colony::read_json(&p);
            (
                p.strip_prefix(&base)
                    .expect("under templates")
                    .to_path_buf(),
                v,
            )
        })
        .collect();
    let owner = |rel: &std::path::Path| {
        rel.components()
            .next()
            .expect("a template")
            .as_os_str()
            .to_string_lossy()
            .to_string()
    };
    // The template a `ref` cell resolves to (`display@1.2.3` -> `display`).
    let target = |cell: &Value| -> Option<String> {
        if cell["type"] != "ref" {
            return None;
        }
        let t = cell["template"].as_str()?;
        Some(t.split('@').next().unwrap_or(t).to_string())
    };
    // Every template that composes a page: a `web` cell of its own, or a `ref` to `web@…` or
    // to a template that composes one -- resolved to the fixpoint, so a ref two steps from
    // a `web` cell is one too. A ref onto `display@…` or `canvy@…` composes their pages
    // under its own `override_params`; skipped, those were pages nobody renders (re-check
    // tv-bn M9f, B-M2).
    let mut holders = std::collections::BTreeSet::new();
    loop {
        let before = holders.len();
        for (rel, v) in &configs {
            let cell = &v["cell"];
            if cell["type"] == "web"
                || target(cell).is_some_and(|t| t == "web" || holders.contains(&t))
            {
                holders.insert(owner(rel));
            }
        }
        if holders.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    for (rel, v) in &configs {
        let cell = &v["cell"];
        let rel = rel.as_path();
        let mount = if cell["type"] == "web" {
            v["params"]["mount"].as_str()
        } else if cell["type"] == "ref"
            && cell["template"]
                .as_str()
                .is_some_and(|t| t.starts_with("web@"))
        {
            v["override_params"][""]["mount"].as_str()
        } else if let Some(t) = target(cell).filter(|t| holders.contains(t)) {
            panic!(
                "templates/{} is a `ref` to `{}`, a template that composes a page: it composes \
                 those pages again under its own params, and this lock renders none of them -- \
                 give it a renderer below and its line in SURFACES",
                rel.display(),
                cell["template"].as_str().unwrap_or(&t)
            );
        } else {
            // Another spelling of a `web` cell is refused by name, never dropped: a
            // surface this walk does not count is a page nobody renders (re-check tv-bn
            // M9e, A-H4).
            let mut strings = Vec::new();
            strings_in(cell, &mut strings);
            assert!(
                !strings.iter().any(|s| {
                    let s = s.trim().to_ascii_lowercase();
                    s == "web" || s.starts_with("web@")
                }),
                "templates/{} names a `web` cell in a form this lock does not read \
                 (`type: \"web\"` or `type: \"ref\"` with `template: \"web@…\"`): {cell}",
                rel.display()
            );
            continue;
        };
        let template = owner(rel);
        let mount = mount.unwrap_or_else(|| {
            panic!(
                "templates/{} ships a web cell without a mount",
                rel.display()
            )
        });
        out.push((template, mount.to_string()));
    }
    let names: Vec<&str> = out.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        names, SURFACES,
        "the templates that compose a page are another set than the one this lock renders \
         ({SURFACES:?}): give a new one its renderer below and move the list"
    );
    out
}

/// Every string anywhere in `v`.
fn strings_in(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|e| strings_in(e, out)),
        Value::Object(m) => m.values().for_each(|e| strings_in(e, out)),
        _ => {}
    }
}

/// What `page_surfaces` finds today, by name and in walk order: the floor beside the
/// renderer match below, so a surface that drops out of the walk -- not only a new one --
/// says itself by name (re-check tv-bn M9e, A-H4).
const SURFACES: &[&str] = &["canvy", "display", "web"];

/// Run one shipped `code` cell the way the substrate does: its `params.runner` on its
/// `params.script_inline`, the stdin document from the cell's own wire
/// (`build_stdin_json`, so the script sees its params as it does in a colony). The
/// emissions, one or several.
fn run_code_cell(
    config_rel: &str,
    target: &str,
    body: Value,
    hop: Value,
    ctx: Value,
) -> Vec<Value> {
    use std::io::Write;
    let cfg = display_colony::read_json(&repo(config_rel));
    let params = &cfg["params"];
    let as_map = |v: Value| match v {
        Value::Object(m) => m,
        _ => Default::default(),
    };
    let msg = meclaw_core::MessageBuilder::new(Path::new(target))
        .hop(as_map(hop))
        .context(as_map(ctx))
        .body(meclaw_core::Body::Inline(body))
        .build();
    let stdin = meclaw_cells::code::wire::build_stdin_json(&msg, params).expect("a stdin document");
    // A script this large is past the argument limit, and the cell writes it to a file
    // and runs that (`code/script_file.rs`); so does this.
    let script = tempfile::TempDir::new().expect("tempdir");
    let file = script.path().join("cell.py");
    std::fs::write(
        &file,
        params["script_inline"].as_str().expect("an inline script"),
    )
    .expect("write the script");
    let mut child = std::process::Command::new(params["runner"].as_str().expect("a runner"))
        .arg(&file)
        .current_dir(repo(config_rel).parent().expect("a cell dir"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the runner starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("the runner ends");
    assert!(
        out.status.success(),
        "{config_rel} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("the cell printed JSON") {
        Value::Array(items) => items,
        one => vec![one],
    }
}

/// A `web` cell over `cell_dir` with `params`, on a listener of its own.
struct WebPage {
    port: u16,
    mount: String,
    sender: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    join: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl WebPage {
    async fn start(cell_dir: &std::path::Path, path: &str, params: Value) -> WebPage {
        let mount = params["mount"].as_str().expect("a mount").to_string();
        let surfaces = Arc::new(SurfaceRegistry::new());
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
            .spawn_cell(
                Path::new(path),
                params,
                out_tx,
                cell_dir.to_path_buf(),
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
            sender,
            stop_tx,
            ..
        } = spawned
        else {
            panic!("a web cell spawns Active");
        };
        wait_for_mount(&surfaces, &mount).await;
        let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
        WebPage {
            port: addr.port(),
            mount,
            sender,
            out_rx,
            join,
            listener,
            _stop: stop_tx,
        }
    }

    async fn get_route(&self, route: &str) -> String {
        let (status, _, page) = get_text(&format!(
            "http://127.0.0.1:{}/{}{route}",
            self.port, self.mount
        ))
        .await;
        assert_eq!(status, 200, "/{}{route} answers: {page}", self.mount);
        page
    }

    fn stop(self) {
        self.join.abort();
        self.listener.abort();
    }
}

/// The display: the shipped screen with one application window, every code view it
/// draws (`code_views`) as its own layout emits it over a small graph, and every exit
/// of the screens it is booted with plus the switch.
async fn render_display(views: &[String]) -> Vec<(String, String)> {
    let colony = boot_with_voice_echo(past_the_fade()).await;
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "system", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    for view in views {
        let config = format!("templates/{view}/layout/config.json");
        assert!(
            repo(&config).is_file(),
            "the display draws the code view `{view}`, and {config} is not there: this lock \
             has no way to render what it draws"
        );
        let owner = format!("/alex/apps/{view}/layout");
        let graph = json!({"graph": {
            "scope": "/os",
            "nodes": [{"path": "/os/one", "cell_type": "code"},
                      {"path": "/os/two", "cell_type": "store"}],
            "edges": [{"from": "/os/one", "to": "/os/two"}]
        }});
        let emissions = run_code_cell(&config, &owner, graph, json!({}), json!({}));
        let emission = emissions
            .iter()
            .find(|e| e["view_id"].is_string())
            .unwrap_or_else(|| {
                panic!("the code view `{view}` drew no view over a graph: {emissions:#?}")
            });
        colony.put_emission(&owner, emission).await;
    }
    let mut exits: Vec<String> = display_colony::screens()
        .as_object()
        .expect("screens")
        .keys()
        .cloned()
        .collect();
    exits.push(String::new());
    let mut pages = Vec::new();
    for exit in exits {
        pages.push((format!("/display/{exit}"), colony.page(&exit).await));
    }
    colony.shutdown().await;
    pages
}

/// canvy: its shipped layout cell over a four-cell graph -- pass one (the snapshot) and
/// pass two (the display's refusal of the query, the bootstrap) -- and the bundle that
/// pass writes, applied to a real `web` cell on canvy's mount.
async fn render_canvy(mount: &str) -> Vec<(String, String)> {
    let layout = "templates/canvy/layout/config.json";
    let graph = json!({
        "scope": "/",
        "nodes": [{"path": "/a/one", "cell_type": "code"}, {"path": "/a/two", "cell_type": "store"},
                  {"path": "/b/three", "cell_type": "llm"}, {"path": "/b/four", "cell_type": "timer"}],
        "edges": [{"id": "e1", "from": "/a/one", "to": "/a/two"},
                  {"id": "e2", "from": "/a/two", "to": "/b/three"},
                  {"id": "e3", "from": "/b/three", "to": "/b/four"}]
    });
    let ask = run_code_cell(
        layout,
        "/canvy/layout",
        json!({"messages": [], "graph": graph}),
        json!({"route": "snapshot"}),
        json!({}),
    );
    let snap = ask
        .first()
        .and_then(|e| e["header"]["canvy_graph"].as_str())
        .unwrap_or_else(|| panic!("pass one carries the graph on: {ask:#?}"))
        .to_string();
    let boot = run_code_cell(
        layout,
        "/canvy/layout",
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "q",
                             "text": "no page declares the route \"/\""}]}),
        json!({"operation": "query", "error_code": "invalid_input"}),
        json!({"canvy_origin": "layout", "canvy_graph": snap}),
    );
    let turns = boot
        .first()
        .and_then(|e| e["messages"].as_array())
        .unwrap_or_else(|| panic!("the bootstrap pass writes a bundle: {boot:#?}"))
        .clone();
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    let mut web = WebPage::start(&cell_dir, "/canvy/web", json!({"mount": mount})).await;
    let msg = meclaw_core::MessageBuilder::new(Path::new("/canvy/web"))
        .body(meclaw_core::Body::Inline(json!({"messages": turns})))
        .reply_to(Path::new("/canvy/layout"))
        .build();
    web.sender
        .send(msg)
        .await
        .expect("the web cell takes the bundle");
    // The kit's scaled hang mark, no deadline of its own (809 `own_deadlines`, S2).
    let reply = meclaw_testing::deadline::hang(
        "the web cell answers the bundle",
        meclaw_testing::EmissionsExt::recv_answer(&mut web.out_rx),
    )
    .await
    .expect("an emission")
    .content;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(0),
        "every leg of canvy's bootstrap lands: {reply}"
    );
    let page = web.get_route("/").await;
    web.stop();
    vec![(format!("/{mount}/"), page)]
}

/// The `web` template itself: a web cell with its own params and its own seed, every
/// route its seed declares.
async fn render_web_template(template: &str) -> Vec<(String, String)> {
    let dir = repo(&format!("templates/{template}"));
    let cfg = display_colony::read_json(&dir.join("config.json"));
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell_dir = td.path().join(template);
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    if dir.join("seed").is_dir() {
        display_colony::copy_tree(&dir.join("seed"), &cell_dir.join("seed"));
    }
    // Every line of the seed is read or red: a line dropped here is a route never rendered
    // (re-check tv-bn M9e, A-H4).
    let pages_jsonl = format!("templates/{template}/seed/pages.jsonl");
    let raw = std::fs::read_to_string(repo(&pages_jsonl))
        .unwrap_or_else(|e| panic!("{pages_jsonl}: {e} -- the template composes a page"));
    // As `web/seed.rs` reads it: line 1 is the schema header and nothing else is; every
    // later line is a row, and a row's extra keys (a `schema` too) are columns the seed
    // skips while the row ships (re-check tv-bn M9f, B-M2).
    let mut lines = raw.lines().enumerate();
    let header = lines.next().map_or("", |(_, l)| l);
    let head: Value = meclaw_core::serde_json::from_str(header).unwrap_or_else(|e| {
        panic!("{pages_jsonl}:1: {e}: line 1 is the schema header web/seed.rs reads")
    });
    assert!(
        head.get("schema").is_some_and(Value::is_object),
        "{pages_jsonl}:1 is no schema header {{\"schema\":{{…}}}}, and web/seed.rs reads line \
         1 as one: {header}"
    );
    let mut routes = Vec::new();
    for (n, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = meclaw_core::serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("{pages_jsonl}:{}: {e}: {line}", n + 1));
        let route = v["route"]
            .as_str()
            .unwrap_or_else(|| panic!("{pages_jsonl}:{} declares no route: {line}", n + 1));
        routes.push(route.to_string());
    }
    assert!(
        !routes.is_empty(),
        "templates/{template} composes a page and its seed declares no route"
    );
    let web = WebPage::start(&cell_dir, &format!("/{template}"), cfg["params"].clone()).await;
    let mut pages = Vec::new();
    for route in routes {
        pages.push((
            format!("/{}{route}", web.mount),
            web.get_route(&route).await,
        ));
    }
    web.stop();
    pages
}

/// The render lock -- the guarantee (re-check tv-bn M9d, M1).
///
/// The discovery above reads the code that ships, and code can spell a script in more
/// forms than any reader of its source will ever know: a page frame of names, a `+=`, a
/// tag kept as a JSON param. So this lock reads what ships at the other end, the pages:
/// every template that composes one (a `web` cell, found in the tree), rendered the way a
/// colony renders it -- the display's own compose code on a booted colony with a window and
/// every code view it draws, canvy's layout passes into a real `web` cell, the `web`
/// template on its own seed -- and every inline script on every one of those pages, read by
/// an HTML parser, has its hash in the template's `csp.json` (no `csp.json`: no inline
/// script at all), every published hash is on one of the pages, and no page carries an
/// inline handler. A template that composes a page this lock cannot render, a published
/// policy that belongs to no page, and a tree in which no template composes one at all are
/// red. Whatever form the source takes, a script that reaches the page is here.
///
/// What stays out of reach is what the fixture does not drive: a script only another input
/// would bring onto a page (another graph, another window, a later pass). There the
/// discovery is the early warning, and the browser lock beside this one runs the display
/// under the policy in a real engine. The pages Rust serves itself are not templates; they
/// are read with the same parser where they are served (the shell and the voice test page
/// above, the operator UI in `meclaw-api`), and `every_html_page_rust_serves_has_its_lock`
/// below holds that list against the source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_page_a_template_composes_carries_only_listed_inline_scripts() {
    if !library_ships() {
        println!("SKIP no template library in this tree");
        return;
    }
    // python3 reads every page here; a host without it is red, not a skip (re-check tv-bn
    // M9f, A-M1).
    assert!(
        have_python(),
        "python3 is required: a reader that cannot read is red"
    );
    let found = run_python(DISCOVER, &json!(repo("").to_str().expect("a utf-8 path")));
    let views: Vec<String> = found["code_views"]
        .as_array()
        .expect("code views")
        .iter()
        .map(|v| v.as_str().expect("a view").to_string())
        .collect();
    let surfaces = page_surfaces();
    assert!(
        !surfaces.is_empty(),
        "no template under templates/ composes a page, so this lock renders nothing"
    );
    for rel in found["policies"].as_array().expect("policies") {
        let rel = rel.as_str().expect("a path");
        assert!(
            surfaces
                .iter()
                .any(|(t, _)| rel == format!("templates/{t}/csp.json")),
            "{rel} is published, and no template page this lock renders belongs to it: \
             {surfaces:?}"
        );
    }
    let mut rendered = Vec::new();
    for (template, mount) in &surfaces {
        let pages = match template.as_str() {
            "display" => render_display(&views).await,
            "canvy" => render_canvy(mount).await,
            "web" => render_web_template(template).await,
            other => panic!(
                "templates/{other} composes a page on the mount `{mount}`, and this lock has \
                 no way to render it: give it a renderer here, or its inline scripts go unseen"
            ),
        };
        assert!(!pages.is_empty(), "templates/{template} rendered no page");
        let rel = format!("templates/{template}/csp.json");
        let script_src =
            policy(&rel).map_or_else(|| vec!["'self'".to_string()], |p| listed(&p, "script-src"));
        // The reader every page lock shares (`meclaw_testing::page_scripts::read_pages`):
        // python's HTML parser, hashing as a browser does, and a scan beside it.
        let htmls: Vec<String> = pages.iter().map(|(_, h)| h.clone()).collect();
        let mut on_pages = std::collections::BTreeSet::new();
        for ((label, _), page) in pages.iter().zip(read_pages(&htmls)) {
            assert_eq!(
                page.closed, 0,
                "{label} carries a self-closed `<script/>`, which a browser opens"
            );
            assert!(
                page.inline_code.is_empty(),
                "{label} carries inline code no hash in a `script-src` admits: {:?}",
                page.inline_code
            );
            for (text, hash) in page.scripts.iter().zip(page.hashes) {
                assert!(
                    script_src.contains(&hash),
                    "{label} carries an inline script whose hash {hash} is not in {rel} \
                     ({script_src:?}); it starts: {:?}",
                    text.chars().take(120).collect::<String>()
                );
                on_pages.insert(hash);
            }
        }
        for hash in script_src.iter().filter(|s| s.starts_with("'sha256-")) {
            assert!(
                on_pages.contains(hash),
                "{rel} lists {hash}, and no page of {template} the lock rendered carries it \
                 -- a hash nobody's script hashes to admits whatever does"
            );
        }
        rendered.push(format!(
            "{template}: {} pages, {} hashes",
            pages.len(),
            on_pages.len()
        ));
    }
    println!("RENDERED {rendered:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// The pages Rust serves itself: every one has its lock (re-check tv-bn M9e, B-M1)
// ─────────────────────────────────────────────────────────────────────────────

/// The lock that reads one Rust page with the shared parser: (test file, test fn).
type Lock = (&'static str, &'static str);

const SHELL_LOCK: Lock = (
    "crates/meclaw-cells/tests/gh867_no_surface_page_carries_unlisted_inline_script.rs",
    "the_web_shell_carries_no_inline_script",
);
const VOICE_LOCK: Lock = (
    "crates/meclaw-cells/tests/gh867_no_surface_page_carries_unlisted_inline_script.rs",
    "the_voice_test_page_carries_no_inline_script",
);
const VOICE_UNIT_LOCK: Lock = (
    "crates/meclaw-cells/src/voice/testpage.rs",
    "page_runs_under_script_src_self",
);
const UI_LOCK: Lock = (
    "crates/meclaw-api/tests/misc/gh867_ui_pages_carry_no_inline_script.rs",
    "every_html_answer_of_the_api_router_carries_no_inline_code",
);

/// Every source file under `crates/*/src` that holds an HTML marker (`text/html`, axum's
/// `Html(`/`Html<`, a doctype, an `include_str!` of an `.html` file), with the number of
/// markers it holds and the locks that read what it serves -- or, where it serves no page,
/// why not. Comment lines do not count; test modules do (they live in the file and move its
/// count like any line).
const RUST_HTML: &[(&str, usize, &[Lock], &str)] = &[
    ("meclaw-api/src/ui/dashboard.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/dead_letters.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/errors.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/graph.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/message.rs", 3, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/messages.rs", 1, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/mod.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/registry.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/templates.rs", 2, &[UI_LOCK], ""),
    ("meclaw-api/src/ui/trace.rs", 2, &[UI_LOCK], ""),
    (
        "meclaw-cells/src/bin/cdp_browser_fixture.rs",
        1,
        &[],
        "a fake CDP endpoint for tests: `text/html` is the value it answers to an evaluate",
    ),
    (
        "meclaw-cells/src/voice/service.rs",
        1,
        &[VOICE_LOCK, VOICE_UNIT_LOCK],
        "",
    ),
    (
        "meclaw-cells/src/voice/testpage.rs",
        1,
        &[VOICE_LOCK, VOICE_UNIT_LOCK],
        "",
    ),
    ("meclaw-cells/src/web/io.rs", 2, &[SHELL_LOCK], ""),
    (
        "meclaw-cells/src/web/markup.rs",
        1,
        &[],
        "a unit test of the URL check: `data:text/html,x` is a link it refuses",
    ),
    (
        "meclaw-testing/src/page_scripts.rs",
        1,
        &[],
        "the shared page reader itself: a doctype in the sample page of its unit test",
    ),
];

/// The HTML markers on one source line.
fn html_markers(line: &str) -> usize {
    let lower = line.to_ascii_lowercase();
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let word_at = |hay: &str, at: usize, len: usize| {
        !hay[..at].chars().next_back().is_some_and(ident)
            && !hay[at + len..].chars().next().is_some_and(ident)
    };
    let mut n = lower.matches("text/html").count() + lower.matches("<!doctype").count();
    n += line
        .match_indices("DOCTYPE")
        .filter(|(at, _)| word_at(line, *at, "DOCTYPE".len()))
        .count();
    n += line
        .match_indices("Html")
        .filter(|(at, _)| {
            !line[..*at].chars().next_back().is_some_and(ident)
                && line[at + 4..].trim_start().starts_with(['(', '<'])
        })
        .count();
    if line.contains("include_str!") && (lower.contains(".html\"") || lower.contains(".htm\"")) {
        n += line.matches("include_str!").count();
    }
    n
}

/// `text` as code: comments dropped (line, block and nested block), the contents of string,
/// raw string and char literals emptied, a lifetime kept. Braces and names counted on it are
/// code, never text (re-check tv-bn M9g, B-M1 (b), H2).
fn code_only(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let ident = |i: usize| c.get(i).is_some_and(|x| x.is_alphanumeric() || *x == '_');
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        match (c[i], c.get(i + 1).copied()) {
            ('/', Some('/')) => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                let mut depth = 0usize;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                out.push(' ');
            }
            ('"', _) => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
                out.push_str("\"\"");
            }
            // `r"…"`, `r#"…"#`, `br#"…"#`: no escapes, ends at the quote and its hashes.
            ('r', Some('"' | '#'))
                if !ident(i.wrapping_sub(1))
                    || (matches!(c.get(i.wrapping_sub(1)), Some('b' | 'c'))
                        && !ident(i.wrapping_sub(2))) =>
            {
                let mut j = i + 1;
                while c.get(j) == Some(&'#') {
                    j += 1;
                }
                if c.get(j) != Some(&'"') {
                    out.push('r');
                    i += 1;
                    continue;
                }
                let hashes = j - i - 1;
                i = j + 1;
                while i < c.len() {
                    if c[i] == '"' && (1..=hashes).all(|k| c.get(i + k) == Some(&'#')) {
                        i += 1 + hashes;
                        break;
                    }
                    i += 1;
                }
                out.push_str("\"\"");
            }
            ('\'', Some('\\')) => {
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
                out.push_str("' '");
            }
            ('\'', Some(_)) if c.get(i + 2) == Some(&'\'') => {
                i += 3;
                out.push_str("' '");
            }
            (x, _) => {
                out.push(x);
                i += 1;
            }
        }
    }
    out
}

/// The body of the one `fn {name}` in `text`, as code ([`code_only`]): what stands between
/// the brace that opens it and the brace that closes it, counted by depth on code, so a body
/// cut to `{}` is empty and never runs into the next item. `None` when there is no such fn,
/// more than one, one without a body, or one with a fn item of its own inside (a local
/// `fn read_pages` would answer for the reader). A lock is proven by what its fn calls, not
/// by what its file names: the `use` at the top of this file named `page_scripts` for every
/// lock in it (re-check tv-bn M9f, B-M1; M9g, B-M1 (b)).
fn fn_body(text: &str, name: &str) -> Option<String> {
    let code = code_only(text);
    let ident = |x: char| x.is_alphanumeric() || x == '_';
    let head = format!("fn {name}");
    let mut heads = code.match_indices(&head).map(|(at, _)| at).filter(|&at| {
        !code[..at].chars().next_back().is_some_and(ident)
            && matches!(
                code[at + head.len()..].trim_start().chars().next(),
                Some('(' | '<')
            )
    });
    let start = heads.next()?;
    if heads.next().is_some() {
        return None;
    }
    // The body opens at the first `{` after the head; a `;` before it is a fn without one.
    let after = start + head.len();
    let open = after + code[after..].find(['{', ';'])?;
    if code[open..].starts_with(';') {
        return None;
    }
    let mut depth = 0usize;
    let mut close = None;
    for (k, x) in code[open..].char_indices() {
        match x {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + k);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &code[open + 1..close?];
    let inner_fn = body.match_indices("fn").any(|(at, _)| {
        !body[..at].chars().next_back().is_some_and(ident)
            && body[at + 2..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
    });
    (!inner_fn).then(|| body.to_string())
}

/// The calls of `meclaw_testing::page_scripts` in a body [`fn_body`] returned.
fn page_reads(body: &str) -> usize {
    body.matches("assert_no_inline_code(").count() + body.matches("read_pages(").count()
}

/// The lock proof reads a body by its braces: a body cut to `{}` is empty and never the next
/// fn's, a brace or a call in a string, a char or a comment is none, and a fn with a fn of
/// its own inside is no lock body (re-check tv-bn M9g, B-M1 (b), H2).
#[test]
fn a_lock_body_is_its_own_braces_and_nothing_else() {
    let read = "assert_no_inline_code(\"x\", &page);";
    let cut = format!("async fn cut() {{}}\n\nasync fn next() {{\n    {read}\n}}\n");
    assert_eq!(
        fn_body(&cut, "cut").map(|b| page_reads(&b)),
        Some(0),
        "a body cut to `{{}}` reads nothing, not the next fn's page: {cut}"
    );
    let tricky = format!(
        "fn lock() {{\n    let s = \"}}\";\n    let r = r#\"}}\"#;\n    let c = '}}';\n    \
         let l: &'static str = s;\n    // }}\n    /* }} /* }} */ */\n    {read}\n}}\nfn other() {{\n    {read}\n}}\n"
    );
    assert_eq!(
        fn_body(&tricky, "lock").map(|b| page_reads(&b)),
        Some(1),
        "braces in strings, raw strings, chars and comments do not end the body: {tricky}"
    );
    for named in [
        "fn lock() {\n    let s = \"read_pages(\";\n}\n",
        "fn lock() {\n    let x = 1; // assert_no_inline_code(\"x\", &p);\n}\n",
        "fn lock() {\n    /* read_pages(&p); */\n}\n",
    ] {
        assert_eq!(
            fn_body(named, "lock").map(|b| page_reads(&b)),
            Some(0),
            "a call in a string or a comment reads no page: {named}"
        );
    }
    let local = "fn lock() {\n    fn read_pages(_: &[u8]) {}\n    read_pages(&[]);\n}\n";
    assert_eq!(
        fn_body(local, "lock"),
        None,
        "a lock fn with a fn of its own inside is no lock body: {local}"
    );
    for gone in [
        "fn other() {}\n",
        "// fn lock() {}\nfn other() {}\n",
        "fn lock();\n",
    ] {
        assert_eq!(fn_body(gone, "lock"), None, "no body of `lock` in: {gone}");
    }
}

/// The two locks that read a window's page boot past the fade by their own code, not only
/// by the values of [`past_the_fade`]: one fallen back to `Boot::default()` reads its window
/// inside the fade again and is red under load, while the value pin stays green
/// (welle-val2 v2-fix-scan, re-check A-M1). Read as code ([`fn_body`]), so a comment or a
/// string naming the helper proves nothing.
#[test]
fn the_window_locks_boot_past_the_fade() {
    let text = std::fs::read_to_string(repo(
        "crates/meclaw-cells/tests/gh867_no_surface_page_carries_unlisted_inline_script.rs",
    ))
    .expect("this file reads");
    for lock in [
        "every_inline_script_of_the_display_is_listed",
        "render_display",
    ] {
        let body = fn_body(&text, lock)
            .unwrap_or_else(|| panic!("`{lock}` is not in this file (or twice)"))
            .split_whitespace()
            .collect::<String>();
        assert_eq!(
            body.matches("boot_with_voice_echo(").count(),
            1,
            "`{lock}` boots one colony"
        );
        assert_eq!(
            body.matches("boot_with_voice_echo(past_the_fade())")
                .count(),
            1,
            "`{lock}` boots its colony with `past_the_fade()`: the helper's default fade ends \
             its window inside a settle under load"
        );
    }
}

/// Every HTML page Rust serves is read by a lock with the shared parser, and the list of
/// them is the source's, not a hand list: every file under `crates/*/src` with an HTML
/// marker must stand in [`RUST_HTML`] with its exact count, so a new `text/html` answer, a
/// new `Html(` or a new baked-in page -- in a new file or a listed one -- is red until it has
/// its lock. Every lock it names must exist; a listed file without its markers is red too.
#[test]
fn every_html_page_rust_serves_has_its_lock() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let crates = repo("crates");
    let mut found = std::collections::BTreeMap::new();
    let mut scanned = 0;
    for krate in std::fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        walk(&src, &mut files);
        for f in files {
            scanned += 1;
            let text =
                std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let n: usize = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(html_markers)
                .sum();
            if n > 0 {
                let rel = f.strip_prefix(&crates).expect("under crates/");
                found.insert(rel.to_string_lossy().replace('\\', "/"), n);
            }
        }
    }
    assert!(
        scanned > 100,
        "the walk read {scanned} source files under crates/*/src -- not the tree"
    );
    let listed: std::collections::BTreeMap<String, usize> = RUST_HTML
        .iter()
        .map(|(f, n, _, _)| (f.to_string(), *n))
        .collect();
    assert_eq!(
        found, listed,
        "the HTML the crates serve is another set than the one whose locks this list names: a \
         new or moved `text/html`/`Html(`/doctype/`.html` page needs a lock that reads it with \
         `meclaw_testing::page_scripts` (or the reason it is no page), then its line here"
    );
    let mut pages = 0;
    for (file, _, locks, why) in RUST_HTML {
        assert!(
            locks.is_empty() != why.is_empty(),
            "{file}: a line names its locks or says why it serves no page, not both or neither"
        );
        for (test_file, test_fn) in *locks {
            let text = std::fs::read_to_string(repo(test_file))
                .unwrap_or_else(|e| panic!("{file} is read by {test_file}, which is gone: {e}"));
            let body = fn_body(&text, test_fn).unwrap_or_else(|| {
                panic!("{file} is read by {test_file}::{test_fn}, which is not there (or twice)")
            });
            // Exactly one read: a lock fn that reads other pages beside its own proves the
            // read of its own page by theirs (re-check tv-bn M9g, B-M1 (a): the voice lock
            // stayed proven by the display reads beside it).
            assert_eq!(
                page_reads(&body),
                1,
                "{file} is read by {test_file}::{test_fn}, and that fn does not read exactly one \
                 page with `meclaw_testing::page_scripts` (`assert_no_inline_code` or `read_pages`)"
            );
        }
        pages += usize::from(!locks.is_empty());
    }
    assert!(
        pages >= 13,
        "{pages} files serve a page with a lock -- fewer than the 13 this list was written for"
    );
}
