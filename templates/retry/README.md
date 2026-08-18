# `retry@1.0.0`

A bounded retry loop around **one** tool, as one `code` cell -- no new cell type, no
Rust, and **no counting in the cell**. The attempt counter is edge authority: the wiring
increments it, the wiring bounds it, and the cell only decides *what* travels on which
lane. That split is the point (GH #3): iteration is topology in this DSL, and a cell
that counted its own retries would carry loop state the graph cannot see.

The trick that makes a stateless cell retry-capable: the call **parks itself in
`context`**. On the way in, the cell copies the call turn onto the hop (`hop.call`),
and the call edge promotes it (`set_context {"call": "hop.call"}`). `context` survives
the tool hop -- so when the error reply comes back, the cell rebuilds the ORIGINAL call
from `context.call` and re-emits it. No `cell.db`, no topology knowledge.

## What it delivers

- **A bounded retry.** An error reply re-enters the tool at most `RETRY_MAX` times;
  the bound is the retry edge's condition, not code.
- **The ORIGINAL call retries.** Not the error echo, not a reconstruction from the
  reply -- the byte-identical turn that was parked on the way in.
- **A clean give-up.** At the cap the retry edge stops matching and the give-up edge
  hands the message to the error sink: the failed call in the body, the original
  `hop.error_code` and the attempt count on the hop. Nothing dead-letters.
- **A success passes through.** A reply without `hop.error_code` leaves on the `ok`
  lane unchanged.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the lane decision and the call park/rebuild. No state, no `cell.db`, no counter. |

This is a single-cell template (the `_cell-types` shape): instantiate it under any name
-- `retry` is the usual one -- and the instance IS the cell.

## Ports and wiring

Entry is a message with **one** `tool_call` turn. Five lanes, all on `hop.route`; wire
them in the SAME mutation that instantiates the cell:

```json
[
  { "from": "./retry", "to": "./tool",
    "condition": "has(hop.route) && hop.route == 'call'",
    "modifier": {"set_context": {"call": "hop.call", "attempt": "'0'"}} },

  { "from": "./tool", "to": "./retry" },

  { "from": "./retry", "to": "./tool",
    "condition": "has(hop.route) && hop.route == 'retry' && int(context.attempt) < ${RETRY_MAX:-3}",
    "modifier": {"set_context": {"attempt": "int(context.attempt) + 1"}} },

  { "from": "./retry", "to": "./done",
    "condition": "has(hop.route) && hop.route == 'ok'",
    "modifier": {"delete_context": ["call", "attempt"]} },

  { "from": "./retry", "to": "./errors",
    "condition": "has(hop.route) && hop.route == 'retry' && int(context.attempt) >= ${RETRY_MAX:-3}",
    "modifier": {"set_hop": {"route": "'error'", "attempt": "int(context.attempt)"}} },

  { "from": "./retry", "to": "./errors",
    "condition": "has(hop.route) && hop.route == 'error'" }
]
```

| edge | job |
|---|---|
| call | forwards the call to the tool, **parks it** (`context.call`) and **seeds the counter** (`context.attempt = '0'`) |
| reply | unconditional: every tool reply -- success or error -- comes back through the cell |
| retry | the loop: matches below the cap, **increments** the counter. This is the only writer of `context.attempt`. |
| ok | the success exit; `delete_context` cleans the parked call out of the context |
| give-up | matches AT the cap; rewrites the route to `'error'` and puts the attempt count on the hop. The body is the failed call; `hop.error_code` still names the last failure. |
| error | the cell's own terminal lane: an error with no rebuildable parked call, or a refused bundle |

**The `int()` cast is not decoration.** CEL deserializes a JSON integer as `uint`, and
`uint < int` / `uint + int` are not defined -- a bare `context.attempt < 3` or
`context.attempt + 1` **errors**, and the substrate skips the edge with a log line.
Every arithmetic touch of the counter needs `int(...)`, on both sides of its life
(`int(context.attempt) + 1` when counting, `int(context.attempt) < N` when bounding).
Same rule as the loop edges in `examples/` -- see `docs/meclaw-overview.md` § Edge-Modell.

**Initialization is load-bearing.** If `context.attempt` is missing, both counter
conditions ERROR (not false), both edges are skipped, and the retry emission
dead-letters as `no_route` -- loud, but downstream of the real mistake. The call edge's
`"attempt": "'0'"` seed is what arms the loop; do not drop it.

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env var | default | meaning |
|---|---|---|
| `RETRY_MAX` | `3` | how many times a failed call re-enters the tool. At `attempt >= RETRY_MAX` the give-up edge fires. The knob lives in the WIRING (both counter conditions, boot-substituted) -- the cell is byte-identical for every value. |

## The failure path, end to end

1. The tool replies with `hop.error_code` set (store/bash/code error replies carry it;
   a tool that fails without the code is invisible to this template).
2. The cell rebuilds the parked call and emits it on `route 'retry'` with the original
   `error_code` still on the hop.
3. Below the cap the retry edge takes it back into the tool, counting. At the cap the
   give-up edge hands it to the error sink instead: body = the failed call,
   `hop {route: 'error', error_code: <original>, attempt: <cap>}`. The last error TEXT
   is not carried along -- it lives in the message log (`parent_message_id` chain), and
   the sink gets the three things it can act on: which call, why, how often.
4. Two floors keep pathologies out of the loop:
   - an error reply with **no rebuildable parked call** (mis-wired call edge, corrupted
     context) leaves on `route 'error'` directly -- retrying is impossible, looping the
     error echo into the tool would be garbage-in;
   - a **bundle** (more than one `tool_call` turn) is refused with
     `error_code: bundle_not_supported` and one synthetic `tool_result` per id -- this
     unit retries ONE call; the [`dispatcher`](../dispatcher/) splits bundles.

## What does NOT live here

- **Backoff timing.** A retry re-enters the tool after a routing hop, not after a
  delay. A real backoff belongs to a `timer` cell between the retry lane and the tool;
  this template keeps the loop structural.
- **`restore_ttl`.** A retry round is two hops; even `RETRY_MAX=20` stays far below the
  colony default budget of 64. A loop whose ROUND is itself routing-heavy (tool loops
  with fan-in) restores its budget on the re-entry edge instead -- see
  `docs/store-backed-tool-loop.md`.
- **Fan-in / bundles.** One call, one tool, one loop.

## Reading the script

| input | emission |
|---|---|
| exactly one `tool_call` turn | `route 'call'`: the turn verbatim, `hop.call` = the turn as JSON (for the park), `hop.tool_call_id` |
| more than one `tool_call` turn | `route 'error'`, `error_code 'bundle_not_supported'`, one synthetic `tool_result` per id |
| reply with `hop.error_code`, parked call rebuildable | `route 'retry'`: the ORIGINAL call, `error_code` passed along |
| reply with `hop.error_code`, no/corrupt parked call | `route 'error'`: the reply passed through, `error_code` passed along |
| `tool_result` without `hop.error_code` | `route 'ok'`: the reply passed through, `hop.tool_call_id` |
| anything else | nothing (empty multi-send, terminal) |

Pinned in [`crates/meclaw-cells/tests/retry_template.rs`](../../crates/meclaw-cells/tests/retry_template.rs):
the script half runs the shipped `script_inline` against real stdin documents; the
colony half boots this template with the wiring above and a deterministic flaky tool --
the loop succeeds exactly when the EDGE has counted high enough, gives up exactly at
the cap, and leaves the dead-letter queue empty.
