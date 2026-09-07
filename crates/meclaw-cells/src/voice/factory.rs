//! The factory for the `voice` cell.
//!
//! Same corridor duty as every long-running type (phase-5 tripwire):
//! everything between the `cell.db` open and the long-running spawn is sync and
//! await-free, because the `RespawnFn` runs inside the colony's restart barrier
//! and an `.await` in there is a deadlock waiting for a restart.
//!
//! `is_lazy` stays `false`, for the reason the `web` cell gives: an endpoint
//! must be up when the colony is. A voice cell that waited for its first
//! message would refuse the first caller.

use crate::voice::cell::VoiceCell;
use crate::voice::io::VoiceIo;
use crate::voice::params::{VoiceOverlay, VoiceParams};
use crate::voice::providers::{build_stt, build_tts};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The six-tuple a (re)spawn hands back: mailbox sender, join handle, and the
/// four lifecycle oneshot ends minted by `build_long_running_task`.
type SpawnTuple = (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// The `voice` cell factory.
pub struct VoiceCellFactory;

impl CellFactory for VoiceCellFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        VoiceParams::parse(params).map(|_| ())
    }

    fn type_name(&self) -> &'static str {
        "voice"
    }

    /// A `voice` cell's one table is the params overlay, created in code — so
    /// the mutation staging seeder keeps out of its database (GH #398), and the
    /// default `validate_cell_dir` refuses a `seed/*.jsonl` beside it, which is
    /// right: nothing would read it.
    fn owns_schema(&self) -> bool {
        true
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_path = path.clone();
        let build = make_build(
            params,
            path,
            outputs_tx,
            cell_dir,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
            contract.transfer_bounds(),
        )?;

        let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
        let respawn: RespawnFn = Box::new(move || {
            let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
            meclaw_colony::renotify_stop_wiring(
                &respawn_inbox,
                respawn_path.clone(),
                stop_tx,
                death_ack_rx,
            );
            (sender, join, peace_rx, backstop_rx)
        });

        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    /// Hand out a real `RespawnFn` for a `voice` cell that booted inactive,
    /// without spawning the task yet — no listener is opened until a reconnect
    /// asks for one. Same construction as `spawn_cell`'s respawn.
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_path = path.clone();
        let build = make_build(
            params,
            path,
            outputs_tx,
            cell_dir,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
            contract.transfer_bounds(),
        )
        .ok()?;
        Some(Box::new(move || {
            let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
            meclaw_colony::renotify_stop_wiring(
                &respawn_inbox,
                respawn_path.clone(),
                stop_tx,
                death_ack_rx,
            );
            (sender, join, peace_rx, backstop_rx)
        }))
    }
}

/// The two deadlines an adapter is held to, derived from the params it will
/// actually run under.
///
/// Named rather than inlined because the build closure and its test have to be
/// looking at the same computation: the closure derives these from the
/// **effective** params (birth ⊕ the `cell.db` overlay), so a timeout an
/// operator moved is the one the providers get on the next life.
pub(crate) fn provider_timeouts(p: &VoiceParams) -> crate::voice::contract::ProviderTimeouts {
    crate::voice::contract::ProviderTimeouts {
        external: Duration::from_millis(p.external_timeout_ms),
        idle: Duration::from_millis(p.provider_idle_timeout_ms),
    }
}

/// Build the closure that constructs a fresh `voice` cell-task.
///
/// Params are parsed once, outside the closure, so a params error is a spawn
/// failure rather than a panic on the respawn path. The providers are built
/// there too, and for the same reason: a provider name nothing answers to must
/// refuse the spawn, not the first caller.
///
/// The adapters the cell actually runs with are built **inside** the closure,
/// from the effective params. `stt` and `tts` cannot move at runtime — they are
/// not in [`crate::voice::params::VoiceOverlay::KNOWN_KEYS`] — but the deadlines
/// they are held to can, and an adapter built once at birth would keep the old
/// ones for the rest of the colony's life. The birth pair stays as the fallback
/// for the one case the closure cannot report: a build that fails on the
/// respawn path, where there is nowhere to put an error.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn make_build(
    params: JsonValue,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell_dir: PathBuf,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
) -> Result<impl Fn() -> SpawnTuple, String> {
    let birth_parsed = VoiceParams::parse(&params)?;
    // Built here so an unknown provider is a spawn failure rather than the
    // first caller's problem. Kept as the fallback for the respawn path.
    let birth_timeouts = provider_timeouts(&birth_parsed);
    let birth_stt = build_stt(&birth_parsed.stt, birth_timeouts)?;
    let birth_tts = match &birth_parsed.tts {
        Some(t) => Some(build_tts(t, birth_timeouts)?),
        None => None,
    };

    let birth_cap = params;
    let birth_parsed_cap = birth_parsed;
    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    let bounds_cap = bounds;

    Ok(move || -> SpawnTuple {
        // 1. Open cell.db (sync). The `params` overlay table comes with every
        //    cell.db, and a `voice` cell needs nothing else: a connection does
        //    not survive a respawn, and a table pretending otherwise would be
        //    a lie on disk.
        let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");

        // 2. Restore: effective params = birth ⊕ the `cell.db` overlay. An
        //    endpoint moved by a params update must come back where it was
        //    moved to, or the crash of a cell would quietly undo an operator's
        //    move. A corrupt overlay is not worth a panic on the restart
        //    barrier — the birth params are a working endpoint, loudly.
        let effective_raw = match crate::params_overlay::restore::<VoiceOverlay>(&conn, &birth_cap)
        {
            Ok(o) => meclaw_core::serde_json::to_value(&o).unwrap_or_else(|_| birth_cap.clone()),
            Err(e) => {
                tracing::error!(
                    path = path_cap.as_str(),
                    error = %e,
                    "voice: could not replay the params overlay — this endpoint \
                     starts on its birth params"
                );
                birth_cap.clone()
            }
        };
        let parsed = VoiceParams::parse(&effective_raw).unwrap_or_else(|e| {
            tracing::error!(
                path = path_cap.as_str(),
                error = %e,
                "voice: the replayed params did not parse — birth params it is"
            );
            birth_parsed_cap.clone()
        });

        // 3. Build the adapters for THIS life, from the effective params, so a
        //    moved timeout takes effect on the next respawn. The birth pair is
        //    the fallback: this closure is the `RespawnFn` and has nowhere to
        //    report a failure, and the birth adapters are a working cell.
        let timeouts = provider_timeouts(&parsed);
        let stt = build_stt(&parsed.stt, timeouts).unwrap_or_else(|e| {
            tracing::error!(
                path = path_cap.as_str(),
                error = %e,
                "voice: could not rebuild the speech-to-text adapter — this life \
                 keeps the one it was born with"
            );
            birth_stt.clone()
        });
        let tts = match &parsed.tts {
            Some(t) => build_tts(t, timeouts).map(Some).unwrap_or_else(|e| {
                tracing::error!(
                    path = path_cap.as_str(),
                    error = %e,
                    "voice: could not rebuild the text-to-speech adapter — this \
                     life keeps the one it was born with"
                );
                birth_tts.clone()
            }),
            None => None,
        };

        // 4. Build both halves (sync). The events channel the substrate mints
        //    replaces the placeholder in `VoiceCell::run_io`.
        let (placeholder_tx, _placeholder_rx) = mpsc::channel(1);
        let mut io = VoiceIo::new(
            parsed.bind.clone(),
            parsed.port,
            stt,
            tts,
            parsed.default_mode,
            Duration::from_millis(parsed.external_timeout_ms),
            Duration::from_millis(parsed.provider_idle_timeout_ms),
            placeholder_tx,
        );
        // Read here, like the two timeouts: this life frames at the value it
        // was born with, and a moved one takes effect on the next respawn.
        io.audio_out_frame_ms = parsed.audio_out_frame_ms;
        let cell = VoiceCell::new(path_cap.clone(), io, &parsed, &effective_raw);
        let db = DbConn::wrap(
            conn,
            Some(Duration::from_millis(parsed.external_timeout_ms)),
        );
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity_cap);
        // 5. The single long-running spawn site. No `.await` reached this line.
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_long_running_task(
            path_cap.clone(),
            rx,
            outputs_cap.clone(),
            64,
            cell,
            db,
            Some(colony_inbox_cap.clone()),
            blob_cap.clone(),
            consumes_cap.clone(),
            bounds_cap.clone(),
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::params::VoiceOverlay;
    use meclaw_core::serde_json::json;

    /// MUST 2: the deadlines the adapters are held to come from the effective
    /// params, not from the birth ones.
    ///
    /// This pins the computation the build closure runs rather than the spawn
    /// itself: the closure is a `RespawnFn` and starts a task, so driving it
    /// from a test would say more about tokio than about this rule. What it
    /// derives — restore the overlay, re-parse, take the timeouts from THAT —
    /// is [`provider_timeouts`] over the same value, and that is what moves
    /// when an operator moves a timeout.
    #[test]
    fn respawn_builds_providers_from_the_overlay() {
        let birth = json!({
            "port": 7900,
            "stt": {"provider": "echo"},
            "external_timeout_ms": 5000,
            "provider_idle_timeout_ms": 30000
        });
        let birth_parsed = VoiceParams::parse(&birth).expect("birth params parse");
        assert_eq!(
            provider_timeouts(&birth_parsed).external,
            Duration::from_millis(5000)
        );

        let td = tempfile::tempdir().expect("tempdir");
        let (mut conn, _status) = open_or_create_cell_db_with_status(&td.path().join("cell.db"))
            .expect("a fresh cell.db");
        let overlay = vec![
            ("external_timeout_ms".to_string(), json!(1234)),
            ("provider_idle_timeout_ms".to_string(), json!(4321)),
        ];
        crate::params_overlay::persist_params_overlay(
            &mut conn,
            &overlay,
            crate::params_overlay::now_unix_seconds(),
        )
        .expect("persist the overlay");

        // Exactly what the closure does: restore, serialise, re-parse.
        let restored = crate::params_overlay::restore::<VoiceOverlay>(&conn, &birth)
            .expect("the overlay replays");
        let effective_raw = meclaw_core::serde_json::to_value(&restored).expect("serialise");
        let effective = VoiceParams::parse(&effective_raw).expect("effective params parse");

        let t = provider_timeouts(&effective);
        assert_eq!(
            t.external,
            Duration::from_millis(1234),
            "an adapter built from the birth params would still be on 5000"
        );
        assert_eq!(t.idle, Duration::from_millis(4321));
        // And the provider identity is untouched by all of it.
        assert!(matches!(
            effective.stt,
            crate::voice::params::SttParams::Echo
        ));
    }
}
