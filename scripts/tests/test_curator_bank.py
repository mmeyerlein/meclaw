"""`scripts/curator_bank.py capture` writes what the capture test prints (GH #1465).

The script is the reader of one test of the cell crate: it builds no wall and
runs no rebuild. Pinned here, against an output made up in this file: every
capture line becomes a file with its content, a damaged line is refused and
said, the manifest names the tier of every planted point and whether it is
expected in the request, a point expected in its request and not in it ends
the run red, a point the role drops (tier `none`) is not expected and ends
the run red when it stands in a request to the summarizer, an output without
a capture is exit 2, and the command of the run is the strand kit's
single-test verb with the capture test's filter. Roles are names of the
capture, nothing here knows one.

D58 plant (GH #1490): a `third_party` point carries its `subject` into the
manifest, a `superseded` point its `old_key` and `new_key`; the old row of a
`superseded` point is judged like a planted row of its own -- expected in the
request where it left the raw part and lies in the summary, red when missing
there, red when it lies in the tier `none` and stands in a request.

d58-3 tag (GH #1527, contract point 7): `capture --window` reads the other
test of the bank, `gh_d583_tag_bank_the_day_window_is_captured` -- the window a
day change sends per wall, horizon and arm of `day_floor` -- through the same
single-test verb of the kit, writes every line as its file and a manifest
row per window (wall, horizon, arm, mark, whether it rebuilt, requests, the
size of the window), and is exit 2 without a line. Without `--window` the
capture is the one of before.
"""

import hashlib
import json
import pathlib
import tempfile
import unittest

from scripts import curator_bank
from scripts.tests import store_isolation


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


def point(key, left_raw, text_in, tier=None, own_way=False):
    """A planted point as the capture test prints it; without a tier it is
    raw while it stands in the raw part and in the summary once it left."""
    tier = tier or ("summary" if left_raw else "raw")
    return {"kind": "fact", "key": key, "text": "filed as %s" % key, "piece": 3,
            "seq": 7, "left_raw": left_raw, "tier": tier,
            "expected_in_request": left_raw and tier == "summary",
            "must_be_absent": tier == "none" and not own_way,
            "key_in_request": text_in, "text_in_request": text_in}


def superseded(new, old):
    """A `superseded` point: the row of the new value, the old row under `old`."""
    rec = dict(new, kind="superseded", old_key=old["key"], new_key=new["key"])
    rec["old"] = dict(old, kind="superseded")
    return rec


def record(wall, role, form, planted, requests=1):
    return {"wall": wall, "role": role, "form": form, "knobs": {},
            "aim": 45000, "cover": 9, "window_target": 100000,
            "requests": [{"route": "summarize", "hop": {}, "body": {"messages": []}}] * requests,
            "planted": planted}


def line(name, rec, digest=None):
    text = json.dumps(rec, separators=(",", ":"))
    digest = digest or hashlib.sha256(text.encode("utf-8")).hexdigest()
    return "%s %s %s %s\n" % (curator_bank.MARK, name, digest, text)


class CaptureTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def run_log(self, lines):
        log = self.dir / "earlier.log"
        log.write_text("".join(lines), encoding="utf-8")
        out = self.dir / "out"
        status = curator_bank.main(["capture", "--out", str(out), "--from-log", str(log)])
        return status, out

    def test_every_line_becomes_a_file_with_its_content(self):
        a = record("wall_a", "role_a", "prose", [point("k1", True, True), point("k2", False, False)])
        b = record("wall_a", "role_a", "brief", [point("k1", True, True)])
        status, out = self.run_log([
            "    PASS [  12.1s] some test\n",
            line("wall_a.role_a.prose.json", a),
            # The runner may show the output of a test twice.
            line("wall_a.role_a.prose.json", a),
            "--- STDOUT ---\n",
            line("wall_a.role_a.brief.json", b),
        ])
        self.assertEqual(status, 0)
        self.assertEqual(json.loads((out / "wall_a.role_a.prose.json").read_text()), a)
        self.assertEqual(json.loads((out / "wall_a.role_a.brief.json").read_text()), b)
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual([r["file"] for r in manifest["files"]],
                         ["wall_a.role_a.brief.json", "wall_a.role_a.prose.json"])
        prose = manifest["files"][1]
        self.assertEqual((prose["planted"], prose["left_raw"], prose["missing"]), (2, 1, []))
        self.assertEqual(manifest["problems"], [])

    def test_the_manifest_names_tier_and_expectation_of_every_point(self):
        rec = record("wall_a", "role_a", "prose", [
            point("k1", True, False, tier="none"),
            point("k2", True, True),
            point("k3", False, False)])
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 0)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual([(p["key"], p["tier"], p["expected_in_request"], p["in_request"])
                          for p in row["points"]],
                         [("k1", "none", False, False), ("k2", "summary", True, True),
                          ("k3", "raw", False, False)])
        self.assertEqual((row["left_raw"], row["expected_in_request"]), (2, 1))
        self.assertEqual((row["missing"], row["unexpected"]), ([], []))

    def test_a_dropped_point_missing_from_the_request_is_no_miss(self):
        rec = record("wall_a", "role_a", "prose", [point("k1", True, False, tier="none")])
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 0)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual((row["missing"], row["unexpected"]), ([], []))

    def test_a_dropped_point_in_a_request_to_the_summarizer_is_red(self):
        rec = record("wall_a", "role_a", "prose", [point("k1", True, True, tier="none")])
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 1)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual((row["missing"], row["unexpected"]), ([], ["k1"]))

    def test_a_dropped_point_on_the_roles_own_way_is_neither_hit_nor_miss(self):
        # The request carries the old window in front there, the point with it.
        rec = record("wall_a", "role_a", "self",
                     [point("k1", True, True, tier="none", own_way=True)])
        status, out = self.run_log([line("wall_a.role_a.self.json", rec)])
        self.assertEqual(status, 0)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual((row["expected_in_request"], row["missing"], row["unexpected"]),
                         (0, [], []))

    def test_a_planted_point_missing_from_its_request_is_red(self):
        rec = record("wall_a", "role_a", "prose", [point("k1", True, False)])
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 1)
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual(manifest["files"][0]["missing"], ["k1"])

    def test_a_capture_without_a_request_misses_what_left_the_raw_part(self):
        rec = record("wall_a", "role_a", "prose", [point("k1", True, True)], requests=0)
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 1)
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual(manifest["files"][0]["missing"], ["k1"])

    def test_the_new_kinds_carry_their_fields_into_the_manifest(self):
        third = dict(point("k1", True, True), kind="third_party", subject="daughter")
        sup = superseded(point("k3", True, True), point("k2", True, True))
        rec = record("wall_a", "role_a", "prose", [third, sup])
        status, out = self.run_log([line("wall_a.role_a.prose.json", rec)])
        self.assertEqual(status, 0)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual(row["points"][0]["subject"], "daughter")
        self.assertEqual((row["points"][1]["old_key"], row["points"][1]["new_key"]), ("k2", "k3"))
        self.assertEqual((row["points"][1]["old_expected_in_request"],
                          row["points"][1]["old_in_request"]), (True, True))
        # Two points, three rows: both rows of the superseded point count.
        self.assertEqual((row["planted"], row["left_raw"], row["expected_in_request"]), (2, 3, 3))
        self.assertEqual((row["missing"], row["unexpected"]), ([], []))

    def test_an_old_row_missing_from_its_request_is_red(self):
        sup = superseded(point("k3", True, True), point("k2", True, False))
        status, out = self.run_log([line("wall_a.role_a.prose.json",
                                         record("wall_a", "role_a", "prose", [sup]))])
        self.assertEqual(status, 1)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual(row["missing"], ["k2"])

    def test_a_dropped_old_row_in_a_request_to_the_summarizer_is_red(self):
        sup = superseded(point("k3", True, True), point("k2", True, True, tier="none"))
        status, out = self.run_log([line("wall_a.role_a.prose.json",
                                         record("wall_a", "role_a", "prose", [sup]))])
        self.assertEqual(status, 1)
        row = json.loads((out / "manifest.json").read_text())["files"][0]
        self.assertEqual((row["missing"], row["unexpected"]), ([], ["k2"]))

    def test_a_damaged_line_is_refused_and_said(self):
        good = record("wall_a", "role_a", "prose", [point("k1", True, True)])
        bad = record("wall_b", "role_a", "prose", [point("k1", True, True)])
        status, out = self.run_log([
            line("wall_a.role_a.prose.json", good),
            line("wall_b.role_a.prose.json", bad, digest="0" * 64),
            line("../outside.role_a.prose.json", bad),
        ])
        self.assertEqual(status, 1)
        self.assertTrue((out / "wall_a.role_a.prose.json").exists())
        self.assertFalse((out / "wall_b.role_a.prose.json").exists())
        self.assertEqual(sorted(p.name for p in self.dir.iterdir()), ["earlier.log", "out"])
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual(len(manifest["problems"]), 2)

    def test_two_contents_under_one_name_are_a_problem(self):
        a = record("wall_a", "role_a", "prose", [point("k1", True, True)])
        b = record("wall_a", "role_a", "prose", [point("k2", True, True)])
        found, problems = curator_bank.parse([line("wall_a.role_a.prose.json", a),
                                              line("wall_a.role_a.prose.json", b)])
        self.assertEqual(found["wall_a.role_a.prose.json"], a)
        self.assertEqual(len(problems), 1)

    def test_an_output_without_a_capture_is_exit_2(self):
        status, out = self.run_log(["strand: REFUSED -- no lane lease\n"])
        self.assertEqual(status, 2)
        self.assertFalse((out / "manifest.json").exists())

    def test_the_capture_is_read_from_the_log_the_kit_names(self):
        said = ["strand: lane x rev 0\n", "strand: test log /some/where/test.log\n", "TEST RED\n"]
        self.assertEqual(curator_bank.test_log_of(said), "/some/where/test.log")
        self.assertIsNone(curator_bank.test_log_of(["TEST RED\n"]))

    def test_the_run_is_the_single_test_verb_of_the_kit(self):
        command = curator_bank.kit_command("/tree")
        self.assertEqual(command[:4], ["/tree/scripts/strand.sh", "-C", "/tree", "test"])
        self.assertIn(curator_bank.TEST, command[4])
        self.assertEqual(command[5:], ["--success-output", "immediate"])


def window_record(wall, horizon, arm, whole, chars):
    """A window as the day window test prints it: one rebuild on the running
    day, the whole day raw (`whole`, arm "1") or curated (arm "0")."""
    return {"wall": wall, "role": "role_a", "horizon": horizon, "day_floor": arm,
            "mark": "none", "reason": "cache", "rebuilt": True,
            "requests": 0 if whole else 1, "cover": 9,
            "pieces": 8, "raw_pieces": 8 if whole else 3, "whole_day": whole,
            "window_chars": chars,
            "window": [{"origin": "user", "type": "text", "text": "x" * chars}],
            "snapshot": [{"id": "b:1"}], "roles": "defaults"}


class WindowCaptureTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def run_log(self, lines):
        log = self.dir / "earlier.log"
        log.write_text("".join(lines), encoding="utf-8")
        out = self.dir / "out"
        status = curator_bank.main(["capture", "--window", "--out", str(out),
                                    "--from-log", str(log)])
        return status, out

    def test_the_window_capture_runs_the_day_window_test(self):
        self.assertEqual(curator_bank.WINDOW_TEST, "gh_d583_tag_bank_the_day_window_is_captured")
        command = curator_bank.kit_command("/tree", window=True)
        self.assertEqual(command[:4], ["/tree/scripts/strand.sh", "-C", "/tree", "test"])
        self.assertIn(curator_bank.WINDOW_TEST, command[4])
        self.assertNotIn(curator_bank.TEST, command[4])
        self.assertEqual(command[5:], ["--success-output", "immediate"])
        # Without the mode the capture is the one of before.
        self.assertIn(curator_bank.TEST, curator_bank.kit_command("/tree")[4])

    def test_every_window_becomes_a_file_and_a_manifest_row(self):
        on = window_record("phone_day", "task", "1", True, 900)
        off = window_record("phone_day", "task", "0", False, 120)
        status, out = self.run_log([
            line("phone_day.task.floor_1.json", on),
            line("phone_day.task.floor_0.json", off),
        ])
        self.assertEqual(status, 0)
        self.assertEqual(json.loads((out / "phone_day.task.floor_1.json").read_text()), on)
        self.assertEqual(json.loads((out / "phone_day.task.floor_0.json").read_text()), off)
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual(manifest["test"], curator_bank.WINDOW_TEST)
        self.assertEqual(manifest["problems"], [])
        rows = {r["file"]: r for r in manifest["files"]}
        self.assertEqual(sorted(rows), ["phone_day.task.floor_0.json",
                                        "phone_day.task.floor_1.json"])
        # Arm "1" is the whole running day raw, arm "0" the curated one: the
        # arms of M1 (OR-583.tag.4), the curated window the smaller.
        row = rows["phone_day.task.floor_1.json"]
        self.assertEqual((row["wall"], row["horizon"], row["day_floor"], row["mark"],
                          row["reason"], row["rebuilt"], row["whole_day"], row["raw_pieces"],
                          row["pieces"], row["requests"], row["window_chars"],
                          row["messages"], row["snapshot"]),
                         ("phone_day", "task", "1", "none", "cache", True, True, 8, 8, 0, 900,
                          1, True))
        row = rows["phone_day.task.floor_0.json"]
        self.assertEqual((row["day_floor"], row["whole_day"], row["raw_pieces"],
                          row["requests"], row["window_chars"]),
                         ("0", False, 3, 1, 120))

    def test_a_damaged_window_line_is_said_and_red(self):
        on = window_record("phone_day", "task", "1", True, 120)
        status, out = self.run_log([
            line("phone_day.task.floor_1.json", on),
            line("phone_day.all.floor_1.json", on, digest="0" * 64),
        ])
        self.assertEqual(status, 1)
        self.assertFalse((out / "phone_day.all.floor_1.json").exists())
        manifest = json.loads((out / "manifest.json").read_text())
        self.assertEqual(len(manifest["problems"]), 1)

    def test_a_window_capture_without_a_line_is_exit_2(self):
        status, out = self.run_log(["strand: REFUSED -- no lane lease\n"])
        self.assertEqual(status, 2)
        self.assertFalse((out / "manifest.json").exists())



class WindowRolesTest(unittest.TestCase):
    """`capture --window --roles <file>` (OR-583.tag.5): the caller names the
    roles; the file lies beside the test for the run and is gone after. The
    run must take them (re-check I-B): a window on the defaults, a role
    without a window, a window with other knobs is exit 1, and so is a run
    without `--roles` whose windows did not come from the defaults (re-check
    M-E); a test that
    refuses the file prints no window, exit 2. A horizon outside day, task,
    all is refused before the run (re-check M-C)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)
        self.root = self.dir / "tree"
        (self.root / "crates" / "meclaw-cells" / "tests").mkdir(parents=True)
        self.placed = self.root / curator_bank.ROLES_PATH
        self.seen = []
        # What the fake test does with the file: echo it (the default), or
        # a function of the placed roles giving the windows it prints.
        self.reply = None
        real = curator_bank.run_kit

        def fake(root, log_path, window=False):
            placed = (json.loads(self.placed.read_text(encoding="utf-8"))
                      if self.placed.exists() else None)
            self.seen.append((root, window, placed))
            if self.reply is not None:
                return self.reply(placed)
            return self.windows(placed or DEFAULT_ROLES, "file" if placed else "defaults"), 0

        curator_bank.run_kit = fake
        self.addCleanup(setattr, curator_bank, "run_kit", real)

    @staticmethod
    def windows(roles, source):
        """The lines the window test prints for `roles` read from `source`."""
        lines = []
        for role in roles:
            rec = window_record("phone_day", role.get("knobs", {}).get("horizon", "task"),
                                "1", True, 900)
            rec.update(role=role["name"], set=role["set"], knobs=role.get("knobs", {}),
                       roles=source)
            lines.append(line("phone_day.%s.floor_1.json" % role["name"], rec))
        return lines

    def roles_file(self, roles):
        path = self.dir / "roles.json"
        path.write_text(json.dumps(roles), encoding="utf-8")
        return str(path)

    def capture(self, *extra):
        return curator_bank.main(["capture", "--window", "--out", str(self.dir / "out"),
                                  "--root", str(self.root)] + list(extra))

    def test_the_roles_lie_beside_the_test_for_the_run_and_are_gone_after(self):
        self.assertEqual(curator_bank.ROLES_PATH,
                         "crates/meclaw-cells/tests/curator_window_roles.json")
        status = self.capture("--roles", self.roles_file(ROLES))
        self.assertEqual(status, 0)
        self.assertEqual(self.seen, [(str(self.root), True, ROLES)])
        self.assertFalse(self.placed.exists())
        self.assertTrue((self.dir / "out" / "phone_day.role_a.floor_1.json").exists())

    def test_without_roles_no_file_is_placed(self):
        self.assertEqual(self.capture(), 0)
        self.assertEqual(self.seen, [(str(self.root), True, None)])

    def test_a_roles_file_left_beside_the_test_does_not_pass_for_the_defaults(self):
        # Re-check M-E: a capture killed before it removed its file leaves
        # it beside the test; a later run without --roles takes it and must
        # not pass for the defaults.
        self.reply = lambda placed: (self.windows(ROLES, "file"), 0)
        self.assertEqual(self.capture(), 1)
        self.assertEqual(self.seen, [(str(self.root), True, None)])

    def test_a_run_on_the_defaults_does_not_pass_for_the_ordered_roles(self):
        # The file did not reach the test: it ran its defaults. Even a role
        # named like a default is no window of the ordered one.
        self.reply = lambda placed: (self.windows(DEFAULT_ROLES, "defaults"), 0)
        ordered = [{"name": "all", "set": "consult", "knobs": {"horizon": "all"}}]
        self.assertEqual(self.capture("--roles", self.roles_file(ordered)), 1)
        self.assertFalse(self.placed.exists())

    def test_an_ordered_role_without_a_window_is_red(self):
        self.reply = lambda placed: (self.windows(placed[:1], "file"), 0)
        self.assertEqual(self.capture("--roles", self.roles_file(ROLES)), 1)

    def test_a_window_with_other_knobs_than_ordered_is_red(self):
        def other(placed):
            changed = [dict(placed[0], knobs={"horizon": "all", "keep_recent": 9})] + placed[1:]
            return self.windows(changed, "file"), 0

        self.reply = other
        self.assertEqual(self.capture("--roles", self.roles_file(ROLES)), 1)

    def test_a_file_the_test_refuses_is_no_capture(self):
        # The test reads the file and fails on it (unreadable, bad horizon):
        # it prints no window, the run is red -- never the defaults.
        self.reply = lambda placed: (["thread panicked: curator_window_roles.json: "
                                      "the roles do not read\n"], 100)
        self.assertEqual(self.capture("--roles", self.roles_file(ROLES)), 2)
        self.assertFalse(self.placed.exists())

    def test_roles_that_do_not_read_run_nothing(self):
        for roles in ([], [{"name": "Role_a", "set": "consult"}],
                      [{"name": "a", "set": "role_b"}],
                      [{"name": "a", "set": "consult"}, {"name": "a", "set": "talky"}],
                      [{"name": "a", "set": "consult", "knobs": ["horizon"]}],
                      [{"name": "a", "set": "consult", "horizon": "all"}],
                      [{"name": "a", "set": "consult", "knobs": {"horizon": "tasks"}}],
                      [{"name": "a", "set": "consult", "knobs": {"horizon": None}}]):
            with self.subTest(roles=roles):
                self.assertEqual(self.capture("--roles", self.roles_file(roles)), 2)
        self.assertEqual(self.capture("--roles", str(self.dir / "missing.json")), 2)
        self.assertEqual(curator_bank.main(["capture", "--out", str(self.dir / "o"),
                                            "--root", str(self.root), "--roles",
                                            self.roles_file([{"name": "a", "set": "consult"}])]),
                         2)
        self.assertEqual(self.seen, [])
        self.assertFalse(self.placed.exists())

    def test_the_roles_are_removed_when_the_run_breaks(self):
        def broken(root, log_path, window=False):
            raise RuntimeError("the kit broke")

        curator_bank.run_kit = broken
        with self.assertRaisesRegex(RuntimeError, "^the kit broke$"):
            self.capture("--roles", self.roles_file([{"name": "a", "set": "consult"}]))
        self.assertFalse(self.placed.exists())


# The roles of the pins: neutral names, one at the horizon `all`.
ROLES = [{"name": "role_a", "set": "consult",
          "knobs": {"horizon": "all", "keep_recent": 2, "window_target": 24000}},
         {"name": "role_b", "set": "talky", "knobs": {}}]
# The defaults of the window test (`default_roles` there).
DEFAULT_ROLES = [{"name": "task", "set": "consult", "knobs": {}},
                 {"name": "all", "set": "consult",
                  "knobs": {"horizon": "all", "keep_recent": 2, "window_target": 24000}},
                 {"name": "day", "set": "talky", "knobs": {}}]


if __name__ == "__main__":
    unittest.main()
