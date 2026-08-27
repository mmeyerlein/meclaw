//! GH #440 — the builder may propose a template, not just a topology.
//!
//! Both doors into the colony are op-agnostic: `read_manifest_source`
//! (`--apply`) and `ManifestBody::detect` ask exactly one question, "is there a
//! `manifest` key", and neither carries an allowlist of diff operations. The
//! BUILDER does carry one — `KINDS` in `templates/builder/normalise/config.json`
//! — because a diff key that does not exist would otherwise be discovered at
//! position k of an application that has no rollback.
//!
//! That list is a copy of the shipped operation table, so a seventh operation
//! makes it stale: the builder would refuse `declaration_malformed` for a
//! declaration the colony accepts, and the gap #440 opens would stay shut on
//! the one path meant to walk through it.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);

fn run_normalise(answer: &str) -> Value {
    emit_one(
        &shipped_script(NORMALISE),
        &json!({
            "target": "/os/builder/normalise",
            "header": {"hop": {"finish_reason": "stop"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "text", "id": "", "text": answer}],
        }),
    )
}

#[test]
fn a_composed_manifest_may_register_a_template() {
    let answer = r#"{"declarations":[{"scope":"/","ctx":{},"diff":{"add_templates":[
        {"name":"note-unit","files":{"template.json":"{}"}}]}}]}"#;
    let out = run_normalise(answer);
    assert!(
        out["manifest"].is_array(),
        "the builder refused a declaration the colony accepts — KINDS is stale: {out:#}",
    );
    assert_ne!(
        out["header"]["error_code"],
        json!("declaration_malformed"),
        "{out:#}"
    );
}

/// The counter-case: the allowlist still refuses an operation that does not
/// exist. Without it, widening `KINDS` could have been "accept everything".
#[test]
fn an_invented_operation_is_still_refused() {
    let answer = r#"{"declarations":[{"scope":"/","ctx":{},"diff":{"add_templatez":[]}}]}"#;
    let out = run_normalise(answer);
    assert_eq!(
        out["header"]["error_code"],
        json!("declaration_malformed"),
        "{out:#}"
    );
    assert!(
        out.get("manifest").is_none(),
        "an empty manifest is a failure wearing the face of an honest answer (GH #308)",
    );
}
