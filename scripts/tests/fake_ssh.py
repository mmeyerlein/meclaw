"""The ONE environment rule of the kit tests' fake `ssh` (GH #1209).

A real ssh carries none of the caller's environment: a name reaches the host
only as a word of the command line. A fake `ssh` that runs the remote command
HERE inherits everything, so a transport pin over it is green without a
transport. Every such fake under `scripts/tests/` therefore strips the same
names, and they come from the transport's own sources, never from a copy:

* `lane_sync.sh VERDICT_ENV` (through `pipeline/lane.py verdict_env()`),
* `lane_sync.sh LANE_WORDS`, every other word `lane_env` and `cmd_test` put
  on the lane's command line (through `pipeline/lane.py lane_words()`),
* `lane_sync.sh GOLDEN_SWITCHES` and `pipeline/lane.py GOLDEN_SWITCHES`,
* and the tests' fake key.

Re-check tv-bn M7c M3: the brake stood here as a hand copy, and the lane's
name, width and flags `lane_env` carries by word stood nowhere -- over a fake
they came from the caller, and a transport pin on them was green without a
transport. `lane_env` now refuses a word `LANE_WORDS` lacks.

Measured before (re-check tv-bn M7, M3): `test_pipeline_lane`'s fake held a
hand copy of the four VERDICT_ENV names; a fifth name in `lane_sync.sh` would
have reached the "lane" from the caller's environment, and its transport pin
would have turned green without a transport. The fakes of `test_strand_sh`,
`test_pipeline_pre` and `test_pipeline_gate_cli` stripped none of them.

A fake writes `MARKER` on a line of its own where the names go and is built
with `script(template)`; `test_fake_ssh` pins every user to it.

THE ONE MAKER (re-check tv-bn M7e, orchestrator decision M7f): the static
sweeps over the forms a test may use to write an `ssh` did not converge
(three rounds, new forms each time). The guarantee moved to run time: this
module is the only allowed maker of a fake `ssh`. `install` (a fake that runs
its arguments here, the rule applied) and `install_inert` (a fake whose body
this module generates and which runs nothing of its arguments) write
`<directory>/ssh` and append its entry -- `<resolved absolute path>@<sha256
of the body written>` -- to the environment variable `REGISTRY`
(`MECLAW_FAKE_SSH_REGISTRY`, entries joined by `:`), in `os.environ` and in
an `env` the caller hands in. The narrow point of the transport checks
ALWAYS (re-check tv-bn M7h A-M1, orchestrator OR-T10-2): it admits the
system's own `ssh` or a fake whose path AND body hash stand in the registry
-- a body overwritten after `_put`, a link to another program, a fake
without a registry are refused. `admits` is the reference of that contract
for the pins. The sweep in `test_fake_ssh` is the early warning beside it,
not the guarantee.
"""

import hashlib
import os
import pathlib
import re
import shlex

REGISTRY = "MECLAW_FAKE_SSH_REGISTRY"

MARKER = "# @fake-ssh: strip the transported names@"

# The key a test sets to prove that nothing of the environment leaves.
TEST_SECRET = ("FAKE_SECRET_API_KEY",)

_GOLDEN_LINE = re.compile(r"^GOLDEN_SWITCHES=\(([A-Z0-9_ ]*)\)[ \t]*$", re.M)


def _shell_golden(path):
    found = _GOLDEN_LINE.findall(pathlib.Path(path).read_text(encoding="utf-8"))
    if len(found) != 1 or not found[0].split():
        raise AssertionError("%s: no single GOLDEN_SWITCHES=(...) line of names" % path)
    return tuple(found[0].split())


def carried_names(lane_sync=None):
    """The names no fake `ssh` may hand on, in a stable order, each once.
    `lane_sync` reads another copy of the transport (the pin of a new name)."""
    from scripts.pipeline import lane   # late: side_effect_guard imports this module bare
    path = lane_sync or lane.LANE_SYNC
    names = (lane.verdict_env(path) + lane.lane_words(path) + _shell_golden(path)
             + tuple(lane.GOLDEN_SWITCHES) + TEST_SECRET)
    return tuple(dict.fromkeys(names))


def strip_line(names=None):
    """The fake's shell line that drops `names` (default: `carried_names()`)."""
    return "unset " + " ".join(carried_names() if names is None else names)


def script(template, names=None):
    """`template` with its one `MARKER` line replaced by `strip_line(names)`."""
    if template.count(MARKER) != 1:
        raise AssertionError("a fake ssh names the marker exactly once, found %d"
                             % template.count(MARKER))
    return template.replace(MARKER, strip_line(names))


def entry(path):
    """The registry entry of the fake at `path`: its resolved absolute path
    and the sha256 of its body, `<path>@<hex>`."""
    real = os.path.realpath(path)
    with open(real, "rb") as fh:
        return "%s@%s" % (real, hashlib.sha256(fh.read()).hexdigest())


def registry(env=None):
    """The registry entries of `env` (default `os.environ`), in order."""
    value = (os.environ if env is None else env).get(REGISTRY, "")
    return tuple(p for p in value.split(":") if p)


def paths(env=None):
    """The paths of the registry entries of `env`, in order."""
    return tuple(e.rpartition("@")[0] for e in registry(env))


def admits(ssh, env=None):
    """The contract of the narrow point for a fake: `ssh` (a path) is a fake
    this module made and nobody rewrote -- its resolved path and the hash of
    its body stand in the registry of `env`."""
    try:
        return entry(ssh) in registry(env)
    except OSError:
        return False


def _register(path, env):
    item = entry(path)
    for target in (os.environ,) + (() if env is None else (env,)):
        # vanished fakes (a test's directory is gone) leave the list: it
        # stays one argument string however many tests a process runs
        keep = [e for e in registry(target)
                if e.rpartition("@")[0] != path and os.path.exists(e.rpartition("@")[0])]
        target[REGISTRY] = ":".join(keep + [item])


def register_tool(path, env=None):
    """Register a test's fake `rsync` or `git` at `path` (written by the
    test) with the hash of its body, in `os.environ` and in `env`: the narrow
    point checks the local program of a remote rsync or git push like ssh
    (orchestrator OR-T10-25), so a fake transport tool needs the registry
    as well. Returns its resolved path."""
    real = os.path.realpath(os.fspath(path))
    if os.path.basename(real) not in ("rsync", "git"):
        raise AssertionError("register_tool registers a fake rsync or git, not %s" % real)
    if ":" in real:
        raise AssertionError("a fake tool path holds the registry's separator: %s" % real)
    _register(real, env)
    return real


def _put(directory, text, env):
    path = os.path.realpath(os.path.join(os.fspath(directory), "ssh"))
    if ":" in path:
        raise AssertionError("a fake ssh path holds the registry's separator: %s" % path)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.chmod(path, 0o755)
    _register(path, env)
    return pathlib.Path(path)


def install(directory, template, names=None, *, env=None):
    """Write `<directory>/ssh` = `script(template, names)`, executable, and
    register it (in `os.environ` and in `env`); returns its path."""
    return _put(directory, script(template, names), env)


def _dq(text):
    return "".join("\\" + c if c in '\\"$`' else c for c in text)


def inert_body(reason, *, stderr=None, log=None, reply=None, rc_file=None, rc=255):
    """The body of an `ssh` that runs nothing of its arguments: generated
    here, never handed in, so no marker can lie about it. It may append
    `ssh <args>` to the file `log`, print the file `reply`, print `stderr`
    (`{argv}` stands for the arguments) to stderr, and exit with the number
    in the file `rc_file` or with `rc`."""
    if "\n" in reason:
        raise AssertionError("an inert fake's reason is one line")
    lines = ["#!/bin/sh", "# fake ssh (scripts/tests/fake_ssh.py, inert): %s" % reason]
    if log is not None:
        lines.append('printf "%%s\\n" "ssh $*" >> %s' % shlex.quote(os.fspath(log)))
    if reply is not None:
        lines.append("cat %s 2>/dev/null" % shlex.quote(os.fspath(reply)))
    if stderr is not None:
        lines.append('printf "%%s\\n" "%s" >&2' % _dq(stderr).replace("{argv}", "$*"))
    if rc_file is not None:
        q = shlex.quote(os.fspath(rc_file))
        lines.append('[ -f %s ] && exit "$(cat %s)"' % (q, q))
    lines.append("exit %d" % int(rc))
    return "\n".join(lines) + "\n"


def install_inert(directory, reason, *, env=None, **body):
    """Write and register `<directory>/ssh` = `inert_body(reason, **body)`."""
    return _put(directory, inert_body(reason, **body), env)


def remote_script(command):
    """The script inside a lane's command line (`lane.remote_command`,
    `lane_sync.sh remote_env_i`: `exec env -i <host names> bash -c <script>`);
    any other text as it is. For the pins that read what a run carries."""
    if command.startswith("exec env -i "):
        return shlex.split(command)[-1]
    return command


_WRAPPED = re.compile(r"""exec env -i (?:\$\{[A-Z_]+\+"[A-Z_]+=\$[A-Z_]+"\} )*bash -c ((?:'[^']*'|"'")+|[^\s'"]+)""")


def unwrap_remote(text):
    """`text` (a log of a fake's arguments, any number of commands) with each
    lane command line replaced by the script inside it (`remote_script`)."""
    return _WRAPPED.sub(lambda m: shlex.split(m.group(1))[0], text)
