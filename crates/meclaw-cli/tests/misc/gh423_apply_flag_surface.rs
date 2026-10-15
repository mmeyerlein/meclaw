//! GH #423 — `--apply` is a FLAG, and it knows its place among the modes.
//!
//! nginx style, hard rule 9: meclaw has no subcommands. `--apply` therefore
//! declares itself like every other mode switch, and this file pins the two
//! things that are true of it before a single colony boots — its parse surface,
//! and where it sits in the precedence order.
//!
//! The precedence itself is measured where it acts (`gh423_apply_one_shot.rs`);
//! here it is only the predicate, asked without booting anything.

use clap::Parser;
use meclaw_cli::{Cli, direct_mode};
use std::path::PathBuf;

fn cli(args: &[&str]) -> Cli {
    let mut all = vec!["meclaw"];
    all.extend_from_slice(args);
    Cli::try_parse_from(all).expect("parse")
}

#[test]
fn apply_takes_a_path() {
    let c = cli(&["--root", "/x", "--apply", "f.json"]);
    assert_eq!(c.apply, Some(PathBuf::from("f.json")));
}

#[test]
fn apply_takes_a_dash_for_stdin() {
    let c = cli(&["--root", "/x", "--apply", "-"]);
    assert_eq!(c.apply, Some(PathBuf::from("-")));
}

#[test]
fn apply_without_a_value_is_a_parse_error() {
    let e = Cli::try_parse_from(["meclaw", "--root", "/x", "--apply"]);
    assert!(e.is_err(), "a value-less --apply must not parse");
}

#[test]
fn without_apply_the_field_is_none() {
    assert_eq!(cli(&["--root", "/x"]).apply, None);
}

/// `--apply` switches the stdin/stdout bridge off.
///
/// A bridge under `--apply` would sit on stdin waiting for a human while the
/// receipt it exists to print has already been written — and `--apply -` reads
/// its manifest FROM stdin, so the two cannot share it anyway.
#[test]
fn apply_switches_direct_mode_off() {
    assert!(
        direct_mode(&cli(&["--root", "/x"])),
        "no mode flag = bridge"
    );
    assert!(!direct_mode(&cli(&["--root", "/x", "--apply", "f.json"])));
    assert!(!direct_mode(&cli(&["--root", "/x", "--daemon"])));
    assert!(!direct_mode(&cli(&[
        "--root",
        "/x",
        "--api",
        "127.0.0.1:0"
    ])));
}
