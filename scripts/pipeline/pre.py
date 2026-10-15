#!/usr/bin/python3 -I
"""meclaw -- `pre`, the builder's pre-run before a strand gate.

WHY THIS EXISTS
===============
Thirty-six expensive red strand runs in five days (7.0 h of lane time), and
two of the five red passes of 2026-10-08 would have been red in seconds: a
pin a bump left behind and a test line that did not compile. Both are cheap
to find and expensive to find late. Verification belongs in the builder's own
loop, in front of the pipeline -- and as a duty with a stamp, because an
agent skips a step that leaves no trace.

TWO HALVES, <= 120 s together (p90)
===================================
LOCAL (this machine, no cargo, <= 15 s): every check of `scripts/precheck.py`
(the same functions the gate station `precheck` runs), the locks of
`scripts/pipeline/locks.py`, the dry version cascade (`cascade_dry.py`,
GH #1173), `hygiene.py check --local` where it exists,
`shellcheck` on touched `.sh`. Beside it, in parallel, the unit tests of the
CHANGED `scripts/tests/test_*.py` modules (only those -- `test_gate_sh` and
`test_strand_sh` alone take minutes), through `selftest_shard.py` like
gate-selftest: side by side, at most 4 shards next to the live egon, a heavy
module split by TestCase class, RED unless every shard says OK (#1160).

LANE (the held lane, warm, <= 100 s), only for a Rust diff: `cargo check
--tests -p <crates>` -- a compile error in a test in seconds -- and the test
binaries of the changed test files with cargo targets `-p <crate> --test
<stem>`, never a bare filterset (`nextest run -E` alone builds every test
binary of the workspace). Without a lane the half is skipped with a NOTE and
the stamp says `lane_half: false`.

THE STAMP
=========
`<git-dir>/pipeline-pre.ok` = `{tree_hash, at, verdict, lane_half, reasons}`
and `pre.json` beside it. `tree_hash` = sha256 over HEAD and the overlay
(every tracked change against HEAD, every untracked file that is not
ignored). `strand.sh gate` in mode strand from a strand worktree refuses
without a GREEN stamp of the same tree younger than 24 h.

LINES
=====
    PRE <check> <secs>s GREEN|RED|NOTE [reason]
    PRE-SUMMARY <tree_hash8> <green>/<total> <secs>s GREEN|RED

Every RED line names `path:line` (where the check has one) and the fix in
one sentence. Exit 0 GREEN, 1 RED, 2 usage.

STORE AND METRIC
================
A pre-run is a workpackage `mode: pre` in the W0 store (`store.py`, path
`$MECLAW_PIPELINE_DB`): one task per check with its `task_runs` row, and the
journal events of a pass (`pass_planned`, `run_dispatched`, `station_ended`
per check, `pass_ended`), so `pipeline receipt <run>` reads it like any other
pass. The metric `pipe_pre {repo, strand, verdict} secs` (V0 section 8) goes
through the W0 emitter (`metrics.py`: `url`, `line`, `post`); a failed push
is the event `metrics_failed`, without an emitter a NOTE says so. Neither
ever blocks a pre-run.
"""

import argparse
import concurrent.futures
import datetime
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.parse
import uuid

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCRIPTS = os.path.dirname(_HERE)
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

from pipeline import cascade_dry, events, gitenv, locks, store  # noqa: E402
import precheck  # noqa: E402

REPO_ROOT = os.path.dirname(_SCRIPTS)
STAMP = "pipeline-pre.ok"
STAMP_TTL_S = 24 * 3600
RUST_CLASSES = frozenset({"rust_src", "rust_test", "crate_file", "workspace"})


# --- the tree ---------------------------------------------------------------

def _git(repo, *args, check=False):
    # `-C repo` names the tree; an inherited GIT_DIR would name another one
    # (re-check tb-s2 h2 m4: `git_dir` gave the caller's log directory).
    res = subprocess.run(["git", "-C", repo] + list(args),
                         capture_output=True, text=True, env=gitenv.without_repo_env(os.environ))
    if check and res.returncode != 0:
        raise RuntimeError("git %s: %s" % (" ".join(args), res.stderr.strip()))
    return res.stdout if res.returncode == 0 else ""


def overlay_paths(repo):
    """Tracked changes against HEAD (deletions included) plus untracked files."""
    paths = set(_git(repo, "diff", "--no-renames", "--name-only", "HEAD").split("\n"))
    paths |= set(_git(repo, "ls-files", "-o", "--exclude-standard").split("\n"))
    return sorted(p for p in paths if p)


def tree_hash(repo, head=None):
    """sha256 over HEAD (or `head`) and the overlay -- the tree a gate would see."""
    h = hashlib.sha256()
    head = head or _git(repo, "rev-parse", "HEAD").strip()
    h.update(("HEAD %s\n" % head).encode())
    for rel in overlay_paths(repo):
        path = os.path.join(repo, rel)
        if os.path.islink(path):
            digest = "link:" + os.readlink(path)
        elif os.path.isfile(path):
            with open(path, "rb") as fh:
                digest = hashlib.sha256(fh.read()).hexdigest()
        else:
            digest = "deleted"
        h.update(("%s %s\n" % (rel, digest)).encode())
    return h.hexdigest()


def diff_paths(repo, base):
    """What a strand gate reads: the diff against the merge base, dirty tree included."""
    paths = set()
    if base:
        paths |= set(_git(repo, "diff", "--no-renames", "--name-only", base).split("\n"))
    paths |= set(overlay_paths(repo))
    return sorted(p for p in paths if p)


def git_dir(repo):
    d = _git(repo, "rev-parse", "--git-dir").strip()
    return d if os.path.isabs(d) else os.path.join(repo, d)


# --- the plan ---------------------------------------------------------------

def plan(paths, repo):
    """`{classes, crates, lane_half, unittests, test_targets}` of a diff."""
    import gate_plan
    classes = set(gate_plan.classify(paths))
    try:
        crates = sorted(gate_plan.crates_of(paths, repo))
    except gate_plan.GatePlanError:
        crates = []
    lane_half = bool(classes & RUST_CLASSES)
    unittests = []
    if "gate_infra" in classes or any(p.startswith("scripts/") for p in paths):
        for p in paths:
            base = os.path.basename(p)
            if (p.startswith("scripts/tests/") and base.startswith("test_")
                    and p.endswith(".py") and p.count("/") == 2
                    and os.path.isfile(os.path.join(repo, p))):
                unittests.append("scripts.tests." + base[:-3])
    targets = []
    for p in paths:
        parts = p.split("/")
        if (len(parts) == 4 and parts[0] == "crates" and parts[2] == "tests"
                and parts[3].endswith(".rs") and os.path.isfile(os.path.join(repo, p))):
            targets.append((parts[1], parts[3][:-3]))
        elif (len(parts) == 5 and parts[0] == "crates" and parts[2] == "tests"
              and parts[4] == "main.rs"):
            targets.append((parts[1], parts[3]))
    return {"classes": sorted(classes), "crates": crates, "lane_half": lane_half,
            "unittests": sorted(set(unittests)), "test_targets": sorted(set(targets))}


def lane_commands(p, repo=None):
    """The cargo argv lists of the lane half: one check, one run per changed test target.

    With `repo`, each call carries the test-hook feature the gate's build
    station gives the same selection (GH #1208, `executor.test_features`):
    without it `check --tests` skips the `#![cfg(feature = "test-hooks")]`
    tests and the run builds a second variant of meclaw-colony. The rule is
    `meclaw_test_features` of scripts/cargo-target.sh, asked here directly:
    pre runs without the executor module (its fixtures copy pre alone)."""
    cmds = _lane_commands(p)
    if repo is None:
        return cmds
    return [c + _test_features(repo, c[1:]) for c in cmds]


def _test_features(repo, argv):
    lib = os.path.join(repo, "scripts", "cargo-target.sh")
    if not os.path.isfile(lib):
        lib = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                           "cargo-target.sh")
    res = subprocess.run(
        ["bash", "-c", '. "$1" || exit 2; shift; meclaw_test_features "$@"', "_", lib, repo] + list(argv),
        capture_output=True, text=True, timeout=30)
    if res.returncode:
        raise RuntimeError("meclaw_test_features failed (exit %d): %s"
                           % (res.returncode, res.stderr.strip()))
    return [ln for ln in res.stdout.splitlines() if ln]


def _lane_commands(p):
    if not p["lane_half"]:
        return []
    pkgs = []
    for c in p["crates"]:
        pkgs += ["-p", c]
    cmds = [["cargo", "check", "--tests"] + (pkgs or ["--workspace"])]
    by_crate = {}
    for crate, stem in p["test_targets"]:
        by_crate.setdefault(crate, []).append(stem)
    for crate in sorted(by_crate):
        # `--no-tests=warn`: an all-ignored browser lock runs zero tests
        # here (its station runs it with `--run-ignored all`); no source
        # heuristic guesses which files those are (review W7 I2).
        argv = ["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn", "-p", crate]
        for stem in sorted(by_crate[crate]):
            argv += ["--test", stem]
        cmds.append(argv)
    return cmds


# --- running ----------------------------------------------------------------

class Result:
    def __init__(self, check, secs, verdict, text="", tests=None, seed=None):
        self.check, self.secs, self.verdict, self.text = check, secs, verdict, text
        # Kit find C4 (Welle Testing): the red unit tests and the seed of their
        # order, for the store row (`station_ended`), not only for the console.
        self.tests, self.seed = list(tests or []), seed

    def line(self):
        tail = " " + self.text if self.text else ""
        return "PRE %s %ds %s%s" % (self.check, round(self.secs), self.verdict, tail)

    def as_json(self):
        out = {"check": self.check, "secs": round(self.secs, 3),
               "verdict": self.verdict, "text": self.text}
        if self.tests:
            out["tests"] = self.tests
        if self.seed is not None:
            out["seed"] = self.seed
        return out


def _from_findings(check, secs, findings):
    if not findings:
        return [Result(check, secs, "GREEN")]
    return [Result(check, secs, f.verdict, locks.render(f)) for f in findings]


def shellcheck_first(text):
    """The first finding of `shellcheck -f gcc` as `path:line:col level: message [SCnnnn]`."""
    for l in text.splitlines():
        parts = l.split(":", 4)
        if len(parts) == 5 and parts[1].isdigit() and parts[2].isdigit():
            body = parts[4].strip()
            msg, sep, code = body.rpartition(" [SC")
            if sep:
                body = "%s [SC%s" % (msg.rstrip("."), code)
            return "%s:%s:%s %s: %s" % (parts[0], parts[1], parts[2], parts[3].strip(), body)
    return (text.strip().splitlines() or ["shellcheck failed"])[0][:200]


def local_half(paths, repo, ctx):
    out = []
    for fn in precheck.NATIVE_CHECKS:
        t0 = time.monotonic()
        found = list(fn(paths, repo))
        secs = time.monotonic() - t0
        name = fn.__name__[len("check_"):].replace("_", "-")
        if not found:
            out.append(Result(name, secs, "GREEN"))
        for f in found:
            out.append(Result(f.check, secs, f.level, f.message))
    for name, fn in locks.LOCKS:
        t0 = time.monotonic()
        found = fn(paths, repo, ctx)
        out.extend(_from_findings(name, time.monotonic() - t0, found))
    # GH #1173: the merge point's cascade, dry -- a pin it cannot raise or a
    # version literal beside a template path failed 7 tests of tb-s8's queue
    # candidate with this pre-run green.
    t0 = time.monotonic()
    found, skipped = cascade_dry.check(paths, repo, ctx)
    out.extend([Result(cascade_dry.CHECK, time.monotonic() - t0, "GREEN", skipped)] if skipped
               else _from_findings(cascade_dry.CHECK, time.monotonic() - t0, found))
    sh = [p for p in paths if p.endswith(".sh") and os.path.isfile(os.path.join(repo, p))]
    if sh and shutil.which("shellcheck"):
        t0 = time.monotonic()
        res = subprocess.run(["shellcheck", "-S", "warning", "-f", "gcc"] + sh, cwd=repo,
                             capture_output=True, text=True)
        verdict = "GREEN" if res.returncode == 0 else "RED"
        out.append(Result("shellcheck", time.monotonic() - t0, verdict,
                          "" if verdict == "GREEN" else
                          "%s -- run `shellcheck %s`" % (shellcheck_first(res.stdout + res.stderr),
                                                         " ".join(sh))))
    hyg = os.path.join(repo, "scripts", "pipeline", "hygiene.py")
    if os.path.isfile(hyg):
        t0 = time.monotonic()
        from pipeline import lane
        res = subprocess.run(lane.python_argv(hyg, "check", "--local"), cwd=repo,
                             capture_output=True, text=True)
        out.append(Result("hygiene", time.monotonic() - t0,
                          "GREEN" if res.returncode == 0 else "RED",
                          "" if res.returncode == 0 else
                          (res.stdout.strip().splitlines() or ["exit %d" % res.returncode])[-1]))
    return out


# pre runs on mm-os01 next to the live egon: never wider than this (#1160).
UNITTEST_SHARDS_MAX = 4
# The bound of the unit-test half (re-check tb-s2 h2 m5: a hung test held pre
# without end): a floor plus three times the serial seconds of the modules in
# the duration table (an unknown module counts UNITTEST_UNKNOWN_S). Far above
# a run under load -- the kit cases measured 1.6-1.7x serial at width 4.
UNITTEST_TIMEOUT_FLOOR_S = 300
UNITTEST_TIMEOUT_FACTOR = 3
UNITTEST_UNKNOWN_S = 60
# One log directory per run under <git-dir>/pre-unittest (re-check tb-s2 h2
# m4: two runs wrote the same shard-k.log); older than this they go.
UNITTEST_LOG_KEEP_S = 24 * 3600


def _nproc():
    from pipeline import selftest_shard
    return selftest_shard._nproc()


def unittest_width(env=None, nproc=None):
    """Shards of the unit-test half: the selftest width, at most 4 and nproc.
    A malformed `MECLAW_SELFTEST_SHARDS` falls back to the default width."""
    from pipeline import plan as shard_plan
    env = os.environ if env is None else env
    nproc = _nproc() if nproc is None else nproc
    try:
        n = shard_plan.selftest_shards(nproc, env)
    except ValueError:
        n = shard_plan.selftest_shards(nproc, {})
    return max(1, min(n, UNITTEST_SHARDS_MAX, nproc))


def unittest_timeout(modules):
    """Seconds the unit-test half may take (see UNITTEST_TIMEOUT_FLOOR_S)."""
    from pipeline import selftest_shard
    table = selftest_shard.load_durations(selftest_shard.DURATIONS)
    return UNITTEST_TIMEOUT_FLOOR_S + UNITTEST_TIMEOUT_FACTOR * sum(
        table.get(m, UNITTEST_UNKNOWN_S) for m in set(modules))


def unittest_logs(repo):
    """A fresh log directory of this run; run directories older than a day go."""
    base = os.path.join(git_dir(repo), "pre-unittest")
    os.makedirs(base, exist_ok=True)
    now = time.time()
    for name in os.listdir(base):
        path = os.path.join(base, name)
        try:
            old = name.startswith("run-") and now - os.path.getmtime(path) > UNITTEST_LOG_KEEP_S
        except OSError:
            continue
        if old:
            shutil.rmtree(path, ignore_errors=True)
    return tempfile.mkdtemp(prefix="run-", dir=base)


def unittest_rerun(modules, seed, width):
    """The command that runs `modules` again (re-check tv-kit fix15 M1): with a
    seed the runner itself, seed AND width -- bare unittest reads neither, and
    the width decides which shard a unit lands in; without one plain unittest.
    Both in the kit's Python (`lane.PYTHON`, M7p): a bare `python3` is refused
    by every test that reaches the narrow point in its own process."""
    from pipeline import lane
    py = " ".join(lane.PYTHON)
    if seed is None:
        return "run `%s scripts/pipeline/hang_dump.py -m unittest %s`" % (py, " ".join(modules))
    from pipeline import plan as shard_plan, selftest_shard
    return "replay: `%s=%d %s=%d %s scripts/pipeline/selftest_shard.py unittest %s`" % (
        selftest_shard.SEED_ENV, seed, shard_plan.SELFTEST_SHARDS_ENV, width, py, " ".join(modules))


def _seed_of(out):
    """The runner's `SELFTEST-SEED <n>` in `out` (str, or bytes of a timeout)."""
    if isinstance(out, bytes):
        out = out.decode("utf-8", "replace")
    seeds = re.findall(r"^SELFTEST-SEED (\d+)$", out or "", re.M)
    return int(seeds[0]) if seeds else None


def unittest_half(modules, repo):
    """The changed script tests through the selftest shard (#1160): the same
    modules side by side, one line, fail-closed (`selftest_shard.judge`)."""
    if not modules:
        return []
    from pipeline import flakes, plan as shard_plan, selftest_shard
    t0 = time.monotonic()
    width = unittest_width()
    logs = unittest_logs(repo)
    limit = unittest_timeout(modules)
    # Never the caller's repository, and the repository stays as it was:
    # config, commit and branch of the tree (pd-1155, re-check pd-legacy m-3).
    # The guard sits here, around the whole shard run: the one-shard path of
    # the runner is a single child as well (re-check tb-s2 h2 M1).
    snap = gitenv.snapshot(repo)
    env = gitenv.without_repo_env(os.environ)
    env.update({shard_plan.SELFTEST_SHARDS_ENV: str(width), selftest_shard.LOG_DIR_ENV: logs})
    try:
        from pipeline import lane
        res = subprocess.run(lane.python_argv(os.path.join(_HERE, "selftest_shard.py"), "unittest")
                             + modules, cwd=repo, capture_output=True, text=True, env=env,
                             timeout=limit)
    except subprocess.TimeoutExpired as exc:
        # The runner got SIGKILL; its shards and its single child die with it
        # (PR_SET_PDEATHSIG). RED, and the guard still speaks. The seed was
        # printed (flushed) before the hang: an order-bound hang replays too
        # (re-check tv-kit fix15 H2).
        drift, note = gitenv.drift(snap)
        seed = _seed_of(exc.stdout)
        out = [Result("unittest", time.monotonic() - t0, "RED",
                      "%s timeout after %ds -- %s%s"
                      % (" ".join(modules), limit, unittest_rerun(modules, seed, width),
                         " -- %s" % drift if drift else ""), seed=seed)]
        if note:
            out.append(Result("repo-config", 0.0, "NOTE", note))
        return out
    except BaseException:
        # An interrupted half still names what it changed (re-check m-2).
        drift, _ = gitenv.drift(snap)
        if drift:
            print("PRE unittest RED -- %s" % drift, file=sys.stderr)
        raise
    text = (res.stdout or "") + "\n" + (res.stderr or "")
    verdict, _ran, _failed, reason = selftest_shard.judge(res.returncode, text)
    ran = [int(m) for m in re.findall(r"^SHARD \d+/(\d+) ", res.stdout or "", re.M)]
    shards = ran[0] if ran else 1
    why = re.findall(r"^SELFTEST-VERDICT RED (.*)$", res.stdout or "", re.M)
    drift, note = gitenv.drift(snap)
    # Kit find C4 (Welle Testing, tv-bn-layout-c run-d6651526d325): a red
    # unittest left `station_ended verdict=RED` in the store and nothing else;
    # the red test came back only from the shard log (24 h under the git dir),
    # the seed of the order not at all. Both are read here, from the runner's
    # output (one shard prints no verdict line of its own), and go into the
    # line and the store row.
    tests, seed = [], None
    if verdict != "GREEN":
        tests = sorted(flakes.parse_unittest_output(text)["failed"])
        seed = _seed_of(res.stdout)
    detail = why[-1] if why else reason
    if tests and " -- red " not in detail:
        detail += " -- red %s" % ",".join(tests)
    if seed is not None and "; seed %d" % seed not in detail:
        detail += "; seed %d" % seed
    hint = "" if verdict == "GREEN" else " %s -- %s" % (detail, unittest_rerun(modules, seed, width))
    out = [Result("unittest", time.monotonic() - t0, "RED" if drift else verdict,
                  "%s shards=%d%s%s" % (" ".join(modules), shards, hint,
                                        " -- %s" % drift if drift else ""),
                  tests=tests, seed=seed)]
    if note:
        out.append(Result("repo-config", 0.0, "NOTE", note))
    # The runner's own NOTE on its measured shard seconds (a stale duration
    # table), one line; never the verdict (re-check tb-s2 h2 m-N8).
    for text in re.findall(r"^SELFTEST-NOTE (shard imbalance .*)$", res.stdout or "", re.M):
        out.append(Result("unittest-balance", 0.0, "NOTE", text))
    return out


def ssh_runner(lane_target, strand, repo, exec_=subprocess.run, popen=subprocess.Popen):
    """Push the tree onto the lane once, then run each cargo step there THROUGH THE KIT.

    Everything lane-side is `scripts/lane_sync.sh` (W1): the push checks the
    commit out in the lane's build path, `check` runs `cargo check` there
    (build path, floor, the lane's environment, the ghost-binary guard), and
    `test` runs the test binaries (`test-tier.sh filter` on the host). pre
    knows no lane path of its own (review I2); the lane's settings come from
    `strand.sh pre` (`lane_settings`) in the environment."""
    state = {"pushed": False, "hold": None, "env": None}
    sync = os.path.join(repo, "scripts", "lane_sync.sh")

    def take():
        # Delta C I-neu-1: the gate's lock on the build path (`<tree>.lock`,
        # `lane.hold`) from the push to the last step -- no token skip
        # bypasses it, a held path is exit 5 before anything travels.
        # Own process group (review Z-A R3 M1): the kill after the deadline
        # takes the ssh child of `lane_sync.sh` along, not only the shell.
        proc = popen([sync, "hold", lane_target, strand], cwd=repo, stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                     start_new_session=True)
        word = proc.stdout.readline().split()
        if word[:1] == ["held"] and len(word) == 2:
            state["hold"] = proc
            state["env"] = dict(os.environ, MECLAW_LANE_HOLD=word[1])
            return 0, ""
        proc.stdin.close()
        err = proc.stderr.read()
        proc.wait()
        proc.stdout.close()
        proc.stderr.close()
        if word[:1] == ["busy"]:
            return 5, ("lane push refused: the build path of %s on %s is in use "
                       "(a gate holds its lock) -- wait for it, then `strand.sh pre` again"
                       % (strand, lane_target))
        return proc.returncode or 2, "lane hold failed: " + err.strip()[-200:]

    def release():
        proc, state["hold"] = state["hold"], None
        if proc is not None:
            proc.stdin.close()
            # Z5: never unbounded -- a lane that keeps the ssh open would keep
            # `strand.sh pre` with it (same deadline as `lane.Hold.release`).
            try:
                proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except OSError:
                    proc.kill()
                proc.wait()
                print("pre: lane hold of %s on %s: hold not released in 30 s, killed"
                      % (strand, lane_target), file=sys.stderr)
            proc.stdout.close()
            proc.stderr.close()

    def run(argv):
        if not state["pushed"]:
            rc, text = take()
            if rc:
                return rc, text
            res = exec_([sync, "push", lane_target, strand], cwd=repo, env=state["env"],
                        capture_output=True, text=True)
            if res.returncode != 0:
                return res.returncode, "lane push failed: " + res.stderr.strip()[-200:]
            state["pushed"] = True
        if argv[1] == "nextest":
            pairs, crate, opts, i = [], None, [], 3
            while i < len(argv):
                if argv[i] == "-p":
                    crate = argv[i + 1]
                    i += 1
                elif argv[i] == "--test":
                    if crate is None:
                        pairs = []
                        break
                    pairs.append((crate, argv[i + 1]))
                    i += 1
                else:
                    opts.append(argv[i])
                i += 1
            if not pairs:
                # an empty filterset would run nothing -- or everything
                return 2, "error: no (-p, --test) pair in %s" % " ".join(argv)
            expr = " | ".join("binary_id(=%s::%s)" % cs for cs in pairs)
            cmd = [sync, "test", lane_target, strand, expr] + opts
            for _, stem in pairs:
                cmd += ["--test", stem]
            res = exec_(cmd, cwd=repo, env=state["env"], capture_output=True, text=True)
            return res.returncode, res.stdout + res.stderr
        res = exec_([sync, "check", lane_target, strand] + list(argv[2:]), cwd=repo,
                    env=dict(state["env"], MECLAW_LANE_EXPECTED_G=str(CHECK_EXPECTED_G)),
                    capture_output=True, text=True)
        return res.returncode, res.stdout + res.stderr
    run.release = release
    return run


def _first_error(text):
    for l in text.splitlines():
        if l.startswith("error"):
            idx = text.splitlines().index(l)
            loc = next((m.strip() for m in text.splitlines()[idx:idx + 4]
                        if m.strip().startswith("-->")), "")
            return ("%s %s" % (loc[3:].strip(), l)).strip()
    return (text.strip().splitlines() or [""])[-1][:200]


# What the lane hygiene reserves for pre's `cargo check` (W4 delta): rmeta
# and check units only, not the 35 G of a cold target (schedule.DEFAULT_EXPECTED_G)
# the check inherited before.
CHECK_EXPECTED_G = 10

# lane_sync.sh's refusal of a gate step on a class egon lane (exit 2).
EGON_REFUSAL = "a class egon lane runs 'test' only"


def _egon_refused(text):
    """lane_sync's own refusal line -- never the text quoted elsewhere (review Zb M1)."""
    return any(l.startswith("lane_sync: " + EGON_REFUSAL) for l in text.splitlines())


# test-tier.sh exits 6 with `no tests matched` when a filter ran no test.
NO_TESTS_EXIT = 6


def lane_half(cmds, runner):
    try:
        return _lane_steps(cmds, runner)
    finally:
        release = getattr(runner, "release", None)
        if release is not None:
            release()  # the build path's lock (Delta C I-neu-1)


def _opt_out_in(text):
    """`lane.opt_out_in`, imported when a lane half ran: a stamp check (`--stamp-check`)
    runs in trees that carry only part of `pipeline/` (test_pipeline_pre Gate)."""
    from pipeline import lane
    return lane.opt_out_in(text)


def _lane_steps(cmds, runner):
    out = []
    for argv in cmds:
        t0 = time.monotonic()
        rc, text = runner(argv)
        name = "cargo-check" if argv[1] == "check" else "test-binaries"
        # Re-check tv-bn M7c M1: `lane_sync.sh test` requires the browsers
        # unless the caller opted out; the opt-out stood only in its text.
        mark = _opt_out_in(text) if name == "test-binaries" else ""
        if rc == 0:
            out.append(Result(name, time.monotonic() - t0, "GREEN",
                              " ".join(argv[2:] + ([mark] if mark else []))))
        elif name == "cargo-check" and rc == 2 and _egon_refused(text):
            # W4 re-check: the lane refuses a check by its class -- no verdict
            # about the tree; the gate's lane runs check and tests.
            out.append(Result(name, time.monotonic() - t0, "NOTE",
                              "the lane is class egon (test only) -- the gate's lane checks"))
            break
        elif name == "test-binaries" and rc == NO_TESTS_EXIT and "no tests matched" in text:
            out.append(Result(name, time.monotonic() - t0, "NOTE",
                              "zero tests ran (all ignored?) -- the gate station runs them"))
        else:
            out.append(Result(name, time.monotonic() - t0, "RED",
                              "%s -- fix it, then `strand.sh pre` again" % _first_error(text)))
            if name == "cargo-check":
                break
    return out


# --- stamp, store, summary --------------------------------------------------

def _now():
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)


def write_stamp(repo, doc):
    d = git_dir(repo)
    with open(os.path.join(d, STAMP), "w", encoding="utf-8") as fh:
        keep = {k: doc[k] for k in ("tree_hash", "at", "verdict", "lane_half", "reasons")}
        # GH #1167: HEAD and the overlay digests let a later check tell a
        # doc-only change (the report head) from a code change.
        keep.update({k: doc[k] for k in ("head", "overlay") if k in doc})
        json.dump(keep, fh, indent=1, sort_keys=True)
        fh.write("\n")
    with open(os.path.join(d, "pre.json"), "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=1, sort_keys=True)
        fh.write("\n")
    return os.path.join(d, STAMP)


def read_stamp(repo):
    try:
        with open(os.path.join(git_dir(repo), STAMP), encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def stamp_status(repo, now=None, head=None):
    """`(ok, line)` -- what `strand.sh gate` asks before a strand gate."""
    st = read_stamp(repo)
    if not st:
        return False, "gate: run strand.sh pre first (no stamp)"
    at = st.get("at", "?")
    try:
        when = datetime.datetime.fromisoformat(at.replace("Z", "+00:00"))
    except ValueError:
        return False, "gate: run strand.sh pre first (stamp unreadable)"
    age = ((now or _now()) - when).total_seconds()
    note = ""
    if st.get("tree_hash") != tree_hash(repo, head):
        ok, changed = _code_equal(repo, st)
        if not ok:
            from pipeline import turnaround
            return False, "gate: run strand.sh pre first (tree changed since %s: %s)" % (
                at, turnaround.describe(changed))
        note = " (code tree unchanged, doc-only changes since)"
    if st.get("verdict") != "GREEN":
        return False, "gate: run strand.sh pre first (last pre at %s was %s)" % (at, st.get("verdict"))
    if age > STAMP_TTL_S:
        return False, "gate: run strand.sh pre first (stamp from %s is older than 24 h)" % at
    note += "" if st.get("lane_half") else " (lane half skipped)"
    return True, "gate: pre stamp %s GREEN at %s%s" % (st["tree_hash"][:8], at, note)


def _code_equal(repo, st):
    """GH #1167: is the worktree code-equal to the stamped tree?"""
    from pipeline import turnaround
    paths = overlay_paths(repo)
    return turnaround.code_equal_stamp(repo, st, _git(repo, "rev-parse", "HEAD").strip(),
                                       turnaround.overlay_digests(repo, paths),
                                       turnaround.branch_wave(repo))


def turnaround_checks(repo, paths):
    """GH #1167 (e) and (f): the merge's later refusals, found before the gate."""
    from pipeline import turnaround
    out = []
    for check, fn in (("main-tree", lambda: turnaround.main_tree_check(repo, paths)),
                      ("owner", lambda: turnaround.owner_check(repo))):
        t0 = time.monotonic()
        verdict, why = fn()
        out.append(Result(check, time.monotonic() - t0, verdict, why))
    return out


REPO = "meclaw"


def _by_check(checks):
    """One verdict and one duration per check: RED wins, a check of NOTEs is NOTE."""
    out = {}
    for c in checks:
        cur = out.setdefault(c["check"], {"verdicts": [], "secs": 0.0, "red": [], "tests": [],
                                          "seed": None})
        cur["verdicts"].append(c["verdict"])
        cur["secs"] += c["secs"]
        if c["verdict"] == "RED":
            cur["red"].append(c.get("text") or "")
            cur["tests"] += c.get("tests") or []
            if c.get("seed") is not None:
                cur["seed"] = c["seed"]
    for cur in out.values():
        v = cur.pop("verdicts")
        cur["verdict"] = "RED" if "RED" in v else ("NOTE" if set(v) == {"NOTE"} else "GREEN")
        cur["secs"] = round(cur["secs"], 3)
    return out


LANE_CHECKS = ("cargo-check", "test-binaries")


def record_pre(db, doc, repo):
    """The pre-run as a workpackage `mode: pre` through the W0 store API; returns its run.

    One transaction (W4 re-check): a break halfway left the workpackage
    `active` without `pass_ended`; now the pre-run is in the store whole or
    not at all."""
    with store.tx(db):
        return _record_pre(db, doc, repo)


def _record_pre(db, doc, repo):
    run = "run-%s" % uuid.uuid4().hex[:12]
    at = doc["at"]
    checks = _by_check(doc["checks"])
    runner = doc.get("lane") or "local"
    rev = _git(repo, "rev-parse", "HEAD").strip()
    dirty = bool(overlay_paths(repo))
    wp_doc = {"repo": REPO, "mode": "pre", "stage": "pre", "strand": doc.get("strand"),
              "wave": None, "rev": rev, "base": doc["base"], "tree": doc["tree_hash"],
              "dirty": dirty, "lane": doc.get("lane"), "archive": None,
              "plan": sorted(checks), "owner_decisions": [], "plan_reason": {},
              "run": run}
    wp = store.item_create(db, "workpackage", "pre %s %s" % (doc.get("strand") or "-",
                                                             doc["tree_hash"][:8]),
                           doc=wp_doc, at=at)
    ver = store.item_transition(db, wp["id"], "planned", "active", wp["version"], at=at)
    events.emit(db, "pass_planned", "status",
                {"run": run, "mode": "pre", "repo": REPO, "rev": rev, "base": doc["base"],
                 "stations": sorted(checks), "reason": {}, "cache_known": 0, "dirty": dirty},
                run=run, item=wp["id"], at=at)
    events.emit(db, "run_dispatched", "note",
                {"run": run, "runner": runner, "tree": doc["tree_hash"], "attempt": 1,
                 "round": 1, "deadline_at": None, "ws": None},
                run=run, item=wp["id"], at=at)
    counts, lane_s = {}, 0.0
    for name in sorted(checks):
        c = checks[name]
        task = store.item_create(db, "task", name, parent=wp["id"], at=at)
        tr = store.run_open(db, task["id"], runner, key=task["key"], at=at)
        state = store.run_close(db, tr, c["verdict"], run_s=c["secs"], at=at)
        tver = store.item_transition(db, task["id"], "open", "in_progress", task["version"], at=at)
        store.item_transition(db, task["id"], "in_progress", store.TASK_AFTER_RUN[state], tver,
                              reason=None if state == "done" else "red", at=at)
        scope = runner if name in LANE_CHECKS else "local"
        if name in LANE_CHECKS:
            lane_s += c["secs"]
        ended = {"run": run, "station": name, "verdict": c["verdict"], "secs": c["secs"],
                 "exit": 1 if c["verdict"] == "RED" else 0, "scope": scope}
        # Kit find C4: a red station keeps why (the gate's `reason`), and a
        # red unittest its tests and seed.
        if c["red"]:
            ended["reason"] = "; ".join(t for t in c["red"] if t)
        if name == "unittest" and c["verdict"] == "RED" and not c["tests"]:
            # No red test named: marked like the gate's C1, never "unknown" (fix15 H1).
            from pipeline import flakes
            ended["reason"] = "%s: %s" % (flakes.UNEXPLAINED, ended.get("reason", ""))
        if c["tests"]:  # names; the receipt's `tests` is a count dict (fix15 H3)
            ended["red_tests"] = sorted(set(c["tests"]))
        if c["seed"] is not None:
            ended["seed"] = c["seed"]
        events.emit(db, "station_ended", "status", ended, run=run, item=task["id"], at=at)
        counts[c["verdict"].lower()] = counts.get(c["verdict"].lower(), 0) + 1
    events.emit(db, "pass_ended", "status",
                {"run": run, "verdict": doc["verdict"], "secs": doc["secs"],
                 "lane_seconds": round(lane_s, 3), "critical_path_s": doc["secs"],
                 "rounds": 1, "cache_hits": 0, "stations": counts},
                run=run, item=wp["id"], at=at)
    store.item_transition(db, wp["id"], "active",
                          "finished" if doc["verdict"] == "GREEN" else "stopped", ver,
                          reason=None if doc["verdict"] == "GREEN" else "red", at=at)
    return run


def _emitter():
    """The W0 metrics emitter (`scripts/pipeline/metrics.py`), or None where it is absent."""
    try:
        from pipeline import metrics
    except ImportError:
        return None
    return metrics


def emit_pipe_pre(db, doc, run):
    """`pipe_pre {repo, strand, verdict} secs` through the W0 emitter; a NOTE text or ''."""
    m = _emitter()
    if m is None:
        return "pipe_pre not sent -- no W0 emitter (scripts/pipeline/metrics.py)"
    target = m.url()
    if not target:
        return ""
    try:
        m.post(target, [m.line("pipe_pre", (("repo", REPO), ("strand", doc.get("strand")),
                                            ("verdict", doc["verdict"])),
                               (("secs", doc["secs"]),), doc["at"])])
    except Exception as exc:  # metrics never fail a run (V0 section 8)
        if db is not None and run:
            events.emit(db, "metrics_failed", "finding",
                        {"run": run, "url_host": urllib.parse.urlsplit(target).hostname or "?",
                         "error": ("%s: %s" % (type(exc).__name__, exc))[:200]}, run=run)
        return ""
    return ""


def record_in_store(doc, repo):
    """Store and metric of a pre-run; the NOTE lines for what did not happen."""
    notes, db, run = [], None, None
    try:
        db = store.open()
        run = record_pre(db, doc, repo)
    except Exception as exc:  # the store never blocks a pre-run
        notes.append(("store", "store write failed: %s" % exc))
    doc["run"] = run
    try:
        note = emit_pipe_pre(db, doc, run)
    except Exception as exc:  # nor does the metric
        note = "pipe_pre failed: %s" % exc
    if note:
        notes.append(("metrics", note))
    if db is not None:
        store.close(db)
    return notes


def summarize(results, th, secs):
    # A NOTE is a finding to read, not a verdict (V0 section 5): a check that
    # only produced NOTEs is neither green nor part of the total (review M1).
    by_check = {}
    for r in results:
        if r.verdict != "NOTE":
            by_check.setdefault(r.check, []).append(r.verdict)
    green = sum(1 for v in by_check.values() if "RED" not in v)
    verdict = "GREEN" if green == len(by_check) else "RED"
    # re-check tv-bn M7c M1: an opt-out of the browser switch on the lane half
    mark = next((m for r in results if r.check == "test-binaries"
                 for m in [_opt_out_in(r.text)] if m), "")
    return green, len(by_check), verdict, "PRE-SUMMARY %s %d/%d %ds %s%s" % (
        th[:8], green, len(by_check), round(secs), verdict, " " + mark if mark else "")


def run_pre(repo, base=None, runner=None, allow_bump=None, strand=None, lane=None, out=print,
            fast=False):
    t0 = time.monotonic()
    repo = os.path.abspath(repo)
    base = base or locks.merge_base(repo)
    paths = diff_paths(repo, base)
    p = plan(paths, repo)
    ctx = {"base": base, "mode": "pre", "allow_bump": allow_bump}
    cmds = lane_commands(p, repo)
    if fast:
        # the local half only: no unit tests, and never the lane (review M2)
        p["unittests"] = []
        runner = None
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        ut = pool.submit(unittest_half, p["unittests"], repo)
        lane_f = pool.submit(lane_half, cmds, runner) if (cmds and runner) else None
        results = local_half(paths, repo, ctx)
        results += turnaround_checks(repo, paths)
        results += ut.result()
        if lane_f is not None:
            results += lane_f.result()
    lane_done = bool(cmds) and runner is not None
    reasons = []
    if cmds and runner is None:
        why = "--fast" if fast else "no lane held"
        results.append(Result("lane", 0, "NOTE", "pre: %s, cargo half skipped" % why))
        reasons.append("%s, cargo half skipped" % why)
    if allow_bump:
        reasons.append("allow-bump: %s" % allow_bump)
    if fast:
        reasons.append("fast: local half only")
    th = tree_hash(repo)
    from pipeline import turnaround
    head = _git(repo, "rev-parse", "HEAD").strip()
    overlay = turnaround.overlay_digests(repo, overlay_paths(repo))
    secs = time.monotonic() - t0
    green, total, verdict, summary = summarize(results, th, secs)
    doc = {"tree_hash": th, "at": _now().isoformat().replace("+00:00", "Z"),
           "verdict": verdict, "lane_half": lane_done or not cmds, "reasons": reasons,
           "mode": "pre", "stage": "pre", "strand": strand, "lane": lane, "base": base,
           "secs": round(secs, 3), "green": green, "total": total,
           "diff_lines": locks.diff_lines(repo, base), "plan": p,
           "checks": [r.as_json() for r in results], "head": head, "overlay": overlay}
    for check, note in record_in_store(doc, repo):
        results.append(Result(check, 0, "NOTE", note))
    for r in results:
        out(r.line())
    write_stamp(repo, doc)
    out(summary)
    return doc


def register(sub):
    sp = sub.add_parser("pre", help="the pre-run before a strand gate (stamp)")
    _args(sp)
    sp.set_defaults(func=_cmd)


def _args(ap):
    ap.add_argument("--repo", default=None)
    ap.add_argument("--base", default=None)
    ap.add_argument("--strand", default=None)
    ap.add_argument("--lane", default=None, help="the held lane's name")
    ap.add_argument("--lane-target", default=None, help="its ssh target")
    ap.add_argument("--no-lane", action="store_true")
    ap.add_argument("--fast", action="store_true",
                    help="the local half only: no lane, no unit tests")
    ap.add_argument("--allow-bump", default=None, metavar="REASON")
    ap.add_argument("--tree-hash", action="store_true", help="print the tree hash and exit")
    ap.add_argument("--stamp-check", action="store_true",
                    help="exit 0 with a fresh GREEN stamp of this tree, 2 otherwise")


def _cmd(args):
    repo = os.path.abspath(args.repo or _git(os.getcwd(), "rev-parse", "--show-toplevel").strip()
                           or REPO_ROOT)
    if args.tree_hash:
        print(tree_hash(repo))
        return 0
    if args.stamp_check:
        ok, line = stamp_status(repo)
        print(line, file=sys.stdout if ok else sys.stderr)
        return 0 if ok else 2
    runner = None
    if args.lane_target and not args.no_lane and not args.fast:
        runner = ssh_runner(args.lane_target, args.strand or "pre", repo)
    doc = run_pre(repo, base=args.base, runner=runner, allow_bump=args.allow_bump,
                  strand=args.strand, lane=args.lane, fast=args.fast)
    return 0 if doc["verdict"] == "GREEN" else 1


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    _args(ap)
    return _cmd(ap.parse_args(argv))


if __name__ == "__main__":
    sys.exit(main())
