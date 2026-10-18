"""meclaw -- the station register of a satellite repository (W10, V0 section 4).

WHY THIS EXISTS
===============
meclaw-core plans its gate from `scripts/gate_plan.py`; the satellites
(coding, soul2me, orga-hive) have no resolver of their own, only a serial
`scripts/gate.py`. The same core should run all of them: one executor, one
receipt, one journal, one set of metrics. So a satellite DECLARES its
stations in `<repo>/pipeline/stations.json` and this module turns the
register and a diff into the plan form of `plan.build_plan` (W3) -- the
executor cannot tell a satellite plan from a meclaw plan.

THE REGISTER (`pipeline/stations.json`)
=======================================
    {"schema": "meclaw.pipeline.stations/1", "repo": "<word, see below>",
     "stations": [{"name", "scope", "stage": ["strand", "integration", "release"],
                   "cargo": 0, "cmds": [{"argv": [...], "cwd": "."}],
                   "needs": {"cpu": n, "colony": n}, "inputs": ["scripts/**", ...],
                   "always": false, "after": [], "timeout_min": n,
                   "parallel": {"by": "file|dir|module|case|cmd", "workers": n,
                                "slot_env": "NAME"}}]}

`parallel.workers` may be the word `"density"` (GH #1163): the station's
size follows the lane's PHYSICAL cores (hardware ruling 10.10.: build01-03 are
6 cores + HT, nproc 12; build04 2 x 6 + HT, nproc 24). The colony density D is
`cores // CORES_PER_COLONY` (cores counted on the lane with `lscpu
-p=Core,Socket`; unknown cores count as nproc / 2), at least 1;
`MECLAW_COLONY_DENSITY=<n>` overrides it (anything but an integer >= 1 is a
RegisterError), capped only at the slot pool: the CLI's `--slots`, else the
register's top-level `"colony_slots": n` (the colony slots the satellite's lab
owns -- its ports; orga and soul2me: 8), else none (D slots 0..D-1).
With `parallel.colonies_per_worker` c (default 1) the station runs
`max(1, D // c)` workers, books `workers * c` colonies and its share of the
lane's threads, `ceil(nproc * colonies / D)` -- at full density the whole
lane, so no unit station runs beside a full lab. A density station never
carries `needs.colony` or `needs.cpu` itself. `parallel.workers: "cores"`
runs one worker per real core (`MECLAW_UNIT_WORKERS=<n>` overrides) and
books that many threads. D is one colony per real core; on a lane the real
cores come from its host-file line (`cores=<n>`), without it nproc / 2. The
plan names cores and density as a note (`colony density 6 (lane cores 6,
one colony per core), nproc 12; unit 6 workers (cores); lab 6 workers x 1
colony, cpu 12`). Two density stations never share D: each takes the
whole budget and the later waits for the earlier one's slots; pair modules
that should run beside single ones go into ONE station with
`unit_runner.py --pair GLOB`.

A top-level `"lane_env": ["NAME", ...]` (welle-tb tb-zcol) names the caller's
variables the stations need on the lane (soul2me: `SOUL2ME_LAB_MECLAW_BIN`):
the gate hands exactly those that are set to the dispatch's `+env`, by exact
name, never over the kit's own; a name that reads like a secret or belongs to
the kit, or a value with a control character, is a RegisterError and the run
aborts before anything travels. Nothing else of the caller's environment
reaches a lane.

`always` and `parallel.slot_env` are additive to V0 (OR-DT.W10.1), and so is
`lib: build|read|none` (GH #1142): the station's role towards the set's
build/lib, which orders the stations that touch it.

THE REPOSITORIES (host file, GH #1262)
======================================
The register's `repo` must be a word the kit knows. Three are built in
(`REPOS`: coding, soul2me, orga); every other repository is named by the host
file `${MECLAW_PIPELINE_REPOS:-~/.config/meclaw/pipeline-repos.json}` beside
the lanes file `~/.config/meclaw/lanes`. Like that file it is this machine's
own and never travels with the export:

    {"schema": "meclaw.pipeline.repos/1",
     "repos": [{"repo": "<word>", "path": "~/<checkout>",
                "asset_cache": "<dir>", "release_key": "<key>"}]}

    repo         the register word a `pipeline/stations.json` names (lower
                 case; never a built-in word, nor `meclaw`, the core's own id)
    path         the repository's checkout on this machine, absolute or below
                 `~/`; descriptive only -- the kit never reads it, it takes the
                 tree from `--repo`
    asset_cache  the directory below `~/.cache/` whose `meclaw/<v>/` holds the
                 release assets the repository's sets pin (`set_pins`)
    release_key  the key a set of the repository pins its meclaw release with

Every entry needs exactly these four fields. An entry only adds a repository:
a built-in word is refused, since its sets pin with both `RELEASE_KEYS` and an
entry carries one key. Without the file exactly the built-in entries hold; a
file that cannot be read or does not have this form is a RegisterError -- fail
closed, a typo there never drops a repository silently.

THE RESOLVER
============
A station is planned when its `stage` holds the mode's stage and the diff
hits one of its `inputs` (fnmatch over the changed paths, `*` crosses `/`),
or when it says `always: true`. Mode `release` (and `ci`, and `--all`) plans
every station of the stage -- the satellites' full pass. The diff is
`git diff --name-only <base> HEAD` plus the dirty tree; the base is the
merge base with the satellite's main branch (`main` or `master`, whichever
exists), like `plan.resolve_base` does against meclaw's `master`. A strand
without that merge base (no main branch, foreign history), or whose diff git
cannot read, has no diff to narrow from: every station of the stage runs, with
a NOTE (GH #1199) -- the rule `plan.build_plan` keeps as well.

THE PLAN
========
`build_plan` returns `{mode, rev, base, repo, classes: [], crates: [],
stations, plan_reason, critical_path_s, critical_path}`; every station has
`{name, scope, cargo, cmds, env, reason, stage, needs, inputs, timeout_min,
run, keep_ws, after, when}` and `parallel` where the register names one.
`stage` uses the plan's letters (S, I, R), `needs` carries every key of
`plan.NEEDS_KEYS`, `inputs` writes a path glob as `glob:<pattern>`. The
placeholders in `argv` and `cwd` are resolved here:

    {root}      the satellite's root
    {pipeline}  this core's `scripts/pipeline` directory (on a lane the kit's,
                `/srv/target/kit/<sha8>/scripts/pipeline`)
    {python}    the core's interpreter
    {workers}   the station's workers (capped by the slots handed in)
    {slots}     comma-separated, one per colony the station books, at least
                one per worker (GH #1163: a worker may hold two colonies)

anywhere in an item (`SOUL2ME_LAB_SLOTS={slots}` after an `env` prefix too);
any other `{name}` is a RegisterError. A station's cmds keep their order.

TRANSPORT (pure functions; `lane.py push` calls them after the W3 merge)
=========================================================================
    kit_push_spec    `scripts/pipeline.py` + `scripts/pipeline/**` + every
                     repository module they import (`kit_imports`) of the
                     calling core to `/srv/target/kit/<sha8>/`, once per core
                     revision (`already_there` reads the lane's listing)
    mirrors_spec     the `.git` directories of meclaw-core, voice2vision,
                     orga-hive -- all refs -- for `build_set.py sources`
    assets_spec      the release-asset cache `~/.cache/<repo>/meclaw/<v>/`

All three are rsync argv lists that exclude every secret by NAME
(`secret_name`): a secret never leaves this machine.

THE SET PINS (GH #1143)
=======================
    set_pins         what a satellite's `sets/*.json` need on a lane: the
                     pinned releases of `~/.cache/<repo>/meclaw/` and one
                     mirror per pinned source repository (meclaw-core for
                     `meclaw_core`), each at its path below the home; a pin
                     that cannot travel is a RegisterError (fail-closed)
    pins_mirror_spec the mirrors' rsync argv lists, additive (no --delete)

`lane.push_pins` carries both and checks them on the lane; `gate` derives
them before the satellite's tree travels.
"""

import dataclasses
import fnmatch
import json
import os
import re
import subprocess
import sys

from . import cpus
from . import plan as _plan

SCHEMA = "meclaw.pipeline.stations/1"
REGISTER_PATH = "pipeline/stations.json"
# the built-in repository words; the host file names more (`repo_entries`)
REPOS = ("coding", "soul2me", "orga")
STAGES = ("strand", "integration", "release")
PARALLEL_BY = ("file", "dir", "module", "case", "cmd")
MODES = ("strand", "integration", "release", "ci")
# the mode -> the register's stage word and the plan's stage letter
STAGE_OF_MODE = {"strand": "strand", "integration": "integration", "release": "release",
                 "ci": "release"}
LETTER = {"strand": "S", "integration": "I", "release": "R"}
FULL_MODES = ("release", "ci")
NEEDS_KEYS = ("cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo")
# A station's role towards the set's build/lib (GH #1142): `build` rebuilds it
# (rmtree, then rename), `read` copies from it, `none` neither. Without the
# field: `set` builds, a station with needs.colony > 0 reads.
LIB_ROLES = ("build", "read", "none")
TIMEOUT_DEFAULT = 10
WHOLE_TREE = ("glob:**",)
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
MAIN_BRANCHES = ("main", "master")
PLACEHOLDERS = ("{root}", "{pipeline}", "{python}", "{workers}", "{slots}")
_CLASS = re.compile(r"^[a-z_]+:")
_PLACEHOLDER = re.compile(r"\{[a-z_]+\}")

PIPELINE_DIR = os.path.dirname(os.path.abspath(__file__))

# The colony density (GH #1163, welle-tb S5): build03 (12 vCPU) ran eight
# colonies beside eight unit workers -- soul2me lab 9/15 RED (peer_book acks
# missing), orga's lab sum +35 %: 8 colonies + 8 workers on 6 physical cores.
# The owner's hardware ruling (10.10.): CPU-bound parallelism follows the real
# cores -- one colony per real core, one unit worker per real core. The
# diagnosis at density 6 on build03 (6 real cores) had every soul2me lab module
# provisioned (peer_book 12/12) and orga green; the real cores come from the
# host file (`cores=`), never from lscpu inside a VM (build03 counted 12).
DENSITY = "density"
CORES = "cores"
CORES_PER_COLONY = 1
DENSITY_ENV = "MECLAW_COLONY_DENSITY"
UNIT_WORKERS_ENV = "MECLAW_UNIT_WORKERS"
# The colony slots a satellite's lab owns (its port range), top level of the
# register; the old fixed cap of 8 colonies per lane (a W3 start value,
# OR-DT-11, never measured) is gone -- the density is the lane's real cores.
COLONY_SLOTS = "colony_slots"
_COUNT = re.compile(r"^[0-9]+$")
# The variables a satellite's stations need from the caller (welle-tb tb-zcol
# A: in Z SOUL2ME_LAB_MECLAW_BIN never reached the lab, the kit hands a lane
# only the golden switches). An ALLOWLIST by exact name at the register's top
# level, never a pattern; a name that reads like a secret or that the kit
# itself sets is refused, so a declaration can neither carry a key off this
# machine nor steer the runner.
LANE_ENV = "lane_env"
_ENV_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")
# A secret is refused by SUBSTRING, case blind (Re-Check M1: the part-wise
# filter let OPENAI_APIKEY, GH_PAT, DBPASS ... through); rather refuse too
# much -- a refused harmless name costs a rename, a passed key leaves the box.
_SECRET_WORDS = ("KEY", "TOKEN", "SECRET", "PASS", "PAT", "AUTH", "BEARER", "JWT",
                 "SALT", "CRED", "PRIVATE", "CERT")
# Short words that only count as a whole `_` part (as substrings they would
# hit half the dictionary).
_SECRET_PARTS = ("PW", "PWD", "COOKIE", "SESSION")
_KIT_PREFIXES = ("MECLAW_", "CARGO", "RUST", "NEXTEST_", "LD_", "PYTHON", "GIT_", "SSH_")
_KIT_NAMES = ("PATH", "HOME", "USER", "SHELL", "TMPDIR", "IFS", "ENV", "BASH_ENV")


class RegisterError(ValueError):
    """The register is missing or does not have the V0 section 4 form."""


# --- the repositories (host file, GH #1262) ---------------------------------

REPOS_ENV = "MECLAW_PIPELINE_REPOS"
REPOS_FILE = "~/.config/meclaw/pipeline-repos.json"
REPOS_SCHEMA = "meclaw.pipeline.repos/1"
REPO_FIELDS = ("repo", "path", "asset_cache", "release_key")
# matched with fullmatch: `$` would let a trailing newline through
_REPO_WORD = re.compile(r"[a-z][a-z0-9_-]*")
_CACHE_DIR = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_SET_KEY = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
# schedule.CORE_REPO: a satellite word equal to it would claim the core's lane trees
_CORE_WORD = "meclaw"


def repos_path(environ=None):
    """The host file of the kit's repositories: `MECLAW_PIPELINE_REPOS`, else
    `~/.config/meclaw/pipeline-repos.json`."""
    env = os.environ if environ is None else environ
    return env.get(REPOS_ENV) or os.path.expanduser(REPOS_FILE)


def _repo_entry(e, where, seen):
    if not isinstance(e, dict) or set(e) != set(REPO_FIELDS):
        raise RegisterError("%s must have exactly the fields %s" % (where, ", ".join(REPO_FIELDS)))
    word, path, cache, key = (e[f] for f in REPO_FIELDS)
    if not isinstance(word, str) or not _REPO_WORD.fullmatch(word):
        raise RegisterError("%s: repo must be a lower-case word" % where)
    if word == _CORE_WORD or word in REPOS:
        # a replaced built-in entry would read one release key only, and a set
        # pinning the other would drop out of the pin transport silently
        raise RegisterError("%s: repo %s is built in (%s); the host file only adds repositories"
                            % (where, word, "|".join((_CORE_WORD,) + REPOS)))
    if word in seen:
        raise RegisterError("%s: repo %s is named twice" % (where, word))
    if not isinstance(path, str) or not (os.path.isabs(path) or path.startswith("~/")):
        raise RegisterError("%s: path must be absolute or below ~/" % where)
    if not isinstance(cache, str) or not _CACHE_DIR.fullmatch(cache):
        raise RegisterError("%s: asset_cache must be one directory name below ~/.cache" % where)
    if not isinstance(key, str) or not _SET_KEY.fullmatch(key):
        raise RegisterError("%s: release_key must be a set key" % where)
    return {"repo": word, "path": path, "asset_cache": cache, "release_keys": (key,)}


def repo_entries(path=None):
    """`{word: {repo, path, asset_cache, release_keys}}` -- the built-in
    entries, joined by the entries of the host file `path` (default
    `repos_path()`), built-in words first. No file: exactly the
    built-in entries (`path` None). RegisterError on a file that cannot be
    read or does not have the form of the module docstring."""
    entries = {w: {"repo": w, "path": None, "asset_cache": ASSET_CACHES[w],
                   "release_keys": RELEASE_KEYS} for w in REPOS}
    path = path or repos_path()
    try:
        with open(path, encoding="utf-8") as fh:
            doc = json.load(fh)
    except FileNotFoundError:
        return entries
    except OSError as e:
        raise RegisterError("%s: %s" % (path, e.strerror or e)) from None
    except ValueError as e:
        raise RegisterError("%s: not JSON (%s)" % (path, e)) from None
    if not isinstance(doc, dict) or doc.get("schema") != REPOS_SCHEMA \
            or not isinstance(doc.get("repos"), list):
        raise RegisterError('%s: must be {"schema": "%s", "repos": [...]}' % (path, REPOS_SCHEMA))
    seen = set()
    for i, e in enumerate(doc["repos"]):
        entry = _repo_entry(e, "%s: repos[%d]" % (path, i), seen)
        seen.add(entry["repo"])
        entries[entry["repo"]] = entry
    return entries


def repo_words():
    """The register words the kit accepts (`repo_entries`, in its order)."""
    return tuple(repo_entries())


# --- the register ---------------------------------------------------------------

def register_path(repo_root):
    return os.path.join(str(repo_root), REGISTER_PATH)


def is_satellite(repo_root):
    """A satellite tree: a station register and no meclaw resolver."""
    root = str(repo_root)
    return (os.path.isfile(register_path(root))
            and not os.path.isfile(os.path.join(root, "scripts", "gate_plan.py")))


def _fail(what):
    raise RegisterError("%s: %s" % (REGISTER_PATH, what))


def _check_station(st, i):
    where = "stations[%d]" % i
    if not isinstance(st, dict):
        _fail("%s is not an object" % where)
    name = st.get("name")
    if not isinstance(name, str) or not name:
        _fail("%s has no name" % where)
    where = "station %s" % name
    stage = st.get("stage")
    if not isinstance(stage, list) or not stage or any(s not in STAGES for s in stage):
        _fail("%s: stage must be a non-empty list of %s" % (where, "|".join(STAGES)))
    cmds = st.get("cmds")
    if not isinstance(cmds, list) or not cmds:
        _fail("%s: cmds must be a non-empty list" % where)
    for c in cmds:
        argv = c.get("argv") if isinstance(c, dict) else None
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
            _fail("%s: every cmd needs an argv of strings" % where)
        if not isinstance(c.get("cwd", "."), str):
            _fail("%s: cwd must be a string" % where)
        unknown = sorted({p for a in argv + [c.get("cwd", ".")] for p in _PLACEHOLDER.findall(a)}
                         - set(PLACEHOLDERS))
        if unknown:
            _fail("%s: unknown placeholder %s (known: %s)"
                  % (where, " ".join(unknown), " ".join(PLACEHOLDERS)))
    inputs = st.get("inputs", [])
    if not isinstance(inputs, list) or not all(isinstance(x, str) for x in inputs):
        _fail("%s: inputs must be a list of globs" % where)
    needs = st.get("needs", {})
    if not isinstance(needs, dict) or not all(
            isinstance(v, int) and not isinstance(v, bool) and v >= 0 for v in needs.values()):
        _fail("%s: needs must map names to counts" % where)
    if not isinstance(st.get("always", False), bool):
        _fail("%s: always must be a boolean" % where)
    if st.get("lib", "") not in ("",) + LIB_ROLES:
        _fail("%s: lib must be one of %s" % (where, "|".join(LIB_ROLES)))
    if st.get("cargo", 0) not in (0, 1):
        _fail("%s: cargo must be 0 or 1" % where)
    timeout = st.get("timeout_min", TIMEOUT_DEFAULT)
    if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or timeout <= 0:
        _fail("%s: timeout_min must be a positive number" % where)
    par = st.get("parallel")
    if par is not None:
        if not isinstance(par, dict) or par.get("by") not in PARALLEL_BY:
            _fail("%s: parallel.by must be one of %s" % (where, "|".join(PARALLEL_BY)))
        workers = par.get("workers", 1)
        density = workers == DENSITY
        cores = workers == CORES
        if not (density or cores) and (not isinstance(workers, int) or isinstance(workers, bool)
                                       or workers < 1):
            _fail('%s: parallel.workers must be >= 1, "%s" or "%s"' % (where, DENSITY, CORES))
        cpw = par.get("colonies_per_worker")
        if cpw is not None:
            if not density:
                _fail('%s: parallel.colonies_per_worker needs parallel.workers "%s"'
                      % (where, DENSITY))
            if not isinstance(cpw, int) or isinstance(cpw, bool) or cpw < 1:
                _fail("%s: parallel.colonies_per_worker must be >= 1" % where)
        carried = [k for k in ("colony", "cpu") if k in needs]
        if density and carried:
            _fail("%s: a density station derives %s from the lane's colony density "
                  "-- drop it from needs" % (where, " and ".join("needs." + k for k in carried)))
        if cores and "cpu" in needs:
            _fail("%s: a cores station derives needs.cpu from the lane's physical cores "
                  "-- drop it from needs" % where)
        if not isinstance(par.get("slot_env", ""), str):
            _fail("%s: parallel.slot_env must be a variable name" % where)
    return name


def load_register(repo_root):
    """The validated register of `repo_root` (RegisterError otherwise)."""
    path = register_path(repo_root)
    try:
        with open(path, encoding="utf-8") as fh:
            doc = json.load(fh)
    except OSError as e:
        raise RegisterError("%s: %s" % (REGISTER_PATH, e.strerror or e)) from None
    except ValueError as e:
        raise RegisterError("%s: not JSON (%s)" % (REGISTER_PATH, e)) from None
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        _fail("schema must be %s" % SCHEMA)
    words = repo_words()
    if doc.get("repo") not in words:
        _fail("repo must be one of %s" % "|".join(words))
    stations = doc.get("stations")
    if not isinstance(stations, list):
        _fail("stations must be a list")
    names = []
    for i, st in enumerate(stations):
        name = _check_station(st, i)
        if name in names:
            _fail("station %s appears twice" % name)
        names.append(name)
    for st in stations:
        after = st.get("after", [])
        if not isinstance(after, list) or any(a not in names for a in after):
            _fail("station %s: after names an unknown station" % st["name"])
    colony_slots(doc)
    lane_env_names(doc)
    return doc


def reads_like_secret(name):
    """True when a variable name carries one of the secret words anywhere
    (substring, case blind) or a short secret word as a whole `_` part."""
    upper = name.upper()
    return any(word in upper for word in _SECRET_WORDS) or any(
        part in _SECRET_PARTS for part in upper.split("_"))


def lane_env_names(doc):
    """The register's `lane_env` names (a list, maybe empty); RegisterError for
    anything but distinct exact upper-case names that are no secret and no
    variable of the kit's own."""
    names = doc.get(LANE_ENV, [])
    if not isinstance(names, list) or not all(isinstance(n, str) for n in names):
        _fail("%s must be a list of variable names" % LANE_ENV)
    for name in names:
        if not _ENV_NAME.match(name):
            _fail("%s: %r is no exact variable name (A-Z, 0-9, _)" % (LANE_ENV, name))
        if reads_like_secret(name):
            _fail("%s: %s reads like a secret -- a secret never leaves this machine"
                  % (LANE_ENV, name))
        if name in _KIT_NAMES or name.startswith(_KIT_PREFIXES):
            _fail("%s: %s is the kit's own variable" % (LANE_ENV, name))
    if len(set(names)) != len(names):
        _fail("%s names a variable twice" % LANE_ENV)
    return list(names)


def lane_env(doc, environ):
    """The declared `lane_env` variables set in `environ`, by exact name, for
    the dispatch's `+env`; a value that cannot travel as one word (newline,
    NUL) is a RegisterError (fail-closed: the run aborts before the push)."""
    out = {}
    for name in lane_env_names(doc):
        if name not in environ:
            continue
        value = environ[name]
        if "\n" in value or "\r" in value or "\0" in value:
            _fail("%s: the value of %s cannot travel (control character)" % (LANE_ENV, name))
        out[name] = value
    return out


def colony_slots(doc):
    """The register's `colony_slots` (an integer >= 1), or None; RegisterError otherwise."""
    if COLONY_SLOTS not in doc:
        return None
    n = doc[COLONY_SLOTS]
    if isinstance(n, bool) or not isinstance(n, int) or n < 1:
        _fail("%s must be an integer >= 1" % COLONY_SLOTS)
    return n


# --- the resolver ---------------------------------------------------------------

def _glob_of(item):
    """The path glob of an `inputs` item, or None for a class (`tool:`, `artefact:`)."""
    if item.startswith("glob:"):
        return item[len("glob:"):]
    return None if _CLASS.match(item) else item


def _hits(pattern, path):
    return fnmatch.fnmatch(path, pattern) or (
        pattern.startswith("**/") and fnmatch.fnmatch(path, pattern[3:]))


def resolve(register, files, mode, all_stations=False):
    """The planned stations of `register` for the diff `files`, in register order.

    Each is a copy of its register row plus `reason`: one `glob:<g>: <path>`
    per glob the diff hits, `always`, or the full-pass reason."""
    if mode not in MODES:
        raise RegisterError("unknown mode %r (one of %s)" % (mode, "|".join(MODES)))
    stage = STAGE_OF_MODE[mode]
    full = mode in FULL_MODES or all_stations
    files = sorted(set(files))
    out = []
    for st in register.get("stations") or []:
        if stage not in st["stage"]:
            continue
        if full:
            reason = ["%s: all stations" % (mode if mode in FULL_MODES else "--all")]
        else:
            reason = []
            for item in st.get("inputs", []):
                g = _glob_of(item)
                hit = next((p for p in files if _hits(g, p)), None) if g else None
                if hit is not None:
                    reason.append("glob:%s: %s" % (g, hit))
            if not reason and st.get("always"):
                reason = ["always"]
        if reason:
            row = dict(st)
            row["reason"] = reason
            out.append(row)
    return out


# --- the diff -------------------------------------------------------------------

def _git(root, *args):
    proc = subprocess.run(["git", "-C", str(root)] + list(args), capture_output=True, text=True)
    return proc.stdout.strip() if proc.returncode == 0 else None


def main_branch(repo_root):
    """`main` or `master`, whichever the satellite has (local first, then origin)."""
    for prefix in ("refs/heads/", "refs/remotes/origin/"):
        for name in MAIN_BRANCHES:
            if _git(repo_root, "rev-parse", "--verify", "--quiet", prefix + name) is not None:
                return name if prefix == "refs/heads/" else "origin/" + name
    return None


def _dirty_files(root):
    out = subprocess.run(["git", "-C", str(root), "status", "--porcelain=v1", "-z",
                          "--untracked-files=all"], capture_output=True, text=True).stdout
    entries = out.split("\0")
    paths, i = [], 0
    while i < len(entries):
        entry = entries[i]
        i += 1
        if len(entry) < 4:
            continue
        paths.append(entry[3:])
        if entry[0] in "RC" or entry[1] in "RC":
            if i < len(entries) and entries[i]:
                paths.append(entries[i])
            i += 1
    return paths


def resolve_base(repo_root, mode, base=None):
    """The diff base: `base`, else the merge base with the main branch; `""`
    without one (no narrowing, see `_resolve_base`)."""
    return _resolve_base(repo_root, mode, base)[0]


def _resolve_base(repo_root, mode, base=None):
    """`(base, unresolved)`: `unresolved` says why there is no merge base with
    the main branch (None when there is one). There is no `HEAD~1` stand-in
    for it -- that diff hid every strand commit but the last (GH #1199, the
    re-check I3 class); `build_plan` runs every station of the stage instead.

    On the main branch itself (the merge base IS HEAD): HEAD with a dirty
    tree, else HEAD~1 (the empty tree for a first commit) -- the rule of
    `plan.resolve_base`."""
    if base:
        return base, None
    ref = main_branch(repo_root)
    if not ref:
        return "", "no main or master branch"
    mb = _git(repo_root, "merge-base", ref, "HEAD")
    if not mb:
        return "", "git merge-base %s HEAD: no merge base" % ref
    if mb != (_git(repo_root, "rev-parse", "HEAD") or ""):
        return mb, None
    if _dirty_files(repo_root):
        return mb, None
    return (_git(repo_root, "rev-parse", "--verify", "--quiet", "HEAD~1") or EMPTY_TREE), None


def changed_files(repo_root, base):
    """`git diff --name-only --no-renames <base> HEAD` plus the dirty tree,
    sorted: a rename names both sides, so a station whose inputs match only the
    source is planned too."""
    return _changed_scope(repo_root, base)[0]


def _changed_scope(repo_root, base):
    """`(paths, unresolved)`: `changed_files`, and why git could not diff
    against `base` (None when it could) -- a failing diff is no empty diff,
    `build_plan` runs every station then (GH #1199, re-check I4)."""
    diff = []
    unresolved = None
    if base:
        out = _git(repo_root, "diff", "--name-only", "--no-renames", base, "HEAD")
        if out is None:
            unresolved = "git diff %s HEAD: failed" % base[:12]
        diff = (out or "").splitlines()
    return sorted({p for p in diff + _dirty_files(repo_root) if p}), unresolved


# --- the plan -------------------------------------------------------------------

def _slot_list(slots):
    if slots is None:
        return None
    if isinstance(slots, str):
        return [s.strip() for s in slots.split(",") if s.strip()]
    return [str(s) for s in slots]


def _subst(text, values):
    for key, value in values.items():
        text = text.replace(key, value)
    return text


def _inputs(row):
    out = []
    for item in row.get("inputs") or []:
        g = _glob_of(item)
        out.append("glob:" + g if g is not None else item)
    if not out:
        out = list(WHOLE_TREE)
    from . import lane   # the one reader of the Python form (re-check tv-bn M7p M1, M7q H1)
    if any(lane.python_rest(c["argv"]) is not None for c in row["cmds"]):
        out.append("tool:python")
    return list(dict.fromkeys(out))


def _slot_take(row, workers, slots, offset):
    """(workers, slots) of one station: one slot per booked colony, at least
    one per worker (GH #1163), from `offset` on, from the pool when there is
    one (from its start again when the rest is too short)."""
    w = int(workers or (row.get("parallel") or {}).get("workers") or 1)
    try:
        colony = int((row.get("needs") or {}).get("colony") or 0)
    except (TypeError, ValueError):
        colony = 0
    k = max(w, colony)
    pool = _slot_list(slots)
    if pool is None:  # build_plan always hands a pool; a bare call counts from `offset`
        return w, [str(i) for i in range(offset, offset + k)]
    # A range that would run past the pool starts from its head again and waits
    # (_after_slots) -- never a slot the satellite's lab does not own.
    k = max(1, min(k, len(pool)))
    w = min(w, k)
    start = offset if offset + k <= len(pool) else 0
    return w, pool[start:start + k]


def count_cores(lscpu):
    """The physical cores in `lscpu -p=Core,Socket` output (distinct core and
    socket pairs), or None when the text names none."""
    pairs = set()
    for line in (lscpu or "").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(",")
        if len(parts) < 2 or not all(_COUNT.match(x.strip()) for x in parts[:2]):
            return None
        pairs.add((parts[0].strip(), parts[1].strip()))
    return len(pairs) or None


def local_cores():
    """This machine's physical cores (`lscpu`), or None."""
    try:
        res = subprocess.run(["lscpu", "-p=Core,Socket"], capture_output=True, text=True,
                             timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    return count_cores(res.stdout) if res.returncode == 0 else None


def _lane_cores(lane_cpu, lane_cores):
    """(cores, why): the lane's physical cores; unknown counts as nproc / 2."""
    if lane_cores:
        return int(lane_cores), "lane cores %d" % int(lane_cores)
    cores = max(1, int(lane_cpu or 1) // 2)
    return cores, "lane cores %d (unknown: nproc %d / 2)" % (cores, int(lane_cpu or 1))


def _env_count(environ, name):
    raw = environ.get(name)
    if raw is None:
        return None
    text = str(raw).strip()
    if not _COUNT.match(text) or int(text) < 1:
        raise RegisterError("%s=%r: must be an integer >= 1" % (name, raw))
    return int(text)


def colony_density(lane_cpu, environ=None, pool=None, lane_cores=None, pool_name="slots"):
    """(D, why): the colonies a lane runs side by side (GH #1163): its real
    cores // CORES_PER_COLONY, one colony per real core (`lane_cores`, declared
    in the lane's host-file line; unknown: nproc / 2).
    `MECLAW_COLONY_DENSITY` in `environ` (default: os.environ) overrides it,
    fail-closed; D is capped at the slot `pool` (`pool_name` in the reason) when
    there is one, else not at all."""
    environ = os.environ if environ is None else environ
    forced = _env_count(environ, DENSITY_ENV)
    if forced is not None:
        d, why = forced, "%s=%d" % (DENSITY_ENV, forced)
    else:
        cores, why = _lane_cores(lane_cpu, lane_cores)
        d = max(1, cores // CORES_PER_COLONY)
        why += ", one colony per core" if CORES_PER_COLONY == 1 else " // %d" % CORES_PER_COLONY
    pool = _slot_list(pool)
    if pool is not None and d > max(1, len(pool)):
        d = max(1, len(pool))
        why += ", capped at %d %s" % (d, pool_name)
    return d, why


def _is_density(row):
    return (row.get("parallel") or {}).get("workers") == DENSITY


def _is_cores(row):
    return (row.get("parallel") or {}).get("workers") == CORES


def _cores_row(row, lane_cpu, lane_cores, environ, workers=None):
    """A cores station: one worker per physical core (MECLAW_UNIT_WORKERS
    overrides, the CLI's `--workers` stays first), booking that many threads."""
    forced = _env_count(environ, UNIT_WORKERS_ENV)
    w = int(workers) if workers else (forced or _lane_cores(lane_cpu, lane_cores)[0])
    needs = dict(row.get("needs") or {}, cpu=max(1, min(int(lane_cpu), w)))
    return dict(row, parallel=dict(row["parallel"], workers=w), needs=needs)


def _density_row(row, density, lane_cpu, workers=None):
    """The row of a density station with its workers and needs resolved; the
    CLI's `--workers` stays first."""
    par = row["parallel"]
    cpw = int(par.get("colonies_per_worker") or 1)
    w = int(workers) if workers else max(1, density // cpw)
    colony = w * cpw
    share = -(-int(lane_cpu) * colony // max(1, int(density)))  # ceil: its part of the lane
    needs = dict(row.get("needs") or {}, colony=colony, cpu=max(1, min(int(lane_cpu), share)))
    return dict(row, parallel=dict(par, workers=w), needs=needs)


def _density_note(density, why, lane_cpu, rows, cored=()):
    parts = ["colony density %d (%s), nproc %d" % (density, why, int(lane_cpu))]
    for r in rows:
        w = r["parallel"]["workers"]
        if r["name"] in cored:
            parts.append("%s %d %s (cores)" % (r["name"], w, "worker" if w == 1 else "workers"))
            continue
        cpw = int(r["parallel"].get("colonies_per_worker") or 1)
        parts.append("%s %d %s x %d %s, cpu %d" % (
            r["name"], w, "worker" if w == 1 else "workers",
            cpw, "colony" if cpw == 1 else "colonies", r["needs"]["cpu"]))
    return "; ".join(parts)


def _station(row, root, pipeline_dir, python, workers, slots, offset=0):
    par = row.get("parallel")
    if _uses_slots(row):
        w, picked = _slot_take(row, workers, slots, offset)
    else:  # no slots to hand out: the pool never narrows its workers (GH #1163)
        w, picked = int(workers or (par or {}).get("workers") or 1), []
    slot_s = ",".join(picked)
    values = {"{root}": root, "{pipeline}": pipeline_dir, "{python}": python,
              "{workers}": str(w), "{slots}": slot_s}
    needs = dict.fromkeys(NEEDS_KEYS, 0)
    needs["cpu"] = 1
    needs["cargo"] = int(row.get("cargo", 0))
    needs.update({k: int(v) for k, v in (row.get("needs") or {}).items()})
    if par and "cpu" not in (row.get("needs") or {}):
        needs["cpu"] = w
    st = {
        "name": row["name"],
        "scope": row.get("scope", ""),
        "cargo": int(row.get("cargo", 0)),
        "cmds": [{"argv": [_subst(a, values) for a in c["argv"]],
                  "cwd": _subst(c.get("cwd") or ".", values)} for c in row["cmds"]],
        "env": "none",
        "reason": list(row.get("reason") or []),
        "stage": [LETTER[s] for s in STAGES if s in row["stage"]],
        "needs": needs,
        "inputs": _inputs(row),
        "timeout_min": row.get("timeout_min", TIMEOUT_DEFAULT),
        "run": True,
        "keep_ws": False,
        "after": list(row.get("after") or []),
        "when": {"outcome": "success"},
    }
    if par:
        st["parallel"] = dict(par, workers=w)
    return st


def _lib_role(row):
    """The row's role towards build/lib: its `lib` field, else the old rule."""
    if row.get("lib"):
        return row["lib"]
    if row["name"] == "set":
        return "build"
    return "read" if (row.get("needs") or {}).get("colony", 0) > 0 else "none"


def _after_set(stations, roles):
    """`set` rebuilds the library under build/ (rmtree, then rename) and every
    station that boots a colony copies from it: such a station never runs beside
    `set`. It runs after `set`, unless `set` already waits for it (W10 part B, F1:
    coding's lab-boot and apply read a half-removed build/lib on a lane)."""
    by = {s["name"]: s for s in stations}
    for b in [s["name"] for s in stations if roles.get(s["name"]) == "build"]:
        before, todo = set(), list(by[b]["after"])
        while todo:
            n = todo.pop()
            if n not in before:
                before.add(n)
                todo.extend(by[n]["after"])
        for s in stations:
            if (roles.get(s["name"]) == "read" and s["name"] not in before
                    and b not in s["after"]):
                s["after"].append(b)


def _waits_for(by, name, other):
    """True when station `name` already waits for `other`, directly or not."""
    seen, todo = set(), list(by[name]["after"])
    while todo:
        n = todo.pop()
        if n == other:
            return True
        if n not in seen and n in by:
            seen.add(n)
            todo.extend(by[n]["after"])
    return False


def _uses_slots(row):
    return any("{slots}" in a for c in row["cmds"] for a in c["argv"])


def _after_colony(stations, slot_names, roles):
    """Every colony station resolves the set's library, and the satellite's
    resolve rebuilds build/lib (rmtree, then rename): two of them never run side
    by side. The colony stations without slots run one after another in register
    order, the slot stations after all of them (W10 part B, G1: coding's apply
    read a half-removed lib while lab-boot rebuilt it)."""
    by = {s["name"]: s for s in stations}
    oneshots = [s["name"] for s in stations if roles.get(s["name"]) == "read"
                and s["name"] not in slot_names]
    for i, name in enumerate(oneshots):
        for n in oneshots[:i]:
            if n not in by[name]["after"] and not _waits_for(by, n, name):
                by[name]["after"].append(n)
    for s in stations:
        if s["name"] in slot_names and roles.get(s["name"]) == "read":
            for n in oneshots:
                if n not in s["after"] and not _waits_for(by, n, s["name"]):
                    s["after"].append(n)


def _after_slots(stations, taken):
    """Slot stations take disjoint ranges of the pool in register order; one
    that had to start from the pool's head again waits for every station whose
    range it shares (W10 part B, G2: soul2me's lab and lab-ops both took slot 0
    and the colony pool let them run side by side)."""
    by = {s["name"]: s for s in stations}
    order = [s["name"] for s in stations if s["name"] in taken]
    for i, name in enumerate(order):
        for n in order[:i]:
            if (taken[n] & taken[name] and n not in by[name]["after"]
                    and not _waits_for(by, n, name)):
                by[name]["after"].append(n)


def _toposort(stations):
    """Stable topological order over `after`; RegisterError on a cycle."""
    index = {s["name"]: i for i, s in enumerate(stations)}
    pending = {s["name"]: {a for a in s["after"] if a in index} for s in stations}
    out = []
    while pending:
        ready = sorted((n for n, deps in pending.items() if not deps), key=index.get)
        if not ready:
            raise RegisterError("cycle in the register: %s" % sorted(pending))
        out.append(ready[0])
        del pending[ready[0]]
        for deps in pending.values():
            deps.discard(ready[0])
    by = {s["name"]: s for s in stations}
    return [by[n] for n in out]


# One longest-chain rule since the W3 merge: plan.critical_path (W10 slot).
critical_path = _plan.critical_path


def build_plan(repo_root, mode, base, files=None, *, pipeline_dir=None, python=None,
               workers=None, slots=None, all_stations=False, run_root=None, only=None,
               lane_cpu=None, environ=None, lane_cores=None):
    """The plan of one satellite pass, in the form of W3's `plan.build_plan`.

    `run_root` is where the stations run (`{root}`), when that is not
    `repo_root` -- the satellite's checkout on a lane. `lane_cpu` (default:
    this machine's) and `environ` (default: os.environ) size the density
    stations (GH #1163); the plan's `notes` say the density chosen.

    -> {mode, rev, base, repo, classes, crates, stations, plan_reason,
        critical_path_s, critical_path}"""
    root = os.path.abspath(str(repo_root))
    reg = load_register(root)
    unresolved = None
    if files is None:
        base, unresolved = _resolve_base(root, mode, base)
        files, unreadable = _changed_scope(root, base)
        unresolved = unresolved or unreadable
    # GH #1199: no clear base, no diff -- fail closed, never a narrower one
    # (`plan.unclear_base_note`, the rule both planners share)
    rows = resolve(reg, files, mode, all_stations=all_stations or bool(unresolved))
    pool, pool_name = _slot_list(slots), "slots"
    if pool is None and colony_slots(reg):
        pool, pool_name = [str(i) for i in range(colony_slots(reg))], "colony slots"
    density_note = None
    dense = {r["name"] for r in rows if _is_density(r)}
    cored = {r["name"] for r in rows if _is_cores(r)}
    if dense or cored:
        environ = os.environ if environ is None else environ
        if not lane_cpu:
            lane_cpu = cpus.threads()
            lane_cores = lane_cores or local_cores()
        density, why = colony_density(lane_cpu, environ, pool, lane_cores, pool_name)
        rows = [_density_row(r, density, lane_cpu, workers) if r["name"] in dense else
                _cores_row(r, lane_cpu, lane_cores, environ, workers) if r["name"] in cored else r
                for r in rows]
        density_note = _density_note(density, why, lane_cpu,
                                     [r for r in rows if r["name"] in dense | cored], cored)
    if pool is None and any(_uses_slots(r) for r in rows):
        # No pool named: the lane's colonies are the pool, D slots (or its real cores).
        if not (dense or cored):
            if not lane_cpu:
                lane_cpu = cpus.threads()
                lane_cores = lane_cores or local_cores()
            density = _lane_cores(lane_cpu, lane_cores)[0]
        pool = [str(i) for i in range(density)]
    stations, taken, offset = [], {}, 0
    for r in rows:
        stations.append(_station(r, run_root or root, pipeline_dir or PIPELINE_DIR,
                                 python or sys.executable, workers, pool, offset))
        if _uses_slots(r):
            w, picked = _slot_take(r, workers, pool, offset)
            taken[r["name"]] = set(picked)
            offset = pool.index(picked[0]) + len(picked)
    names = [s["name"] for s in stations]
    for s in stations:
        s["after"] = [a for a in dict.fromkeys(s["after"]) if a in names and a != s["name"]]
    _after_slots(stations, taken)
    roles = {r["name"]: _lib_role(r) for r in rows}
    _after_set(stations, roles)
    _after_colony(stations, {r["name"] for r in rows if _uses_slots(r)}, roles)
    stations = _toposort(stations)
    if only:
        # W3's `--only` (GH #1142, KF2): an unknown name refuses, the rest
        # stays in the plan with `run: false`.
        known = {st["name"] for st in reg["stations"]}
        unknown = [n for n in only if n not in known]
        if unknown:
            _fail("unknown station(s) in only: %s" % ", ".join(unknown))
        for s in stations:
            if s["name"] not in only:
                s["run"] = False
    run = [s for s in stations if s.get("run") is not False]
    secs = {s["name"]: float(s["timeout_min"]) * 60.0 for s in run}
    cp_s, cp = critical_path(run, secs)
    doc = {
        "mode": mode, "rev": _git(root, "rev-parse", "HEAD") or "", "base": base or "",
        "repo": reg["repo"], "classes": [], "crates": [], "stations": stations,
        "plan_reason": {s["name"]: list(s["reason"]) for s in stations},
        "critical_path_s": float(cp_s), "critical_path": cp,
    }
    if only:
        doc["only"] = list(only)
    if unresolved:
        doc.setdefault("notes", []).append(_plan.unclear_base_note(unresolved))
    if density_note:
        doc.setdefault("notes", []).append(density_note)
    return doc


# --- transport (the verbs live in lane.py: push_kit, push_mirrors, push_assets) ---

KIT_ROOT = "/srv/target/kit"
MIRRORS_ROOT = "/srv/target/mirrors"
# relative: the lane user's home, so `~/.cache/<repo>/meclaw/<v>/` lands where
# the satellites' `build_set.fetch_assets` looks for it
ASSETS_ROOT = ".cache"
SECRET_EXCLUDES = ("--exclude=.env", "--exclude=.env.*", "--exclude=vault.env",
                   "--exclude=vault-pass*", "--exclude=provider.env", "--exclude=.netrc",
                   "--exclude=.git-credentials", "--exclude=.cargo/credentials*")


def secret_name(path):
    """True for a file that holds a secret, by its NAME and nothing else:
    `.env`, `.env.<anything>` except `*.example`, `vault.env`, `vault-pass*`,
    `provider.env`, `.netrc`, `.git-credentials`, `.cargo/credentials*`. A
    path COMPONENT `vault` or `.env*` is no secret. The one rule: W3's
    `lane.secret_name` asks here."""
    base = path.rsplit("/", 1)[-1]
    if base.endswith(".example"):
        return False
    if base in (".env", "vault.env", "provider.env", ".netrc", ".git-credentials"):
        return True
    if base.startswith(".env.") or base.startswith("vault-pass"):
        return True
    full = "/" + path
    return full.endswith("/.cargo/credentials") or "/.cargo/credentials." in full


def _dest(target, path):
    return "%s:%s" % (target, path) if target else path


def _rsync(target):
    """The program word of an rsync argv: the narrow point's checked absolute
    path for a lane (`lane.rsync_program`, orchestrator OR-T10-25: the
    `env -i` of `--rsync-path` holds only if the rsync that reads it is the
    system's or a registered fake), `rsync` for a local copy."""
    if not target:
        return "rsync"
    from . import lane  # late: lane imports this module
    return lane.rsync_program()


def _rsh(target):
    """`-e <ssh>` for an rsync to a lane: the remote shell comes from the
    kit's narrow point (`lane.rsh`, re-check tv-bn M7e B-M1) -- without it
    rsync ran whatever `ssh` PATH named, past the checked program."""
    if not target:
        return []
    from . import lane  # late: lane imports this module
    return ["-e", lane.rsh(), lane.rsync_path()]


def _anchored(path):
    """`/a/b/c` -> `/a/b/./c`: rsync -R recreates `c` below the destination."""
    parent, leaf = os.path.split(path.rstrip("/"))
    return "%s/./%s/" % (parent, leaf)


def _git_dir(repo):
    common = _git(repo, "rev-parse", "--git-common-dir")
    if common:
        return os.path.abspath(os.path.join(repo, common))
    return os.path.join(repo, ".git")


def _mirror_source(repo):
    """(rsync source, mirror name) of one repository: its git directory
    (a worktree's is the main repository's, `--git-common-dir`), anchored
    so rsync -R recreates `<name>/.git/` (or a bare `<name>/`)."""
    gdir = _git_dir(os.path.abspath(os.path.expanduser(str(repo))))
    if os.path.basename(gdir) == ".git":
        name = os.path.basename(os.path.dirname(gdir))
        return "%s/./%s/.git/" % (os.path.dirname(os.path.dirname(gdir)), name), name
    return _anchored(gdir), os.path.basename(gdir.rstrip("/"))


def mirror_dirs(paths, dest_root=MIRRORS_ROOT):
    """The directory below `dest_root` each repository's mirror lands in
    (`mirrors_spec` writes there; `git -C` works on it)."""
    return ["%s/%s" % (dest_root.rstrip("/"), _mirror_source(repo)[1]) for repo in paths]


def mirrors_spec(paths, dest_root=MIRRORS_ROOT, target=None):
    """One rsync argv per repository: its git directory with ALL refs to
    `<dest_root>/<name>/.git/` (`git -C <dest_root>/<name>` works there),
    lock files and secrets excluded; `<name>` as in `mirror_dirs`."""
    return [[_rsync(target), "-aR", "--delete"] + list(SECRET_EXCLUDES)
            + ["--exclude=*.lock"] + _rsh(target)
            + [_mirror_source(repo)[0], _dest(target, dest_root.rstrip("/") + "/")]
            for repo in paths]


def assets_spec(cache_root, dest_root=ASSETS_ROOT, target=None, versions=None):
    """rsync argv(s) for the release-asset cache `~/.cache/<repo>/meclaw/`:
    the whole cache, or one argv per version in `versions`; the layout
    `<repo>/meclaw/<v>/` is kept below `dest_root`, secrets excluded."""
    cache = os.path.abspath(os.path.expanduser(str(cache_root))).rstrip("/")
    leaf = os.path.basename(cache)
    repo_dir = os.path.dirname(cache)
    base = os.path.dirname(repo_dir)
    stem = "%s/./%s/%s" % (base, os.path.basename(repo_dir), leaf)
    srcs = [stem + "/"] if not versions else ["%s/%s/" % (stem, v) for v in versions]
    return [[_rsync(target), "-aR"] + list(SECRET_EXCLUDES) + _rsh(target) + [src, _dest(target, dest_root.rstrip("/") + "/")]
            for src in srcs]


# --------------------------------------------------------------------------
# The set pins -> what a satellite's lane needs (GH #1143)
# --------------------------------------------------------------------------

# The release-asset cache below `~/.cache/` per register `repo`: each
# satellite's `build_set.fetch_assets` reads `~/.cache/<name>/meclaw/<v>/`,
# and orga-hive's directory is not named after its register word. The
# built-in entries' values; a host-file entry names its own (`repo_entries`).
ASSET_CACHES = {"coding": "coding", "soul2me": "soul2me", "orga": "orga-hive"}
# The release a set pins: `meclaw` (coding, orga-hive) or `meclaw_release` (soul2me);
# a built-in entry reads both, a host-file entry its `release_key` alone.
RELEASE_KEYS = ("meclaw", "meclaw_release")
_SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_HEX = re.compile(r"\b[0-9a-f]{7,40}\b")


@dataclasses.dataclass
class PinTransport:
    """What the set pins of a satellite need on its lane: the releases of the
    asset cache `cache` and one mirror per source repository
    (`{repo, git_dir, rel, revs, pins}`, `rel` relative to `home`)."""

    repo: str
    home: str
    cache: str
    versions: list
    mirrors: list


def _expand(path, home, root):
    path = str(path)
    if path == "~" or path.startswith("~/"):
        path = os.path.join(home, path[2:])
    return os.path.normpath(path if os.path.isabs(path) else os.path.join(root, path))


def _rev(repo, rev):
    return _git(repo, "rev-parse", "--verify", "--quiet", "%s^{commit}" % rev)


def _asset_there(folder):
    for top, _dirs, files in os.walk(folder):
        if any(not secret_name(f) for f in files):
            return True
    return False


def set_pins(repo_root, core_root, home=None):
    """The `PinTransport` of the satellite at `repo_root` from its
    `sets/*.json`: every pinned release (the entry's release keys) must be
    in this machine's cache, every `meclaw_core` commit in the repository of
    `core_root`, every `sources.<n>` tag or commit in its repository below
    `home`. Fail-closed: anything that cannot travel is a RegisterError that
    names every broken pin at once -- a lane run on a missing pin builds the
    wrong set or none (W10 part B, F4/S1: both pushed by hand on build03)."""
    home = os.path.abspath(os.path.expanduser(home or "~"))
    root = os.path.abspath(repo_root)
    name = load_register(root)["repo"]
    entry = repo_entries()[name]
    keys = entry["release_keys"]
    cache = os.path.join(home, ".cache", entry["asset_cache"], "meclaw")
    versions, mirrors, bad = set(), {}, []

    def mirror(repo, revs, why):
        """One pin of `repo`: every revision it names (a tag, a commit,
        coding's `source_commits.<n>`) must be here and name the same
        commit -- checking the tag alone lets a stale commit pin through."""
        if not os.path.isdir(repo) or _git(repo, "rev-parse", "--git-dir") is None:
            bad.append("%s: repository %s is not here" % (why, repo))
            return
        shas = {}
        for rev in revs:
            got = _rev(repo, rev)
            if not got:
                bad.append("%s: %s unknown in %s (fetch it)" % (why, rev, repo))
                return
            shas[rev] = got
        if len(set(shas.values())) > 1:
            bad.append("%s: %s disagree in %s" % (why, ", ".join(
                "%s = %s" % (r, s[:12]) for r, s in shas.items()), repo))
            return
        sha = next(iter(shas.values()))
        gdir = _git_dir(repo)
        top = os.path.dirname(gdir)
        if os.path.basename(gdir) != ".git" or os.path.relpath(top, home).startswith(".."):
            bad.append("%s: %s is not below %s with a .git directory -- the lane finds a "
                       "source at the same path below its home" % (why, repo, home))
            return
        m = mirrors.setdefault(gdir, {"repo": top, "git_dir": gdir,
                                      "rel": os.path.relpath(top, home), "revs": [], "pins": []})
        if sha not in m["revs"]:
            m["revs"].append(sha)
        m["pins"].append(why)

    sets_dir = os.path.join(root, "sets")
    files = sorted(f for f in os.listdir(sets_dir) if f.endswith(".json")) \
        if os.path.isdir(sets_dir) else []
    for f in files:
        rel = "sets/" + f
        try:
            with open(os.path.join(sets_dir, f), encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, ValueError) as e:
            bad.append("%s: not JSON (%s)" % (rel, e))
            continue
        if not isinstance(doc, dict):
            bad.append("%s: not a JSON object" % rel)
            continue
        # keyed by their JSON form: a list or object as value is a finding
        # below, not a TypeError that hides every other finding
        named = {}
        for k in keys:
            if k in doc:
                named.setdefault(json.dumps(doc[k], sort_keys=True), doc[k])
        if len(named) > 1:
            bad.append("%s: %s disagree (%s)" % (rel, " and ".join(keys),
                                                 ", ".join(sorted(map(str, named.values())))))
        for v in named.values():
            if not (isinstance(v, str) and _SEMVER.match(v)):
                bad.append("%s: meclaw release %r is not x.y.z" % (rel, v))
            elif not _asset_there(os.path.join(cache, v)):
                bad.append("%s: meclaw release %s is not in %s (fetch it with the "
                           "satellite's build_set.py)" % (rel, v, cache))
            else:
                versions.add(v)
        if "meclaw_core" in doc:
            m = _HEX.search(str(doc["meclaw_core"]))
            if not m:
                bad.append("%s: meclaw_core %r names no commit" % (rel, doc["meclaw_core"]))
            else:
                core = os.path.dirname(_git_dir(os.path.abspath(core_root)))
                mirror(core, [m.group(0)], "%s meclaw_core" % rel)
        sources = doc.get("sources")
        if sources is not None and not isinstance(sources, dict):
            bad.append("%s: sources is not an object" % rel)
            sources = {}
        commits = doc.get("source_commits", {})
        if not isinstance(commits, dict):
            bad.append("%s: source_commits is not an object" % rel)
            commits = {}
        for n, src in sorted((sources or {}).items()):
            why = "%s sources.%s" % (rel, n)
            if not isinstance(src, dict):
                bad.append("%s: not an object" % why)
                continue
            revs = [str(src[k]) for k in ("tag", "commit") if src.get(k)]
            if not src.get("repo") or not revs:
                bad.append("%s: names no repo or neither tag nor commit" % why)
                continue
            if commits.get(n):
                revs.append(str(commits[n]))
            mirror(_expand(src["repo"], home, root), revs, why)
    if bad:
        raise RegisterError("set pins of %s cannot travel:\n  %s" % (root, "\n  ".join(bad)))
    return PinTransport(repo=name, home=home, cache=cache, versions=sorted(versions, key=_semver_key),
                        mirrors=sorted(mirrors.values(), key=lambda m: m["rel"]))


def _semver_key(v):
    return tuple(int(x) for x in v.split("."))


def pins_mirror_spec(transport, dest_root=".", target=None):
    """One rsync argv per mirror of `transport`: the git directory to
    `<dest_root>/<rel>/.git/` (dest_root: the lane user's home), all refs,
    additive (no --delete: what the lane holds stays), without lock files,
    this machine's worktree bookkeeping and secrets."""
    out = []
    for m in transport.mirrors:
        out.append([_rsync(target), "-aR"] + list(SECRET_EXCLUDES)
                   + ["--exclude=*.lock", "--exclude=/%s/.git/worktrees/" % m["rel"]] + _rsh(target)
                   + ["%s/./%s/.git/" % (transport.home, m["rel"]), _dest(target, dest_root.rstrip("/") + "/")])
    return out


# relative: the lane user's home, so `~/projeks/MeClaw/meclaw-core/workshop/
# tools/display-lab/` lands where the satellites' labs fold a screen (W10 G3)
DISPLAY_LAB_ROOT = "projeks/MeClaw/meclaw-core"
DISPLAY_LAB_FILES = ("workshop/tools/display-lab/patches.mjs",)


def display_lab_spec(core_root, dest_root=DISPLAY_LAB_ROOT, target=None):
    """rsync argv for display-lab's fold (`patches.mjs`, node built-ins only)."""
    core = os.path.abspath(os.path.expanduser(str(core_root))).rstrip("/")
    # --mkpath: a fresh lane has no ~/projeks/MeClaw yet (rsync exit 11, W10 I1)
    return [[_rsync(target), "-aR", "--mkpath"] + _rsh(target) + ["%s/./%s" % (core, f) for f in DISPLAY_LAB_FILES]
            + [_dest(target, dest_root.rstrip("/") + "/")]]


@dataclasses.dataclass
class KitPush:
    """The core's pipeline kit for one revision on a lane, pushed once."""

    rev: str
    sha8: str
    core_root: str
    dest_root: str
    files: list
    target: str = None

    @property
    def dest(self):
        return "%s/%s/" % (self.dest_root.rstrip("/"), self.sha8)

    @property
    def marker(self):
        return "%s/%s.ok" % (self.dest_root.rstrip("/"), self.sha8)

    def probe_argv(self):
        """What the lane lists; its output goes to `already_there`."""
        return ["ls", "-1", self.dest_root.rstrip("/")]

    def already_there(self, listing):
        """True when the lane holds this revision's kit completely (its marker)."""
        return ("%s.ok" % self.sha8) in (listing or "").split()

    def rsync_argv(self):
        """The filter comes from `files` -- one include per file plus its parent
        directories, everything else excluded: the list and the copy have one
        source. Re-check W10 I1: a fixed `scripts/pipeline/***` filter left the
        imported modules (`precheck.py`, `gate_plan.py`, ...) off the lane."""
        dirs, incl = set(), []
        for rel in self.files:
            parts = rel.strip("/").split("/")
            dirs.update("/".join(parts[:i]) for i in range(1, len(parts)))
            incl.append("--include=/%s" % "/".join(parts))
        return ([_rsync(self.target), "-a"] + list(SECRET_EXCLUDES)
                + ["--exclude=__pycache__/", "--exclude=*.pyc"]
                + ["--include=/%s/" % d for d in sorted(dirs)] + incl
                + ["--exclude=*"] + _rsh(self.target) + [self.core_root.rstrip("/") + "/", _dest(self.target, self.dest)])

    def remote_before(self):
        return ["mkdir", "-p", self.dest_root.rstrip("/")]

    def remote_after(self):
        """The marker last: a half-copied kit is never `already_there`."""
        return ["touch", self.marker]

    def steps(self):
        """mkdir, rsync, marker -- runnable as they are (ssh-wrapped for a target)."""
        # through the kit's narrow point (re-check tv-bn M7e B-M1); late: lane imports this module
        from . import lane
        wrap = (lambda argv: lane.ssh_argv(self.target, ["--"] + argv)) if self.target else (lambda a: a)
        return [wrap(self.remote_before()), self.rsync_argv(), wrap(self.remote_after())]


def _module_files(root, rel, deleted=()):
    """The repository modules under `scripts/` that the file `rel` imports:
    absolute imports only (the pipeline puts `scripts/` on sys.path, and
    `scripts.<x>` names the same tree from the root), lazy ones included."""
    import ast  # noqa: PLC0415 -- only the kit walk needs it
    try:
        with open(os.path.join(root, rel), encoding="utf-8") as fh:
            tree = ast.parse(fh.read(), filename=rel)
    except (OSError, SyntaxError, ValueError):
        return set()
    names = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names += [a.name for a in node.names]
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            names.append(node.module)
            names += ["%s.%s" % (node.module, a.name) for a in node.names]
    found = set()
    for name in names:
        parts = name.split(".")
        if parts[0] != "scripts":
            parts = ["scripts"] + parts
        base = "/".join(parts)
        for cand in (base + ".py", base + "/__init__.py"):
            if os.path.isfile(os.path.join(root, cand)) or cand in deleted:
                found.add(cand)
    return found


def kit_imports(root, start, deleted=()):
    """Every repository module under `scripts/` that the files `start` import,
    transitively (W10 part B: `pipeline/pre.py` imports `precheck`), sorted."""
    seen, todo = set(), list(start)
    while todo:
        rel = todo.pop()
        if rel in seen:
            continue
        seen.add(rel)
        if rel.endswith(".py"):
            todo += sorted(_module_files(root, rel, deleted) - seen)
    return sorted(seen - set(start))


def kit_files(core_root):
    """`scripts/pipeline.py`, `scripts/pipeline/**` and every repository module
    they import (transitively) without caches and secrets, sorted."""
    root = os.path.abspath(str(core_root))
    out = []
    if os.path.isfile(os.path.join(root, "scripts", "pipeline.py")):
        out.append("scripts/pipeline.py")
    top = os.path.join(root, "scripts", "pipeline")
    for cur, dirs, files in os.walk(top):
        dirs[:] = sorted(d for d in dirs if d != "__pycache__")
        for f in files:
            rel = os.path.relpath(os.path.join(cur, f), root).replace(os.sep, "/")
            if not f.endswith(".pyc") and not secret_name(rel):
                out.append(rel)
    out += [rel for rel in kit_imports(root, out) if not secret_name(rel)]
    return sorted(set(out))


def kit_push_spec(core_root, rev, dest_root=KIT_ROOT, target=None):
    """The kit of core revision `rev` for `/srv/target/kit/<sha8>/`."""
    rev = str(rev)
    if not re.fullmatch(r"[0-9a-f]{8,64}", rev):
        raise ValueError("kit revision must be a hex commit id, got %r" % rev)
    root = os.path.abspath(os.path.expanduser(str(core_root)))
    return KitPush(rev=rev, sha8=rev[:8], core_root=root, dest_root=dest_root,
                   files=kit_files(root), target=target)


# --- command line ---------------------------------------------------------------

def register(sub):
    p = sub.add_parser("register", help="the station register of a satellite (W10)")
    rsub = p.add_subparsers(dest="register_cmd", required=True)
    sp = rsub.add_parser("plan", help="print the plan JSON of a satellite pass")
    sp.add_argument("--repo", required=True, help="the satellite's root")
    sp.add_argument("--mode", choices=MODES, default="strand")
    sp.add_argument("--base", default=None)
    sp.add_argument("--files-from", default=None, metavar="FILE",
                    help="the changed paths, one per line ('-' = stdin) instead of git")
    sp.add_argument("--all", action="store_true", help="every station of the stage")
    sp.add_argument("--workers", type=int, default=None)
    sp.add_argument("--slots", default=None, help="comma-separated slots")
    sp.add_argument("--python", default=None)
    sp.add_argument("--pipeline-dir", default=None)
    sp.set_defaults(func=_cmd_plan)
    pp = rsub.add_parser("pins", help="what the set pins need on a lane: releases and mirrors "
                                      "(GH #1143)")
    pp.add_argument("--repo", required=True, help="the satellite's root")
    pp.add_argument("--core", default=os.path.dirname(os.path.dirname(PIPELINE_DIR)),
                    help="the meclaw-core checkout the `meclaw_core` pins resolve in")
    pp.add_argument("--home", default=None, help="the home the pinned paths are relative to")
    pp.set_defaults(func=_cmd_pins)


def _read_files(arg):
    if arg == "-":
        text = sys.stdin.read()
    else:
        with open(arg, encoding="utf-8") as fh:
            text = fh.read()
    return [line.strip() for line in text.splitlines() if line.strip()]


def _cmd_plan(args):
    try:
        files = _read_files(args.files_from) if args.files_from else None
        doc = build_plan(args.repo, args.mode, args.base, files, pipeline_dir=args.pipeline_dir,
                         python=args.python, workers=args.workers, slots=args.slots,
                         all_stations=args.all)
    except (RegisterError, OSError) as e:
        print("register: %s" % e, file=sys.stderr)
        return 2
    print(json.dumps(doc, indent=2))
    return 0


def _cmd_pins(args):
    try:
        doc = dataclasses.asdict(set_pins(args.repo, args.core, home=args.home))
    except (RegisterError, OSError) as e:
        print("register: %s" % e, file=sys.stderr)
        return 2
    print(json.dumps(doc, indent=2))
    return 0
