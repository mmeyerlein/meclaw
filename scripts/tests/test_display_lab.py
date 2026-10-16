"""Tests for the measuring library `workshop/tools/display-lab/`.

The library is the one copy of the tools that read a running screen. Before it
existed, every wave carried its own copy: 46 files and 19 774 lines between
waves B and H3, 60 % of them with a functional predecessor, and four copy
chains that drifted apart line by line (befund `04-struktur.md` § 8). A copy
also carries its own traps; the two traps of display 2.6.0 cost two waves more
than an hour of agent time (befund `03-blocker.md` § 3.8). Since display 2.7.0
the log holds no curator state at all, and ONE trap says so (GH #809).

What is pinned here is the CONTRACT of the library, not what any tool measures:

  * the inventory -- which files the library owes,
  * the head of every tool -- purpose, usage, output, the trap, date,
  * the target is a parameter -- a call without `--port` (`--db` for
    `headers.py`, which reads a file) refuses with exit 2 instead of reaching
    into whichever colony the tool was born against,
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
import sqlite3
import subprocess
import tempfile
import threading
import sys
import time
import unittest
import urllib.parse
from scripts.tests import kit_time, private_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "display_lab_patches.json"
LAB = REPO / "workshop" / "tools" / "display-lab"
CALLERS = LAB / "callers"


def setUpModule():
    # `workshop/` never travels with the export (R2c). This module does -- it
    # sits in ROOT_FILES because `scripts/tests/test_gate_plan.py` names it --
    # so in the published tree it must skip cleanly instead of failing on a
    # library that was never shipped.
    # `require` (GH #1199): a skip in the export tree, a failure in the private
    # tree, where the directory can not go quiet (re-check RC-K3 I3).
    private_tree.require("workshop/tools/display-lab")
    store_isolation.isolate()

# The tools the library owes, with the interpreter each one is run by.
TOOLS = {
    "zeitraffer.mjs": "node",
    "runline.sh": "bash",
    "headers.py": "python3",
    "turn.py": "python3",
    "tap.mjs": "node",
    "markers.py": "python3",
    "duplex_proof.mjs": "node",
    "worker_stalls.py": "python3",
    "identity_probe.py": "python3",
    "sidecar_quote.py": "python3",
    "fact_yield.py": "python3",
}

# `callers/` holds TEMPLATES, not bound callers: a caller names one colony's
# port, root and member path, and none of those three belong in a public tree.
# The orchestrator substitutes the placeholders when it plants a caller.
# `headers.py` reads a colony's database file, not its port.
# `worker_stalls.py` reads one process's `/proc`, not a port (GH #866).
# `identity_probe.py` reads one colony's tree, not its port (GH #872).
# `sidecar_quote.py` reads a colony's database and one brain's hops (GH #871).
# fact_yield.py reads a colony's database: the in_remember hops (GH #881).
PLACEHOLDERS = {"headers.py": ("@LAB@", "@DB@"), "worker_stalls.py": ("@LAB@", "@PID@"),
                "identity_probe.py": ("@LAB@", "@ROOT@"),
                "sidecar_quote.py": ("@LAB@", "@DB@", "@BRAIN@"),
                "fact_yield.py": ("@LAB@", "@DB@")}
DEFAULT_PLACEHOLDERS = ("@LAB@", "@PORT@")

# The one argument without which a tool refuses (exit 2); `--port` unless named.
MANDATORY = {"headers.py": "--db", "worker_stalls.py": "--pid", "identity_probe.py": "--root",
             "sidecar_quote.py": "--db", "fact_yield.py": "--db"}

# Every head says the same five things, whatever the comment syntax around them.
HEAD_LINES = 60
CONTRACT = ("Purpose:", "Usage:", "Output:", "TRAP F1:", "Since:")


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
                for mark in PLACEHOLDERS.get(name, DEFAULT_PLACEHOLDERS):
                    self.assertIn(mark, template.read_text(encoding="utf-8"))


class HeadContractTest(unittest.TestCase):
    """Purpose, usage, output, the trap and the date -- in every head."""

    def test_head_carries_the_five_lines(self):
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                self.fail("no %s -- the inventory test says why" % path)
            head = head_of(path)
            for mark in CONTRACT:
                with self.subTest(tool=name, mark=mark):
                    self.assertIn(mark, head)

    def test_the_trap_says_the_log_holds_no_curator_state(self):
        """F1 since display 2.7.0 (GH #809): the curator's state is memory. A
        tool that wants the screen folds the `patch` hops or asks the page, and
        `display_request` is the only marker a header carries."""
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                continue
            head = " ".join(head_of(path).split())
            with self.subTest(tool=name):
                for words in ("no curator state", "`patch` hops", "`display_request`"):
                    self.assertIn(words, head)

    def test_the_traps_of_the_state_row_are_gone(self):
        """F2 was the compare-and-set of a row that no longer exists."""
        for name in TOOLS:
            path = LAB / name
            if not path.is_file():
                continue
            with self.subTest(tool=name):
                self.assertNotIn("TRAP F2", head_of(path))
                self.assertNotIn("rows_affected 0", head_of(path))


class PortIsMandatoryTest(unittest.TestCase):
    """A tool without its target refuses -- it does not fall back to a colony."""

    def run_tool(self, name, args):
        path = LAB / name
        if not path.is_file():
            self.fail("no %s -- the inventory test says why" % path)
        return subprocess.run(
            [TOOLS[name], str(path)] + args,
            capture_output=True,
            text=True,
            timeout=4 * kit_time.HANG,
            cwd=str(REPO),
        )

    def test_a_call_without_port_exits_two_and_says_so(self):
        for name in TOOLS:
            with self.subTest(tool=name):
                done = self.run_tool(name, [])
                self.assertEqual(2, done.returncode, done.stderr[-400:])
                self.assertIn(MANDATORY.get(name, "--port"), done.stdout + done.stderr)


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

    `GET /colony/messages` with the filters a reader of this library uses: the
    two path prefixes and keyset paging through `before_id`. A fake that
    ignores paging cannot tell a reader that asks for one page from one that
    asks for four hundred rows.
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
        """The page `GET /colony/messages` would return for this query, newest first."""
        rows = self.fixture["messages"]
        for key, field in (("from_path_prefix", "from_path"), ("to_path_prefix", "to_path")):
            prefix = (query.get(key) or [""])[0]
            rows = [row for row in rows if row[field].startswith(prefix)]
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
        self.thread.join(timeout=kit_time.HANG)


def patch_row(fixture, n, calls, route="patch", to="web"):
    """One log row compose -> `to`, carrying `calls` as its tool-call turns.

    The shape compose sends since display 2.7.0 (`render` in
    `templates/display/compose/compose.py`): one `patch` hop per pass, each call
    one `messages[].text`, and nothing of the curator's state in the header.
    """
    member = fixture["member"]
    body = {"messages": [{"origin": "assistant", "type": "tool_call", "id": "d-%d" % i,
                          "text": json.dumps(call, sort_keys=True)}
                         for i, call in enumerate(calls)]}
    return {"id": "01999999-0000-7000-8000-%012d" % n, "created_at": 1758200000 + n,
            "trace_id": "t-%d" % n,
            "from_path": member + "/channels/display/compose",
            "to_path": member + "/channels/display/" + to,
            "headers_json": json.dumps({"hop": {"route": route}}),
            "body_kind": "inline", "body_payload": json.dumps(body)}


def chat_tile(fixture, open_, op="object.update"):
    """A call on the chat window's dock tile, as `_tile` in compose.py writes it."""
    oid = "view.%s.chat" % fixture["member"].replace("/", "~")
    call = {"op": op, "id": "display.dock/tile." + oid}
    if op == "object.create":
        call.update(parent="display.dock", ord=0, component="display-tile")
    if op != "object.delete":
        call["props"] = {"open": "1" if open_ else "", "rung": "focus" if open_ else "ambient",
                         "oid": oid}
    return call


def with_log(fixture, rows):
    """The fixture, its log replaced by `rows` (given oldest first, served newest first)."""
    fixture = json.loads(json.dumps(fixture))
    fixture["messages"] = list(reversed(rows))
    return fixture


class RunlineChatStateTest(unittest.TestCase):
    """The chat question is asked of the screen the patches built (display 2.7.0).

    `runline.sh` prepends a tap on the chat tile when a step types while the
    chat window is closed -- the tap toggles, so typing into a closed window
    types into nothing. Since display 2.7.0 the curator keeps its state in
    memory; the log carries no state row and no plan in any header (GH #809).
    What it does carry is every `patch` hop compose sent to `./web`, and the
    dock tile of a window says `open` in its props on every create and update.
    """

    TYPING = '[{"at": 0, "do": "type", "text": "hi"}]'

    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))

    def run_line(self, fixture, steps=None):
        with FakeColony(fixture) as colony, tempfile.TemporaryDirectory() as tmp:
            done = subprocess.run(
                ["bash", str(LAB / "runline.sh"), "--dry-run",
                 "--port", str(colony.port), "--mount", "screen",
                 "--root", "/nonexistent", "--member", fixture["member"],
                 "--out-dir", tmp, "--line", "m1", "--steps", steps or self.TYPING],
                capture_output=True, text=True, timeout=6 * kit_time.HANG,
            )
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return done.stdout

    def test_the_fixture_opens_the_chat_last(self):
        """The shipped fixture: created closed, opened by a later patch."""
        out = self.run_line(self.fixture)
        self.assertNotIn("tap prepended", out)

    def test_it_taps_when_the_newest_patch_says_closed(self):
        f = self.fixture
        out = self.run_line(with_log(f, [patch_row(f, 1, [chat_tile(f, True, "object.create")]),
                                         patch_row(f, 2, [chat_tile(f, False)])]))
        self.assertIn("chat was closed -> tap prepended", out)
        self.assertIn('"do": "tap"', out)

    def test_it_does_not_tap_when_the_newest_patch_says_open(self):
        f = self.fixture
        out = self.run_line(with_log(f, [patch_row(f, 1, [chat_tile(f, False, "object.create")]),
                                         patch_row(f, 2, [chat_tile(f, True)])]))
        self.assertNotIn("tap prepended", out)
        self.assertNotIn('"do": "tap"', out)

    def test_a_deleted_tile_is_a_closed_chat(self):
        f = self.fixture
        out = self.run_line(with_log(f, [patch_row(f, 1, [chat_tile(f, True, "object.create")]),
                                         patch_row(f, 2, [chat_tile(f, None, "object.delete")])]))
        self.assertIn("tap prepended", out)

    def test_the_last_call_of_one_patch_wins(self):
        f = self.fixture
        out = self.run_line(with_log(f, [patch_row(f, 1, [chat_tile(f, False, "object.create"),
                                                          chat_tile(f, True)])]))
        self.assertNotIn("tap prepended", out)

    def test_only_patch_hops_to_web_count(self):
        """A `read` to web and a hop to another cell carry no screen."""
        f = self.fixture
        out = self.run_line(with_log(f, [patch_row(f, 1, [chat_tile(f, False, "object.create")]),
                                         patch_row(f, 2, [chat_tile(f, True)], route="read"),
                                         patch_row(f, 3, [chat_tile(f, True)], to="views")]))
        self.assertIn("tap prepended", out)

    def test_no_answer_taps(self):
        """No chat tile in the scanned window: a tap on an open window costs a
        step, typing into a closed one types into nothing."""
        out = self.run_line(with_log(self.fixture, []))
        self.assertIn("tap prepended", out)

    def test_steps_that_do_not_type_ask_nothing(self):
        with FakeColony(self.fixture) as colony, tempfile.TemporaryDirectory() as tmp:
            done = subprocess.run(
                ["bash", str(LAB / "runline.sh"), "--dry-run", "--port", str(colony.port),
                 "--mount", "screen", "--root", "/nonexistent", "--member",
                 self.fixture["member"], "--out-dir", tmp, "--steps", '[{"at":0,"do":"press"}]'],
                capture_output=True, text=True, timeout=6 * kit_time.HANG)
            self.assertEqual(0, done.returncode, done.stderr[-600:])
            self.assertEqual([], colony.requests)


class PatchFoldTest(unittest.TestCase):
    """`patches.mjs` folds the calls the way `support::apply` does
    (`crates/meclaw-cells/tests/support/mod.rs`): create appends, update merges
    props (and a non-null parent), move sets parent and ord, delete drops."""

    def node(self, script):
        done = subprocess.run(
            ["node", "--input-type=module", "-e",
             "import * as p from %s;\n%s" % (json.dumps((LAB / "patches.mjs").as_uri()), script)],
            capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_the_fold_follows_support_apply(self):
        calls = [
            {"op": "component.define", "name": "x"},
            {"op": "object.create", "id": "a", "parent": None, "ord": 0,
             "component": "c", "props": {"k": "1", "j": "2"}},
            {"op": "object.create", "id": "b", "parent": "a", "ord": 1,
             "component": "c", "props": {}},
            {"op": "object.update", "id": "a", "props": {"k": "3"}},
            {"op": "object.move", "id": "b", "parent": "z", "ord": 5},
            {"op": "object.create", "id": "gone", "parent": "a", "ord": 2,
             "component": "c", "props": {}},
            {"op": "object.delete", "id": "gone"},
        ]
        held = self.node("const h = p.fold([], %s); console.log(JSON.stringify(h));"
                         % json.dumps(calls))
        self.assertEqual(
            [{"id": "a", "parent": None, "ord": 0, "component": "c", "props": {"k": "3", "j": "2"}},
             {"id": "b", "parent": "z", "ord": 5, "component": "c", "props": {}}],
            held)

    def test_calls_come_only_out_of_patch_hops_to_web(self):
        f = json.loads(FIXTURE.read_text(encoding="utf-8"))
        rows = [patch_row(f, 1, [chat_tile(f, True)]),
                patch_row(f, 2, [chat_tile(f, True)], route="read"),
                patch_row(f, 3, [chat_tile(f, True)], to="views")]
        got = self.node("console.log(JSON.stringify(%s.map((r) => p.callsOf(r, (x) => JSON.parse(x.body_payload)).length)));"
                        % json.dumps(rows))
        self.assertEqual([1, 0, 0], got)

    def test_tiles_name_the_window_and_its_open_flag(self):
        f = json.loads(FIXTURE.read_text(encoding="utf-8"))
        got = self.node("console.log(JSON.stringify(p.tilesOf(%s)));"
                        % json.dumps([chat_tile(f, True), {"op": "object.update", "id": "display.root",
                                                           "props": {"open": "1"}}]))
        self.assertEqual([{"win": "chat", "open": 1, "rung": "focus", "op": "object.update"}], got)


class TapCountTest(unittest.TestCase):
    """`tap.mjs` counts a tap where it arrives: an `event` hop to
    `.../channels/display/compose` whose body names the event `tap`.

    Until 2026-09-23 it read the tap out of `hop.display_request.tap`, a mark
    display 2.7.0 no longer sets (GH #809) -- measured on a candidate colony:
    the tap arrived as `@external -> .../channels/display/compose`, route
    `event`, body `{"event": {"name": "tap", "value": {"for": <oid>}}}`, and the
    probe printed `taps=0`.
    """

    COMPOSE = "/org/members/m/channels/display/compose"

    def row(self, n, route="event", to=None, name="tap", kind="inline"):
        body = {"event": {"name": name, "value": {"for": "view.chat"}}, "context": {}}
        return {
            "id": "row-%08d" % n,
            "created_at": "2026-09-23T06:20:%02d.000Z" % n,
            "from_path": "@external",
            "to_path": to or self.COMPOSE,
            "headers_json": json.dumps({"hop": {"route": route}}),
            "body_kind": kind,
            "body_payload": ("blob-%d" % n) if kind == "blob" else json.dumps(body),
        }, body

    def taps(self, rows, blobs=None):
        script = (
            "import * as t from %s;\n"
            "const blobs = %s;\n"
            "const bodyOf = (r) => r.body_kind === 'blob' ? (blobs[r.body_payload] ?? null)"
            " : JSON.parse(r.body_payload);\n"
            "console.log(JSON.stringify(t.tapsOf(%s, bodyOf)));"
            % (json.dumps((LAB / "tap.mjs").as_uri()), json.dumps(blobs or {}), json.dumps(rows)))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_a_tap_event_to_compose_counts(self):
        row, _ = self.row(1)
        got = self.taps([row])
        self.assertEqual([{"id": "00000001", "at": row["created_at"], "from": "@external",
                           "tap": "view.chat"}], got)

    def test_only_tap_events_to_compose_count(self):
        rows = [self.row(1)[0],
                self.row(2, route="patch")[0],
                self.row(3, to="/org/members/m/channels/display/web")[0],
                self.row(4, name="hold")[0]]
        self.assertEqual(["00000001"], [t["id"] for t in self.taps(rows)])

    def test_a_blob_body_counts_too(self):
        row, body = self.row(5, kind="blob")
        self.assertEqual(["00000005"], [t["id"] for t in self.taps([row], {"blob-5": body})])

    def test_an_unreadable_body_is_no_tap(self):
        row, _ = self.row(6, kind="blob")
        self.assertEqual([], self.taps([row]))


class HeadersTest(unittest.TestCase):
    """`headers.py` measures the hop headers of the display in `message_log`.

    Since display 2.7.0 no header carries a plan (GH #809): `display_request`
    is the one marker and it is small. The reading is the SQL of OR-D17 over
    the youngest `--last` rows; the lids are `max > 8192`, `avg >= 2048` and
    any row that still says `display_views`.
    """

    SCHEMA = ("CREATE TABLE message_log (id TEXT PRIMARY KEY, trace_id TEXT NOT NULL, "
              "parent_message_id TEXT NULL, correlation_id TEXT NULL, ttl INTEGER NOT NULL, "
              "from_path TEXT NOT NULL, to_path TEXT NOT NULL, reply_to TEXT NULL, "
              "headers TEXT NOT NULL, body_kind TEXT NOT NULL, body_payload TEXT NULL, "
              "created_at INTEGER NOT NULL)")

    def db(self, tmp, rows):
        """A mini colony.db: `rows` = (created_at, from, to, headers), oldest first."""
        import sqlite3
        path = pathlib.Path(tmp) / "colony.db"
        con = sqlite3.connect(str(path))
        con.execute(self.SCHEMA)
        for n, (at, frm, to, headers) in enumerate(rows):
            con.execute("INSERT INTO message_log VALUES (?, 't', NULL, NULL, 8, ?, ?, NULL, ?, "
                        "'inline', '{}', ?)", ("m-%05d" % n, frm, to, headers, at))
        con.commit()
        con.close()
        return path

    def run_tool(self, args):
        return subprocess.run(["python3", str(LAB / "headers.py")] + args,
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)

    @staticmethod
    def hdr(size, extra=""):
        base = json.dumps({"context": {"display_request": '{"rest": true}', "x": extra}})
        return base + " " * max(0, size - len(base))

    COMPOSE = "/os/m/channels/display/compose"
    WEB = "/os/m/channels/display/web"

    def measure(self, rows, extra=()):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.db(tmp, rows)
            before = path.read_bytes()
            done = self.run_tool(["--db", str(path)] + list(extra))
            self.assertEqual(before, path.read_bytes(), "a reader writes nothing")
        return done

    def numbers(self, done):
        m = re.search(r"max (\d+) avg (\d+) n (\d+) display_views_hits (\d+)", done.stdout)
        self.assertTrue(m, done.stdout + done.stderr)
        return tuple(int(x) for x in m.groups())

    def test_small_headers_pass(self):
        done = self.measure([(i, self.COMPOSE, self.WEB, self.hdr(300)) for i in range(10)])
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertEqual((300, 300, 10, 0), self.numbers(done))

    def test_one_header_over_8192_fails(self):
        rows = [(i, self.COMPOSE, self.WEB, self.hdr(200)) for i in range(50)]
        rows.append((99, self.WEB, self.COMPOSE, self.hdr(8193)))
        done = self.measure(rows)
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(8193, self.numbers(done)[0])

    def test_an_average_of_2048_fails(self):
        done = self.measure([(i, self.COMPOSE, self.WEB, self.hdr(2048)) for i in range(4)])
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual((2048, 2048, 4, 0), self.numbers(done))

    def test_a_plan_in_a_header_fails(self):
        rows = [(1, self.COMPOSE, self.WEB, self.hdr(300)),
                (2, self.COMPOSE, self.WEB, self.hdr(300, extra="display_views"))]
        done = self.measure(rows)
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(1, self.numbers(done)[3])

    def test_only_the_youngest_rows_and_only_display_rows_count(self):
        rows = [(1, self.COMPOSE, self.WEB, self.hdr(9000, extra="display_views"))]
        rows += [(10 + i, self.COMPOSE, self.WEB, self.hdr(400)) for i in range(5)]
        rows += [(100, "/os/m/apps/chat", "/os/m/brain", self.hdr(20000))]
        done = self.measure(rows, ["--last", "5"])
        self.assertEqual(0, done.returncode, done.stdout)
        self.assertEqual((400, 400, 5, 0), self.numbers(done))

    def test_without_db_it_exits_two_and_names_the_flag(self):
        done = self.run_tool([])
        self.assertEqual(2, done.returncode)
        self.assertIn("--db", done.stdout + done.stderr)

    def test_a_missing_file_is_not_created(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "none.db"
            done = self.run_tool(["--db", str(path)])
            self.assertFalse(path.exists(), "mode=ro opens, it never creates")
        self.assertEqual(2, done.returncode, done.stdout + done.stderr)


class CallerTemplateTest(unittest.TestCase):
    """A planted caller reaches the library and binds this colony's target."""

    def test_a_planted_headers_caller_reads_the_database(self):
        template = (CALLERS / "headers.py").read_text(encoding="utf-8")
        with tempfile.TemporaryDirectory() as tmp:
            path = HeadersTest.db(HeadersTest, tmp, [
                (1, HeadersTest.COMPOSE, HeadersTest.WEB, HeadersTest.hdr(100))])
            planted = pathlib.Path(tmp) / "headers.py"
            planted.write_text(template.replace("@LAB@", str(LAB)).replace("@DB@", str(path)),
                               encoding="utf-8")
            done = subprocess.run(["python3", str(planted), "--last", "10"],
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        self.assertIn("display_views_hits 0", done.stdout)


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


class OldClassesTest(unittest.TestCase):
    """E7 counts the card classes of the time before the display DNA.

    `colony-view` stood on that list as the class of an old card. Since
    colony-view 1.1.3 (GH #808) the colony view is a real window on the screen,
    and every component it draws is named `colony-view-...`: counted as an old
    class, E7 went red on every colony that runs the app."""

    def setUp(self):
        self.markers = load_tool("markers.py")

    def test_a_colony_view_window_is_no_old_card(self):
        markup = ('<section class="display-pane" data-rung="focus">'
                  '<div class="colony-view-shell"><svg class="colony-view-hive"></svg>'
                  '</div></section>')
        self.assertEqual({}, {k: v for k, v in self.markers.old_classes_in(markup).items() if v})

    def test_an_old_card_still_counts(self):
        self.assertEqual(1, self.markers.old_classes_in('<div class="v2v-card">x</div>')["v2v-card"])


class WeightLidTest(unittest.TestCase):
    """GH #738: E17/E25 hold a page against the lid of the target they measure.

    The first lid was set on a throwaway colony and every shipped instance
    since display 2.4.0 weighed more than it; the marker now picks by target,
    exactly as `is_live` picks whether `--actions` may write."""

    def setUp(self):
        self.markers = load_tool("markers.py")

    def test_a_disposable_colony_keeps_the_wave_f_lid(self):
        self.assertEqual(self.markers.weight_lids("http://127.0.0.1:7999", {7999}),
                         (130_000, 38_000, "disposable"))

    def test_a_shipped_instance_gets_the_instance_lid(self):
        self.assertEqual(self.markers.weight_lids("http://127.0.0.1:7999", {7960}),
                         (170_000, 52_000, "instance"))
        self.assertEqual(self.markers.weight_lids("https://example.invalid/screen", {7999})[2],
                         "instance")

    def test_the_instance_lid_clears_every_page_the_issue_measured(self):
        measured = ((135_209, 39_682), (161_635, 48_720), (163_173, 48_995), (143_599, 40_124))
        for raw, gz in measured:
            self.assertLess(raw, self.markers.INSTANCE_RAW_LID)
            self.assertLess(gz, self.markers.INSTANCE_GZ_LID)
        self.assertGreater(self.markers.INSTANCE_RAW_LID, self.markers.RAW_LID)
        self.assertGreater(self.markers.INSTANCE_GZ_LID, self.markers.GZ_LID)


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
            capture_output=True, text=True, timeout=4 * kit_time.HANG,
        )

    def until(self, lock, mark):
        """The queue reading once `mark` shows up in it; past HANG a `HANG:`
        line (`kit_time.until`, re-check tv-bn M7j B-M1: an own poll of
        100 x 0.1 s, unscaled and silent)."""
        last = []

        def seen():
            last[:] = [self.run_queue(lock).stdout]
            return last[0] if mark in last[0] else None
        return kit_time.until("the queue shows %r" % mark, seen, state=lambda: last[0] if last else None)

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
            holder = subprocess.Popen(["flock", str(lock), "sleep", "infinity"],   # held until killed
                                      start_new_session=True)
            waiter = None
            try:
                # The waiter starts only once the holder really holds: started
                # together, either of them may win the lock first.
                self.assertTrue(self.until(lock, "holder"), "the holder never took it")
                waiter = subprocess.Popen(["flock", "-w", str(int(kit_time.HANG)), str(lock), "true"])
                out = self.until(lock, "wait 1") or self.run_queue(lock).stdout
                self.assertIn("holder", out)
                self.assertIn("wait 1", out)
                # position and AGE -- a queue without an age cannot be judged
                self.assertRegex(out, r"holder\s+pid \d+\s+age \d+s")
                self.assertRegex(out, r"wait 1\s+pid \d+\s+age \d+s")
            finally:
                os.killpg(os.getpgid(holder.pid), signal.SIGKILL)
                holder.wait(timeout=kit_time.HANG)
                if waiter is not None:
                    waiter.wait(timeout=kit_time.HANG)


class WorkerStallsTest(unittest.TestCase):
    """`worker_stalls.py` samples one process's threads out of `/proc` (GH #866).

    A `colony_loop` trip with `witness=kept` and no supervisor lag says the
    colony task was runnable and not polled for 500 ms. The tool shows which
    runtime worker stood in which kernel wait channel while that happened. It
    is exercised against a fixture `/proc` (`--proc-root`), never against a
    running colony: the contract is what it reads, what it calls a core
    worker, how it classes a wait channel, and how it joins a trip.
    """

    PID = 4242
    # tid -> (comm, state, starttime, wchan). Four workers started with the
    # runtime; 4250 is a blocking-pool thread of the same name, born later.
    THREADS = {
        4242: ("meclaw", "S", 100, "futex_wait_queue"),
        4243: ("tokio-rt-worker", "S", 101, "futex_wait_queue"),
        4244: ("tokio-rt-worker", "D", 101, "jbd2_log_wait_commit"),
        4245: ("tokio-rt-worker", "S", 101, "ep_poll"),
        4246: ("tokio-rt-worker", "R", 101, "0"),
        4250: ("tokio-rt-worker", "S", 900, "pipe_read"),
    }

    def setUp(self):
        self.tool = load_tool("worker_stalls.py")

    def proc(self, root):
        for tid, (comm, state, start, wchan) in self.THREADS.items():
            task = pathlib.Path(root) / str(self.PID) / "task" / str(tid)
            task.mkdir(parents=True)
            rest = [state] + ["0"] * 18 + [str(start)] + ["0"] * 5
            (task / "stat").write_text("%d (%s) %s\n" % (tid, comm, " ".join(rest)))
            (task / "wchan").write_text(wchan)
        (pathlib.Path(root) / "pressure").mkdir()
        (pathlib.Path(root) / "pressure" / "io").write_text(
            "some avg10=1.50 avg60=0.20 avg300=0.00 total=1\n"
            "full avg10=1.25 avg60=0.10 avg300=0.00 total=1\n")
        (pathlib.Path(root) / "meminfo").write_text("MemTotal: 1 kB\nDirty:  840000 kB\n")
        fds = pathlib.Path(root) / str(self.PID) / "fd"
        fds.mkdir()
        os.symlink("/srv/colony/main/board/cell.db-wal", str(fds / "7"))
        os.symlink("/srv/colony/log.txt", str(fds / "8"))

    def sample(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.proc(tmp)
            before = sorted((p, p.read_bytes()) for p in pathlib.Path(tmp).rglob("*") if p.is_file())
            done = subprocess.run(
                ["python3", str(LAB / "worker_stalls.py"), "--pid", str(self.PID),
                 "--proc-root", tmp, "--minutes", "1", "--no-align",
                 "--window-before", "0", "--window-after", "0.05"],
                capture_output=True, text=True, timeout=2 * kit_time.HANG)
            after = sorted((p, p.read_bytes()) for p in pathlib.Path(tmp).rglob("*") if p.is_file())
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        self.assertEqual(before, after, "a reader writes nothing into /proc")
        lines = done.stdout.splitlines()
        self.assertEqual(1, len(lines), done.stdout)
        return json.loads(lines[0])

    def test_one_window_is_one_line_of_the_documented_shape(self):
        row = self.sample()
        self.assertEqual(self.PID, row["pid"])
        self.assertRegex(row["minute"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual({"avg10": 1.25, "avg60": 0.1, "avg300": 0.0}, row["psi_io"]["full"])
        self.assertEqual(840000, row["dirty_kb"])
        self.assertEqual([], row["fd_events"], "an unchanged set of files is no event")
        for th in row["threads"]:
            for run in th["runs"]:
                self.assertEqual({"wchan", "state", "from_ms", "to_ms"}, set(run))
                self.assertLessEqual(run["from_ms"], run["to_ms"])

    def test_core_workers_are_the_four_that_started_first(self):
        core = {th["tid"]: th["core"] for th in self.sample()["threads"]}
        self.assertEqual({4242: False, 4243: True, 4244: True, 4245: True, 4246: True,
                          4250: False}, core)

    def test_a_worker_in_a_journal_fsync_is_read_as_such(self):
        runs = {th["tid"]: th["runs"] for th in self.sample()["threads"]}
        self.assertEqual([("D", "jbd2_log_wait_commit")],
                         [(r["state"], r["wchan"]) for r in runs[4244]])

    def test_the_classes_of_the_plan(self):
        cases = {("S", "futex_wait_queue"): "idle", ("S", "ep_poll"): "idle",
                 ("S", "do_epoll_wait"): "idle", ("S", "hrtimer_nanosleep"): "idle",
                 ("D", "jbd2_log_wait_commit"): "H1-fsync",
                 ("D", "file_write_and_wait_range"): "H1-fsync",
                 ("D", "folio_wait_writeback"): "H1-fsync",
                 ("S", "pipe_read"): "H1-exec", ("S", "anon_pipe_read"): "H1-exec",
                 ("D", "balance_dirty_pages"): "H1-dirty",
                 ("D", "filemap_fault"): "H1-fault",
                 ("R", "0"): "H1-cpu", ("S", "0"): "unknown", ("D", "0"): "unknown"}
        for (state, wchan), want in cases.items():
            with self.subTest(state=state, wchan=wchan):
                self.assertEqual(want, self.tool.classify(state, wchan))

    def test_marked_files_are_named_by_their_last_two_parts(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.proc(tmp)
            self.assertEqual({7: "board/cell.db-wal"}, self.tool.read_fds(tmp, self.PID))

    def test_a_database_that_closes_is_an_event(self):
        """A store that falls asleep closes its `cell.db`; the close is the
        moment its connection checkpoints, and a trip beside it names it."""
        snaps = [(0, {7: "board/cell.db-wal", 9: "x/orphan-journal.jsonl"}),
                 (50, {7: "board/cell.db-wal"}),
                 (100, {})]
        self.assertEqual([{"ms": 50, "op": "close", "fd": 9, "file": "x/orphan-journal.jsonl"},
                          {"ms": 100, "op": "close", "fd": 7, "file": "board/cell.db-wal"}],
                         self.tool.fd_events(snaps))

    def test_runs_fold_until_the_reading_changes(self):
        a = {1: ("S", "tokio-rt-worker", 1, "futex_wait_queue")}
        b = {1: ("D", "tokio-rt-worker", 1, "jbd2_log_wait_commit")}
        runs = self.tool.fold([(0, a), (10, b), (20, b), (30, b), (40, a)])[1][1]
        self.assertEqual([("futex_wait_queue", 0, 10), ("jbd2_log_wait_commit", 10, 40),
                          ("futex_wait_queue", 40, 40)],
                         [(r["wchan"], r["from_ms"], r["to_ms"]) for r in runs])

    TRIP = ("2026-09-26T20:00:00.690000Z ERROR meclaw_cli: watchdog trip reason=colony "
            "heartbeat lost for 5 consecutive supervisor periods of 100 ms [starved=colony_loop "
            "silent_for=500ms nominal_window=500ms supervisor_lag=0ms in_flight_work=false "
            "witness=kept armed_for=4131639ms work_item=none] on_trip=exit "
            'starved="colony_loop" fatal=true work_item="none"')

    def window_row(self, minute, block):
        core = [{"tid": t, "comm": "tokio-rt-worker", "core": True,
                 "runs": [{"wchan": "futex_wait_queue", "state": "S", "from_ms": -400,
                           "to_ms": 1600}]} for t in (11, 12, 13)]
        core.append({"tid": 14, "comm": "tokio-rt-worker", "core": True,
                     "runs": [{"wchan": "futex_wait_queue", "state": "S", "from_ms": -400,
                               "to_ms": block[0]},
                              {"wchan": block[2], "state": "D", "from_ms": block[0],
                               "to_ms": block[1]},
                              {"wchan": "futex_wait_queue", "state": "S", "from_ms": block[1],
                               "to_ms": 1600}]})
        return {"minute": minute, "pid": 7, "samples": 200, "threads": core,
                "psi_io": {"full": {"avg10": 2.5}}, "dirty_kb": 840000}

    def report(self, rows, trips):
        with tempfile.TemporaryDirectory() as tmp:
            jsonl = pathlib.Path(tmp) / "m1.jsonl"
            jsonl.write_text("".join(json.dumps(r) + "\n" for r in rows))
            journal = pathlib.Path(tmp) / "trips.raw"
            journal.write_text("".join(t + "\n" for t in trips))
            done = subprocess.run(["python3", str(LAB / "worker_stalls.py"), "--report",
                                   str(jsonl), "--journal", str(journal)],
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return done.stdout

    def test_a_trip_is_joined_with_the_worker_that_blocked(self):
        out = self.report([self.window_row("2026-09-26T20:00:00Z", (20, 780, "jbd2_log_wait_commit")),
                           self.window_row("2026-09-26T20:01:00Z", (30, 530, "pipe_read"))],
                          [self.TRIP])
        self.assertIn("colony_loop True 7 14 H1-fsync jbd2_log_wait_commit D 20 780 760", out)
        self.assertIn("trips_matched 1 h1_block_ge_400ms_from_0_600 1 free_workers 0", out)
        self.assertIn("blocked_windows_without_trip 1/1", out)
        self.assertIn("H1 confirmed=True", out)

    def test_free_workers_during_a_trip_refute(self):
        out = self.report([self.window_row("2026-09-26T20:00:00Z", (900, 950, "pipe_read"))],
                          [self.TRIP])
        self.assertIn("free_workers 1", out)
        self.assertIn("refuted=True", out)

    def test_it_never_signals_traces_or_opens_a_database(self):
        text = (LAB / "worker_stalls.py").read_text(encoding="utf-8")
        for word in ("os.kill", "import signal", "ptrace", "sqlite3", "strace", "gdb"):
            with self.subTest(word=word):
                self.assertNotIn(word, text)

    def test_report_without_journal_exits_two(self):
        done = subprocess.run(["python3", str(LAB / "worker_stalls.py"), "--report", "x.jsonl"],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(2, done.returncode)
        self.assertIn("--journal", done.stderr)


class IdentityProbeTest(unittest.TestCase):
    """`identity_probe.py` says whether an agent's identity reached its brains (GH #872).

    A member reborn from a seed that kept its source's `pack_hash` pushed 2 486
    ticks and not one `in_pack`; three brains came up without `identity.*`. The
    probe reads the colony's own files, read-only: the packs and their receipts
    in `message_log`, the slots in each brain's `system` table and the
    subscription row in the affinity store. It prints paths, slot names,
    lengths and counts -- never a slot's text. It is exercised against a fixture
    tree, never against a running colony.
    """

    MEMBER = "/os/orgs/o/members/m"
    ASSISTANT = "a"
    BRAINS = ("talky", "talky-chat", "cogny")
    SLOTS = ("identity.soul", "instructions.reply")
    SECRET = "the soul text of the fixture agent, never to be printed"
    BIRTH = 1000

    def tree(self, tmp, packs=None, acks=None, slot_len=None, extra_brain_db=False,
             strays=False, nested_brain=False):
        """A colony root: `colony.db`, three brain `cell.db`s, one affinity store.

        `packs` / `acks`: brain -> list of (created_at, error_code) rows; default
        one clean pack at BIRTH+30 and one clean ack at BIRTH+31 for every brain.
        `slot_len`: (brain, slot) -> length of the stored value (default: the
        secret's length).
        `strays`: the other `cell.db`s a real brain composition and affinity
        hive carry beside the named ones, each WITH the table the probe looks
        for (`collector/window`, `session-keeper/night|sessions`,
        `affinity/clock`) -- a real colony had four `system` tables under
        `talky/` (ledger fund 9).
        `nested_brain`: talky's llm cell is not at `talky/brain/` but one level
        deeper (`talky/inner/brain/`) -- the glob fallback's one hit;
        with `extra_brain_db` a second one (`talky/twin/brain/`) beside it.
        """
        import sqlite3
        root = pathlib.Path(tmp) / "colony"
        root.mkdir()
        packs = packs if packs is not None else {b: [(self.BIRTH + 30, "")] for b in self.BRAINS}
        acks = acks if acks is not None else {b: [(self.BIRTH + 31, "")] for b in self.BRAINS}
        slot_len = slot_len or {}
        con = sqlite3.connect(str(root / "colony.db"))
        con.execute(HeadersTest.SCHEMA)
        n = 0
        rim = self.MEMBER + "/assistants/" + self.ASSISTANT + "/"

        def log(at, frm, to, hop, body):
            nonlocal n
            n += 1
            con.execute("INSERT INTO message_log VALUES (?, ?, NULL, NULL, 8, ?, ?, NULL, ?, "
                        "'inline', ?, ?)",
                        ("m-%05d" % n, "t-%05d" % n, frm, to,
                         json.dumps({"hop": hop, "context": {}}), json.dumps(body), at))

        system = {"identity": {"soul": {"text": self.SECRET}},
                  "instructions": {"reply": {"text": "reply rules of the fixture agent"}}}
        for brain in self.BRAINS:
            for at, code in packs.get(brain, []):
                log(at, self.MEMBER + "/assistants/" + self.ASSISTANT, rim + brain,
                    {"route": "in_pack", "error_code": code}, {"system": system})
                # a hop INSIDE the rim is the same pack, not a second one
                log(at, rim + brain, rim + brain + "/brain",
                    {"route": "in_pack"}, {"system": system})
            for at, code in acks.get(brain, []):
                hop = {"route": "pack_ack"}
                if code:
                    hop["error_code"] = code
                log(at, rim + brain, self.MEMBER + "/assistants/" + self.ASSISTANT, hop,
                    {"messages": []})
        # noise: another route to the same rim, and a pack to a stranger
        log(self.BIRTH + 5, self.MEMBER, rim + "talky", {"route": "in_turn"}, {"messages": []})
        log(self.BIRTH + 5, self.MEMBER, "/os/orgs/o/members/x/assistants/a/talky",
            {"route": "in_pack"}, {"system": system})
        con.commit()
        con.close()

        main = root / "main" / self.MEMBER.strip("/")
        def system_db(d):
            d.mkdir(parents=True)
            c = sqlite3.connect(str(d / "cell.db"))
            c.execute("CREATE TABLE system (slot_path TEXT PRIMARY KEY, value TEXT)")
            c.commit()
            c.close()

        for brain in self.BRAINS:
            d = main / "assistants" / self.ASSISTANT / brain / "brain"
            if nested_brain and brain == "talky":
                d = main / "assistants" / self.ASSISTANT / brain / "inner" / "brain"
            d.mkdir(parents=True)
            c = sqlite3.connect(str(d / "cell.db"))
            c.execute("CREATE TABLE system (slot_path TEXT PRIMARY KEY, value TEXT)")
            for s in self.SLOTS + ("tools.schema",):
                c.execute("INSERT INTO system VALUES (?, ?)",
                          (s, "x" * slot_len.get((brain, s), len(self.SECRET))))
            c.commit()
            c.close()
            # a keeper's store beside the brain: a cell.db without a `system` table
            k = main / "assistants" / self.ASSISTANT / brain / "session-keeper" / "store"
            k.mkdir(parents=True)
            sqlite3.connect(str(k / "cell.db")).execute("CREATE TABLE sessions (id TEXT)").connection.close()
            if strays:
                gen = main / "assistants" / self.ASSISTANT / brain
                for part in ("collector/window", "session-keeper/night",
                             "session-keeper/sessions"):
                    system_db(gen / part)
        if extra_brain_db:
            system_db(main / "assistants" / self.ASSISTANT / "talky" / "twin" / "brain")
        if strays:
            clock = main / "affinity" / "clock"
            clock.mkdir(parents=True)
            c = sqlite3.connect(str(clock / "cell.db"))
            c.execute("CREATE TABLE subscribers (id TEXT)")
            c.commit()
            c.close()
        store = main / "affinity" / "store"
        store.mkdir(parents=True)
        c = sqlite3.connect(str(store / "cell.db"))
        c.execute("CREATE TABLE subscribers (id TEXT, cell_path TEXT, subject TEXT, audience TEXT, "
                  "channel TEXT, slots TEXT, pack_hash TEXT, status TEXT, sent_at TEXT)")
        c.execute("INSERT INTO subscribers VALUES ('sub:a-self', './assistants/a', 'entity:a', "
                  "'agent:a', '*', '[\"brain\"]', ?, 'active', '2026-09-28T06:00:30Z')",
                  ("f" * 64,))
        c.commit()
        c.close()
        return root

    def snapshot(self, root):
        return sorted((str(p), p.read_bytes()) for p in root.rglob("*") if p.is_file())

    def probe(self, root, extra=(), birth=True):
        args = ["python3", str(LAB / "identity_probe.py"), "--root", str(root),
                "--member", self.MEMBER, "--assistant", self.ASSISTANT,
                "--brains", ",".join(self.BRAINS)]
        if birth:
            args += ["--birth", str(self.BIRTH)]
        before = self.snapshot(root)
        done = subprocess.run(args + list(extra), capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(before, self.snapshot(root), "a reader writes nothing")
        self.assertNotIn(self.SECRET, done.stdout + done.stderr, "no slot text leaves the tool")
        return done

    def verdict(self, done):
        last = (done.stdout.strip().splitlines() or [""])[-1]
        m = re.match(r"IDENTITY (PASS|FAIL) packs=(\d+)/(\d+) acks=(\d+) "
                     r"slots=(\d+)/(\d+) sent_at=(\S+)$", last)
        self.assertTrue(m, done.stdout + done.stderr)
        return m.group(1), tuple(int(x) for x in m.groups()[1:6]), m.group(7)

    def test_every_brain_with_pack_ack_and_slots_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp))
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertEqual(("PASS", (3, 3, 3, 6, 6), "2026-09-28T06:00:30Z"), self.verdict(done))
        self.assertIn("first_pack_after_s=30", done.stdout)

    def test_a_brain_without_a_pack_fails(self):
        packs = {b: [(self.BIRTH + 30, "")] for b in self.BRAINS if b != "cogny"}
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, packs=packs))
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(("FAIL", (2, 3, 3, 6, 6)), self.verdict(done)[:2])

    def test_a_pack_before_the_birth_does_not_count(self):
        packs = dict({b: [(self.BIRTH + 30, "")] for b in self.BRAINS},
                     talky=[(self.BIRTH - 1, "")])
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, packs=packs))
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(2, self.verdict(done)[1][0])

    def test_an_empty_slot_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, slot_len={("talky-chat", "instructions.reply"): 0}))
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(("FAIL", (3, 3, 3, 5, 6)), self.verdict(done)[:2])

    def test_a_refused_pack_fails(self):
        acks = dict({b: [(self.BIRTH + 31, "")] for b in self.BRAINS},
                    talky=[(self.BIRTH + 31, "pack_slot_refused")])
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, acks=acks))
        self.assertEqual(1, done.returncode, done.stdout)
        self.assertEqual(("FAIL", (3, 3, 2, 6, 6)), self.verdict(done)[:2])
        self.assertIn("errors=1", done.stdout)

    def test_without_birth_only_the_brains_and_the_subscription_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, packs={}, acks={}), birth=False)
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertEqual("PASS", self.verdict(done)[0])

    def test_json_carries_the_same_numbers(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp), extra=["--json"])
        self.assertEqual(0, done.returncode, done.stderr)
        doc = json.loads(done.stdout)
        self.assertEqual("PASS", doc["verdict"])
        self.assertEqual(30, doc["brains"]["cogny"]["first_pack_after_s"])
        self.assertEqual({"identity.soul": len(self.SECRET),
                          "instructions.reply": len(self.SECRET)},
                         doc["brains"]["cogny"]["slots"])
        self.assertEqual(64, doc["subscription"]["pack_hash_len"])

    def test_two_candidate_brain_databases_are_refused_not_guessed(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, nested_brain=True, extra_brain_db=True))
        self.assertEqual(2, done.returncode, done.stdout)
        self.assertIn("talky", done.stderr)
        self.assertIn("want exactly 1", done.stderr)

    def test_collector_and_keeper_tables_are_not_the_brain(self):
        """Fund 9: the brain is `<brain>/brain/cell.db`, the store `affinity/store/cell.db`
        -- a `system` table in a collector's window or a keeper's store, or a
        `subscribers` table in the clock, changes nothing."""
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, strays=True), extra=["--json"])
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        doc = json.loads(done.stdout)
        self.assertEqual("PASS", doc["verdict"])
        for brain in self.BRAINS:
            self.assertTrue(doc["brains"][brain]["cell_db"].endswith(
                "/assistants/a/%s/brain/cell.db" % brain), doc["brains"][brain]["cell_db"])
        self.assertTrue(doc["subscription"]["cell_db"].endswith("/affinity/store/cell.db"))

    def test_a_brain_one_level_deeper_is_found_by_the_fallback(self):
        """No `<brain>/brain/cell.db`: the glob counts only paths ending in
        `/brain/cell.db`, the strays beside it do not make it ambiguous."""
        with tempfile.TemporaryDirectory() as tmp:
            done = self.probe(self.tree(tmp, strays=True, nested_brain=True), extra=["--json"])
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertTrue(json.loads(done.stdout)["brains"]["talky"]["cell_db"].endswith(
            "/talky/inner/brain/cell.db"))


class SidecarQuoteTest(unittest.TestCase):
    """`sidecar_quote.py` over a fixture `colony.db` (GH #871): the count and the
    verdict line, never a running colony."""

    BRAIN = "/a/talky/brain"
    BLOCK = '```sidecar\n{"memory": {"nothing_new": true, "facts": []}}\n```'

    def make_db(self, tmp, turns):
        """`turns`: (class, block, tool) triples, one brain answer each."""
        db = pathlib.Path(tmp) / "colony.db"
        conn = sqlite3.connect(str(db))
        conn.execute("CREATE TABLE message_log (id TEXT PRIMARY KEY, trace_id TEXT,"
                     " parent_message_id TEXT, correlation_id TEXT, ttl INTEGER,"
                     " from_path TEXT, to_path TEXT, reply_to TEXT, headers TEXT,"
                     " body_kind TEXT, body_payload TEXT, created_at INTEGER)")
        collector, splitter = "/a/talky/collector", "/a/talky/splitter"
        for i, (klass, block, tool) in enumerate(turns):
            window = [{"origin": "user", "type": "text", "text": "q"}]
            if klass != "fresh":
                window = [{"origin": "user", "type": "text", "text": "q0"},
                          {"origin": "assistant", "type": "text", "text": "a0"}] + window
            consult = {"open": ["c1"] if klass == "history+consult" else [],
                       "text": "open consults: c1" if klass == "history+consult" else ""}
            parent = {"messages": window, "system": {"consult": consult}}
            if tool:
                out = {"messages": [{"origin": "assistant", "type": "tool_call", "id": "x",
                                     "text": "{}"}]}
                finish = "tool_calls"
            else:
                out = {"messages": [{"origin": "assistant", "type": "text",
                                     "text": "answer" + ("\n\n" + self.BLOCK if block else "")}]}
                finish = "stop"
            pid, oid = "p%04d" % i, "o%04d" % i
            conn.execute("INSERT INTO message_log VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                         (pid, "t", None, None, 8, collector, self.BRAIN, None,
                          json.dumps({"hop": {}}), "inline", json.dumps(parent), 1000 + i))
            conn.execute("INSERT INTO message_log VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                         (oid, "t", pid, None, 8, self.BRAIN, splitter, None,
                          json.dumps({"hop": {"finish_reason": finish,
                                              "tokens_completion": 12}}),
                          "inline", json.dumps(out), 1000 + i))
        conn.commit()
        conn.close()
        return db

    def quote(self, turns, *extra):
        with tempfile.TemporaryDirectory() as tmp:
            db = self.make_db(tmp, turns)
            done = subprocess.run(["python3", str(LAB / "sidecar_quote.py"), "--db", str(db),
                                   "--brain", self.BRAIN] + list(extra),
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, cwd=str(REPO))
        return done.returncode, done.stdout.strip().splitlines()

    def mixed(self, block_in_history=True):
        return ([("fresh", True, False)] * 8 + [("history", block_in_history, False)] * 10
                + [("history+consult", block_in_history, False)] * 4)

    def test_every_answer_with_its_block_passes(self):
        code, lines = self.quote(self.mixed())
        self.assertEqual(0, code, lines)
        self.assertEqual("SIDECAR PASS n=22 with=22 rate=100 history_n=14 history_miss=0",
                         lines[-1])

    def test_history_without_the_block_fails(self):
        code, lines = self.quote(self.mixed(block_in_history=False))
        self.assertEqual(1, code, lines)
        self.assertEqual("SIDECAR FAIL n=22 with=8 rate=36 history_n=14 history_miss=14",
                         lines[-1])

    def test_one_history_miss_is_tolerated_two_are_not(self):
        turns = self.mixed()
        turns[8] = ("history", False, False)
        self.assertEqual(0, self.quote(turns)[0])
        turns[9] = ("history", False, False)
        code, lines = self.quote(turns)
        self.assertEqual(1, code, lines)
        self.assertIn("history_miss=2", lines[-1])

    def test_too_few_answers_fail(self):
        code, lines = self.quote(self.mixed()[:10])
        self.assertEqual(1, code, lines)
        self.assertTrue(lines[-1].startswith("SIDECAR FAIL n=10 "), lines)

    def test_tool_rounds_do_not_count(self):
        code, lines = self.quote(self.mixed() + [("history", False, True)] * 5)
        self.assertEqual(0, code, lines)
        self.assertIn(" n=22 ", lines[-1] + " ")

    def test_the_class_is_the_replay_s_class(self):
        code, lines = self.quote(self.mixed(), "--json")
        rows = [json.loads(line) for line in lines[:-1]]
        self.assertEqual({"fresh": 8, "history": 10, "history+consult": 4},
                         {c: sum(1 for r in rows if r["class"] == c)
                          for c in ("fresh", "history", "history+consult")})

    def test_an_unfenced_section_object_is_read_and_cut_whole(self):
        # GH #871 fix round 1 (review I-2): a model that drops the fence but
        # keeps the section form wrote `{"memory": {...}}`. The naked probe
        # found the INNER object first, read it as malformed (one brace too
        # many) and left `{"memory":` standing in the answer -- in a harness
        # run, 26 of 26 channel answers ended on it. The outer object is the
        # section form and is read and cut as a whole.
        sys.path.insert(0, str(LAB))
        sys.dont_write_bytecode = True
        import sidecar_turns
        for inner in ('{"facts": [], "topic": {"movement": "continue", "name": "tea"}}',
                      '{"nothing_new": true, "facts": []}'):
            with self.subTest(inner=inner):
                text = 'Sure, noted.\n\n{"memory": %s}' % inner
                self.assertEqual("Sure, noted.", sidecar_turns.stripped(text))
                self.assertEqual({"found": True, "parsed": True, "memory": True},
                                 sidecar_turns.verdict(text))

    def test_a_nested_object_before_the_first_marker_is_cut_whole(self):
        # Fix strand F (review minor M-NR-M1 of #871): the backwards scan
        # stopped at the FIRST brace whose tail held a marker. With `topic`
        # written before `facts` that is the inner `topic` object: read alone
        # it has a brace too many, the answer was flagged malformed and kept
        # `{"memory": {"topic":` (section form) or `{"topic":` (legacy form).
        sys.path.insert(0, str(LAB))
        sys.dont_write_bytecode = True
        import sidecar_turns
        topic_first = '{"topic": {"movement": "continue", "name": "tea"}, "facts": []}'
        for text in ('Sure, noted.\n\n{"memory": %s}' % topic_first,
                     'Sure, noted.\n\n%s' % topic_first):
            with self.subTest(text=text):
                self.assertEqual("Sure, noted.", sidecar_turns.stripped(text))
                self.assertEqual({"found": True, "parsed": True, "memory": True},
                                 sidecar_turns.verdict(text))

    def test_prose_after_a_naked_object_stays_in_the_answer(self):
        # Fix strand F (review minor M-NR-M2 of #871): a naked object was cut
        # to the END of the answer, so the sentence the model wrote after it
        # went with it. A readable object is cut as the span it is -- the same
        # cut a fence gets; only an object that does not read still takes the
        # rest of the text, because nothing says where it ends.
        sys.path.insert(0, str(LAB))
        sys.dont_write_bytecode = True
        import sidecar_turns
        for text in ('Example: {"memory": {"facts": ["a"]}} -- that is the form. '
                     'Anything else?',
                     'Example: {"facts": ["a"]} -- that is the form. Anything else?'):
            with self.subTest(text=text):
                out = sidecar_turns.stripped(text)
                self.assertTrue(out.startswith("Example:"), out)
                self.assertTrue(out.endswith("that is the form. Anything else?"), out)
                self.assertNotIn("{", out)
                self.assertTrue(sidecar_turns.verdict(text)["parsed"])
        # Unreadable: still cut to the end, as before.
        self.assertEqual("Ok.", sidecar_turns.stripped('Ok. {"facts": ["a"'))
        # An earlier readable object that does not hold the marker is not the
        # attempt: the broken block after it is, cut to the end.
        self.assertEqual('See {"a": {"b": 1}} then',
                         sidecar_turns.stripped('See {"a": {"b": 1}} then {"facts": ['))

    def test_it_only_reads(self):
        text = (LAB / "sidecar_quote.py").read_text(encoding="utf-8")
        self.assertIn("mode=ro", text)
        for word in ("INSERT", "UPDATE", "DELETE", "urlopen", "requests"):
            with self.subTest(word=word):
                self.assertNotIn(word, text)


class FactYieldTest(unittest.TestCase):
    """`fact_yield.py` over a fixture `colony.db` (GH #881): how many facts each
    block that reached a memory's `in_remember` lane carried -- ids and numbers
    only, never a running colony and never a fact's text."""

    HIVE = "/m/memory"

    def make_db(self, tmp, rows):
        """`rows`: (id, from, to, hop, body_kind, body, created_at) tuples."""
        db = pathlib.Path(tmp) / "colony.db"
        conn = sqlite3.connect(str(db))
        conn.execute("CREATE TABLE message_log (id TEXT PRIMARY KEY, trace_id TEXT,"
                     " parent_message_id TEXT, correlation_id TEXT, ttl INTEGER,"
                     " from_path TEXT, to_path TEXT, reply_to TEXT, headers TEXT,"
                     " body_kind TEXT, body_payload TEXT, created_at INTEGER)")
        for rid, src, dst, hop, kind, body, ts in rows:
            conn.execute("INSERT INTO message_log VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                         (rid, "t", None, None, 8, src, dst, None,
                          json.dumps({"hop": hop, "context": {}}), kind,
                          body if isinstance(body, str) else json.dumps(body), ts))
        conn.commit()
        conn.close()
        return db

    @staticmethod
    def fact(claim="Elvese"):
        return {"subject": "user", "predicate": "lives_in", "claim": claim,
                "fact_kind": "world"}

    def door_pair(self, rid, src, hop, kind, body, ts, hive=None):
        """One block as a running colony logs it (G1, 28.09.): the hop through
        the hive's door (`src` -> hive) and the hive's forward to its own
        `extract-glue`, both on the `in_remember` route, same body."""
        hive = hive or self.HIVE
        return [(rid, src, hive, hop, kind, body, ts),
                (rid + "g", hive, hive + "/extract-glue", hop, kind, body, ts)]

    def lane(self):
        """The positive fixture: what a working lane leaves in the log."""
        talky = "/m/talky"
        inr = {"route": "in_remember", "section": "memory"}
        return (
            self.door_pair("r1", talky, inr, "inline",
                           {"section": "memory",
                            "payload": {"facts": [self.fact(), self.fact("tea")],
                                        "topic": {"movement": "start"}}}, 1000)
            + self.door_pair("r2", talky, inr, "inline",
                             {"section": "memory",
                              "payload": {"nothing_new": True, "facts": [],
                                          "topic": {"movement": "continue"}}}, 1001)
            + self.door_pair("r3", talky, {"route": "in_remember"}, "inline",
                             {"messages": [{"origin": "assistant", "type": "text",
                                            "text": json.dumps(
                                                {"memory": {"facts": [self.fact()],
                                                            "topic": {"movement": "end"}}})}]},
                             1002)
            + self.door_pair("r4", talky, {"route": "in_remember"}, "blob", "blob-id-1", 1003)
            + [("r5", talky, self.HIVE + "/writer", {"route": "in_episode"}, "inline",
                {"messages": []}, 1004),
               ("r6", "/m/talky/splitter", "/m/display", {"route": "sidecar",
                                                          "section": "display"}, "inline",
                {"section": "display", "payload": {"kind": "list"}}, 1005)])

    def yield_of(self, rows, *extra):
        with tempfile.TemporaryDirectory() as tmp:
            db = self.make_db(tmp, rows)
            done = subprocess.run(["python3", str(LAB / "fact_yield.py"), "--db", str(db)]
                                  + list(extra),
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, cwd=str(REPO))
        return done.returncode, done.stdout.strip().splitlines(), done.stdout

    def test_the_blocks_that_reached_the_lane_are_counted(self):
        # Each block is logged twice (door + forward to extract-glue); it is
        # counted once, at the door, with and without --hive.
        for extra in ([], ["--hive", self.HIVE]):
            with self.subTest(extra=extra):
                code, lines, _ = self.yield_of(self.lane(), *extra)
                self.assertEqual(0, code, lines)
                self.assertEqual(
                    "FACT_YIELD PASS n=3 facts=3 mean=1.00 with=2 empty=33 unreadable=1",
                    lines[-1])
                self.assertIn("r1 facts=2 nothing_new=no form=section", lines)
                self.assertIn("r2 facts=0 nothing_new=yes form=section", lines)
                self.assertIn("r3 facts=1 nothing_new=no form=text", lines)
                self.assertTrue(any(line.startswith("r4 unreadable (blob body)")
                                    for line in lines), lines)
                self.assertFalse(any(line.startswith(("r1g ", "r2g ", "r3g ", "r4g "))
                                     for line in lines), lines)

    def test_the_hives_own_forward_is_not_a_second_block(self):
        # Negative: the hive forwarding a block to its own extract-glue is no
        # arrival -- neither under --hive nor without it, and not when the
        # forward comes from a sibling cell inside the hive.
        body = {"section": "memory", "payload": {"facts": [self.fact()]}}
        inr = {"route": "in_remember", "section": "memory"}
        rows = [("f1", self.HIVE, self.HIVE + "/extract-glue", inr, "inline", body, 1000),
                ("f2", self.HIVE + "/router", self.HIVE + "/extract-glue", inr, "inline",
                 body, 1001)]
        code, lines, _ = self.yield_of(rows, "--hive", self.HIVE)
        self.assertEqual(1, code, lines)
        self.assertEqual("FACT_YIELD FAIL n=0 facts=0 mean=0.00 with=0 empty=0 unreadable=0",
                         lines[-1])
        code, lines, _ = self.yield_of(rows[:1])
        self.assertEqual(1, code, lines)
        self.assertEqual("FACT_YIELD FAIL n=0 facts=0 mean=0.00 with=0 empty=0 unreadable=0",
                         lines[-1])

    def test_a_block_wired_straight_to_extract_glue_counts_once(self):
        # Older wiring: the edge points at extract-glue itself -- one hop from
        # outside the hive, one block.
        rows = [("d1", "/m/talky", self.HIVE + "/extract-glue",
                 {"route": "in_remember", "section": "memory"}, "inline",
                 {"section": "memory", "payload": {"facts": [self.fact()]}}, 1000)]
        for extra in ([], ["--hive", self.HIVE]):
            with self.subTest(extra=extra):
                self.assertEqual(
                    "FACT_YIELD PASS n=1 facts=1 mean=1.00 with=1 empty=0 unreadable=0",
                    self.yield_of(rows, *extra)[1][-1])

    def test_unreadable_headers_count_as_unreadable(self):
        with tempfile.TemporaryDirectory() as tmp:
            db = self.make_db(tmp, [])
            conn = sqlite3.connect(str(db))
            conn.execute("INSERT INTO message_log VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                         ("h1", "t", None, None, 8, "/m/talky", self.HIVE, None,
                          '{"hop": {"route": "in_remember"', "inline", "{}", 1000))
            conn.commit()
            conn.close()
            done = subprocess.run(["python3", str(LAB / "fact_yield.py"), "--db", str(db)],
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, cwd=str(REPO))
        lines = done.stdout.strip().splitlines()
        self.assertEqual(1, done.returncode, lines)
        self.assertIn("h1 unreadable (headers not JSON)", lines)
        self.assertEqual("FACT_YIELD FAIL n=0 facts=0 mean=0.00 with=0 empty=0 unreadable=1",
                         lines[-1])

    def test_a_log_where_no_block_reached_the_lane_fails(self):
        # The shape of GH #881: the brain wrote its block, the splitter cut it
        # into `sidecar` hops, and an edge wired on the old route took none of
        # them to the memory -- only episodes arrived.
        block = '```sidecar\n{"memory": {"facts": [], "topic": {"movement": "start"}}}\n```'
        rows = []
        for i in range(4):
            rows.append(("b%d" % i, "/m/talky/brain", "/m/talky/splitter",
                         {"finish_reason": "stop"}, "inline",
                         {"messages": [{"origin": "assistant", "type": "text",
                                        "text": "answer\n\n" + block}]}, 1000 + i))
            rows.append(("s%d" % i, "/m/talky/splitter", "/m/talky",
                         {"route": "sidecar", "section": "memory"}, "inline",
                         {"section": "memory", "payload": {"facts": []}}, 1000 + i))
            rows.append(("e%d" % i, "/m/talky", self.HIVE,
                         {"route": "in_episode"}, "inline", {"messages": []}, 1000 + i))
            rows.append(("e%dw" % i, self.HIVE, self.HIVE + "/writer",
                         {"route": "in_episode"}, "inline", {"messages": []}, 1000 + i))
        code, lines, _ = self.yield_of(rows)
        self.assertEqual(1, code, lines)
        self.assertEqual("FACT_YIELD FAIL n=0 facts=0 mean=0.00 with=0 empty=0 unreadable=0",
                         lines[-1])

    def test_the_hive_filter_keeps_one_memory(self):
        rows = self.lane() + self.door_pair(
            "r7", "/other/talky", {"route": "in_remember"}, "inline",
            {"section": "memory", "payload": {"facts": [self.fact()]}}, 1006,
            hive="/other/memory")
        self.assertIn(" n=4 ", self.yield_of(rows)[1][-1] + " ")
        self.assertIn(" n=3 ", self.yield_of(rows, "--hive", self.HIVE)[1][-1] + " ")

    def test_since_skips_older_blocks(self):
        code, lines, _ = self.yield_of(self.lane(), "--since", "1001")
        self.assertEqual(0, code, lines)
        self.assertTrue(lines[-1].startswith("FACT_YIELD PASS n=2 facts=1 "), lines)
        self.assertNotIn("r1 facts=2 nothing_new=no form=section", lines)

    def test_a_foreign_section_is_unreadable_as_the_hive_refuses_it(self):
        rows = [("x1", "/m/talky", self.HIVE + "/extract-glue",
                 {"route": "in_remember", "section": "display"}, "inline",
                 {"section": "display", "payload": {"kind": "list", "facts": [1]}}, 1000)]
        code, lines, _ = self.yield_of(rows)
        self.assertEqual(1, code, lines)
        self.assertIn("x1 unreadable (section display)", lines)
        self.assertEqual("FACT_YIELD FAIL n=0 facts=0 mean=0.00 with=0 empty=0 unreadable=1",
                         lines[-1])

    def test_no_fact_text_is_printed(self):
        rows = self.door_pair("z1", "/m/talky", {"route": "in_remember"}, "inline",
                              {"section": "memory",
                               "payload": {"facts": [self.fact("ZXQ-UNIQUE-CLAIM")]}}, 1000)
        for extra in ([], ["--json"]):
            with self.subTest(extra=extra):
                code, lines, out = self.yield_of(rows, *extra)
                self.assertEqual(0, code, lines)
                self.assertNotIn("ZXQ-UNIQUE-CLAIM", out)
                self.assertNotIn("lives_in", out)
        _, lines, _ = self.yield_of(rows, "--json")
        self.assertEqual(2, len(lines), lines)
        self.assertEqual({"id": "z1", "to": self.HIVE, "facts": 1,
                          "nothing_new": False, "form": "section"}, json.loads(lines[0]))

    def test_the_module_api_measures_what_the_cli_prints(self):
        mod = load_tool("fact_yield.py")
        with tempfile.TemporaryDirectory() as tmp:
            db = self.make_db(tmp, self.lane())
            summary = mod.measure(str(db), hive=self.HIVE)
        self.assertEqual((3, 3, 2, 1), (summary["n"], summary["facts"], summary["with"],
                                        summary["unreadable"]))
        ok, line = mod.verdict_line(summary, 1)
        self.assertTrue(ok)
        self.assertEqual("FACT_YIELD PASS n=3 facts=3 mean=1.00 with=2 empty=33 unreadable=1",
                         line)

    def test_it_only_reads(self):
        text = (LAB / "fact_yield.py").read_text(encoding="utf-8")
        self.assertIn("mode=ro", text)
        for word in ("INSERT", "UPDATE", "DELETE", "urlopen", "requests"):
            with self.subTest(word=word):
                self.assertNotIn(word, text)
        with tempfile.TemporaryDirectory() as tmp:
            missing = pathlib.Path(tmp) / "no.db"
            done = subprocess.run(["python3", str(LAB / "fact_yield.py"), "--db", str(missing)],
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, cwd=str(REPO))
            self.assertEqual(2, done.returncode, done.stderr)
            self.assertFalse(missing.exists())


class StillShotTest(unittest.TestCase):
    """`stillShot` in `display-browser-lab.mjs` holds the page's running animations
    for the evidence screenshot and plays them on afterwards. Measured on a lane under
    CPUQuota=80 % (TV-F7): the infinite `display-light-drift` kept WebKit at 1-2
    frames a second and the shot ran into Playwright's 30 s mark (gh703 5/5 red);
    paused, the same shot took 4.5 s. A fake page drives it here: no browser."""

    FAKE = r"""
const log = [];
const anim = (name, state) => ({ name, playState: state,
  pause() { log.push("pause " + name); this.playState = "paused"; },
  play() { log.push("play " + name); this.playState = "running"; } });
const anims = [anim("drift", "running"), anim("done", "finished"), anim("held", "paused")];
globalThis.document = { getAnimations: () => anims };
const page = (fail) => ({
  evaluateHandle: async (fn) => { const v = fn(); return {
    evaluate: async (g) => g(v), dispose: async () => log.push("dispose") }; },
  on: () => {},
  screenshot: async (o) => {
    log.push("shot " + o.path + " " + o.scale + " " + o.animations + " " + o.caret + " " + typeof o.timeout
      + " running=" + anims.filter((a) => a.playState === "running").length);
    if (fail) throw new Error("boom");
  },
});
"""

    def node(self, script):
        uri = json.dumps((REPO / "workshop" / "tools" / "display-browser-lab.mjs").as_uri())
        done = subprocess.run(
            ["node", "--input-type=module", "-e",
             "import { stillShot, laboratory } from %s;\n%s\n%s" % (uri, self.FAKE, script)],
            capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_the_shot_is_taken_with_the_animations_held(self):
        log = self.node('await stillShot(page(false), { path: "a.jpg" });'
                        "console.log(JSON.stringify(log));")
        self.assertEqual(["pause drift", "shot a.jpg css disabled hide number running=0", "play drift", "dispose"], log)

    def test_a_failed_shot_still_plays_them_on_and_says_why(self):
        got = self.node('let why = ""; try { await stillShot(page(true), { path: "b.jpg" }); }'
                        " catch (e) { why = e.message; }"
                        "console.log(JSON.stringify({ log, why }));")
        self.assertEqual("boom", got["why"])
        self.assertEqual(["pause drift", "shot b.jpg css disabled hide number running=0", "play drift", "dispose"], got["log"])

    # Re-check tv-bn-drv2-fix1 H1: Playwright's `animations: "disabled"` FINISHES a
    # finite animation the pause held (it only skips `playbackRate === 0`), and a
    # `play()` on a finished animation rewinds it and runs it again -- an entrance
    # that had landed would start over after every evidence shot. Only what is
    # still paused after the shot is played on.
    def test_an_animation_the_shot_finished_is_not_started_again(self):
        log = self.node('anims.push(anim("enter", "running")); const p = page(false);'
                        " const shoot = p.screenshot;"
                        ' p.screenshot = async (o) => { await shoot(o); anims[3].playState = "finished"; };'
                        ' await stillShot(p, { path: "d.jpg" }); console.log(JSON.stringify(log));')
        self.assertEqual(["pause drift", "pause enter", "shot d.jpg css disabled hide number running=0",
                          "play drift", "dispose"], log)

    # Re-check tv-bn-drv2-fix1 M (TV-B4): the shot answers whether it was taken, so
    # a driver can write a missing picture into its receipt as an annotation by
    # name instead of losing it -- the LOG line stays beside the run's output.
    def test_a_failed_evidence_shot_is_an_annotation_never_a_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            got = self.node("const lab = laboratory({ page: page(true), outDir: %s, runMs: %d });"
                            'const r = await lab.shot("c");'
                            " console.log(JSON.stringify({ noise: lab.noise, r })); process.exit(0);"
                            % (json.dumps(tmp), kit_time.HANG_MS))
        self.assertEqual(["SHOT c not taken: boom"], got["noise"])
        self.assertEqual({"taken": False, "why": "boom"}, got["r"])

    def test_a_taken_evidence_shot_says_so(self):
        with tempfile.TemporaryDirectory() as tmp:
            got = self.node("const lab = laboratory({ page: page(false), outDir: %s, runMs: %d });"
                            'const r = await lab.shot("e");'
                            " console.log(JSON.stringify({ noise: lab.noise, r })); process.exit(0);"
                            % (json.dumps(tmp), kit_time.HANG_MS))
        self.assertEqual([], got["noise"])
        self.assertEqual({"taken": True}, got["r"])


class TapMarkerOrderTest(unittest.TestCase):
    """Re-check tv-bn-drv2-fix1 H2: `tap.mjs` closes its reading on a marker sent
    behind the gesture on the same LiveView channel. Sent at once, it overtook a
    frame the page's own handler queued behind a timer of the gesture's task (the
    `pointerdown`/`pointerup` modes), and the probe read "nothing" for a tap that
    was still coming. `markBehindTheGesture` lets the page's queued tasks and the
    next frame run first. A fake page drives it here: no browser."""

    def order(self, script, names="markBehindTheGesture, MARKER"):
        uri = json.dumps((LAB / "tap.mjs").as_uri())
        done = subprocess.run(
            ["node", "--input-type=module", "-e",
             "import { " + names + " } from %s;\n"
             "const sent = [], wire = [];\n"
             "globalThis.window = { __lv: { joinRef: '4', topic: 'lv:x',"
             " ws: { readyState: 1, send: (d) => { wire.push(JSON.parse(d)); sent.push(JSON.parse(d)[3] === 'event'"
             " ? JSON.parse(d)[4].event : d); } } } };\n%s" % (uri, script)],
            capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_the_marker_follows_a_frame_queued_behind_the_gesture(self):
        got = self.order(
            "setTimeout(() => window.__lv.ws.send(JSON.stringify(['4', '9', 'lv:x', 'event',"
            " { event: 'tap' }])), 0);\n"
            "const { ref } = await markBehindTheGesture({ name: MARKER });\n"
            "console.log(JSON.stringify({ ref, sent }));")
        self.assertEqual(["tap", "tap-probe-marker"], got["sent"])
        self.assertEqual("tap-probe-marker", got["ref"])

    # Re-check tv-bn-i-fix3, mutant class: each half of the wait has a page it
    # is there for. Without a frame clock (node) the two task turns are the
    # wait: a handler that chains two tasks before its frame is still ahead of
    # the marker, with either turn gone it is not.
    def test_the_marker_follows_a_frame_two_tasks_behind_the_gesture(self):
        got = self.order(
            "setTimeout(() => setTimeout(() => window.__lv.ws.send(JSON.stringify(['4', '9', 'lv:x',"
            " 'event', { event: 'tap' }])), 0), 0);\n"
            "const mark = await markBehindTheGesture({ name: MARKER });\n"
            "console.log(JSON.stringify({ mark, sent }));")
        self.assertEqual(["tap", "tap-probe-marker"], got["sent"])
        # No frame clock: `framed` is null ("no frame to wait for"), never a
        # verdict either way -- `markerLines` reads only `false` as a HANG.
        self.assertEqual({"ref": "tap-probe-marker", "framed": None}, got["mark"])

    # A frame clock slower than a task (as in a browser): a frame the page sends
    # from the frame after next is still ahead of the marker, because the probe
    # waits two frames, not one.
    def test_the_marker_follows_a_frame_sent_two_frames_on(self):
        got = self.order(
            "const frames = []; let due = false;\n"
            "globalThis.requestAnimationFrame = (cb) => { frames.push(cb);"
            " if (!due) { due = true; setTimeout(() => { due = false;"
            " for (const f of frames.splice(0)) f(); }, 20); } };\n"
            "requestAnimationFrame(() => requestAnimationFrame(() => window.__lv.ws.send(JSON.stringify(['4', '9',"
            " 'lv:x', 'event', { event: 'tap' }]))));\n"
            "const mark = await markBehindTheGesture({ name: MARKER, hangMs: %d });\n"
            "console.log(JSON.stringify({ mark, sent }));" % kit_time.HANG_MS)
        self.assertEqual(["tap", "tap-probe-marker"], got["sent"])
        self.assertEqual({"ref": "tap-probe-marker", "framed": True}, got["mark"])

    def test_without_an_open_channel_there_is_no_marker(self):
        got = self.order("window.__lv.ws.readyState = 3;\n"
                         "const { ref } = await markBehindTheGesture({ name: MARKER });\n"
                         "console.log(JSON.stringify({ ref, sent }));")
        self.assertEqual({"ref": None, "sent": []}, got)

    # Re-check tv-bn-i M2: the race of two frames against `hangMs` resolved
    # silently, and the marker overtook a frame without a word (TV-B1: a wait
    # with a HANG deadline says so when it expires). A fake frame clock that
    # never fires stands for a compositor that does not draw.
    def test_a_frame_that_never_comes_is_a_hang_line(self):
        got = self.order(
            "globalThis.requestAnimationFrame = () => {};\n"
            "const mark = await markBehindTheGesture({ name: MARKER, hangMs: %d });\n"
            "console.log(JSON.stringify({ mark, lines: markerLines(mark, %d), sent }));"
            % (kit_time.OVER_MS, kit_time.OVER_MS),
            names="markBehindTheGesture, markerLines, MARKER")
        self.assertEqual({"ref": "tap-probe-marker", "framed": False}, got["mark"])
        self.assertEqual(["HANG: no frame within %d ms before the marker" % kit_time.OVER_MS], got["lines"])
        self.assertEqual(["tap-probe-marker"], got["sent"])

    def test_frames_that_come_are_no_hang_line(self):
        got = self.order(
            "globalThis.requestAnimationFrame = (cb) => setTimeout(cb, 0);\n"
            "const mark = await markBehindTheGesture({ name: MARKER, hangMs: %d });\n"
            "console.log(JSON.stringify({ mark, lines: markerLines(mark, %d) }));" % (kit_time.HANG_MS, kit_time.HANG_MS),
            names="markBehindTheGesture, markerLines, MARKER")
        self.assertEqual({"ref": "tap-probe-marker", "framed": True}, got["mark"])
        self.assertEqual([], got["lines"])

    def test_no_channel_is_a_hang_line(self):
        got = self.order(
            "window.__lv.ws.readyState = 3;\n"
            "const mark = await markBehindTheGesture({ name: MARKER });\n"
            "console.log(JSON.stringify(markerLines(mark, %d)));" % kit_time.OVER_MS,
            names="markBehindTheGesture, markerLines, MARKER")
        self.assertEqual(["HANG: no open LiveView channel to send the marker on"], got)

    # Re-check tv-bn-i-fix4 M2: the fake read only the event name, so the
    # marker's ref slot, its `type` and the no-channel return went unpinned. The
    # whole frame is the marker's contract: its ref is what `sentSince` leaves
    # out, so a frame without it counted the marker's reply as the tap's answer.
    def test_the_marker_frame_is_the_probes_own(self):
        got = self.order("await markBehindTheGesture({ name: MARKER });\n"
                         "console.log(JSON.stringify(wire));")
        self.assertEqual([["4", "tap-probe-marker", "lv:x", "event",
                           {"type": "click", "event": "tap-probe-marker", "value": {}}]], got)

    # A page that never joined a LiveView channel has no `__lv` at all: no
    # marker, the channel HANG, and no crash in `page.evaluate`.
    def test_a_page_that_never_joined_has_no_channel(self):
        got = self.order(
            "globalThis.window = {};\n"
            "const mark = await markBehindTheGesture({ name: MARKER });\n"
            "console.log(JSON.stringify({ mark, lines: markerLines(mark, %d), sent }));" % kit_time.OVER_MS,
            names="markBehindTheGesture, markerLines, MARKER")
        self.assertEqual({"mark": {"ref": None, "framed": None},
                          "lines": ["HANG: no open LiveView channel to send the marker on"], "sent": []}, got)

    # No frame and no channel: both HANG lines, the frame's one too.
    def test_a_page_without_frames_or_channel_has_both_hang_lines(self):
        got = self.order(
            "globalThis.requestAnimationFrame = () => {};\n"
            "window.__lv.ws.readyState = 3;\n"
            "const mark = await markBehindTheGesture({ name: MARKER, hangMs: %d });\n"
            "console.log(JSON.stringify({ mark, lines: markerLines(mark, %d), sent }));"
            % (kit_time.OVER_MS, kit_time.OVER_MS),
            names="markBehindTheGesture, markerLines, MARKER")
        self.assertEqual({"mark": {"ref": None, "framed": False},
                          "lines": ["HANG: no open LiveView channel to send the marker on",
                                    "HANG: no frame within %d ms before the marker" % kit_time.OVER_MS],
                          "sent": []}, got)

    # Re-check tv-bn-i-fix4 M5: what the page sent and heard decided in `main`
    # (the `sentOf` filters). `sentSince` is that page function, driven here by
    # a fake `window`: one frame before the gesture, the tap's event frame, a
    # heartbeat, the marker's frame, a second event frame; in, one reply before,
    # the marker's reply, the replies to both event frames. Two frames of the
    # gesture against one of the marker, so "only the marker's" is no match.
    def page_counts(self, ref):
        return self.order(
            "globalThis.window = { __outMark: 1, __inMark: 1,"
            " __out: [{ d: '[\"4\",\"1\",\"lv:x\",\"event\",{}]' }, { d: '[\"4\",\"9\",\"lv:x\",\"event\",{}]' },"
            " { d: '[\"4\",\"11\",\"lv:x\",\"event\",{}]' },"
            " { d: '[null,\"10\",\"phoenix\",\"heartbeat\",{}]' },"
            " { d: '[\"4\",\"tap-probe-marker\",\"lv:x\",\"event\",{}]' }],"
            " __in: [{ d: '[\"4\",\"1\",\"lv:x\",\"phx_reply\",{}]' },"
            " { d: '[\"4\",\"tap-probe-marker\",\"lv:x\",\"phx_reply\",{}]' },"
            " { d: '[\"4\",\"9\",\"lv:x\",\"phx_reply\",{}]' }, { d: '[\"4\",\"11\",\"lv:x\",\"phx_reply\",{}]' }] };\n"
            "console.log(JSON.stringify(sentSince(%s)));" % json.dumps(ref),
            names="sentSince")

    def test_the_marker_frames_are_not_the_gestures(self):
        self.assertEqual({"events": 2, "heard": 2}, self.page_counts("tap-probe-marker"))

    def test_without_a_marker_every_frame_since_the_gesture_counts(self):
        self.assertEqual({"events": 3, "heard": 3}, self.page_counts(None))

    # Re-check tv-bn-i-fix5 H2 (mutant X15): an event frame is one with "event"
    # in its event slot. A frame that carries "event" only in its payload --
    # after `[`, before `]`, after `:` -- is none; counted, the settle wait
    # waited for a tap that never comes (a false HANG, exit 1). Re-check
    # tv-bn-i-fix6 H3: `,"event",` alone also matched a payload array
    # (`[1,"event",2]`); the slot is the fourth element of the frame
    # `[join_ref, ref, topic, event, payload]`, even a whole frame of that shape
    # inside the payload is none.
    def test_event_only_in_the_payload_is_no_event_frame(self):
        got = self.order(
            "globalThis.window = { __outMark: 0, __inMark: 0, __in: [],"
            " __out: [{ d: '[\"4\",\"12\",\"lv:x\",\"phx_join\",{\"a\":[\"event\",1],\"b\":[1,\"event\"],"
            "\"c\":\"event\",\"d\":[1,\"event\",2],\"e\":[null,null,\"t\",\"event\",1]}]' }] };\n"
            "console.log(JSON.stringify(sentSince('tap-probe-marker')));",
            names="sentSince")
        self.assertEqual({"events": 0, "heard": 0}, got)

    # Re-check tv-bn-i-fix6 H3: the slot check reads the frame's shape, not
    # quotes around a ref. A push without a join ref (`null`) and a frame cut at
    # 400 characters (the recorder keeps no more, `JSON.parse` would throw) are
    # still event frames.
    def test_an_event_frame_with_a_null_join_ref_or_cut_short_counts(self):
        got = self.order(
            "globalThis.window = { __outMark: 0, __inMark: 0, __in: [],"
            " __out: [{ d: '[null,\"5\",\"lv:x\",\"event\",{}]' },"
            " { d: '[\"4\",null,\"lv:x\",\"event\",{\"value\":\"' + 'x'.repeat(400) }] };\n"
            "console.log(JSON.stringify(sentSince('tap-probe-marker')));",
            names="sentSince")
        self.assertEqual({"events": 2, "heard": 0}, got)

    # Re-check tv-bn-i-fix4 M5: the attribute list and its condition decided in
    # `main`. `heldAttrs` is that page function, driven by a fake `document`.
    def test_the_tile_names_the_attributes_that_hold_frames(self):
        got = self.order(
            "const tile = (attrs) => ({ hasAttribute: (a) => attrs.includes(a) });\n"
            "const tiles = { both: tile(['phx-debounce', 'phx-throttle', 'phx-click']),"
            " throttle: tile(['phx-throttle']), plain: tile(['phx-click']) };\n"
            "globalThis.document = { querySelector: (sel) => tiles[sel] || null };\n"
            "console.log(JSON.stringify(['both', 'throttle', 'plain', 'none'].map(heldAttrs)));",
            names="heldAttrs")
        self.assertEqual([["phx-debounce", "phx-throttle"], ["phx-throttle"], [], []], got)


class MemberPathTest(unittest.TestCase):
    """Re-check tv-bn-i H2-4b: a second arrival of the tap that goes compose ->
    member -> compose is not ordered by the marker. `tap.mjs` follows
    `parent_message_id` from the tap row (the log's causal chain): every hop
    compose -> member the tap caused waits for its answer at compose, a tap in
    that answer counts, and a hop without an answer is a `NOTE: member path
    open`, never a silent whole reading. Fake log rows drive it: no colony.

    Re-check tv-bn-i-fix1 M1: "no hop open" is only a reading once compose has
    handled the marker -- a row whose `parent_message_id` is the marker row.
    compose takes the tap before the marker (one task, FIFO from web), so the
    hops the tap opened are logged by then; before it, a hop compose has not
    logged yet looks like no hop at all. `handled_marker` stands for that row."""

    COMPOSE = "/org/members/m/channels/display/compose"
    MEMBER = "/org/members/m/talky/in"

    def row(self, rid, parent, frm, to, event=None):
        body = {"event": {"name": event, "value": {"for": "view.chat"}}} if event else {"text": "x"}
        return {"id": "row-%08d" % rid, "parent_message_id": parent and "row-%08d" % parent,
                "created_at": "2026-10-12T06:20:%02d.000Z" % rid, "from_path": frm, "to_path": to,
                "headers_json": json.dumps({"hop": {"route": "event"}}),
                "body_kind": "inline", "body_payload": json.dumps(body)}

    def paths(self, rows, hang_ms=kit_time.HANG_MS):
        script = (
            "import * as t from %s;\n"
            "const bodyOf = (r) => JSON.parse(r.body_payload);\n"
            "const rows = %s;\n"
            "const paths = t.memberPathsOf(rows, bodyOf);\n"
            "console.log(JSON.stringify({ paths, lines: t.memberLines(paths, %d),"
            " all: t.tapsOf(rows, bodyOf).length }));"
            % (json.dumps((LAB / "tap.mjs").as_uri()), json.dumps(rows), hang_ms))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def tap(self):
        return self.row(1, None, "@external", self.COMPOSE, event="tap")

    def handled_marker(self):
        return [self.row(8, None, "@external", self.COMPOSE, event="tap-probe-marker"),
                self.row(9, 8, self.COMPOSE, "/org/members/m/channels/display/web")]

    def test_a_second_arrival_through_the_member_counts(self):
        got = self.paths([self.tap(),
                          self.row(2, 1, self.COMPOSE, self.MEMBER),
                          self.row(3, 2, self.MEMBER, self.COMPOSE, event="tap")] + self.handled_marker())
        self.assertEqual(1, got["paths"]["hops"])
        self.assertEqual([], got["paths"]["open"])
        self.assertEqual(["00000003"], [t["id"] for t in got["paths"]["taps"]])
        self.assertEqual(2, got["all"])
        self.assertEqual([], got["lines"])

    def test_an_answer_deeper_in_the_member_closes_the_path(self):
        got = self.paths([self.tap(),
                          self.row(2, 1, self.COMPOSE, self.MEMBER),
                          self.row(3, 2, self.MEMBER, "/org/members/m/talky/llm"),
                          self.row(4, 3, "/org/members/m/talky/llm", self.COMPOSE)] + self.handled_marker())
        self.assertEqual({"hops": 1, "open": [], "taps": [], "handled": True}, got["paths"])
        self.assertEqual([], got["lines"])

    # Re-check tv-bn-i-fix1 M2: the end of a hop is the member's answer AT
    # COMPOSE, not any row below it. A chain compose -> member -> llm that has
    # not answered compose yet is still open; read as closed, a tap on its way
    # back would arrive after a whole reading.
    def test_a_chain_without_an_answer_at_compose_stays_open(self):
        got = self.paths([self.tap(),
                          self.row(2, 1, self.COMPOSE, self.MEMBER),
                          self.row(3, 2, self.MEMBER, "/org/members/m/talky/llm")] + self.handled_marker(),
                         hang_ms=kit_time.OVER_MS)
        self.assertEqual(["00000002"], got["paths"]["open"])
        self.assertEqual(["NOTE: member path open (1 of 1 hop(s) compose -> member without an answer at compose"
                          " within 1 ms: 00000002); a tap on that path can still arrive"], got["lines"])

    def test_an_open_member_path_is_a_note(self):
        got = self.paths([self.tap(), self.row(2, 1, self.COMPOSE, self.MEMBER)] + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual(["00000002"], got["paths"]["open"])
        self.assertEqual(1, len(got["lines"]))
        self.assertTrue(got["lines"][0].startswith("NOTE: member path open"), got["lines"])
        self.assertIn("00000002", got["lines"][0])

    def test_the_display_channel_and_foreign_chains_are_no_member_path(self):
        got = self.paths([self.tap(),
                          self.row(2, 1, self.COMPOSE, "/org/members/m/channels/display/web"),
                          self.row(5, None, "@external", self.COMPOSE, event="tap-probe-marker"),
                          self.row(6, 5, self.COMPOSE, self.MEMBER)])
        self.assertEqual({"hops": 0, "open": [], "taps": [], "handled": True}, got["paths"])
        self.assertEqual([], got["lines"])

    # Re-check tv-bn-i-fix1 M1: the marker arrived at compose, but compose has
    # not handled it -- the hop the tap opens is not logged yet. Read at that
    # moment, "hops=0 open=0" was a silent whole reading; it is a NOTE.
    def test_a_marker_compose_has_not_handled_is_a_note(self):
        got = self.paths([self.tap(),
                          self.row(8, None, "@external", self.COMPOSE, event="tap-probe-marker")], hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 0, "open": [], "taps": [], "handled": False}, got["paths"])
        self.assertEqual(["NOTE: member path open (compose has not handled the marker within 1 ms; a hop"
                          " compose -> member it has not logged yet can still open one)"], got["lines"])

    # Mutant class (fix3): the walk down `parent_message_id` keeps a seen-set,
    # so a malformed log whose parents form a cycle still ends.
    def test_a_cyclic_chain_still_ends(self):
        got = self.paths([self.row(1, 2, "@external", self.COMPOSE, event="tap"),
                          self.row(2, 1, self.COMPOSE, self.MEMBER)] + self.handled_marker())
        self.assertEqual({"hops": 1, "open": [], "handled": True},
                         {k: got["paths"][k] for k in ("hops", "open", "handled")})

    # Re-check tv-bn-i-fix4 M1: no fixture had a row with two children. A fan-out
    # -- compose answers the tap with a frame to `web` and a hop to the member --
    # is the normal case; a children map that keeps only one child lost the hop
    # and read `hops 0` without a NOTE. Both orders of the rows.
    def test_a_fan_out_keeps_every_child_of_the_tap(self):
        web = "/org/members/m/channels/display/web"
        for rows in ([self.row(3, 1, self.COMPOSE, self.MEMBER), self.row(2, 1, self.COMPOSE, web), self.tap()],
                     [self.tap(), self.row(2, 1, self.COMPOSE, web), self.row(3, 1, self.COMPOSE, self.MEMBER)]):
            got = self.paths(rows + self.handled_marker(), hang_ms=kit_time.OVER_MS)
            self.assertEqual({"hops": 1, "open": ["00000003"], "taps": [], "handled": True}, got["paths"])
            self.assertEqual(["NOTE: member path open (1 of 1 hop(s) compose -> member without an answer at compose"
                              " within 1 ms: 00000003); a tap on that path can still arrive"], got["lines"])

    # Re-check tv-bn-i-fix5 M1: no fixture had a hop to a cell under another
    # channel. Where the display hive's exit `.` leads is the parent template's
    # choice, so compose -> `channels/voice/in` is a member path like any other;
    # a channel filter wider than `display` (mutant X14) dropped it and read
    # `hops 0` without a NOTE.
    def test_a_hop_to_another_channel_is_a_member_path(self):
        got = self.paths([self.row(2, 1, self.COMPOSE, "/org/members/m/channels/voice/in"), self.tap()]
                         + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 1, "open": ["00000002"], "taps": [], "handled": True}, got["paths"])
        self.assertEqual(["NOTE: member path open (1 of 1 hop(s) compose -> member without an answer at compose"
                          " within 1 ms: 00000002); a tap on that path can still arrive"], got["lines"])

    # Mutation run 3 (I8, REGEX segment): the display channel is `channels/display`,
    # not every path segment named `display`. A member cell under a hive called
    # `display` is a member like any other.
    def test_a_member_under_a_hive_named_display_is_a_member_path(self):
        got = self.paths([self.tap(), self.row(2, 1, self.COMPOSE, "/org/members/m/display/in")]
                         + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 1, "open": ["00000002"], "taps": [], "handled": True}, got["paths"])

    # Re-check tv-bn-i-fix5 H2 (mutant X13): compose is the cell `compose`, not
    # every cell whose name starts with it. A neighbour `composer` neither opens
    # a hop nor ends one: the answer it gets leaves the hop open.
    def test_a_neighbour_named_like_compose_is_not_compose(self):
        composer = "/org/members/m/channels/display/composer"
        got = self.paths([self.tap(),
                          self.row(2, 1, self.COMPOSE, self.MEMBER),
                          self.row(3, 2, self.MEMBER, composer, event="tap"),
                          self.row(4, 1, composer, "/org/members/m/cogny/in"),
                          # Mutation run 3 (I8): nor is a `display/compose` outside the channels.
                          self.row(5, 1, "/org/members/m/display/compose", "/org/members/m/cogny/in")]
                         + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 1, "open": ["00000002"], "taps": [], "handled": True}, got["paths"])

    # Re-check tv-bn-i-fix6 M1 (mutant Y1, `DISPLAY` without its closing `\/`):
    # the display hive's exit `.` (templates/display/config.json, `./compose -> .`)
    # makes the real hop compose -> the hive path `…/channels/display` itself, no
    # slash after it. That hop leaves the channel; read as display-internal it
    # was `hops 0` without a NOTE, and a tap coming back through it fell out.
    def test_the_hop_out_of_the_display_hive_is_a_member_path(self):
        got = self.paths([self.tap(), self.row(2, 1, self.COMPOSE, "/org/members/m/channels/display")]
                         + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 1, "open": ["00000002"], "taps": [], "handled": True}, got["paths"])
        self.assertEqual(["NOTE: member path open (1 of 1 hop(s) compose -> member without an answer at compose"
                          " within 1 ms: 00000002); a tap on that path can still arrive"], got["lines"])

    # Mutation run 4 (I9, escaped edges; re-check tv-bn-i-fix6 H1, mutant Y2):
    # `channels` is a whole path segment. A segment that only ends in it
    # (`subchannels`, or `x-channels`: a word boundary is no segment boundary)
    # is neither the display channel nor compose: a cell under
    # `subchannels/display` is a member, a `subchannels/display/compose` neither
    # opens a hop nor ends one.
    def test_a_segment_ending_in_channels_is_not_the_display_channel(self):
        for segment in ("subchannels", "x-channels"):
            with self.subTest(segment=segment):
                sub = "/org/members/m/%s/display" % segment
                got = self.paths([self.tap(),
                                  self.row(2, 1, self.COMPOSE, sub + "/in"),
                                  self.row(3, 2, sub + "/in", sub + "/compose", event="tap"),
                                  self.row(4, 1, sub + "/compose", "/org/members/m/cogny/in")]
                                 + self.handled_marker(), hang_ms=kit_time.OVER_MS)
                self.assertEqual({"hops": 1, "open": ["00000002"], "taps": [], "handled": True}, got["paths"])

    # Mechanical mutant class (fix4, L203/L212): with one hop per fixture, hops
    # and answers keyed by anything but their own id still counted right. Two
    # members answer the same tap: two hops, two taps back; one of them open.
    def test_two_hops_count_twice(self):
        cogny = "/org/members/m/cogny/in"
        hops = [self.tap(), self.row(2, 1, self.COMPOSE, self.MEMBER), self.row(3, 1, self.COMPOSE, cogny),
                self.row(4, 2, self.MEMBER, self.COMPOSE, event="tap")]
        got = self.paths(hops + [self.row(5, 3, cogny, self.COMPOSE, event="tap")] + self.handled_marker())
        self.assertEqual(2, got["paths"]["hops"])
        self.assertEqual([], got["paths"]["open"])
        self.assertEqual(["00000004", "00000005"], [t["id"] for t in got["paths"]["taps"]])
        got = self.paths(hops + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 2, "open": ["00000003"], "handled": True},
                         {k: got["paths"][k] for k in ("hops", "open", "handled")})
        self.assertEqual(["NOTE: member path open (1 of 2 hop(s) compose -> member without an answer at compose"
                          " within %d ms: 00000003); a tap on that path can still arrive" % kit_time.OVER_MS],
                         got["lines"])

    # Re-check tv-bn-i-fix4 M3: every fixture had one open hop of one, so the
    # counts and the separator of the NOTE went unpinned (TV-Q3: the last state).
    def test_the_open_note_names_how_many_of_how_many_hops_are_open(self):
        script = ("import { memberLines } from %s;\n"
                  "console.log(JSON.stringify(memberLines({ hops: 3, open: ['aaaa', 'bbbb'], taps: [],"
                  " handled: true }, %d)));" % (json.dumps((LAB / "tap.mjs").as_uri()), kit_time.OVER_MS))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        self.assertEqual(["NOTE: member path open (2 of 3 hop(s) compose -> member without an answer at compose"
                          " within %d ms: aaaa bbbb); a tap on that path can still arrive" % kit_time.OVER_MS],
                         json.loads(done.stdout))

    def test_a_handled_marker_without_a_hop_is_a_whole_reading(self):
        got = self.paths([self.tap()] + self.handled_marker(), hang_ms=kit_time.OVER_MS)
        self.assertEqual({"hops": 0, "open": [], "taps": [], "handled": True}, got["paths"])
        self.assertEqual([], got["lines"])

    # Re-check tv-bn-i-fix2 M1: the wait in `main` ended on "no hop open"
    # alone in fix1; with `handled` gone from its condition every pin above
    # stayed green. `awaitMemberPaths` is that wait, driven here by a fake
    # `read` whose readings come in order (the last one repeats).
    def wait(self, readings, hang_ms):
        script = (
            "import { awaitMemberPaths } from %s;\n"
            "const readings = %s;\n"
            "let reads = 0;\n"
            "const read = async () => readings[Math.min(reads++, readings.length - 1)];\n"
            "const got = await awaitMemberPaths(read, %d, 0);\n"
            "console.log(JSON.stringify({ reads, ...got }));"
            % (json.dumps((LAB / "tap.mjs").as_uri()), json.dumps(readings), hang_ms))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    @staticmethod
    def reading(handled, overflow=0, open=(), fresh=2):
        return {"member": {"handled": handled, "open": list(open), "hops": len(open), "taps": []},
                "overflow": overflow, "fresh": fresh}

    def test_the_wait_ends_only_once_compose_has_handled_the_marker(self):
        got = self.wait([self.reading(False), self.reading(True, fresh=3)], hang_ms=kit_time.HANG_MS)
        self.assertEqual(2, got["reads"])
        self.assertEqual([], got["notes"])
        self.assertEqual(["hangs", "notes", "reading", "reads"], sorted(got))
        self.assertTrue(got["reading"]["member"]["handled"])
        # Re-check tv-bn-i-fix4 M6: the wait hands back the reading it judged.
        self.assertEqual(self.reading(True, fresh=3), got["reading"])

    def test_a_marker_never_handled_ends_the_wait_with_its_note(self):
        got = self.wait([self.reading(False)], hang_ms=kit_time.OVER_MS)
        self.assertEqual(["NOTE: member path open (compose has not handled the marker within 1 ms; a hop"
                          " compose -> member it has not logged yet can still open one)"], got["notes"])

    # Re-check tv-bn-i-fix3 M1: every reading above had `open: []`, so the wait
    # ending on `handled` alone stayed green. A handled marker with a hop still
    # waiting for its answer at compose is no whole reading: the wait goes on.
    def test_the_wait_goes_on_while_a_hop_is_open(self):
        got = self.wait([self.reading(True, open=["h1"]), self.reading(True)], hang_ms=kit_time.HANG_MS)
        self.assertEqual(2, got["reads"])
        self.assertEqual([], got["notes"])
        self.assertEqual([], got["hangs"])
        self.assertEqual([], got["reading"]["member"]["open"])

    def test_a_hop_open_at_the_deadline_ends_the_wait_with_its_note(self):
        got = self.wait([self.reading(True, open=["h1"])], hang_ms=kit_time.OVER_MS)
        self.assertEqual(["NOTE: member path open (1 of 1 hop(s) compose -> member without an answer at compose"
                          " within 1 ms: h1); a tap on that path can still arrive"], got["notes"])
        self.assertEqual([], got["hangs"])

    # Re-check tv-bn-i-fix2 H4: newest rows first (`colony_dispatch.rs`,
    # `ORDER BY created_at DESC`), so an overflowing window loses the tap row
    # first and still holds the marker's child -- `hops=0 handled=true`, a
    # silent whole reading. An overflow ends the wait at once; since fix3 H1 it
    # is a `HANG:` line that marks the run (exit 1), no longer a NOTE.
    def test_an_overflowed_window_ends_the_wait_with_its_hang_line(self):
        got = self.wait([self.reading(True, overflow=300, fresh=7)], hang_ms=kit_time.HANG_MS)
        self.assertEqual(1, got["reads"])
        self.assertEqual([overflow_line(300, 7)], got["hangs"])
        self.assertEqual([], got["notes"])
        self.assertEqual(self.reading(True, overflow=300, fresh=7), got["reading"])

    # Re-check tv-bn-i-fix4 M6: the reading the deadline judged is handed back.
    def test_the_wait_hands_back_the_reading_its_deadline_judged(self):
        got = self.wait([self.reading(True, open=["h1"], fresh=4)], hang_ms=kit_time.OVER_MS)
        self.assertEqual(self.reading(True, open=["h1"], fresh=4), got["reading"])

    # Re-check tv-bn-i-fix4 H3: the overflow is judged before the deadline. A
    # deadline already behind (`hangMs` -1) is past at the first check, with no
    # clock to fake; with the two checks swapped the overflow became a NOTE and
    # exit 0 (swap mutant survived all 60 tests at b4a1f45b0).
    def test_an_overflow_past_the_deadline_is_still_a_hang_line(self):
        got = self.wait([self.reading(True, overflow=300, fresh=7)], hang_ms=kit_time.PAST_MS)
        self.assertEqual({"reads": 1, "notes": [], "hangs": [overflow_line(300, 7)],
                          "reading": self.reading(True, overflow=300, fresh=7)}, got)

    def overflowed(self, rows, before, limit):
        script = (
            "import { windowOverflowed } from %s;\n"
            "console.log(JSON.stringify(windowOverflowed(%s, new Set(%s), %d)));"
            % (json.dumps((LAB / "tap.mjs").as_uri()), json.dumps([{"id": r} for r in rows]),
               json.dumps(before), limit))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_a_window_that_still_holds_a_row_from_before_holds_every_fresh_one(self):
        self.assertFalse(self.overflowed(["c", "b", "a"], ["a"], 3))

    def test_a_full_window_without_a_row_from_before_overflowed(self):
        self.assertTrue(self.overflowed(["e", "d", "c"], ["b", "a"], 3))

    def test_a_window_that_lost_every_row_from_before_overflowed(self):
        self.assertTrue(self.overflowed(["d"], ["b", "a"], 3))
        # One row from before is enough to lose (mechanical mutant fix4 L251).
        self.assertTrue(self.overflowed(["c"], ["a"], 3))

    def test_a_log_shorter_than_the_window_is_whole(self):
        self.assertFalse(self.overflowed(["b", "a"], [], 3))
        self.assertTrue(self.overflowed(["c", "b", "a"], [], 3))

    # Re-check tv-bn-i-fix3 M2 and fix4 M5: a pin on the call alone let `main`
    # drop its result, and a pin on its text let a commented-out statement and an
    # added one through. `main` decides nothing: every decision is a
    # behaviour-pinned function (`TapReadingTest`, `TapMarkerOrderTest`), the
    # lines and the exit code included (`verdictOf`). Pinned here, on the source
    # without comments: each wiring statement exactly once, and how often each
    # value that carries a verdict is named -- a statement added to empty,
    # replace or re-read one changes a count.
    def test_main_waits_through_the_pinned_functions(self):
        src = (LAB / "tap.mjs").read_text(encoding="utf-8")
        main = src[src.index("async function main()"):src.index("function startedAsProgram()")]
        main = re.sub(r"/\*.*?\*/", "", main, flags=re.S)
        main = re.sub(r"(^|\s)//.*$", "", main, flags=re.M)
        for statement in (
            "const limit = windowLimit(flags.limit);",
            "const api = messagesUrl(base, limit);",
            "return readingOf((await (await fetch(api)).json()).messages, before, limit, bodyOf);",
            "const before = await snapIds();",
            "const mark = await page.evaluate(markBehindTheGesture, { name: MARKER, hangMs: HANG_MS });",
            "const markerRef = mark.ref;",
            "const held = await page.evaluate(heldAttrs, tile);",
            "const sentOf = () => page.evaluate(sentSince, markerRef);",
            "const after = await readAfterGesture({ sentOf, read: () => newTaps(before), markerRef, hangMs: HANG_MS,"
            " tickMs: POLL_TICK_MS });",
            "const verdict = verdictOf({ mark, held, after, hangMs: HANG_MS });",
            "for (const line of verdict.lines) console.log(line);",
            "process.exitCode = verdict.exitCode;",
            "const taps = after.reading;",
            "console.log(colonyLine(taps));",
        ):
            self.assertEqual(1, main.count(statement), statement)
        for name, n in (("verdict", 3), ("after", 3), ("mark", 3), ("held", 2), ("sentOf", 2), ("markerRef", 5),
                        ("newTaps", 2), ("limit", 4), ("api", 4), ("before", 4), ("taps", 4), ("hangs", 0), ("notes", 0),
                        ("awaitMemberPaths", 0), ("awaitSettled", 0), ("exitCode", 2), ("POLL_TICK_MS", 1), ("tickMs", 1)):
            self.assertEqual(n, len(re.findall(r"\b%s\b" % name, main)), name)


def overflow_line(limit, fresh):
    return ("HANG: log window overflowed (limit %d; last state: fresh=%d, none from before): the rows logged first"
            " after the gesture can be missing, so this is no whole reading" % (limit, fresh))



class TapReadingTest(unittest.TestCase):
    """Re-check tv-bn-i-fix3 M2/H1 and the mutant class behind them: every
    decision `tap.mjs` takes after the gesture is a function a fake drives here
    (no browser, no colony) -- the reading of one log window (`readingOf`), the
    window's real limit (`windowLimit`), the tile's held frames (`heldLines`),
    the settle wait (`awaitSettled`) and the two waits in order
    (`readAfterGesture`). An overflowed window is a `HANG:` line (fix3 H1): no
    whole reading, so the run is marked."""

    def node(self, script):
        done = subprocess.run(
            ["node", "--input-type=module", "-e",
             "import * as t from %s;\n%s" % (json.dumps((LAB / "tap.mjs").as_uri()), script)],
            capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    COMPOSE = "/org/members/m/channels/display/compose"

    def row(self, rid, parent, event=None, frm="@external", to=COMPOSE):
        body = {"event": {"name": event, "value": {"for": "view.chat"}}} if event else {"text": "x"}
        return {"id": "row-%08d" % rid, "parent_message_id": parent and "row-%08d" % parent,
                "created_at": "2026-10-12T06:20:%02d.000Z" % rid, "from_path": frm, "to_path": to,
                "headers_json": json.dumps({"hop": {"route": "event"}}),
                "body_kind": "inline", "body_payload": json.dumps(body)}

    def reading_of(self, rows, before, limit):
        return self.node(
            "const bodyOf = (r) => JSON.parse(r.body_payload);\n"
            "console.log(JSON.stringify(t.readingOf(%s, new Set(%s), %d, bodyOf)));"
            % (json.dumps(rows), json.dumps(before), limit))

    def test_a_reading_counts_only_the_rows_after_the_gesture(self):
        rows = [self.row(9, 8, frm=self.COMPOSE, to="/org/members/m/channels/display/web"),
                self.row(8, None, event="tap-probe-marker"),
                self.row(3, 2, event="tap", frm="/org/members/m/talky/in"),
                self.row(2, 1, frm=self.COMPOSE, to="/org/members/m/talky/in"),
                self.row(1, None, event="tap"),
                self.row(11, 12, frm=self.COMPOSE, to="/org/members/m/talky/in"),
                self.row(10, None, event="tap-probe-marker"),
                self.row(12, None, event="tap")]
        # Rows 10 to 12 are from before the gesture: an earlier tap with a hop
        # still open and an earlier marker. None of them counts.
        got = self.reading_of(rows, ["row-00000010", "row-00000011", "row-00000012"], 300)
        self.assertEqual(0, got["overflow"])
        self.assertEqual(5, got["fresh"])
        self.assertEqual(["00000003", "00000001"], [x["id"] for x in got["taps"]])
        self.assertEqual(1, got["markers"])
        self.assertEqual({"hops": 1, "open": [], "handled": True}, {k: got["member"][k] for k in ("hops", "open", "handled")})
        self.assertEqual(["00000003"], [x["id"] for x in got["member"]["taps"]])

    def test_an_overflowed_reading_names_its_limit(self):
        got = self.reading_of([self.row(5, None), self.row(4, None)], ["row-00000001"], 2)
        self.assertEqual(2, got["overflow"])
        # A log shorter than the window is whole: the guard counts with the
        # window's own limit (mechanical mutant fix4 L307: limit -> null).
        got = self.reading_of([self.row(5, None), self.row(4, None)], [], 300)
        self.assertEqual(0, got["overflow"])

    def test_the_window_limit_is_clamped_like_the_api(self):
        got = self.node("console.log(JSON.stringify([undefined, '50', 5000, '-5'].map(t.windowLimit)));")
        self.assertEqual([300, 50, 1000, 1], got)

    def test_a_held_frame_is_a_note(self):
        got = self.node("console.log(JSON.stringify([t.heldLines(['phx-debounce']), t.heldLines([])]));")
        self.assertEqual([["NOTE: the tile carries phx-debounce; a frame it holds back can follow the marker"], []], got)

    def drive(self, fn, sents, readings, marker_ref, hang_ms):
        return self.node(
            "const sents = %s, readings = %s;\n"
            "let s = 0, reads = 0;\n"
            "const sentOf = async () => sents[Math.min(s++, sents.length - 1)];\n"
            "const read = async () => readings[Math.min(reads++, readings.length - 1)];\n"
            "const got = %s;\n"
            "console.log(JSON.stringify({ reads, got }));"
            % (json.dumps(sents), json.dumps(readings),
               fn % {"ref": json.dumps(marker_ref), "hang": hang_ms}))

    @staticmethod
    def seen(taps=0, markers=1, overflow=0, handled=True, open=(), tag=0):
        # `tag` tells the readings apart: a wait hands back the one it judged.
        return {"overflow": overflow, "fresh": taps + markers, "taps": [{"id": "t%d" % i} for i in range(taps)],
                "markers": markers, "tag": tag,
                "member": {"handled": handled, "open": list(open), "hops": len(open), "taps": []}}

    def settle(self, sents, readings, marker_ref="tap-probe-marker", hang_ms=kit_time.HANG_MS):
        return self.drive("await t.awaitSettled(sentOf, read, %(ref)s, %(hang)d, 0)",
                          sents, readings, marker_ref, hang_ms)

    def test_the_settle_wait_waits_for_the_marker(self):
        got = self.settle([{"events": 0, "heard": 0}], [self.seen(markers=0), self.seen(markers=1, tag=1)])
        self.assertEqual({"reads": 2, "got": {"hangs": [], "overflow": 0, "reading": self.seen(markers=1, tag=1)}},
                         got)

    def test_without_a_marker_the_settle_wait_asks_for_none(self):
        got = self.settle([{"events": 0, "heard": 0}], [self.seen(markers=0)], marker_ref=None)
        self.assertEqual({"reads": 1, "got": {"hangs": [], "overflow": 0, "reading": self.seen(markers=0)}}, got)

    def test_the_settle_wait_waits_until_every_sent_event_is_logged(self):
        got = self.settle([{"events": 2, "heard": 1}], [self.seen(taps=1), self.seen(taps=2)])
        self.assertEqual(2, got["reads"])
        self.assertEqual([], got["got"]["hangs"])

    def test_the_settle_wait_waits_for_the_answer_of_a_sent_event(self):
        got = self.settle([{"events": 1, "heard": 0}, {"events": 1, "heard": 1}], [self.seen(taps=1)])
        self.assertEqual(2, got["reads"])
        self.assertEqual([], got["got"]["hangs"])

    def test_a_gesture_that_sent_nothing_needs_no_answer(self):
        got = self.settle([{"events": 0, "heard": 0}], [self.seen(taps=0)])
        self.assertEqual(1, got["reads"])
        self.assertEqual([], got["got"]["hangs"])

    def test_a_settle_wait_that_never_settles_is_a_hang_line(self):
        got = self.settle([{"events": 1, "heard": 0}], [self.seen(taps=0, markers=0, tag=5)], hang_ms=kit_time.OVER_MS)
        self.assertEqual(['HANG: the gesture never settled (no answer within 1 ms); last state: '
                          '{"events":1,"heard":0,"logged":0,"marker":0}'], got["got"]["hangs"])
        self.assertEqual(0, got["got"]["overflow"])
        self.assertEqual(self.seen(taps=0, markers=0, tag=5), got["got"]["reading"])

    # Re-check tv-bn-i-fix3 H1: an overflow broke the settle loop with a NOTE
    # and exit 0. It is a `HANG:` line now and ends the wait at once; since fix4
    # H4 it names its last state (the fresh rows).
    def test_an_overflow_ends_the_settle_wait_with_its_hang_line(self):
        reading = self.seen(taps=2, markers=0, overflow=300)
        got = self.settle([{"events": 1, "heard": 0}], [reading])
        self.assertEqual({"reads": 1, "got": {"hangs": [overflow_line(300, 2)], "overflow": 300,
                                              "reading": reading}}, got)

    # Re-check tv-bn-i-fix4 H3: overflow, then success, then the deadline. A
    # deadline already behind (`hangMs` -1) is past at the first check, with no
    # clock to fake; either swap survived all 60 tests at b4a1f45b0.
    def test_an_overflow_past_the_deadline_ends_the_settle_wait_with_its_hang_line(self):
        reading = self.seen(taps=2, markers=0, overflow=300)
        got = self.settle([{"events": 1, "heard": 0}], [reading], hang_ms=kit_time.PAST_MS)
        self.assertEqual({"reads": 1, "got": {"hangs": [overflow_line(300, 2)], "overflow": 300,
                                              "reading": reading}}, got)

    def test_a_reading_settled_past_the_deadline_is_settled(self):
        got = self.settle([{"events": 0, "heard": 0}], [self.seen(tag=1)], hang_ms=kit_time.PAST_MS)
        self.assertEqual({"reads": 1, "got": {"hangs": [], "overflow": 0, "reading": self.seen(tag=1)}}, got)

    def after(self, sents, readings, hang_ms=kit_time.HANG_MS):
        return self.drive("await t.readAfterGesture({ sentOf, read, markerRef: %(ref)s, hangMs: %(hang)d, tickMs: 0 })",
                          sents, readings, "tap-probe-marker", hang_ms)

    # Re-check tv-bn-i-fix5 H1: the poll tick. Each wait sleeps `tickMs` between
    # two reads and never before the first or after the last -- the read rate,
    # no verdict -- and `main` passes `POLL_TICK_MS` (pinned there). With a tick
    # dropped, zeroed or lost on the way, a wait polled `/colony/messages`
    # without a pause until `HANG_MS`, and every verdict stayed the same. A fake
    # `setTimeout` records each delay and runs at once: no clock is read.
    def test_both_waits_sleep_one_tick_between_two_reads(self):
        got = self.node(
            "const delays = [], run = globalThis.setTimeout;\n"
            "globalThis.setTimeout = (f, ms, ...a) => { delays.push(ms); return run(f, 0, ...a); };\n"
            "const readings = %s;\n"
            "let reads = 0;\n"
            "const read = async () => readings[Math.min(reads++, readings.length - 1)];\n"
            "const sentOf = async () => ({ events: 0, heard: 0 });\n"
            "const after = await t.readAfterGesture({ sentOf, read, markerRef: 'tap-probe-marker',"
            " hangMs: %d, tickMs: 7 });\n"
            "const afterReads = reads;\n"
            "reads = 0;\n"
            "await t.awaitSettled(sentOf, read, 'tap-probe-marker', %d, 5);\n"
            "console.log(JSON.stringify({ afterReads, reads, delays, after: after.reading.tag }));"
            % (json.dumps([self.seen(markers=0, tag=1), self.seen(tag=2), self.seen(open=["h1"], tag=3),
                           self.seen(tag=4)]), kit_time.HANG_MS, kit_time.HANG_MS))
        self.assertEqual({"afterReads": 4, "reads": 2, "delays": [7, 7, 5], "after": 4}, got)

    # Mutation run 3 (I8, AWAIT): a tick that is set but not awaited is no pause
    # -- the delays above stayed right while the wait read on at once. A fake
    # `setTimeout` that holds the tick back: until it fires, each wait has read
    # exactly once.
    def test_a_wait_reads_again_only_once_its_tick_fired(self):
        got = self.node(
            "const held = [];\n"
            "globalThis.setTimeout = (f) => { held.push(f); return 0; };\n"
            "const turn = () => new Promise((r) => setImmediate(r));\n"
            "const readings = %s;\n"
            "const out = [];\n"
            "for (const wait of ['member', 'settle']) {\n"
            "  let reads = 0;\n"
            "  const read = async () => readings[Math.min(reads++, readings.length - 1)];\n"
            "  const sentOf = async () => ({ events: 0, heard: 0 });\n"
            "  const done = wait === 'member' ? t.awaitMemberPaths(read, %d, 7)"
            " : t.awaitSettled(sentOf, read, 'tap-probe-marker', %d, 7);\n"
            "  await turn(); await turn();\n"
            "  const before = reads;\n"
            "  held.splice(0).forEach((f) => f());\n"
            "  await done;\n"
            "  out.push([wait, before, reads]);\n"
            "}\n"
            "console.log(JSON.stringify(out));"
            % (json.dumps([self.seen(markers=0, open=["h1"], tag=1), self.seen(tag=2)]), kit_time.HANG_MS,
               kit_time.HANG_MS))
        self.assertEqual([["member", 1, 2], ["settle", 1, 2]], got)

    def test_the_poll_tick_is_100_ms(self):
        self.assertEqual(100, self.node("console.log(JSON.stringify(t.POLL_TICK_MS));"))

    def test_an_overflow_while_settling_skips_the_member_wait(self):
        got = self.after([{"events": 0, "heard": 0}], [self.seen(overflow=300, tag=1), self.seen(tag=2)])
        self.assertEqual({"reads": 1, "got": {"hangs": [overflow_line(300, 1)], "notes": [],
                                              "reading": self.seen(overflow=300, tag=1)}}, got)

    # Re-check tv-bn-i-fix4 M6: `main` read a third time after both waits and
    # printed that reading, whose overflow nobody judged. The reading it prints
    # is the last one a wait judged.
    def test_the_member_wait_follows_the_settle_wait(self):
        got = self.after([{"events": 0, "heard": 0}],
                         [self.seen(tag=1), self.seen(open=["h1"], tag=2), self.seen(tag=3), self.seen(tag=4)])
        self.assertEqual({"reads": 3, "got": {"hangs": [], "notes": [], "reading": self.seen(tag=3)}}, got)

    def test_an_overflow_in_the_member_wait_is_a_hang_line(self):
        got = self.after([{"events": 0, "heard": 0}], [self.seen(), self.seen(taps=3, overflow=300, tag=2)])
        self.assertEqual({"reads": 2, "got": {"hangs": [overflow_line(300, 4)], "notes": [],
                                              "reading": self.seen(taps=3, overflow=300, tag=2)}}, got)

    def test_both_waits_report_what_they_saw(self):
        got = self.after([{"events": 1, "heard": 0}], [self.seen(taps=0, handled=False)], hang_ms=kit_time.OVER_MS)
        self.assertEqual(['HANG: the gesture never settled (no answer within 1 ms); last state: '
                          '{"events":1,"heard":0,"logged":0,"marker":1}'], got["got"]["hangs"])
        self.assertEqual(["NOTE: member path open (compose has not handled the marker within 1 ms; a hop"
                          " compose -> member it has not logged yet can still open one)"], got["got"]["notes"])

    # Re-check tv-bn-i-fix4 M4: every reading above had `markers=1`, so a
    # `readAfterGesture` that dropped the marker's ref stayed green. A marker
    # that never reaches compose is the settle HANG (`"marker":0`) plus the
    # compose NOTE -- exit 1, never a NOTE alone with exit 0.
    def test_a_marker_that_never_arrives_is_a_hang_line(self):
        got = self.after([{"events": 0, "heard": 0}], [self.seen(markers=0, handled=False)], hang_ms=kit_time.OVER_MS)
        self.assertEqual(['HANG: the gesture never settled (no answer within 1 ms); last state: '
                          '{"events":0,"heard":0,"logged":0,"marker":0}'], got["got"]["hangs"])
        self.assertEqual(["NOTE: member path open (compose has not handled the marker within 1 ms; a hop"
                          " compose -> member it has not logged yet can still open one)"], got["got"]["notes"])

    # Re-check tv-bn-i-fix4 M5: `main` printed the lines and set the exit code
    # itself. `verdictOf` is that decision: the HANG lines (marker, then the
    # waits), then the NOTE lines (held frames, then the member wait); a HANG
    # is exit 1.
    def verdict(self, mark, held, after):
        return self.node("console.log(JSON.stringify(t.verdictOf({ mark: %s, held: %s, after: %s, hangMs: %d })));"
                         % (json.dumps(mark), json.dumps(held), json.dumps(after), kit_time.OVER_MS))

    def test_the_verdict_prints_hangs_then_notes_and_a_hang_exits_1(self):
        got = self.verdict({"ref": None, "framed": False}, ["phx-debounce"],
                           {"hangs": ["HANG: a"], "notes": ["NOTE: b"], "reading": {}})
        self.assertEqual({"lines": ["HANG: no open LiveView channel to send the marker on",
                                    "HANG: no frame within 1 ms before the marker", "HANG: a",
                                    "NOTE: the tile carries phx-debounce; a frame it holds back can follow the marker",
                                    "NOTE: b"], "exitCode": 1}, got)

    def test_notes_alone_exit_0(self):
        got = self.verdict({"ref": "tap-probe-marker", "framed": True}, ["phx-throttle"],
                           {"hangs": [], "notes": ["NOTE: b"], "reading": {}})
        self.assertEqual({"lines": ["NOTE: the tile carries phx-throttle; a frame it holds back can follow the marker",
                                    "NOTE: b"], "exitCode": 0}, got)

    def test_a_wait_hang_alone_exits_1(self):
        got = self.verdict({"ref": "tap-probe-marker", "framed": None}, [], {"hangs": ["HANG: a"], "notes": []})
        self.assertEqual({"lines": ["HANG: a"], "exitCode": 1}, got)

    # Re-check tv-bn-i-fix4 M5: the window in the URL is the window the overflow
    # guard counts with.
    def test_the_messages_url_carries_the_window_limit(self):
        got = self.node("console.log(JSON.stringify(t.messagesUrl('http://127.0.0.1:1', t.windowLimit('5000'))));")
        self.assertEqual("http://127.0.0.1:1/colony/messages?limit=1000", got)

    def test_the_colony_line_names_the_reading(self):
        reading = {"fresh": 7, "taps": [{"id": "a"}, {"id": "b"}], "markers": 1,
                   "member": {"handled": True, "hops": 3, "open": ["x"], "taps": [{"id": "b"}]}}
        got = self.node("console.log(JSON.stringify(t.colonyLine(%s)));" % json.dumps(reading))
        self.assertEqual("colony: fresh=7 taps=2 markers=1 member handled=true hops=3 open=1 taps via member=1", got)


class ThrottleLineTest(unittest.TestCase):
    """Re-check tv-bn-drv2-fix1 § 5 (TV-B7): a driver that brakes its page says so
    on stderr (`THROTTLE ...`, `workshop/tools/lib/cpu-throttle.mjs`), and every
    Rust test that runs such a driver captured that stderr and dropped it on a
    green run -- a braked probe and an unbraked one read the same in the gate log.
    Each such test prints the line, and nextest shows it when green."""

    TESTS = REPO / "crates" / "meclaw-cells" / "tests"

    def braking_drivers(self):
        tools = REPO / "workshop" / "tools"
        for path in sorted(tools.glob("*.mjs")):
            if "cpu-throttle.mjs" in path.read_text(encoding="utf-8"):
                yield "workshop/tools/" + path.name

    def callers(self, driver):
        for path in sorted(self.TESTS.rglob("*.rs")):
            if driver in path.read_text(encoding="utf-8"):
                yield path

    def test_every_caller_of_a_braking_driver_prints_its_throttle_line(self):
        nextest = (REPO / ".config" / "nextest.toml").read_text(encoding="utf-8")
        loud = [b for b in nextest.split("[[profile.default.overrides]]")[1:]
                if 'success-output = "immediate-final"' in b.split("[[")[0]]
        seen = 0
        for driver in self.braking_drivers():
            for path in self.callers(driver):
                seen += 1
                with self.subTest(driver=driver, test=path.name):
                    self.assertIn('l.starts_with("THROTTLE ")', path.read_text(encoding="utf-8"))
                    self.assertTrue(any(path.stem in b for b in loud),
                                    "%s: its green output is not in the gate log" % path.stem)
        self.assertGreaterEqual(seen, 3)


class CountdownSettledTest(unittest.TestCase):
    """GH #1078: B-21's question to the page while it watches the countdown
    (`countdownSettled`, `display-browser-lab.mjs`), driven by a fake page -- no
    browser. The proof is decided by the page's own evidence: the tile it measured
    (by `data-for`, never the first tile with the attribute: the product writes
    `data-end-at="0"` on every tile that counts nothing) and a write of the seconds
    in a task that began a page second after the label was read (`countdownWatch`
    stamps every task that changed the dock). A tick count is no second: `updated`
    ticks on every patch."""

    FAKE = r"""
const tile = (id, end, label) => ({
  getAttribute: (n) => (n === "data-for" ? id : n === "data-end-at" ? end : null),
  querySelector: (s) => (s === '[data-role="remaining"]' && label !== null ? { textContent: label } : null),
});
const page = (tiles, scene, watch) => {
  globalThis.window = globalThis;
  globalThis.__displayScene = scene;
  globalThis.__countdownWatch = watch;
  globalThis.document = {
    querySelectorAll: (s) => (s === ".display-tile[data-for]" ? tiles : []),
    querySelector: (s) => (s === ".display-tile[data-end-at]:not([data-end-at=''])"
      ? tiles.find((t) => t.getAttribute("data-end-at") !== "") || null : null),
  };
};
"""

    # The label `was` and the page time `at0` it was read at; the old question took
    # `t0`, the scene's tick count at that moment.
    ARG = {"id": "c1", "was": "01:52", "t0": 6, "at0": 100000}
    TIMER = 'tile("c1", "1790000180000", "%s")'
    TILES = '[tile("w1", "0", null), ' + TIMER % "01:52" + ']'

    def ask(self, tiles, ticks, writes):
        uri = json.dumps((REPO / "workshop" / "tools" / "display-browser-lab.mjs").as_uri())
        watch = dict(self.ARG, writes=writes)
        script = ("import { countdownSettled } from %s;\n%s\npage(%s, %s, %s);\n"
                  "console.log(JSON.stringify(countdownSettled(%s)));"
                  % (uri, self.FAKE, tiles, json.dumps({"ticks": ticks}), json.dumps(watch),
                     json.dumps(self.ARG)))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def test_a_tile_that_counts_nothing_ahead_of_it_is_not_read(self):
        # The product's dock: `data-end-at="0"` on the window tile ahead of the timer.
        got = self.ask(self.TILES, 6, [])
        self.assertIs(False, got, "B-21 read a tile without seconds: %r" % (got,))

    def test_four_ticks_inside_one_second_of_the_page_decide_nothing(self):
        # Four patches (`updated` ticks each) within 400 ms of the reading; the timer is
        # the only tile, so nothing but the ticks is asked here.
        writes = [{"at": 100000 + 100 * i, "wrote": True, "label": "01:52"} for i in range(1, 5)]
        got = self.ask("[%s]" % (self.TIMER % "01:52"), 10, writes)
        self.assertIs(False, got, "four ticks inside one page second judged: %r" % (got,))

    def test_the_write_that_crossed_the_second_is_not_yet_the_proof(self):
        # The stamp at +1000 is the END of its task; its clock may have read +999.
        got = self.ask(self.TILES, 7, [{"at": 101000, "wrote": True, "label": "01:52"}])
        self.assertIs(False, got, "a write that may have read the old second judged: %r" % (got,))

    def test_a_write_after_a_page_second_with_the_same_label_is_read_as_stuck(self):
        writes = [{"at": 101000, "wrote": False, "label": "01:52"},
                  {"at": 101004, "wrote": True, "label": "01:52"}]
        got = self.ask(self.TILES, 7, writes)
        self.assertEqual("01:52", got["label"])

    def test_a_moved_label_is_read_at_once(self):
        tiles = '[tile("w1", "0", null), ' + self.TIMER % "01:51" + ']'
        got = self.ask(tiles, 7, [{"at": 100300, "wrote": True, "label": "01:51"}])
        self.assertEqual("01:51", got["label"])

    def test_the_measured_tile_losing_its_seconds_is_read_at_once(self):
        tiles = '[tile("w1", "0", null), tile("c1", "1790000180000", null)]'
        got = self.ask(tiles, 6, [])
        self.assertIsNone(got["label"])


class CountdownOpeningTest(unittest.TestCase):
    """GH #1078, re-check I1: what B-21 makes of its first reading (`countdownWatch`)
    before it waits (`countdownOpening`, `display-browser-lab.mjs`). A counted tile
    whose end lies within half a second is RED with a reason of its own -- every line
    sets the countdown minutes ahead (stage +120 s, colony fixture +10 min), so such an
    end means a wrong `end_at` (a unit in seconds, a stale value) or a clock that stood
    until the end passed; an earlier `partial` let both pass without looking at the
    label."""

    def opening(self, watch):
        uri = json.dumps((REPO / "workshop" / "tools" / "display-browser-lab.mjs").as_uri())
        script = ("import { countdownOpening } from %s;\n"
                  "console.log(JSON.stringify(countdownOpening(%s)));" % (uri, json.dumps(watch)))
        done = subprocess.run(["node", "--input-type=module", "-e", script],
                              capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, done.returncode, done.stderr[-600:])
        return json.loads(done.stdout)

    def assert_red(self, got):
        self.assertIsInstance(got, dict, got)
        self.assertNotIn("partial", got, "an ended countdown passed as partial: %r" % (got,))
        self.assertIn("end_at is wrong or the clock stood", got.get("why", ""))

    def test_an_end_in_the_past_is_red_with_its_own_reason(self):
        # `end_at` in seconds read as milliseconds lies in 1970; the label is `00:00`.
        self.assert_red(self.opening({"scene": True, "was": "00:00", "at0": 1790000000000,
                                      "left": 1790000180 - 1790000000000}))

    def test_a_clock_that_stood_until_the_end_passed_is_red(self):
        self.assert_red(self.opening({"scene": True, "was": "00:07", "at0": 1000, "left": -4000}))

    def test_an_end_within_half_a_second_is_red(self):
        self.assert_red(self.opening({"scene": True, "was": "00:00", "at0": 1000, "left": 500}))

    def test_an_unreadable_end_is_red(self):
        self.assert_red(self.opening({"scene": True, "was": "01:52", "at0": 1000, "left": None}))

    def test_a_countdown_minutes_ahead_goes_on_to_wait(self):
        self.assertIsNone(self.opening({"scene": True, "was": "01:52", "at0": 1000, "left": 112000}))

    def test_a_tile_without_seconds_is_red(self):
        got = self.opening({"scene": True, "was": None, "at0": 1000, "left": 112000})
        self.assertEqual("the counting tile carries no seconds (D-27)", got.get("why"))

    def test_a_page_without_the_scene_hook_is_not_judged(self):
        got = self.opening({"scene": False, "was": "01:52", "at0": 1000, "left": 112000})
        self.assertIn("partial", got)


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
