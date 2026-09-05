# examples/meclaw-os

An empty folder, a template library, and one declaration. Out of that: a screened,
session-keeping conversational agent, fourteen cells, no new Rust.

**Two roots live in this folder, and they answer different questions.** `seed/` plus the five
`grow-*.json` are what this page walks through: a colony grown one hand-written declaration at a
time, so you can watch each step arrive. `seed-ref/` is the other one — **stage one of a built
colony**, where the root tree declares the shipped `meclaw-os` shell and the first boot grows it,
and every further step is a manifest somebody's builder drafts rather than a file somebody typed.
It has its own section at the end.

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
├── grow-argus.json            step three: the control loop. one node, no edge.
├── grow-canvy.json            step four: the colony's own picture. one node, no edge.
├── grow-operator.json         step five: the front door. one node, two edges.
└── seed-ref/                  the OTHER root: stage one of a built colony, see below
    ├── colony.json            substrate defaults. two lines.
    ├── .env.example           every name the shell needs, and not one value
    ├── main/config.json       type: "hive", ONE edge, and not one cell
    └── main/os/config.json    the declaration: {"cell": {"type": "ref", …}}
```

The five `grow-*.json` are **five files**, and none of them is a cell. There is no door in here, no terminal, no
agent, no memory, no screening and no persona -- every one of those arrives from `templates/`,
at runtime, into a colony that is already up.

Two of the four templates were extracted out of this folder to make that true: the
[`door@1.0.2`](../../templates/door/) that names the ingress lane, and the
[`terminal@1.0.1`](../../templates/terminal/) that every undecided lane ends in. They used to be
"the two cells a library cannot ship". They turned out to be the two cells a library *should*
ship -- generic, ten lines each, and needed by every tree.

**This `colony.json` does not opt in to `mutation_receipts`, and that is deliberate.** The key
turns every committed mutation into a message at the hive it names (GH #553), and this tree grows
no consumer for it: there is no `colony-view` app here -- `canvy` in step four draws its own
picture from its own read -- and no tools hive wired into a collector. An opt-in would be a key
with nothing behind it. `examples/organism` and `examples/display-colony-view` do opt in, because
each grows something that listens; what they pay for it is one **`no_route` dead letter at the
first empty container**, because the boot receipt of an empty seed has nowhere below it to go
yet. That row is the documented, expected one -- it names its sender and its trace, which is what
a dead-letter queue is for (see `CHANGELOG.md`).

## What grows

`grow.json` names four templates and draws the edges between them:

| node | from template | what it brings |
|---|---|---|
| `/door` | [`door@1.0.2`](../../templates/door/) | 1 cell. `POST /messages` becomes a turn on the ingress lane, carrying the channel identity. |
| `/firewall` | [`firewall@2.3.0`](../../templates/firewall/) | 4 cells. Size cap, sender rules, rate limit -- every verdict a comparison or a clock, never a model -- plus a hardline layer no rule row can lift and the custody of a turn parked for a person. |
| `/talky` | [`talky`](../../templates/talky/) | 11 cells. Session keeper, context collector, tool dispatcher, answer splitter, and an `llm` brain, with every internal edge pre-wired. |
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

Reload the registry. Fourteen cells.

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

Nineteen cells. That is the second half of the claim, and it is deliberately a **separate
declaration** rather than five more lines in `grow.json`: the first file answers "what is this
agent", the second answers "what does it think with", and the two are different decisions with
different models behind them (`MODEL_BRAIN` vs `MODEL_CORE`). Splitting them also shows the part
a single big file would hide -- that a running tree can be grown mid-life.

A `talky` is a **channel voice**: one identity, one tonality, one session store per channel,
fast models, and the job of keeping a conversation flowing. A [`cogny`](../../templates/cogny/)
is the **agent core**: core knowledge, core personality, thinking models that may take their
time, and the heavy reasoning and tool work. One core, N voices -- siblings, never nested.

The talky reaches it like a tool and it answers like an event, so the connection is two edges
and one knob (`consult_cogny` in the talky dispatcher's `handoff_tools`, an `override_params`
entry since `dispatcher@1.2.0` -- a handoff is
async *and* says the answer comes from a later turn, which is what lets the round the consult
leaves behind end without a sentence of its own, GH #372). Both edges clear
`col_phase` (the errand leaves another collector's chain mid-assembly and would be refused with
it still set), the ingress promotes `consult_id` to context (single-hop correlation has to
survive the core's whole chain and come home) **and `session_id`** (the core's `in_turn` lane
requires it, because its memory tool asks about sessions), and both restore the TTL -- an errand
is a fresh journey, not the tail of the turn that started it.

**One ingress edge since `cogny@4.4.0`** ([#528](https://github.com/mmeyerlein/meclaw/issues/528)).
Until then there were two, because the core had two lanes: `consult_cogny` took the thinking
lane and a second errand name, `ask_memory`, took a fast one. Both are gone. A quick memory
question belongs to the **talky**, which already holds the window and can ask its own
`memory_recall` in one call -- routing it through the core was the round trip that measurement
was about. What is left here is one class of errand and one lane, and the boundary between "ask
the core" and "look it up yourself" is a sentence in the schema the core hands out itself.
`MODEL_CORE_FAST` went with the lane and is no longer needed in the environment file.

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
- **Composition is topology, not framework.** All fourteen cells came from four templates that
  know nothing about each other. What connects them is four edges in one file.
- **Enforcement is code, phrasing is agentic.** The firewall's verdicts are comparisons; the
  agent never sees a turn it rejected.

Three honest limits: both outbound lanes that ARE wired end in one terminal here (a real
tree splits them); the `turn_write` lane has no memory hive to write into in this
distribution -- it is wired so you can see the turns leave, one message per turn, as they are
said; and three routes the composites still emit match **no edge in this file** at all --
`write` when a session closes, plus `reject` and `error` since GH #284. Since `talky@4.3.0`
(GH #447) nothing inside the composite consumes `write` either -- the closed batch only
leaves, and here it ends nowhere. Naming undecided lanes is this example's whole virtue, and these three it does not
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

`grow-argus.json` adds the [`argus@1.1.0`](../../templates/argus/) — seven more cells that
read a charter, measure this colony out of its own ledger, have a model judge and simulate
against those numbers, send the decided change to the cell it names, verify, and then keep the
change or revert it against a plan authored beforehand. Every cycle writes a receipt.

It arrives inert. Every goal in the charter ships `enabled: 0`, so a grown argus measures
nothing and changes nothing until somebody turns a row on — the only defensible default for a
loop that reaches into the tree it runs in.

And it arrives **unable to act**, which is the more interesting half. The decided change
leaves the hive on the `mutate` lane as an ordinary **params update** — body
`{"system": {}, "params": {…}}`, with `hop.target` naming the cell it belongs to (GH #304) —
so acting means having an edge to that cell:

```json
{"from": "./argus", "to": "/some-llm-cell", "condition": "has(hop.route) && hop.route == 'mutate' && hop.target == '/some-llm-cell'"}
```

That edge is not in the declaration, and the argus cannot draw it: an edge is a mutation,
and **nothing in this hive authors one**. Granting the loop a target is a **boot-time act** —
a human puts the edge in the seed, one per cell the loop may touch. No amount of growing gets
around it, and the bound is per-cell rather than all-or-nothing: an edge on to
`/colony/mutations` would once have handed it the whole tree at a stroke.

**This colony grows it no target at all**, and not by omission. Both agents here are sealed
hives (`talky`, `cogny`, `params.ports: []`), so `/talky/brain` is not an address from
outside — an endpoint reaching past a seal is refused with `hive_port_boundary`. Giving this
argus something to change means adding a cell it may address, not drawing an edge into one
that already exists.

Two more bounds sit behind that, and neither depends on the argus behaving. The target
decides what it merges: an `llm` cell accepts `model`, `temperature`, `max_tokens` and the
other runtime-mutable keys, and refuses the credential and gate keys outright
(`IMMUTABLE_PARAM_KEYS`). And the radius stops before the cell types that have no params lane
at all — a `code` cell's numeric cap, like the collector's `max_iter`, comes back as
`key_outside_radius_<key>` with a receipt, rather than as a change nobody applied.

Note the shape of the endpoint on the way out: it is the **hive**, not a cell inside it.
`argus@1.1.0` is sealed (`params.ports: []`), so `./argus/mutator` is not an address at all
any more — a caller asks for the `mutate` lane and never learns which cell produces it. The
other lane the hive offers, `error`, would be drawn at the hive for the same reason — but
this declaration draws it nowhere (GH #284). An argus whose `error` ended in the sink would
be a loop that measures a colony and files its own failures in a bin; with no edge, a failed
cycle is a dead letter naming `/argus`, and that is a thing somebody can act on. So
`grow-argus.json` is one node and **no edge at all**: the whole step is "this hive now
exists here", and every connection it could have is a decision left to whoever grows it.

So the argus you grow here measures, judges and receipts, and changes nothing. That is a
legitimate way to run it — a good one for the first weeks, in fact, because the receipts tell
you what it *would* have done before you let it do anything.

## Step four: the colony draws itself

`grow-canvy.json` adds [`canvy@2.2.0`](../../templates/canvy/) — a timer, two `code` cells and a
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

## Step five: the front door

`grow-operator.json` adds [`operator@1.2.0`](../../templates/operator/) — a sealed hive that
turns a request from outside into a message with a **sender**, and that since GH #556 carries
the **submitter** as one of its occupants.

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/meclaw-os/grow-operator.json
```

**Why it exists at all.** The substrate stamps `envelope.reply_to` on a *cell's* emission. An
agent inside a colony is a cell, so its submissions are attributable; a person with a shell is
not, so a `POST /messages` arrives with no sender and the submitter's gate refuses it as
anonymous. This hive lends that person a path: a request on `in_submit` becomes a message
emitted by `/operator/intake`, and that is the identity the rest of the colony sees.

**It is identity, never authentication.** No token, no header check, no caller list, no secret
— the same story the display tells one section up. Who may reach the door is a reverse proxy's
question; what the holder of an identity may do is the capability broker's.

**One node, two edges — and one existing edge narrowed.** The way in is
`./door -> ./operator`, taken when the channel is literally `operator`, stamping the
`in_lifecycle` lane. The way out is `./operator -> ./sink` on `receipt`. And
`./door -> ./firewall` grows one clause, because edges **fan out**: without it a turn on that
channel would reach the agent as well, and an operator request is not a conversation.

**Where the round stops here, and why that is the honest wiring.** Through `operator@1.0.0`
the third edge was `./operator -> ./sink` on `apply`, because the submitter stood outside the
hive and the manifest had to leave it. Since GH #556 the submitter is an occupant: `apply` is
an interior edge and never crosses this rim. What crosses instead is `ask`, the one capability
question a submission asks — and **this colony grows no broker**, so a lifecycle request
becomes a manifest, reaches the gate, asks, and stops there. The receipt an operator gets is
the one the front door renders; nothing is applied, and nothing is lost silently. A colony
that wants the round to finish wires `ask` to a broker, `in_verdict` back, and `mutate` on to
the mutation door — which is exactly the shape
[`meclaw-os@1.8.2`](../../templates/meclaw-os/) ships, and the reason a shell is the thing you
grow when you want an OS rather than an agent with a door.

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d '{"target":"/door","body":{"messages":[{"origin":"user","type":"text",
          "text":"hello"}]},"context":{"channel":"operator"}}'
```

A plain sentence is not a lifecycle request, so what lands at the sink is a receipt saying
so — `hop.error_code = 'unknown_lifecycle_op'`, naming the three words the lane does take.
That is the whole behavioural point of this hive: a request it cannot serve comes back as an
**answer**, in the same round, instead of localising itself in a queue. The same holds one
level up, for a lane no occupant serves at all: `unknown_route`, naming the lane verbatim.

## The other root: stage one of a built colony

Everything above grows a colony **by hand**: five declarations, each written by a person who
knew which four edges to draw. That is the right way to *learn* the substrate, and the wrong way
to build a real one, because the fourth or fifth of those files is where a human stops being
better than a machine at drawing fifteen transit edges the same way every time.

A built colony arrives in two stages instead.

**Stage one is one declaration**, and it is `seed-ref/`:

```
seed-ref/
├── colony.json            substrate defaults. two lines.
├── main/config.json       type: "hive", ONE edge, and not one cell
└── main/os/config.json    {"cell": {"type": "ref", "template": "meclaw-os@1.8.2"}}
```

```bash
cp examples/meclaw-os/seed-ref/.env.example examples/meclaw-os/seed-ref/.env
# ...then put a real value on the one line that has none

./target/release/meclaw --root ./examples/meclaw-os/seed-ref \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7778
```

The third file is a **declaration, not a cell**. The first start resolves it against the
template library and grows it — the capability broker, the control loop, the baumeister, the
submitter, the front door, the empty `orgs` container and the forty-eight edges between them —
through the very resolution and staging a mutation takes. Then the marker is **gone**: what stands at its
address is [`meclaw-os@1.8.2`](../../templates/meclaw-os/). A second boot finds nothing to grow.

**The one edge is the whole birth topology.** `./os -> /colony/mutations`, on the `mutate` lane
and nothing else. It cannot be added by a mutation on any scope — an edge *is* a mutation — so it
lives in the root tree or the colony can never change itself. A `ref` marker declares a NODE and
never an EDGE, which is why that one line is written by hand and why it is the only one.

**And it is read, not generated.** A script that writes a colony root before the first boot
produces a tree nobody can diff, and the two things that decide everything — the marker and that
edge — are exactly the two it gets wrong in silence. So the root tree is three checked-in files.
The root seed is a template, never a script.

**What the shell says it needs, before it runs.** `templates/meclaw-os/template.json` §
`requires.env` names every environment variable any config value under the shell substitutes.
Exactly one of them has no default — `OPENROUTER_API_KEY`, the control loop's judge — and a boot
without it is refused with `requirement_missing` **before a single byte is staged**: the marker
is still a marker, and the refusal quotes the declaration's own sentence. The `.env.example`
beside the seed carries every name and not one value.

**Stage two is not a file in this folder.** Once the shell stands, the authoring path inside it
is what grows the rest: a wish reaches the baumeister, the baumeister drafts a manifest, the
submitter carries it to the mutation door under the requester's own identity, and a receipt comes
back. The five `grow-*.json` above are what a stage-two manifest *looks like* — which is exactly
why they are worth reading — but in a built colony nobody types them. Since
[`builder@1.2.0`](../../templates/builder/) the baumeister renders a level's transit edges from a
recipe rather than generating them token by token
([GH #466](https://github.com/mmeyerlein/meclaw/issues/466)), so a declaration of this shape is
an *output* there, not an input.

**Stage two, from a shell, in two acts.** The rim lane `in_build` is what an operator posts a
wish on, and since
[GH #474](https://github.com/mmeyerlein/meclaw/issues/474) the answer is a **draft** rather than a
change:

```bash
# act one: ask. Nothing is applied.
curl -s -X POST http://127.0.0.1:7778/messages -H 'Content-Type: application/json' -d '{
  "target": "/os", "hop": {"route": "in_build"},
  "body": {"messages": [{"origin": "user", "type": "text", "id": "",
    "text": "{\"request\": \"grow an org named acme from org@1.4.0 under /os\", \"scope\": \"/os\"}"}]}}'

# -> receipt, hop.draft_state 'draft_ready', hop.manifest_sha256 <digest>,
#    hop.draft_path /os/operator/drafts, body.manifest the declarations verbatim.

# act two: say yes, by quoting the digest and nothing else.
curl -s -X POST http://127.0.0.1:7778/messages -H 'Content-Type: application/json' -d '{
  "target": "/os", "hop": {"route": "in_submit", "manifest_sha256": "<digest>"},
  "body": {"messages": []}}'
```

The second act carries **no manifest**: the bytes are read back out of the front door's `drafts`
store under the digest that was shown, so a yes is a yes to what was read. A digest nothing is
parked under answers `digest_mismatch` and submits nothing. A caller that genuinely wants one act
— a rebuild script replaying wishes somebody has already read — says so in the wish itself with
`"auto_submit": true` beside the route, and the draft goes straight to the submitter the way it
did in 1.5.0.

**Pinned.** `crates/meclaw-cells/tests/gh465_one_declaration_boots_the_os.rs` boots this seed
against the shipped library, measures the grown tree against the template tree, asserts the
dead-letter queue is empty and the marker consumed itself, and drives the refusal case with the
required key absent — asserting on disk that nothing was written. It is the only proof stage one
may cite.

The two acts are pinned by `crates/meclaw-cells/tests/gh474_a_draft_waits_for_a_yes.rs`.
