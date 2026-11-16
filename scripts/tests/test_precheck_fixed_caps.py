"""Pins of the precheck check `fixed-caps` (`scripts/fixed_caps.py`, GH #1532).

A fixed window, budget or rate value in product code is right for one model
only; the catalog row sizes the window. The check holds the stock in a
baseline and lets a value through only when a rule of
`scripts/fixed_caps_rules.json` justifies it by the structure around it. A
new fixed value -- the mutant of these pins -- is RED with its file and line.

Every pin builds a throw-away tree with the real rules file of this checkout
and runs the precheck itself (`precheck.run`) or the module over it. The
fixtures are Python strings.
"""

import contextlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

from scripts.tests import store_isolation

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import fixed_caps
import precheck

CFG = "templates/demo/pack/config.json"
RS = "crates/meclaw-demo/src/window.rs"

STOCK_SCRIPT = ("import json, sys\n"
                "doc = json.load(sys.stdin)\n"
                "SECTION_BUDGET = 4000\n"
                "print(json.dumps({'budget': SECTION_BUDGET}))\n")

V2_BLOCK = ("# >>> content-budget v2 (the target's rate)\n"
            "CHARS_PER_TOKEN = 3\n"
            "\n"
            "\n"
            "def chars_per_token_or_core(rate):\n"
            "    try:\n"
            "        r = float(rate) if rate not in (None, '') else 0.0\n"
            "    except (TypeError, ValueError):\n"
            "        r = 0.0\n"
            "    return r if r > 0 else CHARS_PER_TOKEN\n"
            "# <<< content-budget v2\n")

V2_CALL = ("doc = {}\n"
           "rate = chars_per_token_or_core(doc.get('chars_per_token'))\n")


def config(script, params=None):
    doc = {"type": "code", "params": dict(params or {}, runner="python3", script_inline=script)}
    return json.dumps(doc, indent=2) + "\n"


def script_key_line(text):
    return next(i for i, ln in enumerate(text.split("\n"), 1) if '"script_inline"' in ln)


class Tree:
    """A throw-away tree with the real rules file of this checkout."""

    def __init__(self, case, files, rules=True):
        tmp = tempfile.TemporaryDirectory()
        case.addCleanup(tmp.cleanup)
        self.root = os.path.realpath(tmp.name)
        if rules:
            dst = os.path.join(self.root, fixed_caps.RULES_FILE)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(REPO / fixed_caps.RULES_FILE, dst)
        for rel, text in files.items():
            self.write(rel, text)

    def write(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)

    def red(self, files):
        """The RED findings of check `fixed-caps` the precheck reports for `files`."""
        return [f for f in precheck.run(files, repo=self.root)
                if f.check == "fixed-caps" and f.level == "RED"]


GIT = ["git", "-c", "user.name=pin", "-c", "user.email=pin@example.invalid", "-c", "commit.gpgsign=false",
       "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=master"]


class GitTree(Tree):
    """A throw-away git repository: `master` carries the base, the work goes on
    a branch `strand` -- the merge base the precheck grades against."""

    def __init__(self, case, files, rules=True):
        super().__init__(case, files, rules=rules)
        self.git("init", "-q")
        self.commit("base")
        self.git("checkout", "-q", "-b", "strand")

    def git(self, *args):
        subprocess.run(GIT + ["-C", self.root] + list(args), check=True, capture_output=True, text=True)

    def commit(self, msg):
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", msg)


class BaselineAndMutant(unittest.TestCase):

    def test_the_stock_held_by_the_baseline_is_green(self):
        tree = Tree(self, {CFG: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        self.assertEqual([], tree.red([CFG]))
        # without the baseline the same stock is a finding: the baseline holds it
        os.remove(os.path.join(tree.root, fixed_caps.BASELINE_FILE))
        self.assertEqual(1, len(tree.red([CFG])))

    def test_a_new_fixed_number_is_red_with_file_and_line(self):
        tree = Tree(self, {CFG: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        mutant = STOCK_SCRIPT + "CONTEXT_WINDOW_TOKENS = 32000\n"
        text = config(mutant)
        tree.write(CFG, text)
        red = tree.red([CFG])
        self.assertEqual(1, len(red), red)
        msg = red[0].message
        self.assertIn("%s:%d (script line 5)" % (CFG, script_key_line(text)), msg)
        self.assertIn("CONTEXT_WINDOW_TOKENS = 32000", msg)
        self.assertIn("catalog row", msg)
        self.assertIn(fixed_caps.RULES_FILE, msg)

    def test_one_more_occurrence_than_the_baseline_holds_is_red(self):
        tree = Tree(self, {CFG: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        tree.write(CFG, config(STOCK_SCRIPT + "OTHER = 1\nSECTION_BUDGET = 4000\n"))
        red = tree.red([CFG])
        self.assertTrue(red)
        self.assertTrue(all("2 here, the baseline holds 1" in f.message for f in red), red)

    def test_a_json_param_and_a_rust_constant_are_graded_too(self):
        tree = Tree(self, {CFG: config("print(1)\n", {"window_tokens": 64000}),
                           RS: "pub const WINDOW_TOKENS: usize = 128_000;\n"
                               "#[cfg(test)]\n"
                               "mod tests {\n"
                               "    const TEST_WINDOW_TOKENS: usize = 9_000;\n"
                               "}\n"})
        red = tree.red([CFG, RS])
        self.assertEqual(2, len(red), red)
        self.assertTrue(any(f.message.startswith("%s:1: WINDOW_TOKENS = 128000" % RS) for f in red), red)
        self.assertTrue(any("window_tokens = 64000" in f.message for f in red), red)

    def test_a_constant_after_a_test_module_is_red(self):
        # only the test module is blind; the scan resumes after its closing brace
        tree = Tree(self, {RS: "#[cfg(test)]\n"
                               "mod tests {\n"
                               "    fn f() { let x = 1; }\n"
                               "    const TEST_WINDOW_TOKENS: usize = 9_000;\n"
                               "}\n"
                               "pub const PROBE_CONTEXT_WINDOW: usize = 128_000;\n"})
        red = tree.red([RS])
        self.assertEqual(1, len(red), red)
        self.assertTrue(red[0].message.startswith("%s:6: PROBE_CONTEXT_WINDOW = 128000" % RS), red)

    def test_a_test_module_declaration_blinds_nothing(self):
        tree = Tree(self, {RS: "#[cfg(test)]\n"
                               "mod tests;\n"
                               "pub const WINDOW_TOKENS: usize = 128_000;\n"})
        red = tree.red([RS])
        self.assertEqual(1, len(red), red)
        self.assertIn("%s:3: WINDOW_TOKENS" % RS, red[0].message)

    def test_a_test_item_is_blind_to_its_end_only(self):
        tree = Tree(self, {RS: "#[cfg(test)]\n"
                               "#[allow(dead_code)]\n"
                               "fn probe() -> [u8; 4]\n"
                               "{\n"
                               "    let s = \"}\";\n"
                               "    let window_tokens = 9_000;\n"
                               "}\n"
                               "#[cfg(test)] const TEST_WINDOW_TOKENS: usize = 7_000;\n"
                               "pub fn f() { let window_tokens = 64_000; }\n"})
        red = tree.red([RS])
        self.assertEqual(1, len(red), red)
        self.assertIn("%s:9: window_tokens = 64000" % RS, red[0].message)

    def test_a_rust_constant_over_two_lines_is_red(self):
        tree = Tree(self, {RS: "pub const WINDOW_TOKENS: usize =\n"
                               "    128_000;\n"})
        red = tree.red([RS])
        self.assertEqual(1, len(red), red)
        self.assertIn("%s:1: WINDOW_TOKENS = 128000" % RS, red[0].message)

    def test_a_time_word_elsewhere_on_the_line_hides_no_window(self):
        tree = Tree(self, {RS: "fn f() { let mut v = Vec::with_capacity(8); let window_tokens = 64_000; }\n",
                           CFG: config("snippet_size = 32000  # sent before the timeout\n"
                                       "headroom = 24000  # see the report\n")})
        red = tree.red([RS, CFG])
        self.assertEqual(3, len(red), red)
        self.assertTrue(any("window_tokens = 64000" in f.message for f in red), red)
        self.assertTrue(any("snippet_size = 32000" in f.message for f in red), red)
        self.assertTrue(any("headroom = 24000" in f.message for f in red), red)
        # the time words themselves still hold: a duration is no window
        quiet = Tree(self, {RS: "fn f() { let budget = Duration::from_secs(3_600); }\n",
                            CFG: config("import time\nt_end = time.monotonic() + 6000  # window refresh\n")})
        self.assertEqual([], quiet.red([RS, CFG]))

    def test_a_generic_limit_without_a_window_word_is_no_finding(self):
        tree = Tree(self, {CFG: config("ROW_LIMIT = 5000\nPARTS_MAX = 2000\nSEQ_MAX = 1000\n")})
        self.assertEqual([], tree.red([CFG]))
        hinted = Tree(self, {CFG: config("ROW_LIMIT = 5000  # tokens of the prompt\n")})
        self.assertEqual(1, len(hinted.red([CFG])))

    def test_the_model_words_name_a_window(self):
        tree = Tree(self, {CFG: config("MAX_INPUT = 200000\nmax_output = 4096\nPROMPT_LIMIT = 100000\n"
                                       "input_limit = 32000\nmax_prompt_len = 8000\n",
                                       {"max_input": 32000}),
                           RS: "const MAX_OUTPUT: u32 = 4096;\n"})
        red = [f.message for f in tree.red([CFG, RS])]
        self.assertEqual(7, len(red), red)
        for shown in ("MAX_INPUT = 200000", "max_output = 4096", "PROMPT_LIMIT = 100000", "input_limit = 32000",
                      "max_prompt_len = 8000", "max_input = 32000", "MAX_OUTPUT = 4096"):
            self.assertTrue(any(shown in m for m in red), (shown, red))
        # the words count whole: `throughput` holds no `output`
        quiet = Tree(self, {CFG: config("THROUGHPUT_MAX = 5000\nINPUTS_SEEN = 5000\n")})
        self.assertEqual([], quiet.red([CFG]))

    def test_a_small_budget_counts_under_a_budget_name_only(self):
        tree = Tree(self, {CFG: config("keep_recent = 40\nDEFAULT_BUDGET = 200\n",
                                       {"window_profile": {"recall_budget": 200, "keep_recent": 40}}),
                           RS: "const SECTION_TOKENS: usize = 64;\n"})
        red = [f.message for f in tree.red([CFG, RS])]
        self.assertEqual(5, len(red), red)
        for shown in ("keep_recent = 40", "DEFAULT_BUDGET = 200", "recall_budget = 200", "SECTION_TOKENS = 64"):
            self.assertTrue(any(shown in m for m in red), (shown, red))
        # under another name a small number stays quiet, the floor of 500 holds there
        quiet = Tree(self, {CFG: config("import secrets\nCODE_MAX_CHARS = 64\nBIND_WINDOW = 32\nkeep_recent = 4\n"
                                        "op_id = secrets.token_hex(16)\n"),
                            RS: "const TAG_MAX_CHARS: usize = 128;\n"})
        self.assertEqual([], quiet.red([CFG, RS]))

    def test_shifted_parsed_and_nested_values_are_red(self):
        tree = Tree(self, {CFG: config("WINDOW_TOKENS = 1 << 15\nCONTEXT_SIZE = int('32000')\n"
                                       "WINDOW_CEILING = float('inf')\n",
                                       {"context": {"size": 32000}})})
        red = [f.message for f in tree.red([CFG])]
        self.assertEqual(3, len(red), red)
        self.assertTrue(any("WINDOW_TOKENS = 32768" in m for m in red), red)
        self.assertTrue(any("CONTEXT_SIZE = 32000" in m for m in red), red)
        self.assertTrue(any("context.size = 32000" in m for m in red), red)

    def test_scenarios_and_tests_under_templates_are_not_read(self):
        files = {"templates/demo/compose/scenarios/run.py": "WINDOW_TOKENS = 32000\n",
                 "templates/demo/tests/check.py": "WINDOW_TOKENS = 32000\n",
                 "templates/demo/pack/test_pack.py": "WINDOW_TOKENS = 32000\n"}
        tree = Tree(self, files)
        self.assertEqual([], tree.red(list(files)))
        self.assertEqual([], [p for p in files if fixed_caps.in_scope(p)])

    def test_a_path_outside_the_scope_is_not_read(self):
        tree = Tree(self, {"workshop/tools/x.py": "WINDOW_TOKENS = 32000\n",
                           "docs/x.json": '{"window_tokens": 64000}\n'})
        self.assertEqual([], tree.red(["workshop/tools/x.py", "docs/x.json"]))


class Rules(unittest.TestCase):

    def test_the_v2_block_rate_is_justified(self):
        tree = Tree(self, {CFG: config(V2_BLOCK + V2_CALL)})
        self.assertEqual([], tree.red([CFG]))

    def test_the_same_three_outside_the_v2_block_is_red(self):
        # the block reads the rate, but the 3 stands above it
        tree = Tree(self, {CFG: config("CHARS_PER_TOKEN = 3\n" + V2_BLOCK.replace("CHARS_PER_TOKEN = 3\n", "")
                                       + V2_CALL)})
        red = tree.red([CFG])
        self.assertEqual(1, len(red), red)
        self.assertIn("(script line 1): CHARS_PER_TOKEN = 3 is a fixed rate value", red[0].message)

    def test_the_v2_block_without_a_read_of_the_rate_is_red(self):
        tree = Tree(self, {CFG: config(V2_BLOCK + "print(1)\n")})
        red = tree.red([CFG])
        self.assertEqual(1, len(red), red)
        self.assertIn("CHARS_PER_TOKEN = 3", red[0].message)

    def test_another_rate_inside_the_v2_block_is_red(self):
        tree = Tree(self, {CFG: config(V2_BLOCK.replace("CHARS_PER_TOKEN = 3", "CHARS_PER_TOKEN = 4") + V2_CALL)})
        self.assertEqual(1, len(tree.red([CFG])))

    def test_the_image_cost_is_justified_only_where_the_cell_reads_the_row(self):
        base = "IMAGE_TOKENS = 1024\n"
        reads = Tree(self, {CFG: config(base + "doc = {}\nk = doc.get('tokens_per_image')\n")})
        self.assertEqual([], reads.red([CFG]))
        blind = Tree(self, {CFG: config(base + "doc = {}\n")})
        self.assertEqual(1, len(blind.red([CFG])))

    def test_the_milli_unit_of_rate_tokens_is_justified_and_a_real_cap_is_red(self):
        # OR-583-95: `rate_tokens` counts in thousandths of a character at the
        # rate the cell read; the 1000 is that unit. A real cap under the same
        # key pattern -- another value, or the same function in a cell that
        # reads no rate -- stays red.
        unit = ("def rate_tokens(text, rate):\n"
                "    milli = max(1, int(rate * 1000 + 0.5))\n"
                "    return -(-len(str(text or '')) * 1000 // milli)\n")
        reads = "doc = {}\nrate = doc.get('chars_per_token')\n"
        green = Tree(self, {CFG: config(unit + reads)})
        self.assertEqual([], green.red([CFG]))
        cap = unit.replace("* 1000 //", "* 4000 //")
        red = Tree(self, {CFG: config(cap + reads)}).red([CFG])
        self.assertEqual(1, len(red), red)
        self.assertIn("return:rate_tokens = 4000", red[0].message)
        blind = Tree(self, {CFG: config(unit + "doc = {}\n")}).red([CFG])
        self.assertEqual(1, len(blind), blind)
        self.assertIn("return:rate_tokens = 1000", blind[0].message)

    def test_the_seed_budget_is_justified_only_beside_the_share(self):
        shared = Tree(self, {CFG: config("print(1)\n", {"seed_budget": 8192, "seed_share": 0.032768})})
        self.assertEqual([], shared.red([CFG]))
        alone = Tree(self, {CFG: config("print(1)\n", {"seed_budget": 8192})})
        self.assertEqual(1, len(alone.red([CFG])))

    def test_the_input_fallbacks_are_justified_at_their_ruled_values(self):
        ruled = Tree(self, {CFG: config("print(1)\n", {"input_soft_fallback": 17500,
                                                       "embed_input_soft_fallback": 2048})})
        self.assertEqual([], ruled.red([CFG]))
        # the declared default of the param is the param's value: the same rule holds it
        declared = Tree(self, {CFG: json.dumps({"type": "code", "contract": {"params": {
            "input_soft_fallback": {"type": "integer", "default": 17500}}}}, indent=2) + "\n"})
        self.assertEqual([], declared.red([CFG]))
        self.assertEqual(1, len(Tree(self, {CFG: json.dumps({"contract": {"params": {
            "input_soft_fallback": {"default": 25000}}}}, indent=2) + "\n"}).red([CFG])))
        other = Tree(self, {CFG: config("print(1)\n", {"input_soft_fallback": 25000})})
        self.assertEqual(1, len(other.red([CFG])))

    def test_the_v2_block_must_define_the_rate_reader(self):
        # the function stands outside the block: the block does not define it
        outside = V2_BLOCK.replace("# <<< content-budget v2\n", "").replace(
            "\n\ndef chars_per_token_or_core", "\n# <<< content-budget v2\n\ndef chars_per_token_or_core")
        tree = Tree(self, {CFG: config(outside + V2_CALL)})
        red = tree.red([CFG])
        self.assertEqual(1, len(red), red)
        self.assertIn("CHARS_PER_TOKEN = 3", red[0].message)

    def test_a_call_inside_the_definition_is_no_read_of_the_rate(self):
        recursive = V2_BLOCK.replace("    return r if r > 0 else CHARS_PER_TOKEN\n",
                                     "    return r if r > 0 else chars_per_token_or_core(CHARS_PER_TOKEN)\n")
        tree = Tree(self, {CFG: config(recursive + "print(1)\n")})
        self.assertEqual(1, len(tree.red([CFG])))

    def test_a_rule_holds_only_its_kind_and_needs_values(self):
        rules = {"about": "pin", "rules": [
            {"id": "num-only", "names": "^FOO_PER_TOKEN$", "kind": "NUM", "values": [4],
             "reason": "a pin rule that names the kind it justifies"},
            {"id": "no-values", "names": "^BAR_TOKENS$",
             "reason": "a pin rule without values justifies no value at all"}]}
        tree = Tree(self, {fixed_caps.RULES_FILE: json.dumps(rules),
                           CFG: config("FOO_PER_TOKEN = 4\nBAR_TOKENS = 32000\n")}, rules=False)
        red = [f.message for f in tree.red([CFG])]
        self.assertEqual(2, len(red), red)
        self.assertTrue(any("FOO_PER_TOKEN = 4 is a fixed rate value" in m for m in red), red)
        self.assertTrue(any("BAR_TOKENS = 32000" in m for m in red), red)

    def test_every_rule_names_a_reason_values_and_a_valid_pattern(self):
        rules = fixed_caps.load_rules(str(REPO))
        self.assertGreaterEqual(len(rules), 5)
        for r in rules:
            with self.subTest(rule=r.get("id")):
                self.assertTrue(r["id"])
                self.assertGreater(len(r["reason"].split()), 5)
                self.assertTrue(r["names"].startswith("^") and r["names"].endswith("$"))
                # GH #1544: a catalog rule allows the row's value, never a list
                self.assertTrue(bool(r.get("values")) != bool(r.get("catalog")),
                                "a rule names its values or its catalog field, exactly one")


CATALOG = fixed_caps.CATALOG_FILE


def catalog(rows):
    """A `models.jsonl`: the schema line, then one row a line."""
    return "\n".join([json.dumps({"schema": {"model_id": "text", "input_soft": "int"}})]
                     + [json.dumps(r) for r in rows]) + "\n"


ROWS = [{"model_id": "vendor/model-1.5", "input_soft": 25000, "package": {"chars_per_token": 4}},
        {"model_id": "other/big", "input_soft": 200000}]
RENDERED = {"input_soft_fallback": {"vendor/model-1.5": 25000, "other/big": 200000}}


def catalog_rules():
    """A rules file with only the catalog rule of the core (a fixture of its own:
    a pin reads nothing of the tree, it travels with the export)."""
    return {"about": "pin", "rules": [
        {"id": "input-soft-fallback-rendered", "names": "^input_soft_fallback\\..+$", "catalog": "input_soft",
         "reason": "a pin rule: each rendered value is justified while it is its catalog row's input_soft"}]}

class RenderedFromTheCatalog(unittest.TestCase):
    """GH #1544: a map a render script writes from the catalog
    (`input_soft_fallback` {model_id: input_soft}) is justified exactly while
    each value is the catalog row's -- a current render is green, a stale one
    or a model without a row is red."""

    def setUp(self):
        # the environment never answers a tree that carries its own catalog;
        # these pins run without it unless they set it
        old = os.environ.pop(fixed_caps.CATALOG_ENV, None)
        if old is not None:
            self.addCleanup(os.environ.__setitem__, fixed_caps.CATALOG_ENV, old)

    def test_a_current_render_is_green(self):
        tree = Tree(self, {CATALOG: catalog(ROWS), CFG: config("print(1)\n", RENDERED)})
        self.assertEqual([], tree.red([CFG]))

    def test_a_stale_render_is_red_with_its_model(self):
        moved = [dict(ROWS[0], input_soft=30000), ROWS[1]]
        tree = Tree(self, {CATALOG: catalog(moved), CFG: config("print(1)\n", RENDERED)})
        red = [f.message for f in tree.red([CFG])]
        self.assertEqual(1, len(red), red)
        self.assertIn("input_soft_fallback.vendor/model-1.5 = 25000", red[0])

    def test_a_model_without_a_row_is_red(self):
        tree = Tree(self, {CATALOG: catalog(ROWS[1:]), CFG: config("print(1)\n", RENDERED)})
        red = [f.message for f in tree.red([CFG])]
        self.assertEqual(1, len(red), red)
        self.assertIn("vendor/model-1.5", red[0])

    def test_without_a_catalog_the_render_is_red(self):
        tree = Tree(self, {CFG: config("print(1)\n", RENDERED)})
        self.assertEqual(2, len(tree.red([CFG])))

    def test_a_catalog_given_by_option_or_env_is_read(self):
        """A repository whose catalog comes from a release asset names it:
        `--catalog <file>` on the command line (also for `--list` and
        `--write-baseline`), `FIXED_CAPS_CATALOG` for the precheck."""
        tree = Tree(self, {CFG: config("print(1)\n", RENDERED)})
        elsewhere = os.path.join(tree.root, "asset", "models.jsonl")
        tree.write("asset/models.jsonl", catalog(ROWS))
        self.assertEqual(1, fixed_caps.main(["--repo", tree.root]))
        self.assertEqual(0, fixed_caps.main(["--repo", tree.root, "--catalog", elsewhere]))
        listed = io.StringIO()
        with contextlib.redirect_stdout(listed):
            fixed_caps.main(["--repo", tree.root, "--list", "--catalog", elsewhere])
        self.assertEqual(2, listed.getvalue().count("rule:input-soft-fallback-rendered"), listed.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            fixed_caps.main(["--repo", tree.root, "--write-baseline", "--catalog", elsewhere])
        self.assertEqual({}, fixed_caps.load_baseline(tree.root))
        os.environ[fixed_caps.CATALOG_ENV] = elsewhere
        self.addCleanup(os.environ.pop, fixed_caps.CATALOG_ENV, None)
        self.assertEqual([], tree.red([CFG]))

    def test_the_trees_own_catalog_wins_over_the_env(self):
        """The environment answers only a tree without a catalog: a stale render
        of the core is red whatever `FIXED_CAPS_CATALOG` names."""
        moved = [dict(ROWS[0], input_soft=30000), ROWS[1]]
        tree = Tree(self, {CATALOG: catalog(moved), CFG: config("print(1)\n", RENDERED)})
        tree.write("asset/models.jsonl", catalog(ROWS))
        os.environ[fixed_caps.CATALOG_ENV] = os.path.join(tree.root, "asset", "models.jsonl")
        self.addCleanup(os.environ.pop, fixed_caps.CATALOG_ENV, None)
        self.assertEqual(1, len(tree.red([CFG])))

    def test_a_diff_at_the_catalog_alone_grades_the_renders(self):
        """A new row value can stale a render: the catalog is one of the check's
        own files, its diff grades the whole scope."""
        tree = Tree(self, {CATALOG: catalog(ROWS), CFG: config("print(1)\n", RENDERED)})
        self.assertEqual([], tree.red([CATALOG]))
        tree.write(CATALOG, catalog([dict(ROWS[0], input_soft=30000), ROWS[1]]))
        red = [f.message for f in tree.red([CATALOG])]
        self.assertEqual(1, len(red), red)
        self.assertIn("vendor/model-1.5", red[0])

    def test_the_rules_file_names_the_repositorys_catalog(self):
        """GH #1548: a repository built on a release has no core catalog in its
        tree and no environment on a lane; its rules file names a catalog
        snapshot it carries. A current render is green, a stale one red, and the
        environment does not override the named file."""
        rules = catalog_rules()
        rules["catalog_file"] = "scripts/catalog_snapshot.jsonl"
        files = {fixed_caps.RULES_FILE: json.dumps(rules), "scripts/catalog_snapshot.jsonl": catalog(ROWS),
                 CFG: config("print(1)\n", RENDERED)}
        tree = Tree(self, files, rules=False)
        self.assertEqual([], tree.red([CFG]))
        tree.write("asset/models.jsonl", catalog(ROWS))
        tree.write("scripts/catalog_snapshot.jsonl", catalog([dict(ROWS[0], input_soft=30000), ROWS[1]]))
        os.environ[fixed_caps.CATALOG_ENV] = os.path.join(tree.root, "asset", "models.jsonl")
        self.addCleanup(os.environ.pop, fixed_caps.CATALOG_ENV, None)
        red = [f.message for f in tree.red([CFG])]
        self.assertEqual(1, len(red), red)
        self.assertIn("vendor/model-1.5", red[0])

    def test_a_diff_at_the_named_catalog_alone_grades_the_renders(self):
        rules = catalog_rules()
        rules["catalog_file"] = "scripts/catalog_snapshot.jsonl"
        tree = Tree(self, {fixed_caps.RULES_FILE: json.dumps(rules), "scripts/catalog_snapshot.jsonl": catalog(ROWS),
                           CFG: config("print(1)\n", RENDERED)}, rules=False)
        self.assertEqual([], tree.red(["scripts/catalog_snapshot.jsonl"]))
        tree.write("scripts/catalog_snapshot.jsonl", catalog([dict(ROWS[0], input_soft=30000), ROWS[1]]))
        self.assertEqual(1, len(tree.red(["scripts/catalog_snapshot.jsonl"])))

    def _named(self, value, snapshot=None):
        rules = catalog_rules()
        rules["catalog_file"] = value
        files = {fixed_caps.RULES_FILE: json.dumps(rules), CFG: config("print(1)\n", RENDERED)}
        if snapshot:
            files[snapshot] = catalog(ROWS)
        return Tree(self, files, rules=False)

    def test_a_named_catalog_is_one_path_however_written(self):
        """Review I1: `./scripts/x` is read as `scripts/x`, and a diff at it
        alone grades the renders (red when it stales them)."""
        tree = self._named("./scripts//catalog_snapshot.jsonl", "scripts/catalog_snapshot.jsonl")
        self.assertEqual([], tree.red([CFG]))
        tree.write("scripts/catalog_snapshot.jsonl", catalog([dict(ROWS[0], input_soft=30000), ROWS[1]]))
        self.assertEqual(1, len(tree.red(["scripts/catalog_snapshot.jsonl"])))

    def test_an_invalid_catalog_file_fails_closed(self):
        """Review I2: absolute, outside the repository, empty or no string: the
        rules file is reported RED and no other catalog (env, tree) answers."""
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        good = os.path.join(outside.name, "models.jsonl")
        with open(good, "w", encoding="utf-8") as fh:
            fh.write(catalog(ROWS))
        os.environ[fixed_caps.CATALOG_ENV] = good
        self.addCleanup(os.environ.pop, fixed_caps.CATALOG_ENV, None)
        for value in (good, "..", "", 123, ["scripts/x.jsonl"]):
            with self.subTest(value=value):
                tree = self._named(value)
                if value == "..":                      # a real catalog one level above the repository root
                    name = "%s-models.jsonl" % os.path.basename(tree.root)     # unique beside the tree
                    above = os.path.join(os.path.dirname(tree.root), name)
                    with open(above, "w", encoding="utf-8") as fh:
                        fh.write(catalog(ROWS))
                    self.addCleanup(os.remove, above)
                    rules = catalog_rules()
                    rules["catalog_file"] = "../" + name
                    tree.write(fixed_caps.RULES_FILE, json.dumps(rules))
                red = [f.message for f in tree.red([CFG])]
                self.assertTrue(any("invalid catalog_file" in m for m in red), red)
                self.assertEqual(3, len(red), red)          # the rules file and both rendered values

    def test_an_unreadable_rules_file_is_red_not_an_exception(self):
        for text in ("{not json", json.dumps({"rules": [{"id": "x", "names": "^(", "values": [1], "reason": "r"}]})):
            with self.subTest(text=text):
                tree = Tree(self, {fixed_caps.RULES_FILE: text, CFG: config("print(1)\n", RENDERED)}, rules=False)
                red = [f.message for f in tree.red([CFG])]
                self.assertEqual(1, len(red), red)
                self.assertIn("unreadable", red[0])

    def test_a_catalog_rule_holds_only_its_field(self):
        rules = {"about": "pin", "rules": [
            {"id": "hard-by-model", "names": "^input_soft_fallback\\..+$", "catalog": "input_hard",
             "reason": "a pin rule naming another field of the row than the map holds"}]}
        rows = [dict(r, input_hard=r["input_soft"] * 2) for r in ROWS]
        tree = Tree(self, {fixed_caps.RULES_FILE: json.dumps(rules), CATALOG: catalog(rows),
                           CFG: config("print(1)\n", RENDERED)}, rules=False)
        self.assertEqual(2, len(tree.red([CFG])))
        tree.write(CFG, config("print(1)\n", {"input_soft_fallback": {"vendor/model-1.5": 50000,
                                                                       "other/big": 400000}}))
        self.assertEqual([], tree.red([CFG]))


class WholeScope(unittest.TestCase):

    def test_a_diff_at_the_rules_grades_the_whole_scope_and_notes_a_stale_entry(self):
        tree = Tree(self, {CFG: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        # the value leaves the tree; a diff at the rules sees the stale entry
        tree.write(CFG, config("print(1)\n"))
        found = [f for f in precheck.run([fixed_caps.RULES_FILE], repo=tree.root) if f.check == "fixed-caps"]
        self.assertEqual(["NOTE"], [f.level for f in found], found)
        self.assertIn("SECTION_BUDGET", found[0].message)
        # a new value elsewhere in the scope is RED on that diff, though the file is not in it
        tree.write(RS, "pub const WINDOW_TOKENS: usize = 128_000;\n")
        self.assertEqual(1, len(tree.red([fixed_caps.RULES_FILE])))

    def test_the_precheck_runs_the_check(self):
        self.assertIn("check_fixed_caps", [c.__name__ for c in precheck.NATIVE_CHECKS])

    def test_this_tree_is_green(self):
        found = fixed_caps.grade_tree(str(REPO))
        red = [f.message for f in found if f.level == "RED"]
        self.assertEqual([], red, "\n".join(red))


class AgainstTheMergeBase(unittest.TestCase):

    LIVE = "templates/demo/pack/config.json"
    COPY = "templates/demo@1.0.0/pack/config.json"

    def test_a_frozen_template_copy_is_stock(self):
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        tree.commit("baseline")
        tree.git("branch", "-f", "master", "HEAD")
        tree.write(self.COPY, config(STOCK_SCRIPT))
        tree.commit("frozen copy")
        self.assertEqual([], tree.red([self.COPY]))
        whole = fixed_caps.grade_tree(tree.root)
        self.assertEqual([], whole, whole)
        # a copy with a changed line is no copy: its values are new
        tree.write(self.COPY, config(STOCK_SCRIPT + "OTHER = 1\n"))
        tree.commit("changed copy")
        self.assertEqual(1, len(tree.red([self.COPY])))
        # back to the frozen copy; the rewritten baseline holds it: no growth, no stale entry
        tree.write(self.COPY, config(STOCK_SCRIPT))
        tree.commit("frozen copy again")
        fixed_caps.write_baseline(tree.root)
        self.assertIn(self.COPY + "#script|SECTION_BUDGET|4000", fixed_caps.load_baseline(tree.root))
        self.assertEqual([], tree.red([self.COPY, fixed_caps.BASELINE_FILE]))
        whole = fixed_caps.grade_tree(tree.root)
        self.assertEqual([], whole, whole)

    def test_a_grown_baseline_is_red_with_its_entry(self):
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        tree.commit("baseline")
        tree.git("branch", "-f", "master", "HEAD")
        # the mutant: a new value plus a rewrite of the baseline in the same diff
        tree.write(self.LIVE, config(STOCK_SCRIPT + "CONTEXT_WINDOW_TOKENS = 32000\n"))
        fixed_caps.write_baseline(tree.root)
        red = tree.red([self.LIVE, fixed_caps.BASELINE_FILE])
        self.assertEqual(1, len(red), red)
        self.assertIn("the baseline grew", red[0].message)
        self.assertIn("%s#script|CONTEXT_WINDOW_TOKENS|32000" % self.LIVE, red[0].message)
        self.assertEqual(1, len([f for f in fixed_caps.grade_tree(tree.root) if f.level == "RED"]))

    def test_a_shrunk_baseline_is_free(self):
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT + "CONTEXT_WINDOW_TOKENS = 32000\n")})
        fixed_caps.write_baseline(tree.root)
        tree.commit("baseline")
        tree.git("branch", "-f", "master", "HEAD")
        tree.write(self.LIVE, config(STOCK_SCRIPT))
        fixed_caps.write_baseline(tree.root)
        self.assertEqual([], tree.red([self.LIVE, fixed_caps.BASELINE_FILE]))

    def test_an_unreadable_base_baseline_is_red(self):
        # fail-closed: what the base holds cannot be read, so no growth is green
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT), fixed_caps.BASELINE_FILE: "{not json\n"})
        fixed_caps.write_baseline(tree.root)
        red = tree.red([fixed_caps.BASELINE_FILE])
        self.assertEqual(1, len(red), red)
        self.assertIn("cannot read %s at the merge base" % fixed_caps.BASELINE_FILE, red[0].message)

    def test_an_unreadable_baseline_of_the_tree_is_red_not_an_exception(self):
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        tree.commit("baseline")
        tree.git("checkout", "-q", "master")
        tree.git("merge", "-q", "--ff-only", "strand")
        tree.git("checkout", "-q", "strand")
        tree.write(fixed_caps.BASELINE_FILE, "{not json\n")
        red = [f.message for f in tree.red([fixed_caps.BASELINE_FILE])]
        self.assertTrue(red and all("unreadable" in m for m in red), red)

    def test_a_first_baseline_has_no_ratchet(self):
        # the merge base carries no baseline yet: the new one is the stock
        tree = GitTree(self, {self.LIVE: config(STOCK_SCRIPT)})
        fixed_caps.write_baseline(tree.root)
        self.assertEqual([], tree.red([self.LIVE, fixed_caps.BASELINE_FILE]))


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
