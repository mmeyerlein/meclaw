"""Tests for `pipeline.py lanes ...`: the cargo token of `strand.sh token` on the store.

The cases are the token cases of `test_strand_sh.py` (classes `TokenTestCase`,
`TestToken`, `TestTokenLanes`, `TestTokenGuards`, `TestLanesStatus`), carried
over to the store: holders are `leases` rows, the queue is `lease_entries`, the
log is the journal. Most cases run the verb in-process (`schedule.run_lanes`,
the same code path as the CLI) to stay fast; the binding case, the concurrent
takes and `--wait` run the real `python3 scripts/pipeline.py lanes ...`.
"""
import dataclasses
import inspect
import io
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock
from typing import Optional

from scripts.pipeline import schedule
from scripts.tests import kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
PIPELINE = REPO / "scripts" / "pipeline.py"
WAVE = "welle-x"

LANES_FILE = """# name  ssh target  [bmc]
north  builder@north.example.invalid  bmc-north.example.invalid
south  builder@south.example.invalid
spare  builder@spare.example.invalid  threads=8
"""

GIT = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid",
       "-c", "init.defaultBranch=master", "-c", "commit.gpgsign=false"]


def git_repo(path, branch):
    path.mkdir(parents=True)
    subprocess.run(GIT + ["init", "-q", "-b", branch, str(path)], check=True)
    subprocess.run(GIT + ["-C", str(path), "commit", "-q", "--allow-empty", "-m", "x"], check=True)
    return path


class Res:
    def __init__(self, returncode, stdout, stderr):
        self.returncode, self.stdout, self.stderr = returncode, stdout, stderr


class TokenTestCase(unittest.TestCase):
    T0 = 1_800_000_000

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        self.db_path = self.tmp / "state" / "pipeline.db"
        self.lanes_file = self.tmp / "lanes"
        self.main = git_repo(self.tmp / "main", "master")
        self._tree = None

    def tearDown(self):
        self._tmp.cleanup()

    @property
    def tree(self):
        """A standing strand tree: a checkout on the branch `welle-x/kit`."""
        if self._tree is None:
            self._tree = git_repo(self.tmp / "kit", "%s/kit" % WAVE)
        return self._tree

    def env(self, minutes=0, extra=None):
        # The old token file is a throw-away path too (Z11): the default is
        # the host's /tmp file, which no case here may read.
        env = {"MECLAW_PIPELINE_DB": str(self.db_path),
               "MECLAW_LANES_FILE": str(self.lanes_file),
               "MECLAW_STRAND_TOKENS": str(self.tmp / "tokens"),
               "MECLAW_STRAND_NOW": str(self.T0 + 60 * minutes)}
        env.update(extra or {})
        return env

    def token(self, *args, cwd=None, minutes=0, env=None):
        code, out, err = schedule.run_lanes(list(args), env=self.env(minutes, env),
                                            cwd=str(cwd or self.main))
        return Res(code, out, err)

    def cli(self, *args, cwd=None, minutes=0, env=None, timeout=2 * kit_time.HANG):
        full = dict(os.environ)
        full.update(self.env(minutes, env))
        res = subprocess.run([sys.executable, str(PIPELINE), "lanes"] + list(args),
                             cwd=str(cwd or self.main), env=full, capture_output=True,
                             text=True, timeout=timeout)
        return Res(res.returncode, res.stdout, res.stderr)

    def arm(self, *args):
        res = self.token("init", *args)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def take(self, strand, minutes=0, cwd=None):
        args = ("take",) if strand is None else ("take", "--strand", strand)
        return self.token(*args, cwd=cwd, minutes=minutes)

    def take_lane(self, strand, lane, minutes=0):
        return self.token("take", "--strand", strand, "--lane", lane, minutes=minutes)

    def _db(self):
        db = sqlite3.connect(str(self.db_path))
        db.row_factory = sqlite3.Row
        return db

    def holders(self):
        with self._db() as db:
            return [r["holder"] for r in db.execute(
                "SELECT holder FROM lease_entries WHERE resource IS NOT NULL ORDER BY seq")]

    def holder_lanes(self):
        with self._db() as db:
            return [r["resource"] for r in db.execute(
                "SELECT resource FROM lease_entries WHERE resource IS NOT NULL ORDER BY seq")]

    def waiting(self):
        with self._db() as db:
            return [r["holder"] for r in db.execute(
                "SELECT holder FROM lease_entries WHERE resource IS NULL ORDER BY seq")]

    def log(self):
        """The journal's lease lines in the old log form: `<at>\\t<verb>\\t<strand>\\t<branch>\\t<detail>`."""
        if not self.db_path.exists():
            return ""
        with self._db() as db:
            rows = db.execute("SELECT at, data FROM journal WHERE event LIKE 'lease_%' ORDER BY seq")
            lines = []
            for row in rows:
                d = json.loads(row["data"])
                lines.append("\t".join([row["at"], d.get("verb", ""), d.get("holder", ""),
                                        d.get("branch") or "-", d.get("detail", "")]))
        return "\n".join(lines) + "\n"

    def events(self, event):
        with self._db() as db:
            return [json.loads(r["data"]) for r in db.execute(
                "SELECT data FROM journal WHERE event = ? ORDER BY seq", (event,))]

    def utc(self, minutes):
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(self.T0 + 60 * minutes))


class TestToken(TokenTestCase):
    def test_lanes_cli_keeps_token_semantics(self):
        """Queue, TTL, offered, release -- through the real entry point."""
        res = self.cli("init", "--max", "1")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: cargo tokens armed: 1, ttl 90 min", res.stderr)
        self.assertEqual(0, self.cli("take", "--strand", "welle-x/a").returncode)
        res = self.cli("take", "--strand", "welle-x/b", minutes=2)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("all 1 cargo tokens are held (welle-x/a 2 min) -- welle-x/b is #1 in the "
                      "queue. End your turn; you are woken when a token is free.", res.stderr)
        res = self.cli("release", "--strand", "welle-x/a", minutes=23)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: cargo token released by welle-x/a after 23 min -- 0/1 held; "
                      "next in the queue: welle-x/b (waiting 21 min) (released by master)",
                      res.stderr)
        # the free token is offered to welle-x/b; within the TTL it keeps it
        res = self.cli("who", minutes=24)
        self.assertIn("#1 welle-x/b  waiting 22 min  offered a token at %s" % self.utc(23),
                      res.stdout)
        self.assertEqual(1, len(self.events("lease_offered")))
        # beyond the TTL a waiter queued from outside (no tree) drops out -- strand.sh's
        # stale-wait comes before the offer rule, 1:1
        res = self.cli("take", "--strand", "welle-x/c", minutes=105)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("dropped welle-x/b from the queue -- waiting 103 min, nobody took it up "
                      "(ttl 90 min)", res.stderr)
        # TTL of a holder: c is reclaimed after 90 min without a sign of life
        res = self.cli("take", "--strand", "welle-x/d", minutes=196)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("reclaimed the cargo token of welle-x/c -- last seen 91 min ago, "
                      "no live process (ttl 90 min)", res.stderr)
        self.assertEqual(["welle-x/d"], self.holders())
        verbs = [line.split("\t")[1] for line in self.log().splitlines()]
        self.assertEqual(["init", "take", "wait", "offer", "release-by", "stale-wait", "take",
                          "stale", "take"], verbs)
        taken = self.events("lease_taken")
        self.assertEqual({"resource": "token:1", "holder": "welle-x/a", "run": None, "ttl_s": 5400},
                         {k: taken[0][k] for k in ("resource", "holder", "run", "ttl_s")})

    def test_take_grants_up_to_max_and_queues_the_rest(self):
        self.arm("--max", "2")
        self.assertEqual(0, self.take("welle-x/a").returncode)
        self.assertEqual(0, self.take("welle-x/b").returncode)
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("all 2 cargo tokens are held", res.stderr)
        self.assertIn("welle-x/c is #1 in the queue", res.stderr)
        self.assertIn("End your turn", res.stderr)
        self.assertEqual(["welle-x/a", "welle-x/b"], self.holders())
        self.assertEqual(["welle-x/c"], self.waiting())

    def test_the_queue_is_first_come_first_served(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.assertEqual(3, self.take("welle-x/b").returncode)
        self.assertEqual(3, self.take("welle-x/c").returncode)
        self.token("release", "--strand", "welle-x/a")
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, "a later strand overtook the queue")
        self.assertIn("welle-x/c is #2 in the queue", res.stderr)
        self.assertEqual(0, self.take("welle-x/b").returncode)
        self.assertEqual(["welle-x/b"], self.holders())

    def test_concurrent_takes_never_exceed_max(self):
        """Eight takes at once against three tokens: exactly three win."""
        self.arm("--max", "3")
        env = dict(os.environ)
        env.update(self.env())
        procs = [subprocess.Popen(
            [sys.executable, str(PIPELINE), "lanes", "take", "--strand", "welle-x/s%d" % i],
            cwd=str(self.main), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            for i in range(8)]
        codes = sorted(p.wait(timeout=2 * kit_time.HANG) for p in procs)
        for p in procs:
            p.stdout.close()
            p.stderr.close()
        self.assertEqual([0, 0, 0, 3, 3, 3, 3, 3], codes)
        self.assertEqual(3, len(self.holders()))
        self.assertEqual(5, len(self.waiting()))

    def test_a_stale_holder_is_reclaimed(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.take("welle-x/b", minutes=91)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("reclaimed the cargo token of welle-x/a -- last seen 91 min "
                      "ago, no live process (ttl 90 min)", res.stderr)
        self.assertEqual(["welle-x/b"], self.holders())
        self.assertIn("\tstale\twelle-x/a", self.log())

    def test_a_holder_with_a_live_process_is_not_stale(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(3, self.take("welle-x/b", minutes=120).returncode)
        self.assertEqual(["welle-x/a"], self.holders())

    def pids_of(self, holder):
        with self._db() as db:
            row = db.execute("SELECT pids FROM lease_entries WHERE holder = ? AND resource IS NOT NULL",
                             (holder,)).fetchone()
        return json.loads(row["pids"] or "[]") if row else None

    def test_take_with_a_pid_keeps_a_holder_lease_alive_past_its_ttl(self):
        """Kit find "holder push" (Welle Testing, 10/12, build02 tv-br:m2): a
        run on the lane of a holder lease `S:n` carried no PID into it --
        `take --pid` was dropped, only `check --pid` set one -- and the lease
        fell away after its TTL under the running load; the next take got the
        lane twice. Now `take --pid` enters the PID (also on a take that
        already holds), and a lease with a live process is never stale."""
        self.lanes_file.write_text(LANES_FILE)
        self.arm("--lanes", "north,south")
        me = os.getpid()
        res = self.token("take", "--strand", "welle-x/a", "--holder", "m2", "--lane", "north", "--pid", str(me))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([me], self.pids_of("welle-x/a:m2"))
        res = self.take_lane("welle-x/b", "north", minutes=120)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertNotIn("welle-x/b", self.holders())
        # the second take of a holder that holds already enters its PID as well
        self.token("take", "--strand", "welle-x/c", "--holder", "n", "--lane", "south")
        self.assertEqual([], self.pids_of("welle-x/c:n"))
        res = self.token("take", "--strand", "welle-x/c", "--holder", "n", "--lane", "south", "--pid", str(me),
                         minutes=1)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([me], self.pids_of("welle-x/c:n"))

    def test_a_test_run_of_the_strand_enters_its_pid_into_the_lease_on_its_lane(self):
        """`strand.sh test --strand S --host <lane>` on the lane of `S:n`: the
        run's PID goes into exactly that lease (before: no lease of `S` -- the
        run was refused, or with a skip reason entered nothing)."""
        from scripts.pipeline import gate, store
        self.lanes_file.write_text(LANES_FILE)
        self.arm("--lanes", "north,south")
        res = self.token("take", "--strand", "%s/kit" % WAVE, "--holder", "sat", "--lane", "south", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.pids_of("%s/kit:sat" % WAVE))
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, self.env()):
            db = store.open()
            self.addCleanup(db.close)
            host = gate._lease_gate(db, str(self.tree), "%s/kit" % WAVE, "south", out, err)
            self.assertEqual("south", host, err.getvalue())
            self.assertEqual([os.getpid()], self.pids_of("%s/kit:sat" % WAVE))
            # another lane is still refused: the second lease does not hold it
            # (exit 3, the refusal names the strand that holds no lease there)
            refused = io.StringIO()
            with self.assertRaisesRegex(gate._TestExit, "^$") as cm:
                gate._lease_gate(db, str(self.tree), "%s/kit" % WAVE, "north", out, refused)
            self.assertEqual(3, cm.exception.code)
            self.assertIn("no cargo token for %s/kit -- " % WAVE, refused.getvalue())

    def test_take_with_a_dead_pid_enters_nothing(self):
        """The mutant beside it: a dead PID keeps nothing alive."""
        self.lanes_file.write_text(LANES_FILE)
        self.arm("--lanes", "north,south")
        dead = subprocess.Popen(["true"])
        dead.wait(kit_time.HANG)
        res = self.token("take", "--strand", "welle-x/a", "--holder", "m2", "--lane", "north",
                         "--pid", str(dead.pid))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.pids_of("welle-x/a:m2"))

    def test_a_holder_whose_worktree_is_gone_is_stale(self):
        self.arm("--max", "1")
        res = self.take(None, cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["%s/kit" % WAVE], self.holders())
        subprocess.run(["rm", "-rf", str(self.tree)], check=True)
        res = self.take("welle-x/b")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("its worktree is gone", res.stderr)

    def test_a_dead_waiter_does_not_block_the_queue(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.assertEqual(3, self.take("welle-x/w").returncode)
        self.token("check", "--strand", "welle-x/a", minutes=94)
        self.token("release", "--strand", "welle-x/a", minutes=95)
        res = self.take("welle-x/c", minutes=95)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.waiting())
        self.assertIn("\tstale-wait\twelle-x/w", self.log())

    def test_a_waiter_with_a_live_worktree_never_expires(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        res = self.take(None, cwd=self.tree)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("%s/kit is #1 in the queue" % WAVE, res.stderr)
        self.assertEqual(3, self.take("welle-x/x", minutes=10).returncode)
        res = self.take("welle-x/x", minutes=95)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertNotIn("dropped", res.stderr)
        self.assertEqual(["%s/kit" % WAVE, "welle-x/x"], self.waiting())
        res = self.token("release", "--strand", "welle-x/a", minutes=96)
        self.assertIn("next in the queue: %s/kit" % WAVE, res.stderr)
        res = self.take(None, cwd=self.tree, minutes=97)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["%s/kit" % WAVE], self.holders())

    def test_an_unarmed_host_leaves_no_trace(self):
        """Unarmed is without a trace: no verb but `init` creates the store.
        `take` refuses there (Z11, below); the others pass."""
        for args in (("release", "--strand", "welle-x/a"),
                     ("check", "--strand", "welle-x/a", "--pid", "0"),
                     ("lane", "--strand", "welle-x/a"),
                     ("who",), ("off",), ("offer",)):
            res = self.token(*args)
            self.assertEqual(0, res.returncode, "%s: %s" % (args, res.stderr))
        self.assertFalse(self.db_path.exists())
        self.assertIn("cargo tokens are not armed on this host", self.token("who").stdout)
        self.assertEqual(2, self.take("welle-x/a").returncode)
        self.assertFalse(self.db_path.exists())

    def test_an_armed_then_switched_off_store_is_unarmed(self):
        self.arm("--max", "1")
        self.assertEqual(0, self.token("off").returncode)
        res = self.take("welle-x/a")
        self.assertEqual(2, res.returncode)
        self.assertIn("cargo tokens are not armed on this host", res.stderr)

    def _gate_and_tier(self, first, second):
        self.token("init", "--max", "1", "--force")
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        for pid in (first, second):
            res = self.token("check", "--strand", "%s/kit" % WAVE, "--pid", str(pid))
            self.assertEqual(0, res.returncode, res.stderr)

    def test_a_single_test_beside_a_running_gate_keeps_the_gates_pid(self):
        gate = os.getpid()
        for order in ("tier after the gate", "gate after the tier"):
            with self.subTest(order=order):
                tier = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
                try:
                    pair = (gate, tier.pid) if order.startswith("tier") else (tier.pid, gate)
                    self._gate_and_tier(*pair)
                finally:
                    tier.kill()
                    tier.wait()
                res = self.take("welle-x/b", minutes=120)
                self.assertEqual(3, res.returncode, res.stderr)
                self.assertEqual(["%s/kit" % WAVE], self.holders())

    def test_the_caller_takes_back_its_own_pid(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        caller = {"MECLAW_LANES_CALLER": str(os.getpid())}
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        res = self.token("check", "--strand", "welle-x/a", "--pid", "0", env=caller)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(0, self.take("welle-x/b", minutes=91).returncode,
                         "the gate gave its pid back; the holder went stale")

    def test_the_heartbeat_after_a_foreign_release_logs_no_refusal(self):
        self.arm()
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        self.token("release", "--strand", "%s/kit" % WAVE)
        res = self.token("check", "--pid", "0", cwd=self.tree)
        self.assertEqual(3, res.returncode)
        self.assertEqual("", res.stderr)
        self.assertNotIn("\trefused\t", self.log())

    def test_check_without_a_token_refuses_with_exit_3(self):
        self.arm()
        res = self.token("check", "--pid", "4242", cwd=self.tree)
        self.assertEqual(3, res.returncode)
        self.assertIn("no cargo token for %s/kit -- run 'scripts/strand.sh token take' first."
                      % WAVE, res.stderr)
        self.assertIn("\trefused\t%s/kit" % WAVE, self.log())
        self.assertEqual(0, self.token("check").returncode, "the main tree needs no token")

    def test_init_refuses_over_waiters_without_force(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.token("release", "--strand", "welle-x/a")
        res = self.token("init", "--max", "2")
        self.assertEqual(2, res.returncode)
        self.assertIn("token init: welle-x/b waits in the queue -- pass --force to start over",
                      res.stderr)
        self.assertEqual(["welle-x/b"], self.waiting())
        self.assertEqual(0, self.token("init", "--max", "2", "--force").returncode)
        self.assertEqual([], self.waiting())

    def test_init_refuses_over_holders_without_force(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.token("init", "--max", "1")
        self.assertEqual(2, res.returncode)
        self.assertIn("welle-x/a holds a cargo token", res.stderr)

    def test_off_names_holders_and_waiters_together(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        res = self.token("off")
        self.assertEqual(2, res.returncode)
        self.assertIn("token off: welle-x/a holds a cargo token, welle-x/b waits in the queue "
                      "-- release them or pass --force", res.stderr)

    def test_off_disarms_and_keeps_the_log(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.token("off", "--force")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("cargo tokens switched off; the log stays", res.stderr)
        self.assertIn("\toff --force\t", self.log())
        self.assertIn("\ttake\twelle-x/a", self.log())
        self.assertEqual([], self.holders())
        self.assertIn("not armed", self.token("who").stdout)

    def test_who_marks_a_waiter_beyond_the_ttl_and_keeps_it(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        res = self.token("who", minutes=90)
        self.assertNotIn("beyond the ttl", res.stdout)
        res = self.token("who", minutes=95)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("#1 %s/kit  waiting 95 min  waiting beyond the ttl" % WAVE, res.stdout)
        self.assertNotIn("STALE", res.stdout)
        self.assertEqual(["%s/kit" % WAVE], self.waiting())

    def test_a_waiter_beyond_the_ttl_that_does_not_take_its_token_drops_out(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        self.assertEqual(3, self.take("welle-x/x", minutes=1).returncode)
        self.token("release", "--strand", "welle-x/a", minutes=100)
        res = self.token("who", minutes=101)
        self.assertIn("#1 %s/kit  waiting 101 min  waiting beyond the ttl  offered a token at %s"
                      % (WAVE, self.utc(100)), res.stdout)
        res = self.take("welle-x/x", minutes=111)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("dropped %s/kit from the queue -- waiter past ttl, did not take" % WAVE,
                      res.stderr)
        self.assertEqual(["welle-x/x"], self.holders())
        self.assertEqual([], self.waiting())
        self.assertRegex(self.log(), r"\tdropped\t%s/kit\t[^\t]*\twaiter past ttl, did not take" % WAVE)

    def test_a_waiter_whose_worktree_is_gone_leaves_the_queue(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        self.assertEqual(3, self.take("welle-x/x", minutes=1).returncode)
        subprocess.run(["rm", "-rf", str(self.tree)], check=True)
        res = self.take("welle-x/x", minutes=300)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("dropped %s/kit from the queue -- its worktree is gone" % WAVE, res.stderr)
        self.assertIn("welle-x/x is #1 in the queue", res.stderr)
        self.assertEqual(["welle-x/x"], self.waiting())
        self.assertIn("\tstale-wait\t%s/kit" % WAVE, self.log())

    def test_release_also_leaves_the_queue(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        res = self.token("release", "--strand", "welle-x/b")
        self.assertEqual(0, res.returncode)
        self.assertIn("welle-x/b left the queue (released by master)", res.stderr)
        self.assertEqual([], self.waiting())
        self.assertIn("\tleave-by\twelle-x/b", self.log())

    def test_the_skip_variable_is_logged(self):
        self.arm("--max", "1")
        res = self.token("check", "--strand", "welle-x/a",
                         env={"MECLAW_STRAND_TOKEN_SKIP": "one-off from the orchestrator"})
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("token check skipped for welle-x/a: one-off from the orchestrator", res.stderr)
        self.assertIn("\tskip\twelle-x/a", self.log())

    def test_who_names_holders_the_queue_and_the_last_events(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b", minutes=3)
        res = self.token("who", minutes=5)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("cargo tokens: 1/1 held, ttl 90 min, armed %s" % self.utc(0), res.stdout)
        self.assertIn("  welle-x/a  held 5 min, seen 5 min ago, pid -", res.stdout)
        self.assertIn("queue:\n  #1 welle-x/b  waiting 2 min", res.stdout)
        self.assertIn("last events:", res.stdout)
        self.assertIn("  %s  wait  welle-x/b" % self.utc(3), res.stdout)

    def test_who_on_an_empty_queue(self):
        self.arm("--max", "2")
        self.assertIn("queue: empty", self.token("who").stdout)

    def test_strand_names_need_a_wave(self):
        self.arm("--max", "1")
        res = self.token("take", "--strand", "kit")
        self.assertEqual(2, res.returncode)
        self.assertIn("token: --strand kit needs --wave, a strand branch, or the form "
                      "<wave>/<name>", res.stderr)
        res = self.token("take", "--strand", "kit", "--wave", "welle-x-2026-09-19")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["welle-x/kit"], self.holders())
        res = self.token("release", "--strand", "kit", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.holders())

    def test_take_without_a_strand_outside_a_strand_tree_refuses(self):
        self.arm("--max", "1")
        res = self.token("take")
        self.assertEqual(2, res.returncode)
        self.assertIn("token take: cannot tell the strand from the branch -- pass --strand",
                      res.stderr)

    def test_take_with_a_ttl_holds_longer(self):
        self.arm("--max", "1")
        self.token("take", "--strand", "welle-x/a", "--ttl", "180")
        self.assertEqual(3, self.take("welle-x/b", minutes=120).returncode)
        self.assertEqual(0, self.take("welle-x/b", minutes=181).returncode)


class TestTakeFailsClosed(TokenTestCase):
    """Z11: `take` on an unarmed host is an error, never a silent pass.

    Measured 2026-10-09: after the W3 merge the token verbs ran on the store,
    the old token file was not carried over, and `token take` said "not armed"
    with exit 0 -- the lanes ran unserialised for twelve minutes. A strand asks
    for a token to serialise its cargo; when nothing can serialise it, the
    answer is an exit != 0 and the way out on stderr.
    """

    def test_take_on_an_unarmed_host_fails_and_names_the_way_out(self):
        res = self.take("welle-x/a")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("cargo tokens are not armed on this host", res.stderr)
        self.assertIn("strand.sh token init --lanes", res.stderr)
        self.assertIn("--ttl", res.stderr)
        self.assertEqual("", res.stdout)

    def test_take_wait_on_an_unarmed_host_fails_at_once(self):
        # "At once" is work, not seconds: one round, and its end is no queue
        # (exit 3 alone blocks on the wake file and takes a second round).
        rounds = []
        real_once = schedule._run_once
        with mock.patch.object(schedule, "_run_once",
                               lambda *a: rounds.append(a[0]) or real_once(*a)):
            res = self.token("take", "--strand", "welle-x/a", "--wait",
                             env={"MECLAW_TOKEN_WAIT_SECS": "30"})
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("not armed", res.stderr)
        self.assertEqual(1, len(rounds), "the unarmed take waited a round")

    def test_take_on_an_existing_unarmed_store_fails(self):
        self.arm("--max", "1")
        self.assertEqual(0, self.token("off").returncode)
        res = self.take("welle-x/a")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertEqual([], self.holders())

    def test_the_cli_exit_is_the_refusal(self):
        res = self.cli("take", "--strand", "welle-x/a")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("token init", res.stderr)

    def test_the_other_verbs_stay_quiet_on_an_unarmed_host(self):
        for args in (("release", "--strand", "welle-x/a"),
                     ("check", "--strand", "welle-x/a", "--pid", "0"),
                     ("check", "--strand", "welle-x/a"),
                     ("lane", "--strand", "welle-x/a"), ("who",), ("off",)):
            self.assertEqual(0, self.token(*args).returncode, args)


class TestOldTokenFileMigration(TokenTestCase):
    """Z11: the old token file (`MECLAW_STRAND_TOKENS`, else next to the cargo
    lock) is carried into an unarmed store -- lanes, max, TTL, holders and the
    queue -- once; a file that cannot be carried over is an error."""

    def setUp(self):
        super().setUp()
        self.lanes_file.write_text(LANES_FILE)
        self.old = self.tmp / "tokens"

    def write_old(self, **over):
        st = {"max": 2, "ttl_min": 45, "armed": self.utc(-120),
              "holders": [{"strand": "welle-x/a", "since": self.T0 - 600,
                           "seen": self.T0 - 60, "pids": [], "tree": "", "lane": "north"}],
              "waiting": [], "lanes": ["north", "south"]}
        st.update(over)
        self.old.write_text(json.dumps(st, indent=2) + "\n")
        return self.old.read_bytes()

    def pool(self):
        if not self.db_path.exists():
            return None
        with self._db() as db:
            row = db.execute("SELECT * FROM lease_pools WHERE pool = 'cargo'").fetchone()
            return None if row is None else dict(row)

    def test_take_carries_lanes_ttl_and_holders_over(self):
        before = self.write_old()
        res = self.take("welle-x/b")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("migrated", res.stderr)
        self.assertIn(str(self.old), res.stderr)
        self.assertEqual("lane south\n", res.stdout)
        pool = self.pool()
        self.assertEqual((2, 45, ["north", "south"], self.utc(-120)),
                         (pool["max"], pool["ttl_min"], json.loads(pool["lanes"]), pool["armed"]))
        self.assertEqual(["welle-x/a", "welle-x/b"], self.holders())
        self.assertEqual(["lane:north", "lane:south"], self.holder_lanes())
        self.assertEqual(before, self.old.read_bytes())      # the old file is only read
        self.assertIn("migrate", [e["verb"] for e in self.events("lease_noted")])

    def test_a_migrated_pool_serialises(self):
        self.write_old(max=1, lanes=None, holders=[
            {"strand": "welle-x/a", "since": self.T0 - 600, "seen": self.T0 - 60,
             "pid": 0, "tree": ""}])
        res = self.take("welle-x/b")
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertEqual(["welle-x/a"], self.holders())
        self.assertEqual(["welle-x/b"], self.waiting())

    def test_the_queue_comes_along(self):
        self.write_old(waiting=[{"strand": "welle-x/c", "since": self.T0 - 300,
                                 "tree": "", "lane": "north"}])
        res = self.token("who")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["welle-x/c"], self.waiting())
        self.assertIn("welle-x/c", res.stdout)
        self.assertIn("1/2 held", res.stdout)

    def test_the_old_file_is_carried_over_once(self):
        self.write_old()
        self.assertEqual(0, self.token("who").returncode)
        self.assertEqual(0, self.token("off", "--force", minutes=1).returncode)
        res = self.take("welle-x/b", minutes=2)
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("not armed", res.stderr)
        self.assertIsNone(self.pool())

    def test_an_init_after_the_old_file_wins(self):
        self.write_old()
        self.assertEqual(0, self.token("init", "--max", "1").returncode)
        self.assertEqual(0, self.token("off").returncode)
        self.assertEqual(2, self.take("welle-x/b", minutes=1).returncode)

    def test_an_armed_store_ignores_the_old_file(self):
        self.write_old()
        self.arm("--max", "1")
        self.assertEqual(0, self.take("welle-x/b").returncode)
        self.assertEqual(["welle-x/b"], self.holders())

    def test_a_broken_old_file_fails_loudly(self):
        self.old.write_text("{not json")
        res = self.take("welle-x/b")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn(str(self.old), res.stderr)
        self.assertIn("cannot be carried over", res.stderr)
        self.assertEqual("{not json", self.old.read_text())
        self.assertIsNone(self.pool())

    def test_an_old_file_with_an_unknown_lane_fails_loudly(self):
        self.write_old(lanes=["north", "west"])
        res = self.take("welle-x/b")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("west", res.stderr)
        self.assertIn("cannot be carried over", res.stderr)
        self.assertIsNone(self.pool())

    # Z15 (review of dt-z11, M2): an old path that cannot be read at all (a
    # directory, no read right) takes its stamp from `stat`, never from now --
    # else an init+off before it never wins and every verb stays exit 2.
    def test_an_unreadable_old_path_does_not_outlive_an_off(self):
        self.old.mkdir()
        os.utime(self.old, (self.T0 - 3600, self.T0 - 3600))
        self.arm("--max", "1")
        self.assertEqual(0, self.token("off").returncode)
        for verb in (("check",), ("release", "--strand", "welle-x/a"), ("who",)):
            with self.subTest(verb=verb[0]):
                res = self.token(*verb, minutes=1)
                self.assertEqual(0, res.returncode, res.stderr)

    # Z15 (M5/M6): a migrated holder past its TTL is reclaimed -- pids 0 and
    # below are no process (`os.kill(0, 0)` asks the own process group).
    def test_a_migrated_holder_past_its_ttl_is_reclaimed(self):
        self.write_old(max=1, lanes=None, holders=[
            {"strand": "welle-x/a", "since": self.T0 - 7200, "seen": self.T0 - 3600,
             "pids": [0, -1], "tree": ""}])
        res = self.take("welle-x/b")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("reclaimed", res.stderr)
        self.assertEqual(["welle-x/b"], self.holders())

    # Z15 (M6): the default path beside `MECLAW_GATE_LOCK`, through a real verb.
    def test_the_old_file_beside_the_cargo_lock_is_migrated_by_a_verb(self):
        self.old = self.tmp / "cargo.tokens"
        self.write_old()
        res = self.token("take", "--strand", "welle-x/b",
                         env={"MECLAW_STRAND_TOKENS": "",
                              "MECLAW_GATE_LOCK": str(self.tmp / "cargo.lock")})
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("migrated", res.stderr)

    # Z15 (M6): a broken old file an off has outlived is no error any more.
    def test_a_broken_old_file_after_an_off_is_exit_0(self):
        self.old.write_text("{not json")
        os.utime(self.old, (self.T0 - 3600, self.T0 - 3600))
        self.arm("--max", "1")
        self.assertEqual(0, self.token("off").returncode)
        res = self.token("check", minutes=1)
        self.assertEqual(0, res.returncode, res.stderr)

    # Z15 (M4): after a store `off` the old file no longer arms the lanes of
    # the host for `meclaw_lanes_active` (test-tier, gate_legacy).
    def test_the_lanes_probe_ignores_an_old_file_the_store_outlived(self):
        self.write_old()

        def probe():
            env = dict(os.environ, **self.env())
            return subprocess.run(
                ["bash", "-c", '. "$1"; meclaw_lanes_active "$2"', "probe",
                 str(REPO / "scripts" / "cargo-target.sh"), str(self.tmp / "cargo.lock")],
                env=env, capture_output=True, text=True, timeout=kit_time.HANG).returncode

        self.assertEqual(0, probe())
        self.arm("--lanes", "north")
        self.assertEqual(0, self.token("off").returncode)
        self.assertEqual(1, probe())

    # Review Z-A I1: an `armed` stamp the probe cannot parse, or a broken
    # journal row, counts as armed (fail-closed), never as "not armed".
    def test_the_lanes_probe_with_a_garbage_armed_stamp_stays_armed(self):
        self.old.write_text(json.dumps({"lanes": ["north"], "armed": "garbage"}))
        env = dict(os.environ, **self.env())
        res = subprocess.run(
            ["bash", "-c", '. "$1"; meclaw_lanes_active "$2"', "probe",
             str(REPO / "scripts" / "cargo-target.sh"), str(self.tmp / "cargo.lock")],
            env=env, capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)

    # Review Z-A2 R1 M1: ... and the broken journal row the comment names.
    def test_the_lanes_probe_with_a_broken_journal_row_stays_armed(self):
        self.old.write_text(json.dumps({"lanes": ["north"], "armed": "2020-01-01T00:00:00Z"}))
        broken = self.tmp / "broken.db"
        db = sqlite3.connect(str(broken))
        db.execute("CREATE TABLE journal (seq INTEGER PRIMARY KEY, at TEXT, event TEXT, data TEXT)")
        for at, data in (("2030-01-01T00:00:00Z", "{not json"), ("garbage", '{"verb": "off"}'),
                         ("2030-01-01T00:00:00Z", "[1]")):
            db.execute("DELETE FROM journal")
            db.execute("INSERT INTO journal (at, event, data) VALUES (?, 'lease_noted', ?)", (at, data))
            db.commit()
            env = dict(os.environ, **self.env(extra={"MECLAW_PIPELINE_DB": str(broken)}))
            res = subprocess.run(
                ["bash", "-c", '. "$1"; meclaw_lanes_active "$2"', "probe",
                 str(REPO / "scripts" / "cargo-target.sh"), str(self.tmp / "cargo.lock")],
                env=env, capture_output=True, text=True, timeout=kit_time.HANG)
            self.assertEqual(0, res.returncode, (at, data, res.stderr))
        db.close()

    def test_the_old_path_follows_the_cargo_lock(self):
        env = {"MECLAW_GATE_LOCK": "/x/y/cargo.lock"}
        self.assertEqual("/x/y/cargo.tokens", schedule.old_tokens_path(env))
        env["MECLAW_STRAND_TOKENS"] = "/z/tokens"
        self.assertEqual("/z/tokens", schedule.old_tokens_path(env))
        self.assertEqual("/tmp/meclaw-w26-cargo.tokens", schedule.old_tokens_path({}))
        self.assertEqual(str(self.old), schedule.old_tokens_path(self.env()))


class TestTokenLanes(TokenTestCase):
    def setUp(self):
        super().setUp()
        self.lanes_file.write_text(LANES_FILE)

    def test_take_hands_out_lanes_in_the_order_of_init(self):
        self.arm("--lanes", "south,north")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane south", res.stdout.strip())
        self.assertEqual("lane north", self.take("welle-x/b").stdout.strip())
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("welle-x/c is #1 in the queue", res.stderr)
        self.assertEqual(["lane:south", "lane:north"], self.holder_lanes())
        with self._db() as db:
            self.assertEqual({"lane:south": "welle-x/a", "lane:north": "welle-x/b"},
                             {r["resource"]: r["holder"] for r in db.execute("SELECT * FROM leases")})

    def test_take_wait_blocks_until_the_lane_is_free(self):
        self.arm("--lanes", "north")
        self.assertEqual(0, self.take("welle-x/a").returncode)
        env = dict(os.environ)
        env.update(self.env(extra={"MECLAW_TOKEN_WAIT_SECS": "0.2"}))
        proc = subprocess.Popen([sys.executable, str(PIPELINE), "lanes", "take", "--strand",
                                 "welle-x/b", "--wait"], cwd=str(self.main), env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if self.db_path.exists() and "welle-x/b" in self.waiting():
                    break
                time.sleep(0.1)
            self.assertIsNone(proc.poll(), "take --wait returned while the lane was held")
            self.assertEqual(0, self.token("release", "--strand", "welle-x/a").returncode)
            out, err = proc.communicate(timeout=kit_time.HANG)
        finally:
            if proc.poll() is None:
                proc.kill()
        self.assertEqual(0, proc.returncode, out + err)
        self.assertEqual("lane north", out.strip().splitlines()[-1])
        self.assertEqual(1, err.count("is #1 in the queue"), "the queue line once, not every retry")
        self.assertEqual(["welle-x/b"], self.holders())

    def test_take_wait_says_it_blocks_not_end_your_turn(self):
        # Kit find (b), 09.10.: `take --wait` printed the queue line of a plain
        # take ("End your turn") and went on blocking -- the caller read it as
        # a return. With --wait the line says that this process waits.
        self.arm("--lanes", "north")
        self.assertEqual(0, self.take("welle-x/a").returncode)
        res = self.take("welle-x/c")
        self.assertIn("End your turn", res.stderr)
        self.assertEqual(0, self.token("release", "--strand", "welle-x/c").returncode)
        env = dict(os.environ)
        env.update(self.env(extra={"MECLAW_TOKEN_WAIT_SECS": "0.2"}))
        proc = subprocess.Popen([sys.executable, str(PIPELINE), "lanes", "take", "--strand",
                                 "welle-x/b", "--wait"], cwd=str(self.main), env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if self.db_path.exists() and "welle-x/b" in self.waiting():
                    break
                time.sleep(0.1)
            self.assertEqual(0, self.token("release", "--strand", "welle-x/a").returncode)
            out, err = proc.communicate(timeout=kit_time.HANG)
        finally:
            if proc.poll() is None:
                proc.kill()
        self.assertEqual(0, proc.returncode, out + err)
        self.assertNotIn("End your turn", err)
        self.assertIn("take --wait blocks here until it is taken", err)

    def test_a_second_holder_key_holds_a_second_lane_of_one_strand(self):
        # GH #1146: a strand runs a satellite chain on one lane and a core gate
        # on another -- `--holder <n>` is a second lease of the same strand.
        self.arm("--lanes", "north,south")
        self.assertEqual("lane north", self.take_lane("welle-x/a", "north").stdout.strip())
        res = self.token("take", "--strand", "welle-x/a", "--holder", "sat", "--lane", "south")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane south", res.stdout.strip())
        self.assertEqual(["welle-x/a", "welle-x/a:sat"], self.holders())
        who = self.token("who")
        self.assertIn("welle-x/a:sat", who.stdout + who.stderr)
        res = self.token("lane", "--strand", "welle-x/a", "--holder", "sat")
        self.assertEqual("south", res.stdout.strip())
        res = self.token("release", "--strand", "welle-x/a", "--holder", "sat")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["welle-x/a"], self.holders())
        res = self.token("take", "--strand", "welle-x/a", "--holder", "a/b")
        self.assertEqual(2, res.returncode)
        self.assertIn("--holder", res.stderr)

    def test_take_wait_wakes_on_the_release_not_on_a_clock(self):
        # Review A I3 / C M7: no retry clock -- the waiter blocks on the pool's
        # wake file, and the release wakes it at once. The watchdog of a round
        # is set to 300 s here; a sleep loop would still be asleep.
        self.arm("--lanes", "north")
        self.assertEqual(0, self.take("welle-x/a").returncode)
        env = dict(os.environ)
        env.update(self.env(extra={"MECLAW_TOKEN_WAIT_SECS": "300"}))
        proc = subprocess.Popen([sys.executable, str(PIPELINE), "lanes", "take", "--strand",
                                 "welle-x/b", "--wait"], cwd=str(self.main), env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            kit_time.until("take --wait of welle-x/b queues",
                           lambda: self.db_path.exists() and "welle-x/b" in self.waiting(),
                           state=lambda: proc.poll())
            self.assertIsNone(proc.poll(), "take --wait returned while the lane was held")
            self.assertEqual(0, self.token("release", "--strand", "welle-x/a").returncode)
            # Only the release can end the wait before its 300-s watchdog: a
            # missed wake holds past the hang marker.
            try:
                out, err = proc.communicate(timeout=kit_time.HANG)
            except subprocess.TimeoutExpired:
                raise kit_time.Hang("HANG: the release did not wake the waiter after %.0f s"
                                    % kit_time.HANG) from None
        finally:
            if proc.poll() is None:
                proc.kill()
        self.assertEqual(0, proc.returncode, out + err)
        self.assertEqual("lane north", out.strip().splitlines()[-1])
        self.assertEqual(["welle-x/b"], self.holders())
        self.assertNotIn("time.sleep", inspect.getsource(schedule.run_lanes))

    def test_take_wait_passes_other_refusals_through(self):
        self.arm("--lanes", "north")
        res = self.token("take", "--strand", "welle-x/b", "--lane", "nowhere", "--wait")
        self.assertNotIn(res.returncode, (0, 3))

    def test_take_wait_names_a_later_refusal(self):
        self.arm("--lanes", "north,south")
        self.assertEqual(0, self.take_lane("welle-x/a", "north").returncode)
        env = dict(os.environ)
        env.update(self.env(extra={"MECLAW_TOKEN_WAIT_SECS": "0.2"}))
        proc = subprocess.Popen([sys.executable, str(PIPELINE), "lanes", "take", "--strand",
                                 "welle-x/b", "--lane", "north", "--wait"], cwd=str(self.main),
                                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if "welle-x/b" in self.waiting():
                    break
                time.sleep(0.1)
            self.assertIsNone(proc.poll(), "take --wait returned while the lane was held")
            self.assertEqual(0, self.token("init", "--force", "--lanes", "south").returncode)
            out, err = proc.communicate(timeout=kit_time.HANG)
        finally:
            if proc.poll() is None:
                proc.kill()
        self.assertNotIn(proc.returncode, (0, 3), out + err)
        self.assertIn("no lane north", err)

    def test_a_released_lane_goes_to_the_next(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.assertEqual(3, self.take("welle-x/c").returncode)
        res = self.token("release", "--strand", "welle-x/a")
        self.assertIn("next in the queue: welle-x/c (lane north, waiting 0 min)", res.stderr)
        res = self.take("welle-x/c")
        self.assertEqual("lane north", res.stdout.strip())

    def test_take_again_names_the_held_lane(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode)
        self.assertEqual("lane north", res.stdout.strip())
        self.assertIn("cargo token already held by welle-x/a (1/1)", res.stderr)

    def test_who_names_the_lanes(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        res = self.token("who")
        self.assertIn("lanes: north south", res.stdout)
        self.assertIn("welle-x/a  lane north  held 0 min", res.stdout)

    def test_lane_prints_the_held_lane(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        self.assertEqual("north", self.token("lane", "--strand", "welle-x/a").stdout.strip())
        self.assertEqual("", self.token("lane", "--strand", "welle-x/b").stdout.strip())

    def test_a_threads_word_in_the_host_file_does_not_break_the_token(self):
        self.arm("--lanes", "spare")
        self.assertEqual("lane spare", self.take("welle-x/a").stdout.strip())

    def test_lanes_and_max_together_refuse(self):
        res = self.token("init", "--lanes", "north", "--max", "2")
        self.assertEqual(2, res.returncode)
        self.assertIn("--lanes and --max exclude each other", res.stderr)

    def test_a_lane_missing_in_the_host_file_refuses(self):
        res = self.token("init", "--lanes", "north,east")
        self.assertEqual(2, res.returncode)
        self.assertIn("token init: no lane east in the host file", res.stderr)

    def test_without_a_host_file_lanes_refuse(self):
        self.lanes_file.unlink()
        res = self.token("init", "--lanes", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("token init: --lanes needs the host file", res.stderr)

    def test_a_lane_name_twice_refuses(self):
        res = self.token("init", "--lanes", "north,north")
        self.assertEqual(2, res.returncode)
        self.assertIn("a lane is named twice in --lanes", res.stderr)

    def test_lanes_belongs_to_init(self):
        self.arm("--lanes", "north")
        res = self.token("take", "--strand", "welle-x/a", "--lanes", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("token take: --lanes belongs to init", res.stderr)

    def test_max_alone_is_the_old_token(self):
        self.arm("--max", "2")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode)
        self.assertEqual("", res.stdout)
        self.assertIn("cargo token 1/2 taken by welle-x/a", res.stderr)

    def test_take_with_a_lane_takes_that_lane(self):
        self.arm("--lanes", "north,south")
        res = self.take_lane("welle-x/a", "south")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane south", res.stdout.strip())
        self.assertEqual("lane north", self.take("welle-x/b").stdout.strip())

    def test_a_named_lane_that_is_held_queues(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        res = self.take_lane("welle-x/b", "north")
        self.assertEqual(3, res.returncode)
        self.assertIn("lane north is held by welle-x/a -- welle-x/b is #1 in the queue", res.stderr)

    def test_release_names_the_waiter_the_freed_lane_goes_to(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.take_lane("welle-x/b", "south")
        self.take_lane("welle-x/c", "south")
        self.take("welle-x/d")
        res = self.token("release", "--strand", "welle-x/a")
        self.assertIn("next in the queue: welle-x/d (lane north, waiting 0 min)", res.stderr)

    def test_release_with_no_waiter_for_the_freed_lane_says_so(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.take_lane("welle-x/b", "south")
        self.take_lane("welle-x/c", "south")
        res = self.token("release", "--strand", "welle-x/a")
        self.assertIn("no waiter takes lane north (1 in the queue, each for a held lane)", res.stderr)

    def test_who_names_the_lane_offered_to_a_waiter(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.token("release", "--strand", "welle-x/a", minutes=5)
        res = self.token("who", minutes=6)
        self.assertIn("#1 welle-x/b  waiting 6 min  offered north at %s" % self.utc(5), res.stdout)
        offered = self.events("lease_offered")
        self.assertEqual(("lane:north", "welle-x/b"), (offered[-1]["resource"], offered[-1]["holder"]))

    def test_the_next_waiter_gets_the_lane_offered_in_turn(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree, minutes=1).returncode)
        self.assertEqual(3, self.take("welle-x/x", minutes=30).returncode)
        self.token("release", "--strand", "welle-x/a", minutes=100)
        res = self.take("welle-x/y", minutes=111)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("dropped %s/kit from the queue -- waiter past ttl, did not take lane north "
                      "offered 11 min ago" % WAVE, res.stderr)
        self.assertIn("welle-x/y is #2 in the queue", res.stderr)
        res = self.token("who", minutes=112)
        self.assertIn("#1 welle-x/x  waiting 82 min  offered north at %s" % self.utc(111), res.stdout)

    def test_a_waiter_within_the_ttl_keeps_an_offered_lane(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        self.take("welle-x/b", minutes=1)
        self.take("welle-x/c", minutes=1)
        self.token("release", "--strand", "welle-x/a", minutes=2)
        res = self.take("welle-x/c", minutes=30)
        self.assertEqual(3, res.returncode)
        self.assertEqual(["welle-x/b", "welle-x/c"], self.waiting())

    def test_a_named_waiter_keeps_its_lane_against_a_later_take(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.take_lane("welle-x/b", "south")
        self.take_lane("welle-x/c", "north")
        self.token("release", "--strand", "welle-x/a")
        res = self.take("welle-x/d")
        self.assertEqual(3, res.returncode, "north goes to the waiter that named it")
        self.assertEqual("lane north", self.take("welle-x/c").stdout.strip())

    def test_a_named_waiter_does_not_block_another_lane(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.take_lane("welle-x/b", "north")
        self.assertEqual("lane south", self.take("welle-x/c").stdout.strip())

    def test_who_names_the_wanted_lane_of_a_waiter(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        self.take_lane("welle-x/b", "north")
        self.assertIn("#1 welle-x/b  for lane north  waiting 0 min", self.token("who").stdout)

    def test_an_unknown_lane_to_take_refuses(self):
        self.arm("--lanes", "north")
        res = self.take_lane("welle-x/a", "east")
        self.assertEqual(2, res.returncode)
        self.assertIn("token take: no lane east among the armed lanes (north)", res.stderr)

    def test_a_held_other_lane_is_not_swapped(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        res = self.take_lane("welle-x/a", "south")
        self.assertEqual(2, res.returncode)
        self.assertIn("welle-x/a holds lane north, not south -- release it first", res.stderr)

    def test_a_lane_without_lanes_armed_refuses(self):
        self.arm("--max", "1")
        res = self.take_lane("welle-x/a", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("--lane north, but this host is not armed with lanes", res.stderr)

    def test_check_with_a_lane_wants_exactly_that_lane(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.assertEqual(0, self.token("check", "--strand", "welle-x/a", "--lane", "north").returncode)
        res = self.token("check", "--strand", "welle-x/a", "--lane", "south")
        self.assertEqual(3, res.returncode)
        self.assertIn("welle-x/a holds lane north, not south -- gate on your own lane", res.stderr)

    def test_offer_names_the_waiter_a_free_lane_goes_to(self):
        self.arm("--lanes", "north,south")
        self.take_lane("welle-x/a", "north")
        self.take_lane("welle-x/b", "south")
        self.take_lane("welle-x/c", "north")
        self.take("welle-x/d")
        self.assertEqual("", self.token("offer").stdout)
        self.token("release", "--strand", "welle-x/b", minutes=4)
        res = self.token("offer", minutes=5)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("welle-x/d  lane south  offered at %s\n" % self.utc(4), res.stdout)


class FakeLane:
    def __init__(self, name):
        self.name, self.ssh, self.bmc, self.root = name, "builder@x.example.invalid", None, "/srv/target"
        self.transport, self.cls, self.threads, self.jobs, self.slots = "ssh", "lane", None, None, None


class FakeLaneModule:
    @staticmethod
    def load_lanes(path=None):
        return [FakeLane("north"), FakeLane("south"), FakeLane("ghost")]

    @staticmethod
    def status(lane):
        if lane.name == "ghost":
            return "unreachable"
        return "reachable  fs ext4  free 373G  bare none"


class BrowserLanes(FakeLaneModule):
    """A lane module whose fingerprint says per engine what `browsers` holds."""
    browsers = {"chromium": "ok", "webkit": "ok"}

    @classmethod
    def fingerprint(cls, lane):
        return {"vcpu": 12, "cores": 6, "browsers": dict(cls.browsers)}


class TestLanesFingerprint(TokenTestCase):
    """Re-Check S3 Teil 2 m-2 (schedule.py browser_refusals/ingest): a lane refused
    for an engine kept the fingerprint of its last run, so it stayed refused after
    `lanes browsers <lane> install` until some other run wrote a new one -- and a
    refused lane gets no browser run. `lanes fingerprint <lane>` asks the lane now and
    writes it to the store; `lanes status` names the refusal."""

    def setUp(self):
        super().setUp()
        self.lanes_file.write_text(LANES_FILE)
        BrowserLanes.browsers = {"chromium": "ok", "webkit": "does not start (newPage): EGL"}
        schedule._lane_override = BrowserLanes
        self.addCleanup(setattr, schedule, "_lane_override", None)

    def capacity(self, name):
        with self._db() as db:
            row = db.execute("SELECT capacity FROM runners WHERE name = ?", (name,)).fetchone()
        return json.loads(row["capacity"])

    def test_a_refused_lane_is_bookable_again_after_a_new_fingerprint(self):
        res = self.token("fingerprint", "north")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertEqual("north browsers webkit: does not start (newPage): EGL\n", res.stdout)
        self.assertEqual(0, self.capacity("north")["browser_webkit"])
        self.assertIn("browsers refused webkit: does not start (newPage): EGL",
                      self.token("status", "north").stdout)
        BrowserLanes.browsers = {"chromium": "ok", "webkit": "ok"}  # after `install`
        res = self.token("fingerprint", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("north browsers ok\n", res.stdout)
        self.assertEqual(1, self.capacity("north")["browser_webkit"])
        self.assertNotIn("refused", self.token("status", "north").stdout)

    def test_an_unknown_lane_is_refused(self):
        res = self.token("fingerprint", "nowhere")
        self.assertEqual(2, res.returncode)
        self.assertIn("no lane nowhere", res.stderr)


class TestLanesStatus(TokenTestCase):
    def setUp(self):
        super().setUp()
        self.lanes_file.write_text(LANES_FILE)
        schedule._lane_override = FakeLaneModule
        self.addCleanup(setattr, schedule, "_lane_override", None)

    def test_one_line_per_lane(self):
        res = self.token("status")
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.splitlines()
        self.assertEqual(3, len(lines), res.stdout)
        self.assertRegex(lines[0], r"^north\s+reachable\s+fs \S+\s+free \d+G\s+bare none$")
        self.assertRegex(lines[2], r"^ghost\s+unreachable$")
        self.assertFalse(self.db_path.exists(), "status on an unarmed host writes nothing")

    def test_status_marks_runners_it_can_see(self):
        self.arm("--max", "1")
        self.token("status")
        with self._db() as db:
            states = {r["name"]: r["state"] for r in db.execute("SELECT name, state FROM runners")}
        self.assertEqual({"north": "idle", "south": "idle", "ghost": "unreachable"}, states)

    def hold(self, lane, run, state="stopped", reason="lost", age_min=12, deadline_min=83):
        """A workpackage that binds `lane` the way a lost run leaves it
        (`stopped/lost` until its deadline, OR-DT.W3.u), dispatched `age_min`
        before the clock of `self.token`."""
        dispatched = schedule._iso(self.T0 - 60 * age_min)
        doc = {"run": run, "lane": lane, "deadline_at": schedule._iso(self.T0 + 60 * deadline_min),
               "demand": {"cpu": 8}}
        with self._db() as db:
            db.execute("INSERT INTO items (id, key, type, title, state, reason, doc, version, "
                       "created_at, updated_at) VALUES (?, ?, 'workpackage', 'wp', ?, ?, ?, 1, ?, ?)",
                       ("wp-" + run, "wp:" + run, state, reason, json.dumps(doc), dispatched, dispatched))
            db.execute("INSERT INTO journal (at, run, kind, event, text, data) VALUES "
                       "(?, ?, 'note', 'run_dispatched', 'run_dispatched', '{}')", (dispatched, run))

    def test_status_shows_a_held_lease_and_never_says_idle(self):
        # Z13: run-696ff322bd86 died on build03 and its admission kept the
        # lane's cpu until the deadline; `lanes status` said `state idle` while
        # two runs were refused `needs cpu 8+8 > 8`.
        class IdleLanes(FakeLaneModule):
            @staticmethod
            def status(lane):
                return "unreachable  state off" if lane.name == "ghost" else \
                    "reachable  fs ext4  free 373G  bare none  state idle"

        schedule._lane_override = IdleLanes
        self.arm("--max", "1")
        self.hold("north", "run-0123456789ab")
        res = self.token("status")
        self.assertEqual(0, res.returncode, res.stderr)
        lines = {line.split()[0]: line for line in res.stdout.splitlines()}
        self.assertEqual({"north", "south", "ghost"}, set(lines), res.stdout)
        self.assertNotIn("idle", lines["north"])
        self.assertRegex(lines["north"], r"\bstate leased\b")
        self.assertRegex(lines["north"],
                         r"\blease run-0123456789ab age 12m deadline \d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ "
                         r"stopped/lost\b")
        self.assertRegex(lines["south"], r"state idle$")
        with self._db() as db:
            states = {r["name"]: r["state"] for r in db.execute("SELECT name, state FROM runners")}
        self.assertEqual("busy", states["north"])
        self.assertEqual("idle", states["south"])

    def test_status_drops_a_lease_past_its_deadline(self):
        # A `stopped/lost` admission binds the lane until its deadline only.
        self.arm("--max", "1")
        self.hold("north", "run-0123456789ab", age_min=120, deadline_min=-1)
        res = self.token("status")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertNotIn("lease", res.stdout)

    def test_without_a_host_file_status_refuses(self):
        self.lanes_file.unlink()
        res = self.token("status")
        self.assertEqual(2, res.returncode)
        self.assertIn("no lane host file at", res.stderr)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
