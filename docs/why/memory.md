# Memory that outlives the window

Where is a conversation kept, once the context window is not the place? In a member's own memory
hive, as rows on disk. The window was never where the conversation was stored: it is assembled
for the turn that is running, out of that record, and thrown away again.

## Where the record lives

[`memory-hive`](../../templates/memory-hive/) is a member's memory as a hive of fifteen cells of
existing types, with no new Rust in it. It hangs at the member level, so two assistants of one
person read one record and a replaced assistant is born knowing it.

The write path uses no model. A turn arrives on a lane and becomes an append-only `episodes` row
immediately, and the agent never waits for it. Every durable row carries who was present when it
was learned, and the read path answers only with rows the current round could have heard. A turn
arriving without an audience or a channel is refused rather than written untagged.

## Reading is a question somebody asks

Recall is a request the hive answers, not a step that happens on its own. `memory_recall` is an
ordinary tool name, declared and served by the member's memory
([GH #552](https://github.com/mmeyerlein/meclaw/issues/552)), and the shipped reasoning core
carries `memory_tier` empty, so it asks by tool call when it wants a time range or a session
(`templates/cogny/template.json`). The shipped conversation surface is wired the other way: its
`memory_tier` is `1`, so every turn reads a small ambient bundle before it answers
(`templates/assistant/talky/config.json`). Which of the two a cell does is a line in its config,
not a rule of the hive.

There are three tiers and the caller picks one. Tier 0 is a deterministic, token-budgeted bundle,
no model and no embedding. Tier 1 fans out into five legs, keyword, semantic, graph walk, temporal,
and the facts about whoever is asking, fused by reciprocal rank fusion in code. Tier 2 has a model
synthesise one answer over those candidates and refuses to pass it on without a gap statement.

## Nothing is deleted

A close pass reads a finished session whole, and a nightly pass supersedes instead of deleting.
The distillate points back, the original stays, and which fact is in force is decided when the
question is asked, on the version chain. The same night decides which relation keys and which
entity spellings mean one thing, and records those merges as revertible aliases.

## What it does not do yet

The graph leg matches entity names exactly, so the fuzzy index covers the fact axis and leaves the
walk alone. The embedder is optional, and without one recall drops from five legs to three while
writes keep queueing. Skills and decay scoring are unbuilt.

The level this hive hangs at, and the parts beside it, are on [meclaw-os](../meclaw-os.md).
