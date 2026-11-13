//! D58 close (GH #1494) -- the names of the curator's lock merge without a
//! conflict, and none stands twice.
//!
//! WHY this file exists: until the close strand the lock's names were one
//! Rust array with a count (`[&str; 31]`), so every strand that added a name
//! changed the count and collided with every other strand that added one --
//! nine strands of the stage did. The names now live one per line in
//! `support/curator_locked_names.txt`, which `.gitattributes` merges by
//! `union`: two branches that each append a name merge with both names. A
//! union can bring a line twice or an empty line, so the second test holds
//! the list itself: no empty name, no name twice, every name of the alphabet
//! of a cell, lane or hop value (`[a-z][a-z0-9_-]*`) -- red with the line
//! and the name, never silent.
//!
//! The first test runs `git merge` in a throw-away repository that carries
//! this tree's `.gitattributes` line for the file and the file itself.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use std::path::Path;
use std::process::Output;

/// The path of the names' file below the repository root.
const NAMES_PATH: &str = "crates/meclaw-cells/tests/support/curator_locked_names.txt";

fn git(dir: &Path, args: &[&str]) -> Output {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "init.defaultBranch=base",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs")
}

fn ok(dir: &Path, args: &[&str]) {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn append(dir: &Path, name: &str) {
    let p = dir.join(NAMES_PATH);
    let mut text = std::fs::read_to_string(&p).expect("the names' file");
    text.push_str(&format!("# a probe branch adds one name\n{name}\n"));
    std::fs::write(&p, text).expect("write");
}

#[test]
fn gh_d58_close_the_locked_names_merge_without_a_conflict() {
    let attributes = repo(".gitattributes");
    if !attributes.is_file() {
        return;
    }
    // The lines of this tree's `.gitattributes` that speak of the file: the
    // test proves the shipped attribute, not one of its own.
    let lines: Vec<String> = std::fs::read_to_string(&attributes)
        .expect(".gitattributes")
        .lines()
        .filter(|l| !l.starts_with('#') && l.split_whitespace().next() == Some(NAMES_PATH))
        .map(str::to_string)
        .collect();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    ok(dir, &["init", "-q"]);
    std::fs::write(dir.join(".gitattributes"), lines.join("\n") + "\n").expect("write");
    let file = dir.join(NAMES_PATH);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, LOCKED_NAMES_TEXT).expect("write");
    ok(dir, &["add", "-A"]);
    ok(dir, &["commit", "-q", "-m", "base"]);
    ok(dir, &["checkout", "-q", "-b", "strand_a"]);
    append(dir, "probe_name_a");
    ok(dir, &["commit", "-q", "-am", "a"]);
    ok(dir, &["checkout", "-q", "base"]);
    ok(dir, &["checkout", "-q", "-b", "strand_b"]);
    append(dir, "probe_name_b");
    ok(dir, &["commit", "-q", "-am", "b"]);
    let merge = git(dir, &["merge", "--no-edit", "strand_a"]);
    assert!(
        merge.status.success(),
        "two strands that each add a name merge without a conflict (attribute lines {lines:?}): {}{}",
        String::from_utf8_lossy(&merge.stdout),
        String::from_utf8_lossy(&merge.stderr)
    );
    let merged = std::fs::read_to_string(&file).expect("merged file");
    let names = locked_names_of(&merged);
    for name in LOCKED_NAMES.iter().chain(&["probe_name_a", "probe_name_b"]) {
        assert!(
            names.contains(name),
            "the merged list keeps `{name}`: {names:?}"
        );
    }
    assert_eq!(
        locked_names_problems(&merged),
        Vec::<String>::new(),
        "the merged list is a list of the lock"
    );
}

#[test]
fn gh_d58_close_no_locked_name_twice() {
    assert_eq!(
        locked_names_problems(LOCKED_NAMES_TEXT),
        Vec::<String>::new(),
        "the shipped list of the lock (`support/curator_locked_names.txt`)"
    );
    assert!(
        LOCKED_NAMES.len() >= 31,
        "check of the setup: the list is read ({} names)",
        LOCKED_NAMES.len()
    );
    // The check sees what a union can bring: a name twice, an empty line.
    let twice = format!("{LOCKED_NAMES_TEXT}# again\nbrain\n");
    let found = locked_names_problems(&twice);
    assert!(
        found.len() == 1 && found[0].contains("`brain`") && found[0].contains("twice"),
        "a name twice is red with the name: {found:?}"
    );
    let empty = format!("{LOCKED_NAMES_TEXT}\n");
    let found = locked_names_problems(&empty);
    assert!(
        found.len() == 1 && found[0].contains("empty"),
        "an empty line is red: {found:?}"
    );
    let odd = format!("{LOCKED_NAMES_TEXT}Quick Way\n");
    let found = locked_names_problems(&odd);
    assert!(
        found.len() == 1 && found[0].contains("`Quick Way`"),
        "a name outside the alphabet is red with the name: {found:?}"
    );
}
