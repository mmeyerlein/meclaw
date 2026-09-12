//! GH #677 — the README's nginx example strips the prefix it announces.
//!
//! The shell reads `X-Forwarded-Prefix` as what a reverse proxy says it
//! STRIPPED (`web/io.rs`, `base_of`): the request that reaches the listener is
//! `/<mount>/…`, and the header is how the shell learns that a browser sees
//! `/<prefix>/<mount>/…` instead. The listener, for its part, routes by the
//! first path segment (`meclaw-colony/src/surfaces/listener.rs`,
//! `first_segment`) — so a proxy that forwards the URI unchanged sends
//! `/<prefix>/<mount>/` to whoever holds `<prefix>`, which is nobody, and the
//! display is never reached however correct the header is.
//!
//! The README's example block is the one place an operator copies from, and it
//! shipped without the trailing slash that makes nginx strip the matched
//! `location`. Nothing was red, because no test read the block.
//!
//! The block also has to carry the socket. `<base>/live` is a WebSocket
//! endpoint (`web/io.rs`, `get_socket`): a request that reaches it without an
//! `Upgrade` header is a `400`, and that is exactly what nginx forwards when the
//! block lacks `proxy_http_version 1.1` and the two `Upgrade`/`Connection`
//! headers — the page renders, and LiveView never connects.
//!
//! This is a drift lock in the sense of the development rules § 2d: it reads the
//! nginx block out of `templates/web/README.md` and asserts, against a real
//! `web` cell on a real listener, the mechanism the block relies on — the
//! stripped form reaches the mount and every link moves with the prefix; the
//! unstripped form does not reach the mount at all; the socket under the mount
//! takes an upgrade and refuses a plain GET. The README's block is the one
//! source: the two overview twins and the `template.json` example are asserted
//! to carry the same directives, so a repair is one edit that cannot half-land.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::{CellEmission, Path, serde_json::json};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The one ```nginx fence in the web README.
fn nginx_block(readme: &str) -> String {
    let fences: Vec<&str> = readme
        .split("```nginx\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap_or(""))
        .collect();
    assert_eq!(
        fences.len(),
        1,
        "the README carries exactly ONE nginx example, so repairing it is one edit"
    );
    fences[0].to_string()
}

/// Every directive line of the block, trimmed, without its `;` and without the
/// `location`'s opening brace — the form the `template.json` example quotes.
fn directives(block: &str) -> Vec<String> {
    block
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "}")
        .map(|l| {
            l.trim_end_matches(';')
                .trim_end_matches('{')
                .trim()
                .to_string()
        })
        .collect()
}

/// The arguments of every `name` line, each trimmed of its `;`.
fn arguments<'a>(block: &'a str, name: &str) -> Vec<&'a str> {
    block
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with(name) && l[name.len()..].starts_with(char::is_whitespace))
        .map(|l| l[name.len()..].trim().trim_end_matches(';').trim())
        .collect()
}

/// The one `name` line's argument.
fn directive<'a>(block: &'a str, name: &str) -> &'a str {
    match arguments(block, name).as_slice() {
        [one] => one,
        found => {
            panic!("the nginx block carries exactly one `{name}` line, found {found:?}:\n{block}")
        }
    }
}

/// The value of the one `proxy_set_header <header>` line.
fn header<'a>(block: &'a str, name: &str) -> &'a str {
    let values: Vec<&str> = arguments(block, "proxy_set_header")
        .into_iter()
        .filter_map(|a| a.split_once(char::is_whitespace))
        .filter(|(h, _)| *h == name)
        .map(|(_, v)| v.trim())
        .collect();
    match values.as_slice() {
        [one] => one,
        found => panic!("the block sets `{name}` exactly once, found {found:?}:\n{block}"),
    }
}

/// Prose with every run of whitespace folded to one space, so a sentence the
/// README wraps over two lines can be matched as one.
fn folded(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The nginx fence in the overview that carries the prefix header — the
/// twin of the README's block.
fn overview_block(doc: &str) -> String {
    let fences: Vec<&str> = doc
        .split("```nginx\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap_or(""))
        .filter(|f| f.contains("X-Forwarded-Prefix"))
        .collect();
    assert_eq!(
        fences.len(),
        1,
        "the overview carries ONE prefixed nginx example"
    );
    fences[0].to_string()
}

/// What the README's block promises: the prefix the browser sees, and the
/// upstream the proxy forwards to.
struct Example {
    prefix: String,
    proxy_pass: String,
}

fn example(readme: &str) -> Example {
    let block = nginx_block(readme);
    let location = directive(&block, "location").trim_end_matches('{').trim();
    let path = location
        .split_whitespace()
        .last()
        .expect("`location` names a path");
    let prefix = path
        .strip_suffix('/')
        .expect("the location path ends with `/` — a prefix match, not a name")
        .to_string();
    assert!(
        prefix.starts_with('/') && prefix.len() > 1,
        "the location names a path prefix; got {location:?}"
    );
    assert_eq!(
        header(&block, "X-Forwarded-Prefix"),
        prefix,
        "the header names exactly the prefix the location matched, without a trailing slash"
    );
    // The three lines that carry the socket. Without `proxy_http_version 1.1`
    // nginx speaks HTTP/1.0 to the upstream, which has no upgrade; without the
    // two headers the upgrade request arrives as a plain GET.
    assert_eq!(directive(&block, "proxy_http_version"), "1.1");
    assert_eq!(header(&block, "Upgrade"), "$http_upgrade");
    assert_eq!(header(&block, "Connection"), "\"upgrade\"");
    Example {
        prefix,
        proxy_pass: directive(&block, "proxy_pass").to_string(),
    }
}

/// GET the URL until the cell answers as a SERVED cell (any status but 503).
async fn get_with_retry(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let last = match reqwest::get(url).await {
            Ok(r) if r.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE => return r,
            Ok(r) => format!("{} (the cell had not published yet)", r.status()),
            Err(e) => format!("{e}"),
        };
        assert!(
            Instant::now() < deadline,
            "the web cell never served on {url}; last answer: {last}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One GET with the proxy's header set.
async fn get_prefixed(url: &str, prefix: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .header("X-Forwarded-Prefix", prefix)
        .send()
        .await
        .expect("the listener answers")
}

/// One page at `/`, so there is something to serve.
fn seed_one_page(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            r#"{"name":"page","template":"<h1>{{body}}</h1>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#
        ),
    )
    .expect("write components");
    std::fs::write(
        seed.join("objects.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            r#"{"id":"root","parent":null,"component":"page","ord":0,"props":"{\"body\":\"hello\"}"}"#
        ),
    )
    .expect("write objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            r#"{"route":"/","root":"root","title":"Home"}"#
        ),
    )
    .expect("write pages");
}

/// What a booted display needs kept alive for the length of the test: the
/// mailbox sender (closing it ends the handler, which ends the I/O half), the
/// stop end, and the two tasks.
struct Live {
    join: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    _sender: mpsc::Sender<meclaw_core::Message>,
    _stop_tx: tokio::sync::oneshot::Sender<()>,
}

/// One `web` cell under `mount`, on one real listener with the HTTP-API
/// fallback answering `404` — the shape `meclaw_testing::surface_listener`
/// gives every one-listener test.
async fn boot(cell_dir: &std::path::Path, mount: &str) -> (String, Live) {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({ "mount": mount }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a web cell with a valid mount must spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active");
    };
    wait_for_mount(&surfaces, mount).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    (
        format!("http://{addr}"),
        Live {
            join,
            listener,
            _sender: sender,
            _stop_tx: stop_tx,
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_readme_example_strips_the_prefix_it_announces() {
    // --- The sentence: the block an operator copies. ---
    let readme = std::fs::read_to_string(repo("templates/web/README.md"))
        .expect("templates/web/README.md ships");
    let Example { prefix, proxy_pass } = example(&readme);
    assert!(
        proxy_pass.starts_with("http://") && proxy_pass.ends_with('/'),
        "`proxy_pass` ends with `/`: that is what makes nginx replace the matched \
         `location` part, i.e. STRIP the prefix the header announces; got {proxy_pass:?}"
    );
    assert!(
        !prefix.contains(char::is_uppercase)
            && prefix[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '/' | '-')),
        "the example's prefix is one the shell's own grammar accepts; got {prefix:?}"
    );
    let prose = folded(&readme);
    assert!(
        prose.contains("The proxy must strip the prefix it announces")
            && prose.contains(
                "In nginx the trailing slash on `proxy_pass` is what does the stripping."
            ),
        "the README says, in words, that the proxy must strip the prefix and that the \
         trailing slash is what does it in nginx"
    );
    assert!(
        prose.contains(
            "the three upgrade lines carry the LiveView socket; without them the page renders and never connects"
        ),
        "the README says, in words, what the three upgrade lines are for"
    );

    // --- The block is the one source: the twins carry the same directives. ---
    let block = nginx_block(&readme);
    let mut ours = directives(&block);
    ours.sort();
    // `docs/meclaw-overview.md` is in both trees (the German edition here, the
    // English bytes under that name in the published tree); the `.en.md` twin
    // exists only here, so it is compared where it is on disk and skipped
    // where it is not -- the gh325 form.
    for twin in ["docs/meclaw-overview.md", "docs/meclaw-overview.en.md"] {
        let path = repo(twin);
        if twin.ends_with(".en.md") && !path.exists() {
            eprintln!("SKIPPED: {twin} is not in this tree -- one edition only");
            continue;
        }
        let doc = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{twin}: {e}"));
        let mut theirs = directives(&overview_block(&doc));
        theirs.sort();
        assert_eq!(
            theirs, ours,
            "{twin} carries the README's block, directive for directive"
        );
    }
    let template: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/web/template.json")).expect("template.json ships"),
    )
    .expect("template.json parses");
    let examples = template["description"]["examples"]
        .as_array()
        .expect("description.examples is a list");
    let proxy_example = examples
        .iter()
        .filter_map(|e| e.as_str())
        .find(|e| e.starts_with("A PROXY MAY PUT IT ANYWHERE"))
        .expect("template.json carries the proxy example");
    for line in &ours {
        assert!(
            proxy_example.contains(line.as_str()),
            "the template.json example quotes every directive of the README's block; \
             missing {line:?} in:\n{proxy_example}"
        );
    }

    // --- The mechanism: the block's two halves, against a real cell. ---
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir);
    let mount = "screen";
    let (base, live) = boot(&cell_dir, mount).await;
    // Published before the prefixed request, so what follows is about the
    // prefix and not about a cell that had not published yet.
    let _ = get_with_retry(&format!("{base}/{mount}/")).await;

    // Stripped, as the block now forwards it: the mount is reached and every
    // link the shell writes carries the prefix the header announced.
    let resp = get_prefixed(&format!("{base}/{mount}/"), &prefix).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the stripped path reaches the mount"
    );
    let body = resp.text().await.expect("read the body");
    assert!(
        body.contains(&format!("<base href=\"{prefix}/{mount}/\">")),
        "the shell's base is the announced prefix plus the mount; body was:\n{body}"
    );
    let socket_url = format!("{prefix}/{mount}/live");
    assert!(
        body.contains(&format!("new LiveView.LiveSocket(\"{socket_url}\"")),
        "and so is the socket URL the shell hands the LiveView client; body was:\n{body}"
    );

    // The socket itself, under the mount: what nginx forwards WITHOUT the three
    // upgrade lines is a plain GET, and the transport refuses it (400, not
    // 404 — the path is right); with them the upgrade goes through.
    let transport = format!("{base}/{mount}/live/websocket");
    let plain = get_prefixed(&transport, &prefix).await;
    assert_eq!(
        plain.status().as_u16(),
        400,
        "a GET without `Upgrade` never becomes a socket — that is the request a block \
         without the three lines forwards"
    );
    let mut req = IntoClientRequest::into_client_request(format!(
        "ws://{}/{mount}/live/websocket",
        base.trim_start_matches("http://")
    ))
    .expect("a websocket request");
    req.headers_mut().insert(
        "X-Forwarded-Prefix",
        prefix.parse().expect("the prefix is a header value"),
    );
    let (_ws, upgraded) = tokio_tungstenite::connect_async(req)
        .await
        .expect("an upgrade request under the mount is accepted");
    assert_eq!(
        upgraded.status().as_u16(),
        101,
        "the same path with `Upgrade` switches protocols"
    );

    // Unstripped, as the block used to forward it: the first segment is the
    // prefix, which is no mount, and the listener's fallback answers — the
    // header cannot repair a path that never reached the cell.
    let resp = get_prefixed(&format!("{base}{prefix}/{mount}/"), &prefix).await;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "a prefix the proxy did not strip is routed by its first segment, and \
         {prefix:?} holds no mount"
    );
    let body = resp.text().await.expect("read the body");
    assert!(
        !body.contains("<base href="),
        "nothing of the shell answers on the unstripped path; body was:\n{body}"
    );

    live.join.abort();
    live.listener.abort();
}
