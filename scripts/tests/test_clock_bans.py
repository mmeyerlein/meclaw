"""The clock bans of the unwrap ratchet (tv-p1b, VORGABEN TV-Z8 / § 5.4).

Production code reads monotonic time through `tokio::time::Instant::now()`
(the same clock in production, virtual under `start_paused`). The std
clocks are banned only inside the ratchet: `.github/gates/clock-bans/
clippy.toml` lists them as `disallowed-methods`, `unwrap_budget.py` runs
clippy with `CLIPPY_CONF_DIR` on that directory and counts the hits per
package in their own columns -- `mono` (std `Instant::now`/`elapsed`, pinned
at 0) and `wall` (`SystemTime`, `chrono` now; pinned, may only go down).
The normal clippy station never sees the file.

The fixture crate is real: clippy runs over it, so a ban path that does not
resolve, or a reason that lands in the wrong column, turns these red.
"""

import importlib.util
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import gate_plan as gp  # noqa: E402
from scripts.pipeline import cache  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from scripts.tests.test_pipeline_cache import (  # noqa: E402
    FINGERPRINT, _git, fixture_metadata, fixture_tree, plain_station, simple_plan)

CLOCK_BANS = REPO / ".github" / "gates" / "clock-bans"


def _load_budget():
    spec = importlib.util.spec_from_file_location(
        "unwrap_budget", REPO / ".github" / "gates" / "unwrap_budget.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


FIXTURE_LIB = """\
use std::time::Instant;

/// A look-alike with the same method name: only the std path is banned.
pub mod virtual_clock {
    pub struct Instant;
    impl Instant {
        pub fn now() -> Self {
            Instant
        }
    }
}

pub fn mono() -> std::time::Duration {
    let started = Instant::now();
    started.elapsed()
}

pub fn wall() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

pub fn not_banned() -> virtual_clock::Instant {
    virtual_clock::Instant::now()
}

pub fn unwrapped(x: Option<u8>) -> u8 {
    x.unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_code_is_out_of_scope() {
        let _ = std::time::Instant::now();
    }
}
"""


class ClockBans(unittest.TestCase):

    def fixture_crate(self, lib):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name) / "ws"
        (root / "src").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            '[package]\nname = "clock-fixture"\nversion = "0.1.0"\nedition = "2021"\n\n'
            "[workspace]\n", encoding="utf-8")
        (root / "src" / "lib.rs").write_text(lib, encoding="utf-8")
        env = dict(os.environ, CARGO_TARGET_DIR=str(pathlib.Path(tmp.name) / "target"))
        env.pop("RUSTFLAGS", None)
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        return root, env

    def test_clock_bans_are_counted_in_their_own_columns(self):
        ub = _load_budget()
        bans = ub.read_bans(CLOCK_BANS / "clippy.toml")
        self.assertEqual(bans["std::time::Instant::now"], "mono")
        self.assertEqual(bans["std::time::Instant::elapsed"], "mono")
        self.assertEqual(bans["std::time::SystemTime::now"], "wall")
        self.assertIn("wall", {bans[p] for p in bans if "chrono" in p})
        root, env = self.fixture_crate(FIXTURE_LIB)
        counts = ub.measure(root=root, conf_dir=CLOCK_BANS, env=env)
        self.assertEqual(counts, {"clock-fixture": {"unwrap": 1, "mono": 2, "wall": 1}})

    def test_a_new_std_instant_in_production_is_red(self):
        ub = _load_budget()
        root, env = self.fixture_crate(
            "pub fn t() -> std::time::Instant {\n    std::time::Instant::now()\n}\n")
        counts = ub.measure(root=root, conf_dir=CLOCK_BANS, env=env)
        pinned = {"clock-fixture": {"unwrap": 0, "mono": 0, "wall": 0}}
        failures, _ = ub.check(counts, pinned)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("mono", failures[0])
        self.assertIn("tokio::time::Instant", failures[0])
        # A lower count is never red, only an invitation to re-pin.
        failures, notes = ub.check(
            {"clock-fixture": {"unwrap": 0, "mono": 0, "wall": 0}},
            {"clock-fixture": {"unwrap": 0, "mono": 0, "wall": 3}})
        self.assertEqual(failures, [])
        self.assertTrue(any("wall" in n for n in notes), notes)

    def test_a_ban_path_that_does_not_resolve_is_red(self):
        """clippy bans nothing for a path it cannot resolve; the ratchet says so."""
        ub = _load_budget()
        root, env = self.fixture_crate(FIXTURE_LIB)
        conf = root.parent / "conf"
        conf.mkdir()
        (conf / "clippy.toml").write_text(
            'disallowed-methods = [{ path = "std::time::Instant::nowx", reason = "mono: x" }]\n',
            encoding="utf-8")
        with self.assertRaisesRegex(SystemExit, "clock ban does not resolve"):
            ub.measure(root=root, conf_dir=conf, env=env)

    def test_the_budget_file_carries_three_columns_and_mono_is_zero_in_cells_and_colony(self):
        ub = _load_budget()
        pinned = ub.read_budget()
        for name, cols in pinned.items():
            self.assertEqual(set(cols), {"unwrap", "mono", "wall"}, name)
        self.assertEqual(pinned["meclaw-cells"]["mono"], 0)
        self.assertEqual(pinned["meclaw-colony"]["mono"], 0)

    def test_a_change_to_the_clock_bans_invalidates_the_ratchet_cache(self):
        path = ".github/gates/clock-bans/clippy.toml"
        self.assertIn("unwrap_infra", gp.classify([path]))
        names = {s.name for s in gp.plan([path], "strand", repo=None)}
        self.assertIn("unwrap-budget", names)
        root = fixture_tree(self)
        conf = pathlib.Path(root, path)
        conf.parent.mkdir(parents=True, exist_ok=True)
        conf.write_text('disallowed-methods = []\n', encoding="utf-8")
        _git(root, "add", "-A")
        _git(root, "commit", "-q", "-m", "bans")
        plan = simple_plan([plain_station("unwrap-budget",
                                          ["python3", ".github/gates/unwrap_budget.py"])])
        meta = fixture_metadata(root)
        before = cache.keys_for_plan(plan, root, FINGERPRINT, "lane-a", metadata=meta)
        conf.write_text('disallowed-methods = [{ path = "std::time::Instant::now", '
                        'reason = "mono: x" }]\n', encoding="utf-8")
        after = cache.keys_for_plan(plan, root, FINGERPRINT, "lane-a", metadata=meta)
        self.assertNotEqual(before[("unwrap-budget", "")]["key"],
                            after[("unwrap-budget", "")]["key"])

    def test_the_bans_never_reach_the_normal_clippy_station(self):
        """No root `clippy.toml`, no `-A` in the clippy station (R-TV-W3)."""
        self.assertFalse((REPO / "clippy.toml").exists())
        self.assertFalse((REPO / ".clippy.toml").exists())
        st = {s.name: s for s in gp.plan(["crates/meclaw-cells/src/lib.rs"], "strand", repo=None)}
        argv = [a for cmd in st["clippy"].cmds for a in cmd]
        self.assertNotIn("-A", argv)
        self.assertFalse(any("CLIPPY_CONF_DIR" in a for a in argv), argv)


if __name__ == "__main__":
    unittest.main()
