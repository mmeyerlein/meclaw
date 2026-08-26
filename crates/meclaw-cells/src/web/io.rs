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
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use meclaw_surface::{bundle, session};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

use crate::web::assets::{Asset, AssetMap};
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
/// read only to fan a frame out. The handler never touches it — it publishes
/// pages through the `watch` channel and asks for a push through the reconfig
/// channel, and this half does the addressing.
///
/// The alternative would be a third task whose only job is to own a `HashMap`
/// and answer over a channel. That is the same lock with more moving parts, and
/// every critical section here is an insert, a remove or a clone of a sender
/// list — no `.await` is held across any of them.
#[derive(Default)]
pub struct ViewerRegistry {
    inner: Mutex<HashMap<String, Viewer>>,
}

impl ViewerRegistry {
    /// Register a viewer under its connection id.
    pub async fn insert(&self, id: String, viewer: Viewer) {
        self.inner.lock().await.insert(id, viewer);
    }

    /// Forget a viewer whose connection ended.
    pub async fn remove(&self, id: &str) {
        self.inner.lock().await.remove(id);
    }

    /// Every viewer currently looking at `route`, as `(sender, join_ref, topic)`.
    ///
    /// Returns clones so the caller can send without holding the lock.
    pub async fn on_route(
        &self,
        route: &str,
    ) -> Vec<(mpsc::Sender<ViewerMsg>, meclaw_core::JsonValue, String)> {
        self.inner
            .lock()
            .await
            .values()
            .filter(|v| v.route == route)
            .map(|v| (v.tx.clone(), v.join_ref.clone(), v.topic.clone()))
            .collect()
    }

    /// Forget every viewer and hand back their senders (GH #410).
    ///
    /// One critical section, and the registry is empty when it ends: a viewer
    /// that joined on a listener which no longer exists must not be found by a
    /// later fan-out. The connection tasks close on their own once they read
    /// the [`ViewerMsg::Close`] the caller sends, and their own `remove` then
    /// finds nothing — which is the harmless order, unlike removing after the
    /// close and racing a re-join against it.
    pub async fn drain(&self) -> Vec<mpsc::Sender<ViewerMsg>> {
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
/// api-side `Dispatcher` follows when it keeps its cache inside one task and
/// answers over a channel.
#[derive(Clone)]
pub struct WebIo {
    /// The address to bind.
    pub bind: String,
    /// The port this instance owns.
    pub port: u16,
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
}

impl WebIo {
    /// Build the I/O state for a cell at `cell_path`.
    pub fn new(
        bind: String,
        port: u16,
        cell_path: &str,
        pages: watch::Receiver<Arc<PageMap>>,
        assets: watch::Receiver<Arc<AssetMap>>,
        ready: watch::Receiver<bool>,
        pushes: mpsc::Receiver<WebReconfig>,
    ) -> Self {
        Self {
            bind,
            port,
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

/// The dead render this cell serves, origin-relative.
///
/// `body` is the materialised page, already rendered — so this is a string
/// concatenation and nothing else. The LiveView client attaches to the same
/// markup on connect rather than replacing it, which is what makes the first
/// paint the real page instead of a spinner.
pub(crate) fn shell(cell_path: &str, title: &str, body: &str) -> String {
    let container = session::container_id(cell_path);
    let token = session::mint(cell_path);

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"csrf-token\" content=\"{token}\">\n\
         <title>{title}</title>\n\
         </head>\n<body>\n\
         <div id=\"{container}\" data-phx-main data-phx-session=\"{token}\" data-phx-static=\"\">\n\
         {body}\n\
         </div>\n\
         <script src=\"/@client/phoenix.min.js\"></script>\n\
         <script src=\"/@client/phoenix_live_view.min.js\"></script>\n\
         <script>\n\
         (function () {{\n\
         var csrf = document.querySelector(\"meta[name=csrf-token]\").content;\n\
         var socket = new LiveView.LiveSocket(\"/live\", Phoenix.Socket, {{\n\
         params: {{_csrf_token: csrf}},\n\
         hooks: window.SurfaceHooks || {{}}\n\
         }});\n\
         socket.connect();\n\
         window.SurfaceSocket = socket;\n\
         }})();\n\
         </script>\n\
         </body>\n</html>\n",
        token = esc(&token),
        title = esc(title),
        container = esc(&container),
        // Already-rendered HTML: escaped per prop when it was built, by the
        // renderer that knows which props were declared as markup.
        body = body,
    )
}

/// `GET /@client/<file>` — the vendored LiveView bundles, compiled in.
///
/// A closed list rather than a lookup (`meclaw_surface::bundle`): the file name
/// comes from a URL, and a list makes traversal impossible rather than guarded.
async fn get_client(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    match bundle(&file) {
        Some((ctype, body)) => ([(header::CONTENT_TYPE, ctype)], body).into_response(),
        None => miss(),
    }
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
async fn get_path(State(io): State<WebIo>, uri: axum::http::Uri) -> Response {
    // Before the first publish this display has nothing to say about any route,
    // and saying `404` would be claiming it does (GH #395). The window closes on
    // its own; a caller that waits gets the page.
    if !*io.ready.borrow() {
        return starting();
    }
    let path = uri.path();

    let pages = io.pages.borrow().clone();
    if let Some(page) = pages.get(path) {
        return (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            shell(&io.cell_path, &page.title, &page.rendered_body()),
        )
            .into_response();
    }

    let assets = io.assets.borrow().clone();
    match assets.get(path) {
        Some(asset) => asset_response(asset),
        None => miss(),
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
fn asset_response(asset: &Asset) -> Response {
    let value = header::HeaderValue::from_str(&asset.content_type).unwrap_or_else(|_| {
        tracing::warn!(
            content_type = %asset.content_type,
            "web: asset content_type is not a legal header value — serving it unlabelled"
        );
        header::HeaderValue::from_static("application/octet-stream")
    });
    // `Vec<u8>` answers as `application/octet-stream`; the row's type replaces
    // that rather than joining it, so a file has exactly one content type.
    let mut resp = asset.body.clone().into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, value);
    resp
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

/// `GET /live/websocket` — the LiveView transport.
///
/// The phoenix client appends exactly `/websocket` to the socket URL it is
/// handed, so the shell says `/live` and the route is this. A plain GET here is
/// a 400 rather than a 404: the path is right, the request is not.
async fn get_socket(State(io): State<WebIo>, upgrade: Option<WebSocketUpgrade>) -> Response {
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
    up.on_upgrade(move |ws| run_connection(ws, io, events_tx, viewers))
}

/// The cell's router.
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

/// What ended one serving round.
enum Round {
    /// The params moved (GH #410): serve this address next. Carried as an
    /// address rather than as an open listener because the old socket is only
    /// released when the round's `serve` future is dropped, and the new one is
    /// bound after that.
    Rebind {
        /// The address to bind.
        bind: String,
        /// The port to bind.
        port: u16,
        /// Where the handler is waiting for the verdict.
        ack: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// The cell is going away.
    Done,
}

/// The I/O loop: bind, serve, and stay up for the cell's whole life.
///
/// **A1′**: this function must not return voluntarily while the cell is live.
/// A clean early return would silence the I/O side while the handler keeps
/// running and open the "io-finish-first" loss class the trait documents. Only
/// the shutdown signal — the handler closing the reconfig channel — ends it.
///
/// # Why this is a loop (GH #410)
///
/// A display moves to another address by being told to, not by being rebuilt.
/// One iteration is one listening address; a `Rebind` on the reconfig channel
/// ends the iteration and the next one begins on the new socket, with the same
/// router over the same published snapshots — so the pages, the files and the
/// readiness seam are literally the same objects before and after the move, and
/// the GH #395 window cannot reopen: `ready` was published long before and is
/// never taken back.
///
/// A round with **no** listener is an ordinary round. That is what makes a
/// failed bind recoverable by message rather than only by restart: a display
/// whose port was taken at boot keeps its task, keeps draining the diffs its
/// handler produces, and starts serving the moment an update names an address
/// it can have. Before this it parked until shutdown, and a port collision cost
/// a restart to fix.
pub async fn run_io(
    io: WebIo,
    events_tx: mpsc::Sender<WebEvent>,
    mut reconfig_rx: mpsc::Receiver<WebReconfig>,
) {
    // The listener half learns where to send browser events only here, because
    // this is where the channel exists.
    let mut io = io;
    io.events_tx = Some(events_tx.clone());
    let viewers = io.viewers.clone();
    let mut pushes = io
        .pushes
        .lock()
        .await
        .take()
        .expect("run_io takes the push receiver exactly once");

    let mut listener = match bind_addr(&io.bind, io.port).await {
        Ok(l) => Some(l),
        Err(e) => {
            let _ = events_tx.send(WebEvent::BindFailed(e)).await;
            None
        }
    };

    loop {
        let round = match listener.take() {
            Some(l) => {
                let bound = l
                    .local_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| "unknown".to_string());
                let _ = events_tx.send(WebEvent::Bound(bound)).await;

                // The scope matters. `serve` owns the listener, and the socket
                // is only released when `serve` is dropped — which happens at
                // the end of this block, BEFORE the next address is bound.
                // Binding while the old socket is still open would fail for the
                // ordinary move (`0.0.0.0:P` collides with `127.0.0.1:P`), so
                // the order is not an optimisation.
                //
                // `IntoFuture` rather than the `Serve` value: pinning it here is
                // what lets the round end without ending the task, and what
                // makes the socket's release a point in the code rather than a
                // guess about when a `select!` drops its arms.
                let serve =
                    std::future::IntoFuture::into_future(axum::serve(l, router(io.clone())));
                tokio::pin!(serve);
                // Three things end a round, and only one of them continues the
                // cell: the server stopping, the handler closing the reconfig
                // channel (the shutdown signal), or a `Rebind`.
                tokio::select! {
                    _ = &mut serve => Round::Done,
                    r = next_round(&mut reconfig_rx) => r,
                    _ = fan_out(&mut pushes, &viewers) => Round::Done,
                }
            }
            // Nothing to serve on, and still not a reason to return. The diffs
            // keep being drained: the handler pushes one per write whether or
            // not anybody can see them, and a receiver nobody reads would fill
            // and block the only writer of the `cell.db`.
            None => tokio::select! {
                r = next_round(&mut reconfig_rx) => r,
                _ = fan_out(&mut pushes, &viewers) => Round::Done,
            },
        };

        let Round::Rebind { bind, port, ack } = round else {
            return;
        };

        // The old socket is closed at this point, so this is the first moment
        // the new address can be bound.
        let attempt = bind_addr(&bind, port).await;
        // Answered before anything else, and deliberately: the handler is
        // parked on this oneshot and drains no events while it waits, so the
        // `Bound` line above must not be able to reach a full events channel
        // ahead of the verdict.
        let _ = ack.send(attempt.as_ref().map(|_| ()).map_err(String::clone));
        listener = match attempt {
            Ok(l) => {
                io.bind = bind;
                io.port = port;
                // Every joined viewer was accepted on a socket that no longer
                // exists — the connection tasks outlive the listener that
                // accepted them, so this is a decision and not a consequence.
                // Dropping them is the honest state: the client reconnects
                // against the address its page now resolves to, and a registry
                // still naming them would fan diffs at connections nobody can
                // reach. Only on a real move: a failed one left the display
                // exactly where its viewers are looking.
                for tx in viewers.drain().await {
                    let _ = tx.try_send(ViewerMsg::Close);
                }
                Some(l)
            }
            // The value passed the parser and still cannot be a listening
            // address. The handler has the verdict already and will refuse the
            // update to whoever sent it; this half's job is to put the display
            // back where it was, so a typo costs a moment of downtime rather
            // than the listener. If even that address is gone now, the next
            // round is a listener-less one — reachable, and still movable.
            Err(e) => {
                let _ = events_tx.send(WebEvent::BindFailed(e)).await;
                match bind_addr(&io.bind, io.port).await {
                    Ok(l) => Some(l),
                    Err(e) => {
                        let _ = events_tx.send(WebEvent::BindFailed(e)).await;
                        None
                    }
                }
            }
        };
    }
}

/// Bind one address, with the failure text an operator can act on.
async fn bind_addr(addr: &str, port: u16) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind((addr, port))
        .await
        .map_err(|e| format!("{addr}:{port}: {e}"))
}

/// Wait for whatever ends this serving round on the reconfig channel.
///
/// A `Rebind` closes nothing here — the old listener is still open, owned by
/// the `serve` future in the caller's scope. The **new** address is bound after
/// this returns, which is why a failure to bind it can still fall back.
async fn next_round(reconfig_rx: &mut mpsc::Receiver<WebReconfig>) -> Round {
    loop {
        match reconfig_rx.recv().await {
            // The handler is gone when this channel closes.
            None => return Round::Done,
            Some(WebReconfig::Rebind { bind, port, ack }) => {
                return Round::Rebind { bind, port, ack };
            }
            // Diffs travel on the cell's own push channel; one arriving here
            // would mean a caller outside this cell built the wiring.
            Some(WebReconfig::Push { route, .. }) => tracing::warn!(
                %route,
                "web: a diff arrived on the reconfig channel and was dropped"
            ),
        }
    }
}

/// Fan every `Push` the handler sends out to the viewers of its route.
async fn fan_out(pushes: &mut mpsc::Receiver<WebReconfig>, viewers: &Arc<ViewerRegistry>) {
    while let Some(push) = pushes.recv().await {
        match push {
            WebReconfig::Push { route, diff } => {
                for (tx, join_ref, topic) in viewers.on_route(&route).await {
                    // The tree rides BARE on the push lane (GH #413). The
                    // `{"diff": ...}` wrapper is the *reply* shape; the client
                    // hands a push payload straight to `Rendered.extract`, so a
                    // wrapper becomes one junk slot and the re-render restores
                    // the old markup — the drag's spring-back.
                    let frame =
                        meclaw_surface::frames::push(&join_ref, &topic, "diff", diff.clone());
                    // A full or closed viewer channel means that browser is
                    // gone or wedged; its connection task cleans up the registry
                    // entry. One slow viewer must not hold up the others.
                    let _ = tx.try_send(ViewerMsg::Frame(frame));
                }
            }
            // Rebinds travel on the substrate's reconfig channel, which is the
            // only one the handler sends them on.
            WebReconfig::Rebind { .. } => {
                tracing::warn!("web: a rebind arrived on the push channel and was dropped")
            }
        }
    }
}
