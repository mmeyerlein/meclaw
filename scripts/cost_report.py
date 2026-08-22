#!/usr/bin/env python3
"""Aggregate the provider cost of a running meclaw colony from its message log.

The colony writes one row per delivered message into `colony.db`. Every message
emitted by an `llm` cell carries the provider's token accounting in its
`headers.hop` object -- `model`, `tokens_prompt`, `tokens_completion`. That is
the whole data source: no extra instrumentation, no provider dashboard, no API
call. This script reads those rows (strictly read-only), groups them per day and
per model, and multiplies them by a dated price list you pass in.

The price list is a separate JSON file on purpose. Prices change; a number
hard-coded in a script is a number nobody can date. `scripts/prices-*.json`
ships one dated snapshot -- copy it, edit it, pass your own.

Usage
-----
    python3 scripts/cost_report.py --db /path/to/colony.db \
                                   --prices scripts/prices-openrouter-2026-08-22.json

    # narrow the window, break the total down per cell, emit JSON
    python3 scripts/cost_report.py --db colony.db --prices prices.json \
            --from 2026-08-14 --to 2026-08-15 --by-cell --json

The database is opened through the SQLite URI `file:<path>?mode=ro`. The script
never writes, never issues a network request, and never prints message content
-- only the `hop` accounting fields and the cell paths (the latter only with
`--by-cell`).
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from collections import defaultdict
from datetime import datetime, timezone

# Rows whose `hop` object carries neither token count are not provider calls
# (store hops, code hops, routing hops). They are counted and reported as
# "skipped", never silently dropped.
TOKEN_FIELDS = ("tokens_prompt", "tokens_completion")


class PriceList:
    """A dated price snapshot: USD per one million tokens, per model id."""

    def __init__(self, doc: dict) -> None:
        self.source = doc.get("source", "<unknown>")
        self.retrieved = doc.get("retrieved", "<undated>")
        self.currency = doc.get("currency", "USD")
        self.models = doc.get("models", {})
        # Optional: attribute a model to cells that do not report one. An
        # `embed` cell is typically a `code` cell calling an embedding endpoint
        # -- it reports `tokens_prompt` but no `model`, because the model id
        # lives in its own payload, not in the substrate's hop header.
        self.fallbacks = doc.get("fallback_models", [])
        self.fx = doc.get("fx", {})

    def model_for(self, hop_model: str | None, from_path: str) -> str:
        if hop_model:
            return hop_model
        for rule in self.fallbacks:
            suffix = rule.get("from_path_suffix")
            if suffix and from_path.endswith(suffix):
                return rule["model"]
        return "unknown"

    def cost_usd(self, model: str, prompt: int, completion: int) -> float | None:
        entry = self.models.get(model)
        if entry is None:
            return None
        return prompt / 1e6 * entry.get("input", 0.0) + completion / 1e6 * entry.get(
            "output", 0.0
        )


def load_rows(db_path: str, ts_from: int | None, ts_to: int | None):
    """Yield `(created_at, from_path, hop)` for every message log row.

    Opened read-only through a SQLite URI. Only three columns are selected;
    `body_payload` -- which holds the actual conversation -- is never read.
    """
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        sql = "SELECT created_at, from_path, headers FROM message_log"
        clauses, params = [], []
        if ts_from is not None:
            clauses.append("created_at >= ?")
            params.append(ts_from)
        if ts_to is not None:
            clauses.append("created_at < ?")
            params.append(ts_to)
        if clauses:
            sql += " WHERE " + " AND ".join(clauses)
        for created_at, from_path, headers in conn.execute(sql, params):
            try:
                hop = json.loads(headers or "{}").get("hop") or {}
            except (json.JSONDecodeError, AttributeError):
                continue
            yield created_at, from_path, hop
    finally:
        conn.close()


def day_of(unix_seconds: int) -> str:
    return datetime.fromtimestamp(unix_seconds, timezone.utc).strftime("%Y-%m-%d")


def parse_bound(text: str) -> tuple[int, bool]:
    """Parse a window bound. Returns `(unix_seconds, is_whole_day)`.

    Accepts `YYYY-MM-DD` (a whole UTC day) and `YYYY-MM-DDTHH:MM` / `... HH:MM`
    (a UTC instant). The whole-day flag matters only for `--to`, which is
    inclusive for a date and exclusive for an instant.
    """
    for fmt, whole_day in (
        ("%Y-%m-%d", True),
        ("%Y-%m-%dT%H:%M", False),
        ("%Y-%m-%d %H:%M", False),
    ):
        try:
            parsed = datetime.strptime(text, fmt)
        except ValueError:
            continue
        return int(parsed.replace(tzinfo=timezone.utc).timestamp()), whole_day
    raise argparse.ArgumentTypeError(
        f"{text!r}: expected YYYY-MM-DD or YYYY-MM-DDTHH:MM (UTC)"
    )


def token_counts(hop: dict) -> tuple[int, int] | None:
    """The `(prompt, completion)` counts of a hop, or `None` if unreadable.

    The `hop` object is not a schema the substrate enforces -- a `code` cell's
    script writes whatever headers it likes, and a provider adapter can hand
    back a string, a float, `null`, or a nested object under a name this script
    expects to be a number. Bare `int(...)` turned each of those into a
    traceback that killed the whole report, throwing away every row that had
    already been read correctly.

    An unreadable count is therefore treated as what it is: a row that is not a
    priced provider call, folded into the same "skipped" tally as a hop with no
    token fields at all. Wrong-but-plausible arithmetic on a coerced value would
    be worse -- a cost report that silently under-reports is not a cost report.
    """
    try:
        return (
            int(hop.get("tokens_prompt", 0) or 0),
            int(hop.get("tokens_completion", 0) or 0),
        )
    except (TypeError, ValueError):
        return None


def collect(rows, prices: PriceList):
    """Fold the rows into per-(day, model) buckets plus window bookkeeping."""
    per_day_model: dict[tuple[str, str], list[int]] = defaultdict(lambda: [0, 0, 0])
    per_cell: dict[tuple[str, str], list[int]] = defaultdict(lambda: [0, 0, 0])
    first = last = None
    total_rows = priced_rows = 0

    for created_at, from_path, hop in rows:
        total_rows += 1
        first = created_at if first is None else min(first, created_at)
        last = created_at if last is None else max(last, created_at)
        if not any(f in hop for f in TOKEN_FIELDS):
            continue
        counts = token_counts(hop)
        if counts is None:
            # Skipped, exactly like a hop carrying no token fields at all.
            continue
        prompt, completion = counts
        priced_rows += 1
        model = prices.model_for(hop.get("model"), from_path)
        for bucket, key in (
            (per_day_model, (day_of(created_at), model)),
            (per_cell, (from_path, model)),
        ):
            acc = bucket[key]
            acc[0] += 1
            acc[1] += prompt
            acc[2] += completion

    return per_day_model, per_cell, first, last, total_rows, priced_rows


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Aggregate provider cost from a colony's message log."
    )
    ap.add_argument("--db", required=True, help="path to colony.db (read-only)")
    ap.add_argument("--prices", required=True, help="path to a dated price JSON")
    ap.add_argument(
        "--from",
        dest="day_from",
        help="window start, UTC: YYYY-MM-DD or YYYY-MM-DDTHH:MM",
    )
    ap.add_argument(
        "--to",
        dest="day_to",
        help="window end, UTC: YYYY-MM-DD (inclusive) or YYYY-MM-DDTHH:MM (exclusive)",
    )
    ap.add_argument("--by-cell", action="store_true", help="also break down per cell")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    args = ap.parse_args()

    with open(args.prices, encoding="utf-8") as fh:
        prices = PriceList(json.load(fh))

    ts_from = parse_bound(args.day_from)[0] if args.day_from else None
    ts_to = None
    if args.day_to:
        bound, whole_day = parse_bound(args.day_to)
        ts_to = bound + 86400 if whole_day else bound

    per_day_model, per_cell, first, last, total_rows, priced_rows = collect(
        load_rows(args.db, ts_from, ts_to), prices
    )

    if first is None:
        print("no rows in the selected window", file=sys.stderr)
        return 1

    days: dict[str, dict] = {}
    unpriced: set[str] = set()
    grand = 0.0
    for (day, model), (calls, prompt, completion) in sorted(per_day_model.items()):
        cost = prices.cost_usd(model, prompt, completion)
        if cost is None:
            unpriced.add(model)
        else:
            grand += cost
        entry = days.setdefault(day, {"models": {}, "cost": 0.0})
        entry["models"][model] = {
            "calls": calls,
            "tokens_prompt": prompt,
            "tokens_completion": completion,
            "cost_usd": cost,
        }
        entry["cost"] += cost or 0.0

    # The extrapolation window is the one that was ASKED for, not the one that
    # happened to contain rows. A quiet night whose last message lands at 03:05
    # spans until 08:00 all the same, and dividing by the observed span instead
    # would silently inflate the per-day rate.
    win_lo = ts_from if ts_from is not None else first
    win_hi = ts_to if ts_to is not None else last
    window_hours = (win_hi - win_lo) / 3600 or 1e-9
    per_24h = grand / window_hours * 24

    report = {
        "price_source": prices.source,
        "price_retrieved": prices.retrieved,
        "window_utc": [
            datetime.fromtimestamp(win_lo, timezone.utc).isoformat(),
            datetime.fromtimestamp(win_hi, timezone.utc).isoformat(),
        ],
        "window_bounds": "requested" if (ts_from or ts_to) else "observed",
        "window_hours": round(window_hours, 2),
        "rows_total": total_rows,
        "rows_with_tokens": priced_rows,
        "days": days,
        "total_usd": grand,
        "usd_per_24h": per_24h,
        "unpriced_models": sorted(unpriced),
    }
    for code, fx in prices.fx.items():
        rate = fx.get("usd_per_unit")
        if rate:
            report[f"total_{code.lower()}"] = grand / rate
            report[f"{code.lower()}_per_24h"] = per_24h / rate

    if args.json:
        if args.by_cell:
            report["cells"] = {
                f"{path}\t{model}": {
                    "calls": c,
                    "tokens_prompt": p,
                    "tokens_completion": k,
                    "cost_usd": prices.cost_usd(model, p, k),
                }
                for (path, model), (c, p, k) in sorted(per_cell.items())
            }
        print(json.dumps(report, indent=2))
        return 0

    print(f"prices : {prices.source} (retrieved {prices.retrieved})")
    print(
        f"window : {report['window_utc'][0]} .. {report['window_utc'][1]} UTC "
        f"({report['window_hours']} h, {report['window_bounds']})"
    )
    print(f"rows   : {total_rows} logged, {priced_rows} carry token counts")
    print()
    head = f"{'day':<12}{'model':<34}{'calls':>7}{'prompt':>12}{'completion':>12}{'USD':>12}"
    print(head)
    print("-" * len(head))
    for day in sorted(days):
        for model, m in sorted(days[day]["models"].items()):
            cost = "n/a" if m["cost_usd"] is None else f"{m['cost_usd']:.5f}"
            print(
                f"{day:<12}{model:<34}{m['calls']:>7}{m['tokens_prompt']:>12}"
                f"{m['tokens_completion']:>12}{cost:>12}"
            )
        print(f"{'':<12}{'= day total':<34}{'':>7}{'':>12}{'':>12}{days[day]['cost']:>12.5f}")
    print("-" * len(head))
    print(f"total  : USD {grand:.5f} over {report['window_hours']} h")
    print(f"per 24h: USD {per_24h:.5f}")
    for code, fx in prices.fx.items():
        rate = fx.get("usd_per_unit")
        if rate:
            print(
                f"per 24h: {code} {per_24h / rate:.5f}  "
                f"(at {rate} USD/{code}, {fx.get('source', '?')} {fx.get('date', '?')})"
            )
    if unpriced:
        print()
        print(
            "WARNING: no price entry for "
            + ", ".join(unpriced)
            + " -- their tokens are listed above but excluded from the total."
        )

    if args.by_cell:
        print()
        print("per cell:")
        for (path, model), (calls, prompt, completion) in sorted(per_cell.items()):
            cost = prices.cost_usd(model, prompt, completion)
            shown = "n/a" if cost is None else f"{cost:.5f}"
            print(f"  {path}  [{model}]  calls={calls}  USD={shown}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
