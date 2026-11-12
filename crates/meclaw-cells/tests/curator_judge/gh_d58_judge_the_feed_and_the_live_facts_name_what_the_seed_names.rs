//! D58 judge (GH #1480, contract point 5, review I3, OR-d58-2.judge.16) --
//! the live feed and the live facts name of the session target's family
//! what the seed's fact line names.
//!
//! WHY this file exists: review 1 of the strand found that `./policy` put
//! the whole family into the feed (`parts: {<source>: {text: <the whole
//! contribution, a structured one as its JSON>}}`) and `./history` gave one
//! live fact per source with the whole JSON text, while the seed's fact line
//! names only what a contribution names (`leaf_names`: the `name` of each
//! entry of a structured list, else the text). Seed and feed said one thing
//! two ways, a change of the voice's prompt the plan never named. Before the
//! strand (abbbd811) the feed carried the screen topics' names alone
//! (`screen_topics: [<name>, ...]`). Now all three read the family through
//! the same function: the feed carries `parts: {<source>: [<name>, ...]}`, a
//! live fact per source carries the names joined as the fact line joins them.
//!
//! The hive in one process (`support/curator_hive.rs`), the start window's
//! target `decide` set on `./history` and `./policy` (`seed_target`), the
//! presenter's topics as its contribution on `in_part`; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

/// The presenter's topics as one structured value (`support/history_view.rs`
/// holds the same pair).
fn screen_value() -> Value {
    json!([
        {"name": "weather", "purpose": "the weather today and the next days",
         "candidates": [{"key": "today", "describe": "today's weather"},
                        {"key": "week", "describe": "the week ahead"}]},
        {"name": "calendar", "purpose": "the person's appointments"}
    ])
}

fn contribute(h: &mut Hive, value: Value) {
    h.lane(
        "in_part",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "target": "decide", "source": "presenter", "value": value}),
    );
}

/// The facts of the running session's end (`in_live_end` under the handle).
fn live_facts(h: &mut Hive) -> Value {
    h.out.clear();
    h.lane(
        "in_live_end",
        json!({"live": LIVE, "audience_set": ROUND_E}),
        json!({"live": LIVE, "session_id": LIVE, "final": true, "seconds": 60}),
        json!({"messages": []}),
    );
    let answers = h.routed("live_facts");
    assert_eq!(answers.len(), 1, "{:?} {:?}", h.out, h.stderr);
    answers[0].body["facts"].clone()
}

/// The live facts of the source `presenter`, their texts.
fn presenter_facts(facts: &Value) -> Vec<Value> {
    facts
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f["source"] == "presenter")
        .collect()
}

#[test]
fn gh_d58_judge_the_feed_and_the_live_facts_name_what_the_seed_names() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[
        ("history", "seed_target", json!("decide")),
        ("policy", "seed_target", json!("decide")),
    ]);
    clear_of_midnight();
    contribute(&mut h, screen_value());

    // The start window names the topics by name.
    let seeds = ask_seed(&mut h, &live_keys(LIVE), "start");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let line = first_line(&seeds[0]);
    assert!(
        line.contains("presenter: weather, calendar"),
        "the seed's fact line names the topics: {line}"
    );

    // The live facts of that session say the same, a fact per source.
    let facts = presenter_facts(&live_facts(&mut h));
    assert_eq!(
        facts,
        vec![json!({"source": "presenter", "id": "", "text": "weather, calendar"})],
        "the live fact names what the fact line names, not the whole contribution"
    );
    for f in &facts {
        let said = format!(
            "{}: {}",
            f["source"].as_str().unwrap_or(""),
            f["text"].as_str().unwrap_or("")
        );
        assert!(
            line.contains(&said),
            "{said:?} stands in the fact line {line}"
        );
    }

    // The screen changes: the feed of the next closed turn names the new topics.
    contribute(
        &mut h,
        json!([{"name": "news", "purpose": "the news of the day",
                "candidates": [{"key": "local", "describe": "the local news"}]}]),
    );
    closed_turn(&mut h, "s1", "t5", &live_keys(LIVE));
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        feeds[0].body["parts"],
        json!({"presenter": ["news"]}),
        "the feed names the topics as the fact line does, nothing of their purpose: {:?}",
        feeds[0].body
    );

    // The next start window and the live facts after the feed agree with it.
    let facts = presenter_facts(&live_facts(&mut h));
    assert_eq!(
        facts,
        vec![json!({"source": "presenter", "id": "", "text": "news"})],
        "the live facts after the feed"
    );
    let seeds = ask_seed(&mut h, &live_keys(LIVE), "renew");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let line = first_line(&seeds[0]);
    assert!(line.contains("presenter: news"), "{line}");
}
