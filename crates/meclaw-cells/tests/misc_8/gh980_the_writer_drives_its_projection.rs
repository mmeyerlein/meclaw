//! GH #980 -- the writer drives its projection: test, export and push are one
//! tool call each.
//!
//! Three model tools of the file space reach `./projection` from `in_tool`:
//! `file_ws_exec {ws, argv}` runs a permitted program in a workspace's
//! directory, `file_ws_export {root, note}` exports the main line under a root
//! into the git projection of that root (the git cell opens the view
//! `view_of(root)`, exports, discards it), `file_ws_push {root, remote?}`
//! pushes it to a remote the owner NAMED (`origin` by default; a path or an
//! unknown name is `remote_unknown`). Each call gets exactly one `tool_result`
//! under its call id, with every context key the call carried; a caller other
//! than the reasoning core is `read_only`, a program outside `exec_allow`
//! `not_allowed`; `ws_commit` answers the workspace NAME. Gap L-8 of the
//! coding proof: the first export after a pull into a fresh repository moves
//! HEAD onto the upstream and commits nothing; a later change is ONE commit.
//!
//! The SHIPPED `file-space` boots with its projection as in gh975 (machinery
//! copied, the file stands alone); every request carries `MESSAGE_DEFAULT_TTL`.
//! A six-file export is measured at the receiver, in the colony's own
//! `message_log`: no dead letter, nothing under `FLOOR`, and the door
//! `/files/tools -> /files/projection` restores the TTL (evidence line `gh980
//! export: deliveries=<n> min_ttl=<n>`). The remote lives beside `base_path` in
//! the test's temp dir (a push target under `base_path` is `remote_inside_base`,
//! `a_remote_under_base_path_is_refused`); no network, no provider. `run` starts programs with
//! `PATH=/usr/local/bin:/usr/bin:/bin`, where `python3` lives on the lanes.
//! Guarded like every template-reading test (GH #49).

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, override_params_on_disk};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long the colony must stay silent before the run calls it quiet.
const QUIET: Duration = Duration::from_millis(1500);

/// What a segment keeps back from the colony budget (OR-BD-11), as in gh929.
const RESERVE: u32 = 16;

/// What a restoring seam hands the delivery right behind it (post-decrement).
const BEHIND_A_SEAM: i64 = MESSAGE_DEFAULT_TTL as i64 - 1;

/// The lowest ttl any delivery of the export may carry: the reserve stays.
const FLOOR: i64 = RESERVE as i64;

/// Files of the measured export.
const SIX: usize = 6;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/file-space/config.json",
        "templates/file-space/tools/config.json",
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
/// `override_params` applied (GH #277, GH #140).
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

fn configs_under(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
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

/// The owner's side of the tree: every projection cell with a `base_path`
/// gets the run's directory and the write grant on exactly that path; `git`
/// names `origin`; `mat` permits `python3` when `exec` (else its
/// `exec_allow` stays the shipped `[]`); `schemas` puts the projection tools
/// on the menu. Summaries and embeddings stay off, no provider is asked.
fn owner_overrides(
    main: &std::path::Path,
    base: &std::path::Path,
    bare: &std::path::Path,
    exec: bool,
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
            // GH #980 M1: the repositories live outside `base_path`, granted
            // to the git cell alone -- `run` cannot write them.
            let gd = git_dirs(base);
            cfg["params"]["git_dir"] = json!(gd);
            // A remote beside `base_path` is the owner's grant to the git
            // cell too; one under it is covered by `base` (and refused).
            let mut grant = vec![base.to_path_buf(), gd];
            if !bare.starts_with(base) {
                grant.push(bare.parent().expect("a remote's directory").to_path_buf());
            }
            cfg["params"]["sandbox"]["filesystem"]["write"] = json!(grant);
        }
        if dir.ends_with("projection/mat") && exec {
            cfg["params"]["exec_allow"] = json!(["python3"]);
        }
        if dir.ends_with("files/schemas") {
            cfg["params"]["projection_tools"] = json!("1");
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

/// Where the owner keeps the git cell's repositories: beside `base_path`,
/// never under it (GH #980 M1).
fn git_dirs(base: &std::path::Path) -> PathBuf {
    base.parent()
        .expect("base_path has a parent")
        .join("git-dirs")
}

/// The repository of the git projection `view`, as the cell keeps it: under
/// `git_dir`, a bare directory whose work tree is `<base_path>/<view>`.
fn view_repo(base: &std::path::Path, view: &str) -> PathBuf {
    git_dirs(base).join(view)
}

fn build(
    td: &tempfile::TempDir,
    base: &std::path::Path,
    bare: &std::path::Path,
    exec: bool,
) -> usize {
    let main = td.path().join("main");
    copy_resolved(&repo("templates/file-space"), &main.join("files"), 0);
    // `answer` (a direct op) and `tool_result` (a tool call) reach the test;
    // `source_changed` (GH #944) and `source_described` (GH #947) leave the
    // space on every head move and are drained, else each is a `no_route`.
    let rim: Vec<Value> = [
        "answer",
        "tool_result",
        "derived",
        "model_refused",
        "source_changed",
        "source_described",
    ]
    .iter()
    .map(|lane| {
        let to = if matches!(*lane, "answer" | "tool_result") {
            "/sink"
        } else {
            "/park"
        };
        json!({"from": "./files", "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
    })
    .collect();
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": rim}}}),
    );
    owner_overrides(&main, base, bare, exec)
}

struct Run {
    h: ColonyHandle,
    sink: mpsc::Receiver<Message>,
    root: PathBuf,
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

/// One booted colony: the temp dir it lives in, `base_path`, and the bare
/// repository the git cell names `origin` (empty, branch `main`) -- beside
/// `base_path`, or under it for the one lock that wants it refused.
struct Lab {
    _td: tempfile::TempDir,
    base: PathBuf,
    bare: PathBuf,
    run: Run,
}

async fn start(exec: bool) -> Lab {
    start_with(exec, false).await
}

async fn start_with(exec: bool, remote_under_base: bool) -> Lab {
    let td = tempfile::TempDir::new().expect("a temp dir under TMPDIR");
    let base = td.path().join("projection");
    let bare = if remote_under_base {
        base.join("remote.git")
    } else {
        td.path().join("remotes").join("remote.git")
    };
    std::fs::create_dir_all(&base).expect("base_path exists before the cells spawn");
    std::fs::create_dir_all(git_dirs(&base)).expect("git_dir exists before the cells spawn");
    let bare_s = bare.to_str().unwrap();
    git(&base, &["init", "-q", "--bare", "-b", "main", bare_s]);
    let granted = build(&td, &base, &bare, exec);
    assert!(
        granted >= 1,
        "expected `git` with a base_path, got {granted}"
    );
    let run = boot(&td).await;
    Lab {
        _td: td,
        base,
        bare,
        run,
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
    /// Wait for the ONE sink message `want` picks; everything else is skipped.
    async fn wait(&mut self, what: &str, want: impl Fn(&Message) -> bool) -> Message {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            match tokio::time::timeout_at(deadline, self.sink.recv()).await {
                Ok(Some(m)) if want(&m) => return m,
                Ok(Some(_)) => continue,
                _ => panic!(
                    "{what}: expected an answer within {DEADLINE:?}, got none; dead letters: {:#?}",
                    dead_letters(&self.root)
                ),
            }
        }
    }

    /// One direct request at the hive path; the ONE answer for it (by `op_id`).
    async fn ask(
        &mut self,
        lane: &str,
        op: &str,
        file: Option<&str>,
        args: Value,
        ws: &str,
    ) -> Value {
        self.seq += 1;
        let op_id = format!("t980-{}", self.seq);
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
            .ttl(MESSAGE_DEFAULT_TTL)
            .build();
        self.h.send(msg).await;
        let m = self
            .wait(op, |m| m.headers.hop.get("op_id") == Some(&json!(op_id)))
            .await;
        let Body::Inline(v) = &m.body else {
            panic!("{op}: expected an inline answer, got a blob")
        };
        has(op, v, "op", json!(op));
        v.clone()
    }

    /// One tool call on `in_tool`, `context` plus `tool_caller` = `caller`;
    /// the ONE `tool_result` under its call id, and its text parsed.
    async fn tool(
        &mut self,
        caller: &str,
        name: &str,
        args: Value,
        context: Map<String, Value>,
    ) -> (Message, Value) {
        self.seq += 1;
        let id = format!("c980-{}", self.seq);
        let mut ctx = context;
        ctx.insert("tool_caller".to_string(), json!(caller));
        let text = meclaw_core::serde_json::to_string(&args).expect("arguments serialise");
        let msg = MessageBuilder::new(Path::new("/files"))
            .hop(map(
                json!({"route": "in_tool", "tool_name": name, "tool_call_id": id}),
            ))
            .context(ctx)
            .body(Body::Inline(json!({"messages": [
                {"origin": "assistant", "type": "tool_call", "id": id, "text": text}]})))
            .ttl(MESSAGE_DEFAULT_TTL)
            .build();
        self.h.send(msg).await;
        let m = self
            .wait(name, |m| {
                m.headers.hop.get("route") == Some(&json!("tool_result"))
                    && m.headers.hop.get("tool_call_id") == Some(&json!(id))
            })
            .await;
        let Body::Inline(b) = &m.body else {
            panic!("{name}: expected an inline tool_result, got a blob")
        };
        let turns = b["messages"].as_array().cloned().unwrap_or_default();
        assert!(
            turns.len() == 1 && turns[0]["id"] == json!(id),
            "{name}: expected one turn under the call id {id}, got {b}"
        );
        let text = turns[0]["text"].as_str().unwrap_or_default();
        let v: Value = meclaw_core::serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("{name}: expected the answer as JSON text ({e}), got {b}"));
        assert!(
            v.get("op_id").is_none() && v.get("caller").is_none(),
            "{name}: expected no op_id and no caller in a tool answer, got {v}"
        );
        (m, v)
    }

    /// A tool call as the reasoning core, no further context.
    async fn cogny(&mut self, name: &str, args: Value) -> (Message, Value) {
        self.tool("cogny", name, args, Map::new()).await
    }

    /// `ws_open` of `name` over `root`, which must go through.
    async fn open(&mut self, name: &str, root: &str) -> Value {
        let args = json!({"name": name, "root": root});
        ok(self.ask("in_ws", "ws_open", None, args, "").await)
    }

    /// `ws_commit` of workspace `ws`, which must go through.
    async fn commit(&mut self, ws: &str) -> Value {
        let args = json!({"note": ws});
        ok(self.ask("in_ws", "ws_commit", None, args, ws).await)
    }

    /// Seed the main line under `/proj` through workspace `ws`.
    async fn seed(&mut self, ws: &str, files: &[(&str, &str)]) {
        self.open(ws, "/proj").await;
        for (path, text) in files {
            let args = json!({"path": path, "text": text});
            ok(self.ask("in_write", "create", None, args, ws).await);
        }
        self.commit(ws).await;
    }
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], json!(true), "expected ok: true, got {v}");
    v
}

/// `v[key] == want`, else a panic naming what was expected and what came.
fn has(what: &str, v: &Value, key: &str, want: Value) {
    assert_eq!(v[key], want, "{what}: expected `{key}` = {want}, got {v}");
}

/// A tool answer that went through: `ok`, no `hop.error_code`.
fn tool_ok(name: &str, (m, v): (Message, Value)) -> Value {
    has(name, &v, "ok", json!(true));
    assert!(
        m.headers.hop.get("error_code").is_none(),
        "{name}: expected no hop.error_code on a success, got {:?}",
        m.headers.hop
    );
    v
}

/// A tool answer refused with `code`, in the text and on the hop.
fn tool_refused(name: &str, (m, v): (Message, Value), code: &str) -> Value {
    has(name, &v, "ok", json!(false));
    has(name, &v["error"], "code", json!(code));
    assert_eq!(
        m.headers.hop.get("error_code"),
        Some(&json!(code)),
        "{name}: expected hop.error_code {code}, got {:?}",
        m.headers.hop
    );
    v
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    let raw = if out.status.success() {
        out.stdout
    } else {
        out.stderr
    };
    let text = String::from_utf8_lossy(&raw).trim().to_string();
    (out.status.success(), text)
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let (good, text) = git_out(dir, args);
    assert!(
        good,
        "expected git {args:?} in {} to succeed: {text}",
        dir.display()
    );
    text
}

/// One row of the colony's `message_log`.
struct Delivery {
    row: i64,
    id: String,
    parent: Option<String>,
    from: String,
    to: String,
    route: String,
    op_id: String,
    ttl: i64,
}

impl Delivery {
    fn say(&self) -> String {
        format!(
            "ttl={:3} {} -> {} [{}]",
            self.ttl, self.from, self.to, self.route
        )
    }
}

fn deliveries(root: &std::path::Path) -> Vec<Delivery> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT rowid, id, parent_message_id, from_path, to_path, headers, ttl \
             FROM message_log ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        let headers: String = r.get(5)?;
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        Ok(Delivery {
            row: r.get(0)?,
            id: r.get(1)?,
            parent: r.get(2)?,
            from: r.get(3)?,
            to: r.get(4)?,
            route: h["hop"]["route"].as_str().unwrap_or_default().to_string(),
            op_id: h["hop"]["op_id"].as_str().unwrap_or_default().to_string(),
            ttl: r.get(6)?,
        })
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

fn last_row(root: &std::path::Path) -> i64 {
    deliveries(root).last().map_or(0, |d| d.row)
}

/// Wait until the message log has not grown for `QUIET`.
async fn quiet(root: &std::path::Path) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut last = deliveries(root).len();
    let mut since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = deliveries(root).len();
        if now != last {
            last = now;
            since = tokio::time::Instant::now();
        } else if since.elapsed() >= QUIET {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected the colony to go quiet within {DEADLINE:?}, it did not"
        );
    }
}

/// The parent chain of `d`, newest first, at most 80 links.
fn chain(log: &[Delivery], d: &Delivery) -> Vec<String> {
    let by_id: HashMap<&str, &Delivery> = log.iter().map(|x| (x.id.as_str(), x)).collect();
    let mut out = vec![d.say()];
    let mut at = d;
    while let Some(p) = at.parent.as_deref().and_then(|p| by_id.get(p)) {
        if out.len() >= 80 {
            out.push("...".to_string());
            break;
        }
        out.push(p.say());
        at = p;
    }
    out
}

const OPS_PY: &str = "def add(a, b):\n    return a + b\n";
const TEST_OPS_PY: &str = "import unittest\n\nfrom ops import add\n\n\n\
class T(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(add(2, 3), 5)\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_runs_its_tests_in_a_workspace() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let run = &mut lab.run;
    let files = [("/proj/ops.py", OPS_PY), ("/proj/test_ops.py", TEST_OPS_PY)];
    run.seed("seed", &files).await;
    run.open("w1", "/proj").await;

    // A failing program first: `-c` imports nothing local, so it leaves no
    // byte-code in the directory the next run lays out again.
    let fail = json!(["python3", "-c", "import sys; sys.exit(3)"]);
    let what = "file_ws_exec (exit 3)";
    let call = json!({"ws": "w1", "argv": fail});
    let v = tool_ok(what, run.cogny("file_ws_exec", call).await);
    has(what, &v, "op", json!("ws_exec"));
    has(what, &v, "exit", json!(3));
    has(what, &v, "argv", fail);

    // The tests pass.
    let tests = json!(["python3", "-m", "unittest", "test_ops"]);
    let what = "file_ws_exec (unittest)";
    let call = json!({"ws": "w1", "argv": tests});
    let v = tool_ok(what, run.cogny("file_ws_exec", call.clone()).await);
    has(what, &v, "exit", json!(0));
    has(what, &v, "argv", tests.clone());
    has(what, &v, "ws", json!("w1"));
    assert!(
        v["err_tail"].as_str().is_some_and(|t| t.contains("OK")),
        "{what}: expected unittest's OK in err_tail, got {v}"
    );
    for key in ["run", "out_tail", "adopted", "proposals"] {
        assert!(v.get(key).is_some(), "{what}: expected `{key}`, got {v}");
    }

    // A surface that only reads: refused at `./tools`, no request leaves it
    // (a request would carry the call id in its `op_id`).
    let answer = run
        .tool("talky", "file_ws_exec", call.clone(), Map::new())
        .await;
    tool_refused("file_ws_exec as talky", answer, "read_only");
    let id = format!("c980-{}", run.seq);
    let leaked: Vec<String> = deliveries(&run.root)
        .iter()
        .filter(|d| d.op_id.ends_with(&format!(":{id}")))
        .map(Delivery::say)
        .collect();
    assert!(
        leaked.is_empty(),
        "expected a read_only call to reach no projection cell, got {leaked:#?}"
    );
    lab.run.h.shutdown().await;

    // A colony whose owner permitted no program.
    let mut closed = start(false).await;
    closed.run.open("w1", "/proj").await;
    let answer = closed.run.cogny("file_ws_exec", call).await;
    tool_refused("file_ws_exec without exec_allow", answer, "not_allowed");
    closed.run.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn export_and_push_are_one_call_each() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let files = [
        ("/proj/ops.py", OPS_PY),
        ("/proj/test_ops.py", TEST_OPS_PY),
        ("/proj/docs/readme.md", "# ops\n"),
    ];
    lab.run.seed("seed", &files).await;

    // Two tool calls, nothing between them: the test opens no view.
    let call = json!({"root": "/proj", "note": "P-1: add ops\n\nbody"});
    let ex = tool_ok("export", lab.run.cogny("file_ws_export", call).await);
    has("export", &ex, "op", json!("ws_export_git"));
    has("export", &ex, "subject", json!("P-1: add ops"));
    // OR-LP-65: export and push echo the root they acted on, so a record of
    // the pair binds to the folder of the task.
    has("export", &ex, "root", json!("/proj"));
    has("export", &ex, "files", json!(files.len()));
    let commit = ex["commit"].as_str().expect("an export names its commit");
    let push = json!({"root": "/proj"});
    let push = tool_ok("push", lab.run.cogny("file_ws_push", push).await);
    has("push", &push, "op", json!("ws_push"));
    has("push", &push, "remote", json!("origin"));
    has("push", &push, "branch", json!("main"));
    has("push", &push, "commit", json!(commit));
    has("push", &push, "root", json!("/proj"));

    let remote_main = git(&lab.bare, &["rev-parse", "main"]);
    assert_eq!(remote_main, commit, "expected the remote's main at export");
    let subject = git(&lab.bare, &["log", "-1", "--format=%s", "main"]);
    assert_eq!(subject, "P-1: add ops", "expected the remote's subject");
    let view_head = git(&view_repo(&lab.base, "git.proj"), &["rev-parse", "HEAD"]);
    assert_eq!(
        view_head, commit,
        "expected <git_dir>/git.proj at the export"
    );
    // GH #980 M1: nothing of the repository lies in the work tree.
    assert!(
        !lab.base.join("git.proj").join(".git").exists(),
        "expected no .git under base_path"
    );

    // The view was discarded before the export answered: its name is free.
    lab.run.open("git.proj", "/proj").await;
    lab.run.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_names_only_configured_remotes() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let other = lab.base.join("other.git");
    let other_s = other.to_str().unwrap();
    git(&lab.base, &["init", "-q", "--bare", "-b", "main", other_s]);
    lab.run.seed("seed", &[("/proj/ops.py", OPS_PY)]).await;
    // An export exists, so a push would have something to send.
    let call = json!({"root": "/proj", "note": "export"});
    tool_ok("export", lab.run.cogny("file_ws_export", call).await);

    let call = json!({"root": "/proj", "remote": other_s});
    let answer = lab.run.cogny("file_ws_push", call).await;
    tool_refused("file_ws_push to a path", answer, "remote_unknown");
    let (has_main, said) = git_out(&other, &["rev-parse", "--verify", "-q", "main"]);
    assert!(
        !has_main,
        "expected the unnamed repository to receive nothing, got main = {said}"
    );

    let call = json!({"root": "/proj", "remote": "nope"});
    let answer = lab.run.cogny("file_ws_push", call).await;
    tool_refused("file_ws_push to an unknown name", answer, "remote_unknown");
    lab.run.h.shutdown().await;
}

/// A planted receive hook: every hook a local push runs on the receiving side
/// touches the marker its argument names.
const PLANT_PY: &str = r##"
import json, os, sys
plan = json.loads(sys.argv[1])
for name in ("pre-receive", "update", "post-receive", "proc-receive"):
    path = os.path.join(plan["hooks"], name)
    with open(path, "w") as f:
        f.write("#!/bin/sh\ntouch '%s'\n" % plan["marker"])
    os.chmod(path, 0o755)
print(json.dumps({"planted": True}))
"##;

/// Welle Loop NV2 (the class of GH #980 M1): `run` may write all of
/// `base_path`, so an owner's remote that is a local path under it is a
/// repository the model's program can rewrite -- a receive hook planted there
/// ran in the git cell's process on the next `file_ws_push`. Such a remote is
/// refused before git runs (`remote_inside_base`): the push sends nothing and
/// the planted hook never runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_under_base_path_is_refused() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start_with(true, true).await;
    assert!(
        lab.bare.starts_with(&lab.base),
        "the lock wants origin under base_path"
    );
    let marker = lab.base.join("hooked");
    lab.run.seed("seed", &[("/proj/ops.py", OPS_PY)]).await;
    let call = json!({"root": "/proj", "note": "export"});
    tool_ok("export", lab.run.cogny("file_ws_export", call).await);

    // The model's program plants the hooks into the configured remote.
    lab.run.open("w1", "/proj").await;
    let plan = json!({"hooks": lab.bare.join("hooks"), "marker": marker});
    let argv = json!(["python3", "-c", PLANT_PY, plan.to_string()]);
    let v = tool_ok(
        "the planting program",
        lab.run
            .cogny("file_ws_exec", json!({"ws": "w1", "argv": argv}))
            .await,
    );
    has("the planting program", &v, "exit", json!(0));
    assert!(
        lab.bare.join("hooks").join("pre-receive").exists(),
        "expected the program to reach the remote under base_path, got {v}"
    );

    let answer = lab
        .run
        .cogny("file_ws_push", json!({"root": "/proj"}))
        .await;
    tool_refused(
        "file_ws_push to origin under base_path",
        answer,
        "remote_inside_base",
    );
    assert!(
        !marker.exists(),
        "expected no planted hook to run, found {}",
        marker.display()
    );
    let (has_main, said) = git_out(&lab.bare, &["rev-parse", "--verify", "-q", "main"]);
    assert!(
        !has_main,
        "expected the remote under base_path to receive nothing, got main = {said}"
    );
    lab.run.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn export_after_pull_commits_nothing() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const N: usize = 3;
    let mut lab = start(true).await;
    // The source: N files on `origin`'s main, one commit.
    let src = lab.base.join("source");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    for i in 0..N {
        std::fs::write(src.join(format!("f{i}.md")), format!("file {i}\n")).unwrap();
    }
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "source"]);
    git(&src, &["push", "-q", lab.bare.to_str().unwrap(), "main"]);
    let upstream = git(&lab.bare, &["rev-parse", "main"]);
    let view = view_repo(&lab.base, "git.proj");

    // Pull into the view's own name, commit it to the main line.
    let run = &mut lab.run;
    run.open("git.proj", "/proj").await;
    let pull_args = json!({"remote": "origin", "branch": "main"});
    let mut pull = run
        .ask("in_ws", "ws_pull", None, pull_args.clone(), "git.proj")
        .await;
    if pull["error"]["code"] == json!("not_materialized") {
        let mat = run.ask("in_ws", "ws_materialize", None, json!({}), "git.proj");
        ok(mat.await);
        pull = run
            .ask("in_ws", "ws_pull", None, pull_args, "git.proj")
            .await;
    }
    has("ws_pull", &ok(pull), "created", json!(N));
    run.commit("git.proj").await;

    // L-8: the export finds the upstream's tree and commits nothing.
    let call = json!({"root": "/proj", "note": "nothing"});
    let what = "file_ws_export after a pull";
    let answer = run.cogny("file_ws_export", call).await;
    let v = tool_refused(what, answer, "nothing_to_commit");
    has(what, &v, "commit", json!(upstream));
    has(what, &v, "root", json!("/proj"));
    let head = git(&view, &["rev-parse", "HEAD"]);
    assert_eq!(head, upstream, "expected the view's HEAD on the upstream");
    let remote_main = git(&lab.bare, &["rev-parse", "main"]);
    assert_eq!(remote_main, upstream, "expected remote main unchanged");

    // Counter-proof: one file changes on the main line.
    run.open("edit", "/proj").await;
    let tree = ok(run.ask("in_ws", "ws_tree", None, json!({}), "edit").await);
    let fs = tree["files"].as_array().cloned().unwrap_or_default();
    let f0 = fs.iter().find(|f| f["path"] == json!("/proj/f0.md"));
    let f0 = f0.unwrap_or_else(|| panic!("expected /proj/f0.md in the tree, got {tree}"));
    let file = f0["file"].as_str().expect("a file address");
    let version = f0["version"].as_str().expect("a version");
    let args = json!({"text": "file 0, changed\n", "base": &version[..version.len().min(12)]});
    let write = run.ask("in_write", "overwrite", Some(file), args, "edit");
    ok(write.await);
    run.commit("edit").await;

    let call = json!({"root": "/proj", "note": "change f0"});
    let v = tool_ok("re-export", run.cogny("file_ws_export", call).await);
    let commit = v["commit"].as_str().expect("a commit").to_string();
    assert_ne!(commit, upstream, "expected a new commit, got {v}");
    let parents = git(&view, &["rev-list", "--parents", "-n", "1", &commit]);
    let want = format!("{commit} {upstream}");
    assert_eq!(parents, want, "expected exactly one parent, the upstream");
    let push = json!({"root": "/proj"});
    let v = tool_ok("push after a change", run.cogny("file_ws_push", push).await);
    has("push after a change", &v, "commit", json!(commit));
    let remote_main = git(&lab.bare, &["rev-parse", "main"]);
    assert_eq!(remote_main, commit, "expected remote main at the new one");
    let count = git(&lab.bare, &["rev-list", "--count", "main"]);
    assert_eq!(count, "2", "expected the source commit plus one");
    lab.run.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_file_export_fits_ttl_64() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let names: Vec<(String, String)> = (0..SIX)
        .map(|i| (format!("/proj/f{i}.md"), format!("file {i}\n")))
        .collect();
    let files: Vec<(&str, &str)> = names.iter().map(|(p, t)| (&p[..], &t[..])).collect();
    lab.run.seed("seed", &files).await;

    quiet(&lab.run.root).await;
    let before = last_row(&lab.run.root);
    let call = json!({"root": "/proj", "note": "six"});
    let ex = tool_ok("export", lab.run.cogny("file_ws_export", call).await);
    has("export", &ex, "files", json!(SIX));
    quiet(&lab.run.root).await;
    let log = deliveries(&lab.run.root);
    let dead = dead_letters(&lab.run.root);
    lab.run.h.shutdown().await;

    let after: Vec<&Delivery> = log.iter().filter(|d| d.row > before).collect();
    let worst = after
        .iter()
        .min_by_key(|d| d.ttl)
        .expect("expected the export to leave deliveries in the message log");
    eprintln!(
        "gh980 export: deliveries={} min_ttl={}",
        after.len(),
        worst.ttl
    );
    let expired: Vec<_> = dead.iter().filter(|d| d.0 == "ttl_expired").collect();
    assert!(expired.is_empty(), "expected no ttl_expired: {expired:#?}");
    assert!(dead.is_empty(), "expected no dead letter, got {dead:#?}");
    assert!(
        worst.ttl >= FLOOR,
        "expected every delivery of the export at ttl >= {FLOOR}, got {}; its chain:\n{}",
        worst.ttl,
        chain(&log, worst).join("\n")
    );
    let doors: Vec<&&Delivery> = after
        .iter()
        .filter(|d| d.from == "/files/tools" && d.to == "/files/projection")
        .collect();
    assert!(
        doors.iter().any(|d| d.ttl == BEHIND_A_SEAM),
        "expected a delivery /files/tools -> /files/projection at {BEHIND_A_SEAM} (the door), \
         got {:#?}",
        doors.iter().map(|d| d.say()).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_tools_keep_the_call_context() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let run = &mut lab.run;
    run.seed("seed", &[("/proj/ops.py", OPS_PY)]).await;

    // `ws_open` answers the id; the commit answers the name.
    let opened = run.open("wc", "/proj").await;
    has("ws_open", &opened, "name", json!("wc"));
    assert!(
        opened["ws"].as_str().is_some_and(|id| id != "wc"),
        "expected ws_open's `ws` to be an id other than the name, got {opened}"
    );
    let args = json!({"path": "/proj/wc.txt", "text": "written in wc\n"});
    ok(run.ask("in_write", "create", None, args, "wc").await);

    let ctx = map(json!({"tool_caller": "cogny", "run_id": "r-ctx-1", "other_key": "x"}));
    let calls = [
        (
            "file_ws_exec",
            json!({"ws": "wc", "argv": ["python3", "-c", "pass"]}),
        ),
        ("file_ws_commit", json!({"ws": "wc", "note": "n"})),
        ("file_ws_export", json!({"root": "/proj", "note": "ctx"})),
        ("file_ws_push", json!({"root": "/proj"})),
    ];
    for (name, args) in calls {
        let (m, v) = run.tool("cogny", name, args, ctx.clone()).await;
        let got = &m.headers.context;
        for (key, want) in [("run_id", "r-ctx-1"), ("other_key", "x")] {
            assert_eq!(
                got.get(key),
                Some(&json!(want)),
                "{name}: expected context {key} = {want} on the tool_result, got {got:?}"
            );
        }
        let v = tool_ok(name, (m, v));
        if name == "file_ws_commit" {
            has(name, &v, "ws", json!("wc"));
        }
    }
    lab.run.h.shutdown().await;
}

/// What the program of `exec_cannot_steer_the_view` does with each path it is
/// handed: append a filter and an `insteadOf` to `<path>/config`, put the
/// filter on every file in `<path>/info/attributes`; one word per path back.
const STEER_PY: &str = r#"
import json, os, sys
plan = json.loads(sys.argv[1])
done = {}
for repo in plan["repos"]:
    try:
        os.makedirs(os.path.join(repo, "info"), exist_ok=True)
        with open(os.path.join(repo, "config"), "a") as f:
            f.write('[url "%s"]\n\tinsteadOf = %s\n[filter "steer"]\n\tclean = touch %s && cat\n'
                    % (plan["evil"], plan["origin"], plan["marker"]))
        with open(os.path.join(repo, "info", "attributes"), "w") as f:
            f.write("* filter=steer\n")
        done[repo] = "written"
    except OSError as e:
        done[repo] = type(e).__name__
print(json.dumps(done))
"#;

/// GH #980 M1 (review): `run` may write all of `base_path`, and the program of
/// a `file_ws_exec` is the model's. The repository of a git projection must
/// lie outside it: a `filter.<x>.clean` in its config ran code in the git
/// cell on the next export, an `url.<r>.insteadOf` sent the next push to a
/// repository nobody configured. The program tries the view's old in-tree
/// `.git` and -- where Landlock holds the sandbox -- the owner's `git_dir`;
/// the next export and push behave as if it had never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_cannot_steer_the_view() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let evil = lab.base.join("evil.git");
    let evil_s = evil.to_str().unwrap().to_string();
    git(&lab.base, &["init", "-q", "--bare", "-b", "main", &evil_s]);
    let marker = lab.base.join("steered");
    lab.run.seed("seed", &[("/proj/ops.py", OPS_PY)]).await;
    // The first export lays the view's repository down.
    let call = json!({"root": "/proj", "note": "first"});
    tool_ok("first export", lab.run.cogny("file_ws_export", call).await);

    let mut repos = vec![lab.base.join("git.proj").join(".git")];
    let landlock = meclaw_cells::sandbox::landlock_abi().is_some();
    if landlock {
        repos.push(git_dirs(&lab.base).join("git.proj"));
    }
    let plan = json!({"repos": repos, "evil": evil_s, "origin": lab.bare,
                      "marker": marker});
    lab.run.open("w1", "/proj").await;
    let argv = json!(["python3", "-c", STEER_PY, plan.to_string()]);
    let call = json!({"ws": "w1", "argv": argv});
    let v = tool_ok(
        "the steering program",
        lab.run.cogny("file_ws_exec", call).await,
    );
    has("the steering program", &v, "exit", json!(0));
    let said: Value = meclaw_core::serde_json::from_str(v["out_tail"].as_str().unwrap_or("{}"))
        .unwrap_or_else(|e| panic!("expected the program's JSON ({e}), got {v}"));
    if landlock {
        let gd = git_dirs(&lab.base).join("git.proj");
        assert_ne!(
            said[gd.to_str().unwrap()],
            json!("written"),
            "expected the kernel to refuse a write into git_dir, got {said}"
        );
    }

    // A real change, then export and push as the model sends them.
    lab.run.seed("seed2", &[("/proj/more.md", "more\n")]).await;
    let call = json!({"root": "/proj", "note": "after the program"});
    let ex = tool_ok("export", lab.run.cogny("file_ws_export", call).await);
    let commit = ex["commit"].as_str().expect("an export names its commit");
    let push = json!({"root": "/proj"});
    let push = tool_ok("push", lab.run.cogny("file_ws_push", push).await);
    has("push", &push, "commit", json!(commit));

    assert!(
        !marker.exists(),
        "expected no filter of the program to run in the git cell, found {}",
        marker.display()
    );
    let (steered, said) = git_out(&evil, &["rev-parse", "--verify", "-q", "main"]);
    assert!(
        !steered,
        "expected the unconfigured repository to receive nothing, got main = {said}"
    );
    let remote_main = git(&lab.bare, &["rev-parse", "main"]);
    assert_eq!(remote_main, commit, "expected the push in `origin`");
    lab.run.h.shutdown().await;
}

/// GH #980 m2 (review): the view of a root belongs to the git cell. A model
/// cannot open a workspace named `git` or `git.<…>` (`reserved_name`), and an
/// export whose view is held open says WHICH workspace is in the way
/// (`ws_exists` with `view`), so the owner knows what to discard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_is_the_projections_own() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut lab = start(true).await;
    let run = &mut lab.run;
    run.seed("seed", &[("/proj/ops.py", OPS_PY)]).await;
    for name in ["git.proj", "git"] {
        let call = json!({"name": name, "root": "/proj"});
        let what = format!("file_ws_open {name}");
        tool_refused(
            &what,
            run.cogny("file_ws_open", call).await,
            "reserved_name",
        );
    }
    let call = json!({"name": "gitty", "root": "/proj"});
    tool_ok("file_ws_open gitty", run.cogny("file_ws_open", call).await);

    // The owner's door (no caller) may open it -- the import of NV does.
    run.open("git.proj", "/proj").await;
    let call = json!({"root": "/proj", "note": "blocked"});
    let what = "file_ws_export with its view open";
    let v = tool_refused(what, run.cogny("file_ws_export", call).await, "ws_exists");
    has(what, &v["error"], "view", json!("git.proj"));
    has(what, &v, "root", json!("/proj"));
    ok(run
        .ask("in_ws", "ws_discard", None, json!({}), "git.proj")
        .await);
    let call = json!({"root": "/proj", "note": "free"});
    tool_ok(
        "file_ws_export after the discard",
        run.cogny("file_ws_export", call).await,
    );
    lab.run.h.shutdown().await;
}
