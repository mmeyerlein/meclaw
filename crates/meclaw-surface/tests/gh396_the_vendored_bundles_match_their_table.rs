//! The vendored bundles must still be what `client/VERSIONS.md` says they are.
//!
//! GH #396. There used to be such a test: `gh159_surface_bundles.rs` in
//! `meclaw-api`, which died with the `/surface/*` route it was written against
//! (GH #383). Nothing has checked the table since — an edited bundle would have
//! drifted past every gate in silence, and the rule the table states ("copied in
//! byte-for-byte and **never edited**") had no enforcement behind it.
//!
//! What it checks: the byte count of each compiled-in bundle against its row,
//! and [`meclaw_surface::LIVEVIEW_VERSION`] against the version column of the
//! LiveView row — the two halves that must move together on an upgrade, because
//! a mismatch between them is only a `console.warn` in the browser.
//!
//! What it deliberately does not check: the SHA-256 column. No crate in the
//! workspace provides a hash, and this test adds no dependency to get one — the
//! predecessor made the same call. The sums are there for a human with
//! `sha256sum`.

const TABLE: &str = include_str!("../src/client/VERSIONS.md");

/// One row of the bundle table: version and byte count for a file name.
///
/// The table is markdown: `| `<file>` | <version> | <bytes> | `<sha>` |`.
/// Splitting on `|` is enough and keeps this test free of a parser dependency.
fn row(file: &str) -> (String, usize) {
    let needle = format!("`{file}`");
    for line in TABLE.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // ["", file, version, bytes, sha, ""]
        if cells.len() != 6 || cells[1] != needle {
            continue;
        }
        let bytes: usize = cells[3]
            .parse()
            .unwrap_or_else(|_| panic!("byte column of `{file}` is not a number: {:?}", cells[3]));
        return (cells[2].to_string(), bytes);
    }
    panic!("no row for `{file}` in client/VERSIONS.md");
}

#[test]
fn every_vendored_bundle_has_the_byte_count_its_row_claims() {
    for file in ["phoenix_live_view.min.js", "phoenix.min.js"] {
        let (_, claimed) = row(file);
        let (_, body) = meclaw_surface::bundle(file).expect("bundle is compiled in");
        assert_eq!(
            body.len(),
            claimed,
            "`{file}` is {} bytes, client/VERSIONS.md claims {claimed} — either a \
             bundle was edited (it must not be) or an upgrade forgot the table",
            body.len()
        );
    }
}

#[test]
fn the_reported_liveview_version_is_the_one_in_the_table() {
    let (version, _) = row("phoenix_live_view.min.js");
    assert_eq!(
        meclaw_surface::LIVEVIEW_VERSION,
        version,
        "the version reported on join and the bundle served must move together"
    );
}
