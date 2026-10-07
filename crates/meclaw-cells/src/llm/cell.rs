//! Phase-8 LlmCell core — orchestrates parse/persist/translate/wire/emit per
//! handle()-Reihenfolge (Plan § 9). T17-T22 grow this incrementally.

use crate::llm::params::{AuthMode, WireDialect};
use crate::llm::translate::{TranslateError, TranslatedResponse};
use crate::llm::window;
use crate::llm::wire::WireError;
use crate::llm::{
    auth, continuation, latency, output, package, params::LlmParams, state, system_gate,
    tool_scope, translate, translate_decisions, translate_responses, wire,
};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReadError, AttachmentReader};
use meclaw_core::serde_json::Value;
use meclaw_core::{Body, Message, OutputSink, Path};

// GH #457: the `credential_pending` receipt's wording and the grant round —
// since GH #1058 shared by every cell type that spends a grant.
use crate::credential::{CREDENTIAL_PENDING_DETAIL, CredentialSlots, ExpiryCause, ParkOutcome};

/// GH #457: one turn held back while the sealed credential is in flight.
///
/// It carries its own `OutputSink`, not the cell's: a sink is per-message (it
/// stamps `parent_message_id`, `trace_id`, ttl and the input headers onto every
/// emission), so answering a parked turn through the sink of whatever message
/// happened to release it would file the answer under the wrong parent. The
/// turn keeps the sink it arrived with, and the receipt or the answer lands on
/// the trace that asked for it.
struct ParkedTurn {
    /// GH #957: the provider the receipt names in `meta.provider`.
    provider: &'static str,
    msg: Message,
    sink: OutputSink,
    reply_target: meclaw_core::Path,
    started_at_unix_ms: i64,
}

/// GH #457 / GH #1058: what the warden of a round does with the turns it held
/// when the round ends without a box — the deadline line, then every turn's
/// `credential_pending` receipt.
///
/// The warden itself lives in [`crate::credential`] and is shared by every
/// cell type; this callback stays here so the timeout line keeps this cell's
/// target and wording exactly as before the move (lock: `gh1058_the_llm_cell_*`
/// T7). A dropped cell (sleep, death, panic) still hands out the receipts, but
/// writes no timeout line — no deadline passed, as before.
fn expiry_receipts() -> crate::credential::ExpiryFn<ParkedTurn> {
    std::sync::Arc::new(|held: Vec<ParkedTurn>, cause: ExpiryCause| {
        Box::pin(async move {
            if let ExpiryCause::Deadline { wait_ms } = cause {
                tracing::warn!(wait_ms, "llm: the sealed credential did not arrive in time");
            }
            for turn in held {
                emit_credential_pending(&turn).await;
            }
        })
    })
}

/// GH #457: hand one parked turn its `credential_pending` receipt.
///
/// A free function on purpose: the deadline task holds no `&LlmCell` and must
/// not — the cell's state belongs to the cell task alone, and everything this
/// emission needs travels with the turn.
async fn emit_credential_pending(turn: &ParkedTurn) {
    output::emit_error_for(
        turn.provider,
        &turn.sink,
        turn.reply_target.clone(),
        "credential_pending",
        CREDENTIAL_PENDING_DETAIL,
        "auth",
        vec![],
        turn.started_at_unix_ms,
        0,
        None,
        None,
        None,
    )
    .await;
}

/// How a provider call failed, with everything `emit_error` needs.
///
/// Exists so the Responses lane can hand one value back to `handle()` instead
/// of duplicating the emit block per failure mode.
struct LaneFailure {
    /// UBF `error_code` (the closed spec enum).
    code: &'static str,
    /// `meta.error.source`.
    source: &'static str,
    /// `meta.error.detail` — never contains a credential.
    detail: String,
    /// P10 fine-grained kind for `meta.error` (plan D10).
    extra: Option<meclaw_core::serde_json::Map<String, Value>>,
}

impl LaneFailure {
    fn from_wire(err: &WireError) -> Self {
        Self {
            code: wire::wire_error_to_code(err),
            source: "wire",
            detail: format!("wire: {err:?}"),
            extra: wire::wire_error_meta(err),
        }
    }
    fn from_translate(err: &TranslateError, source: &'static str) -> Self {
        Self {
            code: translate::translate_error_to_code(err),
            source,
            detail: format!("translate: {err:?}"),
            // GH #999: the new answer checks name their kind.
            extra: translate::translate_error_meta(err),
        }
    }
}

/// Resolve base URL + path for the Responses dialect.
///
/// `params.base_url` wins when set (cell-types Z.132) — that is also how tests
/// point the cell at a fake server.
fn responses_url(params: &LlmParams) -> String {
    let default_base = match params.auth {
        AuthMode::OauthSubscription => auth::DEFAULT_SUBSCRIPTION_BASE_URL,
        AuthMode::ApiKey => wire::OPENAI_DEFAULT_BASE_URL,
    };
    crate::llm::params::endpoint_url(
        params.base_url.as_deref().unwrap_or(default_base),
        wire::OPENAI_RESPONSES_PATH,
    )
}

/// Run one Responses-dialect call, including the auth state machine.
///
/// State machine (plan § 5.1): get token → POST → on 401 ask the broker to
/// refresh (passing the generation we used, so a concurrent refresher wins
/// instead of racing) → retry **exactly once** → typed error. No backoff loop,
/// no failover: failover is topology (cell-types Z.166).
///
/// `wire_out` receives what the provider call(s) cost (GH #124). It stays
/// `None` when the lane failed before reaching the wire — a credential the
/// broker could not produce never touched the provider.
async fn run_responses_lane(
    cell: &LlmCell,
    request_json: &Value,
    timeout: std::time::Duration,
    wire_out: &mut Option<wire::WireTimings>,
) -> Result<TranslatedResponse, LaneFailure> {
    let url = responses_url(&cell.params);
    let attribution = translate::build_attribution_headers(&cell.params);

    let body_text = match cell.params.auth {
        AuthMode::ApiKey => {
            // An empty api_key is no api_key (GH #271) — `None` here means the
            // call goes out with no `Authorization` header at all. The
            // precedence and the empty-is-absent rule both live in `bearer()`
            // (GH #271, and since GH #421 the vault-delivered credential wins
            // over the static one).
            let bearer = cell.bearer();
            let mut headers = translate_responses::build_responses_headers(&cell.params, None);
            headers.extend(attribution);
            let (result, timings) = wire::call_responses_timed(
                &cell.http,
                &url,
                bearer,
                &headers,
                request_json,
                timeout,
            )
            .await;
            *wire_out = Some(timings);
            result.map_err(|e| LaneFailure::from_wire(&e))?
        }
        AuthMode::OauthSubscription => {
            call_with_oauth(cell, &url, &attribution, request_json, timeout, wire_out).await?
        }
    };

    // The subscription backend always streams; a metered endpoint or proxy may
    // answer plain JSON. Sniff the body rather than trusting content-type.
    //
    // A real stream opens with an `event:` line, not `data:` — verified against
    // api.openai.com on 2026-08-09, where sniffing for `data:` alone made the
    // first live smoke fail. Accept both openers.
    let head = body_text.trim_start();
    let translated = if head.starts_with("event:") || head.starts_with("data:") {
        translate_responses::parse_responses_sse(&body_text)
    } else {
        match meclaw_core::serde_json::from_str::<Value>(&body_text) {
            Ok(json) => {
                // GH #75: same shape on this dialect — a 2xx body that carries
                // only an `error` object is the provider's failure signal, not
                // a response the translate stage should try to read.
                if let Some(e) = wire::classify_in_body_error(&json) {
                    return Err(LaneFailure::from_wire(&e));
                }
                translate_responses::parse_responses_response(&json)
            }
            Err(e) => Err(TranslateError::ResponseShape(format!(
                "response was neither SSE nor JSON: {e}"
            ))),
        }
    };
    translated.map_err(|e| LaneFailure::from_translate(&e, "parse"))
}

/// The oauth_subscription branch: token → call → (401 → refresh → one retry).
///
/// `wire_out` accumulates BOTH attempts when the retry fires (GH #124), so a
/// 401-refresh ladder shows up as `wire_attempts: 2` with the summed wall
/// clock instead of silently doubling the apparent handle duration.
async fn call_with_oauth(
    cell: &LlmCell,
    url: &str,
    attribution: &[(String, String)],
    request_json: &Value,
    timeout: std::time::Duration,
    wire_out: &mut Option<wire::WireTimings>,
) -> Result<String, LaneFailure> {
    let auth_ref = cell.params.auth_ref.as_deref().unwrap_or_default();
    let endpoint = cell
        .params
        .oauth_token_endpoint
        .as_deref()
        .unwrap_or(auth::DEFAULT_OAUTH_TOKEN_ENDPOINT);
    let client_id = cell
        .params
        .oauth_client_id
        .as_deref()
        .unwrap_or(auth::DEFAULT_OAUTH_CLIENT_ID);

    let wire_auth_failure =
        |e: auth::AuthError| LaneFailure::from_wire(&WireError::Auth(e.clone()));

    let token = crate::llm::token_broker::get_token(auth_ref, endpoint, client_id, None)
        .await
        .map_err(wire_auth_failure)?;

    let call = |bearer: String, account: Option<String>| {
        let mut headers =
            translate_responses::build_responses_headers(&cell.params, account.as_deref());
        headers.extend(attribution.to_vec());
        async move {
            // The broker's token goes through verbatim: it never crossed a
            // params boundary, so the GH #271 emptiness rule (which lives at
            // the `params.api_key` call sites) does not apply to it.
            wire::call_responses_timed(
                &cell.http,
                url,
                Some(&bearer),
                &headers,
                request_json,
                timeout,
            )
            .await
        }
    };

    let (first, first_timings) = call(token.access_token.clone(), token.account_id.clone()).await;
    *wire_out = Some(first_timings);
    match first {
        Ok(text) => Ok(text),
        Err(WireError::Unauthorized) => {
            // Pass the generation we used: if another cell already refreshed
            // past it, the broker hands us their token instead of rotating
            // again (which would earn `refresh_token_reused`).
            let fresh = crate::llm::token_broker::get_token(
                auth_ref,
                endpoint,
                client_id,
                Some(token.generation),
            )
            .await
            .map_err(wire_auth_failure)?;
            let (retried, retry_timings) =
                call(fresh.access_token.clone(), fresh.account_id.clone()).await;
            // Both POSTs were spent on this one logical call — report their sum.
            *wire_out = Some(first_timings.plus_attempt(retry_timings));
            match retried {
                Ok(text) => Ok(text),
                // Still refused after a fresh token — stop. One retry, no loop.
                Err(WireError::Unauthorized) => {
                    Err(LaneFailure::from_wire(&WireError::AuthExpired))
                }
                Err(e) => Err(LaneFailure::from_wire(&e)),
            }
        }
        Err(e) => Err(LaneFailure::from_wire(&e)),
    }
}

/// Phase-8 LlmCell. First production stateful-cell with cell.db.
///
/// Holds `LlmParams` + `reqwest::Client` as fields. The cell.db `Connection`
/// arrives as `&mut`-param from `cell_task_stateful` per Phase-6.5
/// Connection-Ownership-Modell — NOT a field on LlmCell.
///
/// T17: struct + no-op handle (boilerplate). T18 grows handle() with Plan § 9
/// Step 1 (parse input body) + step 2 (system.* UPSERT). T19-T22 grow
/// it further (messages-write, translate, wire, emit).
pub struct LlmCell {
    /// Pre-validated and parsed params from `LlmParams::parse(raw)`.
    pub params: LlmParams,
    /// reqwest client — built in `LlmCellFactory::spawn_cell` (T24), cloned
    /// into the RespawnFn closure.
    pub http: reqwest::Client,
    /// GH #87: read handle on the blob store, present iff this cell declares
    /// `consumes.body.attachments`. `None` means the cell does not consume
    /// attachments and the slot travels past it untouched — the pre-GH-#87
    /// behaviour, byte for byte.
    attachments: Option<AttachmentReader>,
    /// R3 / GH #421 + GH #457, since GH #1058 on the shared module: the
    /// credential slot of `params.credential_grant_id` — the bearer the vault
    /// delivered, the recipient key of the request in flight, and the round
    /// holding the parked turns. Sealed on the wire, opened here, and held
    /// nowhere else — deliberately NOT part of the params overlay, so it never
    /// reaches `cell.db` and never survives a sleep. A woken cell asks again.
    credentials: CredentialSlots<ParkedTurn>,
    /// GH #457: turns the arriving box released, waiting to be run.
    ///
    /// They are NOT run from inside the delivery's own `handle_one` — that
    /// would be `handle_one` calling itself, and an async fn that awaits itself
    /// has no finite size. `handle` drains this queue after the delivery is
    /// done, which is the same order with a flat call stack.
    released: std::collections::VecDeque<ParkedTurn>,
    /// GH #853: the START value — the birth params (`config.json`, already
    /// `${VAR}`-substituted). `$reset` falls back to it, and the params line
    /// names a key's source against it.
    start: Value,
    /// GH #853: the run-time overlay the effective params were built from —
    /// the same pairs `cell.db.params` holds.
    overlay: meclaw_core::serde_json::Map<String, Value>,
    /// GH #853: the cell's resolved backstop (`cell.message_timeout`), in ms,
    /// so a run-time `external_timeout_ms` is held to the rule the shipped
    /// templates are gated on. `None` = no backstop (or not told).
    message_timeout_ms: Option<u64>,
}

impl LlmCell {
    /// Implementation detail — production entry point is `LlmCellFactory`;
    /// direct construction is `pub` only so tests/integration tests can
    /// drive the cell without the full Colony.
    #[doc(hidden)]
    pub fn new(params: LlmParams, http: reqwest::Client) -> Self {
        // OR-VG-4 (GH #1058): a grant and a literal key together — the literal
        // is never presented, not even while the box is missing (the 22.09.
        // ruling: no `${VAR}` fallback behind a grant). Said once per birth,
        // naming the param and never its value.
        if crate::credential::literal_is_ignored(
            params.credential_grant_id.as_deref(),
            params.api_key.as_deref(),
        ) {
            tracing::warn!(
                "llm: params.api_key is ignored because params.credential_grant_id is set — the \
                 bearer comes from the vault only; remove api_key from this instance (GH #1058)"
            );
        }
        Self {
            http,
            attachments: None,
            credentials: CredentialSlots::new(
                params.credential_wait_ms,
                params.credential_wait_max,
                expiry_receipts(),
            ),
            released: std::collections::VecDeque::new(),
            // A cell built from parsed params alone treats them as its start
            // value; the factory uses [`Self::restored`] with the real birth.
            start: meclaw_core::serde_json::to_value(&params).unwrap_or_default(),
            overlay: meclaw_core::serde_json::Map::new(),
            message_timeout_ms: None,
            params,
        }
    }

    /// GH #853: the cell as the factory births it on wake and respawn — the
    /// birth params as start value, the `cell.db` overlay replayed over them.
    /// Writes the params line, so a restore is as visible as an update.
    #[doc(hidden)]
    pub fn restored(
        conn: &rusqlite::Connection,
        birth: &Value,
        http: reqwest::Client,
    ) -> Result<Self, String> {
        let pairs = crate::params_overlay::read_params_overlay(conn)
            .map_err(|e| format!("read params overlay: {e}"))?;
        let params = LlmParams::parse(&crate::params_overlay::merge_params_overlay(birth, &pairs))?;
        let mut cell = Self::new(params, http);
        cell.start = birth.clone();
        cell.overlay = pairs.into_iter().collect();
        Ok(cell)
    }

    /// GH #853: tell the cell its resolved backstop (the factory's
    /// `message_timeout`).
    #[doc(hidden)]
    #[must_use]
    pub fn with_message_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.message_timeout_ms = timeout.map(|d| d.as_millis() as u64);
        self
    }

    /// The run-time guards over the RESTORED overlay (review of L2, M-1): an
    /// overlay written before 0.46.0 may carry a `base_url` no list allows, or
    /// an `external_timeout_ms` a later `message_timeout` mutation no longer
    /// clears. The restore keeps it in force -- refusing a boot over an old
    /// overlay would take the brain down -- and the factory names what fails
    /// on a warning beside the params line. `Err` = the guard's detail, which
    /// names the key and the rule, and never a secret: a `base_url` refused
    /// for want of a list is not named at all, one whose origin is not on the
    /// list is named by that origin only (no path, no userinfo), and a refused
    /// `external_timeout_ms` by its ms beside the backstop it needs (review
    /// rev-F2, m5; locked in `gh853_the_params_line_is_written_at_the_seam.rs`).
    #[doc(hidden)]
    pub fn restore_check(&self) -> Result<(), String> {
        let start_base_url = self.start.get("base_url").and_then(Value::as_str);
        self.params.check_run_time_update(
            start_base_url,
            &self.overlay,
            &self.params,
            self.message_timeout_ms,
        )
    }

    /// GH #853: the one visible line — effective package, each key's source.
    #[doc(hidden)]
    pub fn params_line(&self, path: &str) -> String {
        package::params_line(path, &self.params, &self.overlay)
    }

    /// OR-KX-C1 (GH #890): the line beside the params line when `breakpoints`
    /// runs as `implicit` on the Responses wire; `None` otherwise.
    #[doc(hidden)]
    pub fn cache_note(&self, path: &str) -> Option<String> {
        package::cache_note(path, &self.params)
    }

    /// GH #853: a params update on the run-time path — `$reset`, the merge over
    /// the START value, and the run-time guards. The detail never names a
    /// value; a credential key is told it needs a mutation.
    fn apply_run_time_update(
        &self,
        update: &meclaw_core::serde_json::Map<String, Value>,
    ) -> Result<crate::params_overlay::OverlayChange<LlmParams>, String> {
        let change = crate::params_overlay::apply_update_over_start::<LlmParams>(
            &self.start,
            &self.overlay,
            update,
        )
        .map_err(|e| match &e {
            crate::params_overlay::ParamUpdateError::Immutable(key)
                if crate::llm::params::CREDENTIAL_KEYS.contains(&key.as_str()) =>
            {
                format!(
                    "{} — provider and credential are fixed at birth; a model package that \
                     needs another one needs a mutation",
                    e.detail()
                )
            }
            _ => e.detail(),
        })?;
        let start_base_url = self.start.get("base_url").and_then(Value::as_str);
        self.params.check_run_time_update(
            start_base_url,
            update,
            &change.merged,
            self.message_timeout_ms,
        )?;
        Ok(change)
    }

    /// Ask the access hive for this cell's bearer credential.
    ///
    /// The message is an ordinary `access.invoke` spend — the broker runs the
    /// same four grant checks it runs for anything else, and the credential's
    /// NAME comes out of the grant, not out of this body. What this cell adds
    /// is the recipient half of a fresh X25519 pair, which it keeps in RAM
    /// until the sealed answer arrives.
    ///
    /// It emits to the reply target like every other emission: the cell knows
    /// no topology, and the edge that carries `hop.route == "credential_request"`
    /// decides where the request goes.
    async fn ask_for_credential(
        &mut self,
        sink: &OutputSink,
        target: &meclaw_core::Path,
        grant_id: &str,
    ) {
        // GH #1058: the key pair and the request's form come from the shared
        // module (`credential::request_content`, the GH #421 form unchanged).
        let content = match self.credentials.request(grant_id) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "llm: no random source for a credential request");
                return;
            }
        };
        // A closed sink means the colony is going down; there is nothing useful
        // to do about it here and nothing secret in this body.
        let _ = sink
            .push(meclaw_core::CellOutput {
                target: target.clone(),
                content,
            })
            .await;
    }

    /// Refuse a credential delivery. Never echoes a value — there is none to
    /// echo on any of these paths, and the message says so in words rather than
    /// by luck.
    async fn emit_credential_reject(
        &self,
        sink: &OutputSink,
        target: meclaw_core::Path,
        detail: &str,
        started_at_unix_ms: i64,
    ) {
        output::emit_error_for(
            &self.params.provider,
            sink,
            target,
            "invalid_input",
            detail,
            "auth",
            vec![],
            started_at_unix_ms,
            0,
            None,
            None,
            None,
        )
        .await;
    }

    /// R3 / GH #421 + GH #457: park a turn of a cell that spends a grant and
    /// holds no credential yet, and ask the vault once. `None` = parked (or
    /// refused with its receipt); `Some` hands the message back to run now.
    /// One guard for every lane -- both chat dialects and, since GH #957, the
    /// decisions wire.
    async fn park_without_credential(
        &mut self,
        msg: Message,
        sink: &OutputSink,
        reply_target: Path,
        started_at_unix_ms: i64,
        clock: &latency::PhaseClock,
    ) -> Option<(Message, Path)> {
        if self.bearer().is_some() {
            return Some((msg, reply_target));
        }
        let Some(grant) = self
            .params
            .credential_grant_id
            .clone()
            .filter(|g| !g.is_empty())
        else {
            return Some((msg, reply_target));
        };
        let ask_target = reply_target.clone();
        let turn = ParkedTurn {
            provider: if self.params.is_decisions() {
                crate::llm::params::PROVIDER_DECISIONS
            } else {
                "openai"
            },
            msg,
            sink: sink.clone(),
            reply_target,
            started_at_unix_ms,
        };
        // The two knobs are run-time mutable (GH #853); a new round reads them
        // fresh, as `park_turn` did before GH #1058.
        self.credentials.set_bounds(
            self.params.credential_wait_ms,
            self.params.credential_wait_max,
        );
        match self.credentials.park(&grant, turn) {
            ParkOutcome::Asked => {
                self.ask_for_credential(sink, &ask_target, &grant).await;
            }
            ParkOutcome::Parked => {}
            ParkOutcome::Refused(turn) => {
                emit_credential_pending(&turn).await;
                self.log_phases(clock, "credential_pending");
            }
        }
        None
    }

    /// GH #957: one call of a `decisions` cell. The body's `decide` slot is
    /// read (neither `system` nor `messages` is), validated, translated in
    /// `translate_decisions`, sent once without a stream, and answered with
    /// exactly one emission: the whole decision, or an error on the error
    /// path -- never half of one. Nothing reaches `cell.db`: a decision has
    /// no history.
    #[allow(clippy::too_many_arguments)]
    async fn run_decisions(
        &mut self,
        msg: Message,
        content_obj: &meclaw_core::serde_json::Map<String, Value>,
        has_params: bool,
        sink: &OutputSink,
        reply_target: Path,
        started_at_unix_ms: i64,
        mut clock: latency::PhaseClock,
    ) {
        let elapsed = || (unix_ms_now() - started_at_unix_ms).max(0) as u64;
        let request = DecisionsRequest {
            messages: content_obj
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            decide: content_obj.get("decide").cloned(),
        };
        // A params-only push (the registry's `{system: {}, params}` form) was
        // applied above and is answered with silence, as on the chat lanes.
        if has_params && !content_obj.contains_key("decide") {
            return;
        }
        if content_obj.contains_key("attachments") {
            decisions_error(
                sink,
                reply_target,
                &request,
                "decisions_unsupported_param",
                "provider 'decisions' takes no attachments",
                "parse",
                started_at_unix_ms,
                0,
                None,
            )
            .await;
            return;
        }
        // GH #966 (E review M-3): the chat slots a decision cannot use are
        // refused like `attachments`, never passed over -- a sender that put
        // facts or tools into `system`, or a `tool_scope`, believed they
        // shaped the answer. An empty `system` is the registry's push form.
        let unused = [
            content_obj
                .get("system")
                .filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
                .map(|_| "system"),
            content_obj
                .get("tool_scope")
                .filter(|v| !v.is_null())
                .map(|_| "tool_scope"),
        ];
        if let Some(slot) = unused.into_iter().flatten().next() {
            decisions_error(
                sink,
                reply_target,
                &request,
                "decisions_unsupported_param",
                &format!("provider 'decisions' takes no {slot} slot"),
                "parse",
                started_at_unix_ms,
                0,
                None,
            )
            .await;
            return;
        }
        let asked = match translate_decisions::parse_decide(content_obj.get("decide")) {
            Ok(a) => a,
            Err(detail) => {
                decisions_error(
                    sink,
                    reply_target,
                    &request,
                    "decide_invalid",
                    &detail,
                    "parse",
                    started_at_unix_ms,
                    0,
                    None,
                )
                .await;
                return;
            }
        };
        // Born on an empty key, filled by the registry or an overlay: until
        // then there is nothing to call, and the caller hears so at once.
        let base = self.params.base_url.clone().unwrap_or_default();
        if self.params.model.trim().is_empty() || base.trim().is_empty() {
            decisions_error(
                sink,
                reply_target,
                &request,
                "decisions_unconfigured",
                "provider 'decisions' needs a model and a base_url before it can be asked",
                "parse",
                started_at_unix_ms,
                0,
                None,
            )
            .await;
            return;
        }
        let request_json = match translate_decisions::build_request(&self.params.model, &asked) {
            Ok(r) => r,
            Err(detail) => {
                decisions_error(
                    sink,
                    reply_target,
                    &request,
                    "decide_invalid",
                    &detail,
                    "translate",
                    started_at_unix_ms,
                    0,
                    None,
                )
                .await;
                return;
            }
        };
        let Some((_msg, reply_target)) = self
            .park_without_credential(msg, sink, reply_target, started_at_unix_ms, &clock)
            .await
        else {
            return;
        };
        clock.translated();
        let url = crate::llm::params::endpoint_url(&base, translate_decisions::DECISIONS_PATH);
        let timeout = std::time::Duration::from_millis(self.params.external_timeout_ms);
        let attribution_headers = translate::build_attribution_headers(&self.params);
        let (wire_result, wire_timings) = wire::call_openai_timed(
            &self.http,
            &url,
            self.bearer(),
            &attribution_headers,
            &request_json,
            timeout,
        )
        .await;
        clock.wired(wire_timings);
        let response_json = match wire_result {
            Ok(json) => json,
            Err(err) => {
                let code = wire::wire_error_to_code(&err);
                decisions_error(
                    sink,
                    reply_target,
                    &request,
                    code,
                    &format!("wire: {err:?}"),
                    "wire",
                    started_at_unix_ms,
                    elapsed(),
                    wire::wire_error_meta(&err),
                )
                .await;
                self.log_phases(&clock, code);
                return;
            }
        };
        let decided = match translate_decisions::parse_response(&response_json, &asked) {
            Ok(d) => d,
            Err(detail) => {
                decisions_error(
                    sink,
                    reply_target,
                    &request,
                    "decision_incomplete",
                    &detail,
                    "parse",
                    started_at_unix_ms,
                    elapsed(),
                    None,
                )
                .await;
                self.log_phases(&clock, "decision_incomplete");
                return;
            }
        };
        let model = decided
            .model
            .clone()
            .unwrap_or_else(|| self.params.model.clone());
        let usage = output::HopUsage {
            tokens_prompt: decided.tokens_prompt,
            tokens_completion: decided.tokens_completion,
            tokens_cached: None,
            tokens_cache_write: None,
            cost: decided.cost,
        };
        output::emit_decision(
            sink,
            reply_target,
            decided.answers,
            &decided.missing,
            usage,
            &model,
            decided.response_id.as_deref().unwrap_or_default(),
            started_at_unix_ms,
            elapsed(),
        )
        .await;
        self.log_phases(&clock, "ok");
    }

    /// GH #1037: continue an answer cut on `length` up to
    /// `length_continuations` times and join the parts; write one line per
    /// continuation and one for an answer that stays cut. A failed
    /// continuation call keeps the text so far: what was answered is not
    /// thrown away because the rest could not be fetched. Every call is held
    /// to the message's backstop (`continuation::continuation_timeout`), so
    /// the backstop never drops an answer a continuation was still adding to.
    #[allow(clippy::too_many_arguments)]
    async fn continue_after_length(
        &self,
        path: &str,
        url: &str,
        headers: &[(String, String)],
        request_json: &meclaw_core::serde_json::Value,
        timeout: std::time::Duration,
        started_at_unix_ms: i64,
        mut answer: translate::TranslatedResponse,
    ) -> translate::TranslatedResponse {
        let budget = self.params.effective_max_tokens();
        let max = self.params.length_continuations;
        let mut round = 0u32;
        while let Some(so_far) = continuation::continuable_text(&answer) {
            if round >= max {
                let why = if max == 0 {
                    "length_continuations is 0"
                } else {
                    "continuation budget spent"
                };
                tracing::warn!(
                    target: continuation::LENGTH_LOG_TARGET,
                    "{}",
                    continuation::cut_line(path, budget, round, why)
                );
                break;
            }
            let elapsed_ms = (unix_ms_now() - started_at_unix_ms).max(0) as u64;
            let call_timeout = match continuation::continuation_timeout(
                timeout,
                self.message_timeout_ms,
                elapsed_ms,
            ) {
                Ok(t) => t,
                Err(why) => {
                    tracing::warn!(
                        target: continuation::LENGTH_LOG_TARGET,
                        "{}",
                        continuation::cut_line(path, budget, round, &why)
                    );
                    break;
                }
            };
            round += 1;
            tracing::warn!(
                target: continuation::LENGTH_LOG_TARGET,
                "{}",
                continuation::continuing_line(path, round, max, budget, so_far.chars().count())
            );
            let mut req = continuation::continuation_request(request_json, &so_far);
            // R-HK-15/16: the continuation carries the text so far, so the
            // window is checked again; a full window keeps the answer cut.
            if let Err(r) = window::apply(&self.params, &mut req, "max_tokens", path) {
                tracing::warn!(
                    target: continuation::LENGTH_LOG_TARGET,
                    "{}",
                    continuation::cut_line(path, budget, round - 1, &r.detail())
                );
                break;
            }
            let (res, _timings) = wire::call_openai_timed(
                &self.http,
                url,
                self.bearer(),
                headers,
                &req,
                call_timeout,
            )
            .await;
            match res
                .map_err(|e| format!("{e:?}"))
                .and_then(|j| translate::parse_openai_response(&j).map_err(|e| format!("{e:?}")))
            {
                Ok(next) => answer = continuation::joined(&so_far, &answer, next),
                Err(e) => {
                    tracing::warn!(
                        target: continuation::LENGTH_LOG_TARGET,
                        "{}",
                        continuation::cut_line(
                            path,
                            budget,
                            round - 1,
                            &format!("continuation call failed: {}", e.chars().take(200).collect::<String>())
                        )
                    );
                    break;
                }
            }
        }
        answer
    }

    /// The credential this cell presents ([`crate::credential::bearer`]).
    ///
    /// A grant is set: ONLY the vault-delivered one — a literal
    /// `params.api_key` beside it is ignored, also while the box is missing
    /// (OR-VG-4, GH #1058; the cell parks and asks instead). No grant: the
    /// literal, the one-release transition for instances that still carry a
    /// key. An empty string is not a credential on either track (GH #271).
    fn bearer(&self) -> Option<&str> {
        let grant = self.params.credential_grant_id.as_deref();
        crate::credential::bearer(
            grant,
            grant.and_then(|g| self.credentials.secret(g)),
            self.params.api_key.as_deref(),
        )
    }

    /// The public half of the recipient key of the request in flight.
    #[cfg(test)]
    fn pending_recipient_hex(&self) -> Option<String> {
        self.params
            .credential_grant_id
            .as_deref()
            .and_then(|g| self.credentials.recipient_hex(g))
    }

    /// GH #87: install the declared-consumer blob reader.
    ///
    /// The factory builds it via `AttachmentReader::for_contract`, which yields
    /// `Some` only for a cell whose contract declares
    /// `consumes.body.attachments`. Passing `None` keeps the cell exactly as it
    /// was before this feature existed.
    #[doc(hidden)]
    #[must_use]
    pub fn with_attachment_reader(mut self, reader: Option<AttachmentReader>) -> Self {
        self.attachments = reader;
        self
    }

    /// Emit the GH #124 phase-summary line for a call that reached a provider
    /// (or died trying), tagged with the cell's dialect and model.
    ///
    /// Called at every terminal point of an inference, so an operating log has
    /// exactly one summary per provider call. `outcome` is `"ok"` or the UBF
    /// `error_code`. Paths that end BEFORE the request is built (body-parse
    /// reject, params reject, Q3 silence) emit nothing: they never called a
    /// provider, and a line for them would dilute the stream this question is
    /// asked of.
    ///
    /// Deliberately called AFTER the emission, not before: `sink.push` awaits a
    /// bounded channel, so a backpressured colony makes the emit itself take
    /// time. Logging first would have hidden exactly that in the blind spot
    /// GH #124 is about — this way it lands in `handle_ms` and therefore in
    /// `unaccounted_ms`.
    fn log_phases(&self, clock: &latency::PhaseClock, outcome: &str) {
        latency::log_summary(
            &clock.finish(),
            self.dialect_name(),
            &self.params.model,
            outcome,
        );
    }

    /// The wire dialect as the string that appears in the instrumentation.
    fn dialect_name(&self) -> &'static str {
        if self.params.is_decisions() {
            return crate::llm::params::PROVIDER_DECISIONS;
        }
        match self.params.effective_wire_dialect() {
            WireDialect::ChatCompletions => "chat_completions",
            WireDialect::Responses => "responses",
        }
    }

    /// Resolve the `attachments[]` slot into provider-native image parts —
    /// `image_url` content parts on chat-completions (GH #87), `input_image`
    /// items on the responses dialect (GH #94). Returns `(error_code, detail)`
    /// on failure — every detail names the attachment and the reason.
    ///
    /// No declaration (no reader) or no slot ⇒ `Ok(vec![])`, and the caller's
    /// request stays byte-identical to the pre-GH-#87 one. The declared MIME
    /// type is checked BEFORE the read so a 40 MB PDF is rejected without ever
    /// entering memory; the sidecar's MIME type — the authority, it is what the
    /// store committed — is checked again after the read and is what the data
    /// URL carries.
    async fn resolve_image_attachments(
        &self,
        slot: Option<&Value>,
    ) -> Result<Vec<Value>, (&'static str, String)> {
        let Some(reader) = &self.attachments else {
            return Ok(Vec::new());
        };
        let Some(entries) = slot.and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        // GH #94: both dialects consume attachments — only the provider-native
        // shape differs, so the shape is the ONLY thing chosen here and the
        // read loop below stays shared (one failure taxonomy, one timeout).
        let to_part: fn(&str, &[u8]) -> Value = match self.params.effective_wire_dialect() {
            WireDialect::ChatCompletions => translate::image_content_part,
            WireDialect::Responses => translate_responses::input_image_item,
        };
        let timeout = std::time::Duration::from_millis(self.params.attachment_timeout_ms);
        let mut parts = Vec::with_capacity(entries.len());
        for entry in entries {
            let raw_id = entry.get("blob_id").and_then(|v| v.as_str()).unwrap_or("");
            let Ok(blob_id) = meclaw_core::Uuid::parse_str(raw_id) else {
                return Err((
                    "invalid_input",
                    format!("attachment '{raw_id}': blob_id is not a UUID"),
                ));
            };
            let declared_mime = entry
                .get("mime_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !is_image_mime(declared_mime) {
                return Err(("invalid_input", non_image_detail(blob_id, declared_mime)));
            }
            let blob = match reader.read(blob_id, timeout).await {
                Ok(b) => b,
                Err(e @ AttachmentReadError::Timeout(..)) => {
                    return Err(("timeout", e.to_string()));
                }
                Err(e) => return Err(("invalid_input", e.to_string())),
            };
            if !is_image_mime(&blob.mime_type) {
                return Err(("invalid_input", non_image_detail(blob_id, &blob.mime_type)));
            }
            parts.push(to_part(&blob.mime_type, &blob.bytes));
        }
        Ok(parts)
    }
}

/// The `llm` cell consumes `image/*` attachments; everything else is a
/// cell-level rejection (GH #87).
fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// Error detail for a non-image attachment — names the attachment and the
/// reason, per the GH #87 failure contract.
fn non_image_detail(blob_id: meclaw_core::Uuid, mime: &str) -> String {
    format!(
        "attachment {blob_id}: mime type '{mime}' is not an image; \
         the llm cell consumes image/* attachments only"
    )
}

/// GH #957: the error path of a `decisions` cell -- the shared error body
/// (`finish_reason` `error`, `error_code`, `meta.error`), `meta.provider`
/// `decisions`, and the request handed on: the caller's `messages` and the
/// incoming `decide` slot ride in the body unchanged, so a failover edge onto
/// a second `decisions` cell has something to decide (review I-1,
/// OR-DP-59; without it the second cell answered `decide_invalid`).
#[allow(clippy::too_many_arguments)]
async fn decisions_error(
    sink: &OutputSink,
    target: Path,
    request: &DecisionsRequest,
    code: &str,
    detail: &str,
    source: &str,
    started_at_unix_ms: i64,
    latency_ms: u64,
    extra: Option<meclaw_core::serde_json::Map<String, Value>>,
) {
    let carry = request.decide.as_ref().map(|d| {
        let mut m = meclaw_core::serde_json::Map::new();
        m.insert("decide".into(), d.clone());
        m
    });
    output::emit_error_carrying(
        crate::llm::params::PROVIDER_DECISIONS,
        sink,
        target,
        code,
        detail,
        source,
        request.messages.clone(),
        started_at_unix_ms,
        latency_ms,
        None,
        None,
        extra,
        None,
        carry,
    )
    .await;
}

/// The body slots a failed decision hands on (see `decisions_error`).
struct DecisionsRequest {
    messages: Vec<Value>,
    decide: Option<Value>,
}

/// Returns the current wall-clock time as Unix milliseconds (i64). Used for
/// the `meta.started_at` field on emissions.
fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis() as i64
}

/// Returns the current wall-clock time as Unix seconds (i64). Used for the
/// `updated_at` / `received_at` columns of `cell.db` SQL writes.
fn unix_secs_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_secs() as i64
}

/// GH #118: answer a refused write into the persistent `system` tree.
///
/// LOUD, never a silent drop — the two halves of "loud" are:
/// * a `WARN` line on the `meclaw::llm::system_gate` target, so an operator
///   watching the daemon sees the refusal even when nobody reads the reply;
/// * a regular error message (`error_code: "invalid_input"`, the same closed
///   enum value the params-update reject uses — the spec's `error_code` list is
///   closed and this is the same class of event: a message asked for something
///   it may not have).
///
/// `input_messages` travels unchanged (Gate-1 pass-through), so a failover edge
/// keyed on `finish_reason == "error"` stays usable. Neither the log line nor
/// the detail ever carries the leaf content.
async fn reject_system_write(
    sink: &OutputSink,
    reply_target: meclaw_core::Path,
    reject: &crate::llm::system_gate::GateReject,
    input_messages: Vec<Value>,
    started_at_unix_ms: i64,
) {
    reject_invalid_system(
        sink,
        reply_target,
        reject.reason(),
        reject.slot(),
        &reject.detail(),
        input_messages,
        started_at_unix_ms,
    )
    .await;
}

/// Refuse a system write, log it value-free, and answer `invalid_input`.
///
/// Shared by the GH #118 gate rejects and the GH #264 marker shape errors:
/// both refuse the WHOLE body (the `messages[]` half included) before anything
/// is written, and both answer the same way — the sender asked for something
/// it may not have, or for something with no reading.
async fn reject_invalid_system(
    sink: &OutputSink,
    reply_target: meclaw_core::Path,
    reason: &str,
    slot: Option<&str>,
    detail: &str,
    input_messages: Vec<Value>,
    started_at_unix_ms: i64,
) {
    tracing::warn!(
        target: "meclaw::llm::system_gate",
        reason,
        slot = slot.unwrap_or("-"),
        "refused a write into the persistent system tree (GH #118/#264)"
    );
    output::emit_error(
        sink,
        reply_target,
        "invalid_input",
        detail,
        "parse",
        input_messages,
        started_at_unix_ms,
        0,
        None,
        None,
        None,
    )
    .await;
}

/// Extract OpenAI-tool objects from a `system_tree.tools.*` sub-object.
///
/// Each leaf `{"text": "<json>"}` under `system.tools` is parsed as the
/// OpenAI tool-object JSON. Keys are visited in alphabetical order so the
/// resulting `Vec` is deterministic — this order IS the menu order a
/// `tool_scope` preserves (GH #845). Each entry keeps its slot key next to the
/// object, because a scope may name a tool by either. Missing `tools` key,
/// non-object `tools`, or empty `tools` all return `Ok(Vec::new())`.
///
/// `concat_system_prompt` (T4) skips `tools` at top-level, so `system_tree`
/// can be passed unchanged to both helpers.
fn extract_tools(system_tree: &Value) -> Result<Vec<(String, Value)>, TranslateError> {
    let Some(obj) = system_tree.as_object() else {
        return Ok(Vec::new());
    };
    let Some(tools_obj) = obj.get("tools").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    let mut keys: Vec<&String> = tools_obj.keys().collect();
    keys.sort();
    let mut out = Vec::with_capacity(keys.len());
    for name in keys {
        let leaf = &tools_obj[name];
        let Some(text) = leaf.get("text").and_then(|v| v.as_str()) else {
            return Err(TranslateError::ToolCallParse(format!(
                "system.tools.{name}: leaf has no text field"
            )));
        };
        let parsed: Value = meclaw_core::serde_json::from_str(text)
            .map_err(|e| TranslateError::ToolCallParse(format!("system.tools.{name}: {e}")))?;
        out.push((name.clone(), parsed));
    }
    Ok(out)
}

#[allow(clippy::manual_async_fn)]
impl StatefulCell for LlmCell {
    /// LlmCell message handler. Walks the Plan § 9 Reihenfolge:
    /// parse input body → persist system.* + messages[] atomically →
    /// Q3-silence-or-translate → wire-call (A-Timeout) → parse response →
    /// emit assistant turn (or `emit_error` on any failure with Gate-1
    /// messages pass-through).
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut meclaw_colony::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            self.handle_one(msg, sink, db).await;
            // GH #457: a sealed delivery releases the turns that were parked
            // waiting for it. They run here, one after the other and in the
            // order they arrived, each through the sink it came in with — the
            // flat drain that keeps `handle_one` from having to call itself.
            //
            // The loop terminates because a released turn finds the credential
            // in RAM and therefore never parks: `released` is filled only from
            // the delivery path, and the delivery is what set it.
            while let Some(turn) = self.released.pop_front() {
                let ParkedTurn { msg, sink, .. } = turn;
                self.handle_one(msg, &sink, db).await;
            }
        }
    }
}

/// GH #862 (R-SN-9): who a `params` slot is for. The registry addresses a
/// push by `hop.subscriber`; the operator's `POST /messages`, `argus`/`steward`
/// and a memory-hive broadcast carry none. The own address is the one the
/// substrate stamps on the sink (`OutputSink::sender_path`, GH #132) -- never
/// message data.
enum PushFor {
    /// No `hop.subscriber`: the operator's message, applied as before.
    Operator,
    /// `hop.subscriber` is this cell's own path.
    Me,
    /// Any other value -- the empty string and a non-string included, so the
    /// rule fails closed: another cell's push. Carries the address as one
    /// line for the detail and the warning.
    Other(String),
}

/// At most this many characters of a foreign address reach a detail or a log
/// line: the value is message data, and a detail is no place for a document.
const PUSH_ADDRESS_MAX_CHARS: usize = 256;

fn push_for(hop: &meclaw_core::serde_json::Map<String, Value>, own: &Path) -> PushFor {
    match hop.get("subscriber") {
        None => PushFor::Operator,
        Some(Value::String(s)) if s == own.as_str() => PushFor::Me,
        Some(v) => PushFor::Other(one_line(v, PUSH_ADDRESS_MAX_CHARS)),
    }
}

/// `v` as one line of at most `max` characters: a string as it is, anything
/// else as its JSON text; a line break or any other control character becomes a
/// space, so a forged address cannot start a second log line.
fn one_line(v: &Value, max: usize) -> String {
    let raw = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

/// The registry's bound on a model id (`MODEL_ID_MAX_CHARS` in
/// `templates/llm-registry/hand`): a longer `model` in a refused slot is named
/// as none rather than carried back cut.
const REFUSED_MODEL_MAX_CHARS: usize = 128;

/// GH #863: a slot this cell did not apply. Addressed to THIS cell, the error names the push on two
/// header keys of its own, so the composite routes it apart from a conversation's errors and the
/// sender's road carries it back; every other refusal keeps the shape it had, byte for byte.
///
/// Measured before the fix (plan R-registry § 1.7-9): a refused push left talky as a conversation
/// error on `./errors`, cogny as `route error`, the memory hive's four llm cells and argus' judge on
/// unconditional edges as a VERDICT, and the registry's `show` kept naming a model the cell did not
/// run. The two keys are new ones rather than a `route` value because the memory-hive cells and
/// `builder/compose` declare `finish_reason` and the brains declare `route` with a closed value list
/// (`meclaw-core/src/contract.rs:258-262` allows an undeclared hop key).
fn refusal_hop(
    push_for: &PushFor,
    own: &Path,
    update: Option<&meclaw_core::serde_json::Map<String, Value>>,
) -> Option<meclaw_core::serde_json::Map<String, Value>> {
    let PushFor::Me = push_for else {
        return None;
    };
    let model = update
        .and_then(|u| u.get("model"))
        .and_then(Value::as_str)
        .filter(|m| m.chars().count() <= REFUSED_MODEL_MAX_CHARS)
        .unwrap_or("");
    Some(meclaw_core::serde_json::Map::from_iter([
        ("refused_subscriber".to_string(), Value::from(own.as_str())),
        ("refused_model".to_string(), Value::from(model)),
    ]))
}

impl LlmCell {
    /// One message, start to finish. The Reihenfolge this doc-comment describes
    /// lives here; [`StatefulCell::handle`] wraps it with the GH #457 drain.
    async fn handle_one(
        &mut self,
        msg: Message,
        sink: &OutputSink,
        db: &mut meclaw_colony::DbConn,
    ) {
        {
            // GH #124: one stopwatch for the whole call. It only reads the
            // monotonic clock at the phase boundaries and is dropped with the
            // call — it can never itself be the reason a call is slow.
            let mut clock = latency::PhaseClock::start();
            let started_at_unix_ms = unix_ms_now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            // Step 1: parse the input body. Validation failures → emit_error
            // with source="parse", input_messages=[], NO DB write.
            let content = match &msg.body {
                Body::Inline(v) => v.clone(),
                Body::Blob(_) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        "provider_error",
                        "invalid input body: blob bodies are Phase-12 deferred",
                        "parse",
                        vec![],
                        started_at_unix_ms,
                        0,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };
            let content_obj = match content.as_object() {
                Some(o) => o,
                None => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        "provider_error",
                        "invalid input body: not a JSON object",
                        "parse",
                        vec![],
                        started_at_unix_ms,
                        0,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };
            // Step 1a (GH #845): the per-request tool scope. Parsed before
            // anything is applied or persisted, so a scope that cannot be read
            // refuses the whole message with no partial effect — silently
            // ignoring it would hand the model the WHOLE menu, the opposite of
            // what the sender asked for.
            let tool_scope = match tool_scope::ToolScope::parse(content_obj.get("tool_scope")) {
                Ok(s) => s,
                Err(detail) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        "invalid_input",
                        &detail,
                        "parse",
                        vec![],
                        started_at_unix_ms,
                        0,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };
            // Step 1b (W4b): params-update slot (config.md § Access l.20).
            // Handled FIRST and strictly: the `params` block is merged into
            // self.params + persisted to cell.db, THEN any system/messages run
            // with the updated params (this call already uses the new model).
            // A params-only message persists and returns silently (analog Q3).
            // All-or-nothing: an immutable/unknown/malformed update is a loud
            // `invalid_input` reject with NO partial apply.
            //
            // GH #853: package keys (`MODEL_PACKAGE_KEYS`) take effect only
            // from a params-only message — a body without `messages`. A cell
            // knows no sender, so the form decides: a slot riding on a turn and
            // naming a package key is not applied at all, the line says so,
            // and the turn runs on (a conversation is never broken off, and it
            // cannot change the model it talks to).
            let has_params = content_obj.contains_key("params");
            let is_turn = content_obj.contains_key("messages");
            // GH #862 (R-SN-9): the registry addresses a push by
            // `hop.subscriber`, and any edge that forwarded its road into
            // another composite's `in_model` door moved that brain too
            // (OR-SN.L2a.14; lock `gh862_a_push_for_another_cell_moves_nothing`
            // `a_forward_into_another_brain_moves_nothing`). A slot addressed
            // to another cell is not applied: a params-only message is refused
            // loudly, and on a turn the slot is skipped whole and the turn
            // runs on (the GH #853 rule). Only the `params` slot is bound; a
            // message without the key is byte-identical to before.
            let push_for = push_for(&msg.headers.hop, sink.sender_path());
            if has_params && let PushFor::Other(to) = &push_for {
                tracing::warn!(
                    target: package::PARAMS_LOG_TARGET,
                    "llm: params {} not applied: a push addressed to '{to}' is not for this cell",
                    sink.sender_path().as_str()
                );
                if !is_turn {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        "invalid_input",
                        &format!(
                            "a params push addressed to '{to}' is not for this cell; \
                             nothing applied"
                        ),
                        "parse",
                        vec![],
                        started_at_unix_ms,
                        0,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            }
            if let Some(params_val) = content_obj
                .get("params")
                .filter(|_| !matches!(push_for, PushFor::Other(_)))
            {
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        // GH #863: an own push refused names itself (no model:
                        // the slot is no object).
                        output::emit_error_with_hop_for(
                            &self.params.provider,
                            sink,
                            reply_target,
                            "invalid_input",
                            "invalid params slot: not a JSON object",
                            "parse",
                            vec![],
                            started_at_unix_ms,
                            0,
                            None,
                            None,
                            None,
                            refusal_hop(&push_for, sink.sender_path(), None),
                        )
                        .await;
                        return;
                    }
                };
                if is_turn && package::touches_package(&update_obj) {
                    tracing::warn!(
                        target: package::PARAMS_LOG_TARGET,
                        "llm: params {} not applied: a turn message cannot change the model \
                         package ({}); send a params-only message",
                        msg.target.as_str(),
                        package::package_keys_in(&update_obj).join(",")
                    );
                } else {
                    match self.apply_run_time_update(&update_obj) {
                        Ok(change) => {
                            let now = unix_secs_now();
                            let set = change.set.clone();
                            let reset = change.reset.clone();
                            let persist_result = db
                                .call(move |conn| {
                                    crate::params_overlay::persist_overlay_change(
                                        conn, &set, &reset, now,
                                    )
                                })
                                .await;
                            if let Err(e) = persist_result {
                                output::emit_error_with_hop_for(
                                    &self.params.provider,
                                    sink,
                                    reply_target,
                                    "provider_error",
                                    &format!("cell.db params write failed: {e}"),
                                    "parse",
                                    vec![],
                                    started_at_unix_ms,
                                    0,
                                    None,
                                    None,
                                    None,
                                    refusal_hop(&push_for, sink.sender_path(), Some(&update_obj)),
                                )
                                .await;
                                return;
                            }
                            // Live apply — this call's inference (if any) uses it.
                            self.params = change.merged;
                            self.overlay = change.overlay;
                            // GH #853: the sight. Every update, `{"params": {}}`
                            // included, answers with this line rather than an
                            // emission — the shipped brains' out-edges would
                            // take an emission for a model answer or dead-letter
                            // it (OR-SN.L2.1).
                            tracing::info!(
                                target: package::PARAMS_LOG_TARGET,
                                "{}",
                                self.params_line(msg.target.as_str())
                            );
                            if let Some(note) = self.cache_note(msg.target.as_str()) {
                                tracing::warn!(target: package::PARAMS_LOG_TARGET, "{note}");
                            }
                        }
                        Err(detail) => {
                            // GH #863: the guard or the immutable rule said no. Addressed
                            // to this cell, the refusal names the push and the model it
                            // named, so the registry learns what this cell does NOT run.
                            output::emit_error_with_hop_for(
                                &self.params.provider,
                                sink,
                                reply_target,
                                "invalid_input",
                                &detail,
                                "parse",
                                vec![],
                                started_at_unix_ms,
                                0,
                                None,
                                None,
                                None,
                                refusal_hop(&push_for, sink.sender_path(), Some(&update_obj)),
                            )
                            .await;
                            return;
                        }
                    }
                }
            }

            // Step 1c (R3 / GH #421): the sealed credential slot. Handled here
            // because it is the one body form that must never touch `cell.db`
            // and never produce an emission — the answer to a delivery is
            // silence, exactly like a params-only message.
            if content_obj.contains_key("sealed") {
                // GH #1058: opening, slot matching and the round's release or
                // refusal live in the shared module; the answers stay here, in
                // the GH #421 wording (`Refusal::detail`).
                match self.credentials.accept_sealed(&content).await {
                    Ok(accepted) => {
                        tracing::info!("llm: bearer credential received sealed and opened in RAM");
                        // GH #457: the round is over. Everything it was holding
                        // moves to the drain queue and is answered by `handle`
                        // after this call.
                        self.released.extend(accepted.released);
                    }
                    Err(refusal) => {
                        let Some(detail) = refusal.detail() else {
                            // A box of a round that expired and was re-asked.
                            // Not the sender's fault and not this round's: the
                            // round in flight keeps waiting for its own box.
                            tracing::warn!(
                                "llm: a sealed box of an earlier credential round arrived late \
                                 and was discarded"
                            );
                            return;
                        };
                        self.emit_credential_reject(
                            sink,
                            reply_target,
                            &detail,
                            started_at_unix_ms,
                        )
                        .await;
                        // GH #457: a delivery that is not usable is a round that
                        // failed. The turns it was holding get their receipt now
                        // — waiting for the deadline would only make the same
                        // answer slower.
                        for turn in refusal.into_refused() {
                            emit_credential_pending(&turn).await;
                        }
                    }
                }
                return;
            }

            // GH #957: the provider fork. A `decisions` cell reads its own
            // `decide` slot and none of the chat path below (no system tree,
            // no history, no tools) -- one `match` on the provider, here,
            // because everything past this point is the chat wire's.
            if self.params.is_decisions() {
                self.run_decisions(
                    msg,
                    content_obj,
                    has_params,
                    sink,
                    reply_target,
                    started_at_unix_ms,
                    clock,
                )
                .await;
                return;
            }

            let system = content_obj.get("system");
            let messages = content_obj.get("messages");
            if system.is_none() && messages.is_none() {
                // params-only message: already persisted above, stay silent.
                if has_params {
                    return;
                }
                output::emit_error_for(
                    &self.params.provider,
                    sink,
                    reply_target,
                    "provider_error",
                    "invalid input body: requires system or messages slot",
                    "parse",
                    vec![],
                    started_at_unix_ms,
                    0,
                    None,
                    None,
                    None,
                )
                .await;
                return;
            }
            // GH #853 (B-8): a package push is `{"system": {}, "params": …}` —
            // the form the registry, `argus` and `steward` send. It writes no
            // system leaf and asks no provider, so it needs no credential:
            // without this return a cell that spends a grant and holds no
            // bearer yet parked the push as a turn, asked the vault, and put a
            // `credential_request` on its lane for a message that was already
            // applied above (measured: gh853 lock
            // `a_params_only_push_never_opens_a_credential_round`).
            if messages.is_none()
                && system.is_none_or(|s| s.as_object().is_some_and(|o| o.is_empty()))
            {
                return;
            }

            // Step 3 (pre): R3 / GH #421 + GH #457. A cell that spends a grant
            // for its credential does not call a provider without one — and it
            // does not throw the turn away either. It PARKS the turn, asks the
            // vault once, and answers the whole batch in order when the box
            // arrives. Placed here because it is the first point at which this
            // is known to be a real inference message, and because it covers
            // BOTH wire dialects with one guard rather than one per lane.
            //
            // The credential lives only in RAM (`self.credentials`), so this is
            // the state after every wake, not once per lifetime — which is why
            // GH #421's "refuse this one turn, serve the next" was a turn lost
            // on every wake, and a chat user's silence.
            //
            // `credential_pending` survives as the receipt for the three ways
            // this can genuinely fail: the round times out (the warden), the
            // delivered box does not open (`Refusal::into_refused`), or the bound is
            // full (here). Every one of them names a message; none of them is
            // silence.
            let Some((_msg, reply_target)) = self
                .park_without_credential(msg, sink, reply_target, started_at_unix_ms, &clock)
                .await
            else {
                return;
            };

            // Step 3 (prep): validate the messages-slot shape. If present but
            // NOT a JSON array → parse-error (same code path as other parse
            // failures, no DB write).
            let messages_array: Option<Vec<meclaw_core::serde_json::Value>> = match messages {
                Some(v) => match v.as_array() {
                    Some(arr) => Some(arr.clone()),
                    None => {
                        output::emit_error_for(
                            &self.params.provider,
                            sink,
                            reply_target,
                            "provider_error",
                            "invalid input body: messages slot must be a JSON array",
                            "parse",
                            vec![],
                            started_at_unix_ms,
                            0,
                            None,
                            None,
                            None,
                        )
                        .await;
                        return;
                    }
                },
                None => None,
            };

            // Steps 2+3: flatten system.* into leaves and persist BOTH
            // system + optional messages atomically in one transaction via
            // `system_first_persist`. Q2 system-first order.
            // GH #264: the same walk also reads the `$replace` marker, which
            // names the roots this message revokes. A malformed marker is a
            // shape error of the body and refuses the whole write — a marker
            // that were silently ignored would hand the writer a revocation
            // that never happened.
            let system_write = match system {
                Some(sys) => match state::parse_system_write(sys) {
                    Ok(w) => w,
                    Err(detail) => {
                        reject_invalid_system(
                            sink,
                            reply_target,
                            "malformed_replace_marker",
                            None,
                            &detail,
                            messages_array.unwrap_or_default(),
                            started_at_unix_ms,
                        )
                        .await;
                        return;
                    }
                },
                None => state::SystemWrite::default(),
            };
            let system_leaves = system_write.leaves;
            let replace_roots = system_write.replace_roots;

            // GH #118: the write gate in front of the persistent system tree.
            // The slot-path and per-leaf-size halves are pure and run HERE, so a
            // refused write never opens a transaction at all. The slot budget
            // needs the current tree and runs inside `system_first_persist`.
            // GH #264 adds the replace roots to the pure half: a root reaches
            // paths this message does not name, so it is gated in its own right.
            let gate = system_gate::SystemGate::from_params(&self.params);
            let pure_verdict = gate
                .check_leaves(&system_leaves)
                .and_then(|()| gate.check_replace_roots(&replace_roots));
            if let Err(reject) = pure_verdict {
                reject_system_write(
                    sink,
                    reply_target,
                    &reject,
                    messages_array.unwrap_or_default(),
                    started_at_unix_ms,
                )
                .await;
                return;
            }

            let now_secs = unix_secs_now();
            let messages_value = messages_array
                .as_ref()
                .map(|m| meclaw_core::serde_json::Value::Array(m.clone()));
            let persist_result = {
                let sys_leaves = system_leaves.clone();
                let roots = replace_roots.clone();
                let msgs_val = messages_value.clone();
                let gate = gate.clone();
                db.call(move |conn| {
                    state::system_first_persist(
                        conn,
                        &gate,
                        &sys_leaves,
                        &roots,
                        msgs_val.as_ref(),
                        now_secs,
                    )
                })
                .await
            };
            if let Err(e) = persist_result {
                match e {
                    state::PersistError::Gate(reject) => {
                        reject_system_write(
                            sink,
                            reply_target,
                            &reject,
                            messages_array.unwrap_or_default(),
                            started_at_unix_ms,
                        )
                        .await;
                    }
                    state::PersistError::Sql(e) => {
                        output::emit_error_for(
                            &self.params.provider,
                            sink,
                            reply_target,
                            "provider_error",
                            &format!("cell.db write failed: {e}"),
                            "parse",
                            messages_array.unwrap_or_default(),
                            started_at_unix_ms,
                            0,
                            None,
                            None,
                            None,
                        )
                        .await;
                    }
                }
                return;
            }

            // GH #124 phase boundary: everything up to here is body parse plus
            // the `cell.db` write transaction. A blocked or slow cell.db shows
            // up as `persist_ms` on the summary line and nowhere else.
            clock.persisted();

            // Step 4: Q3 system-only silence. If no messages slot, the
            // persist above already happened — return without emit/inference.
            let input_messages = match messages_array {
                Some(m) => m,
                None => return,
            };

            // Step 5: build-translate (sync, pure).
            // 5a: read full system-tree from cell.db.
            let system_tree = match db.call(|conn| state::read_system_tree(conn)).await {
                Ok(t) => t,
                Err(e) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        "provider_error",
                        &format!("cell.db read_system_tree failed: {e}"),
                        "parse",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };

            // 5a.2 (GH #95): a cell.db written before GH #86 may still hold an
            // unresolved `{text_id}` leaf — nothing resolves a persisted row
            // any more (the resolver runs at the delivery boundary, and a row
            // read back out of cell.db never crosses it again), and
            // `concat_system_prompt` would silently drop its content from the
            // prompt. Loud-at-read ruling: regular cell error naming the
            // slot(s), no panic, no restart, no provider call. Sits BEFORE
            // extract_tools (uniform story for `tools.*` residue) and before
            // the P10 dialect fork (covers both wires).
            if let Err(detail) = state::check_text_id_residue(&system_tree) {
                output::emit_error_for(
                    &self.params.provider,
                    sink,
                    reply_target,
                    "provider_error",
                    &detail,
                    "translate",
                    input_messages,
                    started_at_unix_ms,
                    (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                    None,
                    None,
                    None,
                )
                .await;
                return;
            }

            // 5b: extract OpenAI tool objects from system.tools.*, narrowed by
            // this request's `tool_scope` (GH #845). The stored menu is never
            // written: the scope lives exactly as long as this call.
            let tools = match extract_tools(&system_tree) {
                Ok(menu) => match &tool_scope {
                    None => menu.into_iter().map(|(_, t)| t).collect(),
                    Some(scope) => {
                        let scoped = scope.apply(menu);
                        if !scoped.unknown.is_empty() {
                            tracing::warn!(
                                unknown = ?scoped.unknown,
                                "llm: tool_scope names tools the menu does not carry; ignored"
                            );
                        }
                        scoped.tools
                    }
                },
                Err(e) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        translate::translate_error_to_code(&e),
                        &format!("translate: {e:?}"),
                        "translate",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };

            // 5c: concat system-prompt (skips tools-subtree at top-level).
            // Infallible since GH #86: the only failure it ever had was an
            // unresolved `{text_id}` leaf, and the substrate resolves those at
            // the delivery boundary now.
            // GH #853: the model's own block goes FIRST, apart from the persona.
            let system_string = translate::compose_system_prompt(
                self.params.model_prompt.as_deref(),
                &system_tree,
                &self.params.system_order,
            );

            // 5c.2 (GH #87 / GH #94): resolve declared `attachments[]` into
            // dialect-native image parts. A cell without the declaration holds
            // no reader, gets an empty vector here and stays byte-identical to
            // pre-#87. The read is I/O and carries its own operation timeout
            // (A) inside `AttachmentReader::read`.
            let image_parts = match self
                .resolve_image_attachments(content_obj.get("attachments"))
                .await
            {
                Ok(parts) => parts,
                Err((code, detail)) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        code,
                        &detail,
                        "parse",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };
            // GH #124: kept before the vector is moved into the request, so the
            // DEBUG detail line can name how many images this call carried.
            let image_part_count = image_parts.len();

            // P10 dialect fork. Everything above (system tree, tools, system
            // prompt) is dialect-neutral and shared; below this point the two
            // wires diverge. The chat-completions branch continues UNCHANGED —
            // pinned by `llm_chat_completions_wire_regression`.
            if self.params.effective_wire_dialect() == WireDialect::Responses {
                let timeout = std::time::Duration::from_millis(self.params.external_timeout_ms);
                let mut wire_timings: Option<wire::WireTimings> = None;
                let outcome = match translate_responses::build_responses_request(
                    &self.params,
                    &system_string,
                    &input_messages,
                    &tools,
                ) {
                    Ok(mut request_json) => {
                        // GH #890: the cell's cache key, keyed by its own
                        // path. No-op under `cache_mode: "off"`.
                        translate_responses::apply_cache_wire(
                            &mut request_json,
                            &self.params,
                            sink.sender_path().as_str(),
                        );
                        // GH #94: fold the resolved images into the typed
                        // input[]. No-op for an empty vector.
                        translate_responses::attach_input_images(
                            &mut request_json,
                            &input_messages,
                            image_parts,
                        );
                        // R-HK-15/16: the window, as on the chat wire.
                        if let Err(r) = window::apply(
                            &self.params,
                            &mut request_json,
                            "max_output_tokens",
                            sink.sender_path().as_str(),
                        ) {
                            Err(LaneFailure {
                                code: "invalid_input",
                                source: "window",
                                detail: r.detail(),
                                extra: Some(r.meta()),
                            })
                        } else {
                            // GH #124: same phase boundary as the chat lane.
                            clock.translated();
                            if tracing::enabled!(target: latency::LATENCY_TARGET, tracing::Level::DEBUG)
                            {
                                latency::log_request_detail(
                                    self.dialect_name(),
                                    meclaw_core::serde_json::to_string(&request_json)
                                        .map(|s| s.len())
                                        .unwrap_or(0),
                                    input_messages.len(),
                                    tools.len(),
                                    image_part_count,
                                    system_string.len(),
                                );
                            }
                            run_responses_lane(self, &request_json, timeout, &mut wire_timings)
                                .await
                        }
                    }
                    Err(e) => Err(LaneFailure::from_translate(&e, "translate")),
                };
                if let Some(t) = wire_timings {
                    clock.wired(t);
                }
                let latency_ms = (unix_ms_now() - started_at_unix_ms).max(0) as u64;
                match outcome {
                    Ok(t) => {
                        let usage = output::HopUsage::of(&t);
                        output::emit_assistant_turn(
                            sink,
                            reply_target,
                            t.assistant_turn,
                            &t.finish_reason,
                            usage,
                            // GH #890: when the cache goes cold, and the window.
                            output::HopCache::of(&self.params),
                            &t.model,
                            &t.response_id,
                            started_at_unix_ms,
                            latency_ms,
                        )
                        .await;
                        self.log_phases(&clock, "ok");
                    }
                    Err(f) => {
                        let code = f.code;
                        // GH #999: a call that reached the provider names what
                        // its request left out and what it sent unchecked; a
                        // request that was never built names nothing.
                        let request_hop = (f.source != "translate" && f.source != "window")
                            .then(|| output::HopCache::request_hop(&self.params))
                            .flatten();
                        output::emit_error_with_hop_for(
                            &self.params.provider,
                            sink,
                            reply_target,
                            f.code,
                            &f.detail,
                            f.source,
                            input_messages,
                            started_at_unix_ms,
                            latency_ms,
                            None,
                            None,
                            f.extra,
                            request_hop,
                        )
                        .await;
                        self.log_phases(&clock, code);
                    }
                }
                return;
            }

            // 5d: build OpenAI Chat-Completions request body.
            let mut request_json = match translate::build_openai_request(
                &self.params,
                &system_string,
                &input_messages,
                &tools,
            ) {
                Ok(mut r) => {
                    // GH #890: the cache wire -- a key or two marks, by
                    // `cache_mode`; nothing at all under `off`. Before the
                    // images, so a mark sits on the text it closes.
                    translate::apply_cache_wire(&mut r, &self.params, sink.sender_path().as_str());
                    // GH #87: fold the resolved images into the message of
                    // the last user turn (GH #847: never a peer turn). No-op
                    // for an empty vector.
                    translate::attach_image_parts(&mut r, &input_messages, image_parts);
                    r
                }
                Err(e) => {
                    output::emit_error_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        translate::translate_error_to_code(&e),
                        &format!("translate: {e:?}"),
                        "translate",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };
            // 5e (R-HK-15/16): the window -- refuse above `input_hard` (no
            // model call), clamp the budget to what `context_window` leaves.
            if let Err(r) = window::apply(
                &self.params,
                &mut request_json,
                "max_tokens",
                sink.sender_path().as_str(),
            ) {
                output::emit_error_for(
                    &self.params.provider,
                    sink,
                    reply_target,
                    "invalid_input",
                    &r.detail(),
                    "window",
                    input_messages,
                    started_at_unix_ms,
                    (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                    None,
                    None,
                    Some(r.meta()),
                )
                .await;
                return;
            }
            // GH #124 phase boundary: system read-back, tools, prompt concat,
            // attachment reads + base64 and the request build all land in
            // `translate_ms`. The DEBUG line explains a large one (a megabyte
            // of images, a thousand-turn history) with sizes only — never with
            // conversation content.
            clock.translated();
            if tracing::enabled!(target: latency::LATENCY_TARGET, tracing::Level::DEBUG) {
                latency::log_request_detail(
                    self.dialect_name(),
                    meclaw_core::serde_json::to_string(&request_json)
                        .map(|s| s.len())
                        .unwrap_or(0),
                    input_messages.len(),
                    tools.len(),
                    image_part_count,
                    system_string.len(),
                );
            }

            // Step 6: HTTP call (async, A timeout via call_openai's
            // internal tokio::time::timeout wrapper).
            let url = crate::llm::params::endpoint_url(
                self.params
                    .base_url
                    .as_deref()
                    .unwrap_or(wire::OPENAI_DEFAULT_BASE_URL),
                wire::OPENAI_CHAT_COMPLETIONS_PATH,
            );
            let timeout = std::time::Duration::from_millis(self.params.external_timeout_ms);
            // A4: the Translate boundary decides each param's wire destination —
            // attribution params become HTTP request headers (body-params stay
            // in `request_json`).
            let attribution_headers = translate::build_attribution_headers(&self.params);
            let (wire_result, wire_timings) = wire::call_openai_timed(
                &self.http,
                &url,
                // Precedence and the empty-is-absent rule both live in
                // `bearer()` (GH #271, GH #421).
                self.bearer(),
                &attribution_headers,
                &request_json,
                timeout,
            )
            .await;
            clock.wired(wire_timings);
            let response_json = match wire_result {
                Ok(json) => json,
                Err(err) => {
                    let code = wire::wire_error_to_code(&err);
                    output::emit_error_with_hop_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        code,
                        &format!("wire: {err:?}"),
                        "wire",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        None,
                        None,
                        // GH #75: the fine kind travels with the failure on this
                        // lane too. `None` for every pre-P10 variant, so the
                        // legacy emitted body stays byte-identical.
                        wire::wire_error_meta(&err),
                        // GH #999 (F5/F6, lifts OR-LP.LR.1): a refused request
                        // names what it left out and what it sent unchecked.
                        output::HopCache::request_hop(&self.params),
                    )
                    .await;
                    self.log_phases(&clock, code);
                    return;
                }
            };

            // Step 7: parse-translate the response (sync, pure). On parse fail
            // we still defensively try to surface model/response_id from the
            // raw response so the error-meta carries them when available.
            let translated = match translate::parse_openai_response(&response_json) {
                Ok(t) => t,
                Err(e) => {
                    let resp_model = response_json.get("model").and_then(|v| v.as_str());
                    let resp_id = response_json.get("id").and_then(|v| v.as_str());
                    output::emit_error_with_hop_for(
                        &self.params.provider,
                        sink,
                        reply_target,
                        translate::translate_error_to_code(&e),
                        &format!("translate response parse: {e:?}"),
                        "parse",
                        input_messages,
                        started_at_unix_ms,
                        (unix_ms_now() - started_at_unix_ms).max(0) as u64,
                        resp_model,
                        resp_id,
                        // GH #999: the answer checks (F1/F2) name their kind;
                        // `None` for every older parse failure.
                        translate::translate_error_meta(&e),
                        output::HopCache::request_hop(&self.params),
                    )
                    .await;
                    self.log_phases(&clock, translate::translate_error_to_code(&e));
                    return;
                }
            };

            // Step 7b (GH #1037): a `length` finish is continued, never cut
            // in silence -- see `continuation.rs` for why the cell owns it.
            let translated = self
                .continue_after_length(
                    sink.sender_path().as_str(),
                    &url,
                    &attribution_headers,
                    &request_json,
                    timeout,
                    started_at_unix_ms,
                    translated,
                )
                .await;

            // Step 8: emit the assistant turn as an atomic UBF body (end).
            let latency_ms = (unix_ms_now() - started_at_unix_ms).max(0) as u64;
            let usage = output::HopUsage::of(&translated);
            output::emit_assistant_turn(
                sink,
                reply_target,
                translated.assistant_turn,
                &translated.finish_reason,
                usage,
                // GH #890: when the cache goes cold, and the window.
                output::HopCache::of(&self.params),
                &translated.model,
                &translated.response_id,
                started_at_unix_ms,
                latency_ms,
            )
            .await;
            self.log_phases(&clock, "ok");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    fn mk_cell() -> LlmCell {
        // Default unit-test cell: points at an unbound loopback port + tiny
        // A-timeout. Tests that exercise the inference path see a fast
        // WireError::Network. Mock-server-driven inference lives in
        // tests/phase_8_cell.rs.
        let raw = json!({
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "sk-test",
            "base_url": "http://127.0.0.1:1",
            "external_timeout_ms": 100u64,
        });
        let params = LlmParams::parse(&raw).unwrap();
        let http = reqwest::Client::builder().build().unwrap();
        LlmCell::new(params, http)
    }

    /// GH #271 / #421, changed on purpose by GH #1058 (OR-VG-4): without a
    /// grant the static key is the bearer; with a grant ONLY the delivered
    /// value is — the static key is no fallback while the box is missing. The
    /// full precedence table (empty delivered value included) is pinned on
    /// `credential::bearer` in `tests/gh1058_the_credential_module.rs`; the
    /// delivered value itself only enters through a sealed box
    /// (`tests/gh1058_the_llm_cell_on_the_shared_module.rs`, T10).
    #[test]
    fn the_static_key_is_the_bearer_only_while_no_grant_is_set() {
        let cell = mk_cell();
        assert_eq!(cell.bearer(), Some("sk-test"));
        let mut cell = mk_cell();
        cell.params.credential_grant_id = Some("grant:1058".to_string());
        assert_eq!(cell.bearer(), None, "a grant without its box has no bearer");
        cell.params.credential_grant_id = Some(String::new());
        assert_eq!(cell.bearer(), Some("sk-test"), "an empty grant is no grant");
    }

    fn mk_sink() -> (OutputSink, mpsc::Receiver<meclaw_core::CellEmission>) {
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
    async fn handle_system_only_no_emit_q3_silence() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(
                json!({"system": {"facts": {"x": {"text": "v"}}}}),
            ))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        // DB written.
        let v: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT value FROM system WHERE slot_path='facts.x'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
            })
            .await;
        assert_eq!(v, r#"{"text":"v"}"#);
        // No emission — Q3 silence.
        assert!(
            rx.try_recv().is_err(),
            "system-only input MUST NOT emit (Q3 silence)"
        );
    }

    /// A cell that spends a grant for its credential and has no static key.
    ///
    /// GH #457: `credential_wait_ms` is deliberately short. These tests measure
    /// the parking, not the deadline, and a warden holding a clone of the sink
    /// keeps the receiver open until it exits — so a long default would only
    /// make every drain here wait for it.
    fn credential_cell() -> LlmCell {
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "",
            "credential_grant_id": "grant:abc",
            "base_url": "http://127.0.0.1:1/never-reached",
            "external_timeout_ms": 100u64,
            "credential_wait_ms": 400u64,
        });
        LlmCell::new(
            LlmParams::parse(&raw).expect("parse"),
            reqwest::Client::builder().build().unwrap(),
        )
    }

    fn user_turn() -> meclaw_core::Message {
        MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": "hello"}]}),
            ))
            .build()
    }

    /// R3 / GH #421: the box opens into RAM. GH #457: and the turn that asked
    /// for it is the turn that gets answered — the delivery message itself is
    /// still answered with silence, exactly like a params-only message.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sealed_credential_is_opened_into_ram_and_answered_with_silence() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = credential_cell();
        let (sink, mut rx) = mk_sink();

        // The cell asks, which is what mints the recipient key.
        cell.handle(user_turn(), &sink, &mut db).await;
        let public = cell.pending_recipient_hex().expect("a pair is in flight");
        let sealed = crate::sealed::seal_to(&public, b"sk-or-v1-DELIVERED").expect("seal");

        let msg = MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(json!({"sealed": sealed.to_json()})))
            .build();
        cell.handle(msg, &sink, &mut db).await;

        assert_eq!(cell.bearer(), Some("sk-or-v1-DELIVERED"));
        assert!(
            cell.pending_recipient_hex().is_none(),
            "the ephemeral key is spent"
        );

        drop(sink);
        let mut after = Vec::new();
        while let Some(em) = rx.recv().await {
            after.push(em.content);
        }
        // Exactly two emissions, and neither is a refusal of the user's turn:
        // the credential REQUEST from the first call, and the answer the parked
        // turn produced once the box released it. That answer is a
        // `provider_error` here because the base_url points at a closed port —
        // the point is that the turn reached the wire lane at all, which under
        // GH #421 it never did.
        assert_eq!(after.len(), 2, "unexpected emissions: {after:?}");
        assert_eq!(after[0]["header"]["route"], "credential_request");
        assert_eq!(
            after[1]["header"]["error_code"], "provider_error",
            "the parked turn was answered, not refused: {after:?}"
        );
        assert!(
            !after
                .iter()
                .any(|c| c["header"]["error_code"] == "credential_pending"),
            "GH #457: a turn that got its credential is never `credential_pending`: {after:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_delivered_credential_never_reaches_cell_db() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = credential_cell();
        let (sink, _rx) = mk_sink();
        cell.handle(user_turn(), &sink, &mut db).await;
        let public = cell.pending_recipient_hex().expect("pair");
        let sealed = crate::sealed::seal_to(&public, b"sk-or-v1-DELIVERED").expect("seal");
        cell.handle(
            MessageBuilder::new(Path::new("/llm"))
                .body(Body::Inline(json!({"sealed": sealed.to_json()})))
                .build(),
            &sink,
            &mut db,
        )
        .await;

        // The whole database, dumped as text. A credential in RAM is a decision;
        // a credential on disk would be an accident nobody would notice.
        let dump: String = db
            .call(|c| {
                let mut s = c
                    .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                    .unwrap();
                let tables: Vec<String> = s
                    .query_map([], |r| r.get(0))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                drop(s);
                let mut all = String::new();
                for t in tables {
                    let mut q = c.prepare(&format!("SELECT * FROM \"{t}\"")).unwrap();
                    let cols = q.column_count();
                    let mut rows = q.query([]).unwrap();
                    while let Some(r) = rows.next().unwrap() {
                        for i in 0..cols {
                            all.push_str(
                                &r.get::<_, rusqlite::types::Value>(i)
                                    .map(|v| format!("{v:?}"))
                                    .unwrap_or_default(),
                            );
                        }
                    }
                }
                all
            })
            .await;
        assert!(
            !dump.contains("sk-or-v1-DELIVERED"),
            "the credential was persisted"
        );
        assert!(!dump.contains("DELIVERED"), "{dump}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_box_nobody_asked_for_is_refused_and_changes_nothing() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = credential_cell();
        let (sink, mut rx) = mk_sink();

        let stranger = crate::sealed::RecipientKeypair::generate().expect("keypair");
        let sealed =
            crate::sealed::seal_to(&stranger.public_hex(), b"sk-or-v1-PLANTED").expect("seal");
        cell.handle(
            MessageBuilder::new(Path::new("/llm"))
                .body(Body::Inline(json!({"sealed": sealed.to_json()})))
                .build(),
            &sink,
            &mut db,
        )
        .await;

        assert_eq!(cell.bearer(), None, "nothing was adopted");
        let em = rx.try_recv().expect("a named refusal, not silence");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }

    /// R3 / GH #421: a cell that declares a credential grant and holds no
    /// credential asks for one instead of calling a provider without a key.
    ///
    /// GH #457 changed what happens to the turn that triggered the ask: it is
    /// PARKED, not refused. What the drain below then sees is the request and,
    /// once the (short) deadline passes with no box, the parked turn's
    /// `credential_pending` — the receipt for a vault that did not answer,
    /// which is the only case it is still emitted in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cell_with_a_credential_grant_and_no_credential_asks_before_it_calls() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = credential_cell();
        let (sink, mut rx) = mk_sink();

        let msg = MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": "hello"}]}),
            ))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        drop(sink);

        let mut seen: Vec<meclaw_core::serde_json::Value> = Vec::new();
        while let Some(em) = rx.recv().await {
            seen.push(em.content);
        }
        let ask = seen
            .iter()
            .find(|c| c["header"]["route"] == "credential_request")
            .expect("the cell asked for its credential");
        let args: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(ask["messages"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(args["grant_id"], "grant:abc");
        assert_eq!(args["operation"], "vault.deliver");
        assert_eq!(
            args["payload"]["recipient_key"].as_str().map(str::len),
            Some(64),
            "an X25519 public key is 32 bytes of hex"
        );

        // GH #457: the receipt exists, but it is the DEADLINE's, not the
        // guard's — it arrives after `credential_wait_ms` with no box, and it
        // carries the parked turn's own trace. A second message would not have
        // produced a second request; one round asks once.
        assert!(
            seen.iter()
                .any(|c| c["header"]["error_code"] == "credential_pending"),
            "the parked turn was never accounted for: {seen:?}"
        );
        assert_eq!(
            seen.len(),
            2,
            "one request, one receipt, and nothing else: {seen:?}"
        );
        assert!(
            cell.pending_recipient_hex().is_some(),
            "the private half stays in RAM until the box arrives"
        );
    }

    /// GH #457: one round, one request — however many turns pile up behind it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_turn_joins_the_round_instead_of_asking_again() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = credential_cell();
        let (sink, mut rx) = mk_sink();

        cell.handle(user_turn(), &sink, &mut db).await;
        cell.handle(user_turn(), &sink, &mut db).await;
        cell.handle(user_turn(), &sink, &mut db).await;
        drop(sink);

        let mut seen: Vec<meclaw_core::serde_json::Value> = Vec::new();
        while let Some(em) = rx.recv().await {
            seen.push(em.content);
        }
        assert_eq!(
            seen.iter()
                .filter(|c| c["header"]["route"] == "credential_request")
                .count(),
            1,
            "the vault was asked more than once for one round: {seen:?}"
        );
        assert_eq!(
            seen.iter()
                .filter(|c| c["header"]["error_code"] == "credential_pending")
                .count(),
            3,
            "every parked turn gets its own receipt when the round times out: {seen:?}"
        );
    }

    /// GH #457: the bound is a bound. The turn that does not fit is refused
    /// immediately — it does not wait for the deadline, and it is not dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_turn_past_the_bound_is_refused_at_once_and_the_first_ones_are_answered() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "",
            "credential_grant_id": "grant:abc",
            "base_url": "http://127.0.0.1:1/never-reached",
            "external_timeout_ms": 100u64,
            "credential_wait_ms": 60_000u64,
            "credential_wait_max": 2usize,
        });
        let mut cell = LlmCell::new(
            LlmParams::parse(&raw).expect("parse"),
            reqwest::Client::builder().build().unwrap(),
        );
        let (sink, mut rx) = mk_sink();

        cell.handle(user_turn(), &sink, &mut db).await;
        cell.handle(user_turn(), &sink, &mut db).await;
        // The third does not fit. Its receipt is here, now — the deadline is a
        // minute away and nothing about this turn is going to change.
        cell.handle(user_turn(), &sink, &mut db).await;
        let mut early = Vec::new();
        while let Ok(em) = rx.try_recv() {
            early.push(em.content);
        }
        assert_eq!(
            early
                .iter()
                .filter(|c| c["header"]["error_code"] == "credential_pending")
                .count(),
            1,
            "the overflow turn was not refused on the spot: {early:?}"
        );

        // And the two that did fit are still parked: the box releases them.
        let public = cell.pending_recipient_hex().expect("pair");
        let sealed = crate::sealed::seal_to(&public, b"sk-or-v1-DELIVERED").expect("seal");
        cell.handle(
            MessageBuilder::new(Path::new("/llm"))
                .body(Body::Inline(json!({"sealed": sealed.to_json()})))
                .build(),
            &sink,
            &mut db,
        )
        .await;
        drop(sink);
        let mut late = Vec::new();
        while let Some(em) = rx.recv().await {
            late.push(em.content);
        }
        assert_eq!(
            late.iter()
                .filter(|c| c["header"]["error_code"] == "provider_error")
                .count(),
            2,
            "both parked turns must reach the wire lane after the box: {late:?}"
        );
        assert!(
            !late
                .iter()
                .any(|c| c["header"]["error_code"] == "credential_pending"),
            "a released turn is answered, never refused: {late:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_params_only_persists_overlay_and_no_emit() {
        // W4b (b): a params-only message persists the overlay to cell.db and
        // returns WITHOUT emitting (params-only silence, analog Q3).
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(json!({"params": {"model": "gpt-4o-mini"}})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        // Overlay persisted.
        let stored: String = db
            .call(|conn| {
                conn.query_row("SELECT value FROM params WHERE key='model'", [], |r| {
                    r.get(0)
                })
                .unwrap()
            })
            .await;
        assert_eq!(stored, r#""gpt-4o-mini""#);
        // Live self.params already reflects the update (this call's path).
        assert_eq!(cell.params.model, "gpt-4o-mini");
        // No emission — params-only silence.
        assert!(
            rx.try_recv().is_err(),
            "params-only input MUST NOT emit (silence)"
        );
    }

    /// GH #890: the three cache keys travel as a model package -- a params-only
    /// push sets all three, and `$reset` takes all three back to the start
    /// value, so no cache setting of an earlier model survives the next one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn model_package_carries_the_cache_keys() {
        use crate::llm::params::CacheMode;
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let push = |params: Value| {
            MessageBuilder::new(Path::new("/llm"))
                .body(Body::Inline(json!({"system": {}, "params": params})))
                .build()
        };
        cell.handle(
            push(json!({"cache_mode": "breakpoints", "cache_ttl_s": 3600,
                        "context_window": 64000})),
            &sink,
            &mut db,
        )
        .await;
        assert_eq!(cell.params.cache_mode, CacheMode::Breakpoints);
        assert_eq!(cell.params.cache_ttl_s, 3600);
        assert_eq!(cell.params.context_window, 64_000);
        assert!(rx.try_recv().is_err(), "a push is answered with silence");

        cell.handle(
            push(json!({"$reset": ["cache_mode", "cache_ttl_s", "context_window"]})),
            &sink,
            &mut db,
        )
        .await;
        assert_eq!(cell.params.cache_mode, CacheMode::Off);
        assert_eq!(cell.params.cache_ttl_s, 0);
        assert_eq!(cell.params.context_window, 0);
        let left: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM params", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(left, 0, "the overlay holds nothing of the old package");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_malformed_params_rejects_invalid_input_no_partial() {
        // W4b (d): a malformed params block (valid model + bad temperature type)
        // → loud invalid_input reject, NO partial apply (overlay stays empty).
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(
                json!({"params": {"model": "gpt-4o-mini", "temperature": "hot"}}),
            ))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
        assert_eq!(em.content["meta"]["error"]["source"], "parse");
        // No partial apply: overlay empty AND live params unchanged.
        let count: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM params", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(count, 0, "malformed update must NOT partially persist");
        assert_eq!(cell.params.model, "gpt-4o");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_immutable_param_update_rejects_invalid_input() {
        // W4b (e): updating an immutable field (api_key) → loud invalid_input,
        // no overlay write (secret-hygiene; W4 Authorization-guard extended).
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({"params": {"api_key": "leaked"}})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
        let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
        assert!(
            detail.contains("immutable") && !detail.contains("leaked"),
            "detail must name the rule, never the value: {detail}"
        );
        let count: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM params", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(count, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_params_slot_not_object_rejects_invalid_input() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({"params": "not-an-object"})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
        assert_eq!(em.content["meta"]["error"]["source"], "parse");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_messages_only_writes_last_input_then_wire_error_to_unbound_port() {
        // T20 rewrite of the T19 placeholder: messages-only input now flows
        // through steps 5+6 and hits the wire. `mk_cell` points at an
        // unbound loopback port with a 100ms A-timeout → WireError::Network
        // → emit_error(provider_error, source="wire"). The cell.db write
        // from step 3 must still be visible afterwards.
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msgs = json!([{"origin":"user","type":"text","text":"Hi"}]);
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({"messages": msgs.clone()})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        // cell.db.last_input written (step 3 persisted before the wire call).
        let stored: String = db
            .call(|conn| {
                conn.query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                    r.get(0)
                })
                .unwrap()
            })
            .await;
        let parsed: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed, msgs);
        // Wire-error emit reached the sink.
        let em = rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/observer"));
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["meta"]["error"]["source"], "wire");
        // Gate-1: messages pass-through unchanged.
        assert_eq!(em.content["messages"], json!(msgs));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_messages_not_array_rejects_with_provider_error() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({"messages": "not-an-array"})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["error_code"], "provider_error");
        let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
        assert!(
            detail.contains("must be a JSON array"),
            "detail must mention array requirement: {detail}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_invalid_body_emits_provider_error_and_no_db_write() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!(42)))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/observer"));
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "provider_error");
        assert_eq!(em.content["meta"]["error"]["source"], "parse");
        let count: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(count, 0, "parse-fail must NOT write to cell.db");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_missing_both_slots_emits_provider_error() {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = mk_cell();
        let (sink, mut rx) = mk_sink();
        let msg = MessageBuilder::new(Path::new("/llm"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({"unrelated": "field"})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["error_code"], "provider_error");
        let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
        assert!(
            detail.contains("requires system or messages"),
            "detail must mention missing slots: {detail}"
        );
        let count: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(count, 0);
    }
}
