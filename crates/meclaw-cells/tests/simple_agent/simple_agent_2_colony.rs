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
use mock_openai::{canned_chat_completion, canned_tool_calls};
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
    // The frozen copy keeps both (R-SA-37); since figure@2 (GH #1311) no
    // template pins it, it stays as the max-agent line.
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

/// Whether `./enrol`'s shipped script writes the entry on a commit receipt
/// with the hop keys `hop`, delivered at `target` (`<agent>/enrol`).
fn enrol_writes(target: &str, hop: Value) -> bool {
    let mut hop = as_map(&hop);
    hop.insert("route".into(), json!("mutation_committed"));
    hop.insert("outcome".into(), json!("committed"));
    hop.insert("changes".into(), json!([]));
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/simple-agent/enrol/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({"target": target, "header": {"hop": hop, "context": {}}, "ttl": 64,
                "messages": [],
                "params": {"agent_id": AGENT_ID,
                           "identity": simple_agent_loop::aieos("to keep the lighthouse")}}),
    );
    !out.is_empty()
}

/// GH #1312 (N1, F1 report, OR-AG-61): with `scopes` and `paths` on the
/// receipt, the enrolment counts a node born at, under or ABOVE the agent and
/// a scope at or under it -- a figure grown by ONE `add_nodes` at the member
/// enrols the agent one level deeper inside it; a sibling's birth, a builder
/// block elsewhere and the colony's container do not. A receipt without the
/// keys keeps the reading before.
#[test]
fn sa2_enrol_reads_where_the_knock_wrote() {
    if !shipped() {
        return;
    }
    let deep = "/m/figure/simple-agent/enrol";
    let flat = "/m/agent/enrol";
    let cases = [
        (
            "one add_nodes bears the figure above the agent",
            deep,
            json!({"form": "single", "scope": "/m", "scopes": ["/m"], "paths": ["/m/figure"]}),
            true,
        ),
        (
            "a sibling of the figure",
            deep,
            json!({"form": "single", "scope": "/m", "scopes": ["/m"], "paths": ["/m/other"]}),
            false,
        ),
        (
            "a block elsewhere and the container's road",
            deep,
            json!({"form": "manifest", "scope": "/", "scopes": ["/m", "/os"], "paths": ["/m/other"]}),
            false,
        ),
        (
            "the block that grows the agent",
            flat,
            json!({"form": "manifest", "scope": "/", "scopes": ["/m", "/os"], "paths": ["/m/agent"]}),
            true,
        ),
        (
            "a sibling at the level",
            flat,
            json!({"form": "single", "scope": "/m", "scopes": ["/m"], "paths": ["/m/other"]}),
            false,
        ),
        (
            "a write inside the agent",
            flat,
            json!({"form": "manifest", "scope": "/", "scopes": ["/m/agent"], "paths": []}),
            true,
        ),
        (
            "a receipt before GH #1312, manifest",
            flat,
            json!({"form": "manifest", "scope": "/"}),
            true,
        ),
        (
            "a receipt before GH #1312, the level",
            flat,
            json!({"form": "single", "scope": "/m"}),
            true,
        ),
        (
            "the boot",
            deep,
            json!({"form": "boot", "scope": "/"}),
            true,
        ),
    ];
    for (why, target, hop, want) in cases {
        assert_eq!(enrol_writes(target, hop), want, "{why}");
    }
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
async fn sa2_enrol_runs_on_a_manifest_receipt_that_wrote_at_the_agent() {
    if !shipped() {
        return;
    }
    // A builder block arrives as a manifest, and a manifest's receipt says
    // scope `/`: it does not say where it was. Since GH #1312 it names its
    // `scopes` and `paths`, so an agent one level down enrols on a manifest
    // that wrote at it -- entry, disclosure, subscription -- and on none that
    // wrote elsewhere (OR-AG-61: before, every manifest counted).
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
    // Two real manifests through the door: one at the root, one inside the agent.
    let never = "has(hop.route) && hop.route == 'a5_never'";
    let mut handed = Vec::new();
    for (scope, from, own) in [("/", "./sink", false), (agent.as_str(), "./watch", true)] {
        let outcome = knock_manifest(
            &h,
            json!([{"scope": scope, "diff": {"add_edges": [
                {"from": from, "to": from, "condition": never}
            ]}}]),
        )
        .await;
        assert!(
            matches!(
                outcome,
                MutationDoorOutcome::Manifest(meclaw_colony::ManifestOutcome::Committed { .. })
            ),
            "precondition: the manifest at {scope} commits: {outcome:?}"
        );
        // The copy re-sent at the agent's rim below surfaces on the sink as
        // well; a receipt already handed on is skipped, never counted.
        let receipt = loop {
            let m = next_on(&h, &mut ports, "mutation_committed", &mut seen).await;
            if m.headers.hop.get("form").and_then(Value::as_str) == Some("manifest")
                && !handed.contains(&m.headers.hop)
            {
                break m;
            }
        };
        handed.push(receipt.headers.hop.clone());
        assert_eq!(
            receipt.headers.hop["scope"], "/",
            "{:?}",
            receipt.headers.hop
        );
        assert_eq!(receipt.headers.hop["scopes"], json!([scope]));
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
        if own {
            wait_route(
                &h,
                &format!("{level}/affinity"),
                &agent,
                "in_propose_ack",
                3,
            )
            .await;
        } else {
            h.quiesce().await;
            assert_eq!(
                enrol_steps_at(&h, level, "upsert").await,
                0,
                "a manifest at the root wrote nothing of the agent's"
            );
        }
    }
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
    let first = render_wish(&wish["params"]);
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the block: {first}"
    );
    first
}

/// The `simple_agent` recipe's first answer on the wish `params` (a manifest or
/// a refusal).
fn render_wish(params: &Value) -> Value {
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
                          "text": json!({"recipe": "simple_agent", "request": "grow the agent",
                                         "params": params}).to_string()}],
        }),
    );
    out.into_iter().next().expect("the recipe answered nothing")
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
    assert_eq!(nodes[0]["template"], "simple-agent@2.2.0");
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
    // `tools/web_search` stays anonymous without a search credential (OR-VG-5),
    // and `strong` without `strong` in the wish (K-3, OR-AG-75).
    let spending: Vec<&str> = MODEL_CELLS
        .iter()
        .copied()
        .filter(|c| *c != "tools/web_search" && *c != "strong")
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

// ─────────────────────────────────── one level deeper (GH #1312, N1, OR-AG-69)

/// `@2`'s rim with every `at` one level down (`./simple-agent/brain`, ...):
/// the form a template carries that holds the agent under a ref (`figure@2`).
fn deeper_contract() -> Value {
    let mut c = v2()["params"]["contract"].clone();
    for side in ["accepts", "emits"] {
        for lane in c[side].as_array_mut().into_iter().flatten() {
            if let Some(at) = lane.get_mut("at").and_then(Value::as_array_mut) {
                for a in at.iter_mut() {
                    let cell = a
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches("./")
                        .to_string();
                    *a = json!(format!("./simple-agent/{cell}"));
                }
            }
        }
    }
    c
}

/// A fixture template `nest` laid out as a colony grows it: a hive whose rim is
/// [`deeper_contract`], with `@2` resolved under `./simple-agent`.
fn nest_instantiated() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let nest = td.path().join("main/nest");
    copy_resolved(
        &repo("templates/simple-agent"),
        &nest.join("simple-agent"),
        0,
    );
    std::fs::write(
        nest.join("config.json"),
        json!({"cell": {"type": "hive"},
               "params": {"ports": [], "contract": deeper_contract()}})
        .to_string(),
    )
    .expect("the nest's config");
    (td, nest)
}

/// The example's wish for the fixture `nest`, its rim handed in as `contract`.
fn nest_wish(extra: Value) -> Value {
    let mut p = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    p["name"] = json!("nest");
    p["template"] = json!("nest@1.0.0");
    p["contract"] = deeper_contract();
    for (k, v) in as_map(&extra) {
        p[k.as_str()] = v;
    }
    p
}

/// The llm cells of `@2`. Its embedder (`memory-hive/embed`) names the app
/// too since GH #1315; `tools/web_search` calls no model and takes no app id.
const LLM_CELLS: [&str; 7] = [
    "brain",
    "strong",
    "curator/summarizer",
    "memory-hive/closer",
    "memory-hive/dialectic",
    "memory-hive/dreamer",
    "memory-hive/judge",
];

#[test]
fn sa2_builder_block_renders_an_agent_one_level_deeper() {
    if !shipped() {
        return;
    }
    let answer = render_wish(&nest_wish(json!({})));
    assert!(
        answer["header"]["error_code"].is_null(),
        "refused: {answer}"
    );
    let manifest = answer["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        manifest.len(),
        1,
        "a registry beside the agent: one declaration"
    );
    let diff = &manifest[0]["diff"];
    let node = &diff["add_nodes"][0];
    assert_eq!(node["template"], "nest@1.0.0");
    let (_td, nest) = nest_instantiated();
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&nest, &registry).expect("the nest parses");
    let overrides = as_map(&node["override_params"]);
    for (cell, params) in &overrides {
        assert!(
            cell.starts_with("simple-agent/"),
            "a key above the agent: {cell}"
        );
        let c = parsed
            .cells
            .iter()
            .find(|x| x.rel_path == cell.as_str())
            .unwrap_or_else(|| panic!("the override names no cell of the nest: {cell}"));
        check_override_params(c, Some(cell.as_str()), "nest", params)
            .unwrap_or_else(|why| panic!("{cell}: {why:?}"));
    }
    for cell in ["door", "enrol", "tool_gate"] {
        assert!(
            overrides.contains_key(&format!("simple-agent/{cell}")),
            "{cell}"
        );
    }
    // The app id reaches every llm cell (W1 finding 3) and, since GH #1315
    // (OR-AG-408 a), the embedder; `strong` only when it is on.
    for cell in LLM_CELLS
        .iter()
        .filter(|c| **c != "strong")
        .chain(&["memory-hive/embed"])
    {
        let o = &overrides[&format!("simple-agent/{cell}")];
        assert_eq!(o["x_title"], "meclaw-example simple-agent-colony", "{cell}");
        assert_eq!(o["http_referer"], "https://meclaw.ai", "{cell}");
    }
    assert!(
        overrides
            .get("simple-agent/tools/web_search")
            .is_none_or(|o| o.get("x_title").is_none()),
        "the search cell calls no model"
    );
    let edges = diff["add_edges"].as_array().cloned().unwrap_or_default();
    let contract = contract_from_cell_dir(&nest, "/nest").expect("the nest has a contract");
    check_required_lanes(std::slice::from_ref(&contract), &root_table(&edges))
        .unwrap_or_else(|e| panic!("the block satisfies the nest's birth: {e:?}"));
    for e in &edges {
        for end in ["from", "to"] {
            let p = e[end].as_str().unwrap_or_default();
            if let Some(cell) = p.strip_prefix("./nest/") {
                assert!(
                    cell.starts_with("simple-agent/"),
                    "an end above the agent: {e}"
                );
                assert!(
                    nest.join(cell).join("config.json").is_file(),
                    "into nothing: {e}"
                );
            }
        }
    }
    let announced = edges
        .iter()
        .find(|e| e["to"] == "./llm-registry" && e["from"] == ".")
        .expect("the announcement");
    assert!(
        announced["modifier"]["set_context"]["model_announced"]
            .as_str()
            .is_some_and(|a| a.contains("\"cell_path\":\"/nest/simple-agent/brain\"")),
        "the brain is announced where it stands: {announced}"
    );
}

/// The `simple_agent` recipe's first answer on the wish `params`, with the
/// builder's own settings (`model_registry_scope`, the assistant's knob).
fn render_wish_with(params: &Value, settings: Value) -> Value {
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
            "params": settings,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "simple_agent", "request": "grow the agent",
                                         "params": params}).to_string()}],
        }),
    );
    out.into_iter().next().expect("the recipe answered nothing")
}

/// K-1 + review C1 (GH #1312): in a member colony the registry stands under
/// `/os` and its pushes arrive at the container the builder names
/// (`model_registry_scope`, `/os/orgs`), exactly as for the assistant. The
/// required lane `in_model` is wired by the entry that gives the agent birth --
/// the door judges each entry on its own -- and the container's edges reach
/// the agent in a second entry. Nothing names `./llm-registry` beside the
/// agent.
#[test]
fn sa2_builder_block_finds_the_registry_through_the_container_scope() {
    if !shipped() {
        return;
    }
    let answer = render_wish_with(
        &nest_wish(json!({"scope": "/os/orgs/o/members/m"})),
        json!({"model_registry_scope": "/os/orgs"}),
    );
    assert!(
        answer["header"]["error_code"].is_null(),
        "refused: {answer}"
    );
    let manifest = answer["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        manifest.len(),
        2,
        "the birth, then the container's road: {answer}"
    );
    assert_eq!(manifest[0]["scope"], "/os/orgs/o/members/m");
    assert_eq!(manifest[1]["scope"], "/os/orgs");
    for d in &manifest {
        for e in d["diff"]["add_edges"].as_array().into_iter().flatten() {
            for end in ["from", "to"] {
                assert!(
                    !e[end].as_str().unwrap_or_default().contains("llm-registry"),
                    "no registry beside the agent: {e}"
                );
            }
        }
    }
    // The birth entry ALONE satisfies the rim: every required lane, `in_model`
    // included (C1: before the fix it stood only in the second entry).
    let birth = manifest[0]["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let (_td, nest) = nest_instantiated();
    let contract = contract_from_cell_dir(&nest, "/nest").expect("the nest has a contract");
    check_required_lanes(std::slice::from_ref(&contract), &root_table(&birth))
        .unwrap_or_else(|e| panic!("the birth entry wires every required lane: {e:?}"));
    assert!(
        birth.iter().any(|e| e["from"] == "./nest"
            && e["to"] == "."
            && e["condition"] == "has(hop.route) && hop.route == 'model_refused'"),
        "a refusal leaves the agent to its level: {birth:?}"
    );
    // The container's road, in the forms the assistant's road has: a push per
    // announced brain onto the agent, the refusal on from the level, the
    // announcement off the level's receipt with the brains it stands for.
    let road = manifest[1]["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let agent = "./o/members/m/nest";
    assert_eq!(
        road[0],
        json!({"from": ".", "to": agent,
               "condition": "has(hop.route) && hop.route == 'in_model' && has(hop.subscriber) \
                             && hop.subscriber == '/os/orgs/o/members/m/nest/simple-agent/brain'",
               "modifier": {"set_hop": {"route": "'in_model'"}}}),
        "{road:?}"
    );
    assert!(
        road.iter()
            .all(|e| !(e["to"] == agent
                && e["condition"].as_str().unwrap_or("").contains("/strong'"))),
        "no strong brain without `strong`: {road:?}"
    );
    let announced = road
        .iter()
        .find(|e| e["modifier"]["set_hop"]["route"] == "'model_subscribe'")
        .expect("the announcement");
    assert_eq!(announced["from"], "./o/members/m");
    assert_eq!(announced["to"], ".");
    assert_eq!(
        announced["modifier"]["set_context"]["model_generation"],
        "'/os/orgs/o/members/m/nest'"
    );
    assert!(
        road.iter().any(|e| e["from"] == "./o/members/m"
            && e["to"] == "."
            && e["condition"] == "has(hop.route) && hop.route == 'model_refused'"
            && e.get("modifier").is_none()),
        "the refusal goes on to the container: {road:?}"
    );
    // Without the setting the rendering is the default one, byte for byte.
    let plain = render_wish_with(&nest_wish(json!({})), json!({}));
    assert_eq!(plain, render_wish(&nest_wish(json!({}))));
}

#[test]
fn sa2_builder_block_refuses_an_unknown_template_without_its_rim() {
    if !shipped() {
        return;
    }
    let mut p = nest_wish(json!({}));
    if let Some(o) = p.as_object_mut() {
        o.remove("contract");
    }
    let answer = render_wish(&p);
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
    let text = answer["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("\"field\": \"contract\""), "{text}");
    // An agent at the container itself has no level between it and the
    // registry's road (OR-agent-2.N1.10): refused by name.
    let answer = render_wish_with(
        &nest_wish(json!({"scope": "/os/orgs"})),
        json!({"model_registry_scope": "/os/orgs"}),
    );
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
}

/// Every grant id a rendered block names: rows, overrides, answer edges.
fn grant_ids(answer: &Value) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let diff = &answer["manifest"][0]["diff"];
    for t in diff["seed_rows"].as_array().into_iter().flatten() {
        for r in t["rows"].as_array().into_iter().flatten() {
            if let Some(id) = r["grant_id"].as_str() {
                out.insert(id.to_string());
            }
        }
    }
    for (_cell, o) in as_map(&diff["add_nodes"][0]["override_params"]) {
        if let Some(id) = o["credential_grant_id"].as_str() {
            out.insert(id.to_string());
        }
    }
    out
}

/// K-2 (GH #1312): two agents of one member -- two roles under one subject --
/// get disjoint grant ids. Before, the id named the subject and the cell only,
/// so the second agent's `seed_rows` overwrote the first one's grants.
#[test]
fn sa2_two_agents_of_one_member_hold_their_own_grants() {
    if !shipped() {
        return;
    }
    let mut first = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    first["name"] = json!("counselor");
    first["agent_id"] = json!("counselor");
    let mut second = first.clone();
    second["name"] = json!("care");
    second["agent_id"] = json!("care");
    let a = grant_ids(&render_wish(&first));
    let b = grant_ids(&render_wish(&second));
    assert!(!a.is_empty() && !b.is_empty(), "{a:?} {b:?}");
    assert!(
        a.is_disjoint(&b),
        "two roles share grant ids: {:?}",
        a.intersection(&b).collect::<Vec<_>>()
    );
    for id in &a {
        assert!(id.contains("/counselor/"), "the agent in the id: {id}");
    }
}

/// K-3 (OR-AG-75): the second brain spends a grant only when the wish turns it
/// on. Without `strong` no row, no override, no edge names it; with it, all three.
#[test]
fn sa2_strong_holds_a_grant_only_when_it_is_on() {
    if !shipped() {
        return;
    }
    let wish = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    let off = render_wish(&wish);
    let text = off["manifest"].to_string();
    assert!(
        !text.contains("./agent/strong"),
        "an edge names strong: {text}"
    );
    assert!(
        as_map(&off["manifest"][0]["diff"]["add_nodes"][0]["override_params"])
            .get("strong")
            .is_none(),
        "an override names strong"
    );
    assert!(
        grant_ids(&off).iter().all(|id| !id.ends_with("/strong")),
        "{:?}",
        grant_ids(&off)
    );
    let mut on = wish.clone();
    on["strong"] = json!("example-provider/strong-model");
    let on = render_wish(&on);
    let overrides = as_map(&on["manifest"][0]["diff"]["add_nodes"][0]["override_params"]);
    let id = overrides["strong"]["credential_grant_id"]
        .as_str()
        .expect("strong is born with its grant");
    assert!(grant_ids(&on).contains(id), "{id}");
    assert!(on["manifest"].to_string().contains("./agent/strong"));
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

/// An `install_app` wish for the app `fig` grown from `figure@2` -- an app
/// whose rim embeds `simple-agent@2` one level down (orga's coordinator has
/// the same shape) -- its rim handed in as `contract`, with the example's
/// credential.
fn embedding_app_wish() -> Value {
    let example = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    let figure = read_json(&repo("templates/figure/config.json"));
    json!({"scope": "/os/orgs/o/members/m", "app": "fig", "template": "figure@2.0.2",
           "screen": "display", "ctx": {"model": "example-provider/brain-model"},
           "declaration": {}, "credential": example["credential"],
           "override_params": {"world": {"figure_id": "fig", "world_id": "w", "role": "npc"}},
           "contract": figure["params"]["contract"]})
}

/// The `install_app` recipe's first answer on `params`, the registry's
/// container `/os/orgs` set (as `meclaw-os` sets it).
fn render_install(params: &Value) -> Value {
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
            "params": {"model_registry_scope": "/os/orgs"},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "install fig",
                                         "params": params}).to_string()}],
        }),
    );
    out.into_iter().next().expect("the recipe answered nothing")
}

/// K-4 (GH #1312, OR-AG-76): an app whose rim REQUIRES `in_sealed` -- the
/// orga coordinator's case, orga-hive O1 report line 541 -- installed without
/// `credential` is refused by name at the recipe. Before, the recipe rendered
/// the app with no credential pair and the door refused its birth with a
/// `hive_contract` message that named the lane, never the missing wish key.
#[test]
fn sa2_install_app_refuses_an_embedded_agent_without_its_credential() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let mut p = embedding_app_wish();
    if let Some(o) = p.as_object_mut() {
        o.remove("credential");
    }
    let answer = render_install(&p);
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
    let text = answer["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("\"field\": \"credential\""), "{text}");
}

/// K-4 (GH #1312, OR-AG-76/-77): `install_app` of an app that embeds
/// `simple-agent@2` draws the agent's required lanes in the entry that
/// instantiates the app -- `in_briefing` and `in_pack` onto the cells the rim
/// names, the level's `in_model` door onto the app -- and the container's road
/// as a second entry, by the block's own function. The agent's search cell
/// stays anonymous as in the block (OR-VG-5). Before, only the credential
/// pairs were drawn (and the search cell was handed the provider key).
#[test]
fn sa2_install_app_draws_the_rim_of_an_embedded_agent() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let answer = render_install(&embedding_app_wish());
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let manifest = answer["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(manifest.len(), 2, "the app, then the container's road");
    let edges = manifest[0]["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let onto = |lane: &str, to: &str| {
        edges
            .iter()
            .any(|e| e["lane"] == lane && e["to"] == to && e["from"] == "./affinity")
    };
    assert!(
        onto("in_briefing", "./apps/fig/simple-agent/collector"),
        "{edges:#?}"
    );
    assert!(
        onto("in_pack", "./apps/fig/simple-agent/curator"),
        "{edges:#?}"
    );
    assert!(
        edges.iter().any(|e| e["from"] == "."
            && e["to"] == "./apps/fig"
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'in_model'"))),
        "the level's in_model door onto the app: {edges:#?}"
    );
    assert_eq!(manifest[1]["scope"], "/os/orgs");
    assert!(
        manifest[1]
            .to_string()
            .contains("hop.subscriber == '/os/orgs/o/members/m/apps/fig/simple-agent/brain'"),
        "{:#}",
        manifest[1]
    );
    assert!(
        !manifest[0]
            .to_string()
            .contains("simple-agent/tools/web_search"),
        "the search cell spends no provider key: {:#}",
        manifest[0]
    );
}

/// GH #1315 (OR-AG-408 a): `install_app` of an app that embeds
/// `simple-agent@2` names the app on the agent's embedder too, like on its llm
/// cells -- the wish's `app_id` on `simple-agent/memory-hive/embed`; the search
/// cell calls no model and gets none. Before, only the llm cells carried it
/// and every embedding of the figure was billed to the colony's default title.
#[test]
fn sa2_install_app_names_the_app_on_the_embedder() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let mut wish = embedding_app_wish();
    wish["app_id"] = json!("example-app fig");
    let answer = render_install(&wish);
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let overrides = app_overrides(&answer);
    for cell in ["brain", "memory-hive/closer", "memory-hive/embed"] {
        let o = &overrides[format!("simple-agent/{cell}")];
        assert_eq!(o["x_title"], "example-app fig", "{cell}: {overrides:#}");
        assert_eq!(o["http_referer"], "https://meclaw.ai", "{cell}");
    }
    assert!(
        overrides["simple-agent/tools/web_search"]
            .get("x_title")
            .is_none(),
        "{overrides:#}"
    );
}

/// The submit gate (`templates/submit/gate`) on `decls`, submitted by
/// `requester`: every message of its first phase.
fn submit_gate(decls: &Value, requester: &str) -> Vec<Value> {
    let gate = repo("templates/submit/gate/config.json");
    let canon = meclaw_testing::code_wire::run_shipped_script(
        concat!(
            "import sys, json, hashlib\n",
            "d = json.load(sys.stdin)\n",
            "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
            "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
        ),
        &decls.to_string(),
    );
    let sha = String::from_utf8(canon.stdout).expect("hex");
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(gate.to_str().expect("a utf-8 path")),
        &json!({
            "target": "/os/operator/submit",
            "reply_to": requester,
            "header": {"hop": {"manifest_sha256": sha}, "context": {}},
            "ttl": 64,
            "manifest": decls,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "op:c1",
                          "text": "{}"}],
            "params": {}
        }),
    )
}

/// The gate's refusal code, or "" when it parked the manifest.
fn gate_refused(out: &[Value]) -> String {
    out.iter()
        .find(|m| m["header"]["route"] == "receipt")
        .and_then(|m| m["header"]["error_code"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Parked and its first question asked: the gate took the form.
fn gate_parked_and_asked(out: &[Value]) -> bool {
    gate_refused(out).is_empty()
        && out.iter().any(|m| m["header"]["route"] == "sstore")
        && out.iter().any(|m| m["header"]["route"] == "ask")
}

/// The operator, and an agent inside the registry's container: the two
/// requesters a builder -> submit carries.
const SUBMITTERS: [&str; 2] = [
    "/os/operator/submit",
    "/os/orgs/o/members/m/assistants/a/talky/brain",
];

/// GH #1315 (OR-AG-79, N1 K-4 item 3): builder -> submit of an app that
/// embeds `simple-agent@2` passes the submit gate. The road `install_app`
/// renders is the block's: the level's door onto the app in the entry that
/// bears it (the bare `in_model` lane, the door's birth probe), one push per
/// announced brain from the container onto the app, addressed one hive deeper
/// (`<app>/simple-agent/brain`), and the announcement off the level's
/// receipt. Before, the gate refused the level's door `model_push_form` and
/// only operator manifests (`--apply`) could grow such an app.
#[test]
fn sa2_install_app_of_an_embedded_agent_passes_the_submit_gate() {
    if !shipped()
        || !repo("templates/figure/config.json").is_file()
        || !repo("templates/submit/gate/config.json").is_file()
    {
        return;
    }
    let manifest = render_install(&embedding_app_wish())["manifest"].clone();
    for requester in SUBMITTERS {
        let out = submit_gate(&manifest, requester);
        assert!(gate_parked_and_asked(&out), "{requester}: {out:#?}");
    }
}

/// GH #1315: the block's own road (`simple_agent` with the registry's
/// container) is the same form one hive shallower and passes the same way.
#[test]
fn sa2_builder_block_in_a_member_colony_passes_the_submit_gate() {
    if !shipped() || !repo("templates/submit/gate/config.json").is_file() {
        return;
    }
    let mut wish = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    wish["scope"] = json!("/os/orgs/o/members/m");
    let answer = render_wish_with(&wish, json!({"model_registry_scope": "/os/orgs"}));
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    for requester in SUBMITTERS {
        let out = submit_gate(&answer["manifest"], requester);
        assert!(gate_parked_and_asked(&out), "{requester}: {out:#?}");
    }
}

/// GH #1315, the other half: the model door is not loosened for anything but
/// that form. A bare `in_model` edge onto a node this manifest does not bring
/// into the world, one whose brains nobody announces, one drawn outside the
/// entry that bears the agent, one with a looser condition; a push one hive
/// deeper to a brain nobody announced; an announcement off a level the agent
/// is not born at -- each is refused, unasked. (Before GH #1315 this lock held
/// the refusal of the whole rendered road; it now holds the foreign edges.)
#[test]
fn sa2_a_foreign_bare_model_edge_stays_refused_at_the_submit_gate() {
    if !shipped()
        || !repo("templates/figure/config.json").is_file()
        || !repo("templates/submit/gate/config.json").is_file()
    {
        return;
    }
    let manifest = render_install(&embedding_app_wish())["manifest"].clone();
    let level_door = |to: &str, abs: &str| {
        json!({"from": ".", "to": to,
               "condition": format!("has(hop.route) && hop.route == 'in_model' && \
                                     (!has(hop.subscriber) || \
                                     hop.subscriber.startsWith('{abs}/'))")})
    };
    let with_edge = |decl: usize, edge: Value| {
        let mut m = manifest.clone();
        m[decl]["diff"]["add_edges"]
            .as_array_mut()
            .expect("edges")
            .push(edge);
        m
    };
    let mut cases: Vec<(&str, Value, &str)> = vec![
        (
            "a door onto a node already in the tree",
            with_edge(
                0,
                level_door("./apps/other", "/os/orgs/o/members/m/apps/other"),
            ),
            "model_push_form",
        ),
        (
            "a door onto the newborn, its condition looser",
            with_edge(
                0,
                json!({"from": ".", "to": "./apps/fig",
                       "condition": "has(hop.route) && hop.route == 'in_model'"}),
            ),
            "model_push_form",
        ),
        (
            "a door onto the newborn addressed under somebody else",
            with_edge(
                0,
                level_door("./apps/fig", "/os/orgs/o/members/m/apps/other"),
            ),
            "model_push_form",
        ),
        (
            "a push one hive deeper to a brain nobody announced",
            with_edge(
                1,
                json!({"from": ".", "to": "./o/members/m/apps/fig",
                       "condition": "has(hop.route) && hop.route == 'in_model' && \
                                     has(hop.subscriber) && hop.subscriber == \
                                     '/os/orgs/o/members/m/apps/fig/simple-agent/strong'",
                       "modifier": {"set_hop": {"route": "'in_model'"}}}),
            ),
            "model_push_form",
        ),
    ];
    // The level's door alone: the container's entry (push + announcement) gone.
    let mut alone = manifest.clone();
    alone.as_array_mut().expect("entries").truncate(1);
    cases.push(("the door with nobody announcing", alone, "model_push_form"));
    // The door drawn in an entry of its own, not the one that bears the app.
    let mut moved = manifest.clone();
    let door = moved[0]["diff"]["add_edges"]
        .as_array()
        .and_then(|e| {
            e.iter().position(|e| {
                e["to"] == "./apps/fig"
                    && e["modifier"].is_null()
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("'in_model'"))
            })
        })
        .expect("the level's door");
    let edge = moved[0]["diff"]["add_edges"]
        .as_array_mut()
        .expect("edges")
        .remove(door);
    moved
        .as_array_mut()
        .expect("entries")
        .push(json!({"scope": "/os/orgs/o/members/m", "diff": {"add_edges": [edge]}}));
    cases.push(("the door outside the birth entry", moved, "model_push_form"));
    // The announcement off a level the app is not born at.
    let mut elsewhere = manifest.clone();
    for e in elsewhere[1]["diff"]["add_edges"]
        .as_array_mut()
        .expect("edges")
        .iter_mut()
    {
        if e["condition"]
            .as_str()
            .is_some_and(|c| c.contains("mutation_committed"))
        {
            e["from"] = json!("./o/members/n");
        }
    }
    cases.push((
        "an announcement off another level",
        elsewhere,
        "model_announcement_form",
    ));
    for (what, decls, code) in cases {
        let out = submit_gate(&decls, SUBMITTERS[1]);
        assert_eq!(gate_refused(&out), code, "{what}: {out:#?}");
        assert!(
            !out.iter().any(|m| m["header"]["route"] == "ask"),
            "{what}: refused without asking the broker: {out:#?}"
        );
    }
    // Only the model road: without it the same manifest is parked and asked.
    let mut bare = manifest.clone();
    for decl in bare.as_array_mut().into_iter().flatten() {
        if let Some(edges) = decl["diff"]["add_edges"].as_array_mut() {
            edges.retain(|e| {
                let text = e.to_string();
                !text.contains("'in_model'") && !text.contains("model_subscribe")
            });
        }
    }
    assert!(gate_parked_and_asked(&submit_gate(&bare, SUBMITTERS[1])));
}

/// The node override params of an `install_app` answer's first entry.
fn app_overrides(answer: &Value) -> Value {
    answer["manifest"][0]["diff"]["add_nodes"][0]["override_params"].clone()
}

/// The subscriber the `in_pack` edge of an `install_app` answer docks for.
fn pack_condition(answer: &Value) -> String {
    answer["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["lane"] == "in_pack")
        .and_then(|e| e["condition"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Review I-1 (GH #1312, OR-AG-81): `install_app` of an app that embeds
/// `simple-agent@2` names the agent ONCE -- the subject of its rim edges
/// (`agent:<id>`), its door's and its enrolment's `agent_id` are one value,
/// by default the app's name, so two such apps of one member are two
/// entities. Before, the edges took the agent's folder (`agent:simple-agent`)
/// and the door and the enrolment kept their default `agent`: the enrolment
/// wrote `entity:agent`, and two apps of a member shared it.
#[test]
fn sa2_install_app_names_the_embedded_agent_once_per_app() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let mut ids = Vec::new();
    for app in ["fig", "fig2"] {
        let mut p = embedding_app_wish();
        p["app"] = json!(app);
        let answer = render_install(&p);
        assert!(answer["header"]["error_code"].is_null(), "{answer}");
        let o = app_overrides(&answer);
        let door = o["simple-agent/door"]["agent_id"].clone();
        assert_eq!(door, json!(app), "{o:#}");
        assert_eq!(o["simple-agent/enrol"]["agent_id"], door, "{o:#}");
        let cond = pack_condition(&answer);
        assert!(
            cond.contains(&format!("hop.subscriber == 'agent:{app}'")),
            "{cond}"
        );
        ids.push(door);
    }
    assert_ne!(ids[0], ids[1], "two apps of one member, two entities");
}

/// Review I-1 (GH #1312): an `agent_id` named in the agent's own overrides is
/// the agent's id for the edges too; two different values are refused by the
/// field, not rendered as two identities.
#[test]
fn sa2_install_app_takes_the_agents_own_id_and_refuses_two() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let mut p = embedding_app_wish();
    p["override_params"]["simple-agent/enrol"] =
        json!({"agent_id": "coordinator", "identity": {"name": "Coordinator"}});
    let answer = render_install(&p);
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let o = app_overrides(&answer);
    assert_eq!(o["simple-agent/door"]["agent_id"], "coordinator", "{o:#}");
    assert_eq!(
        o["simple-agent/enrol"]["identity"]["name"], "Coordinator",
        "{o:#}"
    );
    assert!(
        pack_condition(&answer).contains("'agent:coordinator'"),
        "{answer:#}"
    );
    p["agent_id"] = json!("other");
    let answer = render_install(&p);
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
    let text = answer["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("\"field\": \"agent_id\""), "{text}");
}

/// Review minor (GH #1312, K-3 for `install_app`): the embedded agent's second
/// brain spends a grant only when it is on -- `strong` in the wish or in the
/// agent's door overrides. Before, `install_app` drew its grant, its two
/// credential edges and its `credential_grant_id` always.
#[test]
fn sa2_install_app_grants_the_strong_brain_only_when_it_is_on() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let off = render_install(&embedding_app_wish());
    assert!(off["header"]["error_code"].is_null(), "{off}");
    let text = off["manifest"][0].to_string();
    assert!(
        !text.contains("simple-agent/strong"),
        "{:#}",
        off["manifest"][0]
    );
    assert!(
        text.contains("simple-agent/brain"),
        "the brain still spends"
    );
    let mut p = embedding_app_wish();
    p["strong"] = json!("example-provider/strong-model");
    let on = render_install(&p);
    assert!(on["header"]["error_code"].is_null(), "{on}");
    let o = app_overrides(&on);
    assert!(
        o["simple-agent/strong"]["credential_grant_id"].is_string(),
        "{o:#}"
    );
    assert_eq!(
        o["simple-agent/door"]["strong"],
        "example-provider/strong-model"
    );
}

/// Review I-1 (GH #1312), the block's half of the shared id
/// (`_sa_agent_id`): an `agent_id` the wish names only in the agent's own
/// overrides is the id of its edges and grants as well, and two different
/// values are refused by the field. Before, the block's edges and grants took
/// `name` while the override's id reached the door alone.
#[test]
fn sa2_builder_block_takes_the_agents_own_id_and_refuses_two() {
    if !shipped() {
        return;
    }
    let mut wish = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    if let Some(o) = wish.as_object_mut() {
        o.remove("agent_id");
    }
    wish["override_params"] = json!({"enrol": {"agent_id": "zed"}});
    let answer = render_wish(&wish);
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let overrides = as_map(&answer["manifest"][0]["diff"]["add_nodes"][0]["override_params"]);
    assert_eq!(overrides["door"]["agent_id"], "zed", "{overrides:?}");
    assert!(
        grant_ids(&answer).iter().all(|id| id.contains("/zed/")),
        "{:?}",
        grant_ids(&answer)
    );
    assert!(answer["manifest"].to_string().contains("'agent:zed'"));
    wish["agent_id"] = json!("ava");
    let answer = render_wish(&wish);
    assert_eq!(
        answer["header"]["error_code"], "app_declaration_invalid",
        "{answer}"
    );
    let text = answer["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("\"field\": \"agent_id\""), "{text}");
}

/// The member's own edges out of `./affinity` that a message with `hop` and
/// `context` would take (template `member`).
fn member_affinity_exits(hop: Value, context: Value) -> Vec<String> {
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates/member/config.json"),
        )
        .expect("templates/member/config.json"),
    )
    .expect("member config");
    let (Value::Object(hop), Value::Object(context)) = (hop, context) else {
        panic!("objects");
    };
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("member edges")
        .iter()
        .filter(|e| e["from"] == json!("./affinity"))
        .filter(|e| match e["condition"].as_str() {
            None => true,
            Some(src) => {
                let c = meclaw_colony::cel_eval::parse_condition(src).expect("condition");
                meclaw_colony::cel_eval::evaluate_condition(&c, &context, &hop).unwrap_or(false)
            }
        })
        .map(|e| e["to"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// #1315 (K-6 a, orga-hive O1 lab 0.72.0, trace 01a173d3): the gate's answer to
/// a proposal of an embedded agent left the member twice. The builder's road
/// (`./affinity -> <caller>`, `context.propose_caller`) took it home, and the
/// member's outside reply road (`./affinity -> .` on `ack`/`error`) took a
/// copy up the levels, where no edge carried it on (`hive_no_route` at `/os`,
/// three dead letters). The outside road answers outside callers only: an
/// answer that names an inside caller takes no member exit.
#[test]
fn sa2_an_inside_callers_ack_takes_no_outside_road_of_the_member() {
    for route in ["ack", "error"] {
        let inside = member_affinity_exits(
            json!({"route": route, "outcome": "accepted"}),
            json!({"actor": "agent:a", "propose_caller": "./apps/orga"}),
        );
        assert!(
            inside.is_empty(),
            "an inside caller's {route} leaves the member by {inside:?}"
        );
        let outside = member_affinity_exits(
            json!({"route": route, "outcome": "accepted"}),
            json!({"actor": "operator"}),
        );
        assert_eq!(
            outside,
            vec!["."],
            "an outside caller's {route} still goes home"
        );
    }
}

// ──────────────── consumers beside the agent, the embedder's key (GH #1319)

/// [`deeper_contract`] plus a model cell `./closer` BESIDE the agent that
/// spends a grant too -- orga's coordinator app holds `./closer` next to
/// `./coordinator` (soul2me X2 report, finding B2), a counselor its `judge`.
fn beside_contract() -> Value {
    let mut c = deeper_contract();
    for (side, route) in [("emits", "credential_request"), ("accepts", "in_sealed")] {
        for lane in c[side].as_array_mut().into_iter().flatten() {
            if lane["route"] == route
                && let Some(at) = lane.get_mut("at").and_then(Value::as_array_mut)
            {
                at.push(json!("./closer"));
            }
        }
    }
    c
}

/// The fixture `nest` of [`nest_instantiated`] with that `./closer` beside the
/// agent: an llm cell (the agent's brain's config), so its params are an llm's.
fn beside_instantiated() -> (tempfile::TempDir, std::path::PathBuf) {
    let (td, nest) = nest_instantiated();
    std::fs::create_dir_all(nest.join("closer")).expect("the closer's folder");
    std::fs::copy(
        nest.join("simple-agent/brain/config.json"),
        nest.join("closer/config.json"),
    )
    .expect("the closer's config");
    std::fs::write(
        nest.join("config.json"),
        json!({"cell": {"type": "hive"},
               "params": {"ports": [], "contract": beside_contract()}})
        .to_string(),
    )
    .expect("the nest's config");
    (td, nest)
}

/// An `install_app` wish for the app `nest` grown from the fixture, its rim
/// [`beside_contract`], with the example's credential and an app id.
fn beside_wish() -> Value {
    let mut p = embedding_app_wish();
    p["app"] = json!("nest");
    p["template"] = json!("nest@1.0.0");
    p["contract"] = beside_contract();
    p["app_id"] = json!("example-app nest");
    if let Some(o) = p.as_object_mut() {
        o.remove("override_params");
    }
    p
}

/// GH #1319 (K-10): with `app_id`, `install_app` of an app that embeds
/// `simple-agent@2` names the app on a consumer BESIDE the agent under that
/// consumer's own path (`closer`), never under the agent's prefix
/// (`simple-agent/closer`, a cell the app does not have: the door refused the
/// whole override with `schema`, soul2me X2 lab probe slot 3). A consumer
/// whose app id the wish already names keeps it.
#[test]
fn sa2_install_app_names_the_app_beside_the_agent_on_its_own_path() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let answer = render_install(&beside_wish());
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let overrides = app_overrides(&answer);
    // The door's own check on every override key against the app's tree.
    let (_td, nest) = beside_instantiated();
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&nest, &registry).expect("the nest parses");
    for (cell, params) in as_map(&overrides) {
        let c = parsed
            .cells
            .iter()
            .find(|x| x.rel_path == cell.as_str())
            .unwrap_or_else(|| panic!("the override names no cell of the app: {cell}"));
        check_override_params(c, Some(cell.as_str()), "nest", &params)
            .unwrap_or_else(|why| panic!("{cell}: {why:?}"));
    }
    assert_eq!(overrides["closer"]["x_title"], "example-app nest");
    assert_eq!(overrides["closer"]["http_referer"], "https://meclaw.ai");
    assert_eq!(
        overrides["simple-agent/brain"]["x_title"],
        "example-app nest"
    );

    let mut wish = beside_wish();
    wish["override_params"] = json!({"closer": {"x_title": "orga closer"}});
    let overrides = app_overrides(&render_install(&wish));
    assert_eq!(
        overrides["closer"]["x_title"], "orga closer",
        "the wish's own app id on the cell beside the agent stays: {overrides:#}"
    );
    assert!(
        overrides.get("simple-agent/closer").is_none(),
        "{overrides:#}"
    );
}

/// The `cred_ref` of the grant row whose id ends in `consumer`.
fn grant_ref(answer: &Value, consumer: &str) -> String {
    let diff = &answer["manifest"][0]["diff"];
    diff["seed_rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["table"] == "grants")
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .find(|r| {
            r["grant_id"]
                .as_str()
                .is_some_and(|g| g.ends_with(consumer))
        })
        .and_then(|r| r["cred_ref"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("no grant for {consumer}: {diff:#}"))
}

/// GH #1319 (K-11): `install_app` of an app that embeds `simple-agent@2` seeds
/// the embedder's grant with the credential's `embed_cred_ref` -- the vault
/// name its handle is drawn from -- and every model cell's with `cred_ref`.
/// Before, every row carried `cred_ref`: the embedder was sealed the
/// provider's key under a handle that names the embedding key (soul2me X2
/// report, finding B3). Without `embed_cred_ref` the embedder spends
/// `cred_ref`, as before.
#[test]
fn sa2_install_app_seeds_the_embedders_grant_with_its_own_key() {
    if !shipped() || !repo("templates/figure/config.json").is_file() {
        return;
    }
    let mut wish = embedding_app_wish();
    wish["credential"]["embed_cred_ref"] = json!("cred:openrouter-embed");
    let answer = render_install(&wish);
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    let embed = "/fig-simple-agent-memory-hive-embed";
    assert_eq!(grant_ref(&answer, embed), "cred:openrouter-embed");
    assert_eq!(
        grant_ref(&answer, "/fig-simple-agent-brain"),
        "cred:openrouter"
    );

    let answer = render_install(&embedding_app_wish());
    assert_eq!(grant_ref(&answer, embed), "cred:openrouter");

    // The search cell's row comes from the same mechanism: with
    // `search_cred_ref` it carries that name. Before K-11 it carried the
    // provider's `cred_ref` under a handle drawn from the search key (N5
    // review, M2).
    let search = "/fig-simple-agent-tools-web_search";
    let mut wish = embedding_app_wish();
    wish["credential"]["search_cred_ref"] = json!("cred:brave-search");
    let answer = render_install(&wish);
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    assert_eq!(grant_ref(&answer, search), "cred:brave-search");
    assert_eq!(grant_ref(&answer, embed), "cred:openrouter");

    // Without it the search cell stays anonymous, as before (OR-VG-5).
    let answer = render_install(&embedding_app_wish());
    let has_search_grant = answer["manifest"][0]["diff"]["seed_rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["table"] == "grants")
        .flat_map(|t| t["rows"].as_array().cloned().unwrap_or_default())
        .any(|r| r["grant_id"].as_str().is_some_and(|g| g.ends_with(search)));
    assert!(!has_search_grant, "{answer:#}");
}

/// K-15 (GH #1321): a turn narrows the loop cap of ITSELF through the turn
/// door (`hop.max_iter`); the collector's `max_iter` stays the default and the
/// upper bound. The stub calls a tool on every brain call, so only the cap ends
/// a turn: the narrowed turn ends after its own number of brain calls, the next
/// turn of the same session runs to the param again (the knob rides in the
/// round's own row, not in the session), a larger or unreadable value is the
/// param, and so is a fraction (`2.7` is no whole number: review M-1, it used
/// to be cut to 2). `max_iter` N counts tool re-entries, as the param does: N + 1 brain
/// calls. Events only: the `iters` of each answer and the brain calls on the
/// wire after quiescence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_turn_narrows_its_own_iteration_cap() {
    if !shipped() {
        return;
    }
    let calls = (1..=23)
        .map(|i| {
            canned_tool_calls(vec![(
                format!("c-{i}").as_str(),
                "weather",
                r#"{"place":"Berlin"}"#,
            )])
        })
        .collect();
    let (_td, stubs, h, mut ports) = boot(
        calls,
        json!({"collector/assemble": {"max_iter": 4}, "tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    let mut seen = Vec::new();
    let mut got = Vec::new();
    for (tid, knob) in [
        ("t-1", json!({"max_iter": "2"})),
        ("t-2", json!({})),
        ("t-3", json!({"max_iter": "9"})),
        ("t-4", json!({"max_iter": "many"})),
        ("t-5", json!({"max_iter": 2.7})),
    ] {
        h.send(turn_with(tid, "Loop.", knob)).await;
        let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
        let slot = body_of(&answer)["answer"].clone();
        got.push((tid, slot["iters"].clone(), slot["partial"].clone()));
    }
    assert_eq!(
        got,
        vec![
            ("t-1", json!(3), json!(true)),
            ("t-2", json!(5), json!(true)),
            ("t-3", json!(5), json!(true)),
            ("t-4", json!(5), json!(true)),
            ("t-5", json!(5), json!(true)),
        ],
        "the narrowed turn ends at its own cap, every other at the param"
    );
    h.quiesce().await;
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        23,
        "no brain call past a cap"
    );
}

/// K-15 (GH #1321, review M-2): the required-tools follow-up of a narrowed
/// turn runs under the SAME narrowed cap, and the partial answer names that
/// cap. The stub calls `weather` on every brain call, the turn requires
/// `clock`: the turn (cap 1) spends 2 brain calls, the follow-up the door opens
/// for the missing `clock` 2 more; under the param (4) the follow-up alone
/// would spend 5. Events only: the answer and the brain calls after quiescence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_narrowed_turns_follow_up_keeps_its_cap() {
    if !shipped() {
        return;
    }
    let calls = (1..=10)
        .map(|i| {
            canned_tool_calls(vec![(
                format!("c-{i}").as_str(),
                "weather",
                r#"{"place":"Berlin"}"#,
            )])
        })
        .collect();
    let (_td, stubs, h, mut ports) = boot(
        calls,
        json!({"collector/assemble": {"max_iter": 4}, "tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_with(
        "t-1",
        "Loop.",
        json!({"max_iter": "1", "required_tools": "[\"clock\"]"}),
    ))
    .await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["partial"], json!(true), "{slot:#}");
    assert_eq!(slot["missing_required"], json!(["clock"]), "{slot:#}");
    let text = slot["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("(max_iter=1)"),
        "the partial answer names the narrowed cap: {text}"
    );
    h.quiesce().await;
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        4,
        "turn and follow-up each stop at the narrowed cap"
    );
}
