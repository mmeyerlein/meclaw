//! GH #521 — a two-phase exchange with a store looped, because the return lane
//! could not tell the legs apart.
//!
//! Measured on the verification run of #513. With both wire contracts published
//! (`scriptlet` `SCRIPT`, `shelf` `ARGS`) the composer drew the two-phase dedupe
//! a store that ships no constraints forces — ask the shelf what it already
//! holds, then insert what is new — and guarded the return edge like this:
//!
//! ```text
//! {"from": "./feed_dedupe", "to": "./feed_shelf",
//!  "condition": "hop.route == 'select'",
//!  "modifier": {"set_context": {"phase": "'select'", …}}}
//! {"from": "./feed_shelf",  "to": "./feed_dedupe",
//!  "condition": "has(context.phase) && context.phase == 'select'"}
//! ```
//!
//! Every declaration legal, every emission valid, the shelf right every time —
//! and 1 200 rows for 40 distinct links, 30 rounds a tick, one `ttl_expired` to
//! close it. `context` is PERSISTENT: `phase` was still `'select'` on the answer
//! to the **insert**, so the answer to phase two came back in on phase one's
//! lane and the script re-entered its own write branch.
//!
//! The substrate stamps what was needed and always did: `hop` is single-hop and
//! is REPLACED on every emission (`Headers::carry_context_with_hop`), so the only
//! thing on a store answer that names its leg is `hop.operation` — written
//! unconditionally, the error surface included (GH #331). Nothing was missing
//! but the sentence, and the two places a composer reads: `shelf@1.0.2`'s
//! catalogue row (`RETURN`) and the builder's briefing (`ROUND TRIP`).
//!
//! This file is that sentence's drift lock in the sense of
//! `docs/development-rules.md` § 2d, and it does BOTH halves:
//!
//! 1. it GREPS the two surfaces, and checks the row still travels whole;
//! 2. it RUNS THE MECHANISM — the condition is lifted verbatim out of the
//!    published row and evaluated, through the colony's own CEL, against the
//!    headers a REAL shelf answers a REAL `select` and a REAL `insert` bundle
//!    with. The `context` form the composer drew is run through the same
//!    evaluator against the same two answers, so the sentence about the loop is
//!    a measurement and not a claim.
//!
//! **R2b guard.** Every read is guarded: in a tree that does not carry the
//! template, these tests skip rather than fail on a dead reference.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Message, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30 s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// What the composer drew, verbatim from the s513 colony's edge table. It is
/// not published anywhere and must not be: it is the counter-example.
const THE_GUARD_THAT_LOOPS: &str = "has(context.phase) && context.phase == 'select'";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped `shelf` descriptor, or `None` where this tree does not carry it.
fn shipped() -> Option<Value> {
    let raw = std::fs::read_to_string(repo("templates/shelf/template.json")).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// The `RETURN` line of `description.examples` — the surface under test.
fn return_line(tpl: &Value) -> String {
    tpl["description"]["examples"]
        .as_array()
        .expect("the descriptor carries examples")
        .iter()
        .filter_map(|e| e.as_str())
        .find(|e| e.trim_start().starts_with("RETURN"))
        .unwrap_or_else(|| {
            panic!(
                "`templates/shelf/template.json` publishes no `RETURN` line, so a \
                 composer is told which OPERATIONS a shelf runs and never how an \
                 answer says which one it is answering — which is the whole of GH #521"
            )
        })
        .to_string()
}

/// The `condition` strings the `RETURN` line publishes, in order. Lifted out of
/// the published bytes rather than restated here: a guard this test compiled
/// itself would prove nothing about the one a composer copies.
fn published_conditions(tpl: &Value) -> Vec<String> {
    let line = return_line(tpl);
    let mut out = Vec::new();
    let mut rest = line.as_str();
    while let Some(at) = rest.find("\\\"condition\\\": \\\"") {
        let tail = &rest[at + "\\\"condition\\\": \\\"".len()..];
        let Some(end) = tail.find("\\\"") else { break };
        out.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    if out.is_empty() {
        // The line is a JSON string inside a JSON file; `serde_json` already
        // un-escaped it once, so the plain spelling is the one that hits.
        let mut rest = line.as_str();
        while let Some(at) = rest.find("\"condition\": \"") {
            let tail = &rest[at + "\"condition\": \"".len()..];
            let Some(end) = tail.find('"') else { break };
            out.push(tail[..end].to_string());
            rest = &tail[end..];
        }
    }
    out
}

fn version(tpl: &Value) -> String {
    tpl["version"]
        .as_str()
        .expect("the descriptor declares a version")
        .to_string()
}

// ================================================================== HALF ONE

/// The sentence itself: what a composer is told before it draws the lane.
#[test]
fn the_row_says_which_field_names_the_leg_and_which_one_cannot() {
    let Some(tpl) = shipped() else { return };
    let line = return_line(&tpl);

    for phrase in [
        "`hop.operation`",
        "single-hop",
        "REPLACED",
        "persistent",
        "POSITIVELY",
        "\"default\": true",
        "no_route",
        "GH #521",
    ] {
        assert!(
            line.contains(phrase),
            "the RETURN line does not name {phrase:?} — the measured defect was \
             exactly the difference between the two compartments: {line}"
        );
    }

    let conds = published_conditions(&tpl);
    assert_eq!(
        conds.len(),
        1,
        "the line publishes exactly ONE guarded return edge (the read leg) \
         beside its default; more than one guard would be a topology, not a rule: {conds:?}"
    );
    assert!(
        conds[0].contains("hop.operation"),
        "the published guard must read the field the ANSWER carries: {:?}",
        conds[0]
    );
    assert!(
        !conds[0].contains("context"),
        "a published guard on `context` would be the defect itself: {:?}",
        conds[0]
    );
}

/// The briefing is the other place a composer reads, and it is the one that
/// reaches a composer who never looks the template up.
#[test]
fn the_briefing_carries_the_round_trip_rule() {
    let Ok(raw) = std::fs::read_to_string(repo("templates/builder/brief/config.json")) else {
        return; // R2b: this tree does not carry the template.
    };
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("the brief parses");
    let prompt = cfg["params"]["script_inline"]
        .as_str()
        .expect("the brief carries its prompt inline");

    assert!(
        prompt.contains("ROUND TRIP"),
        "the briefing teaches lanes and conditions and never once says that an \
         ANSWER coming back on the lane a REQUEST left on has to be told apart \
         from the other answers (GH #521)"
    );
    for phrase in [
        "SINGLE-HOP",
        "PERSISTENT",
        "POSITIVELY",
        "hop.operation",
        "context.phase",
        "no_route",
    ] {
        assert!(
            prompt.contains(phrase),
            "the ROUND TRIP rule does not name {phrase:?}"
        );
    }
}

/// The line is only published if it TRAVELS. A catalogue row over the corpus
/// generator's cap is split into `-cont` rows, and the retriever hands a
/// `template` hit over whole — so a descriptor that grew past the cap would
/// publish `RETURN` into a continuation the composer never asks for.
#[test]
fn the_return_line_is_inside_the_one_catalogue_row_the_corpus_carries() {
    let Some(tpl) = shipped() else { return };
    let Ok(raw) =
        std::fs::read_to_string(repo("templates/builder-librarian/store/seed/docs.jsonl"))
    else {
        return; // R2b: this tree does not carry the corpus.
    };
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str(l).ok())
        .filter(|r: &Value| r["kind"] == "template" && r["section"] == "shelf")
        .collect();

    assert_eq!(
        rows.len(),
        1,
        "`shelf` must be ONE catalogue row: the row is at the cap, and a `RETURN` \
         line in a continuation row is a rule published where nobody looks"
    );
    let text = rows[0]["text"].as_str().unwrap_or_default();
    let escaped = meclaw_core::serde_json::to_string(&return_line(&tpl)).expect("a string");
    assert!(
        text.contains(escaped.trim_matches('"')),
        "the corpus is stale — regenerate it with `workshop/tools/build_librarian_seed.py`"
    );
}

// ================================================================== HALF TWO

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "store".to_string(),
        Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create destination");
    for entry in std::fs::read_dir(src).expect("read source") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// A colony with a root hive, a sink, and the shipped `shelf` in its library.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main")).expect("root hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");
    copy_tree(&repo("templates/shelf"), &root.join("templates/shelf"));
    td
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("a colony with nothing but a sink boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the library must register");
    (h, rx)
}

/// One shelf, grown the way a manifest grows it — the flat `override_params`
/// the row publishes — with an UNGUARDED edge onto the sink, so that BOTH legs
/// of the exchange arrive and the guards are what decides, not the wiring.
async fn grow_shelf(h: &ColonyHandle, tpl: &Value) {
    let reference = format!("shelf@{}", version(tpl));
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {
                "add_nodes": [{"name": "headlines", "template": reference,
                               "override_params": {"schema": {"items": {
                                   "link": "text", "title": "text"}}}}],
                "add_edges": [{"from": "./headlines", "to": "./sink"}]
            }}),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the published declaration must be instantiable: {outcome:?}"
    );
}

/// Send one message of `calls` at the shelf, carrying the `context` the
/// composer's outbound edge set, and hand back the answer.
async fn ask(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    calls: Vec<Value>,
    phase: &str,
) -> Message {
    h.send(
        MessageBuilder::new("/headlines")
            .with_inline_messages(calls)
            .with_header("phase", json!(phase))
            .build(),
    )
    .await;
    tokio::time::timeout(RECV_TIMEOUT, rx.recv())
        .await
        .expect("the shelf answers every message it is sent")
        .expect("the sink channel stays open")
}

fn call(id: &str, args: Value) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id, "text": args.to_string()})
}

fn eval(source: &str, m: &Message) -> bool {
    let compiled = parse_condition(source)
        .unwrap_or_else(|e| panic!("the published guard must compile: {source} — {e}"));
    // A missing key is a SKIP, not a defect (spec F3): that is exactly what the
    // guarded edge does with the leg it does not want.
    evaluate_condition(&compiled, &m.headers.context, &m.headers.hop).unwrap_or(false)
}

fn hop_of(m: &Message) -> &Map<String, Value> {
    &m.headers.hop
}

/// **The load-bearing test.** Two legs of one exchange through a real shelf, and
/// the two guards — the published one and the one that looped — evaluated
/// against both answers by the colony's own CEL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_published_guard_takes_the_read_leg_and_the_context_guard_takes_both() {
    let Some(tpl) = shipped() else { return };
    let guard = published_conditions(&tpl)
        .into_iter()
        .next()
        .expect("the RETURN line publishes its guard");

    let td = tree();
    let (h, mut rx) = boot(&td).await;
    grow_shelf(&h, &tpl).await;

    // Phase one, exactly as the composer wrote it: ONE select, and the outbound
    // edge stamps the phase into the persistent compartment.
    let read = ask(
        &h,
        &mut rx,
        vec![call(
            "sel1",
            json!({"operation": "select", "table": "items", "columns": ["link"]}),
        )],
        "select",
    )
    .await;
    assert_eq!(
        hop_of(&read).get("operation"),
        Some(&json!("select")),
        "a read answer names its leg: {:?}",
        hop_of(&read)
    );

    // Phase two, on the SAME message lifecycle as far as `context` is concerned:
    // a compartment does not clear itself, so `phase` is still `'select'`.
    let write = ask(
        &h,
        &mut rx,
        vec![
            call(
                "ins0",
                json!({"operation": "insert", "table": "items",
                       "row": {"link": "https://example.org/a", "title": "a"}}),
            ),
            call(
                "ins1",
                json!({"operation": "insert", "table": "items",
                       "row": {"link": "https://example.org/b", "title": "b"}}),
            ),
        ],
        "select",
    )
    .await;
    assert_eq!(
        hop_of(&write).get("operation"),
        Some(&json!("bundle")),
        "a write answer names its leg too, and a multi-call message says \
         `bundle` — which is why the RULE is to name the READ leg: {:?}",
        hop_of(&write)
    );

    // A single write, so the row's parenthesis about `insert` is measured too.
    let one_write = ask(
        &h,
        &mut rx,
        vec![call(
            "ins2",
            json!({"operation": "insert", "table": "items",
                   "row": {"link": "https://example.org/c", "title": "c"}}),
        )],
        "select",
    )
    .await;
    assert_eq!(
        hop_of(&one_write).get("operation"),
        Some(&json!("insert")),
        "{:?}",
        hop_of(&one_write)
    );

    // ---- the published guard: the read leg, and only the read leg.
    assert!(
        eval(&guard, &read),
        "the published guard must take the READ answer back: {guard}"
    );
    assert!(
        !eval(&guard, &write),
        "the published guard must DECLINE the write answer, or the exchange is \
         the loop it was written to prevent: {guard}"
    );
    assert!(!eval(&guard, &one_write), "{guard}");

    // ---- the guard the composer drew: BOTH legs. This is GH #521, measured.
    assert!(
        eval(THE_GUARD_THAT_LOOPS, &read),
        "the context guard takes the read answer, which is why it looked right"
    );
    assert!(
        eval(THE_GUARD_THAT_LOOPS, &write),
        "if a guard on `context` declined the write answer there would have been \
         no loop and no issue — `context` is persistent, and this is the whole \
         mechanism the RETURN line exists to publish"
    );

    // ---- and the sentence about `context` being unchanged on the way back.
    assert_eq!(
        write.headers.context.get("phase"),
        Some(&json!("select")),
        "the store carries the caller's `context` back unchanged; it is `hop` \
         that is replaced, and that asymmetry IS the rule"
    );

    h.shutdown().await;
}

/// The other half of the published form: the answer the guard declines has
/// somewhere to go. An unguarded `default` edge takes what the ordinary
/// out-edges left, which is what keeps the write answer out of the DLQ.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_edge_the_row_publishes_takes_the_leg_the_guard_declined() {
    let Some(tpl) = shipped() else { return };
    let line = return_line(&tpl);
    assert!(
        line.contains("\"default\": true"),
        "the RETURN line must publish the catch-all in the form `add_edges` \
         takes, or the reader has a rule and no shape for it: {line}"
    );

    let td = tree();
    let (h, mut rx) = boot(&td).await;
    grow_shelf(&h, &tpl).await;

    // Re-wire the sink lane into exactly what the row publishes: the guarded
    // read edge, plus the default beside it.
    let guard = published_conditions(&tpl)
        .into_iter()
        .next()
        .expect("the RETURN line publishes its guard");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {
                "add_edges": [
                    {"from": "./headlines", "to": "./sink", "condition": guard},
                    {"from": "./headlines", "to": "./sink", "default": true}
                ],
                "remove_edges": [{"match": {"from": "./headlines", "to": "./sink",
                                            "condition": null, "default": false}}]
            }}),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the published pair of edges must be drawable by an ordinary manifest: {outcome:?}"
    );

    // The write answer no guard takes still arrives — via the default, not via
    // the dead-letter queue.
    let write = ask(
        &h,
        &mut rx,
        vec![call(
            "ins0",
            json!({"operation": "insert", "table": "items",
                   "row": {"link": "https://example.org/d", "title": "d"}}),
        )],
        "select",
    )
    .await;
    assert_eq!(hop_of(&write).get("operation"), Some(&json!("insert")));

    h.shutdown().await;
}

/// The substrate half the two sentences rest on: `hop` is REPLACED on every
/// emission and `context` is carried. Asserted on `Headers` itself, so the rule
/// is pinned to the mechanism and not only to a store's behaviour.
#[test]
fn hop_is_replaced_on_every_emission_and_context_is_carried() {
    let mut context = Map::new();
    context.insert("phase".into(), json!("select"));
    let mut outbound_hop = Map::new();
    outbound_hop.insert("route".into(), json!("select"));
    let outbound = meclaw_core::Headers::from_parts(context, outbound_hop);

    let mut answer_hop = Map::new();
    answer_hop.insert("operation".into(), json!("bundle"));
    let answer = outbound.carry_context_with_hop(answer_hop);

    assert_eq!(
        answer.hop.get("route"),
        None,
        "nothing the caller stamped on the way out may survive the answer — if \
         it did, `hop.route` would be a legal guard and GH #521 would not exist"
    );
    assert_eq!(answer.hop.get("operation"), Some(&json!("bundle")));
    assert_eq!(
        answer.context.get("phase"),
        Some(&json!("select")),
        "and `context` is the mirror image: unchanged, on BOTH legs"
    );
}

/// The neighbour that already got this right, and the reason the rule is a rule
/// rather than a preference: the shipped librarian recognises its own second
/// phase POSITIVELY, on `hop.operation`, after the naive shape looped there too.
#[test]
fn the_shipped_librarian_recognises_its_second_phase_positively() {
    let Ok(raw) = std::fs::read_to_string(repo("templates/builder-librarian/retrieve/config.json"))
    else {
        return; // R2b.
    };
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("the retriever parses");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("the retriever carries its script inline");
    assert!(
        script.contains("hop.get(\"operation\") in (\"search\", \"bundle\")"),
        "the shipped retriever must keep recognising its answer leg by the \
         operation that produced it; if this branch is ever rewritten as \
         `anything that is not a fresh request`, GH #308 and GH #521 are the \
         same bug in two templates"
    );
}
