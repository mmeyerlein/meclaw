# `org@1.1.0`

The namespace, and nothing else.

**A level owns what its siblings must share.** The members of one organisation share a
name and a boundary. They do not share a memory, an identity, a broker or a firewall --
so this level owns a name and a boundary, and it owns them by being a hive with one
open container and eleven transit edges.

That is the whole template. **Its value is the namespace, not the contents; it should
not be padded to look substantial.**

## What it ships

```
org/
  config.json          the org hive: a transit contract and eleven edges
  members/
    config.json        an open, empty container -- no cells, no ports, no contract
```

Two `config.json` files, both hives, and **no cell of any type anywhere**. An org that
grows a cell has stopped being a namespace; that sentence is the level's definition and
it is a gate, not a preference (`crates/meclaw-cells/tests/gh302_org_is_a_namespace.rs`).

## The lanes

The contract carries exactly what must cross this level to reach or leave a member.

| Direction | Lane | What it means |
|---|---|---|
| accepts | `in_turn` | a turn for one of this organisation's members |
| accepts | `in_recall` | a question against a member's memory |
| accepts | `in_brief` | a read of a member's curated record |
| accepts | `in_propose` | a write against that record |
| emits | `answer` | the brief a member's `affinity` produced |
| emits | `ack` | a proposal was accepted or refused, with the reason code |
| emits | `reject` | a refusal from one of a member's holders |
| emits | `error` | a member failed at something that was not a refusal, or one of its assistants did |
| emits | `write` | an assistant's batched conversation write, crossing on its way out |
| emits | `turn_write` | one finished turn a member's assistant offers for archiving |
| emits | `prune` | a housekeeping report from a member's assistant |
| accepts | `in_build_result` | the builder's answer on its way back into the member it belongs to |
| emits | `build` | a structural wish or a submission leaving one of this organisation's members, bound for the one baumeister the whole colony shares |

Each lane's `because` says the same thing in the hive's own words: **the level is a
boundary and a namespace, not a participant.** It reads nothing, decides nothing and
holds nothing, and it translates nothing -- hop and context cross untouched.

The internal graph is thirteen edges and nothing else: **five doors** `.` into
`./members`, one per accepted lane, and **eight exits** `./members` back out to `.`, one
per emitted lane. The two that arrived with GH #425 add no cell and no decision — thinness
is the property under test here (ADR-0013 corollary b), and a lane pair through an empty
container is exactly what this level already does for `in_turn`/`answer`. Below the container there is nothing to route to until a member is instantiated,
and the mutation that instantiates one draws that member's own edges.

## Where the eleven lanes come from

They are **derived, not invented**. The rule is the one this wave ruled for a container
level: *a level declares the union of what its occupants accept and emit at the version
it was derived from, minus the lanes a sibling inside the level consumes itself.*

The occupant of this level is `member@1.1.0`. Its accepts list is `in_turn`,
`in_recall`, `in_brief`, `in_propose`; its emits list is `answer`, `ack`, `reject`,
`error`, `write`, `turn_write`, `prune`. **Nothing is subtracted**, because the only
thing inside an org is the container -- there is no sibling here to consume anything. So
the union is the whole of both lists, and every `because` in `config.json` names the
version it came from.

The last three emits are the member's own pass-through of what an assistant raises and
nothing at member level consumes (`assistant@1.1.0` emits seven lanes; the member takes
`turn`, `recall` and `extraction` and lets the rest go). They reached this level the day
the member started re-emitting them, which is what the derivation rule is for: nobody
re-derived the list by hand, the test read the neighbour's file and went red.

**The lane that is deliberately absent is `turn`.** A member consumes its own turn
internally -- `./assistants` hands it to `./firewall` -- and never emits one. An org
that declared `turn` would carry an exit edge that can never fire and promise a caller
something no occupant produces.

**If a member moves a lane, this level moves with it, in the same commit.** That is not
a habit: `gh302_org_is_a_namespace.rs` reads `templates/member/config.json` off the tree
and asserts that this level's two lists are exactly the member's two lists. An
under-declared lane is a message that vanishes with `no_route` at a level boundary, and
an over-declared one is an interface that lies -- so the boundary is pinned rather than
established once by hand.

## A group is an audience, not a holder

This is the GH #122 ruling of 2026-08-19, and it is why the level is thin rather than
merely small.

**There is no hive at org level, and specifically no org-level memory.** "What does the
group know about X" is a **filter on the read**, never a second store. Two stores would
force the writer to decide which one a sentence belongs to *before* extraction has run,
and that is not a decision it can make: at write time a turn is a turn, and whether it
turns out to be an organisational fact or a personal one is exactly the thing the
extraction has not concluded yet. One store per member, read with an audience filter,
answers both questions from one truth. Two stores answer neither reliably.

The same argument retires the org-level identity, the org-wide firewall and the shared
drain: identity and meaning belong to the member's `affinity`, observations to the
member's `memory-hive`, the screen to the member's `firewall`. Anything an organisation
seems to need is either one member's or the shell's.

**The one exception, and it is not an exception to the rule.** A group that owns an
agent nobody owns personally -- a shared assistant, a duty desk, a support persona --
is a **member** with its own name, instantiated from `member@1.1.0` like any other. It
gets a memory because it is a member, not because it is a group. The rule survives
intact; only the naming moves.

Beside that ruling stands one fact about this template's container: **the unbound
behaviour of `members` is undeclared.** The slot of GH #285 governs an address that
does *not* exist, and this container does exist, so a slot declaration here could never
fire; a message that reaches the container before anything is instantiated into it takes
the ordinary path. The finding was read out of the slot resolution itself:
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps aside as soon as the target is a registered hive scope.

## Why the container is open

`params.ports` is **absent**, not empty, on both hives. A member is instantiated *into*
`members`, and that mutation draws edges to the member it just created -- endpoints a
sealed hive refuses with `hive_port_boundary`. Sealing this level would seal the one
thing it exists to allow, which is the opposite of what a namespace is for.

The container also declares **no contract**. A lane declared there would have no door
until the first member exists, and a contracted hive with a missing door is refused at
the next mutation the colony runs, not only at the one that would have filled it. The
transit contract therefore lives on the org, where its own eleven edges keep it true from
the first second.

## What is deliberately not here

- **No memory, no identity, no firewall, no broker.** See the ruling above.
- **No sink and no drain.** A `reject` or an `error` that crosses this level is one the
  parent owes a consumer; a level that swallowed it would be a holder (GH #284).
- **No cells at all**, and therefore no factory: a colony can grow an organisation with
  an empty cell-factory registry.
- **No default edge.** Nothing here chooses; there is nothing to choose between.
- **No translation.** Every lane keeps its name across this boundary. A namespace that
  renamed a lane would be participating in the conversation.

## Instantiating one

Two mutations, in this order.

1. **The organisation.** One `add_nodes` with the template `org@1.1.0`, plus the transit
   edges -- the four inbound lanes onto the org's own path, and the seven outbound ones
   back out to whoever asked. Nothing is registered as a cell: both directories become
   hive scopes, and a hive is a scope marker, not an actor.
2. **Each member, afterwards, one at a time.** An `add_nodes` into `<org>/members` with
   `member@1.1.0`, and in the *same* mutation the edges that member needs. The container
   is open precisely so that this mutation is legal.

Pin the version rather than writing a bare name: a bare `org` resolves to the highest
version present, and a tree that silently adopts a newer level is the drift
`registry.template_chain` exists to make visible.
