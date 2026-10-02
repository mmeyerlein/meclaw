//! GH #945 -- the graph answers without waking the files.
//!
//! The lock of a new hive on a real colony (lesson B2 1): the SHIPPED
//! `file-space` and `graph-space`, wired by the member's own three edges
//! (`support/graph_space_colony.rs`, layout **member**), every cell of the
//! graph space running -- `index`, `query`, `store`. A small python package
//! and two markdown files are written through the space's tool door as the
//! core writes (`in_tool`, the writer's answers under `caller` 'tools'):
//!
//! - `/pkg/__init__.py`, `/pkg/b.py` (defines `f`), `/pkg/a.py` (imports `f`
//!   from `pkg.b` AS `ff` and calls `ff` in `g` -- the alias the file space
//!   hands over on the import edge, OR-BC-55 (2));
//! - `/docs/guide.md` (a section `Setup`, a link to `../pkg/a.py`) and
//!   `/docs/index.md` (links to `../pkg/b.py` and to `guide.md#Setup`, a
//!   fragment the graph slugs like the heading, OR-BC-55 (3)).
//!
//! The index arrives although no answer of the writes travels as `answer`:
//! the call edge `a#def:g -> b#def:f` and the three links end `resolved`.
//! Then, with the colony quiet, `callers`, `resolve`, `deps`, `dependents`
//! (also page by page under `limit` 1), `path` and `stats` answer what the
//! package says -- and no message caused by those questions reaches the file
//! space: every delivery to it is walked back along its parents, and none
//! starts at an `in_graph` question (a causal chain, not a time window). Last, a `file_read` tool call sent while another file is being
//! indexed comes back to the tool as `tool_result`, and nothing but the
//! graph's own `gs:` answers ever enters the graph space on `in_pulled`.
//! No dead letter; every delivery out of `index` and `query` carries a route
//! their contract declares.
//!
//! Free of a paid provider by construction: the space's summarizer talks to
//! a local chat stub, its embedder to a local embeddings stub. Guarded like
//! every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{Value, json};
use space::{Layout, create, graph_db, hop_str, quiet, rows, tool_call, wait_for};

const INIT: &str = "\"\"\"A small package.\"\"\"\n";
const B_PY: &str = "def f():\n    return 1\n";
const A_PY: &str = "from pkg.b import f as ff\n\n\ndef g():\n    return ff()\n";
const GUIDE: &str = "# Guide\n\n## Setup\n\nRun [the helper](../pkg/a.py).\n";
const INDEX: &str = "# Index\n\nSee [b](../pkg/b.py) and [setup](guide.md#Setup).\n";
const EXTRA: &str = "def e():\n    return 2\n";

fn count(db: &std::path::Path, sql: &str) -> String {
    rows(db, sql)
        .first()
        .and_then(|r| r.first())
        .cloned()
        .unwrap_or_default()
}

fn addrs(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i["addr"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_graph_answers_across_files_and_wakes_none() {
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

    let (_init, _) = create(&h, &mut ports, &root, "/pkg/__init__.py", INIT).await;
    let (b, _) = create(&h, &mut ports, &root, "/pkg/b.py", B_PY).await;
    let (a, _) = create(&h, &mut ports, &root, "/pkg/a.py", A_PY).await;
    let (guide, _) = create(&h, &mut ports, &root, "/docs/guide.md", GUIDE).await;
    let (index, _) = create(&h, &mut ports, &root, "/docs/index.md", INDEX).await;

    let resolved = |from: &str, to: &str| {
        format!(
            "SELECT count(*) FROM edges WHERE (from_addr = '{from}' OR from_source = '{from}') \
             AND to_addr = '{to}' AND state = 'resolved' AND class = 'extracted'"
        )
    };
    // A markdown link leaves from its section (`fh-…#sec:index`), so a file
    // address names the edge's source.
    let want = [
        (format!("{a}#def:g"), format!("{b}#def:f")),
        (index.clone(), b.clone()),
        (index.clone(), format!("{guide}#sec:setup")),
        (guide.clone(), a.clone()),
    ];
    for (from, to) in &want {
        let sql = resolved(from, to);
        wait_for(&root, &format!("{from} -> {to} resolved"), || {
            let n = count(&db, &sql);
            !n.is_empty() && n != "0"
        })
        .await;
    }
    quiet(&root).await;

    let callers = space::ask(
        &h,
        &mut ports,
        &root,
        "callers",
        "q1",
        json!({"addr": format!("{b}#def:f")}),
    )
    .await;
    let by_name = space::ask(
        &h,
        &mut ports,
        &root,
        "resolve",
        "q2",
        json!({"name": "pkg.b:f"}),
    )
    .await;
    let module = space::ask(
        &h,
        &mut ports,
        &root,
        "resolve",
        "q3",
        json!({"name": "pkg.b"}),
    )
    .await;
    let deps = space::ask(&h, &mut ports, &root, "deps", "q4", json!({"addr": a})).await;
    let dependents = space::ask(
        &h,
        &mut ports,
        &root,
        "dependents",
        "q5",
        json!({"addr": b, "depth": 3}),
    )
    .await;
    let mut paged = Vec::new();
    let mut cursor = String::new();
    for n in 0..10 {
        let page = space::ask(
            &h,
            &mut ports,
            &root,
            "dependents",
            &format!("q6-{n}"),
            json!({"addr": b, "depth": 3, "limit": 1, "cursor": cursor}),
        )
        .await;
        assert!(page["items"].as_array().map_or(0, Vec::len) <= 1, "{page}");
        paged.extend(page["items"].as_array().cloned().unwrap_or_default());
        cursor = page["next"].as_str().unwrap_or_default().to_string();
        if cursor.is_empty() {
            break;
        }
    }
    let path = space::ask(
        &h,
        &mut ports,
        &root,
        "path",
        "q7",
        json!({"from": guide, "to": b}),
    )
    .await;
    let stats = space::ask(&h, &mut ports, &root, "stats", "q8", json!({})).await;

    // A tool's answer while an index job runs goes to the tool.
    h.send(tool_call(
        "file_create",
        "make-extra",
        &json!({"path": "/pkg/extra.py", "text": EXTRA}),
    ))
    .await;
    h.send(tool_call(
        "file_read",
        "read-b",
        &json!({"file": "/pkg/b.py"}),
    ))
    .await;
    let mut seen = Vec::new();
    let read = space::next_matching(
        &mut ports.tsink,
        &root,
        "the result of `file_read`",
        |m| hop_str(m, "route") == "tool_result" && hop_str(m, "tool_call_id") == "read-b",
        &mut seen,
    )
    .await;
    let extra_rows = "SELECT count(*) FROM sources WHERE path = '/pkg/extra.py' \
                      AND version != '' AND version = announced"
        .to_string();
    wait_for(&root, "the extra file is indexed", || {
        count(&db, &extra_rows) == "1"
    })
    .await;
    quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    // The answers.
    assert_eq!(addrs(&callers), vec![format!("{a}#def:g")], "{callers}");
    assert_eq!(callers["broken"], json!(0), "{callers}");
    assert!(addrs(&by_name).contains(&format!("{b}#def:f")), "{by_name}");
    assert!(addrs(&module).contains(&b), "{module}");
    assert!(addrs(&deps).contains(&format!("{b}#def:f")), "{deps}");
    let mut deps_of_b: Vec<(String, i64)> = dependents["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| {
            (
                i["addr"].as_str().unwrap_or_default().to_string(),
                i["depth"].as_i64().unwrap_or(-1),
            )
        })
        .collect();
    deps_of_b.sort();
    let mut want_deps = vec![(a.clone(), 1), (index.clone(), 1), (guide.clone(), 2)];
    want_deps.sort();
    assert_eq!(deps_of_b, want_deps, "{dependents}");
    assert_eq!(
        json!(paged),
        dependents["items"],
        "`limit` 1 and `next` walk the same list"
    );
    assert_eq!(path["items"], json!([guide, a, b]), "{path}");
    assert_eq!(path["hops"], json!(2), "{path}");
    assert_eq!(stats["broken"], json!(0), "{stats}");

    // No delivery to the file space descends from a question.
    let question = |r: &space::Logged| r.to.starts_with("/graph-space") && r.route() == "in_graph";
    let woke: Vec<String> = log
        .iter()
        .filter(|r| r.to == "/file-space" || r.to.starts_with("/file-space/"))
        .filter_map(|r| space::chain_to_seam(&log, r, question))
        .map(|chain| {
            chain
                .iter()
                .map(|r| r.say())
                .collect::<Vec<_>>()
                .join(" => ")
        })
        .collect();
    assert!(woke.is_empty(), "a question woke the file space: {woke:#?}");
    let asked: Vec<String> = log
        .iter()
        .filter(|r| r.to.starts_with("/graph-space/query") && r.route() == "in_graph")
        .map(space::Logged::say)
        .collect();
    assert!(!asked.is_empty(), "the questions reached `./query`");

    // The tool's answer went to the tool; the graph only ever took its own.
    let answer = space::tool_answer(&read);
    assert_eq!(answer["ok"], json!(true), "{answer}");
    let foreign: Vec<String> = log
        .iter()
        .filter(|r| r.to.starts_with("/graph-space"))
        .filter(|r| {
            let route = r.route();
            route == "tool_result"
                || (route == "in_pulled"
                    && !r.hop["op_id"]
                        .as_str()
                        .unwrap_or_default()
                        .starts_with("gs:"))
        })
        .map(space::Logged::say)
        .collect();
    assert!(
        foreign.is_empty(),
        "a foreign answer entered the graph space: {foreign:#?}"
    );

    // Every cell of the new hive ran, and said only what it declares.
    for cell in ["index", "query", "store"] {
        let from = format!("/graph-space/{cell}");
        assert!(log.iter().any(|r| r.from == from), "{from} never emitted");
    }
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
