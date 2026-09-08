# The meclaw documentation

Every document here answers one question. Find the line that matches yours, read
that document, skip the rest. The published documents are written in English.

## Where to go

- [Quickstart: install the binary and boot a colony](../README.md) — four steps, and a
  colony is answering.
- [Glossary](glossary.md) — the sixteen words the other documents assume, each pointing at
  the file that defines it properly.
- [System overview](meclaw-overview.md) — the cell model, the edge model, headers, routing,
  mutations and the lifecycle. On conflict with any other file, this one wins.
- [Writing a `config.json`](config.md) — the `cell`, `params`, `contract` and `description`
  blocks, variable substitution, and what a cell is allowed to know.
- [The cell types](cell-types.md) — `llm`, `store`, `code`, `web_fetch`, `proxy`, `timer`,
  `mcp`, `hive` and the rest, with their params, their contracts and their failure modes.
- [Changing a colony while it runs](rewiring.md) — add, move and disconnect cells against a
  live colony, and the traps that catch everyone once.
- [What is under contract](stability.md) — the five public surfaces, what `0.x` promises about
  them, and which parts of the tree carry no promise at all.
- [The voice wire protocol](voice-wire-protocol.md) — every frame of a `meclaw-voice/1`
  WebSocket in both directions, close codes included.
- [The store-backed tool loop](store-backed-tool-loop.md) — fan tool calls out, collect
  every result, and re-enter inference exactly once.
- [What it costs to run](costs.md) — measure your own provider spend out of `colony.db`, and
  compare it with the numbers measured on one running colony.
- [The template library](../templates/README.md) — reference topologies to instantiate into
  your own tree, and what instantiation copies.
- [The example colonies](../examples/README.md) — from a two-cell colony to a four-level
  stack grown from an empty seed.
- [Walkthrough: a colony refuses an attack](../examples/hard-shell/WALKTHROUGH.md) — repeat
  the run without a provider key, a model or an account. Every command and every output
  block in it was recorded from a real terminal.
- [Walkthrough: an answer out of months-old memory](../examples/never-forgets/WALKTHROUGH.md)
  — repeat the run with a provider key of your own, for a fraction of a cent.
- [The roadmap](../ROADMAP.md) — what shipped, what is being worked on and what comes later;
  the issue tracker carries the detail.
- [Sending a patch](../CONTRIBUTING.md) — build, test, and find out which parts of the code
  are byte-pinned against fixtures on purpose.
- [What changed in a release](../CHANGELOG.md) — which version shipped which behaviour, one
  section per release.

## The why pages

Eight questions come up when meclaw is explained. Each has a page of its own
under [`why/`](why/), where two more files redirect older URLs to the page that
took their subject.

- [Everything is a file](why/everything-is-a-file.md) answers why the harness lives in the filesystem and why there is no SDK.
- [An operating system for agents](why/an-os-for-agents.md) answers what meclaw-os is and what the four composition levels are for.
- [Memory that outlives the window](why/memory.md) answers where a conversation is kept once the context window stops being the place.
- [One assistant, two brains](why/two-brains.md) answers why the shipped assistant runs two models.
- [Why Rust, why Linux only](why/rust-and-linux.md) answers which properties the single binary and the kernel sandbox buy, and what they cost.
- [Ontology, in the meclaw sense](why/ontology.md) answers which typed catalogue the builder designs against, and which meaning of the word is not meant here.
- [Prepared for self-improvement](why/rsi.md) answers which primitives for rebuilding a running colony exist today, and why no loop closes them unattended.
- [You talk, it shows](why/you-talk-it-shows.md) answers what the voice cell and the screen are aimed at together, and what is still a person's judgement.

## If you are new

1. [`../README.md`](../README.md), for what this is and why the graph is the program.
2. [`glossary.md`](glossary.md), so the next document reads as prose.
3. One of the two walkthroughs above, to attach the vocabulary to something running.
4. [`meclaw-overview.md`](meclaw-overview.md), the sections *Core principles*, *Cell model*, *Edge model* and *Headers vs. body*. The rest of the file rests on those four.
5. [`config.md`](config.md) and [`cell-types.md`](cell-types.md) as reference, when you write your first cell.
