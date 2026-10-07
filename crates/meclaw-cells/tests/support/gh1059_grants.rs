//! GH #1059: shared helpers of the long-running grant locks (proxy Telegram,
//! proxy Slack, voice). Include with
//! `#[path = "support/gh1059_grants.rs"] mod gh1059_grants;`.
//!
//! - [`LogCapture`]: a GLOBAL tracing subscriber (installed once per test
//!   binary; nextest runs one test per process). The cells under test run on
//!   worker threads of a multi-thread runtime, where a thread-local
//!   `set_default` would see nothing. Every line keeps level, target, message
//!   AND the formatted field values — the "no secret in the log" scan must see
//!   the values. A std mutex in a TEST collector; the no-lock rule is about
//!   cell state.
//! - delivery helpers: read the recipient out of a `credential_request`, seal
//!   a stub secret to it, build the message `access/invoke` would deliver.

#![allow(dead_code)]

use meclaw_cells::sealed::{RecipientKeypair, seal_to};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// One captured event: level, target, message, `name=value` of every field.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: Vec<String>,
}

impl LogLine {
    /// Everything the line prints, for a substring scan.
    pub fn text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.level,
            self.target,
            self.message,
            self.fields.join(" ")
        )
    }
}

#[derive(Clone, Default)]
pub struct LogCapture(Arc<Mutex<Vec<LogLine>>>);

impl LogCapture {
    /// The process-wide capture (installed on first use).
    pub fn global() -> LogCapture {
        static CAPTURE: OnceLock<LogCapture> = OnceLock::new();
        CAPTURE
            .get_or_init(|| {
                let c = LogCapture::default();
                tracing::subscriber::set_global_default(c.clone())
                    .expect("one global subscriber per test binary");
                c
            })
            .clone()
    }

    pub fn lines(&self) -> Vec<LogLine> {
        self.0.lock().unwrap().clone()
    }

    /// Lines whose text contains `needle`.
    pub fn containing(&self, needle: &str) -> Vec<LogLine> {
        self.lines()
            .into_iter()
            .filter(|l| l.text().contains(needle))
            .collect()
    }
}

struct Fields {
    message: String,
    fields: Vec<String>,
}

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl tracing::Subscriber for LogCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut f = Fields {
            message: String::new(),
            fields: Vec::new(),
        };
        event.record(&mut f);
        let meta = event.metadata();
        self.0.lock().unwrap().push(LogLine {
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: f.message,
            fields: f.fields,
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Is `content` a `credential_request` (for `grant`, if given)?
pub fn is_request(content: &Value, grant: Option<&str>) -> bool {
    content["header"]["route"] == "credential_request"
        && grant.is_none_or(|g| content["header"]["grant_id"] == g)
}

/// The recipient key a request carries.
pub fn recipient_of(request: &Value) -> String {
    let args: Value =
        meclaw_core::serde_json::from_str(request["messages"][0]["text"].as_str().unwrap())
            .unwrap();
    args["payload"]["recipient_key"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The delivery `access/invoke` sends back: the broker's ack (grant id, the
/// request's call id) plus the box sealed to the request's recipient.
pub fn delivery(grant: &str, request: &Value, secret: &str) -> Value {
    json!({
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": request["messages"][0]["id"],
                      "text": json!({"outcome": "ok", "grant_id": grant,
                                     "operation": "vault.deliver"}).to_string()}],
        "sealed": seal_to(&recipient_of(request), secret.as_bytes()).unwrap().to_json(),
    })
}

/// A delivery sealed to a STRANGER's key — a box this cell cannot open.
pub fn foreign_delivery(grant: &str, request: &Value, secret: &str) -> Value {
    let stranger = RecipientKeypair::generate().unwrap();
    json!({
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": request["messages"][0]["id"],
                      "text": json!({"outcome": "ok", "grant_id": grant,
                                     "operation": "vault.deliver"}).to_string()}],
        "sealed": seal_to(&stranger.public_hex(), secret.as_bytes()).unwrap().to_json(),
    })
}

/// Wrap a body as a mailbox message to `target`.
pub fn message(target: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(body))
        .build()
}

/// Wait (failure marker 30 s) for the next `credential_request` among the
/// emissions; other emissions are skipped.
pub async fn next_request(rx: &mut mpsc::Receiver<CellEmission>, grant: Option<&str>) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let em = rx.recv().await.expect("the cell is gone");
            if is_request(&em.content, grant) {
                return em.content;
            }
        }
    })
    .await
    .expect("no credential_request within 30 s")
}

/// Every `credential_request` that arrives within `window` (non-requests are
/// dropped).
pub async fn requests_within(
    rx: &mut mpsc::Receiver<CellEmission>,
    window: Duration,
    grant: Option<&str>,
) -> Vec<Value> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(em)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        if is_request(&em.content, grant) {
            out.push(em.content);
        }
    }
    out
}

/// Wait (failure marker 30 s) until `pred` holds, checking every 20 ms.
pub async fn eventually(what: &str, mut pred: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !pred() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A copy of what the capturing stub saw. The stub's log is a tokio mutex
/// (`meclaw_testing::mock_http`); the helpers that read it run in sync
/// closures (`eventually`), so they spin on `try_lock` — the stub holds it
/// only for one push, on another worker thread.
pub fn snapshot(
    seen: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
) -> Vec<meclaw_testing::mock_http::CapturedRequest> {
    loop {
        if let Ok(guard) = seen.try_lock() {
            return guard.clone();
        }
        std::thread::yield_now();
    }
}
