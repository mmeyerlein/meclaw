//! GH #945 -- a rename breaks the edge, and the graph reports it.
//!
//! The member layout (`support/graph_space_colony.rs`): the shipped file
//! space and graph space on one colony, files written through the space's
//! tool door as the core writes them. `/pkg/b.py` defines `f`, `/pkg/a.py`
//! calls it from `g`, `/docs/index.md` links `../pkg/b.py`. Once the call
//! edge is `resolved`:
//!
//! 1. `def f` becomes `def h` (`file_replace` against the version read): the
//!    call edge from `a#def:g` is `broken` with a `since`, it keeps the
//!    address it pointed at, `broken` lists it, and `callers` of the old node
//!    has no live caller and counts one broken edge;
//! 2. `/pkg/b.py` is removed (`file_remove`): its nodes are gone, its row
//!    stays as a tomb, and every edge into it -- the link from the index
//!    included -- is `broken`.
//!
//! "Reported" means exactly this in 1.0.0: in the store with `since`, listed
//! by `broken`, counted by every answer that touches it. An event to a
//! consumer comes with the first consumer (graph-space README).
//!
//! Two more locks around it:
//!
//! - `lost_is_broken_never_found_is_unresolved` (no colony; phase 1 and 2 of
//!   the shipped `index` script over the store stand-in): an import of a
//!   member its module never had stays `unresolved`; an edge that pointed at
//!   a node which is gone is `broken`, and it stays `broken` -- same `since`
//!   -- when its own source is indexed again (OR-BC-55 (4)). Measured red
//!   before: the never-there import read `broken`.
//! - `a_removal_without_a_version_breaks_its_links` (a booted graph space,
//!   the test plays the source): the file space announces a removal as
//!   `{source, path, tomb: true}` with no version (OR-BC-55 (1)); the index
//!   takes it, the source's nodes go, an edge into it breaks. Measured red
//!   before: dropped as "without a version".
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_pure.rs"]
mod pure;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{Value, json};
use space::{Layout, create, graph_db, hop_str, quiet, rows, tool_answer, tool_call, wait_for};

const LOST: &str = r#"
def lost_and_never():
    def spec(S, path, nodes, links):
        lang = lang_of("", path)
        return ({"source": S, "version": "v1", "path": path, "lang": lang,
                 "module": module_of(path, lang)}, nodes, links)
    A, B, C = "fh-a00000000001", "fh-b00000000002", "fh-c00000000003"
    specs = {
        "A": spec(A, "/pkg/a.py", [{"anchor": "def:g"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "pkg.b:f"},
                   {"kind": "call", "from_anchor": "def:g", "target_name": "f"}]),
        "B": spec(B, "/pkg/b.py", [{"anchor": "def:f"}], []),
        "Bh": spec(B, "/pkg/b.py", [{"anchor": "def:h"}], []),
        "C": spec(C, "/pkg/c.py", [{"anchor": "def:k"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "pkg.b:never"}])}
    db = {"sources": [source_row(A, "/pkg/a.py", "python", "pkg.a", "v1"),
                      source_row(B, "/pkg/b.py", "python", "pkg.b", "v1"),
                      source_row(C, "/pkg/c.py", "python", "pkg.c", "v1")]}

    def edges():
        return sorted([e["from_addr"], e["kind"], e["state"], e["to_addr"], e["since"]]
                      for e in db["edges"] if e["class"] == "extracted")
    out = {}
    interleave(db, specs, ["B1", "B2", "A1", "A2", "C1", "C2"])
    out["indexed"] = edges()
    interleave(db, specs, ["Bh1", "Bh2"], "R")
    out["renamed"] = edges()
    interleave(db, specs, ["A1", "A2", "C1", "C2"], "U")
    out["indexed again"] = edges()
    return out
"#;

#[test]
fn lost_is_broken_never_found_is_unresolved() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let extra = format!("{}\n{LOST}", pure::SIM);
    let got = pure::pure_with("index", &extra, "lost_and_never()", json!(null));
    let (a, b, c) = ("fh-a00000000001", "fh-b00000000002", "fh-c00000000003");
    let f = format!("{b}#def:f");
    let g = format!("{a}#def:g");
    let never = json!([c, "import", "unresolved", "", ""]);
    let want = |state: &str, since: &str| -> Value {
        let mut v = vec![
            json!([a, "import", state, f, since]),
            json!([g, "call", state, f, since]),
            never.clone(),
        ];
        v.sort_by_key(|x| x.to_string());
        Value::Array(v)
    };
    let sorted = |v: &Value| -> Value {
        let mut x = v.as_array().cloned().unwrap_or_default();
        x.sort_by_key(|y| y.to_string());
        Value::Array(x)
    };
    assert_eq!(sorted(&got["indexed"]), want("resolved", ""), "{got}");
    assert_eq!(
        sorted(&got["renamed"]),
        want("broken", "R2"),
        "a lost target is broken, a never-there one unresolved: {got}"
    );
    assert_eq!(
        sorted(&got["indexed again"]),
        want("broken", "R2"),
        "indexing the edge's own source again keeps it broken, since unchanged: {got}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removal_without_a_version_breaks_its_links() {
    if !space::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const X: &str = "fh-a00000000001";
    const Y: &str = "fh-b00000000002";
    const V1: &str = "111111111111";
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    space::build(&td, Layout::Alone, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = space::boot(&td).await;
    let db = graph_db(&root);
    let mut held = Vec::new();

    for (s, path, nodes, links) in [
        (
            X,
            "/pkg/x.py",
            json!([{"anchor": "def:f", "kind": "def"}]),
            json!([]),
        ),
        (
            Y,
            "/pkg/y.py",
            json!([{"anchor": "def:g", "kind": "def"}]),
            json!([{"kind": "import", "from_anchor": "", "target_name": "pkg.x:f"}]),
        ),
    ] {
        h.send(space::announce(s, V1, path, 1, 1, false)).await;
        for part in ["outline", "links", "near"] {
            let id = format!("gs:{part}:{s}:{V1}");
            let m = space::next_matching(
                &mut ports.pulls,
                &root,
                &format!("the `{part}` pull of {s}"),
                |m| hop_str(m, "op_id") == id,
                &mut held,
            )
            .await;
            let body = match part {
                "outline" => json!({"nodes": nodes}),
                "links" => json!({"links": links}),
                _ => json!({"near": []}),
            };
            h.send(space::pulled(&m, body)).await;
        }
    }
    let edge = format!("SELECT state, to_addr FROM edges WHERE from_source = '{Y}'");
    wait_for(&root, "y's import resolves", || {
        rows(&db, &edge) == vec![vec!["resolved".to_string(), format!("{X}#def:f")]]
    })
    .await;

    h.send(space::removal(X, "/pkg/x.py")).await;
    wait_for(&root, "the removal breaks y's import", || {
        rows(&db, &edge) == vec![vec!["broken".to_string(), format!("{X}#def:f")]]
    })
    .await;
    let x_nodes = rows(
        &db,
        &format!("SELECT anchor FROM nodes WHERE source = '{X}'"),
    );
    let x_row = rows(
        &db,
        &format!("SELECT tomb FROM sources WHERE source = '{X}'"),
    );
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    assert!(
        x_nodes.is_empty(),
        "the removed source's nodes are gone: {x_nodes:?}"
    );
    assert_eq!(
        x_row,
        vec![vec!["1".to_string()]],
        "its row stays as a tomb"
    );
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

const INIT: &str = "\"\"\"A small package.\"\"\"\n";
const B_PY: &str = "def f():\n    return 1\n";
const A_PY: &str = "from pkg.b import f\n\n\ndef g():\n    return f()\n";
const INDEX: &str = "# Index\n\nSee [b](../pkg/b.py).\n";

fn one(db: &std::path::Path, sql: &str) -> Vec<String> {
    rows(db, sql).into_iter().next().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renamed_function_breaks_its_callers_and_a_removed_file_its_links() {
    if !space::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    space::build(&td, Layout::Member, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = space::boot(&td).await;
    let db = graph_db(&root);

    create(&h, &mut ports, &root, "/pkg/__init__.py", INIT).await;
    let (b, b_v1) = create(&h, &mut ports, &root, "/pkg/b.py", B_PY).await;
    let (a, _) = create(&h, &mut ports, &root, "/pkg/a.py", A_PY).await;
    let (index, _) = create(&h, &mut ports, &root, "/docs/index.md", INDEX).await;
    let call = format!(
        "SELECT state, to_addr, since FROM edges WHERE from_addr = '{a}#def:g' AND kind = 'call'"
    );
    let link =
        format!("SELECT state, to_addr FROM edges WHERE from_source = '{index}' AND kind = 'link'");
    wait_for(&root, "the call edge resolves", || {
        one(&db, &call).first().map(String::as_str) == Some("resolved")
    })
    .await;
    wait_for(&root, "the link resolves", || {
        one(&db, &link).first().map(String::as_str) == Some("resolved")
    })
    .await;
    quiet(&root).await;

    // 1. The rename.
    h.send(tool_call(
        "file_replace",
        "rename",
        &json!({"file": b, "base": b_v1, "old": "def f():", "new": "def h():"}),
    ))
    .await;
    let mut seen = Vec::new();
    let renamed = space::next_matching(
        &mut ports.tsink,
        &root,
        "the rename's result",
        |m| hop_str(m, "route") == "tool_result" && hop_str(m, "tool_call_id") == "rename",
        &mut seen,
    )
    .await;
    let r = tool_answer(&renamed);
    assert_eq!(r["ok"], json!(true), "{r}");
    let b_v2 = r["version"].as_str().unwrap_or_default().to_string();
    wait_for(&root, "the call edge breaks", || {
        one(&db, &call).first().map(String::as_str) == Some("broken")
    })
    .await;
    quiet(&root).await;
    let after_rename = one(&db, &call);
    let broken = space::ask(&h, &mut ports, &root, "broken", "q1", json!({})).await;
    let callers = space::ask(
        &h,
        &mut ports,
        &root,
        "callers",
        "q2",
        json!({"addr": format!("{b}#def:f")}),
    )
    .await;
    let new_name = space::ask(
        &h,
        &mut ports,
        &root,
        "resolve",
        "q3",
        json!({"name": "pkg.b:h"}),
    )
    .await;

    // 2. The removal.
    h.send(tool_call(
        "file_remove",
        "remove",
        &json!({"file": b, "base": b_v2}),
    ))
    .await;
    let removed = space::next_matching(
        &mut ports.tsink,
        &root,
        "the removal's result",
        |m| hop_str(m, "route") == "tool_result" && hop_str(m, "tool_call_id") == "remove",
        &mut seen,
    )
    .await;
    let rm = tool_answer(&removed);
    assert_eq!(rm["ok"], json!(true), "{rm}");
    wait_for(&root, "the link breaks", || {
        one(&db, &link).first().map(String::as_str) == Some("broken")
    })
    .await;
    quiet(&root).await;
    let b_nodes = rows(
        &db,
        &format!("SELECT anchor FROM nodes WHERE source = '{b}'"),
    );
    let b_row = one(
        &db,
        &format!("SELECT tomb FROM sources WHERE source = '{b}'"),
    );
    let into_b = rows(
        &db,
        &format!("SELECT from_addr, state FROM edges WHERE to_source = '{b}' ORDER BY from_addr"),
    );
    let after_remove = space::ask(&h, &mut ports, &root, "broken", "q4", json!({})).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        after_rename[1],
        format!("{b}#def:f"),
        "a broken edge keeps its address"
    );
    assert!(
        !after_rename[2].is_empty(),
        "a broken edge says since when: {after_rename:?}"
    );
    let listed: Vec<String> = broken["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i["from"].as_str().map(str::to_string))
        .collect();
    assert!(
        listed.contains(&format!("{a}#def:g")),
        "`broken` lists the call: {broken}"
    );
    assert!(
        broken["items"].as_array().is_some_and(|v| v
            .iter()
            .all(|i| i["since"].as_str().is_some_and(|s| !s.is_empty()))),
        "every listed edge carries `since`: {broken}"
    );
    assert_eq!(
        callers["items"],
        json!([]),
        "no live caller of the old name: {callers}"
    );
    assert!(
        callers["broken"].as_u64().unwrap_or(0) >= 1,
        "`callers` counts it: {callers}"
    );
    assert!(
        new_name["items"]
            .as_array()
            .is_some_and(|v| v.iter().any(|i| i["addr"] == json!(format!("{b}#def:h")))),
        "the new name resolves: {new_name}"
    );
    assert!(
        b_nodes.is_empty(),
        "the removed file's nodes are gone: {b_nodes:?}"
    );
    assert_eq!(b_row, vec!["1".to_string()], "its row stays as a tomb");
    assert!(!into_b.is_empty(), "edges pointed into the removed file");
    assert!(
        into_b.iter().all(|r| r[1] == "broken"),
        "every edge into the removed file is broken: {into_b:?}"
    );
    let listed_after: Vec<String> = after_remove["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i["from"].as_str().map(str::to_string))
        .collect();
    assert!(
        listed_after.iter().any(|f| f.starts_with(index.as_str())),
        "`broken` lists the index's link: {after_remove}"
    );
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
