//! What the lane names crosses; everything else is refused, never stripped.
use meclaw_cells::proxy::meclaw::lanes::{Direction, find_lane, project_body, project_context};
use meclaw_cells::proxy::meclaw::params::{Lane, Lanes};
use meclaw_core::serde_json::{Map, Value, json};
fn lane(fields: &[&str]) -> Lane {
    Lane {
        route: "topic".into(),
        fields: fields.iter().map(|s| (*s).to_string()).collect(),
        context: vec!["locale".into()],
        because: "a subject this side may consider, never who said it".into(),
    }
}
#[test]
fn a_named_slot_crosses_and_an_unnamed_one_is_refused_never_stripped() {
    let l = lane(&["topic"]);
    assert_eq!(
        project_body(&l, &json!({"topic": "gardening"})).expect("a named slot crosses"),
        json!({"topic": "gardening"})
    );
    let r = project_body(&l, &json!({"topic": "gardening", "who": "a person"}))
        .expect_err("a field the lane does not name must be refused");
    assert_eq!(r.error_code, "lane_field_denied");
    assert!(
        r.detail.contains("who"),
        "the refusal names the field: {}",
        r.detail
    );
}
#[test]
fn turn_fields_are_named_one_by_one() {
    let l = lane(&["messages[].type", "messages[].text"]);
    let r = project_body(
        &l,
        &json!({"messages": [{"origin": "assistant", "type": "text",
        "text": "hi"}]}),
    )
    .expect_err("origin is not named");
    assert_eq!(r.error_code, "lane_field_denied");
    assert!(
        r.detail.contains("messages[].origin"),
        "detail: {}",
        r.detail
    );
    assert_eq!(
        project_body(&l, &json!({"messages": [{"type": "text", "text": "hi"}]}))
            .expect("the two named turn fields cross"),
        json!({"messages": [{"type": "text", "text": "hi"}]})
    );
}
#[test]
fn an_attachment_is_refused_by_its_own_code_even_when_the_lane_names_it() {
    let l = lane(&["topic", "attachments"]);
    let r = project_body(
        &l,
        &json!({"topic": "x", "attachments": [{"blob_id": "b"}]}),
    )
    .expect_err("blob stores are separate and stay separate");
    assert_eq!(r.error_code, "lane_body_unsupported");
    assert_eq!(
        project_body(&l, &json!("not an object"))
            .expect_err("a non-object body")
            .error_code,
        "lane_body_unsupported"
    );
}
#[test]
fn context_is_projected_silently_and_a_lane_is_found_by_direction() {
    let l = lane(&["topic"]);
    let mut c = Map::new();
    c.insert("locale".into(), json!("de"));
    c.insert("user_id".into(), json!("who"));
    let out = project_context(&l, &c);
    assert_eq!(
        out.len(),
        1,
        "only what the lane names, and no word about the rest"
    );
    assert_eq!(out.get("locale"), Some(&Value::String("de".into())));
    let lanes = Lanes {
        accepts: vec![l],
        emits: vec![],
    };
    assert!(find_lane(&lanes, Direction::Accepts, "topic").is_some());
    assert!(
        find_lane(&lanes, Direction::Emits, "topic").is_none(),
        "direction is part of it"
    );
}
