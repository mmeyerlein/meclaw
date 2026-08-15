# examples/hello

The smallest colony that does something. A root hive, one `llm` cell, and one edge.

This is meclaw stripped to the bone. Two cells and a single edge. If you understand this folder,
you understand the model. There is no step three.

## The tree

```
hello/
└── main/                  the root hive. holds the graph (the one edge).
    ├── config.json        type: "hive"
    ├── responder/
    │   └── config.json    type: "llm"
    └── sink/
        └── config.json    type: "code". a terminal that swallows, so the answer is visible
                           in the trace and nothing dead-letters
```

`config.json` says what a node is. The folder it sits in says where it is. The hive's
`params.graph` has exactly one edge: `responder -> sink`. That edge is the only routing in the
whole colony.

A truly lone `llm` cell with no edges would just answer into the void (its emission matches no
edge and dead-letters, which is the documented "routes to nobody" behavior). The one-line `sink`
edge here exists only so you can watch the answer arrive.

## Run it

```bash
# from the repo root, on a fresh release build
./target/release/meclaw --root ./examples/hello --daemon --api 127.0.0.1:7777
# open http://127.0.0.1:7777/ui/
```

The `llm` cell needs an OpenAI-compatible key (OpenRouter by default). Export it, or drop it in
`examples/hello/.env` and point the daemon at it:

```
OPENROUTER_API_KEY=sk-...
```

```bash
./target/release/meclaw --root ./examples/hello --daemon --api 127.0.0.1:7777 --env ./examples/hello/.env
```

Without a key the colony still boots and the UI still loads. The `llm` cell just returns an auth
error as a normal message.

## Drive it

```bash
curl -X POST http://127.0.0.1:7777/messages -H 'Content-Type: application/json' -d '{
  "target": "/responder",
  "body": {"messages": [{"origin": "user", "type": "text", "text": "Say hello in one short sentence."}]}
}'
```

Then look at the trace, in the UI or directly:

```bash
TID=$(curl -s 'http://127.0.0.1:7777/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7777/ui/trace?trace_id=$TID"
```

You will see two hops: `@external -> /responder` (your question) and `/responder -> /sink` (the
model's answer). One call, one message, one edge.

Ready for the next step? `examples/swarm` wires the same `llm` cell to tool cells with a loopback
edge, and the tool-loop becomes a shape in the tree.
