//! Phase-8 LlmCell params (T5: full struct + serde-Deserialize + parse-validate).

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_temperature() -> f64 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_external_timeout_ms() -> u64 {
    110_000
}

/// GH #118: default cap on the number of distinct slots in the persistent
/// `system` tree. Shared with the gate so the doc-comment lives in one place.
fn default_system_max_slots() -> usize {
    crate::llm::system_gate::DEFAULT_SYSTEM_MAX_SLOTS
}

/// GH #118: default cap on the serialized size of ONE system leaf, in bytes.
fn default_system_max_leaf_bytes() -> usize {
    crate::llm::system_gate::DEFAULT_SYSTEM_MAX_LEAF_BYTES
}

/// GH #87: A-timeout for one `attachments[]` blob read. A local filesystem read
/// of a file the substrate itself committed — orders of magnitude faster than a
/// provider round trip, so it gets a much tighter default than
/// `external_timeout_ms` and its own knob.
fn default_attachment_timeout_ms() -> u64 {
    5_000
}

/// GH #457: default bound on the turns parked while the sealed credential is in
/// flight. Sixteen is a conversation's worth of backlog, not a queue: the round
/// it covers is one in-colony hop pair, so anything beyond this is a vault that
/// is not answering, and that is what the receipt is for.
fn default_credential_wait_max() -> usize {
    16
}

/// GH #457: default A-timeout for the sealed-credential round, in ms. An
/// in-colony round trip, so it sits with `attachment_timeout_ms`'s order rather
/// than with the provider budget — generous enough for a loaded host, far below
/// anything a chat user would call "silence".
fn default_credential_wait_ms() -> u64 {
    10_000
}

/// How the cell authenticates against the provider (P10).
///
/// Vendor-neutral by design: `OauthSubscription` describes "a rotating OAuth
/// token from a token store", not an OpenAI specialty. OpenAI-via-Codex is
/// instance one, not the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Static `api_key` param, sent as `Authorization: Bearer` (the pre-P10
    /// behaviour and the default).
    #[default]
    ApiKey,
    /// Rotating OAuth access token read from the store at `auth_ref`.
    /// Refreshed lazily on 401 by the shared token broker.
    OauthSubscription,
}

/// Where the chat-completions request carries `reasoning_effort` and
/// `thinking_budget` (GH #854).
///
/// `Nested` is the pre-#854 shape byte for byte (`"reasoning": {"effort": …}`)
/// and what the shipped templates keep, because the hosted provider they
/// target reads it. `TopLevel` exists because an OpenAI-compatible local server
/// (vLLM) drops the nested block and reads `reasoning_effort` /
/// `thinking_token_budget` at the root instead — measured: the high default
/// ran into `length` with empty content in 2 of 8 calls, medium answered 38 of
/// 38 (GH #854).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningWire {
    /// `"reasoning": {"effort": …, "max_tokens": …}` — the default.
    #[default]
    Nested,
    /// `"reasoning_effort": …` and `"thinking_token_budget": …` at the root.
    TopLevel,
}

/// Which provider wire format the cell speaks (P10).
///
/// Deliberately orthogonal to `provider`: the Responses API is the SAME vendor
/// with a different wire shape, not a different provider. Keeping this a
/// separate axis leaves the `provider == "openai"` constraint untouched
/// (plan D1) and makes `auth × wire_dialect` the vendor-neutral matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireDialect {
    /// `POST /chat/completions` — the pre-P10 dialect.
    ChatCompletions,
    /// `POST /responses` — typed `input[]` items, SSE transport.
    Responses,
}

/// Phase-8 LlmCell parameters (cell-types.md `llm`-params block), extended by
/// the P10 auth dimension.
///
/// Required: `provider`, `model`, plus exactly one credential — `api_key`
/// (for `auth: "api_key"`) or `auth_ref` (for `auth: "oauth_subscription"`).
/// All other fields have defaults. `provider` names the WIRE PROTOCOL, and
/// `"openai"` is the only one implemented so far (see the field doc).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmParams {
    /// The wire protocol this cell speaks — NOT the vendor (maintainer ruling on
    /// GH #387). `"openai"` is the OpenAI-compatible HTTP API and the first
    /// and currently only implemented protocol; the vendor is chosen through
    /// `base_url` (OpenAI itself, OpenRouter, vLLM, …). Further protocols get
    /// added when a real consumer concretely needs one (`docs/defer-register.md`
    /// carries the defer plus its trigger).
    pub provider: String,
    /// Model id (e.g. `"gpt-4o"`).
    pub model: String,
    /// API key for the provider. Required for `auth: "api_key"`, and MUST be
    /// absent for `auth: "oauth_subscription"` (no double credential).
    #[serde(default)]
    pub api_key: Option<String>,
    /// R3 / GH #421: the grant this cell spends to be handed its bearer
    /// credential, sealed, by the access hive. `None` (the default) is the
    /// pre-Lane-3 behaviour byte for byte: the cell uses `api_key` and emits
    /// nothing. WHICH credential arrives is not named here — it is read from
    /// the grant's `cred_ref`, so a cell cannot ask for a secret it was not
    /// granted (R-AC-2 applied to the vault).
    #[serde(default)]
    pub credential_grant_id: Option<String>,
    /// GH #457: how many turns this cell parks while its sealed credential is
    /// in flight, before the next one is refused with `credential_pending`.
    ///
    /// The buffer exists so the turn that TRIGGERS the vault round is answered
    /// rather than dropped; the bound exists so a vault that never answers
    /// cannot grow the buffer without limit. Overflow is not silence: the turn
    /// that does not fit gets its receipt immediately.
    ///
    /// Only meaningful together with `credential_grant_id` — a cell that holds
    /// its own key never parks anything.
    #[serde(default = "default_credential_wait_max")]
    pub credential_wait_max: usize,
    /// GH #457: how long this cell waits for the sealed box before it gives up
    /// and hands every parked turn its `credential_pending` receipt, in
    /// milliseconds; default 10_000.
    ///
    /// This is an A-timeout on an IN-COLONY round trip (cell → broker → vault →
    /// cell), not on a provider call, so it is derived from
    /// `attachment_timeout_ms`'s order of magnitude rather than from
    /// `external_timeout_ms`. It is also the only way a cell learns that the
    /// round failed: a broker refusal is routed to the topology's error lane,
    /// never back to the asking cell, so silence is the only signal there is.
    #[serde(default = "default_credential_wait_ms")]
    pub credential_wait_ms: u64,
    /// P10: how this cell authenticates. Default `api_key` — every pre-P10
    /// config keeps its exact behaviour.
    #[serde(default)]
    pub auth: AuthMode,
    /// P10: filesystem path to the OAuth token store (Codex `auth.json`
    /// format). Required for `auth: "oauth_subscription"`, forbidden otherwise.
    ///
    /// Deliberately has NO default: an implicit `~/.codex/auth.json` would let
    /// a cell silently rotate the refresh token of a live interactive Codex
    /// session. Pointing at a store is a config decision (plan D7).
    #[serde(default)]
    pub auth_ref: Option<String>,
    /// P10: explicit wire dialect. `None` derives it from `auth`
    /// (`api_key` → chat-completions, `oauth_subscription` → responses).
    /// Read it through `effective_wire_dialect()`, never directly.
    #[serde(default)]
    pub wire_dialect: Option<WireDialect>,
    /// P10: OAuth token endpoint override. `None` → the provider default
    /// (`auth::DEFAULT_OAUTH_TOKEN_ENDPOINT`). Tests point this at a fake.
    #[serde(default)]
    pub oauth_token_endpoint: Option<String>,
    /// P10: OAuth client id override. `None` → the provider default.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// P10: value of the `originator` request header. `None` → the provider
    /// default (`codex_cli_rs`), which is what the pinned reference client
    /// sends.
    #[serde(default)]
    pub oauth_originator: Option<String>,
    /// P14: value of the `version` request header on the subscription lane.
    /// `None` → the provider default (`DEFAULT_OAUTH_CLIENT_VERSION`). The
    /// backend gates model availability on this value, so it is configuration,
    /// not decoration — see the constant's doc comment.
    #[serde(default)]
    pub oauth_client_version: Option<String>,
    /// Optional override of the provider's base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Sampling temperature; default 0.7.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Hard cap on generated tokens per response; default 4096.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Ordered list of UBF `system`-subtree paths to concatenate into the
    /// system prompt; default `[]`.
    #[serde(default)]
    pub system_order: Vec<String>,
    /// GH #118: cap on the number of distinct slots the persistent `system`
    /// tree may hold; default 256. A message write that would push the tree
    /// past it is a loud reject, never a truncation.
    #[serde(default = "default_system_max_slots")]
    pub system_max_slots: usize,
    /// GH #118: cap on the serialized size of ONE system leaf, in bytes;
    /// default 65536. Same loud-reject rule.
    #[serde(default = "default_system_max_leaf_bytes")]
    pub system_max_leaf_bytes: usize,
    /// GH #118: the subtrees a MESSAGE may write in the persistent `system`
    /// tree, as dotted slot-path prefixes (`"handover"`, `"tools"`, …).
    ///
    /// Empty (the default) = no allowlist configured, every slot path stays
    /// writable — pre-#118 behaviour, so the operator's direct `@external`
    /// system update and every in-topology writer keep working untouched. A
    /// non-empty list pins the message-writable surface; the `seed/system.jsonl`
    /// loader is NOT gated by it (configuration tier, see `system_gate`).
    ///
    /// Prefixes are relative to the `system` subtree — `"identity"`, never
    /// `"system.identity"`.
    #[serde(default)]
    pub system_writable: Vec<String>,
    /// Pass-through map of provider-specific extras; default `{}`.
    #[serde(default)]
    pub provider_extra: serde_json::Map<String, serde_json::Value>,
    /// Operation-timeout (A) for the HTTP call to the provider, in
    /// milliseconds; default 110_000 (= 110 s, llm A-default per
    /// cell-types.md Z.1542).
    #[serde(default = "default_external_timeout_ms")]
    pub external_timeout_ms: u64,
    /// GH #87: operation-timeout (A) for reading ONE `attachments[]` blob from
    /// the store, in milliseconds; default 5_000. Only meaningful for a cell
    /// that declares `consumes.body.attachments` — a cell without that
    /// declaration never holds a reader and never reads a blob.
    #[serde(default = "default_attachment_timeout_ms")]
    pub attachment_timeout_ms: u64,
    /// App-attribution: OpenRouter `HTTP-Referer` request header (app page /
    /// model rankings). A regular param (A4 params-uniform ruling): set in
    /// `config.json`, `${VAR}`-substituted from `.env` like any other param.
    /// `None` = no header emitted. The Translate boundary maps this param to a
    /// wire HTTP header (not the request body); see
    /// `translate::build_attribution_headers`.
    #[serde(default)]
    pub http_referer: Option<String>,
    /// App-attribution: OpenRouter `X-Title` request header (app display name
    /// in rankings). Same params-uniform handling as `http_referer`; `None` =
    /// no header emitted.
    #[serde(default)]
    pub x_title: Option<String>,
    /// GH #124: deliberation budget for a thinking-class model on the
    /// chat-completions lane, in the provider's shorthand form (`"low"` /
    /// `"medium"` / `"high"` and whatever else the provider accepts — the
    /// value is passed through, not validated against a list this crate would
    /// have to chase).
    ///
    /// The Translate boundary turns it into the request body's
    /// `"reasoning": {"effort": …}`. `None` = the field is NOT sent and the
    /// request is byte-identical to one from before this param existed — the
    /// same unset-means-absent rule as `http_referer`/`x_title` (A4).
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// GH #124: the full reasoning block, passed to the request body verbatim
    /// (e.g. `{"effort": "low", "exclude": true}` or a `max_tokens` budget).
    ///
    /// Exists next to `reasoning_effort` because the providers' reasoning
    /// objects keep growing and a shorthand cannot express all of them; this
    /// param is the escape hatch that does not need a code change per knob.
    /// When BOTH are set this one wins (it is the strictly more expressive
    /// form). `None` = the field is not sent.
    #[serde(default)]
    pub reasoning: Option<Value>,
    /// GH #854: where `reasoning_effort` and `thinking_budget` go on the
    /// chat-completions wire. Default `nested` = the request of before.
    #[serde(default)]
    pub reasoning_wire: ReasoningWire,
    /// GH #854: a thinking budget in tokens. `nested` sends it as
    /// `reasoning.max_tokens`, `top_level` as `thinking_token_budget` at the
    /// root; a key of the same name in `provider_extra` wins (it is overlaid
    /// last). `None` = not sent.
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    /// GH #853: the prompt block of one model — its peculiarities, apart from
    /// the model-neutral persona. The cell puts it FIRST in the system part
    /// (`translate::compose_system_prompt`). A param, not a system slot: it
    /// travels with the overlay and returns with `$reset`, and the collector
    /// stays the one writer on `system`. Empty = no block. At most
    /// [`MODEL_PROMPT_MAX_BYTES`].
    #[serde(default)]
    pub model_prompt: Option<String>,
    /// GH #853: the origins a RUN-TIME `base_url` may point at. Immutable —
    /// the list is only worth something out of the message path's reach.
    /// Empty (the default) = `base_url` is fixed at run time. The start value
    /// needs no entry.
    #[serde(default)]
    pub base_url_allow: Vec<String>,
    /// GH #858: what this cell needs from a model, in prose — its role, the
    /// latency it can afford, how much context it reads, how deep it thinks,
    /// what it may cost; never a model name. In meclaw-os the `llm-registry`
    /// translates it against its catalogue once per change. The cell itself
    /// never reads it on a call: it only shows in the params line. Immutable
    /// (a statement of the template, not a knob), at most
    /// [`REQUIREMENT_MAX_BYTES`].
    #[serde(default)]
    pub requirement: Option<String>,
}

/// GH #853: the upper bound of `model_prompt`, in bytes. A model's quirks are
/// a paragraph; anything bigger is a persona in the wrong slot.
pub const MODEL_PROMPT_MAX_BYTES: usize = 8 * 1024;

/// GH #858: the upper bound of `requirement`, in bytes. Two to four sentences
/// of need fit many times over; the registry's hand refuses the same bound.
pub const REQUIREMENT_MAX_BYTES: usize = 2 * 1024;

/// GH #853 / #854: the keys of a model package — what an operator (in
/// meclaw-os: the `llm-registry`) sets when it moves a cell to another model.
///
/// They take effect only from a params-only message (a body without a
/// `messages` slot): a conversation cannot change the model it talks to.
/// `reasoning` is in the set because it outranks `reasoning_effort`; leaving
/// it out would let a turn change the deliberation after all. This list is
/// the contract the registry pushes against — a change here is a contract
/// change.
pub const MODEL_PACKAGE_KEYS: &[&str] = &[
    "model",
    "base_url",
    "wire_dialect",
    "reasoning_effort",
    "reasoning_wire",
    "reasoning",
    "thinking_budget",
    "max_tokens",
    "temperature",
    "external_timeout_ms",
    "provider_extra",
    "model_prompt",
];

/// GH #853: the absolute floor of the backstop margin, in ms.
pub const BACKSTOP_MARGIN_FLOOR_MS: u64 = 10_000;

/// GH #853: the relative floor of the backstop margin, as a divisor (10 %).
pub const BACKSTOP_MARGIN_DIVISOR: u64 = 10;

/// The ONE statement of the "B generous, A precise" rule (AGENTS.md rule 12):
/// `Some(required_ms)` iff a backstop of `backstop_ms` does not clear a call
/// of `external_ms` by at least 10 s AND 10 %. A `None` backstop is no
/// backstop, which can never cut the call short.
///
/// Called by the shipped-template gate
/// (`tests/a_shipped_llm_backstop_outlasts_its_own_call.rs`) and by the
/// run-time update path, so a run-time `external_timeout_ms` cannot recreate
/// the inversion the gate removed from the templates (a watchdog kill instead
/// of an answer, CHANGELOG § 0.27.0).
pub fn backstop_shortfall(external_ms: u64, backstop_ms: Option<u64>) -> Option<u64> {
    let backstop = backstop_ms?;
    let required =
        external_ms + BACKSTOP_MARGIN_FLOOR_MS.max(external_ms / BACKSTOP_MARGIN_DIVISOR);
    (backstop < required).then_some(required)
}

/// GH #853: the origin of a URL (`scheme://host[:port]`) if it is one a list
/// may name — `http`/`https`, a host, no userinfo. `None` otherwise.
pub(crate) fn listable_origin(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    url.host_str()?;
    Some(url.origin().ascii_serialization())
}

impl LlmParams {
    /// Implementation detail — production entry point is `LlmCellFactory`;
    /// direct construction is `pub` only so tests/integration tests can
    /// drive the cell without the full Colony.
    ///
    /// Validates `provider == "openai"` — the only wire protocol implemented
    /// so far (cell-types.md § `llm` params; maintainer ruling on GH #387). All
    /// other fields are validated structurally by serde. The returned error
    /// message never echoes the `api_key` value (Plan § 12-API_KEY).
    #[doc(hidden)]
    pub fn parse(raw: &serde_json::Value) -> Result<Self, String> {
        let p: Self =
            serde_json::from_value(raw.clone()).map_err(|e| format!("invalid LlmParams: {e}"))?;
        // `provider` is the wire protocol, not the vendor: `"openai"` is the
        // OpenAI-compatible HTTP API and so far the only protocol with a
        // translate behind it. A vendor swap happens through `base_url`.
        if p.provider != "openai" {
            return Err(format!(
                "provider must be 'openai' in phase 8, got '{}'",
                p.provider
            ));
        }
        // P10 auth validation — strictly ADDITIVE, after the untouched provider
        // check. Exactly one credential, and the dialect must be able to carry
        // the chosen auth. No message ever echoes a param VALUE (cell-types
        // Z.145 secret hygiene).
        match p.auth {
            AuthMode::ApiKey => {
                if p.api_key.is_none() {
                    return Err("api_key is required for auth 'api_key'".to_string());
                }
                if p.auth_ref.is_some() {
                    return Err("auth_ref is only valid for auth 'oauth_subscription'".to_string());
                }
            }
            AuthMode::OauthSubscription => {
                if p.auth_ref.is_none() {
                    return Err("auth_ref is required for auth 'oauth_subscription'".to_string());
                }
                if p.api_key.is_some() {
                    return Err("api_key must not be set for auth 'oauth_subscription'".to_string());
                }
                if p.wire_dialect == Some(WireDialect::ChatCompletions) {
                    return Err(
                        "auth 'oauth_subscription' requires wire_dialect 'responses'".to_string(),
                    );
                }
            }
        }
        // GH #118 system-gate knobs. A cap of zero would wall the cell off from
        // its own system tree; a prefix written in `system.`-space would never
        // match a slot path (they are relative to the `system` subtree) and
        // would silently pin the cell shut. Both are loud at spawn.
        if p.system_max_slots == 0 {
            return Err("system_max_slots must be at least 1".to_string());
        }
        if p.system_max_leaf_bytes == 0 {
            return Err("system_max_leaf_bytes must be at least 1".to_string());
        }
        // GH #853: the model block and the allow list are checked at birth, so
        // a run-time update (which re-parses the merge) is checked the same way.
        if let Some(mp) = &p.model_prompt
            && mp.len() > MODEL_PROMPT_MAX_BYTES
        {
            return Err(format!(
                "model_prompt is {} bytes; at most {MODEL_PROMPT_MAX_BYTES} are allowed",
                mp.len()
            ));
        }
        if let Some(req) = &p.requirement
            && req.len() > REQUIREMENT_MAX_BYTES
        {
            return Err(format!(
                "requirement is {} bytes; at most {REQUIREMENT_MAX_BYTES} are allowed",
                req.len()
            ));
        }
        for entry in &p.base_url_allow {
            // Compared in normal form (review of L2, M-5): `HTTPS://X.com:443`
            // is the origin `https://x.com`; a path, a query or a fragment
            // still makes the entry no origin.
            match (listable_origin(entry), reqwest::Url::parse(entry)) {
                (Some(_), Ok(url))
                    if url.path() == "/" && url.query().is_none() && url.fragment().is_none() => {}
                _ => {
                    return Err(format!(
                        "base_url_allow entry {entry:?} must be an origin \
                         (scheme://host[:port], http or https, no userinfo, no path)"
                    ));
                }
            }
        }
        for prefix in &p.system_writable {
            if prefix.is_empty() {
                return Err(
                    "system_writable entries must be non-empty slot-path prefixes".to_string(),
                );
            }
            if prefix.starts_with("system.") || prefix == "system" {
                return Err(format!(
                    "system_writable entry '{prefix}': prefixes are relative to the system \
                     subtree — write 'identity', not 'system.identity'"
                ));
            }
            if prefix.starts_with('.') || prefix.ends_with('.') {
                return Err(format!(
                    "system_writable entry '{prefix}': must not start or end with '.'"
                ));
            }
        }
        Ok(p)
    }

    /// The wire dialect this cell actually speaks: the explicit
    /// `wire_dialect` param if set, else derived from `auth`.
    ///
    /// `parse` guarantees the derived value is consistent with `auth`, so this
    /// is total and never panics.
    pub fn effective_wire_dialect(&self) -> WireDialect {
        match self.wire_dialect {
            Some(d) => d,
            None => match self.auth {
                AuthMode::ApiKey => WireDialect::ChatCompletions,
                AuthMode::OauthSubscription => WireDialect::Responses,
            },
        }
    }

    /// Apply a runtime params-update (W4b). Pure — no IO.
    ///
    /// `update` is the top-level `params` body-slot of a params-update message
    /// (config.md § Access l.20): a partial map of param keys to new values,
    /// last-write-wins. Returns the merged `LlmParams` plus the overlay pairs to
    /// persist in `cell.db` (`state::persist_params_overlay`). The caller applies
    /// neither on `Err` — all-or-nothing, no partial apply.
    ///
    /// Reject rules (loud, no partial apply):
    /// - an `IMMUTABLE_PARAM_KEYS` key is present (`provider`, `api_key`) →
    ///   `Immutable` (credential / identity; mirrors the A4 `Authorization`
    ///   secret-hygiene ruling, cell-types.md § llm),
    /// - a key outside `KNOWN_PARAM_KEYS` is present → `Unknown` (a typo'd key
    ///   would otherwise silently no-op),
    /// - the merged result fails `LlmParams::parse` (wrong value type, etc.) →
    ///   `Invalid`.
    pub fn apply_update(
        &self,
        update: &serde_json::Map<String, Value>,
    ) -> Result<(LlmParams, Vec<(String, Value)>), ParamUpdateError> {
        // β: delegate to the generic params-overlay core. The immutable/unknown/
        // invalid reject rules + the merge-then-reparse live there; the per-type
        // key-sets + parse are supplied via `impl OverlayParams for LlmParams`.
        crate::params_overlay::apply_update(self, update)
    }

    /// GH #853: the run-time guards a params update passes on top of `parse`.
    ///
    /// `self` is the CURRENT params (its `base_url_allow` is immutable, so it
    /// is also the merged one's), `merged` the params the update would give,
    /// `start_base_url` the birth value, `backstop_ms` the cell's resolved
    /// `message_timeout` (`None` = no backstop). Every detail names the key and
    /// the rule; a URL is named only by its origin, never with its path or
    /// userinfo (and not at all where no list exists), a refused timeout by its
    /// ms and the backstop it needs. No detail carries a secret.
    ///
    /// - `base_url` moves only to an origin in `base_url_allow`; without a list
    ///   it is fixed. Its own start value is always allowed back (a package
    ///   that repeats the start endpoint is no change), and `null` is refused:
    ///   it falls back to the provider default silently and takes the bearer
    ///   with it (`cell.rs`, `wire::OPENAI_DEFAULT_BASE_URL`). Before the list
    ///   a message could send the bearer to any host while `api_key` itself
    ///   was immutable.
    /// - `external_timeout_ms` must stay cleared by the backstop
    ///   ([`backstop_shortfall`], the rule the shipped-template gate uses).
    pub(crate) fn check_run_time_update(
        &self,
        start_base_url: Option<&str>,
        update: &serde_json::Map<String, Value>,
        merged: &LlmParams,
        backstop_ms: Option<u64>,
    ) -> Result<(), String> {
        match update.get("base_url") {
            None => {}
            Some(Value::String(url)) if Some(url.as_str()) == start_base_url => {}
            Some(Value::String(url)) => {
                let origin = listable_origin(url).ok_or_else(|| {
                    "params update rejected: 'base_url' must be an http(s) URL without userinfo"
                        .to_string()
                })?;
                if self.base_url_allow.is_empty() {
                    return Err("params update rejected: 'base_url' is fixed at run time — \
                                params.base_url_allow names no origin"
                        .to_string());
                }
                if !self
                    .base_url_allow
                    .iter()
                    .any(|a| listable_origin(a).as_deref() == Some(origin.as_str()))
                {
                    return Err(format!(
                        "params update rejected: the origin {origin} of 'base_url' is not in \
                         params.base_url_allow"
                    ));
                }
            }
            Some(Value::Null) => {
                return Err(
                    "params update rejected: 'base_url' cannot be null at run time — \
                            '$reset' returns it to its start value"
                        .to_string(),
                );
            }
            // Any other type has already failed `parse`.
            Some(_) => {}
        }
        if update.contains_key("external_timeout_ms")
            && let Some(required) = backstop_shortfall(merged.external_timeout_ms, backstop_ms)
        {
            return Err(format!(
                "params update rejected: 'external_timeout_ms' {} ms is not cleared by this \
                 cell's backstop (needs a message_timeout of at least {required} ms: +10 s and \
                 +10 %)",
                merged.external_timeout_ms
            ));
        }
        Ok(())
    }
}

impl crate::params_overlay::OverlayParams for LlmParams {
    const KNOWN_KEYS: &'static [&'static str] = KNOWN_PARAM_KEYS;
    const IMMUTABLE_KEYS: &'static [&'static str] = IMMUTABLE_PARAM_KEYS;
    fn parse(raw: &Value) -> Result<Self, String> {
        LlmParams::parse(raw)
    }
}

/// Top-level param keys recognized by the `llm` cell (the `LlmParams` fields).
/// A params-update key outside this set is rejected (no silent no-op).
pub(crate) const KNOWN_PARAM_KEYS: &[&str] = &[
    "provider",
    "model",
    "api_key",
    // R3 / GH #421: the sealed credential delivery.
    "credential_grant_id",
    // GH #457: the parking bound + deadline of that same delivery.
    "credential_wait_max",
    "credential_wait_ms",
    "base_url",
    "temperature",
    "max_tokens",
    "system_order",
    "provider_extra",
    "external_timeout_ms",
    // GH #118 system-write gate.
    "system_max_slots",
    "system_max_leaf_bytes",
    "system_writable",
    // GH #87 attachment consumption.
    "attachment_timeout_ms",
    "http_referer",
    "x_title",
    // GH #124 reasoning passthrough (chat-completions lane).
    "reasoning_effort",
    "reasoning",
    // GH #854 reasoning wire form + budget.
    "reasoning_wire",
    "thinking_budget",
    // GH #853 model package: prompt block + run-time endpoint allow list.
    "model_prompt",
    "base_url_allow",
    // GH #858: the prose requirement the registry translates.
    "requirement",
    // P10 auth dimension.
    "auth",
    "auth_ref",
    "wire_dialect",
    "oauth_token_endpoint",
    "oauth_client_id",
    "oauth_originator",
    // P14 auth dimension.
    "oauth_client_version",
];

/// Param keys that may NOT be changed at runtime via a params-update message.
///
/// `provider` (the wire protocol this cell was built around) and `api_key`
/// (credential, secret-hygiene);
/// P10 adds the credential half of the auth dimension — `auth`/`auth_ref` are
/// credential identity, and `oauth_*` decide which endpoint a credential is
/// presented to. Letting a message repoint any of them would let a params
/// update send an existing token somewhere new. `wire_dialect` left this list
/// with GH #853: a model package names its dialect, the credential stays the
/// same, and `parse` re-checks the `auth` pairing on every update.
pub(crate) const IMMUTABLE_PARAM_KEYS: &[&str] = &[
    "provider",
    "api_key",
    // R3 / GH #421: same class as `api_key`. A message that could repoint the
    // grant could make this cell ask the vault for a DIFFERENT credential and
    // then present it — the grant is what names the secret, so the grant is
    // credential identity and belongs in this list, not in the mutable half.
    "credential_grant_id",
    "auth",
    "auth_ref",
    "oauth_token_endpoint",
    "oauth_client_id",
    "oauth_originator",
    // P14: frozen with the family. It feeds the same backend gate as
    // `oauth_originator` and flows into the same `User-Agent`; a runtime
    // overlay would also outlive — and silently override — a later
    // `config.json` fix.
    "oauth_client_version",
    // GH #118: the system-write gate. A message that could raise its own
    // limits or clear its own allowlist would not be gated at all — the
    // declaration only means something if it is out of the message path's
    // reach, exactly like the credential keys above.
    "system_max_slots",
    "system_max_leaf_bytes",
    "system_writable",
    // GH #853: the run-time endpoint allow list. A message that could widen
    // it would make the guard it is about to pass meaningless.
    "base_url_allow",
    // GH #858: what the template says this cell needs. The registry reads it
    // from the template, so a run-time copy would be a second truth nobody
    // translates.
    "requirement",
];

/// GH #853: the immutable keys that are credential identity. An update naming
/// one is told that a package needing another one needs a mutation.
pub(crate) const CREDENTIAL_KEYS: &[&str] = &[
    "provider",
    "api_key",
    "credential_grant_id",
    "auth",
    "auth_ref",
    "oauth_token_endpoint",
    "oauth_client_id",
    "oauth_originator",
    "oauth_client_version",
];

/// β: the reject type now lives in the generic params-overlay core. Re-exported
/// here so existing `llm`-internal references (and W4b tests) are unchanged.
pub use crate::params_overlay::ParamUpdateError;

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_rejects_empty_object() {
        let r = LlmParams::parse(&json!({}));
        assert!(
            r.is_err(),
            "empty params must reject (missing provider/model/api_key)"
        );
    }

    #[test]
    fn parse_minimal_valid_uses_defaults() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.provider, "openai");
        assert_eq!(p.model, "gpt-4o");
        assert_eq!(p.api_key.as_deref(), Some("x"));
        assert_eq!(p.base_url, None);
        assert_eq!(p.temperature, 0.7);
        assert_eq!(p.max_tokens, 4096);
        assert!(p.system_order.is_empty());
        assert!(p.provider_extra.is_empty());
        assert_eq!(p.external_timeout_ms, 110_000);
    }

    #[test]
    fn parse_attachment_timeout_has_a_default_well_below_external_timeout() {
        // GH #87: the A-timeout for a blob read is a filesystem read, not a
        // provider round trip — its own knob, its own (much smaller) default.
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.attachment_timeout_ms, 5_000);
        assert!(p.attachment_timeout_ms < p.external_timeout_ms);
    }

    #[test]
    fn attachment_timeout_is_a_known_mutable_param() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        let update = json!({"attachment_timeout_ms": 250})
            .as_object()
            .unwrap()
            .clone();
        let (merged, overlay) = p.apply_update(&update).unwrap();
        assert_eq!(merged.attachment_timeout_ms, 250);
        assert_eq!(overlay.len(), 1);
    }

    // ───── GH #118: the system-write gate ─────

    /// The safe default is "bounded, not closed": limits are on for every cell,
    /// the allowlist is opt-in and empty — so a pre-#118 config keeps its exact
    /// behaviour (the operator's direct `@external` system update included).
    #[test]
    fn system_gate_params_default_to_bounded_but_unpinned() {
        let p = LlmParams::parse(&api_key_raw()).unwrap();
        assert_eq!(p.system_max_slots, 256);
        assert_eq!(p.system_max_leaf_bytes, 65_536);
        assert!(
            p.system_writable.is_empty(),
            "no allowlist unless a topology declares one"
        );
    }

    #[test]
    fn parse_reads_the_system_gate_params() {
        let mut raw = api_key_raw();
        raw["system_max_slots"] = json!(8);
        raw["system_max_leaf_bytes"] = json!(1024);
        raw["system_writable"] = json!(["handover", "tools"]);
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.system_max_slots, 8);
        assert_eq!(p.system_max_leaf_bytes, 1024);
        assert_eq!(p.system_writable, vec!["handover", "tools"]);
    }

    /// The whole point of the gate: a message must never be able to widen the
    /// gate it is about to pass.
    #[test]
    fn the_system_gate_params_are_immutable_at_runtime() {
        let base = LlmParams::parse(&api_key_raw()).unwrap();
        for (key, value) in [
            ("system_max_slots", json!(999_999)),
            ("system_max_leaf_bytes", json!(999_999)),
            ("system_writable", json!([])),
        ] {
            let update = json!({key: value}).as_object().unwrap().clone();
            let err = base.apply_update(&update).unwrap_err();
            assert!(
                matches!(err, super::ParamUpdateError::Immutable(ref k) if k == key),
                "key {key} must be immutable, got {err:?}"
            );
        }
    }

    /// A prefix in `system.`-space would never match a slot path (slot paths
    /// are relative to the `system` subtree) and would pin the cell shut
    /// without a word. Loud at spawn instead.
    #[test]
    fn parse_rejects_a_system_prefixed_writable_entry() {
        let mut raw = api_key_raw();
        raw["system_writable"] = json!(["system.identity"]);
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("system.identity"), "{err}");
        assert!(err.contains("relative"), "{err}");
    }

    #[test]
    fn parse_rejects_a_zero_system_limit() {
        let mut raw = api_key_raw();
        raw["system_max_slots"] = json!(0);
        assert!(LlmParams::parse(&raw).is_err());
        let mut raw = api_key_raw();
        raw["system_max_leaf_bytes"] = json!(0);
        assert!(LlmParams::parse(&raw).is_err());
    }

    #[test]
    fn parse_rejects_a_malformed_writable_entry() {
        for bad in [json!([""]), json!([".identity"]), json!(["identity."])] {
            let mut raw = api_key_raw();
            raw["system_writable"] = bad.clone();
            assert!(
                LlmParams::parse(&raw).is_err(),
                "entry {bad} must be rejected"
            );
        }
    }

    // ───── GH #124: reasoning passthrough ─────

    #[test]
    fn parse_reasoning_fields_default_to_none() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.reasoning_effort, None);
        assert_eq!(p.reasoning, None);
    }

    #[test]
    fn parse_reads_both_reasoning_forms() {
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x",
            "reasoning_effort": "low",
            "reasoning": {"effort": "high", "exclude": true},
        });
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(
            p.reasoning,
            Some(json!({"effort": "high", "exclude": true}))
        );
    }

    /// A deliberation budget is a knob, not an identity: it must be tunable at
    /// runtime like `model` or `temperature`, not frozen with the credentials.
    #[test]
    fn reasoning_fields_are_known_and_mutable_params() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        let update = json!({"reasoning_effort": "high", "reasoning": {"effort": "high"}})
            .as_object()
            .unwrap()
            .clone();
        let (merged, overlay) = p.apply_update(&update).unwrap();
        assert_eq!(merged.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(merged.reasoning, Some(json!({"effort": "high"})));
        assert_eq!(overlay.len(), 2);
    }

    #[test]
    fn parse_attribution_fields_default_to_none() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.http_referer, None);
        assert_eq!(p.x_title, None);
    }

    #[test]
    fn parse_reads_attribution_fields() {
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x",
            "http_referer": "https://example.com",
            "x_title": "Example App",
        });
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.http_referer.as_deref(), Some("https://example.com"));
        assert_eq!(p.x_title.as_deref(), Some("Example App"));
    }

    #[test]
    fn apply_update_changes_mutable_model_and_returns_overlay() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"model": "gpt-4o-mini"}).as_object().unwrap().clone();
        let (new, overlay) = base.apply_update(&update).unwrap();
        assert_eq!(new.model, "gpt-4o-mini");
        assert_eq!(new.api_key.as_deref(), Some("x")); // unchanged
        assert_eq!(overlay, vec![("model".to_string(), json!("gpt-4o-mini"))]);
    }

    #[test]
    fn apply_update_merges_multiple_mutable_keys() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"temperature": 0.2, "max_tokens": 256})
            .as_object()
            .unwrap()
            .clone();
        let (new, _overlay) = base.apply_update(&update).unwrap();
        assert_eq!(new.temperature, 0.2);
        assert_eq!(new.max_tokens, 256);
    }

    #[test]
    fn apply_update_rejects_immutable_api_key() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"api_key": "leaked"}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "api_key"));
        // detail must NOT echo the attempted value.
        assert!(
            !err.detail().contains("leaked"),
            "detail leaks value: {}",
            err.detail()
        );
    }

    #[test]
    fn apply_update_rejects_immutable_provider() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"provider": "openai"}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "provider"));
    }

    #[test]
    fn apply_update_rejects_unknown_key() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"temperatur": 0.2}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Unknown(ref k) if k == "temperatur"));
    }

    #[test]
    fn apply_update_rejects_malformed_value_no_partial() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        // model is valid but temperature is the wrong type → whole update rejects.
        let update = json!({"model": "gpt-4o-mini", "temperature": "hot"})
            .as_object()
            .unwrap()
            .clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Invalid(_)));
    }

    #[test]
    fn apply_update_immutable_wins_over_otherwise_valid_keys_no_partial() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"model": "gpt-4o-mini", "api_key": "leaked"})
            .as_object()
            .unwrap()
            .clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "api_key"));
    }

    // ───── P10: auth dimension + wire dialect ─────

    fn api_key_raw() -> serde_json::Value {
        json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"})
    }

    #[test]
    fn a_credential_grant_is_optional_and_immutable() {
        let mut raw = api_key_raw();
        raw["credential_grant_id"] = json!("grant:abc");
        let p = LlmParams::parse(&raw).expect("parse");
        assert_eq!(p.credential_grant_id.as_deref(), Some("grant:abc"));
        assert!(
            LlmParams::parse(&api_key_raw())
                .unwrap()
                .credential_grant_id
                .is_none()
        );

        // A message that could repoint it could ask the vault for a different
        // credential — same class as `api_key` itself.
        let mut upd = serde_json::Map::new();
        upd.insert("credential_grant_id".into(), json!("grant:somebody-elses"));
        assert!(matches!(
            p.apply_update(&upd),
            Err(crate::params_overlay::ParamUpdateError::Immutable(_))
        ));
    }

    fn oauth_raw() -> serde_json::Value {
        json!({"provider": "openai", "model": "gpt-5", "auth": "oauth_subscription",
               "auth_ref": "/tmp/auth.json"})
    }

    #[test]
    fn parse_auth_defaults_to_api_key() {
        let p = LlmParams::parse(&api_key_raw()).unwrap();
        assert_eq!(p.auth, AuthMode::ApiKey);
        assert_eq!(p.effective_wire_dialect(), WireDialect::ChatCompletions);
    }

    #[test]
    fn parse_rejects_missing_api_key_in_api_key_mode() {
        let raw = json!({"provider": "openai", "model": "gpt-4o"});
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("api_key"), "{err}");
    }

    #[test]
    fn parse_accepts_oauth_subscription_with_auth_ref() {
        let p = LlmParams::parse(&oauth_raw()).unwrap();
        assert_eq!(p.auth, AuthMode::OauthSubscription);
        assert_eq!(p.auth_ref.as_deref(), Some("/tmp/auth.json"));
        assert_eq!(p.api_key, None);
    }

    #[test]
    fn parse_rejects_oauth_without_auth_ref() {
        let raw = json!({"provider": "openai", "model": "gpt-5",
                         "auth": "oauth_subscription"});
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("auth_ref"), "{err}");
    }

    #[test]
    fn parse_rejects_both_credentials() {
        let mut raw = oauth_raw();
        raw["api_key"] = json!("leaked-key");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("api_key"), "{err}");
        // secret hygiene: the reject must never echo the value.
        assert!(!err.contains("leaked-key"), "error leaks value: {err}");
    }

    #[test]
    fn parse_rejects_auth_ref_in_api_key_mode() {
        let mut raw = api_key_raw();
        raw["auth_ref"] = json!("/tmp/auth.json");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("auth_ref"), "{err}");
    }

    #[test]
    fn oauth_defaults_to_responses_dialect() {
        let p = LlmParams::parse(&oauth_raw()).unwrap();
        assert_eq!(p.effective_wire_dialect(), WireDialect::Responses);
    }

    #[test]
    fn parse_rejects_oauth_with_chat_completions_dialect() {
        let mut raw = oauth_raw();
        raw["wire_dialect"] = json!("chat_completions");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("wire_dialect"), "{err}");
    }

    #[test]
    fn api_key_mode_may_opt_into_responses_dialect() {
        // This is the lane the paid smoke uses: api.openai.com/v1/responses.
        let mut raw = api_key_raw();
        raw["wire_dialect"] = json!("responses");
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.effective_wire_dialect(), WireDialect::Responses);
    }

    #[test]
    fn apply_update_rejects_immutable_auth_keys() {
        let base = LlmParams::parse(&api_key_raw()).unwrap();
        for key in [
            "auth",
            "auth_ref",
            "oauth_token_endpoint",
            "oauth_client_id",
            "oauth_originator",
            // P14: frozen with the family, see IMMUTABLE_PARAM_KEYS.
            "oauth_client_version",
        ] {
            let update = json!({key: "whatever"}).as_object().unwrap().clone();
            let err = base.apply_update(&update).unwrap_err();
            assert!(
                matches!(err, super::ParamUpdateError::Immutable(ref k) if k == key),
                "key {key} must be immutable, got {err:?}"
            );
        }
    }

    /// GH #853: `wire_dialect` changes at run time, and the `auth` pairing is
    /// checked again on every update — a subscription credential still never
    /// goes out on chat-completions.
    #[test]
    fn wire_dialect_is_mutable_and_the_auth_pairing_still_holds() {
        let base = LlmParams::parse(&api_key_raw()).unwrap();
        let update = json!({"wire_dialect": "responses"})
            .as_object()
            .unwrap()
            .clone();
        let (merged, _) = base.apply_update(&update).expect("mutable since GH #853");
        assert_eq!(merged.effective_wire_dialect(), WireDialect::Responses);

        let oauth = LlmParams::parse(&oauth_raw()).unwrap();
        let back = json!({"wire_dialect": "chat_completions"})
            .as_object()
            .unwrap()
            .clone();
        assert!(matches!(
            oauth.apply_update(&back),
            Err(super::ParamUpdateError::Invalid(_))
        ));
    }

    // ───── GH #853: base_url_allow, model_prompt, backstop rule ─────

    #[test]
    fn base_url_allow_is_immutable_and_its_entries_must_be_origins() {
        let base = LlmParams::parse(&api_key_raw()).unwrap();
        let upd = json!({"base_url_allow": ["http://127.0.0.1:1"]})
            .as_object()
            .unwrap()
            .clone();
        assert!(matches!(
            base.apply_update(&upd),
            Err(super::ParamUpdateError::Immutable(ref k)) if k == "base_url_allow"
        ));
        for bad in [
            "127.0.0.1:1",
            "ftp://127.0.0.1",
            "http://u:p@127.0.0.1",
            "http://127.0.0.1/v1",
        ] {
            let mut raw = api_key_raw();
            raw["base_url_allow"] = json!([bad]);
            assert!(LlmParams::parse(&raw).is_err(), "{bad} is not an origin");
        }
        let mut raw = api_key_raw();
        raw["base_url_allow"] = json!(["https://example.com", "http://127.0.0.1:8000/"]);
        assert!(LlmParams::parse(&raw).is_ok());
    }

    /// Review of L2, M-5: an entry spelled with its default port or with
    /// capitals in the host names the same origin and is taken, compared in
    /// normal form -- it no longer fails at birth with "must be an origin".
    #[test]
    fn an_allow_entry_in_another_spelling_of_the_same_origin_is_taken() {
        let mut raw = api_key_raw();
        raw["base_url"] = json!("http://127.0.0.1:1/v1");
        raw["base_url_allow"] = json!(["HTTPS://Host.Example:443", "http://h.example:80/"]);
        let p = LlmParams::parse(&raw).expect("an origin in another spelling is an origin");
        for url in ["https://host.example/v1", "http://h.example/x"] {
            let upd = json!({"base_url": url}).as_object().unwrap().clone();
            let (merged, _) = p.apply_update(&upd).map_err(|e| e.detail()).unwrap();
            assert!(
                p.check_run_time_update(Some("http://127.0.0.1:1/v1"), &upd, &merged, None)
                    .is_ok(),
                "{url} is inside the list"
            );
        }
        for bad in ["https://host.example/v1?x=1", "https://host.example#f"] {
            let mut raw = api_key_raw();
            raw["base_url_allow"] = json!([bad]);
            assert!(LlmParams::parse(&raw).is_err(), "{bad} is not an origin");
        }
    }

    #[test]
    fn the_run_time_base_url_guard() {
        let mut raw = api_key_raw();
        raw["base_url"] = json!("http://127.0.0.1:1/v1");
        raw["base_url_allow"] = json!(["http://127.0.0.1:2"]);
        let p = LlmParams::parse(&raw).unwrap();
        let check = |upd: Value| {
            let upd = upd.as_object().unwrap().clone();
            let (merged, _) = p.apply_update(&upd).map_err(|e| e.detail())?;
            p.check_run_time_update(Some("http://127.0.0.1:1/v1"), &upd, &merged, None)
        };
        assert!(check(json!({"base_url": "http://127.0.0.1:2/x"})).is_ok());
        assert!(
            check(json!({"base_url": "http://127.0.0.1:1/v1"})).is_ok(),
            "the start value is always allowed back"
        );
        let err = check(json!({"base_url": "http://127.0.0.1:3/v1"})).unwrap_err();
        assert!(
            err.contains("http://127.0.0.1:3") && err.contains("base_url_allow"),
            "{err}"
        );
        let err = check(json!({"base_url": "http://user:secret@127.0.0.1:2/v1"})).unwrap_err();
        assert!(!err.contains("secret"), "no userinfo in a detail: {err}");
        assert!(check(json!({"base_url": null})).is_err());

        let fixed = LlmParams::parse(&api_key_raw()).unwrap();
        let upd = json!({"base_url": "http://127.0.0.1:2/v1"})
            .as_object()
            .unwrap()
            .clone();
        let (merged, _) = fixed.apply_update(&upd).unwrap();
        let err = fixed
            .check_run_time_update(None, &upd, &merged, None)
            .unwrap_err();
        assert!(err.contains("fixed at run time"), "{err}");
    }

    #[test]
    fn the_backstop_rule_is_ten_seconds_and_ten_percent() {
        assert_eq!(backstop_shortfall(120_000, Some(180_000)), None);
        assert_eq!(backstop_shortfall(170_000, Some(180_000)), Some(187_000));
        assert_eq!(backstop_shortfall(50_000, Some(59_999)), Some(60_000));
        assert_eq!(
            backstop_shortfall(999_999, None),
            None,
            "no backstop, no inversion"
        );
    }

    #[test]
    fn the_package_keys_are_known_and_mutable() {
        for key in MODEL_PACKAGE_KEYS {
            assert!(KNOWN_PARAM_KEYS.contains(key), "{key} unknown");
            assert!(!IMMUTABLE_PARAM_KEYS.contains(key), "{key} immutable");
        }
    }

    #[test]
    fn model_prompt_has_an_upper_bound() {
        let mut raw = api_key_raw();
        raw["model_prompt"] = json!("x".repeat(MODEL_PROMPT_MAX_BYTES));
        assert!(LlmParams::parse(&raw).is_ok());
        raw["model_prompt"] = json!("x".repeat(MODEL_PROMPT_MAX_BYTES + 1));
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("model_prompt"), "{err}");
    }

    // ───── GH #858: requirement ─────

    #[test]
    fn requirement_is_prose_with_an_upper_bound_and_no_effect_by_default() {
        let p = LlmParams::parse(&api_key_raw()).unwrap();
        assert_eq!(p.requirement, None, "no implicit requirement");
        let mut raw = api_key_raw();
        raw["requirement"] = json!("r".repeat(REQUIREMENT_MAX_BYTES));
        assert!(LlmParams::parse(&raw).is_ok());
        raw["requirement"] = json!("r".repeat(REQUIREMENT_MAX_BYTES + 1));
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("requirement"), "{err}");
    }

    #[test]
    fn requirement_is_known_and_immutable() {
        assert!(KNOWN_PARAM_KEYS.contains(&"requirement"));
        assert!(!MODEL_PACKAGE_KEYS.contains(&"requirement"));
        let mut raw = api_key_raw();
        raw["requirement"] = json!("Answers briefly.");
        let p = LlmParams::parse(&raw).unwrap();
        let upd = json!({"requirement": "Something else."})
            .as_object()
            .unwrap()
            .clone();
        assert!(matches!(
            p.apply_update(&upd),
            Err(super::ParamUpdateError::Immutable(ref k)) if k == "requirement"
        ));
    }

    /// P14 — the param exists, is optional, and carries no implicit value.
    #[test]
    fn parse_accepts_oauth_client_version_and_defaults_to_none() {
        let raw = json!({"provider":"openai","model":"gpt-5.6-luna",
                         "auth":"oauth_subscription","auth_ref":"/tmp/a.json"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(
            p.oauth_client_version, None,
            "no implicit version in params"
        );

        let raw = json!({"provider":"openai","model":"gpt-5.6-luna",
                         "auth":"oauth_subscription","auth_ref":"/tmp/a.json",
                         "oauth_client_version":"0.147.0"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.oauth_client_version.as_deref(), Some("0.147.0"));
    }

    /// P14 / R2 — immutable like the rest of the auth family. A runtime overlay
    /// would live in `cell.db` and silently outrank a later `config.json` fix,
    /// on exactly the value one fixes when the backend moves its version floor.
    #[test]
    fn oauth_client_version_update_is_rejected() {
        let raw = json!({"provider":"openai","model":"gpt-5.6-luna",
                         "auth":"oauth_subscription","auth_ref":"/tmp/a.json"});
        let p = LlmParams::parse(&raw).unwrap();
        let update = json!({"oauth_client_version":"0.148.0"})
            .as_object()
            .unwrap()
            .clone();
        assert!(
            p.apply_update(&update).is_err(),
            "the auth dimension is frozen as a family"
        );
    }

    #[test]
    fn oauth_endpoint_overrides_are_read() {
        let mut raw = oauth_raw();
        raw["oauth_token_endpoint"] = json!("http://127.0.0.1:9/token");
        raw["oauth_client_id"] = json!("client-abc");
        raw["oauth_originator"] = json!("meclaw_test");
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(
            p.oauth_token_endpoint.as_deref(),
            Some("http://127.0.0.1:9/token")
        );
        assert_eq!(p.oauth_client_id.as_deref(), Some("client-abc"));
        assert_eq!(p.oauth_originator.as_deref(), Some("meclaw_test"));
    }

    #[test]
    fn parse_rejects_non_openai_provider() {
        let raw = json!({"provider": "anthropic", "model": "claude-3", "api_key": "x"});
        let r = LlmParams::parse(&raw);
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(
            err.contains("openai"),
            "error must mention provider constraint: {err}"
        );
    }
}
