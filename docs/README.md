# The meclaw documentation

Every document here answers one question. Find the row that matches yours, read
that document, skip the rest.

## Where to go

| If you want to … | Read | What you can do afterwards |
|---|---|---|
| get a colony running | [`../README.md`](../README.md) | Install the binary and boot a working colony from the five commands in the quickstart. |
| look up a word you just met | [`glossary.md`](glossary.md) | Read the other documents as prose. Every term links to the document that defines it properly. |
| understand how the whole thing works | [`meclaw-overview.md`](meclaw-overview.md) | Explain the cell model, the edge model, headers, routing, mutations and the lifecycle to someone else. On conflict with any other file, this one wins. |
| write or configure a cell | [`config.md`](config.md) | Write a `config.json` by hand: the `cell`, `params`, `contract` and `description` blocks, variable substitution, and what a cell is allowed to know. |
| pick the right cell type | [`cell-types.md`](cell-types.md) | Choose between `llm`, `store`, `code`, `web_fetch`, `proxy`, `timer`, `mcp`, `hive` and the rest, with their params, their contracts and their failure modes. |
| change a colony while it runs | [`rewiring.md`](rewiring.md) | Add, move and disconnect cells against a live colony, and see the traps that catch everyone once. |
| write a client that speaks to a colony | [`voice-wire-protocol.md`](voice-wire-protocol.md) | Open a `meclaw-voice/1` WebSocket and handle every frame in both directions, close codes included. |
| build a multi-tool agent loop | [`store-backed-tool-loop.md`](store-backed-tool-loop.md) | Fan tool calls out, collect every result, and re-enter inference exactly once. |
| know what it costs to run | [`costs.md`](costs.md) | Measure your own provider spend from `colony.db`, and compare it with the numbers measured on one running colony. |
| start from something that already works | [`../templates/README.md`](../templates/README.md) | Instantiate a reference topology into your own tree, and know what instantiation copies. |
| read a whole colony end to end | [`../examples/README.md`](../examples/README.md) | Boot the example colonies, from a two-cell one to a four-level stack grown from an empty seed. |
| watch a colony refuse an attack | [`../examples/hard-shell/WALKTHROUGH.md`](../examples/hard-shell/WALKTHROUGH.md) | Repeat the run yourself without a provider key, a model or an account. Every command and every output block in it was recorded from a real terminal. |
| watch a colony answer from months-old memory | [`../examples/never-forgets/WALKTHROUGH.md`](../examples/never-forgets/WALKTHROUGH.md) | Repeat the run with a provider key of your own, for a fraction of a cent. |
| see what is planned | [`../ROADMAP.md`](../ROADMAP.md) | Tell what shipped from what is being worked on and what comes later. The issue tracker carries the detail. |
| send a patch | [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Build, test, and find out which parts of the code are byte-pinned against fixtures on purpose. |
| find out what changed in a release | [`../CHANGELOG.md`](../CHANGELOG.md) | See which version shipped which behaviour, one section per release. |

## The why pages

Six questions come up when meclaw is explained. Each has a page of its own
under [`why/`](why/).

- [Everything is a file](why/everything-is-a-file.md) answers why the harness lives in the filesystem and why there is no SDK.
- [An operating system for agents](why/an-os-for-agents.md) answers what meclaw-os is and what the four composition levels are for.
- [Memory that outlives the window](why/memory.md) answers where a conversation is kept once the context window stops being the place.
- [One assistant, two brains](why/two-brains.md) answers why the shipped assistant runs two models.
- [Why Rust, why Linux only](why/rust-and-linux.md) answers which properties the single binary and the kernel sandbox buy, and what they cost.
- [Self-modification](why/self-modification.md) answers which primitives for rebuilding a running colony exist today, and why no loop closes them unattended.

## If you are new

1. [`../README.md`](../README.md), for what this is and why the graph is the program.
2. [`glossary.md`](glossary.md), so the next document reads as prose.
3. One of the two walkthroughs above, to attach the vocabulary to something running.
4. [`meclaw-overview.md`](meclaw-overview.md), the sections *Core principles*, *Cell model*, *Edge model* and *Headers vs. body*. The rest of the file rests on those four.
5. [`config.md`](config.md) and [`cell-types.md`](cell-types.md) as reference, when you write your first cell.

## About these files

The three specification documents work as a set. `meclaw-overview.md` describes
the system; on conflict, that file wins and the detail spec carries the bug.
`cell-types.md` and `config.md` are detail specs and say so in their own first
paragraph.

The published documents are English. Five of them (the overview, cell types,
config, rewiring and the voice protocol) are written in German in the source
tree and kept with an English twin beside them; publication ships the English
side under the plain `X.md` name, which is why every link on this page points at
the plain name. Drift between a pair is a release gate. Internal registers and
archives stay in the source tree and do not travel with the published one.
