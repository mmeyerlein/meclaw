//! GH #1059: provider slots whose key the vault delivers.
//!
//! A voice cell serves sessions from the first second of its life, but a slot
//! with a `credential_grant_id` has no key until the handler opened the sealed
//! box its `on_start` question brought back. So such a slot is not the adapter
//! itself but a stand-in for it:
//!
//! - The stand-in answers every DECLARATION from a template adapter built from
//!   the same params with the literal key blanked (`name`, formats, rates —
//!   nothing of that depends on a key). `hello` and `GET /info` read the same
//!   wiring before and after the box.
//! - A SESSION (`run_session`, `synthesize`) waits for the keyed adapter, at
//!   most the slot's `credential_wait_ms`, and is then refused with a named
//!   reason instead of reaching the vendor without a key. The template is never
//!   run, so neither an empty key nor an ignored literal reaches a wire.
//! - The keyed adapter arrives over a `watch` channel the I/O half fills when
//!   the handler hands it the opened key ([`VaultedSlot::install`]). A channel
//!   rather than a lock in shared state (`AGENTS.md`: no `Mutex` in cell
//!   state); once filled, every later session finds it at once — the wait is
//!   paid only before the very first box of a life.
//!
//! Slots are independent: each has its own stand-in, channel and round clock,
//! so a delivered `stt` key starts recognition while `tts` still waits.

use crate::credential::Secret;
use crate::credential_rounds::RoundClock;
use crate::voice::contract::{
    AudioFormat, BoxFuture, DuplexError, DuplexProvider, DuplexSession, ProviderTimeouts, SttError,
    SttEvent, SttProvider, TtsError, TtsProvider,
};
use crate::voice::params::{CredentialSlot, VoiceParams};
use crate::voice::providers::{build_duplex, build_stt, build_tts};
use meclaw_colony::io_liveness::IoLivenessMark;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Builds the keyed adapter of a slot from a delivered key and publishes it to
/// the slot's stand-in. Boxed so the I/O half holds the three kinds of slot in
/// one list without knowing which adapter trait each one feeds.
// `Sync` too: the cell holds these, and `&self` crosses an `.await` in
// `handle` — a `Send` future needs every field `Sync` (GH #1059, first build).
type Install = Box<dyn FnMut(&Secret) -> Result<(), String> + Send + Sync>;

/// The I/O half's end of one vaulted slot: what to do with a delivered key,
/// and the clock of the rounds that ask for it.
pub struct VaultedSlot {
    /// Which slot.
    pub slot: CredentialSlot,
    /// The grant it spends (for log lines; never a secret).
    pub grant: String,
    /// The first round's wait, `params.<slot>.credential_wait_ms`.
    pub wait_ms: u64,
    /// `Some` while the slot waits for its key; started when the I/O half
    /// starts, cleared once a key is installed.
    pub(crate) clock: Option<RoundClock>,
    install: Install,
}

impl VaultedSlot {
    /// Build and publish the keyed adapter. Either way the slot's rounds end:
    /// on `Ok` because the key is there, on `Err` because asking again would
    /// bring the same key from the same grant (review V2 M1, GH #1061 — the
    /// clock used to keep firing into a handler that already held a key and
    /// asked nothing). The slot then stays without a key for this life; its
    /// sessions are refused by name, and the next start asks again.
    ///
    /// # Errors
    /// The adapter could not be built from the params with this key.
    pub fn install(&mut self, key: &Secret) -> Result<(), String> {
        let built = (self.install)(key);
        self.clock = None;
        built
    }

    /// Start round 1 now (the `on_start` question is already out).
    pub(crate) fn start_clock(&mut self, backoff_max_ms: u64) {
        self.clock = Some(RoundClock::start(self.wait_ms, backoff_max_ms));
    }
}

/// What a session of a slot waits on: the keyed adapter, once the box opened.
struct Gate<P: ?Sized> {
    slot: CredentialSlot,
    grant: String,
    wait: Duration,
    ready: watch::Receiver<Option<Arc<P>>>,
}

// By hand: a derive would demand `P: Clone`, which no `dyn` adapter is.
impl<P: ?Sized> Clone for Gate<P> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot,
            grant: self.grant.clone(),
            wait: self.wait,
            ready: self.ready.clone(),
        }
    }
}

impl<P: ?Sized + Send + Sync + 'static> Gate<P> {
    /// The keyed adapter — at once when the key is in RAM, else after at most
    /// the slot's wait. `Err` is the refusal's text: it names the slot and the
    /// grant, never a value (there is none yet).
    async fn open(mut self) -> Result<Arc<P>, String> {
        let now = self.ready.borrow().clone();
        if let Some(p) = now {
            return Ok(p);
        }
        // The `Ref` the wait hands back is dropped inside this block, before
        // any further await: it is not `Send`.
        let got = {
            let waited =
                tokio::time::timeout(self.wait, self.ready.wait_for(Option::is_some)).await;
            match waited {
                Ok(Ok(r)) => (*r).clone(),
                _ => None,
            }
        };
        if let Some(p) = got {
            return Ok(p);
        }
        let wait_ms = u64::try_from(self.wait.as_millis()).unwrap_or(u64::MAX);
        tracing::warn!(
            slot = self.slot.as_str(),
            grant = %self.grant,
            wait_ms,
            "voice: a session found no sealed credential for its slot — refused"
        );
        Err(format!(
            "no sealed {} credential arrived from the vault within {wait_ms} ms (grant {})",
            self.slot.as_str(),
            self.grant
        ))
    }
}

/// The `stt` stand-in.
struct VaultedStt {
    template: Arc<dyn SttProvider>,
    gate: Gate<dyn SttProvider>,
}

impl SttProvider for VaultedStt {
    fn name(&self) -> &'static str {
        self.template.name()
    }
    fn input_format(&self) -> AudioFormat {
        self.template.input_format()
    }
    fn input_rates(&self) -> Vec<u32> {
        self.template.input_rates()
    }
    fn negotiate_input(&self, sample_rate: u32) -> Option<AudioFormat> {
        self.template.negotiate_input(sample_rate)
    }
    fn run_session(
        &self,
        format: AudioFormat,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let gate = self.gate.clone();
        Box::pin(async move {
            let keyed = gate.open().await.map_err(SttError::Unavailable)?;
            keyed.run_session(format, audio, events, liveness).await
        })
    }
}

/// The `tts` stand-in.
struct VaultedTts {
    template: Arc<dyn TtsProvider>,
    gate: Gate<dyn TtsProvider>,
}

impl TtsProvider for VaultedTts {
    fn name(&self) -> &'static str {
        self.template.name()
    }
    fn output_format(&self) -> AudioFormat {
        self.template.output_format()
    }
    fn output_rates(&self) -> Vec<u32> {
        self.template.output_rates()
    }
    fn negotiate_output(&self, sample_rate: u32) -> Option<AudioFormat> {
        self.template.negotiate_output(sample_rate)
    }
    fn synthesize(
        &self,
        format: AudioFormat,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let gate = self.gate.clone();
        Box::pin(async move {
            let keyed = gate.open().await.map_err(TtsError::Unavailable)?;
            keyed
                .synthesize(format, text, audio, cancel, liveness)
                .await
        })
    }
}

/// The `duplex` stand-in.
struct VaultedDuplex {
    template: Arc<dyn DuplexProvider>,
    gate: Gate<dyn DuplexProvider>,
}

impl DuplexProvider for VaultedDuplex {
    fn name(&self) -> &'static str {
        self.template.name()
    }
    fn format(&self) -> AudioFormat {
        self.template.format()
    }
    fn rates(&self) -> Vec<u32> {
        self.template.rates()
    }
    fn negotiate(&self, sample_rate: u32) -> Option<AudioFormat> {
        self.template.negotiate(sample_rate)
    }
    fn run_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let gate = self.gate.clone();
        Box::pin(async move {
            let keyed = gate.open().await.map_err(DuplexError::Unavailable)?;
            keyed.run_session(format, session, liveness).await
        })
    }
    fn run_renewed_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let gate = self.gate.clone();
        Box::pin(async move {
            let keyed = gate.open().await.map_err(DuplexError::Unavailable)?;
            keyed.run_renewed_session(format, session, liveness).await
        })
    }
}

/// The adapters of one life after the vault seam: the stand-ins in place of
/// every granted slot's adapter, and the I/O half's ends of those slots.
pub struct Vaulted {
    /// The recogniser to serve sessions with.
    pub stt: Arc<dyn SttProvider>,
    /// The synthesiser, if any.
    pub tts: Option<Arc<dyn TtsProvider>>,
    /// The duplex model, if any.
    pub duplex: Option<Arc<dyn DuplexProvider>>,
    /// One entry per granted slot; empty for a cell on literals alone.
    pub slots: Vec<VaultedSlot>,
}

/// GH #1059: put a stand-in in front of every slot of `params` that spends a
/// grant. `params` must be the life's params with granted literals already
/// blanked ([`VoiceParams::without_granted_literals`]) — the adapters passed in
/// were built from them and serve as the declaration templates.
#[must_use]
pub fn vault(
    params: &VoiceParams,
    timeouts: ProviderTimeouts,
    stt: Arc<dyn SttProvider>,
    tts: Option<Arc<dyn TtsProvider>>,
    duplex: Option<Arc<dyn DuplexProvider>>,
) -> Vaulted {
    let mut slots = Vec::new();
    let stt: Arc<dyn SttProvider> = match params.slot_grant(CredentialSlot::Stt) {
        None => stt,
        Some(grant) => {
            let (gate, tx) = gate::<dyn SttProvider>(params, CredentialSlot::Stt, grant);
            let p = params.clone();
            slots.push(slot(params, CredentialSlot::Stt, grant, move |key| {
                let mut keyed = p.clone();
                keyed.set_key(
                    CredentialSlot::Stt,
                    crate::voice::params::Secret::delivered(key),
                );
                tx.send_replace(Some(build_stt(&keyed.stt, timeouts)?));
                Ok(())
            }));
            Arc::new(VaultedStt {
                template: stt,
                gate,
            })
        }
    };
    let tts: Option<Arc<dyn TtsProvider>> = match (tts, params.slot_grant(CredentialSlot::Tts)) {
        (Some(template), Some(grant)) => {
            let (gate, tx) = gate::<dyn TtsProvider>(params, CredentialSlot::Tts, grant);
            let p = params.clone();
            slots.push(slot(params, CredentialSlot::Tts, grant, move |key| {
                let mut keyed = p.clone();
                keyed.set_key(
                    CredentialSlot::Tts,
                    crate::voice::params::Secret::delivered(key),
                );
                let Some(t) = &keyed.tts else {
                    return Err("the tts block is gone".to_string());
                };
                tx.send_replace(Some(build_tts(t, timeouts)?));
                Ok(())
            }));
            Some(Arc::new(VaultedTts { template, gate }) as Arc<dyn TtsProvider>)
        }
        (tts, _) => tts,
    };
    let duplex_grant = params.slot_grant(CredentialSlot::Duplex);
    let duplex: Option<Arc<dyn DuplexProvider>> = match (duplex, duplex_grant) {
        (Some(template), Some(grant)) => {
            let (gate, tx) = gate::<dyn DuplexProvider>(params, CredentialSlot::Duplex, grant);
            let p = params.clone();
            slots.push(slot(params, CredentialSlot::Duplex, grant, move |key| {
                let mut keyed = p.clone();
                keyed.set_key(
                    CredentialSlot::Duplex,
                    crate::voice::params::Secret::delivered(key),
                );
                let Some(d) = &keyed.duplex else {
                    return Err("the duplex block is gone".to_string());
                };
                tx.send_replace(Some(build_duplex(d, timeouts)?));
                Ok(())
            }));
            Some(Arc::new(VaultedDuplex { template, gate }) as Arc<dyn DuplexProvider>)
        }
        (duplex, _) => duplex,
    };
    Vaulted {
        stt,
        tts,
        duplex,
        slots,
    }
}

/// A slot's gate and the sender its install fills.
fn gate<P: ?Sized>(
    params: &VoiceParams,
    slot: CredentialSlot,
    grant: &str,
) -> (Gate<P>, watch::Sender<Option<Arc<P>>>) {
    let (tx, rx) = watch::channel(None);
    let gate = Gate {
        slot,
        grant: grant.to_string(),
        wait: Duration::from_millis(params.credential(slot).credential_wait_ms),
        ready: rx,
    };
    (gate, tx)
}

/// The I/O half's end of a slot. The key crosses into the build as `&str`
/// only for the length of the build: the adapter keeps it in its own params
/// `Secret`, which prints `<redacted>`.
fn slot(
    params: &VoiceParams,
    slot: CredentialSlot,
    grant: &str,
    mut install: impl FnMut(&str) -> Result<(), String> + Send + Sync + 'static,
) -> VaultedSlot {
    VaultedSlot {
        slot,
        grant: grant.to_string(),
        wait_ms: params.credential(slot).credential_wait_ms,
        clock: None,
        install: Box::new(move |key: &Secret| install(key.expose())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review V2 M1 (GH #1061): a delivered key that does not build the
    /// provider leaves the slot without a key -- and with no clock. Asking
    /// again would bring the same key from the same grant; the old running
    /// clock fired every round into a handler that already held a key and
    /// asked nothing, while the log line promised a new question.
    #[test]
    fn gh1061_a_key_that_builds_no_provider_stops_the_slots_clock() {
        let mut slot = VaultedSlot {
            slot: CredentialSlot::Stt,
            grant: "grant:stub@agent/voice".to_string(),
            wait_ms: 300,
            clock: Some(RoundClock::start(300, 60_000)),
            install: Box::new(|_| Err("the stub provider refuses every key".to_string())),
        };
        assert!(slot.install(&Secret::stub("stub-secret-m1")).is_err());
        assert!(
            slot.clock.is_none(),
            "a slot whose key failed must not keep a round clock"
        );
    }
}
