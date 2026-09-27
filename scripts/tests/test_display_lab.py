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
    "worker_stalls.py": "python3",
    "identity_probe.py": "python3",
    "sidecar_quote.py": "python3",
}

# `callers/` holds TEMPLATES, not bound callers: a caller names one colony's
# port, root and member path, and none of those three belong in a public tree.
# The orchestrator substitutes the placeholders when it plants a caller.
# `headers.py` reads a colony's database file, not its port.
# `worker_stalls.py` reads one process's `/proc`, not a port (GH #866).
# `identity_probe.py` reads one colony's tree, not its port (GH #872).
# `sidecar_quote.py` reads a colony's database and one brain's hops (GH #871).
PLACEHOLDERS = {"headers.py": ("@LAB@", "@DB@"), "worker_stalls.py": ("@LAB@", "@PID@"),
                "identity_probe.py": ("@LAB@", "@ROOT@"),
                "sidecar_quote.py": ("@LAB@", "@DB@", "@BRAIN@")}
DEFAULT_PLACEHOLDERS = ("@LAB@", "@PORT@")

# The one argument without which a tool refuses (exit 2); `--port` unless named.
MANDATORY = {"headers.py": "--db", "worker_stalls.py": "--pid", "identity_probe.py": "--root",
             "sidecar_quote.py": "--db"}

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
                capture_output=True, text=True, timeout=60)
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
                                  capture_output=True, text=True, timeout=60)
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
                              capture_output=True, text=True, timeout=60)
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

    def tree(self, tmp, packs=None, acks=None, slot_len=None, extra_brain_db=False):
        """A colony root: `colony.db`, three brain `cell.db`s, one affinity store.

        `packs` / `acks`: brain -> list of (created_at, error_code) rows; default
        one clean pack at BIRTH+30 and one clean ack at BIRTH+31 for every brain.
        `slot_len`: (brain, slot) -> length of the stored value (default: the
        secret's length).
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
        for brain in self.BRAINS:
            d = main / "assistants" / self.ASSISTANT / brain / "brain"
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
        if extra_brain_db:
            d = main / "assistants" / self.ASSISTANT / "talky" / "twin"
            d.mkdir(parents=True)
            c = sqlite3.connect(str(d / "cell.db"))
            c.execute("CREATE TABLE system (slot_path TEXT PRIMARY KEY, value TEXT)")
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
        done = subprocess.run(args + list(extra), capture_output=True, text=True, timeout=60)
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
            done = self.probe(self.tree(tmp, extra_brain_db=True))
        self.assertEqual(2, done.returncode, done.stdout)
        self.assertIn("talky", done.stderr)


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
                                  capture_output=True, text=True, timeout=60, cwd=str(REPO))
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


if __name__ == "__main__":
    unittest.main()
