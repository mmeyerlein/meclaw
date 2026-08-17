//! The dead render: the four things LiveView needs, and nothing else.
//!
//! # Why there is no canvas in here
//!
//! The picture arrives in the join reply, from the cell. What this module writes is
//! **protocol scaffolding** for the vendored client: a csrf meta tag, one container
//! carrying `data-phx-main` / `data-phx-session` / `data-phx-static` and an id, the
//! script tags, and the socket constructor. That is generic — it is the same for a
//! colony graph and for a Gantt chart — which is exactly why it may live in the
//! binary while the drawing may not.
//!
//! The property this buys is worth stating: a page load costs **zero cell calls**
//! and touches no database. So a colony that is wedged still serves the page, and
//! the client then visibly fails to connect — a state a person can read, instead of
//! a blank screen.
//!
//! # The two script owners
//!
//! The bundles come from `/surface/@client/…` (compiled into this binary, version
//! locked to the socket we speak). The surface's own stylesheet and hook script
//! come from `…/@asset/…`, out of its own cell directory. A surface that declares
//! no asset directory simply gets no such tags — never a tag pointing at a 404.
//!
//! `window.SurfaceHooks` is the one name the two halves agree on: this module
//! offers the slot, the surface's own script fills it.

use super::session;
use meclaw_colony::surface::Located;

/// Escape text for an HTML text node or a double-quoted attribute.
///
/// A surface title comes out of a `config.json` that a `code` cell in the same
/// colony can write, so it is untrusted for this purpose.
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

/// Build the dead render for a located surface.
///
/// `url_cell` is the cell path as it appeared in the URL (no leading slash), so
/// every link this page emits stays under the prefix the request arrived on.
pub fn dead_render(located: &Located, url_cell: &str) -> String {
    let title = if located.decl.title.is_empty() {
        located.cell_path.clone()
    } else {
        located.decl.title.clone()
    };
    let hint = if located.decl.boot_hint.is_empty() {
        "connecting …".to_string()
    } else {
        located.decl.boot_hint.clone()
    };
    let container = session::container_id(&located.cell_path);
    let token = session::mint(&located.cell_path);
    let prefix = format!("/surface/{url_cell}");

    // Ruling: the socket hangs under THIS surface's prefix, never colony-globally.
    // The phoenix client appends exactly "/websocket" to what it is handed.
    let socket_url = format!("{prefix}/live");

    let assets = located.decl.assets.as_deref();
    let stylesheet = match assets {
        Some(_) => format!(
            "<link rel=\"stylesheet\" href=\"{}/@asset/surface.css\">",
            esc(&prefix)
        ),
        None => String::new(),
    };
    let hooks = match assets {
        Some(_) => format!(
            "<script src=\"{}/@asset/surface.js\"></script>",
            esc(&prefix)
        ),
        None => String::new(),
    };

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"csrf-token\" content=\"{token}\">\n\
         <title>{title}</title>\n\
         {stylesheet}\n\
         </head>\n<body>\n\
         <div id=\"{container}\" data-phx-main data-phx-session=\"{token}\" data-phx-static=\"\">\n\
         <div class=\"surface-boot\">{hint}</div>\n\
         </div>\n\
         <script src=\"/surface/@client/phoenix.min.js\"></script>\n\
         <script src=\"/surface/@client/phoenix_live_view.min.js\"></script>\n\
         {hooks}\n\
         <script>\n\
         (function () {{\n\
         var csrf = document.querySelector(\"meta[name=csrf-token]\").content;\n\
         var socket = new LiveView.LiveSocket(\"{socket_url}\", Phoenix.Socket, {{\n\
         params: {{_csrf_token: csrf}},\n\
         hooks: window.SurfaceHooks || {{}}\n\
         }});\n\
         socket.connect();\n\
         window.SurfaceSocket = socket;\n\
         }})();\n\
         </script>\n\
         </body>\n</html>\n",
        token = esc(&token),
        title = esc(&title),
        hint = esc(&hint),
        container = esc(&container),
        socket_url = esc(&socket_url),
        stylesheet = stylesheet,
        hooks = hooks,
    )
}
