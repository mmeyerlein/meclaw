//! Welle Live, zell-uhr (#793, R-L9) — a delegation nobody answers is closed
//! by the cell.
//!
//! `session.delegation.created` hands work to this colony and the provider then
//! waits. Measured over four delegations of one 305 s session (S0,
//! `a-pacing.json`): three of them produced ONE holding sentence of 1.0-2.8 s
//! and then 55-58 s of silence, and no provider-side timeout appeared within
//! the longest observation of 59.4 s. The caller is the one who pays for that,
//! and they pay by hearing nothing.
//!
//! The first line against it is the `advise` prompt (talky answers every
//! delegation, even empty-handed). The second is here, and it is a deadline:
//! a delegation open longer than `params.duplex.delegation_grace_ms` is closed
//! with a `Commentary` append on ITS `delegation_id` carrying
//! `params.duplex.delegation_fallback`. That is the same shape a `fact`
//! travels in — measured taken up and spoken, S0 b4 and proof B-2 — so the
//! caller hears the model say, in its own words, that it cannot look this up
//! right now.
//!
//! Exactly once. A deadline that fired every tick would append the same
//! sentence for the rest of the call.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{advise_msg, boot_fake, duplex_params};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The fallback sentence this fixture configures, so the assertion names the
/// param rather than a string that happens to match the shipped default.
const FALLBACK: &str = "Das kann ich gerade nicht nachsehen.";

/// A duplex params document with the two deadlines held short.
///
/// `gpt_live` rather than `echo`, because the three numbers of this strand live
/// with that adapter exactly as `turn_gap_ms` and `close_grace_ms` do; the
/// object that actually RUNS is still the fixture's fake (see `duplex_params`).
fn ticking(grace_ms: u64) -> meclaw_core::serde_json::Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be Egon.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "delegation_grace_ms": grace_ms,
            "delegation_fallback": FALLBACK,
        },
    }))
}

/// Past the deadline the cell closes the delegation itself — once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_delegation_is_closed_once() {
    let mut live = boot_fake(ticking(300)).await;
    let (_client, _hello) = live.connect("session=stale-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-stale".to_string(),
            offset_ms: 0,
        })
        .await;
    // The lane went out; nothing in this colony answers it.
    let _ = live.emission("delegation").await;

    match live.control().await {
        DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        } => {
            assert_eq!(
                kind,
                AppendKind::Commentary,
                "the fallback is said out loud, like every `fact`"
            );
            assert_eq!(
                delegation_id.as_deref(),
                Some("dlg-stale"),
                "it closes THAT delegation, not the session in general"
            );
            assert_eq!(content, FALLBACK, "the operator's sentence, verbatim");
        }
        other => panic!("the deadline appends a commentary, got {other:?}"),
    }

    // And the delegation is gone from the list: the next ticks find nothing.
    let after = live.controls_for(Duration::from_millis(800)).await;
    assert!(
        after.is_empty(),
        "a closed delegation is closed once, not once per tick: {after:?}"
    );
}

/// A delegation answered inside the grace is never given the fallback.
///
/// The load-bearing half: without it the lock above also passes against a cell
/// that appends the fallback to every delegation the moment it arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answered_delegation_never_hears_the_fallback() {
    let mut live = boot_fake(ticking(5_000)).await;
    let (_client, _hello) = live.connect("session=stale-2&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-answered".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;

    live.send(advise_msg(
        "stale-2",
        "fact",
        "The appointment is on Thursday at a quarter past three.",
        Some("dlg-answered"),
    ))
    .await;

    let appends = live.controls_for(Duration::from_millis(1_200)).await;
    let contents: Vec<String> = appends
        .iter()
        .filter_map(|c| match c {
            DuplexControl::Append { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        contents,
        vec!["The appointment is on Thursday at a quarter past three.".to_string()],
        "the answer, and nothing the deadline added beside it"
    );
}
