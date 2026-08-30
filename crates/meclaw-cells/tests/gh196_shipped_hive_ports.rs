//! GH #196 — a shipped template's `params.ports` opens the doors it names.
//!
//! The port boundary compares the SHORT name of a resolved endpoint against each
//! declared entry, so `./policy` matched nothing: `access` and `argus` were
//! sealed as strictly as `ports: []` while their own READMEs presented those
//! ports as the way in. Nothing noticed, because neither template was ever
//! instantiated — a declaration that can never match is invisible until somebody
//! follows the documentation and gets a `hive_port_boundary` reject that talks
//! about the boundary rather than about the spelling.
//!
//! Whether a template's declared interface is one the substrate can honour is a
//! fact about the FILE, checkable here with no colony and no runtime — the same
//! reasoning as `gh173_shipped_hive_contracts`, and the shape of test that would
//! have caught this before it shipped.
//!
//! The check runs each declaration through the REAL reader and the REAL boundary
//! rather than through a second opinion about what a port name looks like. A test
//! that re-implements the comparison is a test that can agree with itself while
//! disagreeing with the substrate — which is precisely the state this defect
//! lived in. So the template's own `config.json` is planted in a colony root and
//! read by `collect_sealed_hives`, and the question asked of every entry is the
//! only one that matters: does an edge from outside reach the child it names?

use meclaw_colony::config::{HiveParams, PortSpec};
use meclaw_colony::mutation::port_boundary::{
    SealedHive, collect_sealed_hives, validate_hive_port_boundary,
};
use meclaw_core::serde_json::{Value, json};

/// Where the synthetic hive lives while it is being checked. Any path does; what
/// matters is that endpoints resolve the way the colony resolves them.
const HIVE: &str = "/h";

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Plant one template's `config.json` as the hive `/h` of a throwaway colony
/// root, and let the substrate's own reader say what ports it sees. Only
/// `config.json` is needed: that file IS the declaration, and reading it per
/// mutation is how a live colony learns a hive's boundary.
fn seal_the_substrate_reads(config: &std::path::Path) -> SealedHive {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::create_dir_all(root.join("main/h")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::copy(config, root.join("main/h/config.json")).unwrap();
    let paths = [meclaw_core::Path::new(HIVE)];
    let mut sealed = collect_sealed_hives(root, paths.iter());
    assert_eq!(
        sealed.len(),
        1,
        "{}: the reader saw no seal",
        config.display()
    );
    sealed.remove(0)
}

/// Every hive `config.json` in the shipped tree that declared `params.ports`:
/// the file, and the short names of the children the template actually carries.
/// Sub-copies inside composite templates ride along on purpose — a copy that
/// drifted from its original has a different boundary.
fn shipped_sealed_hives() -> Vec<Shipped> {
    let root = templates_root();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(
        out.len() >= 10,
        "the sweep found almost no sealed hive templates: {}",
        out.len()
    );
    out
}

struct Shipped {
    /// Path of the template relative to `templates/`, for the failure message.
    name: String,
    /// The declaration itself, as the substrate would read it.
    config: std::path::PathBuf,
    /// Entries exactly as the file spells them — a plain name or, since GH
    /// #285, a slot object. Either way one entry is one port.
    declared: Vec<PortSpec>,
    /// Short names of the child directories instantiation would copy along.
    children: Vec<String>,
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<Shipped>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            walk(root, &p, out);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let val: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if val
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            != Some("hive")
        {
            continue;
        }
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        if params.is_null() {
            continue;
        }
        let hp: HiveParams = meclaw_core::serde_json::from_value(params)
            .unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
        let Some(declared) = hp.ports else { continue };
        let owndir = p.parent().unwrap();
        let mut children: Vec<String> = std::fs::read_dir(owndir)
            .unwrap()
            .filter_map(|e| {
                let e = e.unwrap();
                e.path()
                    .is_dir()
                    .then(|| e.file_name().to_string_lossy().into_owned())
            })
            .collect();
        children.sort();
        out.push(Shipped {
            name: p
                .strip_prefix(root)
                .unwrap()
                .parent()
                .unwrap()
                .display()
                .to_string(),
            config: p.clone(),
            declared,
            children,
        });
    }
}

#[test]
fn every_declared_port_opens_a_door_of_the_template_that_declares_it() {
    let mut checked = 0usize;
    for t in shipped_sealed_hives() {
        // `ports: []` is not skipped: it is the finished form (GH #197), and
        // the same question has a sharper answer there — the boundary must open
        // ZERO doors, so a hive that quietly regained an interior address is
        // caught by the same assertion rather than by a second one.
        let sealed = vec![seal_the_substrate_reads(&t.config)];
        // Which children does the boundary actually let an outside edge reach?
        // An edge onto a port is legal; onto anything else inside the hive it is
        // a `hive_port_boundary` reject. Counting the doors is the only question
        // that distinguishes a declaration from a decoration.
        let opened: Vec<&String> = t
            .children
            .iter()
            .filter(|c| {
                let e = json!({"add_edges": [{"from": "./caller", "to": format!("{HIVE}/{c}")}]});
                validate_hive_port_boundary(&e, "/", &sealed).is_ok()
            })
            .collect();
        assert_eq!(
            opened.len(),
            t.declared.len(),
            "{}: declares the ports {:?}, but the boundary opens {:?} out of the children {:?} — \
             an entry that opens no door seals this hive instead of opening it",
            t.name,
            t.declared,
            opened,
            t.children
        );
        checked += 1;
    }
    // The floor counts TEMPLATES, not ports. It used to count ports, which made
    // it fall as the library was migrated — and the end state of that migration
    // is that no template declares a port at all, at which point a port count
    // reaches zero and the sweep passes by looking at nothing.
    assert!(checked >= 5, "the sweep checked almost nothing: {checked}");
}
