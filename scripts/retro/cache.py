"""Q11: the calls that came back after the short prompt cache had expired.

A prompt cache entry lives five minutes, or an hour when it is written as a
one-hour entry. An agent that waits longer than five minutes -- for a gate, a
cargo lock, a review -- comes back to a cold cache and writes its whole
context again. Measured over the 87 agents of one wave: 2.2 % of the calls
came after a 5-60 minute pause, and they wrote 57.7 % of all cache tokens;
not one token went into the one-hour cache (the wave's lesson on pipeline
pace, and the planner's probe over its `subagents/`).

The number is a FINDING without a threshold: what a one-hour cache would
save depends on prices, and prices change -- they stay out of a public file.
The counterfactual with prices is a private tool beside the measurements.
"""

from __future__ import annotations

SHORT_TTL_S, LONG_TTL_S = 300, 3600   # the 5-minute and the 1-hour prompt cache


def pauses(agents):
    """Calls that came back after the short cache expired, and what they rewrote.

    One chain per agent AND model: each model keeps its own cache, so a small
    model called every two minutes does not keep the big one's cache warm.
    The pause runs from start to start of two calls (`scan` keeps the first
    line of each call). Returns None when the agents wrote nothing to a cache.
    """
    calls = late = writes = late_writes = long_writes = 0
    for agent in agents:
        last = {}
        for c in agent.get("calls", ()):
            calls += 1
            writes += c["cc"]
            long_writes += c["cc1h"]
            prev = last.get(c["model"])
            if prev is not None and SHORT_TTL_S < (c["t0"] - prev).total_seconds() <= LONG_TTL_S:
                late += 1
                late_writes += c["cc"]
            last[c["model"]] = c["t0"]
    if not calls or not writes:
        return None
    return {"calls": calls, "late": late, "writes": writes,
            "late_writes": late_writes, "long_writes": long_writes}
