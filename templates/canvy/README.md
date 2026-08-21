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

## The four cells

`client/` is the fifth row of the table and is **not** a cell: it is an asset
directory inside `render/`, with no `config.json` and no endpoint.

| | what it is | what it must never do |
|---|---|---|
| `render` | the surface: layout + every tag | read a database, compute an edge path |
| `store` | positions, viewport, topology snapshot | be addressed from outside the hive |
| `refresh` | a timer, every minute by default (`CANVY_REFRESH_CRON`) | know what a topology is |
| `probe` | asks `/colony/graph`, writes the snapshot | sit in a browser's request path |
| `render/client/` | this hive's own JS and CSS — files, not a cell | live in the binary |

## The boundary

The hive seals itself with **`ports: []`**: the hive path is the only address,
and no edge reaches a cell in here — not the store, and not the render cell
either. What a caller asks for rides on `hop.route`:

| lane | direction | carries |
|---|---|---|
| `in_refresh` | in | take the topology snapshot **now**, instead of at the next tick |
| `surface` | out | the drawn page, on the marked egress, back to the browser that asked |

Which cell serves a lane is this hive's business and is stated exactly once, on
the hive's own door edge. `canvy@0.2` declared `ports: ["render", "refresh"]`,
which are the names of two cells in here — a caller had to know the inside in
order to address it. Both addresses are retired; an edge that used to name
`./canvy/refresh` becomes `./canvy` with
`modifier.set_hop.route: "'in_refresh'"`.

The HTTP layer is not an edge and so not covered by the seal — what keeps it
honest is the **route**, which can only ever address a cell that declared
`cell.surface`, and the store does not.

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
 "diff": {"add_nodes": [{"name": "canvy", "template": "canvy@0.3.1"}]}}
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
mutation, not on mouse movement, which makes a minute an honest interval rather
than a compromise.

**And it is a knob.** `refresh` carries one schedule whose cron is
`${CANVY_REFRESH_CRON:-0 * * * * *}` — a 6-field Quartz expression, planned in
**UTC** like every `timer` in the library. The default is "second 0 of every
minute"; a colony whose graph barely moves can set it to `0 */5 * * * *` and pay
a fifth as much, and one that never mutates can stop the tick entirely and drive
the snapshot from the `in_refresh` lane instead.

## The store's one table

Four kinds are written into one `canvas` table, discriminated by `kind`, and the
renderer reads a fifth it no longer writes:

| `kind` | carries |
|---|---|
| `node` | `id`, `x`, `y` — where one box sits |
| `hive_shift` | `id`, `x`, `y` — how far a GROUP was pushed by hand (GH #170) |
| `camera` | `x`, `y`, `z` — the viewport, zoom as integer per-mille |
| `graph` | `doc` — the whole `/colony/graph` answer |
| `hive` | `id`, `x`, `y` — the **legacy** shape of `hive_shift`: a point in the flow layout's space. Read and converted on the way in, never written again |

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
wrapping when the run gets too long for the shape of a screen. There is no fixed
pixel width: the wrap limit is derived per block from `TARGET_RATIO = 2.0` (about
twice as wide as tall) and only considered at all past `WRAP_MIN_W = 1100`, so a
short chain stays one readable stripe. The first version stacked the hives in one
column, which on a 14-hive colony produced a 3672-pixel-tall strip two boxes wide
— deterministic, correct, and unusable. An arrangement in one column carries one
bit of information where a screen offers two dimensions.

The `<svg>` carries a **`viewBox` covering the whole drawing**, so the browser fits
the entire arrangement into the frame before any JavaScript runs. Zoom and pan ride
on top of that as the camera transform on `g.viewport`; the camera the store holds
is applied server-side, so a saved view survives a reload without the client.

Three things are the client's and only the client's, all of them in
`client/surface.js`, mounted through `phx-hook="Canvy"`:

| | |
|---|---|
| **every edge path** | the server sends endpoints and a lane, never a `d` — one routing algorithm, one language |
| **the drag** | a cell, or a whole hive; between pointerdown and pointerup; on release it says "the user let go at 700,240" and the server answers with where the box *is* |
| **pan and zoom** | drag the empty canvas, wheel to zoom around the cursor |

**`id` and `phx-hook` on the canvas element are load-bearing.** A LiveView hook
mounts only on an element carrying both. Without them the client never runs, and
what reaches the browser is a picture with no lines that cannot be moved — which is
exactly what every join served until 2026-08-17, with all the server-side tests
green. They were green because they all asserted about the markup and none about
the seam between the markup and the client. Two tests now cover it: one reads the
hook name out of `surface.js` and looks for it in the rendered markup, and one
mounts the hook against a hand-built DOM.

### Moving a group

Grab a hive anywhere inside its frame — the empty space is the handle — and the
frame, its label and every cell in it move together. On release the client sends
**one** event carrying the group's new box origin, and the server writes **one**
row for it, whatever the hive's size. Twenty cells do not become forty store round
trips on an interactive path.

What is stored is the shift itself — how far the hand pushed the group, measured
against nothing. Neither rectangle it could be measured against survives a colony
that lives: its own frame is derived from the members, so it moves whenever one of
them moves; its corner in the automatic layout moves whenever any cell ANYWHERE
arrives, because that layout is a function of the whole node set. The second one
shipped, and instantiating six cells in a hand-arranged colony walked 12 of its 19
frames off (GH #170). A shift cannot be reinterpreted by a colony growing.

The rectangle stays derived from where the members ended up, which is what lets a
cell dragged out of a crowd GROW its hive instead of being stranded outside a stale
frame. The precedence reads one way and only one way:

    a cell somebody placed by hand  >  the offset of its hive  >  the automatic layout

so moving a hive never silently undoes a hand-placed cell inside it. A row in the
older shape — the box origin rather than the shift — is read once through the
layout it was written against and rewritten, so an arrangement made before GH #170
comes back exactly as it was left.

A hive's frame is the frame around its **whole subtree**, so dragging one takes
every cell and every nested frame below it. Ancestors are deliberately left alone
during the drag: their frames are derived, and the server's answer grows them — a
parent that stretched on the client would be guessing.

### Rows that name nothing

A position outlives the thing it describes. Remove a cell and its row stays; rename
a hive and the row keeps the old name. The picture is unharmed — a row naming
nothing is skipped — but the table drifts away from the registry, and a count over
it stops meaning anything.

The legend says how many such rows there are and offers a **sweep**. It is a press
and not a housekeeping pass on purpose: **the colony has no rename**. A mutation
says `remove_nodes` and `add_nodes`, so a renamed hive is a name that vanished and
a different name that appeared, and nothing in the table can tell that from a
removal. On the 53-cell colony this was written against, all four hive rows naming
nothing were renames — `talky/keeper` → `talky/session-keeper`, `archive` →
`day-archive`, and two more — so a render that swept on its own initiative would
have deleted four hand-placed group positions and nothing else. The snapshot
arrives on a timer as well, so "absent from the picture" also reads as "the tick
has not run since this cell arrived".

The operator who removed the cell is the only one who knows which happened, so the
deletion is their gesture. A press with nothing to shed writes nothing (GH #184).

### Hive in hive

Every ancestor of every cell is a hive and gets a frame, whether or not it holds a
cell of its own: `/org/acme/member/al/assistants/egon/talky/session-keeper` draws seven
nested frames. The layout is recursive and the same shape at every depth — a hive's
own cells on top as rows by flow depth, its child hives packed into shelves below —
so a parent packs its children by their size without knowing anything about their
insides.

The frames are **derived**, one per hive path, from every cell beneath it. An
ancestor is padded more than its descendants (`NEST_PAD` per level), which is what
makes a parent's frame *strictly* contain a child's rather than share an edge with
it: two boxes that touch read as two boxes bumping into each other.

Each depth gets its own faint tint (`depth-1` … `depth-10` in `surface.css`, matched
to `HIVE_DEPTH_TINTS` in `render.py` and checked by a test that reads both files).
They stack, because hives are emitted parent-first, so four nested frames read as
four layers. A colony reaches depth eight in practice, which is why the palette does
not stop at three.

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
