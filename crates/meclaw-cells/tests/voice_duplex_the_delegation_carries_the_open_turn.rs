//! Welle Live, L2b — a delegation carries the sentence it was asked in.
//!
//! `session.delegation.created` carries no task text at all (the wave measured
//! that in the recorded reference run of 21.09.2026, S0). What the backend needs
//! is what the caller was saying, so the `delegation` lane carries the open
//! turn's user text — or, where no turn is open, the last one that closed
//! (contract § 1.4).
//!
//! Two locks, and they are separated on purpose. The first is this strand's:
//! the event becomes exactly one emission on the `delegation` lane, with the
//! delegation's identity, its offset on the model's clock and the engine stamp.
//! The second is the TEXT, and that comes out of `live_turns::step`, which
//! strand L3 fills — it is armed the moment L3 is on master.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake, duplex_params, header, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;

/// The event becomes one emission on its own lane, named and stamped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegation_is_a_lane_of_its_own() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=del-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-42".to_string(),
            offset_ms: 4_200,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        header(&emission, "delegation_id"),
        Some(&json!("dlg-42")),
        "got {emission}"
    );
    assert_eq!(header(&emission, "offset_ms"), Some(&json!(4_200)));
    assert_eq!(header(&emission, "call_id"), Some(&json!("del-1")));
    assert_eq!(
        header(&emission, "engine"),
        Some(&json!("duplex")),
        "the engine is what a member's firewall edge passes through: {emission}"
    );
    assert_eq!(
        header(&emission, "turn_id"),
        Some(&json!("del-1#0")),
        "the delegation belongs to the turn that is open: {emission}"
    );
}

/// And it carries the sentence the caller was in the middle of.
///
/// The assertion below is the whole of contract § 1.4's `delegation` row: the
/// text of the open turn comes from `live_turns::open_user_text`, which is fed
/// by the fragments `live_turns::step` has taken in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegation_carries_what_the_caller_was_saying() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=del-2&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Can you look up".to_string(),
            start_ms: 0,
            end_ms: 900,
        })
        .await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: " when my appointment is?".to_string(),
            start_ms: 900,
            end_ms: 1_800,
        })
        .await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-43".to_string(),
            offset_ms: 1_900,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        message_text(&emission, 0),
        "Can you look up when my appointment is?",
        "both fragments of the open turn, and nothing else: {emission}"
    );
}
