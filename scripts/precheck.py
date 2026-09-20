#!/usr/bin/env python3
"""meclaw -- the cheap form checks of a diff, before anything takes the lock.

WHY THIS EXISTS
===============
A gate run that goes red on form costs the same as one that goes red on
behaviour: the queue in front of the shared cargo lock, then the build, then
the wait. Measured over the waves H2, H3 and G0: four of twenty-three red
strand runs (5 771 s of gate time) were pure form -- a forgotten `cargo fmt`,
the sheet two hundred bytes over its ceiling, an ADR without its anchor line.
Fifty of a hundred and eleven review findings were form as well, and each of
those costs a fix round on top.

None of it needs a build. `rustfmt` reads a file, the sheet is a byte count,
an anchor line is a `grep`. This module is the station that says so in
seconds, before the run joins the queue -- station `precheck`, scope `form`,
no cargo, and the FIRST station of a strand, an integration pass and a release.
Which modes plan it, and why ci does not, is in the docstring of
`gate_plan.py`: the stations live there and nowhere else.

WHAT IT GRADES
==============
RED -- the run stops for these, because the tree is wrong and every later
station would grade it anyway:

    fmt             `rustfmt --check` on the changed `.rs` files. Not
                    `cargo fmt`: that resolves the workspace and wants the
                    lock. The edition comes from the root manifest.
    sheet-cap       the shipped sheet, stripped by the rule of
                    `display_sheet_strip.py`, against the ceiling the lock
                    `gh669_every_class_the_screen_writes_has_a_rule` holds --
                    minus the air that lock reserves. A lower bound on the
                    shell (the shell adds its own markup around the sheet), so
                    this is the early warning and that lock is the verdict.
    adr-pinned-by   an ADR without the `Pinned-by:` line that names the test
                    or symbol carrying the decision (development-rules § 2b).
    root-files      a `scripts/` path a test READS that the export does not
                    carry (`ROOT_FILES`), whichever half of the pair the diff
                    carries. Green here, red in the published tree an hour
                    later -- the R2c class, found by a review in wave G0
                    instead of by a gate.
    docs-twin       `docs/X.md` changed without `docs/X.en.md`, or the other
                    way round, where both faces exist. The export compares
                    them for structural equality; half a change is a red gate.

NOTE -- read, do not stop. These are the review findings that repeat, and a
note before the review is a fix round saved:

    changelog-mark  a release block that names no issue, beside a diff that
                    changes a crate or a template version.
    since-sentence  a `since <version>` in a template README naming a version
                    NEWER than that template's own: a sentence written for a
                    release that has not happened (§ 4a).
    since-doc       a ``since `<template>@<version>` `` in `docs/` naming a
                    version that template has not reached, or the one THIS
                    diff bumps it to -- the shape of the finding that cost the
                    `voice-grace` strand its first review, where the sentence
                    moved onto the new number instead of naming the release
                    the behaviour arrived in.
    run-artefact    a run artefact (`last_run.json`) or a symlink in the diff.
                    Four incidents, each one a commit taken back or a review
                    round, none of them a lost gate run.
    name-pattern    the export's R5 patterns in a file that TRAVELS. The full
                    audit stays the verdict; this is the half that costs a
                    second.
    plan-cap        a strand's part of a plan over the 600 lines of
                    development-rules § 1 (ruling R-P1, 2026-09-19). The
                    parts themselves only, never a subdirectory under them:
                    `plan-parts/voll/` is where a wave keeps the un-shortened
                    copy as its record. A nudge, not a lock -- a planning
                    commit is often made without a gate, so the note speaks
                    only when the part is in a run's diff.

WHAT IT IS NOT
==============
It is not a second opinion on a station that already exists. `fmt` here reads
the changed files and cannot see a workspace-wide reformat; the `fmt` station
still runs. The point is WHEN: this one costs seconds and runs before the
queue, so a form finding is a fix and not a run.

A check whose input is missing says nothing. The published tree has no
`plans/`, so `root-files` and `name-pattern` -- both of which read the
export's own lists -- fall silent there rather than inventing a verdict.

USAGE
=====
    precheck.py (--files-from FILE | --files PATH...) [--repo DIR]

`--files-from -` reads paths from stdin, one per line. Output is one line per
finding:

    <RED|NOTE> <check>: <message>

Exit 1 when a RED finding was printed, 0 otherwise -- so notes alone leave the
station green, and the runner echoes them beside its GATE line.
"""

import argparse
import collections
import functools
import os
import re
import shutil
import subprocess
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

Finding = collections.namedtuple("Finding", "level check message")

# The sheet, its ceiling and the air the Rust lock reserves under it. Both
# numbers are that lock's
# (`crates/meclaw-cells/tests/gh669_every_class_the_screen_writes_has_a_rule.rs`):
# the ceiling has been raised three times, each time once and with a reason,
# and the air is a floor of its own since wave G -- what is left under the
# ceiling belongs to the next wave, not to whoever gets there first.
SHEET = "templates/display/compose/display-dna.css"
SHEET_CEILING = 98_000
SHEET_AIR = 3_000

# A strand's part of a plan, and the ceiling development-rules § 1 puts on it.
# The measurement behind the number: the builders of one wave opened 12 % of
# its 16 712 plan lines, and over three waves they recorded 188 rulings of
# their own -- the lines past the cap were read by nobody and decided nothing.
# The pattern holds the parts themselves and no subdirectory under them:
# `voll/` is the un-shortened copy a wave keeps as its record (wave G kept
# four), and grading the record would make the exemption unusable.
_PLAN_PART = re.compile(r"^plans/[^/]+/plan-parts/[^/]+\.md$")
PLAN_PART_CAP = 600

# The edition rustfmt parses with when the root manifest does not say.
EDITION_FALLBACK = "2021"

# Where the export keeps the two lists this module borrows.
MAKE_EXPORT = os.path.join("plans", "export-fixtures", "make_export.py")

# Roots that never travel (the export's FORBIDDEN_PREFIX). A file under one of
# them may carry anything: R5 is about the tree that is published.
NEVER_TRAVELS = re.compile(r"^(plans|ideas|archive|workshop)/")

_ADR = re.compile(r"^plans/adr/\d{4}-.*\.md$")
_ISSUE_MARK = re.compile(r"#\d+")
_VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
# `since 1.0.1`, ``since `3.4.0` ``, ``since `collector@2.1.1` `` -- and not
# `since GH #556`, which is an issue and not a version. Case-insensitive
# because the sentence is nearly always sentence-initial: `templates/assistant`
# alone writes `Since <version>` three times, and a case-sensitive pattern read
# none of them.
_SINCE = re.compile(r"\bsince\s+`?(?:([a-z0-9-]+)@)?(\d+\.\d+\.\d+)`?", re.IGNORECASE)


# --- the checks -------------------------------------------------------------

def check_fmt(files, repo):
    """`rustfmt --check` over the changed sources that still exist.

    Only under `crates/`: those are the workspace members, and they are what
    `cargo fmt --all` covers. The kata fixtures under `workshop/evals/` are
    Rust and are deliberately not formatted -- they are the input of a
    measurement, not a member of this workspace.
    """
    rust = [f for f in files
            if f.endswith(".rs") and f.startswith("crates/")
            and os.path.isfile(os.path.join(repo, f))]
    if not rust or not shutil.which("rustfmt"):
        return
    res = subprocess.run(
        ["rustfmt", "--check", "--edition", workspace_edition(repo)] + rust,
        cwd=repo, capture_output=True, text=True)
    if res.returncode == 0:
        return
    # rustfmt names what it would rewrite as `Diff in <absolute path>:<line>:`.
    # Both the absolute path and the line belong to rustfmt, not to a finding a
    # reader of this repository should be able to type.
    named = sorted({_relative(m.group(1), repo) for m in
                    re.finditer(r"^Diff in (.+?)(?::\d+:)?\s*$",
                                res.stdout, re.MULTILINE)})
    if named:
        yield Finding("RED", "fmt",
                      "rustfmt would rewrite %s -- run `cargo fmt`" % ", ".join(named))
        return
    # An exit of 1 is not by itself a formatting finding. rustfmt returns it
    # when it could not format at all -- a syntax error, an unresolvable `mod`
    # -- and then it names nothing and says why on stderr. Answering that with
    # "run `cargo fmt`" sends the strand after a fix that changes nothing, in
    # the FIRST station of the run.
    detail = " / ".join(res.stderr.strip().splitlines()[-3:])
    yield Finding("RED", "fmt",
                  "rustfmt could not read %s: %s"
                  % (", ".join(rust), detail or "exit %d, no output" % res.returncode))


def _relative(path, repo):
    """A path rustfmt printed, as this repository spells it."""
    try:
        rel = os.path.relpath(os.path.realpath(path), os.path.realpath(repo))
    except ValueError:           # pragma: no cover -- different drives, not here
        return path
    return path if rel.startswith("..") else rel.replace(os.sep, "/")


def workspace_edition(repo):
    """The edition of the root manifest, so rustfmt parses what cargo parses."""
    try:
        with open(os.path.join(repo, "Cargo.toml"), encoding="utf-8") as fh:
            m = re.search(r'^\s*edition\s*=\s*"([^"]+)"', fh.read(), re.MULTILINE)
    except OSError:
        return EDITION_FALLBACK
    return m.group(1) if m else EDITION_FALLBACK


def check_sheet_cap(files, repo):
    """The shipped sheet against the ceiling, with the wave's air kept free."""
    if SHEET not in files:
        return
    text = _read(os.path.join(repo, SHEET))
    if text is None:
        return
    shipped = len(_strip_sheet(text).encode("utf-8"))
    limit = SHEET_CEILING - SHEET_AIR
    if shipped > limit:
        yield Finding("RED", "sheet-cap",
                      "the stripped sheet weighs %d B, over the %d B a wave may use "
                      "(ceiling %d B minus %d B of air)"
                      % (shipped, limit, SHEET_CEILING, SHEET_AIR))


def _strip_sheet(text):
    """The rule of `display_sheet_strip.py` -- asked, never restated."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    try:
        import display_sheet_strip
    except ImportError:          # pragma: no cover -- the file travels with us
        return text
    return display_sheet_strip.strip(text)


def check_adr_pinned_by(files, repo):
    """Every ADR names the test or symbol that carries its decision."""
    for path in files:
        if not _ADR.match(path):
            continue
        text = _read(os.path.join(repo, path))
        if text is None or "Pinned-by:" in text:
            continue
        yield Finding("RED", "adr-pinned-by",
                      "%s has no `Pinned-by:` line -- name the test or symbol "
                      "that carries the decision" % path)


def check_root_files(files, repo):
    """A `scripts/` path a test READS must be in the export's `ROOT_FILES`.

    Three narrowings, and each one is a false positive the tree already
    carries. The path must appear as a WHOLE string literal (`"scripts/x.py"`)
    -- four tests name a script inside a longer failure message, which the
    published tree never executes. Comment lines do not count, for the same
    reason. And a test that GUARDS the path -- `.exists()` on the literal, or
    on the name it was bound to -- skips cleanly where the file is absent,
    which is the answer the export accepts elsewhere (its `PRESENCE_GUARD`).

    What is left is the shape that cost wave G0 a review finding: a constant
    holding the path, read unconditionally, with the file missing from
    `ROOT_FILES`. Green in this tree, red in the published one.

    BOTH DIRECTIONS. The diff may carry the script, and it may carry the test
    instead -- a new test reading a script that has been sitting there
    unexported for a year is the same defect seen from the other side, and
    grading only the `scripts/` paths of the diff makes that half invisible.
    """
    scripts = sorted(set(f for f in files if f.startswith("scripts/"))
                     | _scripts_named_by(files, repo))
    if not scripts:
        return
    export = _export(repo)
    root_files = getattr(export, "ROOT_FILES", None) if export else None
    if root_files is None:
        return
    for path in scripts:
        if path in root_files or not os.path.isfile(os.path.join(repo, path)):
            continue
        readers = _tests_reading(repo, path)
        if readers:
            yield Finding("RED", "root-files",
                          "%s is read by %s but missing from ROOT_FILES in %s -- "
                          "the published tree would not carry it (R2c)"
                          % (path, ", ".join(sorted(readers)), MAKE_EXPORT))


_IS_TEST = re.compile(r"^(scripts/tests/|crates/[^/]+/tests/)")
_SCRIPT_LITERAL = re.compile(r'"(scripts/[A-Za-z0-9_./-]+)"')


def _scripts_named_by(files, repo):
    """The `scripts/...` literals of the test sources this diff changes."""
    out = set()
    for path in files:
        if not _IS_TEST.match(path):
            continue
        text = _read(os.path.join(repo, path))
        if text is not None:
            out.update(_SCRIPT_LITERAL.findall(text))
    return out


def check_docs_twin(files, repo):
    """Both language faces of a page move in the same change.

    The realistic false alarm of the five red checks: a typo fixed in one face
    alone is structurally equal and would not tear the export gate, but it is
    red here. The house rule is that both faces move together (AGENTS.md rule
    7), so the check follows the rule rather than the gate -- and this is the
    line to remember when it fires on a one-word change.
    """
    seen = set(files)
    for path in files:
        twin = _twin(path)
        if twin is None or twin in seen:
            continue
        if not os.path.isfile(os.path.join(repo, twin)):
            continue
        if not os.path.isfile(os.path.join(repo, path)):
            continue
        yield Finding("RED", "docs-twin",
                      "%s changed without %s -- the export compares the two faces "
                      "for structural equality" % (path, twin))


def _twin(path):
    if not path.startswith("docs/") or not path.endswith(".md"):
        return None
    if path.endswith(".en.md"):
        return path[:-len(".en.md")] + ".md"
    return path[:-len(".md")] + ".en.md"


def check_changelog_mark(files, repo):
    """A release block beside a crate or template change names its issue."""
    if "CHANGELOG.md" not in files:
        return
    if not any(f.startswith("crates/") or
               (f.startswith("templates/") and f.endswith("/template.json"))
               for f in files):
        return
    text = _read(os.path.join(repo, "CHANGELOG.md"))
    if text is None:
        return
    block = _newest_block(text)
    if block and not _ISSUE_MARK.search(block):
        yield Finding("NOTE", "changelog-mark",
                      "the newest CHANGELOG block names no issue (`GH #<n>`), "
                      "beside a diff that changes a crate or a template version")


def _newest_block(text):
    """The first `## [...]` section that carries anything but its heading."""
    parts = re.split(r"^## ", text, flags=re.MULTILINE)[1:]
    for part in parts:
        body = part.split("\n", 1)[1] if "\n" in part else ""
        if body.strip():
            return part
    return ""


def check_since_sentence(files, repo):
    """A since-sentence in a template README, against that template's version.

    What this one catches is the FORWARD half: a sentence written for a version
    the template has not reached. The backward half -- a sentence pulled onto
    the version the diff bumps to -- is `check_since_doc`, because that is the
    shape the `voice-grace` finding had and it is a different comparison.
    """
    for path in files:
        parts = path.split("/")
        if len(parts) < 3 or parts[0] != "templates" or parts[-1] != "README.md":
            continue
        name = parts[1]
        own = _template_version(repo, name)
        if own is None:
            continue
        text = _read(os.path.join(repo, path))
        if text is None:
            continue
        others = _template_names(repo) - {name}
        for line in text.splitlines():
            for named, version in _SINCE.findall(line):
                if named and named != name:
                    continue                  # `since \`collector@2.1.1\``
                # A bare version belongs to whatever the sentence is about,
                # and a sentence that names another template is about that
                # one: "`memory-hive` has had since 2.2.0" is its version,
                # not this README's.
                if not named and any(re.search(r"\b%s\b" % re.escape(o), line)
                                     for o in others):
                    continue
                if _as_tuple(version) > own:
                    yield Finding(
                        "NOTE", "since-sentence",
                        "%s says `since %s` while %s/template.json is at %s -- "
                        "a since-sentence names what shipped (§ 4a)"
                        % (path, version, "templates/" + name, _dotted(own)))


def check_since_doc(files, repo):
    """A `<template>@<version>` since-sentence in `docs/`, against that template.

    The finding that cost `voice-grace` its first review stood in
    `docs/cell-types.md` (and its twin): ``since `voice@2.0.4` `` for a
    behaviour that shipped in 2.0.3, written in the diff that bumped the
    template to 2.0.4. Two ways to name a version that does not carry the
    behaviour, and both are read here: one the template has not reached, and
    one THIS diff is bumping to -- a sentence that moves with the bump names
    the release beside it rather than the one it describes (§ 4a).

    The second half is a NOTE and not a verdict on purpose: a page that
    documents genuinely new behaviour in the diff that ships it looks the same
    from here. Only the strand knows which release the behaviour arrived in,
    which is the question this line asks.
    """
    for path in files:
        if not path.startswith("docs/") or not path.endswith(".md"):
            continue
        text = _read(os.path.join(repo, path))
        if text is None:
            continue
        for line in text.splitlines():
            for named, version in _SINCE.findall(line):
                own = _template_version(repo, named) if named else None
                if own is None:
                    continue           # a bare version names no template
                bumped = "templates/%s/template.json" % named in files
                if _as_tuple(version) > own:
                    why = "templates/%s is at %s" % (named, _dotted(own))
                elif _as_tuple(version) == own and bumped:
                    why = ("this diff bumps templates/%s to %s -- a since-sentence "
                           "names the release the behaviour arrived in" % (named, version))
                else:
                    continue
                yield Finding("NOTE", "since-doc",
                              "%s says `since %s@%s` while %s"
                              % (path, named, version, why))


def _dotted(version):
    return ".".join(str(n) for n in version)


@functools.lru_cache(maxsize=4)
def _template_names(repo):
    """The catalogue's directory names -- who a bare version could belong to."""
    root = os.path.join(repo, "templates")
    if not os.path.isdir(root):
        return frozenset()
    return frozenset(n for n in os.listdir(root)
                     if os.path.isdir(os.path.join(root, n)))


def _template_version(repo, name):
    text = _read(os.path.join(repo, "templates", name, "template.json"))
    if text is None:
        return None
    m = re.search(r'"version"\s*:\s*"([^"]+)"', text)
    return _as_tuple(m.group(1)) if m else None


def _as_tuple(version):
    m = _VERSION.match(version)
    return tuple(int(n) for n in m.groups()) if m else (0, 0, 0)


def check_run_artefact(files, repo):
    """Run artefacts and symlinks belong to a run, not to a commit."""
    ignored = _ignored_paths()
    for path in files:
        full = os.path.join(repo, path)
        if path in ignored:
            yield Finding("NOTE", "run-artefact",
                          "%s is a run artefact -- `git checkout --` it before the "
                          "commit, or it travels with a verdict of its own" % path)
        elif os.path.islink(full):
            yield Finding("NOTE", "run-artefact",
                          "%s is a symlink -- the runner makes one per worktree and "
                          "it does not belong in the diff" % path)


@functools.lru_cache(maxsize=1)
def _ignored_paths():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    try:
        import gate_plan
    except ImportError:          # pragma: no cover -- it travels with us
        return frozenset()
    return frozenset(gate_plan.IGNORED)


def check_plan_cap(files, repo):
    """A plan part past its cap: read it, do not stop for it."""
    for path in files:
        if not _PLAN_PART.match(path):
            continue
        text = _read(os.path.join(repo, path))
        if text is None:
            continue
        lines = len(text.splitlines())
        if lines > PLAN_PART_CAP:
            yield Finding("NOTE", "plan-cap",
                          "%s is a plan part over the cap (R-P1): %d lines, "
                          "cap %d -- contracts, the task list and the tests, "
                          "code only where the form is otherwise unclear"
                          % (path, lines, PLAN_PART_CAP))


def check_name_pattern(files, repo):
    """R5 on the files that travel, with the export's own two exemptions.

    The audit is the verdict; this is the half that costs a second. Both
    exemptions are the audit's and are borrowed rather than restated: a line
    whose only hit is the project's own repository URL, and a line the export
    explains in `ALLOWED_HITS`. Without them the check reports 242 lines on a
    clean tree, which is another way of saying it reports nothing.
    """
    export = _export(repo)
    patterns = getattr(export, "NAME_PATTERNS", None) if export else None
    if not patterns:
        return
    compiled = [(re.compile(rx, re.IGNORECASE), why) for rx, why in patterns]
    docs_map = getattr(export, "DOCS_MAP", {})
    allowed = getattr(export, "_is_allowed", lambda path, line: None)
    for path in files:
        if not _travels(export, path):
            continue
        text = _read(os.path.join(repo, path))
        if text is None:
            continue
        # The export greps the EXPORTED tree, where a doc carries its public
        # name -- and its allowance is keyed on that name.
        as_published = docs_map.get(path, path)
        hit = _first_name_hit(text, compiled, allowed, as_published)
        if hit is not None:
            found, why = hit
            yield Finding("NOTE", "name-pattern",
                          "%s carries `%s` (%s) and travels -- the export's R5 "
                          "reads it too" % (path, found, why))


def _first_name_hit(text, compiled, allowed, as_published):
    """The first pattern this file trips on, or None.

    One finding per FILE, and every file is graded: the check used to `return`
    after the first hit of the whole run, so a diff with three burdened files
    named one of them and the other two waited for the next run.
    """
    for line in text.splitlines():
        bare = _OWN_REPO_URL.sub("", line)
        for rx, why in compiled:
            m = rx.search(bare)
            if m and not allowed(as_published, line):
                return m.group(0), why
    return None


# A link to this project's own repository is not a leak: the owner handle
# matches the name pattern, and every issue link in a README carries it. The
# export strips such a link before it grades a line, and so does this. The
# owner is a wildcard on purpose -- naming it here would put the handle into a
# file that travels, which is the very thing R5 reads for.
_OWN_REPO_URL = re.compile(r"github\.com/[^\s)\"'/]+/meclaw[^\s)\"']*")


def _travels(export, path):
    """Whether the export would carry `path` at all.

    `plans/` is the obvious half and the cheap fallback; the rest is the
    export's own answer -- its root list, its swept tree directories, the
    documentation map and the template subset.
    """
    if NEVER_TRAVELS.match(path):
        return False
    if export is None:
        return True
    if getattr(export, "is_forbidden", lambda _p: False)(path):
        return False
    if path in getattr(export, "ROOT_FILES", ()):
        return True
    if path in getattr(export, "DOCS_MAP", {}):
        return True
    dirs = tuple(d + "/" for d in getattr(export, "TREE_DIRS", ()))
    return bool(dirs and path.startswith(dirs)) or path.startswith("templates/")


# --- the export's own lists -------------------------------------------------

@functools.lru_cache(maxsize=4)
def _export(repo):
    """The export module, or None where it does not exist.

    The export owns the lists this station borrows -- what travels, which
    roots are carried, which names are patterns and which hits are explained.
    They are IMPORTED and never restated: a second copy is a second place one
    of them can be wrong, and the copy is the one that goes stale. `plans/`
    does not travel, so in the published tree there is nothing to import and
    the checks that need it stay quiet rather than invent a verdict.
    """
    path = os.path.join(repo, MAKE_EXPORT)
    if not os.path.isfile(path):
        return None
    import importlib.util
    spec = importlib.util.spec_from_file_location("meclaw_make_export", path)
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception:            # pragma: no cover -- a broken export is audit work
        return None
    return module


# --- reading the tree -------------------------------------------------------

def _read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except (OSError, UnicodeDecodeError):
        return None


@functools.lru_cache(maxsize=4)
def _test_sources(repo):
    """Every test source of the tree, as (label, text).

    The label is what a finding names: `<crate>::<stem>` for a Rust test,
    the path for anything under `scripts/tests/`.
    """
    out = []
    crates = os.path.join(repo, "crates")
    if os.path.isdir(crates):
        for crate in sorted(os.listdir(crates)):
            tests = os.path.join(crates, crate, "tests")
            if not os.path.isdir(tests):
                continue
            for dirpath, _dirs, names in os.walk(tests):
                for fname in sorted(names):
                    if not fname.endswith(".rs"):
                        continue
                    text = _read(os.path.join(dirpath, fname))
                    if text is not None:
                        out.append(("%s::%s" % (crate, fname[:-3]), text))
    own = os.path.join(repo, "scripts", "tests")
    if os.path.isdir(own):
        for fname in sorted(os.listdir(own)):
            if fname.startswith("test_") and fname.endswith(".py"):
                text = _read(os.path.join(own, fname))
                if text is not None:
                    out.append(("scripts/tests/" + fname, text))
    return tuple(out)


_GUARD = re.compile(r"\.(?:exists|is_file|is_dir|try_exists)\(\)")
_BIND = re.compile(r"\b(?:let|const|static)\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\b")


def _tests_reading(repo, path):
    """The tests that read `path` without a guard on it -- see `check_root_files`."""
    literal = '"%s"' % path
    out = set()
    for label, text in _test_sources(repo):
        lines = [("" if line.lstrip().startswith("//") else line)
                 for line in text.splitlines()]
        if not any(literal in line for line in lines):
            continue
        bound = {m.group(1) for m in
                 (_BIND.search(line) for line in lines if literal in line) if m}
        guarded = any(
            _GUARD.search(line)
            and (literal in line
                 or any(re.search(r"\b%s\b" % re.escape(n), line) for n in bound))
            for line in lines)
        if not guarded:
            out.add(label)
    return out


CHECKS = (check_fmt, check_sheet_cap, check_adr_pinned_by, check_root_files,
          check_docs_twin, check_changelog_mark, check_since_sentence,
          check_since_doc, check_run_artefact, check_plan_cap,
          check_name_pattern)


def run(paths, repo=None):
    """Every finding this diff carries, RED first within each check."""
    repo = os.path.abspath(repo or REPO_ROOT)
    files = sorted({p.strip().replace(os.sep, "/") for p in paths if p.strip()})
    out = []
    for check in CHECKS:
        out.extend(check(files, repo))
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--files-from", metavar="FILE",
                     help="read paths from FILE, one per line ('-' = stdin)")
    src.add_argument("--files", nargs="*", metavar="PATH", help="paths on the argv")
    ap.add_argument("--repo", default=REPO_ROOT, help="the tree to read (default: this one)")
    args = ap.parse_args(argv)

    if args.files_from is not None:
        stream = sys.stdin if args.files_from == "-" else open(args.files_from, encoding="utf-8")
        with stream if args.files_from != "-" else _null_ctx(stream) as fh:
            paths = fh.read().splitlines()
    else:
        paths = args.files or []

    findings = run(paths, repo=args.repo)
    for f in findings:
        print("%s %s: %s" % (f.level, f.check, f.message))
    return 1 if any(f.level == "RED" for f in findings) else 0


class _null_ctx:
    """`with` over stdin without closing it."""

    def __init__(self, stream):
        self.stream = stream

    def __enter__(self):
        return self.stream

    def __exit__(self, *exc):
        return False


if __name__ == "__main__":
    sys.exit(main())
