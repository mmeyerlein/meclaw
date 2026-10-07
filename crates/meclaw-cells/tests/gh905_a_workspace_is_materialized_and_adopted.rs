//! GH #905 — the seam lock of the projection: a real colony, because the sandbox
//! plays along (B2-README § 3).
//!
//! ```text
//! /world                         timer (keeps the colony connected)
//! /host                          ref marker -> fs-host (a test template)
//!   /host/files                    ref -> file-space (the shipped one, its model
//!                                  lane pointed at nothing)
//!     /host/files/projection         ref -> projection (the shipped one)
//! ```
//!
//! The owner's knobs reach `mat` and `run` through TWO refs, path-keyed on the
//! marker as `files/projection/mat` -- the shape a member will carry
//! (`member -> files -> projection/mat`, OR-FJ-G9). `base_path` and the sandbox's
//! write grant travel in that one override; the template grants no write at all.
//!
//! The story: a workspace with three files is laid out; a permitted "formatter"
//! (a `python3 -c` one-liner, `auto_adopt` = `["python3"]` here) changes `a.py`
//! and the change is a new working version at once, no proposal; a "generator"
//! writes `gen.txt`, which is a proposal (OR-FJ-G3) and born in the workspace
//! after `ws_adopt`; a program not in `exec_allow` is refused and touches
//! nothing; after a `replace` the next `ws_materialize` writes exactly one file;
//! a program that writes outside `base_path` fails at the kernel and leaves no
//! file there. Everything is read at the receiver: answers leave the colony at
//! the root, files are read on disk, the manifest where the projection's store
//! took it.

use std::sync::Arc;
use std::time::Duration;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::sandbox;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::api_dto::MessageLogFilter;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use tokio::sync::{mpsc, oneshot};

/// Generous on purpose (30 s convention, doubled for a program run under load).
const MARKER: Duration = Duration::from_secs(60);
const MARK: &str = "fs_out";
const HOST_MARKER: &str = "GH905_HOST_MARKER";
const SPACE: &str = "/host/files";
const STORE: &str = "/host/files/projection/store";
const NEEDED: [&str; 2] = ["templates/file-space", "templates/projection"];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("dirs");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("json"),
    )
    .expect("write");
}

fn patch_json(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    write_json(p, &v);
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("dirs");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// The guards of this lock, each a visible line when it skips (the sandbox
/// isolation tests' form: a missing capability is not a red run).
fn can_run() -> bool {
    if !NEEDED
        .iter()
        .all(|t| repo(t).join("template.json").is_file())
    {
        eprintln!("SKIPPED: the template library does not travel in this tree");
        return false;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIPPED: no python3");
        return false;
    }
    if sandbox::landlock_abi().is_none() {
        eprintln!("SKIPPED: no Landlock on this kernel (needs Linux 5.13+)");
        return false;
    }
    if !sandbox::network_isolation_supported() {
        eprintln!("SKIPPED: unprivileged network namespaces unavailable on this host");
        return false;
    }
    true
}

struct Colony {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    egress: mpsc::Receiver<Message>,
    n: u64,
}

/// The sandbox block the owner writes: the shipped one plus the write grant.
fn owner_sandbox(base: &std::path::Path) -> Value {
    json!({"trust": "restricted", "network": "deny",
           "filesystem": {"runtime": true, "write": [base.to_str().unwrap()]}})
}

async fn boot(base: &std::path::Path) -> Colony {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    write_json(&root.join("colony.json"), &json!({"schema_version": 1}));
    let mut mark = meclaw_core::serde_json::Map::new();
    mark.insert(MARK.to_string(), json!("'1'"));
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./world", "to": "./host"},
            {"from": "./host", "to": ".", "modifier": {"set_context": mark}}
        ]}}}),
    );
    write_json(
        &root.join("main/world/config.json"),
        &json!({"cell": {"type": "timer", "timeout": -1},
                "params": {"query_timeout_ms": 5000},
                "contract": {"version": "1.0.0", "settings": {}, "consumes": {}}}),
    );

    // The shipped templates, the space's model lane pointed at nothing: this
    // story writes only inside a workspace, where nothing is summarised.
    let tpl = root.join("templates");
    copy_tree(&repo("templates/projection"), &tpl.join("projection"));
    copy_tree(&repo("templates/file-space"), &tpl.join("file-space"));
    patch_json(&tpl.join("file-space/template.json"), |v| {
        v.as_object_mut().unwrap().remove("requires");
    });
    patch_json(&tpl.join("file-space/summarizer/config.json"), |v| {
        v["params"]["model"] = json!("mock-model");
        v["params"]["api_key"] = json!("fake-key");
        v["params"]["base_url"] = json!("http://127.0.0.1:9");
    });
    // A template that holds a file space at `./files`, as a member will.
    write_json(
        &tpl.join("fs-host/template.json"),
        &json!({"name": "fs-host", "version": "1.0.0",
                "description": {"purpose": "a test holder of one file space",
                                "use_when": "this lock only", "not_in_scope": "everything else"},
                "tags": ["test"], "author": "@example", "license": "MIT"}),
    );
    let lanes = "has(hop.route) && (hop.route == 'in_ws' || hop.route == 'in_read' || \
                 hop.route == 'in_write')";
    write_json(
        &tpl.join("fs-host/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./files", "condition": lanes},
            // Only answers leave: a request delivered to `/host/files` is also
            // seen here as leaving `./files` (hive transit), and an open edge
            // carried it out and back in through `. -> ./files`.
            {"from": "./files", "to": ".", "condition": "has(hop.route) && hop.route == 'answer'"}
        ]}}}),
    );
    let fs_ref = format!(
        "file-space@{}",
        read_json(&repo("templates/file-space/template.json"))["version"]
            .as_str()
            .unwrap()
    );
    write_json(
        &tpl.join("fs-host/files/config.json"),
        &json!({"cell": {"type": "ref", "template": fs_ref}}),
    );

    // The owner's override, two refs deep (OR-FJ-G9).
    let b = base.to_str().unwrap();
    write_json(
        &root.join("main/host/config.json"),
        &json!({"cell": {"type": "ref", "template": "fs-host@1.0.0"},
        "override_params": {
            "files/projection/mat": {"base_path": b, "exec_allow": ["python3"],
                                     "auto_adopt": ["python3"],
                                     "sandbox": owner_sandbox(base)},
            "files/projection/run": {"base_path": b, "sandbox": owner_sandbox(base)}
        }}),
    );

    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ];
    let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories.clone(), MARK);
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: tpl.clone(),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the template table fills");
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the colony boots and grows the marker");
    // The override reached both cells, two refs down.
    for cell in ["mat", "run"] {
        let cfg = read_json(&root.join(format!("main/host/files/projection/{cell}/config.json")));
        assert_eq!(cfg["params"]["base_path"], json!(b), "{cell}: base_path");
        assert_eq!(
            cfg["params"]["sandbox"]["filesystem"]["write"],
            json!([b]),
            "{cell}: the write grant"
        );
    }
    Colony {
        _td: td,
        h,
        egress,
        n: 0,
    }
}

impl Colony {
    /// One request on a lane of the space; its answer, read where it left the colony.
    async fn ask(
        &mut self,
        route: &str,
        op: &str,
        ws: &str,
        file: Option<&str>,
        args: Value,
    ) -> Value {
        self.n += 1;
        let op_id = format!("t{}", self.n);
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".into(), json!(route));
        hop.insert("op".into(), json!(op));
        hop.insert("op_id".into(), json!(op_id));
        if !ws.is_empty() {
            hop.insert("ws".into(), json!(ws));
        }
        let mut body = json!({"op": op, "args": args, "messages": []});
        if let Some(f) = file {
            body["file"] = json!(f);
        }
        let msg = MessageBuilder::new(Path::new(SPACE))
            .hop(hop)
            .body(Body::Inline(body))
            .ttl(64)
            .build();
        self.h.send(msg).await;
        let deadline = tokio::time::Instant::now() + MARKER;
        loop {
            let got = tokio::time::timeout_at(deadline, self.egress.recv())
                .await
                .unwrap_or_else(|_| panic!("no answer to {op} {op_id} within {MARKER:?}"))
                .expect("egress open");
            if got.headers.hop.get("op_id") != Some(&json!(op_id)) {
                continue;
            }
            assert_eq!(got.headers.hop.get("route"), Some(&json!("answer")), "{op}");
            match got.body {
                Body::Inline(v) => return v,
                Body::Blob(_) => panic!("{op}: a blob answer in a lock with tiny files"),
            }
        }
    }

    async fn ok(
        &mut self,
        route: &str,
        op: &str,
        ws: &str,
        file: Option<&str>,
        args: Value,
    ) -> Value {
        let a = self.ask(route, op, ws, file, args).await;
        assert_eq!(a["ok"], json!(true), "{op}: {a}");
        a
    }

    /// The manifest the projection's store took last, read where it took it.
    async fn manifest(&self) -> Value {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    to_path_prefix: Some(STORE.to_string()),
                    limit: 1000,
                    scan_budget: 100_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("log")
            .expect("ack");
        // newest first: the first write of a `proj` row is the latest manifest
        for row in reply.entries {
            let Some(raw) = row.body_payload.as_deref() else {
                continue;
            };
            let Ok(body) = meclaw_core::serde_json::from_str::<Value>(raw) else {
                continue;
            };
            for turn in body["messages"].as_array().cloned().unwrap_or_default() {
                let Some(text) = turn["text"].as_str() else {
                    continue;
                };
                let Ok(call) = meclaw_core::serde_json::from_str::<Value>(text) else {
                    continue;
                };
                if call["table"] != json!("proj") || call["operation"] == json!("select") {
                    continue;
                }
                let m = if call["operation"] == json!("insert") {
                    &call["row"]["manifest"]
                } else {
                    &call["set"]["manifest"]
                };
                return meclaw_core::serde_json::from_str(m.as_str().unwrap()).unwrap();
            }
        }
        panic!("no proj write reached {STORE}");
    }
}

fn py(code: &str) -> Value {
    json!(["python3", "-c", code])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_is_materialized_and_adopted() {
    if !can_run() {
        return;
    }
    // A variable of the host process that must never reach a tool (Review J I-1).
    // SAFETY: set once, before the colony and its cells start in this process.
    unsafe { std::env::set_var(HOST_MARKER, "must-not-leak") };
    let base_td = tempfile::TempDir::new().expect("base");
    let base = base_td.path().to_path_buf();
    let outside_td = tempfile::TempDir::new().expect("outside");
    let mut c = boot(&base).await;

    // A workspace with three files, created in it (not seeded).
    let opened = c
        .ok(
            "in_ws",
            "ws_open",
            "",
            None,
            json!({"name": "w1", "root": "/proj"}),
        )
        .await;
    let ws_id = opened["ws"]
        .as_str()
        .expect("ws_open names its id")
        .to_string();
    let mut ids = std::collections::BTreeMap::new();
    for (path, text) in [
        ("/proj/a.py", "x = 1\n"),
        ("/proj/b.py", "y = 1\n"),
        ("/proj/notes/c.txt", "see\n"),
    ] {
        let a = c
            .ok(
                "in_write",
                "create",
                "w1",
                None,
                json!({"path": path, "text": text}),
            )
            .await;
        ids.insert(
            path,
            (
                a["file"].as_str().unwrap().to_string(),
                a["version"].clone(),
            ),
        );
    }

    // Laid out: three files in `<base>/w1/`, relative to the root, a manifest of three.
    let m = c.ok("in_ws", "ws_materialize", "w1", None, json!({})).await;
    assert_eq!(m["written"], json!(3), "{m}");
    assert_eq!(m["dirty"], json!([]), "{m}");
    let dir = base.join("w1");
    assert_eq!(m["dir"], json!(dir.to_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.py")).unwrap(),
        "x = 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes/c.txt")).unwrap(),
        "see\n"
    );
    let man = c.manifest().await;
    assert_eq!(man.as_object().unwrap().len(), 3, "{man}");
    assert_eq!(man["/proj/a.py"]["file"], json!(ids["/proj/a.py"].0));

    // Named by its id, the workspace is the same directory (OR-FJ.J.1: `ws_tree`
    // names it), and nothing is written twice.
    let m = c
        .ok("in_ws", "ws_materialize", &ws_id, None, json!({}))
        .await;
    assert_eq!(m["dir"], json!(dir.to_str().unwrap()), "{m}");
    assert_eq!(m["written"], json!(0), "{m}");

    // A permitted formatter: a new working version, no proposal.
    let fmt = py(
        "import pathlib; p = pathlib.Path('a.py'); p.write_text(p.read_text().replace('x = 1', 'x = 2'))",
    );
    let r = c
        .ok("in_ws", "ws_exec", "w1", None, json!({"argv": fmt}))
        .await;
    assert_eq!(r["exit"], json!(0), "{r}");
    assert_eq!(r["proposals"], json!([]), "{r}");
    let adopted = r["adopted"].as_array().unwrap();
    assert_eq!(adopted.len(), 1, "{r}");
    assert_eq!(adopted[0]["path"], json!("/proj/a.py"));
    assert_ne!(adopted[0]["version"], ids["/proj/a.py"].1, "a new version");
    let read = c
        .ok(
            "in_read",
            "read",
            "w1",
            Some(&ids["/proj/a.py"].0),
            json!({}),
        )
        .await;
    assert!(read["text"].as_str().unwrap().contains("x = 2"), "{read}");

    // A generator: `created` is a proposal, born after `ws_adopt`.
    let gen_argv = py("open('gen.txt', 'w').write('generated\\n')");
    let r = c
        .ok("in_ws", "ws_exec", "w1", None, json!({"argv": gen_argv}))
        .await;
    assert_eq!(r["adopted"], json!([]), "{r}");
    let props = r["proposals"].as_array().unwrap();
    assert_eq!(props.len(), 1, "{r}");
    assert_eq!(props[0]["path"], json!("/proj/gen.txt"));
    assert_eq!(props[0]["kind"], json!("created"));
    let run = r["run"].as_str().unwrap().to_string();
    let not_yet = c
        .ask("in_read", "info", "w1", Some("/proj/gen.txt"), json!({}))
        .await;
    assert_eq!(
        not_yet["ok"],
        json!(false),
        "no file before the adoption: {not_yet}"
    );
    let a = c
        .ok(
            "in_ws",
            "ws_adopt",
            "w1",
            None,
            json!({"run": run, "ids": [props[0]["id"]]}),
        )
        .await;
    assert_eq!(a["adopted"][0]["path"], json!("/proj/gen.txt"), "{a}");
    let born = c
        .ok("in_read", "read", "w1", Some("/proj/gen.txt"), json!({}))
        .await;
    assert!(
        born["text"].as_str().unwrap().contains("generated"),
        "{born}"
    );
    assert_eq!(born["file"], a["adopted"][0]["file"]);
    let again = c
        .ok(
            "in_ws",
            "ws_adopt",
            "w1",
            None,
            json!({"run": run, "ids": "all"}),
        )
        .await;
    assert_eq!(again["adopted"], json!([]), "never adopted twice: {again}");

    // A program not in exec_allow: refused, nothing touched.
    let before = std::fs::read_dir(&dir).unwrap().count();
    let no = c
        .ask(
            "in_ws",
            "ws_exec",
            "w1",
            None,
            json!({"argv": ["sh", "-c", "touch z"]}),
        )
        .await;
    assert_eq!(no["ok"], json!(false));
    assert_eq!(no["error"]["code"], json!("not_allowed"), "{no}");
    assert!(!dir.join("z").exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);

    // The time cap ends a run that outlives it: `exit: 'timeout'`, nothing adopted.
    let sleeper = py("import time; time.sleep(30)");
    let r = c
        .ok(
            "in_ws",
            "ws_exec",
            "w1",
            None,
            json!({"argv": sleeper, "timeout_ms": 500}),
        )
        .await;
    assert_eq!(r["exit"], json!("timeout"), "{r}");
    assert_eq!(r["adopted"], json!([]), "{r}");
    assert_eq!(r["proposals"], json!([]), "{r}");

    // The environment of a run is PATH, HOME and LANG -- nothing of the host.
    let env_argv = py("import os; print(sorted(os.environ))");
    let r = c
        .ok("in_ws", "ws_exec", "w1", None, json!({"argv": env_argv}))
        .await;
    assert_eq!(r["exit"], json!(0), "{r}");
    assert_eq!(
        r["out_tail"].as_str().unwrap_or_default().trim(),
        "['HOME', 'LANG', 'PATH']",
        "{r}"
    );
    assert!(
        !r.to_string().contains(HOST_MARKER),
        "no host variable reached the tool: {r}"
    );

    // After a replace, the next materialisation writes exactly one file.
    let (bid, bv) = ids["/proj/b.py"].clone();
    c.ok(
        "in_write",
        "replace",
        "w1",
        Some(&bid),
        json!({"old": "y = 1", "new": "y = 3", "base": bv}),
    )
    .await;
    let m = c.ok("in_ws", "ws_materialize", "w1", None, json!({})).await;
    assert_eq!(m["written"], json!(1), "{m}");
    assert_eq!(m["removed"], json!(0), "{m}");
    assert_eq!(
        std::fs::read_to_string(dir.join("b.py")).unwrap(),
        "y = 3\n"
    );

    // A write outside base_path fails at the kernel.
    let escape = outside_td.path().join("escape");
    let argv = py(&format!(
        "open({:?}, 'w').write('x')",
        escape.to_str().unwrap()
    ));
    let r = c
        .ok("in_ws", "ws_exec", "w1", None, json!({"argv": argv}))
        .await;
    assert_ne!(r["exit"], json!(0), "{r}");
    assert!(!escape.exists(), "nothing was written outside base_path");
    assert_eq!(r["proposals"], json!([]), "{r}");

    c.h.shutdown().await;
}
