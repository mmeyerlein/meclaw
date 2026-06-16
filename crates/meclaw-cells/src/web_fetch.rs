//! Phase-7 WebFetchCell. Stateless HTTP-GET tool cell.

use std::time::Duration;

use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_TIMEOUT, build_error_body, build_tool_result_body,
    parse_tool_call_args, with_external_timeout,
};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

/// Stateless HTTP-GET cell.
pub struct WebFetchCell {
    /// reqwest Client (Arc-intern, kein Mutex nötig).
    pub client: reqwest::Client,
    /// External-timeout pro Roundtrip (send + bytes).
    pub external_timeout: Duration,
    /// Max. Anzahl parallel laufender Workers für diese Cell.
    pub max_concurrency: usize,
}

#[derive(Debug)]
pub(crate) struct WebFetchArgs {
    pub url: String,
}

/// Parses tool_call args for WebFetchCell.
pub(crate) fn parse_web_fetch_args(args: &meclaw_core::JsonValue) -> Result<WebFetchArgs, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.url missing or not a string".to_string())?;
    if url.is_empty() {
        return Err("args.url is empty".into());
    }
    Ok(WebFetchArgs {
        url: url.to_string(),
    })
}

#[allow(clippy::manual_async_fn)]
impl meclaw_colony::StatelessCell for WebFetchCell {
    /// Handle one tool_call message: parse `{url}`, issue HTTP GET via
    /// the shared `reqwest::Client`, wrap the whole roundtrip in
    /// `with_external_timeout`, emit a `tool_result` with
    /// `http_status`/`content_type`/`bytes`-Headers. Non-2xx HTTP status
    /// codes are NORMAL tool_results; only DNS/connect/timeout produce
    /// error messages.
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            let (args, id) = match parse_tool_call_args(&msg) {
                Ok(v) => v,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, None, started)
                        .await;
                    return;
                }
            };
            let parsed = match parse_web_fetch_args(&args) {
                Ok(p) => p,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, id, started)
                        .await;
                    return;
                }
            };

            let client = self.client.clone();
            let url = parsed.url.clone();
            let result = with_external_timeout(self.external_timeout, async move {
                let resp = client.get(&url).send().await?;
                let status = resp.status().as_u16();
                let content_type = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let bytes = resp.bytes().await?;
                Ok::<_, reqwest::Error>((status, content_type, bytes.to_vec()))
            })
            .await;

            let duration_ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(Ok((status, content_type, body))) => {
                    let text = String::from_utf8_lossy(&body).into_owned();
                    let bytes_len = text.len() as u64;
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("web_fetch".into()));
                    header.insert("http_status".into(), Value::from(status));
                    header.insert("content_type".into(), Value::String(content_type));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    header.insert("bytes".into(), Value::from(bytes_len));
                    let body_json = build_tool_result_body(text, id, header);
                    tracing::info!(
                        operation = "web_fetch",
                        http_status = status,
                        duration_ms,
                        bytes = bytes_len,
                        "web_fetch ok"
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body_json,
                        })
                        .await;
                }
                Ok(Err(e)) => {
                    self.emit_error(sink, reply_target, ERR_IO_ERROR, e.to_string(), id, started)
                        .await;
                }
                Err(_timeout_msg) => {
                    self.emit_error(
                        sink,
                        reply_target,
                        ERR_TIMEOUT,
                        format!("web_fetch timed out after {:?}", self.external_timeout),
                        id,
                        started,
                    )
                    .await;
                }
            }
        }
    }
}

impl WebFetchCell {
    /// Emit a UBF error-body to `reply_target` with `operation: "web_fetch"`
    /// and the given `code`/`text`.
    async fn emit_error(
        &self,
        sink: &OutputSink,
        reply_target: Path,
        code: &str,
        text: String,
        id: Option<String>,
        started: std::time::Instant,
    ) {
        let duration_ms = started.elapsed().as_millis() as u64;
        let mut header = Map::new();
        header.insert("operation".into(), Value::String("web_fetch".into()));
        header.insert("duration_ms".into(), Value::from(duration_ms));
        let body = build_error_body(code, text, id, header);
        tracing::info!(
            operation = "web_fetch",
            error_code = code,
            duration_ms,
            "web_fetch err"
        );
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

// ---- WebFetchCellFactory ----

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};
use std::sync::Arc;

/// Factory for `WebFetchCell`. Unit struct — stateless, config lives in params.
pub struct WebFetchCellFactory;

const DEFAULT_WEB_FETCH_MAX_CONCURRENCY: usize = 32;
const DEFAULT_WEB_FETCH_EXTERNAL_TIMEOUT_MS: u64 = 30_000;

struct ParsedWebFetchParams {
    external_timeout: Duration,
    max_concurrency: usize,
}

fn parse_params_pure(raw: &meclaw_core::JsonValue) -> Result<ParsedWebFetchParams, String> {
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_WEB_FETCH_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    let ms = match raw.get("external_timeout_ms") {
        None => DEFAULT_WEB_FETCH_EXTERNAL_TIMEOUT_MS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.external_timeout_ms must be a positive integer".to_string())?,
    };
    if ms == 0 {
        return Err("params.external_timeout_ms must be >= 1".into());
    }
    Ok(ParsedWebFetchParams {
        external_timeout: Duration::from_millis(ms),
        max_concurrency: mc,
    })
}

impl CellFactory for WebFetchCellFactory {
    fn validate_params(&self, params: &meclaw_core::JsonValue) -> Result<(), String> {
        parse_params_pure(params).map(|_| ())
    }

    /// Stateless cell — `cell_dir` and the three Phase-13-G-1 substrate params
    /// (`colony_inbox_tx`, `idle_timeout`, `cell_timeout`) are unused.
    fn spawn_cell(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let parsed = parse_params_pure(&params)?;
        let external_timeout = parsed.external_timeout;
        let max_concurrency = parsed.max_concurrency;

        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;

        let cell = Arc::new(WebFetchCell {
            client: client.clone(),
            external_timeout,
            max_concurrency,
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
        // Phase-13.5 Lifecycle-3b Task 3 + P3-A4 funnel: initial dispatcher via
        // `build_stateless_task` (owns the peace-keep-alive; stateless → no
        // cell.db → death_ack on dispatcher task-end). RespawnFn passes
        // `colony_inbox = None`.
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
            path.clone(),
            rx,
            outputs_tx.clone(),
            cell,
            max_concurrency,
            message_timeout,
            Some(colony_inbox_tx.clone()),
            blob_store.clone(),
            contract.consumes.clone(),
        );

        let respawn_path = path.clone();
        let respawn_outputs_tx = outputs_tx.clone();
        let respawn_client = client.clone(); // Arc-clone — kein Rebuild, kein .expect()
        let respawn_blob = blob_store.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            let cell = Arc::new(WebFetchCell {
                client: respawn_client.clone(),
                external_timeout,
                max_concurrency,
            });
            let (tx, rx) =
                tokio::sync::mpsc::channel::<meclaw_core::Message>(respawn_mailbox_capacity);
            let p = respawn_path.clone();
            let o = respawn_outputs_tx.clone();
            let b = respawn_blob.clone();
            // Stateless respawn is intentionally bare (no renotify, colony_inbox
            // = None). Dropping stop_tx/death_ack_rx is behaviorally identical to
            // the old bare `None,None,None` spawn (stop-fut parks, death_ack
            // unobserved). Peace-keep-alive lives in the helper.
            let (join, peace_rx, _stop_tx, _death_ack_rx, backstop_rx) = build_stateless_task(
                p,
                rx,
                o,
                cell,
                max_concurrency,
                message_timeout,
                None,
                b,
                respawn_consumes.clone(),
            );
            (tx, join, peace_rx, backstop_rx)
        });

        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    /// Paket-8: boot-inactive eager respawn (No-Delete reconnect-after-reboot).
    /// Builds the SAME `Arc<WebFetchCell>` as `spawn_cell` (incl. the fresh
    /// reqwest client) and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` when
    /// params no longer parse OR the reqwest client fails to build.
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let parsed = parse_params_pure(&params).ok()?;
        let max_concurrency = parsed.max_concurrency;
        let client = reqwest::Client::builder().build().ok()?;
        let cell = Arc::new(WebFetchCell {
            client,
            external_timeout: parsed.external_timeout,
            max_concurrency,
        });
        Some(meclaw_colony::build_stateless_boot_inactive_respawn(
            path,
            outputs_tx,
            cell,
            max_concurrency,
            message_timeout,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_web_fetch_args_happy_path() {
        let args = parse_web_fetch_args(&json!({"url": "http://127.0.0.1:1/foo"})).unwrap();
        assert_eq!(args.url, "http://127.0.0.1:1/foo");
    }

    #[test]
    fn parse_web_fetch_args_rejects_missing_url() {
        assert!(parse_web_fetch_args(&json!({})).is_err());
    }

    #[test]
    fn parse_web_fetch_args_rejects_non_string_url() {
        assert!(parse_web_fetch_args(&json!({"url": 42})).is_err());
    }

    #[test]
    fn parse_web_fetch_args_rejects_empty_url() {
        assert!(parse_web_fetch_args(&json!({"url": ""})).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_get_200_emits_tool_result_with_http_status() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::ok(b"hello world")).await;
        let cell = WebFetchCell {
            client: reqwest::Client::builder().build().unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
        };

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let url = format!("http://{addr}/foo");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-1"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "hello world");
        assert_eq!(em.content["header"]["operation"], "web_fetch");
        assert_eq!(em.content["header"]["http_status"], 200);
        assert_eq!(em.content["header"]["bytes"], 11);
        let ct = em.content["header"]["content_type"].as_str().unwrap();
        assert!(ct.starts_with("text/plain"), "content_type was {ct}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_get_404_is_normal_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::not_found()).await;
        let cell = WebFetchCell {
            client: reqwest::Client::builder().build().unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let url = format!("http://{addr}/nope");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-2"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["http_status"], 404);
        // 404 ist NORMAL — KEIN finish_reason=error (Decision 3.8).
        assert!(
            em.content["header"].get("finish_reason").is_none()
                || em.content["header"]["finish_reason"] != "error"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_connect_failure_emits_io_error() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        // 127.0.0.1:1 — Port 1 ist nicht reserviert, vermutlich kein Listener → connect refused.
        let cell = WebFetchCell {
            client: reqwest::Client::builder().build().unwrap(),
            external_timeout: std::time::Duration::from_secs(2),
            max_concurrency: 4,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"url":"http://127.0.0.1:1/x"}"#, "id": "call-3"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "io_error");
    }

    // ---- T4: WebFetchCellFactory tests ----

    #[test]
    fn factory_validate_params_accepts_empty_object() {
        use meclaw_colony::CellFactory;
        assert!(
            WebFetchCellFactory
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_ok()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_concurrency_zero() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory
            .validate_params(&meclaw_core::serde_json::json!({"max_concurrency": 0}));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_rejects_external_timeout_zero() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory
            .validate_params(&meclaw_core::serde_json::json!({"external_timeout_ms": 0}));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_accepts_valid_overrides() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory.validate_params(&meclaw_core::serde_json::json!({
            "max_concurrency": 8, "external_timeout_ms": 5000
        }));
        assert!(r.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_spawn_cell_routes_message_to_tool_result() {
        use meclaw_colony::CellFactory;
        use meclaw_core::{Body, CellEmission, MessageBuilder, Path, serde_json::json};
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::ok(b"factory ok")).await;
        let factory: Arc<dyn CellFactory> = Arc::new(WebFetchCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/web"),
                json!({"max_concurrency": 2, "external_timeout_ms": 5000}),
                out_tx,
                std::path::PathBuf::new(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .expect("spawn");

        let url = format!("http://{addr}/x");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-f"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();

        // Deterministisches Rendezvous: recv().await returnt sobald der Worker
        // die Emission in out_tx schreibt. Kein zeitbasierter Failure-Marker —
        // Channel-Close (None) würde den Test mit unwrap() explodieren lassen,
        // was ein echter Failure wäre, kein Flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "factory ok");
        assert_eq!(em.content["header"]["operation"], "web_fetch");
        assert_eq!(em.content["header"]["http_status"], 200);

        drop(sender);
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn web_fetch_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let factory = Arc::new(WebFetchCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let hook = factory.build_boot_inactive_respawn(
            meclaw_core::Path::new("/c"),
            json!({"max_concurrency": 8, "external_timeout_ms": 5000}),
            out_tx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            itx,
            None, // idle_timeout
            0,    // cell_timeout
            None, // message_timeout
            None, // blob_store
            1000, // mailbox_capacity
        );
        assert!(
            hook.is_some(),
            "stateless web_fetch factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
