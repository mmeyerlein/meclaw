"""meclaw -- the cheap locks of a diff, one library for `pre` and `precheck`.

WHY THIS EXISTS
===============
Two of the five red passes of 2026-10-08 were a forgotten pin and a test line
that did not compile. The pin was red after twenty-five minutes, in eight of
fourteen builder scenarios, although a grep over the tree finds it in a
second (GH #1108). A lock that costs a second belongs in front of the queue,
not behind it.

Every lock here is a pure function

    check_<name>(diff_paths, tree, ctx) -> [Finding]

and it exists ONCE: `scripts/pipeline/pre.py` (the builder's local pre-run)
and the gate station `precheck` (`scripts/precheck.py`) both call these
functions, so one finding prints the same line in both places
(`render`). A Finding carries a path and a line wherever there is one, and
the fix in one sentence.

THE LOCKS
=========
    pin-lock           every JSON string `"<template>@<x.y.z>"` under
                       `workshop/`, `examples/`, `templates/`, `docs/` and in
                       `sets/*.json` against `templates/*/template.json`: a
                       pin no template provides is RED with path:line. A
                       `<template>@<x.y.z>` in a changed Markdown file that no
                       template provides is a NOTE -- the shorthand belongs in
                       a `since` sentence (development-rules § 4a), and prose
                       names old releases on purpose. An escaped
                       `\"<template>@<x.y.z>\"` (a script inlined as a JSON
                       string, `script_inline`) is the same pin, at any depth (GH #1169).
                       The net: when the diff numbers a template anew against
                       the base (the merge point's candidate), every pin
                       POSITION `"<template>@<old>"` (the same form, escaped
                       to any depth) left in a file class the cascade owns
                       -- `.json` under the four surfaces, `.rs` under
                       `crates/` and `tests/`, `.md` under `templates/`,
                       `docs/`, `examples/` (`NET_CLASSES`) -- is RED. A pin
                       inside a sentence (prose, a code span, `'…'`) is no
                       position; the seed corpus (`docs.jsonl`), fixtures
                       outside the cascade (`crates/**/*.json`) and
                       `docs/superpowers/plans/` are no class of the cascade.
                       Re-checks B-1/B-2: 27 of 54 bumps RED with a net over
                       every text and spelling, 0 with this one.
    version-lock       a strand diff that changes the `version` of a
                       `templates/*/template.json` or the
                       `workspace.package.version` of `Cargo.toml` is RED:
                       versions are assigned at the merge point (OR-DT-13).
                       `allow_bump` (the coordinator's merge, with a reason)
                       turns it into a NOTE that names the reason.
    test-line-lock     a changed promise (`templates/*/README.md`,
                       `docs/stability.en.md`, `CHANGELOG.md`) without a
                       changed test in the same diff: NOTE, never RED --
                       the wave measures how often the note foretold a later
                       red pass before anybody decides to make it a lock.
    scenario-pin-lock  the builder scenarios' scaffolds and cases
                       (`workshop/evals/builder-scenarios/`) against
                       `template.json` -- the 08.10. trap, named apart so the
                       line says which suite would have gone red.
    set-drift-lock     `sets/<v>.json` `local_pins` against the template
                       versions of the pinned core. meclaw-core has no sets;
                       the satellites run it (W10).
    harness-fold-lock  `scripts/harness_fold.py --check` for every crate with a
                       `tests/HARNESS.toml`, when the diff touches a test file
                       or an input of the fold plan: a new flat file the table
                       folds, a module `main.rs` does not declare, a stale
                       fold is RED with the mismatch (GH #1119). No cargo.
    dead-path-lock     `scripts/harness_fold.py`'s `dead_path_mentions` on
                       EVERY diff: a test path a doc under `templates/` or
                       `docs/` names in a code span (any other text file
                       there: anywhere, e.g. config.json, docs.jsonl, canvy
                       .js/.py) that the tree does not hold is RED (W5
                       re-check R3-C1, OR-DT.W5.tpl). A doc goes dead when
                       the test moves, in a diff that never touches the doc,
                       so no diff gates it; it reads the docs only (~0.39 s).
    pin-drift-lock     `sets/<v>.json` `sources.<n>` = {repo, tag|commit,
                       templates} against the source repository (W10): the
                       `source_commits.<n>` a set records must be the commit
                       its tag or commit resolves to, and every `local_pins`
                       entry of a listed template must be the `version` of
                       `<dir>/<t>/template.json` at that revision (`dir`
                       of the source, default `templates`). A
                       drift is RED and names the form (`tag` or `commit`);
                       the lock reports both forms and changes neither. A
                       pin it cannot check (source repository not here,
                       unknown revision, git error, no template.json, a
                       source that is not an object) is RED, everywhere and
                       always, with the fetch hint (OR-DT.ORCH.19). `~` in
                       `repo` is expanded.
    feature-unity-lock every third-party dependency that two or more
                       workspace members declare (any of `dependencies`,
                       `dev-dependencies`, `build-dependencies`, also under
                       `target.*`) comes from `[workspace.dependencies]` with
                       `workspace = true` and no member `features` or
                       `default-features` of its own; a name the workspace
                       declares is inherited by every member that uses it.
                       The key is the crate, not the name: a `package =`
                       rename is the same dependency, and the workspace
                       names each crate once. A `dep/feature` (or
                       `dep?/feature`) in `[features]` that reaches a shared
                       dependency is RED like a member list, and a root
                       `[package]` is read like a member.
                       WHY: a `-p` set of the build station that resolves
                       unlike the workspace builds its own variant of every
                       shared crate; measured: building the station's two
                       sets on a workspace-warm target took 141.4 s with
                       member-local lists, 45.6 s with one set per dependency
                       (GH #1166/#1168). Runs when a manifest
                       moves; an unreadable or missing manifest, a glob in
                       `members` or a root without members is RED (it reads
                       TOML only, never cargo). Path dependencies are exempt.
                       A crate one member declares with features of its
                       own is RED when Cargo.lock puts it under a shared
                       dependency (tb-s10, `KNOWN_TRANSITIVE` lists the
                       measured exceptions and only shrinks). One exception
                       (Ruling OR-TV-1/OR-TV-4): tokio `test-util` alone, in
                       [dev-dependencies] or in meclaw-testing's
                       [dependencies] (`DEV_ONLY_FEATURES`), so no release
                       build resolves the paused test clock.
    bin-owner-lock     exactly one workspace package declares the `meclaw`
                       binary (`[[bin]]` or what cargo discovers: `src/main.rs`,
                       `src/bin/<n>.rs`, `src/bin/<n>/main.rs`), `meclaw-cli`
                       builds none, and the release recipe's `cargo build -p`
                       and `scripts/install.sh`'s `cargo install` name that
                       crate (GH #1168). Runs when the diff touches a
                       `Cargo.toml`, a `src/main.rs` or `src/bin/` at any
                       depth, `.github/workflows/release.yml` or
                       `scripts/install.sh`; TOML and text only, milliseconds.
                       Fails closed: a member glob, an unreadable manifest, a
                       missing recipe or a line with no literal crate (`-p
                       "$PKG"`, `--workspace`, `${CRATE}`) is RED.
    browser-station-lock  on EVERY diff (GH #1161, re-check M-3): a test unit
                       that starts a browser runs in `browser:display`, never
                       in `tests` (`gate_plan.browser_unit`). RED for one the
                       station cannot take (`gate_plan.browser_unplaced`): an
                       `#[ignore]`d browser proof outside BROWSER_LOCKS (its
                       mode is nobody's decision yet), or a shared test module
                       that names a driver (its callers hide from the rule).
                       A tree without `crates/` says nothing.
    diff-size          changed lines without pure moves and generated files
                       (`docs.jsonl`, `last_run.json`): NOTE `large diff <n>
                       lines` above 400. Never RED.

A lock whose input is missing says nothing: no `templates/`, no base
revision, no `sets/` -- no finding.
"""

import collections
import fnmatch
import functools
import json
import os
import re
import subprocess

Finding = collections.namedtuple("Finding", "verdict check path line text fix")

SURFACES = ("workshop", "examples", "templates", "docs")
SCENARIO_ROOT = "workshop/evals/builder-scenarios"
SKIP_DIRS = frozenset({".git", "node_modules", "target", "__pycache__"})
PROMISE_GLOBS = ("templates/*/README.md", "docs/stability.en.md", "CHANGELOG.md")
TEST_GLOBS = ("scripts/tests/*", "crates/*/tests/*", "workshop/*/tests/*", "*/test_*.py",
              "templates/*/*/scenarios/run_*.py")
GENERATED = ("docs.jsonl", "last_run.json")
DIFF_SIZE_NOTE = 400

_VERSION = r"\d+\.\d+\.\d+"


def render(f):
    """The one line a finding prints, in `pre` and in `precheck` alike."""
    where = "%s:%d " % (f.path, f.line) if f.path and f.line else (
        "%s " % f.path if f.path else "")
    text = "%s%s" % (where, f.text)
    return "%s -- %s" % (text, f.fix) if f.fix else text


# --- the tree ---------------------------------------------------------------

class Unlisted(Exception):
    """git cannot list a tree that has `.git` (GH #1199, re-check M5)."""


def _closed(fn):
    """A lock that reads the tree's listing: `Unlisted` is RED with its reason, never a
    quiet walk (GH #1199, re-check M5) -- the walk would read an ignored rest again."""
    @functools.wraps(fn)
    def check(diff_paths, tree, ctx=None):
        try:
            return list(fn(diff_paths, tree, ctx))
        except Unlisted as exc:
            return [Finding("RED", fn.__name__[len("check_"):].replace("_", "-"), "", 0,
                            str(exc), "`git -C %s ls-files` says why (safe.directory, an "
                            "index lock, the 30-s timeout); repair it and run again" % tree)]
    return check


def template_versions(tree):
    """`{name: version}` from `templates/*/template.json`; `{}` without them.
    Only the tree's own files (`listed`, GH #1199 K19)."""
    out = {}
    for name in template_dirs(tree):
        path = os.path.join(tree, "templates", name, "template.json")
        try:
            with open(path, encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            continue
        if isinstance(doc, dict) and doc.get("version"):
            out[doc.get("name") or name] = str(doc["version"])
    return out


def pin_position_re(names):
    """A pin POSITION: a string (or key) that is exactly `"<name>@<x.y.z>"` --
    escaped to any depth, as long as both quotes carry the same escape:
    `\\"name@x.y.z\\"` is the same pin one JSON level deeper, a script inlined as
    a JSON string (GH #1169). Groups `name` and `ver`. The one form the cascade
    raises, the stale-pin scan reads and the net checks (tb-s8 re-check m-7)."""
    alt = "|".join(sorted((re.escape(n) for n in names), key=len, reverse=True))
    return re.compile(r'(?P<esc>\\*)"(?P<name>%s)@(?P<ver>%s)(?P=esc)"' % (alt, _VERSION))


_pin_re = pin_position_re


def _prose_re(names):
    alt = "|".join(sorted((re.escape(n) for n in names), key=len, reverse=True))
    return re.compile(r"(?<![A-Za-z0-9_./-])(%s)@(%s)\b" % (alt, _VERSION))


def _walk_json(tree, top):
    return _walk(tree, top, ".json")


def listed(tree, top):
    """The files under `top` a pin reader reads (GH #1199, K19 17.10.): what git lists
    there (`_tree_paths`: tracked, and untracked not ignored -- the tree a gate sees,
    overlay included), sorted. An ignored rest the checkout keeps (`git clean` without
    -x) is no file of this tree: four `workshop/workspace/builder-scenarios/*` of an
    older run on build04 pinned `member@2.5.33` and turned pin-lock red after a bump.
    A git tree git cannot list raises `Unlisted` (a lock says RED); a tree without git
    is walked."""
    return sorted(rel for rel in _tree_paths(tree, top)
                  if not SKIP_DIRS.intersection(rel.split("/")[:-1])
                  and os.path.isfile(os.path.join(tree, rel)))


def template_dirs(tree):
    """The directories `<d>` of `templates/<d>/template.json` among `listed`."""
    return sorted({rel.split("/")[1] for rel in listed(tree, "templates")
                   if rel.count("/") == 2 and rel.endswith("/template.json")})


def _walk(tree, top, suffix):
    return (rel for rel in listed(tree, top) if rel.endswith(suffix))


def _lines(tree, rel):
    try:
        with open(os.path.join(tree, rel), encoding="utf-8") as fh:
            return fh.read().splitlines()
    except (OSError, UnicodeDecodeError):
        return []


def _stale_pins(tree, rels, versions, check):
    if not versions:
        return []
    rx = _pin_re(versions)
    out = []
    for rel in rels:
        for no, line in enumerate(_lines(tree, rel), 1):
            if "@" not in line:
                continue
            for m in rx.finditer(line):
                name, ver = m.group("name"), m.group("ver")
                if versions[name] != ver:
                    out.append(Finding(
                        "RED", check, rel, no,
                        "%s@%s not provided" % (name, ver),
                        "raise to %s" % versions[name]))
    return out


# --- the net (GH #1169) ------------------------------------------------------

FROZEN = ("docs/superpowers/plans/",)
# The cascade's file classes, `(suffix, tops)` -- `versions.SURFACES`,
# `CODE_SURFACES` and `PROSE_SURFACES` (a test holds them equal): the net reads
# a pin position wherever the cascade would have raised one (re-check B-2).
NET_CLASSES = ((".json", ("templates", "docs", "examples", "workshop")),
               (".rs", ("crates", "tests")),
               (".md", ("templates", "docs", "examples")))


def moved_templates(diff_paths, tree, base):
    """`{name: (old, new)}` for every `templates/*/template.json` the diff numbers
    anew against `base`; `{}` without a base."""
    if not base:
        return {}
    now = template_versions(tree)
    out = {}
    for rel in diff_paths:
        m = re.match(r"templates/([^/]+)/template\.json$", rel)
        if not m:
            continue
        try:
            doc = json.loads(_git(tree, "show", "%s:%s" % (base, rel)) or "")
        except ValueError:
            continue
        if not isinstance(doc, dict) or not doc.get("version"):
            continue
        name, old = doc.get("name") or m.group(1), str(doc["version"])
        if now.get(name) and now[name] != old:
            out[name] = (old, now[name])
    return out


def _net_files(tree):
    """The files the cascade owns (`NET_CLASSES`), without the frozen archive."""
    for suffix, tops in NET_CLASSES:
        for top in tops:
            for rel in _walk(tree, top, suffix):
                if not rel.startswith(FROZEN):
                    yield rel


def left_behind(diff_paths, tree, base):
    """The net: every pin position `<name>@<old>` of a template the diff bumped."""
    moved = moved_templates(diff_paths, tree, base)
    if not moved:
        return []
    rx = pin_position_re(moved)
    olds = ["%s@%s" % (n, v[0]) for n, v in moved.items()]
    out = []
    for rel in _net_files(tree):
        lines = _lines(tree, rel)
        if not any(o in line for line in lines for o in olds):
            continue
        for no, line in enumerate(lines, 1):
            for m in rx.finditer(line):
                name, ver = m.group("name"), m.group("ver")
                if ver != moved[name][0]:
                    continue
                out.append(Finding(
                    "RED", "pin-lock", rel, no,
                    "%s@%s left behind by the bump to %s" % (name, ver, moved[name][1]),
                    "raise it to %s" % moved[name][1]))
                break
    return out


def _sets(tree):
    return [rel for rel in listed(tree, "sets") if rel.count("/") == 1 and rel.endswith(".json")]


# --- the locks --------------------------------------------------------------

@_closed
def check_pin_lock(diff_paths, tree, ctx=None):
    versions = template_versions(tree)
    if not versions:
        return []
    rels = []
    for top in SURFACES:
        rels.extend(r for r in _walk_json(tree, top)
                    if not r.startswith(SCENARIO_ROOT + "/"))
    rels.extend(_sets(tree))
    out = _stale_pins(tree, rels, versions, "pin-lock")
    prose = _prose_re(versions)
    base = (ctx or {}).get("base")
    for rel in diff_paths:
        if not rel.endswith(".md") or not rel.startswith(SURFACES):
            continue
        added = _added_lines(tree, base, rel)
        for no, line in enumerate(_lines(tree, rel), 1):
            if added is not None and no not in added:
                continue
            for m in prose.finditer(line):
                name, ver = m.group(1), m.group(2)
                if versions[name] != ver:
                    out.append(Finding(
                        "NOTE", "pin-lock", rel, no,
                        "%s@%s is not the tree's %s" % (name, ver, versions[name]),
                        "keep the shorthand to a since sentence (development-rules § 4a)"))
    seen = {(f.path, f.line) for f in out if f.verdict == "RED"}
    out.extend(f for f in left_behind(diff_paths, tree, base) if (f.path, f.line) not in seen)
    return out


def _added_lines(tree, base, rel):
    """Line numbers `rel` adds against `base`; None = every line (no base, new file)."""
    if not base:
        return None
    text = _git(tree, "diff", "-U0", "--no-renames", base, "--", rel)
    if not text:
        tracked = _git(tree, "cat-file", "-e", "%s:%s" % (base, rel))
        return None if tracked is None else set()
    out = set()
    for m in re.finditer(r"^@@ -\S+ \+(\d+)(?:,(\d+))? @@", text, re.MULTILINE):
        start, count = int(m.group(1)), int(m.group(2) or "1")
        out.update(range(start, start + count))
    return out


@_closed
def check_scenario_pin_lock(diff_paths, tree, ctx=None):
    versions = template_versions(tree)
    return _stale_pins(tree, list(_walk_json(tree, SCENARIO_ROOT)), versions,
                       "scenario-pin-lock")


def _git(tree, *args):
    try:
        res = subprocess.run(["git", "-C", tree] + list(args),
                             capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return res.stdout if res.returncode == 0 else None


def _version_of(text, path):
    if text is None:
        return None
    if path == "Cargo.toml":
        m = re.search(r"^\[workspace\.package\][^\[]*?^version\s*=\s*\"([^\"]+)\"",
                      text, re.MULTILINE | re.DOTALL)
        return m.group(1) if m else None
    try:
        doc = json.loads(text)
    except ValueError:
        return None
    return str(doc.get("version")) if isinstance(doc, dict) and doc.get("version") else None


def check_version_lock(diff_paths, tree, ctx=None):
    ctx = ctx or {}
    base = ctx.get("base")
    if not base or ctx.get("mode", "strand") not in ("strand", "pre"):
        return []
    out = []
    for rel in diff_paths:
        if not (rel == "Cargo.toml" or fnmatch.fnmatch(rel, "templates/*/template.json")):
            continue
        new = _version_of("\n".join(_lines(tree, rel)), rel)
        old = _version_of(_git(tree, "show", "%s:%s" % (base, rel)), rel)
        if old is None or new is None or old == new:
            continue
        line = next((n for n, l in enumerate(_lines(tree, rel), 1)
                     if re.match(r'\s*"?version"?\s*[:=]', l)), 1)
        allow = ctx.get("allow_bump")
        if allow:
            out.append(Finding("NOTE", "version-lock", rel, line,
                               "%s -> %s allowed: %s" % (old, new, allow), ""))
        else:
            out.append(Finding("RED", "version-lock", rel, line,
                               "%s -> %s: versions are assigned at the merge point" % (old, new),
                               "revert the bump, the coordinator assigns it (OR-DT-13)"))
    return out


def check_test_line_lock(diff_paths, tree, ctx=None):
    promises = [p for p in diff_paths
                if any(fnmatch.fnmatch(p, g) for g in PROMISE_GLOBS)]
    if not promises:
        return []
    if any(any(fnmatch.fnmatch(p, g) for g in TEST_GLOBS) for p in diff_paths):
        return []
    return [Finding("NOTE", "test-line-lock", p, 0, "no test moved with the promise",
                    "a promise without a pin (development-rules § 2)") for p in promises]


def _local_pins(doc):
    pins = doc.get("local_pins") if isinstance(doc, dict) else None
    if isinstance(pins, dict):
        return sorted((str(k), str(v)) for k, v in pins.items())
    out = []
    for item in pins or []:
        if isinstance(item, str) and "@" in item:
            name, _, ver = item.partition("@")
            out.append((name, ver))
        elif isinstance(item, dict) and item.get("template"):
            out.append((str(item["template"]), str(item.get("version", ""))))
    return out


@_closed
def check_set_drift_lock(diff_paths, tree, ctx=None):
    ctx = ctx or {}
    core = ctx.get("core_versions")
    if core is None:
        core = template_versions(ctx.get("core_tree") or tree)
    out = []
    for rel in _sets(tree):
        lines = _lines(tree, rel)
        try:
            doc = json.loads("\n".join(lines))
        except ValueError:
            out.append(Finding("RED", "set-drift-lock", rel, 1, "not JSON", "fix the set file"))
            continue
        for name, ver in _local_pins(doc):
            have = core.get(name)
            if have is None or have == ver:
                continue
            no = next((n for n, l in enumerate(lines, 1) if name in l and ver in l), 1)
            out.append(Finding("RED", "set-drift-lock", rel, no,
                               "local pin %s@%s, the core ships %s" % (name, ver, have),
                               "raise the pin to %s" % have))
    return out


def _source_repo(tree, repo):
    path = os.path.expanduser(str(repo or ""))
    return path if os.path.isabs(path) else os.path.normpath(os.path.join(tree, path))


def _line_of(lines, *needles):
    return next((n for n, l in enumerate(lines, 1) if all(x in l for x in needles)), 1)


def _pin_drift(rel, lines, doc, tree):
    out = []
    # fail-closed (review I1, OR-DT.ORCH.19): a pin that cannot be checked
    # is RED wherever the lock runs; no context grades it down
    unchecked = "RED"
    pins = dict(_local_pins(doc))
    commits = doc.get("source_commits") if isinstance(doc.get("source_commits"), dict) else {}
    sources = doc.get("sources") if isinstance(doc.get("sources"), dict) else {}
    for n, src in sorted(sources.items()):
        if not isinstance(src, dict):
            out.append(Finding("RED", "pin-drift-lock", rel, _line_of(lines, '"%s"' % n),
                               "source %s is not an object" % n,
                               "write it as {repo, tag|commit, templates}"))
            continue
        form = "tag" if src.get("tag") else "commit" if src.get("commit") else None
        if form is None:
            out.append(Finding("RED", "pin-drift-lock", rel, _line_of(lines, '"%s"' % n),
                               "source %s names neither tag nor commit" % n,
                               "pin the source by tag or commit"))
            continue
        rev = str(src[form])
        repo = _source_repo(tree, src.get("repo"))
        if not src.get("repo") or not os.path.isdir(repo):
            out.append(Finding(unchecked, "pin-drift-lock", rel, _line_of(lines, '"repo"'),
                               "source %s: repository %s is not here, pins unchecked"
                               % (n, src.get("repo")),
                               "clone or fetch the source repository to %s" % src.get("repo")))
            continue
        sha = (_git(repo, "rev-parse", "--verify", "--quiet", rev + "^{commit}") or "").strip()
        if not sha:
            out.append(Finding(unchecked, "pin-drift-lock", rel, _line_of(lines, rev),
                               "source %s: %s %s unknown in %s (or git failed), pins unchecked"
                               % (n, form, rev, src.get("repo")), "fetch the source repository"))
            continue
        have = str(commits.get(n) or "")
        if have and not (len(have) >= 7 and sha.startswith(have)):
            out.append(Finding("RED", "pin-drift-lock", rel, _line_of(lines, have),
                               "source_commits.%s = %s, but %s %s resolves to %s"
                               % (n, have[:12], form, rev, sha[:12]),
                               "record %s or pin the %s that is meant" % (sha, form)))
        top = str(src.get("dir") or "templates").strip("/")
        for t in src.get("templates") or []:
            if t not in pins:
                continue
            path = "%s/%s/template.json" % (top, t)
            ships = _version_of(_git(repo, "show", "%s:%s" % (sha, path)), path)
            if ships is None:
                out.append(Finding(unchecked, "pin-drift-lock", rel, _line_of(lines, '"%s"' % t),
                                   "source %s: %s %s has no %s" % (n, form, rev, path),
                                   "fetch the source repository or fix `dir`/`templates`"))
            elif ships != pins[t]:
                out.append(Finding("RED", "pin-drift-lock", rel,
                                   _line_of(lines, '"%s"' % t, pins[t]),
                                   "local pin %s@%s, but %s %s ships %s"
                                   % (t, pins[t], form, rev, ships),
                                   "set the local pin to %s or move the %s" % (ships, form)))
    return out


@_closed
def check_pin_drift_lock(diff_paths, tree, ctx=None):
    out = []
    for rel in _sets(tree):
        lines = _lines(tree, rel)
        try:
            doc = json.loads("\n".join(lines))
        except ValueError:
            continue  # set-drift-lock reports the broken file
        if isinstance(doc, dict):
            out.extend(_pin_drift(rel, lines, doc, tree))
    return out


# The one `meclaw` binary and the crate the two recipes build (GH #1168,
# tb-s6b). Until #1168 both `crates/meclaw` and `crates/meclaw-cli` declared
# `[[bin]] name = "meclaw"`: which one `CARGO_BIN_EXE_meclaw` started was not
# fixed, every cli change paid a second full binary link (1.3 s alone,
# 2.7-3.0 s under load), and the release built `-p meclaw-cli`. The lock reads
# TOML and text only -- milliseconds -- so it runs here, in `precheck` and
# `pre`; as a class of `gate-selftest` it made every manifest diff, the
# merge's version bump included, pay that station's ~326 s.
BIN_NAME = "meclaw"
BIN_CLI = "meclaw-cli"
BIN_RELEASE_RECIPE = ".github/workflows/release.yml"
BIN_SOURCE_INSTALL = "scripts/install.sh"
# Fail closed: a manifest, a `src/main.rs` or anything under `src/bin/` at ANY
# depth, not only under `crates/` -- a member elsewhere is a binary as well.
_BIN_OWNER_INPUT = re.compile(r"(?:^|.*/)(?:Cargo\.toml|src/main\.rs|src/bin/.+)\Z")
_LITERAL_CRATE = re.compile(r"[A-Za-z0-9_\-]+\Z")
# `cargo install` options that take a value; the value is not the crate.
_INSTALL_OPTS_WITH_VALUE = frozenset({
    "--git", "--path", "--branch", "--tag", "--rev", "--version", "--root",
    "--registry", "--index", "--bin", "--example", "--features", "-F",
    "--target", "--target-dir", "--profile", "--jobs", "-j", "--config",
})


def _read_toml(path):
    import tomllib
    with open(path, "rb") as fh:
        return tomllib.load(fh)


def binary_targets(tree):
    """`[(package, binary name)]` of every workspace member: each `[[bin]]`
    plus what cargo discovers on its own unless `autobins = false` --
    `src/main.rs` under the package name, `src/bin/<stem>.rs` under the stem,
    `src/bin/<dir>/main.rs` under the directory name. A discovered binary
    whose path or name a `[[bin]]` already declares is that one, not a second.
    A member glob raises: the reader does not expand what cargo would."""
    workspace = _read_toml(os.path.join(tree, "Cargo.toml"))
    out = []
    for member in workspace["workspace"]["members"]:
        if any(ch in member for ch in "*?["):
            raise ValueError("workspace member glob %r: list members explicitly" % member)
        crate_dir = os.path.join(tree, member)
        manifest = _read_toml(os.path.join(crate_dir, "Cargo.toml"))
        package = manifest["package"]["name"]
        declared_names, declared_paths = set(), set()
        for target in manifest.get("bin", []):
            out.append((package, target["name"]))
            declared_names.add(target["name"])
            if "path" in target:
                declared_paths.add(os.path.normpath(target["path"]).replace(os.sep, "/"))
        if not manifest["package"].get("autobins", True):
            continue
        discovered = []
        if os.path.isfile(os.path.join(crate_dir, "src", "main.rs")):
            discovered.append(("src/main.rs", package))
        bin_dir = os.path.join(crate_dir, "src", "bin")
        entries = sorted(os.listdir(bin_dir)) if os.path.isdir(bin_dir) else []
        for name in entries:
            if name.endswith(".rs") and os.path.isfile(os.path.join(bin_dir, name)):
                discovered.append(("src/bin/" + name, name[:-3]))
        for name in entries:
            if os.path.isfile(os.path.join(bin_dir, name, "main.rs")):
                discovered.append(("src/bin/%s/main.rs" % name, name))
        for path, name in discovered:
            if path not in declared_paths and name not in declared_names:
                out.append((package, name))
    return out


def bin_owners(tree, name=BIN_NAME):
    """The packages that declare a binary called `name`."""
    return sorted(pkg for pkg, binary in binary_targets(tree) if binary == name)


def cli_binaries(tree):
    """The binaries `meclaw-cli` builds -- none since #1168: a silent one would
    be the next second link of the cli chain (OR-TB.S6b.3)."""
    return [binary for pkg, binary in binary_targets(tree) if pkg == BIN_CLI]


def _logical_lines(text):
    """`text` split into lines, a backslash continuation joined to its line."""
    return re.sub(r"\\\n\s*", " ", text).splitlines()


def release_build_crates(text):
    """The crate of every `cargo build` line of the release recipe. Each line
    must name exactly one literal `-p`/`--package` crate; a variable, a
    `--workspace` or no package at all raises, it is not skipped."""
    import shlex
    found = []
    for line in _logical_lines(text):
        if not re.search(r"\bcargo build\b", line):
            continue
        tokens = shlex.split(line.split("cargo build", 1)[1])
        crates = []
        for i, token in enumerate(tokens):
            if token in ("-p", "--package"):
                crates.append(tokens[i + 1] if i + 1 < len(tokens) else "")
            elif token.startswith("--package="):
                crates.append(token.split("=", 1)[1])
        if len(crates) != 1 or not _LITERAL_CRATE.match(crates[0]):
            raise ValueError("release recipe: `cargo build` line names %r, want one literal "
                             "-p <crate>: %s" % (crates, line.strip()))
        found.append(crates[0])
    if not found:
        raise ValueError("release recipe: no `cargo build -p <crate>` line")
    return found


def source_install_crates(text):
    """The crate every `cargo install …` line of the install script names."""
    import shlex
    found = []
    for line in text.splitlines():
        if "cargo install" not in line:
            continue
        rest = line.split("cargo install", 1)[1].strip().strip("\"'")
        positional, skip = [], False
        for token in shlex.split(rest):
            if skip:
                skip = False
                continue
            if token.startswith("-"):
                skip = token in _INSTALL_OPTS_WITH_VALUE
                continue
            positional.append(token)
        if len(positional) != 1 or not _LITERAL_CRATE.match(positional[0]):
            raise ValueError("install script: `cargo install` line names %r, want one literal "
                             "crate: %s" % (positional, line.strip()))
        found.append(positional[0])
    if not found:
        raise ValueError("install script: no `cargo install … <crate>` line")
    return found


def check_bin_owner_lock(diff_paths, tree, ctx=None):
    if not any(p in (BIN_RELEASE_RECIPE, BIN_SOURCE_INSTALL) or _BIN_OWNER_INPUT.match(p)
               for p in diff_paths):
        return []
    if not os.path.exists(os.path.join(tree, "Cargo.toml")):
        return []  # a satellite without a cargo workspace

    def red(path, text, fix):
        return Finding("RED", "bin-owner-lock", path, 0, text, fix)

    try:
        owners = bin_owners(tree)
        cli = cli_binaries(tree)
    except (OSError, ValueError, KeyError, TypeError) as exc:
        return [red("Cargo.toml", "cannot read the binary targets (%s: %s)"
                    % (type(exc).__name__, exc),
                    "fix the manifests; the lock reads TOML only and fails closed")]
    out = []
    if len(owners) != 1:
        out.append(red("Cargo.toml", "%d packages declare a `%s` binary: %s"
                       % (len(owners), BIN_NAME, ", ".join(owners) or "none"),
                       "exactly one crate owns `%s` (GH #1168)" % BIN_NAME))
    if cli:
        out.append(red("crates/%s/Cargo.toml" % BIN_CLI, "`%s` builds binaries %s"
                       % (BIN_CLI, ", ".join(cli)),
                       "the cli is a library; set `autobins = false` or move the binary"))
    for rel, reader in ((BIN_RELEASE_RECIPE, release_build_crates),
                        (BIN_SOURCE_INSTALL, source_install_crates)):
        try:
            with open(os.path.join(tree, rel), encoding="utf-8") as fh:
                crates = sorted(set(reader(fh.read())))
        except (OSError, ValueError) as exc:
            out.append(red(rel, "cannot name the crate it builds (%s)" % exc,
                           "one literal crate per `cargo build`/`cargo install` line"))
            continue
        if len(owners) == 1 and crates != owners:
            out.append(red(rel, "builds %s, the `%s` binary is owned by %s"
                           % (", ".join(crates), BIN_NAME, owners[0]),
                           "name `%s` there" % owners[0]))
    return out


def diff_lines(tree, base):
    """Changed lines against `base`, without pure moves and generated files."""
    if not base:
        return None
    text = _git(tree, "diff", "-M", "--numstat", base)
    if text is None:
        return None
    total = 0
    for row in text.splitlines():
        parts = row.split("\t")
        if len(parts) < 3 or parts[0] == "-":
            continue
        path = parts[-1]
        if os.path.basename(path.replace("}", "")) in GENERATED:
            continue
        total += int(parts[0]) + int(parts[1])
    for rel in (_git(tree, "ls-files", "-o", "--exclude-standard") or "").splitlines():
        if os.path.basename(rel) not in GENERATED:
            total += len(_lines(tree, rel))
    return total


def check_diff_size(diff_paths, tree, ctx=None):
    n = diff_lines(tree, (ctx or {}).get("base"))
    if n is None or n <= DIFF_SIZE_NOTE:
        return []
    return [Finding("NOTE", "diff-size", "", 0, "large diff %d lines" % n,
                    "a smaller diff gates and reviews faster")]


# What the fold plan reads besides the test files themselves (no rustfmt, no
# rustfmt.toml: the fold writes rustfmt's text itself, #1134).
FOLD_INPUTS = ("scripts/harness_fold.py", "scripts/gate_plan.py", ".config/nextest.toml",
               "plans/export-fixtures/make_export.py", "templates/*/README.md",
               "crates/*/tests/*", "crates/*/Cargo.toml")


def check_harness_fold_lock(diff_paths, tree, ctx=None):
    if not any(fnmatch.fnmatch(p, g) for p in diff_paths for g in FOLD_INPUTS):
        return []
    crates = os.path.join(tree, "crates")
    folded = sorted(c for c in (os.listdir(crates) if os.path.isdir(crates) else ())
                    if os.path.isfile(os.path.join(crates, c, "tests", "HARNESS.toml")))
    if not folded:
        return []
    import sys
    scripts = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    import harness_fold
    out = []
    for c in folded:
        rel = "crates/%s/tests/HARNESS.toml" % c
        try:
            krate = harness_fold.Crate(tree, c)
            bad = harness_fold.check(krate, krate.plan())
        except SystemExit as e:  # the plan itself refuses (a harness name collides)
            bad = [str(e)]
        out += [Finding("RED", "harness-fold-lock", rel, 0, b,
                        "scripts/harness_fold.py --crate %s --apply" % c) for b in bad]
    return out


def check_dead_path_lock(diff_paths, tree, ctx=None):
    if not os.path.isdir(os.path.join(tree, "templates")) and \
            not os.path.isdir(os.path.join(tree, "docs")):
        if not os.path.isdir(os.path.join(tree, "crates")):
            return []  # a satellite: no test paths to name
        # W5 re-check 4 M2: a core tree with nothing to scan is no green
        return [Finding("RED", "dead-path-lock", tree, 0,
                        "neither templates/ nor docs/ in a tree with crates/",
                        "run the lock in a complete tree")]
    import sys
    scripts = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    import harness_fold
    unread = []
    try:
        dead = harness_fold.dead_path_mentions(tree, unread=unread)
    except OSError as exc:
        where = getattr(exc, "filename", None) or tree
        if os.path.islink(where) and not os.path.exists(where):
            return [Finding("RED", "dead-path-lock", where, 0,
                            "is a dangling symlink (target %s missing)" % os.readlink(where),
                            "point the link at a file or remove it")]
        return [Finding("RED", "dead-path-lock", where, 0,
                        "cannot be read (%s)" % exc.strerror, "make the directory readable")]
    # R3 M1: a frozen entry the tree no longer needs would excuse a NEW dead
    # mention later; a docs-only diff must not carry it past pre/precheck. Only
    # entries whose doc is in this tree (a fixture holds none; a removed doc is
    # the selftest's).
    stale = [(doc, path, n, m) for doc, path, n, m in harness_fold.stale_frozen(tree)
             if os.path.isfile(os.path.join(tree, doc))]
    out = [Finding("RED", "dead-path-lock", doc, 0, "names %s, which the tree does not hold" % path,
                   "name the test where it lives now (tests/<harness>/<stem>.rs)")
           for doc, path in dead] + [
        Finding("RED", "dead-path-lock", doc, 0, "FROZEN_HISTORICAL excuses %d mention(s) of %s,"
                " the tree has %d" % (n, path, m), "shrink the entry in scripts/harness_fold.py")
        for doc, path, n, m in stale]
    # Review M of Z-D: a file that is not UTF-8 is counted, never skipped silently.
    if unread:
        out.append(Finding("NOTE", "dead-path-lock", unread[0], 0,
                           "%d file%s not UTF-8, not read" % (len(unread), "" if len(unread) == 1 else "s"),
                           "a test path named there is not checked"))
    return out



# The manifests a feature-unity drift can enter through (tb-s6c, GH #1168).
FEATURE_INPUTS = ("Cargo.toml", "crates/*/Cargo.toml")
_DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
# The one member-local feature the lock allows (Ruling OR-TV-1/OR-TV-4, VG
# TV-W1): tokio `test-util` drives the paused test clock and must never reach
# a release build -- in [workspace.dependencies] the shipped binary would read
# every `Instant` through tokio's test clock (`with_clock` + `Mutex<Inner>`,
# tokio 1.53.1 `time/clock.rs`). So it is allowed only in [dev-dependencies],
# and in the plain [dependencies] of meclaw-testing, which is per spec never a
# runtime dependency (AGENTS.md rule 6). Nothing else, no other crate.
DEV_ONLY_FEATURES = {"tokio": frozenset({"test-util"})}
DEV_ONLY_MEMBERS = frozenset({"meclaw-testing"})


def _dev_only(crate, table, spec, package):
    """True when a member feature list is exactly the dev-only exception."""
    allowed = DEV_ONLY_FEATURES.get(crate)
    feats = spec.get("features")
    if allowed is None or not isinstance(feats, list) or not feats:
        return False
    if any(k in spec for k in ("default-features", "default_features")):
        return False
    if not set(feats) <= allowed:
        return False
    return table == "dev-dependencies" or (table == "dependencies" and package in DEV_ONLY_MEMBERS)


def _toml(path):
    try:
        import tomllib
    except ImportError:          # pragma: no cover -- python < 3.11
        import tomli as tomllib  # type: ignore
    with open(path, "rb") as fh:
        return tomllib.load(fh)


def _declarations(doc):
    """(table, name, spec) for every dependency a member manifest declares."""
    tables = [(t, doc.get(t)) for t in _DEP_TABLES]
    for target, tdoc in sorted((doc.get("target") or {}).items()):
        if isinstance(tdoc, dict):
            tables += [("target.%s.%s" % (target, t), tdoc.get(t)) for t in _DEP_TABLES]
    for table, deps in tables:
        for name, spec in sorted((deps or {}).items()):
            yield table, name, (spec if isinstance(spec, dict) else {"version": spec})


def _crate(name, spec, inherited):
    """The crate a declaration names. Re-check h1 L-1: keyed on the dependency
    NAME, `tk = { package = "tokio", features = [..] }` carried a second
    feature set of a shared crate past the lock (mutations M6/M6b green)."""
    if spec.get("workspace") is True:
        entry = inherited.get(name)
        spec = entry if isinstance(entry, dict) else {}
    crate = spec.get("package", name)
    return crate if isinstance(crate, str) else name


def _feature_deps(doc):
    """(feature, entry, dependency name) for every `dep/feature` or
    `dep?/feature` in a manifest's [features]. Re-check h1 L-2: such an entry
    switches a feature of `dep` on as surely as a member feature list does
    (mutation M7 green before)."""
    feats = doc.get("features")
    for feat, entries in sorted((feats if isinstance(feats, dict) else {}).items()):
        for entry in entries if isinstance(entries, list) else []:
            if isinstance(entry, str) and "/" in entry:  # `dep:x` has none
                yield feat, entry, entry.split("/", 1)[0].rstrip("?")


def check_feature_unity_lock(diff_paths, tree, ctx=None):
    if not any(fnmatch.fnmatch(p, g) for p in diff_paths for g in FEATURE_INPUTS):
        return []
    root = os.path.join(tree, "Cargo.toml")
    if not os.path.exists(root) and not os.path.isdir(os.path.join(tree, "crates")):
        return []  # a satellite without a cargo workspace

    def red(path, text, fix):
        return Finding("RED", "feature-unity-lock", path, 0, text, fix)

    def label(name, crate):
        return name if name == crate else "%s (package %s)" % (name, crate)

    try:
        ws = _toml(root)
    except (OSError, ValueError) as exc:
        return [red("Cargo.toml", "cannot be read as TOML (%s)" % exc,
                    "fix the root manifest; the lock reads TOML only")]
    workspace = ws.get("workspace") if isinstance(ws.get("workspace"), dict) else {}
    members = workspace.get("members")
    if not isinstance(members, list) or not members:
        return [red("Cargo.toml", "declares no [workspace] members",
                    "list every member path explicitly")]
    inherited = workspace.get("dependencies") or {}
    out, users = [], collections.defaultdict(list)
    # One entry per crate in [workspace.dependencies] too: two names for one
    # crate are two feature sets again, one step removed (L-1).
    ws_crates = collections.defaultdict(list)
    for name, entry in sorted(inherited.items()):
        spec = entry if isinstance(entry, dict) else {"version": entry}
        if "path" not in spec:
            ws_crates[_crate(name, spec, inherited)].append(name)
    for crate, names in sorted(ws_crates.items()):
        if len(names) > 1:
            out.append(red("Cargo.toml", "[workspace.dependencies] declares %s under %d names (%s)"
                           % (crate, len(names), ", ".join(names)),
                           "keep one entry for %s and let every member inherit it" % crate))
    # The manifests that resolve features: every member, and the root when it
    # is a package as well (L-2: its tables went unread, mutation M8 green).
    manifests = collections.OrderedDict()
    if isinstance(ws.get("package"), dict):
        manifests["Cargo.toml"] = ws
    for member in members:
        if not isinstance(member, str) or any(c in member for c in "*?["):
            out.append(red("Cargo.toml", "member %r is a pattern the lock does not expand" % (member,),
                           "list every member path explicitly"))
            continue
        rel = os.path.normpath("%s/Cargo.toml" % member.rstrip("/"))
        if rel in manifests:
            continue
        try:
            manifests[rel] = _toml(os.path.join(tree, rel))
        except (OSError, ValueError) as exc:
            out.append(red(rel, "cannot be read as TOML (%s)" % exc,
                           "fix the member manifest; the lock reads TOML only"))
    names = {}  # (manifest, dependency name) -> crate, for [features]
    packages = {}  # manifest -> package name (the dev-only exception)
    for rel, doc in manifests.items():
        for table, name, spec in _declarations(doc):
            if "path" in spec:
                continue  # a workspace member, no third-party feature set
            crate = _crate(name, spec, inherited)
            names[(rel, name)] = crate
            users[crate].append((rel, table, name, spec))
            pkg = doc.get("package")
            packages[rel] = pkg.get("name") if isinstance(pkg, dict) else None

    def shared(crate):
        return len(users[crate]) > 1 or crate in ws_crates

    for crate, decls in sorted(users.items()):
        for rel, table, name, spec in decls:
            if spec.get("workspace") is True:
                if name not in inherited:
                    out.append(red(rel, "[%s] %s inherits from the workspace, which does not "
                                   "declare it" % (table, name),
                                   "declare %s in [workspace.dependencies]" % name))
                own = [k for k in ("features", "default-features", "default_features") if k in spec]
                if own and not _dev_only(crate, table, spec, packages.get(rel)):
                    out.append(red(rel, "[%s] %s adds %s on top of [workspace.dependencies]"
                                   % (table, label(name, crate), "/".join(own)),
                                   "move the features to [workspace.dependencies] %s" % name))
            elif shared(crate):
                where = sorted({r for r, _, _, _ in decls})
                out.append(red(rel, "[%s] %s is declared locally, shared by %s"
                               % (table, label(name, crate), ", ".join(where)),
                               "declare it once in [workspace.dependencies] and use "
                               "`%s = { workspace = true }`" % name))
    own = collections.defaultdict(set)  # crate -> manifests giving it features of their own
    for rel, doc in manifests.items():
        for feat, entry, name in _feature_deps(doc):
            crate = names.get((rel, name))
            if crate is not None and shared(crate):
                out.append(red(rel, "[features] %s switches on %s, a feature of the shared "
                               "dependency %s" % (feat, entry, crate),
                               "add the feature to [workspace.dependencies] %s instead" % name))
            elif crate is not None:
                own[crate].add(rel)
    for crate, decls in users.items():
        for rel, _, _, spec in decls:
            if not shared(crate) and spec.get("features"):
                own[crate].add(rel)
    out.extend(_transitive_own_features(tree, own, [c for c in users if shared(c)] +
                                        [c for c in ws_crates if c not in users], red))
    return out


# tb-s10 point 18 (re-check tb-s6c L-9): a crate exactly one member declares
# with features of its own is exempt above -- but when Cargo.lock shows it
# under a shared dependency, every `-p` set without that member resolves the
# shared crate's subtree with a different feature set: a second variant, the
# cost the lock exists for (S6: 141.4 s vs 45.6 s). The two hits of the tree
# at tb-s10 are known and measured; an entry no manifest hits any more is RED,
# so the list only shrinks.
KNOWN_TRANSITIVE = {
    ("crates/meclaw-cells/Cargo.toml", "tokio-util"):
        "`codec` also arrives through h2 under hyper: no second variant (tb-s6c resolver)",
    ("crates/meclaw-api/Cargo.toml", "tower"):
        "dev-only `util`; colony alone reaches tower through jsonschema -> reqwest, the "
        "known variant of tb-s6c that needs a new direct edge (rule 6)",
}


def _lock_graph(tree):
    """`{package name: {dependency names}}` from Cargo.lock, None without one."""
    try:
        doc = _toml(os.path.join(tree, "Cargo.lock"))
    except (OSError, ValueError):
        return None
    graph = collections.defaultdict(set)
    for pkg in doc.get("package") or []:
        if isinstance(pkg, dict) and isinstance(pkg.get("name"), str):
            graph[pkg["name"]].update(str(d).split(" ", 1)[0] for d in pkg.get("dependencies") or [])
    return graph


def _transitive_own_features(tree, own, shared_crates, red):
    if not own:
        return []
    if not os.path.exists(os.path.join(tree, "Cargo.lock")):
        return []  # no resolved graph to judge (a fixture, a fresh tree); never a guess
    graph = _lock_graph(tree)
    if graph is None:  # present but unreadable: say so, the rule did not run
        return [Finding("NOTE", "feature-unity-lock", "Cargo.lock", 0,
                        "no readable Cargo.lock: %d single declaration(s) with features "
                        "not checked against the shared crates" % len(own), "")]
    under = {}  # crate -> the shared crate it lies under
    for top in sorted(shared_crates):
        stack, seen = [top], {top}
        while stack:
            for dep in sorted(graph.get(stack.pop(), ())):
                if dep not in seen:
                    seen.add(dep)
                    stack.append(dep)
                    under.setdefault(dep, top)
    out, hit = [], set()
    for crate in sorted(own):
        for rel in sorted(own[crate]):
            if crate not in under:
                continue
            if (rel, crate) in KNOWN_TRANSITIVE:
                hit.add((rel, crate))
                continue
            out.append(red(rel, "%s has features of its own here and lies under the shared "
                           "dependency %s (Cargo.lock): a second variant of %s in every -p set "
                           "without this member" % (crate, under[crate], under[crate]),
                           "move %s to [workspace.dependencies] with these features and "
                           "inherit it, or drop the features" % crate))
    for rel, crate in sorted(set(KNOWN_TRANSITIVE) - hit):
        if os.path.exists(os.path.join(tree, rel)):
            out.append(red(rel, "KNOWN_TRANSITIVE names %s here, which no longer lies under a "
                           "shared dependency with features of its own" % crate,
                           "drop the entry from KNOWN_TRANSITIVE in scripts/pipeline/locks.py"))
    return out


def check_browser_station_lock(diff_paths, tree, ctx=None):
    if not os.path.isdir(os.path.join(tree, "crates")):
        return []
    import sys
    scripts = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    import gate_plan
    return [Finding("RED", "browser-station-lock", rel, 0, why,
                    "name it in BROWSER_LOCKS of scripts/gate_plan.py with its modes, "
                    "or start the browser from the test unit itself")
            for rel, why in gate_plan.browser_unplaced(tree)]


# The one place a symlink may leave the tree (OR-TB.S10.Z.4): master carries four
# links under `plans/welle-loop-2026-10-03/` into the coding repo; no build, test or
# gate code reads them and the export does not take `plans/`.
OUT_LINK_ALLOWED = "plans/"


def _tree_paths(tree, top=""):
    """Every path under `top` (default: the whole tree) the stamp and the pin readers see
    -- tracked (index) and untracked, not ignored. The one listing of `locks` (re-check
    M3). A tree with `.git` git cannot list (safe.directory, the 30-s timeout) raises
    `Unlisted`: fail closed is RED, not a walk that reads an ignored rest again (re-check
    M5). A tree without `.git` -- `cascade_dry`'s throwaway copy, an unpacked export, a
    fixture -- is no git tree: every path of a walk, and no process (VG TV-K3)."""
    top = top.strip("/")
    if os.path.lexists(os.path.join(tree, ".git")):
        out = _git(tree, "ls-files", "-z", "--cached", "--others", "--exclude-standard",
                   *(("--", top) if top else ()))
        if out is None:
            raise Unlisted("git cannot list the tree %s%s: no pin is read from a walk of a git "
                           "tree (GH #1199)" % (tree, " under %s/" % top if top else ""))
        return sorted({p for p in out.split("\0") if p})
    found = []
    for base, dirs, files in os.walk(os.path.join(tree, top)):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for name in sorted(dirs + files):
            found.append(os.path.relpath(os.path.join(base, name), tree).replace(os.sep, "/"))
    return found


def out_links(tree):
    """`[(rel, target)]` of the symlinks outside `plans/` that resolve out of the
    tree (chains and directory links included, a dangling one by its text)."""
    root = os.path.realpath(tree)
    out = []
    for rel in _tree_paths(tree):
        path = os.path.join(tree, rel)
        if rel.startswith(OUT_LINK_ALLOWED) or not os.path.islink(path):
            continue
        to = os.path.realpath(path)
        if to != root and not to.startswith(root + os.sep):
            out.append((rel, to))
    return out


@_closed
def check_out_link_lock(diff_paths, tree, ctx=None):
    """OR-TB.S10.Z.4 fail closed (recheck M2): a symlink outside `plans/` that leaves
    the tree is RED in every pre-run, not only when `turnaround.stamp_aliases` happens
    to be asked. The stamp hashes a link as `link:<target>`, so content behind such a
    link changes no `tree_hash`; the whole tree is checked, not the diff."""
    return [Finding("RED", "out-link-lock", rel, 0,
                    "is a symlink out of the tree (to %s): the content behind it is in no "
                    "stamp" % to,
                    "vendor the content into the tree, or keep such links under plans/")
            for rel, to in out_links(tree)]


LOCKS = (
    ("pin-lock", check_pin_lock),
    ("version-lock", check_version_lock),
    ("test-line-lock", check_test_line_lock),
    ("scenario-pin-lock", check_scenario_pin_lock),
    ("set-drift-lock", check_set_drift_lock),
    ("harness-fold-lock", check_harness_fold_lock),
    ("dead-path-lock", check_dead_path_lock),
    ("feature-unity-lock", check_feature_unity_lock),
    ("pin-drift-lock", check_pin_drift_lock),
    ("bin-owner-lock", check_bin_owner_lock),
    ("browser-station-lock", check_browser_station_lock),
    ("out-link-lock", check_out_link_lock),
    ("diff-size", check_diff_size),
)


def merge_base(tree, ref=None):
    """The merge base with `ref`; without one, with `master`, else `main` (a satellite).

    Local branches first, then `origin/*` -- the order `register.main_branch` uses.
    """
    names = ["master", "main"]
    for name in ([ref] if ref else names + ["origin/" + n for n in names]):
        out = _git(tree, "merge-base", name, "HEAD")
        if out and out.strip():
            return out.strip()
    return None


def pins_to_raise(tree):
    """`pipeline pins --check`: every pin a template does not provide."""
    out = check_pin_lock([], tree) + check_scenario_pin_lock([], tree)
    return [f for f in out if f.verdict == "RED"]


def register(sub):
    p = sub.add_parser("pins", help="the pins a bump has to raise (pin-lock)")
    how = p.add_mutually_exclusive_group(required=True)
    how.add_argument("--check", action="store_true")
    how.add_argument("--cascade", action="store_true",
                     help="propose the raise of every stale pin (W9); --write executes it")
    p.add_argument("--write", action="store_true")
    p.add_argument("--repo", default=os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))))
    p.set_defaults(func=_cmd_pins)


def _cmd_pins(args):
    if args.cascade:
        from . import versions
        return versions.cmd_cascade(args)
    found = pins_to_raise(os.path.abspath(args.repo))
    for f in found:
        print("PIN %s" % render(f))
    print("PINS %d to raise" % len(found))
    return 1 if found else 0
