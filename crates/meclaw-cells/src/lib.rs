//! `meclaw-cells`: built-in cell-types (Phase 7+).
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod bash;
pub mod boundary;
pub mod code;
pub mod edit;
pub mod file;
pub mod harness;
pub mod llm;
pub mod mcp;
pub mod orphan_journal;
pub mod params_overlay;
pub(crate) mod process;
pub mod proxy;
pub mod sandbox;
pub mod stdio_child;
pub mod store;
pub mod subcolony;
pub mod timer;
pub mod tool;
pub mod vault;
pub mod web_fetch;
pub mod web_search;
pub use bash::{BashCell, BashCellFactory};
pub use edit::{EditCell, EditCellFactory};
pub use file::{FileCell, FileCellFactory};
pub use llm::{LlmCellFactory, LlmParams};
pub use mcp::McpCellFactory;
pub use proxy::ProxyCellFactory;
pub use timer::TimerCellFactory;
pub use web_fetch::{WebFetchCell, WebFetchCellFactory};
pub use web_search::{WebSearchCell, WebSearchCellFactory};
