"""Tests for the port register `scripts/pipeline/ports.py` (wave dev-tempo W7).

The register gives every lane slot its own port band, so parallel scenario
workers never meet on a port. What is pinned here is the band table itself
(no overlap with the Rust test band, the satellite labs or the kernel's
ephemeral range), the claim protocol (a listener in the band refuses the
claim, a claim file blocks a second claimer) and the slot list grammar the
runners take on `--slots`.

Every test works in a TempDir with an injected listener set or a fake `ss`
-- never against the real sockets of the host.
"""

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import ports  # noqa: E402
from scripts.tests import kit_time, store_isolation  # noqa: E402


# Store, night watchdog and outbound metric targets stay away from the
# machine's (scripts/tests/store_isolation.py).
def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def _overlap(a, b):
    return a[0] <= b[1] and b[0] <= a[1]


def _fake_ss(stdout, rc=0):
    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        return types.SimpleNamespace(returncode=rc, stdout=stdout, stderr="")
    run.calls = calls
    return run


SS_OUT = """\
LISTEN 0      4096         0.0.0.0:31005      0.0.0.0:*
LISTEN 0      128        127.0.0.1:31210      0.0.0.0:*
LISTEN 0      4096            [::]:22            [::]:*
LISTEN 0      511                *:80               *:*
LISTEN 0      4096   127.0.0.53%lo:53         0.0.0.0:*
garbage line
"""


class BandTable(unittest.TestCase):
    def test_ports_bands_do_not_overlap_rust_or_satellite_bands(self):
        self.assertEqual(ports.SLOT_BAND, (31000, 31799))
        self.assertEqual(ports.RUST_BAND, (20000, 29999))
        self.assertEqual(ports.SATELLITE_BAND, (7850, 7999))
        self.assertFalse(_overlap(ports.SLOT_BAND, ports.RUST_BAND))
        self.assertFalse(_overlap(ports.SLOT_BAND, ports.SATELLITE_BAND))
        for slot in range(ports.SLOT_COUNT):
            lo, hi = ports.band(slot)
            self.assertEqual(hi - lo + 1, ports.SLOT_WIDTH)
            self.assertFalse(_overlap((lo, hi), ports.RUST_BAND), slot)
            self.assertFalse(_overlap((lo, hi), ports.SATELLITE_BAND), slot)
            self.assertTrue(ports.SLOT_BAND[0] <= lo and hi <= ports.SLOT_BAND[1])
            for other in range(slot + 1, ports.SLOT_COUNT):
                self.assertFalse(_overlap((lo, hi), ports.band(other)), (slot, other))
        # The slot bands tile SLOT_BAND exactly.
        self.assertEqual(ports.band(0)[0], ports.SLOT_BAND[0])
        self.assertEqual(ports.band(ports.SLOT_COUNT - 1)[1], ports.SLOT_BAND[1])

    def test_ports_slot_band_below_ephemeral_range(self):
        self.assertEqual(ports.EPHEMERAL_BAND, (32768, 60999))
        self.assertLess(ports.SLOT_BAND[1], ports.EPHEMERAL_BAND[0])
        self.assertFalse(_overlap(ports.SLOT_BAND, ports.EPHEMERAL_BAND))

    def test_offsets_fill_one_slot_without_gaps(self):
        covered = []
        for lo, hi in ports.OFFSETS.values():
            covered.extend(range(lo, hi + 1))
        self.assertEqual(sorted(covered), list(range(ports.SLOT_WIDTH)))
        self.assertEqual(ports.OFFSETS["api"], (0, 0))
        self.assertEqual(ports.OFFSETS["mounts"], (1, 9))
        self.assertEqual(ports.OFFSETS["stub_model"], (10, 19))
        self.assertEqual(ports.OFFSETS["peers"], (20, 99))

    def test_band_port_base_and_slot_of_agree(self):
        self.assertEqual(ports.band(3), (31300, 31399))
        self.assertEqual(ports.port_base(3), 31300)
        self.assertEqual(ports.slot_of(31300), 3)
        self.assertEqual(ports.slot_of(31399), 3)
        self.assertEqual(ports.slot_of(31799), 7)
        self.assertIsNone(ports.slot_of(31800))
        self.assertIsNone(ports.slot_of(30999))
        self.assertIsNone(ports.slot_of(7824))
        for bad in (-1, ports.SLOT_COUNT, "1", True):
            with self.assertRaises(ValueError):
                ports.band(bad)

    def test_port_at_stays_inside_its_role(self):
        self.assertEqual(ports.port_at(2, "api"), 31200)
        self.assertEqual(ports.port_at(2, "mounts", 8), 31209)
        self.assertEqual(ports.port_at(2, "stub_model", 1), 31211)
        self.assertEqual(ports.port_at(2, "peers", 79), 31299)
        with self.assertRaises(ValueError):
            ports.port_at(2, "mounts", 9)
        with self.assertRaises(ValueError):
            ports.port_at(2, "nope")


class SlotList(unittest.TestCase):
    def test_parse_and_format_slots(self):
        self.assertEqual(ports.parse_slots("0,3,5"), [0, 3, 5])
        self.assertEqual(ports.parse_slots(" 7 , 1 "), [7, 1])
        self.assertEqual(ports.format_slots([0, 3, 5]), "0,3,5")
        self.assertEqual(ports.parse_slots(ports.format_slots([4, 2])), [4, 2])

    def test_parse_slots_refuses_bad_lists(self):
        for bad in ("", ",", "0,0", "8", "-1", "a", "1,,2", "1.5"):
            with self.assertRaises(ValueError, msg=bad):
                ports.parse_slots(bad)


class Claim(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.ws = pathlib.Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_listeners_parses_ss(self):
        run = _fake_ss(SS_OUT)
        self.assertEqual(ports.listeners(run=run), {31005, 31210, 22, 80, 53})
        self.assertEqual(run.calls, [["ss", "-ltnH"]])

    def test_listeners_refuses_a_failed_ss(self):
        with self.assertRaises(RuntimeError):
            ports.listeners(run=_fake_ss("", rc=1))

    def test_ports_claim_checks_listeners(self):
        # A listener anywhere in slot 1's band refuses the claim, nothing is written.
        with self.assertRaises(ports.SlotBusy) as cm:
            ports.claim(1, self.ws, run_id="r1", listening={22, 31142})
        self.assertEqual(cm.exception.slot, 1)
        self.assertEqual(cm.exception.ports, [31142])
        self.assertFalse((self.ws / "slots" / "1").exists())
        # Listeners outside the band (the neighbour slot, ssh) do not.
        rec = ports.claim(1, self.ws, run_id="r1", listening={22, 31099, 31200})
        self.assertEqual(rec["slot"], 1)
        self.assertEqual((rec["lo"], rec["hi"]), (31100, 31199))
        self.assertEqual(rec["run"], "r1")
        self.assertEqual(rec["pid"], os.getpid())
        self.assertTrue(rec["at"].endswith("Z"))
        on_disk = json.loads((self.ws / "slots" / "1").read_text())
        self.assertEqual(on_disk, rec)
        # Without an injected set the claim asks `ss` itself.
        with mock.patch.object(ports, "listeners", return_value={31250}) as ss:
            with self.assertRaises(ports.SlotBusy):
                ports.claim(2, self.ws)
        ss.assert_called_once_with()

    def test_claim_file_blocks_a_second_claimer_until_release(self):
        ports.claim(0, self.ws, run_id="a", listening=set())
        with self.assertRaises(ports.SlotBusy):
            ports.claim(0, self.ws, run_id="b", listening=set())
        self.assertEqual("a", ports.holder(0, self.ws)["run"])
        ports.release(0, self.ws)
        ports.release(0, self.ws)  # idempotent
        self.assertIsNone(ports.holder(0, self.ws))
        self.assertEqual(ports.claim(0, self.ws, run_id="b", listening=set())["run"], "b")

    def test_claim_of_a_dead_pid_is_taken_over(self):
        slots = self.ws / "slots"
        slots.mkdir()
        (slots / "4").write_text(json.dumps({"slot": 4, "pid": 2 ** 22 + 7}))
        self.assertEqual(ports.claim(4, self.ws, run_id="c", listening=set())["run"], "c")

    def test_stale_takeover_is_atomic(self):
        """Review W7 M2: two claimers who both find a stale claim must not both
        win. Claimer B starts while claimer A is between reading the stale
        record and writing its own; B may only get the slot after A is done --
        and then A's live claim refuses it."""
        slots = self.ws / "slots"
        slots.mkdir()
        (slots / "4").write_text(json.dumps({"slot": 4, "pid": 2 ** 22 + 7}))
        code = ("import sys; sys.path.insert(0, %r)\n"
                "from scripts.pipeline import ports\n"
                "try:\n"
                "    ports.claim(4, %r, run_id='b', listening=set())\n"
                "except ports.SlotBusy:\n"
                "    sys.exit(3)\n" % (str(REPO), str(self.ws)))
        real = ports._live_claim
        seen = {}

        def racing_live_claim(path):
            out = real(path)
            if "b" not in seen:
                seen["b"] = subprocess.Popen([sys.executable, "-I", "-c", code])
                # re-check tv-bn M7e A-M5 (TV-Z3): a 1.5-s pause gave B its
                # chance -- now the event: B waits on the lock A holds
                kit_time.until("claimer B waits on the slots lock",
                               lambda: seen["b"].pid in kit_time.flock_waiters(slots / ".lock"),
                               state=seen["b"].poll)
            return out
        with mock.patch.object(ports, "_live_claim", side_effect=racing_live_claim):
            rec = ports.claim(4, self.ws, run_id="a", listening=set())
        self.assertEqual("a", rec["run"])
        self.assertEqual(3, seen["b"].wait(timeout=kit_time.HANG), "B won a slot A holds")
        self.assertEqual("a", json.loads((slots / "4").read_text())["run"])

    def test_check_base_takes_only_slot_bases(self):
        self.assertEqual(ports.check_base(31000), 0)
        self.assertEqual(ports.check_base(31700), 7)
        for bad in (31050, 31800, 30900, 7824, 0, -1, True, "31000"):
            with self.assertRaises(ValueError, msg=bad):
                ports.check_base(bad)

    def test_default_ws_comes_from_the_environment(self):
        with mock.patch.dict(os.environ, {"MECLAW_PORT_REGISTER": str(self.ws)}):
            self.assertEqual(ports.default_ws(), str(self.ws))
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(ports.default_ws(), ports.DEFAULT_WS)

    def test_free_slots_lowest_first_skips_claims_and_listeners(self):
        ports.claim(0, self.ws, listening=set())
        listening = {31250, 7824}
        self.assertEqual(ports.free_slots(self.ws, 3, listening=listening), [1, 3, 4])
        self.assertEqual(ports.free_slots(self.ws, 0, listening=listening), [])
        # Fewer free than asked: the caller gets what there is.
        self.assertEqual(len(ports.free_slots(self.ws, 99, listening=listening)), 6)
        with self.assertRaises(ValueError):
            ports.free_slots(self.ws, -1, listening=listening)


if __name__ == "__main__":
    unittest.main()
