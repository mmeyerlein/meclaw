//! GH #303 — `telegram-connector` is one cell, not a hive around one cell.
//!
//! The template shipped as a sealed hive (`params.ports: []`) whose only
//! occupant was the credential-bearing `proxy` cell. The hive grouped nothing:
//! it existed to normalise the cell's two emission shapes onto named lanes
//! (`turn` for an inbound turn, `error` for the connector's own failure) and to
//! give a caller one address to wire. ADR-0002 § Nachtrag 2026-08-20 rules that
//! a level which groups a single occupant is not a level — the normalisation
//! belongs to the level that HOLDS channels, and that level is `channels`
//! (built in a later task of this wave).
//!
//! So the wrapper goes and the cell moves up. What a caller loses is the lane
//! rewrite: it no longer names a hive path plus `hop.route == 'in_reply'`, it
//! wires the `proxy` cell directly and reads `hop.error_code` to tell an
//! inbound turn from a connector failure. That is a removal, which is why the
//! template moves to **`2.0.0`** — neither of the two rules in
//! `docs/development-rules.md` § 4 (a repair moves the third digit, an addition
//! the second) covers taking a documented address away.
//!
//! # What is read
//!
//! Three facts off disk, in the shipped tree, with no colony booted — the shape
//! is a filesystem fact and reading it any other way would report on a copy:
//!
//! - the root `config.json` IS the cell (`cell.type == "proxy"`),
//! - nothing sits below it (no child directory carries a `config.json`),
//! - `template.json` says `2.0.0`.
//!
//! The third is not decoration next to the first two: a tree that collapsed
//! while its version stood still is exactly the failure the bump exists to
//! prevent — a caller pinning `telegram-connector@1.0.0` would resolve to a
//! template that no longer answers at the address that version documented.

use meclaw_core::serde_json::Value;

/// `templates/telegram-connector`, from this crate's manifest directory.
fn connector() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/telegram-connector")
}

/// Parse one JSON file of the template, failing with its path.
fn read_json(path: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()))
}

#[test]
fn the_root_config_is_the_proxy_cell() {
    let path = connector().join("config.json");
    let val = read_json(&path);
    assert_eq!(
        val["cell"]["type"].as_str(),
        Some("proxy"),
        "{}: the connector's root is the cell itself, not a hive around it (GH #303)",
        path.display()
    );
}

#[test]
fn nothing_lives_below_the_connector() {
    let root = connector();
    let mut below = Vec::new();
    for entry in std::fs::read_dir(&root).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() && path.join("config.json").is_file() {
            below.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    assert!(
        below.is_empty(),
        "{}: the connector is one cell -- these children still carry a config.json: {below:?}",
        root.display()
    );
}

#[test]
fn the_removal_moved_the_first_digit() {
    // The claim is about the FIRST digit, so that is what is asserted. It used
    // to pin the whole string `2.0.0`, which made every later repair of this
    // template red for a reason that has nothing to do with GH #303 -- a number
    // that ages inside a test, the same defect class GH #408 removed from the
    // prose. The template reached 2.0.1 in the #408 sweep (an unresolvable
    // `firewall` reference in a shipped description field); the major digit is
    // what carries the removed address, and it must never go back below 2.
    let path = connector().join("template.json");
    let val = read_json(&path);
    let version = val["version"].as_str().unwrap_or_default();
    let major = version.split('.').next().unwrap_or_default();
    assert_eq!(
        major,
        "2",
        "{path}: collapsing the hive takes a documented address away -- that is the first \
         digit, not the second and not the third. Found version {version:?}",
        path = path.display()
    );
}
