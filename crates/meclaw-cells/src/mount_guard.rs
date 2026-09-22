//! The one object that gives a mount back.
//!
//! Every cell that holds a name on the colony's one listener (`web`, `voice`,
//! `browser`, and the `meclaw` platform of `proxy`) holds it through this guard.
//! It stood three times byte-identical in the tree before the peer mount would
//! have written it a fourth time; one definition is the only way four copies do
//! not drift apart at the one place a mount is released.

use meclaw_colony::{Registration, SurfaceRegistry};
use std::sync::Arc;

/// Holds a mount for exactly as long as the I/O half that registered it.
///
/// It is the ONE place the name is given back. The ordinary end drops it by
/// hand at the bottom of the owning cell's `run_io`, so the order against the
/// serving task is a decision rather than a scope; every OTHER end (a panic in
/// the handler half, the `message_timeout` backstop, an abort) drops it too,
/// and that is what the guard is for. An entry left standing would keep taking
/// the listener's connections for a cell nobody serves: a request going into a
/// socket that never answers, which is worse than a `404` from a name nothing
/// holds.
pub(crate) struct MountGuard {
    /// The table the mount stands in.
    surfaces: Arc<SurfaceRegistry>,
    /// The name this life registered.
    mount: String,
    /// The token this life registered under. A spent one removes nothing, which
    /// is what makes a respawn's entry safe from the previous life's guard.
    registration: Registration,
}

impl MountGuard {
    /// Takes over `registration` of `mount` on `surfaces`.
    pub(crate) fn new(
        surfaces: &Arc<SurfaceRegistry>,
        mount: &str,
        registration: Registration,
    ) -> Self {
        Self {
            surfaces: Arc::clone(surfaces),
            mount: mount.to_string(),
            registration,
        }
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        // A `Drop` cannot await, and the registry is behind an `Arc`, so the
        // removal is a task of its own. Only on a runtime thread: a guard
        // dropped outside one has no executor to spawn onto, and a process
        // without a runtime has no mount table left to keep tidy either.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let surfaces = Arc::clone(&self.surfaces);
        let mount = std::mem::take(&mut self.mount);
        let registration = self.registration;
        handle.spawn(async move {
            surfaces.unregister(&mount, &registration).await;
        });
    }
}
