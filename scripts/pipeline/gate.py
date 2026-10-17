"""meclaw -- `pipeline.py gate` and `pipeline.py test`: the kernel side of one pass.

`gate <mode> [flags]` plans the pass (`plan.build_plan`), looks up the cache,
and runs it: a plan without a cargo station (or `--lane`, the run already on
its lane) runs the executor here, without a lease; any other plan is admitted
by the scheduler (`schedule.admit`), pushed to its lane (`lane.push`) and
executed there (`lane.exec_stream`), every event folded into the store
(`schedule.ingest`). The lines it prints are the ones of `scripts/gate.sh`:
`GATE ...` per station as it ends, `GATE-SUMMARY ...`, plus `GATE-PLAN ...`
before the start and `GATE-LANE ...` at the end. Exit: GREEN 0, RED 1,
ABORTED or a refused/lost pass 2, queued 3, ASK 4.

Local run data lives under `~/.local/state/meclaw/pipeline/` (`runs/<run>/`,
`metadata/<sha256>.json`, `local-fingerprint.json`).
"""
import contextlib
import datetime
import glob
import hashlib
import importlib
import json
import math
import os
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
import uuid

from . import cache, cpus, events, flakes, full_pass, gitenv, lane, plan, progress, receipt, schedule, store
from . import register as satellites

MODES = ("strand", "integration", "release")
ASKING_STATIONS = ("persona-receipt",)
EXIT_BY_VERDICT = {"GREEN": 0, "RED": 1, "ABORTED": 2, "ASK": 4}
PRE_MODULE = "%s.pre" % __package__
DEFAULT_LOCK = "/tmp/meclaw-w26-cargo.lock"
VALUE_FLAGS = ("--base", "--only", "--archive", "--log-dir", "--decide", "--host", "--strand",
               "--wave", "--reason", "--repo", "--slots", "--holder", "--cache")
SWITCHES = {"--fail-fast": "fail_fast", "--plan-only": "plan_only", "--no-nice": "no_nice",
            "--lane": "lane", "--resync": "resync", "--no-precheck-stop": "no_precheck_stop",
            "--no-pre": "no_pre", "--wait-admission": "wait_admission", "--full": "full",
            "--any-lane": "any_lane"}
# The stations cache switch (GH #1162, messvertrag section 1): `off` makes no
# lookup, so every planned station runs; results are still recorded under the
# same keys (the key never sees the switch, later runs keep their hits).
CACHE_ENV = "MECLAW_GATE_CACHE"
CACHE_VALUES = ("on", "off")
GRACE_S = 1800
STORE_EXIT = 6

USAGE = """usage: pipeline.py gate <strand|integration|release> [--base <rev>] [--only <stations>]
         [--mode <strand|integration|release>]
         [--archive <dir>] [--log-dir <dir>] [--fail-fast] [--plan-only] [--no-nice]
         [--lane] [--resync] [--no-precheck-stop] [--decide <station>=without:"OR-<id>: <reason>"]
         [--host <lane>] [--strand <name>] [--wave <wave>] [--no-pre --reason <text>]
         [--repo <path>] [--slots <n,...>] [--wait-admission] [--holder <n>] [--full]
         [--cache on|off] [--any-lane]

Plans one pass of the gate and runs it -- here when no station needs cargo,
else on a lane the scheduler admits. Exit: GREEN 0, RED 1, ABORTED 2,
queued 3, ASK 4; a refused argument is 2, a lane that refuses this
tree 5, a store error at the admission 6. The mode stands first or as
`--mode <m>` anywhere; two different modes are refused.

--repo plans the repository at <path> instead of the current one; a
satellite (coding, soul2me, orga: `pipeline/stations.json`) is planned from
its register and always runs on a lane the admission books (--host names the
preferred one), from the core's kit there -- never here and never on an egon
lane (--lane is refused, exit 2). --slots hands a satellite's lab stations
their slot pool.

--wait-admission blocks while the pass is queued (exit 3 otherwise) and is
admitted when a lease ends or a run closes -- no retry clock (GH #1145); a
cause nothing resolves (a lane off, an egon lane, no lane) and the end of
MECLAW_ADMISSION_WAIT_SECS (default 4 h) end it with exit 3.
--holder <n> runs under a second lease of the strand (`<strand>:<n>`, its own
lane and workpackage, GH #1146); `strand.sh token ... --holder <n>` frees it.
--full plans a full pass: an integration the full suite, a strand or a merge
without narrowing. A narrowed pass with an unexplained red, a FLAKY station, a
coverage map that does not fit or a cache doubt plans the next pass of its
wave so too, until a GREEN full pass clears the mark (`full_pass`, #1165).
--cache off (or MECLAW_GATE_CACHE=off) looks nothing up in the stations cache:
every planned station runs, results are still recorded; GATE-PLAN and GATE-LANE
say `cache=off`, and a CACHED station in such a pass turns it RED (a defect).
integration and release wait for the widest lane (most threads); --any-lane
lets them take a narrower one. GATE-LANE names the lane's threads (`vcpu=`)."""


# --- state ---------------------------------------------------------------------------------

def state_dir():
    """`~/.local/state/meclaw/pipeline` (`XDG_STATE_HOME` moves it)."""
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(os.path.expanduser("~"), ".local", "state")
    return os.path.join(base, "meclaw", "pipeline")


# Where cargo discovers a target without a `[[test]]`/`[[bin]]` entry
# (relative to a member): a new file here is a new nextest binary.
DISCOVERED_TARGETS = ("tests/*.rs", "tests/*/main.rs", "src/bin/*.rs", "src/bin/*/main.rs",
                      "examples/*.rs", "examples/*/main.rs", "benches/*.rs", "benches/*/main.rs")


def _member_dirs(repo_root):
    """The workspace members of `repo_root` (their globs expanded); `crates/*`
    when the root manifest cannot be read."""
    patterns = ["crates/*"]
    try:
        import tomllib
        with open(os.path.join(repo_root, "Cargo.toml"), "rb") as fh:
            members = (tomllib.load(fh).get("workspace") or {}).get("members")
        if isinstance(members, list):
            patterns = [m for m in members if isinstance(m, str)]
    except (ImportError, OSError, ValueError):
        pass
    found = set()
    for pat in patterns:
        for d in glob.glob(os.path.join(repo_root, pat)):
            if os.path.isfile(os.path.join(d, "Cargo.toml")):
                found.add(os.path.relpath(d, repo_root))
    return sorted(found)


def metadata_path(repo_root):
    """The cached `cargo metadata` of `repo_root`: keyed by Cargo.lock, the
    root manifest, every member's manifest and the names of every target
    cargo discovers on its own (Delta B I-neu1) -- a new tests/*.rs is a new
    key, so a cache made before it never hides its binary from the plan
    (without metadata every named binary is built and run)."""
    h = hashlib.sha256()
    files = ["Cargo.lock", "Cargo.toml"]
    members = _member_dirs(repo_root)
    files += [os.path.join(m, "Cargo.toml") for m in members]
    for name in files:
        h.update(name.encode() + b"\0")
        try:
            with open(os.path.join(repo_root, name), "rb") as fh:
                h.update(fh.read())
        except OSError:
            h.update(b"-")
        h.update(b"\0")
    for m in members:
        for pat in DISCOVERED_TARGETS:
            for path in sorted(glob.glob(os.path.join(repo_root, m, pat))):
                h.update(os.path.relpath(path, repo_root).encode() + b"\0")
    return os.path.join(state_dir(), "metadata", h.hexdigest() + ".json")


def _load_json(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def _write_json(path, doc):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = "%s.tmp-%d" % (path, os.getpid())
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(receipt.dumps(doc))  # one serializer with `pipeline receipt` (#1152)
    os.replace(tmp, path)


def _local_fp_path():
    return os.path.join(state_dir(), "local-fingerprint.json")


def _local_register(old, new):
    """The local lane's register after a run said `new`: the register rule of the
    lanes (`schedule.merge_register`) -- a field the run did not measure, the
    `browsers` of a run that books no engine (#1199 K14), keeps its value."""
    return schedule.merge_register(old if isinstance(old, dict) else {}, new, None)


# --- arguments -----------------------------------------------------------------------------

class Opts:
    def __init__(self):
        self.mode = ""
        self.base = self.only = self.archive = self.host = self.strand = self.wave = None
        self.reason = self.repo = self.slots = self.holder = self.cache = None
        self.decide = []
        for attr in SWITCHES.values():
            setattr(self, attr, False)
        self.help = False


def _take_modes(argv):
    """The modes `argv` names, taken out of it in place: the positional first word
    (or a mode word after a flag) and every `--mode <m>` / `--mode=<m>` wherever
    it stands (#1199 K13: `gate --repo <r> --mode strand` read `--repo` as the
    mode). A value flag's value is never one (`--reason --mode`). `None`: a
    `--mode` without its value."""
    modes, rest, i = [], [], 0
    if argv and not argv[0].startswith("-"):
        modes.append(argv[0])
        i = 1
    while i < len(argv):
        a = argv[i]
        if a in VALUE_FLAGS:
            rest.extend(argv[i:i + 2])
            i += 2
            continue
        if a == "--mode":
            if i + 1 >= len(argv):
                return None
            modes.append(argv[i + 1])
            i += 2
            continue
        if a.startswith("--mode="):
            modes.append(a[len("--mode="):])
        elif a in MODES:  # the positional form after a flag
            modes.append(a)
        else:
            rest.append(a)
        i += 1
    argv[:] = rest
    return modes


def parse(argv, err):
    """`Opts`, or the exit code of a refused command line (2) -- gate.sh's checks."""
    o = Opts()
    argv = list(argv)
    if argv and argv[0] in ("-h", "--help"):
        o.help = True
        return o
    if not argv:
        print(USAGE, file=err)
        return 2
    modes = _take_modes(argv)
    if modes is None:
        print(USAGE, file=err)
        return 2
    if len(set(modes)) > 1:
        print("gate: two modes: %s (`--mode <m>` and the positional `<m>` name the same "
              "one)" % " and ".join(modes), file=err)
        return 2
    if not modes:
        print("gate: no mode (expected strand|integration|release, positional or "
              "--mode <m>)", file=err)
        return 2
    o.mode = modes[0]
    if o.mode not in MODES:
        print("gate: unknown mode: %s (expected strand|integration|release; ci runs "
              "scripts/gate_legacy.sh)" % o.mode, file=err)
        return 2
    o.archive = os.environ.get("MECLAW_GATE_ARCHIVE") or None
    while argv:
        a = argv.pop(0)
        if a in VALUE_FLAGS:
            if not argv:
                print("gate: %s needs a value" % a, file=err)
                return 2
            v = argv.pop(0)
            if a == "--decide":
                o.decide.append(v)
            elif a == "--log-dir":
                o.archive = v
            elif a == "--only":  # a list for the plan and the executor, never a string (B-I1)
                o.only = [n.strip() for n in v.split(",") if n.strip()] or None
            else:
                setattr(o, a[2:], v)
        elif a in SWITCHES:
            setattr(o, SWITCHES[a], True)
        elif a in ("-h", "--help"):
            o.help = True
        else:
            print("gate: unknown argument: %s" % a, file=err)
            return 2
    if o.lane and not o.base:
        print("gate: --lane needs --base <sha> -- a build host has no reliable master; "
              "the kit computes the merge base where it does", file=err)
        return 2
    for d in o.decide:
        bad = _decide_error(o.mode, d)
        if bad:
            print("gate: " + bad, file=err)
            return 2
    if schedule.strand_colon_error(o.strand):
        print("gate: " + schedule.strand_colon_error(o.strand), file=err)
        return 2
    if o.holder is not None and not schedule.HOLDER_RE.match(o.holder):
        print("gate: --holder takes a short suffix ([A-Za-z0-9_-], up to 32): %s" % o.holder,
              file=err)
        return 2
    o.cache = (o.cache or os.environ.get(CACHE_ENV) or "on").strip()
    if o.cache not in CACHE_VALUES:
        print("gate: --cache takes on|off, not %r" % o.cache, file=err)
        return 2
    if o.no_pre and not (o.reason or "").strip():
        print("gate: --no-pre needs --reason <text> -- the skip is written into the receipt", file=err)
        return 2
    return o


# OR-DT.ZI.decide-or: an owner's answer cites the ruling that carries it.
DECIDE_RULING_RE = re.compile(r"OR-[\w.-]*\w")


def _decide_error(mode, d):
    if mode not in ("integration", "release"):
        return "--decide answers a question only integration and release ask (this is %s)" % mode
    if "=" not in d or ":" not in d.split("=", 1)[1]:
        return '--decide wants <station>=without:"<reason>", got: %s' % d
    st, rest = d.split("=", 1)
    ans, why = rest.split(":", 1)
    if st not in ASKING_STATIONS:
        return "--decide names %s, which never asks (asking stations: %s)" % (st, " ".join(ASKING_STATIONS))
    if ans != "without":
        return ("--decide %s=%s -- the only answer is 'without'; the other two answers are "
                "measurements, not switches" % (st, ans))
    if not why.strip():
        return "--decide %s=without needs a reason -- the owner's call is written into the receipt" % st
    if "\t" in why or "\n" in why:
        return ("--decide %s: the reason must be one line without tabs (it is one field of a "
                "tab-separated record)" % st)
    if not DECIDE_RULING_RE.search(why):
        return ('--decide %s=without needs the ruling id in the reason (OR-<id>, OR-DT.ZI.decide-or), '
                'e.g. %s=without:"OR-DT.W5.persona-paths: <reason>"' % (st, st))
    return ""


# --- helpers -------------------------------------------------------------------------------

def _git(repo, *args):
    res = subprocess.run(["git", "-C", repo] + list(args), capture_output=True, text=True)
    return res.stdout.strip() if res.returncode == 0 else ""


def target_dir(repo):
    """The cargo `target/` of `repo`: `CARGO_TARGET_DIR`, else a linked
    worktree's main tree `target/`, else `<repo>/target`. The rule is
    `plans/export-fixtures/make_export.py::gate_dir_for`'s (and wave_retro's,
    which read the receipts under `<target>/gate/<tree>/`) -- keep them one (B-I3)."""
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        return env if os.path.isabs(env) else os.path.join(repo, env)
    common = _git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir")
    if common and os.path.realpath(common) != os.path.realpath(os.path.join(repo, ".git")):
        return os.path.join(os.path.dirname(common.rstrip("/")), "target")
    return os.path.join(repo, "target")


def holder_key(repo, strand, wave, holder=None):
    """The lease holder `<wave>/<name>[:<holder>]` the way `strand.sh token` forms it."""
    key = _holder_key(repo, strand, wave)
    return "%s:%s" % (key, holder) if key and holder else key


def _holder_key(repo, strand, wave):
    """The one chain `--strand` > branch (`gitenv.strand_of`, GH #1199 f)."""
    return strand_of(repo, strand, wave).key


def strand_of(repo, strand="", wave=""):
    """`gitenv.strand_of` for the tree `repo`: its branch read once here."""
    return gitenv.strand_of(_git(repo, "rev-parse", "--abbrev-ref", "HEAD"), strand, wave)


def _pre_applies(repo):
    """Does the pre stamp apply here? A strand worktree: a branch with a `/`
    in a linked worktree -- the main tree's passes never need a stamp (W4,
    `strand.sh` `pre_applies`)."""
    branch = _git(repo, "symbolic-ref", "--short", "-q", "HEAD")
    if not branch or "/" not in branch:
        return False
    gd = _git(repo, "rev-parse", "--git-dir")
    common = _git(repo, "rev-parse", "--git-common-dir")
    if not gd or not common:
        return False
    return (os.path.realpath(os.path.join(repo, gd))
            != os.path.realpath(os.path.join(repo, common)))


def _pre_stamp(o, repo, err):
    """THE PRE STAMP (OR-DT-23, W4; moved here with `gate` from `strand.sh`):
    a strand gate from a strand worktree runs only on a tree `strand.sh pre`
    graded GREEN in the last 24 h. Other modes and the main tree check no
    stamp; `--lane` is no way past it (Delta B M-neu2 -- a lane's tree is a
    detached checkout, `_pre_applies` is false there anyway). `--no-pre --reason` gets past it; the
    executor writes the decision into the receipt (`owner_decision` station
    `pre`, decision `pre_skipped`). Returns the exit of a refusal, or 0."""
    if o.no_pre:
        print("gate: NOTE pre-check skipped: %s" % o.reason, file=err)
        return 0
    if o.mode != "strand" or not _pre_applies(repo):
        return 0
    pre = _pre_module()
    if pre is None:
        print("gate: NOTE the pre-check (scripts/pipeline/pre.py, W4) is not here -- "
              "running without it", file=err)
        return 0
    ok, line = pre.stamp_status(repo)
    if line:
        print(line, file=err)
    return 0 if ok else 2


def _pre_module():
    try:
        return importlib.import_module(PRE_MODULE)
    except ImportError as exc:
        if getattr(exc, "name", "") == PRE_MODULE:
            return None
        raise


def _needs_cargo(st):
    """Does station `st` need the cargo lock -- the admission's rule (`schedule._needs`),
    so plan-only, the local pass and the lane print one `cargo=` (Z17)."""
    return schedule._needs(st)["cargo"] >= 1


def _cmd_line(cmd):
    argv = cmd.get("argv") if isinstance(cmd, dict) else cmd
    return shlex.join(list(argv or []))


def _gate_line(data, scopes):
    name = data.get("station", "")
    reason = data.get("reason") or ""
    return "GATE %s [%s] %ss %s%s" % (name, scopes.get(name, ""), int(data.get("secs") or 0),
                                      data.get("verdict", ""), (" " + reason) if reason else "")


def _now_iso():
    return store.now()


def _deadline(stations):
    mins = sum(int(s.get("timeout_min") or 0) for s in stations)
    t = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=mins * 60 + GRACE_S)
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def keyed_fingerprint(fp, build):
    """The fingerprint the cache keys of a pass are made with: the register's
    (`fp`) under `build`, the build part measured in the stations' environment
    (`executor.stations_build`, the one producer -- re-check tv-bn M7r A-M2). A
    register written before a change of RUSTFLAGS, a CARGO_PROFILE_* or the
    rustc keys the new environment, never the old one: no CACHED across it."""
    return dict(fp or {}, **build)


def _lookup(db, doc, repo, metadata, fp, lane_name, mode, use_cache, build):
    """`(keys, hits, fingerprint)`: the cache keys of the plan, their hits and
    the fingerprint they are made with (`keyed_fingerprint` of `fp` under
    `build`, `executor.stations_build` of the pass's stations -- no default, no
    second environment: orchestrator OR-T12-18 A-M3); no metadata -> no keys.
    `use_cache` off: the same keys (the run still records under them), no hits."""
    fp = keyed_fingerprint(fp, build)
    if metadata is None:
        return {}, [], fp
    keys = cache.keys_for_plan(doc, repo, fp, lane_name, metadata=metadata)
    if not keys:
        return {}, [], fp
    if use_cache == "off":
        return keys, [], fp
    return keys, cache.lookup(db, keys, doc.get("repo") or "meclaw", mode, lane_name, _now_iso()), fp


def cache_off_defects(v2):
    """The stations a `--cache off` pass reports CACHED -- each one a defect: the
    switch promised that every planned station ran (messvertrag section 6)."""
    return [s.get("name") for s in (v2 or {}).get("stations") or []
            if s.get("verdict") == "CACHED"]


def _plan_line(doc, known, keys, cargo, lane_name, reason_path, use_cache="on"):
    """V0 section 5: `stations=` the stations that run, `cached=` those of them
    known whole or with every part key known (B-M2); `reason=` the archived plan
    (B-M1). `cargo=`/`lane=`/`crit=` follow as W3's additions."""
    hits = {(k.get("station"), k.get("part") or "") for k in known}
    runs = [s.get("name") for s in doc.get("stations") or [] if s.get("run") is not False]
    cached = 0
    for name in runs:
        own = [k for k in keys or {} if k[0] == name]
        if (name, "") in hits or (own and all(k in hits for k in own)):
            cached += 1
    line = "GATE-PLAN %s %s stations=%d cached=%d reason=%s cargo=%d lane=%s crit=%ds" % (
        doc.get("mode"), (doc.get("rev") or "")[:7], len(runs), cached, reason_path or "-",
        1 if cargo else 0, lane_name, int(doc.get("critical_path_s") or 0))
    scope = doc.get("narrowing")
    if scope:
        line += " scope=%s since=%s" % (scope.get("tests") or "-", (scope.get("since") or "-")[:9])
    # GH #1141: the runs a merge offers (its strand runs) -- how many of the
    # cache hits they answered, of all hits.
    offer = {r for r in os.environ.get("MECLAW_GATE_CACHE_FROM", "").split(",") if r}
    if offer:
        line += " offer=%d/%d" % (sum(1 for k in known if k.get("from_run") in offer), len(known))
    if use_cache == "off":
        line += " cache=off"
    return line


def _header(doc, repo, mode, out):
    base = doc.get("base") or ""
    try:
        n = len(plan.changed_files(repo, mode, base or None))
    except Exception:  # noqa: BLE001 -- the header is a courtesy, the plan stands
        n = 0
    short = _git(repo, "rev-parse", "--short", base) if base else ""
    why = None
    if not base:   # GH #1199 (I3 class): no merge base is no whole-tree pass -- say so
        try:
            why = plan._resolve_base(repo, "strand" if mode == "merge" else mode, None)[2]
        except Exception:  # noqa: BLE001 -- the header is a courtesy, the plan stands
            why = None
    label = short or base or ("none (%s)" % why if why else "full tree")
    print("gate: base = %s (%d %s)" % (label, n, "file" if n == 1 else "files"), file=out)


def _scope_line(n):
    """`gate: SCOPE ...`: what the tests of a merge/integration are, with the
    base they were narrowed from (re-check H2 M1)."""
    return "gate: SCOPE tests=%s stage=%s since=%s wave=%s map=%s why=%s" % (
        n.get("tests") or "-", n.get("stage") or "-", (n.get("since") or "-")[:9],
        n.get("wave") or "-", (n.get("map") or "-")[:9], n.get("why") or "-")


def _plan_only(doc, out):
    has_tests = False
    for st in doc.get("stations") or []:
        has_tests = has_tests or st.get("name") == "tests"
        for cmd in st.get("cmds") or []:
            cwd = cmd.get("cwd", "") if isinstance(cmd, dict) else ""
            # Column 3 is the raw row flag for the legacy kit (MECLAW_LANE_CARGO),
            # a superset of the cargo predicate -- fail-closed; goes in Z-I (R2 M4).
            print("%s\t%s\t%d\t%s\t%s" % (st.get("name"), st.get("scope", ""), 1 if st.get("cargo") else 0,
                                          _cmd_line(cmd), cwd or ""), file=out)
    print("tests=%s" % ("true" if has_tests else "false"), file=out)


def _plan_fingerprint(db, host, repo=None):
    """`(fingerprint, runner)` `--plan-only` keys with: `--host`'s runner, else the
    runner updated last, else the local one -- the register from the store as a
    run of `repo` expects it (`schedule.register_view`, #1199 K13); the build part
    is measured like the run's (`_plan_build`)."""
    try:
        schedule._ensure(db)
        if host:
            row = db.execute("SELECT fingerprint, name FROM runners WHERE name = ?", (host,)).fetchone()
        else:
            row = db.execute("SELECT fingerprint, name FROM runners WHERE fingerprint IS NOT NULL "
                             "ORDER BY updated_at DESC LIMIT 1").fetchone()
        if row and row[0]:
            return schedule.register_view(json.loads(row[0]), repo), row[1] or host or ""
    except (ValueError, OSError, Exception):  # noqa: BLE001 -- a missing store keys nothing
        pass
    return _load_json(_local_fp_path()) or {}, "local"


def _held_lane(db, repo, o):
    """The lane of the lane lease the pass's holder holds, else '' (an unarmed or
    unreadable store holds none). A batch holder `<wave>/S1+S2` speaks for the
    member that holds the lane, as its admission does (`schedule._batch_speaker`;
    re-check v2-r3-lease n1)."""
    key = holder_key(repo, o.strand, o.wave, o.holder)
    if not key:
        return ""
    try:
        st = schedule._load(db)
        mine = schedule._holder(st, schedule._batch_speaker(st, key, o.host)) if st else None
    except Exception:  # noqa: BLE001 -- a store that cannot say holds no lease
        return ""
    return (mine or {}).get("lane") or ""


def _runs_here(o, repo, db, cargo):
    """Whether a (non-satellite) pass runs on this machine (`_run_local`): `--lane`,
    or a plan without a cargo station, without `--host` and without a held lane
    lease. I3 (ruling test-cc19, 16.10.): cargo-free kit plans (gate-selftest,
    export-tree-tests --kit) with `--host build03` ran here beside the live Egon
    (~120 % load, a load-flake) -- with `--host` or a held lane lease they run on
    the lane (`_run_lane`, a lease as for a cargo plan)."""
    if o.lane:
        return True
    return not cargo and not o.host and not _held_lane(db, repo, o)


def _plan_build(o, repo, satellite, here, fp, lane_name, cargo=False):
    """The build part `--plan-only` keys with -- the run's own (orchestrator OR-T12-18
    A-M3, re-check tv-bn M7w A-M3: the plan looked up in a third environment, this
    process's without the lane's words and the mode): the `+env` the run would hand
    its stations and the same `executor.stations_build`. A pass that would run here
    (`_run_local`) takes the run's own `+env` (`_local_plus_env`, one source -- re-check
    bundle N kit-fp M3, N-K3); a lane pass the words of the runner's lane
    (`lane.env_for`, as `schedule` dispatches) with the register's variables and the
    mode, measured for the tree the admission names on the lane (`_lane_tree`). A
    lane that cannot be named: no build part, no key."""
    from . import executor  # the stations' build part is the executor's rule
    if not satellite and here:
        return executor.stations_build(None, _local_plus_env(o)[3], repo)
    try:
        name = o.host or lane_name
        ln = lane.find_lane(name) if name and name != "local" else None
        passed = _lane_env(repo, satellite, os.environ)
        words = lane.env_for(ln, o.mode, nproc=(fp or {}).get("vcpu")) if ln else None
    except (lane.LaneError, satellites.RegisterError):
        ln = words = None
    if ln is None or words is None:
        return executor._build_part(None)
    return executor.stations_build(ln, dict(passed, **dict(words, MECLAW_GATE_MODE=o.mode)), repo,
                                   _lane_tree(ln, cargo, repo, o))


def _lane_tree(ln, cargo, repo, o):
    """The tree a pass of `repo` runs in on lane `ln`: the admission's own rule
    (`schedule.admission_tree` under `schedule.tree_under`, one source -- re-check
    bundle N delta kit-fp M1) with the words `_run_lane` hands `schedule.admit`.
    Only the `--plan-only` measure takes it: the run measures with the dispatch's
    `tree` itself."""
    holder = holder_key(repo, o.strand, o.wave, o.holder) or o.wave or ""
    return schedule.tree_under(ln.root, schedule.admission_tree(
        cargo, os.path.basename(repo.rstrip("/")), holder))


# --- the run -------------------------------------------------------------------------------

# The owner ruling(s) a pass runs under for the change reservation of the
# testing bans (`scripts/testing_bans.py` RULING_ENV): the receipt names it
# (P1c § 4 "Wert im Receipt", R-TV-W21 c).
TESTING_RULING_ENV = "MECLAW_TESTING_RULES_RULING"


def testing_ruling(environ=None):
    """The pass's `MECLAW_TESTING_RULES_RULING`, stripped; None when unset or blank."""
    value = (os.environ if environ is None else environ).get(TESTING_RULING_ENV) or ""
    return value.strip() or None


class _Pass:
    """One pass from dispatch to receipt: the stream, the receipt, the archive."""

    def __init__(self, o, repo, doc, out, err):
        self.o, self.repo, self.doc, self.out, self.err = o, repo, doc, out, err
        self.scopes = {s["name"]: s.get("scope", "") for s in doc.get("stations") or []}
        self.events, self.pass_ended, self.fingerprint = [], None, None
        self.store_lost = []  # (what, why, busy) of every record the store refused for good
        self._stream_lost = False
        self.vcpu = 0  # the lane's threads for GATE-LANE (GH #1162); 0 = unknown/local
        # The archive's name is fixed at the plan: GATE-PLAN's `reason=` names it.
        ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        self.rid = "%s-%s" % (ts, (doc.get("rev") or "unknown")[:8])

    def plan_path(self):
        if not self.o.archive:
            return None
        self._claim()
        return os.path.join(self.o.archive, self.rid, "plan.json")

    def _claim(self):
        """Z10: the archive directory is taken when its name is printed
        (`reason=`), with the `-N` of a collision already applied -- the name
        in GATE-PLAN and the directory of the archive are one."""
        if getattr(self, "_claimed", False):
            return
        root, base = self.o.archive, self.rid
        cand, n = base, 1
        while True:
            try:
                os.makedirs(os.path.join(root, cand))
                break
            except FileExistsError:
                n += 1
                cand = "%s-%d" % (base, n)
            except OSError:
                return  # unwritable root: archive() reports it
        self.rid, self._claimed = cand, True

    def store(self, what, fn, *args, once=False, **kwargs):
        """`fn` against the store with the bounded retry (`store.retry_write`).
        A record the store refuses for good is one `gate: store: LOST` line
        and ends the pass ABORTED (welle-tb S3, #1161) -- never a traceback,
        never RED, never a silent hole in the audit.

        The LOST line names the exception's class (re-check tb-s3-store m-2):
        a KeyError or a validation error is no busy store, and a second run
        fails the same way. `once` is one try instead of the bounded retry
        (m-3: the stream after its first lost line)."""
        try:
            return store.retry_write(fn, *args, attempts=1 if once else None, **kwargs)
        except Exception as exc:  # noqa: BLE001 -- every refusal is named, the pass ends ABORTED
            why = "%s: %s" % (type(exc).__name__, exc)
            self.store_lost.append((what, why, isinstance(exc, store.StoreWriteError)))
            print("gate: store: LOST %s -- %s" % (what, why), file=self.err, flush=True)
            return None

    def lost_line(self):
        whats = ", ".join(dict.fromkeys(w for w, _, _ in self.store_lost))
        others = list(dict.fromkeys(why.split(":", 1)[0] for _, why, busy in self.store_lost
                                    if not busy))
        advice = ("run the gate again." if not others else
                  "%s is no busy store: fix the cause, a second run fails the same way."
                  % ", ".join(others))
        return ("gate: ABORTED -- the store lost %d record(s) (%s); the stations' results stand "
                "in the archive, this pass does not count -- %s"
                % (len(self.store_lost), whats, advice))

    def note_lost(self, db, run):
        """One `process_finding store_record_lost` for a pass that lost a
        record (re-check tb-s3-store M-2, #1161): its stream's `pass_ended`
        (probe P4: GREEN in the journal and in `pipeline receipt`) is not its
        verdict, and `receipt.project` and `retro.passes` read the finding as
        ABORTED. Best effort with its own retry -- never through `store()`.
        A lost finding (the store still locked, re-check tb-s3-store2 m-n3)
        falls back to one line in the ledger (`events.STORE_LOST_LEDGER`):
        the archive root of the pass, else (none, or unwritable) beside the
        store; every reader of a pass verdict reads both."""
        if db is None or not run or not self.store_lost:
            return
        data = {"rule": events.STORE_LOST_RULE, "subject": run, "value": len(self.store_lost),
                "threshold": 0, "since": store.now(),
                "records": list(dict.fromkeys(w for w, _, _ in self.store_lost))}
        try:
            store.retry_write(events.emit, db, "process_finding", "finding", data, run=run)
        except Exception as exc:  # noqa: BLE001 -- the gate's own lines already say ABORTED
            print("gate: store: LOST finding %s -- %s: %s"
                  % (events.STORE_LOST_RULE, type(exc).__name__, exc), file=self.err, flush=True)
            why = "no archive and no store file"
            for path in events.lost_ledgers([self.o.archive], db):  # the archive first
                try:
                    events.write_ledger(path, dict(data, run=run))
                except OSError as exc2:
                    why = "%s: %s" % (type(exc2).__name__, exc2)
                    continue
                print("gate: store: ledger %s -- %s" % (events.STORE_LOST_RULE, path),
                      file=self.err, flush=True)
                return
            print("gate: store: LOST ledger -- %s" % why, file=self.err, flush=True)

    def stream(self, proc, ingest=None):
        for line in proc.stdout:
            text = line.strip()
            if not text:
                continue
            try:
                ev = json.loads(text)
            except ValueError:
                print(text, file=self.err)
                continue
            if ingest is not None:  # a refusal never stops the stream: the executor drains
                # m-3: after the first lost line one try each -- 3 x busy_timeout
                # per line would back up the executor's pipe of an ABORTED pass.
                lost = len(self.store_lost)
                self.store("stream line %s" % ev.get("event"), ingest, text,
                           once=self._stream_lost)
                self._stream_lost = self._stream_lost or len(self.store_lost) > lost
            self.events.append(ev)
            kind, data = ev.get("event"), ev.get("data") or {}
            if kind == "station_ended":
                print(_gate_line(data, self.scopes), file=self.out, flush=True)
            elif kind == "lane_fingerprint":
                self.fingerprint = data
            elif kind == "pass_ended":
                self.pass_ended = ev
        proc.stdout.close()
        return proc.wait()

    def v2(self, dest, head_rows):
        doc = _load_json(os.path.join(dest, "receipt.json"))
        if isinstance(doc, dict) and doc.get("stations") is not None and doc.get("verdict"):
            return doc
        return receipt.project(head_rows + self.events)

    def finish(self, v2, dest, lane_name, rounds, db=None, run=None):
        pe = (self.pass_ended or {}).get("data") or {}
        verdict = pe.get("verdict") or v2.get("verdict") or "ABORTED"
        if self.store_lost:  # fail-closed (#1161): a pass with a hole in the store does not count
            verdict = "ABORTED"
            self.note_lost(db, run)
        if v2.get("verdict") != verdict:  # the stream is the record: a scope stop after the file
            v2 = dict(v2, verdict=verdict)
        if self.doc.get("full_pass"):  # the lane's receipt.json never saw the plan (#1165)
            v2 = dict(v2, full_pass=self.doc["full_pass"])
        if self.doc.get("graph_build"):  # nor the graph the narrowing built (3a, #1199)
            v2 = dict(v2, graph_build_secs=self.doc["graph_build"].get("secs"))
        if testing_ruling():  # nor the core's environment (R-TV-W21 c)
            v2 = dict(v2, testing_ruling=testing_ruling())
        defects = cache_off_defects(v2) if self.o.cache == "off" else []
        if defects:
            print("gate: DEFECT -- cache=off, yet CACHED: %s (the pass is no measurement)"
                  % " ".join(defects), file=self.err)
            verdict = "RED"
            v2 = dict(v2, verdict=verdict)
        summary = receipt.gate_lines(v2)[-1]
        lane_line = "GATE-LANE %s crit=%d lane_s=%d rounds=%d" % (
            lane_name, int(pe.get("critical_path_s") or 0), int(pe.get("lane_seconds") or 0), rounds)
        if self.vcpu:
            lane_line += " vcpu=%d" % self.vcpu
        if self.o.cache == "off":
            lane_line += " cache=off"
        self.write_legacy(v2)
        if self.o.archive:
            self.archive(v2, dest, receipt.gate_lines(v2) + [lane_line])
        # The full rev (W9, GH #1123): the merge queue accepts a verdict only
        # for its candidate, and the summary line carries seven characters.
        print("gate: rev = %s" % (v2.get("rev") or self.doc.get("rev") or "none"), file=self.out)
        print(summary, file=self.out)
        print(lane_line, file=self.out)
        if self.store_lost:
            print(self.lost_line(), file=self.out)
        if verdict == "RED":
            print("gate: RED -- fix everything listed, then run the gate again as a whole.", file=self.out)
        elif verdict == "ASK":  # the owner's way forward (#1149)
            from . import questions
            asking = [s.get("name") for s in v2.get("stations") or [] if s.get("verdict") == "ASK"]
            print(questions.block(asking, self.o.mode).rstrip("\n"), file=self.out)
        return EXIT_BY_VERDICT.get(verdict, 2)

    def write_legacy(self, v2):
        gate_dir = os.path.join(target_dir(self.repo), "gate", os.path.basename(self.repo.rstrip("/")))
        from .executor import legacy_v1  # the lane writes the same v1 file
        doc = legacy_v1(v2)
        try:
            _write_json(os.path.join(gate_dir, "%s-%s.json" % (self.o.mode, v2.get("rev") or "unknown")), doc)
            _write_json(os.path.join(gate_dir, "last-%s.json" % self.o.mode), doc)
        except OSError as exc:
            print("gate: could not write the local receipt to %s (%s)" % (gate_dir, exc), file=self.err)

    def archive(self, v2, dest, lines):
        """`<root>/<ts>-<sha8>/{receipt.json,summary.txt,events.jsonl,plan.json,logs/}` + `latest`."""
        root = self.o.archive
        rid = self.rid
        try:
            os.makedirs(root, exist_ok=True)
            self._claim()
            rid = self.rid
            adir = os.path.join(root, rid)
            os.makedirs(os.path.join(adir, "logs"), exist_ok=True)
            _write_json(os.path.join(adir, "receipt.json"), v2)
            _write_json(os.path.join(adir, "plan.json"), self.doc)
            with open(os.path.join(adir, "summary.txt"), "w", encoding="utf-8") as fh:
                # The plan's NOTE lines (the narrowing's say, review Z-I1) close
                # it -- the first line stays GATE-SUMMARY (backfill reads it).
                notes = ["gate: NOTE %s" % n for n in self.doc.get("notes") or []]
                if self.doc.get("narrowing"):
                    notes.insert(0, _scope_line(self.doc["narrowing"]))
                fh.write("\n".join(list(lines) + notes) + "\n")
            with open(os.path.join(adir, "events.jsonl"), "w", encoding="utf-8") as fh:
                for ev in self.events:
                    fh.write(json.dumps(ev, sort_keys=True) + "\n")
            logs = os.path.join(dest, "logs")
            if os.path.isdir(logs):
                for name in sorted(os.listdir(logs)):
                    src = os.path.join(logs, name)
                    if os.path.isfile(src):
                        shutil.copy2(src, os.path.join(adir, "logs", name))
            latest = os.path.join(root, "latest")
            try:
                if os.path.lexists(latest):
                    os.remove(latest)
                os.symlink(rid, latest)
            except OSError:
                with open(os.path.join(root, "latest.txt"), "w", encoding="utf-8") as fh:
                    fh.write(rid + "\n")
        except OSError as exc:
            print("gate: could not copy receipt/logs to %s (the run itself stands): %s" % (root, exc),
                  file=self.err)

    def head(self, run, lane_name, rnd, dispatch=None):
        """`pass_planned` and `run_dispatched` of the pass: W0a's receipt projects
        mode/rev/plan/lane/round from them when the lane left no receipt."""
        at = _now_iso()
        src = dispatch or self.doc
        planned = {"run": run, "mode": self.o.mode, "repo": self.doc.get("repo") or "meclaw",
                   "rev": self.doc.get("rev"), "base": self.doc.get("base"),
                   "stations": [s["name"] for s in self.doc.get("stations") or []],
                   "reason": self.doc.get("plan_reason") or {},
                   "cache_known": len({ck.get("station")
                                       for ck in self.doc.get("cache_known") or []}),
                   "dirty": bool(self.doc.get("dirty")), "archive": self.o.archive}
        if self.doc.get("narrowing"):
            planned["narrowing"] = self.doc["narrowing"]
        if self.doc.get("full_pass"):  # additive: the receipt's `full_pass` (#1165)
            planned["full_pass"] = self.doc["full_pass"]
        if self.doc.get("graph_build"):  # the narrowing built the graph (3a, #1199)
            planned["graph_build_secs"] = self.doc["graph_build"].get("secs")
        if testing_ruling():  # the receipt's `testing_ruling` (R-TV-W21 c)
            planned["testing_ruling"] = testing_ruling()
        if self.o.cache == "off":
            planned["cache"] = "off"
        dispatched = {"run": run, "runner": lane_name, "tree": src.get("tree"),
                      "attempt": 1, "round": rnd, "deadline_at": src.get("deadline_at"),
                      "ws": src.get("ws")}
        return [{"at": at, "run": run, "kind": "status", "event": "pass_planned",
                 "data": planned},
                {"at": at, "run": run, "kind": "note", "event": "run_dispatched",
                 "data": dispatched}]


def _journal(db, p, rows):
    """The core's head rows into the store before the dispatch (V0 section 3):
    `pipeline receipt <run>` rebuilds mode/rev/plan from the journal alone."""
    for row in rows:
        p.store("journal %s" % row["event"], events.emit, db, row["event"], row["kind"],
                row["data"], run=row["run"], at=row["at"])


def _aborter(ln, tree, run, ws, err):
    """The engine's abort of `run` (`lane.abort`): the executor ends the stream
    with `pass_ended ABORTED`; a refusal is one line, the stream decides."""
    def abort(why):
        try:
            rc = lane.abort(ln, tree, run, ws)
        except lane.LaneError as exc:
            print("gate: abort of %s (%s): %s" % (run, why, exc), file=err, flush=True)
            return
        if rc != 0:
            print("gate: abort of %s (%s): the executor did not take it (exit %s)" % (run, why, rc),
                  file=err, flush=True)
    return abort


def _stream_to_end(p, proc, ingest, abort, deadline_at):
    """The stream, read to its end (blocking, no polling). At `deadline_at` a
    one-shot timer asks the executor to abort; Ctrl-C does the same and reads
    on -- a second Ctrl-C leaves, the caller then closes the run lost (GR-iii)."""
    timer = None
    try:
        at = datetime.datetime.strptime(deadline_at or "", "%Y-%m-%dT%H:%M:%SZ")
        left = (at.replace(tzinfo=datetime.timezone.utc)
                - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
        timer = threading.Timer(max(0.0, left), abort, args=("deadline",))
        timer.daemon = True
        timer.start()
    except ValueError:
        pass
    try:
        try:
            return p.stream(proc, ingest=ingest)
        except KeyboardInterrupt:
            print("gate: interrupted -- the executor is asked to abort the run; Ctrl-C again "
                  "leaves at once (the run is then lost)", file=p.err, flush=True)
            abort("interrupt")
            return p.stream(proc, ingest=ingest)
    finally:
        if timer is not None:
            timer.cancel()


def _aborted(events_, dest):
    """The executor demonstrably aborted: a fetched receipt or event says
    `pass_ended ABORTED`, or a received station ended `aborted_*`."""
    doc = _load_json(os.path.join(dest, "receipt.json"))
    if isinstance(doc, dict) and doc.get("verdict") == "ABORTED":
        return True
    for ev in events_:
        data = ev.get("data") or {}
        if ev.get("event") == "pass_ended" and data.get("verdict") == "ABORTED":
            return True
        if ev.get("event") == "station_ended" and str(data.get("reason") or "").startswith("aborted_"):
            return True
    return False


def _fetched_events(dest):
    rows = []
    try:
        with open(os.path.join(dest, "events.jsonl"), encoding="utf-8") as fh:
            for line in fh:
                try:
                    rows.append(json.loads(line))
                except ValueError:
                    continue
    except OSError:
        pass
    return rows


def _lane_colonies(adm):
    """The colonies the admitted lane offers (`schedule.colony_capacity`: its `cores=`);
    a capacity without them leaves `COLONY_REST` unresolved, which the executor refuses."""
    col = (adm.capacity or {}).get("colony")
    return int(col) if col else None


def _only(only, stations):
    """`--only` as the plan reads it (Delta B M-neu1): a named station that
    waits for `build` takes `build` along -- the executor gets the same set.
    The same for phase 1 `build:browser` (GH #1162 Weg d): a named station takes
    the build station it waits for along, as `plan.build_plan` does."""
    if not only:
        return only
    wanted = list(only)
    builds = {s.get("name") for s in stations or [] if s.get("build") is not None} | {"build"}
    for s in stations or []:
        if s.get("name") in only:
            for a in s.get("after") or []:
                if a in builds and a not in wanted:
                    wanted.append(a)
    return wanted


def _flags(o, lock, stations=()):
    return {"+lock_file": lock, "+nice": not o.no_nice, "+fail_fast": o.fail_fast,
            "+no_precheck_stop": o.no_precheck_stop, "+only": _only(o.only, stations),
            "+resync": o.resync,
            "decide": list(o.decide), "archive": o.archive,
            "no_pre": {"reason": o.reason} if o.no_pre else None}


def _runner_class(db, name):
    """The runner's class in the store (`runners.class`), or None."""
    row = db.execute("SELECT class FROM runners WHERE name = ?", (name,)).fetchone()
    return row["class"] if row is not None else None


def _flake_quarantine(db):
    """The dispatch's `quarantine` (V0 section 6): expired entries become a
    finding first, then the valid ones travel with the run (W8)."""
    at = store.now()
    flakes.release_expired(db, at)
    return flakes.dispatch_quarantine(db, at)


def _flake_expired(db):
    """The dispatch's `quarantine_expired` (VG TV-F3): ran out without a measured fix."""
    return flakes.dispatch_expired(db, store.now())


def _flake_dispatch(p, db, d, repo, mode, base):
    """The flake half of a dispatch (OR-TV-2): the quarantine with its NOTE
    station `quarantine` beside `tests`, the register's issues, the expired
    entries (RED in that station) and the legacy tests that may count green
    in this diff (VG TV-F4: the strand's merge base; integration and release
    the published base `plan.resolve_base` settles; none without a base)."""
    d["quarantine"] = p.store("flake quarantine", _flake_quarantine, db) or []
    d["flake_issues"] = p.store("flake issues", flakes.dispatch_issues, db) or {}
    d["quarantine_expired"] = p.store("flake expiries", _flake_expired, db) or []
    # B schlank (OR-TV-13, K12): the triage's T2 runs on the lane about 30 % busy.
    d["t2_production"] = True
    try:
        legacy_base, full = plan.resolve_base(repo, mode, base or None)
        d["flaky_legacy"] = [] if full else flakes.legacy_tests(repo, legacy_base)
    except Exception:  # noqa: BLE001 -- no legacy list readable: every FLAKY counts red
        d["flaky_legacy"] = []
    stations = list(d.get("stations") or [])
    names = {st.get("name") for st in stations}
    tests = next((st for st in stations if st.get("name") == "tests"), None)
    if (tests is not None and flakes.QUARANTINE_STATION not in names
            and (d["quarantine"] or d["quarantine_expired"])):
        stations.append(flakes.quarantine_station(tests, d["quarantine"],
                                                  set(tests.get("parts") or ()) or None))
        d["stations"] = stations


def _flake_ingest(db, run, station_items):
    """The run's `flake_isolated` lines into the register: isolation tasks,
    the window, the auto quarantine, and the issue duty -- with `gh` unless
    `MECLAW_FLAKE_ISSUES=0` (W8, OR-DT.W8.K.18)."""
    gh = None if os.environ.get(flakes.ISSUES_ENV) == "0" else flakes.GH_RUNNER
    return flakes.ingest_run(db, run, station_items, gh=gh)


def _cache_doubt(keys, fp_before, fp_after):
    """The cache doubt of a pass for `full_pass` (#1165): the run's tools are not
    the ones the keys were made with (`cache._tools`) -- `None` without one.
    Pure, so the full-pass mark runs before the cache record (the last record,
    re-check tb-s3-store M-3)."""
    if not keys or not isinstance(fp_after, dict):
        return None
    if fp_after.get("unanswered"):   # re-check tv-bn M7r A-M1: null == null is no proof
        return "no fingerprint after the run (%s)" % fp_after["unanswered"]
    specs = set()
    for st in {k[0] for k in keys}:
        specs.update(s for k, v in keys.items() if k[0] == st for s in v.get("inputs") or [])
    before, after = cache._tools(fp_before or {}, specs), cache._tools(fp_after, specs)
    if before != after:
        return "keys of other tools (%s)" % ", ".join(k for k in before if before[k] != after[k])
    unknown = sorted(k for k, v in after.items() if v is None)
    if unknown:
        return "no version of %s after the run" % ", ".join(unknown)
    # M7r A-M2: the build environment the keys were made with is the stations' own
    if cache._semantic_env((fp_before or {}).get("build_env")) != \
            cache._semantic_env(fp_after.get("build_env")):
        return "keys of another build environment"
    return None


def _record(db, run, runner, keys, fp_before, fp_after, events_):
    """`cache.record` for the GREEN stations -- only when the run's tools are
    the ones the keys were made with (`_cache_doubt`); returns that doubt."""
    if not keys or not isinstance(fp_after, dict):
        return
    doubt = _cache_doubt(keys, fp_before, fp_after)
    if doubt:  # nothing recorded -- and a cache doubt for `full_pass` (#1165)
        return doubt
    # OR-TB.S7.10: the browser duty in a key is the plan-time probe; a probe that
    # changed during the run stores no entry of a booking station.
    if (fp_before or {}).get("browsers") != fp_after.get("browsers"):
        keys = {k: v for k, v in keys.items() if v.get("browsers") is None}
    results = []
    for ev in events_:
        data = ev.get("data") or {}
        if ev.get("event") == "station_ended" and data.get("verdict") == "GREEN":
            # A FLAKY station is a finding (OR-TV-2): never a cache entry.
            results.append({"station": data.get("station"), "part": "", "verdict": data["verdict"]})
        elif ev.get("event") == "test_result" and data.get("verdict") == "GREEN":
            results.append({"station": data.get("station"), "part": data.get("part") or "",
                            "verdict": data["verdict"]})
    if results:
        cache.record(db, run, runner, keys, results)


def _full_wave(o, repo):
    """The wave of this pass for the full-pass mark (`full_pass.wave_of`)."""
    return full_pass.wave_of(holder_key(repo, o.strand, o.wave),
                             os.environ.get("MECLAW_GATE_WAVE", ""), o.wave)


def _full_strand(o, repo):
    """The strand of a strand or merge pass for the full-pass mark
    (`full_pass.strand_of`, re-check S1b m-4); None for an integration."""
    if full_pass.scope_of(o.mode) != full_pass.STRAND:
        return None
    return full_pass.strand_of(holder_key(repo, o.strand, o.wave))


def _full_pass_after(o, repo, doc, db, run, p, v2, doubt, out):
    """Mark the wave after a narrowed pass with odd effects, clear it after a
    GREEN full pass (`full_pass`, #1165); a satellite has no mark."""
    if satellites.is_satellite(repo):
        return
    verdict = ((p.pass_ended or {}).get("data") or {}).get("verdict") or v2.get("verdict")
    if p.store_lost:  # the pass ends ABORTED (#1161): the mark stands, never cleared
        verdict = "ABORTED"
    elif o.cache == "off" and cache_off_defects(v2):  # m-RS-1: `finish` books it RED
        verdict = "RED"
    for note in full_pass.after_pass(db, _full_wave(o, repo), doc, v2.get("stations") or [],
                                     verdict, run, doubt, strand=_full_strand(o, repo)):
        print("gate: NOTE %s" % note, file=out)


def _guard_expected_g(adm, env):
    """What W2's hygiene clears beyond the lane's floor (Delta A M-neu-1):
    the admission's need (reserve + the live runs' reservation + expected)
    less `MECLAW_LANE_FLOOR_G` (lane_sync.sh's default 60) -- `expected_g`
    alone stopped short beside a neighbour and the executor refused after
    the push. Never less than the run's own size."""
    own = int(adm.expected_g or 0)
    need = ((adm.dispatch or {}).get("budget") or {}).get("need_free_g")
    if need is None:
        return own
    raw = env.get("MECLAW_LANE_FLOOR_G") or "60"
    floor = int(raw) if raw.isdigit() else 60
    return max(own, int(float(need)) - floor)


def _local_budget(stations):
    """The disk floor of a pass on this machine (Delta A I-neu-1): a cargo
    station keeps legacy's floor -- `MECLAW_GATE_MIN_FREE_G`, default 60 G --
    at the start because a live colony shares this disk; the watcher during
    the run holds `MECLAW_GATE_WATCH_FREE_G` (default the lanes' 30 G, never
    above the entry floor); a plan without cargo needs none."""
    if not any(_needs_cargo(s) for s in stations or []):
        return {}
    floor = _env_g("MECLAW_GATE_MIN_FREE_G", 60.0)
    # Z3: 60 G is the entry check only. The watcher during the run holds the
    # lanes' running floor (schedule.DEFAULT_BUDGET free_min_g) -- holding 60 G
    # all the way turned a cold build started with 61-75 G free into ABORTED.
    # Review Zb M2: no value switches the watcher off -- at least WATCH_MIN_G.
    watch = _env_g("MECLAW_GATE_WATCH_FREE_G", float(schedule.DEFAULT_BUDGET["free_min_g"]))
    watch = min(max(watch, WATCH_MIN_G), floor)
    return {"free_min_g": watch, "reserve_g": floor, "need_free_g": floor}


WATCH_MIN_G = 10.0


def _floor_notes(stations):
    """The NOTE lines of the entry floor (GH #1154 re-check m-2): a value that
    `_env_g` drops says so, and 0 -- the explicit off switch -- says the guard
    is off. A plan without cargo has no floor and says nothing."""
    if not any(_needs_cargo(s) for s in stations or []):
        return []
    raw = os.environ.get("MECLAW_GATE_MIN_FREE_G") or ""
    if not raw:
        return []
    floor = _env_g("MECLAW_GATE_MIN_FREE_G", 60.0)
    try:
        dropped = floor != float(raw)
    except ValueError:
        dropped = True
    if dropped:
        return ["MECLAW_GATE_MIN_FREE_G=%s ignored, floor %dG" % (raw, floor)]
    if floor == 0:
        return ["MECLAW_GATE_MIN_FREE_G=%s: disk guard off" % raw]
    return []


def _env_g(name, default):
    """A size in G from the environment; unset, unreadable, not finite or
    negative -> `default` (GH #1154: a negative floor switched the disk
    guard off without a word; 0 stays the explicit off switch)."""
    try:
        v = float(os.environ.get(name) or default)
    except ValueError:
        return float(default)
    return v if math.isfinite(v) and v >= 0 else float(default)


def _local_width(mode, nproc):
    """(cpu, +env, egon) of a run on this machine: a local lane of class egon
    in the lanes file says this machine runs Egon -- half the cores and the
    class's environment (`lane_sync.sh env`), like a lane run there (B-M5)."""
    try:
        here = [ln for ln in lane.load_lanes() if ln.transport == "local" and ln.cls == "egon"]
    except lane.LaneError:
        here = []
    if not here:
        return nproc, {}, False
    cpu = max(1, nproc // 2)
    try:
        env = lane.env_for(here[0], mode)
    except lane.LaneError:
        env = {"CARGO_BUILD_JOBS": str(cpu), "NEXTEST_TEST_THREADS": "4"}
    return cpu, env, True


def _local_plus_env(o):
    """`(nproc, cpu, egon, +env)` of a pass on this machine -- THE one source of the
    `+env` its stations start under, for `_run_local` and the `--plan-only` measure
    (`_plan_build`; re-check bundle N kit-fp M3, N-K3: the plan rebuilt it a second
    time): `_local_width`'s words and the mode."""
    nproc = cpus.threads()
    cpu, env, egon = _local_width(o.mode, nproc)
    return nproc, cpu, egon, dict(env, MECLAW_GATE_MODE=o.mode)


def _run_local(o, repo, doc, metadata, db, out, err):
    from . import executor  # the local fingerprint is the executor's own
    run = "run-" + uuid.uuid4().hex[:12]
    rdir = os.path.join(state_dir(), "runs", run)
    nproc, cpu, egon, plus_env = _local_plus_env(o)
    # ONE source for the colonies (welle-tb S5 `colony_capacity`): the real cores,
    # at least the widest fixed booking of the core's stations -- an egon-local
    # pass of builder 4 or memory beside browser 3 was RED before the start on
    # mm-os01 with S3's own halved count (re-check tb-s3 h1 M-1).
    colonies = schedule.colony_capacity(satellites.local_cores(), nproc)
    doc_r = plan.resolve_needs(doc, cpu, colonies)
    target = target_dir(repo)
    fp = _load_json(_local_fp_path())
    # the stations' environment, the executor's one rule (M7t A-M2): this process's
    # under the run's `+env` (`_local_plus_env`), exactly the dispatch's
    if not isinstance(fp, dict) or fp.get("unanswered"):
        fp = executor.lane_fingerprint(repo, target, executor.station_env(plus_env, os.environ))
    keys, known, fp = _lookup(db, doc_r, repo, metadata, fp, "local", o.mode, o.cache,
                              executor.stations_build(None, plus_env, repo))
    p = _Pass(o, repo, doc, out, err)
    cargo = any(_needs_cargo(s) for s in doc.get("stations") or [])
    print(_plan_line(doc, known, keys, cargo, "local", p.plan_path(), o.cache), file=out,
          flush=True)
    # TV-K5 (b) here too (fix round 4 v2-r3-kit): a cap is at least 3x the green P95,
    # here the slowest lane class's (`schedule.local_p95`); a cap below it is raised.
    p95 = p.store("station p95", schedule.local_p95, db, doc.get("repo") or "meclaw",
                  o.mode) or {}
    doc_r["stations"], raised = schedule.with_hang_caps(doc_r["stations"], p95)
    for line in schedule.hang_cap_notes(raised, schedule.clamped_caps(doc_r["stations"], p95)):
        print(line, file=out, flush=True)
    for note in _floor_notes(doc_r["stations"]):
        print("gate: NOTE %s" % note, file=out, flush=True)
    if not o.lane:
        # Lease find R3b (welle-validierung-2, 16.10.): a lane lease taken for
        # this pass lay idle and was reclaimed while it ran. Since I3 only a
        # plan without cargo, `--host` and a held lane lease gets here.
        print("gate: NOTE local pass (no cargo station, no --host, no lane lease): runs on this "
              "machine, takes and needs no lane lease", file=out, flush=True)
    dispatch = {"run": run, "repo": doc.get("repo") or "meclaw", "mode": o.mode, "rev": doc.get("rev"),
                "base": doc.get("base"), "dirty": bool(doc.get("dirty")),
                "strand": holder_key(repo, o.strand, o.wave, o.holder), "tree": repo, "target": target,
                "ws": os.path.join(rdir, "ws"), "stations": doc_r["stations"],
                "capacity": dict({"cpu": cpu, "colony": colonies}, **schedule.FIXED_CAPACITY),
                "cache_known": known,
                "cache_keys": cache.part_keys(keys),
                "live_runs": [], "expected_g": 0, "fingerprint_expected": {},
                "budget": _local_budget(doc_r["stations"]),
                "deadline_at": _deadline(doc_r["stations"]), "runner": "local",
                "round": 1, "+env": plus_env,
                "+cpu_overbook": schedule.CPU_OVERBOOK}
    _flake_dispatch(p, db, dispatch, repo, o.mode, doc.get("base"))
    dispatch.update(_flags(o, os.environ.get("MECLAW_GATE_LOCK") or DEFAULT_LOCK, dispatch["stations"]))
    if egon:
        dispatch["+nice"] = True  # the live Egon shares this machine
    # A pass without a lane is the class egon, here and elsewhere (#1158).
    dispatch[executor.RUNNER_CLASS_KEY] = "egon"
    pseudo = lane.Lane(name="local", ssh="local", root=rdir, transport="local")
    head = p.head(run, "local", 1, dispatch)
    # The journal's start is the run's start, on the lane too (#1152).
    dispatch["+started_at"] = dispatch["+dispatched_at"] = head[0]["at"]
    _journal(db, p, head)
    if p.store_lost:  # fail-closed before anything runs (#1161)
        p.note_lost(db, run)
        print(p.lost_line(), file=out)
        return 2
    try:
        proc = lane.exec_stream(pseudo, dispatch)
    except lane.LaneError as exc:
        print("gate: %s" % exc, file=err)
        return 2
    rc = _stream_to_end(p, proc, lambda text: events.ingest(db, text),
                        _aborter(pseudo, repo, run, dispatch["ws"], err), dispatch["deadline_at"])
    if p.pass_ended is None:
        print("gate: the executor ended without pass_ended (exit %s) -- the run is lost" % rc, file=err)
        return 2
    if isinstance(p.fingerprint, dict) and not p.fingerprint.get("unanswered"):
        # M7r A-M1: no answer, no register; K14: what the run did not measure stays
        _write_json(_local_fp_path(), _local_register(_load_json(_local_fp_path()), p.fingerprint))
    p.store("flake register", _flake_ingest, db, run, {})
    v2 = p.v2(dispatch["ws"], p.head(run, "local", 1, dispatch))
    # The full-pass mark (#1165) before the cache: the cache stays the last record.
    p.store("full pass mark", _full_pass_after, o, repo, doc, db, run, p, v2,
            _cache_doubt(keys, fp, p.fingerprint), out)
    if not p.store_lost:
        # Re-check tb-s3-store M-3: the cache is the last record and only of a
        # pass that counts -- a lost record left cache rows, and the advised
        # re-run came back CACHED without healing the hole.
        p.store("cache record", _record, db, run, "local", keys, fp, p.fingerprint, p.events)
    return p.finish(v2, dispatch["ws"], "local", 1, db=db, run=run)


def _sync(db, lanes, host):
    """Register the lanes; an `off` ssh lane that answers is `idle` (`lane.status`)."""
    schedule._ensure(db)
    known = {r[0]: r[1] for r in db.execute("SELECT name, state FROM runners").fetchall()}
    states = {}
    for ln in lanes:
        if ln.transport != "ssh" or (host and ln.name != host):
            continue
        if known.get(ln.name, "off") == "off" and lane.status(ln).startswith("reachable"):
            states[ln.name] = "idle"
    schedule.sync_runners(db, lanes, states=states)


def _fetch_metadata(ln, ws, path):
    """`<ws>/reuse/cargo-metadata.json` of a run with `build` into the metadata
    cache at `path` -- the key computed when the plan was made (Z1): a key
    taken from the live tree after the run would race a tests/*.rs added
    meanwhile and file the metadata under a key the next plan of that tree
    never matches."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    # Review Zb R1 M3: a temp file of its own -- runs with one key (other
    # worktrees too) raced on a shared `.fetch`; the cache is optional, so a
    # lost race never throws out of a finished run.
    tmp = "%s.%d.fetch" % (path, os.getpid())
    res = lane._rsync(ln, ["-a", lane._lane_path(ln, ws.rstrip("/") + "/reuse/cargo-metadata.json"), tmp])
    try:
        if res.returncode == 0 and isinstance(_load_json(tmp), dict):
            os.replace(tmp, path)
    except OSError:
        pass
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)


# Z13: run-696ff322bd86 died at import on build03 after 10 s; the fetch failed
# and its admission kept 8 cpu of the lane until deadline + grace, so two runs
# were refused `needs cpu 8+8 > 8`. A scope systemd reports in one of these
# states runs nothing any more; every other answer (active, activating, no
# answer at all) keeps the lease as before -- the fail-safe side.
UNIT_GONE = "fetch_failed_unit_gone"
# Z17 (OR-DT.Z13.4): the fetch went through, the run never ended (OOM, a
# crash after `ws/`) -- before, its lease held until the deadline.
UNIT_GONE_FETCHED = "unit_gone"
UNIT_GONE_STATES = ("inactive", "failed", "unknown")


def _unit_gone(ln, adm, err):
    """The scope state of `adm.run` when it is gone, else None (the lease stays)."""
    state = lane.unit_state(ln, adm.run)
    if state not in UNIT_GONE_STATES:
        print("gate: the unit of %s on %s is %s -- the lease stays"
              % (adm.run, adm.lane, state or "unknown (no answer)"), file=err)
        return None
    return state


def _note_unit_gone(db, p, adm, reason, state):
    """Z17 (OR-DT.Z13.3): one `lease_noted` row AFTER the close went through --
    the close itself writes the `lease_released` of an armed token, so a row
    here before it stood in the journal for a close the store had refused,
    and twice beside an armed token's own."""
    p.store("unit note", events.emit, db, "lease_noted", "note",
            {"resource": adm.lane, "holder": adm.strand or adm.wave or "-", "run": adm.run,
             "ttl_s": None, "verb": "unit_gone", "detail": reason,
             "unit": "pipe-%s.scope" % adm.run, "unit_state": state},
            run=adm.run)


def _lost(o, p, ln, adm, dest, close, rc, err, db, scoped=False):
    """The stream ended without `pass_ended`: fetch what the lane has. An abort
    the executor demonstrably made ends ABORTED (summary, archive, legacy);
    anything else is lost -- with the partial archive under `--archive`.
    Then the lane is asked for the run's scope (Z13; since Z17 also after a
    fetch that went through, OR-DT.Z13.4): a scope that is gone ends the run
    ABORTED and frees its lease at once. A run started
    without a scope (`scoped` false) has no unit to ask -- `is-active` says
    `inactive` for a unit that never was -- so its lease stays (I1)."""
    fetch_failed = False
    try:
        lane.fetch(ln, adm.run, dest)
    except lane.LaneError as exc:
        fetch_failed = True
        print("gate: %s" % exc, file=err)
    fetched = _fetched_events(dest)
    aborted = _aborted(p.events + fetched, dest)
    if len(fetched) > len(p.events):
        p.events = fetched
    head = p.head(adm.run, adm.lane, adm.round, adm.dispatch)
    if aborted:
        p.pass_ended = {"event": "pass_ended", "data": {"verdict": "ABORTED"}}
        close(p.pass_ended)
        print("gate: the stream from %s ended without pass_ended (exit %s) -- the executor had "
              "aborted the run" % (adm.lane, rc), file=err)
        return p.finish(receipt.project(head + p.events), dest, adm.lane, adm.round)
    gone = None
    reason = UNIT_GONE if fetch_failed else UNIT_GONE_FETCHED
    if not scoped:
        print("gate: NOTE %s on %s started without a scope -- no unit to ask, the lease stays"
              % (adm.run, adm.lane), file=err)
    else:
        gone = _unit_gone(ln, adm, err)
    if gone:
        p.pass_ended = {"event": "pass_ended", "data": {"verdict": "ABORTED", "reason": reason}}
        freed = close(p.pass_ended)
        if freed:
            _note_unit_gone(db, p, adm, reason, gone)
        print("gate: the stream from %s ended without pass_ended (exit %s) and the run's unit is %s "
              "-- closed ABORTED (%s), %s" % (
                  adm.lane, rc, gone, reason, "the lease is free" if freed else
                  "but the store refused the close, the lease stays until its deadline"),
              file=err)
        return p.finish(receipt.project(head + p.events), dest, adm.lane, adm.round)
    close(None, lost=True)
    print("gate: the stream from %s ended without pass_ended (exit %s) -- the run is lost; "
          "the lane keeps its lease until it is seen again" % (adm.lane, rc), file=err)
    if o.archive:
        v2 = receipt.project(head + p.events)
        p.archive(v2, dest, receipt.gate_lines(v2))
    return 2


# Re-check v2-r3-lease m2: the clock of the admission wait line (tests inject it).
_clock = time.monotonic


class _Signalled(SystemExit):
    """SIGTERM/SIGHUP inside a lane pass (#1228 round 3, K13): the default
    action ended the driver without its `finally`, the admission stayed and
    kept the lease. Raised in the main thread, it unwinds like Ctrl-C; as a
    SystemExit the CLI ends with 128 + signal, no traceback."""


class _ExitSignals:
    """The trap of `_trap_exit_signals`: calling it restores the old handlers;
    `held()` defers SIGTERM/SIGHUP/SIGINT over the spawn window (re-check
    v2-r3-lease m1): a signal between the executor's spawn in `exec_stream`
    and `started.append` ended the pass as "not started" -- ABORTED, the
    lease free under a running executor. Deferred, it raises once `started`
    names the executor (or `exec_stream` raised: then nothing started).
    No signal mask: a blocked mask would pass to the spawned child."""

    def __init__(self, old=None):
        self.old = old if old is not None else {}
        self.holding, self.pending = False, None

    def __call__(self):
        import signal
        for sig, handler in self.old.items():
            signal.signal(sig, handler)

    @contextlib.contextmanager
    def held(self):
        import signal
        if not self.old:    # not the main thread: nothing to defer
            yield
            return
        self.holding, self.pending = True, None
        old_int = signal.signal(signal.SIGINT, self._defer)
        try:
            yield
        finally:
            signal.signal(signal.SIGINT, old_int)
            self.holding = False
            pending, self.pending = self.pending, None
            if pending == signal.SIGINT:
                raise KeyboardInterrupt
            if pending is not None:
                raise _Signalled(128 + pending)

    def _defer(self, signum, _frame):
        if self.pending is None:
            self.pending = signum


def _trap_exit_signals():
    """SIGTERM and SIGHUP raise `_Signalled` for the pass; returns the trap
    (`_ExitSignals`: call it to restore). Only the main thread may set a
    handler -- elsewhere nothing changes."""
    import signal
    if threading.current_thread() is not threading.main_thread():
        return _ExitSignals()
    trap = _ExitSignals()

    def raise_(signum, frame):
        # one-shot: a second signal must not break the close on the way out
        for sig in trap.old:
            signal.signal(sig, signal.SIG_IGN)
        if trap.holding:
            return trap._defer(signum, frame)
        raise _Signalled(128 + signum)
    for sig in (signal.SIGTERM, signal.SIGHUP):
        try:
            trap.old[sig] = signal.signal(sig, raise_)
        except (ValueError, OSError):
            continue
    return trap


def _close_on_the_way_out(ln, adm, d, started, close, err):
    """#1228 round 3 (K13): a way out of a pass without its close -- an exception,
    a LaneError, Ctrl-C, SIGTERM/SIGHUP -- frees the lease. Before the executor
    started (`started` empty) nothing of the run lives on the lane: ABORTED.
    After it, the executor is asked to abort (nobody reads its stream any more),
    then the lane is probed (`schedule.run_gone`: the scope, the workspace, the
    tree): gone is ABORTED, anything else -- a pid, no answer, a run without a
    scope (I1) -- stays lost (fail-closed); `take` closes it once the lane
    probes empty and the driver is dead (`schedule._close_dead_runs`)."""
    aborted = {"event": "pass_ended", "data": {"verdict": "ABORTED"}}
    if not started:
        if close(aborted):
            print("gate: the pass of %s ended before its executor started on %s -- closed "
                  "ABORTED, the lease is free" % (adm.run, adm.lane), file=err)
        return
    proc = started[0]
    ws = d.get("ws") or "%s/ws/%s" % (ln.root, adm.run)
    why = ""
    if not getattr(proc, "scoped", False):
        why = "%s started without a scope -- no unit to ask (I1)" % adm.run
    else:
        try:
            lane.abort(ln, d["tree"], adm.run, ws)
        except lane.LaneError as exc:
            print("gate: abort of %s on the way out: %s" % (adm.run, exc), file=err)
        try:
            _state, why = schedule.run_gone(ln, adm.run, ws, d.get("tree") or "", lane)
        except Exception as exc:  # noqa: BLE001 -- fail-closed: no answer keeps the lease
            why = "the probe failed: %s: %s" % (type(exc).__name__, exc)
    if not why and close(aborted):
        print("gate: the pass of %s ended without its close and the lane shows the run "
              "gone -- closed ABORTED, the lease is free" % adm.run, file=err)
        return
    close(None, lost=True)
    print("gate: the pass of %s ended without its close -- the run is lost, the lane keeps "
          "its lease (%s); a take frees it once the lane probes empty"
          % (adm.run, why or "the store refused the close"), file=err)


def _run_lane(o, repo, doc, metadata, db, out, err, meta_path=None):
    meta_path = meta_path or metadata_path(repo)
    queued_at = []

    def queued(q):
        queued_at.append((_clock(), q.reason))
        print("gate: queued -- %s (waiting for admission)" % q.reason, file=err, flush=True)

    try:
        lanes = lane.load_lanes()
        if o.wait_admission:
            # tm-orch4: a waiter outlasts a locked store from its first write on
            schedule.while_locked(lambda: _sync(db, lanes, o.host),
                                  end=time.monotonic() + schedule.wait_secs(
                                      os.environ, "MECLAW_ADMISSION_WAIT_SECS", "14400"),
                                  reset=lambda: schedule._rollback(db))
        else:
            _sync(db, lanes, o.host)
        kw = dict(repo=doc.get("repo") or "meclaw", mode=o.mode,
                  strand=holder_key(repo, o.strand, o.wave, o.holder), wave=o.wave or "",
                  base=doc.get("base"), lane_pref=o.host, now=None,
                  tree_name=os.path.basename(repo.rstrip("/")), any_lane=bool(o.any_lane),
                  # K14: a merge candidate's gate waits ahead of the strands
                  merge=os.environ.get("MECLAW_GATE_MODE", "") == "merge",
                  # re-check Kette Minor 2: the release class reads the strand file
                  # in the caller's tree -- with and without --wait-admission
                  release_tree=os.path.abspath(repo))
        if o.wait_admission:
            adm = schedule.admit_waiting(db, doc, on_queued=queued,
                                         wait_tree=os.path.abspath(repo), **kw)
            if queued_at:
                # re-check v2-r3-lease m2: the wait is said once it ends (admitted
                # or given up) -- `merge --wait` counts it as the lane wait
                at, why = queued_at[0]
                print("gate: admission wait %ds (%s)" % (round(_clock() - at), why),
                      file=err, flush=True)
        else:
            # Kit find (h): a merge gate without --wait never comes back for an offer -- a
            # queue place of it would block the lane for every waiter behind.
            adm = schedule.admit(db, doc, wait=False, **kw,
                                 queue=os.environ.get("MECLAW_GATE_MODE", "") != "merge")
    except (schedule.ScheduleError, lane.LaneError) as exc:
        print("gate: %s" % exc, file=err)
        return getattr(exc, "code", 2) or 2
    except sqlite3.Error as exc:
        # Review Z-A2 R2 M2: a busy or broken store is no RED (1) -- its own exit.
        print("gate: store: %s -- no admission" % exc, file=err)
        return STORE_EXIT
    if isinstance(adm, schedule.Queued):
        print("gate: queued -- %s%s" % (adm.reason, " (power requested: %s)" % adm.power_requested
                                         if adm.power_requested else ""), file=err)
        return 3
    p = _Pass(o, repo, doc, out, err)
    held, closed, started, ln, d = None, [], [], None, {}
    restore_signals = _trap_exit_signals()

    def close(pass_ended, lost=False):
        # One close per admission (a second one walks a finished item); a close
        # the store refuses for good ends the pass ABORTED (#1161).
        closed.append(True)
        ok = []
        if p.store_lost and pass_ended is not None:  # the store's verdict follows the pass (#1161)
            pass_ended = dict(pass_ended, data=dict(pass_ended.get("data") or {}, verdict="ABORTED"))
        p.store("close", lambda: (schedule.close(db, adm, pass_ended, lost=lost), ok.append(1)))
        return bool(ok)

    try:
        ln = lane.find_lane(adm.lane)
        d = dict(adm.dispatch)
        fp_before = d.get("fingerprint_expected") or {}
        # The stations' environment, the executor's one rule (re-check tv-bn M7t A-M2):
        # the dispatch's whole `+env` -- the lane's words (`lane.env_for`), the
        # register's declared variables and the mode, completed here, before the
        # lookup -- measured where the executor will run (`executor.stations_build`:
        # on a host there, under the executor's command form; OR-T12-18 A-M1/A-M3).
        from . import executor  # the stations' environment is the executor's rule
        satellite = satellites.is_satellite(repo)
        try:
            passed, passed_refused = _lane_env(repo, satellite, os.environ), None
        except satellites.RegisterError as exc:  # said where it always was: before the push
            passed, passed_refused = {}, exc
        # Every station sees the pass's mode, as under gate_legacy.sh (W4);
        # the register's declared variables beside it, never over the kit's.
        d["+env"] = dict(passed, **dict(d.get("+env") or {}, MECLAW_GATE_MODE=o.mode))
        build = executor.stations_build(ln, d["+env"], repo, d.get("tree"))
        doc_r = plan.resolve_needs(doc, int((adm.capacity or {}).get("cpu") or 1),
                                   _lane_colonies(adm))
        keys, known, fp_before = _lookup(db, doc_r, repo, metadata, fp_before, adm.lane, o.mode,
                                         o.cache, build)
        p.vcpu = schedule.lane_vcpu(db, adm.lane)
        print(_plan_line(doc, known, keys, adm.cargo, adm.lane, p.plan_path(), o.cache),
              file=out, flush=True)
        for note in adm.notes:   # the admission's say (tb-s10-b P7: a lane fallback)
            print("gate: NOTE %s" % note, file=out, flush=True)
        # `run_dispatched` is the schedule's (one per station, at the admission).
        head = p.head(adm.run, adm.lane, adm.round, adm.dispatch)[:1]
        d["+started_at"] = head[0]["at"]  # the journal's start is the run's start (#1152)
        _journal(db, p, head)
        if p.store_lost:  # m-5: nothing travels after a lost journal head (#1161)
            close({"event": "pass_ended", "data": {"verdict": "ABORTED"}})
            p.note_lost(db, adm.run)
            print(p.lost_line(), file=out)
            return 2
        name = os.path.basename(repo.rstrip("/"))
        try:
            # W1's guards of a lane run (`lane_sync.sh guard`: a local lane only as class egon,
            # no gate on class egon, the floor, the slot) before anything travels.
            # The build path is this run's from the push to the fetch: a lane-side
            # flock no token skip bypasses (W1 hand-over, `lane.hold`). The
            # hygiene in front of the floor reserves this admission's size.
            # The admission's build path (Delta A M-neu-4): `tree/` with cargo, else `wt/<name>`.
            tree, _target = lane.guard(ln, name, cargo=adm.cargo,
                                       expected_g=_guard_expected_g(adm, os.environ))
            held = lane.hold(ln, tree)
        except lane.LaneError as exc:
            print("gate: lane %s refused the run:\n  %s" % (adm.lane, str(exc).replace("\n", "\n  ")),
                  file=err)
            close({"event": "pass_ended", "data": {"verdict": "ABORTED"}})
            return exc.code or 2
        try:
            # Derived before anything travels: a declared variable that
            # cannot travel aborts the run (welle-tb tb-zcol A).
            if passed_refused is not None:
                raise passed_refused
            pushed = lane.push(ln, repo, name, adm.cargo, **_satellite_transport(repo, satellite))
            if satellite:
                # The stations run in the lane's checkout, from the kit there,
                # sized for the lane's cpu (GH #1163: the colony density).
                lane_cpu = int((adm.capacity or {}).get("cpu") or 1)
                sat = _satellite_lane_plan(o, repo, ln, kit=pushed["kit"], run_root=pushed["tree"],
                                           lane_cpu=lane_cpu)
                for note in sat.get("notes") or []:
                    print("gate: NOTE lane %s: %s" % (adm.lane, note), file=out, flush=True)
                doc_r = plan.resolve_needs(sat, lane_cpu, _lane_colonies(adm))
        except (lane.LaneError, satellites.RegisterError) as exc:
            print("gate: %s" % exc, file=err)
            close({"event": "pass_ended", "data": {"verdict": "ABORTED"}})
            return getattr(exc, "code", 2) or 2
        d.update({"tree": pushed.get("tree") or d.get("tree"), "target": pushed.get("target") or d.get("target"),
                  "stations": doc_r["stations"], "cache_known": known, "runner": adm.lane,
                  "cache_keys": cache.part_keys(keys),
                  "round": adm.round, "dirty": bool(doc.get("dirty"))})
        d.update(_flags(o, os.environ.get("MECLAW_GATE_LOCK") or DEFAULT_LOCK, d.get("stations")))
        # The Egon cap follows the store's runner class, never the gate's
        # `+nice` (#1158); a class the store cannot name stays capped.
        from . import executor  # the key is the executor's
        cls = d[executor.RUNNER_CLASS_KEY] = p.store("runner class", _runner_class, db, adm.lane)
        if cls not in executor.UNCAPPED_CLASSES:
            # Only `class=lane` runs the booked width; Egon's lane is niced too (m-1).
            d["+nice"] = True
            if cls != "egon":
                print("gate: NOTE lane %s: runner class %r is neither egon nor lane -- the Egon "
                      "cap holds (class=lane in the host file names a build host)"
                      % (adm.lane, cls), file=err)
        if passed:
            print("gate: NOTE lane %s: passes %s" % (adm.lane, " ".join(sorted(passed))),
                  file=out, flush=True)
        _flake_dispatch(p, db, d, repo, o.mode, doc.get("base"))
        # GH #1162 A3: rank and reserve by the measured durations of this lane's
        # class (`schedule.measured_secs`); a station without one keeps its
        # declared `prior_s` (plan.PRIOR_S), else its timeout.
        measured = p.store("measured durations", schedule.measured_secs, db,
                           doc.get("repo") or "meclaw", o.mode, adm.lane) or {}
        p95 = p.store("station p95", schedule.measured_p95, db,
                      doc.get("repo") or "meclaw", o.mode, adm.lane) or {}
        if p.store_lost:  # fail-closed before anything runs (#1161)
            close({"event": "pass_ended", "data": {"verdict": "ABORTED"}})
            p.note_lost(db, adm.run)
            print(p.lost_line(), file=out)
            return 2
        d["stations"] = schedule.with_measured(d["stations"], measured)
        # TV-K5 (b): a cap is a hang marker of at least 3x the green P95 on this
        # lane's class; a cap below it is a finding and is raised.
        d["stations"], raised = schedule.with_hang_caps(d["stations"], p95)
        # A cap the 120-min clamp holds below 3x P95 is a finding too (re-check tv-p0).
        for line in schedule.hang_cap_notes(raised, schedule.clamped_caps(d["stations"], p95)):
            print(line, file=out, flush=True)
        try:
            # m1: the spawn and its record are one step for a signal
            with restore_signals.held():
                proc = lane.exec_stream(ln, d, pipeline=(pushed["kit"] + "/scripts/pipeline.py")
                                        if satellite else None)
                started.append(proc)
        except lane.LaneError as exc:
            # `exec_stream` raises only when the executor did not start: nothing
            # of the run lives on the lane, the lease is free (#1228 round 3, K13).
            print("gate: %s" % exc, file=err)
            close({"event": "pass_ended", "data": {"verdict": "ABORTED"}})
            return 2
        ws = d.get("ws") or "%s/ws/%s" % (ln.root, adm.run)
        rc = _stream_to_end(p, proc, lambda text: schedule.ingest(db, adm, text),
                            _aborter(ln, d["tree"], adm.run, ws, err), d.get("deadline_at"))
        dest = os.path.join(state_dir(), "runs", adm.run)
        if p.pass_ended is None:
            return _lost(o, p, ln, adm, dest, close, rc, err, db, getattr(proc, "scoped", False))
        try:
            lane.fetch(ln, adm.run, dest, pushed=pushed, repo_root=repo)
        except lane.LaneError as exc:
            print("gate: %s" % exc, file=err)
        _fetch_metadata(ln, ws, meta_path)
        # The register before the close (#1161): a lost flake record makes the
        # close ABORTED; the issue duty (gh) stays after the hold below.
        p.store("flake register", flakes.ingest_run, db, adm.run, dict(adm.tasks or {}))
        close(p.pass_ended)
        # OR-DT.W8.S.1: the build path is free before the issue duty's gh calls
        # (up to 2 x 60 s per pending test) -- nothing travels any more.
        if held is not None:
            held.release()
            held = None
        p.store("flake issue duty", _flake_ingest, db, adm.run, dict(adm.tasks or {}))
        v2 = p.v2(dest, p.head(adm.run, adm.lane, adm.round))
        p.store("full pass mark", _full_pass_after, o, repo, doc, db, adm.run, p, v2,
                _cache_doubt(keys, fp_before, p.fingerprint), out)  # before the cache (local)
        if not p.store_lost:  # M-3: the cache last, only of a pass that counts (see local)
            p.store("cache record", _record, db, adm.run, adm.lane, keys, fp_before,
                    p.fingerprint, p.events)
        return p.finish(v2, dest, adm.lane, adm.round, db=db, run=adm.run)
    finally:
        # Any way out without a close -- an exception, Ctrl-C, SIGTERM/SIGHUP --
        # frees the lease, or leaves the admission lost when the lane may still
        # run it (A-C1; #1228 round 3, K13).
        try:
            if not closed:
                _close_on_the_way_out(ln, adm, d, started, close, err)
        finally:
            restore_signals()
            if held is not None:
                held.release()


def run_gate(args, out, err):
    """`pipeline.py gate` in-process: `args` is the argument list after `gate`."""
    o = parse(args, err)
    if isinstance(o, int):
        return o
    if o.help:
        print(USAGE, file=out)
        return 0
    repo = _git(os.path.abspath(o.repo) if o.repo else os.getcwd(), "rev-parse", "--show-toplevel")
    if not repo:
        print("gate: not inside a git repository", file=err)
        return 2
    satellite = satellites.is_satellite(repo)
    if not o.plan_only:
        refused = _pre_stamp(o, repo, err)
        if refused:
            return refused
    # The cached `cargo metadata` feeds the plan too: without it a rdeps()/all()/t0
    # selection cannot be split into binaries (no parts -> no cache, `build --workspace`).
    # Z1: the key is taken once, here, and travels to the fetch after the run.
    meta_path = metadata_path(repo)
    metadata = _load_json(meta_path)
    metadata = metadata if isinstance(metadata, dict) else None
    try:
        if satellite:
            doc = _satellite_plan(o, repo)
        else:
            # Merge and revert run `gate.sh strand` under MECLAW_GATE_MODE=merge:
            # they plan unnarrowed (review Z-I1 C2) -- unless the merge names its
            # one strand's GREEN run (MECLAW_GATE_STRAND_RUN): then
            # `narrow.merge_decision` decides, fail-closed (welle-tb S1 B, #1159).
            # An integration plans from the wave diff of its wave: MECLAW_GATE_WAVE,
            # else the wave `strand.sh gate` names (--wave, re-check H2 M1).
            kw = {}
            floor_why = None
            if os.environ.get("MECLAW_GATE_MODE", "") == "merge":
                run = os.environ.get("MECLAW_GATE_STRAND_RUN", "").strip()
                narrow_ok = bool(run) and o.mode == "strand"
                if narrow_ok:
                    kw = {"stage": "merge", "strand_run": run}
                else:
                    floor_why = ("no strand run named (MECLAW_GATE_STRAND_RUN)" if not run
                                 else "merge of a %s pass" % o.mode)
            else:
                narrow_ok = True
                if o.mode == "integration":
                    kw = {"wave": os.environ.get("MECLAW_GATE_WAVE", "").strip()
                          or (o.wave or "").strip() or None}
            full = full_pass.for_plan(o.full, _full_wave(o, repo),  # #1165
                                      strand=_full_strand(o, repo))
            doc = plan.build_plan(repo, o.mode, o.base, only=o.only, metadata=metadata,
                                  narrow=narrow_ok, full=full, **kw)
            if floor_why and not doc.get("narrowing"):
                doc["narrowing"] = {"tests": "floor", "stage": "merge", "since": None,
                                    "wave": None, "map": None, "why": floor_why}
    except Exception as exc:  # noqa: BLE001 -- a plan that cannot be made refuses the pass
        print("gate: no plan: %s" % exc, file=err)
        return 2
    _header(doc, repo, o.mode, out)
    if doc.get("narrowing"):
        print(_scope_line(doc["narrowing"]), file=out)
    for note in doc.get("notes") or []:  # the narrowing's say (#1138)
        print("gate: NOTE %s" % note, file=out)
    db = store.open()
    cargo = any(_needs_cargo(s) for s in doc.get("stations") or [])
    here = not satellite and _runs_here(o, repo, db, cargo)
    if o.plan_only:
        fp, lane_name = _plan_fingerprint(db, o.host or ("" if here else _held_lane(db, repo, o)), repo)
        _plan_only(doc, out)
        keys, known, _fp = _lookup(db, doc, repo, metadata, fp, lane_name or "local", o.mode,
                                   o.cache, _plan_build(o, repo, satellite, here, fp, lane_name,
                                                        cargo))
        print(_plan_line(doc, known, keys, cargo, lane_name or "-", None, o.cache), file=out)
        return 0
    if satellite:
        # Koordinator (welle-tb S3 F1, re-check h1 M-1): a satellite never runs on
        # this machine -- the admission books it a lane (`--host` is a preference,
        # none: any lane; an egon lane never takes it, `schedule._admit`).
        if o.lane:
            print("gate: a satellite pass never runs on this machine (--lane): "
                  "the admission books it a lane", file=err)
            return 2
        return _run_lane(o, repo, doc, metadata, db, out, err, meta_path)
    if cargo and o.lane and _lane_flag_refused(db, out, err):
        return 3
    if here:
        return _run_local(o, repo, doc, metadata, db, out, err)
    return _run_lane(o, repo, doc, metadata, db, out, err, meta_path)


def _lane_flag_refused(db, out, err):
    """`--lane` with a cargo plan runs cargo on this machine. While the lanes
    are armed that is refused (#1139, re-check pd-legacy m-2), like the
    tier's and `gate_legacy.sh`'s refusal; `MECLAW_LANE_LOCAL_OK=<reason>`
    is the one way past, and a NOTE names it. A store that cannot say counts
    as armed (fail-closed)."""
    try:
        armed = bool((schedule._load(db) or {}).get("lanes"))
    except Exception:  # noqa: BLE001 -- any fault of the store is "armed"
        armed = True
    if not armed:
        return False
    reason = os.environ.get("MECLAW_LANE_LOCAL_OK", "")
    if reason:
        print("gate: NOTE lane-local-ok %s" % reason, file=out, flush=True)
        return False
    print("gate: lanes active: --lane runs cargo on this machine; use scripts/strand.sh gate "
          "(the lanes) or MECLAW_LANE_LOCAL_OK=<reason>", file=err, flush=True)
    return True


CORE_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _lane_env(repo, satellite, environ):
    """The variables a satellite's register declares (`lane_env`) that are set
    in `environ`, by exact name -- for the dispatch's `+env`; the core declares
    none. RegisterError when a declaration or a value cannot travel."""
    if not satellite:
        return {}
    return satellites.lane_env(satellites.load_register(repo), environ)


def _satellite_transport(repo, satellite, core_root=None, home=None):
    """What travels with a satellite's tree besides the tree (W10 + GH #1143):
    this core's kit and what the satellite's set pins need (asset cache,
    source mirrors with meclaw-core). Derived BEFORE anything travels: a pin
    that cannot travel raises `RegisterError` and the run aborts."""
    if not satellite:
        return {}
    core_root = core_root or CORE_ROOT
    return {"kit": core_root, "pins": satellites.set_pins(repo, core_root, home=home)}


def _satellite_plan(o, repo, kit=None, run_root=None, lane_cpu=None, lane_cores=None):
    """A satellite's plan from its register (W10): here with this core's
    pipeline, on a lane with the kit's (`kit`) in its checkout (`run_root`),
    its density stations sized for `lane_cpu` (default: this machine's)."""
    slots = [s for s in (o.slots or "").split(",") if s.strip()] or None
    doc = satellites.build_plan(repo, o.mode, o.base,
                              pipeline_dir=(kit + "/scripts/pipeline") if kit else None,
                              python="python3" if kit else None, slots=slots, run_root=run_root,
                              only=o.only or None, lane_cpu=lane_cpu, lane_cores=lane_cores)
    return doc


def _satellite_lane_plan(o, repo, ln, kit=None, run_root=None, lane_cpu=None):
    """A satellite's plan on lane `ln`: sized for the real cores its host-file
    line declares (`cores=`, welle-tb S5) -- `lscpu` on a VM lane counts the
    guest's topology (build03: 1 socket x 12 cores x 1 thread on a 6-core
    host). Without the word the register takes nproc / 2 and the plan's notes
    say so first."""
    doc = _satellite_plan(o, repo, kit=kit, run_root=run_root, lane_cpu=lane_cpu,
                          lane_cores=ln.cores or None)
    if not ln.cores:
        doc.setdefault("notes", []).insert(
            0, "the host file declares no cores= for lane %s: cores = nproc %d / 2"
            % (ln.name, int(lane_cpu or 1)))
    return doc


def _cmd_gate(args):
    """`pipeline.py gate`. GH #1199 (V2-A8): an exit that is neither a verdict
    (0 GREEN, 1 RED, 4 ASK) nor a pass that printed its GATE-SUMMARY is a
    refusal before the pass -- its last stderr line goes to stdout too (when
    that is another file) and to the lease, so a wrapper keeping one stream
    and `token who` see it."""
    out, err = progress.Tail(sys.stdout, marks=("GATE-SUMMARY",)), progress.Tail(sys.stderr)
    rc = run_gate(args.argv, out, err)
    if rc not in (0, 1, 4) and not out.marked:
        progress.refuse("gate", err.last or "no reason printed", out=sys.stdout, err=sys.stderr,
                        holder=_gate_holder(args.argv), code=rc, word="REFUSED (exit %d)" % rc)
    return rc


def _gate_holder(argv):
    """The lease key of a gate call from its words (`--strand/--wave/--holder`), "" when unknown."""
    val = {k: argv[i + 1] for i, k in enumerate(argv[:-1]) if k in ("--strand", "--wave", "--holder")}
    try:
        repo = _git(os.getcwd(), "rev-parse", "--show-toplevel")
        return holder_key(repo, val.get("--strand"), val.get("--wave"), val.get("--holder")) if repo else ""
    except (OSError, subprocess.SubprocessError):
        return ""


# --- pipeline.py test ----------------------------------------------------------------------

TEST_USAGE = """usage: pipeline.py test '<filterset>' [--host <lane>] [--strand <name>] [--wave <wave>]
         [--holder <n>] [nextest options] [-- <nextest options>]

Runs one nextest filterset on the lane this strand holds (`--holder <n>`: its
second lease `<strand>:<n>`, GH #1146; or `--host`, which
also binds to a held token unless MECLAW_STRAND_TOKEN_SKIP names a reason) in
its warm build tree (`scripts/test-tier.sh filter`). The log lands in
plans/<wave>/receipts/<strand>/test-<utc>-<rev>/test.log, the last line is
`TEST [<filterset>] <passed>/<total> <secs>s <verdict>`. Exit: the tier's exit,
6 when no test ran, 2 for a lane that refused or was lost, 3 without a lane."""

# nextest options that take the next word (GH #1050) -- never the filterset.
TEST_VALUE_OPTS = frozenset((
    "--run-ignored", "-j", "--test-threads", "--retries", "--max-fail", "--partition",
    "--status-level", "--final-status-level", "--failure-output", "--success-output",
    "--cargo-profile", "--features", "-p", "--package", "-F", "--exclude", "--test", "--bin",
    "--example", "--bench", "--target", "--target-dir", "--no-tests", "-P", "--profile",
    "--manifest-path", "--config", "--tool-config-file", "--color", "--message-format",
    "--build-jobs", "--show-progress"))
TEST_SUMMARY = re.compile(r"([0-9]+) tests? run: ([0-9]+) passed")
STRESS_ITER = re.compile(r"Stress test \[.*\] iteration [0-9]+/[0-9]+: [0-9]+ tests? run:")
STRESS_SUM = re.compile(r"Summary \[.*\] [0-9]+/[0-9]+ stress run iterations:")
RED_LINE = re.compile(r"^ +(FAIL|TIMEOUT|SIG[A-Z]+|LEAK-FAIL) \[")
GOLDEN_LINE = re.compile(r"^lane(?:_sync)?: (golden .*)$")


class _TestExit(Exception):
    def __init__(self, code, message=""):
        super().__init__(message)
        self.code = code


def _parse_test(argv):
    """`(expr, host, strand, wave, holder, extra)` or `_TestExit` -- strand.sh's
    `cmd_test_legacy` loop. `--holder <n>` (kit find (j), tv-s1-rust D2): the
    strand's second lease `<strand>:<n>` like `pre`/`gate`/`token` (GH #1146);
    before, it went to nextest and `<n>` became the filterset."""
    expr, host, strand, wave, holder, extra = "", "", "", "", None, []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            raise _TestExit(0, TEST_USAGE)
        if a in ("--host", "--strand", "--wave", "--holder") or a in TEST_VALUE_OPTS:
            if i + 1 >= len(argv):
                raise _TestExit(2, "strand: %s needs a value" % a)
            if a == "--host":
                host = argv[i + 1]
            elif a == "--strand":
                strand = argv[i + 1]
            elif a == "--wave":
                wave = argv[i + 1]
            elif a == "--holder":
                holder = argv[i + 1]
                if not schedule.HOLDER_RE.match(holder):
                    raise _TestExit(2, "strand: test: --holder takes a short suffix "
                                       "([A-Za-z0-9_-], up to 32): %s" % holder)
            else:
                extra += [a, argv[i + 1]]
            i += 2
            continue
        if a == "--":
            extra += argv[i + 1:]
            break
        if a.startswith("-") or expr:
            extra.append(a)
        else:
            expr = a
        i += 1
    if not expr:
        raise _TestExit(2, "strand: test: expected a nextest filterset, e.g. 'binary(~gh123)'")
    # Kit find tv-br M5 (M7d2): `--strand <s>:<n>` went whole into the push
    # refspec and the push failed; the second lease is `--holder <n>`.
    if schedule.strand_colon_error(strand):
        raise _TestExit(2, "strand: test: " + schedule.strand_colon_error(strand))
    return expr, host, strand, wave, holder, extra


def _main_root(repo):
    common = _git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir")
    return os.path.dirname(common) if common else ""


def _wave_dir(plans, want, key, err):
    """`--wave`, else the one `plans/<wave>-*` of the strand's key `<wave>/<S>`
    -- the branch, or a `--strand <wave>/<S>` in a detached tree (GH #1199 f);
    the strand's report breaks a tie."""
    if want:
        want = want.rstrip("/")
        if not os.path.isdir(os.path.join(plans, want)):
            raise _TestExit(2, "strand: no such wave: plans/%s" % want)
        print("strand: wave %s" % want, file=err)
        return want
    branch = key
    if "/" in branch:
        wave, name = branch.split("/", 1)[0], branch.rsplit("/", 1)[-1]
        try:
            hits = sorted(d for d in os.listdir(plans)
                          if d.startswith(wave + "-") and os.path.isdir(os.path.join(plans, d)))
        except OSError:
            hits = []
        owners = [d for d in hits if os.path.isfile(os.path.join(plans, d, "berichte", name + ".md"))]
        for cand in (hits if len(hits) == 1 else owners if len(owners) == 1 else []):
            print("strand: wave %s (from the branch)" % cand, file=err)
            return cand
    raise _TestExit(2, "strand: cannot tell the wave from the branch -- pass --wave")


def _num(s):
    """awk's `s + 0`: the leading integer, else 0."""
    m = re.match(r"\s*([+-]?[0-9]+)", s)
    return int(m.group(1)) if m else 0


def _stress(lines):
    """A stress series graded by its iterations (GH #1086): `(n, ran, red, passed, total)` or None."""
    n = it = ran = red = sred = passed = total = 0
    seen = False
    for line in lines:
        if STRESS_ITER.search(line):
            rest = re.sub(r".* iteration ", "", line, count=1)
            parts = re.split(r"[/:]", rest)
            n = _num(parts[1]) if len(parts) > 1 else 0
            it += 1
            t = re.sub(r"^[^:]*: ", "", rest, count=1)
            total += _num(t)
            passed += _num(re.sub(r".*run: ", "", t, count=1))
            if re.search(r", [1-9][0-9]* (failed|timed out|exec failed)", t):
                red += 1
        if STRESS_SUM.search(line):
            rest = re.sub(r".*\] ", "", line, count=1)
            parts = re.split(r"[/ ]", rest)
            ran, n = _num(parts[0]), _num(parts[1]) if len(parts) > 1 else 0
            sred = 0
            if re.search(r"passed, [0-9]+ failed", rest):
                sred = _num(re.sub(r".* passed, ", "", rest, count=1))
            seen = True
    if not it and not seen:
        return None
    if seen and sred > red:
        red = sred
    if not seen:
        ran = it
    if not n:
        n = it
    return n, ran, red, passed, total


def _grade(text, rc):
    """`(passed, total, verdict, rc)` of a test log -- strand.sh's grading, byte for byte."""
    lines = text.splitlines()
    found = TEST_SUMMARY.findall(text)
    total, passed = (int(found[-1][0]), int(found[-1][1])) if found else (None, None)
    stress = _stress(lines)
    if stress:
        s_n, s_ran, s_red, passed, total = stress
    verdict = "GREEN" if rc == 0 else "RED"
    if stress:
        if s_red > 0 or s_ran < s_n:
            verdict = "RED (%d/%d iterations red" % (s_red, s_n)
            if s_ran < s_n:
                verdict += ", %d/%d ran" % (s_ran, s_n)
            verdict += ")"
            rc = rc or 100
        elif rc == 0:
            verdict = "GREEN (%d/%d iterations green)" % (s_n, s_n)
    # Zero tests prove nothing (GH #997); the texts count only off any other exit.
    refused = rc not in (0, 100, 101) and re.search(
        r"no tests matched|failed to parse filterset|didn't match any", text) is not None
    if rc in (6, 4) or total == 0 or (rc == 0 and total is None) or refused:
        verdict, rc = "RED (no tests matched)", 6
    return passed, total, verdict, rc


def _red_names(lines):
    """Every red test by name, once (GH #1050, #1086)."""
    names = []
    for line in lines:
        if not RED_LINE.match(line):
            continue
        line = re.sub(r"\[[^]]*\] ", "", line, count=1)
        line = re.sub(r"^ +", "  ", line, count=1)
        line = re.sub(r"^(  [A-Z-]+ )\[ *[0-9]+/[0-9]+\] \( *[0-9]+/[0-9]+\) ", r"\1", line, count=1)
        if line not in names:
            names.append(line)
    return names


def _lease_gate(db, repo, holder, host, out, err):
    """The lane of the test, or `_TestExit` 3: `--host` or the held one, bound to a held token."""
    def say(res):
        if res.out:
            print(res.out.rstrip("\n"), file=out)
        if res.err:
            print(res.err.rstrip("\n"), file=err)
    if not host and holder:
        host = schedule.lease_lane(db, holder).lane or ""
    # a run of `S` on the lane of its second lease `S:n` heartbeats and enters
    # its PID THERE (kit find "holder push", Welle Testing 10/12: it entered
    # none, and the lease fell away after its TTL under the running load)
    holder = schedule.lease_on_lane(db, holder, host) or holder if host else holder
    if not host:
        raise _TestExit(3, "strand: test runs on a lane -- take one ('scripts/strand.sh token take') "
                           "or pass --host <lane> with MECLAW_STRAND_TOKEN_SKIP=<reason>")
    skip = os.environ.get("MECLAW_STRAND_TOKEN_SKIP", "")
    # The lease key decides, not the branch (GH #1199 f, OR-V2.S2.2): a
    # detached tree with `--strand <wave>/<S>` was refused with exit 3 here
    # although the lease of `<wave>/<S>` was held -- the key is the branch on
    # a strand branch and `--strand` wherever it is given (`gitenv.strand_of`).
    if not skip and (not schedule._armed(db) or "/" not in (holder or "")):
        raise _TestExit(3, "strand: a test on a build host binds to a held token -- arm the lanes "
                           "and take one, or name the reason in MECLAW_STRAND_TOKEN_SKIP")
    res = schedule.lease_check(db, holder, pid=os.getpid(), lane=host, caller=os.getpid(), skip=skip)
    say(res)
    if res.code:
        raise _TestExit(res.code)
    return host


class TestProgress:
    """GH #1199 (kit find V2-A8): the pass lines of one `strand.sh test` -- one
    when the lane starts (`pass 0/n`), one per stress iteration as nextest
    prints it, one at the end of a run without iterations -- flushed onto the
    caller's streams and noted at the lease (`token who`). `clock` is the kit
    clock (epoch seconds)."""

    def __init__(self, out, holder, n, clock=time.time, err=None, lane=""):
        self.out, self.err, self.holder, self.n, self.clock = out, err, holder, max(int(n), 1), clock
        self.lane = lane
        self.done = self.red = 0

    def _say(self):
        line = progress.pass_line(self.done, self.n, self.done, self.red, now=self.clock())
        progress.emit(line, self.err, self.out)
        progress.beat(self.holder, line, now=self.clock(), lane=self.lane)

    def start(self):
        self._say()

    def line(self, text):
        if not STRESS_ITER.search(text):
            return
        m = re.search(r" iteration [0-9]+/([0-9]+):", text)
        if m:
            self.n = max(self.n, int(m.group(1)))
        self.done += 1
        if re.search(r", [1-9][0-9]* (failed|timed out|exec failed)", text):
            self.red += 1
        self._say()

    def end(self, rc):
        if self.done == 0:
            self.done, self.red = 1, int(rc != 0)
            self._say()


def _stress_count(extra):
    """`--stress-count N` (or `=N`) among the nextest words, else 1."""
    for i, w in enumerate(extra):
        v = w.split("=", 1)[1] if w.startswith("--stress-count=") else (
            extra[i + 1] if w == "--stress-count" and i + 1 < len(extra) else None)
        if v is not None and v.isdigit():
            return int(v)
    return 1


def _test(argv, out, err):
    expr, host, strand_in, wave_in, holder_n, extra = _parse_test(argv)
    repo = _git(os.getcwd(), "rev-parse", "--show-toplevel")
    root = _main_root(repo) if repo else ""
    if not root:
        raise _TestExit(2, "strand: not a git repository")
    plans = os.path.join(root, "plans")
    # The strand first (GH #1199 f): a detached tree without `--strand` hears
    # the way out, not "cannot tell the wave".
    who = strand_of(repo, strand_in, wave_in)
    if not who.name:
        raise _TestExit(2, "strand: " + who.why)
    wdir = _wave_dir(plans, wave_in, who.key, err)
    name = who.name
    archive_root = os.path.join(plans, wdir, "receipts", name)

    db = store.open()
    holder = holder_key(repo, strand_in, wave_in, holder_n)
    host = _lease_gate(db, repo, holder, host, out, err)
    try:
        ln = lane.find_lane(host)
        # The build path is this test's from the push to the end of the run:
        # a lane-side flock no token skip bypasses (W1 hand-over).
        held = lane.hold(ln, lane.paths(ln, name, True)[0])
    except lane.LaneError as exc:
        what = "refused -- the host would not test this tree:" if exc.code == 5 else "unreachable"
        raise _TestExit(2, "strand: lane %s %s\n  %s" % (host, what, exc))
    try:
        try:
            pushed = lane.push(ln, repo, name, True)
        except lane.LaneError as exc:
            what = "refused -- the host would not test this tree:" if exc.code == 5 else "unreachable"
            raise _TestExit(2, "strand: lane %s %s\n  %s" % (host, what, exc))
        if pushed.get("browser") == "none":
            print("strand: lane %s has no playwright for this tree -- its browser proofs SKIP (an npm "
                  "install in workshop/tools of one tree on the lane seeds its cache)" % host, file=out)

        # Its own directory, NOT a gate run: `test-` keeps it out of `latest`.
        sha = pushed.get("rev") or ""
        run_dir = os.path.join(archive_root, "test-%s-%s" % (
            datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"), sha[:8]))
        while os.path.exists(run_dir):
            run_dir += "-2"
        os.makedirs(run_dir)
        log = os.path.join(run_dir, "test.log")
        print("strand: lane %s (%s) rev %s overlay %s" % (host, ln.ssh.split("@", 1)[-1], sha[:8],
                                                           pushed.get("overlay_hash") or "?"),
              file=out, flush=True)
        # Kit find tv-s1-legacy-a2 (fix6): the caller reads THIS run's log --
        # the output below is only a red run's 20-line tail, and "the newest
        # log of the strand" is a parallel or an earlier run (flake_triage).
        print("strand: test log %s" % log, file=out, flush=True)

        t0 = int(time.time())
        prog = TestProgress(out, holder, _stress_count(extra), err=err, lane=host)
        prog.start()
        # The test on the lane is W1's `lane_sync.sh test` (one truth): the tier
        # in the build path with the lane's width and class, the guards, the
        # golden switches and their way back.
        if lane.partial(extra):
            # one package built: the next push must not take the tree for built
            try:
                lane.mark_partial(ln, pushed["target"])
            except lane.LaneError as exc:
                raise _TestExit(2, "strand: lane %s unreachable\n  %s" % (host, exc))
        rc = lane.run_test(ln, name, expr, extra, log, run=os.path.basename(run_dir),
                           on_line=prog.line)
        prog.end(rc)
        secs = int(time.time()) - t0
    finally:
        held.release()
    schedule.lease_check(db, holder, pid=0, caller=os.getpid())
    with open(log, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    if rc == 255 and not re.search(r"tests? run:", text):
        raise _TestExit(2, "strand: lane %s unreachable (lost during the test, log: %s)" % (host, log))
    refusal = [l[len("lane_sync: "):] for l in text.splitlines() if l.startswith("lane_sync: ")
               and "golden" not in l]
    if rc in (2, 5) and refusal and not re.search(r"tests? run:", text):
        raise _TestExit(2, "strand: lane %s refused the test:\n  %s" % (host, "\n  ".join(refusal)))
    passed, total, verdict, rc = _grade(text, rc)
    lines = text.splitlines()
    if rc != 0:
        for line in lines[-20:] + _red_names(lines):
            print(line, file=out)
    for line in lines:
        m = GOLDEN_LINE.match(line)
        if m:
            print("strand: " + m.group(1), file=out)
    final = "TEST [%s] %s/%s %ss %s" % (expr, passed or 0, total or 0, secs, verdict)
    # Re-check tv-bn M7c M1: an opt-out of the browser switch (lane_sync.sh's
    # NOTE in this run's log) stood only in the raw log -- a driver SKIP read
    # as a full proof in the verdict line. The line names it.
    mark = lane.opt_out_in(text)
    if mark:
        final += " " + mark
    print(final, file=out, flush=True)
    with open(log, "a", encoding="utf-8") as fh:
        fh.write(final + "\n")
    return rc


def run_test(args, out, err):
    """`pipeline.py test` in-process: `args` is the argument list after `test`."""
    try:
        return _test(args, out, err)
    except _TestExit as ex:
        if str(ex):
            print(str(ex), file=out if ex.code == 0 else err)
        if ex.code != 0:
            # GH #1199 (V2-A8): a refused or lost test says so on both streams
            # and at the lease -- a wrapper that keeps one stream sees it.
            first = (str(ex).splitlines() or ["exit %d" % ex.code])[0]
            progress.refuse("pipeline test", first, out=out, err=err, holder=_test_holder(args),
                            code=ex.code, word="REFUSED (exit %d)" % ex.code)
        return ex.code


def _test_holder(args):
    """The lease key of a `pipeline.py test` call, "" when it cannot be told."""
    try:
        _, _, strand_in, wave_in, holder_n, _ = _parse_test(args)
        repo = _git(os.getcwd(), "rev-parse", "--show-toplevel")
        return holder_key(repo, strand_in, wave_in, holder_n) if repo else ""
    except (_TestExit, OSError, subprocess.SubprocessError):
        return ""


def _cmd_test(args):
    return run_test(args.argv, sys.stdout, sys.stderr)


def register(sub):
    # No option of its own: every word after `gate` is gate.sh's command line, `-h` included.
    p = sub.add_parser("gate", help="plan and run one pass of the gate", add_help=False,
                       prefix_chars="\x01", description=__doc__.splitlines()[0])
    p.add_argument("argv", nargs="*")
    p.set_defaults(func=_cmd_gate)
    # The same for `test`: every word after it is strand.sh test's command line.
    t = sub.add_parser("test", help="run one nextest filterset on the held lane", add_help=False,
                       prefix_chars="\x01", description=TEST_USAGE.splitlines()[0])
    t.add_argument("argv", nargs="*")
    t.set_defaults(func=_cmd_test)
