//! GH #947 (ledger 12 and 13) -- the node rows of a file belong to its head,
//! and no bundle of `file-space/derive` takes them from it.
//!
//! Ledger 12: a derive job of an older version may have read "I am the
//! head" and still reach the store AFTER the job of the newer head. Its node
//! bundle deletes by version (`{file, version}`) and below its own main-line
//! position (`{file, seq < its seq}`, rows without a seq too), never by file
//! alone -- so the newer head's rows (a higher `seq`) stay, and the older
//! rows it wrote are taken back by version once it sees the head moved.
//!
//! Ledger 13: a node bundle is no transaction. When the store refuses one
//! leg, the job deletes the version's `node_runs` row FIRST, then its nodes
//! and links: no run row is left that counts rows that are not there, no
//! `source_changed` goes out, and `outline` / `links` answer in full from the
//! blocks, as for a version that has no run row.
//!
//! Both interleavings are made deterministic by SQLite triggers on the
//! harness's store (a writer outside the space, the way `Space::before`
//! is): for 12 the head the older job reads is turned to it for that one
//! read; for 13 one node insert is aborted.
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder. No embedding endpoint (`embed`
//! off), no provider, no net.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const OLD: &str = "def a():\n    pass\n";
const NEW: &str = "def b():\n    pass\n\ndef c():\n    pass\n";
const M_PY: &str = "def a():\n    pass\n\ndef b():\n    return a()\n\ndef c():\n    pass\n";

fn plain() -> Space {
    Space::with("/x/files", &[("derive", "embed", json!("0"))])
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn settle(sp: &mut Space) {
    while sp.llm.front().is_some_and(|(c, _)| c == "summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn count(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

/// `in_derive` as `./write` sends it after a head move.
fn in_derive(file: &str, version: &str) -> (String, Msg) {
    (
        "./write".to_string(),
        Msg {
            context: obj(json!({})),
            hop: obj(json!({"route": "in_derive", "notify": "", "caller": "", "op_id": "w-test"})),
            body: obj(json!({"file": file, "version": version, "messages": []})),
        },
    )
}

/// The store bundles `./derive` sent since `since` in phase `phase`, in the
/// order the store ran them: their legs `(id, op)`.
fn bundles_in(sp: &Space, since: usize, phase: &str) -> Vec<Vec<(String, Value)>> {
    sp.sent[since..]
        .iter()
        .filter(|m| {
            m["from"] == json!("./derive")
                && m["to"] == json!("./store")
                && m["header"]["context"]["cur_phase"] == json!(phase)
        })
        .map(|m| {
            m["body"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    (
                        c["id"].as_str().unwrap_or("").to_string(),
                        sj::from_str::<Value>(c["text"].as_str().unwrap_or("null")).unwrap(),
                    )
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_node_bundle_of_an_older_version_spares_the_newer_head() {
    if !shipped() {
        return;
    }
    let mut sp = plain();
    let f = "fh-0000000000d1";
    // OLD on the main line at seq 1000, NEW at seq 2000; NEW is the head.
    let v = sp.seed_text(f, "/d/n.py", &[OLD, NEW], &[]);
    let (old, new) = (v[0].clone(), v[1].clone());
    sp.db
        .execute("UPDATE files SET dir = '/d' WHERE file = ?1", [f])
        .expect("the file's directory");
    // NEW's job is queued first and reads the head first. Right after that
    // read (its parked state is written in the same bundle, without a node
    // run yet) the head shows OLD for OLD's read, and NEW again right after
    // it: both jobs write node rows, and OLD's bundle runs second.
    sp.db
        .execute_batch(&format!(
            r#"
CREATE TRIGGER show_old AFTER UPDATE ON pending
WHEN NEW.phase = 'derive' AND instr(NEW.body, '{new}') > 0 AND instr(NEW.body, '"run"') = 0
BEGIN UPDATE files SET head = '{old}' WHERE file = '{f}'; END;
CREATE TRIGGER show_new AFTER UPDATE ON pending
WHEN NEW.phase = 'derive' AND instr(NEW.body, '{old}') > 0 AND instr(NEW.body, '"run"') = 0
BEGIN UPDATE files SET head = '{new}' WHERE file = '{f}'; END;
"#
        ))
        .expect("the triggers");
    let n = sp.sent.len();
    sp.pump_all(vec![in_derive(f, &new), in_derive(f, &old)]);
    sp.db
        .execute_batch("DROP TRIGGER show_old; DROP TRIGGER show_new;")
        .expect("drop the triggers");

    let nodes = bundles_in(&sp, n, "d-nodes");
    let order: Vec<String> = nodes
        .iter()
        .map(|legs| {
            let (_, run) = legs.iter().find(|(id, _)| id == "nr").expect("the run leg");
            run["row"]["version"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(
        order,
        vec![new.clone(), old.clone()],
        "the older version's node bundle reaches the store after the newer head's"
    );
    // No delete of a node bundle names the file alone.
    for legs in &nodes {
        for (id, a) in legs
            .iter()
            .filter(|(_, a)| a["operation"] == json!("delete"))
        {
            let w = obj(a["where"].clone());
            assert!(
                w.contains_key("file") && (w.contains_key("version") || w.contains_key("seq")),
                "{id} deletes by the file alone: {a}"
            );
        }
    }
    settle(&mut sp);

    assert_eq!(
        sp.rows(&format!("SELECT head FROM files WHERE file = '{f}'")),
        vec![vec![json!(new)]]
    );
    for t in ["nodes", "node_runs"] {
        assert_eq!(
            sp.rows(&format!(
                "SELECT DISTINCT version, seq FROM {t} WHERE file = '{f}'"
            )),
            vec![vec![json!(new), json!(2000)]],
            "{t}: the newer head's rows stay, the older ones are gone"
        );
    }
    let mut anchors: Vec<String> = sp
        .rows(&format!("SELECT anchor FROM nodes WHERE file = '{f}'"))
        .into_iter()
        .map(|r| r[0].as_str().unwrap().to_string())
        .collect();
    anchors.sort();
    assert_eq!(anchors, vec!["def:b", "def:c"]);
    let ev = sp.routed("source_changed");
    assert_eq!(ev.len(), 1, "one event, the head's");
    assert_eq!(
        (ev[0].body["version"].clone(), ev[0].body["nodes"].clone()),
        (json!(&new[..12]), json!(2))
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
}

#[test]
fn a_refused_node_leg_leaves_no_run_and_no_event() {
    if !shipped() {
        return;
    }
    // The reference: the same file where nothing is refused.
    let mut reference = plain();
    let a = ok(reference.request(
        "in_write",
        "create",
        None,
        json!({"path": "/e/m.py", "text": M_PY}),
        json!({}),
    ));
    let rf = a["file"].as_str().unwrap().to_string();
    settle(&mut reference);
    let want = ok(reference.read("outline", &rf, json!({})));
    let want_links = ok(reference.read("links", &rf, json!({})));
    assert_eq!(want["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(want_links["links"].as_array().unwrap().len(), 1);

    // The store refuses the node `def:b`, the second node leg.
    let mut sp = plain();
    sp.db
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON nodes WHEN NEW.anchor = 'def:b' \
             BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;",
        )
        .expect("the trigger");
    let n = sp.out.len();
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/e/m.py", "text": M_PY}),
        json!({}),
    ));
    let f = a["file"].as_str().unwrap().to_string();
    sp.db
        .execute_batch("DROP TRIGGER refuse;")
        .expect("drop the trigger");
    settle(&mut sp);

    assert_eq!(sp.store_errors.len(), 1, "{:?}", sp.store_errors);
    let parts: Vec<&str> = sp.store_errors[0].splitn(3, ": ").collect();
    assert_eq!(
        (parts[0], parts[1]),
        ("derive", "constraint_violation"),
        "{:?}",
        sp.store_errors
    );
    let leg: Value = sj::from_str(parts[2]).expect("the refused leg");
    assert_eq!(leg["tool_call_id"], json!("n-1"), "the node def:b");

    for t in ["node_runs", "nodes", "links"] {
        assert_eq!(
            count(&sp, &format!("SELECT COUNT(*) FROM {t} WHERE file = '{f}'")),
            0,
            "{t}: no half run is left"
        );
    }
    assert!(
        sp.out[n..].iter().all(|m| m.route() != "source_changed"),
        "no event for rows that are not there"
    );
    let undo = bundles_in(&sp, 0, "noop")
        .into_iter()
        .find(|legs| legs.iter().any(|(id, _)| id.starts_with("nz-")))
        .expect("the bundle that takes the rows back");
    let ids: Vec<&str> = undo.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["nz-node_runs", "nz-nodes", "nz-links"],
        "the run row goes first"
    );

    // `outline` and `links` answer in full, computed from the blocks.
    let got = ok(sp.read("outline", &f, json!({})));
    for k in ["version", "parser", "nodes", "next"] {
        assert_eq!(got[k], want[k], "outline {k}");
    }
    assert_eq!(got.get("mark"), want.get("mark"));
    let got = ok(sp.read("links", &f, json!({})));
    assert_eq!(got["links"], want_links["links"]);

    // The file still counts in its directory, with no nodes.
    assert_eq!(
        sp.rows("SELECT files, nodes FROM dirs WHERE path = '/e'"),
        vec![vec![json!(1), json!(0)]]
    );
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
}
