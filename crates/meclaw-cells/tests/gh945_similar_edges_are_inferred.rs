//! GH #945 -- a neighbour by meaning is an `inferred` edge, never an
//! extracted one (R-BC-1, condition 2: a guessed edge never carries the class
//! "extracted").
//!
//! A booted graph space; the test plays the source. One source is announced
//! with no links, and its `near` answers three neighbours: two other sources
//! (one of them with a version pin, as a source may name it) and the source
//! itself. The store then holds exactly two edges from it -- kind `similar`,
//! class `inferred`, state `resolved`, each with its `score` -- and no
//! `extracted` edge; `similar` answers both, best first; and a structural
//! question does not follow them: `dependents` of a neighbour does not name
//! the source.
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::json;
use space::{Layout, announce, graph_db, pulled, quiet, rows, wait_for};

const A: &str = "fh-a00000000001";
const B: &str = "fh-b00000000002";
const C: &str = "fh-c00000000003";
const V1: &str = "111111111111";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn near_neighbours_become_inferred_edges_with_a_score() {
    if !space::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    space::build(&td, Layout::Alone, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = space::boot(&td).await;
    let db = graph_db(&root);

    h.send(announce(A, V1, "/notes/a.md", 1, 0, false)).await;
    let mut held = Vec::new();
    let mut got = Vec::new();
    for part in ["outline", "links", "near"] {
        let id = format!("gs:{part}:{A}:{V1}");
        let m = space::next_matching(
            &mut ports.pulls,
            &root,
            &format!("the `{part}` pull"),
            |m| space::hop_str(m, "op_id") == id,
            &mut held,
        )
        .await;
        got.push(m);
    }
    let near = &got[2];
    assert_eq!(
        space::body_of(near)["args"]["k"],
        json!(5),
        "`near` asks for `near_k` neighbours"
    );
    h.send(pulled(
        &got[0],
        json!({"nodes": [{"anchor": "sec:intro", "kind": "sec"}]}),
    ))
    .await;
    h.send(pulled(&got[1], json!({"links": []}))).await;
    h.send(pulled(
        near,
        json!({"near": [
            {"file": B, "score": 0.9},
            {"file": format!("{C}@{V1}"), "score": 0.7},
            {"file": A, "score": 1.0}
        ]}),
    ))
    .await;

    wait_for(&root, "the inferred edges are written", || {
        rows(
            &db,
            &format!("SELECT count(*) FROM edges WHERE from_source = '{A}'"),
        ) == vec![vec!["2".to_string()]]
    })
    .await;
    quiet(&root).await;

    let edges = rows(
        &db,
        &format!(
            "SELECT to_addr, kind, class, state, score FROM edges WHERE from_source = '{A}' \
             ORDER BY to_addr"
        ),
    );
    let similar = space::ask(
        &h,
        &mut ports,
        &root,
        "similar",
        "q-sim",
        json!({"addr": A}),
    )
    .await;
    let dependents = space::ask(
        &h,
        &mut ports,
        &root,
        "dependents",
        "q-dep",
        json!({"addr": B, "depth": 3}),
    )
    .await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        edges,
        vec![
            vec![
                B.to_string(),
                "similar".into(),
                "inferred".into(),
                "resolved".into(),
                "0.900000".into()
            ],
            vec![
                C.to_string(),
                "similar".into(),
                "inferred".into(),
                "resolved".into(),
                "0.700000".into()
            ],
        ],
        "two inferred edges with their score, the source itself left out, none extracted"
    );
    assert_eq!(similar["ok"], json!(true), "{similar}");
    assert_eq!(
        similar["items"],
        json!([{"addr": B, "score": 0.9}, {"addr": C, "score": 0.7}]),
        "best first: {similar}"
    );
    assert_eq!(
        dependents["items"],
        json!([]),
        "a structural question does not follow an inferred edge: {dependents}"
    );
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
