"""Pins of the testing bans (`scripts/testing_bans.py`, VORGABEN § 5.4, ADR-0050).

Every pin builds a throw-away repository: a `master` commit with the real ban
data (`.config/testing-bans.toml` -- the data is what is under test), then a
strand branch whose commit adds the forbidden line, and expects the finding
with its ban id. The real tree is read for the ban data only.

The fixtures are Python strings: the check blanks string contents before it
matches, so this file does not trip the bans it pins.
"""

import datetime
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import store_isolation

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import testing_bans as tb  # noqa: E402

BANS = (REPO / ".config" / "testing-bans.toml").read_text(encoding="utf-8")
TODAY = datetime.date(2026, 10, 11)

RS = "crates/meclaw-x/tests/a.rs"
PY = "scripts/tests/test_x.py"
JS = "workshop/tools/x/drive.mjs"

REGISTER = (
    '[[ruling]]\nid = "OR-TV-2"\ndate = "2026-10-11"\nby = "coordinator"\n'
    'text = "the owner opens two bans for the legacy crate"\n'
    'bans = ["rs-thread-sleep", "py-time"]\npaths = ["crates/**", "scripts/tests/**"]\n'
    'loosens = [".config/flaky-legacy.toml", ".config/testing-bans.toml",'
    ' "scripts/testing_bans.py"]\n'
    'expires = "2026-12-31"\n'
    '\n[[ruling]]\nid = "OR-welle-testing.tv-p0.1"\ndate = "2026-10-11"\nby = "coordinator"\n'
    'text = "a strand ruling that somebody entered"\nbans = ["rs-thread-sleep"]\n'
    'paths = ["crates/**"]\nloosens = []\n'
    '\n[[ruling]]\nid = "OR-TV-OLD"\ndate = "2026-01-01"\nby = "owner"\n'
    'text = "an expired opening"\nbans = ["rs-thread-sleep"]\npaths = ["crates/**"]\n'
    'loosens = []\nexpires = "2026-02-01"\n')

GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}


def fn_rs(body, attr="#[tokio::test]", head=""):
    return "%s%s\nasync fn t() {\n%s\n}\n" % (head, attr, body)


class Repo:
    """A master commit with the ban data, then a strand branch."""

    def __init__(self, case, base=None, register=REGISTER, branch="strand"):
        tmp = tempfile.TemporaryDirectory()
        case.addCleanup(tmp.cleanup)
        self.root = os.path.realpath(tmp.name)
        self.git("init", "-q", "-b", "master")
        files = {".config/testing-bans.toml": BANS, "README.md": "x\n"}
        if register is not None:
            files[".config/testing-rulings.toml"] = register
        files.update(base or {})
        for rel, text in files.items():
            self.write(rel, text)
        self.commit("base")
        if branch:
            self.git("checkout", "-q", "-b", branch)

    def git(self, *args):
        res = subprocess.run(["git", "-C", self.root] + list(args), capture_output=True,
                             text=True)
        if res.returncode != 0:
            raise AssertionError("git %s: %s" % (args, res.stderr))
        return res.stdout

    def write(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)

    def commit(self, msg="change"):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)

    def bans(self, files, **kw):
        kw.setdefault("today", TODAY)
        return tb.scan(files, self.root, **kw)

    def rules(self, files, **kw):
        kw.setdefault("today", TODAY)
        return tb.rules(files, self.root, **kw)


def ids(findings, level=None):
    return sorted({f.message.split(" ", 2)[1] for f in findings
                   if f.check == "test-bans" and (level is None or f.level == level)
                   and f.message[:1] != "("})


class Case(unittest.TestCase):

    def setUp(self):
        patcher = mock.patch.dict(os.environ, GIT_ENV)
        patcher.start()
        self.addCleanup(patcher.stop)

    def added(self, rel, text, base=None, **kw):
        r = Repo(self, base=base)
        r.write(rel, text)
        r.commit()
        return r.bans([rel], **kw)

    def assert_red(self, ban, rel, text, base=None):
        found = self.added(rel, text, base=base)
        self.assertIn(ban, ids(found, "RED"), "\n".join(map(str, found)))
        line = [f for f in found if f.message.split(" ", 2)[1] == ban][0].message
        self.assertIn(" -- ", line)          # the replacement travels with the finding
        return found

    def assert_green(self, rel, text, ban):
        found = self.added(rel, text)
        self.assertNotIn(ban, ids(found), "\n".join(map(str, found)))


class EveryBanIsRedInAnAddedLine(Case):

    def test_rs_thread_sleep_in_an_added_line_is_red(self):
        self.assert_red("rs-thread-sleep", RS, fn_rs(
            "    std::thread::sleep(std::time::Duration::from_millis(5));", "#[test]"))

    def test_rs_tokio_sleep_in_an_added_line_is_red(self):
        self.assert_red("rs-tokio-sleep", RS, fn_rs(
            "    tokio::time::sleep(Duration::from_millis(5)).await;"))

    def test_rs_tokio_sleep_bare_with_its_import_is_red(self):
        self.assert_red("rs-tokio-sleep", RS, fn_rs(
            "    sleep(Duration::from_millis(5)).await;", head="use tokio::time::{sleep, Duration};\n"))

    def test_rs_tokio_sleep_under_start_paused_is_green(self):
        self.assert_green(RS, fn_rs("    tokio::time::sleep(Duration::from_secs(5)).await;",
                                    "#[tokio::test(start_paused = true)]"), "rs-tokio-sleep")

    def test_rs_tokio_sleep_in_the_wait_helper_is_green(self):
        self.assert_green("crates/meclaw-testing/src/wait.rs",
                          "pub async fn pause() {\n    tokio::time::sleep(D).await;\n}\n",
                          "rs-tokio-sleep")

    def test_rs_elapsed_verdict_in_an_added_line_is_red(self):
        self.assert_red("rs-elapsed-verdict", RS, fn_rs(
            "    let t0 = std::time::Instant::now();\n    run().await;\n"
            "    assert!(t0.elapsed() < Duration::from_secs(2));"))

    def test_rs_elapsed_verdict_through_a_variable_is_red(self):
        self.assert_red("rs-elapsed-verdict", RS, fn_rs(
            "    let start = Instant::now();\n    run().await;\n    let took = start.elapsed();\n"
            "    assert!(took < LIMIT, \"slow\");"))

    def test_rs_elapsed_verdict_on_tokio_instant_under_start_paused_is_green(self):
        self.assert_green(RS, fn_rs(
            "    let t0 = Instant::now();\n    run().await;\n"
            "    assert_eq!(t0.elapsed(), Duration::from_secs(30));",
            "#[tokio::test(start_paused = true)]", head="use tokio::time::Instant;\n"),
            "rs-elapsed-verdict")

    def test_rs_short_deadline_in_an_added_line_is_red(self):
        self.assert_red("rs-short-deadline", RS, fn_rs(
            "    let m = timeout(Duration::from_millis(200), rx.recv()).await.unwrap();"))

    def test_rs_short_deadline_from_deadline_is_green(self):
        self.assert_green(RS, fn_rs(
            "    let m = timeout(deadline::HANG, rx.recv()).await.unwrap();\n"
            "    let n = timeout(Duration::from_secs(30), rx.recv()).await.unwrap();"),
            "rs-short-deadline")

    def test_rs_neg_window_in_an_added_line_is_red(self):
        self.assert_red("rs-neg-window", RS, fn_rs(
            "    let r = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;\n"
            "    assert!(r.is_err());"))

    def test_rs_neg_window_inline_is_red(self):
        self.assert_red("rs-neg-window", RS, fn_rs(
            "    assert!(matches!(timeout(D, rx.recv()).await, Err(_)));"))

    def test_rs_neg_window_under_start_paused_is_green(self):
        self.assert_green(RS, fn_rs(
            "    let r = timeout(Duration::from_secs(30), rx.recv()).await;\n"
            "    assert!(r.is_err());", "#[tokio::test(start_paused = true)]"),
            "rs-neg-window")

    def test_rs_colony_db_in_an_added_line_is_red(self):
        self.assert_red("rs-colony-db", RS, fn_rs(
            "    let db = rusqlite::Connection::open(root.join(\"colony.db\")).unwrap();"))

    def test_rs_colony_db_in_the_log_recorder_is_green(self):
        self.assert_green("crates/meclaw-testing/src/log_recorder.rs",
                          "pub fn path() -> &'static str {\n    \"colony.db\"\n}\n",
                          "rs-colony-db")

    def test_rs_should_panic_in_an_added_line_is_red(self):
        self.assert_red("rs-should-panic", RS,
                        "#[test]\n#[should_panic]\nfn t() {\n    boom();\n}\n")

    def test_rs_should_panic_with_expected_is_green(self):
        self.assert_green(RS, "#[test]\n#[should_panic(expected = \"boom\")]\nfn t() {\n"
                              "    boom();\n}\n", "rs-should-panic")

    def test_rs_alive_tasks_in_an_added_line_is_red(self):
        # The data grades this ban NOTE (VG § 5.4 table); the pin holds that an
        # added line is named with its id at the level the data gives it.
        found = self.added(RS, fn_rs("    let n = rt.metrics().num_alive_tasks();"))
        level = [b for b in tb.load_bans(REPO) if b["id"] == "rs-alive-tasks"][0]["level"]
        self.assertIn("rs-alive-tasks", ids(found, level), found)

    def test_py_time_in_an_added_line_is_red(self):
        self.assert_red("py-time", PY, "import time\n\n\ndef test():\n    time.sleep(0.1)\n")

    def test_py_time_in_the_kit_clock_is_green(self):
        self.assert_green("scripts/tests/kit_time.py",
                          "import time\n\n\ndef now():\n    return time.monotonic()\n", "py-time")

    def test_py_wall_verdict_in_an_added_line_is_red(self):
        self.assert_red("py-wall-verdict", PY,
                        "class T:\n    def test(self):\n        took = run()\n"
                        "        self.assertLess(took, 2.0)\n")

    def test_py_raises_in_an_added_line_is_red(self):
        self.assert_red("py-raises", PY, "class T:\n    def test(self):\n"
                                         "        with self.assertRaises(ValueError):\n"
                                         "            run()\n")

    def test_py_raises_with_a_regex_is_green(self):
        self.assert_green(PY, "class T:\n    def test(self):\n"
                              "        with self.assertRaisesRegex(ValueError, 'bad'):\n"
                              "            run()\n", "py-raises")

    def test_js_wait_for_timeout_in_an_added_line_is_red(self):
        self.assert_red("js-wait-for-timeout", JS, "await page.waitForTimeout(500);\n")

    def test_js_sleep_in_an_added_line_is_red(self):
        self.assert_red("js-sleep", JS, "await sleep(200);\n")

    def test_js_sleep_of_zero_and_in_display_lab_is_green(self):
        self.assert_green(JS, "await new Promise((r) => setTimeout(r, 0));\n", "js-sleep")
        self.assert_green("workshop/tools/display-lab/run.mjs", "await sleep(200);\n", "js-sleep")

    def test_js_unawaited_in_an_added_line_is_red(self):
        self.assert_red("js-unawaited", JS, "page.click('#go');\nawait next();\n")

    def test_js_awaited_call_is_green(self):
        self.assert_green(JS, "await page.click('#go');\nconst p = page.goto(u);\n", "js-unawaited")

    def test_js_duration_verdict_in_an_added_line_is_red(self):
        self.assert_red("js-duration-verdict", JS,
                        "if (performance.now() - t0 > 500) {\n  throw new Error('slow');\n}\n")

    def test_js_racy_api_in_an_added_line_is_red(self):
        self.assert_red("js-racy-api", JS, "await page.waitForNavigation();\n")

    def test_js_clock_pause_at_in_an_added_line_is_red(self):
        # GH #1209 (M11): an absolute target read from the running page clock is behind it
        # by the time the pause lands on a loaded host -- "Cannot fast-forward to the past".
        self.assert_red("js-clock-pause-at", JS,
                        "await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1));\n")

    def test_js_clock_pause_at_in_every_spelling_is_red(self):
        # Re-check M11 (M1): the ban bound to the text `clock.pauseAt(`; each spelling below
        # reaches the same absolute-target call and passed it.
        for text in ("await page.clock\n  .pauseAt(t);\n",
                     "await page.clock?.pauseAt(t);\n",
                     "const pause = page.clock.pauseAt;\nawait pause.call(page.clock, t);\n",
                     "const { pauseAt } = page.clock;\nawait pauseAt(t);\n"):
            with self.subTest(text=text):
                self.assert_red("js-clock-pause-at", JS, text)

    def test_js_clock_run_for_is_green(self):
        self.assert_green(JS, "await page.clock.runFor(60_000);\n", "js-clock-pause-at")

    def test_js_screenshot_in_an_added_line_is_red(self):
        self.assert_red("js-screenshot", JS, "await page.screenshot({ path: 'a.png' });\n")

    def test_js_screenshot_with_its_three_options_is_green(self):
        self.assert_green(JS, "await page.screenshot({ path: 'a.png', scale: 'css',\n"
                              "  animations: 'disabled', timeout: HANG });\n", "js-screenshot")

    # V2-A6 (GH #1199): the windows and sleeps the first eighteen bans missed.

    def test_rs_neg_window_ok_in_an_added_line_is_red(self):
        self.assert_red("rs-neg-window-ok", RS, fn_rs(
            "    assert!(timeout(QUIET, rx.recv()).await.ok().flatten().is_none());"))

    def test_rs_neg_window_ok_if_let_with_panic_is_red(self):
        self.assert_red("rs-neg-window-ok", RS, fn_rs(
            "    if let Ok(Some(m)) = tokio::time::timeout(QUIET, rx.recv()).await {\n"
            "        panic!(\"arrived: {m:?}\");\n    }"))

    def test_rs_neg_window_ok_receive_loop_and_paused_are_green(self):
        self.assert_green(RS, fn_rs(
            "    if let Ok(Some(m)) = timeout(left, rx.recv()).await {\n        seen.push(m);\n    }"),
            "rs-neg-window-ok")
        self.assert_green(RS, fn_rs(
            "    assert!(timeout(QUIET, rx.recv()).await.ok().flatten().is_none());",
            "#[tokio::test(start_paused = true)]"), "rs-neg-window-ok")

    def test_rs_std_window_in_an_added_line_is_red(self):
        self.assert_red("rs-std-window", RS, fn_rs(
            "    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());", "#[test]"))

    def test_rs_std_window_park_timeout_is_red_and_a_hang_marker_green(self):
        self.assert_red("rs-std-window", RS, fn_rs(
            "    std::thread::park_timeout(Duration::from_secs(2));", "#[test]"))
        self.assert_green(RS, fn_rs(
            "    let m = rx.recv_timeout(Duration::from_secs(30)).unwrap();", "#[test]"),
            "rs-std-window")

    def test_rs_std_window_a_hang_marker_in_millis_is_green(self):
        # Re-check v2-fix-b M6: every `from_millis(<n>)` was a window, 30 s too.
        self.assert_green(RS, fn_rs(
            "    let m = rx.recv_timeout(Duration::from_millis(30_000)).expect(\"HANG\");\n"
            "    let n = rx.recv_timeout(Duration::from_millis(10000)).unwrap();\n"
            "    let o = rx.recv_timeout(Duration::from_millis(30 * 1000)).unwrap();", "#[test]"),
            "rs-std-window")
        for line in ("assert!(rx.recv_timeout(Duration::from_millis(9_999)).is_err());",
                     "let m = rx.recv_timeout(Duration::from_millis(5 * 1000)).unwrap();",
                     "let m = rx.recv_timeout(Duration::from_millis(300u64)).unwrap();"):
            with self.subTest(line=line):
                self.assert_red("rs-std-window", RS, fn_rs("    " + line, "#[test]"))

    def test_rs_sleep_until_in_an_added_line_is_red(self):
        self.assert_red("rs-sleep-until", RS, fn_rs(
            "    tokio::time::sleep_until(t0 + Duration::from_millis(300)).await;"))

    def test_rs_sleep_until_under_start_paused_is_green(self):
        self.assert_green(RS, fn_rs(
            "    tokio::time::sleep_until(t0 + Duration::from_millis(300)).await;",
            "#[tokio::test(start_paused = true)]"), "rs-sleep-until")

    def test_py_short_wait_in_an_added_line_is_red(self):
        self.assert_red("py-short-wait", PY, "def test_x(self):\n"
                                             "    self.assertFalse(done.wait(0.3))\n")

    def test_py_short_wait_sub_second_timeout_is_red_and_a_hang_marker_green(self):
        self.assert_red("py-short-wait", PY, "def test_x(self):\n"
                                             "    item = q.get(timeout=0.3)\n")
        self.assert_green(PY, "def test_x(self):\n    item = q.get(timeout=HANG_S)\n"
                              "    proc.wait(timeout=30)\n", "py-short-wait")

    def test_py_short_wait_a_key_or_a_buffer_size_is_green(self):
        # Re-check v2-fix-b M6: `row.get(0)` read as a 0 s wait. A positional
        # integer of get/recv/acquire is a key, a buffer size or `blocking`.
        self.assert_green(PY, "def test_x(self):\n    first = row.get(0)\n"
                              "    head = sock.recv(4)\n    lock.acquire(1)\n",
                          "py-short-wait")
        for line in ("t.join(2)", "fut.result(5)", "item = q.get(timeout=3)",
                     "conn.recv(timeout=0.5)", "ev.wait(0.3)"):
            with self.subTest(line=line):
                self.assert_red("py-short-wait", PY, "def test_x(self):\n    %s\n" % line)

    def test_js_short_timeout_in_an_added_line_is_red(self):
        self.assert_red("js-short-timeout", JS,
                        "await expect(page.locator('#x')).toBeHidden({ timeout: 300 });\n")

    def test_js_short_timeout_named_deadline_is_green(self):
        self.assert_green(JS, "await expect(page.locator('#x')).toBeHidden({ timeout: HANG });\n"
                              "await page.waitForURL(u, { timeout: 30000 });\n",
                          "js-short-timeout")

    def test_js_short_timeout_seconds_as_a_product_are_red(self):
        # Re-check v2-fix-b M6: `{ timeout: 5 * 1000 }` passed -- the product
        # exemption held for every product, 5 s too.
        for line in ("await until(() => ready(), { timeout: 5 * 1000 });",
                     "await until(() => ready(), { timeout: 1000 * 5 });",
                     "await page.waitForSelector('#x', { timeout: 5_000 });"):
            with self.subTest(line=line):
                self.assert_red("js-short-timeout", JS, line + "\n")
        self.assert_green(JS, "await until(() => ready(), { timeout: 30 * 1000 });\n"
                              "await until(() => ready(), { timeout: 2 * 60 * 1000 });\n",
                          "js-short-timeout")

    def test_every_ban_of_the_data_has_its_pin(self):
        names = {n for n in dir(EveryBanIsRedInAnAddedLine) if n.startswith("test_")}
        for ban in tb.load_bans(REPO):
            self.assertIn("test_%s_in_an_added_line_is_red" % ban["id"].replace("-", "_"), names)
        self.assertEqual(23, len(tb.load_bans(REPO)))


class OnlyAddedLinesCount(Case):

    SLEEP = fn_rs("    std::thread::sleep(D);", "#[test]")

    def test_a_moved_file_is_not_an_added_line(self):
        r = Repo(self, base={RS: self.SLEEP})
        r.git("mv", RS, "crates/meclaw-x/tests/b.rs")
        r.commit()
        self.assertEqual([], ids(r.bans(["crates/meclaw-x/tests/b.rs", RS])))

    def test_an_unchanged_line_of_a_changed_file_is_not_graded(self):
        r = Repo(self, base={RS: self.SLEEP})
        r.write(RS, self.SLEEP + "\nfn other() {}\n")
        r.commit()
        self.assertEqual([], ids(r.bans([RS])))

    def test_the_unchanged_line_below_an_added_one_is_not_graded(self):
        # OR-TV-7: a statement counts from its first non-blank character, not
        # from the `;` that ends the line above it
        before = fn_rs("    let a = 1;\n    std::thread::sleep(D);", "#[test]")
        r = Repo(self, base={RS: before})
        r.write(RS, before.replace("let a = 1;", "let a = 2;"))
        r.commit()
        self.assertEqual([], ids(r.bans([RS])))

    def test_a_comment_line_is_not_matched(self):
        self.assertEqual([], ids(self.added(RS, fn_rs("    // std::thread::sleep(D);\n"
                                                      "    /* tokio::time::sleep(D) */"))))

    def test_an_untracked_file_counts_as_added(self):
        r = Repo(self)
        r.write(RS, self.SLEEP)
        self.assertIn("rs-thread-sleep", ids(r.bans([RS]), "RED"))

    def test_a_multiline_statement_counts_once_a_line_is_added(self):
        before = fn_rs("    assert!(\n        t0.elapsed() < LIMIT,\n        \"slow\"\n    );")
        r = Repo(self, base={RS: before})
        r.write(RS, before.replace("\"slow\"", "\"too slow\""))
        r.commit()
        self.assertIn("rs-elapsed-verdict", ids(r.bans([RS]), "RED"))

    def test_without_a_base_the_check_is_a_note(self):
        r = Repo(self, branch=None)
        r.git("branch", "-m", "master", "published")
        r.write(RS, self.SLEEP)
        found = r.bans([RS])
        self.assertEqual([("NOTE", "test-bans")], [(f.level, f.check) for f in found])
        self.assertIn("no base", found[0].message)

    def test_production_code_outside_cfg_test_is_out_of_scope(self):
        src = ("pub fn run() {\n    std::thread::sleep(D);\n}\n\n#[cfg(test)]\nmod tests {\n"
               "    #[test]\n    fn t() {\n        std::thread::sleep(D);\n    }\n}\n")
        found = self.added("crates/meclaw-x/src/lib.rs", src)
        lines = [f.message.split(" ", 1)[0] for f in found if f.level == "RED"]
        self.assertEqual(["crates/meclaw-x/src/lib.rs:9"], lines)

    def test_js_bans_are_silent_in_the_published_tree(self):
        r = Repo(self)
        # The published tree carries no `workshop/`: the diff may name a path there,
        # the check has nothing to read and says nothing.
        self.assertEqual([], r.bans([JS]))

    def test_test_bans_reads_only_the_diff(self):
        r = Repo(self)
        many = ["crates/meclaw-x/tests/f%d.rs" % i for i in range(12)]
        for rel in many:
            r.write(rel, "#[test]\nfn t() {}\n")
        r.commit()
        calls = []
        real = tb._git

        def counting(tree, *args):
            calls.append(args[0])
            return real(tree, *args)

        with mock.patch.object(tb, "_git", side_effect=counting):
            r.bans(many)
        # merge-base, one diff, one listing of the untracked -- whatever the file count.
        self.assertLessEqual(len(calls), 4, calls)

    def test_ruff_missing_is_a_note_not_a_skip(self):
        with mock.patch.object(tb.shutil, "which", return_value=None):
            found = self.added(PY, "def test():\n    pass\n")
        self.assertIn(("NOTE", "test-bans", "(python) ruff missing, regex fallback"),
                      [tuple(f) for f in found])

    def test_an_installed_ruff_grades_its_bans_on_the_added_lines(self):
        r = Repo(self, base={"ruff.toml": (REPO / "ruff.toml").read_text(encoding="utf-8"),
                             PY: "import time\n\n\ndef old():\n    time.sleep(1)\n"})
        r.write(PY, "import time\n\n\ndef old():\n    time.sleep(1)\n\n\n"
                    "def new():\n    time.sleep(2)\n")
        r.commit()
        fake = os.path.join(r.root, "bin", "ruff")
        rows = [{"code": "TID251", "message": "`time.sleep` is banned: py-time",
                 "filename": os.path.join(r.root, PY), "location": {"row": n}} for n in (5, 9)]
        r.write("bin/ruff", "#!%s\nimport json\nprint(json.dumps(%r))\n" % (sys.executable, rows))
        os.chmod(fake, 0o755)
        with mock.patch.object(tb.shutil, "which", return_value=fake):
            found = r.bans([PY])
        self.assertEqual(["%s:9" % PY], [f.message.split(" ", 1)[0] for f in found
                                         if f.level == "RED"])
        self.assertNotIn("(python) ruff missing, regex fallback", [f.message for f in found])


class Exceptions(Case):

    def sleep_with(self, comment):
        return fn_rs("    %s\n    std::thread::sleep(D);" % comment, "#[test]")

    def test_an_exception_without_a_ruling_id_is_red(self):
        found = self.added(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep: the device needs a pause"))
        self.assertIn("rs-thread-sleep", ids(found, "RED"))

    def test_a_ruling_id_missing_from_the_register_is_red(self):
        found = self.added(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep OR-TV-9: the device needs a pause"))
        self.assertTrue(any("OR-TV-9" in f.message and "register" in f.message
                            for f in found if f.level == "RED"), found)

    def test_a_ruling_out_of_its_scope_is_red(self):
        found = self.added(RS, fn_rs(
            "    // testing-ban: allow rs-tokio-sleep OR-TV-2: the device needs a pause\n"
            "    tokio::time::sleep(D).await;"))
        self.assertIn("rs-tokio-sleep", ids(found, "RED"))
        found = self.added("workshop/evals/x/run_a.py",
                           "import time\n# testing-ban: allow py-time OR-TV-2: "
                           "a pause the device needs\ntime.sleep(1)\n")
        self.assertTrue(any("scope" in f.message for f in found if f.level == "RED"), found)

    def test_an_expired_ruling_is_red(self):
        found = self.added(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep OR-TV-OLD: the device needs a pause"))
        self.assertTrue(any("expired" in f.message for f in found if f.level == "RED"), found)

    def test_a_strand_ruling_does_not_open_a_ban(self):
        found = self.added(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep OR-welle-testing.tv-p0.1: entered by someone"))
        self.assertIn("rs-thread-sleep", ids(found, "RED"))
        self.assertTrue(any("strand ruling" in f.message for f in found), found)

    def test_a_short_reason_is_red(self):
        found = self.added(RS, self.sleep_with("// testing-ban: allow rs-thread-sleep OR-TV-2: why"))
        self.assertIn("rs-thread-sleep", ids(found, "RED"))

    def test_an_unused_exception_is_red(self):
        found = self.added(RS, fn_rs(
            "    // testing-ban: allow rs-thread-sleep OR-TV-2: the device needs a pause\n"
            "    run();", "#[test]"))
        self.assertTrue(any("unused" in f.message for f in found if f.level == "RED"), found)

    def test_a_used_exception_is_a_note_with_its_ruling(self):
        found = self.added(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep OR-TV-2: the device needs a pause"))
        self.assertEqual([], [f for f in found if f.level == "RED"], found)
        self.assertIn(("NOTE", "test-ban-exception", "%s:4 rs-thread-sleep OR-TV-2" % RS),
                      [tuple(f) for f in found])

    def test_without_a_register_no_exception_holds(self):
        r = Repo(self, register=None)
        r.write(RS, self.sleep_with(
            "// testing-ban: allow rs-thread-sleep OR-TV-2: the device needs a pause"))
        r.commit()
        self.assertIn("rs-thread-sleep", ids(r.bans([RS]), "RED"))


class TestingRules(Case):

    LEGACY = ("# schema\n[[flaky]]\ntest = \"meclaw-x::a::t\"\nissue = 1\n"
              "cause = \"async-wait\"\n")

    def test_a_strand_diff_on_the_register_is_red(self):
        r = Repo(self)
        r.write(".config/testing-rulings.toml", REGISTER + "\n# more\n")
        r.commit()
        found = r.rules([".config/testing-rulings.toml"], mode="strand")
        self.assertIn("RED", [f.level for f in found if f.check == "testing-rules"], found)

    def test_an_offered_but_unchanged_register_is_no_finding(self):
        # the whole tree offered (precheck's real-tree pin) names the register
        # without the strand touching it: only a change against the base counts
        r = Repo(self)
        r.write("README.md", "y\n")
        r.commit()
        self.assertEqual([], r.rules([".config/testing-rulings.toml", "README.md"],
                                     mode="strand"))

    def test_a_register_commit_with_other_files_is_red_in_integration(self):
        r = Repo(self)
        r.write(".config/testing-rulings.toml", REGISTER + "\n# more\n")
        r.write("README.md", "y\n")
        r.commit("testing-ruling: OR-TV-3 (coordinator, 2026-10-11)")
        files = [".config/testing-rulings.toml", "README.md"]
        self.assertIn("RED", [f.level for f in r.rules(files, mode="integration")])
        r2 = Repo(self)
        r2.write(".config/testing-rulings.toml", REGISTER + "\n# more\n")
        r2.commit("testing-ruling: OR-TV-3 (coordinator, 2026-10-11)")
        self.assertEqual([], r2.rules([".config/testing-rulings.toml"], mode="integration"))

    def test_loosening_the_ban_data_without_a_ruling_is_red(self):
        r = Repo(self)
        loose = BANS.replace('id = "rs-thread-sleep"', 'id = "rs-thread-sleep-gone"', 1)
        r.write(".config/testing-bans.toml", loose)
        r.commit()
        files = [".config/testing-bans.toml"]
        red = [f for f in r.rules(files, env={}) if f.level == "RED"]
        self.assertTrue(any("rs-thread-sleep" in f.message for f in red), red)
        ok = r.rules(files, env={tb.RULING_ENV: "OR-TV-2"})
        self.assertEqual([], [f for f in ok if f.level == "RED"], ok)
        self.assertTrue(any("OR-TV-2" in f.message for f in ok), ok)
        bad = r.rules(files, env={tb.RULING_ENV: "OR-welle-testing.tv-p0.1"})
        self.assertIn("RED", [f.level for f in bad])

    def test_two_rulings_carry_two_reserved_files(self):
        # R-TV-W21 (d): one diff, two reserved files, two rulings -- the env
        # takes several ids (comma or blank), each checked against the register.
        reg = REGISTER + ('\n[[ruling]]\nid = "OR-TV-7"\ndate = "2026-10-12"\nby = "coordinator"\n'
                          'text = "the ruff data"\nbans = []\npaths = []\n'
                          'loosens = ["ruff.toml"]\nexpires = "2026-12-31"\n')
        ruff = (REPO / "ruff.toml").read_text(encoding="utf-8")
        r = Repo(self, base={"ruff.toml": ruff}, register=reg)
        r.write(".config/testing-bans.toml",
                BANS.replace('id = "rs-thread-sleep"', 'id = "rs-thread-sleep-gone"', 1))
        r.write("ruff.toml", ruff.replace('"time.sleep".msg = "py-time"\n', ""))
        r.commit()
        files = [".config/testing-bans.toml", "ruff.toml"]
        for env in ("OR-TV-2,OR-TV-7", "OR-TV-2, OR-TV-7", "OR-TV-7 OR-TV-2"):
            got = r.rules(files, env={tb.RULING_ENV: env})
            self.assertEqual([], [f for f in got if f.level == "RED"], (env, got))
            notes = " ".join(f.message for f in got)
            self.assertIn("ruff.toml loosened", notes)
            self.assertIn("under OR-TV-7", notes)
            self.assertIn("under OR-TV-2", notes)
        one = r.rules(files, env={tb.RULING_ENV: "OR-TV-2"})
        self.assertTrue(any(f.level == "RED" and "ruff.toml" in f.message for f in one), one)

    def test_a_file_none_of_two_valid_rulings_names_is_red(self):
        # K7 (OR-TV-8): a valid id never opens every file -- each loosened file
        # needs one of the given ids naming it under `loosens`.
        reg = REGISTER + ('\n[[ruling]]\nid = "OR-TV-7"\ndate = "2026-10-12"\nby = "coordinator"\n'
                          'text = "the ruff data"\nbans = []\npaths = []\n'
                          'loosens = ["ruff.toml"]\nexpires = "2026-12-31"\n')
        r = Repo(self, base={BASELINE: baseline(**{"rs-thread-sleep": 3})}, register=reg)
        r.write(".config/testing-bans.toml",
                BANS.replace('id = "rs-thread-sleep"', 'id = "rs-thread-sleep-gone"', 1))
        r.write(BASELINE, baseline(**{"rs-thread-sleep": 4}))
        r.commit()
        got = r.rules([".config/testing-bans.toml", BASELINE],
                      env={tb.RULING_ENV: "OR-TV-2,OR-TV-7"})
        red = [f.message for f in got if f.level == "RED"]
        self.assertEqual(1, len(red), got)
        self.assertIn(BASELINE, red[0])
        self.assertIn("none of OR-TV-2, OR-TV-7 names %s" % BASELINE, red[0])
        self.assertTrue(any(f.level == "NOTE" and "under OR-TV-2" in f.message for f in got), got)

    def test_an_unknown_or_expired_id_among_several_is_red(self):
        r = Repo(self)
        r.write(".config/testing-bans.toml",
                BANS.replace('id = "rs-thread-sleep"', 'id = "rs-thread-sleep-gone"', 1))
        r.commit()
        files = [".config/testing-bans.toml"]
        for extra, why in (("OR-NOPE", "not in the register"), ("OR-TV-OLD", "expired"),
                           ("OR-welle-testing.tv-p0.1", "strand ruling")):
            got = r.rules(files, env={tb.RULING_ENV: "OR-TV-2," + extra})
            red = [f for f in got if f.level == "RED"]
            self.assertTrue(any(why in f.message for f in red), (extra, got))

    def test_a_widened_context_or_lowered_level_is_a_loosening(self):
        r = Repo(self)
        r.write(".config/testing-bans.toml", BANS.replace('level = "RED"', 'level = "NOTE"', 1))
        r.commit()
        self.assertIn("RED", [f.level for f in r.rules([".config/testing-bans.toml"], env={})])

    def test_tightening_the_ban_data_is_free(self):
        r = Repo(self)
        tight = BANS.replace('scope = ["scripts/tests/**", "workshop/evals/**"]',
                             'scope = ["scripts/tests/**", "workshop/evals/**", "scripts/**"]', 1)
        tight += ('\n[[ban]]\nid = "py-extra"\nrule = "TV-S2"\nlang = "python"\n'
                  'pattern = \'\\bnever_this\\(\'\nscope = ["scripts/tests/**"]\ncontext = []\n'
                  'level = "RED"\nexplanation = "x"\nreplacement = "y"\n')
        r.write(".config/testing-bans.toml", tight)
        r.commit()
        self.assertEqual([], r.rules([".config/testing-bans.toml"], env={}))

    def test_a_legacy_flaky_entry_cannot_be_added(self):
        r = Repo(self, base={".config/flaky-legacy.toml": "# schema\n"})
        r.write(".config/flaky-legacy.toml", self.LEGACY)
        r.commit()
        found = r.rules([".config/flaky-legacy.toml"], env={})
        self.assertIn("RED", [f.level for f in found], found)
        self.assertIn("meclaw-x::a::t", found[0].message)

    def test_removing_a_legacy_entry_is_free(self):
        r = Repo(self, base={".config/flaky-legacy.toml": self.LEGACY})
        r.write(".config/flaky-legacy.toml", "# schema\n")
        r.commit()
        self.assertEqual([], r.rules([".config/flaky-legacy.toml"], env={}))

    def test_removing_a_ruff_ban_is_a_loosening(self):
        ruff = (REPO / "ruff.toml").read_text(encoding="utf-8")
        r = Repo(self, base={"ruff.toml": ruff})
        r.write("ruff.toml", ruff.replace('"time.sleep".msg = "py-time"\n', ""))
        r.commit()
        self.assertIn("RED", [f.level for f in r.rules(["ruff.toml"], env={})])

    def test_changing_section_11_needs_a_ruling(self):
        doc = "# Rules\n\n## 10. Ten\n\nten\n\n## 11. Tests\n\nThe rule.\n"
        r = Repo(self, base={"docs/development-rules.md": doc})
        r.write("docs/development-rules.md", doc.replace("The rule.", "The rule, softer."))
        r.commit()
        self.assertIn("RED", [f.level for f in r.rules(["docs/development-rules.md"], env={})])
        r.write("docs/development-rules.md", doc.replace("ten\n", "ten, longer\n"))
        r.commit()
        self.assertEqual([], r.rules(["docs/development-rules.md"], env={}))


class Inventory(Case):

    def test_inventory_rising_over_the_baseline_is_red_in_integration(self):
        r = Repo(self, base={RS: fn_rs("    std::thread::sleep(D);", "#[test]")})
        counts = tb.inventory(r.root)
        self.assertEqual(1, counts["rs-thread-sleep"])
        tb.write_baseline(r.root, counts)
        self.assertEqual(0, tb.check_inventory(r.root)[0])
        r.write("crates/meclaw-x/tests/b.rs", fn_rs("    std::thread::sleep(D);", "#[test]"))
        rc, line = tb.check_inventory(r.root)
        self.assertEqual(1, rc)
        self.assertIn("rs-thread-sleep 2 > 1", line)

    def test_write_lowers_and_never_raises(self):
        r = Repo(self)
        tb.write_baseline(r.root, {"rs-thread-sleep": 3, "py-time": 1})
        tb.write_baseline(r.root, {"rs-thread-sleep": 1, "py-time": 4})
        self.assertEqual({"rs-thread-sleep": 1, "py-time": 1},
                         {k: v for k, v in tb.read_baseline(r.root).items()
                          if k in ("rs-thread-sleep", "py-time")})


LIB = "crates/meclaw-x/src/lib.rs"
SLEEPS = "#[test]\nfn t() {\n    std::thread::sleep(D);\n}\n"
BASELINE = ".config/testing-bans.baseline.json"
CODE = "scripts/testing_bans.py"


def baseline(**counts):
    return json.dumps({"counts": counts}, indent=2) + "\n"


class StringsAndScope(Case):
    """Re-check tv-p0 A1-A3: what a ban sees -- strings where its subject is a
    string, and every file a `#[cfg(test)]` declares, not only inline blocks."""

    def test_js_racy_api_hits_networkidle_in_a_string(self):
        # A1: `networkidle` only ever stands in a string, and strings were blanked.
        self.assert_red("js-racy-api", JS, "await page.goto(u, { waitUntil: 'networkidle' });\n")
        self.assert_red("js-racy-api", JS, 'await page.waitForLoadState("networkidle");\n')

    def test_js_racy_api_ignores_networkidle_in_a_comment(self):
        found = self.added(JS, "// no networkidle here\nawait expect(x).toBeVisible();\n")
        self.assertNotIn("js-racy-api", ids(found))

    def test_rs_colony_db_hits_a_string_over_two_lines(self):
        # A2: `[^"\n]*` stopped at the line break (21 SQL strings of the tree).
        self.assert_red("rs-colony-db", RS, fn_rs(
            '    let q = "SELECT count(*)\n             FROM message_log";\n    run(q);', "#[test]"))

    def test_rs_colony_db_ignores_code_between_two_strings(self):
        # a helper named after the log between two literals is code, not a path or SQL
        found = self.added(RS, fn_rs('    check("a", wait_for_message_log_count(1), "b");',
                                     "#[test]"))
        self.assertNotIn("rs-colony-db", ids(found))

    def test_an_out_of_line_cfg_test_module_is_test_code(self):
        # A3: `#[cfg(test)] mod tests;` puts tests.rs under the bans, a plain `mod` does not.
        base = {LIB: "pub mod prod;\n\n#[cfg(test)]\nmod tests;\n"}
        self.assert_red("rs-thread-sleep", "crates/meclaw-x/src/tests.rs", SLEEPS, base=base)
        prod = self.added("crates/meclaw-x/src/prod.rs", SLEEPS, base=base)
        self.assertNotIn("rs-thread-sleep", ids(prod))

    def test_a_test_module_of_a_non_mod_file_and_its_children_are_test_code(self):
        base = {LIB: "pub mod m;\n",
                "crates/meclaw-x/src/m.rs": "#[cfg(test)]\n#[allow(dead_code)]\nmod lane;\n",
                "crates/meclaw-x/src/m/lane.rs": "mod helpers;\n"}
        self.assert_red("rs-thread-sleep", "crates/meclaw-x/src/m/lane.rs",
                        "mod helpers;\n" + SLEEPS, base=base)
        self.assert_red("rs-thread-sleep", "crates/meclaw-x/src/m/lane/helpers.rs", SLEEPS,
                        base=base)

    def test_a_file_with_an_inner_cfg_test_is_test_code(self):
        self.assert_red("rs-thread-sleep", "crates/meclaw-x/src/probe.rs",
                        "#![cfg(test)]\n" + SLEEPS)

    def test_the_inventory_counts_out_of_line_test_modules(self):
        r = Repo(self, base={LIB: "#[cfg(test)]\nmod tests;\n",
                             "crates/meclaw-x/src/tests.rs": SLEEPS})
        self.assertEqual(1, tb.inventory(r.root)["rs-thread-sleep"])


def js_source(text):
    return tb.Source(text, "js")


def js_statements(text):
    src = js_source(text)
    return [src.code[a:b].strip() for a, b in src.spans]


class JsRegexLiterals(Case):
    """#1208: a JS regex literal is a literal. `//` inside one opened a comment
    (display-type-browser.mjs `/^https?:\\/\\//`), the statement ran to the
    next real line end hundreds of lines later, and every change in between
    re-reported an untouched `setTimeout`."""

    def test_a_regex_with_two_slashes_opens_no_comment(self):
        src = js_source('const host = url.replace(/^https?:\\/\\//, "");\n'
                        "setTimeout(done, 50);\n")
        self.assertEqual([], src.comments)
        a, b = src.statement(0)
        self.assertEqual(1, src.line(max(a, b - 1)))
        self.assertEqual(2, src.line(src.statement(src.code.index("setTimeout"))[0]))

    def test_a_change_beside_the_regex_does_not_report_a_far_sleep(self):
        body = ("const host = url.replace(/^https?:\\/\\//, %s);\n"
                "function finish() {\n  setTimeout(done, 50);\n}\n")
        found = self.added(JS, body % '"b"', base={JS: body % '"a"'})
        self.assertNotIn("js-sleep", ids(found), "\n".join(map(str, found)))

    def test_regex_after_an_operator_paren_comma_or_keyword(self):
        for text in ("x = /a\\/\\//;\ny();\n",
                     "f(/a\\/\\//);\ny();\n",
                     "f(1, /a\\/\\//);\ny();\n",
                     "const m = !/a\\/\\//.test(s);\ny();\n",
                     "const m = s && /a\\/\\//.test(s);\ny();\n",
                     "const m = [/a\\/\\//];\ny();\n",
                     "const o = {k: /a\\/\\//};\ny();\n",
                     "function g(s) {\n  return /a\\/\\//.test(s);\n}\ny();\n",
                     "/a\\/\\//.test(s);\ny();\n"):
            with self.subTest(text=text):
                src = js_source(text)
                self.assertEqual([], src.comments)
                self.assertEqual("y();", js_statements(text)[-1])

    def test_division_after_a_paren_identifier_bracket_or_number(self):
        text = "const d = f(x) / g(y) / 2;\nconst e = a[0] / b / 3 / (c);\ny();\n"
        src = js_source(text)
        self.assertIn("g(y)", src.code)
        self.assertIn("/ (c)", src.code)
        self.assertEqual(["const d = f(x) / g(y) / 2;", "const e = a[0] / b / 3 / (c);", "y();"],
                         js_statements(text))

    def test_a_comment_after_a_division_stays_a_comment(self):
        src = js_source("const a = b / c; // per https://x\nconst z = 1 /* n */ / 2;\n")
        self.assertEqual(["// per https://x", "/* n */"], [c for _, c in src.comments])

    def test_escapes_and_a_slash_in_a_class(self):
        for text in ("const r = /[/]+\\//g; y();\n",
                     "const r = /\\[(\\/)\\]/; y();\n",
                     "const r = /[\\]/]\\(/; y();\n",
                     'const r = /"/; y("x");\n',
                     "const r = /'{/; y('x');\n"):
            with self.subTest(text=text):
                src = js_source(text)
                self.assertEqual([], src.comments)
                self.assertTrue(js_statements(text)[-1].startswith("y("), js_statements(text))
                self.assertEqual({}, {k: v for k, v in src.pairs.items()
                                      if src.code[k] == "{" and k < src.code.index("y();")})


class Reservation(Case):
    """Re-check tv-p0 A4/A5: the baseline only sinks, and the ban code is
    reserved like the ban data -- both under `testing-rules`."""

    def reds(self, r, files, env=None):
        return [f for f in r.rules(files, env=env or {}) if f.level == "RED"]

    def test_raising_the_baseline_is_red(self):
        r = Repo(self, base={BASELINE: baseline(**{"rs-thread-sleep": 3, "py-time": 2})})
        r.write(BASELINE, baseline(**{"rs-thread-sleep": 4, "py-time": 2}))
        r.commit()
        red = self.reds(r, [BASELINE])
        self.assertTrue(any("rs-thread-sleep 3 -> 4" in f.message for f in red), red)

    def test_removing_a_baseline_count_is_red(self):
        r = Repo(self, base={BASELINE: baseline(**{"rs-thread-sleep": 3, "py-time": 2})})
        r.write(BASELINE, baseline(**{"rs-thread-sleep": 3}))
        r.commit()
        red = self.reds(r, [BASELINE])
        self.assertTrue(any("py-time removed" in f.message for f in red), red)

    def test_deleting_the_baseline_is_red(self):
        r = Repo(self, base={BASELINE: baseline(**{"rs-thread-sleep": 3})})
        r.git("rm", "-q", BASELINE)
        r.commit()
        self.assertTrue(self.reds(r, [BASELINE]))

    def test_lowering_the_baseline_and_a_new_ban_count_are_free(self):
        r = Repo(self, base={BASELINE: baseline(**{"rs-thread-sleep": 3, "py-time": 2})})
        r.write(BASELINE, baseline(**{"rs-thread-sleep": 1, "py-time": 2, "py-new": 7}))
        r.commit()
        self.assertEqual([], r.rules([BASELINE], env={}))

    def test_changing_the_ban_code_needs_a_ruling(self):
        r = Repo(self, base={CODE: "X = 1\n"})
        r.write(CODE, "X = 2\n")
        r.commit()
        red = self.reds(r, [CODE])
        self.assertTrue(any(CODE in f.message for f in red), red)
        ok = r.rules([CODE], env={tb.RULING_ENV: "OR-TV-2"})
        self.assertEqual([], [f for f in ok if f.level == "RED"], ok)

    def test_the_ban_code_born_in_the_diff_is_free(self):
        r = Repo(self)
        r.write(CODE, "X = 1\n")
        r.commit()
        self.assertEqual([], r.rules([CODE], env={}))

    # Re-check tv-p0 fix1 M1: a git error never reads as "born" or "no base"
    BROKEN = "0" * 40

    def test_a_git_error_on_a_reserved_file_is_red(self):
        r = Repo(self, base={CODE: "X = 1\n"})
        r.write(CODE, "X = 2\n")
        r.commit()
        red = [f for f in r.rules([CODE], base=self.BROKEN, env={}) if f.level == "RED"]
        self.assertTrue(any(CODE in f.message and "git" in f.message for f in red), red)

    def test_a_reserved_file_without_a_merge_base_is_red(self):
        r = Repo(self, branch=None)
        r.git("branch", "-m", "master", "published")
        r.write(CODE, "X = 1\n")
        r.commit()
        red = [f for f in r.rules([CODE], env={}) if f.level == "RED"]
        self.assertTrue(any("merge base" in f.message for f in red), red)

    def test_a_failing_register_history_is_red_in_integration(self):
        r = Repo(self)
        r.write(".config/testing-rulings.toml", REGISTER + "\n# more\n")
        r.commit("testing-ruling: OR-TV-3 (coordinator, 2026-10-11)")
        found = r.rules([".config/testing-rulings.toml"], mode="integration",
                        base=self.BROKEN)
        self.assertIn("RED", [f.level for f in found], found)

    def test_a_failing_show_of_an_existing_file_is_red(self):
        r = Repo(self, base={CODE: "X = 1\n"})
        r.write(CODE, "X = 2\n")
        r.commit()
        real = tb._git

        def flaky(tree, *args):
            return None if args[:1] == ("show",) else real(tree, *args)
        with mock.patch.object(tb, "_git", flaky):
            red = [f for f in r.rules([CODE], env={}) if f.level == "RED"]
        self.assertTrue(any(CODE in f.message and "git" in f.message for f in red), red)

    def test_a_ban_without_a_baseline_count_is_red_in_the_inventory(self):
        # A4: a missing file or count left the ban unchecked
        r = Repo(self, base={RS: fn_rs("    std::thread::sleep(D);", "#[test]")})
        rc, line = tb.check_inventory(r.root)
        self.assertEqual(1, rc, line)
        self.assertIn("no baseline", line)
        tb.write_baseline(r.root, {"rs-thread-sleep": 1})
        rc, line = tb.check_inventory(r.root)
        self.assertEqual(1, rc, line)
        self.assertIn("py-time", line)


CLOCK = ".github/gates/clock-bans/clippy.toml"
BUDGET = ".github/gates/unwrap_budget.txt"
WORKSPACE = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/a", "crates/b"]\n',
    "crates/a/Cargo.toml": '[package]\nname = "a"\n',
    "crates/b/Cargo.toml": '[package]\nname = "b"\n',
}


def clock(**reasons):
    rows = ['    { path = "%s", reason = "%s" },' % (p.replace("_", "::"), r)
            for p, r in reasons.items()]
    return "disallowed-methods = [\n" + "\n".join(rows) + "\n]\n"


def budget(*rows):
    return "# <package> <unwrap+expect> <mono> <wall>\n" + "".join(r + "\n" for r in rows)


class RatchetOnlySinks(Case):
    """Re-check tv-p1b M1 (OR-TV-10): the clock-bans/unwrap ratchet only sinks --
    a raised pin, a dropped row of a live package, a deleted budget file and a
    clock ban moved from `mono:` to `wall:` are loosenings."""

    def reds(self, r, files):
        return [f for f in r.rules(files, env={}) if f.level == "RED"]

    def bumped(self, old, new, rel=BUDGET, extra=None):
        base = dict(WORKSPACE)
        base.update(extra or {})
        base[rel] = old
        r = Repo(self, base=base)
        r.write(rel, new)
        r.commit()
        return r

    def test_moving_a_clock_ban_from_mono_to_wall_is_red(self):
        r = self.bumped(clock(std_time_Instant_now="mono: tokio"),
                        clock(std_time_Instant_now="wall: tokio"), rel=CLOCK)
        red = self.reds(r, [CLOCK])
        self.assertTrue(any("std::time::Instant::now" in f.message and "mono -> wall"
                            in f.message for f in red), red)

    def test_moving_a_clock_ban_from_wall_to_mono_is_free(self):
        r = self.bumped(clock(std_time_SystemTime_now="wall: w"),
                        clock(std_time_SystemTime_now="mono: w"), rel=CLOCK)
        self.assertEqual([], r.rules([CLOCK], env={}))

    def test_raising_an_unwrap_budget_pin_is_red(self):
        r = self.bumped(budget("a 76 0 22", "b 3 0 1"), budget("a 76 1 22", "b 3 0 1"))
        red = self.reds(r, [BUDGET])
        self.assertTrue(any(BUDGET in f.message and "a mono 0 -> 1" in f.message
                            for f in red), red)

    def test_raising_a_pin_over_the_one_column_base_is_red(self):
        r = self.bumped(budget("a 5", "b 3"), budget("a 6 0 0", "b 3 0 1"))
        red = self.reds(r, [BUDGET])
        self.assertTrue(any("a unwrap 5 -> 6" in f.message for f in red), red)
        self.assertFalse(any(" b " in f.message for f in red), red)   # new columns are free

    def test_dropping_the_row_of_a_live_package_is_red(self):
        r = self.bumped(budget("a 1 0 0", "b 2 0 0"), budget("b 2 0 0"))
        red = self.reds(r, [BUDGET])
        self.assertTrue(any("row a removed" in f.message for f in red), red)

    def test_dropping_the_row_of_a_departed_package_is_free(self):
        r = self.bumped(budget("a 1 0 0", "b 2 0 0", "gone 5 0 0"),
                        budget("a 1 0 0", "b 2 0 0"))
        self.assertEqual([], r.rules([BUDGET], env={}))

    def test_deleting_the_unwrap_budget_is_red(self):
        r = Repo(self, base=dict(WORKSPACE, **{BUDGET: budget("a 1 0 0")}))
        r.git("rm", "-q", BUDGET)
        r.commit()
        self.assertTrue(self.reds(r, [BUDGET]))

    def test_an_unreadable_budget_row_is_red(self):
        r = self.bumped(budget("a 1 0 0"), budget("a 1 0"))
        self.assertTrue(self.reds(r, [BUDGET]))

    def test_lowering_a_pin_a_new_row_and_a_comment_are_free(self):
        r = self.bumped(budget("a 76 1 22", "b 3 0 1"),
                        budget("a 70 0 22", "b 3 0 1", "c 9 0 0") + "# why\n")
        self.assertEqual([], r.rules([BUDGET], env={}))


class Roles(unittest.TestCase):
    """Re-check tv-p0 C2: the register names roles, never persons (export R5)."""

    ENTRY = {"bans": ["rs-thread-sleep"], "paths": ["crates/**"]}

    def test_by_is_a_role(self):
        self.assertEqual(("owner", "coordinator"), tb.OWNERS)
        for who in ("owner", "coordinator"):
            reg = {"OR-X": dict(self.ENTRY, by=who)}
            self.assertIsNone(tb.ruling_problem(reg, "OR-X", path=RS, ban="rs-thread-sleep",
                                                today=TODAY))
        reg = {"OR-X": dict(self.ENTRY, by="someone")}
        self.assertIn("not the owner or the coordinator",
                      tb.ruling_problem(reg, "OR-X", path=RS, ban="rs-thread-sleep", today=TODAY))

    def test_the_real_register_names_roles(self):
        register = tb.load_register(REPO) or {}
        self.assertTrue(register)
        self.assertEqual([], [rid for rid, e in register.items() if e.get("by") not in tb.OWNERS])


class Wiring(unittest.TestCase):

    def test_precheck_runs_both_checks(self):
        import precheck
        names = [c.__name__ for c in precheck.NATIVE_CHECKS]
        self.assertIn("check_test_bans", names)
        self.assertIn("check_testing_rules", names)

    def test_every_legacy_entry_names_an_issue_and_a_cause_class(self):
        """R-TV-W9: the initial stock (strand tv-p1c4) carries for every test an
        issue number and an armable cause class, once each; an entry that the
        fail-closed reader would drop must not stand on the list."""
        from pipeline import flakes
        entries = tb.read_legacy(REPO)
        # The list only shrinks, and empty is where it ends (tv-bn FN5,
        # 2026-10-13): no lower bound on its length, only on its form.
        tests = [e.get("test") for e in entries]
        self.assertEqual(len(tests), len(set(tests)), "a test stands twice")
        for e in entries:
            self.assertIsInstance(e.get("issue"), int, e)
            self.assertGreater(e["issue"], 0, e)
            self.assertTrue(flakes.armable_cause(e.get("cause")), e)
        self.assertEqual(len(entries), len(flakes.legacy_entries(REPO)))


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
