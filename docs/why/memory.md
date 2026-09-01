# Memory that outlives the window

Most agent stacks treat memory as a patch on the context window: summarise when it fills,
compress old turns, hope nothing important was in the compressed half. meclaw starts from
the opposite end:

> **The window was never where the conversation was stored.**

A conversation can run for weeks because the context window is a *projection*, assembled
fresh for the turn that is running, out of a record that never lived in the window at all.

## The hive

[`memory-hive`](../../templates/memory-hive/) is a member's long-term memory as a hive:
thirteen cells, not a line of Rust. It belongs to the **member**, not the assistant — one
source of truth that every agent of that person reads, so two assistants know the same
person and a replaced assistant is born knowing the history.

**The write path uses no LLM.** Turns arrive as messages on a lane and are written as
records — deterministic, cheap, and never able to hallucinate what was said.

**Retrieval is a fan of five model-free legs**: keyword, semantic, graph walk, temporal,
and the asker's own dossier. The legs run against the record and are fused without a
model; every leg has a budget, so retrieval cost is bounded per turn, not proportional to
history length.

**Consolidation supersedes instead of deleting.** A nightly pass distils; the distillate
points back, the original stays. Memory here is append-only in the sense that matters:
you can always ask what the record looked like before.

## The assembled window

On each turn a collector builds the context for exactly that turn: recent thread, the
retrieval bundle, the person's record — each leg under its own budget. The window is
**assembled, not accumulated**. There is no "context is full" cliff, because nothing
accumulates: a week-old fact and a minute-old fact reach the turn the same way, by being
retrieved.

## Compared to the usual approaches

- **Summarise-on-overflow** (most frameworks): lossy at exactly the moment you cannot
  choose what to lose. Here nothing overflows, because nothing accumulates.
- **Vector store bolted beside the agent**: one leg of five. Semantic similarity misses
  names, dates, and relations; the fan exists because no single index answers all four
  question shapes.
- **Memory as an external SaaS**: the hive is cells in your tree — inspectable with the
  same trace, exportable as documents, local if your models are.

## Reborn from its documents

`in_export` walks a member's memory, record, screening rules and session ledger out as
versioned documents; `in_import` takes them back into a running hive. A rebuilt member is
born with their history — which is also the honest test that the memory is *data*, not
state trapped in a process.
