//! GH #586 — `--validate` checks the `override_params` a `ref` marker carries.
//!
//! A `cell.type: "ref"` marker may carry a top-level `override_params` block
//! beside its `cell` block, addressed by the cells of the template it names
//! (`""` is that template's root, `docs/config.md` § Spezialfall
//! Template-Referenz). The mutation door has asked both halves of that block
//! since GH #140 (a key that names no cell) and GH #294 (a key inside an entry
//! that names no param) — `--validate` asked neither, so a typo passed the
//! pre-flight check and cost a whole boot cycle to find.
//!
//! The refusal is a HARD error without `--validate-strict`, the same sharpness
//! GH #424 gave an unresolvable reference: it is not a legal topology somebody
//! might have meant, it is a statement about a param that does not exist. The
//! `error_code` is the door's own (`schema`), and the message is the door's own
//! wording — one cause must not have two formulations.

use std::path::Path;
use std::process::{Command, Stdio};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;
const CELL: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

/// A root with one `leaf` template and a marker referencing it, carrying
/// `overrides` verbatim as its top-level `override_params` block.
fn root_with_overrides(root: &Path, overrides: &str) {
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
        &format!(
            r#"{{"cell":{{"type":"ref","template":"leaf@1.0.0"}},"override_params":{overrides}}}"#
        ),
    );
}

/// A root whose marker carries NO `override_params` at all — the shape every
/// shipped `seed-ref` tree actually has.
fn root_with_no_overrides(root: &Path) {
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
        r#"{"cell":{"type":"ref","template":"leaf@1.0.0"}}"#,
    );
}

/// A root referencing the SUBTREE template `unit`: a hive root that declares
/// only its graph, plus one cell under it. The hive is an ordinary entry in the
/// template's cell list, so `""` is an addressable override target here.
fn root_with_subtree_overrides(root: &Path, overrides: &str) {
    write(
        root,
        "templates/unit/template.json",
        r#"{"name":"unit","version":"1.0.0"}"#,
    );
    write(root, "templates/unit/config.json", HIVE);
    write(root, "templates/unit/inner/config.json", CELL);
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/os/config.json",
        &format!(
            r#"{{"cell":{{"type":"ref","template":"unit@1.0.0"}},"override_params":{overrides}}}"#
        ),
    );
}

/// A root referencing `bare`: one cell with **no** `params` block at all.
fn root_with_bare_overrides(root: &Path, overrides: &str) {
    write(
        root,
        "templates/bare/template.json",
        r#"{"name":"bare","version":"1.0.0"}"#,
    );
    write(
        root,
        "templates/bare/config.json",
        r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/os/config.json",
        &format!(
            r#"{{"cell":{{"type":"ref","template":"bare@1.0.0"}},"override_params":{overrides}}}"#
        ),
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

/// The measured case (GH #586): an override key that names no param of the cell
/// it reached. It used to pass the pre-flight check and surface at boot.
#[test]
fn validate_refuses_an_override_naming_no_param() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_overrides(td.path(), r#"{"":{"emitted_targt":"/sink"}}"#);

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(
        !ok,
        "a param the referenced template does not declare is a hard error WITHOUT \
         --validate-strict: {stderr}"
    );
    assert!(
        stderr.contains("names no param"),
        "the door's own wording: {stderr}"
    );
    assert!(
        stderr.contains("emitted_targt"),
        "the refusal names the offending key: {stderr}"
    );
    assert!(
        stderr.contains("'emitted_target'"),
        "and lists the params that do exist: {stderr}"
    );
    assert!(
        stderr.contains("error_code: schema"),
        "the door's own error_code, not a new one: {stderr}"
    );
}

/// `--validate-strict` is the dial for legal topologies; this is not one, so the
/// verdict must not depend on it.
#[test]
fn validate_strict_refuses_it_too() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_overrides(td.path(), r#"{"":{"emitted_targt":"/sink"}}"#);

    let (ok, _stdout, stderr) = validate(td.path(), true);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("names no param"), "{stderr}");
}

/// The cell half (GH #140, one nesting level up): a key that addresses no cell
/// of the referenced template. `docs/config.md` has claimed since GH #277 that
/// this is an error and not a silent no-op — for the root-tree marker it was
/// neither.
#[test]
fn validate_refuses_an_override_naming_no_cell() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_overrides(td.path(), r#"{"nope":{"emitted_target":"/sink"}}"#);

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(
        !ok,
        "a key that addresses no cell is a hard error, not a silent no-op: {stderr}"
    );
    assert!(
        stderr.contains("names no cell"),
        "the door's own wording: {stderr}"
    );
    assert!(
        stderr.contains("\"\" (root)"),
        "the refusal lists the cells that exist: {stderr}"
    );
    assert!(
        stderr.contains("error_code: schema"),
        "the door's own error_code: {stderr}"
    );
}

/// Regression: a correct override stays clean, and `--validate` still grows
/// nothing.
#[test]
fn validate_accepts_a_correct_override() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_overrides(td.path(), r#"{"":{"emitted_target":"/elsewhere"}}"#);

    let (ok, stdout, stderr) = validate(td.path(), false);
    assert!(ok, "a correct override validates clean; stderr: {stderr}");
    assert!(
        stdout.contains("validate: growth: /os → leaf@1.0.0"),
        "the growth is still listed: {stdout:?}"
    );
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/os/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cfg["cell"]["type"], "ref",
        "--validate touches nothing — the marker is still a marker: {cfg}"
    );
}

/// GH #293's rule, one door further: a marker with two bad keys names BOTH in
/// one run. A pre-flight check that surrendered on the first typo would cost a
/// round trip per typo — the very cost this check exists to remove.
#[test]
fn validate_names_every_bad_key_in_one_run() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_overrides(
        td.path(),
        r#"{"nope":{"emitted_target":"/sink"},"":{"emitted_targt":"/sink"}}"#,
    );

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("names no cell"),
        "the unaddressable key is named: {stderr}"
    );
    assert!(
        stderr.contains("names no param"),
        "and so is the unknown param, in the SAME run: {stderr}"
    );
    assert_eq!(
        stderr.matches("error_code: schema").count(),
        2,
        "two keys, two refusals — not one and a second validate round: {stderr}"
    );
}

/// A hive scope marker is an ordinary entry in a subtree template's cell list,
/// so an override may address it — what is refused is a param the hive does not
/// declare, not the hive itself. Pinned because "a hive is not an actor" invites
/// the opposite assumption, and because a wrong answer here would make
/// `--validate` refuse a legal shipped composite.
#[test]
fn validate_addresses_a_hive_path_and_judges_its_params() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_subtree_overrides(td.path(), r#"{"":{"grph":{"edges":[]}}}"#);

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("names no param"),
        "the hive path IS addressable — the refusal is about the param: {stderr}"
    );
    assert!(
        !stderr.contains("names no cell"),
        "a hive path must never read as 'no such cell': {stderr}"
    );
    assert!(
        stderr.contains("'graph'"),
        "and it lists the params the hive does declare: {stderr}"
    );

    // The same address with the param it really has validates clean.
    let td2 = tempfile::TempDir::new().unwrap();
    root_with_subtree_overrides(td2.path(), r#"{"":{"graph":{"edges":[]}}}"#);
    let (ok2, _stdout2, stderr2) = validate(td2.path(), false);
    assert!(ok2, "a hive param that exists is fine: {stderr2}");
}

/// A cell with **no** `params` block has the EMPTY param set, and an override
/// addressed at it is refused naming that empty list — GH #294's explicit
/// policy, and materially right: `patch_and_substitute_config` merges only into
/// an existing `params` object, so the override would vanish silently.
#[test]
fn validate_refuses_an_override_on_a_cell_that_declares_no_params() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_bare_overrides(td.path(), r#"{"":{"emitted_target":"/sink"}}"#);

    let (ok, _stdout, stderr) = validate(td.path(), false);
    assert!(
        !ok,
        "an override into a cell with no params block is refused, not swallowed: {stderr}"
    );
    assert!(stderr.contains("names no param"), "{stderr}");
    assert!(
        stderr.contains("none"),
        "the refusal names the empty param list: {stderr}"
    );
}

/// The shape every shipped `seed-ref` tree has: a marker with no
/// `override_params` at all. It must stay silent — the check reads the template
/// of every growth, and this pins that reading one never invents a complaint.
#[test]
fn validate_says_nothing_about_a_marker_without_overrides() {
    let td = tempfile::TempDir::new().unwrap();
    root_with_no_overrides(td.path());

    let (ok, stdout, stderr) = validate(td.path(), false);
    assert!(ok, "a marker without overrides validates clean: {stderr}");
    assert!(
        !stderr.contains("override_params"),
        "and nothing is said about a block that is not there: {stderr}"
    );
    assert!(
        stdout.contains("validate: growth: /os → leaf@1.0.0"),
        "the growth is still listed: {stdout:?}"
    );
}
