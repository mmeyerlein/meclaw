//! The `webhook` platform of the `proxy` cell type: a mount on the colony's one
//! listener that takes a verified POST and emits it once on a declared lane.
//!
//! Platform FOUR of the `proxy` cell type, not a new cell type: an inbound
//! surface is the exception `proxy` and `web` already are
//! (`docs/cell-types.md` § `web`), and the seam is `params.platform`
//! (`crate::proxy::platform`). Three rules hold the boundary:
//! - verification happens over the raw body before anything else takes effect;
//! - the sender is answered `202` only after the verdict, and never with content;
//! - the secret appears in no log line, receipt, refusal or `Debug` output.

pub mod cell;
pub mod emit;
pub mod mount;
pub mod params;
pub mod verify;
