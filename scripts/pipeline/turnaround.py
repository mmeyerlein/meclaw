#!/usr/bin/env python3
"""meclaw -- gate a strand once (GH #1167): the code tree, not the commit.

WHY THIS EXISTS
===============
Landing one strand cost pre, strand gate, re-check, report commit, pre again,
gate again and the merge-queue gate: every commit after the receipt (report,
review verdict, ledger line) and every head `strand.sh report` rewrote broke
the receipt (`receipt_not_branch_head`) or the pre stamp (`pre_stamp`), and
the queue named one missing precondition per attempt. A gate judges a TREE:
what it reads. Files nobody reads cannot change its verdict.

GATE-NEUTRAL PATHS
==================
A path is gate-neutral when BOTH hold (fail-closed: anything else is code):

  1. it lies in the allowlist `NEUTRAL` -- `POST_GATE` of a wave (reports,
     strand files, RESTLISTE.md, re-check scripts); `plans/**` as a
     whole is NOT neutral: the corridor drift lock reads
     `plans/**/expected_*_body.txt`, `check_adr_anchors.py` reads `plans/adr/`;
  2. no reader rule knows a reader of it: the classes of `gate_plan.py` are
     exactly `{docs}`, no test names it (rule 8 literal) or reads it at
     runtime (rule 8 runtime, GH #1138), and no Python or shell file under
     `scripts/` and no Python file under `workshop/` names it (rule 8 python:
     the path, its wave directory -- `test_wave_retro` joins
     `REPO / "plans" / "<wave>"` -- or its file name unless every wave
     carries it, `GENERIC_NAMES`).

RECEIPT REUSE (`code_equal_revs`)
=================================
A strand receipt on rev `a` still covers the tip `b` by CONSTRUCTION, with no
link resolution and no tree materialisation (Koordinator-3, fix round 7, after
six rounds of reader-rule leaks B-N1..B-N4 around symlinks, attributes and
`..`): every path of `a..b`

  * lies on `POST_GATE` of the strand's own wave (`plans/<wave>/`
    berichte/, straenge/, RESTLISTE.md, recheck/), with no `.`/`..`/empty
    component;
  * is a regular 100644 file in the tip, and a regular 100644 file or
    absent in the gated rev and master, and no component above it is
    anything but a directory there -- a symlink, a submodule or a mode
    change anywhere on the path re-gates; a new file keeps the receipt, a
    deleted one re-gates (Koordinator-3 ruling on "100644 on both sides");
  * is named by no script or test of the tip or of master (`reuse_readers`:
    `git grep` over the committed files, rule 8 python plus
    `crates/**`) -- a report `test_wave_retro` reads re-gates.

Only the paths of the diff count; other links of the tree do not.

WHAT USES IT
============
  * `pre.stamp_status`: a stamp holds while the worktree is code-equal to the
    stamped tree (the stamp records HEAD and the overlay digests) -- in the
    wave of the strand's branch only (`branch_wave`); no wave, nothing neutral.
  * `merge.preconditions`: the strand receipt holds while the branch tip is
    code-equal to the gated rev.
  * `merge`: the merge gate plans from the strand tip (`gate_base`) when the
    strand receipt covers the tip, comes from a FULL strand gate on a clean
    tree (`partial_receipt`: not dirty, no `--only`, no owner decision, every planned station run) and its base lies on master: only
    master's move and the merge point's own edits are gated. In doubt: full.
  * `merge`: a branch whose diff is gate-neutral only merges without gate and
    without report (plan-only).
  * `pre` checks `main_tree` (the fast-forward would hit `main_tree_dirty`)
    and `owner` (author and committer of `<default branch>..HEAD`, `default_branch`).
"""

import hashlib
import os
import re
import subprocess
import sys

# The ONE list of what the chain writes, documented, after the strand gate,
# relative to `plans/<wave>/` (GH #1199 (e)): the reports, the strand files,
# and since Konvergenz A (PREAMBLE § 6.13) the RESTLISTE.md and the re-check
# scripts the delta check writes. Two copies (`NEUTRAL` globs, a
# `REUSE_DIRS` of names) knew only the first two: v2-p1082 refused
# `receipt_not_branch_head` on 14.10. with RESTLISTE.md alone changed after
# its gate. Globs of `gate_plan._matches` (`**` crosses `/`, `*` does not);
# nothing outside `plans/<wave>/`.
POST_GATE = ("berichte/**", "straenge/**", "RESTLISTE.md", "recheck/**")

NEUTRAL = tuple("plans/welle-*/" + g for g in POST_GATE)

# Receipt reuse (`code_equal_revs`): a receipt survives `POST_GATE` of its
# wave (above); the committed files whose naming a changed path
# re-gates (rule 8 python plus every crate file -- integration tests and the
# `#[cfg(test)]` modules of `src/` alike; `git grep` reads the commit, no
# symlink is followed, no attribute applies). Measured on master 10.10.:
# one such `git grep` over all 1 982 crate files takes 0.9 s.
REUSE_TESTS = (":(glob)crates/**",)

OWNER_ENV = "MECLAW_OWNER"

# Rule 8 python (B1 of the re-check): where a reader of `plans/` may live
# outside the Rust tests. Shell scripts count: a station may be one.
PY_READERS = (":(glob)scripts/**/*.py", ":(glob)scripts/**/*.sh", ":(glob)workshop/**/*.py")
# File names every wave carries: they name no one wave's file, the path and
# the wave directory still do. The session markers count (re-check I-1/m-3):
# DISPATCH-BAU.md of the build session, HANDOVER-PLANUNG.md of the planning
# session (`retro.transcripts.PLANNING`) -- a prompt naming another wave's
# marker reads no file of this one.
# GH #1199 (e): `RESTLISTE.md` (PREAMBLE § 6.13) and the re-check entry
# `run_all.sh` (DISPATCH-BAU § 1; 6 on master 15.10.) are wave-generic too --
# `POST_GATE` itself names RESTLISTE.md, which made this module its reader.
GENERIC_NAMES = frozenset(("ledger.md", "README.md", "summary.txt", "receipt.json",
                           "plan.json", "events.jsonl", "latest",
                           "DISPATCH-BAU.md", "HANDOVER-PLANUNG.md",
                           "RESTLISTE.md", "run_all.sh"))

# A gate run's archive directory (`<ts>-<sha8>`, `gate.py` `archive`).
RUN_DIR = re.compile(r"^\d{8}T\d{6}Z-[0-9a-f]{8}$")

# Verdicts of a planned station that count as run (GREEN, a cache hit, a
# flake with an issue, a finding to read). SKIP, RED, ABORTED, ASK do not.
RAN = frozenset(("GREEN", "CACHED", "NOTE"))   # FLAKY: only a legacy one (OR-TV-2)


def _git(repo, *args):
    res = subprocess.run(["git", "-C", repo] + list(args), capture_output=True)
    return res.returncode, res.stdout


def _lines(repo, *args):
    rc, raw = _git(repo, *args)
    if rc != 0:
        return None
    return [l for l in raw.decode("utf-8", "surrogateescape").split("\n") if l]


def _gate_plan():
    scripts = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    import gate_plan
    return gate_plan


# --- gate-neutral paths --------------------------------------------------------

def allowlisted(path, wave=None):
    """In `NEUTRAL`; with `wave` (a `plans/` directory name) in THAT wave only --
    a strand's receipt is not kept by another wave's bookkeeping."""
    gp = _gate_plan()
    globs = NEUTRAL if not wave else tuple(g.replace("welle-*", wave) for g in NEUTRAL)
    return any(gp._matches(path, g) for g in globs)


def veto(path, repo, wave=None, python=True, allow=None):
    """Why `path` is NOT gate-neutral, or None. Fail closed: a reader rule
    that errors vetoes. `python=False` leaves rule 8 python to the caller
    (`offending` runs it once over every path). `allow` replaces the
    allowlist (`plan_allowlisted` for the plan of a new wave, Fund V-1)."""
    if not (allow or allowlisted)(path, wave):
        return "outside the allowlist"
    gp = _gate_plan()
    try:
        classes = set(gp.classify([path]))
        # GH #1199 (e): `shell` plans shellcheck over `SHELL_GLOBS` alone; a
        # re-check's `run_all.sh` under plans/<wave>/recheck/ is none of them.
        if "shell" in classes and not any(gp._matches(path, g) for g in gp.SHELL_GLOBS):
            classes = (classes - {"shell"}) or {"docs"}
        if classes != {"docs"}:
            return "classes %s" % ",".join(sorted(classes))
        lit = gp._tests_reading_path(repo, path)
        if lit:
            return "rule 8 literal: %s" % ",".join(sorted(lit)[:3])
        run = gp._tests_reading_at_runtime(repo, path)
        if run:
            return "rule 8 runtime: %s" % ",".join(sorted(run)[:3])
        py = python_readers(repo, path) if python else []
        if py:
            return "rule 8 python: %s" % ",".join(py[:3])
    except Exception as exc:  # noqa: BLE001 -- fail closed on any reader error
        return "reader rules failed: %s" % exc
    return None


def _needles(path):
    """What a reader of `path` would have to name, as `(needle, whole)`: the
    path and -- unless every wave carries it -- its file name (`whole`
    False: matched at a path boundary), and each directory between `plans/`
    and the file (`whole` True: only where the name ENDS the component run,
    `_bounded`). A reader naming the wave directory alone (`Path("plans") /
    "<wave>"`) or a directory of it (`"plans/<wave>/berichte/*.md"`) reads
    every file below it; a text naming `plans/<wave>/berichte/<other>.md`
    names that one file, not the directory -- as a wave-directory needle it
    made every report commit of the wave non-neutral (K-R2: a doc comment in
    `writer.rs`). Under `receipts/` the runner names every file
    (`summary.txt`, `logs/<mode>-<station>.log`): there the run directory
    stands in for the file name."""
    parts = path.split("/")
    out = {(path, False)}
    if len(parts) > 2 and parts[0] == "plans":
        for i in range(2, len(parts)):
            out.add(("/".join(parts[1:i]), True))
    if len(parts) > 3 and parts[0] == "plans" and parts[2] == "receipts":
        out.update((c, False) for c in parts[3:-1] if RUN_DIR.match(c))
        return sorted(out)
    name = parts[-1]
    if name not in GENERIC_NAMES and not (name.startswith("last-") and name.endswith(".json")):
        out.add((name, False))
    return sorted(out)


def _bounded(needle, whole=False):
    """An ERE matching `needle` only as a whole path component run: a plain
    substring match made a comment naming `recheck-<name>` a reader of
    `berichte/<name>` (re-check m-N7) -- over-selection that cost a re-gate.
    `whole`: the run must also END there -- `/` plus a further component
    name after it is a deeper path, not this directory (`/*`, `/"`, `/`
    at the end still are)."""
    lit = "".join("\\" + c if c in ".[]()*+?{}|^$\\" else c for c in needle)
    tail = "([^A-Za-z0-9_/]|/?$|/[^A-Za-z0-9_.-])" if whole else "([^A-Za-z0-9_]|$)"
    return "(^|[^A-Za-z0-9_.-])" + lit + tail


def python_readers(repo, path, rev=None, specs=PY_READERS):
    """Rule 8 python: the files under `PY_READERS` (tracked or untracked)
    that name `path` (`_needles`, each at a path boundary, `_bounded`); with
    `rev`, the files of that commit; `specs` the pathspecs searched; `path`
    may be a list (the files naming any of them). A
    failing `git grep` raises -- `veto` turns that into a veto.

    Names come verbatim (`-z`): without it git quotes a name with non-ASCII,
    `"` or `\\` (`"HEAD:scripts/\\303\\244.py"`), the `rev:` prefix stayed on
    and the `:(literal)<hit>` grep of `offending`/`reuse_readers` matched
    nothing -- the reader fell through (re-check I-h1, wave validation 2)."""
    args = ["grep", "-l", "-z", "-E"] + ([] if rev else ["--untracked"])
    for n, whole in sorted({n for q in ([path] if isinstance(path, str) else path)
                            for n in _needles(q)}):
        args += ["-e", _bounded(n, whole)]
    rc, raw = _git(repo, *(args + ([rev] if rev else []) + ["--"] + list(specs)))
    if rc == 1:
        return []
    if rc != 0:
        raise ValueError("git grep for readers of %s failed (rc %d)" % (path, rc))
    got = [l for l in raw.decode("utf-8", "surrogateescape").split("\0") if l]
    if rev:
        got = [l[len(rev) + 1:] if l.startswith(rev + ":") else l for l in got]
    return sorted(got)


def branch_wave(repo):
    """The wave directory of the strand's branch `<wave>/<strand>`, as
    `strand.sh` resolves it (`branch_wave_dir`): the one `plans/<wave>-*`, else
    the one holding `berichte/<strand>.md`; None in doubt."""
    got = _lines(repo, "rev-parse", "--abbrev-ref", "HEAD")
    if not got or "/" not in got[0]:
        return None
    prefix, strand = got[0].split("/", 1)[0], got[0].rsplit("/", 1)[-1]
    plans = os.path.join(repo, "plans")
    try:
        hits = sorted(d for d in os.listdir(plans)
                      if d.startswith(prefix + "-") and os.path.isdir(os.path.join(plans, d)))
    except OSError:
        return None
    if len(hits) == 1:
        return hits[0]
    owners = [d for d in hits if os.path.isfile(os.path.join(plans, d, "berichte", strand + ".md"))]
    return owners[0] if len(owners) == 1 else None


def offending(paths, repo, wave=None, allow=None):
    """`{path: why}` of the paths that are not gate-neutral. Rule 8 python
    runs ONE `git grep` over every path the other rules let through and, only
    on a hit, one per path over the hit files (K-R1: one `--untracked` grep
    per diff path cost a merge preparation 16 min)."""
    out, rest = {}, []
    for p in sorted(set(paths)):
        why = veto(p, repo, wave, python=False, allow=allow)
        if why:
            out[p] = why
        else:
            rest.append(p)
    try:
        hits = python_readers(repo, rest) if rest else []
        for p in rest if hits else ():
            py = python_readers(repo, [p], specs=tuple(":(literal)" + h for h in hits))
            if py:
                out[p] = "rule 8 python: %s" % ",".join(py[:3])
    except Exception as exc:  # noqa: BLE001 -- fail closed on any reader error
        for p in rest:
            out.setdefault(p, "reader rules failed: %s" % exc)
    return out


def neutral_only(paths, repo, wave=None):
    return bool(paths) and not offending(paths, repo, wave)


def describe(bad, limit=5):
    items = ["%s (%s)" % (p, w) for p, w in sorted(bad.items())]
    more = len(items) - limit
    return ", ".join(items[:limit]) + (" and %d more" % more if more > 0 else "")


# --- code-equal trees -----------------------------------------------------------

def changed(repo, a, b):
    """The paths of `a..b`, unquoted (`-z`)."""
    rc, raw = _git(repo, "diff", "--no-renames", "--name-only", "-z", a, b)
    if rc != 0:
        raise ValueError("git diff %s %s failed" % (a[:9], b[:9]))
    return [p for p in raw.decode("utf-8", "surrogateescape").split("\0") if p]


def reuse_path(path, wave):
    """Why `path` is outside the receipt-reuse allowlist of `wave`, or None."""
    parts = path.split("/")
    if any(c in ("", ".", "..") for c in parts):
        return "a '.', '..' or empty path component"
    if not wave or parts[:2] != ["plans", wave] or not post_gate(parts[2:]):
        return "outside plans/%s/{%s}" % (wave or "<no wave>", ",".join(POST_GATE))
    return None


def post_gate(rest):
    """Is `rest` (the components below `plans/<wave>/`) on `POST_GATE`?"""
    gp = _gate_plan()
    return bool(rest) and any(gp._matches("/".join(rest), g) for g in POST_GATE)


# Fund V-1 (welle-validierung, GH #1203): the plan of a NEW wave -- a wave
# directory the merge base does not hold -- comes on the branch
# `<prefix>/plan` with its README, ledger, plan-parts and PLAN-*.md, none of
# them under berichte/ or straenge/. With `reuse_path` as its rule it never
# merged plan-only (14.10. 00:27: `no_report/no_strand_receipt/pre_stamp`,
# landed by hand as 8fdfa581b). For that branch alone the plan-only check
# takes every file of plans/<wave>/ but the gate archive receipts/, under the
# same construction (`reuse_tree`, `reuse_readers`) and the reader rules
# (`offending`). A wave master holds keeps the allowlist (fix round 7).
PLAN_STRAND = "plan"


def plan_path(path, wave):
    """Why `path` is outside the plan of the new wave `wave`, or None."""
    parts = path.split("/")
    if any(c in ("", ".", "..") for c in parts):
        return "a '.', '..' or empty path component"
    if not wave or len(parts) < 3 or parts[:2] != ["plans", wave] or parts[2] == "receipts":
        return "outside plans/%s/ (without receipts/)" % (wave or "<no wave>")
    return None


def plan_allowlisted(path, wave=None):
    return plan_path(path, wave) is None


def reuse_tree(repo, rev, paths, label, present=False):
    """`{path: why}` of the `paths` that in commit `rev` are not a regular
    100644 file (or absent, unless `present`) or have a component above them
    that is not a directory -- a symlink or a submodule on the path. Raises
    ValueError when the tree is unreadable.

    `present` is the tip's half (Koordinator-3 ruling on "100644 on both
    sides"): a path new since the gate keeps the receipt, a path the tip
    lacks is a deletion and re-gates."""
    above = {}
    for p in paths:
        parts = p.split("/")
        for i in range(1, len(parts)):
            above.setdefault("/".join(parts[:i]), []).append(p)
    if not paths:
        return {}
    rc, raw = _git(repo, "--literal-pathspecs", "ls-tree", "-z", "--full-tree", rev, "--",
                   *sorted(set(paths) | set(above)))
    if rc != 0:
        raise ValueError("git ls-tree %s failed (rc %d)" % (rev[:9], rc))
    out, own, seen = {}, set(paths), set()
    for entry in raw.decode("utf-8", "surrogateescape").split("\0"):
        if not entry:
            continue
        meta, name = entry.split("\t", 1)
        mode = meta.split()[0]
        if name in own and mode != "100644":
            out.setdefault(name, "mode %s in %s %s" % (mode, label, rev[:9]))
        if name in above and mode != "040000":
            for p in above[name]:
                out.setdefault(p, "%s is mode %s in %s %s" % (name, mode, label, rev[:9]))
        seen.add(name)
    if present:
        for p in sorted(own - seen):
            out.setdefault(p, "absent in %s %s, a deletion" % (label, rev[:9]))
    return out


def reuse_readers(repo, paths, revs):
    """`{path: why}` of the `paths` a committed script or test of a commit in
    `revs` names (`python_readers` over `PY_READERS` + `REUSE_TESTS`, read off
    the commit; one `git grep` per commit, per path only on a hit and only
    over the hit files). Raises ValueError when `git grep` fails.

    V2-A10 (h, wave validation 2): the per-path grep read every script and
    test of the rev -- 614 greps, 984 s for the 310 re-check files of v2-fix-b
    (`/tmp/claude-v2fixc-h/probe2.log`). A file that names one path is a hit
    of the grep over all of them, so the hit files are the whole search."""
    out, specs = {}, PY_READERS + REUSE_TESTS
    for rev, label in revs:
        hits = python_readers(repo, paths, rev, specs) if paths else []
        if not hits:
            continue
        only = tuple(":(literal)" + h for h in hits)
        for p in paths:
            got = python_readers(repo, [p], rev, only)
            if got:
                out.setdefault(p, "read by %s in %s" % (", ".join(got[:3]), label))
    return out


def code_equal_revs(repo, a, b, wave=None, rule=None):
    """`(ok, offending)` -- does a receipt on `a` still cover `b`? Only when
    every path of `a..b` passes `reuse_path` and `reuse_tree` in the gated rev
    `a`, the tip `b` and master (`repo`'s HEAD), and `reuse_readers` of `b`
    and master names none of them (module doc, RECEIPT REUSE). Fail closed:
    a git error is a refusal. `rule` replaces `reuse_path` for the plan-only
    check of a new wave's plan (`plan_path`, Fund V-1); a receipt is never
    judged by it."""
    rule = rule or reuse_path
    if not a or not b:
        return False, {"?": "no rev"}
    try:
        paths = changed(repo, a, b)
        bad = {p: why for p in paths for why in [rule(p, wave)] if why}
        rest = [p for p in paths if p not in bad]
        for rev, label, present in ((a, "the gated rev", False), (b, "the tip", True),
                                    ("HEAD", "master", False)):
            for p, why in reuse_tree(repo, rev, rest, label, present).items():
                bad.setdefault(p, why)
        rest = [p for p in rest if p not in bad]
        for p, why in reuse_readers(repo, rest, ((b, "the tip"), ("HEAD", "master"))).items():
            bad.setdefault(p, why)
    except ValueError as exc:
        return False, {"?": str(exc)}
    return not bad, bad


def _file_digest(repo, rel):
    path = os.path.join(repo, rel)
    if os.path.islink(path):
        return "link:" + os.readlink(path)
    if os.path.isfile(path):
        with open(path, "rb") as fh:
            return hashlib.sha256(fh.read()).hexdigest()
    return "deleted"


def overlay_digests(repo, paths):
    """`{rel: digest}` of the overlay paths -- the digests `pre.tree_hash` hashes."""
    return {rel: _file_digest(repo, rel) for rel in paths}


def _rev_digest(repo, rev, rel):
    """The digest a file of commit `rev` has in the working tree form."""
    got = _lines(repo, "ls-tree", rev, "--", rel)
    if not got:
        return "deleted"
    mode, _, rest = got[0].partition(" ")
    sha = rest.split("\t", 1)[0].split()[-1]
    rc, data = _git(repo, "cat-file", "blob", sha)
    if rc != 0:
        return "unreadable"
    if mode == "120000":
        return "link:" + data.decode("utf-8", "surrogateescape")
    return hashlib.sha256(data).hexdigest()


def stamp_aliases(repo, head, differ, s_over, overlay):
    """`{path: why}` of the symlink cases the stamp's digests miss, for the
    changed paths `differ` of `code_equal_stamp` (tb-s7 open point, tb-s10
    w7). A symlink's digest is `link:<target>`, so neither a new target nor
    a change behind it moves it. Fail closed, resolved in the worktree the
    gate would see (`os.path.realpath`, chains and directory links included):

      * a changed path that is a symlink on either side re-gates;
      * a changed path with a symlink above it in the worktree re-gates;
      * a changed path some symlink of the worktree resolves to (or to a
        directory above it) re-gates -- the link is an alias;
      * a symlink outside `plans/` resolving out of the tree re-gates: the
        content behind it is in no digest (as `tree_at`; master carries four
        links under `plans/welle-loop-2026-10-03/` into the coding repo, no
        code reads them).

    The links checked are those of `head`'s tree plus the overlay of the
    stamp and of now, present as links in the worktree. Raises ValueError
    when `head`'s tree is unreadable."""
    root = os.path.realpath(repo)
    out, real = {}, {}
    for p in differ:
        sides = (s_over.get(p, ""), overlay.get(p, ""))
        if any(d.startswith("link:") for d in sides) or os.path.islink(os.path.join(root, p)):
            out[p] = "a symlink (its target is not stamped)"
            continue
        real[p] = os.path.join(root, p)
        if os.path.realpath(real[p]) != real[p]:
            out[p] = "a symlink above it in the worktree"
    rc, raw = _git(repo, "ls-tree", "-r", "-z", "--full-tree", head)
    if rc != 0:
        raise ValueError("git ls-tree %s failed (rc %d)" % (head[:9], rc))
    links = {e.split("\t", 1)[1] for e in raw.decode("utf-8", "surrogateescape").split("\0")
             if e.startswith("120000 ")}
    links |= set(s_over) | set(overlay)
    for link in sorted(links):
        path = os.path.join(root, link)
        if not os.path.islink(path):
            continue
        to = os.path.realpath(path)
        if to != root and not to.startswith(root + os.sep):
            if link.startswith("plans/"):
                continue
            out.setdefault(link, "a symlink out of the tree (the content behind it is not stamped)")
            continue
        for p, at in real.items():
            if at == to or at.startswith(to + os.sep):
                out.setdefault(p, "aliased by the symlink %s" % link)
    return out


def code_equal_stamp(repo, stamp, head, overlay, wave):
    """`(ok, offending)` -- is the tree `head` + `overlay` code-equal to the
    stamped tree, with only `wave`'s bookkeeping neutral? A stamp without
    `head`/`overlay` (older pre) never is; without a wave nothing is neutral."""
    s_head, s_over = stamp.get("head"), stamp.get("overlay")
    if not s_head or not isinstance(s_over, dict):
        return False, {"stamp": "no head/overlay recorded (pre before #1167)"}
    try:
        paths = set(changed(repo, s_head, head)) if s_head != head else set()
    except ValueError as exc:
        return False, {"?": str(exc)}
    paths |= set(s_over) | set(overlay)
    differ = []
    for p in sorted(paths):
        was = s_over[p] if p in s_over else _rev_digest(repo, s_head, p)
        now = overlay[p] if p in overlay else _rev_digest(repo, head, p)
        if was != now:
            differ.append(p)
    bad = offending(differ, repo, wave)
    if differ:
        try:
            got = stamp_aliases(repo, head, differ, s_over, overlay)
        except ValueError as exc:
            got = {"?": str(exc)}
        for p, why in got.items():
            bad.setdefault(p, why)
    if not wave:
        for p in differ:
            bad.setdefault(p, "no strand wave: only a strand's own wave is neutral")
    return not bad, bad


# --- the merge gate base (b) -----------------------------------------------------

def is_ancestor(repo, a, b):
    return _git(repo, "merge-base", "--is-ancestor", a, b)[0] == 0


def partial_receipt(doc, plan_doc):
    """Why the strand receipt `doc` (with its `plan.json` `plan_doc`) is NOT a
    full strand gate on a clean tree, or None. Fail closed: a receipt that
    does not SAY it was clean and complete is partial."""
    doc = doc or {}
    if doc.get("dirty") is not False:
        return "dirty tree" if doc.get("dirty") else "the receipt records no clean tree"
    if not isinstance(plan_doc, dict):
        return "no plan.json beside the receipt"
    for d in (doc, plan_doc):
        only = d.get("only") or d.get("+only")
        if only:
            return "--only %s" % ",".join(only if isinstance(only, list) else [str(only)])
    # Re-check H1 m-N2: no `cache` criterion -- neither receipt nor plan.json
    # carries one and the gate has no cache-off switch. The cache only turns
    # a station into CACHED (a green run on the same source closure, browser
    # duty in the key, OR-TB.S7.10), which the station check below counts.
    decisions = doc.get("owner_decisions")
    if decisions:
        return "owner decision(s): %s" % ", ".join(
            "%s %s" % (x.get("station"), x.get("decision")) if isinstance(x, dict) else str(x)
            for x in decisions)
    plan = doc.get("plan")
    if not isinstance(plan, list) or not plan:
        return "the receipt names no planned stations"
    ran = {}
    for st in doc.get("stations") or []:
        if isinstance(st, dict) and st.get("name"):
            ran[st["name"]] = ("CACHED" if st.get("verdict") == "FLAKY" and st.get("legacy") is True
                               else st.get("verdict"))
    for name in plan:
        if ran.get(name) not in RAN:
            return "planned station %s %s" % (name, ran.get(name) or "missing")
    return None


def merge_gate_base(repo, master, tip, receipt_doc):
    """`(base, why)`: the rev the merge gate plans from.

    The strand tip when the strand receipt covers it (`gated` code-equal to the
    tip, checked by the preconditions), is a full gate on a clean tree
    (`partial` None, set by the preconditions) and its base lies on master:
    the diff tip..candidate is then exactly master's move since the strand base
    plus the merge point's edits. Otherwise `master` (the full strand diff) --
    fail closed."""
    receipt_doc = receipt_doc or {}
    if "partial" not in receipt_doc or receipt_doc["partial"]:
        return master, "full: the strand receipt is partial (%s)" % (
            receipt_doc.get("partial") or "coverage unknown")
    base = receipt_doc.get("base")
    if not base:
        return master, "full: the strand receipt names no base"
    if not is_ancestor(repo, base, master):
        return master, "full: strand base %s is not on master" % base[:9]
    if not is_ancestor(repo, base, tip):
        return master, "full: strand base %s is not below the tip" % base[:9]
    moved = "master unmoved" if base == master else "master moved %s..%s" % (base[:9], master[:9])
    return tip, "reuse: strand receipt %s covers the tip, %s" % (
        (receipt_doc.get("rev") or "?")[:9], moved)


# --- pre checks (e) and (f) --------------------------------------------------------

def owner(repo):
    """`Name <email>` of the owner: `$MECLAW_OWNER`, else the identity git
    itself commits with in `repo` (`git config` with includes and a
    repo-local setting, re-check H2 m3)."""
    env = os.environ.get(OWNER_ENV)
    if env:
        return env.strip()
    name = _lines(repo, "config", "--includes", "user.name")
    mail = _lines(repo, "config", "--includes", "user.email")
    if not name or not mail:
        return None
    return "%s <%s>" % (name[0], mail[0])


def foreign_commits(repo, rng, who=None):
    """`(owner, [(sha, author, committer)])` of the commits in `rng` whose
    author OR committer is not the owner. Without an owner every commit is."""
    who = who or owner(repo)
    got = _lines(repo, "log", "--format=%H%x09%an <%ae>%x09%cn <%ce>", rng)
    if got is None:
        return who, [("?", "git log %s failed" % rng, "")]
    bad = []
    for line in got:
        sha, a, c = (line.split("\t") + ["", ""])[:3]
        if who is None or a != who or c != who:
            bad.append((sha, a, c))
    return who, bad


DEFAULT_BRANCHES = ("master", "main")


def default_branch(repo):
    """The repo's default branch, or None: local `master`, else local `main`, else
    the name `origin/HEAD` points at (its local branch, else `origin/<name>`), else
    `origin/master`, else `origin/main` (K13-1: orga-hive and soul2me have only
    `main`). A local branch comes first (fix round 5 v2-r3-kit, M7): meclaw-core's
    origin is the export mirror, and a set `origin/HEAD` would name its `main`."""
    def has(ref):
        return _lines(repo, "rev-parse", "--verify", "--quiet", ref) is not None

    for name in DEFAULT_BRANCHES:
        if has("refs/heads/" + name):
            return name
    head = _lines(repo, "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD")
    names = ([head[0].rsplit("/", 1)[-1]] if head else []) + list(DEFAULT_BRANCHES)
    for name in names:
        for ref, shown in (("refs/heads/" + name, name),
                           ("refs/remotes/origin/" + name, "origin/" + name)):
            if has(ref):
                return shown
    return None


def owner_check(repo, rng=None):
    """`(verdict, reason)` of check (f): author and committer of
    `<default branch>..HEAD` (`default_branch`), or of `rng`."""
    if rng is None:
        base = default_branch(repo)
        if base is None:
            return "RED", ("no default branch in %s (looked for master, main, origin/HEAD) -- "
                           "the owner check has no range" % repo)
        rng = "%s..HEAD" % base
    who, bad = foreign_commits(repo, rng)
    if who is None:
        return "RED", "no owner identity (set git config --global user.name/user.email or $%s)" % OWNER_ENV
    if bad:
        shown = "; ".join("%s author %s committer %s" % (s[:9], a, c) for s, a, c in bad[:5])
        more = " and %d more" % (len(bad) - 5) if len(bad) > 5 else ""
        return "RED", ("%d commit(s) in %s not by %s: %s%s -- rewrite them "
                       "(git rebase --exec 'git commit --amend --no-edit --reset-author')"
                       % (len(bad), rng, who, shown, more))
    return "GREEN", ""


def main_root(repo):
    got = _lines(repo, "rev-parse", "--path-format=absolute", "--git-common-dir")
    return os.path.dirname(got[0].rstrip("/")) if got else None


def main_tree_check(repo, paths):
    """`(verdict, reason)` of check (e): files of the main tree under the strand's
    paths that would stop the fast-forward (`main_tree_dirty`). An untracked file
    byte-equal to the strand's is fine -- the merge sets it aside."""
    root = main_root(repo)
    if not root:
        return "RED", "the main tree of %s is unknown (git rev-parse failed)" % repo
    if os.path.realpath(root) == os.path.realpath(repo):
        return "GREEN", ""
    # `-z`: names verbatim; quoted (`"templates/a/\\303\\244.txt"`) a strand
    # path never met its status key and the check stayed GREEN (I-h1).
    rc, raw = _git(root, "status", "--porcelain=v1", "-z", "--untracked-files=all",
                   "--no-renames")
    if rc != 0:
        return "RED", "git status in the main tree %s failed" % root
    dirty = {}
    for line in raw.decode("utf-8", "surrogateescape").split("\0"):
        if line:
            dirty[line[3:]] = line[:2]
    want = set(paths)
    bad, tracked = [], []
    for rel in sorted(want & set(dirty)):
        code = dirty[rel]
        if code == "??" and _file_digest(root, rel) == _file_digest(repo, rel):
            continue
        (bad if code == "??" else tracked).append("%s [%s]" % (rel, code.strip() or "?"))

    def listed(items):
        return ", ".join(items[:8]) + (" ..." if len(items) > 8 else "")
    if bad:
        return "RED", ("%d main-tree file(s) under the strand's paths would stop the merge "
                       "(main_tree_dirty): %s -- move them out of %s"
                       % (len(bad + tracked), listed(bad + tracked), root))
    if tracked:
        # Re-check H2 m5: a tracked change in the main tree is often the
        # orchestrator's own, gone by the merge hours later -- a finding
        # here; the queue's fast-forward still refuses it (main_tree_dirty).
        return "NOTE", ("%d tracked main-tree change(s) under the strand's paths: %s -- "
                        "the merge stops on them if they are still there"
                        % (len(tracked), listed(tracked)))
    return "GREEN", ""


# --- command line (strand.sh new, g) ----------------------------------------------

def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if len(argv) == 3 and argv[0] == "foreign":
        repo, rng = argv[1], argv[2]
        who, bad = foreign_commits(repo, rng)
        if who is None:
            print("no owner identity (git config --global user.name/user.email or $%s)"
                  % OWNER_ENV, file=sys.stderr)
            return 1
        for sha, a, c in bad:
            print("%s author %s committer %s" % (sha[:9], a, c))
        return 1 if bad else 0
    print("usage: turnaround.py foreign <repo> <range>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
