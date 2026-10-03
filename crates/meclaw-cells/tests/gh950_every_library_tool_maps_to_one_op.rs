//! GH #950 -- the library tools on a model's menu, the librarian's half (the
//! pattern of `gh908_every_file_tool_maps_to_one_op`). The shipped
//! `templates/librarian/tools` and `.../schemas` programs, their pure halves
//! loaded by `ast` (`support/graph_space_pure.rs` `pure_at`), and the hive
//! booted by itself for the calls (`support/librarian_hive.rs`), the test
//! playing the file space and the graph space.
//!
//! 1. **One list, one op per name.** `LIB_OFFER` holds four tools --
//!    `lib_find`, `lib_symbol`, `lib_related`, `lib_tree` -- and every name is
//!    `lib_<op>` of an op `query` serves (its `OPS`).
//! 2. **Two copies, one value.** `./schemas` hands the list out and `./tools`
//!    checks every call against it; a code cell shares no library, so the text
//!    between the `--8<--` markers stands word for word in both, and the two
//!    values are held equal here.
//! 3. **Every schema is a JSON Schema of the checked subset** with exactly the
//!    arguments of GH #950 § 5, and every description names the address form
//!    `fh-<12 hex>#<anchor>` and `file_read`, which reads a hit's span.
//! 4. **An argument is typed before a question leaves**: `check` refuses what
//!    the schema refuses.
//! 5. **A valid call is ONE question**: on `in_lib` from `./tools` to
//!    `./query`, `op_id` `tools:<call id>`, `caller` 'tools' -- and comes back
//!    as ONE `tool_result` under the call id. `lib_symbol` and `lib_related`
//!    ask the graph space and `lib_tree` the file space; such a pull carries no
//!    `caller`, keeps the round of the call (`context.audience_set`), and its
//!    answer is enriched from the catalog.
//! 6. **A wrong call is answered without a question**: a mistyped argument,
//!    arguments that are no JSON object or a call without an id are
//!    `bad_request`, a `lib_*` name the librarian does not serve is
//!    `tool_unknown`.
//! 7. **The menu**: `*` answers the four schemas in menu order; an unknown
//!    name is `tool_unknown`, a question without a list `tools_missing`.
//! 8. **A tree is a page, too**: `lib_tree` takes `limit` and `cursor` like
//!    every other library tool and hands at most 20 `entries` to the model,
//!    ordered by path, with `next` for the rest -- the file space's `list`
//!    behind it answers up to two thousand rows at `depth` 3, and the tool
//!    adapter cuts nothing (`result_text`).
//! 9. **Every brain stamps its own name**: the assistant level carries a
//!    `lib_*` call out on one edge per brain, and that edge sets
//!    `context.tool_caller` to the brain's own name -- the `tool_result` finds
//!    its round by it, so a swapped stamp would hand one channel's answer to
//!    the other. The edge is the `file_*` edge of the same brain with the
//!    other prefix: same route, same keys deleted.
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "support/librarian_hive.rs"]
mod librarian;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_pure.rs"]
mod pure;
#[path = "support/graph_space_colony.rs"]
mod space;

use librarian::{Doc, Ports};
use meclaw_core::Message;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_testing::ColonyHandle;
use space::{Logged, body_of, hop_str};
use std::collections::BTreeSet;

const TOOLS_CFG: &str = "templates/librarian/tools/config.json";
const SCHEMAS_CFG: &str = "templates/librarian/schemas/config.json";
const QUERY_CFG: &str = "templates/librarian/query/config.json";
const NAMES: [&str; 4] = ["lib_find", "lib_symbol", "lib_related", "lib_tree"];
/// The round of the call, as the core stamps it; a pull keeps it.
const AUDIENCE: &str = "[\"member:m-1\"]";
const FILE: &str = "fh-0d0000000001";

fn tools(probe: &str, args: Value) -> Value {
    pure::pure_at(TOOLS_CFG, "", probe, args)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// The text between the two `--8<--` markers of one shipped script.
fn offer_block(rel: &str) -> String {
    let cfg = space::read_json(&space::repo(rel));
    let src = cfg["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel}: a `script_inline` program"));
    let start = src
        .find("# --8<-- lib-offer")
        .unwrap_or_else(|| panic!("{rel}: no `# --8<-- lib-offer` marker"));
    let end = src
        .find("# --8<-- end lib-offer")
        .unwrap_or_else(|| panic!("{rel}: no `# --8<-- end lib-offer` marker"));
    assert!(start < end, "{rel}: the markers are in order");
    src[start..end].to_string()
}

/// The arguments of each tool (GH #950 § 5).
fn arguments(name: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match name {
        "lib_find" => (&["q", "kind", "fmt", "under", "limit", "cursor"], &["q"]),
        "lib_symbol" => (&["name", "limit", "cursor"], &["name"]),
        "lib_related" => (
            &["addr", "how", "depth", "limit", "cursor"],
            &["addr", "how"],
        ),
        "lib_tree" => (&["under", "depth", "limit", "cursor"], &[]),
        other => panic!("{other} is no library tool"),
    }
}

#[test]
fn the_menu_and_the_checker_hold_one_list() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    assert_eq!(
        offer_block(TOOLS_CFG),
        offer_block(SCHEMAS_CFG),
        "`LIB_OFFER` stands word for word between the markers of ./tools and ./schemas"
    );
    let a = pure::pure_at(SCHEMAS_CFG, "", "LIB_OFFER", json!(null));
    let b = tools("LIB_OFFER", json!(null));
    assert_eq!(a, b, "./schemas and ./tools carry one value");
}

#[test]
fn every_library_tool_maps_to_one_op_the_query_serves() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let offer = tools("LIB_OFFER", json!(null));
    let names: Vec<String> = offer
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    let distinct: BTreeSet<String> = names.iter().cloned().collect();
    let want: BTreeSet<String> = NAMES.iter().map(|n| n.to_string()).collect();
    assert_eq!(distinct, want, "the four library tools: {offer}");
    assert_eq!(names.len(), 4, "each once: {names:?}");
    let ops: BTreeSet<String> = strings(&pure::pure_at(QUERY_CFG, "", "list(OPS)", json!(null)))
        .into_iter()
        .collect();
    for n in &names {
        let op = n
            .strip_prefix("lib_")
            .unwrap_or_else(|| panic!("{n}: the name IS the mapping, `lib_<op>`"));
        assert!(
            ops.contains(op),
            "`{n}` asks `{op}`, which ./query does not serve ({ops:?})"
        );
    }
}

#[test]
fn every_schema_is_a_checked_json_schema_and_says_how_to_read_a_hit() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let offer = tools("LIB_OFFER", json!(null));
    let kinds: BTreeSet<&str> = ["string", "integer", "boolean", "array"].into();
    for t in offer.as_array().expect("a list") {
        let name = t["name"].as_str().expect("a name");
        let d = t["description"].as_str().unwrap_or_default();
        assert!(
            d.contains("fh-<12 hex>#<anchor>"),
            "{name}: the description names the address form: {d}"
        );
        assert!(
            d.contains("file_read"),
            "{name}: the description says `file_read` reads a hit: {d}"
        );
        let p = &t["parameters"];
        assert_eq!(p["type"], json!("object"), "{name}");
        assert_eq!(p["additionalProperties"], json!(false), "{name}");
        let props = p["properties"].as_object().cloned().unwrap_or_default();
        let (args, required) = arguments(name);
        let have: BTreeSet<&str> = props.keys().map(String::as_str).collect();
        let want: BTreeSet<&str> = args.iter().copied().collect();
        assert_eq!(have, want, "{name}: its arguments");
        let have: BTreeSet<String> = strings(&p["required"]).into_iter().collect();
        let want: BTreeSet<String> = required.iter().map(|r| r.to_string()).collect();
        assert_eq!(have, want, "{name}: its required arguments");
        for (k, s) in &props {
            let ok = match &s["type"] {
                Value::String(x) => kinds.contains(x.as_str()),
                Value::Array(xs) => xs
                    .iter()
                    .all(|x| x.as_str().is_some_and(|x| kinds.contains(x))),
                _ => false,
            };
            assert!(ok, "{name}.{k}: a checked type: {s}");
        }
        if let Some(l) = props.get("limit") {
            assert_eq!(l["type"], json!("integer"), "{name}.limit: {l}");
            assert_eq!(l["minimum"], json!(1), "{name}.limit: {l}");
            assert_eq!(l["maximum"], json!(20), "{name}.limit: {l}");
        }
        if let Some(dp) = props.get("depth") {
            assert_eq!(dp["type"], json!("integer"), "{name}.depth: {dp}");
            assert_eq!(dp["minimum"], json!(1), "{name}.depth: {dp}");
            assert_eq!(dp["maximum"], json!(3), "{name}.depth: {dp}");
        }
        if let Some(how) = props.get("how") {
            assert_eq!(
                how["enum"],
                json!(["callers", "dependents", "deps", "similar"]),
                "{name}.how: {how}"
            );
        }
    }
}

#[test]
fn the_checker_types_every_argument() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // [tool, arguments, the argument the check names -- '' when they hold]
    let cases = json!([
        ["lib_find", {"q": "x"}, ""],
        ["lib_find", {"q": "x", "kind": "text", "fmt": "python", "under": "/pkg",
                      "limit": 20, "cursor": "20"}, ""],
        ["lib_find", {"q": "x", "limit": "ten"}, "limit"],
        ["lib_find", {"q": "x", "limit": true}, "limit"],
        ["lib_find", {"q": "x", "limit": 21}, "limit"],
        ["lib_find", {"q": "x", "limit": 0}, "limit"],
        ["lib_find", {"limit": 5}, "q"],
        ["lib_find", {"q": 7}, "q"],
        ["lib_find", {"q": "x", "bogus": 1}, "bogus"],
        ["lib_symbol", {"name": "load"}, ""],
        ["lib_symbol", {}, "name"],
        ["lib_related", {"addr": "fh-0123456789ab", "how": "callers"}, ""],
        ["lib_related", {"addr": "fh-0123456789ab#def:f", "how": "dependents", "depth": 3}, ""],
        ["lib_related", {"addr": "fh-0123456789ab", "how": "sideways"}, "how"],
        ["lib_related", {"addr": "fh-0123456789ab", "how": "dependents", "depth": 4}, "depth"],
        ["lib_related", {"how": "callers"}, "addr"],
        ["lib_tree", {}, ""],
        ["lib_tree", {"under": "/pkg", "depth": 3}, ""],
        ["lib_tree", {"depth": 0}, "depth"],
        ["lib_tree", {"depth": "2"}, "depth"],
        ["lib_tree", {"under": "/pkg", "limit": 20, "cursor": "20"}, ""],
        ["lib_tree", {"limit": 21}, "limit"],
        ["lib_tree", {"cursor": 20}, "cursor"]
    ]);
    let got = tools(
        "[check([t for t in LIB_OFFER if t['name'] == n][0]['parameters'], a) \
         for n, a, _ in ARGS]",
        cases.clone(),
    );
    for (i, c) in cases.as_array().expect("cases").iter().enumerate() {
        let problems = strings(&got[i]);
        let key = c[2].as_str().unwrap_or_default();
        if key.is_empty() {
            assert!(
                problems.is_empty(),
                "{}: {} holds, got {problems:?}",
                c[0],
                c[1]
            );
        } else {
            let at = format!("arguments.{key}");
            assert!(
                !problems.is_empty() && problems.iter().all(|p| p.contains(&at)),
                "{}: {} is refused for `{key}`, got {problems:?}",
                c[0],
                c[1]
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════ the colony

fn ctx() -> Value {
    json!({"tool_caller": "talky", "audience_set": AUDIENCE})
}

/// Every question `./tools` asked for call `id`.
fn questions<'a>(log: &'a [Logged], id: &str) -> Vec<&'a Logged> {
    let op_id = json!(format!("tools:{id}"));
    log.iter()
        .filter(|r| r.from == librarian::TOOLS && r.route() == "in_lib" && r.hop["op_id"] == op_id)
        .collect()
}

/// The next pull whose `op_id` starts with `prefix`: no `caller`, the round
/// of the call kept, the hive's working keys gone.
async fn pull_of(ports: &mut Ports, root: &std::path::Path, prefix: &str) -> Message {
    let mut seen = Vec::new();
    let m = space::next_matching(
        &mut ports.pulls,
        root,
        &format!("a pull `{prefix}...`"),
        |m| hop_str(m, "op_id").starts_with(prefix),
        &mut seen,
    )
    .await;
    assert_eq!(hop_str(&m, "route"), "pull");
    assert!(
        !m.headers.hop.contains_key("caller"),
        "a pull carries no `caller` (GH #950 § 1): {:?}",
        m.headers.hop
    );
    assert_eq!(
        m.headers.context.get("audience_set"),
        Some(&json!(AUDIENCE)),
        "the question keeps the round of the call (GH #950 § 4): {:?}",
        m.headers.context
    );
    for k in ["cur_origin", "cur_phase", "cur_call"] {
        assert!(
            !m.headers.context.contains_key(k),
            "`{k}` does not leave the hive: {:?}",
            m.headers.context
        );
    }
    m
}

fn assert_good(hop: &Map<String, Value>, a: &Value, op: &str) {
    assert_eq!(a["ok"], json!(true), "lib_{op}: {a}");
    assert_eq!(a["op"], json!(op), "lib_{op}: {a}");
    assert!(
        hop.get("error_code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "lib_{op}: a good answer carries no error code: {hop:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_library_tool_becomes_one_question_and_comes_back_as_one_result() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;
    // The summary line reaches the catalog on the file's `source_described`
    // (`librarian::index_files`), never on its `info` (GH #950, OR-BC-68).
    let file = Doc::new(FILE, "0123456789ab", "/pkg/app.py")
        .with_oneline("The app.")
        .with_names(&["run", "main"]);
    librarian::index_files(&h, &mut ports, &root, std::slice::from_ref(&file)).await;
    let run = format!("{FILE}#def:run");
    let main = format!("{FILE}#def:main");
    let call = |name: &str, id: &str, args: Value| {
        librarian::tool_call(name, id, &args.to_string(), ctx())
    };

    // find: the catalog alone answers.
    h.send(call("lib_find", "call-find", json!({"q": "app"})))
        .await;
    let (hop, a) = librarian::tool_result(&mut ports, &root, "call-find").await;
    assert_good(&hop, &a, "find");
    assert_eq!(a["items"][0]["file"], json!(FILE), "{a}");

    // symbol: a question to the graph space, its hits enriched from the catalog.
    h.send(call("lib_symbol", "call-symbol", json!({"name": "run"})))
        .await;
    let pull = pull_of(&mut ports, &root, "lib:g:").await;
    assert_eq!(hop_str(&pull, "op"), "resolve");
    let b = body_of(&pull);
    assert_eq!(b["op"], json!("resolve"), "{b}");
    assert_eq!(b["args"]["name"], json!("run"), "{b}");
    h.send(librarian::answer_pull(
        &pull,
        json!({"name": "run", "items": [{"addr": run, "match": "name"}], "next": "",
               "broken": 0}),
    ))
    .await;
    let (hop, a) = librarian::tool_result(&mut ports, &root, "call-symbol").await;
    assert_good(&hop, &a, "symbol");
    let hit = &a["items"][0];
    assert_eq!(hit["addr"], json!(run), "{a}");
    assert_eq!(hit["path"], json!("/pkg/app.py"), "{a}");
    assert_eq!(hit["oneline"], json!("The app."), "{a}");
    assert_eq!(hit["match"], json!("name"), "{a}");

    // related: the same road, the graph's own fields kept.
    h.send(call(
        "lib_related",
        "call-related",
        json!({"addr": run, "how": "callers"}),
    ))
    .await;
    let pull = pull_of(&mut ports, &root, "lib:g:").await;
    assert_eq!(hop_str(&pull, "op"), "callers");
    let b = body_of(&pull);
    assert_eq!(b["op"], json!("callers"), "{b}");
    assert_eq!(b["args"]["addr"], json!(run), "{b}");
    h.send(librarian::answer_pull(
        &pull,
        json!({"addr": run, "items": [{"addr": main}], "next": "", "broken": 0}),
    ))
    .await;
    let (hop, a) = librarian::tool_result(&mut ports, &root, "call-related").await;
    assert_good(&hop, &a, "related");
    assert_eq!(a["addr"], json!(run), "{a}");
    assert_eq!(a["how"], json!("callers"), "{a}");
    let hit = &a["items"][0];
    assert_eq!(hit["addr"], json!(main), "{a}");
    assert_eq!(hit["path"], json!("/pkg/app.py"), "{a}");
    assert_eq!(hit["oneline"], json!("The app."), "{a}");

    // tree: a question to the file space.
    h.send(call(
        "lib_tree",
        "call-tree",
        json!({"under": "/pkg", "depth": 2}),
    ))
    .await;
    let pull = pull_of(&mut ports, &root, "lib:f:t:").await;
    assert_eq!(hop_str(&pull, "op"), "list");
    let b = body_of(&pull);
    assert_eq!(b["op"], json!("list"), "{b}");
    assert_eq!(b["args"]["prefix"], json!("/pkg"), "{b}");
    assert_eq!(b["args"]["depth"], json!(2), "{b}");
    let entries = json!([{"path": "/pkg/app.py", "file": FILE}]);
    h.send(librarian::answer_pull(
        &pull,
        json!({"entries": entries, "next": ""}),
    ))
    .await;
    let (hop, a) = librarian::tool_result(&mut ports, &root, "call-tree").await;
    assert_good(&hop, &a, "tree");
    assert_eq!(a["under"], json!("/pkg"), "{a}");
    assert_eq!(a["depth"], json!(2), "{a}");
    assert_eq!(a["entries"], entries, "{a}");
    assert_eq!(a["next"], json!(""), "{a}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    for (id, op) in [
        ("call-find", "find"),
        ("call-symbol", "symbol"),
        ("call-related", "related"),
        ("call-tree", "tree"),
    ] {
        let asked = questions(&log, id);
        assert_eq!(
            asked.len(),
            1,
            "`lib_{op}` is exactly one question: {:#?}",
            asked.iter().map(|r| r.say()).collect::<Vec<_>>()
        );
        let q = asked[0];
        assert_eq!(q.to, librarian::QUERY, "{}", q.say());
        assert_eq!(q.hop["op"], json!(op), "{}", q.hop);
        assert_eq!(q.hop["caller"], json!("tools"), "{}", q.hop);
    }
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

/// `n` entries of a file space's `list` under `/pkg`, handed in reverse path
/// order: the page is ordered by path whatever order it came in.
fn listing(n: usize) -> Vec<Value> {
    (0..n)
        .rev()
        .map(|i| {
            json!({"path": format!("/pkg/m{i:02}.py"), "file": format!("fh-0d00000001{i:02}"),
                   "bytes": 1, "lines": 1})
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tree_is_a_page_of_at_most_twenty_entries() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;
    // The file space answers every `list` with all 21 entries (and its own
    // cut, `more`); the librarian hands the model one page of them.
    let all = listing(21);
    let mut by_path = all.clone();
    by_path.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    // [call id, arguments, the entries of the page, its `next`]
    let cases = [
        (
            "tree-first",
            json!({"under": "/pkg"}),
            by_path[..20].to_vec(),
            "20",
        ),
        (
            "tree-rest",
            json!({"under": "/pkg", "cursor": "20"}),
            by_path[20..].to_vec(),
            "",
        ),
        (
            "tree-five",
            json!({"under": "/pkg", "limit": 5, "cursor": "5"}),
            by_path[5..10].to_vec(),
            "10",
        ),
        (
            "tree-past",
            json!({"under": "/pkg", "cursor": "40"}),
            vec![],
            "",
        ),
    ];
    for (id, args, want, next) in &cases {
        h.send(librarian::tool_call(
            "lib_tree",
            id,
            &args.to_string(),
            ctx(),
        ))
        .await;
        let pull = pull_of(&mut ports, &root, "lib:f:t:").await;
        assert_eq!(hop_str(&pull, "op"), "list");
        h.send(librarian::answer_pull(
            &pull,
            json!({"entries": all, "more": true}),
        ))
        .await;
        let (hop, a) = librarian::tool_result(&mut ports, &root, id).await;
        assert_good(&hop, &a, "tree");
        assert_eq!(
            a["entries"],
            Value::Array(want.clone()),
            "{id} {args}: at most `limit` (20) entries, ordered by path: {a}"
        );
        assert_eq!(a["next"], json!(next), "{id} {args}: {a}");
        // GH #973 M-8 (OR-BC.B.8): a tree page carries `entries` only -- no
        // `items` twin beside them.
        assert!(a.get("items").is_none(), "{id}: a tree has no items: {a}");
        assert_eq!(
            a["more"],
            json!(true),
            "{id}: the file space's own cut is told: {a}"
        );
    }
    // The cursor is an offset, as for `find`: anything else is refused before
    // the file space is asked.
    h.send(librarian::tool_call(
        "lib_tree",
        "tree-bad-cursor",
        &json!({"cursor": "next"}).to_string(),
        ctx(),
    ))
    .await;
    let (_, a) = librarian::tool_result(&mut ports, &root, "tree-bad-cursor").await;
    assert_eq!(a["ok"], json!(false), "{a}");
    assert_eq!(a["error"]["code"], json!("invalid_input"), "{a}");
    // GH #973 M-7: a superscript two passed `str.isdigit()` and crashed the
    // page; a cursor is ASCII digits, everything else is refused.
    h.send(librarian::tool_call(
        "lib_tree",
        "tree-bad-cursor-sup",
        &json!({"cursor": "\u{b2}"}).to_string(),
        ctx(),
    ))
    .await;
    let (_, a) = librarian::tool_result(&mut ports, &root, "tree-bad-cursor-sup").await;
    assert_eq!(a["ok"], json!(false), "{a}");
    assert_eq!(a["error"]["code"], json!("invalid_input"), "{a}");

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    assert_eq!(
        librarian::pulls_in(&log).len(),
        cases.len(),
        "one `list` per page, none for a refused cursor"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

const ASSISTANT_CFG: &str = "templates/assistant/config.json";
/// The brains of the assistant level that call tools.
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];

/// The assistant level's edges that carry a `<prefix>*` call.
fn carried(edges: &[Value], prefix: &str) -> Vec<Value> {
    let cond = format!("hop.tool_name.startsWith('{prefix}')");
    edges
        .iter()
        .filter(|e| e["condition"].as_str().is_some_and(|c| c.contains(&cond)))
        .cloned()
        .collect()
}

#[test]
fn every_brain_stamps_its_own_name_on_a_library_call() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let cfg = space::read_json(&space::repo(ASSISTANT_CFG));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let lib = carried(&edges, "lib_");
    let file = carried(&edges, "file_");
    assert_eq!(
        lib.len(),
        BRAINS.len(),
        "one `lib_*` edge per brain, no other: {lib:#?}"
    );
    for brain in BRAINS {
        let from = json!(format!("./{brain}"));
        let mine: Vec<&Value> = lib.iter().filter(|e| e["from"] == from).collect();
        assert_eq!(
            mine.len(),
            1,
            "`./{brain}` carries a `lib_*` call out on exactly one edge: {lib:#?}"
        );
        let e = mine[0];
        assert_eq!(e["to"], json!("."), "{brain}: out of the level: {e}");
        let m = &e["modifier"];
        assert_eq!(
            m["set_context"],
            json!({"tool_caller": format!("'{brain}'")}),
            "{brain}: the edge stamps the brain's own name and nothing else -- the \
             `tool_result` finds its round by it: {e}"
        );
        assert_eq!(m["set_hop"], json!({"route": "'tool'"}), "{brain}: {e}");
        let twin: Vec<&Value> = file.iter().filter(|f| f["from"] == from).collect();
        assert_eq!(twin.len(), 1, "{brain}: its `file_*` edge: {file:#?}");
        assert_eq!(
            m["delete_context"], twin[0]["modifier"]["delete_context"],
            "{brain}: the keys the `file_*` edge deletes: {e}"
        );
        assert_eq!(
            e["condition"]
                .as_str()
                .map(|c| c.replace("'lib_'", "'file_'")),
            twin[0]["condition"].as_str().map(str::to_string),
            "{brain}: the condition of the `file_*` edge, the prefix aside: {e}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_call_is_answered_without_a_question() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;

    let cases = [
        (
            "lib_find",
            "c-ten",
            json!({"q": "x", "limit": "ten"}).to_string(),
            "bad_request",
        ),
        (
            "lib_find",
            "c-many",
            json!({"q": "x", "limit": 21}).to_string(),
            "bad_request",
        ),
        (
            "lib_find",
            "c-bogus",
            json!({"q": "x", "bogus": 1}).to_string(),
            "bad_request",
        ),
        (
            "lib_find",
            "c-no-q",
            json!({"limit": 5}).to_string(),
            "bad_request",
        ),
        (
            "lib_related",
            "c-how",
            json!({"addr": "fh-0123456789ab", "how": "sideways"}).to_string(),
            "bad_request",
        ),
        (
            "lib_tree",
            "c-deep",
            json!({"depth": 9}).to_string(),
            "bad_request",
        ),
        (
            "lib_symbol",
            "c-text",
            "not json".to_string(),
            "bad_request",
        ),
        ("lib_nope", "c-nope", "{}".to_string(), "tool_unknown"),
        ("lib_find", "", json!({"q": "x"}).to_string(), "bad_request"),
    ];
    for (name, id, text, code) in &cases {
        h.send(librarian::tool_call(name, id, text, ctx())).await;
        let (hop, a) = librarian::tool_result(&mut ports, &root, id).await;
        assert_eq!(a["ok"], json!(false), "{name} {text}: {a}");
        assert_eq!(a["error"]["code"], json!(code), "{name} {text}: {a}");
        assert_eq!(
            hop.get("error_code").and_then(Value::as_str),
            Some(*code),
            "{name} {text}: the hop names the code: {hop:?}"
        );
    }

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    let asked: Vec<String> = log
        .iter()
        .filter(|r| r.from == librarian::TOOLS && r.route() == "in_lib")
        .map(|r| r.say())
        .collect();
    assert!(asked.is_empty(), "a refused call asks nothing: {asked:#?}");
    assert!(
        log.iter().all(|r| r.to != librarian::QUERY),
        "no question reached ./query"
    );
    assert!(
        librarian::pulls_in(&log).is_empty(),
        "a refused call asks no source"
    );
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}

/// One menu question and its one answer on `/msink`: (hop, body).
async fn menu(
    h: &ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    asked: Option<Value>,
) -> (Map<String, Value>, Value) {
    h.send(librarian::schemas_question(asked)).await;
    let mut seen = Vec::new();
    let m = space::next_matching(
        &mut ports.msink,
        root,
        "the menu's answer",
        |m| hop_str(m, "route") == "tool_schemas",
        &mut seen,
    )
    .await;
    assert_eq!(hop_str(&m, "operation"), "schemas", "{:?}", m.headers.hop);
    (m.headers.hop.clone(), body_of(&m).clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_menu_hands_out_the_four_tools() {
    if !librarian::shipped() {
        eprintln!("the template library did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    librarian::build(&td);
    let root = td.path().to_path_buf();
    let (h, mut ports) = librarian::boot(&td).await;
    let offer: Vec<Value> = tools("LIB_OFFER", json!(null))
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            json!({"name": t["name"], "description": t["description"],
                   "parameters": t["parameters"]})
        })
        .collect();

    let (hop, b) = menu(&h, &mut ports, &root, Some(json!(["*"]))).await;
    assert_eq!(
        b["schemas"],
        Value::Array(offer),
        "`*` hands out the four tools in menu order"
    );
    assert_eq!(b["unknown"], json!([]), "{b}");
    assert_eq!(b["sidecar"], json!([]), "{b}");
    assert_eq!(hop.get("schema_count"), Some(&json!(4)), "{hop:?}");
    assert!(!hop.contains_key("error_code"), "{hop:?}");

    let (hop, b) = menu(
        &h,
        &mut ports,
        &root,
        Some(json!(["lib_find", "file_read", "lib_find"])),
    )
    .await;
    let names: Vec<Value> = b["schemas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| s["name"].clone())
        .collect();
    assert_eq!(names, [json!("lib_find")], "each asked name once: {b}");
    assert_eq!(b["unknown"], json!(["file_read"]), "{b}");
    assert_eq!(
        hop.get("error_code"),
        Some(&json!("tool_unknown")),
        "{hop:?}"
    );

    let (hop, b) = menu(&h, &mut ports, &root, None).await;
    assert_eq!(b["schemas"], json!([]), "{b}");
    assert_eq!(
        hop.get("error_code"),
        Some(&json!("tools_missing")),
        "{hop:?}"
    );

    space::quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    h.shutdown().await;
    librarian::emissions_within_contract(&log);
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
