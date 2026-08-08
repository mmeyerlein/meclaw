# meclaw demo fixtures

Bootstrap-valid fixtures for the phase-12 demo (HTTP API + web UI +
blob storage). The shape is 1:1 the one from the green E2E tests
(`phase_12_b_demo.rs`, `phase_12_x_e2e_attachment_in_trace.rs`,
`phase_12_b_mutations_post.rs`).

## Layout

```
tests/fixtures/
  demo-colony/
    demo/                    # hive marker (the FS name becomes meclaw / at bootstrap)
      config.json            # {"cell":{"type":"hive"}}
    templates/               # templates registry (loaded by the boot scan)
      echo/
        template.json        # template index ({"name":"echo"})
        config.json          # template body (bash cell, empty params)
  demo-mutation.json         # add_nodes scope=/ name=echo template=echo
```

## Path mapping (important)

`assert_single_root_dir` strips the FS root name: the only non-blacklisted
top-level directory inside `--root` becomes meclaw `/`. For `<root>/demo/` that
means: the hive `demo` is `/` in the meclaw namespace, a cell below it is e.g.
`/echo`. The mutation therefore references `"scope": "/"` (not `/demo`).

`config.json` **always** uses the `cell` block (`{"cell":{"type":"hive"}}`),
never `{"type":"hive"}` directly — otherwise `plan_bootstrap` parses nothing.

## Demo run

```bash
# 1. Build
cargo build --workspace --release

# 2. Copy the fixtures to /tmp (no-delete policy: never write in place in examples/)
cp -r tests/fixtures/demo-colony /tmp/demo-colony

# 3. Start meclaw with --api + --blobs
./target/release/meclaw \
  --root /tmp/demo-colony \
  --api 127.0.0.1:7777 \
  --blobs /tmp/demo-colony/blobs

# 4. In a second terminal: the 8-point demo
curl http://127.0.0.1:7777/health                                   # 200 "ok"
curl http://127.0.0.1:7777/ui/                                      # dashboard
curl -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d @tests/fixtures/demo-mutation.json                                   # 200 Committed
curl http://127.0.0.1:7777/ui/registry                              # /echo visible
curl -X POST http://127.0.0.1:7777/messages \
  -F target=/echo \
  -F attachment=@tests/fixtures/demo-mutation.json                        # 202 + attachments[]
# The trace view is search-by-trace_id (see the phase-12 limitations
# "/ui/trace is search-by-trace_id"). To make the hop tree with attachments[]
# visible: pull the newest trace_id from the JSON view, then open the UI.
TID=$(curl -s 'http://127.0.0.1:7777/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7777/ui/trace?trace_id=$TID"                 # hop tree with attachments[]
# List/recent view via JSON: /colony/trace?limit=N. The /ui/trace HTML page
# is a search view (enter a trace_id).
curl 'http://127.0.0.1:7777/ui/dead_letters'                        # empty (everything routed)
curl 'http://127.0.0.1:7777/colony/events'                          # 501 (phase-14 defer)
# Ctrl-C -> graceful shutdown (axum drain -> ColonyMsg::Shutdown -> colony_join)
```

## JSON `POST /messages`

The smoke test above uses the multipart form. Equivalent as a classic JSON POST
(`{target, body, headers?, ttl?}`), here with an explicit `ttl` override:

```bash
curl -X POST http://127.0.0.1:7777/messages \
  -H 'Content-Type: application/json' \
  -d '{
    "target": "/echo",
    "ttl": 8,
    "body": {
      "messages": [{
        "origin": "assistant", "type": "tool_call",
        "text": "{\"command\": \"echo hello\"}", "id": "call-json-1"
      }]
    }
  }'                                                                # 202 + {message_id}
```

- `ttl` is optional: a positive integer; absent/`null` → `colony.json`
  `message_default_ttl`; anything else → `422 invalid_ttl`.
- `headers` is optional and goes into the persistent `context` compartment.
- Fire-and-forget: the response is `202 {message_id}`. The tool result of the
  `/echo` cell runs without `reply_to` (the JSON ingress sets none) into the
  terminal chain from `docs/meclaw-overview.md` § Envelope setter authority
  (the `reply_to` special case) — after that the DLQ is **no longer empty**; the
  "empty" check of the 8-point demo therefore belongs BEFORE this POST.

## What the mutation does

`demo-mutation.json` instantiates a `bash` cell at `/echo`. The template `echo`
is a name wrapper — the actual behaviour comes from the `bash` built-in (phase 7
stateless tool cell). The cell accepts default `params` (an empty object) and is
routable immediately.

## Deliberately not anticipated

- NO Body::Blob auto-offload — multipart files land exclusively in the
  `attachments[]` slot of the UBF message.
- NO `blob_inline_max_bytes` read (phase 13).
- NO auth middleware (hardening = post-roadmap; local discipline
  `--api 127.0.0.1:7777`).
- `/colony/events` answers 501 (phase-14 defer, no WebSocket).
