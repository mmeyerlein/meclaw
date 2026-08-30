//! GH #519 — an episode carries an embedding, so the semantic leg can reach it.
//!
//! Measured on a two-week-old hive: 182 episodes, 34 facts, 34 embeddings —
//! and every one of the 34 belonged to a fact. Not an import defect (the source
//! store held the same distribution row for row) but the design: `./writer`
//! wrote an episode and an extraction queue item and nothing else, only
//! `./extract-glue` ever minted an `embeddings` row, and it wrote
//! `"owner_table": "facts"` as a literal. The nightly backfill filtered for
//! `facts` and the tier-1 semantic leg asked `similar` for `facts`.
//!
//! The consequence is not that episodes were unreachable — the keyword leg
//! searches `episodes_fts` and the graph leg returns episodes through the edge
//! provenance. The consequence is WHICH episodes: only those sharing a word
//! with the question. An episode is raw conversational text, exactly the
//! material a lexical index is worst at and a vector is best at, so the hive
//! embedded the half that needed it least.
//!
//! Four things are pinned here, one per place the fact-only assumption lived:
//!
//! 1. `./writer` mints the queued row in the SAME store bundle as the episode,
//!    and sends the turn's text to the embedding lane beside it.
//! 2. the hive wires `./writer -> ./embed`, or the message above lands nowhere.
//! 3. `./recall` asks `similar` for both owner kinds and reads the candidate's
//!    kind OFF the row, rather than assuming it.
//! 4. an episode hit is gated against `episodes`, not against `facts` — the
//!    semantic leg is the one leg that cannot gate itself, and a hit whose
//!    companion row is fetched from the wrong table reads as the empty audience
//!    set and disappears.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No mock, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const WRITER_CONFIG: &str = "../../templates/memory-hive/writer/config.json";
const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";
const RID: &str = "r-519";

/// `${VAR:-default}` becomes the default — the same substitution the colony
/// performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Hand the shipped script to python3 **on stdin**, never in argv (GH #279).
fn run(script: &str, doc: Value) -> Vec<Value> {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "cell exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn route(m: &Value) -> String {
    m["header"]["route"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The args of one `tool_call` turn of an emission, parsed.
fn call(m: &Value, i: usize) -> Value {
    let text = m["messages"][i]["text"].as_str().expect("tool_call text");
    meclaw_core::serde_json::from_str(text).expect("store args are json")
}

// ═══════════════════════════════════════════════════════════════ 1. the writer

/// One turn on the hive's `in_episode` lane, with the provenance the port edge
/// promotes onto the context.
fn turn_doc(text: &str) -> Value {
    json!({
        "header": {"context": {"session_id": "s1", "turn_id": "s1#0",
                               "audience_set": "[\"member:alex\"]",
                               "channel": "tg:private", "speaker": "member:alex"}},
        "messages": [{"origin": "user", "type": "text", "text": text}]
    })
}

/// The episode and its embedding row are ONE store bundle, and the embedding is
/// the durable half of the pair: `blob` null, `status` queued. That pair is
/// exactly what the nightly backfill selects, which is what makes a lost embed
/// call cost a call and not a row.
#[test]
fn the_writer_mints_a_queued_embedding_row_beside_the_episode() {
    let out = run(&script_of(WRITER_CONFIG), turn_doc("Ich trinke Tee."));
    let store = out
        .iter()
        .find(|m| route(m) == "wstore")
        .expect("the store bundle");
    assert_eq!(
        store["messages"].as_array().map(Vec::len),
        Some(2),
        "the episode and its embedding row travel in ONE bundle: {store}"
    );
    let episode = call(store, 0);
    let embedding = call(store, 1);
    assert_eq!(episode["table"], "episodes");
    assert_eq!(embedding["operation"], "insert");
    assert_eq!(embedding["table"], "embeddings");
    assert_eq!(embedding["row"]["owner_table"], "episodes");
    assert_eq!(
        embedding["row"]["owner_id"], episode["row"]["id"],
        "the row belongs to the episode written beside it"
    );
    assert_eq!(embedding["row"]["status"], "queued");
    assert!(
        embedding["row"]["blob"].is_null(),
        "a queued row carries no vector yet: {embedding}"
    );
    assert_eq!(
        embedding["row"]["created_at"], episode["row"]["recorded_at"],
        "one clock reading for both rows of one turn"
    );
}

/// The text goes to the embedding lane from HERE, carried rather than looked
/// up: this cell wrote the content, so sending the lane back to the store for
/// it would be a round trip over a string already in hand.
#[test]
fn the_writer_sends_the_turn_text_to_the_embedding_lane() {
    let out = run(&script_of(WRITER_CONFIG), turn_doc("Ich trinke Tee."));
    let store = out.iter().find(|m| route(m) == "wstore").expect("wstore");
    let embed = out
        .iter()
        .find(|m| route(m) == "embed")
        .expect("the embed lane");
    let items = call(embed, 0);
    assert_eq!(items["items"][0]["text"], "Ich trinke Tee.");
    assert_eq!(
        items["items"][0]["embedding_id"],
        call(store, 1)["row"]["id"],
        "the call names the row it is filling"
    );
    assert_eq!(
        embed["header"]["episode_id"], store["header"]["episode_id"],
        "both halves carry the same episode id"
    );
}

/// An empty turn still mints the row — the queue is the durable half and a
/// night may decide otherwise — but nothing is sent to a paid embedder over an
/// empty string.
#[test]
fn an_empty_turn_mints_the_row_and_pays_for_nothing() {
    let out = run(&script_of(WRITER_CONFIG), turn_doc("   "));
    assert!(
        out.iter().any(|m| route(m) == "wstore"),
        "the episode is still written: {out:?}"
    );
    let store = out.iter().find(|m| route(m) == "wstore").expect("wstore");
    assert_eq!(call(store, 1)["row"]["status"], "queued");
    assert!(
        !out.iter().any(|m| route(m) == "embed"),
        "no embedder call over an empty string: {out:?}"
    );
}

// ═════════════════════════════════════════════════════════════ 2. the wiring

/// The message above needs an edge, and it needs it inside the hive: `embed` is
/// an interior lane. A route nothing carries is a route that silently drops
/// every turn's vector.
#[test]
fn the_hive_wires_the_writer_to_the_embedding_lane() {
    let raw = std::fs::read_to_string(HIVE_CONFIG).expect("hive config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    let edges = v["params"]["graph"]["edges"].as_array().expect("edges");
    let wired = edges.iter().any(|e| {
        e["from"] == "./writer"
            && e["to"] == "./embed"
            && e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("hop.route == 'embed'")
    });
    assert!(wired, "no ./writer -> ./embed edge in the shipped hive");
    // …and the writer's own contract has to declare the route, or the substrate
    // refuses the emission before any edge is consulted.
    let w: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(WRITER_CONFIG).unwrap())
            .unwrap();
    let routes = w["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("route values");
    assert!(
        routes.iter().any(|r| r == "embed"),
        "the writer declares no `embed` route: {routes:?}"
    );
}

// ═══════════════════════════════════════════════════════════ 3. + 4. the recall

fn ctx(phase: &str) -> Value {
    json!({"mem_phase": phase, "recall_id": RID, "memory_tier": "1",
           "recall_query": "was trinke ich abends",
           "audience_now": "[\"member:alex\"]", "channel": "tg:private"})
}

/// A store BUNDLE reply (#295): N `tool_result` turns plus the `results[]` slot.
fn bundle_reply(phase: &str, legs: &[(&str, Value)]) -> Value {
    json!({
        "header": {"context": ctx(phase),
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

fn scratch(leg: &str, payload: &str) -> Value {
    json!({"request_id": RID, "leg": leg, "payload": payload, "fired": 0})
}

/// The rendezvous the semantic call is issued from: both halves are parked, so
/// this hop is the one that asks `similar`.
fn rendezvous() -> Value {
    let legs = json!({"model": {"model_id": "m-1", "dim": 1024}, "anchors": []});
    let qvec = json!({"vector": [1, 2, 3], "degraded": false});
    bundle_reply(
        "t1-qvec-park",
        &[(
            "r-t1-qvec-park-read",
            json!([
                scratch("legs", &legs.to_string()),
                scratch("qvec", &qvec.to_string()),
            ]),
        )],
    )
}

/// The leg asks for BOTH owner kinds, and it names them: an `owner_table` this
/// lane cannot hydrate must not enter the leg at all, because the audience of a
/// semantic hit is fetched from the owning row and a row nobody selects reads
/// as the empty set and vanishes without a word.
#[test]
fn the_semantic_leg_asks_for_both_owner_kinds() {
    let out = run(&script_of(RECALL_CONFIG), rendezvous());
    let msg = out.first().expect("the join bundle");
    let similar = msg["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| meclaw_core::serde_json::from_str::<Value>(t["text"].as_str().unwrap()).unwrap())
        .find(|a| a["operation"] == "similar")
        .expect("the similar call");
    assert_eq!(
        similar["where"]["owner_table"]["in"],
        json!(["facts", "episodes"]),
        "the filter names the kinds the hydration below covers: {similar}"
    );
    assert_eq!(similar["where"]["status"], "ready");
    assert_eq!(similar["where"]["model_id"], "m-1");
}

/// The join reply: two neighbours, one of each kind. The candidate kind comes
/// off the row, and the episode's audience companion is fetched from
/// `episodes` — the table that holds it — while the fact's still comes from
/// `facts`.
#[test]
fn an_episode_neighbour_becomes_an_episode_candidate_gated_from_episodes() {
    let join = bundle_reply(
        "t1-join",
        &[
            ("r-join-anchor", json!([])),
            (
                "r-join-sem",
                json!([
                    {"owner_id": "f1", "model_id": "m-1", "owner_table": "facts",
                     "distance": 10},
                    {"owner_id": "ep1", "model_id": "m-1", "owner_table": "episodes",
                     "distance": 20},
                    {"owner_id": "x1", "model_id": "m-1", "owner_table": "skills",
                     "distance": 30}
                ]),
            ),
        ],
    );
    let out = run(&script_of(RECALL_CONFIG), join);
    let msg = out.first().expect("the legs bundle");
    let calls: Vec<Value> = msg["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| meclaw_core::serde_json::from_str::<Value>(t["text"].as_str().unwrap()).unwrap())
        .collect();

    // the parked leg carries one candidate per KIND, read off the row
    let parked = calls
        .iter()
        .find(|a| a["table"] == "recall_scratch" && a["row"]["leg"] == "sem")
        .expect("the parked semantic leg");
    let payload: Value =
        meclaw_core::serde_json::from_str(parked["row"]["payload"].as_str().unwrap()).unwrap();
    // An uncapped leg parks as the bare hit list; a capped one wraps it. Read
    // both shapes, so this test measures the KINDS and not the wrapper.
    let hits = payload.get("hits").cloned().unwrap_or(payload.clone());
    assert_eq!(
        hits,
        json!([{"kind": "fact", "id": "f1", "distance": 10},
               {"kind": "episode", "id": "ep1", "distance": 20}]),
        "the kind comes off the row, and an owner kind this lane cannot hydrate \
         is dropped where it is still visible: {payload}"
    );

    // …and each kind is gated against the table that holds its audience
    let fact_aud = calls
        .iter()
        .find(|a| a["table"] == "facts" && a["operation"] == "select")
        .expect("the fact companion page");
    assert_eq!(fact_aud["where"]["id"]["in"], json!(["f1"]));
    let ep_aud = calls
        .iter()
        .find(|a| a["table"] == "episodes" && a["operation"] == "select")
        .expect("the episode companion page");
    assert_eq!(ep_aud["where"]["id"]["in"], json!(["ep1"]));
    assert_eq!(
        ep_aud["columns"],
        json!(["id", "channel", "audience_set"]),
        "an episode carries its own gate columns and no axis: {ep_aud}"
    );
}

/// The drift lock for the sentence on the public template surface
/// (development-rules § 2d): the README's four-leg table says what the semantic
/// leg yields, and the shipped script must agree.
#[test]
fn the_readme_says_what_the_semantic_leg_yields() {
    let readme = std::fs::read_to_string("../../templates/memory-hive/README.md").expect("README");
    let row = readme
        .lines()
        .find(|l| l.starts_with("| semantic |"))
        .expect("the semantic row of the four-leg table");
    assert!(
        row.contains("facts + episodes"),
        "the table still promises a fact-only leg: {row}"
    );
    let script = script_of(RECALL_CONFIG);
    assert!(
        script.contains(r#"SEM_OWNER_KIND = {"facts": "fact", "episodes": "episode"}"#),
        "the leg's own owner map is what the sentence describes"
    );
    assert!(
        readme.contains("Backfilling the episodes of a hive older than GH #519"),
        "a hive that predates the change needs its documented store-op path"
    );
}

// ══════════════════════════════════════════════════ 5. the nightly backfill

const DREAM_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

/// One reply into the nightly chain: the phase on the context, the op on the
/// hop, the rows in the first turn — the shape `rows_of` reads.
fn dream_reply(phase: &str, op: &str, rows: Value) -> Value {
    json!({
        "header": {"context": {"mem_phase": phase, "dream_run": "n-1",
                               "dream_to": "2026-08-29T00:00:00Z"},
                   "hop": {"operation": op, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "d",
                      "text": rows.to_string()}]
    })
}

/// The queue the backfill parks keeps BOTH owner kinds. It used to drop
/// everything that was not a fact, which is why a queued episode row could sit
/// in a live store forever without anybody paying for its vector.
#[test]
fn the_nightly_queue_keeps_episode_owners() {
    let out = run(
        &script_of(DREAM_CONFIG),
        dream_reply(
            "embed-backfill",
            "select",
            json!([
                {"id": "emb-f", "owner_table": "facts", "owner_id": "f1"},
                {"id": "emb-e", "owner_table": "episodes", "owner_id": "ep1"},
                {"id": "emb-x", "owner_table": "skills", "owner_id": "s1"}
            ]),
        ),
    );
    let parked = call(out.first().expect("the park"), 0);
    let queue: Value =
        meclaw_core::serde_json::from_str(parked["row"]["payload"].as_str().unwrap()).unwrap();
    let kinds: Vec<&str> = queue
        .as_array()
        .expect("queue")
        .iter()
        .map(|r| r["owner_table"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["facts", "episodes"],
        "both owner kinds are queued, and an unknown one is not: {queue}"
    );
}

/// A queue that holds only episodes asks `episodes` straight away — it must not
/// ask `facts` for an empty id set and then park with nothing to do.
#[test]
fn a_queue_of_episodes_alone_looks_the_text_up_in_episodes() {
    let queue = json!([{"id": "emb-e", "owner_table": "episodes", "owner_id": "ep1"}]);
    let out = run(
        &script_of(DREAM_CONFIG),
        dream_reply(
            "embed-owners",
            "select",
            json!([{"key": "n-1", "kind": "embed-queue", "payload": queue.to_string()}]),
        ),
    );
    let ask = call(out.first().expect("the lookup"), 0);
    assert_eq!(ask["table"], "episodes");
    assert_eq!(ask["columns"], json!(["id", "content"]));
    assert_eq!(ask["where"]["id"]["in"], json!(["ep1"]));
}

/// A mixed queue does the two lookups ONE AFTER THE OTHER. Both in one bundle
/// would fan the chain out into two embed calls over one queue, and the second
/// would pay a paid embedder for rows the first already sent.
#[test]
fn a_mixed_queue_asks_facts_first_and_episodes_after() {
    let queue = json!([
        {"id": "emb-f", "owner_table": "facts", "owner_id": "f1"},
        {"id": "emb-e", "owner_table": "episodes", "owner_id": "ep1"}
    ]);
    let queue_row = json!([{"key": "n-1", "kind": "embed-queue", "payload": queue.to_string()}]);
    let first = run(
        &script_of(DREAM_CONFIG),
        dream_reply("embed-owners", "select", queue_row.clone()),
    );
    assert_eq!(first.len(), 1, "one lookup per hop: {first:?}");
    let ask = call(&first[0], 0);
    assert_eq!(ask["table"], "facts");
    assert_eq!(ask["columns"], json!(["id", "claim"]));

    // …and the episode half is asked from the phase that follows the fact park.
    let second = run(
        &script_of(DREAM_CONFIG),
        dream_reply("embed-ep-owners", "select", queue_row),
    );
    let ask = call(&second[0], 0);
    assert_eq!(ask["table"], "episodes");
    assert_eq!(ask["where"]["id"]["in"], json!(["ep1"]));
}

/// The send folds both text maps into one call, so a night that backfilled a
/// fact and an episode makes ONE request of the embedder.
#[test]
fn the_send_folds_both_owner_kinds_into_one_call() {
    let queue = json!([
        {"id": "emb-f", "owner_table": "facts", "owner_id": "f1"},
        {"id": "emb-e", "owner_table": "episodes", "owner_id": "ep1"}
    ]);
    let out = run(
        &script_of(DREAM_CONFIG),
        dream_reply(
            "embed-send",
            "select",
            json!([
                {"key": "n-1", "kind": "embed-queue", "payload": queue.to_string()},
                {"key": "n-1", "kind": "embed-claims",
                 "payload": json!({"f1": "alex trinkt Tee"}).to_string()},
                {"key": "n-1", "kind": "embed-claims-ep",
                 "payload": json!({"ep1": "Ich trinke abends Tee."}).to_string()}
            ]),
        ),
    );
    assert_eq!(out.len(), 1, "one embed call per night: {out:?}");
    assert_eq!(route(&out[0]), "embed");
    let items = call(&out[0], 0);
    assert_eq!(
        items["items"],
        json!([{"embedding_id": "emb-f", "text": "alex trinkt Tee"},
               {"embedding_id": "emb-e", "text": "Ich trinke abends Tee."}]),
        "both owner kinds, in queue order: {items}"
    );
}
