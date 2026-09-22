//! Welle Live, L2b — in `hold` the key IS the model's ear (OR-L24).
//!
//! A cascade opens and closes a recognition session around a hold. A duplex
//! session cannot: the session IS the conversation, and closing it would end
//! the call. So the boundary becomes a mute — the model stops hearing between
//! two holds and hears again while the key is down — and nothing about it
//! reaches `turns.rs`, which is not instantiated on this path at all.
//!
//! The negative half is the one that would go unnoticed: in `auto` a `hold` is
//! the wrong frame, and a connection that acted on it anyway would leave the
//! model deaf for the rest of a call nobody is holding a key on.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake, duplex_params};
use meclaw_cells::voice::contract::DuplexControl;
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The failure marker of this file.
const MARKER: Duration = Duration::from_secs(30);

/// In `hold`: shut at the start, open on the key, shut again when it comes up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_key_opens_and_closes_the_model_s_ear() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "hold"}))).await;
    let (mut client, hello) = live.connect("session=hold-1&mode=hold").await;
    assert_eq!(hello["mode"], json!("hold"), "got {hello}");
    let _session = live.session().await;

    assert_eq!(
        live.control().await,
        DuplexControl::Mute,
        "a hold connection starts with the ear shut: nobody has pressed \
         anything yet"
    );

    client.hold().await.expect("press the key");
    assert_eq!(live.control().await, DuplexControl::Unmute);

    client.release().await.expect("let the key go");
    assert_eq!(live.control().await, DuplexControl::Mute);
}

/// In `auto` the ear is open by definition, and a `hold` is the wrong frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_auto_a_hold_is_refused_and_reaches_no_ear() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, _hello) = live.connect("session=hold-2&mode=auto").await;
    let _session = live.session().await;

    client.hold().await.expect("the socket takes the frame");
    let error = client
        .next_frame_of_type("error", MARKER)
        .await
        .expect("a frame that is wrong in this mode is answered");
    assert_eq!(error["code"], json!("wrong_mode"), "got {error}");

    assert!(
        live.controls_for(Duration::from_millis(250))
            .await
            .is_empty(),
        "and the model's ear was not touched"
    );
}

/// A `mode` switch moves the ear with it, and is acknowledged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mode_switch_moves_the_ear() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "hold"}))).await;
    let (mut client, _hello) = live.connect("session=hold-3&mode=hold").await;
    let _session = live.session().await;
    assert_eq!(live.control().await, DuplexControl::Mute);

    client.set_mode("auto").await.expect("switch");
    assert_eq!(
        live.control().await,
        DuplexControl::Unmute,
        "`auto` is an open ear by definition — without this the audio of a \
         page that asked for it goes nowhere, silently"
    );
    let ack = client
        .next_frame_of_type("mode", MARKER)
        .await
        .expect("the switch is acknowledged");
    assert_eq!(ack["mode"], json!("auto"), "got {ack}");
}
