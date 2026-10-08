//! The grant mechanics of a cell that spends a credential (GH #1058).
//!
//! Until GH #1058 this lived inside the `llm` cell alone (GH #421 sealed
//! delivery, GH #457 parking). Five cell types need the same round — `llm`,
//! `web_search`, `code`, `proxy` and `voice` — and a copy per type would be five
//! places where "ask once, park, release on the box, refuse on a bad box" can
//! drift apart. This module is that round, once:
//!
//! ```text
//! item without a credential ──park──▶ slot(grant) ──first item──▶ request(grant)
//!                                         │                         (credential_request)
//!                                         │ wait_ms elapsed          │
//!                                         ▼                          ▼
//!                               on_expired(batch)        sealed box ─▶ accept_sealed
//!                                                                     ├─ opens: secret in RAM,
//!                                                                     │  batch handed back
//!                                                                     └─ does not: batch refused
//! ```
//!
//! What stays with the cell: what an item IS (`T`), how a refused or expired
//! item is answered, every log line (so a cell's lines keep its own target and
//! wording), and where the request is sent. The module holds no `Mutex` — it is
//! `&mut self` state of the one cell task that owns it (`AGENTS.md`), and the
//! only concurrent party, the warden, talks to it over channels.
//!
//! Secrets are held in RAM only, never serialised, and never printed:
//! [`Secret`] and [`CredentialSlots`] print `<sealed>` under `Debug` and have no
//! `Display`.

use crate::sealed::{RecipientKeypair, SealError, SealedBox};
use meclaw_core::serde_json::{Value, json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// GH #457: the `credential_pending` receipt's wording. One constant because
/// three code paths hand it out — the deadline, a broken delivery, and the
/// overflow — and a receipt that reads differently depending on which of them
/// fired would be three receipts to a reader who only has the log. Shared by
/// every cell type since GH #1058, so the receipt reads the same everywhere.
pub const CREDENTIAL_PENDING_DETAIL: &str =
    "the bearer credential was requested from the access hive but did not arrive; retry";

/// GH #457: default bound on the items parked while the sealed credential is
/// in flight. Sixteen is a conversation's worth of backlog, not a queue: the
/// round it covers is one in-colony hop pair, so anything beyond this is a
/// vault that is not answering, and that is what the receipt is for.
#[must_use]
pub fn default_credential_wait_max() -> usize {
    16
}

/// GH #457: default A-timeout for the sealed-credential round, in ms. An
/// in-colony round trip, so it sits with `attachment_timeout_ms`'s order rather
/// than with the provider budget.
#[must_use]
pub fn default_credential_wait_ms() -> u64 {
    10_000
}

/// A grant handle read from hand-parsed params (the proxy connectors):
/// absent, `null` or empty is none; any other type is refused by the param's
/// name. Review V2 M3 (GH #1061): `as_str()` let a number fall through to
/// "no grant", and the params are immutable -- a silent default costs a
/// respawn with a manifest. voice refuses the same way (`split_credential`).
///
/// # Errors
/// `"<key>: must be a string"` for a value that is not one.
pub fn grant_param(
    obj: &meclaw_core::serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(g)) if g.is_empty() => Ok(None),
        Some(Value::String(g)) => Ok(Some(g.clone())),
        Some(_) => Err(format!("{key}: must be a string (a credential_grant_id)")),
    }
}

/// A millisecond credential param read from hand-parsed params: absent or
/// `null` is `default`; anything but a non-negative integer is refused by the
/// param's name (review V2 M3, see [`grant_param`]).
///
/// # Errors
/// `"<key>: must be a non-negative integer (ms)"`.
pub fn ms_param(
    obj: &meclaw_core::serde_json::Map<String, Value>,
    key: &str,
    default: impl FnOnce() -> u64,
) -> Result<u64, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(default()),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("{key}: must be a non-negative integer (ms)")),
    }
}

/// The three grant params of a cell, as one block a cell type can
/// `#[serde(flatten)]` into its params (or nest once per secret slot, for a
/// cell that spends several grants). Field names and defaults are the `llm`
/// cell's, so a template that already sets them reads the same everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CredentialParams {
    /// The grant this cell spends to be handed its credential, sealed. `None`
    /// or `""` = no grant (OR-VG-5: the cell runs without one, or on its
    /// transition literal).
    #[serde(default)]
    pub credential_grant_id: Option<String>,
    /// How many items park while a round is in flight before the next one is
    /// refused with `credential_pending`.
    #[serde(default = "default_credential_wait_max")]
    pub credential_wait_max: usize,
    /// How long a round waits for its box before every parked item gets its
    /// receipt, in ms.
    #[serde(default = "default_credential_wait_ms")]
    pub credential_wait_ms: u64,
}

impl Default for CredentialParams {
    fn default() -> Self {
        Self {
            credential_grant_id: None,
            credential_wait_max: default_credential_wait_max(),
            credential_wait_ms: default_credential_wait_ms(),
        }
    }
}

impl CredentialParams {
    /// The grant, if one is set. The empty string is not a grant (GH #271's
    /// rule for credentials, applied to the handle that fetches one).
    #[must_use]
    pub fn grant(&self) -> Option<&str> {
        self.credential_grant_id
            .as_deref()
            .filter(|g| !g.is_empty())
    }
}

/// A credential the vault delivered, opened in RAM.
///
/// No `Display`, and `Debug` prints `<sealed>`: a `{:?}` of a cell state, a
/// panic message or a test assertion is exactly where a secret leaks without
/// anybody having meant it to.
///
/// `Clone` so a long-running cell can hand it to its I/O half (GH #1059,
/// review V1 M2) — the copy keeps the `<sealed>` `Debug`.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// The value, for the one place that needs it (an `Authorization` header,
    /// a token parameter). Deliberately a method with a loud name rather than
    /// `Deref`, so every use of the value is a visible line.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl Secret {
    /// A stub value for unit tests elsewhere in the crate (never a real key).
    pub(crate) fn stub(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<sealed>")
    }
}

/// The bearer a cell presents, in precedence order (OR-VG-4, GH #1058).
///
/// - A grant is set: ONLY the delivered secret. A literal key next to a grant
///   is never used, not even while the box is still missing — that would be the
///   `${VAR}` fallback the 22.09. ruling forbids. The cell parks and asks.
/// - No grant: the delivered secret if there is one (there cannot be, a box
///   nobody asked for is refused), else the literal — the one-release
///   transition for instances that still carry a key in their config.
///
/// An empty string is not a credential on any track (GH #271).
#[must_use]
pub fn bearer<'a>(
    grant: Option<&str>,
    delivered: Option<&'a str>,
    literal: Option<&'a str>,
) -> Option<&'a str> {
    let delivered = delivered.filter(|s| !s.is_empty());
    if grant.is_some_and(|g| !g.is_empty()) {
        return delivered;
    }
    delivered.or_else(|| literal.filter(|s| !s.is_empty()))
}

/// True when a grant AND a non-empty literal are both configured — the case in
/// which the literal is ignored (OR-VG-4). The caller writes the WARN line
/// itself, naming the param and never the value, so the line carries the
/// cell's own target.
#[must_use]
pub fn literal_is_ignored(grant: Option<&str>, literal: Option<&str>) -> bool {
    grant.is_some_and(|g| !g.is_empty()) && literal.is_some_and(|l| !l.is_empty())
}

/// The content of a `credential_request` emission, exactly the form the `llm`
/// cell has emitted since GH #421: `header.route = "credential_request"`, the
/// grant id beside it, and one `tool_call` spending the grant on
/// `vault.deliver` with the recipient half of a fresh X25519 pair. The edge
/// that matches `hop.route == "credential_request"` decides where it goes.
#[must_use]
pub fn request_content(grant_id: &str, recipient_public_hex: &str, call_id: &str) -> Value {
    let args = json!({
        "grant_id": grant_id,
        "operation": "vault.deliver",
        "payload": {"recipient_key": recipient_public_hex},
    });
    json!({
    "header": {"route": "credential_request", "grant_id": grant_id},
    "messages": [{"origin": "assistant", "type": "tool_call",
                  "id": call_id,
                  "text": args.to_string()}],
    })
}

/// What [`CredentialSlots::park`] did with an item that found no credential.
pub enum ParkOutcome<T> {
    /// Parked, and this item opened the round — the caller asks the vault
    /// ([`CredentialSlots::request`]), exactly once per round.
    Asked,
    /// Parked into a round that is already asking. No second request.
    Parked,
    /// Not parked, because the bound is full. The item gets its receipt now.
    /// Boxed because an item usually carries a whole `Message`.
    Refused(Box<T>),
}

/// A slot's state, for a cell that wants to know without touching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// No secret, no request in flight.
    Empty,
    /// A request is out (or its round expired and nobody re-asked yet).
    Asked,
    /// The secret is in RAM.
    Open,
}

/// The box arrived and opened.
pub struct Accepted<T> {
    /// The slot it opened.
    pub grant: String,
    /// The items the round was holding, in arrival order — empty when the
    /// round's warden had already answered them (the box came after the
    /// deadline) or when the request was an `on_start` ask with nothing parked.
    pub released: Vec<T>,
}

/// GH #1092: the broker denied a round ([`CredentialSlots::accept_denial`]).
pub struct Denied<T> {
    /// The slot the denial names.
    pub grant: String,
    /// The broker's `reason_code` (`vault_locked`, `grant_expired`, …), for
    /// the cell's log line.
    pub reason_code: String,
    /// The items the denied round was holding, in arrival order -- the cell
    /// answers each with its receipt now instead of after `wait_ms`.
    pub refused: Vec<T>,
}

/// Why a delivery was not taken. Every variant that can hold items holds the
/// ones the cell now has to answer with their receipt.
pub enum Refusal<T> {
    /// A box no slot asked for. Nothing was opened, nothing is refused.
    Unsolicited,
    /// The `sealed` slot did not parse. The round of the slot it addressed (if
    /// any) failed — its items are refused.
    Malformed {
        grant: Option<String>,
        error: SealError,
        refused: Vec<T>,
    },
    /// The box did not open with the slot's recipient key.
    DidNotOpen {
        grant: String,
        error: SealError,
        refused: Vec<T>,
    },
    /// The box opened to something that is not UTF-8.
    NotUtf8 { grant: String, refused: Vec<T> },
    /// A box of an earlier round of this slot, superseded by a newer request.
    /// Discarded without refusing the round in flight — that round still has
    /// its own box coming, or its own deadline.
    Late { grant: String },
}

impl<T> Refusal<T> {
    /// The `detail` a cell answers the delivery with, in the wording the `llm`
    /// cell has used since GH #421. `None` for [`Refusal::Late`], which is
    /// answered with a log line only: it is not the sender's fault that a
    /// round expired.
    #[must_use]
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Unsolicited => {
                Some("a sealed box arrived that this cell never asked for".to_string())
            }
            Self::Malformed { error, .. } => Some(format!("malformed sealed slot ({error})")),
            Self::DidNotOpen { error, .. } => {
                Some(format!("the sealed box did not open ({error})"))
            }
            Self::NotUtf8 { .. } => Some("the delivered credential is not valid UTF-8".to_string()),
            Self::Late { .. } => None,
        }
    }

    /// The items this refusal hands back to be answered with their receipt.
    #[must_use]
    pub fn into_refused(self) -> Vec<T> {
        match self {
            Self::Malformed { refused, .. }
            | Self::DidNotOpen { refused, .. }
            | Self::NotUtf8 { refused, .. } => refused,
            Self::Unsolicited | Self::Late { .. } => Vec::new(),
        }
    }
}

/// The future an expiry callback returns.
pub type ExpiryFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Why a warden handed its items to the expiry callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryCause {
    /// The round's `wait_ms` elapsed without a box.
    Deadline {
        /// The bound that elapsed, for the cell's timeout line.
        wait_ms: u64,
    },
    /// The slots were dropped (the cell slept, died or panicked) while the
    /// round was in flight: nobody is left who could answer these items, so
    /// they get their receipt — but no deadline passed, so a cell writes no
    /// timeout line for this cause.
    CellGone,
}

/// GH #457: called by the warden when a round ended without its box, with
/// every item it was holding (arrival order). The cell answers each item with
/// its `credential_pending` receipt and writes its own timeout line. It runs on
/// the warden task, so it holds no `&cell` — whatever the receipt needs travels
/// inside the item.
pub type ExpiryFn<T> = Arc<dyn Fn(Vec<T>, ExpiryCause) -> ExpiryFuture + Send + Sync>;

/// Ask the warden for its batch back: send it the channel to answer on.
type ReleaseTx<T> = tokio::sync::oneshot::Sender<tokio::sync::oneshot::Sender<Vec<T>>>;
type ReleaseRx<T> = tokio::sync::oneshot::Receiver<tokio::sync::oneshot::Sender<Vec<T>>>;

/// GH #457: one in-flight round of one slot. A task OWNS the parked items; an
/// item is owned by exactly one side at every instant — the cell until
/// `try_send`, the warden until it hands the batch back.
struct Round<T> {
    /// The parking lot. Its CAPACITY is the bound, so `try_send` reports the
    /// overflow — there is no counter that could disagree with the buffer.
    items: tokio::sync::mpsc::Sender<T>,
    /// `Err` on send means the warden is gone — the deadline fired and every
    /// item it held already has its receipt.
    release: ReleaseTx<T>,
}

/// How many superseded request ids a slot remembers, to tell a late box from a
/// bad one. A round is re-asked at most once per `wait_ms`, so a handful
/// covers any box that is merely slow; an older one is just a bad box.
const STALE_ROUNDS: usize = 8;

struct Slot<T> {
    secret: Option<Secret>,
    round: Option<Round<T>>,
    /// The recipient key of the request in flight and the request's call id.
    /// `Some` between emitting the request and opening the box — it outlives
    /// an expired round on purpose, so a box that is merely late still opens
    /// (the `llm` behaviour since GH #457).
    recipient: Option<(RecipientKeypair, String)>,
    /// Call ids of requests a newer request superseded.
    stale: VecDeque<String>,
    /// GH #1059 (review V1, I1): the request in flight was asked with no live
    /// round — a long-running cell's `on_start` question, or its re-ask after
    /// a lost one. The first item parked into it JOINS that question instead
    /// of opening a round of its own; otherwise the caller would ask a second
    /// time and the first box would come back `Late`, one wasted round per
    /// cold start. A flag of its own rather than `recipient.is_some()`, so the
    /// llm paths (a recipient that outlived its round) stay byte-identical.
    start_ask: bool,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Self {
            secret: None,
            round: None,
            recipient: None,
            stale: VecDeque::new(),
            start_ask: false,
        }
    }
}

/// The credential slots of one cell, one per grant id, generic over the item a
/// round parks (`llm`: a turn; `proxy`: an outbound send; …).
///
/// Every method that can spawn the warden ([`Self::park`]) must run inside a
/// tokio runtime — a cell's `handle` always does.
pub struct CredentialSlots<T> {
    wait_ms: u64,
    wait_max: usize,
    on_expired: ExpiryFn<T>,
    slots: BTreeMap<String, Slot<T>>,
}

impl<T> std::fmt::Debug for CredentialSlots<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut m = f.debug_map();
        for (grant, slot) in &self.slots {
            let state = match (slot.secret.is_some(), slot.recipient.is_some()) {
                (true, _) => "<sealed>",
                (false, true) => "asked",
                (false, false) => "empty",
            };
            m.entry(grant, &state);
        }
        m.finish()
    }
}

impl<T: Send + 'static> CredentialSlots<T> {
    /// Slots with one round bound (`wait_max` items, `wait_ms` deadline) and
    /// the callback that answers an expired round.
    #[must_use]
    pub fn new(wait_ms: u64, wait_max: usize, on_expired: ExpiryFn<T>) -> Self {
        Self {
            wait_ms,
            wait_max,
            on_expired,
            slots: BTreeMap::new(),
        }
    }

    /// [`Self::new`] with the bounds read from a cell's params block.
    #[must_use]
    pub fn from_params(params: &CredentialParams, on_expired: ExpiryFn<T>) -> Self {
        Self::new(
            params.credential_wait_ms,
            params.credential_wait_max,
            on_expired,
        )
    }

    /// Retune the round bounds. Applies from the next round on — a round in
    /// flight keeps the deadline and capacity it was opened with. The `llm`
    /// cell's two knobs are run-time mutable (GH #853), so it calls this before
    /// every park.
    pub fn set_bounds(&mut self, wait_ms: u64, wait_max: usize) {
        self.wait_ms = wait_ms;
        self.wait_max = wait_max;
    }

    /// The public half of the recipient key of `grant`'s request in flight —
    /// what the broker reads out of the request. Public by construction; for
    /// tests and diagnostics.
    #[must_use]
    pub fn recipient_hex(&self, grant: &str) -> Option<String> {
        self.slots
            .get(grant)
            .and_then(|s| s.recipient.as_ref())
            .map(|(pair, _)| pair.public_hex())
    }

    /// The delivered secret of `grant`, if its box arrived and opened. Empty is
    /// not a secret.
    #[must_use]
    pub fn secret(&self, grant: &str) -> Option<&str> {
        self.slots
            .get(grant)
            .and_then(|s| s.secret.as_ref())
            .map(Secret::expose)
            .filter(|s| !s.is_empty())
    }

    /// The delivered secret of `grant` as a [`Secret`], for a cell that must
    /// carry it out of its own task (a long-running cell's I/O half, GH #1059).
    /// Empty is not a secret.
    #[must_use]
    pub fn secret_handle(&self, grant: &str) -> Option<Secret> {
        self.slots
            .get(grant)
            .and_then(|s| s.secret.clone())
            .filter(|s| !s.expose().is_empty())
    }

    /// Where the slot of `grant` stands.
    #[must_use]
    pub fn state(&self, grant: &str) -> SlotState {
        match self.slots.get(grant) {
            Some(s) if s.secret.is_some() => SlotState::Open,
            Some(s) if s.recipient.is_some() || s.round.is_some() => SlotState::Asked,
            _ => SlotState::Empty,
        }
    }

    /// Forget the secret of `grant` (a provider said it is no good any more).
    /// The next item parks and asks again.
    pub fn forget(&mut self, grant: &str) {
        if let Some(s) = self.slots.get_mut(grant) {
            s.secret = None;
        }
    }

    /// GH #457: hold `item` back until the box of `grant` arrives.
    ///
    /// A round whose warden is gone — the deadline fired — is no round at all:
    /// the item opens a fresh one and the caller asks again. That costs one
    /// request per round rather than one per item.
    pub fn park(&mut self, grant: &str, item: T) -> ParkOutcome<T> {
        let (wait_ms, wait_max) = (self.wait_ms, self.wait_max);
        let on_expired = Arc::clone(&self.on_expired);
        let slot = self
            .slots
            .entry(grant.to_string())
            .or_insert_with(Slot::new);
        let item = match &slot.round {
            Some(round) => match round.items.try_send(item) {
                Ok(()) => return ParkOutcome::Parked,
                Err(tokio::sync::mpsc::error::TrySendError::Full(t)) => {
                    return ParkOutcome::Refused(Box::new(t));
                }
                // The warden timed out and dropped its receiver: this item
                // opens the next round.
                Err(tokio::sync::mpsc::error::TrySendError::Closed(t)) => t,
            },
            None => item,
        };
        // A zero bound would be a channel that cannot be built; one slot is the
        // smallest buffer that still parks the item that triggers the round.
        let (items_tx, items_rx) = tokio::sync::mpsc::channel(wait_max.max(1));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        items_tx
            .try_send(item)
            .unwrap_or_else(|_| unreachable!("a fresh channel with >=1 slot has room"));
        tokio::spawn(warden(items_rx, release_rx, wait_ms, on_expired));
        slot.round = Some(Round {
            items: items_tx,
            release: release_tx,
        });
        // GH #1059: a question already in flight with no round of its own is
        // this round's question — no second request.
        if std::mem::take(&mut slot.start_ask) && slot.recipient.is_some() {
            return ParkOutcome::Parked;
        }
        ParkOutcome::Asked
    }

    /// Mint the recipient key for a request of `grant` and return the
    /// `credential_request` content to emit ([`request_content`]).
    ///
    /// Called after [`ParkOutcome::Asked`], and on its own from `on_start` by a
    /// long-running cell that asks before anything is parked. A request that
    /// supersedes an unanswered one remembers the old call id, so the old
    /// round's box — should it still come — is told apart from a bad box
    /// ([`Refusal::Late`]).
    ///
    /// # Errors
    /// No random source for the key pair; the caller logs it, and a parked
    /// round then ends at its deadline with receipts.
    pub fn request(&mut self, grant: &str) -> Result<Value, SealError> {
        let pair = RecipientKeypair::generate()?;
        let call_id = meclaw_core::Uuid::now_v7().simple().to_string();
        let content = request_content(grant, &pair.public_hex(), &call_id);
        let slot = self
            .slots
            .entry(grant.to_string())
            .or_insert_with(Slot::new);
        // GH #1059: asked with no live round (on_start, or a long-running
        // cell's re-ask) — the next park joins this question.
        slot.start_ask = slot.round.as_ref().is_none_or(|r| r.items.is_closed());
        if let Some((_, old)) = slot.recipient.replace((pair, call_id)) {
            slot.stale.push_back(old);
            while slot.stale.len() > STALE_ROUNDS {
                slot.stale.pop_front();
            }
        }
        Ok(content)
    }

    /// The `on_start` helper of a long-running cell: one request per grant,
    /// nothing parked. A cell's key pair and secret live in RAM only, so every
    /// (re)start asks again. Returns `(grant, content)` per grant whose key
    /// pair could be minted; the errors come back beside them for the log.
    pub fn request_all<'g>(
        &mut self,
        grants: impl IntoIterator<Item = &'g str>,
    ) -> Vec<(String, Result<Value, SealError>)> {
        grants
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| (g.to_string(), self.request(g)))
            .collect()
    }

    /// Take a delivery: the content of a message that carries a `sealed` slot.
    ///
    /// The box is matched to a slot by the `grant_id` the broker's ack names
    /// (`messages[].text` of the `tool_result`), else by the request's call id
    /// (the `tool_result` id), else — a bare `{"sealed": …}` — to the one slot
    /// with a request in flight. A box whose call id names a SUPERSEDED request
    /// is late and discarded; anything else is opened with the slot's recipient
    /// key, which is consumed either way.
    ///
    /// # Errors
    /// [`Refusal`] — the cell answers the delivery with
    /// [`Refusal::detail`] and every [`Refusal::into_refused`] item with its
    /// receipt.
    pub async fn accept_sealed(&mut self, content: &Value) -> Result<Accepted<T>, Refusal<T>> {
        let (grant_hint, call_id) = ack_of(content);
        let grant = self.slot_for(grant_hint.as_deref(), call_id.as_deref());
        let boxed = match SealedBox::from_json(content.get("sealed").unwrap_or(&Value::Null)) {
            Ok(b) => b,
            Err(error) => {
                // Same class as a box that does not open: a delivery came back
                // and was not usable, so the round it answered failed. With no
                // round in flight (a stranger's malformed slot) nothing is
                // refused. The recipient stays — the real box may still come.
                let refused = match &grant {
                    Some(g) => self.reclaim(g).await.unwrap_or_default(),
                    None => Vec::new(),
                };
                return Err(Refusal::Malformed {
                    grant,
                    error,
                    refused,
                });
            }
        };
        let Some(grant) = grant else {
            return Err(Refusal::Unsolicited);
        };
        let Some(slot) = self.slots.get_mut(&grant) else {
            return Err(Refusal::Unsolicited);
        };
        if call_id
            .as_deref()
            .is_some_and(|id| slot.stale.iter().any(|s| s == id))
        {
            return Err(Refusal::Late { grant });
        }
        let Some((pair, _)) = slot.recipient.take() else {
            return Err(Refusal::Unsolicited);
        };
        match pair.open(&boxed) {
            Ok(plain) => match String::from_utf8(plain) {
                Ok(value) => {
                    slot.secret = Some(Secret(value));
                    let released = self.reclaim(&grant).await.unwrap_or_default();
                    Ok(Accepted { grant, released })
                }
                Err(_) => {
                    let refused = self.reclaim(&grant).await.unwrap_or_default();
                    Err(Refusal::NotUtf8 { grant, refused })
                }
            },
            Err(error) => {
                let refused = self.reclaim(&grant).await.unwrap_or_default();
                Err(Refusal::DidNotOpen {
                    grant,
                    error,
                    refused,
                })
            }
        }
    }

    /// GH #1092: take the broker's DENIAL of a request this cell made.
    ///
    /// The access hive answers a refused spend in the same `ack` a delivery
    /// travels in -- one `tool_result` whose `id` is the request's call id and
    /// whose text is `{"outcome": "denied", "reason_code", "grant_id"}` -- only
    /// with no `sealed` slot. Measured in 0.62.0 (orga lab, 8 of 150 runs red):
    /// every colony without a vault deposit answers the shell translator's
    /// round `vault_locked`; with no edge to carry the denial home it
    /// dead-lettered (`hive_no_route` at `/os/access`) while the parked turn
    /// waited out its `wait_ms`.
    ///
    /// `None`: the content is no denial of a slot of THIS cell -- no `sealed`
    /// slot, exactly one `tool_result`, outcome `denied`, a `grant_id` this
    /// cell holds -- and the caller handles it like any other message. `Some`:
    /// the denial was taken. When it answers the request in flight, that
    /// request's key is dropped and the round's items come back in `refused`
    /// (empty when the warden had already answered them); the next item parks
    /// and asks again. A denial of a superseded or unknown request refuses
    /// nothing: the round in flight still has its own answer coming.
    pub async fn accept_denial(&mut self, content: &Value) -> Option<Denied<T>> {
        let (grant, reason_code, call_id) = denial_of(content)?;
        let slot = self.slots.get_mut(&grant)?;
        let current = slot
            .recipient
            .as_ref()
            .is_some_and(|(_, id)| call_id.as_deref().is_none_or(|c| c == id));
        if !current {
            return Some(Denied {
                grant,
                reason_code,
                refused: Vec::new(),
            });
        }
        // The denied request's id joins the superseded ones, so a box that
        // still came for it would read as late, not as a bad box.
        if let Some((_, old)) = slot.recipient.take() {
            slot.stale.push_back(old);
            while slot.stale.len() > STALE_ROUNDS {
                slot.stale.pop_front();
            }
        }
        slot.start_ask = false;
        let refused = self.reclaim(&grant).await.unwrap_or_default();
        Some(Denied {
            grant,
            reason_code,
            refused,
        })
    }

    /// Which slot a delivery addresses (see [`Self::accept_sealed`]).
    fn slot_for(&self, grant_hint: Option<&str>, call_id: Option<&str>) -> Option<String> {
        if let Some(g) = grant_hint
            && self.slots.contains_key(g)
        {
            return Some(g.to_string());
        }
        if let Some(id) = call_id
            && let Some((g, _)) = self.slots.iter().find(|(_, s)| {
                s.recipient.as_ref().is_some_and(|(_, c)| c == id)
                    || s.stale.iter().any(|c| c == id)
            })
        {
            return Some(g.clone());
        }
        let mut asking = self.slots.iter().filter(|(_, s)| s.recipient.is_some());
        match (asking.next(), asking.next()) {
            (Some((g, _)), None) => Some(g.clone()),
            _ if self.slots.len() == 1 => self.slots.keys().next().cloned(),
            _ => None,
        }
    }

    /// GH #457: take the round's items back from its warden. `None` when there
    /// was no round, or when the warden had already given up — then every item
    /// it held has its receipt.
    async fn reclaim(&mut self, grant: &str) -> Option<Vec<T>> {
        let round = self.slots.get_mut(grant)?.round.take()?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        // `items` stays alive across the handshake on purpose: dropping it
        // first would close the warden's inbox in the same breath as the ask.
        let Round { items, release } = round;
        release.send(reply_tx).ok()?;
        let batch = reply_rx.await.ok();
        drop(items);
        batch
    }
}

/// The grant id and call id of the broker's ack that carries a box, when the
/// delivery has one: `messages[]` holds a `tool_result` whose `id` is the
/// request's call id and whose text is `{"outcome", "grant_id", "operation"}`.
fn ack_of(content: &Value) -> (Option<String>, Option<String>) {
    let Some(ack) = content
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|m| m.iter().find(|m| m["type"] == "tool_result"))
    else {
        return (None, None);
    };
    let call_id = ack["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let grant = ack["text"]
        .as_str()
        .and_then(|t| meclaw_core::serde_json::from_str::<Value>(t).ok())
        .and_then(|v| v["grant_id"].as_str().map(str::to_string))
        .filter(|s| !s.is_empty());
    (grant, call_id)
}

/// GH #1092: the grant, reason code and call id of a broker DENIAL -- content
/// with no `sealed` slot whose one message is a `tool_result` reading
/// `{"outcome": "denied", "grant_id": …}` (the `refuse` form of the access
/// hive's `invoke` cell).
fn denial_of(content: &Value) -> Option<(String, String, Option<String>)> {
    if content.get("sealed").is_some() {
        return None;
    }
    let [ack] = content.get("messages")?.as_array()?.as_slice() else {
        return None;
    };
    if ack["type"] != "tool_result" {
        return None;
    }
    let text: Value = meclaw_core::serde_json::from_str(ack["text"].as_str()?).ok()?;
    if text["outcome"] != "denied" {
        return None;
    }
    let grant = text["grant_id"].as_str().filter(|s| !s.is_empty())?;
    let reason = text["reason_code"].as_str().unwrap_or_default();
    let call_id = ack["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((grant.to_string(), reason.to_string(), call_id))
}

/// GH #457: the warden task of one round.
///
/// A task rather than a check on the next message, because there may not BE a
/// next message: a broker refusal reaches the asking cell only where the
/// topology routes the denial home ([`CredentialSlots::accept_denial`],
/// GH #1092), so a cell that only looked at its own inbox would hold the
/// parked items forever — the silence GH #457 is about. And a task
/// that OWNS the items rather than a lock over them, because `AGENTS.md`
/// forbids `Mutex`/`RwLock`/atomics in cell state; this is the same answer
/// `llm::token_broker` gives to "two timelines, one piece of state".
///
/// It never reads the parking channel while it waits — the CHANNEL is the
/// buffer, so its capacity is the bound. It ends in one of three ways: the box
/// arrived (`release` hands the batch back, arrival order); `wait_ms` elapsed
/// (every item goes to `on_expired` with [`ExpiryCause::Deadline`]); or the
/// cell was dropped (the release channel closed — [`ExpiryCause::CellGone`],
/// the receipt branch, because nobody is left to answer).
/// That last exit is also why the warden cannot outlive the cell's items.
async fn warden<T: Send + 'static>(
    mut items: tokio::sync::mpsc::Receiver<T>,
    release: ReleaseRx<T>,
    wait_ms: u64,
    on_expired: ExpiryFn<T>,
) {
    let reply = tokio::select! {
        // Biased, release first: a box that arrived in the same instant the
        // deadline elapsed wins. Answering beats refusing, and a coin flip
        // between the two would be a flaky receipt.
        biased;
        asked = release => asked.map_err(|_| ExpiryCause::CellGone),
        () = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => {
            Err(ExpiryCause::Deadline { wait_ms })
        }
    };
    let mut held = Vec::new();
    while let Ok(item) = items.try_recv() {
        held.push(item);
    }
    match reply {
        Ok(reply) => {
            let _ = reply.send(held);
        }
        Err(cause) => on_expired(held, cause).await,
    }
}
