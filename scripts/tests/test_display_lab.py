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
import subprocess
import tempfile
import threading
import sys
import time
import unittest
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "display_lab_patches.json"
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
    "headers.py": "python3",
    "turn.py": "python3",
    "tap.mjs": "node",
    "markers.py": "python3",
    "duplex_proof.mjs": "node",
}

# `callers/` holds TEMPLATES, not bound callers: a caller names one colony's
# port, root and member path, and none of those three belong in a public tree.
# The orchestrator substitutes the placeholders when it plants a caller.
# `headers.py` reads a colony's database file, not its port.
PLACEHOLDERS = {"headers.py": ("@LAB@", "@DB@")}
DEFAULT_PLACEHOLDERS = ("@LAB@", "@PORT@")

# The one argument without which a tool refuses (exit 2); `--port` unless named.
MANDATORY = {"headers.py": "--db"}

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
            timeout=120,
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
        self.thread.join(timeout=10)


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
                capture_output=True, text=True, timeout=180,
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
                capture_output=True, text=True, timeout=180)
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
            capture_output=True, text=True, timeout=60)
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
                              capture_output=True, text=True, timeout=60)
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
                              capture_output=True, text=True, timeout=60)

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
                                  capture_output=True, text=True, timeout=60)
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
