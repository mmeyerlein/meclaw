#!/usr/bin/env python3
"""How long does a lane actually take? Read it out of the colony's own log.

A colony logs every delivered hop with its instant, so the answer to "how slow
is this" is already on disk. This asks it — read-only, no model call, no cost,
and no instrumentation to add first.

    scripts/trace_latency.py <colony-root> --lane brain_fast --lane brain
    scripts/trace_latency.py <colony-root> --lane recall --breakdown

Each `--lane` is matched against the LAST path segment of either endpoint of a
hop. A trace is attributed to the first lane that appears in it, so listing the
narrower lane first (`brain_fast` before `brain`) keeps the two apart.

What the numbers mean, exactly, because a latency figure with a fuzzy
definition is worse than none:

- **A trace's duration** is the instant of its last logged hop minus its first.
  That is the span the colony was working on this errand — it is not the
  user-visible round trip, which also carries whatever the channel does before
  the first hop and after the last (a Telegram poll interval, for instance).
  It is a lower bound on what somebody waited, and an honest one.
- **`--breakdown`** attributes the gaps between consecutive hops to the leg
  that closed them, which is where a chain reveals whether it is slow in one
  place or slow everywhere. A leg that shows up with a large total and a small
  maximum is the second kind, and no single fix will help it.
- A trace with **one** hop has a duration of zero and is counted. It is not
  noise: it is an errand the colony finished inside one hop.

Percentiles are nearest-rank on the sorted sample. With a handful of traces
they are a shape, not a statistic, so `n` is printed next to every one of them.
"""
import argparse
import collections
import pathlib
import sqlite3
import sys


def load(db):
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    rows = con.execute(
        "select trace_id, from_path, to_path, created_at from message_log "
        "order by created_at, id"
    ).fetchall()
    con.close()
    return rows


def leaf(path):
    return path.rsplit("/", 1)[-1]


def percentile(sorted_xs, q):
    if not sorted_xs:
        return None
    k = max(1, min(len(sorted_xs), round(q * len(sorted_xs))))
    return sorted_xs[k - 1]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root", help="colony root (the directory holding colony.db)")
    ap.add_argument("--lane", action="append", default=[],
                    help="cell name to group traces by; repeatable, first match wins")
    ap.add_argument("--breakdown", action="store_true",
                    help="also show which legs the time sits in")
    ap.add_argument("--top", type=int, default=8, help="legs to show in the breakdown")
    args = ap.parse_args()

    db = pathlib.Path(args.root) / "colony.db"
    if not db.exists():
        sys.exit(f"no colony.db under {args.root}")
    rows = load(db)
    if not rows:
        sys.exit("the message log is empty — nothing has been routed yet")

    traces = collections.defaultdict(list)
    for tid, frm, to, at in rows:
        traces[tid].append((at, leaf(frm), leaf(to)))

    lanes = args.lane or ["*"]
    buckets = collections.defaultdict(list)
    for tid, hops in traces.items():
        names = {n for _, a, b in hops for n in (a, b)}
        for lane in lanes:
            if lane == "*" or lane in names:
                buckets[lane].append(hops)
                break

    print(f"{len(traces)} traces in the log, {len(rows)} hops\n")
    for lane in lanes:
        hop_lists = buckets.get(lane, [])
        if not hop_lists:
            print(f"{lane}: no trace touches it")
            continue
        durations = sorted(h[-1][0] - h[0][0] for h in hop_lists)
        n = len(durations)
        print(
            f"{lane}: n={n}  min={durations[0]}s  "
            f"p50={percentile(durations, 0.5)}s  "
            f"p95={percentile(durations, 0.95)}s  max={durations[-1]}s"
        )
        if not args.breakdown:
            continue
        worst = collections.Counter()
        total = collections.Counter()
        for hops in hop_lists:
            for i in range(1, len(hops)):
                gap = hops[i][0] - hops[i - 1][0]
                if gap <= 0:
                    continue
                leg = f"{hops[i - 1][2]} -> {hops[i][2]}"
                worst[leg] = max(worst[leg], gap)
                total[leg] += gap
        if not worst:
            print("    every hop landed inside the same second")
            continue
        print("    slowest single gap per leg (total across all traces):")
        for leg, w in worst.most_common(args.top):
            print(f"      {w:4}s  (total {total[leg]:4}s)  {leg}")
        print()


main()
