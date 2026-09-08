//! GH #623 (review finding 1): the shipped binary is built from THIS package.
//!
//! `meclaw-cli` carries a `[[bin]] name = "meclaw"` for the workspace, and this
//! wrapper package carries the one that goes to crates.io and into the install
//! script. Both used to hold their own copy of the same `main`, and the copies
//! drifted the moment one of them learned `ask`: `--help` advertised the
//! subcommand, and running it booted a colony in the caller's working directory.
//!
//! The fix is that neither `main` decides anything any more -- both hand the
//! parsed `Cli` to `meclaw_cli::entrypoint`. This test measures the property
//! that drift broke, on the binary an installed user actually runs: `ask`
//! against an address nothing answers must fail as a client, and must leave the
//! working directory exactly as it found it.
//!
//! Deliberately dependency-free (`std` only): this package has no
//! dev-dependencies, and a test that measures the shipped binary should not be
//! the reason it grows one.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A fresh empty directory under the system temp dir, named after this process.
fn empty_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("meclaw-gh623-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn entries(dir: &PathBuf) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read_dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_shipped_binary_runs_ask_and_leaves_the_working_directory_alone() {
    let dir = empty_dir("ask");
    // Port 1 on loopback: nothing binds it, so the client fails at the
    // transport and never gets as far as an answer.
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .current_dir(&dir)
        .args(["ask", "--api", "127.0.0.1:1", "--target", "/door", "hello"])
        .output()
        .expect("spawn meclaw ask");
    let left = entries(&dir);
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        left.is_empty(),
        "`ask` runs no colony, so it writes nothing: found {left:?} (stderr={stderr:?})"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a transport failure exits 1; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(stdout, "", "nothing on stdout when nothing was answered");
    assert!(
        stderr.contains("127.0.0.1:1"),
        "the message names the address it could not reach: {stderr:?}"
    );
}

#[test]
fn the_shipped_binary_advertises_ask_in_its_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--help")
        .output()
        .expect("spawn meclaw --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ask"),
        "the help of the shipped binary lists the command it can run: {stdout}"
    );
}
