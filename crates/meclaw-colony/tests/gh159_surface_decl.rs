//! GH #159 — the surface declaration is opt-in, closed, and about serving only.
//!
//! Every test here has a control: the absent case must keep meaning "no
//! surface", because that is what every cell in every shipped topology says
//! today and none of them may change meaning.

use meclaw_colony::surface::parse_decl;
use serde_json::json;

/// The control. A cell without the key serves nothing, and saying so is a
/// success, not an error — otherwise every existing cell fails to boot.
#[test]
fn absent_declaration_is_no_surface() {
    let cell = json!({ "type": "code" });
    assert!(parse_decl(&cell).expect("absent is legal").is_none());
}

#[test]
fn full_declaration_parses() {
    let cell = json!({
        "type": "code",
        "surface": {
            "title": "Colony topology",
            "assets": "client",
            "boot_hint": "reading the colony"
        }
    });
    let d = parse_decl(&cell).expect("valid").expect("declared");
    assert_eq!(d.title, "Colony topology");
    assert_eq!(d.assets.as_deref(), Some("client"));
    assert_eq!(d.boot_hint, "reading the colony");
}

/// A surface with no files of its own is legal: the binary's bundles are enough
/// for a page that only needs LiveView.
#[test]
fn a_declaration_may_be_empty() {
    let cell = json!({ "type": "code", "surface": {} });
    let d = parse_decl(&cell).expect("valid").expect("declared");
    assert_eq!(d.title, "");
    assert_eq!(d.assets, None);
    assert_eq!(d.boot_hint, "");
}

/// The key set is closed. A misspelled key is a typo the operator must see now,
/// not a silently ignored line they find next month.
#[test]
fn an_unknown_key_is_refused_by_name() {
    let cell = json!({ "type": "code", "surface": { "titel": "oops" } });
    let err = parse_decl(&cell).expect_err("closed key set");
    assert!(err.contains("titel"), "the error must name the key: {err}");
}

/// `assets` becomes a filesystem path under the cell's own directory. Anything
/// that could leave that directory is refused where it is written down, not
/// where it is joined.
#[test]
fn an_assets_path_must_be_one_plain_segment() {
    for bad in ["", "..", ".", "a/b", "/abs", "@client", "a\\b", "cli\0ent"] {
        let cell = json!({ "type": "code", "surface": { "assets": bad } });
        assert!(parse_decl(&cell).is_err(), "assets {bad:?} must be refused");
    }
}

#[test]
fn a_non_object_surface_is_refused() {
    for bad in [json!("yes please"), json!(7), json!([]), json!(true)] {
        let cell = json!({ "type": "code", "surface": bad });
        assert!(
            parse_decl(&cell).is_err(),
            "surface {bad:?} must be refused"
        );
    }
}

/// A title is a string. A number that happens to serialise is not a title.
#[test]
fn wrong_types_are_refused() {
    for bad in [
        json!({ "title": 7 }),
        json!({ "assets": 7 }),
        json!({ "boot_hint": [] }),
    ] {
        let cell = json!({ "type": "code", "surface": bad });
        assert!(
            parse_decl(&cell).is_err(),
            "surface {bad:?} must be refused"
        );
    }
}

/// A declaration must round trip through serde unchanged: the colony serialises
/// `config.json` at instantiation, and a field that does not survive that trip
/// would be a surface that stops existing the first time a cell is instantiated.
#[test]
fn a_declaration_round_trips() {
    let cell = json!({
        "type": "code",
        "surface": { "title": "T", "assets": "client", "boot_hint": "H" }
    });
    let d = parse_decl(&cell).unwrap().unwrap();
    let back: meclaw_colony::surface::SurfaceDecl =
        serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
    assert_eq!(d, back);
}
