//! The host-capability report: which `params.sandbox` properties this machine
//! can actually enforce, asked without running a cell (GH #97).
//!
//! The sandbox is fail-closed, so the first time an operator learns that a
//! host cannot enforce something used to be a production `io_error` carrying
//! `sandbox not applied: ...`. The four probes that answer the question have
//! existed since GH #85 -- the tests skip on them -- they simply had no
//! surface. This module is that surface: it runs them, names each one after
//! the `params.sandbox` key it decides, and renders one line per property.
//!
//! Two of the four answer by SPAWNING `/bin/sh -c :`, because a kernel knob is
//! not the whole answer. Whether they may run is the caller's decision
//! ([`SpawningProbes`]): `--sandbox-probe` is an explicit request for
//! diagnostics and runs them; the `--validate` appendix runs them only when the
//! tree declares a `restricted` profile at all, so a plain configuration check
//! never spawns anything.
//!
//! The sharpest of the four is `limits`. Its answer is not a property of the
//! kernel but of how the daemon was STARTED, so a bare "no" would send an
//! operator hunting for a kernel feature that is already there. See
//! [`CgroupDelegation`].

use std::fmt;

/// The `params.sandbox` properties this report covers, in the order it prints
/// them. Each name is the key an operator writes in `config.json`.
pub const PROBE_NAMES: [&str; 4] = ["filesystem", "network", "limits", "syscalls"];

/// The first line of every rendered report. Present on both the
/// `--sandbox-probe` and the `--validate` path, so a reader of either output
/// knows what the block below it is.
pub const REPORT_HEADER: &str =
    "sandbox probe: which params.sandbox properties this host can enforce";

/// The answer of one probe. Closed set -- an operator's grep must not have to
/// guess at a fourth spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The property can be enforced on this host.
    Yes,
    /// It cannot. The detail column says why, and what would change it.
    No,
    /// The probe was not run. Only the two spawning probes can end up here,
    /// and only when the caller withheld permission to spawn.
    Skipped,
}

impl Verdict {
    /// The printed word. `yes`, `no` or `skipped`.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
            Verdict::Skipped => "skipped",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One property, its verdict, and the reason behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The `params.sandbox` key this line decides.
    pub name: &'static str,
    /// Whether this host can enforce it.
    pub verdict: Verdict,
    /// Why -- an operator-readable sentence, never empty.
    pub detail: String,
}

/// What the cgroup delegation probe found.
///
/// [`super::cgroup_delegation_supported`] is the boolean shadow of this: it is
/// `true` for exactly [`CgroupDelegation::Delegated`]. The variants exist
/// because the two ways of failing want completely different actions from the
/// operator, and a bare `false` conflates them:
///
/// * [`CgroupDelegation::NoDelegatedRoot`] is the host saying the MECHANISM is
///   not there -- no cgroup v2 unified hierarchy, or no controller-enabled
///   directory this uid may write into. Nothing about the daemon's launch
///   changes that.
/// * [`CgroupDelegation::MoveRefused`] with `permission_denied` is the host
///   saying the mechanism IS there and the LAUNCH is wrong. Measured on
///   Ubuntu: creating a sub-cgroup under `user@<uid>.service` succeeds from
///   anywhere, but moving a process into it also needs write access to
///   `cgroup.procs` of the common ancestor of source and destination. A daemon
///   started from an ssh login lives in a root-owned `session-<n>.scope`, so
///   that ancestor is the root-owned `user-<uid>.slice` and the move fails with
///   `EACCES`. The same daemon started as a systemd user unit succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupDelegation {
    /// The whole sequence worked: sub-cgroup created, capped, entered by a real
    /// child, removed again.
    Delegated {
        /// The delegated root the sub-cgroup was created under.
        root: String,
    },
    /// No writable, controller-enabled cgroup v2 directory exists for this uid.
    NoDelegatedRoot,
    /// A root was found, but the sub-cgroup could not be created or capped.
    SetupFailed {
        /// The underlying error, verbatim.
        reason: String,
    },
    /// The sub-cgroup was created, but moving a real child into it was refused.
    MoveRefused {
        /// Whether the kernel answered `EACCES`/`EPERM` -- the launch-shaped
        /// failure rather than an arbitrary one.
        permission_denied: bool,
        /// The underlying error, verbatim.
        reason: String,
    },
}

/// Whether the two probes that spawn `/bin/sh -c :` may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawningProbes {
    /// Run them. What `--sandbox-probe` passes: the operator asked.
    Run,
    /// Do not run them; the given reason lands in both detail columns. What
    /// the `--validate` appendix passes when nothing in the tree asks for a
    /// restricted profile -- a configuration check spawns nothing without cause.
    Skip(String),
}

/// The measured halves of the report that need a child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawningOutcome {
    /// Both spawning probes ran.
    Measured {
        /// Whether an unprivileged network namespace could be entered.
        network: bool,
        /// What the cgroup delegation sequence found.
        cgroup: CgroupDelegation,
    },
    /// Neither ran. The reason is printed instead of a verdict.
    Skipped(String),
}

/// The finished report: exactly the four properties of [`PROBE_NAMES`], in
/// that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProbeReport {
    probes: Vec<Probe>,
}

impl SandboxProbeReport {
    /// The four lines, in print order.
    pub fn probes(&self) -> &[Probe] {
        &self.probes
    }

    /// The report as text: a header line, then one indented line per property.
    /// Ends with a newline, so it can be printed as-is.
    pub fn render(&self) -> String {
        let mut out = String::from(REPORT_HEADER);
        out.push('\n');
        for p in &self.probes {
            out.push_str(&format!(
                "  {:<10}  {:<7}  {}\n",
                p.name,
                p.verdict.as_str(),
                p.detail
            ));
        }
        out
    }
}

/// Run every probe this host allows and render the answers.
///
/// Blocking, and two of the four fork a child: this belongs on a validation
/// path or in a test, never in an async hot path.
pub fn probe_host(spawning: &SpawningProbes) -> SandboxProbeReport {
    let outcome = match spawning {
        SpawningProbes::Run => SpawningOutcome::Measured {
            network: super::network_isolation_supported(),
            cgroup: super::delegation_probe(),
        },
        SpawningProbes::Skip(reason) => SpawningOutcome::Skipped(reason.clone()),
    };
    report_from(super::landlock_abi(), super::seccomp_supported(), &outcome)
}

/// Assemble the report out of already-measured values.
///
/// The seam the formatting tests use: every branch -- including both halves of
/// the cgroup answer -- is reachable without owning the host that produces it.
pub fn report_from(
    landlock_abi: Option<u32>,
    seccomp: bool,
    spawning: &SpawningOutcome,
) -> SandboxProbeReport {
    let (network_probe, limits_probe) = match spawning {
        SpawningOutcome::Measured { network, cgroup } => {
            (network_line(*network), limits_line(cgroup))
        }
        SpawningOutcome::Skipped(reason) => (
            skipped_line("network", reason),
            skipped_line("limits", reason),
        ),
    };
    SandboxProbeReport {
        probes: vec![
            filesystem_line(landlock_abi),
            network_probe,
            limits_probe,
            syscalls_line(seccomp),
        ],
    }
}

/// `params.sandbox.filesystem` -- Landlock.
fn filesystem_line(abi: Option<u32>) -> Probe {
    match abi {
        Some(abi) => Probe {
            name: "filesystem",
            verdict: Verdict::Yes,
            detail: format!("Landlock ABI {abi}"),
        },
        None => Probe {
            name: "filesystem",
            verdict: Verdict::No,
            detail: "this kernel has no Landlock (needs Linux 5.13+ with the LSM enabled); a \
                     restricted profile is refused rather than run unsandboxed"
                .to_string(),
        },
    }
}

/// `params.sandbox.network` -- an unprivileged network namespace.
fn network_line(supported: bool) -> Probe {
    if supported {
        Probe {
            name: "network",
            verdict: Verdict::Yes,
            detail: "an unprivileged CLONE_NEWUSER|CLONE_NEWNET child ran".to_string(),
        }
    } else {
        Probe {
            name: "network",
            verdict: Verdict::No,
            detail: "this host refuses an unprivileged network namespace (a kernel knob, a \
                     container runtime or an LSM policy can each say no); network \"deny\" \
                     cannot be enforced"
                .to_string(),
        }
    }
}

/// `params.sandbox.syscalls` -- seccomp-bpf.
fn syscalls_line(supported: bool) -> Probe {
    if supported {
        Probe {
            name: "syscalls",
            verdict: Verdict::Yes,
            detail: "seccomp filter mode is present for this architecture".to_string(),
        }
    } else {
        Probe {
            name: "syscalls",
            verdict: Verdict::No,
            detail: "no seccomp filter mode for this architecture and kernel; the syscalls \
                     block cannot be enforced"
                .to_string(),
        }
    }
}

/// `params.sandbox.limits` -- a delegated cgroup v2 sub-cgroup.
///
/// The one line that must name the LAUNCH requirement instead of saying "no":
/// on a host whose kernel is entirely capable, a daemon started from an ssh
/// login cannot move a process, and only a different launch fixes it.
fn limits_line(delegation: &CgroupDelegation) -> Probe {
    let (verdict, detail) = match delegation {
        CgroupDelegation::Delegated { root } => (
            Verdict::Yes,
            format!("a real child was capped and moved into a sub-cgroup under {root}"),
        ),
        CgroupDelegation::NoDelegatedRoot => (
            Verdict::No,
            "this host delegates no writable cgroup v2 directory to this uid (no cgroup v2 \
             unified hierarchy, or no directory with controllers handed down); the mechanism \
             is absent, not the permission"
                .to_string(),
        ),
        CgroupDelegation::SetupFailed { reason } => (
            Verdict::No,
            format!(
                "a delegated root exists but the sub-cgroup could not be created or capped: \
                 {reason}"
            ),
        ),
        CgroupDelegation::MoveRefused {
            permission_denied: true,
            reason,
        } => (
            Verdict::No,
            format!(
                "the sub-cgroup was created but moving a child into it was refused ({reason}). \
                 The kernel can do this, the launch cannot: the daemon must run as a systemd \
                 user unit (user@<uid>.service); an ssh session scope cannot move processes, \
                 because the common ancestor user-<uid>.slice is root-owned"
            ),
        ),
        CgroupDelegation::MoveRefused {
            permission_denied: false,
            reason,
        } => (
            Verdict::No,
            format!("the sub-cgroup was created but moving a child into it failed: {reason}"),
        ),
    };
    Probe {
        name: "limits",
        verdict,
        detail,
    }
}

/// A probe that was deliberately not run.
fn skipped_line(name: &'static str, reason: &str) -> Probe {
    Probe {
        name,
        verdict: Verdict::Skipped,
        detail: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(network: bool, cgroup: CgroupDelegation) -> SpawningOutcome {
        SpawningOutcome::Measured { network, cgroup }
    }

    #[test]
    fn the_report_covers_every_property_in_a_fixed_order() {
        // Completeness is the whole point: an operator who reads the report
        // must not have to know which of the four the surface forgot.
        let r = report_from(
            Some(5),
            true,
            &measured(true, CgroupDelegation::NoDelegatedRoot),
        );
        let names: Vec<&str> = r.probes().iter().map(|p| p.name).collect();
        assert_eq!(names, PROBE_NAMES.to_vec());
    }

    #[test]
    fn every_line_carries_a_verdict_from_the_closed_set_and_a_reason() {
        let r = report_from(
            None,
            false,
            &measured(false, CgroupDelegation::NoDelegatedRoot),
        );
        for p in r.probes() {
            assert!(
                ["yes", "no", "skipped"].contains(&p.verdict.as_str()),
                "unexpected verdict word {:?}",
                p.verdict.as_str()
            );
            assert!(!p.detail.is_empty(), "{} has no reason", p.name);
        }
    }

    #[test]
    fn a_supported_host_says_yes_on_all_four() {
        let r = report_from(
            Some(6),
            true,
            &measured(
                true,
                CgroupDelegation::Delegated {
                    root: "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service".into(),
                },
            ),
        );
        assert!(r.probes().iter().all(|p| p.verdict == Verdict::Yes));
        assert!(
            r.probes()[0].detail.contains("Landlock ABI 6"),
            "the filesystem line reports the measured ABI"
        );
    }

    // ---- the cgroup line: the two failures an operator must not confuse ----

    #[test]
    fn a_refused_move_names_the_launch_requirement_not_just_no() {
        // GH #97, the sharpest case: the kernel is fine, the LAUNCH is wrong.
        // "no" alone sends the operator hunting for a kernel feature that is
        // already there.
        let p = limits_line(&CgroupDelegation::MoveRefused {
            permission_denied: true,
            reason: "Permission denied (os error 13)".into(),
        });
        assert_eq!(p.verdict, Verdict::No);
        assert!(
            p.detail.contains("systemd user unit") && p.detail.contains("user@<uid>.service"),
            "the detail must name the launch requirement, was: {}",
            p.detail
        );
        assert!(
            p.detail.contains("ssh session scope"),
            "and the launch that fails, was: {}",
            p.detail
        );
    }

    #[test]
    fn an_absent_mechanism_says_so_and_does_not_blame_the_launch() {
        // The other half of the same distinction: here a different launch
        // would change nothing, so naming systemd would be a false lead.
        let p = limits_line(&CgroupDelegation::NoDelegatedRoot);
        assert_eq!(p.verdict, Verdict::No);
        assert!(
            p.detail.contains("cgroup v2"),
            "the detail must name the absent mechanism, was: {}",
            p.detail
        );
        assert!(
            !p.detail.contains("systemd user unit"),
            "an absent mechanism must NOT be reported as a launch problem, was: {}",
            p.detail
        );
    }

    #[test]
    fn a_non_permission_move_failure_stays_verbatim() {
        let p = limits_line(&CgroupDelegation::MoveRefused {
            permission_denied: false,
            reason: "No such device (os error 19)".into(),
        });
        assert_eq!(p.verdict, Verdict::No);
        assert!(p.detail.contains("No such device"));
        assert!(
            !p.detail.contains("systemd user unit"),
            "only the permission-denied branch names the launch, was: {}",
            p.detail
        );
    }

    #[test]
    fn a_setup_failure_is_its_own_answer() {
        let p = limits_line(&CgroupDelegation::SetupFailed {
            reason: "does not hand the memory controller down".into(),
        });
        assert_eq!(p.verdict, Verdict::No);
        assert!(p.detail.contains("memory controller"));
    }

    // ---- withheld spawns ----

    #[test]
    fn withheld_spawns_produce_skipped_lines_with_the_reason() {
        // The `--validate` appendix must not fork a child when nothing in the
        // tree asks to be sandboxed -- but it still owes the reader a line.
        let r = report_from(
            Some(5),
            true,
            &SpawningOutcome::Skipped("no restricted profile in tree".into()),
        );
        let by_name = |n: &str| {
            r.probes()
                .iter()
                .find(|p| p.name == n)
                .expect("every name is present")
                .clone()
        };
        for n in ["network", "limits"] {
            let p = by_name(n);
            assert_eq!(p.verdict, Verdict::Skipped, "{n} must not have been run");
            assert_eq!(p.detail, "no restricted profile in tree");
        }
        // The two side-effect-free probes still answer.
        assert_ne!(by_name("filesystem").verdict, Verdict::Skipped);
        assert_ne!(by_name("syscalls").verdict, Verdict::Skipped);
    }

    // ---- rendering ----

    #[test]
    fn the_rendering_is_a_header_plus_one_line_per_property() {
        let r = report_from(
            Some(5),
            true,
            &measured(true, CgroupDelegation::NoDelegatedRoot),
        );
        let text = r.render();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "header + four properties, was: {text}");
        assert_eq!(lines[0], REPORT_HEADER);
        for (line, name) in lines[1..].iter().zip(PROBE_NAMES) {
            assert!(
                line.starts_with("  "),
                "a property line is indented under the header, was: {line}"
            );
            let mut words = line.split_whitespace();
            assert_eq!(words.next(), Some(name));
            assert!(
                matches!(words.next(), Some("yes") | Some("no") | Some("skipped")),
                "the verdict follows the name, was: {line}"
            );
            assert!(
                words.next().is_some(),
                "and the reason follows the verdict, was: {line}"
            );
        }
        assert!(text.ends_with('\n'), "printable as-is");
    }

    #[test]
    fn the_report_of_this_host_is_complete_whatever_it_answers() {
        // The verdicts are host-dependent and are NOT pinned; the form and the
        // completeness are. Runs the real probes, spawns included.
        let r = probe_host(&SpawningProbes::Run);
        let names: Vec<&str> = r.probes().iter().map(|p| p.name).collect();
        assert_eq!(names, PROBE_NAMES.to_vec());
        for p in r.probes() {
            assert_ne!(
                p.verdict,
                Verdict::Skipped,
                "{} must actually run when spawning is allowed",
                p.name
            );
            assert!(!p.detail.is_empty());
        }
    }
}
