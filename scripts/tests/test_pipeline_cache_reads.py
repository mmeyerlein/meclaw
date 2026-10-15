"""The closure of a cached station holds everything its checker reads (V2-A5, GH #1199).

Release gate 0.64.0, run 2 on 45a463300: `export-selftest` came GREEN from the
cache (`secs 0.0`) while it was red on that tree. Its key held only
`plans/export-fixtures/**`; the self-test reads the ci plan of
`scripts/gate_plan.py` and every tracked `scripts/` file (`git ls-files`).
A change there must miss the cache.
"""

import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from scripts.pipeline import cache  # noqa: E402
from scripts.tests import private_tree, store_isolation  # noqa: E402

FINGERPRINT = {"rustc": "rustc 1.80.0", "cargo": "cargo 1.80.0", "nextest": "0.9.143",
               "linker": "cc 13", "python": "3.12.0", "cargo_config": "e3b0c44298fc1c14"}


def _git(root, *args):
    subprocess.run(
        ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
         "-c", "init.defaultBranch=master", *args],
        cwd=root, check=True, capture_output=True, text=True)


def tree(case, files):
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name) / "tree"
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "fixture")
    return root


def station(name, argv):
    return {"name": name, "scope": "s", "cargo": False, "run": True,
            "cmds": [{"argv": list(argv), "cwd": ""}], "inputs": list(cache.INPUTS[name]),
            "env": "none"}


def key(root, st, fingerprint=FINGERPRINT):
    meta = {"packages": [], "workspace_members": [], "workspace_root": str(root)}
    keys = cache.keys_for_plan({"mode": "release", "repo": "meclaw", "stations": [st]},
                               str(root), fingerprint, "lane-a", metadata=meta)
    return keys[(st["name"], "")]["key"]


class ExportSelftestClosure(unittest.TestCase):

    FILES = {
        "plans/export-fixtures/test_drift_gate.py": "import make_export\n",
        "plans/export-fixtures/test_export_tree_tests.py": "print('ok')\n",
        "scripts/gate_plan.py": "CI = ['scripts/tests/test_a.py']\n",
        "scripts/make_export.py": "ROOT_FILES = ['scripts/gate_plan.py']\n",
        "scripts/tests/test_a.py": "pass\n",
    }

    def export_selftest(self):
        return station("export-selftest", ["python3", "plans/export-fixtures/test_drift_gate.py"])

    def test_a_change_to_the_ci_plan_misses_the_cache(self):
        root = tree(self, self.FILES)
        st = self.export_selftest()
        green = key(root, st)
        (root / "scripts/gate_plan.py").write_text(
            "CI = ['scripts/tests/test_a.py', 'scripts/tests/test_b.py']\n", encoding="utf-8")
        self.assertNotEqual(key(root, st), green,
                            "export-selftest reads the ci plan of scripts/gate_plan.py")

    def test_a_new_scripts_file_misses_the_cache(self):
        """The ROOT_FILES scan reads `git ls-files scripts`: a new file there
        changes its answer, so it changes the key."""
        root = tree(self, self.FILES)
        st = self.export_selftest()
        green = key(root, st)
        (root / "scripts/tests/test_wave_live.py").write_text("pass\n", encoding="utf-8")
        self.assertNotEqual(key(root, st), green)


# A browser station runs the Rust locks, which start the Node drivers
# `workshop/tools/display-*-browser.mjs`; what a driver imports is read as well.
BROWSER_FINGERPRINT = dict(FINGERPRINT, node="v22.0.0", playwright="1.50.0",
                           browsers={"chromium": "ok", "webkit": "ok"})


class BrowserStationModules(unittest.TestCase):
    """V2-A5 for `browser:alone` / `browser:display` (review v2-fix-b I2): a change
    to a module a display driver imports -- `wait.mjs`, `lib/*.mjs`, transitively --
    must miss the cache."""

    FILES = {
        "workshop/tools/display-a-browser.mjs":
            'import { hang } from "./wait.mjs";\n'
            'import {\n  noFiltersCdp,\n} from "./lib/no-filters.mjs";\n',
        "workshop/tools/wait.mjs": "export const HANG_MS = 1000;\n",
        "workshop/tools/lib/no-filters.mjs":
            'import { osServerMarkup } from "./os-template.mjs";\nexport const x = 1;\n',
        "workshop/tools/lib/os-template.mjs": "export const osServerMarkup = 1;\n",
        "workshop/tools/lib/unused.mjs": "export const nobody = 1;\n",
    }

    def browser_alone(self):
        return dict(station("browser:alone", ["cargo", "nextest", "run"]), cargo=True)

    def assert_change_misses(self, rel):
        root = tree(self, self.FILES)
        st = self.browser_alone()
        green = key(root, st, BROWSER_FINGERPRINT)
        (root / rel).write_text("export const changed = 2;\n", encoding="utf-8")
        self.assertNotEqual(key(root, st, BROWSER_FINGERPRINT), green,
                            "browser:alone keys %s, which its driver imports" % rel)

    def test_a_change_to_wait_mjs_misses_the_cache(self):
        self.assert_change_misses("workshop/tools/wait.mjs")

    def test_a_change_to_a_lib_module_misses_the_cache(self):
        self.assert_change_misses("workshop/tools/lib/no-filters.mjs")

    def test_a_change_to_a_transitively_imported_module_misses_the_cache(self):
        self.assert_change_misses("workshop/tools/lib/os-template.mjs")

    def test_a_module_no_driver_imports_stays_outside_the_key(self):
        """The closure comes from the drivers' imports, not from a directory."""
        root = tree(self, self.FILES)
        st = self.browser_alone()
        green = key(root, st, BROWSER_FINGERPRINT)
        (root / "workshop/tools/lib/unused.mjs").write_text("export const x = 2;\n",
                                                            encoding="utf-8")
        self.assertEqual(key(root, st, BROWSER_FINGERPRINT), green)


def gate_rows(repo, files, modes=("release",)):
    """Every station `gate_plan` plans for `files` in `modes`, one row per name."""
    import gate_plan as gp
    rows = {}
    for mode in modes:
        for st in gp.plan(files, mode, str(repo)):
            if st.name in cache.INPUTS:
                rows.setdefault(st.name, {
                    "name": st.name, "cargo": st.cargo,
                    "cmds": [{"argv": list(a), "cwd": w or ""} for a, w in zip(st.cmds, st.cwds)],
                    "inputs": list(cache.INPUTS[st.name])})
    # a station the pipeline splits off another (`plan.PARENT`) runs its parent's form
    from scripts.pipeline import plan
    for child, parent in plan.PARENT.items():
        if child in cache.INPUTS and parent in rows:
            rows[child] = dict(rows[parent], name=child, inputs=list(cache.INPUTS[child]))
    return rows


def repo_rows():
    repo = HERE.parent.parent
    # The release plan of an empty diff plus the three triggers it lacks
    # (shell, gate infra, a crate source) -- seconds, not the whole index.
    files = ["scripts/gate.sh", "scripts/gate_plan.py", "crates/meclaw-cells/src/lib.rs"]
    meta = {"packages": [], "workspace_members": [], "workspace_root": str(repo)}
    return repo, gate_rows(repo, files), cache._Context(str(repo), meta)


class EveryCachedStation(unittest.TestCase):
    """The lock for the whole class: a cached station outside cargo whose key
    holds less than the whole tree is red here (`cache.unkeyed_reads`), and so is
    any cached station -- cargo or not -- that keys a Node module but not the
    modules it imports."""

    def test_every_cached_station_outside_cargo_keys_what_it_reads(self):
        _repo, rows, ctx = repo_rows()
        # the resolver plans every cacheable station of the table
        self.assertEqual(sorted(set(cache.INPUTS) - cache.NEVER - set(rows)), [])
        self.assertEqual(cache.unkeyed_reads(list(rows.values()), ctx), [])

    @private_tree.private("workshop/tools")   # GH #1199: the browser drivers never travel
    def test_a_browser_station_without_the_import_closure_is_red(self):
        """Mutant: the drivers' import closure out of both browser rows -- the
        check sees `wait.mjs` and `lib/*.mjs` unkeyed (review v2-fix-b I2)."""
        _repo, rows, ctx = repo_rows()
        narrow = {n: [s for s in cache.INPUTS[n] if not s.startswith("mjs:")]
                  for n in ("browser:display", "browser:alone")}
        self.assertNotEqual(narrow["browser:alone"], cache.INPUTS["browser:alone"])
        with mock.patch.dict(cache.INPUTS, narrow):
            for n in narrow:
                rows[n] = dict(rows[n], inputs=list(narrow[n]))
            gaps = cache.unkeyed_reads(list(rows.values()), ctx)
        self.assertEqual(sorted(n for n, _ in gaps), ["browser:alone", "browser:display"])
        why = dict(gaps)["browser:alone"]
        self.assertIn("workshop/tools/wait.mjs", why)
        self.assertIn("workshop/tools/lib/no-filters.mjs", why)

    def test_a_cargo_station_running_node_without_a_tree_is_red(self):
        """Without the tree the import closure cannot be checked: a cargo station
        that keys `tool:node` is no longer exempt on the strength of `cargo`."""
        st = dict(station("browser:alone", ["cargo", "nextest", "run"]), cargo=True)
        self.assertEqual([n for n, _ in cache.unkeyed_reads([st])], ["browser:alone"])

    def test_a_python_station_with_a_narrow_key_is_red(self):
        """The row `persona-cases` had before V2-A5."""
        with mock.patch.dict(cache.INPUTS, {"py-narrow": ["evals_persona", "persona_source"]}):
            st = station("py-narrow", ["python3", "persona_cases.py"])
            self.assertEqual([n for n, _ in cache.unkeyed_reads([st])], ["py-narrow"])

    def test_a_tree_key_a_cargo_station_and_an_argv_only_program_pass(self):
        tree_st = station("gate-selftest", ["python3", "-m", "unittest", "x"])
        cargo_st = dict(station("deny", ["cargo", "deny", "check"]), cargo=True)
        argv_st = station("shellcheck", ["shellcheck", "scripts/a.sh"])
        self.assertEqual(cache.unkeyed_reads([tree_st, cargo_st, argv_st]), [])

    def test_shellcheck_following_sources_is_red(self):
        for flag in ("-x", "--external-sources", "--source-path=scripts"):
            st = station("shellcheck", ["shellcheck", flag, "scripts/a.sh"])
            self.assertEqual([n for n, _ in cache.unkeyed_reads([st])], ["shellcheck"], flag)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
