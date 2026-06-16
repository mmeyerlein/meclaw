use std::process::Command;

#[test]
fn cli_no_args_runs_default_path_and_exits_0() {
    // Direct-Mode (no flags): requires a root hive at `/`. With stdin closed
    // (output() provides no stdin), the bridge reads EOF immediately and the
    // process drains + exits 0. colony.db must be created in cwd.
    let tmp = tempfile::tempdir().unwrap();
    // Create a minimal root hive so Direct-Mode can proceed.
    // The meclaw root hive lives in a subdirectory of <root>
    // (e.g. `main/`); that subdirectory becomes meclaw path `/`.
    std::fs::create_dir_all(tmp.path().join("main")).unwrap();
    std::fs::write(
        tmp.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .current_dir(tmp.path())
        .output()
        .expect("failed to spawn meclaw binary");

    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    // colony.db wurde in cwd erzeugt (run-Pfad lief).
    assert!(
        tmp.path().join("colony.db").exists(),
        "colony.db must exist in cwd after default run"
    );
}

#[test]
fn cli_version_prints_to_stdout_and_exits_0() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--version")
        .current_dir(tmp.path())
        .output()
        .expect("failed to spawn meclaw binary");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("meclaw"), "stdout: {stdout}");
    assert!(stdout.contains("0.0.0"), "stdout: {stdout}");
    assert!(output.stderr.is_empty(), "stderr must be empty");

    let log_in_cwd = tmp.path().join("log.jsonl");
    assert!(!log_in_cwd.exists(), "log.jsonl must not exist in cwd");
}

#[test]
fn cli_help_prints_to_stdout_and_exits_0() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--help")
        .current_dir(tmp.path())
        .output()
        .expect("failed to spawn meclaw binary");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "stdout: {stdout}");
    assert!(stdout.contains("--root"), "stdout: {stdout}");
    assert!(stdout.contains("--log-level"), "stdout: {stdout}");
    assert!(output.stderr.is_empty(), "stderr must be empty");

    let log_in_cwd = tmp.path().join("log.jsonl");
    assert!(!log_in_cwd.exists(), "log.jsonl must not exist in cwd");
}
