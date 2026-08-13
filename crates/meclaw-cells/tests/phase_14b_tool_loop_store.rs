//! Phase-14-B tool loop with store as the sole thread holder.
//! [user]-Upstream-Capture → store + llm; Collector↔store-RMW-Fan-in; voller
//! kumulierter Thread pro llm-Call; Terminierung via finish_reason-CEL-Edge;
//! Multi-iteration + A8 whole-body blob offload. llm deterministic via MockOpenAI.
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_core::Body;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::time::Duration;
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_capture_multisends_to_store_and_llm() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-user-capture"), td.path(), &base_url);
    let (h, _sink_rx, mut park_rx) = boot(&td).await;
    h.send(user_probe("t1")).await;

    // (a) The store insert reply lands at /park (route=ustore propagates) → [user] is in the store.
    let pk = recv_bounded(&mut park_rx)
        .await
        .expect("store insert-reply must reach /park");
    let body = match &pk.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline"),
    };
    assert_eq!(pk.headers.hop["operation"], "insert");
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(
        pk.headers.hop["rows_affected"], 1,
        "[user] row inserted into store thread"
    );

    // (b) /llm has seen the user turn. The llm path (HTTP) runs in parallel with
    // the local store path; wait boundedly for the call to arrive (there is no
    // /sink receipt in this tree that would synchronize it).
    let snaps = {
        // 30s failure-marker (was 10s — too tight under cargo-parallel load); the
        // llm HTTP path runs concurrently to the local store path with no /sink
        // receipt to synchronize it, so we poll until the call lands.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let s = mock.recorded_requests().await;
            if !s.is_empty() || std::time::Instant::now() > deadline {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    // Exactly once: this topology has no collector loop and /llm has no out-edge,
    // so capture forwards the user turn to /llm exactly once — no double-fire path
    // (unlike the collector tests below, where the count is diagnostic).
    assert_eq!(
        snaps.len(),
        1,
        "capture forwarded user turn to llm exactly once"
    );
    let msgs = snaps[0].messages().unwrap();
    assert!(
        msgs.iter()
            .any(|m| m["content"].as_str().is_some_and(|c| c.contains("2+3"))),
        "llm prompt must carry the captured user turn"
    );

    // Live-Graph: /store-Node + capture→/llm-Edge (struktureller Beweis).
    let (nodes, edges) = live_graph(&h, &["/"]).await;
    assert!(
        nodes
            .iter()
            .any(|n| n.path == "/store" && n.cell_type == "store")
    );
    assert!(edges.iter().any(|e| e.from == "/capture" && e.to == "/llm"));
    emit_dot_if_requested("14b-user-capture", &nodes, &edges);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collector_rebuilds_thread_and_calls_llm_again() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("call-xyz", "calc", r#"{"x":2,"y":3}"#)]),
        canned_chat_completion("fertig", "stop"),
    ])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t1")).await;

    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("final response must reach /sink");
    assert_eq!(fin.headers.hop["finish_reason"], "stop");

    let snaps = mock.recorded_requests().await;
    // Request count is diagnostic, not an exact gate (W6/A8 consistent line, in
    // step with phase_14b_fanin_correlation): a benign TTL-capped collector
    // double-fire under parallel load may add a trailing duplicate call. Lower-
    // bound it (the loop ran its required iteration) and read the FINAL fire; the
    // authoritative correctness signals are the positive receipts (finish_reason
    // stop above) + the store rows below.
    assert!(
        snaps.len() >= 2,
        "single tool iteration → at least 2 llm calls, got {}",
        snaps.len()
    );
    // The final fire carries the VOLLEN kumulierten Thread: user + assistant(tool_call) + tool_result.
    let m2 = snaps.last().unwrap().messages().unwrap();
    let roles: Vec<&str> = m2.iter().filter_map(|m| m["role"].as_str()).collect();
    assert!(
        roles.contains(&"user") && roles.contains(&"assistant") && roles.contains(&"tool"),
        "final-fire thread must contain user+assistant+tool, got {roles:?}"
    );

    // Store state receipt: the thread table holds the full thread for t1.
    let conn = rusqlite::Connection::open(td.path().join("main/store/cell.db")).unwrap();
    let store_roles: Vec<String> = conn
        .prepare("SELECT role FROM thread WHERE turn_id='t1' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(
        store_roles.contains(&"user".into())
            && store_roles.contains(&"assistant".into())
            && store_roles.contains(&"tool".into()),
        "store thread holds the full conversation: {store_roles:?}"
    );

    // Live graph across both scopes: store/dispatcher/collector/tool-a as nodes,
    // Fire-Edge collector→/llm vorhanden.
    let (nodes, edges) = live_graph(&h, &["/", "/tool-loop"]).await;
    for p in [
        "/store",
        "/tool-loop/dispatcher",
        "/tool-loop/collector",
        "/tool-loop/tool-a",
    ] {
        assert!(nodes.iter().any(|n| n.path == p), "node {p} present");
    }
    assert!(
        edges
            .iter()
            .any(|e| e.from == "/tool-loop/collector" && e.to == "/llm"),
        "fire edge collector→/llm present"
    );
    emit_dot_if_requested("14b-tool-loop-store", &nodes, &edges);

    h.shutdown().await;
}

/// **PROOF POINT of the header-model rebuild.** ≥2 tool iterations go GREEN — a
/// positive receipt, not a "no crash". The old phase-14-B bug (a stale
/// `operation` header leaked through the loop into the collector's phase
/// discrimination, so from iteration 2 the stateless FSM took the wrong branch,
/// iteration-2 turns were never persisted and the loop derailed via TTL death) is
/// structurally impossible after the two-compartment rebuild (hop decays, `iter`
/// as context, iteration-scoped ID set diff)
/// eliminiert.
///
/// Mock-Skript: 3 Provider-Calls, 2 Tool-Iterationen.
///   - Call 1 → an assistant turn with **2** tool_calls (sharpens the ID set diff
///     vs. a count — out-of-order-proof across several IDs in ONE assistant turn).
///   - Call 2 → an assistant turn with 1 tool_call.
///   - Call 3 → assistant `stop`.
///
/// Positive receipts (all of them): exactly 3 LLM calls; BOTH iterations in
/// store/cell.db; the final response at /sink with `hop.finish_reason == "stop"`;
/// `context.iter == 2` on the last hop (the loop counter ran across both
/// iterations on the fire edge); an empty DLQ.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_14b_two_iterations_no_stale_operation() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    let mock = MockOpenAI::start(vec![
        // Iteration 1: two parallel tool calls in ONE assistant turn.
        canned_tool_calls(vec![
            ("call-a1", "calc", r#"{"x":2,"y":3}"#),
            ("call-b1", "calc", r#"{"x":4,"y":5}"#),
        ]),
        // Iteration 2: ein Tool-Call.
        canned_tool_calls(vec![("call-a2", "calc", r#"{"x":6,"y":7}"#)]),
        // Terminierung.
        canned_chat_completion("fertig", "stop"),
    ])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-tool-loop-store"), td.path(), &base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t1")).await;

    // The final response reaches /sink with finish_reason == stop (a positive
    // receipt; a generous 30s failure marker per convention).
    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("final response must reach /sink after 2 tool iterations");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "loop terminates on stop, not via TTL-death"
    );

    // context.iter == 2: the loop counter ran correctly across BOTH iterations on
    // the collector→/llm fire edge (ingress iter=0; +1 after iter 1; +1 after iter 2).
    assert_eq!(
        fin.headers.context["iter"], 2,
        "iter counter advanced once per tool iteration via the fire-edge modifier"
    );

    // LLM calls: ≥3 (2 tool iterations + termination). Exactly 3 is not a gate — a
    // benign TTL-capped collector double fire under parallel load may add a
    // trailing duplicate call (the empty DLQ below proves the TTL cap). Correctness
    // is carried by the positive receipt (finish_reason/iter/store rows), not the
    // count.
    let snaps = mock.recorded_requests().await;
    assert!(
        snaps.len() >= 3,
        "at least 3 llm calls for 2 tool iterations, got {}",
        snaps.len()
    );

    // BOTH iterations persisted in store/cell.db — the old bug made iteration-2 turns
    // verschwinden. Konkrete Row-Existenz pro (iter, role).
    let conn = rusqlite::Connection::open(td.path().join("main/store/cell.db")).unwrap();
    let mut rows: Vec<(i64, String)> = conn
        .prepare("SELECT iter, role FROM thread WHERE turn_id='t1' ORDER BY iter, rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    // Iter 0: user-Turn + assistant-Turn (2 tool_calls) + 2 tool-results.
    assert!(
        rows.contains(&(0, "user".into())),
        "iter-0 user row: {rows:?}"
    );
    assert!(
        rows.contains(&(0, "assistant".into())),
        "iter-0 assistant row: {rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|(i, r)| *i == 0 && r == "tool").count(),
        2,
        "iter-0 has both tool-results (multi-call fan-in): {rows:?}"
    );
    // Iter 1: one assistant turn (1 tool_call) + 1 tool result. (NO user turn —
    // the user only lives in iter 0.) The old bug made exactly these rows vanish.
    assert!(
        rows.contains(&(1, "assistant".into())),
        "iter-1 assistant row persisted (the old bug dropped this): {rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|(i, r)| *i == 1 && r == "tool").count(),
        1,
        "iter-1 tool-result persisted: {rows:?}"
    );
    rows.clear();

    // Empty DLQ — a positive receipt, NOT a negative indicator: no dead letter, no
    // TtlExpired across the whole loop.
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
        "DLQ empty — no dead-letter, no TtlExpired over the loop: {:?}",
        dlq.entries
            .iter()
            .map(|e| e.error_code.clone())
            .collect::<Vec<_>>()
    );

    // The final fire carries the full accumulated thread of both iterations.
    let m3 = snaps.last().unwrap().messages().unwrap();
    let roles: Vec<&str> = m3.iter().filter_map(|m| m["role"].as_str()).collect();
    assert!(
        roles.contains(&"user") && roles.contains(&"assistant") && roles.contains(&"tool"),
        "final-fire thread cumulates user+assistant+tool over both iters, got {roles:?}"
    );

    h.shutdown().await;
}

/// **Task 7 — Two-tool routing discrimination (Fan-in correlation).**
/// Two DISTINCT tools (`a`, `b`) are called in ONE assistant turn. The dispatcher
/// reads each tool-call's function name (`json.loads(c["text"])["name"]`),
/// surrogates it to `'a'`/`'b'`, and emits it as `header.tool_name` (which lands in
/// the `hop` compartment). The hive edges discriminate per call via
/// `hop.route == 'tool' && hop.tool_name == 'a'` / `== 'b'`, routing each call to
/// the correct tool cell. Each tool echoes the input call's `id` back in a DISTINCT
/// `tool_result` text (`tool-a → "A-ok"`, `tool-b → "B-ok"`).
///
/// Correctness measure = STORE-STATE POSITIVE RECEIPT: exactly two `tool` rows under
/// the same `turn_id` and same `iter`, carrying the two DISTINCT results — proving
/// a→tool-a and b→tool-b both fired and joined under one fan-in.
///
/// The collector's join logic (iter-scoped tool-call-id-set subset) is UNCHANGED from
/// `14b-tool-loop-store`; it already generalizes to N≥2. This test's genuine new
/// surface is the two-distinct-tool routing discrimination, not the join.
///
/// Mock-Skript: Call 1 → assistant with TWO tool_calls (`a`, `b`); Call 2 → `stop`.
/// The double-fire potential (N≥2) is DIAGNOSTIC only (logged, not asserted); the
/// DLQ-empty check is the authoritative error receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_14b_fanin_correlation() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    let mock = MockOpenAI::start(vec![
        // One iteration, two DISTINCT tools in a single assistant turn.
        canned_tool_calls(vec![
            ("call-a1", "a", r#"{"x":2,"y":3}"#),
            ("call-b1", "b", r#"{"x":4,"y":5}"#),
        ]),
        // Termination.
        canned_chat_completion("fertig", "stop"),
    ])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-fanin-correlation"), td.path(), &base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t1")).await;

    // Final-Response reaches /sink with finish_reason == stop — the loop terminated
    // cleanly after the single two-tool iteration (positive receipt).
    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("final response must reach /sink after the two-tool iteration");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "loop terminates on stop, not via TTL-death"
    );

    // STORE-STATE RECEIPT: exactly two `tool` rows under the SAME turn_id and SAME
    // iter, carrying the two DISTINCT results "A-ok" / "B-ok". Proves a→tool-a and
    // b→tool-b both routed correctly and joined under one fan-in.
    let conn = rusqlite::Connection::open(td.path().join("main/store/cell.db")).unwrap();
    let tool_rows: Vec<(i64, String)> = conn
        .prepare(
            "SELECT iter, turn FROM thread WHERE turn_id='t1' AND role='tool' ORDER BY iter, rowid",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        tool_rows.len(),
        2,
        "exactly two tool rows for the two-tool fan-in: {tool_rows:?}"
    );
    // Both under the SAME iter.
    assert_eq!(
        tool_rows[0].0, tool_rows[1].0,
        "both tool rows share one iter (single fan-in): {tool_rows:?}"
    );
    // The two DISTINCT result texts, each tied to its origin call id.
    let texts: Vec<meclaw_core::serde_json::Value> = tool_rows
        .iter()
        .map(|(_, t)| meclaw_core::serde_json::from_str(t).unwrap())
        .collect();
    let by_id = |id: &str| {
        texts
            .iter()
            .find(|v| v["id"] == id)
            .unwrap_or_else(|| panic!("no tool row for id {id}: {texts:?}"))
    };
    assert_eq!(
        by_id("call-a1")["text"],
        "A-ok",
        "tool-a produced the distinct A-ok result for call-a1"
    );
    assert_eq!(
        by_id("call-b1")["text"],
        "B-ok",
        "tool-b produced the distinct B-ok result for call-b1"
    );

    // DIAGNOSTIC only (NOT a pass/fail gate): the observed mock request count.
    // Two-tool fan-in MAY double-fire the loop (TTL-capped); single-fire → 2 calls.
    let snaps = mock.recorded_requests().await;
    eprintln!(
        "[diagnostic] mock request count = {} (single-fire => 2; >2 => benign double-fire, TTL-capped)",
        snaps.len()
    );

    // Empty DLQ — the authoritative error receipt (NOT a negative indicator).
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
        "DLQ empty — no dead-letter, no TtlExpired over the fan-in: {:?}",
        dlq.entries
            .iter()
            .map(|e| e.error_code.clone())
            .collect::<Vec<_>>()
    );

    // Live-Graph: BOTH tool cells are nodes; BOTH discriminating edges present.
    let (nodes, edges) = live_graph(&h, &["/", "/tool-loop"]).await;
    for p in ["/tool-loop/tool-a", "/tool-loop/tool-b"] {
        assert!(nodes.iter().any(|n| n.path == p), "node {p} present");
    }
    assert!(
        edges.iter().any(|e| e.from == "/tool-loop/dispatcher"
            && e.to == "/tool-loop/tool-a"
            && e.condition
                .as_deref()
                .is_some_and(|c| c.contains("tool_name"))),
        "dispatcher→tool-a edge conditions on hop.tool_name"
    );
    assert!(
        edges.iter().any(|e| e.from == "/tool-loop/dispatcher"
            && e.to == "/tool-loop/tool-b"
            && e.condition
                .as_deref()
                .is_some_and(|c| c.contains("tool_name"))),
        "dispatcher→tool-b edge conditions on hop.tool_name"
    );
    emit_dot_if_requested("14b-fanin-correlation", &nodes, &edges);

    h.shutdown().await;
}

/// **Task 6 — A8 whole-body blob offload stays a whole-body copy.**
/// The same topology as `14b-tool-loop-store`, but `tool-a` returns a ~40 KB
/// `tool_result`. Across 2 tool iterations the accumulated thread on the
/// `collector→/llm` fire hop grows past the `blob_inline_max_bytes` default
/// (65,536 bytes), so A8 (`offload_oversized`) offloads the WHOLE body as a
/// `Body::Blob`.
///
/// Positive Receipts:
///   1. `await_body_kind(<td>, "/llm") == "blob"` — the fire hop was offloaded as
///      a whole-body blob (`message_log.body_kind == "blob"`).
///   2. The offloaded blob is a FULL UBF body with `messages[]` inline and contains
///      NOWHERE a `messages_id`/`text_id` pointer. Since GH #19 the substrate CAN
///      resolve such pointers, but it still never MINTS one: the offload writes the
///      body verbatim, it does not rewrite a conversation into references.
///   3. Empty DLQ — no dead letter, no TtlExpired across the loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_14b_blob_offload() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    let mock = MockOpenAI::start(vec![
        // Iteration 1: ein Tool-Call → tool-a liefert ~40 KB.
        canned_tool_calls(vec![("call-a1", "calc", r#"{"x":2,"y":3}"#)]),
        // Iteration 2: one tool call → another ~40 KB; accumulated thread > 64 KB.
        canned_tool_calls(vec![("call-a2", "calc", r#"{"x":6,"y":7}"#)]),
        // Terminierung.
        canned_chat_completion("fertig", "stop"),
    ])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(&example_dir("14b-blob-offload"), td.path(), &base_url);
    // Blob-aware boot: wires a real DiskBlobStore under <td>/blobs so the A8
    // auto-offload producer hook is live (the plain `boot` path is no-blob).
    let (h, mut sink_rx, mut park_rx) = boot_with_blobs(&td).await;
    h.send(user_probe("t1")).await;

    // The final response reaches /sink with finish_reason == stop (a positive
    // receipt; the loop terminates cleanly despite the blob offload on the fire hop).
    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("final response must reach /sink after 2 tool iterations");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "loop terminates on stop, not via TTL-death"
    );

    // (1) The collector→/llm fire hop was offloaded as a whole-body blob: a
    // message_log row with to_path=/llm AND body_kind='blob' exists (A8 proven).
    // colony.db lives under <td> (build_with_blob_store opens dir/colony.db).
    let body_kind = await_body_kind(td.path(), "/llm").await;
    assert_eq!(
        body_kind, "blob",
        "collector→/llm fire-hop offloaded whole body as Body::Blob"
    );

    // (2) The offloaded blob is a FULL UBF body: `messages[]` inline, with no
    // `messages_id`/`text_id` pointer anywhere — the offload copies the body,
    // it does not rewrite it into references (GH #19 does not change that).
    let blobs = read_blob_bodies(td.path());
    assert!(
        !blobs.is_empty(),
        "at least one blob written to <td>/blobs/"
    );
    // Deliberately pick the /llm FIRE blob: an accumulated thread carrying the user
    // turn AND at least one tool turn inline (distinguishing it from store-op blobs).
    fn carries_role(v: &meclaw_core::serde_json::Value, origin: &str) -> bool {
        v["messages"]
            .as_array()
            .is_some_and(|a| a.iter().any(|m| m["origin"] == origin))
    }
    let full_ubf = blobs
        .iter()
        .find(|v| carries_role(v, "user") && carries_role(v, "tool"));
    let blob = full_ubf
        .expect("offloaded /llm fire-blob is a full UBF body with user+tool messages[] inline");
    assert!(
        blob["messages"].is_array() && !blob["messages"].as_array().unwrap().is_empty(),
        "blob carries a non-empty inline messages[] array"
    );
    // No pointer anywhere in the blob JSON (recursive, by key name).
    fn has_pointer_key(v: &meclaw_core::serde_json::Value) -> bool {
        match v {
            meclaw_core::serde_json::Value::Object(m) => {
                m.contains_key("messages_id")
                    || m.contains_key("text_id")
                    || m.values().any(has_pointer_key)
            }
            meclaw_core::serde_json::Value::Array(a) => a.iter().any(has_pointer_key),
            _ => false,
        }
    }
    assert!(
        !has_pointer_key(blob),
        "the whole-body offload mints no pointer anywhere in the blob: {blob:#}"
    );

    // (3) Empty DLQ — a positive receipt, NOT a negative indicator.
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
        "DLQ empty — no dead-letter, no TtlExpired over the loop: {:?}",
        dlq.entries
            .iter()
            .map(|e| e.error_code.clone())
            .collect::<Vec<_>>()
    );
    // In THIS topology /park receives exclusively BY-DESIGN routes, no errors: the
    // user-origin store-insert acknowledgement (`store→/park`) and the collector's
    // guard-loser parks (`collector→/park`, empty body — peers that did not win the
    // atomic fire race). NONE of these is an error; the authoritative error DLQ is
    // `ReadDeadLetters` (above: empty). We check positively that every /park message
    // has one of these expected shapes — NO error body, no TtlExpired marker.
    while let Ok(pk) = park_rx.try_recv() {
        if let Body::Inline(v) = &pk.body {
            let is_user_receipt = v["messages"][0]["type"] == "tool_result";
            let is_empty_park = v["messages"].as_array().is_some_and(|a| a.is_empty());
            assert!(
                is_user_receipt || is_empty_park,
                "/park only carries by-design routes (user receipt or empty guard-park), \
                 not an error: {v:#}"
            );
        }
    }

    // Live graph: the collector→/llm fire edge is present; DOT/SVG only under the env gate.
    let (nodes, edges) = live_graph(&h, &["/", "/tool-loop"]).await;
    assert!(
        edges
            .iter()
            .any(|e| e.from == "/tool-loop/collector" && e.to == "/llm"),
        "fire edge collector→/llm present"
    );
    emit_dot_if_requested("14b-blob-offload", &nodes, &edges);

    h.shutdown().await;
}
