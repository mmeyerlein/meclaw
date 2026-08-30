//! Phase-8 LlmCell core — orchestrates parse/persist/translate/wire/emit per
//! handle()-Reihenfolge (Plan § 9). T17-T22 grow this incrementally.

use crate::llm::params::{AuthMode, WireDialect};
use crate::llm::translate::{TranslateError, TranslatedResponse};
use crate::llm::wire::WireError;
use crate::llm::{
    auth, latency, output, params::LlmParams, state, system_gate, translate, translate_responses,
    wire,
};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReadError, AttachmentReader};
use meclaw_core::serde_json::Value;
use meclaw_core::{Body, Message, OutputSink};

/// GH #457: the `credential_pending` receipt's wording. One constant because
/// three code paths hand it out — the deadline, a broken delivery, and the
/// overflow — and a receipt that reads differently depending on which of them
/// fired would be three receipts to a reader who only has the log.
const CREDENTIAL_PENDING_DETAIL: &str =
    "the bearer credential was requested from the access hive but did not arrive; retry";

/// GH #457: one turn held back while the sealed credential is in flight.
///
/// It carries its own `OutputSink`, not the cell's: a sink is per-message (it
/// stamps `parent_message_id`, `trace_id`, ttl and the input headers onto every
/// emission), so answering a parked turn through the sink of whatever message
/// happened to release it would file the answer under the wrong parent. The
/// turn keeps the sink it arrived with, and the receipt or the answer lands on
/// the trace that asked for it.
struct ParkedTurn {
    msg: Message,
    sink: OutputSink,
    reply_target: meclaw_core::Path,
    started_at_unix_ms: i64,
}

/// GH #457: the warden of ONE in-flight credential round.
///
/// It is a task that OWNS the parked turns, not a lock over them — `AGENTS.md`
/// forbids `Mutex`/`RwLock`/atomics in cell state, and the substrate's standing
/// answer to "two timelines, one piece of state" is the same one
/// `llm::token_broker` uses: hand the state to a task and talk to it over
/// channels. A turn is therefore owned by exactly one side at every instant —
/// the cell until `try_send`, the warden until it hands the batch back.
///
/// A task rather than a check on the next message, because there may not BE a
/// next message: a broker refusal is routed to the topology's error lane, never
/// back to the asking cell, so a cell that only ever looked at its own inbox
/// would hold the parked turns forever and reproduce the very silence GH #457
/// is about.
struct CredentialWait {
    /// The parking lot. Its CAPACITY is `params.credential_wait_max`, so the
    /// bound is the channel's and `try_send` reports the overflow — there is no
    /// counter anywhere that could disagree with the buffer's real length.
    turns: tokio::sync::mpsc::Sender<ParkedTurn>,
    /// Ask the warden for its batch back: send it the channel to answer on.
    /// `Err` means the warden is gone — the deadline fired and every turn it
    /// held already has its receipt.
    release: tokio::sync::oneshot::Sender<tokio::sync::oneshot::Sender<Vec<ParkedTurn>>>,
}

/// What [`LlmCell::park_turn`] did with a turn that found no credential.
enum ParkOutcome {
    /// Parked, and this turn opened the round — the caller asks the vault.
    Asked,
    /// Parked into a round that is already asking. No second request: the vault
    /// is asked once per round, not once per message.
    Parked,
    /// Not parked, because the bound is full. The turn gets its receipt now
    /// rather than a place in a queue that is not moving. Boxed because a
    /// `ParkedTurn` carries a whole `Message` and this enum is otherwise a tag.
    Refused(Box<ParkedTurn>),
}

/// GH #457: the warden task of one credential round.
///
/// It never reads the parking channel while it waits — the CHANNEL is the
/// buffer, so its capacity is the bound and `try_send` on the cell's side is
/// the overflow check. Draining early would free slots the operator never
/// granted, which is exactly the "grows without limit" the issue rules out.
///
/// It ends in one of two ways. Either the box arrived — then `release` hands
/// the batch back to the cell, in arrival order, and the cell answers them. Or
/// `wait_ms` elapsed — then every turn it is holding gets its
/// `credential_pending` receipt right here. That is the only place in this
/// design where a receipt is produced without a message having arrived, and the
/// reason a vault that never answers cannot turn into silence.
///
/// A dropped cell (sleep, death, panic) closes the release channel; that is the
/// third exit, and it takes the receipt branch for the same reason — nobody is
/// left who could answer these turns.
async fn credential_warden(
    mut turns: tokio::sync::mpsc::Receiver<ParkedTurn>,
    release: tokio::sync::oneshot::Receiver<tokio::sync::oneshot::Sender<Vec<ParkedTurn>>>,
    wait_ms: u64,
) {
    let reply = tokio::select! {
        // Biased, release first: a box that arrived in the same instant the
        // deadline elapsed wins. Answering a turn beats refusing it, and a coin
        // flip between the two would be a flaky receipt.
        biased;
        asked = release => asked.ok(),
        () = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => {
            tracing::warn!(wait_ms, "llm: the sealed credential did not arrive in time");
            None
        }
    };
    // One drain, whichever way this ended. `try_recv` empties a channel whose
    // sender may already be gone, and it yields the turns in the order they
    // were parked.
    let mut held = Vec::new();
    while let Ok(turn) = turns.try_recv() {
        held.push(turn);
    }
    match reply {
        Some(reply) => {
            let _ = reply.send(held);
        }
        None => {
            for turn in held {
                emit_credential_pending(&turn).await;
            }
        }
    }
}

/// GH #457: hand one parked turn its `credential_pending` receipt.
///
/// A free function on purpose: the deadline task holds no `&LlmCell` and must
/// not — the cell's state belongs to the cell task alone, and everything this
/// emission needs travels with the turn.
async fn emit_credential_pending(turn: &ParkedTurn) {
    output::emit_error(
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
            extra: None,
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
    format!(
        "{}{}",
        params.base_url.as_deref().unwrap_or(default_base),
        wire::OPENAI_RESPONSES_PATH
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
    /// R3 / GH #421: the bearer credential the vault delivered. Sealed on the
    /// wire, opened here, and held nowhere else — it is deliberately NOT part
    /// of the params overlay, so it never reaches `cell.db` and never survives
    /// a sleep. A woken cell asks again.
    credential: Option<String>,
    /// The ephemeral X25519 recipient key of the credential request in flight.
    /// `Some` between emitting the request and opening the box.
    pending_recipient: Option<crate::sealed::RecipientKeypair>,
    /// GH #457: the credential round currently in flight, with the turns it is
    /// holding. `None` between rounds — which is the state a cell that holds a
    /// key of its own never leaves.
    credential_wait: Option<CredentialWait>,
    /// GH #457: turns the arriving box released, waiting to be run.
    ///
    /// They are NOT run from inside the delivery's own `handle_one` — that
    /// would be `handle_one` calling itself, and an async fn that awaits itself
    /// has no finite size. `handle` drains this queue after the delivery is
    /// done, which is the same order with a flat call stack.
    released: std::collections::VecDeque<ParkedTurn>,
}

impl LlmCell {
    /// Implementation detail — production entry point is `LlmCellFactory`;
    /// direct construction is `pub` only so tests/integration tests can
    /// drive the cell without the full Colony.
    #[doc(hidden)]
    pub fn new(params: LlmParams, http: reqwest::Client) -> Self {
        Self {
            params,
            http,
            attachments: None,
            credential: None,
            pending_recipient: None,
            credential_wait: None,
            released: std::collections::VecDeque::new(),
        }
    }

    /// GH #457: hold this turn back until the sealed box arrives.
    ///
    /// Three outcomes, and the caller acts on all three: the turn opened a new
    /// round (ask the vault, exactly once per round), it joined a round already
    /// asking (say nothing more), or it did not fit (receipt now).
    ///
    /// A round whose warden is gone — the deadline fired — is no round at all:
    /// the turn opens a fresh one and the vault is asked again. That is the
    /// retry lane the old code did not have, and it costs one request per
    /// round rather than one per message.
    fn park_turn(&mut self, turn: ParkedTurn) -> ParkOutcome {
        let turn = match &self.credential_wait {
            Some(wait) => match wait.turns.try_send(turn) {
                Ok(()) => return ParkOutcome::Parked,
                Err(tokio::sync::mpsc::error::TrySendError::Full(t)) => {
                    return ParkOutcome::Refused(Box::new(t));
                }
                // The warden timed out and dropped its receiver. Fall through:
                // this turn opens the next round.
                Err(tokio::sync::mpsc::error::TrySendError::Closed(t)) => t,
            },
            None => turn,
        };
        // A zero bound would be a channel that cannot be built; one slot is the
        // smallest buffer that still parks the turn that triggers the round,
        // which is the whole point of the issue.
        let (turn_tx, turn_rx) = tokio::sync::mpsc::channel(self.params.credential_wait_max.max(1));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        turn_tx
            .try_send(turn)
            .unwrap_or_else(|_| unreachable!("a fresh channel with >=1 slot has room"));
        tokio::spawn(credential_warden(
            turn_rx,
            release_rx,
            self.params.credential_wait_ms,
        ));
        self.credential_wait = Some(CredentialWait {
            turns: turn_tx,
            release: release_tx,
        });
        ParkOutcome::Asked
    }

    /// GH #457: take the round's turns back from its warden.
    ///
    /// `None` (rather than an empty batch) when there was no round, or when the
    /// warden had already given up — in the latter case every turn it held has
    /// its receipt, and there is nothing left to do about them here.
    async fn reclaim_parked(&mut self) -> Option<Vec<ParkedTurn>> {
        let wait = self.credential_wait.take()?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        // `turns` stays alive across the handshake on purpose: dropping it first
        // would close the warden's inbox in the same breath as the request.
        let CredentialWait { turns, release } = wait;
        release.send(reply_tx).ok()?;
        let batch = reply_rx.await.ok();
        drop(turns);
        batch
    }

    /// GH #457: the box arrived — queue what the round was holding.
    ///
    /// The turns are not run here: this is called from inside `handle_one`, and
    /// running them here would be `handle_one` awaiting itself. `handle` drains
    /// the queue afterwards, same order, flat call stack.
    async fn release_parked(&mut self) {
        if let Some(batch) = self.reclaim_parked().await {
            self.released.extend(batch);
        }
    }

    /// GH #457: the round failed — every turn it was holding gets its receipt.
    ///
    /// The delivery-failure counterpart of [`Self::release_parked`]: a box that
    /// does not open is, for a parked turn, the same outcome as a vault that
    /// refused. Waiting for the deadline would only make the same answer
    /// slower.
    async fn refuse_parked(&mut self) {
        let Some(batch) = self.reclaim_parked().await else {
            return;
        };
        for turn in batch {
            emit_credential_pending(&turn).await;
        }
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
        let pair = match crate::sealed::RecipientKeypair::generate() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "llm: no random source for a credential request");
                return;
            }
        };
        let args = meclaw_core::serde_json::json!({
            "grant_id": grant_id,
            "operation": "vault.deliver",
            "payload": {"recipient_key": pair.public_hex()},
        });
        self.pending_recipient = Some(pair);
        // A closed sink means the colony is going down; there is nothing useful
        // to do about it here and nothing secret in this body.
        let _ = sink
            .push(meclaw_core::CellOutput {
                target: target.clone(),
                content: meclaw_core::serde_json::json!({
                "header": {"route": "credential_request", "grant_id": grant_id},
                "messages": [{"origin": "assistant", "type": "tool_call",
                              "id": meclaw_core::Uuid::now_v7().simple().to_string(),
                              "text": args.to_string()}],
                }),
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
        output::emit_error(
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

    /// The credential this cell presents, in precedence order.
    ///
    /// The vault-delivered one wins over the static `params.api_key` so that a
    /// migrating instance can keep its old key in the config while the sealed
    /// path is switched on, and drop it afterwards. An empty string is not a
    /// credential on either track (GH #271).
    fn bearer(&self) -> Option<&str> {
        self.credential
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.params.api_key.as_deref().filter(|s| !s.is_empty()))
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
/// resulting `Vec<Value>` is deterministic. Missing `tools` key, non-object
/// `tools`, or empty `tools` all return `Ok(Vec::new())`.
///
/// `concat_system_prompt` (T4) skips `tools` at top-level, so `system_tree`
/// can be passed unchanged to both helpers.
fn extract_tools(system_tree: &Value) -> Result<Vec<Value>, TranslateError> {
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
        out.push(parsed);
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
            // in RAM and therefore never parks: `release_parked` is reached only
            // from the delivery path, and the delivery is what set it.
            while let Some(turn) = self.released.pop_front() {
                let ParkedTurn { msg, sink, .. } = turn;
                self.handle_one(msg, &sink, db).await;
            }
        }
    }
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
                    output::emit_error(
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
                    output::emit_error(
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
            // Step 1b (W4b): params-update slot (config.md § Access l.20).
            // Handled FIRST and strictly: the `params` block is merged into
            // self.params + persisted to cell.db, THEN any system/messages run
            // with the updated params (this call already uses the new model).
            // A params-only message persists and returns silently (analog Q3).
            // All-or-nothing: an immutable/unknown/malformed update is a loud
            // `invalid_input` reject with NO partial apply.
            let has_params = content_obj.contains_key("params");
            if let Some(params_val) = content_obj.get("params") {
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        output::emit_error(
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
                        )
                        .await;
                        return;
                    }
                };
                match self.params.apply_update(&update_obj) {
                    Ok((new_params, overlay)) => {
                        let now = unix_secs_now();
                        let persist_result = db
                            .call(move |conn| {
                                crate::params_overlay::persist_params_overlay(conn, &overlay, now)
                            })
                            .await;
                        if let Err(e) = persist_result {
                            output::emit_error(
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
                            )
                            .await;
                            return;
                        }
                        // Live apply — this call's inference (if any) uses it.
                        self.params = new_params;
                    }
                    Err(e) => {
                        output::emit_error(
                            sink,
                            reply_target,
                            "invalid_input",
                            &e.detail(),
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
            }

            // Step 1c (R3 / GH #421): the sealed credential slot. Handled here
            // because it is the one body form that must never touch `cell.db`
            // and never produce an emission — the answer to a delivery is
            // silence, exactly like a params-only message.
            if let Some(sealed_val) = content_obj.get("sealed") {
                match crate::sealed::SealedBox::from_json(sealed_val) {
                    Ok(boxed) => match self.pending_recipient.take() {
                        Some(pair) => match pair.open(&boxed) {
                            Ok(plain) => match String::from_utf8(plain) {
                                Ok(value) => {
                                    self.credential = Some(value);
                                    tracing::info!(
                                        "llm: bearer credential received sealed and opened in RAM"
                                    );
                                    // GH #457: the round is over. Everything it
                                    // was holding moves to the drain queue and
                                    // is answered by `handle` after this call.
                                    self.release_parked().await;
                                }
                                Err(_) => {
                                    self.emit_credential_reject(
                                        sink,
                                        reply_target,
                                        "the delivered credential is not valid UTF-8",
                                        started_at_unix_ms,
                                    )
                                    .await;
                                    self.refuse_parked().await;
                                }
                            },
                            Err(e) => {
                                self.emit_credential_reject(
                                    sink,
                                    reply_target,
                                    &format!("the sealed box did not open ({e})"),
                                    started_at_unix_ms,
                                )
                                .await;
                                // GH #457: a box that does not open is a round
                                // that failed. The turns it was holding get
                                // their receipt now — waiting for the deadline
                                // would only make the same answer slower.
                                self.refuse_parked().await;
                            }
                        },
                        None => {
                            self.emit_credential_reject(
                                sink,
                                reply_target,
                                "a sealed box arrived that this cell never asked for",
                                started_at_unix_ms,
                            )
                            .await;
                        }
                    },
                    Err(e) => {
                        self.emit_credential_reject(
                            sink,
                            reply_target,
                            &format!("malformed sealed slot ({e})"),
                            started_at_unix_ms,
                        )
                        .await;
                        // GH #457: same class as a box that does not open —
                        // a delivery came back and was not usable. With no
                        // round in flight (a stranger's malformed slot) this
                        // is a no-op.
                        self.refuse_parked().await;
                    }
                }
                return;
            }

            let system = content_obj.get("system");
            let messages = content_obj.get("messages");
            if system.is_none() && messages.is_none() {
                // params-only message: already persisted above, stay silent.
                if has_params {
                    return;
                }
                output::emit_error(
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

            // Step 3 (pre): R3 / GH #421 + GH #457. A cell that spends a grant
            // for its credential does not call a provider without one — and it
            // does not throw the turn away either. It PARKS the turn, asks the
            // vault once, and answers the whole batch in order when the box
            // arrives. Placed here because it is the first point at which this
            // is known to be a real inference message, and because it covers
            // BOTH wire dialects with one guard rather than one per lane.
            //
            // The credential lives only in RAM (`self.credential`), so this is
            // the state after every wake, not once per lifetime — which is why
            // GH #421's "refuse this one turn, serve the next" was a turn lost
            // on every wake, and a chat user's silence.
            //
            // `credential_pending` survives as the receipt for the three ways
            // this can genuinely fail: the round times out (the warden), the
            // delivered box does not open (`refuse_parked`), or the bound is
            // full (here). Every one of them names a message; none of them is
            // silence.
            if self.bearer().is_none()
                && let Some(grant) = self
                    .params
                    .credential_grant_id
                    .clone()
                    .filter(|g| !g.is_empty())
            {
                let ask_target = reply_target.clone();
                let turn = ParkedTurn {
                    msg,
                    sink: sink.clone(),
                    reply_target,
                    started_at_unix_ms,
                };
                match self.park_turn(turn) {
                    ParkOutcome::Asked => {
                        self.ask_for_credential(sink, &ask_target, &grant).await;
                    }
                    ParkOutcome::Parked => {}
                    ParkOutcome::Refused(turn) => {
                        emit_credential_pending(&turn).await;
                        self.log_phases(&clock, "credential_pending");
                    }
                }
                return;
            }

            // Step 3 (prep): validate the messages-slot shape. If present but
            // NOT a JSON array → parse-error (same code path as other parse
            // failures, no DB write).
            let messages_array: Option<Vec<meclaw_core::serde_json::Value>> = match messages {
                Some(v) => match v.as_array() {
                    Some(arr) => Some(arr.clone()),
                    None => {
                        output::emit_error(
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
                        output::emit_error(
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
                    output::emit_error(
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
                output::emit_error(
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

            // 5b: extract OpenAI tool objects from system.tools.*.
            let tools = match extract_tools(&system_tree) {
                Ok(t) => t,
                Err(e) => {
                    output::emit_error(
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
            let system_string =
                translate::concat_system_prompt(&system_tree, &self.params.system_order);

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
                    output::emit_error(
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
                        // GH #94: fold the resolved images into the typed
                        // input[]. No-op for an empty vector.
                        translate_responses::attach_input_images(&mut request_json, image_parts);
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
                        run_responses_lane(self, &request_json, timeout, &mut wire_timings).await
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
                        output::emit_error(
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
                        )
                        .await;
                        self.log_phases(&clock, code);
                    }
                }
                return;
            }

            // 5d: build OpenAI Chat-Completions request body.
            let request_json = match translate::build_openai_request(
                &self.params,
                &system_string,
                &input_messages,
                &tools,
            ) {
                Ok(mut r) => {
                    // GH #87: fold the resolved images into the last user
                    // message. No-op for an empty vector.
                    translate::attach_image_parts(&mut r, image_parts);
                    r
                }
                Err(e) => {
                    output::emit_error(
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
            let url = format!(
                "{}{}",
                self.params
                    .base_url
                    .as_deref()
                    .unwrap_or(wire::OPENAI_DEFAULT_BASE_URL),
                wire::OPENAI_CHAT_COMPLETIONS_PATH
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
                    output::emit_error(
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
                    output::emit_error(
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
                        None,
                    )
                    .await;
                    self.log_phases(&clock, translate::translate_error_to_code(&e));
                    return;
                }
            };

            // Step 8: emit the assistant turn as an atomic UBF body (end).
            let latency_ms = (unix_ms_now() - started_at_unix_ms).max(0) as u64;
            let usage = output::HopUsage::of(&translated);
            output::emit_assistant_turn(
                sink,
                reply_target,
                translated.assistant_turn,
                &translated.finish_reason,
                usage,
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

    #[test]
    fn the_ram_credential_wins_over_the_static_key_and_an_empty_one_is_none() {
        let mut cell = mk_cell();
        assert_eq!(cell.bearer(), Some("sk-test"));
        cell.credential = Some("sk-from-the-vault".to_string());
        assert_eq!(cell.bearer(), Some("sk-from-the-vault"));
        cell.credential = Some(String::new());
        assert_eq!(
            cell.bearer(),
            Some("sk-test"),
            "an empty credential is no credential"
        );
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
        let public = cell
            .pending_recipient
            .as_ref()
            .expect("a pair is in flight")
            .public_hex();
        let sealed = crate::sealed::seal_to(&public, b"sk-or-v1-DELIVERED").expect("seal");

        let msg = MessageBuilder::new(Path::new("/llm"))
            .body(Body::Inline(json!({"sealed": sealed.to_json()})))
            .build();
        cell.handle(msg, &sink, &mut db).await;

        assert_eq!(cell.bearer(), Some("sk-or-v1-DELIVERED"));
        assert!(
            cell.pending_recipient.is_none(),
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
        let public = cell.pending_recipient.as_ref().expect("pair").public_hex();
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
            cell.pending_recipient.is_some(),
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
        let public = cell.pending_recipient.as_ref().expect("pair").public_hex();
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
