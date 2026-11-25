"""The params doors of sealed hives (`scripts/hive_doors.py`, GH #1353).

A params-only update reaches a cell inside a sealed hive through the lane
`in_params`: the hive declares it at its path, one inner edge per overlay
cell and per inner seal carries it on by `hop.subscriber`, and one way back
per target carries the cell's answer out. An `accepts` row without an inner
edge makes the next mutation of a colony fail (`judge_door`), so the tool
writes both halves or neither, and `--check` refuses a tree that has one.

Every case builds a small template tree; none reads the repository's own.
"""

import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(SCRIPTS.parent))

import hive_doors  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def hive(sealed=True, accepts=(), emits=(), edges=()):
    params = {"ports": []} if sealed else {}
    params["contract"] = {"accepts": [{"route": r, "because": "b"} for r in accepts],
                          "emits": [{"route": r, "because": "b"} for r in emits]}
    params["graph"] = {"edges": list(edges)}
    return {"cell": {"type": "hive"}, "params": params}


def cell(kind):
    return {"cell": {"type": kind}, "params": {}}


def ref(name):
    return {"cell": {"type": "ref", "template": name}}


class Tree(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = os.path.join(tmp.name, "templates")
        self.other = os.path.join(tmp.name, "other")
        os.makedirs(self.root)
        os.makedirs(self.other)

    def put(self, rel, doc, root=None):
        path = os.path.join(root or self.root, rel, "config.json")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(doc, indent=2, ensure_ascii=False) + "\n")

    def doc(self, rel, root=None):
        with open(os.path.join(root or self.root, rel, "config.json"), encoding="utf-8") as fh:
            return json.load(fh)

    def bytes(self):
        out = {}
        for base, _, files in os.walk(os.path.dirname(self.root)):
            for name in files:
                path = os.path.join(base, name)
                with open(path, "rb") as fh:
                    out[path] = fh.read()
        return out

    def run_tool(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = hive_doors.main(["--root", self.root, *args])
        return code, out.getvalue() + err.getvalue()

    def write_all(self, *args):
        code, text = self.run_tool(*args)
        self.assertEqual(code, 0, text)

    def lanes(self, rel, side):
        return [row["route"] for row in self.doc(rel)["params"]["contract"][side]]

    def edges(self, rel, **want):
        return [e for e in self.doc(rel)["params"]["graph"]["edges"]
                if all(e.get(k) == v for k, v in want.items())]

    def doors(self, rel):
        return {e["to"]: e["condition"] for e in self.edges(rel, **{"from": "."})}

    def ways_back(self, rel):
        return sorted(e["from"] for e in self.edges(rel, to="."))


class Door(Tree):
    WAY_BACK = [
        {"from": "./store", "to": ".",
         "condition": "has(hop.params_answer) && hop.params_answer == '%s'" % lane,
         "modifier": {"set_hop": {"route": "'%s'" % lane}}}
        for lane in ("params_applied", "params_refused")]

    def test_a_sealed_hive_gets_the_lane_the_edge_and_the_way_back(self):
        self.put("a", hive())
        self.put("a/store", cell("store"))
        self.put("a/hand", cell("code"))
        self.write_all()
        self.assertEqual(self.lanes("a", "accepts"), ["in_params"])
        self.assertNotIn("at", self.doc("a")["params"]["contract"]["accepts"][0])
        self.assertEqual(self.lanes("a", "emits"), ["params_applied", "params_refused"])
        self.assertEqual(self.doors("a"), {
            "./store": "has(hop.route) && hop.route == 'in_params' && has(hop.subscriber)"
                       " && hop.subscriber.endsWith('/store')"})
        # The answer of a cell carries no route (`params_overlay.rs`): one edge per
        # answer names its lane as a constant, so the contract can read the exit.
        self.assertEqual(self.edges("a", to="."), self.WAY_BACK)

    def test_the_answer_leaves_without_the_working_keys_the_other_exits_take(self):
        # `development-rules.md` section 8c: a hive deletes its own working keys
        # on its exits (gh823, gh1316, gh494). What most exits take is such a key;
        # one that a single exit takes is correlation of its road and rides on.
        def out(sender, *keys):
            return {"from": sender, "to": ".", "condition": "has(hop.route) && hop.route == 'x'",
                    "modifier": {"delete_context": list(keys)}}

        self.put("a", hive(edges=[out("./hand", "phase", "carry"),
                                  out("./hand", "carry", "phase", "turn_id"),
                                  out("./hand", "carry")]))
        self.put("a/hand", cell("code"))
        self.put("a/store", cell("store"))
        self.put("a/inner", hive())
        self.put("a/inner/clock", cell("timer"))
        self.write_all()
        drop = {"delete_context": ["phase", "carry"]}
        backs = [e for e in self.edges("a", to=".") if hive_doors.is_way_back(e)]
        self.assertEqual([e["from"] for e in backs], ["./inner", "./store", "./store"])
        self.assertEqual(backs[0]["modifier"], drop)
        self.assertEqual([e["modifier"] for e in backs[1:]], [
            {"set_hop": {"route": "'params_applied'"}, **drop},
            {"set_hop": {"route": "'params_refused'"}, **drop}])
        code, text = self.run_tool("--check")
        self.assertEqual(code, 0, text)

    def test_every_overlay_type_gets_a_door_and_no_other(self):
        self.put("a", hive())
        kinds = sorted(hive_doors.OVERLAY_TYPES)
        self.assertEqual(kinds, ["harness", "llm", "mcp", "proxy", "store", "subcolony",
                                 "timer", "voice", "web"])
        for kind in kinds + ["code", "file", "vault", "bash"]:
            self.put("a/c_" + kind, cell(kind))
        self.write_all()
        self.assertEqual(sorted(self.doors("a")), ["./c_" + k for k in kinds])

    def test_an_open_hive_and_a_hive_without_an_overlay_cell_stay_as_they_are(self):
        self.put("open", hive(sealed=False))
        self.put("open/store", cell("store"))
        self.put("plain", hive())
        self.put("plain/hand", cell("code"))
        before = self.bytes()
        self.write_all()
        self.assertEqual(self.bytes(), before)

    def test_a_cell_below_an_open_hive_is_reached_from_the_seal(self):
        self.put("a", hive())
        self.put("a/u", hive(sealed=False))
        self.put("a/u/clock", cell("timer"))
        self.write_all()
        self.assertIn("endsWith('/u/clock')", self.doors("a")["./u/clock"])
        self.assertEqual(self.lanes("a/u", "accepts"), [])

    def test_the_lane_stands_after_the_model_lane(self):
        self.put("a", hive(accepts=["in_x", "in_model", "in_y"]))
        self.put("a/brain", cell("llm"))
        self.write_all()
        self.assertEqual(self.lanes("a", "accepts"), ["in_x", "in_model", "in_params", "in_y"])

    def test_the_text_of_a_lane_that_stands_is_kept_and_its_at_is_taken(self):
        doc = hive(accepts=["in_params"])
        doc["params"]["contract"]["accepts"][0]["at"] = ["store"]
        self.put("a", doc)
        self.put("a/store", cell("store"))
        self.write_all()
        self.assertEqual(self.doc("a")["params"]["contract"]["accepts"],
                         [{"route": "in_params", "because": "b"}])

    def test_an_edge_of_the_same_lane_in_another_form_is_not_touched(self):
        relay = {"from": ".", "to": "./relay", "modifier": {"set_hop": {"x": "'1'"}},
                 "condition": "has(hop.route) && hop.route == 'in_params'"}
        self.put("a", hive(edges=[relay]))
        self.put("a/relay", cell("code"))
        self.put("a/store", cell("store"))
        self.write_all()
        self.assertIn(relay, self.edges("a"))


class Nested(Tree):
    def test_an_inner_seal_gets_its_own_door_and_one_edge_from_outside(self):
        self.put("inner", hive())
        self.put("inner/clock", cell("timer"))
        self.put("outer", hive())
        self.put("outer/box", ref("inner@1.0.0"))
        self.write_all()
        self.assertEqual(sorted(self.doors("inner")), ["./clock"])
        self.assertEqual(self.doors("outer"), {
            "./box": "has(hop.route) && hop.route == 'in_params' && has(hop.subscriber)"
                     " && hop.subscriber.matches('/box/[^/]+$')"})
        # ... and the answer of the inner hive carries its lane already
        self.assertEqual(self.edges("outer", to="."), [{
            "from": "./box", "to": ".",
            "condition": "has(hop.route) && (hop.route == 'params_applied'"
                         " || hop.route == 'params_refused')"}])
        self.assertEqual(self.lanes("outer", "emits"), ["params_applied", "params_refused"])

    def test_two_seals_deep_the_condition_names_the_hives_between(self):
        self.put("leaf", hive())
        self.put("leaf/clock", cell("timer"))
        self.put("mid", hive())
        self.put("mid/brain", cell("llm"))
        self.put("mid/leaf", ref("leaf@1.0.0"))
        self.put("top", hive())
        self.put("top/mid", ref("mid@1.0.0"))
        self.write_all()
        self.assertIn("matches('/mid/(leaf/)?[^/]+$')", self.doors("top")["./mid"])
        # a seal whose cells all lie below an inner seal: the part is not optional
        self.put("thin", hive())
        self.put("thin/leaf", ref("leaf@1.0.0"))
        self.put("top2", hive())
        self.put("top2/thin", ref("thin@1.0.0"))
        self.write_all()
        self.assertIn("matches('/thin/(leaf/)[^/]+$')", self.doors("top2")["./thin"])

    def test_a_cell_of_the_same_name_below_an_inner_seal_is_excluded(self):
        self.put("inner", hive())
        self.put("inner/clock", cell("timer"))
        self.put("outer", hive())
        self.put("outer/clock", cell("timer"))
        self.put("outer/box", ref("inner@1.0.0"))
        self.write_all()
        self.assertTrue(self.doors("outer")["./clock"].endswith(
            "hop.subscriber.endsWith('/clock') && !hop.subscriber.endsWith('/box/clock')"))

    def test_a_frozen_version_is_neither_written_nor_wired(self):
        self.put("inner@1.0.0", hive())
        self.put("inner@1.0.0/clock", cell("timer"))
        self.put("inner", hive())
        self.put("inner/clock", cell("timer"))
        self.put("outer", hive())
        self.put("outer/box", ref("inner@1.0.0"))
        self.write_all()
        self.assertEqual(self.lanes("inner@1.0.0", "accepts"), [])
        self.assertEqual(self.lanes("outer", "accepts"), [])

    def test_a_template_of_another_root_is_read_and_never_written(self):
        self.put("inner", hive(), root=self.other)
        self.put("inner/clock", cell("timer"), root=self.other)
        self.put("outer", hive())
        self.put("outer/box", ref("inner@1.0.0"))
        self.put("outer/store", cell("store"))
        self.write_all("--lib", self.other)
        self.assertEqual(sorted(self.doors("outer")), ["./store"])  # no door in there yet
        self.assertEqual(self.doc("inner", self.other), hive())
        door = hive(accepts=["in_params"])
        self.put("inner", door, root=self.other)
        self.write_all("--lib", self.other)
        self.assertEqual(sorted(self.doors("outer")), ["./box", "./store"])
        self.assertEqual(self.doc("inner", self.other), door)

    def test_a_reference_nobody_provides_is_refused_before_anything_is_written(self):
        self.put("a", hive())
        self.put("a/store", cell("store"))
        self.put("b", hive())
        self.put("b/box", ref("gone@1.0.0"))
        before = self.bytes()
        code, text = self.run_tool()
        self.assertEqual(code, 2)
        self.assertIn("gone@1.0.0", text)
        self.assertEqual(self.bytes(), before)

    def test_a_class_on_the_exception_list_gets_no_door(self):
        self.assertTrue(hive_doors.EXCEPTIONS["voice2vision"])
        self.put("voice2vision", hive())
        self.put("voice2vision/store", cell("store"))
        self.put("outer", hive())
        self.put("outer/v", ref("voice2vision@1.0.0"))
        before = self.bytes()
        code, text = self.run_tool()
        self.assertEqual((code, self.bytes()), (0, before))
        self.assertIn(hive_doors.EXCEPTIONS["voice2vision"], text)
        self.assertEqual(self.run_tool("--check")[0], 0)


class Guard(Tree):
    """The answer leaves on the way back and nowhere else.

    Every edge of a sender that matches fires (`apply_edges` fans out). A cell
    whose other out-edge names no lane would hand `params_applied` to its
    neighbour as if it were a tick or a row.
    """

    ANSWERS = "!has(hop.params_answer)"

    def put_edge(self, edge, kind="timer"):
        self.put("a", hive(edges=[edge]))
        self.put("a/x", cell(kind))
        self.put("a/hand", cell("code"))
        self.write_all()
        return self.edges("a")[0]

    def test_an_edge_without_a_condition_is_closed_to_the_answer(self):
        got = self.put_edge({"from": "./x", "to": "./hand", "modifier": {"set_hop": {"route": "'in_t'"}}})
        self.assertEqual(got["condition"], self.ANSWERS)
        self.assertEqual(list(got), ["from", "to", "condition", "modifier"])

    def test_an_edge_that_names_no_lane_is_closed_to_the_answer(self):
        got = self.put_edge({"from": "./x", "to": "./hand",
                             "condition": "!(has(hop.route) && hop.route == 'credential_request')"})
        self.assertTrue(got["condition"].endswith(" && " + self.ANSWERS))

    def test_an_alternative_is_put_in_parentheses_first(self):
        got = self.put_edge({"from": "./x", "to": "./hand",
                             "condition": "has(context.a) || !has(hop.route)"})
        self.assertEqual(got["condition"],
                         "(has(context.a) || !has(hop.route)) && " + self.ANSWERS)

    def test_the_refusal_of_a_model_package_does_not_take_a_refused_update(self):
        got = self.put_edge({"from": "./x", "to": ".", "condition": "has(hop.refused_subscriber)",
                             "modifier": {"set_hop": {"route": "'model_refused'"}}}, kind="llm")
        self.assertEqual(got["condition"], "has(hop.refused_subscriber) && " + self.ANSWERS)

    def test_an_edge_that_names_another_lane_stays_as_it_is(self):
        for condition in ("has(hop.route) && hop.route == 'tick'",
                          "hop.route in ['tick', 'tock'] && has(context.a)",
                          "has(hop.route) && (hop.route == 'a' || hop.route == 'b') && context.n == '1'",
                          "has(hop.turn_tag) && hop.turn_tag != ''",
                          # a key the hive sets inside itself for its own round: an update
                          # that entered by the door carries none
                          "context.store_origin == 'gate'",
                          # the answer of an update is neither a tick nor an output
                          "has(hop.msg_type) && hop.msg_type == 'timer_op_error'",
                          "has(hop.finish_reason) && hop.finish_reason == 'stop'",
                          "has(hop.finish_reason) && !has(hop.refused_subscriber)"):
            edge = {"from": "./x", "to": "./hand", "condition": condition}
            self.assertEqual(self.put_edge(dict(edge)), edge)

    def test_a_default_edge_of_a_cell_yields_to_the_way_back(self):
        edge = {"from": "./x", "to": "./hand", "default": True}
        self.assertEqual(self.put_edge(dict(edge)), edge)

    def test_the_door_lane_enters_by_the_door_only(self):
        got = self.put_edge({"from": ".", "to": "./hand",
                             "condition": "has(hop.route) && hop.route.startsWith('in_')"})
        self.assertEqual(got["condition"],
                         "has(hop.route) && hop.route.startsWith('in_') && "
                         "!(has(hop.route) && hop.route == 'in_params')")

    def test_an_edge_of_the_hive_that_takes_everything_is_closed_to_all_three(self):
        got = self.put_edge({"from": ".", "to": "./hand"})
        self.assertEqual(got["condition"],
                         "!(has(hop.route) && hop.route == 'in_params') && " + self.ANSWERS)

    def test_the_answer_of_an_inner_seal_carries_its_lane_into_the_outer_graph(self):
        self.put("inner", hive())
        self.put("inner/clock", cell("timer"))
        self.put("outer", hive(edges=[{"from": "./box", "to": "./hand",
                                       "condition": "hop.route != 'tick'"},
                                      {"from": "./box", "to": "./hand",
                                       "condition": "!has(hop.route)"}]))
        self.put("outer/box", ref("inner@1.0.0"))
        self.put("outer/hand", cell("code"))
        self.write_all()
        self.assertEqual([e["condition"] for e in self.edges("outer", **{"from": "./box"})][:2],
                         ["hop.route != 'tick' && " + self.ANSWERS, "!has(hop.route)"])

    def test_a_condition_the_tool_cannot_read_is_closed(self):
        got = self.put_edge({"from": "./x", "to": "./hand", "condition": "foo(hop.route) @ 3"})
        self.assertTrue(got["condition"].endswith(self.ANSWERS))


class Check(Tree):
    def standing(self):
        self.put("inner", hive())
        self.put("inner/clock", cell("timer"))
        self.put("a", hive(edges=[{"from": "./store", "to": "./hand"}]))
        self.put("a/store", cell("store"))
        self.put("a/hand", cell("code"))
        self.put("a/box", ref("inner@1.0.0"))
        self.write_all()

    def test_check_is_red_on_a_tree_without_doors_and_writes_nothing(self):
        self.put("a", hive())
        self.put("a/store", cell("store"))
        before = self.bytes()
        code, text = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn("a/config.json", text)
        self.assertEqual(self.bytes(), before)

    def test_check_is_green_after_the_run_and_a_second_run_writes_nothing(self):
        self.standing()
        before = self.bytes()
        self.assertEqual(self.run_tool("--check")[0], 0)
        self.write_all()
        self.assertEqual(self.bytes(), before)

    def test_a_dry_run_names_every_level_and_writes_nothing(self):
        self.put("a", hive())
        self.put("a/store", cell("store"))
        before = self.bytes()
        code, text = self.run_tool("--dry-run")
        self.assertEqual(code, 0)
        self.assertRegex(text, r"a: 1 cell\(s\), 0 seal\(s\), 2 way\(s\) back")
        self.assertEqual(self.bytes(), before)

    def test_every_missing_half_is_red(self):
        def drop(side, route):
            def go(params):
                rows = params["contract"][side]
                rows[:] = [r for r in rows if r["route"] != route]
            return go

        def drop_edge(match):
            def go(params):
                kept = [e for e in params["graph"]["edges"] if not match(e)]
                self.assertNotEqual(kept, params["graph"]["edges"])
                params["graph"]["edges"] = kept
            return go

        for name, change in (
                ("the door without its inner edge",
                 drop_edge(lambda e: e["from"] == "." and e["to"] == "./store")),
                ("the door without the edge to the inner seal",
                 drop_edge(lambda e: e["from"] == "." and e["to"] == "./box")),
                ("the inner edge without the door", drop("accepts", "in_params")),
                ("the target without a way back",
                 drop_edge(lambda e: e["from"] == "./store" and e["to"] == ".")),
                ("the refusal without a way back",
                 drop_edge(lambda e: e["from"] == "./store" and "refused" in e["condition"])),
                ("the answer lane undeclared", drop("emits", "params_refused"))):
            with self.subTest(name):
                self.standing()
                doc = self.doc("a")
                change(doc["params"])
                self.put("a", doc)
                self.assertEqual(self.run_tool("--check")[0], 1)
                self.write_all()
                self.assertEqual(self.run_tool("--check")[0], 0)

    def test_an_open_edge_of_a_target_is_red(self):
        self.standing()
        doc = self.doc("a")
        doc["params"]["graph"]["edges"].append({"from": "./box", "to": "./hand"})
        self.put("a", doc)
        self.assertEqual(self.run_tool("--check")[0], 1)

    def test_a_door_whose_last_cell_is_gone_is_taken_down(self):
        self.put("a", hive())
        self.put("a/store", cell("store"))
        self.write_all()
        self.put("a/store", cell("code"))
        self.assertEqual(self.run_tool("--check")[0], 1)
        self.write_all()
        self.assertEqual((self.lanes("a", "accepts"), self.lanes("a", "emits"), self.edges("a")),
                         ([], [], []))

    def test_an_appended_edge_does_not_move_the_door(self):
        self.standing()
        doc = self.doc("a")
        doc["params"]["graph"]["edges"].append(
            {"from": "./hand", "to": "./store", "condition": "has(hop.route) && hop.route == 'q'"})
        self.put("a", doc)
        self.assertEqual(self.run_tool("--check")[0], 0)

    def test_a_config_the_tool_cannot_write_back_is_refused(self):
        path = os.path.join(self.root, "a", "config.json")
        self.put("a", hive())
        self.put("a/store", cell("store"))
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(hive(), indent=3))
        before = self.bytes()
        code, text = self.run_tool()
        self.assertEqual(code, 2)
        self.assertIn("a/config.json", text)
        self.assertEqual(self.bytes(), before)


if __name__ == "__main__":
    unittest.main()
