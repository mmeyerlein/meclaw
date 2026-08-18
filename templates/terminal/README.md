# `terminal@1.0.0`

The last cell of a lane, as one `code` cell. It accepts anything and emits nothing.

That sounds like a no-op, and it is the opposite of one: in this substrate a lane with
no destination **dead-letters**, and a dead letter is an alarm. A terminal turns "I have
not decided where this goes yet" from a defect into a documented stop -- the message
arrives, the trace records it, and the dead-letter queue stays empty.

## What it delivers

- **An address for an undecided lane.** Point four outbound lanes at it and all four are
  wired, honestly, without pretending they belong together.
- **A quiet dead-letter queue.** Nothing routes into nowhere, so a non-empty DLQ keeps
  meaning what it should mean: a real routing mistake.
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
  { "from": "./firewall", "to": "./sink",
    "condition": "has(hop.route) && hop.route == 'reject'" },
  { "from": "./talky", "to": "./sink",
    "condition": "has(hop.route) && hop.route == 'error'" }
]
```

The conditions live on the edges, as always. The terminal itself never asks what it is
looking at -- that is what makes one instance enough for every unfinished lane in a tree.

## When to remove it

The moment a lane knows where it belongs. Each edge that pointed here gets a real
destination instead -- an answer back out through a channel `proxy`, a rejection into a
log store, an error onto an alarm, an episode into a memory hive on its in_episode lane. The
template is a scaffold with an honest name, not a component to build on.

If a lane's only consumer really is the trace (audit copies, fire-and-forget mirrors),
it can stay for good. That is the one non-temporary use.

## What does NOT live here

- **Storage.** A terminal keeps nothing; the message log already did.
- **Forwarding, retry, judgement.** All three would make it a component. It is a stop.
- **A reply to the caller.** `POST /messages` is fire-and-forget in this substrate; the
  answer arrives on a lane, not in the HTTP response.

Pinned by [`crates/meclaw-cells/tests/meclaw_os_example.rs`](../../crates/meclaw-cells/tests/meclaw_os_example.rs),
which boots `examples/meclaw-os` -- a colony with **zero** checked-in cells -- and grows
this template plus four others into a working agent, with four lanes ending here.
