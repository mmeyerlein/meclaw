//! GH #918 -- two writers that take one path never both land. The lookup of
//! a path and the birth of its file are two bundles of the store, and
//! `files.path` can hold no unique index (a removed file keeps its row and its
//! path), so two `create`s that both read "free" both landed. Every
//! path-taking write (`create`, the target of `move` and `copy`,
//! `create_dir`) therefore inserts a `claims` row, unique on `path`, in the
//! FIRST bundle of the request, before its lookups: of two writers one row
//! lands and the other's leg answers `unique_violation`, which `./write`
//! answers `path_taken`. The claim goes at every end of the request; one
//! whose writer died before that is dropped by the next claimer of the path
//! once it is older than `claim_ttl_s`, and a living one holds.
//!
//! The shipped space in one process (`support/file_space_hive.rs`) with the
//! store's declared indexes applied as the store factory applies them (schema,
//! then indexes): the claim is a compare-and-set only through its unique index.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The unique indexes of the space whose `unique_violation` is an answer, not
/// a fault: a claim lost to a living one, a directory row or a contribution
/// another writer made first. The harness records every leg's code.
const ANSWERS: [&str; 3] = ["claims_path", "dirs_path", "contrib_file"];

/// The shipped space without embeddings, plus `over`, its store carrying the
/// declared indexes (GH #915: the store factory applies `params.indexes` right
/// after the schema).
fn space_with(over: &[(&str, &str, Value)]) -> Space {
    let mut all: Vec<(&str, &str, Value)> = vec![("derive", "embed", json!("0"))];
    all.extend(over.iter().cloned());
    let sp = Space::with("/x/files", &all);
    let store = meclaw_cells::store::StoreParams::parse(&cell_config("store")["params"])
        .expect("the store params parse");
    meclaw_cells::store::ddl::apply_index_ddl(&sp.db, &store.indexes).expect("index ddl");
    sp
}

fn space() -> Space {
    space_with(&[])
}

fn faults(sp: &Space) -> Vec<String> {
    sp.store_errors
        .iter()
        .filter(|e| !(e.contains("unique_violation") && ANSWERS.iter().any(|i| e.contains(i))))
        .cloned()
        .collect()
}

/// How many legs hit the unique index `index`.
fn uniques(sp: &Space, index: &str) -> usize {
    sp.store_errors
        .iter()
        .filter(|e| e.contains("unique_violation") && e.contains(index))
        .count()
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn refused(v: Value, code: &str) -> Value {
    assert_eq!(v["ok"], json!(false), "expected {code}: {v}");
    assert_eq!(v["error"]["code"], json!(code), "{v}");
    v
}

fn write(sp: &mut Space, op: &str, file: Option<&str>, args: Value) -> Value {
    sp.request("in_write", op, file, args, json!({}))
}

/// Answer every summary the recorder holds.
fn settle(sp: &mut Space) {
    while sp.llm.front().map(|(c, _)| c.as_str()) == Some("summarizer") {
        sp.llm_answer("One line of the file.\n\nA short paragraph.", "stop");
    }
    assert!(sp.llm.is_empty(), "only the summarizer is asked");
}

fn create(sp: &mut Space, path: &str, text: &str) -> String {
    let a = ok(write(
        sp,
        "create",
        None,
        json!({"path": path, "text": text}),
    ));
    settle(sp);
    a["file"].as_str().expect("a file id").to_string()
}

/// A request on `in_write` under `op_id`, for [`Space::pump_all`].
fn on_write(op_id: &str, op: &str, file: Option<&str>, args: Value) -> (String, Msg) {
    let mut body = json!({"op": op, "args": args});
    if let Some(f) = file {
        body["file"] = json!(f);
    }
    let msg = Space::on_lane(
        "in_write",
        json!({}),
        json!({"op": op, "op_id": op_id}),
        body,
    );
    (".".to_string(), msg)
}

/// The one answer of each request, `messages` removed.
fn answers(sp: &Space, ids: &[&str]) -> Vec<Value> {
    ids.iter()
        .map(|id| {
            let mine: Vec<&Msg> = sp
                .out
                .iter()
                .filter(|m| m.route() == "answer" && m.hop.get("op_id") == Some(&json!(id)))
                .collect();
            assert_eq!(mine.len(), 1, "{id}: exactly one answer: {mine:?}");
            let mut b = mine[0].body.clone();
            b.remove("messages");
            Value::Object(b)
        })
        .collect()
}

fn claims(sp: &Space) -> Vec<Vec<Value>> {
    sp.rows("SELECT path, op_id FROM claims ORDER BY path")
}

/// Every route `./write`, `./derive` and `./ws` sent is one its contract
/// declares; no store fault, no unscoped store op, no traceback.
fn assert_clean(sp: &Space) {
    for cell in ["write", "derive", "ws"] {
        let declared: Vec<String> =
            strings(&cell_config(cell)["contract"]["emits"]["hop"]["route"]["values"]);
        let from = format!("./{cell}");
        for m in sp.sent.iter().filter(|m| m["from"] == json!(from)) {
            let route = m["route"].as_str().unwrap_or("");
            assert!(
                declared.iter().any(|d| d == route),
                "{cell} sent {route}, its contract does not declare it"
            );
        }
    }
    assert_eq!(faults(sp), Vec::<String>::new(), "store faults");
    assert_eq!(sp.unscoped(), Vec::<String>::new(), "R-FH-1 at the store");
    assert!(
        !sp.stderr.iter().any(|e| e.contains("Traceback")),
        "{:?}",
        sp.stderr
    );
}

#[test]
fn two_creates_on_one_path_land_one_file() {
    if !shipped() {
        return;
    }
    let mut sp = space();
    // Both requests queued at once: both claim bundles reach the store before
    // either birth does.
    sp.pump_all(vec![
        on_write(
            "race-a",
            "create",
            None,
            json!({"path": "/r/x.md", "text": "# A\n"}),
        ),
        on_write(
            "race-b",
            "create",
            None,
            json!({"path": "/r/x.md", "text": "# B\n"}),
        ),
    ]);
    let got = answers(&sp, &["race-a", "race-b"]);
    let won: Vec<&Value> = got.iter().filter(|a| a["ok"] == json!(true)).collect();
    assert_eq!(won.len(), 1, "exactly one create lands: {got:?}");
    let lost = if got[0]["ok"] == json!(true) {
        got[1].clone()
    } else {
        got[0].clone()
    };
    refused(lost, "path_taken");
    assert_eq!(
        sp.rows("SELECT file, dir FROM files WHERE path = '/r/x.md'"),
        vec![vec![won[0]["file"].clone(), json!("/r")]],
        "one file row at the path"
    );
    // The receipt that both lookups ran before a birth: two claim inserts,
    // both ahead of the one `files` insert -- the unique row decided.
    let ops: Vec<(String, String)> = sp
        .store_ops
        .iter()
        .filter(|(s, _)| s == "write")
        .map(|(_, a)| {
            (
                a["operation"].as_str().unwrap_or("").to_string(),
                a["table"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let at = |table: &str| -> Vec<usize> {
        ops.iter()
            .enumerate()
            .filter(|(_, (op, t))| op == "insert" && t == table)
            .map(|(i, _)| i)
            .collect()
    };
    let (claimed, born) = (at("claims"), at("files"));
    assert_eq!((claimed.len(), born.len()), (2, 1), "{ops:?}");
    assert!(
        claimed[1] < born[0],
        "both claims before the birth: {ops:?}"
    );
    assert_eq!(uniques(&sp, "claims_path"), 1, "{:?}", sp.store_errors);
    settle(&mut sp);
    assert_eq!(claims(&sp), Vec::<Vec<Value>>::new(), "every claim dropped");
    assert_clean(&sp);
}

#[test]
fn a_create_and_a_relocation_never_share_their_target() {
    if !shipped() {
        return;
    }
    for op in ["move", "copy"] {
        for create_first in [true, false] {
            let mut sp = space();
            let src = create(&mut sp, "/s.txt", "hello\n");
            let c = on_write(
                "c",
                "create",
                None,
                json!({"path": "/t.txt", "text": "other\n"}),
            );
            let r = on_write("r", op, Some(&src), json!({"to": "/t.txt"}));
            sp.pump_all(if create_first { vec![c, r] } else { vec![r, c] });
            let got = answers(&sp, &["c", "r"]);
            let won = got.iter().filter(|a| a["ok"] == json!(true)).count();
            assert_eq!(won, 1, "{op}, create first {create_first}: {got:?}");
            let lost = if got[0]["ok"] == json!(true) {
                got[1].clone()
            } else {
                got[0].clone()
            };
            refused(lost, "path_taken");
            assert_eq!(
                sp.rows("SELECT file FROM files WHERE path = '/t.txt' AND tomb = ''")
                    .len(),
                1,
                "{op}, create first {create_first}: one living file at the target"
            );
            settle(&mut sp);
            assert_eq!(
                claims(&sp),
                Vec::<Vec<Value>>::new(),
                "{op}: every claim dropped"
            );
            assert_clean(&sp);
        }
    }
}

#[test]
fn an_orphaned_claim_expires_and_a_living_one_holds() {
    if !shipped() {
        return;
    }
    let cfg = cell_config("write");
    assert_eq!(cfg["params"]["claim_ttl_s"], json!(120));
    assert_eq!(
        cfg["contract"]["settings"]["claim_ttl_s"]["default"],
        json!(120)
    );
    // A day of `claim_ttl_s`: the living claim below stays living however long
    // the requests take, the orphan from long ago has expired all the same.
    let mut sp = space_with(&[("write", "claim_ttl_s", json!(86400))]);

    // A claim whose writer died long ago: the next claimer drops it.
    sp.db
        .execute(
            "INSERT INTO claims (path, op_id, at) \
             VALUES ('/o.txt', 'w-dead', '2000-01-01T00:00:00.000000Z')",
            [],
        )
        .expect("seed an orphaned claim");
    let f = create(&mut sp, "/o.txt", "x\n");
    assert_eq!(
        sp.rows(&format!("SELECT path FROM files WHERE file = '{f}'")),
        vec![vec![json!("/o.txt")]]
    );
    assert_eq!(
        claims(&sp),
        Vec::<Vec<Value>>::new(),
        "the orphan went, and the birth's own claim with it"
    );

    // A claim of a writer still at work: every path-taking op refuses.
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string();
    sp.db
        .execute(
            "INSERT INTO claims (path, op_id, at) VALUES ('/p.txt', 'w-live', ?1)",
            [&now],
        )
        .expect("seed a living claim");
    refused(
        write(
            &mut sp,
            "create",
            None,
            json!({"path": "/p.txt", "text": "y\n"}),
        ),
        "path_taken",
    );
    refused(
        write(&mut sp, "move", Some(&f), json!({"to": "/p.txt"})),
        "path_taken",
    );
    refused(
        write(&mut sp, "copy", Some(&f), json!({"to": "/p.txt"})),
        "path_taken",
    );
    refused(
        write(&mut sp, "create_dir", None, json!({"path": "/p.txt"})),
        "path_taken",
    );
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM files WHERE path = '/p.txt'"),
        vec![vec![json!(0)]]
    );
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM dirs WHERE path = '/p.txt'"),
        vec![vec![json!(0)]]
    );
    assert_eq!(
        sp.rows(&format!("SELECT path FROM files WHERE file = '{f}'")),
        vec![vec![json!("/o.txt")]],
        "the refused move moved nothing"
    );
    assert_eq!(
        claims(&sp),
        vec![vec![json!("/p.txt"), json!("w-live")]],
        "the living foreign claim stays; the refused requests left none"
    );
    assert_eq!(uniques(&sp, "claims_path"), 4, "{:?}", sp.store_errors);
    settle(&mut sp);
    assert_clean(&sp);
}
