# What it costs to run

A colony spends money in one place: the provider calls its `llm` cells make. Every delivered
message is already a row in `colony.db`, table `message_log`, and a row that came back from a
provider carries the provider's own accounting in the `hop` object of its `headers` column:
`model`, `tokens_prompt`, `tokens_completion`. Those three fields are the whole
instrumentation, and nothing has to be switched on to get them. Rows with no token fields are
store, code and routing hops, and they cost nothing.

The counts come from the provider's answer, not from a token estimate computed locally, so
what a colony spends is read rather than guessed. Prices sit in dated JSON files beside the
script, one file per retrieval day, so every figure can be traced to a list and a date and
re-derived later. Electricity, the machine the binary runs on and prompt cache discounts are
in none of the numbers here, and where caching is active the real bill is lower. If a figure
computed this way disagrees with what your provider bills you, your provider is right.

## Read your own spend

```sh
python3 scripts/cost_report.py \
    --db     /path/to/your/colony.db \
    --prices scripts/prices-openrouter-2026-08-25.json
```

The database is opened through the SQLite URI `file:<path>?mode=ro`, so a colony that is
currently running is a safe target. The script reads three columns and never touches
`body_payload`, where the conversation itself lives. `--from` and `--to` bound the window,
`--by-cell` breaks the total down per cell, `--json` makes the output machine readable. Bound
the window explicitly, because a running colony keeps appending and an open ended re-run does
not reproduce a figure.

The same rows, without the script:

```sql
SELECT json_extract(headers, '$.hop.model')             AS model,
       count(*)                                         AS calls,
       sum(json_extract(headers, '$.hop.tokens_prompt')),
       sum(json_extract(headers, '$.hop.tokens_completion'))
FROM message_log
WHERE json_extract(headers, '$.hop.tokens_prompt') IS NOT NULL
GROUP BY model;
```

Multiply each row by the price of its model, in USD per million tokens, and add the models up
for a total. A row whose `model` is null reached a provider without the substrate seeing a
model id, which is what an embedding call from a `code` cell looks like; the script attributes
those by cell path through a `fallback_models` rule in the price file, and leaves whatever
stays unattributed out of the total instead of pricing it at a guess. The current price file
is [`../scripts/prices-openrouter-2026-08-25.json`](../scripts/prices-openrouter-2026-08-25.json);
the three older snapshots stay beside it, because a number is only reproducible against the
list it was computed from.

## One colony, measured

A personal assistant running around the clock: a conversational brain on `gpt-5.6-luna`, a
reasoning brain on `claude-opus-5`, a memory hive whose judge runs on the frontier model, and
an embedding lane. The window is 27.27 h, 2026-08-14 09:19 UTC to 2026-08-15 12:35 UTC, and it
holds 110 provider calls out of 6,209 logged messages.

| measured | model | calls | tokens in / out | USD | priced from |
|---|---|---|---|---|---|
| 2026-08-14/15 | `anthropic/claude-opus-5` | 26 | 50,999 / 5,784 | 0.400 | `prices-openrouter-2026-08-15.json` |
| 2026-08-14/15 | `openai/gpt-5.6-luna` | 58 | 92,490 / 8,732 | 0.014 | `prices-openrouter-2026-08-15.json` |
| 2026-08-14/15 | `qwen/qwen3-embedding-8b` | 26 | 1,436 / none | 0.00001 | measured, 2026-08-08 |

That is 0.414 USD over the window, or 0.364 USD per 24 h. The frontier model is 97 % of that
bill on 24 % of the calls, so which cell sits on which tier moves the total further than any
amount of prompt trimming. The window holds no complete calendar day, so the daily rate is an
extrapolation from a partial one. Read the table as one worked example of the method: the
number follows how much you talk to a colony and which models you put where.

## Where to read more

- [`../scripts/cost_report.py`](../scripts/cost_report.py) is the method in about twenty lines of arithmetic
- [`../examples/never-forgets/WALKTHROUGH.md`](../examples/never-forgets/WALKTHROUGH.md) § *Step 8* runs the same command against a four call colony and shows the output
- [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md#what-a-close-pass-costs-measured) prices one closed session of a member's memory
- [`model.md`](model.md) says where the key and the model go
