//! Wave G (GH #766) — the `browser` cell type: one browser per member.
//!
//! The picture of a web page is not a message and never becomes one. This cell
//! holds a Chromium-based browser out of the distribution's packages, a browser
//! context per identity, a page per card, and it puts each page's picture on a
//! topic of the display's socket (`page:<page>`, GH #643's second kind). What
//! comes back on the same link are pointers, wheels, keys and text. The
//! topology hears about pages — that one opened, navigated, went quiet, came
//! back — and never about a frame.
//!
//! **The browser is a prerequisite, not a delivery** (R-G11). meclaw ships no
//! binary, no cache path, no AppArmor profile and no sandbox knob; it adds no
//! sandbox flag of its own, refuses `--no-sandbox`, and a browser whose sandbox
//! does not hold does not start. What `params.sandbox` does here is the cell's
//! ceiling — a cgroup cap and nothing else (R-G13, ADR-0043).
//!
//! Its shape is the one `voice` has: long-running, dual task, mounted by name,
//! with a `cell.db` of its own. Its child process is the one `harness` has:
//! `ChildSpec`, its own process group, `kill_on_drop`, the orphan journal.

/// CDP over the child's pipe.
pub mod cdp;
/// The handler half: cell state, `cell.db`, emissions.
pub mod cell;
/// `cell.db.pages` — what this browser was showing.
pub mod db;
/// The factory.
pub mod factory;
/// What a finger on a screen becomes inside a page.
pub mod input;
/// The I/O half: the browser, its pages, its viewers.
pub mod io;
/// The page register.
pub mod pages;
/// The `params` surface.
pub mod params;
/// Reading one of the four verbs out of a message.
pub mod parse;
/// The door: a `page:` topic on a display's socket.
pub mod service;

/// The closed failure list.
pub mod error;

pub use cdp::{
    CdpEvent, CdpIn, CdpPipe, FD_SHIM, SandboxVerdict, child_spec, flags, renderers_are_sandboxed,
    spawn_browser,
};
pub use cell::{BrowserCell, BrowserCommand, BrowserEvent};
pub use db::{PageRow, delete_page, open_pages, setup_browser_schema, upsert_page};
pub use error::{BrowserError, ERROR_CODES};
pub use factory::BrowserCellFactory;
pub use input::{Input, reshapes_the_page};
pub use io::{Admitted, Browser, BrowserIo, INTERACTION_QUIET_MS, LinkCommand, ProfileDir};
pub use pages::{PageEntry, PageReport, PageState, Register, Viewer, Viewport};
pub use params::{BrowserParams, ScreencastParams, cell_id_of, profile_dir_for};
pub use parse::{ParsedVerb, Verb, parse_verb};
pub use service::{BrowserLinkOpener, head, viewport_of_join};
