//! GH #947 (ledger 11) -- the node bundle of `file-space/derive` holds at the
//! caps. A file with exactly `nodes_max` (2000) items and exactly
//! `links_max` (5000) distinct links goes into the store in ONE bundle of
//! 2000 node legs and 5000 link legs, through the store cell's own
//! dispatcher: every row lands, `node_runs` counts 2000 and 5000 with no
//! mark (at the caps is not over them), no leg is refused, and `outline` and
//! `links` page through all of it without a gap.
//!
//! Why: the node rows go in one bundle so that a reader never sees half a
//! run; that is only true if the largest run the caps allow fits one bundle
//! of the real store, which this lock measures instead of assuming.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder. No embedding endpoint (`embed`
//! off), no provider, no net.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const NODES: usize = 2000;
const LINKS: usize = 5000;

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

/// 2000 top-level functions; the first 1000 call three names, the rest two:
/// 3 * 1000 + 2 * 1000 = 5000 calls, every one a distinct link (the
/// extractor keys a link by its caller's anchor, kind, target and alias).
fn many_defs() -> String {
    (0..NODES)
        .map(|i| {
            if i < 1000 {
                format!("def f{i}():\n    a()\n    b()\n    c()\n")
            } else {
                format!("def f{i}():\n    a()\n    b()\n")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Page through `op` (`outline` or `links`) of `file`; every page's items
/// under `key`, and the number of pages.
fn pages(sp: &mut Space, op: &str, file: &str, key: &str) -> (Vec<Value>, usize) {
    let (mut all, mut cursor, mut n) = (Vec::<Value>::new(), String::new(), 0);
    loop {
        let a = ok(sp.read(op, file, json!({"cursor": cursor})));
        assert!(a.get("mark").is_none(), "no mark at the caps: {a}");
        all.extend(a[key].as_array().cloned().unwrap_or_default());
        n += 1;
        cursor = a["next"].as_str().unwrap_or("").to_string();
        if cursor.is_empty() {
            return (all, n);
        }
        assert!(n < 100, "{op} never ends");
    }
}

#[test]
fn two_thousand_nodes_and_five_thousand_links_go_in_one_bundle() {
    if !shipped() {
        return;
    }
    let derive = cell_config("derive");
    assert_eq!(
        (
            derive["params"]["nodes_max"].clone(),
            derive["params"]["links_max"].clone()
        ),
        (json!(NODES), json!(LINKS)),
        "the caps this lock fills"
    );
    let mut sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    let n = sp.sent.len();
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/big/many.py", "text": many_defs()}),
        json!({}),
    ));
    let f = a["file"].as_str().unwrap().to_string();

    // ONE bundle carries every node and link leg.
    let bundles: Vec<Vec<String>> = sp.sent[n..]
        .iter()
        .filter(|m| {
            m["from"] == json!("./derive")
                && m["to"] == json!("./store")
                && m["header"]["context"]["cur_phase"] == json!("d-nodes")
        })
        .map(|m| {
            m["body"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .collect();
    assert_eq!(bundles.len(), 1, "one node bundle");
    let legs = |p: &str| bundles[0].iter().filter(|id| id.starts_with(p)).count();
    assert_eq!((legs("n-"), legs("l-")), (NODES, LINKS));

    while sp.llm.front().is_some_and(|(c, _)| c == "summarizer") {
        sp.llm_answer("Many functions.\n\nTwo thousand of them.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");

    // Every row landed, the run counts them, no mark, nothing refused.
    let seq = sp.rows(&format!("SELECT head_seq FROM files WHERE file = '{f}'"))[0][0].clone();
    assert_eq!(
        sp.rows(&format!(
            "SELECT fmt, parser, mark, nodes, links, seq FROM node_runs WHERE file = '{f}'"
        )),
        vec![vec![
            json!("python"),
            json!("ast"),
            json!(""),
            json!(NODES),
            json!(LINKS),
            seq
        ]]
    );
    for (table, want) in [("nodes", NODES), ("links", LINKS)] {
        assert_eq!(
            sp.rows(&format!("SELECT COUNT(*) FROM {table} WHERE file = '{f}'")),
            vec![vec![json!(want)]],
            "{table}"
        );
    }
    let ev = sp.routed("source_changed");
    assert_eq!(ev.len(), 1, "one event");
    assert_eq!(
        (
            ev[0].body["nodes"].clone(),
            ev[0].body["links"].clone(),
            ev[0].body["mark"].clone()
        ),
        (json!(NODES), json!(LINKS), json!(""))
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);

    // `outline` pages through every node in source order, `links` through
    // every link, each exactly once.
    let (nodes, n_pages) = pages(&mut sp, "outline", &f, "nodes");
    let anchors: Vec<String> = nodes
        .iter()
        .map(|x| x["anchor"].as_str().unwrap().to_string())
        .collect();
    let want: Vec<String> = (0..NODES).map(|i| format!("def:f{i}")).collect();
    assert_eq!(anchors, want, "the whole outline, in order");
    assert_eq!(n_pages, 10, "200 nodes a page");
    let (links, _) = pages(&mut sp, "links", &f, "links");
    let mut seen: Vec<String> = links
        .iter()
        .map(|l| sj::to_string(&(&l["from_anchor"], &l["target_name"])).unwrap())
        .collect();
    assert_eq!(seen.len(), LINKS);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), LINKS, "every link once");

    // The directory counts the file with its nodes.
    assert_eq!(
        sp.rows("SELECT files, nodes FROM dirs WHERE path = '/big'"),
        vec![vec![json!(1), json!(NODES)]]
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
}
