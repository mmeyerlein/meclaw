# meclaw demo fixtures

Bootstrap-valide Fixtures fuer die Phase-12-Demo (HTTP-API + Web-UI +
Blob-Storage). Form ist 1:1 die aus den gruenen E2E-Tests
(`phase_12_b_demo.rs`, `phase_12_x_e2e_attachment_in_trace.rs`,
`phase_12_b_mutations_post.rs`).

## Layout

```
examples/
  demo-colony/
    demo/                    # hive-marker (FS-Name wird beim Bootstrap zu meclaw /)
      config.json            # {"cell":{"type":"hive"}}
    templates/               # Templates-Registry (vom Boot-Scan geladen)
      echo/
        template.json        # Template-Index ({"name":"echo"})
        config.json          # Template-Body (bash-Cell, leere params)
  demo-mutation.json         # add_nodes scope=/ name=echo template=echo
```

## Pfad-Mapping (wichtig)

`assert_single_root_dir` strippt den FS-root-name: das einzige nicht-blacklisted
Top-Level-Verzeichnis im `--root` wird zu meclaw `/`. Bei `<root>/demo/` heisst
das: das hive `demo` ist im meclaw-Namespace `/`, eine cell darunter z.B.
`/echo`. Die Mutation referenziert daher `"scope": "/"` (nicht `/demo`).

`config.json` benutzt **immer** den `cell`-Block (`{"cell":{"type":"hive"}}`),
nie `{"type":"hive"}` direkt — `plan_bootstrap` parsed sonst nichts.

## Demo-Lauf

```bash
# 1. Build
cargo build --workspace --release

# 2. Fixtures nach /tmp kopieren (No-Delete-Policy: kein in-place-Schreiben in examples/)
cp -r examples/demo-colony /tmp/demo-colony

# 3. meclaw mit --api + --blobs starten
./target/release/meclaw \
  --root /tmp/demo-colony \
  --api 127.0.0.1:7777 \
  --blobs /tmp/demo-colony/blobs

# 4. In zweitem Terminal: 8-Punkte-Demo
curl http://127.0.0.1:7777/health                                   # 200 "ok"
curl http://127.0.0.1:7777/ui/                                      # Dashboard
curl -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d @examples/demo-mutation.json                                   # 200 Committed
curl http://127.0.0.1:7777/ui/registry                              # /echo sichtbar
curl -X POST http://127.0.0.1:7777/messages \
  -F target=/echo \
  -F attachment=@examples/demo-mutation.json                        # 202 + attachments[]
# Trace-View ist search-by-trace_id (siehe Phase-12-Limitations
# "/ui/trace ist search-by-trace_id"). Hop-Baum mit attachments[] sichtbar
# machen: jüngste trace_id aus der JSON-Sicht ziehen, dann UI ansteuern.
TID=$(curl -s 'http://127.0.0.1:7777/colony/trace?limit=1' | jq -r '.trace[0].trace_id')
curl "http://127.0.0.1:7777/ui/trace?trace_id=$TID"                 # Hop-Baum mit attachments[]
# Listen-/Recent-Sicht via JSON: /colony/trace?limit=N. Die /ui/trace-HTML
# ist eine Such-Ansicht (trace_id eingeben).
curl 'http://127.0.0.1:7777/ui/dead_letters'                        # leer (alles routed)
curl 'http://127.0.0.1:7777/colony/events'                          # 501 (Phase-14-Defer)
# Ctrl-C -> graceful shutdown (axum drain -> ColonyMsg::Shutdown -> colony_join)
```

## JSON-`POST /messages`

Der Smoke oben nutzt die multipart-Form. Aequivalent als klassischer JSON-POST
(`{target, body, headers?, ttl?}`), hier mit explizitem `ttl`-Override:

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

- `ttl` ist optional: positiver Integer; absent/`null` → `colony.json`
  `message_default_ttl`; alles andere → `422 invalid_ttl`.
- `headers` ist optional und geht ins persistente `context`-Fach.
- Fire-and-forget: Response ist `202 {message_id}`. Das Tool-Result der
  `/echo`-Cell laeuft ohne `reply_to` (der JSON-Ingress setzt keins) in die
  Terminal-Kette aus `docs/meclaw-overview.md` § Envelope-Setter-Authority
  (`reply_to`-Spezialfall) — danach ist die DLQ **nicht mehr leer**; der
  „leer"-Check der 8-Punkte-Demo gehoert deshalb VOR diesen POST.

## Was die Mutation tut

`demo-mutation.json` instanziert eine `bash`-Cell unter `/echo`. Das
Template `echo` ist ein Name-Wrapper — das tatsaechliche Verhalten kommt
aus dem `bash`-Built-in (Phase 7 stateless tool-cell). Die Cell akzeptiert
default-`params` (leeres Object) und ist sofort routable.

## Anti-Vorgriff

- KEIN Body::Blob-Auto-Offload — multipart-Files landen ausschliesslich im
  `attachments[]`-Slot der UBF-Message.
- KEIN `blob_inline_max_bytes`-Read (Phase 13).
- KEINE Auth-Middleware (Hardening = Post-Roadmap; lokale Disziplin
  `--api 127.0.0.1:7777`).
- `/colony/events` antwortet 501 (Phase-14-Defer, kein WebSocket).
