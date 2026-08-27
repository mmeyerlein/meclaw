//! GH #425 — a reply is READ, not merely forwarded
//! (`templates/builder-hive/README.md` § *A reply is read, not merely
//! forwarded*, GH #355/#360).
//!
//! `{"outcome":"committed"}` and `{"outcome":"rejected"}` are the same slot and
//! opposite facts. A cell that stamps both the same way produces a run that says
//! "applied" and applied nothing. Only `committed` is an application.
//!
//! And "rejected" **alone** is a lie of its own, because a manifest has no
//! rollback: two declarations are live, the third was refused, and the rest was
//! never looked at. The sentence a human reads has to carry that.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const SUBMIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

/// Phase B: the colony's answer, which arrives here directly because the
/// substrate stamps `reply_to` on every cell emission — in a FRESH trace, with
/// no correlation and no context (`templates/builder-hive/README.md` § *What a
/// `/colony` round trip forced on the design*).
fn run_submit_phase_b(receipt: Value) -> Value {
    emit_one(
        &shipped_script(SUBMIT),
        &json!({
            "target": "/os/submit",
            "header": {"hop": {}, "context": {}},
            "ttl": 64,
            "manifest": receipt,
            "messages": [],
            "params": {"policy": []},
        }),
    )
}

#[test]
fn a_rejected_manifest_is_reported_as_rejected_and_names_the_position() {
    let out = run_submit_phase_b(json!({
        "outcome": "rejected", "applied": 2, "ids": ["m1", "m2"],
        "failed_at": 3, "remaining": 2,
        "error_code": "scope_containment", "details": "…", "id": "m3"
    }));
    assert_eq!(out["header"]["route"], json!("receipt"));
    assert_eq!(out["header"]["applied"], json!(2));
    assert_eq!(
        out["header"]["failed_at"],
        json!(3),
        "the position is called `failed_at` and is 1-based (Lane 4, build_manifest_reply)"
    );
    assert_eq!(out["header"]["remaining"], json!(2));
    assert_eq!(
        out["header"]["error_code"],
        json!("scope_containment"),
        "the refusing entry's own code, verbatim — no new string is minted here"
    );
    let text = out["messages"][0]["text"].as_str().expect("a turn");
    assert!(
        text.contains("2 applied"),
        "the sentence a human reads must carry the partial state: a manifest has \
         no rollback, so 'rejected' alone is a lie about what is now live — {text}"
    );
}

#[test]
fn a_committed_manifest_carries_no_error_code() {
    let out = run_submit_phase_b(json!({
        "outcome": "committed", "applied": 3, "ids": ["m1", "m2", "m3"]
    }));
    assert_eq!(out["header"]["route"], json!("receipt"));
    assert!(out["header"].get("error_code").is_none());
    assert!(out["header"].get("failed_at").is_none());
    assert_eq!(out["header"]["applied"], json!(3));
    assert!(
        out["messages"][0]["text"]
            .as_str()
            .expect("a turn")
            .contains("applied")
    );
}

#[test]
fn an_unreadable_manifest_body_comes_back_with_the_colonys_own_schema_code() {
    // A body that meant to be a manifest and could not be read carries no
    // position at all — there was none. `error_code` is `schema`, the one a
    // broken body form has always had (ManifestError::error_code, "never a new
    // string"), and the submitter passes it on rather than inventing its own.
    let out = run_submit_phase_b(json!({
        "outcome": "rejected", "applied": 0,
        "error_code": "schema", "details": "`manifest` must be an array of mutation bodies"
    }));
    assert_eq!(out["header"]["error_code"], json!("schema"));
    assert_eq!(out["header"]["applied"], json!(0));
    assert!(out["header"].get("failed_at").is_none());
}

#[test]
fn phase_b_is_recognised_by_what_it_carries_and_not_by_what_it_lacks() {
    // The submission carries the ordered LIST; the answer carries an OBJECT with
    // an `outcome`. Reading phase B as "anything that is not a fresh submission"
    // would read an error reply as a new submission and re-emit it — the loop
    // workshop/cookbook/reply-to-fallback-loops.md is named after, measured at
    // ~20 round trips a second before it was fixed elsewhere in this tree.
    let out = run_submit_phase_b(json!({"applied": 2, "failed_at": 3}));
    assert_eq!(
        out["header"]["error_code"],
        json!("manifest_missing"),
        "an object with no `outcome` is not an answer, and it is not a manifest \
         either — it is refused, not re-emitted"
    );
    assert_ne!(out["header"]["route"], json!("mutate"));
}
