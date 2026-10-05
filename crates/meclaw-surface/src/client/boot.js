// The page shell's boot (GH #867). Not vendored: this file is ours, and it is
// a file rather than an inline <script> so that a page runs under a
// Content-Security-Policy of `script-src 'self'`. The inline block it replaces
// carried the socket path in its text -- proxy prefix plus mount -- so its hash
// was different on every deployment path and no proxy could list it once.
//
// The socket path is read from <meta name="meclaw-live">, which the shell
// writes from the same base as every other link. Not from `document.baseURI`:
// the base is what relative links resolve against, and the socket is a
// separate fact the shell states on its own.
//
// Loaded without `defer`, after the two bundles and after the page body: the
// hook scripts a page carries in its body have run by then and put their hooks
// on `window.SurfaceHooks`, and the socket reads that object exactly once.
//
// GH #1002: <meta name="meclaw-join-timeout"> (the cell's `join_timeout_ms`)
// becomes the socket's `timeout` -- how long the client waits for a join reply
// before it gives up and reconnects. Absent, the option is not set and the
// client keeps its own default (10 s). Measured: a phone on Slow 3G never
// joined a large page, every attempt aborted at 10.2 s.
//
// GH #1003: the screen this page is drawn on. Every screen of a member shows
// the same content; what differs per screen class is layout -- window
// orientation, menu arrangement (display-hive section 6). The class is the
// route the page was opened on; the join tells the cell what the client
// measured as `params._screen` = {w, h, dpr, orientation, coarse}, which the
// cell hands to an app that opted in (`viewer:screen`). A turn of the phone is
// CSS only: `data-orientation` and `data-coarse` on <html> follow the viewport
// (debounced 150 ms) and nothing is sent -- a server round trip per turn would
// make the content depend on the screen.
(function () {
  var root = document.documentElement;
  var coarse = function () {
    return !!(window.matchMedia && window.matchMedia("(pointer: coarse)").matches);
  };
  var orientation = function () {
    return window.innerHeight > window.innerWidth ? "portrait" : "landscape";
  };
  var mark = function () {
    root.setAttribute("data-orientation", orientation());
    root.setAttribute("data-coarse", coarse() ? "true" : "false");
  };
  mark();
  var pending = null;
  var later = function () {
    if (pending) { clearTimeout(pending); }
    pending = setTimeout(function () { pending = null; mark(); }, 150);
  };
  window.addEventListener("resize", later);
  window.addEventListener("orientationchange", later);

  var csrf = document.querySelector("meta[name=csrf-token]").content;
  var live = document.querySelector("meta[name=meclaw-live]").content;
  var opts = {
    params: {
      _csrf_token: csrf,
      _screen: {
        w: Math.round(window.innerWidth),
        h: Math.round(window.innerHeight),
        dpr: window.devicePixelRatio || 1,
        orientation: orientation(),
        coarse: coarse()
      }
    },
    hooks: window.SurfaceHooks || {}
  };
  var join = document.querySelector("meta[name=meclaw-join-timeout]");
  var ms = join ? parseInt(join.content, 10) : NaN;
  if (ms > 0) { opts.timeout = ms; }
  var socket = new LiveView.LiveSocket(live, Phoenix.Socket, opts);
  socket.connect();
  window.SurfaceSocket = socket;
})();
