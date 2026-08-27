//! `meclaw` CLI library.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod apply;
pub mod bridge;
pub mod factories;
pub mod lease;
pub mod vault_cli;
pub use factories::built_in_factories;
/// GH #84: the trip policy is a field of [`WatchdogTuning`] and of `colony.json`,
/// so the CLI re-exports the substrate's type instead of mirroring it.
pub use meclaw_colony::watchdog::{HostWitness, WatchdogOnTrip, WatchdogTrip};

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
    /// (nginx -t style). Without `--validate` the flag does nothing --
    /// hence the name, which says which mode it modifies.
    #[arg(long = "validate-strict", default_value_t = false)]
    pub validate_strict: bool,

    /// Apply a mutation manifest right after the boot: one ordered list of
    /// mutation bodies in one file, handed to `/colony/mutations` as one body.
    /// `-` reads it from stdin. Without `--daemon`/`--api` this is a one-shot --
    /// boot, apply, print the receipt, shut down; the exit code is the receipt's
    /// verdict. Against a colony that is already running, use its HTTP door: the
    /// same manifest body form travels there (`POST /colony/mutations`).
    #[arg(long, value_name = "PATH")]
    pub apply: Option<PathBuf>,

    /// GH #97: report which `params.sandbox` properties THIS HOST can enforce
    /// and exit. A question about the machine, not about a colony: it needs no
    /// root, opens no `colony.db`, spawns no cell, and exits 0 whatever the
    /// answer — the report IS the answer, and a host that can enforce nothing
    /// is not a failure of the asking. Takes precedence over every other mode.
    #[arg(long = "sandbox-probe", default_value_t = false)]
    pub sandbox_probe: bool,

    /// Blob storage directory. Default: `<root>/blobs`.
    #[arg(long, value_name = "PATH")]
    pub blobs: Option<PathBuf>,

    /// Enable the tokio-console debug layer. Opt-in: off by default. Without
    /// this flag no debug port is bound.
    ///
    /// Hidden from `--help` (`hide = true`): a debugging aid for working on the
    /// substrate itself, not part of the operator surface the flag table in
    /// `docs/meclaw-overview.md` documents. It stays fully functional.
    #[arg(long, default_value_t = false, hide = true)]
    pub tokio_console: bool,

    /// Bind port for the tokio-console layer. Only effective with
    /// `--tokio-console`. Default 6669. Hidden from `--help` for the same
    /// reason as `--tokio-console`.
    #[arg(
        long = "tokio-console-port",
        value_name = "PORT",
        default_value_t = 6669,
        hide = true
    )]
    pub tokio_console_port: u16,

    /// GH #151: the vault cell this invocation talks to, as its colony path
    /// (`/main/access/vault`). Required by every `--vault-*` mode below.
    #[arg(long, value_name = "CELL_PATH")]
    pub vault: Option<String>,

    /// Store a secret under this name in `--vault`. The secret itself is read
    /// from stdin — never from an argument, which would land it in `ps` output
    /// and in shell history. Writes straight into the vault's own database: no
    /// message, no message log, no context window.
    #[arg(long = "vault-add", value_name = "NAME")]
    pub vault_add: Option<String>,

    /// List what `--vault` holds: names and versions, never content.
    #[arg(long = "vault-status", default_value_t = false)]
    pub vault_status: bool,

    /// Revoke every active version of this name in `--vault`. Needs no
    /// passphrase — being locked out must never stop you from disabling a
    /// leaked credential.
    #[arg(long = "vault-revoke", value_name = "NAME")]
    pub vault_revoke: Option<String>,

    /// Where the vault passphrase comes from. Says SOURCE deliberately: the
    /// switch must never be able to carry key material. Default `auto` —
    /// a credentials directory (systemd) wins, else the terminal prompts.
    #[arg(
        long = "vault-key-source",
        value_name = "SOURCE",
        default_value = "auto"
    )]
    pub vault_key_source: String,

    /// Key file for `--vault-key-source plainfile`. Refused unless it is
    /// unreadable by group and others, the same answer ssh gives.
    #[arg(long = "vault-key-file", value_name = "PATH")]
    pub vault_key_file: Option<PathBuf>,

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

/// Supervisor deadline and trip policy of the Deep-Audit F3 heartbeat watchdog.
///
/// Production uses [`WatchdogTuning::default`] (5 consecutive silent periods of
/// 100 ms ≈ 0.5 s, against a colony that beats every 100 ms — a 5× margin) with
/// [`WatchdogOnTrip::Exit`]. The values are parameters of `run_watchdog` itself;
/// they are injectable here so a test can put the supervisor deadline under the
/// colony's own heartbeat period and observe a REAL trip of the real supervisor.
///
/// **GH #84**: an operator no longer needs this seam. The same three values live
/// in `colony.json` (`watchdog_threshold`, `watchdog_period_ms`,
/// `watchdog_on_trip`); [`run_with_hooks`] resolves them from there, and
/// [`WatchdogTuning::from_colony_config`] is the single conversion.
/// [`run_with_hooks_tuned`] keeps the explicit override for tests.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogTuning {
    /// Consecutive silent periods before the watchdog trips.
    pub threshold: u32,
    /// Length of one supervisor period.
    pub period: std::time::Duration,
    /// What a trip does — end the process, or report it and keep running.
    pub on_trip: WatchdogOnTrip,
}

impl Default for WatchdogTuning {
    fn default() -> Self {
        Self {
            threshold: 5,
            period: std::time::Duration::from_millis(100),
            on_trip: WatchdogOnTrip::Exit,
        }
    }
}

impl WatchdogTuning {
    /// Read the tuning out of a parsed `colony.json` (GH #84).
    ///
    /// An absent `colony.json` deserialises to [`ColonyConfig::default`], whose
    /// watchdog fields are exactly the values this struct's [`Default`] carries —
    /// so a colony that says nothing gets the pre-#84 behaviour to the millisecond.
    pub fn from_colony_config(cfg: &meclaw_colony::colony_config::ColonyConfig) -> Self {
        Self {
            threshold: cfg.watchdog_threshold,
            period: cfg.watchdog_period(),
            on_trip: cfg.watchdog_on_trip,
        }
    }
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
    run_with_hooks_tuned(cli, addr_hook, shutdown_hook, None).await
}

/// Does this invocation run the stdin/stdout bridge?
///
/// Direct mode is the ABSENCE of every mode that keeps the process busy with
/// something else: `--api` serves HTTP, `--daemon` just runs, and since GH #423
/// `--apply` boots to hand one manifest to the mutation door and then leaves.
/// A bridge under `--apply` would sit on stdin waiting for a human while the
/// receipt it exists to print has already been written — and `--apply -` reads
/// its manifest FROM stdin, so the two cannot share it anyway.
///
/// A named function rather than an inline `&&`-chain so a test can ask it
/// without booting a colony.
pub fn direct_mode(cli: &Cli) -> bool {
    cli.api.is_none() && !cli.daemon && cli.apply.is_none()
}

/// [`run_with_hooks`] with the watchdog deadline made explicit.
///
/// Same lifecycle, one extra knob: `watchdog_override` decides how much colony
/// silence is a trip and what a trip does.
///
/// * `None` — resolve from `colony.json` (GH #84). This is what `run_with_hooks`
///   passes, so the production path reads the operator's file and, when there is
///   none, runs the pre-#84 values unchanged.
/// * `Some(t)` — use `t` verbatim, ignoring `colony.json`. The test seam: a test
///   puts the supervisor deadline under the colony's own heartbeat period and
///   observes a REAL trip of the real supervisor.
pub async fn run_with_hooks_tuned(
    cli: Cli,
    addr_hook: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>,
    shutdown_hook: Option<tokio::sync::oneshot::Receiver<()>>,
    watchdog_override: Option<WatchdogTuning>,
) -> anyhow::Result<()> {
    // GH #97: --sandbox-probe answers before anything colony-shaped happens.
    // It is a question about the HOST, so it must work in a directory that is
    // not a colony at all — no colony.db, no bootstrap plan, no cell.
    if cli.sandbox_probe {
        if cli.validate || cli.api.is_some() || cli.daemon || cli.apply.is_some() {
            eprintln!(
                "note: --sandbox-probe has precedence; --validate/--api/--daemon/--apply ignored"
            );
        }
        print!(
            "{}",
            meclaw_cells::sandbox::probe::probe_host(
                &meclaw_cells::sandbox::probe::SpawningProbes::Run
            )
            .render()
        );
        return Ok(());
    }

    // GH #151: the vault user channel. Like `--sandbox-probe` it answers
    // before anything colony-shaped happens, and for a sharper reason: these
    // modes must NOT boot a colony. A secret that travels no edge cannot be
    // read off one, and a mode that spawned cells to store a credential would
    // hand that credential to a running tree.
    if cli.vault_add.is_some() || cli.vault_status || cli.vault_revoke.is_some() {
        let cell_path = cli.vault.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "--vault-add/--vault-status/--vault-revoke need --vault <CELL_PATH>, the vault \
                 cell's colony path (e.g. --vault /main/access/vault)"
            )
        })?;
        let chosen: Vec<&str> = [
            cli.vault_add.as_ref().map(|_| "--vault-add"),
            cli.vault_status.then_some("--vault-status"),
            cli.vault_revoke.as_ref().map(|_| "--vault-revoke"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if chosen.len() > 1 {
            anyhow::bail!("one vault mode at a time, got {}", chosen.join(" and "));
        }
        let command = if let Some(name) = cli.vault_add.clone() {
            crate::vault_cli::VaultCommand::Add(name)
        } else if let Some(name) = cli.vault_revoke.clone() {
            crate::vault_cli::VaultCommand::Revoke(name)
        } else {
            crate::vault_cli::VaultCommand::Status
        };
        return crate::vault_cli::run(
            &cli.root,
            cell_path,
            command,
            &cli.vault_key_source,
            cli.vault_key_file.as_deref(),
        );
    }

    // GH #121: the root lease, and it comes FIRST.
    //
    // Two daemons on one `{root}` used to boot side by side, share the WAL and
    // spawn a duplicate of every cell with a duplicate of every child process;
    // `busy_timeout` serialises those writes but guards nothing. So the lease is
    // taken before `colony.db` is opened, before the template scan, and before
    // any registry read or write — every line below this one assumes it is the
    // only colony on this root.
    //
    // Track-Ruling G7-R2: `--validate` takes the lease too. It is a dry run for
    // cells, but not for the database — `boot_load_or_scan` below writes a full
    // template index whenever the stored one is empty or belongs to another
    // root, and that write lands in the live colony's `colony.db`. Only
    // `--sandbox-probe` stays exempt, and it has already returned above: it asks
    // about the host, not about a colony, and touches neither.
    //
    // Ordering against GH #116 (orphan reap): the lease is strictly first. The
    // reap reads the orphan journal and kills processes a crashed daemon left
    // behind — an operation that must never run while another daemon is live on
    // the same root, because those "orphans" would be its children. The lease
    // is what makes that safe, so it sits above every boot step, the reap
    // included.
    //
    // The guard lives to the end of `run_with_hooks_tuned`, so every exit path —
    // `--validate`'s early return, a `?`, a panic unwind, SIGTERM's graceful
    // shutdown — releases the root. The two `std::process::exit` calls below
    // skip `Drop` by design; the lease they leave is a stale one, which the next
    // boot reclaims after verifying the holder is gone.
    let _root_lease = lease::acquire(&cli.root)?;

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

    // GH #98 (Track-Ruling N1, 2026-08-13): a graceful shutdown instead of a
    // plain `drop`. `drop` only releases the writer sender — the writer thread
    // then drains, commits and CLOSES its connection asynchronously (a WAL
    // close-checkpoint takes exclusive locks), racing the spawn re-open below;
    // under full parallel load that race exhausted even a 5 s busy wait and
    // killed the boot with "database is locked". `shutdown_async` sends the
    // explicit shutdown op, awaits the post-commit ack and joins the thread,
    // so the re-open never races the template-scan writer of the SAME process.
    // Cross-process contention stays covered by the explicit 30 s busy budget
    // (`meclaw_colony::persist::DB_BUSY_TIMEOUT`).
    // GH #424: the template rows a planned growth would resolve against, read
    // BEFORE the handle is given up — the scan above has just filled the table,
    // and `--validate` (below) has no colony to ask.
    let scanned_templates = colony_db.read_templates().unwrap_or_default();
    colony_db.shutdown_async().await;

    // --validate: Dry-Run-Vorrang (Spec Z.430).
    //
    // Phase-12-close hardening (T28): in addition to the existing templates
    // scan, two further checks run **without** spawning colony/axum
    // (--validate returns before the `colony_task::spawn` at Z.~210):
    //   1. `plan_bootstrap` — the filesystem bootstrap plan (MultipleRootDirs /
    //      NoRootDir / InvalidParams / CorruptCellDb).
    //   2. `probe_boot_state` — colony.db consistency. Since GH #89 the probe
    //      flags only unreadable persistence tables as
    //      `BootState::Inconsistent`; count-level "mixed" states are
    //      legitimate Reboots (edge-less colonies, hive-only roots).
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
        if cli.api.is_some() || cli.daemon || cli.apply.is_some() {
            eprintln!("note: --validate has precedence; --api/--daemon/--apply ignored");
        }
        let mut had_error = false;
        // GH #97: does anything in this tree ask to be sandboxed? Decides
        // whether the appendix below may fork the two spawning probes. A tree
        // whose plan did not even come together answers "no" — there is
        // nothing to spawn on behalf of.
        let mut tree_declares_restricted = false;

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
        // GH #424: the registry a planned growth would resolve against. The
        // template scan above (`boot_load_or_scan`) has already run, so this is
        // the very table the boot itself would ask.
        let validate_templates = meclaw_colony::templates::TemplatesRegistry::from_entries(
            scanned_templates
                .iter()
                .cloned()
                .map(|r| meclaw_colony::templates::TemplateEntry {
                    template_id: r.template_id,
                    name: r.name,
                    version: r.version,
                    filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                })
                .collect(),
        );
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
                tree_declares_restricted = plan
                    .cells
                    .iter()
                    .any(|c| declares_restricted_sandbox(&c.params));
                // A8 (Phase-16 W1a, Ruling 2026-06-12): static endpoint-existence
                // check. `--validate` has no running colony, so it cannot see
                // runtime-spawned cells — an unresolved edge endpoint is a
                // WARNING (exit 0). `--validate-strict` promotes it to a hard error
                // (operator decides, nginx -t style).
                //
                // GH #168: on a Reboot the plan carries the PERSISTED edges, whose
                // endpoints may legitimately be a node whose directory is gone but
                // whose registry row / hive scope survives (No-Delete). Both
                // persisted universes therefore join the resolvable set — the same
                // two terms the live boot path uses, minus the running registry it
                // has and this dry run does not.
                let mut known: std::collections::HashSet<String> = validate_overlay
                    .keys()
                    .map(|p| p.as_str().to_string())
                    .collect();
                known.extend(meclaw_colony::registered_hive_paths(&cli.root));
                // GH #285: a hive's DECLARED slot is an address that may stand
                // empty, so an edge onto it is the edge the declaration invited
                // — not a typo. Undeclared endpoints are untouched, which is
                // the point: the exemption is bought by the declaration.
                let slot_endpoints = meclaw_colony::declared_slot_endpoints(&cli.root, &plan);
                let unresolved =
                    meclaw_colony::unresolved_boot_endpoints(&plan, &known, &slot_endpoints);
                for (edge_id, endpoint) in &unresolved {
                    eprintln!(
                        "validate: warning: dangling edge endpoint {} (edge {edge_id}) — \
                         resolves to no FS cell/hive, /colony endpoint or declared \
                         `params.ports` slot (runtime-spawned cells are invisible to \
                         --validate)",
                        endpoint.as_str()
                    );
                }
                if cli.validate_strict && !unresolved.is_empty() {
                    had_error = true;
                }
                // A5b (Phase-16 W1b): on a Reboot, unknown (unregistered) cell
                // dirs are a consistency drift — reported as a WARNING (exit 0,
                // nginx -t role); `--validate-strict` promotes them to a hard error.
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
                if cli.validate_strict && !plan.unregistered_nodes.is_empty() {
                    had_error = true;
                }
                // GH #178: the pre-flight surface for header-contract violations
                // in the topology a reboot will actually run. The live boot warns
                // and starts (committed state must not become a crash loop); here
                // is where an operator — or CI — asks the question BEFORE the
                // restart, and `--validate-strict` makes the answer binding.
                for finding in &plan.header_contract_findings {
                    eprintln!("validate: warning: header-contract violation: {finding}");
                }
                if cli.validate_strict && !plan.header_contract_findings.is_empty() {
                    had_error = true;
                }
                // GH #283 (ruling Q1 2026-08-21): the fourth channel, and the
                // only one WITHOUT a `validate_strict` promotion under it. An
                // unguarded default edge is a legal, working topology — the
                // ruling asked for a hint and explicitly refused a refusal, so
                // no flag turns this into a non-zero exit. Printed as `note`
                // rather than `warning` so the difference is visible in the
                // output too. Pinned by
                // `tests/phase_16_w1a_validate_strict.rs::an_unguarded_default_is_a_note_not_a_strict_error`
                // — a later reviewer who moves this onto a promoting channel
                // turns that test red.
                for advisory in &plan.advisories {
                    eprintln!("validate: note: {advisory}");
                }
                // GH #424: what the first boot will GROW, listed — and nothing
                // grown. `--validate` promises to touch nothing (it is the
                // nginx -t role), and a dry run that created directories would
                // be a broken promise. What it CAN check is resolvability, and
                // that it does: an unresolvable reference is a hard error even
                // WITHOUT `--validate-strict`, because it is not a warning
                // class — it is a tree that is guaranteed not to boot, the same
                // sharpness `DanglingEndpoint` already has.
                for g in &plan.growths {
                    println!("validate: growth: {} → {}", g.path.as_str(), g.reference);
                }
                let unresolvable: Vec<&meclaw_colony::PlannedGrowth> = plan
                    .growths
                    .iter()
                    .filter(|g| validate_templates.resolve(&g.reference).is_err())
                    .collect();
                for g in &unresolvable {
                    eprintln!(
                        "validate: growth {} references {}, which no template in this tree \
                         provides — this boot cannot fulfil it",
                        g.path.as_str(),
                        g.reference
                    );
                }
                if !unresolvable.is_empty() {
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

        // 4. GH #97: the host-capability appendix. STRICTLY informative — it
        //    never touches `had_error`. `--validate` checks the tree; whether
        //    this machine can enforce what the tree declares is a different
        //    question, and the fail-closed refusal that answers it happens at
        //    spawn time. Printing it here means an operator who runs the usual
        //    pre-flight check sees the host answer without asking for it.
        //
        //    The two probes that fork `/bin/sh -c :` run only when the tree
        //    declares a `restricted` profile at all: a configuration check
        //    spawns nothing without cause. On stderr, with every other
        //    `--validate` diagnostic (`--sandbox-probe` is the stdout surface).
        eprint!(
            "{}",
            meclaw_cells::sandbox::probe::probe_host(&if tree_declares_restricted {
                meclaw_cells::sandbox::probe::SpawningProbes::Run
            } else {
                meclaw_cells::sandbox::probe::SpawningProbes::Skip(
                    "no restricted profile in tree".to_string(),
                )
            })
            .render()
        );

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
    let is_direct_mode = direct_mode(&cli);

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

    // GH #116 — orphan reap. Position is load-bearing in both directions:
    //
    //   * AFTER the `--validate` / `--sandbox-probe` returns above: those are
    //     dry runs and must not kill anything, and `--validate` promises to be
    //     side-effect free.
    //   * BEFORE the `colony_task` spawn below, and therefore before any cell
    //     exists: the journal the reaper folds must contain only the PREVIOUS
    //     run's entries, and the install must be in place before the first tool
    //     child is spawned.
    //   * AFTER the root lease (GH #121, track G7), once that lands: the lease
    //     is what makes "no other daemon owns this root" a fact rather than an
    //     inference. Until then the reaper carries its own guard — an entry
    //     whose owning daemon is still alive is left untouched — so the two
    //     mechanisms compose in either merge order (see the track receipt).
    let reap = meclaw_cells::orphan_journal::boot(&cli.root);
    if reap.reaped > 0 || reap.skipped > 0 {
        tracing::warn!(
            examined = reap.examined,
            reaped = reap.reaped,
            gone = reap.gone,
            skipped = reap.skipped,
            owned_by_live_daemon = reap.owned_by_live_daemon,
            "orphan journal: previous run left tool children behind"
        );
    } else {
        tracing::info!(
            examined = reap.examined,
            gone = reap.gone,
            owned_by_live_daemon = reap.owned_by_live_daemon,
            "orphan journal: boot reap clean"
        );
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

    // GH #84: the watchdog deadline is an operator knob now. An explicit override
    // (tests) wins; otherwise `colony.json` decides — and an absent file carries
    // the pre-#84 values, so nothing moves by default.
    let watchdog =
        watchdog_override.unwrap_or_else(|| WatchdogTuning::from_colony_config(&colony_config));

    // Phase-12-X T17 / phase-13.5 A8: the blob store is instantiated here.
    // Default path `<root>/blobs`; --blobs overrides it. It flows both into
    // `colony_task`/`runtime` (A8 — cell delivery-boundary resolution +
    // auto-offload) and into the router (T18 — the multipart path of
    // POST /messages).
    let blob_root = cli.blobs.clone().unwrap_or_else(|| cli.root.join("blobs"));
    let blob_store = std::sync::Arc::new(
        meclaw_colony::blob::DiskBlobStore::new(&blob_root)
            .map_err(|e| anyhow::anyhow!("open blob store {}: {e}", blob_root.display()))?
            // GH #19: the bound for recursive in-message pointer resolution
            // rides on the store, which is the handle the delivery boundary
            // already holds. `blob_max_recursion_depth` stops being
            // parsed-but-not-applied here.
            .with_max_recursion_depth(colony_config.blob_max_recursion_depth),
    );

    let inbox_self_tx = inbox_tx.clone();
    // Deep-Audit F3: heartbeat channel — the colony loop emits ~10×/s; the
    // supervisor below counts misses and triggers a clean stop on colony death.
    let (heartbeat_tx, heartbeat_rx) =
        tokio::sync::mpsc::channel::<meclaw_colony::watchdog::Beat>(8);

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
    // GH #277: the same library the boot scan and `ColonyHandle` use, so a
    // rescan triggered from inside the colony walks `--templates` and not the
    // whole workspace.
    .with_templates_root(templates_root.clone())
    .with_heartbeat(heartbeat_tx);
    if let Some(egress_tx) = egress_tx_opt {
        colony_cfg = colony_cfg.with_egress(egress_tx);
    }

    // GH #383 — `--api` opens no second door any more. GH #159 gave this branch a
    // marked egress channel and a `Dispatcher`, because a surface was rendered by a
    // cell and the HTML had to travel back through the HTTP layer that asked for it.
    // A display is a `web` cell now: it owns its own listener, so its answer never
    // leaves the colony as a message at all, and `--api` is back to serving the
    // operator UI and the colony endpoints. `EgressPolicy::Marked` stays in the
    // substrate — this was its only caller, not its only reason.
    //
    // Direct-Mode's own door (`with_egress`, above) is untouched: it is the stdio
    // bridge's way out and always was a different policy (`All`, root only).

    let colony_join = tokio::spawn(meclaw_colony::colony_task(colony_cfg));

    // Deep-Audit F3: heartbeat-watchdog supervisor. Lives OUTSIDE the colony task
    // so a colony panic (loop gone → heartbeats stop) is observable. `threshold`
    // consecutive missed periods → stop (NO restart: a Tokio task's state is not
    // revivable; Ops/boot-supervisor restarts MeClaw). Fires the same graceful
    // shutdown path as SIGTERM via `wd_stop` — but, unlike a signal, it ends the
    // process with a NON-ZERO exit code (issue #6).
    //
    // Issue #6, defect 1: the supervisor task is spawned here but stays DISARMED
    // until `wd_arm_tx` fires, which happens only after the filesystem bootstrap
    // has completed. Boot is not a steady state — the colony task hydrates its
    // tables before its select-loop emits its first heartbeat — so a boot that
    // ran long under parallel load used to trip a watchdog armed at spawn time.
    //
    // GH #84: the supervisor reports a structured `WatchdogTrip` instead of a
    // bare `()`. Every trip is logged (below, once the boot cell count is known);
    // only a FATAL one — any trip under `on_trip: exit`, and a gone colony task
    // under either policy — reaches `wd_fatal_rx` and ends the process.
    let (wd_trip_tx, mut wd_trip_rx) =
        tokio::sync::mpsc::channel::<meclaw_colony::watchdog::WatchdogTrip>(8);
    let (wd_fatal_tx, wd_fatal_rx) =
        tokio::sync::oneshot::channel::<meclaw_colony::watchdog::WatchdogTrip>();
    let (wd_arm_tx, wd_arm_rx) = tokio::sync::oneshot::channel::<()>();
    // GH #165: the second observer. A supervisor that only sleeps cannot tell a
    // wedged colony loop from a host that is not getting anything done — it wakes
    // on time either way, which is how a compile on the same box killed a healthy
    // colony three times in a day with `supervisor_lag=0ms` in the record. This
    // task has to FINISH WORK once per supervisor period; the supervisor holds it
    // to the same bar it holds the colony to, and a colony trip whose witness
    // failed the same test never ends the process.
    let (wd_witness_tx, wd_witness_rx) = tokio::sync::mpsc::channel::<()>(8);
    tokio::spawn(meclaw_colony::watchdog::run_liveness_witness(
        wd_witness_tx,
        watchdog.period,
    ));
    tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        heartbeat_rx,
        wd_trip_tx,
        watchdog.threshold,
        watchdog.period,
        wd_arm_rx,
        watchdog.on_trip,
        Some(wd_witness_rx),
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
    // GH #84: how many cells the boot registered. The watchdog cannot ask the
    // colony how many are live AT trip time — the colony is the only authority on
    // its registry and by definition it is not answering — so the trip line
    // carries the honest number it can have: the one the boot produced.
    let cells_at_boot;
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
            cells_at_boot = report.cell_count;
            // Issue #6: boot is over — from here on, silence from the colony
            // loop is a fault and not a slow start. This is the ONLY arming
            // site; the failure branch below returns without arming.
            let _ = wd_arm_tx.send(());
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

    // GH #423 — `--apply`: one manifest, handed to the mutation door, right
    // here.
    //
    // The position is the whole argument: only AFTER the boot does the tree
    // stand that the manifest mutates. A `--apply` before it would mutate
    // against an empty registry and refuse everything with a straight face.
    //
    // In-process, not HTTP (orchestrator ruling O5): `lease::acquire` is the
    // FIRST thing this function does, so a second `meclaw --root X --apply f`
    // against a running daemon on `X` never boots at all — it gets
    // `LeaseError::Held`, which is the right answer. Against a colony that is
    // already up one mutates through its HTTP door, and since R5 that is one
    // `curl` instead of five. In our own process the door is the same
    // `ColonyMsg::MutationDoor` `post_mutation` sends, one hop shorter and with
    // no address to spell wrong.
    let apply_verdict = if let Some(source) = cli.apply.clone() {
        let body = crate::apply::read_manifest_source(&source, &mut std::io::stdin())?;
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        inbox_tx
            .send(meclaw_colony::ColonyMsg::MutationDoor {
                payload: body,
                reply_to: None,
                trace_id: meclaw_core::Uuid::now_v7(),
                parent_message_id: meclaw_core::Uuid::nil(),
                ack: ack_tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("colony inbox closed before --apply"))?;
        let outcome = ack_rx
            .await
            .map_err(|_| anyhow::anyhow!("colony dropped the --apply ack"))?;
        // A manifest is what `--apply` accepts (`read_manifest_source` refuses
        // anything else), so the door answers in the manifest form or says the
        // form was unreadable. Both are rendered for a terminal here; the exit
        // code is the contract, the text is for a human.
        let (rendered, committed) = match &outcome {
            meclaw_colony::MutationDoorOutcome::Manifest(m) => {
                (crate::apply::render_receipt(m), outcome.is_committed())
            }
            meclaw_colony::MutationDoorOutcome::MalformedManifest(e) => {
                (format!("the manifest could not be read: {e}\n"), false)
            }
            // Unreachable through `read_manifest_source`, and written out
            // rather than `unreachable!()` because a panic here would take the
            // colony's process down over a message it could have printed.
            meclaw_colony::MutationDoorOutcome::Single(_) => (
                "--apply: the door answered as a single mutation; \
                 the body was not a manifest after all\n"
                    .to_string(),
                outcome.is_committed(),
            ),
        };
        // Committed goes to stdout, refused to stderr: a script that pipes
        // `--apply` then gets nothing false on stdout, and a refusal lands in
        // the same stream as every other diagnostic.
        if committed {
            print!("{rendered}");
        } else {
            eprint!("{rendered}");
        }
        Some(committed)
    } else {
        None
    };

    // GH #423 — the one-shot: without `--daemon`/`--api` the process has said
    // everything it came to say.
    //
    // The shutdown is the one the bootstrap failure branch above already runs —
    // Shutdown + ack + join, each with its own timeout. Deliberately NOT
    // `std::process::exit`: that would skip the lease guard's `Drop` and leave
    // a stale lease behind for the next boot to reclaim.
    if let Some(committed) = apply_verdict
        && !cli.daemon
        && cli.api.is_none()
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let _ = inbox_tx
            .send(meclaw_colony::ColonyMsg::Shutdown { ack: ack_tx })
            .await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), ack_rx).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), colony_join).await;
        // The exit-code contract of the overview § CLI: 0 means it worked,
        // anything else means it did not, and no code carries a diagnosis —
        // the receipt already printed one.
        return if committed {
            Ok(())
        } else {
            Err(anyhow::anyhow!("--apply: the manifest was refused"))
        };
    }

    // GH #84, half 3: a trip becomes a reported event. This task is the single
    // place a trip is written down — under both policies, so `log-only` can never
    // be the quiet option — and the single place that decides whether it also
    // ends the process.
    //
    // stderr AND `tracing`, deliberately (issue #6, defect 3): `tracing` goes to
    // the structured JSON log file, stderr is the one stream an operator gets
    // without configuring anything (journalctl, the scenario runner's daemon.log).
    let on_trip = watchdog.on_trip;
    tokio::spawn(async move {
        let mut fatal_tx = Some(wd_fatal_tx);
        while let Some(trip) = wd_trip_rx.recv().await {
            let fatal = trip.is_fatal(on_trip);
            let line = format!("{trip} cells_at_boot={cells_at_boot} on_trip={on_trip}");
            if fatal {
                eprintln!("meclaw: watchdog trip — {line}");
            } else if trip.witness() == meclaw_colony::watchdog::HostWitness::Failed {
                // GH #165: reported, not acted on. The independent witness missed
                // the same window, so this observation says something about the
                // host and nothing about the colony.
                eprintln!(
                    "meclaw: watchdog trip (uncorroborated — an independent worker \
                     missed the same window; the colony keeps running) — {line}"
                );
            } else {
                eprintln!("meclaw: watchdog trip (log-only, the colony keeps running) — {line}");
            }
            tracing::error!(
                reason = %line,
                starved = trip.starved(),
                fatal = fatal,
                "watchdog trip"
            );
            if fatal {
                if let Some(tx) = fatal_tx.take() {
                    let _ = tx.send(trip);
                }
                return;
            }
        }
    });

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
    // Issue #6, defect 2: the trip REASON has to leave the shutdown future, and
    // that future must stay `Output = ()` for axum's graceful-shutdown contract.
    // So it travels on its own one-slot channel, read after the drain below.
    let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<String>(1);
    let signal_future = async {
        // Explicit binding: capture by value, not by reference (see above).
        let trip_tx = trip_tx;
        // Issue #40: SIGTERM exists only on unix. Elsewhere the arm below is
        // compiled out and ctrl_c alone carries the shutdown.
        #[cfg(unix)]
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
        // select! takes no cfg on branches, so the platform split lives in a
        // pre-built future: unix awaits SIGTERM, everything else pends forever.
        #[cfg(unix)]
        let term_future = async move { term.recv().await };
        #[cfg(not(unix))]
        let term_future = std::future::pending::<Option<()>>();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term_future => {},
            _ = shutdown_future => {},
            // Deep-Audit F3: the heartbeat-watchdog supervisor lost the colony →
            // drive the same graceful stop as a signal, but end the process as a
            // FAULT (issue #6, defect 2 — a trip used to exit 0, so a supervisor
            // saw a clean stop and neither restarted nor alerted).
            // GH #84: only a FATAL trip arrives here — the reporter task above has
            // already written every trip down. `Ok(..)` and not `_`: if the
            // reporter ends without a fatal trip (log-only, colony still running)
            // the sender drops and the pattern simply disables this arm, instead
            // of firing a shutdown on a channel-closed.
            Ok(trip) = wd_fatal_rx => {
                let _ = trip_tx.try_send(format!("{trip}"));
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

    // Issue #6, defect 2: the shutdown itself was graceful, its CAUSE was not.
    // A watchdog trip leaves the process with a non-zero exit code so that a
    // supervisor restarts and an alert fires; every other cause still exits 0.
    if let Ok(reason) = trip_rx.try_recv() {
        return Err(anyhow::anyhow!("watchdog trip: {reason}"));
    }

    Ok(())
}

/// Whether a cell's birth params ask for an ENFORCED sandbox (GH #97).
///
/// The `--validate` appendix uses this as the cause for forking the two
/// spawning probes. Deliberately conservative in one direction: a `sandbox`
/// block this parser cannot read counts as a declaration, because whoever
/// wrote it wanted enforcement and deserves to see what the host can do —
/// and a malformed block fails the boot on its own anyway.
///
/// `trust: "trusted"` is the declared escape hatch and asks for nothing, so it
/// is not a cause.
pub fn declares_restricted_sandbox(params: &meclaw_core::JsonValue) -> bool {
    use meclaw_cells::sandbox::SandboxProfile;
    match SandboxProfile::parse(params) {
        Ok(Some(SandboxProfile::Restricted { .. })) => true,
        Ok(_) => false,
        Err(_) => params.get("sandbox").is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ---- GH #97: what counts as a cause for the spawning probes ----

    #[test]
    fn a_restricted_profile_is_a_cause_and_a_trusted_one_is_not() {
        let restricted: meclaw_core::JsonValue = serde_json::from_str(
            r#"{"sandbox":{"trust":"restricted","filesystem":{"read":["/usr"]}}}"#,
        )
        .unwrap();
        assert!(declares_restricted_sandbox(&restricted));

        let trusted: meclaw_core::JsonValue =
            serde_json::from_str(r#"{"sandbox":{"trust":"trusted"}}"#).unwrap();
        assert!(
            !declares_restricted_sandbox(&trusted),
            "the escape hatch asks for no enforcement, so it is no cause to spawn"
        );
    }

    #[test]
    fn no_sandbox_block_is_no_cause() {
        let plain: meclaw_core::JsonValue =
            serde_json::from_str(r#"{"external_timeout_ms":1000}"#).unwrap();
        assert!(!declares_restricted_sandbox(&plain));
    }

    #[test]
    fn an_unreadable_sandbox_block_still_counts_as_asking() {
        // Whoever wrote it wanted enforcement; reporting the host's answer is
        // more useful than silently treating the typo as "no sandbox".
        let broken: meclaw_core::JsonValue =
            serde_json::from_str(r#"{"sandbox":{"trust":"restrictd"}}"#).unwrap();
        assert!(declares_restricted_sandbox(&broken));
    }

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
