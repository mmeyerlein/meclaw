# `dispatcher@1.0.0`

The fan-**out** half of a tool loop, as one `code` cell -- no new cell type, no Rust.
Its counterpart is the fan-**in**: [`collector@1`](../collector/), which assembles the
round and re-enters the brain. Splitting the two is the point (`R-OS-2`): routing a call
to a tool and assembling a context window are different jobs, and a cell that did both
would be the monolith the DSL exists to avoid.

A brain answers a turn in one of two ways. It says something final, or it asks for tools
-- possibly several in one answer, in **one bundle**. This cell turns that bundle into
messages a graph can route.

## What it delivers

- **One message per call, addressed by NAME.** A call leaves carrying `hop.tool_name`
  and `hop.tool_call_id` and nothing else. Which cell answers to `web_search` is a
  question for an edge condition, never for this cell: no tool list, no map of the tree,
  nothing to update when a tool is added.
- **The expectation set first.** The assistant turn goes to the fan-in *before* any tool
  message leaves (PLAIN order). A tool that answers in a millisecond can otherwise report
  a result for a round the collector has not been told about yet.
- **A budget that answers instead of dropping.** A bundle above `DISPATCHER_MAX_CALLS`
  runs no tool at all -- and every expected `tool_call_id` still gets a reply, a synthetic
  error `tool_result`. The round stays fan-in-complete, so the brain sees the refusal and
  has to respond to it. A silently dropped call would stall the fan-in until the TTL runs
  out, and TTL expiry emits **nothing** towards the surface.
- **A final answer, passed through.** `finish_reason == 'stop'` leaves on its own lane,
  unchanged.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the whole split: lanes, the budget, the OpenAI-form unwrap. No state, no `cell.db`. |

This is a single-cell template (the `_cell-types` shape): instantiate it under any name --
`split` and `dispatch` are the usual ones -- and the instance IS the cell.

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/agent","ctx":{},"diff":{
        "add_nodes":[{"name":"split","template":"dispatcher"}],
        "add_edges":[ ... the routes below, in the SAME mutation ... ]}}'
```

## Ports

Entry is the brain's output; there is one lane in and four out, all on `hop.route`.

| route | to | what travels |
|---|---|---|
| `calls` | the collector's `in_calls` port | the assistant turn **verbatim** (all of it, a text turn next to the calls included) -- the expectation set of the round |
| `tool` | one tool cell per name | one `tool_call` turn with the **raw arguments**; `hop.tool_name` selects the cell, `hop.tool_call_id` correlates the result |
| `result` | the collector's `in_tool` port | a synthetic error `tool_result` for a call that will never run; `hop.error_code` says which kind |
| `answer` | the collector's `in_answer` port, or the reply sink | the brain's final turn, `hop.finish_reason` carried along |

The tool lanes guard the key they discriminate on:

```json
{ "from": "./split", "to": "./collect/assemble",
  "condition": "has(hop.route) && hop.route == 'calls'",
  "modifier": {"set_hop": {"route": "'in_calls'"}} },
{ "from": "./split", "to": "./collect/assemble",
  "condition": "has(hop.route) && hop.route == 'result'",
  "modifier": {"set_hop": {"route": "'in_tool'"}} },
{ "from": "./split", "to": "./search",
  "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'" },
{ "from": "./split", "to": "./shell",
  "condition": "has(hop.tool_name) && hop.tool_name == 'bash'" }
```

The `has()` is not decoration: `hop` is single-hop, so the `calls`, `result` and `answer`
emissions carry no `tool_name` at all. A bare `hop.tool_name == 'web_search'` does not
evaluate to `false` on those, it **errors** (CEL semantics) and the substrate skips the
edge with a log line per lane per message. Same rule as everywhere else --
[`docs/store-backed-tool-loop.en.md`](../../../docs/store-backed-tool-loop.en.md).

## Knobs

| env var | default | meaning |
|---|---|---|
| `DISPATCHER_MAX_CALLS` | `16` | per-answer call budget. **At** the cap the bundle runs; one call over it, the bundle is refused **as a whole** and every id is answered with `call budget exceeded`. |

One knob per concern: this one bounds **one brain answer**. It does not bound the loop --
see below.

## Three things that belong on an edge, not in here

**1. The loop bound and `restore_ttl` (GH #82).** A tool round is a dozen routing hops, so
a loop that runs on the colony default budget of 64 dies in its fifth round -- terminal,
straight to the dead-letter queue, nothing emitted towards the origin. The fix is one
modifier on the **re-entry** edge (collector → brain), never one per tool answer:

```json
{ "from": "./collect/assemble", "to": "./brain",
  "condition": "has(hop.route) && hop.route == 'brain' && int(context.iter) < 12",
  "modifier": {"set_context": {"iter": "int(context.iter) + 1"}, "restore_ttl": true} }
```

A restoring edge **must** carry a condition -- the substrate refuses an unconditional one
at config load and at `add_edges`, because the iteration bound is then the only thing left
stopping the loop. Restore once per **round**, not once per tool result: `iter` counts
brain answers, and a bundle of fifteen calls is one answer, one iteration, one restore.
Derivation and hop table: [`docs/store-backed-tool-loop.en.md`](../../../docs/store-backed-tool-loop.en.md).

**2. A failed inference.** A turn that is neither a bundle nor a final answer is terminal
here (empty multi-send). The `llm` cell's error path echoes the **input** conversation back
with `finish_reason: "error"`, and forwarding that onto the answer lane would file the
prompt as the agent's own words. Give the error its own edge off the brain, in front of the
dispatcher edge:

```json
{ "from": "./brain", "to": "./notify",
  "condition": "has(hop.finish_reason) && hop.finish_reason == 'error'" }
```

**3. A tool name nobody answers to.** A model can invent a name, and a name with no edge
is a routing error by design -- the message dead-letters, loudly, and the round then waits
for a result that never comes. If that matters in your tree, give the lane a floor: a
catch-all edge from the dispatcher into a cell that replies with a `tool_result` under the
same `tool_call_id`, keyed on the names you did **not** wire.

## Reading the script

| input | emissions |
|---|---|
| `tool_call` turns present, count ≤ budget | `calls` (the assistant turn), then one `tool` per call, in bundle order |
| `tool_call` turns present, count > budget | `calls`, then one `result` per call: `call budget exceeded`, no tool message at all |
| a call whose `text` is not `{name, arguments}` | `result` with `error_code: malformed_tool_call` in place of that one call; the sound calls still run |
| no calls, `finish_reason == 'stop'` | one `answer` |
| anything else | nothing (empty multi-send, terminal) |

The OpenAI unwrap is the only content work the cell does: the `llm` cell emits a
`tool_call` turn whose `text` is the stringified `function` object, and a tool cell wants
the arguments alone. The `id` survives that unwrap unchanged -- everything downstream
correlates on it.

Pinned in [`crates/meclaw-cells/tests/dispatcher_split.rs`](../../../crates/meclaw-cells/tests/dispatcher_split.rs):
the script half runs the shipped `script_inline` against real stdin documents, the colony
half boots this template and routes a two-tool round through real edges.
