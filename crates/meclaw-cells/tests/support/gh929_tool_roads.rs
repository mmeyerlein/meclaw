//! The tool roads of GH #929's chain-length locks: the two segments only a
//! tool call opens, run on the member road of `gh929_member_road.rs`
//! (`crate::road`), every `llm` cell on a local stub.
//!
//! * S3 -- a history call. The surface's brain asks its own curator's wall
//!   (`history_search`) and the round comes back to the curator. The segment
//!   opens at the round's first delivery into the brain (the restoring
//!   `./curator -> ./brain` edge) and closes at the curator entry that carries
//!   the call's result back (`./collector -> ./curator` on `in_curate`): the
//!   brain's answer, the splitter, the dispatcher, the curator's `history`
//!   cell with its ledger round trips, the `tool_result`, the collector's
//!   fan-in and its window reads.
//! * S4 -- a consult. The surface's brain hands the core an errand
//!   (`consult_cogny`). The segment opens at the consult edge (`./talky ->
//!   ./cogny`, restoring, `in_turn`) and closes at the core's curator entry.
//!
//! No menu tick is sent. The menu decides what a brain is OFFERED
//! (`system.tools`, which the collector composes only on `in_menu_tick` and
//! `in_menu`); where a call GOES is decided by edges on `hop.tool_name`, and
//! no cell on either road reads the menu: the dispatcher names the tool, the
//! collector's fan-in files calls and results by their ids, the history cell
//! checks the name against its own argument table. The `llm` cell turns every
//! `tool_calls` entry of a response into a `tool_call` turn, offered or not,
//! so a stub brain's call travels exactly the road it would behind a menu.
//!
//! The functions measure and return; holding a segment to its reserve is the
//! including test's job, and so is the template guard (GH #49). The including
//! test file declares `#[path = "mock_openai.rs"] mod mock_openai;` and
//! `#[path = "support/gh929_member_road.rs"] mod road;` at its root.
#![allow(dead_code)]

use crate::mock_openai::{
    canned_chat_completion, canned_content_and_tool_calls, canned_tool_calls,
};
use crate::road::{
    self, Delivery, GENERATION, REPLY, Road, Run, Segment, chain_of, person, segment_before,
};
use meclaw_testing::mock_http::MockResponse;

/// The person's words. The history search looks for one of them, so the wall
/// the curator keeps holds a hit by the time the brain asks.
pub const TURN: &str = "Remember the probe for me.";

/// The id of the history call.
pub const HISTORY_ID: &str = "call-929-history";

/// A phrase search for a word of the turn, two neighbours per hit. Valid, so
/// the `history` cell reads its ledger instead of refusing the arguments on
/// the spot; and a hit with `context` walks every leg a search has: the first
/// page, the page again with its blocks, the neighbours, their blocks.
pub const HISTORY_ARGS: &str = r#"{"query": "probe", "mode": "phrase", "context": 2}"#;

/// The id of the consult call.
pub const CONSULT_ID: &str = "call-929-consult";

/// A complete order in the shape `consult_cogny` asks for.
pub const CONSULT_ARGS: &str = r#"{"question": "Say in one sentence what the person calls the probe.", "context": "Goal: remember the probe. Facts: the person asked to keep it. Constraints: none named. Form: one sentence. Length: short."}"#;

/// The sentence the surface says beside the consult (its interim answer).
pub const INTERIM: &str = "Let me ask my core.";

/// The core's answer to the errand.
pub const CORE_REPLY: &str = "The probe is the note the person asked to keep.";

/// An occupant of the generation, as a path.
fn node(name: &str) -> String {
    format!("{GENERATION}/{name}")
}

/// Whether the parent chain of `d` reaches a delivery for which `pred` holds.
fn reaches(log: &[Delivery], d: &Delivery, pred: impl Fn(&Delivery) -> bool) -> bool {
    segment_before(log, d, pred).is_some()
}

/// Where a road stopped, for a failure message: the parent chain of the
/// newest delivery for which `pred` holds.
fn newest_chain(run: &Run, what: &str, pred: impl Fn(&Delivery) -> bool) -> String {
    match run.log.iter().rfind(|d| pred(d)) {
        Some(d) => format!(
            "chain of the newest {what}:\n{}",
            chain_of(&run.log, d, 80).join("\n")
        ),
        None => format!("no {what} in the log"),
    }
}

/// The surface's brain of S3: the history call, then the answer once its
/// result is in.
fn history_brain() -> Vec<MockResponse> {
    vec![
        canned_tool_calls(vec![(HISTORY_ID, "history_search", HISTORY_ARGS)]),
        canned_chat_completion(REPLY, "stop"),
    ]
}

/// The surface's brain of S4: a sentence and the consult in one breath, then
/// the answer once the core's advice is in.
fn consult_brain() -> Vec<MockResponse> {
    vec![
        canned_content_and_tool_calls(INTERIM, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        canned_chat_completion(REPLY, "stop"),
    ]
}

/// The core's brain of S4: it answers the errand.
fn core_brain() -> Vec<MockResponse> {
    vec![canned_chat_completion(CORE_REPLY, "stop")]
}

/// S3 -- a talky turn with an audience whose brain calls `history_search`
/// first and answers after the result. Returns the segment from the round's
/// first delivery into the brain to the curator entry whose parent chain runs
/// through the history call, the run, and that closing delivery.
pub async fn s3_history_call() -> (Segment, Run, Delivery) {
    let talky = node("talky");
    let brain = format!("{talky}/brain");
    let curator = format!("{talky}/curator");
    let history = format!("{curator}/history");
    let run = road::run(Road {
        scripted: vec![("assistants/scribe/talky/brain".to_string(), history_brain())],
        turn: person("talky", TURN, true, 0),
        // The only answer is the one after the tool round, and the round comes
        // back through the closing seam before the brain is asked again.
        answers: 1,
    })
    .await;
    run.trace("S3");
    let opening = run
        .nth(0, |d| d.to == brain)
        .map(|d| d.id.clone())
        .unwrap_or_else(|| {
            panic!(
                "S3: the round never reached the brain; ttl_expired: {:?}; dead letters: {:?}",
                run.expired(),
                run.dead
            )
        });
    let through_the_call = |d: &Delivery| reaches(&run.log, d, |a| a.route == "in_history_call");
    let closing = run
        .nth(0, |d| {
            d.to == curator && d.route == "in_curate" && through_the_call(d)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "S3: no curator entry carries the history call's result back (`in_curate` at \
                 {curator} with `in_history_call` on its parent chain); answers: {}; \
                 ttl_expired: {:?}; dead letters: {:?}\n{}\n{}",
                run.answers.len(),
                run.expired(),
                run.dead,
                newest_chain(&run, "delivery into the history cell", |d| {
                    d.to.starts_with(history.as_str())
                }),
                newest_chain(&run, "curator entry", |d| {
                    d.to == curator && d.route == "in_curate"
                }),
            )
        });
    let seg = segment_before(&run.log, &closing, |d| d.id == opening).unwrap_or_else(|| {
        panic!(
            "S3: the parent chain of the curator entry after the history call does not reach \
             the round's first delivery into the brain ({opening}):\n{}",
            chain_of(&run.log, &closing, 80).join("\n")
        )
    });
    // The search found the turn (review T, M-4): a hit is what sends the
    // history cell past its first page into the neighbours (`search-rows`,
    // `search-page`, `search-near`, ...), one ledger round trip each. A search
    // without a hit answers after one or two and would measure a shorter road
    // without a word.
    let by_id: std::collections::HashMap<&str, &Delivery> =
        run.log.iter().map(|d| (d.id.as_str(), d)).collect();
    let (mut at, mut legs) = (Some(&closing), 0usize);
    while let Some(d) = at {
        if d.from == history && d.route == "lstore" {
            legs += 1;
        }
        if d.id == opening {
            break;
        }
        at = d.parent.as_deref().and_then(|p| by_id.get(p).copied());
    }
    eprintln!("gh929 S3 history call: ledger round trips on the chain={legs}");
    assert!(
        legs >= 3,
        "S3: the history search found the turn and read its neighbours (>= 3 ledger round \
         trips on the chain), not {legs}:\n{}",
        chain_of(&run.log, &closing, 80).join("\n")
    );
    (seg, run, closing)
}

/// S4 -- a talky turn with an audience whose brain consults the core. Returns
/// the segment from the consult edge's delivery into the core (`in_turn`) to
/// the core's first curator entry, the run, and that closing delivery.
pub async fn s4_consult() -> (Segment, Run, Delivery) {
    let cogny = node("cogny");
    let curator = format!("{cogny}/curator");
    let run = road::run(Road {
        scripted: vec![
            ("assistants/scribe/talky/brain".to_string(), consult_brain()),
            ("assistants/scribe/cogny/brain".to_string(), core_brain()),
        ],
        turn: person("talky", TURN, true, 0),
        // The surface's interim sentence leaves first. Its answer after the
        // advice needs the core's round, which opens at the core's curator
        // entry -- so by the second answer that entry stands in the log.
        answers: 2,
    })
    .await;
    run.trace("S4");
    let closing = run
        .nth(0, |d| d.to == curator && d.route == "in_curate")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "S4: the consult never reached the core's curator entry (`in_curate` at \
                 {curator}); answers: {}; ttl_expired: {:?}; dead letters: {:?}\n{}\n{}",
                run.answers.len(),
                run.expired(),
                run.dead,
                newest_chain(&run, "delivery into the core", |d| {
                    d.to.starts_with(cogny.as_str())
                }),
                newest_chain(&run, "delivery on the tool lane", |d| d.route == "tool"),
            )
        });
    let seg = segment_before(&run.log, &closing, |d| {
        d.to == cogny && d.route == "in_turn"
    })
    .unwrap_or_else(|| {
        panic!(
            "S4: the parent chain of the core's curator entry does not reach the consult's \
             delivery into the core ({cogny} on `in_turn`):\n{}",
            chain_of(&run.log, &closing, 80).join("\n")
        )
    });
    (seg, run, closing)
}
