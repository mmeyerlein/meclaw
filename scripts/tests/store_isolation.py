"""Keep every script test away from the machine's pipeline store.

The store's default path is `~/.local/state/meclaw/pipeline/pipeline.db`
(`scripts/pipeline/store.py` `default_path`). A test that runs `strand.sh`,
`gate.sh` or a `pipeline` verb in a subprocess without `MECLAW_PIPELINE_DB`
writes its throw-away passes into that store, where they skew the history
the retro rules read and trigger their evaluation (wave dev-tempo W11: about
63 phantom passes).

Every test module under `scripts/tests/` calls `isolate()` from its
`setUpModule` and `restore()` from its `tearDownModule`; the guard
`test_store_isolation.py` refuses a module that does not. `isolate()` points
`MECLAW_PIPELINE_DB` at a fresh temporary directory and switches the night
watchdog off (`MECLAW_PIPELINE_NIGHT=0`), so subprocesses that inherit the
environment never reach the default store either. It also drops the outbound
targets (`MECLAW_PIPELINE_METRICS_URL`, `MECLAW_GRAFANA_URL`,
`GRAFANA_ADMIN_PASS`), so no test pushes metric lines or dashboards to the
machine's VictoriaMetrics or Grafana. And it installs the side-effect guard
(`side_effect_guard.py`): fakes for `systemctl`, `ssh`, `poweroff` ... in
front of `PATH`, so no test reaches the machine's systemd or a remote host
(the e28 incident of 09.10.); `restore()` removes them.

And it hands no lock of an outer gate run: the engine gives every station
`MECLAW_GATE_LOCK` and takes that lock before its first cargo station, so a
`gate.sh` subprocess of the self-test station that flocked the inherited path
waited for the run that waits for it (W3 gate build02, timeout after 170
tests). `MECLAW_GATE_LOCK` points into the temporary directory;
`MECLAW_CARGO_LOCK_HELD` and `MECLAW_STAGE_LOCK` go.
"""

import os
import pathlib
import shutil
import tempfile

try:
    from . import side_effect_guard
except ImportError:  # run as a top-level module (`python3 store_isolation.py`)
    import side_effect_guard

DB_ENV = "MECLAW_PIPELINE_DB"
NIGHT_ENV = "MECLAW_PIPELINE_NIGHT"
OUTBOUND_ENVS = ("MECLAW_PIPELINE_METRICS_URL", "MECLAW_GRAFANA_URL", "GRAFANA_ADMIN_PASS")
LOCK_ENV = "MECLAW_GATE_LOCK"
# MECLAW_WORKTREE_ROOT (GH #1290): a host-wide setting that changes where `new` puts a
# worktree and how a tree is keyed; a case sets it itself where it tests it.
OUTER_RUN_ENVS = ("MECLAW_CARGO_LOCK_HELD", "MECLAW_STAGE_LOCK", "MECLAW_WORKTREE_ROOT")
DEFAULT_STORE = pathlib.Path.home() / ".local/state/meclaw/pipeline/pipeline.db"
# The system directories a test's minimal PATH keeps (`trapped_path()`).
MINIMAL_PATH = os.pathsep.join(("/usr/bin", "/bin"))

_SAVED = []
FLAKE_ISSUES_ENV = "MECLAW_FLAKE_ISSUES"   # flakes.ISSUES_ENV
FLAKE_DRAFTS_ENV = "MECLAW_FLAKE_DRAFTS"   # flakes.DRAFTS_ENV


NARROW_ENV = "MECLAW_GATE_NARROW"  # = pipeline.narrow.ENV
COVERAGE_DIR_ENV = "MECLAW_PIPELINE_COVERAGE_DIR"  # = pipeline.narrow.coverage_dir
GRAPH_DIR_ENV = "MECLAW_PIPELINE_GRAPH_DIR"  # = pipeline.graph.graph_dir
GRAPH_AUTO_ENV = "MECLAW_PIPELINE_GRAPH_AUTO"  # = pipeline.graph.AUTO_ENV
REPOS_ENV = "MECLAW_PIPELINE_REPOS"  # = pipeline.register.REPOS_ENV


def isolate():
    """Point the store at a temporary directory; returns the store path."""
    tmp = tempfile.mkdtemp(prefix="pipeline-store-")
    names = (DB_ENV, NIGHT_ENV, LOCK_ENV, FLAKE_ISSUES_ENV, FLAKE_DRAFTS_ENV, NARROW_ENV,
             COVERAGE_DIR_ENV, GRAPH_DIR_ENV, GRAPH_AUTO_ENV, REPOS_ENV) \
        + OUTBOUND_ENVS + OUTER_RUN_ENVS
    saved = {name: os.environ.get(name) for name in names}
    _SAVED.append((tmp, saved))
    path = pathlib.Path(tmp) / "pipeline.db"
    os.environ[DB_ENV] = str(path)
    os.environ[NIGHT_ENV] = "0"
    os.environ[LOCK_ENV] = str(pathlib.Path(tmp) / "cargo.lock")
    # W8: a test's pass never files or comments a flake issue, and a draft
    # lands in the temp dir, never under plans/ of the repo.
    os.environ[FLAKE_ISSUES_ENV] = "0"
    os.environ[FLAKE_DRAFTS_ENV] = str(pathlib.Path(tmp) / "flake-drafts")
    # #1138: a test's plan never reads the machine's graph or coverage map.
    os.environ[NARROW_ENV] = "0"
    # ... nor the night's binary check the machine's coverage map (review Z-I1 M3).
    os.environ[COVERAGE_DIR_ENV] = str(pathlib.Path(tmp) / "coverage")
    # welle-validierung-2 3a: a narrowing builds the graph of the head itself --
    # never into the machine's graph directory, and only where a test asks.
    os.environ[GRAPH_DIR_ENV] = str(pathlib.Path(tmp) / "graph")
    os.environ[GRAPH_AUTO_ENV] = "0"
    # GH #1262: the register words come from the built-in entries alone, never
    # from the machine's host file (absent here; a test writes its own).
    os.environ[REPOS_ENV] = str(pathlib.Path(tmp) / "pipeline-repos.json")
    for name in OUTBOUND_ENVS + OUTER_RUN_ENVS:
        os.environ.pop(name, None)
    side_effect_guard.install()
    return path


def restore():
    """Undo the newest `isolate()` and remove its temporary directory."""
    if not _SAVED:
        return
    tmp, saved = _SAVED.pop()
    side_effect_guard.remove()
    for name, old in saved.items():
        if old is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = old
    shutil.rmtree(tmp, ignore_errors=True)


def trapped_path(path=MINIMAL_PATH):
    """`path` with the side-effect guard's fakes in front of it.

    A test that hands a subprocess a minimal PATH would otherwise drop the
    trap and reach the machine's `systemctl` again
    (`test_store_isolation`, `test_no_test_hands_a_subprocess_a_fixed_search_path`).
    """
    trap = side_effect_guard.current_dir()
    return trap + os.pathsep + path if trap else path


# The lane a fake `ssh` plays runs on this machine; its hygiene lists the
# `pipe-run-*` scopes (W3: 123 guard hits in one self-test run). This fake
# answers that listing like a lane without scopes and refuses everything
# else as loudly as the guard does. A test that plays the state of a run's
# scope (`is-active pipe-run-*.scope`, the probe after a failed fetch) writes
# that state into `is-active` next to this fake; without the file the probe
# is refused like every other verb and finds no answer.
LANE_SYSTEMCTL = """#!/bin/sh
echo "$*" >> "$(dirname "$0")/systemctl.calls"
[ "$1 $2" = "--user list-units" ] && exit 0
if [ "$1 $2" = "--user is-active" ] && [ -f "$(dirname "$0")/is-active" ]; then
  case "$3" in pipe-run-*.scope) ;; *) echo "lane fake systemctl: refused (argv: $*)" >&2; exit 97 ;; esac
  read -r state < "$(dirname "$0")/is-active"
  echo "$state"
  [ "$state" = active ] && exit 0
  exit 3
fi
echo "lane fake systemctl: refused (argv: $*)" >&2
exit 97
"""

# ... and its fingerprint asks `loginctl show-user <user> -p Linger`. The
# real one answered for the machine running the test: on build02 its user
# had no linger, `lanes status` grew a `linger no` and two inventory cases
# were red on that lane alone (W3 gate 09.10., 20261009T020350Z). The fake
# lane keeps its user manager (linger yes) and refuses every other verb.
LANE_LOGINCTL = """#!/bin/sh
echo "$*" >> "$(dirname "$0")/loginctl.calls"
[ "$1" = show-user ] && echo yes && exit 0
echo "lane fake loginctl: refused (argv: $*)" >&2
exit 97
"""


def write_lane_fakes(bin_dir):
    """Put the lane's `systemctl` and `loginctl` fakes into `bin_dir` (the fake `ssh`'s directory)."""
    for name, body in (("systemctl", LANE_SYSTEMCTL), ("loginctl", LANE_LOGINCTL)):
        path = pathlib.Path(bin_dir) / name
        path.write_text(body, encoding="utf-8")
        path.chmod(0o755)
    return pathlib.Path(bin_dir)
