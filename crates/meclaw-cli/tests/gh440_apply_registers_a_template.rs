//! GH #440: `--apply` inherits a new diff operation without a line of its own.
//!
//! `read_manifest_source` asks exactly one question — does the value carry a
//! `manifest` key — and leaves everything else to the colony ("No second
//! validation model", apply.rs). `ManifestBody::detect` asks the same one
//! question. Neither carries an op allowlist, so a seventh diff key should
//! travel to the door untouched. This file is the proof rather than the claim.

#[test]
fn a_manifest_file_registering_a_template_applies_from_the_command_line() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("templates")).expect("mkdir");
    // A colony needs a tree to boot into; the manifest changes the LIBRARY, not
    // this hive.
    std::fs::create_dir_all(root.path().join("main")).expect("mkdir");
    std::fs::write(
        root.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write");
    let manifest = root.path().join("grow.json");
    std::fs::write(
        &manifest,
        r#"{"manifest":[{"scope":"/","ctx":{},"diff":{"add_templates":[
             {"name":"note-unit","files":{
               "template.json":"{\"name\":\"note-unit\",\"version\":\"1.0.0\"}",
               "config.json":"{\"cell\":{\"type\":\"store\"},\"params\":{\"schema\":{}},\"contract\":{\"version\":\"0.1.0\",\"settings\":{},\"consumes\":{}}}"}}]}}]}"#,
    )
    .expect("write");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .args([
            "--root",
            root.path().to_str().expect("utf8"),
            "--templates",
            root.path().join("templates").to_str().expect("utf8"),
            "--apply",
            manifest.to_str().expect("utf8"),
        ])
        .output()
        .expect("run");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        root.path()
            .join("templates/local/note-unit/template.json")
            .is_file(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
