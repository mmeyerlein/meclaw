//! GH #303 — the `channel` level is retired, and nothing shipped still addresses it.
//!
//! `channel@1.0.3` was a hive around exactly two things: the connector that owns
//! the chat credential, and the slot its current `talky` generation occupies.
//! ADR-0002 § Nachtrag 2026-08-20 rules that a level which groups nothing is not
//! a level — and this one grouped nothing, because the plurality it was built
//! for never arrived: no colony ever put a second occupant beside the
//! generation slot, and the connector it wrapped is one cell as of the previous
//! task of this wave. What the level did carry — the lane normalisation and the
//! `in_turn → error` drain pairing — moves up to the `channels` level inside the
//! `assistant` template, where more than one channel actually meets.
//!
//! # What is read
//!
//! Three facts off disk, with no colony booted. The shape of the shipped
//! library is a filesystem fact; reading it through an instantiated colony would
//! report on a copy, not on what ships.
//!
//! 1. `templates/channel/` does not exist. The directory is the template — a
//!    retirement that leaves the tree standing has retired nothing.
//! 2. No `template.json` anywhere under `templates/` declares `name ==
//!    "channel"`. The directory name is a convention; the declared name is what
//!    a `template` reference in a mutation resolves against, so a copy of the
//!    level under a different directory would still be instantiable.
//! 3. No shipped `config.json` under `templates/` or `examples/` still points at
//!    `telegram-connector/proxy` — neither as an `override_params` key nor as an
//!    edge endpoint. That path was the address of the credential cell *inside*
//!    the level; it stopped existing twice over (the connector collapsed to one
//!    cell in the previous task, the level dies here), and an override that
//!    names a node no template carries is a staging failure at the customer, not
//!    here.
//!
//! The No-Delete policy of `docs/meclaw-overview.md` is untouched by this: it
//! governs a colony `{root}` — the instantiated tree a colony owns — not this
//! repository's template library, which is source and versioned like source.

use meclaw_core::serde_json::Value;

/// `templates/`, from this crate's manifest directory.
fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// `examples/`, from this crate's manifest directory.
fn examples_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Every file named `file_name` at or below `dir`.
fn collect(dir: &std::path::Path, file_name: &str, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // A tree that did not travel into the public export is not a defect --
        // the same rule the other shipped-tree sweeps follow.
        return;
    };
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect(&path, file_name, out);
        } else if entry.file_name() == file_name {
            out.push(path);
        }
    }
}

#[test]
fn the_channel_directory_is_gone() {
    let dir = templates_root().join("channel");
    assert!(
        !dir.exists(),
        "{}: the level is retired (GH #303, ADR-0002 § Nachtrag 2026-08-20) -- \
         a retirement that leaves the directory standing has retired nothing",
        dir.display()
    );
}

#[test]
fn no_template_declares_the_channel_name() {
    let root = templates_root();
    let mut manifests = Vec::new();
    collect(&root, "template.json", &mut manifests);
    assert!(
        !manifests.is_empty(),
        "{}: no template.json found at all -- the sweep read nothing",
        root.display()
    );
    let mut offenders = Vec::new();
    for path in &manifests {
        let raw =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let val: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
        if val["name"].as_str() == Some("channel") {
            offenders.push(path.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "the declared name is what a mutation's `template` reference resolves against, \
         so a retired level under a new directory is still instantiable: {offenders:?}"
    );
}

/// Does the slash-separated `path` run through the segment sequence `needle`?
///
/// An endpoint is written relative to the hive that holds it (`./x`, `x/y`), so
/// a plain `contains` would match a longer name and an equality test would miss
/// every prefix. Comparing segment runs is the only reading that matches what
/// the router does.
fn runs_through(path: &str, needle: &str) -> bool {
    let want: Vec<&str> = needle.split('/').collect();
    let have: Vec<&str> = path
        .split('/')
        .filter(|s| *s != "." && !s.is_empty())
        .collect();
    have.windows(want.len()).any(|w| w == want.as_slice())
}

/// Collect every hit of `needle` that sits at an address a mutation can use:
/// as a key of an `override_params` object, or as an edge's `from`/`to`.
fn addressed_as(val: &Value, needle: &str, out: &mut Vec<String>) {
    match val {
        Value::Object(map) => {
            if let Some(Value::Object(overrides)) = map.get("override_params") {
                for key in overrides.keys() {
                    if runs_through(key, needle) {
                        out.push(format!("override_params key {key:?}"));
                    }
                }
            }
            if let Some(Value::Array(edges)) = map.get("edges") {
                for edge in edges {
                    for end in ["from", "to"] {
                        if let Some(s) = edge.get(end).and_then(Value::as_str)
                            && runs_through(s, needle)
                        {
                            out.push(format!("edge {end} {s:?}"));
                        }
                    }
                }
            }
            for child in map.values() {
                addressed_as(child, needle, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                addressed_as(item, needle, out);
            }
        }
        _ => {}
    }
}

#[test]
fn nothing_shipped_still_addresses_the_connector_inside_the_level() {
    let mut configs = Vec::new();
    collect(&templates_root(), "config.json", &mut configs);
    collect(&examples_root(), "config.json", &mut configs);
    assert!(
        !configs.is_empty(),
        "no config.json found at all -- the sweep read nothing"
    );
    let mut offenders = Vec::new();
    for path in &configs {
        let raw =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let Ok(val) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            // Not every config.json in an example tree is a template config;
            // an unparseable one is another sweep's finding, not this one's.
            continue;
        };
        let mut hits = Vec::new();
        addressed_as(&val, "telegram-connector/proxy", &mut hits);
        for hit in hits {
            offenders.push(format!("{}: {hit}", path.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "`telegram-connector/proxy` is the address of a cell inside a level that no longer \
         exists -- an override or an edge naming it fails at staging, not here: {offenders:?}"
    );
}
