//! The `vault` cell type — a secret store with no operation that returns a
//! secret IN THE CLEAR.
//!
//! This is not a policy layer over a store. The route surface below has no
//! read: there is `put`, `use`, `rotate`, `revoke`, `status`, `unlock`, `lock`
//! and `deliver`, and there is no `get`. A fully compromised model on the other
//! end of an edge can ask the vault to *use* a credential inside a granted
//! scope, or to *deliver* one sealed to a key it holds for a single request; it
//! cannot ask to see one, because the question has no name here.
//!
//! `deliver` (GH #421, sanctioned ruling R3) is the sanctioned extension that made
//! the `.env` shrink: the answer carries the credential under XChaCha20-Poly1305
//! with a key derived from an X25519 agreement against an ephemeral public key
//! the requester minted for this one call. The message log therefore journals a
//! ciphertext, and the plaintext exists only in the requesting task's RAM.
//!
//! What that buys, honestly stated: the boundary is a type contract, not a
//! guard that can be argued with, and the box is opaque to everyone who was not
//! addressed. What it does not buy: protection against a process that can read
//! the vault's memory while it is unlocked, and no proof of WHO sealed — that is
//! the topology plus the policy, and a signature is later work. The designed
//! answer to the first is placement — running the vault cell in its own process
//! or under its own user — which is a deployment property and changes no edge.

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
