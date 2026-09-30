//! `WebhookCell`: the handler half of a `webhook` proxy.
//!
//! The mount has judged every request before this half sees it; this half only
//! emits. `handle()` answers a message sent to the cell with a refusal: a
//! webhook mount is one-way and every key is immutable, so there is nothing a
//! message could ask of it. No mutex, no second task, and the `cell.db` stays
//! empty: no cursor, no dedup (the receiver dedups on the sender's delivery id,
//! which `headers_allow` lets through).

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

/// The code a message to the cell and a mount that could not be taken are
/// refused with — the `meclaw` platform's word for the same thing.
const INVALID_INPUT: &str = "invalid_input";

/// The code of a mount that requires a proof and holds no secret.
pub const SECRET_MISSING: &str = "secret_missing";

/// The `webhook` proxy. State lives single-threaded in the handler sub-task.
pub struct WebhookCell {
    params: WebhookParams,
    emit_to: Path,
    /// The mount half, consumed exactly once by `split_io`.
    initial_io_cfg: Option<WebhookIo>,
}

impl WebhookCell {
    /// A cell for `p`; its mount half comes with [`Self::with_io`].
    pub fn new(p: &WebhookParams) -> Self {
        Self {
            params: p.clone(),
            emit_to: p.emit_to_path(),
            initial_io_cfg: None,
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

    /// A message to the cell: always one `refused` receipt.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
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
