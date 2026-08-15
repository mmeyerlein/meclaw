//! Phase-7 WebFetchCell. Stateless HTTP-GET tool cell.

use std::time::Duration;

use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_TIMEOUT, build_error_body, build_tool_result_body,
    parse_tool_call_args, with_external_timeout,
};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

/// Stateless HTTP-GET cell.
pub struct WebFetchCell {
    /// reqwest client (Arc internally, no mutex needed).
    pub client: reqwest::Client,
    /// External-timeout pro Roundtrip (send + bytes).
    pub external_timeout: Duration,
    /// Max number of workers running in parallel for this cell.
    pub max_concurrency: usize,
    /// Size cap on the response body handed to the caller, in bytes (GH #83).
    ///
    /// A fetched body is a tool result, and inside a tool loop a tool result is
    /// re-sent to the model on every subsequent round — one large fetch is not a
    /// one-time cost, it is a per-round one. The cap is generous by default and
    /// finite always; a trim is marked, never silent.
    pub max_bytes: usize,
    /// Opt-out of the private-network deny (GH #117). Default `false`.
    ///
    /// `true` disables the deny ENTIRELY — the cell may then reach loopback,
    /// the RFC-1918 blocks and the cloud metadata endpoint. It exists for
    /// topologies whose targets genuinely are local (a mock server in a test, a
    /// service on the same host) and for nothing else.
    pub allow_private_networks: bool,
    /// How many redirect hops the cell follows before giving up (GH #117).
    ///
    /// Redirects are followed by the cell itself, not by reqwest, because every
    /// hop has to pass the deny again — the classic bypass is a public URL that
    /// 302s to `169.254.169.254`.
    pub max_redirects: usize,
}

/// `error_code` for a target the egress policy refuses (GH #117).
///
/// Its own lane, deliberately. `io_error` would read as "network problem,
/// retry" and `invalid_input` as "the URL is malformed" — both send an agent
/// repairing its own call down the wrong road. A blocked target is neither
/// broken syntax nor a flaky network: the call named somewhere it may not go,
/// and the only repair is a different target.
pub(crate) const ERR_TARGET_BLOCKED: &str = "target_blocked";

/// `error_code` for a redirect chain that outran `max_redirects` (GH #117).
pub(crate) const ERR_TOO_MANY_REDIRECTS: &str = "too_many_redirects";

/// `error_code` for a `Location` this cell refuses to follow (GH #117).
pub(crate) const ERR_INVALID_REDIRECT: &str = "invalid_redirect";

/// Cap `text` at `max_bytes`, cutting on a UTF-8 char boundary and appending an
/// explicit marker with the original byte length (GH #83).
///
/// Same visible-truncation convention the `code` cell uses for stderr: a
/// silently shortened payload is worse than a marked one, because the model
/// cannot tell that it is reasoning about a prefix.
pub(crate) fn truncate_body(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let full = text.len();
    (
        format!("{}… [truncated, {full} bytes total]", &text[..end]),
        true,
    )
}

#[derive(Debug)]
pub(crate) struct WebFetchArgs {
    /// The url exactly as the caller wrote it (never re-serialised).
    pub url: String,
    /// The same url, parsed — the form the deny and the redirect loop work on.
    pub target: reqwest::Url,
}

/// Parses tool_call args for WebFetchCell.
///
/// GH #110: the gate PARSES the URL, it does not merely check presence and
/// type. A syntax error used to surface from reqwest and was mapped to
/// `io_error` — which an agent repairing its own call reads as "network
/// problem, retry", the wrong repair applied forever. A broken URL is a broken
/// CALL, so it belongs on the `invalid_input` lane with the missing one.
pub(crate) fn parse_web_fetch_args(args: &meclaw_core::JsonValue) -> Result<WebFetchArgs, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.url missing or not a string".to_string())?;
    if url.is_empty() {
        return Err("args.url is empty".into());
    }
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| format!("args.url {url:?} is not a valid URL: {e}"))?;
    // A URL this cell cannot speak is the same class of mistake as one that
    // does not parse: the call is wrong, not the network. Naming it here also
    // keeps `file://` from ever looking like a transport failure.
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "args.url {url:?} uses scheme {:?}; web_fetch speaks http and https only",
            parsed.scheme()
        ));
    }
    Ok(WebFetchArgs {
        url: url.to_string(),
        target: parsed,
    })
}

// ───── GH #117: the private-network deny ─────
//
// `web_fetch` runs INSIDE the daemon process — no child is spawned, so
// `sandbox.network: "deny"` can never cover it. Whatever egress policy this
// cell enforces, it enforces itself. The floor is a deny for everything that is
// not the open internet: loopback, the RFC-1918 blocks, link-local (which is
// where every cloud provider parks its unauthenticated metadata service at
// 169.254.169.254), and the v6 equivalents.

/// The refusal reason for `ip` under the given policy, or `None` when it may be
/// contacted.
///
/// Two tiers, because the opt-out has a job and a limit. `allow_private_networks
/// = false` (the default) refuses every range that is by definition not
/// reachable from the open internet — which is exactly the set an SSRF payload
/// aims at. `true` opens the ranges a local topology can legitimately live in
/// (loopback, RFC 1918, ULA, CGNAT …) and NOTHING else.
pub(crate) fn deny_reason_for(
    ip: std::net::IpAddr,
    allow_private_networks: bool,
) -> Option<&'static str> {
    if allow_private_networks {
        never_reachable_reason(ip)
    } else {
        deny_reason(ip)
    }
}

/// The default tier: the full deny matrix.
pub(crate) fn deny_reason(ip: std::net::IpAddr) -> Option<&'static str> {
    match canonical(ip) {
        std::net::IpAddr::V4(v4) => deny_reason_v4(v4),
        std::net::IpAddr::V6(v6) => deny_reason_v6(v6),
    }
}

/// The floor under both tiers: link-local, which `allow_private_networks` does
/// NOT open (GH #117 Track-Ruling).
///
/// `169.254.169.254` is the cloud metadata endpoint — an unauthenticated
/// credential vending machine on AWS, GCP and Azure alike. Nothing anybody
/// deliberately runs lives at a link-local address, so "I need local targets"
/// is never a reason to reach one, and a redirect aimed there is an attack in
/// every deployment. The same holds for `fe80::/10`.
fn never_reachable_reason(ip: std::net::IpAddr) -> Option<&'static str> {
    match canonical(ip) {
        std::net::IpAddr::V4(v4) => match v4.octets() {
            [169, 254, ..] => Some("link-local 169.254.0.0/16 (cloud metadata)"),
            _ => None,
        },
        std::net::IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80 => {
            Some("link-local fe80::/10")
        }
        _ => None,
    }
}

/// Reduce the IPv6 forms that EMBED an IPv4 address to that address.
///
/// v4-mapped (`::ffff:a.b.c.d`), the deprecated v4-compatible form
/// (`::a.b.c.d`), NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`) all reach an
/// IPv4 destination. Judging them by the address they carry is what keeps
/// `::ffff:127.0.0.1` from walking past a deny that only knows `127.0.0.1`.
fn canonical(ip: std::net::IpAddr) -> std::net::IpAddr {
    let v6 = match ip {
        std::net::IpAddr::V4(_) => return ip,
        std::net::IpAddr::V6(v6) => v6,
    };
    if v6.is_unspecified() || v6.is_loopback() {
        return ip;
    }
    let seg = v6.segments();
    if let Some(v4) = v6.to_ipv4_mapped() {
        return std::net::IpAddr::V4(v4);
    }
    if seg[0..6] == [0, 0, 0, 0, 0, 0] {
        return std::net::IpAddr::V4(embedded_v4(seg[6], seg[7]));
    }
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        return std::net::IpAddr::V4(embedded_v4(seg[6], seg[7]));
    }
    if seg[0] == 0x2002 {
        return std::net::IpAddr::V4(embedded_v4(seg[1], seg[2]));
    }
    ip
}

/// IPv4 half of [`deny_reason`].
fn deny_reason_v4(ip: std::net::Ipv4Addr) -> Option<&'static str> {
    match ip.octets() {
        [0, ..] => Some("unspecified / this-network 0.0.0.0/8"),
        [10, ..] => Some("private RFC 1918 10.0.0.0/8"),
        [100, b, ..] if (64..128).contains(&b) => {
            Some("shared address space RFC 6598 100.64.0.0/10")
        }
        [127, ..] => Some("loopback 127.0.0.0/8"),
        // 169.254.169.254 lives here: the cloud metadata endpoint, the single
        // most valuable SSRF target there is.
        [169, 254, ..] => Some("link-local 169.254.0.0/16 (cloud metadata)"),
        [172, b, ..] if (16..32).contains(&b) => Some("private RFC 1918 172.16.0.0/12"),
        [192, 0, 0, ..] => Some("IETF protocol assignments 192.0.0.0/24"),
        [192, 168, ..] => Some("private RFC 1918 192.168.0.0/16"),
        [198, 18..=19, ..] => Some("benchmarking 198.18.0.0/15"),
        [224..=239, ..] => Some("multicast 224.0.0.0/4"),
        [240..=255, ..] => Some("reserved / broadcast 240.0.0.0/4"),
        _ => None,
    }
}

/// IPv6 half of [`deny_reason`], for addresses that embed no IPv4 address.
fn deny_reason_v6(ip: std::net::Ipv6Addr) -> Option<&'static str> {
    let seg = ip.segments();
    if ip.is_unspecified() {
        return Some("unspecified ::");
    }
    if ip.is_loopback() {
        return Some("loopback ::1");
    }
    if seg[0] == 0x0100 && seg[1..4] == [0, 0, 0] {
        return Some("discard-only 100::/64");
    }
    if seg[0] & 0xfe00 == 0xfc00 {
        return Some("unique local fc00::/7");
    }
    if seg[0] & 0xffc0 == 0xfe80 {
        return Some("link-local fe80::/10");
    }
    if seg[0] & 0xffc0 == 0xfec0 {
        return Some("deprecated site-local fec0::/10");
    }
    if seg[0] & 0xff00 == 0xff00 {
        return Some("multicast ff00::/8");
    }
    None
}

/// Rebuild the IPv4 address carried in two IPv6 segments (NAT64 / 6to4).
fn embedded_v4(hi: u16, lo: u16) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::new(
        (hi >> 8) as u8,
        (hi & 0xff) as u8,
        (lo >> 8) as u8,
        (lo & 0xff) as u8,
    )
}

/// Why a target did not survive the screen.
///
/// The two halves must not share an `error_code`: one is a policy refusal the
/// caller repairs by naming a different target, the other is a transport fact
/// the caller repairs by retrying or fixing the name.
pub(crate) enum ScreenFailure {
    /// The target resolves into a denied range.
    Denied(String),
    /// The name could not be resolved at all.
    Unresolvable(String),
}

/// Resolve `host` to its addresses via the system resolver.
///
/// `getaddrinfo` is blocking, so it runs on the blocking pool — the same call
/// reqwest's own default resolver makes, just visible to us. No new dependency:
/// `std::net::ToSocketAddrs` plus `spawn_blocking` is the whole mechanism.
async fn resolve_host(host: &str) -> Result<Vec<std::net::IpAddr>, String> {
    let owned = host.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        (owned.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|sa| sa.ip()).collect::<Vec<_>>())
    })
    .await;
    match joined {
        Err(e) => Err(format!("dns lookup task for {host:?} failed: {e}")),
        Ok(Err(e)) => Err(format!("dns lookup for {host:?} failed: {e}")),
        Ok(Ok(ips)) if ips.is_empty() => {
            Err(format!("dns lookup for {host:?} returned no address"))
        }
        Ok(Ok(ips)) => Ok(ips),
    }
}

/// Screen every address `host` resolves to against [`deny_reason`].
///
/// Deny-if-ANY, not deny-if-all: a name whose address set mixes a public and a
/// private address is exactly the shape a rebinding attacker hands you, and
/// there is no honest reason to pick the public one and hope.
async fn screen_host(host: &str, allow_private_networks: bool) -> Result<(), ScreenFailure> {
    // An IP literal never reaches a resolver — neither ours nor reqwest's — so
    // it has to be judged here or not at all. The URL parser has already
    // normalised the decimal/octal/hex spellings (`2130706433`, `0177.0.0.1`)
    // into dotted-quad form, so the deny sees the address, not the disguise.
    let bare = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return match deny_reason_for(ip, allow_private_networks) {
            Some(reason) => Err(ScreenFailure::Denied(format!(
                "web_fetch refuses {ip}: {reason}"
            ))),
            None => Ok(()),
        };
    }
    let ips = resolve_host(host)
        .await
        .map_err(ScreenFailure::Unresolvable)?;
    for ip in &ips {
        if let Some(reason) = deny_reason_for(*ip, allow_private_networks) {
            return Err(ScreenFailure::Denied(format!(
                "web_fetch refuses {host:?}: it resolves to {ip} ({reason})"
            )));
        }
    }
    Ok(())
}

/// Screen the target of `url`.
async fn screen_url(url: &reqwest::Url, allow_private_networks: bool) -> Result<(), ScreenFailure> {
    match url.host_str() {
        Some(h) => screen_host(h, allow_private_networks).await,
        // Unreachable through the gate (`parse_web_fetch_args` rejects a
        // host-less URL), but the screen does not lean on a caller's care.
        None => Err(ScreenFailure::Denied(format!(
            "web_fetch refuses {url}: the URL names no host"
        ))),
    }
}

/// reqwest DNS resolver that never hands the connector a denied address.
///
/// The pre-flight [`screen_url`] produces the readable refusal; THIS is what
/// makes the refusal true. Between the screen and the connect there is a second
/// resolution, and a DNS-rebinding attacker owns exactly that window: answer
/// public once, private once. Because reqwest connects only to addresses this
/// resolver returns, the address that was checked is the address that is dialled.
pub(crate) struct SsrfGuardResolver {
    /// The policy tier this resolver enforces.
    allow_private_networks: bool,
}

impl reqwest::dns::Resolve for SsrfGuardResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let allow_private_networks = self.allow_private_networks;
        Box::pin(async move {
            let ips = resolve_host(&host)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            for ip in &ips {
                if let Some(reason) = deny_reason_for(*ip, allow_private_networks) {
                    let msg = format!("web_fetch refuses {host:?}: it resolves to {ip} ({reason})");
                    return Err(msg.into());
                }
            }
            let addrs: reqwest::dns::Addrs =
                Box::new(ips.into_iter().map(|ip| std::net::SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// Why a fetch did not produce a response.
///
/// Each variant owns an `error_code`, because each names a different repair:
/// pick another target, stop looping, fix the server's `Location`, or retry the
/// network.
pub(crate) enum FetchFailure {
    /// The egress policy refused the target (initial URL or any hop).
    Blocked(String),
    /// The redirect chain outran `max_redirects`.
    TooManyRedirects(String),
    /// A `Location` this cell will not follow.
    InvalidRedirect(String),
    /// DNS, connect, TLS, read — the ordinary transport failures.
    Transport(String),
}

impl FetchFailure {
    /// The `error_code` this failure travels under.
    fn code(&self) -> &'static str {
        match self {
            FetchFailure::Blocked(_) => ERR_TARGET_BLOCKED,
            FetchFailure::TooManyRedirects(_) => ERR_TOO_MANY_REDIRECTS,
            FetchFailure::InvalidRedirect(_) => ERR_INVALID_REDIRECT,
            FetchFailure::Transport(_) => ERR_IO_ERROR,
        }
    }

    /// The human-readable text of this failure.
    fn into_text(self) -> String {
        match self {
            FetchFailure::Blocked(t)
            | FetchFailure::TooManyRedirects(t)
            | FetchFailure::InvalidRedirect(t)
            | FetchFailure::Transport(t) => t,
        }
    }
}

impl From<ScreenFailure> for FetchFailure {
    fn from(f: ScreenFailure) -> Self {
        match f {
            ScreenFailure::Denied(t) => FetchFailure::Blocked(t),
            ScreenFailure::Unresolvable(t) => FetchFailure::Transport(t),
        }
    }
}

/// One completed fetch: status, content type, body bytes, hops followed and the
/// url the body actually came from.
pub(crate) struct FetchOutcome {
    /// HTTP status of the final response.
    pub status: u16,
    /// `Content-Type` of the final response, empty when absent.
    pub content_type: String,
    /// Body bytes of the final response.
    pub body: Vec<u8>,
    /// How many redirects were followed to get here.
    pub redirects: usize,
    /// The url of the final response — differs from the request only after a
    /// redirect.
    pub final_url: String,
}

/// Decide what to do with a `Location` header (GH #117).
///
/// Resolves the (possibly relative) target against `current` and refuses the
/// two shapes this cell will not follow: a scheme it cannot speak, and a
/// downgrade from `https` to `http`.
fn next_hop(current: &reqwest::Url, location: &str) -> Result<reqwest::Url, FetchFailure> {
    let next = current.join(location).map_err(|e| {
        FetchFailure::InvalidRedirect(format!(
            "web_fetch cannot follow the redirect from {current} to {location:?}: {e}"
        ))
    })?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(FetchFailure::InvalidRedirect(format!(
            "web_fetch refuses the redirect from {current} to {next}: scheme {:?} is not http or https",
            next.scheme()
        )));
    }
    // Track-Ruling: an https→http hop is refused. A redirect the caller never
    // saw must not be able to move the fetch onto the clear, where whoever sits
    // on the path picks both the response and the next hop. The upgrade
    // direction (http→https) stays allowed — it is the common canonical-host
    // redirect and it only ever adds protection.
    if current.scheme() == "https" && next.scheme() == "http" {
        return Err(FetchFailure::InvalidRedirect(format!(
            "web_fetch refuses the redirect from {current} to {next}: https must not be downgraded to http"
        )));
    }
    Ok(next)
}

/// GET `target`, following up to `max_redirects` hops (GH #117).
///
/// The loop is the point. reqwest's own redirect follower runs a SYNCHRONOUS
/// policy closure, which cannot resolve a name — so a chain it follows would
/// pass the deny exactly once, at the URL the caller wrote. That is the classic
/// bypass: a perfectly public URL that answers `302 Location:
/// http://169.254.169.254/`. Here every hop, including the first, is screened
/// before a connection to it is opened.
async fn fetch_following_redirects(
    client: &reqwest::Client,
    target: reqwest::Url,
    allow_private_networks: bool,
    max_redirects: usize,
) -> Result<FetchOutcome, FetchFailure> {
    let mut current = target;
    let mut redirects = 0usize;
    loop {
        screen_url(&current, allow_private_networks)
            .await
            .map_err(FetchFailure::from)?;
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| FetchFailure::Transport(e.to_string()))?;
        let status = resp.status();

        // A 3xx WITHOUT a Location is not a redirect, it is a document with a
        // 3xx status — and a status is data for the caller (the cell's standing
        // convention), so it goes back as a normal result.
        let location = if status.is_redirection() {
            resp.headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        } else {
            None
        };

        if let Some(location) = location {
            if redirects >= max_redirects {
                return Err(FetchFailure::TooManyRedirects(format!(
                    "web_fetch stopped at {current}: the redirect chain exceeds max_redirects = {max_redirects}"
                )));
            }
            current = next_hop(&current, &location)?;
            redirects += 1;
            continue;
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp
            .bytes()
            .await
            .map_err(|e| FetchFailure::Transport(e.to_string()))?
            .to_vec();
        return Ok(FetchOutcome {
            status: status.as_u16(),
            content_type,
            body,
            redirects,
            final_url: current.to_string(),
        });
    }
}

/// Build the cell's reqwest client (GH #117).
///
/// Two settings carry the hardening: the guard resolver (unless the deny is
/// opted out of) and `redirect::Policy::none()`. Redirects are followed by the
/// cell, hop by hop, because reqwest's own follower cannot re-run an async
/// screen between hops — and an unscreened hop is the entire bypass.
pub(crate) fn build_web_fetch_client(
    allow_private_networks: bool,
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(SsrfGuardResolver {
            allow_private_networks,
        }))
        .build()
        .map_err(|e| format!("reqwest client build failed: {e}"))
}

#[allow(clippy::manual_async_fn)]
impl meclaw_colony::StatelessCell for WebFetchCell {
    /// Handle one tool_call message: parse `{url}`, issue HTTP GET via
    /// the shared `reqwest::Client`, wrap the whole roundtrip in
    /// `with_external_timeout`, emit a `tool_result` with
    /// `http_status`/`content_type`/`bytes`-Headers. Non-2xx HTTP status
    /// codes are NORMAL tool_results; only DNS/connect/timeout produce
    /// error messages.
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            let (args, id) = match parse_tool_call_args(&msg) {
                Ok(v) => v,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, None, started)
                        .await;
                    return;
                }
            };
            let parsed = match parse_web_fetch_args(&args) {
                Ok(p) => p,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, id, started)
                        .await;
                    return;
                }
            };

            let client = self.client.clone();
            // The caller's string is what the LOG says; the parsed form is what
            // the deny and the redirect loop work on. reqwest parses the string
            // identically on its way to the wire, so the GH #110 "verbatim"
            // property is unchanged — only the carrier is.
            let requested = parsed.url.clone();
            let target = parsed.target.clone();
            let allow_private = self.allow_private_networks;
            let max_redirects = self.max_redirects;
            // The WHOLE chain — every screen, every hop — lives inside the one
            // operation timeout (rule 12 concept A). A redirect budget is not a
            // time budget; without this, `max_redirects` hops against a slow
            // server would multiply the wait.
            let result = with_external_timeout(self.external_timeout, async move {
                fetch_following_redirects(&client, target, allow_private, max_redirects).await
            })
            .await;

            let duration_ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(Ok(FetchOutcome {
                    status,
                    content_type,
                    body,
                    redirects,
                    final_url,
                })) => {
                    let text = String::from_utf8_lossy(&body).into_owned();
                    // GH #83: `bytes` reports what the server sent, so a trimmed
                    // reply still says how big the document really was.
                    let bytes_len = text.len() as u64;
                    let (text, truncated) = truncate_body(text, self.max_bytes);
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("web_fetch".into()));
                    header.insert("http_status".into(), Value::from(status));
                    header.insert("content_type".into(), Value::String(content_type));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    header.insert("bytes".into(), Value::from(bytes_len));
                    if truncated {
                        header.insert("truncated".into(), Value::Bool(true));
                    }
                    // GH #117: a body that came from somewhere other than the
                    // requested url says so. A redirect the caller cannot see is
                    // a redirect the caller cannot judge.
                    if redirects > 0 {
                        header.insert("redirects".into(), Value::from(redirects as u64));
                        header.insert("final_url".into(), Value::String(final_url.clone()));
                    }
                    let body_json = build_tool_result_body(text, id, header);
                    tracing::info!(
                        operation = "web_fetch",
                        http_status = status,
                        duration_ms,
                        bytes = bytes_len,
                        truncated,
                        redirects,
                        url = %requested,
                        "web_fetch ok"
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body_json,
                        })
                        .await;
                }
                Ok(Err(failure)) => {
                    let code = failure.code();
                    self.emit_error(sink, reply_target, code, failure.into_text(), id, started)
                        .await;
                }
                Err(_timeout_msg) => {
                    self.emit_error(
                        sink,
                        reply_target,
                        ERR_TIMEOUT,
                        format!("web_fetch timed out after {:?}", self.external_timeout),
                        id,
                        started,
                    )
                    .await;
                }
            }
        }
    }
}

impl WebFetchCell {
    /// Emit a UBF error-body to `reply_target` with `operation: "web_fetch"`
    /// and the given `code`/`text`.
    async fn emit_error(
        &self,
        sink: &OutputSink,
        reply_target: Path,
        code: &str,
        text: String,
        id: Option<String>,
        started: std::time::Instant,
    ) {
        let duration_ms = started.elapsed().as_millis() as u64;
        let mut header = Map::new();
        header.insert("operation".into(), Value::String("web_fetch".into()));
        header.insert("duration_ms".into(), Value::from(duration_ms));
        let body = build_error_body(code, text, id, header);
        tracing::info!(
            operation = "web_fetch",
            error_code = code,
            duration_ms,
            "web_fetch err"
        );
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

// ---- WebFetchCellFactory ----

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};
use std::sync::Arc;

/// Factory for `WebFetchCell`. Unit struct — stateless, config lives in params.
pub struct WebFetchCellFactory;

const DEFAULT_WEB_FETCH_MAX_CONCURRENCY: usize = 32;
const DEFAULT_WEB_FETCH_EXTERNAL_TIMEOUT_MS: u64 = 30_000;
/// Default body cap: 256 KiB (GH #83).
///
/// Generous but finite. It clears an ordinary web page or specification
/// document whole, and it stops a multi-megabyte payload from entering a tool
/// loop, where every subsequent round would pay for it again. A cell inside an
/// agent loop wants a much smaller value; that is what the knob is for.
const DEFAULT_WEB_FETCH_MAX_BYTES: usize = 256 * 1024;
/// Default redirect budget: 5 hops (GH #117).
///
/// Enough for the ordinary shortener / canonical-host / trailing-slash chain,
/// far short of a loop. Every hop is re-screened, so the budget is a cost cap,
/// not the security boundary.
const DEFAULT_WEB_FETCH_MAX_REDIRECTS: usize = 5;

struct ParsedWebFetchParams {
    external_timeout: Duration,
    max_concurrency: usize,
    max_bytes: usize,
    allow_private_networks: bool,
    max_redirects: usize,
}

fn parse_params_pure(raw: &meclaw_core::JsonValue) -> Result<ParsedWebFetchParams, String> {
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_WEB_FETCH_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    let ms = match raw.get("external_timeout_ms") {
        None => DEFAULT_WEB_FETCH_EXTERNAL_TIMEOUT_MS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.external_timeout_ms must be a positive integer".to_string())?,
    };
    if ms == 0 {
        return Err("params.external_timeout_ms must be >= 1".into());
    }
    let mb = match raw.get("max_bytes") {
        None => DEFAULT_WEB_FETCH_MAX_BYTES,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_bytes must be a positive integer".to_string())?
            as usize,
    };
    if mb == 0 {
        return Err("params.max_bytes must be >= 1".into());
    }
    // GH #117: the opt-out has to be WRITTEN to exist. A non-bool value is a
    // reject, never a silent coercion — a config that meant to open the private
    // network and mistyped it must fail loudly, and so must one that meant to
    // keep it shut.
    let apn = match raw.get("allow_private_networks") {
        None => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| "params.allow_private_networks must be a boolean".to_string())?,
    };
    let mr = match raw.get("max_redirects") {
        None => DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_redirects must be a non-negative integer".to_string())?
            as usize,
    };
    Ok(ParsedWebFetchParams {
        external_timeout: Duration::from_millis(ms),
        max_concurrency: mc,
        max_bytes: mb,
        allow_private_networks: apn,
        max_redirects: mr,
    })
}

impl CellFactory for WebFetchCellFactory {
    fn validate_params(&self, params: &meclaw_core::JsonValue) -> Result<(), String> {
        parse_params_pure(params).map(|_| ())
    }

    /// Stateless cell — `cell_dir` and the three Phase-13-G-1 substrate params
    /// (`colony_inbox_tx`, `idle_timeout`, `cell_timeout`) are unused.
    fn spawn_cell(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let parsed = parse_params_pure(&params)?;
        let external_timeout = parsed.external_timeout;
        let max_concurrency = parsed.max_concurrency;
        let max_bytes = parsed.max_bytes;
        let allow_private_networks = parsed.allow_private_networks;
        let max_redirects = parsed.max_redirects;

        // GH #117: the deny lives in the client too (guard resolver), not only
        // in the cell's pre-flight screen. `redirect::Policy::none()` hands the
        // redirect chain to the cell so every hop can be screened.
        let client = build_web_fetch_client(allow_private_networks)?;

        let cell = Arc::new(WebFetchCell {
            client: client.clone(),
            external_timeout,
            max_concurrency,
            max_bytes,
            allow_private_networks,
            max_redirects,
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
        // Phase-13.5 Lifecycle-3b Task 3 + P3-A4 funnel: initial dispatcher via
        // `build_stateless_task` (owns the peace-keep-alive; stateless → no
        // cell.db → death_ack on dispatcher task-end). RespawnFn passes
        // `colony_inbox = None`.
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
            path.clone(),
            rx,
            outputs_tx.clone(),
            cell,
            max_concurrency,
            message_timeout,
            Some(colony_inbox_tx.clone()),
            blob_store.clone(),
            contract.consumes.clone(),
        );

        let respawn_path = path.clone();
        let respawn_outputs_tx = outputs_tx.clone();
        let respawn_client = client.clone(); // Arc clone — no rebuild, no .expect()
        let respawn_blob = blob_store.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            let cell = Arc::new(WebFetchCell {
                client: respawn_client.clone(),
                external_timeout,
                max_concurrency,
                max_bytes,
                allow_private_networks,
                max_redirects,
            });
            let (tx, rx) =
                tokio::sync::mpsc::channel::<meclaw_core::Message>(respawn_mailbox_capacity);
            let p = respawn_path.clone();
            let o = respawn_outputs_tx.clone();
            let b = respawn_blob.clone();
            // Stateless respawn is intentionally bare (no renotify, colony_inbox
            // = None). Dropping stop_tx/death_ack_rx is behaviorally identical to
            // the old bare `None,None,None` spawn (stop-fut parks, death_ack
            // unobserved). Peace-keep-alive lives in the helper.
            let (join, peace_rx, _stop_tx, _death_ack_rx, backstop_rx) = build_stateless_task(
                p,
                rx,
                o,
                cell,
                max_concurrency,
                message_timeout,
                None,
                b,
                respawn_consumes.clone(),
            );
            (tx, join, peace_rx, backstop_rx)
        });

        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    /// Paket-8: boot-inactive eager respawn (No-Delete reconnect-after-reboot).
    /// Builds the SAME `Arc<WebFetchCell>` as `spawn_cell` (incl. the fresh
    /// reqwest client) and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` when
    /// params no longer parse OR the reqwest client fails to build.
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let parsed = parse_params_pure(&params).ok()?;
        let max_concurrency = parsed.max_concurrency;
        let client = build_web_fetch_client(parsed.allow_private_networks).ok()?;
        let cell = Arc::new(WebFetchCell {
            client,
            external_timeout: parsed.external_timeout,
            max_concurrency,
            max_bytes: parsed.max_bytes,
            allow_private_networks: parsed.allow_private_networks,
            max_redirects: parsed.max_redirects,
        });
        Some(meclaw_colony::build_stateless_boot_inactive_respawn(
            path,
            outputs_tx,
            cell,
            max_concurrency,
            message_timeout,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_web_fetch_args_happy_path() {
        let args = parse_web_fetch_args(&json!({"url": "http://127.0.0.1:1/foo"})).unwrap();
        assert_eq!(args.url, "http://127.0.0.1:1/foo");
    }

    #[test]
    fn parse_web_fetch_args_rejects_missing_url() {
        assert!(parse_web_fetch_args(&json!({})).is_err());
    }

    #[test]
    fn parse_web_fetch_args_rejects_non_string_url() {
        assert!(parse_web_fetch_args(&json!({"url": 42})).is_err());
    }

    #[test]
    fn parse_web_fetch_args_rejects_empty_url() {
        assert!(parse_web_fetch_args(&json!({"url": ""})).is_err());
    }

    /// GH #110: the gate parses. A syntax error is a broken CALL — it must not
    /// have to travel through reqwest to be discovered as one.
    #[test]
    fn parse_web_fetch_args_rejects_unparseable_urls_and_foreign_schemes() {
        for bad in [
            "not-a-url",
            "http://",
            "://example.org",
            "ht tp://a.example",
            "file:///etc/passwd",
            "ftp://example.org/x",
        ] {
            let err = parse_web_fetch_args(&json!({ "url": bad })).unwrap_err();
            assert!(err.contains(bad), "the error quotes the url: {err}");
        }
        // Everything the cell CAN speak still parses, including ports, query
        // strings, IPv6 literals and userinfo.
        for good in [
            "http://127.0.0.1:8080/foo?a=1#frag",
            "https://example.org",
            "http://[::1]:80/x",
            "https://user:pw@example.org/p",
        ] {
            assert_eq!(
                parse_web_fetch_args(&json!({ "url": good })).unwrap().url,
                good,
                "the url is passed through verbatim, never re-serialized"
            );
        }
    }

    // ───── GH #117: the private-network deny matrix ─────

    /// Every range the deny matrix must refuse, with the label it must name.
    ///
    /// The list is the test, not an illustration of one: a range that is not in
    /// here is a range nobody proved is closed.
    #[test]
    fn deny_reason_refuses_every_private_and_special_range() {
        for (raw, expect) in [
            // ── IPv4 ──
            ("0.0.0.0", "unspecified"),
            ("0.1.2.3", "unspecified"),
            ("10.0.0.1", "RFC 1918"),
            ("10.255.255.255", "RFC 1918"),
            ("100.64.0.1", "RFC 6598"),
            ("100.100.100.200", "RFC 6598"), // Alibaba metadata
            ("100.127.255.255", "RFC 6598"),
            ("127.0.0.1", "loopback"),
            ("127.255.255.254", "loopback"),
            ("169.254.169.254", "link-local"), // AWS/GCP/Azure metadata
            ("169.254.0.1", "link-local"),
            ("172.16.0.1", "RFC 1918"),
            ("172.31.255.255", "RFC 1918"),
            ("192.0.0.1", "IETF protocol"),
            ("192.168.0.1", "RFC 1918"),
            ("198.18.0.1", "benchmarking"),
            ("198.19.255.255", "benchmarking"),
            ("224.0.0.1", "multicast"),
            ("240.0.0.1", "reserved"),
            ("255.255.255.255", "reserved"),
            // ── IPv6 ──
            ("::", "unspecified"),
            ("::1", "loopback"),
            ("::ffff:127.0.0.1", "loopback"), // v4-mapped
            ("::ffff:169.254.169.254", "link-local"),
            ("::ffff:10.1.2.3", "RFC 1918"),
            ("::127.0.0.1", "loopback"), // deprecated v4-compatible form
            ("64:ff9b::7f00:1", "loopback"), // NAT64 of 127.0.0.1
            ("64:ff9b::a9fe:a9fe", "link-local"), // NAT64 of 169.254.169.254
            ("2002:7f00:1::", "loopback"), // 6to4 of 127.0.0.1
            ("100::1", "discard-only"),
            ("fc00::1", "unique local"),
            ("fd12:3456::1", "unique local"),
            ("fe80::1", "link-local"),
            ("febf::1", "link-local"),
            ("fec0::1", "site-local"),
            ("ff02::1", "multicast"),
        ] {
            let ip: std::net::IpAddr = raw.parse().expect("test fixture parses");
            let reason = deny_reason(ip)
                .unwrap_or_else(|| panic!("{raw} must be refused, the matrix let it through"));
            assert!(
                reason.contains(expect),
                "{raw} was refused as {reason:?}, expected a reason naming {expect:?}"
            );
        }
    }

    /// The opt-out opens the ranges a local topology can legitimately live in
    /// — and NOT the one nothing legitimate ever lives in.
    #[test]
    fn the_opt_out_opens_the_local_ranges_but_never_link_local() {
        for raw in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "100.100.100.200",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
        ] {
            let ip: std::net::IpAddr = raw.parse().unwrap();
            assert!(
                deny_reason_for(ip, true).is_none(),
                "{raw} is what the opt-out exists for"
            );
            assert!(
                deny_reason_for(ip, false).is_some(),
                "{raw} is closed by default"
            );
        }
        // Track-Ruling: link-local stays shut in BOTH tiers. 169.254.169.254 is
        // the cloud metadata endpoint — no topology deliberately runs a service
        // there, so no opt-out has a reason to reach it, and a redirect that
        // aims at it is an attack in every deployment.
        for raw in [
            "169.254.169.254",
            "169.254.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "2002:a9fe:a9fe::",
            "fe80::1",
        ] {
            let ip: std::net::IpAddr = raw.parse().unwrap();
            assert!(
                deny_reason_for(ip, true).is_some(),
                "{raw} must stay refused even with allow_private_networks"
            );
        }
    }

    /// The counter-list: public addresses stay reachable. A deny that also
    /// refuses the open internet is not a hardening, it is an outage.
    #[test]
    fn deny_reason_lets_public_addresses_through() {
        for raw in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "99.0.0.1",        // just below 100.64/10
            "100.63.255.255",  // just below 100.64/10
            "100.128.0.0",     // just above 100.64/10
            "126.255.255.255", // just below 127/8
            "128.0.0.1",       // just above 127/8
            "169.253.255.255", // just below 169.254/16
            "169.255.0.1",     // just above 169.254/16
            "172.15.255.255",  // just below 172.16/12
            "172.32.0.1",      // just above 172.16/12
            "192.0.1.1",       // just above 192.0.0/24
            "192.167.255.255", // just below 192.168/16
            "192.169.0.1",     // just above 192.168/16
            "198.17.255.255",  // just below 198.18/15
            "198.20.0.1",      // just above 198.18/15
            "223.255.255.255", // just below multicast
            "239.255.255.255", // still multicast? no — top of 224/4
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",   // v4-mapped public
            "64:ff9b::8.8.8.8", // NAT64 of a public address
            "2002:0808:0808::", // 6to4 of 8.8.8.8
            "fbff::1",          // just below fc00::/7
            "fe7f::1",          // just below fe80::/10
            "ff00::1",          // multicast — NOT public
        ] {
            let ip: std::net::IpAddr = raw.parse().expect("test fixture parses");
            let verdict = deny_reason(ip);
            // Three entries above are deliberately NOT public and act as the
            // matrix's own tripwire; the rest must pass.
            let must_be_denied = matches!(raw, "239.255.255.255" | "ff00::1");
            assert_eq!(
                verdict.is_some(),
                must_be_denied,
                "{raw}: verdict was {verdict:?}"
            );
        }
    }

    // ───── GH #117: the deny at the URL gate ─────

    /// Test helper: a cell with the deny ACTIVE (the shipped default).
    fn guarded_cell() -> WebFetchCell {
        WebFetchCell {
            client: build_web_fetch_client(false).unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_bytes: DEFAULT_WEB_FETCH_MAX_BYTES,
            allow_private_networks: false,
            max_redirects: DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        }
    }

    /// Test helper: drive one url through `handle` and return the emission.
    async fn fetch_once(cell: &WebFetchCell, url: &str, id: &str) -> meclaw_core::JsonValue {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
        use tokio::sync::mpsc;

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": json!({"url": url}).to_string(), "id": id
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        out_rx.recv().await.unwrap().content
    }

    /// The core refusal. Every one of these is a URL an agent can be talked
    /// into fetching, and each one reaches something the daemon can see but the
    /// caller must not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_private_target_is_refused_as_a_regular_error_message() {
        let cell = guarded_cell();
        for (i, url) in [
            "http://127.0.0.1:8080/admin",
            "http://localhost:8080/admin",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]:8080/",
            "http://[::ffff:127.0.0.1]/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            // The classic obfuscations: the URL parser normalises them to
            // 127.0.0.1 before the deny ever sees them.
            "http://2130706433/",
            "http://0177.0.0.1/",
            "http://0x7f.0.0.1/",
        ]
        .into_iter()
        .enumerate()
        {
            let out = fetch_once(&cell, url, &format!("blk-{i}")).await;
            assert_eq!(
                out["header"]["error_code"], "target_blocked",
                "{url} must be refused, got {}",
                out["messages"][0]["text"]
            );
            assert_eq!(out["header"]["finish_reason"], "error");
            let text = out["messages"][0]["text"].as_str().unwrap();
            assert!(
                text.contains("web_fetch"),
                "the refusal names the cell: {text}"
            );
        }
    }

    /// The refusal is an ERROR MESSAGE (rule 12 shape), not a panic and not a
    /// dead letter: `handle` returns normally and the caller gets a body it can
    /// read, carrying the `tool_call` id it sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_refusal_is_a_well_formed_body_that_carries_the_call_id() {
        let out = fetch_once(&guarded_cell(), "http://169.254.169.254/", "call-ssrf").await;
        meclaw_core::validate_ubf_body(&out).unwrap();
        assert_eq!(out["messages"][0]["id"], "call-ssrf");
        assert_eq!(out["header"]["operation"], "web_fetch");
        assert!(out["header"]["duration_ms"].is_number());
    }

    /// The escape hatch works — and only when it is written down.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_explicit_opt_out_reaches_a_local_server_the_default_refuses() {
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};

        let (addr, _join) = start_mock_server(MockResponse::ok(b"local only")).await;
        let url = format!("http://{addr}/x");

        let refused = fetch_once(&guarded_cell(), &url, "c-deny").await;
        assert_eq!(refused["header"]["error_code"], "target_blocked");

        let mut opted_out = guarded_cell();
        opted_out.allow_private_networks = true;
        opted_out.client = build_web_fetch_client(true).unwrap();
        let allowed = fetch_once(&opted_out, &url, "c-allow").await;
        assert_eq!(allowed["header"]["http_status"], 200);
        assert_eq!(allowed["messages"][0]["text"], "local only");
    }

    /// A host that does not resolve is a NETWORK fact, not a policy one — it
    /// must not borrow the `target_blocked` lane, or an agent reading the code
    /// would apply the wrong repair (GH #110's lesson).
    ///
    /// `.invalid` is reserved as never-resolvable (RFC 2606). The assertion is
    /// deliberately about the lane and not about which network error it is:
    /// a machine with no resolver at all times out instead of failing fast, and
    /// that is still not a policy refusal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_unresolvable_host_stays_off_the_target_blocked_lane() {
        let out = fetch_once(
            &guarded_cell(),
            "http://no-such-host.invalid/x",
            "c-nxdomain",
        )
        .await;
        assert_ne!(
            out["header"]["error_code"], "target_blocked",
            "DNS failure is a transport fact, not a policy one: {}",
            out["messages"][0]["text"]
        );
        assert_eq!(out["header"]["finish_reason"], "error");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_get_200_emits_tool_result_with_http_status() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::ok(b"hello world")).await;
        let cell = WebFetchCell {
            client: build_web_fetch_client(true).unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_bytes: DEFAULT_WEB_FETCH_MAX_BYTES,
            // GH #117: the mock server lives on 127.0.0.1, which the shipped
            // DEFAULT now refuses. The test takes the documented opt-out — the
            // default is not softened to keep it green.
            allow_private_networks: true,
            max_redirects: DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        };

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let url = format!("http://{addr}/foo");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-1"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "hello world");
        assert_eq!(em.content["header"]["operation"], "web_fetch");
        assert_eq!(em.content["header"]["http_status"], 200);
        assert_eq!(em.content["header"]["bytes"], 11);
        let ct = em.content["header"]["content_type"].as_str().unwrap();
        assert!(ct.starts_with("text/plain"), "content_type was {ct}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_get_404_is_normal_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::not_found()).await;
        let cell = WebFetchCell {
            client: build_web_fetch_client(true).unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_bytes: DEFAULT_WEB_FETCH_MAX_BYTES,
            // GH #117: local mock server — explicit opt-out, see above.
            allow_private_networks: true,
            max_redirects: DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let url = format!("http://{addr}/nope");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-2"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["http_status"], 404);
        // 404 is NORMAL — NO finish_reason=error (decision 3.8).
        assert!(
            em.content["header"].get("finish_reason").is_none()
                || em.content["header"]["finish_reason"] != "error"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_connect_failure_emits_io_error() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        // 127.0.0.1:1 — port 1 is not reserved, presumably no listener → connect refused.
        let cell = WebFetchCell {
            client: build_web_fetch_client(true).unwrap(),
            external_timeout: std::time::Duration::from_secs(2),
            max_concurrency: 4,
            max_bytes: DEFAULT_WEB_FETCH_MAX_BYTES,
            // GH #117: 127.0.0.1 is the POINT of this test (connect refused on
            // a dead port). Without the opt-out it would become
            // `target_blocked` and stop proving anything about io_error.
            allow_private_networks: true,
            max_redirects: DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"url":"http://127.0.0.1:1/x"}"#, "id": "call-3"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "io_error");
    }

    // ---- T4: WebFetchCellFactory tests ----

    #[test]
    fn factory_validate_params_accepts_empty_object() {
        use meclaw_colony::CellFactory;
        assert!(
            WebFetchCellFactory
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_ok()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_concurrency_zero() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory
            .validate_params(&meclaw_core::serde_json::json!({"max_concurrency": 0}));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_rejects_external_timeout_zero() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory
            .validate_params(&meclaw_core::serde_json::json!({"external_timeout_ms": 0}));
        assert!(r.is_err());
    }

    // ───── GH #83: the size cap ─────

    #[test]
    fn truncate_body_leaves_a_body_under_the_cap_alone() {
        let (out, cut) = truncate_body("small".to_string(), 1024);
        assert_eq!(out, "small");
        assert!(!cut);
    }

    #[test]
    fn truncate_body_marks_the_cut_and_names_the_full_size() {
        let (out, cut) = truncate_body("x".repeat(5000), 100);
        assert!(cut);
        assert!(
            out.starts_with(&"x".repeat(100)),
            "the prefix is the first max_bytes bytes"
        );
        assert!(
            out.contains("[truncated, 5000 bytes total]"),
            "the model must be able to see that it got a prefix: {}",
            &out[out.len() - 40..]
        );
    }

    #[test]
    fn truncate_body_cuts_on_a_char_boundary() {
        // 'ä' is two bytes; a cap of 3 lands inside the second one.
        let (out, cut) = truncate_body("äää".to_string(), 3);
        assert!(cut);
        assert!(out.starts_with("ä"), "no broken code point: {out:?}");
        assert!(out.contains("[truncated, 6 bytes total]"), "{out:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_body_over_the_cap_arrives_trimmed_and_marked() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use tokio::sync::mpsc;

        let huge = "y".repeat(20_000);
        let (addr, _join) = start_mock_server(MockResponse::ok(huge.as_bytes())).await;
        let cell = WebFetchCell {
            client: build_web_fetch_client(true).unwrap(),
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            max_bytes: 1000,
            // GH #117: local mock server — explicit opt-out, see above.
            allow_private_networks: true,
            max_redirects: DEFAULT_WEB_FETCH_MAX_REDIRECTS,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/web"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let url = format!("http://{addr}/big");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-cap"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.len() < 20_000,
            "the cap must bite: {} bytes arrived",
            text.len()
        );
        assert!(
            text.contains("[truncated, 20000 bytes total]"),
            "cut marked"
        );
        assert_eq!(
            em.content["header"]["truncated"], true,
            "the declared `truncated` header finally has a producer"
        );
        assert_eq!(
            em.content["header"]["bytes"], 20_000,
            "`bytes` reports what the server sent, not what survived the cap"
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_bytes_zero() {
        use meclaw_colony::CellFactory;
        assert!(
            WebFetchCellFactory
                .validate_params(&meclaw_core::serde_json::json!({"max_bytes": 0}))
                .is_err()
        );
    }

    #[test]
    fn factory_defaults_max_bytes_to_a_generous_but_finite_cap() {
        let p = parse_params_pure(&meclaw_core::serde_json::json!({})).unwrap();
        assert_eq!(p.max_bytes, DEFAULT_WEB_FETCH_MAX_BYTES);
        assert!(p.max_bytes > 0, "finite means non-zero, not unbounded");
    }

    // ───── GH #117: which hops the cell will follow ─────

    /// Track-Ruling: `https → http` is refused. A redirect the caller never saw
    /// must not be able to move the fetch onto the clear.
    ///
    /// (Pinned at the unit, not end to end: proving it over the wire would need
    /// a real TLS server, which no hermetic battery here has.)
    #[test]
    fn a_downgrade_from_https_to_http_is_refused_but_the_upgrade_is_not() {
        let secure = reqwest::Url::parse("https://example.org/a").unwrap();
        let err = next_hop(&secure, "http://example.org/b")
            .expect_err("a downgrade must not be followed");
        assert_eq!(err.code(), ERR_INVALID_REDIRECT);
        assert!(
            err.into_text().contains("downgrade"),
            "the refusal says why it refused"
        );

        // Upgrade and same-scheme hops stay ordinary.
        let plain = reqwest::Url::parse("http://example.org/a").unwrap();
        assert!(next_hop(&plain, "https://example.org/b").is_ok());
        assert!(next_hop(&secure, "https://elsewhere.example/c").is_ok());
        assert!(next_hop(&plain, "/relative").is_ok());
    }

    /// A `Location` this cell cannot speak is refused with its own code — it is
    /// neither a network fact nor a blocked target.
    #[test]
    fn a_foreign_or_unparseable_location_is_refused_with_its_own_code() {
        let here = reqwest::Url::parse("http://example.org/a").unwrap();
        for bad in ["file:///etc/passwd", "ftp://example.org/x", "gopher://x/1"] {
            let err = next_hop(&here, bad).expect_err("must not be followed");
            assert_eq!(err.code(), ERR_INVALID_REDIRECT, "for {bad}");
        }
    }

    /// The refusal codes are three distinct lanes, because they are three
    /// distinct repairs (GH #110's lesson, applied to the new failures).
    #[test]
    fn the_new_failures_do_not_share_a_lane() {
        let codes = [
            FetchFailure::Blocked(String::new()).code(),
            FetchFailure::TooManyRedirects(String::new()).code(),
            FetchFailure::InvalidRedirect(String::new()).code(),
            FetchFailure::Transport(String::new()).code(),
        ];
        let mut seen = codes.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4, "each failure travels under its own code");
        assert_eq!(
            FetchFailure::Transport(String::new()).code(),
            ERR_IO_ERROR,
            "transport keeps the lane it always had"
        );
    }

    // ───── GH #117: the params surface ─────

    /// Default-deny is the whole point: a colony that never says the word
    /// must still refuse the private network.
    #[test]
    fn factory_defaults_the_private_network_deny_to_on() {
        let p = parse_params_pure(&meclaw_core::serde_json::json!({})).unwrap();
        assert!(
            !p.allow_private_networks,
            "the deny is the DEFAULT; an opt-out has to be written down to exist"
        );
        assert_eq!(p.max_redirects, DEFAULT_WEB_FETCH_MAX_REDIRECTS);
    }

    #[test]
    fn factory_accepts_the_explicit_opt_out_and_a_redirect_budget() {
        let p = parse_params_pure(&meclaw_core::serde_json::json!({
            "allow_private_networks": true, "max_redirects": 0
        }))
        .unwrap();
        assert!(p.allow_private_networks);
        assert_eq!(p.max_redirects, 0);
    }

    #[test]
    fn factory_rejects_a_non_bool_opt_out_and_a_non_integer_budget() {
        use meclaw_colony::CellFactory;
        for bad in [
            meclaw_core::serde_json::json!({"allow_private_networks": "yes"}),
            meclaw_core::serde_json::json!({"allow_private_networks": 1}),
            meclaw_core::serde_json::json!({"max_redirects": -1}),
            meclaw_core::serde_json::json!({"max_redirects": "5"}),
        ] {
            assert!(
                WebFetchCellFactory.validate_params(&bad).is_err(),
                "a fat-fingered opt-out must not silently read as `false` or `true`: {bad}"
            );
        }
    }

    #[test]
    fn factory_validate_params_accepts_valid_overrides() {
        use meclaw_colony::CellFactory;
        let r = WebFetchCellFactory.validate_params(&meclaw_core::serde_json::json!({
            "max_concurrency": 8, "external_timeout_ms": 5000
        }));
        assert!(r.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_spawn_cell_routes_message_to_tool_result() {
        use meclaw_colony::CellFactory;
        use meclaw_core::{Body, CellEmission, MessageBuilder, Path, serde_json::json};
        use meclaw_testing::mock_http::{MockResponse, start_mock_server};
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let (addr, _join) = start_mock_server(MockResponse::ok(b"factory ok")).await;
        let factory: Arc<dyn CellFactory> = Arc::new(WebFetchCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/web"),
                // GH #117: the mock server is on 127.0.0.1 — explicit opt-out
                // through the PARAMS (the route a real operator would take),
                // not a softened default.
                json!({"max_concurrency": 2, "external_timeout_ms": 5000,
                       "allow_private_networks": true}),
                out_tx,
                std::path::PathBuf::new(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .expect("spawn");

        let url = format!("http://{addr}/x");
        let msg = MessageBuilder::new(Path::new("/web"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": format!(r#"{{"url":"{url}"}}"#), "id": "call-f"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();

        // Deterministic rendezvous: recv().await returns as soon as the worker
        // writes the emission into out_tx. No time-based failure marker — a
        // channel close (None) would blow the test up on unwrap(), which would be
        // a real failure, not a flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "factory ok");
        assert_eq!(em.content["header"]["operation"], "web_fetch");
        assert_eq!(em.content["header"]["http_status"], 200);

        drop(sender);
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn web_fetch_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let factory = Arc::new(WebFetchCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let hook = factory.build_boot_inactive_respawn(
            meclaw_core::Path::new("/c"),
            json!({"max_concurrency": 8, "external_timeout_ms": 5000}),
            out_tx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            itx,
            None, // idle_timeout
            0,    // cell_timeout
            None, // message_timeout
            None, // blob_store
            1000, // mailbox_capacity
        );
        assert!(
            hook.is_some(),
            "stateless web_fetch factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
