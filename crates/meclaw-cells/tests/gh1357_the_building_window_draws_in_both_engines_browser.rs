//! live-2 GR (#1357) -- a window under construction draws in BOTH engines (R-23-10),
//! in full contrast and still under reduced motion (display-hive.md § 1.4, § 6.17).
//!
//! Two stacks rendered with the `web` cell's own renderer out of the templates
//! `compose.py` defines, put into the leading window of the lab's stage on the sheet
//! that ships: one that says `build: building` (the construction pictogram,
//! `display-status` with `kind: building` and no text, and two `display-skeleton`s) and
//! one that says `build: done` with `settle_ms` 600 (a card). Measured by
//! `workshop/tools/display-building-browser.mjs`, once with motion and once under
//! reduced motion: the pictogram has a box and no text, nothing of the building content
//! is greyed (opacity 1 and no filter from every element up to the pane), the skeleton's
//! head is readable, the frame lies on the pane's edge, the building frame breathes and
//! the done frame settles once over 0.6 s -- and under reduced motion both stand still.
//! No screenshot: the driver's checks are the verdict.
//!
//! **Why SKIP and not RED** -- the tool guard of every browser proof in this tree (R2b):
//! without `playwright`, the bundle, the laboratory or `node` the driver says `SKIP`
//! and leaves with 3; under `MECLAW_REQUIRE_BROWSERS=1` a skip is red.

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
const DRIVER: &str = "workshop/tools/display-building-browser.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the driver's default engine is WebKit). Sourced for Chromium too, a host without
/// the laboratory read its `SKIP` line as the Chromium run's own, and the lock proved
/// nothing there (XB-M review I-1: build02 skipped every Chromium run).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments.
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";
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
    let building = [
        render(all, "display-status", json!({"kind": "building"}), &[]),
        render(
            all,
            "display-skeleton",
            json!({"block": "display-list", "title": "Items", "lines": 3}),
            &[],
        ),
        render(
            all,
            "display-skeleton",
            json!({"block": "display-card", "title": "", "lines": 2}),
            &[],
        ),
    ];
    let done = [render(
        all,
        "display-card",
        json!({"kicker": "Today", "title": "Weather", "body": "Mild and dry all day."}),
        &[],
    )];
    [
        render(
            all,
            "display-stack",
            json!({"gap": "s", "build": "building"}),
            &building,
        ),
        render(
            all,
            "display-stack",
            json!({"gap": "s", "build": "done", "settle_ms": 600}),
            &done,
        ),
    ]
    .concat()
}

fn the_blocks_draw_in(engine: &str) {
    if !library_ships() {
        println!("SKIP the template library does not ship in this tree");
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under the switch: the template library does not ship in this tree"
        );
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under the switch: the browser driver does not ship in this tree"
        );
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let Some(all) = page_parts(td.path()) else {
        println!("SKIP no python3 on this host");
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under the switch: no python3 on this host"
        );
        return;
    };
    std::fs::write(td.path().join("blocks.html"), blocks(&all)).expect("blocks.html");
    let out = match Command::new("sh")
        .arg("-c")
        .arg(WITH_LAB)
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
            assert_ne!(
                std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
                Ok("1"),
                "SKIP under the switch: neither node nor a shell for it on this host"
            );
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        // GH #1161 (OR-TB.S3.39): the gate's station booked this engine and runs with
        // `MECLAW_REQUIRE_BROWSERS=1`; there a skipped driver is a red test, never a pass.
        assert!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() != Ok("1"),
            "a skip under MECLAW_REQUIRE_BROWSERS=1 is red:\n{stderr}"
        );
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
        "{engine}: the window under construction does not hold -- {failed:?}\n{report:#}\n{stderr}"
    );
}

#[test]
fn building_state_renders_full_contrast_and_still_under_reduced_motion_in_chromium() {
    the_blocks_draw_in("chromium");
}

#[test]
fn building_state_renders_full_contrast_and_still_under_reduced_motion_in_webkit() {
    the_blocks_draw_in("webkit");
}
