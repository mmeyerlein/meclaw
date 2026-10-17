//! `WebhookCell`: the handler half of a `webhook` proxy.
//!
//! The mount has judged every request before this half sees it; this half only
//! emits. `handle()` answers a message sent to the cell with a refusal: a
//! webhook mount is one-way and every key is immutable, so there is nothing a
//! message could ask of it. No mutex, no second task, and the `cell.db` stays
//! empty: no cursor, no dedup (the receiver dedups on the sender's delivery id,
//! which `headers_allow` lets through).
//!
//! GH #1106: the one message this half takes is the sealed box of its own
//! credential round. A mount with `verify.secret_grant_id` asks the vault in
//! `on_start` (and again after every round the mount half reports empty),
//! opens the box in RAM and hands the secret to the mount half; it is never
//! written to `cell.db` and never printed.

use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellOutput, Message, OriginSink, OutputSink, Path};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::emit::{arrived_emission, crossed_receipt, refused_receipt};
use super::mount::{HookEvent, HookReconfig, WebhookIo, run_io};
use super::params::{WebhookParams, refuse_params_update};
use super::verify::WEBHOOK_UNVERIFIED;
use crate::credential::CredentialSlots;

/// The code a message to the cell and a mount that could not be taken are
/// refused with — the `meclaw` platform's word for the same thing.
const INVALID_INPUT: &str = "invalid_input";

/// The code of a mount that requires a proof and holds no secret.
pub const SECRET_MISSING: &str = "secret_missing";

/// GH #1106: the code of a POST that reached a grant's mount before the
/// vault's box opened -- the word every cell type uses for a credential that
/// was asked for and has not arrived.
pub const CREDENTIAL_PENDING: &str = "credential_pending";

/// GH #1106: the grant of a mount whose secret the vault delivers. Key pair
/// and secret live in RAM only, so every (re)start asks again.
struct HookCredential {
    /// `params.verify.secret_grant_id`.
    grant: String,
    /// The slot of that grant. Nothing is ever parked in it: a POST before
    /// the box is answered `503` by the mount half at once.
    slots: CredentialSlots<()>,
}

/// The `webhook` proxy. State lives single-threaded in the handler sub-task.
pub struct WebhookCell {
    params: WebhookParams,
    emit_to: Path,
    /// The mount half, consumed exactly once by `split_io`.
    initial_io_cfg: Option<WebhookIo>,
    /// GH #1106: `Some` when the secret comes sealed under a grant.
    credential: Option<HookCredential>,
}

impl WebhookCell {
    /// A cell for `p`; its mount half comes with [`Self::with_io`]. A grant
    /// in `verify.secret_grant_id` makes it ask the vault in `on_start`.
    pub fn new(p: &WebhookParams) -> Self {
        let credential = p.grant().map(|grant| {
            let on_expired: crate::credential::ExpiryFn<()> = Arc::new(|_, _| Box::pin(async {}));
            HookCredential {
                grant: grant.to_string(),
                slots: CredentialSlots::new(
                    crate::credential::default_credential_wait_ms(),
                    crate::credential::default_credential_wait_max(),
                    on_expired,
                ),
            }
        });
        Self {
            params: p.clone(),
            emit_to: p.emit_to_path(),
            initial_io_cfg: None,
            credential,
        }
    }

    /// The public recipient key of the question in flight (tests/diagnosis;
    /// never a secret).
    pub fn credential_recipient_hex(&self) -> Option<String> {
        let c = self.credential.as_ref()?;
        c.slots.recipient_hex(&c.grant)
    }

    /// GH #1106: ask the access hive for the secret. Emitted towards
    /// `emit_to` like every other emission of this cell; the edge on
    /// `hop.route == "credential_request"` decides where it goes.
    async fn ask_for_secret(&mut self, sink: &OriginSink) {
        let Some(c) = self.credential.as_mut() else {
            return;
        };
        let content = match c.slots.request(&c.grant) {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(error = %e, "proxy/webhook: no random source for a credential request");
                return;
            }
        };
        tracing::info!(grant = %c.grant, mount = %self.params.mount,
            "proxy/webhook: asking the vault for the mount's secret");
        let _ = sink
            .emit(CellOutput {
                target: self.emit_to.clone(),
                content,
            })
            .await;
    }

    /// GH #1106: take a sealed delivery. Opened -> the mount half gets the
    /// secret and opens. Not opened -> the mount stays closed; the next round
    /// asks again. Never echoes a value.
    async fn accept_secret(&mut self, content: &Value, reconfig_tx: &mpsc::Sender<HookReconfig>) {
        let Some(c) = self.credential.as_mut() else {
            return;
        };
        match c.slots.accept_sealed(content).await {
            Ok(accepted) => {
                let Some(secret) = c.slots.secret_handle(&accepted.grant) else {
                    tracing::warn!(grant = %accepted.grant,
                        "proxy/webhook: the sealed secret opened empty -- the mount stays closed");
                    return;
                };
                tracing::info!(grant = %accepted.grant,
                    "proxy/webhook: secret received sealed and opened in RAM -- the mount opens");
                let _ = reconfig_tx.send(HookReconfig::Credential { secret }).await;
            }
            Err(refusal) => match refusal.detail() {
                None => tracing::warn!(
                    "proxy/webhook: a sealed box of an earlier credential round arrived late and \
                     was discarded"
                ),
                Some(detail) => tracing::warn!(%detail,
                    "proxy/webhook: the sealed secret was refused -- the mount stays closed and \
                     asks again after the round's wait"),
            },
        }
    }

    /// Hands the cell its mount half.
    pub fn with_io(mut self, io: WebhookIo) -> Self {
        self.initial_io_cfg = Some(io);
        self
    }

    /// The emission for one event from the mount.
    fn content_for(&self, event: HookEvent) -> Vec<Value> {
        let route = self.params.route.as_str();
        let mount = self.params.mount.as_str();
        match event {
            HookEvent::Arrived(body) => {
                vec![arrived_emission(route, body), crossed_receipt(route, mount)]
            }
            HookEvent::Refused(u) => {
                vec![refused_receipt(
                    route,
                    mount,
                    WEBHOOK_UNVERIFIED,
                    u.detail(),
                )]
            }
            HookEvent::SecretMissing => vec![refused_receipt(
                route,
                mount,
                SECRET_MISSING,
                "verify.secret is bound to an empty value; this mount answers every request \
                 503 until the variable is set and the colony restarted",
            )],
            HookEvent::MountFailed(e) => vec![refused_receipt(route, mount, INVALID_INPUT, &e)],
            HookEvent::CredentialPending => vec![refused_receipt(
                route,
                mount,
                CREDENTIAL_PENDING,
                "the secret of verify.secret_grant_id has not arrived from the vault yet; this \
                 mount answers every request 503 until it does",
            )],
            // Handled in `handle_event` (it asks again); it emits nothing itself.
            HookEvent::CredentialRoundExpired { .. } => Vec::new(),
        }
    }
}

impl LongRunningCell for WebhookCell {
    type Event = HookEvent;
    type Reconfig = HookReconfig;
    type Io = WebhookIo;

    /// A second call finds no mount half and gets one on a table of its own:
    /// it serves nobody, which is the honest answer to a wiring nobody made.
    fn split_io(&mut self) -> Self::Io {
        self.initial_io_cfg.take().unwrap_or_else(|| {
            WebhookIo::new(
                &self.params,
                "",
                Arc::new(meclaw_colony::SurfaceRegistry::new()),
            )
        })
    }

    /// The mount half; see [`run_io`]. `clippy::manual_async_fn`: the same
    /// stable false positive as `crate::proxy::meclaw::cell::MeclawCell::run_io`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io, events_tx, reconfig_rx)
    }

    /// GH #1106: a grant's mount asks for its secret before anything else;
    /// key pair and secret are RAM-only, so a respawn asks again.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { self.ask_for_secret(sink).await }
    }

    /// A message to the cell: one `refused` receipt -- except, for a grant's
    /// mount, the sealed box of its own credential round (GH #1106), which
    /// never touches `cell.db` and is never answered with an emission.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            if self.credential.is_some()
                && let meclaw_core::Body::Inline(v) = &msg.body
                && v.get("sealed").is_some()
            {
                self.accept_secret(v, reconfig_tx).await;
                return;
            }
            let params_slot = matches!(&msg.body, meclaw_core::Body::Inline(v)
                if v.get("params").is_some());
            let detail = if params_slot {
                refuse_params_update()
            } else {
                "a webhook proxy is one-way: it receives requests at its mount and takes no \
                 messages"
                    .to_string()
            };
            let content = refused_receipt(
                &self.params.route,
                &self.params.mount,
                INVALID_INPUT,
                &detail,
            );
            let _ = sink
                .push(CellOutput {
                    target: msg.target.clone(),
                    content,
                })
                .await;
        }
    }

    /// The incoming direction: the mount has judged, this only emits. Each is
    /// a source emission with a fresh trace — a request from outside starts
    /// one.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // GH #1106: a round ended without a box -> ask again with a fresh
            // recipient. A box that opened in between makes it moot.
            if let HookEvent::CredentialRoundExpired { round, wait_ms } = event {
                if let Some(c) = &self.credential
                    && c.slots.secret(&c.grant).is_none()
                {
                    tracing::warn!(round, wait_ms, grant = %c.grant,
                        "proxy/webhook: no sealed secret arrived in time -- asking again");
                    self.ask_for_secret(sink).await;
                }
                return;
            }
            for content in self.content_for(event) {
                let out = CellOutput {
                    target: self.emit_to.clone(),
                    content,
                };
                if sink.emit(out).await.is_err() {
                    tracing::warn!(
                        mount = %self.params.mount,
                        "proxy/webhook: an emission found the colony gone"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::webhook::verify::Unverified;
    use serde_json::json;

    fn cell() -> WebhookCell {
        let p = WebhookParams::parse(&json!({
            "platform": "webhook", "mount": "probe-hook", "route": "probe", "emit_to": "/",
            "verify": {"kind": "token", "secret": "tok-921-cell", "header": "X-Probe-Token"}
        }))
        .expect("params");
        WebhookCell::new(&p)
    }

    #[test]
    fn each_event_has_its_emissions_and_none_names_the_secret() {
        let c = cell();
        let arrived = c.content_for(HookEvent::Arrived(json!({"messages": [], "webhook": {}})));
        assert_eq!(arrived.len(), 2, "the arrival and its crossed receipt");
        assert_eq!(arrived[0]["header"]["route"], json!("probe"));
        assert_eq!(arrived[1]["header"]["peer_event"], json!("crossed"));

        let refused = c.content_for(HookEvent::Refused(Unverified::Mismatch));
        assert_eq!(refused.len(), 1, "a refusal emits no arrival");
        assert_eq!(
            refused[0]["header"]["error_code"],
            json!("webhook_unverified")
        );

        let missing = c.content_for(HookEvent::SecretMissing);
        assert_eq!(missing[0]["header"]["error_code"], json!("secret_missing"));

        let failed = c.content_for(HookEvent::MountFailed("taken".into()));
        assert_eq!(failed[0]["header"]["error_code"], json!("invalid_input"));

        // GH #1106: a POST before the box books `credential_pending`; an
        // expired round emits nothing by itself (the handler asks again).
        let pending = c.content_for(HookEvent::CredentialPending);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0]["header"]["error_code"],
            json!("credential_pending")
        );
        meclaw_core::validate_ubf_body(&pending[0]).expect("a UBF body");
        assert!(
            c.content_for(HookEvent::CredentialRoundExpired {
                round: 2,
                wait_ms: 20_000
            })
            .is_empty()
        );

        for v in arrived
            .iter()
            .chain(&refused)
            .chain(&missing)
            .chain(&failed)
        {
            assert!(!v.to_string().contains("tok-921-cell"), "{v}");
            meclaw_core::validate_ubf_body(v).expect("a UBF body");
        }
    }
}
