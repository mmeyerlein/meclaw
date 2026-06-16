//! `meclaw-colony`: actor lifecycle, registry, routing, supervision.

pub mod api_dto;
pub mod blob;
pub mod bootstrap;
mod bootstrap_apply;
mod build_task;
pub mod cel_eval;
mod cell_task;
mod colony;
pub mod colony_config;
pub mod colony_dispatch;
pub mod config;
pub mod connectivity;
pub mod db_conn;
pub mod dead_letter;
pub mod edge_table;
pub mod env_file;
pub mod factory;
pub mod hive_scope;
pub mod long_running_cell;
pub mod mutation;
pub mod path_truth;
pub mod persist;
mod runtime;
pub mod stateful_cell;
pub mod stateless_cell;
pub mod templates;
pub mod term_ack;
pub mod watchdog;

/// Default-Wert für die Idle-Dauer (in ms) bei stateful Cells mit
/// `cell.timeout: 0`. Spec: `docs/meclaw-overview.md` § "colony.json — Schema",
/// Schlüssel `idle_timeout_default_ms`.
///
/// **Seit Phase-13.5 Slice-6 (A7) nur noch Seed-Quelle für
/// [`ColonyConfig::default`]** — der einzige Konsument dieser Konstante. Alle
/// Live-Spawn-Pfade (Bootstrap-apply, Mutation-Spawn, Subtree-Instanziierung,
/// Swap-Re-Spawn, R3-Rollback) lesen den Default aus
/// `colony_config.idle_timeout_default_ms` (gefüllt aus `colony.json`, sonst aus
/// dieser Konstante), nie direkt aus der Konstante. Pro-Cell-Override via
/// `cell.idle_timeout_ms` (siehe 13-B-2/3).
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 60_000;

pub use blob::{BlobError, BlobSidecar, DiskBlobStore};
pub use bootstrap::{
    BootState, BootstrapError, BootstrapErrors, BootstrapPlan, PlannedCell, PlannedEdge,
    PlannedHive, plan_bootstrap, plan_bootstrap_with_env, probe_boot_state,
};
pub use bootstrap_apply::{
    BootstrapReport, apply_bootstrap_plan, bootstrap_from_filesystem,
    bootstrap_from_filesystem_with_env, unresolved_boot_endpoints,
};
pub use build_task::{
    build_long_running_task, build_stateful_task_with_peace, build_stateless_boot_inactive_respawn,
    build_stateless_task, renotify_stop_wiring,
};
pub use cell_task::{cell_task, cell_task_long_running, cell_task_stateful, stateless_dispatcher};
pub use colony::{
    ColonyMsg, ColonyTaskConfig, DeathKind, NodeContract, RegistryEntry, RespawnFn, colony_task,
    set_term_timeout_ms_for_test, spawn_watcher,
};
pub use colony_config::{
    COLONY_CONFIG_SCHEMA_VERSION, ColonyConfig, ConfigError, resolve_message_timeout,
};
pub use config::{CellHeader, EdgeSpec, GraphHints, HiveParams, ParsedConfig};
pub use db_conn::{DbConn, QueryTimeout};
pub use dead_letter::{DeadLetter, DeadLetterReason};
pub use edge_table::{Edge, EdgeDecision, EdgeTable, apply_edges, evaluate_edge};
pub use factory::{CellFactory, CellFactoryRegistry, ContractView, SpawnedCellKind, WakeFn};
pub use hive_scope::{HiveScope, HiveScopeTable};
pub use long_running_cell::LongRunningCell;
pub use mutation::{MutationError, MutationOutcome};
pub use persist::colony_db::{ColonyDb, RegistryOverlay, read_registry_overlay};
pub use runtime::ColonyRuntime;
pub use stateful_cell::StatefulCell;
pub use stateless_cell::StatelessCell;
pub use term_ack::TermAckGuard;
pub use watchdog::{Watchdog, WatchdogAction};

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
