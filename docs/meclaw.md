# meclaw

meclaw is a workflow substrate whose topology is a directory tree. Every directory with a
`config.json` is a node, one binary reads the tree at boot, and nodes talk only over declared
edges. Seven words carry the model: colony, cell, hive, edge, hop, mutation, template vs. instance.

Because the topology is files, the tools you already have work on it: `cat`, `grep`, `diff`,
`git`. It is also the shape a builder LLM writes, so a program and a person rewire a running
colony through the same door.

## colony

A colony is a folder, and the sole write authority in the system: the registry, the routing, the
template library, the lifecycle of cells and every mutation. Every cell registers directly with it,
so routing is one lookup however deep the tree goes. The binary takes one with `--root`.

## cell

A cell is an actor: one task, one mailbox, one job, single-threaded on the inside. It knows its
contract, its `params` and the message in front of it, and never the sender, the receiver or
another cell, which is what makes it movable. Sixteen cell types have a factory in the binary,
among them `llm`, `store`, `bash`, `web` and `voice`.

## hive

A hive is a directory whose `config.json` says `type: "hive"`, with no task and no mailbox. It
marks a path prefix as an authority and mutation boundary, and an edge from outside addresses
the hive and never a cell inside it. A message aimed at the hive has its out-edges evaluated.

## edge

An edge connects one output to one input, and it is where the logic of a colony lives. One
output may carry several edges, which is fan-out and runs in parallel. Its `condition` is a CEL
boolean deciding whether the edge is responsible, reading only the two header namespaces and never
the body; its `modifier` is the sole header authority over `context.*` and `hop.*`.

## hop

`hop` is one of the two header compartments, the one that lives for exactly one hop: the output of
the cell that just emitted, refined by the edge the message travelled, replaced wholesale at the
next emission. Its sibling `context` persists, so a value survives only if an edge promotes it with
`set_context`. Content-aware routing runs on that pair, a cell setting a key and an edge reading it.

## mutation

A mutation is a body POSTed to `/colony/mutations`, carrying a `scope` and a `diff` written in
eight operation keys. The colony validates the hypothetical post-state in one stage, builds the new
cell directories under `.staging/` and renames them into place. New cells spawn while the process
keeps running, and every entry leaves a `mutation_log` row in `colony.db`.

## template vs. instance

Cells in `templates/` are classes; cells in the tree are instances. Instantiation copies the
subtree in, mints fresh UUIDs and stamps the provenance, and from then on the instance has no
link back, so editing a template never changes a colony that already grew from it.

## The shape on disk

```
examples/swarm/              the colony root, one --root, one process
└── main/config.json         type: "hive". params.graph seeds the edges below
    ├── llm/config.json      type: "llm"
    └── prep/, dispatch/, calc/, lookup/, collector/, done/   type: "code"

    prep ─► llm ─► dispatch ─┬─► calc ───┐
             │               └─► lookup ─┴─► collector
             ├─► done   hop.finish_reason == 'stop'    │
             ▲                                         │
             └─────────────────────────────────────────┘  the loop is this edge

    a person with curl ──────────────┐  POST /colony/mutations
                                     ├──► the colony, which alone applies it
    a builder hive, through `submit` ┘
```

## A cell

```json
{
  "cell": { "type": "llm" },
  "params": {
    "model": "openai/gpt-4o-mini",
    "api_key": "${OPENROUTER_API_KEY}"
  }
}
```

`cell.type` picks the factory in the binary and is immutable. `params` is handed to the cell 1:1
and the colony does not interpret it, because what the keys mean is the cell type's business.
`${OPENROUTER_API_KEY}` binds at every read, so the secret stays in `.env`, and the file carries
no name field because the directory it sits in is the address.

## A mutation

```json
{
  "scope": "/",
  "diff": {
    "add_nodes": [ { "name": "firewall", "template": "firewall" } ],
    "add_edges": [
      { "from": "./door", "to": "./firewall", "condition": "has(hop.route) && hop.route == 'turn'",
        "modifier": { "set_context": { "channel": "hop.chat_id" } } }
    ]
  }
}
```

`scope` is the prefix everything below resolves against; a path leaving it is rejected.
`add_nodes` names a template and an address, never a cell type. Edge endpoints are scope-relative,
and `add_edges` may name an address the same diff creates, so the cell arrives wired. Trimmed
from [`grow.json`](../examples/meclaw-os/grow.json), four cells and four edges in one POST.

## No loop is shipped

An `llm` cell makes one provider call, emits one message and is done, with no inner loop, and
meclaw brings no prefabricated tool-loop, dispatcher or collector topology. A loop is an edge
that routes an answer back into the `llm` cell, and in `examples/swarm` that edge is
`collector -> llm`. Delete it and the agent answers blind; change its condition and it reasons
differently, with no cell code touched. The iteration counter lives in `context`, incremented by
the loopback edge, because counting is edge authority.

## Where to read on

- [cells.md](cells.md), what each cell type is for
- [config.md](config.md), every block of a `config.json`
- [rewiring.md](rewiring.md), the operator's view of mutations, with worked recipes
- [meclaw-overview.md](meclaw-overview.md), the specification behind this page
- [glossary.md](glossary.md), the seventeen words
- [why/everything-is-a-file.md](why/everything-is-a-file.md), what the file idea buys and its limits
- [why/ontology.md](why/ontology.md), the typed vocabulary a colony is composed from
