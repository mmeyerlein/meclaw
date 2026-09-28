//! GH #458 — the closed list of writable pack families, and the prose says so too.
//!
//! The `in_pack` lane writes durable `system.*` state for an agent's brain.
//! What it may write is a closed list — `identity`, `persona`, `handover`,
//! `instructions` — and a slot outside it refuses the WHOLE pack.
//!
//! Until GH #889 that list was the tuple constant `PACK_SLOTS` of
//! `collector/assemble`, and this file locked it as a SUBSET of the same
//! script's `SYS_KEEP`: stage 5 of the collector's curation cut any `system.*`
//! family over `curate_slot_chars` that was not in `SYS_KEEP`, so a writable
//! slot outside it would have been acked with `error_code: ""` and then curated
//! away behind the sender's back on the next assembly.
//!
//! GH #889 (`collector@5.0.0`) took the lane, both constants and every curation
//! stage out of the collector (R-27-1: the collector collects and hands on,
//! nothing else). The pack enters `./curator` now, which holds it in its ledger
//! (`slots`, owner `pack`) and hands it to the brain with the next call. The
//! half of this lock that was about the curator's reach — a written pack is
//! never cut — is the curator's own promise and is pinned where it lives
//! (`curator@1.0.0`, plan K § 2 "Pack": `an_unknown_family_is_slot_unknown`,
//! `a_valid_pack_is_acked_empty_and_reaches_the_next_call`).
//!
//! What stays here is the half a CALLER reads: the two rims that take the lane,
//! `talky` and `cogny`, state the closed list in their own `in_pack` accept
//! term, and their READMEs state the same list. Both are read out of the
//! shipped artefacts (`docs/development-rules.md` § 2d); the list below is only
//! what the failure messages talk about.
//!
//! No colony: this file is about one sentence per rim and two READMEs.

use meclaw_core::serde_json::Value;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read_file(rel: &str) -> String {
    let p = templates_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?} must be readable: {e}"))
}

/// The rims that take the `in_pack` lane.
const RIMS: [&str; 2] = ["talky", "cogny"];

/// The closed list as a rim states it: the backticked names of the sentence
/// "Only `…` may be written." in the rim's own `in_pack` accept term.
///
/// GH #889 moved the list's machine-readable home here: `PACK_SLOTS` left
/// `collector/assemble` with the lane, and the curator that enforces the list
/// publishes no constant a caller could read. Reading the sentence out of the
/// artefact is the point — a copy of the list in this file could agree with
/// itself while disagreeing with what ships.
fn rim_writable(composite: &str) -> Vec<String> {
    let rel = format!("{composite}/config.json");
    let v: Value = meclaw_core::serde_json::from_str(&read_file(&rel))
        .unwrap_or_else(|e| panic!("{rel} must be JSON: {e}"));
    let because = v["params"]["contract"]["accepts"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel} must declare accepts"))
        .iter()
        .find(|a| a["route"] == "in_pack")
        .unwrap_or_else(|| {
            panic!(
                "{rel} must still declare the `in_pack` lane. § 2d: the lane and its \
                 prose are one change."
            )
        })["because"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel}: an accept term carries a `because`"))
        .to_string();
    let at = because.find("Only `").unwrap_or_else(|| {
        panic!(
            "{rel}: the `in_pack` because no longer states the closed list as \
             \"Only `…` may be written.\" -- repair the pin in the same change that \
             reworded it (`docs/development-rules.md` § 2d): {because:?}"
        )
    });
    let rest = &because[at..];
    let end = rest.find(" may be written").unwrap_or_else(|| {
        panic!("{rel}: the closed-list sentence must end in \"may be written\": {rest:?}")
    });
    let names: Vec<String> = rest[..end]
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(
        !names.is_empty(),
        "{rel}: the closed list must name at least one family: {rest:?}"
    );
    names
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

/// Claim 1. Each rim names exactly the four durable families the pack door
/// carries.
///
/// GH #889: this was `every_pack_slot_is_a_protected_family`, which read
/// `PACK_SLOTS ⊆ SYS_KEEP` out of `collector/assemble`. Both constants left
/// with the lane; the subset half (the curator never cuts a pack slot) is the
/// curator's promise now, and the exact-list half is read at the rims.
#[test]
fn each_rim_names_exactly_the_four_writable_families() {
    let mut want: Vec<String> = THE_WRITABLE.iter().map(|s| s.to_string()).collect();
    want.sort();
    for rim in RIMS {
        let mut named = rim_writable(rim);
        named.sort();
        assert_eq!(
            named, want,
            "{rim}: the lane writes exactly the four durable families the pack door \
             carries — a shorter list refuses a pack the affinity renders, a longer \
             one opens a family nobody audited; read out of {rim}/config.json"
        );
    }
}

/// Claim 2. The families the collector re-derives every round are not
/// writable, and the charter is.
///
/// GH #889: the derived families are the ones `collector@5.0.0` writes itself —
/// `tools` on its menu lane, `consult` and `roster` on every `curate` — so a
/// sender writing them would fight the collector for the same slot path
/// forever. `budget` was the second derived family until R-27-1 took the
/// window out of the collector (it no longer looks after the context window);
/// `system.budget` is gone with it and has nothing left to protect.
///
/// `instructions` used to be a third subtraction, and GH #488 removed it: the
/// reason it existed — an identity that could overwrite the charter could
/// rewrite what the agent is for — assumed the charter had another owner; it
/// had none, which is why the last assertion is the opposite of what it was.
#[test]
fn the_families_the_collector_derives_are_not_writable() {
    for rim in RIMS {
        let pack = rim_writable(rim);
        for derived in ["tools", "consult", "roster"] {
            assert!(
                !pack.contains(&derived.to_string()),
                "{rim}: `{derived}` must NOT be writable over in_pack: the collector \
                 re-derives it every round, so a sender writing it would be \
                 overwritten every round and would fight the cell for the same slot \
                 path forever. Writable: {pack:?}"
            );
        }
        assert!(
            pack.contains(&"instructions".to_string()),
            "{rim}: `instructions` must be writable over in_pack since GH #488: it is \
             the agent's own charter, it had no other owner, nothing exported it and \
             no template seeded it — so a family nobody may write was not a \
             protected family, it was an empty one. What protects it now is the door \
             itself: a route stamped by an edge that only the access rule for a \
             brain's own push edge draws. Writable: {pack:?}"
        );
    }
}

/// Claim 3. The prose names the same families. A machine-readable list and a
/// README that disagree are worse than either alone — an operator reads the
/// README and the lane refuses what it promised.
///
/// GH #889: the machine-readable half was the collector's own `in_pack` accept
/// term, which left with the lane; the rims' accept terms are read by
/// `rim_writable` above. The accept term no longer states a subset relation:
/// `SYS_KEEP` is gone, and what keeps a pack out of reach is the curator's.
#[test]
fn the_prose_names_the_same_families() {
    // The two agent READMEs, each at the sentence that states the closed list.
    // The wordings differ (talky states the lane in full, cogny states it in
    // one sentence and points at talky), so the anchor is the phrase both
    // sentences are built around rather than either sentence verbatim.
    for rim in RIMS {
        let pack = rim_writable(rim);
        let readme = format!("{rim}/README.md");
        let text = read_file(&readme);
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
                "{readme} must name `{slot}` where it states the closed list, because \
                 {rim}/config.json's `in_pack` accept term does: {sentence:?}"
            );
        }
    }
}
