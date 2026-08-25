//! W8 (GH #380): the `web` cell — a port-owning display substrate.
//!
//! A long-running cell whose I/O half is an HTTP + WebSocket server bound to a
//! port from its own `params`. The type is deliberately **multiple**: several
//! instances per colony, each with its own port and its own `cell.db`
//! (R-W8-1). Authentication and TLS are external, forever — a reverse proxy in
//! front — so the default bind is loopback (R-W8-2).
//!
//! # Why a new cell type does not break the "no ingress cell types" doctrine
//!
//! The substrate rejected ingress cell types once, and the argument stands: a
//! cell must not implicitly know it hangs on an endpoint. This follows the
//! sanctioned exception the `proxy` cell already is — a long-running cell that
//! owns an external platform connection and mints ingress context at a declared
//! entry edge. The platform here is HTTP-inbound instead of a chat API. Cells
//! still know no topology; a `web` cell knows its own port and its own DB.

pub mod assets;
pub mod cell;
pub mod db;
pub mod factory;
pub mod io;
pub mod ops;
pub mod output;
pub mod params;
pub mod render;
pub mod seed;
pub mod socket;

pub use assets::{Asset, AssetMap};
pub use cell::{WebCell, WebEvent, WebReconfig};
pub use factory::WebCellFactory;
pub use io::WebIo;
pub use params::WebParams;
