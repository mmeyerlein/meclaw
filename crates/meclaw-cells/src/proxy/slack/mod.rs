//! P12: the Slack platform variant of the `proxy` cell type.
//!
//! Slack is instance TWO of the `proxy` cell type, not a new cell type. The
//! seam is `params.platform` (see `crate::proxy::platform`), dispatched in the
//! factory; everything Slack-specific lives under this module.
//!
//! Transport is Socket Mode: an outbound WebSocket opened with an app-level
//! token. It is the structural analog of the Telegram variant's long poll — no
//! inbound HTTP surface, no public URL, same double-task shape.

pub mod cell;
pub mod client;
pub mod db;
pub mod emit;
pub mod io;
pub mod params;
pub mod wire;
