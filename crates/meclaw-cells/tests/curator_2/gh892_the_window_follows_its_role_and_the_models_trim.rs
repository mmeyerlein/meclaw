//! GH #892, the colony case -- the window of a shipped composite follows its
//! role, and the model trims it through the sidecar of its own answer.
//!
//! Measured AT THE RECEIVER: the request bodies a stub provider recorded, the
//! rows of the curator's own ledger, the answers that left the composite and
//! the colony's dead letters. The SHIPPED `talky` and `cogny` are booted with
//! a stub behind every `llm` cell -- nothing here reaches a paid endpoint.
//!
//! talky (role `talky`, the brain stamping a two-second cache):
//!
//! 1. every foreign block the provider reads begins with its short id, the
//!    id of the wall block that holds it;
//! 2. while the cache is warm, the second request begins with the whole of
//!    the first -- short ids do not move the prefix;
//! 3. an answer whose sidecar releases the first turn's id changes nothing
//!    until the cache has gone cold; after the rebuild the request carries the
//!    one-line form in the block's place, and the ledger still holds the block
//!    byte for byte;
//! 4. the person never sees the block, and nothing dead-letters;
//! 5. the menu asked at boot reaches the curator, and its `window` section
//!    stands in the brain's system part (OR-KY-G1).
//!
//! cogny (role `consult`, the splitter it grew for the curator): an answer
//! with a block leaves as an advice without it, the block's `memory` section
//! leaves nothing but a `topic` mark in the core's ledger, and nothing
//! dead-letters.
//!
//! Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

// harness_fold: #[path = "support/composite_colony.rs"]
#[allow(unused_imports)]
use crate::composite_colony; // harness_fold: mod composite_colony;

use composite_colony::*;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use mock_openai::{MockOpenAI, canned_chat_completion};

const CHANNEL: &str = "chat:892";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;
const FIRST: &str = "please keep this long list of names in mind for a moment";
const SECOND: &str = "thanks, that is all about the list";
const THIRD: &str = "what else is new?";

const CONSULT_ID: &str = "k-892";
const CONSULT_SESSION: &str = "s-892";
const QUESTION: &str = r#"{"question":"which way does the river run?"}"#;
const ADVICE: &str = "It runs north, past the old mill.";

fn talky_turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn consult() -> Message {
    MessageBuilder::new(Path::new("/cogny"))
        .hop(map(json!({"route": "in_turn", "consult_id": CONSULT_ID,
                        "session_id": CONSULT_SESSION, "tool_name": "consult_cogny"})))
        .context(map(json!({"consult_id": CONSULT_ID,
                            "session_id": CONSULT_SESSION, "col_phase": ""})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": CONSULT_ID, "text": QUESTION}]})))
        .ttl(400)
        .build()
}

/// talky: short ids on the wire, a warm prefix that holds, and a release
/// that becomes one line after the cache went cold -- never before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh892_the_window_follows_its_role_and_the_models_trim() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // A day that turns during the run would move the talky's day tiers under
    // the assertions: the curator tests' one midnight guard (GH #1033/#1062),
    // a wait for the clock, not for a result.
    curator_hive::clear_of_midnight();
    let first = curator_hive::user(FIRST);
    let first_id = curator_hive::short_id(&first);
    let release = format!(
        "Done, the list can go.\n\n```sidecar\n{{\"window\": {{\"release\": [\"#{first_id}\"]}}}}\n```"
    );
    let brain = MockOpenAI::start(vec![
        canned_chat_completion("Noted, I have the list.", "stop"),
        canned_chat_completion(&release, "stop"),
        canned_chat_completion("Nothing else is new.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    // The brain stamps a cache that goes cold two seconds after each call:
    // the curator's clock is ordered on that stamp (GH #890).
    build(
        &td,
        "talky",
        &brain.base_url,
        &background.base_url,
        json!({"cache_mode": "implicit", "cache_ttl_s": 2}),
    );
    ask_the_menu_at_boot(&td, "talky");
    let (h, mut ports) = boot(&td).await;
    // D58 target (contract point 13): the model cells report their profiles
    // before the first turn, as the registry's push after the birth does.
    report_packages_at_boot(&td, &h, "talky").await;
    let ledger = td.path().join("main/talky/curator/ledger/cell.db");
    // 5. The curator answered the menu: its `window` section stands in the
    //    sidecar contract the collector filed with the curator.
    until_row(
        &ledger,
        "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash \
         WHERE s.path = 'instructions.sidecar'",
        "[#0123456789ab]",
        "the curator's menu answer reached the sidecar contract",
    )
    .await;

    h.send(talky_turn(FIRST)).await;
    answer_or_explain(&mut ports.sink, &h, "turn 1").await;
    h.send(talky_turn(SECOND)).await;
    let second = answer_or_explain(&mut ports.sink, &h, "turn 2").await;
    let reqs = provider_requests(&brain, &h, 2).await;

    // 1. The first turn's words, behind the id of the wall block that holds them.
    let one = messages(&reqs[0]);
    let two = messages(&reqs[1]);
    // 5, at the receiver: the brain reads the `window` section and how to
    // write it in its system part (OR-KY-G1, the curator as menu answerer).
    let system: String = one
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(
        system.contains("[#0123456789ab]") && system.contains("\"window\""),
        "the brain's system part carries the curator's `window` section: {system}"
    );
    let shown = format!("[#{first_id}] {FIRST}");
    assert!(
        one.iter()
            .any(|m| m["role"] == "user" && m["content"] == json!(shown)),
        "the first request shows the turn behind its short id: {one:?}"
    );
    // 2. Warm: the second request begins with the whole of the first.
    assert_eq!(
        &two[..one.len()],
        one.as_slice(),
        "short ids do not move the prefix a provider's cache keys on"
    );
    // 4. The person hears the prose, never the block.
    let said = texts_of(&second);
    assert!(
        said.iter().any(|t| t == "Done, the list can go.")
            && !said
                .iter()
                .any(|t| t.contains("```") || t.contains("release")),
        "the answer leaves without its block: {said:?}"
    );
    // The release is a mark in the ledger at once ...
    until_row(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'release'",
        &first_id,
        "the window section reached the curator as a `release` mark",
    )
    .await;
    // ... and the window at the next rebuild: after the cache went cold. The
    // plan is the round's (`window_plan:<round key>`, GH #943).
    until_row(
        &ledger,
        // GH #1448: the plan of blocks the answers write ahead names the
        // turn too -- the rebuild's plan is the one with a cover.
        "SELECT value FROM state WHERE key LIKE 'window_plan:%' AND value LIKE '%\"cover\":%'",
        &first_id,
        "a cold cache rebuilt the window with the release in its plan",
    )
    .await;
    h.send(talky_turn(THIRD)).await;
    answer_or_explain(&mut ports.sink, &h, "turn 3").await;
    let reqs = provider_requests(&brain, &h, 3).await;
    let three = messages(&reqs[2]);
    let line = format!("[#{first_id} released \u{2014} history_read(\"#{first_id}\")]");
    assert!(
        three
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == json!(line)),
        "after the rebuild the block is one line: {three:?}"
    );
    assert!(
        !three
            .iter()
            .any(|m| m["content"].as_str().is_some_and(|c| c.contains(FIRST))),
        "and its words are gone from the window: {three:?}"
    );
    // 3. The ledger holds the block byte for byte.
    let body = rows(
        &ledger,
        &format!("SELECT body FROM blocks WHERE hash LIKE '{first_id}%' AND kind = 'user'"),
    );
    assert_eq!(
        body,
        vec![vec![curator_hive::canonical(&first)]],
        "the released block stands in the ledger as it came"
    );
    // "Nothing dead-lettered" is read once the colony settled (TV-A2): every
    // delivery that follows from the three turns has been handed over.
    h.quiesce().await;
    let dead = dead_letters(&h).await;
    while ports.park.try_recv().is_ok() {}
    h.shutdown().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

/// cogny: the splitter it grew cuts the block out of the advice, and the
/// block's sections stay in the core.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh892_cogny_cuts_its_block_and_keeps_its_sections() {
    if !shipped() {
        return;
    }
    let with_block = format!(
        "{ADVICE}\n\n```sidecar\n{{\"memory\": {{\"nothing_new\": false, \"facts\": [], \
         \"topic\": {{\"movement\": \"start\", \"name\": \"the river\"}}}}}}\n```"
    );
    let brain = MockOpenAI::start(vec![canned_chat_completion(&with_block, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        "cogny",
        &brain.base_url,
        &background.base_url,
        json!({}),
    );
    let (h, mut ports) = boot(&td).await;
    // D58 target (contract point 13): the model cells report their profiles
    // before the first turn, as the registry's push after the birth does.
    report_packages_at_boot(&td, &h, "cogny").await;
    let ledger = td.path().join("main/cogny/curator/ledger/cell.db");

    h.send(consult()).await;
    let advice = answer_or_explain(&mut ports.sink, &h, "cogny").await;
    let said = texts_of(&advice);
    assert!(
        said.iter().any(|t| t == ADVICE)
            && !said
                .iter()
                .any(|t| t.contains("```") || t.contains("topic")),
        "the advice leaves without its block: {:?}",
        body_of(&advice)
    );
    until_row(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'topic'",
        r#"{"movement":"start","name":"the river"}"#,
        "the memory section left its topic mark in the core's ledger",
    )
    .await;
    // "No section leaves" is read once the colony settled (TV-A2): every
    // delivery that follows from the advice has been handed over to the park.
    h.quiesce().await;
    let mut parked = Vec::new();
    while let Ok(m) = ports.park.try_recv() {
        parked.push(m.headers.hop.clone());
    }
    let dead = dead_letters(&h).await;
    h.shutdown().await;
    assert!(
        !parked
            .iter()
            .any(|hop| hop.get("route").and_then(Value::as_str) == Some("sidecar")),
        "no section leaves the core: {parked:?}"
    );
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}
