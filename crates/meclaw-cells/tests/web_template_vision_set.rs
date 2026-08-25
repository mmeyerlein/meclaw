//! W8 Task 12 (GH #382): the Vision component set, and the two material rules.
//!
//! Task 11 shipped the token sheet — the material as CSS. This suite is about
//! the other half: the nine components that spend those tokens, the demo page
//! composed only of them, and the two adoption rules that the design language
//! states as prose and this cell states as **refusals**.
//!
//! The rules are the reason there is Rust in a seed-data task at all:
//!
//! 1. **Glass lives on the navigation layer.** A component that declares
//!    `layer: "content"` and writes one of the three closed glass class names
//!    is refused at `component.define` — and, because the seed path never goes
//!    through `component.define`, at seed-check time too. Both go through the
//!    same function; a rule enforced in one of two places is a rule that ships
//!    broken through the other.
//! 2. **Glass never sits on glass.** Two navigation-glass components in a
//!    parent/child edge are refused at `object.create`, where the edge is made.
//!
//! What is asked of the shipped template is asked through the substrate's own
//! readers — `check_seed_files` (what `--validate` runs) and the pair
//! `load_seed_if_present` / `materialize_all` (what a first spawn runs) — never
//! through a second opinion written in this file.

use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::ops::apply;
use meclaw_cells::web::render::materialize_all;
use meclaw_cells::web::seed::{check_seed_files, load_seed_if_present};
use meclaw_core::serde_json::{Value, json};
use rusqlite::Connection;
use std::collections::BTreeSet;

/// The shipped template directory.
fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/web")
}

/// A fresh database with the shipped seed loaded, exactly as a first spawn
/// (`OpenStatus::Created`) does it.
fn seeded_db() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    setup_web_schema(&conn).expect("the web schema applies");
    load_seed_if_present(&conn, &template_dir()).expect("the shipped seed loads");
    conn
}

/// The set, and the layer each member declares.
///
/// The two navigation members are the two that ARE glass: a card is a pane and
/// an ornament is a floating dock. Everything else is content that sits on
/// them.
const VISION_SET: &[(&str, &str)] = &[
    ("stack", "content"),
    ("card", "navigation"),
    ("heading", "content"),
    ("text", "content"),
    ("table", "content"),
    ("button", "content"),
    ("input", "content"),
    ("badge", "content"),
    ("ornament", "navigation"),
];

/// A minimal cell directory holding one components seed file.
fn dir_with_components(rows: &str) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(td.path().join("seed")).expect("seed dir");
    std::fs::write(
        td.path().join("seed").join("components.jsonl"),
        format!(
            "{}\n{rows}\n",
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#
        ),
    )
    .expect("components.jsonl");
    td
}

/// A database with just enough of the set to make an edge.
fn db_with(components: &[(&str, &str, &str)]) -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    setup_web_schema(&conn).expect("schema");
    for (name, template, layer) in components {
        conn.execute(
            "INSERT INTO components (name, template, prop_schema, editable, layer)
             VALUES (?1, ?2, '{}', '[]', ?3)",
            rusqlite::params![name, template, layer],
        )
        .expect("component row");
    }
    conn
}

#[test]
fn the_template_ships_the_nine_vision_components_with_their_layers() {
    let conn = seeded_db();
    let mut stmt = conn
        .prepare("SELECT name, layer FROM components ORDER BY name")
        .expect("components");
    let shipped: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();

    let mut expected: Vec<(String, String)> = VISION_SET
        .iter()
        .map(|(n, l)| ((*n).to_string(), (*l).to_string()))
        .collect();
    expected.sort();

    assert_eq!(
        shipped, expected,
        "the shipped component library is the Vision set, and the layer each \
         member declares is what the two material rules are enforced ABOUT"
    );
}

#[test]
fn the_demo_page_is_composed_only_of_the_shipped_component_set() {
    let conn = seeded_db();
    let root: String = conn
        .query_row("SELECT root FROM pages WHERE route = '/demo'", [], |r| {
            r.get(0)
        })
        .expect("the template ships a page at /demo — Task 18's visual smoke target");

    // Walk the tree the page actually shows, rather than the whole table: a
    // demo that reached for something it does not ship would be invisible here
    // if the objects were merely counted.
    let mut used = BTreeSet::new();
    let mut frontier = vec![root];
    let mut seen = 0usize;
    while let Some(id) = frontier.pop() {
        seen += 1;
        assert!(seen < 500, "the demo tree does not terminate");
        let component: String = conn
            .query_row("SELECT component FROM objects WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap_or_else(|e| panic!("demo object {id:?} is missing: {e}"));
        used.insert(component);
        let mut stmt = conn
            .prepare("SELECT id FROM objects WHERE parent = ?1")
            .expect("children");
        frontier.extend(
            stmt.query_map([&id], |r| r.get::<_, String>(0))
                .expect("children query")
                .map(Result::unwrap),
        );
    }

    let expected: BTreeSet<String> = VISION_SET.iter().map(|(n, _)| (*n).to_string()).collect();
    assert_eq!(
        used, expected,
        "the demo page is the set's own proof: every member appears on it, and \
         nothing that is not a member does"
    );
}

#[test]
fn the_demo_page_renders_every_component_in_the_set() {
    let conn = seeded_db();
    let pages = materialize_all(&conn).expect("the shipped pages render");
    let demo = pages.get("/demo").expect("a page at /demo");
    let body = demo.rendered_body();

    // One marker per component: the markup or the class that component IS.
    // Asking for the component's *name* would prove nothing — a name never
    // reaches the page.
    for (component, marker) in [
        ("stack", "class=\"stack\""),
        ("card", "glass--thin"),
        ("heading", "title-2"),
        ("text", "class=\"text"),
        ("table", "<table"),
        ("button", "<button"),
        ("input", "<input"),
        ("badge", "badge"),
        ("ornament", "ornament"),
    ] {
        assert!(
            body.contains(marker),
            "`{component}` renders nothing on the demo page — no {marker:?} in:\n{body}"
        );
    }

    assert!(
        body.contains("<link rel=\"stylesheet\" href=\"/vision.css\">"),
        "the demo page must name the token sheet, or it is the set with no \
         design on it: {body}"
    );
}

#[test]
fn the_demo_page_root_holds_one_slot_so_its_dead_render_is_the_whole_page() {
    // RETRACTED, GH #394: this comment used to say that `Materialized::statics`
    // is always two pieces and that a second root child would therefore put the
    // closing static in the middle of the page. That WAS true when the demo
    // page was composed, and it was a defect, not a contract — `statics` now
    // carries one entry more than there are slots, for any child count, and a
    // root with three children renders all three.
    //
    // The demo page stays one-slot anyway, and the assertion below stays with
    // it: the page root carries the stylesheet link and its single child holds
    // the content, which is a composition CHOICE now rather than a constraint.
    // What the test still buys is the identity below — for a one-slot page the
    // served body is exactly the statics around that slot — which is the shape
    // the LiveView client attaches to instead of replacing on connect.
    let conn = seeded_db();
    let pages = materialize_all(&conn).expect("pages render");
    for route in ["/", "/demo"] {
        let page = pages
            .get(route)
            .unwrap_or_else(|| panic!("a page at {route}"));
        assert_eq!(
            page.slots.len(),
            1,
            "{route} has {} root children; the dead render carries at most one",
            page.slots.len()
        );
        assert_eq!(
            page.rendered_body(),
            format!("{}{}{}", page.statics[0], page.slots[0].1, page.statics[1]),
            "{route}: the served body is statics around the one slot"
        );
    }
}

#[test]
fn a_content_layer_component_may_not_wear_the_glass_material() {
    let conn = db_with(&[]);
    for class in ["glass", "glass--thin", "glass--thick"] {
        let (outcome, touched) = apply(
            &conn,
            &json!({
                "op": "component.define",
                "name": "panel",
                "template": format!("<div class=\"{class} panel\">{{{{children}}}}</div>"),
                "layer": "content",
            }),
        );
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("invalid_input"),
            "a content-layer component wearing {class:?} must be refused"
        );
        let text = outcome.error_text.unwrap_or_default();
        assert!(
            text.contains(class) && text.contains("navigation"),
            "the refusal names the class and where glass lives: {text}"
        );
        assert!(touched.slots.is_empty(), "a refusal re-renders nothing");
    }

    // The same template on the navigation layer is exactly what a card is.
    let (ok, _) = apply(
        &conn,
        &json!({
            "op": "component.define",
            "name": "panel",
            "template": "<div class=\"glass--thin panel\">{{children}}</div>",
            "layer": "navigation",
        }),
    );
    assert!(
        !ok.is_error(),
        "glass on the navigation layer is the design, not a violation: {:?}",
        ok.error_text
    );

    // And a class that merely starts with the token is not the token: the
    // check compares class names, not substrings.
    let (fine, _) = apply(
        &conn,
        &json!({
            "op": "component.define",
            "name": "glassware",
            "template": "<div class=\"glassy\">{{children}}</div>",
            "layer": "content",
        }),
    );
    assert!(
        !fine.is_error(),
        "`glassy` is not one of the three glass classes: {:?}",
        fine.error_text
    );
}

#[test]
fn a_seeded_content_layer_component_wearing_glass_fails_the_static_seed_check() {
    // The seed path never goes through `component.define`, so the rule has to
    // be reachable from both — this is the half that a shipped template would
    // otherwise walk straight past.
    let td = dir_with_components(
        r#"{"name":"panel","template":"<div class=\"glass\">{{children}}</div>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
    );
    let err = check_seed_files(td.path())
        .expect_err("a seeded content-layer glass component must fail --validate");
    assert!(
        err.contains("glass") && err.contains("panel"),
        "the refusal names the component and the class: {err}"
    );

    // The same row on the navigation layer loads.
    let ok = dir_with_components(
        r#"{"name":"panel","template":"<div class=\"glass\">{{children}}</div>","prop_schema":"{}","editable":"[]","layer":"navigation"}"#,
    );
    check_seed_files(ok.path()).expect("navigation-layer glass is the design");
}

#[test]
fn glass_on_glass_is_refused_at_object_create() {
    let conn = db_with(&[
        (
            "ornament",
            "<nav class=\"glass ornament\">{{children}}</nav>",
            "navigation",
        ),
        (
            "card",
            "<section class=\"glass--thin card\">{{children}}</section>",
            "navigation",
        ),
        (
            "stack",
            "<div class=\"stack\">{{children}}</div>",
            "content",
        ),
    ]);

    let (dock, _) = apply(
        &conn,
        &json!({"op": "object.create", "id": "dock", "component": "ornament"}),
    );
    assert!(!dock.is_error(), "{:?}", dock.error_text);

    let (refused, touched) = apply(
        &conn,
        &json!({"op": "object.create", "id": "pane", "component": "card", "parent": "dock"}),
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("invalid_input"),
        "a card inside an ornament is glass on glass"
    );
    let text = refused.error_text.unwrap_or_default();
    assert!(
        text.contains("card") && text.contains("ornament"),
        "the refusal names both panes: {text}"
    );
    assert!(touched.slots.is_empty(), "a refusal re-renders nothing");
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM objects WHERE id = 'pane'", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(rows, 0, "a refused create writes nothing");

    // Content between them is the way through — and the way the demo page is
    // built.
    let (spacer, _) = apply(
        &conn,
        &json!({"op": "object.create", "id": "row", "component": "stack", "parent": "dock"}),
    );
    assert!(!spacer.is_error(), "{:?}", spacer.error_text);
    let (nested, _) = apply(
        &conn,
        &json!({"op": "object.create", "id": "pane", "component": "card", "parent": "row"}),
    );
    assert!(
        !nested.is_error(),
        "the rule is about the edge it can answer for: {:?}",
        nested.error_text
    );
}

#[test]
fn the_shipped_demo_objects_declare_only_props_their_components_know() {
    // `object.create` checks props against `prop_schema`; a seeded row does
    // not. A prop nobody declared renders as silence, which on a demo page is
    // a missing label nobody would trace back to the seed.
    let conn = seeded_db();
    let mut stmt = conn
        .prepare(
            "SELECT o.id, o.component, o.props, c.prop_schema
               FROM objects o JOIN components c ON c.name = o.component",
        )
        .expect("objects");
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty(), "the template seeds no objects at all");

    for (id, component, props, schema) in rows {
        let props: Value = meclaw_core::serde_json::from_str(&props).expect("props is JSON");
        let schema: Value = meclaw_core::serde_json::from_str(&schema).expect("schema is JSON");
        for key in props.as_object().expect("props is an object").keys() {
            assert!(
                schema.get(key).is_some(),
                "object {id:?} sets {key:?}, which `{component}` does not declare"
            );
        }
    }
}
