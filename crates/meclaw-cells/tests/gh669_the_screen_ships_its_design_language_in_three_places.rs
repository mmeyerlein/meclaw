//! GH #669 -- the screen ships its design language, and ships it as bytes.
//!
//! `display-dna.css` is the sheet a person reads, greps and diffs. What this
//! file pins is the part that is true before anything runs: the sheet exists in
//! the template, carries the tokens the screen is built on, names itself once,
//! and is written in a form the component language can carry without either
//! side mistaking the other's brackets.
//!
//! Since OR-G0.1 the copies that TRAVEL carry the sheet without its reasoning:
//! the source keeps every comment, and `KIT_CSS` gets the rules plus the head
//! comment, which is the nameplate and the way back here. So the drift check
//! below is no longer `==` on the raw file. It asks
//! `scripts/display_sheet_strip.py` -- the one place that rule lives, the same
//! one `display_sync.py` copies with -- rather than restating it in Rust,
//! because two spellings of one rule are two places a copy can be wrong, and
//! catching exactly that is what this file is for.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! library is skipped, never judged. The sheet itself is NOT part of the guard
//! -- a sheet that disappears is a red test, not a skipped one.

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";
const CONFIG: &str = "templates/display/compose/config.json";
const STRIP: &str = "scripts/display_sheet_strip.py";

/// The sentence the sheet says about itself, once. The gallery that drew the
/// sheet's ancestors carries a different one, and a sheet that reaches a
/// running screen is recognised by this string alone.
const MARKER: &str = "meclaw display DNA v1 (kit 6)";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn sheet() -> String {
    read(SHEET)
}

/// The sheet as it TRAVELS: the source put through `scripts/display_sheet_strip.py`,
/// the rule `display_sync.py` copies with. `None` when there is no `python3` on this
/// host, like every other interpreter guard in this tree (R2b).
fn sheet_as_it_travels() -> Option<String> {
    let out = std::process::Command::new("python3")
        .arg(repo(STRIP))
        .arg(repo(SHEET))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "{STRIP} refused the sheet:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8(out.stdout).expect("the stripped sheet is text"))
}

/// Pull one `NAME = r\"\"\"…\"\"\"` literal out of a Python source.
///
/// Deliberately dumb, like the one `gh455_the_two_templates_ship` uses for the
/// app's browser half: one assignment, one raw triple-quoted string, no escapes
/// to interpret. A splice gate that can be argued with is not a gate.
fn extract_constant(src: &str, name: &str) -> Option<String> {
    let open = format!("{name} = r\"\"\"");
    let start = src.find(&open)? + open.len();
    let end = src[start..].find("\"\"\"")? + start;
    Some(src[start..end].to_string())
}

/// The copy a running cell gets: `KIT_CSS` out of the `script_inline` that
/// `config.json` carries. The file is what a person reads; this is what the
/// screen renders, and every byte check below runs on both.
fn shipped_sheet() -> String {
    let cfg: Value = meclaw_core::serde_json::from_str(&read(CONFIG))
        .unwrap_or_else(|e| panic!("{CONFIG}: {e}"));
    let inline = cfg["params"]["script_inline"]
        .as_str()
        .expect("compose declares script_inline");
    extract_constant(inline, "KIT_CSS")
        .expect("the shipped script_inline carries no extractable KIT_CSS")
}

/// `components()` as the shipped script defines them, asked of the script
/// itself. `None` when there is no `python3` on this host.
fn components() -> Option<Vec<Value>> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    Some(v.as_array().expect("a list").clone())
}

/// The `display-shell` definition out of `components()`.
fn shell() -> Option<Value> {
    components()?
        .into_iter()
        .find(|c| c["name"] == "display-shell")
        .or_else(|| panic!("components() defines no display-shell"))
}

/// Render one component template with its props, the way the `web` cell does
/// for one object with no children (`render_pieces_plain`: the cell's parser,
/// the cell's walk, the cell's truthiness).
fn render(template: &str, props: &Value, schema: &Value) -> String {
    render_pieces_plain(template, props, schema)
        .unwrap_or_else(|e| panic!("the web cell would refuse this template: {e}"))
}

/// The shell as the screen's root renders it: the stylesheet linked, no faces.
fn rendered_shell() -> Option<String> {
    let shell = shell()?;
    let template = shell["template"].as_str().expect("a template");
    Some(render(
        template,
        &json!({"stylesheet": true, "faces": ""}),
        &shell["prop_schema"],
    ))
}

/// The line that declares one custom property, for a value check that cannot
/// be satisfied by the same token appearing somewhere else in the sheet.
fn declaration<'a>(sheet: &'a str, token: &str) -> &'a str {
    let needle = format!("{token}:");
    sheet
        .lines()
        .find(|l| l.trim_start().starts_with(&needle))
        .unwrap_or_else(|| panic!("the sheet declares no `{token}`"))
}

#[test]
fn the_sheet_is_the_source_of_the_design_language() {
    if !library_ships() {
        return;
    }
    assert!(repo(SHEET).is_file(), "{SHEET} does not ship");
    let sheet = sheet();

    // The size is the cheapest proof that the whole sheet arrived and not a
    // stub: the kit measures about fifty thousand bytes without its faces.
    assert!(
        sheet.len() > 40_000,
        "the sheet is {} bytes; the design language is not that short",
        sheet.len()
    );

    // The three tokens the screen is built on, checked on their declaration
    // lines. The window radius is the DNA's own correction of visionOS, the
    // ground is what everything stands on, and the tertiary tier stays at the
    // gallery's .55 (M-B1).
    assert!(
        declaration(&sheet, "--r-window").contains("22px"),
        "the window radius is 22px"
    );
    declaration(&sheet, "--ground");
    assert!(
        declaration(&sheet, "--fg-tertiary").contains(".55"),
        "the tertiary tier stays at .55: {}",
        declaration(&sheet, "--fg-tertiary")
    );

    // P-B2: a window nobody assigned a rung to stands at the top rung -- at
    // 0-1-0, like every rung, so the forced-colours fallback can take it back.
    // Since GH #679 the rule covers both windows that scroll and those that
    // do not: `:is()` on the class keeps the one class of specificity. The
    // word is `rung` since 2.5.0 (display-hive.md § 2: `state` is struck).
    assert!(
        sheet.contains(
            ":is(.display-pane, .display-panel):where(:not([data-rung]), [data-rung=\"\"]) {"
        ),
        "the focus default is stated on the two forms of `no state`, inside `:where()`"
    );
    assert!(
        !sheet.contains(".display-pane:not([data-rung])"),
        "no 0-2-0 form of the default is left in the sheet"
    );

    // The nine keyframes travel under the screen's own prefix, every one of
    // them; a keyframe left under the old name is an animation that no rule
    // in this sheet can reach.
    for name in [
        "light-drift",
        "enter",
        "leave",
        "pulse",
        "notification-breathe",
        "urgent-breathe",
        "timer-drain",
        "timer-heat",
        "timer-alarm",
    ] {
        assert!(
            sheet.contains(&format!("@keyframes display-{name} ")),
            "the keyframe `display-{name}` is missing"
        );
    }
}

#[test]
fn the_sheet_names_itself_once() {
    if !library_ships() {
        return;
    }
    for (which, sheet) in [
        ("the source", sheet()),
        ("the shipped copy", shipped_sheet()),
    ] {
        assert_eq!(
            sheet.matches(MARKER).count(),
            1,
            "{which}: the marker `{MARKER}` stands exactly once in the head comment"
        );
        // The prefix swap is complete or it is not done: one `v2v-` left is a
        // class no template of this scope writes, or a keyframe no rule reaches.
        let stray: Vec<&str> = sheet.lines().filter(|l| l.contains("v2v-")).collect();
        assert!(
            stray.is_empty(),
            "{which}: `v2v-` survives the prefix swap: {stray:#?}"
        );
    }
}

/// The sheet travels inside a component template and inside a Python raw
/// string. Two `{` or two `}` are the template language's own marker,
/// `</style` would end the block the shell wraps it in, and `"""` would end the
/// raw string the drift lock extracts. The sheet is never minified, so none of
/// these can arise from formatting alone.
#[test]
fn the_sheet_is_written_so_the_template_language_can_carry_it() {
    if !library_ships() {
        return;
    }
    for (which, sheet) in [
        ("the source", sheet()),
        ("the shipped copy", shipped_sheet()),
    ] {
        for forbidden in ["{{", "}}", "</style", "\"\"\""] {
            let hits: Vec<(usize, &str)> = sheet
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains(forbidden))
                .map(|(i, l)| (i + 1, l))
                .collect();
            assert!(
                hits.is_empty(),
                "{which} carries `{forbidden}`, which the carrier would misread: {hits:#?}"
            );
        }
    }
}

/// The sheet is a file a person reads, greps and diffs, and it is the same sheet
/// in three places: the file, the `KIT_CSS` constant in `compose.py`, and the copy
/// of `compose.py` inside `config.json`. The same arrangement `colony-view` keeps
/// for its browser half, with the same lock. A running `code` cell has no working
/// directory to read the file from, so the constant is what reaches the screen, and
/// this is what makes an edit to the file an edit to the screen.
///
/// The two that travel carry the sheet WITHOUT its reasoning (OR-G0.1), so the first
/// comparison runs through the strip rule and the second is still byte for byte --
/// `config.json` copies `compose.py` whole and has nothing of its own to decide.
#[test]
fn the_design_language_is_the_same_sheet_in_three_places() {
    if !library_ships() {
        return;
    }
    let Some(travels) = sheet_as_it_travels() else {
        return;
    };
    let in_compose = extract_constant(&read(COMPOSE), "KIT_CSS")
        .expect("compose.py carries no extractable KIT_CSS");
    assert_eq!(
        in_compose, travels,
        "compose.py's KIT_CSS has drifted from display-dna.css (run scripts/display_sync.py)"
    );
    assert_eq!(
        shipped_sheet(),
        in_compose,
        "config.json's script_inline has drifted from compose.py (run scripts/display_sync.py)"
    );
    // And the third copy is the whole script, not just the sheet inside it. Until
    // now only the extracted `KIT_CSS` was compared here, so `compose.py` could
    // have changed anywhere ELSE -- the curator's own pass, for one -- and
    // `config.json` would have gone on shipping yesterday's. `display_sync.py`
    // writes the file verbatim into the key, so this holds by construction; a
    // lock is what makes it hold after the next hand edit.
    let cfg: Value = meclaw_core::serde_json::from_str(&read(CONFIG))
        .unwrap_or_else(|e| panic!("{CONFIG}: {e}"));
    assert_eq!(
        cfg["params"]["script_inline"].as_str().expect("a string"),
        read(COMPOSE),
        "config.json's script_inline is not compose.py (run scripts/display_sync.py)"
    );
}

/// What the source keeps and the copies do not: the reasons. The sheet is where a
/// change is argued, and the argument has to be IN it -- a sheet whose comments
/// thinned out to nothing would pass the drift check above and still have lost the
/// thing OR-G0.1 was meant to protect.
#[test]
fn the_source_keeps_the_reasons_the_copies_drop() {
    if !library_ships() {
        return;
    }
    let Some(travels) = sheet_as_it_travels() else {
        return;
    };
    let on_disk = sheet();
    assert!(
        on_disk.matches("/*").count() > 100,
        "the source argues for itself in {} comments; that is too few",
        on_disk.matches("/*").count()
    );
    assert_eq!(
        travels.matches("/*").count(),
        1,
        "what travels carries the head comment and no other"
    );
    assert!(
        on_disk.len() > travels.len() + 40_000,
        "the copies should be far lighter than the source: {} against {}",
        travels.len(),
        on_disk.len()
    );
}

/// P-B1: the shell renders exactly ONE `<style>` block, and the sheet is in it.
/// The page carries two blocks after that -- LiveView's and this one -- and a
/// third would be the shell's layout rules and the sheet in separate tags.
#[test]
fn the_shell_renders_one_style_block() {
    if !library_ships() {
        return;
    }
    let Some(shell) = shell() else {
        return;
    };
    let template = shell["template"].as_str().expect("a template");
    assert_eq!(template.matches("<style>").count(), 1, "one opening tag");
    assert_eq!(template.matches("</style>").count(), 1, "one closing tag");
    assert!(
        template.contains(MARKER),
        "the sheet is inside the shell's style block"
    );
    // The faces are a prop the shell renders RAW: an `@font-face` block
    // rendered escaped is not an `@font-face`. `html` is what makes `{{&…}}`
    // raw in the web cell.
    assert!(
        template.contains("{{&faces}}"),
        "the faces enter through a prop"
    );
    assert_eq!(shell["prop_schema"]["faces"], "html");
}

/// The sheet stands LAST: after the link to `/vision.css`, whose tokens it
/// re-tokenises at equal specificity, and after the shell's own layout rules.
#[test]
fn the_sheet_stands_after_the_link_and_after_the_layout() {
    if !library_ships() {
        return;
    }
    let Some(page) = rendered_shell() else {
        return;
    };
    let link = page.find("vision.css").expect("the base sheet is linked");
    let layout = page
        .find(".display-columns {")
        .expect("the two-column rule is in the shell");
    let sheet = page.find(MARKER).expect("the sheet is in the shell");
    assert!(link < layout, "the link stands before the layout rules");
    assert!(layout < sheet, "the layout rules stand before the sheet");
    // And the page is one style block, not one per source.
    assert_eq!(page.matches("<style>").count(), 1);
    assert_eq!(page.matches("</style>").count(), 1);
}
