"""Pins of the precheck check `wall-clock` (`scripts/wall_clock.py`, K-UHREN).

The session keeper's close cell read the wall clock for its night window
(22:00-04:00 UTC); about 24 session-close tests were red only inside it, every
gate until 06:00 local time. The testing bans read test code only. The check
grades the ADDED lines of product code: a direct wall-clock read is RED unless
the line names its test override (`wall-clock: <param|env>`) and the tree
carries that name.

Every pin builds a throw-away repository: a `master` commit with the stock,
then a strand branch with the working-tree change, and runs the precheck
itself (`precheck.run`) over it. The fixtures are Python strings; the check
reads product code only, so this file trips nothing.
"""

import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import store_isolation

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
sys.path.insert(0, str(SCRIPTS))

import precheck
import testing_bans
import wall_clock

# The ratchet's clock bans: the one list of the Rust wall-clock reads.
CLOCK_BANS = SCRIPTS.parent / ".github" / "gates" / "clock-bans" / "clippy.toml"
RATCHET = SCRIPTS.parent / ".github" / "gates" / "unwrap_budget.py"


def ratchet():
    """The unwrap ratchet as a module: its `read_bans` is the one reading of the list."""
    spec = importlib.util.spec_from_file_location("unwrap_budget_pin", RATCHET)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod

GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}

RS = "crates/meclaw-x/src/clock.rs"
PY = "templates/keeper/close/close.py"
JS = "templates/keeper/view/view.js"
CFG = "templates/keeper/close/config.json"

RS_STOCK = ("pub fn stamp() -> u64 {\n"
            "    std::time::SystemTime::now().elapsed().map(|d| d.as_secs()).unwrap_or(0)\n"
            "}\n")
SCRIPT_STOCK = ("import sys, json, datetime\n"
                "doc = json.load(sys.stdin)\n"
                "now = datetime.datetime.now(datetime.timezone.utc)\n"
                "print(json.dumps({'at': now.isoformat()}))\n")


def config(script, params=None):
    doc = {"type": "code", "params": dict(params or {}, runner="python3", script_inline=script)}
    return json.dumps(doc, indent=2) + "\n"


class Repo:
    """A master commit with the stock, then a strand branch."""

    def __init__(self, case, stock):
        tmp = tempfile.TemporaryDirectory()
        case.addCleanup(tmp.cleanup)
        self.root = os.path.realpath(tmp.name)
        self.git("init", "-q", "-b", "master")
        for rel, text in dict(stock, **{"README.md": "x\n"}).items():
            self.write(rel, text)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "base")
        self.git("checkout", "-q", "-b", "strand")

    def git(self, *args):
        env = dict(os.environ, **GIT_ENV)
        return subprocess.run(["git", "-C", self.root] + list(args), check=True,
                              capture_output=True, text=True, env=env).stdout

    def write(self, rel, text):
        path = pathlib.Path(self.root, rel)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def findings(self, *paths):
        return [f for f in precheck.run(list(paths), repo=self.root) if f.check == "wall-clock"]


class ANewReadIsRed(unittest.TestCase):
    """An added direct read without a marker: RED with file and line."""

    def test_a_new_rust_read_is_red_with_file_and_line(self):
        repo = Repo(self, {RS: "pub fn zero() -> u64 {\n    0\n}\n"})
        repo.write(RS, "pub fn zero() -> u64 {\n    0\n}\n"
                       "pub fn wall() -> std::time::SystemTime {\n"
                       "    std::time::SystemTime::now()\n}\n")
        found = repo.findings(RS)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn(RS + ":5:", found[0].message)
        self.assertIn("SystemTime::now", found[0].message)

    def test_the_rule_knows_every_wall_read_of_the_clock_bans(self):
        """Pin WC-B1-a (review B-M1): the Rust reads ARE the `wall:` lines of
        the clock bans -- read here from the file itself, each one RED on a
        synthetic product line, `SystemTime::elapsed` among them. The `wall:`
        lines are the ratchet's own reading of them (`read_bans`, M1)."""
        paths = sorted(p for p, column in ratchet().read_bans(CLOCK_BANS).items()
                       if column == "wall")
        self.assertIn("std::time::SystemTime::elapsed", paths)
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, "fn a() {}\n" + "".join("fn r%d() { let _ = %s(); }\n" % (i, p)
                                              for i, p in enumerate(paths)))
        found = repo.findings(RS)
        self.assertEqual([f.level for f in found], ["RED"] * len(paths), found)
        for i, (f, p) in enumerate(zip(found, paths)):
            self.assertIn("%s:%d:" % (RS, i + 2), f.message)
            self.assertIn("::".join(p.split("::")[-2:]), f.message)

    def test_system_time_elapsed_is_a_read(self):
        """Pin WC-B1-b: `SystemTime::elapsed` (a `wall:` ban) is RED."""
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, "fn a() {}\n"
                       "fn b(t: std::time::SystemTime) { let _ = std::time::SystemTime::elapsed(&t); }\n")
        found = repo.findings(RS)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn(RS + ":2:", found[0].message)

    def test_a_new_python_read_in_a_template_script_is_red(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nstarted = time.time()\n")
        found = repo.findings(PY)
        self.assertEqual(len(found), 1, found)
        self.assertIn(PY + ":2:", found[0].message)

    def test_new_date_and_date_now_in_template_js_are_red(self):
        repo = Repo(self, {JS: "const a = 1;\n"})
        repo.write(JS, "const a = 1;\nconst b = Date.now();\nconst c = new Date();\n"
                       "const d = new Date(b);\n")
        found = repo.findings(JS)
        self.assertEqual(sorted(f.message.split(" ")[0] for f in found),
                         [JS + ":2:", JS + ":3:"], found)

    def test_a_new_line_in_a_script_inline_is_red_with_its_script_line(self):
        repo = Repo(self, {CFG: config(SCRIPT_STOCK)})
        repo.write(CFG, config(SCRIPT_STOCK + "late = datetime.datetime.utcnow()\n"))
        found = repo.findings(CFG)
        self.assertEqual(len(found), 1, found)
        self.assertIn("(script_inline line 5)", found[0].message)
        self.assertIn(CFG + ":", found[0].message)

    def test_a_new_template_file_is_graded_as_a_whole(self):
        repo = Repo(self, {})
        repo.write(PY, "import datetime\nnow = datetime.datetime.now()\n")
        found = repo.findings(PY)
        self.assertEqual(len(found), 1, found)
        self.assertIn(PY + ":2:", found[0].message)


class AMarkerNamingARealOverrideIsGreen(unittest.TestCase):
    """`wall-clock: <name>` on the line, the name a param or env the tree carries."""

    def test_a_marker_and_its_param_in_the_same_diff_are_green(self):
        repo = Repo(self, {CFG: config(SCRIPT_STOCK)})
        script = SCRIPT_STOCK + ("night = doc.get('night_from_utc') or "
                                 "datetime.datetime.now()  # wall-clock: night_from_utc\n")
        repo.write(CFG, config(script, {"night_from_utc": "22:00"}))
        self.assertEqual(repo.findings(CFG), [])

    def test_a_marker_on_the_comment_line_above_is_green(self):
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, "fn a() {}\n"
                       "fn now() -> std::time::SystemTime {\n"
                       "    // wall-clock: MECLAW_FAKE_NOW\n"
                       "    std::time::SystemTime::now()\n}\n")
        repo.write("crates/meclaw-x/src/env.rs",
                   'pub fn fake() -> Option<String> { std::env::var("MECLAW_FAKE_NOW").ok() }\n')
        self.assertEqual(repo.findings(RS, "crates/meclaw-x/src/env.rs"), [])

    def test_a_marker_naming_nothing_real_is_red(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nstarted = time.time()  # wall-clock: no_such_param\n")
        found = repo.findings(PY)
        self.assertEqual(len(found), 1, found)
        self.assertIn("no_such_param", found[0].message)
        self.assertIn(PY + ":2:", found[0].message)

    def test_a_marker_two_lines_up_does_not_count(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\n# wall-clock: started_at\nx = 1\n"
                       "started = time.time()\nstarted_at = None\n")
        found = repo.findings(PY)
        self.assertEqual(len(found), 1, found)


# The re-check's six self-referential markers (review B-I1): each names a word
# of the read line or a keyword, and nothing in the tree is a param or an env.
SELF_REFERENCE = (
    (RS, "pub fn a() {}\n",
     "pub fn b() -> std::time::SystemTime { std::time::SystemTime::now() } // wall-clock: SystemTime\n"),
    (RS, "pub fn a() {}\n",
     "pub fn b() -> std::time::SystemTime { std::time::SystemTime::now() } // wall-clock: now\n"),
    (RS, "pub fn a() {}\n",
     "pub fn b() -> std::time::SystemTime { std::time::SystemTime::now() } // wall-clock: fn\n"),
    (PY, "import time\n", "started = time.time()  # wall-clock: time\n"),
    (PY, "import time\n", "started = time.time()  # wall-clock: started\n"),
    (JS, "const a = 1;\n", "const b = Date.now(); // wall-clock: Date\n"),
)

ENV = "crates/meclaw-x/src/env.rs"
OTHER_CFG = "templates/keeper/plan/config.json"


class AMarkerNamesARealOverrideNeverAWordOfTheReadLine(unittest.TestCase):
    """Pins WC-B2 (review B-I1): the name counts only as a param key of a
    template config.json, an env read of the product, or a string literal on
    another line of the same source -- exact, never a word of the read line."""

    def test_a_word_of_the_read_line_or_a_keyword_is_red(self):
        for path, stock, added in SELF_REFERENCE:
            with self.subTest(added=added.strip()):
                repo = Repo(self, {path: stock})
                repo.write(path, stock + added)
                found = repo.findings(path)
                self.assertEqual([f.level for f in found], ["RED"], found)
                self.assertIn(path + ":2:", found[0].message)

    def test_a_literal_on_the_read_line_alone_is_red(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nnight = params.get('night_from_utc') or time.time()"
                       "  # wall-clock: night_from_utc\n")
        found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn("night_from_utc", found[0].message)

    def test_a_literal_on_another_line_of_the_same_source_is_green(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nNIGHT = 'night_from_utc'\n"
                       "night = params.get(NIGHT) or time.time()  # wall-clock: night_from_utc\n")
        self.assertEqual(repo.findings(PY), [])

    def test_a_param_key_of_a_template_config_is_green(self):
        stock = {PY: "import time\n", OTHER_CFG: json.dumps(
            {"type": "code", "params": {"night_from_utc": "22:00"}}) + "\n"}
        repo = Repo(self, stock)
        repo.write(PY, "import time\nnight = time.time()  # wall-clock: night_from_utc\n")
        self.assertEqual(repo.findings(PY), [])

    def test_every_env_read_form_is_green(self):
        forms = (
            (ENV, 'pub fn f() -> Option<String> { std::env::var("FAKE_NOW").ok() }\n'),
            (ENV, 'pub fn f() -> bool { std::env::var_os("FAKE_NOW").is_some() }\n'),
            ("templates/keeper/plan/plan.py", 'import os\nX = os.environ["FAKE_NOW"]\n'),
            ("templates/keeper/plan/plan.py", "import os\nX = os.environ.get('FAKE_NOW')\n"),
            ("templates/keeper/plan/plan.py", 'import os\nX = os.getenv("FAKE_NOW")\n'),
            ("templates/keeper/plan/plan.js", "const x = process.env.FAKE_NOW;\n"),
            ("templates/keeper/plan/plan.js", 'const x = process.env["FAKE_NOW"];\n'),
            (OTHER_CFG, json.dumps({"type": "llm", "params": {"at": "${FAKE_NOW:-0}"}}) + "\n"),
            (OTHER_CFG, config("import os\nX = os.environ.get('FAKE_NOW')\n")),
        )
        for rel, text in forms:
            with self.subTest(rel=rel, text=text.strip()):
                repo = Repo(self, {RS: "fn a() {}\n", rel: text})
                repo.write(RS, "fn a() {}\n"
                               "fn b() { let _ = std::time::SystemTime::now(); } // wall-clock: FAKE_NOW\n")
                self.assertEqual(repo.findings(RS), [])

    def test_an_env_read_on_the_read_line_is_green(self):
        repo = Repo(self, {PY: "import os, time\n"})
        repo.write(PY, "import os, time\n"
                       "t = float(os.environ.get('FAKE_NOW') or time.time())  # wall-clock: FAKE_NOW\n")
        self.assertEqual(repo.findings(PY), [])

    def test_a_part_of_a_word_is_red(self):
        cases = (
            ("env", {ENV: 'pub fn f() -> Option<String> { std::env::var("FAKE_NOW_AT").ok() }\n'}),
            ("param", {OTHER_CFG: json.dumps({"type": "code", "params": {"FAKE_NOW_AT": 1}}) + "\n"}),
            ("literal", {}),
        )
        for label, extra in cases:
            with self.subTest(label):
                lead = "KEY = 'FAKE_NOW_AT'\n" if label == "literal" else "import time\n"
                repo = Repo(self, dict(extra, **{PY: lead}))
                repo.write(PY, lead + "t = time.time()  # wall-clock: FAKE_NOW\n")
                found = repo.findings(PY)
                self.assertEqual([f.level for f in found], ["RED"], found)
                self.assertIn("FAKE_NOW", found[0].message)

    def test_a_mention_in_a_comment_is_no_override(self):
        repo = Repo(self, {ENV: '// read std::env::var("FAKE_NOW") one day\npub fn f() {}\n',
                           PY: "import time\n# KEY = 'FAKE_NOW'\n"})
        repo.write(PY, "import time\n# KEY = 'FAKE_NOW'\nt = time.time()  # wall-clock: FAKE_NOW\n")
        found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)

    def test_a_bare_word_of_the_product_is_no_override(self):
        repo = Repo(self, {"crates/meclaw-x/src/other.rs": "pub fn night_window() {}\n",
                           PY: "import time\n"})
        repo.write(PY, "import time\nt = time.time()  # wall-clock: night_window\n")
        found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)


class WhatTheRuleCannotReadIsRed(unittest.TestCase):
    """Pins WC-B3 (review B-M3): an input the check cannot read is RED, never a
    silent green."""

    def test_a_config_json_of_the_diff_that_does_not_parse_is_red(self):
        repo = Repo(self, {CFG: config(SCRIPT_STOCK)})
        broken = config(SCRIPT_STOCK + "late = datetime.datetime.now()\n").replace(
            '"params": {', '"params": {,', 1)
        repo.write(CFG, broken)
        found = repo.findings(CFG)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn(CFG, found[0].message)
        self.assertIn("cannot read", found[0].message)

    def test_a_failed_git_diff_is_red(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nstarted = time.time()\n")
        found = list(wall_clock.scan([PY], repo.root, base="0" * 40))
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn("cannot read", found[0].message)

    def test_an_unreadable_file_of_the_diff_is_red(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nstarted = time.time()\n")
        with mock.patch.object(wall_clock, "_read", return_value=None):
            found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn(PY + ": cannot read", found[0].message)

    def test_a_file_the_diff_helper_misses_is_red(self):
        # `testing_bans.added_lines` reads a failed git as "nothing added" (its
        # code is the bans' own and stays): the check cross-reads diff and listing
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nstarted = time.time()\n")
        self.assertEqual(testing_bans.added_lines(repo.root, "0" * 40, [PY]), {})
        with mock.patch.object(testing_bans, "added_lines", return_value={}):
            found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn("cannot read", found[0].message)

    def test_a_failed_listing_of_untracked_files_is_red(self):
        repo = Repo(self, {"README.md": "x\n"})
        repo.write(PY, "import time\nstarted = time.time()\n")
        real = testing_bans._git

        def no_listing(tree, *args):
            return None if args[:1] == ("ls-files",) else real(tree, *args)

        with mock.patch.object(testing_bans, "_git", side_effect=no_listing):
            found = repo.findings(PY)
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn("cannot read", found[0].message)


class TheClockBansAreTheOneListOfRustReads(unittest.TestCase):
    """Pins WC-B4 (review B-M1): without a readable list of `wall:` bans a Rust
    diff is RED, never graded against a guess."""

    def _rust_findings(self, bans_text):
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, "fn a() {}\nfn b() {}\n")
        bans = os.path.join(repo.root, "bans.toml")
        if bans_text is not None:
            with open(bans, "w", encoding="utf-8") as fh:
                fh.write(bans_text)
        with mock.patch.object(wall_clock, "CLOCK_BANS", bans):
            return repo.findings(RS)

    def test_missing_broken_or_wall_free_clock_bans_are_red(self):
        cases = (
            ("missing", None),
            ("broken", "disallowed-methods = [\n"),
            ("no wall entry", 'disallowed-methods = [\n  { path = "std::time::Instant::now", '
                              'reason = "mono: x" },\n]\n'),
        )
        for label, text in cases:
            with self.subTest(label):
                found = self._rust_findings(text)
                self.assertEqual([f.level for f in found], ["RED"], found)
                self.assertIn("cannot read", found[0].message)
                self.assertIn("clippy.toml", found[0].message)

    def test_the_rule_reads_the_bans_as_the_ratchet_counts_them(self):
        """Pin WC-M1 (re-check 9f61a3224 M1): one reading of the list. A ban
        whose reason is `wall : ...` or ` wall: ...` is a wall clock for the
        ratchet (`unwrap_budget.read_bans`), so it is a read for the rule too
        -- beside a plain `wall:` line, the case that hid the gap."""
        text = ('disallowed-methods = [\n'
                '  { path = "std::time::SystemTime::now", reason = "wall : spaced" },\n'
                '  { path = "chrono::Utc::now", reason = " wall: leading" },\n'
                '  { path = "chrono::Local::now", reason = "wall: plain" },\n'
                '  { path = "std::time::Instant::now", reason = "mono: monotonic" },\n'
                ']\n')
        repo = Repo(self, {RS: "fn a() {}\n"})
        bans = os.path.join(repo.root, "bans.toml")
        with open(bans, "w", encoding="utf-8") as fh:
            fh.write(text)
        counted = ratchet().read_bans(pathlib.Path(bans))
        self.assertEqual({"std::time::SystemTime::now": "wall", "chrono::Utc::now": "wall",
                          "chrono::Local::now": "wall", "std::time::Instant::now": "mono"}, counted)
        reads = ["SystemTime::now", "Utc::now", "Local::now", "Instant::now"]
        repo.write(RS, "fn a() {}\n" + "".join("fn r%d() { let _ = %s(); }\n" % (i, r)
                                              for i, r in enumerate(reads)))
        with mock.patch.object(wall_clock, "CLOCK_BANS", bans):
            found = repo.findings(RS)
        self.assertEqual([f.level for f in found], ["RED"] * 3, found)
        for f, (i, r) in zip(found, [(i, r) for i, r in enumerate(reads) if r != "Instant::now"]):
            self.assertIn("%s:%d:" % (RS, i + 2), f.message)
            self.assertIn(r, f.message)

    def test_a_ban_the_ratchet_refuses_is_red(self):
        """Pin WC-M1b: the ratchet refuses a ban without a `mono`/`wall`
        reason; the rule reads the list through the same function, so the
        same list is RED there (`cannot read`), never half read."""
        found = self._rust_findings(
            'disallowed-methods = [\n'
            '  { path = "chrono::Local::now", reason = "wall: plain" },\n'
            '  { path = "chrono::Utc::now", reason = "clock: neither" },\n'
            ']\n')
        self.assertEqual([f.level for f in found], ["RED"], found)
        self.assertIn("cannot read", found[0].message)
        self.assertIn("clippy.toml", found[0].message)

    def test_a_diff_without_rust_does_not_need_the_clock_bans(self):
        repo = Repo(self, {PY: "import time\n"})
        repo.write(PY, "import time\nx = 1\n")
        with mock.patch.object(wall_clock, "CLOCK_BANS", os.path.join(repo.root, "none.toml")):
            self.assertEqual(repo.findings(PY), [])


class TheStockIsNotGraded(unittest.TestCase):
    """Only added lines: a stock read stays green beside a new line."""

    def test_a_stock_rust_line_in_a_changed_file_is_green(self):
        repo = Repo(self, {RS: RS_STOCK})
        repo.write(RS, RS_STOCK + "pub fn one() -> u64 {\n    1\n}\n")
        self.assertEqual(repo.findings(RS), [])

    def test_a_stock_line_of_a_script_inline_is_green(self):
        repo = Repo(self, {CFG: config(SCRIPT_STOCK)})
        repo.write(CFG, config(SCRIPT_STOCK + "print('done')\n"))
        self.assertEqual(repo.findings(CFG), [])

    def test_an_unchanged_file_in_the_list_is_green(self):
        repo = Repo(self, {RS: RS_STOCK})
        self.assertEqual(repo.findings(RS), [])


class WhatIsNotProductCode(unittest.TestCase):
    """Test code has the testing bans; comments and strings are no reads."""

    def test_a_cfg_test_module_in_src_is_not_graded(self):
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, "fn a() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n"
                       "    fn t() { let _ = std::time::SystemTime::now(); }\n}\n")
        self.assertEqual(repo.findings(RS), [])

    def test_crate_tests_and_meclaw_testing_are_not_graded(self):
        paths = ("crates/meclaw-x/tests/a.rs", "crates/meclaw-testing/src/clock.rs")
        repo = Repo(self, {})
        for p in paths:
            repo.write(p, "fn t() { let _ = std::time::SystemTime::now(); }\n")
        self.assertEqual(repo.findings(*paths), [])

    def test_template_test_sources_are_not_graded(self):
        paths = ("templates/keeper/view/view.test.js", "templates/keeper/close/test_close.py",
                 "templates/keeper/compose/scenarios/keeper_scenario.py")
        repo = Repo(self, {})
        for p in paths:
            repo.write(p, "x = Date.now(); y = time.time()\n")
        self.assertEqual(repo.findings(*paths), [])

    def test_a_mention_in_a_comment_or_a_string_is_no_read(self):
        repo = Repo(self, {RS: "fn a() {}\n"})
        repo.write(RS, 'fn a() {}\n// SystemTime::now() is banned here\n'
                       'const HINT: &str = "use SystemTime::now() elsewhere";\n')
        self.assertEqual(repo.findings(RS), [])

    def test_no_merge_base_is_a_note(self):
        repo = Repo(self, {RS: "fn a() {}\n"})
        found = list(wall_clock.scan([RS], repo.root, base=None))
        self.assertEqual([f.level for f in found], ["NOTE"], found)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
