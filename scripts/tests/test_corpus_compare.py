"""`scripts/corpus_compare.py` judges a recorded window corpus against the old one.

Pinned here against fixtures made up in a throwaway git tree: an unchanged
corpus is GREEN; a changed `system`, `messages` or `tool_scope` is RED and
can never be freed; a route, a hop key or a mark kind differs only RED until
the free list names it; the order within a step is free, the bytes are not;
a step that is gone or new differs; a case that is gone differs in every
step; a free list without a reason is refused (exit 2); and the recording
command is the strand kit's single-test verb with the corpus test's filter.
"""

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest

from scripts import corpus_compare
from scripts.tests import store_isolation


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


def call(route="brain", system=None, messages=None, hop=None, scope=None):
    body = {"messages": messages if messages is not None else [{"origin": "user", "text": "hi"}],
            "system": system if system is not None else {"mode": {"text": "Be brief."}}}
    if scope is not None:
        body["tool_scope"] = scope
    return {"route": route, "hop": hop if hop is not None else {"iter": "0"}, "body": body}


def mark(kind, value):
    return {"kind": kind, "session_id": "s1", "turn_id": "t1",
            "audience_set": "[\"member:e\"]", "value": json.dumps(value, sort_keys=True)}


def case(steps):
    return {"case": "c", "steps": steps}


def step(label, out=(), marks=(), summarizer=()):
    return {"step": label, "out": list(out), "marks": list(marks),
            "summarizer": list(summarizer)}


BASE = {
    "brain_text": case([
        step("01 say t1", [call()], [mark("curate", {"call": "c1", "at": "10:00:00.000033Z"})]),
        step("02 reply brain stop", [call("turn_write", hop={"turn_id": "t1"})],
             [mark("escalate", {"from": "quick", "to": "brain"})]),
    ]),
    "quick_text": case([
        step("01 say t1", [call("quick", scope={"allow": ["web"]}),
                           call("quick", hop={"iter": "0", "curator_call": "c2-bridge"})]),
    ]),
}


class Tree:
    """A git tree whose HEAD holds `BASE` as the old fixtures."""

    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.dir = os.path.join(self.root, corpus_compare.FIXTURES)
        os.makedirs(self.dir)
        self.git("init", "-q", "-b", "master")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "test")
        self.write(BASE)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "base")

    def git(self, *args):
        subprocess.run(["git", "-C", self.root] + list(args), check=True,
                       capture_output=True)

    def write(self, fixtures):
        for name in os.listdir(self.dir):
            os.remove(os.path.join(self.dir, name))
        for name, doc in fixtures.items():
            with open(os.path.join(self.dir, name + ".json"), "w", encoding="utf-8") as fh:
                json.dump(doc, fh, indent=2)

    def free(self, entries):
        path = os.path.join(self.root, "free.json")
        with open(path, "w", encoding="utf-8") as fh:
            json.dump({"free": entries}, fh)
        return path

    def run(self, free=None, base="HEAD"):
        out = io.StringIO()
        err = io.StringIO()
        argv = ["compare", "--root", self.root, "--base", base]
        if free:
            argv += ["--free", free]
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = corpus_compare.main(argv)
        return code, out.getvalue().splitlines(), err.getvalue()


def changed(f):
    doc = json.loads(json.dumps(BASE))
    f(doc)
    return doc


class CorpusCompareTest(unittest.TestCase):
    def setUp(self):
        self.t = Tree()

    def tearDown(self):
        self.t.tmp.cleanup()

    def test_an_unchanged_corpus_is_green(self):
        code, lines, _ = self.t.run()
        self.assertEqual(code, 0, lines)
        self.assertEqual(lines[-1], "CORPUS 3 steps 0 differ GREEN")

    def test_the_default_base_is_the_merge_base_of_master(self):
        self.t.git("checkout", "-q", "-b", "strand")
        code, lines, err = self.t.run(base="")
        self.assertEqual(code, 0, (lines, err))
        self.assertEqual(lines[-1], "CORPUS 3 steps 0 differ GREEN")

    def test_a_changed_system_part_is_red_and_named(self):
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["out"][0]["body"]
                             ["system"].update({"mode": {"text": "Be long."}})))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertEqual(lines[-1], "CORPUS 3 steps 1 differ RED")
        diff = [x for x in lines if x.startswith("DIFF ")]
        self.assertEqual(len(diff), 1, lines)
        self.assertIn("brain_text 01 say t1", diff[0])
        self.assertIn("body.system", diff[0])

    def test_messages_and_tool_scope_are_compared_too(self):
        self.t.write(changed(lambda d: (
            d["brain_text"]["steps"][0]["out"][0]["body"].update(
                {"messages": [{"origin": "user", "text": "hi!"}]}),
            d["quick_text"]["steps"][0]["out"][0]["body"].update(
                {"tool_scope": {"allow": []}}))))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertEqual(lines[-1], "CORPUS 3 steps 2 differ RED")
        text = "\n".join(lines)
        self.assertIn("body.messages", text)
        self.assertIn("body.tool_scope", text)

    def test_the_locked_body_parts_can_never_be_freed(self):
        for part in ("system", "messages", "tool_scope"):
            free = self.t.free([{"field": "body." + part, "why": "a strand wants it"}])
            code, _, err = self.t.run(free)
            self.assertEqual(code, 2, part)
            self.assertIn("may never be freed", err)

    def test_a_route_differs_until_the_free_list_names_it(self):
        self.t.write(changed(lambda d: [o.update({"route": "model"})
                                        for o in d["quick_text"]["steps"][0]["out"]]))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertIn("route", [x for x in lines if x.startswith("DIFF")][0])
        free = self.t.free([{"field": "route", "why": "one route for every model call"}])
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 0, lines)

    def test_a_new_hop_key_differs_until_freed(self):
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["out"][0]["hop"]
                             .update({"target": "brain"})))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertIn("hop.target", "\n".join(lines))
        code, lines, _ = self.t.run(self.t.free([{"field": "hop.target", "why": "the label"}]))
        self.assertEqual(code, 0, lines)

    def test_a_mark_kind_differs_until_freed_and_other_kinds_stay_strict(self):
        def f(d):
            d["brain_text"]["steps"][1]["marks"][0] = mark("escalate", {"from": "a", "to": "b"})
            d["brain_text"]["steps"][1]["marks"].append(mark("target", {"missing": "profile"}))
        self.t.write(changed(f))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertIn("mark differs", "\n".join(lines))
        free = self.t.free([{"mark": "escalate", "why": "wording"},
                            {"mark": "target", "why": "a new mark"}])
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 0, lines)
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["marks"][0].update(
            {"value": "{}"})))
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 1, lines)

    def test_the_order_within_a_step_is_free_the_bytes_are_not(self):
        self.t.write(changed(lambda d: d["quick_text"]["steps"][0]["out"].reverse()))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 0, lines)

    def test_the_microseconds_below_the_millisecond_are_masked(self):
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["marks"][0].update(
            {"value": json.dumps({"call": "c1", "at": "10:00:00.000041Z"}, sort_keys=True)})))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 0, lines)
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["marks"][0].update(
            {"value": json.dumps({"call": "c1", "at": "10:00:00.001033Z"}, sort_keys=True)})))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1, lines)

    def test_a_gone_or_new_step_differs_and_a_relabel_is_a_note(self):
        def f(d):
            d["brain_text"]["steps"].pop()
            d["quick_text"]["steps"][0]["step"] = "01 say t1 again"
        self.t.write(changed(f))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertEqual(lines[-1], "CORPUS 3 steps 1 differ RED")
        self.assertTrue(any(x.startswith("NOTE quick_text") for x in lines), lines)
        self.assertTrue(any("the step is gone" in x for x in lines), lines)

    def test_a_gone_case_differs_in_every_step_a_new_case_is_a_note(self):
        doc = changed(lambda d: d.pop("brain_text"))
        doc["fresh"] = case([step("01 say t1", [call()])])
        self.t.write(doc)
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1)
        self.assertEqual(lines[-1], "CORPUS 3 steps 2 differ RED")
        self.assertTrue(any(x.startswith("NOTE fresh") for x in lines), lines)

    def test_a_free_list_entry_without_a_reason_or_target_is_refused(self):
        for entries in ([{"field": "route"}], [{"field": "route", "why": " "}],
                        [{"field": "nope", "why": "x"}],
                        [{"field": "route", "mark": "escalate", "why": "x"}]):
            code, _, err = self.t.run(self.t.free(entries))
            self.assertEqual(code, 2, entries)
            self.assertTrue(err.startswith("corpus_compare: "), err)

    def test_an_extra_message_of_a_freed_route_passes_and_nothing_else(self):
        ask = {"route": "catalog_ask", "hop": {"session_id": "s1"},
               "body": {"system": {}, "params": {}}}
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["out"].append(ask)))
        code, lines, _ = self.t.run()
        self.assertEqual(code, 1, lines)
        self.assertIn("out new now", "\n".join(lines))
        free = self.t.free([{"extra": "catalog_ask", "without": ["hop.catalog_for"],
                             "why": "the ask to all beside the call"}])
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 0, lines)
        self.assertEqual(lines[-1], "CORPUS 3 steps 0 differ GREEN")
        # the same route with the key the entry excludes stays red
        aimed = json.loads(json.dumps(ask))
        aimed["hop"]["catalog_for"] = "x"
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["out"].append(aimed)))
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 1, lines)
        # another extra route stays red
        other = dict(ask, route="model")
        self.t.write(changed(lambda d: d["brain_text"]["steps"][0]["out"].append(other)))
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 1, lines)
        # a missing message stays red, even of the freed route
        base = json.loads(json.dumps(BASE))
        base["brain_text"]["steps"][0]["out"].append(ask)
        self.t.write(base)
        self.t.git("add", "-A")
        self.t.git("commit", "-q", "-m", "ask")
        self.t.write(BASE)
        code, lines, _ = self.t.run(free)
        self.assertEqual(code, 1, lines)
        self.assertIn("out missing now", "\n".join(lines))

    def test_an_extra_entry_needs_a_route_and_fields_it_excludes(self):
        for entries in ([{"extra": "", "why": "x"}],
                        [{"extra": "catalog_ask", "without": "hop.catalog_for", "why": "x"}],
                        [{"extra": "catalog_ask", "without": ["nope"], "why": "x"}],
                        [{"extra": "catalog_ask"}]):
            code, _, err = self.t.run(self.t.free(entries))
            self.assertEqual(code, 2, entries)
            self.assertTrue(err.startswith("corpus_compare: "), err)

    def test_a_bad_revision_is_exit_2(self):
        code, _, err = self.t.run(base="no-such-rev")
        self.assertEqual(code, 2)
        self.assertIn("git", err)

    def test_recording_runs_the_corpus_test_through_the_kit(self):
        cmd = corpus_compare.record_command("/tree")
        self.assertEqual(cmd[:4], ["/tree/scripts/strand.sh", "-C", "/tree", "test"])
        self.assertEqual(cmd[4], "binary(/curator_/) & test(/%s/)" % corpus_compare.TEST)
        self.assertEqual(corpus_compare.BLESS, "MECLAW_D38_C0_BLESS")


if __name__ == "__main__":
    unittest.main()
