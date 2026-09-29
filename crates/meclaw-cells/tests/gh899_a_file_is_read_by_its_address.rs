//! GH #899: a file of a file space is read by its address, never by its
//! place (R-FH-1, ADR 0047). The shipped `templates/file-space` runs in one
//! process (`support/file_space_hive.rs`): the `read` script under python3,
//! the hive's own edges under the colony's CEL, the store through the store
//! cell's own dispatcher.
//!
//! The three sentences of the acceptance:
//!   (a) every address form -- id, path, `@<hex>`, `@snap:`, `@ws:` -- answers
//!       with the file id and the version token;
//!   (b) the rows of a file moved into the store of a second space answer
//!       every op the same (bar `op_id`), and no answer names a space or a
//!       store;
//!   (c) two files holding the same block hash with different bytes never
//!       read each other's bytes -- every store query names its file.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const A: &str = "fh-aaaaaaaaaaa1";
const B: &str = "fh-bbbbbbbbbbb2";

/// After every test: no query left its file out or its limit off, no store
/// op failed, and `read` cleaned its working rows up.
fn clean(s: &Space) {
    assert_eq!(
        s.unscoped(),
        Vec::<String>::new(),
        "R-FH-1 Auflage 2 at the store"
    );
    assert_eq!(s.store_errors, Vec::<String>::new(), "a store op failed");
    assert_eq!(
        s.rows("SELECT COUNT(*) FROM pending")[0][0],
        json!(0),
        "read leaves no pending row behind"
    );
    let writes: Vec<&(String, Value)> = s
        .store_ops
        .iter()
        .filter(|(_, a)| {
            matches!(
                a["operation"].as_str(),
                Some("insert" | "update" | "delete")
            ) && a["table"] != "pending"
        })
        .collect();
    assert!(writes.is_empty(), "read writes no file: {writes:?}");
}

fn text(v: &Value) -> String {
    v["text"].as_str().unwrap_or("").to_string()
}

fn lines(v: &Value) -> Vec<String> {
    text(v).lines().map(str::to_string).collect()
}

/// `<n>:<h4>|<text>` for lines `from..` of `src`, computed here.
fn formatted(src: &[&str], from: usize) -> Vec<String> {
    src.iter()
        .enumerate()
        .map(|(i, l)| format!("{}:{}|{}", from + i, h4(l), l))
        .collect()
}

#[test]
fn the_space_is_sealed_its_store_cold_and_an_unknown_op_never_reaches_it() {
    if !shipped() {
        return;
    }
    let hive = hive_config();
    assert_eq!(hive["cell"]["type"], "hive");
    assert_eq!(
        hive["params"]["ports"],
        json!([]),
        "sealed: every endpoint is the hive path"
    );
    let store = cell_config("store");
    assert_eq!(store["cell"]["type"], "store");
    assert_eq!(store["params"]["write_surface"], "internal");
    for knob in ["idle_timeout", "lifecycle", "resident"] {
        assert!(
            store["params"].get(knob).is_none() && store["cell"].get(knob).is_none(),
            "the store keeps the default idle timeout (cold until asked): {knob}"
        );
    }
    let lanes: Vec<Value> = hive["params"]["contract"]["accepts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["route"].clone())
        .collect();
    // `in_write` and `in_model` join the contract with the cells behind them
    // (gh173: a declared lane has a door); reading and workspaces stand now.
    for lane in ["in_read", "in_ws"] {
        assert!(
            lanes.contains(&json!(lane)),
            "{lane} is declared: {lanes:?}"
        );
    }

    let mut s = Space::new();
    let got = s.read("frobnicate", A, json!({}));
    assert_eq!(got["ok"], false);
    assert_eq!(got["error"]["code"], "unknown_op");
    assert!(
        s.store_ops.is_empty(),
        "an unknown op wakes no store: {:?}",
        s.store_ops
    );
    let got = s.read("read", &format!("{A}#intro"), json!({}));
    assert_eq!(got["error"]["code"], "anchor_unsupported");
    let got = s.read("read", "not an address", json!({}));
    assert_eq!(got["error"]["code"], "bad_address");
    assert!(s.store_ops.is_empty());
    clean(&s);
}

#[test]
fn every_address_form_answers_with_its_file_and_token() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(
        A,
        "/notes/plan.md",
        &["alpha\nbeta\n", "alpha\nBETA\ngamma\n"],
        &[7],
    );
    s.seed_snap(A, "first", &v[0]);
    // A workspace opened between the two heads (seq 1000 and 2000): untouched,
    // it reads the main line as it stood then.
    s.seed_ws("w1", "draft", "/", 1500);

    let new = vec!["alpha", "BETA", "gamma"];
    let old = vec!["alpha", "beta"];
    let cases: Vec<(String, String, Vec<&str>)> = vec![
        (A.to_string(), v[1].clone(), new.clone()),
        ("/notes/plan.md".to_string(), v[1].clone(), new),
        (format!("{A}@{}", &v[0][..8]), v[0].clone(), old.clone()),
        (
            format!("/notes/plan.md@{}", &v[0][..12]),
            v[0].clone(),
            old.clone(),
        ),
        (format!("{A}@snap:first"), v[0].clone(), old.clone()),
        (format!("{A}@ws:draft"), v[0].clone(), old),
    ];
    for (addr, version, want) in cases {
        let got = s.read("read", &addr, json!({}));
        assert_eq!(got["ok"], true, "{addr}: {got}");
        assert_eq!(got["file"], A, "{addr}: the answer names the id");
        assert_eq!(got["version"], json!(&version[..12]), "{addr}: the token");
        assert_eq!(lines(&got), formatted(&want, 1), "{addr}");
    }
    // `hop.ws` reads in the workspace like `@ws:`; a touched file reads its
    // working version, not the base.
    s.seed_ws_file("w1", A, &v[0], &v[1], "touched");
    let got = s.request(
        "in_read",
        "read",
        Some(A),
        json!({}),
        json!({"ws": "draft"}),
    );
    assert_eq!(got["version"], json!(&v[1][..12]));
    clean(&s);
}

#[test]
fn a_moved_file_answers_every_op_the_same_and_names_no_place() {
    if !shipped() {
        return;
    }
    let mut here = Space::at("/lib/alpha/space");
    let v = here.seed_text(
        A,
        "/docs/guide.md",
        &["one\ntwo\nthree\n", "one\nTWO\nthree\nfour\n"],
        &[5, 3],
    );
    here.seed_snap(A, "rel", &v[0]);
    here.seed_summary(A, &v[1], "oneline", "a guide in four lines");
    let mut there = Space::at("/lib/beta/elsewhere");
    here.copy_file_to(A, &there);

    let v0 = &v[0][..8];
    let snap = format!("{A}@snap:rel");
    let asks: Vec<(&str, &str, Value)> = vec![
        ("info", A, json!({})),
        ("read", A, json!({"from": 2, "to": -1})),
        ("read", "/docs/guide.md", json!({})),
        (
            "search",
            A,
            json!({"pattern": "T.O", "mode": "regex", "context": 1}),
        ),
        ("summary", A, json!({"level": "oneline"})),
        ("history", A, json!({"limit": 5})),
        ("show", A, json!({"version": v0})),
        ("diff", A, json!({"a": v0})),
        ("raw", A, json!({})),
        ("read", snap.as_str(), json!({})),
    ];
    let places = [
        "/lib/alpha/space",
        "/lib/beta/elsewhere",
        "/lib/",
        "/store",
        "cell.db",
    ];
    for (op, addr, args) in &asks {
        let mut a = here.read(op, addr, args.clone());
        let mut b = there.read(op, addr, args.clone());
        assert_eq!(a["ok"], true, "{op} {addr}: {a}");
        for (x, y) in [(&mut a, "here"), (&mut b, "there")] {
            for s in strings(x) {
                for p in places {
                    assert!(
                        !s.contains(p),
                        "{op} {y}: an answer names a place ({p}): {s}"
                    );
                }
            }
            x.as_object_mut().unwrap().remove("op_id");
        }
        assert_eq!(a, b, "{op} {addr}: the same answer after the move");
    }
    let (mut l1, mut l2) = (
        here.read_space("list", json!({"prefix": "/", "depth": 3})),
        there.read_space("list", json!({"prefix": "/", "depth": 3})),
    );
    for l in [&mut l1, &mut l2] {
        l.as_object_mut().unwrap().remove("op_id");
    }
    assert_eq!(l1, l2, "list: the same answer after the move");
    clean(&here);
    clean(&there);
}

#[test]
fn a_block_hash_two_files_share_never_carries_the_other_files_bytes() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let va = s.seed_text(A, "/x.md", &["shared\nmine\n"], &[7]);
    let vb = s.seed_text(B, "/y.md", &["shared\ntheirs\n"], &[7]);
    // The first block of both is `shared\n`: the same hash in two files is two
    // rows. Y's row is made to carry other bytes under the same hash -- a read
    // of X that let `file` out of a query could pick it up.
    let h = sha256_hex(b"shared\n");
    s.db.execute(
        "UPDATE blocks SET body = 'LEAK-OF-Y\n' WHERE file = ?1 AND hash = ?2",
        [B, h.as_str()],
    )
    .unwrap();
    assert_eq!(
        s.rows(&format!("SELECT COUNT(*) FROM blocks WHERE hash = '{h}'"))[0][0],
        json!(2)
    );
    let reads = [
        s.read("read", A, json!({})),
        s.read("search", A, json!({"pattern": "LEAK"})),
        s.read("search", A, json!({"pattern": "shared"})),
        s.read("raw", A, json!({})),
        s.read("show", A, json!({})),
        s.read("diff", A, json!({"a": &va[0][..8]})),
    ];
    for got in &reads {
        assert_eq!(got["ok"], true, "{got}");
        for x in strings(got) {
            assert!(!x.contains("LEAK"), "X read Y's bytes: {got}");
        }
    }
    assert_eq!(reads[1]["total"], 0);
    assert_eq!(reads[2]["total"], 1);
    assert_eq!(reads[3]["b64"], json!(b64(b"shared\nmine\n")));
    // And Y reads its own row.
    let y = s.read("read", B, json!({}));
    assert_eq!(y["version"], json!(&vb[0][..12]));
    assert!(text(&y).contains("LEAK-OF-Y"));
    clean(&s);
}

#[test]
fn a_version_is_put_together_from_its_blocks_and_a_missing_one_is_corrupt() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let body: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let v = s.seed_text(A, "/long.txt", &[body.as_str()], &[13, 1, 40]);
    let got = s.read("raw", A, json!({}));
    assert_eq!(
        got["b64"],
        json!(b64(body.as_bytes())),
        "every cut reads the same bytes"
    );
    assert_eq!(got["version"], json!(&v[0][..12]));

    // A binary file: `b64` blocks, bytes back unchanged, and no text of its own.
    let bin: Vec<u8> = (0u8..=255).chain([0, 0, 7]).collect();
    s.seed_file(B, "/blob.bin", "binary", "application/octet-stream");
    let vb = s.seed_version(B, &bin, &[100], "");
    s.seed_head(B, &vb, 1000, "create");
    assert_eq!(s.read("raw", B, json!({}))["b64"], json!(b64(&bin)));
    assert_eq!(s.read("read", B, json!({}))["error"]["code"], "no_text");

    // One block gone: `corrupt`, never a silent gap.
    s.db.execute(
        "DELETE FROM blocks WHERE rowid = (SELECT MIN(rowid) FROM blocks WHERE file = ?1)",
        [A],
    )
    .unwrap();
    for op in ["read", "raw", "search"] {
        let got = s.read(op, A, json!({"pattern": "x"}));
        assert_eq!(got["error"]["code"], "corrupt", "{op}: {got}");
    }
    clean(&s);
}

#[test]
fn info_read_and_summary_answer_by_their_contract() {
    if !shipped() {
        return;
    }
    let mut s = Space::with("/x/files", &[("read", "max_lines", json!(4))]);
    let body: String = (1..=30).map(|i| format!("row {i}\r\n")).collect();
    let v = s.seed_text(A, "/crlf.txt", &[body.as_str()], &[64]);
    s.seed_snap(A, "keep", &v[0]);
    s.seed_ws("w1", "draft", "/", 5000);
    s.seed_ws_file("w1", A, &v[0], &v[0], "touched");

    let info = s.read("info", "/crlf.txt", json!({}));
    assert_eq!(info["file"], A);
    assert_eq!(info["path"], "/crlf.txt");
    assert_eq!(info["lines"], 30);
    assert_eq!(info["bytes"], json!(body.len()));
    assert_eq!(info["pages"], 0);
    assert_eq!(info["workspaces"], json!(["draft"]));
    assert_eq!(
        info["snapshots"],
        json!([{"name": "keep", "version": &v[0][..12]}])
    );

    // A range longer than `max_lines` stops there and names the rest; the
    // `\r` of a CRLF ending is not part of the line.
    let got = s.read("read", A, json!({"from": 5, "to": 20}));
    assert_eq!(
        (got["from"].clone(), got["to"].clone()),
        (json!(5), json!(8))
    );
    assert_eq!(got["more"], json!({"from": 9, "to": 12}));
    assert_eq!(
        lines(&got),
        formatted(&["row 5", "row 6", "row 7", "row 8"], 5)
    );
    // Negative numbers count from the end.
    let got = s.read("read", A, json!({"from": -2}));
    assert_eq!(lines(&got), formatted(&["row 29", "row 30"], 29));
    assert_eq!(
        s.read("read", A, json!({"from": 40}))["error"]["code"],
        "bad_range"
    );

    // Pages: the text view of a non-text file is its derived pages.
    s.seed_file(B, "/doc.pdf", "binary", "application/pdf");
    let vb = s.seed_version(B, b"%PDF-1.7\x00\x01binary", &[], "");
    s.seed_head(B, &vb, 1000, "create");
    s.seed_derived(B, &vb, 2, "second\n");
    s.seed_derived(B, &vb, 1, "first\npage\n");
    let all = s.read("read", B, json!({}));
    assert_eq!(
        lines(&all),
        formatted(&["--- page 1 ---", "first", "page", "--- page 2 ---"], 1)
    );
    assert_eq!(
        all["more"],
        json!({"from": 5, "to": 5}),
        "max_lines is 4 here"
    );
    let p2 = s.read("read", B, json!({"page": 2}));
    assert_eq!(p2["page"], 2);
    assert_eq!(p2["pages"], 2);
    assert_eq!(lines(&p2), formatted(&["--- page 2 ---", "second"], 4));
    assert_eq!(
        s.read("read", B, json!({"page": 3}))["error"]["code"],
        "page_unknown"
    );
    assert_eq!(s.read("info", B, json!({}))["pages"], 2);

    // Summary: the newest of its level, else pending.
    let got = s.read("summary", A, json!({"level": "short"}));
    assert_eq!(got["pending"], true);
    s.seed_summary(A, &v[0], "short", "thirty rows");
    let got = s.read("summary", A, json!({"level": "short"}));
    assert_eq!(got["text"], "thirty rows");
    assert!(got.get("pending").is_none());
    clean(&s);
}

#[test]
fn search_history_show_and_diff_answer_by_their_contract() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(
        A,
        "/s.md",
        &["a\nb\nc\n", "a\nB\nc\nd\n", "a\nB\nc\nd\ne\n"],
        &[4],
    );
    let got = s.read("search", A, json!({"pattern": "B", "context": 1}));
    assert_eq!(
        got["hits"],
        json!([{"line": 2, "h4": h4("B"), "text": "B",
                "before": [format!("1:{}|a", h4("a"))],
                "after": [format!("3:{}|c", h4("c"))]}])
    );
    let got = s.read(
        "search",
        A,
        json!({"pattern": "^[a-e]$", "mode": "regex", "limit": 2}),
    );
    assert_eq!(got["total"], 4);
    assert_eq!(got["hits"].as_array().unwrap().len(), 2);
    assert_eq!(
        s.read("search", A, json!({"pattern": "(", "mode": "regex"}))["error"]["code"],
        "bad_pattern"
    );

    let got = s.read("history", A, json!({"limit": 2}));
    let e = got["entries"].as_array().unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[0]["version"], json!(&v[2][..12]));
    assert_eq!(e[0]["prev"], json!(&v[1][..12]));
    assert_eq!(e[1]["version"], json!(&v[1][..12]));
    assert_eq!(e[0]["at"], json!(stamp(3000)));

    let got = s.read("show", A, json!({"version": &v[0][..6]}));
    assert_eq!(got["version"], json!(&v[0][..12]));
    assert_eq!(got["lines"], 3);
    assert_eq!(
        got["head"],
        json!(formatted(&["a", "b", "c"], 1).join("\n"))
    );
    assert!(got.get("parent").is_none());

    let got = s.read("diff", A, json!({"a": &v[0][..8], "b": &v[1][..8]}));
    let want = format!(
        "--- {A}@{}\n+++ {A}@{}\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d",
        &v[0][..12],
        &v[1][..12]
    );
    assert_eq!(got["diff"], json!(want));
    assert_eq!(got["b"], json!(&v[1][..12]));
    // `b` defaults to the addressed state.
    assert_eq!(
        s.read("diff", A, json!({"a": &v[0][..8]}))["b"],
        json!(&v[2][..12])
    );
    assert_eq!(s.read("diff", A, json!({}))["error"]["code"], "bad_request");
    clean(&s);
}

#[test]
fn list_find_raw_and_every_resolution_rule() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(A, "/docs/a.md", &["one\n", "two\n"], &[]);
    s.seed_text(B, "/docs/deep/b.md", &["b\n"], &[]);
    s.seed_text("fh-ccccccccccc3", "/top.txt", &["t\n"], &[]);
    s.seed_text("fh-ddddddddddd4", "/gone.txt", &["g\n"], &[]);
    s.seed_tomb("fh-ddddddddddd4");
    // Born in a workspace only: no head, no place outside it.
    s.seed_file("fh-eeeeeeeeeee5", "/docs/new.md", "text", "text/plain");
    let vn = s.seed_version("fh-eeeeeeeeeee5", b"fresh\n", &[], "");
    s.seed_ws("w1", "draft", "/docs/", 1500);
    s.seed_ws_file("w1", "fh-eeeeeeeeeee5", "", &vn, "created");

    let got = s.read_space("list", json!({"prefix": "/", "depth": 1}));
    let paths: Vec<Value> = got["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].clone())
        .collect();
    assert_eq!(paths, vec![json!("/docs/"), json!("/top.txt")]);
    let got = s.read_space("list", json!({"prefix": "/docs", "depth": 2}));
    let paths: Vec<Value> = got["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].clone())
        .collect();
    assert_eq!(paths, vec![json!("/docs/a.md"), json!("/docs/deep/b.md")]);
    let got = s.read_space("find", json!({"glob": "*.md"}));
    assert_eq!(
        got["files"],
        json!([{"file": A, "path": "/docs/a.md"}, {"file": B, "path": "/docs/deep/b.md"}])
    );

    // Tombstone: the path is gone, the id says so, its versions stay readable.
    assert_eq!(
        s.read("read", "/gone.txt", json!({}))["error"]["code"],
        "not_found"
    );
    assert_eq!(
        s.read("read", "fh-ddddddddddd4", json!({}))["error"]["code"],
        "tombstoned"
    );
    assert!(s.read("info", "fh-ddddddddddd4", json!({}))["tomb"].is_string());

    // Born in a workspace: not found outside, found inside.
    assert_eq!(
        s.read("read", "/docs/new.md", json!({}))["error"]["code"],
        "not_found"
    );
    let got = s.read("read", "/docs/new.md@ws:draft", json!({}));
    assert_eq!(got["version"], json!(&vn[..12]));
    assert_eq!(
        s.read("read", &format!("{A}@ws:none"), json!({}))["error"]["code"],
        "ws_unknown"
    );
    // Untouched in the workspace: the main line at `base_seq` (1500 -> v0),
    // not the head (v1).
    assert_eq!(
        s.read("read", &format!("{A}@ws:draft"), json!({}))["version"],
        json!(&v[0][..12])
    );

    // Version prefixes: unknown, ambiguous with candidates, snapshot unknown.
    assert_eq!(
        s.read("read", &format!("{A}@ffff"), json!({}))["error"]["code"],
        "version_unknown"
    );
    let (x, y) = (
        format!("abcd{}", "0".repeat(60)),
        format!("abcd{}", "1".repeat(60)),
    );
    for fake in [&x, &y] {
        s.db.execute(
            "INSERT INTO versions (file, version, blocks, bytes, lines, parent, force, made_by, at) \
             VALUES (?1, ?2, '[]', 0, 0, '', '', 'seed', '')",
            [A, fake.as_str()],
        )
        .unwrap();
    }
    let got = s.read("read", &format!("{A}@abcd"), json!({}));
    assert_eq!(got["error"]["code"], "version_ambiguous");
    assert_eq!(got["error"]["candidates"], json!([&x[..12], &y[..12]]));
    assert_eq!(
        s.read("read", &format!("{A}@snap:none"), json!({}))["error"]["code"],
        "snap_unknown"
    );

    // A prepared commit holds the lock: readers see the head until the
    // commit point, then the plan's target -- all or nothing, and never a write.
    s.seed_commit(
        "c1",
        "w1",
        "prepared",
        json!([{"file": A, "from": &v[1], "to": &v[0]}]),
        "",
    );
    s.seed_lock(A, "c1");
    assert_eq!(s.read("info", A, json!({}))["version"], json!(&v[1][..12]));
    s.db.execute(
        "UPDATE commits SET state = 'committed' WHERE \"commit\" = 'c1'",
        [],
    )
    .unwrap();
    assert_eq!(s.read("info", A, json!({}))["version"], json!(&v[0][..12]));
    assert_eq!(
        s.read("read", A, json!({}))["text"],
        json!(formatted(&["one"], 1).join("\n"))
    );

    assert_eq!(
        s.read("raw", &format!("{A}@{}", &v[1][..8]), json!({}))["b64"],
        json!(b64(b"two\n"))
    );
    clean(&s);
}

/// Review S I-1 / M-4, section 2.7: a commit shows ALL its files at once,
/// also one born in its workspace (`head` '') and one it removes (`to` '').
/// Before the commit point both read as before; past it the plan decides --
/// while the lock still stands, before step 4 is done.
#[test]
fn a_commit_shows_a_born_and_a_removed_file_all_or_nothing() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let born = "fh-eeeeeeeeeee5";
    let va = s.seed_text(A, "/w/a.md", &["old\n"], &[]);
    s.seed_file(born, "/w/new.md", "text", "text/plain");
    let vn = s.seed_version(born, b"fresh\n", &[], "");
    s.seed_ws("w1", "draft", "/w/", 1500);
    s.seed_ws_file("w1", born, "", &vn, "created");
    s.seed_ws_file("w1", A, &va[0], "", "removed");

    // Outside the workspace a file born in it has no version at all (M-4).
    assert_eq!(
        s.read("read", &format!("{born}@{}", &vn[..8]), json!({}))["error"]["code"],
        "not_found"
    );

    s.seed_commit(
        "c1",
        "w1",
        "prepared",
        json!([{"file": born, "from": "", "to": &vn, "kind": "create"},
               {"file": A, "from": &va[0], "to": "", "kind": "remove"}]),
        "",
    );
    s.seed_lock(born, "c1");
    s.seed_lock(A, "c1");
    // Before the commit point: the born file is not there, the removed one is.
    assert_eq!(
        s.read("read", born, json!({}))["error"]["code"],
        "not_found"
    );
    assert_eq!(
        s.read("read", "/w/new.md", json!({}))["error"]["code"],
        "not_found"
    );
    assert_eq!(s.read("info", A, json!({}))["version"], json!(&va[0][..12]));

    s.db.execute(
        "UPDATE commits SET state = 'committed' WHERE \"commit\" = 'c1'",
        [],
    )
    .unwrap();
    // Past it, both at once: the born file reads its plan target, the removed
    // one is gone -- by id and by path.
    let got = s.read("read", "/w/new.md", json!({}));
    assert_eq!(got["file"], json!(born));
    assert_eq!(got["version"], json!(&vn[..12]));
    assert_eq!(got["text"], json!(formatted(&["fresh"], 1).join("\n")));
    assert_eq!(s.read("read", A, json!({}))["error"]["code"], "not_found");
    assert_eq!(
        s.read("info", "/w/a.md", json!({}))["error"]["code"],
        "not_found"
    );
    clean(&s);
}

/// Section 2.2 / review S I-3: an internal caller's answer mirrors `caller`,
/// and the edge `./read -> .` carries only answers with an empty `caller` --
/// the internal caller draws its own edge (B2).
#[test]
fn a_callers_answer_mirrors_it_and_never_leaves_the_space() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    s.seed_text(A, "/c.md", &["x\n"], &[]);
    let before = s.out.len();
    s.lane(
        "in_read",
        json!({}),
        // `tools` draws its own edge since B2 (#908): a caller no cell of the
        // space draws one for stands in for "an internal caller" here.
        json!({"op": "info", "op_id": "q-other", "caller": "other"}),
        json!({"op": "info", "file": A, "args": {}}),
    );
    assert_eq!(
        s.out.len(),
        before,
        "an answer to an internal caller leaves no hive path"
    );
    assert!(
        s.stderr.is_empty(),
        "the cell answered without complaint: {:?}",
        s.stderr
    );
    // The answer went out: `read` dropped its working row.
    clean(&s);
    // From outside (`caller` empty) the same op answers through `.`, and the
    // mirrored `caller` is checked by `request`.
    let got = s.read("info", A, json!({}));
    assert_eq!(got["file"], json!(A));
    clean(&s);
}

/// OR-FH-85: a recovery race can append a commit's `line` row twice (append
/// only, nothing deleted); `history` shows each commit once, at its first row.
#[test]
fn history_shows_a_commit_once() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let v = s.seed_text(A, "/h.md", &["a\n", "b\n"], &[]);
    s.seed_line(A, 3000, &v[0], &v[1], "commit", "", "w1", "c9");
    s.seed_line(A, 3001, &v[0], &v[1], "commit", "", "w1", "c9");
    let got = s.read("history", A, json!({"limit": 10}));
    let e = got["entries"].as_array().unwrap();
    assert_eq!(e.len(), 3, "{e:?}");
    assert_eq!(e[0]["commit"], "c9");
    assert_eq!(e[0]["at"], json!(stamp(3000)), "the smallest seq");
    assert_eq!(e[1]["version"], json!(&v[1][..12]));
    // The limit counts entries, not rows.
    let got = s.read("history", A, json!({"limit": 2}));
    assert_eq!(got["entries"].as_array().unwrap().len(), 2);
    clean(&s);
}

/// Review S I-2: the probe `unscoped()` exempts `files` (and `ws_files` by
/// `ws`) for READS only; an update or delete on them without `file` is a
/// breach of Auflage 2.
#[test]
fn the_store_probe_exempts_reads_only() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    let ops = [
        json!({"operation": "select", "table": "files", "where": {"path": "/a"}, "limit": 1}),
        json!({"operation": "select", "table": "ws_files", "where": {"ws": "w1"}, "limit": 9}),
        json!({"operation": "update", "table": "files", "where": {"lock": "c1"}, "set": {"lock": ""}}),
        json!({"operation": "delete", "table": "ws_files", "where": {"ws": "w1"}}),
        json!({"operation": "select", "table": "blocks", "where": {"hash": "x"}, "limit": 1}),
    ];
    for a in ops {
        s.store_ops.push(("probe".to_string(), a));
    }
    let found = s.unscoped();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found[0].contains("update on files"), "{found:?}");
    assert!(found[1].contains("delete on ws_files"), "{found:?}");
    assert!(found[2].contains("select on blocks"), "{found:?}");
}
