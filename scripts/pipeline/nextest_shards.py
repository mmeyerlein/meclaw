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

The first run on eight shards (CI of v0.77.1) took 178-290 s of test wall
for 175-234 s of plain seconds / 4: a test that reserves slots
(`threads-required` in `.config/nextest.toml`, the long-first group) holds
two of the four threads, and shard 3 ran 133 s at two tests at a time. The
table therefore counts slot-seconds (`time x threads-required`); their sum
/ 4 met every shard's wall within 9 s (290 -> 284 predicted). Ten shards on
that table (6906 slot-seconds) carry 683 each, the heaviest binary alone 756
(curator_1, a shard of its own): <= 189 s of wall, ~9.1 min for the job.

    nextest_shards.py filter <k>/<n>             the filterset of shard k
    nextest_shards.py durations [--out F] [--union OLD] [--nextest-config F] <junit.xml> ...
                                                 the table from nextest JUnit
    nextest_shards.py plan                       seconds and binaries per shard

TABLE
=====
`nextest_shard_durations.json` next to this file, `{binary_id: seconds}`: the
summed slot-seconds of every test case of a binary (`<testsuite name>` is the
binary id), over all the JUnit files given -- the CI uploads one per shard. A
test case weighs `time x threads-required`, read from the overrides of the
profiles `ci` and `default` of `.config/nextest.toml` (`--nextest-config`)
in the forms `binary_id(=B)`, `binary_id(=B) & test(=t)` and
`binary_id(=B) & (test(=a) + test(=b) ...)`, joined by `+`; a term of another
form weighs 1, as before. `--union OLD` keeps the binaries of the old table
the JUnit does not name (a binary without a test case in that run). It is
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
import tomllib
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
DURATIONS = os.path.join(HERE, "nextest_shard_durations.json")
NEXTEST_CONFIG = os.path.join(os.path.dirname(os.path.dirname(HERE)), ".config", "nextest.toml")
SHARDS = 10
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


# One `+`-term of an override filter that `slot_weights` reads (see TABLE).
_WEIGHT_TERM = re.compile(
    r"^\(?binary_id\(=([^)]+)\)(?: & \(?((?:test\(=[^)]+\)(?: \+ )?)+)\)?)?\)?$")


def slot_weights(config_path):
    """`{(binary, test or None): threads}` of every override with
    `threads-required` > 1 (first match wins, `ci` before `default`, as nextest
    reads them); {} without a readable config."""
    try:
        with open(config_path, "rb") as fh:
            cfg = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError):
        return {}
    out = {}
    for profile in ("ci", "default"):
        for o in (cfg.get("profile", {}).get(profile, {}) or {}).get("overrides", []) or []:
            n = o.get("threads-required")
            if not isinstance(n, int) or isinstance(n, bool) or n <= 1:
                continue
            text = str(o.get("filter") or "").strip()
            for part in re.split(r"\s*\+\s*(?=\(?binary_id)", text):
                m = _WEIGHT_TERM.match(part.strip())
                if not m:
                    continue
                for test in re.findall(r"test\(=([^)]+)\)", m.group(2) or "") or [None]:
                    out.setdefault((m.group(1), test), n)
    return out


def durations_from_junit(paths, weights=None):
    """`{binary_id: slot-seconds}` summed over the test cases of every file (0.1 s)."""
    weights = weights or {}
    secs = {}
    for path in paths:
        for suite in ET.parse(path).getroot().iter("testsuite"):
            name = suite.get("name") or ""
            _term(name)
            total = sum(float(tc.get("time") or 0)
                        * weights.get((name, tc.get("name")), weights.get((name, None), 1))
                        for tc in suite.iter("testcase"))
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
        return usage("usage: nextest_shards.py filter <k>/<n> | durations [--out F] [--union OLD] "
                     "[--nextest-config F] <junit> ... | plan")
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
        opts = {"--out": None, "--union": None, "--nextest-config": NEXTEST_CONFIG}
        while rest[:1] and rest[0] in opts:
            if len(rest) < 2:
                return usage("%s needs a value" % rest[0])
            opts[rest[0]], rest = rest[1], rest[2:]
        out = opts["--out"]
        if not rest:
            return usage("durations needs at least one JUnit file")
        try:
            table = durations_from_junit(rest, slot_weights(opts["--nextest-config"]))
        except (OSError, ET.ParseError, ValueError) as exc:
            return usage(str(exc))
        if opts["--union"]:
            old = load(opts["--union"])
            if not old:
                return usage("no table at %s" % opts["--union"])
            table = dict(sorted({**old, **table}.items()))
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
