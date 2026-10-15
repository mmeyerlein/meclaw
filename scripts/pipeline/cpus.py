"""The one CPU source of the kit (GH #1199, re-check A-I1).

`--kit` (plans/export-fixtures/export_tree_tests.py `ci_runner`) runs the kit in
the public runner's form: its CPUs pinned by `taskset` (the affinity) and its
topology in a sysroot (`MECLAW_FP_SYSROOT`, the variable `lane_fingerprint.sh`
reads too). Every CPU count of the kit comes from here, so `--kit` and the
public CI compute the same widths:

  * `threads()` -- the CPUs this process may run on (its affinity), what GNU
    `nproc` answers without OMP_NUM_THREADS/OMP_THREAD_LIMIT. `os.cpu_count()`
    counts the machine, not the affinity: under the runner's 4-CPU pin of a
    24-thread host it said 24 where the public runner says 4.
  * `physical_cores(root)` -- the real cores of the sysroot's topology.

Stdlib only, no sibling imports: importable as `pipeline.cpus`,
`scripts.pipeline.cpus` and relatively. `scripts/pipeline/hygiene.py` runs
alone on a lane and counts its affinity inline (see its `_affinity_threads`).
The structure rule `test_pipeline_hygiene.KitCpuSource` keeps every other
source out of the kit.
"""

import os
import re

SYSROOT_ENV = "MECLAW_FP_SYSROOT"


def sysroot():
    """The root the topology is read from: `MECLAW_FP_SYSROOT` (the runner form
    of `--kit`, tests), else `/`."""
    return os.environ.get(SYSROOT_ENV) or "/"


def threads():
    """The CPUs this process may run on: its affinity, else (no affinity on this
    platform) the machine's count. The ONE `os.cpu_count()` of the kit."""
    try:
        return max(1, len(os.sched_getaffinity(0)))
    except (AttributeError, OSError):
        return max(1, os.cpu_count() or 1)


def _sibling_lists(root):
    """The `thread_siblings_list` of every CPU of the topology under `root`, or
    None when the kernel does not say (no /sys)."""
    base = os.path.join(root, "sys", "devices", "system", "cpu")
    try:
        names = os.listdir(base)
    except OSError:
        return None
    out = []
    for name in names:
        if not re.fullmatch(r"cpu\d+", name):
            continue
        try:
            with open(os.path.join(base, name, "topology", "thread_siblings_list")) as f:
                out.append(f.read().strip())
        except OSError:
            continue
    return out


def topology_cpus(root=None):
    """The CPUs the topology under `root` (default `sysroot()`) lists, or None."""
    lists = _sibling_lists(sysroot() if root is None else root)
    return len(lists) if lists else None


def physical_cores(root=None):
    """The real cores of the topology under `root` (default `sysroot()`): distinct
    sibling sets of /sys topology (6 on a 6-core CPU with hyper-threading, nproc
    12); None when the kernel does not say. A guest (`hypervisor` in
    /proc/cpuinfo) whose topology shows no siblings says nothing either: KVM shows
    every vCPU as its own core (build02: 12 x 1 on an E5-1660 v2, 6 cores + HT),
    so its count is the threads, not the cores (OR-TB.S3.15). A diagnostic of the
    fingerprint only: the colonies come from the host file's `cores=`
    (`schedule.colony_capacity`), never from this count."""
    root = sysroot() if root is None else root
    lists = _sibling_lists(root)
    if not lists:
        return None
    sets = set(lists)
    if len(sets) == len(lists) and _is_guest(root):
        return None
    return len(sets)


def _is_guest(root):
    try:
        with open(os.path.join(root, "proc", "cpuinfo")) as f:
            return any(ln.startswith("flags") and "hypervisor" in ln.split() for ln in f)
    except OSError:
        return False
