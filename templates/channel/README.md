# `channel@1.0.0`

One channel as one hive. Inside it: the connector that owns the chat's credential, and
one slot that the channel's current agent occupies.

**The hive IS the channel** -- the identity that stays. The talkies inside it are its
**generations**: one active, the older ones silent and preserved. A generation ends when
the **participant set** changes, which makes the set of people who were present a
constant of a generation's lifetime rather than a roster somebody has to maintain
(ADR-0002, E7 and E8). Nothing here curates that set, nothing here ages it out, and
nothing here can get it wrong, because it is never written down twice.

## Cells

| path | type | from |
|---|---|---|
| `telegram-connector` | hive + `proxy` | [`telegram-connector@1.0.0`](../telegram-connector/) **(sealed)** |
| `terminal` | `code` | [`terminal@1.0.0`](../terminal/) -- the generation slot, still empty |

Both are **byte copies** of their templates: the substrate has no template-in-template
reference, instantiation is a recursive directory copy, and a nested `template.json`
would only register a second template with the scanner. The copies are pinned rather
than hoped away -- `crates/meclaw-cells/tests/channel_composite.rs` fails if one of them
drifts from its source.

Swap the connector sub-unit for another provider's and the rest of this template is
unchanged: the lanes it speaks (`in_reply` in, `turn` and `error` out) are the connector
contract, not a Telegram detail.

### The slot is a terminal, and that is the whole trick

A hive template cannot carry a door to a node it does not have -- a `params.graph`
endpoint that resolves to nothing is a loud boot failure, not a warning. So a channel
that shipped with "the generation goes here, wire it later" would ship with a contract
that is false until somebody finishes it.

`terminal@1.0.0` is the cell whose entire job is to be an address: it accepts anything
and emits nothing, so a lane that has no destination yet still HAS one. Occupying the
slot with it makes every lane of this contract true from the moment of instantiation. A
turn that arrives before the channel has an agent is swallowed and recorded in the trace
instead of dead-lettering, which is the honest behaviour for a room nobody has moved
into yet.

**The exits declared from the slot are the shape of the slot, not a claim that the
terminal speaks.** A terminal emits nothing, by design. What
`{"from": "./terminal", "to": "."}` states is where a lane goes *once a generation
occupies the slot* -- and the swap below moves those edges onto the generation verbatim.

## Lanes

`params.ports` is empty. **The address is the hive**, and what a caller wants rides on
`hop.route`. `./telegram-connector` and `./terminal` are not addresses from outside; an
edge naming one is refused with `hive_port_boundary`.

The inbound lane names are `talky@3`'s own, unchanged. A facade that renames the lanes
behind it is a facade you have to learn twice.

| lane | direction | what travels |
|---|---|---|
| `in_turn` | in | the screened turn coming back from the shared firewall. **The door promotes `context.channel_open_history`** -- see below |
| `in_reply` | in | something to say in this channel that the generation did not produce: a digest, an alarm, an operator's line |
| `in_tool` | in | one tool result coming back |
| `in_advice` | in | an advisor's answer coming back from an agent core |
| `in_bundle` | in | a memory bundle coming back |
| `in_memory_call` | in | a memory tool call handed back in -- see "Why not a self-loop" |
| `in_sweep`, `in_prune`, `in_round_sweep` | in | the three operator lanes |
| `turn` | out | the unscreened turn, for the **shared** firewall. `channel`, `chat_id` and `user_id` are already in `context` |
| `recall` | out | a memory read the active generation needs |
| `write` | out | the closed session as one write batch |
| `turn_write` | out | the same batch after every stored turn, off unless the generation switches it on |
| `tool` | out | a tool call; `hop.tool_name` says which |
| `error` | out | every failure of this channel, normalised. **MUST** be wired, and `required_drains` says so |

Three sorts arrive on `error`, on purpose: a reply the connector could not deliver, a
failed inference, and a round that hit its iteration cap. From outside a channel, all
three are the same event -- the room went quiet -- and a parent drains one edge instead
of three.

**The reply never leaves the hive.** The generation's `answer` goes straight to the
connector on an internal edge, guarded by `!has(hop.round_capped)`; the capped sort
leaves on `error` instead of being said out loud in the chat.

## Wiring one

Four edges in the parent scope, in the same mutation that instantiates the channel:

```json
{"from": "./channel", "to": "./firewall",
 "condition": "has(hop.route) && hop.route == 'turn'",
 "modifier": {"set_hop": {"route": "'in_turn'"}}},
{"from": "./firewall", "to": "./channel",
 "condition": "has(hop.route) && hop.route == 'pass'",
 "modifier": {"set_hop": {"route": "'in_turn'"}}},
{"from": "./channel", "to": "./sink",
 "condition": "has(hop.route) && hop.route == 'error'"},
{"from": "./firewall", "to": "./sink",
 "condition": "has(hop.route) && hop.route == 'reject'"}
```

Then, per channel, whatever the generation inside needs: `recall` out to the member's
memory and `in_bundle` back, `write` (and `turn_write`) out to a memory drain, `tool` out
to each tool cell and `in_tool` back. Every one of them is an edge at the channel's own
path -- the generation is not an address.

### The firewall stays outside, and this is settled

It is tempting to put the screening inside, one firewall per channel, and the argument
against it is not tidiness:

- **Sight.** A shared firewall sees an attacker who touches three channels at once.
  Three channel-local firewalls each see a third of the pattern, and none of them fires.
- **The rate window.** The firewall's `arrivals` table IS its rate limit. A firewall
  that lived one level lower -- inside a generation -- would start with an empty one at
  every generation change, so the limit would reset exactly when an unknown person joins:
  the trigger of the change would be the trigger of the reset (ADR-0002, finding B3).

The connector is the opposite case and lives inside for an equally hard reason: a bot
token tolerates exactly **one** `getUpdates` consumer, and a second poller gets
`409 Conflict`. It cannot sit in a generation, because waking an old generation to ask it
something would wake its poller too (B1) -- and it must not be shared across channels for
the same reason it must not be duplicated. **One channel, one connector, one token.** Two
channels means two tokens:

```json
{"name": "channel-2", "template": "channel@1.0.0",
 "override_params": {"telegram-connector/proxy": {"bot_token": "${TELEGRAM_BOT_TOKEN_2}"}}}
```

## Generations

### The first one

One mutation, scoped at the channel:

```json
{"scope": "/main/channel",
 "diff": {
   "add_nodes": [{"name": "talky", "template": "talky@3.0.8"}],
   "swap_nodes": [{"match": {"name": "terminal"}, "with": {"name": "talky"}}]
 },
 "ctx": {"model": "openai/gpt-4o-mini"}}
```

`add_nodes` puts the generation in the hive. `swap_nodes` **swings every external edge of
the slot onto it, atomically** -- the doors in, the exits out and the reply edge to the
connector, conditions and modifiers cloned verbatim. That single operation is what
redirects the ingress: nothing names the generation from outside, so nothing outside has
to be touched, and there is no window in which the lane hangs twice or not at all.

The existing-node form of `swap_nodes[].with` is what makes it one mutation: it may
forward-reference a node the same diff creates with `add_nodes`. The instantiate form
(`with.template`) cannot be used here -- it refuses subtree templates, and `talky` is one.

### The next one

A participant joins or leaves, so the generation ends (E8). Same shape:

```json
{"scope": "/main/channel",
 "diff": {
   "add_nodes": [{"name": "talky-2", "template": "talky@3.0.8"}],
   "swap_nodes": [{"match": {"name": "talky"}, "with": {"name": "talky-2"}}]
 },
 "ctx": {"model": "openai/gpt-4o-mini"}}
```

The old generation keeps its path, its `cell_id` and its `cell.db`, and loses every edge:
**disconnected and preserved**, which is the no-delete policy and E7's "silent and
readable" in one operation. Its window, its sessions and its summaries are still on disk
and still in the trace; what it no longer has is a way to be spoken to.

Instance names follow the rule the rest of the library follows: an instance is named
exactly like its template, and the second one in the same scope takes a suffix --
`talky`, `talky-2`, `talky-3`. The slot's occupant is always named after whatever
occupies it, which is why the placeholder is called `terminal`.

**One convention sits beside the rule, and only one.** A template whose name carries a
dash may write its own instance shortened to the tail after the last dash --
`memory-drain` -> `drain` -- and the shipped recipe in `memory-drain/README.md` does. It
is deliberately not global: the alias resolves only inside that template's own documents
([#238](https://github.com/mmeyerlein/meclaw/issues/238), and
`crates/meclaw-cells/tests/gh203_documented_port_addresses.rs` encodes exactly this).
Everywhere else -- another template's README, an example colony, a mutation -- the
instance is the full template name. Anything that is neither the template name nor its
dash-tail is drift, not a choice.

### Two things the swap does not do for you

**It clones modifiers verbatim, including the ones that were about the OLD generation.**
If the ingress door declares the participant set (see below), the new generation inherits
the old declaration, and that is exactly the thing that just changed. Re-declare it in a
**second** mutation, right after -- a `remove_edges` and an `add_edges` over the same
endpoints in ONE diff match over the post-state and take the new edge with them.

**It drops self-loops.** An edge whose two endpoints both swing onto the new node would
become `talky-2 -> talky-2` and is dropped rather than swung. Any lane a parent wired
from the generation back to itself is gone after the first generation change, silently.

**And one it does do for you, since GH #256:** it leaves the generation's *inside* alone.
A generation is a subtree, and the edges with which its hive serves its own cells
(`talky -> talky/session-keeper` and back) are internal, not external -- they stay with the
generation they wire. The old one is therefore disconnected *whole*, and a swing back to it
gets a working unit rather than a hollow one. Before that fix those edges were swung too,
so `talky-2` addressed `talky`'s cells and `talky`'s cells answered `talky-2`: one turn ran
through both generations. It was invisible on the FIRST change, because a `terminal` is a
leaf and has no inside to drag.

#### Why not a self-loop: the memory tool

`talky@3` serves its own `memory_recall` tool by sending to itself. Inside a channel,
wire that loop **through the hive path** instead -- out on `tool`, back in on
`in_memory_call`:

```json
{"from": "./talky", "to": ".",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'memory_recall'"},
{"from": ".", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'in_memory_call'"}
```

Two edges, neither of them a self-loop, both of them swung by the next generation change.
The second one is already in this template (it is the `in_memory_call` door); only the
first is per-instance.

## `channel_open_history`: the room's own policy

Whether a person who joins can read what was said before them is a property of the
**room**, and it has to be *declared*: the Telegram Bot API does not expose the setting
at all (ADR-0002, O1). So it is declared here, on the one edge every turn of this channel
crosses on its way in:

```json
{"from": ".", "to": "./terminal",
 "condition": "has(hop.route) && hop.route == 'in_turn'",
 "modifier": {"set_context": {"channel_open_history": "'0'"}}}
```

From there it rides in `context` through the generation and out on `recall`, where the
audience gate reads it as `context.channel_open_history` and applies the second, channel-
local clause of the visibility rule:

```
visible  <=>  the round now present is a subset of the audience it was said to
          OR  (the same channel AND that channel shows joiners its history)
```

The rule is: *the agent must not be leakier than the room -- and it need not be more
discreet than the room either.* The second clause never crosses a channel boundary; the
dangerous direction (a private two-person chat into a group) stays shut whatever this
flag says.

**The default is `'0'`, closed.** Three reasons, in order of weight:

1. The gate treats absence as false and every other value as false too, so `'0'` is the
   value that agrees with what a missing declaration already means. A default that
   disagreed with the fallback would make the same room behave differently depending on
   whether anybody had thought about it.
2. The two errors are not the same size. A room wrongly declared **open** hands a joiner
   a conversation that happened before they arrived, once, irreversibly. A room wrongly
   declared **closed** costs somebody a repeated question.
3. It cannot be detected, only declared. A default that assumed the permissive answer
   would be assuming the thing nobody checked.

**Opening a room is a mutation, not a knob**, and deliberately so -- it is a policy
change, and it leaves a line in the mutation log. Replace that one edge with the same
edge carrying `'1'`, in two mutations: the `remove_edges` first, the `add_edges` after.
Nothing upstream can open it: the door sets the key on every turn, so a caller that set
it on its own ingress edge is overwritten. The room decides, not the sender.

**Provenance is never rewritten.** A channel that opens its history later does not widen
the audience of anything already recorded -- the old rows still say who was present. The
policy lives in the rule, the data stay as evidence (E12).

### And the participant set itself

`context.audience_set` is the other half of the same gate, and it is the ingress door's
business for the same reason: the participants are a constant of the generation's
lifetime, so the set belongs on the edge that carries turns into the generation, written
once when the generation is made. This template does not put a set there -- it does not
know who is in your room -- and the write path refuses a turn that arrives without one
rather than storing it untagged.

```json
{"from": ".", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'in_turn'",
 "modifier": {"set_context": {"channel_open_history": "'0'",
                              "audience_set": "'[\"member:alex\",\"agent:scribe\"]'"}}}
```

Whoever declares it declares it on that door, and re-declares it on the door of the next
generation -- which is exactly the shape the swap does not do for you, two sections up.
See [`memory-hive`](../memory-hive/)'s contract for what the write path does with it.

**Declaring it on the door covers the close, too.** A generation is ended by the keeper's
night sweep, and a sweep carries no context of its own. Since `session-keeper@2.0.1` the
set declared here is written onto the generation row at the open and read back off it at
the seal, so the day that leaves on the `write` port carries the round it was spoken in
rather than the sweep's (GH #273). A door that declares nothing produces a generation
that closes untagged -- refused at the drain, not stored untagged -- and wiring the key
afterwards takes effect on the NEXT generation, never on one that is already open.

## What a colony still wires by hand

Instantiating this template gives you a channel with no agent and nothing behind it. What
is left is genuinely per-colony and the template refuses to guess at it:

- **A shared `firewall@2.0.0`** in front, with the `turn`/`pass` pair above and both
  `reject` and `error` drained.
- **The first generation**, with its model, its persona and its tool schemas -- the
  composite carries the topology, never the identity.
- **A memory**: `recall` out to the member's `memory-hive`, `in_bundle` back, `write`
  (and `turn_write`) out through a `memory-drain`.
- **Tools**: one edge pair each, on `hop.tool_name`.
- **The participant set** on the ingress door, and `channel_open_history` if this room is
  not a closed one.
