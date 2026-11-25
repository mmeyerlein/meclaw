"""`pipeline.py candidate <rev>` -- one commit built on every lane, proven equal (GH #1389).

`strand.sh candidate <rev> [--lanes a,b,...] [--wave <W>] [--wait-admission]`
builds the `meclaw` binary of `<rev>` on each named lane (default: every lane of
the host file with `class=lane`), each under its own lease
`<wave>/candidate:<rev12>-<n>` taken through the normal admission (`schedule.
candidate_take`), in its own target `<root>/kb-target` with `--locked`
(`lane.build_candidate`). Each lease goes back as soon as its lane has built:
no lane stands leased and empty while another one is waited for (#1638). Then:

    CANDIDATE <rev12> <lane> <sha256> <secs>s          one line per lane
    CANDIDATE <rev12> EQUAL n/n sha256 <hex> -> <path> exit 0
    CANDIDATE <rev12> UNEQUAL <lane>...                exit 7

Equal means one sha256 over all named lanes. A lane whose sha differs from
the most frequent one (ties: the group of the first lane) is suspected of a
stale target -- OR-LV-47: build02 built differently for weeks from an old
`libtokio_macros*.so` in its warm target, unseen because only one lane built --
and is rebuilt exactly once with an empty `kb-target`, under its lease taken
again (a lane that is gone by then: exit 3 without `--wait-admission`);
whatever still differs makes the run UNEQUAL, a finding, never a reason to
loosen. An EQUAL binary comes home from the first equal lane (`~/kb/bin/
meclaw-<rev12>`), its sha256 checked again and its `--version` run; a copy
that differs or does not answer is exit 6 and never stays at that path.

A build that fails on a lane aborts the run (exit 5, verdict `aborted`); a
lane leased elsewhere without `--wait-admission` is exit 3 (verdict `queued`)
with no queue place left behind. Every lease taken goes back in every case.
Runs of different revs build side by side (the lease names its rev); a second
run of the same rev waits for the first (`--wait-admission`, else exit 3).
The receipt `plans/<W>/receipts/candidate-<rev12>.json` carries rev, lanes,
sha and seconds per lane, the retries, the verdict and the binary.

The lane binary and the release asset are not the same bytes (the asset is
built from the export tree): the candidate serves labs and batteries.

Reuse (w1 kandidat, #1606, OR-54s-62): when `~/kb/bin` holds a candidate
whose `core_sha` differs from `<rev>` in no build input (`BUILD_INPUTS`,
`git diff --quiet`), nothing is built and no lane is leased: the same bytes
go under `meclaw-<rev12>` here and on the named lanes, the `origin.json`
names `reused_from`, the verdict is `REUSED` (exit 0):

    CANDIDATE <rev12> REUSED from <old12> sha256 <hex> -> <path>

`--prune` (no `<rev>`) removes every `meclaw-<rev12>` (with its
`origin.json`) older than `PRUNE_DAYS` that no set of the three
repositories names (`meclaw_candidate.rev`, or the commit `meclaw_core`
names), here and on the lanes; a set repository that is missing or a set
that is no JSON refuses the whole prune (exit 2): nothing is guessed.
"""

import collections
import fcntl
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time

from . import lane as lane_mod
from . import schedule

EXIT_EQUAL, EXIT_QUEUED, EXIT_ABORTED, EXIT_FETCH, EXIT_UNEQUAL = 0, 3, 5, 6, 7
LOCK_ENV = "MECLAW_CANDIDATE_LOCK"
DEFAULT_LOCK = "/tmp/meclaw-candidate.lock"
VERSION_TIMEOUT_S = 60
WAVE_DATE = re.compile(r"-[0-9]{4}-[0-9]{2}-[0-9]{2}$")
# What goes into the binary: a candidate whose commit differs from <rev> in
# none of these is the same build (the include_* paths of crates/ stay inside it).
BUILD_INPUTS = ("crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo")
CANDIDATE_NAME = re.compile(r"^meclaw-([0-9a-f]{12})$")
PRUNE_DAYS = 14
# the set repositories below the home (the core's own tree comes from --repo)
SET_REPOS = ("projeks/soul2me", "projeks/orga-hive")


def default_lanes(lanes_mod=lane_mod):
    """Every lane of the host file with `class=lane` (not the owner's machine)."""
    return [ln.name for ln in lanes_mod.load_lanes() if ln.cls == "lane"]


def _sha256(path):
    return lane_mod._sha256(path)


def _wave_name(wave):
    """`<name>-<yyyy-mm-dd>` -> `<name>` (the rule of `strand.sh wave_name`)."""
    return WAVE_DATE.sub("", wave.rstrip("/"))


def _lock(env, wait, r12):
    """One candidate run per rev and machine (`<lock>.<rev12>`): two runs of one
    rev would share the keys of their leases, the binary's path and the receipt.
    The lanes' `wt/kb-bin` and `kb-target` are one build at a time by the lane's
    lease, so runs of different revs need no common lock (#1638: they ended or
    waited for each other on disjoint lanes). Returns the open lock file, or
    None when held and not `wait`."""
    path = "%s.%s" % (env.get(LOCK_ENV) or DEFAULT_LOCK, r12)
    fh = open(path, "a+")
    try:
        fcntl.flock(fh, fcntl.LOCK_EX | (0 if wait else fcntl.LOCK_NB))
    except BlockingIOError:
        fh.close()
        return None
    return fh


def _show(path):
    home = os.path.expanduser("~")
    return "~" + path[len(home):] if path.startswith(home + os.sep) else path


def _reference(shas, lanes):
    """The most frequent sha; a tie goes to the group of the earliest lane."""
    counts = collections.Counter(shas[ln] for ln in lanes)
    top = max(counts.values())
    return next(shas[ln] for ln in lanes if counts[shas[ln]] == top)


class _Run:
    def __init__(self, rev, lanes, wave, receipts_dir, wait, env, cwd, out, lanes_mod, bin_dir):
        self.rev, self.r12, self.lanes, self.wave = rev, rev[:12], list(lanes), wave
        self.receipts_dir, self.wait, self.env, self.cwd = receipts_dir, wait, env, cwd
        self.out, self.lanes_mod, self.bin_dir = out, lanes_mod, bin_dir
        self.wave_name = _wave_name(wave)
        self.n = {ln: i + 1 for i, ln in enumerate(self.lanes)}
        self.held = []
        self.builds = {}
        self.record = {"rev": rev, "lanes": self.lanes, "wave": wave, "builds": self.builds,
                       "leases": [schedule.candidate_key(self.wave_name, self.r12, i + 1)
                                  for i in range(len(self.lanes))],
                       "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
        self._print_lock = threading.Lock()

    def say(self, text):
        with self._print_lock:
            self.out.write("CANDIDATE %s %s\n" % (self.r12, text))
            self.out.flush()

    # -- leases ------------------------------------------------------------
    def take(self, ln):
        code, out, err = schedule.candidate_take(self.wave_name, self.r12, self.n[ln], ln,
                                                 env=self.env, cwd=self.cwd, wait=self.wait,
                                                 pid=os.getpid())
        if code == 0:
            with self._print_lock:
                self.held.append(ln)
            return None
        return "lease %s on %s: exit %d %s" % (
            schedule.candidate_key(self.wave_name, self.r12, self.n[ln]), ln, code,
            " ".join((out + err).split())[:300])

    def release(self, ln):
        """Give the lease of `ln` back (nothing when not held); one that does not
        go back stays in `held` for `release_all` at the end of the run."""
        if ln not in self.held:
            return
        try:
            code, out, err = schedule.candidate_release(self.wave_name, self.r12, self.n[ln],
                                                        env=self.env, cwd=self.cwd)
        except Exception as exc:  # noqa: BLE001 -- a locked store loses neither the build nor the rest
            code, out, err = 6, "", "%s: %s" % (type(exc).__name__, exc)
        if code == 0:
            with self._print_lock:
                self.held.remove(ln)
        else:
            self.say("lease on %s kept: exit %d %s" % (ln, code, " ".join((out + err).split())[:300]))

    def release_all(self):
        for ln in list(self.held):
            self.release(ln)
        return list(self.held)

    # -- builds ------------------------------------------------------------
    def build(self, ln, fresh=False):
        lane = self.lanes_mod.find_lane(ln)
        t0 = time.monotonic()
        got = self.lanes_mod.build_candidate(lane, self.cwd, self.rev, fresh=fresh)
        got = dict(got, secs=round(time.monotonic() - t0, 1), fresh_target=bool(fresh))
        return got

    def _one(self, ln, errors):
        try:
            if self.wait:
                why = self.take(ln)
                if why:
                    errors[ln] = ("queued", why)
                    return
            got = self.build(ln)
            self.builds[ln] = got
            self.say("%s %s %ss" % (ln, got["sha256"], got["secs"]))
        except lane_mod.LaneError as exc:
            errors[ln] = ("aborted", str(exc))
        except Exception as exc:  # noqa: BLE001 -- a lane thread never takes the run down unreleased
            errors[ln] = ("aborted", "%s: %s" % (type(exc).__name__, exc))
        finally:
            # B17-1 (#1638): four lanes taken one by one stood leased and empty
            # (14 min for two of them) until the last one was free
            self.release(ln)

    def build_all(self):
        errors = {}
        threads = [threading.Thread(target=self._one, args=(ln, errors), daemon=True)
                   for ln in self.lanes]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        return errors

    # -- the run -----------------------------------------------------------
    def finish(self, verdict, code, **extra):
        self.record.update(extra, verdict=verdict, exit=code,
                           ended=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
        os.makedirs(self.receipts_dir, exist_ok=True)
        path = os.path.join(self.receipts_dir, "candidate-%s.json" % self.r12)
        with open(path + ".part", "w", encoding="utf-8") as fh:
            json.dump(self.record, fh, indent=1, sort_keys=True)
            fh.write("\n")
        os.replace(path + ".part", path)
        self.say("receipt %s" % path)
        return code

    def reuse(self, old):
        """Lay the binary of candidate `old` (its origin dict, `path`) under
        this rev's name here and on every named lane; no build, no lease."""
        dest = os.path.join(self.bin_dir, "meclaw-%s" % self.r12)
        part = dest + ".part"
        shutil.copy2(old["path"], part)
        if _sha256(part) != old["sha256"]:
            os.unlink(part)
            self.say("FETCH the copy of %s changed its sha256" % _show(old["path"]))
            return self.finish("fetch_mismatch", EXIT_FETCH, reused_from=old["core_sha"])
        os.replace(part, dest)
        origin = {k: v for k, v in old["origin"].items() if k != "reused_from"}
        origin.update(core_sha=self.rev, reused_from=old["core_sha"])
        with open(dest + ".origin.json.part", "w", encoding="utf-8") as fh:
            fh.write(json.dumps(origin, indent=1) + "\n")
        os.replace(dest + ".origin.json.part", dest + ".origin.json")
        self.record["leases"] = []
        for ln in self.lanes:
            try:
                self.lanes_mod.push_candidate(self.lanes_mod.find_lane(ln), dest)
            except lane_mod.LaneError as exc:
                self.say("%s ABORTED -- %s" % (ln, exc))
                return self.finish("aborted", EXIT_ABORTED, errors={ln: str(exc)},
                                   reused_from=old["core_sha"])
            self.say("%s %s reused" % (ln, old["sha256"]))
        self.say("REUSED from %s sha256 %s -> %s" % (old["core_sha"][:12], old["sha256"], _show(dest)))
        return self.finish("REUSED", EXIT_EQUAL, reference=old["sha256"], binary=dest,
                           reused_from=old["core_sha"], build_inputs=list(BUILD_INPUTS),
                           version=origin.get("version", ""))

    def go(self):
        if not self.wait:
            for ln in self.lanes:
                why = self.take(ln)
                if why:
                    self.say("queued -- %s (no queue place kept; --wait-admission waits)" % why)
                    return self.finish("queued", EXIT_QUEUED, reason=why)
        errors = self.build_all()
        if errors:
            kind = "aborted" if any(k == "aborted" for k, _ in errors.values()) else "queued"
            for ln, (_, why) in sorted(errors.items()):
                self.say("%s %s -- %s" % (ln, kind.upper(), why))
            return self.finish(kind, EXIT_ABORTED if kind == "aborted" else EXIT_QUEUED,
                               errors={ln: why for ln, (_, why) in errors.items()})
        shas = {ln: self.builds[ln]["sha256"] for ln in self.lanes}
        ref = _reference(shas, self.lanes)
        for ln in [ln for ln in self.lanes if shas[ln] != ref]:
            self.say("stale target suspected on %s -- one rebuild with an empty kb-target" % ln)
            why = self.take(ln)
            if why:
                self.say("queued -- %s (the rebuild needs its lane again)" % why)
                # every lane built and some differ: the finding outlives the queue
                return self.finish("queued", EXIT_QUEUED, reason=why, reference=ref,
                                   deviating=[x for x in self.lanes if shas[x] != ref])
            try:
                again = self.build(ln, fresh=True)
            except lane_mod.LaneError as exc:
                self.say("%s ABORTED -- %s" % (ln, exc))
                return self.finish("aborted", EXIT_ABORTED, errors={ln: str(exc)})
            finally:
                self.release(ln)
            self.builds[ln]["retry"] = again
            shas[ln] = again["sha256"]
            self.say("%s %s %ss (empty kb-target)" % (ln, again["sha256"], again["secs"]))
        deviating = [ln for ln in self.lanes if shas[ln] != ref]
        if deviating:
            self.say("UNEQUAL %s" % " ".join(deviating))
            return self.finish("UNEQUAL", EXIT_UNEQUAL, deviating=deviating, reference=ref)
        return self.fetch(ref)

    def fetch(self, ref):
        first = self.lanes[0]
        dest = os.path.join(self.bin_dir, "meclaw-%s" % self.r12)
        stage = os.path.join(self.bin_dir, ".candidate-%s.%d" % (self.r12, os.getpid()))
        os.makedirs(stage, exist_ok=True)
        try:
            try:
                got = self.lanes_mod.fetch_candidate(self.lanes_mod.find_lane(first), self.rev, stage)
            except lane_mod.LaneError as exc:
                self.say("ABORTED -- %s" % exc)
                return self.finish("aborted", EXIT_ABORTED, errors={first: str(exc)}, reference=ref)
            have = _sha256(got)
            if have != ref:
                self.say("FETCH the copy from %s has sha256 %s, the lanes built %s" % (first, have, ref))
                return self.finish("fetch_mismatch", EXIT_FETCH, reference=ref, fetched=have)
            try:
                res = subprocess.run([got, "--version"], capture_output=True, text=True,
                                     timeout=VERSION_TIMEOUT_S)
                version = (res.stdout.strip().splitlines() or [""])[0]
                ok = res.returncode == 0 and version.startswith("meclaw")
            except (OSError, subprocess.TimeoutExpired) as exc:
                res, version, ok = None, str(exc), False
            if not ok:
                self.say("FETCH %s --version failed (exit %s: %s)"
                         % (_show(dest), res.returncode if res else "-", version[:200]))
                return self.finish("fetch_mismatch", EXIT_FETCH, reference=ref, version=version)
            if os.path.exists(dest) and _sha256(dest) != ref:
                self.say("FETCH %s exists with another sha256 -- left as it is" % _show(dest))
                return self.finish("fetch_mismatch", EXIT_FETCH, reference=ref,
                                   existing=_sha256(dest))
            for name in os.listdir(stage):
                os.replace(os.path.join(stage, name), os.path.join(self.bin_dir, name))
        finally:
            for name in os.listdir(stage):
                os.unlink(os.path.join(stage, name))
            os.rmdir(stage)
        self.say("version %s" % version)
        self.say("EQUAL %d/%d sha256 %s -> %s" % (len(self.lanes), len(self.lanes), ref, _show(dest)))
        return self.finish("EQUAL", EXIT_EQUAL, reference=ref, binary=dest, version=version,
                           fetched_from=first)


def _origin(path):
    try:
        with open(path + ".origin.json", encoding="utf-8") as fh:
            doc = json.load(fh)
    except (OSError, ValueError):
        return None
    return doc if isinstance(doc, dict) else None


def reusable(repo, rev, bin_dir):
    """The candidate in `bin_dir` whose `core_sha` differs from `rev` in no
    build input, its bytes still those of its `origin.json` -- {core_sha,
    sha256, path, origin} of the newest such, or None."""
    found = []
    for name in sorted(os.listdir(bin_dir)) if os.path.isdir(bin_dir) else []:
        m = CANDIDATE_NAME.match(name)
        path = os.path.join(bin_dir, name)
        origin = m and os.path.isfile(path) and _origin(path)
        if not origin:
            continue
        core = str(origin.get("core_sha", ""))
        if core == rev or not core.startswith(m.group(1)) or not os.access(path, os.X_OK):
            continue
        same = subprocess.run(["git", "-C", repo, "diff", "--quiet", core, rev, "--"] + list(BUILD_INPUTS),
                              capture_output=True)
        if same.returncode != 0 or _sha256(path) != origin.get("sha256"):
            continue  # moved (1), or an unknown commit (128), or other bytes
        found.append((os.stat(path).st_mtime, {"core_sha": core, "sha256": origin["sha256"],
                                               "path": path, "origin": origin}))
    return max(found, key=lambda t: t[0])[1] if found else None


def named_revs(set_roots):
    """(names, error): every commit prefix a set of `set_roots` names
    (`meclaw_candidate.rev`, the commit of `meclaw_core`); error when a root
    is missing or a set is no JSON object."""
    names = set()
    for root in set_roots:
        if not os.path.isdir(root):
            return None, "set repository %s is not here" % root
        sets = os.path.join(root, "sets")
        for f in sorted(os.listdir(sets)) if os.path.isdir(sets) else []:
            if not f.endswith(".json"):
                continue
            try:
                with open(os.path.join(sets, f), encoding="utf-8") as fh:
                    doc = json.load(fh)
            except (OSError, ValueError) as exc:
                return None, "%s/sets/%s is no JSON (%s)" % (root, f, exc)
            if not isinstance(doc, dict):
                return None, "%s/sets/%s is no JSON object" % (root, f)
            cand = doc.get("meclaw_candidate")
            if isinstance(cand, dict) and cand.get("rev"):
                names.add(str(cand["rev"]))
            core = re.search(r"\b[0-9a-f]{7,40}\b", str(doc.get("meclaw_core", "")))
            if core:
                names.add(core.group(0))
    return names, None


def prune(*, repo, set_roots=None, lanes=None, bin_dir=None, out=None, lanes_mod=lane_mod,
          days=PRUNE_DAYS, now=None):
    """`--prune`: remove the candidates no set names and older than `days`,
    here and on the lanes (ssh lanes; a local lane is this home). Exit 0,
    2 refused (nothing removed), 5 a lane failed."""
    out = out or sys.stdout
    home = os.path.expanduser("~")
    roots = set_roots if set_roots is not None else [repo] + [os.path.join(home, r) for r in SET_REPOS]
    names, why = named_revs(roots)
    if why:
        out.write("CANDIDATE prune REFUSED -- %s\n" % why)
        return 2
    bin_dir = bin_dir or os.path.join(home, lane_mod.CANDIDATE_BIN)
    # wall-clock: now (the kit test passes a fixed instant)
    cut = (time.time() if now is None else now) - days * 86400

    def named(r12):
        return any(n.startswith(r12) or r12.startswith(n) for n in names)

    keep = sorted({n[:12] for n in names})
    for name in sorted(os.listdir(bin_dir)) if os.path.isdir(bin_dir) else []:
        m = CANDIDATE_NAME.match(name)
        path = os.path.join(bin_dir, name)
        if not m or named(m.group(1)) or os.stat(path).st_mtime >= cut:
            continue
        for p in (path, path + ".origin.json"):
            if os.path.exists(p):
                os.unlink(p)
        out.write("CANDIDATE prune PRUNED %s here\n" % m.group(1))
    code = 0
    for ln in list(lanes) if lanes else default_lanes(lanes_mod):
        try:
            lane = lanes_mod.find_lane(ln)
            if getattr(lane, "transport", "ssh") == "local":
                continue
            for r12 in lanes_mod.prune_candidates(lane, keep, days):
                out.write("CANDIDATE prune PRUNED %s on %s\n" % (r12, ln))
        except lane_mod.LaneError as exc:
            out.write("CANDIDATE prune %s ABORTED -- %s\n" % (ln, exc))
            code = EXIT_ABORTED
    return code


def run(rev, *, repo, lanes, wave, receipts_dir, wait=False, env=None, out=None,
        lanes_mod=lane_mod, bin_dir=None):
    """The verb; returns its exit code (0 EQUAL, 3 queued, 5 aborted, 6 fetch, 7 UNEQUAL)."""
    env = dict(os.environ if env is None else env)
    out = out or sys.stdout
    res = subprocess.run(["git", "-C", repo, "rev-parse", "--verify", "--quiet", rev + "^{commit}"],
                         capture_output=True, text=True)
    full = res.stdout.strip()
    if res.returncode != 0 or not full:
        out.write("CANDIDATE %s ABORTED -- no such commit in %s\n" % (rev, repo))
        return 2
    lanes = list(lanes) if lanes else default_lanes(lanes_mod)
    if not lanes or len(set(lanes)) != len(lanes):
        out.write("CANDIDATE %s ABORTED -- name the lanes once each (--lanes a,b,...)\n" % full[:12])
        return 2
    for ln in lanes:
        try:
            lanes_mod.find_lane(ln)
        except lane_mod.LaneError as exc:
            out.write("CANDIDATE %s ABORTED -- %s\n" % (full[:12], exc))
            return 2
    bin_dir = bin_dir or os.path.expanduser("~/" + lane_mod.CANDIDATE_BIN)
    os.makedirs(bin_dir, exist_ok=True)
    lock = _lock(env, wait, full[:12])
    if lock is None:
        out.write("CANDIDATE %s queued -- another candidate run of this rev builds on this "
                  "machine (--wait-admission waits)\n" % full[:12])
        return EXIT_QUEUED
    run_ = _Run(full, lanes, wave, receipts_dir, wait, env, repo, out, lanes_mod, bin_dir)
    old = reusable(repo, full, bin_dir)
    if old:
        try:
            return run_.reuse(old)
        finally:
            lock.close()
    try:
        run_.say("start on %s (leases %s)" % (",".join(lanes),
                                              schedule.candidate_key(run_.wave_name, run_.r12, "<n>")))
        try:
            return run_.go()
        except Exception as exc:  # noqa: BLE001 -- the receipt says aborted, the leases go back
            run_.say("ABORTED -- %s: %s" % (type(exc).__name__, exc))
            return run_.finish("aborted", EXIT_ABORTED, errors={"run": str(exc)})
    finally:
        run_.release_all()
        lock.close()


def _cmd(args):
    if args.prune:
        if args.rev:
            sys.stderr.write("candidate: --prune takes no <rev>\n")
            return 2
        return prune(repo=args.repo or os.getcwd(),
                     lanes=[x for x in (args.lanes or "").split(",") if x])
    if not (args.rev and args.wave and args.receipts):
        sys.stderr.write("candidate: <rev>, --wave and --receipts are needed\n")
        return 2
    return run(args.rev, repo=args.repo or os.getcwd(),
               lanes=[x for x in (args.lanes or "").split(",") if x],
               wave=args.wave, receipts_dir=args.receipts, wait=args.wait_admission)


def register(sub):
    p = sub.add_parser("candidate", help="build <rev> on every lane, prove the sha256 equal (GH #1389)")
    p.add_argument("rev", nargs="?")
    p.add_argument("--lanes", default="", help="a,b,... (default: the host file's class=lane lanes)")
    p.add_argument("--prune", action="store_true",
                   help="remove candidates no set names, older than %d days, here and on the lanes"
                   % PRUNE_DAYS)
    p.add_argument("--wave", help="the wave directory under plans/ (lease names)")
    p.add_argument("--receipts", help="the directory of candidate-<rev12>.json")
    p.add_argument("--repo", help="the repository of <rev> (default: the working directory)")
    p.add_argument("--wait-admission", action="store_true",
                   help="wait in the lane queue instead of exit 3")
    p.set_defaults(func=_cmd)
