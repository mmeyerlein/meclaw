"""`scripts/pipeline/lane_browser_deps.sh`: the system packages a browser engine needs.

GH #1161, OR-TB.S3.19: build02's WebKit could not load libGL.so.1, and every WebKit proof there
SKIPped green. The recipe provisions a lane from the PINNED Playwright: the package
list is what `install-deps --dry-run <engine>` of that version reports missing, and the
install is that version's own `install-deps <engine>` -- never a list kept by hand.
It runs ON the lane (`lane_sync.sh browsers` sends it on stdin), and everything it
cannot do is a plain-text reason with a non-zero exit, never a silent "ok".

The fakes play the lane: `node` answers `install-deps [--dry-run]` from a state
directory, `sudo` runs its command or refuses like a password prompt would.
"""

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest
from scripts.tests import kit_time, private_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "pipeline" / "lane_browser_deps.sh"
LANE_SYNC = REPO / "scripts" / "lane_sync.sh"


def playwright_pin():
    """The playwright pin of the browser drivers. GH #1199: read inside the
    test, never at import -- `workshop/` does not travel, and a module-level
    read killed a whole gate-selftest shard of the public CI before its first
    test (exit 1 without a unittest summary)."""
    private_tree.require("workshop/tools/package.json")
    return json.loads((REPO / "workshop" / "tools" / "package.json").read_text())[
        "dependencies"]["playwright"]


# The pin the recipe is handed when the test drives `lane_browser_deps.sh` itself:
# the recipe reads the pin from its argument, never from the tree, so these tests
# need a version, not the tree's (GH #1199: they travel with the export). Only
# what goes through `lane_sync.sh`, which reads the tree's pin, takes `playwright_pin()`.
FIXTURE_PIN = "1.50.1"

FAKE_NODE = r"""#!/bin/sh
# plays `node <cli.js> install-deps [--dry-run] <engine...>` of the pinned playwright
state="$FAKE_PW_STATE"
[ -f "$state/eats_stdin" ] && cat >/dev/null
case " $* " in
    *" install --dry-run webkit "*)
        echo "$*" >> "$state/argv_bundle"
        echo "WebKit 26.6 (playwright webkit v2359)"
        echo "  Install location:    $FAKE_BROWSERS/webkit-2359"
        exit 0 ;;
    *" install webkit "*)
        echo "$*" >> "$state/argv_bundle"
        echo "uid=$(id -u) sudo=${FAKE_UNDER_SUDO:-0}" >> "$state/bundle_install"
        b="$FAKE_BROWSERS/webkit-2359/minibrowser-wpe"
        mkdir -p "$b/bin" "$b/lib" "$b/sys/lib"
        printf '#!/bin/sh\n' > "$b/bin/MiniBrowser"; chmod 755 "$b/bin/MiniBrowser"
        echo "NEEDS libEGL.so.1" > "$b/lib/libWPEWebKit-2.0.so.1"
        : > "$b/sys/lib/libEGL.so.1"
        [ -f "$state/reinstall_shadowed" ] && echo "NEEDS libgallium-old.so" > "$b/sys/lib/libEGL_mesa.so.0"
        exit 0 ;;
esac
echo "$*" >> "$state/argv"
case " $* " in *" install-deps "*) ;; *) echo "fake node: unexpected $*" >&2; exit 9 ;; esac
case " $* " in
    *" --dry-run "*)
        if [ -f "$state/installed" ]; then echo "All system dependencies are installed."; exit 0; fi
        cat "$state/dry_run"
        exit 1 ;;
    *)
        echo "uid=$(id -u) sudo=${FAKE_UNDER_SUDO:-0} frontend=${DEBIAN_FRONTEND:-}" >> "$state/install"
        [ -f "$state/install_fails" ] && exit 1
        [ -f "$state/install_no_effect" ] || touch "$state/installed"
        echo "Installing dependencies..."
        exit 0 ;;
esac
"""

FAKE_SUDO = r"""#!/bin/sh
if [ -f "$FAKE_PW_STATE/no_sudo" ]; then echo "sudo: a password is required" >&2; exit 1; fi
echo "sudo $*" >> "$FAKE_PW_STATE/sudo"
while [ "${1#-}" != "$1" ]; do shift; done
FAKE_UNDER_SUDO=1 exec "$@"
"""

FAKE_LDD = r"""#!/bin/sh
# plays `ldd <file>`: a file names what it links as `NEEDS <soname>` lines; a soname
# resolves from LD_LIBRARY_PATH first, then from the system ($FAKE_PW_STATE/syslib)
for lib in $(sed -n 's/^NEEDS //p' "$1"); do
    hit=""
    for d in $(echo "${LD_LIBRARY_PATH:-}" | tr ':' ' ') "$FAKE_PW_STATE/syslib"; do
        if [ -e "$d/$lib" ]; then hit="$d/$lib"; break; fi
    done
    printf '\t%s => %s\n' "$lib" "${hit:-not found}"
done
"""

MISSING = "Missing system dependencies (2):\n  libgl1\n  libgles2\n"


class _Lane(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        base = pathlib.Path(self._tmp.name)
        self.root, self.state, fakes = base / "lane", base / "state", base / "fakes"
        for d in (self.root, self.state, fakes):
            d.mkdir()
        for name, body in (("node", FAKE_NODE), ("sudo", FAKE_SUDO), ("ldd", FAKE_LDD)):
            (fakes / name).write_text(body)
            (fakes / name).chmod(0o755)
        self.fakes = fakes
        self.base = base
        self.browsers = base / "ms-playwright"
        (self.state / "syslib").mkdir()
        self.playwright(FIXTURE_PIN)
        (self.state / "dry_run").write_text(MISSING)
        self.env = dict(os.environ, HOME=str(base), FAKE_PW_STATE=str(self.state),
                        FAKE_BROWSERS=str(self.browsers),
                        PATH=str(fakes) + os.pathsep + store_isolation.trapped_path())
        # the pinned bundle as the CDN ships it: every library it links resolves
        subprocess.run([str(fakes / "node"), "cli.js", "install", "webkit"], env=self.env,
                       check=True, capture_output=True)
        (self.state / "bundle_install").unlink()
        (self.state / "argv_bundle").unlink()

    def tearDown(self):
        self._tmp.cleanup()

    def playwright(self, version, where="cache/workshop-tools"):
        d = self.root / where / "node_modules" / "playwright"
        d.mkdir(parents=True, exist_ok=True)
        (d / "package.json").write_text(json.dumps({"name": "playwright", "version": version},
                                                  indent=2))
        (d / "cli.js").write_text("// the fake node plays it\n")
        return d

    def run_script(self, mode, *engines, stdin=False, pin=None, cwd=None):
        pin = FIXTURE_PIN if pin is None else pin
        args = [str(self.root), pin, mode] + list(engines or ("webkit",))
        if stdin:
            return subprocess.run(["sh", "-s", "--"] + args, input=SCRIPT.read_text(),
                                  capture_output=True, text=True, timeout=2 * kit_time.HANG, env=self.env,
                                  cwd=cwd)
        return subprocess.run(["sh", str(SCRIPT)] + args, capture_output=True, text=True,
                              timeout=2 * kit_time.HANG, env=self.env, cwd=cwd)

    def lines(self, name):
        p = self.state / name
        return p.read_text().splitlines() if p.exists() else []


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestBrowserDeps(_Lane):
    def test_the_list_is_what_the_pinned_playwright_reports_missing(self):
        res = self.run_script("check")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps missing webkit 2: libgl1 libgles2", res.stdout)
        # asked the pinned CLI of the lane, read-only, for exactly this engine
        cli = self.root / "cache/workshop-tools/node_modules/playwright/cli.js"
        self.assertEqual(["%s install-deps --dry-run webkit" % cli], self.lines("argv"))
        self.assertEqual([], self.lines("sudo"))

    def test_nothing_missing_is_ok_without_sudo(self):
        (self.state / "installed").touch()
        for mode in ("check", "install"):
            with self.subTest(mode=mode):
                res = self.run_script(mode)
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertIn("browser-deps ok webkit", res.stdout)
        self.assertEqual([], self.lines("sudo"))
        self.assertEqual([], self.lines("install"))

    def test_install_runs_the_pinned_install_deps_as_root_and_checks_again(self):
        res = self.run_script("install")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps installed webkit 2: libgl1 libgles2", res.stdout)
        self.assertIn("browser-deps ok webkit", res.stdout)
        cli = self.root / "cache/workshop-tools/node_modules/playwright/cli.js"
        # dry run, install (the pinned CLI's own, no package list by hand), dry run again
        self.assertEqual(["%s install-deps --dry-run webkit" % cli,
                          "%s install-deps webkit" % cli,
                          "%s install-deps --dry-run webkit" % cli], self.lines("argv"))
        self.assertEqual(["sudo=1", "frontend=noninteractive"],
                         self.lines("install")[0].split()[1:])
        # idempotent: a second run finds nothing to do and needs no sudo
        before = self.lines("sudo")
        again = self.run_script("install")
        self.assertEqual(0, again.returncode, again.stdout + again.stderr)
        self.assertIn("browser-deps ok webkit", again.stdout)
        self.assertEqual(before, self.lines("sudo"))
        self.assertEqual(1, len(self.lines("install")))

    def test_no_passwordless_sudo_is_a_plain_reason_and_installs_nothing(self):
        (self.state / "no_sudo").touch()
        res = self.run_script("install")
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn("no passwordless sudo", res.stderr)
        self.assertIn("libgl1 libgles2", res.stderr)
        self.assertEqual([], self.lines("install"))

    def test_an_install_that_leaves_packages_missing_fails(self):
        (self.state / "install_no_effect").touch()
        res = self.run_script("install")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("still missing after install-deps", res.stderr)
        (self.state / "install_fails").touch()
        res = self.run_script("install")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("install-deps webkit failed", res.stderr)

    def test_what_it_cannot_determine_is_red_with_a_reason(self):
        # a playwright on the lane that is not the pin of the tree
        self.playwright("0.0.1")
        res = self.run_script("install")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("not the pinned %s" % FIXTURE_PIN, res.stderr)
        self.assertEqual([], self.lines("argv"))
        self.playwright(FIXTURE_PIN)
        # output the recipe does not know: no list, a count that does not match
        for out in ("BEWARE: your OS is not officially supported\n",
                    "Missing system dependencies (3):\n  libgl1\n  libgles2\n"):
            with self.subTest(out=out):
                (self.state / "dry_run").write_text(out)
                res = self.run_script("install")
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                self.assertIn("no package list", res.stderr)
                self.assertEqual([], self.lines("install"))

    def test_no_playwright_and_no_node_are_reasons(self):
        shutil.rmtree(self.root / "cache")
        res = self.run_script("check")
        self.assertEqual(2, res.returncode)
        self.assertIn("no playwright", res.stderr)
        (self.fakes / "node").unlink()
        self.playwright(FIXTURE_PIN)
        res = self.run_script("check")
        if shutil.which("node", path=store_isolation.trapped_path()) is None:
            self.assertEqual(2, res.returncode, res.stdout + res.stderr)
            self.assertIn("no node", res.stderr)

    def test_only_the_three_engines_reach_install_deps(self):
        """Re-check tb-s3 h2 m-3: an engine is chromium, webkit or firefox. Anything
        else -- another browser channel, an option, a glob -- is refused before the
        pinned CLI is asked, so it never reaches `install-deps` as root."""
        (self.root / "webkit").touch()   # what an unquoted `webki*` would expand to
        for bad in (["chrome"], ["msedge"], ["--force"], ["*"], ["webkit", "webki*"]):
            with self.subTest(engines=bad):
                res = self.run_script("install", *bad, cwd=str(self.root))
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                self.assertIn("not an engine", res.stderr)
                self.assertEqual([], self.lines("argv"))
        res = self.run_script("check", "chromium", "webkit", "firefox")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["%s install-deps --dry-run chromium webkit firefox"
                          % (self.root / "cache" / "workshop-tools" / "node_modules"
                             / "playwright" / "cli.js")], self.lines("argv"))

    def test_it_runs_from_stdin_even_when_a_child_reads_stdin(self):
        """On the lane the script itself IS stdin (`sh -s`): a child that reads it
        (apt-get, node) would eat the rest of the recipe."""
        (self.state / "eats_stdin").touch()
        res = self.run_script("install", stdin=True)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps installed webkit 2: libgl1 libgles2", res.stdout)
        self.assertIn("browser-deps ok webkit", res.stdout)

    def test_lane_sync_sends_it_with_the_pin_of_the_tree(self):
        pin = playwright_pin()
        self.playwright(pin)
        res = subprocess.run(["bash", str(LANE_SYNC), "browsers", "local", "check", "webkit"],
                             capture_output=True, text=True, timeout=2 * kit_time.HANG,
                             env=dict(self.env, MECLAW_LANE_ROOT=str(self.root)))
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps missing webkit 2: libgl1 libgles2", res.stdout)
        self.playwright("0.0.1")
        res = subprocess.run(["bash", str(LANE_SYNC), "browsers", "local", "check"],
                             capture_output=True, text=True, timeout=2 * kit_time.HANG,
                             env=dict(self.env, MECLAW_LANE_ROOT=str(self.root)))
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("not the pinned %s" % pin, res.stderr)
        self.assertEqual([], self.lines("install"))

    def test_lane_sync_never_installs_on_the_owners_machine(self):
        """Re-check tb-s3 h2 M-1: `local` is this machine and `class=egon` the owner's
        -- `install` (apt as root) is refused there before the recipe is sent, even
        with a lane root that would let it run through. `check` stays allowed."""
        self.playwright(FIXTURE_PIN)
        for target, cls in (("local", ""), ("builder@egon.example.invalid", "egon")):
            with self.subTest(target=target):
                res = subprocess.run(
                    ["bash", str(LANE_SYNC), "browsers", target, "install", "webkit"],
                    capture_output=True, text=True, timeout=2 * kit_time.HANG,
                    env=dict(self.env, MECLAW_LANE_ROOT=str(self.root),
                             MECLAW_LANE_CLASS=cls))
                self.assertNotEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertIn("never installs", res.stderr)
                self.assertEqual([], self.lines("install"))
                self.assertEqual([], self.lines("argv"), "the recipe was sent")


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestBundleShadowsTheSystem(_Lane):
    """One lane, 10.10.: every WebKit proof red with `newPage: Target page, context or
    browser has been closed`, the web process said `Could not create EGL display: no
    supported platform available`. The apt packages were complete; the bundle was not:
    the laboratory recipe (wkenv.sh) had copied 325 libraries into
    `minibrowser-wpe/sys/lib`, among them a Mesa `libEGL_mesa.so.0` of
    25.2.8-0ubuntu0.24.04.2 that links `libgallium-25.2.8-0ubuntu0.24.04.2.so` -- on
    the host that built the laboratory, not on that lane (24.04.4). Playwright's wrapper
    puts `sys/lib` ahead of the system, so the stale copy shadowed the system Mesa."""

    SHADOW = "libgallium-25.2.8-0ubuntu0.24.04.2.so"

    def setUp(self):
        super().setUp()
        (self.state / "installed").touch()
        self.syslib = self.wpe / "sys" / "lib"
        (self.syslib / "libEGL_mesa.so.0").write_text("NEEDS %s\n" % self.SHADOW)
        (self.state / "syslib" / "libgallium-25.2.8-0ubuntu0.24.04.4.so").touch()

    @property
    def wpe(self):
        return self.browsers / "webkit-2359" / "minibrowser-wpe"

    def test_check_names_what_the_bundle_cannot_resolve(self):
        res = self.run_script("check")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps shadowed webkit 1: %s" % self.SHADOW, res.stdout)
        self.assertNotIn("browser-deps ok", res.stdout)
        self.assertTrue((self.syslib / "libEGL_mesa.so.0").exists())
        self.assertEqual([], self.lines("bundle_install"))

    def test_install_sets_the_shadowing_bundle_aside_and_installs_the_pin(self):
        res = self.run_script("install")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps reinstalled webkit 1: %s" % self.SHADOW, res.stdout)
        self.assertIn("browser-deps ok webkit", res.stdout)
        # the pinned CLI installs the bundle as the user, never as root
        self.assertEqual(["uid=%d sudo=0" % os.getuid()], self.lines("bundle_install"))
        self.assertFalse((self.syslib / "libEGL_mesa.so.0").exists())
        # nothing deleted: the old bundle is set aside outside the browsers path
        aside = list((self.base / ".cache" / "meclaw-shadowed-bundles").glob("webkit-2359-*"))
        self.assertEqual(1, len(aside), aside)
        self.assertTrue((aside[0] / "minibrowser-wpe/sys/lib/libEGL_mesa.so.0").exists())
        # idempotent: the second run finds the bundle whole and touches nothing
        again = self.run_script("install")
        self.assertEqual(0, again.returncode, again.stdout + again.stderr)
        self.assertEqual(1, len(self.lines("bundle_install")))

    def test_a_reinstall_that_stays_shadowed_fails(self):
        (self.state / "reinstall_shadowed").touch()
        res = self.run_script("install")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("still shadowed after a fresh install", res.stderr)

    def test_a_laboratory_whose_copies_resolve_stays_untouched(self):
        """The host the laboratory was built on has the same libgallium: its
        bundle resolves, and the recipe must not take it apart."""
        (self.state / "syslib" / self.SHADOW).touch()
        res = self.run_script("install")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps ok webkit", res.stdout)
        self.assertTrue((self.syslib / "libEGL_mesa.so.0").exists())
        self.assertEqual([], self.lines("bundle_install"))

    def test_lane_sync_reports_a_shadowed_bundle(self):
        self.playwright(playwright_pin())   # lane_sync sends the tree's pin
        res = subprocess.run(["bash", str(LANE_SYNC), "browsers", "local", "check", "webkit"],
                             capture_output=True, text=True, timeout=2 * kit_time.HANG,
                             env=dict(self.env, MECLAW_LANE_ROOT=str(self.root)))
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps shadowed webkit 1: %s" % self.SHADOW, res.stdout)


FAKE_SHELL = r"""#!/bin/sh
# plays the playwright chromium headless shell: its own sandbox needs userns
echo "$*" >> "$FAKE_PW_STATE/shell"
if [ -f "$FAKE_PW_STATE/userns" ]; then exit 0; fi
echo "FATAL:zygote_host_impl_linux.cc No usable sandbox!" >&2
exit 133
"""

FAKE_APPARMOR_PARSER = r"""#!/bin/sh
echo "uid=$(id -u) sudo=${FAKE_UNDER_SUDO:-0} $*" >> "$FAKE_PW_STATE/apparmor"
[ -f "$FAKE_PW_STATE/parser_ignores" ] || touch "$FAKE_PW_STATE/userns"
"""


@unittest.skipUnless(shutil.which("sh"), "needs a POSIX sh")
class TestChromiumSandbox(_Lane):
    """tb-s10 (GH #1189): under `MECLAW_REQUIRE_BROWSERS` the seven gh766 units of
    misc_6 run the playwright headless shell -- and on build03 (Ubuntu 24.04,
    `apparmor_restrict_unprivileged_userns=1`) every one died with "No usable
    sandbox!" (`spawn_failed`). meclaw adds no `--no-sandbox` (R-G11), so the lane
    gives the package a userns profile, as Ubuntu does for Google Chrome."""

    def setUp(self):
        super().setUp()
        (self.state / "installed").touch()
        d = self.browsers / "chromium_headless_shell-1243" / "chrome-headless-shell-linux64"
        d.mkdir(parents=True)
        self.shell = d / "chrome-headless-shell"
        self.shell.write_text(FAKE_SHELL)
        self.shell.chmod(0o755)
        ap = self.fakes / "apparmor_parser"
        ap.write_text(FAKE_APPARMOR_PARSER)
        ap.chmod(0o755)
        self.apparmor = self.base / "apparmor.d"
        self.apparmor.mkdir()
        self.env.update(PLAYWRIGHT_BROWSERS_PATH=str(self.browsers),
                        MECLAW_APPARMOR_DIR=str(self.apparmor))

    def test_check_names_a_shell_without_a_sandbox(self):
        res = self.run_script("check", "chromium")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps sandbox chromium: %s" % self.shell, res.stdout)
        self.assertEqual(["--headless --dump-dom about:blank"], self.lines("shell"))
        self.assertEqual([], self.lines("sudo"))

    def test_install_loads_a_userns_profile_as_root_and_starts_the_shell_again(self):
        res = self.run_script("install", "chromium")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        prof = self.apparmor / "meclaw-playwright-chromium"
        self.assertIn("browser-deps sandboxed chromium: AppArmor profile %s" % prof, res.stdout)
        text = prof.read_text()
        self.assertIn("profile meclaw-playwright-chromium %s/chromium_headless_shell-*/"
                      "chrome-headless-shell-linux64/chrome-headless-shell flags=(unconfined) {"
                      % self.browsers, text)
        self.assertIn("  userns,", text)
        self.assertEqual(["uid=%d sudo=1 -r %s" % (os.getuid(), prof)], self.lines("apparmor"))
        self.assertEqual(2, len(self.lines("shell")))
        # and nothing that takes the sandbox away
        self.assertNotIn("no-sandbox", text + " ".join(self.lines("shell")))

    def test_a_profile_that_does_not_help_fails(self):
        (self.state / "parser_ignores").touch()
        res = self.run_script("install", "chromium")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("still has no usable sandbox", res.stderr)

    def test_a_working_sandbox_and_webkit_alone_touch_nothing(self):
        (self.state / "userns").touch()
        res = self.run_script("install", "chromium")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual([], self.lines("apparmor"))
        (self.state / "userns").unlink()
        res = self.run_script("check")   # webkit: the shell is not asked
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(1, len(self.lines("shell")))

    def test_check_without_an_engine_asks_the_chromium_sandbox_too(self):
        """tb-s10 deckel 17: `strand.sh lanes browsers <lane> check` without an engine
        asked webkit alone -- a lane without the userns profile read "ok" and fell only
        in `browser:display` with "No usable sandbox!"."""
        res = subprocess.run(["sh", str(SCRIPT), str(self.root), FIXTURE_PIN, "check"],
                             capture_output=True, text=True, timeout=2 * kit_time.HANG, env=self.env)
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps sandbox chromium: %s" % self.shell, res.stdout)
        self.assertIn("scripts/strand.sh lanes browsers <lane> install chromium", res.stdout)
        self.assertEqual(["--headless --dump-dom about:blank"], self.lines("shell"))
        self.assertEqual([], self.lines("sudo"))
        # with the profile loaded the same bare check is ok, for both engines
        (self.state / "userns").touch()
        res = subprocess.run(["sh", str(SCRIPT), str(self.root), FIXTURE_PIN, "check"],
                             capture_output=True, text=True, timeout=2 * kit_time.HANG, env=self.env)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("browser-deps ok chromium webkit", res.stdout)

    def test_no_passwordless_sudo_is_exit_3(self):
        (self.state / "no_sudo").touch()
        res = self.run_script("install", "chromium")
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertEqual([], self.lines("apparmor"))


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
