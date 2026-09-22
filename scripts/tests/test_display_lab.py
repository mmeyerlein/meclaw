"""Tests for the measuring library `workshop/tools/display-lab/`.

The library is the one copy of the tools that read a running screen. Before it
existed, every wave carried its own copy: 46 files and 19 774 lines between
waves B and H3, 60 % of them with a functional predecessor, and four copy
chains that drifted apart line by line (befund `04-struktur.md` § 8). A copy
also carries its own traps, and the two traps below cost two waves more than
an hour of agent time (befund `03-blocker.md` § 3.8).

What is pinned here is the CONTRACT of the library, not what any tool measures:

  * the inventory -- which files the library owes,
  * the head of every tool -- purpose, usage, output, BOTH traps, date,
  * the target is a parameter -- a call without `--port` refuses with exit 2
    instead of reaching into whichever colony the tool was born against,
  * no private host, path or person in a public surface.

The tools themselves are exercised against a fake colony (a local HTTP server
serving a fixture), never against a running one.
"""

import http.server
import json
import os
import pathlib
import re
import signal
import subprocess
import tempfile
import threading
import sys
import time
import unittest
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "display_lab_staterow.json"
LAB = REPO / "workshop" / "tools" / "display-lab"
CALLERS = LAB / "callers"


def setUpModule():
    # `workshop/` never travels with the export (R2c). This module does -- it
    # sits in ROOT_FILES because `scripts/tests/test_gate_plan.py` names it --
    # so in the published tree it must skip cleanly instead of failing on a
    # library that was never shipped.
    if not LAB.is_dir():
        raise unittest.SkipTest("workshop/tools/display-lab/ is not in this tree")

# The tools the library owes, with the interpreter each one is run by.
TOOLS = {
    "zeitraffer.mjs": "node",
    "runline.sh": "bash",
    "staterow.py": "python3",
    "turn.py": "python3",
    "tap.mjs": "node",
    "markers.py": "python3",
    "duplex_proof.mjs": "node",
}

# `callers/` holds TEMPLATES, not bound callers: a caller names one colony's
# port, root and member path, and none of those three belong in a public tree.
# The orchestrator substitutes the placeholders when it plants a caller.
PLACEHOLDERS = ("@LAB@", "@PORT@")

# Every head says the same six things, whatever the comment syntax around them.
HEAD_LINES = 60
CONTRACT = ("Purpose:", "Usage:", "Output:", "TRAP F1:", "TRAP F2:", "Since:")


def head_of(path):
    """The first `HEAD_LINES` lines of a file -- the head the contract lives in."""
    with path.open(encoding="utf-8") as fh:
        return "".join(line for _, line in zip(range(HEAD_LINES), fh))


class InventoryTest(unittest.TestCase):
    """The library owes six tools, a README and a caller template per tool."""

    def test_every_tool_is_there(self):
        missing = [name for name in TOOLS if not (LAB / name).is_file()]
        self.assertEqual([], missing, "missing in %s" % LAB)

    def test_every_tool_is_executable(self):
        """N7: every tool carries a shebang, so every tool may be run as one.

        Three of the six were 644 and three 755. The caller templates walk
        around that (`os.execv(sys.executable, ...)`, `spawnSync(process.execPath,
        ...)`) -- `callers/runline.sh` does not, it `exec`s the file.
        """
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                continue
            with self.subTest(tool=name):
                self.assertTrue(os.access(path, os.X_OK),
                                "%s has a shebang and mode %o" % (name, path.stat().st_mode & 0o777))

    def test_readme_is_there_and_short(self):
        """A page, not a manual: a row in the table and a line in the examples.

        The deckel is per TOOL, not a round number -- it moved from 60 to 64
        when wave Live added the seventh tool, and a tool that needs more than
        its row and its call belongs in its own head, where the contract test
        already reads it.
        """
        readme = LAB / "README.md"
        self.assertTrue(readme.is_file(), "no README.md in %s" % LAB)
        self.assertLessEqual(len(readme.read_text(encoding="utf-8").splitlines()),
                             42 + 3 * len(TOOLS))

    def test_every_tool_has_a_caller_template(self):
        missing = [name for name in TOOLS if not (CALLERS / name).is_file()]
        self.assertEqual([], missing, "missing in %s" % CALLERS)

    def test_caller_templates_are_short_and_unbound(self):
        for name in TOOLS:
            template = CALLERS / name
            if not template.is_file():
                continue
            with self.subTest(tool=name):
                body = [
                    line
                    for line in template.read_text(encoding="utf-8").splitlines()
                    if line.strip() and not line.lstrip().startswith(("#", "//", '"""'))
                ]
                self.assertLessEqual(len(body), 6, "a caller is three lines, not a fork")
                for mark in PLACEHOLDERS:
                    self.assertIn(mark, template.read_text(encoding="utf-8"))


class HeadContractTest(unittest.TestCase):
    """Purpose, usage, output, both traps and the date -- in every head."""

    def test_head_carries_the_six_lines(self):
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                self.fail("no %s -- the inventory test says why" % path)
            head = head_of(path)
            for mark in CONTRACT:
                with self.subTest(tool=name, mark=mark):
                    self.assertIn(mark, head)

    def test_trap_one_names_the_state_before_the_pass(self):
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                continue
            with self.subTest(tool=name):
                self.assertIn("display_views", head_of(path))

    def test_trap_two_names_the_rejected_write(self):
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                continue
            with self.subTest(tool=name):
                self.assertIn("rows_affected 0", head_of(path))


class PortIsMandatoryTest(unittest.TestCase):
    """A tool without `--port` refuses -- it does not fall back to a colony."""

    def run_tool(self, name, args):
        path = LAB / name
        if not path.is_file():
            self.fail("no %s -- the inventory test says why" % path)
        return subprocess.run(
            [TOOLS[name], str(path)] + args,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=str(REPO),
        )

    def test_a_call_without_port_exits_two_and_says_so(self):
        for name in TOOLS:
            with self.subTest(tool=name):
                done = self.run_tool(name, [])
                self.assertEqual(2, done.returncode, done.stderr[-400:])
                self.assertIn("--port", done.stdout + done.stderr)


def load_tool(name):
    """A tool of the library, imported as a module -- without leaving a `.pyc`."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("lab_%s" % name.replace(".", "_"),
                                                  LAB / name)
    module = importlib.util.module_from_spec(spec)
    # No `__pycache__` in the library: an import would otherwise leave a binary
    # in a public tree, and the surface test would find `/home/` inside it.
    written, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = written
    return module


class PublicSurfaceTest(unittest.TestCase):
    """`workshop/` is a public surface: no home directory, no named colony,
    no host of the owner's and no domain of theirs."""

    # A URL in the library points at loopback or at a placeholder. Everything
    # else names a machine, and a machine of the owner's is not public.
    HOST_IN_URL = re.compile(r"https?://([A-Za-z0-9._%<>-]+)")
    ALLOWED_HOSTS = {"127.0.0.1", "localhost", "host", "%s", "<host>"}
    DOMAIN = re.compile(r"\b[a-z0-9][a-z0-9-]*\.(?:eu|ai|com|net|de|org|io|dev|app)\b")

    def files(self):
        for path in sorted(LAB.rglob("*")):
            if path.is_file() and "__pycache__" not in path.parts:
                yield path

    def test_no_foreign_host_or_domain(self):
        for path in self.files():
            with self.subTest(file=path.name):
                text = path.read_text(encoding="utf-8", errors="replace")
                self.assertEqual(set(), set(self.HOST_IN_URL.findall(text)) - self.ALLOWED_HOSTS)
                self.assertEqual([], self.DOMAIN.findall(text))

    def test_no_private_path_or_colony_name(self):
        for path in sorted(LAB.rglob("*")):
            if not path.is_file() or "__pycache__" in path.parts:
                continue
            with self.subTest(file=path.name):
                text = path.read_text(encoding="utf-8", errors="replace")
                self.assertNotIn("/home/", text)
                self.assertNotIn("mm-os-e", text)



class FakeColony:
    """A colony's message log, served out of the fixture.

    Two state writes: one the store took (`rows_affected 1`) and one it
    rejected (`rows_affected 0`, F2). A reader that prints both the same way
    is printing attempts and calling them state.
    """

    def __init__(self, fixture):
        self.fixture = fixture
        self.requests = []
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802 -- http.server's spelling
                query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
                outer.requests.append(query)
                payload = json.dumps(outer.answer(query)).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        self.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]

    def answer(self, query):
        """The page `GET /colony/messages` would return for this query.

        Three filters, as the colony has them: `parent_message_id` (indexed,
        single write), the two path prefixes, and keyset paging through
        `before_id`. A fake that ignores paging cannot tell a reader that asks
        for one page from one that asks for four hundred rows.
        """
        parent = (query.get("parent_message_id") or [None])[0]
        if parent:
            rows = [row for row in self.fixture["answers"]["messages"]
                    if row.get("parent_message_id") == parent]
            return {"messages": rows, "next": None}
        which = "writes" if "compose" in (query.get("from_path_prefix") or [""])[0] else "answers"
        rows = self.fixture[which]["messages"]
        before = (query.get("before_id") or [None])[0]
        if before is not None:
            ids = [row["id"] for row in rows]
            rows = rows[ids.index(before) + 1:] if before in ids else []
        limit = int((query.get("limit") or ["100"])[0])
        page = rows[:limit]
        more = len(rows) > limit
        return {"messages": page,
                "next": ({"created_at": page[-1]["created_at"], "id": page[-1]["id"]}
                         if more and page else None)}

    def __enter__(self):
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=10)


class StaterowVerdictTest(unittest.TestCase):
    """F2, pinned: a rejected write is named, not printed as state."""

    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))

    def read_rows(self, extra=()):
        with FakeColony(self.fixture) as colony:
            done = subprocess.run(
                ["python3", str(LAB / "staterow.py"), "--port", str(colony.port),
                 "--root", "/nonexistent", "--member", self.fixture["member"],
                 "-n", "3"] + list(extra),
                capture_output=True, text=True, timeout=120,
            )
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return [line for line in done.stdout.splitlines() if line.startswith("== ")]

    def test_every_write_is_printed(self):
        self.assertEqual(3, len(self.read_rows()))

    def test_the_insert_path_bundle_is_not_a_verdict(self):
        """M2: the first write of a colony is a bundle, and a sum is not a verdict.

        `state_write_ops` sends `s-delete` + `s-insert` when there is no row yet
        (`templates/display/compose/compose.py`), the store answers
        `operation: "bundle"` with the SUM over the legs
        (`crates/meclaw-cells/src/store/output.rs`). Reading that sum as the
        answer to the state write happens to be right today and is wrong for the
        reason it is read.
        """
        bundles = [line for line in self.read_rows() if " bundle " in line]
        self.assertEqual(1, len(bundles), "exactly one write took the insert path")
        for word in ("LANDED", "REJECTED"):
            self.assertNotIn(word, bundles[0])

    def test_the_rejected_write_is_marked(self):
        rejected = [line for line in self.read_rows() if "rows_affected 0" in line]
        self.assertEqual(1, len(rejected), "exactly one write was refused by the store")
        self.assertIn("REJECTED", rejected[0])

    def test_the_landed_write_is_marked(self):
        landed = [line for line in self.read_rows() if "LANDED" in line]
        self.assertEqual(1, len(landed))
        self.assertNotIn("REJECTED", landed[0])

    def test_a_repeat_is_named(self):
        self.assertTrue(any("REPEAT 2" in line for line in self.read_rows()))


class RowsAffectedParityTest(unittest.TestCase):
    """M2: the reader counts rows the way the curator does, or it counts wrong.

    `templates/display/compose/compose.py` asks `results[]` for the leg of the
    operation first and the hop only when the hop IS that operation. Anything
    laxer reads a bundle's sum, or another leg's count, as the answer to the
    state write.
    """

    def setUp(self):
        self.staterow = load_tool("staterow.py")

    def rows(self, body, hop, operation="update"):
        return self.staterow.rows_affected_of(body, hop, operation)

    def test_the_result_list_wins_over_the_hop(self):
        body = {"results": [{"operation": "update", "rows_affected": 1}]}
        self.assertEqual(1, self.rows(body, {"operation": "update", "rows_affected": 0}))

    def test_the_hop_counts_only_for_its_own_operation(self):
        self.assertEqual(0, self.rows({}, {"operation": "update", "rows_affected": 0}))
        self.assertIsNone(self.rows({}, {"operation": "bundle", "rows_affected": 2}))

    def test_a_foreign_leg_does_not_answer_for_this_one(self):
        body = {"results": [{"operation": "delete", "rows_affected": 1}]}
        self.assertIsNone(self.rows(body, {"operation": "bundle", "rows_affected": 2}))

    def test_zero_is_a_value_and_no_answer_is_none(self):
        self.assertEqual(0, self.rows({"results": [{"operation": "update",
                                                    "rows_affected": 0}]}, {}))
        self.assertIsNone(self.rows({}, {}))


class StaterowFetchTest(unittest.TestCase):
    """M3: a row costs megabytes, so no single call asks for a big window.

    The state BEFORE the pass rides in `context.display_views` of every hop
    header (F1), so a message log row of a busy screen is not a few hundred
    bytes but hundreds of kilobytes: measured against a running screen, 0.62 MB
    per row in both directions, 5.3 MB and 21 s for a window of eight. The
    reader used to fetch TWO windows of 400 -- half a gigabyte to learn whether
    a chat window was open. Every call has to stay under 5 MB.
    """

    # 0.62 MB per row measured -> eight rows are 5.0 MB, the ceiling itself.
    PAGE_CEILING = 8

    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))

    def run_reader(self, extra=()):
        with FakeColony(self.fixture) as colony:
            done = subprocess.run(
                ["python3", str(LAB / "staterow.py"), "--port", str(colony.port),
                 "--root", "/nonexistent", "--member", self.fixture["member"]] + list(extra),
                capture_output=True, text=True, timeout=120,
            )
            self.assertEqual(0, done.returncode, done.stderr[-600:])
            return done, list(colony.requests)

    def test_no_call_asks_for_more_than_a_page(self):
        _, requests = self.run_reader()
        asked = [int((q.get("limit") or ["100"])[0]) for q in requests]
        self.assertTrue(asked, "the reader made no call at all")
        self.assertLessEqual(max(asked), self.PAGE_CEILING,
                             "a call of %s rows is megabytes, not rows" % max(asked))

    def test_the_answer_is_asked_for_by_parent_not_by_a_second_window(self):
        _, requests = self.run_reader()
        by_parent = [q for q in requests if q.get("parent_message_id")]
        self.assertTrue(by_parent, "the store's answer was not asked for by parent")
        second_window = [q for q in requests
                         if "views" in (q.get("from_path_prefix") or [""])[0]]
        self.assertEqual([], second_window, "a second prefix window is the old price")

    def test_it_pages_until_it_has_what_it_was_asked_for(self):
        """A small page is a page, not a ceiling on what the reader can find."""
        done, requests = self.run_reader(["-n", "2", "--page", "1"])
        self.assertEqual(2, len([line for line in done.stdout.splitlines()
                                 if line.startswith("== ")]))
        writes = [q for q in requests if "compose" in (q.get("from_path_prefix") or [""])[0]]
        self.assertGreater(len(writes), 1, "two writes in pages of one need two calls")


class CallerTemplateTest(unittest.TestCase):
    """A planted caller reaches the library and binds this colony's target."""

    def test_a_planted_caller_reads_the_fake_colony(self):
        fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
        template = (CALLERS / "staterow.py").read_text(encoding="utf-8")
        with FakeColony(fixture) as colony, tempfile.TemporaryDirectory() as tmp:
            planted = pathlib.Path(tmp) / "staterow.py"
            planted.write_text(
                template.replace("@LAB@", str(LAB))
                .replace("@PORT@", str(colony.port))
                .replace("@ROOT@", tmp)
                .replace("@MEMBER@", fixture["member"]),
                encoding="utf-8",
            )
            done = subprocess.run(["python3", str(planted), "-n", "2"],
                                  capture_output=True, text=True, timeout=120)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        self.assertIn("REJECTED", done.stdout)
        self.assertIn("LANDED", done.stdout)


def with_chat_open(write, value):
    """The same write, with the chat window's `open` flag set to `value`."""
    write = json.loads(json.dumps(write))
    body = json.loads(write["body_payload"])
    for leg in body["messages"]:
        if leg["id"] not in ("s-insert", "s-update"):
            continue
        op = json.loads(leg["text"])
        slot = "row" if "row" in op else "set"
        state = json.loads(op[slot]["content"])
        state["views"]["win.chat"]["curator"]["open"] = value
        op[slot]["content"] = json.dumps(state)
        leg["text"] = json.dumps(op)
    write["body_payload"] = json.dumps(body)
    return write


class RunlineChatStateTest(unittest.TestCase):
    """M1: the chat question is asked of a write that LANDED.

    `runline.sh` prepends a tap on the chat tile when a step types while the
    chat window is closed -- the tap toggles, so typing into a closed window
    types into nothing. It read the newest PRINTED write for that, and the head
    of the file says in the same breath that "the chat was open" is only true of
    a write that landed: 190 of 401 measured writes were compare-and-set
    refusals, a state the store never took. Here the newest write says open and
    was refused, and the newest LANDED write says closed.
    """

    TYPING = '[{"at": 0, "do": "type", "text": "hi"}]'

    def run_line(self, landed_open):
        fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
        writes = fixture["writes"]["messages"]
        writes[0] = with_chat_open(writes[0], not landed_open)   # newest, refused
        writes[1] = with_chat_open(writes[1], landed_open)       # the one that landed
        with FakeColony(fixture) as colony, tempfile.TemporaryDirectory() as tmp:
            done = subprocess.run(
                ["bash", str(LAB / "runline.sh"), "--dry-run",
                 "--port", str(colony.port), "--mount", "screen",
                 "--root", "/nonexistent", "--member", fixture["member"],
                 "--out-dir", tmp, "--line", "m1", "--steps", self.TYPING],
                capture_output=True, text=True, timeout=180,
            )
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return done.stdout

    def test_it_taps_when_the_landed_write_says_closed(self):
        out = self.run_line(landed_open=False)
        self.assertIn("chat was closed -> tap prepended", out)
        self.assertIn('"do": "tap"', out)

    def test_it_does_not_tap_when_the_landed_write_says_open(self):
        out = self.run_line(landed_open=True)
        self.assertNotIn("tap prepended", out)
        self.assertNotIn('"do": "tap"', out)


class StaterowLandedOnlyTest(unittest.TestCase):
    """`--landed-only`: print the writes the store took, and only those."""

    def read(self, extra=()):
        fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
        with FakeColony(fixture) as colony:
            done = subprocess.run(
                ["python3", str(LAB / "staterow.py"), "--port", str(colony.port),
                 "--root", "/nonexistent", "--member", fixture["member"]] + list(extra),
                capture_output=True, text=True, timeout=120,
            )
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return [line for line in done.stdout.splitlines() if line.startswith("== ")]

    def test_it_skips_what_the_store_refused(self):
        lines = self.read(["--landed-only", "-n", "3"])
        self.assertEqual(1, len(lines), "one of the three writes landed")
        self.assertIn("LANDED", lines[0])

    def test_without_it_the_newest_write_is_the_refused_one(self):
        self.assertIn("REJECTED", self.read(["-n", "1"])[0])


class OneLanguageTest(unittest.TestCase):
    """N4: the library is English -- including what it WRITES.

    `workshop/` does not travel, so no gate reads it (`GERMAN_SCAN_ROOTS`
    leaves it out on purpose) and the rule has to hold itself. The report
    `zeitraffer.mjs` writes was the one German surface of a library whose six
    heads, README and every other line are English; the next agent reads the
    report, not the head.
    """

    GERMAN = ("Kolonie", "Aufl\u00f6sung", "Herkunft", "Zeitleiste", "Ereignis",
              "Mutationen", "Schritte", "Dauer", "Seite", "Hinweise", "Mikrofon",
              "Zustand", "gesamt", "nichts bewegte")

    def test_no_german_in_the_library(self):
        for path in sorted(LAB.rglob("*")):
            if not path.is_file() or "__pycache__" in path.parts:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            found = [word for word in self.GERMAN if word in text]
            with self.subTest(file=path.name):
                self.assertEqual([], found)


class DisposableColonyTest(unittest.TestCase):
    """N2: which ports are disposable is a call's business, not the tree's.

    `markers.py --actions` writes into a colony, and it refuses to do that to a
    live one. The set it measured that against was a constant -- the ports one
    wave's plan handed out, a year of waves ago. A constant per colony is as
    wrong as a port per colony (OR-P.lab.3).
    """

    def setUp(self):
        self.markers = load_tool("markers.py")

    def test_the_disposable_ports_come_from_the_call(self):
        self.assertFalse(self.markers.is_live("http://127.0.0.1:7999", {7999}))
        self.assertTrue(self.markers.is_live("http://127.0.0.1:7999", {7960}))

    def test_anything_off_loopback_is_live_whatever_the_port_says(self):
        self.assertTrue(self.markers.is_live("http://10.0.0.1:7999", {7999}))


class QueueTest(unittest.TestCase):
    """`runline.sh --queue` answers the question agents used to poll for.

    In one wave, 48 of 112 commands touching the stage lock were pure polls,
    and nine runs waited a median of 9,4 minutes without anyone being able to
    say how long the queue was (befund `03-blocker.md` § 3.7). One reading,
    printed once: who holds the stage, who waits behind them, and since when.
    """

    def run_queue(self, lock):
        return subprocess.run(
            ["bash", str(LAB / "runline.sh"), "--queue", "--lock", str(lock)],
            capture_output=True, text=True, timeout=120,
        )

    def until(self, lock, mark, tries=100):
        """The queue reading once `mark` shows up in it, or `None`."""
        for _ in range(tries):
            out = self.run_queue(lock).stdout
            if mark in out:
                return out
            time.sleep(0.1)
        return None

    def test_queue_needs_no_port(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = pathlib.Path(tmp) / "stage.lock"
            lock.touch()
            done = self.run_queue(lock)
        self.assertEqual(0, done.returncode, done.stderr[-400:])

    def test_a_free_stage_says_free(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = pathlib.Path(tmp) / "stage.lock"
            lock.touch()
            done = self.run_queue(lock)
        self.assertIn("free", done.stdout)

    def test_a_busy_stage_names_the_holder_and_the_waiters(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = pathlib.Path(tmp) / "stage.lock"
            lock.touch()
            # Own session: `flock` execs `sleep`, and killing only the parent
            # would leave the fd -- and the lock -- with the orphan.
            holder = subprocess.Popen(["flock", str(lock), "sleep", "30"],
                                      start_new_session=True)
            waiter = None
            try:
                # The waiter starts only once the holder really holds: started
                # together, either of them may win the lock first.
                self.assertTrue(self.until(lock, "holder"), "the holder never took it")
                waiter = subprocess.Popen(["flock", "-w", "30", str(lock), "true"])
                out = self.until(lock, "wait 1") or self.run_queue(lock).stdout
                self.assertIn("holder", out)
                self.assertIn("wait 1", out)
                # position and AGE -- a queue without an age cannot be judged
                self.assertRegex(out, r"holder\s+pid \d+\s+age \d+s")
                self.assertRegex(out, r"wait 1\s+pid \d+\s+age \d+s")
            finally:
                os.killpg(os.getpgid(holder.pid), signal.SIGKILL)
                holder.wait(timeout=10)
                if waiter is not None:
                    waiter.wait(timeout=30)


if __name__ == "__main__":
    unittest.main()
