# examples/swarm

The showcase colony. A small tool-loop where the loop is not code. It is an edge.

This is the colony the top-level quickstart points at. It is deliberately small, but every
hop is real: an `llm` cell thinks, two tool cells do the work, and a loopback edge feeds the
tool result back to the `llm` for a second pass. Nobody wrote a `while` loop. The shape in the
filesystem is the loop.

## The topology

```
/prep        code   turns your question into persona + tool schemas + a turn for the llm
/llm         llm    one call, one message. either a tool_call or a final answer
/dispatch    code   reads the tool_call, routes it by tool name
/lookup      code   tool: returns a short canned fact
/calc        code   tool: evaluates a simple arithmetic expression
/collector   code   rebuilds the thread (question + tool_call + result) and sends it back
/done        code   terminal sink. the final answer already lives in the trace
```

The edges (in `main/config.json`) are the interesting part:

```
prep      -> llm                                 (carry the question into context)
llm       -> dispatch     when finish_reason == 'tool_calls'
llm       -> done         when finish_reason == 'stop'
dispatch  -> lookup       when tool_name == 'lookup'
dispatch  -> calc         when tool_name == 'calc'
lookup    -> collector
calc      -> collector
collector -> llm                                 <-- THE LOOP. it is an edge.
```

`collector -> llm` is the whole point. That single edge is the tool-loop. Delete it and the
swarm answers blind. Add a smarter condition and you change how the agent reasons, without
touching a line of cell code.

## Run it

```bash
# from the repo root, on a fresh release build
./target/release/meclaw --root ./examples/swarm --daemon --api 127.0.0.1:7777
# open http://127.0.0.1:7777/ui/
```

The `llm` cells talk to an OpenAI-compatible endpoint (OpenRouter by default). Give them a key,
either by exporting it before you start the daemon or by dropping it in `examples/swarm/.env`:

```
OPENROUTER_API_KEY=sk-...
```

Then start the daemon pointed at that env file:

```bash
./target/release/meclaw --root ./examples/swarm --daemon --api 127.0.0.1:7777 --env ./examples/swarm/.env
```

No key? The colony still boots and the UI still loads. The `llm` cell just returns an auth
error as a normal message instead of an answer. Nothing crashes.

## Drive it

Send the swarm a task. It enters at `/prep`:

```bash
curl -X POST http://127.0.0.1:7777/messages -H 'Content-Type: application/json' -d '{
  "target": "/prep",
  "body": {"messages": [{"origin": "user", "type": "text", "text": "What is 6 * 7? Use a tool."}]}
}'
```

Then watch it in the UI, or pull the trace directly:

```bash
TID=$(curl -s 'http://127.0.0.1:7777/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7777/ui/trace?trace_id=$TID"
```

You will see the full chain: `prep -> llm -> dispatch -> calc -> collector -> llm -> done`, the
loopback firing once, and the final answer "The result of 6 * 7 is 42." Ask it "What is meclaw?
Use the lookup tool." and the `lookup` branch lights up instead.

## What this demonstrates, honestly

- The tool-loop is topology, not control flow. One edge, `collector -> llm`, is the loop.
- Cells are dumb. `/calc` has no idea a loop exists. It gets one message, does one thing.
- The edges do the thinking: routing by `finish_reason` and `tool_name`, carrying the question
  through context.

It is single-iteration by design (`parallel_tool_calls` is off, so the `llm` calls one tool at
a time and answers on the next pass). For the store-backed, multi-tool, fan-in version of this
pattern, the building blocks are all here. Wire a `store` cell in and let the collector
accumulate. That is exactly the kind of good first contribution we are looking for.
