//! GH #950 -- a model finds a function by name and reads it (R-28-15).
//!
//! The proof lock of the ruling "a procedure becomes code": what a model did
//! by hand before -- list a tree, open files one after another, guess where a
//! function lives -- becomes three tool calls against the member's own
//! librarian and file space, and none of them reads a file the model did not
//! ask for. On a real colony (`support/librarian_member.rs`, the member at the
//! root): the SHIPPED `file-space`, `graph-space` and `librarian`, wired by the
//! member's own edges, and a stand-in for the assistant level that places the
//! calls as the core does (`context.tool_caller` 'cogny', the arguments as the
//! text of one `tool_call` turn) through the member's tool edges.
//!
//! The fixture is a python package, because only python's `ast` extractor
//! yields exact nodes (`def:f`) and call edges (receipt C1, "for later waves"
//! 7): `/pkg/__init__.py`, `/pkg/b.py` (`f` returns a value nothing else in the
//! fixture holds, `h` after it another one), `/pkg/a.py` (`from pkg.b import
//! f`, and `g` calls `f()`), and a `/README.md` beside them. The run waits on
//! signals, not time: the call edge `a#def:g -> b#def:f` resolved in the
//! graph's own store, and the librarian's catalog holding every file at the
//! version last announced for it, with `f` and `g` among the names.
//!
//! Then the model's three calls, each answered by exactly one tool result:
//!
//! 1. `lib_symbol {name: "f"}` names the address `fh-<b>#def:f`, with `path`
//!    `/pkg/b.py` from the catalog;
//! 2. `file_read {file: <that address>}` reads the span of `f` -- its value is
//!    there, the value of `h` (same file, outside the span) is not;
//! 3. `lib_related {addr: <that address>, how: "callers"}` names `g` in
//!    `/pkg/a.py`.
//!
//! At the seams (the colony's `message_log`): every question the librarian put
//! to a space -- `in_graph` for the two lookups, `in_read` for its index pulls
//! -- carries a `lib:` `op_id`, was delivered exactly once and answered exactly
//! once; the two lookups descend from their own tool calls (a causal chain
//! walked back along the parents, not a time window). The menu question of
//! the member is answered by the library with exactly the four `lib_` tools.
//! Every cell of the librarian ran and said only what its contract declares,
//! the graph space's likewise, and no letter died.
//!
//! Free of a paid provider by construction: the file space's summarizer talks
//! to a local chat stub, its embedder to a local embeddings stub, and the
//! librarian has no model at all. Guarded like every template-reading test
//! (GH #49).

#[path = "support/librarian_member.rs"]
mod librarian;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{Value, json};

/// The member stands at the colony's root, as in the graph space's lock.
const ME: &str = "";

/// The value `f` returns: nothing else in the fixture holds it, so a read
/// that carries it read `f`.
const F_MARK: &str = "9500173";
/// The value `h` returns: `h` stands in the same file after `f`, so a read of
/// `f`'s span must not carry it.
const H_MARK: &str = "7711330";

const INIT: &str = "\"\"\"A small package.\"\"\"\n";
const A_PY: &str = "from pkg.b import f\n\n\ndef g():\n    return f()\n";
const README: &str = "# A small package\n\nOne function of it calls another.\n";

const SYMBOL_ID: &str = "call-950-symbol";
const READ_ID: &str = "call-950-read";
const RELATED_ID: &str = "call-950-related";

fn b_py() -> String {
    format!("def f():\n    return {F_MARK}\n\n\ndef h():\n    return {H_MARK}\n")
}

/// The one item of `answer` whose `path` is `path`.
fn item_at(answer: &Value, path: &str) -> Value {
    librarian::items(answer)
        .into_iter()
        .find(|i| i["path"] == json!(path))
        .unwrap_or_else(|| panic!("no item of the answer names `{path}`: {answer}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_finds_and_reads_a_function() {
    if !librarian::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td, &[ME], &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td, &[ME]).await;
    let mut heard = ports.heard.remove(0);

    // The menu: the member's `schemas` question reaches the librarian and its
    // answer comes back as the library's (the `schemas` and `tool_schemas`
    // edges).
    let menu = librarian::menu(&h, &mut heard, &root, ME).await;

    let (init, _) = librarian::create(&h, &mut heard, &root, ME, "/pkg/__init__.py", INIT).await;
    let (b, _) = librarian::create(&h, &mut heard, &root, ME, "/pkg/b.py", &b_py()).await;
    let (a, _) = librarian::create(&h, &mut heard, &root, ME, "/pkg/a.py", A_PY).await;
    let (readme, _) = librarian::create(&h, &mut heard, &root, ME, "/README.md", README).await;

    let call_edge = format!(
        "SELECT count(*) FROM edges WHERE from_addr = '{a}#def:g' AND to_addr = '{b}#def:f' \
         AND state = 'resolved' AND class = 'extracted'"
    );
    librarian::wait_for(
        &root,
        &[ME],
        "the call g -> f resolved in the graph",
        || librarian::count(&librarian::graph_db(&root, ME), &call_edge) > 0,
    )
    .await;
    let catalog = [
        ("/pkg/__init__.py", None),
        ("/pkg/b.py", Some("f")),
        ("/pkg/a.py", Some("g")),
        ("/README.md", None),
    ];
    for (path, name) in catalog {
        librarian::wait_for(&root, &[ME], &format!("{path} in the catalog"), || {
            librarian::catalogued(&root, ME, path, name)
        })
        .await;
    }
    space::quiet(&root).await;

    // 1. The model names a function and gets its address.
    let symbol = librarian::call(
        &h,
        &mut heard,
        &root,
        ME,
        "lib_symbol",
        SYMBOL_ID,
        &json!({"name": "f"}),
    )
    .await;
    assert_eq!(symbol["ok"], json!(true), "{symbol}");
    let hit = item_at(&symbol, "/pkg/b.py");
    let addr = hit["addr"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        addr,
        format!("{b}#def:f"),
        "`lib_symbol` names the definition's address: {symbol}"
    );

    // 2. It reads exactly that span through the file space's own tool.
    let read = librarian::call(
        &h,
        &mut heard,
        &root,
        ME,
        "file_read",
        READ_ID,
        &json!({"file": addr}),
    )
    .await;
    assert_eq!(read["ok"], json!(true), "{read}");
    let text = read.to_string();
    assert!(
        text.contains(F_MARK),
        "the read of `{addr}` carries the body of `f`: {read}"
    );
    assert!(
        !text.contains(H_MARK),
        "the read of `{addr}` is the span of `f`, not the whole file: {read}"
    );

    // 3. It asks who calls it, and is told the caller's file.
    let related = librarian::call(
        &h,
        &mut heard,
        &root,
        ME,
        "lib_related",
        RELATED_ID,
        &json!({"addr": addr, "how": "callers"}),
    )
    .await;
    assert_eq!(related["ok"], json!(true), "{related}");
    let caller = item_at(&related, "/pkg/a.py");
    assert_eq!(caller["addr"], json!(format!("{a}#def:g")), "{related}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;

    // The menu.
    let want: Vec<String> = librarian::LIB_TOOLS.iter().map(|s| s.to_string()).collect();
    assert_eq!(menu, want, "the library offers its four tools");

    // One question, one answer, at every seam the librarian crossed.
    let (graph, files) = librarian::one_question_one_answer(&log, ME);
    assert_eq!(
        graph.len(),
        2,
        "two lookups, two questions to the graph space: {graph:?}"
    );
    for source in [&init, &b, &a, &readme] {
        for part in ["info", "outline"] {
            let pull = format!("lib:f:i:{part}:{source}:");
            assert!(
                files.iter().any(|id| id.starts_with(&pull)),
                "the librarian pulled `{part}` of {source} from the file space: {files:#?}"
            );
        }
    }
    for (id, op) in [(SYMBOL_ID, "resolve"), (RELATED_ID, "callers")] {
        let asked = librarian::graph_questions_of(&log, ME, id);
        assert_eq!(
            asked.len(),
            1,
            "the call `{id}` put exactly one question to the graph space: {:#?}",
            asked.iter().map(|r| r.say()).collect::<Vec<_>>()
        );
        assert_eq!(asked[0].hop["op"], json!(op), "{}", asked[0].say());
    }
    assert!(
        librarian::graph_questions_of(&log, ME, READ_ID).is_empty(),
        "reading a span asks the file space, not the graph"
    );

    // Every cell ran and said only what it declares; nothing died.
    librarian::librarian_within_contract(&log, ME);
    space::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
