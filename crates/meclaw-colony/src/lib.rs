//! `meclaw-colony`: actor lifecycle, registry, routing, supervision.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod api_dto;
pub mod blob;
pub mod bootstrap;
mod bootstrap_apply;
mod bootstrap_grow;
mod build_task;
pub mod cel_eval;
mod cell_task;
mod colony;
pub mod colony_config;
pub mod colony_dispatch;
pub mod config;
pub mod connectivity;
pub mod db_conn;
pub mod db_transfer;
pub mod dead_letter;
mod drain;
pub mod edge_table;
pub mod env_file;
pub mod factory;
pub mod hive_scope;
pub mod io_liveness;
pub mod long_running_cell;
mod mailbox_rescue;
pub mod mutation;
pub mod neighbourhood;
pub mod path_truth;
pub mod persist;
mod runtime;
pub mod stateful_cell;
pub mod stateless_cell;
pub mod templates;
pub mod term_ack;
pub mod watchdog;

/// Default value for the idle duration (in ms) for stateful cells with
/// `cell.timeout: 0`. Spec: `docs/meclaw-overview.md` § "colony.json — Schema",
/// key `idle_timeout_default_ms`.
///
/// **Since phase-13.5 slice-6 (A7) it is only the seed source for
/// [`ColonyConfig::default`]** — the sole consumer of this constant. All
/// Live-Spawn-Pfade (Bootstrap-apply, Mutation-Spawn, Subtree-Instanziierung,
/// swap re-spawn, R3 rollback) read the default from
/// `colony_config.idle_timeout_default_ms` (filled from `colony.json`, otherwise
/// from this constant), never directly from the constant. Per-cell override via
/// `cell.idle_timeout_ms` (see 13-B-2/3).
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 60_000;

pub use blob::{
    AttachmentBytes, AttachmentReadError, AttachmentReader, BlobError, BlobSidecar, DiskBlobStore,
};
pub use bootstrap::{
    BootState, BootstrapError, BootstrapErrors, BootstrapPlan, PlannedCell, PlannedEdge,
    PlannedGrowth, PlannedHive, plan_bootstrap, plan_bootstrap_with_env, probe_boot_state,
    registered_hive_paths,
};
pub use bootstrap_apply::{
    BootstrapReport, apply_bootstrap_plan, boot_edges_from_graph, bootstrap_from_filesystem,
    bootstrap_from_filesystem_with_env, declared_slot_endpoints, unresolved_boot_endpoints,
};
pub use build_task::{
    build_long_running_task, build_stateful_task_with_peace, build_stateless_boot_inactive_respawn,
    build_stateless_task, renotify_stop_wiring,
};
pub use cell_task::{cell_task, cell_task_long_running, cell_task_stateful, stateless_dispatcher};
pub use colony::{
    CellStatus, ColonyMsg, ColonyTaskConfig, DeathKind, EgressPolicy, NodeContract, RegistryEntry,
    RespawnFn, colony_task, set_term_timeout_ms_for_test, spawn_watcher,
};
pub use colony_config::{
    COLONY_CONFIG_SCHEMA_VERSION, ColonyConfig, ConfigError, resolve_message_timeout,
};
pub use colony_dispatch::mutation_door_reply;
pub use config::{CellHeader, EdgeSpec, GraphHints, HiveParams, ParsedConfig};
pub use db_conn::{DbConn, QueryTimeout};
pub use dead_letter::{DeadLetter, DeadLetterReason};
pub use edge_table::{Edge, EdgeDecision, EdgeTable, apply_edges, evaluate_edge};
pub use factory::{CellFactory, CellFactoryRegistry, ContractView, SpawnedCellKind, WakeFn};
pub use hive_scope::{HiveScope, HiveScopeTable};
pub use io_liveness::IoLivenessMark;
pub use long_running_cell::LongRunningCell;
pub use mutation::{
    ManifestBody, ManifestError, ManifestOutcome, MutationDoorOutcome, MutationError,
    MutationOutcome,
};
pub use neighbourhood::{NeighbourhoodError, NeighbourhoodView};
pub use persist::colony_db::{ColonyDb, RegistryOverlay, read_registry_overlay};
pub use runtime::ColonyRuntime;
pub use stateful_cell::StatefulCell;
pub use stateless_cell::StatelessCell;
pub use term_ack::TermAckGuard;
pub use watchdog::{
    HostWitness, Watchdog, WatchdogAction, WatchdogOnTrip, WatchdogTrip, WorkItem, WorkPulse,
};

#[cfg(test)]
mod tests {
    #[test]
    fn default_idle_timeout_ms_is_60000() {
        assert_eq!(super::DEFAULT_IDLE_TIMEOUT_MS, 60_000);
    }
}

#[cfg(test)]
mod phase_4_dep_smoke {
    #[test]
    fn serde_derive_compiles() {
        #[derive(serde::Deserialize)]
        struct Probe {
            _x: i32,
        }
        let _: Probe = meclaw_core::serde_json::from_str(r#"{"_x": 1}"#).unwrap();
    }

    #[test]
    fn tempfile_dep_available() {
        let _td = tempfile::TempDir::new().expect("tempfile creates tmp dir");
    }
}

#[cfg(test)]
mod phase_5_dep_smoke {
    #[test]
    fn rusqlite_in_memory_create_table_works() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch("CREATE TABLE probe (x INTEGER);")
            .expect("DDL ok");
    }
}
