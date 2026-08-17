//! GH #159 — which paths resolve to a surface, and which must not resolve at all.
//!
//! This is the file that decides what a browser can reach, so its cases are
//! written as attacks first and features second.

use meclaw_colony::surface::{LocateError, locate};
use std::fs;
use std::path::Path;

/// A colony root with one declared surface, one ordinary cell, and two stores
/// that hold real data and must stay unreachable.
fn fixture() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    write_cell(
        td.path(),
        "org/acme/canvy/render",
        r#"{
          "cell": {
            "type": "code",
            "surface": { "title": "Acme", "assets": "client" }
          },
          "params": { "runner": "python3", "script_inline": "pass" }
        }"#,
    );
    write_cell(
        td.path(),
        "org/acme/canvy/store",
        r#"{ "cell": { "type": "store" },
             "params": { "schema": { "canvas": { "kind": "text" } } } }"#,
    );
    write_cell(
        td.path(),
        "org/acme/memory/store",
        r#"{ "cell": { "type": "store" },
             "params": { "schema": { "facts": { "id": "text" } } } }"#,
    );
    td
}

fn write_cell(root: &Path, rel: &str, config: &str) {
    let dir = root.join("main").join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.json"), config).unwrap();
}

#[test]
fn a_declared_cell_resolves_with_its_paths() {
    let td = fixture();
    let l = locate(td.path(), "org/acme/canvy/render").expect("declared");
    assert_eq!(l.cell_path, "/org/acme/canvy/render");
    assert_eq!(l.cell_dir, td.path().join("main/org/acme/canvy/render"));
    assert_eq!(l.decl.title, "Acme");
    assert_eq!(l.decl.assets.as_deref(), Some("client"));
}

/// The reason this whole module exists. An undeclared cell holds real data —
/// session windows, an affinity store, a vault — and must be unreachable. Both
/// stores in the fixture are undeclared, including the surface's OWN store: the
/// surface is the renderer, and the data behind it is not addressable.
#[test]
fn an_undeclared_cell_is_a_miss() {
    let td = fixture();
    for path in ["org/acme/memory/store", "org/acme/canvy/store"] {
        assert!(
            matches!(locate(td.path(), path), Err(LocateError::NoSurface)),
            "{path} must not be reachable"
        );
    }
}

#[test]
fn a_path_with_no_cell_is_a_miss() {
    let td = fixture();
    assert!(matches!(
        locate(td.path(), "org/acme/nowhere"),
        Err(LocateError::NotFound)
    ));
}

/// Traversal, in every spelling that reaches the parser. `main/` is the floor:
/// nothing above it is addressable, whatever the URL says.
#[test]
fn traversal_never_escapes_the_main_subtree() {
    let td = fixture();
    for attack in [
        "../colony.json",
        "..",
        "org/../../colony.json",
        "org/acme/canvy/render/..",
        "/org/acme/canvy/render",
        "",
        ".",
        "org//acme",
        "org/./acme/canvy/render",
        "org/acme/canvy/render/",
    ] {
        let r = locate(td.path(), attack);
        assert!(
            r.is_err(),
            "{attack:?} must not resolve — it did, to {:?}",
            r.map(|l| l.cell_path)
        );
    }
}

/// Two reserved segments. `@` is ours (it opens a verb), `live` is the phoenix
/// client's (it appends "/websocket" to what it is handed). A cell whose name is
/// either cannot be addressed, so it is refused where paths are validated.
#[test]
fn reserved_segments_are_not_addressable() {
    let td = fixture();
    for rel in ["org/@weird/render", "org/live/render", "org/acme/live"] {
        write_cell(
            td.path(),
            rel,
            r#"{ "cell": { "type": "code", "surface": {} },
                 "params": { "runner": "python3", "script_inline": "pass" } }"#,
        );
        assert!(locate(td.path(), rel).is_err(), "{rel} must not resolve");
    }
}

/// A declared surface whose declaration is broken must say so, loudly and by
/// name — the one error class that is NOT flattened into a 404, because it is the
/// operator's own typo and hiding it wastes their afternoon.
#[test]
fn a_broken_declaration_is_reported_not_hidden() {
    let td = fixture();
    write_cell(
        td.path(),
        "org/acme/broken/render",
        r#"{ "cell": { "type": "code", "surface": { "assets": "../etc" } },
             "params": { "runner": "python3", "script_inline": "pass" } }"#,
    );
    match locate(td.path(), "org/acme/broken/render") {
        Err(LocateError::Malformed(m)) => assert!(m.contains("assets"), "{m}"),
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// An unreadable or non-JSON config is a miss, not a panic. A half-written
/// config.json during a mutation must not take the HTTP layer down.
#[test]
fn unreadable_config_is_a_miss() {
    let td = fixture();
    let dir = td.path().join("main/org/acme/torn/render");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.json"), b"{ not json").unwrap();
    assert!(locate(td.path(), "org/acme/torn/render").is_err());
}

/// A directory with no `config.json` at all is a miss. A hive marker's own
/// directory, a `.staging` leftover, an empty folder — none of them are surfaces.
#[test]
fn a_directory_without_a_config_is_a_miss() {
    let td = fixture();
    fs::create_dir_all(td.path().join("main/org/acme/hollow")).unwrap();
    assert!(matches!(
        locate(td.path(), "org/acme/hollow"),
        Err(LocateError::NotFound)
    ));
}
