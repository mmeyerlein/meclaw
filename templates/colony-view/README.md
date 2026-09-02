# `colony-view@1.0.2`

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
| `colony-view-hive` | one dashed, tinted rectangle per hive, one tint per depth, labelled `<name> · <level>` | `x`, `y`, `w`, `h` |
| `colony-view-edge` | one line per edge, plus the fat invisible twin a mouse can hit | -- |
| `colony-view-node` | one box per cell, coloured by cell type | `hand`, `pinned` |

Every name begins with `colony-view-`, and that is a rule rather than a habit:
the display refuses a component whose name does not start with the view's own
id, and says `component_prefix`. A display may hold views from several apps, so
a name that did not carry its origin would be a name two apps could both claim.

**A declared v-lane is drawn differently.** A mutation may name the lane that
made a deep edge legal (GH #559), and `/colony/graph` reports it on the edge.
The layout hands it down as the `vlane` prop and the browser half draws such an
edge dashed, with the lane's name in its tooltip. It is a rendering and nothing
more: the component keeps `editable: []`, nothing is written back, and an edge
that declares no lane looks exactly as it did before.

A new kind of thing on the canvas is therefore one more component and one more
branch of the tree -- a template edit, never a release. The binary that serves
it knows none of this.

**`colony-view-node` declares `editable: ["hand","pinned"]`, and that
declaration is the whole authorisation model.** The display checks it against the **component**,
never against the message: a browser says what it wants changed, and the
component says what may be. A prop that is not on the list is refused with
`not_editable` and nothing is written.

## The flow's half and the hand's half

A box carries **two** translates, and the split is the whole of how an
arrangement and a growing colony live together:

```
transform="translate(x, y) translate(hand)"
             ^ the flow's       ^ the hand's, ONE prop: "dx,dy"
```

`x`/`y` are this cell's, recomputed and rewritten on **every** tick. `hand` is a
hand's, written by a drag and never by this cell -- an offset against the
cell's own spot, so it travels with its hive instead of being left behind when
the next arriving cell re-ranks the columns. SVG composes a transform list left
to right, so the pair needs no arithmetic anywhere, which matters: the component
language has none.

A drag is a **local write** inside the display: pointer down, move, up, then one
write -- `hand` -- plus a second the first time a box is moved at all, landing in
the display's own database and diffed to every open browser without a single
message entering the colony router. Dragging a node is not a conversation with
anybody.

**One write, because a prop at a time is a picture at a time.** The offset was
two props first, and a browser writes a prop at a time while the display diffs a
prop at a time -- so one drag reached the page as two pictures, and the frames,
derived from where the boxes are, were derived once from the half-moved one.
Measured in a browser, sampling what was actually PAINTED per animation frame:
three rectangles for a single drag, the middle one 971 wide and still 92 high.
And what this browser wrote now stands until a diff says it
(`colony-view.js:reconcile`) -- the first diff after a drag can still carry the
value from before it, which puts the box back where it started for a beat and
draws the frame around that. Both together: one painted rectangle per drag.

What makes it **survive** a tick is one word in the tree. Each node's entry
declares:

```json
{"component": "colony-view-node", "props": {"x": 120, "y": 40, "hand": "0,0", "pinned": ""},
 "key": "n.os~builder~compose", "keep": ["hand", "pinned"]}
```

On an object the display already holds, the props named in `keep` are left out
of the update; an update merges per key, so the value the browser wrote stays.
`x` and `y` are deliberately **not** among them.

## The pin is a marker, and the marker is the whole of it

`pinned` says a hand was here. A coordinate says nothing -- and reading one as a
pin is the defect of 1.0.0, measured on a running colony of 104 cells in
[#544](https://github.com/mmeyerlein/meclaw/issues/544): every box froze at
whatever the flow had computed on the tick its object happened to be created, so
the picture became a collage of a dozen incompatible layouts. 208 of 215
unrelated hive pairs had overlapping frames, one frame ran 299x the area of the
three cells inside it, and 1133 (hive, foreign cell) pairs intersected. Nobody
had dragged anything; nobody could, because `data-oid` named an object one tree
level away from the one the display mints, so every drag wrote to an id that
does not exist.

`canvy` had already learnt that a pin needs a marker of its own (2.1.8), and the
re-cut of [#455](https://github.com/mmeyerlein/meclaw/issues/455) lost it. This
is that lesson, kept, plus two things `canvy` did not have:

- **A box is named by its cell, not by its slot.** The node's tree entry carries
  a `key` (`display`, since 1.0.1) -- `n.<path with ~ for />` for a cell, `h.…`
  for a hive -- so the object id is `<parent>/<key>` rather than
  `<parent>/<index>`. An index is a slot: the picture writes hives, then
  edges, then cells, so one edge more shifts every box's index by one and hands
  the kept props to whichever cell inherits the slot. That is how 103 of 104
  boxes came to wear a position computed for somebody else.
- **The flow owns where a box sits, and it says so every tick.** `x`/`y` are
  recomputed and rewritten on every snapshot, so a box nobody has touched stands
  where the flow put it -- and the flow is disjoint, nested, and 1.08x-1.43x the
  boxes it holds. A hand adds an offset beside that, never in place of it.

**The offset is NOT bounded, and that was measured rather than assumed.** 1.0.1
first shipped a bound: an offset was trimmed to its own hive's frame, which made
the four target points of #544 hold by construction. On a real screen, at the
zoom a whole colony is looked at, a shrink-wrapped frame left a box **85 x 15
screen pixels** of travel before it hit a wall. The gesture read as broken, and a
constraint a person experiences as a broken gesture is not a constraint, it is a
bug. So the bound is gone: a hand may put a box anywhere, and the frames follow
the boxes rather than fencing them.

What is therefore no longer promised is written down rather than hoped away: **an
arrangement can put two frames over each other**, because a hand asked for it.
The durable answer is for the app to REMEMBER the arrangement -- a store in the
app hive and an event lane back from the screen, so the flow can pack around what
a hand did instead of being blind to it. That is a build, not a repair, and it is
filed as its own issue. The sentence is kept beside the code in
`layout.py:hive_of_cell_frames`.

**An outermost frame is not a grab handle.** A frame with no parent frame in
the picture is not a frame around anything -- it is the canvas, and on a real
colony it is 96 % empty space, so almost every press that misses a box lands
inside it and inside nothing else. Measured on a live colony of 108 cells: ONE
press on that emptiness dragged every cell and marked every one of them
hand-placed -- the whole picture leaving the layout in a single gesture. A root
frame therefore pans; the group drag is offered for the frames that are
actually groups. And a group drag that did not move anything writes nothing: one
delta is applied to every member, so a delta that rounds to nothing leaves every
box where it was, marks nobody hand-placed and reaches the display not at all.

The detail panel says which of the two placed a box, and offers the way back:
*by hand -- release* clears the marker and both halves of the offset, and the
next tick puts the box where the layout wants it.

What is still gone from `canvy`, and is not coming back: the read-back of the
display's objects into this cell, the anchoring of unpinned siblings to pinned
clusters, and the eviction of pin-free blocks out of foreign frames. This cell
never learns a position, and it no longer needs to: it owns `x`/`y` outright and
says them again on every tick, and the hand's offset rides on top of whatever it
last said.

The hive frames stay derived on both sides -- the client recomputes them from
the same constants the layout used, read off the markup rather than kept as a
second copy -- so a hive re-shapes itself around its members while the cursor is
still down instead of waiting a minute for the next tick. A frame FOLLOWS its
cells: a box is inside its own hive by construction however far it was dragged,
and what the browser derives it also writes back, so the store says what the
screen shows.

## A frame says which level it is

Since 1.0.2 a hive frame is labelled `<name> · <level>` rather than with its
directory name alone: `alex · member` around the person, `alex · assistant`
around the agent inside them. A member is a person and an assistant is a
generation of that person's agent, so the obvious name for the first generation
is the person's own name -- and two nested frames carrying one word left the
reader counting rectangles to tell them apart
([#549](https://github.com/mmeyerlein/meclaw/issues/549)).

The level is READ OFF THE PATH and nothing new travels for it. The composition
levels address their children through fixed containers, so a frame whose parent
segment is `orgs`, `members`, `assistants`, `channels` or `apps` is an `org`, a
`member`, an `assistant`, a `channel` or an `app`; anything else is a plain
`hive`. `/colony/graph` carries no `kind` and no template provenance -- its
`nodes[]` entries have exactly two keys and hives are not nodes at all, they
exist only as prefixes -- and putting a level on that wire would be a public API
round for one consumer. Five string comparisons in the layout cost no contract,
and a tree that does not use the composition levels reads `hive` everywhere,
which is the honest answer rather than a guess.

The label is single-sourced here. The browser half places it -- `hiveParts`
takes the FIRST `<text>` of a frame and moves it with the rectangle -- and never
writes its content, so the component keeps exactly one text element and the
layout decides what it says.

## Unwired cells are hidden by default

A cell that takes part in no edge at all is `unwired`, and since 1.0.1 **the
default picture does not draw it**: the shell ships `class="hide-unwired"` and
the legend carries a `<n> unwired` button that brings them back. In this
substrate an unwired cell is almost always a leftover, because `remove_nodes`
and `swap_nodes` DISCONNECT and never delete (`docs/rewiring.en.md`: getting rid
of a registry row is an operator action with the colony stopped), so a live
colony accumulates them -- 31 of 123 cells on the one this was built against, 13
of them in a single hive. They are part of the truth and they are not part of
what the colony DOES, which is exactly what a toggle is for.

Hidden cells shape nothing: the layout computes the hive frames and the `viewBox`
over the cells the picture shows, and flipping the toggle re-derives the FRAMES in
the browser, from whatever is left visible -- a frame drawn around boxes nobody is
looking at is a frame around nothing. The `viewBox` is deliberately not re-derived
with them: a session keeps the frame it mounted with (§ *The camera is yours*), so
a toggle never re-centres the picture. The toggle is a class on the container and
nothing else: no round trip, no stored preference, the same kind of local fact as
where you are looking.

## The camera is yours

Pan and zoom are local state. Nothing is sent and nothing is stored. A fresh
load starts from the picture's own `viewBox`, which frames the whole drawing
before any script runs, so the canvas is legible even if the client never loads
at all.

That first fit is also the only one a session takes. The server re-fits the
`viewBox` over the content's bounding box on every layout tick, and a diff
carrying a new fit would jump the whole picture to re-centre it -- which reads as
"the page reloaded" to whoever is looking at it. So the frame the page mounted
with is recorded once and pinned across every later morph, and across the unwired
toggle: a session's view moves only by its own hand.

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
