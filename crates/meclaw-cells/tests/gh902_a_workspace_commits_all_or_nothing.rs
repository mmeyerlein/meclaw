//! GH #902: a workspace commits all or nothing. The shipped file space in one
//! process (`support/file_space_hive.rs`): the real `./ws`, `./write` and
//! `./read` scripts, the shipped edges evaluated by the colony's CEL, the
//! store behind its own dispatcher. Two workspaces over overlapping files --
//! one commits, the other merges the main line in (cleanly at separate
//! lines, with conflict markers at the same line, then commits after an
//! ordinary `replace` removed them); an untouched file never gets a
//! `ws_files` row (copy-on-write). The two interrupted commits are SEEDED as
//! store rows (OR-FH-V1: no test knob in the cell): prepared past its
//! deadline -- aborted and unlocked by the next commit over the file; past
//! the commit point with heads partly old -- read already shows the new
//! state, and `in_recover` (twice: it must bear duplicates) brings every head
//! forward exactly once. A file born in a discarded workspace frees its path.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const X: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

fn ws(s: &mut Space, op: &str, name: &str, args: Value) -> Value {
    let hop = if name.is_empty() {
        json!({})
    } else {
        json!({"ws": name})
    };
    s.request("in_ws", op, None, args, hop)
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn code(v: &Value) -> String {
    assert_eq!(v["ok"], json!(false), "expected a refusal: {v}");
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

fn one(s: &Space, sql: &str) -> Value {
    let r = s.rows(sql);
    assert_eq!(r.len(), 1, "{sql}: {r:?}");
    r[0][0].clone()
}

fn count(s: &Space, sql: &str) -> i64 {
    one(s, sql).as_i64().unwrap()
}

/// The text of `addr` as `./read` shows it: `text` holds the read form
/// `<n>:<h4>|<line>` joined by `\n` (OR-FH-83), the prefixes go.
fn text(s: &mut Space, addr: &str) -> String {
    let r = ok(s.read("read", addr, json!({})));
    let t = r["text"].as_str().unwrap_or("");
    if t.is_empty() {
        return String::new();
    }
    t.split('\n')
        .map(|l| format!("{}\n", &l[l.find('|').unwrap() + 1..]))
        .collect()
}

fn open(s: &mut Space, name: &str) -> String {
    let r = ok(ws(s, "ws_open", "", json!({"name": name, "root": "/"})));
    r["ws"].as_str().unwrap().to_string()
}

fn patch(s: &mut Space, name: &str, diff: &str) -> Value {
    ws(s, "ws_patch", name, json!({"diff": diff}))
}

/// `./derive` replaced by a recorder: every message that reaches it over an
/// edge is one stderr line `in_derive <file> <version>`. The count is taken
/// at the receiving end of the edge, not from `./derive`'s own work -- that
/// is GH #903's and may still be the placeholder (review V I-3).
const DERIVE_RECORDER: &str = "import sys, json\n\
doc = json.load(sys.stdin)\n\
b = doc.get('body') or {}\n\
h = doc['envelope']['header']['hop']\n\
sys.stderr.write('%s %s %s ws=%s\\n' % (h.get('route'), b.get('file'), b.get('version'), h.get('ws', '')))\n\
sys.stdout.write('[]')\n";

fn space() -> Space {
    Space::with(
        "/x/files",
        &[("derive", "script_inline", json!(DERIVE_RECORDER))],
    )
}

/// The `in_derive` that reached `./derive` for `file`.
fn derived(s: &Space, file: &str) -> usize {
    let probe = format!("derive: in_derive {file} ");
    s.stderr.iter().filter(|e| e.starts_with(&probe)).count()
}

/// The end of every test: no stderr but the recorder's, no failed store op,
/// R-FH-1 Auflage 2 at the store (`ws_files` by `ws` is S' own exception,
/// OR-FH-68 -- no filter here), no parked job left.
fn clean(s: &Space) {
    let noise: Vec<&String> = s
        .stderr
        .iter()
        .filter(|e| !e.starts_with("derive: "))
        .collect();
    assert!(noise.is_empty(), "{noise:?}");
    // GH #947: a file a commit removed reaches `./derive` once more, on
    // `in_dirs` (its directories count it out) -- no lane carries `ws`.
    for e in s.stderr.iter().filter(|e| e.starts_with("derive: ")) {
        assert!(
            e.starts_with("derive: in_derive ") || e.starts_with("derive: in_dirs "),
            "only in_derive and in_dirs reach derive: {e}"
        );
        assert!(
            e.trim_end().ends_with("ws="),
            "in_derive never carries ws (OR-FH-73): {e}"
        );
    }
    assert!(s.store_errors.is_empty(), "{:?}", s.store_errors);
    assert_eq!(
        s.unscoped(),
        Vec::<String>::new(),
        "R-FH-1 Auflage 2 at the store"
    );
    assert_eq!(
        count(s, "SELECT COUNT(*) FROM pending WHERE cell = 'ws'"),
        0
    );
}

fn recover(s: &mut Space, commit: &str) {
    s.pump(
        "./write",
        Msg {
            hop: obj(json!({"route": "in_recover"})),
            body: obj(json!({"commit": commit, "messages": []})),
            ..Default::default()
        },
    );
}

fn head(s: &Space, file: &str) -> String {
    one(s, &format!("SELECT head FROM files WHERE file = '{file}'"))
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_workspace_commits_all_or_nothing() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (x, y, z) = ("fh-00000000000a", "fh-00000000000b", "fh-00000000000c");
    s.seed_text(x, "/docs/x.md", &[X], &[]);
    s.seed_text(y, "/docs/y.md", &["alpha\nbeta\n"], &[]);
    s.seed_text(z, "/docs/z.md", &["untouched\n"], &[]);

    // --- two workspaces over x.md; A also edits y.md --------------------------
    let a = open(&mut s, "A");
    let b = open(&mut s, "B");
    assert_eq!(
        code(&ws(&mut s, "ws_open", "", json!({"name": "A"}))),
        "ws_exists"
    );
    ok(patch(
        &mut s,
        "A",
        "--- a/docs/x.md\n+++ b/docs/x.md\n@@ -1,2 +1,2 @@\n-one\n+ONE\n two\n\
         --- a/docs/y.md\n+++ b/docs/y.md\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+BETA\n",
    ));
    ok(patch(
        &mut s,
        "B",
        "--- a/docs/x.md\n+++ b/docs/x.md\n@@ -9,2 +9,2 @@\n-nine\n+NINE\n ten\n",
    ));
    // the main line does not see a working version
    assert_eq!(text(&mut s, x), X);
    assert_eq!(
        text(&mut s, &format!("{x}@ws:A")).lines().next(),
        Some("ONE")
    );

    let ca = ok(ws(&mut s, "ws_commit", "A", json!({"note": "A"})));
    let c = ca["commit"].as_str().unwrap().to_string();
    assert_eq!(ca["files"].as_array().unwrap().len(), 2, "{ca}");
    assert_eq!(
        one(
            &s,
            &format!("SELECT state FROM commits WHERE \"commit\" = '{c}'")
        ),
        json!("committed")
    );
    assert_eq!(
        count(
            &s,
            &format!("SELECT COUNT(*) FROM line WHERE \"commit\" = '{c}'")
        ),
        2
    );
    assert_eq!(count(&s, "SELECT COUNT(*) FROM files WHERE lock != ''"), 0);
    assert!(text(&mut s, x).starts_with("ONE\n"));
    assert_eq!(text(&mut s, y), "alpha\nBETA\n");
    assert_eq!(derived(&s, x), 1, "one in_derive per committed file");
    assert_eq!(derived(&s, y), 1, "one in_derive per committed file");
    assert_eq!(code(&ws(&mut s, "ws_status", &a, json!({}))), "ws_closed");

    // B is behind on x.md: its commit merges first, cleanly (separate lines)
    let st = ok(ws(&mut s, "ws_status", &b, json!({})));
    assert_eq!(st["files"][0]["behind"], json!(true), "{st}");
    ok(ws(&mut s, "ws_commit", "B", json!({})));
    assert_eq!(
        text(&mut s, x),
        "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nNINE\nten\n"
    );
    // copy-on-write: z.md was never touched, no workspace holds a row of it
    assert_eq!(
        count(
            &s,
            &format!("SELECT COUNT(*) FROM ws_files WHERE file = '{z}'")
        ),
        0
    );

    // --- the same line in two workspaces: a conflict with markers --------------
    open(&mut s, "C1");
    open(&mut s, "C2");
    let d1 = "--- a/docs/y.md\n+++ b/docs/y.md\n@@ -1,2 +1,2 @@\n-alpha\n+ALPHA-1\n BETA\n";
    let d2 = "--- a/docs/y.md\n+++ b/docs/y.md\n@@ -1,2 +1,2 @@\n-alpha\n+ALPHA-2\n BETA\n";
    ok(patch(&mut s, "C1", d1));
    ok(patch(&mut s, "C2", d2));
    ok(ws(&mut s, "ws_commit", "C1", json!({})));
    let head = one(&s, &format!("SELECT head FROM files WHERE file = '{y}'"));
    let e = ws(&mut s, "ws_commit", "C2", json!({}));
    assert_eq!(code(&e), "conflict", "{e}");
    assert_eq!(e["error"]["files"][0]["file"], json!(y), "{e}");
    assert!(
        !e["error"]["files"][0]["lines"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{e}"
    );
    assert_eq!(
        one(&s, &format!("SELECT head FROM files WHERE file = '{y}'")),
        head,
        "a conflicting commit moves no head"
    );
    let marked = text(&mut s, &format!("{y}@ws:C2"));
    assert!(marked.contains("<<<<<<< ours\n") && marked.contains(">>>>>>> theirs\n"));
    assert_eq!(code(&ws(&mut s, "ws_commit", "C2", json!({}))), "conflict");
    // an ordinary replace in the workspace takes the markers out
    let from = marked.find("<<<<<<< ours\n").unwrap();
    let to = marked.find(">>>>>>> theirs\n").unwrap() + ">>>>>>> theirs\n".len();
    ok(s.request(
        "in_write",
        "replace",
        Some(y),
        json!({"old": &marked[from..to], "new": "ALPHA-2\n", "expected": 1}),
        json!({"ws": "C2"}),
    ));
    ok(ws(&mut s, "ws_commit", "C2", json!({})));
    assert_eq!(text(&mut s, y), "ALPHA-2\nBETA\n");

    // --- seeded: prepared, deadline passed -> aborted and unlocked -------------
    let hx = one(&s, &format!("SELECT head FROM files WHERE file = '{x}'"));
    let hx = hx.as_str().unwrap();
    s.seed_commit(
        "c-dead",
        "ws-gone",
        "prepared",
        json!([{"file": x, "from": hx, "to": hx, "kind": "modify"}]),
        "2026-01-01T00:00:00.000000Z",
    );
    s.seed_lock(x, "c-dead");
    open(&mut s, "G");
    ok(patch(
        &mut s,
        "G",
        "--- a/docs/x.md\n+++ b/docs/x.md\n@@ -1,2 +1,2 @@\n-ONE\n+one!\n two\n",
    ));
    ok(ws(&mut s, "ws_commit", "G", json!({})));
    assert_eq!(
        one(&s, "SELECT state FROM commits WHERE \"commit\" = 'c-dead'"),
        json!("aborted")
    );
    assert!(text(&mut s, x).starts_with("one!\n"));

    // --- seeded: committed, two of three heads still old -----------------------
    let files = ["fh-0000000000f1", "fh-0000000000f2", "fh-0000000000f3"];
    let mut plan = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let old = s.seed_text(f, &format!("/p/{i}.md"), &[&format!("old {i}\n")], &[]);
        let new = s.seed_version(f, format!("new {i}\n").as_bytes(), &[], &old[0]);
        plan.push(json!({"file": f, "from": old[0], "to": new, "kind": "modify"}));
    }
    s.seed_commit(
        "c-half",
        "ws-gone",
        "committed",
        json!(plan),
        "2026-01-01T00:00:00.000000Z",
    );
    let first = plan[0]["to"].as_str().unwrap().to_string();
    s.seed_head(files[0], &first, 5000, "commit");
    s.seed_lock(files[1], "c-half");
    s.seed_lock(files[2], "c-half");
    // past the commit point a reader already sees the commit
    assert_eq!(text(&mut s, files[1]), "new 1\n");
    assert_eq!(text(&mut s, files[2]), "new 2\n");
    for _ in 0..2 {
        recover(&mut s, "c-half");
    }
    for (i, f) in files.iter().enumerate() {
        assert_eq!(
            s.rows(&format!("SELECT head, lock FROM files WHERE file = '{f}'")),
            vec![vec![plan[i]["to"].clone(), json!("")]],
            "{f}"
        );
    }
    assert_eq!(
        count(&s, "SELECT COUNT(*) FROM line WHERE \"commit\" = 'c-half'"),
        2,
        "two recoveries, one line row per finished file"
    );
    assert_eq!(derived(&s, files[1]) + derived(&s, files[2]), 2);

    // --- a file born in a discarded workspace frees its path -------------------
    open(&mut s, "D");
    let born = "--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1 @@\n+hello\n";
    ok(patch(&mut s, "D", born));
    assert_eq!(code(&patch(&mut s, "D", born)), "path_taken");
    let tree = ok(ws(&mut s, "ws_tree", "D", json!({})));
    let paths: Vec<&str> = tree["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"/docs/new.md") && paths.contains(&"/docs/z.md"),
        "{tree}"
    );
    ok(ws(&mut s, "ws_discard", "D", json!({})));
    open(&mut s, "E");
    ok(patch(&mut s, "E", born));

    clean(&s);
}

/// Review V C-1 + I-2 (g): a file born in a workspace and a file removed in
/// it commit together. `./write`'s `in_put create` writes the one
/// `ws_files(created)` row (OR-FH-78); a second row planned the file twice
/// and turned every such commit into `busy`.
#[test]
fn a_born_and_a_removed_file_commit_together() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let z = "fh-00000000000c";
    s.seed_text(z, "/docs/z.md", &["gone soon\n"], &[]);
    open(&mut s, "N");
    let r = ok(patch(
        &mut s,
        "N",
        "--- /dev/null\n+++ b/docs/n.md\n@@ -0,0 +1,2 @@\n+born\n+here\n\
         --- a/docs/z.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone soon\n",
    ));
    let n = r["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == json!("/docs/n.md"))
        .expect("the born file in the answer")["file"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        count(
            &s,
            &format!("SELECT COUNT(*) FROM ws_files WHERE file = '{n}'")
        ),
        1,
        "one ws_files row for a born file"
    );
    let st = ok(ws(&mut s, "ws_status", "N", json!({})));
    assert_eq!(st["files"].as_array().unwrap().len(), 2, "{st}");
    let c = ok(ws(&mut s, "ws_commit", "N", json!({"note": "born"})));
    let cid = c["commit"].as_str().unwrap().to_string();
    assert_eq!(text(&mut s, &n), "born\nhere\n");
    assert_eq!(
        s.rows(&format!(
            "SELECT op FROM line WHERE file = '{n}' AND \"commit\" = '{cid}'"
        )),
        vec![vec![json!("commit")]]
    );
    assert_eq!(
        s.rows(&format!(
            "SELECT op FROM line WHERE file = '{z}' AND \"commit\" = '{cid}'"
        )),
        vec![vec![json!("remove")]]
    );
    assert_ne!(
        one(&s, &format!("SELECT tomb FROM files WHERE file = '{z}'")),
        json!("")
    );
    assert_eq!(derived(&s, &n), 1, "in_derive for the born file");
    assert_eq!(derived(&s, z), 0, "no in_derive for a removed file");
    assert_eq!(count(&s, "SELECT COUNT(*) FROM files WHERE lock != ''"), 0);
    clean(&s);
}

/// Review V I-1 / OR-FH-75: step 4 writes the `line` row before it swings
/// the head. Seeded: the process died between the two -- `line` row there,
/// head old, `lock=C`, the workspace still `open`. Recovery (twice: it must
/// bear duplicates) claims the head at the row's own seq, writes no further
/// row, and closes the workspace. A file whose row is missing gets it.
/// OR-FH-85: `line` is append-only -- `m` carries a second, identical row
/// too, as a visitor that lost the claim in a recovery race leaves it; it
/// stands, and `history` shows the commit once.
#[test]
fn a_commit_cut_between_line_and_head_is_finished_once() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (m, k) = ("fh-0000000000e1", "fh-0000000000e2");
    let om = s.seed_text(m, "/m.md", &["old m\n"], &[]);
    let ok_ = s.seed_text(k, "/k.md", &["old k\n"], &[]);
    let nm = s.seed_version(m, b"new m\n", &[], &om[0]);
    let nk = s.seed_version(k, b"new k\n", &[], &ok_[0]);
    s.seed_ws("ws-cut", "CUT", "/", 500);
    s.seed_commit(
        "c-cut",
        "ws-cut",
        "committed",
        json!([{"file": k, "from": ok_[0], "to": nk, "kind": "modify"},
               {"file": m, "from": om[0], "to": nm, "kind": "modify"}]),
        "2026-01-01T00:00:00.000000Z",
    );
    s.seed_line(m, 7777, &nm, &om[0], "commit", "", "ws-cut", "c-cut");
    s.seed_line(m, 7778, &nm, &om[0], "commit", "", "ws-cut", "c-cut");
    s.seed_lock(m, "c-cut");
    s.seed_lock(k, "c-cut");
    assert_eq!(text(&mut s, m), "new m\n", "past the commit point");
    recover(&mut s, "c-cut");
    recover(&mut s, "c-cut");
    assert_eq!(
        s.rows(&format!(
            "SELECT head, head_seq, lock FROM files WHERE file = '{m}'"
        )),
        vec![vec![json!(nm), json!(7777), json!("")]],
        "the head takes the seq of the row written before the cut"
    );
    assert_eq!(
        count(
            &s,
            &format!("SELECT COUNT(*) FROM line WHERE file = '{m}' AND \"commit\" = 'c-cut'")
        ),
        2,
        "append-only: the twin row stands, recovery adds none (OR-FH-85)"
    );
    let hist = ok(s.read("history", m, json!({})));
    let commits: Vec<&Value> = hist["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["commit"] == json!("c-cut"))
        .collect();
    assert_eq!(commits.len(), 1, "history shows a commit once: {hist}");
    let lk = s.rows(&format!(
        "SELECT seq FROM line WHERE file = '{k}' AND \"commit\" = 'c-cut'"
    ));
    assert_eq!(lk.len(), 1, "the missing row is written: {lk:?}");
    assert_eq!(
        s.rows(&format!(
            "SELECT head, head_seq, lock FROM files WHERE file = '{k}'"
        )),
        vec![vec![json!(nk), lk[0][0].clone(), json!("")]]
    );
    assert_eq!(
        one(&s, "SELECT state FROM ws WHERE ws = 'ws-cut'"),
        json!("committed"),
        "recovery closes the workspace too"
    );
    assert_eq!(derived(&s, m) + derived(&s, k), 2);
    clean(&s);
}

/// Review V I-2 (a): `ws_tree` is the workspace's state -- an untouched file
/// the main line moved after `ws_open` shows its version at `base_seq`; after
/// `ws_merge` the basis moves and the tree shows the new head.
#[test]
fn ws_tree_shows_the_basis_and_ws_merge_moves_it() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (x, z) = ("fh-00000000000a", "fh-00000000000c");
    let vx = s.seed_text(x, "/docs/x.md", &[X], &[]);
    let vz = s.seed_text(z, "/docs/z.md", &["untouched\n"], &[]);
    open(&mut s, "T");
    ok(patch(
        &mut s,
        "T",
        "--- a/docs/x.md\n+++ b/docs/x.md\n@@ -1,2 +1,2 @@\n-one\n+ONE\n two\n",
    ));
    let bs0 = one(&s, "SELECT base_seq FROM ws WHERE name = 'T'")
        .as_i64()
        .unwrap();
    // the main line moves both files after the opening
    ok(s.request(
        "in_write",
        "replace",
        Some(z),
        json!({"old": "untouched", "new": "moved", "expected": 1, "base": &vz[0][..12]}),
        json!({}),
    ));
    ok(s.request(
        "in_write",
        "replace",
        Some(x),
        json!({"old": "nine", "new": "NINE", "expected": 1, "base": &vx[0][..12]}),
        json!({}),
    ));
    let version_of = |tree: &Value, f: &str| -> String {
        tree["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["file"] == json!(f))
            .unwrap_or_else(|| panic!("{f} in {tree}"))["version"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let tree = ok(ws(&mut s, "ws_tree", "T", json!({})));
    assert_eq!(
        version_of(&tree, z),
        &vz[0][..12],
        "the basis, not the head"
    );
    let st = ok(ws(&mut s, "ws_status", "T", json!({})));
    assert_eq!(st["files"][0]["behind"], json!(true), "{st}");

    let m = ok(ws(&mut s, "ws_merge", "T", json!({})));
    assert_eq!(m["merged"].as_array().unwrap().len(), 1, "{m}");
    assert_eq!(m["conflicts"], json!([]), "{m}");
    let bs1 = one(&s, "SELECT base_seq FROM ws WHERE name = 'T'")
        .as_i64()
        .unwrap();
    assert!(bs1 > bs0, "ws_merge moves base_seq: {bs0} -> {bs1}");
    let st = ok(ws(&mut s, "ws_status", "T", json!({})));
    assert_eq!(st["files"][0]["behind"], json!(false), "{st}");
    assert_eq!(
        text(&mut s, &format!("{x}@ws:T")),
        "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nNINE\nten\n"
    );
    let tree = ok(ws(&mut s, "ws_tree", "T", json!({})));
    assert_eq!(version_of(&tree, z), &head(&s, z)[..12], "the new basis");
    assert_eq!(
        count(
            &s,
            &format!("SELECT COUNT(*) FROM ws_files WHERE file = '{z}'")
        ),
        0,
        "reading the tree touches nothing"
    );
    clean(&s);
}

/// Review V I-2 (b) + (f): `ws_diff` is base -> working per touched file;
/// a `ws_patch` that fails late (a create `./write` refuses, after the
/// modifies swung) swings every pointer back, and the file it gave birth to
/// on the way leaves as `removed`, its path free again.
#[test]
fn ws_diff_and_a_late_patch_failure_swings_back() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (a, b) = ("fh-0000000000a1", "fh-0000000000b1");
    s.seed_text(a, "/a.md", &["1\n"], &[]);
    s.seed_text(b, "/b.md", &["x\n"], &[]);
    open(&mut s, "P");
    ok(patch(
        &mut s,
        "P",
        "--- a/a.md\n+++ b/a.md\n@@ -1 +1 @@\n-1\n+2\n",
    ));
    ok(patch(
        &mut s,
        "P",
        "--- /dev/null\n+++ b/c.md\n@@ -0,0 +1 @@\n+see\n",
    ));
    let d = ok(ws(&mut s, "ws_diff", "P", json!({})));
    let diff = d["diff"].as_str().unwrap();
    assert!(
        diff.contains("--- a/a.md\n+++ b/a.md\n") && diff.contains("-1\n+2\n"),
        "{diff}"
    );
    assert!(
        diff.contains("--- /dev/null\n+++ b/c.md\n") && diff.contains("+see\n"),
        "{diff}"
    );
    assert_eq!(d["files"].as_array().unwrap().len(), 2, "{d}");

    let before = s.rows("SELECT file, base, working, state FROM ws_files ORDER BY file");
    let bad = patch(
        &mut s,
        "P",
        "--- a/a.md\n+++ b/a.md\n@@ -1 +1 @@\n-2\n+3\n\
         --- a/b.md\n+++ b/b.md\n@@ -1 +1 @@\n-x\n+y\n\
         --- /dev/null\n+++ b/ok.md\n@@ -0,0 +1 @@\n+k\n\
         --- /dev/null\n+++ b/d/../bad.md\n@@ -0,0 +1 @@\n+f\n",
    );
    assert_eq!(code(&bad), "patch_failed", "{bad}");
    let born = s.rows("SELECT file, tomb FROM files WHERE path = '/ok.md'");
    assert_eq!(born.len(), 1, "{born:?}");
    assert_ne!(born[0][1], json!(""), "the grave frees the path");
    let bf = born[0][0].as_str().unwrap().to_string();
    assert_eq!(
        s.rows(&format!("SELECT state FROM ws_files WHERE file = '{bf}'")),
        vec![vec![json!("removed")]]
    );
    let after = s.rows(&format!(
        "SELECT file, base, working, state FROM ws_files WHERE file != '{bf}' ORDER BY file"
    ));
    assert_eq!(after, before, "every pointer swung back");
    ok(patch(
        &mut s,
        "P",
        "--- /dev/null\n+++ b/ok.md\n@@ -0,0 +1 @@\n+k\n",
    ));
    clean(&s);
}

/// Review V I-2 (d) + (e): a commit `prepared` and in time holds its locks.
/// A reader sees the old head (visibility is all or nothing), and a second
/// commit over the same file answers `busy` at once -- no `commits` row of
/// its own, no lock of its own left, the other commit's lock untouched.
#[test]
fn a_prepared_commit_hides_its_heads_and_makes_others_busy() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (p, q) = ("fh-0000000000c1", "fh-0000000000c2");
    let vp = s.seed_text(p, "/p.md", &["old\n"], &[]);
    s.seed_text(q, "/q.md", &["q\n"], &[]);
    let np = s.seed_version(p, b"new\n", &[], &vp[0]);
    open(&mut s, "L");
    ok(patch(
        &mut s,
        "L",
        "--- a/q.md\n+++ b/q.md\n@@ -1 +1 @@\n-q\n+Q\n\
         --- a/p.md\n+++ b/p.md\n@@ -1 +1 @@\n-old\n+mine\n",
    ));
    s.seed_commit(
        "c-live",
        "ws-other",
        "prepared",
        json!([{"file": p, "from": vp[0], "to": np, "kind": "modify"}]),
        "2099-01-01T00:00:00.000000Z",
    );
    s.seed_lock(p, "c-live");
    assert_eq!(text(&mut s, p), "old\n", "prepared is not visible");
    let e = ws(&mut s, "ws_commit", "L", json!({}));
    assert_eq!(code(&e), "busy", "{e}");
    assert_eq!(count(&s, "SELECT COUNT(*) FROM commits"), 1);
    assert_eq!(
        s.rows("SELECT file, lock FROM files WHERE lock != '' ORDER BY file"),
        vec![vec![json!(p), json!("c-live")]]
    );
    assert_eq!(
        one(&s, "SELECT state FROM ws WHERE name = 'L'"),
        json!("open")
    );
    assert_eq!(text(&mut s, q), "q\n");
    clean(&s);
}
