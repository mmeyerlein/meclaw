//! The sealed core of the display fixture (OR-T11-25): the types that make the fixture's
//! barriers a matter of the COMPILER, not of a scan.
//!
//! `display_colony` declares this file as its private child `mod sealed`, so every
//! constructor and opener marked `pub(super)` below is visible to `display_colony` and its
//! descendants (`wire`, `presenter_colony`) and to nothing else. Fields are private to THIS
//! module, so even the fixture cannot reach into a value past its one opener.
//!
//! The file is std only on purpose, but for the one barrier: the pin
//! `809_a_restart…::the_sealed_core_refuses_every_way_around_it` compiles it on its own with
//! `rustc --cfg sealed_probe` (a std stand-in of [`barrier`]), next to a probe of every way
//! around it, in every run of the 809 binary.
//!
//! **Threat model (OR-T12-1).** The guarantees of the readers (1 to 5) hold against an
//! ACCIDENTAL read of the colony past its barrier. A deliberate way around them -- a type
//! alias or a function pointer against the scans, the peer address of a socket, `unsafe`,
//! a fake implementation of a fixture trait -- lies outside it.
#![allow(dead_code)]
#![allow(unexpected_cfgs)]

use std::panic::Location;

/// Why a lock may look at the colony without the barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A fixture's own wait polls for a positive event; the value never leaves the poll.
    Watch,
    /// A positive claim about the held request's own row while the test holds the request.
    Held,
    /// Text for a failure message (a hang's last state, an assert's context); never a verdict.
    Report,
}

/// A value read without the barrier; only [`Unsettled::because`] opens it.
///
/// The field is private to this module (re-check tv-bn M7b11 A-M1 i): `u.0`,
/// `let Unsettled(t) = u` and a `match` on it do not compile anywhere else -- not in a
/// test, and not in the fixture either.
#[must_use = "an unsettled read is opened only with a named reason"]
pub struct Unsettled<T>(T);

impl<T> Unsettled<T> {
    /// Wrap a value read without the barrier; only the fixture builds one.
    pub(super) fn new(value: T) -> Self {
        Self(value)
    }

    /// Open the value, naming why a read without the barrier is good enough here.
    pub fn because(self, why: Why) -> T {
        let _ = why;
        self.0
    }
}

/// The proof that the colony settled: only [`barrier::settle`] builds one, once the colony
/// answered `ColonyMsg::Settle`, and the address of the colony is handed to a fixture reader
/// only against it ([`Door::settled`]). `Debug` only: a test that holds one (from
/// `Colony::settle`) can print it, not use it.
#[derive(Debug)]
pub struct Settled(());

impl Settled {
    /// Private to this module (re-check tv-bn M7b12 A-M1): the fixture cannot mint one;
    /// only the child module [`barrier`] calls it.
    fn new() -> Self {
        Self(())
    }
}

/// The barrier, and the one mint of [`Settled`]: it asks the colony to settle and builds
/// the proof only from the colony's answer.
#[cfg(not(sealed_probe))]
pub(super) mod barrier {
    use super::Settled;

    /// Send `ColonyMsg::Settle` and await the colony's own answer; `None` when the colony
    /// ended before answering. The caller holds the hang mark around it.
    pub(in super::super) async fn settle(h: &meclaw_testing::ColonyHandle) -> Option<Settled> {
        let (ack, rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(meclaw_colony::ColonyMsg::Settle { ack })
            .await
            .ok()?;
        rx.await.ok()?;
        Some(Settled::new())
    }
}

/// The probe's std stand-in of the barrier: the mint stays in this child module.
#[cfg(sealed_probe)]
pub(super) mod barrier {
    use super::Settled;

    /// The stand-in mint.
    pub(in super::super) fn settle() -> Settled {
        Settled::new()
    }
}

/// The key to the fixture's private boot (`boot_with`): a boot that hands out the colony's
/// root takes it, so even a `pub` boot of that kind could not be called from a test.
pub struct Key(());

impl Key {
    /// Only the fixture cuts one.
    pub(super) fn new() -> Self {
        Self(())
    }
}

/// A raw door the fixture holds but a test must not (the held decider of the presenter):
/// opened only by the fixture, whatever the field's visibility.
pub struct Sealed<T>(T);

impl<T> Sealed<T> {
    /// Seal a door.
    pub(super) fn new(door: T) -> Self {
        Self(door)
    }

    /// The door, for the fixture.
    pub(super) fn open(&self) -> &T {
        &self.0
    }

    /// The door, for the fixture, to use.
    pub(super) fn open_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// The seal of [`Named`] (re-check tv-bn M7b12 B-M2): the trait lives in a module private
/// to this one, so only the fixture (through [`NamedSeal`]) can name it, and a test cannot
/// implement [`Named`] for a site of its own.
mod seal {
    /// Implemented by the fixture's sites only.
    pub trait Sealed {}
}

/// The seal, for the fixture's own sites.
pub(super) use seal::Sealed as NamedSeal;

/// A named exception to the barrier: one call site of a user, the file and the line it
/// stands at, and why what it gets is no read of the colony's state.
pub trait Named: seal::Sealed + Copy + std::fmt::Debug {
    /// The file (its name under `tests/`, with its harness directory if it has one) the
    /// one call site stands in.
    fn file(self) -> &'static str;

    /// The line of the one call site in [`Named::file`] (re-check tv-bn M7b12 A-M2): the
    /// binding holds in every process, not only in the one that saw the first call.
    fn line(self) -> u32;

    /// Why the address is handed out there: one sentence.
    fn reason(self) -> &'static str;

    /// Whether a call from `caller_file` at `caller_line` is this site's: the file ends on
    /// [`Named::file`], the line is [`Named::line`], and the reason is not empty.
    fn admits(self, caller_file: &str, caller_line: u32) -> Result<(), String> {
        if self.reason().trim().is_empty() {
            return Err(format!("{self:?} names no reason"));
        }
        let file = self.file();
        let ours = caller_file == file
            || caller_file.ends_with(&format!("/{file}"))
            || caller_file.ends_with(&format!("\\{file}"));
        if !ours {
            return Err(format!(
                "{self:?} is the site of {file}, not of {caller_file}"
            ));
        }
        if caller_line != self.line() {
            return Err(format!(
                "{self:?} stands at {file}:{}, not at line {caller_line}: one site, one place",
                self.line()
            ));
        }
        Ok(())
    }
}

/// The colony's two ports, as the one door hands them out.
#[derive(Debug, Clone, Copy)]
pub struct Ports {
    /// The one listener in front of the whole colony (the mount).
    pub port: u16,
    /// The API listener a browser proof needs.
    pub api_port: u16,
}

/// The one door to the colony's address (re-check tv-bn M7b11 A-M3/A-M4): the ports are
/// private here; the fixture gets them against a [`Settled`] proof, a user only through a
/// named site ([`Door::hand`]).
pub struct Door {
    port: u16,
    api_port: u16,
}

impl Door {
    /// The door of a booted colony.
    pub(super) fn new(port: u16, api_port: u16) -> Self {
        Self { port, api_port }
    }

    /// The ports for a fixture reader, against the proof that the colony settled.
    pub(super) fn settled(&self, proof: Settled) -> Ports {
        let Settled(()) = proof;
        self.ports()
    }

    /// The ports for one named call site of a user. The site must be the caller's
    /// (`#[track_caller]` chain from the user): its file and its line, so a second place
    /// with the same site panics in every process.
    #[track_caller]
    pub(super) fn hand<S: Named>(&self, site: S) -> Ports {
        let at = Location::caller();
        if let Err(why) = site.admits(at.file(), at.line()) {
            panic!("the colony's address is handed out at {at}: {why}");
        }
        self.ports()
    }

    fn ports(&self) -> Ports {
        Ports {
            port: self.port,
            api_port: self.api_port,
        }
    }
}
