//! W8 (GH #380): the `web` cell — a display substrate with a name of its own.
//!
//! A long-running cell whose I/O half serves HTTP and WebSocket under the
//! mount its `params` declare: the colony's one listener peeks the first path
//! segment of every connection it accepts and hands the stream, unread, to
//! whoever registered that name (ADR-0031). The type is deliberately
//! **multiple**: several instances per colony, each with its own mount and its
//! own `cell.db` (R-W8-1). Authentication and TLS are external, forever — a
//! reverse proxy in front of that one listener (R-W8-2) — and a proxy that
//! serves a display under a path of its own says so with `X-Forwarded-Prefix`,
//! which the shell reads and writes every link from.
//!
//! Since `web@2.0.0` there is no `port` and no `bind`: a cell type gets a port
//! only when there is no other way, and a display has another way. A params
//! document that still carries either key is refused at parse with the
//! migration named.
//!
//! # Why a new cell type does not break the "no ingress cell types" doctrine
//!
//! The substrate rejected ingress cell types once, and the argument stands: a
//! cell must not implicitly know it hangs on an endpoint. This follows the
//! sanctioned exception the `proxy` cell already is — a long-running cell that
//! owns an external platform connection and mints ingress context at a declared
//! entry edge. The platform here is HTTP-inbound instead of a chat API. Cells
//! still know no topology; a `web` cell knows its own mount and its own DB.

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
