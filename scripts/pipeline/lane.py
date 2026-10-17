"""meclaw -- the lane transport of the pipeline (W3; successor of `scripts/lane_sync.sh`).

A lane is a build machine named in the PRIVATE host file
`${MECLAW_LANES_FILE:-~/.config/meclaw/lanes}`, one line per lane:

    <name> <ssh-target>|local [<bmc>|<root>] [threads=<n>] [cores=<n>] [class=<c>] [jobs=<n>] [slots=<n>] [<key>=<value> ...]

`#` starts a comment. The third word is the BMC of a host reached by ssh, or
the lane root of a `local` lane. A number word that is no positive number is
ignored (a typo costs the cap, not the lane); `cores=<n>` declares the lane's
real cores, the colony density of a satellite pass (none: nproc / 2, said
in the run log) -- `lscpu` inside a VM cannot tell them, a 1 socket x 12
cores x 1 thread guest on a 6-core host counts 12; `class=egon` marks the owner's
machine beside a live colony, `class=lane` a build host that runs its booked
width; there is no default class (#1158): a missing class stays empty and an
unknown one keeps its word, and both keep the Egon cap; further
`key=value` words (`sccache=1`, `linker=mold`, `watch=<unit>`) land in
`Lane.opts`. The host file is the one source of names -- nothing here names a
machine.

THE LAYOUT under the lane root (`MECLAW_LANE_ROOT` or the root of a `local`
line, default `/srv/target`): `meclaw.git` (the bare repository the trees are
pushed into), `tree/` (THE build path: every run with a cargo station checks
its commit out here, one path per lane, so cargo's embedded source paths stay
constant), `target/` (its cargo target directory), `wt/<tree>` (runs without a
cargo station), `ws/<run>/` (the executor's workspace of one run),
`cache/workshop-tools/node_modules` (the browser drivers, shared by every tree).

WHAT TRAVELS is exactly the tree a local gate would see -- the semantics of
`lane_sync.sh push`, kept byte for byte where both sides meet:

  * the commit by `git push` (plus the refs `github-main` and `master` when
    this clone has them, and the `origin` URL when it is a plain https URL
    without credentials);
  * the uncommitted rest as an overlay measured against HEAD -- every staged
    or unstaged change of a tracked file, every untracked file that is not
    ignored, every deletion;
  * never a file whose NAME says secret (`secret_name`). A tracked secret
    refuses the push with its path before any contact (`LaneError` code 5);
  * after the overlay both sides hash the same list (HEAD, then per path its
    sha256 or `absent`) and a difference refuses the push (code 5);
  * a file whose content the lane did not hold before the sync gets the
    lane's time (GH #1071), an unchanged one keeps the carried time;
  * a key-free `.env`, written ON the lane by `gate_plan.py --print lane-env`;
  * the browser drivers' `node_modules` as a link to the lane's cache.

THE TRANSPORT is ssh for a host and `local` for a lane on this machine: the
same shell steps run in a local `bash`, the bare repository borrows this
clone's objects (`objects/info/alternates`) and the overlay is a local rsync.
`ssh`, `rsync` and `git` are found on PATH (tests put fakes there).

Errors are `LaneError(code, message)`: 2 = the lane did not answer or a step
failed there, 5 = refused here (a tracked secret, a different tree).
"""

import dataclasses
import hashlib
import json
import os
import re
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time

from . import register as _register

DEFAULT_ROOT = "/srv/target"

# W1's transport (`scripts/lane_sync.sh`) beside this package: where it already
# computes the lane side -- the status line, the environment words, the guards
# of a run, a test on the lane -- this module calls it instead of computing it
# a second time (one truth).
LANE_SYNC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lane_sync.sh")

# The switches whose goldens come back from a lane (GH #1051). An ALLOWLIST by
# exact name, never a pattern: a pattern over the environment is how a key
# would leave this machine.
GOLDEN_SWITCHES = ("GH277_WRITE_GOLDEN", "MECLAW_GH935_BLESS")

q = shlex.quote


class LaneError(Exception):
    """A transport failure. `code` 2 = the lane did not answer or a step on it
    failed; 5 = refused here (a tracked secret or a different tree)."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


@dataclasses.dataclass
class Lane:
    """One line of the host file."""

    name: str
    ssh: str
    bmc: str = ""
    root: str = DEFAULT_ROOT
    transport: str = "ssh"
    cls: str = ""
    threads: int = 0
    cores: int = 0
    jobs: int = 0
    slots: int = 1
    opts: dict = dataclasses.field(default_factory=dict)


# --------------------------------------------------------------------------
# The host file
# --------------------------------------------------------------------------

def lanes_file():
    """The path of the host file."""
    return os.environ.get("MECLAW_LANES_FILE") or os.path.expanduser("~/.config/meclaw/lanes")


def _posint(word):
    return int(word) if word and re.fullmatch(r"[1-9][0-9]*", word) else 0


def load_lanes(path=None):
    """The lanes of the host file, in file order; no file = no lanes."""
    path = path or lanes_file()
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        return []
    env_root = os.environ.get("MECLAW_LANE_ROOT") or DEFAULT_ROOT
    lanes = []
    for line in text.splitlines():
        words = line.split("#", 1)[0].split()
        if len(words) < 2:
            continue
        name, target = words[0], words[1]
        transport = "local" if target == "local" else "ssh"
        positional = [w for w in words[2:] if "=" not in w]
        opts = dict(w.split("=", 1) for w in words[2:] if "=" in w)
        third = positional[0] if positional else ""
        lanes.append(Lane(
            name=name, ssh=target,
            bmc=third if transport == "ssh" else "",
            root=(third if transport == "local" and third else env_root).rstrip("/") or "/",
            transport=transport,
            cls=opts.pop("class", ""),
            threads=_posint(opts.pop("threads", "")),
            cores=_posint(opts.pop("cores", "")),
            jobs=_posint(opts.pop("jobs", "")),
            slots=_posint(opts.pop("slots", "")) or 1,
            opts=opts))
    return lanes


def find_lane(name, path=None):
    """The lane `name` of the host file, or `LaneError` (code 2)."""
    for lane in load_lanes(path):
        if lane.name == name:
            return lane
    raise LaneError(2, "no lane %s in the host file %s" % (name, path or lanes_file()))


def env_for(lane, mode, nproc=None):
    """The environment of a run on `lane` in `mode`, for the dispatch's `+env`:
    the words of `lane_sync.sh env <mode>` (W1: lane name, incremental, the
    width of the class, sccache, linker) in their order, behind the runner's
    floor of 0 (`MECLAW_GATE_LANE_MIN_FREE_G=0`: no wipe of the shared
    target) -- not for `test`, and not under `MECLAW_LANE_WIPE_OK`.
    `nproc` stands for this machine's cores in a class `egon` width (GNU
    `nproc` reads `OMP_NUM_THREADS`) and for the lane's vCPUs where a build
    host names `$(nproc)` (full test width)."""
    extra = {"OMP_NUM_THREADS": nproc} if nproc else {}
    res = _sync(lane, "env", mode, env=sync_env(lane, **extra))
    if res.returncode != 0:
        raise LaneError(2, "lane_sync.sh env %s failed (%s)" % (mode, _tail(res.stderr)))
    env = {}
    if mode != "test" and not os.environ.get("MECLAW_LANE_WIPE_OK"):
        env["MECLAW_GATE_LANE_MIN_FREE_G"] = "0"
    for word in shlex.split(res.stdout.decode()):
        key, _, value = word.partition("=")
        if value == "$(nproc)":
            # a build host at full width: its vCPUs (the fingerprint), never
            # the literal; unknown, the key stays away (test-tier.sh's floor)
            if not nproc:
                continue
            value = str(nproc)
        env[key] = value
    return env


# The verdict's names (GH #1210): ONE list, `VERDICT_ENV=(...)` of
# `lane_sync.sh`, read from there -- no second copy that can lack a name
# (flake_triage's lacked MECLAW_REQUIRE_BROWSERS, and a driver SKIP graded
# green on the lane).
_VERDICT_LINE = re.compile(r"^VERDICT_ENV=\(([A-Z0-9_ ]*)\)[ \t]*$", re.M)
_NAME = re.compile(r"^[A-Z_][A-Z0-9_]*$")


def _shell_names(var, path=None):
    """The names of the one `<var>=(...)` line of `lane_sync.sh`, in its
    order; LaneError when the line is missing, doubled or names nothing."""
    path = path or LANE_SYNC
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError as exc:
        raise LaneError(2, "cannot read the %s names of %s (%s)" % (var, path, exc))
    line = _VERDICT_LINE if var == "VERDICT_ENV" else \
        re.compile(r"^%s=\(([A-Z0-9_ ]*)\)[ \t]*$" % re.escape(var), re.M)
    found = line.findall(text)
    names = tuple(found[0].split()) if len(found) == 1 else ()
    if not names or not all(_NAME.match(n) for n in names):
        raise LaneError(2, "%s: no single %s=(...) line of names" % (path, var))
    return names


def verdict_env(path=None):
    """The names of `lane_sync.sh VERDICT_ENV`, in its order; LaneError when
    the line is missing or names nothing."""
    return _shell_names("VERDICT_ENV", path)


def lane_words(path=None):
    """The names of `lane_sync.sh LANE_WORDS` (re-check tv-bn M7c M3): the
    words `lane_env` and `cmd_test` write on the lane's command line beside
    VERDICT_ENV and GOLDEN_SWITCHES -- the names a kit test's fake ssh strips
    (`scripts/tests/fake_ssh.py`)."""
    return _shell_names("LANE_WORDS", path)


# The word check of every way to a lane (OR-TV.tv-bn.M7e-1; re-check tv-bn
# M7e B-M2, orchestrator M7g: fail-closed instead of a list of quoting
# forms). M7e looked for a name only behind a blank, `;&|(` or a literal
# `\n`; eight forms passed both checks (`'FOO=`, `"FOO=`, `FOO+=`,
# `$'FOO=..\n..'`, a backtick, `--setenv=FOO=`, and the kit's own idiom
# `shlex.quote("FOO=a b")` as a word or a script) -- exactly the forms the
# kit's quoting (`q`, `shlex.join`, `printf %q`) writes. Now the check has no
# left context at all: EVERY upper-case name directly before `=`, `+=`, `:=`
# or an array index, anywhere in the command, and every upper-case name a
# setting builtin takes (`export`, `declare`, `typeset`, `readonly`, `local`,
# `read`, `readarray`, `mapfile`, `for`, `printf -v`) is one of the four
# lists of `lane_sync.sh`, or the command never leaves. A false positive is
# a refusal (fail-closed). `lane_sync.sh remote_words_known` is the same rule;
# `test_lane_engstelle` pins both against one corpus. Lower-case names are a
# host script's own (OR-M7e-2).
_SET_ASSIGN = re.compile(r"([A-Z_][A-Z0-9_]*)(?:\[[^\]]*\])?[+:]?=")
_SET_KEYWORD = re.compile(r"(?:^|(?<=[^A-Za-z0-9_-]))(?:export|declare|typeset|readonly|local|read|readarray"
                          r"|mapfile|for|printf\s+-v)\s([^;&|\n]*)")
_SET_TOKEN = re.compile(r"(?:^|(?<=[^A-Za-z0-9_${/.:-]))([A-Z_][A-Z0-9_]*)(?![A-Za-z0-9_])")
# The value of a `NAME=value` word behind a setting builtin is no name: a pass
# under a ruling env (`MECLAW_TESTING_RULES_RULING=OR-TV-8,OR-TV-9`) was
# refused for `OR` (welle-testing M7h, pre seed 384518679). Only the value
# goes -- unquoted, '...' or "..." parts -- the names beside it stay checked.
_SET_VALUE = re.compile(r"""=(?:'[^']*'|"[^"]*"|[^\s'";&|])+""")
# ... except behind a nameref option (`declare|typeset|local -n r=FOO`): there
# the value IS a name (re-check tv-bn M7h A-M2.1, the regression of M7h).
_NAMEREF = re.compile(r"(?:^|\s)-[A-Za-z]*n")
_REMOTE_LISTS = ("LANE_WORDS", "VERDICT_ENV", "GOLDEN_SWITCHES", "HOST_OWN")
# The names a caller may hand to the narrow point as data (`env=`): the
# lists whose values come from this side. HOST_OWN is not among them -- its
# values belong to the host or are constants of the kit (re-check M7e H1),
# so they stand literally in a script and never come from a caller.
_ENV_LISTS = ("LANE_WORDS", "VERDICT_ENV", "GOLDEN_SWITCHES")


def set_names(script):
    """Every upper-case name `script` may set or export, in order (the rule of
    `remote_words_known`; doubles kept)."""
    names = _SET_ASSIGN.findall(script)
    for seg in _SET_KEYWORD.findall(script):
        names += _SET_TOKEN.findall(seg if _NAMEREF.search(seg) else _SET_VALUE.sub("=", seg))
    return names


def _known(lists, path=None):
    known = set()
    for var in lists:
        known.update(_shell_names(var, path))
    return known


def remote_words_known(script, path=None):
    """LaneError(2) when `script` sets a name none of the four lists of
    `lane_sync.sh` names -- the check of the narrow point."""
    known = _known(_REMOTE_LISTS, path)
    for name in set_names(script):
        if name not in known:
            raise LaneError(2, "the lane word %s is not in LANE_WORDS (scripts/lane_sync.sh) -- name it there, "
                               "the kit tests' fake ssh strips what the list names (command not sent)" % name)


def remote_base(path=None):
    """The names of `lane_sync.sh REMOTE_BASE`: the host's own environment a
    command for a lane keeps under `env -i`."""
    return _shell_names("REMOTE_BASE", path)


def remote_command(script):
    """The command line a host runs for `script` (orchestrator OR-T10-2,
    re-check tv-bn M7h A-M2): `exec env -i <REMOTE_BASE, each if set there>
    bash -c <script>` -- the environment of the command is built from data
    alone (the host's own names, expanded there, and the words `script`
    carries), so nothing of this side reaches the lane, however it was set,
    over a real ssh or a test's fake. The rule of `lane_sync.sh
    remote_env_i`."""
    return "exec %sbash -c %s" % (remote_env_i(), q(script))


def remote_env_i():
    """`env -i <REMOTE_BASE, each if set there> ` -- the front of every
    program a host runs for a lane: `remote_command`'s bash, and the rsync
    (`--rsync-path`) and git-receive-pack (`--receive-pack`) the transport
    starts there (re-check tv-bn M7j A-M4, orchestrator OR-T10-11: rsync and
    git built their remote command themselves, without `env -i`; measured
    P8: the fake got `h rsync --server ...`). The rule of `lane_sync.sh
    remote_env_keep`."""
    return "env -i " + "".join('${%s+"%s=$%s"} ' % (n, n, n) for n in remote_base())


def rsync_path():
    """rsync's `--rsync-path=` for a lane: rsync on the host under `env -i`."""
    return "--rsync-path=%srsync" % remote_env_i()


def receive_pack():
    """git push's `--receive-pack=` for a lane: under `env -i` on the host."""
    return "--receive-pack=%sgit-receive-pack" % remote_env_i()


def env_prefix(env=None, unset=()):
    """The shell text the narrow point puts before a script for the
    environment `env` (name -> value) and the names `unset`: built here, each
    value `shlex.quote`d, each name one of LANE_WORDS, VERDICT_ENV or
    GOLDEN_SWITCHES -- else LaneError(2), nothing is sent (orchestrator M7g:
    the narrow point builds the remote environment from data instead of
    reading it back out of a composed string)."""
    known = _known(_ENV_LISTS)
    for name in list(unset) + list(env or {}):
        if name not in known:
            raise LaneError(2, "the lane word %s is not in LANE_WORDS, VERDICT_ENV or GOLDEN_SWITCHES "
                               "(scripts/lane_sync.sh) -- name it there (command not sent)" % name)
    text = ""
    if unset:
        text += "unset %s; " % " ".join(unset)
    if env:
        text += "export %s; " % " ".join("%s=%s" % (k, q(str(v))) for k, v in env.items())
    return text


# The ssh program of every way to a lane (re-check tv-bn M7h A-M1,
# orchestrator OR-T10-2: the guarantee by origin, not by path or a switch).
# The check runs ALWAYS and its result is what runs (re-check tv-bn M7j
# A-M1): `ssh_program` returns the checked program's resolved ABSOLUTE path,
# and that path is the program word of every way (ssh argv[0], rsync `-e`,
# GIT_SSH_COMMAND) -- measured before: the check returned the name `ssh`, a
# child with another PATH ran a hand fake (P4), and an empty PATH skipped the
# check and ran `./ssh` (P10). The `ssh` of the first ABSOLUTE entry of PATH
# (a relative entry names no place, it is skipped; no PATH or no ssh is
# LaneError(2)) is admitted only if it is
# (a) the system's own -- its realpath is the realpath of `ssh` in one of
#     SYSTEM_SSH_DIRS, and that file and every directory above it up to `/`
#     belong to root and are writable by neither group nor others (re-check
#     tv-bn M7j A-M2: the momentary writability admitted the user's own file
#     with mode 0555, whose owner may `chmod u+w` it at any time), or
# (b) a fake of `scripts/tests/fake_ssh.py` -- the registry
#     MECLAW_FAKE_SSH_REGISTRY (`:`-separated `<realpath>@<sha256>`) names
#     its resolved path AND the hash of its body (a body rewritten after the
#     maker wrote it is refused; an entry whose hash is not 64 hex digits
#     never matches).
# No registry admits no fake (fail-closed). `lane_sync.sh ssh_bin` is the
# same rule; `test_lane_engstelle` pins both on one set of probes.
#
# The same check admits the local programs of the two transports that carry
# the host side's `env -i` (`rsync_path`, `receive_pack`): `rsync` and `git`
# of a remote way run by their checked absolute path, the system's or a
# registered fake (re-check tv-bn M7l A-H5, orchestrator OR-T10-25: a PATH
# shadow ran its own rsync past `--rsync-path`). A candidate is a REGULAR
# file that is executable, on both sides and in both rules (M7l A-M2: a FIFO
# named `ssh` first on PATH was skipped by Bash and taken by Python, and a
# registered FIFO blocked the hash for good); the hash opens without
# blocking and reads only a regular file. The check runs in a process
# started without the caller's Python environment (`/usr/bin/python3 -I`
# at every entry of the kit, M7l A-M1: a sitecustomize on PYTHONPATH lied
# the hash of a hand fake into the registry's).
#
# The lock at the entries is not enough on its own (re-check tv-bn M7m): `-I`
# is not inherited by a child the kit starts with a bare `python3`, and
# `sys.flags.isolated` alone can be faked by a sitecustomize on PYTHONPATH.
# So both: every Python the kit starts comes from PYTHON below (the one
# source), and the check refuses in a process that is not isolated
# (fail-closed) -- a child started past PYTHON fails loudly instead of
# running with the caller's PYTHONPATH.
#
# Not checked by this narrow point (orchestrator OR-T11-5, OR-T11-13): the
# loader variables LD_PRELOAD, LD_AUDIT, LD_LIBRARY_PATH and GCONV_PATH. They
# act before any line of a script runs, so no entry lock and no check in the
# kit can close them.
FAKE_SSH_REGISTRY = "MECLAW_FAKE_SSH_REGISTRY"
SYSTEM_SSH_DIRS = ("/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin")
_HEX64 = re.compile(r"[0-9a-f]{64}")
# The one interpreter of every Python process the kit starts, locally, in a
# systemd unit or on a lane host (re-check tv-bn M7m M2: hygiene's idle-off
# units, the night timer, the coverage build, the lane dispatch and pre's
# children ran a bare `python3`, and a PYTHONPATH sitecustomize got in).
PYTHON = ("/usr/bin/python3", "-I")


def python_argv(*args):
    """The argv that runs `args` (a script and its words, or `-m`/`-c`) in
    PYTHON."""
    return list(PYTHON) + [str(a) for a in args]


# Interpreter options that take the next word as their value (`-X dev`, `-W error`);
# `-m`/`-c` end the options (their value is the rest of the command).
_PY_VALUE_OPTS = ("X", "W")
_PY_END_OPTS = ("m", "c")
# One `NAME=value` word of an `env` prefix.
ENV_ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def own_name(runner=None):
    """The name of the lane this process runs on: `runner` when given, else
    MECLAW_LANE_NAME (the dispatch sets it, `env_for`), else the short host
    name. The one source (re-check tv-bn M7q, neighbour classes): the executor
    took the full host name, pool and hygiene the short one. hygiene runs on a
    host from stdin without the kit and mirrors this rule (pinned)."""
    return runner or os.environ.get("MECLAW_LANE_NAME") or socket.gethostname().split(".")[0]


def command_words(argv):
    """The words of `argv` past an `env NAME=value ...` prefix: the program and
    its arguments. The one reader of that prefix (re-check tv-bn M7q H1): a
    satellite cmd is `env SOUL2ME_LAB_SLOTS={slots} {python} ...`."""
    argv = [str(a) for a in (argv or ())]
    if argv and os.path.basename(argv[0]) == "env":
        rest = argv[1:]
        while rest and ENV_ASSIGN.match(rest[0]):
            rest = rest[1:]
        if rest:
            return rest
    return argv


def program(argv):
    """The program of `argv` past an `env NAME=value ...` prefix, "" for none."""
    words = command_words(argv)
    return words[0] if words else ""


def is_python_program(word):
    """True when `word` names a Python interpreter: PYTHON's program, any
    `python*` by basename, or the satellite placeholder `{python}`."""
    word = str(word)
    return word == "{python}" or os.path.basename(word).startswith("python")


def python_rest(argv):
    """The words of `argv` after its Python interpreter and the interpreter's
    own options (PYTHON's `-I`, `-u`, `-X dev`, `-Im`, ...): `[script, ...]`
    or `["-m", module, ...]` / `["-c", code, ...]`; None when `argv` (past an
    `env NAME=value` prefix) does not start with a Python.

    The one reader of the Python form of a command (re-check tv-bn M7p M1,
    M7q M2/H1/H2): every recogniser of a station's argv (`flakes.station_kind`,
    `flakes.scenario_runner_path`, `plan._inputs`, `register._inputs`) asks
    here instead of assuming `python...` as the first word and the script or
    `-m` as the second -- PYTHON is two words. Short options combine the way
    the interpreter reads them: `-Im x` is `-I -m x`, `-mx` is `-m x`."""
    argv = command_words(argv)
    if not argv or not is_python_program(argv[0]):
        return None
    i = 1
    while i < len(argv):
        word = argv[i]
        if word == "-" or not word.startswith("-") or word.startswith("--"):
            if word.startswith("--") and word != "--":
                i += 1
                continue
            return argv[i + 1:] if word == "--" else argv[i:]
        for j, ch in enumerate(word[1:], 1):
            if ch in _PY_END_OPTS:
                value = word[j + 1:]
                return ["-" + ch] + ([value] if value else []) + argv[i + 1:]
            if ch in _PY_VALUE_OPTS:
                i += 0 if word[j + 1:] else 1   # `-Xdev` carries its value, `-X dev` takes the next
                break
        i += 1
    return []


def _sha256(path):
    """The hash of a regular file's body; anything else is OSError (a FIFO
    or a device never blocks the check)."""
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as fh:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError("not a regular file: %s" % path)
        return hashlib.sha256(fh.read()).hexdigest()


def _registered_fake(real, value):
    """True when the registry `value` names `real` with the hash of its body
    (rule b above)."""
    entries = [e.rpartition("@") for e in value.split(":") if e]
    wanted = {sha for path, _, sha in entries
              if os.path.isabs(path) and os.path.realpath(path) == real and _HEX64.fullmatch(sha)}
    try:
        return bool(wanted) and _sha256(real) in wanted
    except OSError:
        return False


def _root_owned(real):
    """`real` and every directory above it up to `/`: owner root, no group
    or other write bit (rule a above)."""
    path = real
    while True:
        try:
            st = os.stat(path)
        except OSError:
            return False
        if st.st_uid != 0 or st.st_mode & 0o022:
            return False
        if path == "/":
            return True
        path = os.path.dirname(path)


def _regular(path):
    """`path` (an executable `which` found) if it is a regular file after
    resolution, else None -- the candidate rule of `lane_sync.sh tool_bin`
    (`-f && -x`)."""
    return path if path and os.path.isfile(path) else None


def checked_program(name, environ=None):
    """The resolved absolute path of the `name` PATH names, after the check
    above (the system's or a registered fake), else LaneError(2) -- nothing
    runs."""
    if not sys.flags.isolated:
        raise LaneError(2, "the narrow point runs only in an isolated Python (%s): this process was "
                           "started without -I, so the caller's PYTHONPATH could have answered for the "
                           "check (command not sent)" % " ".join(PYTHON))
    environ = os.environ if environ is None else environ
    path = environ.get("PATH")
    if not path:
        raise LaneError(2, "no PATH to find %s on -- the narrow point runs only the program it checked "
                           "(command not sent)" % name)
    # the first absolute entry that holds an executable `name`; a relative
    # entry names no place (it would follow the caller's cwd) and is skipped
    found = next((os.path.normpath(p) for p in (_regular(shutil.which(name, path=d)) for d in path.split(":")
                                                if os.path.isabs(d)) if p), None)
    if found is None:
        raise LaneError(2, "no %s on PATH (%s) -- the narrow point runs only the program it checked "
                           "(command not sent)" % (name, path))
    real = os.path.realpath(found)
    for d in SYSTEM_SSH_DIRS:   # rule a: the system's own, root's, written by nobody else
        own = _regular(shutil.which(name, path=d))
        if own and os.path.realpath(own) == real and _root_owned(real):
            return real
    if _registered_fake(real, environ.get(FAKE_SSH_REGISTRY, "")):   # rule b
        return real
    raise LaneError(2, "unregistered %s on PATH: %s -- neither the system's nor a fake with its hash in %s "
                       "(scripts/tests/fake_ssh.py registers a test's fake; command not sent)"
                    % (name, found, FAKE_SSH_REGISTRY))


def ssh_program(environ=None):
    """The program of every ssh argv: `checked_program("ssh")`."""
    return checked_program("ssh", environ)


def rsync_program(environ=None):
    """The local rsync of a remote way (OR-T10-25): `checked_program("rsync")`."""
    return checked_program("rsync", environ)


def git_program(environ=None):
    """The local git of a push to a remote lane (OR-T10-25): `checked_program("git")`."""
    return checked_program("git", environ)


def ssh_argv(target, command, long=False, connect_timeout=None, env=None, unset=(), bare=False):
    """THE narrow point of every command for a lane (re-check tv-bn M7e B-M1:
    flake_triage, hygiene, register, coverage and measure built their own
    ssh argv and passed the word check by). `command` is the remote command
    -- a string, or a list of words ssh joins with blanks; `target` "local"
    runs the string under `bash -c` here. The words are checked
    (`remote_words_known`), the environment comes as data (`env`, `unset`,
    `env_prefix`), the program is checked (`ssh_program`); LaneError(2)
    before any process exists. `test_lane_engstelle` pins that no other
    production code builds an ssh argv."""
    words = list(command) if isinstance(command, (list, tuple)) else None
    if words is not None and (env or unset):
        raise TypeError("ssh_argv: env/unset need a command string")
    remote_words_known(" ".join(words) if words is not None else command)
    script = None if words is not None else env_prefix(env, unset) + command
    if target == "local":
        return ["bash", "-c", script if script is not None else " ".join(words)]
    opts = [] if bare else ["-o", "BatchMode=yes", "-o", "ConnectTimeout=%s"
                            % (connect_timeout or os.environ.get("MECLAW_LANE_CONNECT_TIMEOUT", "10"))]
    if long:
        opts += ["-o", "ServerAliveInterval=60"]
    if words is not None:   # what ssh would join, its own `--` left out
        script = " ".join(words[1:] if words[:1] == ["--"] else words)
    return [ssh_program()] + opts + [target, remote_command(script)]


def verdict_env_words(environ=None):
    """`NAME=value` (shell-quoted) of the verdict names set and non-empty in
    `environ` -- the rule of `lane_sync.sh lane_env`."""
    environ = os.environ if environ is None else environ
    return ["%s=%s" % (n, q(environ[n])) for n in verdict_env() if environ.get(n)]


REQUIRE_BROWSERS = "MECLAW_REQUIRE_BROWSERS"


def test_run_env(environ=None):
    """`(env, note)` for a TEST run outside the gate: a copy of `environ` in
    which the browsers are required unless the caller opted out -- the rule of
    `lane_sync.sh require_browsers_default` (GH #1209, re-check tv-bn M7 M4:
    nobody set the switch outside the gate, and a browser driver on a lane
    without its engine SKIPped green in a TV-F7 or measure run). Unset is
    `1`; any other value is kept, and `note` names the opt-out ("" if none)."""
    env = dict(os.environ if environ is None else environ)
    if REQUIRE_BROWSERS not in env:
        env[REQUIRE_BROWSERS] = "1"
        return env, ""
    if env[REQUIRE_BROWSERS] == "1":
        return env, ""
    return env, ("NOTE: %s='%s' (opt-out) -- a browser driver without an engine SKIPs green,"
                 " no browser proof" % (REQUIRE_BROWSERS, env[REQUIRE_BROWSERS]))


# Re-check tv-bn M7c M1: the opt-out stood only on stderr or in a raw log,
# and a browser proof read green in the RESULT -- the TEST line, the triage
# report, the measure row, pre's line and summary, the coverage map. Each
# names it with this mark; `opt_out_in` reads lane_sync.sh's NOTE (the rule
# that ran) from a run's text.
_OPT_OUT_NOTE = re.compile(r"^NOTE: %s='(.*)' \(opt-out\)|\(browsers opt-out: %s='(.*)'\)"
                           % (REQUIRE_BROWSERS, REQUIRE_BROWSERS), re.M)


def opt_out_mark(value):
    """The mark a result carries for the opt-out `value` of the browser switch."""
    return "(browsers opt-out: %s='%s')" % (REQUIRE_BROWSERS, value)


def browsers_opt_out(environ=None):
    """The mark of a TEST run outside the gate under `environ`, "" without an opt-out."""
    env, note = test_run_env(environ)
    return opt_out_mark(env[REQUIRE_BROWSERS]) if note else ""


def opt_out_in(text):
    """The mark of the opt-out NOTE `lane_sync.sh` wrote into `text` (or of a
    mark a result already carries), else ""."""
    m = _OPT_OUT_NOTE.search(text or "")
    return opt_out_mark(m.group(1) if m.group(1) is not None else m.group(2)) if m else ""


def golden_env():
    """The golden switches set here, by exact name -- for the dispatch's `+env`."""
    return {name: os.environ[name] for name in GOLDEN_SWITCHES if name in os.environ}


# --------------------------------------------------------------------------
# What travels (this side)
# --------------------------------------------------------------------------

def secret_name(path):
    """True for a file that holds a secret, by its NAME and nothing else --
    the one rule of `register.secret_name` (W10: it adds `provider.env`).
    A path COMPONENT `vault` or `.env*` is no secret (review I2)."""
    return _register.secret_name(path)


def _git(top, *args):
    res = subprocess.run(["git", "-C", top] + list(args), capture_output=True)
    if res.returncode != 0:
        raise LaneError(2, "git %s failed: %s" % (args[0], _tail(res.stderr)))
    return res.stdout


def _tail(data):
    text = data.decode(errors="replace").strip() if isinstance(data, bytes) else (data or "").strip()
    return text.splitlines()[-1] if text else "no message"


def _paths(data):
    return [os.fsdecode(p) for p in data.split(b"\0") if p]


def _nul(paths):
    return b"".join(os.fsencode(p) + b"\0" for p in paths)


def _top(repo_root):
    res = subprocess.run(["git", "-C", repo_root, "rev-parse", "--show-toplevel"],
                         capture_output=True)
    if res.returncode != 0:
        raise LaneError(2, "not a git repository: %s" % repo_root)
    return os.fsdecode(res.stdout.strip())


def _changed_tracked(top):
    # `--no-renames`: a rename is a deletion plus a new file, both must travel.
    return _paths(_git(top, "diff", "--no-renames", "--name-only", "-z", "--diff-filter=d", "HEAD", "--"))


def _deleted_tracked(top):
    return _paths(_git(top, "diff", "--no-renames", "--name-only", "-z", "--diff-filter=D", "HEAD", "--"))


def _untracked(top):
    return _paths(_git(top, "ls-files", "-z", "-o", "--exclude-standard"))


def collisions(repo_root):
    """Tracked secrets, sorted: in the commit (they would leave by the push)
    or changed against it (they would leave by the overlay)."""
    top = _top(repo_root)
    tracked = _paths(_git(top, "ls-tree", "-r", "-z", "--name-only", "HEAD")) + _changed_tracked(top)
    return sorted({p for p in tracked if secret_name(p)}, key=os.fsencode)


def overlay_files(repo_root):
    """What the overlay sends: changed tracked files, then untracked ones that
    are not ignored -- never a secret by name."""
    top = _top(repo_root)
    return [p for p in _changed_tracked(top) + _untracked(top) if not secret_name(p)]


def _digest_list(top):
    return sorted(set(overlay_files(top)) | set(_deleted_tracked(top)), key=os.fsencode)


# The overlay hash, run HERE and THERE over the same list on stdin, in the tree.
DIGEST = '''{ git rev-parse HEAD; while IFS= read -r -d "" f; do
    if [ -L "$f" ]; then printf "link %s %s\\n" "$f" "$(readlink -- "$f")"
    elif [ -e "$f" ]; then sha256sum -- "$f"
    else printf "absent  %s\\n" "$f"; fi
done; } | sha256sum | cut -c1-16'''

# The content of the overlay files as a tree holds them: one NUL-terminated
# record `<sha256> <path>` per regular file.
HASHES = '''while IFS= read -r -d "" f; do
    if [ -f "$f" ] && [ ! -L "$f" ]; then
        printf "%s %s\\0" "$(sha256sum <"$f" | cut -c1-64)" "$f"
    fi
done'''

# The browser drivers of a tree on the lane (args: tree, lane root); prints
# own|linked|none|n/a. The first tree with a real install seeds the cache.
BROWSER_DEPS = '''wt=$1; root=$2
nm="$wt/workshop/tools/node_modules"; cache="$root/cache/workshop-tools/node_modules"
[ -d "$wt/workshop/tools" ] || { echo n/a; exit 0; }
if [ -d "$nm/playwright" ] && [ ! -L "$nm" ]; then echo own; exit 0; fi
if [ ! -d "$cache/playwright" ]; then
    for seed in "$root"/tree/workshop/tools/node_modules "$root"/slot/*/tree/workshop/tools/node_modules \\
                "$root"/wt/*/workshop/tools/node_modules; do
        if [ -d "$seed/playwright" ] && [ ! -L "$seed" ]; then
            # A copy of its own, moved in only while the cache is still absent: two
            # pushes on one host at once would otherwise move one copy INTO the other.
            tmp="$cache.tmp.$$"
            mkdir -p "${cache%/*}" && rm -rf "$tmp" && cp -a "$seed" "$tmp" \\
                && { [ -d "$cache" ] || mv -T "$tmp" "$cache"; }; rm -rf "$tmp"
            break
        fi
    done
fi
if [ -d "$cache/playwright" ]; then
    [ -L "$nm" ] || rm -rf "$nm"
    ln -sfn "$cache" "$nm" && echo linked
else
    echo none
fi'''

# What counts as a golden: a tracked or new (not ignored) file under a
# `fixtures/` or `golden/` directory; NUL-terminated `<sha256>  <path>`.
GOLDEN_HASH = '''spec=(":(glob)**/fixtures/**" ":(glob)**/golden/**")
{ git ls-files -z -- "${spec[@]}"; git ls-files -z -o --exclude-standard -- "${spec[@]}"; } \\
    | xargs -0 -r sha256sum -z -- 2>/dev/null | LC_ALL=C sort -z; true'''

def _digest_here(top, listing):
    res = subprocess.run(["bash", "-c", DIGEST], cwd=top, input=listing, capture_output=True)
    return res.stdout.decode().strip() if res.returncode == 0 else ""


def digest(repo_root):
    """The overlay hash of this tree (16 hex), as `lane_sync.sh digest` prints
    it under `LC_ALL=C` (the list is sorted in byte order)."""
    top = _top(repo_root)
    return _digest_here(top, _nul(_digest_list(top)))


def _records(data, sep=" "):
    """`<sha256><sep><path>` NUL records -> {path: sha256}."""
    out = {}
    for rec in data.split(b"\0"):
        if len(rec) > 64 + len(sep):
            out[os.fsdecode(rec[64 + len(sep):])] = rec[:64].decode()
    return out


# --------------------------------------------------------------------------
# The transport
# --------------------------------------------------------------------------

def sync_env(lane, **extra):
    """The environment of a `lane_sync.sh` call for `lane`: the settings
    `strand.sh lane_settings` exports from the host file, plus `extra`."""
    env = dict(os.environ)
    env.update({"MECLAW_LANE_NAME": lane.name, "MECLAW_LANE_ROOT": lane.root,
                "MECLAW_LANE_CLASS": "egon" if lane.cls == "egon" else "",
                "MECLAW_LANE_THREADS": str(lane.threads or ""), "MECLAW_LANE_JOBS": str(lane.jobs or ""),
                "MECLAW_LANE_SLOTS": str(lane.slots or ""),
                "MECLAW_LANE_WATCH": lane.opts.get("watch", ""),
                "MECLAW_LANE_SCCACHE": lane.opts.get("sccache", ""),
                "MECLAW_LANE_LINKER": lane.opts.get("linker", "")})
    env.update({k: str(v) for k, v in extra.items()})
    return env


def _sync(lane, *args, env=None, **kw):
    """`lane_sync.sh <args>` for `lane` (stdin closed unless given)."""
    kw.setdefault("stdin", subprocess.DEVNULL)
    return subprocess.run([LANE_SYNC] + [str(a) for a in args], env=env or sync_env(lane),
                          capture_output=True, **kw)


def _ssh_opts():
    return ["-o", "BatchMode=yes",
            "-o", "ConnectTimeout=%s" % os.environ.get("MECLAW_LANE_CONNECT_TIMEOUT", "10")]


def rsh():
    """The remote shell of an rsync or a git push to a lane (`-e`,
    GIT_SSH_COMMAND): the narrow point's checked program (`ssh_program`, its
    absolute path) with the transport's options; the program on the host
    comes from `rsync_path`/`receive_pack`."""
    return " ".join([q(ssh_program())] + _ssh_opts())


def _remote_argv(lane, script, long=False, env=None):
    """`ssh_argv` for `lane` (the narrow point; also for `local`)."""
    return ssh_argv("local" if lane.transport == "local" else lane.ssh, script, long=long, env=env)


def _remote(lane, script, input=b"", fail=None):
    """Run `script` on the lane (ssh, or a local bash). With `fail`, a
    non-zero exit raises `LaneError(2, fail)`."""
    res = subprocess.run(_remote_argv(lane, script), input=input, capture_output=True)
    if fail and res.returncode != 0:
        raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
    return res


def _lane_path(lane, path):
    return path if lane.transport == "local" else "%s:%s" % (lane.ssh, path)


def _git_push(lane, top, bare, refs):
    """git push of `refs` from `top` to the lane's bare repository `bare`:
    over ssh with the checked program (`rsh`), git-receive-pack on the host
    under `env -i` (`receive_pack`); a local lane pushes to the path. The
    rule of `lane_sync.sh lane_git_push`."""
    if lane.transport == "local":
        return subprocess.run(["git", "-C", top, "push", "-q", "--force", bare] + refs, capture_output=True)
    return subprocess.run([git_program(), "-C", top, "push", "-q", "--force", receive_pack(), _lane_path(lane, bare)] + refs,
                          env=dict(os.environ, GIT_SSH_COMMAND=rsh()), capture_output=True)


def _rsync(lane, args, cwd=None):
    argv = ["rsync"] if lane.transport == "local" else [rsync_program(), "-e", rsh(), rsync_path()]
    return subprocess.run(argv + args, cwd=cwd, capture_output=True)


# What a strand's tree name cannot carry (kit find tv-br M5, Welle Testing
# M7d): it becomes a ref (`refs/heads/lane/<name>`), a path (`wt/<name>`,
# `runs/<name>`) and a receipt directory. `strand.sh test --strand tv-br:m2`
# put the second lease's name into the push refspec, whose colon splits
# src:dst -- the push failed on the lane. git check-ref-format's characters,
# `@{`, a leading `-` and a `.lock` end are refused before anything travels.
_REF_BAD = re.compile(r"[:~^?*\[\\\x00-\x20\x7f]|@\{|^-|\.lock$|\.$|//")


def _check_name(name, what):
    if (not name or name.startswith("/") or "\0" in name or re.search(r"\s", name)
            or ".." in name.split("/") or (what == "tree" and _REF_BAD.search(name))):
        raise LaneError(2, "not a %s name: %r" % (what, name))


def _plain_https(url):
    """Only a plain https URL travels: no credentials, no blanks, no ssh."""
    if not url.startswith("https://") or "@" in url[len("https://"):]:
        return ""
    return "" if re.search(r"\s", url) else url


def _stderr(line):
    print("lane: " + line, file=sys.stderr)


def _two_paths(lane, verb, args, cargo, slot, expected_g=None):
    extra = {"MECLAW_LANE_CARGO": "1" if cargo else "0"}
    if slot is not None:
        extra["MECLAW_LANE_SLOT"] = slot
    if expected_g is not None:
        extra["MECLAW_LANE_EXPECTED_G"] = str(int(expected_g))
    res = _sync(lane, verb, *args, env=sync_env(lane, **extra))
    lines = res.stdout.decode().splitlines()
    if res.returncode != 0 or len(lines) != 2:
        err = "\n".join(l.replace("lane_sync: ", "", 1) for l in
                        res.stderr.decode(errors="replace").splitlines() if l.strip())
        raise LaneError(res.returncode if res.returncode in (2, 5) else 2,
                        err or "lane_sync.sh %s failed (exit %s)" % (verb, res.returncode))
    return lines[0], lines[1]


def paths(lane, tree_name, cargo, slot=None):
    """(build path, target) of a run of `tree_name` on `lane`: `lane_sync.sh
    paths` (W1 `build_path`/`target_path`) -- `tree/` + `target/` for slot 0,
    `slot/<n>/{tree,target}` for a slot the host file grants, `wt/<tree>` for
    a run without cargo. `slot` defaults to `MECLAW_LANE_SLOT` (0); a slot
    the lane does not grant raises `LaneError` code 5."""
    _check_name(tree_name, "tree")
    return _two_paths(lane, "paths", [tree_name], cargo, slot)


def guard(lane, tree_name, cargo=True, slot=None, expected_g=None):
    """The guards of a gate run on `lane` before anything travels --
    `lane_sync.sh guard` (W1):
    a slot the lane does not grant or a `local` lane without class `egon`
    raise `LaneError` code 5; a class `egon` lane without
    `MECLAW_LANE_EGON_GATE_OK`, or less than the floor free
    (`MECLAW_LANE_FLOOR_G`, 60 G; skipped under `MECLAW_LANE_WIPE_OK`), code 2;
    W2's hygiene in front of the floor (exit 6: no room even after collecting)
    is a refusal as well, code 2. `expected_g` (the admission's) is what the
    hygiene reserves for this run (`MECLAW_LANE_EXPECTED_G`, set explicitly,
    never inherited from the caller's environment). Returns `paths`."""
    _check_name(tree_name, "tree")
    return _two_paths(lane, "guard", [lane.ssh, tree_name], cargo, slot, expected_g)


def run_test(lane, tree_name, expr, extra, log, run="", on_line=None):
    """A nextest filterset on `lane` through `lane_sync.sh test` (W1: the tier
    in the build path, `--no-fail-fast`, the width and class of the lane --
    nice/ionice and the watchdog for class `egon` --, the floor, a `local`
    lane only as class `egon`, the golden switches and their way back).
    stdout and stderr go to the file `log`; returns the exit (255 = ssh lost
    the host, 5 = refused here, 2 = refused there or goldens not back).
    `run` names the run in a `watchdog_fired` line (`MECLAW_LANE_RUN`).
    `on_line(text)` (GH #1199): every line of the run as it arrives, after it
    is in `log` (flushed) -- the caller's pass line per stress iteration."""
    _check_name(tree_name, "tree")
    env = sync_env(lane, MECLAW_LANE_CARGO="1", MECLAW_LANE_RUN=run or "?")
    argv = [LANE_SYNC, "test", lane.ssh, tree_name, expr] + list(extra)
    with open(log, "ab") as fh:
        if on_line is None:
            res = subprocess.run(argv, env=env, stdin=subprocess.DEVNULL, stdout=fh,
                                 stderr=subprocess.STDOUT)
            rc = res.returncode
        else:
            proc = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT)
            for raw in proc.stdout:
                fh.write(raw)
                fh.flush()
                on_line(raw.decode("utf-8", "replace").rstrip("\n"))
            proc.stdout.close()
            rc = proc.wait()
    return rc if rc >= 0 else 128 - rc


class Hold:
    """A lane-side `flock` on `<build path>.lock`, held by one shell on the
    lane for as long as this side keeps its stdin open (`release`, or the end
    of this process: the pipe closes, the shell ends, the lock is free)."""

    def __init__(self, proc, lock, nonce=""):
        self.proc, self.lock, self.nonce = proc, lock, nonce

    def release(self):
        proc, self.proc = self.proc, None
        if proc is None:
            return
        try:
            proc.stdin.close()
        except OSError:
            pass
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.release()

    def __del__(self):
        self.release()


def hold(lane, tree):
    """Take the build path `tree` of `lane` for one push+run (W1 hand-over):
    a lane-side `flock -n` on `<tree>.lock`, whatever token or skip this side
    has -- a parallel push must not rewrite the sources under a running build.
    Returns a `Hold`; a held path raises `LaneError` code 5, a lane that does
    not answer code 2."""
    lock = tree.rstrip("/") + ".lock"
    # Z4: the holder's name beside the lock, as `lane_sync.sh hold` writes it
    # (HOLD_SH) -- a push there checks `<lock>.who` against MECLAW_LANE_HOLD.
    nonce = "%d-%d-%s" % (os.getpid(), time.time_ns(), os.urandom(4).hex())
    script = ("mkdir -p %s && exec 9>>%s && flock -n 9 || { echo busy; exit 0; }; "
              "printf %%s %s >%s; echo held; exec cat >/dev/null"
              % (q(os.path.dirname(lock)), q(lock), q(nonce), q(lock + ".who")))
    try:
        proc = subprocess.Popen(_remote_argv(lane, script, long=True), stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except OSError as exc:
        raise LaneError(2, "the host %s did not answer (%s)" % (lane.name, exc))
    word = proc.stdout.readline().decode(errors="replace").strip()
    if word == "held":
        return Hold(proc, lock, nonce)
    proc.stdin.close()
    err = proc.stderr.read()
    proc.wait()
    if word == "busy":
        raise LaneError(5, "refused -- the build path %s on %s is in use (another push+run "
                           "holds %s)" % (tree, lane.name, lock))
    raise LaneError(2, "the host %s did not answer (%s)" % (lane.name, _tail(err)))


# A run that built one package (`test -p`) leaves this mark beside the
# target's `.gate-tree`: what the build path held before the next push is then
# no proof of a build (GH #1071), and that push freshens every overlay file.
PARTIAL_MARK = ".gate-partial"


def partial(words):
    """Does a nextest word list build only part of the workspace (`-p`)?"""
    for w in words:
        if w == "--":
            return False
        if w in ("-p", "--package") or w.startswith("--package=") or (w.startswith("-p") and w[2:3] != "-"):
            return True
    return False


def mark_partial(lane, target):
    """Leave the partial-build mark in `target` on the lane."""
    _remote(lane, "mkdir -p %s && touch %s" % (q(target), q(target + "/" + PARTIAL_MARK)),
            fail="the partial mark on %s failed" % lane.name)


def push(lane, repo_root, tree_name, cargo, slot=None, kit=None, mirrors=(), assets=None,
         pins=None):
    """Commit + overlay of `repo_root` onto `lane`.

    `cargo=True` checks out into the build path of the slot (`paths`: `<root>/tree`
    or `<root>/slot/<n>/tree`), otherwise into `<root>/wt/<tree_name>`. Returns `{rev, overlay_hash, browser, tree,
    target, ws_root}`, plus `goldens_before` ({path: sha256} of the lane
    tree's goldens) when a golden switch is set here -- `fetch(pushed=...)`
    brings back what the run changed. A tracked secret or a different tree on
    the lane raises `LaneError` code 5; a lane that does not answer code 2.

    W10 (satellites): `kit=<core root>` also pushes the core's pipeline kit
    (`push_kit`, result `kit`), `mirrors=[<repo>, ...]` the source mirrors
    (`push_mirrors`), `assets=<cache dir>` the release-asset cache
    (`push_assets`), `pins=<register.PinTransport>` what the satellite's set
    pins need (`push_pins`, result `pins`, GH #1143). A tree without
    `scripts/gate_plan.py` gets no lane env.
    """
    _check_name(tree_name, "tree")
    top = _top(repo_root)
    sha = _git(top, "rev-parse", "HEAD").decode().strip()
    root = lane.root
    tree, target = paths(lane, tree_name, cargo, slot)
    bare = root + "/meclaw.git"

    # Before anything reaches the lane: no tracked secret. Fail closed.
    hits = collisions(top)
    if hits:
        raise LaneError(5, "refused -- tracked secret file(s), they would leave this machine:\n"
                        + "\n".join("  " + h for h in hits))

    _remote(lane, "mkdir -p %s && { test -d %s || git init -q --bare %s; }"
            % (q(root + "/wt"), q(bare), q(bare)), fail="the host %s did not answer" % lane.name)
    if lane.transport == "local":
        # A local lane borrows this clone's objects instead of copying the history.
        common = os.fsdecode(_git(top, "rev-parse", "--path-format=absolute", "--git-common-dir").strip())
        alternates = os.path.join(bare, "objects", "info", "alternates")
        if not os.path.isfile(alternates):
            with open(alternates, "w", encoding="utf-8") as fh:
                fh.write(os.path.realpath(os.path.join(common, "objects")) + "\n")

    refs = ["%s:refs/heads/lane/%s" % (sha, tree_name)]
    for ref in ("github-main", "master"):
        if subprocess.run(["git", "-C", top, "rev-parse", "--verify", "--quiet", "refs/heads/" + ref],
                          capture_output=True).returncode == 0:
            refs.append("refs/heads/%s:refs/heads/%s" % (ref, ref))
    res = _git_push(lane, top, bare, refs)
    if res.returncode != 0:
        raise LaneError(2, "git push to %s failed (%s)" % (lane.name, _tail(res.stderr)))
    url = _plain_https(subprocess.run(["git", "-C", top, "remote", "get-url", "origin"],
                                      capture_output=True, text=True).stdout.strip())
    if url:
        _remote(lane, "git --git-dir=%s config remote.origin.url %s" % (q(bare), q(url)),
                fail="the host %s did not answer" % lane.name)

    # GH #1071: the content of every overlay path as the lane holds it, before
    # the checkout rewrites it.
    overlay = overlay_files(top)
    listing = _nul(overlay)
    pre = _remote(lane, "cd %s 2>/dev/null && bash -c %s || true" % (q(tree), q(HASHES)),
                  input=listing, fail="the host %s did not answer" % lane.name).stdout
    mark = target + "/" + PARTIAL_MARK
    if _remote(lane, "test -f %s" % q(mark)).returncode == 0:
        pre = b""   # the last run built one package: nothing before counts as built
    else:
        mark = ""

    # The tree: checked out at the commit, cleaned of the last overlay
    # (`git clean` without -x keeps the ignored files a run left behind).
    _remote(lane, """set -e
        if git -C %(t)s rev-parse --git-dir >/dev/null 2>&1; then
            # W1: the last run touched files after its checkout (tree-sync,
            # the overlay stamp); the refresh compares content, so the forced
            # checkout rewrites only what differs -- not every member.
            git -C %(t)s update-index -q --refresh >/dev/null || true
            git -C %(t)s checkout -q --force --detach %(sha)s
            git -C %(t)s clean -fdq
        else
            rm -rf %(t)s
            mkdir -p %(p)s
            git --git-dir=%(b)s worktree prune
            git --git-dir=%(b)s worktree add -q --force --detach %(t)s %(sha)s
        fi""" % {"t": q(tree), "p": q(os.path.dirname(tree)), "b": q(bare), "sha": sha},
            fail="the checkout on %s failed" % lane.name)

    if overlay:
        with tempfile.NamedTemporaryFile(prefix="lane-overlay-") as fh:
            fh.write(listing)
            fh.flush()
            # `--checksum`: the quick check (size + mtime) skipped a change of
            # equal size within the checkout's second.
            res = _rsync(lane, ["-a", "--checksum", "--from0", "--files-from=" + fh.name,
                                "./", _lane_path(lane, tree) + "/"], cwd=top)
        if res.returncode != 0:
            raise LaneError(2, "the overlay to %s failed (%s)" % (lane.name, _tail(res.stderr)))
        post = subprocess.run(["bash", "-c", HASHES], cwd=top, input=listing, capture_output=True).stdout
        before = set(pre.split(b"\0"))
        fresh = sorted((rec[65:] for rec in post.split(b"\0") if rec and rec not in before))
        if fresh:
            _remote(lane, "cd %s && xargs -0 -r touch -c --" % q(tree),
                    input=b"".join(p + b"\0" for p in fresh), fail="the stamp on %s failed" % lane.name)
    if mark:
        _remote(lane, "rm -f %s" % q(mark), fail="the partial mark on %s failed" % lane.name)
    deleted = _deleted_tracked(top)
    if deleted:
        _remote(lane, "cd %s && xargs -0 rm -f --" % q(tree), input=_nul(deleted),
                fail="the deletions on %s failed" % lane.name)
    # The key-free `.env` of a lane, from the tree's own table; its name says
    # secret, so it is in neither hash list on either side.
    # the kit's one Python, isolated (re-check tv-bn M7r B-M2: a bare `python3`
    # from PATH was a second form of it on the host)
    _remote(lane, "cd %s && { test ! -f scripts/gate_plan.py || %s >.env; }"
            % (q(tree), " ".join(q(a) for a in python_argv("scripts/gate_plan.py", "--print", "lane-env"))),
            fail="the lane env on %s failed" % lane.name)
    browser = _remote(lane, "bash -c %s _ %s %s" % (q(BROWSER_DEPS), q(tree), q(root)),
                      fail="the browser drivers on %s failed" % lane.name).stdout.decode().strip()

    # The same tree on both sides, or no run there.
    dlist = _nul(_digest_list(top))
    here = _digest_here(top, dlist)
    there = _remote(lane, "cd %s && bash -c %s" % (q(tree), q(DIGEST)), input=dlist,
                    fail="the overlay hash on %s failed" % lane.name).stdout.decode().strip()
    if not here or here != there:
        raise LaneError(5, "refused -- the overlay on %s is not this tree (here %s, there %s)"
                        % (lane.name, here or "?", there or "?"))
    result = {"rev": sha, "overlay_hash": here, "browser": browser or "none", "tree": tree,
              "target": target, "ws_root": root + "/ws"}
    if golden_env():
        hashes = _remote(lane, "cd %s && bash -c %s" % (q(tree), q(GOLDEN_HASH)),
                         fail="the golden hashes on %s failed" % lane.name).stdout
        result["goldens_before"] = _records(hashes, sep="  ")
    if kit:
        result["kit"] = push_kit(lane, kit)
        result["display_lab"] = push_display_lab(lane, kit)
    if mirrors:
        result["mirrors"] = push_mirrors(lane, mirrors)
    if assets:
        result["assets"] = push_assets(lane, assets)
    if pins is not None:
        result["pins"] = push_pins(lane, pins)
    return result


# --------------------------------------------------------------------------
# What travels for a satellite (W10): kit, mirrors, assets
# --------------------------------------------------------------------------

def _target(lane):
    return None if lane.transport == "local" else lane.ssh


def _argv_there(lane, argv, fail):
    return _remote(lane, " ".join(q(w) for w in argv), fail=fail)


def push_kit(lane, core_root, rev=None):
    """The core's pipeline kit (`scripts/pipeline.py`, `scripts/pipeline/**`
    and the repository modules they import) of revision `rev` (default: HEAD
    of `core_root`) at `<root>/kit/<sha8>/` on `lane` -- once per revision,
    the marker `<sha8>.ok` last. Uncommitted kit files refuse (code 5): the
    directory would not be that revision. Returns the kit's directory there."""
    top = _top(core_root)
    rev = rev or _git(top, "rev-parse", "HEAD").decode().strip()
    spec = _register.kit_push_spec(top, rev, dest_root=lane.root.rstrip("/") + "/kit",
                                   target=_target(lane))
    dirty = _git(top, "status", "--porcelain", "--", *spec.files)
    # W10 re-check M-b: a deleted kit file is in no list taken from the tree,
    # so the kit's own roots are checked for tracked changes as well.
    dirty += _git(top, "status", "--porcelain", "--untracked-files=no", "--",
                  "scripts/pipeline.py", "scripts/pipeline")
    # Review Zb R2 M3: a deleted tracked module the kit imports from outside
    # its roots (`scripts/precheck.py`) is in no list taken from the tree either.
    gone = set(os.fsdecode(_git(top, "ls-files", "--deleted", "--", "scripts")).splitlines())
    if gone:
        start = [f for f in spec.files if f.endswith(".py")]
        for rel in sorted(gone & set(_register.kit_imports(top, start, gone))):
            dirty += (" D %s\n" % rel).encode()
    if dirty.strip():
        raise LaneError(5, "refused -- the kit of %s has uncommitted files:\n%s"
                        % (rev[:8], os.fsdecode(dirty).rstrip()))
    fail = "the kit push to %s failed" % lane.name
    # Review pd-1147 M-1 (a): the listing refreshes the marker in the same
    # command, so a reused kit is young again for the lane hygiene (KF5 keeps
    # a kit 24 h) before the run that names it starts -- no extra ssh hop.
    marker = q(spec.marker)
    listing = _remote(lane, "ls -1 %s 2>/dev/null; [ -e %s ] && touch %s; true"
                      % (q(spec.dest_root), marker, marker), fail=fail).stdout
    if not spec.already_there(os.fsdecode(listing)):
        _argv_there(lane, spec.remote_before(), fail)
        res = _rsync(lane, spec.rsync_argv()[1:])
        if res.returncode != 0:
            raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
        _argv_there(lane, spec.remote_after(), fail)
    return spec.dest.rstrip("/")


def push_display_lab(lane, core_root):
    """display-lab's fold below the lane user's home, where the satellites'
    labs look for it (W10 part B, G3). A local lane shares this home: None."""
    if lane.transport == "local":
        return None
    fail = "the display-lab push to %s failed" % lane.name
    for argv in _register.display_lab_spec(core_root, target=_target(lane)):
        res = _rsync(lane, argv[1:])
        if res.returncode != 0:
            raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
    return _register.DISPLAY_LAB_ROOT


def push_mirrors(lane, repos):
    """Each repository's git directory with all refs at
    `<root>/mirrors/<name>/.git/` on `lane`, secrets and lock files left
    out. Returns the mirror directories there."""
    dest_root = lane.root.rstrip("/") + "/mirrors"
    fail = "the mirror push to %s failed" % lane.name
    _argv_there(lane, ["mkdir", "-p", dest_root], fail)
    for argv in _register.mirrors_spec(repos, dest_root=dest_root, target=_target(lane)):
        res = _rsync(lane, argv[1:])
        if res.returncode != 0:
            raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
    return _register.mirror_dirs(repos, dest_root=dest_root)


def push_assets(lane, cache_root, versions=None, dest_root=None):
    """The release-asset cache `<cache_root>` (`~/.cache/<repo>/meclaw/`)
    below `dest_root` on `lane` (default: the lane user's `~/.cache`). A
    local lane without `dest_root` shares this home: nothing travels, None.
    Returns the destination."""
    if dest_root is None:
        if lane.transport == "local":
            return None
        dest_root = _register.ASSETS_ROOT
    fail = "the asset push to %s failed" % lane.name
    _argv_there(lane, ["mkdir", "-p", dest_root], fail)
    for argv in _register.assets_spec(cache_root, dest_root=dest_root, target=_target(lane),
                                      versions=versions):
        res = _rsync(lane, argv[1:])
        if res.returncode != 0:
            raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
    return dest_root


def push_pins(lane, transport, home_root=None):
    """What a satellite's set pins need (`register.set_pins`, GH #1143) below
    the lane user's home (`home_root`): the pinned releases of the asset
    cache and the source mirrors, meclaw-core included, at the path their
    pins name -- additive, nothing there is deleted. Then the lane must
    resolve every pinned revision and hold every release, else code 2. A
    local lane without `home_root` shares this home: nothing travels, None."""
    if home_root is None:
        if lane.transport == "local":
            return None
        home_root = "."
    fail = "the pin push to %s failed" % lane.name
    # `assets_spec` keeps `<repo>/meclaw/<v>/` below its destination: the
    # cache's grandparent (`~/.cache`) relative to the home, there.
    cache_rel = os.path.relpath(transport.cache, transport.home)
    assets_dest = "%s/%s" % (home_root.rstrip("/"), os.path.dirname(os.path.dirname(cache_rel)))
    _argv_there(lane, ["mkdir", "-p", home_root, assets_dest], fail)
    argvs = _register.pins_mirror_spec(transport, dest_root=home_root, target=_target(lane))
    if transport.versions:
        argvs += _register.assets_spec(transport.cache, dest_root=assets_dest,
                                       target=_target(lane), versions=transport.versions)
    for argv in argvs:
        res = _rsync(lane, argv[1:])
        if res.returncode != 0:
            raise LaneError(2, "%s (%s)" % (fail, _tail(res.stderr)))
    checks = ["test -d %s || echo %s" % (q("%s/%s/%s" % (home_root, cache_rel, v)),
                                         q("release %s" % v)) for v in transport.versions]
    for m in transport.mirrors:
        gdir = "%s/%s/.git" % (home_root, m["rel"])
        checks += ["git --git-dir=%s cat-file -e %s 2>/dev/null || echo %s"
                   % (q(gdir), q(rev + "^{commit}"), q("%s %s" % (m["rel"], rev[:12])))
                   for rev in m["revs"]]
    missing = _remote(lane, "; ".join(checks) or "true", fail=fail).stdout.decode().splitlines()
    missing = [x for x in missing if x.strip()]
    if missing:
        raise LaneError(2, "%s -- the lane lacks: %s" % (fail, ", ".join(missing)))
    return {"home": home_root, "versions": list(transport.versions),
            "mirrors": [m["rel"] for m in transport.mirrors]}


def _golden_back(lane, tree, before, top):
    """The goldens the run changed, from the lane's tree into this one. A
    golden whose file HERE no longer matches the state before the run was
    edited here meanwhile: it stays and is named as a conflict."""
    back, conflicts = [], []
    res = _remote(lane, "cd %s && bash -c %s" % (q(tree), q(GOLDEN_HASH)))
    if res.returncode != 0:
        _stderr("golden fetch failed -- no hashes from %s" % lane.name)
        return back, conflicts, True
    after = _records(res.stdout, sep="  ")
    for path in sorted(after, key=os.fsencode):
        if after[path] == before.get(path) or secret_name(path):
            continue
        local = os.path.join(top, path)
        here = ""
        if os.path.isfile(local):
            with open(local, "rb") as fh:
                here = hashlib.sha256(fh.read()).hexdigest()
        if here != before.get(path, ""):
            _stderr("golden conflict: %s -- changed here during the run, kept; "
                    "the lane's copy stays on %s" % (path, lane.name))
            conflicts.append(path)
            continue
        back.append(path)
    if back:
        with tempfile.NamedTemporaryFile(prefix="lane-goldens-") as fh:
            fh.write(_nul(back))
            fh.flush()
            res = _rsync(lane, ["-a", "--from0", "--files-from=" + fh.name,
                                _lane_path(lane, tree) + "/", top + "/"])
        if res.returncode != 0:
            _stderr("golden fetch failed -- rsync from %s" % lane.name)
            return [], conflicts, True
        for path in back:
            _stderr("golden back: " + path)
    elif not conflicts:
        _stderr("golden back: none -- the run changed no golden")
    return back, conflicts, False


def fetch(lane, run, dest_dir, pushed=None, repo_root=None):
    """`<ws_root>/<run>/{logs/,receipt.json,events.jsonl}` from the lane into
    `dest_dir`; with `pushed` (the result of `push` under a golden switch),
    also the goldens the run changed, into `repo_root` (default: the cwd's
    repository). Returns {dest, receipt, events, logs, goldens_back,
    golden_conflicts, golden_failed}; the caller turns a conflict or a failed
    golden fetch into its exit (lane_sync.sh: 2). A run that is not on the
    lane raises `LaneError` code 2. Call it with `pushed` only after a run
    that ended (a lost stream brings nothing back)."""
    _check_name(run, "run")
    ws = "%s/ws/%s" % (lane.root, run)
    dest = dest_dir.rstrip("/") or "/"
    os.makedirs(dest, exist_ok=True)
    res = _rsync(lane, ["-a", "--include=/logs/***", "--include=/receipt.json",
                        "--include=/events.jsonl", "--exclude=*",
                        _lane_path(lane, ws) + "/", dest + "/"])
    if res.returncode != 0:
        raise LaneError(2, "could not fetch the run %s from %s (%s)" % (run, lane.name, _tail(res.stderr)))
    logs = os.path.join(dest, "logs")
    out = {"dest": dest, "receipt": os.path.isfile(os.path.join(dest, "receipt.json")),
           "events": os.path.isfile(os.path.join(dest, "events.jsonl")),
           "logs": sorted(os.listdir(logs)) if os.path.isdir(logs) else [],
           "goldens_back": [], "golden_conflicts": [], "golden_failed": False}
    if pushed and pushed.get("goldens_before") is not None:
        top = _top(repo_root or os.getcwd())
        back, conflicts, failed = _golden_back(lane, pushed["tree"], pushed["goldens_before"], top)
        out.update(goldens_back=back, golden_conflicts=conflicts, golden_failed=failed)
    return out


# A non-interactive ssh shell does not read the profile that puts rustup's
# `~/.cargo/bin` on PATH; a command for the stations of a host does it itself --
# the executor (`exec_stream`) and the probe of their build environment
# (`executor.stations_build`): one line.
REMOTE_PATH = 'export PATH="$HOME/.cargo/bin:$PATH"'


def exec_stream(lane, dispatch, stderr=None, pipeline=None):
    """Start the executor of `dispatch` on the lane; returns the `Popen` whose
    stdout is the JSONL event stream (text, line by line). The dispatch goes
    on stdin, the executor reads it whole before its first event.

    ssh: `ssh <lane> ... systemd-run --user --scope --unit=pipe-<run> --collect
    python3 <tree>/scripts/pipeline.py exec --dispatch -` (a lost host = exit
    255 without `pass_ended`). local: the same under `systemd-run` when this
    machine has it, else `python3 ...` directly. `MECLAW_PIPELINE_NO_SCOPE=1`
    drops the scope on both transports. `proc.scoped` says whether the run
    got its scope: only then does `unit_state` speak for it (I1). `pipeline`
    names another starter -- a satellite's tree has none, its pass runs the
    kit's (W10)."""
    run, tree = dispatch["run"], dispatch["tree"]
    _check_name(run, "run")
    inner = python_argv(pipeline or tree + "/scripts/pipeline.py", "exec", "--dispatch", "-")
    scope = ["systemd-run", "--user", "--scope", "--unit=pipe-" + run, "--collect"]
    scoped = os.environ.get("MECLAW_PIPELINE_NO_SCOPE") != "1" and (
        lane.transport != "local" or bool(shutil.which("systemd-run")))
    if lane.transport == "local":
        argv = scope + inner if scoped else inner
        cwd = tree
    else:
        # A non-interactive ssh shell does not read the profile that puts
        # rustup's `~/.cargo/bin` on PATH; the command does it itself.
        command = " ".join(q(w) for w in (scope + inner if scoped else inner))
        argv = _remote_argv(lane, '%s && cd %s && %s' % (REMOTE_PATH, q(tree), command), long=True)
        cwd = None
    try:
        proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=stderr, text=True, bufsize=1, cwd=cwd)
    except OSError as exc:
        raise LaneError(2, "the executor on %s did not start (%s)" % (lane.name, exc))
    proc.scoped = scoped
    try:
        proc.stdin.write(json.dumps(dispatch))
        proc.stdin.close()
    except BrokenPipeError:
        pass  # the stream ends without `pass_ended`: the caller says `lost`
    return proc


def abort(lane, tree, run, ws=None):
    """`pipeline.py exec --abort <run> [--ws <ws>]` on the lane: the executor
    of `run` aborts like a watcher and ends its stream with `pass_ended
    ABORTED` (GR-iii). Returns the exit: 0 = signalled, 1 = no such executor,
    255 = the host did not answer."""
    _check_name(run, "run")
    inner = python_argv(tree + "/scripts/pipeline.py", "exec", "--abort", run) + (["--ws", ws] if ws else [])
    try:
        res = subprocess.run(_remote_argv(lane, " ".join(q(w) for w in inner)), stdin=subprocess.DEVNULL,
                             capture_output=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise LaneError(2, "could not abort %s on %s (%s)" % (run, lane.name, exc))
    return res.returncode


def unit_state(lane, run, timeout=30):
    """`systemctl --user is-active pipe-<run>.scope` on the lane (the scope
    `exec_stream` starts): the word it prints ("active", "inactive",
    "failed", ...), or None when the probe found no answer (ssh 255, a
    timeout, a refusal without output). Z13: run-696ff322bd86 died at import
    on build03, the fetch failed, and only this probe tells a dead run from
    a lane that merely did not answer."""
    _check_name(run, "run")
    script = "systemctl --user is-active " + shlex.quote("pipe-%s.scope" % run)
    try:
        res = subprocess.run(_remote_argv(lane, script), stdin=subprocess.DEVNULL,
                             capture_output=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if lane.transport != "local" and res.returncode == 255:
        return None
    words = res.stdout.decode(errors="replace").split()
    return words[0] if words else None


# The probe marks itself: its own shell (and the subshells it forks) carry the
# marker in their command line and are skipped.
WS_PROBE_MARK = "meclaw-ws-probe"
WS_PROBE_END = "ws-probe-end"
# A word of cmdline/environ names the workspace only when the path ends there or
# goes on with a character no file name continues with ("/", ":", "=", ...):
# the substring match of `grep -F` took `.../lane10` for the root `.../lane1`
# and held the neighbour lane's lease (re-check tv-kit fix11 H4). `grep -F`
# stays the cheap prefilter; this runs only on its hits.
WS_PATH_WORD = (
    "function hit(s, p,  i, c) { if (p == \"\") return 0; "
    "while ((i = index(s, p)) > 0) { c = substr(s, i + length(p), 1); "
    "if (c == \"\" || c !~ /[A-Za-z0-9._~+-]/) return 1; s = substr(s, i + 1) } return 0 } "
    "hit($0, a) || hit($0, b) { f = 1; exit } END { exit !f }")


def ws_procs(lane, ws, timeout=30):
    """The pids on the lane that live in the run workspace `ws`: a cwd at or
    below `ws`, or `ws` in the command line. A list (empty: nothing there),
    or None when the probe found no answer (ssh 255, a timeout, no end
    marker) -- a caller that frees a lane reads None as "maybe busy".

    Also matched (re-check tv-kit fix10): `ws` in the environment (H1: a
    source tree outside the root whose CARGO_TARGET_DIR alone points below
    it hid the parent cargo, measured), and the canonical form of `ws` (H2:
    `readlink cwd` is canonical, a symlinked root matched no cwd, measured).
    An empty `readlink -f` falls back to `ws` -- an empty pattern would match
    every process. A cmdline/environ word matches only at a path boundary
    (`WS_PATH_WORD`, fix11 H4): `.../lane1` never finds `.../lane10`."""
    if not ws or not ws.startswith("/") or ws.rstrip("/") in ("", "/"):
        raise LaneError(2, "no workspace path to probe: %r" % ws)
    script = (
        ": %s; ws=%s; wr=$(readlink -f -- \"$ws\" 2>/dev/null); "
        "case \"$wr\" in /?*) ;; *) wr=$ws ;; esac; "
        "for d in /proc/[0-9]*; do "
        "grep -aqsF -- %s \"$d/cmdline\" && continue; "
        "c=$(readlink \"$d/cwd\" 2>/dev/null); "
        "case \"$c\" in \"$ws\"|\"$ws\"/*|\"$wr\"|\"$wr\"/*) echo \"${d#/proc/}\"; continue ;; esac; "
        "grep -aqsF -e \"$ws\" -e \"$wr\" -- \"$d/cmdline\" \"$d/environ\" "
        "&& cat -- \"$d/cmdline\" \"$d/environ\" 2>/dev/null | tr '\\0' '\\n' "
        "| awk -v a=\"$ws\" -v b=\"$wr\" %s && echo \"${d#/proc/}\"; "
        "done; echo %s"
        % (WS_PROBE_MARK, shlex.quote(ws.rstrip("/")), WS_PROBE_MARK, shlex.quote(WS_PATH_WORD),
           WS_PROBE_END))
    try:
        res = subprocess.run(_remote_argv(lane, script), stdin=subprocess.DEVNULL,
                             capture_output=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        return None
    words = res.stdout.decode(errors="replace").split()
    if not words or words[-1] != WS_PROBE_END:
        return None
    # isascii: '²'.isdigit() is True, int('²') a ValueError (re-check fix10 H5)
    return [int(w) for w in words[:-1] if w.isascii() and w.isdigit()]


WS_KILL_GRACE_TENTHS = 50   # TERM, then up to 5 s for the run to end, then KILL


def ws_kill(lane, ws, pids, timeout=30):
    """End the pids `pids` that a `ws_procs` probe found in the run workspace
    `ws` (GH #1228: a forced lease release ends the lane run under it): TERM,
    up to `WS_KILL_GRACE_TENTHS` tenths of a second for them to end, KILL the
    rest, then probe `ws` again. Returns what still lives there (a list, empty
    when the tree is idle) or None when the lane gave no answer. A zombie is
    ended (it waits only for its parent's reap). Each pid is checked again at
    the signal, by the probe's own rule (`ws_procs`: cwd at or below `ws`, or
    `ws` as a word of cmdline/environ): a pid that left the tree since the
    probe -- ended and reused by another process -- is never signalled
    (re-check v2-r3-lease M1)."""
    if not ws or not ws.startswith("/") or ws.rstrip("/") in ("", "/"):
        raise LaneError(2, "no workspace path to end pids in: %r" % ws)
    words = [str(p) for p in pids if isinstance(p, int) and p > 1]
    if words:
        script = (
            ": %s; ws=%s; wr=$(readlink -f -- \"$ws\" 2>/dev/null); "
            "case \"$wr\" in /?*) ;; *) wr=$ws ;; esac; "
            "own() { d=/proc/$1; grep -aqsF -- %s \"$d/cmdline\" && return 1; "
            "c=$(readlink \"$d/cwd\" 2>/dev/null); "
            "case \"$c\" in \"$ws\"|\"$ws\"/*|\"$wr\"|\"$wr\"/*) return 0 ;; esac; "
            "grep -aqsF -e \"$ws\" -e \"$wr\" -- \"$d/cmdline\" \"$d/environ\" "
            "&& cat -- \"$d/cmdline\" \"$d/environ\" 2>/dev/null | tr '\\0' '\\n' "
            "| awk -v a=\"$ws\" -v b=\"$wr\" %s; }; "
            "ps=; for p in %s; do own $p && ps=\"$ps $p\"; done; "
            "[ -z \"$ps\" ] || kill -TERM $ps 2>/dev/null; "
            "up() { for p in $ps; do [ -d /proc/$p ] && "
            "! grep -qs '^State:[[:space:]]*[ZX]' /proc/$p/status && own $p && echo $p; done; }; "
            "i=0; while [ $i -lt %d ] && [ -n \"$(up)\" ]; do sleep 0.1; i=$((i+1)); done; "
            "left=$(up); [ -z \"$left\" ] || kill -KILL $left 2>/dev/null; echo %s"
            % (WS_PROBE_MARK, shlex.quote(ws.rstrip("/")), WS_PROBE_MARK, shlex.quote(WS_PATH_WORD),
               " ".join(words), WS_KILL_GRACE_TENTHS, WS_PROBE_END))
        try:
            res = subprocess.run(_remote_argv(lane, script), stdin=subprocess.DEVNULL,
                                 capture_output=True, timeout=timeout)
        except (OSError, subprocess.TimeoutExpired):
            return None
        out = res.stdout.decode(errors="replace").split()
        if not out or out[-1] != WS_PROBE_END:
            return None
    return ws_procs(lane, ws, timeout=timeout)


def fingerprint(lane, script=None):
    """The lane's fingerprint (V0 section 2.3) as a dict: W1's
    `lane_fingerprint.sh` beside this module, sent on stdin (`sh -s`), so the
    lane needs no copy of the kit."""
    script = script or os.path.join(os.path.dirname(os.path.abspath(__file__)), "lane_fingerprint.sh")
    try:
        with open(script, "rb") as fh:
            body = fh.read()
    except OSError:
        raise LaneError(2, "no fingerprint script at %s" % script)
    # under the stations' words (`lane_sync.sh env`): the linker and the build
    # environment are read from them there, never sent as a word of their own
    # (re-check tv-bn M7r A-M2, one source); `incremental_policy` too (M7t A-M3)
    # (`test`: the words alone, without the runner's floor, which is no lane word)
    words = " ".join("%s=%s" % (k, q(v)) for k, v in env_for(lane, "test").items())
    # ... and under the stations' PATH (REMOTE_PATH, as `exec_stream` and
    # `executor.stations_build` start them): without it the register's `build_env`
    # carried the login PATH and the first run after every `lanes fingerprint` noted
    # `lane-drift [build_env]` -- 46 receipts of welle-validierung-2, all four lanes
    # (ops-build01, Runde 3 R3 (4))
    res = _remote(lane, "%s && env %s sh -s -- %s" % (REMOTE_PATH, words, q(lane.root)),
                  input=body, fail="the host %s did not answer" % lane.name)
    try:
        fp = json.loads(res.stdout.decode())
    except ValueError:
        raise LaneError(2, "the fingerprint of %s is no JSON" % lane.name)
    if not isinstance(fp, dict):
        raise LaneError(2, "the fingerprint of %s is no JSON object" % lane.name)
    from . import executor   # its field list (re-check tv-bn M7r A-M1: a half answer is none)
    missing = [k for k in executor.FINGERPRINT_FIELDS if k not in fp]
    if missing:
        raise LaneError(2, "the fingerprint of %s has no %s" % (lane.name, ",".join(missing)))
    return fp


def status(lane):
    """One status line for `lanes status`: exactly `lane_sync.sh status` (W1:
    `reachable  fs .. free .. bare .. deps .. incr .. gen .. wt .. slots ..
    sccache .. fp .. state ..`, or `unreachable  state off|booting`). Never
    raises."""
    try:
        res = _sync(lane, "status", lane.ssh)
    except OSError:
        return "unreachable  state off"
    lines = res.stdout.decode(errors="replace").splitlines()
    if res.returncode != 0 or not lines or not lines[0].strip():
        return "unreachable  state off"
    return lines[0].rstrip()
