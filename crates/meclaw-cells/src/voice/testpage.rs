//! The built-in browser test page the `voice` cell serves at `GET /` (R-V9).
//!
//! Four static files, no seed row, no template file: the page is part of the
//! cell the way the wire protocol is. A voice cell that boots on a machine
//! with nothing else installed can be pointed at with a browser and answers
//! with something that speaks its own protocol — press a key, watch partials
//! arrive, hear the synthesis come back. That is the calibration tool the
//! echo provider is for, made visible.
//!
//! Self-contained on purpose: nothing from another origin, no CDN script, no
//! build step. Since `voice@2.3.0` (GH #867) it is four files rather than one
//! string, and every one of them comes from the cell itself: the HTML at `/`,
//! its module script at `test.js`, its style at `test.css` and the
//! `AudioWorklet` that turns microphone floats into PCM16 at `worklet.js`.
//! One string with an inline module script, an inline style and a worklet
//! installed from an object URL did not run behind a proxy that sets a
//! Content-Security-Policy of `script-src 'self'`; four files from the page's
//! own origin do. The unit tests below hold both lines.
//!
//! The four files live in `testpage/` beside this module and are compiled in
//! with `include_str!`, for the reason `meclaw_surface::bundle` gives: the
//! installer puts one binary in place, and a page read from disk would be
//! missing everywhere but on the build host.
//!
//! The page is a diagnostic, not a product: no design ambition, no automatic
//! reconnect, no state it keeps across a reload.

/// The built-in browser test page's HTML.
///
/// Served verbatim as `text/html` from `GET /<mount>/` on the colony's
/// listener. Its three other files are relative links (`test.css`, `test.js`,
/// and from the script `worklet.js`), which is why `GET /<mount>` without the
/// slash answers with a redirect rather than with this page.
pub fn html() -> &'static str {
    PAGE
}

/// The page's module script, served as `GET /<mount>/test.js`.
pub fn script() -> &'static str {
    SCRIPT
}

/// The page's stylesheet, served as `GET /<mount>/test.css`.
pub fn style() -> &'static str {
    STYLE
}

/// The capture worklet, served as `GET /<mount>/worklet.js`.
///
/// Processor `pcm-framer`, option `targetRate`, linear resampling. It is NOT
/// the display microphone's worklet (`@client/display-mic-worklet.js` of the
/// `web` cell): that one is processor `mic`, option `rate`, nearest-sample
/// decimation and a `flush` message, and the two pages rely on the
/// difference. Moving them into files changed no byte of either.
pub fn worklet() -> &'static str {
    WORKLET
}

const PAGE: &str = include_str!("testpage/index.html");
const SCRIPT: &str = include_str!("testpage/test.js");
const STYLE: &str = include_str!("testpage/test.css");
const WORKLET: &str = include_str!("testpage/worklet.js");

#[cfg(test)]
mod tests {
    use super::{html, script, style, worklet};

    /// Every frame type of the wire protocol has to be reachable from the
    /// page, otherwise it is not a test page for *this* protocol.
    #[test]
    fn page_mentions_every_frame_type() {
        let page = script();
        for needle in [
            "hello",
            "partial",
            "turn",
            "speak_start",
            "speak_end",
            "hold",
            "release",
            "cancel",
            "AudioWorklet",
            "'ws?'",
        ] {
            assert!(page.contains(needle), "test page never mentions {needle}");
        }
    }

    /// The socket address is built relative to the page, so the cell survives a
    /// reverse proxy that serves it under a path prefix (`/voice/` and
    /// `/voice/ws`, not `/ws`). Only the scheme is rewritten, because a
    /// WebSocket URL has no relative form for it.
    #[test]
    fn page_builds_a_relative_socket_url() {
        let page = script();
        assert!(
            page.contains("new URL('ws?' + params.toString(), base)"),
            "the socket url must be resolved against the page, not against the host"
        );
        assert!(
            page.contains("if (!base.pathname.endsWith('/')) base.pathname += '/'"),
            "the page's own path is the directory the socket sits in, even when \
             a proxy mounted it without the trailing slash"
        );
        assert!(
            !page.contains("location.host"),
            "an absolute '/ws' leaves the prefix a proxy mounted this page under"
        );
        assert!(
            page.contains("url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'"),
            "the scheme still follows the page's, so a TLS proxy gets wss"
        );
    }

    /// The page must work on a machine with no internet: no foreign script,
    /// no external stylesheet, nothing to fetch but its own files and the
    /// socket itself.
    #[test]
    fn page_is_self_contained() {
        for (name, file) in [
            ("index.html", html()),
            ("test.js", script()),
            ("test.css", style()),
            ("worklet.js", worklet()),
        ] {
            assert!(
                !file.contains("http://"),
                "{name} loads something over http"
            );
            assert!(
                !file.contains("https://"),
                "{name} loads something over https"
            );
            assert!(!file.contains("//cdn"), "{name} names a CDN");
        }
        assert!(
            html().contains("<link rel=\"stylesheet\" href=\"test.css\">"),
            "the one stylesheet is the page's own, relative to it"
        );
    }

    /// GH #867: nothing on the page needs `'unsafe-inline'` or `blob:` in a
    /// proxy's `script-src`. Every `<script>` has a `src`, the style is a file,
    /// and the worklet is loaded by URL rather than from a string.
    #[test]
    fn page_runs_under_script_src_self() {
        let page = html();
        let scripts: Vec<&str> = page.split("<script").skip(1).collect();
        assert_eq!(scripts.len(), 1, "one module script: {page}");
        for tag in scripts {
            let open = tag.split('>').next().unwrap_or_default();
            assert!(
                open.contains(" src=\"test.js\"") && open.contains("type=\"module\""),
                "the one script is the module file, not inline text: {open}"
            );
        }
        assert!(
            !page.contains("<style"),
            "the style is test.css, not a block"
        );
        for (name, file) in [("index.html", page), ("test.js", script())] {
            assert!(!file.contains("blob:"), "{name} names a blob: URL");
            assert!(
                !file.contains("createObjectURL"),
                "{name} makes an object URL"
            );
            assert!(!file.contains("new Blob"), "{name} builds a Blob");
        }
        assert!(
            script().contains("addModule(new URL('worklet.js', dir).toString())"),
            "the worklet is the file beside the page"
        );
        assert!(
            worklet().contains("registerProcessor('pcm-framer', PcmFramer)"),
            "and that file registers the processor the script builds its node on"
        );
    }
}
