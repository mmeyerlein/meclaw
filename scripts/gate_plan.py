#!/usr/bin/env python3
"""meclaw -- resolve a diff into the list of gate stations that must run.

WHY THIS EXISTS
===============
Until now every gate chain ran the full suite for every diff. A scripts-only
change fell through `scripts/test-tier.sh changed` ("no crate touched --
falling back to t1") and paid minutes of rustc for a Python edit. This module
is the ONE place that maps changed paths to stations; nothing downstream --
neither the shell runner nor CI -- may hard-code a station trigger again.

It plans, it does not run. The verdict semantics, the receipt JSON and the
gate-line format belong to the runner.

CLASSES (a path can carry several)
==================================
    rust_src      crates/<c>/{src/**,build.rs,Cargo.toml,tests/{common,support}/**}
    rust_test     crates/<c>/tests/<stem>.rs, crates/<c>/tests/<stem>/**
    workspace     Cargo.toml, Cargo.lock, .cargo/**, .config/nextest.toml,
                  crates/meclaw-testing/**, deny.toml, rust-toolchain*
    corridor      crates/meclaw-colony/src/colony.rs, .github/fixtures/**,
                  plans/**/expected_*_body.txt, .github/gates/corridor_byte_gates.sh
    template      templates/<name>/**          (templates/README.md -> catalogue)
                  -- selection by REFERENCE shape, see `_template_reference_re`
                     and `_names_the_shipped_root` (GH #713)
    catalogue     templates/README.md
    example       examples/<name>/**
    docs          docs/**, top-level *.md, plans/** (minus corridor/export fixtures)
                  -- "docs-only means no cargo" holds only while no test READS
                  the file; see rule 8 in `test_filter`
    corpus_source see CORPUS_SOURCES below
    evals_memory  workshop/evals/scenarios/**, workshop/fixtures/**memory-hive**,
                  workshop/evals/p5-longmemeval/tools/**
    evals_builder workshop/evals/builder-scenarios/**
    display_scenarios  templates/display/compose/scenarios/**
                  -- carries `template` as well (the path lies under
                     templates/), so it pulls the template stations with it.
                     The class of its own exists so the station is planned
                     when only the driver changes
    display_browser  workshop/tools/display-*, and the two Rust locks that
                  run the layout driver (BROWSER_LOCKS). Never `template`:
                  the driver lies under workshop/, which does not travel
    display_lab   workshop/tools/display-lab/**, scripts/tests/test_display_lab.py
                  -- the measuring library. Its prefix is the longer one and
                     wins: a tool that READS a screen is not the driver that
                     lays one out, and must not pull `browser:display`
    gate_infra    scripts/gate.sh, scripts/gate_plan.py, scripts/precheck.py,
                  scripts/tests/**, scripts/test-tier.sh, scripts/wave_retro.py,
                  scripts/retro/**, scripts/display_sync.py, .config/nextest.toml
                  (the profile that decides what may be retried is gate
                  infrastructure too)
    unwrap_infra  .github/gates/unwrap_budget.{py,txt}
    export_infra  plans/export-fixtures/**
    shell         **/*.sh
    ci            .github/workflows/**

STATIONS (S strand, I integration, R release, C ci)
===================================================
    precheck        the cheap form checks of the diff, FIRST and before the
                    cargo lock: `rustfmt --check` on the changed sources, the
                    sheet's byte ceiling, an ADR's anchor line, a `scripts/`
                    path a test reads without an export entry, a docs page
                    changed without its twin -- plus five findings that are
                    notes rather than judgements (`scripts/precheck.py`).
                    A RED stops the cargo stations: the runner skips every
                    station that needs a build, takes no lock, and runs the
                    cheap rest on (`gate.sh --no-precheck-stop` overrides).
                    Never in C: the published tree has no `plans/`, so three
                    of the checks have no input there, and ci's diff is often
                    the whole tree, where the artefact notes grade files
                    nobody changed. An empty diff plans nothing to check
    roadmap-anchors adr-anchors claims tree-rules   always
    corpus-committed  the seed corpus against the tree AS IT IS, before
                    anything regenerates it. NEVER in C, for the same reason
                    as `corpus`. NOTE in S (the strand still owes the commit),
                    RED in I/R (a committed corpus that lies is a finding)
    corpus          corpus_source -> `regenerate+check+librarian`, else
                    `check+librarian`; NEVER in C. Two checks, not one: the
                    seed corpus AND `build_librarian.py` (the R11 pair)
    catalogue       template/catalogue class; I/R always
    shellcheck      shell/gate_infra; C always
    gate-selftest   gate_infra; C always (resolver, runner, strand kit, wave
                    retro, merge driver, form station, the source mark of the
                    display scenarios and the quarantine grammar of nextest)
    display-lab     display_lab (workshop/tools/display-lab/** and its test);
                    I/R always; never in C -- workshop/ does not travel
    fmt             rust_src/rust_test/workspace
    clippy          rust_src/rust_test/workspace  (-p <crates> in S, else --workspace)
    unwrap-budget   rust_src/workspace/unwrap_infra; C always (cargo: it
                    shells out to `cargo clippy --workspace`)
    corridor        corridor; C always
    tests           see `test_filter`; planned but not run in C (shards run it).
                    An empty diff plans the t0 TIER, not the equivalent filter
                    expression -- see `T0_FLOOR`
    doctests        I/R with rust_src/workspace
    deny            workspace; R always. NOT in I for every diff (R-P2,
                    2026-09-19): the `workspace` class is what moves the
                    dependency graph it grades
    scenarios:*     see the template/example/evals triggers; I/R always --
                    EXCEPT `scenarios:memory`, which R plans always and I only
                    when the diff carries `evals_memory` or the memory-hive
                    template (R-P2, 2026-09-19: 43 integration runs,
                    16 317 s, no finding -- wave P, finding 04 § 3.4).
                    `scenarios:display` runs the scenarios of display-hive.md
                    § 11 against the real curator AND the reference model
                    against itself (development-rules § 10). It travels: the
                    driver and its scenario file live under templates/, so ci
                    runs it too -- it is NOT in CI_EXCLUDED, and it is planned
                    in I, R and C for EVERY diff, not only when one names it.
                    Second command: the cheap mark check
                    `display_sync.py --check-source`, a NOTE on stdout and
                    never a verdict (the Rust lock is what goes red)
    browser:display the B-proofs of display-hive.md § 5-9 in Chromium AND
                    WebKit (§ 6.7, R-23-10). TWO shapes, one station name:
                    S runs the SHEET half (scope `sheet`) -- a page the driver
                    builds out of `compose.py`'s own parts, ~30 s, no colony;
                    I and R run that AND the COLONY half (scope
                    `sheet+colony`), six boots against three exits in two
                    engines, 4-6 min, behind `#[ignore]` and
                    `--run-ignored all`. The colony half was release-only
                    until 0.39.0, and the first gate that ever ran it found it
                    red on three proofs (GH #746): a proof only the release
                    night reaches is a proof nobody sees, and the pass that
                    declares a wave done is where it belongs. In CI_EXCLUDED,
                    unlike `scenarios:display`: the driver lives under
                    workshop/, which never travels
    recall-harness  memory-hive template, recall sources, evals_memory; R always.
                    NOT in I for every diff (R-P2), same measure as
                    `scenarios:memory` and one trigger more: a recall source
                    under `crates/meclaw-cells/src/` reaches the harness and
                    not the scenario cases
    deny-advisories R always (the runner grades it NOTE, never RED)
    export-selftest export_infra; I/R always (seconds, pure Python)
    export-audit    I/R always, last station. TWO shapes, one station name:
                    R runs the FULL audit (scope `R1-R17`, cargo:1 --
                    make_export.py runs `cargo check --workspace
                    --all-targets` in a fresh target); I runs it DRY
                    (`--skip-cargo --rev HEAD`, scope `R1-R17 dry`, cargo:0
                    -- it builds nothing). `--rev HEAD` because the pass
                    judges the tree its receipt was written over; the release
                    audit keeps the `master` default. The dry run is seconds
                    and surfaces the cheap
                    findings of R2b/R2c/R5/R10 -- dead template references
                    in tests, tests reaching into a never-exported root,
                    name/domain patterns, relative links -- in the
                    integration pass instead of an hour later in the release

USAGE
=====
    gate_plan.py --mode {strand,integration,release,ci}
                 (--files-from FILE | --files PATH...) [--repo DIR]
                 [--format {json,tsv}]
    gate_plan.py --print scenario|ignored

`--files-from -` reads paths from stdin, one per line.

JSON:  {"mode", "classes": [...], "crates": [...], "stations": [...]}
       A station carries one `cmd` (argv list) or several `cmds`; a command
       that needs a working directory carries `cwd` (repo-relative). PATHS IN
       AN ARGV ARE RELATIVE TO THAT `cwd`, not to the repo root -- the runner
       `cd`s into it before it execs. The `tests` station carries
       `"run": false` in ci mode.
TSV:   EXACTLY FIVE tab-separated columns on every row, in this order:

           name <TAB> scope <TAB> cargo(0|1) <TAB> shell-quoted cmd <TAB> cwd

       This five-column shape is the contract with the runner. The fifth
       column is EMPTY, not absent, when the command needs no working
       directory -- so a shell reader must name five variables:

           while IFS=$'\t' read -r name scope cargo cmd cwd; do ...; done

       Four variables would swallow the cwd into `cmd` on every station that
       carries one, and `read` would leave `cwd` unset on every station that
       does not. One line per command: a station with several commands emits
       several lines under the same name. A row with a non-empty fifth column
       carries an argv whose paths are relative to that directory.

       The TSV has no room for the `run` flag, so it says what to RUN. In ci
       mode -- the one mode where a planned station is deliberately not run --
       read the JSON, which carries `"run": false`.
"""

import argparse
import collections
import glob as globmod
import json
import os
import re
import shlex
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

MODES = ("strand", "integration", "release", "ci")

# The scenario class. This is the ONLY copy: `scripts/test-tier.sh` no longer
# spells it out, it asks for it with `gate_plan.py --print scenario`. Change it
# here and the tiers and the per-diff selection move together.
# The `*_live` binaries are here for a second reason beside cost: they open a
# billed session against a third party, so they never ride a diff.
# `scripts/tests/test_gate_plan.py` locks the suffix rule against the tree.
SCENARIO = ('binary(/_demo$/) + binary(/_demo_/) + binary(/e2e/) '
            '+ binary(/^workshop_scenario$/) + binary(/^slack_live$/) '
            '+ binary(/^gpt_live_live$/) '
            '+ binary(/^harness_real_cli_smoke$/) + binary(/^audit_14_/)')

# Run artefacts of the two suites. They are committed, they change on every
# run, and they never justify a station -- so they carry no diff class at all.
# `gate_plan.py --print ignored` publishes the list, and `scripts/gate.sh`
# subtracts it before it calls a tree dirty in the receipt: the scenario
# stations REWRITE these files while the run is in progress, so without the
# subtraction every release gate ends `dirty: true` and the export refuses.
IGNORED = (
    "workshop/evals/scenarios/last_run.json",
    "workshop/evals/builder-scenarios/last_run.json",
)

# A SUPERSET of the source globs of `workshop/tools/build_librarian_seed.py`,
# not a mirror of them. Only the direction that matters is enforced: a source
# that feeds the librarian seed and is missing here would let a corpus edit
# travel without a regenerate, which is the one failure mode the seed gate
# exists against -- and that is all the drift test checks (coverage, not
# equality). The other direction is deliberate over-selection: the README
# entries feed `build_librarian.py`, the second half of the corpus station,
# rather than the seed generator, and an extra `regenerate` costs a second.
CORPUS_SOURCES = (
    "docs/meclaw-overview.en.md",
    "docs/cell-types.en.md",
    "docs/config.en.md",
    "docs/rewiring.en.md",
    "README.md",
    "templates/*/template.json",
    "templates/**/config.json",
    "templates/*/README.md",
    "workshop/cookbook/*.md",
    "workshop/references/README.md",
    "workshop/corpus/*/ITEM.md",
    "examples/organism/grow-*.json",
    "workshop/fixtures/negative/*/expected_error.json",
)

# The shell sources shellcheck reads. CI runs on the published export mirror,
# which has neither `plans/` nor `workshop/`, so only the first two patterns
# are used there (`SHELL_GLOBS[:2]`).
SHELL_GLOBS = ("scripts/*.sh", ".github/gates/*.sh", "plans/meclaw-os/*.sh",
               "workshop/tools/display-lab/*.sh")

# Stations that are never planned in ci mode. ci runs in the published tree:
# no workshop/, no plans/, and cargo-deny runs in its own job. Planning any of
# these there produces a red station or a silent duplicate run (GH #234).
CI_EXCLUDED = frozenset({
    # The form station reads what the published tree does not carry, and ci's
    # diff is often `git ls-files` -- see its entry in the docstring.
    "precheck",
    "corpus",            # workshop/tools/build_librarian*.py
    "corpus-committed",  # ... and its pre-check reads the same builder
    "scenarios:memory",  # workshop/evals/scenarios/
    "scenarios:builder",  # workshop/evals/builder-scenarios/
    "recall-harness",    # workshop/evals/p5-longmemeval/
    "browser:display",   # workshop/tools/display-layout-browser.mjs
    "display-lab",       # workshop/tools/display-lab/
    "export-selftest",   # plans/export-fixtures/
    "export-audit",      # plans/export-fixtures/
    "deny",              # the CI `deny` job runs the cargo-deny action itself
    "deny-advisories",   # ... and would double-run or skip here
})

# The order stations are reported in: cheap first, so a red anchor costs
# seconds instead of a full cargo build; the release audit is last because it
# reads the tree every earlier station just proved.
STATION_ORDER = (
    "precheck",
    "roadmap-anchors", "adr-anchors", "claims", "tree-rules",
    "corpus-committed", "corpus", "catalogue", "shellcheck", "gate-selftest",
    "display-lab",
    "fmt", "clippy", "unwrap-budget", "corridor",
    "tests", "doctests", "deny",
    "scenarios:memory", "scenarios:builder", "scenarios:display",
    "browser:display", "recall-harness",
    "deny-advisories", "export-selftest", "export-audit",
)


# The two Rust locks the station `browser:display` runs. They are named here and
# nowhere else: `gate_plan.py` is THE place a station trigger may be written down,
# and a test that changes the way a browser proof is driven must plan the station
# that drives it -- the `tests` filter would build the binary without ever saying
# which station owns it.
BROWSER_LOCKS = frozenset({
    "crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs",
    "crates/meclaw-cells/tests/710_the_colony_holds_in_both_engines_browser.rs",
})

# The empty-diff floor. `plan()` turns it into `scripts/test-tier.sh t0`
# rather than a `filter` run: the tier passes `--lib --bins` and builds only the
# unit targets, while the identical filterset would build every test binary in
# the workspace before selecting from it.
T0_FLOOR = "kind(lib) + kind(bin)"


class GatePlanError(Exception):
    """A diff the resolver refuses to plan for (e.g. a crate name mismatch)."""


Station = collections.namedtuple(
    "Station", "name scope cargo cmds cwds run", defaults=(None, True))


def station(name, scope, cargo, cmds, cwds=None, run=True):
    """Build a `Station`, defaulting the per-command cwd list to all-unset."""
    if cwds is None:
        cwds = [None] * len(cmds)
    if len(cwds) != len(cmds):
        # A caller bug inside this module, not a property of the diff.
        raise ValueError("station %s: %d commands but %d cwds"
                         % (name, len(cmds), len(cwds)))
    return Station(name, scope, cargo, cmds, cwds, run)


# --- path matching ----------------------------------------------------------

def _glob_re(pattern):
    """Compile one glob to a regex. `**` crosses `/`, `*` and `?` do not."""
    out, i = [], 0
    while i < len(pattern):
        ch = pattern[i]
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif ch == "*":
            out.append("[^/]*")
            i += 1
        elif ch == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(ch))
            i += 1
    return re.compile("^" + "".join(out) + "$")


_GLOB_CACHE = {}


def _matches(path, pattern):
    rx = _GLOB_CACHE.get(pattern)
    if rx is None:
        rx = _GLOB_CACHE[pattern] = _glob_re(pattern)
    return rx.match(path) is not None


def _norm(paths):
    """Repo-relative, slash-separated, de-duplicated, run artefacts dropped."""
    out = []
    for p in paths:
        p = p.strip().replace(os.sep, "/")
        # Only a literal `./` prefix is noise. `lstrip("./")` would eat the
        # dot of `.github/` and `.cargo/` and quietly drop two classes.
        while p.startswith("./"):
            p = p[2:]
        if not p or p in IGNORED or p in out:
            continue
        out.append(p)
    return out


# --- classification ---------------------------------------------------------

def _crate_of(path):
    """`crates/<c>/...` -> `<c>`, else None."""
    parts = path.split("/")
    if len(parts) >= 3 and parts[0] == "crates":
        return parts[1]
    return None


def _classes_of_path(path):
    """Every class this single path carries."""
    cls = set()
    parts = path.split("/")
    crate = _crate_of(path)

    if crate:
        rest = "/".join(parts[2:])
        if (rest.startswith("src/") or rest == "build.rs" or rest == "Cargo.toml"
                or rest.startswith("tests/common/") or rest.startswith("tests/support/")):
            cls.add("rust_src")
        elif rest.startswith("tests/"):
            cls.add("rust_test")
        if crate == "meclaw-testing":
            cls.add("workspace")

    if path in ("Cargo.toml", "Cargo.lock", "deny.toml", ".config/nextest.toml"):
        cls.add("workspace")
    if path.startswith(".cargo/") or path.startswith("rust-toolchain"):
        cls.add("workspace")

    # The byte gates are corridor infrastructure: the script that diffs the
    # frozen bodies is as load-bearing as the fixtures it diffs against.
    if (path == "crates/meclaw-colony/src/colony.rs"
            or path.startswith(".github/fixtures/")
            or path == ".github/gates/corridor_byte_gates.sh"
            or (path.startswith("plans/") and _matches(path, "plans/**/expected_*_body.txt"))):
        cls.add("corridor")

    # The unwrap ratchet is its own gate: touching the budget file or the
    # checker must re-run it, and neither is a Rust source. The `scripts/
    # check_*.py` anchors need no such class -- their stations run in every
    # mode anyway.
    if path in (".github/gates/unwrap_budget.py", ".github/gates/unwrap_budget.txt"):
        cls.add("unwrap_infra")

    if path == "templates/README.md":
        cls.add("catalogue")
    elif path.startswith("templates/") and len(parts) >= 3:
        cls.add("template")

    if path.startswith("examples/") and len(parts) >= 3:
        cls.add("example")

    if path.startswith("docs/") or (len(parts) == 1 and path.endswith(".md")):
        cls.add("docs")
    if (path.startswith("plans/") and "corridor" not in cls
            and not path.startswith("plans/export-fixtures/")):
        cls.add("docs")

    if any(_matches(path, g) for g in CORPUS_SOURCES):
        cls.add("corpus_source")

    if (path.startswith("workshop/evals/scenarios/")
            or path.startswith("workshop/evals/p5-longmemeval/tools/")
            or (path.startswith("workshop/fixtures/") and "memory-hive" in path)):
        cls.add("evals_memory")
    if path.startswith("workshop/evals/builder-scenarios/"):
        cls.add("evals_builder")

    # The pins of the display hive travel with the template, so the path also
    # carries `template` above and pulls the template stations. The class of
    # its own is what plans `scenarios:display` when the driver alone changes.
    if path.startswith("templates/display/compose/scenarios/"):
        cls.add("display_scenarios")

    # The measuring library: the one copy of the tools that read a running
    # screen (befund `04-struktur.md` section 8). Its contract is a unittest,
    # and a unittest no station plans is not a lock. The prefix is asked FIRST
    # and the browser prefix below excludes it: the library lies under
    # `workshop/tools/display-`, so without this every one of its files dragged
    # the two Rust locks and the Chromium/WebKit run behind a 5 s unittest.
    lab = (path.startswith("workshop/tools/display-lab/")
           or path == "scripts/tests/test_display_lab.py")
    if lab:
        cls.add("display_lab")

    # The browser half of the same document: the layout driver under workshop/ and
    # the two locks that run it. No `template` class comes with it -- workshop/ is a
    # FORBIDDEN_PREFIX of the export, which is why the station is in CI_EXCLUDED.
    if (path.startswith("workshop/tools/display-") and not lab) or path in BROWSER_LOCKS:
        cls.add("display_browser")

    # `.config/nextest.toml` is gate infrastructure as much as the runner is:
    # it decides which test may be retried, and `test_nextest_quarantine.py`
    # is the lock that keeps an entry from reaching nothing. It keeps
    # `workspace` as well -- a changed profile changes every test run.
    if (path in ("scripts/gate.sh", "scripts/gate_plan.py", "scripts/precheck.py",
                 "scripts/test-tier.sh", "scripts/wave_retro.py",
                 "scripts/display_sync.py", ".config/nextest.toml")
            or path.startswith("scripts/tests/")
            or path.startswith("scripts/retro/")):
        cls.add("gate_infra")

    if path.startswith("plans/export-fixtures/"):
        cls.add("export_infra")

    if path.endswith(".sh"):
        cls.add("shell")
    if path.startswith(".github/workflows/"):
        cls.add("ci")

    return cls


def classify(paths):
    """The union of the diff classes over `paths`."""
    out = set()
    for p in _norm(paths):
        out |= _classes_of_path(p)
    return out


def _templates_of(paths):
    """The template names the diff touches (first path segment under templates/)."""
    out = set()
    for p in _norm(paths):
        parts = p.split("/")
        if parts[0] == "templates" and len(parts) >= 3:
            out.add(parts[1])
    return out


def _examples_of(paths):
    """The example names the diff touches."""
    out = set()
    for p in _norm(paths):
        parts = p.split("/")
        if parts[0] == "examples" and len(parts) >= 3:
            out.add(parts[1])
    return out


def crates_of(paths, repo=None):
    """The crate directories the diff touches (rust_src or rust_test).

    A crate directory is the crate name. Where the manifest exists that
    assumption is VERIFIED rather than trusted: a `-p <dir>` built from a
    directory whose `Cargo.toml` names something else selects nothing and the
    gate would go green on an empty run.
    """
    root = REPO_ROOT if repo is None else repo
    out = set()
    for p in _norm(paths):
        crate = _crate_of(p)
        if crate and _classes_of_path(p) & {"rust_src", "rust_test"}:
            out.add(crate)
    for crate in sorted(out):
        manifest = os.path.join(root, "crates", crate, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        with open(manifest, encoding="utf-8") as fh:
            m = re.search(r'^\s*name\s*=\s*"([^"]+)"', fh.read(), re.MULTILINE)
        if m and m.group(1) != crate:
            raise GatePlanError(
                "crates/%s/Cargo.toml declares name = %r. The resolver builds "
                "`-p %s` and `rdeps(%s)` from the directory name; a mismatch "
                "selects nothing and the gate would pass on an empty run."
                % (crate, m.group(1), crate, crate))
    return out


# --- test sources -----------------------------------------------------------

_SOURCE_CACHE = {}


def _test_sources(repo):
    """`[(crate, stem, text)]` for every `crates/*/tests/*.rs` under `repo`."""
    root = REPO_ROOT if repo is None else repo
    cached = _SOURCE_CACHE.get(root)
    if cached is not None:
        return cached
    out = []
    crates_dir = os.path.join(root, "crates")
    if os.path.isdir(crates_dir):
        for crate in sorted(os.listdir(crates_dir)):
            tests_dir = os.path.join(crates_dir, crate, "tests")
            if not os.path.isdir(tests_dir):
                continue
            for entry in sorted(os.listdir(tests_dir)):
                if not entry.endswith(".rs"):
                    continue
                path = os.path.join(tests_dir, entry)
                if not os.path.isfile(path):
                    continue
                try:
                    with open(path, encoding="utf-8", errors="replace") as fh:
                        out.append((crate, entry[:-3], fh.read()))
                except OSError:
                    continue
    _SOURCE_CACHE[root] = out
    return out


def _binary_id(crate, stem):
    return "binary_id(=%s::%s)" % (crate, stem)


def _test_binary_exists(repo, crate, stem):
    """Is `<crate>::<stem>` an integration-test target that actually EXISTS?

    nextest rejects a `binary_id(=..)` that matches nothing -- not by ignoring
    the term, but with "operator didn't match any binary IDs" and "failed to
    parse filterset", which takes the WHOLE `tests` station down. So a term is
    only ever emitted for a file that is there to compile.

    `git diff --name-only` lists DELETIONS, so a diff naming a test file is no
    proof that the file is still in the tree; the same holds for any path whose
    stem is not a target (a leftover directory without a `main.rs`). Both are
    the shape cargo autodiscovers: `tests/<stem>.rs` or `tests/<stem>/main.rs`.
    """
    root = REPO_ROOT if repo is None else repo
    tests = os.path.join(root, "crates", crate, "tests")
    return (os.path.isfile(os.path.join(tests, stem + ".rs"))
            or os.path.isfile(os.path.join(tests, stem, "main.rs")))


def _sharers(repo, crate, stem):
    """Binaries that COMPILE `<stem>.rs` in as a module (shared test helpers).

    `mock_openai.rs` is its own binary and also `mod mock_openai;` in a dozen
    others; editing it changes all of them, so all of them are selected.
    """
    out = set()
    for c, s, text in _test_sources(repo):
        if c != crate or s == stem:
            continue
        if ("mod %s;" % stem) in text or ('"%s.rs"' % stem) in text:
            out.add(_binary_id(c, s))
    return out


# How close a `read_dir(` may sit below the line that named the template root
# and still be read as iterating it. Three lines covers the idiomatic
# `let dir = templates_root();` / `let mut names = vec![];` / `for e in
# fs::read_dir(dir)` and stops well short of the next function.
CATALOGUE_WINDOW = 3

# Tokens that make a `read_dir`/`glob` line an iteration over the CATALOGUE
# rather than over some directory the test happens to hold.
CATALOGUE_TOKENS = ("templates_root", '"templates', "templates/")

# The SHIPPED catalogue root, however a test spells it: the helper that
# resolves it (`templates_root()`, `templates_dir()`) or the literal path that
# builds it (`PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")`
# -- any literal ENDING in `/templates`). A bare `"templates"` segment is
# deliberately NOT one: `tempdir.join("templates")` is a fixture library the
# test writes itself, and a shipped template that changes says nothing about
# it. This is the anchor of two rules -- the catalogue sweep below and
# `_names_the_shipped_root` -- and it was one spelling short in both: the two
# real sweeps of the tree name their root `templates_dir()` and
# `"../../templates"`, so neither was read as a sweep and neither travelled
# with any template diff (GH #713).
_SHIPPED_ROOT_RE = re.compile(r'templates_root\(|templates_dir\(|"[^"]*/templates"')

# A test's own recursive walker, `fn walk(dir, ..)` called on the root it just
# resolved. It is not in CATALOGUE_WALKS because the `read_dir(` is inside the
# helper, pages away from the line that names the root.
_LOCAL_WALK_RE = re.compile(r'\bwalk\w*\(')

# A template path built by a format string: `format!("templates/{dir}")`,
# `panic!("templates/{name} declares no contract")`. WHICH template it names is
# a runtime value, so such a file counts for EVERY template -- over-selection
# by construction, and the only alternative is missing it.
CATALOGUE_FORMAT = "templates/{"

# The catalogue ROOT as an expression -- `templates_root()`, `repo("templates")`,
# `root.join("templates")` -- but NOT one narrowed to a single template by a
# following `.join(...)`: `templates_root().join("talky")` copies one template
# and says nothing about the rest of the catalogue.
CATALOGUE_ROOT_RE = re.compile(
    r'(?:templates_root\(\)|"templates")(?!\s*\)?\s*\.join\()')

# A directory walk of any shape. `scan_templates_dir(` is in here because it
# IS the catalogue scan the colony itself uses.
CATALOGUE_WALKS = ("read_dir(", "glob(", "WalkDir", "scan_templates_dir(")

# A function signature: `fn templates_root() -> PathBuf` DEFINES the root, it
# does not point a walk at it.
_FN_RE = re.compile(r"\s*(?:pub\s+)?fn\s+[A-Za-z_][A-Za-z0-9_]*\s*[(<]")


def _is_catalogue_wide(text):
    """Does this test enumerate the whole template catalogue?

    Such a test asserts something about EVERY shipped template, so any
    template diff must run it. Recognised in six forms:

    (a) it calls `shipped_templates(`;
    (b) one line carries `read_dir` together with a catalogue token
        (`templates_root`, `"templates`, `templates/`);
    (c) a `read_dir(` sits within `CATALOGUE_WINDOW` lines BELOW a line that
        named the catalogue root -- the two-statement form
        `let dir = templates_root(); ... for e in fs::read_dir(dir)`;
    (d) a `glob(` line spells `templates/*`;
    (e) it walks a directory anywhere AND names the catalogue ROOT as an
        expression somewhere -- `copy_tree(&repo("templates"), ..)`,
        `let root = templates_root();` followed 25 lines later by the helper
        that reads it. (b) and (c) see only one line at a time and miss both
        shapes, and missing them is UNDER-selection: such a test asserts over
        every shipped template. A root narrowed by a following `.join(...)`
        (`templates_root().join("talky")`) does NOT count -- that is one
        template -- and neither does the `fn templates_root()` signature;
    (g) a line names the SHIPPED root (`_SHIPPED_ROOT_RE`) and a walk of any
        shape -- `read_dir(`, `WalkDir`, or the file's own `walk(` -- follows
        within `CATALOGUE_WINDOW` lines. (b) and (c) know one spelling of the
        root and (e) another; `store_wake_failure_modes` (`let root = ...
        .join("../../templates"); walk(&root, ..)`) and
        `gh494_no_interior_marker_leaves_a_hive` (`fn templates_dir()`) read
        every shipped `config.json` through neither;
    (f) a line builds a template path with a format string --
        `format!("templates/{dir}")`, `panic!("templates/{name} ...")`. The
        name is a runtime value and cannot be resolved from the source, so the
        file counts for every template. Without this,
        `gh455_the_two_templates_ship` and `gh482_the_catalogue_says_no_by_name`
        fell out of every template diff -- under-selection.

    The earlier rule was "`read_dir(` anywhere AND `templates` anywhere in the
    file", which called 140 of 763 integration tests catalogue-wide. Almost all
    of those were a private `fn copy_cells(src, dst)` or `fn walk_rs(dir)` --
    a recursive helper over a directory PARAMETER -- in a file that elsewhere
    mentions its own template by name. Those files stay selected for their own
    template through the name rule; they were never enumerating the catalogue.
    """
    if "shipped_templates(" in text:
        return True
    lines = text.splitlines()
    for line in lines:
        if "read_dir" in line and (any(tok in line for tok in CATALOGUE_TOKENS)
                                   or _SHIPPED_ROOT_RE.search(line)):
            return True
        if "glob(" in line and "templates/*" in line:
            return True
        if CATALOGUE_FORMAT in line:
            return True
    for i, line in enumerate(lines):
        if "templates_root(" in line or "templates/" in line:
            for nxt in lines[i + 1:i + 1 + CATALOGUE_WINDOW]:
                if "read_dir(" in nxt:
                    return True
    if any(walk in text for walk in CATALOGUE_WALKS):
        for line in lines:
            if not _FN_RE.match(line) and CATALOGUE_ROOT_RE.search(line):
                return True
    for i, line in enumerate(lines):
        if _FN_RE.match(line) or not _SHIPPED_ROOT_RE.search(line):
            continue
        for nxt in lines[i + 1:i + 1 + CATALOGUE_WINDOW]:
            if any(walk in nxt for walk in CATALOGUE_WALKS) or _LOCAL_WALK_RE.search(nxt):
                return True
    return False


_TEMPLATE_REF_CACHE = {}


def _template_reference_re(name):
    """The shapes in which a test REFERENCES the template `name`.

    A bare `"<name>"` is NOT one of them. It used to be, and it made every
    template diff pull a third of the suite: `assistant`, `member`, `talky` and
    `display` are instance and cell names all over the example trees, and a
    tree that instantiates a cell called `assistant` says nothing about the
    template of that name. What counts is a path INTO the template, a versioned
    reference, or a declaration naming it as a template.

    Over-selection stays allowed -- these patterns deliberately match prose in
    comments too. To go back to the old behaviour, add `'"%s"' % n` as one more
    alternative below; that single line is the whole difference.
    """
    rx = _TEMPLATE_REF_CACHE.get(name)
    if rx is None:
        n = re.escape(name)
        parts = [
            # Anything but a name character ends the name: `/`, `"`, a space,
            # end of line -- and a closing backtick, because doc comments spell
            # it `templates/memory-hive` and a prose mention still counts.
            r'templates/%s(?![A-Za-z0-9_-])',
            r'"%s@',                           # a versioned reference
            r'join\("%s"\)',                   # a path built from the name
            r'\\?"template\\?"\s*:\s*\\?"%s(?=[@"\\])',  # a JSON declaration
            r'template\s*=\s*"%s(?=[@"])',     # attribute / TOML form
            r'template\("%s"\)',               # a helper call
        ]
        rx = _TEMPLATE_REF_CACHE[name] = re.compile(
            "|".join(part % n for part in parts), re.MULTILINE)
    return rx


# A test that reads a SHIPPED template without ever spelling its path. It
# resolves the catalogue root once -- `fn templates_root() -> PathBuf` over
# `CARGO_MANIFEST_DIR/../../templates` -- and joins a RUNTIME value onto it:
# `fn shipped(rel) { templates_root().join(rel) }`, called as
# `shipped("assistant/config.json")`. The literal `templates/assistant` is
# nowhere in the file, so `_template_reference_re` does not see it; the diff
# path `templates/assistant/config.json` is nowhere either, so rule 8 does
# not; and the root is narrowed by a `.join(`, so `_is_catalogue_wide` does
# not. `gh529_the_menu_merges_every_answerers_declarations` and
# `gh561_the_pack_rides_a_v_lane` read exactly that file and fell out of the
# strand gate that changed it -- both went red on the integration branch
# afterwards (GH #713).
#
# The root must be the SHIPPED one: `tempdir.join("templates").join(name)`
# builds a fixture library the test writes itself, and a shipped template that
# changes says nothing about it. So only a root spelled `templates_root(` or a
# literal path ENDING in `/templates` ("../../templates") counts -- a bare
# `"templates"` segment does not.
# What is left of the path below the root is not the resolver's business:
# `join(rel)`, `join(name)` and `join("memory-hive/embed/config.json")` are the
# same reach through the same helper. Requiring a RUNTIME join kept
# `gh204_the_shipped_embedding_generation_agrees` --
# `templates_root().join("memory-hive/embed/config.json")` -- out of that
# file's own diff, which is the defect of GH #713 one spelling further on.


def _names_the_shipped_root(text):
    """Does this test reach into the shipped catalogue at all?"""
    return bool(_SHIPPED_ROOT_RE.search(text))


_ROOT_RELATIVE_CACHE = {}


def _root_relative_reference_re(name):
    """The template `name` as it arrives at such a helper, root-relative.

    Two shapes, and only these two:

        "<name>/..."        a path INTO the template -- `"assistant/config.json"`
        ([,) "<name>"       the bare name HANDED to something -- `shipped(
                            "assistant", FILES)`, `&["collector"]`

    The second shape is why the bare name may not count on its own anywhere
    else: `{"origin": "assistant"}` is an LLM role, and it is in a third of
    the suite. A delimiter in front of it is a cheap filter, not a proof --
    `turn("assistant", ..)` is that same role handed to a helper, and
    `cfg["tools"]` is a Rust index. It may over-select; what it buys is that
    the JSON-body form, which is most of them, drops out: measured on the tree
    that is 3 binaries selected for `templates/assistant` instead of 18.
    """
    rx = _ROOT_RELATIVE_CACHE.get(name)
    if rx is None:
        n = re.escape(name)
        rx = _ROOT_RELATIVE_CACHE[name] = re.compile(
            r'"%s/|[(,\[]\s*&?"%s"' % (n, n))
    return rx


def _tests_referencing_template(repo, name):
    ref = _template_reference_re(name)
    rel = _root_relative_reference_re(name)
    out = set()
    for crate, stem, text in _test_sources(repo):
        if ref.search(text) or _is_catalogue_wide(text):
            out.add(_binary_id(crate, stem))
        elif _names_the_shipped_root(text) and rel.search(text):
            out.add(_binary_id(crate, stem))
    return out


def _path_literals(path):
    """The spellings under which a test may name `path`.

    `docs/X.en.md` also travels as `docs/X.md`: the export maps the English
    bytes onto the public name, and tests in this tree cite both (8 files name
    `docs/config.md`, one names `docs/config.en.md`). Selecting only the spelling
    the diff happens to use would miss the readers of the other.
    """
    out = [path]
    if path.endswith(".en.md"):
        out.append(path[:-len(".en.md")] + ".md")
    return out


def _tests_reading_path(repo, path):
    """Binaries that name `path` as a literal -- rule 8.

    A test that READS a file is that file's drift lock. `gh466_the_seed_gate_is
    _wired.rs` opens `.github/workflows/ci.yml` and asserts a wording; a diff to
    that workflow that did not run it left the lock unrun. The rule generalises
    the template and example rules to every path outside `crates/`: docs, plans,
    workflows, CONTRIBUTING.md, README.md, workshop material.
    """
    literals = _path_literals(path)
    out = set()
    for crate, stem, text in _test_sources(repo):
        if any(lit in text for lit in literals):
            out.add(_binary_id(crate, stem))
    return out


def _tests_referencing_example(repo, name):
    out = set()
    needle = "examples/%s" % name
    for crate, stem, text in _test_sources(repo):
        if needle in text:
            out.add(_binary_id(crate, stem))
    return out


# --- the test filter --------------------------------------------------------

def test_filter(paths, mode, repo=None):
    """The nextest filterset for this diff, or None when no test must run.

    1. workspace class            -> all()
    2. rust_src in crate X        -> rdeps(X)
    3. rust_test file             -> its binary_id, plus every binary that
                                     compiles it in as a module -- but ONLY if
                                     the target still exists in the tree
                                     (`_test_binary_exists`): a deleted test
                                     cannot run, and nextest treats an
                                     unmatched `binary_id(=..)` as a hard
                                     error that kills the whole station
    4. template <name>            -> the binaries that REFERENCE it
                                     (`_template_reference_re` -- a bare
                                     `"<name>"` does not count), the ones that
                                     name the shipped root and `<name>`
                                     below it (`_names_the_shipped_root`),
                                     plus the catalogue-wide ones
                                     (`_is_catalogue_wide`)
    5. example <name>             -> the binaries that reference it
    6. none of the above, diff not empty -> None (no `tests` station at all)
    7. empty diff                 -> the t0 floor, `T0_FLOOR`; `plan()` turns
                                     that one value into `test-tier.sh t0`
    8. any path outside `crates/` -> the binaries that name it as a literal
                                     (`_tests_reading_path`). A test that reads
                                     a file is that file's drift lock, so the
                                     file's diff must run it. Additive: it
                                     extends rules 4 and 5, it replaces neither.

    Over-selection is allowed here, under-selection is not: a missed binary is
    a gate that passes over an untested change.
    """
    files = _norm(paths)
    if not files:
        return T0_FLOOR

    classes = classify(files)
    if "workspace" in classes:
        terms = {"all()"}
    else:
        terms = set()
        for crate in crates_of(files, repo):
            if any("rust_src" in _classes_of_path(p) and _crate_of(p) == crate
                   for p in files):
                terms.add("rdeps(%s)" % crate)
        for p in files:
            if "rust_test" not in _classes_of_path(p):
                continue
            crate = _crate_of(p)
            stem = p.split("/")[3].removesuffix(".rs")
            # A path the diff names may be GONE -- `git diff --name-only`
            # lists deletions. An id for a binary that does not exist is not
            # an over-selection, it is a filterset nextest refuses to parse.
            # Rules 4, 5 and 8 are safe without this check: they derive every
            # id from a file they just READ out of the tree.
            if not _test_binary_exists(repo, crate, stem):
                continue
            terms.add(_binary_id(crate, stem))
            terms |= _sharers(repo, crate, stem)
        for name in _templates_of(files):
            terms |= _tests_referencing_template(repo, name)
        for name in _examples_of(files):
            terms |= _tests_referencing_example(repo, name)
        for p in files:
            if not p.startswith("crates/"):
                terms |= _tests_reading_path(repo, p)

    if not terms:
        return None
    expr = " + ".join(sorted(terms))
    if mode == "ci":
        # CI shards run the tests; the expression is planned, not reduced.
        return expr
    return "%s - (%s)" % (expr, SCENARIO)


# --- scope helpers ----------------------------------------------------------

def _case_scope(repo, rel):
    """`<n> cases` counted from the case directory, or `cases` when absent."""
    root = REPO_ROOT if repo is None else repo
    path = os.path.join(root, rel)
    if not os.path.isdir(path):
        return "cases"
    return "%d cases" % len([f for f in os.listdir(path) if f.endswith(".json")])


def _scenario_scope(repo, rel):
    """`<n> scenarios` counted from the scenario file, or `scenarios` when absent.

    The number stands in every GATE line and in every receipt, so a scenario that
    silently left the file is visible in the pass that ran without it.
    """
    root = REPO_ROOT if repo is None else repo
    path = os.path.join(root, rel)
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return "scenarios"
    return "%d scenarios" % len(data.get("scenarios", ()))


def _shell_files(repo, globs):
    """Expand the shellcheck globs against the tree; patterns as a fallback.

    The expansion reads `repo`, but the argv it produces is repo-RELATIVE --
    the runner executes it from the repo root, which for a `--repo` pointing
    somewhere else is not the tree that was globbed. That combination only
    occurs in tests, so it stays a note rather than a second path model.
    """
    root = REPO_ROOT if repo is None else repo
    out = []
    for pattern in globs:
        for p in sorted(globmod.glob(os.path.join(root, pattern))):
            out.append(os.path.relpath(p, root).replace(os.sep, "/"))
    return out or list(globs)


# --- the plan ---------------------------------------------------------------

def plan(paths, mode, repo=None):
    """The stations this diff must run, cheapest first."""
    if mode not in MODES:
        raise GatePlanError("unknown mode: %s (expected %s)"
                            % (mode, "|".join(MODES)))
    files = _norm(paths)
    classes = classify(files)
    crates = sorted(crates_of(files, repo))
    templates = _templates_of(files)
    examples = _examples_of(files)
    ci = mode == "ci"
    irc = mode in ("integration", "release", "ci")
    ir = mode in ("integration", "release")
    check = ["--check"] if ci else []

    out = {}

    # --- form, before anything takes the lock. A red form station costs the
    # queue and the build for a `cargo fmt` somebody forgot; measured over
    # three waves, that was four of twenty-three red strand runs (5 771 s).
    # The paths travel on the ARGV, not on stdin: the runner runs every
    # station with stdin closed, because a station that reads it eats the
    # rest of its own command list.
    if files and not ci:
        out["precheck"] = station(
            "precheck", "form", False,
            [["python3", "scripts/precheck.py", "--files"] + list(files)])

    # --- always: the anchor and claim gates. Seconds, and they are the ones
    # that catch a doc that promised something the code no longer carries.
    out["roadmap-anchors"] = station(
        "roadmap-anchors", "ROADMAP.md", False,
        [["python3", "scripts/check_roadmap_anchors.py"] + check])
    out["adr-anchors"] = station(
        "adr-anchors", "plans/adr", False,
        [["python3", "scripts/check_adr_anchors.py"] + check])
    out["claims"] = station(
        "claims", "claims.tsv", False,
        [["python3", "scripts/check_claims.py"] + check])
    out["tree-rules"] = station(
        "tree-rules", "templates+examples", False,
        [["python3", "scripts/check_tree_rules.py"]])

    # --- corpus. A touched source regenerates and then checks; otherwise the
    # check alone proves the committed seed still matches its sources. CI never
    # regenerates -- it has nowhere to put the result.
    # NEVER in ci. The CI runs on the published export mirror and `workshop/`
    # does not travel, so the seed builder is not there at all -- which is why
    # `.github/workflows/ci.yml` forbids the step and the Rust twin
    # `librarian_seed_corpus.rs` skips cleanly in the public tree (GH #234).
    # Planning it there would be red on the first run, and planning the
    # REGENERATE half would be red on a tree CI cannot write back to anyway.
    #
    # TWO checks, not one: the seed corpus AND `templates/builder-librarian`
    # against the source it was generated from. The old R11 stage ran both, and
    # a station that ran only the first would let a drifted librarian template
    # through. If `build_librarian.py --check` goes red after a corpus
    # regenerate, regenerate the template with
    # `python3 workshop/tools/build_librarian.py` in the same strand.
    seed = ["python3", "workshop/tools/build_librarian_seed.py"]
    librarian = ["python3", "workshop/tools/build_librarian.py", "--check"]
    if not ci:
        # The pre-check, and it must come BEFORE `corpus` in STATION_ORDER.
        # `corpus` regenerates first and checks second, so it grades the file
        # it has just written: a committed corpus that no longer describes its
        # sources was GREEN in every mode. Measured 2026-09-05, two strands
        # with version bumps in `templates/*/template.json`: `docs.jsonl`
        # travelled with the OLD versions and no station ever said so
        # (GH #596). `--check` regenerates into a temp file and byte-compares
        # against the committed path -- run against the tree as it is, it is
        # the only station that reads what would actually travel.
        #
        # It costs a quarter of a second, so it runs in every non-ci mode
        # regardless of the diff: a stale corpus is stale whoever last touched
        # it, and the strand that finds it is the cheapest place to say so.
        out["corpus-committed"] = station(
            "corpus-committed", "seed as committed", False,
            [seed + ["--check"]])
        if "corpus_source" in classes:
            out["corpus"] = station(
                "corpus", "regenerate+check+librarian", False,
                [list(seed), seed + ["--check"], librarian])
        else:
            out["corpus"] = station(
                "corpus", "check+librarian", False,
                [seed + ["--check"], librarian])

    if classes & {"template", "catalogue"} or ir:
        out["catalogue"] = station(
            "catalogue", "templates/README.md", False,
            [["python3", "scripts/check_catalogue.py"]])

    if classes & {"shell", "gate_infra"} or ci:
        globs = SHELL_GLOBS[:2] if ci else SHELL_GLOBS
        out["shellcheck"] = station(
            "shellcheck", " ".join(globs), False,
            [["shellcheck"] + _shell_files(repo, globs)])

    if "gate_infra" in classes or ci:
        out["gate-selftest"] = station(
            "gate-selftest", "unittest", False,
            # One command, every module: the resolver's own tests, the
            # runner's, the strand kit's, the wave retro's and the merge
            # driver's. They all arrive with the runner (ROOT_FILES), so this
            # list is the same in both trees; the retro's one private-tree
            # case skips itself there.
            [["python3", "-m", "unittest",
              "scripts.tests.test_gate_plan", "scripts.tests.test_gate_sh",
              "scripts.tests.test_strand_sh",
              "scripts.tests.test_wave_receipt",
              "scripts.tests.test_wave_retro",
              "scripts.tests.test_git_merge_display_sync",
              "scripts.tests.test_precheck",
              "scripts.tests.test_display_sync",
              "scripts.tests.test_nextest_quarantine"]])

    # `ir` as well as the class: the library is what every display measurement
    # of the next wave is read with, it costs five seconds, and a pass that
    # declares a wave done is exactly where a broken instrument has to surface
    # (the same reasoning as `browser:display` below, GH #746).
    if (ir or "display_lab" in classes) and not ci:
        out["display-lab"] = station(
            "display-lab", "unittest", False,
            [["python3", "-m", "unittest", "scripts.tests.test_display_lab"]])

    # --- cargo work.
    if classes & {"rust_src", "rust_test", "workspace"}:
        out["fmt"] = station("fmt", "workspace", True,
                             [["cargo", "fmt", "--all", "--", "--check"]])
        if "workspace" in classes or irc or not crates:
            selector, scope = ["--workspace"], "workspace"
        else:
            selector = [a for c in crates for a in ("-p", c)]
            scope = " ".join(crates)
        out["clippy"] = station(
            "clippy", scope, True,
            [["cargo", "clippy"] + selector + ["--all-targets", "--", "-D", "warnings"]])

    if classes & {"rust_src", "workspace", "unwrap_infra"} or ci:
        # cargo:1 -- it is a Python script, but it runs
        # `cargo clippy --workspace` under the hood, so the runner must give it
        # the same nice/ionice/flock and build budget as any other cargo work.
        out["unwrap-budget"] = station(
            "unwrap-budget", "workspace", True,
            [["python3", ".github/gates/unwrap_budget.py"]])

    if "corridor" in classes or ci:
        out["corridor"] = station(
            "corridor", "colony.rs", False,
            [[".github/gates/corridor_byte_gates.sh"]])

    expr = test_filter(files, mode, repo)
    if expr == T0_FLOOR:
        # The floor is a TIER, not a filterset: `test-tier.sh t0` passes
        # `--lib --bins`, so nextest builds the unit targets only. Handing the
        # same expression to `filter` would compile all 650+ test binaries
        # first and then run a handful of them -- minutes of rustc for the
        # cheapest station in the table.
        out["tests"] = station(
            "tests", "t0 floor", True,
            [["scripts/test-tier.sh", "t0"]], run=not ci)
    elif expr is not None:
        out["tests"] = station(
            "tests", expr, True,
            [["scripts/test-tier.sh", "filter", expr]], run=not ci)

    if ir and classes & {"rust_src", "workspace"}:
        out["doctests"] = station("doctests", "workspace", True,
                                  [["cargo", "test", "--workspace", "--doc"]])

    # `workspace` only, plus the release (R-P2, 2026-09-19): cargo-deny grades
    # the dependency graph, and the `workspace` class is what moves that graph.
    # 39 integration runs over five waves, 0 RED (wave P, finding 04 § 3.2-3.4).
    # The release keeps it unconditional -- it is the run that ships the tree.
    if "workspace" in classes or mode == "release":
        out["deny"] = station(
            "deny", "bans licenses sources", True,
            [["cargo", "deny", "check", "bans", "licenses", "sources"]])

    # --- the offline suites. They are Python, they need no network and no
    # model, and they are the only place the memory-hive state machine and the
    # declaration lane run as a whole.
    # Triggered by the diff, not by the mode (R-P2, 2026-09-19): the two
    # memory stations answer only about the memory hive, its recall lane and
    # their own cases. 43 integration runs over five waves, 16 317 s, not one
    # finding (wave P, finding 04 § 3.4). `release` keeps both unconditional.
    if mode == "release" or "evals_memory" in classes or "memory-hive" in templates:
        out["scenarios:memory"] = station(
            "scenarios:memory",
            _case_scope(repo, "workshop/evals/scenarios/cases"), False,
            [["python3", "workshop/evals/scenarios/run_scenarios.py"]])

    if (ir or "evals_builder" in classes
            or (templates - {"memory-hive"})
            or examples & {"meclaw-os", "organism"}):
        # The argv is relative to the `cwd` below, not to the repo root: the
        # runner `cd`s into it before it execs, so a repo-relative path would
        # resolve as <cwd>/<repo-relative path> and miss (measured 2026-09-04:
        # ".../builder-scenarios/workshop/evals/builder-scenarios/
        # run_builder_scenarios.py: No such file").
        out["scenarios:builder"] = station(
            "scenarios:builder",
            _case_scope(repo, "workshop/evals/builder-scenarios/cases"), False,
            [["python3", "run_builder_scenarios.py"]],
            cwds=["workshop/evals/builder-scenarios"])

    # The pins of the one normative display document (development-rules § 10).
    # Pure Python, seconds, no colony and no browser -- so it runs in S as soon
    # as the diff touches the template or the scenario file, and in I, R and C
    # ALWAYS. C is where the other scenario suites cannot run at all, because
    # workshop/ does not travel; this one does, and two seconds is not a price
    # worth making conditional (wave H, plan § 10 "Produces": I/R/C always).
    if ir or mode == "ci" or "display" in templates or "display_scenarios" in classes:
        out["scenarios:display"] = station(
            "scenarios:display",
            _scenario_scope(repo, "templates/display/compose/scenarios/scenarios.json"),
            False,
            [["python3",
              "templates/display/compose/scenarios/run_display_scenarios.py"],
             # The cheap half of the drift question (GH #753): does the mark
             # `SOURCE` still describe the living description tree? Seconds,
             # no cargo, and it never fails the station -- it prints a NOTE
             # and exits 0, because the verdict belongs to the Rust lock
             # `710_the_scenarios_run_against_the_curator`. Where the living
             # tree is absent (ci, a foreign clone) there is nothing to
             # compare and it says so.
             ["python3", "scripts/display_sync.py", "--check-source"]])

    # The B-proofs of the same document, in a real engine (§ 6.7, R-23-10). Two
    # shapes of one station, like `export-audit`: the SHEET half builds its page out
    # of `compose.py`'s own parts, needs no colony and costs about half a minute, so
    # a strand pays for it; the COLONY half boots the throwaway colony once and
    # drives three exits in two engines -- 4 to 6 minutes, a fifth of the pass's
    # whole budget. It is `#[ignore]`d in the tree, and the two modes that declare
    # work finished take it off the shelf.
    #
    # Both of them, and not `release` alone (GH #746): the station was written with
    # the colony half in `release` only, the release gate of 0.39.0 was the first
    # gate of any mode to run it, and it was red on three proofs that had stood
    # unmeasured since the day the arm was added. An expensive proof nobody reaches
    # until the release night is a proof nobody reads, and the pass that says a wave
    # is done is exactly where that has to be paid.
    #
    # `cargo: True` -- both halves are nextest runs and build test binaries, so they
    # belong under the same nice/ionice/flock/build-width hygiene as any other cargo
    # station. Through `test-tier.sh`, which owns the nextest profile and knows not
    # to take the cargo lock twice.
    #
    # No browser on this host is a SKIP, never a RED: the driver leaves with 3 and
    # the Rust locks return green (R2b). That judgement is the test's, not the
    # runner's -- `gate.sh` grades a station by its exit code alone.
    if ir or "display" in templates or "display_browser" in classes:
        colony = ir
        cmds = [["scripts/test-tier.sh", "filter", "binary(/710_the_sheet_holds/)"]]
        if colony:
            cmds.append(["scripts/test-tier.sh", "filter",
                         "binary(/710_the_colony_holds/)", "--run-ignored", "all"])
        out["browser:display"] = station(
            "browser:display", "sheet+colony" if colony else "sheet", True, cmds)

    recall_src = any(p.startswith("crates/meclaw-cells/src/") and "recall" in p
                     for p in files)
    if (mode == "release" or "memory-hive" in templates
            or "evals_memory" in classes or recall_src):
        # cwd-relative argv, same rule as `scenarios:builder` above.
        out["recall-harness"] = station(
            "recall-harness", "tier-1", False,
            [["python3", "recall_cases.py"]],
            cwds=["workshop/evals/p5-longmemeval/tools"])

    if mode == "release":
        out["deny-advisories"] = station(
            "deny-advisories", "advisories", True,
            [["cargo", "deny", "check", "advisories"]])

    if "export_infra" in classes or ir:
        # Seconds of pure Python, so the integration pass pays for it too --
        # it is the self-test of the very rules the audit below applies.
        out["export-selftest"] = station(
            "export-selftest", "drift-fixtures", False,
            [["python3", "plans/export-fixtures/test_drift_gate.py"]])

    if ir:
        # `{receipt}` is a placeholder the runner substitutes with the path it
        # wants the audit receipt written to. The runner ALWAYS substitutes it,
        # in every mode -- so `make_export.py` never falls back to its own
        # default receipt lookup from here.
        #
        # TWO shapes of the same station. Release runs the full audit; cargo:1
        # -- it is a Python script, but it runs `cargo check --workspace
        # --all-targets` in a fresh target directory inside the export tree.
        # That is a cold build, and it belongs under the same
        # nice/ionice/flock/build-width hygiene as any other cargo station.
        #
        # Integration runs the SAME audit with `--skip-cargo`: R8/R9/R12-class
        # work drops out, nothing is built (cargo:0), and what remains are the
        # cheap rules -- R2b dead template references in tests, R2c tests
        # reaching into a never-exported root, R5 name/domain patterns, R10
        # relative links. Those used to surface only in the
        # release pass, an hour after the integration pass had declared the
        # wave done (v0.30.0).
        #
        # `--rev HEAD` is the second half of the dry shape. `make_export.py`
        # defaults to `--rev master`, and the receipt the audit reads belongs
        # to the revision the RUNNER gated -- HEAD. On master the two are the
        # same commit, which is why the release audit never noticed; from a
        # wave branch they differ and the audit answered with a rev mismatch
        # instead of a verdict (measured 2026-09-04). The integration pass
        # judges the tree it just gated, so it names it. Release keeps the
        # `master` default: an export is of master, not of whatever is checked
        # out.
        dry = mode == "integration"
        argv = ["python3", "plans/export-fixtures/make_export.py", "--keep-going"]
        if dry:
            argv += ["--skip-cargo", "--rev", "HEAD"]
        out["export-audit"] = station(
            "export-audit", "R1-R17 dry" if dry else "R1-R17", not dry,
            [argv + ["--gate-receipt", "{receipt}"]])

    if ci:
        # One rule, not eight conditions spread through the builder above:
        # whatever the diff says, ci only plans what the published tree holds.
        for name in CI_EXCLUDED:
            out.pop(name, None)

    return [out[name] for name in STATION_ORDER if name in out]


# --- rendering --------------------------------------------------------------

def to_json(paths, mode, repo=None):
    stations = plan(paths, mode, repo)
    rows = []
    for st in stations:
        row = {"name": st.name, "scope": st.scope, "cargo": bool(st.cargo)}
        if len(st.cmds) == 1:
            row["cmd"] = st.cmds[0]
            if st.cwds[0]:
                row["cwd"] = st.cwds[0]
        else:
            row["cmds"] = st.cmds
            if any(st.cwds):
                row["cwds"] = list(st.cwds)
        if not st.run:
            row["run"] = False
        rows.append(row)
    return {
        "mode": mode,
        "classes": sorted(classify(paths)),
        "crates": sorted(crates_of(paths, repo)),
        "stations": rows,
    }


def to_tsv(paths, mode, repo=None):
    lines = []
    for st in plan(paths, mode, repo):
        for cmd, cwd in zip(st.cmds, st.cwds):
            lines.append("\t".join([
                st.name, st.scope, "1" if st.cargo else "0",
                " ".join(shlex.quote(a) for a in cmd), cwd or ""]))
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="gate_plan.py",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Resolve changed paths into the gate stations that must run.",
        epilog="TSV output has EXACTLY five tab-separated columns on every row:\n"
               "  name <TAB> scope <TAB> cargo(0|1) <TAB> shell-quoted cmd <TAB> cwd\n"
               "The fifth column is empty, not absent, when no working directory\n"
               "is needed, so a shell reader must name all five variables:\n"
               "  while IFS=$'\\t' read -r name scope cargo cmd cwd; do ...; done\n"
               "The `run` flag lives in the JSON only; ci mode plans the `tests`\n"
               "station with \"run\": false because the CI shards run it.")
    ap.add_argument("--mode", choices=MODES)
    ap.add_argument("--files-from", metavar="FILE",
                    help="read paths from FILE, one per line ('-' = stdin)")
    ap.add_argument("--files", nargs="*", default=[], metavar="PATH")
    ap.add_argument("--repo", metavar="DIR",
                    help="tree the test sources are read from (default: this repo)")
    ap.add_argument("--format", choices=("json", "tsv"), default="tsv")
    ap.add_argument("--print", dest="what", choices=("scenario", "ignored"),
                    help="print a constant and exit (`ignored`: one path per line)")
    args = ap.parse_args(argv)

    if args.what == "scenario":
        print(SCENARIO)
        return 0

    if args.what == "ignored":
        # One path per line, for `scripts/gate.sh`: a run artefact that the
        # suites rewrite while the gate runs must not make the tree look dirty
        # in the receipt, or every release refuses on its own scenario station.
        for path in IGNORED:
            print(path)
        return 0

    if not args.mode:
        ap.error("--mode is required (or use --print)")

    paths = list(args.files)
    if args.files_from:
        if args.files_from == "-":
            paths += sys.stdin.read().splitlines()
        else:
            try:
                with open(args.files_from, encoding="utf-8") as fh:
                    paths += fh.read().splitlines()
            except OSError as exc:
                ap.error("--files-from %s: %s" % (args.files_from, exc.strerror))

    try:
        if args.format == "json":
            print(json.dumps(to_json(paths, args.mode, args.repo), indent=2))
        else:
            text = to_tsv(paths, args.mode, args.repo)
            if text:
                print(text)
    except GatePlanError as exc:
        print("gate_plan: %s" % exc, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
