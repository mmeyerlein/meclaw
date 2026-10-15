//! The fan-out is not written a second time. `dispatcher@1.1.1` already names
//! the tool and lets an edge know the cell, already refuses a bundle above its
//! call budget with synthetic error results that keep the round fan-in-complete,
//! and already declares `multi_send_capable`. A copy of it inside the builder
//! would be a second thing to keep true.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

#[test]
fn the_builders_dispatcher_is_a_ref_to_the_shipped_dispatcher() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/dispatcher/config.json");
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("dispatcher config"))
            .expect("parses");
    assert_eq!(cfg["cell"]["type"], "ref");
    let t = cfg["cell"]["template"].as_str().expect("template named");
    assert!(
        t.starts_with("dispatcher@"),
        "the fan-out is the shipped dispatcher, referenced -- found {t}"
    );
    assert!(t.contains('@'), "a reference names a version: {t}");
}
