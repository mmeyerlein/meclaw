//! GH #511 — the retriever cut every long row at 1200 characters, silently, and
//! mid-word.
//!
//! `builder-librarian/retrieve` rendered each hit as `text[:1200]`: no marker,
//! no boundary, and nothing in the row saying it had been cut. The corpus
//! generator has had a length discipline since GH #344 — a section over 4000
//! characters becomes `-cont` continuation rows and the heading says
//! `continued`, so a model does not read a fragment as a whole statement. The
//! retriever's own cap was half that, and it said nothing.
//!
//! Counted over the shipped corpus: 330 of 603 rows were cut, and **80 of the
//! 87 catalogue rows**. That is the class where the cut did the damage. A
//! catalogue row is `CONTRACT —`, `STORES —`, `PARAMS —` and then the whole
//! `template.json`, and `description.examples` is the LAST key of every one of
//! them — so the cut landed, every time, on the only place a template's worked
//! instantiation is published.
//!
//! Measured end to end on one wish. The composer called
//! `catalogue_lookup("clock")` three times. The corpus row for `clock` carries
//! `schedules`, `emit_to` and a full worked `override_params` block. What
//! arrived in the tool result was:
//!
//! ```text
//! ### templates/clock/template.json -- clock (template) [d0306]
//! CONTRACT — …
//! { "name": "clock", … "use_when": "… the `CLOCK_CRON` knob colony-wi
//!
//! ### templates/session-keeper/template.json -- …
//! ```
//!
//! Cut mid-word, straight into the next row. Counts in that tool result:
//! `schedules` 0, `emit_to` 0, `cron` 0. The model then guessed
//! `override_params['interval_ms']`, was refused, guessed
//! `override_params['cron']`, was refused, and set `schedules` without the
//! `emit_to` the door requires — three repair rounds, every one of them
//! recoverable from a block that existed and never travelled (GH #508).
//!
//! And a second-order effect: the briefing tells the composer *"LOOKING IS DONE
//! … when a call answers the way an earlier one did"*. Three lookups returned
//! the identical cut row, so the corpus correctly read as having nothing further
//! to give, while more than half of it had simply not been sent.
//!
//! What is pinned here:
//!
//! 1. a CATALOGUE row travels whole — and the shipped corpus is such that this
//!    is a fact about the tree, not a hope;
//! 2. the measured row itself: `clock`'s params reach the model;
//! 3. every other kind keeps the recall window, cuts on a WORD BOUNDARY, and
//!    says so with a marker that counts what it dropped;
//! 4. a row that fits is not touched and carries no marker;
//! 5. the drift lock of `docs/development-rules.md` § 2d, both halves: the two
//!    knobs and their defaults are grepped on the public surfaces AND derived
//!    from the shipped script, so the prose cannot outlive the mechanism.
//!
//! **R2b guard.** Every read is guarded by [`shipped`]: where the template does
//! not ship, these tests skip rather than fail on a dead reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder-librarian")
}

fn builder_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder")
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

/// The value of one window knob, read out of the SHIPPED script rather than
/// written down here. `shipped_script` resolves `${NAME:-<default>}` to its
/// default, so what comes back is the number an operator who sets nothing gets.
fn knob(r: &Path, name: &str) -> usize {
    let script = retrieve_script(r);
    let needle = format!("{name} = ");
    let at = script
        .find(&needle)
        .unwrap_or_else(|| panic!("the shipped retriever has no `{name}` — GH #511 named it"));
    let rest = &script[at + needle.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("`{name}` is not a literal number: {rest:.40}"))
}

fn row_chars(r: &Path) -> usize {
    knob(r, "ROW_CHARS")
}

fn catalogue_chars(r: &Path) -> usize {
    knob(r, "CATALOGUE_CHARS")
}

/// Phase B, driven the way the store's return edge drives it: one `tool_result`
/// turn keyed `lib1` carrying the slate, and the per-leg metadata beside it.
fn briefed(r: &Path, request: &str, hits: Vec<Value>) -> Value {
    let mut out = emit_all(
        &retrieve_script(r),
        &json!({
            "header": {
                "hop": {"operation": "search", "rows_affected": hits.len()},
                "context": {"orig_request": request},
            },
            "params": {},
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "lib1",
                "text": Value::Array(hits.clone()).to_string(),
            }],
            "results": [{
                "tool_call_id": "lib1", "operation": "search",
                "rows_affected": hits.len(), "duration_ms": 1,
            }],
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

/// A row of `len` characters of ordinary prose, so that a word boundary exists
/// everywhere and a cut landing mid-word is visible as one.
fn body(len: usize) -> String {
    let mut s = String::new();
    let mut n = 0usize;
    while s.len() < len {
        s.push_str(&format!("word{n} "));
        n += 1;
    }
    s.truncate(len);
    s
}

fn a_row(kind: &str, text: &str) -> Value {
    json!({
        "id": "d0001",
        "source": "templates/clock/template.json",
        "section": "clock",
        "kind": kind,
        "text": text,
    })
}

const MARKER: &str = "[TRUNCATED:";

/// Every `kind: "template"` row of the shipped corpus, longest first.
fn catalogue_rows(r: &Path) -> Vec<(usize, String, String)> {
    let raw = std::fs::read_to_string(r.join("store/seed/docs.jsonl")).expect("seed corpus");
    let mut rows: Vec<(usize, String, String)> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter(|row| row["kind"] == "template")
        .filter_map(|row| {
            let text = row["text"].as_str()?.to_string();
            let section = row["section"].as_str()?.to_string();
            Some((text.chars().count(), section, text))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows
}

// --------------------------------------------------------------- the catalogue

#[test]
fn a_catalogue_row_travels_whole() {
    let Some(r) = shipped() else { return };
    let text = body(catalogue_chars(&r) - 1);
    let brief = briefed(&r, "clock", vec![a_row("template", &text)]);
    let out = said(&brief);
    assert!(
        !out.contains(MARKER),
        "a catalogue row inside the window must not be marked as cut: {out:.200}"
    );
    assert!(
        out.contains(&text),
        "a catalogue row IS the template's interface — half an interface is a \
         wrong answer, not a shorter one"
    );
}

/// The claim *"a catalogue row is never cut"* is a claim about the TREE, so it
/// is checked against the tree: the corpus chunker caps a row at 4000
/// characters (`workshop/tools/build_librarian_seed.py`, `MAX_CHARS`) and the
/// catalogue window has to clear that, or the sentence is a wish.
#[test]
fn the_catalogue_window_clears_the_longest_row_the_corpus_holds() {
    let Some(r) = shipped() else { return };
    let rows = catalogue_rows(&r);
    assert!(
        rows.len() > 20,
        "{} catalogue rows is not the shipped corpus — the check would pass \
         vacuously",
        rows.len()
    );
    let (len, section, _) = &rows[0];
    assert!(
        *len <= catalogue_chars(&r),
        "the longest catalogue row (`{section}`, {len} characters) does not fit \
         the {} the retriever gives it, so the corpus DOES cut a catalogue row \
         and the README says it does not",
        catalogue_chars(&r)
    );
}

/// The measured failure itself, on the shipped row rather than a fixture.
/// `clock` is one of the four blank single-cell templates: it exists in order
/// to be overridden, so the params surface IS its interface.
#[test]
fn the_clock_row_reaches_the_model_with_its_params_on_it() {
    let Some(r) = shipped() else { return };
    let Some((_, _, text)) = catalogue_rows(&r)
        .into_iter()
        .find(|(_, s, _)| s == "clock")
    else {
        eprintln!("skipped: the corpus carries no catalogue row for `clock`");
        return;
    };
    let out = said(&briefed(
        &r,
        "a clock that ticks the firewall's in_sweep every five minutes",
        vec![a_row("template", &text)],
    ));
    for name in ["schedules", "emit_to"] {
        assert!(
            out.contains(name),
            "`{name}` counted 0 in the measured tool result and is what three \
             repair rounds were spent guessing; it stands in the row and must \
             reach the model"
        );
    }
    // The two names above are answered by the `PARAMS —` demand line of GH
    // #505, which is budgeted to sit inside the OLD window. What #511 adds is
    // the rest of the row: the whole `template.json`, whose LAST key is
    // `description.examples` — the worked `override_params` block the composer
    // was guessing at, and the half that never travelled.
    assert!(
        !out.contains(MARKER),
        "the `clock` row is still being cut, so its worked example still does \
         not reach the model: {out:.300}"
    );
    assert!(
        out.contains(&text),
        "the catalogue row must arrive whole — the examples are its last key"
    );
}

// ------------------------------------------------------------- everything else

#[test]
fn a_row_that_fits_is_not_touched() {
    let Some(r) = shipped() else { return };
    let text = body(row_chars(&r) / 2);
    let out = said(&briefed(&r, "a spec question", vec![a_row("spec", &text)]));
    assert!(out.contains(&text), "a short row travels verbatim");
    assert!(
        !out.contains(MARKER),
        "a row that was not cut must not claim it was: {out:.200}"
    );
}

#[test]
fn a_row_that_does_not_fit_is_cut_on_a_word_boundary_and_says_so() {
    let Some(r) = shipped() else { return };
    let cap = row_chars(&r);
    let text = body(cap * 3);
    let out = said(&briefed(&r, "a spec question", vec![a_row("spec", &text)]));

    assert!(
        out.contains(MARKER),
        "the cut has to be legible — a fragment a reader cannot recognise as a \
         fragment is a different object from one it can (GH #344): {out:.300}"
    );

    // What survived, between the row heading and the marker.
    let head = out
        .split_once('\n')
        .expect("the row carries its heading first")
        .1
        .split(MARKER)
        .next()
        .expect("text before the marker")
        .trim_end_matches(['\n', '\u{2026}', ' ']);
    assert!(
        !head.is_empty() && text.starts_with(head),
        "what travels must be a PREFIX of the row and nothing invented"
    );
    assert!(
        head.len() <= cap,
        "the window is {cap}; {} characters travelled",
        head.len()
    );
    assert!(
        head.len() > cap / 2,
        "a seam is worth having only while it costs a word, not half the window"
    );
    assert!(
        text[head.len()..].starts_with(' '),
        "the cut landed MID-WORD — that is the defect, and it is what made the \
         last token of every catalogue row unreadable: {:?}",
        &text[head.len().saturating_sub(20)..head.len() + 10]
    );

    // The marker counts what it dropped, and the two numbers agree with the row.
    let after = out.split(MARKER).nth(1).expect("the marker's body");
    let nums: Vec<usize> = after
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(2)
        .map(|s| s.parse().expect("a count"))
        .collect();
    assert_eq!(nums.len(), 2, "the marker says <n> of <m>: {after:.120}");
    assert_eq!(
        nums[1],
        text.len(),
        "the marker's second number is the row's whole length"
    );
    assert_eq!(
        nums[0],
        text.len() - head.len(),
        "the marker's first number is what did NOT travel — a count that does \
         not add up is worse than none"
    );
    assert!(
        after.contains("FRAGMENT"),
        "the marker has to say what the row now IS, not only that it is short"
    );
    assert!(
        after.contains("catalogue_lookup"),
        "and it names the one retrieval that is never cut, so the reader has a \
         move rather than a regret"
    );
}

/// Two rows, two windows, in ONE briefing — the kind decides per row, not per
/// call, so a `librarian_search` that happens to surface a catalogue row still
/// hands it over whole.
#[test]
fn the_window_is_decided_per_row_and_not_per_call() {
    let Some(r) = shipped() else { return };
    let long = body(catalogue_chars(&r) - 1);
    let out = said(&briefed(
        &r,
        "clock",
        vec![a_row("template", &long), a_row("spec", &long)],
    ));
    assert_eq!(
        out.matches(MARKER).count(),
        1,
        "exactly one of the two rows is cut — the template row is not: {out:.300}"
    );
}

// ------------------------------------------------------------------ drift lock

/// § 2d, both halves. The knob names and their defaults are published on two
/// public template surfaces; here they are grepped there AND derived from the
/// script, so neither can move without the other.
#[test]
fn the_two_windows_are_published_with_the_numbers_the_script_uses() {
    let Some(r) = shipped() else { return };
    let readme = std::fs::read_to_string(r.join("README.md")).expect("the librarian README");
    for (env, value) in [
        ("BUILDER_LIBRARIAN_ROW_CHARS", row_chars(&r)),
        ("BUILDER_LIBRARIAN_CATALOGUE_CHARS", catalogue_chars(&r)),
    ] {
        assert!(
            readme.contains(env),
            "`{env}` is a knob of this template and its README does not name it"
        );
        assert!(
            readme.contains(&format!("`{env}` | `{value}`")),
            "the README publishes `{env}` with a default the script does not \
             use — the script says {value}"
        );
        // A number in template prose stands exactly once, or it is derived.
        // Here it is derived, and the second surface therefore carries the NAME
        // and no second copy of the number.
        assert!(
            std::fs::read_to_string(r.join("retrieve/config.json"))
                .expect("the retriever")
                .contains(&format!("${{{env}:-{value}}}")),
            "the knob is not written as `${{{env}:-{value}}}` in the shipped \
             config, so an operator cannot set it at all"
        );
    }
    let builder = builder_root().join("README.md");
    if builder.exists() {
        let builder = std::fs::read_to_string(builder).expect("the builder README");
        for env in [
            "BUILDER_LIBRARIAN_ROW_CHARS",
            "BUILDER_LIBRARIAN_CATALOGUE_CHARS",
        ] {
            assert!(
                builder.contains(env),
                "the builder publishes the librarian's knobs beside its own and \
                 `{env}` is missing"
            );
        }
    }
}

/// The mechanism half of the same lock, on the descriptor: `retrieve`'s own
/// `description` claims the marker's wording, so the claim is driven through
/// the cell.
#[test]
fn the_descriptor_publishes_the_marker_the_cell_actually_writes() {
    let Some(r) = shipped() else { return };
    let raw = std::fs::read_to_string(r.join("retrieve/config.json")).expect("retrieve config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("parses");
    let says = cfg["description"]["emits_meaning"]
        .as_str()
        .expect("the retriever describes what it emits");
    assert!(
        says.contains("TRUNCATED"),
        "the descriptor does not mention the marker at all"
    );
    let text = body(row_chars(&r) * 2);
    let out = said(&briefed(&r, "a spec question", vec![a_row("spec", &text)]));
    let marker_tail = out
        .split(MARKER)
        .nth(1)
        .expect("the marker")
        .split(']')
        .next()
        .expect("the marker closes");
    for sentence in ["FRAGMENT", "catalogue_lookup"] {
        assert!(
            says.contains(sentence) && marker_tail.contains(sentence),
            "`{sentence}` is published on one side and written on the other — \
             that is the drift the lock exists to catch"
        );
    }
}
