//! Phase-7 WebSearchCell. Detail in T7.

use std::time::Duration;

/// Stateless HTTP search-proxy cell (generic JSON wrapper).
pub struct WebSearchCell {
    /// reqwest client (Arc internally, no mutex needed).
    pub client: reqwest::Client,
    /// Search-endpoint URL (e.g. `https://api.search.example/search`).
    pub endpoint: String,
    /// Optional Bearer-token for the search API. `None` = no `Authorization`
    /// header at all; an empty configured value is `None` (GH #270).
    pub api_key: Option<String>,
    /// External-timeout pro Roundtrip (send + bytes).
    pub external_timeout: Duration,
    /// Max number of workers running in parallel for this cell.
    pub max_concurrency: usize,
    /// Cap on the number of results handed to the caller (GH #83).
    ///
    /// A search result is a tool result, and inside a tool loop a tool result
    /// is re-sent to the model on every subsequent round. A conforming provider
    /// list longer than this is trimmed in place; the JSON stays valid and
    /// carries the cut visibly (`"truncated": true`, `"total_results": N`),
    /// because the hop header does not travel into the thread row the model
    /// reads. `header.result_count` keeps the full provider count.
    pub max_results: usize,
    /// Byte backstop on the outgoing `text` (GH #83) — same convention as
    /// `web_fetch`. It catches what the list cap cannot: a non-conforming
    /// pass-through body, or a conforming list with absurdly large snippets.
    pub max_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct WebSearchArgs {
    pub query: String,
}

use meclaw_core::JsonValue;

/// Parses tool_call args for WebSearchCell.
pub(crate) fn parse_web_search_args(args: &JsonValue) -> Result<WebSearchArgs, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.query missing or not a string".to_string())?;
    if query.is_empty() {
        return Err("args.query is empty".into());
    }
    Ok(WebSearchArgs {
        query: query.to_string(),
    })
}

use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_TIMEOUT, build_error_body, build_tool_result_body,
    parse_tool_call_args, with_external_timeout,
};
use meclaw_core::serde_json::{self, Map, Value};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

#[allow(clippy::manual_async_fn)]
impl meclaw_colony::StatelessCell for WebSearchCell {
    /// Dispatches a `web_search` tool call: parses args, performs the HTTP GET, and emits the
    /// result (or a structured error) to the `reply_to` path.
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
            let parsed = match parse_web_search_args(&args) {
                Ok(p) => p,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, id, started)
                        .await;
                    return;
                }
            };

            let client = self.client.clone();
            let endpoint = self.endpoint.clone();
            let api_key = self.api_key.clone();
            let query = parsed.query.clone();
            let result = with_external_timeout(self.external_timeout, async move {
                let mut req = client.get(&endpoint).query(&[("q", &query)]);
                if let Some(k) = api_key {
                    req = req.bearer_auth(k);
                }
                let resp = req.send().await?;
                let bytes = resp.bytes().await?;
                Ok::<_, reqwest::Error>(bytes.to_vec())
            })
            .await;

            let duration_ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(Ok(body)) => {
                    let text = String::from_utf8_lossy(&body).into_owned();
                    // GH #83: `bytes` reports what the provider sent, so a
                    // trimmed reply still says how big the response really was.
                    let bytes_len = text.len() as u64;
                    // Graceful: derive result_count from the results array,
                    // otherwise 0. `result_count` is the FULL provider count —
                    // GH #83 trims the delivered list, not the bookkeeping.
                    let mut result_count = 0u64;
                    let mut text = text;
                    let mut truncated = false;
                    if let Ok(mut v) = serde_json::from_str::<Value>(&text) {
                        let full_len = v.get("results").and_then(|r| r.as_array()).map(|a| a.len());
                        if let Some(n) = full_len {
                            result_count = n as u64;
                            if n > self.max_results {
                                // GH #83: trim the list in place; the JSON stays
                                // valid and names the cut where the model reads.
                                if let Some(arr) =
                                    v.get_mut("results").and_then(|r| r.as_array_mut())
                                {
                                    arr.truncate(self.max_results);
                                }
                                if let Some(obj) = v.as_object_mut() {
                                    obj.insert("truncated".into(), Value::Bool(true));
                                    obj.insert("total_results".into(), Value::from(n as u64));
                                }
                                text = v.to_string();
                                truncated = true;
                            }
                        }
                    }
                    // GH #83 byte backstop, same convention as web_fetch:
                    // catches the non-conforming pass-through body too.
                    let (text, byte_cut) = crate::web_fetch::truncate_body(text, self.max_bytes);
                    let truncated = truncated || byte_cut;
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("web_search".into()));
                    header.insert("result_count".into(), Value::from(result_count));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    header.insert("bytes".into(), Value::from(bytes_len));
                    if truncated {
                        header.insert("truncated".into(), Value::Bool(true));
                    }
                    let body_json = build_tool_result_body(text, id, header);
                    tracing::info!(
                        operation = "web_search",
                        result_count,
                        duration_ms,
                        truncated,
                        "web_search ok"
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
                        format!("web_search timed out after {:?}", self.external_timeout),
                        id,
                        started,
                    )
                    .await;
                }
            }
        }
    }
}

impl WebSearchCell {
    /// Builds and pushes a structured error body to `reply_to`; logs the failure via `tracing`.
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
        header.insert("operation".into(), Value::String("web_search".into()));
        header.insert("duration_ms".into(), Value::from(duration_ms));
        let body = build_error_body(code, text, id, header);
        tracing::info!(
            operation = "web_search",
            error_code = code,
            duration_ms,
            "web_search err"
        );
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

// ---- WebSearchCellFactory ----

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};
use std::sync::Arc;

/// Factory for `WebSearchCell`. Unit struct — stateless, all config lives in params.
///
/// Required param: `endpoint` (non-empty string).
/// Optional params: `api_key` (string; empty = no `Authorization` header,
/// GH #270), `max_concurrency` (≥1, default 8),
/// `external_timeout_ms` (≥1, default 15 000).
pub struct WebSearchCellFactory;

const DEFAULT_WEB_SEARCH_MAX_CONCURRENCY: usize = 8;
const DEFAULT_WEB_SEARCH_EXTERNAL_TIMEOUT_MS: u64 = 15_000;
/// Default result-list cap: 10 (GH #83) — a full first page. Search providers
/// rarely return more by default, and title+url+snippet keeps the tool result
/// at a few KB per search, which an agent loop can afford on every round.
const DEFAULT_WEB_SEARCH_MAX_RESULTS: usize = 10;
/// Default byte backstop: 256 KiB (GH #83) — the same generous-but-finite
/// value `web_fetch` and `bash` use, so the tool cells share one default.
const DEFAULT_WEB_SEARCH_MAX_BYTES: usize = 256 * 1024;

struct ParsedWebSearchParams {
    endpoint: String,
    api_key: Option<String>,
    external_timeout: Duration,
    max_concurrency: usize,
    max_results: usize,
    max_bytes: usize,
}

/// Parses and validates `spawn_cell` / `validate_params` params for `WebSearchCellFactory`.
///
/// Enforces: `endpoint` present + non-empty, `max_concurrency` ≥ 1, `external_timeout_ms` ≥ 1.
fn parse_params_pure(raw: &meclaw_core::JsonValue) -> Result<ParsedWebSearchParams, String> {
    let endpoint = raw
        .get("endpoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "params.endpoint missing or not a string".to_string())?;
    if endpoint.is_empty() {
        return Err("params.endpoint is empty".into());
    }
    // An empty api_key is no api_key (GH #270), the same repair `mcp`'s
    // `parse_http` carries since GH #268. `api_key` is written as
    // `${SEARCH_API_KEY:-}` wherever the operator may leave the variable
    // unset — the shipped `.env.example` even ships it set to empty — and the
    // substitution turns that into `""`. `Some("")` is not `None`, so without
    // this filter every search went out carrying `Authorization: Bearer ` with
    // nothing after it. Against an endpoint that would have answered
    // anonymously that header can be a flat rejection, which then reads as a
    // search backend being down rather than as a credential nobody set. Every
    // declaration in the tree describes the empty value as an absent token.
    let api_key = raw
        .get("api_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_WEB_SEARCH_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    let ms = match raw.get("external_timeout_ms") {
        None => DEFAULT_WEB_SEARCH_EXTERNAL_TIMEOUT_MS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.external_timeout_ms must be a positive integer".to_string())?,
    };
    if ms == 0 {
        return Err("params.external_timeout_ms must be >= 1".into());
    }
    let mr = match raw.get("max_results") {
        None => DEFAULT_WEB_SEARCH_MAX_RESULTS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_results must be a positive integer".to_string())?
            as usize,
    };
    if mr == 0 {
        return Err("params.max_results must be >= 1".into());
    }
    let mb = match raw.get("max_bytes") {
        None => DEFAULT_WEB_SEARCH_MAX_BYTES,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_bytes must be a positive integer".to_string())?
            as usize,
    };
    if mb == 0 {
        return Err("params.max_bytes must be >= 1".into());
    }
    Ok(ParsedWebSearchParams {
        endpoint: endpoint.to_string(),
        api_key,
        external_timeout: Duration::from_millis(ms),
        max_concurrency: mc,
        max_results: mr,
        max_bytes: mb,
    })
}

impl CellFactory for WebSearchCellFactory {
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
        let endpoint = parsed.endpoint;
        let api_key = parsed.api_key;
        let external_timeout = parsed.external_timeout;
        let max_concurrency = parsed.max_concurrency;
        let max_results = parsed.max_results;
        let max_bytes = parsed.max_bytes;

        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;

        let cell = Arc::new(WebSearchCell {
            client: client.clone(),
            endpoint: endpoint.clone(),
            api_key: api_key.clone(),
            external_timeout,
            max_concurrency,
            max_results,
            max_bytes,
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
        let respawn_endpoint = endpoint.clone();
        let respawn_api_key = api_key.clone();
        let respawn_client = client.clone(); // Arc clone — no rebuild, no .expect()
        let respawn_blob = blob_store.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            let cell = Arc::new(WebSearchCell {
                client: respawn_client.clone(),
                endpoint: respawn_endpoint.clone(),
                api_key: respawn_api_key.clone(),
                external_timeout,
                max_concurrency,
                max_results,
                max_bytes,
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
    /// Builds the SAME `Arc<WebSearchCell>` as `spawn_cell` (incl. the fresh
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
        let cell = Arc::new(WebSearchCell {
            client,
            endpoint: parsed.endpoint,
            api_key: parsed.api_key,
            external_timeout: parsed.external_timeout,
            max_concurrency,
            max_results: parsed.max_results,
            max_bytes: parsed.max_bytes,
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
    fn parse_web_search_args_happy_path() {
        let args = parse_web_search_args(&json!({"query": "rust async"})).unwrap();
        assert_eq!(args.query, "rust async");
    }

    #[test]
    fn parse_web_search_args_rejects_missing_query() {
        assert!(parse_web_search_args(&json!({})).is_err());
    }

    #[test]
    fn parse_web_search_args_rejects_non_string_query() {
        assert!(parse_web_search_args(&json!({"query": 42})).is_err());
    }

    #[test]
    fn parse_web_search_args_rejects_empty_query() {
        assert!(parse_web_search_args(&json!({"query": ""})).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_search_with_results_emits_result_count() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let json_body = br#"{"results":[{"title":"A","url":"u","snippet":"s"},{"title":"B","url":"u2","snippet":"s2"}]}"#;
        let (addr, _join) = start_mock_server(MockResponse::ok_json(json_body)).await;
        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: format!("http://{addr}/search"),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            max_bytes: DEFAULT_WEB_SEARCH_MAX_BYTES,
        };

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"rust async"}"#, "id": "call-1"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["header"]["operation"], "web_search");
        assert_eq!(em.content["header"]["result_count"], 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_search_with_non_conforming_response_is_graceful() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        // Non-conforming JSON: has no "results" array.
        let (addr, _join) = start_mock_server(MockResponse::ok_json(br#"{"hits":[1,2,3]}"#)).await;
        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: format!("http://{addr}/search"),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            max_bytes: DEFAULT_WEB_SEARCH_MAX_BYTES,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"x"}"#, "id": "call-2"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        // GRACEFUL: result_count=0, NO error. The body is passed through in text.
        assert_eq!(em.content["header"]["result_count"], 0);
        assert!(
            em.content["header"].get("finish_reason").is_none()
                || em.content["header"]["finish_reason"] != "error"
        );
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("hits"),
            "body must be passed through, got {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_search_connect_failure_emits_io_error() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: "http://127.0.0.1:1/search".into(),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(2),
            max_concurrency: 4,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            max_bytes: DEFAULT_WEB_SEARCH_MAX_BYTES,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"x"}"#, "id": "call-3"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "io_error");
    }

    // ---- T8: WebSearchCellFactory tests ----

    #[test]
    fn factory_validate_params_rejects_missing_endpoint() {
        use meclaw_colony::CellFactory;
        assert!(
            WebSearchCellFactory
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_rejects_empty_endpoint() {
        use meclaw_colony::CellFactory;
        assert!(
            WebSearchCellFactory
                .validate_params(&meclaw_core::serde_json::json!({"endpoint": ""}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_accepts_endpoint_only() {
        use meclaw_colony::CellFactory;
        assert!(
            WebSearchCellFactory
                .validate_params(
                    &meclaw_core::serde_json::json!({"endpoint": "http://localhost/search"})
                )
                .is_ok()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_concurrency_zero() {
        use meclaw_colony::CellFactory;
        assert!(WebSearchCellFactory
            .validate_params(
                &meclaw_core::serde_json::json!({"endpoint": "http://x/s", "max_concurrency": 0})
            )
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_spawn_cell_routes_message_to_tool_result() {
        use meclaw_colony::CellFactory;
        use meclaw_core::{Body, CellEmission, MessageBuilder, Path, serde_json::json};
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let json_body = br#"{"results":[{"title":"X","url":"u","snippet":"s"}]}"#;
        let (addr, _server) = start_mock_server(MockResponse::ok_json(json_body)).await;
        let factory: Arc<dyn CellFactory> = Arc::new(WebSearchCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/search"),
                json!({
                    "endpoint": format!("http://{addr}/search"),
                    "max_concurrency": 2,
                    "external_timeout_ms": 5000
                }),
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

        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"test"}"#, "id": "call-f"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();

        // Deterministic rendezvous: recv().await returns as soon as the worker
        // writes the emission into out_tx. No time-based failure marker — a
        // channel close (None) would blow the test up on unwrap(), which would be
        // a real failure, not a flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["header"]["operation"], "web_search");
        assert_eq!(em.content["header"]["result_count"], 1);

        drop(sender);
        join.await.unwrap();
    }

    // ───── GH #83: the result-list cap ─────

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_long_result_list_arrives_trimmed_and_marked() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let results: Vec<Value> = (0..5)
            .map(|i| json!({"title": format!("t{i}"), "url": format!("u{i}"), "snippet": "s"}))
            .collect();
        let body = serde_json::to_vec(&json!({"results": results})).unwrap();
        let (addr, _join) = start_mock_server(MockResponse::ok_json(&body)).await;
        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: format!("http://{addr}/search"),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_results: 2,
            max_bytes: DEFAULT_WEB_SEARCH_MAX_BYTES,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"x"}"#, "id": "call-cap"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).expect("trimmed text stays valid JSON");
        assert_eq!(
            v["results"].as_array().unwrap().len(),
            2,
            "the list is cut to max_results"
        );
        assert_eq!(
            v["truncated"], true,
            "the cut is visible IN the JSON the model reads"
        );
        assert_eq!(v["total_results"], 5, "and the full count is named there");
        assert_eq!(
            em.content["header"]["truncated"], true,
            "the hop header says it too"
        );
        assert_eq!(
            em.content["header"]["result_count"], 5,
            "`result_count` reports what the provider sent, not what survived"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_result_list_under_the_cap_passes_through_byte_identical() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let json_body = br#"{"results":[{"title":"A","url":"u","snippet":"s"}]}"#;
        let (addr, _join) = start_mock_server(MockResponse::ok_json(json_body)).await;
        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: format!("http://{addr}/search"),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            max_bytes: DEFAULT_WEB_SEARCH_MAX_BYTES,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"x"}"#, "id": "call-u"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(
            em.content["messages"][0]["text"].as_str().unwrap(),
            std::str::from_utf8(json_body).unwrap(),
            "no trim, no re-serialization — the provider body passes through untouched"
        );
        assert!(
            em.content["header"].get("truncated").is_none(),
            "no cut, no marker"
        );
        assert_eq!(em.content["header"]["result_count"], 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_huge_non_conforming_body_hits_the_byte_backstop() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        // Not JSON at all — the graceful pass-through path, 20,000 bytes long.
        let huge = "z".repeat(20_000);
        let (addr, _join) = start_mock_server(MockResponse::ok(huge.as_bytes())).await;
        let cell = WebSearchCell {
            client: reqwest::Client::builder().build().unwrap(),
            endpoint: format!("http://{addr}/search"),
            api_key: None,
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            max_bytes: 1000,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/search"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/search"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"query":"x"}"#, "id": "call-b"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.len() < 20_000,
            "the backstop must bite: {} bytes arrived",
            text.len()
        );
        assert!(
            text.contains("[truncated, 20000 bytes total]"),
            "cut marked"
        );
        assert_eq!(em.content["header"]["truncated"], true);
        assert_eq!(
            em.content["header"]["bytes"], 20_000,
            "`bytes` reports what the provider sent"
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_results_zero() {
        use meclaw_colony::CellFactory;
        assert!(
            WebSearchCellFactory
                .validate_params(
                    &meclaw_core::serde_json::json!({"endpoint": "http://x/s", "max_results": 0})
                )
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_bytes_zero() {
        use meclaw_colony::CellFactory;
        assert!(
            WebSearchCellFactory
                .validate_params(
                    &meclaw_core::serde_json::json!({"endpoint": "http://x/s", "max_bytes": 0})
                )
                .is_err()
        );
    }

    #[test]
    fn factory_defaults_the_two_caps() {
        let p =
            parse_params_pure(&meclaw_core::serde_json::json!({"endpoint": "http://x/s"})).unwrap();
        assert_eq!(p.max_results, DEFAULT_WEB_SEARCH_MAX_RESULTS);
        assert_eq!(p.max_bytes, DEFAULT_WEB_SEARCH_MAX_BYTES);
        assert_eq!(
            DEFAULT_WEB_SEARCH_MAX_BYTES,
            256 * 1024,
            "one consistent byte default across the tool cells"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn web_search_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let factory = Arc::new(WebSearchCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let hook = factory.build_boot_inactive_respawn(
            meclaw_core::Path::new("/c"),
            json!({"endpoint": "http://localhost/search", "max_concurrency": 2, "external_timeout_ms": 5000}),
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
            "stateless web_search factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
