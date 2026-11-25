"""Tests for `pipeline.py candidate <rev>` (`strand.sh candidate`, GH #1389).

A candidate is one commit built on every named lane, each lane under its own
lease `<wave>/candidate:<rev12>-<n>` taken through the normal admission, in its own
target (`<root>/kb-target`, `--locked`); the sha256 of the binary must be the
same on all of them (OR-LV-47: build02 built differently for weeks from a stale
warm target, unseen because only one lane built). The binary then comes home
from the first equal lane and is verified again.

The lanes are faked (`FakeLanes`: build, fetch); the leases are real, taken in
a throw-away store armed on the four lane names, the lane tree probe of a
release answers "nothing lives there" (`schedule._lane_tree_probe`).
"""
import dataclasses
import fcntl
import hashlib
import io
import json
import os
import pathlib
import re
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

from scripts.pipeline import candidate, schedule
from scripts.tests import kit_time, store_isolation

WAVE = "welle-c-2026-11-02"
LANES = ("build01", "build02", "build03", "build04")
GIT = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid",
       "-c", "init.defaultBranch=master", "-c", "commit.gpgsign=false"]
BINARY = b"#!/bin/sh\necho 'meclaw 9.9.9'\n"
OTHER = b"#!/bin/sh\necho 'meclaw 9.9.9 (stale tokio-macros)'\n"


def sha(data):
    return hashlib.sha256(data).hexdigest()


@dataclasses.dataclass
class FakeLane:
    name: str
    ssh: str = "builder@lane.example.invalid"
    root: str = "/srv/target"
    transport: str = "ssh"


class FakeLanes:
    """The lane side of a candidate: `find_lane`, `build_candidate`, `fetch_candidate`.

    `builds[lane]` is the list of binaries the lane builds, one per call (the
    last one repeats); `broken` lanes raise `LaneError` at the build; `fetched`
    is what the fetch writes home (default: the lane's last binary)."""

    def __init__(self, builds=None, broken=(), fetched=None):
        self.builds = {ln: list((builds or {}).get(ln, [BINARY])) for ln in LANES}
        self.broken = set(broken)
        self.fetched = fetched
        self.calls = []
        self.fetches = []
        self.pushes = []
        self.pruned = []
        self.last = {}

    def find_lane(self, name, path=None):
        if name not in LANES:
            raise candidate.lane_mod.LaneError(2, "no lane %s" % name)
        return FakeLane(name)

    def build_candidate(self, lane, repo_root, rev, fresh=False):
        self.calls.append((lane.name, fresh))
        if lane.name in self.broken:
            raise candidate.lane_mod.LaneError(2, "cargo build failed on %s" % lane.name)
        queue = self.builds[lane.name]
        data = queue.pop(0) if len(queue) > 1 else queue[0]
        self.last[lane.name] = data
        return {"sha256": sha(data), "cargo": "cargo 1.95.0", "version": "meclaw 9.9.9"}

    def push_candidate(self, lane, path):
        self.pushes.append((lane.name, os.path.basename(path)))

    def prune_candidates(self, lane, keep, days):
        self.pruned.append((lane.name, sorted(keep), days))
        return []

    def fetch_candidate(self, lane, rev, dest_dir):
        self.fetches.append(lane.name)
        path = os.path.join(dest_dir, "meclaw-%s" % rev[:12])
        with open(path, "wb") as fh:
            fh.write(self.fetched if self.fetched is not None else self.last[lane.name])
        os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)
        return path


class CandidateCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        self.repo = self.tmp / "core"
        subprocess.run(GIT + ["init", "-q", "-b", "master", str(self.repo)], check=True)
        subprocess.run(GIT + ["-C", str(self.repo), "commit", "-q", "--allow-empty", "-m", "x"],
                       check=True)
        self.rev = subprocess.run(["git", "-C", str(self.repo), "rev-parse", "HEAD"],
                                  capture_output=True, text=True, check=True).stdout.strip()
        self.bin_dir = self.tmp / "kb" / "bin"
        self.receipts = self.tmp / "plans" / WAVE / "receipts"
        lanes_file = self.tmp / "lanes"
        lanes_file.write_text("".join("%s builder@%s.example.invalid\n" % (n, n) for n in LANES))
        self.db_path = self.tmp / "state" / "pipeline.db"
        self.env = {"MECLAW_PIPELINE_DB": str(self.db_path),
                    "MECLAW_LANES_FILE": str(lanes_file),
                    "MECLAW_STRAND_TOKENS": str(self.tmp / "tokens"),
                    "MECLAW_CANDIDATE_LOCK": str(self.tmp / "candidate.lock")}
        self.addCleanup(setattr, schedule, "_lane_tree_probe", schedule._lane_tree_probe)
        schedule._lane_tree_probe = lambda ln: []
        code, out, err = schedule.run_lanes(["init", "--lanes", ",".join(LANES)],
                                            env=self.env, cwd=str(self.repo))
        self.assertEqual(0, code, out + err)

    def tearDown(self):
        self._tmp.cleanup()

    def candidate(self, fake, lanes=LANES, wait=False, rev=None):
        out = io.StringIO()
        code = candidate.run(rev or self.rev, repo=str(self.repo), lanes=list(lanes), wave=WAVE,
                             receipts_dir=str(self.receipts), wait=wait, env=self.env,
                             out=out, lanes_mod=fake, bin_dir=str(self.bin_dir))
        return code, out.getvalue()

    def held(self):
        db = sqlite3.connect(str(self.db_path))
        try:
            return [r[0] for r in db.execute(
                "SELECT holder FROM lease_entries WHERE holder LIKE '%candidate%'")]
        finally:
            db.close()

    def receipt(self):
        with open(self.receipts / ("candidate-%s.json" % self.rev[:12]), encoding="utf-8") as fh:
            return json.load(fh)

    def test_an_equal_build_on_every_lane_is_a_candidate(self):
        fake = FakeLanes()
        code, out = self.candidate(fake)
        self.assertEqual(0, code, out)
        r12 = self.rev[:12]
        for ln in LANES:
            self.assertRegex(out, r"(?m)^CANDIDATE %s %s %s [0-9.]+s$" % (r12, ln, sha(BINARY)))
        dest = self.bin_dir / ("meclaw-%s" % r12)
        self.assertIn("CANDIDATE %s EQUAL 4/4 sha256 %s -> %s" % (r12, sha(BINARY), dest), out)
        self.assertEqual(sha(BINARY), sha(dest.read_bytes()))
        self.assertEqual(sorted((ln, False) for ln in LANES), sorted(fake.calls))
        rec = self.receipt()
        self.assertEqual(self.rev, rec["rev"])
        self.assertEqual(list(LANES), rec["lanes"])
        self.assertEqual("EQUAL", rec["verdict"])
        self.assertEqual({ln: sha(BINARY) for ln in LANES},
                         {ln: v["sha256"] for ln, v in rec["builds"].items()})
        self.assertTrue(all("secs" in v for v in rec["builds"].values()))
        self.assertEqual(str(dest), rec["binary"])
        self.assertEqual([], self.held())

    def test_one_deviating_lane_is_no_candidate(self):
        fake = FakeLanes(builds={"build02": [OTHER, OTHER]})
        code, out = self.candidate(fake)
        self.assertEqual(7, code, out)
        r12 = self.rev[:12]
        self.assertRegex(out, r"(?m)^CANDIDATE %s UNEQUAL build02$" % r12)
        self.assertIn("stale target suspected on build02", out)
        # exactly one automatic rebuild of the deviating lane, with an empty kb-target
        self.assertEqual([("build02", False), ("build02", True)],
                         [c for c in fake.calls if c[0] == "build02"])
        self.assertEqual(1, len([c for c in fake.calls if c[0] == "build01"]))
        self.assertEqual([], fake.fetches)
        self.assertFalse((self.bin_dir / ("meclaw-%s" % r12)).exists())
        rec = self.receipt()
        self.assertEqual("UNEQUAL", rec["verdict"])
        self.assertEqual(["build02"], rec["deviating"])
        self.assertEqual(sha(OTHER), rec["builds"]["build02"]["sha256"])
        self.assertEqual(sha(OTHER), rec["builds"]["build02"]["retry"]["sha256"])
        self.assertEqual([], self.held())
        # ... and a lane that builds equal from an empty target makes the candidate.
        fake = FakeLanes(builds={"build03": [OTHER, BINARY]})
        code, out = self.candidate(fake)
        self.assertEqual(0, code, out)
        self.assertIn("stale target suspected on build03", out)
        self.assertIn("EQUAL 4/4", out)
        self.assertEqual("EQUAL", self.receipt()["verdict"])

    def test_leases_are_released_on_failure(self):
        fake = FakeLanes(broken={"build03"})
        code, out = self.candidate(fake)
        self.assertNotEqual(0, code, out)
        self.assertNotIn(code, (6, 7), out)
        self.assertIn("build03", out)
        self.assertEqual("aborted", self.receipt()["verdict"])
        self.assertEqual([], self.held())
        self.assertEqual([], fake.fetches)
        # a lane leased to another strand: without --wait-admission no wait, no
        # queue place, and the leases already taken go back
        code_, out_, err_ = schedule.run_lanes(
            ["take", "--strand", "other/s", "--lane", "build04"], env=self.env, cwd=str(self.repo))
        self.assertEqual(0, code_, out_ + err_)
        fake = FakeLanes()
        code, out = self.candidate(fake)
        self.assertEqual(3, code, out)
        self.assertEqual([], fake.calls)
        self.assertEqual([], self.held())

    def test_the_binary_is_fetched_from_the_first_equal_lane_and_verified(self):
        fake = FakeLanes()
        code, out = self.candidate(fake)
        self.assertEqual(0, code, out)
        self.assertEqual(["build01"], fake.fetches)
        self.assertRegex(out, r"(?m)^CANDIDATE %s version meclaw 9\.9\.9$" % self.rev[:12])
        # a copy that differs from the lanes' sha is no candidate (exit 6), and
        # it never stays at the binary's path
        for name in os.listdir(self.bin_dir):
            os.unlink(self.bin_dir / name)
        code, out = self.candidate(FakeLanes(fetched=OTHER))
        self.assertEqual(6, code, out)
        self.assertFalse((self.bin_dir / ("meclaw-%s" % self.rev[:12])).exists())
        self.assertEqual([], self.held())
        # ... and neither is a binary whose `--version` fails
        broken = b"#!/bin/sh\nexit 1\n"
        code, out = self.candidate(FakeLanes(builds={ln: [broken] for ln in LANES}))
        self.assertEqual(6, code, out)
        self.assertIn("--version", out)
        self.assertFalse((self.bin_dir / ("meclaw-%s" % self.rev[:12])).exists())


class SideBySideCase(CandidateCase):
    """w2 candlease (#1638, B17 1 and 3): four lanes taken one by one stood
    leased and empty until the last one was free, and two runs of one wave
    shared the lease names `<wave>/candidate:<n>` and the machine's one lock."""

    def second(self):
        """Another commit of the repository (a build input moves: no reuse)."""
        path = self.repo / "crates" / "x.rs"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("x\n")
        subprocess.run(GIT + ["-C", str(self.repo), "add", "-A"], check=True)
        subprocess.run(GIT + ["-C", str(self.repo), "commit", "-q", "-m", "x"], check=True)
        return subprocess.run(["git", "-C", str(self.repo), "rev-parse", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()

    def started(self, fake, **kw):
        """`candidate` in a thread, its first build held: (thread, result, go)."""
        building, go, got = threading.Event(), threading.Event(), []
        real = fake.build_candidate

        def build(lane, repo_root, rev, fresh=False):
            building.set()
            go.wait(kit_time.HANG)
            return real(lane, repo_root, rev, fresh=fresh)
        fake.build_candidate = build
        th = threading.Thread(target=lambda: got.append(self.candidate(fake, **kw)), daemon=True)
        self.addCleanup(th.join, kit_time.HANG)
        self.addCleanup(go.set)
        th.start()
        self.assertTrue(building.wait(kit_time.HANG), "HANG: the first run never built")
        return th, got, go

    def test_a_waiting_candidate_keeps_no_lane_it_has_built(self):
        # the lane build04 comes only once the other three went back: were one
        # kept until the verdict, the wait for build04 would never see them free
        back = threading.Semaphore(0)
        take, release = schedule.candidate_take, schedule.candidate_release
        got_back = []

        def released(*args, **kw):
            res = release(*args, **kw)
            back.release()
            return res

        def taken(wave_name, *args, **kw):
            if "build04" in args:
                got_back.extend(back.acquire(timeout=kit_time.HANG) for _ in range(3))
                if not all(got_back):
                    return 3, "", "build01-03 stay leased while build04 is waited for"
            return take(wave_name, *args, **kw)
        fake = FakeLanes()
        with mock.patch.object(schedule, "candidate_release", side_effect=released), \
                mock.patch.object(schedule, "candidate_take", side_effect=taken):
            code, out = self.candidate(fake, wait=True)
        self.assertEqual(0, code, out)
        self.assertEqual([True] * 3, got_back)
        self.assertIn("EQUAL 4/4", out)
        self.assertEqual([], self.held())

    def test_the_queue_place_of_an_ended_candidate_frees_the_lane(self):
        """w2 batchgolden (#1639), 24.11.: `w2/candidate:2` -- a candidate with
        `--wait-admission` whose process had ended -- held build03 as an offer
        from 16:05 on. The place of a candidate that ended goes with the next
        take, also within the TTL."""
        take = ["take", "--strand", "welle-c/x", "--lane", "build03"]
        code, _, err = schedule.run_lanes(take, env=self.env, cwd=str(self.repo))
        self.assertEqual(0, code, err)
        prog = ("import json, sys; sys.path.insert(0, sys.argv[1]); "
                "from scripts.pipeline import schedule; "
                "schedule.select.select = lambda *a: sys.exit(9); "   # blocked here: it ends
                "schedule.candidate_take('welle-c', sys.argv[2], 1, 'build03', "
                "env=json.loads(sys.argv[3]), cwd=sys.argv[4], wait=True)")
        res = subprocess.run(
            [sys.executable, "-c", prog, str(pathlib.Path(__file__).resolve().parents[2]),
             self.rev[:12], json.dumps(self.env), str(self.repo)],
            capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(9, res.returncode, res.stderr)
        self.assertEqual([schedule.candidate_key("welle-c", self.rev[:12], 1)], self.held())
        code, _, err = schedule.run_lanes(["release", "--strand", "welle-c/x"], env=self.env,
                                          cwd=str(self.repo))
        self.assertEqual(0, code, err)
        take[2] = "welle-c/y"
        code, _, err = schedule.run_lanes(take, env=self.env, cwd=str(self.repo))
        self.assertEqual(0, code, err)
        self.assertEqual([], self.held())

    def test_a_failed_build_gives_its_lane_back_before_the_others_end(self):
        fake = FakeLanes(broken={"build02"})
        back = threading.Event()
        release, real = schedule.candidate_release, fake.build_candidate
        seen = []

        def released(*args, **kw):
            res = release(*args, **kw)
            back.set()
            return res

        def build(lane, repo_root, rev, fresh=False):
            if lane.name != "build02":
                seen.append(back.wait(kit_time.HANG))
            return real(lane, repo_root, rev, fresh=fresh)
        fake.build_candidate = build
        with mock.patch.object(schedule, "candidate_release", side_effect=released):
            code, out = self.candidate(fake, lanes=("build01", "build02"))
        self.assertEqual(5, code, out)
        self.assertEqual([True], seen)
        self.assertEqual([], self.held())

    def test_the_rebuild_of_a_deviating_lane_is_leased_again(self):
        fake = FakeLanes(builds={"build03": [OTHER, BINARY]})
        real, held = fake.build_candidate, []

        def build(lane, repo_root, rev, fresh=False):
            if fresh:
                held.extend(self.held())
            return real(lane, repo_root, rev, fresh=fresh)
        fake.build_candidate = build
        code, out = self.candidate(fake, wait=True)
        self.assertEqual(0, code, out)
        self.assertEqual([schedule.candidate_key("welle-c", self.rev[:12], 3)], held)
        self.assertEqual([], self.held())

    def test_a_rebuild_whose_lane_is_gone_keeps_the_deviation_in_the_receipt(self):
        # without --wait-admission the lane of a deviating build may be leased
        # on before its rebuild: exit 3, and the finding "stale target" stays
        release = schedule.candidate_release
        others = []

        def released(wave_name, r12, n, **kw):
            res = release(wave_name, r12, n, **kw)
            if n == 3:
                others.append(schedule.run_lanes(["take", "--strand", "other/s", "--lane", "build03"],
                                                 env=self.env, cwd=str(self.repo)))
            return res
        fake = FakeLanes(builds={"build02": [OTHER, BINARY], "build03": [OTHER]})
        with mock.patch.object(schedule, "candidate_release", side_effect=released):
            code, out = self.candidate(fake)
        self.assertEqual(3, code, out)
        self.assertEqual([0], [c for c, _, _ in others], others)
        self.assertIn(("build02", True), fake.calls)
        self.assertNotIn(("build03", True), fake.calls)
        rec = self.receipt()
        self.assertEqual("queued", rec["verdict"])
        # build02 built equal from its empty target, build03 still differs
        self.assertEqual((["build03"], sha(BINARY)), (rec.get("deviating"), rec.get("reference")))
        self.assertEqual([], self.held())

    def test_two_revs_of_one_wave_build_side_by_side(self):
        new = self.second()
        th, got, go = self.started(FakeLanes(), lanes=("build01",))
        code, out = self.candidate(FakeLanes(), lanes=("build02",), rev=new)
        self.assertEqual(0, code, out)
        self.assertEqual([schedule.candidate_key("welle-c", self.rev[:12], 1)], self.held())
        go.set()
        th.join(kit_time.HANG)
        self.assertEqual([0], [c for c, _ in got], got)
        self.assertEqual([], self.held())

    def test_a_second_run_of_one_rev_waits(self):
        th, got, go = self.started(FakeLanes(), lanes=("build01",))
        # without --wait-admission: exit 3 as before
        code, out = self.candidate(FakeLanes(), lanes=("build02",))
        self.assertEqual(3, code, out)
        # with it: queued behind the first, then its own verdict
        fake, second, order, met = FakeLanes(), [], [], []
        at_lock = threading.Event()
        flock, finish, real = candidate.fcntl.flock, candidate._Run.finish, fake.build_candidate

        def lock(fh, flags):
            if flags & fcntl.LOCK_NB or not str(getattr(fh, "name", "")).startswith(
                    self.env["MECLAW_CANDIDATE_LOCK"]):
                return flock(fh, flags)
            # the second run's blocking lock: is it held when the run comes to it?
            try:
                flock(fh, flags | fcntl.LOCK_NB)
                met.append("free")
            except BlockingIOError:
                met.append("held")
            at_lock.set()
            return flock(fh, flags)

        def finished(run, verdict, *args, **kw):
            order.append((run.lanes[0], verdict))
            return finish(run, verdict, *args, **kw)

        def build(lane, repo_root, rev, fresh=False):
            order.append((lane.name, "build"))
            return real(lane, repo_root, rev, fresh=fresh)
        fake.build_candidate = build
        with mock.patch.object(candidate.fcntl, "flock", side_effect=lock), \
                mock.patch.object(candidate._Run, "finish", finished):
            th2 = threading.Thread(target=lambda: second.append(
                self.candidate(fake, lanes=("build02",), wait=True)), daemon=True)
            self.addCleanup(th2.join, kit_time.HANG)
            th2.start()
            self.assertTrue(at_lock.wait(kit_time.HANG), "HANG: the second run never came to the lock")
            # the first run is still in its build: the lock is its own
            self.assertEqual(["held"], met)
            go.set()
            th.join(kit_time.HANG)
            th2.join(kit_time.HANG)
        self.assertEqual([0], [c for c, _ in got], got)
        self.assertEqual([0], [c for c, _ in second], second)
        # the second builds only when the first has its verdict
        self.assertEqual([("build01", "EQUAL"), ("build02", "build"), ("build02", "EQUAL")], order)
        self.assertEqual([], self.held())

    def test_the_lease_names_its_rev_and_an_old_name_still_reads(self):
        self.assertEqual("w2/candidate:%s-1" % ("a" * 12),
                         schedule.candidate_key("w2", "a" * 12, 1))
        # an entry an older kit took: `who` names it, `release` frees it
        for verb in (["take", "--lane", "build01"], ["release"]):
            argv = verb[:1] + ["--strand", "welle-c/candidate", "--holder", "1"] + verb[1:]
            code, out, err = schedule.run_lanes(argv, env=self.env, cwd=str(self.repo))
            self.assertEqual(0, code, out + err)
            if verb[0] == "take":
                _, out, err = schedule.run_lanes(["who"], env=self.env, cwd=str(self.repo))
                self.assertIn("welle-c/candidate:1", out + err)
                code, out = self.candidate(FakeLanes(), lanes=("build02",))
                self.assertEqual(0, code, out)
                self.assertEqual(["welle-c/candidate:1"], self.held())
        self.assertEqual([], self.held())
        code, out = self.candidate(FakeLanes())
        self.assertEqual([schedule.candidate_key("welle-c", self.rev[:12], n) for n in (1, 2, 3, 4)],
                         self.receipt()["leases"], out)


class ReuseCase(CandidateCase):
    """w1 kandidat (#1606, OR-54s-62): a candidate whose `core_sha` differs
    from `<rev>` in no build input (`candidate.BUILD_INPUTS`) is not built
    again -- the same binary goes under the new name, `reused_from` says whence."""

    def commit(self, path, text="x\n"):
        full = self.repo / path
        full.parent.mkdir(parents=True, exist_ok=True)
        full.write_text(text)
        subprocess.run(GIT + ["-C", str(self.repo), "add", "-A"], check=True)
        subprocess.run(GIT + ["-C", str(self.repo), "commit", "-q", "-m", path], check=True)
        return subprocess.run(["git", "-C", str(self.repo), "rev-parse", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()

    def old_candidate(self):
        self.bin_dir.mkdir(parents=True, exist_ok=True)
        path = self.bin_dir / ("meclaw-%s" % self.rev[:12])
        path.write_bytes(BINARY)
        path.chmod(0o755)
        (self.bin_dir / ("meclaw-%s.origin.json" % self.rev[:12])).write_text(json.dumps(
            {"core_sha": self.rev, "sha256": sha(BINARY), "lane": "build01", "cargo": "cargo 1.95.0",
             "version": "meclaw 9.9.9"}, indent=1) + "\n")

    def test_no_moved_build_input_reuses_the_candidate(self):
        self.old_candidate()
        new = self.commit("docs/x.md")
        fake = FakeLanes()
        code, out = self.candidate(fake, rev=new)
        self.assertEqual(0, code, out)
        self.assertEqual([], fake.calls)  # no build, no lane queue
        self.assertEqual([], self.held())
        dest = self.bin_dir / ("meclaw-%s" % new[:12])
        self.assertEqual(BINARY, dest.read_bytes())
        origin = json.loads((self.bin_dir / ("meclaw-%s.origin.json" % new[:12])).read_text())
        self.assertEqual((new, self.rev, sha(BINARY)),
                         (origin["core_sha"], origin["reused_from"], origin["sha256"]))
        self.assertEqual(sorted((ln, "meclaw-%s" % new[:12]) for ln in LANES), sorted(fake.pushes))
        self.assertRegex(out, r"(?m)^CANDIDATE %s REUSED from %s" % (new[:12], self.rev[:12]))
        with open(self.receipts / ("candidate-%s.json" % new[:12]), encoding="utf-8") as fh:
            rec = json.load(fh)
        self.assertEqual(("REUSED", self.rev, 0), (rec["verdict"], rec["reused_from"], rec["exit"]))

    def test_a_moved_file_below_crates_builds(self):
        self.old_candidate()
        new = self.commit("crates/meclaw/src/x.rs")
        fake = FakeLanes()
        code, out = self.candidate(fake, rev=new)
        self.assertEqual(0, code, out)
        self.assertEqual(sorted(LANES), sorted(ln for ln, _ in fake.calls))
        self.assertNotIn("reused_from", json.loads(
            (self.bin_dir / ("meclaw-%s.origin.json" % new[:12])).read_text()
            if (self.bin_dir / ("meclaw-%s.origin.json" % new[:12])).exists() else "{}"))
        self.assertEqual([], fake.pushes)


# A fixed instant: the age of a candidate is data, never the machine's clock (TV-K1).
PRUNE_NOW = 1_800_000_000


class PruneCase(CandidateCase):
    """`strand.sh candidate --prune`: a candidate no set of the three
    repositories names and older than 14 days goes, here and on the lanes."""

    def put(self, name, days):
        self.bin_dir.mkdir(parents=True, exist_ok=True)
        for n in (name, name + ".origin.json"):
            p = self.bin_dir / n
            p.write_text("x\n")
            stamp = PRUNE_NOW - days * 86400
            os.utime(p, (stamp, stamp))

    def test_prune_keeps_named_and_young_candidates(self):
        named, legacy = "b" * 40, "c" * 40
        soul = self.tmp / "soul2me"
        (soul / "sets").mkdir(parents=True)
        (soul / "sets" / "0.73.json").write_text(json.dumps(
            {"meclaw_core": legacy[:9] + " (x)", "meclaw_candidate": {"rev": named, "sha256": "0" * 64}}))
        orga = self.tmp / "orga-hive"
        orga.mkdir()  # no sets/ directory: names nothing
        self.put("meclaw-" + "a" * 12, 20)
        self.put("meclaw-" + named[:12], 20)
        self.put("meclaw-" + legacy[:12], 20)
        self.put("meclaw-" + "d" * 12, 2)
        self.put("meclaw-0.86.1-asset", 20)
        fake = FakeLanes()
        out = io.StringIO()
        code = candidate.prune(repo=str(self.repo), set_roots=[str(self.repo), str(soul), str(orga)],
                               lanes=["build01", "build02"], bin_dir=str(self.bin_dir), out=out,
                               lanes_mod=fake, now=PRUNE_NOW)
        self.assertEqual(0, code, out.getvalue())
        left = sorted(os.listdir(self.bin_dir))
        self.assertNotIn("meclaw-" + "a" * 12, left)
        self.assertNotIn("meclaw-%s.origin.json" % ("a" * 12), left)
        for keep in (named[:12], legacy[:12], "d" * 12, "0.86.1-asset"):
            self.assertIn("meclaw-" + keep, left)
        self.assertEqual(["build01", "build02"], [ln for ln, _, _ in fake.pruned])
        self.assertIn(named[:12], fake.pruned[0][1])
        self.assertIn("PRUNED %s" % ("a" * 12), out.getvalue())

    def test_prune_refuses_without_a_set_repository(self):
        self.put("meclaw-" + "a" * 12, 20)
        fake = FakeLanes()
        out = io.StringIO()
        code = candidate.prune(repo=str(self.repo), set_roots=[str(self.tmp / "gone")],
                               lanes=["build01"], bin_dir=str(self.bin_dir), out=out, lanes_mod=fake)
        self.assertEqual(2, code)
        self.assertIn("meclaw-" + "a" * 12, os.listdir(self.bin_dir))
        self.assertEqual([], fake.pruned)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
