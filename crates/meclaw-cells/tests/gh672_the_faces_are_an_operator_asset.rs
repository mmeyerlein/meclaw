//! GH #672 -- the two faces are an operator asset, never bytes in this tree.
//!
//! The screen's sheet names `Inter` and `Fraunces` at the head of two fallback
//! stacks and declares neither. Whether an `@font-face` reaches the page is a
//! param: `font_base` names the directory the two files are served from,
//! relative to the page's own base, and an empty base -- the default -- means no
//! declaration, no request and no 404. The `@font-face` rules are BUILT from the
//! param by the compose cell and enter the shell as a raw prop; nothing under
//! `templates/` is a font file, and no sheet embeds one.
//!
//! The shell is rendered the way the `web` cell renders one object: the cell's
//! own parser cuts the template, raw props stay raw only where `prop_schema`
//! says `html`. Skips when `python3` is absent or the templates do not ship,
//! like every other interpreter guard in this tree (R2b).

use meclaw_cells::web::render::{Piece, escape, parse_template};
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const CONFIG: &str = "templates/display/compose/config.json";
const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn read_json(rel: &str) -> Value {
    meclaw_core::serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// What the shipped script says: its components, the faces it builds for an
/// empty and for a set base, and the root props `build()` writes when the
/// module-level base is set the way the dispatcher sets it from `params`.
/// `None` when there is no `python3` on this host.
fn probe() -> Option<Value> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             m.FONT_BASE = 'fonts/'\n\
             root = m.build([])[m.ROOT_ID]['props']\n\
             print(json.dumps({'components': m.components(),\n\
                               'unset': m.faces(''),\n\
                               'set': m.faces('fonts/'),\n\
                               'bare': m.faces('fonts'),\n\
                               'root_faces': root.get('faces')}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the probe is JSON"))
}

/// Render one component template with its props, the way the `web` cell does
/// for one object with no children. A public renderer without a database does
/// not exist in the crate, so this is the faithful minimum: the cell's parser,
/// the cell's escaping, `html` as the only raw type, `{{children}}` empty.
fn render(template: &str, props: &Value, schema: &Value) -> String {
    let pieces = parse_template(template)
        .unwrap_or_else(|e| panic!("the web cell would refuse this template: {e}"));
    let mut out = String::new();
    render_pieces(&pieces, props, schema, &mut out);
    out
}

fn render_pieces(pieces: &[Piece], props: &Value, schema: &Value, out: &mut String) {
    let text = |name: &str| -> String {
        match props.get(name) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    };
    for piece in pieces {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Prop(name) => out.push_str(&escape(&text(name))),
            Piece::Raw(name) => {
                if schema.get(name).and_then(Value::as_str) == Some("html") {
                    out.push_str(&text(name));
                } else {
                    out.push_str(&escape(&text(name)));
                }
            }
            Piece::Children => {}
            Piece::If { prop, body } => {
                let on = match props.get(prop) {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(b)) => *b,
                    Some(Value::String(s)) => !s.is_empty(),
                    Some(other) => !other.is_null(),
                };
                if on {
                    render_pieces(body, props, schema, out);
                }
            }
        }
    }
}

/// The `display-shell`, rendered with the given faces.
fn shell_with(probe: &Value, faces: &Value) -> String {
    let shell = probe["components"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|c| c["name"] == "display-shell")
        .expect("components() defines no display-shell");
    render(
        shell["template"].as_str().expect("a template"),
        &json!({"stylesheet": true, "faces": faces}),
        &shell["prop_schema"],
    )
}

#[test]
fn an_unset_base_declares_no_face() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let page = shell_with(&probe, &probe["unset"]);
    assert_eq!(
        page.matches("@font-face").count(),
        0,
        "an empty base declares nothing: the fallback stacks carry the type"
    );

    // The default IS empty, in both places a caller reads it: the param the
    // cell runs with, and the contract line that describes it. A set-but-unserved
    // base would send two requests into nothing, so empty is the shipped value.
    let cfg = read_json(CONFIG);
    assert_eq!(
        cfg["params"]["font_base"], "",
        "params.font_base defaults to empty"
    );
    let spec = &cfg["contract"]["settings"]["font_base"];
    assert_eq!(spec["type"], "string");
    assert_eq!(spec["secret"], false);
    assert_eq!(spec["default"], "");
    let description = spec["description"].as_str().expect("described");
    assert!(
        description.contains("Empty means no @font-face"),
        "the contract says what empty means: {description}"
    );
    assert!(
        description.contains("No font file ships in this repository"),
        "the contract says the faces are not in the tree: {description}"
    );
}

#[test]
fn a_set_base_declares_exactly_two_relative_faces() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let page = shell_with(&probe, &probe["set"]);
    assert_eq!(page.matches("@font-face").count(), 2, "one face per file");
    assert_eq!(
        page.matches("url(\"fonts/").count(),
        2,
        "both faces are RELATIVE to the page's own base: {page}"
    );
    assert!(page.contains("url(\"fonts/inter.woff2\") format(\"woff2\")"));
    assert!(page.contains("url(\"fonts/fraunces.woff2\") format(\"woff2\")"));
    assert!(
        !probe["set"].as_str().expect("faces").contains("http"),
        "an absolute URL would be an external request, or would break under the next proxy prefix"
    );
    assert_eq!(page.matches("font-display: swap").count(), 2);
    assert!(
        page.contains("font-variation-settings: \"SOFT\" 55, \"WONK\" 0"),
        "Fraunces carries its two axes"
    );

    // The faces reach the shell as the ROOT'S prop, built from the base the
    // dispatcher reads out of `params`: what `build()` writes on the root is
    // what `faces()` makes of the module-level base.
    assert_eq!(
        probe["root_faces"], probe["set"],
        "the root's `faces` prop is faces(FONT_BASE)"
    );
    let src = read(COMPOSE);
    assert!(
        src.contains("params.get(\"font_base\")"),
        "the base is read from params, like voice_mount"
    );
    // A directory, with or without its trailing slash: a base glued to a file
    // name (`fontsinter.woff2`) is a request into nothing.
    assert_eq!(
        probe["bare"], probe["set"],
        "`fonts` and `fonts/` serve the same two files"
    );
}

/// The param path, as behaviour: the shipped script is run the way a `code`
/// cell runs it, with `params.font_base` on the document, on a bootstrap pass.
/// With a base set, the root the bootstrap creates carries two `@font-face`
/// under that base; with the param absent it carries none.
#[test]
fn the_base_reaches_the_root_from_params() {
    if !library_ships() {
        return;
    }
    let Some(with) = bootstrap_root(Some("fonts/")) else {
        return;
    };
    let faces = with["props"]["faces"].as_str().expect("faces is a string");
    assert_eq!(faces.matches("@font-face").count(), 2, "{faces}");
    assert_eq!(faces.matches("url(\"fonts/").count(), 2, "{faces}");
    assert!(faces.contains("fonts/inter.woff2") && faces.contains("fonts/fraunces.woff2"));

    let without = bootstrap_root(None).expect("python3 answered once already");
    assert_eq!(
        without["props"]["faces"], "",
        "no param, no face: {}",
        without["props"]
    );
}

/// The `object.create` of the root out of one bootstrap read pass, driven over
/// stdin with the given `params.font_base` (or no `params` at all). `None`
/// when there is no `python3` on this host.
fn bootstrap_root(font_base: Option<&str>) -> Option<Value> {
    use std::io::Write;
    let mut doc = json!({
        "body": {"messages": []},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": [], "define": []}).to_string(),
            },
        }},
    });
    if let Some(base) = font_base {
        doc["params"] = json!({"font_base": base});
    }
    let mut child = std::process::Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(
        out.status.success(),
        "compose.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    let root = answer["messages"]
        .as_array()
        .expect("a bootstrap answers with a bundle")
        .iter()
        .map(|turn| -> Value {
            meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .find(|call| call["op"] == "object.create" && call["id"] == "display.root")
        .expect("the bootstrap creates the root");
    Some(root)
}

/// The global constraint of the wave, as a test: no `@font-face` in the shipped
/// sheet, no embedded font data, and no font file anywhere under `templates/`.
#[test]
fn no_font_file_ships_in_the_tree() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    assert!(
        !sheet.contains("@font-face"),
        "the shipped sheet declares a face"
    );
    assert!(
        !sheet.contains("data:font/"),
        "the shipped sheet embeds a font"
    );
    assert!(
        !sheet.contains("base64"),
        "the shipped sheet embeds binary data"
    );

    let mut stray = Vec::new();
    walk(&repo("templates"), &mut stray);
    assert!(
        stray.is_empty(),
        "a font file ships in templates/: {stray:?}"
    );
}

fn walk(dir: &std::path::Path, stray: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, stray);
        } else if matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("woff2" | "woff" | "ttf" | "otf")
        ) {
            stray.push(p);
        }
    }
}

/// Without a face the fallback stacks carry the type, so the two stacks stay
/// in the sheet with at least three fallbacks each behind the named face.
#[test]
fn the_sheet_keeps_its_fallback_stacks() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    for (token, face) in [("--font-ui", "Inter"), ("--font-voice", "Fraunces")] {
        let start = sheet
            .find(&format!("{token}:"))
            .unwrap_or_else(|| panic!("the sheet declares no {token}"));
        let end = sheet[start..].find(';').expect("a declaration ends") + start;
        let stack = &sheet[start..end];
        assert!(
            stack.contains(face),
            "{token} names {face} at the head of its stack: {stack}"
        );
        // Comma-separated: the face, then the fallbacks.
        let fallbacks = stack.split(',').count() - 1;
        assert!(
            fallbacks >= 3,
            "{token} keeps at least three fallbacks, has {fallbacks}: {stack}"
        );
    }
}
