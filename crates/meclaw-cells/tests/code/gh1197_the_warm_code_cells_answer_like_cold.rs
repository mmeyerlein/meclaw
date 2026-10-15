//! GH #1197 -- the file-space and projection cells whose script holds nothing
//! between messages run `warm`; every other `code` cell of the two templates
//! stays `cold` (R-DT-13: the runner mode is decided per cell).
//!
//! Two pins:
//! 1. **the shipped modes** -- the `runner_mode` of every `code` cell under
//!    `templates/file-space/` and `templates/projection/` is exactly the
//!    decided one: the warm set below, everything else cold. A later edit that
//!    flips a cell, or a new cell that arrives warm, is red until this list
//!    moves with it -- the decision does not drift silently.
//! 2. **warm answers like cold** -- the shipped script of every warm cell,
//!    spawned on the production path (`CodeCellFactory`) once `warm` with ONE
//!    child and once `cold`, gets the same two messages one after the other,
//!    and warm's answers equal cold's. Message 2 runs in the interpreter
//!    message 1 left behind, so a module global, a cache, a working directory
//!    or a `sys.path` a run leaves behind shows up as a different answer
//!    (the pattern of GH #1026).
//!
//! What a run makes fresh on purpose -- a `secrets` token, a file id, the wall
//! clock -- differs between ANY two runs, cold against cold as well; the
//! comparison masks exactly those shapes (`mask`) and drops the one header
//! field the cell itself adds from the wall clock (`duration_ms`), nothing else.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

/// The cells that run warm (GH #1197). Every one is reached several times per
/// turn or per file operation, reads its input from the message and keeps no
/// process state between messages; the report of the strand gives the line
/// per cell.
const WARM: [&str; 7] = [
    "file-space/guard",
    "file-space/ingest",
    "file-space/read",
    "file-space/tools",
    "file-space/write",
    "file-space/ws",
    "projection/mat",
];

fn templates() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped(cell: &str) -> Value {
    let path = templates().join(cell).join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

// ── 1. the shipped modes ─────────────────────────────────────────────────

/// Every `code` cell one level below `templates/file-space/` and
/// `templates/projection/`, with its runner mode (absent reads `cold`, the
/// parser's default).
fn shipped_modes() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for tree in ["file-space", "projection"] {
        let mut dirs: Vec<_> = std::fs::read_dir(templates().join(tree))
            .expect("the template tree")
            .map(|e| e.expect("a dir entry").path())
            .filter(|p| p.join("config.json").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = format!("{tree}/{}", dir.file_name().unwrap().to_string_lossy());
            let cfg = shipped(&name);
            if cfg["cell"]["type"] != json!("code") {
                continue;
            }
            let mode = cfg["params"]["runner_mode"].as_str().unwrap_or("cold");
            out.push((name, mode.to_string()));
        }
    }
    out
}

#[test]
fn the_shipped_runner_modes_are_the_decided_ones() {
    let modes = shipped_modes();
    for cell in WARM {
        assert!(
            modes.iter().any(|(name, _)| name == cell),
            "{cell}: a decided cell is a shipped code cell"
        );
    }
    for (cell, mode) in &modes {
        let decided = if WARM.contains(&cell.as_str()) {
            "warm"
        } else {
            "cold"
        };
        assert_eq!(
            mode, decided,
            "{cell}: the shipped runner_mode is the one GH #1197 decided"
        );
    }
}

// ── 2. warm answers like cold ────────────────────────────────────────────

/// One message of a probe: its hop, its context and its body.
struct Probe {
    hop: Value,
    context: Value,
    body: Value,
}

fn probe(hop: Value, body: Value) -> Probe {
    Probe {
        hop,
        context: json!({}),
        body,
    }
}

fn object(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// `text` with what a run makes fresh replaced: a run of 12 or more lowercase
/// hex digits (a `secrets` token, a file id, a micro- or millisecond stamp)
/// becomes `<hex>`, a `YYYY-MM-DD` date `<date>` and a `THH:MM:SS[.f]Z` right
/// after it `<time>`. Everything else stays, byte for byte.
fn mask(text: &str) -> String {
    let b = text.as_bytes();
    let is_date = |s: &[u8]| {
        s.len() >= 10
            && s[..4].iter().all(u8::is_ascii_digit)
            && s[4] == b'-'
            && s[5..7].iter().all(u8::is_ascii_digit)
            && s[7] == b'-'
            && s[8..10].iter().all(u8::is_ascii_digit)
    };
    let is_hex = |c: &u8| c.is_ascii_digit() || (b'a'..=b'f').contains(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if is_date(&b[i..]) {
            out.push_str("<date>");
            i += 10;
            if b.get(i) == Some(&b'T') {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':' || b[i] == b'.') {
                    i += 1;
                }
                if b.get(i) == Some(&b'Z') {
                    i += 1;
                }
                out.push_str("<time>");
            }
            continue;
        }
        let run = b[i..].iter().take_while(|c| is_hex(c)).count();
        if run >= 12 {
            out.push_str("<hex>");
            i += run;
        } else if run > 0 {
            out.push_str(&text[i..i + run]);
            i += run;
        } else {
            let ch = text[i..].chars().next().expect("i is a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Spawn `script_params` (a cell's shipped params) as `cell` in `mode` (overlaid
/// with `overlay`, one child), send `probes` one after the other and return, per message, every
/// emission up to its consume mark -- target and body, masked, with `base`
/// (a directory the overlay names) written as `<base>`.
async fn answers(
    cell: &str,
    script_params: &Value,
    mode: &str,
    overlay: &Value,
    base: Option<&str>,
    probes: &[Probe],
) -> Vec<Vec<String>> {
    let mut params = script_params.clone();
    for (k, v) in object(overlay) {
        params[k] = v;
    }
    params["runner_mode"] = json!(mode);
    // One child: message 2 runs in the interpreter message 1 left behind.
    params["max_concurrency"] = json!(1);
    let at = Path::new(&format!("/files/{}", cell.rsplit('/').next().unwrap()));
    let (otx, mut orx) = tokio::sync::mpsc::channel(64);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            at.clone(),
            params,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap_or_else(|e| panic!("{cell} {mode}: the shipped params spawn: {e:?}"));
    let tx = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };
    let mut all = Vec::with_capacity(probes.len());
    for (n, p) in probes.iter().enumerate() {
        let msg = MessageBuilder::new(at.clone())
            .hop(object(&p.hop))
            .context(object(&p.context))
            .body(Body::Inline(p.body.clone()))
            .reply_to(Path::new("/sink"))
            .build();
        let id = msg.id;
        tx.send(msg).await.expect("the cell takes the message");
        let mut got = Vec::new();
        loop {
            let em = tokio::time::timeout(
                meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG),
                orx.recv(),
            )
            .await
            .unwrap_or_else(|_| panic!("{cell} {mode} message {}: HANG, no consume mark", n + 1))
            .unwrap_or_else(|| panic!("{cell} {mode}: the cell ended"));
            if em.consumed_mark_id() == Some(id) {
                break;
            }
            assert!(
                em.content["header"]["error_code"].is_null(),
                "{cell} {mode} message {}: the script must run, not fail: {}",
                n + 1,
                em.content
            );
            // `header.duration_ms` is the cell's wall clock around the run,
            // not the script's answer (time is never a verdict, rule 11).
            let mut content = em.content.clone();
            if let Some(h) = content.get_mut("header").and_then(Value::as_object_mut) {
                h.remove("duration_ms");
            }
            let text = format!("{} {}", em.target.as_str(), content);
            let text = match base {
                Some(b) => text.replace(b, "<base>"),
                None => text,
            };
            got.push(mask(&text));
        }
        assert!(
            !got.is_empty(),
            "{cell} {mode} message {}: a probe that answers nothing proves nothing",
            n + 1
        );
        all.push(got);
    }
    all
}

/// The acceptance of GH #1197 for one cell: two messages, warm on one child
/// against cold, the same answers.
async fn warm_answers_like_cold(cell: &str, overlay: impl Fn(&str) -> Value, probes: [Probe; 2]) {
    assert!(
        WARM.contains(&cell),
        "{cell}: only a warm cell needs the pin"
    );
    let cold_base = tempfile::TempDir::new().expect("tempdir");
    let warm_base = tempfile::TempDir::new().expect("tempdir");
    let cold_dir = cold_base.path().to_str().unwrap().to_string();
    let warm_dir = warm_base.path().to_str().unwrap().to_string();
    let script = &shipped(cell)["params"];
    let cold = answers(
        cell,
        script,
        "cold",
        &overlay(&cold_dir),
        Some(&cold_dir),
        &probes,
    )
    .await;
    let warm = answers(
        cell,
        script,
        "warm",
        &overlay(&warm_dir),
        Some(&warm_dir),
        &probes,
    )
    .await;
    for (n, (c, w)) in cold.iter().zip(&warm).enumerate() {
        assert_eq!(
            w,
            c,
            "{cell} message {}: warm answers what cold answers{}",
            n + 1,
            if n > 0 {
                " -- nothing of message 1 may reach message 2"
            } else {
                ""
            }
        );
    }
}

fn no_overlay(_: &str) -> Value {
    json!({})
}

/// The control: the comparison sees a leak. A script that keeps a counter on
/// `sys` -- process state, which a fresh globals dict does not reset -- answers
/// `1` to both messages cold and `2` to message 2 warm, and the pin's very
/// comparison tells them apart. Without this, a comparator that masked too much
/// would pass every cell above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_script_that_keeps_process_state_answers_differently_warm() {
    let leaky = json!({
        "runner": "python3",
        "external_timeout_ms": 30_000,
        "script_inline": "import sys, json\n\
                          json.load(sys.stdin)\n\
                          sys.meclaw_gh1197 = getattr(sys, 'meclaw_gh1197', 0) + 1\n\
                          sys.stdout.write(json.dumps({'messages': [], 'n': sys.meclaw_gh1197}))\n"
    });
    let two = || [probe(json!({}), json!({})), probe(json!({}), json!({}))];
    let cold = answers("control/leaky", &leaky, "cold", &json!({}), None, &two()).await;
    let warm = answers("control/leaky", &leaky, "warm", &json!({}), None, &two()).await;
    assert_eq!(warm[0], cold[0], "message 1 starts from the same state");
    assert_ne!(
        warm[1], cold[1],
        "message 2 warm sees what message 1 left on `sys`; the comparison must say so"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guard_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/guard",
        no_overlay,
        [
            probe(
                json!({"route": "in_check", "op_id": "w-1"}),
                json!({"kind": "text", "path": "/a.toml", "text": "a = ["}),
            ),
            probe(
                json!({"route": "in_check", "op_id": "w-2"}),
                json!({"kind": "text", "path": "/b.json", "text": "{\"x\": 1}"}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingest_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/ingest",
        no_overlay,
        [
            probe(
                json!({"route": "in_ingest"}),
                json!({
                    "messages": [{"origin": "user", "type": "text", "text": "see"}],
                    "attachments": [{"blob_id": "b1", "mime_type": "text/plain",
                                     "filename": "a.txt", "size_bytes": 5, "sha256": "00"}]
                }),
            ),
            probe(
                json!({"route": "in_ingest"}),
                json!({"messages": [{"origin": "user", "type": "text", "text": "no file"}]}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/read",
        no_overlay,
        [
            probe(
                json!({"route": "in_read", "op": "read", "op_id": "r-1", "caller": "tools"}),
                json!({"file": "/a.md", "args": {}}),
            ),
            probe(
                json!({"route": "in_read", "op": "list", "op_id": "r-2", "caller": "tools"}),
                json!({"args": {"dir": "/"}}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_answers_alike_warm_and_cold() {
    let call = |id: &str, tool: &str, text: &str| Probe {
        hop: json!({"route": "in_tool", "tool_name": tool, "tool_call_id": id}),
        context: json!({"tool_caller": "/brain"}),
        body: json!({"messages": [{"id": id, "text": text}]}),
    };
    warm_answers_like_cold(
        "file-space/tools",
        no_overlay,
        [
            call("c1", "file_read", "{\"file\": \"/a.md\"}"),
            call("c2", "file_list", "{}"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/write",
        no_overlay,
        [
            probe(
                json!({"route": "in_write", "op": "create", "op_id": "tools:c1", "caller": "tools"}),
                json!({"args": {"path": "/a.md", "text": "hello\n"}}),
            ),
            probe(
                json!({"route": "in_write", "op": "overwrite", "op_id": "tools:c2", "caller": "tools"}),
                json!({"file": "/b.md", "args": {"text": "more\n"}}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/ws",
        no_overlay,
        [
            probe(
                json!({"route": "in_ws", "op": "ws_open", "op_id": "tools:c1", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {"name": "w1"}}),
            ),
            probe(
                json!({"route": "in_ws", "op": "ws_status", "op_id": "tools:c2", "caller": "tools",
                       "ws": "w2"}),
                json!({"args": {}}),
            ),
        ],
    )
    .await;
}

/// `projection/mat` writes below `base_path`, which the owner's override grants
/// to its sandbox (as `gh905_a_workspace_is_materialized_and_adopted` does);
/// each mode gets its own base, so neither run finds the other's layout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mat_answers_alike_warm_and_cold() {
    let owner = |base: &str| {
        json!({
            "base_path": base,
            "exec_allow": ["true"],
            "sandbox": {"trust": "restricted", "network": "deny",
                        "filesystem": {"runtime": true, "write": [base]}}
        })
    };
    warm_answers_like_cold(
        "projection/mat",
        owner,
        [
            probe(
                json!({"route": "in_proj", "op": "ws_materialize", "op_id": "m-1", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {}}),
            ),
            probe(
                json!({"route": "in_proj", "op": "ws_exec", "op_id": "m-2", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {"argv": ["true"]}}),
            ),
        ],
    )
    .await;
}
