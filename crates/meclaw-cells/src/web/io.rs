//! W8 (GH #380): the I/O half of the `web` cell — the listener.
//!
//! This half owns the axum server and nothing else. It holds no cell state, no
//! `cell.db` handle and no `OutputSink`; what it learns from the outside world
//! it pushes to the handler over the events channel. That is the substrate's
//! dual-task rule, and for a display it is also what makes a page load cost
//! zero cell calls.
//!
//! # Why a page load touches nothing
//!
//! The pages are rendered by the handler half and **published** here as an
//! immutable snapshot (see [`WebIo`]). A GET looks a route up in that snapshot
//! and concatenates it into the shell: no database, no cell call, no diff work
//! — R-W8-4(a) and (b), which is the whole reason the rendering is materialised
//! rather than done per request.
//!
//! The cell's files travel the same way since GH #393: a second published
//! snapshot ([`crate::web::assets::AssetMap`]), read by the same wildcard
//! handler, so serving a stylesheet costs a map lookup and a byte copy.
//!
//! Two consequences worth stating. A colony that is wedged still serves its
//! pages, and the client then visibly fails to *connect* — a state a person can
//! read, rather than a blank screen. And the first paint is the real page, not
//! a spinner: the LiveView client attaches to markup that is already correct,
//! so a display shows something even if the socket never comes up.
//!
//! # Why this shell is its own
//!
//! There used to be a second one: `meclaw_surface::page::dead_render`, which
//! took a `Located` — the resolved `cell.surface` declaration — and wrote its
//! URLs under the `/surface/<cell-path>/` prefix the HTTP API served them on. A
//! `web` cell had neither: it declares no surface (its `pages` table is the only
//! route source, R-W8-3) and it owns its whole origin, so its bundles live at
//! `/@client/…` and its socket at `/live`. GH #383 retired that other shell with
//! the route it wrote for, so this one is now the only one — but the reason it
//! was written separately is the reason above, not the removal. What survives
//! from the shared half is where it always lived: the container id and the
//! session token come from `meclaw_surface::session`.

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use meclaw_surface::session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

use crate::mount_guard::MountGuard;
use crate::web::assets::{Asset, AssetMap, client_file, client_files, etag_of};
use crate::web::backlog::{BacklogPolicy, Outbox, PageStamp};
use crate::web::cell::{WebEvent, WebReconfig};
use crate::web::render::PageMap;
use crate::web::socket::{Viewer, ViewerMsg, run_connection};

/// Who is currently looking at which page.
///
/// # Why this one is behind a mutex
///
/// The substrate's rule is that **cell state** lives in its task and is never
/// shared. This is not cell state: it is a table of live socket senders, owned
/// by the I/O half, written by whichever connection task joins or leaves, and
/// read to fan a frame out. The handler never writes it — it publishes pages
/// through the `watch` channel and asks for a push through the reconfig
/// channel, and this half does the addressing. The `viewers` op (GH #1006)
/// does not read it either: it asks over the same channel, and the fan-out
/// answers with [`ViewerRegistry::report`].
///
/// The alternative would be a third task whose only job is to own a `HashMap`
/// and answer over a channel. That is the same lock with more moving parts, and
/// every critical section here is an insert, a remove or a clone of a sender
/// list — no `.await` is held across any of them.
#[derive(Default)]
pub struct ViewerRegistry {
    inner: Mutex<HashMap<String, Viewer>>,
    /// What this life reports about each viewer's backlog (GH #1006), read by
    /// every connection when it builds its meter. Fixed for the life, like the
    /// mount: a params update takes effect on the next one.
    policy: BacklogPolicy,
    /// Whether joins and leaves are reported as `viewer:screen` (GH #1003,
    /// `viewer_events` names `"screen"`). Fixed for the life, like `policy`.
    screen_events: bool,
}

/// One viewer, as a fan-out addresses it.
///
/// The `id` is what makes a failed send attributable: a `Sender` is not a key,
/// and the whole point of GH #414's second half is remembering **which** viewer
/// lost a frame. The `route` travels with it because the resync sends that
/// page's whole tree, and the map is keyed by route (see [`fan_out`]).
pub struct Addressed {
    /// The connection id the registry holds this viewer under.
    pub id: String,
    /// Where to send frames, with the meter that counts them (GH #1006).
    pub tx: Outbox,
    /// The client's join reference, needed to address a server-initiated push.
    pub join_ref: meclaw_core::JsonValue,
    /// The topic this viewer joined.
    pub topic: String,
    /// The route this viewer is looking at.
    pub route: String,
}

impl ViewerRegistry {
    /// An empty registry whose connections meter by `policy` (GH #1006).
    pub fn with_policy(policy: BacklogPolicy) -> Self {
        Self {
            inner: Mutex::default(),
            policy,
            screen_events: false,
        }
    }

    /// The same registry, reporting each viewer's join and leave as
    /// `viewer:screen` when `on` (GH #1003).
    pub fn with_screen_events(mut self, on: bool) -> Self {
        self.screen_events = on;
        self
    }

    /// Whether joins and leaves are reported as `viewer:screen`.
    pub fn screen_events(&self) -> bool {
        self.screen_events
    }

    /// What this life reports about each viewer's backlog.
    pub fn policy(&self) -> BacklogPolicy {
        self.policy
    }

    /// Every joined viewer's backlog, ordered by connection id — the answer
    /// of the read-only `viewers` op (GH #1006).
    ///
    /// Called by the fan-out when the handler asks (`WebReconfig::Viewers`),
    /// never by the handler itself: a snapshot of counters, taken under the
    /// same short lock a fan-out takes, with no `.await` inside it.
    pub async fn report(&self) -> Vec<meclaw_core::JsonValue> {
        let inner = self.inner.lock().await;
        let mut rows: Vec<(&String, &Viewer)> = inner.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        rows.into_iter()
            .map(|(_, v)| {
                let meter = v.tx.meter();
                let r = meter.reading();
                meclaw_core::serde_json::json!({
                    "session_id": meter.session_id(),
                    "route": v.route,
                    "frames": r.frames,
                    "bytes": r.bytes,
                    "oldest_ms": r.oldest_ms,
                    "resyncs_total": r.resyncs_total,
                    "screen": v.screen,
                })
            })
            .collect()
    }

    /// Register a viewer under its connection id; the one it replaces (a
    /// second join on the same socket), if any.
    pub async fn insert(&self, id: String, viewer: Viewer) -> Option<Viewer> {
        self.inner.lock().await.insert(id, viewer)
    }

    /// Forget a viewer whose connection ended; the viewer it was, if joined.
    pub async fn remove(&self, id: &str) -> Option<Viewer> {
        self.inner.lock().await.remove(id)
    }

    /// Every viewer currently looking at `route`, ordered by id.
    ///
    /// Returns clones so the caller can send without holding the lock. The order
    /// is stable rather than a `HashMap`'s: a fan-out that visits its viewers in
    /// a different order every time is untestable at the seam that matters, and
    /// nothing is bought by the arbitrary one.
    pub async fn on_route(&self, route: &str) -> Vec<Addressed> {
        let mut out: Vec<Addressed> = self
            .inner
            .lock()
            .await
            .iter()
            .filter(|(_, v)| v.route == route)
            .map(|(id, v)| Addressed {
                id: id.clone(),
                tx: v.tx.clone(),
                join_ref: v.join_ref.clone(),
                topic: v.topic.clone(),
                route: v.route.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Forget every viewer and hand back their senders (GH #410).
    ///
    /// One critical section, and the registry is empty when it ends: a viewer
    /// that joined on a listener which no longer exists must not be found by a
    /// later fan-out. The connection tasks close on their own once they read
    /// the [`ViewerMsg::Close`] the caller sends, and their own `remove` then
    /// finds nothing — which is the harmless order, unlike removing after the
    /// close and racing a re-join against it.
    pub async fn drain(&self) -> Vec<Outbox> {
        let mut inner = self.inner.lock().await;
        inner.drain().map(|(_, v)| v.tx).collect()
    }

    /// How many viewers are joined. Diagnostics and tests.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Whether nobody is looking.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// Everything the listener needs.
///
/// Cheap to clone into axum's state: the path is behind an `Arc` and the page
/// map is read through a `watch` receiver.
///
/// # Why a `watch` channel and not a lock
///
/// The rendered pages are written by the handler half and read by every
/// request task the server spawns. A `Mutex` around them would be the shape the
/// substrate forbids — shared mutable actor state — and it would put every GET
/// behind the same contended lock. A `watch` channel is message passing: the
/// handler publishes a new immutable snapshot, readers take a cheap borrow of
/// whatever is current, and nobody waits on anybody. The same reasoning the
/// api-side render cache followed when it kept its pages inside one task and
/// answered over a channel — that code is retired (GH #396), the shape is not.
#[derive(Clone)]
pub struct WebIo {
    /// The name this display is reached under on the colony's listener. Every
    /// link the shell writes starts with it, and the router this half serves a
    /// handed stream with is nested under `/<mount>`.
    pub mount: String,
    /// The request header a proxy in front puts the viewer's identity in, or
    /// empty for none. Read once per socket upgrade — see [`identity_of`].
    pub identity_header: String,
    /// GH #833: the proxies whose `identity_header` is believed. Loopback from
    /// [`WebIo::new`]; the factory sets the cell's own list after it.
    pub trusted: Arc<Vec<meclaw_colony::surfaces::ProxyNet>>,
    /// GH #833: whether THIS connection came from an address in [`Self::trusted`].
    /// Decided once per connection at the handoff (the peer address is the
    /// connection's, O-639-6) and `false` on the state no connection has
    /// reached yet, so a router built without a handoff believes nobody.
    pub peer_trusted: bool,
    /// GH #869: the mounts a `voice:`/`page:` join may reach; empty is every
    /// mount. Empty from [`WebIo::new`]; the factory sets the cell's own list
    /// after it, like [`Self::trusted`].
    pub link_mounts: Arc<Vec<String>>,
    /// The cell's own path — the identity the session token is minted for.
    pub cell_path: Arc<str>,
    /// The rendered pages, as last published by the handler half.
    pub pages: watch::Receiver<Arc<PageMap>>,
    /// The files this cell serves, as last published by the handler half.
    ///
    /// # Why a second channel and not a second field in the page snapshot
    ///
    /// The two have different cadences. The pages are re-published on every
    /// write; the assets are published once, at start, because no op writes
    /// that table (GH #393). Folding them into one snapshot would mean every
    /// `publish_and_push` had to carry the assets forward by hand, and the one
    /// call site that forgot would silently drop every file the cell serves.
    /// Two channels make that impossible rather than merely unlikely, and cost
    /// one `watch` — the reader side of which is an `Arc` clone per request
    /// either way.
    pub assets: watch::Receiver<Arc<AssetMap>>,
    /// Has the handler half published its first snapshot yet? (GH #395)
    ///
    /// # Why this is not "is the page map empty"
    ///
    /// The two halves start together: the I/O half binds the port and begins
    /// answering as soon as its task runs, while the handler half builds the
    /// page map in `on_start`. Between the two there is a window in which the
    /// display is reachable and answers `404` for a page its own seed declares
    /// — small, self-closing, and observed about one run in three on an 8-core
    /// box with the cell installed into a running colony.
    ///
    /// It matters because `404` from this cell is a **meaningful** answer: the
    /// `pages` table is the only route source (R-W8-3), so `404` means "no such
    /// route" — and for a moment after boot it also meant "not ready yet". Two
    /// different facts arriving as one status code, which nothing on the wire
    /// could tell apart; a reverse proxy in front (the deployment shape,
    /// R-W8-2) could not either, so an early health check marked a healthy
    /// display broken.
    ///
    /// An empty [`PageMap`] cannot carry that signal, because a display with
    /// **zero pages is a legitimate state** and must go on answering `404`.
    /// Hence a channel of its own, in the idiom the other two seams already
    /// use.
    pub ready: watch::Receiver<bool>,
    /// Who is joined, and to which page.
    pub viewers: Arc<ViewerRegistry>,
    /// Browser events, on their way to the handler — the only writer.
    pub events_tx: Option<mpsc::Sender<WebEvent>>,
    /// Diffs from the handler, waiting to be fanned out.
    ///
    /// Taken once by `run_io`. `WebIo` is `Clone` because axum wants its state
    /// cloneable, and a receiver is not — so it lives behind an `Option` in an
    /// `Arc<Mutex<…>>` that only `run_io` ever touches.
    pub pushes: Arc<Mutex<Option<mpsc::Receiver<WebReconfig>>>>,
    /// The process's mount table, set by the factory (ADR-0031).
    ///
    /// The socket loop reaches a surface that mounted on it through this; a
    /// display built outside a colony gets a table of its own.
    pub surfaces: Arc<meclaw_colony::SurfaceRegistry>,
    /// Closes when this cell's I/O half goes away. Filled by [`run_io`], which
    /// keeps the sending end among its own locals.
    ///
    /// # Why a channel nobody ever sends on
    ///
    /// The substrate ends a long-running cell by **aborting** `run_io` the
    /// moment its handler half returns (`cell_task_long_running`), so code
    /// after the loop is not a shutdown path — a dropped future is. Everything
    /// that has to happen on the way out therefore hangs off a `Drop`: the
    /// mount off [`MountGuard`], the accept loop and its connections off a
    /// `JoinSet`, and the sockets off this. An upgraded WebSocket is the one
    /// that cannot be aborted from here at all — axum's `on_upgrade` runs it
    /// on a task of its own — so it watches this instead and closes itself.
    ///
    /// A browser that reads a close frame reconnects to the next life. One
    /// whose socket merely stops answering draws its last picture until
    /// somebody reloads the tab, which is what a respawn must not cost a wall
    /// screen.
    pub shutdown: Option<watch::Receiver<()>>,
    /// GH #1002: the most bytes one join frame (and one resync frame)
    /// carries. The default from [`WebIo::new`]; the factory sets the
    /// effective param after it, like [`Self::trusted`].
    pub join_chunk: usize,
    /// GH #1002: the join timeout the shell states to the client, or `None`
    /// to state nothing (the client's own 10 s). Set like [`Self::join_chunk`].
    pub join_timeout_ms: Option<u64>,
    /// GH #1002: each route's page validator, computed once per publish by
    /// the I/O half (see [`watch_page_tags`]) — not per request, and not in
    /// the handler, whose write path is the one that has to stay cheap.
    pub page_tags: watch::Receiver<Arc<PageTags>>,
    /// The sending end of [`Self::page_tags`]; only [`run_io`] sends.
    pub page_tags_tx: Arc<watch::Sender<Arc<PageTags>>>,
}

/// GH #1002: the validators of one published page map, by route.
///
/// `of` is the snapshot they were computed from. A request that reads a newer
/// map than the tags (the watcher has not run yet) computes its page's tag
/// itself rather than answer with a validator of the previous content.
#[derive(Default)]
pub struct PageTags {
    /// The page map these tags belong to.
    pub of: Arc<PageMap>,
    /// Route → the content hash of what the shell embeds for it.
    pub tags: HashMap<String, [u8; 16]>,
}

/// GH #1002: a page's `Cache-Control`. `no-cache`, because a page is a live
/// render; `private`, because it carries the session token of one load.
pub const PAGE_CACHE_CONTROL: &str = "private, no-cache";

/// GH #1002: what the GET shell embeds of a LARGE page: its slots up to here.
///
/// A6 (warm second visit ≤ 300 KB) left the page about 50 KB next to a join of
/// about 230 KB; 48 KB is the first screen of the measured city (its full
/// page was 636 KB, the same tree the join carries). The join's pieces fill
/// the rest. It is a budget, not a threshold: only a page above
/// [`crate::web::render::CUT_ABOVE_BYTES`] is cut at all, so a display page
/// (whose screen alone is larger than 48 KB) is served whole as before.
pub const PAGE_INLINE_BYTES: usize = 48 * 1024;

impl WebIo {
    /// Build the I/O state for a cell at `cell_path`.
    ///
    /// Eight arguments, because every one of them is a channel end or a value
    /// the factory alone knows: the same shape [`crate::voice::io::VoiceIo::new`]
    /// carries, and grouping them into a struct would only move the list.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mount: String,
        identity_header: String,
        cell_path: &str,
        pages: watch::Receiver<Arc<PageMap>>,
        assets: watch::Receiver<Arc<AssetMap>>,
        ready: watch::Receiver<bool>,
        pushes: mpsc::Receiver<WebReconfig>,
        surfaces: Arc<meclaw_colony::SurfaceRegistry>,
    ) -> Self {
        let (tags_tx, tags_rx) = watch::channel(Arc::new(PageTags::default()));
        Self {
            mount,
            identity_header,
            trusted: Arc::new(meclaw_colony::surfaces::loopback_only()),
            peer_trusted: false,
            link_mounts: Arc::new(Vec::new()),
            cell_path: Arc::from(cell_path),
            pages,
            assets,
            ready,
            viewers: Arc::new(ViewerRegistry::default()),
            // Filled by `run_io`, which is where the events channel first
            // exists. The listener is not running before that, so no request
            // can observe the `None`.
            events_tx: None,
            pushes: Arc::new(Mutex::new(Some(pushes))),
            surfaces,
            // Minted by `run_io`, which is the only side that knows when this
            // half goes away.
            shutdown: None,
            join_chunk: crate::web::params::JOIN_CHUNK_DEFAULT,
            join_timeout_ms: None,
            page_tags: tags_rx,
            page_tags_tx: Arc::new(tags_tx),
        }
    }

    /// The state one handed connection is served with: this display's state,
    /// plus whether the connection's peer address is a trusted proxy (GH #833).
    pub fn for_connection(&self, peer: std::net::IpAddr) -> Self {
        Self {
            peer_trusted: meclaw_colony::surfaces::admits(&self.trusted, peer),
            ..self.clone()
        }
    }
}

/// Escape text for an HTML text node or a double-quoted attribute.
///
/// The values reaching the shell come from a `config.json` that a `code` cell
/// in the same colony can write, so they are untrusted for this purpose. Same
/// rule as the API-side dead render.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// The one default style the shell ships: the runtime's own connection states.
///
/// **This is not the cell deciding what a display looks like.** That rule holds
/// (`templates/web/README.md`, "The stylesheet link rides on `stack`"): the
/// shell links no stylesheet and says nothing about the page inside it. What it
/// does say something about is the *container it writes itself* — the
/// `data-phx-main` div — in the states the *client it ships* puts on it.
/// `setContainerClasses` in the vendored bundle writes `phx-loading`,
/// `phx-error`, `phx-client-error` and `phx-server-error` there, after a short
/// delay so a blink does not flash anything. Shipping a runtime that publishes
/// a state and no way to see it is the half-delivery this closes: a page whose
/// socket is gone otherwise keeps drawing its last picture, indefinitely, with
/// nothing on screen to say so.
///
/// **A page overrides it by saying the same thing later.** The block sits in
/// `<head>`; a page's own rules arrive in the body and win the cascade at equal
/// specificity on document order. `templates/colony-view` already carries such
/// rules, and every property declared here is one it declares too — so on that
/// page the banner is entirely its own, and there is exactly one, because
/// `::after` is one box per element.
///
/// **No dim.** Dimming is deliberately left to the page. `opacity` on the
/// container would fade the banner with it, and `opacity` on the container's
/// children MULTIPLIES with a dim the page already applies further down
/// (colony-view's `.colony-view` is a grandchild, so 0.5 × 0.5 = 0.25 — a
/// picture the operator cannot read any more). The banner is the signal; how
/// far a page fades behind it is that page's judgement.
const CONNECTION_STATE_CSS: &str = concat!(
    "[data-phx-main].phx-loading::after,",
    "[data-phx-main].phx-error::after,",
    "[data-phx-main].phx-client-error::after,",
    "[data-phx-main].phx-server-error::after{",
    "content:\"connection lost — this page may be out of date\";",
    "position:fixed;z-index:9;left:50%;top:0;transform:translateX(-50%);",
    "padding:5px 14px 6px;background:#ffffff;color:#b3261e;",
    "border:1px solid #b3261e;border-radius:0 0 8px 8px;",
    "font:11.5px/1.4 ui-sans-serif,system-ui,sans-serif;pointer-events:none;}",
);

/// The longest `X-Forwarded-Prefix` this cell will read, in characters after
/// the leading slash. A proxy path deeper than this is a mistake, not a mount
/// point.
const PREFIX_MAX: usize = 200;

/// Whether a proxy's prefix is one this cell will write into its own links.
///
/// The grammar is `^/[A-Za-z0-9._~/-]{0,200}$` without a trailing slash — path
/// characters that need no escaping, and nothing else. Everything a value could
/// smuggle into an HTML attribute or a URL (a quote, a `?`, a `#`, a `%`, a
/// space) is therefore outside it, so a malformed value is **ignored** rather
/// than sanitised into something that looks plausible (O-P-3).
///
/// # The two shapes the character set does not catch
///
/// A prefix is written into a `<base href>` and into a `<script src>`, and two
/// shapes built entirely from allowed characters are not paths at all:
///
/// * **`//host`** is protocol-relative. `//evil.com` passes the character set,
///   and the shell would then write
///   `<script src="//evil.com/screen/@client/…">` — a script tag pointed at a
///   host the *requester* named. The header reaches this cell from whoever
///   spoke HTTP to the listener, and a cache in front keyed on the URL alone
///   is the classic `X-Forwarded-*` poisoning shape.
/// * **`..`** walks the base up. `/a/../..` is three allowed segments and one
///   silent escape from the path an operator configured.
///
/// Both are refused rather than normalised, for the reason the whole function
/// exists: this cell does not know what the proxy in front meant, and a
/// repaired prefix is a guess about somebody else's configuration.
fn prefix_is_usable(prefix: &str) -> bool {
    let Some(rest) = prefix.strip_prefix('/') else {
        return false;
    };
    if rest.len() > PREFIX_MAX || prefix.ends_with('/') {
        return false;
    }
    // The leading empty segment is what makes `//host` protocol-relative.
    if prefix.starts_with("//") {
        return false;
    }
    if rest.split('/').any(|segment| segment == "..") {
        return false;
    }
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '/' | '-'))
}

/// Where this display's own URLs start, for the request `headers` describe.
///
/// Two parts: what a reverse proxy says it stripped (`X-Forwarded-Prefix`, the
/// one header read for this — O-P-3) and the mount the colony's listener
/// reaches this cell under. Without a proxy the base is `/<mount>`; behind
/// `location ^~ /egon/` with `X-Forwarded-Prefix /egon` it is `/egon/<mount>`,
/// and every link the shell writes moves with it.
///
/// A header the grammar refuses is ignored, never trusted and never repaired:
/// the value comes from whoever spoke HTTP to the listener, and a client that
/// can reach the port directly can set it too.
pub(crate) fn base_of(headers: &HeaderMap, mount: &str) -> String {
    let prefix = headers
        .get("x-forwarded-prefix")
        .and_then(|v| v.to_str().ok())
        .filter(|p| prefix_is_usable(p))
        .unwrap_or("");
    format!("{prefix}/{mount}")
}

/// The viewport every page of a `web` cell declares.
///
/// `width=device-width` is what makes a phone lay the page out at its own
/// width instead of at an assumed 980 px. `viewport-fit=cover` is the other
/// half and the one that is invisible when it is missing: without it a
/// browser on a device with a notch or a home indicator lays the page out
/// INSIDE the safe area, and every `env(safe-area-inset-*)` a stylesheet asks
/// for reads 0 — a correct answer to the wrong question. The display sheet
/// lifts its OS mark by exactly that inset (`display@2.4.0`), so without this
/// attribute the mark sits under the home indicator on an iPhone.
const VIEWPORT: &str = "width=device-width, initial-scale=1, viewport-fit=cover";

/// The dead render this cell serves, relative to `base`.
///
/// `body` is the materialised page, already rendered — so this is a string
/// concatenation and nothing else. The LiveView client attaches to the same
/// markup on connect rather than replacing it, which is what makes the first
/// paint the real page instead of a spinner.
///
/// Every URL in it is written from `base` ([`base_of`]) rather than from the
/// origin root: this cell no longer owns an origin, it owns a mount inside one,
/// and a page that linked `/live` would join whatever else the listener has
/// there.
///
/// # Why the head carries a `<base>`
///
/// The page body is materialised **before** any request arrives, so nothing in
/// it can know the mount, let alone the prefix a proxy stripped from this one
/// request. A component that wants the cell's own stylesheet therefore writes
/// a RELATIVE URL (`vision.css`), and `<base>` is what makes that resolve to
/// the same file from `/<mount>/` and from `/<mount>/a/b` alike. Without it a
/// page one segment deep would ask for `/<mount>/a/vision.css`, and an asset
/// row would be unreachable from exactly the pages that are not the root.
///
/// # Why the shell carries no inline script (GH #867)
///
/// The shell used to boot LiveView from an inline `<script>` whose text held
/// the socket URL -- proxy prefix plus mount. A proxy that sets a
/// Content-Security-Policy of `script-src 'self'` can only admit inline script
/// by its hash, and that hash was different on every deployment path: a proxy
/// in front of many colonies would have needed one per colony, and a new
/// colony would have been a proxy reload. So the socket URL is a `<meta>`
/// (`meclaw-live`, data, not script) and the boot is `@client/boot.js`, served
/// from the same origin as the bundles. The shell itself now runs under
/// `script-src 'self'`; a page that brings inline script of its own needs
/// that script's hash listed (`templates/display/csp.json` publishes the
/// display's).
///
/// `boot.js` stands after the body and without `defer`: a page's hook scripts
/// sit in the body and register on `window.SurfaceHooks` before the socket
/// constructor reads it, exactly as the inline block did. The `<style>` stays:
/// `style-src` needs `'unsafe-inline'` for the display's own attributes
/// anyway, and a style block cannot run anything.
#[cfg(test)]
pub(crate) fn shell(base: &str, cell_path: &str, title: &str, body: &str) -> String {
    shell_with(base, cell_path, title, body, None)
}

/// [`shell`], with the join timeout the page states to its client (GH #1002).
#[cfg(test)]
pub(crate) fn shell_with(
    base: &str,
    cell_path: &str,
    title: &str,
    body: &str,
    join_timeout_ms: Option<u64>,
) -> String {
    shell_for(
        base,
        cell_path,
        title,
        body,
        join_timeout_ms,
        &session::mint(cell_path),
    )
}

/// The shell under a token the caller minted, with the join timeout the page
/// states to its client (GH #1002). The page handler mints the token itself
/// because the page's validator covers it.
///
/// Every client file is named with its `?v=<stamp>`: the stamp is the file's
/// validator, so the URL changes exactly when the file does and the answer can
/// be `immutable` — a second visit does not even ask. `join_timeout_ms` is
/// written as `<meta name="meclaw-join-timeout">` only when set, so a page
/// without the param is the page it was.
fn shell_for(
    base: &str,
    cell_path: &str,
    title: &str,
    body: &str,
    join_timeout_ms: Option<u64>,
    token: &str,
) -> String {
    let container = session::container_id(cell_path);
    let stamp = |f: &str| {
        client_file(f)
            .map(|c| c.stamp().to_string())
            .unwrap_or_default()
    };
    let timeout = join_timeout_ms
        .map(|ms| format!("<meta name=\"meclaw-join-timeout\" content=\"{ms}\">\n"))
        .unwrap_or_default();

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"{VIEWPORT}\">\n\
         <base href=\"{base}/\">\n\
         <meta name=\"csrf-token\" content=\"{token}\">\n\
         <meta name=\"meclaw-live\" content=\"{base}/live\">\n\
         {timeout}\
         <title>{title}</title>\n\
         <style>{states}</style>\n\
         </head>\n<body>\n\
         <div id=\"{container}\" data-phx-main data-phx-session=\"{token}\" data-phx-static=\"\">\n\
         {body}\n\
         </div>\n\
         <script src=\"{base}/@client/phoenix.min.js?v={v_phx}\"></script>\n\
         <script src=\"{base}/@client/phoenix_live_view.min.js?v={v_lv}\"></script>\n\
         <script src=\"{base}/@client/boot.js?v={v_boot}\"></script>\n\
         </body>\n</html>\n",
        token = esc(token),
        // Hex digits only: nothing to escape.
        v_phx = stamp("phoenix.min.js"),
        v_lv = stamp("phoenix_live_view.min.js"),
        v_boot = stamp("boot.js"),
        title = esc(title),
        container = esc(&container),
        // Sanitised before it got here — the grammar in `prefix_is_usable`
        // admits no character HTML would have to escape — and escaped anyway,
        // because a value that came off the wire is escaped where it is
        // written, not where it was checked.
        base = esc(base),
        // A compile-time constant with no `<` in it: nothing here comes from a
        // `config.json`, so there is nothing to escape.
        states = CONNECTION_STATE_CSS,
        // Already-rendered HTML: escaped per prop when it was built, by the
        // renderer that knows which props were declared as markup.
        body = body,
    )
}

/// `GET /@client/<file>` — the vendored LiveView bundles and the two client
/// files of our own (`boot.js`, `display-mic-worklet.js`), compiled in.
///
/// A closed list rather than a lookup (`meclaw_surface::bundle`): the file name
/// comes from a URL, and a list makes traversal impossible rather than guarded.
///
/// GH #1002: with the stamp the shell writes (`?v=<stamp>` equal to this
/// binary's), the answer is `immutable` — the URL changes when the file does,
/// so a browser never has to ask again. Without it (the microphone worklet is
/// loaded that way), or with a stamp that is not this binary's, it is
/// `no-cache` plus an `ETag`: since GH #867 the boot and the worklet are files
/// here, and a heuristically cached copy would keep running a client the
/// binary has long replaced; the validator makes the check a `304`.
/// Measured: the three client files were 148 KB on EVERY visit.
async fn get_client(
    axum::extract::Path(file): axum::extract::Path<String>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    match client_file(&file) {
        Some(c) => cached_response(
            header::HeaderValue::from_static(c.content_type),
            c.body.as_bytes(),
            c.gz.as_deref(),
            &c.etag,
            stamped(uri.query(), c.stamp()),
            &headers,
        ),
        None => miss(),
    }
}

/// Whether a query carries `v=<stamp>` for exactly this file's stamp.
fn stamped(query: Option<&str>, stamp: &str) -> bool {
    !stamp.is_empty()
        && query.is_some_and(|q| q.split('&').any(|kv| kv.strip_prefix("v=") == Some(stamp)))
}

/// Whether the request accepts gzip (`q` above zero).
fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|item| {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or("").trim();
            if !coding.eq_ignore_ascii_case("gzip") {
                return false;
            }
            parts
                .filter_map(|p| p.trim().strip_prefix("q="))
                .all(|q| q.trim().parse::<f32>().map_or(true, |q| q > 0.0))
        })
}

/// Whether `If-None-Match` names `etag` (or is `*`). A weak comparison, as
/// RFC 9110 § 13.1.2 asks for this header.
fn none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag)
}

/// The validator of a gzip variant: the file's, with `-gz` inside the quotes.
/// Two representations, two validators — a cache must never confirm a raw
/// copy with the compressed one's tag.
fn gz_tag(etag: &str) -> String {
    format!("{}-gz\"", etag.trim_end_matches('"'))
}

/// GH #1002: one file as a response — validator, `304`, cache policy, and the
/// precompressed variant when the client takes gzip.
///
/// Nothing is compressed here: `gz` was computed when the file was loaded
/// (R-H4-3). `Vary: Accept-Encoding` goes on every answer of a file that HAS a
/// variant, so a proxy in front never hands one client's encoding to another.
fn cached_response(
    content_type: header::HeaderValue,
    body: &[u8],
    gz: Option<&[u8]>,
    etag: &str,
    immutable: bool,
    headers: &HeaderMap,
) -> Response {
    let use_gz = gz.filter(|_| accepts_gzip(headers));
    let tag = match use_gz {
        Some(_) => gz_tag(etag),
        None => etag.to_string(),
    };
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut resp = if none_match(headers, &tag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = use_gz.unwrap_or(body).to_vec().into_response();
        r.headers_mut().insert(header::CONTENT_TYPE, content_type);
        if use_gz.is_some() {
            r.headers_mut().insert(
                header::CONTENT_ENCODING,
                header::HeaderValue::from_static("gzip"),
            );
        }
        r
    };
    let h = resp.headers_mut();
    if let Ok(v) = header::HeaderValue::from_str(&tag) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache),
    );
    if gz.is_some() {
        h.insert(
            header::VARY,
            header::HeaderValue::from_static("Accept-Encoding"),
        );
    }
    resp
}

/// `GET /` and `GET /*path` — a page, or a file, or the one 404.
///
/// **The `pages` table is the only route source** (R-W8-3): a route nothing
/// declares is not a page here. Since GH #393 it may still be a file — the
/// `assets` table is the cell's second declared surface, and until this handler
/// existed nothing delivered it.
///
/// # Why one handler and not two routes
///
/// Both surfaces live in the same origin-relative namespace, so a router built
/// from two competing patterns over the same wildcard would decide which one a
/// path reaches by axum's matching order — that is shadowing, and it would make
/// a whole table quietly unreachable for a class of paths. One handler asks
/// **both** maps for **every** path, so no row of either table can be made
/// unreachable by the other's existence. That is provable by construction
/// rather than by reading a route table.
///
/// # Which surface wins a collision
///
/// Pages. If both tables declare the identical path, the page answers: R-W8-3
/// says the `pages` table is the only route source, and an asset that could
/// take over a declared route would make that sentence false. The reverse
/// preference would also be the more damaging accident — an asset named `/`
/// would blank the display, while a page named `/vision.css` merely serves HTML
/// at an odd name. (`/@client/…` and `/live/websocket` are reserved earlier in
/// the router and reach neither map; the route grammar bars `@` and `live` from
/// pages for the same reason.)
///
/// Either way this is R-W8-4(a)'s request path: two published snapshots, no
/// database, no cell call — a wedged colony still serves its pages and its
/// files.
async fn get_path(State(io): State<WebIo>, headers: HeaderMap, uri: axum::http::Uri) -> Response {
    serve_path(&io, &headers, uri.path(), uri.query())
}

/// `GET /<mount>/` — the display's own root.
///
/// Its own route because `nest` does not answer the prefix WITH a trailing
/// slash: the nested router would be handed an empty path and match nothing,
/// and `/<mount>/` is exactly the URL a person types and a `<base href>`
/// resolves against. It is the same handler over the route `/`.
async fn get_root(State(io): State<WebIo>, headers: HeaderMap) -> Response {
    serve_path(&io, &headers, "/", None)
}

/// The body of [`get_path`]: one path, both declared surfaces.
fn serve_path(io: &WebIo, headers: &HeaderMap, path: &str, query: Option<&str>) -> Response {
    // Before the first publish this display has nothing to say about any route,
    // and saying `404` would be claiming it does (GH #395). The window closes on
    // its own; a caller that waits gets the page.
    if !*io.ready.borrow() {
        return starting();
    }

    let pages = io.pages.borrow().clone();
    if let Some(page) = pages.get(path) {
        // GH #1002: the validator is the route's content (computed once per
        // publish), the base this request is served under AND the session
        // token minted for this load -- together exactly the body below. The
        // token is in it because its nonce is the `session_id` of every
        // semantic event (`session.rs`: two loads of one surface never carry
        // the same string): a `304` would hand a reload, a second tab or the
        // next person behind a shared cache the old load's nonce. A fresh load
        // therefore never matches an old validator and is always a `200`; the
        // validators that pay are the client's and the assets' (A6), not the
        // page's.
        let base = base_of(headers, &io.mount);
        let token = session::mint(&io.cell_path);
        let tags = io.page_tags.borrow().clone();
        let content = match tags.tags.get(path) {
            Some(t) if Arc::ptr_eq(&tags.of, &pages) => *t,
            _ => page_hash(page, io.join_timeout_ms),
        };
        let mut seed = content.to_vec();
        seed.extend_from_slice(base.as_bytes());
        seed.push(0);
        seed.extend_from_slice(token.as_bytes());
        let etag = etag_of(&seed);
        if none_match(headers, &etag) {
            let mut resp = StatusCode::NOT_MODIFIED.into_response();
            if let Ok(v) = header::HeaderValue::from_str(&etag) {
                resp.headers_mut().insert(header::ETAG, v);
            }
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static(PAGE_CACHE_CONTROL),
            );
            return resp;
        }
        return (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                // A page is a live render and must never be served from a
                // heuristic browser cache: it names the client files and
                // carries a display's hook scripts inline, and a cached copy
                // keeps running a client the cell has long replaced — seen as
                // fixed bugs that will not die in one person's tab. `no-cache`
                // still allows conditional reuse; the response has no
                // validators, so in practice it means "fetch". Since GH #1002
                // it has one, over the body with its token, and `private`: a
                // page carries one load's session token, which no shared cache
                // may hand to a second person.
                (header::CACHE_CONTROL, PAGE_CACHE_CONTROL.to_string()),
                (header::ETAG, etag),
            ],
            shell_for(
                &base,
                &io.cell_path,
                &page.title,
                &page.page_body(PAGE_INLINE_BYTES),
                io.join_timeout_ms,
                &token,
            ),
        )
            .into_response();
    }

    let assets = io.assets.borrow().clone();
    match assets.get(path) {
        Some(asset) => asset_response(asset, headers, query),
        None => miss(),
    }
}

/// GH #1002: the content hash of what the shell embeds for `page` — the cut
/// body, the title, the stated join timeout and the client stamps the shell
/// names (a new binary is a new page even when the content is not).
fn page_hash(page: &crate::web::render::Materialized, join_timeout_ms: Option<u64>) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(page.page_body(PAGE_INLINE_BYTES).as_bytes());
    h.update([0]);
    h.update(page.title.as_bytes());
    h.update([0]);
    h.update(join_timeout_ms.unwrap_or(0).to_le_bytes());
    for c in client_files().values() {
        h.update(c.etag.as_bytes());
    }
    let digest = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// GH #1002: keep [`WebIo::page_tags`] current — one hash per route per
/// publish, in the I/O half, so a GET never hashes a page and the handler's
/// write path never does either.
async fn watch_page_tags(
    mut pages: watch::Receiver<Arc<PageMap>>,
    tags_tx: Arc<watch::Sender<Arc<PageTags>>>,
    join_timeout_ms: Option<u64>,
) {
    loop {
        let map = pages.borrow_and_update().clone();
        // Off the worker, like the eager init in `run_io`: hashing every route
        // is CPU work, and the first one waits for the client files' digests.
        let of = Arc::clone(&map);
        let Ok(tags) = tokio::task::spawn_blocking(move || {
            of.iter()
                .map(|(route, page)| (route.clone(), page_hash(page, join_timeout_ms)))
                .collect::<HashMap<_, _>>()
        })
        .await
        else {
            return;
        };
        tags_tx.send_replace(Arc::new(PageTags { of: map, tags }));
        if pages.changed().await.is_err() {
            return;
        }
    }
}

/// One asset row, as a response.
///
/// The `Content-Type` is taken from the row and **parsed** rather than trusted:
/// the value came out of a seed file, and axum's header tuple conversion
/// panics on a string that is not a legal header value. A seed file does not
/// get to panic a request task, so an unusable one falls back to
/// `application/octet-stream` — which is honestly what an unlabelled byte
/// stream is.
fn asset_response(asset: &Asset, headers: &HeaderMap, query: Option<&str>) -> Response {
    let value = header::HeaderValue::from_str(&asset.content_type).unwrap_or_else(|_| {
        tracing::warn!(
            content_type = %asset.content_type,
            "web: asset content_type is not a legal header value — serving it unlabelled"
        );
        header::HeaderValue::from_static("application/octet-stream")
    });
    // GH #1002: a validator, a `304`, and the variant computed at load. A
    // request that names the file's own stamp (`?v=`) may keep it for good.
    cached_response(
        value,
        &asset.body,
        asset.gz.as_deref(),
        &asset.etag,
        stamped(query, asset.stamp()),
        headers,
    )
}

/// The one negative answer. Same body for "no such route", "no such file" and
/// "no such bundle": a display should not enumerate what it does not serve. Two
/// lookups behind one handler must not become two distinguishable refusals, or
/// a probe could read the difference as a table of contents.
fn miss() -> Response {
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

/// The answer while the handler half has not published yet (GH #395).
///
/// `503` and not `404`, because the two say different things and a proxy in
/// front acts on the difference: `404` is "this route does not exist", which is
/// a statement about the page map — and before the first publish there is no
/// page map to make a statement from. It is also this cell's existing
/// vocabulary for "reachable, cannot serve you" (see the handler-gone arm
/// below).
fn starting() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "starting\n").into_response()
}

/// Who the proxy says is on this connection, if the operator named a header.
///
/// Read at the upgrade and kept for the whole connection: a socket is one
/// browser tab, and the identity of the person in front of it does not change
/// mid-frame. With no `identity_header` param nothing is read and nothing is
/// stamped (O-P-4), and a header the proxy did not send stamps nothing either.
fn identity_of(headers: &HeaderMap, identity_header: &str) -> Option<String> {
    if identity_header.is_empty() {
        return None;
    }
    headers
        .get(identity_header)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// GH #833: the identity of a connection — the header, but only when the
/// connection came from a proxy in `trusted_proxies`. Anywhere else the header
/// is a line any client can write, so it names nobody, and the socket is
/// served without one (`hop.user_id` absent, O-P-4).
fn connection_identity(
    headers: &HeaderMap,
    identity_header: &str,
    peer_trusted: bool,
) -> Option<String> {
    identity_of(headers, identity_header).filter(|_| peer_trusted)
}

/// `GET /<mount>/live/websocket` — the LiveView transport.
///
/// The phoenix client appends exactly `/websocket` to the socket URL it is
/// handed, so the shell says `<base>/live` and the route is this. A plain GET
/// here is a 400 rather than a 404: the path is right, the request is not.
///
/// The upgrade request is also where the connection learns two things it can
/// read nowhere else: the base its page was served under (the join payload
/// carries the browser's URL, which carries the proxy's prefix) and who the
/// proxy says is looking.
async fn get_socket(
    State(io): State<WebIo>,
    headers: HeaderMap,
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let Some(up) = upgrade else {
        return (
            StatusCode::BAD_REQUEST,
            "this path is a websocket endpoint\n",
        )
            .into_response();
    };
    let Some(events_tx) = io.events_tx.clone() else {
        // Only reachable if a router were built outside `run_io`.
        return (StatusCode::SERVICE_UNAVAILABLE, "no handler\n").into_response();
    };
    let viewers = io.viewers.clone();
    let base = base_of(&headers, &io.mount);
    let user_id = connection_identity(&headers, &io.identity_header, io.peer_trusted);
    up.on_upgrade(move |ws| run_connection(ws, io, events_tx, viewers, base, user_id))
}

/// The cell's router, as it is reached under `/<mount>` — see [`mounted_router`].
pub(crate) fn router(io: WebIo) -> Router {
    Router::new()
        // The transport, before the page wildcard: `/live/websocket` is not a
        // page and must not be looked up as one.
        .route("/live/websocket", get(get_socket))
        // Ordered most-specific first: the client prefix cannot be a page,
        // because a page route never starts with `@` (the same reservation the
        // API side makes, for the same reason).
        .route("/@client/:file", get(get_client))
        // Everything else is ONE handler over both declared surfaces — see
        // `get_path` for why pages and assets are not two competing routes.
        .route("/", get(get_path))
        .route("/*path", get(get_path))
        .with_state(io)
}

/// The cell's router under the name the listener hands it connections for.
///
/// The nest is the whole of the mount: `/screen/` reaches `get_path` with the
/// path `/`, `/screen/live/websocket` reaches the socket, and the display sees
/// its own route grammar unchanged behind it. Two displays on one listener are
/// therefore two nests, and neither can be reached under the other's name.
pub(crate) fn mounted_router(io: WebIo) -> Router {
    let mount = io.mount.clone();
    let root: Router = Router::new()
        .route(&format!("/{mount}/"), get(get_root))
        .with_state(io.clone());
    root.nest(&format!("/{mount}"), router(io))
}

/// The I/O loop: register the mount, serve what the listener hands over, and
/// stay up for the cell's whole life.
///
/// **A1′**: this function must not return voluntarily while the cell is live.
/// A clean early return would silence the I/O side while the handler keeps
/// running and open the "io-finish-first" loss class the trait documents. Only
/// the shutdown signal — the handler closing the reconfig channel — ends it.
///
/// # Why there is no listener here any more (`web@2.0.0`)
///
/// A display owned a port until `web@1.1.0`, bound it here, and moved it with a
/// `Rebind` round. It owns a **name** now: the colony's one listener peeks the
/// first path segment of every connection it accepts and hands the stream,
/// unread, to whoever registered that name. So this half binds nothing, and a
/// port collision — the failure the round machinery existed to recover from —
/// cannot happen to a display at all. What can is a name another cell holds,
/// and that is reported once, as [`WebEvent::MountFailed`]: the cell stays
/// alive, serves nobody, and an operator reads which name collided.
///
/// The registration happens once per life and is released with it (O-P-2): a
/// `mount` a params update names takes effect on the next life, because a live
/// remount would move a running display out from under the proxy rule pointed
/// at it while its viewers hold sockets on the old name.
pub async fn run_io(
    io: WebIo,
    events_tx: mpsc::Sender<WebEvent>,
    mut reconfig_rx: mpsc::Receiver<WebReconfig>,
) {
    // The listener half learns where to send browser events only here, because
    // this is where the channel exists.
    let mut io = io;
    io.events_tx = Some(events_tx.clone());
    let chunk = io.join_chunk;
    let viewers = io.viewers.clone();
    let mut pushes = io
        .pushes
        .lock()
        .await
        .take()
        .expect("run_io takes the push receiver exactly once");
    // Cloned once, before the loop: the resync needs the published pages, and
    // `io` is cloned into a router per handed connection.
    let pages = io.pages.clone();
    // Who owes a whole-tree resync (GH #414), for as long as the I/O half
    // lives. It lives here rather than in the registry because it is the
    // fan-out's bookkeeping and nothing else reads it — one owner, no lock.
    let mut dirty: HashMap<String, Addressed> = HashMap::new();

    let surfaces = Arc::clone(&io.surfaces);
    let mount = io.mount.clone();
    // The name goes on the table before anything can be handed over, once per
    // life (ADR-0031). A name another cell holds is an operator's mistake to
    // read — not a reason to tear the cell down, which is the stance a port
    // collision used to get. The token comes back with the receiver and is what
    // the unregister at the end of this life must carry: a respawn that
    // registered while this half was still draining holds a newer one, and a
    // spent token removes nothing. It is kept in a [`MountGuard`], so an end
    // that never reaches the shutdown arm takes the mount with it as well.
    let mut registration: Option<MountGuard> = None;
    let handoff: Option<mpsc::Receiver<meclaw_colony::HandedConnection>> = {
        let entry = meclaw_colony::SurfaceEntry {
            kind: "web",
            cell_path: meclaw_core::Path::new(&io.cell_path),
            // A display answers pages and sockets, never a topic link: it is
            // the side that OPENS one, on somebody else's mount (GH #643).
            links: None,
        };
        match surfaces.register(&mount, entry).await {
            Ok((rx, held)) => {
                registration = Some(MountGuard::new(&surfaces, &mount, held));
                Some(rx)
            }
            Err(e) => {
                let _ = events_tx.send(WebEvent::MountFailed(e.to_string())).await;
                None
            }
        }
    };

    // The accepted connections get a task of their own, and that is not
    // tidiness: the loop below carries the fan-out, whose `push_one` takes a
    // diff OFF the push channel before it reaches its viewers. A `select!` arm
    // that fired on every incoming request would cancel that future between
    // the two, and the diff would be gone — GH #414's loss class, re-opened at
    // the rate of a page load. Two tasks cannot cancel each other.
    // The two ends of the way out. Both are LOCALS of this future on purpose:
    // the substrate aborts `run_io` when the handler half returns, so what
    // runs at shutdown is what a dropped future drops — never a line after the
    // loop.
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    io.shutdown = Some(shutdown_rx);
    let mut serving = tokio::task::JoinSet::new();
    // GH #1002: the client files' validators and gzip variants, built now
    // rather than by the first page request (eager init) -- but only AFTER the
    // mount is on the table and off this worker. Hashing and gzip level 9 of
    // about 150 KB of client script took longer than a test's half second
    // in a debug build, and done inline before `surfaces.register` it held the
    // mount back: GH #644 read a mount table without the display
    // (`gh644_one_port_for_api_and_voice`, red after 0.50 s). Nothing
    // awaits it; an early request waits at the `OnceLock` instead.
    tokio::task::spawn_blocking(|| {
        let _ = client_files();
    });
    // GH #1002: the page validators, in this half and dropped with it.
    serving.spawn(watch_page_tags(
        io.pages.clone(),
        Arc::clone(&io.page_tags_tx),
        io.join_timeout_ms,
    ));
    if let Some(mut rx) = handoff {
        let io = io.clone();
        serving.spawn(async move {
            // Each connection is served for its whole life on a task of its
            // own: a WebSocket lives as long as the tab, and this loop has to
            // be back at `recv` before the next request arrives.
            //
            // The tasks live in a `JoinSet` rather than detached, and that is
            // what makes them end WITH the cell: dropping a `JoinSet` aborts
            // everything in it, and this one is a local of the task the
            // shutdown below aborts. A display that is going away has to take
            // its sockets with it — a browser left holding an open LiveView
            // socket draws the last snapshot forever and never reconnects to
            // the next life, because nothing ever told it the connection
            // ended. Until `web@1.1.0` the dropped `axum::serve` did this.
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    handed = rx.recv() => match handed {
                        Some(handed) => {
                            // GH #833: the connection's address decides,
                            // once, whether its identity header counts.
                            connections.spawn(crate::handed::serve_handed(
                                handed.stream,
                                mounted_router(io.for_connection(handed.peer.ip())),
                            ));
                        }
                        // The registry dropped the sender: a respawn of this
                        // cell registered over this entry. Nothing more will
                        // arrive, and the connections that are still open go
                        // with this task when it is aborted.
                        None => break,
                    },
                    // Reaping, so the set does not grow with every tab that
                    // was ever opened. Guarded, because `join_next` on an
                    // empty set is `None` at once and would spin.
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
    }

    // Only the handler going away ends this half (A1′) — and usually not even
    // that: the substrate aborts this future the moment the handler returns.
    // Both ways out are the same way out, because everything that has to
    // happen is a `Drop`:
    //
    // * `registration` gives the mount back (`MountGuard`), so the listener
    //   stops handing connections to a cell that is going;
    // * `serving` is a `JoinSet`, so the accept loop and every plain HTTP
    //   connection inside its own set end with it;
    // * `shutdown_tx` closes, and every upgraded WebSocket — which no
    //   `JoinSet` here holds — reads that and sends its client a close frame.
    //
    // The ORDER of the three is deliberate. `shutdown_tx` goes first, so every
    // socket that had already upgraded gets its close frame while the tasks
    // around it are still standing. `registration` goes last, so a connection
    // arriving during the teardown still finds the name on the table and reads
    // `503 surface busy` at the registration (`surfaces::listener`,
    // `refuse_busy`, the answer a mount whose handoff channel is closed gives)
    // rather than the API fallback's `404`. A `503` says "this name is here and
    // cannot take you now"; a `404` says "no such display", which is the one
    // thing that is not true at that moment.
    //
    // A line after this `select!` would run on exactly one of the two paths,
    // which is why there is none.
    tokio::select! {
        _ = wait_for_shutdown(&mut reconfig_rx) => {}
        _ = fan_out(&mut pushes, &viewers, &pages, &mut dirty, chunk) => {}
    }
    drop(shutdown_tx);
    drop(serving);
    drop(registration);
}

/// Wait for the handler to go away.
///
/// The reconfig channel carries nothing else since `web@2.0.0` — the `Rebind`
/// that moved a listener went with the listener — so its closing is the whole
/// of its remaining job. A `Push` arriving here would mean a caller outside
/// this cell built the wiring; it is dropped with a line rather than silently.
async fn wait_for_shutdown(reconfig_rx: &mut mpsc::Receiver<WebReconfig>) {
    loop {
        match reconfig_rx.recv().await {
            None => return,
            Some(WebReconfig::Push { route, .. }) => tracing::warn!(
                %route,
                "web: a diff arrived on the reconfig channel and was dropped"
            ),
            // Dropping `respond` tells the asker "no viewers here".
            Some(WebReconfig::Viewers { .. }) => tracing::warn!(
                "web: a viewers request arrived on the reconfig channel and was dropped"
            ),
        }
    }
}

/// The tree a viewer of `route` needs to be current again, as the frames'
/// payloads: the head, then the pieces (GH #1002, cut at `chunk` like a join).
///
/// A tree with more pieces than a queue of `max_frames` can EVER hold goes as
/// the one packed tree, as before. Not by gluing the pieces back together:
/// every piece numbers its shared statics in a `"p"` of its own (GH #1001), so
/// merged pieces would name each other's statics.
///
/// GH #1003: returned ENCODED, so the viewers of one route behind at the same
/// state share one encoding (see [`Trees`]).
fn whole_tree(
    pages: &watch::Receiver<Arc<PageMap>>,
    route: &str,
    chunk: usize,
    max_frames: usize,
) -> Option<Tree> {
    let map = pages.borrow();
    let page = map.get(route)?;
    let (head, pieces) = page.join_frames(chunk);
    if 1 + pieces.len() > max_frames {
        return Some(Tree {
            generation: map.generation,
            frames: vec![encode_payload(&page.packed_tree())],
        });
    }
    let mut out = Vec::with_capacity(1 + pieces.len());
    out.push(encode_payload(&head));
    out.extend(pieces.iter().map(encode_payload));
    Some(Tree {
        generation: map.generation,
        frames: out,
    })
}

/// One route's whole tree, encoded, with the generation of the pages it was
/// cut from (GH #1013: the viewer that is written it holds that generation).
struct Tree {
    generation: u64,
    frames: Vec<String>,
}

/// The whole trees one fan-out step has encoded, by route and queue size
/// (GH #1003).
///
/// One step reads one `PageMap` state, so a second viewer of the same route
/// that is owed the tree gets the same bytes without a second encoding — the
/// tree of a measured 2-D world is ~230 KB of JSON, and the resync after a
/// burst is owed by every slow screen at once.
#[derive(Default)]
struct Trees(HashMap<(String, usize), Option<Arc<Tree>>>);

impl Trees {
    /// The encoded tree of `route` for a queue of `max_frames`, encoded on
    /// first ask.
    fn get(
        &mut self,
        pages: &watch::Receiver<Arc<PageMap>>,
        route: &str,
        chunk: usize,
        max_frames: usize,
    ) -> Option<Arc<Tree>> {
        self.0
            .entry((route.to_string(), max_frames))
            .or_insert_with(|| whole_tree(pages, route, chunk, max_frames).map(Arc::new))
            .clone()
    }
}

/// A frame payload as JSON text: the one encoding every viewer's frame wraps
/// (GH #1003).
///
/// Before, every viewer's frame encoded the whole diff again
/// (`frames::push` per viewer): at three viewers of a panning 2-D world the
/// daemon ran at 76 % CPU (max 92) and the bundle answer at p95 233 ms. The
/// frame around it is per socket (`frames::push_raw`); the payload is not.
fn encode_payload(payload: &meclaw_core::JsonValue) -> String {
    #[cfg(test)]
    ENCODED.with(|n| n.set(n.get() + 1));
    meclaw_core::serde_json::to_string(payload).unwrap_or_default()
}

#[cfg(test)]
thread_local! {
    /// How many payloads this thread encoded: the lock's counter (GH #1003).
    static ENCODED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// GH #1002: hand a viewer the whole tree in pieces — all of them or none.
///
/// All or none, because the queue is the viewer's and a partial resync would
/// leave its empty slots to a later diff that may never come. A tree with more
/// pieces than the queue can EVER hold goes as one frame, as before.
///
/// GH #1013: every frame is stamped with the tree's generation; the viewer
/// holds that generation once it is written, and diffs it already contains
/// stay away.
fn send_tree(a: &Addressed, tree: &Tree) -> Result<(), bool> {
    if a.tx.is_closed() {
        return Err(false);
    }
    if a.tx.capacity() < tree.frames.len() {
        return Err(true);
    }
    for payload in &tree.frames {
        let frame = meclaw_surface::frames::push_raw(&a.join_ref, &a.topic, "diff", payload);
        match a
            .tx
            .try_send_page(ViewerMsg::Frame(frame), PageStamp::Tree(tree.generation))
        {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(true),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(false),
        }
    }
    Ok(())
}

/// Send one diff to every viewer of its route — and to a viewer that lost a
/// frame, the whole tree instead (GH #414).
///
/// A `Full` channel means that browser is behind, and dropping the frame was
/// always the right call for the others: one slow viewer must not hold up the
/// rest. What was wrong was **forgetting** it. The mark says "this one is
/// behind"; the next frame it can be given is the page's whole packed tree,
/// which is correct from any starting point — a positional diff is not, because
/// it patches a picture the viewer never received.
async fn push_one(
    push: WebReconfig,
    viewers: &Arc<ViewerRegistry>,
    pages: &watch::Receiver<Arc<PageMap>>,
    dirty: &mut HashMap<String, Addressed>,
    chunk: usize,
) {
    let (route, diff, generation) = match push {
        WebReconfig::Push {
            route,
            diff,
            generation,
        } => (route, diff, generation),
        // GH #1006: the `viewers` op. Answered here, in order with the pushes
        // before it; a gone asker is nothing to report.
        WebReconfig::Viewers { respond } => {
            let _ = respond.send(viewers.report().await);
            return;
        }
    };
    // GH #1003: the diff is encoded at most once for all of the route's
    // viewers, and so is the tree the ones behind are owed.
    let mut encoded: Option<String> = None;
    let mut trees = Trees::default();
    for a in viewers.on_route(&route).await {
        let behind = dirty.contains_key(&a.id);
        if let Some(tree) = behind
            .then(|| trees.get(pages, &a.route, chunk, a.tx.max_capacity()))
            .flatten()
        {
            match send_tree(&a, &tree) {
                Ok(()) => {
                    dirty.remove(&a.id);
                    a.tx.meter().resync_settled();
                }
                Err(false) => {
                    dirty.remove(&a.id);
                }
                Err(true) => {
                    // Still owed: keep the backlog report alive (its check
                    // repeats `high` once a second) as the whole-frame path did.
                    a.tx.meter().resync_owed();
                    dirty.insert(a.id.clone(), a);
                }
            }
            continue;
        }
        let payload = encoded.get_or_insert_with(|| encode_payload(&diff));
        // The tree rides BARE on the push lane (GH #413). The `{"diff": ...}`
        // wrapper is the *reply* shape; the client hands a push payload straight
        // to `Rendered.extract`, so a wrapper becomes one junk slot.
        let frame = meclaw_surface::frames::push_raw(&a.join_ref, &a.topic, "diff", payload);
        // GH #1013: stamped, so a viewer whose snapshot holds it drops it.
        match a
            .tx
            .try_send_page(ViewerMsg::Frame(frame), PageStamp::Diff(generation))
        {
            Ok(()) => {
                if dirty.remove(&a.id).is_some() {
                    a.tx.meter().resync_settled();
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // GH #1006: the mark is the one fact about a slow viewer the
                // app could not see; the meter reports it as `high`.
                a.tx.meter().resync_owed();
                dirty.insert(a.id.clone(), a);
            }
            // That browser is gone; its connection task cleans the registry up.
            Err(mpsc::error::TrySendError::Closed(_)) => {
                dirty.remove(&a.id);
            }
        }
    }
}

/// How often a viewer that lost a frame is offered the whole tree again.
///
/// Short, because the window it closes is a person watching a picture that is
/// already wrong; and only ever armed while somebody is actually marked, so an
/// idle display ticks not at all.
const RESYNC_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

/// Offer every marked viewer its page's whole tree again.
///
/// The mark is kept only while the channel is merely **full** — a closed one
/// belongs to a browser that is gone, and a page that has no tree (its route was
/// removed) cannot be resynced at all, so that mark is dropped rather than
/// retried forever.
fn resync(
    pages: &watch::Receiver<Arc<PageMap>>,
    dirty: &mut HashMap<String, Addressed>,
    chunk: usize,
) {
    let mut trees = Trees::default();
    dirty.retain(|_, a| {
        let Some(tree) = trees.get(pages, &a.route, chunk, a.tx.max_capacity()) else {
            return false;
        };
        // GH #1002: in the join's pieces, and only once they all fit.
        let still_owed = matches!(send_tree(a, &tree), Err(true));
        if !still_owed {
            a.tx.meter().resync_settled();
        }
        still_owed
    });
}

/// Fan every `Push` the handler sends out to the viewers of its route, and never
/// end on a dropped frame (GH #414).
///
/// `dirty` is lent, not owned: a round is a serving round, and a viewer's debt
/// outlives it (GH #594 — see the caller).
///
/// Two arms, and the second only exists while somebody is marked: a burst that
/// fills a viewer's channel is caught by the push path (the next frame that
/// viewer can be given is the whole tree), and the LAST frame of a burst — the
/// one with no successor — is caught by the retry. Without it a person who drops
/// a node sees the picture snap back and stay there while the database is
/// already correct.
async fn fan_out(
    pushes: &mut mpsc::Receiver<WebReconfig>,
    viewers: &Arc<ViewerRegistry>,
    pages: &watch::Receiver<Arc<PageMap>>,
    dirty: &mut HashMap<String, Addressed>,
    chunk: usize,
) {
    loop {
        if dirty.is_empty() {
            match pushes.recv().await {
                Some(push) => push_one(push, viewers, pages, dirty, chunk).await,
                None => return,
            }
        } else {
            tokio::select! {
                p = pushes.recv() => match p {
                    Some(push) => push_one(push, viewers, pages, dirty, chunk).await,
                    None => return,
                },
                _ = tokio::time::sleep(RESYNC_RETRY) => resync(pages, dirty, chunk),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::backlog::{Meter, Queued};
    use crate::web::cell::WebEvent;
    use crate::web::socket::Viewer;
    use meclaw_core::serde_json::json;

    /// One registered viewer, with a channel the test keeps the receiving end of.
    fn viewer(route: &str, cap: usize) -> (Viewer, mpsc::Receiver<Queued>) {
        let (tx, rx) = mpsc::channel::<Queued>(cap);
        (
            Viewer {
                tx: Outbox::new(tx, Arc::new(Meter::silent())),
                route: route.to_string(),
                join_ref: json!("1"),
                topic: "lv:c".to_string(),
                session_id: String::new(),
                screen: meclaw_core::JsonValue::Null,
            },
            rx,
        )
    }

    #[tokio::test]
    async fn a_fan_out_addresses_its_viewers_by_id_in_a_stable_order() {
        let viewers = ViewerRegistry::default();
        let (v2, _r2) = viewer("/", 4);
        let (v1, _r1) = viewer("/", 4);
        let (vx, _rx) = viewer("/other", 4);
        viewers.insert("v2".to_string(), v2).await;
        viewers.insert("v1".to_string(), v1).await;
        viewers.insert("v3".to_string(), vx).await;

        let addressed = viewers.on_route("/").await;
        let ids: Vec<&str> = addressed.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["v1", "v2"],
            "a fan-out names its viewers and visits them in a stable order"
        );
        assert!(
            addressed.iter().all(|a| a.route == "/"),
            "and each address carries the route it is joined to"
        );
    }

    use crate::web::render::Materialized;

    /// A page map with one route, so a resync has a tree to send.
    fn page_map() -> Arc<PageMap> {
        let mut m = PageMap::new();
        m.insert(
            "/".to_string(),
            Materialized {
                statics: vec!["<main>".to_string(), "</main>".to_string()],
                slots: vec![("n1".to_string(), "<i>one</i>".into())],
                title: "t".to_string(),
            },
        );
        Arc::new(m)
    }

    /// The payload of a phoenix frame: the fifth element of the tuple.
    fn payload(frame: &str) -> meclaw_core::JsonValue {
        let v: meclaw_core::JsonValue =
            meclaw_core::serde_json::from_str(frame).expect("a frame is JSON");
        v[4].clone()
    }

    fn diff_push(slot: &str, html: &str) -> WebReconfig {
        WebReconfig::Push {
            route: "/".to_string(),
            diff: json!({ slot: html }),
            generation: 1,
        }
    }

    /// The whole of GH #414's second half: a viewer whose channel was full does
    /// NOT silently keep a picture the database has left behind. The next frame
    /// it can be sent is the whole tree, never the diff it would have had.
    /// What a viewer's write loop lets through, in queue order (GH #1013):
    /// the queue drained through [`Meter::admits`], payloads only.
    fn written(rx: &mut mpsc::Receiver<Queued>, meter: &Meter) -> Vec<meclaw_core::JsonValue> {
        let mut out = Vec::new();
        while let Ok(q) = rx.try_recv() {
            if meter.admits(q.page)
                && let ViewerMsg::Frame(f) = q.msg
            {
                out.push(payload(&f));
            }
        }
        out
    }

    async fn push_gen(
        viewers: &Arc<ViewerRegistry>,
        pages: &watch::Receiver<Arc<PageMap>>,
        dirty: &mut HashMap<String, Addressed>,
        generation: u64,
        html: &str,
    ) {
        push_one(
            WebReconfig::Push {
                route: "/".to_string(),
                diff: json!({ "0": html }),
                generation,
            },
            viewers,
            pages,
            dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        )
        .await;
    }

    /// GH #1013 (review C1): a diff the join snapshot already holds is not
    /// written. The join read generation 4; the diffs of generations 3 and 4
    /// were fanned out after it registered (published before the snapshot,
    /// fanned out after it) and stay away, the one of generation 5 goes.
    #[tokio::test]
    async fn gh1013_a_diff_the_join_snapshot_holds_is_not_written() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        let (v, mut rx) = viewer("/", 8);
        let meter = Arc::clone(v.tx.meter());
        viewers.insert("v1".to_string(), v).await;
        meter.holds_snapshot(4);
        let mut dirty: HashMap<String, Addressed> = HashMap::new();
        for (generation, html) in [(3, "three"), (4, "four"), (5, "five")] {
            push_gen(&viewers, &pages_rx, &mut dirty, generation, html).await;
        }
        assert_eq!(written(&mut rx, &meter), vec![json!({"0": "five"})]);
    }

    /// GH #1013: the resync edge. A whole tree of generation 7 is written to
    /// a viewer that held 2; a diff of generation 6 fanned out after it (the
    /// tree contains it) stays away, the one of 8 goes. A tree older than the
    /// viewer's snapshot (a second join on the socket) is not written at all.
    #[tokio::test]
    async fn gh1013_a_resync_tree_holds_its_generation() {
        let mut m = (*page_map()).clone();
        m.generation = 7;
        let (_pages_tx, pages_rx) = watch::channel(Arc::new(m));
        let viewers = Arc::new(ViewerRegistry::default());
        let (v, mut rx) = viewer("/", 8);
        let meter = Arc::clone(v.tx.meter());
        viewers.insert("v1".to_string(), v).await;
        meter.holds_snapshot(2);
        let mut dirty: HashMap<String, Addressed> = HashMap::new();
        let owed = viewers.on_route("/").await.remove(0);
        dirty.insert(owed.id.clone(), owed);
        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(dirty.is_empty(), "the tree fits");
        push_gen(&viewers, &pages_rx, &mut dirty, 6, "six").await;
        push_gen(&viewers, &pages_rx, &mut dirty, 8, "eight").await;
        let tree = page_map().get("/").expect("route").packed_tree();
        assert_eq!(written(&mut rx, &meter), vec![tree, json!({"0": "eight"})]);

        meter.holds_snapshot(9);
        let owed = viewers.on_route("/").await.remove(0);
        dirty.insert(owed.id.clone(), owed);
        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(
            written(&mut rx, &meter).is_empty(),
            "a tree older than the snapshot is not written"
        );
    }

    #[tokio::test]
    async fn a_viewer_that_lost_a_frame_gets_the_tree_and_not_the_next_diff() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        // Capacity one, so the second push cannot fit: the burst that fills a
        // 64-slot channel on a live display is the same event, larger.
        let (v, mut rx) = viewer("/", 1);
        viewers.insert("v1".to_string(), v).await;
        let mut dirty: HashMap<String, Addressed> = HashMap::new();

        push_one(
            diff_push("0", "<i>a</i>"),
            &viewers,
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        )
        .await;
        assert!(dirty.is_empty(), "the first frame fits");

        push_one(
            diff_push("0", "<i>b</i>"),
            &viewers,
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        )
        .await;
        assert!(
            dirty.contains_key("v1"),
            "a dropped frame must be remembered, not forgotten"
        );

        // The browser reads its backlog: the slot is free again.
        let ViewerMsg::Frame(first) = rx.recv().await.expect("the first frame").msg else {
            panic!("a frame, not a close")
        };
        assert_eq!(payload(&first), json!({"0": "<i>a</i>"}));

        push_one(
            diff_push("0", "<i>c</i>"),
            &viewers,
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        )
        .await;
        let ViewerMsg::Frame(second) = rx.recv().await.expect("the second frame").msg else {
            panic!("a frame, not a close")
        };
        assert_eq!(
            payload(&second),
            page_map().get("/").expect("route").packed_tree(),
            "the frame after a drop is the WHOLE tree — a diff would patch a \
             picture the viewer never received"
        );
        assert!(
            dirty.is_empty(),
            "and the mark is cleared once the tree is on its way"
        );
    }

    /// The retry itself: a mark survives a full channel and is cleared the
    /// moment the tree fits.
    #[tokio::test]
    async fn a_resync_holds_its_mark_until_the_tree_actually_fits() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let (v, mut rx) = viewer("/", 1);
        let addressed = Addressed {
            id: "v1".to_string(),
            tx: v.tx.clone(),
            join_ref: v.join_ref.clone(),
            topic: v.topic.clone(),
            route: v.route.clone(),
        };
        // The channel is full before the first attempt.
        v.tx.try_send(ViewerMsg::Frame("busy".to_string()))
            .expect("prefill");
        let mut dirty = HashMap::from([("v1".to_string(), addressed)]);

        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(
            dirty.contains_key("v1"),
            "a viewer that is still wedged keeps its mark — the alternative is \
             ending on a dropped frame, which is the defect"
        );

        let _ = rx.recv().await.expect("the prefilled frame");
        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(dirty.is_empty(), "and the mark goes when the tree is sent");
        let ViewerMsg::Frame(f) = rx.recv().await.expect("the tree").msg else {
            panic!("a frame, not a close")
        };
        assert_eq!(
            payload(&f),
            page_map().get("/").expect("route").packed_tree()
        );
    }

    /// T6 (GH #1006): a full queue — the resync mark of GH #414 — is reported
    /// as `high` with the mark counted, whatever the byte and age thresholds
    /// say: it is the one state in which the viewer is certainly behind.
    #[tokio::test]
    async fn gh1006_a_resync_counts_as_high() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let policy = BacklogPolicy {
            report: true,
            // Thresholds far out of reach: only the mark can raise `high`.
            high_bytes: 1 << 30,
            high_ms: 60_000,
        };
        let meter = Arc::new(Meter::new(policy, Some(events_tx)));
        meter.joined("/", "s-1");
        let (tx, mut rx) = mpsc::channel::<Queued>(1);
        let v = Viewer {
            tx: Outbox::new(tx, Arc::clone(&meter)),
            route: "/".to_string(),
            join_ref: json!("1"),
            topic: "lv:c".to_string(),
            session_id: String::new(),
            screen: meclaw_core::JsonValue::Null,
        };
        v.tx.try_send(ViewerMsg::Frame("busy".to_string()))
            .expect("prefill");
        viewers.insert("v1".to_string(), v).await;
        let mut dirty = HashMap::new();
        push_one(
            diff_push("0", "<b>x</b>"),
            &viewers,
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        )
        .await;
        assert!(dirty.contains_key("v1"), "the queue was full: marked");

        let Ok(WebEvent::Backlog {
            high,
            resyncs,
            session_id,
            ..
        }) = events_rx.try_recv()
        else {
            panic!("a backlog report on the mark");
        };
        assert!(high, "a resync is high");
        assert!(resyncs >= 1, "with the mark counted");
        assert_eq!(session_id, "s-1");
        assert_eq!(meter.reading().resyncs_total, 1);

        // The viewer drains and the tree gets in: the debt is paid, and once
        // the queue is empty again the report is `clear`.
        let q = rx.recv().await.expect("the prefilled frame");
        meter.writing(q.at);
        meter.written(q.at, q.bytes);
        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(dirty.is_empty(), "the tree fit");
        let q = rx.recv().await.expect("the tree");
        meter.writing(q.at);
        meter.written(q.at, q.bytes);
        let Ok(WebEvent::Backlog { high, .. }) = events_rx.try_recv() else {
            panic!("a clear once caught up");
        };
        assert!(!high);
    }

    /// GH #1006: the `viewers` op is a request on the push channel, answered
    /// by the fan-out from the registry — the handler holds no lock of this
    /// half. The answer is one row per joined viewer, in id order.
    #[tokio::test]
    async fn gh1006_the_fan_out_answers_a_viewers_request() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        let (v2, _r2) = viewer("/b", 4);
        let (v1, _r1) = viewer("/a", 4);
        viewers.insert("v2".to_string(), v2).await;
        viewers.insert("v1".to_string(), v1).await;
        let (push_tx, mut pushes) = mpsc::channel(4);
        let mut dirty: HashMap<String, Addressed> = HashMap::new();
        let looping = {
            let viewers = Arc::clone(&viewers);
            tokio::spawn(async move {
                fan_out(
                    &mut pushes,
                    &viewers,
                    &pages_rx,
                    &mut dirty,
                    crate::web::params::JOIN_CHUNK_DEFAULT,
                )
                .await
            })
        };

        let (respond, rows) = tokio::sync::oneshot::channel();
        push_tx
            .send(WebReconfig::Viewers { respond })
            .await
            .expect("the fan-out is listening");
        let rows = rows.await.expect("the fan-out answers");
        let routes: Vec<&str> = rows.iter().filter_map(|r| r["route"].as_str()).collect();
        assert_eq!(
            routes,
            ["/a", "/b"],
            "one row per viewer, in id order: {rows:?}"
        );
        assert_eq!(rows[0]["frames"], json!(0));
        assert!(rows[0]["screen"].is_null());

        drop(push_tx);
        looping.await.expect("the fan-out ends with its channel");
    }

    /// GH #1002 T11: a resync of a large page goes in the join's pieces — no
    /// frame over the limit, all of them or none, and together the tree.
    #[tokio::test]
    async fn gh1002_a_resync_goes_in_chunks() {
        // Large enough to be cut at all (`CUT_ABOVE_BYTES`), in 40 slots.
        let slot = crate::web::render::CUT_ABOVE_BYTES / 40 + 100;
        let big = Materialized {
            statics: std::iter::once("<main>".to_string())
                .chain(std::iter::repeat_n(String::new(), 39))
                .chain(std::iter::once("</main>".to_string()))
                .collect(),
            slots: (0..40)
                .map(|i| {
                    (
                        format!("n{i}"),
                        format!("<p>{}</p>", "x".repeat(slot)).into(),
                    )
                })
                .collect(),
            title: "t".to_string(),
        };
        let whole = big.packed_tree();
        let mut m = PageMap::new();
        m.insert("/".to_string(), big);
        let (_pages_tx, pages_rx) = watch::channel(Arc::new(m));
        let limit = 3 * slot;

        // Room for two frames: the tree needs more, so nothing is sent yet.
        let (v, mut rx) = viewer("/", 64);
        for _ in 0..62 {
            v.tx.try_send(ViewerMsg::Frame("busy".to_string()))
                .expect("prefill");
        }
        let addressed = |v: &Viewer| Addressed {
            id: "v1".to_string(),
            tx: v.tx.clone(),
            join_ref: v.join_ref.clone(),
            topic: v.topic.clone(),
            route: v.route.clone(),
        };
        let mut dirty = HashMap::from([("v1".to_string(), addressed(&v))]);
        resync(&pages_rx, &mut dirty, limit);
        assert!(dirty.contains_key("v1"), "a partial resync is no resync");
        assert_eq!(v.tx.capacity(), 2, "and not one piece went into the queue");
        for _ in 0..62 {
            let _ = rx.recv().await;
        }

        resync(&pages_rx, &mut dirty, limit);
        assert!(dirty.is_empty());
        let mut tree = meclaw_core::JsonValue::Null;
        let mut frames = 0;
        while let Ok(q) = rx.try_recv() {
            let ViewerMsg::Frame(f) = q.msg else {
                panic!("a frame, not a close")
            };
            assert!(
                f.len() <= limit + 256,
                "a resync frame of {} bytes",
                f.len()
            );
            let p = payload(&f);
            if tree.is_null() {
                tree = p;
            } else {
                // GH #1013: a piece appends entries to the root's keyed list
                // and carries its count.
                for (k, val) in p["0"]["k"].as_object().expect("a list diff") {
                    tree["0"]["k"][k] = val.clone();
                }
            }
            frames += 1;
        }
        assert!(
            frames > 1,
            "a tree of 40 slots at a limit of three is several frames"
        );
        assert_eq!(tree, whole, "the pieces are the tree");
    }

    /// …and the loop actually runs it: one push, dropped, and NO further push.
    /// The viewer still ends up current.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_last_frame_a_viewer_gets_is_the_newest_state() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        // `wedged` sorts before `witness`, and `on_route` visits in id order
        // (Task 1) — so when the witness has its frame, the wedged one has
        // already been tried and dropped. No sleep, no guess.
        let (wedged, mut wedged_rx) = viewer("/", 1);
        wedged
            .tx
            .try_send(ViewerMsg::Frame("busy".to_string()))
            .expect("prefill");
        let (witness, mut witness_rx) = viewer("/", 8);
        viewers.insert("a-wedged".to_string(), wedged).await;
        viewers.insert("b-witness".to_string(), witness).await;

        let (push_tx, mut push_rx) = mpsc::channel::<WebReconfig>(8);
        let viewers_for_task = viewers.clone();
        let pages_for_task = pages_rx.clone();
        let job = tokio::spawn(async move {
            let mut dirty: HashMap<String, Addressed> = HashMap::new();
            fan_out(
                &mut push_rx,
                &viewers_for_task,
                &pages_for_task,
                &mut dirty,
                crate::web::params::JOIN_CHUNK_DEFAULT,
            )
            .await;
        });

        push_tx
            .send(diff_push("0", "<i>only</i>"))
            .await
            .expect("push");
        let _ = witness_rx.recv().await.expect("the witness sees the diff");

        // The browser catches up on its backlog. Nothing else is ever pushed.
        let _ = wedged_rx.recv().await.expect("the prefilled frame");
        // Failure-marker timeout, generous by convention: it only elapses when
        // the property under test is broken.
        let got = tokio::time::timeout(std::time::Duration::from_secs(30), wedged_rx.recv())
            .await
            .expect("a viewer that lost a frame must not be left stale")
            .expect("a frame")
            .msg;
        let ViewerMsg::Frame(f) = got else {
            panic!("a frame, not a close")
        };
        assert_eq!(
            payload(&f),
            page_map().get("/").expect("route").packed_tree(),
            "with no further write, the retry is what makes the last frame the \
             newest state"
        );

        drop(push_tx);
        let _ = job.await;
    }

    /// Every page a display serves says so when the socket is gone.
    ///
    /// Until this, the LiveView connection classes were styled in exactly one
    /// place — `templates/colony-view`'s own stylesheet, which travels inside
    /// that view's markup. Every other page of every other display drew the
    /// last picture it had for as long as the tab stayed open, with nothing on
    /// screen to say the picture had stopped moving.
    #[test]
    fn the_shell_styles_the_runtimes_own_connection_states() {
        let html = shell("/screen", "/web", "Home", "<h1>hello</h1>");

        let head = html.split("</head>").next().expect("the shell has a head");
        assert!(
            head.contains("<style>"),
            "the default belongs in the head, before the body it dims; shell was:\n{html}"
        );
        for state in [
            "phx-loading",
            "phx-error",
            "phx-client-error",
            "phx-server-error",
        ] {
            assert!(
                head.contains(&format!("[data-phx-main].{state}::after")),
                "the shell must style the {state} state the runtime it ships sets; shell was:\n{html}"
            );
        }
        assert!(
            head.contains("connection lost"),
            "the hint has to be readable prose, not a colour change alone"
        );
    }

    /// The base a request is answered from, and the one header that moves it.
    #[test]
    fn a_forwarded_prefix_is_read_only_when_it_is_a_path() {
        let bare = HeaderMap::new();
        assert_eq!(base_of(&bare, "screen"), "/screen");

        let mut moved = HeaderMap::new();
        moved.insert("x-forwarded-prefix", "/egon".parse().expect("a header"));
        assert_eq!(base_of(&moved, "screen"), "/egon/screen");

        // Everything else is ignored rather than repaired: the value comes off
        // the wire, and a client that reaches the listener can set it too.
        for bad in [
            "egon",             // not a path
            "/egon/",           // a trailing slash would double
            "/",                // the same, at the root
            "/e%2Fgon",         // an escape this cell will not write
            "/egon?x=1",        // a query string
            "/egon\"><script>", // the reason the grammar is narrow
            "//evil.com",       // protocol-relative: a host, not a path
            "//evil.com/egon",  // the same, dressed as a prefix
            "/..",              // one segment up, out of the mount
            "/egon/../..",      // and the walk that hides inside a path
            "/../egon",         // wherever the segment stands
        ] {
            let mut h = HeaderMap::new();
            let Ok(value) = bad.parse() else { continue };
            h.insert("x-forwarded-prefix", value);
            assert_eq!(
                base_of(&h, "screen"),
                "/screen",
                "{bad:?} must be ignored, not written into a link"
            );
        }
        let mut too_deep = HeaderMap::new();
        too_deep.insert(
            "x-forwarded-prefix",
            format!("/{}", "a".repeat(PREFIX_MAX + 1))
                .parse()
                .expect("a header"),
        );
        assert_eq!(base_of(&too_deep, "screen"), "/screen");
    }

    /// Every URL the shell writes starts at the base, and that is what makes a
    /// display reachable under a proxy path at all.
    #[test]
    fn the_shell_writes_its_links_from_the_base() {
        let html = shell("/egon/screen", "/web", "Home", "<h1>hello</h1>");
        assert!(
            html.contains("\"/egon/screen/live\""),
            "the socket URL rides the base; shell was:\n{html}"
        );
        assert!(
            html.contains("src=\"/egon/screen/@client/phoenix.min.js?v=")
                && html.contains("src=\"/egon/screen/@client/phoenix_live_view.min.js?v="),
            "and so do the bundles; shell was:\n{html}"
        );
        assert!(
            !html.contains("\"/live\"") && !html.contains("\"/@client/"),
            "nothing may be written from the origin root any more; shell was:\n{html}"
        );
        assert!(
            html.contains("<meta name=\"meclaw-live\" content=\"/egon/screen/live\">")
                && html.contains("src=\"/egon/screen/@client/boot.js?v="),
            "the boot reads the socket URL from a meta the base wrote; shell was:\n{html}"
        );
    }

    /// GH #867: the shell runs under `script-src 'self'`. Every `<script>` it
    /// writes has a `src`, so no proxy has to list a hash for it -- the inline
    /// boot it replaced had the base in its text and a hash per deployment path.
    #[test]
    fn the_shell_carries_no_inline_script() {
        let html = shell("/c/abc/screen", "/web", "Home", "<h1>hello</h1>");
        let scripts: Vec<&str> = html.split("<script").skip(1).collect();
        assert_eq!(
            scripts.len(),
            3,
            "two bundles and the boot; shell was:\n{html}"
        );
        for tag in scripts {
            let open = tag.split('>').next().unwrap_or_default();
            assert!(
                open.contains(" src=\""),
                "a script without src is inline script; shell was:\n{html}"
            );
        }
        assert!(
            !html.contains("LiveSocket"),
            "the boot is a file, not text in the page; shell was:\n{html}"
        );
        // The boot comes after both bundles: it names `LiveView` and `Phoenix`.
        let at = |needle: &str| html.find(needle).expect(needle);
        assert!(at("@client/phoenix_live_view.min.js") < at("@client/boot.js"));
        assert!(at("@client/phoenix.min.js") < at("@client/boot.js"));
    }

    /// The identity header is read only when an operator named one (O-P-4).
    #[test]
    fn an_identity_is_read_only_from_the_header_the_operator_named() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-user", "alex".parse().expect("a header"));
        assert_eq!(
            identity_of(&h, "X-Forwarded-User").as_deref(),
            Some("alex"),
            "the lookup is case-insensitive, as HTTP header names are"
        );
        assert_eq!(identity_of(&h, ""), None, "no param, no identity");
        assert_eq!(identity_of(&h, "X-Other"), None, "a header nobody sent");
        let mut empty = HeaderMap::new();
        empty.insert("x-forwarded-user", "".parse().expect("a header"));
        assert_eq!(
            identity_of(&empty, "X-Forwarded-User"),
            None,
            "an empty value names nobody"
        );
    }

    /// GH #833: the same header names the viewer only on a connection from a
    /// trusted proxy; the decision is the connection's, made at the handoff.
    #[test]
    fn an_identity_counts_only_on_a_trusted_connection() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-user", "alex".parse().expect("a header"));
        assert_eq!(
            connection_identity(&h, "X-Forwarded-User", true).as_deref(),
            Some("alex")
        );
        assert_eq!(
            connection_identity(&h, "X-Forwarded-User", false),
            None,
            "a header from outside trusted_proxies names nobody"
        );
    }

    /// Payloads this thread has encoded so far.
    fn encoded() -> u64 {
        ENCODED.with(std::cell::Cell::get)
    }

    /// Every frame text waiting in `rx`.
    fn drain(rx: &mut mpsc::Receiver<Queued>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(q) = rx.try_recv() {
            if let ViewerMsg::Frame(f) = q.msg {
                out.push(f);
            }
        }
        out
    }

    /// GH #1003 T1: ten screens of one route, 200 diffs — each diff is
    /// encoded ONCE, not once per screen, and every screen gets the same bytes.
    /// Before: `frames::push` per viewer, 10 encodings per diff.
    #[tokio::test]
    async fn gh1003_a_frame_is_encoded_once_for_many_viewers() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        let mut rxs = Vec::new();
        for i in 0..10 {
            let (v, rx) = viewer("/", 256);
            viewers.insert(format!("v{i:02}"), v).await;
            rxs.push(rx);
        }
        let mut dirty: HashMap<String, Addressed> = HashMap::new();
        let before = encoded();
        for n in 0..200 {
            push_one(
                diff_push("0", &format!("<i>{n}</i>")),
                &viewers,
                &pages_rx,
                &mut dirty,
                crate::web::params::JOIN_CHUNK_DEFAULT,
            )
            .await;
        }
        assert_eq!(encoded() - before, 200, "one encoding per diff");
        let first = drain(&mut rxs[0]);
        assert_eq!(first.len(), 200);
        assert_eq!(
            first[7],
            meclaw_surface::frames::push(&json!("1"), "lv:c", "diff", json!({"0": "<i>7</i>"})),
            "the frame is the one `frames::push` built"
        );
        for rx in &mut rxs[1..] {
            assert_eq!(drain(rx), first, "every screen gets the same bytes");
        }
    }

    /// GH #1003: the whole tree three screens of one route are owed after a
    /// burst is encoded once for all three, and they get the same bytes.
    #[tokio::test]
    async fn gh1003_a_resync_encodes_the_tree_once_per_route() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = ViewerRegistry::default();
        let mut dirty: HashMap<String, Addressed> = HashMap::new();
        let mut rxs = Vec::new();
        for i in 0..3 {
            let (v, rx) = viewer("/", 4);
            viewers.insert(format!("v{i}"), v).await;
            rxs.push(rx);
        }
        for a in viewers.on_route("/").await {
            dirty.insert(a.id.clone(), a);
        }
        let before = encoded();
        resync(
            &pages_rx,
            &mut dirty,
            crate::web::params::JOIN_CHUNK_DEFAULT,
        );
        assert!(dirty.is_empty(), "all three were given the tree");
        assert_eq!(encoded() - before, 1, "one tree, one encoding");
        let first = drain(&mut rxs[0]);
        assert_eq!(
            payload(&first[0]),
            page_map().get("/").expect("route").packed_tree()
        );
        for rx in &mut rxs[1..] {
            assert_eq!(drain(rx), first, "the same bytes on every screen");
        }
    }

    /// GH #1003 T2, measured once and alone (`#[ignore]`, never in a gate
    /// under a parallel suite — OR-H4-20): the cost of one diff's fan-out at
    /// 1 vs. 10 screens of a route, against the per-viewer encoding before.
    /// The diff is a tranche of 400 figures (~44 KB of JSON), the shape of the
    /// measured 2-D world's pan.
    #[tokio::test]
    #[ignore = "measurement: run alone, numbers go to the report (GH #1003)"]
    async fn gh1003_fan_out_cost_is_flat_in_viewers() {
        let diff: meclaw_core::JsonValue = (0..400)
            .map(|i| {
                (
                    i.to_string(),
                    json!(format!(
                        "<i data-x=\"{i}-17\" class=\"fig\">figure {i:04} round 000017 \u{2014} walking</i>"
                    )),
                )
            })
            .collect::<meclaw_core::serde_json::Map<_, _>>()
            .into();
        const ROUNDS: u32 = 400;
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let mut after = Vec::new();
        let mut before = Vec::new();
        for n in [1usize, 10] {
            let viewers = Arc::new(ViewerRegistry::default());
            let mut rxs = Vec::new();
            for i in 0..n {
                let (v, rx) = viewer("/", 4);
                viewers.insert(format!("v{i:02}"), v).await;
                rxs.push(rx);
            }
            let mut dirty: HashMap<String, Addressed> = HashMap::new();
            let mut spent = std::time::Duration::ZERO;
            for _ in 0..ROUNDS {
                let push = WebReconfig::Push {
                    route: "/".to_string(),
                    diff: diff.clone(),
                    generation: 1,
                };
                let t = std::time::Instant::now();
                push_one(
                    push,
                    &viewers,
                    &pages_rx,
                    &mut dirty,
                    crate::web::params::JOIN_CHUNK_DEFAULT,
                )
                .await;
                spent += t.elapsed();
                for rx in &mut rxs {
                    let _ = drain(rx);
                }
            }
            after.push(spent / ROUNDS);
            // Before GH #1003: one `frames::push` (a whole encoding) per viewer.
            let t = std::time::Instant::now();
            for _ in 0..ROUNDS {
                for _ in 0..n {
                    std::hint::black_box(meclaw_surface::frames::push(
                        &json!("1"),
                        "lv:c",
                        "diff",
                        diff.clone(),
                    ));
                }
            }
            before.push(t.elapsed() / ROUNDS);
        }
        let ratio = after[1].as_secs_f64() / after[0].as_secs_f64();
        let ratio_before = before[1].as_secs_f64() / before[0].as_secs_f64();
        println!(
            "GH1003 fan-out per diff: after 1={:?} 10={:?} ratio={ratio:.2}; \
             before 1={:?} 10={:?} ratio={ratio_before:.2}",
            after[0], after[1], before[0], before[1]
        );
        assert!(ratio <= 1.5, "flat in the number of screens: {ratio:.2}");
    }
}
