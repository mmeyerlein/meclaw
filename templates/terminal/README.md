# `terminal@1.0.1`

The last cell of a lane, as one `code` cell. It accepts anything and emits nothing.

That sounds like a no-op, and it is the opposite of one: in this substrate a lane with
no destination **dead-letters**, and a dead letter is an alarm. A terminal turns "I have
not decided where this goes yet" from a defect into a documented stop -- the message
arrives, the trace records it, and the dead-letter queue stays empty.

## Retraction (GH #284): not for a `reject`, not for an `error`

Until `1.0.0` this page and this template's own `description` offered the terminal as the
place "where a rejection is logged, where errors are alarmed". **That offer is withdrawn.**
It was never what this cell does: it logs nothing and alarms nobody, it writes `[]` and
returns.

A `reject` or an `error` lane has exactly **two** honest states, and a swallowing cell is
neither of them:

1. **It has a consumer that does something** -- a cell that writes the refusal where an
   operator sees it (a store row, stderr, an alert). Emitting nothing is fine; *recording*
   nothing is not.
2. **It has no edge at all**, and the emission becomes `no_route` in the dead-letter queue,
   where it localises itself with its sender and its trace.

State (2) is the honest default while nobody has decided yet, and it is the one every
shipped example now takes: a routine refusal showing up in the DLQ is a **signal about the
topology**, not a reason to put a silencer back in front of it.

The distinction is a lane-by-lane one, not a route-name one: an answer that nothing sends
back out yet is genuinely undecided, and this cell is the right honest stop for it. A
refusal is not undecided -- somebody refused something, and that fact is worth exactly as
much as whoever reads it.

Pinned by
[`crates/meclaw-cells/tests/gh284_no_shipped_topology_silences_a_reject.rs`](../../crates/meclaw-cells/tests/gh284_no_shipped_topology_silences_a_reject.rs),
which scans every shipped `templates/` and `examples/` declaration and fails on the first
`reject`/`error` edge that ends in a cell which swallows it.

## What it delivers

- **An address for an undecided lane.** Point the answer lane and the write lane at it and
  both are wired, honestly, without pretending they belong together.
- **A quiet dead-letter queue, for the lanes it is wired on.** An undecided answer no longer
  dead-letters, so what is left in the DLQ still means something: a routing mistake, or a
  refusal that deliberately has no consumer yet (see the retraction above).
- **Nothing lost.** Every arrival is in the message log with its full `parent_message_id`
  chain -- `GET /ui/trace` shows what came home on which lane. Swallowing costs no
  information, it only costs *action*.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | `sys.stdout.write("[]")`. No state, no `cell.db`, no branch. |

Single-cell template (the `_cell-types` shape): instantiate it under any name -- `sink`
is the usual one -- and the instance IS the cell.

## Ports and wiring

Entry is any turn on any lane; there is no exit.

```json
[
  { "from": "./talky", "to": "./sink",
    "condition": "has(hop.route) && hop.route == 'answer'" },
  { "from": "./talky", "to": "./sink",
    "condition": "has(hop.route) && hop.route == 'turn_write'" }
]
```

The conditions live on the edges, as always. The terminal itself never asks what it is
looking at -- that is what makes one instance enough for every unfinished lane in a tree,
and it is also why the gate above lives on the edge rather than in this cell: the cell
cannot tell a refusal from an answer, so the topology has to.

## When to remove it

The moment a lane knows where it belongs. Each edge that pointed here gets a real
destination instead -- an answer back out through a channel `proxy`, a finished turn into a
memory hive on its in_episode lane. The template is a scaffold with an honest name, not a
component to build on.

If a lane's only consumer really is the trace (audit copies, fire-and-forget mirrors),
it can stay for good. That is the one non-temporary use.

## What does NOT live here

- **Storage.** A terminal keeps nothing; the message log already did.
- **Forwarding, retry, judgement.** All three would make it a component. It is a stop.
- **A reply to the caller.** `POST /messages` is fire-and-forget in this substrate; the
  answer arrives on a lane, not in the HTTP response.

Pinned by [`crates/meclaw-cells/tests/meclaw_os_example.rs`](../../crates/meclaw-cells/tests/meclaw_os_example.rs),
which boots `examples/meclaw-os` -- a colony with **zero** checked-in cells -- and grows
this template plus three others into a working agent, with two lanes ending here.
