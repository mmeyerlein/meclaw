//! GH #496 — the corpus of a RUNNING colony is a snapshot, and `in_ingest` is
//! how it stops being one.
//!
//! `store/seed/docs.jsonl` is loaded once, at `OpenStatus::Created`, and never
//! again. So a running librarian answers *what templates exist* about the
//! library of the moment it was born. A class registered since — by an
//! `add_templates` (GH #440), by a directory dropped into `templates/` plus a
//! rescan — is resolvable at the mutation door and does not exist for the
//! composer. Measured: a design-lane run spent seven rounds and its whole
//! budget looking for a `clock` that had been in the library for an hour, and
//! ended with no manifest.
//!
//! Two drifts wear the same face and only ONE of them is this file's subject.
//! The committed corpus against the tree is a build-product gate and it already
//! stands (`librarian_seed_corpus.rs`, `gh466_the_seed_gate_is_wired.rs`,
//! a2 block 1, export rule R11). No gate can reach the other one: the corpus
//! was correct when the colony booted and the library moved afterwards.
//!
//! What is pinned here:
//!
//! 1. the nudge asks `/colony/templates` — a header, an EMPTY `messages[]` and
//!    the filter under `query`, which is how a `code` cell asks a colony
//!    endpoint;
//! 2. the registry answer is carried across the store round trip in
//!    `hop.cat_seen`, because a colony reply comes back with an empty context
//!    and this endpoint echoes no tag;
//! 3. only names the corpus does NOT hold are written — the diff is what makes
//!    a second reconciliation a no-op, in a table that has no key;
//! 4. the row it writes says what it cannot know instead of inventing a
//!    `requires` block;
//! 5. a refused read is not an empty registry, and a refused write is named;
//! 6. the lane the hive declares is the lane the cell speaks, and the read it
//!    draws is one the substrate permits — without that entry the template dies
//!    at growth rather than at runtime;
//! 7. the drift lock of `docs/development-rules.md` § 2d: the counted promise is
//!    grepped in the README AND driven through the mechanism.
//!
//! **R2b guard.** Every read is guarded by [`shipped`]: where the template does
//! not ship, these tests skip rather than fail on a dead reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder-librarian")
}

/// The files this suite reads. The list is the guard AND the inventory.
const FILES: &[&str] = &["catalogue/config.json", "config.json", "README.md"];

fn shipped() -> Option<PathBuf> {
    let r = root();
    FILES.iter().all(|f| r.join(f).exists()).then_some(r)
}

fn script(r: &Path) -> String {
    shipped_script(r.join("catalogue/config.json").to_str().expect("path"))
}

fn config(r: &Path, rel: &str) -> Value {
    let raw = std::fs::read_to_string(r.join(rel)).expect("config readable");
    meclaw_core::serde_json::from_str(&raw).expect("config parses")
}

/// One store op, taken off the turn whose id is `slot`.
fn op_of(msg: &Value, slot: &str) -> Value {
    let turn = msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|t| t["id"] == slot)
        .unwrap_or_else(|| panic!("no turn keyed {slot} in {msg}"));
    meclaw_core::serde_json::from_str(turn["text"].as_str().expect("op travels in text"))
        .expect("the op is JSON")
}

/// Phase 1 — the nudge.
fn nudged(r: &Path) -> Value {
    emit_one(
        &script(r),
        &json!({"header": {"hop": {"route": "in_ingest"}, "context": {}},
                "params": {}, "messages": []}),
    )
}

/// Phase 2 — the registry answered with these entries.
fn registry_answered(r: &Path, entries: Value) -> Value {
    emit_one(
        &script(r),
        &json!({"header": {"hop": {}, "context": {}}, "params": {},
                "templates": entries}),
    )
}

/// Phase 3 — the corpus answered with these names, having seen `cat_seen`.
fn corpus_answered(r: &Path, cat_seen: &str, names: &[&str]) -> Value {
    let rows: Vec<Value> = names.iter().map(|n| json!({"section": n})).collect();
    emit_one(
        &script(r),
        &json!({
            "header": {"hop": {"operation": "select"},
                       "context": {"cat_seen": cat_seen, "cat_added": "", "cat_note": ""}},
            "params": {},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "cat-names",
                          "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}],
        }),
    )
}

#[test]
fn the_nudge_asks_the_registry_and_nothing_else() {
    let Some(r) = shipped() else { return };
    let out = nudged(&r);

    assert_eq!(out["header"]["route"], "cat_read");
    assert_eq!(
        out["messages"].as_array().map(Vec::len),
        Some(0),
        "a colony endpoint is asked with an EMPTY messages[] and the question under \
         `query` — a UBF body needs one of system/messages/attachments and an empty \
         list is a valid one (cookbook colony-endpoint-roundtrip, rule 1)"
    );
    assert!(
        out["query"]["limit"].is_number(),
        "the read carries its own limit, so the answer's completeness is a number this \
         cell can check rather than assume: {out}"
    );
}

#[test]
fn the_registry_answer_survives_the_store_round_trip_on_the_hop() {
    let Some(r) = shipped() else { return };
    let out = registry_answered(
        &r,
        json!([{"name": "foo", "version": "1.0.0", "filesystem_path": "/x/templates/local/foo"}]),
    );

    assert_eq!(out["header"]["route"], "cat_store");
    let op = op_of(&out, "cat-names");
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "docs");
    assert_eq!(op["where"]["kind"], "template");

    assert_eq!(
        out["header"]["cat_seen"], "foo@1.0.0",
        "a colony reply arrives with an EMPTY context and /colony/templates echoes no \
         tag of its own, so the registry answer has to travel on the hop the hive's \
         edge promotes — nothing else of this round survives: {out}"
    );
    assert!(
        !out["header"]["cat_seen"]
            .as_str()
            .unwrap_or_default()
            .contains("/x/templates"),
        "the filesystem path is deliberately NOT carried: one absolute path per \
         template would grow the compartment with the deployment"
    );
}

#[test]
fn only_the_names_the_corpus_lacks_are_written() {
    let Some(r) = shipped() else { return };
    let out = corpus_answered(&r, "foo@1.0.0 collector@3.0.0", &["collector"]);

    assert_eq!(out["header"]["route"], "cat_store");
    let turns = out["messages"].as_array().expect("messages");
    assert_eq!(
        turns.len(),
        1,
        "`collector` is already in the corpus and `foo` is not, so exactly one row is \
         written: {out}"
    );

    let op = op_of(&out, "cat-foo");
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "docs");
    assert_eq!(op["row"]["section"], "foo");
    assert_eq!(
        op["row"]["kind"], "template",
        "the row has to be a CATALOGUE row or `catalogue_lookup` — which filters on \
         kind since 2.0.8 — cannot see it, and the name appeal reads the same rows"
    );
    assert_eq!(op["row"]["id"], "cat-foo");
}

#[test]
fn a_second_reconciliation_writes_nothing() {
    let Some(r) = shipped() else { return };
    let out = corpus_answered(&r, "foo@1.0.0 collector@3.0.0", &["collector", "foo"]);

    assert_eq!(
        out["header"]["route"], "catalogue",
        "nothing missing means nothing to write and the report goes out at once: {out}"
    );
    assert_eq!(out["header"]["catalogue_ingested"], 0);
    assert_eq!(
        out["header"]["catalogue_known"], 2,
        "`docs` has no key, so a re-insert would double every row and the name appeal \
         would answer the same name twice. The diff against the corpus is the whole of \
         the idempotence: {out}"
    );
}

#[test]
fn the_row_it_writes_does_not_invent_a_contract() {
    let Some(r) = shipped() else { return };
    let out = corpus_answered(&r, "foo@1.0.0", &[]);
    let text = op_of(&out, "cat-foo")["row"]["text"]
        .as_str()
        .expect("row text")
        .to_string();

    assert!(
        text.starts_with("CONTRACT --"),
        "every catalogue row opens with the demand, because `retrieve` hands the model \
         text[:1200] and a demand past the truncation is not published: {text}"
    );
    assert!(
        !text.contains("requires no ctx and no env key"),
        "the registry answers an IDENTITY, not a declaration — it carries no `requires` \
         block at all. A row that read 'requires nothing' out of that silence would be \
         the confidently-wrong answer this corpus exists against, and BM25 ranks it as \
         high as a true one: {text}"
    );
    assert!(
        text.contains("EXISTS"),
        "what the row DOES buy is the half that was missing — the name is in the \
         catalogue, so a lookup stops answering with neighbours: {text}"
    );
}

#[test]
fn a_refused_read_is_not_a_registry_of_zero_templates() {
    let Some(r) = shipped() else { return };
    // `/colony/templates` refuses an unreadable filter with an OBJECT, not a list.
    let out = registry_answered(
        &r,
        json!({"status": "error", "error_code": "invalid_query"}),
    );

    assert_eq!(out["header"]["route"], "catalogue");
    assert_eq!(
        out["header"]["error_code"], "catalogue_unavailable",
        "a refusal read as an empty registry would report a healthy reconciliation of \
         nothing — a failure wearing the face of an honest answer (GH #308): {out}"
    );
    assert_eq!(out["header"]["catalogue_ingested"], 0);
}

#[test]
fn a_refused_write_is_named_and_the_corpus_is_left_alone() {
    let Some(r) = shipped() else { return };
    let out = emit_one(
        &script(&r),
        &json!({
            "header": {"hop": {"operation": "bundle", "bundle_errors": 1},
                       "context": {"cat_seen": "foo@1.0.0", "cat_added": "foo", "cat_note": ""}},
            "params": {},
            "messages": [],
            "results": [{"tool_call_id": "cat-foo", "error_code": "invalid_input"}],
        }),
    );

    assert_eq!(out["header"]["route"], "catalogue");
    assert_eq!(
        out["header"]["error_code"], "store_refused",
        "a bundle never reports a leg's failure in the header's own error_code — \
         `bundle_errors` counts them — so both have to be read or a partly refused \
         write reports as a success: {out}"
    );
}

#[test]
fn the_lane_the_hive_declares_is_the_lane_the_cell_speaks() {
    let Some(r) = shipped() else { return };
    let hive = config(&r, "config.json");

    let lanes = |slot: &str| -> Vec<String> {
        hive.pointer(slot)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|l| l["route"].as_str().map(str::to_string))
            .collect()
    };
    assert!(
        lanes("/params/contract/accepts").contains(&"in_ingest".to_string()),
        "the reconciliation is reached on a declared lane or it is unreachable"
    );
    assert!(
        lanes("/params/contract/emits").contains(&"catalogue".to_string()),
        "the report leaves on a declared lane or it is a dead letter"
    );

    // § 2c, the other half: every route the cell emits has an edge that carries
    // it. A route with no edge is a message that dies at the first hop.
    let edges: Vec<&Value> = hive
        .pointer("/params/graph/edges")
        .and_then(Value::as_array)
        .expect("the hive declares edges")
        .iter()
        .collect();
    for route in ["cat_read", "cat_store", "catalogue"] {
        assert!(
            edges.iter().any(|e| {
                e["from"] == "./catalogue"
                    && e["condition"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(&format!("'{route}'"))
            }),
            "no edge leaves ./catalogue on route {route}; the emission would dead-letter"
        );
    }
    assert!(
        edges.iter().any(|e| e["from"] == "./store"
            && e["condition"] == "context.librarian_origin == 'catalogue'"),
        "the way home from the store is a CONTEXT marker and not a hop field — the same \
         shape `retrieve` uses, and for the same reason: it is independent of whichever \
         header shape the store answers with"
    );
}

#[test]
fn the_read_the_librarian_draws_is_one_the_substrate_permits() {
    let Some(r) = shipped() else { return };
    let hive = config(&r, "config.json");

    let targets: Vec<String> = hive
        .pointer("/params/graph/edges")
        .and_then(Value::as_array)
        .expect("edges")
        .iter()
        .filter_map(|e| e["to"].as_str())
        .filter(|t| t.starts_with("/colony"))
        .map(str::to_string)
        .collect();

    assert!(
        targets.contains(&"/colony/templates".to_string()),
        "the reconciliation reads the registry over the colony's own door, because \
         § Database isolation forbids reading `colony.db` — even for reading"
    );
    for t in &targets {
        assert!(
            meclaw_colony::mutation::MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS.contains(&t.as_str()),
            "{t} is not in MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS, so this template dies at \
             GROWTH with `subtree edge endpoint {t} escapes subtree root <root>` — not at \
             runtime, and not with a message about the catalogue. Every path that grows \
             the OS resolves internal edges through the same check."
        );
    }
}

/// The drift lock `docs/development-rules.md` § 2d demands of counted prose:
/// the sentence is grepped AND the mechanism asserted.
#[test]
fn the_counted_report_is_documented_and_stamped() {
    let Some(r) = shipped() else { return };
    let readme = std::fs::read_to_string(r.join("README.md")).expect("README");

    for key in ["catalogue_known", "catalogue_ingested"] {
        assert!(
            readme.contains(key),
            "the README does not name `{key}`, and a counted promise nobody publishes is \
             a number nobody can check"
        );
    }

    let out = corpus_answered(&r, "foo@1.0.0 collector@3.0.0", &["collector"]);
    // The report only comes after the write, so drive one more phase.
    let reported = emit_one(
        &script(&r),
        &json!({
            "header": {"hop": {"operation": "insert"},
                       "context": {"cat_seen": out["header"]["cat_seen"],
                                   "cat_added": out["header"]["cat_added"],
                                   "cat_note": ""}},
            "params": {}, "messages": [],
        }),
    );
    assert_eq!(reported["header"]["catalogue_known"], 1);
    assert_eq!(reported["header"]["catalogue_ingested"], 1);
    assert!(
        reported["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("foo"),
        "the report names what it added, so a reconciliation that ran and found nothing \
         is distinguishable from one that did not run: {reported}"
    );
}
