"""The time building blocks of the kit's self-tests (`scripts/tests/kit_time.py`).

Run: `python3 -m unittest scripts.tests.test_kit_time` from the root.
"""
import datetime
import os
import unittest
from unittest import mock

from scripts.tests import kit_time, store_isolation


class TestFakeClock(unittest.TestCase):
    def test_the_clock_moves_only_on_advance(self):
        clock = kit_time.FakeClock(start=10.0, wall=100.0)
        self.assertEqual((10.0, 100.0, 10.0), (clock.monotonic(), clock.time(), clock()))
        self.assertEqual(14.5, clock.advance(4.5))
        self.assertEqual((14.5, 104.5), (clock.monotonic(), clock.time()))
        clock.sleep(0.5)
        self.assertEqual(15.0, clock.monotonic())

    def test_the_clock_does_not_run_backwards(self):
        with self.assertRaisesRegex(ValueError, "does not run backwards"):
            kit_time.FakeClock().advance(-1)

    def test_two_clocks_share_nothing(self):
        one, two = kit_time.FakeClock(), kit_time.FakeClock()
        one.advance(7)
        self.assertEqual(7.0, one.monotonic() - two.monotonic())


class TestClockShift(unittest.TestCase):
    """T4 of the flake triage (VG TV-F2): `MECLAW_TEST_CLOCK_SHIFT` moves a
    fake clock's default start a day on, or just before a month's or a year's
    end, so a test that runs one second further crosses it."""

    def clock(self, shift, **kw):
        with mock.patch.dict(os.environ, {kit_time.CLOCK_SHIFT_ENV: shift}):
            return kit_time.FakeClock(**kw)

    def utc(self, seconds):
        return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc)

    def test_the_reader_names_the_variable_of_the_triage(self):
        self.assertEqual("MECLAW_TEST_CLOCK_SHIFT", kit_time.CLOCK_SHIFT_ENV)
        self.assertEqual(("+24h", "month", "year"), kit_time.CLOCK_SHIFTS)

    def test_without_the_variable_the_clock_starts_where_it_says(self):
        with mock.patch.dict(os.environ):
            os.environ.pop(kit_time.CLOCK_SHIFT_ENV, None)
            unset = kit_time.FakeClock()
        for clock in (unset, self.clock("")):
            self.assertEqual((1000.0, 1_760_000_000.0), (clock.monotonic(), clock.time()))

    def test_a_day_on_moves_both_clocks(self):
        clock = self.clock("+24h")
        self.assertEqual((1000.0 + 86400, 1_760_000_000.0 + 86400),
                         (clock.monotonic(), clock.time()))

    def test_month_and_year_land_one_second_before_the_end(self):
        month = self.clock("month")
        self.assertEqual(self.utc(month.time()), datetime.datetime(
            2025, 10, 31, 23, 59, 59, tzinfo=datetime.timezone.utc))
        month.advance(1)
        self.assertEqual((11, 1), (self.utc(month.time()).month, self.utc(month.time()).day))
        year = self.clock("year")
        self.assertEqual(self.utc(year.time()), datetime.datetime(
            2025, 12, 31, 23, 59, 59, tzinfo=datetime.timezone.utc))
        self.assertEqual(year.time() - 1_760_000_000.0, year.monotonic() - 1000.0,
                         "monotonic moves with the wall clock")
        year.advance(1)
        self.assertEqual((2026, 1, 1), self.utc(year.time()).timetuple()[:3])

    def test_a_start_the_test_names_is_not_moved(self):
        """A fixed instant is an argument (TV-Z11 a), not the clock's guess."""
        clock = self.clock("year", start=10.0, wall=100.0)
        self.assertEqual((10.0, 100.0), (clock.monotonic(), clock.time()))
        half = self.clock("+24h", wall=100.0)
        self.assertEqual((1000.0 + 86400, 100.0), (half.monotonic(), half.time()))

    def test_an_unknown_shift_is_refused_not_ignored(self):
        with self.assertRaisesRegex(ValueError, r"MECLAW_TEST_CLOCK_SHIFT='\+1w'.*\+24h, month, year"):
            self.clock("+1w")


class TestRealClock(unittest.TestCase):
    def test_the_real_clock_has_the_fake_clocks_face(self):
        """Helpers that cross a process take `clock=` with `kit_time.REAL` as
        the default; a test hands a `FakeClock` in its place."""
        for name in ("monotonic", "time", "sleep"):
            self.assertTrue(callable(getattr(kit_time.REAL, name)), name)
            self.assertTrue(callable(getattr(kit_time.FakeClock(), name)), name)
        self.assertNotIsInstance(kit_time.REAL, kit_time.FakeClock)


class TestHang(unittest.TestCase):
    def test_the_marker_is_thirty_seconds_times_the_factor(self):
        self.assertEqual(30.0, kit_time.HANG_S)
        with mock.patch.dict(os.environ, {"MECLAW_TEST_TIME_FACTOR": "3"}):
            self.assertEqual(3.0, kit_time.time_factor())
        with mock.patch.dict(os.environ, {"MECLAW_TEST_TIME_FACTOR": "0.1"}):
            self.assertEqual(1.0, kit_time.time_factor(), "a factor never shortens a marker")
        with mock.patch.dict(os.environ, {"MECLAW_TEST_TIME_FACTOR": "fast"}):
            self.assertEqual(1.0, kit_time.time_factor())

    def test_until_returns_the_first_truthy_value(self):
        seen = iter([None, 0, "", "ready"])
        self.assertEqual("ready", kit_time.until("the value", lambda: next(seen), poll=0))

    def test_until_past_the_marker_names_the_condition_and_the_last_state(self):
        with self.assertRaisesRegex(kit_time.Hang, r"^HANG: the marker file after 0 s \(last: 'absent'\)"):
            kit_time.until("the marker file", lambda: False, deadline=0, state=lambda: "absent",
                           poll=0)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
