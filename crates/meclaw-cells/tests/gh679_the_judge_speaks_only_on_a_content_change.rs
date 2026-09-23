//! GH #679 -- the judge speaks only on a content change.
//!
//! Beside the compose cell stands `judge`, an `llm` cell with one prompt. The
//! curator asks it at the end of a pass in which at least one window took an APP
//! touch -- never on a stroke, never after a write that changed nothing, and at
//! the earliest `judge_min_interval_ms` after the last call (display-hive.md
//! § 4.3). The answer comes back on the lane `in_verdict`, and what the judge
//! may write is four things: `judged_relevance` and `judged_hidden` per window,
//! `bar` and `weights` on the state (§ 4.5).
//!
//! What the pass DOES with a verdict is pinned next door, against the reference
//! model, in `707_the_judge_sees_the_situation_and_answers_a_verdict.rs` (Q-15,
//! S-019, S-032, S-033). This file keeps the three things that live outside the
//! model: the hive's wiring, the edge that turns a model's answer into the one
//! event of § 4.1, and the three drift locks on the prose -- the README's sentences
//! about the judge, the guideline the judge is actually handed, and what it is told
//! its own numbers do.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, repo};

const README: &str = "templates/display/README.md";

fn read_json(path: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The README as one line: its prose is hard-wrapped, and a sentence that spans
/// two lines is still one sentence.
fn readme_prose() -> String {
    std::fs::read_to_string(repo(README))
        .expect("README")
        .replace("**", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn params(judge: &str) -> Value {
    json!({"judge": judge, "judge_min_interval_ms": 3000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

fn weather(title: &str) -> Value {
    component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"title": title, "context": "weather", "relevance": "0.7",
                   "topic": "weather:berlin"}),
        ),
    )
}

/// The one judge question of the last pass, or none.
fn question(screen: &Screen) -> Option<&Value> {
    let asked = screen.lane("judge");
    assert!(asked.len() <= 1, "at most one question a pass");
    asked.first().copied()
}

/// A write that changes nothing is no content change, so the judge is not asked.
///
/// The name of this file, as a measurement: the brake of § 4.3 is not the only
/// thing that keeps the judge quiet. An ambient application that re-sends the
/// same view every twenty seconds would otherwise buy a verdict per tick.
#[test]
fn an_unchanged_write_asks_nothing_and_a_changed_one_asks() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params("on"));
    screen.write(weather("Sunny"), 100_000);
    assert!(
        question(&screen).is_some(),
        "a new window is a touch (§ 4.8 a)"
    );

    // Well past `judge_min_interval_ms`, so the brake is not what answers here.
    screen.write(weather("Sunny"), 110_000);
    assert!(
        question(&screen).is_none(),
        "identical props are no touch (§ 4.8 b)"
    );

    screen.write(weather("Rain"), 111_000);
    assert!(
        question(&screen).is_some(),
        "a changed own prop is a content change"
    );
}

/// The lane `in_verdict`: what the judge answered becomes the ONE event of § 4.1
/// at the edge, and never reaches the pass as a model's answer.
///
/// Since display 2.7.0 the cell holds the state in memory (GH #809), so the
/// verdict is a pass of its own right there, over the windows the cell knows:
/// no store round trip carries it any more. What is read is what it left
/// behind -- the state the pass wrote, the patch to the display.
///
/// Fenced JSON is tolerated (the memory hive's lesson). An error, and an answer
/// that is not JSON, change nothing at all: the floor has already drawn.
#[test]
fn the_verdict_lane_turns_an_answer_into_the_one_event() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params("on"));
    screen.write(weather("Sunny"), 100_000);
    screen.write(
        component_view(
            "b",
            "main",
            pane(
                "b",
                json!({"title": "Cloud", "context": "weather", "relevance": "0.7",
                       "topic": "weather:potsdam"}),
            ),
        ),
        110_000,
    );
    assert!(question(&screen).is_some(), "the judge was asked");

    let text = format!(
        "```json\n{}\n```",
        json!({"bar": 0.8, "weights": {"weather": 0},
               "windows": [{"id": "view.alex.a", "judged_hidden": true},
                           {"id": "view.alex.b", "judged_relevance": 0.95}]})
    );
    let drawn = screen.send(
        json!({
            "body": {"messages": [{"origin": "assistant", "type": "text", "text": text}]},
            "envelope": {"header": {
                "hop": {"route": "in_verdict", "finish_reason": "stop", "model": "x"},
                // The context of the turn that asked travels back on the reply. The
                // lane decides, not that origin: taken for the display's answer to a
                // read, it would reach a running cell as a stray reply and do nothing.
                "context": {"display_origin": "read"},
            }},
        }),
        111_000,
    );
    // `windows` became a map by id, and the two per-window words kept the names
    // § 3 gives them -- so § 4.5 read a verdict and not a wire format.
    let state = screen.screen_state();
    assert_eq!(state["bar"], 0.8);
    assert_eq!(state["weights"]["weather"].as_f64(), Some(0.0));
    assert_eq!(
        state["judge"]["verdict"]["at"], 111_000,
        "applied on arrival"
    );
    assert_eq!(
        state["views"]["view.alex.a"]["verdict"]["judged_hidden"],
        true
    );
    assert_eq!(
        state["views"]["view.alex.b"]["verdict"]["judged_relevance"],
        0.95
    );
    assert_eq!(screen.curator("view.alex.a", "rung"), "hidden");
    assert!(
        !drawn.is_empty(),
        "and the display got the verdict's picture in the same turn"
    );
    assert!(
        screen.hops().iter().all(|h| h["route"] != "read"),
        "a verdict reads nothing"
    );

    // An error and a non-JSON answer leave the state where it stands.
    let before = screen.screen_state();
    for (hop, body) in [
        (
            json!({"route": "in_verdict", "finish_reason": "error", "error_code": "timeout"}),
            json!({"messages": [], "meta": {"detail": "no model"}}),
        ),
        (
            json!({"route": "in_verdict", "finish_reason": "stop"}),
            json!({"messages": [{"origin": "assistant", "type": "text", "text": "I think so"}]}),
        ),
    ] {
        let nothing = screen.send(
            json!({"body": body, "envelope": {"header": {"hop": hop}}}),
            112_000,
        );
        assert!(nothing.is_empty(), "the floor stands: {nothing:?}");
        assert!(
            screen.hops().is_empty(),
            "nothing leaves: {:?}",
            screen.hops()
        );
        assert!(screen.last.is_empty(), "nothing is said: {:?}", screen.last);
        assert_eq!(screen.screen_state(), before, "the state is untouched");
    }
}

/// The hive carries the judge: an `llm` cell with an empty model as shipped,
/// a JSON answer asked for, a small budget, and two edges word for word.
#[test]
fn the_hive_wires_the_judge() {
    if !library_ships() {
        return;
    }
    let judge = read_json(&repo("templates/display/judge/config.json"));
    assert_eq!(judge["cell"]["type"], "llm");
    assert_eq!(judge["params"]["model"], "${DISPLAY_JUDGE_MODEL:-}");
    assert_eq!(judge["contract"]["settings"]["model"]["default"], "");
    assert_eq!(judge["contract"]["settings"]["api_key"]["secret"], true);
    assert_eq!(
        judge["params"]["provider_extra"]["response_format"]["type"],
        "json_object"
    );
    assert!(
        judge["params"]["max_tokens"]
            .as_u64()
            .is_some_and(|n| n <= 1000)
    );
    // Measured on a throw-away colony: with 4 s two verdicts in nine timed
    // out against a provider answering in 2-4 s (OR-C-Bau-9).
    assert_eq!(judge["params"]["external_timeout_ms"], 8000);
    assert!(
        judge["cell"]["message_timeout"]
            .as_u64()
            .is_some_and(|b| b > 8000 * 2),
        "the backstop stays well above the operation timeout"
    );
    assert_eq!(judge["params"]["system_order"], json!(["instructions"]));

    let hive = read_json(&repo("templates/display/config.json"));
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    for wanted in [
        json!({"from": "./compose", "to": "./judge",
               "condition": "has(hop.route) && hop.route == 'judge'"}),
        json!({"from": "./judge", "to": "./compose",
               "condition": "has(hop.finish_reason)",
               "modifier": {"set_hop": {"route": "'in_verdict'"}}}),
    ] {
        assert!(edges.contains(&wanted), "missing edge {wanted}");
    }
    assert_eq!(hive["params"]["ports"], json!([]));
}

/// What the README says about the judge, against what carries it
/// (`docs/development-rules.md` § 2d drift lock).
///
/// Since display@2.5.0 the README is the rendering of the display-hive
/// description, and it makes three claims about the judge and no others: the
/// knob that lets the curator ask at all, the interval between two calls, and
/// the four values a verdict may carry. The mechanism half of each is the
/// contract of the cell and the answer schema the judge is given -- the claims
/// the README once made about the lane `in_verdict` and about ticks are gone
/// from it, so they are gone from here.
#[test]
fn the_readme_names_the_judge() {
    if !library_ships() {
        return;
    }
    let readme = readme_prose();
    let settings =
        read_json(&repo("templates/display/compose/config.json"))["contract"]["settings"].clone();

    assert!(
        readme.contains("`on` lets the curator ask the judge cell"),
        "the knob"
    );
    assert_eq!(
        settings["judge"]["default"], "off",
        "and it is off as shipped"
    );

    assert!(
        readme.contains(
            "`judge_min_interval_ms` | 3000 | the shortest distance between two judge calls"
        ),
        "the interval"
    );
    assert_eq!(settings["judge_min_interval_ms"]["default"], 3000);

    assert!(
        readme.contains(
            "The judge writes four things and nothing else: `judged_relevance` and \
             `judged_hidden` per window, `bar` and `weights` on the state."
        ),
        "the four values"
    );
    let instructions = judge_instructions().expect("the guideline");
    for word in [
        "judged_relevance",
        "judged_hidden",
        "\"bar\"",
        "\"weights\"",
    ] {
        assert!(
            instructions.contains(word),
            "the answer schema asks for {word}"
        );
    }
    for word in ["rung", "score", "rank", "level"] {
        assert!(
            !instructions.contains(&format!("\"{word}\"")),
            "the judge is asked for {word}, which is the curator's (§ 4.5)"
        );
    }
}

/// The judge is told what its own numbers do (GH #726).
///
/// The guideline of § 1.3 tells a model what a screen is for; it says nothing about
/// the arithmetic the screen then runs. Measured on the live instance and its twin on
/// 18.09.2026: with the guideline alone, `openai/gpt-5.6-luna` put `bar` at 0.9 in
/// every one of forty verdicts and hid nearly every window at `judged_relevance` 0.05.
/// At that bar only `weight x relevance x decay >= 0.9` opens a window -- in practice
/// a context weighing 1.0 and a relevance of 0.9 -- so the window that ANSWERED the
/// person's question stayed a tile while the conversation kept the screen (§ 4.13),
/// and verdicts reversed within seconds with no event in between.
///
/// So four things are now in the prompt and are locked here: the arithmetic with its
/// threshold, the span a usable bar lives in, the answer's own window, and that a
/// verdict does not move without an event. `workshop/tools/judge_eval.py` measures
/// whether a model obeys them; this test only holds them in the file.
#[test]
fn the_judge_is_told_what_its_numbers_do() {
    if !library_ships() {
        return;
    }
    let Some(instructions) = judge_instructions() else {
        return;
    };
    for claim in [
        // The arithmetic and the comparison it feeds.
        "score = weight x relevance x decay",
        "LARGE when score >= bar",
        "threshold on a PRODUCT",
        // The span, because "high means a concentrated screen" is what produced 0.9.
        "A usable bar lies between 0.2 and 0.5",
        "0.3 is the normal choice",
        // A weight is not an off switch, and the conversation gets no bonus (§ 1.3).
        "A weight is not an off switch",
        "weighs at least 0.7",
        "`conversation` is not automatically 1.0",
        // The answer's window, and the conversation stepping back (§ 4.13).
        "`judged_relevance` of 0.8 or more",
        "do not hide it",
        "The conversation (topic `chat`)",
        "you do not have to hide it",
        // Stability: the last verdict is information, not an anchor, and the list of
        // events includes the one the judge is asked on most (§ 5.10).
        "not as an anchor",
        "a window whose content changed",
        "the same picture gets the same verdict",
        // `judged_hidden` is for what disturbs; a quiet clock is a low relevance.
        "what would disturb the person",
        "never with a high bar",
    ] {
        assert!(
            instructions.contains(claim),
            "the judge is told: {claim}\n{instructions}"
        );
    }
    // The sentence that invited the 0.9 is gone from the answer schema.
    assert!(
        !instructions.contains("high means an empty, concentrated screen"),
        "the schema no longer sells a high bar as concentration"
    );
    // The prompt travels with EVERY judge call, so its length is a running cost.
    // 3 863 characters as written; the cap is room to say more, not a target.
    assert!(
        instructions.len() <= 5_000,
        "the guideline is read on every call: {} characters",
        instructions.len()
    );
}

/// The instructions of the one judge question a fresh screen produces.
fn judge_instructions() -> Option<String> {
    let mut screen = Screen::new(params("on"));
    screen.write(weather("Sunny"), 100_000);
    Some(
        question(&screen)?["system"]["instructions"]["text"]
            .as_str()
            .expect("the guideline rides as instructions")
            .to_string(),
    )
}

/// The guideline the judge is handed is the guideline the README states
/// (`docs/development-rules.md` § 2d drift lock).
///
/// Both halves carry display-hive.md § 1, but not in the same voice: the judge
/// gets § 1 word for word, in the second person, while the README renders it as
/// numbered principles in its own prose. So what is locked is the load-bearing
/// clause of each principle, which both sides do spell identically -- a changed
/// claim breaks the lock, a re-wrapped paragraph does not. The guiding sentence
/// is matched without its first word, which the README lowercases mid-sentence.
#[test]
fn the_judge_reads_the_guideline_the_readme_states() {
    if !library_ships() {
        return;
    }
    let Some(instructions) = judge_instructions() else {
        return;
    };
    let readme = readme_prose();
    for claim in [
        "Focus is the state of the whole screen",
        "one highlighted element",
        "is only what is of use now",
        "At every real event the situation is judged",
        "can it go entirely",
        "as little as possible",
    ] {
        assert!(
            instructions.contains(claim),
            "the judge is told: {claim}\n{instructions}"
        );
        assert!(readme.contains(claim), "the README states it: {claim}");
    }
    // The one principle the judge needs and the README carries as its own
    // sentence: no sender has a bonus.
    assert!(
        instructions.contains("The display has no agenda of its own."),
        "the judge is told it has no agenda of its own"
    );
    assert!(
        readme.contains("the judge writes relevance and the bar, the curator writes the rung"),
        "and the README says where that line runs"
    );
}
