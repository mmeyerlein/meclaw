//! Welle Live, L2b — there is no second session (OR-L20).
//!
//! A cascade retries once: a recogniser that dropped its socket can be given a
//! new one, and the conversation is unaffected because the conversation lives
//! in the cell. A duplex session is the opposite — the session IS the
//! conversation, and a fresh socket would be a fresh conversation with no
//! memory of this one. So a provider that gives up ends the call: the error
//! lane says `duplex_failed`, the client reads `1011`, and the provider is
//! never asked for a second session.
//!
//! The session counter is what makes the last sentence a measurement rather
//! than a hope.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_failing, duplex_params, header};
use meclaw_core::serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The failure marker of this file.
const MARKER: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_gives_up_ends_the_call_once() {
    let mut live = boot_failing(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, hello) = live.connect("session=gone-1&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");

    let error = live.emission("error").await;
    assert_eq!(
        header(&error, "error_code"),
        Some(&json!("duplex_failed")),
        "a call that ends without a word on the error lane is a call nobody \
         can explain: {error}"
    );
    assert_eq!(header(&error, "call_id"), Some(&json!("gone-1")));

    let closed = client
        .collect_until(|f| f.as_close().is_some(), MARKER)
        .await;
    let code = closed
        .iter()
        .find_map(|(_, f)| f.as_close())
        .expect("the client is told the call is over");
    assert_eq!(code, 1011, "got {closed:?}");

    // The measurement the ruling is about: one session, ever.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        1,
        "no retry and no reconnect: the session was the conversation"
    );
}
