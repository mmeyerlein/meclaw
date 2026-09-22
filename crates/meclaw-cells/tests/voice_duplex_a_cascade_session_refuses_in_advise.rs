//! Welle Live, L2b — a cascade has no append channel, and says so.
//!
//! There is a recogniser and a voice behind a cascade session, and neither
//! takes guidance up: a `fact` handed to one would have to become a synthesis,
//! which is a different thing said in a different voice. So the refusal is by
//! NAME — `wrong_engine`, from the closed list of contract § 1.4 — rather than
//! a silent drop, because a colony that believes it delivered guidance and did
//! not is a colony that will say the same thing twice.
//!
//! The second half is the one the code has to get right: a cascade emission
//! carries no `engine` key at all. Not `engine: "cascade"` — the KEY is absent,
//! which is what lets a member's firewall edge read `has(hop.engine) ?
//! hop.engine : ''` and a colony wired against an older `voice` go on working.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{advise_msg, boot_cascade, cascade_params, header};
use meclaw_core::serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cascade_refuses_an_advise_by_name() {
    let mut live = boot_cascade(cascade_params()).await;
    let (_client, hello) = live.connect("session=casc-1&mode=auto").await;
    assert_eq!(
        hello["duplex"], false,
        "the fixture really is a cascade: {hello}"
    );

    live.send(advise_msg(
        "casc-1",
        "fact",
        "The appointment is set.",
        None,
    ))
    .await;

    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code"),
        Some(&json!("wrong_engine")),
        "got {refusal}"
    );
    assert_eq!(header(&refusal, "call_id"), Some(&json!("casc-1")));
    assert_eq!(
        header(&refusal, "engine"),
        None,
        "a cascade stamps NOTHING — the key is absent, not a second name: \
         {refusal}"
    );
}

/// And a cascade is refused for the engine before its sessions are looked at:
/// the engine is the reason there is no channel, so a section it knows and a
/// call that never existed get the same answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_engine_is_read_before_the_session_is_looked_up() {
    let mut live = boot_cascade(cascade_params()).await;
    // No connection at all: `wrong_engine` is a fact about the CELL, and a
    // caller must not have to guess whether its call was simply over.
    live.send(advise_msg("nobody", "correction", "Be formal.", None))
        .await;

    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code"),
        Some(&json!("wrong_engine")),
        "got {refusal}"
    );
}
