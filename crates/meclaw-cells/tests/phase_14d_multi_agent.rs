//! Phase-14-D Multi-Agent-Dispatch-Topologie — TDD-Tests.
//! Task 1: Router-Klassifikator isoliert (deterministisch).
//!   `router_classifies_question_to_agent_target` — keyword-overlap classify,
//!   emits hop.agent_target ∈ {billing,tech}, passes body.messages through.
//! Task 2: agent_target discriminates exactly one agent.
//!   `agent_target_routes_to_exactly_one_agent` — full topology + two MockOpenAI
//!   instances; billing question → mock_billing count=1, mock_tech count=0;
//!   tech question → reversed.
//! Task 3: hop.agent_target decays / context.turn_id couples (the core slice).
//!   `agent_target_decays_turn_id_couples` — at /sink: context.turn_id=="t1"
//!   present (coupling across 2 hops), hop.agent_target absent (decay),
//!   hop.exit_code == 0 (non-hollow positive signal from capture code cell).
//! Task 4: finale Antwort gekoppelt, end-to-end + Receipt.
//!   `final_answer_coupled_to_turn` — capture Receipt carries "BILLING_ANSWER" +
//!   context.turn_id=="t1"; billing mock recorded request equals the user turn
//!   text; tech mock receives 0 requests; DLQ empty.
//! Task 5: the mutation validator confirms build-time-clean (+ a negative case).
//!   `validator_accepts_multi_agent_topology` — plan_bootstrap on the committed
//!   tree returns Ok (turn_id reachable via ingress root, no hop locality violation).
//!   `validator_rejects_unreachable_turn_id` — after injecting delete_context:
//!   [turn_id] on router→billing, plan_bootstrap returns Err with
//!   HeaderContractViolation mentioning "turn_id" and "context presence not reachable".
//! - Task 6: live-graph DOT/SVG render + env-gated artifact emit (topology_svg_from_live_graph)

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{
    BootstrapError, CellFactory, CellFactoryRegistry, RegistryOverlay, bootstrap_from_filesystem,
    plan_bootstrap,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copies the router sub-tree only (`main/router/config.json`) into `dst`.
fn copy_router_tree(dst: &std::path::Path) {
    // dst layout: main/router/config.json
    let src_router = support::example_dir("14d-multi-agent").join("main/router");
    let dst_router = dst.join("main/router");
    std::fs::create_dir_all(&dst_router).unwrap();
    std::fs::copy(
        src_router.join("config.json"),
        dst_router.join("config.json"),
    )
    .unwrap();

    // Minimal hive config so bootstrap_from_filesystem can boot /main.
    let main_dir = dst.join("main");
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./router","to":"/sink"}]}}}"#,
    )
    .unwrap();
}

/// Copies the full 14d-multi-agent example tree into `dst`, replaces the
/// `params.base_url` in the billing and tech `llm` configs with the live mock
/// addresses, and adds a `capture→/sink` edge to `main/config.json` so the
/// test can tap the settle signal via the `/sink` CaptureCell.
///
/// Mirrors `copy_14c_tree` + `patch_llm_base_url_14c` from `phase_14c_lexical_rag.rs`,
/// generalised to two separate llm configs and the 14-D tree shape.
///
/// `billing_base_url` and `tech_base_url` are full `http://host:port/v1` strings
/// (i.e. already include the `/v1` path prefix) — the function writes them
/// verbatim into `params.base_url`, replacing the placeholder entirely.
fn copy_14d_tree_patch(dst: &std::path::Path, billing_base_url: &str, tech_base_url: &str) {
    // Copy the tree (no SVG/DOT artefacts).
    support::copy_dir_recursive(&support::example_dir("14d-multi-agent"), dst);

    // Patch billing/config.json: set base_url to the billing mock address.
    patch_llm_base_url_in_config(&dst.join("main/billing/config.json"), billing_base_url);

    // Patch tech/config.json: set base_url to the tech mock address.
    patch_llm_base_url_in_config(&dst.join("main/tech/config.json"), tech_base_url);

    // Patch main/config.json: add capture→/sink so the test receives the
    // settle signal.  The committed config ends at billing/tech→capture;
    // we extend it here exactly as 14-C-4 does for llm→capture.
    let main_cfg_path = dst.join("main/config.json");
    let txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut cfg: Value = meclaw_core::serde_json::from_str(&txt).unwrap();
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("main/config.json must have graph.edges array");
    edges.push(json!({"from": "./capture", "to": "/sink"}));
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

/// Reads `path` as a JSON `llm` config, overwrites `params.base_url` with
/// `base_url` verbatim, and writes the file back.
///
/// Mirrors `patch_llm_base_url_14c` from `phase_14c_lexical_rag.rs` but
/// targets a single named file rather than scanning a tree.
fn patch_llm_base_url_in_config(path: &std::path::Path, base_url: &str) {
    let txt = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let mut v: Value = meclaw_core::serde_json::from_str(&txt)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    v["params"]["base_url"] = Value::String(base_url.to_string());
    std::fs::write(path, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// Builds a `CellFactoryRegistry` with `code` and `llm` factories (the two
/// types used by the 14-D topology). Mirrors `factories_14c` from
/// `phase_14c_lexical_rag.rs`.
fn factories_14d() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "code".to_string(),
        Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
    );
    reg.insert(
        "llm".to_string(),
        Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
    );
    reg
}

/// Boot: `code` + `llm` factories + `/sink` CaptureCell + bootstrap over `dir`.
/// Mirrors `boot_code_store_and_llm` from `phase_14c_lexical_rag.rs` but without
/// the `store` factory (the 14-D topology is store-free).
async fn boot_code_and_llm(td: &TempDir) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "llm".to_string(),
                Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
            ),
        ],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let registry = factories_14d();
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// Boot full topology: copy+patch the 14-D tree, boot code+llm, return handle +
/// both mocks + sink receiver + the TempDir (must be kept alive).
///
/// Both mocks are pre-wired with their distinct canned answers:
/// - `mock_billing` → `"BILLING_ANSWER"` (finish_reason: `"stop"`)
/// - `mock_tech`    → `"TECH_ANSWER"`    (finish_reason: `"stop"`)
///
/// Callers that need to assert mock request counts or snapshots receive the mock
/// handles back. The `TempDir` is returned because dropping it would delete the
/// tree that the colony is still serving from.
async fn boot_full_topology() -> (
    ColonyHandle,
    MockOpenAI,
    MockOpenAI,
    mpsc::Receiver<meclaw_core::Message>,
    TempDir,
) {
    let mock_billing =
        MockOpenAI::start(vec![canned_chat_completion("BILLING_ANSWER", "stop")]).await;
    let mock_tech = MockOpenAI::start(vec![canned_chat_completion("TECH_ANSWER", "stop")]).await;

    let billing_base_url = format!("{}/v1", mock_billing.base_url);
    let tech_base_url = format!("{}/v1", mock_tech.base_url);

    let td = TempDir::new().unwrap();
    copy_14d_tree_patch(td.path(), &billing_base_url, &tech_base_url);

    let (h, sink_rx) = boot_code_and_llm(&td).await;
    (h, mock_billing, mock_tech, sink_rx, td)
}

/// Inject a routed question into the colony: context `turn_id="t1"`, `session_id="s1"`,
/// TTL 16, body carries a single user-origin message with `text`.
///
/// Routes to `/router` — the entry-point of the 14-D topology.
async fn send_routed_question(h: &ColonyHandle, text: &str) {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("turn_id".into(), json!("t1"));
    ctx.insert("session_id".into(), json!("s1"));
    h.send(
        MessageBuilder::new(Path::new("/router"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": text}]
            })))
            .context(ctx)
            .ttl(16)
            .build(),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Task 1 — Router classifier isolated (deterministic)
// ---------------------------------------------------------------------------

/// **14-D-1 — Router classifies question to hop.agent_target.**
///
/// Boots a minimal topology containing only `/main/router` (code cell) and
/// a `/sink` CaptureCell. Feeds two questions and asserts:
///
/// - `"I need a refund for my invoice"` → `hop.agent_target == "billing"`
///   (BILLING score 2: refund, invoice; TECH score 0)
/// - `"login error"` → `hop.agent_target == "tech"`
///   (TECH score 2: login, error; BILLING score 0)
/// - `body.messages` == the passed-through input messages (1 user message each)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn router_classifies_question_to_agent_target() {
    let td = TempDir::new().unwrap();
    copy_router_tree(td.path());

    let (h, mut sink_rx) = support::boot_code_only(&td).await;

    // --- Case 1: billing question ---
    let billing_messages =
        json!([{"origin": "user", "type": "text", "text": "I need a refund for my invoice"}]);
    h.send(
        MessageBuilder::new(Path::new("/router"))
            .body(Body::Inline(json!({"messages": billing_messages})))
            .ttl(8)
            .build(),
    )
    .await;

    let msg1 = support::recv_bounded(&mut sink_rx)
        .await
        .expect("router must emit a message to /sink for billing question");

    assert_eq!(
        msg1.headers.hop.get("agent_target").and_then(Value::as_str),
        Some("billing"),
        "billing question must classify to hop.agent_target=billing; \
         hop = {:?}",
        msg1.headers.hop
    );

    // body.messages must be passed through unchanged
    let body1 = match &msg1.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected Inline body from router"),
    };
    let out_msgs1 = body1["messages"]
        .as_array()
        .expect("body.messages must be an array");
    assert_eq!(
        out_msgs1.len(),
        1,
        "exactly one message must be passed through"
    );
    assert_eq!(
        out_msgs1[0]["text"], "I need a refund for my invoice",
        "passed-through message text must match input"
    );

    // --- Case 2: tech question ---
    let tech_messages = json!([{"origin": "user", "type": "text", "text": "login error"}]);
    h.send(
        MessageBuilder::new(Path::new("/router"))
            .body(Body::Inline(json!({"messages": tech_messages})))
            .ttl(8)
            .build(),
    )
    .await;

    let msg2 = support::recv_bounded(&mut sink_rx)
        .await
        .expect("router must emit a message to /sink for tech question");

    assert_eq!(
        msg2.headers.hop.get("agent_target").and_then(Value::as_str),
        Some("tech"),
        "tech question must classify to hop.agent_target=tech; \
         hop = {:?}",
        msg2.headers.hop
    );

    // body.messages must be passed through unchanged
    let body2 = match &msg2.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected Inline body from router"),
    };
    let out_msgs2 = body2["messages"]
        .as_array()
        .expect("body.messages must be an array");
    assert_eq!(
        out_msgs2.len(),
        1,
        "exactly one message must be passed through"
    );
    assert_eq!(
        out_msgs2[0]["text"], "login error",
        "passed-through message text must match input"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Task 2 — agent_target discriminates exactly one agent
// ---------------------------------------------------------------------------

/// **14-D-2 — agent_target routes to exactly one agent.**
///
/// Boots the full topology (`router → billing|tech → capture → /sink`) with
/// two independent `MockOpenAI` instances (billing mock #1, tech mock #2).
///
/// Proof of single-fire via mock request COUNT (positive signal):
/// - Billing question `"I need a refund for my invoice"`:
///   mock_billing.recorded_requests().len() == 1, mock_tech.len() == 0
/// - Tech question `"login error"`:
///   mock_tech.recorded_requests().len() == 1, mock_billing.len() == 0
///
/// The settle signal is the `/sink` CaptureCell receipt (capture→/sink edge
/// added in the test, mirroring the 14-C-4 pattern).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_target_routes_to_exactly_one_agent() {
    // --- Case 1: billing question → mock_billing fires, mock_tech silent ---
    {
        let (h, mock_billing, mock_tech, mut sink_rx, _td) = boot_full_topology().await;
        send_routed_question(&h, "I need a refund for my invoice").await;

        // Settle signal: wait for /sink to receive the capture output.
        let _settle = support::recv_bounded(&mut sink_rx)
            .await
            .expect("billing path must deliver a message to /sink via capture");

        // Assert mock request counts (positive signal for single-fire).
        let billing_reqs = mock_billing.recorded_requests().await;
        let tech_reqs = mock_tech.recorded_requests().await;
        assert_eq!(
            billing_reqs.len(),
            1,
            "billing mock must receive exactly 1 request for billing question; \
             billing_reqs.len() = {}, tech_reqs.len() = {}",
            billing_reqs.len(),
            tech_reqs.len()
        );
        // Race-free zero-read: a mis-routed request would be recorded by the wrong mock within
        // the same dispatch, long before the correct path clears llm -> capture -> sink; the
        // sink receipt above therefore bounds both counts.
        assert_eq!(
            tech_reqs.len(),
            0,
            "tech mock must receive 0 requests for billing question; \
             billing_reqs.len() = {}, tech_reqs.len() = {}",
            billing_reqs.len(),
            tech_reqs.len()
        );

        h.shutdown().await;
    }

    // --- Case 2: tech question → mock_tech fires, mock_billing silent ---
    {
        let (h, mock_billing, mock_tech, mut sink_rx, _td) = boot_full_topology().await;
        send_routed_question(&h, "login error").await;

        // Settle signal.
        let _settle = support::recv_bounded(&mut sink_rx)
            .await
            .expect("tech path must deliver a message to /sink via capture");

        // Assert mock request counts (positive signal for single-fire).
        let billing_reqs = mock_billing.recorded_requests().await;
        let tech_reqs = mock_tech.recorded_requests().await;
        assert_eq!(
            tech_reqs.len(),
            1,
            "tech mock must receive exactly 1 request for tech question; \
             billing_reqs.len() = {}, tech_reqs.len() = {}",
            billing_reqs.len(),
            tech_reqs.len()
        );
        // Race-free zero-read: same rationale as the billing case above.
        assert_eq!(
            billing_reqs.len(),
            0,
            "billing mock must receive 0 requests for tech question; \
             billing_reqs.len() = {}, tech_reqs.len() = {}",
            billing_reqs.len(),
            tech_reqs.len()
        );

        h.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// Task 3 — hop.agent_target decays / context.turn_id couples — the core slice
// ---------------------------------------------------------------------------

/// **14-D-3 — hop.agent_target decays, context.turn_id couples across 2 hops.**
///
/// Full topology `router → billing|tech → capture → /sink`.  Asserts on the
/// message received at `/sink` (capture's emission, 2+ hops downstream of the
/// router):
///
/// POSITIVE: `context.turn_id == "t1"` — survives the full dispatch chain
///   (router → agent → capture) because `context` is persistent and only
///   written by edges/ingress, never replaced by cell emissions.
///
/// POSITIVE (session coupling): `context.session_id == "s1"` also survives.
///
/// NEGATIVE: `hop.agent_target` is ABSENT — the router emitted it into `hop`,
///   the dispatch edge consumed it in its CEL condition, and the agent's own
///   emission replaced `hop` entirely with fresh llm output keys.  There is no
///   mechanism to carry `hop.agent_target` past the agent emission.
///
/// POSITIVE (hop shape): `hop.exit_code == 0` — `capture` is a `code` cell;
///   its emission hop carries `{exit_code, duration_ms, had_stderr}`.
///   The llm's `{finish_reason, model, …}` keys are nested inside the echoed
///   body content, not in `msg.headers.hop`.  This positive signal makes the
///   absence assert non-hollow: the hop is not empty, it just no longer carries
///   the routing control field.  Exit code 0 (not merely present) confirms the
///   capture script exited cleanly.
///
/// This is the exact 14-C analogue of `hop_decays_unless_promoted`:
///   14-C: `hop.scores` decays / `context.retrieved_top_k` (promoted) survives.
///   14-D: `hop.agent_target` decays / `context.turn_id` (ingress) survives.
///
/// First-run result: RED — wrong positive signal: the test asserted
/// `hop.finish_reason` (the llm's own emission key) at the sink; diagnosis
/// showed that `capture` (a `code` cell) emits a fresh hop carrying
/// `{exit_code, duration_ms, had_stderr}`, replacing the llm's hop entirely —
/// this IS the decay mechanism per spec.  Fix was test-side only: positive
/// signal corrected to `hop.exit_code`.  Second run: GREEN.  No substrate change.
/// Task-4 refactor: `exit_code` assert tightened from `is_some()` to `== Some(0)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_target_decays_turn_id_couples() {
    let (h, _mock_billing, _mock_tech, mut sink_rx, _td) = boot_full_topology().await;
    send_routed_question(&h, "I need a refund for my invoice").await;

    // Settle signal: /sink receives capture's emission (2 hops past router).
    let msg = support::recv_bounded(&mut sink_rx)
        .await
        .expect("billing path must deliver a message to /sink via capture");

    // --- POSITIVE: context.turn_id survives 2 hops (router→billing→capture) ---
    let turn_id = msg
        .headers
        .context
        .get("turn_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        turn_id, "t1",
        "context.turn_id must survive router→agent→capture; \
         context = {:?}",
        msg.headers.context
    );

    // --- POSITIVE: context.session_id also survives ---
    let session_id = msg
        .headers
        .context
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        session_id, "s1",
        "context.session_id must survive router→agent→capture; \
         context = {:?}",
        msg.headers.context
    );

    // --- NEGATIVE: hop.agent_target is absent — decayed at agent emission ---
    assert!(
        msg.headers.hop.get("agent_target").is_none(),
        "hop.agent_target must be absent at /sink — the router emitted it into hop, \
         the dispatch edge consumed it in CEL, and the agent's own emission replaced \
         hop entirely; hop = {:?}",
        msg.headers.hop
    );

    // --- POSITIVE (non-hollow): hop.exit_code == 0 (capture code cell's fresh hop) ---
    // The capture cell is a `code` cell; its own emission hop carries
    // {exit_code, duration_ms, had_stderr}.  The llm's {finish_reason, model, …}
    // keys are inside the echoed body content (not promoted to context), not in
    // msg.headers.hop.  Exit code 0 (not merely present) confirms the capture
    // script exited cleanly, proving the hop is not empty but no longer carries
    // the routing control field.
    assert_eq!(
        msg.headers.hop.get("exit_code").and_then(Value::as_i64),
        Some(0),
        "hop.exit_code must be 0 — the capture code cell's fresh output hop, \
         proving the hop carries the cell's own output keys (not agent_target); \
         hop = {:?}",
        msg.headers.hop
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Task 4 — finale Antwort gekoppelt, end-to-end + Receipt
// ---------------------------------------------------------------------------

/// **14-D-4 — the chosen agent's final answer is coupled to turn_id.**
///
/// Full topology `router → billing|tech → capture → /sink`.  Billing question
/// `"I need a refund for my invoice"` → billing agent fires.
///
/// Asserts (all required):
///
/// RECEIPT: `/sink` receives capture's emission carrying `"BILLING_ANSWER"` in
///   `body.messages[0].text` — the capture cell echoes the llm body verbatim.
///   This is the positive Receipt signal (CaptureCell ⟺ live billing agent
///   response arrived end-to-end).
///
/// COUPLING: `context.turn_id == "t1"` at the /sink message — the user turn
///   context survives the full router→billing→capture dispatch chain.
///
/// REQUEST CONTENT: the billing mock's captured request equals the injected
///   user question in `messages[].content` — the billing agent's HTTP call to
///   Mock #1 carries the user turn that was routed to it.
///
/// SINGLE-FIRE: `mock_tech.recorded_requests().is_empty()` — the tech mock
///   receives 0 requests for a billing question (re-asserts Task-2 e2e).
///
/// DLQ EMPTY: `ReadDeadLetters` returns 0 entries — no dead-letter anywhere
///   over the full dispatch chain.
///
/// First-run result: GREEN on first attempt.  The harness already wired distinct
/// canned answers ("BILLING_ANSWER"/"TECH_ANSWER") since Task 2, and the
/// `boot_full_topology` helper (introduced by the Task-3 quality review refactor)
/// bundles them.  The new asserts (body content, request snapshot, DLQ) were not
/// previously checked — adding them here is the diagnosis receipt.  Confirmed
/// GREEN without substrate change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_answer_coupled_to_turn() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    let (h, mock_billing, mock_tech, mut sink_rx, _td) = boot_full_topology().await;
    send_routed_question(&h, "I need a refund for my invoice").await;

    // --- RECEIPT: /sink receives capture's emission ---
    let msg = support::recv_bounded(&mut sink_rx)
        .await
        .expect("billing path must deliver a message to /sink via capture");

    // Capture echoes the llm body: messages[0].text must be "BILLING_ANSWER".
    let body_val = match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected Inline body from capture"),
    };
    let answer_text = body_val["messages"][0]["text"]
        .as_str()
        .unwrap_or("<missing>");
    assert_eq!(
        answer_text, "BILLING_ANSWER",
        "capture Receipt must carry BILLING_ANSWER in body.messages[0].text; \
         body = {:?}",
        body_val
    );

    // --- COUPLING: context.turn_id == "t1" at the sink ---
    assert_eq!(
        msg.headers.context.get("turn_id").and_then(Value::as_str),
        Some("t1"),
        "context.turn_id must be t1 at /sink (coupled across router→billing→capture); \
         context = {:?}",
        msg.headers.context
    );

    // --- REQUEST CONTENT: billing mock received the user question ---
    let billing_snaps = mock_billing.recorded_requests().await;
    assert_eq!(
        billing_snaps.len(),
        1,
        "billing mock must have exactly 1 recorded request; got {}",
        billing_snaps.len()
    );
    let req_messages = billing_snaps[0]
        .messages()
        .expect("billing mock request must carry a messages array");
    // The user-role turn must carry the exact injected question text.
    let user_turn = req_messages
        .iter()
        .find(|m| m["role"] == "user")
        .expect("billing mock request must contain a user-role message");
    assert_eq!(
        user_turn["content"], "I need a refund for my invoice",
        "user-role content must equal the injected question exactly; messages = {:?}",
        req_messages
    );

    // --- SINGLE-FIRE: tech mock receives 0 requests ---
    // Race-free zero-read: see rationale in agent_target_routes_to_exactly_one_agent.
    let tech_snaps = mock_tech.recorded_requests().await;
    assert_eq!(
        tech_snaps.len(),
        0,
        "tech mock must receive 0 requests for a billing question (single-fire holds e2e); \
         got {}",
        tech_snaps.len()
    );

    // --- DLQ EMPTY ---
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.runtime()
        .inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let dlq = ack_rx.await.unwrap();
    assert_eq!(
        dlq.entries.len(),
        0,
        "DLQ must be empty — no dead-letter over the full billing dispatch chain: {:?}",
        dlq.entries
            .iter()
            .map(|e| e.error_code.clone())
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Task 5 — the mutation validator confirms build-time-clean (+ a negative case)
// ---------------------------------------------------------------------------

/// **14-D-5 Positiv — Validator akzeptiert die committete Multi-Agent-Topologie.**
///
/// Bootstraps the full `tests/fixtures/14d-multi-agent/` tree via `plan_bootstrap`
/// (validate-only, no spawn, no HTTP). Asserts `Ok`:
///
/// - `consumes.context.turn_id` required at billing/tech/capture is reachable via
///   the ingress root — `/router` DECLARES `contract.ingress.context: turn_id`
///   (GH #185: it used to qualify by having no incoming edge, which was a fact
///   about the graph rather than about the cell), and the path
///   router→billing/tech/capture carries no delete_context for turn_id.
/// - `agent_target` lives only in Edge conditions (not in any `consumes.hop`) →
///   hop-locality rule is vacuously satisfied (no required hop keys anywhere).
///
/// First-run result: GREEN — Tasks 2–4 already boot this tree successfully;
/// a clean boot IS the validator pass.  The test pins this property explicitly.
#[test]
fn validator_accepts_multi_agent_topology() {
    let td = TempDir::new().unwrap();
    // Copy tree; leave PLACEHOLDER base_urls — plan_bootstrap does not spawn,
    // so HTTP is irrelevant.
    support::copy_dir_recursive(&support::example_dir("14d-multi-agent"), td.path());

    let overlay = RegistryOverlay::new();
    let result = plan_bootstrap(td.path(), &factories_14d(), &overlay);
    assert!(
        result.is_ok(),
        "plan_bootstrap must accept the committed 14-D topology \
         (turn_id reachable via ingress root, no hop-locality violation); \
         errors: {:?}",
        result.err().map(|e| e.items().to_vec())
    );
}

/// **14-D-5 negative — the validator rejects `turn_id` as unreachable after delete_context.**
///
/// Copies the 14-D tree and mutates `main/config.json`: adds
/// `"modifier": {"delete_context": ["turn_id"]}` to the `router→billing` edge.
/// This severs the path from the declared ingress for `turn_id` on the billing branch —
/// billing declares `consumes.context.turn_id` required, but the only incoming
/// edge now carries a delete for that key.
///
/// Assertion: `plan_bootstrap` returns `Err` with a `HeaderContractViolation`
/// whose `reason` mentions both `"turn_id"` and `"context presence not reachable"`.
/// Proves: the correlation chain is not optional — the validator enforces it at
/// build time.
#[test]
fn validator_rejects_unreachable_turn_id() {
    let td = TempDir::new().unwrap();
    support::copy_dir_recursive(&support::example_dir("14d-multi-agent"), td.path());

    // Mutate main/config.json: inject delete_context: ["turn_id"] on router→billing.
    let main_cfg_path = td.path().join("main/config.json");
    let txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut cfg: Value = meclaw_core::serde_json::from_str(&txt).unwrap();
    // Surgical injection: find the router→billing edge in the PARSED committed config
    // and add the modifier — fails loudly if the edge vanishes from the committed tree.
    cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["to"] == "./billing")
        .expect("router→billing edge must exist in committed config")
        .as_object_mut()
        .unwrap()
        .insert("modifier".into(), json!({"delete_context": ["turn_id"]}));
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let overlay = RegistryOverlay::new();
    let result = plan_bootstrap(td.path(), &factories_14d(), &overlay);
    let errs = result.expect_err(
        "plan_bootstrap must REJECT topology where delete_context severs turn_id \
         before billing (context presence not reachable)",
    );

    // At least one HeaderContractViolation must name "turn_id" and the canonical
    // reachability message.
    let found = errs.items().iter().any(|e| {
        if let BootstrapError::HeaderContractViolation { reason } = e {
            reason.contains("turn_id") && reason.contains("context presence not reachable")
        } else {
            false
        }
    });
    assert!(
        found,
        "expected HeaderContractViolation mentioning 'turn_id' and \
         'context presence not reachable'; got: {:?}",
        errs.items()
    );
}

// ---------------------------------------------------------------------------
// Task 6 — live-graph SVG/DOT from the booted multi-agent topology
// ---------------------------------------------------------------------------

/// **14-D-6 — live-graph SVG/DOT from the booted multi-agent topology.**
///
/// Boots the complete `tests/fixtures/14d-multi-agent/` tree (a TempDir copy,
/// both llm cells against mocks), reads the live graph via `ColonyMsg::ReadGraph`
/// for the scope `/`, renders `graph.dot` + `graph.svg` with the zero-dep
/// generator (identical to 14c) and writes them to
/// `tests/fixtures/14d-multi-agent/` (under the env gate `MECLAW_EMIT_DOT=1`).
///
/// Assertions:
/// - All 4 topology nodes (`/router`, `/billing`, `/tech`, `/capture`)
///   erscheinen im DOT.
/// - Both CEL conditions (`agent_target == 'billing'`, `agent_target == 'tech'`)
///   erscheinen im DOT als Edge-Labels.
/// - The generated SVG is a valid SVG document (`<svg`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_svg_from_live_graph() {
    let (h, _mock_billing, _mock_tech, mut sink_rx, _td) = boot_full_topology().await;
    send_routed_question(&h, "I need a refund for my invoice").await;

    // Positive receipt: the full chain router→billing→capture→/sink runs completely,
    // so that all nodes are registered in the live graph.
    let _msg = support::recv_bounded(&mut sink_rx)
        .await
        .expect("billing path must deliver a message to /sink via capture");

    // Read the live graph: scope "/" returns all topology nodes + edges.
    let (nodes, edges) = support::live_graph(&h, &["/"]).await;

    // Write SVG/DOT to tests/fixtures/14d-multi-agent/ under the env gate `MECLAW_EMIT_DOT=1`.
    support::emit_dot_if_requested("14d-multi-agent", &nodes, &edges);

    // --- Render DOT/SVG locally for the assertions (independent of MECLAW_EMIT_DOT) ---
    let dot = support::render_dot(&nodes, &edges);
    let svg = support::render_svg(&nodes, &edges);

    // All 4 topology nodes must appear in the DOT.
    for node in &["/router", "/billing", "/tech", "/capture"] {
        assert!(
            dot.contains(node),
            "DOT must contain node {node}; dot = {dot:?}"
        );
    }

    // Both CEL conditions must appear as complete edge labels in the DOT.
    assert!(
        dot.contains("hop.agent_target == 'billing'"),
        "DOT must contain full billing edge label; dot = {dot:?}"
    );
    assert!(
        dot.contains("hop.agent_target == 'tech'"),
        "DOT must contain full tech edge label; dot = {dot:?}"
    );

    // The SVG must be a valid SVG document and contain all 4 topology nodes.
    assert!(
        svg.contains("<svg"),
        "rendered output must be an SVG document; got = {svg:.200?}"
    );
    for node in &["/router", "/billing", "/tech", "/capture"] {
        assert!(
            svg.contains(node),
            "SVG must contain node {node}; svg = {svg:.200?}"
        );
    }

    h.shutdown().await;
}
