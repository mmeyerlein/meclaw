//! GH #937 — a `delete_context` entry `"<p>*"` clears every working key of a
//! hive that starts with `<p>`, and nothing else.
//!
//! Before #937 a hive that parked its working keys in the context had to name
//! every one of them on its exit edge, and every new key had to be added to a
//! canonical list per hive — a key someone forgot leaked into the next level.
//! The prefix entry is the narrowest form that ends that (ruling OR-S3-16):
//! at least one character, the `*` once and last. `"*"` would clear the whole
//! context on one edge — the round, the session, the channel — and `"a*b"`
//! reads like a glob nobody implements, so the mutation door refuses both.
//!
//! Three halves, one rule:
//! - **the delete** (`apply_modifier`): exact entries as before, a prefix entry
//!   every key it starts, never a key it does not start;
//! - **the form** (mutation door, `EdgeSchema`): `"*"`, `"a*b"`, `"**"`, `"*a"`
//!   refused, `"c_*"` accepted;
//! - **the locality check**: a prefix entry severs a key it covers exactly as an
//!   exact entry does — the check is never looser than the delete;
//! - **the round** (review of the strand, Important 3): a prefix that covers a
//!   key of the round (`ROUND_CONTEXT_KEYS`: `audience_set`, `audience_now`) is
//!   refused at the door. An exact entry naming one stays legal, because it is
//!   visible in the manifest; `"a*"` deleting the round without the word
//!   `audience` anywhere in it is not.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::cel_eval::{
    ROUND_CONTEXT_KEYS, apply_modifier, delete_entry_covers_round, delete_entry_matches,
    delete_entry_prefix, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_colony::mutation::MutationError;
use meclaw_colony::mutation::validate::{
    HeaderEdgeView, HeaderNodeView, validate_header_contract_locality,
    validate_post_state_with_edges,
};
use meclaw_core::Headers;
use meclaw_core::serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn headers_with(keys: &[&str]) -> Headers {
    let mut h = Headers::new();
    for k in keys {
        h.context
            .insert((*k).to_string(), Value::String(format!("v-{k}")));
    }
    h
}

fn deleted(entries: &[&str], keys: &[&str]) -> Vec<String> {
    let spec = ModifierSpec {
        delete_context: entries.iter().map(|s| (*s).to_string()).collect(),
        ..ModifierSpec::default()
    };
    let m = parse_modifier(&spec).expect("a delete-only modifier compiles");
    let out = apply_modifier(&m, &headers_with(keys)).expect("apply");
    let mut left: Vec<String> = out.context.keys().cloned().collect();
    left.sort();
    left
}

/// **The delete.** Exact, prefix, both, and a prefix that hits nothing.
#[test]
fn a_prefix_clears_every_key_it_starts_and_no_other() {
    let ctx = [
        "c_phase",
        "c_body",
        "c_",
        "cx",
        "audience_set",
        "session_id",
    ];
    for (entries, left) in [
        // exact, as before #937
        (
            vec!["c_phase"],
            vec!["audience_set", "c_", "c_body", "cx", "session_id"],
        ),
        // prefix: every `c_` key, the bare `c_` too, never `cx`
        (vec!["c_*"], vec!["audience_set", "cx", "session_id"]),
        // both in one list
        (vec!["c_*", "session_id"], vec!["audience_set", "cx"]),
        // a prefix nobody starts with clears nothing
        (
            vec!["zz_*"],
            vec![
                "audience_set",
                "c_",
                "c_body",
                "c_phase",
                "cx",
                "session_id",
            ],
        ),
    ] {
        assert_eq!(
            deleted(&entries, &ctx),
            left,
            "delete_context {entries:?} over {ctx:?}"
        );
    }
}

/// The round travels unchanged: a prefix that does not start `audience_set`
/// never touches it, and a key this very edge promotes under the prefix is gone
/// too — set runs before delete (spec § Edge-Modell), exactly as for an exact
/// entry naming it.
#[test]
fn a_prefix_runs_after_the_sets_of_its_edge() {
    let spec = ModifierSpec {
        set_context: BTreeMap::from([
            ("c_fresh".to_string(), "'x'".to_string()),
            ("keep".to_string(), "'y'".to_string()),
        ]),
        delete_context: vec!["c_*".to_string()],
        ..ModifierSpec::default()
    };
    let m = parse_modifier(&spec).expect("compiles");
    let out = apply_modifier(&m, &headers_with(&["c_old", "audience_set"])).expect("apply");
    let mut left: Vec<&str> = out.context.keys().map(String::as_str).collect();
    left.sort_unstable();
    assert_eq!(left, vec!["audience_set", "keep"]);
    assert_eq!(
        out.context.get("audience_set"),
        Some(&Value::String("v-audience_set".into())),
        "the round is carried verbatim"
    );
}

/// The one reading of an entry, as a table.
#[test]
fn only_p_star_is_a_prefix() {
    for (entry, prefix) in [
        ("c_*", Some("c_")),
        ("a*", Some("a")),
        ("*", None),
        ("**", None),
        ("a*b", None),
        ("*a", None),
        ("a**", None),
        ("plain", None),
    ] {
        assert_eq!(delete_entry_prefix(entry), prefix, "entry {entry:?}");
    }
    assert!(delete_entry_matches("c_*", "c_phase"));
    assert!(!delete_entry_matches("c_*", "cx"));
    assert!(delete_entry_matches("plain", "plain"));
    assert!(!delete_entry_matches("plain", "plainer"));
    // a malformed entry is never a pattern: it matches only itself
    assert!(!delete_entry_matches("*", "anything"));
}

fn door(entry: &str) -> Result<(), MutationError> {
    let diff = json!({"add_edges": [{
        "from": "a", "to": "b",
        "modifier": {"delete_context": [entry]}
    }]});
    validate_post_state_with_edges(
        &diff,
        &CellFactoryRegistry::new(),
        &["a".to_string(), "b".to_string()],
        &[],
        &[],
    )
}

/// **The form.** The door refuses every `*` entry that is not `"<p>*"`, by name.
#[test]
fn the_door_refuses_every_star_that_is_not_a_prefix() {
    for bad in ["*", "a*b", "**", "*a", "a**"] {
        match door(bad) {
            Err(MutationError::EdgeSchema(msg)) => assert!(
                msg.contains("delete_context") && msg.contains(bad),
                "the refusal names the field and the entry: {msg}"
            ),
            other => panic!("{bad:?} must be refused as edge_schema, got {other:?}"),
        }
    }
    for good in ["b*", "c_*", "plain"] {
        assert!(door(good).is_ok(), "{good:?} is a key or a prefix");
    }
}

/// **The round.** A prefix that starts a key of the round would delete who may
/// hear the turn without naming it; the door refuses it, by entry and by key.
/// The exact key stays legal — it is what the manifest then says out loud.
#[test]
fn the_door_refuses_a_prefix_that_covers_the_round() {
    assert_eq!(ROUND_CONTEXT_KEYS, ["audience_set", "audience_now"]);
    assert_eq!(delete_entry_covers_round("au*"), Some("audience_set"));
    assert_eq!(
        delete_entry_covers_round("audience_n*"),
        Some("audience_now")
    );
    assert_eq!(
        delete_entry_covers_round("audience_set"),
        None,
        "exact is not a prefix"
    );
    assert_eq!(delete_entry_covers_round("c_*"), None);
    for bad in ["a*", "au*", "audience_*", "audience_s*", "audience_now*"] {
        match door(bad) {
            Err(MutationError::EdgeSchema(msg)) => assert!(
                msg.contains("delete_context") && msg.contains(bad) && msg.contains("audience_"),
                "the refusal names the field, the entry and the round key: {msg}"
            ),
            other => panic!("{bad:?} covers a round key and must be refused, got {other:?}"),
        }
    }
    for good in [
        "audience_set",
        "audience_now",
        "aux*",
        "audience_setx*",
        "b*",
    ] {
        assert!(
            door(good).is_ok(),
            "{good:?} names the round exactly or covers none of it"
        );
    }
}

/// **The locality check.** `b` requires `c_phase`; `x -> a` promotes it and
/// `a -> b` deletes. A prefix covering the key severs the path exactly as the
/// exact entry does; a prefix that does not cover it leaves the path whole.
#[test]
fn the_locality_check_sees_what_the_prefix_deletes() {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "b".to_string(),
        HeaderNodeView {
            required_context: BTreeSet::from(["c_phase".to_string()]),
            ..HeaderNodeView::default()
        },
    );
    let graph = |del: &str| {
        vec![
            HeaderEdgeView {
                from: "x".into(),
                to: "a".into(),
                set_context: BTreeSet::from(["c_phase".to_string()]),
                ..HeaderEdgeView::default()
            },
            HeaderEdgeView {
                from: "a".into(),
                to: "b".into(),
                delete_context: BTreeSet::from([del.to_string()]),
                ..HeaderEdgeView::default()
            },
        ]
    };
    let hives = BTreeSet::new();
    for (del, severed) in [
        ("c_phase", true),
        ("c_*", true),
        ("c_p*", true),
        ("d_*", false),
        ("c_phasex", false),
    ] {
        let verdict = validate_header_contract_locality(&nodes, &graph(del), &hives);
        assert_eq!(
            verdict.is_err(),
            severed,
            "delete_context [{del:?}] on a -> b: severed={severed}, got {verdict:?}"
        );
    }
}
