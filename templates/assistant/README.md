# `assistant@1.1.0`

One generation of one person's agent. Two occupants, one open container, and
twenty-one edges.

| what | it is | why it is at THIS level |
|---|---|---|
| `cogny` | a `ref` to [`cogny`](../cogny/README.md) — the reasoning core | every channel of this assistant consults the same second opinion; two cores would be two opinions |
| `tools` | a `ref` to [`tools`](../tools/README.md) — the tool surface, one node with one contract | every channel calls the same tools; replacing all of them is one `swap_nodes` and no edge of this level moves |
| `channels` | a real, **empty, open** container hive | the surfaces stand here — and because it is a node of this template, the eighteen edges that address it ship **once** (GH #303) |

## a level owns what its siblings must share

That is the rule all four composition levels — `meclaw-os`, `org`, `member`,
`assistant` — are authored under, and it is the only test that decides what
belongs here. Ask it of anything you are tempted to add: do *all channels of
this assistant* share it? A reasoning core, yes. A tool surface, yes. A memory,
**no** — memory belongs to the member (GH #122), and it must survive a
generation swap. A firewall, **no** — the screen sits outside the generation so
that two channels of one person meet one view of an attacker and the rate window
does not restart because the agent was replaced.

## The four-segment shape, before and after

GH #303 dissolved a whole path level. What used to be

```
.../assistants/<agent>/channels/channel/telegram-connector/proxy
```

is now two nodes side by side, paired by edges:

```
.../assistants/<agent>/channels/<connector>
.../assistants/<agent>/channels/<talky>
```

`telegram-connector` is **one cell** — it has no hive around it any more,
and it is addressed as the cell it is. `talky` is a sealed hive and is
addressed by path and lane. What binds them is one edge (`<talky> → <connector>`
on `answer`), not a shared directory.

**Identity comes from the edge, never from the body.** A generation swap
replaces the talky at the `channels` level and the older one stays in place,
silent and readable. No statist is created to satisfy a contract.

## What it ships

```
assistant/
  config.json            the level: thirteen lanes, two drain pairings, twenty-one edges
  cogny/config.json      a ref to cogny, at the version its because names
  tools/config.json      a ref to tools, at the version its because names
  channels/config.json   an open, empty container -- no cells, no ports, NO CONTRACT
```

## The container declares nothing, and that is the rule

`channels` ships with **no `params` block at all**. Both halves are decisions:

- **No `ports`** — a sealed hive refuses an `add_edges` endpoint below its own
  path with `hive_port_boundary`, and the pairing edge between a connector and
  its talky *is* such an endpoint. Sealing this level would forbid exactly the
  wiring the level exists for.
- **No `contract`** — and this is the sharper one. `check_lane_doors` skips a
  hive only while *nobody* addresses its path (`hive_path_is_wired`). This level
  addresses `./channels` on eighteen of its own edges, so the container is wired
  from the moment the assistant is instantiated. From then on every declared
  `accepts` lane owes a door to a cell **inside** the container — and an empty
  container has no inside. A contract here would refuse *every* mutation of the
  colony with `hive_contract` until the first channel stood there, and an
  assistant that has no channel yet is a legitimate intermediate state.

So the lanes are declared by the **level**, whose own edges satisfy the door and
exit check from birth. The container's transit list is prose in its own
`description` — what the instantiating mutation reads, not what the substrate
enforces against an empty directory.

**The unbound behaviour of `./channels` is undeclared**, deliberately: GH #285's
slot governs an address that does **not** exist, and this container does, so the
declared word could never fire. A message that reaches it before a channel is
instantiated takes the ordinary path. The measurement comes from
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps aside as soon as the target is a registered hive scope.

## Lanes

Fifteen, all at the assistant's own path. The level is **open**: it declares no
`params.ports`, because it is wired into.

| in | what travels |
|---|---|
| `in_turn` | a screened turn, normally the one this assistant sent up on `turn` and the member's firewall passed back. Promote `context.channel`, and `context.audience_set` if the closed sessions of this generation are ever to reach a memory |
| `in_bundle` | the member's memory answer |
| `in_advice` | an advisor's answer arriving as its own turn — the core inside answers on this lane too |
| `in_sweep` | an operator-forced session sweep |
| `in_prune` | a prune verdict for the context window |
| `in_round_sweep` | a round that ran out of iterations |
| `in_build_result` | the builder's answer on its way back to the tool round that asked — a draft manifest, or the receipt of one that was submitted. This level carries it down to the tool surface and reads nothing in it |

| out | what travels |
|---|---|
| `turn` | an inbound turn from a channel, on its way to the member's screen — **before** it is answered |
| `write` | a closed session as one write batch |
| `turn_write` | one finished turn per message, never a batch (GH #298) |
| `extraction` | the per-turn sidecar, for the member's memory hive `in_remember` door |
| `recall` | a memory read this turn needs |
| `prune` | the report of a window prune |
| `error` | a normalised failure from anything inside this generation |
| `build` | a structural wish leaving this generation, or a manifest being submitted by whoever drafted it. The one lane on which a tool of this assistant reaches OUT of the assistant — declared rather than hidden, for the same reason `sandbox_union` exists one level down (GH #425) |

Two pairings are declared in `params.required_drains`, both in the **lane** form:
`in_turn → error` (from the retired channel hive) and `in_prune → prune` (from
the `talky`). A parent that sends turns in and does not take the failures back
has built a generation whose every failure is a dead letter.

### Where the lanes come from

*A level declares the union of the lanes its occupants ship, minus the lanes a
sibling inside the level consumes itself.* Derived from `talky` and
`telegram-connector` in the container, `cogny` and `tools`
beside it, each at the version its `because` names. Four subtractions, every one of them a lane an occupant really ships:

- **`answer`** — the talky's, consumed by the connector on the per-channel
  pairing edge.
- **`tool`** — both the talky's and the core's, consumed by `./tools` through
  the guarded default below.
- **`tool_call` / `tool_result`** — the tool surface's own pair, both ends
  inside.
- **`in_tool`** — supplied by `./tools`, never from outside.

`in_memory_call` and `in_thread_call` are declared by the `talky` and are
deliberately **not** declared here: no occupant outside this level produces them,
and a declared lane with no door is `hive_contract` at the next mutation the
colony runs. GH #55 serves them inside the talky.

`extraction` routes **upward**, to the member's memory hive, exactly where
`talky`'s own recipe sends it — two edges, never one. The assistant grows no
memory of its own: under GH #122 the memory belongs to the member, and a second
store would force the writer to pick a store before extraction has run.

`crates/meclaw-cells/tests/gh302_assistant_wires_channels_once.rs` reads
`templates/talky/config.json`, `templates/cogny/config.json` and
`templates/member/config.json` off the tree and fails until this level moves with
them.

## One edge to the tool surface, and it names no tool

```json
{"from": "./channels", "to": "./tools", "default": true,
 "condition": "has(hop.route) && hop.route == 'tool'",
 "modifier": {"set_hop": {"route": "'tool_call'"},
              "set_context": {"tool_caller": "'channels'"}}}
```

That is the **#286 + #283 win, measured.** The exclusion this replaces named
every tool on the live tree — nine terms, hand-kept in sync with nine positive
edges. #286 put the tool surface behind one contract, which reduced it to two
errands that are not tools at all. #283's guarded default removes the last two:
the two consult errands stay **ordinary** conditioned edges on `hop.tool_name`,
so a consult fires a regular edge and the default stays silent, while a real
tool call fires nothing regular and the default carries it. **Nothing on this
edge names a tool or an errand any more, and adding a tool touches nothing
here.**

> **Suppression is per SENDER.** If *any* regular out-edge of `./channels` fires,
> the default is silent. Every other edge out of `./channels` is conditioned on
> something a `tool` message does not carry — the seven outward lanes, and the
> two errands by name — and there is **no unconditional tee**. If this set ever
> grows a logger, a tap or a mirror without its own route condition, the tool
> surface goes dark for every call. The requirement is written into the config's
> own `because` and gated by
> `no_regular_out_edge_of_the_channels_level_is_unconditional`.

The two callers of the one tool surface are told apart on the way back by
`context.tool_caller` — context, not hop, because the hop decays at the next
cell and the answer comes back through two of them.

### The consult edges

Written exactly as `cogny`'s own recipe prescribes, and both read the **lane
before the discriminator** (driver ruling W7-R4): an answer travels back through
the very path the dispatch left from, and a door that asks only about
`hop.tool_name` hands an answer to its own sender until the TTL runs out.

**`consult_cogny` and `ask_memory` belong in `DISPATCHER_HANDOFF_TOOLS` on the
talky side** (GH #372). They are not synchronous tool calls: an advisor's answer
arrives as its own turn, and a consult wired as a tool call strands the round.
That is an env setting of this assistant's instance, not an edge.

## The measured fan-in: eighteen edges, not fourteen

#303 counted **14** edges between `channels` and its siblings on the live tree —
the reasoning core, four tool cells, the drain, the sink, and the assistant
itself. This template draws **18**, and the difference is not drift:

- the four tool cells became **one** `tools` hive (#286) — one edge out, one back;
- the sink is **gone** (ruling Q2, GH #284) — nothing swallows a refusal any more;
- the drain is **gone** — per-turn extraction replaced it (#298, ruling Q11), and
  `extraction` now leaves on a lane of its own;
- what grew instead is the **lane surface** the level carries in its own right:
  six doors down from the assistant path and seven exits back up, because the
  channels level now owns lanes that used to be wired per instance.

Six + seven + two consult + one tools + one advice back + one tool result back =
**18**, and the count is asserted against the live edge table in
`a_second_channel_adds_no_edge_between_the_channels_level_and_its_siblings`.
A second channel does not move it — that is the whole of #303's ruling.

## Instantiating

One `add_nodes` into a member's `assistants` container, with the transit lanes in
the same mutation. The hive is an island until an edge crosses into it.

```json
{"scope": "<member>/assistants", "diff": {
  "add_nodes": [{"name": "agent", "template": "assistant@1.1.0"}]
}}
```

The member's own edges already carry `in_turn` and `in_bundle`
down into the container and take `turn`, `recall` and `extraction` off it. The
other four outward lanes — `write`, `turn_write`, `prune`, `error` — cross the
member and are the parent's to drain.

### Adding a channel

Two nodes and their edges, in one mutation. Every endpoint is `./channels`
itself or something **below** it, which is why the container has to stay open —
and none of them is an edge between `./channels` and a sibling.

The mutation is scoped to the **assistant**, not to the container: a node is
addressed by its `name` plus the scope, and the name carries the `/`. Endpoints
are scope-relative, always. Scoping to `<assistant>/channels` and then writing
an absolute endpoint (`<assistant>/channels/tg`) is refused with
`scope_out_of_bounds` before anything else is looked at, and scoping there and
writing `"to": "."` is refused with `edge_schema` — `.` names no node.

```json
{"scope": "<assistant>", "ctx": {"model": "<the brain's model>"}, "diff": {
  "add_nodes": [
    {"name": "channels/tg",    "template": "telegram-connector@2.0.1"},
    {"name": "channels/talky", "template": "talky@4.2.3"}
  ],
  "add_edges": [
    {"from": "./channels/tg", "to": "./channels",
     "condition": "!has(hop.error_code)",
     "modifier": {"set_hop": {"route": "'turn'"},
                  "set_context": {"channel": "'telegram'",
                                  "chat_id": "hop.chat_id",
                                  "user_id": "hop.user_id"}}},
    {"from": "./channels/tg", "to": "./channels",
     "condition": "has(hop.error_code)",
     "modifier": {"set_hop": {"route": "'error'"}}},
    {"from": "./channels/talky", "to": "./channels/tg",
     "condition": "has(hop.route) && hop.route == 'answer'"}
  ]
}}
```

plus one edge per lane the level carries **down** to that talky (`in_turn`,
`in_bundle`, `in_advice`, `in_sweep`, `in_prune`, `in_round_sweep`, and `in_tool`
coming back off the tool surface), and one per lane it carries **up** (`write`,
`turn_write`, `extraction`, `recall`, `tool`, `prune`, `error`). Seventeen edges
for a channel — and the eighteen above do not move. The `ctx` is not decoration:
`talky` declares `model` as a required ctx key, so a mutation without it is
refused `requirement_missing`. The whole thing written out, with every lane
named, is [`examples/organism/grow-channel.json`](../../examples/organism/grow-channel.json).

The connector emits **one wire**: an emission carrying `hop.error_code` is the
connector's own failure, one without it is an inbound turn. Normalising the two
onto `turn` and `error` is the level's job since `telegram-connector@2.0.0`, and
the two edges above are that normalisation. The outbound edge must promote
`hop.chat_id` to context, or the reply has no chat to go to. One `getUpdates`
consumer per bot token: a second poller on the same token gets 409 and the two
steal each other's updates.

## Not in scope

- **No memory hive and no memory-drain.** The memory belongs to the member
  (GH #122, ADR 0012); per-turn extraction replaced the drain (#298, ruling Q11).
- **No firewall.** The screen sits outside the generation (GH #302).
- **No identity.** `affinity` is the member's — memory produces, affinity decides.
- **No terminal and no sink of any kind** (ruling Q2, GH #284). `error` leaves
  the level, and if nobody consumes it, it becomes `no_route` in the DLQ:
  recorded and self-localising.
- **No tool schemas and no per-tool credentials.** A schema lives in the calling
  brain's `system.tools`; a credential in the cell that needs it.
- **No connector and no talky inside the container.** A channel is instantiated,
  never shipped.
- **No memory leg for the reasoning core.** At this boundary the surface's recall
  leg and the core's would collapse onto **one** lane pair, and the bundle coming
  back carries nothing that tells them apart. `cogny` ships `memory_tier`
  empty and asks for no recall unless an instance turns the leg on; an instance
  that does must wire that leg itself and pick its own correlation key.

## Versioning

`1.0.0` is the first shipped version. This level's lanes are a public contract:
dropping or renaming one is breaking for every parent that wired it, and moves
the first digit. Adding a lane nothing ever promised is additive and takes the
second; a repair takes the third. The occupant pins in `cogny/config.json` and
`tools/config.json` are version-pinned on purpose — a bare name resolves to the
highest version present, which is the drift `registry.template_chain` exists to
make visible.
