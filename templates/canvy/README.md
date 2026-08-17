# canvy — a canvas the colony serves itself

One interactive page, served over HTTP under a cell path, drawn entirely
server-side by a `code` cell. The browser owns exactly one thing: the drag.

```
GET /surface/<path to canvy/render>          the page
GET /surface/<path to canvy/render>/live/websocket   the transport
GET /surface/<path to canvy/render>/@asset/surface.js   this hive's own JS
GET /surface/@client/phoenix_live_view.min.js           the binary's bundles
```

Everything a canvas needs sits under **one** prefix, which is the point:

```nginx
location /surface/org/acme/member/alice/canvy/ { ... }
```

is a complete access rule for that canvas — page, assets and transport — and it
needs to know nothing about MeClaw.

## The five cells

| | what it is | what it must never do |
|---|---|---|
| `render` | the surface: layout + every tag | read a database, compute an edge path |
| `store` | positions, viewport, topology snapshot | be addressed from outside the hive |
| `refresh` | a timer, 60 s | know what a topology is |
| `probe` | asks `/colony/graph`, writes the snapshot | sit in a browser's request path |
| `client/` | this hive's own JS and CSS | live in the binary |

The hive seals itself with `ports: ["render", "refresh"]`: no later mutation can
wire around the render cell straight into the store. The HTTP layer is not an
edge and so not covered by that seal — what keeps it honest is the **route**,
which can only ever address a cell that declared `cell.surface`, and the store
does not.

## Two round trips, and the numbers

A **page load** costs **zero** cell calls: the dead render is protocol
scaffolding from the binary, and the picture arrives in the join reply. A colony
that is wedged still serves the page, and the client then visibly fails to
connect.

A **join** and a **drop** each cost **two** `code` cell calls, ≈34 ms — pass 1
asks the store, pass 2 renders and (on a drop) writes. A `code` cell invocation is
~16 ms of interpreter start, measured (`plans/p13-collector-measurement.md`). A
second viewer of an unchanged canvas costs **zero**: the binary caches the last
render per surface and replaces it the moment a newer one arrives.

A **drag** costs nothing. The server sees the start and the end of a drag and
never the movement in between.

## Installing it

### By mutation — works, but the canvas needs feeding

```json
{"scope": "/org/acme/member/alice",
 "ctx": {},
 "diff": {"add_nodes": [{"name": "canvy", "template": "canvy@0.1.0"}]}}
```

Nothing needs to point at it: the only way in is the HTTP route and the only way
out is the colony's egress door. A hive nothing points at is normally a defect;
here it is the design.

What a mutation **cannot** grant is the lane to `/colony/graph`. An absolute edge
endpoint is out of scope for any mutation (`scope_out_of_bounds`), so installed
this way the topology snapshot never arrives and the canvas reports that it has
none. Feed the store from outside — one message to `./probe` carrying a `graph` slot is
enough, and the probe writes the snapshot the same way it would from a tick:

```bash
curl -s -X POST localhost:PORT/messages -H 'Content-Type: application/json' -d '{
  "target": "/…/canvy/probe",
  "body": {"messages": [],
           "graph": {"scope": "/", "nodes": [{"path": "/a/b", "cell_type": "code"}],
                     "edges": []}}}'
```

`messages: []` is not decoration: the ingress validates the UBF body and a document
without that slot is `422 invalid_ubf_body`.

### By bootstrap — the topology refreshes itself

Declare the node **and** the privileged lane in the colony's root
`config.json`, the same shape and the same reason as the receptionist's mutation
lane:

```json
{"from": "./org/acme/member/alice/canvy/probe", "to": "/colony/graph",
 "condition": "has(hop.route) && hop.route == 'ask_colony'"}
```

Only a bootstrap `config.json` can write that edge, which is precisely what keeps
the privilege from spreading: no mutation can mint it at any scope.

**The condition is not optional, and leaving it off does not merely route too
much.** An edge matches every emission of the cell it starts at, and the probe
emits two store writes for every snapshot it takes. Granted unconditionally, the
lane sends those writes to `/colony/graph` as well; each one comes back as a graph
answer, each answer produces two more writes, and the growth is exponential. What
that looks like from the outside is a colony that stops routing after about twenty
seconds with an **empty** dead-letter queue and nothing in the message log — the
routing loop is blocked on a full mailbox, so there is no record of what it was
carrying. That was [#161](https://github.com/mmeyerlein/meclaw/issues/161), and it
cost most of a day to find, which is why the condition is in the snippet and why a
test reads this file to make sure it stays there.

## Why the topology is a snapshot and not a live read

Two rules meet here.

**No cell reads a database it does not own** (`docs/meclaw-overview.md`
§ Datenbank-Isolation) — not even reading, so the graph comes from
`/colony/graph`, by message, out of the colony's in-memory registry.

**A `/colony/*` reply arrives on a fresh envelope**: new trace, no
`parent_message_id`, no `correlation_id`, no `context`. So that leg cannot be part
of a browser's request — the answer would come back carrying nothing that says
which browser asked. `templates/builder-hive` hits the same wall and works around
it with an on-disk in-flight pointer, which is only sound because a lease
guarantees one request at a time; with several browsers it is not.

So the snapshot is taken where **nothing is waiting**. A colony's graph changes on
mutation, not on mouse movement, which makes 60 seconds an honest interval rather
than a compromise.

## The store's one table

Three kinds share one `canvas` table, discriminated by `kind`:

| `kind` | carries |
|---|---|
| `node` | `id`, `x`, `y` — where one box sits |
| `camera` | `x`, `y`, `z` — the viewport, zoom as integer per-mille |
| `graph` | `doc` — the whole `/colony/graph` answer |

One table because a store message carries exactly **one** operation: three tables
would be three round trips on an interactive path, and a `kind` column costs
nothing. Zoom is per-mille because a store column is `text`, `int` or `json` —
there is no float, and a thousandth is finer than a mouse wheel can express.

A position is **deleted and rewritten**, never appended. The no-delete promise
protects what was said and what was learned; where a rectangle sits is neither.

## Editing the scripts

`render.py` and `probe.py` are the source. `config.json` carries a copy of each in
`script_inline`, because a `code` cell's `script_path` is handed to the interpreter
with no working directory of its own — a relative path would resolve against the
daemon's cwd, and an absolute path baked into a template is the exported-tree
defect class of GH #20. After editing a `.py`:

```bash
python3 scripts/canvy_sync.py
```

`crates/meclaw-cells/tests/canvy_template.rs` fails if the two ever drift.

## Editing the picture

`render.py` owns every tag. `client/surface.js` owns how a line is routed and how
a drag feels. Neither is in the binary, so a canvas that should look different
costs a template edit and not a release — which is the whole reason this is a hive
and not a route.

The 19 property tests that come with the edge routing are its own:

```bash
node templates/canvy/render/client/surface.test.js
```

They exist because the routing was the part that was visibly wrong — lines ran
under the cells they connected — and that is the last defect anyone should have to
report twice.
