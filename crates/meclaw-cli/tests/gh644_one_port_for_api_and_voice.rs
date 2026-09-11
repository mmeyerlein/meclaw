//! GH #644, extended by #645: the API, a `voice` cell and a display answer on
//! ONE port.
//!
//! Each of the three is reached the same way. A `voice` cell and a `web` cell
//! mount under `params.mount` and are handed every connection whose first path
//! segment is that name; the HTTP API answers every other one on the same
//! socket. When #644 shipped, the display was deliberately left out — it owned
//! its whole origin (`/live/websocket`, `/@client/<file>`, `/` and every page
//! route), and what that origin becomes behind a shared listener was the open
//! question. `web@2.0.0` answers it: the origin becomes `/<mount>/`, the shell
//! writes every link from it, and the display has no port left to keep.
//!
//! Everything here goes through the real CLI: one `run_with_hooks` with `--api`
//! and `--daemon`, a tree written into a tempdir, and a `voice` cell with the
//! `echo` provider so no credential and no provider socket is involved.

use meclaw_cli::{Cli, run_with_hooks};
use meclaw_testing::voice_client::VoiceClient;
use std::net::SocketAddr;
use std::time::Duration;

/// Generous failure marker (30 s convention): every expectation below is a
/// fraction of a second under normal load, and the deadline only fences a hang.
const MARKER: Duration = Duration::from_secs(30);

/// 20 ms of 8 kHz PCM16 mono — one frame, the size a phone edge sends.
const FRAME_BYTES: usize = 320;

/// One root hive with a mounted `voice` cell and a mounted `web` cell in it.
/// `{root}` holds exactly one root directory, and that directory IS the root of
/// the tree, so the two cells are `/voice` and `/display`.
fn write_tree(root: &std::path::Path) {
    let main = root.join("main");
    std::fs::create_dir_all(&main).expect("create the root hive dir");
    std::fs::write(
        main.join("config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");

    // No `port`: this cell is reached through its mount alone, which is what the
    // one listener has to be proven on.
    std::fs::create_dir_all(main.join("voice")).expect("create the voice dir");
    std::fs::write(
        main.join("voice/config.json"),
        br#"{"cell":{"type":"voice","timeout":-1},
             "params":{"mount":"voice","stt":{"provider":"echo"},"tts":null},
             "contract":{"version":"1.0.0","settings":{},
               "ingress":{"context":["session_id"]},
               "emits":{"body":{"messages":{"type":"array","required":true}}},
               "consumes":{"body":{"messages":{"type":"array","required":true}}}}}"#,
    )
    .expect("write the voice cell");

    let display = main.join("display");
    std::fs::create_dir_all(&display).expect("create the display dir");
    std::fs::write(
        display.join("config.json"),
        // No `port` and no `bind` either: since `web@2.0.0` a document carrying
        // one is refused at parse, and this cell is reached at `/screen/`.
        br#"{"cell":{"type":"web","timeout":-1},
             "params":{"mount":"screen"},
             "contract":{"version":"1.0.0","settings":{},
               "consumes":{"body":{"messages":{"type":"array","required":false}}},
               "emits":{"body":{"messages":{"type":"array","required":true}}},
               "capabilities":["db:own"]}}"#,
    )
    .expect("write the display cell");
    seed_one_page(&display);
}

/// One page at `/` so the display has something to serve. Without a `pages` row
/// a `web` cell answers 404 everywhere, and a 404 would prove nothing here.
fn seed_one_page(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create the seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"page","template":"<h1>{{body}}</h1>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("write the components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root","parent":null,"component":"page","ord":0,"props":"{\"body\":\"the display\"}"}"#,
            "\n"
        ),
    )
    .expect("write the objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root","title":"Display"}"#,
            "\n"
        ),
    )
    .expect("write the pages");
}

/// `--api` on an ephemeral port plus `--daemon`, everything else off.
fn api_cli(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("a bind address")),
        daemon: true,
        validate: false,
        validate_strict: false,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
        command: None,
    }
}

/// GET until the answer is not the `503 starting` a cell gives before it has
/// published, or the marker passes (GH #578: the bind and the first publish are
/// two moments, and the gap between them is not a verdict).
async fn get_with_retry(url: &str) -> reqwest::Response {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        let last = match reqwest::get(url).await {
            Ok(r) if r.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE => return r,
            Ok(r) => format!("{} (nothing published yet)", r.status()),
            Err(e) => format!("{e}"),
        };
        assert!(
            tokio::time::Instant::now() < deadline,
            "nothing served on {url}; last answer: {last}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_api_a_voice_cell_and_a_display_share_one_port() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_tree(td.path());

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let cli = api_cli(td.path());
    let colony =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let api = tokio::time::timeout(MARKER, addr_rx)
        .await
        .expect("the listener binds within the failure marker")
        .expect("the address hook fires");

    // 1. The mounted cell's own declaration, through the API's port.
    let info: serde_json::Value = get_with_retry(&format!("http://{api}/voice/info"))
        .await
        .json()
        .await
        .expect("the declaration is JSON");
    assert_eq!(info["protocol"], "meclaw-voice/1");
    assert_eq!(info["stt"], "echo");

    // 2. A WebSocket through the same port, and audio that comes back unchanged.
    //    The echo provider writes the frames it was given, byte for byte.
    let (mut client, hello) = VoiceClient::connect(&format!("ws://{api}/voice/ws?session=one"))
        .await
        .expect("the mount answers a websocket on the API's port");
    assert_eq!(hello["session_id"], "one");
    let sent: Vec<u8> = (0..FRAME_BYTES).map(|i| (i % 251) as u8).collect();
    client.send_audio(&sent).await.expect("send one frame");
    let back = client
        .next_frame(MARKER)
        .await
        .expect("the echo comes back within the failure marker");
    assert_eq!(
        back.as_audio(),
        Some(sent.as_slice()),
        "the echo provider returns the frame byte for byte; got {back:?}"
    );

    // 3. The mount table, and the listener it names. Two rows now, sorted by
    //    name, and neither of them owns an address of its own.
    let table: serde_json::Value = reqwest::get(format!("http://{api}/colony/surfaces"))
        .await
        .expect("the mount table answers")
        .json()
        .await
        .expect("it is JSON");
    assert_eq!(table["listener"], api.to_string());
    let rows: Vec<(&str, &str)> = table["surfaces"]
        .as_array()
        .expect("the table is a list")
        .iter()
        .map(|r| {
            (
                r["mount"].as_str().expect("a mount"),
                r["kind"].as_str().expect("a kind"),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![("screen", "web"), ("voice", "voice")],
        "both surfaces stand in the table, sorted by name"
    );

    // 4. The same table as the proxy's map.
    let traefik: serde_json::Value =
        reqwest::get(format!("http://{api}/colony/surfaces?format=traefik"))
            .await
            .expect("the traefik document answers")
            .json()
            .await
            .expect("it is JSON");
    assert_eq!(
        traefik["http"]["routers"]["meclaw-voice"]["rule"],
        "PathPrefix(`/voice`)"
    );
    assert_eq!(
        traefik["http"]["routers"]["meclaw-voice"]["service"],
        "meclaw"
    );
    assert_eq!(
        traefik["http"]["services"]["meclaw"]["loadBalancer"]["servers"][0]["url"],
        format!("http://{api}")
    );
    println!(
        "TRAEFIK {}",
        serde_json::to_string_pretty(&traefik).expect("render the document")
    );

    // 5. The API is untouched: its own endpoints answer on the same socket.
    let graph = reqwest::get(format!("http://{api}/colony/graph"))
        .await
        .expect("the graph answers");
    assert_eq!(graph.status(), 200);
    let graph = graph.text().await.expect("the graph body");
    for path in ["/voice", "/display"] {
        assert!(graph.contains(path), "the graph names {path}; got {graph}");
    }

    // 6. The display, on the same socket as the API and the voice cell — the
    //    half #644 could not have. Its own tree path is `/display`, its mount
    //    is `screen`, and the mount is what the URL says: a page is reached
    //    under the name its params declare, never under a cell path.
    let page = get_with_retry(&format!("http://{api}/screen/")).await;
    assert_eq!(page.status(), 200);
    let page = page.text().await.expect("the page body");
    assert!(
        page.contains("data-phx-main"),
        "the display serves its shell under its mount on the one listener; got {page}"
    );
    assert!(
        page.contains("\"/screen/live\"") && page.contains("<base href=\"/screen/\">"),
        "and every link it writes starts at that mount; got {page}"
    );
    let by_cell_path = reqwest::get(format!("http://{api}/display/"))
        .await
        .expect("the API answers");
    assert_eq!(
        by_cell_path.status(),
        404,
        "nothing is mounted as `display`, and the API says so rather than guessing"
    );

    let _ = client.close().await;
    shutdown_tx.send(()).expect("the colony is still listening");
    tokio::time::timeout(MARKER, colony)
        .await
        .expect("the shutdown does not hang")
        .expect("join")
        .expect("run_with_hooks Ok");
}
