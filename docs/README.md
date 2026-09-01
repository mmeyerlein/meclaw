# The meclaw documentation

These documents are not meant to be read in order. Find the row that matches
what you are trying to do.

## Where to go

| If you want to… | Read | Why that one |
|---|---|---|
| get a colony running in five minutes | [`../README.md`](../README.md) and [`../examples/`](../examples/) | The repo README has the quickstart and the vocabulary in five bullets; the examples are working colonies you can boot. |
| understand what the words mean | [`glossary.md`](glossary.md) | Sixteen terms, two sentences each, every one pointing at the place that defines it properly. |
| understand how the whole thing works | [`meclaw-overview.md`](meclaw-overview.md) | The system description and the **single source of truth**: cell model, edge model, headers, routing, mutations, lifecycle. On conflict with any other file, this one wins. |
| write or configure a cell | [`config.md`](config.md) | The `config.json` format, block by block — `cell`, `params`, `contract`, `description` — plus variable substitution and what a cell is and is not allowed to know. |
| pick the right cell type | [`cell-types.md`](cell-types.md) | Every built-in type in detail: `llm`, `store`, `code`, `web_fetch`, `proxy`, `timer`, `mcp`, `hive` and the rest, with their params, their contracts and their failure modes. |
| build a multi-tool agent loop | [`store-backed-tool-loop.md`](store-backed-tool-loop.md) | The protocol for fanning tool calls out, waiting for every result, and re-entering inference exactly once — worked through against a real example colony. |
| know what it costs to run | [`costs.md`](costs.md) | How to measure provider spend from a colony's own database, the numbers measured on one production colony, and which tiers are honestly unmeasured. |
| see what is stable and what is next | [`../ROADMAP.md`](../ROADMAP.md) | What has shipped, what is being worked on, and what is deliberately not planned. The issue tracker carries the substance. |
| contribute | [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | How to build, how to test, and which parts of the codebase are byte-pinned against fixtures on purpose. |

## The why pages

meclaw is different enough that the same questions come up every time. Each has
a page of its own under [`why/`](why/) — one question, one answer:

- [Everything is a file](why/everything-is-a-file.md) — why the harness lives in
  the filesystem, and why there is no SDK.
- [An operating system for agents](why/an-os-for-agents.md) — what meclaw-os is,
  the four composition levels, the authorities, and apps.
- [One assistant, two brains](why/two-brains.md) — why the shipped assistant
  runs two models.
- [Memory that outlives the window](why/memory.md) — the memory hive, and why
  the context window is assembled instead of accumulated.
- [Ontology, in the meclaw sense](why/ontology.md) — the typed catalogue the
  builder designs against, and how it learns new words.
- [Recursive self-improvement — seriously?](why/rsi.md) — which primitives
  exist, and why the loop deliberately does not.
- [The strange names](why/names.md) — argus, affinity, talky, cogny and
  friends, one line each.
- [Why Rust, why Linux only](why/rust-and-linux.md) — the binary, the kernel
  sandbox, and the reverse-proxy stance.
- [You talk, it shows](why/you-talk-it-shows.md) — the vision, explicitly
  marked as an idea rather than a feature.

## Two step-by-step walkthroughs

Both are transcripts: every command was run, every output block is what came
back.

- [`../examples/hard-shell/WALKTHROUGH.md`](../examples/hard-shell/WALKTHROUGH.md)
  — attack a colony on purpose. No key, no model, no account. Ends with four
  encoded SSRF attempts that all get refused.
- [`../examples/never-forgets/WALKTHROUGH.md`](../examples/never-forgets/WALKTHROUGH.md)
  — teach a colony three months of history and then ask it a question with a
  date in it. Needs a provider key; costs a fraction of a cent.

## The reading order that actually works

If you are new and want the whole picture rather than an answer to one question:

1. [`../README.md`](../README.md) — what this is and why the graph is the program.
2. [`glossary.md`](glossary.md) — so the next document reads as prose.
3. One walkthrough above — the vocabulary attached to something running.
4. [`meclaw-overview.md`](meclaw-overview.md) §§ *Core principles*, *Cell model*,
   *Edge model*, *Headers vs. body* — about a fifth of the file, and the fifth
   everything else rests on.
5. [`config.md`](config.md) and [`cell-types.md`](cell-types.md) as reference,
   when you write your first cell.

## About these files

**The three specification documents are a trio.**
[`meclaw-overview.md`](meclaw-overview.md) is the source of truth;
[`cell-types.md`](cell-types.md) and [`config.md`](config.md) are detail specs
that say so in their own first paragraph. If they disagree with the overview,
the overview is right and the detail spec is a bug.

**Language.** The trio is written in German and maintained with an English twin
alongside it (`X.md` German, `X.en.md` English); the newer documents — this index, the glossary, `costs` and
`store-backed-tool-loop` — are English-only and have no German twin. Publication resolves that: the English side is what ships,
under the plain `X.md` name. So a published tree is entirely English and
contains no `.en.md` file at all, which is why every link on this page points at
the plain name. Drift between a pair is a release gate, not a matter of
diligence.

**`defer-register.md`** in this directory is an internal register of what is
deliberately **not** built — one row per topic, each with the trigger that would
make it due. It was called `roadmap.md` until 2026-08-28, when the name turned
out to be doing two jobs: the ordering job moved to
[`../ROADMAP.md`](../ROADMAP.md), which is the forward-looking list a reader
wants and which does travel. Neither the register nor `deferred.md` (the
transient phase-14 cycle log, archived on 2026-08-20 to `archive/deferred.md`
when that cycle closed) is part of a published tree, which is why both are
named here rather than linked: a link to a file the published tree does not
carry is a dead link in the only tree that matters.

**`demo.sh`, `demo.cast` and `demo.svg`** are a terminal recording of
`docs/demo.sh` driving [`../examples/swarm/`](../examples/swarm/) — the
tool-loop showing up as a path through the tree. Replayable with
`asciinema play docs/demo.cast`.
