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
//! { "name": "clock", … "use_when": "… the cadence a param colony-wi
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
//! GH #1085 (R-IG-1) took the retriever's own numbers away: the per-kind
//! windows of #511 (1200, 4000, 1600) knew nothing of the model reading the
//! briefing. The briefing is one tool result now and takes a tool result's
//! share (10 %) of that model's window; without a known window every row
//! travels whole.
//!
//! What is pinned here:
//!
//! 1. without a window every row travels whole -- a catalogue row, the measured
//!    `clock` row, and an ordinary row three times the old 1200;
//! 2. with a window, rows travel whole while they fit; the first that does not
//!    travels as a PREFIX with the one mark, which counts what it showed and
//!    the total and names the retrieval to ask, and every row after it is
//!    named as dropped with its size;
//! 3. a row that fits is not touched and carries no mark;
//! 4. the drift lock of `docs/development-rules.md` § 2d: the README and the
//!    descriptor publish the mark the cell writes, and the retriever carries
//!    no window knob of its own any more.
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

/// Phase B, driven the way the store's return edge drives it: one `tool_result`
/// turn keyed `lib1` carrying the slate, and the per-leg metadata beside it.
fn briefed(r: &Path, request: &str, hits: Vec<Value>) -> Value {
    briefed_in(r, request, hits, None)
}

/// The same, for a reader whose window is `input_soft` tokens (in the context,
/// where the curator puts it).
fn briefed_in(r: &Path, request: &str, hits: Vec<Value>, input_soft: Option<u64>) -> Value {
    let mut context = json!({"orig_request": request});
    if let Some(w) = input_soft {
        context["input_soft"] = json!(w);
    }
    let mut out = emit_all(
        &retrieve_script(r),
        &json!({
            "header": {
                "hop": {"operation": "search", "rows_affected": hits.len()},
                "context": context,
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
    a_row_id("d0001", kind, text)
}

fn a_row_id(id: &str, kind: &str, text: &str) -> Value {
    json!({
        "id": id,
        "source": "templates/clock/template.json",
        "section": "clock",
        "kind": kind,
        "text": text,
    })
}

const MARK: &str = "...[cut: ";

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

// --------------------------------------------------------- no window: whole

#[test]
fn without_a_window_every_row_travels_whole() {
    let Some(r) = shipped() else { return };
    let catalogue = body(10_000);
    let spec = body(3600).replace("word", "spec");
    let out = said(&briefed(
        &r,
        "clock",
        vec![a_row("template", &catalogue), a_row("spec", &spec)],
    ));
    assert!(
        out.contains(&catalogue),
        "a catalogue row over the old 4000 travels whole"
    );
    assert!(
        out.contains(&spec),
        "an ordinary row three times the old 1200 travels whole"
    );
    assert!(
        !out.contains(MARK) && !out.contains("[dropped:") && !out.contains("TRUNCATED"),
        "nothing was cut, so nothing may say it was: {out:.200}"
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
    // No window, and the window of a 32k-token reader: both carry it whole.
    for window in [None, Some(32_000)] {
        let out = said(&briefed_in(
            &r,
            "a clock that ticks the firewall's in_sweep every five minutes",
            vec![a_row("template", &text)],
            window,
        ));
        for name in ["schedules", "emit_to"] {
            assert!(
                out.contains(name),
                "`{name}` counted 0 in the measured tool result and is what three \
                 repair rounds were spent guessing; it must reach the model"
            );
        }
        assert!(
            !out.contains(MARK) && out.contains(&text),
            "{window:?}: the catalogue row must arrive whole -- the examples are its \
             last key: {out:.300}"
        );
    }
}

// ------------------------------------------------------- with a window: budget

#[test]
fn a_row_that_fits_is_not_touched() {
    let Some(r) = shipped() else { return };
    let text = body(600);
    // 20 000 tokens: a tenth of it, at three characters a token, is 6000.
    let out = said(&briefed_in(
        &r,
        "a spec question",
        vec![a_row("spec", &text)],
        Some(20_000),
    ));
    assert!(out.contains(&text), "a short row travels verbatim");
    assert!(
        !out.contains(MARK),
        "a row that was not cut must not claim it was: {out:.200}"
    );
}

#[test]
fn rows_fill_the_budget_then_one_is_cut_with_the_mark_and_the_rest_are_named() {
    let Some(r) = shipped() else { return };
    let first = body(3000);
    let second = body(5000);
    let third = body(2000).replace("word", "third");
    let out = said(&briefed_in(
        &r,
        "a spec question",
        vec![
            a_row_id("d0001", "spec", &first),
            a_row_id("d0002", "spec", &second),
            a_row_id("d0003", "spec", &third),
        ],
        Some(20_000),
    ));
    assert!(out.contains(&first), "the best row fits and travels whole");
    assert_eq!(
        out.matches(MARK).count(),
        1,
        "exactly one row is cut: {out:.300}"
    );

    // The cut row: a PREFIX of its rendered form and nothing invented, then
    // the mark with what it showed and the total.
    let cut = out
        .split("### ")
        .find(|s| s.contains("[d0002]"))
        .expect("the second row");
    let (shown, after) = cut.split_once(MARK).expect("the mark");
    let shown = format!("### {shown}");
    let rendered = format!("{}\n{second}", shown.split_once('\n').expect("a heading").0);
    assert!(
        rendered.starts_with(&shown),
        "what travels is a prefix of the row"
    );
    let nums: Vec<usize> = after
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(2)
        .map(|s| s.parse().expect("a count"))
        .collect();
    assert_eq!(
        nums,
        vec![shown.chars().count(), rendered.chars().count()],
        "{after:.120}"
    );
    assert!(
        after.contains("catalogue_lookup"),
        "the mark names the retrieval that asks for one row by name"
    );
    // The third row did not travel, and the briefing says so with its size.
    assert!(!out.contains(&third), "{out:.300}");
    assert!(out.contains("...[dropped: d0003 ("), "{out:.300}");
}

// ------------------------------------------------------------------ drift lock

/// § 2d, both halves: what the cell writes is what the README and the
/// descriptor publish, and the retriever holds no window number any more.
#[test]
fn the_mark_is_published_as_the_cell_writes_it() {
    let Some(r) = shipped() else { return };
    let readme = std::fs::read_to_string(r.join("README.md")).expect("the librarian README");
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(r.join("retrieve/config.json")).expect("the retriever"),
    )
    .expect("the retriever config parses");
    let says = cfg["description"]["emits_meaning"]
        .as_str()
        .expect("the retriever describes what it emits");
    let out = said(&briefed_in(
        &r,
        "a spec question",
        vec![
            a_row_id("d0001", "spec", &body(9000)),
            a_row_id("d0002", "spec", &body(10)),
        ],
        Some(20_000),
    ));
    let mark_tail = out
        .split(MARK)
        .nth(1)
        .expect("the mark")
        .split(']')
        .next()
        .expect("the mark closes");
    for sentence in ["chars shown; budget of the window", "catalogue_lookup"] {
        assert!(
            mark_tail.contains(sentence) && says.contains(sentence) && readme.contains(sentence),
            "`{sentence}` is written by the cell and must stand on both surfaces"
        );
    }
    for surface in [says, readme.as_str()] {
        assert!(
            surface.contains("...[dropped: "),
            "the dropped mark is published"
        );
        assert!(
            !surface.contains("TRUNCATED"),
            "the old marker is gone from the prose"
        );
    }
    for knob in ["row_chars", "catalogue_chars", "level_chars"] {
        assert!(
            cfg["params"].get(knob).is_none() && cfg["contract"]["settings"].get(knob).is_none(),
            "`{knob}` is a number of the retriever's own, and R-IG-1 took it away"
        );
    }
}
