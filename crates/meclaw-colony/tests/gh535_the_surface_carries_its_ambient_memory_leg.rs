//! GH #535 — a grown generation must go into a turn with its memory already
//! read, not only with a tool it may decide to call.
//!
//! `collector@`'s `memory_tier` ships EMPTY and `talky` leaves it that way. That
//! is right standalone: a `talky` on its own has no memory hive beside it, so a
//! per-turn recall leg would leave on `recall` and die at an address nobody
//! wired. What it does ship on is the TOOL — `memory_call_tier` defaults to
//! `"1"` and the composite routes `memory_recall` to its own collector since
//! `talky@4.2.1`.
//!
//! The `assistant` level is the other case and it is the only place that can
//! know it: it is instantiated into a member that HAS a hive, and the member's
//! own edges already carry `recall` up and `in_bundle` back down. Without the
//! override, a generation gets its memory only when the model decides to ask for
//! it — the conversation surface, the half whose whole job is to answer fast,
//! goes into a turn with the window and nothing else, and pays a second round
//! trip for what a cheap leg would have put in front of the first call.
//!
//! So the knob sits on the ref marker, beside the two overrides that are there
//! for exactly the same reason and no other: `brain.model` (#516) and
//! `collector/assemble.tools` (#529). Both say the same kind of thing — a fact
//! about the COMPOSITION that the referenced template cannot know. *Is there a
//! memory to read from* is the third.
//!
//! Three assertions, and each one alone would be a false green:
//!
//! - `the_surface_renders_with_its_ambient_memory_leg` is the shipped half, on
//!   the real tree through the real registry: the staged surface collector
//!   carries `memory_tier == "1"`.
//! - `the_reasoning_core_keeps_no_ambient_leg` pins the other side of the split.
//!   A problem solver asks about a time range or a session on purpose and is not
//!   handed a bundle before it has read the question (`cogny@4.4.0`, #528); an
//!   override that turned BOTH on would pass the first assertion and undo that.
//! - `the_ambient_leg_does_not_replace_the_memory_tool` keeps the tool where it
//!   is. The two legs answer different questions and #535 adds one rather than
//!   swapping it.
//!
//! Nothing here touches `talky` or `cogny`: both stand standalone elsewhere and
//! keep the knob empty.

use meclaw_colony::mutation::subtree::{StagedSubtree, SubtreeOverrides, stage_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// A registry snapshot of the templates under `dir` — built by the scanner a
/// booted colony uses, because a `cell.type: "ref"` resolves against exactly
/// that and a hand-rolled snapshot would be free to disagree with it.
fn registry_of(dir: &Path) -> TemplatesRegistry {
    let scanned = scan_templates_dir(dir).unwrap_or_else(|e| panic!("scan {}: {e}", dir.display()));
    assert!(!scanned.is_empty(), "{} scanned to nothing", dir.display());
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

/// Stage the shipped `assistant` under the logical name `gen`, through the real
/// staging path. No model is called and nothing boots: this is the DISK view a
/// boot re-reads.
fn staged_assistant() -> (tempfile::TempDir, StagedSubtree) {
    let registry = registry_of(&repo("templates"));
    let root = tempfile::tempdir().expect("tempdir");
    // Both shipped brains carry a defaultless `${OPENROUTER_API_KEY}` and the
    // runtime view of a staged config has to resolve, so the env map cannot be
    // empty. The value never reaches disk (GH #20).
    let env: HashMap<String, String> = [(
        "OPENROUTER_API_KEY".to_string(),
        "test-placeholder".to_string(),
    )]
    .into_iter()
    .collect();
    let ctx: HashMap<String, String> = [
        ("model".to_string(), "the-core-model".to_string()),
        ("model_surface".to_string(), "the-surface-model".to_string()),
    ]
    .into_iter()
    .collect();
    let staged = stage_subtree(
        root.path(),
        "m-gh535",
        "/main",
        "gen",
        &repo("templates/assistant"),
        &env,
        &ctx,
        None,
        &SubtreeOverrides::default(),
        &registry,
        &Default::default(),
        &meclaw_colony::WorkPulse::silent(),
        meclaw_colony::mutation::Birth::Active,
    )
    .unwrap_or_else(|e| panic!("stage the assistant: {e:?}"));
    (root, staged)
}

/// One staged cell's `params` — the disk view again, addressed the way the
/// substrate addresses it.
fn staged_params(staged: &StagedSubtree, absolute_path: &str) -> Value {
    let cell = staged
        .cells
        .iter()
        .find(|c| c.absolute_path.as_str() == absolute_path)
        .unwrap_or_else(|| {
            let known: Vec<&str> = staged
                .cells
                .iter()
                .map(|c| c.absolute_path.as_str())
                .collect();
            panic!("no staged cell at {absolute_path:?}; staged: {known:?}")
        });
    cell.params.clone()
}

/// The defect: the surface used to render with the collector's empty default,
/// so no `recall` left before the brain call and the bundle the member is wired
/// to answer with was never asked for.
#[test]
fn the_surface_renders_with_its_ambient_memory_leg() {
    let (_root, staged) = staged_assistant();
    assert_eq!(
        staged_params(&staged, "/main/gen/talky/collector/assemble")["memory_tier"],
        "1",
        "the conversation surface reads the member's memory BEFORE it calls its \
         brain — `talky` ships the knob empty because standalone it has no hive \
         beside it, and this level is the one that knows it has one"
    );
}

/// The other side of the split, and the reason the override is on ONE ref marker
/// rather than in the level's ctx: the reasoning core is a problem solver and is
/// not handed a bundle before it has read the question (`cogny@4.4.0`, #528).
#[test]
fn the_reasoning_core_keeps_no_ambient_leg() {
    let (_root, staged) = staged_assistant();
    let tier = staged_params(&staged, "/main/gen/cogny/collector/assemble")["memory_tier"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        tier.trim().is_empty(),
        "the core asks about a time range or a session on purpose; an ambient \
         bundle in front of every consult is the split undone (got {tier:?})"
    );
}

/// The ambient leg is an addition, never a swap: the two legs answer different
/// questions. `memory_recall` stays the deliberate lookup a model asks for by
/// name, and the collector goes on serving it itself (#512, `talky@4.2.1`).
#[test]
fn the_ambient_leg_does_not_replace_the_memory_tool() {
    let (_root, staged) = staged_assistant();
    let params = staged_params(&staged, "/main/gen/talky/collector/assemble");
    assert_eq!(
        params["memory_call_tier"], "1",
        "the memory TOOL is still on — a switch left empty is a lane that \
         answers a typed refusal, and the charter gate (#531) would then find a \
         menu naming a tool nothing serves"
    );
}

/// The knob is the LEVEL's, and it is stated where the other two of its kind
/// are. A test on the staged tree alone would pass just as well if somebody put
/// the value in `talky`'s own collector, which is the drift #516 and #529 both
/// spent a paragraph refusing.
#[test]
fn the_knob_stands_on_the_ref_marker_and_talky_keeps_it_empty() {
    let surface: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/assistant/talky/config.json"))
            .expect("read the surface ref marker"),
    )
    .expect("parse the surface ref marker");
    assert_eq!(
        surface["override_params"]["collector/assemble"]["memory_tier"], "1",
        "the level says it, on the same marker that names the surface's model \
         (#516) and its declared tool list (#529)"
    );

    let talky: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/talky/collector/config.json"))
            .expect("read talky's collector marker"),
    )
    .expect("parse talky's collector marker");
    let own = &talky["override_params"]["assemble"]["memory_tier"];
    assert!(
        own.is_null() || own.as_str().is_some_and(|s| s.trim().is_empty()),
        "standalone `talky` has no hive beside it and keeps the knob empty — \
         moving the value down there is the drift that would make every \
         standalone talky emit a `recall` nobody wired (got {own:?})"
    );
}

/// The half the knob revealed, measured on a running colony before it was
/// fixed: turning `memory_tier` on made `recall` leave, `in_query` arrive and
/// the bundle come back — and the collector answered it with silence, because
///
/// ```text
/// if lane == "in_bundle":
///     if not turn_id:
///         park()
/// ```
///
/// and `turn_id` rode only on the HOP. A hive forms its own hop (GH #411);
/// context is the one compartment that survives one, so the promotion has to
/// happen on the member's own `recall` edge. `talky`'s contract has named
/// `turn_id` among that exit's keys all along — `member` promoted seven of them
/// and not that one.
///
/// It never showed on the TOOL path, which is why it shipped: a `memory_recall`
/// call happens AFTER the brain call, and the brain edge has promoted
/// `hop.turn_id` into context since long before. The ambient leg leaves BEFORE
/// the model has seen the turn, so there is no context copy yet.
#[test]
fn the_members_recall_edge_promotes_the_turn_id() {
    let member: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/member/config.json"))
            .expect("read the member graph"),
    )
    .expect("parse the member graph");
    let edges = member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member declares edges");
    let recall = edges
        .iter()
        .find(|e| {
            e["from"] == "./assistants"
                && e["to"] == "./memory-hive"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'recall'"))
        })
        .expect("the member carries `recall` up to its hive");
    let set = &recall["modifier"]["set_context"];
    assert_eq!(
        set["turn_id"], "has(hop.turn_id) ? hop.turn_id : ''",
        "without it the ambient bundle comes home unable to name the round it \
         belongs to, and the collector parks the whole turn in silence"
    );
    // The keys that were already right stay right — this is one added key, not
    // a rewritten modifier.
    assert_eq!(
        set["audience_now"],
        "has(context.audience_set) ? context.audience_set : ''"
    );
    assert_eq!(
        set["memory_call_id"],
        "has(hop.memory_call_id) ? hop.memory_call_id : ''"
    );
}
