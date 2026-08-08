//! `meclaw` CLI library.

pub mod bridge;
pub mod factories;
pub use factories::built_in_factories;

use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;

/// Initialize the **global** tracing subscriber with two layers:
///   1. `console-subscriber` for tokio-console async-task observability
///      (requires `--cfg tokio_unstable`, set in `.cargo/config.toml` Phase 0).
///   2. JSON `fmt` layer writing non-blocking to `log_path` via `tracing-appender`.
///
/// Returns the `WorkerGuard` for the non-blocking writer — caller MUST keep it
/// alive for the duration of the process (Drop flushes pending writes). Calling
/// twice in the same process is an error (`set_global_default` may only be set
/// once). Tests that need a fresh subscriber must run in separate processes
/// (integration tests do; multiple `#[test]` fns in the same crate share state).
pub fn setup_subscriber(
    log_path: &Path,
    level: &str,
    filter: Option<&str>,
    tokio_console: bool,
    tokio_console_port: u16,
) -> anyhow::Result<WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::*;

    if let Some(parent) = log_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let env_filter = match filter {
        Some(expr) => EnvFilter::try_new(expr)?,
        None => EnvFilter::try_new(level)?,
    };

    // U10: build the layer only on opt-in; check the port up front (a clear
    // error message instead of a panic on an occupied port). `None` ⇒ an
    // `Option<Layer>` no-op.
    let console_layer = check_tokio_console(tokio_console, tokio_console_port)?.map(|addr| {
        console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .server_addr(addr)
            .spawn()
    });

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_filter(env_filter);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("set_global_default failed: {e}"))?;

    Ok(guard)
}

/// U10 (RULED A8, 2026-06-12): tokio-console is opt-in. Decides whether the
/// debug layer becomes active, and CHECKS the port via a pre-bind so that an
/// occupied port yields a CLEAR error message instead of letting
/// `console_subscriber` panic internally with `AddrInUse` (the old symptom on a
/// parallel start).
///
/// - `tokio_console == false` ⇒ `Ok(None)` — no bind, layer off.
/// - enabled + port free ⇒ `Ok(Some(addr))` (the probe listener is released
///   again immediately; the remaining TOCTOU window is accepted for the POC).
/// - enabled + port occupied ⇒ `Err` with the port in the message, no panic.
pub fn check_tokio_console(
    tokio_console: bool,
    port: u16,
) -> anyhow::Result<Option<std::net::SocketAddr>> {
    if !tokio_console {
        return Ok(None);
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let probe = std::net::TcpListener::bind(addr).map_err(|e| {
        anyhow::anyhow!(
            "tokio-console: port {port} is already in use ({e}) — choose a different \
             --tokio-console-port or stop the blocking process"
        )
    })?;
    drop(probe);
    Ok(Some(addr))
}

/// Command-line interface surface. Unknown flags are rejected by clap.
#[derive(Debug, clap::Parser)]
#[command(
    name = "meclaw",
    version,
    about = "File-based, LLM-oriented actor workflow system for agentic harnesses."
)]
pub struct Cli {
    /// Filesystem root of the colony.
    #[arg(long, default_value = ".")]
    pub root: PathBuf,

    /// Path to the JSONL tracing log. Default: `{root}/log.jsonl`.
    #[arg(long)]
    pub log: Option<PathBuf>,

    /// Tracing default level (overridden by colony.json if present).
    #[arg(long = "log-level", default_value = "info")]
    pub log_level: String,

    /// `RUST_LOG`-style per-module filter expression.
    #[arg(long = "log-filter")]
    pub log_filter: Option<String>,

    /// `.env` file for variable substitution. Default: `<root>/.env`.
    #[arg(long, value_name = "PATH")]
    pub env: Option<PathBuf>,

    /// Templates directory. Default: `<root>/templates`.
    #[arg(long, value_name = "PATH")]
    pub templates: Option<PathBuf>,

    /// Rebuild the templates registry from the `templates/` directory.
    #[arg(long, default_value_t = false)]
    pub rescan_templates: bool,

    /// Bind address for the HTTP API and operator web UI. Default: off (no port opened).
    #[arg(long, value_name = "BIND")]
    pub api: Option<std::net::SocketAddr>,

    /// Daemon mode: foreground, no stdin/stdout bridge. Independent of
    /// `--api`: `--daemon` without `--api` runs the colony headless (spawn +
    /// bootstrap, timer/proxy topologies run, no HTTP server) until
    /// SIGTERM/ctrl_c; `--daemon --api` runs with the HTTP server. No
    /// fork/setsid (systemd Type=simple).
    #[arg(long, default_value_t = false)]
    pub daemon: bool,

    /// Dry run: bootstrap, schema checks, templates, and mutation replay;
    /// no cell spawns, no HTTP listen.
    #[arg(long, default_value_t = false)]
    pub validate: bool,

    /// Strict mode for `--validate`: promotes otherwise-warned static
    /// findings (in particular dangling `params.graph` endpoints that a
    /// static run cannot resolve against the runtime registry) to hard
    /// errors (non-zero exit). Off by default; the operator decides
    /// (nginx -t style).
    #[arg(long, default_value_t = false)]
    pub strict: bool,

    /// Blob storage directory. Default: `<root>/blobs`.
    #[arg(long, value_name = "PATH")]
    pub blobs: Option<PathBuf>,

    /// Enable the tokio-console debug layer. Opt-in: off by default. Without
    /// this flag no debug port is bound.
    #[arg(long, default_value_t = false)]
    pub tokio_console: bool,

    /// Bind port for the tokio-console layer. Only effective with
    /// `--tokio-console`. Default 6669.
    #[arg(
        long = "tokio-console-port",
        value_name = "PORT",
        default_value_t = 6669
    )]
    pub tokio_console_port: u16,

    /// Wire format of the stdin/stdout bridge. Default `text`.
    #[arg(
        long = "stdio-format",
        value_enum,
        value_name = "FORMAT",
        default_value_t = StdioFormat::Text
    )]
    pub stdio_format: StdioFormat,
}

/// Wire format of the stdin/stdout bridge.
///
/// Two formats, one bridge. `text` is what the bridge has always spoken and
/// stays the default, so every existing pipe and interactive session is
/// unaffected. `json` is the versioned protocol surface a program talks: it
/// carries the envelope (`trace_id`, `ttl`, `context`) the text format cannot
/// express, announces itself with a boot handshake, and returns whole bodies
/// instead of a single line. It is the wire a sub-colony parent drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StdioFormat {
    /// One raw text line in, one assistant line out.
    Text,
    /// One JSON frame per line, with full envelope control.
    Json,
}

/// Type alias for `Cli` — public surface consumed by integration tests and Phase 11+.
pub type Args = Cli;

/// Run the CLI. Phase 0 reaches this only via `--version`/`--help`,
/// which clap handles before this is called.
///
/// Phase-12-A: `async fn` — `boot_load_or_scan` has been async since phase 12-A
/// (the δ bridge `send_op_try` was removed). `main.rs` runs under
/// `#[tokio::main]` (multi_thread, `worker_threads = 4`).
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    run_with_hooks(cli, None, None).await
}

/// Lifecycle implementation: bind → serve → graceful shutdown.
///
/// `addr_hook`: an optional oneshot, filled with the real `SocketAddr` resolved
/// from `0.0.0.0:0` after a successful bind (for tests).
/// `shutdown_hook`: an optional oneshot receiver — when `Some`, it is used as a
/// shutdown trigger in addition to `ctrl_c`/`SIGTERM`.
///
/// The production path in `run()` calls `run_with_hooks(cli, None, None)`.
/// Since T12 (slice 12-B): a real `colony_task` is spawned, the inbox sender
/// moves into the `ColonyHandle`, and `bootstrap_from_filesystem` hands the
/// filesystem plan to the running colony. Shutdown order:
/// axum drain → `ColonyMsg::Shutdown` + ack → `colony_join.await` (with
/// timeouts against a hang).
pub async fn run_with_hooks(
    cli: Cli,
    addr_hook: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>,
    shutdown_hook: Option<tokio::sync::oneshot::Receiver<()>>,
) -> anyhow::Result<()> {
    let db_path = cli.root.join("colony.db");
    let colony_db = meclaw_colony::ColonyDb::open(&db_path)
        .map_err(|e| anyhow::anyhow!("open colony.db: {e}"))?;

    // Slice 11-D: Boot-Wiring — Templates-Registry laden oder auto-scannen.
    let templates_root = cli
        .templates
        .clone()
        .unwrap_or_else(|| cli.root.join("templates"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Err(e) = meclaw_colony::templates::boot_load_or_scan(
        &templates_root,
        &colony_db,
        cli.rescan_templates,
        now,
    )
    .await
    {
        eprintln!("boot template scan failed: {e:?}");
        std::process::exit(1);
    }

    drop(colony_db);

    // --validate: Dry-Run-Vorrang (Spec Z.430).
    //
    // Phase-12-close hardening (T28): in addition to the existing templates
    // scan, two further checks run **without** spawning colony/axum
    // (--validate returns before the `colony_task::spawn` at Z.~210):
    //   1. `plan_bootstrap` — the filesystem bootstrap plan (MultipleRootDirs /
    //      NoRootDir / InvalidParams / CorruptCellDb).
    //   2. `probe_boot_state` — colony.db consistency (registry/edges/
    //      hive_scopes mixed counts → `BootState::Inconsistent`).
    //      Called DIRECTLY (a read-only connection probe, no panic),
    //      NOT via the `colony_task` panic path in `colony.rs:386`
    //      (phase-5 legacy, its own robustness pass after phase 13/14).
    //
    // Aggregation: eprintln every error, then `Err(...)` at the end if ANY check
    // fails — process exit != 0. exit 0 only when all three are clean.
    //
    // Spec aspiration gap: schema checks (validate_params), template resolution
    // (add_nodes replay) and mutation replay from `mutation_log` stay deferred
    // (their own robustness pass). See PROGRESS § phase-12 limitations.
    if cli.validate {
        if cli.api.is_some() || cli.daemon {
            eprintln!("note: --validate has precedence; --api/--daemon ignored");
        }
        let mut had_error = false;

        // 1. Filesystem-Bootstrap-Plan (additiv).
        let factories = built_in_factories();
        // A5b (Phase-16 W1b): `--validate` lists unknown (unregistered) nodes on
        // a Reboot, so it must consult the REAL persisted overlay + boot state
        // (not an empty dry-run overlay). On a FirstBoot the overlay is empty and
        // the boot classifies FirstBoot → no node is ever reported unregistered,
        // exactly as before. A probe failure falls back to FirstBoot (the
        // dedicated consistency probe below reports an inconsistent DB).
        let validate_overlay = meclaw_colony::read_registry_overlay(&db_path)
            .unwrap_or_else(|_| meclaw_colony::RegistryOverlay::new());
        let validate_boot_state = meclaw_colony::probe_boot_state(&db_path)
            .unwrap_or(meclaw_colony::BootState::FirstBoot);
        match meclaw_colony::plan_bootstrap_with_env(
            &cli.root,
            &factories,
            &validate_overlay,
            validate_boot_state,
            cli.env.as_deref(),
        ) {
            Err(errs) => {
                for e in errs.items() {
                    eprintln!("validate: bootstrap-plan error: {e:?}");
                }
                had_error = true;
            }
            Ok(plan) => {
                // A8 (Phase-16 W1a, Ruling 2026-06-12): static endpoint-existence
                // check. `--validate` has no running colony, so it cannot see
                // runtime-spawned cells — an unresolved `params.graph` endpoint
                // is a WARNING (exit 0). `--strict` promotes it to a hard error
                // (operator decides, nginx -t style). The registry term is empty
                // here (no live colony); only the plan + `/colony/*` resolve.
                let unresolved = meclaw_colony::unresolved_boot_endpoints(
                    &plan,
                    &std::collections::HashSet::new(),
                );
                for (edge_id, endpoint) in &unresolved {
                    eprintln!(
                        "validate: warning: dangling edge endpoint {} (edge {edge_id}) — \
                         resolves to no FS cell/hive or /colony endpoint \
                         (runtime-spawned cells are invisible to --validate)",
                        endpoint.as_str()
                    );
                }
                if cli.strict && !unresolved.is_empty() {
                    had_error = true;
                }
                // A5b (Phase-16 W1b): on a Reboot, unknown (unregistered) cell
                // dirs are a consistency drift — reported as a WARNING (exit 0,
                // nginx -t role); `--strict` promotes them to a hard error.
                // Registration is instantiation/mutation-only; the operator
                // mutates with `adopt` to register such a node.
                for path in &plan.unregistered_nodes {
                    eprintln!(
                        "validate: warning: unregistered cell directory {} — NOT adopted on \
                         reboot (registration is instantiation/mutation-only; mutate with \
                         `adopt` to register it)",
                        path.as_str()
                    );
                }
                if cli.strict && !plan.unregistered_nodes.is_empty() {
                    had_error = true;
                }
            }
        }

        // 2. colony.db consistency (additive, called directly — no panic).
        match meclaw_colony::probe_boot_state(&db_path) {
            Ok(meclaw_colony::BootState::FirstBoot) | Ok(meclaw_colony::BootState::Reboot) => {}
            Ok(meclaw_colony::BootState::Inconsistent { reason }) => {
                eprintln!("validate: colony.db inconsistent: {reason}");
                had_error = true;
            }
            Err(e) => {
                eprintln!("validate: colony.db probe failed: {e:?}");
                had_error = true;
            }
        }

        // 3. colony.json parse (additiv, strict-fail). Absent → default (no error);
        //    present-but-broken / wrong schema → hard validate failure (Demo g).
        if let Err(e) = meclaw_colony::colony_config::read_colony_config(&cli.root) {
            eprintln!("validate: colony.json invalid: {e}");
            had_error = true;
        }

        if had_error {
            return Err(anyhow::anyhow!(
                "validate failed: see stderr for diagnostics"
            ));
        }
        return Ok(());
    }

    // U9 (RULED A8, 2026-06-12): headless mode is legitimate. The flags are
    // independent of each other — `--daemon` = the process runs (as a daemon),
    // `--api` = HTTP server on; every combination is valid. `--daemon` without
    // `--api` boots the full colony headless (timer-/proxy-driven topologies run
    // without HTTP); the spawn + bootstrap below runs for all modes. Direct mode
    // (no flag) additionally needs the root-hive guard.
    let is_direct_mode = cli.api.is_none() && !cli.daemon;

    // Step 5.1 — root-hive guard: direct mode requires `/` as a hive scope.
    // Checked via the bootstrap plan (no side effect, identical to --validate).
    if is_direct_mode {
        let factories = built_in_factories();
        let plan_overlay = meclaw_colony::read_registry_overlay(&db_path)
            .unwrap_or_else(|_| meclaw_colony::RegistryOverlay::new());
        let plan_boot_state = meclaw_colony::probe_boot_state(&db_path)
            .unwrap_or(meclaw_colony::BootState::FirstBoot);
        let plan = meclaw_colony::plan_bootstrap_with_env(
            &cli.root,
            &factories,
            &plan_overlay,
            plan_boot_state,
            cli.env.as_deref(),
        )
        .unwrap_or_else(|_| meclaw_colony::BootstrapPlan::default());
        let root_is_hive = plan.hives.iter().any(|h| h.path.as_str() == "/");
        if !root_is_hive {
            anyhow::bail!(
                "root must be a hive for direct-mode: {} has no root hive (`type: \"hive\"` \
                 at `/`). Add a `config.json` with `{{\"cell\":{{\"type\":\"hive\"}}}}` at \
                 the root, or use `--daemon` / `--api` for non-hive roots.",
                cli.root.display()
            );
        }
    }

    // T12 Real Colony-Spawn (replaces T5 stub):
    // 1. Re-open colony.db (the earlier handle was dropped after boot template scan).
    // 2. Build inbox + outputs channels (cap 1024, matches Phase-12-Pre Bounded-Writer
    //    headroom philosophy for HTTP-driven burst-load).
    // 3. Spawn `colony_task` — runs until `ColonyMsg::Shutdown` is received.
    // 4. Bootstrap from filesystem (zero hives/cells/edges for empty roots).
    // 5. Build `ColonyHandle` with the real inbox sender and hand it to the router.
    let colony_db = meclaw_colony::ColonyDb::open(&db_path)
        .map_err(|e| anyhow::anyhow!("re-open colony.db for spawn: {e}"))?;
    // Channels: inbox for ColonyMsg::Route/Shutdown, outputs for CellEmission.
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::channel(1024);
    let (outputs_tx, outputs_rx) = tokio::sync::mpsc::channel(1024);
    let factories = built_in_factories();
    let root_path = cli.root.clone();

    // Phase-13.5 A7: read colony.json (absent → defaults; broken → hard boot fail).
    let colony_config = meclaw_colony::colony_config::read_colony_config(&cli.root)
        .map_err(|e| anyhow::anyhow!("colony.json: {e}"))?;

    // Phase-12-X T17 / phase-13.5 A8: the blob store is instantiated here.
    // Default path `<root>/blobs`; --blobs overrides it. It flows both into
    // `colony_task`/`runtime` (A8 — cell delivery-boundary resolution +
    // auto-offload) and into the router (T18 — the multipart path of
    // POST /messages).
    let blob_root = cli.blobs.clone().unwrap_or_else(|| cli.root.join("blobs"));
    let blob_store = std::sync::Arc::new(
        meclaw_colony::blob::DiskBlobStore::new(&blob_root)
            .map_err(|e| anyhow::anyhow!("open blob store {}: {e}", blob_root.display()))?,
    );

    let inbox_self_tx = inbox_tx.clone();
    // Deep-Audit F3: heartbeat channel — the colony loop emits ~10×/s; the
    // supervisor below counts misses and triggers a clean stop on colony death.
    let (heartbeat_tx, heartbeat_rx) = tokio::sync::mpsc::channel::<()>(8);

    // Step 5.3 — Direct-Mode egress channel: root-hive HiveNoRoute → stdout.
    // Only set in Direct-Mode; --daemon/--api paths leave egress as None → DLQ
    // behaviour unchanged.
    let (egress_tx_opt, egress_rx_direct) = if is_direct_mode {
        let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1024);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let mut colony_cfg = meclaw_colony::ColonyTaskConfig::new(
        inbox_self_tx,
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        colony_db,
        factories.clone(),
        root_path.clone(),
        colony_config.clone(),
        Some(blob_store.clone()),
        cli.env.clone(), // U8 (RULED A8): the colony remembers its env source from startup — the same source as the boot substitution path (bootstrap_from_filesystem_with_env below)
    )
    .with_heartbeat(heartbeat_tx);
    if let Some(egress_tx) = egress_tx_opt {
        colony_cfg = colony_cfg.with_egress(egress_tx);
    }
    let colony_join = tokio::spawn(meclaw_colony::colony_task(colony_cfg));

    // Deep-Audit F3: heartbeat-watchdog supervisor. Lives OUTSIDE the colony task
    // so a colony panic (loop gone → heartbeats stop) is observable. 5 consecutive
    // missed periods (~0.5 s of silence) → clean stop (NO restart: a Tokio task's
    // state is not revivable; Ops/boot-supervisor restarts MeClaw). Fires the same
    // graceful shutdown path as SIGTERM via `wd_stop`.
    let (wd_stop_tx, wd_stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        heartbeat_rx,
        wd_stop_tx,
        5,
        std::time::Duration::from_millis(100),
    ));

    // Bootstrap from filesystem (reads config.json files, plans + applies).
    // Empty roots (no config.json) → BootstrapReport with zero counts.
    // TTL slice (2026-06-11): keep the ingress TTL default before colony_config
    // moves into the runtime — the router consumes it below.
    let message_default_ttl = colony_config.message_default_ttl;
    let runtime = meclaw_colony::ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx: outputs_tx.clone(),
        colony_config,
        blob_store: Some(blob_store.clone()),
    };
    match meclaw_colony::bootstrap_from_filesystem_with_env(
        &root_path,
        &factories,
        &runtime,
        cli.env.as_deref(),
    )
    .await
    {
        Ok(report) => {
            tracing::info!(
                hives = report.hive_count,
                cells = report.cell_count,
                edges = report.edge_count,
                "filesystem bootstrap applied"
            );
        }
        Err(e) => {
            eprintln!("bootstrap_from_filesystem failed: {e:?}");
            // Don't leak the colony task: send Shutdown + await ack + join.
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            let _ = inbox_tx
                .send(meclaw_colony::ColonyMsg::Shutdown { ack: ack_tx })
                .await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), ack_rx).await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), colony_join).await;
            return Err(anyhow::anyhow!("bootstrap failed: {e:?}"));
        }
    }

    // Steps 5.4 + 5.5 — Direct-Mode stdin-reader and egress-writer tasks.
    // Spawned only when is_direct_mode; the Option variables hold their abort
    // handles so we can clean up on shutdown.
    let (eof_rx_for_select, stdin_task_handle, egress_task_handle) = if is_direct_mode {
        // EOF oneshot: the reader sends () here when stdin closes.
        let (eof_tx, eof_rx) = tokio::sync::oneshot::channel::<()>();

        // P9 step A7 — the boot handshake. Written here and not earlier: the
        // bootstrap above has succeeded at this point, so the frame's arrival is
        // the signal that this colony can actually answer. Synchronous and
        // before the reader task is spawned, so it is provably the first line.
        let stdio_format = cli.stdio_format;
        if stdio_format == StdioFormat::Json {
            println!("{}", bridge::ready_frame());
        }

        // Step 5.4 — stdin-reader task.
        let inbox_for_stdin = inbox_tx.clone();
        let chat_id = uuid::Uuid::now_v7();
        let stdin_handle = tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                // P9 step A6 — one bridge, two wires. The text branch is
                // untouched; the JSON branch reads the envelope the caller
                // supplied instead of synthesising one.
                let msg = match stdio_format {
                    StdioFormat::Text => {
                        bridge::line_to_message(&line, bridge::STDIO_USER_ID, chat_id)
                    }
                    StdioFormat::Json => match bridge::parse_ingress_frame(&line) {
                        Ok(frame) => {
                            bridge::frame_to_message(frame, bridge::STDIO_USER_ID, chat_id)
                        }
                        // Answer, do not swallow: the sender is a program
                        // waiting on a correlation key, and a rejected line
                        // still owes it a reply.
                        Err(reason) => {
                            println!("{}", bridge::ingress_error_frame(&reason));
                            continue;
                        }
                    },
                };
                if inbox_for_stdin
                    .send(meclaw_colony::ColonyMsg::Route {
                        sender_path: meclaw_core::Path::new("/"),
                        msg,
                    })
                    .await
                    .is_err()
                {
                    break; // colony gone
                }
            }
            let _ = eof_tx.send(());
        });

        // Step 5.5 — egress-writer task (root-hive HiveNoRoute → stdout).
        // `.map` over the Option avoids a panic-by-construction: egress_rx_direct
        // is Some in Direct-Mode, None otherwise (then no writer is spawned).
        let egress_handle = egress_rx_direct.map(|mut erx| {
            tokio::spawn(async move {
                while let Some(msg) = erx.recv().await {
                    match stdio_format {
                        StdioFormat::Text => match bridge::message_to_stdout_line(&msg) {
                            Some(line) => println!("{line}"),
                            None => {
                                tracing::warn!("egress message has no assistant turn — discarded")
                            }
                        },
                        // The JSON wire never discards: an unrepresentable body
                        // becomes a typed error frame rather than a warning
                        // nobody on the far side can see.
                        StdioFormat::Json => println!("{}", bridge::message_to_egress_frame(&msg)),
                    }
                }
            })
        });

        (Some(eof_rx), Some(stdin_handle), egress_handle)
    } else {
        // Non-Direct-Mode: no stdin reader, no egress writer, no EOF signal.
        drop(egress_rx_direct); // already None; explicit for clarity
        (None, None, None)
    };

    // U9 (RULED A8): the shutdown trigger, shared across all paths — HTTP serve
    // (`with_graceful_shutdown`) and headless (`.await`).
    // Step 5.6 — Direct-Mode EOF arm: stdin-EOF triggers the same graceful
    // shutdown as a signal. `--daemon` never reaches here (eof_rx is None →
    // the arm pends forever = EOF is ignored).
    let signal_future = async {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(error = %e, "SIGTERM-signal init failed; using ctrl_c only");
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        let shutdown_future = async {
            match shutdown_hook {
                Some(rx) => {
                    let _ = rx.await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        // Step 5.6 — EOF future: resolves on stdin-EOF (Direct-Mode only).
        // When eof_rx_for_select is None (daemon/api), this future pends forever.
        let eof_future = async {
            match eof_rx_for_select {
                Some(rx) => {
                    let _ = rx.await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
            _ = shutdown_future => {},
            // Deep-Audit F3: the heartbeat-watchdog supervisor lost the colony →
            // drive the same graceful stop as a signal.
            _ = wd_stop_rx => {
                tracing::warn!("watchdog-triggered shutdown (colony heartbeat lost)");
            },
            // Step 5.6: stdin-EOF in Direct-Mode → graceful shutdown.
            _ = eof_future => {
                tracing::info!("stdin EOF — initiating Direct-Mode shutdown");
            },
        }
    };

    // U9: --api → bind + serve the HTTP server; without --api (headless
    // `--daemon`) → no bind, just wait for the shutdown signal. The colony is
    // already running in both cases (spawn + bootstrap above).
    if let Some(bind_addr) = cli.api {
        let colony = std::sync::Arc::new(meclaw_api::ColonyHandle {
            inbox: inbox_tx.clone(),
            templates_root: templates_root.clone(),
        });
        // Phase-12-X T18: the same blob store (instantiated above) goes to the
        // router for the multipart path of POST /messages.
        let router = meclaw_api::router::build_router(colony, blob_store, message_default_ttl);

        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;
        if let Some(tx) = addr_hook {
            let _ = tx.send(local_addr);
        }

        meclaw_api::axum::serve(listener, router)
            .with_graceful_shutdown(signal_future)
            .await?;
    } else {
        // Headless (no --api): no HTTP listener. Wait for ctrl_c/SIGTERM/hook.
        signal_future.await;
    }

    // Graceful Colony-Shutdown after axum has stopped accepting + drained:
    // send Shutdown → Colony drains in-flight work + fires ack → join the task.
    // Timeouts prevent indefinite hangs if Colony is wedged.
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if inbox_tx
        .send(meclaw_colony::ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .is_ok()
    {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), ack_rx).await;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), colony_join).await;

    // Step 5.6 — Direct-Mode cleanup: abort stdin-reader (it may still be
    // blocked on stdin if shutdown was signal-triggered), then join the egress
    // writer (it drains once the colony drops the egress_tx → channel closed).
    if let Some(h) = stdin_task_handle {
        h.abort();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), h).await;
    }
    if let Some(h) = egress_task_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), h).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_defaults() {
        let cli = Cli::parse_from(["meclaw"]);
        assert_eq!(cli.root, PathBuf::from("."));
        assert_eq!(cli.log, None);
        assert_eq!(cli.log_level, "info");
        assert_eq!(cli.log_filter, None);
        assert_eq!(cli.api, None);
        assert!(!cli.daemon);
        assert!(!cli.validate);
        assert_eq!(cli.blobs, None);
    }

    // --- P9 step A5: the `--stdio-format` flag ---

    #[test]
    fn the_stdio_format_defaults_to_text() {
        // Load-bearing: every existing invocation of the bridge must keep
        // behaving byte-identically. The JSON wire is strictly opt-in.
        let cli = Cli::parse_from(["meclaw"]);
        assert_eq!(cli.stdio_format, StdioFormat::Text);
    }

    #[test]
    fn the_stdio_format_can_be_switched_to_json() {
        let cli = Cli::parse_from(["meclaw", "--stdio-format", "json"]);
        assert_eq!(cli.stdio_format, StdioFormat::Json);
    }

    #[test]
    fn an_unknown_stdio_format_is_rejected() {
        // nginx-style flags: an unknown value is an error, never a silent
        // fallback to the default — a sub-colony parent that mistypes the wire
        // must not get a text-speaking child.
        assert!(Cli::try_parse_from(["meclaw", "--stdio-format", "yaml"]).is_err());
    }

    #[test]
    fn parse_overrides() {
        let cli = Cli::parse_from([
            "meclaw",
            "--root",
            "/tmp/x",
            "--log",
            "/tmp/x/log.jsonl",
            "--log-level",
            "debug",
            "--log-filter",
            "meclaw_cli=trace",
        ]);
        assert_eq!(cli.root, PathBuf::from("/tmp/x"));
        assert_eq!(cli.log, Some(PathBuf::from("/tmp/x/log.jsonl")));
        assert_eq!(cli.log_level, "debug");
        assert_eq!(cli.log_filter, Some("meclaw_cli=trace".into()));
    }

    #[test]
    fn parse_env_flag() {
        // U7: `--env <path>` overrides the `.env` location (spec CLI table,
        // overview Z.476 — Phase-6 flag, default `<root>/.env`).
        let cli = Cli::parse_from(["meclaw", "--env", "/tmp/x/.env"]);
        assert_eq!(cli.env, Some(PathBuf::from("/tmp/x/.env")));
    }

    #[test]
    fn parse_env_flag_default_none() {
        // Absent flag ⇒ `None` ⇒ the boot path falls back to `<root>/.env`.
        let cli = Cli::parse_from(["meclaw"]);
        assert_eq!(cli.env, None);
    }

    // ---- U10: tokio-console Opt-in + Port konfigurierbar (RULED A8) ----

    #[test]
    fn tokio_console_flags_default_off_and_6669() {
        // Default: opt-out — the debug port does NOT bind (before: hardwired
        // 6669, always bound). The default port stays 6669 when enabled.
        let cli = Cli::parse_from(["meclaw"]);
        assert!(!cli.tokio_console);
        assert_eq!(cli.tokio_console_port, 6669);
    }

    #[test]
    fn tokio_console_flags_parse_override() {
        let cli = Cli::parse_from(["meclaw", "--tokio-console", "--tokio-console-port", "7000"]);
        assert!(cli.tokio_console);
        assert_eq!(cli.tokio_console_port, 7000);
    }

    #[test]
    fn check_tokio_console_disabled_yields_none_no_bind() {
        // U10-a: without the flag ⇒ no bind. None signals "layer off".
        assert!(check_tokio_console(false, 6669).unwrap().is_none());
    }

    #[test]
    fn check_tokio_console_enabled_reserves_configured_port() {
        // U10-b: enabled ⇒ Some(addr) with the configured port.
        // Determine a free port via an ephemeral bind (TOCTOU accepted, POC).
        let port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        let addr = check_tokio_console(true, port)
            .unwrap()
            .expect("enabled ⇒ Some");
        assert_eq!(addr.port(), port);
    }

    #[test]
    fn check_tokio_console_occupied_port_errors_no_panic() {
        // U10-c: an occupied port ⇒ a clear error message, NO panic (no silent
        // failure on a parallel start).
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let err = check_tokio_console(true, port).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains(&port.to_string()) && msg.to_lowercase().contains("port"),
            "the error message must name the port, was: {msg}"
        );
    }
}
