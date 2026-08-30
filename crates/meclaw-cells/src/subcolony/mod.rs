//! P9 — the `subcolony` cell type: a whole child colony that behaves as ONE
//! cell in the parent tree.
//!
//! Composition, not federation. The parent sees one path, one mailbox, one
//! contract; the child's internal tree is invisible and NOT addressable from
//! outside. That is not a limitation to be relaxed later — transparent
//! cross-colony routing would mean a distributed registry, distributed routing
//! and partial-failure semantics, for no need anyone has today. Any design that
//! opens parent paths into the child tree is a stop, not a feature.
//!
//! Built on the P7 stdio-child core, like `harness`, but the lifecycle is the
//! other way round: a harness runs one child PER TASK and a dead child is the
//! ordinary end of one; here the child is the cell's ability to answer at all,
//! so it lives as long as the cell is awake and its death is a restart (the
//! `mcp` shape).
//!
//! The design is ratified; its shipped surface is documented in
//! `docs/cell-types.md` § `subcolony`.

/// The composition boundary: what crosses and what does not.
pub mod wire;

/// Birth configuration (`params`).
pub mod params;

/// The handler side of the cell.
pub mod cell;

/// The three emission shapes.
pub mod emit;

/// The cell factory.
pub mod factory;

/// The I/O sub-task: one child colony for the life of the cell.
pub mod io;

pub use cell::SubcolonyCell;
pub use factory::SubcolonyCellFactory;
pub use io::{SubcolonyEvent, SubcolonyIo, SubcolonyReconfig};
pub use params::{SubcolonyOverlay, SubcolonyParams};
