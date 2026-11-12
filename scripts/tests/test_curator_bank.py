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


if __name__ == "__main__":
    unittest.main()
