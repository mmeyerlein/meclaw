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

/// One viewer, as a fan-out addresses it.
///
/// The `id` is what makes a failed send attributable: a `Sender` is not a key,
/// and the whole point of GH #414's second half is remembering **which** viewer
/// lost a frame. The `route` travels with it because the resync sends that
/// page's whole tree, and the map is keyed by route (see [`fan_out`]).
pub struct Addressed {
    /// The connection id the registry holds this viewer under.
    pub id: String,
    /// Where to send frames.
    pub tx: mpsc::Sender<ViewerMsg>,
    /// The client's join reference, needed to address a server-initiated push.
    pub join_ref: meclaw_core::JsonValue,
    /// The topic this viewer joined.
    pub topic: String,
    /// The route this viewer is looking at.
    pub route: String,
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
}

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
        Self {
            mount,
            identity_header,
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
pub(crate) fn shell(base: &str, cell_path: &str, title: &str, body: &str) -> String {
    let container = session::container_id(cell_path);
    let token = session::mint(cell_path);

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <base href=\"{base}/\">\n\
         <meta name=\"csrf-token\" content=\"{token}\">\n\
         <title>{title}</title>\n\
         <style>{states}</style>\n\
         </head>\n<body>\n\
         <div id=\"{container}\" data-phx-main data-phx-session=\"{token}\" data-phx-static=\"\">\n\
         {body}\n\
         </div>\n\
         <script src=\"{base}/@client/phoenix.min.js\"></script>\n\
         <script src=\"{base}/@client/phoenix_live_view.min.js\"></script>\n\
         <script>\n\
         (function () {{\n\
         var csrf = document.querySelector(\"meta[name=csrf-token]\").content;\n\
         var socket = new LiveView.LiveSocket(\"{base}/live\", Phoenix.Socket, {{\n\
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
async fn get_path(State(io): State<WebIo>, headers: HeaderMap, uri: axum::http::Uri) -> Response {
    serve_path(&io, &headers, uri.path())
}

/// `GET /<mount>/` — the display's own root.
///
/// Its own route because `nest` does not answer the prefix WITH a trailing
/// slash: the nested router would be handed an empty path and match nothing,
/// and `/<mount>/` is exactly the URL a person types and a `<base href>`
/// resolves against. It is the same handler over the route `/`.
async fn get_root(State(io): State<WebIo>, headers: HeaderMap) -> Response {
    serve_path(&io, &headers, "/")
}

/// The body of [`get_path`]: one path, both declared surfaces.
fn serve_path(io: &WebIo, headers: &HeaderMap, path: &str) -> Response {
    // Before the first publish this display has nothing to say about any route,
    // and saying `404` would be claiming it does (GH #395). The window closes on
    // its own; a caller that waits gets the page.
    if !*io.ready.borrow() {
        return starting();
    }

    let pages = io.pages.borrow().clone();
    if let Some(page) = pages.get(path) {
        return (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                // A page is a live render and must never be served from a
                // heuristic browser cache: it carries the client script inline,
                // and a cached copy keeps running a client the cell has long
                // replaced — seen as fixed bugs that will not die in one
                // person's tab. `no-cache` still allows conditional reuse; the
                // response has no validators, so in practice it means "fetch".
                (header::CACHE_CONTROL, "no-cache"),
            ],
            shell(
                &base_of(headers, &io.mount),
                &io.cell_path,
                &page.title,
                &page.rendered_body(),
            ),
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
    let user_id = identity_of(&headers, &io.identity_header);
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

/// Holds a mount for exactly as long as the I/O half that registered it.
///
/// It is the ONE place the name is given back. The ordinary end drops it by
/// hand at the bottom of [`run_io`], so the order against the serving task is
/// a decision rather than a scope; every OTHER end — a panic in the handler
/// half, the `message_timeout` backstop, an abort — drops it too, and that is
/// what the guard is for. An entry left standing would keep taking the
/// listener's connections for a display nobody serves: a page loading into a
/// socket that never answers, which is worse than a `404` from a name nothing
/// holds. The voice cell's guard is the same object for the same reason.
struct MountGuard {
    /// The table the mount stands in.
    surfaces: Arc<meclaw_colony::SurfaceRegistry>,
    /// The name this life registered.
    mount: String,
    /// The token this life registered under. A spent one removes nothing, which
    /// is what makes a respawn's entry safe from the previous life's guard.
    registration: meclaw_colony::Registration,
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
                registration = Some(MountGuard {
                    surfaces: Arc::clone(&surfaces),
                    mount: mount.clone(),
                    registration: held,
                });
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
                            connections.spawn(crate::handed::serve_handed(
                                handed.stream,
                                mounted_router(io.clone()),
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
        _ = fan_out(&mut pushes, &viewers, &pages, &mut dirty) => {}
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
        }
    }
}

/// The tree a viewer of `route` needs to be current again.
fn whole_tree(
    pages: &watch::Receiver<Arc<PageMap>>,
    route: &str,
) -> Option<meclaw_core::JsonValue> {
    pages.borrow().get(route).map(|p| p.packed_tree())
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
) {
    let WebReconfig::Push { route, diff } = push;
    for a in viewers.on_route(&route).await {
        let behind = dirty.contains_key(&a.id);
        let payload = match behind.then(|| whole_tree(pages, &a.route)).flatten() {
            Some(tree) => tree,
            None => diff.clone(),
        };
        // The tree rides BARE on the push lane (GH #413). The `{"diff": ...}`
        // wrapper is the *reply* shape; the client hands a push payload straight
        // to `Rendered.extract`, so a wrapper becomes one junk slot.
        let frame = meclaw_surface::frames::push(&a.join_ref, &a.topic, "diff", payload);
        match a.tx.try_send(ViewerMsg::Frame(frame)) {
            Ok(()) => {
                dirty.remove(&a.id);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
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
fn resync(pages: &watch::Receiver<Arc<PageMap>>, dirty: &mut HashMap<String, Addressed>) {
    dirty.retain(|_, a| {
        let Some(tree) = whole_tree(pages, &a.route) else {
            return false;
        };
        let frame = meclaw_surface::frames::push(&a.join_ref, &a.topic, "diff", tree);
        matches!(
            a.tx.try_send(ViewerMsg::Frame(frame)),
            Err(mpsc::error::TrySendError::Full(_))
        )
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
) {
    loop {
        if dirty.is_empty() {
            match pushes.recv().await {
                Some(push) => push_one(push, viewers, pages, dirty).await,
                None => return,
            }
        } else {
            tokio::select! {
                p = pushes.recv() => match p {
                    Some(push) => push_one(push, viewers, pages, dirty).await,
                    None => return,
                },
                _ = tokio::time::sleep(RESYNC_RETRY) => resync(pages, dirty),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::socket::Viewer;
    use meclaw_core::serde_json::json;

    /// One registered viewer, with a channel the test keeps the receiving end of.
    fn viewer(route: &str, cap: usize) -> (Viewer, mpsc::Receiver<ViewerMsg>) {
        let (tx, rx) = mpsc::channel::<ViewerMsg>(cap);
        (
            Viewer {
                tx,
                route: route.to_string(),
                join_ref: json!("1"),
                topic: "lv:c".to_string(),
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
                slots: vec![("n1".to_string(), "<i>one</i>".to_string())],
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
        }
    }

    /// The whole of GH #414's second half: a viewer whose channel was full does
    /// NOT silently keep a picture the database has left behind. The next frame
    /// it can be sent is the whole tree, never the diff it would have had.
    #[tokio::test]
    async fn a_viewer_that_lost_a_frame_gets_the_tree_and_not_the_next_diff() {
        let (_pages_tx, pages_rx) = watch::channel(page_map());
        let viewers = Arc::new(ViewerRegistry::default());
        // Capacity one, so the second push cannot fit: the burst that fills a
        // 64-slot channel on a live display is the same event, larger.
        let (v, mut rx) = viewer("/", 1);
        viewers.insert("v1".to_string(), v).await;
        let mut dirty: HashMap<String, Addressed> = HashMap::new();

        push_one(diff_push("0", "<i>a</i>"), &viewers, &pages_rx, &mut dirty).await;
        assert!(dirty.is_empty(), "the first frame fits");

        push_one(diff_push("0", "<i>b</i>"), &viewers, &pages_rx, &mut dirty).await;
        assert!(
            dirty.contains_key("v1"),
            "a dropped frame must be remembered, not forgotten"
        );

        // The browser reads its backlog: the slot is free again.
        let ViewerMsg::Frame(first) = rx.recv().await.expect("the first frame") else {
            panic!("a frame, not a close")
        };
        assert_eq!(payload(&first), json!({"0": "<i>a</i>"}));

        push_one(diff_push("0", "<i>c</i>"), &viewers, &pages_rx, &mut dirty).await;
        let ViewerMsg::Frame(second) = rx.recv().await.expect("the second frame") else {
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

        resync(&pages_rx, &mut dirty);
        assert!(
            dirty.contains_key("v1"),
            "a viewer that is still wedged keeps its mark — the alternative is \
             ending on a dropped frame, which is the defect"
        );

        let _ = rx.recv().await.expect("the prefilled frame");
        resync(&pages_rx, &mut dirty);
        assert!(dirty.is_empty(), "and the mark goes when the tree is sent");
        let ViewerMsg::Frame(f) = rx.recv().await.expect("the tree") else {
            panic!("a frame, not a close")
        };
        assert_eq!(
            payload(&f),
            page_map().get("/").expect("route").packed_tree()
        );
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
            fan_out(&mut push_rx, &viewers_for_task, &pages_for_task, &mut dirty).await;
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
            .expect("a frame");
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
            html.contains("src=\"/egon/screen/@client/phoenix.min.js\"")
                && html.contains("src=\"/egon/screen/@client/phoenix_live_view.min.js\""),
            "and so do the bundles; shell was:\n{html}"
        );
        assert!(
            !html.contains("\"/live\"") && !html.contains("\"/@client/"),
            "nothing may be written from the origin root any more; shell was:\n{html}"
        );
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
}
