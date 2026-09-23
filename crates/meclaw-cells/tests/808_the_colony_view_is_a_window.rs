//! GH #808 -- the colony view is a window the curator can read.
//!
//! Until `colony-view@1.1.3` the layout emitted its `colony-view-shell` as the
//! ROOT of the view. The display's `unwrap_window` looks for the first
//! `display-pane`/`display-panel`/`display-overlay`/`display-view-prose` in a
//! tree, found none, and read the shell's props (`title`, `viewbox`, `cells`,
//! ...) as the window's hints: no `context`, no `relevance`, no `pinned`, no
//! tile. With the curator's defaults that is weight 0.5 x relevance 0.5 = 0.25
//! below a bar of 0.3 -- never present, never in the dock, nothing to tap.
//!
//! This is the script lock: `layout.py` runs as a subprocess on a two-cell
//! snapshot, and the display's own `hints_of_row` reads what came out, so the
//! vocabulary is checked by the reader that matters rather than by a copy of
//! it. The colony lock beside it (`808_the_colony_view_stands_in_the_dock`)
//! measures the same claim at the `web` cell.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const LAYOUT: &str = "templates/colony-view/layout/layout.py";
const COMPOSE: &str = "templates/display/compose/compose.py";
const OWNER: &str = "/x/apps/colony-view/layout";

fn library_ships() -> bool {
    repo(LAYOUT).is_file() && repo(COMPOSE).is_file()
}

fn python(args: &[&str], stdin: &str) -> String {
    let mut child = Command::new("python3")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(
        out.status.success(),
        "python3 {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// One layout pass over a two-cell, one-edge snapshot of `/os`.
fn emit() -> Value {
    let doc = json!({
        "body": {"graph": {
            "scope": "/os",
            "nodes": [
                {"path": "/os/one", "cell_type": "code"},
                {"path": "/os/two", "cell_type": "store"}
            ],
            "edges": [{"from": "/os/one", "to": "/os/two"}]
        }},
        "envelope": {"target": OWNER}
    });
    let out = python(&[repo(LAYOUT).to_str().expect("path")], &doc.to_string());
    meclaw_core::serde_json::from_str(&out).expect("the layout answers JSON")
}

/// The display's own reading of a store row carrying `content`.
fn hints(content: &Value) -> Value {
    let script = format!(
        "import importlib.util, json, sys\n\
         spec = importlib.util.spec_from_file_location('compose', {path:?})\n\
         m = importlib.util.module_from_spec(spec)\n\
         spec.loader.exec_module(m)\n\
         row = json.load(sys.stdin)\n\
         print(json.dumps(m.hints_of_row(row)))\n",
        path = repo(COMPOSE).to_str().expect("path")
    );
    let row = json!({"kind": "component", "content": content.to_string()});
    let out = python(&["-c", &script], &row.to_string());
    meclaw_core::serde_json::from_str(&out).expect("hints are JSON")
}

#[test]
fn the_view_is_a_pane_around_the_shell_with_a_tile() {
    if !library_ships() {
        return;
    }
    let view = emit();
    assert_eq!(view["header"]["route"], "view");
    assert_eq!(view["kind"], "component");
    let win = &view["content"];
    assert_eq!(
        win["component"], "display-pane",
        "the root of the view is a window the curator reads, not the shell: {}",
        win["component"]
    );
    let p = &win["props"];
    assert!(
        p["pane_id"].as_str().is_some_and(|s| !s.is_empty()),
        "a pane without `pane_id` has no DOM id, and the dock tile's `data-for` points at nothing"
    );
    assert_ne!(
        p["pane_id"], "colony-view",
        "the shell already carries the DOM id `colony-view`; a pane with the same one is two elements with one id"
    );
    assert_eq!(p["context"], "system");
    assert_eq!(p["pinned"], json!(true));
    assert_eq!(p["topic"], "colony");
    let relevance: f64 = p["relevance"]
        .as_str()
        .expect("relevance travels as text (display README: numbers travel as text)")
        .parse()
        .expect("relevance is a number");
    assert!((0.0..=1.0).contains(&relevance), "relevance {relevance}");
    let touched = p["touched"].as_str().expect("`touched` travels as text");
    assert!(
        touched.parse::<u64>().is_ok_and(|t| t > 0),
        "`touched` is an integer epoch: {touched}"
    );

    let kids = win["children"].as_array().expect("the window has children");
    assert_eq!(kids.len(), 2, "a tile and the shell: {kids:?}");
    assert_eq!(kids[0]["component"], "display-tile");
    assert_eq!(kids[0]["key"], "tile");
    assert_eq!(kids[0]["props"]["value"], "2", "the tile counts the cells");
    assert_eq!(kids[1]["component"], "colony-view-shell");
    assert_eq!(kids[1]["props"]["cells"], json!(2));
}

#[test]
fn the_curator_reads_the_window_and_its_tile() {
    if !library_ships() {
        return;
    }
    let view = emit();
    let h = hints(&view["content"]);
    assert_eq!(
        h["pinned"],
        json!(true),
        "hints_of_row must read `pinned` off the window: {h}"
    );
    assert_eq!(h["context"], "system");
    assert_eq!(h["topic"], "colony");
    assert_eq!(
        h["children"]["tile"]["value"], "2",
        "the curator finds the tile under the window: {h}"
    );
    assert!(
        h.get("viewbox").is_none() && h.get("cells").is_none(),
        "the shell's props are not hints any more: {h}"
    );
}

#[test]
fn every_snapshot_touches_the_window_again() {
    if !library_ships() {
        return;
    }
    let first = emit();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = emit();
    let t = |v: &Value| -> u64 {
        v["content"]["props"]["touched"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .expect("touched")
    };
    assert!(
        t(&second) > t(&first),
        "a later snapshot says a greater `touched` (§ 4.8 c), or a committed mutation \
         never lifts the picture: {} then {}",
        t(&first),
        t(&second)
    );
}

/// The ids a drag writes to are the ids the display mints for the WINDOWED tree.
///
/// The shell moved one level down when the pane came around it, so every
/// `data-oid` the layout computes moves with it -- or GH #544 is back: a drag
/// that writes to an object that does not exist.
#[test]
fn a_box_still_names_the_object_the_display_mints() {
    if !library_ships() {
        return;
    }
    let script = format!(
        "import importlib.util, json, sys\n\
         def load(p, n):\n\
         \x20   s = importlib.util.spec_from_file_location(n, p)\n\
         \x20   m = importlib.util.module_from_spec(s)\n\
         \x20   s.loader.exec_module(m)\n\
         \x20   return m\n\
         c = load({compose:?}, 'compose')\n\
         view = json.load(sys.stdin)\n\
         wrapper = 'view.' + {owner:?}.replace('/', '~') + '.colony-view'\n\
         want, tiles = {{}}, {{}}\n\
         c.add_tree(want, wrapper, view['content'], 0, tiles)\n\
         claimed = []\n\
         def walk(n):\n\
         \x20   o = (n.get('props') or {{}}).get('oid')\n\
         \x20   if o: claimed.append(o)\n\
         \x20   for k in n.get('children') or []: walk(k)\n\
         walk(view['content'])\n\
         print(json.dumps({{'claimed': claimed, 'minted': sorted(want), 'tiles': len(tiles)}}))\n",
        compose = repo(COMPOSE).to_str().expect("path"),
        owner = OWNER,
    );
    let out = python(&["-c", &script], &emit().to_string());
    let got: Value = meclaw_core::serde_json::from_str(&out).expect("json");
    let minted: Vec<&str> = got["minted"]
        .as_array()
        .expect("minted")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let claimed = got["claimed"].as_array().expect("claimed");
    assert!(
        claimed.len() >= 3,
        "two boxes and a frame claim an oid: {got}"
    );
    for oid in claimed {
        let oid = oid.as_str().expect("an oid is text");
        assert!(
            minted.contains(&oid),
            "the picture claims {oid}, which the display never mints: {minted:?}"
        );
    }
    assert_eq!(
        got["tiles"],
        json!(1),
        "the display takes the tile out: {got}"
    );
}
