"""The browser locks of an integration (welle-tb S1b, #1165, owner's ruling 10.10., point 2).

WHY THIS EXISTS
===============
`gate_plan.plan()` gives every integration the colony half and every ignored
browser lock, whatever the wave touched. The ruling: no double or needless
work, and a full run whenever something looks odd. Since point 1 the coverage
map proves which files each lock executes (`browser_cover.marker`, field
`browser_locks`), so an integration that narrows its tests (`plan.build_plan`
with a wave decision term) keeps a lock -- the sheet and the colony half among
them -- when ANY of this holds, fail-closed:

  (a) the wave diff touches the lock file or a file of its parts in the map
      (the files since the wave start plus those since the map's rev, as
      `narrow.narrow` reads them: want = files | since), or the lock's test
      binary changed since the map's rev (`map_fits` terms);
  (b) the lock is not measured (no field, no entry, `measured` not true);
  (c) the map does not fit (none, not complete, too old, from the future,
      `narrow.map_fits` false) -- the same checks as `narrow.narrow`;
  (d) the diff touches an input of the station the map cannot see:
      `cache.INPUTS["browser:display"]` plus `_EXTRA` -> every lock;
  (e) a crate file of the diff is in no part of the map -- a Rust file the
      map never ran, or a file it cannot see at all (`schemas/*.json` that
      `include_str!` compiles in, re-check S1b B1) -- or a workspace file
      changed -> every lock;
  (f) the lock spawns a workspace binary (`CARGO_BIN_EXE_` in its sources):
      the child's code is not its part's, so any Rust change keeps it.

Everything else leaves, and the NOTE says why for each lock. `release` never
comes here (every lock runs), a strand plans as before, an integration in
doubt (no decision term) runs every lock. The station and its triggers are
written down in `gate_plan.py`; this module only cuts its integration row.
"""

import fnmatch
import os
import sys
import types

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import gate_plan  # noqa: E402

from . import browser_cover  # noqa: E402

ROW = "browser:display"
# Station inputs the locks read that `cache.INPUTS` does not name: the test
# groups (`.config/nextest.toml`), the web template `support/display_colony.rs`
# copies into its colony, the WebKit environment of the drivers and the tier
# script the station calls.
_EXTRA = ("glob:.config/nextest.toml", "glob:templates/web/**", "glob:workshop/tools/wkenv.sh",
          "glob:scripts/test-tier.sh")
_RUST = ("rust_src", "rust_test")


def _shown(paths, n=3):
    return ", ".join(paths[:n]) + (" (+%d)" % (len(paths) - n) if len(paths) > n else "")


def map_doubt(cov, cwhy, repo, metadata, now=None):
    """`(why, since, terms)`: why the map cannot judge the locks (None when it
    fits), the files changed since its rev and the test terms of `map_fits`."""
    from . import coverage as coverage_mod
    from . import impact as impact_mod
    from . import narrow as narrow_mod
    if cov is None:
        return cwhy or "no coverage map", [], []
    short = str(cov.get("rev"))[:9]
    if not coverage_mod.is_complete(cov):
        return "coverage map %s is not complete" % short, [], []
    try:
        age = coverage_mod.age_days(cov, now)
    except (KeyError, TypeError, ValueError, AttributeError):
        return "coverage map %s has no valid built_at" % short, [], []
    if age < -narrow_mod.FUTURE_SKEW_DAYS:
        return "coverage map %s was built in the future (%s)" % (short, cov.get("built_at")), [], []
    if age > impact_mod.STALE_DAYS:
        return ("coverage map %s is %.1f days old (> %d)"
                % (short, age, impact_mod.STALE_DAYS)), [], []
    ok, notes, since, terms = narrow_mod.map_fits(cov, repo, metadata)
    if not ok:
        return "; ".join(n.removeprefix("narrowing: ").split(" -- ")[0] for n in notes) \
            or "coverage map %s does not fit" % short, [], []
    return None, since, terms


def station_input(path):
    """Is `path` an input of the station that no coverage part can show?"""
    from . import cache
    if path in gate_plan.BROWSER_LOCKS:
        return False   # a lock file is judged per lock (a)
    cls = gate_plan._classes_of_path(path)
    for spec in list(cache.INPUTS.get(ROW) or ()) + list(_EXTRA):
        head, sep, rest = spec.partition(":")
        if (not sep and head in cls) or (head == "glob" and fnmatch.fnmatchcase(path, rest)):
            return True
    return False


def decide(files, repo, metadata=None, cov=None, cwhy=None, now=None):
    """`{lock: (runs, why)}` for every lock of `gate_plan.BROWSER_LOCKS`."""
    from . import spawn_closure
    locks = sorted(gate_plan.BROWSER_LOCKS)
    doubt, since, terms = map_doubt(cov, cwhy, repo, metadata, now)
    if doubt:
        return {l: (True, "the map cannot judge: %s" % doubt) for l in locks}
    want = sorted(set(files) | set(since))
    inputs = [p for p in want if station_input(p)]
    if inputs:
        return {l: (True, "station input %s" % _shown(inputs)) for l in locks}
    parts = cov.get("parts") or []
    known = {f for p in parts for f in (p.get("files") or {})}
    classes = {p: gate_plan._classes_of_path(p) for p in want}
    work = [p for p in want if "workspace" in classes[p]]
    if work:
        return {l: (True, "workspace file %s" % _shown(work)) for l in locks}
    rust = [p for p in want if classes[p] & set(_RUST)]
    # every crate file, not only rust_src/rust_test: the map lists .rs files
    # only, and `crates/meclaw-core/schemas/ubf-body.json` is compiled into
    # the colony by `include_str!` (re-check S1b B1) -- unseen is never untouched
    unknown = [p for p in want if (p in rust or gate_plan._crate_of(p)) and p not in known]
    if unknown:
        return {l: (True, "%s not in the coverage map" % _shown(unknown)) for l in locks}
    field = cov.get(browser_cover.FIELD)
    spawners = {s for v in spawn_closure.spawners(repo).values() for s in v}
    short = str(cov.get("rev"))[:9]
    out = {}
    for lock in locks:
        bid = browser_cover.binary_of(lock)
        entry = field.get(lock) if isinstance(field, dict) else None
        if not isinstance(entry, dict) or entry.get("measured") is not True:
            why = (entry or {}).get("why") if isinstance(entry, dict) else None
            out[lock] = (True, "not measured: %s" % (
                why or ("no entry in the map" if isinstance(field, dict)
                        else "the map has no %s" % browser_cover.FIELD)))
            continue
        mine = {f for p in parts if p.get("binary") == bid for f in (p.get("files") or {})}
        hit = [p for p in want if p == lock or p in mine]
        changed = [t for t in terms
                   if t in ("binary_id(=%s)" % bid, "rdeps(%s)" % gate_plan._crate_of(lock))]
        spawn = sorted(s for s in spawners if s == lock or s in mine)
        if hit:
            out[lock] = (True, "covers %s" % _shown(hit))
        elif changed:
            out[lock] = (True, "changed since map %s: %s" % (short, ", ".join(changed)))
        elif spawn and rust:
            out[lock] = (True, "spawns a workspace binary: %s" % _shown(spawn))
        else:
            out[lock] = (False, "measured, untouched")
    return out


def _cmds(row):
    return row["cmds"] if "cmds" in row else ([row["cmd"]] if "cmd" in row else [])


def apply(rows, files, repo, metadata=None, env=None, now=None, cov=None):
    """Cut the `browser:display` row of an integration plan in place (see the
    module docstring); the NOTE lines. No lock left: the row leaves `rows`."""
    from . import narrow as narrow_mod
    row = next((r for r in rows if r.get("name") == ROW), None)
    if row is None:
        return []
    if not narrow_mod.enabled(env):
        return ["browser locks: narrowing off (%s=0) -- every planned lock runs" % narrow_mod.ENV]
    cwhy = None
    if cov is None:
        cov, cwhy = narrow_mod.newest_map()
    extra = set(gate_plan._browser_select(types.SimpleNamespace(cmds=_cmds(row)))["extra"])
    planned = {l for l in gate_plan.BROWSER_LOCKS if browser_cover.binary_of(l) in extra}
    verdict = decide(files, repo, metadata=metadata, cov=cov, cwhy=cwhy, now=now)
    notes = ["browser lock %s: %s (%s)" % (browser_cover._stem(l),
                                            "runs" if verdict[l][0] else "skipped", verdict[l][1])
             for l in sorted(planned)]
    keep = {l for l in planned if verdict[l][0]}
    if keep == planned:
        return notes + ["browser:display: all %d planned locks run" % len(planned)]
    # the browser units outside the locks the wave diff reaches (rule 11 of the
    # resolver, GH #1161 re-check M-3) stay -- `keep` never drops them
    cmds = gate_plan.browser_cmds(True, keep=keep,
                                  moved=gate_plan.browser_moved(files, "integration", repo))
    if not cmds:
        rows.remove(row)
        return notes + ["browser:display: no lock runs -- the station leaves the plan"]
    row.pop("cmd", None)
    row.pop("cmds", None)
    if len(cmds) == 1:
        row["cmd"] = cmds[0]
    else:
        row["cmds"] = cmds
    row["select"] = gate_plan._browser_select(types.SimpleNamespace(cmds=cmds))
    row["scope"] = "%d of %d locks" % (len(keep), len(planned))
    return notes + ["browser:display: %d of %d planned locks run" % (len(keep), len(planned))]
