"""meclaw -- versions and the pin cascade, assigned at the merge point.

WHY THIS EXISTS
===============
Strands that numbered their own templates in parallel collided: two strands
both raised `builder` to the same third digit, and the second one rebased,
re-gated and re-numbered (OR-DT-13). A pin nobody raised went red after
twenty-five minutes in eight builder scenarios (GH #1108). So a strand
declares only the KIND of its change, per template, in
`plans/<wave>/straenge/<S>.json`::

    {"strand": "dt-w9", "issue": 1123, "title": "...",
     "kind": {"talky": "feature", "workspace": "fix"}}

A strand that adds a store migration also names its number there
(``"migrations": [4]``, OR-DT.store-migrations); the merge queue refuses two
open strands with the same number and merges the smaller number first.

A strand on a carrier issue -- one that stays open past the merge, such as the
release issue of a version not exported yet -- says ``"close_issue": false``:
the merge comments the issue (English, the same text) and leaves it open.
The default is true, the merge closes the strand's issues (V2-A1).

and the merge point -- the one place that sees the current master -- turns
that into numbers (development-rules § 4):

    fix      third digit   (repairing a promise; the default)
    feature  second digit  (a caller can do something never promised before)
    break    first digit   (0.x: the second -- SemVer for 0.x)

The new number is always computed from MASTER's version at the merge point,
never from the strand's base and never from a number the strand wrote: a
published number is never given twice (§ 4, no renumbering).

THE CASCADE
===========
A bumped template leaves pins behind. `cascade` raises them in the same
commit, by the two greps of development-rules § 4a:

    JSON pins   every `"<template>@<x.y.z>"` string in a JSON file under
                templates/, docs/, examples/, workshop/ (the four surfaces;
                what a run boots or instantiates) -- the same strings the
                pin-lock reads, so a cascaded tree is pin-lock clean. An
                escaped `\"<template>@<x.y.z>\"` (a script inlined as a
                JSON string, at any depth) is a pin too (GH #1169).
    full versions in Markdown  `<template>@<old>` under templates/, docs/,
                examples/ -- the second mandatory grep. Only the version
                the template had before the bump moves; an older number in
                prose is a since sentence. `docs/superpowers/plans/` is
                frozen history (R7) and never touched; `workshop/` Markdown
                is measurement records and stays.
    shorthand   `name@N` is historical by definition and never touched.

A template whose files the cascade changed has changed: it gets a fix bump
of its own, and its pinners follow (`meclaw-os` after `builder`). The loop
ends when no file moves. Satellite sets (`sets/*.json` in other
repositories) are never written here: each bumped template a satellite pins
is a `satellite_pin_needed` line for that repository.

THE HISTORY
===========
A template's `description.purpose` tells its versions as `Since <x.y.z> (`
sentences, and every number the merge point gives -- by a strand's kind or
by the cascade -- gets one of its own (gh1038, KF2 review B1). A strand that
numbered itself and wrote `Since <its number> (` keeps its sentence under the
merge point's number; otherwise `history_sentence` writes one, deterministic
and in English. The sentences before it are history and keep their pins (the
cascade never reads backticked prose inside JSON). The sentences are written
after every round, so the corpus (`before_round`) reads the final purposes.
The issues are links when the purpose already links issues, else plain `#N`
(`browser` carries no address, gh766). A purpose a generator under
`workshop/tools/` holds as a literal (builder-librarian) moves there too.
"""

import glob
import json
import os
import re
import subprocess

from . import locks

KINDS = ("fix", "feature", "break")
SURFACES = ("templates", "docs", "examples", "workshop")
PROSE_SURFACES = ("templates", "docs", "examples")
# Rust tests that lock a shipped pin as a quoted literal (W9 probe: gh504).
CODE_SURFACES = ("crates", "tests")
FROZEN = ("docs/superpowers/plans/",)
# development-rules § 4a, the second mandatory grep (full versions).
FULL_GREP = r"\b[a-z][a-z0-9-]*@[0-9]+\.[0-9]+\.[0-9]+\b"
# ... and the first one (the shorthand), which the cascade never touches.
SHORT_GREP = r"\b[a-z][a-z0-9-]*@[0-9]+([^0-9.]|$)"
MAX_ROUNDS = 8
_SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


class VersionError(ValueError):
    """A number the merge point must not give (`version_conflict`)."""


def bump(version, kind):
    m = _SEMVER.match(str(version))
    if not m:
        raise VersionError("not a version: %r" % (version,))
    x, y, z = (int(g) for g in m.groups())
    if kind == "fix":
        return "%d.%d.%d" % (x, y, z + 1)
    if kind == "feature" or (kind == "break" and x == 0):
        return "%d.%d.0" % (x, y + 1)
    if kind == "break":
        return "%d.0.0" % (x + 1)
    raise VersionError("unknown kind %r (expected %s)" % (kind, "|".join(KINDS)))


def _key(version):
    return tuple(int(p) for p in _SEMVER.match(version).groups())


def _git(repo, *args):
    res = subprocess.run(["git", "-C", repo] + list(args), capture_output=True, text=True)
    return res.stdout if res.returncode == 0 else None


# --- the strand file ------------------------------------------------------------

def strand_file(plans_wave_dir, strand):
    return os.path.join(plans_wave_dir, "straenge", "%s.json" % strand)


def new_strand_doc(strand, issues, title=""):
    return {"strand": strand, "issue": issues[0] if len(issues) == 1 else issues,
            "title": title, "kind": {}}


def read_strand(path):
    """The strand file, or a default one (`kind` empty: every template `fix`)."""
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        text = None
    return strand_doc(text, path)


def strand_doc(text, where):
    """The strand file's document from its text (`None`: no file, the default
    one). A file that is there but broken is an error, never the default: the
    default numbers every template `fix`, a silent downgrade of a feature."""
    try:
        doc = json.loads(text) if text is not None else {}
    except ValueError as exc:
        raise VersionError("%s: not JSON (%s)" % (where, exc))
    if not isinstance(doc, dict):
        raise VersionError("%s: not a JSON object" % where)
    kind = doc.get("kind", {})
    if not isinstance(kind, dict):
        raise VersionError("%s: kind is not an object" % where)
    for name, value in kind.items():
        if value not in KINDS:
            raise VersionError("%s: kind of %s is %r (expected %s)"
                               % (where, name, value, "|".join(KINDS)))
    doc["kind"] = kind
    nums = doc.get("migrations", [])
    if not isinstance(nums, list) or any(
            isinstance(n, bool) or not isinstance(n, int) or n < 1 for n in nums):
        raise VersionError("%s: migrations is %r (expected a list of store migration "
                           "numbers, OR-DT.store-migrations)" % (where, nums))
    if not isinstance(doc.get("close_issue", True), bool):
        raise VersionError("%s: close_issue is %r (expected true or false)"
                           % (where, doc.get("close_issue")))
    return doc


def close_issue_of(doc):
    """Whether the merge closes the strand's issues (`"close_issue"`, default
    true); false: it only comments them (a carrier issue, V2-A1)."""
    return doc.get("close_issue", True) is not False


def migrations_of(doc):
    """The store migrations a strand adds (`"migrations": [n, ...]`), sorted."""
    return sorted(set(doc.get("migrations") or []))


def issues_of(doc):
    raw = doc.get("issue")
    raw = raw if isinstance(raw, list) else ([raw] if raw not in (None, "") else [])
    return [str(i).lstrip("#") for i in raw]


# --- the tree -------------------------------------------------------------------

def template_dirs(tree):
    """`{dir: name}` of every `templates/<dir>/template.json`."""
    out = {}
    root = os.path.join(tree, "templates")
    if not os.path.isdir(root):
        return out
    for d in sorted(os.listdir(root)):
        path = os.path.join(root, d, "template.json")
        try:
            with open(path, encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            continue
        if isinstance(doc, dict) and doc.get("version"):
            out[d] = doc.get("name") or d
    return out


def touched_templates(paths, dirs):
    """Template names whose files a path list touches."""
    out = set()
    for p in paths:
        parts = p.split("/")
        if len(parts) >= 3 and parts[0] == "templates" and parts[1] in dirs:
            out.add(dirs[parts[1]])
    return out


def version_at(repo, ref, d):
    text = _git(repo, "show", "%s:templates/%s/template.json" % (ref, d))
    if text is None:
        return None
    try:
        doc = json.loads(text)
    except ValueError:
        return None
    return str(doc.get("version")) if isinstance(doc, dict) and doc.get("version") else None


def _tree_version(tree, d):
    try:
        with open(os.path.join(tree, "templates", d, "template.json"), encoding="utf-8") as fh:
            doc = json.load(fh)
    except (OSError, ValueError):
        return None
    return str(doc.get("version")) if isinstance(doc, dict) and doc.get("version") else None


def _set_template_version(tree, d, version):
    path = os.path.join(tree, "templates", d, "template.json")
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    new, n = re.subn(r'("version"\s*:\s*")[^"]*(")', r"\g<1>%s\g<2>" % version, text, count=1)
    if n != 1:
        raise VersionError("%s has no version field" % path)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(new)


def workspace_version(text):
    return locks._version_of(text, "Cargo.toml")


def set_workspace_version(tree, new):
    """Cargo.toml `[workspace.package] version` and the members in Cargo.lock.

    No cargo: the lock file's workspace members carry the workspace version
    and have no `source` line, so the rewrite is exact -- the lane half of the
    merge gate builds with it (a lock diff without a build is a red merge).
    Returns `(old, raised)`: the version before and the inter-crate pins it
    raised, `[{path, line, template, from, to}]` like `cascade` (GH #1199, 3c:
    the merge line said `Pins: none` while 0.64.0 raised them)."""
    cargo = os.path.join(tree, "Cargo.toml")
    with open(cargo, encoding="utf-8") as fh:
        text = fh.read()
    old = workspace_version(text)
    if old is None:
        raise VersionError("Cargo.toml has no [workspace.package] version")
    text = re.sub(r"(^\[workspace\.package\][^\[]*?^version\s*=\s*\")[^\"]+(\")",
                  r"\g<1>%s\g<2>" % new, text, count=1, flags=re.MULTILINE | re.DOTALL)
    with open(cargo, "w", encoding="utf-8") as fh:
        fh.write(text)
    lock = os.path.join(tree, "Cargo.lock")
    if os.path.isfile(lock):
        with open(lock, encoding="utf-8") as fh:
            blocks = fh.read().split("\n[[package]]\n")
        for i, block in enumerate(blocks):
            if 'version = "%s"' % old in block and "\nsource = " not in block \
                    and not block.startswith("# "):
                blocks[i] = block.replace('version = "%s"' % old, 'version = "%s"' % new, 1)
        with open(lock, "w", encoding="utf-8") as fh:
            fh.write("\n[[package]]\n".join(blocks))
    return old, _raise_inter_crate_pins(tree, new)


# A path dependency on a workspace crate carries a caret requirement
# (`version = "0.63.1"`). A patch bump stays inside it; the first minor bump
# through the merge point (0.63.5 -> 0.64.0, welle-validierung V-5) did not,
# and cargo refused the candidate in 22 s. Only `meclaw-*` path entries move;
# a registry dependency with the same number is never touched.
_PIN = re.compile(r'(meclaw-[a-z-]+ = \{ path = "\.\./meclaw-[a-z-]+", version = ")'
                  r'(\d+)\.(\d+)\.(\d+)(")')


def _admits(req, new):
    """Does the caret requirement `req` (a, b, c) admit `new`?"""
    if req[0] > 0:
        return new[0] == req[0] and new >= req
    if req[1] > 0:
        return new[:2] == req[:2] and new >= req
    return new == req


def _raise_inter_crate_pins(tree, new):
    """Raise every `meclaw-*` path pin `new` no longer satisfies; returns
    `[{path, line, template, from, to}]` (`template` = the pinned crate)."""
    want = tuple(int(x) for x in new.split("."))
    raised = []

    for path in sorted(glob.glob(os.path.join(tree, "crates", "*", "Cargo.toml"))):
        rel = os.path.relpath(path, tree).replace(os.sep, "/")
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
        changed = False
        for i, line in enumerate(lines):
            def sub(m, i=i):
                req = (int(m.group(2)), int(m.group(3)), int(m.group(4)))
                if _admits(req, want):
                    return m.group(0)
                raised.append({"path": rel, "line": i + 1,
                               "template": m.group(1).split(" ", 1)[0],
                               "from": "%d.%d.%d" % req, "to": new})
                return "%s%s%s" % (m.group(1), new, m.group(5))
            out = _PIN.sub(sub, line)
            if out != line:
                lines[i], changed = out, True
        if changed:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("\n".join(lines))
    return raised


# --- the cascade -------------------------------------------------------------------

def _files(tree, top, suffix):
    root = os.path.join(tree, top)
    if not os.path.isdir(root):
        return
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in locks.SKIP_DIRS)
        for fn in sorted(filenames):
            if fn.endswith(suffix):
                rel = os.path.relpath(os.path.join(dirpath, fn), tree).replace(os.sep, "/")
                if not rel.startswith(FROZEN):
                    yield rel


def cascade(tree, moved, also=None):
    """Raise the pins of `moved` (`{name: (old, new)}`) in place.

    `also` (`{name: version}`) names one more number prose may still claim:
    the one the candidate's own diff put into `template.json` before the
    merge point numbered it -- a revert restores the pre-merge number, and
    the Markdown it restores names that one, not master's.
    Returns `[{path, line, template, from, to}]` -- what the cascade changed."""
    if not moved:
        return []
    also = also or {}
    names = sorted(moved, key=len, reverse=True)
    alt = "|".join(re.escape(n) for n in names)
    # one pin form with the stale-pin scan and the net: `"name@x.y.z"`, escaped to
    # any depth -- a script inlined as a JSON string (GH #1169)
    json_rx = locks.pin_position_re(names)
    prose_rx = re.compile(r"(?<![A-Za-z0-9_./-])(?P<name>%s)@(?P<ver>%s)(?![0-9.]*[0-9])"
                          % (alt, locks._VERSION))
    out = []

    def rewrite(rel, rx, only_old):
        path = os.path.join(tree, rel)
        try:
            with open(path, encoding="utf-8") as fh:
                lines = fh.read().split("\n")
        except (OSError, UnicodeDecodeError):
            return
        changed = False
        for no, line in enumerate(lines):
            if "@" not in line:
                continue

            def sub(m, no=no, rel=rel):
                nonlocal changed
                name, ver = m.group("name"), m.group("ver")
                old, new = moved[name]
                if ver == new or (only_old and ver not in (old, also.get(name))):
                    return m.group(0)
                changed = True
                out.append({"path": rel, "line": no + 1, "template": name,
                            "from": ver, "to": new})
                return m.group(0).replace("%s@%s" % (name, ver), "%s@%s" % (name, new), 1)
            lines[no] = rx.sub(sub, line)
        if changed:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("\n".join(lines))

    for top in SURFACES:
        for rel in _files(tree, top, ".json"):
            rewrite(rel, json_rx, only_old=False)
    for top in PROSE_SURFACES:
        for rel in _files(tree, top, ".md"):
            rewrite(rel, prose_rx, only_old=True)
    for top in CODE_SURFACES:
        for rel in _files(tree, top, ".rs"):
            rewrite(rel, json_rx, only_old=True)
    out.extend(catalogue(tree, moved, alt))
    return out


CATALOGUE = "templates/README.md"


def catalogue(tree, moved, alt):
    """The version column of the public catalogue (`scripts/check_catalogue.py`):
    a listed template's row names the number it ships at."""
    path = os.path.join(tree, CATALOGUE)
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
    except (OSError, UnicodeDecodeError):
        return []
    rx = re.compile(r"^(\|\s*\[`(%s)`\]\([^)]*\)\s*\|\s*)(%s)(\s*\|)" % (alt, locks._VERSION))
    out = []
    for no, line in enumerate(lines):
        hit = rx.match(line)
        if not hit or hit.group(3) == moved[hit.group(2)][1]:
            continue
        new = moved[hit.group(2)][1]
        out.append({"path": CATALOGUE, "line": no + 1, "template": hit.group(2),
                    "from": hit.group(3), "to": new})
        lines[no] = hit.group(1) + new + hit.group(4) + line[hit.end():]
    if out:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines))
    return out


def satellite_pins(repos, moved):
    """`[(repo, template, version)]` for every satellite set that pins a moved template."""
    out = []
    for repo in repos:
        for rel in locks._sets(repo):
            text = "\n".join(locks._lines(repo, rel))
            for name in sorted(moved):
                if re.search(r'"%s@%s"' % (re.escape(name), locks._VERSION), text) \
                        or re.search(r'"%s"\s*:\s*"%s"' % (re.escape(name), locks._VERSION), text):
                    out.append((os.path.basename(os.path.normpath(repo)), name, moved[name][1]))
    return sorted(set(out))


ISSUE_URL = "https://github.com/mmeyerlein/meclaw/issues/%s"
_DIGIT_AT = ("first", "second", "third")


def _join(items):
    return items[0] if len(items) == 1 else "%s and %s" % (", ".join(items[:-1]), items[-1])


def history_sentence(new, old, issues, pins, cascade, revert=False, linked=True):
    """The `Since <new> (` sentence of a number the merge point gave.

    `linked` follows the purpose's own convention: a purpose that links no
    issue (`browser`, gh766: no address in its files) gets a plain `#N`."""
    links = ", ".join(("[#%s](%s)" % (i, ISSUE_URL % i)) if linked else "#%s" % i
                      for i in issues) or "this merge"
    head = "Since %s (%s%s)" % (new, "revert of " if revert else "", links)
    pinned = _join(["`%s@%s`" % p for p in sorted(pins.items())]) if pins else ""
    if cascade and not revert and pinned:
        what = "the template pins %s" % pinned
    else:
        what = "the template %s" % ("takes this change back" if revert else "carries this change")
        if pinned:
            what += " and pins %s" % pinned
    at = next((i for i, (a, b) in enumerate(zip(_SEMVER.match(old).groups(),
                                                    _SEMVER.match(new).groups())) if a != b), 2)
    return "%s %s; it is the %s digit." % (head, what, _DIGIT_AT[at])


def _purpose_span(text):
    """`(start, end)` of the raw `description.purpose` literal (quotes included), or None."""
    try:
        want = json.loads(text)["description"]["purpose"]
    except (ValueError, KeyError, TypeError):
        return None
    if not isinstance(want, str):
        return None
    for hit in re.finditer(r'"purpose"\s*:\s*"', text):
        i = hit.end()
        while i < len(text) and text[i] != '"':
            i += 2 if text[i] == "\\" else 1
        try:
            if json.loads(text[hit.end() - 1:i + 1]) == want:
                return hit.end() - 1, i + 1
        except ValueError:
            continue
    return None


# Generators that hold a template's purpose as a literal (builder-librarian:
# `workshop/tools/build_librarian.py`, whose `--check` compares the template).
GENERATORS = ("workshop/tools",)


def _mirror(tree, old_lit, new_lit):
    """Apply a purpose edit to the generator literal that holds the same text
    (the generator may spell it unescaped where the JSON escapes, `§`)."""
    old, new = json.loads(old_lit), json.loads(new_lit)
    pairs = [(json.dumps(old, ensure_ascii=a), json.dumps(new, ensure_ascii=a)) for a in (False, True)]
    for top in GENERATORS:
        for rel in _files(tree, top, ".py"):
            path = os.path.join(tree, rel)
            try:
                with open(path, encoding="utf-8") as fh:
                    text = fh.read()
            except (OSError, UnicodeDecodeError):
                continue
            for old_src, new_src in pairs:
                if text.count(old_src) == 1:
                    with open(path, "w", encoding="utf-8") as fh:
                        fh.write(text.replace(old_src, new_src))
                    break


def write_history(tree, base_ref, plan, pins, history, written, repo):
    """Give every number in `plan` its `Since` sentence (THE HISTORY above).

    `written` (`{name: raw sentence}`) remembers what an earlier round wrote,
    so a later round replaces it with the pins it has collected since."""
    for name, p in sorted(plan.items()):
        if p["kind"] == "new":
            continue
        path = os.path.join(tree, "templates", p["dir"], "template.json")
        try:
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
        except OSError:
            continue
        span = _purpose_span(text)
        if span is None:
            continue
        lit = text[span[0]:span[1]]
        own = {}
        for pin in pins:
            if pin["path"].startswith("templates/%s/" % p["dir"]) and pin["template"] != name:
                own[pin["template"]] = pin["to"]
        mark = "Since %s (" % p["to"]
        if name in written:
            new_lit = lit.replace(written.pop(name), "", 1)
        elif mark in lit:
            continue
        else:
            new_lit = lit
            cur = p.get("strand")
            base = _git(repo, "show", "%s:templates/%s/template.json" % (base_ref, p["dir"])) or ""
            if cur and cur not in (p["from"], p["to"]):
                theirs = "Since %s (" % cur
                if lit.count(theirs) > base.count(theirs):
                    at = lit.rfind(theirs)
                    new_lit = lit[:at] + mark + lit[at + len(theirs):]
        if mark not in new_lit:
            raw = json.dumps(" " + history_sentence(
                p["to"], p["from"], history.get("issues") or [], own, p.get("cascade", False),
                history.get("revert", False), ISSUE_URL % "" in lit), ensure_ascii=False)[1:-1]
            new_lit = new_lit[:-1] + raw + '"'
            written[name] = raw
        if new_lit != lit:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text[:span[0]] + new_lit + text[span[1]:])
            _mirror(tree, lit, new_lit)


def assign(tree, base_ref, changed_paths, kinds, repo=None, before_round=None, history=None):
    """The merge point's numbers: bump, cascade, repeat until nothing moves.

    `tree` is the candidate (master + the strand, not yet committed),
    `base_ref` the master commit it stands on, `changed_paths` the candidate's
    diff against it, `kinds` the strand file's `kind`. `before_round(tree)`
    may change files (the corpus) and returns the paths it changed.
    `history` (`{issues, revert}`) writes the `Since` sentences (THE HISTORY).
    Returns `{versions, pins, workspace}`; raises `VersionError`."""
    repo = repo or tree
    written = {}
    dirs = template_dirs(tree)
    by_name = {n: d for d, n in dirs.items()}
    plan, pins, cascaded = {}, [], set()
    todo = {n: kinds.get(n, "fix") for n in touched_templates(changed_paths, dirs)}
    for _round in range(MAX_ROUNDS):
        if before_round:
            extra = {n for n in touched_templates(before_round(tree), dirs)
                     if n not in plan and n not in todo}
            cascaded |= extra
            todo.update({n: "fix" for n in extra})
        todo = {n: k for n, k in todo.items() if n not in plan}
        if not todo:
            break
        moved, also = {}, {}
        for name, kind in sorted(todo.items()):
            d = by_name[name]
            old = version_at(repo, base_ref, d)
            cur = _tree_version(tree, d)
            if old is None:
                plan[name] = {"from": None, "to": None, "kind": "new", "dir": d}
                continue
            new = bump(old, kind)
            if _key(new) <= _key(old):
                raise VersionError("%s: %s -> %s renumbers" % (name, old, new))
            _set_template_version(tree, d, new)
            plan[name] = {"from": old, "to": new, "kind": kind, "dir": d, "strand": cur,
                          "cascade": name in cascaded}
            moved[name] = (old, new)
            if cur and cur not in (old, new):
                also[name] = cur
        todo = {}
        raised = cascade(tree, moved, also)
        pins.extend(raised)
        todo = {n: "fix" for n in touched_templates([p["path"] for p in raised], dirs)
                if n not in plan}
        cascaded |= set(todo)
        if history is not None:
            write_history(tree, base_ref, plan, pins, history, written, repo)
    else:
        raise VersionError("the cascade did not settle in %d rounds" % MAX_ROUNDS)
    workspace = None
    if kinds.get("workspace"):
        text = _git(repo, "show", "%s:Cargo.toml" % base_ref)
        old = workspace_version(text or "")
        if old:
            new = bump(old, kinds["workspace"])
            _old, raised = set_workspace_version(tree, new)
            pins.extend(raised)
            workspace = {"from": old, "to": new, "kind": kinds["workspace"]}
    versions = {}
    for n, p in plan.items():
        if p["kind"] == "new":
            continue
        versions[n] = {k: v for k, v in p.items() if k in ("from", "to", "kind")}
        if n in cascaded:
            versions[n]["cascade"] = True
    return {"versions": versions, "pins": pins, "workspace": workspace}


def version_line(result):
    """`Versions: talky 4.1.0 -> 4.2.0 (feature), meclaw-os 2.2.4 -> 2.2.5 (fix)`."""
    parts = ["%s %s -> %s (%s)" % (n, v["from"], v["to"], v["kind"])
             for n, v in sorted(result["versions"].items())]
    if result.get("workspace"):
        w = result["workspace"]
        parts.append("workspace %s -> %s (%s)" % (w["from"], w["to"], w["kind"]))
    return "Versions: " + (", ".join(parts) if parts else "none")


def pins_line(result):
    pins = result.get("pins") or []
    if not pins:
        return "Pins: none"
    files = sorted({p["path"] for p in pins})
    return "Pins: %d raised in %d files (%s)" % (len(pins), len(files), ", ".join(files[:8])
                                                + (", ..." if len(files) > 8 else ""))


DIGIT = {"fix": "third digit", "feature": "second digit", "break": "first digit"}


def changelog_line(result, issues):
    """The `[Unreleased]` line in today's form, or None without a bump."""
    vs = result["versions"]
    if not vs:
        return None
    tag = ", ".join("#%s" % i for i in issues)
    first = [n for n, v in sorted(vs.items()) if not v.get("cascade")]
    follow = [n for n, v in sorted(vs.items()) if v.get("cascade")]
    groups = {}
    for n in first:
        groups.setdefault(vs[n]["kind"], []).append("`%s@%s`" % (n, vs[n]["to"]))
    text = "; ".join("%s, %s" % (DIGIT[k], ", ".join(v)) for k, v in sorted(groups.items()))
    if follow:
        text += "%sthird digit for the pins that follow, %s" % (
            "; " if text else "",
            ", ".join("`%s@%s`" % (n, vs[n]["to"]) for n in follow))
    return "- **Template versions (%s):** %s." % (tag, text)


def add_changelog(tree, line):
    """Append `line` at the end of the `## [Unreleased]` section."""
    path = os.path.join(tree, "CHANGELOG.md")
    if not line or not os.path.isfile(path):
        return False
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    try:
        start = next(i for i, l in enumerate(lines) if l.startswith("## [Unreleased]"))
    except StopIteration:
        return False
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## [")),
               len(lines))
    at = end
    while at > start + 1 and not lines[at - 1].strip():
        at -= 1
    lines[at:at] = ([""] if not lines[at - 1].startswith("- ") else []) + [line]
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines))
    return True


# --- `pipeline pins --cascade` ----------------------------------------------------

def cascade_proposal(tree):
    """Every stale JSON pin the pin-lock sees, as `{name: (old, tree version)}` moves."""
    found = locks.pins_to_raise(tree)
    versions = locks.template_versions(tree)
    out = []
    for f in found:
        name = f.text.split("@", 1)[0]
        old = f.text.split("@", 1)[1].split()[0]
        out.append({"path": f.path, "line": f.line, "template": name,
                    "from": old, "to": versions.get(name)})
    return out


def cmd_cascade(args):
    tree = os.path.abspath(args.repo)
    proposal = cascade_proposal(tree)
    if args.write and proposal:
        names = {p["template"]: p["to"] for p in proposal}
        # Raise every stale JSON pin to the tree's version (the pin-lock's rule),
        # in the one pin form the proposal read -- escaped ones too (GH #1169).
        rx = locks.pin_position_re(names)
        for p in proposal:
            path = os.path.join(tree, p["path"])
            with open(path, encoding="utf-8") as fh:
                lines = fh.read().split("\n")
            i = p["line"] - 1
            lines[i] = rx.sub(
                lambda m, p=p: m.group(0) if (m.group("name"), m.group("ver")) != (
                    p["template"], p["from"]) else m.group(0).replace(
                    "@" + p["from"], "@" + names[p["template"]]), lines[i])
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("\n".join(lines))
    for p in proposal:
        print("CASCADE %s:%d %s@%s -> %s%s" % (p["path"], p["line"], p["template"], p["from"],
                                               p["to"], "" if args.write else " (proposal)"))
    print("CASCADE %d pins %s" % (len(proposal), "raised" if args.write else "to raise"))
    return 0
