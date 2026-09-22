//! Welle Live, L2b — `close_grace_ms` is the operator's number, not a constant.
//!
//! When the caller hangs up, the duplex connection drops the model's audio and
//! waits for the provider's own verdict (contract § 1.3). The wait is not
//! politeness: the final `Closed { reason, usage_seconds }` travels on the
//! EVENT channel during it, and after OR-L22 the log line the handler writes
//! from that event is the only place a session's cost is ever reported.
//!
//! The wait was first built on a constant of 15 s (OR-L.L2b.1), which is what
//! the shipped `params.duplex.gpt_live.close_grace_ms` says. A document that
//! raised that key got the constant anyway: on a slow close the provider was
//! still finalising when the connection gave up, the verdict came back
//! `Timeout`, the caller was long gone — and the only thing lost was the
//! number, silently. This file is the lock that the connection waits the wait
//! it was configured with.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_lingering, duplex_params};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The failure marker of this file.
const MARKER: Duration = Duration::from_secs(30);

/// The operator's number: not the shipped 15 000, and short enough that a
/// connection still on the constant is unmistakable.
const GRACE_MS: u64 = 400;

/// A close grace well under the shipped 15 s ends the wait at its own number.
///
/// The model here never finalises, so nothing but the clock can end the wait —
/// and the model is the one that measures it: the connection dropping its run
/// closes the control channel, and that is the moment it let go. The
/// discriminator is the distance between 400 ms and 15 s, a factor of
/// thirty-seven, so the window needs no tight bound to be read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_connection_lets_go_at_the_close_grace_it_was_given() {
    let mut live = boot_lingering(duplex_params(json!({})), GRACE_MS).await;
    let (client, _hello) = live.connect("session=grace-1&mode=auto").await;
    let _session = live.session().await;

    // The caller hangs up. `audio_in` closing IS the end of the call, and the
    // connection now owes the provider its close grace and nothing more.
    client.close().await.expect("the caller hangs up");

    let waited = tokio::time::timeout(MARKER, live.let_go.recv())
        .await
        .expect("the connection lets go of the model within the failure marker")
        .expect("the model reports how long it was held");

    assert!(
        waited < Duration::from_secs(5),
        "the connection waited {waited:?} — that is the 15 s constant and not \
         the {GRACE_MS} ms this cell was configured with"
    );
    assert!(
        waited >= Duration::from_millis(GRACE_MS / 2),
        "the connection let go after {waited:?}: it did not wait its grace at \
         all, and a provider that needs a moment to report its meter would be \
         cut off"
    );
}
