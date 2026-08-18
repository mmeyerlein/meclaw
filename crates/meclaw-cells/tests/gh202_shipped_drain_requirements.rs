//! GH #202 — a shipped template's `params.required_drains` still insists.
//!
//! `required_drains[].port` is documented as the same shape as `params.ports`,
//! and since #196 that shape accepts both spellings of one node. The drain
//! reader had re-derived a stricter rule of its own and refused every `/`, so a
//! port written `./recall` was dropped with a warning and the hive that
//! declared the pairing quietly had no pairing left — lenient in the direction
//! that removes a guarantee, and invisible until somebody wires an ingress that
//! should have been refused.
//!
//! A declaration that no longer applies looks exactly like a declaration that
//! applies, so the file cannot answer this question about itself. The substrate
//! can: plant the template's `config.json` in a throwaway colony root, let the
//! REAL reader collect the requirements, and then ask the REAL check what
//! happens to a mutation that wires the port without its drain. A refusal means
//! the declaration bites. Anything else means it is decoration.
//!
//! Same reasoning and same shape as `gh196_shipped_hive_ports` — a test that
//! re-implements the rule can agree with itself while disagreeing with the
//! substrate, which is the state both defects lived in.

use meclaw_colony::config::HiveParams;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::required_drains::{DrainRequirement, check_required_drains};
use meclaw_core::serde_json::Value;

/// Where the synthetic hive lives while it is being checked, and a caller and a
/// sink outside it. Any paths do; what matters is that "outside" really is.
const HIVE: &str = "/h";
const CALLER: &str = "/caller";
const SINK: &str = "/sink";

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Plant one template's `config.json` as the hive `/h` of a throwaway colony
/// root and let the substrate's own reader say which requirements it sees. Only
/// `config.json` is needed: that file IS the declaration, and reading it per
/// mutation is how a live colony learns what a hive insists on.
fn requirements_the_substrate_reads(config: &std::path::Path) -> Vec<DrainRequirement> {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::create_dir_all(root.join("main/h")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::copy(config, root.join("main/h/config.json")).unwrap();
    let paths = [meclaw_core::Path::new(HIVE)];
    meclaw_colony::mutation::required_drains::collect_required_drains(root, paths.iter())
}

fn edge(from: &str, to: &str, condition: Option<&str>) -> Edge {
    Edge {
        id: meclaw_core::Uuid::now_v7(),
        from: meclaw_core::Path::new(from),
        to: meclaw_core::Path::new(to),
        condition: condition
            .map(|c| meclaw_colony::cel_eval::parse_condition(c).expect("test condition parses")),
        modifier: None,
    }
}

fn table(edges: Vec<Edge>) -> EdgeTable {
    let mut t = EdgeTable::new();
    for e in edges {
        t.insert(e);
    }
    t
}

/// A condition that carries exactly the declared hop, written the way a parent
/// would write it. The drain half of the pairing, so the passing case proves
/// the requirement is about this port and this route and not about nothing.
fn drain_condition(req: &DrainRequirement) -> String {
    req.hop
        .iter()
        .map(|(k, v)| format!("has(hop.{k}) && hop.{k} == '{v}'"))
        .collect::<Vec<_>>()
        .join(" && ")
}

struct Shipped {
    /// Path of the template relative to `templates/`, for the failure message.
    name: String,
    /// The declaration itself, as the substrate would read it.
    config: std::path::PathBuf,
    /// Port names exactly as the file spells them.
    declared: Vec<String>,
    /// Short names of the child directories instantiation would copy along.
    children: Vec<String>,
}

/// Every hive `config.json` in the shipped tree that declared `required_drains`.
/// Sub-copies inside composite templates ride along on purpose — a copy that
/// drifted from its original insists on something different.
fn shipped_hives_with_drains() -> Vec<Shipped> {
    let root = templates_root();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
        let declared: Vec<String> = hp
            .required_drains
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.port)
            .collect();
        if declared.is_empty() {
            continue;
        }
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
fn every_declared_drain_requirement_refuses_the_mutation_it_was_written_to_refuse() {
    let mut checked = 0usize;
    for t in shipped_hives_with_drains() {
        let reqs = requirements_the_substrate_reads(&t.config);
        assert_eq!(
            reqs.len(),
            t.declared.len(),
            "{}: declares the drain ports {:?}, but the reader kept {:?} — a dropped entry is a \
             hive that looks like it insists and does not",
            t.name,
            t.declared,
            reqs.iter().map(|r| &r.port_path).collect::<Vec<_>>()
        );
        for req in &reqs {
            // The port path has to be one a resolved endpoint can equal, or the
            // requirement can never fire however the colony is wired. Asking
            // which child it names is the same question as `./recall` failing
            // to be `recall`.
            let child = req
                .port_path
                .strip_prefix(&format!("{HIVE}/"))
                .unwrap_or_default();
            assert!(
                t.children.iter().any(|c| c == child),
                "{}: the requirement points at '{}', which is none of the children {:?} this \
                 template ships — no endpoint can ever resolve to it",
                t.name,
                req.port_path,
                t.children
            );

            // Wired from outside, nothing draining the declared route: this is
            // exactly the mutation the declaration exists to stop.
            let undrained = table(vec![edge(CALLER, &req.port_path, None)]);
            let err = check_required_drains(std::slice::from_ref(req), &undrained).unwrap_err();
            assert_eq!(
                err.error_code(),
                "required_drain_missing",
                "{}: wiring '{}' without its drain must be refused",
                t.name,
                req.port_path
            );

            // And the same wiring WITH the drain commits — otherwise the check
            // is refusing something other than what the hive declared.
            let drained = table(vec![
                edge(CALLER, &req.port_path, None),
                edge(&req.port_path, SINK, Some(&drain_condition(req))),
            ]);
            assert!(
                check_required_drains(std::slice::from_ref(req), &drained).is_ok(),
                "{}: '{}' with a drain for hop {:?} must commit",
                t.name,
                req.port_path,
                req.hop
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 2,
        "the sweep checked almost nothing: {checked} drain requirements"
    );
}
