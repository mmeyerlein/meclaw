//! The judge seam: the question leaves, a verdict comes back, and the NEXT pass reads it.
//!
//! `compose.py` decides whether to ask (§ 4.3) and builds what the judge sees (§ 4.4), and
//! a subprocess test can read that emission. What it cannot show is the return leg: that
//! the `llm` cell beside the compose cell really carries the question to a provider, that
//! the answer really comes back on `in_verdict`, and that the verdict really becomes the
//! bar, the weights and a closed window of the pass after it. Nor can it show the brake:
//! `judge_min_interval_ms` is a distance in REAL time between two calls, and real time is
//! what a subprocess does not have.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "judge"): **Q-06** and **S-032** (a
//! content change asks, a gesture does not), **S-033** and **S-074** (`bar` and `weights`
//! are the verdict's), **Q-09** and **Q-15** (a judged window and what the judge saw),
//! **S-019** (the weights reach the score).
//!
//! The provider is a loopback mock that answers ONE fixed verdict, for ever. Never a real
//! model: a throwaway colony gets no keys.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, boot, curator, have_python, library_ships};
use meclaw_core::serde_json::json;

const APP: &str = "/alex/apps/note";

/// The window the fixed verdict closes. Its id is what a verdict names (§ 2 Id), so it is
/// spelled here and written below.
const DIM_OID: &str = "view.~alex~apps~note.dim";

/// § 4.3: the shortest distance between two calls. Held over real time, so it is the one
/// tight number in this file and it is why the second write below asks nothing.
const BRAKE: Duration = Duration::from_millis(3000);

const QUIET: Duration = Duration::from_millis(300);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verdict_comes_back_from_the_judge() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A bar well above `focus_default` and weights nobody would guess: a state that carries
    // them can only have got them from the answer.
    let colony = boot(Boot {
        judge: true,
        judge_min_interval_ms: BRAKE.as_millis() as u64,
        // Long, so nothing in this file is the clock's doing.
        linger_ms: 60_000,
        fade_ms: 120_000,
        verdict: json!({
            "bar": 0.8,
            "weights": {"work": 0.9, "ambient": 0.1},
            "windows": [{"id": DIM_OID, "judged_hidden": true}]
        }),
    })
    .await;
    let note = colony.oid(APP, "note");
    let dim = colony.oid(APP, "dim");
    assert_eq!(
        dim, DIM_OID,
        "the id in the verdict is the id of the window"
    );

    // -- Q-06, S-032: a window's arrival is a real event, and it asks -----------------
    // `dim` goes up FIRST because the verdict names it: the words of § 4.5 are said about
    // the windows that stand when the answer arrives, and they stay on the window
    // afterwards (which is what the second half of this file leans on).
    colony
        .put(
            APP,
            "dim",
            json!({"title": "Dim", "context": "work", "relevance": "0.9",
                   "topic": "dim:1", "touched": "1"}),
        )
        .await;
    let judged = colony
        .wait_state("the verdict comes back and reaches the next pass", |s| {
            s["bar"] == json!(0.8)
        })
        .await;

    // -- S-033, S-074: the bar and the weights are the judge's, not the floor's -------
    assert_eq!(
        judged["weights"]["work"],
        json!(0.9),
        "the weights of the verdict stand on the state (§ 4.5): {judged}"
    );
    assert_eq!(judged["weights"]["ambient"], json!(0.1));
    assert!(
        !judged["judge"]["verdict"].is_null(),
        "and the state remembers that a verdict stands"
    );
    assert_eq!(
        judged["dials"]["settings"]["focus_default"],
        json!(0.3),
        "the floor did not move -- 0.8 came from the answer, not from the dial"
    );

    // -- Q-15: what the judge was asked is the situation, and it really left ----------
    let asked = colony.judge_asks.lock().await.len();
    assert_eq!(
        asked,
        colony.judge_questions().await,
        "every question the hive sent reached the provider"
    );
    assert!(asked >= 1, "and at least one did");
    let body = String::from_utf8_lossy(&colony.judge_asks.lock().await[0].body).to_string();
    assert!(
        body.contains("\\\"bar\\\"") || body.contains("bar"),
        "the judge is shown the screen state (§ 4.4): {body}"
    );
    assert!(
        body.contains(&dim),
        "with the window it is to judge, by id: {body}"
    );

    // -- Q-09, § 4.14: the verdict closes the window it named -------------------------
    assert_eq!(
        judged["views"][&dim]["verdict"]["judged_hidden"],
        json!(true),
        "the window carries the word the judge said about it (§ 3): {judged}"
    );
    assert_eq!(
        curator(&judged, &dim, "score"),
        json!(0.0),
        "a window the verdict hides scores nothing (§ 4.14)"
    );
    assert_eq!(curator(&judged, &dim, "open"), json!(false));

    // -- § 4.3: the brake holds over real time ---------------------------------------
    // A second real event, taken well inside `judge_min_interval_ms`: the window arrives,
    // the pass runs, and the judge is NOT asked.
    let before = colony.judge_questions().await;
    let standing = colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    colony.settle(QUIET).await;
    assert_eq!(
        colony.judge_questions().await,
        before,
        "a second real event inside the brake asks nothing (§ 4.3)"
    );

    // -- S-019, § 4.14: the weight really reaches the score --------------------------
    // `w x relevance x decay` = 0.9 x 0.9 x 1 = 0.81, which clears the bar of 0.8. The
    // pass that computed it asked nobody: the verdict of the pass before it still stands.
    assert_eq!(
        curator(&standing, &note, "score"),
        json!(0.81),
        "the verdict's weight is a factor of the score: {standing}"
    );
    assert_eq!(
        curator(&standing, &note, "open"),
        json!(true),
        "and 0.81 clears the bar of 0.8 (§ 4.16)"
    );
    assert_eq!(
        curator(&standing, &dim, "open"),
        json!(false),
        "while the closed one stays closed: a verdict stands until the next (§ 4.5)"
    );

    // -- § 4.3: past the brake, a real event asks again -------------------------------
    tokio::time::sleep(BRAKE).await;
    colony
        .put(
            APP,
            "late",
            json!({"title": "Late", "context": "work", "relevance": "0.7",
                   "topic": "late:1", "touched": "1"}),
        )
        .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if colony.judge_questions().await > before {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "past the brake the next real event asks again (§ 4.3)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    colony.shutdown().await;
}

/// A verdict is a pass too, and one that comes back between two writes loses no window.
///
/// What this case prevents is the lost window: the answer travels the same road as a write
/// and takes the same time, so the pass that takes it can be handed a state row that
/// predates the write beside it -- and a window that state row never saw would be gone for
/// ever, although the judge decides what is OPEN and never what exists (§ 4.11).
/// `reconcile()` (§ 3.1, OR-H0.9, H1-F4) catches the state up with the store's rows before
/// the pass's own event runs, and carries this case;
/// `707_the_state_is_reconciled_with_the_store.rs` pins the rule itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verdict_between_two_writes_loses_no_window() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A bar no dial of this colony carries (`focus_default` is 0.3), so "the verdict is in
    // this state row" is a measurable answer. The verdict names no window at all: what is
    // asserted below is existence, and about that the judge has no word.
    let colony = boot(Boot {
        judge: true,
        // The brake wide open, so every pass may ask and answers keep coming back INTO the
        // writes below rather than politely after them.
        judge_min_interval_ms: 1,
        linger_ms: 60_000,
        fade_ms: 120_000,
        verdict: json!({"bar": 0.25, "weights": {"work": 0.9}, "windows": []}),
    })
    .await;
    let first = colony.oid(APP, "first");
    let second = colony.oid(APP, "second");

    // Write A. Its pass asks, and the answer is on its way back while the next line
    // already writes -- nothing here waits for the colony to go quiet.
    colony
        .put(
            APP,
            "first",
            json!({"title": "First", "context": "work", "relevance": "0.9",
                   "topic": "first:1", "touched": "1"}),
        )
        .await;
    // Write B, into the same breath.
    colony
        .put(
            APP,
            "second",
            json!({"title": "Second", "context": "work", "relevance": "0.9",
                   "topic": "second:1", "touched": "1"}),
        )
        .await;

    // The verdict came back, and it took neither window with it.
    let held = colony
        .wait_state(
            "the verdict reaches a pass and both windows are still in the state",
            |s| {
                s["bar"] == json!(0.25)
                    && s["views"].get(&first).is_some()
                    && s["views"].get(&second).is_some()
            },
        )
        .await;
    assert_eq!(
        held["weights"]["work"],
        json!(0.9),
        "the weights are the verdict's, so this state row really took the answer: {held}"
    );
    for oid in [&first, &second] {
        assert_eq!(
            curator(&held, oid, "present"),
            json!(true),
            "{oid} is still on the screen after the verdict: {held}"
        );
    }

    colony.shutdown().await;
}
