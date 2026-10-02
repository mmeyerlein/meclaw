//! GH #950 -- another member's files never appear (B.5).
//!
//! A librarian catalogues ONE member's files: its own file space announces to
//! it, and it asks its own spaces and nobody else. Two members stand side by
//! side on one colony (`support/librarian_member.rs`, members `a` and `b`,
//! each with the SHIPPED `file-space`, `graph-space` and `librarian` wired by
//! the member's own edges, and each with a stand-in for its assistant level).
//! `b` holds a file whose path and only function name nothing in `a` shares
//! (`/secret_b.py`, `def only_in_b`); `a` holds one of its own. The run waits
//! until both librarians and both graph spaces hold their member's file.
//!
//! Then, through `a`'s door: `lib_find` for `b`'s function name and for its
//! file name, `lib_symbol` for the function, `lib_tree` over the whole tree --
//! and no tool result of `a` shows `b`'s path, `b`'s function name or `b`'s
//! file address (the echo of the question itself aside: a lookup may repeat
//! what it was asked). The same calls through `b`'s door DO show them, and
//! `a`'s door shows `a`'s own file, so a silent door cannot pass for an
//! isolated one.
//!
//! And in the running colony's graph (read the way `/colony/graph` reads it)
//! no edge joins the two members: none has one end under `/a` and the other
//! under `/b`, so in particular none joins `/a/librarian` to `/b/librarian`.
//! The positive half: the graph does carry each member's own announcement
//! edge `./file-space -> ./librarian`. No letter died.
//!
//! Free of a paid provider by construction (local chat and embeddings stubs;
//! the librarian has no model). Guarded like every template-reading test
//! (GH #49).

#[path = "support/librarian_member.rs"]
mod librarian;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{Value, json};

const MEMBERS: [&str; 2] = ["a", "b"];

const SECRET_PATH: &str = "/secret_b.py";
const SECRET_FILE: &str = "secret_b";
const SECRET_NAME: &str = "only_in_b";
const OWN_PATH: &str = "/mine_a.py";
const OWN_FILE: &str = "mine_a";
const OWN_NAME: &str = "only_in_a";

/// What a tool's answer shows of a member's files: the answer without the
/// echo of the question (`q`, `name`, `under`, `args`), which repeats what
/// the caller typed and says nothing about the tree.
fn shown(answer: &Value) -> String {
    let mut a = answer.clone();
    if let Some(o) = a.as_object_mut() {
        for k in ["q", "name", "under", "args"] {
            o.remove(k);
        }
    }
    a.to_string()
}

/// Whether `path` is `member`'s path or lies below it.
fn under(path: &str, member: &str) -> bool {
    path == member || path.starts_with(&format!("{member}/"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_members_files_never_appear() {
    if !librarian::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td, &MEMBERS, &stubs);
    let root = td.path().to_path_buf();
    let (h, ports) = librarian::boot(&td, &MEMBERS).await;
    let mut heard = ports.heard.into_iter();
    let mut heard_a = heard.next().expect("member a's capture");
    let mut heard_b = heard.next().expect("member b's capture");

    let own_text = format!("def {OWN_NAME}():\n    return 1\n");
    let secret_text = format!("def {SECRET_NAME}():\n    return 2\n");
    let (own, _) = librarian::create(&h, &mut heard_a, &root, "a", OWN_PATH, &own_text).await;
    let (secret, _) =
        librarian::create(&h, &mut heard_b, &root, "b", SECRET_PATH, &secret_text).await;
    for (member, path, name) in [("a", OWN_PATH, OWN_NAME), ("b", SECRET_PATH, SECRET_NAME)] {
        librarian::wait_for(
            &root,
            &MEMBERS,
            &format!("{path} in member {member}'s catalog"),
            || librarian::catalogued(&root, member, path, Some(name)),
        )
        .await;
        librarian::wait_for(
            &root,
            &MEMBERS,
            &format!("`{name}` in member {member}'s graph"),
            || librarian::graph_knows(&root, member, name),
        )
        .await;
    }
    space::quiet(&root).await;

    let asks = [
        ("lib_find", json!({"q": SECRET_NAME})),
        ("lib_find", json!({"q": SECRET_FILE})),
        ("lib_symbol", json!({"name": SECRET_NAME})),
        ("lib_tree", json!({"under": "/", "depth": 3})),
    ];

    // Through a's door: b's file never shows.
    let mut through_a = Vec::new();
    for (n, (tool, args)) in asks.iter().enumerate() {
        let id = format!("a-{n}-{tool}");
        let answer = librarian::call(&h, &mut heard_a, &root, "a", tool, &id, args).await;
        assert_eq!(
            answer["ok"],
            json!(true),
            "{tool} {args} through a: {answer}"
        );
        through_a.push((tool.to_string(), args.clone(), answer));
    }
    // ... while a's own file does: the door answers.
    let own_find = librarian::call(
        &h,
        &mut heard_a,
        &root,
        "a",
        "lib_find",
        "a-own-find",
        &json!({"q": OWN_NAME}),
    )
    .await;

    // Through b's door: b's file shows.
    let mut through_b = Vec::new();
    for (n, (tool, args)) in asks.iter().enumerate() {
        let id = format!("b-{n}-{tool}");
        let answer = librarian::call(&h, &mut heard_b, &root, "b", tool, &id, args).await;
        assert_eq!(
            answer["ok"],
            json!(true),
            "{tool} {args} through b: {answer}"
        );
        through_b.push((tool.to_string(), args.clone(), answer));
    }

    let edges = librarian::colony_edges(&h).await;
    space::quiet(&root).await;
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    for (tool, args, answer) in &through_a {
        let seen = shown(answer);
        for marker in [SECRET_FILE, SECRET_NAME, secret.as_str()] {
            assert!(
                !seen.contains(marker),
                "`{tool}` {args} through member a's door shows `{marker}` of member b: {answer}"
            );
        }
    }
    assert!(
        librarian::items(&own_find)
            .iter()
            .any(|i| i["path"] == json!(OWN_PATH)),
        "member a's door finds member a's own file: {own_find}"
    );
    let tree_a = &through_a[3].2;
    assert!(
        shown(tree_a).contains(OWN_FILE),
        "member a's tree lists member a's own file: {tree_a}"
    );

    for (tool, args, answer) in &through_b {
        let seen = shown(answer);
        assert!(
            seen.contains(SECRET_FILE),
            "`{tool}` {args} through member b's door shows member b's file: {answer}"
        );
        assert!(
            !seen.contains(OWN_FILE) && !seen.contains(own.as_str()),
            "`{tool}` {args} through member b's door shows member a's file: {answer}"
        );
    }
    let symbol_b = &through_b[2].2;
    assert!(
        librarian::items(symbol_b).iter().any(|i| {
            i["addr"] == json!(format!("{secret}#def:{SECRET_NAME}"))
                && i["path"] == json!(SECRET_PATH)
        }),
        "member b's `lib_symbol` names its own definition: {symbol_b}"
    );

    // The colony's graph: no edge between the two members.
    let cross: Vec<&(String, String)> = edges
        .iter()
        .filter(|(from, to)| {
            (under(from, "/a") && under(to, "/b")) || (under(from, "/b") && under(to, "/a"))
        })
        .collect();
    assert!(
        cross.is_empty(),
        "an edge joins the two members: {cross:#?}"
    );
    for m in ["/a", "/b"] {
        let own_edge = (format!("{m}/file-space"), format!("{m}/librarian"));
        assert!(
            edges.contains(&own_edge),
            "the graph read carries member {m}'s own announcement edge: {edges:#?}"
        );
    }
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
