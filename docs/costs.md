# What a colony costs to run

> This method run against a four-call colony, with the real output:
> [`../examples/never-forgets/WALKTHROUGH.md`](../examples/never-forgets/WALKTHROUGH.md) § *Step 8 — what it cost*.

A colony that runs around the clock spends money in exactly one place: the
provider calls its `llm` cells make. This document describes how to measure that
spend from a colony's own database, gives the numbers measured on one production
colony, and states plainly which tiers have not been measured yet.

The method is the point. Every number below can be re-derived with
[`scripts/cost_report.py`](../scripts/cost_report.py) against your own colony, or
re-implemented from scratch in about twenty lines using the description in
[Method](#method). If a number here disagrees with what your provider bills you,
the provider is right and this document is wrong.

## What is measured, and what is not

Measured: provider tokens, priced from a public price list.

Not measured, and not included in any number below:

- **Electricity and hardware.** A local model on your own GPU costs no provider
  tokens; it costs power and a card. That is a real cost and this method does
  not see it.
- **The machine the colony runs on.** meclaw is one Rust binary; the substrate
  itself is cheap enough that it has never been the interesting term. It is
  still not zero.
- **Prompt-cache discounts and cache-write surcharges.** The substrate records
  the token counts the provider returns; it does not record whether a prompt was
  served from cache. Where caching is active, the real bill is lower than the
  number computed here.

## Method

### 1. The data source

The colony logs one row per delivered message into `colony.db`, table
`message_log`. An `llm` cell writes the provider's token accounting into the
message it emits, under the `hop` object of the `headers` column:

| field | meaning |
|---|---|
| `hop.model` | the model id the provider actually served |
| `hop.tokens_prompt` | prompt tokens the provider counted |
| `hop.tokens_completion` | completion tokens the provider counted |

That is the entire instrumentation. Nothing has to be switched on, no metrics
sidecar has to run, and the numbers come from the provider's own response rather
than from a token estimate computed locally.

Rows whose `hop` carries neither token field are not provider calls (store hops,
code hops, routing hops) and are skipped.

**This is the named contract of the measurement**, and it is deliberately small —
six things, and nothing else in the schema is load-bearing for a cost report:

| what | why it is read |
|---|---|
| `message_log.created_at` | the day and the window bound |
| `message_log.from_path` | which cell made the call (per-cell breakdown, `fallback_models` attribution) |
| `message_log.headers` | the JSON that carries the `hop` object |
| `headers.hop.model` | which model was served |
| `headers.hop.tokens_prompt` | prompt tokens the provider counted |
| `headers.hop.tokens_completion` | completion tokens the provider counted |

Any reader that touches those six can compute the same numbers. `body_payload`,
where the conversation itself lives, is never read.

**The `headers` column has changed shape once, silently.** Since the
two-compartment header model, `headers` is `{"context": {…}, "hop": {…}}`; rows
written before it carry a flat JSON object with no `hop` key at all. A
long-lived `colony.db` therefore holds **both** forms, and nothing migrated the
old rows — the no-delete policy means they stay as they were written. A reader
has to tolerate both, and the failure mode matters: a flat row yields no `hop`,
so it is skipped as "not a provider call" rather than reported as a parse error.
A cost report over a window that reaches back into the old format will
under-count in silence, which is why `cost_report.py` prints the number of rows
it scanned next to the number it priced.

### 2. Attributing a model to cells that do not report one

Some cells reach a provider without being an `llm` cell — an `embed` cell is
typically a `code` cell calling an embedding endpoint. Such a cell reports
`tokens_prompt` but no `hop.model`, because the model id lives in its own
payload rather than in the substrate's hop header.

Those tokens are attributed by cell path, through a `fallback_models` rule in
the price file. Tokens that stay unattributed land in an `unknown` bucket, are
printed, and are excluded from the total — they are never silently folded in at
a guessed price.

### 3. The formula

Group the rows by UTC day and by model, sum the two token columns, then:

```
cost(model) = tokens_prompt      / 1e6 * price_input(model)
            + tokens_completion  / 1e6 * price_output(model)
```

Sum over models for a day total. To express a partial window as a daily rate,
divide by the window length in hours and multiply by 24. Use the window you
asked for, not the span between the first and last row that happened to fall
inside it — otherwise a quiet night whose last message lands at 03:05 is scored
as if it had ended there, and the daily rate comes out too high.

### 4. The prices

Prices live in a dated JSON file, not in the code, so that every number can be
tied to a price list and a date. The snapshot used below is
[`scripts/prices-openrouter-2026-08-15.json`](../scripts/prices-openrouter-2026-08-15.json),
retrieved from `https://openrouter.ai/api/v1/models` on 2026-08-15:

| model | input, USD / 1M tokens | output, USD / 1M tokens |
|---|---|---|
| `openai/gpt-5.6-luna` | 0.10 | 0.60 |
| `anthropic/claude-opus-5` | 5.00 | 25.00 |
| `qwen/qwen3-embedding-8b` | 0.01 | — |

**Embedding models are not in that listing.** `https://openrouter.ai/api/v1/models`
answers with chat models only; the embeddings endpoint is a separate API and
carries no price list, so an embedding figure is always *measured* — one call,
its billed amount divided by its tokens — and dated like any other measurement:

| model | input, USD / 1M tokens | measured |
|---|---|---|
| `qwen/qwen3-embedding-8b` | 0.01 | 2026-08-08, 5 tokens |
| `google/gemini-embedding-2` | 0.20 | 2026-08-22, 4 tokens |

`google/gemini-embedding-2` is what `memory-hive` ships as of 2.3.0 — twenty
times the price of the generation before it, and still the smallest line on
every bill below, because an embedding call bills the prompt side of a few dozen
tokens where a synthesis call bills thousands on both sides. The qwen row stays
because the figures further down were measured against it.

Where euro figures are given, they use the ECB euro reference exchange rate of
2026-08-14, 1 EUR = 1.1567 USD.

**The date is part of the measurement, not decoration.** A price list belongs to
the day it was retrieved, and a number computed from it is only reproducible
against that file. Newer lists therefore land **beside** the old ones, never on
top of them: a new `scripts/prices-openrouter-<date>.json`, a new row, the old
figure left standing with its own file next to it. Overwriting a price file
would silently rewrite every number ever derived from it, and nobody would see
the edit.

**The current list is**
[`scripts/prices-openrouter-2026-08-22.json`](../scripts/prices-openrouter-2026-08-22.json),
retrieved on 2026-08-22. Point a colony you are measuring *today* at that one;
the figures further down keep the 2026-08-15 list, because a number is only
reproducible against the list it was computed from. Two things moved between the
two:

| model | input, USD / 1M tokens | output, USD / 1M tokens | vs. 2026-08-15 |
|---|---|---|---|
| `openai/gpt-5.6-luna` | 0.20 | 1.20 | doubled |
| `anthropic/claude-opus-5` | 5.00 | 25.00 | unchanged |
| `google/gemini-embedding-2` | 0.20 | — | new; the shipped generation since 2026-08-19 |

The `qwen/qwen3-embedding-8b` row is carried forward into the new list rather
than dropped, so that a window spanning the 2026-08-19 switch prices both
generations instead of pushing the older half into the `unknown` bucket. One
thing the flat two-number format does not express: `openai/gpt-5.6-luna` bills
prompts above 272,000 tokens at a higher tier (0.40 / 1.80), so a colony that
routinely sends contexts that large is priced *low* by this report.

### 5. Running it

```sh
python3 scripts/cost_report.py \
    --db     /path/to/your/colony.db \
    --prices scripts/prices-openrouter-2026-08-22.json
```

The database is opened read-only through the SQLite URI `file:<path>?mode=ro`,
so it is safe to point at a colony that is currently running. The script reads
three columns — `created_at`, `from_path`, `headers` — and never touches
`body_payload`, where the conversation itself lives. Useful flags: `--from` and
`--to` to bound the window (a date, or a `YYYY-MM-DDTHH:MM` UTC instant),
`--by-cell` for a per-cell breakdown, `--json` for machine-readable output.

## Measured: the M tier

The M tier runs the small model locally or cheaply and reserves a frontier model
for the hard calls. The colony measured here is a personal assistant that runs
24/7: a conversational brain on `gpt-5.6-luna`, a reasoning brain on
`claude-opus-5`, a memory hive whose extractor and dreamer run on the small model
and whose judge runs on the frontier model, and an embedding lane.

Measured on one production colony over a 27.27 h window, 2026-08-14 09:19 UTC to
2026-08-15 12:35 UTC — 110 provider calls out of 6,209 logged messages. The
window is pinned explicitly, because a running colony keeps appending and an
unbounded re-run would not reproduce the same figure:

```sh
python3 scripts/cost_report.py --db colony.db \
    --prices scripts/prices-openrouter-2026-08-15.json \
    --from 2026-08-14T09:19 --to 2026-08-15T12:35
```

| window | length | provider calls | USD total | USD / 24 h |
|---|---|---|---|---|
| full observation window | 27.27 h | 110 | 0.414 | **0.364** |
| unattended overnight (`--from 2026-08-14T17:00 --to 2026-08-15T08:00`) | 15.00 h | 7 | 0.018 | **0.028** |

The unattended row was measured with the **v0.8.0 gate defaults** of the memory
lane — `MEMORY_BATCH_TOKENS=512` and `MEMORY_BATCH_MAX_AGE_MIN=30`, the two knobs
that decide how often an idle colony opens an extraction round. Those defaults
changed in 0.9.0 to `128` / `2`
([#51](https://github.com/mmeyerlein/meclaw/issues/51)), so the
overnight figure is pinned to the configuration above and not to a version: a
colony on the new defaults will produce a different number from the same traffic.
Re-run the command to get yours.

Per model, over the full window:

| model | calls | prompt tokens | completion tokens | USD |
|---|---|---|---|---|
| `anthropic/claude-opus-5` | 26 | 50,999 | 5,784 | 0.400 |
| `openai/gpt-5.6-luna` | 58 | 92,490 | 8,732 | 0.014 |
| `qwen/qwen3-embedding-8b` | 26 | 1,436 | — | 0.00001 |

The embedding row names `qwen/qwen3-embedding-8b` because this window was
measured before the shipped generation moved to `google/gemini-embedding-2` on
2026-08-19. At the price above the same 1,436 tokens would be 0.0003 USD —
under a thousandth of this table's total, which is why the switch is not worth
re-measuring the window for.

In euro at the rate above: 0.32 EUR / 24 h for the full window, 0.024 EUR / 24 h
unattended.

Two things are worth reading off this table rather than the headline. First, the
frontier model is 97 % of the bill on 24 % of the calls — the tier split is
where the money is, and moving one cell between tiers moves the total more than
any amount of prompt trimming. Second, the unattended figure is what the colony
costs when nobody is talking to it: timers, the nightly consolidation run, and
the memory lane. The difference between the two rows is conversation.

**Read this as one data point, not as a rate card.** The number is dominated by
how much you talk to the colony and which cells you put on which tier. A
different traffic shape gives a different number, and the honest use of this
table is as a worked example of the method, not as a prediction of your bill.
The observation window also contains no complete calendar day — the colony was
started into this configuration mid-window — so both figures are extrapolations
from partial windows, computed as described in [The formula](#3-the-formula).

## The S tier: not measured

The S tier runs everything on a local model. There are no provider tokens, so
this method reports zero by construction — which is exactly why it is not a
measurement. The honest statement is **~$0/day in provider spend, plus
electricity**, and the electricity term is the entire cost and is not measured
here. A local-tier measurement needs a different instrument (wall power over a
representative day) and does not exist yet.

## The L and XL tiers: not measured

**Not yet measured.** No production run has been made on either tier, and no
number will be quoted for them until one has. The method above applies unchanged
— point the script at such a colony and it produces the same table.

| tier | status |
|---|---|
| S — everything local | provider spend ~$0 by construction; electricity not measured |
| M — local plus a frontier model for the hard calls | measured, see above |
| L — frontier mix | not yet measured |
| XL — full frontier | not yet measured |
