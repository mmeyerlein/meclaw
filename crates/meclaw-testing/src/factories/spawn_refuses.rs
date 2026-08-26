//! A factory whose `validate_params` accepts and whose `spawn_cell` refuses.
//!
//! It exists to reach the **apply-stage failure** path of the mutation lane —
//! the one that writes `status='failed'` rather than a `rejected` row, after
//! the `in_flight` row already stands. Until GH #404 that path was reachable
//! with any ordinary factory: instantiation did not deserialize the `params` it
//! wrote, so a cell whose params the factory rejects passed validation and died
//! at the spawn step. The boot-parity guard closed that door — a params defect
//! is now refused during staging, pre-destructively, as `invalid_params`.
//!
//! What is left behind that door is real and worth pinning: a spawn can still
//! fail for reasons no parse can see (a listener that cannot bind, an asset
//! that vanished between staging and spawn), and the audit log has to tell that
//! class apart from a validation reject. Since no *shipped* factory may diverge
//! from the parser invariant (`meclaw_colony::CellFactory` docs), the only
//! honest way to stand in that spot is a test factory that diverges on purpose
//! and says so in its name.
//!
//! Deliberately NOT a mock cell: nothing is ever spawned, so there is no cell
//! to write.

use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::{JsonValue, Path};
use std::sync::Arc;

/// The refusal `spawn_cell` returns, verbatim. Public so a test can assert the
/// reason it provoked travelled, rather than matching on prose.
pub const SPAWN_REFUSAL: &str = "spawn refused on purpose (test factory)";

/// Accepts every `params` block and refuses every spawn.
pub struct SpawnRefusesCellFactory;

impl CellFactory for SpawnRefusesCellFactory {
    /// Accepts anything — the divergence from `spawn_cell` is the point.
    fn validate_params(&self, _raw: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _raw: JsonValue,
        _outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        Err(SPAWN_REFUSAL.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn it_validates_what_it_will_not_spawn() {
        let f = SpawnRefusesCellFactory;
        f.validate_params(&json!({}))
            .expect("the whole point: validation passes");
        f.validate_params(&json!({"anything": "at all"}))
            .expect("no params block is refused");
    }
}
