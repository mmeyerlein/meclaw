//! GH #1225 (P1-A) -- every fact of an agent round keeps its source, in one
//! colony.
//!
//! The pins of `gh1225_every_sidecar_fact_is_checked_at_its_own_source.rs` run
//! each shipped script over stdin. This one runs the road they make together:
//! a member's generation booted from the SHIPPED templates (the member road of
//! `support/gh929_member_road.rs`: the assistant's talky with its splitter,
//! curator and error drain, the shipped memory hive, the member's own edges
//! between them), every `llm` cell on a local stub, and the splitter's knob as
//! it ships -- the `quote` of every `memory` fact checked at the source the
//! fact names.
//!
//! Two rounds with no person row of their own, each with two facts:
//!
//! 1. **A consult round.** The person asks, the talky consults its core, and
//!    the core's advice comes back as a round of its own (the collector's
//!    frame `[advice from your reasoning core, consult <id>]`). Its answer
//!    names one fact of the advice (`agent:cogny`) and one of the person's
//!    words of the episode the advice answers (`person`).
//! 2. **A timer round.** A turn framed `[timer <id>]` opens the round. Its
//!    answer names one fact of the timer's words (`agent:timer:<id>`) and one
//!    it claims of the person, on words nobody said.
//!
//! What is pinned: the memory holds exactly the three provable facts, each in
//! the source column its item named (`agent:cogny`, the person's `""`,
//! `agent:timer:<id>`); the fourth leaves as one refusal on the talky's
//! `error` lane (`error_source` `sidecar_verify`, `quote_not_in_source`,
//! never its words) and one `verify_reject` mark -- so the items scripted are
//! the items filed plus the items refused, and none is lost silently; and
//! each round's `topic` mark carries the source its topic named.
//!
//! Free of a real provider by construction. Guarded like every
//! template-reading test (GH #49): a tree that does not carry one of the
//! templates is skipped, never judged.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "support/gh929_member_road.rs"]
#[allow(unused_imports)]
use crate::road; // harness_fold: mod road;

use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Body, Message};
use meclaw_testing::deadline::HANG;
use meclaw_testing::wait::until_some;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use road::Stubs;
use std::collections::{BTreeSet, HashMap};
use tokio::sync::mpsc;

const TALKY_BRAIN: &str = "assistants/scribe/talky/brain";
const COGNY_BRAIN: &str = "assistants/scribe/cogny/brain";

const ASKS: &str = "Can you plan a quiet weekend for me and my dog Bruno?";
const INTERIM: &str = "Let me ask my core.";
const CONSULT_ID: &str = "call-c1225";
const CONSULT_ARGS: &str = r#"{"question": "Plan a quiet weekend for the person and the dog.", "context": "Goal: a quiet weekend. Facts: a dog named Bruno. Constraints: none named. Form: a short plan. Length: keep the answer under 4000 characters when you can."}"#;
const COGNY_SAYS: &str = "Saturday a walk by the river with Bruno, Sunday a book in the garden.";
const TALKY_SAYS: &str = "A walk by the river on Saturday and a book on Sunday.";

const TIMER_ID: &str = "t-1225";
const TIMER_SAYS: &str = "[timer t-1225] Time to water the tomatoes on the balcony.";
const TIMER_ANSWERS: &str = "Your tomatoes on the balcony need water now.";

const ADVICE_CLAIM: &str = "A walk by the river suits the weekend.";
const PERSON_CLAIM: &str = "The person has a dog named Bruno.";
const TIMER_CLAIM: &str = "The person grows tomatoes on the balcony.";
const UNSAID_CLAIM: &str = "The person hates gardening.";
const UNSAID_QUOTE: &str = "I hate gardening and always have";

fn fact(subject: &str, claim: &str, quote: &str, source: &str) -> Value {
    json!({"subject": subject, "predicate": "is", "claim": claim, "quote": quote,
           "fact_kind": "world", "valid_from": null, "source": source})
}

/// An answer with the memory section the shipped offer asks for.
fn with_memory(text: &str, facts: Value, topic: Value) -> String {
    let block = json!({"memory": {"facts": facts, "topic": topic}});
    format!("{text}\n\n```sidecar\n{block}\n```")
}

fn said(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Until an answer saying `needle` leaves the generation.
async fn answer_saying(sink: &mut mpsc::Receiver<Message>, needle: &str) {
    let found = tokio::time::timeout(HANG, async {
        while let Some(m) = sink.recv().await {
            if said(&m).contains(needle) {
                return true;
            }
        }
        false
    })
    .await;
    assert!(
        matches!(found, Ok(true)),
        "HANG: no answer saying {needle:?} left the generation"
    );
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .unwrap_or_default()
}

/// The rows of `sql` once at least `n` stand.
async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) -> Vec<Vec<String>> {
    let (db, sql) = (db.to_path_buf(), sql.to_string());
    until_some(what, || {
        let got = rows(&db, &sql);
        async move { (got.len() >= n).then_some(got) }
    })
    .await
}

/// Every refusal of the word-for-word check that reached the member's
/// drain, read until `n` stand.
async fn refusals(park: &mut mpsc::Receiver<Message>, n: usize) -> Vec<Message> {
    let mut out = Vec::new();
    let done = tokio::time::timeout(HANG, async {
        while out.len() < n {
            match park.recv().await {
                Some(m) if m.headers.hop.get("error_source") == Some(&json!("sidecar_verify")) => {
                    out.push(m)
                }
                Some(_) => {}
                None => return,
            }
        }
    })
    .await;
    assert!(
        done.is_ok() && out.len() >= n,
        "HANG: {n} refusal(s) of the check on the member's drain, got {}",
        out.len()
    );
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_fact_of_an_agent_round_keeps_its_source() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let timer_source = format!("agent:timer:{TIMER_ID}");
    let consult_facts = json!([
        fact(
            "the weekend",
            ADVICE_CLAIM,
            "a walk by the river",
            "agent:cogny"
        ),
        fact("Bruno", PERSON_CLAIM, "my dog Bruno", "person"),
    ]);
    let timer_facts = json!([
        fact(
            "the tomatoes",
            TIMER_CLAIM,
            "water the tomatoes on the balcony",
            &timer_source
        ),
        fact("gardening", UNSAID_CLAIM, UNSAID_QUOTE, "person"),
    ]);
    let scripted_items = 4;
    let talky = MockOpenAI::start(vec![
        canned_content_and_tool_calls(INTERIM, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        canned_chat_completion(
            &with_memory(
                TALKY_SAYS,
                consult_facts,
                json!({"movement": "start", "name": "the weekend", "source": "person"}),
            ),
            "stop",
        ),
        canned_chat_completion(
            &with_memory(
                TIMER_ANSWERS,
                timer_facts,
                json!({"movement": "continue", "name": "the weekend", "source": timer_source}),
            ),
            "stop",
        ),
    ])
    .await;
    let cogny = MockOpenAI::start(vec![canned_chat_completion(COGNY_SAYS, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(road::BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        scripted: HashMap::from([
            (TALKY_BRAIN.to_string(), talky.base_url.clone()),
            (COGNY_BRAIN.to_string(), cogny.base_url.clone()),
        ]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = road::build_member(&td, &stubs);
    for cell in [TALKY_BRAIN, COGNY_BRAIN] {
        assert!(
            pointed.iter().any(|c| c == cell),
            "{cell} is an llm cell of the road: {pointed:?}"
        );
    }
    let memory = td.path().join("main/memory-hive/store/cell.db");
    let ledger = td
        .path()
        .join("main/assistants/scribe/talky/curator/ledger/cell.db");
    let (h, mut ports) = road::boot(&td).await;

    // Round 1: the person asks, the core advises, the answer of the advice
    // round carries the section.
    h.send(road::person("talky", ASKS, true, 0)).await;
    answer_saying(&mut ports.sink, TALKY_SAYS).await;
    // Round 2: a timer opens the round; no person row of its own.
    h.send(road::person("talky", TIMER_SAYS, true, 0)).await;
    answer_saying(&mut ports.sink, TIMER_ANSWERS).await;

    // Filed: the three provable facts, each under the source it named.
    let timer_fact = until_rows(
        &memory,
        &format!("SELECT claim FROM facts WHERE source = '{timer_source}'"),
        1,
        "the timer round's own fact reaches the memory",
    )
    .await;
    assert_eq!(timer_fact, vec![vec![TIMER_CLAIM.to_string()]]);
    let filed = until_rows(
        &memory,
        "SELECT claim, source FROM facts ORDER BY claim",
        3,
        "the consult round's two facts reach the memory",
    )
    .await;
    let filed: BTreeSet<(String, String)> = filed
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let expected: BTreeSet<(String, String)> = [
        (ADVICE_CLAIM.to_string(), "agent:cogny".to_string()),
        (PERSON_CLAIM.to_string(), String::new()),
        (TIMER_CLAIM.to_string(), timer_source.clone()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        filed, expected,
        "every fact stands in the source column its item named -- the advice never as the \
         person's, the person's words of the episode as the person's, the timer's as the timer's"
    );

    // Refused: the timer round's claim about the person, on words nobody
    // said -- one report, its class and nothing of its words.
    let refused = refusals(&mut ports.park, 1).await;
    let report = &refused[0];
    assert_eq!(
        report.headers.hop.get("reject_reason"),
        Some(&json!("quote_not_in_source")),
        "{:?}",
        report.headers.hop
    );
    let whole = format!("{:?} {:?}", report.headers, report.body);
    for words in [UNSAID_CLAIM, UNSAID_QUOTE] {
        assert!(
            !whole.contains(words),
            "a refusal never carries the item's words: {whole}"
        );
    }

    // None lost silently: every scripted item is filed or refused.
    let marks = until_rows(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'verify_reject' ORDER BY seq",
        1,
        "the refusal's ledger mark",
    )
    .await;
    assert_eq!(marks.len(), 1, "one mark per refusal: {marks:?}");
    let mark: Value = sj::from_str(&marks[0][0]).expect("a verify_reject value is JSON");
    assert_eq!(mark["reason"], "quote_not_in_source", "{mark}");
    assert_eq!(mark["source"], "person", "{mark}");
    assert!(
        !marks[0][0].contains(UNSAID_QUOTE),
        "a mark never carries the words: {mark}"
    );
    assert_eq!(
        filed.len() + refused.len(),
        scripted_items,
        "items scripted = items filed + items refused"
    );

    // Each round's topic mark carries the source its topic named.
    let topics = until_rows(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'topic' ORDER BY seq",
        2,
        "the topic marks of both rounds",
    )
    .await;
    let sources: Vec<Value> = topics
        .iter()
        .map(|r| sj::from_str::<Value>(&r[0]).expect("a topic mark is JSON")["source"].clone())
        .collect();
    assert!(
        sources.contains(&json!("person")) && sources.contains(&json!(timer_source)),
        "the topic marks name their sources: {topics:?}"
    );
    h.shutdown().await;

    // The knob as it ships: nothing in this test sets it. Asserted last, so a
    // tree without the source rule fails at the source column above, not here.
    let knob = road::read_json(
        &td.path()
            .join("main/assistants/scribe/talky/splitter/config.json"),
    )["params"]["sidecar_verify"]
        .clone();
    assert_eq!(
        knob,
        json!({"memory": {"items": "facts", "quote": "source"}}),
        "the generation runs the shipped source rule"
    );
    let dead = road::dead_letters(td.path());
    assert!(
        dead.iter()
            .all(|(from, _, _)| !from.contains("/curator") && !from.contains("/errors")),
        "the check dead-letters nothing: {dead:?}"
    );
}
