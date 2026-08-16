use clap::Parser;
use meclaw_cli::Cli;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // GH #97: `--sandbox-probe` asks about the host, not about a colony. It is
    // routinely run from a directory that is not one, so it skips the
    // subscriber for the same reason `--help`/`--version` do: an operator
    // asking a question must not find a `log.jsonl` afterwards.
    if cli.sandbox_probe {
        return meclaw_cli::run(cli).await;
    }
    // U6 (roadmap 2026-06-11): wire the global tracing subscriber in the
    // binary entrypoint, before any colony work. clap has already
    // short-circuited `--help`/`--version`, so the info-only flags stay
    // side-effect-free (spec § CLI: no log.jsonl creation there). The guard
    // must outlive `run` — Drop flushes the non-blocking appender.
    let log_path = cli
        .log
        .clone()
        .unwrap_or_else(|| cli.root.join("log.jsonl"));
    let _log_guard = meclaw_cli::setup_subscriber(
        &log_path,
        &cli.log_level,
        cli.log_filter.as_deref(),
        cli.tokio_console,
        cli.tokio_console_port,
    )?;
    meclaw_cli::run(cli).await
}
