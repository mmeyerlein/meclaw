//! GH #1273 — a registry push moves an `llm` cell onto another provider, and
//! the credential follows the row through the vault, without a rebirth.
//!
//! | claim | test |
//! |---|---|
//! | a push switches openai → anthropic: next call on the Messages wire with the second ref's secret; a push back returns to the chat wire with the first ref's secret, no new vault round; one `llm_switch` line each | [`gh1273_a_push_moves_the_cell_onto_another_provider_and_back`] |
//! | a switch without its own `base_url`, a switch without its own `credential_ref` and a switch into `decisions` are refused; a denied ref answers the parked turn at once | same test |
//! | a cell born without `base_url` takes a push naming its lane's default host | same test |
//!
//! The vault half is played by the test (as in `gh1058_*`): it reads the
//! recipient key out of the cell's `credential_request` and seals a stub value
//! to it. Stub values only.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::{LlmParams, sealed};
use meclaw_colony::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use tokio::sync::mpsc;

const GRANT: &str = "grant:1273";

fn sink_for(tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn user_turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

fn push(update: Value) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/registry"))
        .body(Body::Inline(json!({"system": {}, "params": update})))
        .build()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "luna-1273",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "from chat"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

fn messages_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "msg_1273", "type": "message", "role": "assistant", "model": "claude-1273",
            "content": [{"type": "text", "text": "from messages"}], "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

fn cell_db(td: &tempfile::TempDir) -> meclaw_colony::DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    meclaw_colony::DbConn::wrap(conn, None)
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em.content);
    }
    out
}

fn args_of(seen: &[Value]) -> (Value, Value) {
    let req = seen
        .iter()
        .find(|e| e["header"]["route"] == "credential_request")
        .unwrap_or_else(|| panic!("the cell never asked for its credential: {seen:?}"));
    let args: Value =
        meclaw_core::serde_json::from_str(req["messages"][0]["text"].as_str().expect("text"))
            .expect("args are JSON");
    (req.clone(), args)
}

fn seal_for(seen: &[Value], secret: &str) -> Message {
    let (_, args) = args_of(seen);
    let recipient = args["payload"]["recipient_key"]
        .as_str()
        .expect("recipient_key");
    let boxed = sealed::seal_to(recipient, secret.as_bytes())
        .expect("seal")
        .to_json();
    let (req, _) = args_of(seen);
    // The broker's ack names the bare grant and echoes the call id -- the form
    // the access hive delivers in.
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "sealed": boxed,
            "messages": [{"origin": "tool", "type": "tool_result",
                          "id": req["messages"][0]["id"],
                          "text": json!({"outcome": "delivered", "grant_id": GRANT,
                                         "operation": "vault.deliver"}).to_string()}],
        })))
        .build()
}

fn refused_with(seen: &[Value], detail: &str) -> bool {
    seen.iter()
        .any(|e| e["header"]["error_code"] == "invalid_input" && e.to_string().contains(detail))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1273_a_push_moves_the_cell_onto_another_provider_and_back() {
    let log = meclaw_testing::log_recorder::global();
    let (chat, _c, chat_seen) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer()]).await;
    let (anth, _a, anth_seen) = start_mock_server_capturing(vec![messages_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let mut c = LlmCell::new(
        LlmParams::parse(&json!({
            "provider": "openai", "model": "luna-1273", "api_key": "",
            "credential_grant_id": GRANT,
            "base_url": format!("http://{chat}/v1"),
            "base_url_allow": [format!("http://{chat}"), format!("http://{anth}")],
            "x_title": "Example App", "http_referer": "https://example.com",
            "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
            "credential_wait_ms": meclaw_testing::deadline::product_wait_ms(),
        }))
        .expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    );
    let (tx, mut rx) = mpsc::channel(64);

    // ── born on the chat wire: the grant's own credential, no ref named ──
    c.handle(user_turn("one"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    let (req, args) = args_of(&asked);
    assert_eq!(req["header"]["grant_id"], GRANT);
    assert!(args["payload"].get("cred_ref").is_none(), "{args}");
    c.handle(seal_for(&asked, "stub-or"), &sink_for(&tx), &mut db)
        .await;
    drain(&mut rx);

    // ── the push onto the native Messages wire ──
    c.handle(
        push(json!({"model": "claude-1273", "provider": "anthropic",
                    "base_url": format!("http://{anth}/v1"),
                    "credential_ref": "cred:anthropic", "wire_dialect": null})),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");
    c.handle(user_turn("two"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    let (req, args) = args_of(&asked);
    assert_eq!(
        req["header"]["grant_id"], GRANT,
        "the bare grant: the edges filter on it"
    );
    assert_eq!(args["grant_id"], GRANT);
    assert_eq!(args["payload"]["cred_ref"], "cred:anthropic");
    c.handle(seal_for(&asked, "stub-an"), &sink_for(&tx), &mut db)
        .await;
    let answered = drain(&mut rx);
    assert!(
        answered
            .iter()
            .any(|e| e["messages"].to_string().contains("from messages")),
        "{answered:?}"
    );
    let anth_reqs = anth_seen.lock().await.clone();
    assert_eq!(anth_reqs.len(), 1);
    assert_eq!(anth_reqs[0].path, "/v1/messages");
    assert_eq!(
        anth_reqs[0].headers.get("x-api-key").map(String::as_str),
        Some("stub-an")
    );
    assert_eq!(
        anth_reqs[0].headers.get("x-title").map(String::as_str),
        Some("Example App")
    );

    // ── the push back: the birth package, the first ref's slot, no new round ──
    c.handle(
        push(
            json!({"$reset": ["model", "provider", "base_url", "credential_ref",
                               "wire_dialect"]}),
        ),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    c.handle(user_turn("three"), &sink_for(&tx), &mut db).await;
    let seen = drain(&mut rx);
    assert!(
        seen.iter()
            .all(|e| e["header"]["route"] != "credential_request"),
        "the first ref's secret is still in RAM: {seen:?}"
    );
    let chat_reqs = chat_seen.lock().await.clone();
    let bearers: Vec<_> = chat_reqs
        .iter()
        .map(|r| r.headers.get("authorization").cloned().unwrap_or_default())
        .collect();
    assert_eq!(bearers, ["Bearer stub-or", "Bearer stub-or"]);

    let switches = log.lines(tracing::Level::INFO, &["llm_switch", "claude-1273"]);
    assert_eq!(switches.len(), 2, "{switches:?}");
    assert!(!format!("{switches:?}").contains("stub-"), "names only");

    // ── the guards ──
    c.handle(
        push(json!({"model": "claude-1273", "provider": "anthropic"})),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    assert!(
        refused_with(&drain(&mut rx), "provider_switch_needs_base_url"),
        "a switch on the birth host is refused"
    );
    c.handle(
        push(json!({"model": "claude-1273", "provider": "anthropic",
                    "base_url": format!("http://{anth}/v1"), "wire_dialect": null})),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    assert!(
        refused_with(&drain(&mut rx), "provider_switch_needs_credential"),
        "the start credential never goes to the other provider"
    );
    c.handle(
        push(json!({"provider": "decisions"})),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    assert!(
        refused_with(&drain(&mut rx), "provider_switch_refused"),
        "decisions is no switch target"
    );

    // ── the access hive refuses a ref: the parked turn is answered at once ──
    c.handle(
        push(json!({"model": "claude-1273", "provider": "anthropic",
                    "base_url": format!("http://{anth}/v1"),
                    "credential_ref": "cred:other", "wire_dialect": null})),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    c.handle(user_turn("four"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    let (req, args) = args_of(&asked);
    assert_eq!(args["payload"]["cred_ref"], "cred:other");
    let denial = MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "tool", "type": "tool_result", "id": req["messages"][0]["id"],
            "text": json!({"outcome": "denied", "reason_code": "cred_ref_not_granted",
                           "grant_id": GRANT}).to_string()}]})))
        .build();
    c.handle(denial, &sink_for(&tx), &mut db).await;
    let answered = drain(&mut rx);
    assert!(
        answered
            .iter()
            .any(|e| e["header"]["error_code"] == "credential_pending"),
        "{answered:?}"
    );

    // ── born natively without `base_url`: its own start row is no change ──
    let td2 = tempfile::TempDir::new().unwrap();
    let mut db2 = cell_db(&td2);
    let mut native = LlmCell::new(
        LlmParams::parse(&json!({
            "provider": "anthropic", "model": "claude-1273", "api_key": "",
            "credential_grant_id": GRANT,
            "x_title": "Example App", "http_referer": "https://example.com",
        }))
        .expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    );
    native
        .handle(
            push(json!({"model": "claude-1273", "provider": "anthropic",
                        "base_url": "https://API.anthropic.com/v1/"})),
            &sink_for(&tx),
            &mut db2,
        )
        .await;
    let seen = drain(&mut rx);
    assert!(
        seen.is_empty(),
        "the lane's default host is the birth host: {seen:?}"
    );
}
