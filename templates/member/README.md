# `member@1.1.0`

One person, as a level. Three holders and one open container:

| holder | what it holds |
|---|---|
| [`affinity@3.0.0`](../affinity/README.md) | **identity and meaning** — the curated record of who this person is and who their people are to them. Curated, fail-closed, quotable: it answers *who is X to me* and it is the only thing that answers it. |
| [`memory-hive@3.0.3`](../memory-hive/README.md) | **observations**, tagged with the participant set they were learned in. Raw, allowed to be wrong, carrying a confidence — this is what was said, not what it means. |
| [`firewall@2.0.5`](../firewall/README.md) | **the screen**. Every inbound turn is measured before it reaches anything of this person's, and the verdict is a comparison or a clock, never a model. |

The fourth thing a channel needs — the channel itself — is the
[`talky`](../talky/README.md)'s, and a talky lives inside an assistant, one level
down. Memory produces, affinity decides, the talky carries. Three holders, three
jobs, no overlap (GH #122, the three-holders ruling of 2026-08-19).

## a level owns what its siblings must share

That is the rule the whole of GH #302 is built on, and this level is where it
stops being an abstraction. #302 names two instances, and both are concrete:

- **The memory sits here because two assistants of one person must know the same
  person.** One person, one memory. A second assistant is not seeded, not synced
  and not migrated — it starts already knowing what the member knows, because
  there was only ever one store and it was never the agent's.
- **The firewall sits here because two channels need one view of an attacker.**
  A rate window that restarts when a generation is replaced is not a rate window.
  The screen belongs to the person being screened for, not to the surface the
  attacker happened to pick.

**The memory belongs to the member, not to the agent.** Everything else in this
template follows from that one sentence: it is why the write half of the memory
lane is here rather than in the assistant, why there is no second store at org
level, and why the assistant template grows no memory of its own.

## What crosses the boundary

Five lanes in, eight out. Every one is a lane an occupant actually has at the
version pinned above — nothing here describes a lane a holder lost.

| in | goes to | the caller promotes |
|---|---|---|
| `in_turn` | the screen | `context.channel` — and `context.user_id` if this colony has per-user firewall rules |
| `in_recall` | the memory, as its own `in_query` | `hop.recall_query`, `hop.memory_tier`, `hop.recall_window_from`, `hop.recall_window_to`, `hop.memory_call_id` (the collector's correlation id, GH #411 — promoted into context because a hop does not survive the hive), plus the round: `context.audience_set` and `context.channel` |
| `in_brief` | the record, to read | `context.asker` and `context.audience_set` |
| `in_propose` | the record, to write | `context.actor`, and `context.subscriber` for a `subscribe` |
| `in_build_result` | `./assistants`, under the same name | nothing. Which generation it belongs to is decided by the per-instance edge inside the container, the same way `in_bundle` finds its way home |

| out | from | what it is |
|---|---|---|
| `answer` | the record | the brief, for whoever asked. Told apart from a push by `hop.subscriber == ''` |
| `ack` | the record | a proposed change was accepted or rejected, with its reason code |
| `reject` | the screen **or** the memory | a refusal. `hop.reject_reason` says which case |
| `error` | the record **or** an assistant | a failure that was not a refusal |
| `write` | an assistant | a batched conversation write, on its way past this level |
| `turn_write` | an assistant | one finished turn, offered for archiving as it is produced |
| `prune` | an assistant | a housekeeping report, raised when something above fired `in_prune` |
| `build` | an assistant | a structural wish or a submission leaving one of this person's generations, on its way to the one baumeister the colony shares. The member neither reads it nor answers it: everything between the tool surface and the OS level is transit (GH #425) |

The last four rows are the assistant's, and they are the reason this level has
an outward edge from `./assistants` at all. `assistant@1.1.0` emits **seven**
lanes; three of them stop inside this member — `turn` at the screen, `recall`
and `extraction` at the memory — and the other four have to leave, because
nothing here consumes them. A level that declared them without the edge, or
carried the edge without declaring them, would be lying in one of the two
directions; the pin is
`crates/meclaw-cells/tests/gh302_member_holds_the_memory.rs`
§ `every_lane_an_assistant_emits_is_consumed_here_or_leaves_the_level`, which
reads `templates/assistant/config.json` off the tree and admits no third answer.

`error` is the sharp one, and it is why that test reads the *edges* and not only
the contract: this level already emitted `error` from `./affinity`, so the
declaration was satisfied while an **assistant's** error had no exit at all and
died as `no_route` at the container. Two senders, one lane, one declaration —
and one exit edge each.

### The two refusal lanes leave, and nothing inside consumes them

Both `reject` edges are wired outward and read by nobody in this template. That
is the honest state (2) of [#284](https://github.com/mmeyerlein/meclaw/issues/284):
**a screened-off turn with no consumer is `no_route` in the DLQ, recorded and
self-localising.** There is no `terminal` here and there will not be one — a sink
that accepts a refusal and drops it is the one arrangement in which nobody finds
out.

The memory's half of that lane is not optional in the same loose sense:
`memory-hive@3.0.3` declares `required_drains` pairings for `in_query`,
`in_remember` and `in_episode` against `reject`, and this level sends the first
two. Whoever wires a member drains its `reject`.

## The wiring, and why each edge exists

**The screen.** An unscreened turn leaves the assistants container on `turn` and
is re-stamped into the firewall's `in_turn`; the screened turn comes back on
`pass`, re-stamped to `in_turn` again, and the container routes it to the
assistant that sent it by an edge the instantiating mutation drew. Both edges out
of the firewall clear its own context keys (`fw_body`, `fw_now`, `fw_phase`,
`store_origin`), because the parked copy of the turn rides along otherwise —
that is the firewall's own instruction, not a precaution added here.

**The memory: one pair that reads, one edge that writes.** `recall` → `in_query`
and `bundle` → `in_bundle` are the pair the whole level exists for: one memory,
every assistant of this member reading it. The write half is one edge —
`extraction` → `in_remember` — and it is here because the memory is the
member's. It is the second half of the recipe `talky` prescribes to its parent
(*two edges, never one*, [`../talky/README.md`](../talky/README.md) § the
extraction sidecar); the first half is inside the talky, and the drain the recipe
asks for is the `reject` lane above.

**The record.** Read on `in_brief`, write on `in_propose`, answers back out. If
this member's identity updates should be **pushed** into an assistant's brain,
do not re-invent the mechanism: it is one edge per subscribing cell, drawn by the
instantiating mutation, and the recipe is
[`../affinity/README.md`](../affinity/README.md) § *Wiring `out_push` for a
subscribing brain*. A `subscribe` writes a row, never an edge — mutation
authority is the colony's — so a subscription with no edge behind it is accepted
and undeliverable.

**The four that only pass through.** `write`, `turn_write`, `prune` and an
assistant's `error` get one plain exit edge each, `./assistants -> .`, and no
translation on the way. This level owns no archive and no timer, so it has
nothing to do with any of them except refuse to swallow them — which is the
same rule the refusal lanes follow, applied to lanes that are not refusals.

### Identity comes from the edge, at every door

Since [#291](https://github.com/mmeyerlein/meclaw/issues/291) a `context` key a
hive lane declares is **enforced**: an edge that states that lane must promote
the key itself or have a setter reachable upstream. At this level `.` is the
door and nothing is upstream of it, so **the edge is the only setter root**, and
every edge here that stamps a lane into a holder promotes what that lane asks
for.

Each promotion is written `has(...) ? ... : ''` rather than as a bare read. A
modifier that fails to evaluate skips the whole edge, and a turn that vanishes on
an edge is invisible; a turn that arrives with an empty key is refused **by the
holder**, on the `reject` lane this level already drains, with a reason. Empty
string means unset in all three holders, which is what makes that trade legal.

The round has exactly one spelling: **`audience_set`**. `participants` is
**retired, not aliased** ([#330](https://github.com/mmeyerlein/meclaw/issues/330))
— a request that spells the round that way declared no round at all and is
refused like any other undeclared one. No template may introduce a second name
for it.

## The container

`assistants` is a real, empty, **open** hive. Open because the mutation that
instantiates an assistant draws edges to that assistant, and a sealed hive
refuses exactly those endpoints with `hive_port_boundary`. It ships with no
cells and no edges of its own; the member wires it, and each assistant
instantiation wires itself.

**Its unbound behaviour is undeclared.** GH #285's slot governs an address that
does **not** exist, and this container does exist — so the declared word could
never fire, and a message that reaches the container before an assistant is
instantiated takes the ordinary path. The measurement comes from
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps aside as soon as the target is a registered hive scope. Writing `params.ports` for a
slot's sake would additionally **seal** the member, which is the opposite of what
a level that gets wired into is for. No hive in this template carries a `ports`
key.

**What transits it**, derived from the contract of `assistant@1.1.0` and from
what this member sends back down (`firewall@2.0.5`, `memory-hive@3.0.3`):

- **in** — `in_turn` (the screened turn), `in_bundle` (the memory's answer).
  Both are produced by a sibling of the container, not by a caller outside the
  member.
- **out** — the seven an assistant emits: `turn`, `write`, `turn_write`,
  `extraction`, `recall`, `prune`, `error`

Three stop at this level — `turn` at the screen, `recall` and `extraction` at
the memory. The other four (`write`, `turn_write`, `prune`, `error`) are
re-emitted on the member's own contract and leave; the parent drains them.

### Four inbound lanes this level deliberately does not carry

`assistant@1.1.0` accepts six lanes. Two of them are handed down by a sibling of
the container: `in_turn` from the screen and `in_bundle` from the memory. The
other four — **`in_advice`**, **`in_sweep`**, **`in_prune`** and
**`in_round_sweep`** — are **not** lanes of this member, and that is a decision
rather than an omission (orchestrator ruling W7-R5).

A level's transit contract carries the lanes that *cross* it. An emitted lane
always crosses: it is produced inside and has to get out, which is exactly why
the four outward ones above are here. An accepted lane crosses only when its
producer sits **outside** the level and addresses **through** it. These four do
not:

| lane | who produces it |
|---|---|
| `in_advice` | `./cogny`, inside the assistant. The other producer is a second agent, which stands beside the first in this same container. |
| `in_sweep` | an operator. The assistant's own `because` says it *"enters at the assistant path rather than being produced by a sibling"*. |
| `in_prune` | a timer or an operator — paired with the `prune` report the member *does* carry outward. |
| `in_round_sweep` | the same owner as `in_sweep`, entering the same way. |

They reach the assistant at its own address, `<member>/assistants/<agent>`, and
they may: neither this level nor the assistant declares `params.ports`, so both
are **open**, and the port boundary forbids an outside endpoint below a hive path
only for a *sealed* hive
(`crates/meclaw-colony/src/mutation/port_boundary.rs`). Declaring them here would
promise a road nobody drives on — the mirror image of the four outward lanes that
had no road at all.

The exception is pinned, not merely written down:
`gh302_member_holds_the_memory.rs`
§ `the_lanes_an_assistant_takes_from_an_operator_deliberately_do_not_cross_this_level`
lists exactly these four and requires every *other* lane the assistant accepts to
be supplied from inside. A fifth lane that really does arrive from above goes red
there; carrying one of these four later is a deliberate edit of that list, this
paragraph, and the `org` and `meclaw-os` contracts with it.

### Why the container carries no contract

That list is prose in the container's `description`, not a `params.contract`, and
the reason is mechanical rather than stylistic. `addressed_lane_doors` skips a
hive only while **nothing addresses its path** (`hive_path_is_wired`). This
member addresses `./assistants` four times, so the container is wired the moment
the member is instantiated — and from then on every lane it declared would owe a
`door_exists`: a message arriving at the container path must reach a cell
*inside* it. An empty container has no inside. The violation would be collected
on **every** mutation of the colony, not only on one that touches this member, so
a contract here would lock the colony for exactly as long as this member has no
assistant yet — which is a perfectly ordinary intermediate state.

The rule, which holds for all four levels of this wave: **a container hive that
its own level wires declares no `params.contract`. The transit lanes are declared
by the level whose own edges satisfy the door and exit check from birth.** A
container nobody wires could technically carry a dormant contract; it should not
— a declaration that is green only because nothing is looking is the same defect
class as the slot this wave struck.

A container is an address. An address is not an interface until something stands
at it.

## What is deliberately not here

- **No `memory-drain`.** Per-turn extraction ([#298](https://github.com/mmeyerlein/meclaw/issues/298),
  ruling Q11) replaced it, and #302 says explicitly that it does not belong in
  the assistant either.
- **No sink.** See the refusal lanes above.
- **No org-level anything.** A group is an audience, not a holder: *what does the
  group know about X* is a filter on the read, never a second store. Two stores
  would force the writer to pick one before extraction has run, which is not a
  decision it can make. A group that owns an agent nobody owns personally is a
  **member** with its own name, instantiated from this template like any other.
- **No close pass.** `memory-hive@3.0.3` has an `in_close_pass` lane; this level
  does not send it. Whether the close pass should cross the member boundary is a
  level question, and an unanswered one is better than a lane declared here that
  nothing opens.
- **No brain, no channel, no tools.** All three are the assistant's, and one
  member may own several assistants.
