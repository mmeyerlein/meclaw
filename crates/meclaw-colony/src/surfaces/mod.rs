//! The mount table of a process: which name reaches which surface cell.
//!
//! A surface cell is reached under a **declared mount name**, never under its
//! tree path. The name lives here, written by the cell's I/O half on every life
//! and read by whoever has a client to hand over: the CLI's one listener for an
//! accepted TCP stream, a `web` cell's socket loop for a topic. Two handoff
//! kinds sit on one entry — a [`LinkOpener`] for frames that never touch a
//! socket, and an mpsc channel of [`HandedConnection`] for a stream accepted
//! elsewhere.
//!
//! Why a lock here is not the forbidden shape: the registry is not the state of
//! any actor. It is a table of live senders written by whichever I/O half starts
//! or ends and read to address one of them — the class `ViewerRegistry` and the
//! voice `sessions` table already are, with the same rule: **no `.await` is held
//! across a critical section**. The alternative, a task whose only job is to own
//! a `HashMap` and answer over a channel, is the same lock with more moving
//! parts, and it would make a TCP accept and a topic join wait on the actor that
//! routes every message. ADR-0031 records the decision.

pub mod listener;

use meclaw_core::Path;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

/// A future a [`LinkOpener`] returns.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The first path segments the API owns; a mount may not shadow them.
pub const RESERVED_MOUNTS: &[&str] = &["colony", "messages", "health", "ui", "live", "@client"];

/// The longest mount name.
pub const MOUNT_MAX: usize = 64;

/// `[a-z0-9-]{1,64}` and not reserved.
///
/// The grammar is deliberately narrower than a path segment: a mount becomes the
/// first segment of a URL on a shared listener, so anything that needs escaping,
/// or that an API route already answers, is refused before a cell spawns.
pub fn mount_is_valid(name: &str) -> bool {
    if name.is_empty() || name.len() > MOUNT_MAX {
        return false;
    }
    if RESERVED_MOUNTS.contains(&name) {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One frame of a topic link: the shape of a WebSocket minus the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkFrame {
    /// A text frame, the same JSON the socket door carries.
    Text(String),
    /// A binary frame, the same audio the socket door carries.
    Binary(Vec<u8>),
    /// The end of the link, with the code and reason the socket door would send.
    Close {
        /// The WebSocket close code.
        code: u16,
        /// The close reason, as text.
        reason: String,
    },
}

/// What a client asks for when it opens a link — the voice door's query string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkRequest {
    /// The session (a call) this link belongs to.
    pub session: Option<String>,
    /// The conversation mode the client asks for.
    pub mode: Option<String>,
    /// The sample rate of the audio the client will send.
    pub sample_rate: Option<u32>,
    /// The encoding of the audio the client will send.
    pub encoding: Option<String>,
}

/// A refusal, with the HTTP status the socket door would have answered and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRefused {
    /// The status the HTTP door answers the same refusal with.
    pub status: u16,
    /// The text the HTTP door answers the same refusal with.
    pub detail: String,
}

/// One open link. `to_cell` carries the client's frames in, `from_cell` the cell's frames out.
pub struct Link {
    /// The client writes its frames here.
    pub to_cell: mpsc::Sender<LinkFrame>,
    /// The client reads the cell's frames here.
    pub from_cell: mpsc::Receiver<LinkFrame>,
}

/// Capacity of each direction of a link: ~1.3 s of 20 ms audio.
pub const LINK_QUEUE: usize = 64;

/// Whoever answers a topic for a mount.
pub trait LinkOpener: Send + Sync {
    /// Open one link, or refuse it with the text the HTTP door would give.
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>>;
}

/// A TCP connection accepted elsewhere, unread, for the cell to serve.
pub struct HandedConnection {
    /// The accepted stream, with not a byte read from it.
    pub stream: tokio::net::TcpStream,
    /// Who connected.
    pub peer: SocketAddr,
}

/// Capacity of the handoff channel per mount.
pub const HANDOFF_QUEUE: usize = 16;

/// What a surface cell registers.
pub struct SurfaceEntry {
    /// The cell type behind the mount, as the mount table publishes it.
    pub kind: &'static str,
    /// Who registered. Never published on a client-facing surface (O-639-5).
    pub cell_path: Path,
    /// Answers a topic, when this surface serves frames without a socket.
    pub links: Option<Arc<dyn LinkOpener>>,
}

/// Why a registration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountRefused {
    /// The name breaks the mount grammar or names an API segment.
    #[error("mount {0:?}: must be 1..=64 characters from [a-z0-9-] and none of the reserved names")]
    Invalid(String),
    /// Another cell holds the name.
    #[error("mount {mount:?} is held by {holder}")]
    Taken {
        /// The name that is taken.
        mount: String,
        /// The path of the cell that holds it.
        holder: String,
    },
}

/// One row of the mount table, as the API publishes it. No cell path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MountRow {
    /// The name a client reaches the surface under.
    pub mount: String,
    /// The cell type behind it.
    pub kind: &'static str,
}

/// What one registration is, so a life can only remove its own.
///
/// A cell path is not enough. A respawn registers at the top of the new life
/// while the previous life may still be draining, and the old life's
/// `unregister` at the end of its round would then delete the **new** life's
/// entry — the mount gone while the new I/O half believes it holds it. The token
/// is minted per registration, so a call carrying a spent one is a no-op.
///
/// Opaque and cheap: a number the registry hands out, never parsed, never
/// published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration(u64);

/// One mount: what the cell declared, the handoff channel the registry minted,
/// and which registration minted it.
struct Slot {
    entry: SurfaceEntry,
    handoff: mpsc::Sender<HandedConnection>,
    registration: Registration,
}

/// The mounts and the counter that names their registrations.
///
/// The counter lives under the same lock as the map rather than in an atomic:
/// it is only ever read and written inside a critical section that is already
/// held, so an atomic would add a second synchronisation for nothing.
#[derive(Default)]
struct Table {
    mounts: HashMap<String, Slot>,
    next_registration: u64,
}

/// The mount table of one process, plus the address of the one listener.
pub struct SurfaceRegistry {
    inner: Mutex<Table>,
    listener: watch::Sender<Option<SocketAddr>>,
}

impl Default for SurfaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceRegistry {
    /// An empty table with no listener bound yet.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Table::default()),
            listener: watch::Sender::new(None),
        }
    }

    /// Register `mount`; the registry mints the handoff channel and hands the receiver to the caller,
    /// with the [`Registration`] that names this life.
    /// The same `cell_path` replaces its own entry (a respawn); another path is `Taken`.
    pub async fn register(
        &self,
        mount: &str,
        entry: SurfaceEntry,
    ) -> Result<(mpsc::Receiver<HandedConnection>, Registration), MountRefused> {
        if !mount_is_valid(mount) {
            return Err(MountRefused::Invalid(mount.to_string()));
        }
        let mut table = self.inner.lock().await;
        if let Some(held) = table.mounts.get(mount)
            && held.entry.cell_path != entry.cell_path
        {
            return Err(MountRefused::Taken {
                mount: mount.to_string(),
                holder: held.entry.cell_path.as_str().to_string(),
            });
        }
        table.next_registration += 1;
        let registration = Registration(table.next_registration);
        let (handoff, rx) = mpsc::channel(HANDOFF_QUEUE);
        table.mounts.insert(
            mount.to_string(),
            Slot {
                entry,
                handoff,
                registration,
            },
        );
        Ok((rx, registration))
    }

    /// Remove `mount` if `registration` is the one that currently holds it.
    ///
    /// A spent token is a no-op that returns `false`: the life it named has
    /// already been replaced, and the entry standing there belongs to whoever
    /// replaced it.
    pub async fn unregister(&self, mount: &str, registration: &Registration) -> bool {
        let mut table = self.inner.lock().await;
        let holds = table
            .mounts
            .get(mount)
            .is_some_and(|held| &held.registration == registration);
        if holds {
            table.mounts.remove(mount);
        }
        holds
    }

    /// Open a link on `mount`. `None` when nothing is mounted there or the entry has no opener.
    pub async fn open_link(
        &self,
        mount: &str,
        req: LinkRequest,
    ) -> Option<Result<Link, LinkRefused>> {
        // The `Arc` is cloned under the lock, `open` is called after it is
        // released — a link that waits on a provider must not hold the table.
        let opener = {
            let table = self.inner.lock().await;
            table
                .mounts
                .get(mount)
                .and_then(|held| held.entry.links.clone())?
        };
        Some(opener.open(req).await)
    }

    /// The handoff sender of `mount`, if mounted.
    pub async fn take_handoff(&self, mount: &str) -> Option<mpsc::Sender<HandedConnection>> {
        let table = self.inner.lock().await;
        table.mounts.get(mount).map(|held| held.handoff.clone())
    }

    /// Every mount, sorted by name.
    pub async fn table(&self) -> Vec<MountRow> {
        let table = self.inner.lock().await;
        let mut rows: Vec<MountRow> = table
            .mounts
            .iter()
            .map(|(mount, held)| MountRow {
                mount: mount.clone(),
                kind: held.entry.kind,
            })
            .collect();
        rows.sort_by(|a, b| a.mount.cmp(&b.mount));
        rows
    }

    /// Where the one listener is bound (set by the CLI after the bind).
    pub fn set_listener(&self, addr: SocketAddr) {
        self.listener.send_replace(Some(addr));
    }

    /// The address of the one listener, once it is bound.
    pub fn listener(&self) -> Option<SocketAddr> {
        *self.listener.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOpener;
    impl LinkOpener for NoOpener {
        fn open(&self, _req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
            Box::pin(async {
                Err(LinkRefused {
                    status: 503,
                    detail: "nothing answers".into(),
                })
            })
        }
    }

    fn entry(path: &str, links: bool) -> SurfaceEntry {
        SurfaceEntry {
            kind: "voice",
            cell_path: Path::new(path),
            links: links.then(|| Arc::new(NoOpener) as Arc<dyn LinkOpener>),
        }
    }

    #[test]
    fn a_mount_is_a_short_lowercase_name_and_never_an_api_segment() {
        for ok in ["voice", "v1", "a-b", &"x".repeat(64)] {
            assert!(mount_is_valid(ok), "{ok:?} is a mount");
        }
        for bad in [
            "",
            "Voice",
            "a/b",
            "a b",
            "a.b",
            &"x".repeat(65),
            "colony",
            "messages",
            "health",
            "ui",
            "live",
            "@client",
        ] {
            assert!(!mount_is_valid(bad), "{bad:?} must be refused");
        }
    }

    #[tokio::test]
    async fn a_second_path_is_refused_and_the_same_path_replaces_its_entry() {
        let reg = SurfaceRegistry::new();
        let (_rx, first) = reg
            .register("voice", entry("/a/voice", false))
            .await
            .expect("free");
        let err = reg
            .register("voice", entry("/b/voice", false))
            .await
            .expect_err("taken");
        assert_eq!(
            err,
            MountRefused::Taken {
                mount: "voice".into(),
                holder: "/a/voice".into()
            }
        );
        // A respawn of the holder registers again and wins.
        let (_rx2, second) = reg
            .register("voice", entry("/a/voice", true))
            .await
            .expect("the holder may replace");
        assert!(
            reg.open_link("voice", LinkRequest::default())
                .await
                .is_some(),
            "the new entry stands"
        );
        assert_ne!(
            first, second,
            "each registration is a life of its own, and says so"
        );
        // The draining old life hands back a spent token: it must not take the
        // entry the new life is holding.
        assert!(
            !reg.unregister("voice", &first).await,
            "a spent registration removes nothing"
        );
        assert_eq!(
            reg.table().await.len(),
            1,
            "the respawned life still holds its mount"
        );
        assert!(
            reg.unregister("voice", &second).await,
            "the current registration removes its own entry"
        );
        assert!(matches!(
            reg.register("colony", entry("/a/x", false)).await,
            Err(MountRefused::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn only_the_holder_unregisters_and_the_table_is_sorted() {
        let reg = SurfaceRegistry::new();
        // Four mounts, inserted in reverse order, so the sort decides the order
        // rather than the hasher happening to agree with it.
        let (_d, delta) = reg
            .register("delta", entry("/d", false))
            .await
            .expect("free");
        let (_b, _beta) = reg
            .register("beta", entry("/b", false))
            .await
            .expect("free");
        let (_c, _gamma) = reg
            .register("gamma", entry("/c", false))
            .await
            .expect("free");
        let (_a, alpha) = reg
            .register("alpha", entry("/a", false))
            .await
            .expect("free");
        // A stranger's token — here another mount's registration — removes nothing.
        assert!(!reg.unregister("alpha", &delta).await);
        let names: Vec<String> = reg.table().await.into_iter().map(|r| r.mount).collect();
        assert_eq!(names, vec!["alpha", "beta", "delta", "gamma"]);
        assert!(reg.unregister("alpha", &alpha).await);
        assert_eq!(reg.table().await.len(), 3);
        assert!(reg.take_handoff("alpha").await.is_none());
        assert!(reg.take_handoff("beta").await.is_some());
    }

    #[tokio::test]
    async fn a_handed_connection_reaches_the_holder_and_a_dropped_receiver_closes_the_sender() {
        let reg = SurfaceRegistry::new();
        let (mut rx, _held) = reg
            .register("voice", entry("/v", false))
            .await
            .expect("free");
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        let _client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let (stream, peer) = l.accept().await.expect("accept");
        reg.take_handoff("voice")
            .await
            .expect("mounted")
            .send(HandedConnection { stream, peer })
            .await
            .expect("handed");
        let got = rx.recv().await.expect("the holder receives it");
        assert_eq!(got.peer, peer);
        drop(rx);
        assert!(
            reg.take_handoff("voice")
                .await
                .expect("still mounted")
                .is_closed()
        );
    }

    #[test]
    fn the_listener_address_is_published_once_set() {
        let reg = SurfaceRegistry::new();
        assert!(reg.listener().is_none());
        let addr: SocketAddr = "127.0.0.1:7777".parse().expect("addr");
        reg.set_listener(addr);
        assert_eq!(reg.listener(), Some(addr));
    }
}
