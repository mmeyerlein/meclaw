//! GH #458 — `PACK_SLOTS` is a SUBSET of `SYS_KEEP`, and the prose says so too.
//!
//! The `in_pack` lane writes durable `system.*` state into an agent's brain.
//! What it may write is a closed list, `PACK_SLOTS`, and the promise attached
//! to that list is not "these three are nice slots" — it is that a pack, once
//! written, is out of the curator's reach at any budget.
//!
//! That promise rests on ONE relation nothing in the shipped script enforces:
//! `PACK_SLOTS ⊆ SYS_KEEP`. Stage 5 of the same script (`w11_curator.rs`) cuts
//! any `system.*` family that is over `curate_slot_chars` and is NOT in
//! `SYS_KEEP`. So a writable slot outside `SYS_KEEP` would be accepted on the
//! lane, upserted into the brain's `cell.db`, and then curated away behind the
//! sender's back on the very next assembly — a pack silently truncated between
//! two turns, with a `pack_ack` that said `error_code: ""`.
//!
//! Nothing catches that at runtime: the ack is written before the write, the
//! cut names itself only inside a prompt nobody diffs, and the sender is a
//! different hive. So it is caught here, as a drift lock
//! (`docs/development-rules.md` § 2d): both constants are read out of the
//! shipped script, the subset relation is asserted, and the two subtractions
//! that make the subset PROPER are asserted as subtractions rather than
//! left to a reader's memory.
//!
//! No colony: this file is about two tuples and three sentences.

use meclaw_core::serde_json::Value;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The shipped `collector/assemble` script, straight out of its `config.json`
/// — the same technique `w11_curator.rs` reads `SYS_KEEP` with. Reading the
/// constant out of the artefact is the point: a copy of the list in this file
/// could agree with itself while disagreeing with what ships.
fn assemble_script() -> String {
    let p = templates_root().join("collector/assemble/config.json");
    let raw = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("the shipped assemble config must be readable: {p:?}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{p:?} must be JSON: {e}"));
    v["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{p:?} must carry params.script_inline"))
        .to_string()
}

/// The string members of a python tuple constant `NAME = ("a", "b", ...)`,
/// read out of the shipped source. The declaration may wrap over lines, so the
/// scan runs from the `=` to the closing paren and collects every double-quoted
/// word in between.
fn tuple_constant(src: &str, name: &str) -> Vec<String> {
    let needle = format!("\n{name} = (");
    let at = src.find(&needle).unwrap_or_else(|| {
        panic!("`{name}` must stand in the shipped collector/assemble script as a tuple constant")
    });
    let from = at + needle.len();
    let len = src[from..].find(')').unwrap_or_else(|| {
        panic!(
            "the `{name}` declaration must close its paren: {:?}",
            &src[from..from + 200]
        )
    });
    let decl = &src[from..from + len];
    let mut out = Vec::new();
    let mut rest = decl;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    assert!(
        !out.is_empty(),
        "`{name}` must name at least one slot; read: {decl:?}"
    );
    out
}

fn read_file(rel: &str) -> String {
    let p = templates_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?} must be readable: {e}"))
}

/// The slots the lane writes, as this file names them. Every assertion below
/// reads its own copy out of the artefact under test; this is only what the
/// failure messages talk about.
///
/// `instructions` joined the list with GH #488. The subtraction this file used
/// to assert protected nothing: the charter had no other owner, no export and
/// no seed, so a grown agent came up with an empty one. See
/// `gh488_the_agent_record_is_where_the_identity_lives.rs`.
const THE_WRITABLE: [&str; 4] = ["identity", "persona", "handover", "instructions"];

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. Every slot the `in_pack` lane may write is a family the curator is
/// forbidden to touch.
#[test]
fn every_pack_slot_is_a_protected_family() {
    let src = assemble_script();
    let pack = tuple_constant(&src, "PACK_SLOTS");
    let keep = tuple_constant(&src, "SYS_KEEP");

    let outside: Vec<&String> = pack.iter().filter(|s| !keep.contains(s)).collect();
    assert!(
        outside.is_empty(),
        "PACK_SLOTS must be a subset of SYS_KEEP, and {outside:?} is not in it. \
         A writable slot outside SYS_KEEP is a slot stage 5 of this same script \
         may cut whenever it is over `curate_slot_chars`: the pack would be \
         accepted, acked with error_code \"\", upserted into the brain's cell.db \
         — and then curated away behind the sender's back on the next assembly. \
         A pack silently truncated between two turns is the one failure this \
         lane's all-or-nothing promise exists to rule out. \
         PACK_SLOTS = {pack:?}, SYS_KEEP = {keep:?}"
    );

    // And the subset is the one the issue names, not an accidentally smaller
    // one: a PACK_SLOTS that shrank to nothing would satisfy the line above.
    let mut named = pack.clone();
    named.sort();
    let mut want: Vec<String> = THE_WRITABLE.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        named, want,
        "the lane writes exactly the four durable families the pack door \
         carries; read out of the shipped script: {pack:?}"
    );
}

/// Claim 2. The subset is PROPER, and it is proper in the two places that
/// carry a reason — the two families this cell re-derives every round.
///
/// It used to be proper in a third place, `instructions`, and GH #488 removed
/// that subtraction. The reason it existed — an identity that could overwrite
/// the charter could rewrite what the agent is for — assumed the charter had
/// another owner; it had none, which is why the third assertion below is now
/// the opposite of what it was: the charter MUST be writable, or a rebuilt
/// agent has no way of being told how it answers.
#[test]
fn the_two_families_the_collector_derives_are_not_writable() {
    let src = assemble_script();
    let pack = tuple_constant(&src, "PACK_SLOTS");
    let keep = tuple_constant(&src, "SYS_KEEP");

    for derived in ["tools", "budget"] {
        assert!(
            keep.contains(&derived.to_string()),
            "{derived} is a protected family of this cell's own; SYS_KEEP = {keep:?}"
        );
        assert!(
            !pack.contains(&derived.to_string()),
            "`{derived}` must NOT be writable over in_pack: the collector \
             re-derives it on every assembly (stage 4 for `tools`, this cell's \
             own budget sentence for `budget`), so a sender writing it would be \
             overwritten every round and would fight the cell for the same slot \
             path forever. PACK_SLOTS = {pack:?}"
        );
    }
    assert!(
        keep.contains(&"instructions".to_string()),
        "`instructions` is a protected family; SYS_KEEP = {keep:?}"
    );
    assert!(
        pack.contains(&"instructions".to_string()),
        "`instructions` must be writable over in_pack since GH #488: it is the \
         agent's own charter, it had no other owner, nothing exported it and no \
         template seeded it — so a family nobody may write was not a protected \
         family, it was an empty one. What protects it now is the door itself: \
         a route stamped by an edge that only the access rule for a brain's own \
         push edge draws. PACK_SLOTS = {pack:?}"
    );
}

/// Claim 3. The prose names the same families. A constant and a README that
/// disagree are worse than either alone — an operator reads the README and the
/// lane refuses what it promised.
#[test]
fn the_prose_names_the_same_families() {
    let src = assemble_script();
    let pack = tuple_constant(&src, "PACK_SLOTS");

    // The two agent READMEs, each at the sentence that states the closed list.
    // The wordings differ (talky states the lane in full, cogny states it in
    // one sentence and points at talky), so the anchor is the phrase both
    // sentences are built around rather than either sentence verbatim.
    for readme in ["talky/README.md", "cogny/README.md"] {
        let text = read_file(readme);
        let at = text.find("closed list").unwrap_or_else(|| {
            panic!(
                "{readme}: the sentence this drift lock reads (the \"closed list\" of \
                 writable `in_pack` slots) has been reworded away. \
                 `docs/development-rules.md` § 2d: a documented promise is pinned by a \
                 test, and the pin has to be repaired in the same change that reworded \
                 it -- not deleted."
            )
        });
        let sentence = &text[at..(at + 300).min(text.len())];
        for slot in &pack {
            assert!(
                sentence.contains(&format!("`{slot}`")),
                "{readme} must name `{slot}` where it states the closed list, because the \
                 shipped PACK_SLOTS does: {sentence:?}"
            );
        }
    }

    // And the machine-readable half: the collector's own `in_pack` accept term.
    let cfg = read_file("collector/config.json");
    let v: Value = meclaw_core::serde_json::from_str(&cfg).expect("collector config is JSON");
    let because = v["params"]["contract"]["accepts"]
        .as_array()
        .expect("the collector declares accepts")
        .iter()
        .find(|a| a["route"] == "in_pack")
        .unwrap_or_else(|| {
            panic!(
                "collector/config.json must still declare the `in_pack` lane. \
                 § 2d: the lane and its prose are one change."
            )
        })["because"]
        .as_str()
        .expect("an accept term carries a `because`")
        .to_string();
    for slot in &pack {
        assert!(
            because.contains(&format!("`{slot}`")),
            "collector/config.json's `in_pack` because must name `{slot}`, because the \
             shipped PACK_SLOTS does: {because:?}"
        );
    }
    assert!(
        because.contains("SUBSET") || because.contains("subset"),
        "the `in_pack` because must still state the subset relation this file locks — \
         it is the reason the three are safe from the curator: {because:?}"
    );
}
