//! GH #144 -- regression lock: `network: "allow"` must be able to resolve a
//! name, not merely to open a socket.
//!
//! # The defect this pins
//!
//! Under `trust: "restricted"` the Landlock allow-list was the runtime set
//! `/usr /lib /lib64 /bin /sbin /etc /proc /sys` plus the usual device nodes.
//! `/etc/resolv.conf` is inside it -- but on a systemd-resolved host that file
//! is a SYMLINK into `/run/systemd/resolve/`, and `/run` was in no set at all.
//! So a cell that declared `network: "allow"` got a working socket layer and a
//! dead resolver: every outbound call died in `getaddrinfo`, which Python
//! reports as `URLError`, while the profile said the network was open. Measured
//! on a benchmark run: 953/953 embedding calls "endpoint unreachable" at
//! `exit_code: 0`.
//!
//! # What is locked, and what deliberately is not
//!
//! Locked: `network: "allow"` implies the resolver configuration stays
//! readable. That is the property the word "allow" promises, so it is the
//! property that gets a test.
//!
//! Not locked: WHICH path carries it. `/etc/resolv.conf` points at
//! `/run/systemd/resolve/` here, at `/run/resolvconf/` elsewhere and at a plain
//! file on a third host; pinning the path would pin this laptop rather than the
//! property.
//!
//! # Skipping, not failing
//!
//! Same discipline as `sandbox_isolation.rs`: a kernel without Landlock cannot
//! enforce the property under test, and a test that goes red there hardens
//! nobody -- it only invites someone to weaken the assertion. The capability is
//! probed by trying, and a missing one prints a visible line and ends green.

use meclaw_cells::sandbox;
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

/// True when this kernel can enforce a filesystem allow-list.
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
/// namespace -- what `network: "deny"` needs to be applied at all.
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

/// Where `/etc/resolv.conf` really lives on this host, if anywhere.
fn resolver_target() -> Option<std::path::PathBuf> {
    std::fs::canonicalize("/etc/resolv.conf").ok()
}

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

/// The text of the single message in an emission.
fn text_of(em: &CellEmission) -> String {
    em.content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A python probe that reports whether the resolver configuration could be
/// read. It opens `/etc/resolv.conf` by its usual name, symlink and all --
/// which is exactly what `getaddrinfo` does before it sends a query.
fn resolv_probe_script() -> String {
    "import sys, json\n\
     try:\n    open('/etc/resolv.conf').read()\n    r = 'READ_OK'\n\
     except Exception as e:\n    r = 'READ_DENIED:' + type(e).__name__\n\
     sys.stdout.write(json.dumps({'messages': [\
     {'origin': 'assistant', 'type': 'text', 'text': r}]}))\n"
        .to_string()
}

/// The profile the shipped templates use for an egress cell: nothing but the
/// runtime set, and the network open.
fn allow_profile() -> meclaw_core::JsonValue {
    json!({"trust": "restricted", "network": "allow", "filesystem": {"runtime": true}})
}

// ---- the lock -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_allow_keeps_the_resolver_configuration_readable() {
    const T: &str = "network_allow_keeps_the_resolver_configuration_readable";
    if !have_landlock(T) {
        return;
    }
    let Some(target) = resolver_target() else {
        eprintln!("[{T}] SKIPPED: this host has no /etc/resolv.conf to resolve with");
        return;
    };
    eprintln!("[{T}] /etc/resolv.conf resolves to {}", target.display());

    let em = run_code(json!({
        "runner": "python3",
        "script_inline": resolv_probe_script(),
        "external_timeout_ms": 20000,
        "sandbox": allow_profile(),
    }))
    .await;

    assert_eq!(
        text_of(&em),
        "READ_OK",
        "network \"allow\" promises the network, and a name that cannot be resolved is not \
         a network; {} must stay readable (header {})",
        target.display(),
        em.content["header"]
    );
}

/// The control: the widening rides on `network: "allow"` and on nothing else.
///
/// Only assertable where the resolver configuration actually lives outside the
/// runtime set -- on a host whose `/etc/resolv.conf` is a plain file in `/etc`
/// it is readable under both policies, and asserting a denial there would pin
/// the host rather than the cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_widening_rides_on_allow_and_nothing_else() {
    const T: &str = "the_widening_rides_on_allow_and_nothing_else";
    if !have_landlock(T) || !have_netns(T) {
        return;
    }
    let Some(target) = resolver_target() else {
        eprintln!("[{T}] SKIPPED: this host has no /etc/resolv.conf");
        return;
    };
    if target.starts_with("/etc") {
        eprintln!(
            "[{T}] SKIPPED: {} is inside the runtime set on this host, so both policies read it",
            target.display()
        );
        return;
    }

    let em = run_code(json!({
        "runner": "python3",
        "script_inline": resolv_probe_script(),
        "external_timeout_ms": 20000,
        "sandbox": {"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}},
    }))
    .await;

    assert_eq!(
        text_of(&em),
        "READ_DENIED:PermissionError",
        "a cell in a fresh network namespace has nothing to resolve for, so it must not be \
         handed {} either -- the grant is part of \"allow\", not of the runtime set (header {})",
        target.display(),
        em.content["header"]
    );
}
