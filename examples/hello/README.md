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

**The key is a vault grant (#801).** The model cell holds no key: `api_key` is empty and `credential_grant_id` names a grant seeded in `main/access/store/seed/`; the cell asks `./access` for `cred:openrouter` and gets it sealed. The vault opens itself from `key_source: plainfile` (for a long-running unit: `systemd-cred`). The wiring is [`vault-pilot`](../vault-pilot/)'s. To give it a key:

```bash
# 1. grow the broker around the checked-in grant half of ./access (once)
./target/release/meclaw --root ./examples/hello --templates ./templates \
                        --apply ./examples/hello/grow-access.json

# 2. a vault passphrase in a 0600 file outside the checkout, named in the colony's .env
KEYFILE="$HOME/.local/share/meclaw/hello.vault-key"
(umask 077; mkdir -p "$(dirname "$KEYFILE")"; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$KEYFILE")
printf 'MECLAW_VAULT_KEY_FILE=%s\n' "$KEYFILE" > ./examples/hello/.env

# 3. the credential into the vault, from stdin, before the daemon runs
printf '%s' "$OPENROUTER_KEY" | ./target/release/meclaw --root ./examples/hello \
    --vault /main/access/vault --vault-key-source plainfile --vault-key-file "$KEYFILE" \
    --vault-add cred:openrouter

./target/release/meclaw --root ./examples/hello --daemon --api 127.0.0.1:7777 --env ./examples/hello/.env
```

Without a deposit the colony still boots and the UI still loads; the `llm` cell's credential round
is refused and it answers with `credential_pending` as a normal message.

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
