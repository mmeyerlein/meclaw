//! 0.3.x follow-up F4 -- a night describes the questions it actually has
//! (GitHub #69).
//!
//! The consolidation round is ONE model call carrying every question the night
//! asks, and that stays. What did not stay is the instruction block: it was
//! rendered whole on every night, including the nights that had none of those
//! questions to ask. It grew from about 5.1 kB to about 8.1 kB over the
//! statement-identity track (7715 to 9915 prompt tokens per night, measured over
//! the eight rounds of the track-end run), while the DATA half already behaved:
//! the cardinality section is absent without an open relation, the per-axis
//! refusal list is absent without a refusal, and both are pinned as absent.
//!
//! So the fix is a mapping, not a rewrite: one instruction section per data
//! section, one answer-shape key per instruction section, and a set of question
//! names derived ONCE that decides all three (call or no call, which paragraphs,
//! which keys). The risk the issue names is the reason half of the pins below
//! exist: the block is also where the questions constrain each other ("do not
//! merge two quantities", "do not close an enumeration"), and dropping a section
//! must not change how the remaining questions are answered.
//!
//! Everything here runs the REAL `params.script_inline` of the `code` cell
//! against injected store replies, so no model is called and nothing costs
//! anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("config");
    let config: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(config["params"]["script_inline"].as_str().expect("script"))
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let script = glue_script();
    let out = run_script_on_stdin(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// The four data sections of one night, as the payload builder parks them.
#[derive(Clone, Copy)]
struct Night {
    predicates: usize,
    pairs: usize,
    axes: usize,
    cardinality: usize,
}

/// The night the statement-identity track ends on: every question has data.
const FULL: Night = Night {
    predicates: 2,
    pairs: 1,
    axes: 1,
    cardinality: 1,
};

impl Night {
    fn with(self, f: impl FnOnce(&mut Night)) -> Night {
        let mut out = self;
        f(&mut out);
        out
    }
}

/// The parked scan of a night with the requested sections, built section by
/// section instead of derived from facts: this file is about the RENDERING, and
/// what the scan derives is pinned where it is derived (`p5_canonical_dream`,
/// `w3_judge_closures`, `w5_judged_cardinality`, `w6_claim_aliases`).
fn scan_of(night: Night) -> serde_json::Value {
    let predicates: serde_json::Map<String, serde_json::Value> = (0..night.predicates)
        .map(|i| (format!("relation_{i}"), serde_json::json!(["user"])))
        .collect();
    let axes: Vec<serde_json::Value> = (0..night.axes)
        .map(|i| {
            serde_json::json!({
                "subject": "user", "predicate": format!("axis_{i}"),
                "statements": [
                    {"id": format!("s{i}a"), "claim": "practices yoga twice a week",
                     "since": "2026-01-01T00:00:00Z", "last_asserted": "2026-01-01T00:00:00Z",
                     "assertions": 1},
                    {"id": format!("s{i}b"), "claim": "The user practices yoga.",
                     "since": "2026-02-01T00:00:00Z", "last_asserted": "2026-02-01T00:00:00Z",
                     "assertions": 1}
                ]
            })
        })
        .collect();
    let cardinality: Vec<serde_json::Value> = (0..night.cardinality)
        .map(|i| {
            serde_json::json!({"predicate": format!("collects_{i}"),
                                    "values": ["vinyl", "stamps"]})
        })
        .collect();
    let mut scan = serde_json::json!({
        "predicates": predicates,
        "context": {"user": ["favorite editor is helix"]},
        "axes": axes
    });
    if !cardinality.is_empty() {
        scan["cardinality"] = serde_json::json!(cardinality);
    }
    scan
}

/// Everything the round emits for a night of that shape.
fn round(night: Night) -> Vec<serde_json::Value> {
    let pairs: Vec<serde_json::Value> = (0..night.pairs)
        .map(|i| {
            serde_json::json!({"left": format!("site:alpha{i}"),
                                    "right": format!("site:alpha{i}x"), "score": 0.9})
        })
        .collect();
    emit(serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "canon-ask",
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": "select", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text":
            serde_json::json!([
                {"key": RUN, "kind": "canon-scan", "payload": scan_of(night).to_string()},
                {"key": RUN, "kind": "canon-pairs",
                 "payload": serde_json::Value::from(pairs).to_string()},
                {"key": RUN, "kind": "canon-card", "payload": "{}"},
                {"key": RUN, "kind": "canon-refused", "payload": "[]"}
            ]).to_string()}]
    }))
}

/// The instruction block the round put to the judge, or None when it made no
/// call at all.
fn instructions(night: Night) -> Option<String> {
    let msgs = round(night);
    let msg = msgs.iter().find(|m| m["header"]["route"] == "judge")?;
    Some(
        msg["system"]["instructions"]["text"]
            .as_str()
            .expect("instructions")
            .to_string(),
    )
}

/// The block of a night that has something to ask -- the normal case here.
fn asked(night: Night) -> String {
    instructions(night).expect("this night has a question and therefore a call")
}

/// The declared answer shape: the JSON skeleton in the first sentence.
fn shape(text: &str) -> String {
    let start = text.find('{').expect("a shape");
    let end = text.find("}.\n\n").expect("the end of the shape");
    text[start..end + 1].to_string()
}

// --------------------------------------------------------------- the full night

#[test]
fn a_night_that_carries_every_question_declares_every_key() {
    // The invariance half of the issue: a night that still has all five
    // questions must be asked exactly what it was asked before. Both halves of
    // that -- the shape and the map of the questions -- are pinned verbatim,
    // because "same verdicts as before" is only free while the prompt is the
    // same prompt.
    let text = asked(FULL);
    assert_eq!(
        shape(&text),
        "{\"predicates\":[{\"alias\":\"\",\"canonical\":\"\"}],\
         \"entities\":[{\"alias\":\"\",\"canonical\":\"\"}],\
         \"different\":[{\"dimension\":\"subject\",\"left\":\"\",\"right\":\"\"}],\
         \"closures\":[{\"subject\":\"\",\"predicate\":\"\",\"closed\":\"\",\
         \"superseded_by\":\"\",\"ended_at\":\"\",\"reason\":\"\"}],\
         \"reopenings\":[{\"subject\":\"\",\"predicate\":\"\",\"statement\":\"\",\
         \"closed_by\":\"\",\"reason\":\"\"}],\
         \"cardinality\":[{\"predicate\":\"\",\"verdict\":\"\",\"reason\":\"\"}],\
         \"same_value\":[{\"subject\":\"\",\"predicate\":\"\",\"canonical\":\"\",\
         \"alias\":\"\",\"reason\":\"\"}]}",
        "the answer shape of a full night is the one the track ended on"
    );
    assert!(
        text.contains(
            "Five questions in one payload. The first two are about IDENTITY: nothing you \
             say there changes a stored value, it states that two spellings are one thing. \
             The third is about CURRENCY: which of the statements this memory holds are \
             still true. The fourth is about the SHAPE of a relation and is answered once \
             per relation, not per value. None of your answers ever deletes a row or edits \
             a written value."
        ),
        "the map of a full night is the paragraph it always was: {text}"
    );
    for header in [
        "1. `predicates`",
        "2. `entity_pairs`",
        "3. `axes`",
        "5. `same_value`",
        "4. `cardinality`",
        "Core vocabulary",
        "Never invent a value",
    ] {
        assert!(text.contains(header), "a full night lost {header:?}");
    }
}

#[test]
fn the_sections_of_a_full_night_stand_in_the_order_they_always_did() {
    // Rendering per section is a filter over a fixed list, never a re-ordering:
    // question 5 sits between 3 and 4 because it reads the same page as 3, and a
    // block that shuffled on some nights would be a different prompt on those
    // nights even with the same sections in it.
    let text = asked(FULL);
    let at = |needle: &str| text.find(needle).expect("section");
    let order = [
        at("1. `predicates`"),
        at("Core vocabulary"),
        at("2. `entity_pairs`"),
        at("3. `axes`"),
        at("5. `same_value`"),
        at("4. `cardinality`"),
        at("Never invent a value"),
    ];
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "the sections moved: {order:?}"
    );
}

// ------------------------------------------------------------- the quiet nights

#[test]
fn a_night_without_an_open_relation_is_not_asked_about_cardinality() {
    // The section the issue names first: absent from the payload since W5,
    // described in the instructions every night until now.
    let text = asked(FULL.with(|n| n.cardinality = 0));
    assert!(
        !text.contains("4. `cardinality`"),
        "the cardinality question was described without a relation to ask about: {text}"
    );
    assert!(
        !shape(&text).contains("\"cardinality\""),
        "the answer shape still declares a key the night has no question for: {}",
        shape(&text)
    );
    assert!(
        !text.contains("The fourth is about the SHAPE"),
        "the map still announces the question: {text}"
    );
    assert!(
        text.contains("3. `axes`") && text.contains("5. `same_value`"),
        "the questions that DID have data have to survive the cut: {text}"
    );
}

#[test]
fn a_night_without_candidate_pairs_is_not_asked_about_names() {
    let text = asked(FULL.with(|n| n.pairs = 0));
    assert!(
        !text.contains("2. `entity_pairs`") && !text.contains("ENTITY NAMES ARE VERBATIM"),
        "the entity question was described without a pair to judge: {text}"
    );
    assert!(
        !shape(&text).contains("\"entities\""),
        "the answer shape still declares the entity aliases: {}",
        shape(&text)
    );
}

#[test]
fn a_night_with_one_relation_is_not_asked_which_two_are_one() {
    // The oldest of the four conditions, and the only one that is not simply
    // "non-empty": one relation cannot be a synonym of itself. It has gated the
    // CALL since P5 -- now it gates the paragraph as well.
    let text = asked(FULL.with(|n| n.predicates = 1));
    assert!(
        !text.contains("1. `predicates`"),
        "a night with one relation was asked to group it: {text}"
    );
    assert!(
        !shape(&text).contains("\"predicates\""),
        "the answer shape still declares the predicate aliases: {}",
        shape(&text)
    );
    assert!(
        !text.contains("Core vocabulary"),
        "the canonical-key vocabulary belongs to the question that names keys: {text}"
    );
}

#[test]
fn a_night_with_single_statement_axes_is_asked_neither_currency_nor_rewording() {
    let text = asked(FULL.with(|n| n.axes = 0));
    for gone in [
        "3. `axes`",
        "5. `same_value`",
        "The third is about CURRENCY",
    ] {
        assert!(
            !text.contains(gone),
            "the axis page is empty and {gone:?} was still rendered: {text}"
        );
    }
    let shape = shape(&text);
    for gone in ["\"closures\"", "\"reopenings\"", "\"same_value\""] {
        assert!(
            !shape.contains(gone),
            "the answer shape still declares {gone}: {shape}"
        );
    }
}

#[test]
fn a_night_with_nothing_to_ask_still_makes_no_call() {
    // Invariance: the guard that skips the call has been there since P5. It is
    // now the SAME predicate that renders the sections -- derived once -- so a
    // call without a question and a question without a section became the same
    // impossibility instead of two rules that could drift.
    let quiet = Night {
        predicates: 1,
        pairs: 0,
        axes: 0,
        cardinality: 0,
    };
    assert!(
        instructions(quiet).is_none(),
        "a night with nothing to ask called the most expensive model of the hive"
    );
    let msgs = round(quiet);
    assert_eq!(msgs.len(), 1, "the round should walk on: {msgs:?}");
}

// ------------------------------------------------- the constraints that must stay

#[test]
fn the_two_questions_on_one_axis_page_are_never_rendered_apart() {
    // The cross-question risk the issue names, and the structural answer to it:
    // "do not close an enumeration" (3) and "do not merge two quantities" (5)
    // read the SAME data section, so no combination of sections can separate
    // them. Proven over every combination of the other three.
    for predicates in [1, 2] {
        for pairs in [0, 1] {
            for cardinality in [0, 1] {
                for axes in [0, 1] {
                    let night = Night {
                        predicates,
                        pairs,
                        axes,
                        cardinality,
                    };
                    let text = instructions(night).unwrap_or_default();
                    assert_eq!(
                        text.contains("3. `axes`"),
                        text.contains("5. `same_value`"),
                        "one of the two axis questions was rendered without the other \
                         ({predicates}/{pairs}/{axes}/{cardinality}): {text}"
                    );
                }
            }
        }
    }
}

#[test]
fn every_guard_rail_stands_wherever_the_question_it_guards_stands() {
    // Rule 2 of the map: a constraint lives with the question whose ANSWERS it
    // guards. Each pair below is (the rail, the question it belongs to), checked
    // in both directions over every combination of sections -- a rail that
    // outlived its question would cost tokens for an answer nobody can give, and
    // a rail that died with a section still present would change how that
    // section is answered. That second one is the whole risk of this package.
    let rails = [
        (
            "closing one of its values deletes an answer that is true",
            "3. `axes`",
        ),
        (
            "NUMBERS, QUANTITIES, DATES AND SIZES ARE NEVER A REWORDING",
            "5. `same_value`",
        ),
        ("ENTITY NAMES ARE VERBATIM", "2. `entity_pairs`"),
        (
            "Put every pair you turned down into `different`",
            "2. `entity_pairs`",
        ),
        ("`dimension` set to \"claim\"", "5. `same_value`"),
        (
            "an axis may carry a `known_different` list",
            "5. `same_value`",
        ),
        ("the verdict is about the RELATION", "4. `cardinality`"),
        (
            "a key used on completely unrelated subjects is a hint",
            "1. `predicates`",
        ),
        ("Core vocabulary", "1. `predicates`"),
    ];
    for predicates in [1, 2] {
        for pairs in [0, 1] {
            for cardinality in [0, 1] {
                for axes in [0, 1] {
                    let night = Night {
                        predicates,
                        pairs,
                        axes,
                        cardinality,
                    };
                    let text = instructions(night).unwrap_or_default();
                    for (rail, question) in rails {
                        assert_eq!(
                            text.contains(rail),
                            text.contains(question),
                            "{rail:?} and {question:?} parted ways \
                             ({predicates}/{pairs}/{axes}/{cardinality})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_answer_shape_never_declares_a_key_without_its_question() {
    // The other direction of the mapping, over every combination: the judge is
    // never asked for a key it has no question for. `different` is deliberately
    // not in this list -- it is fed by TWO questions and has its own pin.
    let keys = [
        ("\"predicates\"", "1. `predicates`"),
        ("\"entities\"", "2. `entity_pairs`"),
        ("\"closures\"", "3. `axes`"),
        ("\"reopenings\"", "3. `axes`"),
        ("\"same_value\"", "5. `same_value`"),
        ("\"cardinality\"", "4. `cardinality`"),
    ];
    for predicates in [1, 2] {
        for pairs in [0, 1] {
            for cardinality in [0, 1] {
                for axes in [0, 1] {
                    let night = Night {
                        predicates,
                        pairs,
                        axes,
                        cardinality,
                    };
                    let Some(text) = instructions(night) else {
                        continue;
                    };
                    let declared = shape(&text);
                    for (key, question) in keys {
                        assert_eq!(
                            declared.contains(key),
                            text.contains(question),
                            "{key} and {question:?} disagree \
                             ({predicates}/{pairs}/{axes}/{cardinality}): {declared}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_refusal_log_stands_as_long_as_either_question_that_feeds_it() {
    // `different` is the one key two questions write to: entity pairs turned
    // down (dimension `subject`) and rewordings turned down (dimension `claim`).
    // It therefore survives the loss of either one -- and the dimension it shows
    // is the one that night can receive, because an item that names no dimension
    // is read as `subject` on the apply side.
    let pairs_only = shape(&asked(FULL.with(|n| n.axes = 0)));
    assert!(
        pairs_only.contains("\"different\":[{\"dimension\":\"subject\""),
        "the entity refusals kept the log alive, on their own dimension: {pairs_only}"
    );
    let axes_only = shape(&asked(Night {
        predicates: 1,
        pairs: 0,
        axes: 1,
        cardinality: 0,
    }));
    assert!(
        axes_only.contains("\"different\":[{\"dimension\":\"claim\""),
        "with only the rewordings left, the log shows the dimension they use: {axes_only}"
    );
    let card_only = shape(&asked(Night {
        predicates: 1,
        pairs: 0,
        axes: 0,
        cardinality: 1,
    }));
    assert!(
        !card_only.contains("\"different\""),
        "nothing feeds the refusal log on this night: {card_only}"
    );
}

#[test]
fn the_cardinality_question_never_needed_the_vocabulary_it_lost() {
    // Why the core vocabulary may travel with question 1 although question 4
    // speaks of `single` and `multi` too: question 4 defines both words in its
    // own paragraph, and it is never shown a relation off those lists --
    // `cardinality_candidates` drops a seeded relation before the payload
    // exists (`the_scan_offers_the_predicates_whose_cardinality_is_still_open`).
    let text = asked(Night {
        predicates: 1,
        pairs: 0,
        axes: 0,
        cardinality: 1,
    });
    assert!(
        !text.contains("Core vocabulary"),
        "the list travelled with the wrong question: {text}"
    );
    assert!(
        text.contains("FUNCTIONAL (`single`: one value at a time")
            && text.contains("ENUMERATING (`multi`: the values coexist"),
        "the two words the question uses have to be defined where it asks: {text}"
    );
}

// ------------------------------------------------------------- what it is worth

#[test]
fn a_quiet_night_pays_a_fraction_of_what_a_full_one_pays() {
    // The measurement the issue is about. The block grew to about 8.1 kB over
    // the track; a store whose only open question is one relation's cardinality
    // now carries under a quarter of that, every night, forever.
    let full = asked(FULL).len();
    let quiet = asked(Night {
        predicates: 1,
        pairs: 0,
        axes: 0,
        cardinality: 1,
    })
    .len();
    assert!(
        quiet * 4 < full,
        "a one-question night should not cost like a five-question one: {quiet} vs {full}"
    );
    assert!(
        full > 8000,
        "the full block is the one the track ended on ({full} bytes), \
         so the comparison means something"
    );
}
