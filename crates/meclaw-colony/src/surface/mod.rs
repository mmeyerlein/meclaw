//! What a colony hands to a browser, and on whose authority.
//!
//! A **surface** is a cell that says it may be served over HTTP. The cell path
//! is the address — `/surface/<cell-path>` — which is the whole design decision
//! in one line: many surfaces are told apart by exactly what tells cells apart,
//! so there is no second namespace to keep in step, and a reverse proxy in front
//! can authorise one surface on a path prefix without asking the colony
//! anything.
//!
//! # What this module is NOT allowed to do
//!
//! It does not open a database. Not `colony.db`, not the surface's own
//! `cell.db`, not read-only, not off the runtime, not "just this once" — see
//! `docs/meclaw-overview.md` § Datenbank-Isolation. Everything a surface draws,
//! the surface's own cell obtains by sending messages, and the HTTP layer only
//! carries the answer. What is left here is small on purpose: parse a
//! declaration, resolve a path, hand back a directory.
//!
//! Reading a `config.json` is not a database read. It is the same file the
//! colony reads at instantiation and the same class of read as a `code` cell's
//! `script_path`: a declaration on disk, not a cell's state.
//!
//! # What it does not know
//!
//! Anything about what a surface contains. There is no `kind`, no table name, no
//! notion of a topology or a layout anywhere in this module. A surface that
//! draws a colony graph and one that draws a Gantt chart are the same thing
//! here, and that is what keeps an object library a template change instead of a
//! release.
//!
//! # Why the declaration is opt-in and immutable
//!
//! "Reads are free from anywhere" is true **inside** a colony and must not be
//! inherited across an HTTP boundary: the tree holds a `vault`, session windows,
//! and an affinity store full of what the system knows about people. So a cell
//! is reachable here only if it says so, the default is absent, and an
//! undeclared cell answers 404 rather than 403 — a surface nobody declared
//! should not confirm it exists.
//!
//! Immutable for the reason `write_surface` is: a boundary a message can switch
//! off is not a boundary. It lives in the `cell` block, which no message
//! rewrites — the colony writes `config.json` exactly once, at instantiation.

mod decl;
mod locate;

pub use decl::{SurfaceDecl, parse_decl};
pub use locate::{LocateError, Located, locate};
