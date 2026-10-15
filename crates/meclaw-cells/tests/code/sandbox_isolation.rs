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

/// True when this host lets an unprivileged process cap a child through a
/// delegated cgroup v2 sub-cgroup (GH #85).
///
/// Creating the directory is not the hard part; moving the child in is. The
/// move needs write access to `cgroup.procs` of the common ancestor of source
/// and destination, which an ssh login shell (living in a root-owned
/// `session-<n>.scope`) does not have. The same daemon under
/// `systemctl --user` does. So this skips on a login shell and enforces under
/// a user unit -- run `systemd-run --user --scope cargo test ...` to exercise
/// it here.
fn have_cgroups(test: &str) -> bool {
    if sandbox::cgroup_delegation_supported() {
        eprintln!(
            "[{test}] cgroup delegation at {:?}",
            sandbox::delegated_root()
        );
        true
    } else {
        eprintln!(
            "[{test}] SKIPPED: no usable cgroup v2 delegation on this host (root {:?}); \
             a session scope cannot move processes into user@<uid>.service -- retry under \
             `systemd-run --user --scope`",
            sandbox::delegated_root()
        );
        false
    }
}

/// True when this kernel can enforce a seccomp-bpf syscall filter (GH #85).
fn have_seccomp(test: &str) -> bool {
    if sandbox::seccomp_supported() {
        true
    } else {
        eprintln!("[{test}] SKIPPED: no seccomp-bpf filter mode on this kernel or architecture");
        false
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

// ---- property 3: resource caps (GH #85) ----------------------------------

/// A python script that starts `n` threads and reports how far it got.
///
/// Threads, not forks: a thread is a task the `pids` controller counts just
/// like a process, and it needs no second interpreter, no pipe and no reaping.
fn thread_probe_script(n: usize) -> String {
    format!(
        "import sys, json, threading\n\
         ev = threading.Event()\n\
         ths = []\n\
         started = 0\n\
         try:\n    \
             for _ in range({n}):\n        \
                 t = threading.Thread(target=ev.wait)\n        \
                 t.start()\n        \
                 ths.append(t)\n        \
                 started += 1\n\
         except Exception:\n    \
             r = 'PIDS_CAPPED:%d' % started\n\
         else:\n    \
             r = 'PIDS_ALL:%d' % started\n\
         ev.set()\n\
         for t in ths:\n    t.join()\n\
         sys.stdout.write(json.dumps({{'messages': [\
         {{'origin': 'assistant', 'type': 'text', 'text': r}}]}}))\n"
    )
}

/// A python script that tries to touch `mb` megabytes and says whether it got
/// there. Under a `memory.max` below `mb` the process never reaches the print:
/// the kernel OOM-kills it, which the cell reports as a failed script.
fn memory_probe_script(mb: usize) -> String {
    format!(
        "import sys, json\n\
         buf = bytearray({mb} * 1024 * 1024)\n\
         for i in range(0, len(buf), 4096):\n    buf[i] = 1\n\
         sys.stdout.write(json.dumps({{'messages': [\
         {{'origin': 'assistant', 'type': 'text', 'text': 'ALLOC_OK'}}]}}))\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_cell_hits_the_declared_pids_cap() {
    const T: &str = "code_cell_hits_the_declared_pids_cap";
    if !have_landlock(T) || !have_cgroups(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();
    let fs = json!({"read": [work.path().to_str().unwrap()]});
    let script = thread_probe_script(40);

    // The proof: a cap of 8 tasks, and the interpreter is already one of them.
    let capped = run_code(json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 20000,
        "sandbox": {
            "trust": "restricted", "network": "allow", "filesystem": fs.clone(),
            "limits": {"pids_max": 8}
        },
    }))
    .await;
    let t = text_of(&capped);
    assert!(
        t.starts_with("PIDS_CAPPED:"),
        "a pids cap of 8 must stop a script that wants 40 threads, got {t:?} (header {})",
        capped.content["header"]
    );

    // The control: the same script, the same profile, a cap it cannot reach.
    // Without this a script that simply cannot start threads would "prove" the
    // cap by failing.
    let free = run_code(json!({
        "runner": "python3",
        "script_inline": thread_probe_script(40),
        "external_timeout_ms": 20000,
        "sandbox": {
            "trust": "restricted", "network": "allow", "filesystem": fs,
            "limits": {"pids_max": 400}
        },
    }))
    .await;
    assert_eq!(
        text_of(&free),
        "PIDS_ALL:40",
        "under a cap of 400 the very same script must start all 40 threads (header {})",
        free.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_cell_hits_the_declared_memory_cap() {
    const T: &str = "code_cell_hits_the_declared_memory_cap";
    if !have_landlock(T) || !have_cgroups(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();
    let fs = json!({"read": [work.path().to_str().unwrap()]});

    // The proof: 192 MiB wanted, 48 MiB allowed. The OOM killer ends the
    // script, so the cell never sees the ALLOC_OK line.
    let capped = run_code(json!({
        "runner": "python3",
        "script_inline": memory_probe_script(192),
        "external_timeout_ms": 30000,
        "sandbox": {
            "trust": "restricted", "network": "allow", "filesystem": fs.clone(),
            "limits": {"memory_max_bytes": 50331648}
        },
    }))
    .await;
    assert_ne!(
        text_of(&capped),
        "ALLOC_OK",
        "a memory cap of 48 MiB must stop a 192 MiB allocation (header {})",
        capped.content["header"]
    );
    assert_eq!(
        capped.content["header"]["error_code"], "script_failed",
        "and it must surface as a failed script, not as a timeout or a parse error \
         (header {})",
        capped.content["header"]
    );

    // The control: the same script, the same profile, a cap it fits into.
    let free = run_code(json!({
        "runner": "python3",
        "script_inline": memory_probe_script(192),
        "external_timeout_ms": 30000,
        "sandbox": {
            "trust": "restricted", "network": "allow", "filesystem": fs,
            "limits": {"memory_max_bytes": 1073741824}
        },
    }))
    .await;
    assert_eq!(
        text_of(&free),
        "ALLOC_OK",
        "under a cap of 1 GiB the very same allocation must succeed (header {})",
        free.content["header"]
    );
}

// ---- property 4: the syscall filter (GH #85) -----------------------------

/// A python script that signals itself and then its parent, and reports both.
///
/// The parent of a `code` cell's child is the daemon, so "may I signal my
/// parent" is literally the question the filter exists to answer. Signal `0`
/// delivers nothing and only runs the permission check, so the daemon is never
/// actually disturbed by the probe.
const SIGNAL_PROBE: &str = "import sys, json, os\n\
     def probe(pid):\n    \
         try:\n        \
             os.kill(pid, 0)\n        \
             return 'OK'\n    \
         except PermissionError:\n        \
             return 'EPERM'\n    \
         except Exception as e:\n        \
             return type(e).__name__\n\
     r = 'self=%s parent=%s' % (probe(os.getpid()), probe(os.getppid()))\n\
     sys.stdout.write(json.dumps({'messages': [\
     {'origin': 'assistant', 'type': 'text', 'text': r}]}))\n";

/// A python script that calls `ptrace(PTRACE_ATTACH)` on a pid that cannot
/// exist and reports the errno.
///
/// The errno is the whole point. Unfiltered, the call reaches the kernel and
/// comes back `ESRCH` because there is no such process. Filtered, it never
/// reaches the kernel and comes back `EPERM`. So the control does not merely
/// "fail differently", it proves the syscall was dispatched at all.
const PTRACE_PROBE: &str = "import sys, json, ctypes, errno\n\
     libc = ctypes.CDLL(None, use_errno=True)\n\
     libc.ptrace.restype = ctypes.c_long\n\
     libc.ptrace.argtypes = [ctypes.c_long] * 4\n\
     ctypes.set_errno(0)\n\
     libc.ptrace(16, 2147483646, 0, 0)\n\
     e = ctypes.get_errno()\n\
     r = {errno.EPERM: 'EPERM', errno.ESRCH: 'ESRCH'}.get(e, 'errno=%d' % e)\n\
     sys.stdout.write(json.dumps({'messages': [\
     {'origin': 'assistant', 'type': 'text', 'text': r}]}))\n";

/// A python script that asks for a raw socket and reports what it got.
const RAW_SOCKET_PROBE: &str = "import sys, json, socket\n\
     try:\n    \
         socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP).close()\n    \
         r = 'RAW_OK'\n\
     except PermissionError:\n    r = 'RAW_EPERM'\n\
     except Exception as e:\n    r = type(e).__name__\n\
     sys.stdout.write(json.dumps({'messages': [\
     {'origin': 'assistant', 'type': 'text', 'text': r}]}))\n";

/// Run `script` under a restricted profile, with `syscalls` either absent
/// (`None`, the control) or as given.
async fn run_filtered(
    script: &str,
    syscalls: Option<meclaw_core::JsonValue>,
    work: &std::path::Path,
) -> CellEmission {
    let mut sb = json!({
        "trust": "restricted",
        "network": "allow",
        "filesystem": {"read": [work.to_str().unwrap()]}
    });
    if let Some(s) = syscalls {
        sb.as_object_mut().unwrap().insert("syscalls".into(), s);
    }
    run_code(json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 20000,
        "sandbox": sb,
    }))
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filtered_cell_cannot_signal_the_daemon_that_started_it() {
    const T: &str = "a_filtered_cell_cannot_signal_the_daemon_that_started_it";
    if !have_landlock(T) || !have_seccomp(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();

    // The control first: without the filter the child may signal its parent,
    // which is exactly the hole Landlock leaves open.
    let unfiltered = run_filtered(SIGNAL_PROBE, None, work.path()).await;
    assert_eq!(
        text_of(&unfiltered),
        "self=OK parent=OK",
        "without a filter a sandboxed child can signal the daemon (header {})",
        unfiltered.content["header"]
    );

    // The proof: the same script, the same profile, one axis closed.
    let filtered = run_filtered(
        SIGNAL_PROBE,
        Some(json!({"foreign_signals": "deny", "ptrace": "allow", "raw_sockets": "allow"})),
        work.path(),
    )
    .await;
    assert_eq!(
        text_of(&filtered),
        "self=OK parent=EPERM",
        "the filter must stop signals to the daemon and leave self-signalling \
         (which is what raise/abort need) alone (header {})",
        filtered.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filtered_cell_cannot_ptrace() {
    const T: &str = "a_filtered_cell_cannot_ptrace";
    if !have_landlock(T) || !have_seccomp(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();

    let unfiltered = run_filtered(PTRACE_PROBE, None, work.path()).await;
    assert_eq!(
        text_of(&unfiltered),
        "ESRCH",
        "the control must reach the kernel and be answered by it (header {})",
        unfiltered.content["header"]
    );

    let filtered = run_filtered(
        PTRACE_PROBE,
        Some(json!({"ptrace": "deny", "foreign_signals": "allow", "raw_sockets": "allow"})),
        work.path(),
    )
    .await;
    assert_eq!(
        text_of(&filtered),
        "EPERM",
        "under the filter the very same call never reaches the kernel (header {})",
        filtered.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filtered_cell_cannot_open_a_raw_socket() {
    const T: &str = "a_filtered_cell_cannot_open_a_raw_socket";
    if !have_landlock(T) || !have_seccomp(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();

    // No positive control, no proof. An unprivileged process without
    // CAP_NET_RAW is refused a raw socket by the capability check anyway, so on
    // such a host a denial says nothing about the filter. Measured on the
    // target platform: Ubuntu with `apparmor_restrict_unprivileged_userns = 1`
    // refuses it even inside a fresh user namespace.
    let unfiltered = run_filtered(RAW_SOCKET_PROBE, None, work.path()).await;
    if text_of(&unfiltered) != "RAW_OK" {
        eprintln!(
            "[{T}] SKIPPED: this host refuses a raw socket even unfiltered ({:?}), so there \
             is no positive control to prove the filter against",
            text_of(&unfiltered)
        );
        return;
    }

    let filtered = run_filtered(
        RAW_SOCKET_PROBE,
        Some(json!({"raw_sockets": "deny", "ptrace": "allow", "foreign_signals": "allow"})),
        work.path(),
    )
    .await;
    assert_eq!(
        text_of(&filtered),
        "RAW_EPERM",
        "the filter must refuse the raw socket the control just got (header {})",
        filtered.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_filter_leaves_an_ordinary_socket_alone() {
    // The scope check: `raw_sockets: deny` must close the raw path and nothing
    // else, otherwise the deny would be a blanket socket ban wearing a name it
    // does not deserve.
    const T: &str = "the_filter_leaves_an_ordinary_socket_alone";
    if !have_landlock(T) || !have_seccomp(T) {
        return;
    }
    let work = tempfile::TempDir::new().unwrap();
    let em = run_filtered(
        "import sys, json, socket\n\
         s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n\
         s.close()\n\
         sys.stdout.write(json.dumps({'messages': [\
         {'origin': 'assistant', 'type': 'text', 'text': 'TCP_OK'}]}))\n",
        Some(json!({})),
        work.path(),
    )
    .await;
    assert_eq!(
        text_of(&em),
        "TCP_OK",
        "a TCP socket is not a raw socket (header {})",
        em.content["header"]
    );
}

// ---- the template default, end to end (GH #85) ---------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_profile_for_template_sourced_cells_actually_runs_a_script() {
    // The other half of the migration cut lives in `meclaw-colony`
    // (`gh85_template_default_deny.rs`), which can only assert the SHAPE of the
    // block it writes. This is the half that matters to an operator: the very
    // same block, parsed and enforced by this crate, still lets an ordinary
    // template-sourced script run. A default that quietly broke every
    // instantiated `code` cell would be a worse outcome than no default.
    const T: &str = "the_default_profile_for_template_sourced_cells_actually_runs_a_script";
    if !have_landlock(T) || !have_netns(T) {
        return;
    }
    let em = run_code(json!({
        "runner": "python3",
        "script_inline": "import sys, json, os\n\
                          sys.stdout.write(json.dumps({'messages': [{'origin': 'assistant', \
                          'type': 'text', 'text': 'DEFAULT_OK'}]}))\n",
        "external_timeout_ms": 20000,
        // Byte for byte what `default_sandbox_block()` writes into an
        // instantiated config.json.
        "sandbox": {"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}},
    }))
    .await;
    assert_eq!(
        text_of(&em),
        "DEFAULT_OK",
        "the default profile must leave the interpreter and its libraries \
         reachable (header {})",
        em.content["header"]
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
