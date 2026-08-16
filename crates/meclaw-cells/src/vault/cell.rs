//! The vault cell — the route surface with no read on it.
//!
//! Seven operations: `status`, `unlock`, `lock`, `put`, `rotate`, `revoke`,
//! `use`. There is no eighth one that returns a secret, and that is the entire
//! security claim of this cell type. Everything else here — the sender ACL, the
//! unlock attestation, the audit trail — protects that claim; none of it
//! *creates* it.
//!
//! Two callers exist, and they can do different things:
//!
//! - **The user channel** — a message with no `reply_to`, which is what a
//!   source message from the CLI or the API looks like. No edge can produce
//!   one, because the colony stamps `reply_to` on everything a cell emits. This
//!   is the only way a secret gets *in*.
//! - **The broker** — the one path named in `params.broker`. It may ask the
//!   vault to *use* a secret. The grant lookup is the broker's job, not the
//!   vault's: a cell cannot query another cell within one `handle()`, so the
//!   vault does what it alone can do — check who is talking — and requires the
//!   broker to carry the grant it validated.
//!
//! Everything else is refused and audited.

use super::attest::{self, Attestation};
use super::crypto::{self, MasterKey};
use super::keysource::{self, KeyMaterial};
use super::params::VaultParams;
use super::store;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellOutput, Message, OutputSink, Path};

/// The operations a vault answers. Written out as a list so that the absence
/// of `get` is a visible property of the source, not an accident of dispatch.
const OPS: &[&str] = &["status", "unlock", "lock", "put", "rotate", "revoke", "use"];

/// The `vault` cell: an encrypted store plus an executor, and no way out for
/// what it holds.
pub struct VaultCell {
    /// Effective params.
    pub params: VaultParams,
    /// The master key while unlocked. `None` is the resting state — a vault
    /// that has just woken is locked, whatever it was before it slept.
    key: Option<MasterKey>,
    /// When the current unlock expires (`params.unlock_ttl_ms`).
    unlocked_until: Option<std::time::Instant>,
    /// This cell's own directory, for finding `colony.db` at attestation time.
    cell_dir: std::path::PathBuf,
}

impl VaultCell {
    /// Construct a locked vault. Production entry is [`super::VaultCellFactory`].
    #[doc(hidden)]
    pub fn new(params: VaultParams, cell_dir: std::path::PathBuf) -> Self {
        Self {
            params,
            key: None,
            unlocked_until: None,
            cell_dir,
        }
    }

    /// Whether the vault currently holds its key, expiring a stale unlock on
    /// the way. Checked at the top of every operation, so a TTL cannot be
    /// outlived by a long-idle vault that nobody asked anything.
    fn unlocked(&mut self) -> bool {
        if let Some(until) = self.unlocked_until
            && std::time::Instant::now() >= until
        {
            self.key = None;
            self.unlocked_until = None;
        }
        self.key.is_some()
    }

    /// Drop the key.
    fn relock(&mut self) {
        self.key = None;
        self.unlocked_until = None;
    }
}

/// Who is talking to the vault.
#[derive(Debug, PartialEq, Eq)]
enum Caller {
    /// A source message — the CLI or the API, never an edge.
    UserChannel,
    /// The one broker named in params.
    Broker,
    /// Anyone else. Carries the path for the audit row.
    Foreign(String),
}

fn classify(sender: Option<&Path>, params: &VaultParams, vault_path: &str) -> Caller {
    let broker = params.broker_path(vault_path);
    match sender {
        None => Caller::UserChannel,
        Some(p) if p.as_str() == broker => Caller::Broker,
        Some(p) => Caller::Foreign(p.as_str().to_string()),
    }
}

impl Caller {
    /// The actor string written into the audit trail.
    fn actor(&self) -> &str {
        match self {
            Self::UserChannel => "user-channel",
            Self::Broker => "broker",
            Self::Foreign(p) => p,
        }
    }

    /// Which operations this caller may invoke.
    ///
    /// Secrets enter over the user channel only — that is the filling workflow,
    /// and it is why an agent that has fully taken over the colony's message
    /// surface still cannot plant a credential. `use` is the broker's, because
    /// the broker is where the grant was checked.
    fn may(&self, op: &str) -> bool {
        match self {
            Self::UserChannel => matches!(
                op,
                "status" | "unlock" | "lock" | "put" | "rotate" | "revoke"
            ),
            Self::Broker => matches!(op, "status" | "use" | "revoke"),
            Self::Foreign(_) => false,
        }
    }
}

/// Parse the inbound `tool_call` turn into `(args, call_id)`.
fn parse_tool_call(msg: &Message) -> Result<(Value, Option<String>), String> {
    let Body::Inline(v) = &msg.body else {
        return Err("vault: body must be inline".into());
    };
    let messages = v
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("vault: missing messages array")?;
    let first = messages.first().ok_or("vault: messages empty")?;
    let ty = first
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("vault: turn missing type")?;
    if ty != "tool_call" {
        return Err(format!("vault: expected tool_call turn, got {ty}"));
    }
    let text = first
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("vault: missing text")?;
    let args: Value = meclaw_core::serde_json::from_str(text)
        .map_err(|e| format!("vault: tool_call.text is not valid JSON: {e}"))?;
    let id = first.get("id").and_then(|v| v.as_str()).map(str::to_string);
    Ok((args, id))
}

/// Emit one `tool_result` turn.
async fn emit(
    sink: &OutputSink,
    target: &Path,
    call_id: Option<String>,
    payload: Value,
    error_code: Option<&str>,
    started: std::time::Instant,
) {
    let mut header = json!({ "duration_ms": started.elapsed().as_millis() as i64 });
    if let Some(code) = error_code {
        header["error_code"] = json!(code);
    }
    let text = match payload.as_str() {
        Some(s) => s.to_string(),
        None => payload.to_string(),
    };
    let body = json!({
        "header": header,
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": text,
            "id": call_id.unwrap_or_default(),
        }]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit a hard error turn (`finish_reason: "error"`), for malformed input only.
async fn emit_invalid_input(
    sink: &OutputSink,
    target: &Path,
    detail: String,
    started: std::time::Instant,
) {
    let body = json!({
        "header": {
            "finish_reason": "error",
            "error_code": "invalid_input",
            "duration_ms": started.elapsed().as_millis() as i64,
        },
        "messages": [{"origin":"tool","type":"tool_result","text": detail,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

fn now_iso() -> String {
    store::now_iso()
}

/// A required string argument.
fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("vault: missing or empty argument {key:?}"))
}

#[allow(clippy::manual_async_fn)]
impl StatefulCell for VaultCell {
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
            let vault_path = sink.sender_path().clone();
            // The sender the COLONY stamped. An identity a caller can write
            // into the body is not an identity.
            let caller = classify(msg.reply_to.as_ref(), &self.params, vault_path.as_str());

            let (args, call_id) = match parse_tool_call(&msg) {
                Ok(v) => v,
                Err(e) => {
                    emit_invalid_input(sink, &reply_target, e, started).await;
                    return;
                }
            };

            let op = match args.get("op").and_then(|v| v.as_str()) {
                Some(o) => o.to_string(),
                None => {
                    emit_invalid_input(
                        sink,
                        &reply_target,
                        "vault: missing \"op\"".to_string(),
                        started,
                    )
                    .await;
                    return;
                }
            };

            // An unknown op is refused by name, and `get` is unknown here in
            // exactly the same way `frobnicate` is. That is the point: the
            // absence is structural, not a special case that could be argued
            // away later.
            if !OPS.contains(&op.as_str()) {
                let actor = caller.actor().to_string();
                let (o, a) = (op.clone(), actor.clone());
                let at = now_iso();
                let _ = db
                    .call(move |c| {
                        store::audit(c, &at, &o, &a, None, "refused", Some("unknown_op"))
                    })
                    .await;
                emit(
                    sink,
                    &reply_target,
                    call_id,
                    json!(format!(
                        "vault: unknown op {op:?} — this cell type has no operation that returns \
                         a secret; available: {}",
                        OPS.join(", ")
                    )),
                    Some("unknown_op"),
                    started,
                )
                .await;
                return;
            }

            if !caller.may(&op) {
                let actor = caller.actor().to_string();
                let reason = match caller {
                    Caller::Foreign(_) => "sender_not_broker",
                    _ => "op_not_permitted_for_caller",
                };
                let (o, a, at) = (op.clone(), actor.clone(), now_iso());
                let _ = db
                    .call(move |c| store::audit(c, &at, &o, &a, None, "refused", Some(reason)))
                    .await;
                emit(
                    sink,
                    &reply_target,
                    call_id,
                    json!(format!(
                        "vault: '{op}' refused for this caller ({actor}). Secrets enter over the \
                         user channel; the broker is the only sender that may use them."
                    )),
                    Some("access_denied"),
                    started,
                )
                .await;
                return;
            }

            let outcome = match op.as_str() {
                "status" => self.op_status(db).await,
                "lock" => {
                    self.relock();
                    Ok(json!({"locked": true}))
                }
                "unlock" => self.op_unlock(db, &args, &vault_path, sink).await,
                "put" | "rotate" => self.op_put(db, &args).await,
                "revoke" => self.op_revoke(db, &args).await,
                "use" => self.op_use(db, &args).await,
                _ => unreachable!("op list checked above"),
            };

            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let actor = caller.actor().to_string();
            let at = now_iso();
            let (o, ok) = (op.clone(), outcome.is_ok());
            let reason = outcome.as_ref().err().cloned();
            let _ = db
                .call(move |c| {
                    store::audit(
                        c,
                        &at,
                        &o,
                        &actor,
                        name.as_deref(),
                        if ok { "ok" } else { "refused" },
                        reason.as_deref(),
                    )
                })
                .await;

            match outcome {
                Ok(v) => emit(sink, &reply_target, call_id, v, None, started).await,
                Err(e) => {
                    let code = error_code_for(&e);
                    emit(sink, &reply_target, call_id, json!(e), Some(code), started).await
                }
            }
        }
    }
}

/// Map a failure message onto a stable error code for the wire.
fn error_code_for(detail: &str) -> &'static str {
    if detail.starts_with("vault: locked") {
        "vault_locked"
    } else if detail.contains("attestation") {
        "attestation_failed"
    } else if detail.contains("no such secret") {
        "unknown_secret"
    } else if detail.starts_with("vault: missing") {
        "invalid_input"
    } else {
        "vault_error"
    }
}

impl VaultCell {
    /// `status` — metadata only: locked or not, the key's fingerprint, and the
    /// names held with their active version. Never a secret, by construction.
    async fn op_status(&mut self, db: &mut DbConn) -> Result<Value, String> {
        let unlocked = self.unlocked();
        let key_id = self.key.as_ref().map(crypto::key_id);
        let held = db.call(|c| store::names(c)).await?;
        Ok(json!({
            "locked": !unlocked,
            "key_id": key_id,
            "key_source": self.params.key_source.as_str(),
            "secrets": held
                .into_iter()
                .map(|(n, v)| json!({"name": n, "version": v}))
                .collect::<Vec<_>>(),
        }))
    }

    /// `unlock` — attest first, then take the key.
    ///
    /// The order is the whole point. A vault that attests *after* unlocking has
    /// already lost: the key was in memory next to a cell that should not have
    /// been able to reach it.
    async fn op_unlock(
        &mut self,
        db: &mut DbConn,
        args: &Value,
        vault_path: &Path,
        sink: &OutputSink,
    ) -> Result<Value, String> {
        let expected = self.params.attested_neighbors(vault_path.as_str());
        let path = vault_path.as_str().to_string();
        let dir = self.cell_dir.clone();
        let verdict =
            tokio::task::spawn_blocking(move || match attest::colony_db_from_cell_dir(&dir) {
                Some(db_path) => attest::attest(&db_path, &path, &expected),
                None => Attestation::Unverifiable("no colony.db above this cell".into()),
            })
            .await
            .map_err(|e| format!("vault: attestation task failed: {e}"))?;

        if verdict != Attestation::Matches {
            return Err(format!(
                "vault: unlock refused, attestation failed — {}. The vault stays locked; a \
                 topology that was not sealed never sees the key.",
                verdict.reason()
            ));
        }

        // Passphrase: from the message when the user channel carries one,
        // otherwise from the configured source.
        let passphrase: Vec<u8> = match args.get("passphrase").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p.as_bytes().to_vec(),
            _ => match keysource::load(&self.params).await? {
                KeyMaterial::Passphrase(p) => p,
                KeyMaterial::AwaitUserChannel => {
                    return Err(
                        "vault: key_source \"prompt\" needs the passphrase on the unlock call \
                         (meclaw --vault-unlock reads it from stdin, never from a param)"
                            .into(),
                    );
                }
            },
        };

        let salt = db.call(|c| store::salt_or_create(c)).await?;
        let key = MasterKey::derive(&passphrase, &salt).map_err(|e| e.to_string())?;

        // A wrong passphrase must not silently produce a vault that fails on
        // every later `use`. If anything is stored, prove the key against it now.
        if let Some(first) = db
            .call(|c| store::names(c).map(|n| n.first().map(|(name, _)| name.clone())))
            .await?
        {
            let probe = db.call(move |c| store::active(c, &first)).await?;
            if let Some(sealed) = probe
                && key.open(&sealed.nonce, &sealed.ciphertext).is_err()
            {
                return Err(
                    "vault: unlock refused — that passphrase does not open this vault".to_string(),
                );
            }
        }

        let key_id = crypto::key_id(&key);
        self.key = Some(key);
        self.unlocked_until = self
            .params
            .unlock_ttl_ms
            .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

        // Injection-at-unlock: the one path on which a plaintext secret leaves
        // this cell, and it happens ONCE, here, to a target named in
        // configuration — never per request and never to an address a message
        // could choose. The requester learns only how many went out.
        let delivered = self.inject_all(db, vault_path, sink).await?;
        Ok(json!({"locked": false, "key_id": key_id, "injected": delivered}))
    }

    /// Hand every configured secret to the cell that needs it.
    ///
    /// The receiving cell gets a params update — the same wire an `llm` cell
    /// (or any cell with a mutable param) already understands: a body with a
    /// `params` slot and no `messages`, which is merged, persisted and answered
    /// with silence. No inference, no cost, no echo.
    ///
    /// A missing secret is skipped with a warning rather than failing the
    /// unlock: a vault that refuses to open because one of five credentials has
    /// not been deposited yet is a vault nobody can commission.
    async fn inject_all(
        &mut self,
        db: &mut DbConn,
        vault_path: &Path,
        sink: &OutputSink,
    ) -> Result<Vec<Value>, String> {
        let plan = self.params.injections(vault_path.as_str());
        let mut delivered = Vec::new();
        for injection in plan {
            let n = injection.name.clone();
            let sealed = match db.call(move |c| store::active(c, &n)).await? {
                Some(s) => s,
                None => {
                    tracing::warn!(
                        secret = injection.name.as_str(),
                        target = injection.to.as_str(),
                        "vault: nothing to inject under this name — deposit it with \
                         `meclaw --vault-add` and unlock again"
                    );
                    continue;
                }
            };
            let key = self.key.as_ref().expect("unlocked before injecting");
            let plaintext = key
                .open(&sealed.nonce, &sealed.ciphertext)
                .map_err(|e| format!("vault: {e}"))?;
            let value = String::from_utf8(plaintext).map_err(|_| {
                "vault: this secret is not valid UTF-8 and cannot travel as a param".to_string()
            })?;
            let _ = sink
                .push(CellOutput {
                    target: Path::new(&injection.to),
                    content: json!({
                        "header": {"msg_type": "params_update", "vault_key": injection.key},
                        "params": {injection.key.clone(): value},
                    }),
                })
                .await;
            // What comes back to the requester is metadata: which name went
            // where, under which key. Never the value.
            delivered.push(json!({
                "name": injection.name,
                "to": injection.to,
                "key": injection.key
            }));
        }
        Ok(delivered)
    }

    /// `put` / `rotate` — the filling workflow. Same code, two names: a put
    /// onto an existing name *is* a rotation, because nothing is overwritten.
    async fn op_put(&mut self, db: &mut DbConn, args: &Value) -> Result<Value, String> {
        if !self.unlocked() {
            return Err("vault: locked — unlock before storing a secret".into());
        }
        let name = arg(args, "name")?.to_string();
        let secret = arg(args, "secret")?.to_string();
        let key = self.key.as_ref().expect("unlocked checked above");
        let (nonce, ct) = key.seal(secret.as_bytes()).map_err(|e| e.to_string())?;
        let at = now_iso();
        let n = name.clone();
        let version = db
            .call(move |c| store::put(c, &n, &nonce, &ct, &at))
            .await?;
        Ok(json!({"name": name, "version": version, "stored": true}))
    }

    /// `revoke` — flip every active version of a name. The rows stay.
    async fn op_revoke(&mut self, db: &mut DbConn, args: &Value) -> Result<Value, String> {
        let name = arg(args, "name")?.to_string();
        let n = name.clone();
        let changed = db.call(move |c| store::revoke(c, &n)).await?;
        Ok(json!({"name": name, "revoked_versions": changed}))
    }

    /// `use` — do something WITH a secret and return only the result.
    ///
    /// v1 knows one operation, `sign`: an HMAC-SHA256 over a caller-supplied
    /// payload. It is deliberately the ssh-agent shape — the secret does work
    /// and stays home. Anything that would need the plaintext to leave this
    /// cell (handing a credential to a connector) is a separate operation with
    /// its own design, not a flag on this one.
    async fn op_use(&mut self, db: &mut DbConn, args: &Value) -> Result<Value, String> {
        if !self.unlocked() {
            return Err("vault: locked — the broker cannot use a secret before an unlock".into());
        }
        // The grant the broker validated. The vault does not look it up (a cell
        // cannot query another cell inside one handle) — it requires the broker
        // to carry it, and records it.
        let grant_id = arg(args, "grant_id")?.to_string();
        let name = arg(args, "name")?.to_string();
        let action = args.get("use").and_then(|v| v.as_str()).unwrap_or("sign");
        if action != "sign" {
            return Err(format!(
                "vault: unknown use {action:?} — v1 signs (HMAC-SHA256); there is no action that \
                 returns the secret"
            ));
        }
        let payload = arg(args, "payload")?.to_string();

        let n = name.clone();
        let sealed = db
            .call(move |c| store::active(c, &n))
            .await?
            .ok_or_else(|| {
                format!("vault: no such secret {name:?} (or every version is revoked)")
            })?;
        let key = self.key.as_ref().expect("unlocked checked above");
        let plaintext = key
            .open(&sealed.nonce, &sealed.ciphertext)
            .map_err(|e| format!("vault: {e}"))?;
        let tag = crypto::mac(&plaintext, payload.as_bytes());
        // `plaintext` dies here, at the end of this scope, having never been
        // put into a body.
        Ok(json!({
            "name": name,
            "version": sealed.version,
            "grant_id": grant_id,
            "signature": crypto::hex(&tag),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn p(broker: &str) -> VaultParams {
        VaultParams::parse(&json!({"broker": broker})).unwrap()
    }

    #[test]
    fn there_is_no_operation_that_returns_a_secret() {
        assert!(!OPS.contains(&"get"), "a vault with a get is a store");
        assert!(!OPS.contains(&"read"));
        assert!(!OPS.contains(&"export"));
        assert_eq!(OPS.len(), 7);
    }

    #[test]
    fn a_source_message_is_the_user_channel_and_an_edge_never_is() {
        let params = p("/main/access/broker");
        let vault = "/main/access/vault";
        assert_eq!(classify(None, &params, vault), Caller::UserChannel);
        assert_eq!(
            classify(Some(&Path::new("/main/access/broker")), &params, vault),
            Caller::Broker
        );
        assert_eq!(
            classify(Some(&Path::new("/main/egon/brain")), &params, vault),
            Caller::Foreign("/main/egon/brain".into())
        );
    }

    #[test]
    fn only_the_user_channel_may_put_and_only_the_broker_may_use() {
        assert!(Caller::UserChannel.may("put"));
        assert!(!Caller::UserChannel.may("use"));
        assert!(Caller::Broker.may("use"));
        assert!(
            !Caller::Broker.may("put"),
            "a broker that can plant a secret makes the filling workflow pointless"
        );
        for op in OPS {
            assert!(
                !Caller::Foreign("/x".into()).may(op),
                "a foreign sender may do nothing at all, not even {op}"
            );
        }
    }

    #[test]
    fn error_codes_are_stable_for_the_wire() {
        assert_eq!(
            error_code_for("vault: locked — unlock first"),
            "vault_locked"
        );
        assert_eq!(
            error_code_for("vault: unlock refused, attestation failed — unexpected_neighbors: /x"),
            "attestation_failed"
        );
        assert_eq!(
            error_code_for("vault: no such secret \"tg\""),
            "unknown_secret"
        );
        assert_eq!(
            error_code_for("vault: missing or empty argument \"name\""),
            "invalid_input"
        );
    }

    #[test]
    fn an_empty_argument_is_missing_rather_than_accepted() {
        assert!(arg(&json!({"name": ""}), "name").is_err());
        assert!(arg(&json!({}), "name").is_err());
        assert_eq!(arg(&json!({"name": "tg"}), "name").unwrap(), "tg");
    }
}
