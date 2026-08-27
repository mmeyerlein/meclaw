//! GH #424 — `--validate` says what the first boot will GROW, and grows
//! nothing.
//!
//! `--validate` is the nginx -t role: it promises to touch nothing. A dry run
//! that created cell directories would be a broken promise, so this file pins
//! both halves — the listing, and the untouched tree.
//!
//! What it CAN check is resolvability, and an unresolvable reference is a HARD
//! error even without `--validate-strict`. That is not the strictness dial: a
//! reference nothing provides is a tree guaranteed not to boot, the same
//! sharpness `DanglingEndpoint` already carries. The dial belongs to findings
//! that are legal topologies.

use std::path::Path;
use std::process::{Command, Stdio};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;
const CELL: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

/// A root with one `leaf` template and a marker that references `reference`.
fn root_with_marker(root: &Path, reference: &str) {
    write(
        root,
        "templates/leaf/template.json",
        r#"{"name":"leaf","version":"1.0.0"}"#,
    );
    write(root, "templates/leaf/config.json", CELL);
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/os/config.json",
        &format!(r#"{{"cell":{{"type":"ref","template":"{reference}"}}}}"#),
    );
}

fn validate(root: &Path, strict: bool) -> (bool, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root").arg(root).arg("--validate");
    if strict {
        cmd.arg("--validate-strict");
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --validate");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn validate_lists_a_planned_growth() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_marker(td.path(), "leaf@1.0.0");

    let (ok, stdout, stderr) = validate(td.path(), false);
    assert!(ok, "a resolvable growth validates clean; stderr: {stderr}");
    assert!(
        stdout.contains("validate: growth: /os → leaf@1.0.0"),
        "one line per marker, naming the position and the reference: {stdout:?}"
    );
}

#[test]
fn validate_grows_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_marker(td.path(), "leaf@1.0.0");

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(ok, "stderr: {stderr}");
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/os/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cfg["cell"]["type"], "ref",
        "--validate touches nothing — the marker is still a marker: {cfg}"
    );
}

#[test]
fn validate_fails_on_a_growth_whose_template_is_missing() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_marker(td.path(), "leaf@9.9.9");

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(
        !ok,
        "an unresolvable reference is an error WITHOUT --validate-strict: {stderr}"
    );
    assert!(stderr.contains("leaf@9.9.9"), "{stderr}");
    assert!(
        stderr.contains("cannot fulfil it"),
        "the refusal says what the consequence is: {stderr}"
    );
}
