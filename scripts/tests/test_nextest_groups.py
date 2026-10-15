"""The nextest test groups of `.config/nextest.toml` against the test tree.

Two groups decide how wide the expensive tests run, in place of a host-wide
thread cap:

- `webkit` (`max-threads = 1`): WebKit renders in software here, and one page
  spreads over every core of the host. Two WebKit pages side by side starved
  each other (GH #1048), so they take turns.
- `colony-boot` (`max-threads = 4`): the tests that boot a heavy colony -- the
  display colony of `support/display_colony.rs` (python compose cells, a judge
  mock, mounts), the browser locks, the scenario class (`*_demo`, `e2e`,
  `workshop_scenario`). At full lane width (24 threads) too many of them at
  once ran the colony half of the B-proofs past its timeout (GH #1088).

A third, `long-first` (`max-threads = 4`), is no boot cap but a schedule: the
47 measured longest tests by name, so that `priority` does not start them all
at once (GH #1164; pinned in `test_pipeline_ir_path.py`).

nextest puts a test into exactly ONE group: of the overrides that set
`test-group`, the first one whose filter matches wins -- the profile's own
overrides first, then those of `default`, which every profile inherits. So a
test that boots a colony AND drives WebKit stays in `webkit`, and the
`webkit` filter has to stay narrow: a whole binary there would put its
Chromium tests in the one-at-a-time line as well.

What "boots a colony" means here: the source uses the display colony
(`display_colony`), or it is a browser lock or a scenario-class binary and
reaches a colony (`bootstrap_from_filesystem` / `display_colony`). The plain
`bootstrap_from_filesystem` tests (a few hundred binaries, a tiny colony each)
are left at full width on purpose: capping them at four would throttle the
whole `tests` station for the cost of one class.

The evaluator is `scripts/slow_tests.py`'s: the filterset subset this file
uses (`binary_id()`, `test()`, `&`, `+`/`|`, `not`, parentheses) and a loud
error on anything else. No cargo: binaries and test names are read from the
sources (`crates/<crate>/tests/<binary>.rs`).
"""

import pathlib
import re
import sys
import tomllib
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import harness_fold  # noqa: E402
import slow_tests  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()

CONFIG = REPO / ".config" / "nextest.toml"
COLONY_710 = "meclaw-cells::710_the_colony_holds_in_both_engines_browser"
COLONY_710_FILE = (REPO / "crates" / "meclaw-cells" / "tests"
                   / "710_the_colony_holds_in_both_engines_browser.rs")
# The browser locks that used to loop over both engines inside one test.
SPLIT_LOCKS = ("meclaw-cells::gh961_motion_by_target_state_browser",
               "meclaw-cells::gh1028_a_new_window_keeps_the_others_in_place_browser")

# A binary boots a display colony when it takes the support module in
# (`mod display_colony;`, `use [crate::]display_colony;`, `display_colony::...`), not when its text names the
# file: 708's source pins read `support/display_colony.rs` as text and boot
# nothing -- the bare word put its 16 pins into the colony-boot check
# (welle-testing M7h, red on 46536c56c and 6da512e59).
DISPLAY_COLONY = re.compile(r"\b(?:mod|use)\s+(?:(?:crate|super|self)::)*display_colony\b|\bdisplay_colony::")
BOOTS = re.compile(r"bootstrap_from_filesystem|" + DISPLAY_COLONY.pattern)
HEAVY = re.compile(r"_browser$|_demo$|_demo_|e2e|^workshop_scenario$")
# A test function: its test attribute, then attributes and comments, then `fn`.
TEST_FN = re.compile(
    r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*"
    r"(?:(?:#\[[^\n]*\]|//[^\n]*)\s*)*"
    r"(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")


def load_config():
    with open(CONFIG, "rb") as f:
        return tomllib.load(f)


def crate_name(crate_dir):
    m = re.search(r'(?m)^name\s*=\s*"([^"]+)"', (crate_dir / "Cargo.toml").read_text())
    return m.group(1) if m else crate_dir.name


def built_test_binaries():
    """`{binary_id: source text}` for every integration test file of the tree."""
    out = {}
    for crate_dir in sorted((REPO / "crates").iterdir()):
        tests = crate_dir / "tests"
        if not tests.is_dir() or not (crate_dir / "Cargo.toml").is_file():
            continue
        name = crate_name(crate_dir)
        for src in sorted(tests.glob("*.rs")):
            out["%s::%s" % (name, src.stem)] = src.read_text(encoding="utf-8")
    return out


def folded_units():
    """`[(harness binary_id, stem, source text)]` for every folded module: in a
    harness (`scripts/harness_fold.py`, GH #1119) a test runs in the binary
    `<crate>::<harness>` under the name `<stem>::<test>`."""
    out = []
    for crate_dir in sorted((REPO / "crates").iterdir()):
        tests = crate_dir / "tests"
        if not tests.is_dir() or not (crate_dir / "Cargo.toml").is_file():
            continue
        name = crate_name(crate_dir)
        for main in sorted(tests.glob("*/main.rs")):
            for src in sorted(main.parent.glob("*.rs")):
                if src.name != "main.rs":
                    out.append(("%s::%s" % (name, main.parent.name), src.stem,
                                src.read_text(encoding="utf-8")))
    return out


def harness_binaries():
    """`{binary_id: [(name prefix, source text)]}` as the harness table of each
    crate places the tests (`crates/<c>/tests/HARNESS.toml`, read through
    `harness_fold.Crate.plan`, GH #1119/#1153): a kept stem is its own binary
    `<c>::<stem>` with its names as written, a folded stem runs in
    `<c>::<harness>` as `<stem>::<test>`. Binary directories the fold did not
    write stand as binaries without sources."""
    out = {}
    for crate_dir in sorted((REPO / "crates").iterdir()):
        if not (crate_dir / "tests").is_dir() or not (crate_dir / "Cargo.toml").is_file():
            continue
        name = crate_name(crate_dir)
        c = harness_fold.Crate(str(REPO), crate_dir.name)
        for stem, (where, _) in sorted(c.plan().items()):
            text = c.files[stem][1]
            if where == "keep":
                out.setdefault("%s::%s" % (name, stem), []).append(("", text))
            else:
                out.setdefault("%s::%s" % (name, where), []).append((stem + "::", text))
        for d in c.other_dirs:
            out.setdefault("%s::%s" % (name, d), [])
    return out


BINARY_ID_TERM = re.compile(r"binary_id\(([=/])([^)]*)\)")


def gone_binary_names(config, binaries, crates):
    """`[(profile, term)]` of every override term `binary_id(=<id>)` that names no
    binary of the tree, and every `binary_id(/<re>/)` that matches none. A
    library's own id (`<c>`) and a bin target (`<c>::bin/<n>`) count as named."""
    gone = []
    for profile, prof in sorted(config.get("profile", {}).items()):
        for ov in prof.get("overrides", []):
            for kind, arg in BINARY_ID_TERM.findall(ov.get("filter", "")):
                if kind == "=":
                    ok = (arg in binaries or arg in crates
                          or ("::bin/" in arg and arg.split("::", 1)[0] in crates))
                else:
                    pat = re.compile(arg[:-1] if arg.endswith("/") else arg)
                    ok = any(pat.search(b) for b in binaries)
                if not ok:
                    gone.append((profile, "binary_id(%s%s)" % (kind, arg)))
    return gone


def dead_override_filters(config, units):
    """`[(profile, filter)]` of every override whose filter takes no test of the
    tree, as the harness table places them (`harness_binaries`). The
    `priority` overrides are left out: they name macro-made tests
    (`_runs_N_to_M`) the source reader cannot list, and they have their own
    lock (`test_priority_overrides_name_existing_tests`)."""
    pairs = [(binary, prefix + name)
             for binary, entries in units.items()
             for prefix, text in entries
             for name in listed_test_names(text)]
    dead = []
    for profile, prof in sorted(config.get("profile", {}).items()):
        for ov in prof.get("overrides", []):
            if "priority" in ov:
                continue
            match = slow_tests.compile_filter(ov["filter"])
            if not any(match(b, n) for b, n in pairs):
                dead.append((profile, ov["filter"]))
    return dead


def ungrouped_boots(rules, units):
    """`[binary test]` of every colony-booting test (`boots_a_colony`, judged
    on its own stem) that lands in neither `colony-boot` nor `webkit`, with
    the binary and the name the harness table gives it."""
    missing, booting = [], 0
    for binary, entries in sorted(units.items()):
        for prefix, text in entries:
            stem = prefix[:-2] if prefix else binary.split("::", 1)[1]
            if not boots_a_colony("%s::%s" % (binary.split("::", 1)[0], stem), text):
                continue
            for name in listed_test_names(text) or [""]:
                booting += 1
                if group_of(rules, binary, prefix + name) not in ("colony-boot", "webkit"):
                    missing.append("%s %s" % (binary, prefix + name))
    return booting, missing


def listed_test_names(text):
    return TEST_FN.findall(text)


def group_rules(config, profile):
    """`[(match, group)]` in the order nextest reads them for `profile`."""
    profiles = config.get("profile", {})
    chain = [profiles.get(profile, {})]
    if profile != "default":
        chain.append(profiles.get("default", {}))
    rules = []
    for prof in chain:
        for ov in prof.get("overrides", []):
            if "test-group" in ov:
                rules.append((slow_tests.compile_filter(ov["filter"]), ov["test-group"]))
    return rules


def group_of(rules, binary_id, test):
    """The ONE group of a test: the first rule that matches, or None."""
    for match, group in rules:
        if match(binary_id, test):
            return group
    return None


def boots_a_colony(binary_id, text):
    stem = binary_id.split("::", 1)[1]
    return DISPLAY_COLONY.search(text) is not None or (HEAVY.search(stem) is not None
                                                       and BOOTS.search(text) is not None)


class TestGroups(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = load_config()
        cls.binaries = built_test_binaries()

    def test_the_groups_and_their_widths(self):
        groups = self.config["test-groups"]
        self.assertEqual(groups["webkit"]["max-threads"], 1)
        self.assertEqual(groups["colony-boot"]["max-threads"], 4)
        for profile in self.config["profile"]:
            for ov in self.config["profile"][profile].get("overrides", []):
                if "test-group" in ov:
                    with self.subTest(profile=profile, filter=ov["filter"]):
                        self.assertIn(ov["test-group"], groups)

    def test_every_group_filter_is_inside_the_evaluated_subset(self):
        """Loud on a term the evaluator does not know, instead of a guess."""
        for profile in self.config["profile"]:
            group_rules(self.config, profile)
        with self.assertRaises(slow_tests.FilterError):
            slow_tests.compile_filter("kind(test) + binary_id(=a::b)")
        with self.assertRaises(slow_tests.FilterError):
            slow_tests.compile_filter("binary_id(=a::b) &")
        with self.assertRaises(slow_tests.FilterError):
            slow_tests.compile_filter("binary(/a/)")

    def test_the_browser_profiles_exist_with_their_own_junit(self):
        """V0 § 6: the browser station's two calls run with their own profile,
        so each writes its own JUnit file (`target/nextest/<profile>/`) and
        neither overwrites the one the slow lock reads from `default`."""
        profiles = self.config["profile"]
        import gate_plan  # the parts' profiles (GH #1161)
        for name in ("browser", "browser-ignored") + tuple(gate_plan.BROWSER_PROFILES):
            with self.subTest(profile=name):
                self.assertIn(name, profiles)
                self.assertEqual(profiles[name]["junit"]["path"], "junit.xml")
                # Inherited from `default`, never re-declared: one place. The one
                # exception is the Chromium part's width (OR-TB.S3.14): a single
                # test-group override and nothing else -- no timeout, no retry.
                if name == "browser-chromium":
                    self.assertEqual(
                        [{"filter": "binary_id(/_browser$/) & not test(/webkit/)",
                          "test-group": "chromium-part"}],
                        profiles[name].get("overrides"))
                else:
                    self.assertNotIn("overrides", profiles[name])

    def test_710_colony_is_six_tests(self):
        """Six lines, six tests, one boot each: three exits in two engines."""
        text = COLONY_710_FILE.read_text(encoding="utf-8")
        names = listed_test_names(text)
        self.assertEqual(len(names), 6, names)
        self.assertEqual(len(set(names)), 6, names)
        self.assertEqual(sorted(n for n in names if "chromium" in n),
                         sorted(n for n in names if "webkit" not in n))
        self.assertEqual(len([n for n in names if "webkit" in n]), 3, names)
        self.assertEqual(len([n for n in names if "chromium" in n]), 3, names)
        # Every one is ignored WITH a reason, and none loops over the lines.
        attrs = re.findall(r"(?m)^#\[ignore(\s*=\s*\"[^\"]+\")?\]", text)
        self.assertEqual(len(attrs), 6, attrs)
        self.assertTrue(all(a for a in attrs), attrs)
        self.assertNotIn("for line in LINES", text)
        # Each test drives exactly one line, and together they drive all six.
        bodies = re.findall(r"fn\s+(\w+)\(\)\s*\{\s*hold_the_line\(\"(L\d)\"\)\.await;\s*\}",
                            text)
        self.assertEqual(sorted(n for n, _ in bodies), sorted(names))
        self.assertEqual(sorted(run for _, run in bodies),
                         ["L1", "L2", "L3", "L4", "L5", "L6"])
        # The engine in the name is the engine of the line (L1-L3 Chromium).
        for name, run in bodies:
            engine = "chromium" if run in ("L1", "L2", "L3") else "webkit"
            self.assertIn(engine, name, (name, run))
        # The `webkit` filter picks exactly the three WebKit tests.
        pick = slow_tests.compile_filter("test(/webkit/)")
        self.assertEqual(len([n for n in names if pick(COLONY_710, n)]), 3)

    def test_the_split_locks_are_two_tests_each(self):
        for binary in SPLIT_LOCKS:
            names = listed_test_names(self.binaries[binary])
            with self.subTest(binary=binary):
                self.assertEqual(len(names), 2, names)
                self.assertEqual(len([n for n in names if "chromium" in n]), 1, names)
                self.assertEqual(len([n for n in names if "webkit" in n]), 1, names)
                self.assertNotIn('for engine in ["chromium", "webkit"]',
                                 self.binaries[binary])

    def test_webkit_group_holds_only_webkit_tests_of_710_colony(self):
        """The colony half runs its Chromium lines side by side and its WebKit
        lines one at a time: the `webkit` override narrows the binary to
        `test(/webkit/)`, as the sheet's does, and the Chromium lines fall
        through to `colony-boot`."""
        names = listed_test_names(self.binaries[COLONY_710])
        self.assertEqual(len(names), 6, names)
        for profile in ("default", "ci", "browser", "browser-ignored",
                        "browser-chromium", "browser-webkit"):
            rules = group_rules(self.config, profile)
            for name in names:
                with self.subTest(profile=profile, test=name):
                    got = group_of(rules, COLONY_710, name)
                    if "webkit" in name:
                        self.assertEqual(got, "webkit")
                    else:
                        self.assertNotEqual(got, "webkit")
            for binary in SPLIT_LOCKS:
                for name in listed_test_names(self.binaries[binary]):
                    with self.subTest(profile=profile, binary=binary, test=name):
                        got = group_of(rules, binary, name)
                        self.assertEqual(got == "webkit", "webkit" in name, got)
        rules = group_rules(self.config, "default")
        for name in names:
            if "webkit" not in name:
                self.assertEqual(group_of(rules, COLONY_710, name), "colony-boot", name)

    def test_the_webkit_group_holds_only_webkit_tests(self):
        """Narrow, tree-wide: a test lands in `webkit` only when its binary or
        its own name says WebKit."""
        rules = group_rules(self.config, "default")
        wide = []
        for binary, text in self.binaries.items():
            for name in listed_test_names(text) or [""]:
                if (group_of(rules, binary, name) == "webkit"
                        and "webkit" not in binary and "webkit" not in name):
                    wide.append("%s %s" % (binary, name))
        self.assertEqual(wide, [])

    def test_a_text_that_only_names_the_colony_support_boots_nothing(self):
        """Negative control (M7h): the support module taken in boots, its
        file name in a source pin does not."""
        self.assertTrue(boots_a_colony("c::x", '#[path = "support/display_colony.rs"]\nmod display_colony;\n'))
        self.assertTrue(boots_a_colony("c::x", "fn f() { display_colony::Boot::new(); }\n"))
        self.assertTrue(boots_a_colony("c::x", "use crate::display_colony;\n"))
        self.assertFalse(boots_a_colony("c::x", 'fn f() { read("tests/support/display_colony.rs"); }\n'))

    def test_nextest_group_colony_boot_lists_every_booting_test(self):
        """Every test that boots a colony is in `colony-boot`, or in `webkit`
        when it drives WebKit (a test is in ONE group; `webkit` is the
        narrower line and counts as cover). A new display-colony test that is
        not listed runs at full width and is named here."""
        rules = group_rules(self.config, "default")
        booting, missing = 0, []
        for binary, text in sorted(self.binaries.items()):
            if not boots_a_colony(binary, text):
                continue
            for name in listed_test_names(text) or [""]:
                booting += 1
                if group_of(rules, binary, name) not in ("colony-boot", "webkit"):
                    missing.append("%s %s" % (binary, name))
        # The tree has dozens of them; zero means the reader went blind.
        self.assertGreater(booting, 10)
        self.assertEqual(missing, [], "add them to a `colony-boot` override in "
                                      ".config/nextest.toml")

    def test_the_light_colony_tests_keep_full_width(self):
        """Negative control: a plain `bootstrap_from_filesystem` test outside
        the heavy classes is in no group -- the cap is for the heavy boots."""
        rules = group_rules(self.config, "default")
        # flat binaries and folded modules alike -- the fold moved most of the
        # light ones into harnesses, and a flat-only reader saw 6 (W5 delta selftest)
        light = [(b, "", t) for b, t in self.binaries.items()
                 if "bootstrap_from_filesystem" in t and not boots_a_colony(b, t)]
        light += [(b, stem + "::", t) for b, stem, t in folded_units()
                  if "bootstrap_from_filesystem" in t
                  and not boots_a_colony("%s::%s" % (b.split("::")[0], stem), t)]
        self.assertGreater(len(light), 10)
        # `long-first` is no boot cap: it holds the 47 measured longest tests by
        # name (GH #1164), pinned in test_pipeline_ir_path.py; the rest of their
        # binaries keeps full width (the `nothing_else` control there).
        boot_caps = ("colony-boot", "webkit")
        grouped = ["%s %s" % (b, p) for b, p, t in light
                   if any(group_of(rules, b, p + n) in boot_caps
                          for n in listed_test_names(t) or [""])]
        self.assertEqual(grouped, [])



class TestGroupsAfterTheFold(unittest.TestCase):
    """#1158 (S0): at full lane width the two groups are the only thing between
    the heavy boots and the watchdog (#1088). After the harness fold (#1153)
    a filter naming a binary of before the fold matches nothing and the group
    goes quiet without a word -- these tests read the binaries the harness
    table places, not the flat files."""

    @classmethod
    def setUpClass(cls):
        cls.config = load_config()
        cls.units = harness_binaries()
        cls.crates = {crate_name(d) for d in (REPO / "crates").iterdir()
                      if (d / "Cargo.toml").is_file()}

    def test_nextest_group_overrides_name_no_gone_binary(self):
        """Every override (groups, output, leashes), every profile: an exact
        `binary_id` names a binary of the tree, a regex one matches one."""
        self.assertGreater(len(self.units), 100)
        self.assertEqual(gone_binary_names(self.config, self.units, self.crates), [],
                         "a folded binary is `binary_id(=<c>::<harness>) & test(/^<stem>::/)`")
        # The reader bites: a stem folded into a harness is a gone name.
        folded = next(b for b, e in sorted(self.units.items()) if e and e[0][0])
        stem = self.units[folded][0][0][:-2]
        old = "%s::%s" % (folded.split("::", 1)[0], stem)
        fake = {"profile": {"default": {"overrides": [
            {"filter": "binary_id(=%s)" % old, "test-group": "colony-boot"},
            {"filter": "binary_id(=%s) & test(/^%s::/)" % (folded, stem), "test-group": "webkit"},
            {"filter": "binary_id(/^no-such-crate::/)", "success-output": "final"}]}}}
        self.assertEqual(gone_binary_names(fake, self.units, self.crates),
                         [("default", "binary_id(=%s)" % old),
                          ("default", "binary_id(/^no-such-crate::/)")])

    def test_every_non_priority_override_takes_a_test(self):
        """L-8 (tb-s10): a live `binary_id` is not a live filter -- its `test()`
        term can still name a test the fold renamed or a commit removed, and
        the override goes quiet without a word. Every override but `priority`
        (groups, quarantine, output) takes at least one test of the tree."""
        self.assertEqual(dead_override_filters(self.config, self.units), [])
        # The reader bites: a live binary with a dead test term is dead.
        live = next(b for b, e in sorted(self.units.items())
                    if any(listed_test_names(t) for _, t in e))
        fake = {"profile": {"default": {"overrides": [
            {"filter": "binary_id(=%s)" % live, "test-group": "colony-boot"},
            {"filter": "binary_id(=%s) & test(/^gone::/)" % live, "test-group": "webkit"},
            {"filter": "binary_id(=%s) & test(/^gone::/)" % live, "priority": 10}]}}}
        self.assertEqual(dead_override_filters(fake, self.units),
                         [("default", "binary_id(=%s) & test(/^gone::/)" % live)])

    def test_colony_boot_and_webkit_groups_match_folded_tests(self):
        """Each group takes at least one test in every profile that sees it, and
        every colony-booting test -- flat or folded -- is in one of the two."""
        for profile in self.config["profile"]:
            rules = group_rules(self.config, profile)
            hits = {}
            for binary, entries in self.units.items():
                for prefix, text in entries:
                    for name in listed_test_names(text) or [""]:
                        g = group_of(rules, binary, prefix + name)
                        if g:
                            hits[g] = hits.get(g, 0) + 1
            with self.subTest(profile=profile):
                for group in ("colony-boot", "webkit"):
                    if any(g == group for _, g in rules):
                        self.assertGreater(hits.get(group, 0), 0, group)
                self.assertGreater(hits.get("webkit", 0), 0)
        rules = group_rules(self.config, "default")
        booting, missing = ungrouped_boots(rules, self.units)
        self.assertGreater(booting, 10)
        self.assertEqual(missing, [], "add them to a `colony-boot` override in "
                                      ".config/nextest.toml (folded form for a harness)")
        # The reader bites: a booting stem folded into a harness, the override
        # still naming its binary of before, is a missing test.
        fake_units = {"meclaw-cells::display": [("x_boots::", "use display_colony;\n"
                                                  "#[tokio::test]\nasync fn t() {}\n")]}
        stale = [(slow_tests.compile_filter("binary_id(=meclaw-cells::x_boots)"), "colony-boot")]
        self.assertEqual(ungrouped_boots(stale, fake_units),
                         (1, ["meclaw-cells::display x_boots::t"]))
        folded = [(slow_tests.compile_filter(
            "binary_id(=meclaw-cells::display) & test(/^x_boots::/)"), "colony-boot")]
        self.assertEqual(ungrouped_boots(folded, fake_units), (1, []))


if __name__ == "__main__":
    unittest.main()
