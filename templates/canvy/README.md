# `canvy@2.1.4`

One interactive canvas of the colony, served on a port of its own. A timer takes
a topology snapshot, a `code` cell turns it into display objects, and a `web`
cell holds those objects and serves the page. The browser owns two things and
neither of them is the picture: the drag, and where you are looking.

The first — and so far only — thing it draws is the colony itself.

```
refresh (timer)  ->  probe (code)  ->  layout (code)  ->  web (display)
     every minute        the colony's       objects,          the page,
                         graph endpoint     not markup        on its own port
```

## What changed in 2.0.0, and why the first digit moved

1.x drew HTML. A `code` cell produced the whole SVG on every request, a `store`
cell held the positions, and the page was served by the HTTP API under
`/surface/<cell path>`. All three are gone:

| 1.x | 2.0.0 |
|---|---|
| `render` (code) emitted markup | `layout` (code) emits **objects** |
| `store` held positions and the snapshot | the display's own object tree holds both |
| served by `--api` under `/surface/…` | served by the `web` cell on **its own port** |
| a drag was two `code` cell calls (~34 ms) | a drag is a **local write** inside the display |
| the camera was written back to the store | the camera never leaves the browser |

That is a removal-shaped change on every address the template offered, which is
the first digit (the `telegram-connector@2.0.0` precedent). An instance of 1.x is
not upgraded in place — it is instantiated fresh beside the old one, its saved
positions replayed as object patches, and the old hive retired by disconnect.
Running that is an operator's act and this repository only ships the recipe.

## The pipeline, pass by pass

**`refresh`** is a `timer` with one schedule and no opinions.
`${CANVY_REFRESH_CRON:-0 * * * * *}` — a 6-field Quartz expression, planned in
**UTC** like every `timer` in the library. Default: second 0 of every minute.

**`probe`** asks the colony's read-only graph endpoint and hands the answer on,
unread. Two passes: a tick becomes a read, a reply becomes a snapshot. It is
deliberately not part of anybody's request path — see *Why the topology is a
snapshot* below.

**`layout`** is the whole picture. Three passes, and the third one is the one
that matters:

1. a snapshot arrives → emit one `query` over the page, with the graph riding
   along on the hop, which the hive's own edge promotes into `context`. `hop`
   survives exactly one edge and pass 2 is two edges away, so carrying it on the
   hop alone would lose it silently.
2. the display answers → compute the layout and emit **one bundle** of
   `object.*` calls. The question the answer settles is **"is this page mine"**,
   and there are two ways it is not. A display whose page has never been set
   answers `query` with `invalid_input`; a display whose `/` carries somebody
   else's root answers successfully, with a tree that does not contain
   `canvy`. Both are the **bootstrap** case, and the same bundle then defines
   the components, creates the root and sets the page.

   **Correction ([#402](https://github.com/mmeyerlein/meclaw/issues/402)):** this
   said the refusal was the bootstrap signal and the only one there is. It was
   not, and reading it that way made `canvy` unusable over its own substrate:
   `canvy/web` is a `ref` to `web@1.1.0`, which **seeds a demo page at `/`**, so
   the `query` succeeded, the branch never ran, the `canvy-*` components were
   never defined, and every `object.create` in the bundle came back
   `unknown_component` while the deletes landed. The bootstrap pass adopts a
   foreign page and **deletes nothing** while doing it — those objects are not
   this cell's to remove, and another route may still point at them.
3. the display acknowledges the patch → emit **nothing**. A cell that cannot
   recognise the reply to its own write has no way to stop; in 1.x that mistake
   turned one tick into two into four and wedged the routing loop on a full
   mailbox inside twenty seconds ([#161](https://github.com/mmeyerlein/meclaw/issues/161)).

**`web`** is a reference to [`web@1.1.0`](../web/) with one default overridden:
the port. It holds four tables — objects, components, pages, assets — renders
its pages once into a materialised tree, and serves them from that. **A page
load therefore costs no cell call at all**: a colony that is wedged still serves
its picture, and the browser then visibly fails to *connect*, which is a state a
person can read.

## The picture is data, not code

Four components, defined by message on the bootstrap pass and stored as rows:

| Component | What it draws | `editable` |
|---|---|---|
| `canvy-shell` | the frame: the SVG, the arrow markers, the detail panel, the legend — and the browser half of canvy in a `<style>` and a `<script>` | — |
| `canvy-hive` | one dashed, tinted rectangle per hive, one tint per depth | — |
| `canvy-edge` | one line per edge, plus the fat invisible twin a mouse can hit | — |
| `canvy-node` | one box per cell, coloured by cell type | `x`, `y` |

A new kind of thing on the canvas is therefore one more component and one more
patch — a template edit, never a release. The binary that serves it knows none
of this.

**`canvy-node` declares `editable: ["x","y"]`, and that declaration is the whole
authorisation model.** The display checks it against the **component**, never
against the message: a browser says what it wants changed, and the component
says what may be. A prop that is not on the list is refused with `not_editable`
and nothing is written.

## A drag costs no message

Pointer down, move, up. On release the browser sends `object:set` twice — once
for `x`, once for `y` — and that event is the display's **local** lane: the
write lands in the cell's own database and is diffed to every open browser
without a single message entering the colony router. Dragging a node is not a
conversation with anybody.

Dragging a **hive** is the same gesture repeated over its members. There is no
group row and nothing to measure a group against: 1.x stored a *shift* per hive
because its positions lived in a table the flow layout re-derived on every
render, and a point measured against a layout that every arriving cell changes
does not survive the colony growing — twelve of nineteen hand-placed frames in a
real colony walked off ([#170](https://github.com/mmeyerlein/meclaw/issues/170)).
Here the members' own `x`/`y` **are** the record, so moving the group is moving
the members, and every frame follows from where they end up.

The frames themselves are derived and never stored, on both sides: the client
recomputes them from the same constants the layout cell used — read off the
markup rather than kept as a second copy — so a cell dragged out of a hive grows
that hive, and every hive above it, while the cursor is still down.

**And a position, once set, is kept.** On every tick the layout reads back what
the display already holds and leaves those coordinates alone; only a cell the
display has never seen is given a computed spot, which is then settled out of
the way of everything that was placed by hand.

## The camera is yours

Pan and zoom are local state. Nothing is sent and nothing is stored — 1.x wrote
the camera back on a 400 ms debounce, which spent a store round trip on a
gesture that means nothing to anybody else. A fresh load starts from the
picture's own `viewBox`, which frames the whole drawing before any script runs,
so the canvas is legible even if the client never loads at all.

## The browser half is a file, and it is tested

`layout/canvy.js` and `layout/canvy.css` are files a person reads, greps and
diffs. They reach the browser as two raw props of the root object, spliced into
the layout cell's `script_inline` by `scripts/canvy_sync.py`; a test fails if
the copies ever drift.

`layout/canvy.test.js` runs under plain `node` and is invoked from
`crates/meclaw-cells/tests/canvy2_client_geometry.rs`. That is not a
convenience. Every client-side defect this canvas has ever had was invisible to
the server-side tests — the hook was never mounted, and when it did run its edge
call threw — so **the client path is never proven over the websocket alone**.

## Wiring one

The hive is the address. `params.ports` is empty, so no edge reaches a cell
inside it; a caller names the hive and a lane on `hop.route`.

| Lane | Direction | Meaning |
|---|---|---|
| `in_refresh` | in | take the topology snapshot now, instead of at the next tick |
| `event` | out | something a person did in the browser that this hive does not handle itself |

Nothing has to point at canvy at all: the way in is the HTTP port the display
owns. `in_refresh` exists for the case where a mutation has just landed and
waiting a minute is silly.

Nothing inside consumes `event`, and that is deliberate — a browser event nobody
wired for dead-letters as `no_route`, recorded and self-localising, which is
state (2) of [#284](https://github.com/mmeyerlein/meclaw/issues/284) rather than
a silence.

### The port

The display's port is the one knob an instance almost always sets. The template
ships `7810`; a second canvas in the same colony needs a different one, because
two displays sharing a port is a bind race rather than a configuration.

```json
{"add_nodes": [{"path": "/ops", "name": "canvy", "template": "canvy@2.1.4",
                "override_params": {"web": {"port": 7900}}}]}
```

**RETRACTED (GH #410, `canvy@2.1.0`).** Up to `canvy@2.0.1` this page said:
*"The port is **immutable once the cell exists**: a params update naming it is
refused, loudly and without partial apply, because rebinding a live display
would move it out from under whatever reverse proxy is pointed at it."* **That
refusal is withdrawn** — `web@1.1.0` rebinds a running listener. The port is
still chosen here, at instantiation, and it is still the identity that keeps two
canvases apart; it is simply no longer irreversible. To move a live canvas, send
its display cell a params update:

```json
{"params": {"bind": "0.0.0.0", "port": 7901}}
```

The canvas keeps its `cell.db` — every node position a person dragged is exactly
where it was — and the open browsers reconnect on their own. This is what
`MIGRATION.md` step 1's "pick a free port" no longer costs you if you pick the
wrong one.

### Auth and TLS are somebody else's job, permanently

The display binds `127.0.0.1` by default and grows no authentication story ever
(R-W8-2). Put a reverse proxy in front of it and one location block is the whole
access rule for one canvas — which is also why two canvases are two hives on two
ports rather than two paths on one.

### The one absolute lane

```json
{"from": "./probe", "to": "/colony/graph",
 "condition": "has(hop.route) && hop.route == 'ask_colony'"}
```

The hive carries this lane itself, in its own `config.json`, because an absolute
endpoint is out of scope for any mutation — bootstrap-only, the same shape and
the same reason as the receptionist's mutation lane
([#163](https://github.com/mmeyerlein/meclaw/issues/163)).

**The condition is not optional, and leaving it off does not merely route too
much.** An edge matches every emission of the cell it starts at, and the probe
emits on two different lanes. Granted unconditionally, the lane would send the
snapshot to the graph endpoint as well; each one comes back as a graph answer,
each answer produces another snapshot, and the growth is exponential. What that
looks like from the outside is a colony that stops routing after about twenty
seconds with an **empty** dead-letter queue and nothing in the message log — the
routing loop is blocked on a full mailbox, so there is no record of what it was
carrying. That was
[#161](https://github.com/mmeyerlein/meclaw/issues/161), and it cost most of a
day to find.

## Why the topology is a snapshot and not a live read

Two rules meet here.

**No cell reads a database it does not own** (`docs/meclaw-overview.md`
§ Database isolation) — not even reading, so the graph comes from the colony's
read-only graph endpoint, by message, out of its in-memory registry. That
endpoint is one of the absolute endpoints a mutation may draw, precisely because
it is the sanctioned alternative to reading `colony.db`;
`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` enumerates them, and the
counts-never-content ledger endpoint joined it with
[#267](https://github.com/mmeyerlein/meclaw/issues/267). The mutation, trace and
dead-letter endpoints stay out of bounds — authority transfer, and other cells'
message content.

**A `/colony/*` reply arrives on a fresh envelope**: new trace, no
`parent_message_id`, no `correlation_id`, no `context`. So that leg could never
be part of a browser's request — the answer would come back carrying nothing
that says which browser asked. `templates/builder-hive` hits the same wall and
works around it with an on-disk in-flight pointer, which is only sound because a
lease guarantees one request at a time; with several browsers it is not.

So the snapshot is taken where **nothing is waiting**. A colony's graph changes
on mutation, not on mouse movement, which makes a minute an honest interval
rather than a compromise. In 2.0.0 there is not even a request path for it to be
part of: the display serves from a materialised tree, so nothing a browser does
reaches a `code` cell at all.
