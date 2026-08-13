//! S4 (GH #35): the isolation properties, proven against a real kernel.
//!
//! Every proof has a control. A denial only means something if the same
//! operation succeeds when the profile permits it: without the control a broken
//! script would "prove" isolation just by failing.
//!
//! # Skipping, not failing
//!
//! User namespaces and Landlock can be absent (older kernel), switched off
//! (`kernel.unprivileged_userns_clone`, `user.max_user_namespaces`) or fenced
//! in by an LSM. A test that goes red on such a host hardens nobody; it only
//! invites someone to weaken the assertion. So the capability is probed first
//! and a missing one prints a visible line and ends green. The probes live in
//! `meclaw_cells::sandbox` and answer by trying, not by reading knobs.

use meclaw_cells::sandbox;
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

// ---- capability probes ----------------------------------------------------

/// True when this kernel can enforce a filesystem allowlist.
fn have_landlock(test: &str) -> bool {
    match sandbox::landlock_abi() {
        Some(abi) => {
            eprintln!("[{test}] landlock abi {abi}");
            true
        }
        None => {
            eprintln!("[{test}] SKIPPED: no Landlock on this kernel (needs Linux 5.13+)");
            false
        }
    }
}

/// True when this host lets an unprivileged process enter a fresh network
/// namespace.
fn have_netns(test: &str) -> bool {
    if sandbox::network_isolation_supported() {
        true
    } else {
        eprintln!(
            "[{test}] SKIPPED: unprivileged network namespaces unavailable on this host \
             (kernel knob, sandbox or LSM policy)"
        );
        false
    }
}

// ---- harness --------------------------------------------------------------

fn sink_for(otx: mpsc::Sender<CellEmission>, path: &str) -> OutputSink {
    OutputSink::new(
        otx,
        Path::new(path),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// Run one `code` cell built from `params` and return the single emission.
async fn run_code(params: meclaw_core::JsonValue) -> CellEmission {
    let parsed = meclaw_cells::code::CodeParams::parse(&params).expect("params parse");
    let cell = meclaw_cells::code::CodeCell::new(parsed, false, None, false);
    let (otx, mut orx) = mpsc::channel(8);
    let sink = sink_for(otx, "/code");
    let msg = MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink).await;
    drop(sink);
    orx.recv().await.expect("exactly one emission")
}

/// Run one `bash` cell command under `sandbox` and return the single emission.
async fn run_bash(sandbox_block: meclaw_core::JsonValue, command: &str) -> CellEmission {
    let mut params = json!({"external_timeout_ms": 20000, "max_concurrency": 1});
    if !sandbox_block.is_null() {
        params
            .as_object_mut()
            .unwrap()
            .insert("sandbox".into(), sandbox_block);
    }
    let factory: std::sync::Arc<dyn meclaw_colony::CellFactory> =
        std::sync::Arc::new(meclaw_cells::BashCellFactory);
    let (otx, mut orx) = mpsc::channel(8);
    let (itx, _irx) = mpsc::channel(8);
    let spawned = factory
        .spawn_cell(
            Path::new("/bash"),
            params,
            otx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            16,
        )
        .expect("spawn");
    let sender = match spawned {
        meclaw_colony::SpawnedCellKind::Active { sender, .. } => sender,
        meclaw_colony::SpawnedCellKind::Dormant { .. } => unreachable!("bash is stateless"),
    };
    let msg = MessageBuilder::new(Path::new("/bash"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": json!({"command": command}).to_string(), "id": "call-1"
            }]
        })))
        .build();
    sender.send(msg).await.unwrap();
    orx.recv().await.expect("emission")
}

/// The text of the single message in an emission.
fn text_of(em: &CellEmission) -> String {
    em.content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A python one-liner that reports whether `path` could be read, as a UBF body.
fn read_probe_script(path: &std::path::Path) -> String {
    format!(
        "import sys, json\n\
         try:\n    open({p}).read()\n    r = 'READ_OK'\n\
         except Exception as e:\n    r = 'READ_DENIED:' + type(e).__name__\n\
         sys.stdout.write(json.dumps({{'messages': [\
         {{'origin': 'assistant', 'type': 'text', 'text': r}}]}}))\n",
        p = json!(path.to_str().unwrap())
    )
}

/// A python one-liner that reports whether `127.0.0.1:port` could be reached.
fn connect_probe_script(port: u16) -> String {
    format!(
        "import sys, json, socket\n\
         try:\n    socket.create_connection(('127.0.0.1', {port}), timeout=3).close()\n    r = 'NET_OK'\n\
         except Exception as e:\n    r = 'NET_DENIED:' + type(e).__name__\n\
         sys.stdout.write(json.dumps({{'messages': [\
         {{'origin': 'assistant', 'type': 'text', 'text': r}}]}}))\n"
    )
}

// ---- property 1: cannot read outside the allowed paths -------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_cell_cannot_read_outside_allowed_paths() {
    const T: &str = "code_cell_cannot_read_outside_allowed_paths";
    if !have_landlock(T) {
        return;
    }
    let allowed = tempfile::TempDir::new().unwrap();
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(allowed.path().join("visible"), b"visible").unwrap();
    std::fs::write(forbidden.path().join("secret"), b"secret").unwrap();

    let sb = json!({
        "trust": "restricted",
        "network": "allow",
        "filesystem": {"read": [allowed.path().to_str().unwrap()]}
    });

    // The proof: a file in a directory that was never declared.
    let denied = run_code(json!({
        "runner": "python3",
        "script_inline": read_probe_script(&forbidden.path().join("secret")),
        "external_timeout_ms": 20000,
        "sandbox": sb.clone(),
    }))
    .await;
    assert_eq!(
        text_of(&denied),
        "READ_DENIED:PermissionError",
        "a file outside the allowlist must be unreachable, and REFUSED rather than \
         merely absent (header {})",
        denied.content["header"]
    );

    // The control: the same script, same profile, a file inside the allowlist.
    // Without this a broken script would "prove" isolation by failing.
    let ok = run_code(json!({
        "runner": "python3",
        "script_inline": read_probe_script(&allowed.path().join("visible")),
        "external_timeout_ms": 20000,
        "sandbox": sb,
    }))
    .await;
    assert_eq!(
        text_of(&ok),
        "READ_OK",
        "a file inside the allowlist must stay readable, got {:?} (header {})",
        text_of(&ok),
        ok.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bash_cell_cannot_read_outside_allowed_paths() {
    const T: &str = "bash_cell_cannot_read_outside_allowed_paths";
    if !have_landlock(T) {
        return;
    }
    let allowed = tempfile::TempDir::new().unwrap();
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(allowed.path().join("visible"), b"visible").unwrap();
    std::fs::write(forbidden.path().join("secret"), b"secret").unwrap();

    let sb = json!({
        "trust": "restricted",
        "network": "allow",
        "filesystem": {"read": [allowed.path().to_str().unwrap()]}
    });

    let denied = run_bash(
        sb.clone(),
        &format!(
            "cat {} && echo READ_OK || echo READ_DENIED",
            forbidden.path().join("secret").display()
        ),
    )
    .await;
    let t = text_of(&denied);
    assert!(
        t.contains("READ_DENIED") && t.contains("Permission denied"),
        "a file outside the allowlist must be REFUSED, not merely absent, got {t:?}"
    );

    let ok = run_bash(
        sb,
        &format!(
            "cat {} > /dev/null && echo READ_OK || echo READ_DENIED",
            allowed.path().join("visible").display()
        ),
    )
    .await;
    assert!(
        text_of(&ok).contains("READ_OK"),
        "a file inside the allowlist must stay readable, got {:?}",
        text_of(&ok)
    );
}

// ---- property 2: cannot reach the network when denied --------------------

/// Bind a listener on loopback and hand back its port. The listener stays alive
/// for as long as the returned value does.
fn loopback_listener() -> (std::net::TcpListener, u16) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_cell_cannot_reach_the_network_when_denied() {
    const T: &str = "code_cell_cannot_reach_the_network_when_denied";
    if !have_landlock(T) || !have_netns(T) {
        return;
    }
    // Loopback, not the internet: a fresh network namespace has nothing but a
    // `lo` in state DOWN, so even 127.0.0.1 is out of reach. That makes the
    // proof deterministic on an air-gapped runner.
    let (_listener, port) = loopback_listener();
    let work = tempfile::TempDir::new().unwrap();
    let fs = json!({"read": [work.path().to_str().unwrap()]});

    let denied = run_code(json!({
        "runner": "python3",
        "script_inline": connect_probe_script(port),
        "external_timeout_ms": 20000,
        "sandbox": {"trust": "restricted", "network": "deny", "filesystem": fs.clone()},
    }))
    .await;
    assert!(
        text_of(&denied).starts_with("NET_DENIED"),
        "network deny must cut the connection, got {:?} (header {})",
        text_of(&denied),
        denied.content["header"]
    );
    assert_ne!(
        text_of(&denied),
        "NET_DENIED:timeout",
        "a timeout would prove nothing: the namespace must refuse, not stall"
    );

    // The control: same script, same allowlist, network allowed.
    let ok = run_code(json!({
        "runner": "python3",
        "script_inline": connect_probe_script(port),
        "external_timeout_ms": 20000,
        "sandbox": {"trust": "restricted", "network": "allow", "filesystem": fs},
    }))
    .await;
    assert_eq!(
        text_of(&ok),
        "NET_OK",
        "with network allow the very same connect must succeed, got {:?}",
        text_of(&ok)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bash_cell_cannot_reach_the_network_when_denied() {
    const T: &str = "bash_cell_cannot_reach_the_network_when_denied";
    if !have_landlock(T) || !have_netns(T) {
        return;
    }
    let (_listener, port) = loopback_listener();
    let work = tempfile::TempDir::new().unwrap();
    let fs = json!({"read": [work.path().to_str().unwrap()]});
    let cmd = format!(
        "python3 -c \"import socket; socket.create_connection(('127.0.0.1', {port}), timeout=3)\" \
         && echo NET_OK || echo NET_DENIED"
    );

    let denied = run_bash(
        json!({"trust": "restricted", "network": "deny", "filesystem": fs.clone()}),
        &cmd,
    )
    .await;
    assert!(
        text_of(&denied).contains("NET_DENIED"),
        "network deny must cut the connection, got {:?}",
        text_of(&denied)
    );

    let ok = run_bash(
        json!({"trust": "restricted", "network": "allow", "filesystem": fs}),
        &cmd,
    )
    .await;
    assert!(
        text_of(&ok).contains("NET_OK"),
        "with network allow the very same connect must succeed, got {:?}",
        text_of(&ok)
    );
}

// ---- fail-closed ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_profile_that_cannot_be_applied_kills_the_spawn() {
    // No kernel feature needed: a declared path that does not exist is caught
    // in the parent, before any child is forked. The point is that the cell
    // reports an error instead of quietly running the script unsandboxed.
    let em = run_code(json!({
        "runner": "python3",
        "script_inline": "import sys; sys.stdout.write('{\"messages\":[]}')",
        "external_timeout_ms": 20000,
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/nonexistent-s4-sandbox-path"]}
        },
    }))
    .await;
    assert_eq!(em.content["header"]["error_code"], "io_error");
    let t = text_of(&em);
    assert!(
        t.contains("sandbox not applied") && t.contains("/nonexistent-s4-sandbox-path"),
        "the error must say which boundary could not be built, got {t:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_profile_is_the_declared_escape_hatch() {
    // The counterpart to the proofs above: `trusted` really does nothing, so a
    // local cell that needs full rights can say so and keep them.
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(forbidden.path().join("secret"), b"secret").unwrap();
    let em = run_code(json!({
        "runner": "python3",
        "script_inline": read_probe_script(&forbidden.path().join("secret")),
        "external_timeout_ms": 20000,
        "sandbox": {"trust": "trusted"},
    }))
    .await;
    assert_eq!(text_of(&em), "READ_OK");
}
