//! GH #766 (wave G, T2a) — a third shape of `params.sandbox`: the cap alone.
//!
//! The two shapes this schema had both start from a filesystem view, because
//! both were written for a cell that runs code somebody sent it. A browser out
//! of the distribution's packages is the other case: it brings its OWN
//! confinement — the package's AppArmor profile plus the browser's namespace
//! and seccomp sandbox — and what the substrate has to add is not a second
//! boundary but a ceiling, so one member's browser cannot eat the machine.
//!
//! The measurement that forces the shape (18.09.2026): `snap-confine`, the
//! launcher of a snap-packaged browser, does not start under `no_new_privs`,
//! and this substrate set that flag for every `restricted` profile. So the
//! third shape declares `limits` and nothing else, and `no_new_privs` moves to
//! where it is actually needed — Landlock and seccomp, which are the two things
//! that need it (R-G13).
//!
//! `limits` stays mandatory in that shape. A `restricted` profile that
//! restricts nothing would be `trusted` under another name, and the whole key
//! exists so that a cap nobody enforces cannot tell an operator a comforting
//! lie.

use meclaw_cells::sandbox::{self, NetworkPolicy, SandboxProfile};
use meclaw_core::serde_json::json;

/// The shape a packaged browser ships with (`templates/browser/config.json`).
fn the_cap_alone() -> meclaw_core::JsonValue {
    json!({"sandbox": {
        "trust": "restricted",
        "network": "allow",
        "limits": {"memory_max_bytes": 2_000_000_000u64, "cpu_max_percent": 200, "pids_max": 512}
    }})
}

#[test]
fn a_cap_alone_is_a_restricted_profile() {
    let p = SandboxProfile::parse(&the_cap_alone())
        .expect("the third shape parses")
        .expect("a profile");
    match p {
        SandboxProfile::Restricted {
            network,
            filesystem,
            limits,
            syscalls,
        } => {
            assert_eq!(network, NetworkPolicy::Allow);
            assert!(
                filesystem.is_none(),
                "no view: the package's own confinement is the view"
            );
            assert!(
                syscalls.is_none(),
                "no filter: a filter would need the flag that stops the launcher"
            );
            let caps = limits.expect("the cap is the whole profile");
            assert_eq!(caps.memory_max_bytes, Some(2_000_000_000));
            assert_eq!(caps.cpu_max_percent, Some(200));
            assert_eq!(caps.pids_max, Some(512));
        }
        other => panic!("expected a restricted profile, got {other:?}"),
    }
}

#[test]
fn a_restricted_profile_that_caps_nothing_is_refused() {
    let e = SandboxProfile::parse(&json!({"sandbox": {
        "trust": "restricted",
        "network": "allow"
    }}))
    .expect_err("a profile that restricts nothing is not restricted");
    assert!(
        e.contains("params.sandbox.filesystem") && e.contains("limits"),
        "the refusal must name both ways out: {e}"
    );
}

#[test]
fn syscalls_without_a_filesystem_view_is_still_refused() {
    let e = SandboxProfile::parse(&json!({"sandbox": {
        "trust": "restricted",
        "limits": {"pids_max": 64},
        "syscalls": {"ptrace": "deny"}
    }}))
    .expect_err("a filter without a view is half a boundary");
    assert!(
        e.contains("params.sandbox.filesystem") && e.contains("syscalls"),
        "the refusal must say which key made the view mandatory: {e}"
    );
}

#[test]
fn the_two_older_shapes_are_untouched() {
    // A view alone, network defaulting to deny.
    let p = SandboxProfile::parse(&json!({"sandbox": {
        "trust": "restricted",
        "filesystem": {"read": ["/srv/data"], "write": ["/srv/work"]}
    }}))
    .expect("parses")
    .expect("a profile");
    match p {
        SandboxProfile::Restricted {
            network,
            filesystem,
            limits,
            ..
        } => {
            assert_eq!(network, NetworkPolicy::Deny);
            let fs = filesystem.expect("the view is there");
            assert_eq!(fs.read, vec![std::path::PathBuf::from("/srv/data")]);
            assert!(fs.runtime, "the runtime set still defaults on");
            assert!(limits.is_none());
        }
        other => panic!("expected a restricted profile, got {other:?}"),
    }
    // And the escape hatch.
    assert!(matches!(
        SandboxProfile::parse(&json!({"sandbox": {"trust": "trusted"}}))
            .expect("parses")
            .expect("a profile"),
        SandboxProfile::Trusted
    ));
}

/// Applied, the third shape sets the cap and NOTHING else.
///
/// `NoNewPrivs: 0` in the child's `/proc/<pid>/status` is the whole assertion:
/// it is the flag that stops a packaged browser's launcher, and it must not be
/// set for a profile that asks for neither Landlock nor seccomp. Read through
/// the pid this test itself remembers — never a name pattern, because this host
/// runs other people's processes.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cap_alone_sets_the_cap_and_no_flag() {
    let test = "the_cap_alone_sets_the_cap_and_no_flag";
    if !sandbox::cgroup_delegation_supported() {
        println!(
            "[{test}] SKIPPED: no usable cgroup v2 delegation on this host (root {:?}); \
             retry under `systemd-run --user --scope`",
            sandbox::delegated_root()
        );
        return;
    }
    // Said on the way IN as well, so a run of this file shows whether the
    // measurement happened. `.config/nextest.toml` turns the output of these
    // two tests on even when they pass, because a measurement that can quietly
    // not happen is not a measurement (review I13).
    println!("[{test}] MEASURED: cgroup v2 delegation is available on this host");
    let profile = SandboxProfile::parse(&the_cap_alone())
        .expect("parses")
        .expect("a profile");
    let spec = meclaw_cells::stdio_child::ChildSpec {
        // `cat` holds stdin open and needs no interpreter of its own, so the
        // child lives long enough to be read and dies with its pipes.
        program: "/bin/cat".to_string(),
        kill_grace_ms: 500,
        sandbox: Some(Box::new(profile)),
        ..Default::default()
    };
    let child = StdioChildUnderTest::spawn(&spec);
    let pid = child.pid().expect("the child is running");
    let status =
        std::fs::read_to_string(format!("/proc/{pid}/status")).expect("the child's status");
    let flag = status
        .lines()
        .find(|l| l.starts_with("NoNewPrivs:"))
        .expect("the kernel reports the flag");
    assert!(
        flag.split_whitespace().nth(1) == Some("0"),
        "a profile that asks for neither Landlock nor seccomp must not set \
         no_new_privs — it is what stops a packaged browser's launcher: {flag}"
    );
    let cgroup =
        std::fs::read_to_string(format!("/proc/{pid}/cgroup")).expect("the child's cgroup");
    assert!(
        cgroup.contains("meclaw"),
        "and the cap IS applied: the child sits in the substrate's sub-cgroup, not ours: {cgroup}"
    );
    child.terminate().await;
}

/// And `network: "deny"` still bites without the flag.
///
/// The measurement point of R-G13: `no_new_privs` moved to Landlock and
/// seccomp, and the question that leaves open is whether the third mechanism —
/// `unshare(CLONE_NEWUSER|CLONE_NEWNET)` — was leaning on it. It was not, and
/// this is where that is written down rather than assumed: a child under a cap
/// and a network denial sees one interface, `lo`, and this process sees more.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_network_still_bites_without_the_flag() {
    let test = "a_denied_network_still_bites_without_the_flag";
    if !sandbox::cgroup_delegation_supported() {
        println!("[{test}] SKIPPED: no usable cgroup v2 delegation on this host");
        return;
    }
    if !sandbox::network_isolation_supported() {
        println!("[{test}] SKIPPED: unprivileged network namespaces unavailable on this host");
        return;
    }
    // The measurement point of R-G13 says so on the way in, so the gate log
    // carries the proof that it ran (review I13).
    println!("[{test}] MEASURED: cgroup delegation and unprivileged netns both available");
    let profile = SandboxProfile::parse(&json!({"sandbox": {
        "trust": "restricted",
        "network": "deny",
        "limits": {"pids_max": 64}
    }}))
    .expect("parses")
    .expect("a profile");
    let spec = meclaw_cells::stdio_child::ChildSpec {
        program: "/bin/cat".to_string(),
        kill_grace_ms: 500,
        sandbox: Some(Box::new(profile)),
        ..Default::default()
    };
    let child = StdioChildUnderTest::spawn(&spec);
    let pid = child.pid().expect("the child is running");
    let theirs = std::fs::read_to_string(format!("/proc/{pid}/net/dev")).expect("the child's net");
    let ours = std::fs::read_to_string("/proc/self/net/dev").expect("our own net");
    let count = |s: &str| s.lines().skip(2).filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        count(&theirs),
        1,
        "a denied network is one interface and nothing else, flag or no flag: {theirs}"
    );
    assert!(theirs.contains("lo:"), "and it is the loopback: {theirs}");
    assert!(
        count(&ours) > 1,
        "the control: this process is not in that namespace: {ours}"
    );
    child.terminate().await;
}

/// A thin wrapper so the spawn and the teardown read as one thing in the test
/// above, and so the child is reaped through the pid this test remembers.
///
/// Both tests are `#[tokio::test]` and not `#[test]`: `StdioChild::spawn`
/// reaches `tokio::process`, which panics with "there is no reactor running"
/// outside a runtime. Measured 19.09. under `systemd-run --user --scope` —
/// which is the only way the two arms above get past their early exit, and
/// therefore the only way that panic was ever reachable (review I13).
#[cfg(target_os = "linux")]
struct StdioChildUnderTest(meclaw_cells::stdio_child::StdioChild);

#[cfg(target_os = "linux")]
impl StdioChildUnderTest {
    fn spawn(spec: &meclaw_cells::stdio_child::ChildSpec) -> Self {
        Self(meclaw_cells::stdio_child::StdioChild::spawn(spec).expect("the child starts"))
    }
    fn pid(&self) -> Option<u32> {
        self.0.pid()
    }
    async fn terminate(self) {
        self.0
            .terminate(std::time::Duration::from_millis(500))
            .await;
    }
}
