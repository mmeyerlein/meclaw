# `archive-bridge@1.0.0`

A generic "archive to store" bridge, as one `code` cell -- no new cell type, no Rust.

Stores are driven by **tool_call args**, not by headers: a `store` cell reads
`{operation, table, row}` out of a `tool_call` turn and answers with a `tool_result`.
An `llm`'s final answer is neither -- it is an assistant **text** turn. Every colony
that wants to keep its answers needs the same three lines of translation, and they
existed inline in the example colonies (`examples/telegram-research/main/archive`).
This template lifts that pattern into a clean, documented, reusable cell (GH #4).

## What it delivers

- **The translation.** The LAST non-empty assistant text turn becomes
  `{operation: "insert", table: ${ARCHIVE_TABLE:-archive}, row: {id, text, recorded_at}}`
  on route `store` -- `id` a fresh uuid4, `recorded_at` an ISO-8601 UTC timestamp,
  `text` the answer verbatim.
- **A recognizable insert.** The `tool_call` turn id and `hop.tool_call_id` are the
  same fresh `archive-<uuid8>` id, so the store's reply is correlatable.
- **The echo dies here, quietly and on purpose.** The store answers every insert with
  a `tool_result` under that id. That echo has nowhere useful to go -- and an emission
  that matches no out-edge dead-letters (`no_route`), loudly. The wiring routes the
  reply BACK into the bridge, and the bridge swallows any input without an assistant
  text turn as an empty multi-send. **The swallowing is deliberate, documented
  behavior, not an accident**: the bridge doubles as the quiet drain that keeps the
  dead-letter queue empty.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the answer scan, the insert construction, the echo guard. No state, no `cell.db`. |

This is a single-cell template (one cell of one cell type, the smallest `config.json` that starts it, and a README that explains its declarations): instantiate it under any
name -- `archive` is the usual one -- and the instance IS the cell.

## Ports and wiring

Two lanes, wired in the SAME mutation that instantiates the cell:

```json
[
  { "from": "./brain", "to": "./archive",
    "condition": "has(hop.finish_reason) && hop.finish_reason == 'stop'" },

  { "from": "./archive", "to": "./keep",
    "condition": "has(hop.route) && hop.route == 'store'" },

  { "from": "./keep", "to": "./archive" }
]
```

| edge | job |
|---|---|
| entry | the parent's choice: whatever carries final answers. A `finish_reason == 'stop'` condition off the brain is the usual form; behind a [`dispatcher`](../dispatcher/), tap the `answer` lane instead. |
| store | the insert `tool_call` into the store cell |
| echo | **unconditional** reply lane back into the bridge. This edge is load-bearing: without it the store's `tool_result` matches no out-edge and dead-letters as `no_route`. |

The store cell must **own the table**: `params.schema` with the three text columns --

```json
"schema": { "archive": { "id": "text", "text": "text", "recorded_at": "text" } }
```

An insert into a table the store does not know is answered with
`error_code: unknown_table` -- and that echo dies in the bridge like every other
reply. If archived rows silently do not appear, look for that code in the message log
first.

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env var | default | meaning |
|---|---|---|
| `ARCHIVE_TABLE` | `archive` | target table of the insert. Boot-substituted into the shipped script; the store's schema must declare the same table. |

## Reading the script

| input | emission |
|---|---|
| a message whose last non-empty assistant text turn is `T` | one `tool_call` on route `store`: `{operation: insert, table, row: {id, text: T, recorded_at}}` |
| the store's reply echo (`tool_result` under an `archive-` id) | nothing -- swallowed on purpose (empty multi-send, terminal) |
| user turns, tool rounds, empty answers, anything else | nothing -- same guard, same reason |

"Last" is deliberate: a conversation body can carry earlier assistant turns (partial
sentences, tool-round commentary); the final answer is the last one. A body that is
only a tool round passes the bridge without a trace, which is what makes the bridge
safe to hang off busy lanes.

## Known limits

1. **Shared stores share the echo lane.** The echo edge routes EVERY reply of that
   store into the bridge. The bridge swallows anything without an assistant text turn,
   so it is a safe drain for other writers' insert echoes too -- but a reply another
   cell WANTS to see must be routed by a more specific edge (condition on
   `context`, as the example colonies do with `store_origin`). Simplest form: give the
   bridge its own store cell.
2. **Append-only.** No dedup: the same answer archived twice is two rows.
3. **No delivery receipt.** The bridge cannot tell its parent the insert worked; the
   proof lives in the store (SELECT) and the message log. A failed insert is visible
   as an `error_code` on the echo, nowhere else.

Pinned in [`crates/meclaw-cells/tests/archive_bridge.rs`](../../crates/meclaw-cells/tests/archive_bridge.rs):
the script half runs the shipped `script_inline` against real stdin documents; the
colony half boots this template next to a real `store` cell, finds the answer as a row
in the store's `cell.db` (id + timestamp included) and proves the echo lane leaves the
dead-letter queue empty.
