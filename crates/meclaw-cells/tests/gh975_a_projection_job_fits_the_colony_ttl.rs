//! GH #975 -- a projection job fits the colony TTL (L-7 of the coding proof).
//!
//! Every file step of a projection job is a door: each request the child hive
//! `./projection` sends into its space (`./projection -> ./ws`, `./read`,
//! `./write`) restores the colony TTL, the seam table of
//! `docs/meclaw-overview.md` § Edge model, row "Door". The job's file list
//! bounds its steps (the git cell refuses a tree past `import_max_files`).
//! Measured before the fix: a `ws_pull` of six files ran 138 hops in the
//! coding runner, which had to send at TTL 400 against the colony's 64.
//!
//! This file boots the SHIPPED `file-space` with its projection as gh906 does
//! (the machinery is copied, the file stands alone) and walks: a source of
//! six text files imported into workspace `A` and committed; workspace `B`
//! exported and pushed into a bare remote; all six files changed in a clone
//! and pushed; `ws_pull` in `B` brings six modifications in. Every request
//! carries `MESSAGE_DEFAULT_TTL`, nothing more.
//!
//! Measured at the receiver, in the colony's own `message_log`, over every
//! delivery logged after the pull was sent: no dead letter at all (a
//! `ttl_expired` among them is the old failure), the lowest ttl is at least
//! `FLOOR` = 63 - 48 = 15 -- every segment starts at 63 behind a root or a
//! seam, so that is "no segment spends more than 48 routing decisions" -- and
//! at least six deliveries `/files/projection -> /files/write` carry 63. The
//! door is measured on each of its three edges, over the whole job run
//! (import, commit, export, push, pull): every delivery `/files/projection ->
//! /files/ws`, `-> /files/read` and `-> /files/write` arrives with 63, and each
//! edge carries at least one. Without the three `restore_ttl` modifiers the
//! pull's file steps share one chain from the root, so it dies of
//! `ttl_expired` (no answer, a dead letter) or its ttl sinks under the floor.
//! One line `gh975 pull: deliveries=<n> min_ttl=<n> doors=<n>` is the
//! evidence.
//!
//! The source repository and the remote live under `base_path`, everything
//! under the test's temporary directory; no network, no provider. Guarded
//! like every template-reading test (GH #49).

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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long the colony must stay silent before the run calls it quiet.
const QUIET: Duration = Duration::from_millis(1500);

/// What a segment keeps back from the colony budget (OR-BD-11), as in gh929.
const RESERVE: u32 = 16;

/// The most routing decisions one segment may spend.
const SEGMENT_MAX: i64 = (MESSAGE_DEFAULT_TTL - RESERVE) as i64;

/// What a restoring seam hands the delivery right behind it, as the message
/// log records it: the colony budget less the routing decision that crossed
/// the seam (the log stores the post-decrement ttl).
const BEHIND_A_SEAM: i64 = MESSAGE_DEFAULT_TTL as i64 - 1;

/// The lowest ttl a delivery may carry when no segment spent more than
/// `SEGMENT_MAX`: every root and every seam starts at `BEHIND_A_SEAM`.
const FLOOR: i64 = BEHIND_A_SEAM - SEGMENT_MAX;

/// Files in the source repository, all changed outside before the pull.
const FILES: usize = 6;

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
    // `source_changed` (GH #944) leaves the space on every head move, whoever
    // wrote, and `source_described` (GH #947) after every stored summary of a
    // living head: undrained, each import would be a `no_route` dead letter.
    let rim: Vec<Value> = [
        "answer",
        "derived",
        "model_refused",
        "source_changed",
        "source_described",
    ]
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
        let op_id = format!("t975-{}", self.seq);
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
            // GH #975: the run holds the colony TTL; every file step of a
            // projection job is a door, so a job of any size fits it.
            .ttl(MESSAGE_DEFAULT_TTL)
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

/// One row of the colony's `message_log`: insertion order, id, parent,
/// sender and target as logged, the hop's route and the ttl.
struct Delivery {
    row: i64,
    id: String,
    parent: Option<String>,
    from: String,
    to: String,
    route: String,
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
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(row, id, parent, from, to, headers, ttl)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        let route = h["hop"]["route"].as_str().unwrap_or_default().to_string();
        Delivery {
            row,
            id,
            parent,
            from,
            to,
            route,
            ttl,
        }
    })
    .collect()
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
            "the colony did not go quiet within {DEADLINE:?}"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_projection_job_of_six_files_fits_ttl_64() {
    if !shipped() {
        eprintln!("the projection did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("a temp dir under TMPDIR");
    let base = td.path().join("projection");
    let src = base.join("source");
    let bare = base.join("remote.git");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    for i in 0..FILES {
        std::fs::write(src.join(format!("f{i}.md")), format!("file {i}\n")).unwrap();
    }
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

    // Import into A, commit.
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
    assert_eq!(imp["created"], json!(FILES), "{imp}");
    ok(run
        .ask("in_ws", "ws_commit", None, json!({"note": "import"}), "A")
        .await);

    // Export B, push into the empty remote.
    ok(run
        .ask(
            "in_ws",
            "ws_open",
            None,
            json!({"name": "B", "root": "/"}),
            "",
        )
        .await);
    let ex = ok(run
        .ask(
            "in_ws",
            "ws_export_git",
            None,
            json!({"note": "export"}),
            "B",
        )
        .await);
    assert_eq!(ex["files"], json!(FILES), "{ex}");
    ok(run
        .ask(
            "in_ws",
            "ws_push",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await);

    // Every file changes outside.
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
    for i in 0..FILES {
        std::fs::write(
            clone.join(format!("f{i}.md")),
            format!("file {i}, changed outside\n"),
        )
        .unwrap();
    }
    git(&clone, &["commit", "-qam", "outside"]);
    git(&clone, &["push", "-q", "origin", "main"]);

    // The pull, measured from its first delivery on.
    quiet(&run.root).await;
    let before = deliveries(&run.root).last().map_or(0, |d| d.row);
    let pull = ok(run
        .ask(
            "in_ws",
            "ws_pull",
            None,
            json!({"remote": "origin", "branch": "main"}),
            "B",
        )
        .await);
    assert_eq!(pull["modified"], json!(FILES), "{pull}");
    quiet(&run.root).await;
    let log = deliveries(&run.root);
    let dead = dead_letters(&run.root);
    run.h.shutdown().await;

    let after: Vec<&Delivery> = log.iter().filter(|d| d.row > before).collect();
    let worst = after
        .iter()
        .min_by_key(|d| d.ttl)
        .expect("the pull left deliveries in the message log");
    let doors = after
        .iter()
        .filter(|d| {
            d.from == "/files/projection" && d.to == "/files/write" && d.ttl == BEHIND_A_SEAM
        })
        .count();
    eprintln!(
        "gh975 pull: deliveries={} min_ttl={} doors={doors}",
        after.len(),
        worst.ttl
    );
    // The door on each of its three edges, over the whole run: every file step
    // the projection sends into its space restores the TTL, not only `write`.
    for space in ["/files/ws", "/files/read", "/files/write"] {
        let steps: Vec<&Delivery> = log
            .iter()
            .filter(|d| d.from == "/files/projection" && d.to == space)
            .collect();
        let short: Vec<String> = steps
            .iter()
            .filter(|d| d.ttl != BEHIND_A_SEAM)
            .map(|d| format!("row {} ttl {}", d.row, d.ttl))
            .collect();
        eprintln!(
            "gh975 door /files/projection -> {space}: steps={} short={}",
            steps.len(),
            short.len()
        );
        assert!(
            !steps.is_empty(),
            "the job sent no file step /files/projection -> {space}; the door on that edge is unmeasured"
        );
        assert!(
            short.is_empty(),
            "file steps /files/projection -> {space} arrived without the restored \
             {BEHIND_A_SEAM}: {short:#?}"
        );
    }
    let expired: Vec<_> = dead.iter().filter(|d| d.0 == "ttl_expired").collect();
    assert!(
        expired.is_empty(),
        "a file step ran out of TTL: {expired:#?}"
    );
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
    assert!(
        worst.ttl >= FLOOR,
        "a segment of the pull spends more than {SEGMENT_MAX} routing decisions \
         (lowest ttl {} < {FLOOR}); its chain:\n{}",
        worst.ttl,
        chain(&log, worst).join("\n")
    );
    assert!(
        doors >= FILES,
        "{doors} file steps /files/projection -> /files/write arrived with {BEHIND_A_SEAM}, \
         not at least {FILES}"
    );
}
