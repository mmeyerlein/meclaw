//! GH #905: the pure half of the projection (`templates/projection`), held to
//! tables, and the doors that lead a request into it.
//!
//! `mat` decides what a materialisation writes, keeps and refuses to touch
//! (`plan_materialize`), what a run changed (`changes_of`), which of it is taken
//! over at once (`auto_hit`, `split_changes`), which programs may start at all
//! (`allowed`) and where a path lands on disk (`rel_of`, `check_base`,
//! `ignored`); `run` caps a run (`cap_ms`) and builds its whole environment
//! (`run_env`). A regression here is a tool change overwritten, a file written
//! outside its directory or a program that should never have started -- so the
//! docstrings are pinned as behaviour. The second half asks the shipped CEL of
//! `file-space` which door each `in_ws` op and each answer opens: the seven
//! projection ops go to `./projection` and nowhere else (exclusive, one answer
//! per request), and an answer to the projection never leaves the space.

#[path = "support/projection_hive.rs"]
mod projection_hive;

use meclaw_core::serde_json::{Map, Value, json};
use projection_hive::*;

const MANIFEST_PROBE: &str =
    "[plan_materialize(a['tree'], a['manifest'], a['disk'], a['force']) for a in ARGS]";

/// `rel_of`: a space path under the workspace root becomes a path relative to
/// the directory (OR-FJ-70); anything that could leave it is None (`outside_root`).
#[test]
fn a_path_lands_inside_its_directory_or_nowhere() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        ("root /", json!(["/a.py", "/"]), json!("a.py")),
        (
            "nested under a root",
            json!(["/repo/src/a.py", "/repo"]),
            json!("src/a.py"),
        ),
        (
            "a root with a trailing slash",
            json!(["/repo/src/a.py", "/repo/"]),
            json!("src/a.py"),
        ),
        (
            "outside the root",
            json!(["/other/a.py", "/repo"]),
            Value::Null,
        ),
        ("the root itself", json!(["/repo", "/repo"]), Value::Null),
        (
            "a prefix that is no directory",
            json!(["/repository/a", "/repo"]),
            Value::Null,
        ),
        ("dot-dot", json!(["/a/../../etc/passwd", "/"]), Value::Null),
        ("a dot segment", json!(["/a/./b", "/"]), Value::Null),
        ("an empty segment", json!(["/a//b", "/"]), Value::Null),
        ("not absolute", json!(["a.py", "/"]), Value::Null),
        ("a NUL", json!(["/a\u{0}b", "/"]), Value::Null),
    ];
    check("mat", "[rel_of(a[0], a[1]) for a in ARGS]", &table);
}

/// `check_base`: '' for a usable base, else the refusal code.
#[test]
fn the_base_is_absolute_and_outside_the_colony() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        ("empty", json!(["", "/colony"]), json!("no_base_path")),
        (
            "relative",
            json!(["proj", "/colony"]),
            json!("no_base_path"),
        ),
        (
            "the root of the disk",
            json!(["/", "/colony"]),
            json!("bad_base_path"),
        ),
        (
            "the cell's own working directory",
            json!(["/colony", "/colony"]),
            json!("bad_base_path"),
        ),
        (
            "inside it",
            json!(["/colony/proj", "/colony"]),
            json!("bad_base_path"),
        ),
        (
            "not normalised",
            json!(["/srv/../colony", "/colony"]),
            json!("bad_base_path"),
        ),
        ("a sibling", json!(["/srv/proj", "/colony"]), json!("")),
        (
            "a trailing slash",
            json!(["/srv/proj/", "/colony"]),
            json!(""),
        ),
        (
            "a name that only starts like the cwd",
            json!(["/colony2/p", "/colony"]),
            json!(""),
        ),
    ];
    check("mat", "[check_base(a[0], a[1]) for a in ARGS]", &table);
}

/// `colony_root` / `colony_above` (OR-FJ-74): the cwd counts as the colony
/// root only when it holds a `colony.json`; a base inside any colony tree is
/// found by walking up, whatever the cwd.
#[test]
fn the_colony_root_is_where_colony_json_lies() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "a cwd holding colony.json is the root",
            json!(["/colony", ["/colony/colony.json"]]),
            json!("/colony"),
        ),
        (
            "a cwd without one (a home directory) is no root",
            json!(["/home/u", ["/colony/colony.json"]]),
            json!(""),
        ),
        ("no cwd at all", json!(["", []]), json!("")),
    ];
    check(
        "mat",
        "[colony_root(a[0], lambda p, s=set(a[1]): p in s) for a in ARGS]",
        &table,
    );
    let above: Vec<(&str, Value, Value)> = vec![
        (
            "deep inside a colony",
            json!(["/srv/c/main/x", ["/srv/c/colony.json"]]),
            json!("/srv/c"),
        ),
        (
            "the colony root itself",
            json!(["/srv/c", ["/srv/c/colony.json"]]),
            json!("/srv/c"),
        ),
        (
            "a sibling of a colony",
            json!(["/srv/proj", ["/srv/c/colony.json"]]),
            json!(""),
        ),
        ("no colony anywhere", json!(["/srv/proj", []]), json!("")),
    ];
    check(
        "mat",
        "[colony_above(a[0], lambda p, s=set(a[1]): p in s) for a in ARGS]",
        &above,
    );
}

/// OR-FJ-74 on disk: a daemon whose cwd is not a colony root (a home
/// directory) does not lock the bases below it; once that cwd is a colony
/// root, the same base is refused.
#[test]
fn a_base_below_a_plain_cwd_is_usable_below_a_colony_it_is_not() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let home = td.path().join("home");
    let base = home.join("proj");
    std::fs::create_dir_all(&base).expect("base");
    let home_s = home.to_str().expect("utf-8").to_string();
    let base_s = base.to_str().expect("utf-8").to_string();
    let probe = "[__import__('os').chdir(a[0]) or base_problem(a[1]) for a in ARGS]";
    let got = pure("mat", probe, json!([[home_s, base_s]]));
    assert_eq!(got, json!([""]), "a plain cwd locks nothing below it");
    std::fs::write(home.join("colony.json"), "{}").expect("colony.json");
    let got = pure("mat", probe, json!([[home_s, base_s]]));
    assert_eq!(
        got,
        json!(["bad_base_path"]),
        "below a colony root the base is refused"
    );
}

/// Symlinks a tool planted (Review J I-2): laying out a file never makes a
/// directory through a symlink -- not even an empty one outside -- and the
/// scan names a symlink leading out instead of reading through it.
#[test]
fn a_symlink_a_tool_planted_never_leads_out() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let ws = td.path().join("w1");
    let out = td.path().join("out");
    std::fs::create_dir_all(&ws).expect("ws");
    std::fs::create_dir_all(&out).expect("out");
    std::os::unix::fs::symlink(&out, ws.join("sub")).expect("dir symlink");
    let s = |p: &std::path::Path| p.to_str().expect("utf-8").to_string();
    let stage = |n: &str| {
        let p = td.path().join(n);
        std::fs::write(&p, "bytes").expect("stage");
        s(&p)
    };
    let rows = json!([
        [s(&ws), "sub/deep/x", stage("s1")],
        [s(&ws), "sub/x", stage("s2")],
        [s(&ws), "ok/deep/x", stage("s3")],
    ]);
    let probe = concat!(
        "(exec('def _try(f):\\n try:\\n  f()\\n  return \"ok\"\\n",
        " except OSError:\\n  return \"refused\"', globals()),",
        " [_try(lambda a=a: put_file(a[0], a[1], a[2])) for a in ARGS])[1]"
    );
    let got = pure("mat", probe, rows);
    assert_eq!(got, json!(["refused", "refused", "ok"]));
    assert_eq!(
        std::fs::read_dir(&out).expect("out").count(),
        0,
        "nothing was made through the symlink, not even a directory"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("ok/deep/x")).expect("laid out"),
        "bytes"
    );

    // A file symlink out of the directory: named in `skipped`, never read.
    std::fs::write(out.join("secret"), "no").expect("secret");
    std::os::unix::fs::symlink(out.join("secret"), ws.join("leak")).expect("file symlink");
    let got = pure(
        "mat",
        "[scan(a[0], '/proj', []) for a in ARGS]",
        json!([[s(&ws)]]),
    );
    let (found, skipped) = (&got[0][0], &got[0][1]);
    assert!(found.get("/proj/ok/deep/x").is_some(), "{got}");
    assert!(found.get("/proj/leak").is_none(), "{got}");
    assert!(found.get("/proj/sub/secret").is_none(), "{got}");
    assert_eq!(skipped, &json!(["/proj/leak", "/proj/sub"]), "{got}");
}

/// `ignored`: globs over relative paths, `**` crosses directories.
#[test]
fn ignore_globs_cut_whole_directories() {
    if !shipped() {
        return;
    }
    let dflt = json!([".git/**", "target/**"]);
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "a git object",
            json!([".git/objects/ab/cd", dflt]),
            json!(true),
        ),
        (
            "a build product",
            json!(["target/debug/app", dflt]),
            json!(true),
        ),
        ("a source file", json!(["src/main.rs", dflt]), json!(false)),
        (
            "a .git segment at any depth (OR-FJ-72)",
            json!(["vendor/.git/x", dflt]),
            json!(true),
        ),
        (
            "even with no globs at all",
            json!(["a/.git", []]),
            json!(true),
        ),
        (
            "a name that only contains .git",
            json!(["x.git/a", dflt]),
            json!(false),
        ),
        (
            "a file named like the directory",
            json!(["target", dflt]),
            json!(false),
        ),
        ("one segment star", json!(["a.pyc", ["*.pyc"]]), json!(true)),
        (
            "star does not cross a slash",
            json!(["d/a.pyc", ["*.pyc"]]),
            json!(false),
        ),
        (
            "double star does",
            json!(["d/e/a.pyc", ["**.pyc"]]),
            json!(true),
        ),
        ("no globs", json!(["x", []]), json!(false)),
    ];
    check("mat", "[ignored(a[0], a[1]) for a in ARGS]", &table);
}

/// `allowed` (exact program name) and `auto_hit` (argv prefix, word by word).
#[test]
fn only_named_programs_start_and_only_formatters_adopt() {
    if !shipped() {
        return;
    }
    let allow: Vec<(&str, Value, Value)> = vec![
        (
            "listed",
            json!([["python3", "-c", "x"], ["python3"]]),
            json!(true),
        ),
        (
            "a path is not the name",
            json!([["/usr/bin/python3"], ["python3"]]),
            json!(false),
        ),
        ("nothing listed", json!([["python3"], []]), json!(false)),
        ("no argv", json!([[], ["python3"]]), json!(false)),
    ];
    check("mat", "[allowed(a[0], a[1]) for a in ARGS]", &allow);
    let auto: Vec<(&str, Value, Value)> = vec![
        (
            "cargo fmt --all",
            json!([["cargo", "fmt", "--all"], ["cargo fmt", "rustfmt"]]),
            json!(true),
        ),
        (
            "rustfmt a file",
            json!([["rustfmt", "src/a.rs"], ["cargo fmt", "rustfmt"]]),
            json!(true),
        ),
        (
            "a word that only starts alike",
            json!([["cargo", "fmtx"], ["cargo fmt"]]),
            json!(false),
        ),
        (
            "cargo test",
            json!([["cargo", "test"], ["cargo fmt", "rustfmt"]]),
            json!(false),
        ),
        (
            "the shipped default is not python3",
            json!([["python3", "-c", "x"], ["cargo fmt", "rustfmt"]]),
            json!(false),
        ),
    ];
    check("mat", "[auto_hit(a[0], a[1]) for a in ARGS]", &auto);
    let params = cell_config("mat")["params"].clone();
    assert_eq!(
        params["exec_allow"],
        json!([]),
        "OR-FJ-G2: nothing may run by default"
    );
    assert_eq!(
        params["auto_adopt"],
        json!(["cargo fmt", "rustfmt"]),
        "OR-FJ-G3"
    );
    assert_eq!(
        params["base_path"],
        json!(""),
        "no private path as a default"
    );
}

fn entry(file: &str, version: &str, sha: &str) -> Value {
    json!({"file": file, "version": version, "sha256": sha})
}

/// `plan_materialize`: a tool change nobody adopted is `dirty` and never
/// overwritten or removed (unless `force`); a moved version is fetched; a
/// path the tree no longer names is removed.
#[test]
fn a_materialisation_never_overwrites_what_a_tool_changed() {
    if !shipped() {
        return;
    }
    let tree = json!([
        {"path": "/a", "file": "fh-a", "version": "aaaaaaaaaaaa"},
        {"path": "/b", "file": "fh-b", "version": "bbbbbbbbbbbb"},
        {"path": "/c", "file": "fh-c", "version": "cccccccccccc"}
    ]);
    let mut manifest = Map::new();
    manifest.insert("/a".into(), entry("fh-a", "aaaaaaaaaaaa", "sa"));
    manifest.insert("/b".into(), entry("fh-b", "b0b0b0b0b0b0", "sb"));
    manifest.insert("/d".into(), entry("fh-d", "dddddddddddd", "sd"));
    manifest.insert("/e".into(), entry("fh-e", "eeeeeeeeeeee", "se"));
    let manifest = Value::Object(manifest);
    let disk = json!({"/a": "sa-tool", "/b": "sb", "/c": null, "/d": "sd", "/e": "se-tool"});
    let fetched = |paths: &[&str]| -> Value {
        let all = tree.as_array().unwrap();
        json!(
            paths
                .iter()
                .map(|p| all.iter().find(|t| t["path"] == json!(p)).unwrap().clone())
                .collect::<Vec<_>>()
        )
    };
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "the first time: everything, nothing to remove",
            json!({"tree": tree, "manifest": {}, "disk": {"/a": null, "/b": null, "/c": null}, "force": false}),
            json!({"fetch": fetched(&["/a", "/b", "/c"]), "remove": [], "dirty": []}),
        ),
        (
            "unchanged: nothing",
            json!({"tree": [tree[0]], "manifest": {"/a": entry("fh-a", "aaaaaaaaaaaa", "sa")},
                   "disk": {"/a": "sa"}, "force": false}),
            json!({"fetch": [], "remove": [], "dirty": []}),
        ),
        (
            "tool changes are dirty, a moved version is fetched, a gone path removed",
            json!({"tree": tree, "manifest": manifest, "disk": disk, "force": false}),
            json!({"fetch": fetched(&["/b", "/c"]), "remove": ["/d"], "dirty": ["/a", "/e"]}),
        ),
        (
            "force takes the space's word for everything",
            json!({"tree": tree, "manifest": manifest, "disk": disk, "force": true}),
            json!({"fetch": fetched(&["/a", "/b", "/c"]), "remove": ["/d", "/e"], "dirty": []}),
        ),
        (
            "a file a tool deleted is dirty too",
            json!({"tree": [tree[0]], "manifest": {"/a": entry("fh-a", "aaaaaaaaaaaa", "sa")},
                   "disk": {"/a": null}, "force": false}),
            json!({"fetch": [], "remove": [], "dirty": ["/a"]}),
        ),
        (
            "a file a tool made where the space now names one is dirty",
            json!({"tree": [tree[2]], "manifest": {}, "disk": {"/c": "sc-tool"}, "force": false}),
            json!({"fetch": [], "remove": [], "dirty": ["/c"]}),
        ),
        (
            "force lays the space's file over it",
            json!({"tree": [tree[2]], "manifest": {}, "disk": {"/c": "sc-tool"}, "force": true}),
            json!({"fetch": fetched(&["/c"]), "remove": [], "dirty": []}),
        ),
    ];
    check("mat", MANIFEST_PROBE, &table);
}

/// `changes_of` and `split_changes`: the difference after a run, numbered;
/// with `auto` only `modified` is taken over at once (OR-FJ-G3).
#[test]
fn a_scan_sorts_what_a_tool_wrote() {
    if !shipped() {
        return;
    }
    let manifest = json!({"/a": entry("fh-a", "aaaaaaaaaaaa", "1"),
                          "/b": entry("fh-b", "bbbbbbbbbbbb", "2"),
                          "/c": entry("fh-c", "cccccccccccc", "3")});
    let found = json!({"/a": "1x", "/c": "3", "/gen.txt": "9"});
    let changes = json!([
        {"path": "/a", "kind": "modified", "file": "fh-a", "base": "aaaaaaaaaaaa", "sha256": "1x"},
        {"path": "/b", "kind": "deleted", "file": "fh-b", "base": "bbbbbbbbbbbb"},
        {"path": "/gen.txt", "kind": "created", "sha256": "9"}
    ]);
    check(
        "mat",
        "[changes_of(a[0], a[1]) for a in ARGS]",
        &[(
            "modified, deleted, created in path order",
            json!([manifest, found]),
            changes.clone(),
        )],
    );
    let numbered = |states: [&str; 3]| -> Value {
        let mut out = Vec::new();
        for (i, (c, s)) in changes.as_array().unwrap().iter().zip(states).enumerate() {
            let mut e = c.clone();
            e["id"] = json!(i + 1);
            e["state"] = json!(s);
            out.push(e);
        }
        Value::Array(out)
    };
    check(
        "mat",
        "[split_changes(a[0], a[1], 1) for a in ARGS]",
        &[
            (
                "a formatter: modified at once, the rest proposed",
                json!([changes, true]),
                numbered(["auto", "open", "open"]),
            ),
            (
                "any other program: all proposed",
                json!([changes, false]),
                numbered(["open", "open", "open"]),
            ),
        ],
    );
    check(
        "mat",
        "[pick(a[0], a[1]) for a in ARGS]",
        &[
            (
                "ids: only open ones, the rest named",
                json!([[{"id": 1, "state": "open"}, {"id": 2, "state": "adopted"}, {"id": 3, "state": "open"}], [2, 3, 9]]),
                json!([[{"id": 3, "state": "open"}], [2, 9]]),
            ),
            (
                "all: every open one",
                json!([[{"id": 1, "state": "open"}, {"id": 2, "state": "adopted"}], "all"]),
                json!([[{"id": 1, "state": "open"}], []]),
            ),
        ],
    );
}

/// `run`: the caller only lowers the cap; the environment is exactly three
/// variables; the run directory lies strictly below the base.
#[test]
fn a_run_is_capped_and_its_environment_is_three_variables() {
    if !shipped() {
        return;
    }
    check(
        "run",
        "[cap_ms(a[0], a[1]) for a in ARGS]",
        &[
            ("no ask: the cap", json!([null, 600000]), json!(600000)),
            ("a lower ask", json!([1000, 600000]), json!(1000)),
            (
                "a higher ask cannot lift it",
                json!([900000000, 600000]),
                json!(600000),
            ),
            ("a broken cap falls back", json!([null, "x"]), json!(600000)),
        ],
    );
    check(
        "run",
        "[run_env(a[0], a[1], a[2]) for a in ARGS]",
        &[
            (
                "defaults",
                json!([null, "", "/srv/p"]),
                json!({"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": "/srv/p", "LANG": "C.UTF-8"}),
            ),
            (
                "set",
                json!(["/opt/bin", "/home/tool", "/srv/p"]),
                json!({"PATH": "/opt/bin", "HOME": "/home/tool", "LANG": "C.UTF-8"}),
            ),
        ],
    );
    check(
        "run",
        "[under(a[0], a[1]) for a in ARGS]",
        &[
            (
                "a workspace directory",
                json!(["/srv/p", "/srv/p/ws"]),
                json!(true),
            ),
            ("the base itself", json!(["/srv/p", "/srv/p"]), json!(false)),
            (
                "a sibling with the same prefix",
                json!(["/srv/p", "/srv/pp/ws"]),
                json!(false),
            ),
            ("no base", json!(["", "/x"]), json!(false)),
        ],
    );
    check(
        "run",
        "[len(tail(a)) for a in ARGS]",
        &[("the last 8 KiB", json!("x".repeat(9000)), json!(8192))],
    );
    let run = cell_config("run");
    let ext = run["params"]["external_timeout_ms"].as_u64().unwrap();
    let cap = run["params"]["exec_timeout_ms"].as_u64().unwrap();
    let msg = run["cell"]["message_timeout"].as_u64().unwrap();
    assert!(
        cap < ext && ext < msg,
        "AGENTS.md rule 12: cap {cap} < A {ext} < B {msg}"
    );
    for cell in ["mat", "run"] {
        let sb = &cell_config(cell)["params"]["sandbox"];
        assert_eq!(sb["trust"], json!("restricted"), "{cell}");
        assert_eq!(sb["network"], json!("deny"), "{cell}");
        assert!(
            sb["filesystem"].get("write").is_none(),
            "{cell}: no write grant in the template"
        );
    }
}

// ─────────────────────────────────────────────────────────────── the doors

fn space_edges() -> Vec<Value> {
    read_json(&repo("templates/file-space/config.json"))["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .clone()
}

fn open_doors(edges: &[Value], from: &str, hop: &Value) -> Vec<String> {
    let hop: Map<String, Value> = hop.as_object().unwrap().clone();
    edges
        .iter()
        .filter(|e| e["from"] == json!(from))
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or("true");
            let c = meclaw_colony::cel_eval::parse_condition(c).unwrap();
            matches!(
                meclaw_colony::cel_eval::evaluate_condition(&c, &Default::default(), &hop),
                Ok(true)
            )
        })
        .map(|e| e["to"].as_str().unwrap().to_string())
        .collect()
}

const PROJECTION_OPS: [&str; 7] = [
    "ws_materialize",
    "ws_exec",
    "ws_adopt",
    "ws_export_git",
    "ws_import_git",
    "ws_push",
    "ws_pull",
];

/// Every `in_ws` op opens exactly one door: the seven projection ops
/// `./projection`, every other one (and none) `./ws`.
#[test]
fn every_in_ws_op_opens_exactly_one_door() {
    if !shipped() {
        return;
    }
    let edges = space_edges();
    let ws_ops = [
        "ws_open",
        "ws_status",
        "ws_diff",
        "ws_patch",
        "ws_merge",
        "ws_commit",
        "ws_discard",
        "ws_tree",
        "nonsense",
        "",
    ];
    for op in PROJECTION_OPS.iter().chain(ws_ops.iter()) {
        let mut hop = json!({"route": "in_ws", "op_id": "x"});
        if !op.is_empty() {
            hop["op"] = json!(op);
        }
        let want = if PROJECTION_OPS.contains(op) {
            "./projection"
        } else {
            "./ws"
        };
        assert_eq!(open_doors(&edges, ".", &hop), vec![want], "op {op:?}");
    }
}

/// An answer of `ws`, `read` or `write` to the projection goes back into it
/// and never out of the space; an answer to an outside caller goes out.
#[test]
fn an_answer_to_the_projection_never_leaves_the_space() {
    if !shipped() {
        return;
    }
    let edges = space_edges();
    for cell in ["./ws", "./read", "./write"] {
        let inside = json!({"route": "answer", "op_id": "mat:j:t", "caller": "projection"});
        assert_eq!(
            open_doors(&edges, cell, &inside),
            vec!["./projection"],
            "{cell}"
        );
        let outside = json!({"route": "answer", "op_id": "q1"});
        assert_eq!(open_doors(&edges, cell, &outside), vec!["."], "{cell}");
    }
    for (route, to) in [
        ("in_ws", "./ws"),
        ("in_read", "./read"),
        ("in_write", "./write"),
    ] {
        let hop = json!({"route": route, "op_id": "mat:j:1", "caller": "projection"});
        assert_eq!(
            open_doors(&edges, "./projection", &hop),
            vec![to],
            "{route}"
        );
    }
    let hop = json!({"route": "answer", "op_id": "q1", "op": "ws_exec"});
    assert_eq!(open_doors(&edges, "./projection", &hop), vec!["."]);
}

/// The doors inside the projection (OR-FJ-68, Review J M-1): `. -> ./mat`
/// opens for the three J ops and an answer under `mat:`, never for a git op
/// or a git answer; `./mat -> .` carries no answer meant for `git`.
#[test]
fn the_projection_opens_mat_for_its_own_ops_only() {
    if !shipped() {
        return;
    }
    let edges = read_json(&repo("templates/projection/config.json"))["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .clone();
    for op in PROJECTION_OPS {
        let hop = json!({"route": "in_proj", "op": op, "op_id": "q1"});
        let mat = open_doors(&edges, ".", &hop).contains(&"./mat".to_string());
        assert_eq!(mat, PROJECTION_OPS[..3].contains(&op), "op {op}");
    }
    // No op or an unknown one reaches `mat` (it answers `unknown_op`): the
    // door is the lane minus the git ops, so gh173's lane probe finds it.
    for hop in [
        json!({"route": "in_proj", "op_id": "q1"}),
        json!({"route": "in_proj", "op": "nonsense", "op_id": "q1"}),
    ] {
        assert_eq!(open_doors(&edges, ".", &hop), vec!["./mat"], "{hop}");
    }
    let mine = json!({"route": "in_answer", "op_id": "mat:j:t"});
    assert_eq!(open_doors(&edges, ".", &mine), vec!["./mat"]);
    // an answer without the `mat:` prefix still reaches `mat`, which drops it
    // (the lane has a door for gh173); only `git:` answers never do
    let bare = json!({"route": "in_answer", "op_id": "q1"});
    assert_eq!(open_doors(&edges, ".", &bare), vec!["./mat"]);
    let gits = json!({"route": "in_answer", "op_id": "git:j:t"});
    assert!(!open_doors(&edges, ".", &gits).contains(&"./mat".to_string()));
    for (caller, out) in [("", true), ("projection", true), ("git", false)] {
        let mut hop = json!({"route": "answer", "op_id": "q1"});
        if !caller.is_empty() {
            hop["caller"] = json!(caller);
        }
        assert_eq!(
            open_doors(&edges, "./mat", &hop).contains(&".".to_string()),
            out,
            "caller {caller:?}"
        );
    }
}
