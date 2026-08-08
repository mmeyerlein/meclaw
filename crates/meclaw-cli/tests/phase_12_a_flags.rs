//! Phase-12-A TDD anchor: top-level flags (CLAUDE.md R9 — no subcommands).
//! --validate takes precedence, with a one-line stderr note when --api/--daemon
//! stand alongside it (no error). --daemon without --api is allowed.

use clap::Parser;
use meclaw_cli::Cli;
use std::net::SocketAddr;

#[test]
fn parses_api_daemon_validate_blobs_flags() {
    let cli = Cli::parse_from([
        "meclaw",
        "--api",
        "127.0.0.1:7777",
        "--daemon",
        "--blobs",
        "/tmp/b",
    ]);
    assert_eq!(
        cli.api,
        Some("127.0.0.1:7777".parse::<SocketAddr>().unwrap())
    );
    assert!(cli.daemon);
    assert_eq!(cli.blobs, Some(std::path::PathBuf::from("/tmp/b")));
    assert!(!cli.validate);
}

#[test]
fn validate_without_api_or_daemon_parses_clean() {
    let cli = Cli::parse_from(["meclaw", "--validate"]);
    assert!(cli.validate);
    assert_eq!(cli.api, None);
    assert!(!cli.daemon);
}

#[test]
fn daemon_without_api_is_allowed() {
    let cli = Cli::parse_from(["meclaw", "--daemon"]);
    assert!(cli.daemon);
    assert_eq!(cli.api, None);
}
