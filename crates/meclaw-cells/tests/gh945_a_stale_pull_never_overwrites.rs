//! GH #945 -- a stale pull never overwrites, and two sources that name each
//! other end up resolved in any order.
//!
//! 1. **The order of phases** (`two_sources_resolve_each_other_in_every_order`,
//!    no colony): phase 1 and phase 2 of the shipped `index` script, run
//!    against a table-level stand-in of the store in every interleaving of two
//!    python sources that import and call each other (A1 B1 A2 B2, A1 B1 B2 A2,
//!    ...). Every interleaving ends with all four edges `resolved`. Measured
//!    red before the fix: an ANNOUNCED source counted as a module without
//!    nodes, so the earlier phase 2 broke the edge the later one would have
//!    resolved (A1 B1 A2 B2 left A's two edges `broken`).
//! 2. **A stale version** (`the_older_answer_arrives_last_and_is_dropped`, a
//!    booted graph space, the test plays the source): v1 and v2 of one source
//!    are announced back to back, the v2 answers are delivered first and the v1
//!    answers last. The store holds v2 -- its node, its version -- and keeps
//!    no row of v1. The lock reads the store only once the index has HANDLED
//!    v1: the bundle that drops the stale set (phase `done`, v1 in its call)
//!    and the store's answer to it are in the message log -- a signal, not a
//!    quiet window, so a slow machine cannot pass it before v1 arrived.
//! 3. **Two sources at once** (`two_sources_answered_together_resolve`): the
//!    six answers of two sources that name each other are delivered in one
//!    burst, in two different orders, and every extracted edge ends
//!    `resolved`.
//!
//! Free of a paid provider by construction: the graph space has no model and
//! no network. Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_pure.rs"]
mod pure;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::Message;
use meclaw_core::serde_json::{Value, json};
use space::{Layout, Logged, Ports, announce, graph_db, pulled, quiet, rows, wait_for};
use std::collections::BTreeMap;

const SPECS: &str = r#"
def two_sources():
    def spec(S, path, nodes, links):
        lang = lang_of("", path)
        return ({"source": S, "version": "v1", "path": path, "lang": lang,
                 "module": module_of(path, lang)}, nodes, links)
    A, B = "fh-a00000000001", "fh-b00000000002"
    return A, B, {
        "A": spec(A, "/pkg/a.py", [{"anchor": "def:g"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "pkg.b:f"},
                   {"kind": "call", "from_anchor": "def:g", "target_name": "f"}]),
        "B": spec(B, "/pkg/b.py", [{"anchor": "def:f"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "pkg.a:g"},
                   {"kind": "call", "from_anchor": "def:f", "target_name": "g"}])}


def every_order():
    A, B, specs = two_sources()
    out = {}
    for order in ARGS:
        # Both are announced before either is written: the announce bundle
        # inserts the row with an empty `version`.
        db = {"sources": [source_row(A, "/pkg/a.py", "python", "pkg.a", "v1"),
                          source_row(B, "/pkg/b.py", "python", "pkg.b", "v1")]}
        interleave(db, specs, order)
        out[" ".join(order)] = sorted([e["from_addr"], e["kind"], e["state"], e["to_addr"]]
                                      for e in db["edges"])
    return out
"#;

#[test]
fn two_sources_resolve_each_other_in_every_order() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let orders = json!([
        ["A1", "A2", "B1", "B2"],
        ["A1", "B1", "A2", "B2"],
        ["A1", "B1", "B2", "A2"],
        ["B1", "A1", "A2", "B2"],
        ["B1", "A1", "B2", "A2"],
        ["B1", "B2", "A1", "A2"]
    ]);
    let extra = format!("{}\n{SPECS}", pure::SIM);
    let got = pure::pure_with("index", &extra, "every_order()", orders);
    let a = "fh-a00000000001";
    let b = "fh-b00000000002";
    let want = json!([
        [a, "import", "resolved", format!("{b}#def:f")],
        [
            format!("{a}#def:g"),
            "call",
            "resolved",
            format!("{b}#def:f")
        ],
        [b, "import", "resolved", format!("{a}#def:g")],
        [
            format!("{b}#def:f"),
            "call",
            "resolved",
            format!("{a}#def:g")
        ]
    ]);
    let mut want_sorted: Vec<Value> = want.as_array().cloned().unwrap_or_default();
    want_sorted.sort_by_key(|v| v.to_string());
    for (order, edges) in got.as_object().expect("one result per order") {
        let mut e: Vec<Value> = edges.as_array().cloned().unwrap_or_default();
        e.sort_by_key(|v| v.to_string());
        assert_eq!(e, want_sorted, "order {order}: every edge resolved");
    }
}

// ═════════════════════════════════════════════════════════════════ the colony

const A: &str = "fh-a00000000001";
const V1: &str = "111111111111";
const V2: &str = "222222222222";

/// The three pulls of one announcement, by part.
async fn take_pulls(
    ports: &mut Ports,
    root: &std::path::Path,
    source: &str,
    version: &str,
    held: &mut Vec<Message>,
) -> BTreeMap<String, Message> {
    let mut out = BTreeMap::new();
    for part in ["outline", "links", "near"] {
        let id = format!("gs:{part}:{source}:{version}");
        if let Some(i) = held.iter().position(|m| space::hop_str(m, "op_id") == id) {
            out.insert(part.to_string(), held.remove(i));
            continue;
        }
        let m = space::next_matching(
            &mut ports.pulls,
            root,
            &format!("the `{part}` pull of {source}@{version}"),
            |m| space::hop_str(m, "op_id") == id,
            held,
        )
        .await;
        assert_eq!(space::hop_str(&m, "route"), "pull");
        assert_eq!(space::hop_str(&m, "caller"), "", "a pull carries no caller");
        let b = space::body_of(&m);
        assert_eq!(b["file"], json!(format!("{source}@{version}")), "{b}");
        out.insert(part.to_string(), m);
    }
    out
}

/// The index handled the stale set of `version`: its `done` bundle went to
/// the store, and the store answered that very bundle.
fn stale_set_dropped(log: &[Logged], version: &str) -> bool {
    log.iter()
        .filter(|r| {
            r.from == "/graph-space/index"
                && r.to == "/graph-space/store"
                && r.hop["phase"] == json!("done")
                && r.hop["cur_call"]
                    .as_str()
                    .is_some_and(|c| c.contains(version))
        })
        .any(|bundle| {
            log.iter().any(|r| {
                r.from == "/graph-space/store"
                    && r.to == "/graph-space/index"
                    && r.parent.as_deref() == Some(bundle.id.as_str())
            })
        })
}

fn answers(pulls: &BTreeMap<String, Message>, nodes: Value, links: Value) -> Vec<Message> {
    vec![
        pulled(&pulls["outline"], json!({"nodes": nodes})),
        pulled(&pulls["links"], json!({"links": links})),
        pulled(&pulls["near"], json!({"near": []})),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_older_answer_arrives_last_and_is_dropped() {
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
    let mut held = Vec::new();

    h.send(announce(A, V1, "/pkg/a.py", 1, 0, false)).await;
    let p1 = take_pulls(&mut ports, &root, A, V1, &mut held).await;
    h.send(announce(A, V2, "/pkg/a.py", 1, 0, false)).await;
    let p2 = take_pulls(&mut ports, &root, A, V2, &mut held).await;

    for m in answers(
        &p2,
        json!([{"anchor": "def:new", "kind": "def"}]),
        json!([]),
    ) {
        h.send(m).await;
    }
    wait_for(&root, "v2 is written", || {
        rows(
            &db,
            &format!("SELECT version FROM sources WHERE source = '{A}'"),
        ) == vec![vec![V2.to_string()]]
    })
    .await;
    for m in answers(
        &p1,
        json!([{"anchor": "def:old", "kind": "def"}]),
        json!([]),
    ) {
        h.send(m).await;
    }
    wait_for(&root, "the stale v1 set is handled", || {
        stale_set_dropped(&space::message_log(&root), V1)
    })
    .await;

    let nodes = rows(
        &db,
        &format!("SELECT anchor FROM nodes WHERE source = '{A}'"),
    );
    let source = rows(
        &db,
        &format!("SELECT version, announced FROM sources WHERE source = '{A}'"),
    );
    let left = rows(&db, "SELECT source, version, part FROM pulls");
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        nodes,
        vec![vec!["def:new".to_string()]],
        "the store holds v2's node only"
    );
    assert_eq!(
        source,
        vec![vec![V2.to_string(), V2.to_string()]],
        "v2 indexed and announced"
    );
    assert!(left.is_empty(), "no row of the stale v1 is kept: {left:?}");
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sources_answered_together_resolve() {
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
    let mut held = Vec::new();

    // Two pairs: the first answered alternately, the second one source after
    // the other in reverse.
    let pairs = [
        (
            "fh-c00000000003",
            "/one/c.py",
            "one.d",
            "fh-d00000000004",
            "/one/d.py",
            "one.c",
            false,
        ),
        (
            "fh-e00000000005",
            "/two/e.py",
            "two.f",
            "fh-f00000000006",
            "/two/f.py",
            "two.e",
            true,
        ),
    ];
    for (x, xp, xt, y, yp, yt, reverse) in pairs {
        h.send(announce(x, V1, xp, 1, 2, false)).await;
        h.send(announce(y, V1, yp, 1, 2, false)).await;
        let px = take_pulls(&mut ports, &root, x, V1, &mut held).await;
        let py = take_pulls(&mut ports, &root, y, V1, &mut held).await;
        let ax = answers(
            &px,
            json!([{"anchor": "def:xf", "kind": "def"}]),
            json!([{"kind": "import", "from_anchor": "", "target_name": format!("{xt}:yf")},
                   {"kind": "call", "from_anchor": "def:xf", "target_name": "yf"}]),
        );
        let ay = answers(
            &py,
            json!([{"anchor": "def:yf", "kind": "def"}]),
            json!([{"kind": "import", "from_anchor": "", "target_name": format!("{yt}:xf")},
                   {"kind": "call", "from_anchor": "def:yf", "target_name": "xf"}]),
        );
        let burst: Vec<Message> = if reverse {
            ay.into_iter().chain(ax).collect()
        } else {
            ax.into_iter().zip(ay).flat_map(|(a, b)| [a, b]).collect()
        };
        for m in burst {
            h.send(m).await;
        }
        let (x, y) = (x.to_string(), y.to_string());
        wait_for(&root, "both pairs resolved", || {
            rows(
                &db,
                &format!(
                    "SELECT count(*) FROM edges WHERE class = 'extracted' AND state = 'resolved' \
                     AND from_source IN ('{x}', '{y}')"
                ),
            ) == vec![vec!["4".to_string()]]
        })
        .await;
    }
    quiet(&root).await;
    let open = rows(
        &db,
        "SELECT from_addr, kind, state FROM edges WHERE state != 'resolved'",
    );
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    assert!(open.is_empty(), "every edge resolved: {open:?}");
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
