//! GH #494 — an interior round-trip marker does not leave a sealed hive.
//!
//! A hive has no `cell.db`. When one of its cells has to remember which round
//! trip an answer belongs to, the only place to put that memory is **context**,
//! promoted on one of the hive's own edges — `<name>_origin` plus a phase and a
//! carry is the shape the shipped tree converged on. And context is persistent
//! for the life of a chain: nothing removes those keys again. So unless the
//! exit edge clears them, they leave a sealed hive on the answer and ride on
//! the caller's next message, where the cell that reads the marker *before* it
//! reads the inbound lane dispatches a fresh request as this hive's own echo.
//!
//! That is GH #481 (`access`: the second capability question of a submission
//! was refused without the policy table ever being read) and GH #490 (`submit`:
//! a permitted manifest was reported lost while it lay parked). Both were
//! repaired with the same pair, and the second half of that pair is the rule
//! this file holds for the whole library:
//!
//! > **No context key a hive sets on one of its own edges survives an exit
//! > edge.** What a hive remembers about its own interior is the hive's; the
//! > rim is where it ends.
//!
//! Two named exemptions, and they are named rather than inferred:
//!
//! * [`SHARED`] — the context vocabulary the templates pass **between** hives on
//!   purpose. `session_id` is minted inside `talky` and read by `member` on the
//!   edge that carries a write to the memory hive; clearing it at `talky`'s rim
//!   would cut the memory off from the session it belongs to. `happened_at`
//!   joined it with GH #527 for the same reason one level over: the member
//!   promotes it off the hop onto the memory hive's `in_episode` door, and the
//!   writer reads it off the context as the EVENT half of its bi-temporal split
//!   — cleared, every replayed turn is stamped with the writer's own clock.
//!   `recall_caller` joined it with GH #533: the reply-to token of ADR-0019 is
//!   stamped by the ASKER (an assistant's surface or core) or by the member's own
//!   `in_recall` door for an asker outside, travels four levels untouched and is
//!   turned back into a hop key by the memory hive's exit — a rim that cleared it
//!   would send every bundle to the default door and lose the addressing the
//!   whole mechanism exists for.
//! * [`CARRIED`] — a hive's own key that a round trip **out** of the hive has to
//!   bring back, because the answer re-enters through a door that does not
//!   re-establish it. Three entries, each with the edge pair that needs it.
//!
//! An exemption is an assertion, not a mute: a key listed in either table that
//! no hive sets any more fails this test, so the lists cannot rot into prose.
//!
//! GH #499 removed the third list. Two templates — `builder` and
//! `builder-librarian` — were held by another strand while this sweep ran and
//! stood in a deferral table, which was the only reason the rule did not bite
//! them; both clear their interior keys now, and the table is gone with them.
//! That pass also moved one key into [`SHARED`]: `build_call_id` is set by
//! `templates/tools` on its own exit edge and read back off `context` by
//! `tools/build-draft` and `tools/build-apply` when a build result returns through the
//! `in_build_result` door — clearing it at the builder's rim would leave the
//! assistant's build tool call open forever.
//!
//! The measuring tool is `workshop/tools/hive_context_sweep.py`, which carries
//! the same two lists and can write the fix; this test is what keeps the tree
//! from drifting back. The rule is written down where a template author meets
//! it: `templates/README.md` § The hive boundary, authoring rule 5.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use meclaw_core::serde_json::{Value, from_str};

/// Context the templates pass between hives on purpose. Never cleared at a rim.
const SHARED: [&str; 22] = [
    "actor",
    "asker",
    "audience_now",
    "audience_set",
    "build_auto_submit",
    "build_call_id",
    "build_caller",
    "channel",
    "chat_id",
    "happened_at",
    "iter",
    "memory_call_id",
    "memory_tier",
    "recall_as_of",
    "recall_caller",
    "recall_query",
    "recall_window_from",
    "recall_window_to",
    "requester",
    "session_id",
    "subscriber",
    "turn_id",
];

/// `(template, key, why)` — a hive's own key that has to survive its own rim.
///
/// `cogny`/`consult_class` was the second entry and left with the lane that read
/// it ([#528](https://github.com/mmeyerlein/meclaw/issues/528)): the core has one
/// brain, so no edge inside it decides anything on a class and there is no key to
/// carry across the rim.
const CARRIED: [(&str, &str, &str); 3] = [
    (
        "assistant",
        "tool_caller",
        "the `build` round trip leaves on `./tools -> .` and comes back on the \
         `in_build_result` door, which does not re-establish it — and \
         `./tools -> ./talky` against `./tools -> ./cogny` is decided on it",
    ),
    (
        "assistant",
        "consult_id",
        "the reasoning core's memory leg leaves on `./cogny -> .` and the bundle \
         comes back through the `in_bundle` door, which does not re-establish it \
         — and the advice the core finally produces is filed by the surface's \
         collector under exactly this id (GH #532)",
    ),
    (
        "meclaw-os",
        "sub_ask",
        "the shell's own correlation of a capability question it put through \
         `./access`, read on the way back out of that hive",
    ),
];

fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn edges(config: &Value) -> Vec<&Value> {
    config["params"]["graph"]["edges"]
        .as_array()
        .map(|list| list.iter().collect())
        .unwrap_or_default()
}

fn set_context_keys(edge: &Value) -> Vec<String> {
    edge["modifier"]["set_context"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn deleted_keys(edge: &Value) -> BTreeSet<String> {
    edge["modifier"]["delete_context"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|key| key.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn describe(edge: &Value) -> String {
    let condition = edge["condition"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            if edge["default"].as_bool() == Some(true) {
                "<default>".to_string()
            } else {
                "<unconditional>".to_string()
            }
        });
    format!(
        "{} -> . [{condition}]",
        edge["from"].as_str().unwrap_or("?")
    )
}

/// Every shipped hive marker, by template name.
fn shipped_hives() -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    let root = templates_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&root)
        .expect("templates/ is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        let config = path.join("config.json");
        if !config.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&config).expect("a shipped config.json is readable");
        let value: Value = from_str(&raw).unwrap_or_else(|err| panic!("{config:?}: {err}"));
        if value["cell"]["type"].as_str() == Some("hive") {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a template directory has a name")
                .to_string();
            found.insert(name, value);
        }
    }
    found
}

/// The keys a hive sets on one of its OWN edges. An exit edge is not one of
/// them: what it sets, it sets on the way out, and that is the caller's.
fn interior_keys(config: &Value) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for edge in edges(config) {
        if edge["to"].as_str() == Some(".") {
            continue;
        }
        keys.extend(set_context_keys(edge));
    }
    keys
}

#[test]
fn no_interior_context_key_of_a_hive_survives_one_of_its_exit_edges() {
    let hives = shipped_hives();
    assert!(
        hives.len() >= 25,
        "the sweep must see the shipped library, not a fragment of it: {} hives",
        hives.len()
    );

    let shared: BTreeSet<&str> = SHARED.into_iter().collect();

    let mut leaks: Vec<String> = Vec::new();
    let mut edges_checked = 0usize;
    let mut keys_checked = 0usize;

    for (name, config) in &hives {
        let carried: BTreeSet<&str> = CARRIED
            .iter()
            .filter(|(template, ..)| template == name)
            .map(|(_, key, _)| *key)
            .collect();
        let must_clear: Vec<String> = interior_keys(config)
            .into_iter()
            .filter(|key| !shared.contains(key.as_str()) && !carried.contains(key.as_str()))
            .collect();
        if must_clear.is_empty() {
            continue;
        }
        keys_checked += must_clear.len();
        for edge in edges(config) {
            if edge["to"].as_str() != Some(".") {
                continue;
            }
            edges_checked += 1;
            let cleared = deleted_keys(edge);
            let missing: Vec<&str> = must_clear
                .iter()
                .map(String::as_str)
                .filter(|key| !cleared.contains(*key))
                .collect();
            if !missing.is_empty() {
                leaks.push(format!(
                    "{name}: {} lets {} out",
                    describe(edge),
                    missing.join(", ")
                ));
            }
        }
    }

    // The floor catches a DEGENERATE sweep -- a refactor that leaves
    // `shipped_hives()` empty, or an `interior_keys()` that stops recognising
    // the shape -- and it has to be a number BOTH trees clear. The published
    // subset carries 38 of the 44 templates (`PUBLIC_TEMPLATES`), so a floor
    // read off the private tree is a floor the public CI cannot meet: measured
    // 2026-08-30, the published tree sweeps 99 exit edges over 98 keys while
    // the private one is at or above 98/99, and a floor of 99 keys turned the
    // 0.28.0 release CI red for a reason that was not the code's. Same class,
    // same remedy as the count floor in `gh80_shipped_conditions_are_guarded`.
    assert!(
        edges_checked >= 80 && keys_checked >= 80,
        "the sweep degenerated: {edges_checked} exit edges, {keys_checked} keys"
    );
    assert!(
        leaks.is_empty(),
        "an interior context marker leaves a sealed hive — clear it with \
         `delete_context` on the exit edge, or name it in SHARED / CARRIED with \
         the reason (GH #494):\n  {}",
        leaks.join("\n  ")
    );
}

#[test]
fn every_named_exemption_still_names_a_key_a_hive_sets() {
    let hives = shipped_hives();
    let mut all_interior: BTreeSet<String> = BTreeSet::new();
    for config in hives.values() {
        all_interior.extend(interior_keys(config));
    }

    let stale: Vec<&str> = SHARED
        .into_iter()
        .filter(|key| !all_interior.contains(*key))
        .collect();
    assert!(
        stale.is_empty(),
        "SHARED names context no hive sets any more, so the exemption is prose: {stale:?}"
    );

    for (template, key, why) in CARRIED {
        let config = hives
            .get(template)
            .unwrap_or_else(|| panic!("CARRIED names `{template}`, which is not a shipped hive"));
        assert!(
            interior_keys(config).contains(key),
            "CARRIED says `{template}` carries `{key}` ({why}), but the hive no \
             longer sets it — drop the entry"
        );
    }
}
