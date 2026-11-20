#!/usr/bin/env python3
"""Refuse a release binary that needs a CPU above the x86-64-v2 floor.

    python3 scripts/cpu_floor.py <binary> [--qemu <qemu-x86_64>] [--cpu <model>]

WHY THIS EXISTS
===============
The 0.89.2 assets were built on a new runner image and carried AVX2 code in
the bundled SQLite (its FTS5 set-up, compiled by the image's C toolchain).
Every host without AVX2 died with SIGILL (exit 132) on the first `--apply`,
while `--help`, `--version` and `--validate` on the builder passed: the
builder had AVX2 (GH #1571). A static count of AVX instructions does not
tell the two apart either, because the Rust crates ship AVX2 paths they only
take after a CPUID check. So the binary is RUN on the floor: under
`qemu-x86_64 -cpu Nehalem` (SSE4.2 and POPCNT, no AVX: x86-64-v2), once with
`--apply` of an empty manifest and once with `--validate` of
`examples/hello`. A run that a signal ends (SIGILL above all) is red. The
`--apply` run may exit non-zero (the binary answering is green); the
`--validate` run of examples/hello has to exit 0, so a qemu that fails by
itself (unknown CPU model, loader error) and never runs the binary is red
too: fail closed.

Exit: 0 green, 1 red (a signal, or validate not 0), 2 usage (no binary, no qemu).
"""
import argparse
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FLOOR_CPU = "Nehalem"  # x86-64-v2: SSE4.2, POPCNT, no AVX


def runs(binary, workdir):
    """The two runs of the probe: (name, argv) without the qemu prefix."""
    work = Path(workdir)
    (work / "lib").mkdir()
    (work / "s.json").write_text('{"manifest": []}\n')
    (work / "env.txt").write_text("OPENROUTER_API_KEY=dummy\n")
    hello = work / "hello"
    shutil.copytree(ROOT / "examples" / "hello", hello)
    return [
        ("apply", [binary, "--root", str(work / "root"), "--env", str(work / "env.txt"),
                   "--templates", str(work / "lib"), "--apply", str(work / "s.json"),
                   "--log-stderr", "off"]),
        ("validate", [binary, "--validate", "--root", str(hello), "--env", str(work / "env.txt")]),
    ]


def probe(binary, qemu, cpu, timeout_s=300):
    """[(name, returncode)] of every run, the binary under `qemu -cpu cpu`."""
    out = []
    with tempfile.TemporaryDirectory(prefix="cpu-floor-") as work:
        for name, argv in runs(str(binary), work):
            try:
                rc = subprocess.run([qemu, "-cpu", cpu] + argv, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    timeout=timeout_s).returncode
            except subprocess.TimeoutExpired:
                rc = "timeout"
            out.append((name, rc))
    return out


MUST_EXIT_0 = ("validate",)  # the run that proves the binary really ran


def red_run(name, rc):
    """True when the run is red: ended by a signal or a hang, or a run that
    has to exit 0 did not."""
    return signalled(rc) or (name in MUST_EXIT_0 and rc != 0)


def signalled(rc):
    """True when the run did not end by the binary's own exit: a signal (a
    negative code from Popen, or 128+n from a wrapper that re-raises) or a hang."""
    return rc == "timeout" or rc < 0 or rc > 128


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("binary")
    p.add_argument("--qemu", default="qemu-x86_64")
    p.add_argument("--cpu", default=FLOOR_CPU)
    a = p.parse_args(argv)
    if not Path(a.binary).is_file():
        print("cpu-floor: no binary at %s" % a.binary, file=sys.stderr)
        return 2
    qemu = shutil.which(a.qemu) or (a.qemu if Path(a.qemu).is_file() else None)
    if not qemu:
        print("cpu-floor: %s not found (apt-get install qemu-user)" % a.qemu, file=sys.stderr)
        return 2
    red = 0
    for name, rc in probe(a.binary, qemu, a.cpu):
        bad = red_run(name, rc)
        red += bad
        what = rc
        if isinstance(rc, int) and signalled(rc):
            n = -rc if rc < 0 else rc - 128
            try:
                what = "%s (%s)" % (rc, signal.Signals(n).name)
            except ValueError:
                pass
        print("CPU-FLOOR %s cpu=%s rc=%s %s" % (name, a.cpu, what, "RED" if bad else "GREEN"))
    print("CPU-FLOOR-SUMMARY %s %s" % (os.path.basename(a.binary), "RED" if red else "GREEN"))
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main())
