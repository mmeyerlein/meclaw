"""The SKIP contract of the browser drivers under the gate (GH #1161, OR-TB.S3.17).

A host without an engine SKIPs a browser proof: the driver says `SKIP ...` on a line
of its own and leaves with 3, and the Rust lock reads that as a pass (R2b). That is
right on a developer's machine and wrong in the gate: on build02 every WebKit proof
of `browser:display` skipped (`libGL.so.1` missing in the bundle) and the station
read GREEN. The station that books an engine runs its proofs with
`MECLAW_REQUIRE_BROWSERS=1`; then an engine that does not start is a failing run --
no `SKIP ` line, exit 1 -- and the lock is red. Without the switch nothing changes.
"""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

from scripts.tests import kit_time, store_isolation

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
TOOLS = ROOT / "workshop" / "tools"
NODE = shutil.which("node")

# The drivers the locks of `browser:display` start (scripts/gate_plan.py,
# BROWSER_LOCKS), each with arguments that reach the engine start and no further.
DRIVERS = {
    "display-slow3g-browser.mjs": lambda tmp: ["http://127.0.0.1:9/", str(tmp / "expected")],
    "display-orientation-browser.mjs": lambda tmp: ["http://127.0.0.1:9/"],
    "display-csp-browser.mjs": lambda tmp: ["http://127.0.0.1:9/", str(tmp / "csp.json")],
    "display-keep-browser.mjs": lambda tmp: ["http://127.0.0.1:9/", str(tmp / "steps"),
                                             "--engine", "webkit"],
    "display-motion-browser.mjs": lambda tmp: ["http://127.0.0.1:9/", str(tmp / "steps"),
                                               "--engine", "webkit"],
    "display-layout-browser.mjs": lambda tmp: ["http://127.0.0.1:9/", str(tmp / "out"),
                                               "--engine", "webkit", "--run", "L1"],
}


def skip_lines(stderr):
    """The lines the Rust locks read as the driver's own SKIP."""
    return [ln for ln in stderr.splitlines() if ln.startswith("SKIP ")]


class Host(unittest.TestCase):
    """A host with no engine at all: an empty home, an empty Playwright browser
    path, a PATH with nothing but node -- whatever the lane has installed."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="browser-required-")
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)
        (self.tmp / "home").mkdir()
        (self.tmp / "browsers").mkdir()
        (self.tmp / "steps").mkdir()
        (self.tmp / "out").mkdir()
        (self.tmp / "expected").write_text("hello\n")
        (self.tmp / "csp.json").write_text("{}\n")
        # node alone on PATH: a chromium beside it in /usr/bin would be found
        (self.tmp / "bin").mkdir()
        if NODE:
            (self.tmp / "bin" / "node").symlink_to(NODE)

    def env(self, required):
        env = {"HOME": str(self.tmp / "home"), "TMPDIR": str(self.tmp),
               "PATH": store_isolation.trapped_path(str(self.tmp / "bin")),
               "PLAYWRIGHT_BROWSERS_PATH": str(self.tmp / "browsers"),
               "MECLAW_DISPLAY_API": "http://127.0.0.1:9",
               "MECLAW_WKDEPS": str(self.tmp / "no-laboratory")}
        if required:
            env["MECLAW_REQUIRE_BROWSERS"] = "1"
        return env


class TestWkenv(Host):
    def source(self, required):
        sh = shutil.which("sh") or "/bin/sh"
        return subprocess.run([sh, "-c", '. "$1"', "wkenv", str(TOOLS / "wkenv.sh")],
                              capture_output=True, text=True, timeout=kit_time.HANG,
                              env=dict(self.env(required), PATH=store_isolation.trapped_path()))

    def test_no_laboratory_is_a_skip_line_without_the_switch(self):
        res = self.source(required=False)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertTrue(skip_lines(res.stderr), res.stderr)

    def test_no_laboratory_is_no_skip_line_under_the_switch(self):
        # The laboratory is one way to the libraries, the system is the other: under
        # the switch wkenv.sh says what it saw, and the engine start decides.
        res = self.source(required=True)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], skip_lines(res.stderr), res.stderr)
        self.assertIn("MECLAW_REQUIRE_BROWSERS=1", res.stderr)


@unittest.skipUnless(NODE, "the drivers want node")
class TestDrivers(Host):
    def drive(self, driver, required):
        argv = [NODE, str(TOOLS / driver)] + DRIVERS[driver](self.tmp)
        return subprocess.run(argv, capture_output=True, text=True, timeout=2 * kit_time.HANG,
                              env=self.env(required), cwd=str(self.tmp))

    def test_no_engine_is_a_skip_without_the_switch(self):
        for driver in DRIVERS:
            with self.subTest(driver=driver):
                res = self.drive(driver, required=False)
                self.assertEqual(3, res.returncode, res.stderr)
                self.assertTrue(skip_lines(res.stderr), res.stderr)

    def test_no_engine_is_a_failing_run_under_the_switch(self):
        for driver in DRIVERS:
            with self.subTest(driver=driver):
                res = self.drive(driver, required=True)
                self.assertEqual(1, res.returncode, res.stderr)
                self.assertEqual([], skip_lines(res.stderr), res.stderr)
                self.assertIn("MECLAW_REQUIRE_BROWSERS=1", res.stderr)


# The spellings of "the driver left with 3": `Some(3)` (a comparison, a
# `matches!`, a match arm), an unwrapped code compared with 3, and a `3 =>` arm.
# Re-check m-neu-1: `Some(3)` alone did not see `code().unwrap_or(0) == 3`.
# Re-check tb-s3 L-6: nor `exit_code == 3`, `3 == code`, `is_some_and(|c| c == 3)`,
# or a constant `const SKIP_EXIT: i32 = 3;` in place of the 3 (`{n}`).
_EXIT3_SHAPES = (r"\bSome\(\s*{n}\s*\)"
                 r"|\bcode\(\)(?:\s*\.\s*\w+\([^()]*\))*\s*==\s*{n}"
                 r"|\b\w*code\s*==\s*{n}"
                 r"|(?<![\w.]){n}\s*==\s*[\w.]*code\b"
                 r"|\|\s*\w+\s*\|\s*\*?\w+\s*==\s*{n}"
                 r"|(?<![\w.]){n}\s*=>")
_EXIT3_CONST_RE = re.compile(r"\bconst\s+(\w+)\s*:\s*[\w:]+\s*=\s*3\s*;")


def _exit3_re(code):
    """`_EXIT3_SHAPES` for the code `code`: 3, or a constant of it declares as 3."""
    names = ["3"] + _EXIT3_CONST_RE.findall(code)
    return re.compile(_EXIT3_SHAPES.replace("{n}", r"(?:%s)\b" % "|".join(names)))


_EXIT3_RE = _exit3_re("")
# GH #1189: a unit that drives the browser in-process (gh766) prints its skip --
# `let .. else { eprintln!("SKIP .."); return; }` -- and leaves no exit code.
_SKIP_LINE_RE = re.compile(r"\be?println!\s*\(\s*\"SKIP")
SWITCH = "MECLAW_REQUIRE_BROWSERS"
# A READ of the switch, not its name: the message of the assert names it as well
# (mutation: the read replaced by `true` left the name and the check passed).
_SWITCH_READ = r"\benv::var(?:_os)?\(\s*\"%s\"\s*\)" % SWITCH
# A read that turns the skip red: an assert over it, or an `if` over it whose
# branch panics. Re-check m-neu-1: `let _ = env::var(..)` read the switch, changed
# nothing, and the pin stayed green.
_RED_UNDER_THE_SWITCH_RE = re.compile(
    r"\bassert(?:_eq|_ne)?!\s*\([^;]*?%s" % _SWITCH_READ
    + r"|\bif\b[^{;]*?%s[^{;]*\{.*?\b(?:panic|unreachable)!" % _SWITCH_READ, re.S)


def skips_without_the_switch(text):
    """The lines of the Rust unit `text` where a driver's exit 3 or a printed `SKIP`
    is taken as a pass that `MECLAW_REQUIRE_BROWSERS` does not turn red in that
    branch -- read from the exit-3 test (`_exit3_re`) or the `SKIP` line
    (`_SKIP_LINE_RE`) to the branch's first `return`, comments blanked (a
    comment naming the switch is no check), literals kept (the switch is a string).
    Red means an `assert!`/`assert_eq!`/`assert_ne!` over a read of the switch, or an
    `if` over the read with a `panic!` (`_RED_UNDER_THE_SWITCH_RE`); a bare read is not.
    A unit the lexer doubts is `[0]` -- unread is no proof of a check (fail-closed)."""
    import gate_plan as gp  # here, not on import: the public-tree run copies no gate_plan
    code = gp._rust_code_aligned(text)
    if code is None:
        return [0]
    out = []
    # Every hit is read: an earlier branch that ends on the same `return` covers no
    # later one (re-check tb-s3 m-neu-2); only the reported lines are one.
    hits = sorted([*_exit3_re(code).finditer(code), *_SKIP_LINE_RE.finditer(code)],
                  key=lambda m: m.start())
    for m in hits:
        end = code.find("return", m.end())
        end = len(code) if end < 0 else end
        start = m.end()
        if m.re is _SKIP_LINE_RE:
            # a SKIP line after the branch's assert (gh703: assert, then print): the
            # branch reaches back to the previous `return` or `fn`, no further
            start = max(code.rfind("return", 0, m.start()), code.rfind("fn ", 0, m.start()), 0)
        if not _RED_UNDER_THE_SWITCH_RE.search(code[start:end]):
            line = code.count("\n", 0, m.start()) + 1
            if line not in out:
                out.append(line)
    return out


_SKIPS = """\
fn drive() {
    let out = run();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        // a comment is no check: std::env::var("MECLAW_REQUIRE_BROWSERS")
        println!("{stderr}");
        return;
    }
}
"""

_RED_UNDER_THE_SWITCH = """\
fn drive() {
    let out = run();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        if std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() == Ok("1") {
            panic!("skipped under the switch");
        }
        println!("{stderr}");
        return;
    }
}
"""


# Re-check m-neu-1: a branch that READS the switch but does not turn red on it.
# `let _ = env::var(..)` and an `if` without a panic both passed the pin before.
_READ_ONLY = """\
fn drive() {
    let out = run();
    if out.status.code() == Some(3) {
        let _ = std::env::var("MECLAW_REQUIRE_BROWSERS");
        println!("{stderr}");
        return;
    }
}
"""

_IF_WITHOUT_PANIC = """\
fn drive() {
    let out = run();
    if out.status.code() == Some(3) {
        if std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() == Ok("1") {
            eprintln!("skipped under the switch");
        }
        return;
    }
}
"""

# Re-check m-neu-1: exit 3 recognised without the spelling `Some(3)`.
_UNWRAPPED_EXIT3 = """\
fn drive() {
    let out = run();
    if out.status.code().unwrap_or(0) == 3 {
        println!("{stderr}");
        return;
    }
}
"""

_ARM_EXIT3 = """\
fn drive() {
    let out = run();
    match out.status.code().unwrap_or(-1) {
        3 => {
            println!("{stderr}");
            return;
        }
        _ => {}
    }
}
"""

_ASSERT_NE_UNDER_THE_SWITCH = """\
fn drive() {
    let out = run();
    if out.status.code().unwrap_or(0) == 3 {
        assert_ne!(std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(), Ok("1"), "{stderr}");
        return;
    }
}
"""


# GH #1189: a unit that runs the browser in-process takes its skip as a printed
# `SKIP` line and a `return`, no exit 3 (gh766, `packaged()` without MECLAW_CHROMIUM).
_LET_ELSE_SKIP = """\
fn drive() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM");
        return;
    };
}
"""

_LET_ELSE_SKIP_RED = """\
fn drive() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM");
        assert_ne!(std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(), Ok("1"), "SKIP");
        return;
    };
}
"""

# gh703: the exit-3 branch asserts, then prints its SKIP line; the SKIP anchor
# reaches back to the branch's assert, not past the previous `return`.
_ASSERT_THEN_SKIP_LINE = """\
fn drive() {
    if lib_missing() {
        return;
    }
    if out.status.code() == Some(3) {
        assert_ne!(std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(), Ok("1"), "red");
        println!("SKIP webkit: {stderr}");
        return;
    }
}
"""

# Re-check tb-s3 m-neu-2: an earlier exit-3 branch with an assert and no `return`
# of its own ended on the `return` of a later branch without one and covered it.
_EARLIER_BRANCH_COVERS = """\
fn drive() {
    let out = run();
    if code == Some(3) && webkit {
        assert!(std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() != Ok("1"));
        println!("{stderr}");
    }
    if code == Some(3) {
        return;
    }
}
"""

# Re-check tb-s3 L-6: exit 3 spelled as a named code, reversed, in a closure, or
# as a constant.
_EXIT3_SPELLINGS = [
    "if out.exit_code == 3 {",
    "if 3 == code {",
    "if out.status.code().is_some_and(|c| c == 3) {",
    "if out.status.code() == Some(SKIP_EXIT) {",
    "if code == SKIP_EXIT {",
]


def _exit3_branch(cond):
    return ("const SKIP_EXIT: i32 = 3;\nfn drive() {\n    let out = run();\n    %s\n"
            "        println!(\"{stderr}\");\n        return;\n    }\n}\n" % cond)


class TestCallersUnderTheSwitch(unittest.TestCase):
    """GH #1161, OR-TB.S3.39 (ruling (b) of the wave's coordinator): the units
    `browser:display` books besides the locks start their drivers themselves and
    read exit 3 as a pass. No common caller exists (each unit runs `node` on its
    own driver), so each one reads the switch where it takes the skip: under
    `MECLAW_REQUIRE_BROWSERS=1` the station booked the engine, and a skipped
    driver is a red test. Before, junit counted such a skip as `passed`
    (re-check 003a717c3: 33 Chromium and 3 WebKit tests of the station could not
    be told from a skip). The locks keep the driver side (`engineAbsent`,
    TestDrivers above)."""

    def test_a_skip_without_the_switch_is_found(self):
        self.assertEqual([3], skips_without_the_switch(_SKIPS))

    def test_a_skip_that_is_red_under_the_switch_passes(self):
        self.assertEqual([], skips_without_the_switch(_RED_UNDER_THE_SWITCH))

    def test_a_read_that_does_not_turn_the_skip_red_is_found(self):
        # re-check m-neu-1: the switch read, but no assert or panic over it
        self.assertEqual([3], skips_without_the_switch(_READ_ONLY))
        self.assertEqual([3], skips_without_the_switch(_IF_WITHOUT_PANIC))

    def test_exit_3_spelled_without_some_3_is_found(self):
        # re-check m-neu-1: `code().unwrap_or(0) == 3` and a `3 =>` arm
        self.assertEqual([3], skips_without_the_switch(_UNWRAPPED_EXIT3))
        self.assertEqual([4], skips_without_the_switch(_ARM_EXIT3))
        self.assertEqual([], skips_without_the_switch(_ASSERT_NE_UNDER_THE_SWITCH))

    def test_a_printed_skip_without_exit_3_is_found(self):
        # GH #1189: `let .. else { eprintln!("SKIP.."); return; }` (gh766)
        self.assertEqual([3], skips_without_the_switch(_LET_ELSE_SKIP))
        self.assertEqual([], skips_without_the_switch(_LET_ELSE_SKIP_RED))
        self.assertEqual([], skips_without_the_switch(_ASSERT_THEN_SKIP_LINE))

    def test_an_earlier_branch_does_not_cover_a_later_one(self):
        # re-check tb-s3 m-neu-2: every hit is read, only the reported lines are one
        self.assertEqual([7], skips_without_the_switch(_EARLIER_BRANCH_COVERS))

    def test_exit_3_spelled_as_a_name_a_closure_or_a_constant_is_found(self):
        # re-check tb-s3 L-6
        for cond in _EXIT3_SPELLINGS:
            with self.subTest(cond=cond):
                self.assertEqual([4], skips_without_the_switch(_exit3_branch(cond)))

    def test_a_unit_the_lexer_doubts_is_a_skip_at_line_0(self):
        # GH #1161 x #1162 (re-check h2 M-2): a comment left open is a doubt of
        # the token lexer; unread is no proof of a check under the switch, so
        # even the unit that passes when read is `[0]` (fail-closed)
        doubt = "/* a comment left open\n"
        self.assertEqual([0], skips_without_the_switch(_SKIPS + doubt))
        self.assertEqual([0], skips_without_the_switch(_RED_UNDER_THE_SWITCH + doubt))

    def test_every_browser_unit_beside_the_locks_is_red_on_a_skip_under_the_switch(self):
        import gate_plan as gp
        units = gp.browser_units(str(ROOT))
        self.assertTrue(units, "no browser unit found in this tree")
        found = {}
        for crate, unit, text in units.values():
            if gp._browser_lock_term(crate, unit):
                continue
            lines = skips_without_the_switch(text)
            if lines:
                found["crates/%s/tests/%s.rs" % (crate, unit)] = lines
        self.assertEqual({}, found)


class TestPublicTree(unittest.TestCase):
    """The module travels in the export (ROOT_FILES: gate-selftest names it and runs
    in `ci`, GH #1161 re-check M-3), the drivers under `workshop/` never do. In a
    tree without them the module skips -- before, every driver run was a failure
    there (`sh` sourcing a missing wkenv.sh, node on a missing driver)."""

    def test_the_module_skips_in_a_tree_without_the_workshop(self):
        with tempfile.TemporaryDirectory(prefix="browser-required-public-") as tmp:
            tests = pathlib.Path(tmp) / "scripts" / "tests"
            tests.mkdir(parents=True)
            here = pathlib.Path(__file__).resolve().parent
            # kit_time: every kit deadline (TV-Z3); fake_ssh: the guard's ssh stub
            # comes from the one maker (re-check tv-bn M7e) -- both travel (M7h)
            for name in ("__init__.py", "store_isolation.py", "side_effect_guard.py",
                         "kit_time.py", "fake_ssh.py", pathlib.Path(__file__).name):
                shutil.copy(here / name, tests / name)
            # scripts/tests/__init__.py imports the pipeline package, which travels
            shutil.copytree(here.parent / "pipeline", tests.parent / "pipeline",
                            ignore=shutil.ignore_patterns("__pycache__"))
            # ... and reads the verdict names from lane_sync.sh, which travels too
            # (ROOT_FILES; tv-bn M7z: the copy had left it out, the import died)
            shutil.copy(here.parent / "lane_sync.sh", tests.parent / "lane_sync.sh")
            res = subprocess.run(
                ["python3", "-m", "unittest", "-v",  # never this class again: no recursion
                 "scripts.tests.test_browser_required.TestWkenv",
                 "scripts.tests.test_browser_required.TestDrivers"],
                capture_output=True, text=True, timeout=2 * kit_time.HANG, cwd=tmp,
                env={"PATH": os.environ.get("PATH", ""), "HOME": tmp,
                     "MECLAW_PIPELINE_DB": os.path.join(tmp, "store.db")})
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("workshop/tools", res.stderr)
        self.assertRegex(res.stderr, r"OK \(skipped=\d+\)")


def setUpModule():
    # `workshop/` never travels with the export (R2c); this module does (ROOT_FILES),
    # so the published tree skips it instead of failing on drivers never shipped.
    if not TOOLS.is_dir():
        raise unittest.SkipTest("workshop/tools/ is not in this tree")
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
