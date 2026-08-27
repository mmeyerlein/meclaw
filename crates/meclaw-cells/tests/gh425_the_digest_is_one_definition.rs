//! GH #425 — `canonical()` and `digest()` exist in three shipped scripts (the
//! two that draw the digest, and the one that checks it). They are copies
//! because a `code` cell has no shared helper, and a copy that drifts turns the
//! integrity check into a coin flip that always says no.
//!
//! So the copies are compared, byte for byte, off the tree.

use meclaw_core::serde_json::Value;

const SOURCES: &[(&str, &str)] = &[
    ("recipes", "templates/builder/recipes/config.json"),
    ("normalise", "templates/builder/normalise/config.json"),
    ("submit", "templates/submit/gate/config.json"),
];

const OPEN: &str = "# --8<-- digest-helper";
const CLOSE: &str = "# --8<-- end";

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The block between the two markers, verbatim.
///
/// The markers are shipped INSIDE the scripts on purpose: they are the anchor,
/// and whoever removes one makes this test red rather than silent.
fn helper_block(rel: &str) -> String {
    let path = repo_root().join(rel);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel}: no script_inline"));
    let start = script
        .find(OPEN)
        .unwrap_or_else(|| panic!("{rel}: the opening marker `{OPEN}` is gone"));
    let end = script[start..]
        .find(CLOSE)
        .unwrap_or_else(|| panic!("{rel}: the closing marker `{CLOSE}` is gone"));
    script[start..start + end + CLOSE.len()].to_string()
}

#[test]
fn the_digest_helper_is_identical_in_every_script_that_carries_it() {
    let extracted: Vec<(&str, String)> = SOURCES
        .iter()
        .map(|(name, path)| (*name, helper_block(path)))
        .collect();
    let (first_name, first) = &extracted[0];
    assert!(
        !first.trim().is_empty(),
        "no helper block found in {first_name} — the marker moved"
    );
    assert!(
        first.contains("sort_keys=True") && first.contains("sha256"),
        "the block between the markers is not the digest helper any more"
    );
    for (name, block) in &extracted[1..] {
        assert_eq!(
            block, first,
            "the digest helper in {name} has drifted from the one in {first_name}"
        );
    }
}

#[test]
fn the_two_halves_of_the_check_agree_on_the_same_bytes() {
    // Not "the sources look alike" — actually run both and compare the hex. The
    // builder draws the digest and the submitter checks it; if they ever
    // disagreed, every honest manifest would be refused as a forgery and the
    // failure would read like a security event.
    let program = format!(
        "{}\nimport sys\nd = json.load(sys.stdin)\nsys.stdout.write(digest(d))\n",
        // The helper alone, plus the imports it needs.
        format_args!("import json, hashlib\n{}", helper_block(SOURCES[0].1))
    );
    let manifest = meclaw_core::serde_json::json!([
        {"scope": "/os", "ctx": {"note": "ümlaut and a comma, too"},
         "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}}
    ]);
    let a = meclaw_testing::run_shipped_script(&program, &manifest.to_string());
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));

    let program_b = format!(
        "{}\nimport sys\nd = json.load(sys.stdin)\nsys.stdout.write(digest(d))\n",
        format_args!("import json, hashlib\n{}", helper_block(SOURCES[2].1))
    );
    let b = meclaw_testing::run_shipped_script(&program_b, &manifest.to_string());
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));

    assert_eq!(
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&b.stdout),
        "the builder and the submitter hash the same manifest differently"
    );
    assert_eq!(a.stdout.len(), 64, "a sha256 hex digest is 64 characters");
}
