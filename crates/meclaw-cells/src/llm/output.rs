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

use crate::llm::params::{CacheMode, LlmParams};
use crate::llm::translate::TranslatedResponse;
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, OutputSink, Path};

/// Every key this module writes into a hop header, with its JSON type -- the
/// success path ([`emit_assistant_turn`]), the error path
/// ([`emit_error_with_hop_for`]) and the decisions path ([`emit_decision`])
/// together, each written only when it has a value.
///
/// GH #890 (F15 of the gate wave, OR-KX-G8): the header grew keys the shipped
/// `llm` cells never declared -- `latency_ms`, `tokens_cached` and `cost` were
/// undeclared in 18 of 18 -- and nothing failed, because the contract check
/// leaves undeclared slots open. `gh890_every_llm_cell_declares_what_output_writes`
/// reads THIS list and holds every `llm` cell under `templates/` to it, so a
/// key added here without its declaration is a red test, not a quiet gap.
/// The unit test `every_key_the_emitters_write_is_named_here` holds the list
/// to the emitters in the other direction. Keys a caller hands in through
/// `extra_hop` (the GH #863 refusal pair) are the caller's, not this module's.
pub const HOP_KEYS: &[(&str, &str)] = &[
    ("finish_reason", "string"),
    ("error_code", "string"),
    ("latency_ms", "number"),
    ("tokens_prompt", "number"),
    ("tokens_completion", "number"),
    ("tokens_cached", "number"),
    ("tokens_cache_write", "number"),
    ("cost", "number"),
    ("model", "string"),
    ("cache_expires_at", "string"),
    ("context_window", "number"),
];

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
    /// GH #890: cache-WRITE tokens, in whichever spelling the provider used.
    /// Read before this issue by nobody: the one figure that says a prefix
    /// was just written, and so when a cache begins to age.
    pub(crate) tokens_cache_write: Option<u64>,
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
            tokens_cache_write: t.tokens_cache_write,
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
        // GH #890: a zero write is no write. A provider that reports
        // `cache_write_tokens: 0` on every call that wrote nothing would
        // otherwise put a key on every hop that says nothing happened.
        if let Some(t) = self.tokens_cache_write.filter(|t| *t > 0) {
            header.insert("tokens_cache_write".into(), Value::from(t));
        }
        // `Value::from(f64)` yields `Null` for a non-finite float, and a null
        // in a summed header column is worse than a missing key: SQLite would
        // read it as "present, unreadable" rather than "not reported".
        if let Some(c) = self.cost.filter(|c| c.is_finite()) {
            header.insert("cost".into(), Value::from(c));
        }
    }
}

/// GH #890: what the cell states about its own cache and window, stamped on a
/// SUCCESSFUL answer only. An error carries none of it: a call that failed
/// wrote no prefix, so there is nothing that could go cold, and the error
/// path keeps the header it has had since GH #463.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HopCache {
    /// Seconds the provider keeps the written prefix warm. `Some` only with
    /// `cache_mode` not `off` and `cache_ttl_s > 0`: `off` marks nothing, and
    /// a TTL of 0 is an unknown expiry (a local engine's prefix cache).
    pub(crate) ttl_s: Option<u64>,
    /// The model's context window in tokens, `Some` only above 0.
    pub(crate) context_window: Option<u64>,
}

impl HopCache {
    /// Read the stamp rules off the cell's params.
    pub(crate) fn of(params: &LlmParams) -> Self {
        Self {
            ttl_s: (params.cache_mode != CacheMode::Off && params.cache_ttl_s > 0)
                .then_some(params.cache_ttl_s),
            context_window: (params.context_window > 0).then_some(params.context_window),
        }
    }

    /// Write the stamp. `answered_at_unix_ms` is the end of the provider call
    /// -- `started_at + latency_ms`, the instant `latency_ms` measures to --
    /// cut to whole seconds, in UTC, and the TTL is added to that: RFC 3339 on
    /// seconds with `Z`, the form a `timer` takes as `at`, so the curator sets
    /// its alarm on the stamp as it stands (OR-KX-G2).
    fn write_into(&self, header: &mut Map<String, Value>, answered_at_unix_ms: i64) {
        if let Some(ttl) = self.ttl_s {
            let at = answered_at_unix_ms
                .div_euclid(1000)
                .saturating_add(ttl as i64);
            if let Some(t) = chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0) {
                header.insert(
                    "cache_expires_at".into(),
                    Value::String(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                );
            }
        }
        if let Some(window) = self.context_window {
            header.insert("context_window".into(), Value::from(window));
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
    cache: HopCache,
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
    cache.write_into(
        &mut header,
        started_at_unix_ms.saturating_add(latency_ms as i64),
    );
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

/// GH #957 success path of a `decisions` cell: ONE emission carrying the
/// whole verdict, `{decision: {answers, model, ms}}`, beside an empty
/// `messages` slot (a UBF body needs one of its three slots, and a decision
/// is no conversation turn).
///
/// The hop header carries only keys of [`HOP_KEYS`]: `finish_reason` `stop`
/// (so an edge tells it from the error path's `error` the way it does for a
/// chat answer), `model`, `latency_ms` and the usage figures the service
/// reported. A caller's correlation is NOT written here: `hop` dies at every
/// emission and `context` travels on unchanged, so the caller carries its own
/// key in `context` (OR-DP-11) and this cell never learns its name.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_decision(
    sink: &OutputSink,
    target: Path,
    answers: Map<String, Value>,
    usage: HopUsage,
    model: &str,
    response_id: &str,
    started_at_unix_ms: i64,
    latency_ms: u64,
) {
    let mut header = Map::new();
    header.insert("finish_reason".into(), Value::String("stop".into()));
    usage.write_into(&mut header);
    header.insert("latency_ms".into(), Value::from(latency_ms));
    header.insert("model".into(), Value::String(model.to_string()));

    let mut decision = Map::new();
    decision.insert("answers".into(), Value::Object(answers));
    decision.insert("model".into(), Value::String(model.to_string()));
    decision.insert("ms".into(), Value::from(latency_ms));

    let mut meta = Map::new();
    meta.insert(
        "provider".into(),
        Value::String(crate::llm::params::PROVIDER_DECISIONS.into()),
    );
    meta.insert("model".into(), Value::String(model.to_string()));
    meta.insert("response_id".into(), Value::String(response_id.to_string()));
    meta.insert("latency_ms".into(), Value::from(latency_ms));
    meta.insert("started_at".into(), Value::from(started_at_unix_ms));

    let mut body = Map::new();
    body.insert("decision".into(), Value::Object(decision));
    body.insert("messages".into(), Value::Array(Vec::new()));
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
    emit_error_for(
        "openai",
        sink,
        target,
        error_code,
        detail,
        error_source,
        input_messages,
        started_at_unix_ms,
        latency_ms,
        response_model,
        response_id,
        extra_error_meta,
    )
    .await;
}

/// [`emit_error`] naming the cell's own provider in `meta.provider`
/// (GH #957) -- what every call site inside the cell uses.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_error_for(
    provider: &str,
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
    emit_error_with_hop_for(
        provider,
        sink,
        target,
        error_code,
        detail,
        error_source,
        input_messages,
        started_at_unix_ms,
        latency_ms,
        response_model,
        response_id,
        extra_error_meta,
        None,
    )
    .await;
}

/// [`emit_error`] with header keys of the caller's own beside the fixed ones.
///
/// GH #863: the refusal of a params push addressed to THIS cell names the push
/// on two keys (`refused_subscriber`, `refused_model`), so the composite can
/// route it apart from a conversation's errors and the sender's road can carry
/// it back to the registry. `None` is byte-identical to [`emit_error`] -- every
/// other error keeps the shape it had. A key the fixed header already carries
/// (`finish_reason`, `error_code`, `latency_ms`) is never overwritten: an error
/// stays an error whatever the caller adds.
///
/// GH #957: `provider` names the wire that failed in `meta.provider`,
/// instead of a fixed `openai`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_error_with_hop_for(
    provider: &str,
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
    extra_hop: Option<Map<String, Value>>,
) {
    emit_error_carrying(
        provider,
        sink,
        target,
        error_code,
        detail,
        error_source,
        input_messages,
        started_at_unix_ms,
        latency_ms,
        response_model,
        response_id,
        extra_error_meta,
        extra_hop,
        None,
    )
    .await;
}

/// The shared error body, plus `carry`: body slots of the incoming request
/// handed on unchanged (GH #957, review I-1 / OR-DP-59: a failed `decisions`
/// call carries its `decide` slot so a failover cell has something to
/// decide). A carried key never overwrites `messages`, `meta` or `header`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_error_carrying(
    provider: &str,
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
    extra_hop: Option<Map<String, Value>>,
    carry: Option<Map<String, Value>>,
) {
    let mut header = Map::new();
    header.insert("finish_reason".into(), Value::String("error".into()));
    header.insert("error_code".into(), Value::String(error_code.to_string()));
    // GH #463: a failed call took time too, and a timeout is the one hop whose
    // latency an operator most wants summed. The error path carries no usage
    // block — a call that failed reports no tokens and no cost.
    header.insert("latency_ms".into(), Value::from(latency_ms));
    if let Some(extra) = extra_hop {
        for (k, v) in extra {
            header.entry(k).or_insert(v);
        }
    }

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
    meta.insert("provider".into(), Value::String(provider.into()));
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
    for (k, v) in carry.unwrap_or_default() {
        body.entry(k).or_insert(v);
    }

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
                tokens_cache_write: None,
                cost: Some(0.000_25),
            },
            HopCache::default(),
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
                tokens_cache_write: None,
                cost: None,
            },
            HopCache::default(),
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
            HopCache::default(),
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

    // ───── GH #890: the cache stamp ─────

    fn cache(ttl_s: Option<u64>, context_window: Option<u64>) -> HopCache {
        HopCache {
            ttl_s,
            context_window,
        }
    }

    async fn header_of(usage: HopUsage, cache: HopCache, started: i64, latency: u64) -> Value {
        let (sink, mut rx) = mk_sink();
        emit_assistant_turn(
            &sink,
            Path::new("/sink"),
            vec![],
            "stop",
            usage,
            cache,
            "m",
            "r",
            started,
            latency,
        )
        .await;
        rx.recv().await.unwrap().content["header"].clone()
    }

    /// The answer time is the end of the provider call (`started + latency`),
    /// cut to whole seconds in UTC; the TTL is added to that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_cache_stamp_is_the_answer_time_plus_the_ttl() {
        let h = header_of(
            HopUsage::default(),
            cache(Some(300), Some(200_000)),
            1_700_000_000_500,
            1_700,
        )
        .await;
        // 1_700_000_002_200 ms -> 1_700_000_002 s, + 300 s.
        assert_eq!(h["cache_expires_at"], "2023-11-14T22:18:22Z", "{h}");
        assert_eq!(h["context_window"], 200_000, "{h}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_ttl_and_no_window_stamp_nothing() {
        let h = header_of(HopUsage::default(), HopCache::default(), 1, 1).await;
        let h = h.as_object().unwrap();
        assert!(!h.contains_key("cache_expires_at"), "{h:?}");
        assert!(!h.contains_key("context_window"), "{h:?}");
    }

    #[test]
    fn off_or_a_zero_ttl_leaves_the_expiry_unknown() {
        let params = |extra: Value| {
            let mut raw = json!({"provider": "openai", "model": "m", "api_key": "k"});
            for (k, v) in extra.as_object().cloned().unwrap_or_default() {
                raw[k] = v;
            }
            LlmParams::parse(&raw).unwrap()
        };
        assert_eq!(
            HopCache::of(&params(json!({"cache_mode": "off", "cache_ttl_s": 300}))),
            cache(None, None)
        );
        assert_eq!(
            HopCache::of(&params(json!({"cache_mode": "implicit", "cache_ttl_s": 0}))),
            cache(None, None)
        );
        assert_eq!(
            HopCache::of(&params(
                json!({"cache_mode": "breakpoints", "cache_ttl_s": 300,
                                        "context_window": 1000})
            )),
            cache(Some(300), Some(1000))
        );
        assert_eq!(
            HopCache::of(&params(json!({"context_window": 1000}))),
            cache(None, Some(1000)),
            "the window is stamped without a cache"
        );
    }

    /// Review focus (4): a zero write is not stamped, a real one is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_zero_cache_write_is_not_stamped() {
        let zero = HopUsage {
            tokens_cache_write: Some(0),
            ..HopUsage::default()
        };
        let h = header_of(zero, HopCache::default(), 1, 1).await;
        assert!(h.get("tokens_cache_write").is_none(), "{h}");
        let some = HopUsage {
            tokens_cache_write: Some(176),
            ..HopUsage::default()
        };
        let h = header_of(some, HopCache::default(), 1, 1).await;
        assert_eq!(h["tokens_cache_write"], 176, "{h}");
    }

    /// `HOP_KEYS` is what the two emitters write, no more and no less: every
    /// key of a fully reported success and of an error is in it, and every
    /// entry of it is written by one of them. The template sweep reads the
    /// constant, so a dead entry would be a declaration nobody emits.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_key_the_emitters_write_is_named_here() {
        let full = HopUsage {
            tokens_prompt: Some(1),
            tokens_completion: Some(1),
            tokens_cached: Some(1),
            tokens_cache_write: Some(1),
            cost: Some(0.1),
        };
        let success = header_of(full, cache(Some(60), Some(1)), 1, 1).await;
        let (sink, mut rx) = mk_sink();
        emit_error(
            &sink,
            Path::new("/sink"),
            "provider_error",
            "x",
            "wire",
            vec![],
            1,
            1,
            None,
            None,
            None,
        )
        .await;
        let error = rx.recv().await.unwrap().content["header"].clone();
        // GH #957: the decisions emitter writes from the same list.
        emit_decision(
            &sink,
            Path::new("/sink"),
            Map::new(),
            full,
            "vendor/decider",
            "id-1",
            1,
            1,
        )
        .await;
        let decision = rx.recv().await.unwrap().content["header"].clone();
        let named: std::collections::BTreeSet<&str> = HOP_KEYS.iter().map(|(k, _)| *k).collect();
        let mut written = std::collections::BTreeSet::new();
        for header in [&success, &error, &decision] {
            for (key, value) in header.as_object().unwrap() {
                let (_, ty) = HOP_KEYS
                    .iter()
                    .find(|(k, _)| *k == key.as_str())
                    .unwrap_or_else(|| panic!("{key} is written but not in HOP_KEYS"));
                let fits = match *ty {
                    "string" => value.is_string(),
                    "number" => value.is_number(),
                    other => panic!("{key}: unknown type {other}"),
                };
                assert!(fits, "{key} is declared {ty} but written as {value}");
                written.insert(key.as_str());
            }
        }
        assert_eq!(written, named, "an entry of HOP_KEYS no emitter writes");
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

    /// GH #957: a decision is ONE emission -- the verdict, an empty `messages`
    /// slot (UBF), `meta.provider` = `decisions`, `finish_reason` `stop`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_decision_is_one_whole_body() {
        let (sink, mut rx) = mk_sink();
        let mut answers = Map::new();
        answers.insert("wants".into(), json!({"yes": 0.8}));
        emit_decision(
            &sink,
            Path::new("/sink"),
            answers,
            HopUsage::default(),
            "vendor/decider",
            "id-1",
            7,
            42,
        )
        .await;
        let em = rx.recv().await.unwrap().content;
        assert_eq!(
            em["decision"],
            json!({"answers": {"wants": {"yes": 0.8}}, "model": "vendor/decider", "ms": 42})
        );
        assert_eq!(em["messages"], json!([]));
        assert_eq!(em["meta"]["provider"], "decisions");
        assert_eq!(em["header"]["finish_reason"], "stop");
        meclaw_core::validate_ubf_body(&em).expect("a decision is a UBF body");
        assert!(rx.try_recv().is_err(), "exactly one emission");
    }

    /// GH #863: `emit_error` is `emit_error_with_hop_for` without extra keys, and
    /// that is byte for byte the body it always emitted -- every error that is
    /// not the refusal of an own push keeps its shape.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_error_without_extra_hop_is_byte_identical() {
        let (sink, mut rx) = mk_sink();
        let input = vec![json!({"origin":"user","type":"text","text":"Hi"})];
        emit_error(
            &sink,
            Path::new("/sink"),
            "invalid_input",
            "base_url outside the allow list",
            "parse",
            input.clone(),
            7,
            3,
            None,
            None,
            None,
        )
        .await;
        let plain = rx.recv().await.unwrap().content;
        emit_error_with_hop_for(
            "openai",
            &sink,
            Path::new("/sink"),
            "invalid_input",
            "base_url outside the allow list",
            "parse",
            input,
            7,
            3,
            None,
            None,
            None,
            None,
        )
        .await;
        let with_none = rx.recv().await.unwrap().content;
        assert_eq!(
            meclaw_core::serde_json::to_string(&plain).unwrap(),
            meclaw_core::serde_json::to_string(&with_none).unwrap(),
            "no extra hop keys, no difference"
        );
        assert_eq!(
            plain["header"],
            json!({"finish_reason": "error", "error_code": "invalid_input", "latency_ms": 3})
        );
    }

    /// GH #863: the extra keys ride beside the fixed ones and never replace
    /// one of them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_error_with_hop_adds_keys_and_keeps_the_fixed_ones() {
        let (sink, mut rx) = mk_sink();
        let mut extra = Map::new();
        extra.insert("refused_subscriber".into(), json!("/a/talky/brain"));
        extra.insert("refused_model".into(), json!("m2"));
        extra.insert("finish_reason".into(), json!("stop"));
        emit_error_with_hop_for(
            "openai",
            &sink,
            Path::new("/sink"),
            "invalid_input",
            "refused",
            "parse",
            vec![],
            1,
            0,
            None,
            None,
            None,
            Some(extra),
        )
        .await;
        let h = rx.recv().await.unwrap().content["header"].clone();
        assert_eq!(h["refused_subscriber"], "/a/talky/brain");
        assert_eq!(h["refused_model"], "m2");
        assert_eq!(h["finish_reason"], "error", "an error stays an error");
        assert_eq!(h["error_code"], "invalid_input");
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
