//! The outgoing half of the carrier: one POST under one operation timeout.
//!
//! `Connection: close` is deliberate: the far mount serves per handed-over
//! connection (`crate::handed::serve_handed`), and a keep-alive outliving a
//! crossing would hold one of sixteen handoff-queue places for a peer with
//! nothing more to say.
//!
//! Redirects are not followed: a 3xx is a non-200 and ends as
//! `peer_unreachable`. Following one would re-POST the frame to a URL the peer
//! chose, and the receipt could come from a third party no operator declared.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONNECTION, CONTENT_TYPE};
use serde_json::Value as JsonValue;

use super::lanes::Refusal;
use super::wire::{INVALID_FRAME, PEER_TIMEOUT, PEER_UNREACHABLE};

/// The HTTP client a `meclaw` proxy posts frames with. `Clone` is cheap.
#[derive(Clone)]
pub struct PeerClient {
    inner: reqwest::Client,
}

impl PeerClient {
    /// Builds the client. A build failure (e.g. TLS init) is a spawn error.
    pub fn new() -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("reqwest build: {e}"))?;
        Ok(Self { inner })
    }

    /// POSTs `frame` to `url` and returns the parsed answer. The whole send is
    /// wrapped in `timeout_ms` (hard rule 12): elapsed is `peer_timeout`,
    /// anything the carrier did not finish is `peer_unreachable`, an answer
    /// that is not JSON is `invalid_frame`. The verdict inside a well-formed
    /// answer is `wire::read_receipt`'s to read.
    pub async fn post_frame(
        &self,
        url: &str,
        frame: &JsonValue,
        timeout_ms: u64,
    ) -> Result<JsonValue, Refusal> {
        let send = async {
            let resp = self
                .inner
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .header(CONNECTION, "close")
                .body(frame.to_string())
                .send()
                .await
                .map_err(|e| {
                    Refusal::new(PEER_UNREACHABLE, format!("no answer from {url}: {e}"))
                })?;
            let status = resp.status();
            if status != StatusCode::OK {
                return Err(Refusal::new(
                    PEER_UNREACHABLE,
                    format!("{url} answered {status}, not 200"),
                ));
            }
            let bytes = resp.bytes().await.map_err(|e| {
                Refusal::new(
                    PEER_UNREACHABLE,
                    format!("the answer from {url} broke off: {e}"),
                )
            })?;
            serde_json::from_slice::<JsonValue>(&bytes).map_err(|e| {
                Refusal::new(
                    INVALID_FRAME,
                    format!("the answer from {url} is not JSON: {e}"),
                )
            })
        };
        match tokio::time::timeout(Duration::from_millis(timeout_ms), send).await {
            Ok(result) => result,
            Err(_) => Err(Refusal::new(
                PEER_TIMEOUT,
                format!("no receipt from {url} within {timeout_ms} ms"),
            )),
        }
    }
}
