//! Welle Live, L1 — the duplex seam has two implementations, and the count is
//! the assertion.
//!
//! ADR-0023 put it in writing before either of them existed: "a trait with one
//! implementation is a shape borrowed from that implementation". The cascade
//! seams each ship three adapters and the shape survived contact with all of
//! them; this one ships two, and the second is the loopback — which is the
//! cheapest possible second opinion and still a real one, because it exercises
//! every event variant the contract has a shape for.
//!
//! So the lock is a count over the source tree rather than a behaviour: a
//! second implementation cannot be quietly removed, and a third one is a
//! deliberate edit of this number. Guarded like every tree-reading test
//! (GH #49): a tree that did not travel is skipped rather than judged.

use meclaw_cells::voice::contract::ProviderTimeouts;
use meclaw_cells::voice::params::DuplexParams;
use meclaw_cells::voice::providers::build_duplex;
use meclaw_core::serde_json::json;

/// The provider directory of the `voice` cell, or `None` where it did not
/// travel.
fn provider_sources() -> Option<Vec<(String, String)>> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/voice/providers");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()? {
        let path = entry.ok()?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name()?.to_string_lossy().to_string();
        out.push((name, std::fs::read_to_string(&path).ok()?));
    }
    out.sort();
    (!out.is_empty()).then_some(out)
}

#[test]
fn exactly_two_files_implement_the_duplex_trait() {
    let Some(sources) = provider_sources() else {
        return;
    };
    let implementors: Vec<&str> = sources
        .iter()
        .filter(|(_, body)| {
            body.lines()
                .any(|l| l.trim_start().starts_with("impl DuplexProvider for "))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        implementors,
        vec!["duplex_echo.rs", "gpt_live.rs"],
        "the duplex seam has exactly these two implementations; a third is a \
         deliberate edit of this list, and a second one going missing turns the \
         trait back into a shape borrowed from one vendor (ADR-0023)"
    );
}

/// And the registry reaches both of them by the name a config writes.
#[test]
fn the_registry_builds_both_by_name() {
    for (doc, name) in [
        (json!({"provider": "echo"}), "echo"),
        (
            json!({"provider": "gpt_live", "api_key": "k", "instructions": "x"}),
            "gpt_live",
        ),
    ] {
        let params: DuplexParams =
            meclaw_core::serde_json::from_value(doc.clone()).expect("the block parses");
        let built = build_duplex(&params, ProviderTimeouts::default())
            .unwrap_or_else(|e| panic!("`{name}` has an adapter: {e}"));
        assert_eq!(built.name(), name, "built from {doc}");
    }
}
