//! GH #951 -- an object's document edge resolves in the graph, and only the
//! object's round sees the object.
//!
//! A booted graph space (`support/graph_space_colony.rs`, layout `Alone`);
//! the test plays both sources: a file space (`fh-`) and an object hive
//! (`ob-`), answering every `pull` on `in_pulled`. `/docs/guide.md` holds the
//! sections `x` and `y`; `/docs/index.md` links `guide.md` and the object's
//! address; the object (round `member:p`, `agent:a`) has a root node and a
//! slot, a `doc` edge to `guide.md#sec:x` and an `owner`.
//!
//! 1. The object is pulled TWICE -- `outline` and `links`, no `near` -- and
//!    every pull names its source in `hop.source` (the member routes by it).
//!    Its root node sits at its own address, its round is kept canonical.
//! 2. `doc` -> `fh-…#sec:x` is `resolved`; the `owner` is `external`; the
//!    index's link to the object's address resolves to the object.
//! 3. The object appears only in answers of a round it covers: `resolve` of
//!    its name, `dependents` of the section (and the index file, which
//!    reaches the section only through the object), `deps` of the index
//!    file. A
//!    round with someone the object's round lacks, and a question without a
//!    round, see none of it -- and a question about its address answers as
//!    one about an unknown address.
//! 4. The section is renamed (a new version without `sec:x`): `doc` is
//!    `broken`, listed by `broken` and counted by `stats` -- for the covering
//!    round only; the others count nothing of it.
//! 5. The file moves (same source, new version, new path): the index's
//!    markdown link to the old path is `unresolved` -- not `broken`, and not
//!    resolved through the address it had (D.6 `move`).
//! 6. A file links to the object, then (a new version) to an object that never
//!    was: a round that may not see the object counts both alike in `stats`
//!    -- a link to a hidden `ob-` and a link to an unknown one fall away by
//!    their `target_name`, whatever their state (review O I-1: the `stats`
//!    counters told a resolved, hidden link from an unresolved one and so
//!    whether an object of another round exists).
//!
//! Measured red before: the first pull carried no `hop.source`; an `ob-`
//! source waited for a `near` it never gets, and a `doc` target was no
//! address the graph knew.
//!
//! Free of a paid provider by construction: the graph space has no model and
//! no network. Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use space::{
    Layout, Ports, announce, body_of, graph_db, hop_str, map, next_matching, pulled, quiet, rows,
    wait_for,
};

const F: &str = "fh-f00000000001";
const D: &str = "fh-d00000000002";
const O: &str = "ob-0b0000000001";
const UNKNOWN: &str = "ob-0b0000000777";
const E: &str = "fh-e00000000003";
const V1: &str = "111111111111";
const V2: &str = "222222222222";
const V3: &str = "333333333333";
const OV: &str = "aaaaaaaaaaaa";
/// The object's round as the object hive sends it -- not yet canonical.
const ROUND: &str = r#"["member:p","agent:a"]"#;
/// Inside the object's round: nobody here is missing from it.
const INSIDE: &str = r#"["member:p"]"#;
/// `peer:q` is missing from the object's round.
const OUTSIDE: &str = r#"["member:p","peer:q"]"#;

fn message(hop: Value, ctx: Value, body: Value) -> Message {
    MessageBuilder::new(Path::new("/graph-space"))
        .hop(map(hop))
        .context(map(ctx))
        .body(Body::Inline(body))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// The object hive's announcement (objects@1.0.0 `source_changed`).
fn object_changed(nodes: u64, links: u64) -> Message {
    message(
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": O, "version": OV, "path": "", "fmt": "object", "parser": "objects",
               "mark": "", "nodes": nodes, "links": links, "tomb": false,
               "audience_set": ROUND, "messages": []}),
    )
}

/// Answer the pulls of one announcement, part by part; every pull must name
/// `source` in `hop.source`.
async fn answer_pulls(
    h: &meclaw_testing::ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    source: &str,
    version: &str,
    answers: &[(&str, Value)],
) {
    let mut held = Vec::new();
    for (part, body) in answers {
        let id = format!("gs:{part}:{source}:{version}");
        let m = next_matching(
            &mut ports.pulls,
            root,
            &format!("the `{part}` pull of {source}"),
            |m| hop_str(m, "op_id") == id,
            &mut held,
        )
        .await;
        assert_eq!(
            hop_str(&m, "source"),
            source,
            "every pull names its source in `hop.source`, the member's switch (#951): {:?}",
            m.headers.hop
        );
        assert_eq!(body_of(&m)["source"], json!(source), "and in its body");
        h.send(pulled(&m, body.clone())).await;
    }
}

async fn index_file(
    h: &meclaw_testing::ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    (source, version, path): (&str, &str, &str),
    nodes: Value,
    links: Value,
) {
    let n = nodes.as_array().map_or(0, Vec::len) as u64;
    let l = links.as_array().map_or(0, Vec::len) as u64;
    h.send(announce(source, version, path, n, l, false)).await;
    answer_pulls(
        h,
        ports,
        root,
        source,
        version,
        &[
            ("outline", json!({"nodes": nodes})),
            ("links", json!({"links": links})),
            ("near", json!({"near": []})),
        ],
    )
    .await;
}

/// One question in a round (`ctx`), the body of its one answer.
async fn ask_in(
    h: &meclaw_testing::ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    (op, op_id): (&str, &str),
    args: Value,
    ctx: Value,
) -> Value {
    h.send(message(
        json!({"route": "in_graph", "op": op, "op_id": op_id}),
        ctx,
        json!({"op": op, "args": args, "messages": []}),
    ))
    .await;
    let mut seen = Vec::new();
    let m = next_matching(
        &mut ports.gsink,
        root,
        &format!("the answer to `{op}` ({op_id})"),
        |m| hop_str(m, "route") == "answer" && hop_str(m, "op_id") == op_id,
        &mut seen,
    )
    .await;
    let b = body_of(&m).clone();
    assert_eq!(b["ok"], json!(true), "{op}: {b}");
    b
}

fn rounds() -> [(&'static str, Value); 3] {
    [
        ("inside", json!({"audience_now": INSIDE})),
        ("outside", json!({"audience_set": OUTSIDE})),
        ("no round", json!({})),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_edge_resolves_and_only_its_round_sees_the_object() {
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
    let sx = format!("{F}#sec:x");

    index_file(
        &h,
        &mut ports,
        &root,
        (F, V1, "/docs/guide.md"),
        json!([{"anchor": "sec:x", "kind": "sec"}, {"anchor": "sec:y", "kind": "sec"}]),
        json!([]),
    )
    .await;
    index_file(
        &h,
        &mut ports,
        &root,
        (D, V1, "/docs/index.md"),
        json!([{"anchor": "sec:index", "kind": "sec"}]),
        json!([{"kind": "link", "from_anchor": "sec:index", "target_name": "guide.md"},
               {"kind": "link", "from_anchor": "sec:index", "target_name": O}]),
    )
    .await;

    // 1. The object: two parts, each naming its source.
    h.send(object_changed(2, 2)).await;
    answer_pulls(
        &h,
        &mut ports,
        &root,
        O,
        OV,
        &[
            (
                "outline",
                json!({"nodes": [
                    {"anchor": "", "kind": "object", "name": "Kettle",
                     "oneline": "thing: a kettle", "parser": "objects"},
                    {"anchor": "slot:what", "kind": "slot", "name": "what",
                     "oneline": "a kettle", "parser": "objects"}]}),
            ),
            (
                "links",
                json!({"links": [
                    {"kind": "doc", "from_anchor": "", "target_name": sx},
                    {"kind": "owner", "from_anchor": "", "target_name": "member:p"}]}),
            ),
        ],
    )
    .await;

    // 2. The edges resolve.
    let obj_edges =
        format!("SELECT kind, state, to_addr FROM edges WHERE from_source = '{O}' ORDER BY kind");
    let to_object = format!("SELECT state FROM edges WHERE to_module = '{O}'");
    wait_for(
        &root,
        "the object's `doc` edge resolves to the section",
        || {
            rows(&db, &obj_edges)
                == vec![
                    vec!["doc".to_string(), "resolved".into(), sx.clone()],
                    vec!["owner".to_string(), "external".into(), String::new()],
                ]
                && rows(&db, &to_object) == vec![vec!["resolved".to_string()]]
        },
    )
    .await;
    quiet(&root).await;
    let pulls_of_o: Vec<Value> = space::message_log(&root)
        .iter()
        .filter(|r| r.to == "/pulls" && r.route() == "pull")
        .filter(|r| r.hop["op_id"].as_str().is_some_and(|i| i.contains(O)))
        .map(|r| json!([r.hop["op"], r.hop["source"]]))
        .collect();
    assert_eq!(
        pulls_of_o,
        vec![json!(["outline", O]), json!(["links", O])],
        "an object hive is pulled twice -- no `near` -- each pull naming its source"
    );
    assert_eq!(
        rows(
            &db,
            &format!("SELECT audience_set FROM sources WHERE source = '{O}'")
        ),
        vec![vec![r#"["agent:a","member:p"]"#.to_string()]],
        "the object's round is kept, canonical"
    );
    assert_eq!(
        rows(
            &db,
            &format!("SELECT addr, anchor FROM nodes WHERE source = '{O}' ORDER BY addr")
        ),
        vec![
            vec![O.to_string(), String::new()],
            vec![format!("{O}#slot:what"), "slot:what".into()]
        ],
        "the object's root node sits at the object's own address"
    );

    // 3. Only the covering round sees the object.
    let mut n = 0;
    for (who, ctx) in rounds() {
        let inside = who == "inside";
        let mut q = |op: &'static str| {
            n += 1;
            (op, format!("q-{n}"))
        };
        let (op, id) = q("resolve");
        let got = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"name": "Kettle"}),
            ctx.clone(),
        )
        .await;
        let want = if inside {
            json!([{"addr": O, "match": "name"}])
        } else {
            json!([])
        };
        assert_eq!(
            got["items"], want,
            "{who}: `resolve` of the object's name: {got}"
        );

        let (op, id) = q("dependents");
        let got = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"addr": sx, "depth": 2}),
            ctx.clone(),
        )
        .await;
        // The index file depends on the section only THROUGH the object: a
        // round that may not see the object sees no path, so no dependent.
        let want = if inside {
            json!([{"addr": O, "depth": 1}, {"addr": format!("{D}#sec:index"), "depth": 2}])
        } else {
            json!([])
        };
        assert_eq!(
            got["items"], want,
            "{who}: `dependents` of the section: {got}"
        );

        let (op, id) = q("deps");
        let got = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"addr": D}),
            ctx.clone(),
        )
        .await;
        let want = if inside {
            json!([{"addr": F, "kind": "link"}, {"addr": O, "kind": "link"}])
        } else {
            json!([{"addr": F, "kind": "link"}])
        };
        assert_eq!(got["items"], want, "{who}: `deps` of the index file: {got}");

        let (op, id) = q("callers");
        let got = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"addr": F}),
            ctx.clone(),
        )
        .await;
        assert_eq!(
            got["items"],
            json!([]),
            "{who}: no call into the file: {got}"
        );

        let (op, id) = q("deps");
        let asked = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"addr": O}),
            ctx.clone(),
        )
        .await;
        let (op, id) = q("deps");
        let unknown = ask_in(
            &h,
            &mut ports,
            &root,
            (op, id.as_str()),
            json!({"addr": UNKNOWN}),
            ctx.clone(),
        )
        .await;
        if inside {
            assert_eq!(
                asked["items"],
                json!([{"addr": sx, "kind": "doc"}]),
                "{asked}"
            );
        } else {
            let strip = |v: &Value| {
                let mut v = v.clone();
                for k in ["addr", "op_id"] {
                    v[k] = json!("");
                }
                v
            };
            assert_eq!(
                strip(&asked),
                strip(&unknown),
                "{who}: a hidden address answers as an unknown one"
            );
        }
    }

    // 4. The section is renamed: `doc` breaks, and only its round counts it.
    index_file(
        &h,
        &mut ports,
        &root,
        (F, V2, "/docs/guide.md"),
        json!([{"anchor": "sec:z", "kind": "sec"}, {"anchor": "sec:y", "kind": "sec"}]),
        json!([]),
    )
    .await;
    wait_for(&root, "the renamed section breaks the `doc` edge", || {
        rows(&db, &obj_edges).first() == Some(&vec!["doc".to_string(), "broken".into(), sx.clone()])
    })
    .await;
    for (who, ctx) in rounds() {
        let inside = who == "inside";
        n += 1;
        let broken = ask_in(
            &h,
            &mut ports,
            &root,
            ("broken", format!("q-{n}").as_str()),
            json!({}),
            ctx.clone(),
        )
        .await;
        n += 1;
        let stats = ask_in(
            &h,
            &mut ports,
            &root,
            ("stats", format!("q-{n}").as_str()),
            json!({}),
            ctx.clone(),
        )
        .await;
        let (listed, sources, nodes) = if inside { (1, 3, 5) } else { (0, 2, 3) };
        assert_eq!(broken["broken"], json!(listed), "{who}: `broken`: {broken}");
        assert_eq!(
            broken["items"].as_array().map_or(0, Vec::len),
            listed,
            "{who}: `broken` lists the object's edge for its round only: {broken}"
        );
        assert_eq!(
            stats["broken"],
            json!(listed),
            "{who}: `stats` counts it: {stats}"
        );
        assert_eq!(stats["sources"], json!(sources), "{who}: sources: {stats}");
        assert_eq!(stats["nodes"], json!(nodes), "{who}: nodes: {stats}");
    }

    // 5. The file moves: the link to the old path is unresolved.
    index_file(
        &h,
        &mut ports,
        &root,
        (F, V3, "/docs/moved.md"),
        json!([{"anchor": "sec:z", "kind": "sec"}, {"anchor": "sec:y", "kind": "sec"}]),
        json!([]),
    )
    .await;
    let link = format!(
        "SELECT state, to_addr FROM edges WHERE from_source = '{D}' AND target_name = 'guide.md'"
    );
    let moved = format!("SELECT path, version FROM sources WHERE source = '{F}'");
    wait_for(
        &root,
        "the moved file is indexed under its new path",
        || rows(&db, &moved) == vec![vec!["/docs/moved.md".to_string(), V3.into()]],
    )
    .await;
    wait_for(&root, "the link to the old path is unresolved", || {
        rows(&db, &link) == vec![vec!["unresolved".to_string(), String::new()]]
    })
    .await;

    // 6. A link to a hidden object and a link to an object that never was
    //    count alike for a round without the object (review O I-1).
    let note = format!("SELECT state FROM edges WHERE from_source = '{E}'");
    let mut counted = Vec::new();
    for (version, target, state) in [(V1, O, "resolved"), (V2, UNKNOWN, "unresolved")] {
        index_file(
            &h,
            &mut ports,
            &root,
            (E, version, "/docs/note.md"),
            json!([{"anchor": "sec:n", "kind": "sec"}]),
            json!([{"kind": "link", "from_anchor": "sec:n", "target_name": target}]),
        )
        .await;
        wait_for(
            &root,
            &format!("the note's link to {target} is {state}"),
            || rows(&db, &note) == vec![vec![state.to_string()]],
        )
        .await;
        let mut per_round = Vec::new();
        for (who, ctx) in rounds().into_iter().filter(|(w, _)| *w != "inside") {
            n += 1;
            let mut stats = ask_in(
                &h,
                &mut ports,
                &root,
                ("stats", format!("q-{n}").as_str()),
                json!({}),
                ctx,
            )
            .await;
            stats["op_id"] = json!("");
            per_round.push((who, stats));
        }
        counted.push(per_round);
    }
    assert_eq!(
        counted[0], counted[1],
        "a round without the object counts a link to it as one to an object that never was"
    );
    quiet(&root).await;
    let last = rows(&db, &link);
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        last,
        vec![vec!["unresolved".to_string(), String::new()]],
        "a link names a path: after the move it is unresolved, not broken"
    );
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
