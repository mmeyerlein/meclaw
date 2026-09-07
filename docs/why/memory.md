# Memory that outlives the window

The window was never where the conversation was stored. A context window in meclaw is built
for the turn that is running and thrown away again, out of a record that lives somewhere
else: in a member's own memory hive.

## Where the record lives

[`memory-hive`](../../templates/memory-hive/) is a member's memory as a hive of fifteen
cells and no new Rust. `store`, `writer`, `recall`, `extract-glue`, `close-glue`, `closer`,
`dream-glue`, `dreamer`, `judge`, `clock`, `embed`, `dialectic`, `porter`, `tool` and
`schemas` are the whole of it (`templates/memory-hive/README.md`). It hangs at the member
level, so two assistants of one person read one record and a replaced assistant is born
knowing it.

The write path uses no model. A turn arrives on a lane and becomes an append-only `episodes`
row immediately, and the agent never waits for it. Every durable row carries who was
present when it was learned, and the read path answers only with rows the current round
could have heard; a turn arriving without an audience or a channel is refused instead of
written untagged.

## How a window gets built

Reading is three tiers and the caller picks one. Tier 0 is a deterministic token-budgeted
bundle with no model and no embedding in it. Tier 1 is a fan of five legs, keyword,
semantic, graph walk, temporal, and the facts about whoever is asking; the legs run against
the record and are fused by reciprocal rank fusion in code. Tier 2 has a model synthesise
one answer over the tier-1 candidates and refuses to pass it on without a gap statement.

How often a turn reaches for any of that is a wiring decision of the tree above the hive.
The shipped reasoning core carries `memory_tier` empty, so it never fires the ambient leg
and asks the memory by tool call when it wants a time range or a session
(`templates/cogny/template.json`). The same idea runs in the tool loop, where a round of
tool calls rebuilds the conversation out of a store before it goes back to the model,
written out hop by hop in [the store-backed tool loop](../store-backed-tool-loop.md).

## Nothing is deleted

A nightly pass reads a delta window and supersedes instead of deleting. The distillate
points back, the original stays, and which fact is in force is decided when the question is
asked, on the version chain. The same night judges which relation keys and which entity
spellings mean one thing, and records the merges as revertible aliases.

Everything the hive holds can leave it as a versioned document and enter another running
hive; applied twice, the document changes nothing the second time.

## What it does not do yet

The graph leg matches entity names exactly, so the fuzzy index covers the fact axis and
leaves the walk alone. The embedder is optional, and without one recall drops from five legs
to three while writes keep queueing.

Skills and decay scoring are unbuilt. The transfer lane moves content and leaves the colony
behind; a file is a whole table, and a hive whose tables outgrow one read needs a paged form
that does not exist yet (`templates/memory-hive/template.json`).
