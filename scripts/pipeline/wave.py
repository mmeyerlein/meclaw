"""`pipeline wave close <wave>` -- the end of a wave as one checked step.

PREAMBLE section 1 says waves clear their own issues; this command is the lock
behind that sentence. In order:

1. **Issues.** The wave's issues are the ``issues: [...]`` lists in the header
   blocks of ``plans/<wave>/berichte/*.md`` plus every ``--issue <n>``. Each one
   is read with ``gh issue view <n> --json state,labels,title`` (read-only; the
   command never writes to GitHub). An issue passes when it is CLOSED, or when it
   is open and carries a mark:

   * a label named (case-insensitive) one of ``MARK_LABELS``
     (``ruling``, ``needs-ruling``, ``investigation``), or
   * a mark line in any ``*.md`` of the wave folder or its ``berichte/``::

         OPEN #<n> ruling: <why>
         OPEN #<n> investigation: <why>

     (a leading ``-``/``*`` is allowed).

   Anything else -- also an issue ``gh`` cannot read -- refuses the close.
   (An ``OPEN #<n> owner-ruling: <reference>`` line marks an issue too.)

   **Balance** (GH #1196). The issues CREATED in the wave window -- from the
   wave's start: the commit time of the first ``wave start`` rev in the store
   (not the moment ``wave start`` ran) or the commit of its ``wave open`` row in
   ``plans/README.md`` (author or committer time, the earlier), the earlier
   one -- and named
   (``#<n>``) in ``berichte/*.md`` or ``ledger.md``, minus the closed ones.
   One ``gh issue list --search created:>=<start>`` reads them. A balance
   above zero refuses the close with the list; the only excuse is an owner
   ruling, the mark line ``OPEN #<n> owner-ruling: <reference>`` -- the
   ``ruling``/``investigation`` marks and labels above do not excuse an issue
   the wave opened itself. No known start, or GitHub not readable: refused
   (fail-closed). The line:
   ``BALANCE opened=<n> closed=<n> excused=<n> balance=<n> since=<start> (<source>)``.
2. **Orga inputs.** Every strand report (a ``berichte/*.md`` with a header block
   naming ``strang:``/``strand:``) names at least one ``E-DT-<n>`` that exists as
   a line in ``plans/<wave>/orga-eingaben.md``, or says there is none
   (``E-DT: none``; the German reports write ``keine``). Missing -> refused.
   **Coverage on the end tip** (welle-tb S1 C-Beleg, KR-TB-1 a). The map build
   (``pipeline coverage build --lane <lane>``) on the end tip -- ``master`` of the
   tree -- is the full suite of the wave. Its map ``coverage-<tip>.json`` in the
   coverage directory (``narrow.coverage_dir()``) must be GREEN
   (``coverage.verdict_of``); RED (with the red tests by name), INCOMPLETE, a
   map of an older rev only (MISSING) or an unknown tip refuses the close before
   anything is written. The line: ``COVERAGE end-tip=<sha> verdict=<v> ...``.
3. **Retro.** ``scripts/wave_retro.py <wave>`` writes ``retro.md`` and the line
   in ``plans/retro/RETRO.md``; the Q14..Q17 numbers of ``retro.quarter`` (part A)
   are printed when ``scripts/pipeline/retro.py`` exists.
4. **Safety bump.** Prepared as a line only. With ``--bump`` it runs
   ``scripts/pipeline/versions.py --workspace fix`` (W9) -- only when that file
   exists and only after the confirmation question is answered yes. It never
   tags; the coordinator tags.
5. **Export.** ``export-orga <wave>`` into ``plans/<wave>/orga-export/``.

``pipeline wave start <wave> --rev <sha>`` records where a wave starts: the
event ``wave_started`` (``{wave, start}``) in the store's journal. The rev is
required (re-check M-N1: a late first start without one would take the tip
after merges of the wave as its base); it must be a commit and an ancestor of
``master`` (no free SHA). A narrowed integration (``MECLAW_GATE_WAVE=<wave>``)
plans its tests from ``<start>..HEAD`` (welle-tb S1 part C, KR-TB-1); a measuring
wave with start T is ``wave start <name> --rev T``.

The last line of ``close`` is ``WAVE <wave> issues=<ok>/<n> retro=<v> bump=<v> export=<v>``
where a value is ``ok``, ``dry`` (``--dry-run``), ``skipped`` (refused before),
``failed`` or, for the bump, ``prepared`` / the new version. Exit 0 only when
nothing was refused or failed. ``--dry-run`` writes nothing but stdout: it
reads GitHub and the store, skips the retro script and builds the export in
memory.
"""

import json
import os
import pathlib
import re
import subprocess
import sys

from . import orga_export, store

MARK_LABELS = ("ruling", "needs-ruling", "investigation")
MARK_LINE = re.compile(r"^\s*(?:[-*]\s*)?OPEN\s+#(\d+)\s+(owner-ruling|ruling|investigation)\b",
                       re.MULTILINE)
EDT_ID = re.compile(r"\bE-DT-(\d+)\b")
EDT_DEFINED = re.compile(r"\*\*E-DT-(\d+)\b")
# "none" in the reports' own language is data, not prose: the German reports say "keine".
EDT_NONE = re.compile(r"E-DT\s*[:=-]?\s*(?:none|keine)\b|\b(?:none|keine)\s+E-DT\b", re.IGNORECASE)
HEADER_KEYS = ("strang", "strand")
GH_FIELDS = "state,labels,title"
BUMP_ARGV = ("scripts/pipeline/versions.py", "--workspace", "fix")


def _retro():
    """Part A's module, or None while `scripts/pipeline/retro.py` is not there."""
    try:
        from . import retro  # noqa: PLC0415
    except ImportError:
        return None
    return retro


def _run(argv, **kw):
    kw.setdefault("capture_output", True)
    kw.setdefault("text", True)
    return subprocess.run(argv, check=False, **kw)


def header(text):
    """The `key: value` lines of a leading `---` block, {} without one."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    out = {}
    for line in lines[1:]:
        if line.strip() == "---":
            return out
        if ":" in line:
            k, v = line.split(":", 1)
            out[k.strip()] = v.strip()
    return {}


def reports(wave_dir):
    """[(path, strand, header, text)] of the strand reports of a wave."""
    out = []
    for path in sorted((wave_dir / "berichte").glob("*.md")):
        text = path.read_text(encoding="utf-8", errors="replace")
        head = header(text)
        strand = next((head[k] for k in HEADER_KEYS if head.get(k)), None)
        if strand:
            out.append((path, strand, head, text))
    return out


def wave_issues(wave_dir, extra=()):
    found = set(int(n) for n in extra)
    for _path, _strand, head, _text in reports(wave_dir):
        found |= {int(n) for n in re.findall(r"#?(\d+)", head.get("issues", ""))}
    return sorted(found)


def marks(wave_dir):
    """{issue: kind} of the mark lines in the wave folder and its reports."""
    out = {}
    files = list(wave_dir.glob("*.md")) + list((wave_dir / "berichte").glob("*.md"))
    for path in sorted(files):
        for m in MARK_LINE.finditer(path.read_text(encoding="utf-8", errors="replace")):
            out[int(m.group(1))] = m.group(2)
    return out


def check_issues(wave_dir, issues, runner, out):
    """(ok_count, all_ok): one ISSUE line per issue."""
    mark = marks(wave_dir)
    ok = 0
    for n in issues:
        res = runner(["gh", "issue", "view", str(n), "--json", GH_FIELDS],
                     capture_output=True, text=True)
        if res.returncode != 0:
            out(f"ISSUE #{n} UNREADABLE gh exit {res.returncode}")
            continue
        try:
            body = json.loads(res.stdout)
        except ValueError:
            out(f"ISSUE #{n} UNREADABLE gh answer is not JSON")
            continue
        state = str(body.get("state", "")).upper()
        title = body.get("title", "")
        labels = {str(lb.get("name", "")).lower() for lb in body.get("labels") or []
                  if isinstance(lb, dict)}
        hit = sorted(labels & set(MARK_LABELS))
        if state == "CLOSED":
            ok += 1
            out(f"ISSUE #{n} CLOSED")
        elif hit or n in mark:
            ok += 1
            why = f"label {hit[0]}" if hit else f"mark line {mark[n]}"
            out(f"ISSUE #{n} OPEN marked ({why}): {title}")
        else:
            out(f"ISSUE #{n} OPEN without a ruling/investigation mark: {title}")
    return ok, ok == len(issues)


def check_edt(wave_dir, out):
    """(ok_count, total): every strand report names a known E-DT line or none."""
    path = wave_dir / "orga-eingaben.md"
    defined = set(EDT_DEFINED.findall(path.read_text(encoding="utf-8"))) if path.is_file() else set()
    ok = total = 0
    for _p, strand, _head, text in reports(wave_dir):
        total += 1
        named = set(EDT_ID.findall(text))
        if named & defined or EDT_NONE.search(text):
            ok += 1
            continue
        why = (f"names E-DT-{', E-DT-'.join(sorted(named, key=int))} but orga-eingaben.md has "
               "no such line" if named else "names no E-DT line and does not say none")
        out(f"EDT {strand} {why}")
    out(f"EDT reports={ok}/{total}")
    return ok, total


# --- the issue balance (GH #1196) --------------------------------------------------
#
# Findings were moved into fresh issues (often marked `investigation`) to get
# strands green; every such issue has to be understood again later without
# context. The balance counts what the wave opened and left open.

# The excuse: an owner ruling with a reference -- a placeholder (`<...>`) is none.
OWNER_RULING = re.compile(r"^\s*(?:[-*]\s*)?OPEN\s+#(\d+)\s+owner-ruling:[ \t]*"
                          r"([^<>\n]*[^<>\s])[ \t]*$", re.MULTILINE)
MENTION = re.compile(r"#(\d+)\b")
BALANCE_LIMIT = 1000
LIVE_RUNNING_ROW = "| `%s/` | **Running since"


def _utc(stamp):
    """RFC 3339 (`Z` or an offset) -> `YYYY-MM-DDTHH:MM:SSZ`."""
    import datetime  # noqa: PLC0415
    return store.parse_at(stamp).astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def owner_rulings(wave_dir):
    """{issue: reference} of the owner-ruling mark lines (wave folder and reports)."""
    out = {}
    files = list(wave_dir.glob("*.md")) + list((wave_dir / "berichte").glob("*.md"))
    for path in sorted(files):
        for m in OWNER_RULING.finditer(path.read_text(encoding="utf-8", errors="replace")):
            out[int(m.group(1))] = m.group(2).strip()
    return out


def named_issues(wave_dir):
    """Every `#<n>` in the wave's reports and its ledger."""
    files = sorted((wave_dir / "berichte").glob("*.md"))
    if (wave_dir / "ledger.md").is_file():
        files.append(wave_dir / "ledger.md")
    found = set()
    for path in files:
        found |= {int(n) for n in MENTION.findall(path.read_text(encoding="utf-8",
                                                                 errors="replace"))}
    return found


def wave_window_start(wave, root, db_path, runner=None):
    """`(start, source)` of the wave window, or `(None, why)`: the commit time
    of the first `wave_started` rev of the store, the commit of the `wave open`
    row -- the earlier one when both are there. Git is read through `runner`."""
    from . import narrow  # noqa: PLC0415
    runner = runner or _run
    found = []
    try:
        db = store.open_ro(db_path)
    except FileNotFoundError:
        db = None
    rev = None
    if db is not None:
        try:
            for row in db.execute("SELECT data FROM journal WHERE event='wave_started' "
                                  "ORDER BY seq ASC"):
                data = json.loads(row[0] or "{}")
                if narrow._same_wave(data.get("wave"), wave):
                    rev = str(data.get("start") or "")
                    break
        finally:
            store.close(db)
    if rev is not None:
        # Re-check v2-p1196 I1: the journal time is when `wave start` RAN; it is
        # recorded late on purpose (`--rev`, re-check M-N1), so that time put the
        # window at "now" and the issues opened since the start rev fell out
        # (fail-open). The commit time of the start rev is when the wave began; an
        # unreadable rev refuses -- the journal time is no fallback.
        res = runner(["git", "-C", str(root), "show", "-s", "--format=%cI", rev + "^{commit}"],
                     capture_output=True, text=True, env=_git_clean_env())
        stamp = res.stdout.strip() if res.returncode == 0 else ""
        try:
            found.append((_utc(stamp), "wave start"))
        except ValueError:
            return None, (f"the start rev {rev[:9]} of `wave start` is not readable in "
                          f"{root} (no commit time)")
    # `wave open` has no rev: the commit that wrote the row is the act itself.
    # A rebase of that branch before its merge moves only the committer time
    # (later); the author time stays the moment of the act -- the earlier of the
    # two opens the window (a larger window refuses more, never less).
    res = runner(["git", "-C", str(root), "log", "master", "--reverse", "--format=%aI %cI",
                  "-S", LIVE_RUNNING_ROW % wave, "--", "plans/README.md"],
                 capture_output=True, text=True, env=_git_clean_env())
    first = res.stdout.splitlines()[:1] if res.returncode == 0 else []
    if first and first[0].split():
        found.append((min(_utc(s) for s in first[0].split()), "wave open"))
    if not found:
        return None, ("no wave start: neither `wave start` in the store nor a `wave open` "
                      "row of plans/README.md on master")
    return min(found)


UNKNOWN = "the issue balance of the wave is unknown (fail-closed)"


def check_balance(wave, root, wave_dir, runner, out, db_path=None):
    """None when the wave left no issue open that it opened itself (owner
    rulings excused), else the reason of the refusal. Prints the `BALANCE`
    lines; fail-closed."""
    since, source = wave_window_start(wave, root, db_path, runner)
    if since is None:
        # strand.sh routes only `wave open|close`; `wave start` is pipeline.py's (re-check M1)
        out(f"BALANCE unknown: {source} -- run "
            f"`scripts/pipeline.py wave start {wave} --rev <sha>`")
        return UNKNOWN
    res = runner(["gh", "issue", "list", "--state", "all", "--search", f"created:>={since}",
                  "--json", "number,state,createdAt,title", "--limit", str(BALANCE_LIMIT)],
                 capture_output=True, text=True, cwd=str(root))
    rows = None
    if res.returncode == 0:
        try:
            rows = json.loads(res.stdout)
        except ValueError:
            rows = None
    if res.returncode != 0 or not isinstance(rows, list):
        why = f"exit {res.returncode}" if res.returncode else "answer is not a JSON list"
        out(f"BALANCE unknown: gh issue list {why}")
        return UNKNOWN
    if len(rows) >= BALANCE_LIMIT:
        out(f"BALANCE unknown: gh issue list returned {len(rows)} issues, the limit -- "
            "the list may be cut")
        return UNKNOWN
    named = named_issues(wave_dir)
    excuse = owner_rulings(wave_dir)
    try:
        opened = sorted((r for r in rows if isinstance(r, dict) and r.get("number") in named
                         and _utc(str(r.get("createdAt"))) >= since), key=lambda r: r["number"])
    except (TypeError, ValueError) as exc:
        out(f"BALANCE unknown: gh issue list answer has no readable createdAt ({exc})")
        return UNKNOWN
    left = [r for r in opened if str(r.get("state", "")).upper() != "CLOSED"]
    excused = [r for r in left if r["number"] in excuse]
    balance = len(left) - len(excused)
    for r in left:
        n = r["number"]
        if n in excuse:
            out(f"BALANCE #{n} OPEN excused (owner-ruling {excuse[n]}): {r.get('title', '')}")
        else:
            out(f"BALANCE #{n} OPEN opened by the wave, no owner ruling: {r.get('title', '')}")
    out(f"BALANCE opened={len(opened)} closed={len(opened) - len(left)} excused={len(excused)} "
        f"balance={balance} since={since} ({source})")
    return None if balance == 0 else "the wave opened more issues than it closed"


def _window(wave, db, now):
    mod = _retro()
    if mod is not None and hasattr(mod, "wave_window"):
        try:
            since, until = mod.wave_window(wave, db)
            return orga_export._stamp(since), orga_export._stamp(until)
        except Exception:  # noqa: BLE001 -- part A's window is optional here
            pass
    m = re.search(r"(\d{4}-\d{2}-\d{2})$", wave)
    if not m:
        raise ValueError(f"no date in the wave name {wave!r}")
    return f"{m.group(1)}T00:00:00Z", now or store.now()


def _quarter(db, since, until, out):
    mod = _retro()
    if mod is None:
        out("RETRO quarter n/a: scripts/pipeline/retro.py is not there")
        return
    try:
        q = mod.quarter(db, since, until)
        out("RETRO " + " ".join(f"{k}={q.get(k)}" for k in ("Q14", "Q15", "Q16", "Q17")))
        out(f"RETRO table rows={len(mod.table(db, since, until))}")
    except Exception as exc:  # noqa: BLE001 -- a broken metric is a finding, not a stop
        out(f"RETRO quarter n/a: {type(exc).__name__}: {exc}")


def _bump(root, bump, dry_run, runner, confirm, out):
    argv = ["python3", *BUMP_ARGV]
    out("BUMP prepared: " + " ".join(argv) + " -- the coordinator confirms and tags")
    versions = root / BUMP_ARGV[0]
    if not bump or dry_run:
        return "prepared"
    if not versions.is_file():
        out(f"BUMP not run: {BUMP_ARGV[0]} is not there (W9)")
        return "prepared"
    if not confirm("Run the safety bump now (no tag)? [y/N] "):
        out("BUMP not run: not confirmed")
        return "prepared"
    res = runner(["python3", str(versions), *BUMP_ARGV[1:]], capture_output=True, text=True,
                 cwd=str(root))
    if res.returncode != 0:
        out(f"BUMP failed: exit {res.returncode}")
        return "failed"
    words = res.stdout.split()
    return words[-1] if words else "done"


def _git_clean_env():
    env = dict(os.environ)
    for k in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"):
        env.pop(k, None)
    return env


def check_end_tip_coverage(root, out, map_dir=None):
    """`(ok, verdict)` of the coverage map on the end tip (`master` of `root`).
    Fail-closed: only a GREEN map of exactly that rev passes."""
    from . import coverage, narrow  # noqa: PLC0415
    res = subprocess.run(["git", "-C", str(root), "rev-parse", "--verify", "--quiet",
                          "master^{commit}"], capture_output=True, text=True,
                         env=_git_clean_env())
    tip = res.stdout.strip()
    if res.returncode != 0 or not tip:
        out("COVERAGE end-tip=unknown verdict=MISSING -- no master in %s" % root)
        return False, "MISSING"
    path = pathlib.Path(map_dir or narrow.coverage_dir()) / ("coverage-%s.json" % tip)
    cov, why = coverage.load_map(path)
    if cov is None:
        out("COVERAGE end-tip=%s verdict=MISSING -- %s; build it: pipeline coverage build"
            " --lane <lane> on the end tip" % (tip[:9], why))
        return False, "MISSING"
    verdict = coverage.verdict_of(cov) if cov.get("rev") == tip else "MISSING"
    detail = "; ".join(cov.get("incomplete") or []) or "complete"
    out("COVERAGE end-tip=%s verdict=%s map=%s -- %s" % (tip[:9], verdict, path, detail))
    reds = list(cov.get("red_tests") or [])
    for name in reds[:50]:
        out("COVERAGE red %s" % name)
    return verdict == "GREEN", verdict


def close(wave, root=None, db_path=None, issues=(), dry_run=False, bump=False,
          runner=None, confirm=None, out=None, now=None):
    """Close a wave; returns the exit code. See the module docstring."""
    root = pathlib.Path(root) if root else orga_export.repo_root()
    runner = runner or _run
    out = out or print
    confirm = confirm or (lambda q: input(q).strip().lower() in ("y", "yes"))
    wave_dir = root / "plans" / wave
    if not wave_dir.is_dir():
        out(f"wave close: no wave folder {wave_dir}")
        out(f"WAVE {wave} issues=0/0 retro=skipped bump=skipped export=skipped")
        return 2

    nums = wave_issues(wave_dir, issues)
    ok, issues_ok = check_issues(wave_dir, nums, runner, out)
    edt_ok, edt_total = check_edt(wave_dir, out)
    balance_refused = check_balance(wave, root, wave_dir, runner, out, db_path)
    head = f"WAVE {wave} issues={ok}/{len(nums)}"
    if not issues_ok or edt_ok != edt_total or balance_refused:
        out("wave close: refused -- " + ("open issues without a mark" if not issues_ok
                                         else "strand reports without Orga inputs"
                                         if edt_ok != edt_total else balance_refused))
        out(f"{head} retro=skipped bump=skipped export=skipped")
        return 1
    cov_ok, verdict = check_end_tip_coverage(root, out)
    if not cov_ok:
        out(f"wave close: refused -- the coverage build on the end tip is {verdict}"
            " (the full suite of the wave; red is a finding, not a closed wave)")
        out(f"{head} retro=skipped bump=skipped export=skipped")
        return 1

    db = None
    try:
        db = store.open_ro(db_path)
    except FileNotFoundError as exc:
        out(f"wave close: {exc}")
    since, until = _window(wave, db, now) if db is not None else (None, None)

    if dry_run:
        retro = "dry"
    else:
        res = runner(["python3", str(root / "scripts" / "wave_retro.py"), wave],
                     capture_output=True, text=True, cwd=str(root))
        retro = "ok" if res.returncode == 0 else "failed"
        if res.returncode != 0:
            out(f"RETRO wave_retro.py exit {res.returncode}: {(res.stderr or '').strip()[-200:]}")
    if db is not None:
        _quarter(db, since, until, out)

    bump_v = _bump(root, bump, dry_run, runner, confirm, out)

    export = "failed"
    if db is not None:
        try:
            if dry_run:
                graph, runs = orga_export.build(db, wave=wave, since=since, until=until,
                                                lanes=orga_export.lanes_from_file())
                out(f"EXPORT dry items={len(graph['items'])} edges={len(graph['edges'])} "
                    f"task_runs={len(runs['task_runs'])}")
                export = "dry"
            else:
                g, r = orga_export.export(db, wave=wave, out_dir=wave_dir / "orga-export",
                                          since=since, until=until,
                                          lanes=orga_export.lanes_from_file())
                out(f"EXPORT {g} {r}")
                export = "ok"
        except Exception as exc:  # noqa: BLE001 -- named in the line, never hidden
            out(f"EXPORT failed: {type(exc).__name__}: {exc}")
        finally:
            store.close(db)
    rc = 0 if retro in ("ok", "dry") and export in ("ok", "dry") and bump_v != "failed" else 1
    if rc == 0:
        _live_close_step(root, wave, (now or store.now())[:10], dry_run, out)
    out(f"{head} retro={retro} bump={bump_v} export={export}")
    return rc


# --- plans/README.md section Live (welle-validierung-2 3b, GH #1199) --------------
#
# Section Live was an archive in order: the top row was the wave built LAST and
# the running wave got its row only at its close, so `strand.sh` in the main
# tree (`wave_dir`) found no running wave and every kit call needed `--wave`.
# `wave open` writes the row `**Running since <date>:** <title>` on top and
# commits it; `wave close` turns it into `**Built <date>:**` (the orchestrator
# describes the result in it afterwards, as before) and writes a new Built row
# for a wave opened before `wave open` existed.

LIVE_ROW = re.compile(r"^\| *`(?P<dir>[^`]+)/` *\| *(?P<rest>.*)$")
RUNNING = re.compile(r"^\*\*Running since (\d{4}-\d{2}-\d{2}):\*\*\s*")
NEW_BUILT_TEXT = "Description pending."


def _live_table(lines):
    """`(first, end)`: the index after the header rule of the section Live
    table and the index of the first line after the table; ValueError if the
    file has no such table."""
    try:
        sec = next(i for i, ln in enumerate(lines) if ln.strip() == "## Live")
    except StopIteration:
        raise ValueError("plans/README.md has no section Live") from None
    for i in range(sec + 1, len(lines)):
        if lines[i].startswith("## "):
            break
        if re.match(r"^\|\s*-{3,}\s*\|", lines[i]):
            end = i + 1
            while end < len(lines) and lines[end].startswith("|"):
                end += 1
            return i + 1, end
    raise ValueError("section Live of plans/README.md has no table")


def _live_rows(lines):
    first, end = _live_table(lines)
    for i in range(first, end):
        m = LIVE_ROW.match(lines[i])
        if m:
            yield i, m.group("dir"), m.group("rest")


def live_running(text):
    """Every wave directory section Live names as `**Running since <date>:**` --
    the one reader of the row; `strand.sh wave_dir` asks it through
    `pipeline wave running` (re-check v2-kit c23 M-5)."""
    return [d for _i, d, rest in _live_rows(text.split("\n")) if RUNNING.match(rest)]


def live_open(text, wave, title, date):
    """`text` with the row `| `<wave>/` | **Running since <date>:** <title> |` on
    top of section Live. ValueError for a wave the table names already or a
    title that would break the table row."""
    title = (title or "").strip()
    if not title or "|" in title or "\n" in title:
        raise ValueError("the title is one non-empty line without `|`: %r" % title)
    lines = text.split("\n")
    for _i, d, rest in _live_rows(lines):
        if d == wave:
            raise ValueError("section Live names %s/ already: %s" % (wave, rest[:60]))
    first, _end = _live_table(lines)
    lines.insert(first, "| `%s/` | **Running since %s:** %s |" % (wave, date, title))
    return "\n".join(lines)


def live_close(text, wave, date):
    """`(text, what)` with the row of `wave` built on `date`: `built` (its
    Running row turned Built), `new` (no row yet: a Built row on top), `kept`
    (a row that is not Running stays as it is)."""
    lines = text.split("\n")
    for i, d, rest in _live_rows(lines):
        if d != wave:
            continue
        if not RUNNING.match(rest):
            return text, "kept"
        lines[i] = "| `%s/` | %s" % (wave, RUNNING.sub("**Built %s:** " % date, rest, count=1))
        return "\n".join(lines), "built"
    first, _end = _live_table(lines)
    lines.insert(first, "| `%s/` | **Built %s:** %s |" % (wave, date, NEW_BUILT_TEXT))
    return "\n".join(lines), "new"


def _live_close_step(root, wave, date, dry_run, out):
    """The Live line of `close`; never decides the exit code."""
    readme = root / "plans" / "README.md"
    try:
        text = readme.read_text(encoding="utf-8")
        new, what = live_close(text, wave, date)
    except (OSError, ValueError) as exc:
        out(f"LIVE skipped: {exc}")
        return
    if what == "kept":
        out(f"LIVE plans/README.md row of {wave}/ is not Running -- kept")
        return
    if dry_run:
        out(f"LIVE dry: plans/README.md row of {wave}/ would be Built {date} ({what})")
        return
    readme.write_text(new, encoding="utf-8")
    out(f"LIVE plans/README.md row of {wave}/ is Built {date} ({what}) -- describe the "
        f"result in that row and commit it")


def open_wave(wave, title, root=None, out=None, today=None):
    """`pipeline wave open <wave> --title <sentence>`: the Running row on top of
    section Live, committed on the current branch. 0, or 2 when refused."""
    import datetime  # noqa: PLC0415
    root = pathlib.Path(root) if root else orga_export.repo_root()
    out = out or print
    date = today or datetime.date.today().isoformat()
    readme = root / "plans" / "README.md"
    if not (root / "plans" / wave).is_dir():
        out(f"wave open: no wave folder plans/{wave} -- refused")
        return 2
    rel = "plans/README.md"
    dirty = subprocess.run(["git", "-C", str(root), "status", "--porcelain", "--", rel],
                           capture_output=True, text=True)
    if dirty.returncode != 0 or dirty.stdout.strip():
        out(f"wave open: {rel} has changes of its own (or no git) -- commit them first; refused")
        return 2
    try:
        text = live_open(readme.read_text(encoding="utf-8"), wave, title, date)
    except (OSError, ValueError) as exc:
        out(f"wave open: {exc} -- refused")
        return 2
    readme.write_text(text, encoding="utf-8")
    # The commit form of a wave-level commit: the wave first, as the closing
    # commits (`welle-gate: Receipt, Retro, Takt`) and the hand-made Live rows
    # (`welle-live: Zeile in plans/README.md`) always were. The strand form
    # `<marker> <S> (#<issue>)` cannot apply: `wave open` runs before any strand
    # or issue of the wave exists (re-check v2-kit c23 M-6).
    msg = "%s: plans/README.md § Live, läuft seit %s" % (wave, date)
    res = subprocess.run(["git", "-C", str(root), "commit", "-q", "-m", msg, "--", rel],
                         capture_output=True, text=True)
    if res.returncode != 0:
        out(f"wave open: git commit failed: {(res.stderr or res.stdout).strip()[-300:]}")
        return 2
    out(f"WAVE-OPEN {wave} running since {date}")
    return 0


def start(wave, root=None, db=None, db_path=None, rev=None, out=None):
    """Record the start rev of `wave` (event `wave_started`); 0, or 2 when refused.

    The FIRST start of a wave stands (KR-TB-1 b, re-check H2 M2): another
    start of the same wave (with or without its date) is refused, the same
    start again is a no-op."""
    from . import events  # noqa: PLC0415
    from . import narrow  # noqa: PLC0415
    root = str(root or orga_export.repo_root())
    out = out or print

    def git(*args):
        return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True)
    if not str(rev or "").strip():
        # re-check M-N1 (OR-TB.S1.22): no default -- the store cannot tell
        # whether strands of the wave merged already, so the rev is named
        out(f"wave start: {wave} needs --rev <sha> (master at the wave start, the ledger "
            f"names it) -- refused")
        return 2
    want = str(rev).strip()
    res = git("rev-parse", "--verify", "--quiet", "%s^{commit}" % want)
    sha = res.stdout.strip()
    if res.returncode != 0 or not sha:
        out(f"wave start: {want} is no commit -- refused")
        return 2
    if git("merge-base", "--is-ancestor", sha, "master").returncode != 0:
        out(f"wave start: {sha[:9]} is no ancestor of master -- refused")
        return 2
    own = db is None
    db = db if db is not None else store.open(db_path)
    try:
        had = narrow.wave_starts(db, wave)
        if had and had != [sha]:
            out(f"wave start: {wave} started at {had[0][:9]} already "
                f"({', '.join(h[:9] for h in had)}) -- {sha[:9]} refused, the first start stands")
            return 2
        if not had:
            events.emit(db, "wave_started", "status", {"wave": wave, "start": sha})
    finally:
        if own:
            store.close(db)
    out(f"WAVE-START {wave} {sha}")
    return 0


def _cmd_start(args):
    return start(args.wave, db_path=args.db, rev=args.rev)


def _cmd_open(args):
    return open_wave(args.wave, args.title, root=args.root)


def _cmd_running(args):
    """`pipeline wave running <readme>`: one Running wave directory per line;
    2 when the file or its section Live cannot be read."""
    try:
        names = live_running(pathlib.Path(args.readme).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        print(f"wave running: {exc}", file=sys.stderr)
        return 2
    for name in names:
        print(name)
    return 0


def _cmd_wave(args):
    return close(args.wave, db_path=args.db, issues=args.issue or (), dry_run=args.dry_run,
                 bump=args.bump)


def register(sub):
    """`pipeline wave close <wave>` (plan part W11 item 5), `wave start <wave>` (#1159),
    `wave open <wave>` (welle-validierung-2 3b, #1199)."""
    p = sub.add_parser("wave", help="wave steps: open, start, close")
    wsub = p.add_subparsers(dest="wave_cmd", required=True)
    s = wsub.add_parser("start", help="record the master rev a wave starts from")
    s.add_argument("wave", help="wave name (MECLAW_GATE_WAVE of its integration pass)")
    s.add_argument("--rev", default=None,
                   help="start rev, required (master at the wave start; an ancestor of master)")
    s.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    s.set_defaults(func=_cmd_start)
    o = wsub.add_parser("open", help="the Running row of a wave on top of plans/README.md Live")
    o.add_argument("wave", help="wave directory name under plans/")
    o.add_argument("--title", required=True, help="one English sentence for the row")
    o.add_argument("--root", default=None, help="tree to write and commit in (default: the kit's)")
    o.set_defaults(func=_cmd_open)
    r = wsub.add_parser("running", help="the Running waves plans/README.md section Live names")
    r.add_argument("readme", help="the plans/README.md to read")
    r.set_defaults(func=_cmd_running)
    c = wsub.add_parser("close", help="issues lock, retro, bump line, Orga inputs, export")
    c.add_argument("wave", help="wave directory name under plans/")
    c.add_argument("--issue", type=int, action="append", help="an issue beyond the reports")
    c.add_argument("--dry-run", action="store_true", help="write nothing but stdout")
    c.add_argument("--bump", action="store_true",
                   help="run the prepared bump (only with versions.py, after confirmation)")
    c.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    c.set_defaults(func=_cmd_wave)


def main(argv=None):
    import argparse  # noqa: PLC0415
    ap = argparse.ArgumentParser(prog="wave")
    register(ap.add_subparsers(dest="cmd", required=True))
    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(["wave", *sys.argv[1:]]))
