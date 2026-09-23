//! GH #811 — "a colony keeps its OWN copy": the copy lies in a directory the
//! colony owns, never in a library it was merely pointed at.
//!
//! Measured in the integration pass of 2026-09-23: the scenario runner boots
//! throwaway colonies with `--templates templates`, the repository's own
//! library, and the keep of #811 wrote `templates/local/memory-hive@3.4.1/`
//! into the repository. The kept copy was built from the library root
//! (`{templates_root}/local/…`), and the library root was not the colony's.
//!
//! What this file pins, with a real process (`meclaw --root R --templates L
//! --apply <manifest>`, GH #423 one-shot) and a library `L` OUTSIDE `R`:
//! 1. a grow, an `add_templates` and a lift leave `L` byte-identical;
//! 2. the kept copies lie under `R/templates/local/<name>@<version>/` and the
//!    rows point there;
//! 3. after the library moved on, the way back to the old version still needs
//!    no `add_templates`;
//! 4. the next boot does not take the colony-owned rows for a foreign index
//!    (GH #61) — the rows and their ids stay as they are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn store(contract_version: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"store"}},"params":{{"schema":{{"items":{{"id":"int"}}}}}},"contract":{{"version":"{contract_version}","settings":{{}},"consumes":{{}}}}}}"#
    )
}

/// The library ships `leaf` the way an instance build lays it down: ONE
/// directory `L/leaf/` holding the current version.
fn ship_leaf(library: &Path, version: &str, contract_version: &str) {
    let dir = library.join("leaf");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    write(
        library,
        "leaf/template.json",
        &format!(r#"{{"name":"leaf","version":"{version}"}}"#),
    );
    write(library, "leaf/config.json", &store(contract_version));
}

/// Every file below `dir`, relative path → bytes. The library's fingerprint.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

/// One CLI boot over root `R` with the library `L`, applying one diff at `/`.
fn apply(root: &Path, library: &Path, label: &str, diff: &str, rescan: bool) {
    let manifest = root.join(format!("{label}.manifest.json"));
    std::fs::write(
        &manifest,
        format!(r#"{{"manifest":[{{"scope":"/","ctx":{{}},"diff":{diff}}}]}}"#),
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root")
        .arg(root)
        .arg("--templates")
        .arg(library)
        .arg("--apply")
        .arg(&manifest);
    if rescan {
        cmd.arg("--rescan-templates");
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --apply");
    assert!(
        out.status.success(),
        "{label}: the boot must succeed and the manifest commit\nstdout: {}\nstderr: {}",
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

fn own(root: &Path, name: &str, version: &str) -> PathBuf {
    root.join(format!("templates/local/{name}@{version}"))
}

const LIFT_TO_1_1: &str =
    r#"{"replace_nodes":[{"match":{"name":"notes"},"with":{"template":"leaf@1.1.0"}}]}"#;
const LIFT_BACK_TO_1_0: &str =
    r#"{"replace_nodes":[{"match":{"name":"notes"},"with":{"template":"leaf@1.0.0"}}]}"#;

#[test]
fn a_library_outside_the_root_stays_byte_identical() {
    let colony = tempfile::TempDir::new().unwrap();
    let shared = tempfile::TempDir::new().unwrap();
    let root = colony.path();
    let library = shared.path().join("templates");
    write(root, "main/config.json", HIVE);
    ship_leaf(&library, "1.0.0", "0.1.0");
    let shipped_one = snapshot(&library);

    // Grow from the foreign library, and register a template of its own.
    apply(
        root,
        &library,
        "grow",
        r#"{"add_templates":[{"name":"probe","files":{
             "template.json":"{\"name\":\"probe\",\"version\":\"1.0.0\"}",
             "config.json":"{\"cell\":{\"type\":\"store\"},\"params\":{\"schema\":{}},\"contract\":{\"version\":\"0.1.0\",\"settings\":{},\"consumes\":{}}}"}}],
            "add_nodes":[{"name":"notes","template":"leaf@1.0.0"}]}"#,
        false,
    );
    assert_eq!(
        registry_stamp(root, "/notes"),
        (Some("leaf".into()), Some("1.0.0".into())),
        "precondition: /notes stands on leaf@1.0.0"
    );
    assert_eq!(
        snapshot(&library),
        shipped_one,
        "the grow wrote into a library the colony does not own"
    );
    let copy_one = own(root, "leaf", "1.0.0");
    assert!(
        copy_one.join("template.json").is_file(),
        "the colony keeps leaf@1.0.0 under its own root"
    );
    let leaf_rows = rows(root, "leaf");
    assert_eq!(leaf_rows.len(), 1, "{leaf_rows:?}");
    assert_eq!(
        leaf_rows[0].2, copy_one,
        "the row points at the colony's copy"
    );
    let id_one = leaf_rows[0].0.clone();
    let probe_rows = rows(root, "probe");
    assert_eq!(probe_rows.len(), 1, "{probe_rows:?}");
    assert_eq!(
        probe_rows[0].2,
        own(root, "probe", "1.0.0"),
        "a registration lands in the colony's own directory too"
    );

    // The library moves on (the owner's act, not the colony's): 1.0.0 is gone.
    ship_leaf(&library, "1.1.0", "0.2.0");
    let shipped_two = snapshot(&library);
    apply(root, &library, "lift", LIFT_TO_1_1, true);
    assert_eq!(
        registry_stamp(root, "/notes"),
        (Some("leaf".into()), Some("1.1.0".into())),
        "the lift reached 1.1.0"
    );
    assert!(own(root, "leaf", "1.1.0").join("template.json").is_file());

    // The way back needs no add_templates.
    apply(root, &library, "back", LIFT_BACK_TO_1_0, false);
    assert_eq!(
        registry_stamp(root, "/notes"),
        (Some("leaf".into()), Some("1.0.0".into())),
        "back at 1.0.0 without an add_templates"
    );
    assert_eq!(
        snapshot(&library),
        shipped_two,
        "lift and way back wrote into a library the colony does not own"
    );

    let after = rows(root, "leaf");
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0].0, id_one, "1.0.0 kept its id");
    for (_, v, p) in &after {
        assert_eq!(p, &own(root, "leaf", v.as_deref().unwrap()));
    }
    assert!(
        !library.join("local").exists(),
        "no local/ in the foreign library"
    );
}
