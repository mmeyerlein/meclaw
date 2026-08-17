//! GH #159 — the LiveView client is IN the binary. A test, because the failure
//! mode is silent: a page that loads and renders nothing, on every machine except
//! the one that built it.
//!
//! These assert byte length and anchor substrings, **not** a SHA-256: no crate in
//! the workspace provides a hash and this feature adds none.
//! `src/surface/client/VERSIONS.md` records the real sums for a human, and that
//! limit is stated here rather than implied away.

use meclaw_api::surface::bundle;

#[test]
fn both_bundles_are_compiled_in() {
    for (file, needle, min_bytes) in [
        ("phoenix.min.js", "Socket", 20_000),
        ("phoenix_live_view.min.js", "LiveSocket", 100_000),
    ] {
        let (ctype, body) = bundle(file).unwrap_or_else(|| panic!("{file} is not compiled in"));
        assert!(ctype.starts_with("text/javascript"), "{file}: {ctype}");
        assert!(
            body.len() > min_bytes,
            "{file} is only {} bytes",
            body.len()
        );
        assert!(body.contains(needle), "{file} does not contain {needle}");
    }
}

/// The bundles must not have been edited. A drifted byte count is the cheapest
/// signal available without a hash crate, and editing a bundle is the one thing
/// adopting LiveView is meant to avoid — the moment we patch it we own the
/// browser matrix again.
///
/// The expected counts are parsed out of `VERSIONS.md` rather than duplicated
/// here, so the documentation and the check cannot drift apart.
#[test]
fn the_bundles_match_the_byte_counts_recorded_in_versions_md() {
    let doc = include_str!("../src/surface/client/VERSIONS.md");
    let mut checked = 0;
    for line in doc.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // | `<file>` | <version> | <bytes> | `<sha>` |
        if cells.len() < 5 {
            continue;
        }
        let Some(file) = cells[1].strip_prefix('`').and_then(|f| f.strip_suffix('`')) else {
            continue;
        };
        let Ok(expected) = cells[3].parse::<usize>() else {
            continue;
        };
        let (_, body) = bundle(file)
            .unwrap_or_else(|| panic!("VERSIONS.md names {file}, which is not compiled in"));
        assert_eq!(
            body.len(),
            expected,
            "{file}: VERSIONS.md says {expected} bytes, the compiled-in copy has {}",
            body.len()
        );
        checked += 1;
    }
    assert_eq!(
        checked, 2,
        "VERSIONS.md must carry a byte count for both bundles — parsed {checked}"
    );
}

/// The version we report on join lives in the Rust source, and the bundle carries
/// its own. They must agree, and a mismatch is only a `console.warn` in the
/// client — which is exactly why it needs a test.
#[test]
fn the_liveview_bundle_carries_the_version_versions_md_claims() {
    let doc = include_str!("../src/surface/client/VERSIONS.md");
    assert!(
        doc.contains("| 1.2.9 |"),
        "VERSIONS.md must record the LiveView version"
    );
    let (_, body) = bundle("phoenix_live_view.min.js").expect("compiled in");
    assert!(
        body.contains("1.2.9"),
        "the bundle does not contain the version VERSIONS.md claims"
    );
}

#[test]
fn an_unlisted_bundle_does_not_exist() {
    for bad in [
        "../../../etc/passwd",
        "phoenix.min.js.bak",
        "",
        "PHOENIX.MIN.JS",
        "client/phoenix.min.js",
        "canvy.js",
    ] {
        assert!(bundle(bad).is_none(), "{bad:?} must not resolve");
    }
}
