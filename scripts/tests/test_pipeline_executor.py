"""The lane executor of the dev pipeline (`scripts/pipeline/executor.py`).

Every station here is a fake: `sh -c 'sleep ...; exit n'`, a fake `cargo` and
`cargo-nextest` on PATH that write their argv into a calls file, and a fake
`scripts/test-tier.sh` in a throw-away tree that records the profile and the
filterset it was handed. Nothing compiles; the whole file stays under 30 s.

Run: `python3 -m unittest scripts.tests.test_pipeline_executor` from the root.
"""
import datetime
import fcntl
import gc
import io
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import tomllib
import types
import unittest
from unittest import mock

from scripts.pipeline import events, receipt, schedule
from scripts.tests import kit_time, kit_tree, private_tree, store_isolation
from scripts import gate_plan

try:
    from scripts.pipeline import executor
except ImportError:  # red until the module exists
    executor = None

REPO = pathlib.Path(__file__).resolve().parents[2]
RUN = "run-0123456789ab"
REV = "a" * 40
BASE = "b" * 40


def _iso(delta_s):
    t = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=delta_s)
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def station(name, cmd="true", *, after=(), needs=None, timeout_min=1, when="success", **extra):
    st = {
        "name": name, "scope": "scope of %s" % name, "cargo": 0,
        "stage": ["S", "I", "R"],
        "needs": dict({"cpu": 1, "colony": 0, "browser_chromium": 0,
                       "browser_webkit": 0, "export": 0, "cargo": 0}, **(needs or {})),
        "inputs": [], "cmds": [{"argv": ["sh", "-c", cmd], "cwd": ""}] if cmd is not None else [],
        "env": "none", "reason": ["test"], "timeout_min": timeout_min,
        "after": list(after), "when": {"outcome": when}, "run": True, "keep_ws": True,
    }
    st["cargo"] = st["needs"]["cargo"]
    st.update(extra)
    return st


FAKE_TIER = """#!/bin/sh
if [ -n "${MECLAW_TIER_LIST:-}" ]; then
  if [ -n "${FAKE_TIER_LIST_SLEEP:-}" ]; then sleep "$FAKE_TIER_LIST_SLEEP"; fi
  printf 'LIST\\t%s\\t%s\\n' "$MECLAW_TIER_PROFILE" "$*" >> "$TIER_CALLS"
  echo "test-tier: LISTED ${FAKE_TIER_LISTED:-0}"
  exit "${FAKE_TIER_LIST_RC:-0}"
fi
if [ "$1" = t0 ] && [ -n "${FAKE_TIER_T0_RC:-}" ]; then
  echo "test-tier: RED -- no tests matched: kind(lib)" >&2
  exit "$FAKE_TIER_T0_RC"
fi
if [ -n "${FAKE_TIER_SLEEP:-}" ]; then sleep "$FAKE_TIER_SLEEP"; fi
printf '%s\\t%s\\t%s\\n' "$MECLAW_TIER_PROFILE" "${MECLAW_TIER_REUSE:-}${MECLAW_TIER_REUSE:+ lane=${MECLAW_TIER_LANE:-}}" "$*" >> "$TIER_CALLS"
mkdir -p "target/nextest/$MECLAW_TIER_PROFILE"
if [ -n "${FAKE_JUNIT:-}" ]; then cp "$FAKE_JUNIT" "target/nextest/$MECLAW_TIER_PROFILE/junit.xml"; fi
case "$*" in
  *"& test(/webkit/)"*)
    if [ -n "${FAKE_TIER_WEBKIT_RC:-}" ]; then
      if [ -n "${FAKE_TIER_WEBKIT_STDERR:-}" ]; then echo "$FAKE_TIER_WEBKIT_STDERR" >&2; fi
      exit "$FAKE_TIER_WEBKIT_RC"
    fi ;;
esac
if [ -n "${FAKE_TIER_STDERR:-}" ]; then echo "$FAKE_TIER_STDERR" >&2; fi
exit "${FAKE_TIER_RC:-0}"
"""

# A fake `cargo`: every call is one line in $CALLS. `nextest list` answers with
# a binaries-only document naming the selection, `metadata` with a stub.
FAKE_CARGO = """#!/bin/sh
printf 'cargo %s\\n' "$*" >> "$CALLS"
case "$1 $2" in
  "nextest list")
    printf '{"rust-build-meta": {"target-directory": "%s", "base-output-directories": ["debug"], ' "$CARGO_TARGET_DIR"
    printf '"non-test-binaries": {}, "build-script-out-dirs": {}, "linked-paths": []}, '
    printf '"rust-binaries": {"c1::t1": {"binary-id": "c1::t1", "binary-path": "%s/debug/deps/t1"}' "$CARGO_TARGET_DIR"
    if [ -n "${FAKE_LIST_EXTRA:-}" ]; then
      printf ', "%s": {"binary-id": "%s", "binary-path": "%s/debug/deps/x"}' "$FAKE_LIST_EXTRA" "$FAKE_LIST_EXTRA" "$CARGO_TARGET_DIR"
    fi
    printf '}}\\n' ;;
  "metadata --format-version") printf '{"packages": [], "workspace_root": "%s"}\\n' "$PWD" ;;
esac
exit 0
"""

FAKE_NEXTEST = """#!/bin/sh
printf 'cargo-nextest %s\\n' "$*" >> "$CALLS"
profile=default
prev=""
for a in "$@"; do
  [ "$prev" = "--profile" ] && profile="$a"
  prev="$a"
done
case "$*" in
  *"--version"*) echo "cargo-nextest 0.9.143"; exit 0 ;;
esac
mkdir -p "target/nextest/$profile"
cat > "target/nextest/$profile/junit.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="0" errors="0">
  <testsuite name="c1::t1" tests="1" failures="0" errors="0" disabled="0">
    <testcase name="the_one" classname="c1::t1" time="0.010"/>
  </testsuite>
</testsuites>
XML
echo "     Summary [   0.010s] 1 test run: 1 passed, 0 skipped"
exit 0
"""

JUNIT_TWO = """<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="4" failures="1" errors="0">
  <testsuite name="a::x" tests="2" failures="0" errors="0" disabled="0">
    <testcase name="first" classname="a::x" time="0.5"/>
    <testcase name="second" classname="a::x" time="0.25"/>
  </testsuite>
  <testsuite name="b::z" tests="2" failures="1" errors="0" disabled="0">
    <testcase name="good" classname="b::z" time="0.1"/>
    <testcase name="broken" classname="b::z" time="0.2"><failure type="test failure">boom</failure></testcase>
  </testsuite>
</testsuites>
"""


class ExecutorCase(unittest.TestCase):
    def setUp(self):
        if executor is None:
            self.fail("scripts/pipeline/executor.py is missing")
        self._tmp = tempfile.TemporaryDirectory(prefix="pipe-exec-")
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        self.tree = self.root / "tree"
        self.target = self.root / "target"
        self.ws = self.root / "ws" / RUN
        self.bin = self.root / "bin"
        for d in (self.tree, self.target, self.bin):
            d.mkdir(parents=True)
        self.calls = self.root / "calls.txt"
        self.tier_calls = self.root / "tier-calls.txt"
        self.lock = self.root / "cargo.lock"
        # #1158 B-1: the executor reads THIS machine's host file for the Egon
        # lock; the suite's machine is a build host unless a test says so.
        self.lanes = self.root / "lanes"
        self.lanes.write_text("")
        env = mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(self.lanes)})
        env.start()
        self.addCleanup(env.stop)

    # --- fixtures ----------------------------------------------------------
    def write_exe(self, path, text):
        path = pathlib.Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(0o755)
        return path

    def fake_tier(self):
        return self.write_exe(self.tree / "scripts" / "test-tier.sh", FAKE_TIER)

    def fake_cargo(self):
        self.write_exe(self.bin / "cargo", FAKE_CARGO)
        self.write_exe(self.bin / "cargo-nextest", FAKE_NEXTEST)

    def dispatch(self, stations, **kw):
        d = {
            "run": RUN, "repo": "meclaw", "mode": "strand", "rev": REV, "base": BASE,
            "strand": "dt-test", "tree": str(self.tree), "target": str(self.target),
            "ws": str(self.ws), "stations": stations,
            "capacity": {"cpu": 24, "colony": 8, "browser_chromium": 1,
                         "browser_webkit": 1, "export": 1, "cargo": 1},
            "cache_known": [], "live_runs": [], "expected_g": 0,
            "fingerprint_expected": {}, "budget": {"free_min_g": 0, "reserve_g": 0},
            "quarantine": [], "decide": [], "deadline_at": _iso(600),
            "+env": {"CALLS": str(self.calls), "TIER_CALLS": str(self.tier_calls),
                     "PATH": "%s:%s" % (self.bin, os.environ.get("PATH", ""))},
            "+nice": False, "+cpu_overbook": 1.25, "+lock_file": str(self.lock),
            "+export_root": str(self.root / "export"), "+fail_fast": False,
            "+no_precheck_stop": False, "+only": None,
        }
        d.update(kw)
        return d

    def run_exec(self, d, **kw):
        out, err = io.StringIO(), io.StringIO()
        rc = executor.run(d, out, err, **kw)
        evs = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        self.err_text = err.getvalue()
        return rc, evs

    @staticmethod
    def ended(evs):
        return {e["data"]["station"]: e["data"] for e in evs if e["event"] == "station_ended"}

    @staticmethod
    def order(evs, event):
        return [e["data"]["station"] for e in evs if e["event"] == event]

    def call_lines(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []


class TestExecutorScheduling(ExecutorCase):
    def test_a_station_never_inherits_the_callers_repository(self):
        # pd-1155 kit find: `GIT_DIR`/`GIT_WORK_TREE` of the caller made every
        # git call of a station act on that repository, not on the tree.
        out = self.root / "git-env.txt"
        cmd = ('echo "${GIT_DIR-unset} ${GIT_WORK_TREE-unset} ${GIT_INDEX_FILE-unset} '
               '${GIT_COMMON_DIR-unset}" > %s' % out)
        with mock.patch.dict(os.environ, {"GIT_DIR": "/elsewhere/.git", "GIT_WORK_TREE": "/elsewhere",
                                          "GIT_INDEX_FILE": "/elsewhere/.git/index",
                                          "GIT_COMMON_DIR": "/elsewhere/.git"}):
            rc, _ = self.run_exec(self.dispatch([station("probe", cmd)]))
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual("unset unset unset unset", out.read_text().strip())

    def test_executor_runs_independent_stations_in_parallel(self):
        # A FIFO rendezvous (VG M8): each station blocks until the other one runs;
        # run one after the other, `a` waits for a reader that never comes and the
        # station deadline (a hang marker, never a speed verdict) ends it RED.
        fifo = self.root / "rv"
        os.mkfifo(fifo)
        d = self.dispatch([station("a", "echo a > %s" % fifo), station("b", "cat %s" % fifo)])
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual({"a": "GREEN", "b": "GREEN"},
                         {k: v["verdict"] for k, v in self.ended(evs).items()})

    def test_executor_respects_cargo_lock(self):
        env_file = self.root / "held.txt"
        cmd = 'sleep 0.4; echo "$MECLAW_CARGO_LOCK_HELD" >> %s' % env_file
        d = self.dispatch([station("c1", cmd, needs={"cargo": 1}),
                           station("c2", cmd, needs={"cargo": 1})])
        # Somebody else holds the lane lock while the run blocks on it: the run
        # names the wait on its own NOTE row and in the receipt -- on a fake
        # clock (TV-K1), so the seconds are exact. The holder moves the clock by
        # 1.3 s only once the run BLOCKS (#1171: counted from the holder's side
        # the run's own start ate the wait); the executor's blocking LOCK_EX
        # signals it (the stage-lock probe is LOCK_NB and does not count).
        clock = kit_time.FakeClock()
        ready, blocked = threading.Event(), threading.Event()

        def run_flock(fd, op):
            if op == fcntl.LOCK_EX:
                blocked.set()
            return fcntl.flock(fd, op)
        run_fcntl = types.SimpleNamespace(LOCK_EX=fcntl.LOCK_EX, LOCK_NB=fcntl.LOCK_NB,
                                          LOCK_UN=fcntl.LOCK_UN, LOCK_SH=fcntl.LOCK_SH,
                                          flock=run_flock)

        def hold():
            with open(self.lock, "w") as fh:
                fcntl.flock(fh, fcntl.LOCK_EX)
                ready.set()
                blocked.wait(kit_time.HANG)
                clock.advance(1.3)
        holder = threading.Thread(target=hold)
        holder.start()
        ready.wait(kit_time.HANG)
        with mock.patch.object(executor, "fcntl", run_fcntl):
            rc, evs = self.run_exec(d, clock=clock.monotonic)
        holder.join()
        self.assertTrue(blocked.is_set(), "the run never blocked on the cargo lock")
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")
               and e["data"].get("station") in ("c1", "c2")]
        # Serial: the second cargo station starts only after the first ended.
        self.assertEqual(["station_started", "station_ended"] * 2, [s[0] for s in seq])
        self.assertEqual(["1", "1"], env_file.read_text().split())
        lock_wait = self.ended(evs).get("lock-wait")
        self.assertIsNotNone(lock_wait, "no lock-wait row after a 1.3 s wait")
        self.assertEqual(("NOTE", 1), (lock_wait["verdict"], lock_wait["secs"]))
        v2 = json.loads((self.ws / "receipt.json").read_text())
        self.assertEqual(1, v2["lock_wait_secs"])
        # The stations' own seconds never carry the queue in front of them.
        self.assertEqual((0, 0), (self.ended(evs)["c1"]["secs"], self.ended(evs)["c2"]["secs"]))

    def test_an_uncontended_lock_books_no_wait_on_a_loaded_host(self):
        # GH #764: a free lock booked a phantom second under load -- the time
        # between the two clock reads around the take, not a queue. A lock
        # nobody holds is no wait by construction; the clock here jumps a
        # second at every read, like a host that is slow everywhere.
        class Loaded(kit_time.FakeClock):
            def monotonic(self):
                return self.advance(1.0)
            __call__ = monotonic
        d = self.dispatch([station("c1", "true", needs={"cargo": 1})])
        rc, evs = self.run_exec(d, clock=Loaded())
        self.assertEqual(0, rc, self.err_text)
        self.assertNotIn("lock-wait", self.ended(evs))
        self.assertEqual(0, json.loads((self.ws / "receipt.json").read_text())["lock_wait_secs"])

    def test_executor_respects_capacity_vector(self):
        sts = [station("s%d" % i, "sleep 0.6", needs={"colony": 1}) for i in range(9)]
        rc, evs = self.run_exec(self.dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        running, peak, ninth_after_end = 0, 0, None
        ended_any = False
        for e in evs:
            if e["event"] == "station_started":
                running += 1
                peak = max(peak, running)
                if running == 8 or ninth_after_end is not None:
                    pass
                if sum(1 for x in evs[:evs.index(e) + 1] if x["event"] == "station_started") == 9:
                    ninth_after_end = ended_any
            elif e["event"] == "station_ended":
                running -= 1
                ended_any = True
        self.assertEqual(8, peak)
        self.assertTrue(ninth_after_end, "the ninth colony station started before a slot was free")

    def test_executor_orders_by_cpm_rank(self):
        # One cpu: one station at a time. `b` heads the long chain (b -> c),
        # so it goes first although `a` comes first in the plan.
        sts = [station("a", "sleep 0.05", timeout_min=1),
               station("b", "sleep 0.05", timeout_min=1),
               station("c", "sleep 0.05", after=["b"], timeout_min=5)]
        d = self.dispatch(sts, capacity={"cpu": 1, "colony": 0, "browser_chromium": 0,
                                         "browser_webkit": 0, "export": 0, "cargo": 1})
        d["+cpu_overbook"] = 1.0
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual(["b", "c", "a"], self.order(evs, "station_started"))

    CAP24 = {"cpu": 24, "colony": 0, "browser_chromium": 0, "browser_webkit": 0,
             "export": 0, "cargo": 1}

    def test_executor_reserves_for_the_wide_station_and_backfills_only_what_keeps_its_start(self):
        # GH #1162 B-S4-1: `wide` (20 of 24) waits for `pre`. `long` (8) would hold 8 past
        # the end of `pre` and move the start of `wide` -- it waits (before: it started at
        # once and `wide` waited for it). `short` ends before `pre` ends, so it backfills.
        sts = [station("pre", "sleep 0.5", calib_s=1),
               station("wide", "sleep 0.2", after=["pre"], needs={"cpu": 20}, calib_s=50),
               station("long", "sleep 1", needs={"cpu": 8}, calib_s=30),
               station("short", "true", calib_s=0.1)]
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")]
        self.assertLess(seq.index(("station_started", "wide")),
                        seq.index(("station_started", "long")), seq)
        self.assertLess(seq.index(("station_started", "short")),
                        seq.index(("station_ended", "pre")), seq)

    def test_executor_reservation_holds_a_station_no_longer_than_the_age_bound(self):
        # Fairness: a station the reservation keeps back starts once it has waited
        # RESERVE_AGE_S, although `wide` still waits for `pre`.
        sts = [station("pre", "sleep 2", calib_s=2),
               station("wide", "true", after=["pre"], needs={"cpu": 20}, calib_s=100),
               station("small", "true", needs={"cpu": 8}, calib_s=60)]
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        with mock.patch.object(executor, "RESERVE_AGE_S", 0.3):
            rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")]
        self.assertLess(seq.index(("station_started", "small")),
                        seq.index(("station_ended", "pre")), seq)

    def test_an_alone_station_runs_with_nothing_beside_it(self):
        # GH #1191 (welle-tb S1e): gh1002 measures a 6 s bound under a CPU slowed 4x;
        # beside `tests` it measured the lane's load (6/10 isolated on build04). A
        # station `alone` (plan.ALONE) starts on an empty lane and nothing with a
        # booking starts beside it -- a cpu booking of 1 would fit beside everything.
        sts = [station("a", "sleep 0.4", calib_s=1),
               station("solo", "sleep 0.4", alone=True, calib_s=1),
               station("b", "sleep 0.4", calib_s=1),
               station("c", "sleep 0.4", calib_s=1)]
        rc, evs = self.run_exec(self.dispatch(sts, capacity=self.CAP24))
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")]
        start, end = seq.index(("station_started", "solo")), seq.index(("station_ended", "solo"))
        self.assertEqual(start + 1, end, "something ran beside the alone station: %s" % seq)
        began = {n for ev, n in seq[:start] if ev == "station_started"}
        ended = {n for ev, n in seq[:start] if ev == "station_ended"}
        self.assertEqual(began, ended, "the alone station started beside %s: %s"
                         % (sorted(began - ended), seq))
        self.assertGreater(executor._needs({"alone": True, "needs": {"cpu": 4}})["cpu"],
                           24 * 1.5, "an alone station books more than any lane holds")

    def test_earliest_slot_keeps_the_whole_length_free_on_every_dimension(self):
        # GH #1162 B-S4-3: a reservation is an interval; a planned one that starts
        # inside it counts, and the locks count like the width.
        cap = {"cpu": 24.0, "colony": 8.0}
        planned = [({"cpu": 20.0}, 5.0, 15.0)]
        self.assertEqual(0.0, executor.earliest_slot({"cpu": 4.0}, 4.0, planned, 0.0, cap))
        self.assertEqual(15.0, executor.earliest_slot({"cpu": 8.0}, 10.0, planned, 0.0, cap))
        colony = [({"cpu": 1.0, "colony": 2.0}, 0.0, 9.0)]
        self.assertEqual(9.0, executor.earliest_slot({"cpu": 1.0, "colony": 8.0}, 3.0, colony,
                                                     1.0, cap))
        self.assertEqual(1.0, executor.earliest_slot({"cpu": 30.0}, 3.0, [], 1.0, cap),
                         "a station bigger than the lane fits an empty one")
        self.assertIsNone(executor.earliest_slot({"cpu": 20.0}, 3.0, [({"cpu": 8.0}, 0.0, None)],
                                                 1.0, cap))
        self.assertIsNone(executor.earliest_slot({"cpu": 20.0}, None, [], 1.0, cap))
        self.assertIsNone(executor.earliest_slot({"cpu": 20.0}, 3.0, [], None, cap))

    def test_reserve_plans_every_blocked_station_in_order(self):
        cap = {"cpu": 24.0, "colony": 8.0}
        running = [({"cpu": 8.0, "colony": 2.0}, 0.0, 5.0)]
        blocked = [({"cpu": 20.0}, 1.0, 10.0), ({"cpu": 4.0, "colony": 8.0}, 1.0, 3.0),
                   ({"cpu": 8.0}, 0.0, 20.0)]
        self.assertEqual([5.0, 5.0, 15.0], executor.reserve(blocked, running, cap))
        self.assertIsNone(executor.reserve(blocked + [({"cpu": 1.0}, 0.0, None)], running, cap))
        # A successor reserves after its predecessor's reservation; one that will be
        # skipped (no needs, no time) holds its own successors for no time.
        chain = [({"cpu": 20.0}, 1.0, 10.0), ({}, 0.0, 0.0, [0]), ({"cpu": 4.0}, 0.0, 2.0, [1])]
        self.assertEqual([5.0, 15.0, 15.0], executor.reserve(chain, running, cap))

    def test_executor_backfill_keeps_every_reservation_so_the_chain_is_not_delayed(self):
        # GH #1162 B-S4-3, the warm release in small: `chain` (10 of 24, narrow) waits
        # for `selftest`, `filler` (10) is ready at once. EASY reserved for wide stations
        # only, so `filler` started and `chain` waited for its end (release warm 670.8 s
        # in V6). Every blocked station holds a reservation now: `filler` waits for `chain`.
        sts = [station("base", "sleep 0.9", needs={"cpu": 6}, calib_s=2),
               station("selftest", "sleep 0.2", calib_s=0.2),
               station("chain", "sleep 0.2", after=["selftest"], needs={"cpu": 10}, calib_s=1),
               station("tail", "true", after=["chain"], calib_s=1),
               station("filler", "sleep 0.3", needs={"cpu": 10}, calib_s=1.5)]
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")]
        self.assertLess(seq.index(("station_started", "chain")),
                        seq.index(("station_started", "filler")), seq)
        self.assertLess(seq.index(("station_started", "base")),
                        seq.index(("station_ended", "selftest")), seq)

    def test_no_backfill_moves_the_start_of_any_reservation(self):
        # `wide` (14 of 24) and `narrow` (4) wait for `pre`; `late` (8) would keep the
        # start of `wide` but move the one of `narrow` -- it waits (EASY started it).
        sts = [station("pre", "sleep 0.3", calib_s=0.3),
               station("wide", "sleep 0.2", after=["pre"], needs={"cpu": 14}, calib_s=1),
               station("narrow", "sleep 0.2", after=["pre"], needs={"cpu": 4}, calib_s=1),
               station("late", "true", needs={"cpu": 8}, calib_s=0.5)]
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        seq = [(e["event"], e["data"].get("station")) for e in evs
               if e["event"] in ("station_started", "station_ended")]
        self.assertLess(seq.index(("station_started", "narrow")),
                        seq.index(("station_started", "late")), seq)
        self.assertLess(seq.index(("station_started", "wide")),
                        seq.index(("station_started", "late")), seq)

    def test_the_age_bound_counts_only_the_time_a_reservation_keeps_a_station_back(self):
        # Fairness under the conservative backfill, on a fake clock: `c` (15 of 24)
        # would move the start of `n` (after `pre`, which overruns its 50 s) and waits.
        # From 10 to 40 `x` runs and `c` does not fit at all -- no reservation keeps it
        # back then, so the bound (60 s) is reached at 90, not at 60.
        sts = [station("pre", "true", calib_s=50),
               station("n", "true", after=["pre"], needs={"cpu": 10}, calib_s=200),
               station("c", "true", needs={"cpu": 15}, calib_s=100),
               station("x0", "true", calib_s=10),
               station("x", "true", after=["x0"], needs={"cpu": 9}, calib_s=30)]
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        clock = [0.0]
        run = executor._Run(d, io.StringIO(), io.StringIO(), None, clock=lambda: clock[0])
        run._station = lambda st: None

        def at(t):
            clock[0] = t
            while run._start_ready():
                pass

        def end(name):
            for k, v in executor._needs(run.by_name[name]).items():
                run.used[k] -= v
            run._finish(name, "GREEN", 0)
        with mock.patch.object(executor, "RESERVE_AGE_S", 60.0):
            at(0)
            self.assertEqual(("running", "pending", "running"),
                             (run.state["pre"], run.state["c"], run.state["x0"]))
            end("x0")
            at(10)
            self.assertEqual(("running", "pending"), (run.state["x"], run.state["c"]))
            end("x")
            at(40)
            at(60)
            self.assertEqual("pending", run.state["c"],
                             "the 30 s in which c did not fit counted toward the bound")
            self.assertAlmostEqual(30.0, run._age_wait())
            at(90)
            self.assertEqual("running", run.state["c"])

    def fake_run(self, sts):
        d = self.dispatch(sts, capacity=self.CAP24)
        d["+cpu_overbook"] = 1.0
        run = executor._Run(d, io.StringIO(), io.StringIO(), None, clock=kit_time.FakeClock())
        run._station = lambda st: None
        return run

    def test_a_station_waiting_for_the_cargo_lock_holds_its_reservation(self):
        # GH #1162 OR-TB.S4.27 (hA-m-2), on a fake clock: `cb` (cargo, 20 of 24) is
        # ready, the lock comes late. Its reservation stands from the first pass, so
        # `long` (8 for 30 s) waits instead of taking the room `cb` needs.
        run = self.fake_run([station("cb", "true", needs={"cpu": 20, "cargo": 1}, calib_s=50),
                             station("long", "true", needs={"cpu": 8}, calib_s=30)])
        run.lock_state = "waiting"      # the lock thread is out; no real lock here
        with mock.patch.object(executor, "RESERVE_AGE_S", 60.0):
            while run._start_ready():
                pass
            self.assertEqual(("pending", "pending"), (run.state["cb"], run.state["long"]))
            run.lock_state = "held"
            while run._start_ready():
                pass
            self.assertEqual(("running", "pending"), (run.state["cb"], run.state["long"]))

    def test_a_station_that_will_be_skipped_reserves_no_room(self):
        # hA-m-3: `doomed` (20 of 24) waits for `pre` and will be skipped -- `cand`
        # (8 for 30 s) moves no reservation of it.
        run = self.fake_run([station("pre", "true", calib_s=10),
                             station("doomed", "true", after=["pre"], needs={"cpu": 20},
                                     calib_s=100),
                             station("cand", "true", needs={"cpu": 8}, calib_s=30)])
        run.state["pre"], run.started_at["pre"] = "running", 0.0
        needs = executor._needs(run.by_name["cand"])
        self.assertFalse(run._moves_reservations([("doomed", True)], needs, 30.0, 0.0))
        self.assertTrue(run._moves_reservations([("doomed", False)], needs, 30.0, 0.0))

    def test_a_station_without_an_estimate_moves_every_reservation(self):
        # hA-m-3: unknown counts as moving -- it never backfills past a reservation.
        run = self.fake_run([station("a", "true", calib_s=10),
                             station("b", "true", needs={"cpu": 1})])
        needs = executor._needs(run.by_name["b"])
        self.assertTrue(run._moves_reservations([("a", False)], needs, None, 0.0))
        self.assertFalse(run._moves_reservations([], needs, None, 0.0))

    def test_a_reservation_that_holds_while_nothing_runs_waits_for_the_age_bound(self):
        # GH #1162 hA-m-1: with nothing running, a station kept back by a reservation
        # is no dead edge -- SKIP `unreachable` there let a pass end GREEN while
        # nothing ran. It waits for RESERVE_AGE_S and starts.
        sts = [station("a", "true", calib_s=1), station("b", "true", calib_s=1)]
        d = self.dispatch(sts, capacity=self.CAP24)
        with mock.patch.object(executor, "RESERVE_AGE_S", 0.2), \
                mock.patch.object(executor._Run, "_moves_reservations",
                                  lambda self, *a: True):
            rc, evs = self.run_exec(d)
        ended = self.ended(evs)
        self.assertEqual({"a": "GREEN", "b": "GREEN"},
                         {n: e["verdict"] for n, e in ended.items() if n in ("a", "b")},
                         ended)
        self.assertEqual(0, rc, self.err_text)

    def test_executor_precheck_red_skips_build_dependents(self):
        sts = [station("precheck", "exit 1"),
               station("build", "true", after=["precheck"], needs={"cargo": 1}),
               station("tests", "true", after=["precheck", "build"]),
               station("scenarios:memory", "true", after=["precheck", "build"]),
               station("clippy", "true", after=["precheck"], needs={"cargo": 1}),
               station("docs-links", "true", after=["precheck"])]
        rc, evs = self.run_exec(self.dispatch(sts))
        self.assertEqual(1, rc)
        ended = self.ended(evs)
        for name in ("build", "tests", "scenarios:memory", "clippy"):
            self.assertEqual(("SKIP", "precheck red"),
                             (ended[name]["verdict"], ended[name].get("reason")), name)
        self.assertEqual("GREEN", ended["docs-links"]["verdict"])
        self.assertEqual("RED", ended["precheck"]["verdict"])
        # ... and `+no_precheck_stop` lets the cargo stations run all the same.
        rc, evs = self.run_exec(self.dispatch(sts, **{"+no_precheck_stop": True}))
        self.assertEqual("GREEN", self.ended(evs)["build"]["verdict"])

    def test_executor_when_any_runs_after_red(self):
        mark = self.root / "audit-ran"
        sts = [station("a", "exit 1"),
               station("b", "true", after=["a"]),
               station("export-audit", "touch %s" % mark, after=["a", "b"], when="any")]
        rc, evs = self.run_exec(self.dispatch(sts))
        self.assertEqual(1, rc)
        ended = self.ended(evs)
        self.assertEqual("SKIP", ended["b"]["verdict"])
        self.assertEqual("GREEN", ended["export-audit"]["verdict"])
        self.assertTrue(mark.exists())

    def test_executor_timeout_kills_station_and_reports(self):
        rc, evs = self.run_exec(self.dispatch([station("slow", "sleep 20", timeout_min=0.02)]))
        self.assertEqual(1, rc)
        end = self.ended(evs)["slow"]
        self.assertEqual("RED", end["verdict"])
        self.assertEqual("HANG: station slow exceeded 0.02 min -- log logs/slow.log",
                         end.get("reason"))  # #1150, TV-K5
        self.assertEqual(124, end["exit"])

    def test_a_station_over_its_cap_is_red_as_a_hang(self):
        """TV-K5 (b), ruling K6 2026-10-11, ADR-0050: a station cap is a hang
        marker, not a run-time budget. Exceeding it kills the station and ends
        it RED as `HANG: station <name> exceeded <n> min`, with the diagnosis in
        the log. `sleep 20` finishing would be GREEN with exit 0 -- RED with 124
        is the kill, no wall clock needed. What the diagnosis holds is pinned on
        `hang_diagnosis` itself (HangDiagnosis): asserting it here assumed the
        shell had written its lines inside the 1.2-s cap (re-check tv-p0 B3, TV-K2)."""
        rc, evs = self.run_exec(self.dispatch([station("slow", "sleep 20", timeout_min=0.02)]))
        self.assertEqual(1, rc)
        end = self.ended(evs)["slow"]
        self.assertEqual("RED", end["verdict"])
        self.assertTrue(end["reason"].startswith("HANG: station slow exceeded 0.02 min"), end)
        self.assertTrue(end["reason"].endswith("-- log logs/slow.log"), end)  # #1150
        self.assertEqual(124, end["exit"])
        log = (self.ws / "logs" / "slow.log").read_text()
        self.assertIn("HANG: station slow exceeded 0.02 min", log)
        self.assertIn(executor.HANG_TREE_HEAD, log)


class HangDiagnosis(unittest.TestCase):
    """TV-K5 (b): the diagnosis a hang writes before the kill -- the line, the
    running test, the last 40 log lines and the process tree of the session.
    Ordered by events, not by time (re-check tv-p0 B3): the log is written
    before the call, and the child is in the tree once it has said `ready`."""

    MARK = "hang-probe-%d" % os.getpid()

    def test_the_diagnosis_names_the_test_the_tail_and_the_tree(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = os.path.join(tmp.name, "slow.log")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("".join("line-%d\n" % i for i in range(1, 46)))
            fh.write("        SLOW [> 60.000s] meclaw-cells::misc hangs::forever\n")
        # The child blocks on its stdin until the test closes it: no sleep, and
        # its `ready` line is the rendezvous -- by then its argv is the probe's.
        child = subprocess.Popen(
            [sys.executable, "-c", "import sys; print('ready', flush=True); sys.stdin.read()",
             self.MARK], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
            start_new_session=True)
        self.addCleanup(child.wait)
        self.addCleanup(child.stdin.close)
        self.assertEqual("ready\n", child.stdout.readline())
        with open(path, "a", encoding="utf-8") as log:
            executor.hang_diagnosis({"name": "slow", "timeout_min": 0.02}, log, child.pid)
        text = pathlib.Path(path).read_text()
        self.assertIn("HANG: station slow exceeded 0.02 min", text)
        self.assertIn("running test: meclaw-cells::misc hangs::forever", text)
        tail = text.split("--- last 40 log lines ---\n", 1)[1].split("--- process tree", 1)[0]
        self.assertEqual(40, len(tail.splitlines()), tail)
        self.assertNotIn("line-5\n", tail)
        self.assertIn("line-45\n", tail)
        tree = text.split(executor.HANG_TREE_HEAD + "\n", 1)[1]
        self.assertIn(self.MARK, tree)

    def test_the_diagnosis_names_state_fds_stack_and_outside_pipe_holders(self):
        # Fix round 4 v2-r3-kit: three HANG dumps showed a Python stack waiting on a
        # child and a tree without that child. The diagnosis now names, for every
        # process of the session, its state and wait channel, its open fds and its
        # kernel stack, and who OUTSIDE the session holds one of its pipes. A
        # sleeping fake child (blocked on a pipe whose writer is this test) is the
        # rendezvous: the event `ready <pid>`, no time.
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = os.path.join(tmp.name, "hung.log")
        pathlib.Path(path).write_text("$ a station\n")
        leader = subprocess.Popen(
            [sys.executable, "-c",
             "import subprocess, sys\n"
             "kid = subprocess.Popen([sys.executable, '-c', 'import sys; sys.stdin.read()', %r],\n"
             "                       stdin=sys.stdin)\n"
             "print('ready', kid.pid, flush=True)\n"
             "kid.wait()\n" % self.MARK],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, start_new_session=True)
        self.addCleanup(leader.wait)
        self.addCleanup(leader.stdout.close)
        self.addCleanup(leader.stdin.close)
        word, kid = leader.stdout.readline().split()
        self.assertEqual("ready", word)
        with open(path, "a", encoding="utf-8") as log:
            executor.hang_diagnosis({"name": "hung", "timeout_min": 20}, log, leader.pid)
        text = pathlib.Path(path).read_text()
        tree = text.split(executor.HANG_TREE_HEAD + "\n", 1)[1]
        self.assertRegex(tree, r"PID +PPID +STAT +WCHAN")
        row = [ln for ln in tree.splitlines() if ln.split()[:2] == [kid, str(leader.pid)]]
        self.assertEqual(1, len(row), tree)
        self.assertRegex(row[0].split()[2], r"^[DRSTtZI]", row)   # its state, then wchan
        for pid in (str(leader.pid), kid):
            fds = text.split("--- fds of %s (ls -l /proc/%s/fd) ---\n" % (pid, pid), 1)[1]
            self.assertRegex(fds.split("---", 1)[0], r"(?m)^0 -> pipe:\[\d+\]$")
            self.assertIn("--- kernel stack of %s (/proc/%s/stack) ---\n" % (pid, pid), text)
        outside = text.split("--- pipes of the session held outside it ---\n", 1)[1]
        self.assertRegex(outside, r"(?m)^pipe:\[\d+\] .*\bpid %d\b" % os.getpid())


class HangDump(unittest.TestCase):
    """TV-K5 (b) / TV-Z5: Python under a station dumps its stacks 30 s before the
    station's cap -- at the executor's monotonic instant, injected clock."""

    def test_the_dump_arms_at_the_instant_before_the_cap(self):
        from scripts.pipeline import hang_dump
        self.assertEqual("970.000", hang_dump.at(1000.0))
        with mock.patch("faulthandler.dump_traceback_later") as dump:
            got = hang_dump.arm({hang_dump.ENV: "970.000"}, clock=lambda: 900.0)
        self.assertEqual(70.0, got)
        dump.assert_called_once_with(70.0, repeat=False, exit=False)
        with mock.patch("faulthandler.dump_traceback_later") as dump:
            self.assertIsNone(hang_dump.arm({hang_dump.ENV: "970.000"}, clock=lambda: 980.0))
            self.assertIsNone(hang_dump.arm({}, clock=lambda: 0.0))
            self.assertIsNone(hang_dump.arm({hang_dump.ENV: "x"}, clock=lambda: 0.0))
        dump.assert_not_called()

    def test_the_wrapper_runs_the_module_like_python_dash_m(self):
        from scripts.pipeline import hang_dump
        res = subprocess.run(hang_dump.argv(sys.executable, "unittest")
                             + ["scripts.tests.test_pipeline_executor.HangDump"
                                ".test_the_dump_arms_at_the_instant_before_the_cap"],
                             cwd=str(pathlib.Path(__file__).resolve().parents[2]),
                             capture_output=True, text=True, timeout=4 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("Ran 1 test", res.stderr)


class TestExecutorEvents(ExecutorCase):
    def test_executor_emits_events_in_contract_order(self):
        sts = [station("a", "echo hello"), station("b", "exit 3", after=["a"]),
               station("c", "true")]
        d = self.dispatch(sts, cache_known=[{"station": "c", "part": "", "key": "k" * 64,
                                             "from_run": "run-feedfeedfeed"}])
        rc, evs = self.run_exec(d)
        self.assertEqual(1, rc)
        self.assertEqual("hygiene_checked", evs[0]["event"])
        self.assertEqual("lane_fingerprint", evs[1]["event"])
        self.assertEqual("pass_ended", evs[-1]["event"])
        for e in evs:
            # The W0a stream line: the journal row without `seq` (and without `item`).
            self.assertEqual({"event", "kind", "at", "run", "text", "data"}, set(e))
            self.assertEqual(RUN, e["run"])
            # Every line passes the contract table (required fields present).
            events.validate(e["event"], e["kind"], e["data"])
        fp = evs[1]["data"]
        for key in ("rustc", "cargo", "nextest", "node", "playwright", "python", "kernel",
                    "vcpu", "ram_g", "target_fs", "target_free_g", "ulimit_n",
                    "incremental_policy", "linker", "sccache", "linger", "pdftotext",
                    "portal_venv"):
            self.assertIn(key, fp)
        per = {}
        for i, e in enumerate(evs):
            st = e["data"].get("station")
            if st:
                per.setdefault(st, []).append(e["event"])
        self.assertEqual(["station_started", "station_log", "station_ended"], per["a"])
        self.assertEqual(["station_started", "station_log", "station_ended"], per["b"])
        self.assertEqual(["cache_hit", "station_ended"], per["c"])
        self.assertEqual(1, sum(1 for e in evs if e["event"] == "station_ended"
                                and e["data"]["station"] == "a"))
        pe = evs[-1]["data"]
        self.assertEqual("RED", pe["verdict"])
        self.assertEqual({"green": 1, "red": 1, "cached": 1, "flaky": 0, "skip": 0,
                          "note": 0, "ask": 0, "aborted": 0}, pe["stations"])
        self.assertEqual(1, pe["cache_hits"])
        self.assertEqual(1, pe["rounds"])
        with open(self.ws / "events.jsonl", encoding="utf-8") as fh:
            self.assertEqual(evs, list(events.read_jsonl(fh)))

    def test_lane_drift_names_only_properties_of_the_lane(self):
        # tv-bn M7o (pre 30/31, test_pipeline_ir_path.CacheOffLane under four shards): the
        # second lane run of a test compared the measured free space of the first one and
        # a probe that timed out under load (None) -- a `lane-drift` NOTE for a lane that
        # had not changed. A measure and a silent probe say nothing; a changed tool does.
        first = self.run_exec(self.dispatch([station("a", "true")]))[1][1]["data"]
        expected = {k: v for k, v in first.items() if k != "run"}
        expected["target_free_g"] = (expected["target_free_g"] or 0) + 7.3
        expected["rustc"] = "rustc 0.0.0 (seen before)"
        quiet = dict(expected, rustc=None)
        rc, evs = self.run_exec(self.dispatch([station("a", "true")], fingerprint_expected=quiet))
        self.assertEqual(0, rc, self.err_text)
        self.assertNotIn("lane-drift", self.ended(evs))
        with mock.patch.object(executor, "lane_fingerprint",
                               side_effect=lambda *a: dict(first, rustc=None)):
            rc, evs = self.run_exec(self.dispatch([station("a", "true")],
                                                  fingerprint_expected=expected))
        self.assertNotIn("lane-drift", self.ended(evs))
        moved = dict(quiet, kernel="0.0.0-other")
        rc, evs = self.run_exec(self.dispatch([station("a", "true")], fingerprint_expected=moved))
        self.assertEqual("NOTE", self.ended(evs)["lane-drift"]["verdict"])
        self.assertEqual("kernel", self.ended(evs)["lane-drift"]["scope"])

    def test_executor_writes_receipt_after_each_station_and_legacy_files(self):
        snap = self.root / "snapshot.json"
        legacy_snap = self.root / "legacy-snapshot.json"
        sts = [station("a", "true"),
               station("b", 'cp "%s" "%s"' % (self.ws / "receipt.json", snap), after=["a"]),
               station("export-audit", None, after=["a", "b"], when="any",
                       cmds=[{"argv": ["sh", "-c", 'cp "$1" "%s"' % legacy_snap, "x",
                                       "{receipt}"], "cwd": ""}])]
        rc, evs = self.run_exec(self.dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        mid = json.loads(snap.read_text())
        self.assertEqual("meclaw.gate.receipt/2", mid["schema"])
        self.assertEqual(["a"], [s["name"] for s in mid["stations"]])
        self.assertIsNone(mid["finished"])
        # `{receipt}` is the legacy file, with every earlier verdict in it.
        old = json.loads(legacy_snap.read_text())
        self.assertNotIn("schema", old)
        self.assertEqual(["a", "b"], [s["name"] for s in old["stations"]])
        gate_dir = self.target / "gate" / self.tree.name
        final = json.loads((gate_dir / ("strand-%s.json" % REV)).read_text())
        self.assertEqual(final, json.loads((gate_dir / "last-strand.json").read_text()))
        self.assertEqual({"mode", "rev", "base", "dirty", "lock_wait_secs", "archive", "plan",
                          "started", "finished", "stations", "verdict", "owner_decisions"},
                         set(final))
        self.assertEqual("GREEN", final["verdict"])
        self.assertEqual(["a", "b", "export-audit"], final["plan"])
        v2 = json.loads((self.ws / "receipt.json").read_text())
        self.assertIsNotNone(v2["finished"])
        self.assertEqual(3, len(v2["stations"]))
        self.assertTrue((self.ws / "logs" / "a.log").exists())

    def test_executor_cache_hit_skips_command_and_emits_event(self):
        mark = self.root / "ran"
        d = self.dispatch([station("catalogue", "touch %s" % mark)],
                          cache_known=[{"station": "catalogue", "part": "", "key": "f" * 64,
                                        "from_run": "run-aaaaaaaaaaaa"}])
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        self.assertFalse(mark.exists())
        hits = [e["data"] for e in evs if e["event"] == "cache_hit"]
        self.assertEqual([{"run": RUN, "station": "catalogue", "part": "", "key": "f" * 64,
                           "from_run": "run-aaaaaaaaaaaa"}], hits)
        end = self.ended(evs)["catalogue"]
        self.assertEqual(("CACHED", 0), (end["verdict"], end["secs"]))
        self.assertEqual([], self.order(evs, "station_started"))
        v2 = json.loads((self.ws / "receipt.json").read_text())
        self.assertEqual("run-aaaaaaaaaaaa", v2["stations"][0]["cache_from"])


    def test_a_flaky_station_reports_its_seconds_with_the_isolation(self):
        """GH #1161 (S3-diag): the GATE line of a FLAKY station showed the seconds of
        its first, red call (`scenarios:memory` 3 s) and not the 137 s it held the
        lane with its isolation -- the line and the critical path read the same secs.
        On the fake clock (TV-K1) the red call costs 0 s, the isolation 137 s: the
        line books exactly the 137."""
        clock = kit_time.FakeClock()

        def slow_isolation(run, st, cmd, log_path, started_wall, junits):
            clock.advance(137)
            return {"verdict": "FLAKY", "isolated": 1, "kind": "unittest", "events": []}

        with mock.patch.object(executor._Run, "_isolate", slow_isolation):
            rc, evs = self.run_exec(self.dispatch([station("memory", "exit 1")]),
                                    clock=clock.monotonic)
        ended = self.ended(evs)["memory"]
        self.assertEqual("FLAKY", ended["verdict"], ended)
        self.assertEqual(137.0, ended["secs"], ended)
        passed = [e["data"] for e in evs if e["event"] == "pass_ended"][-1]
        self.assertEqual(137.0, passed["critical_path_s"], passed)
        self.assertEqual(1, rc, self.err_text)  # FLAKY ends the pass RED (VG TV-F1)


class TestExecutorNextest(ExecutorCase):
    def _tests_station(self, parts, flt="binary(/x/)"):
        return station("tests", None, after=[], parts=parts,
                       nextest={"profile": "default", "filter": flt},
                       cmds=[{"argv": ["scripts/test-tier.sh", "filter", flt], "cwd": ""}])

    def tier_lines(self):
        return [ln.split("\t") for ln in self.tier_calls.read_text().splitlines()] \
            if self.tier_calls.exists() else []

    def test_executor_reuse_tells_the_tier_the_gate_decided_where_it_runs(self):
        # Delta C M-neu-3: test-tier.sh asks the #942 probe under a reuse set
        # by hand; the executor's own reuse carries MECLAW_TIER_LANE.
        self.fake_tier()
        reuse = self.ws / "reuse"
        reuse.mkdir(parents=True, exist_ok=True)
        (reuse / "binaries.json").write_text("{}")
        rc, evs = self.run_exec(self.dispatch([self._tests_station(["a::x"])]))
        self.assertEqual(0, rc, self.err_text)
        (profile, reused, args), = self.tier_lines()
        self.assertEqual("%s lane=1" % reuse, reused)

    def test_executor_tests_round_two_excludes_cached_binaries(self):
        self.fake_tier()
        d = self.dispatch([self._tests_station(["a::x", "a::y", "b::z"], "rdeps(a)")],
                          cache_known=[
                              {"station": "tests", "part": "a::x", "key": "1" * 64,
                               "from_run": "run-111111111111"},
                              # not a part of this plan: never in the filterset
                              {"station": "tests", "part": "a::ghost", "key": "2" * 64,
                               "from_run": "run-111111111111"}])
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        (profile, _reuse, args), = self.tier_lines()
        self.assertEqual("default", profile)
        self.assertEqual("filter (rdeps(a)) & not (binary_id(=a::x))", args)
        hits = [e["data"]["part"] for e in evs if e["event"] == "cache_hit"]
        self.assertEqual(["a::x"], hits)
        self.assertEqual("GREEN", self.ended(evs)["tests"]["verdict"])

    # GH #1167 (i): the merge-queue gate of pd-1157 was RED in `tests` after
    # 0.25 s -- the cached binaries subtracted from the filter left nothing,
    # and test-tier exits 6 `no tests matched`.
    EMPTY = {"FAKE_TIER_RC": "6",
             "FAKE_TIER_STDERR": "test-tier: RED -- no tests matched: (rdeps(a)) & not (...)"}

    def _emptied(self, cache_known, parts=("a::x", "a::y"), env=None, cache_keys=None,
                 station=None, **kw):
        self.fake_tier()
        keys = {"tests::%s" % ck["part"]: ck["key"] for ck in cache_known if ck.get("key")}
        d = self.dispatch([station or self._tests_station(list(parts), "rdeps(a)")],
                          cache_known=cache_known,
                          cache_keys=keys if cache_keys is None else cache_keys)
        d["+env"].update(self.EMPTY)
        d["+env"].update(env or {})
        return self.run_exec(d, **kw)

    HIT_X = {"station": "tests", "part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"}

    def test_i_a_selection_emptied_by_green_hits_is_cached(self):
        rc, evs = self._emptied([
            {"station": "tests", "part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"}])
        self.assertEqual(0, rc, self.err_text)
        end = self.ended(evs)["tests"]
        self.assertEqual("CACHED", end["verdict"])
        self.assertEqual([{"part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"}],
                         end["cached_parts"])
        self.assertIn("run-111111111111", end["reason"])

    # Re-check H2 B1 / V1 / m1: CACHED needs every point PROVEN.

    def test_i_a_refused_filterset_is_no_empty_rest(self):
        rc, evs = self._emptied([self.HIT_X], env={
            "FAKE_TIER_STDERR": "test-tier: RED -- no tests matched (filterset refused): x"})
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_a_remainder_that_lists_tests_stays_red(self):
        rc, evs = self._emptied([self.HIT_X], env={"FAKE_TIER_LISTED": "3"})
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])
        lists = [ln for ln in self.tier_calls.read_text().splitlines() if ln.startswith("LIST")]
        self.assertEqual(1, len(lists))
        self.assertIn("not (binary_id(=a::x))", lists[0])

    def test_i_a_failing_list_stays_red(self):
        rc, evs = self._emptied([self.HIT_X], env={"FAKE_TIER_LIST_RC": "94"})
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_a_remainder_missing_from_the_reused_build_stays_red(self):
        reuse = self.ws / "reuse"
        reuse.mkdir(parents=True)
        (reuse / "binaries.json").write_text(json.dumps({"rust-binaries": {"a::x": {}}}))
        rc, evs = self._emptied([self.HIT_X])
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_a_subtracted_part_without_a_plan_key_stays_red(self):
        rc, evs = self._emptied([self.HIT_X], cache_keys={})
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_exit_6_of_a_command_the_cache_did_not_cut_stays_red(self):
        st = self._tests_station(["a::x", "a::y"], "rdeps(a)")
        st["cmds"].append({"argv": ["scripts/test-tier.sh", "t0"], "cwd": ""})
        rc, evs = self._emptied([self.HIT_X], station=st,
                                env={"FAKE_TIER_RC": "0", "FAKE_TIER_STDERR": "",
                                     "FAKE_TIER_T0_RC": "6"})
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def _browser(self, parts=("w::csp", "w::sheet"), timeout_min=1):
        cmds = [{"argv": a, "cwd": ""} for a in gate_plan.browser_cmds(False)]
        return station("browser:display", None, after=[], parts=list(parts),
                       nextest={"profile": "browser", "filter": "x"}, cmds=cmds,
                       timeout_min=timeout_min)

    def test_i_n1_the_list_proof_runs_as_the_stations_process(self):
        # Re-check H2 m-N1: the `nextest list` proof goes through `_spawn`
        # (registered, killed by an abort, the station's deadline) -- an abort
        # that lands before it leaves the station RED, never CACHED.
        spawn = executor._Run._spawn
        lists = []

        def aborting(run, st, argv, cwd, env, log, t0, capture=None):
            if env.get("MECLAW_TIER_LIST"):
                lists.append(argv)
                with run.cond:
                    run.aborted = True
            return spawn(run, st, argv, cwd, env, log, t0, capture=capture)

        with mock.patch.object(executor._Run, "_spawn", aborting):
            rc, evs = self._emptied([self.HIT_X])
        self.assertEqual(1, len(lists), lists)
        self.assertNotEqual("CACHED", self.ended(evs)["tests"]["verdict"])
        self.assertNotIn("cached_parts", self.ended(evs)["tests"])
        self.assertFalse([ln for ln in self.tier_lines() if ln[0] == "LIST"])

    def _browser_emptied(self, env, hit="w::csp", timeout_min=1, **kw):
        """browser:display with `hit` cached: the commands run against the fake
        tier with `env` (re-check H2 M-N2 / m-N2)."""
        self.fake_tier()
        self.write_exe(self.bin / "node", "#!/bin/sh\necho v22.0.0\n")
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(
            parents=True, exist_ok=True)
        d = self.dispatch([self._browser(timeout_min=timeout_min)], cache_known=[
            {"station": "browser:display", "part": hit, "key": "1" * 64,
             "from_run": "run-111111111111"}],
            cache_keys={"browser:display::%s" % hit: "1" * 64})
        d["+env"].update(env)
        rc, evs = self.run_exec(d, **kw)
        runs = [c for c in self.tier_lines() if c[0] != "LIST"]
        return rc, self.ended(evs)["browser:display"], runs

    LOCKS_EMPTY = {"FAKE_TIER_RC": "6",
                   "FAKE_TIER_STDERR": "test-tier: RED -- no tests matched: (x) & not (y)"}

    def test_i_n2_after_an_emptied_command_a_red_rest_is_red(self):
        # The locks emptied by the cache say nothing about the sheet.
        rc, end, runs = self._browser_emptied(dict(self.LOCKS_EMPTY, FAKE_TIER_WEBKIT_RC="1"))
        self.assertEqual(1, rc)
        self.assertEqual("RED", end["verdict"])
        self.assertEqual(2, len(runs), runs)
        self.assertIn("& test(/webkit/)", runs[1][2])

    def test_i_n2_after_an_emptied_command_a_green_rest_is_green(self):
        rc, end, runs = self._browser_emptied(dict(self.LOCKS_EMPTY, FAKE_TIER_WEBKIT_RC="0"))
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual("GREEN", end["verdict"])
        self.assertIn("the rest ran", end["reason"])
        self.assertEqual(2, len(runs), runs)
        self.assertIn("& test(/webkit/)", runs[1][2])

    def test_i_n2_an_emptied_command_after_a_green_one_is_green_not_cached(self):
        # Re-check H2 m-N2: the locks RAN green, then the cut emptied the sheet.
        rc, end, runs = self._browser_emptied(
            {"FAKE_TIER_WEBKIT_RC": "6",
             "FAKE_TIER_WEBKIT_STDERR": "test-tier: RED -- no tests matched: (x) & not (y)"},
            hit="w::sheet")
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual("GREEN", end["verdict"])
        self.assertIn("1 earlier command(s) ran green", end["reason"])
        self.assertEqual([{"part": "w::sheet", "key": "1" * 64, "from_run": "run-111111111111"}],
                         end["cached_parts"])
        self.assertEqual(2, len(runs), runs)

    def _browser_parts_emptied(self, env, hit="w::csp"):
        """browser:display as the plan builds it since GH #1161: the two engine
        parts side by side (`parallel` by cmd)."""
        self.fake_tier()
        self.write_exe(self.bin / "node", "#!/bin/sh\necho v22.0.0\n")
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(
            parents=True, exist_ok=True)
        st = self._browser()
        st["parallel"] = {"by": "cmd", "profiles": list(gate_plan.BROWSER_PROFILES)}
        d = self.dispatch([st], cache_known=[
            {"station": "browser:display", "part": hit, "key": "1" * 64,
             "from_run": "run-111111111111"}],
            cache_keys={"browser:display::%s" % hit: "1" * 64})
        d["+env"].update(env)
        rc, evs = self.run_exec(d)
        runs = [c for c in self.tier_lines() if c[0] != "LIST"]
        return rc, self.ended(evs)["browser:display"], runs

    def test_restack_an_emptied_part_does_not_run_the_other_part_twice(self):
        # Restack tb-s7 on GH #1161: the parts of a `parallel` station all ran
        # once -- after the Chromium part was emptied by the cache, the WebKit
        # part (327 s of 469.6 s in the 10.10. integration pass) must not run
        # a second time; its own green keeps the station.
        rc, end, runs = self._browser_parts_emptied(
            dict(self.LOCKS_EMPTY, FAKE_TIER_WEBKIT_RC="0"))
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual("GREEN", end["verdict"])
        self.assertEqual(2, len(runs), runs)
        self.assertEqual(1, sum("& test(/webkit/)" in r[2] for r in runs), runs)

    def test_restack_an_emptied_part_beside_a_red_part_is_red(self):
        rc, end, runs = self._browser_parts_emptied(
            dict(self.LOCKS_EMPTY, FAKE_TIER_WEBKIT_RC="1"))
        self.assertEqual(1, rc)
        self.assertEqual("RED", end["verdict"])
        self.assertNotIn("cached_parts", end)
        self.assertEqual(1, sum("& test(/webkit/)" in r[2] for r in runs), runs)

    # Re-check H2 M-N3 / m-N5: the station has ONE deadline, counted from its
    # start (`t0`). Measured on b2e3a1645: deadline 6 s, locks 4 s (emptied),
    # sheet 4 s -> GREEN after 8.0 s, because the rest after the cache cut got
    # a fresh clock. On a fake clock (TV-K1) the deadline each command is
    # handed is exact: 6 s for the first, what is left of them for the rest.
    DEADLINE_MIN = kit_time.virtual(6) / 60  # 6 s on the fake clock

    def _fake_seconds(self, clock, handed):
        """`_wrap` on the fake clock: a command costs `FAKE_COST` fake seconds
        (`FAKE_LIST_COST` for the list proof); one that costs more than the
        deadline it is handed ends like the `timeout` utility, exit 124 after
        exactly that deadline. `handed` collects the deadlines."""
        def wrap(run, st, argv, remaining_s, env):
            cost = float(env.get("FAKE_LIST_COST" if env.get("MECLAW_TIER_LIST") else "FAKE_COST")
                         or 0)
            handed.append(remaining_s)
            if cost > remaining_s:
                clock.advance(remaining_s)
                return ["sh", "-c", "exit 124"]
            clock.advance(cost)
            return list(argv)
        return mock.patch.object(executor._Run, "_wrap", wrap)

    def test_m_n3_the_rest_after_an_emptied_command_keeps_the_stations_deadline(self):
        clock, handed = kit_time.FakeClock(), []
        with self._fake_seconds(clock, handed):
            rc, end, _runs = self._browser_emptied(
                dict(self.LOCKS_EMPTY, FAKE_TIER_WEBKIT_RC="0", FAKE_COST="4"),
                timeout_min=self.DEADLINE_MIN, clock=clock.monotonic)
        self.assertEqual(1, rc, end)
        self.assertEqual("RED", end["verdict"])
        self.assertTrue(end["reason"].startswith("HANG: station "), end["reason"])  # TV-K5
        # locks 6 s, the list proof and the sheet what the locks left of them
        self.assertEqual([6.0, 2.0, 2.0], handed, "the rest got a fresh deadline")
        self.assertEqual(6.0, end["secs"])

    def test_m_n5_the_list_proof_keeps_the_stations_deadline(self):
        # The emptied run takes 4 s, the list proof another 4 s: with the
        # deadline from `t0` the proof is cut at 6 s and proves nothing.
        clock, handed = kit_time.FakeClock(), []
        st = station("tests", None, after=[], parts=["a::x", "a::y"],
                     nextest={"profile": "default", "filter": "rdeps(a)"},
                     cmds=[{"argv": ["scripts/test-tier.sh", "filter", "rdeps(a)"], "cwd": ""}],
                     timeout_min=self.DEADLINE_MIN)
        with self._fake_seconds(clock, handed):
            rc, evs = self._emptied([self.HIT_X], station=st, clock=clock.monotonic,
                                    env={"FAKE_COST": "4", "FAKE_LIST_COST": "4"})
        end = self.ended(evs)["tests"]
        self.assertEqual(1, rc, end)
        self.assertEqual("RED", end["verdict"])
        self.assertNotIn("cached_parts", end)
        self.assertEqual([6.0, 2.0], handed, "the list proof got a fresh deadline")

    def test_i_n3_a_hit_with_the_plan_key_but_without_its_run_stays_red(self):
        # Re-check H2 m-N3: the key alone is no proof -- the run it came from is.
        rc, evs = self._emptied([dict(self.HIT_X, from_run="")])
        self.assertEqual(1, rc)
        end = self.ended(evs)["tests"]
        self.assertEqual("RED", end["verdict"])
        self.assertNotIn("cached_parts", end)

    def test_v1_a_cached_lock_keeps_run_ignored_for_the_rest(self):
        self.fake_tier()
        self.write_exe(self.bin / "node", "#!/bin/sh\necho v22.0.0\n")
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(parents=True)
        d = self.dispatch([self._browser()], cache_known=[
            {"station": "browser:display", "part": "w::sheet", "key": "1" * 64,
             "from_run": "run-111111111111"}],
            cache_keys={"browser:display::w::sheet": "1" * 64})
        rc, evs = self.run_exec(d)
        calls = self.tier_lines()
        self.assertEqual(2, len(calls), calls)
        self.assertIn("--run-ignored all", calls[0][2])
        self.assertEqual("browser-ignored", calls[0][0])
        for c in calls:
            self.assertIn("not (binary_id(=w::sheet))", c[2])

    # Re-check H2 M-N1: the stations come from the plan, not from a hand list --
    # every station of `plan.NEXTEST` with the commands `gate_plan.plan` gives it
    # (the `t0` floor, a `filter` run, `browser_cmds(False)` and, in I/R,
    # `browser_cmds(True)`); a nextest station the diffs below do not reach
    # makes the test red.
    NEXTEST_DIFFS = (([], "strand"), (["crates/meclaw-colony/src/lib.rs"], "strand"),
                     (["templates/display/x.json"], "strand"),
                     (["templates/display/x.json"], "integration"),
                     # tb-s10 D: `scenario-tests` is a release-only station of `plan.NEXTEST`
                     (["docs/x.md"], "release"))

    def _planned_nextest_stations(self):
        from scripts.pipeline import plan as plan_mod
        out = []
        for paths, mode in self.NEXTEST_DIFFS:
            # the plan's own split (GH #1191): `browser:alone` is a station of
            # `plan.NEXTEST` that `gate_plan.plan` never names
            rows = [{"name": r.name, "scope": r.scope, "cmds": [list(a) for a in r.cmds],
                     "cwds": list(r.cwds or [])}
                    for r in gate_plan.plan(paths, mode, str(REPO))]
            plan_mod.split_alone(rows)
            for row in rows:
                if row["name"] not in plan_mod.NEXTEST:
                    continue
                argvs = [row["cmd"]] if "cmd" in row else row["cmds"]
                cwds = [row.get("cwd")] if "cmd" in row else list(row.get("cwds") or [])
                cmds = [{"argv": list(a), "cwd": (cwds[k] if k < len(cwds) else "") or ""}
                        for k, a in enumerate(argvs)]
                parts = ["p::%d" % k for k in range(len(cmds) + 1)]
                out.append(station(row["name"], None, after=[], parts=parts, cmds=cmds,
                                   nextest={"profile": plan_mod.NEXTEST[row["name"]],
                                            "filter": row["scope"]}))
        return plan_mod, out

    def _assert_cut_keeps_flags(self, st, stations):
        cache = [{"station": x["name"], "part": x["parts"][-1], "key": "1" * 64,
                  "from_run": "run-111111111111"} for x in stations]
        run = executor._Run(self.dispatch(stations, cache_known=cache,
                                          quarantine=[{"test": "q"}]),
                            io.StringIO(), io.StringIO(), None)
        run._quarantine_argv = lambda: ["--tool-config-file", "pipeline:/q.toml"]
        run_full = executor._Run(self.dispatch(stations, quarantine=[{"test": "q"}]),
                                 io.StringIO(), io.StringIO(), None)
        run_full._quarantine_argv = run._quarantine_argv
        full = run_full._cmds(st)[0]
        cut = run._cmds(st)[0]
        self.assertEqual(len(full), len(cut), st["name"])
        for (fa, fc, fe), (ca, cc, ce) in zip(full, cut):
            self.assertEqual((fc, fe), (cc, ce), st["name"])
            if executor._tier_filter(fa):
                self.assertEqual(fa[:2] + fa[3:], ca[:2] + ca[3:], (st["name"], fa))
                self.assertTrue(ca[2].startswith("(%s) & not (" % fa[2]), (st["name"], ca))
            else:
                self.assertEqual(fa, ca, st["name"])

    def test_v1_every_cut_command_keeps_every_flag_of_the_full_one(self):
        # Property over the planned stations: argv(cut) == argv(full) but the
        # filter, the cut filter starts from the full one, the env (profile)
        # is the same.
        self.fake_tier()
        plan_mod, planned = self._planned_nextest_stations()
        self.assertLessEqual(set(plan_mod.NEXTEST), {st["name"] for st in planned})
        argvs = [c["argv"] for st in planned for c in st["cmds"]]
        # The plan's browser union carries the units outside BROWSER_LOCKS the
        # diff reaches (GH #1161 re-check M-3), so each diff's own commands.
        wants, colonies = [["scripts/test-tier.sh", "t0"]], set()
        for paths, mode in self.NEXTEST_DIFFS:
            if "browser:display" not in {r.name for r in gate_plan.plan(paths, mode, str(REPO))}:
                continue
            ir = mode in ("integration", "release")
            colonies.add(ir)
            # after the plan's split (GH #1191): the alone locks run in their own row
            rows = [{"name": "browser:display", "cmds": gate_plan.browser_cmds(
                ir, moved=gate_plan.browser_moved(paths, mode, str(REPO)))}]
            plan_mod.split_alone(rows)
            for row in rows:
                wants += [row["cmd"]] if "cmd" in row else row["cmds"]
        self.assertEqual({True, False}, colonies)
        for want in wants:
            self.assertIn(want, argvs)
        for st in planned:
            self._assert_cut_keeps_flags(st, [st])
        # and a filter command with flags of its own after the filter
        extra = self._tests_station(["a::x", "a::y"], "rdeps(a)")
        extra["cmds"] = [{"argv": ["scripts/test-tier.sh", "filter", "rdeps(a)", "--features",
                                   "x", "--run-ignored", "all"], "cwd": ""}]
        self._assert_cut_keeps_flags(extra, [extra])

    def test_i_a_part_cached_in_one_station_only_is_built(self):
        # Re-check H2 B1 c: cached in browser:display, run in tests.
        build = station("build", None, after=[], build={"tests": [["w", "sheet"]]})
        tests = self._tests_station(["w::sheet"], "binary(/sheet/)")
        run = executor._Run(self.dispatch([build, tests, self._browser()], cache_known=[
            {"station": "browser:display", "part": "w::sheet", "key": "1" * 64,
             "from_run": "run-111111111111"}]), io.StringIO(), io.StringIO(), None)
        argvs = run._build_argvs(build)
        self.assertTrue(any("sheet" in " ".join(a) for a in argvs), argvs)

    def test_i_a_selection_empty_without_the_cache_stays_red(self):
        rc, evs = self._emptied([])
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_a_subtracted_part_without_a_proven_key_stays_red(self):
        rc, evs = self._emptied([
            {"station": "tests", "part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"},
            {"station": "tests", "part": "a::y", "key": "", "from_run": "run-111111111111"}],
            parts=("a::x", "a::y", "a::z"))
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_i_a_subtracted_part_whose_key_differs_from_the_plan_stays_red(self):
        self.fake_tier()
        d = self.dispatch([self._tests_station(["a::x", "a::y"], "rdeps(a)")], cache_known=[
            {"station": "tests", "part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"}],
            cache_keys={"tests::a::x": "9" * 64})
        d["+env"].update(self.EMPTY)
        rc, evs = self.run_exec(d)
        self.assertEqual(1, rc)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])

    def test_executor_junit_to_per_binary_results(self):
        self.fake_tier()
        junit = self.root / "junit.xml"
        junit.write_text(JUNIT_TWO)
        d = self.dispatch([self._tests_station(["a::x", "b::z"])])
        d["+env"].update({"FAKE_JUNIT": str(junit), "FAKE_TIER_RC": "100"})
        rc, evs = self.run_exec(d)
        self.assertEqual(1, rc)
        res = {e["data"]["part"]: e["data"] for e in evs if e["event"] == "test_result"}
        self.assertEqual({"a::x", "b::z"}, set(res))
        self.assertEqual("GREEN", res["a::x"]["verdict"])
        self.assertEqual({"total": 2, "passed": 2, "failed": 0, "flaky": 0, "skipped": 0},
                         res["a::x"]["tests"])
        self.assertEqual("RED", res["b::z"]["verdict"])
        self.assertEqual(["broken"], res["b::z"]["failed_names"])
        self.assertAlmostEqual(0.75, res["a::x"]["secs"], places=3)
        self.assertEqual("RED", self.ended(evs)["tests"]["verdict"])
        v2 = json.loads((self.ws / "receipt.json").read_text())
        row = v2["stations"][0]
        self.assertEqual({"total": 4, "passed": 3, "failed": 1, "flaky": 0, "skipped": 0},
                         row["tests"])
        # W0a counts the parts of a station; the parts themselves are the test_results.
        self.assertEqual(2, row["parts"])

    def test_executor_all_parts_cached_starts_no_tests(self):
        self.fake_tier()
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1},
                        build={"meclaw_bin": False, "workspace": False, "packages": [],
                               "tests": [["a", "x"], ["a", "y"]], "lib_bins": [], "extra": []})
        tests = self._tests_station(["a::x", "a::y"])
        tests["after"] = ["build"]
        d = self.dispatch([build, tests], cache_known=[
            {"station": "tests", "part": p, "key": "3" * 64, "from_run": "run-222222222222"}
            for p in ("a::x", "a::y")])
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual([], self.tier_lines())
        self.assertEqual([], [c for c in self.call_lines() if "--version" not in c])
        ended = self.ended(evs)
        self.assertEqual(("CACHED", 0), (ended["tests"]["verdict"], ended["tests"]["secs"]))
        self.assertEqual(("SKIP", "all cached"),
                         (ended["build"]["verdict"], ended["build"].get("reason")))
        self.assertEqual(2, sum(1 for e in evs if e["event"] == "cache_hit"))
        self.assertNotIn("tests", self.order(evs, "station_started"))

    def test_executor_nextest_stations_have_distinct_junit_profiles(self):
        self.fake_tier()
        # A browser station without node or playwright is SKIP (browser_short):
        # give the throw-away tree both, so the browser station really runs.
        self.write_exe(self.bin / "node", "#!/bin/sh\necho v22.0.0\n")
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(parents=True)
        browser =station("browser:display", None, nextest={"profile": "browser", "filter": ""},
                          parts=[], cmds=[
                              {"argv": ["scripts/test-tier.sh", "filter", "binary(/710_the_sheet_holds/)"],
                               "cwd": ""},
                              {"argv": ["scripts/test-tier.sh", "filter", "binary(/710_the_colony_holds/)",
                                        "--run-ignored", "all"], "cwd": ""}])
        rc, evs = self.run_exec(self.dispatch([self._tests_station(["a::x"]), browser]))
        self.assertEqual(0, rc, self.err_text)
        by_profile = {}
        for profile, _reuse, args in self.tier_lines():
            by_profile.setdefault(profile, []).append(args)
        self.assertEqual({"default", "browser", "browser-ignored"}, set(by_profile))
        self.assertIn("--run-ignored", by_profile["browser-ignored"][0])
        # The profiles exist, inherit the default's settings and write their
        # own JUnit file -- parallel stations never overwrite each other's.
        cfg = tomllib.loads((REPO / ".config" / "nextest.toml").read_text())
        for name in ("browser", "browser-ignored", "isolate"):
            self.assertIn(name, cfg["profile"], name)
            self.assertEqual("junit.xml", cfg["profile"][name]["junit"]["path"])
            self.assertEqual("default", cfg["profile"][name].get("inherits", "default"))

    def test_executor_nextest_stations_reuse_build_without_cargo(self):
        # The REAL test-tier.sh in a throw-away tree, fake cargo on PATH.
        scripts = self.tree / "scripts"
        scripts.mkdir(parents=True, exist_ok=True)
        for f in ("test-tier.sh", "gate_plan.py", "slow_tests.py"):
            shutil.copy(REPO / "scripts" / f, scripts / f)
        # gate_plan.py reads Rust through `pipeline.crate_root` (GH #1162)
        (scripts / "pipeline").mkdir(exist_ok=True)
        for f in ("__init__.py", "crate_root.py"):
            shutil.copy(REPO / "scripts" / "pipeline" / f, scripts / "pipeline" / f)
        (scripts / "test-tier.sh").chmod(0o755)
        kit_tree.seed(scripts, "test-tier.sh")
        (self.tree / ".config").mkdir()
        shutil.copy(REPO / ".config" / "nextest.toml", self.tree / ".config" / "nextest.toml")
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1},
                        build={"meclaw_bin": True, "workspace": False, "packages": [],
                               "tests": [["c1", "t1"]], "lib_bins": [], "extra": []})
        tests = self._tests_station(["c1::t1"], "binary_id(=c1::t1)")
        tests["after"] = ["build"]
        rc, evs = self.run_exec(self.dispatch([build, tests]))
        self.assertEqual(0, rc, self.err_text + "\n" + "\n".join(self.call_lines()))
        lines = self.call_lines()
        run_idx = [i for i, ln in enumerate(lines) if ln.startswith("cargo-nextest nextest run")]
        self.assertEqual(1, len(run_idx), lines)
        nextest_run = lines[run_idx[0]]
        reuse = self.ws / "reuse"
        self.assertIn("--binaries-metadata %s" % (reuse / "binaries.json"), nextest_run)
        self.assertIn("--cargo-metadata %s" % (reuse / "cargo-metadata.json"), nextest_run)
        self.assertIn("--profile default", nextest_run)
        # Not one cargo call after the build: the nextest station reused it.
        last_cargo = max(i for i, ln in enumerate(lines) if ln.startswith("cargo "))
        self.assertLess(last_cargo, run_idx[0])
        self.assertTrue((reuse / "binaries.json").exists())
        self.assertIn("c1::t1", json.loads((reuse / "binaries.json").read_text())["rust-binaries"])
        self.assertTrue((reuse / "cargo-metadata.json").exists())
        res = [e["data"] for e in evs if e["event"] == "test_result"]
        self.assertEqual([("c1::t1", "GREEN")], [(r["part"], r["verdict"]) for r in res])

    def test_tier_reuse_calls_nextest_without_cargo(self):
        # scripts/test-tier.sh on its own: MECLAW_TIER_REUSE runs cargo-nextest
        # straight against the build's metadata -- no cargo, no cargo lock, no
        # token check, no tree sync -- and t0 drops its cargo-only build flags.
        scripts = self.tree / "scripts"
        scripts.mkdir(parents=True, exist_ok=True)
        for f in ("test-tier.sh", "gate_plan.py", "slow_tests.py"):
            shutil.copy(REPO / "scripts" / f, scripts / f)
        # gate_plan.py reads Rust through `pipeline.crate_root` (GH #1162)
        (scripts / "pipeline").mkdir(exist_ok=True)
        for f in ("__init__.py", "crate_root.py"):
            shutil.copy(REPO / "scripts" / "pipeline" / f, scripts / "pipeline" / f)
        (scripts / "test-tier.sh").chmod(0o755)
        kit_tree.seed(scripts, "test-tier.sh")
        token = self.root / "token-calls.txt"
        self.write_exe(scripts / "strand.sh", '#!/bin/sh\necho "$*" >> %s\n' % token)
        (self.tree / ".config").mkdir()
        shutil.copy(REPO / ".config" / "nextest.toml", self.tree / ".config" / "nextest.toml")
        self.fake_cargo()
        reuse = self.root / "reuse"
        reuse.mkdir()
        lock = self.root / "tier.lock"
        env = {k: v for k, v in os.environ.items()
               if k not in ("CI", "MECLAW_CARGO_LOCK_HELD", "MECLAW_TIER_DRY", "MECLAW_TIER_LANE")}
        env.update({"PATH": "%s:%s" % (self.bin, os.environ.get("PATH", "")),
                    "CALLS": str(self.calls), "CARGO_TARGET_DIR": str(self.target),
                    "MECLAW_GATE_LOCK": str(lock), "MECLAW_TIER_REUSE": str(reuse),
                    "MECLAW_TIER_PROFILE": "browser"})
        for args in (["filter", "binary_id(=c1::t1)"], ["t0"]):
            p = subprocess.run([str(scripts / "test-tier.sh"), *args], cwd=self.tree, env=env,
                               capture_output=True, text=True, timeout=2 * kit_time.HANG)
            self.assertEqual(0, p.returncode, p.stdout + p.stderr)
        meta = ("--profile browser --binaries-metadata %s --cargo-metadata %s"
                % (reuse / "binaries.json", reuse / "cargo-metadata.json"))
        self.assertEqual(["cargo-nextest nextest run %s -E binary_id(=c1::t1)" % meta,
                          "cargo-nextest nextest run %s -E kind(lib) + kind(bin)" % meta],
                         self.call_lines())
        self.assertFalse(token.exists(), "the token is not asked in reuse mode")
        self.assertFalse(lock.exists(), "the cargo lock is not taken in reuse mode")

    def _hook_crates(self):
        # meclaw-colony owns the `test-hooks` feature, c1 depends on it, c2 does not
        for name, body in (("meclaw-colony", ""),
                           ("c1", '[dependencies]\nmeclaw-colony = { path = "../meclaw-colony" }\n'),
                           ("c2", "[dependencies]\n")):
            d = self.tree / "crates" / name
            d.mkdir(parents=True, exist_ok=True)
            (d / "Cargo.toml").write_text('[package]\nname = "%s"\n\n%s' % (name, body))
        scripts = self.tree / "scripts"
        scripts.mkdir(parents=True, exist_ok=True)
        kit_tree.library(scripts, "cargo-target.sh")

    def test_build_station_builds_the_test_hooks(self):
        # GH #1208 (review H8, R-TV-W10): `#![cfg(feature = "test-hooks")]`
        # tests (phase_6_crash_recovery, ...) were empty stubs in every gate --
        # the build station built the tests without the feature, so the folded
        # test list never held them. Every build of the station carries it,
        # `build --bin meclaw` included (re-check M1, section 7): one variant of
        # meclaw-colony in the shared target/, one `target/debug/meclaw`. The
        # product binary is the release build, which never does.
        self._hook_crates()
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1},
                        build={"meclaw_bin": True, "workspace": False, "packages": ["c2"],
                               "tests": [["c1", "t1"]], "lib_bins": [], "extra": []})
        rc, _evs = self.run_exec(self.dispatch([build]))
        self.assertEqual(0, rc, self.err_text + "\n" + "\n".join(self.call_lines()))
        lines = [ln for ln in self.call_lines() if ln.startswith("cargo ")]
        feat = "--features meclaw-colony/test-hooks"
        self.assertIn("cargo build --bin meclaw %s" % feat, lines)
        c1 = [ln for ln in lines if ln.startswith("cargo nextest") and "-p c1" in ln]
        self.assertEqual(2, len(c1), lines)   # the build and its `nextest list`
        for ln in c1:
            self.assertIn(feat, ln)
        # a selection without a crate that knows the feature: cargo refuses the flag there
        c2 = [ln for ln in lines if ln.startswith("cargo nextest") and "-p c2" in ln]
        self.assertEqual(2, len(c2), lines)
        for ln in c2:
            self.assertNotIn(feat, ln)

    def test_tier_builds_the_test_hooks(self):
        # GH #1208: scripts/test-tier.sh (single tests, CI shards) builds with
        # the same feature as the gate's build station -- the same rule.
        self._hook_crates()
        scripts = self.tree / "scripts"
        for f in ("test-tier.sh", "gate_plan.py", "slow_tests.py"):
            shutil.copy(REPO / "scripts" / f, scripts / f)
        (scripts / "pipeline").mkdir(exist_ok=True)
        for f in ("__init__.py", "crate_root.py"):
            shutil.copy(REPO / "scripts" / "pipeline" / f, scripts / "pipeline" / f)
        (scripts / "test-tier.sh").chmod(0o755)
        kit_tree.seed(scripts, "test-tier.sh")
        (self.tree / ".config").mkdir()
        shutil.copy(REPO / ".config" / "nextest.toml", self.tree / ".config" / "nextest.toml")
        self.fake_cargo()
        # a nextest run that reports its one test (the tier judges what ran, GH #997)
        self.write_exe(self.bin / "cargo", "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> \"$CALLS\"\n"
                       "echo '     Summary [   0.010s] 1 test run: 1 passed, 0 skipped'\n")
        env = {k: v for k, v in os.environ.items()
               if k not in ("MECLAW_TIER_REUSE", "MECLAW_TIER_DRY", "MECLAW_TIER_LANE")}
        env.update({"PATH": "%s:%s" % (self.bin, os.environ.get("PATH", "")), "CI": "1",
                    "CALLS": str(self.calls), "CARGO_TARGET_DIR": str(self.target)})
        for args in (["filter", "binary_id(=c1::t1)"], ["filter", "binary_id(=c1::t1)", "-p", "c1"],
                     ["filter", "binary_id(=c2::t3)", "-p", "c2"]):
            p = subprocess.run([str(scripts / "test-tier.sh"), *args], cwd=self.tree, env=env,
                               capture_output=True, text=True, timeout=2 * kit_time.HANG)
            self.assertEqual(0, p.returncode, p.stdout + p.stderr)
        feat = "--features meclaw-colony/test-hooks"
        self.assertEqual(
            ["cargo nextest run --workspace %s --profile default -E binary_id(=c1::t1)" % feat,
             "cargo nextest run %s --profile default -E binary_id(=c1::t1) -p c1" % feat,
             "cargo nextest run --profile default -E binary_id(=c2::t3) -p c2"],
            self.call_lines())

    def test_coverage_host_builds_the_test_hooks(self):
        # GH #1208 (re-check M1): the coverage run built its tests without the
        # feature, so the `#![cfg(feature = "test-hooks")]` tests were empty
        # stubs in the coverage map and the hook code counted as never measured.
        # All four cargo calls take the flags from the same rule as the gate.
        self._hook_crates()
        pipe = self.tree / "scripts" / "pipeline"
        pipe.mkdir(exist_ok=True)
        shutil.copy(REPO / "scripts" / "pipeline" / "coverage_host.sh", pipe / "coverage_host.sh")
        (pipe / "coverage_host.sh").chmod(0o755)
        kit_tree.seed(pipe.parent, "pipeline/coverage_host.sh")
        (pipe / "browser_cover.py").write_text(
            "import sys\nprint({'select': '-p c1 --test l1', 'filter': 'binary_id(=c1::l1)'}"
            "[sys.argv[1]])\n")
        (self.tree / "scripts" / "pipeline.py").write_text("")
        sysroot = self.root / "sysroot"
        self.write_exe(sysroot / "lib" / "rustlib" / "x86_64-unknown-linux-gnu" / "bin"
                       / "llvm-profdata", "#!/bin/sh\n")
        self.write_exe(self.bin / "rustc", "#!/bin/sh\necho %s\n" % sysroot)
        self.write_exe(self.bin / "cargo", "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> \"$CALLS\"\n")
        env = dict(os.environ, PATH="%s:%s" % (self.bin, os.environ.get("PATH", "")),
                   CALLS=str(self.calls))
        for scope in ("full", "browser"):
            p = subprocess.run(["bash", str(pipe / "coverage_host.sh"), str(self.root / "cov" / "target"),
                                "600", REV, scope], cwd=self.tree, env=env,
                               capture_output=True, text=True, timeout=2 * kit_time.HANG)
            self.assertEqual(0, p.returncode, p.stdout + p.stderr)
        feat = "--features meclaw-colony/test-hooks"
        cargo = [ln for ln in self.call_lines() if ln.startswith("cargo nextest")]
        self.assertEqual(5, len(cargo), cargo)
        for ln in cargo:
            self.assertIn(feat, ln)
        self.assertIn("cargo nextest run --workspace %s --no-fail-fast" % feat, cargo[0])
        self.assertIn("cargo nextest list --workspace %s --message-format json" % feat, cargo)
        self.assertIn("cargo nextest list -p c1 --test l1 %s --message-format json" % feat, cargo)

    def test_ci_and_release_keep_the_hooks_where_they_belong(self):
        # GH #1208: the CI test jobs build through test-tier.sh (above) and the
        # doctests with the feature; the release binary never carries it.
        ci = (REPO / ".github" / "workflows" / "ci.yml").read_text()
        for want in ("scripts/test-tier.sh t2",
                     "cargo test --workspace --doc --features meclaw-colony/test-hooks"):
            self.assertTrue(want in ci, "ci.yml lacks: " + want)
        release = (REPO / ".github" / "workflows" / "release.yml").read_text()
        self.assertTrue("cargo build --release" in release, "release.yml builds no release")
        self.assertFalse("test-hooks" in release, "the release build carries the test hooks")


class TestExecutorBuildAndExport(ExecutorCase):
    def test_build_targets_follow_plan_not_filterset(self):
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1}, build={
            "meclaw_bin": True, "workspace": False, "packages": ["c4"],
            "tests": [["c1", "t1"], ["c1", "t2"], ["c2", "t3"]], "lib_bins": ["c3"],
            "extra": [["c1", "710_x"]]})
        tests = station("tests", "true", after=["build"], parts=["c1::t1", "c1::t2", "c2::t3"],
                        nextest={"profile": "default", "filter": "rdeps(c1)"},
                        inputs=["artefact:meclaw"])
        d = self.dispatch([build, tests], cache_known=[
            {"station": "tests", "part": "c1::t2", "key": "4" * 64, "from_run": "run-333333333333"}])
        d["stations"][1]["cmds"] = []
        rc, evs = self.run_exec(d)
        self.assertEqual("GREEN", self.ended(evs)["build"]["verdict"], self.err_text)
        builds = [ln for ln in self.call_lines()
                  if ln.startswith("cargo build") or "--no-run" in ln]
        self.assertIn("cargo build --bin meclaw --features meclaw-colony/test-hooks", builds)
        self.assertIn("cargo nextest run --no-run -p c1 --test t1 --test 710_x", builds)
        self.assertIn("cargo nextest run --no-run -p c2 --test t3", builds)
        self.assertIn("cargo nextest run --no-run -p c4", builds)
        self.assertIn("cargo nextest run --no-run -p c3 --lib --bins", builds)
        joined = "\n".join(self.call_lines())
        self.assertNotIn("t2", joined)           # cached: not built
        self.assertNotIn(" -E ", joined)         # the filterset never selects a build
        lists = [ln for ln in self.call_lines() if ln.startswith("cargo nextest list")]
        self.assertTrue(lists and all("--list-type binaries-only --message-format json" in ln
                                      for ln in lists), lists)
        self.assertIn("cargo metadata --format-version 1", self.call_lines())
        # The pure half: workspace wins, and the t0 floor builds lib and bins only.
        self.assertEqual([["nextest", "run", "--no-run", "--workspace"]],
                         executor.build_argvs({"workspace": True, "tests": [["c1", "t1"]]}, set()))
        self.assertEqual([["nextest", "run", "--no-run", "-p", "c3", "--lib", "--bins"]],
                         executor.build_argvs({"lib_bins": ["c3"]}, set()))

    def test_an_unrequested_binary_in_the_list_is_a_note(self):
        # Re-check m-1 (#1166): one call builds `-p a -p b --test x`, and cargo
        # applies every `--test` to every `-p`. The merge rule keeps the binary
        # set equal on the plan's metadata; this guard checks it on cargo's own
        # answer: a binary id `nextest list` names that the plan did not ask for
        # is a NOTE row naming it, never silence.
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1}, build={
            "meclaw_bin": False, "workspace": False, "packages": [],
            "tests": [["c1", "t1"]], "lib_bins": [], "extra": []})
        d = self.dispatch([build])
        d["+env"]["FAKE_LIST_EXTRA"] = "c2::t1"
        rc, evs = self.run_exec(d)
        ended = self.ended(evs)
        self.assertEqual("GREEN", ended["build"]["verdict"], self.err_text)
        note = ended.get("build-unrequested")
        self.assertIsNotNone(note, sorted(ended))
        self.assertEqual("NOTE", note["verdict"])
        self.assertIn("c2::t1", note.get("reason", ""))
        # the requested set alone: no row
        d = self.dispatch([build])
        rc, evs = self.run_exec(d)
        self.assertNotIn("build-unrequested", self.ended(evs))

    def test_a_package_with_only_its_lib_cached_is_still_built(self):
        # Review B risk: `-p <crate>` used to vanish once its lib part was cached,
        # so its uncached test binaries were never built (missing in binaries.json).
        self.fake_cargo()
        build = station("build", None, needs={"cargo": 1}, build={
            "meclaw_bin": False, "workspace": False, "packages": ["c4", "c5"], "tests": [],
            "lib_bins": [], "extra": []})
        tests = station("tests", "true", after=["build"], parts=["c4", "c4::it", "c5", "c5::it"],
                        nextest={"profile": "default", "filter": "rdeps(c4)"})
        d = self.dispatch([build, tests], cache_known=[
            {"station": "tests", "part": part, "key": "4" * 64, "from_run": "run-333333333333"}
            for part in ("c4", "c5", "c5::it")])
        d["stations"][1]["cmds"] = []
        rc, evs = self.run_exec(d)
        self.assertEqual("GREEN", self.ended(evs)["build"]["verdict"], self.err_text)
        builds = [ln for ln in self.call_lines() if "--no-run" in ln]
        self.assertIn("cargo nextest run --no-run -p c4", builds)
        self.assertNotIn("c5", "\n".join(builds))   # every part of c5 cached: not built
        # The pure half: a lib_bins crate stays while one of its parts is uncached.
        self.assertEqual([["nextest", "run", "--no-run", "-p", "c3", "--lib", "--bins"]],
                         executor.build_argvs({"lib_bins": ["c3"]}, {"c3"},
                                              all_parts={"c3", "c3::bin/x"}))
        self.assertEqual([], executor.build_argvs({"lib_bins": ["c3"]}, {"c3"}, all_parts={"c3"}))

    def test_executor_export_target_is_persistent_under_srv(self):
        out_t = self.root / "tree-tests.env"
        out_a = self.root / "audit.env"
        sts = [station("export-tree-tests",
                       'echo "$MECLAW_EXPORT_WORKDIR" > %s' % out_t, needs={"export": 1}),
               station("export-audit",
                       'echo "$MECLAW_EXPORT_CHECK_TARGET" > %s' % out_a, needs={"export": 1},
                       after=["export-tree-tests"], when="any")]
        rc, _evs = self.run_exec(self.dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        export_root = self.root / "export"
        self.assertEqual(str(export_root), out_t.read_text().strip())
        self.assertEqual(str(export_root / "check"), out_a.read_text().strip())
        self.assertTrue((export_root / "check").is_dir())

    def test_executor_pass_ended_before_scope_stop(self):
        seen = {}

        def stopper(run):
            seen["run"] = run
            seen["out"] = buf.getvalue()

        buf = io.StringIO()
        d = self.dispatch([station("long", "sleep 30", timeout_min=5),
                           station("later", "true", after=["long"])],
                          deadline_at=_iso(1))
        rc = executor.run(d, buf, io.StringIO(), scope_stopper=stopper)
        self.assertEqual(2, rc)
        self.assertEqual(RUN, seen.get("run"))
        lines = [json.loads(ln) for ln in seen["out"].splitlines()]
        self.assertEqual("pass_ended", lines[-1]["event"])
        self.assertEqual("ABORTED", lines[-1]["data"]["verdict"])
        fired = [e["data"] for e in lines if e["event"] == "watchdog_fired"]
        self.assertEqual([("deadline", "aborted")], [(f["kind"], f["action"]) for f in fired])
        ended = self.ended(lines)
        self.assertEqual("ABORTED", ended["long"]["verdict"])
        # never started after the abort: ABORTED, never SKIP (the store books SKIP as done)
        self.assertEqual(("ABORTED", "aborted_deadline"),
                         (ended["later"]["verdict"], ended["later"].get("reason")))


class TestBuildArgvsJoint(unittest.TestCase):
    """S6 (#1166): the build station merges its per-crate `--test` calls into as
    few cargo calls as build exactly the same binaries -- cargo applies every
    `--test` to every `-p`, and every separate `-p` set is its own feature
    variant of the shared crates (S6-DIAG: 3 calls 164 s, merged 56 s)."""

    NO_RUN = ["nextest", "run", "--no-run"]

    def test_crates_without_a_name_clash_share_one_call(self):
        build = {"tests": [["c1", "t1"], ["c1", "t2"], ["c2", "t3"]],
                 "test_targets": {"c1": ["t1", "t2", "t9"], "c2": ["t3", "t4"]}}
        self.assertEqual([self.NO_RUN + ["-p", "c1", "-p", "c2",
                                         "--test", "t1", "--test", "t2", "--test", "t3"]],
                         executor.build_argvs(build, set()))

    def test_a_name_another_crate_also_has_keeps_the_calls_apart(self):
        # c2 has a `misc` nobody asked for: one call would build it too.
        build = {"tests": [["c1", "misc"], ["c2", "t3"], ["c3", "t5"]],
                 "test_targets": {"c1": ["misc"], "c2": ["misc", "t3"], "c3": ["t5"]}}
        self.assertEqual([self.NO_RUN + ["-p", "c1", "-p", "c3", "--test", "misc", "--test", "t5"],
                          self.NO_RUN + ["-p", "c2", "--test", "t3"]],
                         executor.build_argvs(build, set()))

    def test_a_name_two_members_ask_for_is_passed_once(self):
        # Re-check m-2 (#1166): c1 and c2 both ask for their `misc`; they share a
        # call, and `--test misc` names both targets once (S4's `tests_of` rule).
        build = {"tests": [["c1", "misc"], ["c2", "misc"], ["c2", "t3"]],
                 "test_targets": {"c1": ["misc"], "c2": ["misc", "t3"]}}
        self.assertEqual([self.NO_RUN + ["-p", "c1", "-p", "c2",
                                         "--test", "misc", "--test", "t3"]],
                         executor.build_argvs(build, set()))

    def test_without_the_targets_of_a_crate_nothing_is_merged(self):
        # fail-closed: a crate whose test targets the plan does not know builds alone
        build = {"tests": [["c1", "t1"], ["c2", "t3"]], "test_targets": {"c1": ["t1"]}}
        self.assertEqual([self.NO_RUN + ["-p", "c1", "--test", "t1"],
                          self.NO_RUN + ["-p", "c2", "--test", "t3"]],
                         executor.build_argvs(build, set()))

    def test_the_plan_carries_the_test_targets_of_the_named_crates(self):
        from scripts.pipeline import plan
        md = {"workspace_members": ["c1 0.1", "c2 0.1"], "packages": [
            {"id": "c1 0.1", "name": "c1", "dependencies": [], "targets": [
                {"name": "c1", "kind": ["lib"]}, {"name": "t1", "kind": ["test"]},
                {"name": "misc", "kind": ["test"]}]},
            {"id": "c2 0.1", "name": "c2", "dependencies": [], "targets": [
                {"name": "misc", "kind": ["test"]}]}]}
        consumers = [{"name": "tests"}]
        st = plan._build_station(consumers, [{"tests": [["c1", "t1"], ["c2", "misc"]]}],
                                 "strand", None, metadata=md)
        self.assertEqual({"c1": ["misc", "t1"], "c2": ["misc"]}, st["build"]["test_targets"])
        self.assertEqual([["cargo"] + self.NO_RUN + ["-p", "c1", "--test", "t1"],
                          ["cargo"] + self.NO_RUN + ["-p", "c2", "--test", "misc"]],
                         [c["argv"] for c in st["cmds"]])


class TestReceipt(unittest.TestCase):
    HEAD = {"mode": "strand", "rev": REV, "base": BASE, "dirty": False, "archive": None,
            "repo": "meclaw", "lane": "lane-x", "run": RUN, "plan": ["a", "b", "c", "d"],
            "started": "2026-10-08T10:00:00Z", "owner_decisions": [], "plan_reason": {},
            "fingerprint": {}}

    def head_rows(self):
        """The core's `pass_planned` and `run_dispatched` (W0a projects the head from them)."""
        h = self.HEAD
        planned = {"run": RUN, "mode": h["mode"], "repo": h["repo"], "rev": h["rev"],
                   "base": h["base"], "stations": h["plan"], "reason": h["plan_reason"],
                   "cache_known": 1, "dirty": h["dirty"], "archive": h["archive"]}
        dispatched = {"run": RUN, "runner": h["lane"], "tree": "/t", "attempt": 1, "round": 1,
                      "deadline_at": None, "ws": "/w"}
        return [self.mk("pass_planned", planned, RUN, at=h["started"]),
                self.mk("run_dispatched", dispatched, RUN, at=h["started"])]

    @staticmethod
    def mk(event, data, run, at):
        kind = events.EVENTS[event].kinds[0]
        events.validate(event, kind, data)
        return {"event": event, "kind": kind, "at": at, "run": run, "data": data}

    def events(self):
        mk = self.mk
        return self.head_rows() + [
            mk("station_ended", {"run": RUN, "station": "a", "verdict": "GREEN", "secs": 3,
                                 "exit": 0, "scope": "sa"}, RUN, at="2026-10-08T10:00:03Z"),
            mk("cache_hit", {"run": RUN, "station": "b", "part": "", "key": "k" * 64,
                             "from_run": "run-bbbbbbbbbbbb"}, RUN, at="2026-10-08T10:00:03Z"),
            mk("station_ended", {"run": RUN, "station": "b", "verdict": "CACHED", "secs": 0,
                                 "exit": None, "scope": "sb"}, RUN, at="2026-10-08T10:00:03Z"),
            mk("station_ended", {"run": RUN, "station": "c", "verdict": "FLAKY", "secs": 7,
                                 "exit": 0, "scope": "sc"}, RUN, at="2026-10-08T10:00:10Z"),
            mk("station_ended", {"run": RUN, "station": "d", "verdict": "NOTE", "secs": 1,
                                 "exit": 0, "scope": "sd", "reason": "clean"}, RUN,
               at="2026-10-08T10:00:11Z"),
            # FLAKY `c` is a finding (VG TV-F1): the pass ends RED.
            mk("pass_ended", {"run": RUN, "verdict": "RED", "secs": 12, "lane_seconds": 11,
                              "critical_path_s": 10, "rounds": 1, "cache_hits": 1,
                              "stations": {"green": 1, "red": 0, "cached": 1, "flaky": 1,
                                           "skip": 0, "note": 1, "ask": 0, "aborted": 0}},
               RUN, at="2026-10-08T10:00:12Z"),
        ]

    def test_legacy_receipt_maps_cached_to_green_and_flaky_to_red(self):
        v2 = receipt.project(self.events())
        self.assertEqual("meclaw.gate.receipt/2", v2["schema"])
        self.assertEqual(["CACHED", "FLAKY"], [s["verdict"] for s in v2["stations"][1:3]])
        old = executor.legacy_v1(v2)
        self.assertEqual({"mode", "rev", "base", "dirty", "lock_wait_secs", "archive", "plan",
                          "started", "finished", "stations", "verdict", "owner_decisions"},
                         set(old))
        rows = {r["name"]: r for r in old["stations"]}
        # W0a's legacy rows keep every v2 station field; the v1 ones read as gate.sh wrote.
        v1 = ("name", "scope", "secs", "verdict", "log", "cache_from", "flaky")
        self.assertEqual({"name": "b", "scope": "sb", "secs": 0, "verdict": "GREEN",
                          "log": None, "cache_from": "run-bbbbbbbbbbbb", "flaky": None},
                         {k: rows["b"].get(k) for k in v1})
        self.assertEqual({"name": "c", "scope": "sc", "secs": 7, "verdict": "RED",
                          "log": None, "cache_from": None, "flaky": True},
                         {k: rows["c"].get(k) for k in v1})
        self.assertEqual({"name": "a", "scope": "sa", "secs": 3, "verdict": "GREEN",
                          "log": None, "cache_from": None, "flaky": None},
                         {k: rows["a"].get(k) for k in v1})
        self.assertEqual("RED", old["verdict"])
        # The v2 document is left alone.
        self.assertEqual("CACHED", v2["stations"][1]["verdict"])

    def test_summary_line_byte_compatible(self):
        """`summary_lines` prints what scripts/gate.sh prints -- byte for byte.

        The two printf formats are read out of the runner itself (the legacy
        copy once W3 turns gate.sh into a hull) and rendered by bash, so a
        changed format on either side turns this red."""
        src = REPO / "scripts" / "gate_legacy.sh"
        if not src.exists():
            src = REPO / "scripts" / "gate.sh"
        text = src.read_text()
        gate_fmt = "'GATE %s [%s] %ss %s%s\\n'"
        summary_fmt = "'GATE-SUMMARY %s %s %s/%s %ss %s\\n'"
        self.assertIn(gate_fmt, text)
        self.assertIn(summary_fmt, text)
        v2 = receipt.project(self.events())
        # A RED, a SKIP, an ASK and a browser SKIP that counts in the total
        # (the executor marks it `in_total`, W0a's `gate_lines` counts that).
        v2["stations"] += [
            {"name": "e", "scope": "se", "secs": 2, "verdict": "RED", "log": ""},
            {"name": "f", "scope": "sf", "secs": 0, "verdict": "SKIP", "log": "",
             "reason": "cargo-deny not installed"},
            {"name": "g", "scope": "sg", "secs": 4, "verdict": "ASK", "log": "",
             "reason": "no persona report"},
            {"name": "browser:display", "scope": "sheet", "secs": 0, "verdict": "SKIP",
             "log": "", "reason": "no-node: the browser proofs did not run (counted)",
             "in_total": True},
        ]
        v2["verdict"] = "RED"

        def bash_printf(fmt, *args):
            return subprocess.run(["bash", "-c", "printf %s \"$@\"" % fmt, "x", *args],
                                  capture_output=True, text=True, check=True).stdout

        want = ""
        for s in v2["stations"]:
            reason = s.get("reason") or ""
            want += bash_printf(gate_fmt, s["name"], s["scope"], str(s["secs"]), s["verdict"],
                                (" " + reason) if reason else "")
        # judged = GREEN + RED; CACHED counts green, FLAKY red (VG TV-F1); the counted
        # browser SKIP is in the total only: green 2 (a, b), total 5 (+ c, + e, + browser).
        want += bash_printf(summary_fmt, "strand", REV[:7], "2", "5", "12", "RED")
        got = "".join(line + "\n" for line in receipt.gate_lines(v2))
        self.assertEqual(want, got)
        # ABORTED is the sum, whatever the stations say.
        v2["verdict"] = "ABORTED"
        self.assertTrue(receipt.gate_lines(v2)[-1].endswith(" ABORTED"))


class TestExportCaches(unittest.TestCase):
    """W3 item 6: the export stations keep tree and target on the lane.

    `MECLAW_EXPORT_WORKDIR` (set by the executor for `export-tree-tests`) and
    `MECLAW_EXPORT_CHECK_TARGET` (for `export-audit`) are additive: without
    them both scripts build in fresh temporary directories and delete them,
    exactly as before. git, cargo and the scan are stubbed; nothing builds."""

    def setUp(self):
        import importlib
        import sys
        from unittest import mock
        # GH #1199: both scripts live under plans/, which never travels
        private_tree.require("plans/export-fixtures/export_tree_tests.py",
                             "plans/export-fixtures/make_export.py")
        for p in (REPO / "scripts", REPO / "plans" / "export-fixtures"):
            if str(p) not in sys.path:
                sys.path.insert(0, str(p))
        self.ett = importlib.import_module("export_tree_tests")
        self.me = importlib.import_module("make_export")
        self.mock = mock
        self._tmp = tempfile.TemporaryDirectory(prefix="pipe-export-")
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)

    def _run_tree_tests(self, env):
        seen = {"extract": [], "judge": []}

        def extract(tree, dest):
            seen["extract"].append((tree, pathlib.Path(dest)))
            (pathlib.Path(dest) / "Cargo.toml").write_text("[workspace]\n")

        def judge(tree, selection, runner=None, env=None):
            seen["judge"].append((pathlib.Path(tree), env["CARGO_TARGET_DIR"],
                                  pathlib.Path(tree).is_dir()))
            seen["stray_before"] = (pathlib.Path(tree) / "stray.txt").exists()
            # A test that writes into the tree, and a build that fills the target.
            (pathlib.Path(tree) / "stray.txt").write_text("state\n")
            (pathlib.Path(env["CARGO_TARGET_DIR"]) / "built").write_text("1\n")
            return []
        m = self.mock
        with m.patch.dict(os.environ, env), \
                m.patch.object(self.ett, "export_index_tree", return_value=("a" * 40, "c" * 40)), \
                m.patch.object(self.ett, "extract_tree", side_effect=extract), \
                m.patch.object(self.ett, "select", return_value={"c1": ["t1"]}), \
                m.patch.object(self.ett, "judge", side_effect=judge), \
                m.patch.object(self.ett.shutil, "which", return_value="/bin/cargo"):
            rc = self.ett.main([])
        return rc, seen

    # R5 of welle-validierung-2 (GH #1199): ONE target per lane across exports.
    # A stub export is a dict {path: content}; its git tree id is "<letter>" * 40.
    # The stub build stamps `target/built` with the newest tree file's mtime (a
    # build runs after the sync; cargo: stale only when a source is NEWER than
    # its dep-info); `seen["built"]` is that mtime before the run.

    def _sync(self, work, tree, files, changed=None, fail_extract=False):
        """One run of the station on the stub export `files`; `changed` is what
        `changed_paths` answers (default: the diff to the previous stub)."""
        seen = {"before": {}}
        prev = getattr(self, "_prev_files", None)

        def extract(_tree, dest):
            if fail_extract:
                raise RuntimeError("archive broke")
            for rel, text in files.items():
                p = pathlib.Path(dest) / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)

        def changed_paths(old, new, repo=None):
            if changed is not None:
                return changed(old, new)
            if prev is None:
                return None
            return {k for k in set(prev) | set(files) if prev.get(k) != files.get(k)}

        def judge(tree, selection, runner=None, env=None):
            target = pathlib.Path(env["CARGO_TARGET_DIR"])
            seen["target"] = target
            seen["built_before"] = (target / "built").is_file()
            seen["built"] = (target / "built").stat().st_mtime_ns if seen["built_before"] else None
            seen["stray_before"] = (pathlib.Path(tree) / "stray.txt").exists()
            for rel in files:
                seen["before"][rel] = (pathlib.Path(tree) / rel).stat().st_mtime_ns
            (pathlib.Path(tree) / "stray.txt").write_text("state\n")
            for hook in getattr(self, "_after_tests", ()):
                hook(pathlib.Path(tree))
            if seen.get("build", True):
                built = target / "built"
                built.write_text(tree.name + "\n")
                ns = max(seen["before"].values())
                os.utime(built, ns=(ns, ns))
            return []
        seen["build"] = getattr(self, "_build", True)
        m = self.mock
        with m.patch.dict(os.environ, {"MECLAW_EXPORT_WORKDIR": str(work)}), \
                m.patch.object(self.ett, "export_index_tree", return_value=("a" * 40, tree)), \
                m.patch.object(self.ett, "extract_tree", side_effect=extract), \
                m.patch.object(self.ett, "changed_paths", side_effect=changed_paths), \
                m.patch.object(self.ett, "select", return_value={"c1": ["t1"]}), \
                m.patch.object(self.ett, "judge", side_effect=judge), \
                m.patch.object(self.ett.shutil, "which", return_value="/bin/cargo"):
            rc = self.ett.main([])
        if not fail_extract:
            self._prev_files = files
        return rc, seen

    def test_export_tree_tests_workdir_is_one_lane_target_across_exports(self):
        work = self.root / "export"
        rc1, first = self._sync(work, "c" * 40, {"Cargo.toml": "[workspace]\n", "src/a.rs": "1"})
        rc2, second = self._sync(work, "d" * 40, {"Cargo.toml": "[workspace]\n", "src/a.rs": "2"})
        self.assertEqual((0, 0), (rc1, rc2))
        base = work / self.ett.LANE_DIR
        self.assertEqual(base / "target", first["target"])
        self.assertEqual(base / "target", second["target"], "a new export keeps the lane target")
        self.assertTrue(second["built_before"], "the second export starts from the first build")
        # B-M6: the tree is unpacked fresh on every run -- nothing a test wrote into it
        # reaches the next run.
        self.assertFalse(second["stray_before"], "a fresh tree per run")
        self.assertEqual("d" * 40, (base / "target" / self.ett.STAMP).read_text().split()[0])
        self.assertEqual([], [p.name for p in work.iterdir() if p.name != self.ett.LANE_DIR],
                         "no directory per export any more")

    def test_export_tree_tests_changed_file_is_newer_than_its_build(self):
        """Pin 1: a file the export changed must look newer than the lane's build
        (cargo: source mtime > dep-info mtime => rebuild). Mutant without the touch:
        the changed file keeps its old mtime, cargo keeps the stale build -- red."""
        work = self.root / "export"
        self._sync(work, "c" * 40, {"Cargo.toml": "[workspace]\n", "src/a.rs": "1"})
        _rc, second = self._sync(work, "d" * 40, {"Cargo.toml": "[workspace]\n", "src/a.rs": "2",
                                                 "src/new.rs": "n"})
        self.assertGreater(second["before"]["src/a.rs"], second["built"], "changed: rebuilt")
        self.assertGreater(second["before"]["src/new.rs"], second["built"], "added: rebuilt")
        self.assertLessEqual(second["before"]["Cargo.toml"], second["built"], "unchanged: kept")

    def test_export_tree_tests_unchanged_export_builds_incrementally(self):
        """Pin 2: what the export did not change keeps an mtime older than the lane's
        build, on the same export and on a new one -- cargo keeps those units."""
        work = self.root / "export"
        files = {"Cargo.toml": "[workspace]\n", "src/a.rs": "1", "src/b.rs": "1"}
        self._sync(work, "c" * 40, files)
        _rc, same = self._sync(work, "c" * 40, dict(files))
        _rc, other = self._sync(work, "d" * 40, dict(files, **{"src/a.rs": "2"}))
        for rel in files:
            self.assertLessEqual(same["before"][rel], same["built"], "same export: %s kept" % rel)
        self.assertLessEqual(other["before"]["src/b.rs"], other["built"], "unchanged file kept")
        self.assertLessEqual(other["before"]["Cargo.toml"], other["built"], "unchanged file kept")

    def test_export_tree_tests_change_without_a_build_stays_newer(self):
        """A change a run did not build (not selected, or the run broke) must still
        look newer on the next export: the kept mtime is the one the last run set,
        not the fixed one."""
        work = self.root / "export"
        self._sync(work, "c" * 40, {"src/a.rs": "1", "src/b.rs": "1"})
        self._build = False
        self._sync(work, "d" * 40, {"src/a.rs": "2", "src/b.rs": "1"})
        self._build = True
        _rc, third = self._sync(work, "e" * 40, {"src/a.rs": "2", "src/b.rs": "2"})
        self.assertGreater(third["before"]["src/a.rs"], third["built"], "moved in run 2, unbuilt")
        self.assertGreater(third["before"]["src/b.rs"], third["built"], "moved in run 3")

    def test_export_tree_tests_test_rewound_mtime_does_not_hide_an_unbuilt_change(self):
        """M3 (re-check v2-r3-export): a test that rewrites a tracked source with an
        older mtime (`copy2`, `utime`) must not hide a change the run did not build.
        The kept mtime is the one the sync SET, carried in the stamp -- not the one
        the tree holds after the tests. Mutant (old behaviour: read the mtime from
        the previous tree): the rewound file looks older than the build -- red."""
        work = self.root / "export"
        self._sync(work, "c" * 40, {"src/a.rs": "1", "src/b.rs": "1"})

        def rewind(tree):
            os.utime(tree / "src" / "a.rs", ns=(1, 1))
        self._build, self._after_tests = False, (rewind,)
        self._sync(work, "d" * 40, {"src/a.rs": "2", "src/b.rs": "1"})
        self._build, self._after_tests = True, ()
        _rc, third = self._sync(work, "e" * 40, {"src/a.rs": "2", "src/b.rs": "2"})
        self.assertGreater(third["before"]["src/a.rs"], third["built"],
                           "moved in run 2, unbuilt, rewound by a test")
        self.assertGreater(third["before"]["src/b.rs"], third["built"], "moved in run 3")

    def _foreign_stamp(self, work, tree, files, build=False):
        """A writer that syncs `files` and rewrites the stamp in the old form
        (tree only) but leaves the mtime record alone -- the pre-fix code of this
        strand on the same lane. `build`: it also builds (target newer than all)."""
        target = work / self.ett.LANE_DIR / "target"
        (target / self.ett.STAMP).write_text(tree + "\n")
        if build:
            ns = (target / "built").stat().st_mtime_ns + 1  # after every mtime the sync set
            (target / "built").write_text("foreign\n")
            os.utime(target / "built", ns=(ns, ns))
        self._prev_files = files

    def test_export_tree_tests_foreign_mtime_record_touches_everything(self):
        """Minor 1 (re-check 8618835b9): the mtime record is bound to the stamp
        (tree sha + stamp id). A record that belongs to another stamp is foreign,
        dropped, and every file gets a fresh mtime. Mutant (old behaviour, record
        unbound): export A's mtimes under the stamp of an unbuilt B pass A's build
        as fresh for a path B changed -- red."""
        work = self.root / "export"
        self._sync(work, "c" * 40, {"src/a.rs": "1", "src/b.rs": "1"})
        self._foreign_stamp(work, "d" * 40, {"src/a.rs": "2", "src/b.rs": "1"})
        _rc, third = self._sync(work, "e" * 40, {"src/a.rs": "2", "src/b.rs": "2"})
        self.assertGreater(third["before"]["src/a.rs"], third["built"],
                           "moved under a foreign stamp, unbuilt")
        self.assertTrue(all(ns > third["built"] for ns in third["before"].values()), third)
        record = json.loads((work / self.ett.LANE_DIR / "target" / self.ett.MTIMES).read_text())
        stamp = (work / self.ett.LANE_DIR / "target" / self.ett.STAMP).read_text().split()
        self.assertEqual(["e" * 40, record["stamp"]], stamp, "the record names its stamp")
        self.assertEqual("e" * 40, record["tree"])

    def test_export_tree_tests_same_tree_foreign_stamp_touches_everything(self):
        """The tree sha alone does not bind: a foreign writer that built another
        export and then re-stamped the SAME tree leaves a record whose tree
        matches, but whose mtimes are older than the foreign build. The stamp id
        tells them apart. Mutant (tree sha only): the foreign build looks fresh."""
        work = self.root / "export"
        files = {"src/a.rs": "1", "src/b.rs": "1"}
        self._sync(work, "c" * 40, files)
        self._foreign_stamp(work, "c" * 40, dict(files), build=True)
        _rc, third = self._sync(work, "e" * 40, dict(files, **{"src/b.rs": "2"}))
        self.assertGreater(third["before"]["src/a.rs"], third["built"],
                           "a foreign build since the record: no kept mtime")

    def test_export_tree_tests_unknown_history_touches_everything(self):
        """Fail-closed: no stamp, an unreadable diff, or a sync that broke halfway --
        every file looks newer than the build, the lane target rebuilds the tree."""
        work = self.root / "export"
        files = {"src/a.rs": "1", "src/b.rs": "1"}
        self._sync(work, "c" * 40, files)
        _rc, unknown = self._sync(work, "d" * 40, files, changed=lambda old, new: None)
        self.assertTrue(all(ns > unknown["built"] for ns in unknown["before"].values()), unknown)
        self._sync(work, "e" * 40, files)
        rc, _ = self._sync(work, "f" * 40, files, fail_extract=True)
        self.assertEqual(2, rc)
        self.assertFalse((work / self.ett.LANE_DIR / "target" / self.ett.STAMP).exists(),
                         "a half-synced tree carries no stamp")
        _rc, after = self._sync(work, "f" * 40, files, changed=lambda old, new: set())
        self.assertTrue(all(ns > after["built"] for ns in after["before"].values()), after)

    def test_export_changed_paths_reads_the_git_trees(self):
        repo = self.root / "repo"
        repo.mkdir()

        def git(*a):
            return subprocess.run(["git", "-C", str(repo), *a], check=True,
                                  capture_output=True, text=True).stdout.strip()
        git("init", "-q")
        (repo / "keep.rs").write_text("k")
        (repo / "edit.rs").write_text("1")
        (repo / "gone.rs").write_text("g")
        (repo / "run.sh").write_text("x")
        git("add", "-A")
        old = git("write-tree")
        (repo / "edit.rs").write_text("2")
        (repo / "gone.rs").unlink()
        (repo / "sub").mkdir()
        (repo / "sub" / "new.rs").write_text("n")
        (repo / "run.sh").chmod(0o755)
        git("add", "-A")
        new = git("write-tree")
        self.assertEqual({"edit.rs", "gone.rs", "sub/new.rs", "run.sh"},
                         self.ett.changed_paths(old, new, repo=repo))
        self.assertEqual(set(), self.ett.changed_paths(new, new, repo=repo))
        self.assertIsNone(self.ett.changed_paths("0" * 40, new, repo=repo))
        self.assertIsNone(self.ett.changed_paths("", new, repo=repo))

    def test_export_tree_tests_without_workdir_stays_temporary(self):
        env = {k: v for k, v in os.environ.items() if k != "MECLAW_EXPORT_WORKDIR"}
        with self.mock.patch.dict(os.environ, env, clear=True):
            rc, seen = self._run_tree_tests({})
        self.assertEqual(0, rc)
        (tree, target, existed), = seen["judge"]
        self.assertTrue(existed)
        self.assertFalse(tree.exists(), "the temporary tree is deleted as before")
        self.assertFalse(pathlib.Path(target).exists(), "the temporary target is deleted")

    def test_export_check_target_is_persistent(self):
        check = self.root / "export" / "check"
        calls = []

        def run(argv, cwd=None, env=None, **kw):
            if argv[:2] == ["cargo", "check"]:
                calls.append((pathlib.Path(env["CARGO_TARGET_DIR"]), pathlib.Path(cwd)))
            return subprocess.CompletedProcess(argv, 0, "", "")
        m = self.mock
        for value in (str(check), None):
            env = {k: v for k, v in os.environ.items() if k != "MECLAW_EXPORT_CHECK_TARGET"}
            if value:
                env["MECLAW_EXPORT_CHECK_TARGET"] = value
            with m.patch.dict(os.environ, env, clear=True), \
                    m.patch.object(self.me, "extract_tree"), \
                    m.patch.object(self.me.subprocess, "run", side_effect=run):
                self.me.receipt_on_worktree("c" * 40, [])
        (persistent, tree1), (temporary, tree2) = calls
        self.assertEqual(check, persistent)
        self.assertTrue(check.is_dir(), "the check target outlives the audit")
        self.assertFalse(temporary.exists(), "without the variable it is temporary as before")
        self.assertFalse(tree1.exists() or tree2.exists(), "the unpacked tree is always temporary")


class TestExecutorFailures(ExecutorCase):
    """Fix round 1 of W3 (review A): a station thread that fails, an abort, a hand-made dispatch."""

    def run_guarded(self, d, timeout=kit_time.HANG, **kw):
        box = {}

        def go():
            box["res"] = self.run_exec(d, **kw)
        t = threading.Thread(target=go, daemon=True)
        t.start()
        t.join(timeout)
        self.assertFalse(t.is_alive(), "the run hangs")
        return box["res"]

    def test_executor_station_exception_ends_aborted_and_frees_capacity(self):
        real = executor._Run._judge

        def judge(run, st, *args):
            if st["name"] == "boom":
                raise OSError(28, "No space left on device")
            return real(run, st, *args)
        stations = [station("boom", needs={"cpu": 24}),
                    station("side", needs={"cpu": 24}),
                    station("next", after=["boom"], when="any")]
        with mock.patch.object(executor._Run, "_judge", judge):
            rc, evs = self.run_guarded(self.dispatch(stations))
        ended = self.ended(evs)
        self.assertEqual(("ABORTED", "aborted_process"),
                         (ended["boom"]["verdict"], ended["boom"].get("reason")))
        self.assertEqual("GREEN", ended["side"]["verdict"], "the capacity of boom came back")
        self.assertEqual("pass_ended", evs[-1]["event"])
        self.assertEqual(("ABORTED", "process"),
                         (evs[-1]["data"]["verdict"], evs[-1]["data"].get("reason")))
        self.assertEqual(2, rc)

    def test_executor_hygiene_stop_ends_every_station_aborted(self):
        d = self.dispatch([station("a"), station("b", after=["a"])],
                          budget={"free_min_g": 0, "reserve_g": 10 ** 9})
        rc, evs = self.run_guarded(d)
        self.assertEqual(2, rc)
        ended = self.ended(evs)
        self.assertEqual({"a": ("ABORTED", "hygiene"), "b": ("ABORTED", "hygiene")},
                         {n: (e["verdict"], e.get("reason")) for n, e in ended.items()})
        self.assertEqual(("ABORTED", "hygiene"),
                         (evs[-1]["data"]["verdict"], evs[-1]["data"].get("reason")))

    def test_executor_nices_nextest_stations_on_an_egon_lane(self):
        d = self.dispatch([station("tests", nextest={"profile": "default"}), station("lint")],
                          **{"+nice": True})
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        env = {"PATH": os.environ.get("PATH", "")}
        self.assertIn("nice", run._wrap(run.by_name["tests"], ["x"], 60, env),
                      "tests ran with cargo:1 under gate.sh, so with nice/ionice")
        self.assertNotIn("nice", run._wrap(run.by_name["lint"], ["x"], 60, env))

    # #1158 (S0): the runner class from the store decides the Egon cap, never `+nice`.
    def _booked(self, cpu=12, **kw):
        d = self.dispatch([station("tests", needs={"cpu": cpu, "cargo": 1},
                                   nextest={"profile": "default"})], **kw)
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        env = run._env(run.by_name["tests"])
        return {k: env.get(k) for k in executor.THREAD_KEYS}

    def test_lane_class_runs_booked_width_not_egon_cap(self):
        # The gate's flags say `+nice` on a lane too (the prefix stays); the
        # class alone lifts the cap: a 12er lane books 12 jobs and 12 threads.
        d = self.dispatch([station("tests", needs={"cpu": 12, "cargo": 1})],
                          **{"+nice": True, executor.RUNNER_CLASS_KEY: "lane"})
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        first = (self.ws / "logs" / "tests.log").read_text().splitlines()[0]
        self.assertIn("CARGO_BUILD_JOBS=12 NEXTEST_TEST_THREADS=12", first)
        self.assertEqual({k: "12" for k in executor.THREAD_KEYS},
                         self._booked(**{"+nice": False, executor.RUNNER_CLASS_KEY: "lane"}))

    def test_egon_class_keeps_four_threads(self):
        for nice in (True, False):
            with self.subTest(nice=nice):
                self.assertEqual({k: "4" for k in executor.THREAD_KEYS},
                                 self._booked(**{"+nice": nice, executor.RUNNER_CLASS_KEY: "egon"}))
        # Under the cap a narrower booking stays what it booked.
        self.assertEqual({k: "2" for k in executor.THREAD_KEYS},
                         self._booked(cpu=2, **{executor.RUNNER_CLASS_KEY: "egon"}))

    def test_unknown_runner_class_keeps_egon_cap(self):
        # Fail-closed: a missing, empty, unknown or non-string class is Egon's.
        for cls in (None, "", "Lane", "build", 1, ["lane"]):
            with self.subTest(cls=cls):
                self.assertEqual({k: "4" for k in executor.THREAD_KEYS},
                                 self._booked(**{executor.RUNNER_CLASS_KEY: cls}))
        d = self.dispatch([station("tests", needs={"cpu": 12, "cargo": 1})])
        d.pop(executor.RUNNER_CLASS_KEY, None)
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        self.assertEqual("4", run._env(run.by_name["tests"])["NEXTEST_TEST_THREADS"])

    def test_no_nice_drops_prefix_but_egon_class_keeps_cap(self):
        d = self.dispatch([station("tests", needs={"cpu": 12, "cargo": 1},
                                   nextest={"profile": "default"})],
                          **{"+nice": False, executor.RUNNER_CLASS_KEY: "egon"})
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        env = {"PATH": os.environ.get("PATH", "")}
        self.assertNotIn("nice", run._wrap(run.by_name["tests"], ["x"], 60, env))
        self.assertEqual("4", run._env(run.by_name["tests"])["CARGO_BUILD_JOBS"])
        # On a lane the prefix stays with `+nice` and does not touch the width.
        d = self.dispatch([station("tests", needs={"cpu": 12, "cargo": 1},
                                   nextest={"profile": "default"})],
                          **{"+nice": True, executor.RUNNER_CLASS_KEY: "lane"})
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        self.assertIn("nice", run._wrap(run.by_name["tests"], ["x"], 60, env))
        self.assertEqual("12", run._env(run.by_name["tests"])["CARGO_BUILD_JOBS"])

    def test_egon_machine_keeps_cap_and_prefix_whatever_the_dispatch_says(self):
        # #1158 B-1: a host file that names a local lane of class egon is the
        # live Egon's machine -- a dispatch with class `lane` (a misconfigured
        # ssh line pointing here) still runs 4 threads under nice/ionice.
        self.lanes.write_text("b02 builder@b02.invalid class=lane\n"
                              "here local %s class=egon threads=4\n" % self.root)
        self.assertEqual({k: "4" for k in executor.THREAD_KEYS},
                         self._booked(**{"+nice": False, executor.RUNNER_CLASS_KEY: "lane"}))
        d = self.dispatch([station("tests", needs={"cpu": 12, "cargo": 1},
                                   nextest={"profile": "default"})],
                          **{"+nice": False, executor.RUNNER_CLASS_KEY: "lane"})
        run = executor._Run(d, io.StringIO(), io.StringIO(), None)
        env = {"PATH": os.environ.get("PATH", "")}
        self.assertIn("nice", run._wrap(run.by_name["tests"], ["x"], 60, env))
        self.assertTrue(executor.egon_capped(d))

    def test_build_host_with_class_lane_keeps_its_width(self):
        # A host file with ssh lanes only (or none) is no Egon machine: class
        # `lane` runs the booked width.
        self.lanes.write_text("b02 builder@b02.invalid class=lane\n")
        self.assertEqual({k: "12" for k in executor.THREAD_KEYS},
                         self._booked(**{executor.RUNNER_CLASS_KEY: "lane"}))
        self.assertFalse(executor.egon_capped({executor.RUNNER_CLASS_KEY: "lane"}))

    def test_executor_refuses_a_cargo_station_without_a_lock_file(self):
        d = self.dispatch([station("build", needs={"cargo": 1})])
        del d["+lock_file"]
        rc, evs = self.run_guarded(d)
        self.assertEqual(2, rc)
        # Delta A M-neu-3: refused before any station, yet the stream ends --
        # the kern books ABORTED, never a lost run whose tree stays live.
        self.assertEqual(["pass_ended"], [e["event"] for e in evs])
        self.assertEqual(("ABORTED", "process"), (evs[0]["data"]["verdict"], evs[0]["data"]["reason"]))
        self.assertIn("+lock_file", self.err_text)
        # Without cargo stations the run goes on, and no child is told the lock is held.
        d = self.dispatch([station("plain", 'test -z "${MECLAW_CARGO_LOCK_HELD:-}"')])
        del d["+lock_file"]
        rc, evs = self.run_guarded(d)
        self.assertEqual((0, "GREEN"), (rc, self.ended(evs)["plain"]["verdict"]))

    def test_executor_abort_between_check_and_register_still_kills(self):
        real = subprocess.Popen
        holder = {}

        def popen(args, *a, **kw):
            proc = real(args, *a, **kw)
            if "sleep 600" in " ".join(map(str, args)) and not holder.get("fired"):
                holder["fired"] = True
                holder["run"].abort("process")   # after the check, before the register
            return proc
        # `sleep 600` outlives the hang marker: a process the abort missed keeps
        # the run alive past it -- the event, not a duration, decides.
        d = self.dispatch([station("long", "sleep 600", timeout_min=15)])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        holder["run"] = run
        with mock.patch.object(executor.subprocess, "Popen", popen):
            box = {}
            t = threading.Thread(target=lambda: box.update(rc=run.execute()), daemon=True)
            t.start()
            t.join(kit_time.HANG)
        self.assertFalse(t.is_alive(), "HANG: the late process was never killed")
        self.assertEqual(2, box["rc"])

    def test_executor_hygiene_prefers_the_admissions_need_free_g(self):
        # The admission's need (reserve + the live runs' reservation + expected) wins.
        d = self.dispatch([station("a")], budget={"free_min_g": 0, "reserve_g": 0,
                                                   "need_free_g": 10 ** 9})
        rc, evs = self.run_guarded(d)
        self.assertEqual(2, rc)
        checked = [e["data"] for e in evs if e["event"] == "hygiene_checked"][0]
        self.assertEqual(10 ** 9, checked["reserved_g"])
        d = self.dispatch([station("a")], budget={"free_min_g": 0, "reserve_g": 10 ** 9,
                                                   "need_free_g": 0})
        rc, evs = self.run_guarded(d)
        self.assertEqual(0, rc, self.err_text)

    def test_executor_hygiene_refuses_a_floor_it_cannot_measure(self):
        # Delta A I-neu-1: fail-closed -- an unmeasurable disk never passes a floor.
        with mock.patch.object(executor, "_free_g", return_value=None):
            d = self.dispatch([station("a")], budget={"need_free_g": 60})
            rc, evs = self.run_guarded(d)
            self.assertEqual(2, rc)
            d = self.dispatch([station("a")], budget={})
            rc, evs = self.run_guarded(d)
            self.assertEqual(0, rc, self.err_text)

    def test_executor_disk_watcher_aborts_when_free_space_cannot_be_measured(self):
        # GH #1154 item 6: the watcher during the run fails closed like the
        # entry check -- an unmeasurable disk aborts, it never keeps running.
        with mock.patch.object(executor, "_free_g", return_value=None), \
                mock.patch.object(executor, "WATCH_PERIOD_S", 0.05):
            d = self.dispatch([station("long", "sleep 30", timeout_min=5)],
                              budget={"free_min_g": 10, "need_free_g": 0})
            rc, evs = self.run_guarded(d)
        self.assertEqual(2, rc)
        fired = [e["data"] for e in evs if e["event"] == "watchdog_fired"]
        self.assertEqual([("disk", None, "aborted")],
                         [(f["kind"], f["value"], f["action"]) for f in fired])

    def test_executor_receipt_without_pass_ended_is_never_green(self):
        # Review A M2: the receipt the lane writes after each station (and its
        # legacy v1 files) must not read GREEN before `pass_ended` -- W0a's
        # `receipt.project` keeps the verdict RUNNING until then.
        d = self.dispatch([station("a"), station("b", "exit 3", after=["a"], when="any")])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        self.assertEqual(1, run.execute())
        for drop in ([], ["b"]):
            rows = run._head_rows() + [e for e in run.events if e["event"] != "pass_ended"
                                       and e["data"].get("station") not in drop]
            v2 = receipt.project(rows)
            self.assertEqual("RUNNING", v2["verdict"])
            self.assertNotEqual("GREEN", executor.legacy_v1(v2)["verdict"])
        legacy = json.loads(pathlib.Path(run.legacy_path).read_text())
        self.assertEqual("RED", legacy["verdict"], "after pass_ended the file carries the verdict")

    def _exec_until_started(self, d):
        path = self.root / "dispatch.json"
        path.write_text(json.dumps(d))
        proc = subprocess.Popen([sys.executable, str(REPO / "scripts" / "pipeline.py"), "exec",
                                 "--dispatch", str(path)], cwd=str(REPO), text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: proc.poll() is None and proc.kill())
        head = []
        for line in proc.stdout:          # blocking: the stream itself is the event
            head.append(json.loads(line))
            if head[-1]["event"] == "station_started":
                break
        return proc, head

    def test_executor_sigterm_ends_the_run_aborted_with_pass_ended(self):
        # A foreign scope stop (systemd: SIGTERM to the executor) and the abort
        # verb (`pipeline.py exec --abort <run>`) both end the run ABORTED, with
        # `pass_ended` in the stream -- never `lost`.
        for way in ("signal", "verb"):
            with self.subTest(way=way):
                shutil.rmtree(self.ws, ignore_errors=True)
                d = self.dispatch([station("long", "sleep 600", timeout_min=15),
                                   station("later", after=["long"])])
                proc, head = self._exec_until_started(d)
                if way == "signal":
                    proc.send_signal(signal.SIGTERM)
                else:
                    res = subprocess.run([sys.executable, str(REPO / "scripts" / "pipeline.py"),
                                          "exec", "--abort", RUN, "--ws", str(self.ws)],
                                         cwd=str(REPO), capture_output=True, text=True,
                                         timeout=kit_time.HANG)
                    self.assertEqual(0, res.returncode, res.stderr)
                out, err = proc.communicate(timeout=kit_time.HANG)
                evs = head + [json.loads(ln) for ln in out.splitlines() if ln.strip()]
                self.assertEqual(2, proc.returncode, err)
                self.assertEqual("pass_ended", evs[-1]["event"])
                self.assertEqual(("ABORTED", "process"),
                                 (evs[-1]["data"]["verdict"], evs[-1]["data"].get("reason")))
                ended = self.ended(evs)
                self.assertEqual(("ABORTED", "aborted_process"),
                                 (ended["long"]["verdict"], ended["long"].get("reason")))
                self.assertEqual("ABORTED", ended["later"]["verdict"])

    def test_executor_sigterm_a_station_thread_takes_aborts_at_once(self):
        # GH #1178: Python runs a signal handler only in the main thread, and the
        # executor's main thread sleeps in `Condition.wait(None)`. A SIGTERM that
        # a station thread takes (its mask resets after the vfork of its process
        # start) must still abort the run at once, not at the end of the station.
        # Deterministic, no clock: the station thread signals itself right after
        # its process started, while it holds `cond` with the main thread's
        # waiter registered -- schedule() holds `cond` except inside `wait`, so
        # the main thread sleeps in the wait. Discriminator is the verdict of
        # `long` (`sleep 30`): killed = ABORTED, ran to its end = GREEN.
        if threading.current_thread() is not threading.main_thread():
            self.skipTest("the executor installs its handler from the main thread only")
        d = self.dispatch([station("long", "sleep 30", timeout_min=5),
                           station("later", after=["long"])])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        real_popen = subprocess.Popen
        box = {}

        def popen(argv, *a, **kw):
            proc = real_popen(argv, *a, **kw)
            if "sleep 30" in list(argv) and not box:
                with run.cond:
                    box["main_waits"] = bool(run.cond._waiters)
                    box["handler"] = signal.getsignal(signal.SIGTERM) == run._on_sigterm
                    if box["handler"]:      # never the default action: it ends the test runner
                        signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
            return proc
        with mock.patch.object(executor.subprocess, "Popen", popen):
            rc = run.execute()
        self.assertEqual({"main_waits": True, "handler": True}, box)
        ended = self.ended(run.events)
        self.assertEqual(("ABORTED", "aborted_process"),
                         (ended["long"]["verdict"], ended["long"].get("reason")))
        self.assertEqual("ABORTED", ended["later"]["verdict"])
        self.assertEqual(("ABORTED", "process"),
                         (run.events[-1]["data"]["verdict"], run.events[-1]["data"].get("reason")))
        self.assertEqual(2, rc)

    def test_executor_a_foreign_signal_on_the_wakeup_fd_does_not_abort(self):
        # GH #1178 L-3: the wakeup fd carries EVERY signal with a Python handler,
        # not only SIGTERM; the reader filters on SIGTERM and SIGINT (L-6, a stop
        # too). Without the filter a SIGUSR1 (or a SIGCHLD with a handler) would
        # abort the pass. Deterministic, no clock: the byte is in the pipe before
        # `execute` returns, the undo wakes the reader, the reader drains the
        # pipe and ends -- joined by the undo (GH #1218), so a wrong abort has
        # happened by the time `execute` returns.
        if threading.current_thread() is not threading.main_thread():
            self.skipTest("the executor installs its wakeup fd from the main thread only")
        d = self.dispatch([station("probe", "sleep 0.2")])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        real_popen = subprocess.Popen
        box = {}

        def popen(argv, *a, **kw):
            proc = real_popen(argv, *a, **kw)
            if not box:
                box["reader"] = [t for t in threading.enumerate() if t.name == "sigterm-wakeup"]
                signal.pthread_kill(threading.get_ident(), signal.SIGUSR1)
            return proc
        prev = signal.signal(signal.SIGUSR1, lambda signum, frame: None)
        try:
            with mock.patch.object(executor.subprocess, "Popen", popen):
                rc = run.execute()
        finally:
            signal.signal(signal.SIGUSR1, prev)
        self.assertEqual(1, len(box["reader"]))
        self.assertFalse(box["reader"][0].is_alive(), "the run returned with its reader alive")
        self.assertFalse(run.terminated, "a SIGUSR1 on the wakeup fd aborted the pass")
        self.assertEqual(0, rc)
        self.assertEqual("GREEN", self.ended(run.events)["probe"]["verdict"])

    def test_executor_sigint_a_station_thread_takes_aborts_at_once(self):
        # GH #1178 L-6: while `execute` runs in the main thread, a SIGINT is a
        # stop like SIGTERM (OR-TB.S10.B.1). A SIGINT a station thread takes
        # must abort the run at once, not raise KeyboardInterrupt at the end of
        # the station. Deterministic, no clock, as the SIGTERM pin above: the
        # station thread signals itself while the main thread waits in `cond`.
        # The test's own SIGINT handler stands in for `default_int_handler`
        # (a KeyboardInterrupt would end the test runner) and must be back after
        # the run. Discriminator is the verdict of `long` (`sleep 30`).
        if threading.current_thread() is not threading.main_thread():
            self.skipTest("the executor installs its handlers from the main thread only")
        d = self.dispatch([station("long", "sleep 30", timeout_min=5),
                           station("later", after=["long"])])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        real_popen = subprocess.Popen
        box = {}

        def popen(argv, *a, **kw):
            proc = real_popen(argv, *a, **kw)
            if "sleep 30" in list(argv) and not box:
                with run.cond:
                    box["main_waits"] = bool(run.cond._waiters)
                    box["handler"] = signal.getsignal(signal.SIGINT) == run._on_sigterm
                    signal.pthread_kill(threading.get_ident(), signal.SIGINT)
            return proc

        def own(signum, frame):
            box["own_handler_ran"] = True
        prev = signal.signal(signal.SIGINT, own)
        try:
            with mock.patch.object(executor.subprocess, "Popen", popen):
                rc = run.execute()
            self.assertIs(own, signal.getsignal(signal.SIGINT), "the old SIGINT handler is back")
        finally:
            signal.signal(signal.SIGINT, prev)
        ended = self.ended(run.events)
        self.assertEqual(("ABORTED", "aborted_process"),
                         (ended["long"]["verdict"], ended["long"].get("reason")))
        self.assertEqual({"main_waits": True, "handler": True}, box)
        self.assertEqual("ABORTED", ended["later"]["verdict"])
        self.assertEqual(("ABORTED", "process"),
                         (run.events[-1]["data"]["verdict"], run.events[-1]["data"].get("reason")))
        self.assertEqual(2, rc)

    def test_executor_wakeup_write_end_is_closed_by_the_reader_after_the_restore(self):
        # GH #1178 L-2: a C signal handler on a foreign thread may have read the
        # wakeup fd just before `undo` restores the old one and write after it.
        # Had the main thread closed the write end right after the restore, that
        # byte could land in a file that took over the fd number. The undo only
        # restores and wakes the reader; the reader closes both ends after it
        # drained the pipe. Pinned by recording every `os.close` with its thread,
        # plus: no fd leak, and the reader has ended when `execute` returns --
        # the undo joins it (GH #1218: a reader still draining after the return
        # closed its two fds inside the NEXT test's fd count, 6 != 4).
        if threading.current_thread() is not threading.main_thread():
            self.skipTest("the executor installs its wakeup fd from the main thread only")
        d = self.dispatch([station("probe", "true")])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        log = []
        real_close, real_set = os.close, signal.set_wakeup_fd

        def close(fd):
            log.append(("close", fd, threading.current_thread().name))
            return real_close(fd)

        def set_wakeup_fd(fd, **kw):
            log.append(("set", fd, threading.current_thread().name))
            return real_set(fd, **kw)
        real_popen = subprocess.Popen
        box = {}

        def popen(argv, *a, **kw):
            if not box:
                box["reader"] = [t for t in threading.enumerate() if t.name == "sigterm-wakeup"]
            return real_popen(argv, *a, **kw)
        # Garbage of an earlier test (an unclosed file or socket in a reference
        # cycle) holds fds until a collection: collect it first.
        gc.collect()
        fds_before = len(os.listdir("/proc/self/fd"))
        with mock.patch.object(executor.os, "close", close), \
                mock.patch.object(executor.signal, "set_wakeup_fd", set_wakeup_fd), \
                mock.patch.object(executor.subprocess, "Popen", popen):
            rc = run.execute()
            self.assertEqual(1, len(box["reader"]))
            self.assertFalse(box["reader"][0].is_alive(), "the run returned with its reader alive")
        self.assertEqual(0, rc)
        sets = [e for e in log if e[0] == "set"]
        self.assertEqual(2, len(sets), log)
        w = sets[0][1]
        w_closes = [i for i, e in enumerate(log) if e[:2] == ("close", w)]
        self.assertEqual(1, len(w_closes), log)
        self.assertEqual("sigterm-wakeup", log[w_closes[0]][2],
                         "the write end of the wakeup pipe was closed by %s" % log[w_closes[0]][2])
        self.assertLess(log.index(sets[1]), w_closes[0], log)
        self.assertEqual(fds_before, len(os.listdir("/proc/self/fd")), "fd leak")

    def test_executor_wakeup_undo_restores_the_flag_of_the_previous_fd(self):
        # GH #1178 L-1: `undo` hands the previous wakeup fd back. CPython 3.12
        # has no getter for its `warn_on_full_buffer`, so the kit keeps books:
        # a fd the kit set returns with the flag the kit set (False), a fd of a
        # foreign setter with the documented CPython default True, spelled out
        # explicitly -- never the implicit default by accident. Pinned by
        # recording every `set_wakeup_fd` call with its keywords.
        if threading.current_thread() is not threading.main_thread():
            self.skipTest("the executor installs its wakeup fd from the main thread only")
        d = self.dispatch([station("probe", "true")])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        calls = []
        real_set = signal.set_wakeup_fd

        def set_wakeup_fd(fd, **kw):
            calls.append((fd, kw))
            return real_set(fd, **kw)
        fr, fw = os.pipe()
        os.set_blocking(fw, False)
        orig = real_set(fw)                 # a foreign setter, flag unknown to the kit
        try:
            with mock.patch.object(executor.signal, "set_wakeup_fd", set_wakeup_fd):
                undo_outer = run._sigterm_wakeup()
                undo_inner = run._sigterm_wakeup()
                undo_inner()
                undo_outer()
        finally:
            real_set(orig)
            os.close(fr)
            os.close(fw)
        self.assertEqual(4, len(calls), calls)
        outer_w, inner_w = calls[0][0], calls[1][0]
        self.assertEqual({"warn_on_full_buffer": False}, calls[0][1])
        self.assertEqual({"warn_on_full_buffer": False}, calls[1][1])
        self.assertEqual((outer_w, {"warn_on_full_buffer": False}), calls[2],
                         "the kit's own fd came back without the kit's flag")
        self.assertEqual((fw, {"warn_on_full_buffer": True}), calls[3],
                         "a foreign fd came back without the explicit CPython default")

    def test_executor_outside_the_main_thread_installs_nothing(self):
        # GH #1178 L-4: `execute` from a thread other than the main thread can
        # neither set a handler nor a wakeup fd (Python allows both only in the
        # main thread). It installs nothing: no reader thread, SIGTERM/SIGINT
        # dispositions unchanged during and after the run, `set_wakeup_fd`
        # never called -- a SIGTERM keeps its process-wide meaning.
        d = self.dispatch([station("probe", "true")])
        run = executor._Run(d, io.StringIO(), io.StringIO(), lambda r: None)
        calls = []
        real_set = signal.set_wakeup_fd

        def set_wakeup_fd(*a, **kw):
            calls.append(a)
            return real_set(*a, **kw)
        real_popen = subprocess.Popen
        box = {}
        before = (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT))

        def popen(argv, *a, **kw):
            if "during" not in box:
                box["during"] = (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT))
                box["readers"] = [t.name for t in threading.enumerate()
                                  if t.name == "sigterm-wakeup"]
            return real_popen(argv, *a, **kw)

        def worker():
            try:
                box["rc"] = run.execute()
            except BaseException as exc:   # the pin reports it, not a stray thread trace
                box["exc"] = repr(exc)
        with mock.patch.object(executor.signal, "set_wakeup_fd", set_wakeup_fd), \
                mock.patch.object(executor.subprocess, "Popen", popen):
            t = threading.Thread(target=worker, name="not-main")
            t.start()
            t.join(kit_time.HANG)
        self.assertFalse(t.is_alive())
        self.assertNotIn("exc", box)
        self.assertEqual(0, box["rc"])
        self.assertEqual([], calls, "set_wakeup_fd was called")
        self.assertEqual([], box["readers"])
        self.assertEqual(before, box["during"])
        self.assertEqual(before, (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)))

    def test_executor_abort_verb_without_a_run_says_so(self):
        res = subprocess.run([sys.executable, str(REPO / "scripts" / "pipeline.py"), "exec",
                              "--abort", "run-000000000000", "--ws", str(self.ws)],
                             cwd=str(REPO), capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(1, res.returncode)
        self.assertIn("run-000000000000", res.stderr)

    def test_executor_overlaps_python_stations_and_clippy_beside_tests(self):
        # GR-ii: the plan's start values (build/tests cpu nproc, clippy nproc/2,
        # the rest 1) with the admission's overbooking: the small stations run
        # beside build, clippy beside tests.
        stations = [station("build", "sleep 0.4", needs={"cpu": 8, "cargo": 1}),
                    station("tests", "sleep 0.8", after=["build"], needs={"cpu": 8}),
                    station("clippy", "sleep 0.8", needs={"cpu": 4, "cargo": 1})]
        stations += [station("py%d" % i, "sleep 0.1") for i in range(3)]
        d = self.dispatch(stations, capacity={"cpu": 8, "colony": 8, "browser_chromium": 1,
                                              "browser_webkit": 1, "export": 1, "cargo": 1},
                          **{"+cpu_overbook": schedule.CPU_OVERBOOK})
        rc, evs = self.run_guarded(d)
        self.assertEqual(0, rc, self.err_text)
        pos = {(e["event"], e["data"].get("station")): i for i, e in enumerate(evs)}
        for py in ("py0", "py1", "py2"):
            self.assertLess(pos[("station_started", py)], pos[("station_ended", "build")],
                            "%s waited behind build" % py)
        self.assertLess(pos[("station_started", "clippy")], pos[("station_ended", "tests")],
                        "clippy waited behind tests")
        self.assertLess(pos[("station_started", "tests")], pos[("station_ended", "clippy")])


class TestCrateIsolationRed(ExecutorCase):
    """#1166 re-check m-5: a red `crate-isolation` says which `-p` failed, and a
    refused member list says why -- not a bare `rustc` place or nothing."""

    def _iso(self, argvs):
        return station("crate-isolation", cmd=None, scope="%d members, own features" % len(argvs),
                       cmds=[{"argv": a, "cwd": ""} for a in argvs])

    def test_a_red_second_call_names_its_crate(self):
        ran = self.root / "ran.txt"
        call = ("echo \"$2\" >> %s; [ \"$2\" != b ] || "
                "{ echo 'error[E0425]: cannot find value' >&2; "
                "echo '  --> crates/b/src/lib.rs:3:5' >&2; exit 101; }" % ran)
        st = self._iso([["sh", "-c", call, "sh", "-p", n] for n in ("a", "b", "c")])
        rc, evs = self.run_exec(self.dispatch([st]))
        ended = self.ended(evs)["crate-isolation"]
        self.assertEqual("RED", ended["verdict"])
        self.assertIn("`cargo check -p b` failed alone (exit 101)", ended["reason"])
        self.assertIn("at crates/b/src/lib.rs:3:5", ended["reason"])
        self.assertNotIn("-p a", ended["reason"])
        self.assertEqual(["a", "b"], ran.read_text().split()[:2], "a, then the red b")
        self.assertNotIn("c", ran.read_text().split(), "nothing runs after the red call")

    def test_a_refused_member_list_names_its_cause(self):
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
        self.addCleanup(sys.path.remove, sys.path[0])
        import gate_plan
        (self.tree / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/a"]\n')
        (self.tree / "crates" / "a").mkdir(parents=True)
        (self.tree / "crates" / "a" / "Cargo.toml").write_text("[package]\nversion = \"0.1.0\"\n")
        scope, cmds = gate_plan.crate_isolation(str(self.tree))
        self.assertEqual("refused", scope)
        st = self._iso(cmds)
        st["scope"] = scope
        rc, evs = self.run_exec(self.dispatch([st]))
        ended = self.ended(evs)["crate-isolation"]
        self.assertEqual("RED", ended["verdict"])
        self.assertIn("crates/a/Cargo.toml:1: the member has no [package] name", ended["reason"])
        self.assertIn("at crates/a/Cargo.toml:1 -- log", ended["reason"])


# A fake `ldd`: every file below a `minibrowser-wpe` misses libGL.so.1 (build02,
# 2026-10-10), every other one resolves.
FAKE_LDD = """#!/bin/sh
case "$1" in
  *minibrowser-wpe*) printf '\\tlibGL.so.1 => not found\\n\\tlibc.so.6 => /lib/libc.so.6\\n' ;;
  *) printf '\\tlibc.so.6 => /lib/libc.so.6\\n' ;;
esac
"""

# A fake `node`: its version, and "ok" from the start probe (`node -e <probe> ...`).
FAKE_NODE = """#!/bin/sh
[ "$1" = "-e" ] && { echo ok; exit 0; }
echo v24.19.0
"""

WEBKIT_BINS = ("MiniBrowser", "WPEWebProcess", "WPENetworkProcess", "WPEGPUProcess")


class BrowserLane(ExecutorCase):
    """A lane tree with the pinned Playwright and its two bundles beside it."""

    def browser_lane(self, ldd=FAKE_LDD, webkit=True):
        tools = self.tree / "workshop" / "tools" / "node_modules"
        (tools / "playwright").mkdir(parents=True, exist_ok=True)
        (tools / "playwright" / "package.json").write_text('{"version": "1.63.0"}')
        (tools / "playwright-core").mkdir(parents=True, exist_ok=True)
        (tools / "playwright-core" / "browsers.json").write_text(json.dumps({"browsers": [
            {"name": "chromium", "revision": "1243"},
            {"name": "chromium-headless-shell", "revision": "1243"},
            {"name": "webkit", "revision": "2359"}]}, indent=2))  # as Playwright ships it
        browsers = self.root / "ms-playwright"
        self.write_exe(browsers / "chromium_headless_shell-1243" / "chrome-headless-shell-linux64"
                       / "chrome-headless-shell", "")
        if webkit:
            for b in WEBKIT_BINS:
                self.write_exe(browsers / "webkit-2359" / "minibrowser-wpe" / "bin" / b, "")
        if ldd is not None:
            self.write_exe(self.bin / "ldd", ldd)
        # the probe is `lane_fingerprint.sh --browsers` (sh, sed, awk on the path) and
        # ends in a real start under the tree's wkenv.sh: a node that answers it
        self.write_exe(self.bin / "node", FAKE_NODE)
        (self.tree / "workshop" / "tools" / "wkenv.sh").write_text(":\n")
        dpkg = self.root / "dpkg-status"
        dpkg.write_text("Package: libegl-mesa0\n")
        return {"PATH": "%s%s%s" % (self.bin, os.pathsep, store_isolation.trapped_path()),
                "HOME": str(self.root), "PLAYWRIGHT_BROWSERS_PATH": str(browsers),
                "MECLAW_DPKG_STATUS": str(dpkg)}


class TestBrowserProbe(BrowserLane):
    # GH #1161, OR-TB.S3.17: build02's WebKit bundle could not load libGL.so.1, every
    # WebKit proof SKIPped, and `browser:display` read GREEN. The lane says per engine
    # whether it starts, in plain words when it does not.
    def test_the_probe_names_the_library_a_bundle_cannot_load(self):
        got = executor.browser_probe(str(self.tree), self.browser_lane())
        self.assertEqual("ok", got["chromium"])
        self.assertIn("libGL.so.1", got["webkit"])
        self.assertIn("webkit-2359", got["webkit"])

    def test_the_probe_names_a_missing_bundle_node_playwright_and_ldd(self):
        env = self.browser_lane(webkit=False)
        self.assertIn("webkit-2359", executor.browser_probe(str(self.tree), env)["webkit"])
        # "no ldd on the lane" is the lane script's sentence now; the path of the
        # probe carries the system's ldd, so no fake can take it away here.
        (self.bin / "node").unlink()
        if not shutil.which("node", path=env["PATH"]):  # a system node stays on the path
            self.assertEqual({"chromium", "webkit"},
                             {e for e, why in executor.browser_probe(str(self.tree), env).items()
                              if "node" in why})
        env = self.browser_lane()
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright-core"
         / "browsers.json").unlink()
        self.assertIn("playwright", executor.browser_probe(str(self.tree), env)["webkit"])

    def test_the_fingerprint_carries_the_probe(self):
        env = self.browser_lane()
        fp = executor.lane_fingerprint(str(self.tree), str(self.target), env)
        self.assertEqual("ok", fp["browsers"]["chromium"])
        self.assertIn("libGL.so.1", fp["browsers"]["webkit"])


class TestBrowserBooking(BrowserLane):
    def booked(self, cmd="true"):
        return [station("browser:display", cmd,
                        needs={"browser_chromium": 1, "browser_webkit": 1})]

    def test_a_booked_engine_that_does_not_start_is_red_before_the_start(self):
        env = self.browser_lane()
        marker = self.root / "ran"
        d = self.dispatch(self.booked("touch %s" % marker))
        d["+env"].update(env, PATH="%s:%s" % (self.bin, os.environ.get("PATH", "")))
        rc, evs = self.run_exec(d)
        row = self.ended(evs)["browser:display"]
        self.assertEqual("RED", row["verdict"], row)
        self.assertIn("webkit", row["reason"])
        self.assertIn("libGL.so.1", row["reason"])
        self.assertFalse(marker.exists(), "the proofs never ran")
        self.assertEqual(1, rc)
        # the run says it as a NOTE too (Re-Check S3 Teil 2, point 5)
        note = self.ended(evs)["browsers"]
        self.assertEqual("NOTE", note["verdict"])
        self.assertIn("capacity 0 on this lane: webkit: libraries not found: libGL.so.1",
                      note["reason"])

    def test_a_lane_without_playwright_is_red_not_skip_for_a_booking_station(self):
        # OR-DP-83 counted this SKIP in the total; a booking station is red now.
        d = self.dispatch(self.booked())
        rc, evs = self.run_exec(d)
        row = self.ended(evs)["browser:display"]
        self.assertEqual("RED", row["verdict"], row)
        self.assertIn("playwright", row["reason"])

    def test_a_booking_station_runs_with_the_switch_and_others_without(self):
        env = self.browser_lane(ldd="#!/bin/sh\nprintf '\\tlibc.so.6 => /lib/libc.so.6\\n'\n")
        seen = self.root / "seen"
        d = self.dispatch(self.booked('echo "b=${MECLAW_REQUIRE_BROWSERS:-}" >> %s' % seen)
                          + [station("plain", 'echo "p=${MECLAW_REQUIRE_BROWSERS:-}" >> %s' % seen)])
        d["+env"].update(env, PATH="%s:%s" % (self.bin, os.environ.get("PATH", "")))
        rc, evs = self.run_exec(d)
        self.assertEqual("GREEN", self.ended(evs)["browser:display"]["verdict"], self.err_text)
        self.assertEqual(["b=1", "p="], sorted(seen.read_text().split()))

    def test_a_run_that_books_no_engine_never_probes_the_browsers(self):
        """#1199 K14 (16.10.: all four lanes, five hand fixes): a docs-only run, a
        satellite or a plan without a browser station probed the browsers of ITS tree
        and the register took the answer -- a tree without `workshop/tools` said
        "refused" over a lane whose engines start. Only a run that checks the
        browsers measures them; the fingerprint of any other run says nothing."""
        calls = []

        def probe(tree, env):
            calls.append(tree)
            return {"chromium": "ok", "webkit": "ok"}

        with mock.patch.object(executor, "browser_probe", side_effect=probe):
            rc, evs = self.run_exec(self.dispatch([station("docs", "true")]))
            fp = [e["data"] for e in evs if e["event"] == "lane_fingerprint"]
            self.assertEqual(0, rc, self.err_text)
            self.assertEqual([], calls)
            self.assertEqual(1, len(fp))
            self.assertNotIn("browsers", fp[0])
            rc, evs = self.run_exec(self.dispatch(self.booked()))
            fp = [e["data"] for e in evs if e["event"] == "lane_fingerprint"]
            self.assertEqual(1, len(calls))
            self.assertEqual({"chromium": "ok", "webkit": "ok"}, fp[0]["browsers"])


class TestNeedsField(unittest.TestCase):
    def test_executor_reads_needs_not_the_legacy_cargo_flag(self):
        reuse = {"name": "tests", "cargo": True, "needs": {"cpu": 8, "cargo": 0}}
        self.assertFalse(executor._needs_cargo(reuse), "a nextest station after build reuses without the lock")
        self.assertTrue(executor._needs_cargo({"name": "old", "cargo": 1}), "a plan without needs keeps the old flag")


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class TestFlakyBlocksLikeRed(ExecutorCase):
    """Re-check tv-p1c (H, TV-F1): a non-legacy FLAKY stops the successors and
    trips fail-fast like a RED; a legacy FLAKY counts green and does neither."""

    def run_of(self, **kw):
        sts = [station("tests", "true"), station("after-tests", "true", after=["tests"])]
        return executor._Run(self.dispatch(sts, **kw), io.StringIO(), io.StringIO(), None)

    def test_a_flaky_station_blocks_its_successors(self):
        r = self.run_of()
        r._finish("tests", "FLAKY", 1.0, red=True)
        self.assertEqual(r._skip_reason(r.by_name["after-tests"]), "tests flaky")
        r = self.run_of()
        r._finish("tests", "FLAKY", 1.0, red=False)    # legacy: green
        self.assertIsNone(r._skip_reason(r.by_name["after-tests"]))

    def test_a_flaky_station_trips_fail_fast(self):
        r = self.run_of(**{"+fail_fast": True})
        r._finish("tests", "FLAKY", 1.0, red=True)
        self.assertTrue(r.fail_fast_hit)
        r = self.run_of(**{"+fail_fast": True})
        r._finish("tests", "FLAKY", 1.0, red=False)
        self.assertFalse(r.fail_fast_hit)


if __name__ == "__main__":
    unittest.main()
