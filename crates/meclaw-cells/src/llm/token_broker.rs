//! P10 — the single-refresher token broker.
//!
//! # Why this exists
//!
//! The OAuth `refresh_token` **rotates**: every refresh invalidates the token
//! that was used. Two cells refreshing the same store concurrently produce
//! `refresh_token_reused` — a *permanent* failure that forces a human re-login
//! (see `auth::is_permanent_refresh_error`). Refreshes therefore have to be
//! serialized *before* the network call, not reconciled after it.
//!
//! # Why it is an actor and not a lock
//!
//! `AGENTS.md` forbids `Mutex`/`RwLock`/atomics in cell state, and the standing
//! no-polling rule rules out a lockfile wait loop. The substrate's own answer to
//! "serialize access to one resource" is **one task that owns it**. Because the
//! refresh POST runs *inside* the broker task, single-flight is guaranteed by
//! construction: while a refresh is in progress every other request simply sits
//! in the mpsc queue.
//!
//! # The dedup rule
//!
//! A caller that got a 401 asks for a refresh and passes the `generation` it
//! saw. If the cached generation has already moved on, someone else refreshed
//! in the meantime and the caller just gets the fresher token — **no second
//! refresh**. That is what turns "N cells hit 401 at once" into exactly one
//! network refresh.
//!
//! # Known limit
//!
//! This serializes within one process. A different process (an interactive
//! `codex` session) writing the same store can still race us. One colony is one
//! process, and `auth_ref` has no default precisely so that sharing a store with
//! a live session is a deliberate act. Registered in `docs/defer-register.md`.

use crate::llm::auth::{self, AuthError, StoredTokens};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::{mpsc, oneshot};

/// What a cell gets back: the bearer to present, plus the generation it was
/// taken from (which the cell echoes back if that bearer turns out to be stale).
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSnapshot {
    /// Value for the `Authorization: Bearer` header.
    pub access_token: String,
    /// Value for the `ChatGPT-Account-ID` header, when the store carries one.
    pub account_id: Option<String>,
    /// Monotonic per-store counter; bumped on every successful refresh.
    pub generation: u64,
}

/// Redacting `Debug` — same discipline as `auth::StoredTokens`.
impl std::fmt::Debug for TokenSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSnapshot")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .field("generation", &self.generation)
            .finish()
    }
}

/// A request to the broker actor.
struct BrokerRequest {
    auth_ref: PathBuf,
    token_endpoint: String,
    client_id: String,
    /// `None` = "just give me a token". `Some(g)` = "the token from generation
    /// `g` was rejected, refresh unless someone already did".
    seen_generation: Option<u64>,
    reply: oneshot::Sender<Result<TokenSnapshot, AuthError>>,
}

/// Per-store broker state. Lives **inside** the actor task.
struct Cached {
    tokens: StoredTokens,
    generation: u64,
}

/// Process-wide handle to the broker actor.
///
/// **This holds a HANDLE, not state.** All broker state (`HashMap<PathBuf,
/// Cached>`) lives inside the actor task and is touched by that task alone —
/// the one-task-per-actor rule of `AGENTS.md` § Concurrency is satisfied, and
/// there is deliberately no lock anywhere in this module.
static BROKER: OnceLock<mpsc::Sender<BrokerRequest>> = OnceLock::new();

/// Runtime the broker actor runs on.
///
/// # Why the broker owns a runtime instead of using the ambient one
///
/// A `OnceLock` is initialized exactly once and never again. If the actor is
/// spawned onto whichever runtime happens to construct the first oauth cell,
/// then the **handle outlives the task**: when that runtime shuts down the task
/// dies, but `BROKER` still looks initialized, so every later `get_token`
/// silently talks to a dead actor and fails with "broker dropped the request".
///
/// This is invisible in production — one colony is one process with one
/// long-lived runtime — and immediately fatal in tests, where every
/// `#[tokio::test]` builds and tears down its own runtime. It was found exactly
/// that way (`separate_stores_are_independent`, P10 step B6).
///
/// Owning a runtime ties the actor's lifetime to the process, which is what its
/// role already implies: it is the serialization point for a credential shared
/// by every cell. One worker thread is enough — the broker is a single actor
/// whose only work is one HTTP POST at a time. No `block_on` is involved: the
/// runtime has its own workers, so `spawn` from outside is all we need
/// (`AGENTS.md` rules 10 + 11).
static BROKER_RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();

/// Start the broker actor if it is not running yet.
///
/// Returns `false` only if the dedicated runtime could not be created, which
/// makes every subsequent token request fail loudly instead of silently
/// degrading.
///
/// Call this **eagerly** when an `oauth_subscription` cell is constructed (see
/// `LlmCellFactory`), not from the first call's hot path: the project's
/// process-discipline rule is to initialize expensive `OnceLock`s at startup,
/// so the first use site never pays for construction and cannot time out on it.
pub fn ensure_broker_started() -> bool {
    broker_sender().is_some()
}

/// Internal accessor: the broker's sender, starting the actor on first use.
///
/// Private because `BrokerRequest` is an implementation detail — callers talk
/// to the broker through `get_token`, never by constructing requests.
fn broker_sender() -> Option<&'static mpsc::Sender<BrokerRequest>> {
    let rt = BROKER_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("meclaw-token-broker")
            .build()
            .map_err(|e| tracing::error!("token broker runtime unavailable: {e}"))
            .ok()
    });
    let rt = rt.as_ref()?;
    Some(BROKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<BrokerRequest>(64);
        rt.spawn(broker_task(rx));
        tx
    }))
}

/// Ask the broker for a usable access token.
///
/// `seen_generation` is `None` on the first attempt of a call and
/// `Some(generation)` after a 401 — see the module docs for the dedup rule.
pub async fn get_token(
    auth_ref: &str,
    token_endpoint: &str,
    client_id: &str,
    seen_generation: Option<u64>,
) -> Result<TokenSnapshot, AuthError> {
    let (reply, rx) = oneshot::channel();
    let req = BrokerRequest {
        auth_ref: PathBuf::from(auth_ref),
        token_endpoint: token_endpoint.to_string(),
        client_id: client_id.to_string(),
        seen_generation,
        reply,
    };
    broker_sender()
        .ok_or_else(|| AuthError::StoreUnavailable("token broker unavailable".to_string()))?
        .send(req)
        .await
        .map_err(|_| AuthError::StoreUnavailable("token broker stopped".to_string()))?;
    rx.await
        .map_err(|_| AuthError::StoreUnavailable("token broker dropped the request".to_string()))?
}

/// The actor loop. Sequential by construction — one request at a time.
async fn broker_task(mut rx: mpsc::Receiver<BrokerRequest>) {
    let mut cache: HashMap<PathBuf, Cached> = HashMap::new();
    let client = reqwest::Client::new();
    while let Some(req) = rx.recv().await {
        let result = serve(&mut cache, &client, &req).await;
        // A dropped receiver just means the cell gave up; nothing to do.
        let _ = req.reply.send(result);
    }
}

/// Serve one request against the broker's own state.
async fn serve(
    cache: &mut HashMap<PathBuf, Cached>,
    client: &reqwest::Client,
    req: &BrokerRequest,
) -> Result<TokenSnapshot, AuthError> {
    // Ensure we have something cached for this store.
    if !cache.contains_key(&req.auth_ref) {
        let tokens = read_store_off_thread(req.auth_ref.clone()).await?;
        cache.insert(
            req.auth_ref.clone(),
            Cached {
                tokens,
                generation: 0,
            },
        );
    }
    let entry = match cache.get(&req.auth_ref) {
        Some(e) => e,
        // Unreachable: just inserted. Handled without unwrap per coding standards.
        None => {
            return Err(AuthError::StoreUnavailable(
                "broker cache entry vanished".to_string(),
            ));
        }
    };

    let Some(seen) = req.seen_generation else {
        return Ok(snapshot(entry));
    };
    if seen < entry.generation {
        // Someone already refreshed past the rejected token — hand out theirs.
        return Ok(snapshot(entry));
    }

    // We are the refresher for this generation.
    let fresh =
        auth::refresh_tokens(client, &req.token_endpoint, &req.client_id, &entry.tokens).await?;
    let path = req.auth_ref.clone();
    let to_persist = fresh.clone();
    write_store_off_thread(path, to_persist).await?;
    let generation = entry.generation + 1;
    let updated = Cached {
        tokens: fresh,
        generation,
    };
    let out = snapshot(&updated);
    cache.insert(req.auth_ref.clone(), updated);
    Ok(out)
}

fn snapshot(c: &Cached) -> TokenSnapshot {
    TokenSnapshot {
        access_token: c.tokens.access_token.clone(),
        account_id: c.tokens.account_id.clone(),
        generation: c.generation,
    }
}

/// `auth::read_store` is blocking filesystem I/O; async code must not block a
/// worker thread (`AGENTS.md` § Coding-Standards).
async fn read_store_off_thread(path: PathBuf) -> Result<StoredTokens, AuthError> {
    match tokio::task::spawn_blocking(move || auth::read_store(&path)).await {
        Ok(r) => r,
        Err(e) => Err(AuthError::StoreUnavailable(format!(
            "store read task failed: {e}"
        ))),
    }
}

/// Blocking counterpart of `read_store_off_thread`.
async fn write_store_off_thread(path: PathBuf, tokens: StoredTokens) -> Result<(), AuthError> {
    match tokio::task::spawn_blocking(move || auth::write_store(&path, &tokens)).await {
        Ok(r) => r,
        Err(e) => Err(AuthError::StoreUnavailable(format!(
            "store write task failed: {e}"
        ))),
    }
}
