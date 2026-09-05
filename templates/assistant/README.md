# `assistant@2.5.0`

One generation of one person's agent. **Three refs, no container at all,** and
thirty-eight edges.

| what | it is | why it is at THIS level |
|---|---|---|
| `talky` | a `ref` to [`talky`](../talky/README.md) — the conversation surface that keeps this generation's sessions, calls its brain, splits the answer and raises the extraction | one generation has **one** session store; two would have to be told apart before a sweep could decide which one a closed session belongs to |
| `cogny` | a `ref` to [`cogny`](../cogny/README.md) — the reasoning core | every channel that reaches this generation consults the same second opinion; two cores would be two opinions |
| `tools` | a `ref` to [`tools`](../tools/README.md) — the tool surface, one node with one contract | every caller inside the generation calls the same tools; replacing all of them is one `swap_nodes` and no edge of this level moves |

Each pin is version-pinned in its own `config.json`, and each lane's `because`
names the version it was derived from.

## What `2.0.0` changed, and why it is the first digit

[#454](https://github.com/mmeyerlein/meclaw/issues/454) moved the **channels out
of the generation and up to the person.** What used to be an open `channels`
container — filled by one follow-up mutation per channel, each carrying a
connector and a talky of its own — is now one shipped occupant: the single
talky of this generation, wired to its two siblings by edges of this template.
It was called `surface` until 2.3.0; see *What `2.3.0` changed* below.

Two lanes moved with it, and both fall out of the derivation rule rather than out
of a decision:

- **`turn` was removed.** The raw wire that produced it is a connector, the
  connector now stands in `<member>/channels`, and a raw inbound message reaches
  the member's screen **without crossing this level at all.**
- **`answer` was added.** It used to be subtracted, because the connector that
  consumed it stood *inside* this level on the per-channel pairing edge. That
  connector is outside now, so the lane crosses.

Taking an address away (`./channels` no longer exists) and taking a lane away
(`turn`) is breaking for every parent that wired either. Neither of the two
additive rules of `docs/development-rules.md` § 4 covers a removal, so this
is the first digit.

[#303](https://github.com/mmeyerlein/meclaw/issues/303) is what #454 overtakes.
#303 dissolved the four-segment channel path and made the eighteen edges around
the container ship **once** instead of once per channel; #454 keeps that win and
removes the container the win was measured on. They address `./talky` now (see
*the measured fan-in* below), and more have been drawn since — the win was never
the number, it was that the number stopped multiplying by channel.

The pattern is the one **ADR 0013** states and
[#122](https://github.com/mmeyerlein/meclaw/issues/122) settled for the memory:
**a level owns what its siblings must share.** A channel is shared by every
generation of one person, so it is the person's.

## What `2.3.0` changed: the node is called after its template

[#545](https://github.com/mmeyerlein/meclaw/issues/545). A `ref` onto template
`X` is a directory called `X` — that is how nineteen of the library's
twenty-two ref markers are spelled, `./cogny` and `./tools` on this very level
among them. The conversation surface was the exception: a ref onto `talky`,
named `surface` after the ROLE it plays. The tree said one thing and the address
said another, and every reader had to learn the translation before they could
follow an edge.

The node is `./talky`. **Twenty-seven of this level's thirty-eight edges carry
the name**, and two stamped tokens are renamed with it, because a discriminator that
outlives the node it is named after is a word that has to be read historically:

- `context.recall_caller` now reads `'talky'` where it read `'surface'`
  ([#532](https://github.com/mmeyerlein/meclaw/issues/532)). Nothing outside this
  level compares against the value — `member`, `memory-hive` and `org` carry it
  through and test only `== 'outside'` — so the token is this level's alone.
- `context.tool_caller` likewise ([#464](https://github.com/mmeyerlein/meclaw/issues/464),
  [#529](https://github.com/mmeyerlein/meclaw/issues/529)). The return edges test
  `!= 'cogny'` and never resolve the other value against the tree.

**`ctx.model_surface` is deliberately NOT renamed.** It names the ROLE the model
plays for the level, which is exactly what a level's own ctx key should name; a
`cogny` has a brain too, and `model_talky` would say nothing about why the key
exists. It is also a public contract surface — `examples/organism/grow-assistant.json`,
`grow.manifest.json`, the librarian corpus and the builder's own README all name
it — and renaming a ref is no reason to move a key.

**The migration, in one line:** an `override_params` path or a mutation that
named `<assistant>/surface/...` names `<assistant>/talky/...` from 2.3.0 on.
That is the level's own address space, which is why this is the second digit and
not the third. A generation already grown from `2.2.0` keeps the node name it was
born with: the template library is not on the runtime path of a booted colony,
and a grown level is renamed, if at all, by a mutation.

## a level owns what its siblings must share

That is the rule all four composition levels — `meclaw-os`, `org`, `member`,
`assistant` — are authored under, and it is the only test that decides what
belongs here. Ask it of anything you are tempted to add: do *all parts of this
generation* share it? A reasoning core, yes. A tool surface, yes. A conversation
surface, yes — there is exactly one. A memory, **no**: it belongs to the member
(GH #122) and it must survive a generation swap. A firewall, **no**: the screen
sits outside the generation so that two channels of one person meet one view of
an attacker and the rate window does not restart because the agent was replaced.
And since 2.0.0, a **channel, no**:

- a bot a generation owned would be **one agent's** bot, and the person's second
  agent could not be reached through it;
- a bot a generation owned would have to be re-created on every swap, taking the
  chat account with it;
- a screen two of the person's agents both draw on would have **no owner at all.**

## What it ships

```
assistant/
  config.json            the level: twenty-two lanes, five drain pairings, thirty-eight edges
  talky/config.json      a ref to talky, at the version its because names
  cogny/config.json      a ref to cogny, at the version its because names
  tools/config.json      a ref to tools, at the version its because names
```

No container, no `params.ports`, no cell of its own. The level is **open**,
because it is a level that gets wired *into*: sealing it would refuse exactly the
endpoints the member's own container needs.

**The level is complete at birth.** Since 2.0.0 there is no per-channel follow-up
mutation, and therefore no intermediate state in which the generation is
instantiated but silent. That is the practical difference the removal bought, and
it is why the container's *"declare no contract, an empty container has no
inside"* paragraph is gone from this template: there is no empty container left
to reason about.

## Lanes

Twenty-two, all at the assistant's own path — plus **four that are not**: `in_pack`
and `pack_ack` ([#561](https://github.com/mmeyerlein/meclaw/issues/561)) and
`recall` and `in_bundle` ([#562](https://github.com/mmeyerlein/meclaw/issues/562))
name connect points (`at: ["./talky", "./cogny"]`) and dock on the two occupants
rather than on this rim. They are declared here all the same, because the
declaration is the permission: a deep edge may end on an occupant of this level
only where this level says it may, and an edge that tries to deliver one of them
AT this path is refused by name.

| in | what travels |
|---|---|
| `in_turn` | a **screened** turn, the one a channel of the MEMBER raised and the member's firewall passed back down. It arrives with `context.channel_node`, `context.channel`, `context.user_id`, `context.audience_set` and `context.assistant` already stamped — the member's container reads that last key, and nothing below it does |
| `in_bundle` | the member's memory answer, carried down to the occupant that **asked**. Since [#532](https://github.com/mmeyerlein/meclaw/issues/532) there are two of them and `hop.recall_caller` tells them apart: `cogny` reaches the reasoning core through its own door, anything else takes the **default** door to `./talky`. This level keeps no copy |
| `in_advice` | an advisor's answer arriving as its own turn. `./cogny` answers on this lane too; the lane stays outward-facing for the other case, a second agent that was asked something and answered late |
| `in_sweep` | an operator-forced session sweep outside the night timer |
| `in_prune` | a prune verdict for the context window, from a timer or an operator |
| `in_round_sweep` | a round that ran out of iterations, swept by an operator |
| `in_build_result` | the builder's answer on its way back to the tool round that asked — a draft manifest, or the receipt of one that was submitted. This level carries it down to the tool surface and reads nothing in it |
| `in_pack` | a durable `system.*` slot for this generation's brains: `identity`, `persona`, `handover` or `instructions`, and nothing else (the charter joined the list in [#488](https://github.com/mmeyerlein/meclaw/issues/488), which measured that nothing else ever wrote it). Since 2.4.0 this level does **not carry** the lane: it declares a **connect point** for it, `"at": ["./talky", "./cogny"]`, and the sender draws one **v-lane** per rim straight from its own `affinity` ([#559](https://github.com/mmeyerlein/meclaw/issues/559), [#561](https://github.com/mmeyerlein/meclaw/issues/561)). The fan-out is still a fan-out and still for the same reason — one generation is one person's agent, and its brains must not disagree about who that is — it is simply two edges the sender draws rather than two this level draws. **Paired**: see `pack_ack`. Since 2.1.0 (#458) |
| `in_tool` | the answer to a `memory_recall` call this generation made, coming back off the **member's own memory** ([#552](https://github.com/mmeyerlein/meclaw/issues/552)). It arrives at the level rather than at the asker, because the level is what knows which of its two occupants asked: `context.tool_caller` was stamped on the way out and the two entry edges read it |
| `in_menu` | that same memory's declaration of that tool, on its way to whichever occupant asked. It carries `context.tool_answerer == 'memory'`, which is what lets the asking collector MERGE a third answerer's declarations into its menu instead of overwriting them (#529, #552) |
| `in_export` | a demand for the one store this generation holds that the member cannot recompute: the session ledger inside `./talky`. Pure transit, no modifier — the lane is named the same on both sides. **Paired**: see `dump`. Since 2.1.0 (#475) |
| `in_import` | one part of such a document, for a generation that is already running. Same crossing, same pairing. WHICH generation a part belongs to is decided one level up, by the edge that addresses this assistant. Since 2.1.0 (#475) |

| out | what travels |
|---|---|
| `answer` | **what this generation said**, on its way back to the channel that asked. New in 2.0.0. The assistant does not know which channel it came from and must not: `context.channel_node` rode in on the turn and rides back out on the answer, and the member's own edge into `./channels` is what turns that name into an address (`context.channel`, the chat, rides along beside it — GH #522) |
| `write` | a closed session as one write batch |
| `turn_write` | one finished turn per message, after every stored turn and every stored answer — never a batch (GH #298, ruling Q11) |
| `extraction` | the per-turn sidecar, for the member's memory hive `in_remember` door |
| `recall` | a memory read this turn needs. **One lane, two askers** since [#532](https://github.com/mmeyerlein/meclaw/issues/532): the surface and the reasoning core, each stamping `context.recall_caller` with its own name on the way out |
| `prune` | the report of a window prune: one message per cut session, or a single zero report |
| `error` | a normalised failure from anything inside this generation — the surface or the reasoning core. A **channel's** failure is no longer among them: since #454 the connector stands in the member's `channels` container and its failures leave beside this lane, one level up |
| `tool` | a `memory_recall` call one of this level's two brains made, on its way OUT to the member's memory ([#552](https://github.com/mmeyerlein/meclaw/issues/552)). It is the **one** tool name that leaves: everything else this level can answer it answers inside, at `./tools` or at `./cogny`, and a named edge beside the guarded default is what takes this one out. The member is the mandatory hop, because it is the level that stamps the round a recall is asked in |
| `schemas` | the menu tick of an occupant, on its way out to that same memory (#552). Inside the level the same question already reaches `./tools` and `./cogny`; this is the **third** answerer, and it lives outside because a memory belongs to the MEMBER and not to one of its generations |
| `build` | a structural wish leaving this generation, or a manifest being submitted by whoever drafted it. The one lane on which a tool of this assistant reaches OUT of the assistant — declared rather than hidden, for the same reason `sandbox_union` exists one level down (GH #425) |
| `pack_ack` | the receipt one `in_pack` answers with — **twice** per pack, once per occupant, and that is the fan-out's arithmetic rather than a defect (`./cogny` answers once for both of its brains). A caller here counts occupants, not packs; the sender reads its own delivery off `hop.pack_owner` and `hop.error_code`, which every receipt carries. Joining the two would need a cell at this level to hold them, and this level holds no state of any kind. Since 2.4.0 it rides the road it came in on: the same `"at"` connect points, one v-lane per rim, back out to whoever drew the corridor — a v-lane is judged at BOTH ends, so the level that vouches for the push vouches for the receipt too (#561). Since 2.1.0 (#458) |
| `export_done` | the keeper inside `./talky` wrote its whole session ledger itself and says where: `hop.seed_dir` (relative to the fence its store declares), `hop.export_hive`, `hop.export_of`, `hop.rows_written`. Carried out of this level unchanged. Since 2.5.0 ([#555](https://github.com/mmeyerlein/meclaw/issues/555)) |
| `dump` | the receipt of one applied import part (`hop.rows_written`, `hop.export_final == "1"` on the last). Since #555 that is all this lane carries. **Drain it with a PLAIN `hop.route == 'dump'` test** — an edge that also tested a second hop key reads as no drain under the `required_drains` probe. The member does exactly that and carries it out of the level. A refusal never travels here: the surface normalises a porter refusal into `error` before it reaches this level. Since 2.1.0 (#475) |

**GH #552 moved four.** Until `assistant@2.5.0` the memory road crossed this level
only as `recall` / `in_bundle` — the AMBIENT leg, fired once per turn before the model
has seen it. The deliberate lookup, the one a model makes by name, was answered inside
`talky`'s own collector against a schema that template had typed by hand. It is the
member's memory's now: `tool` and `schemas` leave here, `in_tool` and `in_menu` come
back, and the level's job on that road is to remember which of its two occupants asked
(`context.tool_caller`, stamped on the way out, read by the two entry doors). Two traps
worth naming, because both are silent: `./cogny -> ./tools` had to become a `default`
edge in the same act — a named `memory_recall` edge beside an unconditional one delivers
every core tool call **twice** — and the `schemas` question now reaches **three**
answerers, which the menu merge of #529 carries because it keys its store table on
`tool_answerer`.

**GH #464 moved no lane at all.** `talky` and `cogny` each grew `schemas` out and
`in_menu` in, and neither crosses: `./tools` consumes the request and supplies the answer,
both inside this level, exactly as it does with `tool_call` / `tool_result`. So the
boundary was unchanged and the level carried four more edges instead — see *Two edges to
the tool surface*. **GH #529 is the same rule applied a second time**: the surface's
`schemas` also reaches `./cogny` and the core's `tool_schemas` comes back, both consumed
inside this level, so the boundary stands still again and the level carries two more
edges — see *The two answerers*. **GH #530 removes one**, the `ask_memory` errand, and an
errand name is an edge CONDITION rather than an address, so nothing a caller out here can
do is lost either. GH #475 is the opposite case and adds three: nothing inside this level
consumes `in_export`, `in_import` or `dump`, so all three cross, and the sessions of a
generation stop being the one thing a rebuilt member came back without.

Five pairings are declared in `params.required_drains`, all in the **lane**
form: `in_turn → error`, `in_prune → prune`, `in_pack → pack_ack`, and — since
#475 — `in_export → dump` and `in_import → dump`. A parent
that sends turns in and does not take the failures back has built a generation
whose every failure is a dead letter; a prune answers unconditionally, and an
operator who sends one without taking the report back cuts unwitnessed; a pack
answers unconditionally too, and a push whose receipt nobody takes leaves the
sender with a `sent_at` stamp for a delivery nobody can confirm; and an export
nobody drains is a walk that read the whole session ledger for nothing, while an
import receipt nobody takes is the only positive signal a transfer had.

**`in_turn → answer` is deliberately not a third pairing.** The answer's consumer
is a channel of the *member*, one level up, and an assistant instantiated before
its person has a channel is a legitimate intermediate state. The obligation is
real, but it is the member's: the member level ships the edge
(`./assistants -> ./channels` on `answer`) that discharges it, plus the guarded
default that catches an answer naming no channel.

### One hive, two askers (#532)

`recall` and `in_bundle` are **one** lane pair and always were. What
[#532](https://github.com/mmeyerlein/meclaw/issues/532) changed is that both occupants may use it: the conversation
surface and the reasoning core ask the same memory — the member's, because two
generations of one person must know the same person (GH #122) — and each gets
**its** answer back.

The mechanism is a reply-to token that changes compartment on the way home.

| where | what happens |
|---|---|
| `./talky -> .` and `./cogny -> .` on `recall` | the asker stamps `context.recall_caller` with its own name, `'talky'` or `'cogny'`. **Stamped, never defaulted** — the core's answer re-enters the surface on `in_advice` carrying the core's whole context, so a leg that only wrote the token when it was missing would post its own bundle through the core's door |
| the member, the `assistants` container, the memory hive | carry it untouched. Context is the only compartment that survives a hive: the `recall` cell forms its own hop (GH #411) |
| `./recall -> .` on `bundle` and on `reject`, inside the hive | hand it back on `hop.recall_caller`, off the context the question came in with |
| `. -> ./cogny` on `in_bundle` | the core's door, guarded on that **hop** key |
| `. -> ./talky` on `in_bundle` | the **default**, so an absent, empty or unknown token lands where every bundle landed before |

Why it has to arrive on the hop rather than stay in context: the template gate
probes a door with a bare `hop.route` and an **empty** context compartment, so a
door guarded on context alone can never fire under it and is condemned
(`crates/meclaw-cells/tests/gh173_shipped_hive_contracts.rs`). The inward
carve-out of W7-R1 (GH #286) is written for exactly this shape — a door reading a
hop key the probe cannot carry.

It is the arrangement this level already uses one door over: `context.tool_caller`
is what tells the surface's and the core's tool results apart on their way back
from `./tools`. The round trip is proven end to end, in both directions and twice
at once, in `crates/meclaw-cells/tests/gh532_two_askers_one_hive.rs`.

**Nothing an instance wires changes.** The container's `in_bundle` edge, this
level's `recall` exit and every shipped grow recipe keep working byte for byte,
and a bundle that carries no token still reaches the surface.

### Two legs, and only one of them is a tool (#535)

The pair above is used **twice by the surface alone**, and the two uses are not the
same question:

| leg | when | knob |
|---|---|---|
| the **ambient** leg | before the brain is called, every turn | `memory_tier` — `"1"` for the surface since [#535](https://github.com/mmeyerlein/meclaw/issues/535) |
| the **tool** `memory_recall` | when the model asks for it by name | no knob at all since [#552](https://github.com/mmeyerlein/meclaw/issues/552): the member's own memory declares the tool and answers the call, and this level draws the four edges that reach it |

`collector@` ships `memory_tier` empty and `talky` leaves it that way, which is
right for a `talky` standing on its own: no hive stands beside it, so a per-turn
`recall` would leave for an address nobody wired. This level is the other case
and the **only** place that can know it — it is instantiated into a member that
has a hive, and the member's edges already carry `recall` up and `in_bundle`
back down. Until the knob was set, a whole generation remembered only when its
model decided to ask, and the half whose job is to answer fast paid a second
round trip for what a cheap leg puts in front of the first call.

So the value stands on the ref marker in `talky/config.json`, as
`override_params["collector/assemble"].memory_tier`, beside the two overrides
that are there for the same reason and no other — the surface's model
([#516](https://github.com/mmeyerlein/meclaw/issues/516)) and its declared tool
list ([#529](https://github.com/mmeyerlein/meclaw/issues/529)). All three say one
kind of thing: a fact about the **composition** that the referenced template
cannot know. Neither occupant moves.

The reasoning core keeps `memory_tier` empty, and that is the split rather than
an omission: a problem solver asks about a time range or a session on purpose and
is not handed a bundle before it has read the question (`cogny@4.4.0`,
[#528](https://github.com/mmeyerlein/meclaw/issues/528)).

**The audience gate needs nothing new.** `memory-hive`'s `in_query` refuses a
request without `context.audience_now` and `context.channel` rather than
answering an empty bundle — and the member's own `./assistants -> ./memory-hive`
edge derives `audience_now` from the `audience_set` the ingress screened onto the
turn, on the very edge the tool call already travels. Worth stating because since
[#533](https://github.com/mmeyerlein/meclaw/issues/533) a refusal comes back to
the collector as `in_bundle`: a missing key would be a silently short bundle, not
an error, so the path is asserted rather than assumed
(`crates/meclaw-colony/tests/gh535_the_surface_carries_its_ambient_memory_leg.rs`).

### Where the lanes come from

*A level declares the union of the lanes its occupants ship, minus the lanes a
sibling inside the level consumes itself.* Derived from the `talky` at `./talky`, the
`cogny` at `./cogny` and the `tools` at `./tools`, each at the version its `because` names. **One**
subtraction since 2.5.0, and it is a lane an occupant really ships:

- **`tool_call` / `tool_result`** — the tool surface's own pair, both ends
  inside.

`answer` was one until 2.0.0, and `tool` and `in_tool` until 2.5.0. That is the
derivation rule doing its work twice: #454 moved the connector out of the level, so
`answer` crossed; #552 moved the `memory_recall` ANSWERER out of it, so one tool
name leaves on `tool` and its result comes back on `in_tool`. Everything else on
`tool` is still consumed inside by `./tools`, through the guarded default below —
a lane can cross for one name and stay inside for the rest, and the named edge
beside the default is what says which.

`in_thread_call` is declared by the `talky` and is deliberately **not** declared
here: no occupant outside this level produces it, and a declared lane with no door
is `hive_contract` at the next mutation the colony runs. GH #55 serves it inside
the talky. `in_memory_call` stood beside it until `talky@5.0.0` and is gone from
the library — the memory answers that call now
([#552](https://github.com/mmeyerlein/meclaw/issues/552)).

`extraction` routes **upward**, to the member's memory hive, exactly where
`talky`'s own recipe sends it — two edges, never one. The assistant grows no
memory of its own: under GH #122 the memory belongs to the member, and a second
store would force the writer to pick a store before extraction has run.

`crates/meclaw-cells/tests/gh302_assistant_wires_channels_once.rs` reads
`templates/talky/config.json`, `templates/cogny/config.json` and
`templates/member/config.json` off the tree and fails until this level moves with
them; `§ the_boundary_matches_the_member_this_level_is_instantiated_into` is the
half that admits no drift between this contract and the member's.

## Two edges to the tool surface, and neither names a tool

```json
{"from": "./talky", "to": "./tools", "default": true,
 "condition": "has(hop.route) && hop.route == 'tool'",
 "modifier": {"set_hop": {"route": "'tool_call'"},
              "set_context": {"tool_caller": "'talky'"}}}
```

and, since GH #464, one more that is a LANE rather than a tool:

```json
{"from": "./talky", "to": "./tools",
 "condition": "has(hop.route) && hop.route == 'schemas'",
 "modifier": {"set_hop": {"route": "'in_schemas'"},
              "set_context": {"tool_caller": "'talky'"}}}
```

The first is the **#286 + #283 win, measured.** The exclusion this replaces named
every tool on the live tree — nine terms, hand-kept in sync with nine positive
edges. #286 put the tool surface behind one contract, which reduced it to two
errands that are not tools at all. #283's guarded default removes the last two:
the consult errands stay **ordinary** conditioned edges on `hop.tool_name` (two
of them until #530 retired `ask_memory`, one since),
so a consult fires a regular edge and the default stays silent, while a real tool
call fires nothing regular and the default carries it. **Nothing on this edge
names a tool or an errand any more, and adding a tool touches nothing here.**

The second is the request half of `schemas` / `in_menu`: the surface asks the tool
surface for the declarations of the tools its own template declares
(`templates/talky/README.md` § *The menu is asked for*), and the answer comes back
on `tool_schemas`, renamed to `in_menu` and steered by the same `context.tool_caller`
a tool result is steered by. It is an **ordinary** edge, not a second default — two
defaults out of one sender would compete for every message nothing regular carried —
and it names no tool either: what it carries is the caller's whole declaration, and
adding a tool to the hive touches nothing here. The core draws the same pair for
itself, which is why the tool surface is reached from two senders and answers each on
the token it was asked with — and since #529 the surface's `schemas` reaches a second
ANSWERER as well, `./cogny`, whose reply is merged rather than substituted. See *The two
answerers, and the menu that is their union*.

The `tool_caller` value is a **token, not a path** — the return edge tests
`context.tool_caller != 'cogny'` and never resolves the other value against the
tree. It is renamed with the node it names, every time: `'channels'` until
2.0.0, `'surface'` until 2.3.0, `'talky'` now. A discriminator that outlives the
node it is named after would be the one word in this file that has to be read
historically, and nothing else here does.

> **Suppression is per SENDER.** If *any* regular out-edge of `./talky` fires,
> the default is silent. Every other edge out of `./talky` is conditioned on
> something a `tool` message does not carry — the outward lanes, the one errand by
> name, and the two `schemas` asks — and there is **no unconditional tee**. If this set ever
> grows a logger, a tap or a mirror without its own route condition, the tool
> surface goes dark for every call. The requirement is written into the config's
> own `because` and gated by
> `no_regular_out_edge_of_the_channels_level_is_unconditional` in
> `gh302_assistant_wires_channels_once.rs`.

The two callers of the one tool surface are told apart on the way back by
`context.tool_caller` — context, not hop, because the hop decays at the next cell
and the answer comes back through two of them.

### The consult edges

Written exactly as `cogny`'s own recipe prescribes, and both read the **lane
before the discriminator** (driver ruling W7-R4): an answer travels back through
the very path the dispatch left from, and a door that asks only about
`hop.tool_name` hands an answer to its own sender until the TTL runs out.

**`consult_cogny` belongs in the talky dispatcher's `handoff_tools`**
(GH #372), and since [#530](https://github.com/mmeyerlein/meclaw/issues/530) it is the
whole list. It is not a synchronous tool call: an advisor's answer arrives as its own
turn, and a consult wired as a tool call strands the round. That is an env setting of
this assistant's instance, not an edge.

**RETRACTED: the second errand, `ask_memory`** (GH #124 — retired in #530). Up to 2.1.0
this level drew a second `./talky -> ./cogny` edge on that name, setting
`context.consult_class` to `'lookup'` so the core would pick its fast brain. The edge is
gone, the name is gone, and an instance whose charter still offers it offers a tool no
edge carries. The reason is a thing only this level can see: at the time that edge was
drawn **the core had no memory leg at all** — `cogny` ships `memory_tier` empty, so
nothing assembled a bundle for a turn of the core — and a memory errand therefore reached
a brain with no memory to answer it from. The surface HAS one, one hop away, through the
`memory_recall` its own collector serves. The boundary outlives that reason: a lookup
routed through the core costs a whole extra TURN — the errand leaves, the round it left
behind ends, and the answer comes home later to be said again in the surface's voice —
against one tool call answered inside the round the person is waiting in. So the class
boundary survived the retraction and stopped being a name the model picks: **a fast memory
question the surface asks itself; a synthesis, a time series or anything multi-step comes
here as `consult_cogny`, and the answer arrives as its own turn.** The copyable charter paragraph lives in
[`../talky/README.md`](../talky/README.md) § *The one errand, and the memory question
that is not one*.

**`consult_cogny` takes `question` AND `context`, both required, and the asking side
filters nothing** — the answering side is the one that knows what its window holds, and a
filter on the asking side is a second curator working with less information.
`context.session_id` travels on the errand as persistent context and **must not be
promoted on this edge**: a dispatcher's tool emission carries no `hop.session_id`, so a
`set_context` reading it would FAIL, and a failed modifier skips the edge — which kills
every consult silently. The config's own `because` says so beside the edge.

### The two answerers, and the menu that is their union (#529)

`consult_cogny` is **not a tool of any hive.** It is an errand this level routes, from
`./talky` to `./cogny`, and the tool surface has never had anything under that name.
Only the side that ANSWERS a name can declare it — which is why its schema used to be a
hand-typed row in the surface's brain, and why #464 lost it: that issue replaced the
hand-typed menu with an asked-for one, and the row went with the thing it replaced. The
generation was left offering a charter that named a tool its menu no longer carried.

So the surface's `schemas` tick now reaches **both** occupants, and this level draws the
second pair:

```json
{"from": "./talky", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'schemas'",
 "modifier": {"set_hop": {"route": "'in_schemas'"},
              "set_context": {"tool_caller": "'talky'"}}}
{"from": "./cogny", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'tool_schemas'",
 "modifier": {"set_hop": {"route": "'in_menu'"},
              "set_context": {"tool_answerer": "'cogny'"}}}
```

Both `schemas` edges out of `./talky` are **ordinary** and both fire for one message:
the tick asks every answerer at once. `context.tool_answerer` is the mirror of the
`context.tool_caller` the request already carries — the caller says who asked so the
answer comes home to the right occupant, the answerer says who answered so the collector
can MERGE the two halves instead of overwriting one with the other. The two
`./tools -> X` return edges set it to `'tools'` for the same reason.

Why it has to be a merge: the collector writes the menu with `$replace`, so before #529
the second answer would not join the first, it would delete it, and the two answerers
would take the menu away from each other on every tick forever. Since `collector@3.4.0`
each answerer's last submenu is one row of the collector's own store, keyed by that name,
and every answer rewrites the union. `hop.menu_unknown` is computed against the MERGED
menu, so **a name one answerer has nothing under is not a finding when another delivers
it** — the hive has nothing under `consult_cogny`, the core has nothing under the search
tools, and neither of those is a defect.

**The declared list is this level's, for the same reason the model is (#516).** It stands
on `talky/config.json` as `override_params["collector/assemble"].tools` and reads
`["web_search", "web_fetch", "consult_cogny"]`. Standalone a `talky` declares its two
search tools and is right to — there is no core beside it to consult, so the errand is not
its to name. The list is written out in full rather than appended to, because an override
replaces a param and not the elements of a list.

## The measured fan-in, and what actually moves it

#303 counted **14** edges between the channel level and its siblings on the live
tree — the reasoning core, four tool cells, the drain, the sink, and the
assistant itself. This template draws **27** around `./talky` today, and #454
moved none of them, only the node they are drawn around; what has moved the number
since is a LANE each time, never a channel and never a tool:

```
9  . -> ./talky             the entry lanes that reach the surface, the memory's
                            two answers among them since #552 and the mutation
                            receipt since #553
10 ./talky -> .             the exits it produces, the memory road's two among
                            them since #552 and the keeper's own completion
                            word since #555
2  ./talky -> ./cogny       the ONE consult errand by name, and the schemas ask (#529)
2  ./talky -> ./tools       the guarded default, and the schemas request (#464)
2  ./cogny -> ./talky       the advice coming back, and the core's own menu half (#529)
2  ./tools -> ./talky       the tool result, and the declarations (#464)
```

The `./talky -> ./cogny` pair is **not** two errands. It was, up to 2.1.0 —
`consult_cogny` and `ask_memory` — and #530 retired the second name while #529 put a
`schemas` ask in its place, so the count stood still while both of its halves changed.

Eleven more edges do not touch the surface at all — `./cogny -> ./tools` twice and
`./tools -> ./cogny` twice (the same two lane pairs, drawn for the core), `./cogny -> .`
three times (`error`, and since #552 the memory road's `tool` and `schemas`),
`. -> ./cogny` twice (the two answers coming back, on one edge guarded by
`context.tool_caller`, and the mutation receipt since #553), `./tools -> .` on
`build`, and `. -> ./tools` on `in_build_result` — which makes **thirty-eight**
for the level.
`in_build_result` is the only entry lane that does *not* reach the surface: it
belongs to the tool round that asked, so it is delivered to `./tools` directly.

### The sessions leave, and come back (#475)

The one store this generation holds that the member cannot recompute is the session
ledger inside `./talky` — the table that decides whether a conversation continues or
starts at zero. It has had a transfer lane since `session-keeper@2.1.0`, and for one
release nothing above it forwarded one, so a member rebuilt from its own export greeted a
person it had been talking to for a year as a stranger. Three edges of this level close
that, and all three are pure transit:

```json
[
  { "from": ".", "to": "./talky",
    "condition": "has(hop.route) && hop.route == 'in_export'" },
  { "from": ".", "to": "./talky",
    "condition": "has(hop.route) && hop.route == 'in_import'" },
  { "from": "./talky", "to": ".",
    "condition": "has(hop.route) && hop.route == 'dump'" }
]
```

**No modifier, and that is a requirement rather than a style.** `in_export` and
`in_import` are named the same on both sides of every boundary they cross, so a `set_hop`
here would rename a lane onto itself — and it would hide the pairing from the drain probe,
which runs the described hop through the real edge evaluator.

**This level reads no part.** What may leave is the keeper's own walk; the document format
is the keeper's; the idempotency of a repeated part is the keeper's probe. And a refusal
never reaches this boundary as a refusal: the surface normalises a porter's `reject` into
one `error` beside every other failure of the generation, which is why `dump` carries only
parts and receipts.

**Which generation a part belongs to is the member's question, not this level's.** The
container above addresses this assistant on `context.assistant`, exactly as it does for a
turn — see [`../member/README.md`](../member/README.md) § *The export, and the one cell
this level owns*.

**A second channel still adds no edge here.** That was #303's ruling and it is
now true in the strongest possible sense: a second channel is a node in
`<member>/channels` and two edges of the *member's*, and this level does not
learn about it. A turn from any channel arrives on the same `in_turn` door, and
the answer leaves on the same `answer` lane with `context.channel_node` telling
the member where to send it back.

## Instantiating

One `add_nodes` into a member's `assistants` container, with the transit lanes in
the same mutation. The hive is an island until an edge crosses into it. **Nothing
comes afterwards.**

```json
{"scope": "<member>",
 "ctx": {"model": "<the reasoning core's model>",
         "model_surface": "<the conversation surface's model>"},
 "diff": {
  "add_nodes": [{"name": "assistants/scribe", "template": "assistant@2.5.0",
                 "override_params": {"cogny/brain": {"temperature": 0.2}}}],
  "add_edges": [
    {"from": "./assistants", "to": "./assistants/scribe",
     "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.assistant) && context.assistant == 'scribe'"},
    {"from": "./assistants/scribe", "to": "./assistants",
     "condition": "has(hop.route) && hop.route == 'answer'"}
  ]
}}
```

**`ctx` is not optional, and it carries TWO model keys.** Both occupants
substitute a model id, and the mutation door resolves those against
the declaration's own `ctx`, not against the colony's environment. Which key
reaches which brain is the level's own decision and it is drawn on the ref
markers, not left to the occupants:

| key | reaches | declared by |
|---|---|---|
| `model` | `cogny/brain`, the reasoning core | inherited from `cogny` |
| `model_surface` | `talky/brain`, the conversation surface | **this level**, [#516](https://github.com/mmeyerlein/meclaw/issues/516) |

It was THREE up to 2.1.0. `model_fast` reached the core's fast brain, the lane the retired
`ask_memory` errand chose; `cogny` dropped that brain with the errand, and an
instantiation that still passes the key is not refused, it is ignored.

`model_surface` exists because `talky` and `cogny` both read `ctx.model` in their
brain and both are right to: standalone, each is THE agent of its instantiation.
Composed into one level they meet ONE flat `ctx`, so both brains resolved the
same key and the conversation surface ran the reasoning model — every turn paying
core latency for the half whose whole job is to answer fast, with nothing in the
tree saying so. The level draws the distinction on `talky/config.json`
(`override_params.brain.model`, the `override_params` a `ref` has carried since
GH #277) and declares the key STRICT, so a mutation without it is refused rather
than silently collapsing the split again. Neither occupant moved.

A mutation without
`"ctx": {"model": "...", "model_surface": "..."}` is refused
with `requirement_missing` before a single node lands. `override_params` is the
other half and answers a different question: `ctx` says what the template
DEMANDS, `override_params` says what this generation prefers — and a mutation's
own `override_params` still wins over the ref marker's, param key by param key,
so a generation that really wants one model everywhere can still say so
explicitly.
[`../../examples/organism/grow-assistant.json`](../../examples/organism/grow-assistant.json)
carries both, and is the copy to read before writing one by hand.

Those two are the **addressing** pair and the mutation needs the rest of the
lanes as well: one edge down for `in_build_result`, and one edge up for each of
`write`, `turn_write`, `extraction`, `prune`, `error`, `build` and `dump` — the
outward lanes that are not `answer` and not `recall`. Plus the two transfer lanes
downward (`in_export` and `in_import`, both guarded on `context.assistant`: a
member with two generations has two session ledgers, and they are not one
document).

**`recall` and `in_bundle` are drawn differently since 2.4.0, and that is the
whole of what changed for whoever writes this mutation
([#562](https://github.com/mmeyerlein/meclaw/issues/562), ADR-0020).** They are
**v-lanes**, exactly like the identity door's pair one section down: one edge per
asker per direction, ending on `./assistants/scribe/talky` and
`./assistants/scribe/cogny` rather than on the generation's path, each naming its
lane with `"lane": "recall"` / `"lane": "in_bundle"`. The recall edges carry the
`recall_caller` stamp this level's own rim used to write, and the two doors back
keep the shape GH #532 gave them — `hop.recall_caller == 'cogny'` guarded, the
surface's the `default`. The permission is this level's `at` and nothing else: an
edge that ends anywhere else on the lane is refused with
`v_lane_no_connect_point`, one that tries to deliver the lane AT this path is
refused with `hive_contract`, and a level in between that declares the lane and
names no connect point may not be skipped at all (`v_lane_mandatory_hop` — which
is what keeps the member's stamping door in the road). Twenty-two edges for a
generation, and none of them is per **channel**.

The mutation is scoped to the **member**, not to the container: a node is
addressed by its `name` plus the scope, the name carries the `/`, and endpoints
are scope-relative always. Scoping to `<member>/assistants` and writing an
absolute endpoint is refused with `scope_out_of_bounds`; scoping there and
writing `"to": "."` is refused with `edge_schema`, because `.` names no node.

The first guarded edge is what makes the generation **addressable**: the member's
channel stamped `context.assistant`, and this is the edge that reads it. One edge
per assistant, per direction — the substrate resolves an edge target statically
(`Edge.to` is a `Path` in `crates/meclaw-colony/src/edge_table.rs`), so there is
no single edge meaning *"send it to whatever `context.assistant` names"*. The
full recipe, both directions, is
[`../member/README.md`](../member/README.md) § *Addressing an assistant through a
channel*.

**Model and prompt come from `override_params`,** and they are aimed one level
inside the surface — `talky/brain` for the model that answers, and
`talky/collector` for the one that extracts. The keys are relative to the node
being added, so a mutation scoped to the member writes them as `talky/brain`
and `talky/collector`; addressed from outside the mutation they are
`<assistant>/talky/brain` and `<assistant>/talky/collector`. Nothing else of
the generation is configured this way: everything below the surface is `talky`'s
own.

The member's own edges already carry `in_turn`, `in_bundle`, `in_export` and
`in_import` down into the container and take `answer`, `recall`, `extraction`,
`export_done` and `dump` off it — since #555 the keeper writes its own ledger
beside the documents of its three holders, so what the member takes off the
container is the completion word and a receipt rather than a document (#475,
#555). The other five outward lanes — `write`, `turn_write`,
`prune`, `error`, `build` — cross the member and are the parent's to drain.

### Adding a channel costs nothing here

It is one `add_nodes` into `<member>/channels` and two edges of the member's. No
edge of this template moves, no lane changes, and this level is not touched by
the mutation at all. See [`../member/README.md`](../member/README.md) § *The
channels container*.

## Not in scope

- **No channel and no connector** (GH #454). A channel belongs to the person, not
  to one of their generations.
- **No memory hive and no memory-drain.** The memory belongs to the member
  (GH #122, ADR 0012); per-turn extraction replaced the drain (#298, ruling Q11).
- **No firewall.** The screen sits outside the generation (GH #302).
- **No identity.** `affinity` is the member's — memory produces, affinity decides.
- **No terminal and no sink of any kind** (ruling Q2, GH #284). `error` leaves
  the level, and if nobody consumes it, it becomes `no_route` in the DLQ:
  recorded and self-localising.
- **No tool schemas and no per-tool credentials.** A schema lives in the calling
  brain's `system.tools`; a credential in the cell that needs it.
- **No second surface.** Two session stores of one generation would have to be
  told apart before a sweep could run.
- **No memory of its own for the reasoning core.** The core has a memory *leg*
  since [#532](https://github.com/mmeyerlein/meclaw/issues/532) — the same lane pair the surface uses, told apart by a
  reply-to token (see *One hive, two askers*). What this level still does not
  have is a store to point it at: the memory belongs to the member, and a second
  one here would be a second person.

## The credential lane crosses this level and this level says nothing (GH #560)

Both surfaces name `./brain` as the connect point of the credential lane
(`talky` and `cogny`). The edge that spends a grant is drawn one level
UP — at the member, the lowest common ancestor of a brain in here and the
person's own `access` — and it passes straight through this level as a **v-lane**
(GH #559).

This level declares **nothing** about that lane, and that is the decision rather
than an omission. A generation takes no influence on a credential: it does not
stamp it, filter it or guard it, and a level that declares a lane it merely
forwards would read as a claim to influence it and would make itself a mandatory
hop (`v_lane_mandatory_hop`) on the very edge it was replaced by. Transparent is
the correct row of that rule table, and the exception it makes to the union rule
is written down as one in `docs/development-rules.md` § 8b.

## Versioning

`1.0.0` is the first shipped version; `2.0.0` is the first breaking one. This
level's lanes and its inner addresses are a public contract: dropping or renaming
either is breaking for every parent that wired it, and moves the first digit —
which is what removing `./channels` and the `turn` lane did. Adding a lane
nothing ever promised is additive and takes the second; a repair takes the third.

`2.2.0` is the second digit. It carries #529, #530, #532 and #535 together, and the level goes
from thirty-four edges to **thirty-seven**: #530 removes the `ask_memory` errand, #529
adds the `schemas` / `in_menu` pair out to the core, #532 adds the core's own recall leg
and the door its bundle comes back through. None of the three touches the lane set at this
boundary — `schemas` is consumed by an occupant inside this level, `in_menu` is supplied
by one, an errand name is an edge condition rather than an address, and `recall` /
`in_bundle` were already declared — so nothing a caller out here can do is gone, and the
one thing that is new (addressing the core with a bundle) costs a caller no key. What DID move for an instance is its
charter and its `ctx`: `ask_memory` must be struck from `instructions.reply` and from
the talky dispatcher's `handoff_tools`, and `model_fast` no longer reaches anything.
[#535](https://github.com/mmeyerlein/meclaw/issues/535) rides in the same
unreleased number and moves no edge at all: it is one more `override_params` key
on a ref marker this level already writes two of, so no lane, no address and no
count changes. `2.2.0` has not shipped, so it is extended rather than superseded
(`docs/development-rules.md` § 4) — the same reading #464, #475 and #516 got
inside this wave.

`2.4.0` carries a SECOND subtraction of the same shape
([#562](https://github.com/mmeyerlein/meclaw/issues/562)), and it rides in the
same unreleased number for the reason `docs/development-rules.md` § 4 gives: a
version is a shipped fact, and cutting a `2.5.0` for the second half of one wave
would invent a version nobody could ever have wired against. The memory road
stops crossing this level. `recall` and `in_bundle` were the other pair this
level neither produced nor consumed — an asker emitted, this rim forwarded, the
container above forwarded again, and only the member did anything to the message.
Both entries stay in the contract and gain `at: ["./talky", "./cogny"]`; the four
pass-through edges are struck, and thirty-three edges become **twenty-nine**.
What delivers instead is one v-lane per asker per direction, drawn by the
mutation that instantiates the generation. The reply-to token, the core's guarded
door and the surface default are unchanged and moved one level up with the edges
that carry them (`crates/meclaw-cells/tests/gh532_two_askers_one_hive.rs` keeps
measuring them there). What is NOT the same as #561 is the level above: the
member declares the very same two lanes and names no connect point below
`./assistants`, so it stays a **mandatory hop** — a v-lane that would carry a
generation's recall past the door that stamps the round is refused with
`v_lane_mandatory_hop`. Vouching and influence are the two halves of one rule,
and this level is on the vouching side of it.

`2.4.0` is the second digit again, and it is a **subtraction of edges without a
subtraction of lanes** ([#561](https://github.com/mmeyerlein/meclaw/issues/561),
under the v-lane ruling of [#559](https://github.com/mmeyerlein/meclaw/issues/559)).
The identity pack used to travel as a per-level chain — the member's rim, the
`assistants` container, this level, the two brains — with this level declaring
`in_pack` / `pack_ack` purely as a **pass-through**. Under #559 rule 2 that reads
as an influence claim: a level that declares a lane has said it takes part in it,
and this one stamped nothing, filtered nothing and guarded nothing. So the four
pass-through edges are gone — thirty-seven edges become **thirty-three** — and
what is left in the contract is the connect point, `"at": ["./talky", "./cogny"]`
on both lanes. The sender now draws one v-lane per rim (`lane: "in_pack"`) and
one back (`lane: "pack_ack"`), straight from the member's own `./affinity`. It is
the second digit because an **address** moved: a caller that wired `in_pack` at
this level's own path has to redraw the edge at the two rims the corridor names.
Nothing else about the lane changed — same slots, same fan-out, same two receipts
per pack — and the pairing that obliges the receipt is now read at the rim, where
`talky` and `cogny` declare it themselves. This level's own
`required_drains` entry stays, because the statement stays true of it: whoever is
given the corridor is given both halves of it.

The occupant pins in `talky/config.json`, `cogny/config.json` and
`tools/config.json` are version-pinned on purpose — a bare name resolves to the
highest version present, which is the drift `registry.template_chain` exists to
make visible.
