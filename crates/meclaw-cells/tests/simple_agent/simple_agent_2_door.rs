//! `simple-agent@2` -- the door and the second brain (plan A2, GH #1308).
//!
//! What this file pins: turns of several callers queue (priority, then
//! arrival) instead of a `turn_busy` refusal; a turn past its deadline ends
//! `expired` without a model call; the queue, the answered turn ids and the
//! start-memory mark survive a restart in `./door_store`; the answer carries
//! iterations, usage, cost and stage; a required tool the answer lacks gets
//! exactly one follow-up turn; an unanswered client call times out named; a
//! note reaches the window; `stage: strong` runs the `strong` cell when the
//! agent has one and says `fast` when it has none, on one window.
//!
//! The colony is grown by `simple_agent_loop::boot` (Form::V2). Real time with
//! hang marks only (AGENTS rule 11); the client timeout and the deadline are
//! SUT configuration, never a verdict.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
#[allow(unused_imports)]
use crate::simple_agent_loop; // harness_fold: mod simple_agent_loop;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{canned_chat_completion, canned_tool_calls};
use simple_agent_loop::{
    AUDIENCE, Ports, ROUND_KEYS, STRONG_MODEL, as_map, body_of, boot, count_route, look_tool,
    next_on, reboot, result_for, shipped, tool_names, wire_text,
};

/// A turn with the rim keys of A2 on its hop (caller, priority, deadline,
/// required tools, stage) and the client tools in its body.
fn turn_on(turn_id: &str, text: &str, tools: Value, hop: Value) -> Message {
    let mut head = json!({"route": "turn", "turn_id": turn_id, "session_id": "s1",
                          "channel": "c1", "audience_set": AUDIENCE});
    for (k, v) in as_map(&hop) {
        head[k] = v;
    }
    let mut body = json!({"messages": [{"origin": "user", "type": "text", "text": text}]});
    if !tools.is_null() {
        body["tools"] = tools;
    }
    MessageBuilder::new(Path::new("/agent"))
        .hop(as_map(&head))
        .context(Map::new())
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

fn answer_of(m: &Message) -> Value {
    body_of(m)["answer"].clone()
}

/// The next `n` answers, in the order they leave the rim.
async fn answers(
    h: &meclaw_testing::ColonyHandle,
    ports: &mut Ports,
    n: usize,
    seen: &mut Vec<Message>,
) -> Vec<Message> {
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(next_on(h, ports, "answer", seen).await);
    }
    out
}

/// Every rim message the run has left in the sink so far (after a quiesce).
fn drained(ports: &mut Ports, seen: &mut Vec<Message>) -> Vec<Message> {
    let mut all = std::mem::take(seen);
    while let Ok(m) = ports.sink.try_recv() {
        all.push(m);
    }
    all
}

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// A Chat-Completions answer with the usage figures a provider reports.
fn answer_with_usage(text: &str) -> MockResponse {
    MockResponse::ok_json(
        json!({"id": "chatcmpl-usage", "model": "brain-stub",
               "choices": [{"message": {"role": "assistant", "content": text},
                            "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 100, "completion_tokens": 20, "cost": 0.25,
                         "prompt_tokens_details": {"cached_tokens": 40,
                                                   "cache_write_tokens": 7}}})
        .to_string()
        .as_bytes(),
    )
}

fn look_call() -> MockResponse {
    canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)])
}

// ═══════════════════════════════════════════════════════ the queue

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_turns_queue_by_priority_then_arrival() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            look_call(),
            canned_chat_completion("First.", "stop"),
            canned_chat_completion("Urgent.", "stop"),
            canned_chat_completion("Early.", "stop"),
            canned_chat_completion("Late.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Look.", json!([look_tool()]), json!({})))
        .await;
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    // While t-1 waits on its client call: two of the default priority, then
    // an urgent one.
    h.send(turn_on("t-2", "Early.", Value::Null, json!({})))
        .await;
    h.send(turn_on(
        "t-3",
        "Late.",
        Value::Null,
        json!({"priority": "5"}),
    ))
    .await;
    h.send(turn_on(
        "t-4",
        "Urgent.",
        Value::Null,
        json!({"priority": "0"}),
    ))
    .await;
    h.quiesce().await;
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let order: Vec<String> = answers(&h, &mut ports, 4, &mut seen)
        .await
        .iter()
        .map(|a| answer_of(a)["turn_id"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        order,
        ["t-1", "t-4", "t-2", "t-3"],
        "priority, then arrival"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_second_caller_waits_instead_of_turn_busy() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            look_call(),
            canned_chat_completion("I see a red door.", "stop"),
            canned_chat_completion("It smells of pine.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on(
        "t-1",
        "What is there?",
        json!([look_tool()]),
        json!({"caller": "caller:a"}),
    ))
    .await;
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    assert_eq!(
        hop_str(&calls, "caller"),
        "caller:a",
        "the call finds its caller"
    );
    let other = json!({"name": "smell", "description": "Smell something.",
                       "parameters": {"type": "object", "properties": {}}});
    h.send(turn_on(
        "t-2",
        "And now?",
        json!([other]),
        json!({"caller": "caller:b"}),
    ))
    .await;
    h.quiesce().await;
    let early = drained(&mut ports, &mut seen);
    assert!(
        !early
            .iter()
            .any(|m| simple_agent_loop::route_of(m) == "error"),
        "no refusal while the first turn runs: {:?}",
        early.iter().map(body_of).collect::<Vec<_>>()
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 1, "t-2 waits");
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let done = answers(&h, &mut ports, 2, &mut seen).await;
    assert_eq!(answer_of(&done[0])["turn_id"], "t-1");
    assert_eq!(answer_of(&done[0])["text"], "I see a red door.");
    assert_eq!(hop_str(&done[0], "caller"), "caller:a");
    assert_eq!(answer_of(&done[1])["turn_id"], "t-2");
    assert_eq!(answer_of(&done[1])["text"], "It smells of pine.");
    assert_eq!(hop_str(&done[1], "caller"), "caller:b");
    let reqs = stubs.brain.recorded_requests().await;
    let names = tool_names(&reqs[1]);
    assert!(
        names.contains(&"look".to_string()) && !names.contains(&"smell".to_string()),
        "the open turn's menu row stays until it ends: {names:?}"
    );
    assert!(
        tool_names(&reqs[2]).contains(&"smell".to_string()),
        "the waiting turn brings its own tools"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_deadline_passed_ends_expired_without_a_model_call() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Never.", "stop")], json!({})).await;
    let mut seen = Vec::new();
    // Epoch millisecond 1: a deadline long gone when the turn could begin.
    h.send(turn_on(
        "t-1",
        "Hello.",
        Value::Null,
        json!({"deadline_ms": "1"}),
    ))
    .await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["turn_id"], "t-1", "{a}");
    assert_eq!(a["expired"], true, "{a}");
    assert_eq!(a["iters"], 0, "{a}");
    h.quiesce().await;
    assert!(
        stubs.brain.recorded_requests().await.is_empty(),
        "an expired turn calls no model"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_queued_turn_expires_behind_a_hanging_turn() {
    if !shipped() {
        return;
    }
    // Review I2 (OR-AG-50): t-1 waits on a client call nobody answers; t-2
    // behind it has a deadline. The door's one-shot `deadline` watchdog ends
    // t-2 as expired while t-1 still hangs -- the answer itself is the marker.
    let (_td, stubs, h, mut ports) = boot(vec![look_call()], json!({})).await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Look.", json!([look_tool()]), json!({})))
        .await;
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    // SUT configuration, never a verdict: a deadline one to two seconds after
    // the door's own stamp on its call (the colony's clock, whole seconds).
    let deadline = (calls.created_at + 2) * 1000;
    h.send(turn_on(
        "t-2",
        "Then this.",
        Value::Null,
        json!({"deadline_ms": deadline.to_string()}),
    ))
    .await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["turn_id"], "t-2", "the waiting turn ends first: {a}");
    assert_eq!(a["expired"], true, "{a}");
    assert_eq!(a["iters"], 0, "{a}");
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        1,
        "only t-1 called a model"
    );
}

// ═══════════════════════════════════════════════════════ the durable door

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_respawned_door_keeps_its_queue_and_refuses_a_seen_turn_id() {
    if !shipped() {
        return;
    }
    let (td, _stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("Zero.", "stop"),
            look_call(),
            canned_chat_completion("One.", "stop"),
            canned_chat_completion("Two.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    // Review M3: a turn answered BEFORE the restart -- its id must survive it.
    h.send(turn_on("t-0", "Hello.", Value::Null, json!({})))
        .await;
    let zero = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(zero["turn_id"], "t-0", "{zero}");
    h.send(turn_on("t-1", "Look.", json!([look_tool()]), json!({})))
        .await;
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    h.send(turn_on("t-2", "Then this.", Value::Null, json!({})))
        .await;
    h.quiesce().await;
    h.shutdown().await;
    // A new colony on the same tree: every child starts a new life.
    let (h, mut ports) = reboot(&td).await;
    let mut seen = Vec::new();
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let done = answers(&h, &mut ports, 2, &mut seen).await;
    assert_eq!(
        answer_of(&done[0])["turn_id"],
        "t-1",
        "the open turn goes on"
    );
    assert_eq!(
        answer_of(&done[1])["turn_id"],
        "t-2",
        "the queued turn was kept"
    );
    h.send(turn_on("t-1", "Again.", Value::Null, json!({})))
        .await;
    let err = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&err)["error"].clone();
    assert_eq!(slot["error_code"], "turn_seen", "{slot}");
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    h.send(turn_on("t-0", "Again.", Value::Null, json!({})))
        .await;
    let err = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&err)["error"].clone();
    assert_eq!(
        slot["error_code"], "turn_seen",
        "answered before the restart: {slot}"
    );
    assert_eq!(slot["turn_id"], "t-0", "{slot}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_start_memories_are_written_once_across_respawn() {
    if !shipped() {
        return;
    }
    let (td, _stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("One.", "stop"),
            canned_chat_completion("Two.", "stop"),
        ],
        json!({"door": {"start_memories": ["We met at the lake."]}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Hello.", Value::Null, json!({})))
        .await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let seeds = |rows: &[meclaw_colony::api_dto::MessageLogDto]| {
        rows.iter()
            .filter(|r| {
                r.from_path.starts_with("/agent/door")
                    && r.to_path.starts_with("/agent/memory-hive")
            })
            .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
            .filter(|h| h["hop"]["route"] == "in_episode" && h["hop"]["session_id"] == "seed")
            .count()
    };
    let all = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    assert_eq!(seeds(&all), 1, "the start memory, once");
    h.shutdown().await;
    // The whole log, not a mark after the reboot: `./tool_gate`'s menu tick can wake the
    // new door child (and its boot) before any mark is taken.
    let (h, mut ports) = reboot(&td).await;
    let mut seen = Vec::new();
    h.send(turn_on("t-2", "Hello again.", Value::Null, json!({})))
        .await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let all = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    let packs = all
        .iter()
        .filter(|r| {
            r.from_path.starts_with("/agent/door") && r.to_path.starts_with("/agent/curator")
        })
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_pack")
        .count();
    assert!(
        packs >= 2,
        "both door child lives booted (a frame pack each): {packs}"
    );
    assert_eq!(
        seeds(&all),
        1,
        "a new child life writes no start memory again"
    );
}

// ═══════════════════════════════════════════════════════ the answer

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_answer_carries_usage_cost_iters_and_stage() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![look_call(), answer_with_usage("A red door.")],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Look.", json!([look_tool()]), json!({})))
        .await;
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["iters"], 2, "{a}");
    // 10 + 100 in, 5 + 20 out (the stub's tool call reports 10/5).
    assert_eq!(
        a["usage"],
        json!({"in": 110, "cached_in": 40, "cache_write": 7, "out": 25}),
        "{a}"
    );
    assert_eq!(a["cost"], 0.25, "{a}");
    assert_eq!(a["stage"], "fast", "{a}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_missing_required_tool_gets_one_follow_up_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("Done without looking.", "stop"),
            canned_chat_completion("Still not looking.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on(
        "t-1",
        "Look at the door.",
        json!([look_tool()]),
        json!({"required_tools": "[\"look\"]"}),
    ))
    .await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["turn_id"], "t-1", "{a}");
    assert_eq!(a["missing_required"], json!(["look"]), "{a}");
    assert_eq!(a["text"], "Still not looking.", "{a}");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "exactly one follow-up");
    assert!(
        wire_text(&reqs[1]).contains("Call look now."),
        "{}",
        wire_text(&reqs[1])
    );
    assert!(
        tool_names(&reqs[1]).contains(&"look".to_string()),
        "the follow-up keeps the tool"
    );
    assert_eq!(
        count_route(&h, "/agent", "/sink", "answer").await,
        1,
        "one answer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_unanswered_client_tool_times_out_named() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            look_call(),
            canned_chat_completion("Nobody answered.", "stop"),
        ],
        // SUT configuration: the client's deadline, short for the run.
        json!({"door": {"client_tool_timeout_ms": 300}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Look.", json!([look_tool()]), json!({})))
        .await;
    next_on(&h, &mut ports, "calls", &mut seen).await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["text"], "Nobody answered.", "{a}");
    let reqs = stubs.brain.recorded_requests().await;
    let wire = wire_text(&reqs[1]);
    assert!(
        wire.contains("error: timeout") && wire.contains("look"),
        "the result names the timeout and the tool: {wire}"
    );
}

// ═══════════════════════════════════════════════════════ notes

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_in_note_reaches_the_window_before_the_newest_turn() {
    if !shipped() {
        return;
    }
    let cfg = simple_agent_loop::read_json(&simple_agent_loop::repo(
        "templates/simple-agent/config.json",
    ));
    let lane = cfg["params"]["contract"]["accepts"]
        .as_array()
        .and_then(|l| l.iter().find(|x| x["route"] == "in_note"))
        .cloned()
        .expect("the rim takes `in_note`");
    assert_eq!(lane["at"], json!(["./curator"]), "{lane}");
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Slowly, then.", "stop")],
        json!({}),
    )
    .await;
    let note = MessageBuilder::new(Path::new("/agent/curator"))
        .hop(as_map(&json!({"route": "in_note"})))
        .context(as_map(
            &json!({"audience_set": AUDIENCE, "session_id": "s1"}),
        ))
        .body(Body::Inline(json!({"messages": [], "notes": [
            {"id": "n-1", "source": "care", "text": "Speak slowly.", "scope": "session"}]})))
        .ttl(400)
        .build();
    h.send(note).await;
    h.quiesce().await;
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Hello there.", Value::Null, json!({})))
        .await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    let reqs = stubs.brain.recorded_requests().await;
    let msgs = reqs[0].messages().cloned().unwrap_or_default();
    let last = msgs
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .map(|m| m["content"].to_string())
        .unwrap_or_default();
    let (n, t) = (last.find("Speak slowly."), last.find("Hello there."));
    assert!(
        n.is_some() && t.is_some() && n < t,
        "the note opens the newest turn: {last}"
    );
}

// ═══════════════════════════════════════════════════════ the second brain

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_strong_stage_calls_the_strong_cell() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Deeply.", "stop")],
        json!({"door": {"strong": STRONG_MODEL}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn_on(
        "t-1",
        "Think.",
        Value::Null,
        json!({"stage": "strong"}),
    ))
    .await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["stage"], "strong", "{a}");
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs[0].model(), Some(STRONG_MODEL), "the strong cell asked");
    assert_eq!(
        count_route(&h, "/agent/strong", "/agent/door", "usage").await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_strong_unset_runs_fast_and_says_so() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Quickly.", "stop")], json!({})).await;
    let mut seen = Vec::new();
    h.send(turn_on(
        "t-1",
        "Think.",
        Value::Null,
        json!({"stage": "strong"}),
    ))
    .await;
    let a = answer_of(&next_on(&h, &mut ports, "answer", &mut seen).await);
    assert_eq!(a["stage"], "fast", "{a}");
    let reqs = stubs.brain.recorded_requests().await;
    assert_ne!(reqs[0].model(), Some(STRONG_MODEL), "the brain answered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_window_is_sized_by_the_answering_models_package() {
    if !shipped() {
        return;
    }
    let replies = (0..7)
        .map(|i| canned_chat_completion(&format!("Reply {i}."), "stop"))
        .collect();
    let (_td, stubs, h, mut ports) = boot(replies, json!({"door": {"strong": STRONG_MODEL}})).await;
    let mut seen = Vec::new();
    // Five fast turns, a strong one, a fast one: one session, one window.
    for (i, stage) in ["fast", "fast", "fast", "fast", "fast", "strong", "fast"]
        .iter()
        .enumerate()
    {
        h.send(turn_on(
            &format!("t-{i}"),
            &format!("Turn {i}."),
            Value::Null,
            json!({"stage": stage}),
        ))
        .await;
        next_on(&h, &mut ports, "answer", &mut seen).await;
    }
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 7);
    assert_eq!(reqs[5].model(), Some(STRONG_MODEL));
    // The curator reads each answering model's package from its own answer
    // (`in_llm` from both cells); a stage switch is no rebuild: the window's
    // head stands, and the history only grows.
    assert_eq!(
        count_route(&h, "/agent/strong", "/agent/curator", "in_llm").await,
        1
    );
    let sys = |i: usize| simple_agent_loop::system_text(&reqs[i]);
    assert_eq!(sys(4), sys(5), "fast -> strong keeps the head");
    assert_eq!(sys(5), sys(6), "strong -> fast keeps the head");
    let len = |i: usize| reqs[i].messages().map_or(0, Vec::len);
    assert!(
        len(4) < len(5) && len(5) < len(6),
        "the history grows across the switch"
    );
}

// ═══════════════════════════════════════════════════════ client tools

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_a_client_tool_passes_when_its_family_is_not_enabled() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Fine.", "stop")], json!({})).await;
    // No family enabled (the default of A4's `tools_enabled`): the caller's
    // `bash` is the caller's, the door drops nothing.
    let client = json!({"name": "bash", "description": "CLIENT BASH",
                        "parameters": {"type": "object", "properties": {}}});
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Hi.", json!([client]), json!({})))
        .await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let in_turn: Vec<Value> = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_turn")
        .collect();
    assert!(!in_turn.is_empty());
    assert!(
        in_turn
            .iter()
            .all(|h| h["hop"].get("client_tools_dropped").is_none()),
        "nothing dropped: {in_turn:?}"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        simple_agent_loop::tool_description(&reqs[0], "bash").contains("CLIENT"),
        "the caller's bash stands in the menu: {:?}",
        tool_names(&reqs[0])
    );
}
