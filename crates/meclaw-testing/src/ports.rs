//! Ports for tests that have to serve on a real socket.
//!
//! # Why this is not `bind(("127.0.0.1", 0))` and a `drop`
//!
//! Every `web`-cell test used to carry its own copy of that four-liner: bind an
//! ephemeral port, read the number, drop the listener, hand the number to a
//! cell that binds it again. Between the drop and the second bind the port
//! belongs to nobody, and the kernel is free to give it to somebody else — that
//! is a time-of-check/time-of-use race, and it was measured as a flake
//! (`web_cell_assets::a_path_that_is_neither_page_nor_asset_stays_the_same_404`,
//! 2026-08-28). The cell then fails to bind, keeps running listener-less by
//! design (GH #410), and the test waits out its full deadline for an answer
//! that cannot come.
//!
//! The thief is not hypothetical, and it is usually not even another test's
//! listener: `bind(…, 0)` draws from `ip_local_port_range` (32768–60999 on
//! Linux by default), which is the same pool every **outgoing** connection
//! draws from — and these tests make a lot of outgoing HTTP requests.
//!
//! # What this does instead
//!
//! It hands out ports from a band **below** the ephemeral range, so no
//! `bind(…, 0)` and no outgoing socket anywhere on the machine can land on one
//! of them. Inside the band three things narrow the field to almost nothing:
//!
//! * a **bind probe** — a port something else already listens on is skipped, so
//!   a foreign service inside the band cannot make a test red;
//! * a **per-process set** of what was already handed out, so two calls in one
//!   test binary never return the same number even before either is bound;
//! * a **per-process starting offset** derived from the pid, so two test
//!   processes scanning at the same moment start ten thousand ports apart
//!   rather than at the same place.
//!
//! What is left is a window of microseconds in which two *test* processes probe
//! the identical port out of ten thousand. That is not zero — only a socket
//! handed over while still bound would be — but it is several orders of
//! magnitude below the range this replaces.

use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};

/// First port of the band, chosen below `ip_local_port_range`'s default start
/// (32768) and above the ports the shipped examples and local installs use.
const BAND_START: u16 = 20_000;

/// How many ports the band holds. Wide enough that a per-process offset keeps
/// concurrent test binaries far apart.
const BAND_SPAN: u32 = 10_000;

/// The ports this process has already handed out. A test that asks twice must
/// get two answers, even before the first one is bound by anything.
fn handed_out() -> &'static Mutex<HashSet<u16>> {
    static HANDED_OUT: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    HANDED_OUT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Where this process starts scanning the band.
///
/// Knuth's multiplicative constant over the pid: neighbouring pids — which is
/// what a test runner produces — land far apart, so two processes scanning
/// concurrently do not walk the same ports in the same order.
fn scan_start() -> u32 {
    std::process::id().wrapping_mul(2_654_435_761) % BAND_SPAN
}

/// A TCP port on `127.0.0.1` that nothing is listening on and that no other
/// caller in this process has been given.
///
/// The returned port is **not** held open — it cannot be, since the point is to
/// hand it to a cell that binds it itself. See the module docs for what makes
/// that safe enough and what remains.
///
/// # Panics
///
/// If every port in the band is occupied, which means the machine is not one a
/// test can serve on.
pub fn free_port() -> u16 {
    // `into_inner` on a poisoned lock rather than a panic: a test that panicked
    // while holding this set left the SET intact -- it is a plain `HashSet` and
    // every mutation of it is one insert -- so the only thing poisoning would
    // add here is a second, less informative failure in every later test.
    let mut given = match handed_out().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let start = scan_start();
    for step in 0..BAND_SPAN {
        let port = BAND_START + ((start + step) % BAND_SPAN) as u16;
        if given.contains(&port) {
            continue;
        }
        // The probe is the whole check: a port that binds here is one no
        // service holds, and the band puts it out of reach of the ephemeral
        // allocator that would otherwise take it from under us.
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                given.insert(port);
                return port;
            }
            Err(_) => continue,
        }
    }
    panic!(
        "no free port in {BAND_START}..{}: every port in the test band is \
         occupied",
        BAND_START as u32 + BAND_SPAN - 1
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two asks, two answers — the property `assert_ne!(port_a, port_b)` in the
    /// `web`-cell tests relies on.
    #[test]
    fn two_asks_never_return_the_same_port() {
        let a = free_port();
        let b = free_port();
        assert_ne!(a, b, "the same port was handed out twice");
    }

    /// Out of the ephemeral range, which is the whole reason this exists.
    #[test]
    fn a_handed_out_port_is_below_the_ephemeral_range() {
        let p = free_port();
        assert!(
            (BAND_START..BAND_START + BAND_SPAN as u16).contains(&p),
            "port {p} is outside the reservation band"
        );
        assert!(
            p < 32_768,
            "port {p} is inside the ephemeral range an outgoing socket draws from"
        );
    }

    /// And it is actually bindable at the moment it is handed over.
    #[test]
    fn a_handed_out_port_binds() {
        let p = free_port();
        let l = TcpListener::bind(("127.0.0.1", p)).expect("the handed-out port must bind");
        drop(l);
    }
}
