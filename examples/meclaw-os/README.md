# examples/meclaw-os

An empty folder, a template library, and one declaration. Out of that: a screened,
session-keeping conversational agent, sixteen cells, no new Rust.

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
├── grow.json                  the declaration. four nodes, four edges.
├── grow-cogny.json            step two: the thinking core. one node, three edges.
├── grow-steward.json          step three: the control loop. one node, no edge.
└── grow-canvy.json            step four: the colony's own picture. one node, no edge.
```

That is **four files**, and none of them is a cell. There is no door in here, no terminal, no
agent, no memory, no screening and no persona -- every one of those arrives from `templates/`,
at runtime, into a colony that is already up.

Two of the four templates were extracted out of this folder to make that true: the
[`door@1.0.2`](../../templates/door/) that names the ingress lane, and the
[`terminal@1.0.1`](../../templates/terminal/) that every undecided lane ends in. They used to be
"the two cells a library cannot ship". They turned out to be the two cells a library *should*
ship -- generic, ten lines each, and needed by every tree.

## What grows

`grow.json` names four templates and draws the edges between them:

| node | from template | what it brings |
|---|---|---|
| `/door` | [`door@1.0.2`](../../templates/door/) | 1 cell. `POST /messages` becomes a turn on the ingress lane, carrying the channel identity. |
| `/firewall` | [`firewall@2.0.5`](../../templates/firewall/) | 2 cells. Size cap, sender rules, rate limit -- every verdict a comparison or a clock, never a model. |
| `/talky` | [`talky`](../../templates/talky/) | 12 cells. Session keeper, context collector, tool dispatcher, answer splitter, summarizer, and an `llm` brain, with every internal edge pre-wired. |
| `/sink` | [`terminal@1.0.1`](../../templates/terminal/) | 1 cell. The stop for two lanes that have not been decided yet. |

```
                        grow.json draws these four

  POST /messages
        |
        v
    /door ──turn──> /firewall ──pass──> /talky/session-keeper
                                                    ⋮
                                              (the composite's own
                                               internal edges:
                                               seam, brain, splitter,
                                               dispatcher, loopback,
                                               close path)
                                                    ⋮
                                           /talky/collector
                                                 │           │
                                           answer│           │turn_write
                                                 v           v
                                              /sink       /sink
```

Two of those four edges end in `/sink`, and that is the honest part of this example: an
answer and a finished turn are **two different decisions**, and this example makes neither
of them for you. In a real tree the answer goes back out of the surface and the turn into a
memory hive's in_episode lane. Here they both stop in one place so you can watch them arrive
in the trace.

**What is deliberately not drawn (GH #284).** The firewall's `reject` and the talky's
`error` used to end in `/sink` too, and they no longer end anywhere: there is no edge for
them in this file. That is not an omission, it is the second of a refusal's two honest
states. A lane that reports a refusal either has a consumer that *does* something with it --
a log store, stderr, an alarm -- or it has no edge at all, and then the emission becomes
`no_route` and localises itself in the dead-letter queue with its sender and its trace. What
it must never have is a cell that accepts it and drops it, because that is the one
arrangement in which nobody finds out. And if a reject here starts firing routinely, that is
**a signal about the topology** -- something upstream is sending what the firewall is there
to stop -- not a reason to reinstate a silencer in front of it.

**Retraction (GH #298, ruling Q11).** Until this version the `turn_write` lane came out of a
`memory-drain` node that cut the talky's close batch into single turns. It is gone from this
declaration, and gone rather than moved: the collector emits one message per turn on
`turn_write` itself — `hop.turn_id`, `hop.turn_index`, `hop.happened_at` — which is the shape a
memory hive's `in_episode` lane reads, so there is nothing left for a decomposer to do. What
this example loses with it is a hive it never had a memory behind anyway; what it keeps is the
lane, ending where every undecided lane here ends.

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

Reload the registry. Sixteen cells.

`MODEL_BRAIN` is read at instantiation and again at every read afterwards, so a different model
is an `.env` line and a reboot, not a config edit. Any OpenAI-compatible endpoint works --
OpenRouter is only the default `base_url` the template carries.

## Drive it

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/door", "headers": {"channel": "chat-1"},
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
@external -> /door -> /firewall -> /firewall/screen -> (three round trips to
/firewall/rules) -> /firewall ->
/talky/session-keeper -> /talky/session-keeper/stamp -> (two round trips to
/talky/session-keeper/sessions) -> /talky/session-keeper -> /talky/collector ->
/talky/collector/assemble -> /talky/collector -> /talky/brain -> /talky/splitter ->
/talky/dispatcher -> /talky/collector -> /talky/collector/assemble ->
/talky/collector -> /sink
```

**A hive is a hop in that chain, and it appears twice per transit.** `/talky/collector`
and `/talky/session-keeper` are hives: no mailbox, no cell task, nothing runs in them. The
colony still logs the message that *arrives* at a hive, and logs the forwarded follow-up
with the hive as its sender -- so a message crossing a hive reads `-> /talky/collector ->
/talky/collector/assemble -> /talky/collector ->`, hive, cell, hive. `/firewall` reads the
same way, because `grow.json` addresses the hive and the `in_turn` lane picks the cell
behind it; since the seal (GH #228) there is no other way to enter it. A hive is in the
trace when it is **addressed**, not because it exists.

`headers` on an HTTP post land in the message's **context** compartment, which is why the
channel identity survives every later hop -- and why the firewall rate-limits per channel and
the keeper mints one session per channel instead of flattening every chat into one.

Without a key the empty colony still boots — but the grow step needs `OPENROUTER_API_KEY` to
*exist* in `.env`, because instantiating the `llm` cells substitutes it and a missing variable
is a hard reject (`env_var_missing`), by design. With a dummy value the colony grows and
routes; the `llm` cell then returns an auth error as a normal message on the error lane --
which this declaration deliberately does not wire (GH #284), so what you watch is the
dead-letter queue naming `/talky` and the trace that got it there.

## Step two: grow the thinking core

The colony is up and answering. Growing it again is another `curl`, not a restart:

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/meclaw-os/grow-cogny.json
```

Twenty-one cells. That is the second half of the claim, and it is deliberately a **separate
declaration** rather than five more lines in `grow.json`: the first file answers "what is this
agent", the second answers "what does it think with", and the two are different decisions with
different models behind them (`MODEL_BRAIN` vs `MODEL_CORE`). Splitting them also shows the part
a single big file would hide -- that a running tree can be grown mid-life.

A `talky` is a **channel voice**: one identity, one tonality, one session store per channel,
fast models, and the job of keeping a conversation flowing. A [`cogny`](../../templates/cogny/)
is the **agent core**: core knowledge, core personality, thinking models that may take their
time, and the heavy reasoning and tool work. One core, N voices -- siblings, never nested.

The talky reaches it like a tool and it answers like an event, so the connection is three edges
and one knob (`DISPATCHER_HANDOFF_TOOLS=consult_cogny,ask_memory` in `.env` -- a handoff is async
*and* says the answer comes from a later turn, which is what lets the round the consult leaves
behind end without a sentence of its own, GH #372). Every edge clears
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
- **Composition is topology, not framework.** All sixteen cells came from four templates that
  know nothing about each other. What connects them is four edges in one file.
- **Enforcement is code, phrasing is agentic.** The firewall's verdicts are comparisons; the
  agent never sees a turn it rejected.

Three honest limits: both outbound lanes that ARE wired end in one terminal here (a real
tree splits them); the `turn_write` lane has no memory hive to write into in this
distribution -- it is wired so you can see the turns leave, one message per turn, as they are
said; and three routes the composites still emit match **no edge in this file** at all --
`write` when a session closes, plus `reject` and `error` since GH #284. The talky's own
summarizer consumes `write` inside the hive, but the copy that leaves the composite ends
nowhere. Naming undecided lanes is this example's whole virtue, and these three it does not
name. Since GH #298 (ruling Q11) `write` carries a closed day for whoever archives one, and
this example archives nothing; a tree that wants that day wires it, and a tree that does not
should say so with a `terminal` rather than by silence. For `reject` and `error` the honest
default is the opposite one -- no edge, and the dead-letter queue as the record -- because a
`terminal` there would be a silence with a name on it.

This is a proof of concept on a frozen schema. Read
[`talky`](../../templates/talky/README.md) next -- it is the longest of the template
READMEs because it is the one that pays off.

## Pinned

`crates/meclaw-cells/tests/meclaw_os_example.rs` boots this seed and applies **these** two
declarations -- the files, not copies of them -- against a mock provider, and drives one turn
from the HTTP surface to the reply port. It measures the seed (two files, zero cells, no edge)
and both counts (16, then 21). If the example rots, that test goes red first.

`crates/meclaw-cells/tests/gh284_no_shipped_topology_silences_a_reject.rs` measures the other
half: no declaration in `examples/` and no `config.json` in `templates/` routes a `reject` or
an `error` into a cell that swallows it.

## Step three: the colony that measures itself

`grow-steward.json` adds the [`steward@2.0.11`](../../templates/steward/) — seven more cells that
read a charter, measure this colony out of its own ledger, have a model judge and simulate
against those numbers, send the decided change to the cell it names, verify, and then keep the
change or revert it against a plan authored beforehand. Every cycle writes a receipt.

It arrives inert. Every goal in the charter ships `enabled: 0`, so a grown steward measures
nothing and changes nothing until somebody turns a row on — the only defensible default for a
loop that reaches into the tree it runs in.

And it arrives **unable to act**, which is the more interesting half. The decided change
leaves the hive on the `mutate` lane as an ordinary **params update** — body
`{"system": {}, "params": {…}}`, with `hop.target` naming the cell it belongs to (GH #304) —
so acting means having an edge to that cell:

```json
{"from": "./steward", "to": "/some-llm-cell", "condition": "has(hop.route) && hop.route == 'mutate' && hop.target == '/some-llm-cell'"}
```

That edge is not in the declaration, and the steward cannot draw it: an edge is a mutation,
and **nothing in this hive authors one**. Granting the loop a target is a **boot-time act** —
a human puts the edge in the seed, one per cell the loop may touch. No amount of growing gets
around it, and the bound is per-cell rather than all-or-nothing: an edge on to
`/colony/mutations` would once have handed it the whole tree at a stroke.

**This colony grows it no target at all**, and not by omission. Both agents here are sealed
hives (`talky`, `cogny`, `params.ports: []`), so `/talky/brain` is not an address from
outside — an endpoint reaching past a seal is refused with `hive_port_boundary`. Giving this
steward something to change means adding a cell it may address, not drawing an edge into one
that already exists.

Two more bounds sit behind that, and neither depends on the steward behaving. The target
decides what it merges: an `llm` cell accepts `model`, `temperature`, `max_tokens` and the
other runtime-mutable keys, and refuses the credential and gate keys outright
(`IMMUTABLE_PARAM_KEYS`). And the radius stops before the cell types that have no params lane
at all — a `code` cell's numeric cap, like the collector's `max_iter`, comes back as
`key_outside_radius_<key>` with a receipt, rather than as a change nobody applied.

Note the shape of the endpoint on the way out: it is the **hive**, not a cell inside it.
`steward@2.0.11` is sealed (`params.ports: []`), so `./steward/mutator` is not an address at all
any more — a caller asks for the `mutate` lane and never learns which cell produces it. The
other lane the hive offers, `error`, would be drawn at the hive for the same reason — but
this declaration draws it nowhere (GH #284). A steward whose `error` ended in the sink would
be a loop that measures a colony and files its own failures in a bin; with no edge, a failed
cycle is a dead letter naming `/steward`, and that is a thing somebody can act on. So
`grow-steward.json` is one node and **no edge at all**: the whole step is "this hive now
exists here", and every connection it could have is a decision left to whoever grows it.

So the steward you grow here measures, judges and receipts, and changes nothing. That is a
legitimate way to run it — a good one for the first weeks, in fact, because the receipts tell
you what it *would* have done before you let it do anything.

## Step four: the colony draws itself

`grow-canvy.json` adds [`canvy@2.1.4`](../../templates/canvy/) — a timer, two `code` cells and a
`web` cell that serves one interactive canvas of this colony on a port of its own:

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/meclaw-os/grow-canvy.json
```

Then open `http://127.0.0.1:7811/` and drag the boxes around. Every cell the three steps above
grew is in there, in its hive, with its edges — and where you put a box is where it stays,
because a position is a prop of an object inside the display and the layout cell reads back
what the display already holds before it writes.

**One node, no edge, and a port override.** The node is no edge's business: the way in is the
HTTP port the display owns, which is also the whole access story — put a reverse proxy in front
of it, because the display binds loopback and grows no authentication ever. The override is
there because the port is the one knob an instance almost always sets:

```json
"override_params": {"web": {"port": 7811}}
```

The template's own default is `7810`; this example takes `7811` so that a canvas you already
run elsewhere on the default keeps it. A port is settled at instantiation — two displays
sharing one is a bind race rather than a configuration. **The "and immutable afterwards" half
of that sentence is withdrawn** (GH #410): a running display moves to another address on a
params update, keeping its `cell.db` and every hand-placed object.

The `event` lane out of the hive is **not** wired here, for the reason the rest of this example
gives: something a person does in the browser that this colony has no consumer for
dead-letters as `no_route` and localises itself, which is state (2) of GH #284 rather than a
silence. Nothing inside canvy consumes it either.

Upgrading an older canvas rather than growing a fresh one is a different exercise, and it has
its own recipe: [`templates/canvy/MIGRATION.md`](../../templates/canvy/MIGRATION.md).
