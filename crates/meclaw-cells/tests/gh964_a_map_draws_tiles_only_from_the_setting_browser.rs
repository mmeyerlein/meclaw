//! GH #964 -- a map draws its tiles in BOTH engines (R-23-10), and only from the
//! operator's setting.
//!
//! Two maps of the same place are built the way a running screen builds them --
//! `compose.py`'s own tree walk (`add_tree`), once with `map_tiles` set to a test
//! host and once with the shipped empty setting -- rendered with the `web` cell's
//! own renderer out of the catalogue's templates, and put into the leading window
//! of the lab's stage on the sheet that ships. `workshop/tools/display-map-browser.mjs`
//! answers every request to the test host itself (a local tile server inside the
//! page's own network layer, tiles out of a fixture image) and measures: the first
//! map shows exactly four loaded tiles with the expected indices in a two-by-two
//! grid, its pin inside the tiles, its name, coordinates and credit line readable;
//! the second shows no image at all and still its name and coordinates; and the page
//! asked no host but the setting's for an image.
//!
//! `templates/display/csp.json` is unchanged (`img-src 'self' data:`): the tile
//! host is the operator's proxy line, named in the template README. This proof runs
//! without the CSP proxy; `gh867_a_display_runs_under_a_strict_csp_browser` stays
//! the CSP proof.
//!
//! **Why SKIP and not RED** -- the same tool guard every browser proof in this tree
//! uses (R2b): without `playwright`, the bundle, the laboratory or `node` the driver
//! says `SKIP` on a line of its own and leaves with 3.

use std::path::Path;
use std::process::Command;

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Map, Value};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-map-browser.mjs";
const WKENV: &str = "workshop/tools/wkenv.sh";
/// The test host. `.test` is reserved (RFC 2606): nothing outside the page's own
/// network layer could ever answer it.
const SETTING: &str = "https://tiles.example.test/{z}/{x}/{y}.png";
const SLOT: &str = "@@children@@";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The sheet and the scene as `compose.py` ships them, `components()`, and the two
/// maps as the screen's tree walk builds them: `{"components", "set", "unset"}`.
fn page_parts(dir: &Path) -> Option<Value> {
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
             node = {'component': 'display-map', 'props': {'lat': '52.52', 'lon': '13.405',\n\
                     'zoom': 12, 'w': 2, 'h': 2, 'label': 'Capital'}}\n\
             out = {'components': m.components()}\n\
             for name, setting in (('set', sys.argv[3]), ('unset', '')):\n\
             \x20   m.MAP_TILES = m.map_setting(setting)\n\
             \x20   m.MAP_ATTRIBUTION = 'Tiles: example data' if setting else ''\n\
             \x20   want = {}\n\
             \x20   m.add_tree(want, name, json.loads(json.dumps(node)), 0)\n\
             \x20   out[name] = want\n\
             print(json.dumps(out))",
        )
        .arg(repo(COMPOSE))
        .arg(dir)
        .arg(SETTING)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON"))
}

/// One object and everything under it, rendered the way the `web` cell renders them:
/// children by `parent`, in `ord`.
fn render(all: &[Value], objects: &Map<String, Value>, id: &str) -> String {
    let o = &objects[id];
    let name = o["component"].as_str().expect("a component");
    let c = all
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is not defined"));
    let mut kids: Vec<(&String, i64)> = objects
        .iter()
        .filter(|(_, k)| k["parent"] == id)
        .map(|(k, v)| (k, v["ord"].as_i64().unwrap_or(0)))
        .collect();
    kids.sort_by_key(|(k, ord)| (*ord, (*k).clone()));
    let inner: String = kids.iter().map(|(k, _)| render(all, objects, k)).collect();
    let template = c["template"]
        .as_str()
        .expect("a template")
        .replace("{{children}}", SLOT);
    render_pieces_plain(&template, &o["props"], &c["prop_schema"])
        .unwrap_or_else(|e| panic!("the web cell would refuse `{name}`: {e}"))
        .replace(SLOT, &inner)
}

fn the_map_draws_in(engine: &str) {
    if !library_ships() {
        println!("SKIP the template library does not ship in this tree");
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let Some(parts) = page_parts(td.path()) else {
        println!("SKIP no python3 on this host");
        return;
    };
    let all = parts["components"]
        .as_array()
        .expect("components()")
        .clone();
    let body: String = ["set", "unset"]
        .iter()
        .map(|which| {
            let objects = parts[*which].as_object().expect("objects");
            format!(
                "<section data-map=\"{which}\">{}</section>",
                render(&all, objects, &format!("{which}/0"))
            )
        })
        .collect();
    assert!(
        body.contains("tiles.example.test/12/2200/1342.png"),
        "the point's own tile is in the markup: {body}"
    );
    std::fs::write(td.path().join("blocks.html"), &body).expect("blocks.html");
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
        .arg("--host")
        .arg("tiles.example.test")
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
        "{engine}: the map does not hold -- {failed:?}\n{report:#}\n{stderr}"
    );
    // The indices the page asked for are the screen's: the four of the grid, no other.
    let mut asked: Vec<String> = report["checks"]["only_the_setting"]["asked"]
        .as_array()
        .expect("the driver lists what it was asked")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    asked.sort();
    asked.dedup();
    let mut want: Vec<String> = parts["set"]
        .as_object()
        .expect("objects")
        .values()
        .filter(|o| o["component"] == "display-map-tile")
        .map(|o| o["props"]["src"].as_str().unwrap_or("").to_string())
        .collect();
    want.sort();
    assert_eq!(
        asked, want,
        "{engine}: the page fetched exactly the screen's tiles"
    );
}

#[test]
fn the_map_draws_in_chromium() {
    the_map_draws_in("chromium");
}

#[test]
fn the_map_draws_in_webkit() {
    the_map_draws_in("webkit");
}
