//! The `vault` cell type — a secret store with no operation that returns a
//! secret.
//!
//! This is not a policy layer over a store. The route surface below simply has
//! no read: there is `put`, `use`, `rotate`, `revoke`, `status`, `unlock` and
//! `lock`, and there is no `get`. A fully compromised model on the other end of
//! an edge can ask the vault to *use* a credential inside a granted scope; it
//! cannot ask to see one, because the question has no name here.
//!
//! What that buys, honestly stated: the boundary is a type contract, not a
//! guard that can be argued with. What it does not buy: protection against a
//! process that can read the vault's memory while it is unlocked. The designed
//! answer to that is placement — running the vault cell in its own process or
//! under its own user — which is a deployment property and changes no edge.

pub mod attest;
pub mod cell;
pub mod crypto;
pub mod degraded;
pub mod factory;
pub mod keysource;
pub mod params;
pub mod store;
pub mod user_channel;

pub use cell::VaultCell;
pub use factory::VaultCellFactory;
pub use params::VaultParams;
