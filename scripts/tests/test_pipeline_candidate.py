"""Tests for `pipeline.py candidate <rev>` (`strand.sh candidate`, GH #1389).

A candidate is one commit built on every named lane, each lane under its own
lease `<wave>/candidate:<n>` taken through the normal admission, in its own
target (`<root>/kb-target`, `--locked`); the sha256 of the binary must be the
same on all of them (OR-LV-47: build02 built differently for weeks from a stale
warm target, unseen because only one lane built). The binary then comes home
from the first equal lane and is verified again.

The lanes are faked (`FakeLanes`: build, fetch); the leases are real, taken in
a throw-away store armed on the four lane names, the lane tree probe of a
release answers "nothing lives there" (`schedule._lane_tree_probe`).
"""
import dataclasses
import hashlib
import io
import json
import os
import pathlib
import re
import sqlite3
import stat
import subprocess
import tempfile
import unittest

from scripts.pipeline import candidate, schedule
from scripts.tests import store_isolation

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

    def candidate(self, fake, lanes=LANES, wait=False):
        out = io.StringIO()
        code = candidate.run(self.rev, repo=str(self.repo), lanes=list(lanes), wave=WAVE,
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


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
