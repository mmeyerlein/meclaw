"""No script test ever reaches the machine's systemd, a remote host or a power switch.

WHY (wave dev-tempo W7, 09.10. 02:28): a red pin test of `pool stop --unit`
ran its CLI against the real `systemctl`, and `systemctl --user stop
mm-os-e28` took the live colony down for 13 s. Patching one test is not
enough: the next red test can make the same call through another path.

`install()` puts a directory of fakes at the FRONT of `PATH` -- `systemctl`,
`systemd-run`, `ssh`, `scp`, `poweroff`, `reboot`, `shutdown`, `halt`,
`pkill`, `killall`, `sudo`, `doas`, `pkexec`, `busctl`, `loginctl`,
`ipmitool`, `kill`, `dbus-send`, `gdbus`, `telinit`, `systemd-inhibit`,
`machinectl`, `killall5`, `init`, `fuser`, `skill`. Each fake prints what
was blocked and exits with `BLOCKED_RC`, so a test that reaches one fails
loudly instead of acting. The environment change is inherited by every
subprocess (`strand.sh`, `gate.sh`, the runners). A test that needs its own
fake (a fake `ssh` that plays the lane) puts its directory in front of this
one -- an explicit fake still wins, an accidental real call never does.

`store_isolation.isolate()` calls `install()` and `restore()` calls
`remove()`, so every module under `scripts/tests/` is covered by the
`setUpModule` it already has; `test_side_effect_guard.py` pins it, and it
also refuses absolute paths such as `/usr/bin/systemctl` (also under
`/usr/local/bin`) in test code, the pipeline, `scripts/*.sh` and the
runners, which no `PATH` fake can catch.

Z7: a subprocess started with `env={...}` WITHOUT `PATH` used to fall back
to `/bin:/usr/bin` and reach the real binary. While a guard is installed,
`subprocess.Popen` gives such an env the guard's directory in front of that
fallback (`os.defpath`); an env that names its own `PATH` is left alone.
The same hook sends an absolute path to a blocked tool (`/usr/bin/systemctl`
as argv[0], also when built at run time) to its fake.

Signals (review ZC I1): the `kill` fake only covers the BINARY. While a
guard is installed, `os.kill`/`os.killpg` from the test process refuse any
pid that is not the test process itself or one of its descendants (pid 0,
-1 and the runner's own group included) with a `PermissionError`; signals to
own children and their groups pass. Signal 0 is a pure existence probe
without a side effect and always reaches the kernel, so a liveness check
(`ProcessLookupError` = dead, `PermissionError` = alive, other user) stays
truthful under the guard. The shell BUILTIN `kill` cannot be
caught at run time, so `test_side_effect_guard` forbids it in test scripts
by a lint. Not covered: a Python grandchild that builds its own environment
without `PATH` (`env={}`), and `kill` inside the scripts the tests drive --
those only signal pids they started themselves.
"""

import errno
import os
import shutil
import subprocess
import tempfile

# `ssh` is not among the stubs written here: the one maker of a fake ssh
# (`fake_ssh.install_inert`) writes and registers it (re-check tv-bn M7e,
# the narrow point admits only a registered ssh).
_STUBS = ("systemctl", "systemd-run", "scp", "poweroff", "reboot",
           "shutdown", "halt", "pkill", "killall",
           # re-check W7 N2: escalation and the raw bus reach systemd as well
           "sudo", "doas", "pkexec", "busctl",
           # Z7: logind, the BMC, the `kill` BINARY, the raw bus by CLI
           "loginctl", "ipmitool", "kill", "dbus-send", "gdbus",
           # review ZC: runlevel 0, shutdown inhibitors, machined
           "telinit", "systemd-inhibit", "machinectl",
           # re-check ZC M4: killall5 signals every own process outside the
           # session (e28 too), `init 0`, `fuser -k`, `skill`
           "killall5", "init", "fuser", "skill")
BLOCKED = _STUBS + ("ssh",)
BLOCKED_RC = 97
MARK = "side_effect_guard"

FAKE = """#!/bin/sh
echo "%s: real '%s' is blocked in tests (argv: $*)" >&2
exit %d
"""

_STACK = []
_POPEN_INIT = subprocess.Popen.__init__
_OS_KILL = os.kill
_OS_KILLPG = os.killpg


def _is_own(pid):
    """True for this process or one of its descendants (via /proc ppid)."""
    me = os.getpid()
    for _ in range(64):
        if pid == me:
            return True
        if pid <= 1:
            return False
        try:
            with open(f"/proc/{pid}/stat", encoding="utf-8") as fh:
                pid = int(fh.read().rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            return False
    return False


def _refuse(what):
    raise PermissionError(errno.EPERM, f"{MARK}: {what} is blocked in tests")


def _guarded_kill(pid, sig):
    if sig == 0:  # pure existence probe, no side effect: the kernel answers
        return _OS_KILL(pid, sig)
    if pid < -1:
        _check_group(-pid, f"os.kill({pid}, {sig})")
    elif pid <= 0 or not _is_own(pid):
        _refuse(f"os.kill({pid}, {sig})")
    return _OS_KILL(pid, sig)


def _check_group(pgid, what):
    if pgid in (os.getpgrp(), os.getpid()) or not _is_own(pgid):
        _refuse(what)


def _guarded_killpg(pgid, sig):
    if sig == 0:  # pure existence probe, no side effect: the kernel answers
        return _OS_KILLPG(pgid, sig)
    _check_group(pgid, f"os.killpg({pgid}, {sig})")
    return _OS_KILLPG(pgid, sig)


def _registered_fake(head, env):
    """True when `head` is a fake ssh of the one maker, registered with the
    hash of its body (in `env` or this process's registry): the narrow point
    runs the checked absolute path (re-check tv-bn M7j A-M1), and the guard
    redirects every other absolute `ssh` -- the system's -- to its stub."""
    try:
        from . import fake_ssh
    except ImportError:   # imported bare by a script's own guard
        import fake_ssh
    return os.path.basename(head) == "ssh" and any(
        fake_ssh.admits(head, e) for e in ((os.environ,) if env is None else (env, os.environ)))


def _guarded_popen_init(self, args, *rest, **kw):
    """`Popen.__init__` that keeps the fakes in front for an env without PATH."""
    env = kw.get("env")
    if env is not None and _STACK and "PATH" not in env and b"PATH" not in env:
        kw["env"] = dict(env, PATH=_STACK[-1][0] + os.pathsep + os.defpath)
    if _STACK and isinstance(args, (list, tuple)) and args and not kw.get("shell"):
        head = os.fsdecode(args[0])
        if os.path.isabs(head) and os.path.basename(head) in BLOCKED \
                and os.path.dirname(head) != _STACK[-1][0] and not _registered_fake(head, kw.get("env")):
            args = [os.path.join(_STACK[-1][0], os.path.basename(head))] + list(args[1:])
    _POPEN_INIT(self, args, *rest, **kw)


def current_dir():
    """The fakes' directory of the newest `install()`, or None."""
    return _STACK[-1][0] if _STACK else None


def _fake_ssh():
    """`fake_ssh` from this file's own directory, however this module was
    loaded: as `scripts.tests.side_effect_guard`, bare with `scripts/tests` on
    sys.path, or by file location with neither (the scenario suites under
    `workshop/evals/**` load it that way, from outside the package)."""
    try:
        from . import fake_ssh
        return fake_ssh
    except ImportError:
        pass
    try:
        import fake_ssh   # imported bare, `scripts/tests` on sys.path
        return fake_ssh
    except ModuleNotFoundError:
        pass
    import importlib.util
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fake_ssh.py")
    spec = importlib.util.spec_from_file_location("fake_ssh", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def install():
    """Put the fakes in front of `PATH`; returns their directory."""
    tmp = tempfile.mkdtemp(prefix="side-effect-guard-")
    for name in _STUBS:
        path = os.path.join(tmp, name)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(FAKE % (MARK, name, BLOCKED_RC))
        os.chmod(path, 0o755)
    fake_ssh = _fake_ssh()
    fake_ssh.install_inert(tmp, "the side-effect guard's stub: refuses with BLOCKED_RC",
                           stderr="%s: real 'ssh' is blocked in tests (argv: {argv})" % MARK, rc=BLOCKED_RC)
    old = os.environ.get("PATH")
    _STACK.append((tmp, old))
    os.environ["PATH"] = tmp + (os.pathsep + old if old else "")
    subprocess.Popen.__init__ = _guarded_popen_init
    os.kill, os.killpg = _guarded_kill, _guarded_killpg
    return tmp


def remove():
    """Undo the newest `install()`."""
    if not _STACK:
        return
    tmp, old = _STACK.pop()
    if old is None:
        os.environ.pop("PATH", None)
    else:
        os.environ["PATH"] = old
    shutil.rmtree(tmp, ignore_errors=True)
    if not _STACK:
        subprocess.Popen.__init__ = _POPEN_INIT
        os.kill, os.killpg = _OS_KILL, _OS_KILLPG
