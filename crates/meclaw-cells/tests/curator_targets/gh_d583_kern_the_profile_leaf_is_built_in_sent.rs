//! D58-3 kern (GH #1524, contract point 3, O4) -- the profile's leaf is
//! listed as built.
//!
//! WHY this file exists: a target whose profile carries a text for its own
//! slot (`instructions.slot`/`text`) gets that text as a leaf of the system
//! part while the composition left the slot empty. The curator made that
//! leaf -- no cell of the composition sent it -- but the snapshot (`sent`)
//! listed it `raw`, as if it came with the tree: the leaf was added before the
//! cut took the paths it holds (`s_had`). "Was the context there?" then
//! points at a tree that never held the text. The leaf is listed `built`.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const RULES: &str = "Speak in short sentences.";
const SLOT: &str = "instructions.voice_rules";

#[test]
fn gh_d583_kern_the_profile_leaf_is_built_in_sent() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    announce_profile(
        &mut h,
        "voice",
        json!({"instructions": {"slot": "voice_rules", "text": RULES}}),
    );
    let c = say(&mut h, Some("voice"), "t1", "Hello there.");
    let leaves = system_leaves(&Value::Object(c.body.clone())["system"]);
    assert!(
        leaves.iter().any(|(p, t)| p == SLOT && t == RULES),
        "the profile's text is sent in its slot: {leaves:?}"
    );
    let sent = sent_of(&h, &c).unwrap_or_else(|| {
        panic!(
            "the snapshot row of the call carries `sent`: {:?}",
            h.stderr
        )
    });
    let hash = sha256_hex(&canonical(&json!({"path": SLOT, "text": RULES})));
    let e = sent
        .iter()
        .find(|e| e["hash"] == json!(hash))
        .unwrap_or_else(|| panic!("the profile's leaf is listed: {sent:?}"));
    assert_eq!(e["kind"], json!("frame"), "a leaf of the system part: {e}");
    assert_eq!(
        e["form"],
        json!("built"),
        "the curator made the leaf, so it is listed built: {e}"
    );
    // A leaf the composition sent stays raw.
    let mode = sha256_hex(&canonical(
        &json!({"path": "instructions.mode", "text": "Be brief."}),
    ));
    let m = sent
        .iter()
        .find(|e| e["hash"] == json!(mode))
        .unwrap_or_else(|| panic!("the composition's leaf is listed: {sent:?}"));
    assert_eq!(m["form"], json!("raw"), "{m}");
}
