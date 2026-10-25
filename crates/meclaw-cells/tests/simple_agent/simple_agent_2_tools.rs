//! `simple-agent@2` -- own tools by enablement, conversations at the rim
//! (R-SA-17 (3), R-SA-18, plan A4).
//!
//! `tool_gate.tools_enabled` names the tool FAMILIES this agent offers (`web`
//! is web_fetch and web_search), default none: a family nobody enabled is not
//! on the menu, a call of one never reaches `./tools` -- it is not the agent's
//! own, so it goes to the door like every name the agent does not serve, and a
//! client tool of the turn by that name (`bash` of the caller) leaves on the
//! rim -- and its code cells ship `cold`. The gate tells the door what is
//! enabled (lane `tools_enabled`). The agent's core tools (memory_recall,
//! history_*) are always offered, and a client tool of the caller passes
//! whatever is enabled. The rim carries `in_conversations` to the agent's own
//! memory hive and its answer back out on `conversations`.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
#[allow(unused_imports)]
use crate::simple_agent_loop; // harness_fold: mod simple_agent_loop;
use meclaw_core::serde_json::{Value, json};
use mock_openai::{OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use simple_agent_loop::{boot, copy_resolved, next_on, read_json, repo, shipped, turn};

/// The family tools as the menu shows them: `web` is the op tool the
/// dispatcher unfolds into web_fetch/web_search (GH #1245).
const FAMILY_TOOLS: [&str; 7] = [
    "bash",
    "file",
    "edit",
    "web",
    "web_fetch",
    "web_search",
    "weather",
];

fn names(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect()
}

fn look() -> Value {
    json!({"name": "look", "description": "Look at something.",
           "parameters": {"type": "object", "properties": {"at": {"type": "string"}}}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_no_family_is_offered_unless_enabled() {
    if !shipped() {
        return;
    }
    // Nothing enabled: no family tool on the menu, the core tools are.
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Hi.", "stop")], json!({})).await;
    h.send(turn("t-1", "Hello.", Value::Null)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    let offered = names(&stubs.brain.recorded_requests().await[0]);
    for t in FAMILY_TOOLS {
        assert!(!offered.iter().any(|n| n == t), "{t} offered: {offered:?}");
    }
    assert!(offered.iter().any(|n| n == "memory_recall"), "{offered:?}");
    h.shutdown().await;

    // `web` enabled: exactly its two tools join the menu.
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hi.", "stop")],
        json!({"tool_gate": {"tools_enabled": ["web"]}}),
    )
    .await;
    h.send(turn("t-1", "Hello.", Value::Null)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    let offered = names(&stubs.brain.recorded_requests().await[0]);
    for t in FAMILY_TOOLS {
        if t.starts_with("web_") {
            continue;
        }
        assert_eq!(
            offered.iter().any(|n| n == t),
            t == "web",
            "{t}: {offered:?}"
        );
    }
    h.shutdown().await;
}

fn bash_schema() -> Value {
    json!({"name": "bash", "description": "Run a command on the caller's machine.",
           "parameters": {"type": "object", "properties": {"command": {"type": "string"}}}})
}

/// Lock R-SA-18 (ledger OR-AG-53/54): a tool family nobody enabled is
/// neither in the model's menu nor executable. The caller brings its own
/// `bash` and the agent's bash family is off: the menu offers exactly one
/// `bash`, the caller's (its description), and no other family tool; the
/// model's `bash` call is the caller's, it leaves on the rim as `calls`, and
/// nothing of the turn reaches the agent's own `./tools` -- the end-to-end
/// road gate -> door -> menu (A4 review, minor 3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_family_not_enabled_is_neither_in_the_menu_nor_executable() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-bash", "bash", r#"{"command":"ls"}"#)]),
            canned_chat_completion("Done.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "List it.", json!([bash_schema()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    let first = &stubs.brain.recorded_requests().await[0];
    let offered = names(first);
    assert_eq!(
        offered.iter().filter(|n| *n == "bash").count(),
        1,
        "one bash on the menu: {offered:?}"
    );
    assert_eq!(
        simple_agent_loop::tool_description(first, "bash"),
        "Run a command on the caller's machine.",
        "the bash on the menu is the caller's"
    );
    for t in FAMILY_TOOLS.iter().filter(|t| **t != "bash") {
        assert!(!offered.iter().any(|n| n == t), "{t} offered: {offered:?}");
    }
    let slot = simple_agent_loop::body_of(&calls)["calls"].clone();
    assert_eq!(slot["name"], "bash", "{slot}");
    assert_eq!(slot["arguments"]["command"], "ls", "{slot}");
    h.quiesce().await;
    assert_eq!(
        simple_agent_loop::count_route(&h, "/agent", "/agent/tools", "tool_call").await,
        0,
        "a call of a family nobody enabled never reaches ./tools"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_the_gate_tells_the_door_what_is_enabled() {
    if !shipped() {
        return;
    }
    // The menu tick passes the gate; the gate answers the door on lane
    // `tools_enabled` with the enabled families, so the door can keep a
    // client tool whose own family is off.
    let (_td, _stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hi.", "stop")],
        json!({"tool_gate": {"tools_enabled": ["web"]}}),
    )
    .await;
    h.send(turn("t-1", "Hello.", Value::Null)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    simple_agent_loop::wait_route(&h, "/agent/tool_gate", "/agent/door", "tools_enabled", 1).await;
    let log = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    let row = log
        .iter()
        .find(|r| {
            r.from_path.starts_with("/agent/tool_gate")
                && r.to_path.starts_with("/agent/door")
                && meclaw_core::serde_json::from_str::<Value>(&r.headers_json)
                    .is_ok_and(|hd| hd["hop"]["route"] == "tools_enabled")
        })
        .expect("a tools_enabled message to the door");
    let hd: Value = meclaw_core::serde_json::from_str(&row.headers_json).expect("headers");
    assert_eq!(hd["hop"]["tools_enabled"], "[\"web\"]", "{hd}");
    h.shutdown().await;
}

#[test]
fn sa2_unenabled_family_cells_are_cold() {
    if !shipped() {
        return;
    }
    let gate = read_json(&repo("templates/simple-agent/tool_gate/config.json"));
    assert_eq!(
        gate["params"]["tools_enabled"],
        json!([]),
        "the default enables none"
    );
    let td = tempfile::TempDir::new().expect("tempdir");
    let agent = td.path().join("agent");
    copy_resolved(&repo("templates/simple-agent"), &agent, 0);
    // The code cells of the families the default leaves off (weather) and of
    // the family this agent never offers (the build pair) run cold.
    for cell in ["weather", "build-draft", "build-apply"] {
        let cfg = read_json(&agent.join("tools").join(cell).join("config.json"));
        assert_eq!(cfg["params"]["runner_mode"], "cold", "tools/{cell}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_client_tools_pass_whatever_is_enabled() {
    if !shipped() {
        return;
    }
    for overlay in [
        json!({}),
        json!({"tool_gate": {"tools_enabled": ["bash", "web"]}}),
    ] {
        let (_td, stubs, h, mut ports) =
            boot(vec![canned_chat_completion("Hi.", "stop")], overlay.clone()).await;
        h.send(turn("t-1", "Hello.", json!([look()]))).await;
        let mut seen = Vec::new();
        next_on(&h, &mut ports, "answer", &mut seen).await;
        let offered = names(&stubs.brain.recorded_requests().await[0]);
        assert!(
            offered.iter().any(|n| n == "look"),
            "{overlay}: {offered:?}"
        );
        h.shutdown().await;
    }
}

/// A run of the rim's `in_conversations` (A4 review, minor 4): after one
/// turn, a reader outside the round asks the agent at its rim for the
/// session; the request reaches the agent's own memory hive (its `./sessions`
/// reads it) and the answer leaves the rim on `conversations` with the reader
/// on `hop.reader` and the turn's words in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_the_rim_carries_conversations_to_its_memory_in_a_run() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Hello.", "stop")], json!({})).await;
    let mut seen = Vec::new();
    h.send(turn("t-1", "Hi there.", Value::Null)).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    simple_agent_loop::wait_route(&h, "/agent", "/agent/memory-hive", "in_episode", 1).await;
    h.quiesce().await;
    h.send(
        meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/agent"))
            .hop(simple_agent_loop::as_map(
                &json!({"route": "in_conversations"}),
            ))
            .context(meclaw_core::serde_json::Map::new())
            .body(meclaw_core::Body::Inline(json!({
                "messages": [], "reader": "counselor",
                "audience_set": simple_agent_loop::AUDIENCE, "session_id": "s1"})))
            .ttl(400)
            .build(),
    )
    .await;
    let answer = next_on(&h, &mut ports, "conversations", &mut seen).await;
    assert!(
        simple_agent_loop::count_route(&h, "", "/agent/memory-hive/sessions", "in_conversations")
            .await
            >= 1,
        "the request reached the agent's own memory"
    );
    assert_eq!(answer.headers.hop["reader"], "counselor");
    assert!(
        answer.headers.hop.get("error_code").is_none(),
        "{:?}",
        answer.headers.hop
    );
    let text = simple_agent_loop::body_of(&answer)["messages"][0]["text"]
        .as_str()
        .expect("the answer text")
        .to_string();
    let a: Value = meclaw_core::serde_json::from_str(&text).expect("answer json");
    assert_eq!(a["sessions"][0]["session_id"], "s1", "{a}");
    assert!(
        a["sessions"][0]["episodes"].as_array().is_some_and(|e| e
            .iter()
            .any(|e| e["role"] == "person" && e["text"] == "Hi there.")),
        "the turn's words came back: {a}"
    );
    h.shutdown().await;
}

#[test]
fn sa2_the_rim_carries_conversations_to_its_memory() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo("templates/simple-agent/config.json"));
    let lanes = |side: &str| -> Vec<String> {
        cfg["params"]["contract"][side]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|l| l["route"].as_str().map(str::to_string))
            .collect()
    };
    assert!(lanes("accepts").contains(&"in_conversations".to_string()));
    assert!(lanes("emits").contains(&"conversations".to_string()));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let has = |from: &str, to: &str, route: &str| {
        edges.iter().any(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{route}'")))
        })
    };
    assert!(has(".", "./memory-hive", "in_conversations"));
    assert!(has("./memory-hive", ".", "conversations"));
    let drains = cfg["params"]["required_drains"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        drains
            .iter()
            .any(|d| d["accepts"] == "in_conversations" && d["emits"] == "conversations")
    );
}
