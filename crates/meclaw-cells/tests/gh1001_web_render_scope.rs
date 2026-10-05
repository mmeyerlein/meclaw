//! GH #1001 T1 — a write renders the slots it touched and nothing else.
//!
//! Before: `publish_and_push` re-materialised every route after every write,
//! so a figure that moved on the lab page rendered all 4 871 objects (deployed page:
//! ≈ 51 ms on 1 000 figures). [`rerender`] reports how many objects it
//! rendered; the count is the objects of the touched slots.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_cells::web::ops;
use meclaw_cells::web::render::{materialize_all, rerender};
use meclaw_core::serde_json::json;
use web_fixture::{Shape, kid, memory_db, update};

#[test]
fn gh1001_a_write_renders_only_the_slots_it_touched() {
    let shape = Shape::default();
    let conn = memory_db(shape);
    let pages = materialize_all(&conn).expect("pages");

    // A figure is a root child of its own: one object.
    let (outcome, touched) = ops::apply(&conn, &update("fig-17", json!({"x": 5})));
    assert!(!outcome.is_error());
    let done = rerender(&conn, &pages, &touched).expect("rerender");
    assert_eq!(done.objects, 1, "a moved figure renders the figure");

    // A child of a chunk: the chunk's slot, the chunk and its 128 children.
    let (_, touched) = ops::apply(&conn, &update(&kid(3, 7), json!({"x": 999})));
    let again = rerender(&conn, &done.pages, &touched).expect("rerender");
    assert_eq!(
        again.objects,
        1 + shape.kids,
        "a child update renders its root-child slot and nothing beside it"
    );
    assert_eq!(
        again.pages["/"].rendered_body(),
        materialize_all(&conn).expect("pages")["/"].rendered_body(),
        "and the page it publishes is the page a whole render draws"
    );

    // A create inside a chunk is structural for the slot, not for the page.
    let (_, touched) = ops::apply(
        &conn,
        &json!({"op": "object.create", "id": "x-1", "parent": "chunk-2",
                "component": "tree", "ord": 3, "props": {"x": 1, "y": 2}}),
    );
    assert!(touched.structural);
    let third = rerender(&conn, &again.pages, &touched).expect("rerender");
    assert_eq!(third.objects, 1 + shape.kids + 1);
    assert_eq!(
        third.pages["/"].rendered_body(),
        materialize_all(&conn).expect("pages")["/"].rendered_body()
    );
    let (_, frame) = &third.frames[0];
    assert!(
        frame.get("s").is_none(),
        "the root's child list held, so the frame is the chunk's children part, \
         not the packed tree: {}",
        &frame.to_string()[..frame.to_string().len().min(200)]
    );
}

/// A route the viewers' pages do not have — it fell out after a render error —
/// is sent whole: an update names its slots, but the viewer holds no page for
/// them to land in, so the frame is the packed tree (`"s"` at the top), never
/// a lone slot whose neighbours are stale.
#[test]
fn gh1001_a_route_missing_from_the_held_pages_is_sent_whole() {
    let conn = memory_db(Shape {
        figures: 8,
        chunks: 2,
        kids: 4,
    });
    let mut held = materialize_all(&conn).expect("pages");
    held.remove("/");
    let (outcome, touched) = ops::apply(&conn, &update("fig-3", json!({"x": 5})));
    assert!(!outcome.is_error() && !touched.structural);
    let done = rerender(&conn, &held, &touched).expect("rerender");
    let (route, frame) = &done.frames[0];
    assert_eq!(route, "/");
    assert!(
        frame.get("s").is_some(),
        "the route is sent as its packed tree: {frame}"
    );
    assert_eq!(
        done.pages["/"].rendered_body(),
        materialize_all(&conn).expect("pages")["/"].rendered_body()
    );
}
