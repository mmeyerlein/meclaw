"""The browser binaries leave `tests` for `browser:display` (GH #1161, re-check M-3).

`MECLAW_REQUIRE_BROWSERS` hangs on the engine booking, and only `browser:display`
books the engines. A test binary that starts a browser in the station `tests`
ran unbooked and without the switch: a lane without the engine left it
SKIP-green -- in strand, integration and release alike (gh703, the WebKit phone,
among eight). The rule: every test unit that starts a browser (stem
`*_browser`, or a driver `workshop/tools/display-*-browser.mjs` named in code
that starts a process) is subtracted from `tests` and planned into
`browser:display` whenever the diff's selection reaches it; one that the
station cannot take (an `#[ignore]`d proof outside the named locks, a shared
module that names a driver) is RED in `precheck` (`browser-station-lock`).
"""

import os
import pathlib
import re
import sys
import tempfile
import unittest
import unittest.mock

from scripts.tests import private_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import gate_plan as gp  # noqa: E402
from scripts.pipeline import locks, plan as pl  # noqa: E402

GH703 = "binary_id(=meclaw-cells::gh703_the_phone_holds_in_webkit_browser)"
GH703_FILE = "crates/meclaw-cells/tests/gh703_the_phone_holds_in_webkit_browser.rs"
SHEET = "binary_id(=meclaw-cells::710_the_sheet_holds_in_both_engines_browser)"
CELLS_SRC = "crates/meclaw-cells/src/lib.rs"

# The eight of the re-check, by stem -- plus the three the content marker finds.
EIGHT = ("gh643_audio_in_the_display_window_browser",
         "gh695_the_scene_ticks_and_chimes_browser",
         "gh703_the_phone_holds_in_webkit_browser",
         "gh767_the_drivers_tap_reaches_the_curator_browser",
         "gh767_the_page_canvas_draws_and_sends_browser",
         "gh772_a_refusal_after_the_start_ends_the_browser",
         "gh958_card_and_steps_render_in_both_engines_browser",
         "gh964_a_map_draws_tiles_only_from_the_setting_browser")
BY_MARKER = ("binary_id(=meclaw-cells::744_a_late_patch_does_not_take_the_drawing_back)",
             "(binary_id(=meclaw-cells::misc_6) & test(/^gh703_the_chat_shows_its_newest_line::/))",
             "(binary_id(=meclaw-cells::misc_6) & "
             "test(/^gh704_a_refused_channel_does_not_take_the_dock::/))")


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


def by_name(stations):
    return {s.name: s for s in stations}


def browser_filters(st):
    return [c[2] for c in st.cmds if c[:2] == ["scripts/test-tier.sh", "filter"]]


def webkit_part(st):
    return [f for f in browser_filters(st) if f.endswith("& %s" % gp.WEBKIT_TESTS)]


# The browser units travel with their drivers under `workshop/tools/`, and the
# public tree carries neither (`plans/export-fixtures/make_export.py`): the cases
# on the real tree skip there, the fixture trees run everywhere.
DRIVERS = REPO / "workshop" / "tools"


@private_tree.private("workshop/tools")
class TheRealTree(unittest.TestCase):

    def test_the_eight_and_the_marker_three_are_browser_units(self):
        units = set(gp.browser_terms())
        for stem in EIGHT:
            self.assertIn("binary_id(=meclaw-cells::%s)" % stem, units, stem)
        for term in BY_MARKER:
            self.assertIn(term, units)
        # a driver named only to READ its text starts no browser (720/722), and a
        # driver in a doc comment is no call (misc_6 gh703 tap/hold)
        for stem in ("720_the_marks_refusal_survives_a_patch",
                     "722_the_refusal_is_visible_to_a_person"):
            self.assertNotIn("binary_id(=meclaw-cells::%s)" % stem, units)
        self.assertFalse(any("gh703_the_mark_knows_a_tap_from_a_hold" in t for t in units))
        # GH #1189: the four gh766 modules that read MECLAW_CHROMIUM
        for stem in ("every_page_of_one_browser_draws", "the_ninth_page_is_refused_by_name",
                     "the_packaged_browser_answers_for_itself", "two_viewers_share_one_stream"):
            self.assertIn("(binary_id(=meclaw-cells::misc_6) & test(/^gh766_%s::/))" % stem,
                          units, stem)

    def test_gh703_lands_in_the_browser_station_not_in_tests(self):
        for mode in ("strand", "integration", "release"):
            st = by_name(gp.plan([GH703_FILE], mode))
            if "tests" in st:
                self.assertNotIn(GH703, st["tests"].cmds[0][2].split(" - ")[0], mode)
            self.assertIn("browser:display", st, mode)
            # the WebKit part runs it: its one test is `the_sheet_behaves_in_webkit`
            self.assertTrue(any(GH703 in f for f in webkit_part(st["browser:display"])), mode)
        # the station books both engines -- the switch and `engineAbsent` apply
        self.assertEqual(pl.NEEDS["browser:display"]["browser_webkit"], 1)
        self.assertEqual(pl.NEEDS["browser:display"]["browser_chromium"], 1)

    def test_a_narrowed_integration_keeps_the_moved_units(self):
        # `browser_plan.apply` cuts the locks with `keep`; a reached unit outside
        # BROWSER_LOCKS stays, in the one call of a narrowed plan (tb-s3 restack
        # onto tb-s1b)
        moved = gp.browser_moved([GH703_FILE], "integration")
        self.assertTrue(any(GH703 in t for t in moved), moved)
        self.assertEqual([["scripts/test-tier.sh", "filter", " + ".join(sorted(moved)),
                           "--run-ignored", "all"]],
                         gp.browser_cmds(True, keep=set(), moved=moved))
        self.assertEqual(gp.browser_cmds(True, moved=moved),
                         gp.browser_cmds(True, keep=set(gp.BROWSER_LOCKS), moved=moved))

    def test_a_crate_wide_selection_subtracts_every_browser_unit(self):
        for mode in ("strand", "integration", "release"):
            st = by_name(gp.plan([CELLS_SRC], mode))
            expr = st["tests"].cmds[0][2]
            head, _, minus = expr.partition(" - ")
            # a release plans the full suite whatever the diff (KR-TB-1 d, GH #1162)
            # and subtracts every browser unit all the same (rule 11)
            self.assertIn("all()" if mode == "release" else "rdeps(meclaw-cells)", head, mode)
            for term in gp.browser_terms():
                self.assertIn(term, minus, (mode, term))
            union = " ".join(browser_filters(st["browser:display"]))
            for stem in EIGHT:
                self.assertIn("binary_id(=meclaw-cells::%s)" % stem, union, (mode, stem))
            for term in BY_MARKER:
                self.assertIn(term, union, mode)
            # the sheet ran twice before: in `tests` unbooked and in the station
            self.assertIn(SHEET, minus, mode)

    def test_the_station_builds_what_it_runs(self):
        st = by_name(gp.plan([CELLS_SRC], "strand"))["browser:display"]
        extra = gp._browser_select(st)["extra"]
        self.assertIn("meclaw-cells::gh703_the_phone_holds_in_webkit_browser", extra)
        self.assertIn("meclaw-cells::misc_6", extra)

    def test_a_diff_that_reaches_no_browser_unit_moves_nothing(self):
        st = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], "strand"))
        # meclaw-cells depends on meclaw-core: rdeps reaches the browser units
        self.assertIn("browser:display", st)
        st = by_name(gp.plan(["crates/meclaw-api/src/lib.rs"], "strand"))
        self.assertNotIn("browser:display", st)
        self.assertNotIn("_browser", st["tests"].cmds[0][2].partition(" - ")[0])

    def test_no_subtracted_browser_unit_runs_nowhere(self):
        """The generic guard (tb-s3-b): a browser unit `tests` subtracts while
        one of its own summands reaches it runs in `browser:display` -- the
        resolver's plans; the narrowing's in test_pipeline_narrow."""
        from scripts.pipeline import narrow
        diffs = ([CELLS_SRC], [GH703_FILE], ["crates/meclaw-core/src/lib.rs"],
                 ["crates/meclaw-api/src/lib.rs"], [CELLS_SRC, "crates/meclaw-core/src/lib.rs"])
        for mode in ("strand", "integration", "release"):
            for diff in diffs:
                rows = [{"name": s.name, "cmds": s.cmds,
                         "select": {"filter": s.cmds[0][2]} if s.name == "tests" else {}}
                        for s in gp.plan(diff, mode)]
                self.assertEqual([], narrow.browser_orphans(rows, None), (mode, diff))

    def test_ci_keeps_them_in_tests(self):
        st = by_name(gp.plan([GH703_FILE], "ci"))
        self.assertIn(GH703, st["tests"].cmds[0][2])
        self.assertNotIn("browser:display", st)

    def test_the_narrowing_subtracts_them_too(self):
        minus = gp.tests_minus_for()
        for term in gp.browser_terms():
            self.assertIn(term, minus)
        src = (REPO / "scripts" / "pipeline" / "impact.py").read_text(encoding="utf-8")
        self.assertIn("gate_plan.tests_minus_for(repo)", src)

    def test_every_webkit_test_of_a_browser_unit_takes_turns(self):
        """The WebKit part runs its tests one at a time only for the `webkit`
        group; the moved units bring WebKit tests of their own (gh958, gh964),
        which must not meet another WebKit page (GH #1048)."""
        from scripts.tests import test_nextest_groups as ng
        import tomllib
        config = tomllib.loads((REPO / ".config" / "nextest.toml").read_text(encoding="utf-8"))
        rules = ng.group_rules(config, "default")
        seen = set()
        for term, (crate, unit, text) in gp.browser_units().items():
            if "/" in unit:
                continue
            for name in ng.listed_test_names(text):
                if "webkit" in name:
                    seen.add(name)
                    self.assertEqual(ng.group_of(rules, "%s::%s" % (crate, unit), name),
                                     "webkit", (unit, name))
        self.assertLessEqual({"the_card_and_the_steps_draw_in_webkit",
                              "the_map_draws_in_webkit", "the_sheet_behaves_in_webkit"}, seen)

    def test_the_real_tree_has_no_unplaced_browser_unit(self):
        found = locks.check_browser_station_lock([], str(REPO))
        self.assertEqual([f for f in found if f.verdict == "RED"], [])


def _tree(files):
    root = tempfile.mkdtemp()
    for rel, text in files.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(text)
    return root


CARGO = '[package]\nname = "x"\nversion = "0.1.0"\n'
RUNS = ('const DRIVER: &str = "workshop/tools/display-new-browser.mjs";\n'
        '#[test]\nfn a() { let _ = std::process::Command::new("node").arg(DRIVER); }\n')


class AFixtureTree(unittest.TestCase):

    def test_the_marker_rule(self):
        root = _tree({
            "crates/x/Cargo.toml": CARGO,
            "crates/x/tests/runs_it.rs": RUNS,
            "crates/x/tests/reads_it.rs":
                'const D: &str = "workshop/tools/display-new-browser.mjs";\n'
                '#[test]\nfn a() { let _ = std::fs::read_to_string(D); }\n',
            "crates/x/tests/comment_only.rs":
                '/// see `display-new-browser.mjs`\n#[test]\n'
                'fn a() { let _ = std::process::Command::new("ls"); }\n',
            "crates/x/tests/named_browser.rs": '#[test]\nfn a() {}\n',
        })
        self.assertEqual(gp.browser_terms(root),
                         ["binary_id(=x::named_browser)", "binary_id(=x::runs_it)"])

    def test_a_new_browser_binary_is_planned_into_the_station(self):
        root = _tree({"crates/x/Cargo.toml": CARGO,
                      "crates/x/tests/new_thing_browser.rs": '#[test]\nfn a() {}\n'})
        rel = "crates/x/tests/new_thing_browser.rs"
        self.assertIsNone(gp.test_select([rel], "strand", repo=root))
        self.assertEqual(gp.browser_moved([rel], "strand", repo=root),
                         ["binary_id(=x::new_thing_browser)"])

    def test_an_ignored_browser_proof_outside_the_locks_is_red(self):
        root = _tree({"crates/x/Cargo.toml": CARGO,
                      "crates/x/tests/new_thing_browser.rs":
                          '#[test]\n#[ignore]\nfn a() {}\n#[test]\nfn b() {}\n'})
        found = locks.check_browser_station_lock(["README.md"], root)
        self.assertEqual([f.verdict for f in found], ["RED"])
        self.assertIn("crates/x/tests/new_thing_browser.rs", found[0].path)
        self.assertIn("browser-station-lock", dict(locks.LOCKS))

    def test_an_ignored_proof_in_a_unit_the_lexer_doubts_is_red(self):
        # GH #1161 x #1162 (re-check h2 M-1): the count of a unit the token lexer
        # doubts is (0, 0) -- no proof of "no #[ignore]". The twin without the
        # comment left open is red for its #[ignore]d proof (as in S3 alone,
        # whose regex lexer never doubted); the doubted one is red too.
        proof = ('const DRIVER: &str = "workshop/tools/display-new-browser.mjs";\n'
                 '#[test]\n#[ignore]\n'
                 'fn a() { let _ = std::process::Command::new("node").arg(DRIVER); }\n')
        doubted = proof + "/* a comment left open\n"
        self.assertIsNone(gp._rust_code(doubted))
        for text, why in ((proof, "an #[ignore]d browser proof"), (doubted, "the lexer doubts")):
            root = _tree({"crates/x/Cargo.toml": CARGO,
                          "crates/x/tests/doubted_proof.rs": text})
            self.assertEqual(gp.browser_terms(root), ["binary_id(=x::doubted_proof)"], why)
            found = locks.check_browser_station_lock([], root)
            self.assertEqual([f.verdict for f in found], ["RED"], why)
            self.assertEqual(found[0].path, "crates/x/tests/doubted_proof.rs")
            self.assertIn(why, found[0].text)

    def test_a_browser_unit_whose_tests_a_macro_may_make_is_red(self):
        # tb-s10 A1 point 21 (L-c): an item macro may make an #[ignore]d proof
        # the attribute count cannot see -- outside BROWSER_LOCKS no proof of
        # "no #[ignore]" (in the tree only the lock 710 holds one, 11.10.2026)
        text = RUNS + "gen_tests!();\n"
        self.assertEqual((1, 1), gp._test_counts(text))
        root = _tree({"crates/x/Cargo.toml": CARGO, "crates/x/tests/by_macro.rs": text})
        self.assertEqual(gp.browser_terms(root), ["binary_id(=x::by_macro)"])
        found = locks.check_browser_station_lock([], root)
        self.assertEqual([f.verdict for f in found], ["RED"])
        self.assertEqual(found[0].path, "crates/x/tests/by_macro.rs")
        self.assertIn("a macro may make", found[0].text)

    def test_a_browser_unit_whose_attributes_cannot_be_counted_is_red(self):
        # the lexer reads it, the attribute count does not (a `[` left open):
        # (0, 0) again -- the same doubt, the same red
        text = RUNS + "#[ignore\n"
        self.assertIsNotNone(gp._rust_code(text))
        self.assertEqual((0, 0), gp._test_counts(text))
        root = _tree({"crates/x/Cargo.toml": CARGO, "crates/x/tests/open_attr.rs": text})
        self.assertEqual(gp.browser_terms(root), ["binary_id(=x::open_attr)"])
        found = locks.check_browser_station_lock([], root)
        self.assertEqual([f.verdict for f in found], ["RED"])
        self.assertEqual(found[0].path, "crates/x/tests/open_attr.rs")
        self.assertIn("the lexer doubts", found[0].text)

    def test_a_shared_module_that_names_a_driver_is_red(self):
        root = _tree({"crates/x/Cargo.toml": CARGO,
                      "crates/x/tests/support/mod.rs": RUNS.replace("#[test]\n", ""),
                      "crates/x/tests/uses.rs": "mod support;\n#[test]\nfn a() {}\n"})
        found = locks.check_browser_station_lock([], root)
        self.assertEqual([f.verdict for f in found], ["RED"])
        self.assertEqual(found[0].path, "crates/x/tests/support/mod.rs")

    def test_a_shared_module_the_lexer_doubts_that_names_a_driver_is_red(self):
        # fail-closed (GH #1161 x #1162): a module the token lexer cannot read is
        # no proof that it does not start a browser -- a comment left open
        doubted = ('pub const D: &str = "workshop/tools/display-new-browser.mjs";\n'
                   '/* a comment left open\n')
        self.assertIsNone(gp._rust_code_aligned(doubted))
        root = _tree({"crates/x/Cargo.toml": CARGO,
                      "crates/x/tests/support/mod.rs": doubted,
                      "crates/x/tests/uses.rs": "mod support;\n#[test]\nfn a() {}\n"})
        found = locks.check_browser_station_lock([], root)
        self.assertEqual([f.verdict for f in found], ["RED"])
        self.assertEqual(found[0].path, "crates/x/tests/support/mod.rs")
        self.assertIn("the lexer doubts", found[0].text)

    def test_a_tree_without_crates_says_nothing(self):
        root = _tree({"README.md": "x"})
        self.assertEqual(locks.check_browser_station_lock([], root), [])



class TheMarkerRuleReadsTheTokenLexer(unittest.TestCase):
    """GH #1161 x #1162: the marker rule (S3) read the text of the regex lexer
    with its literals kept; S4 replaced that lexer by `crate_root.tokens`
    (the regex one lost its phase on a `\\`-newline string). The rule reads
    the token lexer now, through `crate_root.token_spans`: offsets and lines
    kept, every comment blank, every literal whole, None in doubt."""

    def test_token_spans_are_the_tokens_at_their_offsets(self):
        from scripts.pipeline import crate_root
        text = ('mod a; // c\n/// d\nfn b() { let s = r#"x"#; let c = \'c\'; }\n'
                '/** e */ const S: &str = "a\\\n b";\n')
        spans = crate_root.token_spans(text)
        self.assertEqual(crate_root.tokens(text), [t for _i, t in spans])
        for i, t in spans:
            if not t.startswith(("//", "/*")):
                self.assertEqual(t, text[i:i + len(t)])

    def test_aligned_code_blanks_comments_and_keeps_literals_and_lines(self):
        text = ('// display-a-browser.mjs\nfn a() { /* x\n y */ run("display-b-browser.mjs"); }\n'
                '/// doc Command\nconst S: &str = "a\\\n b";\n')
        code = gp._rust_code_aligned(text)
        self.assertEqual(len(text), len(code))
        self.assertEqual(text.count("\n"), code.count("\n"))
        self.assertNotIn("display-a-browser", code)
        self.assertIn('run("display-b-browser.mjs");', code)
        self.assertNotIn("Command", code)
        self.assertIn('"a\\\n b"', code)

    def test_aligned_code_is_none_in_doubt(self):
        self.assertIsNone(gp._rust_code_aligned('fn a() { let s = "open; }\n'))

    def test_a_unit_the_lexer_doubts_that_names_a_driver_is_a_starter(self):
        # a comment left open is a doubt of the token lexer (fail-closed)
        text = 'const D: &str = "display-x-browser.mjs";\n/* a comment left open\n'
        self.assertIsNone(gp._rust_code_aligned(text))
        self.assertTrue(gp.browser_unit("reads_it", text))

    def test_a_unit_that_reads_the_engine_path_is_a_starter(self):
        # GH #1189: gh766 drives Chromium in-process and names no driver; it reads
        # MECLAW_CHROMIUM. A comment that names it reads nothing.
        reads = 'fn packaged() -> Option<String> { std::env::var("MECLAW_CHROMIUM").ok() }\n'
        self.assertTrue(gp.browser_unit("in_process", reads))
        names = '// std::env::var("MECLAW_CHROMIUM")\nfn packaged() {}\n'
        self.assertFalse(gp.browser_unit("in_process", names))
        self.assertTrue(gp.browser_unit("in_process", reads + "/* a comment left open\n"))



def _release_units():
    """`[(crate, unit)]` of the real tree plus each crate's lib/bin unit tests."""
    units = [(c, u) for c, u, _t in gp._test_sources(None)]
    return units + [(c, c) for c in sorted({c for c, _u in units})]


def _hits(terms, crate, unit):
    """True when a planned term selects the unit. Only the three forms the planner
    writes; any other form selects nothing here, so its unit shows as a gap."""
    binary = unit.split("/")[0]
    for t in terms:
        m = gp._TERM_RE.match(t)
        if m and re.search(m.group(1), binary):
            return True
        if t in (gp._binary_id(crate, unit), "binary_id(=%s::%s)" % (crate, binary)):
            return True
    return False


# GH #1199: the scenario class counts `slack_live.rs` (8 terms, 57 units), which the
# export blocklists; only the three counting tests read it, the rest travel.
SLACK_LIVE = "crates/meclaw-cells/tests/slack_live.rs"


class TheReleasePartition(unittest.TestCase):
    """KR-TB-1 (d) "the release stays the full suite" (AGENTS.md t2), OR-TB.S4.55:
    the release subtracted the scenario class (8 terms, 57 units) from `tests` and
    planned it nowhere else -- 57 units no release ran since before the wave. The
    class is a station of its own now, and the plan refuses a release whose
    stations do not split `all()`: `tests` + `browser:display` + the scenario
    station, every unit exactly once (fail-closed)."""

    @private_tree.private(SLACK_LIVE)
    def test_the_release_plans_the_scenario_class_as_a_station(self):
        scen = gp.scenario_for(None)
        self.assertEqual(8, len(scen.split(" + ")))
        for files in (["docs/x.md"], [], ["crates/meclaw-cells/src/lib.rs"]):
            stations = gp.plan(files, "release", repo=None)
            st = by_name(stations)
            self.assertIn(gp.SCENARIO_STATION, st, files)
            row = st[gp.SCENARIO_STATION]
            # no --run-ignored: the live units stay `#[ignore]`d and skip as in CI
            self.assertEqual([["scripts/test-tier.sh", "filter", scen]], row.cmds, files)
            self.assertTrue(row.cargo, files)
            names = [s.name for s in stations]
            self.assertEqual(names.index("slow-tests") + 1, names.index(gp.SCENARIO_STATION))
        for mode in ("strand", "integration", "ci"):
            self.assertNotIn(gp.SCENARIO_STATION,
                             by_name(gp.plan(["crates/meclaw-cells/src/lib.rs"], mode, repo=None)),
                             mode)

    @private_tree.private(SLACK_LIVE)
    def test_the_release_stations_split_all_of_it(self):
        """Read from the planned commands alone: every unit in exactly one part."""
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        tests = st["tests"].cmds[0][2]
        self.assertTrue(tests.startswith("all() - (") and tests.endswith(")"), tests)
        cut = tests[len("all() - ("):-1].split(" + ")
        union = browser_filters(st["browser:display"])[0]
        tail = ") - %s" % gp.WEBKIT_TESTS
        self.assertTrue(union.startswith("(") and union.endswith(tail), union)
        union = union[1:-len(tail)].split(" + ")
        scen = st[gp.SCENARIO_STATION].cmds[0][2].split(" + ")
        gaps, twice, in_scen = [], [], 0
        for crate, unit in _release_units():
            where = [n for n, inside in (("tests", not _hits(cut, crate, unit)),
                                         ("browser:display", _hits(union, crate, unit)),
                                         ("scenario", _hits(scen, crate, unit))) if inside]
            if not where:
                gaps.append((crate, unit))
            elif len(where) > 1:
                twice.append((crate, unit, where))
            in_scen += where == ["scenario"]
        self.assertEqual([], gaps)
        self.assertEqual([], twice)
        self.assertEqual(57, in_scen)
        self.assertEqual(([], []), gp.release_partition(st, repo=None))

    def test_a_gap_in_the_release_plan_is_refused(self):
        terms = gp._scenario_terms(None)
        dropped = terms[0]
        with unittest.mock.patch.object(gp, "scenario_station_terms",
                                        lambda repo=None: terms[1:]):
            with self.assertRaises(gp.GatePlanError) as cm:
                gp.plan(["docs/x.md"], "release", repo=None)
        msg = str(cm.exception)
        self.assertIn("in no station", msg)
        self.assertIn("KR-TB-1", msg)
        self.assertRegex(msg, re.compile(gp._TERM_RE.match(dropped).group(1).strip("^$")))

    def test_a_unit_in_two_stations_is_refused(self):
        units = [(c, u) for c, u, t in gp._test_sources(None)
                 if "/" not in u and not gp.browser_unit(u, t)
                 and not _hits(gp._scenario_terms(None), c, u)]
        crate, unit = units[0]
        extra = gp._scenario_terms(None) + ["binary_id(=%s::%s)" % (crate, unit)]
        with unittest.mock.patch.object(gp, "scenario_station_terms",
                                        lambda repo=None: extra):
            with self.assertRaises(gp.GatePlanError) as cm:
                gp.plan(["docs/x.md"], "release", repo=None)
        self.assertIn("in two stations", str(cm.exception))
        self.assertIn(unit, str(cm.exception))

    def test_a_term_the_guard_cannot_read_is_refused(self):
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        row = st["tests"]
        odd = row.cmds[0][2][:-1] + " + kind(test))"
        st["tests"] = row._replace(cmds=[["scripts/test-tier.sh", "filter", odd]])
        gaps, twice = gp.release_partition(st, repo=None)
        self.assertIn(("tests", "kind(test)"), gaps)

    def test_a_compound_binary_term_is_no_single_operator(self):
        """`_TERM_RE` is greedy: the compound below read as one binary regex."""
        compound = "binary(/(a/) & test(/b)/)"
        self.assertIsNotNone(gp._TERM_RE.match(compound))      # the greedy reading
        self.assertIsNone(gp._ONE_BINARY_RE.match(compound))
        self.assertIsNone(gp._term_hits(compound, [("c", "a")]))
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        row = st[gp.SCENARIO_STATION]
        st[gp.SCENARIO_STATION] = row._replace(
            cmds=[["scripts/test-tier.sh", "filter", row.cmds[0][2] + " + " + compound]])
        self.assertIn((gp.SCENARIO_STATION, compound), gp.release_partition(st, repo=None)[0])

    def test_the_partition_reads_browser_alone_after_the_split(self):
        """Fund 27 (re-check tb-s1e-alone, Post-xhigh-8): `plan.split_alone` moves
        the locks of `ALONE_LOCKS` out of `browser:display` into `browser:alone`
        after `gate_plan.plan()`. The guard reads the plan the gate runs, so
        `browser:alone` is a browser part of its own -- read without it, every
        alone lock is a unit in no station."""
        import types
        rows = gp.to_json(["docs/x.md"], "release", repo=None)["stations"]
        notes = pl.split_alone(rows)
        self.assertTrue(notes, "no lock ran alone -- the pin reads nothing")
        st = {r["name"]: types.SimpleNamespace(cmds=[r["cmd"]] if "cmd" in r
                                               else list(r.get("cmds") or []))
              for r in rows}
        self.assertIn(pl.ALONE, st)
        self.assertEqual(([], []), gp.release_partition(st, repo=None))
        # and the refusal holds behind the split: the alone row lost is a gap
        del st[pl.ALONE]
        gaps, _twice = gp.release_partition(st, repo=None)
        self.assertTrue(gaps)

    def test_the_pipeline_refuses_a_release_the_split_broke(self):
        """The same guard after `split_alone`, in `pipeline.plan` (fail-closed):
        an alone row that lost its lock leaves the unit in no station."""
        rows = gp.to_json(["docs/x.md"], "release", repo=None)["stations"]
        pl.split_alone(rows)
        self.assertEqual([], pl.release_guard(rows, None))
        alone = next(r for r in rows if r["name"] == pl.ALONE)
        rows.remove(alone)
        # `pipeline.plan` loads its own copy of gate_plan (`_gp()`)
        with self.assertRaises(pl._gp().GatePlanError) as cm:
            pl.release_guard(rows, None)
        self.assertIn("KR-TB-1", str(cm.exception))

    @private_tree.private(SLACK_LIVE)
    def test_the_explain_row_names_the_scenario_binaries(self):
        rows = {r["name"]: r for r in gp.to_json(["docs/x.md"], "release", explain=True)["stations"]}
        row = rows[gp.SCENARIO_STATION]
        self.assertEqual(["mode: release"], row["reason"])
        sel = row["select"]
        self.assertEqual(gp.scenario_for(None), sel["filter"])
        self.assertEqual(57, len(sel["extra"]))
        self.assertFalse(sel["all"] or sel["floor"] or sel["binaries"] or sel["rdeps"])

    def test_the_pipeline_runs_it_on_its_own_profile_from_the_one_build(self):
        name = gp.SCENARIO_STATION
        self.assertEqual("scenario", pl.NEXTEST[name])
        self.assertIn(name, pl.REUSE)
        self.assertIn(name, pl.BUILD_CONSUMERS)
        toml = (REPO / ".config" / "nextest.toml").read_text()
        self.assertIn("[profile.scenario.junit]", toml)


class EveryTermReaderTakesOneBinaryOperator(unittest.TestCase):
    """tb-s10 D (line 16): the greedy `_TERM_RE` read a compound term
    `binary(/(a/) & test(/b)/)` as ONE binary regex `(a/) & test(/b)` in every
    reader past the guard -- the scenario class, the `minus` of `test_select`,
    the browser-lock narrowing and `harness_fold`. Each reads a single
    operator now (`_ONE_BINARY_RE`) and takes a compound as a form it cannot
    read: kept, never a garbage regex."""

    COMPOUND = "binary(/(gh7/) & test(/b)/)"
    BROKEN = "binary(/^gh7/) & test(/b/)"   # greedy: `^gh7/) & test(/b`, no regex

    def _repo(self):
        return _tree({"crates/x/Cargo.toml": CARGO,
                      "crates/x/tests/gh7_runs.rs": 'const R: &str = "scripts/test-tier.sh";\n'
                                                   '#[test]\nfn a() {}\n'})

    def test_the_scenario_class_keeps_a_compound_term(self):
        with unittest.mock.patch.object(gp, "SCENARIO_TERMS", (self.COMPOUND,)):
            self.assertEqual(gp.scenario_for(self._repo()), self.COMPOUND)

    def test_the_scenario_binaries_skip_a_compound_term(self):
        with unittest.mock.patch.object(gp, "SCENARIO_TERMS", (self.BROKEN,)):
            self.assertEqual(gp.scenario_binaries(self._repo()), [])

    def test_the_release_minus_holds_no_compound_regex(self):
        with unittest.mock.patch.object(gp, "_scenario_terms", lambda repo=None: [self.COMPOUND]):
            sel = gp.test_select(["docs/x.md"], "release", repo=self._repo())
        self.assertEqual(sel["minus"], [])
        self.assertIn(self.COMPOUND, sel["filter"])

    def test_the_integration_minus_holds_no_compound_regex(self):
        with unittest.mock.patch.object(gp, "_scenario_terms", lambda repo=None: [self.COMPOUND]):
            sel = gp.test_select(["scripts/test-tier.sh"], "integration", repo=self._repo())
        self.assertEqual(sel["minus"], [])
        self.assertIn(self.COMPOUND, sel["filter"])

    def test_the_browser_lock_narrowing_keeps_a_compound_lock(self):
        with unittest.mock.patch.object(gp, "IGNORED_LOCKS", (self.COMPOUND,)):
            cmds = gp.browser_cmds(False, keep=["crates/x/tests/other_browser.rs"])
        self.assertTrue(any(self.COMPOUND in argv[2] for argv in cmds), cmds)

    def test_harness_fold_keeps_every_stem_for_a_compound_scenario_term(self):
        import harness_fold  # noqa: PLC0415
        with unittest.mock.patch.object(gp, "SCENARIO_TERMS", (self.COMPOUND,)):
            regs, _locks = harness_fold._gate_plan_keeps()
        self.assertTrue(any(r.search("gh7_runs") for r in regs))


# --- OR-TM-8 (K15, GH #1199): a PYTHONPATH reaches its modules, not every .py ---
#
# Measured on AN3-FIX (6bd5216f1, 16.10.): a Python-only diff under
# `workshop/tools/llm-conformance/` planned `build`, `build:browser`, both parts of
# `browser:display` and `browser:alone` (crit 10800 s on build03). Rule 8 runtime
# read `RUNTIME_DIR_READS_ONLY["workshop/tools"] = (".py",)` as "every .py at any
# depth": every unit that names `workshop/tools` -- the PYTHONPATH reader gh661, but
# also the driver walkers 708/744/gh1209 -- read the file. An `import` from that
# root reaches only the modules whose every name is a Python identifier;
# `llm-conformance/` and `display-lab/` are no package.

PY_READER = ('#[test]\nfn a() {\n    let tools = std::path::Path::new("workshop/tools");\n'
             '    let _ = std::process::Command::new("python3").env("PYTHONPATH", tools);\n}\n')
WALKER = ('#[test]\nfn a() {\n    let tools = std::path::Path::new("workshop/tools");\n'
          '    let _ = std::fs::read_dir(tools);\n}\n')
WALK_ALL = ('#[test]\nfn a() {\n    let d = std::path::Path::new("workshop/fixtures");\n'
            '    let _ = std::fs::read_dir(d);\n}\n')
PY_TREE = {
    "crates/x/Cargo.toml": CARGO,
    "crates/x/tests/seeds_from_the_tools.rs": PY_READER,
    "crates/x/tests/walks_the_drivers_browser.rs": WALKER,
    "crates/x/tests/walks_the_fixtures.rs": WALK_ALL,
    "workshop/tools/seed.py": "X = 1\n",
    "workshop/tools/pkg/sub.py": "Y = 1\n",
    "workshop/tools/llm-conformance/fake_provider.py": "Z = 1\n",
    "workshop/tools/llm-conformance/README.md": "z\n",
    "workshop/tools/display-lab/turn.py": "T = 1\n",
    "workshop/fixtures/some-set/case.py": "C = 1\n",
}
SEEDS = "binary_id(=x::seeds_from_the_tools)"
WALKER_UNIT = "x::walks_the_drivers_browser"
# The fixture's readers named like the real ones (RUNTIME_DIR_READS_ONLY is per
# reader, fix round stapel): the PYTHONPATH reader as gh661, the browser walker
# as gh1209_no_stage. Unnamed they read every file below (fail-closed).
FIXTURE_READERS = {"workshop/tools": {
    "seeds_from_the_tools": ((".py",), "import"),
    "walks_the_drivers_browser": ((".mjs", ".js"), "any")}}


def _name_the_fixture_readers(case):
    patcher = unittest.mock.patch.dict(gp.RUNTIME_DIR_READS_ONLY, FIXTURE_READERS)
    patcher.start()
    case.addCleanup(patcher.stop)


def _stations(paths, root, mode="strand"):
    return {s["name"]: s for s in gp.to_json(paths, mode, root, explain=True)["stations"]}


class APythonPathReachesItsModules(unittest.TestCase):
    """Pins (a)-(c) on a fixture tree: they run in the public tree as well."""

    def setUp(self):
        self.root = _tree(PY_TREE)
        _name_the_fixture_readers(self)

    def test_a_module_below_a_hyphened_directory_plans_no_cargo_station(self):
        # pin (a): no reader, no `tests`, no `browser:display` -- so nothing to build
        for rel in ("workshop/tools/llm-conformance/fake_provider.py",
                    "workshop/tools/display-lab/turn.py"):
            st = _stations([rel], self.root)
            self.assertNotIn("tests", st, rel)
            self.assertNotIn("browser:display", st, rel)
            self.assertFalse([n for n, s in st.items() if s.get("cargo")], rel)
            self.assertEqual({}, gp.test_terms([rel], self.root), rel)

    def test_a_module_the_import_reaches_plans_its_import_reader_and_no_browser(self):
        # pin (b), fix round stapel I2 (OR-V2.v2-r3-planbrowser.8): a top-level
        # module and one of a package plan the PYTHONPATH reader; the browser
        # walker keeps `.mjs`/`.js` only -- before, it ran for every module
        for rel in ("workshop/tools/seed.py", "workshop/tools/pkg/sub.py"):
            st = _stations([rel], self.root)
            self.assertIn(SEEDS, st["tests"]["select"]["filter"], rel)
            self.assertNotIn("browser:display", st, rel)
            self.assertNotIn(WALKER_UNIT, st["tests"]["select"]["filter"], rel)

    def test_a_walk_outside_the_table_still_reads_every_file_below(self):
        # pin (c): fail-closed stays -- a directory the table does not name is
        # read whole, a hyphened sub-directory and any suffix included
        rel = "workshop/fixtures/some-set/case.py"
        self.assertIn("binary_id(=x::walks_the_fixtures)", gp.test_terms([rel], self.root))
        self.assertTrue(gp.runtime_dir_applies("workshop/fixtures",
                                               "workshop/fixtures/new-set/x.bin"))

    def test_a_crate_fixture_no_test_names_stays_wide(self):
        # pin (c): the unread shape keeps rdeps(<crate>) with its reason
        rel = "crates/x/tests/fixtures/new-set/case.json"
        terms = gp.test_terms([rel], self.root)
        self.assertIn("rdeps(x)", terms)
        self.assertTrue(any(r.startswith("rule:8 runtime unread ") for r in terms["rdeps(x)"]))


# OR-V2.v2-r3-planbrowser.2 (stapel c): a browser unit that walks `workshop/tools`
# for the drivers (744 `.mjs`, gh1209 `.mjs`/`.js`, every depth but node_modules)
# reads them below a hyphened directory too; the PYTHONPATH reader beside it does
# not. Before, `RUNTIME_DIR_READS_ONLY` gave every reader `.py` only, and a diff to
# `lib/no-filters.mjs` or `display-lab/tap.mjs` planned no `browser:display`.
JS_TREE = dict(PY_TREE, **{"workshop/tools/display-lab/tap.mjs": "export const t = 1;\n",
                           "workshop/tools/lib/switch.mjs": "export const s = 1;\n",
                           "workshop/tools/lib/old.js": "module.exports = 1;\n"})


class ABrowserWalkReadsTheDrivers(unittest.TestCase):
    """Pin (c) of the stapel on a fixture tree: it runs in the public tree as well."""

    def setUp(self):
        self.root = _tree(JS_TREE)
        _name_the_fixture_readers(self)

    def test_a_walked_driver_plans_the_browser_walker_and_no_tests(self):
        for rel in ("workshop/tools/display-lab/tap.mjs", "workshop/tools/lib/switch.mjs",
                    "workshop/tools/lib/old.js"):
            st = _stations([rel], self.root)
            self.assertIn(WALKER_UNIT, st["browser:display"]["select"]["extra"], rel)
            self.assertEqual(["browser unit: %s" % rel], st["browser:display"]["reason"], rel)
            self.assertNotIn("tests", st, rel)
            self.assertNotIn(SEEDS, gp.test_terms([rel], self.root), rel)

    def test_a_driver_of_the_class_plans_the_walker_beside_the_class(self):
        # stapel c2: a driver `display-*.mjs` is a walked `.mjs` too -- before,
        # RUNTIME_DIR_BROWSER_READS left the class out and the walker never ran
        rel = "workshop/tools/display-new-browser.mjs"
        st = _stations([rel], self.root)
        self.assertEqual(["display_browser: %s" % rel, "browser unit: %s" % rel],
                         st["browser:display"]["reason"])
        self.assertIn(WALKER_UNIT, st["browser:display"]["select"]["extra"])
        self.assertNotIn("tests", st)

    def test_ci_keeps_the_walker_in_tests(self):
        # rule 11 holds: in ci a browser unit stays in `tests`
        st = _stations(["workshop/tools/display-lab/tap.mjs"], self.root, "ci")
        self.assertIn(WALKER_UNIT, st["tests"]["select"]["filter"])
        self.assertNotIn(SEEDS, st["tests"]["select"]["filter"])


class AReaderTheRuleDoesNotNameReadsEverything(unittest.TestCase):
    """Fix round stapel (m1): RUNTIME_DIR_READS_ONLY narrows its NAMED readers
    only; an unnamed reader of `workshop/tools` reads every file below it, any
    suffix, any depth, in its own station. Before, every reader was narrowed to
    the importable modules (and a browser unit to `.mjs`/`.js` beside them) --
    a new walker of `.py` at every depth, or of `.json`, ran for none."""

    def setUp(self):
        self.root = _tree(JS_TREE)   # the readers unnamed: no FIXTURE_READERS

    def test_an_unnamed_reader_reads_every_file_below(self):
        for rel in ("workshop/tools/llm-conformance/README.md",
                    "workshop/tools/llm-conformance/fake_provider.py",
                    "workshop/tools/display-lab/tap.mjs", "workshop/tools/seed.py"):
            st = _stations([rel], self.root)
            self.assertIn(SEEDS, st["tests"]["select"]["filter"], rel)
            self.assertIn(WALKER_UNIT, st["browser:display"]["select"]["extra"], rel)
            self.assertNotIn(WALKER_UNIT, st["tests"]["select"]["filter"], rel)
        self.assertTrue(gp.runtime_dir_applies("workshop/tools", "workshop/tools/a-b/c.bin",
                                               "an_unnamed_walker"))


@private_tree.private("workshop/tools")
class TheConformanceToolPlansNoBrowser(unittest.TestCase):
    """Pins (a)/(b) on the real tree: the AN3-FIX diff and the browser tools."""

    AN3_PY_ONLY = ["scripts/tests/test_llm_conformance.py",
                   "workshop/tools/llm-conformance/README.md",
                   "workshop/tools/llm-conformance/cost_guard.py",
                   "workshop/tools/llm-conformance/fake_provider.py"]

    def test_the_tool_without_a_named_file_plans_no_cargo_station(self):
        st = _stations(self.AN3_PY_ONLY, None)
        self.assertIn("llm-conformance", st)
        self.assertNotIn("tests", st)
        self.assertNotIn("browser:display", st)
        self.assertFalse([n for n, s in st.items() if s.get("cargo")])

    def test_the_file_a_drift_lock_names_plans_that_lock_alone(self):
        # h5 reads `conformance.py` by name (SAMPLING_PARAMS): it stays, alone
        st = _stations(self.AN3_PY_ONLY + ["workshop/tools/llm-conformance/conformance.py"],
                       None)
        self.assertEqual(["meclaw-cells::misc_8"], st["tests"]["select"]["binaries"])
        self.assertIn("h5_the_catalogue_matches_the_measurement", st["tests"]["select"]["filter"])
        self.assertNotIn("browser:display", st)

    def test_a_browser_tool_plans_the_browser_station_as_before(self):
        # pin (b): driver, lock, browser test file, the library a stand-in imports
        # with both routes of today: the class `display_browser` and the
        # browser units the selection reaches
        for rel, labels in (
                ("workshop/tools/display-filters-browser.mjs",
                 ("display_browser", "browser unit")),
                ("crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs",
                 ("display_browser", "browser unit")),
                ("crates/meclaw-cells/tests/gh1209_no_stage_draws_a_filter_browser.rs",
                 ("browser unit",)),
                ("workshop/tools/lib/os-template.mjs", ("browser unit",))):
            st = _stations([rel], None)
            self.assertEqual(["%s: %s" % (label, rel) for label in labels],
                             st["browser:display"]["reason"], rel)

    def test_a_top_level_module_still_plans_its_readers(self):
        # pin (b): the PYTHONPATH reader gh661 imports top-level modules
        st = _stations(["workshop/tools/judge_eval.py"], None)
        self.assertIn("gh661_the_briefing_states_the_operator_set_rule",
                      st["tests"]["select"]["filter"])

    def test_no_module_the_import_reaches_names_a_hyphened_directory(self):
        # the boundary of the rule: a file below `llm-conformance/` or
        # `display-lab/` is reached by path only, and no module the PYTHONPATH
        # reaches names one of them -- the day one does, the rule must follow
        # -- at every depth (fix round stapel m1): a package module
        # (`<ident>/x.py`) is reached as well, a hyphened directory below one too
        tools = DRIVERS
        hyphened = sorted({p.name for p in tools.rglob("*")
                           if p.is_dir() and not p.name.isidentifier()
                           and "node_modules" not in p.relative_to(tools).parts})
        self.assertIn("llm-conformance", hyphened)   # not vacuous
        for py in sorted(tools.rglob("*.py")):
            rel = py.relative_to(tools).as_posix()
            if not gp._importable(rel):
                continue
            text = py.read_text(encoding="utf-8")
            for d in hyphened:
                self.assertNotIn(d, text, "%s names %s/" % (rel, d))


# The two browser units that walk `workshop/tools` for the drivers (grep of the
# sources): 744 `walk(&repo("workshop/tools"))` keeps every `.mjs`, gh1209
# `files(&tools, &[".mjs", ".js"])` every `.mjs`/`.js` -- both at every depth,
# node_modules aside, `display-lab/` and `lib/` included.
WALKERS = ("meclaw-cells::744_a_late_patch_does_not_take_the_drawing_back",
           "meclaw-cells::gh1209_no_stage_draws_a_filter_browser")


@private_tree.private("workshop/tools")
class TheBrowserToolInputsPlanTheBrowserStation(unittest.TestCase):
    """Pin (c) of the stapel on the real tree (OR-V2.v2-r3-planbrowser.2): every
    input of the browser drivers plans `browser:display` on its own -- the
    Playwright pin and its lock by the class (`lane_sync.sh browsers` provisions
    the engine from `package.json`, no test names either), the filter switch and
    the lab's `.mjs` by the two units that walk them."""

    def test_the_playwright_pin_and_its_lock_plan_the_station_by_its_class(self):
        for rel in ("workshop/tools/package.json", "workshop/tools/package-lock.json"):
            st = _stations([rel], None)
            self.assertIn("browser:display", st, rel)
            self.assertEqual(["display_browser: %s" % rel], st["browser:display"]["reason"], rel)

    def test_each_walked_input_plans_the_units_that_walk_it(self):
        lab = sorted(p.relative_to(REPO).as_posix()
                     for p in (DRIVERS / "display-lab").rglob("*.mjs"))
        self.assertGreaterEqual(len(lab), 4, lab)   # not vacuous
        for rel in ["workshop/tools/lib/no-filters.mjs"] + lab:
            st = _stations([rel], None)
            self.assertIn("browser:display", st, rel)
            self.assertEqual(["browser unit: %s" % rel], st["browser:display"]["reason"], rel)
            for unit in WALKERS:
                self.assertIn(unit, st["browser:display"]["select"]["extra"], rel)
            self.assertNotIn("tests", st, rel)

    def test_a_driver_diff_plans_the_walkers_beside_the_class(self):
        # stapel c2 (rest of OR-V2.v2-r3-planbrowser.2): a driver of the class
        # `display_browser` is a `.mjs` below `workshop/tools` as well, and the
        # walkers read it -- gh1209 refuses exactly a new driver without
        # `withoutFilters`. Before, a driver-only diff ran the class's locks and
        # neither walker. Fix round stapel I1 (OR-V2.v2-r3-planbrowser.7): 708
        # reads every top-level `.mjs`, so a driver diff plans it in `tests` --
        # the walkers stay out of `tests`.
        for rel in ("workshop/tools/display-layout-browser.mjs",
                    "workshop/tools/display-new-browser.mjs"):
            st = _stations([rel], None)
            self.assertIn("browser:display", st, rel)
            reason = st["browser:display"]["reason"]
            self.assertIn("display_browser: %s" % rel, reason, rel)
            self.assertIn("browser unit: %s" % rel, reason, rel)
            for unit in WALKERS:
                self.assertIn(unit, st["browser:display"]["select"]["extra"], rel)
                if "tests" in st:
                    self.assertNotIn(unit, st["tests"]["select"]["filter"], rel)
        st = _stations(["workshop/tools/display-new-browser.mjs"], None)
        self.assertEqual(["meclaw-cells::708_the_client_draws_the_tap_before_the_pass"],
                         st["tests"]["select"]["binaries"])

    def test_no_python_module_of_the_lab_plans_a_browser(self):
        # OR-V2.v2-r3-planbrowser.1: no browser test and no lock reads the
        # lab's Python; the walkers read `.mjs`/`.js` only
        lab = sorted(p.relative_to(REPO).as_posix()
                     for p in (DRIVERS / "display-lab").rglob("*.py"))
        self.assertGreaterEqual(len(lab), 8, lab)   # not vacuous
        for rel in lab:
            st = _stations([rel], None)
            self.assertNotIn("browser:display", st, rel)
            self.assertFalse([n for n, s in st.items() if s.get("cargo")], rel)


# Fix round stapel (test-cc25, I1/I2/m1): what a walk of `workshop/tools` reads
# is a property of the READER, not of the directory -- gh661 imports the
# modules (`tests`), 708 `every_driver_keeps_the_answer_of_an_evidence_shot`
# reads every `.mjs` of the top level (`tests`), 744 every `.mjs` and gh1209
# every `.mjs`/`.js` at every depth (`browser:display`, rule 11).
SHOT_WALK = "meclaw-cells::708_the_client_draws_the_tap_before_the_pass"
IMPORT_READER = "gh661_the_briefing_states_the_operator_set_rule"
IMPORTABLE = ("build_librarian.py", "build_librarian_seed.py", "cache_ttl_calc.py",
              "check_reasoning_effort.py", "hive_context_sweep.py", "judge_eval.py")


@private_tree.private("workshop/tools")
class EachReaderOfTheToolsReadsItsOwnFiles(unittest.TestCase):
    """I1/I2 of the fix round on the real tree: a driver diff runs the shot walk
    708 in `tests`, an importable module runs gh661 and no browser."""

    def test_a_driver_diff_plans_the_shot_walk_in_tests(self):
        # I1: a driver of today and a new one -- 708 names none of them; before,
        # a driver-only diff planned no `tests` and a dropped shot answer passed
        for rel in ("workshop/tools/display-filters-browser.mjs",
                    "workshop/tools/display-scene-browser.mjs",
                    "workshop/tools/display-new-browser.mjs"):
            st = _stations([rel], None)
            self.assertIn(SHOT_WALK, st["tests"]["select"]["filter"], rel)
            for unit in WALKERS:
                self.assertIn(unit, st["browser:display"]["select"]["extra"], rel)
                self.assertNotIn(unit, st["tests"]["select"]["filter"], rel)

    def test_an_importable_module_plans_its_import_reader_and_no_browser(self):
        # I2: no browser unit reads a `.py` there; before, the walkers 744 and
        # gh1209 read every importable module and pulled the browser run
        for name in IMPORTABLE:
            rel = "workshop/tools/" + name
            self.assertTrue((DRIVERS / name).is_file(), rel)   # not vacuous
            st = _stations([rel], None)
            self.assertNotIn("browser:display", st, rel)
            self.assertIn(IMPORT_READER, st["tests"]["select"]["filter"], rel)
            for unit in WALKERS:
                self.assertNotIn(unit, st["tests"]["select"]["filter"], rel)

    def test_every_reader_of_the_tools_is_named_with_what_it_keeps(self):
        # m1: the rule against the tree -- every unit that names `workshop/tools`
        # has its entry (a new walker fails here, and plans fail-closed until
        # its entry says what it keeps), and each entry keeps exactly the
        # suffixes its source filters on, at the depth its walk goes
        table = gp.RUNTIME_DIR_READS_ONLY["workshop/tools"]
        readers = {u for _c, u in gp._runtime_index(str(REPO))["workshop/tools"]}
        self.assertEqual(sorted(readers), sorted(table))
        sources = {u: t for _c, u, t in gp._test_sources(str(REPO))}
        # an extension filter: `Some("mjs")`, `== "mjs"`, `[".mjs", ".js"]`
        ext = re.compile(r'(?:Some\("|== "|"\.)(mjs|js|cjs|json|py|css|html|sh|ts|md)"')
        for unit, (suffixes, depth) in sorted(table.items()):
            text = sources[unit]
            if depth == "import":
                # the root goes onto the import path of the generator it runs
                self.assertIn("sys.path.insert", text, unit)
                self.assertEqual((".py",), suffixes, unit)
                continue
            self.assertEqual(sorted(suffixes), sorted("." + m for m in ext.findall(text)), unit)
            # "any": the walk descends into directories; "top": one `read_dir`
            self.assertEqual(depth == "any", ".is_dir()" in text, unit)


if __name__ == "__main__":
    unittest.main()
