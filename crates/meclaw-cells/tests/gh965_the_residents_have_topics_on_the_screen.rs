//! GH #965 -- the member's residents as screen topics: the built-in topics of
//! the presenter (`params.builtin_topics` of `presenter/stage`) and the one new
//! read lane they need, the colony view's counts.
//!
//! - Every built-in topic passes the presenter's own manifest check against the
//!   catalogue copy it ships with (`stage.check_topic`), names a standard, and
//!   reads each of its sets from a declared SOURCE -- a resident the recipe
//!   knows (`reads_residents`) -- with no code per topic.
//! - The colony view answers `in_read {op: stats}` with its counts out of the
//!   last snapshot, kept between messages (`runner_mode: resident`), and a
//!   child that holds none answers `not_ready` and asks the colony in the same
//!   run. Counts and never content: no path, no edge condition leaves.
//!
//! The drivers are Python and load the shipped scripts; SKIPs where the
//! templates do not travel (R2b).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn python(driver: &str, args: &[std::path::PathBuf]) -> String {
    let out = Command::new("python3")
        .arg("-c")
        .arg(driver)
        .args(args)
        .output()
        .expect("python3 runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    stdout
}

/// The residents the builder recipe can read (`RESIDENTS` there).
const RESIDENTS: [&str; 7] = [
    "memory-hive",
    "file-space",
    "graph-space",
    "objects",
    "librarian",
    "affinity",
    "colony-view",
];

const TOPICS_DRIVER: &str = r#"
import importlib.util, json, sys
stage_py, config = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("stage", stage_py)
st = importlib.util.module_from_spec(spec)
sys.argv = [stage_py]
spec.loader.exec_module(st)
params = json.load(open(config, encoding="utf-8"))["params"]
cat = st.catalogue(params)
out = []
for t in params.get("builtin_topics") or []:
    # A built-in topic is checked as the presenter's own (`builtin_topics` in stage.py
    # does the same, `own=True`): only those may name a source. The same manifest
    # offered by an app is refused (Q.4, OR-DP-77) -- `as_app` holds that answer.
    why = st.check_topic(t, cat, own=True)
    as_app = st.check_topic(t, cat)
    sources = sorted({json.dumps(c.get("source"), sort_keys=True) for c in t["candidates"]})
    reads = sorted({(c.get("source") or {}).get("read") for c in t["candidates"]})
    out.append({"topic": t["topic"], "why": why, "standard": t.get("standard"),
                "keys": [c["key"] for c in t["candidates"]], "reads": reads,
                "sourceless": [c["key"] for c in t["candidates"] if not c.get("source")],
                "sources": len(sources), "as_app": as_app})
print(json.dumps(out))
"#;

/// Red before GH #965: `presenter/stage` carries no `builtin_topics`.
#[test]
fn every_builtin_topic_binds_and_reads_a_declared_resident() {
    let stage = repo("templates/presenter/stage/stage.py");
    let config = repo("templates/presenter/stage/config.json");
    if !stage.exists() {
        println!("SKIP every_builtin_topic_binds_and_reads_a_declared_resident: no presenter");
        return;
    }
    let got: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&python(TOPICS_DRIVER, &[stage, config])).expect("json");
    let topics = got.as_array().expect("a list");
    let names: Vec<&str> = topics
        .iter()
        .map(|t| t["topic"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "memory", "files", "graph", "library", "objects", "people", "colony"
        ],
        "the resident topics of the inventory (DISPATCH-D2 § 2)"
    );
    for t in topics {
        assert!(t["why"].is_null(), "the manifest check refuses {t}");
        assert!(
            t["as_app"]
                .as_str()
                .is_some_and(|w| w.contains("a source is for the presenter's own topics")),
            "the same manifest from an app is refused: {t}"
        );
        let keys = t["keys"].as_array().unwrap();
        assert!(
            (2..=4).contains(&keys.len()),
            "a standard and 1-3 variants: {t}"
        );
        assert!(keys.contains(&t["standard"]), "{t}");
        assert_eq!(t["sourceless"], meclaw_core::serde_json::json!([]), "{t}");
        for r in t["reads"].as_array().unwrap() {
            assert!(
                RESIDENTS.contains(&r.as_str().unwrap_or("")),
                "{r} is no resident the recipe can read: {t}"
            );
        }
    }
}

const PROBE_DRIVER: &str = r#"
import contextlib, io, json, sys
src = open(sys.argv[1], encoding="utf-8").read()
g = {}
def run(doc):
    sys.stdin = io.StringIO(json.dumps(doc))
    out = io.StringIO()
    g["__name__"] = "__main__"
    with contextlib.redirect_stdout(out):
        exec(compile(src, "probe.py", "exec"), g)
    v = json.loads(out.getvalue())
    return v if isinstance(v, list) else [v]
def read(op="stats", op_id="res:7"):
    return run({"envelope": {"header": {"hop": {"route": "in_read", "op": op, "op_id": op_id},
                                        "context": {"resident_caller": "viewer"}}},
                "body": {"messages": []}, "params": {}})
cold = read()
snap = run({"envelope": {"header": {}}, "body": {"graph": {"scope": "/", "nodes": [
    {"path": "/m", "cell_type": "hive"}, {"path": "/m/a", "cell_type": "code"},
    {"path": "/m/b", "cell_type": "code"}, {"path": "/m/c", "cell_type": "llm"}],
    "edges": [{"from": "/m/a", "to": "/m/b", "condition": "secret == 'x'"}]}}, "params": {}})
warm = read()
other = read(op="paths", op_id="res:8")
print(json.dumps({"cold": cold, "snap": snap, "warm": warm, "other": other}))
"#;

#[test]
fn the_colony_view_answers_its_counts_out_of_the_last_snapshot() {
    let probe = repo("templates/colony-view/probe/probe.py");
    if !probe.exists() {
        println!("SKIP the_colony_view_answers_its_counts: colony-view does not travel");
        return;
    }
    let v: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&python(PROBE_DRIVER, &[probe])).expect("json");
    // A child with no snapshot: not_ready, and the colony is asked in the same run.
    let cold = v["cold"].as_array().unwrap();
    assert_eq!(cold.len(), 2, "{v}");
    assert_eq!(cold[0]["header"]["route"], "answer");
    assert_eq!(cold[0]["header"]["op_id"], "res:7");
    assert_eq!(cold[0]["ok"], false);
    assert_eq!(cold[0]["error"]["code"], "not_ready");
    assert_eq!(cold[1]["header"]["route"], "ask_colony");
    // The snapshot still goes to the layout, unread.
    assert_eq!(v["snap"][0]["header"]["route"], "snapshot");
    // Then the counts, exactly one answer.
    let warm = v["warm"].as_array().unwrap();
    assert_eq!(warm.len(), 1, "{v}");
    let a = &warm[0];
    assert_eq!(a["header"]["route"], "answer");
    assert_eq!(a["ok"], true);
    assert_eq!(a["cells"], 4);
    assert_eq!(a["hives"], 1);
    assert_eq!(a["edges"], 1);
    assert_eq!(
        a["kinds"],
        meclaw_core::serde_json::json!([{"kind": "code", "n": 2}, {"kind": "hive", "n": 1},
                                        {"kind": "llm", "n": 1}])
    );
    let text = a.to_string();
    for content in ["/m/a", "secret"] {
        assert!(!text.contains(content), "counts and never content: {text}");
    }
    assert_eq!(v["other"][0]["error"]["code"], "unknown_op");
}

/// The probe keeps its counts between messages, so it runs resident (one
/// child); the hive answers `in_read` on `answer` and says so in its contract.
#[test]
fn the_colony_view_declares_its_read_lane() {
    let cfg = repo("templates/colony-view/config.json");
    if !cfg.exists() {
        println!("SKIP the_colony_view_declares_its_read_lane: colony-view does not travel");
        return;
    }
    let hive: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(cfg).unwrap()).unwrap();
    let probe: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/colony-view/probe/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(probe["params"]["runner_mode"], "resident");
    let p = &hive["params"];
    assert!(
        p["contract"]["accepts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["route"] == "in_read")
    );
    assert!(
        p["contract"]["emits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["route"] == "answer")
    );
    let edges = p["graph"]["edges"].as_array().unwrap();
    assert!(edges.iter().any(|e| e["from"] == "."
        && e["to"] == "./probe"
        && e["condition"] == "has(hop.route) && hop.route == 'in_read'"));
    assert!(edges.iter().any(|e| e["from"] == "./probe"
        && e["to"] == "."
        && e["condition"] == "has(hop.route) && hop.route == 'answer'"));
}
