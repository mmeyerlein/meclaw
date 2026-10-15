"""The lab stations run at the width the lane booked (GH #1161, "started == booked").

The scenario stations booked colony slots and ran one case at a time: `plan.py`
booked `scenarios:memory` with eight colonies and four threads, and nothing handed
a slot to `run_scenarios.py`, whose default is one worker -- 396 s of serial
cases on a lane that had reserved eight colonies for them. `browser:display`
booked one Chromium and one WebKit side by side and ran its two nextest calls
one after the other, each WebKit test holding the whole call (469.6 s in the
integration pass of 2026-10-10, over the 466.3 s target on its own).

The executor now hands each colony station its booked slots of the port register
(`MECLAW_COLONY_SLOTS`, like `MECLAW_SELFTEST_SHARDS` for gate-selftest), the
runners take them as their `--slots`, and `browser:display` runs one part per
engine side by side, each at its share of the booked threads. Fail-closed: every
part runs exactly once, a red part is a red station, and a booking the register
cannot serve is RED before anything boots -- never a silent serial run on
colliding ports.

The runner halves (every case once, a red case) live beside the runners in
`workshop/evals/*/tests/`, because `workshop/` does not travel.

Run: `python3 -m unittest scripts.tests.test_pipeline_lab_width` from the root.
"""
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import store_isolation
from scripts.tests.test_pipeline_executor import ExecutorCase, station

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import gate_plan as gp  # noqa: E402
from scripts.pipeline import executor, plan as pl, ports, schedule  # noqa: E402

SLOTS = "MECLAW_COLONY_SLOTS"

# Writes the slots and threads it was started with, one line per call.
FAKE_LAB = """#!/bin/sh
printf '%s\\tslots=%s\\tthreads=%s\\n' "$1" "${MECLAW_COLONY_SLOTS:-}" "${NEXTEST_TEST_THREADS:-}" >> "$LAB_CALLS"
sleep "${2:-0}"
exit 0
"""

# A tier that takes its time, so two parts that run side by side overlap.
FAKE_TIER_SLOW = """#!/bin/sh
printf 'start\\t%s\\t%s\\t%s\\n' "$MECLAW_TIER_PROFILE" "$NEXTEST_TEST_THREADS" "$*" >> "$TIER_CALLS"
sleep 0.5
printf 'end\\t%s\\n' "$MECLAW_TIER_PROFILE" >> "$TIER_CALLS"
[ "$MECLAW_TIER_PROFILE" = "${FAKE_RED_PROFILE:-}" ] && exit 1
exit 0
"""


def by_name(stations):
    return {s.name: s for s in stations}


def write_topology(root, cpus, siblings, guest):
    """A fake `/sys` + `/proc/cpuinfo` under `root`: `cpus` CPUs, `siblings` threads per
    core (None: every CPU its own core), the `hypervisor` flag when `guest`."""
    base = pathlib.Path(root, "sys", "devices", "system", "cpu")
    per = siblings or 1
    for n in range(cpus):
        topo = base / ("cpu%d" % n) / "topology"
        topo.mkdir(parents=True)
        core = n % (cpus // per)
        topo.joinpath("thread_siblings_list").write_text(
            ",".join(str(core + k * (cpus // per)) for k in range(per)) + "\n")
    proc = pathlib.Path(root, "proc")
    proc.mkdir(parents=True, exist_ok=True)
    flags = "fpu vme sse2" + (" hypervisor" if guest else "")
    proc.joinpath("cpuinfo").write_text(
        "".join("processor\t: %d\nflags\t\t: %s\n\n" % (n, flags) for n in range(cpus)))


def fake_guest_without_siblings(root):
    """True on a guest whose topology shows one thread per core (this host, maybe)."""
    try:
        flags = pathlib.Path(root, "proc", "cpuinfo").read_text()
        base = pathlib.Path(root, "sys", "devices", "system", "cpu")
        lists = [p.read_text().strip() for p in base.glob("cpu[0-9]*/topology/thread_siblings_list")]
    except OSError:
        return False
    return " hypervisor" in flags and len(set(lists)) == len(lists)


class LabCase(ExecutorCase):
    def setUp(self):
        super().setUp()
        self.lab_calls = self.root / "lab-calls.txt"
        self.lab = self.write_exe(self.bin / "fake-lab", FAKE_LAB)
        # The width is what is measured here, not the lane's engines: both start
        # (the probe and its refusal: test_pipeline_executor.TestBrowserBooking).
        probe = mock.patch.object(executor, "browser_probe",
                                  return_value={"chromium": "ok", "webkit": "ok"})
        probe.start()
        self.addCleanup(probe.stop)

    def lab_station(self, name, colony, delay=0, cpu=1):
        return station(name, "%s %s %s" % (self.lab, name, delay),
                       needs={"colony": colony, "cpu": cpu})

    def lab_lines(self):
        out = {}
        for line in self.lab_calls.read_text().splitlines():
            name, slots, threads = line.split("\t")
            out[name] = (slots.split("=", 1)[1], threads.split("=", 1)[1])
        return out

    def dispatch(self, stations, **kw):
        d = super().dispatch(stations, **kw)
        d["+env"]["LAB_CALLS"] = str(self.lab_calls)
        return d


class TestScenarioStations(LabCase):
    def test_scenarios_station_passes_booked_slots_to_runner(self):
        """A memory-hive diff plans `scenarios:memory`; the lane books its colonies
        (the rest beside `browser:display`), and the runner is handed exactly those
        slots of the register."""
        st = by_name(gp.plan(["templates/memory-hive/embed/config.json"], "strand", repo=None))
        self.assertIn("scenarios:memory", st)
        lane = self.dispatch([])["capacity"]["colony"]
        n = pl.resolve_colony(pl.NEEDS["scenarios:memory"]["colony"], lane)
        self.assertLessEqual(n, ports.SLOT_COUNT)
        want = ",".join(str(i) for i in range(n))
        rc, evs = self.run_exec(self.dispatch([self.lab_station("scenarios:memory", n)]))
        self.assertEqual(0, rc, self.err_text)
        slots, _ = self.lab_lines()["scenarios:memory"]
        self.assertEqual(want, slots)
        log = (self.ws / "logs" / "scenarios:memory.log").read_text()
        self.assertIn("# colony slots (booked %d): %s" % (n, want), log)

    def test_memory_runs_beside_browser_display(self):
        """OR-TB.S3.12 (S4 RZ-S4-1): with eight colonies booked, `scenarios:memory`
        took the whole register and never fitted beside `browser:display` (two) --
        in the integration path it ran AFTER browser (build -> browser 416.3 s ->
        memory 52.6 s). It books the REST beside browser on the executing lane, so
        both start side by side on disjoint slots, on six real cores (build01-03)
        as on eight, twelve (build04) and twenty-four, and no slot stays unbooked.
        Re-check tb-s3 n1 M-neu-1: the lane's colonies (`cores=`) are not the port
        register's slots -- on build04 (cores=12) the rest was 12 - 3 = 9 over
        `ports.SLOT_COUNT` 8, RED before start while the admission booked it; the
        rest is counted against min(colonies, SLOT_COUNT)."""
        brw = pl.NEEDS["browser:display"]
        self.assertEqual(pl.COLONY_REST, pl.NEEDS["scenarios:memory"]["colony"])
        for lane_colonies in (6, 8, 12, 24):
            with self.subTest(lane_colonies=lane_colonies):
                mem = dict(pl.NEEDS["scenarios:memory"],
                           colony=pl.resolve_colony(pl.COLONY_REST, lane_colonies))
                self.assertEqual(min(lane_colonies, ports.SLOT_COUNT),
                                 mem["colony"] + brw["colony"])
                self.lab_calls.unlink(missing_ok=True)
                cap = dict(self.dispatch([])["capacity"], colony=lane_colonies)
                rc, evs = self.run_exec(self.dispatch([
                    station("scenarios:memory", "%s scenarios:memory 0.5" % self.lab, needs=mem),
                    station("lab-browser", "%s lab-browser 0.5" % self.lab, needs=dict(brw)),
                ], capacity=cap))
                self.assertEqual(0, rc, self.err_text)
                seq = [(e["event"], e["data"]["station"]) for e in evs
                       if e["event"] in ("station_started", "station_ended")
                       and e["data"]["station"] in ("scenarios:memory", "lab-browser")]
                first_end = min(i for i, (ev, _) in enumerate(seq) if ev == "station_ended")
                self.assertEqual({"scenarios:memory", "lab-browser"},
                                 {n for ev, n in seq[:first_end] if ev == "station_started"}, seq)
                got = self.lab_lines()
                memory = got["scenarios:memory"][0].split(",")
                browser = got["lab-browser"][0].split(",")
                self.assertEqual(mem["colony"], len(memory))
                self.assertEqual(brw["colony"], len(browser))
                self.assertEqual(set(), set(memory) & set(browser), got)

    def test_memory_rest_on_a_wide_lane_stays_in_the_register(self):
        """Re-check tb-s3 n1 M-neu-1: `cores=` 12 (build04) and 24 give the lane 12 and
        24 colonies (`schedule.colony_capacity`), the register has `ports.SLOT_COUNT`;
        memory books the rest of the register beside browser, on the plan path
        (`resolve_needs`) and on the admission path (`_demand_on`) alike."""
        brw = pl.NEEDS["browser:display"]["colony"]
        want = ports.SLOT_COUNT - brw
        for cores in (12, 24):
            with self.subTest(cores=cores):
                colonies = schedule.colony_capacity(cores=cores)
                self.assertEqual(cores, colonies)
                doc = {"stations": [{"name": "scenarios:memory",
                                     "needs": dict(pl.NEEDS["scenarios:memory"])}]}
                got = pl.resolve_needs(doc, 24, colonies)["stations"][0]["needs"]["colony"]
                self.assertEqual(want, got)
                with mock.patch.object(schedule, "_demand", side_effect=lambda st: st):
                    adm = schedule._demand_on(doc["stations"], 24, colonies)
                self.assertEqual(want, adm[0]["needs"]["colony"])

    def test_a_third_colony_station_waits_for_slots_on_a_wide_lane(self):
        """Re-check tb-s3 n1 M-neu-1: on a lane with more colonies than the register
        (build04, cores=12) the executor admitted a third colony station by the lane's
        colonies (5 + 3 + 4 <= 12) while the register's slots were gone -- RED before
        start. Admission counts the slots it can hand out, so the third waits."""
        rc, evs = self.run_exec(self.dispatch([
            self.lab_station("scenarios:memory", ports.SLOT_COUNT - 3, delay=0.5),
            self.lab_station("lab-browser", 3, delay=0.5),
            self.lab_station("scenarios:builder", 4),
        ], capacity=dict(self.dispatch([])["capacity"], colony=12)))
        self.assertEqual(0, rc, self.err_text)
        self.assertEqual({"GREEN"}, {e["verdict"] for e in self.ended(evs).values()},
                         self.ended(evs))
        got = self.lab_lines()
        self.assertEqual(4, len(got["scenarios:builder"][0].split(",")))

    def test_an_unresolved_colony_booking_is_red(self):
        """The rest beside browser is a symbol until the lane's colonies are known; a
        station that still carries it is RED before the runner starts -- never a run on
        no slots or on slots it did not book."""
        rc, evs = self.run_exec(self.dispatch([
            self.lab_station("scenarios:memory", pl.COLONY_REST)]))
        self.assertEqual("RED", self.ended(evs)["scenarios:memory"]["verdict"])
        self.assertFalse(self.lab_calls.exists(), "the runner started on an unresolved booking")


    def test_builder_scenarios_pass_booked_slots(self):
        """Two colony stations side by side (builder 4 + browser 2 fit the eight
        colonies of a lane) get disjoint slots, each exactly its booking; a third that
        starts after the builder ended reuses the freed slots."""
        rc, evs = self.run_exec(self.dispatch([
            self.lab_station("scenarios:builder", 4, delay=0.5),
            self.lab_station("lab-two", 2, delay=0.5),
            dict(self.lab_station("lab-after", 4), after=["scenarios:builder"]),
        ]))
        self.assertEqual(0, rc, self.err_text)
        got = self.lab_lines()
        builder = got["scenarios:builder"][0].split(",")
        two = got["lab-two"][0].split(",")
        self.assertEqual(4, len(builder))
        self.assertEqual(2, len(two))
        self.assertEqual(set(), set(builder) & set(two), got)
        self.assertEqual(4, len(got["lab-after"][0].split(",")))
        self.assertEqual(set(), set(got["lab-after"][0].split(",")) & set(two), got)

    def test_scenarios_port_collision_is_red(self):
        """A booking the port register cannot serve (more colonies than slots) is RED
        before the runner starts -- not a run alone on ports it would share."""
        rc, evs = self.run_exec(self.dispatch([
            self.lab_station("scenarios:memory", ports.SLOT_COUNT + 1)]))
        ended = self.ended(evs)["scenarios:memory"]
        self.assertEqual("RED", ended["verdict"])
        self.assertFalse(self.lab_calls.exists(), "the runner started on colliding ports")
        log = (self.ws / "logs" / "scenarios:memory.log").read_text()
        self.assertIn("colony slots", log)

    def test_a_booking_over_the_lane_names_the_lanes_colonies_not_the_port_register(self):
        """Re-check tb-s3 h1 M-1: a lane with 2 colonies said "the port register has 2"
        -- the port register has SLOT_COUNT slots; the lane's colonies come from its
        `cores=` (`schedule.colony_capacity`). Both numbers, each by its name."""
        d = self.dispatch([self.lab_station("scenarios:builder", 3)])
        d["capacity"]["colony"] = 2
        rc, evs = self.run_exec(d)
        self.assertEqual("RED", self.ended(evs)["scenarios:builder"]["verdict"])
        self.assertFalse(self.lab_calls.exists(), "the runner started over the lane's colonies")
        log = (self.ws / "logs" / "scenarios:builder.log").read_text()
        self.assertIn("colony slots: booked 3, the lane offers 2 colonies (cores=", log)
        self.assertIn("the port register has %d slots" % ports.SLOT_COUNT, log)
        self.assertNotIn("the port register has 2", log)

    def test_recall_and_persona_stations_pass_booked_width(self):
        """Stations that book no colony get no slots -- a value left in the lane's
        environment never reaches them -- and run at their booked thread."""
        for name in ("recall-harness", "persona-receipt"):
            self.assertNotIn(name, pl.NEEDS)          # one thread, no colony
        d = self.dispatch([self.lab_station("recall-harness", 0),
                           self.lab_station("persona-receipt", 0)])
        d["+env"][SLOTS] = "0,1,2,3,4,5,6,7"
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        got = self.lab_lines()
        self.assertEqual(("", "1"), got["recall-harness"])
        self.assertEqual(("", "1"), got["persona-receipt"])
        st = by_name(gp.plan(["templates/memory-hive/embed/config.json"], "integration",
                             repo=None))
        for name in ("recall-harness", "persona-receipt"):
            self.assertIsNone(st[name].parallel, name)

    def test_display_scenarios_run_with_booked_workers(self):
        """`scenarios:display` books four threads; its runner forks that many workers
        in every mode the lane executor runs (ci keeps the legacy serial call)."""
        cpu = pl.NEEDS["scenarios:display"]["cpu"]
        for mode in ("strand", "integration", "release"):
            st = by_name(gp.plan(["templates/display/compose/scenarios/scenarios.json"],
                                 mode, repo=None))
            self.assertIn(["python3", "templates/display/compose/scenarios/"
                           "run_display_scenarios.py", "--workers", str(cpu)],
                          st["scenarios:display"].cmds, mode)
        st = by_name(gp.plan(["templates/display/compose/scenarios/scenarios.json"],
                             "ci", repo=None))
        self.assertNotIn("--workers", st["scenarios:display"].cmds[0])


class TestRealCores(unittest.TestCase):
    """Ruling 2026-10-10: CPU-bound parallel width counts the REAL cores of the executing
    lane, not nproc (build01-03: 6 cores + HT = nproc 12; build04: 2 x 6 + HT = 24).
    A lane's colonies -- the lab slots of memory, builder and browser -- are its real
    cores from ONE source, the host file's `cores=` (`schedule.colony_capacity`,
    welle-tb S5; re-check tb-s3 h1 m-3); the executor hands out at most the port
    register's slots; nextest threads stay nproc."""

    def lane(self, cores=0, cls="lane", threads=None):
        return type("L", (), {"cls": cls, "threads": threads, "cores": cores})()

    def test_lane_colonies_follow_the_declared_cores(self):
        # `cores=` wins over every count of the fingerprint (its own `cores` included)
        self.assertEqual(6, schedule.runner_capacity(self.lane(6), {"vcpu": 12, "cores": 12})["colony"])
        self.assertEqual(12, schedule.runner_capacity(self.lane(12), {"vcpu": 24})["colony"])
        # no `cores=`: half the fingerprint's threads (hyper-threading), never nproc;
        # the fingerprint's own `cores` is no second source
        self.assertEqual(6, schedule.runner_capacity(self.lane(), {"vcpu": 12, "cores": 12})["colony"])
        # an egon lane books its declared cores like any lane (one number); a satellite
        # never lands there (`schedule._admit`), the core's fixed bookings stay admissible
        widest = max(int(n.get("colony", 0)) for n in pl.NEEDS.values())
        self.assertEqual(max(2, widest),
                         schedule.runner_capacity(self.lane(2, cls="egon"), {"vcpu": 4})["colony"])

    def test_runner_capacity_books_colonies_by_cores_and_cpu_by_threads(self):
        lane = type("L", (), {"cls": "lane", "threads": None})()
        cap = schedule.runner_capacity(lane, {"vcpu": 12, "cores": 6})
        self.assertEqual(12, cap["cpu"])
        self.assertEqual(6, cap["colony"])
        self.assertEqual(set(schedule.CAPACITY_KEYS), set(cap))

    def test_needs_resolve_on_the_lane(self):
        doc = {"stations": [{"name": n, "needs": dict(pl.NEEDS[n])}
                            for n in ("scenarios:memory", "scenarios:builder", "browser:display")]}
        # OR-TB.S3.14: browser books three (Chromium two at a time, WebKit one)
        for colonies, memory in ((6, 3), (8, 5)):
            got = {s["name"]: s["needs"] for s in pl.resolve_needs(doc, 12, colonies)["stations"]}
            self.assertEqual(memory, got["scenarios:memory"]["colony"])
            self.assertEqual(4, got["scenarios:builder"]["colony"])
            self.assertEqual(3, got["browser:display"]["colony"])
        self.assertEqual(4, pl.resolve_colony(4, 6))

    def test_the_station_docstring_names_the_booking_of_plan_needs(self):
        """The stations are described in the docstring of gate_plan.py and nowhere
        else; its colony booking follows `plan.NEEDS` (re-check h1 m-1: it still said
        "memory 6" after memory became the rest beside browser)."""
        doc = " ".join((gp.__doc__ or "").split())
        self.assertEqual(pl.COLONY_REST, pl.NEEDS["scenarios:memory"]["colony"])
        self.assertIn("memory the rest beside browser:display", doc)
        self.assertNotIn("memory 6", doc)
        self.assertIn("builder %d" % pl.NEEDS["scenarios:builder"]["colony"], doc)
        self.assertIn("browser:display %d" % pl.NEEDS["browser:display"]["colony"], doc)

    def test_local_fingerprint_counts_real_cores(self):
        cores = schedule.physical_cores()
        if fake_guest_without_siblings("/"):
            self.assertIsNone(cores)   # a guest's flat topology says nothing (OR-TB.S3.15)
            return
        self.assertIsInstance(cores, int)
        self.assertGreaterEqual(cores, 1)
        self.assertLessEqual(cores, os.cpu_count())

    def test_a_guest_without_visible_siblings_has_no_real_cores(self):
        """OR-TB.S3.15 (S3-diag, build02): a KVM guest shows every vCPU as its own core
        (E5-1660 v2 = 6 cores + HT, guest: 12 cores x 1 thread), so the sibling sets
        counted 12 and the lane would book 8 colonies on 6 real cores. A guest whose
        topology hides the siblings has no measured cores; the lane counts half its
        threads, like a fingerprint without `cores`. A bare host and a guest that shows
        its siblings keep the count."""
        for guest, siblings, want in ((True, None, None), (False, 2, 6), (True, 2, 6),
                                      (False, None, 12)):
            with self.subTest(guest=guest, siblings=siblings):
                with tempfile.TemporaryDirectory() as root:
                    write_topology(root, 12, siblings, guest)
                    self.assertEqual(want, schedule.physical_cores(root))
        self.assertEqual(6, schedule.colony_capacity(None, 12))


class TestBrowserParts(LabCase):
    def setUp(self):
        super().setUp()
        self.write_exe(self.bin / "node", "#!/bin/sh\nexit 0\n")
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(parents=True)
        self.write_exe(self.tree / "scripts" / "test-tier.sh", FAKE_TIER_SLOW)

    def browser_station(self, mode="integration"):
        st = by_name(gp.plan(["docs/x.md"], mode, repo=None))["browser:display"]
        return station("browser:display", None,
                       needs=pl.NEEDS["browser:display"],
                       cmds=[{"argv": c, "cwd": ""} for c in st.cmds],
                       nextest={"profile": "browser", "filter": ""},
                       parallel=st.parallel, parts=[])

    def tier_lines(self):
        return [ln.split("\t") for ln in self.tier_calls.read_text().splitlines()]

    def test_browser_station_runs_parts_with_booked_width(self):
        """One part per engine, side by side, each at its share of the four booked
        threads, each on its own nextest profile (two calls on one profile delete each
        other's JUnit file). The two filters split one union: every test once, every
        WebKit test in the one part that runs them one at a time (GH #1048). A
        release adds every browser unit outside the locks to the union: its `tests`
        is `all()` and gives them up (GH #1162) -- the set read from
        `browser_units`, not from the reach that moves them."""
        moved = sorted(t for t, (c, u, _x) in gp.browser_units().items()
                       if not gp._browser_lock_term(c, u))
        self.assertTrue(moved)
        for mode, colony in (("strand", False), ("integration", True), ("release", True)):
            diff = ["workshop/tools/display-layout-browser.mjs"] if mode == "strand" else ["docs/x.md"]
            st = by_name(gp.plan(diff, mode, repo=None))["browser:display"]
            self.assertEqual({"by": "cmd", "profiles": ["browser-chromium", "browser-webkit"]},
                             st.parallel, mode)
            union = " + ".join(([gp.COLONY_LOCK] if colony else []) + list(gp.IGNORED_LOCKS)
                               + [gp.SHEET_LOCK] + (moved if mode == "release" else []))
            self.assertEqual([["scripts/test-tier.sh", "filter",
                               "(%s) - %s" % (union, gp.WEBKIT_TESTS), "--run-ignored", "all"],
                              ["scripts/test-tier.sh", "filter",
                               "(%s) & %s" % (union, gp.WEBKIT_TESTS), "--run-ignored", "all"]],
                             st.cmds, mode)
        rc, evs = self.run_exec(self.dispatch([self.browser_station()]))
        self.assertEqual(0, rc, self.err_text)
        lines = self.tier_lines()
        starts = [ln for ln in lines if ln[0] == "start"]
        self.assertEqual(["browser-chromium", "browser-webkit"], sorted(s[1] for s in starts))
        self.assertEqual(["2", "2"], [s[2] for s in starts], "booked 4 threads, two parts")
        self.assertEqual(["start", "start", "end", "end"], [ln[0] for ln in lines],
                         "the parts ran one after the other")
        self.assertEqual("GREEN", self.ended(evs)["browser:display"]["verdict"])
        log = (self.ws / "logs" / "browser:display.log").read_text()
        self.assertIn("# part 1/2 (browser-chromium)", log)
        self.assertIn("# part 2/2 (browser-webkit)", log)

    def test_browser_part_red_is_station_red(self):
        """A red part makes the station red, and the other part still runs -- once."""
        d = self.dispatch([self.browser_station()])
        d["+env"]["FAKE_RED_PROFILE"] = "browser-chromium"
        rc, evs = self.run_exec(d)
        self.assertEqual("RED", self.ended(evs)["browser:display"]["verdict"])
        starts = [ln[1] for ln in self.tier_lines() if ln[0] == "start"]
        self.assertEqual(["browser-chromium", "browser-webkit"], sorted(starts))
        self.assertNotEqual(0, rc)

    def test_a_red_other_part_keeps_an_isolated_flaky_part_red(self):
        """Fail-closed (GH #1161, re-check M-2): both parts red, the first one isolates
        green. The isolation only speaks for the first part -- the second part's red
        keeps the station RED ("later command red"), never FLAKY (greenish). Without
        the guard in `_run_station` the station ended FLAKY with a red WebKit part.
        Exit 100 is nextest's test-failure code: any other exit is `unexplained red`
        before the guard is ever asked."""
        self.write_exe(self.tree / "scripts" / "test-tier.sh",
                       "#!/bin/sh\nprintf 'start\\t%s\\n' \"$MECLAW_TIER_PROFILE\" "
                       ">> \"$TIER_CALLS\"\nexit 100\n")
        isolated = []

        def green_isolation(run, st, cmd, log_path, started_wall, junits):
            isolated.append(cmd)
            return {"verdict": "FLAKY", "isolated": 1, "kind": "nextest", "events": []}

        with mock.patch.object(executor._Run, "_isolate", green_isolation):
            rc, evs = self.run_exec(self.dispatch([self.browser_station()]))
        ended = self.ended(evs)["browser:display"]
        self.assertEqual(1, len(isolated), "only the first red part is isolated")
        self.assertEqual("RED", ended["verdict"], ended)
        self.assertIn("later command red", ended.get("reason") or "", ended)
        self.assertNotEqual(0, rc)
        starts = [ln[1] for ln in self.tier_lines() if ln[0] == "start"]
        self.assertEqual(["browser-chromium", "browser-webkit"], sorted(starts),
                         "each part ran exactly once, none again after the isolation")

    def test_browser_parts_keep_their_flags_when_a_part_is_cached(self):
        """A cached lock binary leaves each part's own filter, never its flags: before,
        the first call was replaced by the whole union WITHOUT `--run-ignored` (every
        lock is `#[ignore]`d, so it ran nothing) and the other call ran unfiltered."""
        st = self.browser_station()
        st["parts"] = ["meclaw-cells::gh1003_the_shell_marks_orientation_browser",
                       "meclaw-cells::710_the_sheet_holds_in_both_engines_browser"]
        st["nextest"]["filter"] = "binary(/x/)"
        d = self.dispatch([st], cache_known=[{
            "station": "browser:display", "part": st["parts"][0], "key": "k",
            "from_run": "run-old"}])
        rc, evs = self.run_exec(d)
        self.assertEqual(0, rc, self.err_text)
        starts = sorted((ln[1], ln[3]) for ln in self.tier_lines() if ln[0] == "start")
        self.assertEqual(2, len(starts), starts)
        for profile, args in starts:
            self.assertTrue(args.endswith("--run-ignored all"), (profile, args))
            self.assertIn("& not (binary_id(=%s))" % st["parts"][0], args)
        self.assertIn("- %s" % gp.WEBKIT_TESTS, starts[0][1])
        self.assertIn("& %s" % gp.WEBKIT_TESTS, starts[1][1])


class TestBrowserColonyWidth(unittest.TestCase):
    def test_browser_parts_boot_at_most_the_booked_colonies(self):
        """OR-TB.S3.14: a station never runs wider than it books. Every test of the
        station boots a colony (`*_browser`), so the colonies at once are the tests at
        once: per part the summed `max-threads` of the groups its tests land in under
        the part's profile, over both parts equal to `needs.colony`. Before, the
        Chromium part fell through to `colony-boot` (four at a time; two at its two
        threads): with the WebKit test up to three colonies on a booking of two."""
        from scripts.tests import test_nextest_groups as ng
        import slow_tests
        config = ng.load_config()
        binaries = ng.built_test_binaries()
        width = 0
        for profile, cmd in zip(gp.BROWSER_PROFILES, gp.browser_cmds(True)):
            expr = cmd[2].replace("binary(", "binary_id(").replace(") - test(", ") & not test(")
            part = slow_tests.compile_filter(expr)
            rules = ng.group_rules(config, profile)
            groups, tests = set(), 0
            for binary, text in sorted(binaries.items()):
                for name in ng.listed_test_names(text):
                    if not part(binary, name):
                        continue
                    tests += 1
                    self.assertTrue(binary.endswith("_browser"), binary)
                    group = ng.group_of(rules, binary, name)
                    self.assertIsNotNone(group, "%s %s runs at the part's threads" % (binary, name))
                    groups.add(group)
            self.assertGreater(tests, 0, profile)
            width += sum(int(config["test-groups"][g]["max-threads"]) for g in groups)
        self.assertEqual(pl.NEEDS["browser:display"]["colony"], width)


class TestBrowserWorkerFormula(unittest.TestCase):
    """VORGABEN TV-B7: browser workers = max(1, floor((nproc - CARGO_BUILD_JOBS) / 2)),
    WebKit serial. The station's threads are its booking (`executor._book_threads`) and
    admission keeps every cargo station outside that share, so for `browser:display`
    nproc is the booked `cpu` and no cargo job runs inside it: the Chromium group's
    width is the formula over the booking, not a number of its own."""

    def test_the_formula(self):
        self.assertEqual(2, pl.browser_workers(8, 4))
        self.assertEqual(3, pl.browser_workers(12, 6))
        self.assertEqual(1, pl.browser_workers(2, 1))
        self.assertEqual(1, pl.browser_workers(1, 4))

    def test_the_chromium_group_is_the_formula_over_the_booking(self):
        from scripts.tests import test_nextest_groups as ng
        groups = ng.load_config()["test-groups"]
        need = pl.NEEDS["browser:display"]
        workers = pl.browser_workers(need["cpu"], need.get("cargo", 0))
        self.assertEqual(workers, groups["chromium-part"]["max-threads"])
        self.assertEqual(workers, executor._part_width(need["cpu"], len(gp.BROWSER_PROFILES)))
        self.assertEqual(1, groups["webkit"]["max-threads"])


class TestPartWidth(unittest.TestCase):
    def test_part_width_splits_the_booking(self):
        self.assertEqual(2, executor._part_width(4, 2))
        self.assertEqual(1, executor._part_width(1, 2))
        self.assertEqual(3, executor._part_width(7, 2))


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
