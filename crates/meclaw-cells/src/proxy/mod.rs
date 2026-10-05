//! The `proxy` cell: a long-running double task on the 10-A substrate.
//! Telegram, Slack, a peer colony and an inbound webhook behind one cell
//! type; the seam is `params.platform` (see `platform`). See `docs/cell-types.md` § `proxy`.

pub mod cell;
pub mod consumed;
pub mod db;
pub mod emit;
pub mod factory;
pub mod io;
pub mod meclaw;
pub mod params;
pub mod platform;
pub mod slack;
pub mod telegram;
pub mod typing;
pub mod webhook;

pub use factory::ProxyCellFactory;
