//! GH #906 -- a repository round-trips through the hive (K7, R-27-5).
//!
//! git is the exchange format of a workspace, never its store. This file boots
//! the SHIPPED `file-space` with its projection in a real colony -- the code
//! cells under their own sandbox, which the owner's override opens for
//! writing on the projection's `base_path` only -- and walks the road the
//! Coding wave will walk:
//!
//! 1. A test repository of twelve entries (one binary, one symlink) is
//!    imported into workspace `A`: eleven files are born, the symlink is
//!    `skipped`.
//! 2. `README.md` is changed by `replace` inside `A`, and `ws_commit` puts it
//!    on the main line.
//! 3. A new workspace `B` is exported: `git log` in its projection directory
//!    shows the commit under its note, and `ws_push` puts it into an empty
//!    bare remote whose `main` then holds the changed README.
//! 4. The remote moves on by itself: `ws_push` is `push_rejected`, never
//!    forced; `ws_pull` brings the change in as a working version of `B`,
//!    and export plus `ws_push` go through as a fast-forward (a merge of the
//!    old head and the upstream).
//!
//! Every answer is read where it arrives -- at a capture cell behind the
//! hive path -- and the run leaves no dead letter. The source repository and
//! the remote live under `base_path` (the sandbox reads and writes nothing
//! else), everything under the test's temporary directory; no network.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! projection's cells is skipped, never judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, override_params_on_disk};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(60);

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/file-space/config.json",
        "templates/projection/config.json",
        "templates/projection/git/config.json",
    ]
    .iter()
    .all(|p| repo(p).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The shipped template laid out the way instantiation lays it out; a
/// `cell.type: "ref"` directory is replaced by the referenced template, its
/// `override_params` applied (GH #277, GH #140) -- the pattern of
/// `gh889_a_turn_runs_collector_curator_brain.rs`.
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(depth < 8, "ref chain does not end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// The owner's side of the tree: every cell of the projection that carries a
/// `base_path` gets the run's directory and the sandbox write grant on
/// exactly that path (`override_params`, J § 1 of the wave); `git` gets the
/// remote by name. The summaries and embeddings of the space stay off -- no
/// provider is called in this run -- and the summarizer names a model that is
/// never asked.
fn owner_overrides(
    main: &std::path::Path,
    base: &std::path::Path,
    bare: &std::path::Path,
) -> usize {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut granted = 0;
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
        let mut cfg = read_json(&f);
        let dir = f.parent().expect("a cell directory");
        if cfg["params"].get("base_path").is_some() {
            cfg["params"]["base_path"] = json!(base);
            cfg["params"]["sandbox"]["filesystem"]["write"] = json!([base]);
            granted += 1;
        }
        if dir.ends_with("projection/git") {
            cfg["params"]["remotes"] = json!({"origin": bare});
        }
        if dir.ends_with("derive") {
            cfg["params"]["summary_on_commit"] = json!("0");
            cfg["params"]["embed"] = json!("0");
        }
        if cfg["cell"]["type"] == "llm" {
            cfg["params"]["model"] = json!("stub-never-asked");
            cfg["params"]["base_url"] = json!("http://127.0.0.1:9/v1");
        }
        write_json(&f, &cfg);
    }
    let root = main.parent().expect("the colony root");
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("the env file");
    granted
}

fn build(td: &tempfile::TempDir, base: &std::path::Path, bare: &std::path::Path) -> usize {
    let main = td.path().join("main");
    copy_resolved(&repo("templates/file-space"), &main.join("files"), 0);
    let rim: Vec<Value> = ["answer", "derived", "model_refused"]
        .iter()
        .map(|lane| {
            let to = if *lane == "answer" { "/sink" } else { "/park" };
            json!({"from": "./files", "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect();
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": rim}}}),
    );
    owner_overrides(&main, base, bare)
}

struct Run {
    h: ColonyHandle,
    sink: mpsc::Receiver<Message>,
    root: std::path::PathBuf,
    seq: u32,
}

async fn boot(td: &tempfile::TempDir) -> Run {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    let (park_tx, _park_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped file space and its projection must boot");
    Run {
        h,
        sink: sink_rx,
        root: td.path().to_path_buf(),
        seq: 0,
    }
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT error_code, sender_path, resolved_target FROM dead_letters ORDER BY id")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

impl Run {
    /// One request at the hive path; the ONE answer that arrives for it.
    async fn ask(
        &mut self,
        lane: &str,
        op: &str,
        file: Option<&str>,
        args: Value,
        ws: &str,
    ) -> Value {
        self.seq += 1;
        let op_id = format!("t906-{}", self.seq);
        let mut hop = json!({"route": lane, "op": op, "op_id": op_id});
        if !ws.is_empty() {
            hop["ws"] = json!(ws);
        }
        let mut body = json!({"op": op, "args": args});
        if let Some(f) = file {
            body["file"] = json!(f);
        }
        let msg = MessageBuilder::new(Path::new("/files"))
            .hop(map(hop))
            .context(Map::new())
            .body(Body::Inline(body))
            .ttl(400)
            .build();
        self.h.send(msg).await;
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            let m = match tokio::time::timeout_at(deadline, self.sink.recv()).await {
                Ok(Some(m)) => m,
                _ => panic!(
                    "{op}: no answer within {DEADLINE:?}; dead letters: {:#?}",
                    dead_letters(&self.root)
                ),
            };
            if m.headers.hop.get("op_id") != Some(&json!(op_id)) {
                continue;
            }
            let Body::Inline(v) = &m.body else {
                panic!("{op}: a blob answer")
            };
            assert_eq!(v["op"], json!(op), "the answer mirrors op: {v}");
            return v.clone();
        }
    }
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: {v}");
    v
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repo_round_trips_through_the_hive() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("a temp dir under TMPDIR");
    let base = td.path().join("projection");
    let src = base.join("source");
    let bare = base.join("remote.git");
    // The source: twelve entries -- README, nine text files, one binary, one
    // symlink.
    std::fs::create_dir_all(src.join("notes")).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("README.md"), "# demo\n\nfirst version\n").unwrap();
    for i in 0..9 {
        std::fs::write(src.join(format!("notes/n{i}.md")), format!("note {i}\n")).unwrap();
    }
    std::fs::write(src.join("logo.bin"), [0u8, 159, 146, 150, 255, 0, 1]).unwrap();
    std::os::unix::fs::symlink("README.md", src.join("link.md")).unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "source"]);
    git(
        &base,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );

    let granted = build(&td, &base, &bare);
    assert!(granted >= 1, "at least `git` carries a base_path");
    let mut run = boot(&td).await;
    let src_s = src.to_str().unwrap().to_string();

    // 1. import into A
    ok(run
        .ask(
            "in_ws",
            "ws_open",
            None,
            json!({"name": "A", "root": "/"}),
            "",
        )
        .await);
    let imp = ok(run
        .ask(
            "in_ws",
            "ws_import_git",
            None,
            json!({"source": src_s}),
            "A",
        )
        .await);
    assert_eq!(imp["created"], json!(11), "{imp}");
    assert_eq!(imp["modified"], json!(0), "{imp}");
    assert_eq!(imp["removed"], json!(0), "{imp}");
    assert_eq!(
        imp["skipped"],
        json!([{"path": "/link.md", "reason": "symlink"}]),
        "{imp}"
    );
    // The binary came in byte for byte.
    let raw = ok(run
        .ask("in_read", "raw", Some("/logo.bin"), json!({}), "A")
        .await);
    assert_eq!(raw["b64"], json!("AJ+Slv8AAQ=="), "{raw}");

    // 2. change README in A, commit
    let rd = ok(run
        .ask("in_read", "read", Some("/README.md"), json!({}), "A")
        .await);
    let base_v = rd["version"].as_str().expect("a version").to_string();
    ok(run
        .ask(
            "in_write",
            "replace",
            Some("/README.md"),
            json!({"old": "first version", "new": "changed in the hive", "base": base_v}),
            "A",
        )
        .await);
    ok(run
        .ask("in_ws", "ws_commit", None, json!({"note": "readme"}), "A")
        .await);

    // 3. export B, push into the empty remote
    ok(run
        .ask(
            "in_ws",
            "ws_open",
            None,
            json!({"name": "B", "root": "/"}),
            "",
        )
        .await);
    let pushed_early = run
        .ask(
            "in_ws",
            "ws_push",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await;
    assert_eq!(
        pushed_early["error"]["code"],
        json!("not_exported"),
        "{pushed_early}"
    );
    let ex = ok(run
        .ask(
            "in_ws",
            "ws_export_git",
            None,
            json!({"note": "export one"}),
            "B",
        )
        .await);
    assert_eq!(ex["files"], json!(11), "{ex}");
    let proj_b = base.join("B");
    assert_eq!(git(&proj_b, &["log", "--format=%s"]), "export one");
    ok(run
        .ask(
            "in_ws",
            "ws_push",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await);
    assert!(git(&bare, &["show", "main:README.md"]).contains("changed in the hive"));
    assert_eq!(
        git(&bare, &["ls-tree", "-r", "--name-only", "main"])
            .lines()
            .count(),
        11
    );

    // 4. the remote moves on; push refused, pull, export, fast-forward
    let clone = td.path().join("clone");
    git(
        td.path(),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    std::fs::write(clone.join("notes/n3.md"), "note 3, changed outside\n").unwrap();
    git(&clone, &["commit", "-qam", "outside"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    let outside = git(&bare, &["rev-parse", "main"]);
    let rej = run
        .ask(
            "in_ws",
            "ws_push",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await;
    assert_eq!(rej["error"]["code"], json!("push_rejected"), "{rej}");
    assert_eq!(git(&bare, &["rev-parse", "main"]), outside, "never forced");
    let pull = ok(run
        .ask(
            "in_ws",
            "ws_pull",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await);
    assert_eq!(pull["modified"], json!(1), "{pull}");
    assert_eq!(pull["created"], json!(0), "{pull}");
    assert_eq!(pull["removed"], json!(0), "{pull}");
    let n3 = ok(run
        .ask("in_read", "read", Some("/notes/n3.md"), json!({}), "B")
        .await);
    assert!(n3.to_string().contains("changed outside"), "{n3}");
    ok(run
        .ask(
            "in_ws",
            "ws_export_git",
            None,
            json!({"note": "merge outside"}),
            "B",
        )
        .await);
    ok(run
        .ask(
            "in_ws",
            "ws_push",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await);
    assert_eq!(
        git(&bare, &["log", "-1", "--format=%s", "main"]),
        "merge outside"
    );
    assert!(
        git(&bare, &["merge-base", "--is-ancestor", &outside, "main"]).is_empty(),
        "the outside commit is an ancestor: the push was a fast-forward"
    );

    let dead = dead_letters(&run.root);
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
    run.h.shutdown().await;
}
