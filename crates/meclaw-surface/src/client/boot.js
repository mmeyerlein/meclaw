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
(function () {
  var csrf = document.querySelector("meta[name=csrf-token]").content;
  var live = document.querySelector("meta[name=meclaw-live]").content;
  var socket = new LiveView.LiveSocket(live, Phoenix.Socket, {
    params: {_csrf_token: csrf},
    hooks: window.SurfaceHooks || {}
  });
  socket.connect();
  window.SurfaceSocket = socket;
})();
