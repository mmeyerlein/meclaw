//! `channel@1.0.2` carries its sub-units as byte copies, and they are pinned.
//!
//! The substrate DOES have a template-in-template reference since GH #277
//! (`cell.type: "ref"`), but `channel` does not use it: it still holds its
//! sub-units as copies of their `config.json` files, which is a fork risk
//! unless something reads both sides. This file is that something.
//!
//! `talky` and `cogny` carried their sub-units the same way until GH #277
//! turned those copies into `cell.type: "ref"` markers, and their byte pins
//! retired with them. **`channel` is deliberately NOT converted**: GH #303
//! dissolves the template altogether in a later wave, and converting a
//! template on its way out buys nothing. So the byte pin below stays alive --
//! a pin lives exactly as long as the byte copy it guards, and this one dies
//! with the template that carries it (orchestrator ruling 2026-08-21). Until
//! then a change to `telegram-connector@1` or `terminal@1` that does not
//! travel into `channel/` fails here, in the same test run, instead of
//! drifting into a colony that instantiated the composite.
//!
//! The file used to be called `channel_composite.rs`; it was renamed in the
//! same commit that converted `cogny` (GH #277, W3 task 11), because the two
//! `*_composite.rs` names now mean two different things and only this one
//! still watches copies.
//!
//! The second test is about the slot rather than the copies. `channel` ships
//! with its generation slot occupied by a terminal, because a `params.graph`
//! endpoint that resolves to nothing is a loud boot failure -- a template
//! cannot carry a door to a node it does not have. If the placeholder ever
//! disappears, every lane of the contract that runs through the slot loses its
//! door, and the contract becomes false at instantiation rather than at the
//! first generation.

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every `config.json` at or below `dir`, relative to it.
fn collect_configs(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            collect_configs(root, &p, out);
        } else if entry.file_name() == "config.json" {
            out.push(p.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

#[test]
fn the_sub_unit_copies_are_byte_identical_to_their_templates() {
    let root = templates_root();
    let channel = root.join("channel");
    if !channel.is_dir() {
        // The public export ships a subset; a template that did not travel is
        // not a defect (same rule the shipped-doc sweeps follow).
        return;
    }
    let mut checked = 0usize;
    for unit in ["telegram-connector", "terminal"] {
        let src = root.join(unit);
        let mut rels = Vec::new();
        collect_configs(&src, &src, &mut rels);
        assert!(!rels.is_empty(), "{unit}: no config.json found");
        for rel in rels {
            let a = std::fs::read(src.join(&rel)).unwrap();
            let b = std::fs::read(channel.join(unit).join(&rel)).unwrap_or_else(|e| {
                panic!(
                    "channel/{unit}/{} missing ({e}) -- the sub-template grew a cell the \
                     composite does not carry",
                    rel.display()
                )
            });
            assert!(
                a == b,
                "channel/{unit}/{} drifted from {unit}/{}",
                rel.display(),
                rel.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 3, "the pin swept almost nothing: {checked}");
}

#[test]
fn the_generation_slot_ships_occupied() {
    let channel = templates_root().join("channel");
    if !channel.is_dir() {
        return;
    }
    let slot = channel.join("terminal/config.json");
    let raw = std::fs::read_to_string(&slot).unwrap_or_else(|e| {
        panic!(
            "{}: the generation slot is empty ({e}). A hive template cannot carry a door to a \
             node it does not have -- without the placeholder every lane through the slot is a \
             dangling endpoint and the colony refuses to boot",
            slot.display()
        )
    });
    let val: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&raw).expect("the slot's config.json parses");
    assert_eq!(
        val["cell"]["type"].as_str(),
        Some("code"),
        "the slot is occupied by something other than a terminal"
    );
}

/// Every edge of the template compiles -- condition AND modifier.
///
/// The shipped-template sweeps read conditions (`gh173_shipped_hive_contracts`
/// parses each one to build its router table) and deliberately drop modifiers:
/// a modifier does not decide WHETHER an edge is taken, so the lane checks do
/// not need one. That leaves the modifier expressions of a shipped template
/// unread by anything until a colony boots -- and this template's modifiers are
/// load-bearing. One promotes the chat identity into `context`, without which a
/// reply has no chat to go to; one declares `context.channel_open_history`,
/// which is the room's disclosure policy and the only place it is written down.
/// A typo in either is a boot failure at the customer, not here.
#[test]
fn every_edge_of_the_channel_compiles() {
    let mut checked = 0usize;
    for hive in [
        "channel",
        "telegram-connector",
        "channel/telegram-connector",
    ] {
        checked += edges_compile(&templates_root().join(hive).join("config.json"));
    }
    assert!(checked >= 15, "the sweep read almost no edges: {checked}");
}

/// Compile every edge of one hive `config.json` and return how many were read.
/// A template that is not in this tree is skipped, not judged -- the public
/// export ships a subset.
fn edges_compile(config: &std::path::Path) -> usize {
    let Ok(raw) = std::fs::read_to_string(config) else {
        return 0;
    };
    let val: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&raw).expect("the hive config.json parses");
    let params: meclaw_colony::config::HiveParams =
        meclaw_core::serde_json::from_value(val["params"].clone()).expect("params deserialise");
    let mut checked = 0usize;
    for spec in &params.graph.edges {
        if let Some(src) = &spec.condition {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("condition {src:?} does not compile: {e}"));
        }
        if let Some(m) = &spec.modifier {
            meclaw_colony::cel_eval::parse_modifier(m).unwrap_or_else(|e| {
                panic!(
                    "modifier of {} -> {} does not compile: {e:?}",
                    spec.from, spec.to
                )
            });
        }
        checked += 1;
    }
    checked
}
