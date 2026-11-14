"""GH #1521: the gate's stdout survives a station line longer than a pipe buffer.

A strand gate ended ABORTED without a verdict: `_Pass.stream` printed the
`GATE tests [...]` line (65 663 characters, the scope listed every selected
test) onto a stdout pipe that was non-blocking, and `print` raised
`BlockingIOError: [Errno 11]`. Two causes; the pins:

* the station line carries a short form of the scope -- the full scope stays
  in the receipt, in `summary.txt` and in the station logs;
* every line the runner writes to its stdout survives a non-blocking pipe
  whose reader reads slowly: the whole line arrives, and a reader that takes
  no byte at all ends in a named error instead of an endless wait;
* stdout and stderr are both wrapped (one pipe under `strand.sh gate`), and a
  line holds the output until it is whole (`_OUTPUT_LOCK`);
* after that named error (review R1) the pass ends ABORTED, exit 2 -- never
  RED --, the executor is asked to abort, the reason stands at the lease, in
  the receipt and in the store, and no further line waits again.

Why the pipe is non-blocking: `strand.sh gate` starts the runner as
`... 2>&1 | gate_tee`, so fd 1 and fd 2 are one open file description of one
pipe; the lane's `ssh` inherits fd 2 and sets O_NONBLOCK on it while it runs
(OpenSSH does so for every non-tty stdio fd) -- and with it on fd 1. A caller
that sends `strand.sh gate` into a file does not change that inner pipe.
"""

import contextlib
import io
import json
import os
import pathlib
import re
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import gate  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

RUN = "run-0123456789ab"
REV = "abcdef0123456789abcdef0123456789abcdef01"
PIPE_BUF_BYTES = 64 * 1024
HANG_MARK_S = 60  # a hang marker only, never a verdict (dev-rules section 11)
# The `red_lines` pattern of scripts/strand.sh -- the reader of the line.
RED_LINES = re.compile(r"^GATE [^ ]+ \[.*\] [0-9]+s (RED|ASK|ABORTED)")
GATE_LINE = re.compile(r"^GATE (\S+) \[(.*)\] (\d+)s (\S+)(?: (.*))?$")


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def _big_scope(n=1500):
    """A `build`-like scope: n test binaries, comma-separated, over 64 KiB."""
    return ", ".join("meclaw-cells::gh%04d_a_long_test_binary_name_for_the_scope" % i
                     for i in range(n))


def _row(sec, kind, event, data):
    return {"at": "2026-11-14T04:00:%02dZ" % sec, "run": RUN, "kind": kind, "event": event,
            "data": dict(data, run=RUN)}


MAX_WAITS = 10000  # a hang marker only: a stall that never ends waits for ever


class _Hang(BaseException):
    """A wait that does not end (BaseException: no `except Exception` of the
    runner swallows it)."""


class _VirtualClock:
    """The output's clock and its wait, virtual: a wait moves the clock by its
    timeout and no real time passes. More than MAX_WAITS waits is a hang --
    the pin goes red instead of hanging."""

    def __init__(self, case):
        self.case, self.now, self.waits = case, 0.0, 0

    def clock(self):
        return self.now

    def wait(self, fd, timeout):
        self.waits += 1
        if self.waits > MAX_WAITS:
            raise _Hang("still waiting after %d waits (%.0f virtual s)" % (self.waits, self.now))
        self.now += max(0.0, timeout)

    def patched(self):
        stack = contextlib.ExitStack()
        stack.enter_context(mock.patch.object(gate, "_out_clock", self.clock, create=True))
        stack.enter_context(mock.patch.object(gate, "_wait_writable", self.wait))
        return stack


def _full_pipe():
    """A non-blocking pipe whose buffer is full and whose reader reads nothing."""
    r, w = os.pipe()
    os.set_blocking(w, False)
    for size in (4096, 1):
        while True:
            try:
                os.write(w, b"f" * size)
            except BlockingIOError:
                break
    return r, w


def _close_quietly(fh, fd):
    try:
        fh.close()
    except BlockingIOError:  # the old form: the buffer still held a line
        try:
            os.close(fd)
        except OSError:
            pass  # the failed close already let the fd go


class StationLineCut(unittest.TestCase):
    """The station line carries a short scope, the receipt the full one."""

    def test_a_station_line_over_64_kib_is_cut_and_the_receipt_keeps_the_scope(self):
        scope = _big_scope()
        entries = scope.split(", ")
        full_line = "GATE tests [%s] 120s RED 1 failed" % scope
        self.assertGreater(len(full_line.encode()), PIPE_BUF_BYTES)
        doc = {"rev": REV, "stations": [{"name": "tests", "scope": scope},
                                        {"name": "lock-wait", "scope": "12s behind other runs"}]}
        rows = [
            _row(1, "status", "station_ended",
                 {"station": "lock-wait", "scope": "12s behind other runs", "verdict": "NOTE",
                  "secs": 12, "exit": 0}),
            _row(2, "status", "station_ended",
                 {"station": "tests", "scope": scope, "verdict": "RED", "secs": 120, "exit": 1,
                  "reason": "1 failed"}),
            _row(3, "status", "pass_ended",
                 {"verdict": "RED", "secs": 132, "lane_seconds": 132, "critical_path_s": 132,
                  "rounds": 1, "cache_hits": 0, "stations": {"red": 1}}),
        ]
        proc = types.SimpleNamespace(
            stdout=io.StringIO("".join(json.dumps(r) + "\n" for r in rows)), wait=lambda: 1)
        with tempfile.TemporaryDirectory() as tmp:
            o = types.SimpleNamespace(mode="strand", archive=tmp, cache="on")
            out, err = io.StringIO(), io.StringIO()
            p = gate._Pass(o, tmp, doc, out, err)
            head = p.head(RUN, "local", 1)
            p.stream(proc)
            lines = out.getvalue().splitlines()
            self.assertEqual("GATE lock-wait [12s behind other runs] 12s NOTE", lines[0])
            line = lines[1]
            self.assertLess(len(line), 512, line[:600])
            m = GATE_LINE.match(line)
            self.assertIsNotNone(m, line)
            self.assertEqual(("tests", "120", "RED", "1 failed"),
                             (m.group(1), m.group(3), m.group(4), m.group(5)))
            self.assertRegex(line, RED_LINES)  # strand.sh still lists it as red
            short = m.group(2)
            shown, more = re.fullmatch(r"(.*) \.\.\. \+(\d+) more", short).groups()
            kept = shown.split(", ")
            self.assertEqual(entries[:len(kept)], kept)
            self.assertEqual(len(entries) - len(kept), int(more))
            self.assertGreater(len(kept), 0)
            # The receipt and summary.txt carry the full scope.
            v2 = p.v2(os.path.join(tmp, "no-dest"), head)
            with mock.patch.object(gate._Pass, "write_legacy"):
                p.finish(v2, os.path.join(tmp, "no-dest"), "local", 1)
            with open(os.path.join(tmp, "latest", "receipt.json"), encoding="utf-8") as fh:
                stations = {s["name"]: s for s in json.load(fh)["stations"]}
            self.assertEqual(scope, stations["tests"]["scope"])
            with open(os.path.join(tmp, "latest", "summary.txt"), encoding="utf-8") as fh:
                self.assertIn(full_line, fh.read().splitlines())

    def test_a_scope_of_one_long_entry_is_cut_to_its_head(self):
        """A nextest filter is one entry at its top level: its head stays."""
        scope = "all() - (%s)" % " + ".join("binary(/^b%05d$/)" % i for i in range(5000))
        line = gate._gate_line({"station": "tests", "secs": 3, "verdict": "GREEN"},
                               {"tests": scope})
        self.assertLess(len(line), 512, line[:600])
        short = GATE_LINE.match(line).group(2)
        head, more = re.fullmatch(r"(.*) \.\.\. \+(\d+) more", short).groups()
        self.assertTrue(scope.startswith(head), head)
        self.assertEqual("1", more)

    def test_a_short_scope_stays_byte_for_byte(self):
        for scope in ("workspace", "12s behind other runs", "scripts/*.sh .github/gates/*.sh",
                      "a, b + c", ""):
            with self.subTest(scope=scope):
                self.assertEqual("GATE s [%s] 1s GREEN" % scope,
                                 gate._gate_line({"station": "s", "secs": 1, "verdict": "GREEN"},
                                                 {"s": scope}))


class NonBlockingStdout(unittest.TestCase):
    """Every runner line reaches a slow reader of a non-blocking pipe whole."""

    def test_a_line_over_64_kib_reaches_a_slow_reader_on_a_non_blocking_pipe(self):
        line = "GATE tests [%s] 1s GREEN" % ("x" * (PIPE_BUF_BYTES + 4096))
        summary = "GATE-SUMMARY strand abcdef0 1/1 1s GREEN"
        r, w = os.pipe()
        os.set_blocking(w, False)
        wf = open(w, "w", encoding="utf-8")  # noqa: SIM115 -- closed below, the reader needs EOF
        stuck, got = threading.Event(), []

        def reader():
            # Slow: not one byte before the writer met a full pipe, then 4 KiB a read.
            stuck.wait(HANG_MARK_S)
            while True:
                chunk = os.read(r, 4096)
                if not chunk:
                    return
                got.append(chunk)

        wait = getattr(gate, "_wait_writable", None)

        def blocked(fd, timeout):
            stuck.set()
            return wait(fd, timeout)

        def run_gate(argv, out, err):
            print(line, file=out, flush=True)
            print(summary, file=out, flush=True)
            return 0

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        try:
            with mock.patch.object(gate, "run_gate", run_gate), \
                    mock.patch.object(gate, "_wait_writable", blocked, create=True), \
                    mock.patch("sys.stdout", wf), mock.patch("sys.stderr", io.StringIO()):
                rc = gate._cmd_gate(types.SimpleNamespace(argv=["strand"]))
        finally:
            stuck.set()
            try:
                wf.close()
            except BlockingIOError:  # the old form: the buffer still held the line
                try:
                    os.close(w)
                except OSError:
                    pass  # the failed close already let the fd go
            t.join(HANG_MARK_S)
            os.close(r)
        self.assertFalse(t.is_alive(), "the reader saw no EOF")
        self.assertEqual(0, rc)
        data = b"".join(got).decode("utf-8")
        self.assertEqual(line + "\n" + summary + "\n", data)

    def test_a_line_over_64_kib_on_stderr_reaches_a_slow_reader_on_a_non_blocking_pipe(self):
        """stderr is the same pipe as stdout under `strand.sh gate`: wrapped too."""
        line = "gate: %s" % ("e" * (PIPE_BUF_BYTES + 4096))
        r, w = os.pipe()
        os.set_blocking(w, False)
        wf = open(w, "w", encoding="utf-8")  # noqa: SIM115 -- closed below, the reader needs EOF
        stuck, got = threading.Event(), []

        def reader():
            stuck.wait(HANG_MARK_S)
            while True:
                chunk = os.read(r, 4096)
                if not chunk:
                    return
                got.append(chunk)

        wait = gate._wait_writable

        def blocked(fd, timeout):
            stuck.set()
            return wait(fd, timeout)

        def run_gate(argv, out, err):
            print(line, file=err, flush=True)
            return 0

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        try:
            with mock.patch.object(gate, "run_gate", run_gate), \
                    mock.patch.object(gate, "_wait_writable", blocked), \
                    mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", wf):
                rc = gate._cmd_gate(types.SimpleNamespace(argv=["strand"]))
        finally:
            stuck.set()
            _close_quietly(wf, w)
            t.join(HANG_MARK_S)
            os.close(r)
        self.assertFalse(t.is_alive(), "the reader saw no EOF")
        self.assertEqual(0, rc)
        self.assertEqual(line + "\n", b"".join(got).decode("utf-8"))

    def test_no_other_thread_writes_into_a_line_that_waits_for_its_reader(self):
        """stdout and stderr are one pipe: a line holds the output until it is whole."""
        big = "A" * (PIPE_BUF_BYTES + 4096) + "\n"
        small = "B: the deadline timer's line\n"
        r, w = os.pipe()
        os.set_blocking(w, False)
        w2 = os.dup(w)  # fd 2 beside fd 1, one pipe
        fa = open(w, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        fb = open(w2, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        one, two = gate._WholeLines(fa), gate._WholeLines(fb)
        stuck, got, seen = threading.Event(), [], {}
        wait = gate._wait_writable
        second = threading.Thread(target=two.write, args=(small,), daemon=True)

        def reader():
            stuck.wait(HANG_MARK_S)
            while True:
                chunk = os.read(r, 4096)
                if not chunk:
                    return
                got.append(chunk)

        def took_the_output():
            lock = gate._OUTPUT_LOCK
            try:
                took = lock.acquire(blocking=False)
            except AttributeError:  # no lock at all
                return True
            if took:
                lock.release()
            return bool(took)

        def blocked(fd, timeout):
            if threading.current_thread() is threading.main_thread() and not stuck.is_set():
                # The big line is half written and waits: no other thread takes the output.
                probe = threading.Thread(
                    target=lambda: seen.setdefault("took", took_the_output()), daemon=True)
                probe.start()
                probe.join(HANG_MARK_S)
                second.start()
                stuck.set()
            return wait(fd, timeout)

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        try:
            with mock.patch.object(gate, "_wait_writable", blocked):
                one.write(big)
                second.join(HANG_MARK_S)
        finally:
            stuck.set()
            _close_quietly(fa, w)
            _close_quietly(fb, w2)
            t.join(HANG_MARK_S)
            os.close(r)
        self.assertFalse(second.is_alive(), "the second line never went out")
        self.assertFalse(t.is_alive(), "the reader saw no EOF")
        self.assertIs(False, seen.get("took"), "another thread took the output mid-line")
        self.assertEqual(big + small, b"".join(got).decode("utf-8"))

    def test_a_reader_that_takes_no_byte_ends_in_a_named_error(self):
        """The hang marker: no endless wait on a pipe nobody reads (virtual clock)."""
        r, w = _full_pipe()
        clock = _VirtualClock(self)
        try:
            with clock.patched(), \
                    self.assertRaisesRegex(gate.OutputStalled, "took no byte") as cm:
                gate._write_all(w, b"x" * PIPE_BUF_BYTES)
            self.assertIsInstance(cm.exception, OSError)
            self.assertEqual(gate.OUTPUT_STALL_S, clock.now)  # the marker, not sooner
        finally:
            os.close(w)
            os.close(r)


class OutputStall(unittest.TestCase):
    """A reader that took no byte for OUTPUT_STALL_S: the pass ends ABORTED
    (exit 2, never RED), its reason is kept, and no line waits a second time."""

    def setUp(self):
        reset = getattr(gate, "_output_reset", None)
        if reset is not None:
            reset()
            self.addCleanup(reset)

    def test_a_stalled_gate_ends_aborted_with_its_reason_at_the_lease_and_waits_once(self):
        r, w = _full_pipe()
        w2 = os.dup(w)
        fa = open(w, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        fb = open(w2, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        clock = _VirtualClock(self)
        beats = []

        def run_gate(argv, out, err):
            try:
                print("GATE-PLAN strand abcdef0", file=out, flush=True)
            finally:  # the way out of a pass prints its close -- onto the same pipe
                print("gate: the pass of run-x ended without its close", file=err, flush=True)
                print("gate: and one more line", file=out, flush=True)
            return 0

        def beat(holder, text, state="progress", **_kw):
            beats.append((state, text))
            return True

        try:
            with clock.patched(), mock.patch.object(gate, "run_gate", run_gate), \
                    mock.patch.object(gate.progress, "beat", beat), \
                    mock.patch("sys.stdout", fa), mock.patch("sys.stderr", fb):
                rc = gate._cmd_gate(types.SimpleNamespace(argv=["strand"]))
        finally:
            _close_quietly(fa, w)
            _close_quietly(fb, w2)
            os.close(r)
        self.assertEqual(2, rc)  # ABORTED: no verdict on the commit, never RED (1)
        self.assertEqual(gate.OUTPUT_STALL_S, clock.now, "a line after the stall waited again")
        self.assertEqual(1, len(beats), beats)
        state, text = beats[0]
        self.assertEqual("aborted", state)
        self.assertRegex(text, r"^gate: ABORTED \(exit 2\) -- .*took no byte for %ds"
                         % gate.OUTPUT_STALL_S)

    def test_a_stall_in_the_stream_aborts_the_run_and_the_receipt_keeps_the_reason(self):
        scope = "workspace"
        doc = {"rev": REV, "stations": [{"name": "tests", "scope": scope}]}
        rows = [
            _row(1, "status", "station_ended",
                 {"station": "tests", "scope": scope, "verdict": "GREEN", "secs": 3, "exit": 0}),
            _row(2, "status", "pass_ended",
                 {"verdict": "GREEN", "secs": 3, "lane_seconds": 3, "critical_path_s": 3,
                  "rounds": 1, "cache_hits": 0, "stations": {"green": 1}}),
        ]
        proc = types.SimpleNamespace(
            stdout=io.StringIO("".join(json.dumps(x) + "\n" for x in rows)), wait=lambda: 0)
        r, w = _full_pipe()
        w2 = os.dup(w)
        fa = open(w, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        fb = open(w2, "w", encoding="utf-8")  # noqa: SIM115 -- closed below
        clock = _VirtualClock(self)
        aborts, ingested = [], []
        try:
            with tempfile.TemporaryDirectory() as tmp, clock.patched():
                o = types.SimpleNamespace(mode="strand", archive=tmp, cache="on")
                out, err = gate._WholeLines(fa), gate._WholeLines(fb)
                p = gate._Pass(o, tmp, doc, out, err)
                head = p.head(RUN, "local", 1)
                rc = gate._stream_to_end(p, proc, ingested.append, aborts.append, None)
                self.assertEqual(0, rc)
                self.assertEqual(["output stalled"], aborts)
                self.assertEqual(2, len(ingested), "the stream is read to its end")
                data = p.pass_ended["data"]
                self.assertEqual(("ABORTED", gate.OUTPUT_STALLED),
                                 (data["verdict"], data.get("reason")))
                v2 = p.v2(os.path.join(tmp, "no-dest"), head)
                with mock.patch.object(gate._Pass, "write_legacy"):
                    code = p.finish(v2, os.path.join(tmp, "no-dest"), "local", 1)
                self.assertEqual(2, code)
                with open(os.path.join(tmp, "latest", "receipt.json"), encoding="utf-8") as fh:
                    rec = json.load(fh)
                self.assertEqual("ABORTED", rec["verdict"])
                self.assertRegex(rec.get("aborted_reason") or "", "took no byte")
                with open(os.path.join(tmp, "latest", "summary.txt"), encoding="utf-8") as fh:
                    self.assertRegex(fh.read(), r"(?m)^gate: ABORTED -- .*took no byte")
        finally:
            _close_quietly(fa, w)
            _close_quietly(fb, w2)
            os.close(r)
        self.assertEqual(gate.OUTPUT_STALL_S, clock.now, "a line after the stall waited again")

    def test_the_store_notes_the_stall_at_the_lane_lease(self):
        from scripts.pipeline import schedule, store  # noqa: PLC0415 -- the isolated store
        db = store.open()
        schedule._ensure(db)
        try:
            gate._OUTPUT_STALL["why"] = "gate: the reader of fd took no byte for 600s"
            adm = types.SimpleNamespace(lane="build01", strand="d54g/gatepipe", wave="d54g",
                                        run=RUN)
            p = gate._Pass(types.SimpleNamespace(mode="strand", archive="", cache="on"),
                           str(REPO), {"rev": REV, "stations": []}, io.StringIO(), io.StringIO())
            gate._note_output_stalled(db, p, adm)
            noted = [json.loads(x[0]) for x in db.execute(
                "SELECT data FROM journal WHERE event = 'lease_noted'").fetchall()]
        finally:
            store.close(db)
        self.assertEqual([("build01", "d54g/gatepipe", RUN, gate.OUTPUT_STALLED)],
                         [(n["resource"], n["holder"], n["run"], n["verb"]) for n in noted])
        self.assertRegex(noted[0]["detail"], "took no byte")


if __name__ == "__main__":
    unittest.main()
