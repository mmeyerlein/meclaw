//! W8 Task 5 (GH #380): server-side rendering and the materialised tree.
//!
//! Two claims are under test here, and they are the two R-W8-4 asks for.
//!
//! **The template syntax is closed.** Four forms exist — `{{prop}}`,
//! `{{&prop}}`, `{{children}}` and `{{#if prop}}…{{/if}}` — and anything else
//! between braces is refused when a component is *defined*, not when it is
//! rendered. A template language that grows by accident is one an LLM will
//! discover accidentally, and a refusal at render time would surface as a blank
//! area on a page rather than as an answer to whoever wrote the component.
//!
//! **Escaping is the default, and the exception is declared.** `{{prop}}`
//! escapes; `{{&prop}}` does not, and is only honoured for a prop the
//! component's own `prop_schema` types as `"html"`. Props are written by
//! models and by browsers, so the raw form has to cost a declaration.

use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::render::{RenderError, materialize, render_object};
use meclaw_core::serde_json::json;
use rusqlite::Connection;

/// A database with the given components and objects, and one page at `/`.
fn db(
    components: &[(&str, &str, &str)],
    objects: &[(&str, Option<&str>, &str, i64, &str)],
) -> Connection {
    let conn = Connection::open_in_memory().expect("open");
    setup_web_schema(&conn).expect("schema");
    for (name, template, prop_schema) in components {
        conn.execute(
            "INSERT INTO components (name, template, prop_schema) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, template, prop_schema],
        )
        .expect("insert component");
    }
    for (id, parent, component, ord, props) in objects {
        conn.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, parent, component, ord, props],
        )
        .expect("insert object");
    }
    conn
}

#[test]
fn a_prop_is_substituted_and_escaped() {
    let conn = db(
        &[("text", "<p>{{body}}</p>", r#"{"body":"text"}"#)],
        &[(
            "a",
            None,
            "text",
            0,
            r#"{"body":"<script>alert(1)</script>"}"#,
        )],
    );
    let html = render_object(&conn, "a").expect("render");
    assert!(
        !html.contains("<script>alert(1)"),
        "escaped by default: {html}"
    );
    assert!(html.contains("&lt;script&gt;"), "{html}");
}

#[test]
fn the_raw_form_needs_the_prop_typed_as_html() {
    // Declared: honoured.
    let conn = db(
        &[("rich", "<div>{{&body}}</div>", r#"{"body":"html"}"#)],
        &[("a", None, "rich", 0, r#"{"body":"<b>bold</b>"}"#)],
    );
    assert!(
        render_object(&conn, "a")
            .expect("render")
            .contains("<b>bold</b>")
    );

    // Not declared: the raw marker does not buy anything, and the value is
    // escaped exactly as `{{body}}` would have escaped it. Silently escaping
    // beats rendering markup a schema never promised was markup.
    let conn = db(
        &[("plain", "<div>{{&body}}</div>", r#"{"body":"text"}"#)],
        &[("a", None, "plain", 0, r#"{"body":"<b>bold</b>"}"#)],
    );
    let html = render_object(&conn, "a").expect("render");
    assert!(
        !html.contains("<b>bold</b>"),
        "must not emit raw markup: {html}"
    );
    assert!(html.contains("&lt;b&gt;"), "{html}");
}

#[test]
fn children_render_in_ord_order() {
    let conn = db(
        &[
            ("stack", "<div>{{children}}</div>", "{}"),
            ("text", "<p>{{body}}</p>", r#"{"body":"text"}"#),
        ],
        &[
            ("root", None, "stack", 0, "{}"),
            // Deliberately inserted out of order: `ord` decides, not rowid.
            ("second", Some("root"), "text", 1, r#"{"body":"two"}"#),
            ("first", Some("root"), "text", 0, r#"{"body":"one"}"#),
        ],
    );
    let html = render_object(&conn, "root").expect("render");
    let one = html.find("one").expect("first child present");
    let two = html.find("two").expect("second child present");
    assert!(one < two, "ord decides the order, got: {html}");
}

#[test]
fn the_conditional_shows_and_hides() {
    let comp = &[(
        "badge",
        "<span>{{#if label}}{{label}}{{/if}}</span>",
        r#"{"label":"text"}"#,
    )];
    let shown = db(comp, &[("a", None, "badge", 0, r#"{"label":"NEW"}"#)]);
    assert!(render_object(&shown, "a").expect("render").contains("NEW"));

    // Absent, empty and false are all "no". A conditional that showed an empty
    // string would render an empty box, which is worse than nothing.
    for props in [r#"{}"#, r#"{"label":""}"#, r#"{"label":false}"#] {
        let conn = db(comp, &[("a", None, "badge", 0, props)]);
        let html = render_object(&conn, "a").expect("render");
        assert_eq!(html, "<span></span>", "props {props} rendered {html}");
    }
}

#[test]
fn an_unknown_component_is_named_rather_than_rendered_blank() {
    let conn = db(&[], &[("a", None, "ghost", 0, "{}")]);
    match render_object(&conn, "a") {
        Err(RenderError::UnknownComponent(name)) => assert_eq!(name, "ghost"),
        other => panic!("expected UnknownComponent, got {other:?}"),
    }
}

#[test]
fn a_cycle_in_the_tree_is_refused_instead_of_hanging() {
    // Nothing stops a patch from making an object its own ancestor, and a
    // renderer that recursed into that would take the cell down rather than
    // report anything.
    let conn = db(
        &[("stack", "<div>{{children}}</div>", "{}")],
        &[
            ("a", Some("b"), "stack", 0, "{}"),
            ("b", Some("a"), "stack", 0, "{}"),
        ],
    );
    assert!(matches!(
        render_object(&conn, "a"),
        Err(RenderError::TooDeep { .. })
    ));
}

/// A page whose root is `<main>{{children}}</main>`, with `n` text children.
fn page_with_children(n: usize) -> Connection {
    let mut objects: Vec<(String, Option<String>, String, i64, String)> = vec![(
        "root".to_string(),
        None,
        "stack".to_string(),
        0,
        "{}".into(),
    )];
    for i in 0..n {
        objects.push((
            format!("c{i}"),
            Some("root".to_string()),
            "text".to_string(),
            i as i64,
            format!(r#"{{"body":"{i}"}}"#),
        ));
    }
    let borrowed: Vec<(&str, Option<&str>, &str, i64, &str)> = objects
        .iter()
        .map(|(id, parent, component, ord, props)| {
            (
                id.as_str(),
                parent.as_deref(),
                component.as_str(),
                *ord,
                props.as_str(),
            )
        })
        .collect();
    let conn = db(
        &[
            ("stack", "<main>{{children}}</main>", "{}"),
            ("text", "<p>{{body}}</p>", r#"{"body":"text"}"#),
        ],
        &borrowed,
    );
    conn.execute(
        "INSERT INTO pages (route, root, title) VALUES ('/', 'root', 'Home')",
        [],
    )
    .expect("page");
    conn
}

#[test]
fn materialize_splits_the_root_template_at_its_children() {
    // The packed tree LiveView wants: statics around one slot per direct child
    // of the page root. That granularity is the diff granularity — a patch to
    // any descendant re-renders its root-child ancestor's slot and nothing else.
    //
    // RETRACTION (GH #394). This test used to assert
    // `m.statics == ["<main>", "</main>"]` for these two children, and the
    // matching two-static `packed_tree`. That was pinning the defect, not the
    // contract: the closing static landed *between* the two children, so the
    // second child rendered outside the element that was meant to contain it,
    // and the wire shape carried n statics for n dynamics where the client
    // expects n+1. The shape asserted below replaces it — for n slots,
    // `statics` is the text before the marker, n−1 empty separators, and the
    // text after it.
    let conn = page_with_children(2);

    let m = materialize(&conn, "/").expect("materialize");
    assert_eq!(
        m.statics,
        vec!["<main>".to_string(), String::new(), "</main>".to_string()],
        "n+1 statics for n slots: the separator between two children is empty"
    );
    assert_eq!(m.slots.len(), 2, "one slot per direct child of the root");
    assert_eq!(m.slots[0].0, "c0");
    assert!(m.slots[0].1.contains(">0<"));
    assert_eq!(m.slots[1].0, "c1");

    // The wire shape: {"s": statics, "0": …, "1": …}
    assert_eq!(
        m.packed_tree(),
        json!({"s": ["<main>", "", "</main>"], "0": "<p>0</p>", "1": "<p>1</p>"})
    );
    assert_eq!(
        m.rendered_body(),
        "<main><p>0</p><p>1</p></main>",
        "both children sit inside the root element"
    );
    assert_eq!(m.title, "Home");
}

#[test]
fn a_root_with_three_children_keeps_all_three_inside_it() {
    // The case nobody had: `rendered_body` iterates over `statics`, so with the
    // old two-static shape it ran twice however many slots existed and dropped
    // every child from the third on — silently, in the served HTML. This is the
    // acceptance case of GH #394.
    let conn = page_with_children(3);
    let m = materialize(&conn, "/").expect("materialize");

    assert_eq!(m.slots.len(), 3);
    assert_eq!(
        m.statics.len(),
        m.slots.len() + 1,
        "n+1 statics for n slots, at any child count: {:?}",
        m.statics
    );
    assert_eq!(
        m.rendered_body(),
        "<main><p>0</p><p>1</p><p>2</p></main>",
        "all three children render inside the root element, in `ord` order"
    );
    assert_eq!(
        m.packed_tree(),
        json!({
            "s": ["<main>", "", "", "</main>"],
            "0": "<p>0</p>", "1": "<p>1</p>", "2": "<p>2</p>"
        })
    );
}

#[test]
fn a_root_with_one_child_is_the_shape_it_always_was() {
    // The case that worked before the fix, and has to keep working byte for
    // byte: one slot, two statics, the served body wrapping the one child.
    let conn = page_with_children(1);
    let m = materialize(&conn, "/").expect("materialize");

    assert_eq!(m.statics, vec!["<main>".to_string(), "</main>".to_string()]);
    assert_eq!(m.slots.len(), 1);
    assert_eq!(m.rendered_body(), "<main><p>0</p></main>");
    assert_eq!(
        m.packed_tree(),
        json!({"s": ["<main>", "</main>"], "0": "<p>0</p>"})
    );
}

#[test]
fn a_root_whose_children_marker_has_nothing_to_show_is_one_static() {
    // A root that declares `{{children}}` and has no children is a page with
    // zero dynamics, so the packed tree is one static and nothing else. The
    // served body is unchanged — `<main></main>` either way — but a tree with
    // two statics and no dynamic is not a shape the client has a reading for.
    let conn = page_with_children(0);
    let m = materialize(&conn, "/").expect("materialize");

    assert!(m.slots.is_empty());
    assert_eq!(m.statics, vec!["<main></main>".to_string()]);
    assert_eq!(m.rendered_body(), "<main></main>");
    assert_eq!(m.packed_tree(), json!({"s": ["<main></main>"]}));
}

#[test]
fn a_slot_index_addresses_the_same_child_in_the_diff_and_in_the_tree() {
    // The diff-push path sends `{ "<i>": <slot html> }` with `i` from
    // `slot_of`. That only patches the right thing if `i` is the *dynamic*
    // index — the child's position — and not an index into `statics`. Growing
    // `statics` to n+1 must therefore leave this addressing alone, which is the
    // one place the GH #394 fix could have broken something.
    let conn = page_with_children(3);
    let m = materialize(&conn, "/").expect("materialize");
    let tree = m.packed_tree();

    for (i, (id, html)) in m.slots.iter().enumerate() {
        assert_eq!(
            m.slot_of(id),
            Some(i),
            "slot_of({id}) must name the child's own position"
        );
        assert_eq!(
            tree[i.to_string()],
            json!(html),
            "the diff key {i} and the tree's dynamic {i} are the same child"
        );
    }
    assert_eq!(m.slot_of("not-on-this-page"), None);
}

#[test]
fn a_route_no_page_declares_is_a_miss_not_an_error_page() {
    let conn = db(&[], &[]);
    assert!(matches!(
        materialize(&conn, "/nope"),
        Err(RenderError::UnknownRoute(_))
    ));
}

#[test]
fn a_root_with_no_children_still_materialises() {
    // A page that is one component with no `{{children}}` at all: everything is
    // static, no slots. It must render, or a legitimate one-component page
    // would 500.
    let conn = db(
        &[("hero", "<h1>{{title}}</h1>", r#"{"title":"text"}"#)],
        &[("root", None, "hero", 0, r#"{"title":"Hi"}"#)],
    );
    conn.execute("INSERT INTO pages (route, root) VALUES ('/', 'root')", [])
        .expect("page");
    let m = materialize(&conn, "/").expect("materialize");
    assert!(m.slots.is_empty());
    assert_eq!(m.statics, vec!["<h1>Hi</h1>".to_string()]);
    assert_eq!(m.packed_tree(), json!({"s": ["<h1>Hi</h1>"]}));
}
