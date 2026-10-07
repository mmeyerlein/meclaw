//! GH #976 (PE-DP-9) -- the last result of the member's apps shows on the screen only to
//! the round it was made for: the presenter's built-in topics `digest` and `research` read
//! the SHIPPED `daily-digest` and `research-assistant` over the road the builder renders,
//! and what reaches the screen is what that round may see.
//!
//! # Why this file exists
//!
//! The pure locks (`gh976_a_result_is_kept_with_its_round.rs`) prove what each script
//! says, and the hive-level lock (`gh976_a_digest_is_kept_with_its_round_and_shown_only_to_it.rs`)
//! proves what leaves each hive's rim. Neither shows the whole road: the presenter's
//! `resident_read` leaving its rim, the builder's edge restamping it onto the app's
//! `in_read` with the MEMBER's round, the app answering only the rows that round covers,
//! each with its own round, and the stage drawing the card or the list out of that answer.
//! A digest kept for another round (another member's run) must never become this member's
//! card, and a research answer to another round's question must never stand in this
//! member's list. This file boots that road on a colony and measures at the receivers.
//!
//! # What is booted
//!
//! The presenter colony of `support/presenter_colony.rs` (the shipped presenter under
//! `/alex/apps/presenter`, a held decider, the shipped display), and then, by ONE ordinary
//! mutation at the member's scope `/alex`:
//!
//! - the SHIPPED `daily-digest` as `/alex/apps/daily-digest` and the SHIPPED
//!   `research-assistant` as `/alex/apps/research-assistant`, neutralised the way the
//!   hive-level lock does it: the cells that would reach a network (`daily-digest/fetcher`,
//!   `/notifier`; `research-assistant/planner`, `/proxy`, `/reader`, `/searcher`) are inert
//!   code cells with the shipped hop contract, and the digest clock keeps a literal schedule
//!   id and a cron that cannot fire during a run (29 February, midnight);
//! - the edges `install_app` renders for the presenter's own `reads_residents`
//!   (`templates/presenter/template.json`, generation `sam`, `ctx.member_person` `alex`),
//!   exactly as rendered, for those two residents.
//!
//! A kept result is handed in the way the hive's own cells deliver it: one test lane per
//! copied hive (`test_keep`, an edge from the hive path onto `./shelf` resp. `./archive`,
//! added to the COPY only) carries a formatted digest with `context.digest_round`, or the
//! planner's final text with `context.question` / `turn_id` / `audience_set`. Nothing of the
//! shipped templates is changed by that: a sealed hive takes nothing at an inner address
//! from outside, so the door has to be the hive path.
//!
//! # What is measured, at the receiver
//!
//! 1. A digest kept only for ANOTHER round is not drawn: the stage's answer from the digest
//!    carries no row, the window is withdrawn at the screen (the sentinel), and neither the
//!    card nor the digest's text reached the screen. Then a digest kept for the member's
//!    round is drawn as the `display-card` of topic `digest`, and still nothing of the
//!    other round's digest stands.
//! 2. A research answer kept for the member's round stands in the `research` list with its
//!    question; one kept for another round is neither in the stage's answer nor at the
//!    screen.
//!
//! Absence is proven by a later event of the same order, never by waiting. Guarded like
//! every template-reading test (GH #49).

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use display_colony::{SCREEN, body_of, copy_tree, hop_of, patch_json, read_json, repo, write_json};
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::mock_http::MockResponse;
use presenter_colony::{Dials, PRESENTER, Stage, boot, decision, guard, to_path, warm};
use tokio::sync::oneshot;

/// The member's round, as the builder's edge stamps it (`agent:<generation>`,
/// `member:<ctx.member_person>`) -- already in the canonical (sorted) order a kept row
/// carries it in.
const ROUND: &str = r#"["agent:sam","member:alex"]"#;
/// Another member's round under the same generation: it does not cover the member's.
const OTHER_ROUND: &str = r#"["agent:sam","member:bob"]"#;
const GENERATION: &str = "sam";
const PERSON: &str = "alex";

const DIGEST: &str = "/alex/apps/daily-digest";
const RESEARCH: &str = "/alex/apps/research-assistant";

/// The lead (first sentence) of each kept digest, and the question of each kept answer.
const OUR_LEAD: &str = "Round one news for the member.";
const OTHER_LEAD: &str = "Secret news of another member.";
const OUR_QUESTION: &str = "What changed in the garden today?";
const OTHER_QUESTION: &str = "What did the neighbour sell?";

/// A cron that cannot fire during a test run.
const NEVER_CRON: &str = "0 0 0 29 2 *";

fn round() -> Value {
    meclaw_core::serde_json::from_str(ROUND).expect("the round parses")
}

/// The templates this file reads besides the presenter colony's own (R2b, GH #9).
fn shipped() -> bool {
    [
        "templates/daily-digest/config.json",
        "templates/daily-digest/shelf/config.json",
        "templates/research-assistant/config.json",
        "templates/research-assistant/shelf/config.json",
        "templates/research-assistant/archive/config.json",
        "templates/builder/recipes/config.json",
        "templates/presenter/template.json",
        "templates/presenter/stage/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

// ═══════════════════════════════════════════════════════════ the stand-ins

/// An inert stand-in for a cell that would reach a network: it answers nothing and keeps
/// the shipped hop contract, so every edge out of it reads the keys it read before.
fn stand_in(config: &std::path::Path) {
    let shipped = read_json(config);
    let hop = shipped["contract"]["emits"]["hop"].clone();
    let hop = if hop.is_object() { hop } else { json!({}) };
    write_json(
        config,
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": "import sys\nsys.stdout.write('[]')\n",
                       "external_timeout_ms": 10000},
            "contract": {
                "version": "1.0.0",
                "settings": {},
                "multi_send_capable": true,
                "emits": {"body": {"messages": {"type": "array", "required": false}}, "hop": hop},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "capabilities": ["shell:exec"]
            },
            "description": {"purpose": "Test stand-in for a cell that would reach a network.",
                            "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
        }),
    );
}

/// The test door of a copied hive: `test_keep` at the hive path, onto `inner`, declared
/// in the copy's own contract so the lane is one the hive says it takes.
fn test_door(hive: &std::path::Path, inner: &str) {
    patch_json(&hive.join("config.json"), |v| {
        v["params"]["graph"]["edges"]
            .as_array_mut()
            .expect("edges")
            .push(json!({"from": ".", "to": inner,
                         "condition": "has(hop.route) && hop.route == 'test_keep'"}));
        v["params"]["contract"]["accepts"]
            .as_array_mut()
            .expect("accepts")
            .push(json!({"route": "test_keep",
                         "because": "test fixture only: a kept result handed in the way the hive's own cell delivers it"}));
    });
}

/// The scheduled door of the copied digest: `test_tick` at the hive path, onto `./shelf`,
/// writing `context.digest_origin` 'schedule' the way the clock's edge `./clock -> ./fetcher`
/// does (overwriting whatever the sender put there). Added to the COPY only.
fn tick_door(hive: &std::path::Path) {
    patch_json(&hive.join("config.json"), |v| {
        v["params"]["graph"]["edges"]
            .as_array_mut()
            .expect("edges")
            .push(json!({"from": ".", "to": "./shelf",
                         "condition": "has(hop.route) && hop.route == 'test_tick'",
                         "modifier": {"set_context": {"digest_origin": "'schedule'"}}}));
        v["params"]["contract"]["accepts"]
            .as_array_mut()
            .expect("accepts")
            .push(json!({"route": "test_tick",
                         "because": "test fixture only: a scheduled digest handed in the way the clock's run delivers it"}));
    });
}

// ═══════════════════════════════════════════════════════════════ the road

/// `install_app` as the builder renders it over the presenter's own `reads_residents`.
fn rendered_diff() -> Value {
    let block = read_json(&repo("templates/presenter/template.json"))["app"].clone();
    let declaration = json!({"reads_residents": block["reads_residents"]});
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/alex", "app": "presenter",
                                                    "template": "presenter@1.2.7",
                                                    "screen": "display-main",
                                                    "generation": GENERATION,
                                                    "declaration": declaration,
                                                    // The two member apps are drawn only
                                                    // when named (Y fix round 1, I-1).
                                                    "residents_present": [
                                                        "daily-digest",
                                                        "research-assistant"],
                                                    "ctx": {"member_person": PERSON}}})
                                      .to_string()}],
        }),
    );
    let first = out.first().expect("the recipe emitted nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the presenter's resident words: {first}"
    );
    first["manifest"][0]["diff"].clone()
}

/// The rendered edges between the presenter and the two apps booted here, as rendered.
fn resident_road() -> Vec<Value> {
    let booted = ["./apps/daily-digest", "./apps/research-assistant"];
    let edges = rendered_diff()["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let kept: Vec<Value> = edges
        .into_iter()
        .filter(|e| {
            let (from, to) = (e["from"].as_str(), e["to"].as_str());
            (from == Some("./apps/presenter") && booted.contains(&to.unwrap_or("")))
                || (to == Some("./apps/presenter") && booted.contains(&from.unwrap_or("")))
        })
        .collect();
    // Out and back for each of the two.
    assert_eq!(
        kept.len(),
        4,
        "the rendered road of the booted residents: {kept:#?}"
    );
    for e in kept.iter().filter(|e| e["to"] == "./apps/presenter") {
        assert_eq!(
            e["modifier"]["set_hop"]["resident_round"],
            json!(format!("'{ROUND}'")),
            "an app's answer comes back in the member's round: {e}"
        );
    }
    kept
}

/// The templates the mutation grows from: the two shipped apps, neutralised, and `web`
/// (the table is rescanned whole).
fn templates_dir() -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("a temporary directory");
    let root = td.path();
    copy_tree(&repo("templates/web"), &root.join("web"));
    // GH #1061 (#801): both apps carry their own broker, a ref to `access@…`,
    // so the library holds it; nothing here asks it (the cells that would
    // spend a key are stand-ins), and a broker nobody asks never wakes.
    copy_tree(&repo("templates/access"), &root.join("access"));

    let digest = root.join("daily-digest");
    copy_tree(&repo("templates/daily-digest"), &digest);
    patch_json(&digest.join("clock/config.json"), |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000009d7");
        v["params"]["schedules"][0]["cron"] = json!(NEVER_CRON);
    });
    for cell in ["fetcher", "notifier"] {
        stand_in(&digest.join(cell).join("config.json"));
    }
    test_door(&digest, "./shelf");
    tick_door(&digest);

    let research = root.join("research-assistant");
    copy_tree(&repo("templates/research-assistant"), &research);
    for cell in ["planner", "proxy", "reader", "searcher"] {
        stand_in(&research.join(cell).join("config.json"));
    }
    test_door(&research, "./archive");
    td
}

/// The two apps and their road, grown into the running member by one mutation; the
/// returned directory holds the templates and lives as long as the test.
async fn install_residents(s: &Stage) -> tempfile::TempDir {
    let td = templates_dir();
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().to_path_buf(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");
    let diff = json!({
        "add_nodes": [
            {"name": "apps/daily-digest", "template": "daily-digest"},
            {"name": "apps/research-assistant", "template": "research-assistant"}
        ],
        "add_edges": resident_road(),
    });
    let (ack_tx, ack_rx) = oneshot::channel();
    s.c.h
        .inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/alex", "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    let outcome = ack_rx.await.expect("the mutation answers");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "growing the two apps and their rendered road is one ordinary mutation: {outcome:?}"
    );
    td
}

// ═══════════════════════════════════════════════════════════ keep and observe

/// How many writes have reached the store `store` inside `hive` so far -- one per kept
/// result before any read is made. The store takes its mailbox in order, so a read
/// delivered after them is answered after they are kept.
async fn writes(s: &Stage, hive: &str, store: &str) -> usize {
    s.c.log(Some(&format!("{hive}/{store}"))).await.len()
}

/// Wait until `n` writes have reached the store inside `hive`.
async fn wait_kept(s: &Stage, hive: &str, store: &str, n: usize) {
    s.c.wait_until("the result reaches its store", || async {
        writes(s, hive, store).await >= n
    })
    .await;
}

/// A finished digest, kept with `round` the way `./format` hands it to `./shelf`.
async fn keep_digest(s: &Stage, round: &str, lead: &str) {
    s.c.h
        .send(to_path(
            DIGEST,
            json!({"route": "test_keep"}),
            json!({"digest_round": round}),
            json!({"messages": [{"origin": "assistant", "type": "text",
                                 "text": format!("Daily digest:\n{lead} More of it.")}]}),
        ))
        .await;
}

/// A finished digest of a SCHEDULED run, handed in over the copy's `test_tick` lane (whose
/// edge writes `digest_origin` 'schedule'). The sender also forges another round as
/// `digest_round`: a scheduled run ignores it and is kept for its holder (OR-NL-158).
async fn tick_digest(s: &Stage, lead: &str) {
    s.c.h
        .send(to_path(
            DIGEST,
            json!({"route": "test_tick"}),
            json!({"digest_round": OTHER_ROUND, "digest_origin": "parent"}),
            json!({"messages": [{"origin": "assistant", "type": "text",
                                 "text": format!("Daily digest:\n{lead} More of it.")}]}),
        ))
        .await;
}

/// The planner's final text, kept with the turn's question, id and round the way the
/// planner hands it to `./archive`.
async fn keep_answer(s: &Stage, round: &str, turn_id: &str, question: &str, answer: &str) {
    s.c.h
        .send(to_path(
            RESEARCH,
            json!({"route": "test_keep"}),
            json!({"question": question, "turn_id": turn_id, "audience_set": round}),
            json!({"messages": [{"origin": "assistant", "type": "text", "text": answer}]}),
        ))
        .await;
}

/// A sure verdict for `topic` with `lead`, every other asked key answered with its quiet
/// default (the translate wants every key: `decision_incomplete` otherwise).
fn verdict(topic: &str, lead: &str) -> MockResponse {
    let cfg = read_json(&repo("templates/presenter/stage/config.json"));
    let mut owned: Vec<(String, String, f64)> =
        vec![("topic".to_string(), topic.to_string(), 0.95)];
    for t in cfg["params"]["builtin_topics"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = t["topic"].as_str().unwrap_or_default();
        let pick = if name == topic {
            lead
        } else {
            t["standard"].as_str().unwrap_or_default()
        };
        owned.push((format!("{name}.lead"), pick.to_string(), 0.95));
        owned.push((format!("{name}.also"), "none".to_string(), 0.95));
    }
    let refs: Vec<(&str, &str, f64)> = owned
        .iter()
        .map(|(k, c, p)| (k.as_str(), c.as_str(), *p))
        .collect();
    decision(&refs)
}

/// The round a logged `resident_round` names, read whether it travels as a list or as
/// its JSON text.
fn round_of(v: &Value) -> Value {
    match v.as_str() {
        Some(text) => meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null),
        None => v.clone(),
    }
}

/// The `resident_answer` from `resident` for the read `op_id` as it reached `stage` --
/// the receiver.
async fn answer_at_stage(s: &Stage, resident: &str, op_id: &str) -> (Value, Value) {
    let stage = format!("{PRESENTER}/stage");
    let is_it = |h: &Value| {
        h["route"] == "resident_answer" && h["resident"] == resident && h["op_id"] == op_id
    };
    s.c.wait_until("the app's answer reaches stage", || async {
        s.c.log(Some(&stage))
            .await
            .iter()
            .any(|r| is_it(&hop_of(r)))
    })
    .await;
    let rows = s.c.log(Some(&stage)).await;
    let row = rows
        .iter()
        .find(|r| is_it(&hop_of(r)))
        .expect("the answer stands in the log");
    (hop_of(row), body_of(row))
}

/// The props of the one object at `web` whose id carries `needle` and that is drawn as
/// `component`.
async fn drawn_props(s: &Stage, needle: &str, component: &str) -> Value {
    let tree = s.c.tree().await;
    tree.as_object()
        .and_then(|m| {
            m.iter()
                .find(|(k, v)| k.contains(needle) && v["component"] == component)
                .map(|(_, v)| v["props"].clone())
        })
        .unwrap_or(Value::Null)
}

/// Every view the screen RECEIVED (its `in_view` lane), as text, in arrival order.
async fn views_at_screen(s: &Stage) -> Vec<String> {
    s.c.log(Some(SCREEN))
        .await
        .iter()
        .filter(|r| hop_of(r)["route"] == "in_view")
        .map(|r| body_of(r).to_string())
        .collect()
}

/// The `key` of every entry of an answer's list, in the answer's order.
fn leads(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("a list expected: {list}"))
        .iter()
        .map(|d| d[key].as_str().unwrap_or_default().to_string())
        .collect()
}

// ═══════════════════════════════════════════════════════════ the measurement

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_digest_is_shown_only_to_its_round() {
    if !guard("a_digest_is_shown_only_to_its_round") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        data_wait_ms: 5_000,
        // The shipped built-in topics: this lock answers every question they add.
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_residents(&s).await;
    warm(&s).await;

    // Only another round's digest is kept.
    keep_digest(&s, OTHER_ROUND, OTHER_LEAD).await;
    wait_kept(&s, DIGEST, "store", 1).await;

    s.turn_in("t1", "what did the digest say", round()).await;
    s.ask().await.release(verdict("digest", "last"));
    let (hop, body) = answer_at_stage(&s, "daily-digest", "t1/last").await;
    assert_eq!(
        round_of(&hop["resident_round"]),
        round(),
        "the digest answers in the member's round: {hop}"
    );
    assert_eq!(
        body["ok"], true,
        "the read carried the member's round: {body}"
    );
    assert_eq!(
        body["digests"],
        json!([]),
        "a digest kept for another round is no row of the member's read: {body}"
    );

    // The sentinel: the window's withdrawal reaches the screen. The screen takes the
    // presenter's lane in order, so a block written before it would have arrived before.
    s.c.wait_until("the digest window is withdrawn at the screen", || async {
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .any(|r| hop_of(r)["route"] == "in_withdraw" && body_of(r)["view_id"] == "show-digest")
    })
    .await;
    let at_screen = views_at_screen(&s).await;
    for never in ["show-digest-last", OTHER_LEAD] {
        assert!(
            !at_screen.iter().any(|b| b.contains(never)),
            "another round's digest reached the member's screen: {never}"
        );
        assert!(!s.drawn(never).await, "{never} stands at web");
    }
    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("no_data"), "{row}");

    // The member's own digest is kept: now the card stands, and only with it.
    keep_digest(&s, ROUND, OUR_LEAD).await;
    wait_kept(&s, DIGEST, "store", 2).await;

    s.turn_in("t2", "what did the digest say", round()).await;
    s.ask().await.release(verdict("digest", "last"));
    s.wait_drawn("show-digest-last").await;

    let (_, body) = answer_at_stage(&s, "daily-digest", "t2/last").await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        leads(&body["digests"], "lead"),
        vec![OUR_LEAD.to_string()],
        "the member's read holds the member's digest and no other: {body}"
    );
    assert_eq!(
        body["digests"][0]["audience_set"],
        round(),
        "each row carries its own round: {body}"
    );
    let card = drawn_props(&s, "show-digest-last", "display-card").await;
    assert!(
        card.to_string().contains(OUR_LEAD),
        "the card carries the member's digest: {card}"
    );
    assert!(
        !s.c.tree().await.to_string().contains(OTHER_LEAD),
        "another round's digest stands at web"
    );
    let row = s.journal_of("t2").await;
    assert_eq!(row["topic"], json!("digest"), "{row}");
    assert_eq!(row["fallback"], json!("none"), "{row}");
    s.c.shutdown().await;
}

/// One boot of the member colony with `screen` as the presenter's `screen_audience`, the
/// apps grown in, and one scheduled digest kept for its holder.
///
/// `star_only` is the stage's `window_requires_star_data` (`null` = shipped, on).
async fn boot_with_a_scheduled_digest(
    screen: Value,
    star_only: Value,
) -> (Stage, tempfile::TempDir) {
    let s = boot(Dials {
        screen_audience: screen,
        data_wait_ms: 5_000,
        builtin_topics: Value::Null,
        window_requires_star_data: star_only,
        ..Dials::default()
    })
    .await;
    let templates = install_residents(&s).await;
    warm(&s).await;
    tick_digest(&s, OUR_LEAD).await;
    wait_kept(&s, DIGEST, "store", 1).await;
    (s, templates)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_digest_shows_on_the_member_screen_and_no_other() {
    if !guard("a_scheduled_digest_shows_on_the_member_screen_and_no_other") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }

    // 1. The member's screen: the scheduled digest is the member's, read under the round
    //    the builder's read edge stamps, and drawn as the `digest` card.
    let (mut s, _templates) =
        boot_with_a_scheduled_digest(json!(["member:alex"]), Value::Null).await;
    s.turn_in("t1", "what did the digest say", round()).await;
    s.ask().await.release(verdict("digest", "last"));
    s.wait_drawn("show-digest-last").await;
    let (hop, body) = answer_at_stage(&s, "daily-digest", "t1/last").await;
    assert_eq!(round_of(&hop["resident_round"]), round(), "{hop}");
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        leads(&body["digests"], "lead"),
        vec![OUR_LEAD.to_string()],
        "the holder's scheduled digest is the member's read: {body}"
    );
    assert_eq!(
        body["digests"][0]["audience_set"],
        round(),
        "the holder's digest answers with the member's round, never the forged one: {body}"
    );
    let card = drawn_props(&s, "show-digest-last", "display-card").await;
    assert!(
        card.to_string().contains(OUR_LEAD),
        "the card carries the scheduled digest: {card}"
    );
    let row = s.journal_of("t1").await;
    assert_eq!(row["topic"], json!("digest"), "{row}");
    assert_eq!(row["fallback"], json!("none"), "{row}");
    s.c.shutdown().await;

    // 2. A screen of a wider round (it also shows another member): the same scheduled
    //    digest is answered under the member's round, which does not cover that screen --
    //    and (R-HP-18, the shipped switch off) the member's window shows it all the same.
    //    A screen with no member of the turn in it opens nothing (GH #1027, gh1027).
    another_round(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_switch_on_no_digest_window_opens() {
    if !guard("with_the_switch_on_no_digest_window_opens") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    // R-HP-9 (c), `window_requires_star_data` on: the way back.
    another_round(true).await;
}

/// Case 2 of the digest lock under one setting of `window_requires_star_data`.
async fn another_round(star_only: bool) {
    let dial = if star_only { json!(true) } else { Value::Null };
    let (mut s, _templates) =
        boot_with_a_scheduled_digest(json!(["member:alex", "member:bob"]), dial).await;
    s.turn_in("t1", "what did the digest say", round()).await;
    s.ask().await.release(verdict("digest", "last"));
    let (_, body) = answer_at_stage(&s, "daily-digest", "t1/last").await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        body["digests"][0]["audience_set"],
        round(),
        "the app answers the member's round: {body}"
    );
    if !star_only {
        // R-HP-18, at the receiver: the member's digest stands on the shared screen.
        s.wait_drawn("show-digest-last").await;
        let at_screen = views_at_screen(&s).await;
        assert!(
            at_screen.iter().any(|b| b.contains(OUR_LEAD)),
            "the member's digest reaches a screen it shares with another member: {at_screen:?}"
        );
        assert_eq!(s.journal_of("t1").await["fallback"], json!("none"));
        s.c.shutdown().await;
        return;
    }
    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("no_data"), "{row}");
    {
        // Nothing left stage for the window: no view and no withdrawal of `show-digest`
        // in the whole log, so nothing can be on its way to the screen either.
        let stage = format!("{PRESENTER}/stage");
        let sent: Vec<String> =
            s.c.log(None)
                .await
                .iter()
                .filter(|r| r.from_path == stage && body_of(r)["view_id"] == "show-digest")
                .map(|r| hop_of(r)["route"].to_string())
                .collect();
        assert!(
            sent.is_empty(),
            "stage sent for the digest window: {sent:?}"
        );
    }
    let at_screen = views_at_screen(&s).await;
    assert!(
        !at_screen.iter().any(|b| b.contains("show-digest")),
        "no digest window on a shared screen with the switch on: {at_screen:?}"
    );
    for never in ["show-digest-last", OUR_LEAD] {
        assert!(
            !at_screen.iter().any(|b| b.contains(never)),
            "the member's scheduled digest reached a screen of another round: {never}"
        );
        assert!(!s.drawn(never).await, "{never} stands at web");
    }
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_research_answer_is_shown_only_to_its_round() {
    if !guard("a_research_answer_is_shown_only_to_its_round") {
        return;
    }
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        // The shipped built-in topics: this lock answers every question they add.
        builtin_topics: Value::Null,
        ..Dials::default()
    })
    .await;
    let _templates = install_residents(&s).await;
    warm(&s).await;

    keep_answer(
        &s,
        OTHER_ROUND,
        "turn-other",
        OTHER_QUESTION,
        "The neighbour sold a bicycle.",
    )
    .await;
    wait_kept(&s, RESEARCH, "memory", 1).await;
    keep_answer(
        &s,
        ROUND,
        "turn-ours",
        OUR_QUESTION,
        "The roses opened today.",
    )
    .await;
    wait_kept(&s, RESEARCH, "memory", 2).await;

    s.turn_in("t1", "what did I ask the research assistant", round())
        .await;
    s.ask().await.release(verdict("research", "answers"));
    s.wait_drawn("show-research-answers").await;

    let (hop, body) = answer_at_stage(&s, "research-assistant", "t1/answers").await;
    assert_eq!(
        round_of(&hop["resident_round"]),
        round(),
        "the research assistant answers in the member's round: {hop}"
    );
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        leads(&body["answers"], "question_id"),
        vec!["turn-ours".to_string()],
        "the member's read holds the member's answer and no other: {body}"
    );
    assert_eq!(body["answers"][0]["question"], OUR_QUESTION, "{body}");
    assert_eq!(body["answers"][0]["audience_set"], round(), "{body}");

    // The list is one block written out of that one answer: once its question stands,
    // the list is complete.
    s.c.wait_until("the member's question stands in the list", || async {
        s.c.tree().await.to_string().contains(OUR_QUESTION)
    })
    .await;
    assert!(
        s.c.tree()
            .await
            .to_string()
            .contains("The roses opened today."),
        "the answer stands beside its question"
    );
    assert!(
        !s.c.tree().await.to_string().contains(OTHER_QUESTION),
        "another round's question stands at web"
    );
    assert!(
        !views_at_screen(&s)
            .await
            .iter()
            .any(|b| b.contains(OTHER_QUESTION) || b.contains("bicycle")),
        "another round's answer reached the member's screen"
    );
    let row = s.journal_of("t1").await;
    assert_eq!(row["topic"], json!("research"), "{row}");
    assert_eq!(row["fallback"], json!("none"), "{row}");
    s.c.shutdown().await;
}
