//! GH #958 -- the catalogue's new blocks draw in BOTH engines (R-23-10).
//!
//! `display-card`, `display-steps` with a `display-step` in each of its five
//! states, and `display-status` with `kind: working` -- the first answer of a
//! window whose content is still on its way. The blocks are rendered here with
//! the `web` cell's own renderer out of the templates `compose.py` defines, put
//! into the leading window of the lab's stage on the sheet that ships, and
//! measured by `workshop/tools/display-blocks-browser.mjs`: every block has a
//! box, its text is readable, no two blocks and no two steps overlap, every step
//! wears its state and draws its mark beside its label, and the running step
//! and the working stroke animate; a standing `listening` status keeps the dot
//! it shipped with (GH #966).
//!
//! **Why a new file and not a section of the sheet lock** (OR-DP.K.3): the
//! sheet lock drives the B-sentences of display-hive.md on a stage of windows
//! the driver builds itself; these blocks are content INSIDE a window, rendered
//! from the catalogue, and a red here should name the blocks, not a B-number.
//!
//! **Why SKIP and not RED** -- the same tool guard every browser proof in this
//! tree uses (R2b): without `playwright`, the bundle, the laboratory or `node`
//! the driver says `SKIP` on a line of its own and leaves with 3.

use std::path::Path;
use std::process::Command;

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-blocks-browser.mjs";
const WKENV: &str = "workshop/tools/wkenv.sh";
/// Where a parent's children go while it is rendered alone: `render_pieces_plain`
/// draws `{{children}}` as nothing, so the slot is swapped for this text first.
const SLOT: &str = "@@children@@";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The sheet, the scene and `components()` as `compose.py` ships them.
fn page_parts(dir: &Path) -> Option<Vec<Value>> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys, json\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             d = sys.argv[2]\n\
             open(d + '/sheet.css', 'w').write(m.LAYOUT_RULES + m.KIT_CSS)\n\
             open(d + '/scene.js', 'w').write(m.SCENE_CLIENT_JS)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .arg(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    Some(v.as_array().expect("a list").clone())
}

/// One node and its children, rendered the way the `web` cell renders them.
fn render(all: &[Value], name: &str, props: Value, children: &[String]) -> String {
    let c = all
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is not defined"));
    let template = c["template"]
        .as_str()
        .expect("a template")
        .replace("{{children}}", SLOT);
    render_pieces_plain(&template, &props, &c["prop_schema"])
        .unwrap_or_else(|e| panic!("the web cell would refuse `{name}`: {e}"))
        .replace(SLOT, &children.concat())
}

fn blocks(all: &[Value]) -> String {
    let steps: Vec<String> = [
        ("Plan", "done", "agreed", "09:00"),
        ("Build", "running", "second of three", "10:00"),
        ("Review", "todo", "", ""),
        ("Ship", "failed", "the gate went red", "11:30"),
        ("Announce", "blocked", "waits on ship", ""),
    ]
    .iter()
    .map(|(label, state, detail, at)| {
        render(
            all,
            "display-step",
            json!({"label": label, "state": state, "detail": detail, "at": at}),
            &[],
        )
    })
    .collect();
    [
        render(
            all,
            "display-card",
            json!({"kicker": "Today", "title": "Weather", "value": "21", "unit": "°C",
                   "body": "Mild and dry all day."}),
            &[],
        ),
        render(
            all,
            "display-steps",
            json!({"title": "Critical path"}),
            &steps,
        ),
        render(
            all,
            "display-status",
            json!({"kind": "working", "text": "Fetching the forecast"}),
            &[],
        ),
        // GH #966 (K review M5): a standing status beside it, whose dot keeps the
        // look it shipped with -- the working hint's box rule does not reach it.
        render(
            all,
            "display-status",
            json!({"kind": "listening", "text": "Listening"}),
            &[],
        ),
    ]
    .concat()
}

fn the_blocks_draw_in(engine: &str) {
    if !library_ships() {
        println!("SKIP the template library does not ship in this tree");
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let Some(all) = page_parts(td.path()) else {
        println!("SKIP no python3 on this host");
        return;
    };
    std::fs::write(td.path().join("blocks.html"), blocks(&all)).expect("blocks.html");
    let out = match Command::new("sh")
        .arg("-c")
        .arg(". \"$1\"; shift; exec node \"$@\"")
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(td.path())
        .arg(td.path().join("shots"))
        .arg("--engine")
        .arg(engine)
        .arg("--profile")
        .arg("monitor")
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            println!("SKIP neither node nor a shell for it on this host: {e}");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("SKIP {engine}: {}", stderr.trim());
        return;
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("the driver printed no report:\n{stdout}\n{stderr}"));
    let report: Value = meclaw_core::serde_json::from_str(line).expect("the report is JSON");
    let failed: Vec<&String> = report["checks"]
        .as_object()
        .expect("checks")
        .iter()
        .filter(|(_, c)| c["ok"] != true)
        .map(|(k, _)| k)
        .collect();
    assert!(
        failed.is_empty() && report["ok"] == true,
        "{engine}: the blocks do not hold -- {failed:?}\n{report:#}\n{stderr}"
    );
}

#[test]
fn the_card_and_the_steps_draw_in_chromium() {
    the_blocks_draw_in("chromium");
}

#[test]
fn the_card_and_the_steps_draw_in_webkit() {
    the_blocks_draw_in("webkit");
}
