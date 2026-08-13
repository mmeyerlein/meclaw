//! GH #97: an operator can ask which sandbox properties this host can enforce
//! WITHOUT running a cell.
//!
//! The sandbox is fail-closed, so before this flag the first news of a host
//! that cannot enforce something was an `io_error` from a production cell. The
//! four probes existed since GH #85; they had no surface.
//!
//! What these tests pin is the FORM and the COMPLETENESS of the answer, never
//! the verdicts themselves: whether this machine has Landlock, may unshare a
//! network namespace or was started as a systemd user unit is a property of
//! the machine running `cargo test`, and a test that pinned it would go red on
//! the next host instead of reporting.

use clap::Parser;
use meclaw_cli::Cli;
use std::process::Command;

/// The verdict words the report may use. Closed set.
const VERDICTS: [&str; 3] = ["yes", "no", "skipped"];

/// Every property the report must cover, in print order.
const NAMES: [&str; 4] = ["filesystem", "network", "limits", "syscalls"];

/// Pull the report block out of a captured stream and return one
/// `(name, verdict, detail)` triple per property.
fn parse_report(stream: &str) -> Vec<(String, String, String)> {
    let header = meclaw_cells::sandbox::probe::REPORT_HEADER;
    let after = stream
        .split_once(header)
        .unwrap_or_else(|| panic!("the report header is missing from:\n{stream}"))
        .1;
    after
        .lines()
        .filter(|l| l.starts_with("  "))
        .take(NAMES.len())
        .map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next().unwrap_or_default().to_string();
            let verdict = it.next().unwrap_or_default().to_string();
            let detail = it.collect::<Vec<_>>().join(" ");
            (name, verdict, detail)
        })
        .collect()
}

/// Assert the shape every report has to have, wherever it was printed.
fn assert_well_formed(rows: &[(String, String, String)]) {
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        names,
        NAMES.to_vec(),
        "all four properties, in order — an operator must not have to know which one is missing"
    );
    for (name, verdict, detail) in rows {
        assert!(
            VERDICTS.contains(&verdict.as_str()),
            "{name}: verdict {verdict:?} is outside the closed set {VERDICTS:?}"
        );
        assert!(!detail.is_empty(), "{name}: a verdict without a reason");
    }
}

// ---- the flag itself -------------------------------------------------------

#[test]
fn the_flag_is_a_flag_and_defaults_off() {
    // nginx style (CONTRIBUTING R9 / spec § CLI): a flag on the one binary,
    // never a subcommand.
    assert!(!Cli::parse_from(["meclaw"]).sandbox_probe);
    assert!(Cli::parse_from(["meclaw", "--sandbox-probe"]).sandbox_probe);
    assert!(Cli::try_parse_from(["meclaw", "sandbox-probe"]).is_err());
}

// ---- solo: the report is the answer ---------------------------------------

#[test]
fn sandbox_probe_reports_all_four_properties_without_a_colony_and_exits_zero() {
    // The point of the flag: a pure question about the host. No colony root,
    // no colony.db, no cell. A host that can enforce nothing is a legitimate
    // answer and not a failure of the asking, so the exit code stays 0.
    let td = tempfile::TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--sandbox-probe")
        .current_dir(td.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "a non-enforcing host is not an error of the probe call; exit was {:?}, stderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let rows = parse_report(&stdout);
    assert_well_formed(&rows);
    for (name, verdict, _) in &rows {
        assert_ne!(
            verdict, "skipped",
            "{name}: an explicit --sandbox-probe runs every probe, spawns included"
        );
    }

    // An empty directory is not a colony and must not become one.
    assert!(
        !td.path().join("colony.db").exists(),
        "--sandbox-probe must not create a colony.db"
    );
    assert!(
        !td.path().join("log.jsonl").exists(),
        "--sandbox-probe must not materialise a log next to a directory that is not a colony"
    );
}

#[test]
fn the_limits_line_names_the_launch_requirement_when_the_move_is_refused() {
    // The cgroup answer is not a property of the kernel but of how the daemon
    // was started, so a bare "no" would send the operator hunting for a kernel
    // feature that is already there. Host-dependent, hence a conditional
    // assertion with a visible skip line rather than a pinned verdict.
    let td = tempfile::TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--sandbox-probe")
        .current_dir(td.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let rows = parse_report(&stdout);
    let (_, verdict, detail) = rows
        .iter()
        .find(|r| r.0 == "limits")
        .expect("the limits line is always present");
    if verdict == "yes" {
        eprintln!(
            "[the_limits_line_names_the_launch_requirement_when_the_move_is_refused] \
             SKIPPED: this host delegates a usable cgroup — the refusal branch is covered by \
             the unit tests in meclaw-cells sandbox::probe"
        );
        return;
    }
    assert!(
        detail.contains("systemd user unit") || detail.contains("cgroup v2"),
        "a refusal must say whether the mechanism or the launch is missing, was: {detail}"
    );
}

// ---- appended to --validate ------------------------------------------------

/// A tree whose single cell declares an ENFORCED sandbox profile.
fn tree_with_restricted_profile(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo/a")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("demo/a/config.json"),
        br#"{"cell":{"type":"bash"},"params":{"sandbox":{"trust":"restricted","network":"deny","filesystem":{"read":["/usr"],"runtime":true}}},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// The same tree, without any `params.sandbox` block.
fn tree_without_sandbox(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo/a")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("demo/a/config.json"),
        br#"{"cell":{"type":"bash"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

fn run_validate(root: &std::path::Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(root)
        .arg("--validate")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn validate_appends_the_report_and_spawns_nothing_without_a_restricted_profile() {
    // Ruling: the appendix is on the validation path, but a configuration
    // check must not fork children for a tree that never asked to be
    // sandboxed. The two side-effect-free probes still answer.
    let td = tempfile::TempDir::new().unwrap();
    tree_without_sandbox(td.path());
    let (ok, stderr) = run_validate(td.path());

    let rows = parse_report(&stderr);
    assert_well_formed(&rows);
    for (name, verdict, detail) in &rows {
        match name.as_str() {
            "network" | "limits" => {
                assert_eq!(
                    verdict, "skipped",
                    "{name} spawns a child and must not run without cause"
                );
                assert!(
                    detail.contains("no restricted profile in tree"),
                    "{name}: the skip must say why, was: {detail}"
                );
            }
            _ => assert_ne!(
                verdict, "skipped",
                "{name} is side-effect free and always answers"
            ),
        }
    }
    assert!(
        ok,
        "the appendix is informative — it never changes the validate verdict; stderr:\n{stderr}"
    );
}

#[test]
fn validate_runs_every_probe_when_the_tree_declares_a_restricted_profile() {
    let td = tempfile::TempDir::new().unwrap();
    tree_with_restricted_profile(td.path());
    let (ok, stderr) = run_validate(td.path());

    let rows = parse_report(&stderr);
    assert_well_formed(&rows);
    for (name, verdict, _) in &rows {
        assert_ne!(
            verdict, "skipped",
            "{name}: a tree that declares `restricted` is the cause the spawns needed"
        );
    }
    assert!(
        ok,
        "a host that cannot enforce the declared profile is reported, NOT turned into a \
         validate failure — the fail-closed refusal happens at spawn time; stderr:\n{stderr}"
    );
}
