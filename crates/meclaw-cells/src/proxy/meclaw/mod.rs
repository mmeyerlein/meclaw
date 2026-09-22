//! The `meclaw` platform variant of the `proxy` cell type: one colony speaking
//! to another.
//!
//! This is platform THREE of the `proxy` cell type, not a new cell type. The
//! seam is `params.platform` (see `crate::proxy::platform`), exactly as for
//! Slack; everything peer-specific lives under this module.
//!
//! Three rules hold the boundary:
//! - each side judges its own edge and never reads the other side's declaration;
//! - a body field the lane does not name is refused, never stripped, while an
//!   unnamed `context` key falls away silently;
//! - every crossing and every refusal leaves a receipt on both sides.

pub mod cell;
pub mod client;
pub mod emit;
pub mod io;
pub mod lanes;
pub mod mount;
pub mod params;
pub mod wire;
