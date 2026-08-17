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

### By mutation, into a colony that is already running

```json
{"scope": "/org/acme/member/alice",
 "ctx": {},
 "diff": {"add_nodes": [{"name": "canvy", "template": "canvy@0.1.0"}]}}
```

That is the whole installation. No restart, no lane to grant, no edge for the
parent to draw: the page answers over HTTP as soon as the mutation commits.

Nothing needs to point at it either — the only way in is the HTTP route and the
only way out is the colony's egress door. A hive nothing points at is normally a
defect; here it is the design.

Two rules had to move before that sentence was true
([#163](https://github.com/mmeyerlein/meclaw/issues/163)), and both are worth
knowing because they are what the hive's own two unusual edges rest on:

- **The egress door is not a place.** It used to open only at the root hive, so
  the lane carrying an answer back had to be `-> /` — and no mutation may draw an
  edge that leaves its own subtree. Now the *marker* decides: with
  `EgressPolicy::Marked` a message that carries the mark leaves from whichever
  hive it ran out of graph at, and only the HTTP layer can mint the mark (a cell
  cannot write `context`). So the lane is `./render -> .` and stays at home.
  Direct-Mode (`EgressPolicy::All`, stdout) is unchanged and still root-only:
  there the mark means nothing, and a dead end deep in the tree is a real dead
  end.
- **`/colony/graph` is drawable by a mutation.** It is the one absolute endpoint
  that is, because it is not a cell — it is the colony's read-only topology
  endpoint, dispatched before any edge is consulted, and it is the *sanctioned*
  way to learn topology, since § Database isolation forbids reading `colony.db`.
  Refusing the lane never protected anything; it only meant a canvas had to be
  born with it or somebody would go read the database instead.
  `/colony/mutations`, `/colony/trace` and `/colony/dead_letters` stay out of
  bounds (authority transfer, and other cells' message content).

### By bootstrap

The same tree in a colony's bootstrap directory needs nothing added to the
parent's `config.json` either — the hive carries both lanes itself:

```json
{"from": "./probe", "to": "/colony/graph",
 "condition": "has(hop.route) && hop.route == 'ask_colony'"}
```

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

## The arrangement, and what the client owns

The default layout is two levels. Inside a hive: rows by flow depth, so a request
sits above the thing it asks. Between hives: **packed into rows**, left to right,
wrapping at 2400 px. The first version stacked the hives in one column, which on a
14-hive colony produced a 3672-pixel-tall strip two boxes wide — deterministic,
correct, and unusable. An arrangement in one column carries one bit of information
where a screen offers two dimensions.

The `<svg>` carries a **`viewBox` covering the whole drawing**, so the browser fits
the entire arrangement into the frame before any JavaScript runs. Zoom and pan ride
on top of that as the camera transform on `g.viewport`; the camera the store holds
is applied server-side, so a saved view survives a reload without the client.

Three things are the client's and only the client's, all of them in
`client/surface.js`, mounted through `phx-hook="Canvy"`:

| | |
|---|---|
| **every edge path** | the server sends endpoints and a lane, never a `d` — one routing algorithm, one language |
| **the drag** | between pointerdown and pointerup; on release it says "the user let go at 700,240" and the server answers with where the box *is* |
| **pan and zoom** | drag the empty canvas, wheel to zoom around the cursor |

**`id` and `phx-hook` on the canvas element are load-bearing.** A LiveView hook
mounts only on an element carrying both. Without them the client never runs, and
what reaches the browser is a picture with no lines that cannot be moved — which is
exactly what every join served until 2026-08-17, with all the server-side tests
green. They were green because they all asserted about the markup and none about
the seam between the markup and the client. Two tests now cover it: one reads the
hook name out of `surface.js` and looks for it in the rendered markup, and one
mounts the hook against a hand-built DOM.

## Editing the picture

`render.py` owns every tag. `client/surface.js` owns how a line is routed, how a
drag feels and where the camera is. Neither is in the binary, so a canvas that
should look different costs a template edit and not a release — which is the whole
reason this is a hive and not a route.

The client has its own suite, and `cargo test` runs it:

```bash
node templates/canvy/render/client/surface.test.js
```

It started as 19 property tests for the edge routing, because the routing was the
part that was visibly wrong — lines ran under the cells they connected. It grew a
section that mounts the hook, because the second time this view was reported broken
the geometry was fine and the hook was the problem: it was never attached, and the
one expression it evaluated to fill in a path threw. A test file that only exists
is a comment, so a Rust test runs this one now.
