# `summarizer@2.0.2`

The session handover step as a hive of existing cell types -- no new cell type, no Rust.
Two cells: `prep` (a `code` cell, the glue) and `writer` (an `llm` cell, the prose).

**No longer part of `talky`.** Since `talky@4.3.0` the composite carries no summarizer:
the handover for the next generation comes from the member's memory recall bundle
instead ([GH #447](https://github.com/mmeyerlein/meclaw/issues/447)). This template stays
in the library as a standalone unit for a tree that wants the step on its own.

**When a generation closes, its successor should wake up with yesterday, not with
nothing.** The session keeper ends a generation, the collector hands the whole day out as
one batch on route `write` (R-OS-6) -- and this hive is the step behind that batch: it
turns the closed session into ONE fresh, recency-weighted summary and emits it in exactly
the form an llm cell consumes as a `system.*` update **without a provider call**. The next
generation opens lazily on the first morning turn and already carries the handover
(R-OS-3: the transition is not noticeable).

## What it delivers

- **One batch, one summary.** The collector's close batch enters on one lane and leaves as
  exactly one emission on route `summary` -- or exactly one on `summary_error`, never
  both, never silence.
- **Recency weighting as structure, not hope.** The prompt is shaped deterministically
  before any model sees it: the newest `SUMMARIZER_RECENT_TURNS` turns travel verbatim,
  everything older phases out to a bounded per-turn preview and is counted, tool rounds
  enter as capped previews. What the model weights is what the prompt already weighted.
- **The prompt lives in the glue phase.** An `llm` cell has no `params.system` slot; its
  system state arrives by message. `prep` sends the instructions fresh with every batch
  (`system.instructions`, accumulate-replace: an idempotent upsert), next to the one user
  document that is the day.
- **Honesty over invention.** The shipped instructions say it in one sentence: an empty or
  short session yields a short, honest summary -- never invent content the session does
  not contain. A summarizer that invents a day poisons the next generation's memory; the
  Nordstern is violated by a wrong answer, never by a modest one.
- **Degradation is a route, not a swallow.** A failed provider call (and an empty answer,
  see below) leaves on `summary_error` with the cause on the hop. The parent tree decides
  what that means -- drain, alarm, or nothing. There is **no retry in the template**:
  retry is edge business.

## Cells

| cell | type | what it does |
|---|---|---|
| `prep` | `code` | three lanes: shape the batch into the prompt (`in_batch`), form the answer into the handover update (`in_answer`), hand a failure on (`in_error`) |
| `writer` | `llm` | one provider call per batch, prose in, prose out; no tools, no identity of its own |

## The slot: `system.handover` (R-OS-1)

The summary emission's body is nothing but

```json
{ "system": { "handover": { "text": "<the summary>" } } }
```

-- a `system.*` update. Delivered to an llm cell it upserts `system.handover` in its
`cell.db` **without triggering an inference** (the emission carries no `messages[]`), and
because `system.*` is accumulate-replace per path, every close **replaces** the previous
handover with the fresh one. That replace is the feature: the slot always holds the
summary of the *last* closed generation, nothing accumulates stale.

**Slot discipline (R-OS-1: one writer per `system` path): the summarizer is the ONLY
writer of `system.handover`.** The collector owns `messages[]` + `system.memory`, the
affinity owns her paths (`identity`, `peer`, `relationship`, `channel`) -- nobody else
writes `handover`, and this hive writes nothing else.

## Ports

The entry lane goes **onto the hive path itself**: `params.ports` is `[]`, so
`<summarizer>` is the only address and the `in_` lane the edge names is what the door
inside picks up. The parent edge consumes exactly the collector's close-batch form
(`messages[]` the whole day in order, the raw round rows in the top-level slot `rounds`,
`hop.session_id` / `turn_count` / `round_count` the sizes):

```json
{"from": "<collector>", "to": "<summarizer>",
 "condition": "has(hop.route) && hop.route == 'write'",
 "modifier": {"set_hop": {"route": "'in_batch'"}}}
```

| lane | who sends it | what it does |
|---|---|---|
| `in_batch` | the collector's close lane (route `write`) | shapes the day into the prompt, asks the writer |

Exits leave **from the hive path** on `hop.route` -- `./prep -> .` is the out-door, so a
parent drains `<summarizer>` and never the cell behind it. **Where they lead is the parent's wiring,
not this hive's** -- the template does not know its target (Track E / the agent tree
decides):

| route | payload | typical target |
|---|---|---|
| `summary` | body = the `system.handover` update, `hop.session_id`, `hop.summary_chars` | the agent llm (so the next generation wakes up briefed); a store, if the handover should also be durable |
| `summary_error` | one assistant text naming session and cause, `hop.error_code` | a drain or an alarm -- the parent MUST wire it (error lanes are ports like all others) |

Wire the ports in the **same mutation** that instantiates the hive: an island without a
crossing edge derives inactive and never spawns. Instantiation needs `ctx.model` -- a
**resolved literal** per the K-H2 convention (the builder reads `MODEL_<ROLE>` from
`.env` and passes the value, not the `${...}` token).

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env var | default | meaning |
|---|---|---|
| `SUMMARIZER_RECENT_TURNS` | `12` | how many of the newest turns travel verbatim into the prompt |
| `SUMMARIZER_PHASEOUT_CHARS` | `200` | per-turn character cap on the phased-out older turns |
| `SUMMARIZER_TOOL_CHARS` | `200` | per-item character cap on tool call/result previews |
| `SUMMARIZER_ROUND_LINES` | `40` | how many tool-activity lines enter at most (newest kept) |

## The protocol, row by row

```
in_batch  -> shape the prompt            ROUTE llm   (internal, to ./writer)
writer    -> finish_reason stop|length   -> in_answer (internal edge)
          -> finish_reason error|content_filter -> in_error (internal edge)
in_answer -> ROUTE summary               <- ONE system.handover update
          -> empty answer? ROUTE summary_error (empty_summary)
in_error  -> ROUTE summary_error         <- error_code rides through
```

An unknown document parks (empty multi-send, terminal by design) -- the same discipline
as the collector this sits behind.

Decisions worth naming:

- **`length` is an answer, not an error.** A summary cut by `max_tokens` is truncated but
  real -- better a cut handover than none. The internal return edge treats `stop` and
  `length` the same.
- **An empty answer is an error, not an empty update.** `system.*` is accumulate-replace:
  an empty update would REPLACE a real handover with nothing. It leaves as
  `summary_error` with `error_code: "empty_summary"` instead.
- **`leg-*` round rows do not enter the prompt.** They are the collector's bookkeeping
  (assembly legs, eviction reports); the turns above them already are the conversation.
  Only `assistant`/`tool` rows -- the actual tool activity -- become context lines.
- **The session rides on the hop.** `prep` reads `hop.session_id` first (the collector
  stamps it on the batch), `context.session_id` second. The internal edge promotes it to
  context so the answer still knows whose summary it carries.

## What it is not

- **Not a session keeper.** It does not decide when a generation ends; it answers a batch
  that already left.
- **Not the memory hive.** One summary is written; what else the day is worth (extraction,
  episodes, facts) belongs to whoever else the parent routes the same `write` batch to.
- **Not a persona.** The writer has no identity beyond its instructions; the summary
  describes the session, it does not speak as the agent.
- **Not self-routing.** The template does not know where `summary` lands. The v1 target
  picture is the agent llm's `system.handover` slot; wiring it -- and whether the handover
  is additionally persisted -- is the parent mutation's decision.

## Pins

- `crates/meclaw-cells/tests/summarizer_prep.rs` -- the shipped `script_inline` against
  real stdin documents: the recency structure, the honesty sentence, the capped tool
  context, the handover update form, both error paths, the park.
- `crates/meclaw-cells/tests/summarizer_colony.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: one batch in, exactly one `system.handover` update
  out (and the instructions proven ON the wire), a provider 500 that leaves as exactly one
  `summary_error`.
