# examples/meclaw-os

An empty folder, a template library, and one declaration. Out of that: a screened,
session-keeping, memory-draining conversational agent, seventeen cells, no new Rust.

This is the example the other three build up to. `hello` shows you a cell, `swarm` shows you a
loop, `telegram-research` shows you a real agent written out node by node. This one writes
nothing out. It boots a seed with **zero cells in it**, hands the colony **one JSON file**, and
the agent is there.

## What is checked in

```
meclaw-os/
├── seed/                      the --root of the colony. This is the whole tree.
│   ├── colony.json            substrate defaults. two lines.
│   └── main/config.json       type: "hive", and its graph is EMPTY
├── grow.json                  the declaration. five nodes, seven edges.
├── grow-cogny.json            step two: the thinking core. one node, two edges.
└── grow-steward.json          step three: the control loop. one node, one edge.
```

That is **three files**, and none of them is a cell. There is no door in here, no terminal, no
agent, no memory, no screening and no persona -- every one of those arrives from `templates/`,
at runtime, into a colony that is already up.

Two of the five templates were extracted out of this folder to make that true: the
[`door@1`](../../templates/door/) that names the ingress lane, and the
[`terminal@1`](../../templates/terminal/) that every outbound lane ends in. They used to be
"the two cells a library cannot ship". They turned out to be the two cells a library *should*
ship -- generic, ten lines each, and needed by every tree.

## What grows

`grow.json` names five templates and draws the edges between them:

| node | from template | what it brings |
|---|---|---|
| `/surface` | [`door@1`](../../templates/door/) | 1 cell. `POST /messages` becomes a turn on the ingress lane, carrying the channel identity. |
| `/firewall` | [`firewall@1`](../../templates/firewall/) | 2 cells. Size cap, sender rules, rate limit -- every verdict a comparison or a clock, never a model. |
| `/talky` | [`talky`](../../templates/talky/) | 11 cells. Session keeper, context collector, tool dispatcher, summarizer, and an `llm` brain, with all twelve internal edges pre-wired. |
| `/drain` | [`memory-drain`](../../templates/memory-drain/) | 2 cells. Turns a closed session into one episode per turn, idempotently. |
| `/sink` | [`terminal@1`](../../templates/terminal/) | 1 cell. The stop for four lanes that have not been decided yet. |

```
                       grow.json draws these seven

  POST /messages
        |
        v
    /surface ──turn──> /firewall/screen ──pass──> /talky/session-keeper
                            │                       ⋮
                            │                 (the composite's own
                            │                  twelve internal edges:
                            │                  seam, brain, dispatcher,
                            │                  loopback, close path)
                            │                       ⋮
                            │              /talky/collector
                            │                    │           │
                            │              answer│           │write
                            │                    v           v
                            └──reject──────>  /sink  <──episode── /drain
                                                ^
                            /talky/errors ──error┘
```

Four of those seven edges end in `/sink`, and that is the honest part of this example: an
answer, a rejection, an error report and a drained episode are **four different decisions**, and
this example makes none of them for you. In a real tree the answer goes back out of the surface,
the rejection into a log, the error onto an alarm, and the episode into a memory hive's
in_episode lane. Here they all stop in one place so you can watch them arrive in the trace.

## Run it

```bash
# from the repo root, on a fresh release build
cargo build --workspace --release

cat > examples/meclaw-os/seed/.env <<'EOF'
OPENROUTER_API_KEY=sk-...
MODEL_BRAIN=openai/gpt-4o-mini
MODEL_CORE=openai/gpt-4o
MODEL_CORE_FAST=openai/gpt-4o-mini
EOF

./target/release/meclaw --root ./examples/meclaw-os/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7777
```

Open `http://127.0.0.1:7777/ui/registry`. **Nothing.** Now grow it:

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/meclaw-os/grow.json
```

Reload the registry. Seventeen cells.

`MODEL_BRAIN` is read at instantiation and again at every read afterwards, so a different model
is an `.env` line and a reboot, not a config edit. Any OpenAI-compatible endpoint works --
OpenRouter is only the default `base_url` the template carries.

## Drive it

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/surface", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "Say hello in one short sentence."}]}}'
```

```bash
TID=$(curl -s 'http://127.0.0.1:7777/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7777/ui/trace?trace_id=$TID"
```

The hop chain, with the firewall's three store round trips and the collector's window
bookkeeping folded away:

```
@external -> /surface -> /firewall/screen -> (three round trips to /firewall/rules) ->
/talky/session-keeper -> /talky/session-keeper/stamp -> (two round trips to
/talky/session-keeper/sessions) -> /talky/session-keeper -> /talky/collector ->
/talky/collector/assemble -> /talky/collector -> /talky/brain -> /talky/dispatcher ->
/talky/collector -> /talky/collector/assemble -> /talky/collector -> /sink
```

**A hive is a hop in that chain, and it appears twice per transit.** `/talky/collector`
and `/talky/session-keeper` are hives: no mailbox, no cell task, nothing runs in them. The
colony still logs the message that *arrives* at a hive, and logs the forwarded follow-up
with the hive as its sender -- so a message crossing a hive reads `-> /talky/collector ->
/talky/collector/assemble -> /talky/collector ->`, hive, cell, hive. `/firewall` is a hive
too and never shows up, because every edge addresses `/firewall/screen` directly and
nothing is ever routed to `/firewall` itself. A hive is in the trace when it is
**addressed**, not because it exists.

`headers` on an HTTP post land in the message's **context** compartment, which is why the
channel identity survives every later hop -- and why the firewall rate-limits per channel and
the keeper mints one session per channel instead of flattening every chat into one.

Without a key the empty colony still boots — but the grow step needs `OPENROUTER_API_KEY` to
*exist* in `.env`, because instantiating the `llm` cells substitutes it and a missing variable
is a hard reject (`env_var_missing`), by design. With a dummy value the colony grows and
routes; the `llm` cell then returns an auth error as a normal message on the error lane, and
you can watch that arrive too.

## Step two: grow the thinking core

The colony is up and answering. Growing it again is another `curl`, not a restart:

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/meclaw-os/grow-cogny.json
```

Twenty-two cells. That is the second half of the claim, and it is deliberately a **separate
declaration** rather than five more lines in `grow.json`: the first file answers "what is this
agent", the second answers "what does it think with", and the two are different decisions with
different models behind them (`MODEL_BRAIN` vs `MODEL_CORE`). Splitting them also shows the part
a single big file would hide -- that a running tree can be grown mid-life.

A `talky` is a **channel voice**: one identity, one tonality, one session store per channel,
fast models, and the job of keeping a conversation flowing. A [`cogny`](../../templates/cogny/)
is the **agent core**: core knowledge, core personality, thinking models that may take their
time, and the heavy reasoning and tool work. One core, N voices -- siblings, never nested.

The talky reaches it like a tool and it answers like an event, so the connection is three edges
and one knob (`DISPATCHER_ASYNC_TOOLS=consult_cogny,ask_memory` in `.env`). Every edge clears
`col_phase` (the errand leaves another collector's chain mid-assembly and would be refused with
it still set), promotes `consult_id` to context (single-hop correlation has to survive the
core's whole chain and come home), and restores the TTL -- an errand is a fresh journey, not the
tail of the turn that started it.

**Two ingress edges, not one**, because the core has two lanes since `cogny@1.1.0`: an errand
that arrives as `consult_cogny` is a research consult and takes the thinking lane, one that
arrives as `ask_memory` is a lookup and takes `brain_fast` -- same errand, same window, same
memory bundle, faster model, shorter answer. The class is the tool name the model chose, lifted
into `context.consult_class` on these edges and read by the seam inside the composite. A lookup
therefore never queues behind a research answer: an `llm` cell is strictly serial, so the second
lane is a second mailbox. That is also why `MODEL_CORE_FAST` is its own `.env` line.

## Add tools

The composite carries no tool cells on purpose -- which tools an agent has is the one thing
nobody else can decide for you. Adding one is an edge pair in a third mutation:

```json
{"from": "./talky", "to": "./weather",
 "condition": "has(hop.tool_name) && hop.tool_name == 'get_weather'"},
{"from": "./weather", "to": "./talky",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

The tool's *schema* is a different thing again: it lives in the brain's `system.tools`, written
by a system update or seeded on first birth. Topology is not persona.

## What this demonstrates, honestly

- **The tree is not the source code.** What is version-controlled here is an empty seed and a
  declaration; the substance lives in a library and arrives at runtime, into a colony that is
  already up. Nothing was restarted -- not for the first declaration and not for the second.
- **Composition is topology, not framework.** All seventeen cells came from five templates that
  know nothing about each other. What connects them is seven edges in one file.
- **Enforcement is code, phrasing is agentic.** The firewall's verdicts are comparisons; the
  agent never sees a turn it rejected.

Two honest limits: the four outbound lanes all end in one terminal here (a real tree splits
them), and the drain's `episode` port has no memory hive to write into in this distribution --
it is wired so you can see the episodes leave, one per turn of a closed session.

This is a proof of concept on a frozen schema. Read
[`talky`](../../templates/talky/README.md) next -- it is the longest of the template
READMEs because it is the one that pays off.

## Pinned

`crates/meclaw-cells/tests/meclaw_os_example.rs` boots this seed and applies **these** two
declarations -- the files, not copies of them -- against a mock provider, and drives one turn
from the HTTP surface to the reply port. It measures the seed (two files, zero cells, no edge)
and both counts (17, then 21). If the example rots, that test goes red first.

## Step three: the colony that measures itself

`grow-steward.json` adds the [`steward@2`](../../templates/steward/) — seven more cells that
read a charter, measure this colony out of its own ledger, have a model judge and simulate
against those numbers, mutate through the ordinary gated lane, verify, and then keep the
change or revert it against a plan authored beforehand. Every cycle writes a receipt.

It arrives inert. Every goal in the charter ships `enabled: 0`, so a grown steward measures
nothing and changes nothing until somebody turns a row on — the only defensible default for a
loop that mutates the tree it runs in.

And it arrives **unable to act**, which is the more interesting half:

```json
{"from": "./steward", "to": "/colony/mutations", "condition": "has(hop.route) && hop.route == 'mutate'"}
```

That edge is not in the declaration, and it cannot be: `/colony/*` is a virtual endpoint
rather than a registry node, so `add_edges` refuses it by name — at any scope, from any cell,
including from the steward itself. Granting the loop its mutation lane is a **boot-time
act**: a human puts that edge in the seed. No amount of growing gets around it.

Note the shape of the endpoint: it is the **hive**, not a cell inside it. `steward@2` is sealed
(`params.ports: []`), so `./steward/mutator` is not an address at all any more — a caller asks
for the `mutate` lane and never learns which cell produces it. The one edge the declaration
*can* draw is the other lane, `error`, and it is drawn at the hive for the same reason.

So the steward you grow here measures, judges and receipts, and changes nothing. That is a
legitimate way to run it — a good one for the first weeks, in fact, because the receipts tell
you what it *would* have done before you let it do anything.
