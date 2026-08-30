# `colony-view@1.0.0`

The colony, drawn. A timer takes a topology snapshot, a `code` cell turns it
into one view, and a display holds it and serves the page. The browser owns two
things and neither of them is the picture: the drag, and where you are looking.

```
refresh (timer)  ->  probe (code)  ->  layout (code)  ->  view
   every minute       the colony's       one component      out of the hive,
                      graph endpoint     tree               towards a display
```

## What an app is here

An app is a template whose `template.json` carries the tag `app`. That is the
whole marker -- `tags` is a field the template scanner already reads, so a
catalogue of apps costs no substrate change and no new field.

**An app owns no channel and no port.** It has no listener to bind, no origin of
its own, no address a browser could type. What it produces is a **view**: the
whole picture, stated on a lane, for whatever surface is wired to hold it. Which
surface that is is the wiring's question and never this template's -- the same
drawing can go to a display on a laptop, a display behind a proxy, or to two
displays at once, and nothing in here changes for any of it.

That split is what this template exists to demonstrate. `canvy` fused the two
halves: it drew the colony AND served it, so a second canvas was a second port
and a second access rule. Here the surface is somebody else's template and this
one is the picture.

## The pipeline, pass by pass

**`refresh`** is a `timer` with one schedule and no opinions.
`COLONY_VIEW_REFRESH_CRON` is the knob -- a six-field Quartz expression, planned
in **UTC** like every `timer` in the library. It ticks at second zero of every
minute unless an instance says otherwise.

**`probe`** asks the colony's read-only graph endpoint and hands the answer on,
unread. Two passes: a tick becomes a read, a reply becomes a snapshot. It is
deliberately not part of anybody's request path -- see *Why the topology is a
snapshot* below.

**`layout`** is the whole picture, and it has **one** pass:

> a snapshot arrives with a graph on it -> compute the drawing and emit exactly
> one message on the `view` lane. Anything else -> emit nothing.

`canvy` needed three. Its second pass existed because it wrote **into** a
display and had to interrogate that display first: what does it already hold,
which of those objects are mine, is this page mine at all. Its third existed
only to recognise the acknowledgement of its own write -- and that pass was not
bookkeeping. A cell that cannot recognise the reply to its own patch has no way
to stop: one tick became two, two became four, and the routing loop wedged on a
full mailbox inside twenty seconds
([#161](https://github.com/mmeyerlein/meclaw/issues/161)).

A view is a **statement**, not a patch. Nothing answers this cell, so there is
nothing to recognise, and the discriminator that had to exist has nothing left
to discriminate. Gone with the two passes: the query over the page, the
comparison against what the display holds, every `object.*` call, the page
assignment, the deletes, and the vocabulary repair that kept an older display's
components in step. The display does all of that now, from the tree it is
handed.

## The picture is data, not code

Four components, handed over with the view and stored by the display as rows:

| Component | What it draws | `editable` |
|---|---|---|
| `colony-view-shell` | the frame: the SVG, the arrow markers, the detail panel, the legend -- and the browser half in a `<style>` and a `<script>` | -- |
| `colony-view-hive` | one dashed, tinted rectangle per hive, one tint per depth | -- |
| `colony-view-edge` | one line per edge, plus the fat invisible twin a mouse can hit | -- |
| `colony-view-node` | one box per cell, coloured by cell type | `x`, `y` |

Every name begins with `colony-view-`, and that is a rule rather than a habit:
the display refuses a component whose name does not start with the view's own
id, and says `component_prefix`. A display may hold views from several apps, so
a name that did not carry its origin would be a name two apps could both claim.

A new kind of thing on the canvas is therefore one more component and one more
branch of the tree -- a template edit, never a release. The binary that serves
it knows none of this.

**`colony-view-node` declares `editable: ["x","y"]`, and that declaration is the
whole authorisation model.** The display checks it against the **component**,
never against the message: a browser says what it wants changed, and the
component says what may be. A prop that is not on the list is refused with
`not_editable` and nothing is written.

## `keep`, and why there is no pin

A drag is a **local write** inside the display: pointer down, move, up, then two
writes -- one for `x`, one for `y` -- landing in the display's own database and
diffed to every open browser without a single message entering the colony
router. Dragging a node is not a conversation with anybody.

What makes it **survive** is one word in the tree. Each node's entry declares:

```json
{"component": "colony-view-node", "props": {"x": 120, "y": 40}, "keep": ["x", "y"]}
```

On an object the display already holds, the props named in `keep` are left out
of the update; an update merges per key, so the value the browser wrote stays.
On a create every prop is written, which is how a box nobody has touched still
gets the spot the layout computed for it.

**This is the deliberate difference from `canvy`, and it is a removal.** There
the coordinate could not say who put it there, so a separate `pinned` marker
said it: a drag set the marker, the layout read every node object back off the
display each tick, kept the marked coordinates, computed the rest around them,
and the detail panel offered a *release to the layout* button to clear the
marker again. All of that is gone -- the marker, the read-back, the anchoring of
unpinned siblings to pinned clusters, the eviction of pin-free blocks out of
foreign frames, and the button. **They are not replaced by anything.** The
display keeps the browser's coordinates because the tree told it to, and this
cell simply never learns them.

The cost is stated plainly: a box a hand has moved is never rearranged again by
the layout, and the flow is only ever consulted for boxes nobody has touched. An
arrangement and a growing colony are reconciled by the hand that made the
arrangement.

The hive frames stay derived on both sides -- the client recomputes them from
the same constants the layout used, read off the markup rather than kept as a
second copy -- so a cell dragged out of a hive grows that hive, and every hive
above it, while the cursor is still down.

## The camera is yours

Pan and zoom are local state. Nothing is sent and nothing is stored. A fresh
load starts from the picture's own `viewBox`, which frames the whole drawing
before any script runs, so the canvas is legible even if the client never loads
at all.

## The browser half is a file

`layout/colony-view.js` and `layout/colony-view.css` are files a person reads,
greps and diffs. They reach the browser as two raw props of the shell component,
and they exist in three places: the files themselves, two constants in
`layout/layout.py`, and the copy of that script inside `layout/config.json`.

There is no generator here. The constants are cut out by marker lines, one raw
triple-quoted literal each, nothing escaped:

```text
# --- BEGIN colony-view.js ---
CLIENT_JS = r"""...the file, verbatim..."""
# --- END colony-view.js ---
```

A test compares all three copies and fails if any of them drifts. If the browser
half ever needs a triple quote in it, the answer is to change the browser half,
not the extraction -- a splice gate that can be argued with is not a gate.

## Wiring one

The hive is the address. `params.ports` is empty, so no edge reaches a cell
inside it; a caller names the hive and a lane on `hop.route`.

| Lane | Direction | Meaning |
|---|---|---|
| `in_refresh` | in | take the topology snapshot now, instead of at the next tick |
| `view` | out | the whole picture, ready to be laid on a surface |

Nothing is drawn until somebody is wired to hold it. One edge does that, from
this hive to a display, renaming the lane on the way:

```json
{"from": "./colony-view", "to": "./display",
 "condition": "has(hop.route) && hop.route == 'view'",
 "modifier": {"set_hop": {"route": "'in_view'"}}}
```

That edge is the whole integration. Point a second one at another display and
both show the same picture; point none anywhere and the view dead-letters as
`no_route`, which is recorded and self-localising rather than silent.

`in_refresh` exists for the case where a mutation has just landed and waiting a
minute is silly.

### The one absolute lane

```json
{"from": "./probe", "to": "/colony/graph",
 "condition": "has(hop.route) && hop.route == 'ask_colony'"}
```

The hive carries this lane itself, in its own `config.json`, because an absolute
endpoint is out of scope for any mutation -- bootstrap-only, the same shape and
the same reason as the receptionist's mutation lane.

**The condition is not optional, and leaving it off does not merely route too
much.** An edge matches every emission of the cell it starts at, and the probe
emits on two different lanes. Granted unconditionally, the lane would send the
snapshot to the graph endpoint as well; each one comes back as a graph answer,
each answer produces another snapshot, and the growth is exponential. What that
looks like from the outside is a colony that stops routing after a few seconds
with an **empty** dead-letter queue and nothing in the message log -- the routing
loop is blocked on a full mailbox, so there is no record of what it was
carrying. That was the wedge above, and it cost most of a day to find.

## Why the topology is a snapshot and not a live read

Two rules meet here.

**No cell reads a database it does not own** (`docs/meclaw-overview.md`
§ Database isolation) -- not even reading, so the graph comes from the colony's
read-only graph endpoint, by message, out of its in-memory registry. That
endpoint is one of the absolute endpoints a mutation may draw, precisely because
it is the sanctioned alternative to reading `colony.db`;
`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` enumerates them. The mutation, trace and
dead-letter endpoints stay out of bounds -- authority transfer, and other cells'
message content.

**A `/colony/*` reply arrives on a fresh envelope**: new trace, no
`parent_message_id`, no `correlation_id`, no `context`. So that leg could never
be part of a browser's request -- the answer would come back carrying nothing
that says which browser asked, and a cell has no way to supply the pairing
afterwards.

An app has no request path for it to be part of anyway, and that is the deeper
reason. It states the picture on a timer, where **nothing is waiting**. A
colony's graph changes on mutation, not on mouse movement, which makes a minute
an honest interval rather than a compromise.

## What is not here

- **No port and no origin.** An app has no surface. There is nothing to bind,
  nothing to proxy and nothing to move.
- **No authentication and no TLS, permanently.** Not deferred -- there is no
  request to authenticate. Whatever guards the display guards this too, and one
  location block in front of that display is the whole access rule.
- **No object vocabulary beyond cells, hives and edges.** It draws what the
  topology snapshot holds and nothing else.
- **No stored camera.** Where a person is looking is theirs.
- **No client test.** `canvy` runs its geometry under plain `node` from a Rust
  test, and every client-side defect that canvas ever had was invisible to the
  server-side tests -- the hook was never mounted, and when it did run its edge
  call threw. That harness is **not** carried into this first cut. The routing
  geometry here is the same code, but it arrives unproven: the client path is
  still never provable over the websocket alone, and here that is a stated gap
  rather than a covered one. Carrying it over is the obvious next thing to do
  and it has not been done.
