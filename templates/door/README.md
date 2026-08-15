# `door@1.0.0`

The first cell of a colony, as one `code` cell. It exists because of a small, hard
fact about the substrate: the HTTP ingress puts the request headers into the message's
**`context`** compartment and leaves the **`hop` empty** -- and `set_hop` is an *edge's*
job. Above the first cell there is no edge, so the first lane has to be named by a cell.

That is the whole template. Ten lines of Python, one emission, no state.

## What it delivers

- **A named lane.** An inbound request leaves as `route 'turn'`, so the first edge of
  your tree has a condition to match instead of having to fire unconditionally.
- **A channel identity that survives.** `context.channel` (or a `chat_id` already on the
  hop) is promoted to `hop.chat_id`. The edge below usually pushes it back into
  `context`, and from there it survives every later hop -- which is what lets a firewall
  rate-limit *per channel* and a session keeper mint one session *per channel* instead
  of flattening every conversation into one.
- **A body it does not touch.** The turns pass through byte-identical. A door decides
  nothing about content.
- **No credential.** This template is the colony's own HTTP ingress, so a tree built on
  it runs without a single token. A real chat surface is a `proxy` cell holding a bot
  token instead; it emits the same two hop fields, and everything downstream is
  unchanged.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the lane naming and the channel promotion. No state, no `cell.db`. |

Single-cell template (the `_cell-types` shape): instantiate it under any name --
`surface` is the usual one -- and the instance IS the cell.

## Ports and wiring

Entry is `POST /messages` targeted at the instance. Exactly one lane leaves it:

```json
[
  { "from": "./surface", "to": "./firewall/screen",
    "condition": "has(hop.route) && hop.route == 'turn'",
    "modifier": {"set_hop": {"route": "'in_turn'"},
                 "set_context": {"channel": "hop.chat_id"}} }
]
```

| field | meaning |
|---|---|
| `hop.route` | always `'turn'` -- the one lane this cell knows |
| `hop.chat_id` | `context.channel`, else `hop.chat_id`, else `'default'` |

The `set_context` in the modifier is the load-bearing half: the hop is per-message, the
context travels. A tree that reads `hop.chat_id` three cells later reads nothing.

## Driving it

```bash
curl -s -X POST http://127.0.0.1:7777/messages \
     -H 'Content-Type: application/json' \
     -d '{"target": "/surface", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "Say hello in one short sentence."}]}}'
```

`headers` on an HTTP post land in `context`; that is why `channel` is where this cell
looks first. A request without one arrives as channel `default` -- one lane, one
session, and no special case anywhere in the wiring.

## What does NOT live here

- **Screening.** Size caps, sender rules and rate limits are the
  [`firewall`](../firewall/)'s business, one cell further in.
- **Sessions and personas.** A door does not know what a conversation is.
- **The way back.** Answers do not leave through the door in this shape; outbound lanes
  end in a [`terminal`](../terminal/), or in a `proxy` cell that owns a real channel.

Pinned by [`crates/meclaw-cells/tests/meclaw_os_example.rs`](../../crates/meclaw-cells/tests/meclaw_os_example.rs),
which boots `examples/meclaw-os` -- a colony with **zero** checked-in cells -- and grows
this template plus four others into a working agent.
