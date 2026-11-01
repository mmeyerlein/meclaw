#!/usr/bin/python3 -I
"""meclaw -- the test shards of the public CI, cut by run time.

WHY THIS EXISTS
===============
The `test` job of `.github/workflows/ci.yml` ran the suite as
`--partition count:k/3`: nextest's count partition deals out tests, not run
time, and the three shards of one run took 11:28 / 15:50 / 16:49 (test wall
638 / 592 / 425 s after a ~5 min build each). Their JUnit files hold 227
binaries and 6159 s of test time, the largest binary 512 s, the longest single
test 40 s. Cut by binary and by the measured seconds, eight shards carry
~770 s each, i.e. ~210 s of wall time at four test threads -- with the build
(~5:20) and the setup (~35 s) about 9.4 min for the longest job.

    nextest_shards.py filter <k>/<n>             the filterset of shard k
    nextest_shards.py durations [--out F] <junit.xml> ...
                                                 the table from nextest JUnit
    nextest_shards.py plan                       seconds and binaries per shard

TABLE
=====
`nextest_shard_durations.json` next to this file, `{binary_id: seconds}`: the
summed `time` of every test case of a binary (`<testsuite name>` is the binary
id), over all the JUnit files given -- the CI uploads one per shard. It is
written by `durations`, never by hand.

DISTRIBUTION
============
Deterministic: longest binary first onto the least loaded shard (ties: the
lower shard; equal seconds: by name). A shard's filterset is the union of
`binary_id(=<id>)` over its binaries. A binary the table does not know (a new
test target, one without tests) is not lost: the shard with the smallest sum
(ties: the lower one) also selects `not (<every known binary>)`. So the union
of all filters is `all()` and no two shards share a binary, by construction,
with no binary list needed at run time. A shard without a binary selects
`none()`. `scripts/test-tier.sh` ANDs the filter of `MECLAW_TIER_SHARD=<k>/<n>`
with its tier's own.

`<n>` must be `SHARDS`: the matrix of the workflow has exactly that many jobs
(lock: `scripts/tests/test_pipeline_nextest_shards.py`), and a filter asked for
another count would cut the suite into pieces no job runs. Exit 0, 2 usage.
"""

import json
import os
import re
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
DURATIONS = os.path.join(HERE, "nextest_shard_durations.json")
SHARDS = 8
# A binary id the filterset carries verbatim inside `binary_id(=...)`.
_NAME = re.compile(r"^[A-Za-z0-9_.:/-]+$")


def usage(msg):
    print("nextest_shards: %s" % msg, file=sys.stderr)
    return 2


def load(path):
    """`{binary_id: seconds}` of the table ({} when missing or unreadable)."""
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {str(k): float(v) for k, v in data.items()
            if isinstance(v, (int, float)) and not isinstance(v, bool)}


def assign(table, shards):
    """`[[binary, ...], ...]`: `shards` lists (LPT, see DISTRIBUTION), each sorted."""
    out = [[] for _ in range(max(1, int(shards)))]
    load_ = [0.0] * len(out)
    for b in sorted(table, key=lambda b: (-table[b], b)):
        k = min(range(len(out)), key=lambda i: (load_[i], i))
        out[k].append(b)
        load_[k] += table[b]
    for lst in out:
        lst.sort()
    return out


def unknown_shard(table, shards):
    """The 0-based shard that also selects every binary the table does not know."""
    sums = [sum(table[b] for b in s) for s in assign(table, shards)]
    return min(range(len(sums)), key=lambda i: (sums[i], i))


def _term(binary):
    if not _NAME.match(binary):
        raise ValueError("binary id %r cannot stand in a filterset" % binary)
    return "binary_id(=%s)" % binary


def filters(table, shards):
    """The nextest filterset of every shard (index 0 = shard 1)."""
    parts = assign(table, shards)
    rest = unknown_shard(table, shards)
    known = " | ".join(_term(b) for b in sorted(table))
    out = []
    for k, binaries in enumerate(parts):
        terms = [_term(b) for b in binaries]
        if k == rest:
            terms.append("not (%s)" % known if known else "all()")
        out.append(" | ".join(terms) if terms else "none()")
    return out


def durations_from_junit(paths):
    """`{binary_id: seconds}` summed over the test cases of every file (0.1 s)."""
    secs = {}
    for path in paths:
        for suite in ET.parse(path).getroot().iter("testsuite"):
            name = suite.get("name") or ""
            _term(name)
            total = sum(float(tc.get("time") or 0) for tc in suite.iter("testcase"))
            secs[name] = secs.get(name, 0.0) + total
    return {k: round(v, 1) for k, v in sorted(secs.items())}


def _parse_part(text):
    m = re.fullmatch(r"(\d+)/(\d+)", text or "")
    if not m:
        raise ValueError("want <k>/<n>, got %r" % text)
    k, n = int(m.group(1)), int(m.group(2))
    if n != SHARDS:
        raise ValueError("%d shards asked, the table is cut into %d (SHARDS)" % (n, SHARDS))
    if not 1 <= k <= n:
        raise ValueError("shard %d is not in 1..%d" % (k, n))
    return k, n


def main(argv):
    if not argv:
        return usage("usage: nextest_shards.py filter <k>/<n> | durations [--out F] <junit> ... | plan")
    verb, rest = argv[0], argv[1:]
    if verb == "filter":
        if len(rest) != 1:
            return usage("filter takes one <k>/<n>")
        try:
            k, n = _parse_part(rest[0])
            table = load(DURATIONS)
            if not table:
                return usage("no table at %s" % DURATIONS)
            print(filters(table, n)[k - 1])
        except ValueError as exc:
            return usage(str(exc))
        return 0
    if verb == "durations":
        out = None
        if rest[:1] == ["--out"]:
            if len(rest) < 2:
                return usage("--out needs a value")
            out, rest = rest[1], rest[2:]
        if not rest:
            return usage("durations needs at least one JUnit file")
        try:
            table = durations_from_junit(rest)
        except (OSError, ET.ParseError, ValueError) as exc:
            return usage(str(exc))
        text = json.dumps(table, indent=1, sort_keys=True) + "\n"
        if out:
            with open(out, "w", encoding="utf-8") as fh:
                fh.write(text)
        else:
            sys.stdout.write(text)
        return 0
    if verb == "plan":
        table = load(DURATIONS)
        rest_k = unknown_shard(table, SHARDS)
        for k, binaries in enumerate(assign(table, SHARDS), 1):
            print("SHARD %d/%d binaries=%d secs=%.0f%s" % (
                k, SHARDS, len(binaries), sum(table[b] for b in binaries),
                " +unknown" if k - 1 == rest_k else ""))
        print("SHARD-PLAN shards=%d binaries=%d secs=%.0f"
              % (SHARDS, len(table), sum(table.values())))
        return 0
    return usage("unknown verb %s" % verb)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
