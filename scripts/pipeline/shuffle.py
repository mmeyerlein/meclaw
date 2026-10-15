"""T3 of the flake triage (OR-TV-2, VG TV-F2): one unittest module in a random order.

    python3 scripts/pipeline/shuffle.py --seed <n> <module> [<name> ...]

More names (modules, classes, test ids) are shuffled as one suite --
`scripts/flake_triage.py --unittest` runs the tests it triages that way.
Every test's duration is printed (`--durations` of unittest, all of them).

The seed is on the command line and in the first output line, so a red
order can be replayed exactly. The output ends in unittest's own summary
(`Ran N tests` / `OK` / `FAILED (...)`), which the triage reads.
"""

import argparse
import os
import random
import sys
import unittest


def flatten(suite):
    for t in suite:
        if isinstance(t, unittest.TestSuite):
            yield from flatten(t)
        else:
            yield t


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("module", nargs="+")
    args = ap.parse_args(argv)
    sys.path.insert(0, os.getcwd())
    print("SHUFFLE-SEED %d %s" % (args.seed, " ".join(args.module)), file=sys.stderr, flush=True)
    tests = list(flatten(unittest.defaultTestLoader.loadTestsFromNames(args.module)))
    random.Random(args.seed).shuffle(tests)
    result = unittest.TextTestRunner(verbosity=2, durations=0).run(unittest.TestSuite(tests))
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
