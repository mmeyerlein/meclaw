"""`scripts/pipeline/lane_fingerprint.sh`: the fingerprint of a lane (V0 2.3).

The script runs ON the lane (sent over ssh on stdin by `scripts/lane_sync.sh
fingerprint`) and prints one JSON object with every field of the contract --
a tool the lane does not have is `null`, never a missing key.
"""

import fnmatch
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest
from scripts.tests import kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "pipeline" / "lane_fingerprint.sh"

FIELDS = {"rustc", "cargo", "nextest", "node", "playwright", "python", "kernel", "vcpu", "cores",
          "ram_g", "target_fs", "target_free_g", "ulimit_n", "incremental_policy",
          "linker", "build_env", "cargo_config", "sccache", "linger", "pdftotext", "portal_venv",
          "browsers"}

def clean_env(**extra):
    """The environment of a build test: PATH (the tools, the triple) and HOME (in the
    exclusion list) of this process, plus `extra` -- nothing else. Since orchestrator
    OR-T12-18 A-M1 the build environment is the WHOLE environment minus the
    exclusion list, so a test that wants to see one name change starts from a
    known environment instead of filtering this process's."""
    env = {k: os.environ[k] for k in ("PATH", "HOME") if k in os.environ}
    env.update(extra)
    return env


def digest(value):
    """The form a value takes in `build_env`: the first 16 hex digits of the sha256
    of the value, a newline as \\036 (the script reads the kernel's environment block)."""
    return hashlib.sha256(value.replace("\n", "\x1e").encode()).hexdigest()[:16]


def excluded():
    """`[(pattern, reason)]` of the script's closed exclusion list BUILD_ENV_NOT."""
    text = SCRIPT.read_text().split("BUILD_ENV_NOT='", 1)[1].split("'", 1)[0]
    return [tuple(ln.split(None, 1)) for ln in text.splitlines() if ln.strip()]


def fingerprint(*args, stdin=False, env=None):
    """The script, run in a directory of its own (M7y): the probes it starts
    (rustup, cargo) resolve a relative value against the cwd -- a HOME of "1"
    left `1/.rustup` in the caller's tree, the worktree the suite ran from. The
    directory carries the tree's toolchain file only: rustup picks the
    toolchain by the cwd, as in the tree a station runs in."""
    env = clean_env() if env is None else env
    with tempfile.TemporaryDirectory() as cwd:
        shutil.copy(pathlib.Path(__file__).resolve().parents[2] / "rust-toolchain.toml", cwd)
        if stdin:
            return subprocess.run(["sh", "-s", "--"] + list(args), input=SCRIPT.read_text(),
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, env=env,
                                  cwd=cwd)
        return subprocess.run(["sh", str(SCRIPT)] + list(args), capture_output=True, text=True,
                              timeout=2 * kit_time.HANG, env=env, cwd=cwd)


def kit_python_version():
    """The version of the kit's one Python in the one form, `platform.python_version()`."""
    from scripts.pipeline import lane
    return subprocess.run(lane.python_argv("-c", "import platform; print(platform.python_version())"),
                          capture_output=True, text=True, timeout=kit_time.HANG).stdout.strip()


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestOneProducer(unittest.TestCase):
    """Re-check tv-bn M7q M1 (OR-T11-22, one fact one source): `lanes fingerprint`
    writes this script's answer into the register, the run's `executor.lane_fingerprint`
    writes its own after every run; the cache keys, the cache doubt and the drift NOTE
    compare the two. Two producers gave two forms (`Python 3.12.3` / `3.12.3`, `ext4` /
    `ext2/ext3`, `rust-lld` / a linker from RUSTFLAGS, `63` / `62.8`): after every
    `lanes fingerprint` the next run missed every cache entry, doubted its pass and
    noted a drift. The run's fingerprint is this script's."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        (self.root / "tree").mkdir()
        # the run measures in its tree (N-K1): a tree carries its toolchain file
        shutil.copy(REPO / "rust-toolchain.toml", self.root / "tree")

    def tearDown(self):
        self._tmp.cleanup()

    def test_lanes_fingerprint_and_the_run_give_one_fingerprint(self):
        from scripts.pipeline import cache, executor, gate
        # (the stations' RUSTFLAGS -- the lane's `linker=` as lane_sync.sh writes it --
        # and their CARGO_INCREMENTAL: lane.fingerprint sends both as words, the
        # executor has them in its `base_env`)
        mold = "--cfg tokio_unstable -C link-arg=-fuse-ld=mold"
        for flags, incr in ((None, None), (mold, "1"), (None, "0")):
            with self.subTest(flags=flags, incr=incr):
                extra = dict({} if flags is None else {"RUSTFLAGS": flags},
                             **({} if incr is None else {"CARGO_INCREMENTAL": incr}))
                env = clean_env(**extra)
                res = fingerprint(str(self.root), stdin=True, env=env)  # lane.fingerprint
                self.assertEqual(0, res.returncode, res.stderr)
                lanes = json.loads(res.stdout)
                run = executor.lane_fingerprint(str(self.root / "tree"), str(self.root), env)
                self.assertEqual("mold" if flags else "rust-lld", run["linker"])
                self.assertEqual(incr or "default", run["incremental_policy"])
                self.assertEqual(FIELDS, set(run))
                same = {k for k in FIELDS if k != "target_free_g"}
                self.assertEqual({k: lanes[k] for k in same}, {k: run[k] for k in same})
                # no drift NOTE, no cache misfire, no cache doubt after `lanes fingerprint`
                self.assertEqual([], executor.fingerprint_drift(run, lanes))
                specs = {"tool:python"}
                self.assertEqual(cache._tools(lanes, specs), cache._tools(run, specs))
                keys = {("gate-selftest", "all"): {"inputs": ["tool:python"]}}
                self.assertIsNone(gate._cache_doubt(keys, lanes, run))
                self.assertEqual(kit_python_version(), run["python"])

    def test_the_incremental_policy_is_measured_never_said(self):
        """Re-check tv-bn M7t A-M3: the policy is CARGO_INCREMENTAL of the
        environment the script runs in (`default` unset); a caller's word is no
        policy, and no file of the kit spells one."""
        from scripts.pipeline import executor, lane
        for incr, word, want in ((None, "", "default"), (None, "class=egon", "default"),
                                 ("0", "strand=1,pass=0", "0"), ("1", "", "1")):
            with self.subTest(incr=incr, word=word):
                env = clean_env() if incr is None else clean_env(CARGO_INCREMENTAL=incr)
                res = fingerprint(str(self.root), word, stdin=True, env=env)
                self.assertEqual(0, res.returncode, res.stderr)
                self.assertEqual(want, json.loads(res.stdout)["incremental_policy"])
        src = "".join(pathlib.Path(m.__file__).read_text() for m in (lane, executor)) \
            + (REPO / "scripts" / "lane_sync.sh").read_text() + SCRIPT.read_text()
        self.assertFalse("pass=0" in src, "a spelled policy in the kit")


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestLaneFingerprint(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_fingerprint_json_has_all_fields(self):
        res = fingerprint(str(self.root))
        self.assertEqual(0, res.returncode, res.stderr)
        fp = json.loads(res.stdout)
        self.assertEqual(FIELDS, set(fp))
        for key in ("vcpu", "ram_g", "target_free_g", "ulimit_n"):
            self.assertTrue(fp[key] is None or isinstance(fp[key], int), (key, fp[key]))
        self.assertGreaterEqual(fp["vcpu"], 1)
        self.assertEqual("rust-lld", fp["linker"])   # the toolchain's own by default

    def test_it_runs_from_stdin_like_on_a_lane(self):
        res = fingerprint(str(self.root), stdin=True,
                          env=clean_env(RUSTFLAGS="-C link-arg=-fuse-ld=mold", CARGO_INCREMENTAL="1"))
        self.assertEqual(0, res.returncode, res.stderr)
        fp = json.loads(res.stdout)
        self.assertEqual(("1", "mold"), (fp["incremental_policy"], fp["linker"]))

    def test_a_guest_without_visible_siblings_reports_no_cores(self):
        """OR-TB.S3.15: the lane's side of `schedule.physical_cores` -- a KVM guest that
        shows every vCPU as its own core (build02: 12 x 1 on 6 real cores) reports
        `cores` null; a bare host and a guest with visible siblings keep the count."""
        from scripts.tests.test_pipeline_lab_width import write_topology

        for guest, siblings, want in ((True, None, None), (False, 2, 6), (True, 2, 6),
                                      (False, None, 12)):
            with self.subTest(guest=guest, siblings=siblings):
                with tempfile.TemporaryDirectory() as sysroot:
                    write_topology(sysroot, 12, siblings, guest)
                    res = subprocess.run(["sh", str(SCRIPT), str(self.root)], capture_output=True,
                                         text=True, timeout=2 * kit_time.HANG,
                                         env=dict(os.environ, MECLAW_FP_SYSROOT=sysroot))
                    self.assertEqual(0, res.returncode, res.stderr)
                    self.assertEqual(want, json.loads(res.stdout)["cores"])

    def test_the_python_version_is_the_kits_not_the_first_on_path(self):
        """Re-check tv-bn M7p M1/H2: `python` enters the cache key; it is the version of
        the interpreter the stations run in (`lane.PYTHON`), not of a `python3` that
        stands first on PATH (`$HOME/.local/bin`, lane_sync.sh)."""
        fakes = self.root / "fakes"
        fakes.mkdir()
        fake = fakes / "python3"
        fake.write_text("#!/bin/sh\necho 'Python 0.0.0-path'\n")
        fake.chmod(0o755)
        res = subprocess.run(["/bin/sh", str(SCRIPT), str(self.root)], capture_output=True,
                             text=True, timeout=2 * kit_time.HANG,
                             env={"PATH": str(fakes) + os.pathsep + store_isolation.trapped_path(),
                                  "HOME": str(self.root)})
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(kit_python_version(), json.loads(res.stdout)["python"])

    def test_a_missing_tool_is_null(self):
        # an empty PATH beside the script's own additions: no node, no sccache
        res = subprocess.run(["/bin/sh", str(SCRIPT), str(self.root)], capture_output=True,
                             text=True, timeout=2 * kit_time.HANG,
                             env={"PATH": store_isolation.trapped_path(), "HOME": str(self.root)})
        self.assertEqual(0, res.returncode, res.stderr)
        fp = json.loads(res.stdout)
        self.assertEqual(FIELDS, set(fp))
        self.assertIsNone(fp["portal_venv"])
        self.assertIsNone(fp["playwright"])
        # no engine is ever "ok" on a lane that cannot be checked
        self.assertEqual({"chromium", "webkit"}, set(fp["browsers"]))
        self.assertNotIn("ok", fp["browsers"].values())


# A fake `ldd`: every WebKit binary misses libGL.so.1 (build02, GH #1161), the rest
# resolves; it notes the LD_LIBRARY_PATH it was asked under.
FAKE_LDD = r"""#!/bin/sh
echo "$1 $LD_LIBRARY_PATH" >> "$FAKE_LDD_LOG"
case "$1" in
    *"/webkit-"*) printf '\tlibGL.so.1 => not found\n\tlibc.so.6 => /lib/libc.so.6 (0x0)\n' ;;
    *) printf '\tlibc.so.6 => /lib/libc.so.6 (0x0)\n' ;;
esac
"""

# A fake `node` for the real start (GH #1161, build02 10.10.): `--version`, and the
# start probe `node -e <probe> <playwright> <engine> <secs>`. It notes the engine and
# the environment it was started under; a WebKit bundle that carries a copied
# `libEGL_mesa.so.0` in `sys/lib` dies at newPage the way build02's did, and
# FAKE_NODE_HANG makes the start hang.
FAKE_NODE = r"""#!/bin/sh
[ "$1" = "--version" ] && { echo v24.19.0; exit 0; }
[ "$1" = "-e" ] || exit 2
echo "$4 req=${MECLAW_REQUIRE_BROWSERS:-} wk=${FAKE_WKENV:-}" >> "$FAKE_NODE_LOG"
[ -n "${FAKE_NODE_HANG:-}" ] && exec sleep 30
case "$4" in
    webkit) if [ -f "$PLAYWRIGHT_BROWSERS_PATH/webkit-2359/minibrowser-wpe/sys/lib/libEGL_mesa.so.0" ]; then
        echo "launch ok"
        echo "does not start (newPage): browserContext.newPage: Target page, context or browser has been closed -- [pid=7][err] Could not create EGL display: no supported platform available. Aborting..."
        exit 1
    fi ;;
esac
echo ok
"""

# A fake `ldd` that resolves everything, as build02's did for its shadowed bundle:
# the copied libEGL_mesa is loaded by glvnd at run time, no ldd chain shows it.
FAKE_LDD_ALL_RESOLVE = r"""#!/bin/sh
echo "$1 $LD_LIBRARY_PATH" >> "$FAKE_LDD_LOG"
printf '\tlibc.so.6 => /lib/libc.so.6 (0x0)\n'
"""


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class BrowserLaneCase(unittest.TestCase):
    """OR-TB.S3.20 (A3): the lane check docks onto the executor's `browsers` field
    (`executor.browser_probe`, OR-TB.S3.18) -- per engine "ok" or why it cannot start,
    from the PINNED Playwright's revisions and `ldd` with the bundle's own library path.
    The same lane gives the same answer from both sides."""

    def setUp(self):
        from scripts.pipeline import executor
        self.executor = executor
        self._tmp = tempfile.TemporaryDirectory()
        base = self.base = pathlib.Path(self._tmp.name)
        self.root, self.bundles, self.fakes = base / "lane", base / "ms-playwright", base / "fakes"
        self.tree = self.root / "tree"
        nm = self.tree / "workshop" / "tools" / "node_modules"
        (nm / "playwright").mkdir(parents=True)
        (nm / "playwright-core").mkdir(parents=True)
        (nm / "playwright" / "package.json").write_text('{"version": "1.63.0"}')
        (nm / "playwright-core" / "browsers.json").write_text(json.dumps({"browsers": [
            {"name": "chromium", "revision": "1243"},
            {"name": "chromium-headless-shell", "revision": "1243"},
            {"name": "webkit", "revision": "2359"}]}, indent=2))
        for engine, (_, prefix, bins, _libs) in executor.BROWSER_BUNDLES.items():
            rev = "2359" if engine == "webkit" else "1243"
            for b in bins:
                f = self.bundles / ("%s-%s" % (prefix, rev)) / b
                f.parent.mkdir(parents=True, exist_ok=True)
                f.write_text("")
        self.fakes.mkdir()
        (self.tree / "workshop" / "tools" / "wkenv.sh").write_text("FAKE_WKENV=sourced\nexport FAKE_WKENV\n")
        for name, body in (("ldd", FAKE_LDD), ("node", FAKE_NODE)):
            (self.fakes / name).write_text(body)
            (self.fakes / name).chmod(0o755)
        self.log = base / "ldd.log"
        self.env = {"PATH": str(self.fakes) + os.pathsep + store_isolation.trapped_path(),
                    "HOME": str(base), "PLAYWRIGHT_BROWSERS_PATH": str(self.bundles),
                    "FAKE_LDD_LOG": str(self.log), "FAKE_NODE_LOG": str(base / "node.log"),
                    "MECLAW_DPKG_STATUS": str(base / "dpkg-status")}
        (base / "dpkg-status").write_text("Package: libegl-mesa0\nVersion: 25.2.8-0ubuntu0.24.04.4\n")
        self.node_log = base / "node.log"

    def tearDown(self):
        self._tmp.cleanup()

    def lane(self):
        res = subprocess.run(["sh", "-s", "--", str(self.root)], input=SCRIPT.read_text(),
                             capture_output=True, text=True, timeout=2 * kit_time.HANG, env=self.env)
        self.assertEqual(0, res.returncode, res.stderr)
        return json.loads(res.stdout)["browsers"]


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestLaneBrowsers(BrowserLaneCase):
    def test_a_missing_library_names_itself_and_chromium_stays_ok(self):
        got = self.lane()
        self.assertEqual("ok", got["chromium"])
        self.assertEqual("libraries not found: libGL.so.1 (webkit-2359, ldd with its own "
                         "LD_LIBRARY_PATH)", got["webkit"])
        # asked under the bundle wrapper's own library path, every launched binary
        wk = self.bundles / "webkit-2359"
        lines = [ln for ln in self.log.read_text().splitlines() if "/webkit-" in ln]
        self.assertEqual(4, len(lines))
        for ln in lines:
            self.assertTrue(ln.endswith(" %s/minibrowser-wpe/lib:%s/minibrowser-wpe/sys/lib"
                                        % (wk, wk)), ln)

    def path_without_node(self):
        """The test's PATH with every tool but `node`: one directory of links, first
        entry wins (the side-effect fakes stay in front). On a machine with node in
        /usr/bin the no-node case was skipped and never compared (re-check tb-s3 h2
        m-6)."""
        out = self.base / "no-node-bin"
        out.mkdir()
        for entry in self.env["PATH"].split(os.pathsep):
            if not os.path.isdir(entry):
                continue
            for name in sorted(os.listdir(entry)):
                src = os.path.join(entry, name)
                if (name == "node" or os.path.lexists(out / name) or not os.path.isfile(src)
                        or not os.access(src, os.X_OK)):
                    continue
                (out / name).symlink_to(src)
        return str(out)

    def no_node(self):
        (self.fakes / "node").unlink()
        self.env["PATH"] = self.path_without_node()
        self.assertIsNone(shutil.which("node", path=self.env["PATH"]))

    def test_the_lane_and_the_executor_give_the_same_answer(self):
        cases = {
            "libgl": lambda: None,
            "bundle-absent": lambda: shutil.rmtree(self.bundles / "webkit-2359"),
            "no-node": self.no_node,
        }
        for name, setup in cases.items():
            with self.subTest(case=name):
                setup()
                got = self.lane()
                self.assertEqual(self.executor.browser_probe(str(self.tree), self.env), got)
                if name == "no-node":
                    self.assertEqual({"chromium": "no node on the lane",
                                      "webkit": "no node on the lane"}, got)


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestBrowserStart(BrowserLaneCase):
    """GH #1161, build02 10.10.: stale laboratory copies in the WebKit bundle's
    `sys/lib` shadowed the system Mesa; `ldd` resolved every binary (glvnd loads
    libEGL_mesa at run time), the field said "ok", and every WebKit start died at
    newPage with "Could not create EGL display". The field now reports a real start:
    launch, newPage, setContent, evaluate, under wkenv.sh and the gate switch."""

    def setUp(self):
        super().setUp()
        (self.fakes / "ldd").write_text(FAKE_LDD_ALL_RESOLVE)
        self.sys_lib = self.bundles / "webkit-2359" / "minibrowser-wpe" / "sys" / "lib"
        self.sys_lib.mkdir(parents=True)

    def starts(self):
        if not self.node_log.exists():
            return []
        return self.node_log.read_text().splitlines()

    def test_a_shadowed_bundle_whose_start_dies_is_not_ok(self):
        (self.sys_lib / "libEGL_mesa.so.0").write_text("")
        got = self.lane()
        self.assertEqual("ok", got["chromium"])
        self.assertNotEqual("ok", got["webkit"])
        self.assertIn("Could not create EGL display", got["webkit"])
        self.assertIn("newPage", got["webkit"])
        self.assertEqual(got, self.executor.browser_probe(str(self.tree), self.env))

    def test_the_start_runs_under_wkenv_and_the_gate_switch(self):
        self.assertEqual({"chromium": "ok", "webkit": "ok"}, self.lane())
        self.assertEqual(["chromium req=1 wk=sourced", "webkit req=1 wk=sourced"], self.starts())

    def test_an_ok_is_kept_until_the_package_state_or_the_bundle_changes(self):
        self.lane()
        self.assertEqual(2, len(self.starts()))
        self.assertEqual({"chromium": "ok", "webkit": "ok"}, self.lane())
        self.assertEqual(2, len(self.starts()), "an unchanged lane starts no browser again")
        with open(self.env["MECLAW_DPKG_STATUS"], "a") as fh:
            fh.write("Package: libgallium\n")
        self.lane()
        self.assertEqual(4, len(self.starts()), "a new package state starts both again")
        (self.sys_lib / "libEGL_mesa.so.0").write_text("")
        self.assertIn("EGL", self.lane()["webkit"])
        self.assertEqual(["webkit req=1 wk=sourced"], self.starts()[4:],
                         "a changed bundle starts its engine again")

    def test_a_changed_sandbox_profile_starts_chromium_again(self):
        """An "ok" kept from before the userns profile went away would admit the lane
        on: the profile is part of the key."""
        apparmor = self.base / "apparmor.d"
        apparmor.mkdir()
        self.env["MECLAW_APPARMOR_DIR"] = str(apparmor)
        self.lane()
        self.assertEqual(2, len(self.starts()))
        (apparmor / "meclaw-playwright-chromium").write_text("profile x {\n  userns,\n}\n")
        self.lane()
        self.assertEqual(["chromium req=1 wk=sourced"], self.starts()[2:])

    def test_a_failed_start_is_never_kept(self):
        (self.sys_lib / "libEGL_mesa.so.0").write_text("")
        self.lane()
        self.lane()
        self.assertEqual(2, sum(1 for ln in self.starts() if ln.startswith("webkit")))

    def test_a_start_that_hangs_is_a_reason_after_the_timeout(self):
        self.env.update(FAKE_NODE_HANG="1", MECLAW_BROWSER_PROBE_SECS="1")
        got = self.lane()
        for engine in ("chromium", "webkit"):
            self.assertIn("no answer", got[engine])

    def test_a_missing_wkenv_is_a_reason(self):
        (self.tree / "workshop" / "tools" / "wkenv.sh").unlink()
        got = self.lane()
        self.assertIn("wkenv.sh", got["webkit"])
        self.assertEqual([], self.starts())


# A fake `playwright` for the real probe under a real node: per engine
# FAKE_PW_<ENGINE> = ok | launch | newPage | goto | hang; it notes every launch
# (engine, executablePath, the gate switch) in FAKE_PW_LOG.
FAKE_PLAYWRIGHT = r"""
const fs = require("fs");
const engine = (name) => ({ launch: async (o) => {
  const mode = process.env["FAKE_PW_" + name.toUpperCase()] || "ok";
  fs.appendFileSync(process.env.FAKE_PW_LOG, name + " " + (o.executablePath || "-") + " req=" +
                    process.env.MECLAW_REQUIRE_BROWSERS + "\n");
  if (mode === "launch") throw new Error("browserType.launch: Failed to launch " + name);
  if (mode === "hang") await new Promise(() => {});
  const page = {
    goto: async (url) => { if (mode === "goto") throw new Error("page.goto: net::ERR_FAILED at " + url);
                           if (!fs.existsSync(url.replace("file://", ""))) throw new Error("no page " + url); },
    evaluate: async () => "meclaw" };
  return { newContext: async () => ({ newPage: async () => {
             if (mode === "newPage") {
               o.logger.log("browser", "error", "[pid=7][err] Could not create EGL display: no supported platform available. Aborting...");
               throw new Error("browserContext.newPage: Target page, context or browser has been closed");
             }
             return page; } }),
           close: async () => {} };
} });
module.exports = { chromium: engine("chromium"), webkit: engine("webkit") };
"""


# The headless shell the CDP drivers' findBrowser picks, started directly the way the
# gh766 units start it: FAKE_NO_SANDBOX plays Ubuntu 24.04 without the userns profile.
FAKE_SANDBOX_SHELL = r"""#!/bin/sh
echo "$*" >> "$FAKE_SHELL_LOG"
if [ -n "${FAKE_NO_SANDBOX:-}" ]; then
    echo "[7:7:FATAL:zygote_host_impl_linux.cc(127)] No usable sandbox! See the docs." >&2
    exit 133
fi
echo "<html><head></head><body></body></html>"
"""


@unittest.skipUnless(shutil.which("sh") and shutil.which("node"), "needs a POSIX sh and node")
class TestBrowserStartProbe(BrowserLaneCase):
    """Re-Check S3 Teil 2: the probe itself under a real node, against a fake
    playwright -- launch, newContext, newPage, goto a local page, evaluate, close per
    engine, for chromium a second pass with the CDP drivers' findBrowser binary. The
    tb-s8 case (launch ok, newPage dies) is no "ok"."""

    def setUp(self):
        super().setUp()
        (self.fakes / "ldd").write_text(FAKE_LDD_ALL_RESOLVE)
        (self.fakes / "node").unlink()
        (self.fakes / "node").symlink_to(shutil.which("node"))
        nm = self.tree / "workshop" / "tools" / "node_modules" / "playwright"
        (nm / "package.json").write_text('{"name": "playwright", "version": "1.63.0", "main": "index.js"}')
        (nm / "index.js").write_text(FAKE_PLAYWRIGHT)
        home = pathlib.Path(self.env["HOME"])
        # findBrowser's root is PLAYWRIGHT_BROWSERS_PATH, as for lane_browser_deps.sh
        # and the units (tb-s10, re-check M-3): the bundle's own headless shell
        self.cdp = self.shell("1243")
        self.pw_log = home / "pw.log"
        self.shell_log = home / "shell.log"
        self.env.update(FAKE_PW_LOG=str(self.pw_log), TMPDIR=str(home),
                        FAKE_SHELL_LOG=str(self.shell_log))

    def launches(self):
        return self.pw_log.read_text().splitlines() if self.pw_log.exists() else []

    def shell(self, rev, root=None):
        exe = ((root or self.bundles) / ("chromium_headless_shell-%s" % rev)
               / "chrome-headless-shell-linux64" / "chrome-headless-shell")
        exe.parent.mkdir(parents=True, exist_ok=True)
        exe.write_text(FAKE_SANDBOX_SHELL)
        exe.chmod(0o755)
        return exe

    def install_pick(self):
        """The shell `lane_browser_deps.sh` (`headless_shell`) picks in this env."""
        deps = SCRIPT.parent / "lane_browser_deps.sh"
        fns = subprocess.run(["sed", "-n", "/^browsers_dir()/,/^}/p", str(deps)],
                             capture_output=True, text=True, check=True).stdout
        self.assertIn("headless_shell()", fns)
        return subprocess.run(["sh", "-c", fns + "\nheadless_shell\n"], capture_output=True,
                              text=True, env=self.env, check=True).stdout.strip()

    def test_findBrowser_takes_the_shell_under_PLAYWRIGHT_BROWSERS_PATH(self):
        """Re-check post-xhigh-8 M-3: findBrowser read only ~/.cache/ms-playwright, the
        units and lane_browser_deps.sh read PLAYWRIGHT_BROWSERS_PATH -- the admission
        checked another binary than the unit starts."""
        decoy = self.shell("1243", pathlib.Path(self.env["HOME"]) / ".cache" / "ms-playwright")
        self.assertEqual("ok", self.lane()["chromium"])
        self.assertIn("chromium %s req=1" % self.cdp, self.launches())
        self.assertNotIn("chromium %s req=1" % decoy, self.launches())
        self.assertEqual(str(self.cdp), self.install_pick())

    def test_of_several_versions_findBrowser_takes_the_units_and_the_installs_shell(self):
        """Several bundles: the bytewise last whose binary exists -- what the units'
        `playwright_chromium` (sorted PathBufs, pop) and `headless_shell` (LC_ALL=C sort |
        tail) start, inside the AppArmor profile's glob -- never the first readdir hit."""
        self.shell("1243", pathlib.Path(self.env["HOME"]) / ".cache" / "ms-playwright")
        # the old pick, the first readdir hit (libuv hands names out sorted), is 1243;
        # 9999 sorts after the pick but holds no binary
        self.shell("1250")
        want = self.shell("999")
        self.shell("1300")
        (self.bundles / "chromium_headless_shell-9999").mkdir()
        self.assertEqual("ok", self.lane()["chromium"])
        picked = [l.split(" ")[1] for l in self.launches() if l.startswith("chromium /")]
        self.assertEqual([str(want)], picked)
        self.assertEqual(str(want), self.install_pick())
        glob = "%s/chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell"
        self.assertTrue(fnmatch.fnmatchcase(picked[0], glob % self.bundles))

    def test_every_step_passes_and_chromium_runs_findBrowsers_binary_too(self):
        self.assertEqual({"chromium": "ok", "webkit": "ok"}, self.lane())
        self.assertEqual(["chromium - req=1", "chromium %s req=1" % self.cdp, "webkit - req=1"],
                         self.launches())
        self.assertEqual(["--headless --dump-dom about:blank"],
                         self.shell_log.read_text().splitlines())

    def test_a_shell_without_a_usable_sandbox_refuses_chromium_for_the_admission(self):
        """tb-s10 deckel 17: playwright launches chromium with its own --no-sandbox, so
        both passes read "ok" on a lane where the headless shell cannot start its own
        sandbox (Ubuntu 24.04, no userns profile) -- the lane was admitted and
        `browser:display` died in gh766 with "No usable sandbox!". The shell started
        the way those units start it decides, and the reason names the install."""
        from scripts.pipeline import schedule
        self.env["FAKE_NO_SANDBOX"] = "1"
        got = self.lane()
        self.assertEqual("ok", got["webkit"])
        self.assertIn("does not start (sandbox of findBrowser %s)" % self.cdp, got["chromium"])
        self.assertIn("No usable sandbox!", got["chromium"])
        self.assertIn("scripts/strand.sh lanes browsers <lane> install chromium", got["chromium"])
        self.assertNotIn("no-sandbox", self.shell_log.read_text())
        self.assertEqual(["chromium"], list(schedule.browser_refusals({"browsers": got})))
        self.assertEqual(got, self.executor.browser_probe(str(self.tree), self.env))

    def test_launch_ok_and_newpage_dies_is_not_ok(self):
        # tb-s8 / build02: the engine launches, the first page closes the browser
        self.env["FAKE_PW_WEBKIT"] = "newPage"
        got = self.lane()
        self.assertEqual("ok", got["chromium"])
        self.assertEqual("does not start (newPage): browserContext.newPage: Target page, context or "
                         "browser has been closed -- [pid=7][err] Could not create EGL display: no "
                         "supported platform available. Aborting...", got["webkit"])

    def test_a_failing_launch_and_goto_name_their_stage(self):
        self.env.update(FAKE_PW_WEBKIT="launch", FAKE_PW_CHROMIUM="goto")
        got = self.lane()
        self.assertIn("does not start (launch): browserType.launch: Failed to launch webkit",
                      got["webkit"])
        self.assertIn("does not start (goto): page.goto: net::ERR_FAILED at file://", got["chromium"])

    def test_a_hanging_launch_is_a_reason_within_the_engines_seconds(self):
        self.env.update(FAKE_PW_WEBKIT="hang", MECLAW_BROWSER_PROBE_SECS="1")
        got = self.lane()
        self.assertEqual("ok", got["chromium"])
        self.assertIn("no answer within 1s at launch", got["webkit"])

    def test_no_chromium_for_findBrowser_is_not_ok(self):
        # MECLAW_CHROMIUM wins in findBrowser; a system chromium on PATH would else answer
        self.env["MECLAW_CHROMIUM"] = str(self.cdp) + ".absent"
        got = self.lane()
        self.assertIn("does not start (findBrowser): the CDP drivers find no chromium", got["chromium"])
        self.assertEqual("ok", got["webkit"])

    def test_a_wrapper_script_is_left_to_the_real_start(self):
        # build03/build04 10.10.: chrome-headless-shell is a sh wrapper around
        # chrome-headless-shell.real; ldd says "not a dynamic executable", exit 1. ldd
        # knows nothing about a script, the start decides.
        shell = (self.bundles / "chromium_headless_shell-1243" / "chrome-headless-shell-linux64"
                 / "chrome-headless-shell")
        shell.write_text('#!/bin/sh\nexec "$0.real" "$@"\n')
        real = shell.with_name(shell.name + ".real")
        real.write_text(FAKE_SANDBOX_SHELL)
        real.chmod(0o755)
        (self.fakes / "ldd").write_text("#!/bin/sh\necho '\tnot a dynamic executable'\nexit 1\n")
        got = self.lane()
        self.assertEqual("ok", got["chromium"])
        self.assertIn("exited 1", got["webkit"])

    def test_an_ldd_that_fails_is_a_reason(self):
        (self.fakes / "ldd").write_text("#!/bin/sh\nexit 1\n")
        self.cdp.write_text("")  # a binary for ldd, not the fake shell script ldd skips
        got = self.lane()
        for engine in ("chromium", "webkit"):
            self.assertIn("exited 1", got[engine])
        self.assertEqual([], self.launches())


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestBuildEnvironment(unittest.TestCase):
    """Re-check tv-bn M7r A-M2 (OR-T11-22): `build_env` had no producer -- the cache
    key read it, nobody wrote it --, RUSTFLAGS and CARGO_PROFILE_* entered no key,
    and the local linker was the literal `rust-lld`. Both are the script's answer
    for the environment it runs in: the stations'."""

    def build(self, **extra):
        res = fingerprint("--build-env", ".", env=clean_env(**extra))
        self.assertEqual(0, res.returncode, res.stderr)
        return json.loads(res.stdout)

    def base(self):
        """`build_env` of `clean_env()`: PATH alone (HOME is excluded)."""
        return {"PATH": digest(os.environ["PATH"])}

    def with_(self, extra):
        return dict(self.base(), **{k: digest(v) for k, v in extra.items()})

    def test_no_lane_setting_enters_the_build_environment(self):
        # tv-bn M7z: `lanes status` (python, `lane.sync_env`: slots "1", threads,
        # jobs) and `lane_sync.sh status` (strand.sh: slots "") measured ONE local
        # lane with two fingerprints -- the settings strand.sh exports reached
        # build_env. What they change in a build reaches it through measured names.
        body = re.search(r"\nlane_settings\(\) \{.*?\n\}\n",
                         (REPO / "scripts" / "strand.sh").read_text(), re.S).group(0)
        names = re.findall(r"export (MECLAW_LANE_\w+)", body)
        self.assertGreaterEqual(len(names), 8, body)
        self.assertEqual(self.base(), self.build(**{n: "1" for n in names})["build_env"])

    def test_the_linker_and_the_build_environment_come_from_the_environment(self):
        mold = "--cfg tokio_unstable -C link-arg=-fuse-ld=mold"
        wild = "--cfg tokio_unstable -C linker=clang -C link-arg=--ld-path=/opt/bin/wild"
        for extra, linker in (({}, "rust-lld"), ({"RUSTFLAGS": mold}, "mold"),
                              ({"RUSTFLAGS": wild}, "wild"), ({"RUSTFLAGS": "-C linker=clang"}, "clang"),
                              ({"CARGO_BUILD_RUSTFLAGS": "-Clinker=/usr/bin/gcc"}, "gcc"),
                              ({"CARGO_ENCODED_RUSTFLAGS": "-C\x1flink-arg=-fuse-ld=lld"},
                               "lld")):
            with self.subTest(extra=extra):
                got = self.build(**extra)
                self.assertEqual(linker, got["linker"])
                self.assertEqual(self.with_(extra), got["build_env"])
        got = self.build(RUSTFLAGS="", CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_RELEASE_LTO="fat",
                         CARGO_INCREMENTAL="1", CARGO_BUILD_JOBS="4", RUSTC_WRAPPER="sccache")
        # an empty RUSTFLAGS is kept (it overrides the config's flags); how fast is never in it
        self.assertEqual(self.with_({"RUSTFLAGS": "", "CARGO_PROFILE_DEV_DEBUG": "0",
                                     "CARGO_PROFILE_RELEASE_LTO": "fat"}), got["build_env"])
        # a newline is no space: two values stay two values
        self.assertNotEqual(self.build(RUSTFLAGS="a\nb")["build_env"],
                            self.build(RUSTFLAGS="a b")["build_env"])

    def test_every_name_that_changes_the_build_is_in_it(self):
        """Orchestrator OR-T12-18 A-M1 (re-check tv-bn M7w A-M1, fail-open): the rule
        named prefixes, and a build-script name of a dependency, a target form with
        dashes (dash drops it from its variables) and the C driver's paths entered no
        key. The rule is the WHOLE environment minus a closed exclusion list: every
        name the re-check measured, and a name nobody ever heard of, is in."""
        triple = subprocess.run(["rustc", "-vV"], capture_output=True, text=True,
                                timeout=kit_time.HANG).stdout if shutil.which("rustc") else ""
        host = next((ln.split(": ", 1)[1] for ln in triple.splitlines() if ln.startswith("host: ")), "")
        target = "CARGO_TARGET_%s_" % host.upper().replace("-", "_").replace(".", "_")
        for name, value in (("RUSTC", "/opt/other/rustc"), ("CARGO_BUILD_RUSTC", "/opt/other/rustc"),
                            ("RUSTC_BOOTSTRAP", "1"), ("RUSTC_WORKSPACE_WRAPPER", "clippy-driver"),
                            ("RUSTUP_TOOLCHAIN", "nightly"), ("CARGO", "/opt/other/cargo"),
                            ("CC", "clang"), ("CXX", "clang++"), ("AR", "llvm-ar"), ("CFLAGS", "-O3"),
                            ("CXXFLAGS", "-O3"), ("LDFLAGS", "-s"), ("CC_x86_64_unknown_linux_gnu", "clang"),
                            ("TARGET_CC", "clang"), ("CRATE_CC_NO_DEFAULTS", "1"),
                            ("PKG_CONFIG_PATH", "/opt/pc"), ("CARGO_UNSTABLE_BUILD_STD", "std"),
                            ("CARGO_NEW_NAME_NOBODY_LISTED", "x"),
                            # M7w A-M1 (a): the build scripts of the dependencies
                            ("LIBSQLITE3_FLAGS", "-DSQLITE_ENABLE_FTS5"), ("LIBSQLITE3_SYS_USE_PKG_CONFIG", "1"),
                            ("SQLITE_MAX_COLUMN", "4000"), ("SQLITE_MAX_EXPR_DEPTH", "0"),
                            ("SQLITE_MAX_VARIABLE_NUMBER", "99999"), ("ICU4X_DATA_DIR", "/opt/icu"),
                            ("RING_PREGENERATE_ASM", "1"),
                            # (b) the target forms with dashes, which dash never shows
                            ("CC_x86_64-unknown-linux-gnu", "clang"), ("CFLAGS_x86_64-unknown-linux-gnu", "-O3"),
                            ("AR_x86_64-unknown-linux-gnu", "llvm-ar"), ("ARFLAGS_x86_64-unknown-linux-gnu", "crs"),
                            # (c) the C driver
                            ("CPATH", "/opt/inc"), ("C_INCLUDE_PATH", "/opt/inc"), ("CPLUS_INCLUDE_PATH", "/opt/inc"),
                            ("LIBRARY_PATH", "/opt/lib"), ("CXXSTDLIB", "c++"),
                            # a name nobody ever listed, nor any prefix of it
                            ("ZQX_INVENTED_BY_THE_PIN", "1"), ("zqx.invented-too", "1")):
            with self.subTest(name=name):
                self.assertEqual(self.with_({name: value}), self.build(**{name: value})["build_env"])
        # the closed exclusion list: each line a name or glob and its one reason; none of them
        # ever changes the answer
        rows = excluded()
        self.assertGreater(len(rows), 10)
        for pattern, reason in rows:
            with self.subTest(out=pattern):
                self.assertGreaterEqual(len(reason.split()), 3, pattern)
                # HOME: a directory of its own -- rustup creates `$HOME/.rustup` (a relative "1" left one in the tree)
                value = self.scratch_home() if pattern == "HOME" else "1"
                self.assertEqual(self.base(), self.build(**{pattern.replace("*", "ZQX"): value})["build_env"])
        for name in ("CARGO_INCREMENTAL", "CARGO_BUILD_JOBS", "RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER",
                     "RUST_TEST_THREADS", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_HOME",
                     "CARGO_LOG", "CARGO_TERM_COLOR", "CARGO_NET_OFFLINE", "CARGO_HTTP_TIMEOUT",
                     "CARGO_REGISTRY_TOKEN", "CARGO_REGISTRIES_X_TOKEN", "SSH_CONNECTION",
                     "CLAUDE_CODE_SESSION_ID", "PWD", "SHLVL", "MECLAW_GATE_ARCHIVE"):
            with self.subTest(out=name):
                self.assertEqual(self.base(), self.build(**{name: "1"})["build_env"])
        if not host:
            self.skipTest("no rustc names the host triple")
        mold = "-C link-arg=-fuse-ld=mold"
        for extra, linker in (({target + "RUSTFLAGS": mold}, "mold"), ({target + "LINKER": "clang"}, "clang"),
                              # cargo's order: RUSTFLAGS before the target's, the target's before build's
                              ({target + "RUSTFLAGS": mold, "RUSTFLAGS": "-C link-arg=-fuse-ld=wild"}, "wild"),
                              ({target + "RUSTFLAGS": mold, "CARGO_BUILD_RUSTFLAGS": "-C linker=gcc"}, "mold"),
                              ({target + "LINKER": "clang", "RUSTFLAGS": "-C linker=gcc"}, "gcc")):
            with self.subTest(extra=extra):
                got = self.build(**extra)
                self.assertEqual(linker, got["linker"])
                self.assertEqual(self.with_(extra), got["build_env"])
        # a target name and no triple to read it with: no linker, no key (fail-closed)
        self.assertIsNone(self.build(**{target + "LINKER": "clang", "RUSTC": "/nonexistent/rustc"})["linker"])

    def test_a_relative_value_writes_nothing_into_the_callers_tree(self):
        """M7y: the probes of the script resolve a relative HOME against the cwd;
        a run from the worktree left `1/.rustup` in it (untracked, found after the
        M7x run). The helper runs in a directory of its own."""
        rel = "m7y-rel-home-%d" % os.getpid()
        self.build(HOME=rel)
        self.assertFalse(os.path.exists(rel))
        self.assertFalse((pathlib.Path(__file__).resolve().parents[2] / rel).exists())

    def scratch_home(self):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        return d.name

    def test_no_value_is_in_clear(self):
        """The whole environment carries session tokens: a value enters only as its digest."""
        secret = "tok-%s" % os.getpid()
        res = fingerprint(tempfile.gettempdir(), env=clean_env(CLAUDE_X_TOKEN=secret, ZQX_TOKEN=secret))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertNotIn(secret, res.stdout)
        self.assertEqual(digest(secret), json.loads(res.stdout)["build_env"]["ZQX_TOKEN"])

    def fake_rustc(self, version):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        path = pathlib.Path(d.name, "rustc")
        path.write_text("#!/bin/sh\ncase \"$1\" in -vV) printf 'rustc %s\\nhost: x86_64-unknown-linux-gnu\\n' ;;"
                        " *) echo 'rustc %s (fake)' ;; esac\n" % (version, version))
        path.chmod(0o755)
        return str(path)

    def test_the_rustc_is_the_one_that_builds(self):
        """Orchestrator OR-T12-18 A-M2 (re-check tv-bn M7w A-M2): the field measured the
        PATH's rustc while RUSTC built -- a compiler update behind the same RUSTC path was
        CACHED. cargo's order: RUSTC, then CARGO_BUILD_RUSTC, then PATH."""
        fake, other = self.fake_rustc("9.9.9"), self.fake_rustc("8.8.8")
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path_rustc = self.build()["rustc"]
        for extra, want in (({"RUSTC": fake}, "rustc 9.9.9 (fake)"),
                            ({"CARGO_BUILD_RUSTC": fake}, "rustc 9.9.9 (fake)"),
                            ({"RUSTC": fake, "CARGO_BUILD_RUSTC": other}, "rustc 9.9.9 (fake)"),
                            ({}, path_rustc)):
            with self.subTest(extra=sorted(extra)):
                self.assertEqual(want, self.build(**extra)["rustc"])
                res = fingerprint(tmp.name, env=clean_env(**extra))
                self.assertEqual(want, json.loads(res.stdout)["rustc"])
        self.assertNotEqual("rustc 9.9.9 (fake)", path_rustc)

    def test_the_full_fingerprint_and_the_build_part_are_one_answer(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        tree = os.path.join(tmp.name, "tree")
        os.mkdir(tree)
        shutil.copy(REPO / "rust-toolchain.toml", tree)
        env = clean_env(RUSTFLAGS="-C link-arg=-fuse-ld=mold", CARGO_PROFILE_TEST_OPT_LEVEL="1")
        from scripts.pipeline import executor
        full = executor.lane_fingerprint(tree, tmp.name, env)
        self.assertNotIn("unanswered", full)
        part = executor.build_fingerprint(env, tree)
        self.assertEqual(part, {k: full[k] for k in executor.BUILD_PART})
        self.assertEqual("mold", part["linker"])
        self.assertEqual(self.with_({"RUSTFLAGS": "-C link-arg=-fuse-ld=mold",
                                     "CARGO_PROFILE_TEST_OPT_LEVEL": "1"}), part["build_env"])

    def test_no_caller_sends_the_linker_as_a_word(self):
        """The lane's `linker=` reaches the script only through RUSTFLAGS (lane_env)."""
        from scripts.pipeline import lane
        src = pathlib.Path(lane.__file__).read_text()
        self.assertNotIn('lane.opts.get("linker", ""))', src.split("def fingerprint(", 1)[1].split("\ndef ", 1)[0])
        sync = (REPO / "scripts" / "lane_sync.sh").read_text()
        body = sync.split("cmd_fingerprint() {", 1)[1].split("\n}\n", 1)[0]
        self.assertNotIn("MECLAW_LANE_LINKER", body)
        self.assertIn("lane_env test", body)


# A rustup proxy in small: rustc and cargo answer with the channel of the first
# `rust-toolchain.toml` from the directory they start in up (rustup's rule), else
# "default" (`rustup default`).
FAKE_PROXY = """#!/bin/sh
d=$PWD; ch=default
while :; do
    if [ -f "$d/rust-toolchain.toml" ]; then
        ch=$(sed -n 's/^channel *= *"\\(.*\\)"/\\1/p' "$d/rust-toolchain.toml"); break
    fi
    [ "$d" = / ] && break
    d=$(dirname "$d")
done
case "$1" in
    -vV) printf '%s %s\\nhost: x86_64-unknown-linux-gnu\\n' TOOL "$ch" ;;
    *) echo "TOOL $ch (fake)" ;;
esac
"""


class TestTheTreesToolchain(unittest.TestCase):
    """Re-check bundle N kit-fp M1/M2 (N-K1, N-K2): the key measured rustc and cargo
    where the gate stood (its cwd; on a host `$HOME`, where `rustup default` answers)
    while the stations build in the tree, whose `rust-toolchain.toml` rustup follows;
    and cargo's configuration outside the tree (`$CARGO_HOME/config.toml`, a
    `.cargo/config.toml` above the tree) entered no key. A rustup in small (FAKE_PROXY)
    answers by the directory it starts in."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, str(self.tmp), True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        for tool in ("rustc", "cargo"):
            path = self.bin / tool
            path.write_text(FAKE_PROXY.replace("TOOL", tool))
            path.chmod(0o755)
        # the one search path of this class (local and host): the fakes, then the
        # caller's PATH with the guard's trap in front -- never a literal one
        # (test_store_isolation, test_no_test_hands_a_subprocess_a_fixed_search_path)
        self.path = str(self.bin) + os.pathsep + store_isolation.trapped_path(os.environ["PATH"])
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.tree = self.tmp / "lane" / "tree"
        self.tree.mkdir(parents=True)
        (self.tree / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "7.7.7"\n')
        self.elsewhere = self.tmp / "elsewhere"
        self.elsewhere.mkdir()
        here = os.getcwd()
        os.chdir(str(self.elsewhere))  # the gate started from anywhere (`--repo <tree>`)
        self.addCleanup(os.chdir, here)

    def env(self, **extra):
        return dict({"PATH": self.path, "HOME": str(self.home)}, **extra)

    def part(self, **extra):
        from scripts.pipeline import executor
        return executor.build_fingerprint(self.env(**extra), str(self.tree))

    def test_the_build_part_is_the_trees_toolchain_from_any_cwd(self):
        from scripts.pipeline import executor
        part = self.part()
        self.assertEqual("rustc 7.7.7 (fake)", part["rustc"])
        self.assertEqual("cargo 7.7.7 (fake)", part["cargo"])
        # the run's own fingerprint (`fp_after`) measures in the same tree
        full = executor.lane_fingerprint(str(self.tree), str(self.tmp), self.env())
        self.assertNotIn("unanswered", full)
        self.assertEqual(part, {k: full[k] for k in executor.BUILD_PART})
        # a pass on this machine: `stations_build` of the tree, never of the cwd
        from unittest import mock
        with mock.patch.dict(os.environ, self.env(), clear=True):
            local = executor.stations_build(None, {"MECLAW_GATE_MODE": "strand"}, str(self.tree))
        self.assertEqual(("rustc 7.7.7 (fake)", "cargo 7.7.7 (fake)"), (local["rustc"], local["cargo"]))
        # the register's measure in a login directory answers the default: it never keys
        res = subprocess.run(["sh", str(SCRIPT), str(self.tmp)], capture_output=True, text=True,
                             timeout=2 * kit_time.HANG, env=self.env(), cwd=str(self.home))
        self.assertEqual("rustc default (fake)", json.loads(res.stdout)["rustc"])

    def test_a_local_measure_without_a_tree_answers_nothing(self):
        from scripts.pipeline import executor
        self.assertEqual(dict.fromkeys(executor.BUILD_PART),
                         executor.build_fingerprint(self.env(), ""))
        res = subprocess.run(["sh", str(SCRIPT), "--build-env"], capture_output=True, text=True,
                             timeout=2 * kit_time.HANG, env=self.env(), cwd=str(self.tree))
        self.assertEqual(2, res.returncode, res.stdout)

    def test_a_host_measures_the_toolchain_of_the_tree_it_will_get(self):
        """On a host the script starts in the login directory, and the tree there is
        the last pass's or none: the toolchain files of THIS tree travel as data."""
        from unittest import mock
        from scripts.pipeline import executor, lane
        host_home = self.tmp / "host-home"
        host_home.mkdir()
        (host_home / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.0.0"\n')
        host_tree = self.tmp / "host" / "tree"   # not there yet
        host_tmp = self.tmp / "host-tmp"
        host_tmp.mkdir()
        # cargo's configuration above the host's tree (re-check bundle N delta kit-fp M1:
        # without a file the empty list hashed alike from every start -- mutant MH1, the
        # scan from the temporary directory, stayed green)
        empty = self.part()["cargo_config"]
        flags = '[build]\nrustflags = ["-C", "target-cpu=native"]\n'
        (self.tmp / "host" / ".cargo").mkdir(parents=True)
        (self.tmp / "host" / ".cargo" / "config.toml").write_text(flags)
        seen = []

        def remote_argv(ln, script, **kw):
            seen.append(script)
            return ["env", "-i", "PATH=%s" % self.path, "HOME=%s" % host_home,
                    "TMPDIR=%s" % host_tmp, "bash", "-c", "cd \"$HOME\" && " + script]

        ln = lane.Lane(name="lane-h", ssh="builder@lane-h.invalid")
        with mock.patch.object(lane, "_remote_argv", side_effect=remote_argv):
            got = executor.stations_build(ln, {"MECLAW_GATE_MODE": "strand"}, str(self.tree),
                                          str(host_tree))
            self.assertEqual(dict.fromkeys(executor.BUILD_PART),
                             executor.stations_build(ln, {}, str(self.tree), None))
        self.assertEqual(1, len(seen), seen)
        self.assertEqual(("rustc 7.7.7 (fake)", "cargo 7.7.7 (fake)"), (got["rustc"], got["cargo"]))
        self.assertNotEqual(empty, got["cargo_config"])
        # the same file above the local tree is the same build
        self.assertEqual(empty, self.part()["cargo_config"])
        (self.tree.parent / ".cargo").mkdir()
        (self.tree.parent / ".cargo" / "config.toml").write_text(flags)
        self.assertEqual(self.part()["cargo_config"], got["cargo_config"])
        self.assertEqual(digest("strand"), got["build_env"]["MECLAW_GATE_MODE"])
        self.assertEqual([], os.listdir(str(host_tmp)))  # the directory went with the shell

    def test_the_cargo_configuration_outside_the_tree_enters_the_key(self):
        base = self.part()["cargo_config"]
        self.assertRegex(base, r"^[0-9a-f]{16}$")
        flags = '[build]\nrustflags = ["-C", "target-cpu=native"]\n'
        cargo_home = self.tmp / "cargo-home"
        cargo_home.mkdir()
        self.assertEqual(base, self.part(CARGO_HOME=str(cargo_home))["cargo_config"])
        (cargo_home / "config.toml").write_text(flags)
        with_home = self.part(CARGO_HOME=str(cargo_home))["cargo_config"]
        self.assertNotEqual(base, with_home)
        # the same file in another CARGO_HOME is the same build
        other = self.tmp / "cargo-home-2"
        other.mkdir()
        (other / "config.toml").write_text(flags)
        self.assertEqual(with_home, self.part(CARGO_HOME=str(other))["cargo_config"])
        # without CARGO_HOME: $HOME/.cargo, both file names
        for name in ("config.toml", "config"):
            with self.subTest(home_file=name):
                (self.home / ".cargo").mkdir(exist_ok=True)
                (self.home / ".cargo" / name).write_text(flags)
                self.assertNotEqual(base, self.part()["cargo_config"])
                (self.home / ".cargo" / name).unlink()
        self.assertEqual(base, self.part()["cargo_config"])
        # a directory above the tree (cargo walks up from where it starts)
        for above in (self.tree.parent, self.tmp):
            with self.subTest(above=str(above)):
                (above / ".cargo").mkdir(exist_ok=True)
                (above / ".cargo" / "config.toml").write_text(flags)
                self.assertNotEqual(base, self.part()["cargo_config"])
                (above / ".cargo" / "config.toml").unlink()
        # the tree's own configuration is its source (the inputs of a key), not this field
        (self.tree / ".cargo").mkdir()
        (self.tree / ".cargo" / "config.toml").write_text(flags)
        self.assertEqual(base, self.part()["cargo_config"])

    @unittest.skipIf(os.geteuid() == 0, "root reads every file")
    def test_a_configuration_that_cannot_be_read_is_no_key(self):
        (self.home / ".cargo").mkdir()
        cfg = self.home / ".cargo" / "config.toml"
        cfg.write_text("[build]\n")
        cfg.chmod(0)
        self.addCleanup(cfg.chmod, 0o644)
        self.assertIsNone(self.part()["cargo_config"])

    def test_the_exclusion_reasons_name_the_configuration(self):
        reasons = dict(excluded())
        for name in ("CARGO_HOME", "HOME"):
            self.assertIn("cargo_config", reasons[name])


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestNoAnswer(unittest.TestCase):
    """Re-check tv-bn M7r A-M1 (fail-open): a lane_fingerprint.sh that did not
    answer gave a fingerprint of nulls; the register took it, the next run keyed
    its cache over null tools, null == null raised no doubt, and the entries made
    there answered every later null run CACHED across a change of rustc or python."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        (self.root / "tree").mkdir()

    def tearDown(self):
        self._tmp.cleanup()

    def run_with(self, script):
        from scripts.pipeline import executor
        old = executor.LANE_FINGERPRINT_SH
        executor.LANE_FINGERPRINT_SH = str(script)
        try:
            return executor.lane_fingerprint(str(self.root / "tree"), str(self.root), clean_env())
        finally:
            executor.LANE_FINGERPRINT_SH = old

    def said(self, name, doc):
        path = self.root / ("%s.json" % name)
        path.write_text(json.dumps(doc), encoding="utf-8")
        return path

    def fake(self, body):
        path = self.root / ("fake_%d.sh" % len(list(self.root.glob("fake_*.sh"))))
        path.write_text(body, encoding="utf-8")
        return path

    def test_a_script_that_does_not_answer_is_no_fingerprint(self):
        whole = json.loads(fingerprint(str(self.root)).stdout)
        half = {k: v for k, v in whole.items() if k != "python"}
        for why, script in (("exit 2", self.root / "missing.sh"),
                            ("exit 3", self.fake("cat '%s'; exit 3\n" % self.said("whole", whole))),
                            ("no field python", self.fake("cat '%s'\n" % self.said("half", half))),
                            ("no JSON", self.fake("printf '{\"rustc\": \"1\n\"}'\n"))):
            with self.subTest(why=why):
                fp = self.run_with(script)
                self.assertTrue(fp.get("unanswered", "").startswith(why), fp.get("unanswered"))
                self.assertEqual({None}, {fp[k] for k in FIELDS if k != "browsers"})

    def test_no_fingerprint_keys_nothing_and_records_nothing(self):
        from scripts.pipeline import cache, gate
        nulls = self.run_with(self.root / "missing.sh")
        good = json.loads(fingerprint(str(self.root)).stdout)
        keys = {("gate-selftest", ""): {"inputs": ["tool:python"]}}
        # null == null is no proof: a run without an answer doubts its pass
        self.assertIsNotNone(gate._cache_doubt(keys, nulls, nulls))
        self.assertIsNotNone(gate._cache_doubt(keys, good, nulls))
        partial = dict(good, python=None)
        self.assertIsNotNone(gate._cache_doubt(keys, partial, partial))
        self.assertIsNone(gate._cache_doubt(keys, good, good))
        # and keys nothing (no tool version, no key: never CACHED, never stored)
        self.assertEqual({}, cache.keys_for_plan({"stations": []}, str(self.root), nulls, "x",
                                                 metadata={}))


@unittest.skipUnless(shutil.which("bash"), "needs bash")
class TestIncrementalPolicy(unittest.TestCase):
    """Re-check tv-bn M7t A-M3 (OR-T11-22): the script said `strand=1,pass=0` while
    the gate's passes (integration, release) built incrementally -- `lane_sync.sh env`
    sets nothing there; its pin chose the modes itself (main-pass, m0), which no gate
    ever sends. Now the policy is measured: for every mode the gate sends
    (`gate.MODES`), the fingerprint under that mode's words says that mode's
    CARGO_INCREMENTAL, and the run's own fingerprint is its stations' -- no echo."""

    def words(self, mode, root):
        env = dict(os.environ, MECLAW_LANE_NAME="lx", MECLAW_LANE_ROOT=root,
                   MECLAW_LANE_THREADS="", MECLAW_LANE_SCCACHE="", MECLAW_LANE_LINKER="")
        res = subprocess.run(["bash", str(REPO / "scripts" / "lane_sync.sh"), "env", mode],
                             env=env, capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        return dict(w.split("=", 1) for w in shlex.split(res.stdout) if "=" in w)

    def test_every_mode_the_gate_sends_says_its_own_policy(self):
        from scripts.pipeline import executor, gate
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        (pathlib.Path(tmp.name) / "tree").mkdir()
        self.assertEqual(("strand", "integration", "release"), gate.MODES)
        seen = set()
        for mode in gate.MODES:
            with self.subTest(mode=mode):
                words = self.words(mode, tmp.name)
                incr = {k: v for k, v in words.items() if k == "CARGO_INCREMENTAL"}
                env = clean_env(**incr)
                said = json.loads(fingerprint(tmp.name, env=env).stdout)["incremental_policy"]
                run = executor.lane_fingerprint(str(pathlib.Path(tmp.name) / "tree"), tmp.name, env)
                want = words.get("CARGO_INCREMENTAL", "default")
                self.assertEqual((want, want), (said, run["incremental_policy"]))
                seen.add(want)
        # strand builds incrementally by word, the passes by cargo's default -- said as such
        self.assertEqual({"1", "default"}, seen)

    def test_the_run_measures_its_policy_and_echoes_no_register(self):
        from scripts.pipeline import executor
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        tree = pathlib.Path(tmp.name) / "tree"
        tree.mkdir()
        ex = executor._Run.__new__(executor._Run)
        ex.d = {"fingerprint_expected": {"incremental_policy": "1"}}
        ex.tree, ex.target, ex.head, ex.by_name = str(tree), tmp.name, {}, {}
        ex.base_env = clean_env(CARGO_INCREMENTAL="0")
        ex.emit = lambda *a, **kw: None
        ex.note_row = lambda *a, **kw: None
        ex._fingerprint()
        self.assertEqual("0", ex.head["fingerprint"]["incremental_policy"])
        # no third spelling of the rule
        from scripts.pipeline import lane
        self.assertFalse(hasattr(lane, "INCREMENTAL_ON") or hasattr(lane, "INCREMENTAL_OFF"))


if __name__ == "__main__":
    unittest.main()
