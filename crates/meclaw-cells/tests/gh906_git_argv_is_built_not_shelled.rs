//! GH #906: the pure half of the cell `git` of `templates/projection`, held to
//! tables, and its git plumbing held to a real `git` in the test's temporary
//! directory. git is the exchange format of a workspace (K7), never its
//! store: every call is an argv list under an explicit environment, a push is
//! never forced, an export stages only the manifest's paths, and no answer
//! carries a credential. The functions are plain functions of the cell's
//! `script_inline` -- a script knows no library (OR-FH-G1) -- so this lock
//! loads them the way the file-space tables do (the AST loader of
//! `support/file_space_hive.rs`, here with the cell's exception class).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::{read_json, repo, run_python};
use meclaw_core::serde_json::{self as sj, Value, json};
use std::path::Path;
use std::process::Command;

const CELL: &str = "templates/projection/git/config.json";

fn shipped() -> bool {
    repo(CELL).is_file()
}

fn config() -> Value {
    read_json(&repo(CELL))
}

fn script() -> String {
    config()["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Imports, defs, classes and upper-case constants of the script, in file
/// order, under `params` (with `over` laid on top); then `setup` (python
/// statements) runs and `probe` (a python expression, `ARGS` the argument) is
/// printed as JSON.
const LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
scope["ARGS"] = inp.get("args")
exec(inp.get("setup") or "", scope)
print(json.dumps(eval(inp["probe"], scope)))
"#;

fn pure_with(over: Value, setup: &str, probe: &str, args: Value) -> Value {
    let mut params = config()["params"].as_object().cloned().unwrap_or_default();
    params.remove("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"src": script(), "params": params, "setup": setup, "probe": probe,
                     "args": args});
    let out = run_python(LOADER, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "git: the pure half does not run: {err}"
    );
    sj::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

fn pure(probe: &str, args: Value) -> Value {
    pure_with(json!({}), "", probe, args)
}

/// One call of `probe` over every input of the table, each answer held to
/// its row.
fn check(probe: &str, table: &[(&str, Value, Value)]) {
    let args: Vec<Value> = table.iter().map(|(_, a, _)| a.clone()).collect();
    let got = pure(probe, json!(args));
    let got = got.as_array().expect("the probe answers a list");
    assert_eq!(got.len(), table.len(), "one answer per row: {got:?}");
    for ((label, arg, want), got) in table.iter().zip(got) {
        assert_eq!(got, want, "{label}: input {arg}");
    }
}

#[test]
fn credentials_never_leave_in_an_answer() {
    if !shipped() {
        return;
    }
    check(
        "[mask(a) for a in ARGS]",
        &[
            (
                "user and token in an https URL",
                json!("fatal: unable to access 'https://bot:ghp_secret@example.com/r.git/'"),
                json!("fatal: unable to access 'https://***@example.com/r.git/'"),
            ),
            (
                "two URLs in one text",
                json!("a ssh://u:p@h/x b http://x:y@z/"),
                json!("a ssh://***@h/x b http://***@z/"),
            ),
            (
                "a user alone is masked too (it may be a token)",
                json!("ssh://git@example.com/r.git"),
                json!("ssh://***@example.com/r.git"),
            ),
            (
                "a token as the user of an https URL",
                json!("fatal: 'https://ghp_0123456789@example.com/r.git' refused"),
                json!("fatal: 'https://***@example.com/r.git' refused"),
            ),
            (
                "a local path stays",
                json!("/srv/remote.git"),
                json!("/srv/remote.git"),
            ),
        ],
    );
    // The tail keeps the LAST bytes, and it is masked too.
    let got = pure(
        "[len(tail('x' * 9000).encode()), tail('https://a:b@h/ ' + 'y' * 10)]",
        json!(null),
    );
    assert_eq!(got, json!([4096, "https://***@h/ yyyyyyyyyy"]));
}

#[test]
fn a_ref_cannot_hide_an_option() {
    if !shipped() {
        return;
    }
    check(
        "[valid_ref(a) for a in ARGS]",
        &[
            ("a branch", json!("main"), json!(true)),
            ("a nested branch", json!("feature/x-1.2"), json!(true)),
            ("an option", json!("--upload-pack=touch"), json!(false)),
            ("a leading slash", json!("/main"), json!(false)),
            ("two dots", json!("a..b"), json!(false)),
            ("a space", json!("a b"), json!(false)),
            ("a reflog form", json!("main@{1}"), json!(false)),
            ("a trailing .lock", json!("main.lock"), json!(false)),
            ("a hidden segment", json!("a/.b"), json!(false)),
            ("empty", json!(""), json!(false)),
            ("a force refspec", json!("+main"), json!(false)),
        ],
    );
}

#[test]
fn a_remote_is_a_name_or_an_absolute_path() {
    if !shipped() {
        return;
    }
    let remotes = json!({"origin": "/srv/r.git", "hub": "https://example.com/r.git"});
    let rows = [
        ("a name", "origin", json!("/srv/r.git")),
        ("a URL name", "hub", json!("https://example.com/r.git")),
        ("an absolute path", "/data/x.git", json!("/data/x.git")),
        ("a relative path", "x.git", Value::Null),
        ("a path that climbs", "/data/../etc", Value::Null),
        ("an unknown URL", "https://evil.example/r.git", Value::Null),
        ("an option", "--upload-pack=x", Value::Null),
        ("empty", "", Value::Null),
    ];
    let args: Vec<Value> = rows.iter().map(|(_, a, _)| json!(a)).collect();
    let got = pure_with(
        json!({"remotes": remotes}),
        "",
        "[resolve_remote(a, P['remotes']) for a in ARGS]",
        json!(args),
    );
    for ((label, _, want), got) in rows.iter().zip(got.as_array().unwrap()) {
        assert_eq!(got, want, "{label}");
    }
}

#[test]
fn a_remote_with_userinfo_is_refused() {
    // OR-FJ-71: B2 has no credential path. A URL that carries a user (with or
    // without a secret) is `credentials_unsupported`, never used.
    if !shipped() {
        return;
    }
    check(
        "[has_userinfo(a) for a in ARGS]",
        &[
            (
                "user and secret",
                json!("https://u:p@example.com/r.git"),
                json!(true),
            ),
            (
                "a token as the user",
                json!("https://ghp_x@example.com/r.git"),
                json!(true),
            ),
            (
                "ssh with a user",
                json!("ssh://git@example.com/r.git"),
                json!(true),
            ),
            ("scp form", json!("git@example.com:r.git"), json!(true)),
            (
                "a plain https URL",
                json!("https://example.com/r.git"),
                json!(false),
            ),
            (
                "an @ in the path only",
                json!("https://example.com/a@b/r.git"),
                json!(false),
            ),
            ("a local path", json!("/srv/r.git"), json!(false)),
            ("a local path with @", json!("/srv/a@b.git"), json!(false)),
        ],
    );
}

#[test]
fn every_call_is_an_argv_without_force() {
    if !shipped() {
        return;
    }
    let got = pure(
        "[argv_push('/r', '/srv/x.git', 'main'), argv_fetch('/r', '/srv/x.git', 'main'), \
          argv_commit_tree('/r', 't', ['p1', 'p2'], 'a note; rm -rf /')]",
        json!(null),
    );
    let lists = got.as_array().unwrap();
    for l in lists {
        let l: Vec<&str> = l
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(&l[..3], &["git", "-C", "/r"], "{l:?}");
        assert!(l.contains(&"core.autocrlf=false"), "{l:?}");
        // OR-FJ-72: a foreign tree must not run code through git itself.
        assert!(l.contains(&"core.hooksPath=/dev/null"), "{l:?}");
        assert!(l.contains(&"core.fsmonitor=false"), "{l:?}");
        assert!(
            !l.iter()
                .any(|a| *a == "--force" || *a == "-f" || a.starts_with("+")),
            "no force: {l:?}"
        );
    }
    let push: Vec<&str> = lists[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(
        &push[push.len() - 3..],
        &["--", "/srv/x.git", "HEAD:refs/heads/main"]
    );
    let fetch: Vec<&str> = lists[1]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(&fetch[fetch.len() - 3..], &["--", "/srv/x.git", "main"]);
    let commit: Vec<&str> = lists[2]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(
        &commit[commit.len() - 8..],
        &[
            "commit-tree",
            "t",
            "-p",
            "p1",
            "-p",
            "p2",
            "-m",
            "a note; rm -rf /"
        ]
    );
    // No shell anywhere in the script: one `subprocess.run` with an argv and
    // `shell=False`, no `os.system`, no `popen`.
    let src = script();
    assert!(!src.contains("shell=True"));
    assert!(!src.contains("os.system") && !src.contains("popen"));
    assert_eq!(src.matches("subprocess.run(").count(), 1);
    assert!(src.contains("shell=False"));
}

#[test]
fn git_reads_no_configuration_of_the_host() {
    if !shipped() {
        return;
    }
    let got = pure(
        "[git_env('/b/ws', 'Ada <ada@example.org>'), git_env('/b/ws', 'nonsense')]",
        json!(null),
    );
    let env = got[0].as_object().unwrap();
    let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "GIT_AUTHOR_EMAIL",
            "GIT_AUTHOR_NAME",
            "GIT_COMMITTER_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_LITERAL_PATHSPECS",
            "GIT_TERMINAL_PROMPT",
            "HOME",
            "LANG",
            "LC_ALL",
            "PATH"
        ]
    );
    assert_eq!(env["HOME"], json!("/b/ws"));
    assert_eq!(env["GIT_CONFIG_GLOBAL"], json!("/dev/null"));
    assert_eq!(env["GIT_AUTHOR_NAME"], json!("Ada"));
    assert_eq!(got[1]["GIT_AUTHOR_EMAIL"], json!("meclaw@example.invalid"));
}

#[test]
fn ls_tree_is_parsed_and_sorted_into_what_enters() {
    if !shipped() {
        return;
    }
    let raw = concat!(
        "100644 blob 1111111111111111111111111111111111111111      12\tREADME.md\0",
        "100755 blob 2222222222222222222222222222222222222222       3\tbin/run.sh\0",
        "120000 blob 3333333333333333333333333333333333333333       9\tlink\0",
        "160000 commit 4444444444444444444444444444444444444444       -\tvendor/lib\0",
        "100644 blob 5555555555555555555555555555555555555555 99999999\tbig.bin\0",
        "100644 blob 6666666666666666666666666666666666666666       1\ta dir/with space.md\0",
        "100644 blob 7777777777777777777777777777777777777777       1\t.git/config\0",
        "100755 blob 8888888888888888888888888888888888888888       1\tsub/.GIT/hooks/pre-push\0",
        "100644 blob 9999999999999999999999999999999999999999       1\ta/../b.md\0"
    );
    let got = pure(
        "[[e['path'], e['size'], skip_reason(e, 1000)] for e in parse_ls_tree(ARGS)]",
        json!(raw),
    );
    assert_eq!(
        got,
        json!([
            ["README.md", 12, ""],
            ["bin/run.sh", 3, ""],
            ["link", 9, "symlink"],
            ["vendor/lib", -1, "submodule"],
            ["big.bin", 99999999, "too_large"],
            ["a dir/with space.md", 1, ""],
            [".git/config", 1, "bad_path"],
            ["sub/.GIT/hooks/pre-push", 1, "bad_path"],
            ["a/../b.md", 1, "outside_root"]
        ])
    );
}

#[test]
fn an_import_writes_only_the_difference() {
    if !shipped() {
        return;
    }
    let d1 = "a".repeat(64);
    let d2 = "b".repeat(64);
    let d3 = "c".repeat(64);
    let entries = json!([
        {"path": "same.md", "sha": "s1", "digest": d1},
        {"path": "changed.md", "sha": "s2", "digest": d2},
        {"path": "new/born.md", "sha": "s3", "digest": d3}
    ]);
    let tree = json!([
        {"file": "fh-000000000001", "path": "/same.md", "kind": "text", "version": &d1[..12]},
        {"file": "fh-000000000002", "path": "/changed.md", "kind": "text", "version": "dddddddddddd"},
        {"file": "fh-000000000003", "path": "/gone.md", "kind": "text", "version": "eeeeeeeeeeee"}
    ]);
    let got = pure(
        "import_plan(ARGS[0], ARGS[1], '/', [])",
        json!([entries, tree]),
    );
    assert_eq!(
        got,
        json!({"same": 1, "steps": [
            {"do": "overwrite", "path": "/changed.md", "sha": "s2",
             "file": "fh-000000000002", "base": "dddddddddddd"},
            {"do": "create", "path": "/new/born.md", "sha": "s3"},
            {"do": "remove", "path": "/gone.md", "file": "fh-000000000003",
             "base": "eeeeeeeeeeee"}
        ]})
    );
}

#[test]
fn an_import_stays_under_the_workspace_root() {
    // OR-FJ-70: the repository tree is root-relative. A workspace rooted at
    // `/proj` gets `a.md` as `/proj/a.md`; a file outside the root is never
    // touched; a path the import skipped is never removed (M-3).
    if !shipped() {
        return;
    }
    let d = "a".repeat(64);
    let entries = json!([{"path": "a.md", "sha": "s1", "digest": d}]);
    let tree = json!([
        {"file": "fh-000000000001", "path": "/proj/old.md", "kind": "text", "version": "eeeeeeeeeeee"},
        {"file": "fh-000000000002", "path": "/proj/link", "kind": "text", "version": "ffffffffffff"},
        {"file": "fh-000000000003", "path": "/other.md", "kind": "text", "version": "dddddddddddd"}
    ]);
    let got = pure(
        "import_plan(ARGS[0], ARGS[1], '/proj/', ['/proj/link'])",
        json!([entries, tree]),
    );
    assert_eq!(
        got,
        json!({"same": 0, "steps": [
            {"do": "create", "path": "/proj/a.md", "sha": "s1"},
            {"do": "remove", "path": "/proj/old.md", "file": "fh-000000000001",
             "base": "eeeeeeeeeeee"}
        ]})
    );
    check(
        "[[repo_path(p, r), hive_path(q, r)] for p, q, r in ARGS]",
        &[
            (
                "the space root",
                json!(["/a/b.md", "a/b.md", "/"]),
                json!(["a/b.md", "/a/b.md"]),
            ),
            (
                "a root",
                json!(["/proj/a.md", "a.md", "/proj"]),
                json!(["a.md", "/proj/a.md"]),
            ),
            (
                "outside the root",
                json!(["/projx/a.md", "a.md", "/proj"]),
                json!([null, "/proj/a.md"]),
            ),
            (
                "the root itself",
                json!(["/proj", "a.md", "/proj"]),
                json!([null, "/proj/a.md"]),
            ),
            (
                "a climbing path",
                json!(["/proj/../etc/x", "a.md", "/proj"]),
                json!([null, "/proj/a.md"]),
            ),
        ],
    );
}

#[test]
fn an_export_stages_the_manifest_and_nothing_beside_it() {
    if !shipped() {
        return;
    }
    check(
        "[manifest_paths(a, '/') for a in ARGS]",
        &[
            (
                "hive paths lose their root, in order",
                json!({"/src/b.rs": {}, "/a.md": {}}),
                json!(["a.md", "src/b.rs"]),
            ),
            (
                "a manifest as the store keeps it (json text)",
                json!("{\"/x.md\": {\"file\": \"fh-000000000001\"}}"),
                json!(["x.md"]),
            ),
            ("no manifest", json!(null), json!([])),
            (
                "a .git segment never enters a commit (OR-FJ-72)",
                json!({"/.git/config": {}, "/a/.Git/hooks/x": {}, "/ok.md": {}}),
                json!(["ok.md"]),
            ),
        ],
    );
    // OR-FJ-70: a rooted workspace loses its root; a path outside it stays out.
    let got = pure(
        "manifest_paths(ARGS, '/proj')",
        json!({"/proj/src/b.rs": {}, "/proj/a.md": {}, "/other.md": {}}),
    );
    assert_eq!(got, json!(["a.md", "src/b.rs"]));
    let got = pure(
        "[unstaged(['a.md', 'gone.md', ''], ['a.md']), parents_of('h', 'u', False), \
          parents_of('h', 'u', True), parents_of('', 'u', False), parents_of('', '', False)]",
        json!(null),
    );
    assert_eq!(got, json!([["gone.md"], ["h", "u"], ["h"], ["u"], []]));
}

// ------------------------------------------------------------ real git

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The plumbing of the three git ops against a real `git`, the cell's own
/// functions called directly (the seam through the hive is
/// `gh906_a_repo_round_trips_through_the_hive`).
const PLUMBING: &str = r#"
import os, json
def _job(op, ws, **args):
    j = {"op": op, "op_id": "t", "caller": "", "ws": ws, "args": args}
    use_env(j)
    return j
def _code(fn, *a):
    try:
        r = fn(*a)
        return r.get("error", {}).get("code") if not r.get("ok") else "ok"
    except GitError as e:
        return e.code
"#;

#[test]
fn export_import_and_push_work_on_a_real_git() {
    if !shipped() {
        return;
    }
    let tmp = tempfile::tempdir().expect("a temp dir under TMPDIR");
    let t = tmp.path();
    let base = t.join("proj");
    let src = t.join("src");
    let bare = t.join("bare.git");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("a.md"), "a\n").unwrap();
    std::fs::write(src.join("b.bin"), [0u8, 1, 2, 255]).unwrap();
    std::os::unix::fs::symlink("a.md", src.join("link")).unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "init"]);
    git(
        t,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    // A workspace directory as `./mat` leaves it: two files of the manifest
    // and a tool's artifact beside them.
    let ws = base.join("B");
    std::fs::create_dir_all(ws.join("target")).unwrap();
    std::fs::write(ws.join("x.md"), "x\n").unwrap();
    std::fs::write(ws.join("y.md"), "y\n").unwrap();
    std::fs::write(ws.join("target/junk"), "j").unwrap();
    std::fs::write(ws.join(".gitignore"), "x.md\n").unwrap();

    let over = json!({"base_path": base, "remotes": {"origin": bare}, "import_max_files": 3});
    let probe = r#"[
        _code(do_push, _job("ws_push", "B", remote="origin", branch="main")),
        do_export(_job("ws_export_git", "B", note="first"),
                  {"manifest": json.dumps({"/x.md": {}, "/y.md": {}})}).get("files"),
        _code(do_export, _job("ws_export_git", "B", note="again"),
              {"manifest": json.dumps({"/x.md": {}, "/y.md": {}})}),
        _code(do_export, _job("ws_export_git", "B", note="y gone"),
              {"manifest": json.dumps({"/x.md": {}})}),
        _code(do_push, _job("ws_push", "B", remote="origin", branch="main")),
        _code(do_push, _job("ws_push", "B", remote="elsewhere", branch="main")),
        [len(r[1]), r[2]] if (r := do_fetch(_job("ws_import_git", "A", source=ARGS))) else None,
        _code(do_push, _job("ws_push", "A", remote="origin", branch="--force")),
    ]"#;
    let got = pure_with(over.clone(), PLUMBING, probe, json!(src));
    assert_eq!(got[0], json!("not_exported"), "{got}");
    assert_eq!(got[1], json!(2), "{got}");
    assert_eq!(got[2], json!("nothing_to_commit"), "{got}");
    assert_eq!(got[3], json!("ok"), "{got}");
    assert_eq!(got[4], json!("ok"), "{got}");
    assert_eq!(got[5], json!("remote_unknown"), "{got}");
    assert_eq!(
        got[6],
        json!([2, [{"path": "link", "reason": "symlink"}]]),
        "{got}"
    );
    assert_eq!(got[7], json!("bad_request"), "{got}");
    // Only the manifest landed: `.gitignore` did not keep `x.md` out, the
    // tool's `target/` and the `.gitignore` itself stayed out, the removed
    // `y.md` left the index.
    assert_eq!(git(&ws, &["ls-files"]), "x.md");
    assert_eq!(git(&ws, &["log", "--format=%s"]), "y gone\nfirst");
    assert_eq!(git(&bare, &["log", "--format=%s", "main"]), "y gone\nfirst");
    // The fetch holds its commit as `incoming`; the upstream waits for the
    // last write of the pull (review I-1).
    assert_eq!(
        git(&base.join("A"), &["rev-parse", "refs/meclaw/incoming"]),
        git(&src, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&base.join("A"), &["for-each-ref", "refs/meclaw/upstream"]),
        ""
    );

    // Too many entries: refused before anything moves.
    std::fs::write(src.join("c.md"), "c\n").unwrap();
    std::fs::write(src.join("d.md"), "d\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "more"]);
    let got = pure_with(
        over.clone(),
        PLUMBING,
        r#"_code(do_fetch, _job("ws_import_git", "A", source=ARGS))"#,
        json!(src),
    );
    assert_eq!(got, json!("too_many_files"));
    assert_ne!(
        git(&base.join("A"), &["rev-parse", "refs/meclaw/incoming"]),
        git(&src, &["rev-parse", "HEAD"])
    );

    // A remote that moved on: the push is refused, never forced; after a pull
    // the export has both heads as parents and the push fast-forwards.
    let clone = t.join("clone");
    git(
        t,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    std::fs::write(clone.join("x.md"), "remote\n").unwrap();
    git(&clone, &["commit", "-qam", "remote"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    let remote_head = git(&bare, &["rev-parse", "main"]);
    let got = pure_with(
        over.clone(),
        PLUMBING,
        r#"[_code(do_push, _job("ws_push", "B", remote="origin", branch="main")),
            (lambda c: next_write(dict(_job("ws_pull", "B"), id="gj-t", steps=[], i=0,
                 tally={"created": 0, "modified": 1, "removed": 0}, skipped=[],
                 upstream=c))[1]["upstream"])(
                 do_fetch(_job("ws_pull", "B", remote="origin", branch="main"))[0])]"#,
        json!(null),
    );
    assert_eq!(got[0], json!("push_rejected"));
    assert_eq!(got[1], json!(remote_head));
    std::fs::write(ws.join("x.md"), "remote\n").unwrap();
    let got = pure_with(
        over,
        PLUMBING,
        r#"[_code(do_export, _job("ws_export_git", "B", note="merge"),
                  {"manifest": json.dumps({"/x.md": {}})}),
            _code(do_push, _job("ws_push", "B", remote="origin", branch="main"))]"#,
        json!(null),
    );
    assert_eq!(got, json!(["ok", "ok"]));
    assert_eq!(git(&bare, &["log", "-1", "--format=%s", "main"]), "merge");
    assert_eq!(
        git(&bare, &["rev-list", "--parents", "-1", "main"])
            .split(' ')
            .count(),
        3,
        "the export after a pull is a merge of the head and the upstream"
    );
}

/// OR-FJ-72, OR-FJ-71, OR-FJ-70 against a real `git`: a foreign tree that
/// smuggles `.git/…` in is skipped, a hook or fsmonitor in the repository of
/// a workspace never runs, a remote with userinfo is refused before git
/// starts, and a rooted workspace exports root-relative.
#[test]
fn a_foreign_tree_runs_no_code_and_brings_no_credentials() {
    if !shipped() {
        return;
    }
    let tmp = tempfile::tempdir().expect("a temp dir under TMPDIR");
    let t = tmp.path();
    let base = t.join("proj");
    let src = t.join("src");
    let bare = t.join("bare.git");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    git(
        t,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    // A commit whose tree holds `.git/config` next to `ok.md` -- `git add`
    // refuses that, `mktree` does not, and a fetch takes it by default.
    let hash = |text: &str| -> String {
        let p = t.join("blob");
        std::fs::write(&p, text).unwrap();
        git(&src, &["hash-object", "-w", p.to_str().unwrap()])
    };
    let cfg = hash("[core]\n\thooksPath = hooks\n");
    let ok = hash("ok\n");
    let mktree = |lines: &str| -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&src)
            .arg("mktree")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(lines.as_bytes())?;
                c.wait_with_output()
            })
            .expect("git mktree");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let inner = mktree(&format!("100644 blob {cfg}\tconfig\n"));
    let top = mktree(&format!(
        "040000 tree {inner}\t.git\n100644 blob {ok}\tok.md\n"
    ));
    let commit = git(&src, &["commit-tree", &top, "-m", "evil"]);
    git(&src, &["update-ref", "refs/heads/main", &commit]);

    // The repository of workspace B: exported once, then armed with a
    // pre-push hook and an fsmonitor that would leave a marker.
    let ws = base.join("B");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("x.md"), "x\n").unwrap();
    let marker = t.join("ran");
    let over = json!({"base_path": base, "remotes": {
        "origin": bare, "hub": "https://bot:ghp_secret@example.invalid/r.git",
        "scp": "git@example.invalid:r.git"}});
    let first = pure_with(
        over.clone(),
        PLUMBING,
        r#"_code(do_export, _job("ws_export_git", "B", note="first"),
                 {"manifest": json.dumps({"/x.md": {}})})"#,
        json!(null),
    );
    assert_eq!(first, json!("ok"));
    let hook = ws.join(".git/hooks/pre-push");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&ws, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
    std::fs::write(ws.join("x.md"), "x2\n").unwrap();

    let got = pure_with(
        over,
        PLUMBING,
        r#"[
            _code(do_export, _job("ws_export_git", "B", note="second"),
                  {"manifest": json.dumps({"/x.md": {}})}),
            _code(do_push, _job("ws_push", "B", remote="origin", branch="main")),
            _code(do_push, _job("ws_push", "B", remote="hub", branch="main")),
            _code(do_push, _job("ws_push", "B", remote="scp", branch="main")),
            _code(do_fetch, _job("ws_pull", "B", remote="hub", branch="main")),
            [[r[1][0]["path"], r[2]] if (r := do_fetch(_job("ws_import_git", "A", source=ARGS)))
             else None][0],
        ]"#,
        json!(src),
    );
    assert_eq!(got[0], json!("ok"), "{got}");
    assert_eq!(got[1], json!("ok"), "{got}");
    assert_eq!(got[2], json!("credentials_unsupported"), "{got}");
    assert_eq!(got[3], json!("credentials_unsupported"), "{got}");
    assert_eq!(got[4], json!("credentials_unsupported"), "{got}");
    assert_eq!(
        got[5],
        json!(["ok.md", [{"path": ".git/config", "reason": "bad_path"}]]),
        "{got}"
    );
    assert!(
        !marker.exists(),
        "a hook or fsmonitor of the repository ran"
    );
    assert!(!got.to_string().contains("ghp_secret"), "{got}");

    // A workspace rooted at `/proj`: the commit holds `a.md`, not `proj/a.md`.
    let wr = base.join("R");
    std::fs::create_dir_all(&wr).unwrap();
    std::fs::write(wr.join("a.md"), "a\n").unwrap();
    let got = pure_with(
        json!({"base_path": base}),
        PLUMBING,
        r#"(lambda j: (j.update(root="/proj"), _code(do_export, j,
             {"manifest": json.dumps({"/proj/a.md": {}, "/other.md": {}})}))[1])(
             _job("ws_export_git", "R", note="rooted"))"#,
        json!(null),
    );
    assert_eq!(got, json!("ok"));
    assert_eq!(git(&wr, &["ls-files"]), "a.md");
}

/// Review I-1 (Fix-Runde 2): `refs/meclaw/upstream` moves only once a pull
/// has written everything into the workspace. Before, the fetch set it: a
/// pull that failed after the fetch (a `cat-file` time-out, `ws_tree`
/// refused, a store error, a write refused) left it on the foreign commit,
/// the next export took it as a second parent over a tree WITHOUT the
/// foreign change, and the push fast-forwarded that change away in silence
/// -- the very case `push_rejected` exists for.
#[test]
fn a_failed_pull_never_moves_the_upstream() {
    if !shipped() {
        return;
    }
    let tmp = tempfile::tempdir().expect("a temp dir under TMPDIR");
    let t = tmp.path();
    let base = t.join("proj");
    let bare = t.join("bare.git");
    git(
        t,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    let ws = base.join("B");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("n.md"), "old\n").unwrap();
    let over = json!({"base_path": base, "remotes": {"origin": bare}});
    let got = pure_with(
        over.clone(),
        PLUMBING,
        r#"[_code(do_export, _job("ws_export_git", "B", note="first"),
                  {"manifest": json.dumps({"/n.md": {}})}),
            _code(do_push, _job("ws_push", "B", remote="origin", branch="main"))]"#,
        json!(null),
    );
    assert_eq!(got, json!(["ok", "ok"]));
    // A change made directly in the remote.
    let clone = t.join("clone");
    git(
        t,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    std::fs::write(clone.join("n.md"), "foreign\n").unwrap();
    git(&clone, &["commit", "-qam", "foreign"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    let foreign = git(&bare, &["rev-parse", "main"]);
    let upstream = || -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&ws)
            .args(["rev-parse", "--verify", "--quiet", "refs/meclaw/upstream"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    // The pull fetches, then fails before its last write (here: the import
    // stops right after the fetch, as a refused `ws_tree` or a store error
    // stops it). The workspace is untouched, a new export and a push follow.
    std::fs::write(ws.join("n.md"), "mine\n").unwrap();
    let got = pure_with(
        over.clone(),
        PLUMBING,
        r#"[do_fetch(_job("ws_pull", "B", remote="origin", branch="main"))[0],
            _code(do_export, _job("ws_export_git", "B", note="mine"),
                  {"manifest": json.dumps({"/n.md": {}})}),
            _code(do_push, _job("ws_push", "B", remote="origin", branch="main"))]"#,
        json!(null),
    );
    assert_eq!(got[0], json!(foreign), "{got}");
    assert_eq!(got[1], json!("ok"), "{got}");
    assert_eq!(
        got[2],
        json!("push_rejected"),
        "a failed pull let the push fast-forward over the foreign change: {got}"
    );
    assert_eq!(upstream(), "", "the upstream moved without an import");
    assert_eq!(git(&bare, &["rev-parse", "main"]), foreign, "never forced");
    assert_eq!(git(&bare, &["show", "main:n.md"]), "foreign");

    // The last step of a pull: a write the space refused keeps the upstream
    // where it was; a pull whose writes all went through moves it.
    let got = pure_with(
        over.clone(),
        PLUMBING,
        r#"[(lambda c, refused: next_write(dict(_job("ws_pull", "B"), id="gj-t", steps=[], i=0,
                 tally={"created": 0, "modified": 1, "removed": 0}, skipped=[], upstream=c,
                 refused=refused))[1]["upstream"])(
                 do_fetch(_job("ws_pull", "B", remote="origin", branch="main"))[0], r)
            for r in (True, False)]"#,
        json!(null),
    );
    assert_eq!(got, json!(["", foreign]), "{got}");
    assert_eq!(upstream(), foreign);
    // Now the pull counts: the workspace holds the foreign change merged in,
    // the export has both heads as parents and the push fast-forwards.
    std::fs::write(ws.join("n.md"), "merged\n").unwrap();
    let got = pure_with(
        over,
        PLUMBING,
        r#"[_code(do_export, _job("ws_export_git", "B", note="merged"),
                  {"manifest": json.dumps({"/n.md": {}})}),
            _code(do_push, _job("ws_push", "B", remote="origin", branch="main"))]"#,
        json!(null),
    );
    assert_eq!(got, json!(["ok", "ok"]));
    assert_eq!(git(&bare, &["show", "main:n.md"]), "merged");
    git(&bare, &["merge-base", "--is-ancestor", &foreign, "main"]);
}
