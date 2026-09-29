//! GH #900, the seam of `./write`: every write goes through the file space's
//! own lanes and edges and is judged at the store it lands in.
//!
//! The space runs in one process (`support/file_space_hive.rs`): the shipped
//! scripts of the cells, the shipped edges evaluated by the colony's CEL, the
//! store as an in-memory SQLite behind the store cell's own dispatcher. What
//! is measured is where it lands -- rows in `files`, `versions`, `blocks`,
//! `line`, `ws_files` -- and what reaches the receiving cell: `./derive` and
//! `./ws` run recorder scripts here (overridden `script_inline`) that write
//! every message they get to stderr, so "one `in_derive` per swing" is
//! counted at `./derive`, never read off the emitting script.
//!
//! The hook section (OR-FH-H1) needs the shipped `./guard` (GH #901).
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::{Msg, Space, h4, obj, shipped};
use meclaw_core::serde_json::{self as sj, Value, json};

/// `./derive` as a recorder: every message it gets, one stderr line.
const DERIVE_REC: &str = r#"import sys, json
d = json.load(sys.stdin)
h = d["envelope"]["header"]["hop"]
sys.stderr.write("DERIVE " + json.dumps({"hop": h, "body": d.get("body") or {}}) + "\n")
sys.stdout.write("[]")
"#;

/// `./ws` as a relay: `in_ws` op `relay` sends `body.put` on `in_put` to
/// `./write`; the `put` answer and every `in_recover` are written to stderr.
const WS_RELAY: &str = r#"import sys, json
d = json.load(sys.stdin)
h = d["envelope"]["header"]["hop"]
b = d.get("body") or {}
r = h.get("route")
if r == "in_ws" and b.get("op") == "relay":
    m = dict(b["put"])
    m.setdefault("messages", [])
    m["header"] = {"route": "in_put", "op": m.get("op", ""), "op_id": h.get("op_id", ""),
                   "caller": ""}
    sys.stdout.write(json.dumps([m]))
    sys.exit(0)
sys.stderr.write("WS " + json.dumps({"hop": h, "body": b}) + "\n")
sys.stdout.write("[]")
"#;

/// `./guard` that never answers: the request stays parked at its hook.
const GUARD_SILENT: &str = "import sys\nsys.stdout.write('[]')\n";

/// `./guard` that passes everything unchecked with a note (OR-FH-66).
const GUARD_UNCHECKED: &str = r#"import sys, json
d = json.load(sys.stdin)
h = d["envelope"]["header"]["hop"]
sys.stdout.write(json.dumps([{"header": {"route": "checked", "op_id": h.get("op_id", "")},
                              "hook": "none", "lang": "", "note": "too_deep_to_check",
                              "messages": []}]))
"#;

fn space() -> Space {
    Space::with(
        "/x/files",
        &[
            ("derive", "script_inline", json!(DERIVE_REC)),
            ("ws", "script_inline", json!(WS_RELAY)),
        ],
    )
}

/// The recorded lines `tag` of the space's stderr, parsed.
fn recorded(s: &Space, tag: &str) -> Vec<Value> {
    let needle = format!("{tag} ");
    s.stderr
        .iter()
        .flat_map(|e| e.lines().map(str::to_string).collect::<Vec<_>>())
        .filter_map(|l| {
            l.find(&needle)
                .map(|i| sj::from_str(&l[i + needle.len()..]).expect("a recorded line"))
        })
        .collect()
}

fn write(s: &mut Space, op: &str, file: Option<&str>, args: Value) -> Value {
    s.request("in_write", op, file, args, json!({}))
}

fn write_ws(s: &mut Space, ws: &str, op: &str, file: Option<&str>, args: Value) -> Value {
    s.request("in_write", op, file, args, json!({ "ws": ws }))
}

fn create(s: &mut Space, path: &str, text: &str) -> (String, String) {
    let a = write(s, "create", None, json!({"path": path, "text": text}));
    assert_eq!(a["ok"], json!(true), "create {path}: {a}");
    (
        a["file"].as_str().unwrap().to_string(),
        a["version"].as_str().unwrap().to_string(),
    )
}

fn one(s: &Space, sql: &str) -> Value {
    s.rows(sql)
        .into_iter()
        .next()
        .map(|r| r[0].clone())
        .unwrap_or(Value::Null)
}

fn head(s: &Space, file: &str) -> String {
    one(s, &format!("SELECT head FROM files WHERE file = '{file}'"))
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn text_of(s: &mut Space, file: &str) -> String {
    let r = s.read("read", file, json!({}));
    assert_eq!(r["ok"], json!(true), "read {file}: {r}");
    // `read` answers `text`: its lines in the read form `n:h4|line`, joined
    r["text"]
        .as_str()
        .unwrap_or("")
        .lines()
        .map(|l| format!("{}\n", &l[l.find('|').unwrap() + 1..]))
        .collect()
}

/// The answer form every successful write carries (README § 2.2).
fn assert_form(op: &str, a: &Value, base: &str) {
    assert_eq!(a["ok"], json!(true), "{op}: {a}");
    let v = a["version"].as_str().unwrap_or("");
    assert!(
        v.len() == 12 && v.chars().all(|c| c.is_ascii_hexdigit()),
        "{op}: version is a 12-hex token: {a}"
    );
    assert_eq!(a["base"], json!(base), "{op}: base mirrored: {a}");
    assert!(a["diff"].is_string(), "{op}: a diff: {a}");
    assert!(
        ["ok", "none", "forced"].contains(&a["hook"].as_str().unwrap_or("")),
        "{op}: hook: {a}"
    );
    assert!(
        a["file"].as_str().unwrap_or("").starts_with("fh-"),
        "{op}: file: {a}"
    );
    for k in ["store", "space", "hive", "path_in_store"] {
        assert!(
            a.get(k).is_none(),
            "{op}: no answer names a place ({k}): {a}"
        );
    }
}

/// README § 2.3 Auflage 2 and BUILD-PREAMBLE § 3 at the seam (review W I-5):
/// every store access of the run names its file where the table has one, and
/// every select carries a limit.
fn assert_scoped(s: &Space) {
    let breaches = s.unscoped();
    assert!(breaches.is_empty(), "unscoped store access: {breaches:#?}");
}

const TWENTY: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n\
                      l11\nl12\nl13\nl14\nl15\nl16\nl17\nl18\nl19\nl20\n";

// ------------------------------------------------------------ every op

/// README § 2.9: `create`, then every write op of `in_write` over the main
/// line, each with its base and the answer form of § 2.2 -- the version it
/// made, the base it was made against, the diff between them, the hook.
#[test]
fn every_write_op_answers_with_version_base_diff_and_hook() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (f, mut v) = create(&mut s, "/notes/a.txt", TWENTY);
    assert_eq!(one(&s, "SELECT kind FROM files"), json!("text"));
    let hl2 = h4("l2");
    let hl3 = h4("l3");
    let steps: Vec<(&str, Value, &str)> = vec![
        ("replace", json!({"old": "l1\n", "new": "one\n"}), "+one"),
        (
            "replace_regex",
            json!({"pattern": r"^l1(\d)$", "repl": r"x\1", "expected": "all"}),
            "+x9",
        ),
        (
            "replace_lines",
            json!({"from": 2, "to": 3, "hashes": [hl2, hl3], "new": "two\nthree"}),
            "+three",
        ),
        (
            "insert",
            json!({"line": 1, "where": "after", "text": "ins"}),
            "+ins",
        ),
        ("delete", json!({"from": 2, "to": 2}), "-ins"),
        (
            "patch",
            json!({"diff": "@@ -4,3 +4,3 @@\n l4\n-l5\n+five\n l6\n"}),
            "+five",
        ),
        ("overwrite", json!({"text": "fresh\n"}), "+fresh"),
    ];
    for (op, mut args, needle) in steps {
        args["base"] = json!(v);
        let a = write(&mut s, op, Some(&f), args);
        assert_form(op, &a, &v);
        assert!(
            a["diff"].as_str().unwrap().contains(needle),
            "{op}: the diff shows {needle}: {a}"
        );
        assert_eq!(
            &head(&s, &f)[..12],
            a["version"].as_str().unwrap(),
            "{op} moved the head"
        );
        v = a["version"].as_str().unwrap().to_string();
    }
    let snap = write(&mut s, "snapshot", Some(&f), json!({"name": "s1"}));
    assert_eq!(snap["version"], json!(v), "{snap}");
    let again = write(&mut s, "snapshot", Some(&f), json!({"name": "s1"}));
    assert_eq!(again["error"]["code"], json!("snap_exists"), "{again}");
    let gone = write(&mut s, "remove", Some(&f), json!({"base": v}));
    assert_form("remove", &gone, &v);
    assert_ne!(
        one(&s, "SELECT tomb FROM files"),
        json!(""),
        "remove sets the tomb"
    );
    let after = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v, "old": "a", "new": "b"}),
    );
    assert_eq!(after["error"]["code"], json!("tombstoned"), "{after}");
    let missing = write(&mut s, "replace", Some(&f), json!({"old": "a", "new": "b"}));
    assert_eq!(missing["error"]["code"], json!("tombstoned"), "{missing}");
    assert!(s.store_errors.is_empty(), "{:?}", s.store_errors);
    assert_eq!(
        one(&s, "SELECT COUNT(*) FROM pending"),
        json!(0),
        "nothing stays parked"
    );
    assert_scoped(&s);
}

/// `create`: text and binary, a taken path, a too large content, a path that
/// is not one; a write without its base is refused (§ 2.6).
#[test]
fn create_sorts_text_from_binary_and_keeps_paths_unique() {
    if !shipped() {
        return;
    }
    let mut s = Space::with(
        "/x/files",
        &[
            ("derive", "script_inline", json!(DERIVE_REC)),
            ("write", "max_bytes", json!(1000)),
        ],
    );
    let (f, v) = create(&mut s, "/a.md", "# A\n");
    let bin = file_space_hive::b64(&[0u8, 1, 2, 255, 0, 7]);
    let b = write(
        &mut s,
        "create",
        None,
        json!({"path": "/x.bin", "b64": bin}),
    );
    assert_eq!(b["ok"], json!(true), "{b}");
    let bf = b["file"].as_str().unwrap();
    assert_eq!(
        s.rows(&format!("SELECT kind, mime FROM files WHERE file = '{bf}'"))[0],
        vec![json!("binary"), json!("application/octet-stream")]
    );
    assert_eq!(
        one(&s, &format!("SELECT enc FROM blocks WHERE file = '{bf}'")),
        json!("b64")
    );
    let taken = write(
        &mut s,
        "create",
        None,
        json!({"path": "/a.md", "text": "x"}),
    );
    assert_eq!(taken["error"]["code"], json!("path_taken"), "{taken}");
    assert_eq!(
        taken["error"]["file"],
        json!(f),
        "path_taken names the id: {taken}"
    );
    let big = write(
        &mut s,
        "create",
        None,
        json!({"path": "/big.md", "text": "x".repeat(1001)}),
    );
    assert_eq!(big["error"]["code"], json!("too_large"), "{big}");
    for p in ["a.md", "/a/../b", "/a//b", "/"] {
        let bad = write(&mut s, "create", None, json!({"path": p, "text": "x"}));
        assert_eq!(bad["error"]["code"], json!("bad_path"), "{p}: {bad}");
    }
    let nobase = write(&mut s, "replace", Some(&f), json!({"old": "A", "new": "B"}));
    assert_eq!(nobase["error"]["code"], json!("base_required"), "{nobase}");
    let anchor = write(
        &mut s,
        "replace",
        Some(&format!("{f}#x")),
        json!({"base": v, "old": "A", "new": "B"}),
    );
    assert_eq!(
        anchor["error"]["code"],
        json!("anchor_unsupported"),
        "{anchor}"
    );
    let by_path = write(
        &mut s,
        "replace",
        Some("/a.md"),
        json!({"base": v, "old": "A", "new": "B"}),
    );
    assert_eq!(
        by_path["file"],
        json!(f),
        "a path resolves to the id: {by_path}"
    );
    assert_scoped(&s);
}

// ------------------------------------------------------------ the cascade

/// S-27-1 at the cell: each stage of the cascade once, reported as `stage`.
#[test]
fn every_cascade_stage_is_reached_and_reported() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let text =
        "fn a() {\n    let x = 1;   \n    return x;\n}\nsay \u{201c}hi\u{201d} \u{2014} ok\n";
    let (f, mut v) = create(&mut s, "/c.rs", text);
    let cases = [
        ("fn a() {", "fn b() {", 1),
        ("let x = 1;\n", "let x = 2;\n", 2),
        ("let x = 2;\nreturn x;", "let y = 2;\nreturn y;", 3),
        ("say \"hi\" - ok", "say hello", 4),
    ];
    for (old, new, stage) in cases {
        let a = write(
            &mut s,
            "replace",
            Some(&f),
            json!({"base": v, "old": old, "new": new}),
        );
        assert_eq!(a["stage"], json!(stage), "{old:?}: {a}");
        v = a["version"].as_str().unwrap().to_string();
    }
    assert_eq!(
        text_of(&mut s, &f),
        "fn b() {\n    let y = 2;\n    return y;\n}\nsay hello\n",
        "stage 3 shifted `new` into the file's indentation"
    );
    let amb = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v, "old": " y", "new": " z"}),
    );
    assert_eq!(amb["error"]["code"], json!("ambiguous"), "{amb}");
    assert_eq!(amb["error"]["count"], json!(2), "{amb}");
    let none = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v, "old": "nowhere", "new": "z"}),
    );
    assert_eq!(none["error"]["code"], json!("not_found"), "{none}");
    let all = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v, "old": " y", "new": " z", "expected": "all"}),
    );
    assert_eq!(all["ok"], json!(true), "{all}");
    assert_scoped(&s);
}

// ------------------------------------------------------------ CAS and rebase

/// § 2.6: a write against an old base whose change does not touch what moved
/// is merged onto the head (`rebased`); one that does is refused with the
/// current lines around the overlap and the current token. `replace_lines`
/// with an `h4` that no longer names its line is refused.
#[test]
fn a_moved_base_rebases_apart_and_refuses_an_overlap() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (f, v0) = create(&mut s, "/t.txt", TWENTY);
    let a = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "l3\n", "new": "L3\n"}),
    );
    let v1 = a["version"].as_str().unwrap().to_string();
    let b = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "l15", "new": "L15"}),
    );
    assert_eq!(b["ok"], json!(true), "{b}");
    assert_eq!(b["rebased"], json!(true), "{b}");
    assert_eq!(b["base"], json!(v0), "{b}");
    let t = text_of(&mut s, &f);
    assert!(
        t.contains("L3\n") && t.contains("L15\n"),
        "both changes stand: {t}"
    );
    let v2 = b["version"].as_str().unwrap().to_string();
    let c = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v1, "old": "l15", "new": "M15"}),
    );
    assert_eq!(c["error"]["code"], json!("base_moved"), "{c}");
    assert_eq!(c["error"]["current"], json!(v2), "the new token: {c}");
    let lines: Vec<String> = file_space_hive::strings(&c["error"]["lines"]);
    assert!(
        lines.iter().any(|l| l == &format!("15:{}|L15", h4("L15"))),
        "the current lines around the overlap: {c}"
    );
    assert_eq!(&head(&s, &f)[..12], v2, "a refusal moves nothing");
    let stale = write(
        &mut s,
        "replace_lines",
        Some(&f),
        json!({"base": v2, "from": 3, "to": 3, "hashes": [h4("l3")], "new": "x"}),
    );
    assert_eq!(stale["error"]["code"], json!("stale_lines"), "{stale}");
    assert_eq!(
        file_space_hive::strings(&stale["error"]["lines"]),
        vec![format!("3:{}|L3", h4("L3"))]
    );
    assert_scoped(&s);
}

/// § 2.6: a second writer moves the head between the first one's read and
/// its swing (here: while the first waits at its hook). The compare-and-swap
/// judged by `rows_affected` loses, the write starts again from the new head
/// and lands rebased -- nothing is overwritten.
#[test]
fn a_lost_swing_starts_again_from_the_new_head() {
    if !shipped() {
        return;
    }
    let mut s = Space::with(
        "/x/files",
        &[
            ("derive", "script_inline", json!(DERIVE_REC)),
            ("guard", "script_inline", json!(GUARD_SILENT)),
        ],
    );
    let c = write(
        &mut s,
        "create",
        None,
        json!({"path": "/t.txt", "text": TWENTY, "force": "1"}),
    );
    let (f, v0) = (
        c["file"].as_str().unwrap().to_string(),
        c["version"].as_str().unwrap().to_string(),
    );
    // the first writer: parked at its hook
    s.lane(
        "in_write",
        json!({}),
        json!({"op": "replace", "op_id": "first"}),
        json!({"op": "replace", "file": f, "args": {"base": v0, "old": "l2\n", "new": "L2\n"}}),
    );
    let pid = one(&s, "SELECT op_id FROM pending WHERE cell = 'write'");
    assert!(pid.is_string(), "the first write waits at its hook");
    // the second writer lands (force: no hook)
    let second = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "l18\n", "new": "L18\n", "force": "1"}),
    );
    assert_eq!(second["ok"], json!(true), "{second}");
    let cas_before = s
        .store_ops
        .iter()
        .filter(|(c, o)| c == "write" && o["operation"] == "update" && o["table"] == "files")
        .count();
    let resume = |s: &mut Space, pid: &Value| {
        s.pump(
            "./guard",
            Msg {
                context: Default::default(),
                hop: obj(json!({"route": "checked", "op_id": pid})),
                body: obj(json!({"hook": "ok", "lang": ""})),
            },
        );
    };
    resume(&mut s, &pid);
    // the swing lost; the write read the new head again and waits at its hook
    let pid2 = one(&s, "SELECT op_id FROM pending WHERE cell = 'write'");
    assert_eq!(pid2, pid, "the same request, again at its hook");
    resume(&mut s, &pid2);
    let cas: Vec<_> = s
        .store_ops
        .iter()
        .filter(|(c, o)| c == "write" && o["operation"] == "update" && o["table"] == "files")
        .collect();
    assert_eq!(cas.len() - cas_before, 2, "one lost swing, one that held");
    let ans: Vec<Msg> = s
        .routed("answer")
        .into_iter()
        .filter(|m| m.hop.get("op_id") == Some(&json!("first")))
        .collect();
    assert_eq!(ans.len(), 1, "{:?}", s.stderr);
    assert_eq!(ans[0].body["rebased"], json!(true), "{:?}", ans[0].body);
    let t = text_of(&mut s, &f);
    assert!(
        t.contains("L2\n") && t.contains("L18\n"),
        "neither write is lost: {t}"
    );
    assert_scoped(&s);
}

// ------------------------------------------------------------ blocks, line, derive

/// R-27-6 at the store: after a one-line change of a 3 000-line file the two
/// versions share every block but at most two, counted as rows in `blocks`;
/// `revert` is a new `line` row, never a rewind.
#[test]
fn a_one_line_change_shares_all_blocks_but_two_and_revert_appends() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let big: String = (0..3000)
        .map(|i| format!("{i} {}\n", "abcdefghij".repeat(1 + i % 7)))
        .collect();
    let (f, v0) = create(&mut s, "/big.txt", &big);
    let rows0 = one(
        &s,
        &format!("SELECT COUNT(*) FROM blocks WHERE file = '{f}'"),
    )
    .as_i64()
    .unwrap();
    let a = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "1500 ", "new": "fifteen hundred "}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let rows1 = one(
        &s,
        &format!("SELECT COUNT(*) FROM blocks WHERE file = '{f}'"),
    )
    .as_i64()
    .unwrap();
    assert!(
        rows1 - rows0 <= 2 && rows1 > rows0,
        "{rows0} -> {rows1} block rows"
    );
    let lists: Vec<Vec<String>> = s
        .rows(&format!(
            "SELECT blocks FROM versions WHERE file = '{f}' ORDER BY rowid"
        ))
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap()).unwrap())
        .collect();
    let old: std::collections::BTreeSet<_> = lists[0].iter().collect();
    assert!(lists[1].iter().filter(|h| !old.contains(h)).count() <= 2);
    let v1 = a["version"].as_str().unwrap().to_string();
    let lines0 = one(&s, &format!("SELECT COUNT(*) FROM line WHERE file = '{f}'"));
    let r = write(
        &mut s,
        "revert",
        Some(&f),
        json!({"base": v1, "version": v0}),
    );
    assert_eq!(
        r["version"],
        json!(v0),
        "the old content, the old version: {r}"
    );
    assert_eq!(
        one(&s, &format!("SELECT COUNT(*) FROM line WHERE file = '{f}'")).as_i64(),
        lines0.as_i64().map(|n| n + 1),
        "revert appends"
    );
    assert_eq!(
        s.rows(&format!(
            "SELECT op, note FROM line WHERE file = '{f}' ORDER BY seq DESC LIMIT 1"
        ))[0],
        vec![json!("revert"), json!(v0)]
    );
    assert_scoped(&s);
}

/// § 2.2/2.9: exactly one `in_derive` reaches `./derive` per swing of the main
/// line, with `notify` and `caller` of the write, and none for a working
/// version of a workspace, whose first touch makes the one `ws_files` row.
#[test]
fn one_derive_per_swing_and_none_per_working_edit() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (f, v0) = create(&mut s, "/d.md", "a\nb\nc\n");
    let a = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "b", "new": "B"}),
    );
    let v1 = a["version"].as_str().unwrap().to_string();
    let seen = recorded(&s, "DERIVE");
    assert_eq!(seen.len(), 2, "create + replace: {seen:?}");
    assert!(
        seen.iter()
            .all(|m| m["hop"]["route"] == "in_derive" && m["body"]["file"] == f)
    );
    let full = head(&s, &f);
    assert_eq!(seen[1]["body"]["version"], json!(full));
    // an internal caller: its answer does not leave the space, `./derive` hears it
    s.lane(
        "in_write",
        json!({}),
        json!({"op": "create", "op_id": "n1", "caller": "ingest"}),
        json!({"op": "create", "args": {"path": "/n.md", "text": "x\n", "notify": "1"}}),
    );
    let last = recorded(&s, "DERIVE").pop().unwrap();
    assert_eq!(last["hop"]["notify"], json!("1"), "{last}");
    assert_eq!(last["hop"]["caller"], json!("ingest"), "{last}");
    // a workspace: first touch makes the row, the second reuses it
    s.seed_ws("w1", "feature", "/", i64::MAX / 2);
    let before = recorded(&s, "DERIVE").len();
    let w = write_ws(
        &mut s,
        "feature",
        "replace",
        Some(&f),
        json!({"base": v1, "old": "a", "new": "A"}),
    );
    assert_eq!(w["ok"], json!(true), "{w}");
    assert_eq!(
        w["ws"],
        json!("feature"),
        "the answer names the workspace: {w}"
    );
    assert_eq!(
        one(&s, "SELECT ws FROM ws_files"),
        json!("w1"),
        "the row keys the workspace by its id"
    );
    let by_id = write_ws(
        &mut s,
        "w1",
        "replace",
        Some(&f),
        json!({"base": v1, "old": "a", "new": "A"}),
    );
    assert_eq!(
        by_id["error"]["code"],
        json!("ws_unknown"),
        "hop.ws is the name, not the id (OR-FH-81): {by_id}"
    );
    let w2 = write_ws(
        &mut s,
        "feature",
        "replace",
        Some(&f),
        json!({"base": w["version"], "old": "c", "new": "C"}),
    );
    assert_eq!(w2["ok"], json!(true), "{w2}");
    assert_eq!(
        one(&s, "SELECT COUNT(*) FROM ws_files"),
        json!(1),
        "one row per file"
    );
    assert_eq!(
        one(&s, "SELECT working FROM ws_files")
            .as_str()
            .map(|x| &x[..12]),
        w2["version"].as_str()
    );
    assert_eq!(
        &head(&s, &f)[..12],
        v1.as_str(),
        "the main line did not move"
    );
    assert_eq!(
        recorded(&s, "DERIVE").len(),
        before,
        "no derive for a working version"
    );
    let wc = write_ws(
        &mut s,
        "feature",
        "create",
        None,
        json!({"path": "/w.md", "text": "w\n"}),
    );
    let wf = wc["file"].as_str().unwrap();
    assert_eq!(head(&s, wf), "", "born in the workspace only");
    assert_eq!(
        one(
            &s,
            &format!("SELECT state FROM ws_files WHERE file = '{wf}'")
        ),
        json!("created")
    );
    let wr = write_ws(
        &mut s,
        "feature",
        "remove",
        Some(&f),
        json!({"base": w2["version"]}),
    );
    assert_eq!(wr["ok"], json!(true), "{wr}");
    assert_eq!(
        one(
            &s,
            &format!("SELECT state FROM ws_files WHERE file = '{f}'")
        ),
        json!("removed")
    );
    s.db.execute("UPDATE ws SET state = 'committed'", [])
        .unwrap();
    let closed = write_ws(
        &mut s,
        "feature",
        "replace",
        Some(&f),
        json!({"base": v1, "old": "a", "new": "b"}),
    );
    assert_eq!(closed["error"]["code"], json!("ws_closed"), "{closed}");
    assert_scoped(&s);
}

/// § 2.7: a write that meets a file held by a commit answers `busy`, and when
/// that commit is due (committed, or prepared past its deadline) it asks
/// `./ws` to recover it -- at `./ws`.
#[test]
fn a_locked_file_is_busy_and_a_due_commit_is_handed_to_recovery() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (f, v0) = create(&mut s, "/l.md", "a\n");
    s.seed_commit(
        "c-live",
        "w1",
        "prepared",
        json!([]),
        "2999-01-01T00:00:00.000000Z",
    );
    s.seed_lock(&f, "c-live");
    let a = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "a", "new": "b"}),
    );
    assert_eq!(a["error"]["code"], json!("busy"), "{a}");
    assert!(
        recorded(&s, "WS").is_empty(),
        "a live prepare is not recovered"
    );
    s.seed_commit(
        "c-due",
        "w1",
        "prepared",
        json!([]),
        "2000-01-01T00:00:00.000000Z",
    );
    s.seed_lock(&f, "c-due");
    let b = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "a", "new": "b"}),
    );
    assert_eq!(b["error"]["code"], json!("busy"), "{b}");
    let rec = recorded(&s, "WS");
    assert_eq!(rec.len(), 1, "{rec:?}");
    assert_eq!(rec[0]["hop"]["route"], json!("in_recover"));
    assert_eq!(rec[0]["body"]["commit"], json!("c-due"));
    assert_eq!(&head(&s, &f)[..12], v0, "nothing moved");
    assert_scoped(&s);
}

// ------------------------------------------------------------ in_put

/// The three content requests of `./ws` on `in_put` (GH #902 builds on them):
/// `merge3` makes the merged version (forced when it conflicts), `patch` the
/// patched one, `create` a file born in a workspace -- none moves a head.
#[test]
fn in_put_merges_patches_and_creates_without_moving_a_head() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let (f, v0) = create(&mut s, "/m.txt", TWENTY);
    let full0 = head(&s, &f);
    let a = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": v0, "old": "l2\n", "new": "L2\n"}),
    );
    let full1 = head(&s, &f);
    let b = write(
        &mut s,
        "replace",
        Some(&f),
        json!({"base": a["version"], "old": "l19\n", "new": "L19\n"}),
    );
    assert_eq!(b["ok"], json!(true));
    let full2 = head(&s, &f);
    let put = |s: &mut Space, body: Value| -> Value {
        let before = recorded(s, "WS").len();
        s.lane(
            "in_ws",
            json!({}),
            json!({"op": "relay", "op_id": "p1"}),
            json!({"op": "relay", "put": body}),
        );
        let rec = recorded(s, "WS");
        assert_eq!(rec.len(), before + 1, "one put: {:?}", s.stderr);
        let m = rec.last().unwrap().clone();
        assert_eq!(m["hop"]["route"], json!("put"), "{m}");
        assert_eq!(m["hop"]["op_id"], json!("p1"), "{m}");
        m["body"].clone()
    };
    let clean = put(
        &mut s,
        json!({"op": "merge3", "file": f,
                                   "args": {"base": full0, "ours": full1, "theirs": full2}}),
    );
    assert_eq!(clean["clean"], json!(true), "{clean}");
    let conflict_ours = write_ws_version(&mut s, &f, &full2, "L2\n", "OURS\n");
    let bad = put(
        &mut s,
        json!({"op": "merge3", "file": f,
                                 "args": {"base": full0, "ours": conflict_ours, "theirs": full2}}),
    );
    assert_eq!(bad["clean"], json!(false), "{bad}");
    assert_eq!(bad["conflicts"].as_array().map(Vec::len), Some(1), "{bad}");
    let mv = bad["version"].as_str().unwrap();
    assert_eq!(
        one(
            &s,
            &format!("SELECT force FROM versions WHERE file = '{f}' AND version = '{mv}'")
        ),
        json!("1"),
        "a conflicted merge is a forced version"
    );
    let p = put(
        &mut s,
        json!({"op": "patch", "file": f,
                               "args": {"base": full2, "diff": "@@ -1,2 +1,2 @@\n l1\n-L2\n+P2\n"}}),
    );
    assert!(p["version"].is_string(), "{p}");
    let pf = put(
        &mut s,
        json!({"op": "patch", "file": f,
                                "args": {"base": full2, "diff": "@@ -1,2 +1,2 @@\n nope\n-x\n+y\n"}}),
    );
    assert_eq!(pf["error"]["code"], json!("patch_failed"), "{pf}");
    s.seed_ws("w9", "nine", "/", i64::MAX / 2);
    let c = put(
        &mut s,
        json!({"op": "create", "args": {"path": "/born.md", "text": "b\n", "ws": "nine"}}),
    );
    let cf = c["file"].as_str().unwrap();
    assert_eq!(head(&s, cf), "", "{c}");
    assert_eq!(
        one(&s, &format!("SELECT ws FROM ws_files WHERE file = '{cf}'")),
        json!("w9"),
        "born in the workspace the name names"
    );
    let loose = put(
        &mut s,
        json!({"op": "create", "args": {"path": "/loose.md", "text": "l\n"}}),
    );
    assert_eq!(
        loose["error"]["code"],
        json!("bad_request"),
        "in_put create needs its workspace (review W m-2): {loose}"
    );
    assert_eq!(head(&s, &f), full2, "in_put moves no head");
    assert!(
        recorded(&s, "DERIVE")
            .iter()
            .all(|m| m["body"]["file"] != cf)
    );
    assert_scoped(&s);
}

/// A version of `file` whose content is `base`'s with `old` -> `new`, made
/// through a workspace (no head moves); its full id.
fn write_ws_version(s: &mut Space, file: &str, base_full: &str, old: &str, new: &str) -> String {
    s.seed_ws("wv", "versions", "/", i64::MAX / 2);
    let a = write_ws(
        s,
        "versions",
        "replace",
        Some(file),
        json!({"base": &base_full[..12], "old": old, "new": new}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    one(
        s,
        &format!("SELECT working FROM ws_files WHERE ws = 'wv' AND file = '{file}'"),
    )
    .as_str()
    .unwrap()
    .to_string()
}

// ------------------------------------------------------------ the hook (OR-FH-H1)

/// S-27-2 through `in_write` (the shipped `./guard`, GH #901): broken JSON,
/// TOML and Python are refused with the parser's message, a preview of the
/// new content and the original at the same place, the head unmoved; a file
/// the register does not know passes with `hook: none`; `force` lands the
/// broken content and the version says so.
#[test]
fn a_broken_file_is_refused_unless_forced() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let cases = [
        ("/c.json", "{\"a\": 1}\n", "{\"a\": 1,\n", "json"),
        ("/c.toml", "a = 1\n", "a = = 1\n", "toml"),
        ("/c.py", "x = 1\n", "def (:\n", "python"),
    ];
    for (path, good, broken, lang) in cases {
        let (f, v) = create(&mut s, path, good);
        let a = write(
            &mut s,
            "overwrite",
            Some(&f),
            json!({"base": v, "text": broken}),
        );
        assert_eq!(a["error"]["code"], json!("syntax"), "{path}: {a}");
        assert_eq!(a["error"]["lang"], json!(lang), "{path}: {a}");
        assert!(
            !a["error"]["message"].as_str().unwrap_or("").is_empty(),
            "{a}"
        );
        assert!(
            !file_space_hive::strings(&a["error"]["preview"]).is_empty(),
            "{a}"
        );
        assert!(
            !file_space_hive::strings(&a["error"]["original"]).is_empty(),
            "{a}"
        );
        assert_eq!(&head(&s, &f)[..12], v, "{path}: the head stays");
        let forced = write(
            &mut s,
            "overwrite",
            Some(&f),
            json!({"base": v, "text": broken, "force": "1"}),
        );
        assert_eq!(forced["hook"], json!("forced"), "{forced}");
        let fv = head(&s, &f);
        assert_eq!(
            one(
                &s,
                &format!("SELECT force FROM versions WHERE file = '{f}' AND version = '{fv}'")
            ),
            json!("1")
        );
    }
    let (f, v) = create(&mut s, "/m.rs", "fn main() {}\n");
    let a = write(
        &mut s,
        "overwrite",
        Some(&f),
        json!({"base": v, "text": "fn main( {\n"}),
    );
    assert_eq!(a["hook"], json!("none"), "no parser for .rs: {a}");
    let (j, jv) = create(&mut s, "/ok.json", "[1]\n");
    let ok = write(
        &mut s,
        "overwrite",
        Some(&j),
        json!({"base": jv, "text": "[1, 2]\n"}),
    );
    assert_eq!(ok["hook"], json!("ok"), "{ok}");
    assert_scoped(&s);
}

/// OR-FH-66 (review W I-1): a hook that could not check -- `too_deep_to_check`
/// passes content unchecked, broken or not -- answers `hook: none` with its
/// `note`, and the write says so as `hook_note`.
#[test]
fn an_unchecked_pass_says_why() {
    if !shipped() {
        return;
    }
    let mut s = Space::with(
        "/x/files",
        &[
            ("derive", "script_inline", json!(DERIVE_REC)),
            ("guard", "script_inline", json!(GUARD_UNCHECKED)),
        ],
    );
    let c = write(
        &mut s,
        "create",
        None,
        json!({"path": "/deep.json", "text": "[[1]]\n"}),
    );
    assert_eq!(c["hook"], json!("none"), "{c}");
    assert_eq!(c["hook_note"], json!("too_deep_to_check"), "{c}");
    let o = write(
        &mut s,
        "overwrite",
        c["file"].as_str(),
        json!({"base": c["version"], "text": "[[2]]\n"}),
    );
    assert_eq!(o["hook"], json!("none"), "{o}");
    assert_eq!(o["hook_note"], json!("too_deep_to_check"), "{o}");
    assert_scoped(&s);
}

/// README § 2.9 (docking point of B2) and OR-FH-72 (review W I-6): a
/// `create` with a text view writes its `derived` rows for the new version
/// BEFORE the head names it (the store ops in order), and a later swing
/// keeps the one-liner `./derive` wrote -- only a birth sets it.
#[test]
fn derived_rows_land_before_the_head_and_a_swing_keeps_the_one_liner() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let a = write(
        &mut s,
        "create",
        None,
        json!({"path": "/scan.pdf", "b64": "JVBERi0xLjQKAAAA",
               "derived": [{"part": 1, "text": "page one"}, {"part": 2, "text": "page two"}]}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let f = a["file"].as_str().unwrap().to_string();
    let v = head(&s, &f);
    assert_eq!(
        s.rows(&format!(
            "SELECT version, kind, part, body FROM derived WHERE file = '{f}' ORDER BY part"
        )),
        vec![
            vec![json!(v), json!("text"), json!(1), json!("page one")],
            vec![json!(v), json!("text"), json!(2), json!("page two")],
        ]
    );
    let order: Vec<String> = s
        .store_ops
        .iter()
        .filter(|(c, o)| c == "write" && o["operation"] == "insert")
        .map(|(_, o)| o["table"].as_str().unwrap_or("").to_string())
        .filter(|t| ["derived", "files", "line"].contains(&t.as_str()))
        .collect();
    assert_eq!(
        order,
        vec!["derived", "derived", "files", "line"],
        "the text view lands before the head names its version"
    );

    let (t, tv) = create(&mut s, "/one.md", "first\nsecond\n");
    assert_eq!(
        one(&s, &format!("SELECT oneline FROM files WHERE file = '{t}'")),
        json!("first"),
        "a birth sets the first line as the stand-in"
    );
    s.db.execute(
        &format!("UPDATE files SET oneline = 'the summary' WHERE file = '{t}'"),
        [],
    )
    .unwrap();
    let r = write(
        &mut s,
        "replace",
        Some(&t),
        json!({"base": tv, "old": "first", "new": "a new first line"}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(
        one(&s, &format!("SELECT oneline FROM files WHERE file = '{t}'")),
        json!("the summary"),
        "a swing keeps the one-liner (OR-FH-72)"
    );
    assert_scoped(&s);
}
