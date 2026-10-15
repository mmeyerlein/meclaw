//! GH #811 — the CLI boot keeps every template version the colony stands on.
//!
//! The colony-level suite (`meclaw-colony/tests/gh811_…`) boots through its own
//! harness, and that harness keeps through the RESCAN door. The two keep calls
//! in `meclaw_cli::run_with_hooks_tuned` — before the template scan (the
//! migration of a colony from before #811) and after the filesystem bootstrap
//! (a first boot, and nodes a boot grows from a `ref` marker, GH #424) — are
//! only reached by the real binary. So every proof here is a real process:
//! `meclaw --root R --apply <manifest>` boots the colony exactly like a daemon
//! would, applies a harmless manifest and exits (GH #423 one-shot).
//!
//! What is measured is the far end: the copy on disk under
//! `templates/local/<name>@<version>/` and the `templates` row in `colony.db`
//! pointing there under the same `template_id`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;
const STORE: &str = r#"{"cell":{"type":"store"},"params":{"schema":{"items":{"id":"int"}}},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A template in the shape a library ships it: `templates/<name>/`.
fn ship(root: &Path, name: &str, version: &str) {
    write(
        root,
        &format!("templates/{name}/template.json"),
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
    write(root, &format!("templates/{name}/config.json"), STORE);
}

fn kept(root: &Path, name: &str, version: &str) -> PathBuf {
    root.join(format!("templates/local/{name}@{version}"))
}

/// One CLI boot. The manifest registers a template of its own (`probe` names
/// it, unique per boot so a later boot is not refused as taken) — it touches no
/// node, so every copy this test finds was made by the boot.
fn boot(root: &Path, probe: &str) {
    let manifest = root.join(format!("{probe}.manifest.json"));
    std::fs::write(
        &manifest,
        format!(
            r#"{{"manifest":[{{"scope":"/","ctx":{{}},"diff":{{"add_templates":[
             {{"name":"{probe}","files":{{
               "template.json":"{{\"name\":\"{probe}\",\"version\":\"1.0.0\"}}",
               "config.json":"{{\"cell\":{{\"type\":\"store\"}},\"params\":{{\"schema\":{{}}}},\"contract\":{{\"version\":\"0.1.0\",\"settings\":{{}},\"consumes\":{{}}}}}}"}}}}]}}}}]}}"#
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(root)
        .arg("--apply")
        .arg(&manifest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --apply");
    assert!(
        out.status.success(),
        "the boot must succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `(template_id, version, filesystem_path)` of every row named `name`.
fn rows(root: &Path, name: &str) -> Vec<(String, Option<String>, PathBuf)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT template_id, version, filesystem_path FROM templates \
             WHERE name = ?1 ORDER BY version",
        )
        .unwrap();
    stmt.query_map([name], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            PathBuf::from(r.get::<_, String>(2)?),
        ))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn registry_stamp(root: &Path, path: &str) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open(root.join("colony.db")).unwrap();
    conn.query_row(
        "SELECT template, template_version FROM registry WHERE path = ?1",
        [path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

/// A first boot that GROWS a node from a `ref` marker (GH #424). The growth
/// writes its provenance through a colony message, not through the mutation
/// door, and the template table was empty when the boot started — only a keep
/// after the bootstrap sees it.
#[test]
fn a_first_boot_keeps_the_version_it_grew_from() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    ship(root, "leaf", "1.0.0");
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/os/config.json",
        r#"{"cell":{"type":"ref","template":"leaf@1.0.0"}}"#,
    );

    boot(root, "probe-a");

    assert_eq!(
        registry_stamp(root, "/os"),
        (Some("leaf".into()), Some("1.0.0".into())),
        "precondition: the boot grew /os from leaf@1.0.0"
    );
    let copy = kept(root, "leaf", "1.0.0");
    assert!(
        copy.join("template.json").is_file(),
        "the first boot kept leaf@1.0.0 under local/"
    );
    assert_eq!(
        std::fs::read(copy.join("config.json")).unwrap(),
        std::fs::read(root.join("templates/leaf/config.json")).unwrap(),
        "the copy carries the bytes of the shipped version"
    );
    let first = rows(root, "leaf");
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].2, copy, "the row points at the kept copy");

    // A second boot finds everything in place and changes nothing.
    boot(root, "probe-b");
    assert_eq!(rows(root, "leaf"), first, "the second boot is idempotent");
}

/// Migration: a colony from before GH #811 has its row on the SHIPPED
/// directory and no copy. Its next CLI boot keeps the version under the same
/// `template_id` and repoints the row.
#[test]
fn an_existing_colony_is_migrated_by_its_next_boot() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    ship(root, "note-unit", "1.0.0");
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/notes/config.json",
        r#"{"cell":{"type":"store","provenance":{"template":"note-unit","template_version":"1.0.0","template_chain":[["note-unit","1.0.0"]],"instantiated_at":1}},"params":{"schema":{"items":{"id":"int"}}},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    boot(root, "probe-a");

    // Roll the colony back to the pre-#811 state: no copy, the row on the
    // shipped directory.
    let shipped = root.join("templates/note-unit");
    let copy = kept(root, "note-unit", "1.0.0");
    if copy.exists() {
        std::fs::remove_dir_all(&copy).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(root.join("colony.db")).unwrap();
        conn.execute(
            "UPDATE templates SET filesystem_path = ?1 WHERE name = 'note-unit'",
            [shipped.to_string_lossy().into_owned()],
        )
        .unwrap();
    }
    let before = rows(root, "note-unit");
    assert_eq!(before.len(), 1, "{before:?}");
    assert_eq!(
        before[0].2, shipped,
        "precondition: the row sits on the shipped directory"
    );
    assert_eq!(
        registry_stamp(root, "/notes"),
        (Some("note-unit".into()), Some("1.0.0".into())),
        "precondition: the registry names the version"
    );

    boot(root, "probe-b");

    assert!(
        copy.join("template.json").is_file(),
        "the boot kept note-unit@1.0.0 under local/"
    );
    let after = rows(root, "note-unit");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0].0, before[0].0, "the same row, the same id");
    assert_eq!(after[0].2, copy, "the row now points at the kept copy");

    boot(root, "probe-c");
    assert_eq!(
        rows(root, "note-unit"),
        after,
        "the next boot is idempotent"
    );
}
