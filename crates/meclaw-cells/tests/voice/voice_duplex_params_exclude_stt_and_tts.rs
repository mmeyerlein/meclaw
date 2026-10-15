//! Welle Live, L1 — a `voice` cell is a cascade OR a duplex session, never both.
//!
//! The exclusivity is the whole of the configuration contract (`vertraege.md`
//! § 1.2). A document that named `duplex` beside `stt` would describe two
//! models listening to one microphone, and whichever of them the code happened
//! to read first would be the answer — so it is refused at parse time, where
//! the message can say which of the two the operator has to drop.
//!
//! The second half is the migration: `override_params` MERGES and cannot remove
//! a key, so the only way an instantiation can switch a shipped cascade
//! template onto a duplex session is to set the two blocks to `null` beside the
//! new one. If `null` did not count as absent here, the switch would be
//! impossible to write.

use meclaw_cells::voice::params::{DuplexParams, SttParams, VoiceParams};
use meclaw_core::serde_json::json;

/// The defaults of a `gpt_live` block, from `vertraege.md` § 1.2 — one assert
/// per key, because a default that quietly moved is a session opened on a
/// number nobody chose.
#[test]
fn a_gpt_live_block_defaults_to_the_documented_table() {
    let p = VoiceParams::parse(&json!({
        "mount": "voice",
        "duplex": {"provider": "gpt_live", "api_key": "k", "instructions": "be Egon"}
    }))
    .expect("provider + credential + instructions is enough");
    let Some(DuplexParams::GptLive(g)) = &p.duplex else {
        panic!(
            "the duplex block parses into the gpt_live variant: {:?}",
            p.duplex
        );
    };
    assert_eq!(g.base_url, "wss://api.openai.com");
    assert_eq!(g.model, "gpt-live-1");
    assert_eq!(g.voice, "marin");
    assert_eq!(g.sample_rate, 16000);
    assert_eq!(g.instructions, "be Egon");
    assert_eq!(g.greeting, "", "an empty greeting means the model waits");
    assert_eq!(g.turn_gap_ms, 1000);
    assert_eq!(g.backchannel_max_ms, 1500);
    assert_eq!(g.spoken_quiet_ms, 1500);
    assert_eq!(g.spoken_cap_ms, 8000);
    assert_eq!(g.close_grace_ms, 15000);
}

/// A duplex block stands alone, with the cascade keys missing or `null`.
#[test]
fn a_duplex_block_stands_without_the_cascade_pair() {
    for doc in [
        json!({"mount": "voice", "duplex": {"provider": "echo"}}),
        json!({"mount": "voice", "duplex": {"provider": "echo"}, "stt": null, "tts": null}),
    ] {
        let p = VoiceParams::parse(&doc).unwrap_or_else(|e| {
            panic!("a duplex cell needs no stt and no tts: {e} ({doc})");
        });
        assert!(p.duplex.is_some(), "the block reached the params: {doc}");
        assert!(
            p.tts.is_none(),
            "a duplex session speaks for itself; there is no synthesis beside it"
        );
        assert!(
            matches!(p.stt, SttParams::Echo),
            "and the recogniser slot carries the inert placeholder (OR-L23), \
             never a second ear: {:?}",
            p.stt
        );
    }
}

/// Both blocks at once is refused, and the message says which rule it broke.
#[test]
fn a_duplex_block_beside_a_recogniser_is_refused() {
    for doc in [
        json!({"mount": "voice", "duplex": {"provider": "echo"}, "stt": {"provider": "echo"}}),
        json!({
            "mount": "voice",
            "duplex": {"provider": "echo"},
            "stt": null,
            "tts": {"provider": "openai", "api_key": "k"}
        }),
    ] {
        let e = VoiceParams::parse(&doc).expect_err("two engines is not a configuration");
        assert!(
            e.contains("exclusive"),
            "the refusal names the rule rather than a key: {e}"
        );
    }
}

/// Without a duplex block nothing moves: `stt` is still required, and a `null`
/// still counts as absent rather than as an engine.
#[test]
fn a_null_recogniser_without_a_duplex_block_is_still_a_cell_with_no_ear() {
    let e = VoiceParams::parse(&json!({"mount": "voice", "stt": null}))
        .expect_err("a cascade cell needs a recogniser");
    assert!(
        e.starts_with("stt: required"),
        "the refusal is the one it always was: {e}"
    );
}

/// The two refusals a live session is worth refusing for.
#[test]
fn an_empty_instruction_and_a_rate_the_model_refuses_are_refused_here() {
    let empty = VoiceParams::parse(&json!({
        "mount": "voice",
        "duplex": {"provider": "gpt_live", "api_key": "k", "instructions": "  "}
    }))
    .expect_err("a live model with nothing said to it answers as somebody else");
    assert!(
        empty.contains("duplex.instructions") && empty.contains("must not be empty"),
        "the refusal names the key an operator has to fill: {empty}"
    );

    let rate = VoiceParams::parse(&json!({
        "mount": "voice",
        "duplex": {
            "provider": "gpt_live", "api_key": "k", "instructions": "x", "sample_rate": 8000
        }
    }))
    .expect_err("8000 is what the vendor refuses, measured");
    assert!(
        rate.contains("16000") && rate.contains("24000"),
        "and it names both rates that would have worked: {rate}"
    );
}

/// An unresolved credential is refused by the NAME of the variable, never by
/// its value — the same rule the two cascade keys have carried since 2.0.0.
#[test]
fn an_unresolved_duplex_credential_is_refused_by_its_name() {
    let e = VoiceParams::parse(&json!({
        "mount": "voice",
        "duplex": {
            "provider": "gpt_live", "api_key": "${OPENAI_API_KEY}", "instructions": "x"
        }
    }))
    .expect_err("an unresolved placeholder is a cell that cannot connect");
    assert!(
        e.starts_with("duplex.api_key:") && e.contains("OPENAI_API_KEY"),
        "the message names the key and the variable: {e}"
    );
}
