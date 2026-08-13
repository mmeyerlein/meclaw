//! The syscall filter: seccomp-bpf, assembled in this tree (GH #85).
//!
//! # Why the program is built here and not pulled in
//!
//! The tech stack is a closed allow-list (hard rule 6), so a seccomp crate is
//! a dependency question, not a shortcut. It is also not needed: a seccomp
//! filter is a classic-BPF program, and classic BPF is forty instructions of
//! four fields each. Phase 1 spoke to Landlock the same way, through
//! `libc::syscall` on a hand-built `struct`. The whole program this module
//! emits fits on one screen and is asserted instruction by instruction below.
//!
//! # What it closes, and what it deliberately does not
//!
//! Landlock is a filesystem LSM. It says nothing about `ptrace`, nothing about
//! signals to other processes of the same user, and nothing about raw sockets.
//! Those three are the axes of [`SyscallPolicy`], and each is a separate
//! `deny`.
//!
//! The signal rule is "self only", not "own process group". A BPF program
//! cannot consult the process table, so it can compare the target pid against
//! one constant and nothing else; the constant is this process's own pid,
//! patched in after the fork. `kill(0, ...)` -- the whole process group, which
//! for a cell's child is the DAEMON's group -- is therefore denied too, and so
//! is `kill(-1, ...)`. What stays open is `kill(self)` and
//! `tgkill(self, tid)`, which is what `raise()` and `abort()` compile down to.
//! The cost is that a shell script under this axis cannot `kill $!` its own
//! background job. That is a real restriction and it is documented as one; the
//! alternative reading, "allow every positive pid", would protect nothing.
//!
//! A denial is `SECCOMP_RET_ERRNO | EPERM`, not a kill: the sandboxed program
//! sees an ordinary permission error and can report it, which is what makes
//! the boundary debuggable. Only an architecture mismatch kills the process,
//! because a filter keyed to the wrong syscall table is not a filter.

use super::profile::SyscallPolicy;
use std::io;

// ---- seccomp uapi ---------------------------------------------------------

/// `seccomp(2)` operation: install a classic-BPF filter.
const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;

/// Let the syscall through.
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
/// Fail the syscall with the errno in the low 16 bits, without running it.
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
/// End the whole process. Reserved for an architecture mismatch.
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;

/// `AUDIT_ARCH_X86_64`.
#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
/// `AUDIT_ARCH_AARCH64`.
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;

/// The first x32 syscall number on x86_64. An x32 caller reaches the same
/// syscall table through different numbers, so anything from here up is a way
/// around a filter keyed to the 64-bit numbers.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

// ---- classic BPF ----------------------------------------------------------

/// `BPF_LD | BPF_W | BPF_ABS`: load a 32-bit word from `struct seccomp_data`.
const BPF_LD_W_ABS: u16 = 0x20;
/// `BPF_ALU | BPF_AND | BPF_K`.
const BPF_AND_K: u16 = 0x54;
/// `BPF_JMP | BPF_JEQ | BPF_K`.
const BPF_JEQ_K: u16 = 0x15;
/// `BPF_JMP | BPF_JGE | BPF_K`.
const BPF_JGE_K: u16 = 0x35;
/// `BPF_JMP | BPF_JA`.
const BPF_JA: u16 = 0x05;
/// `BPF_RET | BPF_K`.
const BPF_RET_K: u16 = 0x06;

/// Offset of `seccomp_data.nr`.
const OFF_NR: u32 = 0;
/// Offset of `seccomp_data.arch`.
const OFF_ARCH: u32 = 4;
/// Offset of the low half of `seccomp_data.args[n]` on a little-endian host.
const fn off_arg_lo(n: u32) -> u32 {
    16 + 8 * n
}
/// Offset of the high half of `seccomp_data.args[n]`.
const fn off_arg_hi(n: u32) -> u32 {
    off_arg_lo(n) + 4
}

/// `struct sock_filter`, one classic-BPF instruction.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

/// `struct sock_fprog`, the program handed to the kernel.
#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

/// Symbolic jump targets, resolved once the terminal instructions have an
/// index. Real offsets in this program never come close to these values -- the
/// longest program it emits is well under a hundred instructions -- and
/// [`Filter::build`] asserts that no placeholder survives resolution.
const L_ALLOW: u8 = 250;
const L_DENY: u8 = 251;
const L_KILL: u8 = 252;

// ---- assembly -------------------------------------------------------------

/// Load a word from `struct seccomp_data`.
fn ld(offset: u32) -> SockFilter {
    SockFilter {
        code: BPF_LD_W_ABS,
        jt: 0,
        jf: 0,
        k: offset,
    }
}

/// `A &= mask`.
fn and(mask: u32) -> SockFilter {
    SockFilter {
        code: BPF_AND_K,
        jt: 0,
        jf: 0,
        k: mask,
    }
}

/// Branch on `A == k`.
fn jeq(k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter {
        code: BPF_JEQ_K,
        jt,
        jf,
        k,
    }
}

/// Branch on `A >= k`.
fn jge(k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter {
        code: BPF_JGE_K,
        jt,
        jf,
        k,
    }
}

/// Unconditional forward jump, `k` instructions ahead.
fn ja(k: u32) -> SockFilter {
    SockFilter {
        code: BPF_JA,
        jt: 0,
        jf: 0,
        k,
    }
}

/// Return a seccomp action.
fn ret(action: u32) -> SockFilter {
    SockFilter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: action,
    }
}

// ---- the filter -----------------------------------------------------------

/// A built, not yet installed seccomp program.
///
/// Built in the parent so that everything which can fail with a readable
/// reason fails before a child exists. The one value it cannot know yet is the
/// child's pid, so the instruction holding it is recorded by index and patched
/// in after the fork -- a single store into an already allocated vector, which
/// is what keeps [`Filter::install`] allocation-free.
#[derive(Debug)]
pub(crate) struct Filter {
    prog: Vec<SockFilter>,
    /// Indices of the instructions whose `k` is the sandboxed process's pid.
    pid_slots: Vec<usize>,
}

impl Filter {
    /// Assemble the program for `policy`.
    pub(crate) fn build(policy: &SyscallPolicy) -> io::Result<Self> {
        if !arch_known() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "params.sandbox.syscalls asks for a seccomp filter, but this build targets an \
                 architecture whose AUDIT_ARCH this substrate does not know (x86_64 and aarch64 \
                 are handled); refusing to run the cell unfiltered",
            ));
        }
        let mut prog: Vec<SockFilter> = Vec::new();
        let mut pid_slots: Vec<usize> = Vec::new();

        // The architecture gate comes first and it kills rather than refuses: a
        // program keyed to the wrong syscall table would deny and allow the
        // wrong things, which is worse than not running at all.
        prog.push(ld(OFF_ARCH));
        prog.push(jeq(AUDIT_ARCH, 0, L_KILL));
        prog.push(ld(OFF_NR));
        prog.push(jge(X32_SYSCALL_BIT, L_KILL, 0));

        if policy.deny_ptrace {
            emit_ptrace(&mut prog);
        }
        if policy.deny_raw_sockets {
            emit_raw_sockets(&mut prog);
        }
        if policy.deny_foreign_signals {
            emit_foreign_signals(&mut prog, &mut pid_slots);
        }

        // Terminals, in the order the placeholders below expect them.
        let allow_idx = prog.len();
        prog.push(ret(SECCOMP_RET_ALLOW));
        prog.push(ret(SECCOMP_RET_ERRNO | (libc::EPERM as u32)));
        prog.push(ret(SECCOMP_RET_KILL_PROCESS));

        for (i, insn) in prog.iter_mut().enumerate() {
            insn.jt = resolve(insn.jt, i, allow_idx);
            insn.jf = resolve(insn.jf, i, allow_idx);
        }
        debug_assert!(
            prog.iter().all(|i| i.jt < L_ALLOW && i.jf < L_ALLOW),
            "an unresolved placeholder would jump into nowhere"
        );
        if prog.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the assembled seccomp program is longer than a sock_fprog can describe",
            ));
        }
        Ok(Self { prog, pid_slots })
    }

    /// Patch in the pid and install the filter on the calling thread and every
    /// child it goes on to have.
    ///
    /// Runs post-fork: stores into an existing vector and one syscall, no
    /// allocation and no formatting. `PR_SET_NO_NEW_PRIVS` must already be set,
    /// which [`super::apply`] does before it gets here.
    pub(crate) fn install(&mut self) -> io::Result<()> {
        // SAFETY: `getpid` reads a field of the calling process and cannot
        // fail. After `fork` this is already the child's pid, and `execve`
        // keeps it, so the constant is right for the program that will run.
        let pid = unsafe { libc::getpid() } as u32;
        for &slot in &self.pid_slots {
            self.prog[slot].k = pid;
        }
        let fprog = SockFprog {
            len: self.prog.len() as u16,
            filter: self.prog.as_ptr(),
        };
        // SAFETY: `fprog` describes `self.prog`, which is live for the duration
        // of the call, and its length matches exactly. The return value is
        // checked.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_SET_MODE_FILTER,
                0 as libc::c_ulong,
                &fprog as *const SockFprog,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Turn a symbolic jump target into the relative offset the kernel wants.
///
/// Classic BPF counts from the instruction AFTER the branch, so the arithmetic
/// is `target - (i + 1)`. A value that is not a placeholder is already a real
/// offset and passes through untouched.
fn resolve(slot: u8, i: usize, allow_idx: usize) -> u8 {
    let target = match slot {
        L_ALLOW => allow_idx,
        L_DENY => allow_idx + 1,
        L_KILL => allow_idx + 2,
        other => return other,
    };
    (target - (i + 1)) as u8
}

/// Whether this build knows its own `AUDIT_ARCH`.
const fn arch_known() -> bool {
    cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))
}

/// `ptrace` and the two syscalls that read and write another process's memory
/// without it. They are the same capability under three names.
fn emit_ptrace(prog: &mut Vec<SockFilter>) {
    prog.push(ld(OFF_NR));
    for nr in [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
    ] {
        prog.push(jeq(nr as u32, L_DENY, 0));
    }
}

/// `AF_PACKET` of any type, and `SOCK_RAW` of any family.
///
/// The type argument carries `SOCK_NONBLOCK` and `SOCK_CLOEXEC` in its high
/// bits, so it is masked down to the type itself before the comparison --
/// without the mask a `SOCK_RAW | SOCK_CLOEXEC` would walk straight past.
fn emit_raw_sockets(prog: &mut Vec<SockFilter>) {
    prog.push(ld(OFF_NR));
    let skip_at = prog.len();
    prog.push(jeq(libc::SYS_socket as u32, 0, 0));
    prog.push(ld(off_arg_lo(0)));
    prog.push(jeq(libc::AF_PACKET as u32, L_DENY, 0));
    prog.push(ld(off_arg_lo(1)));
    prog.push(and(0xf));
    prog.push(jeq(libc::SOCK_RAW as u32, L_DENY, 0));
    // Not a `socket` call: jump past the block, which also skips the argument
    // loads that would otherwise leave `A` holding something other than `nr`.
    prog[skip_at].jf = (prog.len() - (skip_at + 1)) as u8;
}

/// Every signal whose target is not this very process.
///
/// The four syscalls below all carry the target pid or tgid in argument 0, so
/// they share one check. `tkill` names a thread id with no tgid to check it
/// against and `pidfd_send_signal` names a descriptor the filter cannot read,
/// so both are refused outright.
fn emit_foreign_signals(prog: &mut Vec<SockFilter>, pid_slots: &mut Vec<usize>) {
    prog.push(ld(OFF_NR));
    let mut to_check: Vec<usize> = Vec::new();
    for nr in [
        libc::SYS_kill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
    ] {
        to_check.push(prog.len());
        prog.push(jeq(nr as u32, 0, 0));
    }
    for nr in [libc::SYS_tkill, libc::SYS_pidfd_send_signal] {
        prog.push(jeq(nr as u32, L_DENY, 0));
    }
    let skip_at = prog.len();
    prog.push(ja(0));

    let check_idx = prog.len();
    // A pid whose high word is set is not a pid at all, and `-1` (signal
    // everything this user owns) arrives exactly that way.
    prog.push(ld(off_arg_hi(0)));
    prog.push(jeq(0, 0, L_DENY));
    prog.push(ld(off_arg_lo(0)));
    pid_slots.push(prog.len());
    prog.push(jeq(0, L_ALLOW, L_DENY));

    for i in to_check {
        prog[i].jt = (check_idx - (i + 1)) as u8;
    }
    prog[skip_at].k = (prog.len() - (skip_at + 1)) as u32;
}

// ---- feature detection ----------------------------------------------------

/// Whether this kernel and this build can install a seccomp filter.
///
/// Cheap and side-effect free: `seccomp(SECCOMP_SET_MODE_FILTER, 0, NULL)`
/// installs nothing. A kernel that has filter mode gets as far as copying the
/// program and answers `EFAULT` for the null pointer, or `EACCES` when
/// `PR_SET_NO_NEW_PRIVS` is not set on the probing process -- either answer
/// proves the mode exists. A kernel without it answers `EINVAL`, and one
/// without the syscall at all answers `ENOSYS`.
pub fn seccomp_supported() -> bool {
    if !arch_known() {
        return false;
    }
    // SAFETY: the probe passes a NULL program pointer by design; the kernel
    // reads no memory from this process on the failing path.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            0 as libc::c_ulong,
            std::ptr::null::<SockFprog>(),
        )
    };
    if rc == 0 {
        // Cannot happen with a null program, but a kernel that says yes is a
        // kernel that supports the mode.
        return true;
    }
    matches!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::EFAULT) | Some(libc::EACCES)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_denied() -> SyscallPolicy {
        SyscallPolicy {
            deny_ptrace: true,
            deny_raw_sockets: true,
            deny_foreign_signals: true,
        }
    }

    /// Walk the program the way the kernel's verifier does, so a jump into
    /// nowhere is caught here rather than as an `EINVAL` at spawn time.
    fn assert_jumps_land_inside(prog: &[SockFilter]) {
        for (i, insn) in prog.iter().enumerate() {
            match insn.code {
                BPF_JEQ_K | BPF_JGE_K => {
                    for (arm, off) in [("true", insn.jt as usize), ("false", insn.jf as usize)] {
                        assert!(
                            i + 1 + off < prog.len(),
                            "instruction {i} jumps {arm} to {} of {}",
                            i + 1 + off,
                            prog.len()
                        );
                    }
                }
                BPF_JA => assert!(
                    i + 1 + (insn.k as usize) < prog.len(),
                    "instruction {i} jumps unconditionally out of the program"
                ),
                // Loads, ALU ops and returns do not branch; their jt/jf are
                // ignored by the kernel and carry no meaning here.
                _ => {}
            }
        }
    }

    #[test]
    fn every_jump_of_the_full_filter_lands_inside_the_program() {
        let f = Filter::build(&all_denied()).unwrap();
        assert_jumps_land_inside(&f.prog);
    }

    #[test]
    fn every_jump_of_every_axis_subset_lands_inside_the_program() {
        // Each axis shifts the terminals, so a resolution bug can hide in one
        // combination and not in another.
        for a in [false, true] {
            for b in [false, true] {
                for c in [false, true] {
                    let p = SyscallPolicy {
                        deny_ptrace: a,
                        deny_raw_sockets: b,
                        deny_foreign_signals: c,
                    };
                    let f = Filter::build(&p).unwrap();
                    assert_jumps_land_inside(&f.prog);
                }
            }
        }
    }

    #[test]
    fn no_placeholder_survives_resolution() {
        let f = Filter::build(&all_denied()).unwrap();
        for (i, insn) in f.prog.iter().enumerate() {
            assert!(insn.jt < L_ALLOW, "instruction {i} kept a jt placeholder");
            assert!(insn.jf < L_ALLOW, "instruction {i} kept a jf placeholder");
        }
    }

    #[test]
    fn the_program_opens_with_the_architecture_gate() {
        let f = Filter::build(&all_denied()).unwrap();
        assert_eq!(f.prog[0], ld(OFF_ARCH), "the arch word is loaded first");
        assert_eq!(f.prog[1].code, BPF_JEQ_K);
        assert_eq!(f.prog[1].k, AUDIT_ARCH);
        // A mismatch must reach the KILL terminal, which is the last one.
        let target = 1 + 1 + f.prog[1].jf as usize;
        assert_eq!(
            f.prog[target],
            ret(SECCOMP_RET_KILL_PROCESS),
            "a foreign architecture must end the process, not merely be refused"
        );
    }

    #[test]
    fn the_last_three_instructions_are_the_three_terminals() {
        let f = Filter::build(&all_denied()).unwrap();
        let n = f.prog.len();
        assert_eq!(f.prog[n - 3], ret(SECCOMP_RET_ALLOW));
        assert_eq!(
            f.prog[n - 2],
            ret(SECCOMP_RET_ERRNO | libc::EPERM as u32),
            "a denial is a permission error the program can report, not a kill"
        );
        assert_eq!(f.prog[n - 1], ret(SECCOMP_RET_KILL_PROCESS));
    }

    #[test]
    fn an_axis_that_is_allowed_emits_no_instruction_for_it() {
        let none = SyscallPolicy {
            deny_ptrace: false,
            deny_raw_sockets: false,
            deny_foreign_signals: false,
        };
        let bare = Filter::build(&none).unwrap();
        // Prologue plus the three terminals, and nothing else.
        assert_eq!(bare.prog.len(), 7, "{:?}", bare.prog);
        assert!(
            bare.pid_slots.is_empty(),
            "no signal axis means no pid to patch"
        );
        assert!(Filter::build(&all_denied()).unwrap().prog.len() > 7);
    }

    #[test]
    fn the_signal_axis_records_exactly_one_pid_slot() {
        let f = Filter::build(&SyscallPolicy {
            deny_ptrace: false,
            deny_raw_sockets: false,
            deny_foreign_signals: true,
        })
        .unwrap();
        assert_eq!(f.pid_slots.len(), 1);
        let slot = f.pid_slots[0];
        assert_eq!(f.prog[slot].code, BPF_JEQ_K);
        assert_eq!(
            f.prog[slot].k, 0,
            "the pid is a hole before the fork, never a guess"
        );
    }

    #[test]
    fn the_raw_socket_block_masks_the_type_before_comparing_it() {
        // Without the mask a `SOCK_RAW | SOCK_CLOEXEC` walks past the check.
        let f = Filter::build(&SyscallPolicy {
            deny_ptrace: false,
            deny_raw_sockets: true,
            deny_foreign_signals: false,
        })
        .unwrap();
        let and_at = f
            .prog
            .iter()
            .position(|i| i.code == BPF_AND_K)
            .expect("the type is masked");
        assert_eq!(f.prog[and_at].k, 0xf);
        assert_eq!(f.prog[and_at - 1], ld(off_arg_lo(1)), "and it masks arg 1");
        assert_eq!(f.prog[and_at + 1].k, libc::SOCK_RAW as u32);
    }

    #[test]
    fn a_pid_with_a_high_word_is_refused_before_the_low_word_is_looked_at() {
        // `kill(-1, ...)` -- signal everything this user owns -- arrives as a
        // 64-bit argument whose high word is set.
        let f = Filter::build(&SyscallPolicy {
            deny_ptrace: false,
            deny_raw_sockets: false,
            deny_foreign_signals: true,
        })
        .unwrap();
        let hi_at = f
            .prog
            .iter()
            .position(|i| *i == ld(off_arg_hi(0)))
            .expect("the high word is loaded");
        let guard = f.prog[hi_at + 1];
        assert_eq!(guard.code, BPF_JEQ_K);
        assert_eq!(guard.k, 0);
        let denied = hi_at + 2 + guard.jf as usize;
        assert_eq!(
            f.prog[denied],
            ret(SECCOMP_RET_ERRNO | libc::EPERM as u32),
            "a non-zero high word must go straight to the denial"
        );
    }

    #[test]
    fn the_probe_does_not_lie_about_this_kernel() {
        // Whatever it answers, it must not claim support on an architecture
        // this build has no AUDIT_ARCH for.
        if seccomp_supported() {
            assert!(arch_known());
        }
    }
}
