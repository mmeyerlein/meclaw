//! Phase-8 LlmCell output composition.
//!
//! Two async helpers used by the `handle()`-orchestrator (T18..T22):
//!
//! - `emit_assistant_turn` — success path. Pushes a UBF body with the assistant
//!   turn(s), full `meta` (provider/model/response_id/latency_ms/started_at)
//!   and `header` (finish_reason/model plus the whole usage block:
//!   tokens_prompt/tokens_completion/tokens_cached/cost/latency_ms).
//! - `emit_error` — error path. Gate-1 pass-through: `messages` is the caller's
//!   `input_messages` UNCHANGED so that failover-edges-on-finish_reason="error"
//!   can route the original conversation to a backup llm-cell. `meta.error.*`
//!   carries source+detail; optional `meta.model`/`meta.response_id` only-if-Some.
//!
//! The body is shaped per Plan § 10: cell-emitted `content` carries a nested
//! `"header"` object; Colony extracts `content.header` into `message.headers`
//! via the Phase-3b mechanism.

use crate::llm::translate::TranslatedResponse;
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, OutputSink, Path};

/// The usage block of one provider call, as it travels into the hop header.
///
/// GH #463: before this struct, `tokens_cached` and the provider's own `cost`
/// were parsed off the wire and then dropped, and `latency_ms` lived only in
/// `meta` — which `/colony/ledger` never reads. A watcher therefore had the
/// numbers in the log and no way to sum them. Every field here is `Option`
/// and every `None` is omitted from the header: a figure the provider did not
/// report must not arrive downstream as a measured zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HopUsage {
    /// `usage.prompt_tokens` / `usage.input_tokens`.
    pub(crate) tokens_prompt: Option<u64>,
    /// `usage.completion_tokens` / `usage.output_tokens`.
    pub(crate) tokens_completion: Option<u64>,
    /// Cache-read tokens, in whichever spelling the provider used.
    pub(crate) tokens_cached: Option<u64>,
    /// The provider's own cost figure for the call. Never computed here.
    pub(crate) cost: Option<f64>,
}

impl HopUsage {
    /// Lift the usage figures out of a parsed provider response.
    pub(crate) fn of(t: &TranslatedResponse) -> Self {
        Self {
            tokens_prompt: t.tokens_prompt,
            tokens_completion: t.tokens_completion,
            tokens_cached: t.tokens_cached,
            cost: t.cost,
        }
    }

    /// Write every reported figure into a hop header. Absent figures are
    /// absent keys, never zeroes.
    fn write_into(&self, header: &mut Map<String, Value>) {
        if let Some(t) = self.tokens_prompt {
            header.insert("tokens_prompt".into(), Value::from(t));
        }
        if let Some(t) = self.tokens_completion {
            header.insert("tokens_completion".into(), Value::from(t));
        }
        if let Some(t) = self.tokens_cached {
            header.insert("tokens_cached".into(), Value::from(t));
        }
        // `Value::from(f64)` yields `Null` for a non-finite float, and a null
        // in a summed header column is worse than a missing key: SQLite would
        // read it as "present, unreadable" rather than "not reported".
        if let Some(c) = self.cost.filter(|c| c.is_finite()) {
            header.insert("cost".into(), Value::from(c));
        }
    }
}

/// Success-path emission: build UBF body with the assistant turn(s), full `meta`
/// and nested `header`, then push via the per-message `OutputSink`.
///
/// `assistant_turns` is the result of `translate::parse_openai_response`
/// (typically one `{origin:"assistant", type:"text", text:...}` turn, or a
/// `tool_call` turn).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_assistant_turn(
    sink: &OutputSink,
    target: Path,
    assistant_turns: Vec<Value>,
    finish_reason: &str,
    usage: HopUsage,
    model: &str,
    response_id: &str,
    started_at_unix_ms: i64,
    latency_ms: u64,
) {
    let mut header = Map::new();
    header.insert(
        "finish_reason".into(),
        Value::String(finish_reason.to_string()),
    );
    usage.write_into(&mut header);
    // GH #463: the measured wall time belongs in the header, because that is
    // the compartment `/colony/ledger` sums over. It STAYS in `meta` as well —
    // dropping it there would break every reader that has been reading it
    // since Phase 8, and one number in two places is cheaper than a migration.
    header.insert("latency_ms".into(), Value::from(latency_ms));
    header.insert("model".into(), Value::String(model.to_string()));

    let mut meta = Map::new();
    meta.insert("provider".into(), Value::String("openai".into()));
    meta.insert("model".into(), Value::String(model.to_string()));
    meta.insert("response_id".into(), Value::String(response_id.to_string()));
    meta.insert("latency_ms".into(), Value::from(latency_ms));
    meta.insert("started_at".into(), Value::from(started_at_unix_ms));

    let mut body = Map::new();
    body.insert("messages".into(), Value::Array(assistant_turns));
    body.insert("meta".into(), Value::Object(meta));
    body.insert("header".into(), Value::Object(header));

    let _ = sink
        .push(CellOutput {
            target,
            content: Value::Object(body),
        })
        .await;
}

/// Error-path emission (Gate-1 final): `messages` is `input_messages` UNCHANGED
/// so failover edges keyed on `finish_reason="error"` can route the original
/// conversation to a backup llm-cell. `header.error_code` carries the typed
/// code, `meta.error.{source,detail}` the diagnostic detail.
///
/// Sonderfall: empty `input_messages` is allowed for the parse-fail case (the
/// caller's input body was rejected before any messages could be extracted —
/// then `error_source = "parse"`).
///
/// `response_model` / `response_id` are `Some` only when the HTTP roundtrip
/// got far enough to surface them (e.g. parse-fail after a 200 OK).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_error(
    sink: &OutputSink,
    target: Path,
    error_code: &str,
    detail: &str,
    error_source: &str,
    input_messages: Vec<Value>,
    started_at_unix_ms: i64,
    latency_ms: u64,
    response_model: Option<&str>,
    response_id: Option<&str>,
    extra_error_meta: Option<Map<String, Value>>,
) {
    let mut header = Map::new();
    header.insert("finish_reason".into(), Value::String("error".into()));
    header.insert("error_code".into(), Value::String(error_code.to_string()));
    // GH #463: a failed call took time too, and a timeout is the one hop whose
    // latency an operator most wants summed. The error path carries no usage
    // block — a call that failed reports no tokens and no cost.
    header.insert("latency_ms".into(), Value::from(latency_ms));

    let mut error_obj = Map::new();
    error_obj.insert("source".into(), Value::String(error_source.to_string()));
    error_obj.insert("detail".into(), Value::String(detail.to_string()));
    // P10 (plan D10): the fine-grained failure kind lives here, because the
    // spec's `error_code` enum is closed. Pre-P10 call sites pass `None`, so
    // their emitted body stays byte-identical.
    if let Some(extra) = extra_error_meta {
        for (k, v) in extra {
            error_obj.insert(k, v);
        }
    }

    let mut meta = Map::new();
    meta.insert("provider".into(), Value::String("openai".into()));
    meta.insert("latency_ms".into(), Value::from(latency_ms));
    meta.insert("started_at".into(), Value::from(started_at_unix_ms));
    meta.insert("error".into(), Value::Object(error_obj));
    if let Some(m) = response_model {
        meta.insert("model".into(), Value::String(m.to_string()));
    }
    if let Some(rid) = response_id {
        meta.insert("response_id".into(), Value::String(rid.to_string()));
    }

    let mut body = Map::new();
    body.insert("messages".into(), Value::Array(input_messages));
    body.insert("meta".into(), Value::Object(meta));
    body.insert("header".into(), Value::Object(header));

    let _ = sink
        .push(CellOutput {
            target,
            content: Value::Object(body),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use meclaw_core::{CellEmission, OutputSink, Path, Uuid};
    use tokio::sync::mpsc;

    fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
        let (tx, rx) = mpsc::channel(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/llm"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            32,
            meclaw_core::Headers::new(),
            None,
        );
        (sink, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_assistant_turn_pushes_message_and_meta() {
        let (sink, mut rx) = mk_sink();
        let turns = vec![json!({"origin":"assistant","type":"text","text":"hi"})];
        emit_assistant_turn(
            &sink,
            Path::new("/sink"),
            turns.clone(),
            "stop",
            HopUsage {
                tokens_prompt: Some(10),
                tokens_completion: Some(5),
                tokens_cached: Some(8),
                cost: Some(0.000_25),
            },
            "gpt-4o",
            "chatcmpl-1",
            1234567890,
            42,
        )
        .await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/sink"));
        assert_eq!(em.content["messages"], json!(turns));
        assert_eq!(em.content["header"]["finish_reason"], "stop");
        assert_eq!(em.content["header"]["tokens_prompt"], 10);
        assert_eq!(em.content["header"]["tokens_completion"], 5);
        assert_eq!(em.content["header"]["tokens_cached"], 8);
        assert_eq!(em.content["header"]["cost"], 0.000_25);
        // GH #463: the ledger sums the HOP header, so the measured wall time
        // has to be there — and it stays in `meta` for the readers that had it.
        assert_eq!(em.content["header"]["latency_ms"], 42);
        assert_eq!(em.content["header"]["model"], "gpt-4o");
        assert_eq!(em.content["meta"]["provider"], "openai");
        assert_eq!(em.content["meta"]["model"], "gpt-4o");
        assert_eq!(em.content["meta"]["response_id"], "chatcmpl-1");
        assert_eq!(em.content["meta"]["latency_ms"], 42);
        assert_eq!(em.content["meta"]["started_at"], 1234567890);
    }

    /// GH #463: a figure the provider did not report is an ABSENT header key,
    /// never a zero. A summed zero and an unreported figure are the same number
    /// in a ledger total and two completely different facts — the ledger can
    /// only tell them apart if the header does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_usage_figure_the_provider_never_reported_is_absent_not_zero() {
        let (sink, mut rx) = mk_sink();
        emit_assistant_turn(
            &sink,
            Path::new("/sink"),
            vec![json!({"origin":"assistant","type":"text","text":"hi"})],
            "stop",
            HopUsage {
                tokens_prompt: Some(10),
                tokens_completion: Some(5),
                tokens_cached: None,
                cost: None,
            },
            "gpt-4o",
            "chatcmpl-1",
            1,
            7,
        )
        .await;
        let em = rx.recv().await.unwrap();
        let header = em.content["header"].as_object().expect("header object");
        assert!(
            !header.contains_key("tokens_cached"),
            "an unreported cache count must not appear as 0: {header:?}"
        );
        assert!(
            !header.contains_key("cost"),
            "an unreported cost must not appear as 0: {header:?}"
        );
        assert_eq!(header["tokens_prompt"], 10);
        assert_eq!(header["latency_ms"], 7, "latency is always measured");
    }

    /// GH #463: a non-finite cost is a broken field, not a price. It never
    /// reaches the header, because `SUM()` over one NaN poisons a whole window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_non_finite_cost_never_reaches_the_header() {
        let (sink, mut rx) = mk_sink();
        emit_assistant_turn(
            &sink,
            Path::new("/sink"),
            vec![],
            "stop",
            HopUsage {
                cost: Some(f64::NAN),
                ..HopUsage::default()
            },
            "gpt-4o",
            "chatcmpl-1",
            1,
            0,
        )
        .await;
        let em = rx.recv().await.unwrap();
        assert!(
            !em.content["header"]
                .as_object()
                .expect("header object")
                .contains_key("cost"),
            "NaN is not a cost: {}",
            em.content["header"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_error_passes_input_messages_through_unchanged_gate1() {
        let (sink, mut rx) = mk_sink();
        let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
        let input = vec![user_turn.clone()];
        emit_error(
            &sink,
            Path::new("/sink"),
            "rate_limit",
            "429 from OpenAI",
            "wire",
            input.clone(),
            1234567890,
            25,
            None,
            None,
            None,
        )
        .await;
        let em = rx.recv().await.unwrap();
        // Gate-1: messages == input.messages, byte-identisch.
        assert_eq!(em.content["messages"], json!(input));
        assert_eq!(em.content["messages"][0], user_turn);
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "rate_limit");
        // GH #463: a failed call still took 25 ms, and that is summable.
        assert_eq!(em.content["header"]["latency_ms"], 25);
        assert_eq!(em.content["meta"]["error"]["source"], "wire");
        assert_eq!(em.content["meta"]["error"]["detail"], "429 from OpenAI");
        assert_eq!(em.content["meta"]["provider"], "openai");
        assert!(
            em.content["meta"].get("model").is_none(),
            "model omitted when None"
        );
        assert!(em.content["meta"].get("response_id").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_error_with_empty_input_messages_for_parse_fail() {
        let (sink, mut rx) = mk_sink();
        emit_error(
            &sink,
            Path::new("/sink"),
            "provider_error",
            "invalid input body",
            "parse",
            vec![],
            1234567890,
            0,
            None,
            None,
            None,
        )
        .await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["messages"], json!([]));
        assert_eq!(em.content["meta"]["error"]["source"], "parse");
        assert_eq!(em.content["header"]["error_code"], "provider_error");
        assert_eq!(em.content["header"]["finish_reason"], "error");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_error_includes_model_and_response_id_when_provided() {
        let (sink, mut rx) = mk_sink();
        emit_error(
            &sink,
            Path::new("/sink"),
            "provider_error",
            "body parse fail after 200 OK",
            "parse",
            vec![json!({"origin":"user","type":"text","text":"Hi"})],
            1234567890,
            50,
            Some("gpt-4o-actual"),
            Some("chatcmpl-failed"),
            None,
        )
        .await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["meta"]["model"], "gpt-4o-actual");
        assert_eq!(em.content["meta"]["response_id"], "chatcmpl-failed");
    }
}
