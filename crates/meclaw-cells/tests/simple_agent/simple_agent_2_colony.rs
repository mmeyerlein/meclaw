//! `simple-agent@2` -- the agent of a colony (R-SA-37, plan A1).
//!
//! `@1` was a sealed agent with its own affinity, its own access and fixed
//! grants; `@2` keeps no record of people and no keys: affinity, access and
//! the llm-registry are the colony's, reached over contract lanes the embedder
//! draws. What this file pins: the agent has neither child, every model cell
//! asks the colony's access under the grant it was born with, the identity
//! arrives by pack from the colony's affinity, the brief is asked per turn
//! with the round (and refused without one), the affinity tool writes into the
//! colony's entry, a birth without the credential edge is refused, and a
//! native Anthropic brain is reached with the colony's key.
//!
//! The colony is grown by `simple_agent_loop::boot` (Form::V2): the colony's
//! `access` (`access@<shipped>`, grants seeded) and `affinity`
//! (`affinity@<shipped>`) beside the agent, the edges drawn by the root.
//! Real time with hang marks only (AGENTS rule 11).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
#[allow(unused_imports)]
use crate::simple_agent_loop; // harness_fold: mod simple_agent_loop;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::hive_contract::{check_required_lanes, contract_from_cell_dir};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::TemplatesRegistry;
use meclaw_colony::{ColonyMsg, MutationDoorOutcome};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::canned_chat_completion;
use simple_agent_loop::{
    AGENT_ID, ANTHROPIC_KEY, AUDIENCE, MODEL_CELLS, OPENROUTER_KEY, SUBSCRIBER, as_map, body_of,
    boot, colony_edges, copy_resolved, count_route, grant_of, next_on, read_json, repo, shipped,
    system_text, wait_route,
};

/// The lanes `@2` insists the embedder wires at birth (OR-AG-6).
const REQUIRED: [&str; 4] = ["in_briefing", "in_model", "in_pack", "in_sealed"];

/// The colony affinity's answer to a brief it may not serve (affinity's brief
/// script, `DENIED`).
const DENIED: &str = "nothing is disclosed to this audience";

fn v2() -> Value {
    read_json(&repo("templates/simple-agent/config.json"))
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn lane<'a>(cfg: &'a Value, side: &str, route: &str) -> &'a Value {
    cfg["params"]["contract"][side]
        .as_array()
        .and_then(|l| l.iter().find(|x| x["route"] == route))
        .unwrap_or_else(|| panic!("the rim {side} `{route}`"))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}

/// `@2` laid out as a colony grows it, under `<td>/main/agent`.
fn instantiated() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let agent = td.path().join("main/agent");
    copy_resolved(&repo("templates/simple-agent"), &agent, 0);
    (td, agent)
}

/// A turn with extra hop keys (the counterpart of the brief leg, a round).
fn turn_with(turn_id: &str, text: &str, hop: Value) -> Message {
    let mut head = json!({"route": "turn", "turn_id": turn_id, "session_id": "s1",
                          "channel": "c1", "audience_set": AUDIENCE});
    for (k, v) in as_map(&hop) {
        head[k] = v;
    }
    MessageBuilder::new(Path::new("/agent"))
        .hop(as_map(&head))
        .context(Map::new())
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// The logged headers of every message on `route` from under `from` to under `to`.
async fn headers_on(
    h: &meclaw_testing::ColonyHandle,
    from: &str,
    to: &str,
    route: &str,
) -> Vec<Value> {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| r.from_path.starts_with(from) && r.to_path.starts_with(to))
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == route)
        .collect()
}

/// The whole request body of every brain call, as text.
async fn brain_bodies(stubs: &simple_agent_loop::Stubs) -> Vec<String> {
    stubs
        .brain
        .recorded_requests()
        .await
        .iter()
        .map(|r| r.body.to_string())
        .collect()
}

// ═══════════════════════════════════════════════════════ the file

#[test]
fn sa2_has_no_access_and_no_affinity_child() {
    if !shipped() {
        return;
    }
    let dir = repo("templates/simple-agent");
    for child in ["access", "affinity"] {
        assert!(
            !dir.join(child).join("config.json").exists(),
            "@2 carries no `{child}`: it is the colony's (R-SA-37)"
        );
    }
    let tpl = read_json(&dir.join("template.json"));
    assert!(
        tpl["version"].as_str().is_some_and(|v| v.starts_with("2.")),
        "the break is a major: {}",
        tpl["version"]
    );
    assert!(
        tpl["requires"].get("credentials").is_none(),
        "@2 names no fixed consumers: {}",
        tpl["requires"]
    );
    let cfg = v2();
    let raw = cfg.to_string();
    for inner in ["./access", "./affinity", "template-simple-agent"] {
        assert!(!raw.contains(inner), "@2 names `{inner}`");
    }
    let routes = |side: &str| -> Vec<String> {
        sorted(
            cfg["params"]["contract"][side]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|l| l["route"].as_str().map(str::to_string))
                .collect(),
        )
    };
    assert_eq!(
        routes("accepts"),
        sorted(
            [
                "turn",
                "tool_result",
                "close",
                "in_pack",
                "in_model",
                "mutation_committed",
                "in_sealed",
                "in_briefing",
                "in_propose_ack",
                "in_conversations",
                "in_note",
            ]
            .map(str::to_string)
            .to_vec()
        ),
        "@2 accepts the turn door and the colony's lanes"
    );
    assert_eq!(
        routes("emits"),
        sorted(
            [
                "answer",
                "calls",
                "close_report",
                "model_refused",
                "error",
                "credential_request",
                "pack_ack",
                "brief",
                "affinity_propose",
                "conversations",
            ]
            .map(str::to_string)
            .to_vec()
        ),
        "@2 emits the turn door and the colony's lanes"
    );
    // The frozen copy keeps both (R-SA-37): `figure@1.0.5` pins it.
    let v1 = repo("templates/simple-agent@1.1.4");
    if v1.join("config.json").is_file() {
        assert!(v1.join("access").is_dir() && v1.join("affinity").is_dir());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_every_model_cell_asks_the_colony_access() {
    if !shipped() {
        return;
    }
    // The file: no consumer is born with a grant, every one admits the
    // embedder's (immutable, so set at birth via `override_params`), and the
    // rim opens both credential lanes at exactly these eight cells.
    let (_ti, agent) = instantiated();
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&agent, &registry).expect("@2 parses");
    for cell in MODEL_CELLS {
        let cfg = read_json(&agent.join(cell).join("config.json"));
        assert_eq!(
            cfg["params"]["credential_grant_id"].as_str().unwrap_or(""),
            "",
            "{cell}: the template names no grant"
        );
        let node = parsed
            .cells
            .iter()
            .find(|x| x.rel_path == cell)
            .unwrap_or_else(|| panic!("a cell {cell}"));
        let mut grant = json!({"credential_grant_id": grant_of(cell, "openrouter")});
        if cfg["cell"]["type"] == "llm" {
            grant["api_key"] = json!("");
        }
        check_override_params(node, Some(cell), "simple-agent@2", &grant)
            .unwrap_or_else(|why| panic!("{cell}: the grant is set at birth: {why:?}"));
    }
    let cfg = v2();
    let at = |side, route| sorted(strings(&lane(&cfg, side, route)["at"]));
    let want = sorted(MODEL_CELLS.iter().map(|c| format!("./{c}")).collect());
    assert_eq!(at("emits", "credential_request"), want);
    assert_eq!(at("accepts", "in_sealed"), want);

    // The run: every cell that asks is answered by the colony's access, and
    // every model call carries the colony's key -- the strict check is on
    // the wire of both stubs.
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hello.", "stop")],
        json!({"door": {"agent_id": AGENT_ID}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_with("t-1", "Hi.", json!({}))).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Hello.");
    h.send(
        MessageBuilder::new(Path::new("/agent"))
            .hop(as_map(&json!({"route": "close"})))
            .context(Map::new())
            .body(Body::Inline(
                json!({"messages": [], "close": {"session_id": "s1"}}),
            ))
            .ttl(400)
            .build(),
    )
    .await;
    next_on(&h, &mut ports, "close_report", &mut seen).await;
    h.quiesce().await;
    let mut asked = Vec::new();
    for cell in MODEL_CELLS {
        let path = format!("/agent/{cell}");
        let asks = headers_on(&h, &path, "/access", "in_invoke").await;
        if asks.is_empty() {
            continue;
        }
        for a in &asks {
            assert_eq!(
                a["context"]["requester"],
                format!("agent:agent/{cell}"),
                "{cell}: the requester is the edge's"
            );
        }
        assert!(
            !headers_on(&h, "/access", &path, "in_sealed")
                .await
                .is_empty(),
            "{cell} asked the colony's access and got no sealed box"
        );
        asked.push(cell);
    }
    for must in ["brain", "memory-hive/closer"] {
        assert!(asked.contains(&must), "{must} asked the colony: {asked:?}");
    }
    let bearer = format!("Bearer {OPENROUTER_KEY}");
    for (stub, reqs) in [
        ("brain", stubs.brain.recorded_requests().await),
        ("background", stubs.background.recorded_requests().await),
    ] {
        assert!(!reqs.is_empty(), "{stub}: a call went out");
        for r in &reqs {
            assert_eq!(
                r.headers.get("authorization").map(String::as_str),
                Some(bearer.as_str()),
                "{stub} {}: the colony's key on the wire",
                r.path
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_identity_arrives_by_pack_from_the_colony_affinity() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("I am Ava.", "stop")],
        json!({"door": {"agent_id": AGENT_ID}}),
    )
    .await;
    // The enrolment (A5 builds it as the `enrol` cell; here the root writes
    // it): the agent's entry, its disclosure to itself, the subscription.
    let me = format!("agent:{AGENT_ID}");
    let entity = format!("entity:{AGENT_ID}");
    let ops = [
        json!({"op": "upsert_entity", "entity_id": entity, "kind": "agent",
               "display_name": "Ava Quill",
               "aieos": simple_agent_loop::aieos("to keep the lighthouse")}),
        json!({"op": "set_disclosure", "entity_id": entity, "field_path": "aieos",
               "audience": me, "audience_set": [me], "mode": "share"}),
        json!({"op": "subscribe", "subject": entity, "slots": ["identity"]}),
    ];
    for (i, op) in ops.iter().enumerate() {
        h.send(
            MessageBuilder::new(Path::new("/affinity"))
                .hop(as_map(&json!({"route": "in_propose"})))
                .context(as_map(&json!({"actor": me, "subscriber": SUBSCRIBER})))
                .body(Body::Inline(json!({"messages": [
                    {"origin": "assistant", "type": "tool_call", "id": format!("enrol-{i}"),
                     "text": op.to_string()}]})))
                .ttl(400)
                .build(),
        )
        .await;
        h.quiesce().await;
    }
    wait_route(&h, "/affinity", "/agent", "in_pack", 1).await;
    wait_route(&h, "/agent", "/affinity", "in_pack_ack", 1).await;
    let acks = headers_on(&h, "/agent", "/affinity", "in_pack_ack").await;
    assert!(
        acks.iter().all(|a| a["context"]["pack_sub"]
            .as_str()
            .is_some_and(|s| !s.is_empty())),
        "the receipt names the subscriber row it books: {acks:?}"
    );
    h.send(turn_with("t-1", "Who are you?", json!({}))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    let reqs = stubs.brain.recorded_requests().await;
    let system = system_text(&reqs[0]);
    assert!(system.contains("Ava"), "the turn knows the agent: {system}");
    assert!(system.contains("lighthouse"), "{system}");
    assert!(
        system.contains("You are an AI agent"),
        "the frame: {system}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_brief_is_asked_per_turn_with_the_round() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("One.", "stop"),
            canned_chat_completion("Two.", "stop"),
            canned_chat_completion("Three.", "stop"),
        ],
        json!({"door": {"agent_id": AGENT_ID}}),
    )
    .await;
    let mut seen = Vec::new();
    for t in ["t-1", "t-2"] {
        h.send(turn_with(t, "Hello.", json!({"counterpart": "peer:north"})))
            .await;
        next_on(&h, &mut ports, "answer", &mut seen).await;
    }
    // A turn without a counterpart asks nobody.
    h.send(turn_with("t-3", "Hello.", json!({}))).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let mut briefs = headers_on(&h, "/agent", "/affinity", "in_brief").await;
    briefs.sort_by_key(|b| b["hop"]["turn_id"].as_str().unwrap_or("").to_string());
    assert_eq!(briefs.len(), 2, "one brief per turn with a counterpart");
    for (b, t) in briefs.iter().zip(["t-1", "t-2"]) {
        assert_eq!(b["hop"]["turn_id"], t, "{b}");
        assert_eq!(b["context"]["audience_set"], AUDIENCE, "the round: {b}");
        assert_eq!(b["context"]["asker"], SUBSCRIBER, "{b}");
    }
    assert_eq!(
        headers_on(&h, "/affinity", "/agent", "in_briefing")
            .await
            .len(),
        2,
        "every brief comes home"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_brief_without_round_is_refused() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hello.", "stop")],
        json!({"door": {"agent_id": AGENT_ID}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_with(
        "t-1",
        "Hello.",
        json!({"counterpart": "peer:north", "audience_set": ""}),
    ))
    .await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Hello.");
    h.quiesce().await;
    // The colony's affinity denies a brief without a round (`no_round`, its
    // audit): the answer discloses nothing, and the collector shows the
    // model the denial instead of a brief (collector README, "The brief leg").
    let back: Vec<String> = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| r.from_path.starts_with("/affinity") && r.to_path.starts_with("/agent"))
        .filter(|r| r.headers_json.contains("\"in_briefing\""))
        .map(|r| r.body_payload.clone().unwrap_or_default())
        .collect();
    assert_eq!(back.len(), 1, "the refusal comes home: {back:?}");
    assert!(
        back[0].contains(DENIED),
        "the colony's affinity discloses nothing to a brief without a round: {}",
        back[0]
    );
    for body in brain_bodies(&stubs).await {
        assert!(
            !body.contains("trust_level"),
            "no pack about the counterpart reaches the brain: {body}"
        );
        assert!(
            body.contains(DENIED),
            "the model is shown the denial: {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_propose_goes_to_the_colony_entry() {
    if !shipped() {
        return;
    }
    let good = json!({"op": "upsert_entity", "entity_id": "entity:ava", "kind": "agent",
                      "display_name": "Ava Quill",
                      "aieos": simple_agent_loop::aieos("to map the coast")});
    // The colony's affinity refuses a trust without its fields.
    let bad = json!({"op": "set_trust"});
    let (_td, stubs, h, mut ports) = boot(
        vec![
            mock_openai::canned_tool_calls(vec![("c-1", "affinity_propose", &good.to_string())]),
            mock_openai::canned_tool_calls(vec![("c-2", "affinity_propose", &bad.to_string())]),
            canned_chat_completion("Noted.", "stop"),
        ],
        json!({"door": {"agent_id": AGENT_ID}}),
    )
    .await;
    h.send(turn_with("t-1", "Remember who you are.", json!({})))
        .await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Noted.");
    let proposals = headers_on(&h, "/agent", "/affinity", "in_propose").await;
    assert_eq!(proposals.len(), 2, "both writes reach the colony's entry");
    for p in &proposals {
        assert_eq!(p["context"]["actor"], SUBSCRIBER, "the edge's actor: {p}");
    }
    let wire = brain_bodies(&stubs).await;
    assert!(
        wire[1].contains("accepted"),
        "the colony's ack is the tool's result: {}",
        wire[1]
    );
    assert!(
        wire[2].contains("error:"),
        "a refusal is a named error in the round, never silent: {}",
        wire[2]
    );
    assert_eq!(
        count_route(&h, "/agent/door", "/agent/affinity", "").await,
        0,
        "no own affinity"
    );
}

/// `/main`-relative edges (`./agent/brain`) as the colony's edge table.
fn root_table(edges: &[Value]) -> EdgeTable {
    let abs = |p: &str| {
        if let Some(rest) = p.strip_prefix("./") {
            format!("/{rest}")
        } else {
            p.to_string()
        }
    };
    let mut t = EdgeTable::new();
    for e in edges {
        let spec: meclaw_colony::config::EdgeSpec =
            meclaw_core::serde_json::from_value(e.clone()).expect("an edge");
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

#[test]
fn sa2_birth_without_in_sealed_edge_is_refused() {
    if !shipped() {
        return;
    }
    let cfg = v2();
    let required: Vec<String> = sorted(
        cfg["params"]["contract"]["accepts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|l| l["required"] == true)
            .filter_map(|l| l["route"].as_str().map(str::to_string))
            .collect(),
    );
    assert_eq!(required, REQUIRED.map(str::to_string).to_vec(), "OR-AG-6");
    let (_td, agent) = instantiated();
    for cell in MODEL_CELLS {
        meclaw_testing::override_params_on_disk(
            &agent.join(cell),
            &json!({"credential_grant_id": grant_of(cell, "openrouter")}),
        );
    }
    let contract = contract_from_cell_dir(&agent, "/agent").expect("@2 has a contract");
    let mut full = colony_edges(&agent);
    full.push(json!({"from": "./registry", "to": "./agent",
                     "condition": "has(hop.route) && hop.route == 'in_model'"}));
    check_required_lanes(std::slice::from_ref(&contract), &root_table(&full))
        .unwrap_or_else(|e| panic!("the embedder's edges satisfy the birth: {e:?}"));
    let without = |route: &str| -> Vec<Value> {
        full.iter()
            .filter(|e| {
                e["lane"] != route
                    && !e["modifier"]["set_hop"]["route"]
                        .as_str()
                        .is_some_and(|r| r == format!("'{route}'"))
                    && !(route == "in_model" && e["from"] == "./registry")
            })
            .cloned()
            .collect()
    };
    for route in REQUIRED {
        let err = check_required_lanes(
            std::slice::from_ref(&contract),
            &root_table(&without(route)),
        )
        .expect_err(&format!("a birth without `{route}` is refused"));
        assert!(
            format!("{err:?}").contains(route),
            "{route}: the refusal names the lane: {err:?}"
        );
    }
}

/// An Anthropic Messages answer.
fn anthropic_answer(text: &str) -> MockResponse {
    let body = json!({
        "id": "msg_sa2", "type": "message", "role": "assistant", "model": "brain-stub",
        "content": [{"type": "text", "text": text}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_anthropic_native_reaches_the_brain() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![anthropic_answer("Hello from Anthropic.")],
        json!({"door": {"agent_id": AGENT_ID},
               "brain": {"provider": "anthropic",
                         "credential_grant_id": grant_of("brain", "anthropic")}}),
    )
    .await;
    h.send(turn_with("t-1", "Hi.", json!({}))).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Hello from Anthropic.");
    let reqs = stubs.brain.recorded_requests().await;
    let req = &reqs[0];
    assert!(
        req.path.ends_with("/messages"),
        "the native lane: {}",
        req.path
    );
    let header = |k: &str| req.headers.get(k).map(String::as_str);
    assert_eq!(header("x-api-key"), Some(ANTHROPIC_KEY), "the colony's key");
    assert!(header("anthropic-version").is_some(), "the native wire");
    assert_eq!(header("authorization"), None, "the key travels once");
    assert!(
        header("x-title").is_some_and(|v| !v.is_empty()),
        "the app id"
    );
    assert!(
        !headers_on(&h, "/access", "/agent/brain", "in_sealed")
            .await
            .is_empty(),
        "one grant per provider, from the colony's access"
    );
    assert_eq!(
        count_route(&h, "/agent/brain", "/agent/access", "").await,
        0,
        "the brain asks the colony's access and no own"
    );
}

// ═══════════════════════════════════════════════════════ the enrolment (A5)

/// A commit receipt at the agent's rim, with the hop keys the mutation door
/// stamps (`docs/meclaw-overview.md`, `mutation_receipts`).
fn receipt(form: &str, scope: &str) -> Message {
    MessageBuilder::new(Path::new("/agent"))
        .hop(as_map(&json!({"route": "mutation_committed", "form": form,
                            "scope": scope, "outcome": "committed", "changes": []})))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

/// The `in_propose` writes the enrolment sent on its step `step` (`upsert`,
/// `disclose:<i>:<n>`, `subscribe`, ...).
async fn enrol_steps(h: &meclaw_testing::ColonyHandle, step: &str) -> usize {
    enrol_steps_at(h, "", step).await
}

/// [`enrol_steps`] of an agent at the level `level` (`""` is the root).
async fn enrol_steps_at(h: &meclaw_testing::ColonyHandle, level: &str, step: &str) -> usize {
    headers_on(
        h,
        &format!("{level}/agent"),
        &format!("{level}/affinity"),
        "in_propose",
    )
    .await
    .iter()
    .filter(|p| p["context"]["sa_enrol"] == step)
    .count()
}

fn enrol_overlay(identity: Value) -> Value {
    json!({"door": {"agent_id": AGENT_ID},
           "enrol": {"agent_id": AGENT_ID, "identity": identity}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_enrol_runs_on_the_birth_receipt_not_on_a_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("I am Ava.", "stop")],
        enrol_overlay(simple_agent_loop::aieos("to keep the lighthouse")),
    )
    .await;
    // The run's own menu receipt names no form and no scope, and a mutation
    // elsewhere in the colony is not the agent's (OR-AG-48): no write.
    h.send(receipt("single", "/elsewhere")).await;
    h.quiesce().await;
    assert_eq!(
        enrol_steps(&h, "upsert").await,
        0,
        "a receipt that is not the agent's enrols nothing"
    );
    h.send(receipt("boot", "/")).await;
    wait_route(&h, "/affinity", "/agent/curator", "in_pack", 1).await;
    wait_route(&h, "/agent", "/affinity", "in_pack_ack", 1).await;
    h.quiesce().await;
    assert_eq!(
        enrol_steps(&h, "upsert").await,
        1,
        "one upsert on the birth"
    );
    assert_eq!(enrol_steps(&h, "subscribe").await, 1);
    let subs = headers_on(&h, "/agent", "/affinity", "in_propose").await;
    let sub = subs
        .iter()
        .find(|p| p["context"]["sa_enrol"] == "subscribe")
        .expect("the subscription");
    assert_eq!(
        sub["context"]["subscriber"], SUBSCRIBER,
        "from the edge: {sub}"
    );
    h.send(turn_with("t-1", "Who are you?", json!({}))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    let system = system_text(&reqs[0]);
    assert!(
        system.contains("lighthouse"),
        "the enrolled identity: {system}"
    );
    assert_eq!(
        enrol_steps(&h, "upsert").await,
        1,
        "a turn enrols nothing again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_enrol_is_idempotent_on_recommit() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("I am Ava.", "stop")],
        enrol_overlay(simple_agent_loop::aieos("to keep the lighthouse")),
    )
    .await;
    h.send(receipt("boot", "/")).await;
    // upsert, the one disclosure (`aieos`), the subscription.
    wait_route(&h, "/affinity", "/agent", "in_propose_ack", 3).await;
    h.quiesce().await;
    // A mutation under the agent's own path: the same writes once more.
    h.send(receipt("single", "/agent/tools")).await;
    wait_route(&h, "/affinity", "/agent", "in_propose_ack", 6).await;
    h.quiesce().await;
    // The turn reads the identity from the curator's pack: every pack the
    // affinity sent is acknowledged before the turn (a marker, no sleep).
    let packs = count_route(&h, "/affinity", "/agent/curator", "in_pack").await;
    wait_route(&h, "/agent", "/affinity", "in_pack_ack", packs.max(1)).await;
    let props = headers_on(&h, "/agent", "/affinity", "in_propose").await;
    assert_eq!(props.len(), 6, "the same three writes twice: {props:?}");
    for step in ["upsert", "disclose:0:1", "subscribe"] {
        assert_eq!(enrol_steps(&h, step).await, 2, "{step}");
    }
    let acks = headers_on(&h, "/affinity", "/agent", "in_propose_ack").await;
    assert!(
        acks.iter()
            .all(|a| a["hop"]["propose_outcome"] == "ack" && a["hop"]["outcome"] == "accepted"),
        "every write accepted again: {acks:?}"
    );
    h.send(turn_with("t-1", "Who are you?", json!({}))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    assert!(
        !seen
            .iter()
            .any(|m| body_of(m)["error"]["error_code"] == "enrol_refused"),
        "no refusal on the rim"
    );
    let reqs = stubs.brain.recorded_requests().await;
    let system = system_text(&reqs[0]);
    assert_eq!(
        system.matches("lighthouse").count(),
        1,
        "the identity once in the brain's system: {system}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_first_turn_does_not_wait_for_identity() {
    if !shipped() {
        return;
    }
    // No `metadata.instance_id`: the colony's affinity refuses the entry
    // (its README, the mandatory AIeOS paths).
    let mut broken = simple_agent_loop::aieos("to keep the lighthouse");
    broken["metadata"] = json!({});
    let (_td, _stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hello.", "stop")],
        enrol_overlay(broken),
    )
    .await;
    h.send(receipt("boot", "/")).await;
    let mut seen = Vec::new();
    let refused = next_on(&h, &mut ports, "error", &mut seen).await;
    let err = &body_of(&refused)["error"];
    assert_eq!(
        err["error_code"], "enrol_refused",
        "named, never silent: {err}"
    );
    assert_eq!(err["step"], "upsert", "{err}");
    h.send(turn_with("t-1", "Hi?", json!({}))).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Hello.");
    assert_eq!(
        enrol_steps(&h, "subscribe").await,
        0,
        "no subscription after a refusal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_turn_before_any_enrolment_answers_without_identity() {
    if !shipped() {
        return;
    }
    // No receipt at all: the enrolment has not even started, and the turn
    // does not wait for it -- it answers without an identity slot.
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hello.", "stop")],
        enrol_overlay(simple_agent_loop::aieos("to keep the lighthouse")),
    )
    .await;
    h.send(turn_with("t-1", "Hi?", json!({}))).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Hello.");
    let reqs = stubs.brain.recorded_requests().await;
    let system = system_text(&reqs[0]);
    assert!(
        !system.contains("lighthouse"),
        "no identity before the enrolment: {system}"
    );
    assert_eq!(enrol_steps(&h, "upsert").await, 0, "a turn enrols nothing");
}

/// Knock one manifest at the colony's mutation door (no sender, as
/// `--apply` and the HTTP door knock) and return its verdict.
async fn knock_manifest(h: &meclaw_testing::ColonyHandle, manifest: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload: json!({"manifest": manifest}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony's inbox is open");
    ack_rx.await.expect("the door answers")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_enrol_runs_on_a_manifest_receipt_one_level_down() {
    if !shipped() {
        return;
    }
    // A builder block arrives as a manifest, and a manifest's receipt says
    // scope `/`: it does not say where it was. An agent one level down
    // enrols on it -- entry, disclosure, subscription -- without the boot.
    let (_td, _stubs, h, mut ports) = simple_agent_loop::boot_form(
        simple_agent_loop::Form::V2Deep,
        vec![canned_chat_completion("I am Ava.", "stop")],
        enrol_overlay(simple_agent_loop::aieos("to keep the lighthouse")),
    )
    .await;
    let level = simple_agent_loop::LEVEL;
    let agent = format!("{level}/agent");
    let mut seen = Vec::new();
    let boot_receipt = next_on(&h, &mut ports, "mutation_committed", &mut seen).await;
    assert_eq!(
        boot_receipt.headers.hop.get("form").and_then(Value::as_str),
        Some("boot"),
        "{:?}",
        boot_receipt.headers.hop
    );
    h.quiesce().await;
    assert_eq!(
        enrol_steps_at(&h, level, "upsert").await,
        0,
        "the boot's receipt never reached the agent"
    );
    // A real manifest through the door, at a level that is not the agent's.
    let outcome = knock_manifest(
        &h,
        json!([{"scope": "/", "diff": {"add_edges": [
            {"from": "./sink", "to": "./sink", "condition": "has(hop.route) && hop.route == 'a5_never'"}
        ]}}]),
    )
    .await;
    assert!(
        matches!(
            outcome,
            MutationDoorOutcome::Manifest(meclaw_colony::ManifestOutcome::Committed { .. })
        ),
        "precondition: the manifest commits: {outcome:?}"
    );
    let receipt = loop {
        let m = next_on(&h, &mut ports, "mutation_committed", &mut seen).await;
        if m.headers.hop.get("form").and_then(Value::as_str) == Some("manifest") {
            break m;
        }
    };
    assert_eq!(
        receipt.headers.hop["scope"], "/",
        "{:?}",
        receipt.headers.hop
    );
    // The door's receipt, verbatim, at the agent's rim.
    h.send(
        MessageBuilder::new(Path::new(&agent))
            .hop(receipt.headers.hop.clone())
            .context(Map::new())
            .body(Body::Inline(json!({"messages": []})))
            .ttl(400)
            .build(),
    )
    .await;
    wait_route(
        &h,
        &format!("{level}/affinity"),
        &agent,
        "in_propose_ack",
        3,
    )
    .await;
    h.quiesce().await;
    for step in ["upsert", "disclose:0:1", "subscribe"] {
        assert_eq!(enrol_steps_at(&h, level, step).await, 1, "{step}");
    }
    let subs = headers_on(&h, &agent, &format!("{level}/affinity"), "in_propose").await;
    let sub = subs
        .iter()
        .find(|p| p["context"]["sa_enrol"] == "subscribe")
        .expect("the subscription");
    assert_eq!(
        sub["context"]["subscriber"], SUBSCRIBER,
        "from the edge: {sub}"
    );
}

// ═══════════════════════════════════════════════════════ the builder block (A5)

/// The builder's `simple_agent` recipe on the example's wish
/// (`examples/simple-agent-colony/wish.json`): the recipe's whole answer.
fn render_block() -> Value {
    let wish = read_json(&repo("examples/simple-agent-colony/wish.json"));
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
                          "text": json!({"recipe": wish["recipe"], "request": "grow the agent",
                                         "params": wish["params"]}).to_string()}],
        }),
    );
    let first = out.into_iter().next().expect("the recipe answered nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the block: {first}"
    );
    first
}

#[test]
fn sa2_builder_block_renders_one_agent_the_door_accepts() {
    if !shipped() {
        return;
    }
    let answer = render_block();
    let manifest = answer["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(manifest.len(), 1, "one block, one declaration: {answer}");
    let decl = &manifest[0];
    let diff = &decl["diff"];
    let nodes = diff["add_nodes"].as_array().expect("add_nodes");
    assert_eq!(nodes.len(), 1, "one agent");
    assert_eq!(nodes[0]["template"], "simple-agent@2.0.0");
    assert_eq!(
        decl["ctx"]["model"], "example-provider/brain-model",
        "ctx.model"
    );
    // The example's manifest is this rendering, byte for byte (drift lock):
    // the wish pins the grants' birth time (`issued_at`, OR-agent-2.A5.9).
    let shipped_decl = simple_agent_loop::example_declaration();
    assert_eq!(
        shipped_decl,
        decl.clone(),
        "examples/simple-agent-colony/grow.json is the recipe's rendering of wish.json"
    );
    // The door's own checks, cell by cell and lane by lane: every override names
    // a cell and a param of @2 (`check_override_params`, the check
    // `/colony/mutations` asks), and the edges satisfy the rim's required lanes
    // (`hive_contract`, a birth check the boot only warns about). The real
    // `meclaw --validate --validate-strict` on the block rendered into the
    // example colony: `crates/meclaw/tests` `gh1310_simple_agent_block_validates_strict`.
    let (_ti, agent) = instantiated();
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&agent, &registry).expect("@2 parses");
    let overrides = as_map(&nodes[0]["override_params"]);
    for (cell, params) in &overrides {
        let node = parsed
            .cells
            .iter()
            .find(|x| x.rel_path == cell.as_str())
            .unwrap_or_else(|| panic!("the override names no cell of @2: {cell}"));
        check_override_params(node, Some(cell.as_str()), "simple-agent@2", params)
            .unwrap_or_else(|why| panic!("{cell}: {why:?}"));
    }
    assert_eq!(
        overrides["brain"]["x_title"], "meclaw-example simple-agent-colony",
        "the block's app id on the brain"
    );
    assert_eq!(overrides["brain"]["http_referer"], "https://meclaw.ai");
    assert_eq!(overrides["door"]["agent_id"], "ava");
    assert_eq!(overrides["enrol"]["agent_id"], "ava");
    assert_eq!(overrides["tool_gate"]["tools_enabled"], json!(["weather"]));
    for (_cell, params) in &overrides {
        assert!(
            params.get("runner_mode").is_none(),
            "no warm/cold per family (OR-AG-53): {params}"
        );
    }
    let edges = diff["add_edges"].as_array().cloned().unwrap_or_default();
    let contract = contract_from_cell_dir(&agent, "/agent").expect("@2 has a contract");
    check_required_lanes(std::slice::from_ref(&contract), &root_table(&edges))
        .unwrap_or_else(|e| panic!("the block satisfies the birth: {e:?}"));
    let neighbours = ["./access", "./affinity", "./llm-registry", "."];
    for e in &edges {
        for end in ["from", "to"] {
            let p = e[end].as_str().unwrap_or_default();
            assert!(
                neighbours.contains(&p) || p == "./agent" || p.starts_with("./agent/"),
                "an edge end outside the agent and the level's three: {e}"
            );
            if let Some(cell) = p.strip_prefix("./agent/") {
                assert!(
                    agent.join(cell).join("config.json").is_file(),
                    "an edge into nothing: {e}"
                );
            }
        }
    }
}

#[test]
fn sa2_builder_block_puts_the_grants_into_the_colony_access() {
    if !shipped() {
        return;
    }
    let answer = render_block();
    let diff = &answer["manifest"][0]["diff"];
    let overrides = &diff["add_nodes"][0]["override_params"];
    let rows = diff["seed_rows"].as_array().expect("seed_rows");
    assert!(
        rows.iter().all(|t| t["target"] == "./access/store"),
        "every row into the level's access, none into the agent: {rows:?}"
    );
    let grants = rows
        .iter()
        .find(|t| t["table"] == "grants")
        .and_then(|t| t["rows"].as_array())
        .expect("grant rows");
    // `tools/web_search` stays anonymous without a search credential (OR-VG-5).
    let spending: Vec<&str> = MODEL_CELLS
        .iter()
        .copied()
        .filter(|c| *c != "tools/web_search")
        .collect();
    assert_eq!(
        grants.len(),
        spending.len(),
        "one grant per consumer: {grants:?}"
    );
    let edges = diff["add_edges"].as_array().expect("edges");
    for cell in spending {
        let id = overrides[cell]["credential_grant_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{cell} is born with no grant: {overrides}"));
        let g = grants
            .iter()
            .find(|g| g["grant_id"] == id)
            .unwrap_or_else(|| panic!("{cell}: {id} has no row in the colony's access"));
        assert_eq!(g["capability"], "credential.read");
        assert_eq!(g["cred_ref"], "cred:openrouter");
        let at = format!("./agent/{cell}");
        assert!(
            edges.iter().any(|e| e["from"] == at.as_str()
                && e["to"] == "./access"
                && e["lane"] == "credential_request"
                && e["modifier"]["set_context"]["requester"]
                    == format!("'{}'", g["requester"].as_str().unwrap_or_default())),
            "{cell}: the ask carries the grant's requester"
        );
        assert!(
            edges.iter().any(|e| e["from"] == "./access"
                && e["to"] == at.as_str()
                && e["lane"] == "in_sealed"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("hop.grant_id == '{id}'")))),
            "{cell}: the sealed box comes back by its grant"
        );
    }
    assert!(
        rows.iter().any(|t| t["table"] == "grant_events"),
        "each grant with its birth event"
    );
}

/// The resident set size in kB of a process (`/proc/<pid>/status`, `VmRSS`).
fn rss_kb(pid: &str) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// Every descendant of this process (`/proc/<pid>/task/<tid>/children`).
fn descendants(pid: &str, out: &mut Vec<String>) {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return;
    };
    for t in tasks.flatten() {
        let kids = std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
        for k in kids.split_whitespace() {
            out.push(k.to_string());
            descendants(k, out);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_example_colony_answers_a_turn_with_the_stub() {
    if !shipped() || !repo("templates/llm-registry/config.json").is_file() {
        return;
    }
    let (_td, stubs, h, mut ports) = simple_agent_loop::boot_form(
        simple_agent_loop::Form::Example,
        vec![canned_chat_completion("I keep the lighthouse.", "stop")],
        json!({}),
    )
    .await;
    h.send(turn_with("t-1", "Who are you?", json!({}))).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "I keep the lighthouse.");
    let reqs = stubs.brain.recorded_requests().await;
    assert!(!reqs.is_empty(), "the brain was asked");
    // The block's app id on the wire (AGENTS rule 15), not only in the params.
    let wish = read_json(&repo("examples/simple-agent-colony/wish.json"));
    let headers = &reqs[0].headers;
    assert_eq!(
        headers.get("x-title").map(String::as_str),
        wish["params"]["app_id"].as_str(),
        "{headers:?}"
    );
    assert_eq!(
        headers.get("http-referer").map(String::as_str),
        Some("https://meclaw.ai"),
        "{headers:?}"
    );
    assert!(
        wait_sealed(&h).await > 0,
        "the brain's key came sealed from the colony's access"
    );
    // RAM of the instance (no criterion, plan A5): the run's process with every
    // child after the turn, and without the children -- what stays once every
    // warm child stopped idle (GH #1276, 30 min).
    let me = std::process::id().to_string();
    let mut kids = Vec::new();
    descendants(&me, &mut kids);
    let children: u64 = kids.iter().map(|k| rss_kb(k)).sum();
    println!(
        "SA2-RAM after-turn colony={}kB children={}kB ({} processes) idle-stopped~{}kB",
        rss_kb(&me),
        children,
        kids.len(),
        rss_kb(&me)
    );
}

/// How many sealed boxes the colony's access delivered into the agent.
async fn wait_sealed(h: &meclaw_testing::ColonyHandle) -> usize {
    wait_route(h, "/access", "/agent", "in_sealed", 1).await;
    count_route(h, "/access", "/agent", "in_sealed").await
}
