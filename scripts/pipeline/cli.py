"""`scripts/pipeline.py <cmd>` -- the argument parser of the pipeline core.

Every module hooks its commands in with `register(sub)`; this file only holds
the list, one line per module (additive -- each strand adds its own line).
"""

import argparse
import importlib
import sys

# One line per module that brings commands (V0 section 1).
MODULES = (
    "store",    # store init|check
    "receipt",  # receipt <run>
    "metrics",  # metrics push --since <at> (and the events hook)
    "backfill",  # backfill [--root] [--since] [--until]
    "grafana_push",  # grafana push
    "pre",      # pre (W4)
    "locks",    # pins --check (W4) / --cascade (W9)
    "retro",    # retro, finding resolve|list (W11)
    "night",    # night (W11)
    "orga_export",  # export-orga (W11)
    "wave",     # wave close (W11)
    "executor",  # exec (W3)
    "schedule",  # lanes (W3)
    "gate",      # gate, test (W3)
    "measure",  # measure minimum (M0)
    "merge",    # merge [--strand|--batch], close (W9)
    "revert",   # revert <merge|strand> (W9)
    "flakes",   # flakes list|show|rate|quarantine|release|issue (W8)
    "graph",    # graph build|dependents|deps|path|stats|broken (W6)
    "impact",   # impact --diff|--replay (W6)
    "coverage",  # coverage build|aggregate|stale|journal (W6)
    "register",  # register plan --repo <satellite> (W10)
)


def parser():
    p = argparse.ArgumentParser(
        prog="pipeline", description="meclaw pipeline core",
        epilog="-C <dir> before the command: run as if started in <dir> (as git -C; "
               "scripts/pipeline.py takes it)")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in MODULES:
        importlib.import_module(f".{name}", __package__).register(sub)
    return p


def main(argv=None):
    args = parser().parse_args(sys.argv[1:] if argv is None else argv)
    return args.func(args)
