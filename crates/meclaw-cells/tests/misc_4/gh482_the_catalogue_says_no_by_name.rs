//! GH #482, the `catalogue_lookup` half — the catalogue can say *no, by name*.
//!
//! `catalogue_lookup` is FTS5 over the corpus and it always returns its best
//! hits. Asked for a template that does not exist it answers with four
//! plausible neighbours and nothing that says they are neighbours, so a caller
//! cannot tell *not found* from *found something adjacent* — and a caller that
//! cannot tell has no reason to stop asking. Measured on one wish: seven
//! rounds, the same four questions rephrased, a prompt growing from 5 557 to
//! 50 220 tokens, and no manifest at the end of it.
//!
//! The repair is a second leg on the same store bundle: the catalogue's own
//! **name appeal** (`select section from docs where kind = 'template'`), which
//! for a catalogue row *is* the template name. Phase B compares the request
//! against that list on word boundaries and puts the verdict in front of the
//! briefing — either the names the request does hold, or `no template by that
//! name` plus every name there is.
//!
//! What is pinned here:
//!
//! 1. a marked lookup (`hop.lib_kind == "template"`) asks TWO ops in ONE
//!    message; an unmarked one (`librarian_search`) still asks one;
//! 2. a request naming nothing is refused BY NAME and handed the whole list;
//! 3. a request naming a shipped template is not refused;
//! 4. a name inside a longer word (`member` in `remember`) is not a name, and
//!    a name inside a longer NAME (`builder` in `builder-librarian`) does not
//!    hide it;
//! 5. zero search hits still get the names — `(no matching patterns)` alone is
//!    precisely the answer that does not say what there is instead;
//! 6. a failed name appeal costs the briefing nothing (retrieval is an
//!    enhancement and must never be able to hang a build);
//! 7. the drift lock of `docs/development-rules.md` § 2d, both halves: the
//!    documented answer form is grepped in `description` and README AND driven
//!    through the mechanism.
//!
//! **R2b guard.** Every read is guarded by [`shipped`]: where the template does
//! not ship, these tests skip rather than fail on a dead reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder-librarian")
}

/// The files this suite reads. The list is the guard AND the inventory.
const FILES: &[&str] = &["retrieve/config.json", "README.md", "store/seed/docs.jsonl"];

fn shipped() -> Option<PathBuf> {
    let r = root();
    FILES.iter().all(|f| r.join(f).exists()).then_some(r)
}

fn retrieve_script(r: &Path) -> String {
    shipped_script(r.join("retrieve/config.json").to_str().expect("path"))
}

fn retrieve_config(r: &Path) -> Value {
    let raw = std::fs::read_to_string(r.join("retrieve/config.json")).expect("retrieve config");
    meclaw_core::serde_json::from_str(&raw).expect("parses")
}

/// Every template name the shipped catalogue holds, read the way the name
/// appeal reads it: the `section` of every `kind: "template"` row, deduplicated
/// and sorted. Derived, never written down — a number in a test that is not
/// derived from the corpus is a second copy of the corpus.
fn catalogue_names(r: &Path) -> Vec<String> {
    let raw = std::fs::read_to_string(r.join("store/seed/docs.jsonl")).expect("seed corpus");
    let mut names: Vec<String> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter(|row| row["kind"] == "template")
        .filter_map(|row| row["section"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// How many catalogue rows the corpus holds — the number the name appeal's
/// `limit` may not cut.
fn catalogue_rows(r: &Path) -> usize {
    let raw = std::fs::read_to_string(r.join("store/seed/docs.jsonl")).expect("seed corpus");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter(|row| row["kind"] == "template")
        .count()
}

// ------------------------------------------------------------------ phase A

/// Phase A over a fresh request, with or without the catalogue mark.
fn asked(r: &Path, request: &str, marked: bool) -> Value {
    let mut hop = json!({"route": "in_request"});
    if marked {
        hop["lib_kind"] = json!("template");
    }
    let mut out = emit_all(
        &retrieve_script(r),
        &json!({
            "header": {"hop": hop, "context": {}},
            "params": {},
            "messages": [{"origin": "user", "type": "text", "id": "", "text": request}],
        }),
    );
    assert_eq!(
        out.len(),
        1,
        "phase A speaks once — both ops ride in ONE message or the store never \
         sees them as a bundle"
    );
    out.remove(0)
}

fn op_of(msg: &Value, slot: &str) -> Value {
    let turn = msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|t| t["id"] == slot)
        .unwrap_or_else(|| panic!("no turn keyed `{slot}` in {msg}"));
    meclaw_core::serde_json::from_str(turn["text"].as_str().expect("op travels as text"))
        .expect("the op travels as JSON")
}

#[test]
fn a_catalogue_lookup_asks_for_the_names_too() {
    let Some(r) = shipped() else { return };
    let out = asked(&r, "a feed cell that fetches three rss feeds", true);
    let turns = out["messages"].as_array().expect("messages");
    assert_eq!(
        turns.len(),
        2,
        "a catalogue lookup asks the corpus AND the catalogue for its names, \
         in one bundle: {out}"
    );

    // The search leg is unchanged and stays FIRST — everything that reads a
    // phase-A emission reads `messages[0]`.
    let search = op_of(&out, "lib1");
    assert_eq!(search["operation"], "search");
    assert_eq!(search["where"]["kind"], "template");
    assert_eq!(out["messages"][0]["id"], "lib1");

    let names = op_of(&out, "lib-names");
    assert_eq!(names["operation"], "select", "the name appeal is a select");
    assert_eq!(names["table"], "docs");
    assert_eq!(
        names["columns"],
        json!(["section"]),
        "a catalogue row's `section` IS the template name; nothing else is needed"
    );
    assert_eq!(names["where"]["kind"], "template");
    let limit = names["limit"].as_u64().expect("the appeal carries a limit") as usize;
    assert!(
        limit > catalogue_rows(&r),
        "the appeal's limit ({limit}) does not clear the {} catalogue rows the \
         corpus holds — a cut list answers `these are the names there are` \
         with a subset, which is worse than not answering at all",
        catalogue_rows(&r)
    );
}

#[test]
fn an_unmarked_search_still_asks_exactly_one_question() {
    let Some(r) = shipped() else { return };
    let out = asked(&r, "a feed cell that fetches three rss feeds", false);
    let turns = out["messages"].as_array().expect("messages");
    assert_eq!(
        turns.len(),
        1,
        "`librarian_search` is not the catalogue: it must not grow a second \
         leg, and it must not carry a name verdict: {out}"
    );
    assert!(
        op_of(&out, "lib1").get("where").is_none(),
        "an unmarked search stays unfiltered"
    );
}

// ------------------------------------------------------------------ phase B

/// One catalogue row as the search leg hands it back.
fn row(section: &str) -> Value {
    json!({
        "id": "d0001",
        "source": format!("templates/{section}/template.json"),
        "section": section,
        "kind": "template",
        "text": format!("CONTRACT -- what {section} accepts and emits."),
    })
}

/// The store's answer to a catalogue lookup, as the return edge delivers it:
/// `operation: bundle`, one `tool_result` turn per leg keyed on that leg's
/// `tool_call_id`, and the per-leg metadata in the body's `results[]` slot.
fn briefed(r: &Path, request: &str, hits: Vec<Value>, names: Option<&[String]>) -> Value {
    let mut turns = vec![json!({
        "origin": "tool", "type": "tool_result", "id": "lib1",
        "text": Value::Array(hits.clone()).to_string(),
    })];
    let mut results = vec![json!({
        "tool_call_id": "lib1", "operation": "search",
        "rows_affected": hits.len(), "duration_ms": 1,
    })];
    if let Some(names) = names {
        let rows: Vec<Value> = names.iter().map(|n| json!({"section": n})).collect();
        turns.push(json!({
            "origin": "tool", "type": "tool_result", "id": "lib-names",
            "text": Value::Array(rows).to_string(),
        }));
        results.push(json!({
            "tool_call_id": "lib-names", "operation": "select",
            "rows_affected": names.len(), "duration_ms": 1,
        }));
    }
    let operation = if names.is_some() { "bundle" } else { "search" };
    let mut out = emit_all(
        &retrieve_script(r),
        &json!({
            "header": {
                "hop": {"operation": operation, "rows_affected": hits.len(),
                        "bundle_errors": 0},
                "context": {"orig_request": request},
            },
            "params": {},
            "messages": turns,
            "results": results,
        }),
    );
    assert_eq!(out.len(), 1, "phase B hands over exactly one briefing");
    out.remove(0)
}

/// What the briefing actually says to the model — the `tool_result` turn.
fn said(brief: &Value) -> String {
    brief["messages"][1]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

const REFUSAL: &str = "no template by that name";

#[test]
fn a_name_the_catalogue_does_not_have_comes_back_refused_by_name() {
    let Some(r) = shipped() else { return };
    let names = catalogue_names(&r);
    let brief = briefed(
        &r,
        "rss feed poller template",
        vec![row("daily-digest"), row("clock")],
        Some(&names),
    );
    let text = said(&brief);
    assert!(
        text.starts_with(REFUSAL),
        "the verdict has to come FIRST — a reader that meets four plausible \
         neighbours before the refusal has already started reading them as \
         answers: {text:?}"
    );
    for n in &names {
        assert!(
            text.contains(n.as_str()),
            "the refusal must carry the WHOLE list of names — `{n}` is missing, \
             so the caller still cannot see what there is instead: {text:?}"
        );
    }
    assert!(
        text.contains(&names.len().to_string()),
        "the refusal names how many names there are, so a truncated list is \
         visible as one: {text:?}"
    );
    assert!(
        text.contains("nearest neighbour"),
        "the rows below the refusal must be marked as neighbours, or they read \
         as an answer: {text:?}"
    );
    assert!(
        text.contains("add_nodes"),
        "the refusal must say what the neighbours are NOT — a name a mutation \
         can use: {text:?}"
    );
    // The rows still travel: a refusal is not a reason to withhold the corpus.
    assert!(text.contains("daily-digest"));
    assert_eq!(
        brief["header"]["catalogue_named"], 0,
        "the verdict is measured on the hop, not only spoken in prose"
    );
    assert_eq!(brief["header"]["catalogue_names"], names.len());
    assert_eq!(brief["header"]["route"], "brief");
}

#[test]
fn a_name_the_catalogue_does_have_is_not_refused() {
    let Some(r) = shipped() else { return };
    let names = catalogue_names(&r);
    assert!(
        names.iter().any(|n| n == "daily-digest"),
        "this test needs the shipped catalogue to hold `daily-digest`"
    );
    let brief = briefed(
        &r,
        "build me a daily-digest that posts to telegram",
        vec![row("daily-digest")],
        Some(&names),
    );
    let text = said(&brief);
    assert!(
        !text.contains(REFUSAL),
        "the request names a template that exists — refusing it is the loop \
         one step further on: {text:?}"
    );
    let head = text.lines().next().unwrap_or_default();
    assert!(
        head.contains("daily-digest"),
        "the head line must name what the request named: {head:?}"
    );
    assert_eq!(brief["header"]["catalogue_named"], 1);
}

#[test]
fn a_name_inside_a_longer_word_is_not_a_name() {
    let Some(r) = shipped() else { return };
    let names = catalogue_names(&r);
    assert!(
        names.iter().any(|n| n == "member"),
        "this test needs the shipped catalogue to hold `member`"
    );
    let brief = briefed(
        &r,
        "remember the membership rules of this colony",
        vec![row("member")],
        Some(&names),
    );
    let text = said(&brief);
    assert!(
        text.starts_with(REFUSAL),
        "`member` inside `remember` and `membership` is not a name the request \
         holds — reading it as one hands back a false confirmation, which is \
         worse than the neighbours it replaces: {text:?}"
    );
    assert_eq!(brief["header"]["catalogue_named"], 0);
}

#[test]
fn a_name_inside_a_longer_name_does_not_hide_it() {
    let Some(r) = shipped() else { return };
    let names = catalogue_names(&r);
    for n in ["builder", "builder-librarian"] {
        assert!(
            names.iter().any(|x| x == n),
            "this test needs the shipped catalogue to hold `{n}`"
        );
    }
    let brief = briefed(
        &r,
        "extend the builder-librarian with a second corpus",
        vec![row("builder-librarian")],
        Some(&names),
    );
    let head = said(&brief).lines().next().unwrap_or_default().to_string();
    assert!(
        head.contains("builder-librarian"),
        "the longest name the request holds is the name it holds: {head:?}"
    );
    assert!(
        !head.contains(", builder.") && !head.contains(": builder,"),
        "`builder` is a substring of the name the request actually used, and \
         reporting both sends the composer to the wrong template: {head:?}"
    );
}

#[test]
fn zero_hits_still_learn_what_there_is() {
    let Some(r) = shipped() else { return };
    let names = catalogue_names(&r);
    let brief = briefed(&r, "a quantum teleporter cell", vec![], Some(&names));
    let text = said(&brief);
    assert_ne!(
        text, "(no matching patterns)",
        "that answer is exactly the one that does not say what there IS \
         instead, which is what kept the caller asking"
    );
    assert!(text.starts_with(REFUSAL), "{text:?}");
    for n in &names {
        assert!(text.contains(n.as_str()), "`{n}` is missing: {text:?}");
    }
    assert_eq!(brief["header"]["hits"], 0);
    assert!(
        brief["header"].get("degraded").is_none() || brief["header"]["degraded"] == false,
        "a clean search that found nothing is still not a degradation: {brief}"
    );
}

#[test]
fn a_lost_name_appeal_costs_the_briefing_nothing() {
    let Some(r) = shipped() else { return };
    // The appeal leg answered with an empty slate — the shape a refused or
    // emptied catalogue produces. Retrieval is an enhancement on top of an
    // enhancement: losing it may not cost the briefing a single row.
    let brief = briefed(&r, "a daily-digest", vec![row("daily-digest")], Some(&[]));
    let text = said(&brief);
    assert!(
        text.contains("daily-digest"),
        "the patterns still travel: {text:?}"
    );
    assert!(
        !text.contains(REFUSAL),
        "an EMPTY catalogue cannot refuse a name — it knows no names to refuse \
         it against: {text:?}"
    );
    assert_eq!(brief["header"]["route"], "brief");
    assert_eq!(brief["header"]["stage"], "briefed");
}

#[test]
fn an_unmarked_search_gets_no_verdict() {
    let Some(r) = shipped() else { return };
    // `librarian_search` never asks the second leg, so its briefing is the one
    // it always was — byte for byte, including the honest zero-hit marker.
    let brief = briefed(&r, "something nobody wrote about", vec![], None);
    assert_eq!(said(&brief), "(no matching patterns)");
    assert!(brief["header"].get("catalogue_names").is_none());
    assert!(brief["header"].get("catalogue_named").is_none());
}

#[test]
fn a_refused_search_leg_is_still_not_a_zero_hit() {
    let Some(r) = shipped() else { return };
    // GH #308, one reply shape further on: a bundle reports a leg's failure in
    // `results[]` and never in the header, so a cell reading only
    // `hop.error_code` would render the store's error text as zero rows.
    let names = catalogue_names(&r);
    let rows: Vec<Value> = names.iter().map(|n| json!({"section": n})).collect();
    let mut out = emit_all(
        &retrieve_script(&r),
        &json!({
            "header": {
                "hop": {"operation": "bundle", "rows_affected": 0, "bundle_errors": 1},
                "context": {"orig_request": "build me a daily-digest"},
            },
            "params": {},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "lib1",
                 "text": "no such column: digest"},
                {"origin": "tool", "type": "tool_result", "id": "lib-names",
                 "text": Value::Array(rows).to_string()},
            ],
            "results": [
                {"tool_call_id": "lib1", "operation": "search", "rows_affected": 0,
                 "duration_ms": 1, "error_code": "sql_error"},
                {"tool_call_id": "lib-names", "operation": "select",
                 "rows_affected": names.len(), "duration_ms": 1},
            ],
        }),
    );
    let brief = out.remove(0);
    assert_eq!(brief["header"]["degraded"], true, "{brief}");
    let text = said(&brief);
    assert!(text.contains("sql_error"), "{text:?}");
    assert_ne!(text, "(no matching patterns)");
}

// -------------------------------------------------------------- drift lock

/// `docs/development-rules.md` § 2d, both halves: the sentence is grepped on
/// the two public surfaces AND the mechanism that makes it true is driven.
/// Either half alone lets the two walk apart.
#[test]
fn the_documented_answer_form_is_the_one_the_cell_produces() {
    let Some(r) = shipped() else { return };

    let cfg = retrieve_config(&r);
    let desc = cfg["description"]
        .as_object()
        .expect("the retrieve cell publishes a `description` block");
    let published = meclaw_core::serde_json::to_string(desc).expect("serialise");
    for phrase in [REFUSAL, "nearest neighbour", "add_nodes"] {
        assert!(
            published.contains(phrase),
            "the cell's `description` no longer documents the catalogue \
             lookup's answer form (`{phrase}` is gone) — then this lock is \
             guarding nothing: {published}"
        );
    }

    let readme = std::fs::read_to_string(r.join("README.md")).expect("README");
    for phrase in [REFUSAL, "nearest neighbour"] {
        assert!(
            readme.contains(phrase),
            "the README no longer documents the catalogue lookup's answer form \
             (`{phrase}` is gone): the promise outlived its prose"
        );
    }

    // And the mechanism: the same two sentences, produced by the shipped
    // script over a shipped catalogue.
    let names = catalogue_names(&r);
    let miss = said(&briefed(
        &r,
        "an rss poller",
        vec![row("clock")],
        Some(&names),
    ));
    assert!(
        miss.contains(REFUSAL) && miss.contains("nearest neighbour"),
        "{miss:?}"
    );
    let hit = said(&briefed(
        &r,
        "a daily-digest for the team",
        vec![row("daily-digest")],
        Some(&names),
    ));
    assert!(!hit.contains(REFUSAL), "{hit:?}");
    assert!(hit.contains("daily-digest"), "{hit:?}");

    // The contract carries the two measured keys, or the header can drift away
    // from what the prose claims is measurable.
    let emits = &cfg["contract"]["emits"]["hop"];
    for key in ["catalogue_names", "catalogue_named"] {
        assert_eq!(
            emits[key]["required"], false,
            "`{key}` must be declared on the contract as optional: {emits}"
        );
    }
}
