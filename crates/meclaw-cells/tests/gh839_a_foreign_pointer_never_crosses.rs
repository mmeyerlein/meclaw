//! GH #839: a blob reference inside `messages[]` never crosses a colony
//! boundary, whatever the lane names.
//!
//! `text_id` and `messages_id` are resolved against the RECEIVING colony's
//! blob store at its delivery boundary (`meclaw-colony/src/cell_task.rs`,
//! `resolve_blob_for_delivery`). A peer that could send one would read that
//! store through the pointer: the lane allow-list alone let it through as soon
//! as the declaration named `messages[].text_id`. The refusal is the one
//! `attachments[]` already gets, before the allow-list and regardless of it.
use meclaw_cells::proxy::meclaw::lanes::project_body;
use meclaw_cells::proxy::meclaw::params::Lane;
use meclaw_core::serde_json::json;

/// A lane that names every turn field a pointer turn carries, the two
/// pointer keys included: the declaration is what must NOT unlock them.
fn pointer_lane() -> Lane {
    Lane {
        route: "topic".into(),
        fields: [
            "messages[].text_id",
            "messages[].messages_id",
            "messages[].origin",
            "messages[].type",
            "messages[].text",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        context: vec![],
        because: "a subject this side may consider, never who said it".into(),
    }
}

const POINTER_ID: &str = "0199a3c2-0000-7000-8000-000000000839";

#[test]
fn both_pointer_forms_are_refused_even_when_the_lane_names_them() {
    let l = pointer_lane();
    for turn in [
        json!({"text_id": POINTER_ID}),
        json!({"origin": "user", "type": "text", "text_id": POINTER_ID}),
        json!({"messages_id": POINTER_ID}),
    ] {
        let r = project_body(&l, &json!({"messages": [turn.clone()]}))
            .expect_err("a foreign blob reference must never cross");
        assert_eq!(r.error_code, "lane_body_unsupported", "{turn}");
        assert!(
            r.detail.contains("blob reference") && r.detail.contains("`fields`"),
            "the detail says what and that the declaration does not change it: {}",
            r.detail
        );
        assert!(!r.detail.contains(POINTER_ID), "never the id: {}", r.detail);
    }
}

#[test]
fn a_pointer_in_one_of_several_turns_refuses_the_whole_body() {
    let l = pointer_lane();
    let r = project_body(
        &l,
        &json!({"messages": [
            {"origin": "user", "type": "text", "text": "plain"},
            {"origin": "user", "type": "text", "text_id": POINTER_ID}
        ]}),
    )
    .expect_err("one pointer turn is enough");
    assert_eq!(r.error_code, "lane_body_unsupported");
}

#[test]
fn a_pointer_wins_over_an_undeclared_turn_field_in_an_earlier_turn() {
    // The pointer is judged before the allow-list: its own code, not the
    // allow-list's, even when another turn would fail the allow-list first.
    let l = pointer_lane();
    let r = project_body(
        &l,
        &json!({"messages": [
            {"origin": "user", "type": "text", "text": "x", "who": "a person"},
            {"messages_id": POINTER_ID}
        ]}),
    )
    .expect_err("refused");
    assert_eq!(r.error_code, "lane_body_unsupported", "{}", r.detail);
}

#[test]
fn an_ordinary_turn_still_crosses() {
    let l = pointer_lane();
    let body = json!({"messages": [{"origin": "user", "type": "text", "text": "gardening"}]});
    assert_eq!(
        project_body(&l, &body).expect("a plain inline turn crosses"),
        body
    );
}
