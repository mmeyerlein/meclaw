//! GH #963 -- a declared source is read after a sure verdict, and its answer counts only in
//! the round the builder's road stamped on it.
//!
//! The presenter's own topic `notes` declares `source: {read, hop, body, rows}` (Q.4, the
//! seam OR-DP.M.1). A sure verdict opens the window and, in the same output, sends ONE
//! `resident_read`: `hop.resident`, `hop.op_id`, the source's hop keys, and its body with
//! `$request` replaced by the turn's text. The test plays the resident's road (strand M
//! draws it): the answer comes back as `resident_answer` with `hop.resident_round`.
//! - An answer WITHOUT `resident_round` places nothing, whatever round its body claims
//!   (fail-closed). Proven by a sentinel: the read of the next turn leaves only after
//!   `stage` handled that answer.
//! - An answer with it places the rows; a row of another round never reaches the screen,
//!   an unmarked row inherits the stamped round.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::mock_http::MockResponse;
use presenter_colony::{Dials, PRESENTER, body, boot, decision, guard, to_path, warm};

fn notes_topic() -> Value {
    json!([{
        "topic": "notes", "title": "Notes", "describe": "the notes a resident keeps",
        "glyph": "N", "standard": "list",
        "candidates": [
            {"key": "list", "block": "display-list", "describe": "the notes as a list",
             "set": "notes",
             "source": {"read": "librarian", "hop": {"op": "find"},
                        "body": {"op": "find", "args": {"q": "$request"}}, "rows": "items"},
             "bind": {"title": "=Notes"},
             "children": [{"each": "notes", "block": "display-item", "bind": {"k": "$.name"}}]}
        ]
    }])
}

fn sure() -> MockResponse {
    decision(&[
        ("topic", "notes", 0.9),
        ("notes.lead", "list", 0.9),
        ("notes.also", "none", 0.9),
    ])
}

fn answer(op_id: &Value, round: Option<&str>) -> meclaw_core::Message {
    let mut hop = json!({"route": "resident_answer", "op_id": op_id, "resident": "librarian",
                         "resident_status": "answer"});
    if let Some(r) = round {
        hop["resident_round"] = json!(r);
    }
    to_path(
        PRESENTER,
        hop,
        json!({}),
        json!({"messages": [], "audience_set": ["*"], "items": [
            {"name": "mine", "audience_set": ["member:alex"]},
            {"name": "theirs", "audience_set": ["member:sam"]},
            {"name": "plain"}]}),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_source_is_read_with_the_round() {
    if !guard("a_declared_source_is_read_with_the_round") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        builtin_topics: notes_topic(),
        ..Dials::default()
    })
    .await;
    warm(&s).await;

    s.turn_in(
        "n1",
        "notes about actors",
        json!(["agent:g1", "member:alex"]),
    )
    .await;
    s.ask().await.release(sure());
    let read = s.out("resident_read").await;
    let hop = &read.headers.hop;
    assert_eq!(hop.get("resident"), Some(&json!("librarian")));
    assert_eq!(hop.get("op"), Some(&json!("find")));
    assert_eq!(hop.get("op_id"), Some(&json!("n1/notes")));
    assert_eq!(
        body(&read)["args"],
        json!({"q": "notes about actors"}),
        "`$request` is the turn's text"
    );
    s.wait_drawn("show-notes-hint").await;

    // Without `resident_round`: nothing placed, though the body claims `*`.
    s.c.h.send(answer(&json!("n1/notes"), None)).await;
    s.turn_in("n2", "my notes again", json!(["agent:g1", "member:alex"]))
        .await;
    s.ask().await.release(sure());
    let read2 = s.out("resident_read").await;
    assert_eq!(read2.headers.hop.get("op_id"), Some(&json!("n2/notes")));
    assert!(
        !s.drawn("show-notes-list").await,
        "an answer without its round was placed"
    );

    // With it: the rows of the round, never the foreign one.
    s.c.h
        .send(answer(
            &json!("n2/notes"),
            Some(r#"["agent:g1","member:alex"]"#),
        ))
        .await;
    s.wait_drawn("show-notes-list").await;
    assert_eq!(s.ids("show-notes-list-").await.len(), 2);
    assert!(
        !s.c.tree().await.to_string().contains("theirs"),
        "a row of another round reached the screen"
    );
    s.c.shutdown().await;
}
