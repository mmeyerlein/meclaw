//! Welle Live, L1 — a typo in the duplex block is a typo, not a setting.
//!
//! The rule the whole params surface of this cell runs on: a key nobody reads
//! that is silently ignored is a setting an operator believes in. The duplex
//! block joins it, and it joins the OTHER half too — `duplex` is not a key a
//! runtime params update may name, for the reason `stt` and `tts` are not: it
//! carries a credential and the format identity of a live session, and both are
//! settled at birth.

use meclaw_cells::params_overlay::{OverlayParams, apply_update};
use meclaw_cells::voice::params::{VoiceOverlay, VoiceParams};
use meclaw_core::serde_json::{Map, json};

fn duplex_doc() -> meclaw_core::JsonValue {
    json!({
        "mount": "voice",
        "duplex": {"provider": "gpt_live", "api_key": "k", "instructions": "x"}
    })
}

/// A key the adapter does not have is named back, so an operator can see the
/// typo instead of wondering why a knob did nothing.
#[test]
fn a_key_the_adapter_does_not_have_is_named_back() {
    let e = VoiceParams::parse(&json!({
        "mount": "voice",
        "duplex": {
            "provider": "gpt_live", "api_key": "k", "instructions": "x", "banana": 1
        }
    }))
    .expect_err("an unknown key in the block is not a setting");
    assert!(e.contains("banana"), "the refusal names the typo: {e}");
}

/// And the loopback knows two keys, deliberately: a knob on a calibration is a
/// knob on the measurement.
#[test]
fn the_loopback_knows_its_rate_and_nothing_else() {
    VoiceParams::parse(&json!({
        "mount": "voice", "duplex": {"provider": "echo", "sample_rate": 24000}
    }))
    .expect("the rate is the one knob the loopback has");
    let e = VoiceParams::parse(&json!({
        "mount": "voice", "duplex": {"provider": "echo", "voice": "marin"}
    }))
    .expect_err("a loopback has no voice");
    assert!(e.contains("voice"), "the refusal names the key: {e}");

    let unknown = VoiceParams::parse(&json!({
        "mount": "voice", "duplex": {"provider": "nobody"}
    }))
    .expect_err("a provider nothing answers to is refused at spawn");
    assert!(
        unknown.starts_with("duplex:"),
        "and it is refused as a duplex problem: {unknown}"
    );
}

/// An update naming `duplex` is refused exactly as one naming `stt` is —
/// `unknown param`, because it is not a KNOWN key rather than a forbidden one.
#[test]
fn an_update_may_not_move_the_engine() {
    let current = VoiceOverlay::parse(&duplex_doc()).expect("the duplex document parses");
    for key in ["duplex", "stt", "tts"] {
        let mut update = Map::new();
        update.insert(key.to_string(), json!({"provider": "echo"}));
        let e = apply_update(&current, &update)
            .err()
            .unwrap_or_else(|| panic!("`{key}` is settled at birth"));
        assert!(
            e.detail().contains(key),
            "the refusal names the key that may not move: {}",
            e.detail()
        );
    }
}

/// A mutable key still travels, and the engine survives the merge: the merge
/// base is the SERIALISED current params, so a block missing from the overlay
/// is a block missing from the next parse.
#[test]
fn a_mount_only_update_does_not_lose_the_engine() {
    let current = VoiceOverlay::parse(&duplex_doc()).expect("the duplex document parses");
    let mut update = Map::new();
    update.insert("mount".to_string(), json!("phone"));
    let (merged, _overlay) = apply_update(&current, &update).expect("a mount may move");
    assert_eq!(merged.mount, "phone");
    assert!(
        merged.duplex.is_some(),
        "the engine came through the merge: {merged:?}"
    );
}
