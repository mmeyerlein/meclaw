//! GH #498 — a reasoning model spent the composer's whole completion budget on
//! reasoning and answered with empty content.
//!
//! `compose` sent no `reasoning` block, so a hosted reasoning model was called
//! at whatever its provider's default is. Measured against a Sonnet-class model
//! over a hosted endpoint, on the writing round of a design build: three calls,
//! the same 34 000-token thread, the same system prompt, the same
//! `temperature: 0` —
//!
//! ```text
//! as shipped                     stop     967 completion,   965 reasoning, content ""
//! as shipped + a closing turn    length  4000 completion,  4000 reasoning, content ""
//! reasoning disabled             stop     744 completion,     0 reasoning, the manifest
//! ```
//!
//! and with the shipped `max_tokens` the same call spent the ENTIRE budget on
//! reasoning: 32 767 reasoning tokens, `finish_reason: length`, content empty,
//! 0.39 USD. `normalise` reads the last turn, finds nothing and answers
//! `no_manifest_in_answer` — paid for, and nothing delivered, which is the one
//! ending the round budget exists against (GH #485).
//!
//! This is the composer's setting rather than the model's fault: an unsent field
//! hands the decision to the provider. GH #498 answered with `reasoning` OFF.
//! GH #1300 (owner ruling: every shipped llm cell, every Anthropic model, effort
//! medium; the composer runs on Sonnet 5.5 native) replaces that with a declared
//! `reasoning_effort: "medium"`. The empty answer above came from the old small
//! cap; since GH #1097 the ceiling is the catalogue's `max_output`, which a
//! medium deliberation does not exhaust. What stays from GH #498 is that the
//! composer NAMES its effort instead of leaving it to the provider.
//!
//! A drift lock in the sense of `docs/development-rules.md` § 2d: it asserts the
//! setting AND that the cell's description still says why, because a param
//! nobody can explain is a param the next reader deletes.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

fn compose() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/compose/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).expect("compose config"))
        .expect("parses")
}

/// GH #1300 (owner ruling: every shipped llm cell at effort medium): the
/// composer still names what it wants of a reasoning model rather than leaving
/// it to the provider's default -- now `reasoning_effort: "medium"`, under a
/// ceiling that is the catalogue's `max_output` since GH #1097, so a medium
/// deliberation no longer exhausts the budget the measurement above ran out of.
/// The OFF object is gone: it outranked the declared effort, and on the
/// Anthropic wire it was no form the Messages API reads.
#[test]
fn the_composer_declares_what_it_wants_of_a_reasoning_model() {
    let cfg = compose();
    assert_eq!(
        cfg["params"]["reasoning_effort"], "medium",
        "`compose` must name its effort, or a reasoning model is called at its \
         provider's default: {}",
        cfg["params"]
    );
    assert!(
        cfg["params"].get("reasoning").is_none(),
        "a `reasoning` object outranks the declared effort: {}",
        cfg["params"]
    );
    assert_eq!(
        cfg["params"]["max_tokens"], 0,
        "the ceiling is the catalogue's max_output (GH #1097), not a cap a medium \
         deliberation could exhaust"
    );
}

#[test]
fn the_reason_for_it_travels_with_the_cell() {
    let cfg = compose();
    let said = format!(
        "{} {}",
        cfg["description"]["purpose"].as_str().unwrap_or(""),
        cfg["description"]["not_in_scope"].as_str().unwrap_or("")
    );
    assert!(
        said.contains("reasoning"),
        "the cell carries a setting its own description does not explain, which \
         is a setting the next reader deletes"
    );
    assert!(
        said.contains("override_params"),
        "and it must name the way back: a build that wants the deliberation \
         overrides one cell and one param"
    );
}
