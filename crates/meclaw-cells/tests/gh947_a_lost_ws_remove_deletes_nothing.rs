//! GH #947 (ledger 16) -- a workspace commit that loses the claim of a
//! removed file deletes nothing. Step r2 of `file-space/ws`'s commit swings
//! every file with a compare-and-swap on its lock (`r-cl`, `where {file,
//! lock: <commit>}`); only a visitor whose claim WON drops the buried
//! head's node rows (r3), and only it sends the file's `source_changed` with
//! `tomb: true` and the `in_dirs` that counts the file out of its
//! directories (r4). A visitor that lost -- another one of the same commit
//! was first -- leaves rows, events and counters to the winner.
//!
//! Why: a store bundle is no transaction. Deletes riding beside a lost claim
//! would take the node rows of a file that may still be the living head,
//! and a second tomb event or `in_dirs` would count one removal twice.
//!
//! The lost claim is made deterministic by a SQLite trigger on the
//! harness's store: right after the commit wrote its `line` row for the
//! remove, a rival visitor's claim takes the lock first (what step r2 of the
//! winner writes). The counter-checks: an uncontested remove, and two real
//! visitors of one commit racing (`in_recover` twice, queued together).
//!
//! The shipped space in one process (`support/file_space_hive.rs`): every
//! script and edge the shipped one, the store the store cell's dispatcher,
//! the summarizer the harness's recorder. No embedding endpoint (`embed`
//! off), no provider, no net.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const DOC: &str = "# Doc\n\n## One\nx\n\n## Two\ny\n";
const OTHER: &str = "# E\n\n## A\nz\n";
/// The workspace patch that removes `/k/d.md`.
const REMOVE_DOC: &str =
    "--- a/k/d.md\n+++ /dev/null\n@@ -1,7 +0,0 @@\n-# Doc\n-\n-## One\n-x\n-\n-## Two\n-y\n";

type Counters = (i64, i64, i64);

fn plain() -> Space {
    Space::with("/x/files", &[("derive", "embed", json!("0"))])
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn ws(sp: &mut Space, op: &str, name: &str, args: Value) -> Value {
    let hop = if name.is_empty() {
        json!({})
    } else {
        json!({"ws": name})
    };
    ok(sp.request("in_ws", op, None, args, hop))
}

/// `create` and its summary: the file with its node rows and its counts.
fn create(sp: &mut Space, path: &str, body: &str) -> String {
    let a = ok(sp.request(
        "in_write",
        "create",
        None,
        json!({"path": path, "text": body}),
        json!({}),
    ));
    while sp.llm.front().is_some_and(|(c, _)| c == "summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
    a["file"].as_str().unwrap().to_string()
}

fn one(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

/// `(files, bytes, nodes)` of a directory row.
fn counters(sp: &Space, path: &str) -> Counters {
    let r = sp.rows(&format!(
        "SELECT files, bytes, nodes FROM dirs WHERE path = '{path}'"
    ));
    assert_eq!(r.len(), 1, "a row for {path}");
    (
        r[0][0].as_i64().unwrap(),
        r[0][1].as_i64().unwrap(),
        r[0][2].as_i64().unwrap(),
    )
}

/// What `file` itself adds to every directory above it.
fn own(sp: &Space, file: &str) -> Counters {
    (
        1,
        one(
            sp,
            &format!("SELECT bytes FROM files WHERE file = '{file}'"),
        ),
        one(
            sp,
            &format!("SELECT nodes FROM node_runs WHERE file = '{file}'"),
        ),
    )
}

fn minus(a: Counters, b: Counters) -> Counters {
    (a.0 - b.0, a.1 - b.1, a.2 - b.2)
}

/// Every node row of `file`, whole.
fn node_rows(sp: &Space, file: &str) -> Vec<Vec<Vec<Value>>> {
    ["nodes", "links", "node_runs"]
        .iter()
        .map(|t| {
            sp.rows(&format!(
                "SELECT * FROM {t} WHERE file = '{file}' ORDER BY 1, 2, 3"
            ))
        })
        .collect()
}

/// The ids of the store legs sent since `since` that start with `prefix`.
fn legs_sent(sp: &Space, since: usize, prefix: &str) -> Vec<String> {
    sp.sent[since..]
        .iter()
        .filter(|m| m["to"] == json!("./store"))
        .flat_map(|m| {
            m["body"]["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|c| c["id"].as_str().map(str::to_string))
        .filter(|id| id.starts_with(prefix))
        .collect()
}

/// The store's answers to the claim `r-cl:<file>` since `since`.
fn claims(sp: &Space, since: usize, file: &str) -> Vec<Value> {
    let id = json!(format!("r-cl:{file}"));
    sp.sent[since..]
        .iter()
        .filter(|m| m["from"] == json!("./store") && m["to"] == json!("./ws"))
        .flat_map(|m| m["body"]["results"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["tool_call_id"] == id)
        .collect()
}

/// The tomb events and the `in_dirs` of `file` since the marks.
fn tombs(sp: &Space, out: usize, sent: usize, file: &str) -> (Vec<Value>, usize) {
    let events: Vec<Value> = sp.out[out..]
        .iter()
        .filter(|m| m.route() == "source_changed")
        .map(|m| {
            let mut b = m.body.clone();
            b.remove("messages");
            Value::Object(b)
        })
        .collect();
    let dirs = sp.sent[sent..]
        .iter()
        .filter(|m| m["route"] == json!("in_dirs"))
        .inspect(|m| assert_eq!(m["body"]["file"], json!(file), "{m}"))
        .count();
    (events, dirs)
}

/// The store's refusals, less the unique keys the sync job and the path
/// claim are built to meet (GH #947); no stderr of a broken job; no parked
/// job; R-FH-1 at the store.
fn clean(sp: &Space) {
    let refused: Vec<&String> = sp
        .store_errors
        .iter()
        .filter(|e| {
            let Some((_, leg)) = e.split_once(": unique_violation: ") else {
                return true;
            };
            let leg: Value = sj::from_str(leg).unwrap_or(Value::Null);
            let id = leg["tool_call_id"].as_str().unwrap_or("");
            !(id.starts_with("s-claim") || id.starts_with("di-") || id == "claim")
        })
        .collect();
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert_eq!(
        one(
            sp,
            "SELECT COUNT(*) FROM pending WHERE cell = 'ws' OR op_id LIKE 's-%' \
             OR op_id LIKE 'd-%'"
        ),
        0,
        "no job left parked"
    );
    for e in &sp.stderr {
        for bad in ["Traceback", "dropped", "lost its", "given up", "drift"] {
            assert!(!e.contains(bad), "{e}");
        }
    }
}

#[test]
fn a_remove_whose_claim_is_lost_deletes_nothing() {
    if !shipped() {
        return;
    }
    let mut sp = plain();
    let f = create(&mut sp, "/k/d.md", DOC);
    create(&mut sp, "/k/e.md", OTHER);
    let rows_before = node_rows(&sp, &f);
    let dirs_before = (counters(&sp, "/k"), counters(&sp, "/"));
    ws(&mut sp, "ws_open", "", json!({"name": "R", "root": "/"}));
    ws(&mut sp, "ws_patch", "R", json!({"diff": REMOVE_DOC}));
    // The rival: right after this commit's `line` row for the remove, a
    // visitor of the same commit wins the claim (tomb, lock released, the
    // row's seq) -- this one's compare-and-swap finds no lock left.
    sp.db
        .execute_batch(&format!(
            r#"
CREATE TRIGGER rival AFTER INSERT ON line WHEN NEW.op = 'remove' AND NEW.file = '{f}'
BEGIN UPDATE files SET tomb = '2026-10-02T00:00:00.000000Z', lock = '', head_seq = NEW.seq
      WHERE file = NEW.file AND lock = NEW."commit"; END;
"#
        ))
        .expect("the rival");
    let (n_out, n_sent) = (sp.out.len(), sp.sent.len());
    let c = ws(&mut sp, "ws_commit", "R", json!({}));
    sp.db
        .execute_batch("DROP TRIGGER rival;")
        .expect("drop the rival");
    assert_eq!(c["files"], json!([{"file": f, "removed": true}]), "{c}");

    let cl = claims(&sp, n_sent, &f);
    assert_eq!(cl.len(), 1, "the commit tried its claim once: {cl:?}");
    assert_eq!(cl[0]["rows_affected"], json!(0), "and lost it");
    assert_eq!(node_rows(&sp, &f), rows_before, "no node row deleted");
    assert_eq!(legs_sent(&sp, n_sent, "r-nx"), Vec::<String>::new());
    let (events, dirs) = tombs(&sp, n_out, n_sent, &f);
    assert_eq!(events, Vec::<Value>::new(), "no tomb event");
    assert_eq!(dirs, 0, "no in_dirs");
    assert_eq!(
        (counters(&sp, "/k"), counters(&sp, "/")),
        dirs_before,
        "the directories still count the file"
    );
    assert_eq!(
        one(
            &sp,
            &format!("SELECT COUNT(*) FROM contrib WHERE file = '{f}'")
        ),
        1
    );
    clean(&sp);
}

#[test]
fn a_remove_that_wins_counts_the_file_out_once() {
    if !shipped() {
        return;
    }
    let mut sp = plain();
    let f = create(&mut sp, "/k/d.md", DOC);
    create(&mut sp, "/k/e.md", OTHER);
    let mine = own(&sp, &f);
    assert!(mine.1 > 0 && mine.2 > 0, "{mine:?}");
    let before = (counters(&sp, "/k"), counters(&sp, "/"));
    ws(&mut sp, "ws_open", "", json!({"name": "R", "root": "/"}));
    ws(&mut sp, "ws_patch", "R", json!({"diff": REMOVE_DOC}));
    let (n_out, n_sent) = (sp.out.len(), sp.sent.len());
    let c = ws(&mut sp, "ws_commit", "R", json!({}));
    assert_eq!(c["files"], json!([{"file": f, "removed": true}]), "{c}");

    let cl = claims(&sp, n_sent, &f);
    assert_eq!(cl.len(), 1);
    assert_eq!(cl[0]["rows_affected"], json!(1), "the claim won");
    let (events, dirs) = tombs(&sp, n_out, n_sent, &f);
    assert_eq!(
        events,
        vec![json!({"source": f, "path": "/k/d.md", "tomb": true})],
        "exactly one tomb event"
    );
    assert_eq!(dirs, 1, "exactly one in_dirs");
    for t in ["nodes", "links", "node_runs"] {
        assert_eq!(
            one(&sp, &format!("SELECT COUNT(*) FROM {t} WHERE file = '{f}'")),
            0,
            "{t}"
        );
    }
    assert_eq!(
        (counters(&sp, "/k"), counters(&sp, "/")),
        (minus(before.0, mine), minus(before.1, mine)),
        "every ancestor counts the file out, once"
    );
    assert_eq!(
        one(
            &sp,
            &format!("SELECT COUNT(*) FROM contrib WHERE file = '{f}'")
        ),
        0
    );
    clean(&sp);
}

#[test]
fn two_visitors_of_one_commit_remove_once() {
    if !shipped() {
        return;
    }
    let mut sp = plain();
    let f = create(&mut sp, "/k/d.md", DOC);
    let mine = own(&sp, &f);
    let before = (counters(&sp, "/k"), counters(&sp, "/"));
    let head = sp.rows(&format!(
        "SELECT head, head_seq FROM files WHERE file = '{f}'"
    ));
    let (v, seq) = (
        head[0][0].as_str().unwrap().to_string(),
        head[0][1].as_i64().unwrap(),
    );
    // A commit past its commit point that removes the file, its lock still
    // held: both visitors read the lock before either claims.
    sp.seed_ws("ws-r", "R", "/", seq);
    sp.seed_commit(
        "c-race",
        "ws-r",
        "committed",
        json!([{"file": f, "from": v, "to": "", "kind": "remove"}]),
        "2026-01-01T00:00:00.000000Z",
    );
    sp.seed_lock(&f, "c-race");
    let visit = || {
        (
            "./write".to_string(),
            Msg {
                hop: obj(json!({"route": "in_recover"})),
                body: obj(json!({"commit": "c-race", "messages": []})),
                ..Default::default()
            },
        )
    };
    let (n_out, n_sent) = (sp.out.len(), sp.sent.len());
    sp.pump_all(vec![visit(), visit()]);

    let mut won: Vec<Value> = claims(&sp, n_sent, &f)
        .iter()
        .map(|r| r["rows_affected"].clone())
        .collect();
    won.sort_by_key(|v| v.as_i64());
    assert_eq!(won, vec![json!(0), json!(1)], "two claims, one wins");
    assert_eq!(
        legs_sent(&sp, n_sent, "r-nx").len(),
        3,
        "the node rows are dropped by the winner alone"
    );
    let (events, dirs) = tombs(&sp, n_out, n_sent, &f);
    assert_eq!(
        events,
        vec![json!({"source": f, "path": "/k/d.md", "tomb": true})]
    );
    assert_eq!(dirs, 1);
    assert_eq!(
        (counters(&sp, "/k"), counters(&sp, "/")),
        (minus(before.0, mine), minus(before.1, mine))
    );
    clean(&sp);
}
