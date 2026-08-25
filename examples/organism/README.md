# examples/organism

An empty folder, the template library, and **five declarations**. Out of that: a colony
shell, an organisation, a person, one generation of that person's agent, and a Telegram
surface for it — **55 cells and 287 edges**, of which **48 edges were written by hand**.

`meclaw-os` is the example that grows *one agent* from templates. This one grows the whole
**stack**: four levels of composition, each instantiated into the level above it, each
bringing its own interior with it.

> **Measuring stick (GH #302, set 2026-08-19):** *a rebuild that takes ~30 minutes collapses
> to seconds.* The thirty minutes were not typing — they were instantiating a tree by trial
> and rejection, and re-copying every leaf bump up a chain of byte copies. Five declarations
> and one `curl` each is what is left of it.

## The rule

**A level owns what its siblings must share.**

All four levels are authored under that one sentence, and all four READMEs repeat it in the
same words. GH #302 gives three instances of it, taken from the layout that was already
running before the rule was written down:

- **memory belongs to the member**, because two assistants of one person must know the same
  person;
- **the firewall sits outside the generation**, because two channels need one view of an
  attacker and the rate window must not restart with a generation;
- **the reasoning core and the tools belong to the assistant**, the transport to the channel.

Read the four levels downward and the rule reads off them: the shell owns the capability
broker and the control loop (every organisation asks the same broker); the organisation owns
a name and a boundary and nothing else (a group is an audience, not a holder); the member
owns the memory, the curated record and the screen; the assistant owns the reasoning core and
the tool surface.

## What is checked in

```
organism/
├── seed/                      the --root of the colony. This is the whole tree.
│   ├── colony.json            substrate defaults. two lines.
│   └── main/config.json       type: "hive", and its graph is EMPTY
├── grow-os.json               1. the shell.      1 node,  0 edges
├── grow-org.json              2. an organisation. 1 node, 11 edges
├── grow-member.json           3. a person.        1 node, 11 edges
├── grow-assistant.json        4. one generation.  1 node,  9 edges
└── grow-channel.json          5. a Telegram surface. 2 nodes, 17 edges
```

**Zero cells.** Not a door, not a brain, not a store, not a screen — every one of them
arrives from `templates/`, at runtime, into a colony that is already up. That is the seed
principle of GH #26: a tree is grown, not checked in.

## What grows

```
/os                                 meclaw-os@1.0.0   the shell
├── access                            → access@2.0.5        the capability broker
├── steward                           → steward@2.0.10      the control loop
└── orgs                              (empty container)
    └── acme                       org@1.0.0         a namespace and a boundary
        └── members                  (empty container)
            └── alex               member@1.0.0      one person
                ├── affinity          → affinity@3.0.0      identity and meaning
                ├── firewall          → firewall@2.0.4      the screen
                ├── memory-hive       → memory-hive@3.0.1   what was said to them
                └── assistants        (empty container)
                    └── scribe    assistant@1.0.0   one generation of an agent
                        ├── cogny     → cogny@4.0.2         the reasoning core
                        ├── tools     → tools@1.0.0         the tool surface
                        └── channels  (empty container)
                            ├── telegram-connector   telegram-connector@2.0.0
                            └── talky                talky@4.2.0
```

Six `add_nodes` entries name six templates, and **thirteen** distinct templates end up stamped
on the registry's leaf rows — because a level is a composite and a composite resolves through
`ref`s. The four levels themselves carry no cell at all, so they never appear as a leaf stamp;
they appear in the **chain**. `<scribe>/cogny/collector/assemble` records
`[["assistant","1.0.0"],["cogny","4.0.2"],["collector","3.0.0"]]` — outermost first, its own
template last. An update addressing `assistant` finds that cell through the first hop, one
addressing `collector` through the last. That is the second acceptance bullet of GH #302, and
it is the question GH #277 could not answer at all.

## The five declarations, and why they are five

Each level is instantiated into the **open container** the level above ships for it — `orgs`,
`members`, `assistants`, `channels`. A container is a real hive with no children, no ports and
no contract, and its whole job is to be an address that already exists so the next mutation
has somewhere to put something.

That is also why these are five separate mutations rather than one. A container carries **no
`params.contract`** (driver ruling W7-R2), and it carries none precisely so that each level can
stand alone: a lane declared on a container would owe a door to a cell *inside* it, an empty
container has no inside, and from the first instantiation onwards that declaration would refuse
**every** later mutation of the colony until something stood there. An assistant with no channel
yet is a legitimate intermediate state, and this example is the proof — step 4 commits, step 5
is a separate act.

### 1. `grow-os.json` — the shell

```json
{"scope": "/",
 "diff": {"add_nodes": [{"name": "os", "template": "meclaw-os@1.0.0"}],
          "add_edges": []}}
```

One node, **no edges at all**. The shell is the outermost boundary: what reaches it comes from
outside the colony, and what leaves it leaves the colony. Everything under it — the broker, the
control loop, the nineteen edges between them and the `orgs` container — came with the template.

### 2. `grow-org.json` — an organisation

Instantiated into `/os/orgs`, with the transit lanes in the **same** mutation, because a hive is
an island until an edge crosses into it. Four doors down (`in_turn`, `in_recall`, `in_brief`,
`in_propose`) and seven exits back up (`answer`, `ack`, `reject`, `error`, `write`, `turn_write`,
`prune`). Eleven edges, and every one of them lands on the organisation's **own path** — never
on anything inside it.

```json
{"scope": "/os",
 "diff": {"add_nodes": [{"name": "orgs/acme", "template": "org@1.0.0"}],
          "add_edges": [{"from": "./orgs", "to": "./orgs/acme",
                         "condition": "has(hop.route) && hop.route == 'in_turn'"},
                        {"from": "./orgs/acme", "to": "./orgs",
                         "condition": "has(hop.route) && hop.route == 'answer'"}]}}
```

**The spelling matters, and it is the one thing worth copying exactly.** The scope is the level
*above* the container, and the node's `name` carries a `/` — the sanctioned way to name a node
one level below the scope. Writing `"scope": "/os/orgs"` and then `"from": "."` does **not**
work: a mutation endpoint is resolved as a node name, and `.` names none, so the whole diff is
rejected with `edge_schema` (`from='.' unknown`). Every declaration in this folder is written in
the parent-scope form for that reason, and all four levels read the same way because of it.

### 3. `grow-member.json` — a person

The same eleven lanes, one level down, into `/os/orgs/acme/members`. The member brings its three
holders and its own eighteen internal edges with it.

### 4. `grow-assistant.json` — one generation

Nine edges: two down and seven up.

Down are `in_turn` (the screened turn coming back off the member's firewall) and `in_bundle` (the
memory hive's answer). Up are `turn`, `recall`, `extraction`, `write`, `turn_write`, `prune` and
`error` — the member consumes the first three itself and passes the other four on.

**The four lanes that deliberately do not cross the member** (driver ruling W7-R5): `in_advice`,
`in_sweep`, `in_prune` and `in_round_sweep` are operator and timer traffic. Their producer is the
reasoning core inside the assistant, a second agent beside it, or an operator — and each of those
addresses `<member>/assistants/<agent>` **at its own path**. That is legal because neither the
member nor the assistant declares `params.ports`, so both are open, and the port boundary refuses
an outside endpoint only for a *sealed* hive. Housekeeping arrives at the agent, not through the
person.

The two doors are guarded on `context.agent`, because a member may hold more than one agent and
the container fans in to all of them. The channel promotes the name on the way up; a turn that
declares no agent reaches none of them and becomes `no_route` in the dead-letter queue —
recorded and self-localising, which is the honest state (2) of GH #284.

### 5. `grow-channel.json` — a Telegram surface

**Two instantiations, one mutation, no intermediate hive.** A channel is a connector and a talky
standing side by side in `channels`, plus the edges that pair them:

- the connector's **one wire**, normalised by the level it sits in: an emission carrying
  `hop.error_code` is the connector's own failure and becomes `error`, one without it is an
  inbound turn and becomes `turn` (`telegram-connector@2.0.0`, GH #303). The outbound edge
  promotes `hop.chat_id` to context, or the reply has no chat to go to;
- the pairing edge, `talky → connector` on `answer`;
- seven lanes carried down to the talky and seven carried back up.

Seventeen edges. **The eighteen edges between `channels` and its siblings are not among them** —
they belong to `assistant@1.0.0` and were drawn once, when step 4 ran. That is the whole of
#303's ruling, and a second channel does not move the number.

## A second agent, a second channel

Both are one instantiation with their own parameters, and neither re-runs anything:

```json
{"scope": "/os/orgs/acme/members/alex",
 "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}"},
 "diff": {"add_nodes": [{"name": "assistants/aide", "template": "assistant@1.0.0",
                         "override_params": {"cogny/brain": {"temperature": "0.9"}}}],
          "add_edges": []}}
```

— plus the same nine transit edges `grow-assistant.json` draws, with `scribe` read as `aide`
in both the endpoints and the two `context.agent` guards.

The member's own nine edges to and from its `assistants` container stay at nine. A second
channel likewise costs its own seventeen edges and nothing else.

## The numbers, measured

| what | how many |
|---|---:|
| cells checked in | **0** |
| cells after the five declarations | **55** |
| edges after the five declarations | **287** |
| edges written by hand in the five files | **48** |
| edges that came with a template | **239** |
| `add_nodes` entries | **6** |
| distinct templates stamped in the registry | **13** |
| edges between `channels` and its siblings | **18** |

Re-measure them with

```text
cargo test -p meclaw-cells --test gh302_the_stack_grows_from_templates \
    print_the_measurement -- --ignored --nocapture
```

Every hand-written edge in these five files lands either on a node the same declaration
instantiates or on the open container it is instantiated into. **Not one reaches into an
interior**, and that is the first acceptance bullet of GH #302, asserted in
`crates/meclaw-cells/tests/gh302_the_stack_grows_from_templates.rs`.

## Run it

```bash
# from the repo root, on a fresh release build
cargo build --workspace --release

cat > examples/organism/seed/.env <<'ENV'
OPENROUTER_API_KEY=sk-...
MODEL_BRAIN=openai/gpt-4o-mini
MODEL_CORE=openai/gpt-4o
MODEL_CORE_FAST=openai/gpt-4o-mini
MODEL_CLOSER=openai/gpt-4o-mini
MODEL_DIALECTIC=openai/gpt-4o-mini
MODEL_DREAMER=openai/gpt-4o-mini
TELEGRAM_BOT_TOKEN=...
ENV

./target/release/meclaw --root ./examples/organism/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7777
```

Open `http://127.0.0.1:7777/ui/registry`. **Nothing.** Now grow it, in order:

```bash
for step in os org member assistant channel; do
  curl -s -X POST http://127.0.0.1:7777/colony/mutations \
       -H 'Content-Type: application/json' \
       -d @examples/organism/grow-$step.json
done
```

Reload the registry. Fifty-five cells.

Every credential in this folder is a `${VAR}` token and never a value; the substitution reads
`{root}/.env` (or `--env`), not the process environment. A variable with no default has to
exist before the declaration that reads it will commit — `TELEGRAM_BOT_TOKEN` carries none, so
step 5 is rejected with `env_var_missing` until the line is there. Any value gets the mutation
through; a real one is what makes the bot poll. One `getUpdates` consumer per bot token: a
second poller on the same token gets 409 and the two steal each other's updates, so a second
channel wants a second bot.

The broker and the control loop start **inert by design** — every seeded policy row ships
disabled and every charter goal ships disabled — so a fresh shell grants nothing and changes
nothing until an operator turns on exactly what they mean.

## Not in scope

- **No sink.** Nothing here accepts a refusal and emits nothing (ruling Q2, GH #284): a lane
  with no consumer becomes `no_route` in the dead-letter queue, which is recorded and
  self-localising. What it must never have is a cell that accepts it and drops it.
- **No slot.** The substrate's slot governs an address that does **not** exist, and every
  container in this tree does exist — so the declaration would be silent, and the
  `params.ports` it needs would have *sealed* the level that declared it.
- **No second vault.** `access@2.0.5` carries its own interior one (ruling Q20).
- **No live migration.** This folder is a walkthrough for a colony that is grown from nothing.
  Running any of it against a deployed tree is a separate, operator-owned act.
