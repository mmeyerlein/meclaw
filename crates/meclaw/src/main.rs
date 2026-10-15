use clap::Parser;
use meclaw_cli::Cli;

/// The published `meclaw` binary — the one the install script fetches and
/// crates.io ships. It decides nothing itself: the parsed `Cli` goes to
/// [`meclaw_cli::entrypoint`], which owns the mode order, the tracing
/// subscriber and the `ask` dispatch. Since GH #1168 it is also the only
/// `meclaw` binary in the workspace: the release builds this package, and the
/// tests that start the binary live here, so no second copy can disagree with
/// it about what a command does (GH #623 review).
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    meclaw_cli::entrypoint(Cli::parse()).await
}
