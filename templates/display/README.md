# `display@2.0.0`

One screen, reached at `/<mount>/` on the colony's one listener, that many
agents and applications write onto at the same time. A **view** is a named, owned, optionally expiring piece of
that screen: whoever sends one owns it, replaces it under the same name, and
takes it down again. Nobody who writes to it needs to know that anybody else
does.

```
in_view / in_withdraw  ->  compose (code)  <->  views (store)
                                 |                what is up
                                 v
                              web (display)   the page, under its mount
```

## What it is not

- **Not a window manager.** Nothing overlaps, nothing has a z-order, nothing is
  resized, and there is no camera. A screen is two columns of views, and inside
  a column an order that is not time.
- **Not a model.** The compose cell is deterministic and offline: it opens no
  socket, asks nothing and decides nothing about content. Given the same table
  and the same display it produces the same bundle.
- **Not a layout judgement.** Two columns, a band inside a column, and a tie
  broken on `(owner, view_id)`. That is the whole of its taste. An application
  that wants a different arrangement builds it inside its own view, where it
  belongs.
- **Not the owner of content.** What is inside a view is whatever the sender
  sent, rendered by whatever component the sender defined. This scope wraps it,
  places it, and takes it away again.

## The four passes

The pass is decided by the **envelope header** -- `hop.route` on the way in,
`context.display_origin` on the way back -- and never by the shape of the body.
A body is written by whoever sent it; a header is written by the edge that
carried it, and the edges of this hive are the only thing that knows where a
message has already been.

1. **A request.** `in_view` and `in_withdraw` are validated and become **one
   store bundle**: a `select` of the whole table, a `delete` of this owner's row
   for this `view_id`, and -- on `in_view` -- an `insert` of the new one. The
   select is the first leg on purpose: it is the before-state, and the delete is
   about to remove the row that would have described it. The request rides along
   as a JSON string on `hop.display_request`, which the hive's own edge promotes
   into `context`, because `hop` survives exactly one edge. A refusal ends the
   pass here, as one `receipt` and no write at all. The third source of a
   request is a browser event, which is handled below.
2. **The store answered.** The after-state is computed in memory from the
   before-state -- minus the row that was deleted, plus the row that was
   inserted -- because a second `select` would be another round trip for a set
   this cell already knows. Expired views drop out here, the rest is put in a
   deterministic order that reads no clock -- region, band, identity -- and the
   result travels on `hop.display_views` with one `query` at the display.
3. **The display answered.** The question that answer settles is *is this page
   mine*, and there are two ways it is not: there is no page at `/` at all, or
   there is one and its root is somebody else's. Both are the **bootstrap** case
   -- see below. The answer is also where the **seats** come from, which is why
   this pass computes the layout rather than only diffing it. Then, either way,
   one bundle of `object.*` calls. The order of the screen lives in `ord`, and
   `object.update` writes props and nothing else, so a view that moved up the
   column is patched with an `object.move` beside whatever else changed about
   it.
4. **The display acknowledged the patch.** Nothing is emitted, and that is what
   stops the loop. A cell that cannot recognise the reply to its own write has
   no way to stop; one write becomes two, two become four, and the routing loop
   wedges on a full mailbox inside twenty seconds
   ([#161](https://github.com/mmeyerlein/meclaw/issues/161)).

### The bootstrap is not the refusal

A display whose `/` has never been set refuses `query` with `invalid_input`.
A display carrying the `web` template's own **seeded demo page** answers that
query perfectly well, with a tree that contains no root of ours. Reading only
the refusal is the [#402](https://github.com/mmeyerlein/meclaw/issues/402)
defect: the query succeeds, the branch never runs, the vocabulary is never
defined, and every `object.create` comes back `unknown_component` while the
deletes land. So the test is *is our root in the answer*, not *did the answer
arrive*.

The bootstrap **deletes nothing**. Those objects are not this scope's to remove,
and another route may still point at them. `/` is re-pointed at our own root and
the old tree is left standing.

## Two regions, and an order that is not time

A screen has two columns, and a view names the one it wants:

| `region` | what it is for |
|---|---|
| `main` | the wide column, and the **default**. A view that names no region lands here, exactly as it did before there was a second one |
| `aside` | the narrow column beside it, for what simply **stands**: a clock, a weather tile, a countdown. It takes no width at all while it is empty |

Anything else is `invalid_view` and nothing is written. A closed list rather
than a free string, because an unknown region is a view nobody would ever see,
which is worse than a refusal the sender can read.

**Inside a region the order is three keys, and the interesting one is the key
that is missing.**

1. **`ord`**, the band the view declared. Optional, `0` by default, signed: a
   widget that wants to stand above a conversation asks for `-10` instead of
   asking every other sender to move down. It is a sort key and never an index.
2. **First appearance.** A new view sorts behind everything already standing,
   is given the next seat, and keeps that seat through every rewrite until
   something above it goes away.
3. **`(owner, view_id)`**, so the one tie left is broken on identity rather
   than on whatever order the store happened to return.

**The moment a view was last written is not one of them**, and that is the
whole of [#609](https://github.com/mmeyerlein/meclaw/issues/609). Until 1.1.0
a region was sorted newest-first, so a view rewritten every twenty seconds took
the top slot on every tick -- not because it was important, but because it was
recent. That is the right answer for a card and the wrong one for anything
standing, and the two readings cannot share a screen. `updated_at` is an expiry
clock now and nothing else.

**First appearance is remembered by the screen**, not by a column of the table.
The seat of a view is the `ord` the display is already holding it at, which the
compose cell reads back on pass 3 anyway -- so the layout takes what the display
holds as an *input* rather than only as something to diff against. Two
consequences worth knowing: a page this cell has to **bootstrap** has no seats
at all, and every view on it is new together (band, then identity); and a view
that **changes region** is new in the region it arrives in, because a height in
the column it came from means nothing in the column it goes to.

**The regions themselves stand in declaration order**, `main` before `aside`,
as `ord` `0` and `10` under the page root. Both hang there directly. That used
to be impossible -- a materialised page carried two statics whatever the child
count, so the closing static landed between the first child and the second and
everything from the second on rendered outside the element meant to contain it.
[#394](https://github.com/mmeyerlein/meclaw/issues/394) replaced that with n+1
statics for n slots, and the `web` README says as much: a root with one child is
"a composition CHOICE now rather than a constraint".

The **layout** is this scope's own, and it travels in the `display-shell`
template as one `<style>` block rather than as a rule in `/vision.css`: the
token sheet belongs to the `web` template and describes a design language,
while *main is wide and aside is narrow* is a statement about this screen. Two
flex columns, the aside at `clamp(15rem, 22%, 24rem)`, `display: none` while it
is empty, and stacked one above the other under 60rem.

## What is on the screen: the `views` table

| column | type | what it holds |
|---|---|---|
| `owner` | `text` | the `envelope.reply_to` of whoever put the view up |
| `view_id` | `text` | that sender's own name for it, `[a-z0-9-]{1,64}` |
| `region` | `text` | which column it stands in: `main` or `aside` |
| `ord` | `int` | the band it asked for inside that column. `0` by default, signed |
| `kind` | `text` | `prose` or `component` |
| `content` | `json` | the prose `{title, body}`, or the root node of a component tree |
| `components` | `json` | the `component.define` arguments the view brought with it |
| `ttl_ms` | `int` | how long the view stays fresh. `0` is forever |
| `updated_at` | `int` | epoch milliseconds. An **expiry** clock, and since 1.1.0 nothing else |

**`(owner, view_id)` is the identity, and the store cannot say so.** A `store`
schema declaration carries column types and nothing else -- no PRIMARY KEY, no
UNIQUE, no index. So the uniqueness is held by the compose cell, as a **delete
followed by an insert** in one bundle, in that order. A bundle is not a
transaction and does not roll back, which is precisely why the read of the
before-state is a leg of that same bundle rather than a separate round trip:
there is exactly one moment at which the old row is still there and this cell is
already looking.

## The owner is the envelope, never the body

The owner of a view is `envelope.reply_to` -- the path of the cell that emitted
the message. A body may repeat it. A body that repeats it **wrong** is refused
rather than believed, because a sender that could name somebody else's owner
could withdraw somebody else's views. A message with no `reply_to` has no owner
and is refused too: there would be nothing to address a receipt to and nothing
to delete against later.

The refusals are a closed list, all of them on the `receipt` lane, all of them
leaving the table untouched:

| `error_code` | what happened |
|---|---|
| `owner_unknown` | the message carries no `envelope.reply_to` |
| `not_owner` | the body claims an owner that is not the sender |
| `invalid_view` | a missing or wrongly typed field, an unknown `kind`, a region this screen does not have, an `ord` that is not an integer, or a component tree whose form does not hold -- two children of one node naming the same `key`, or a child `key` that is a plain number |
| `component_prefix` | a component name that does not start with `<view_id>-` |
| `store_failed` | a leg of the store bundle came back with an `error_code` |

Every receipt carries `error_code`, `owner`, `view_id` and a `detail` string.
Every one of those keys is always present, empty where unknown: a key that is
sometimes missing is a router branch nobody tests.

## An application brings its own vocabulary

A `kind: "component"` view carries a `components[]` list of `component.define`
arguments beside its tree. **Every name must start with `<view_id>-`.** The
component library of a display is one namespace shared by everything writing to
that screen, and the prefix is what keeps two applications from redefining each
other's vocabulary out from under a page that is already rendered. A name
without it is `component_prefix` and nothing is written.

**The definitions only travel when they changed.** The `components` column holds
what the view brought last time; a write whose components are byte-identical to
the stored ones sends no `component.define` at all. That matters because a
redefinition re-renders **every** route in the display: an application ticking
once a second with an unchanged vocabulary would otherwise re-render the whole
screen once a second for no difference.

The same economy applies one level down. An `object.update` whose props the
display already holds is not sent
([#412](https://github.com/mmeyerlein/meclaw/issues/412)). The display applies a
bundle through its single database actor, and a browser's own `object:set` is
served by that same actor: a full rewrite of an unchanged tree holds it for the
length of the rewrite, and anything a person did in that window is written late
while the rewrite's diffs re-render it where it was.

## `keep`: how a drag survives a tick

A node of a component tree may declare `"keep": ["hand", "pinned"]`. On an
`object.update` against an object the display **already holds**, those prop keys
are left out of the call. `object.update` merges per key, so the value the
browser wrote stands. On an `object.create` everything is written, because there
is nothing to preserve yet.

That makes `keep` the exact counterpart of the component's own `editable`
declaration: the component says what a browser **may** write, and `keep` says
that the next tick will not write over it. A hand-set value survives every tick
that does not mean to move it, and the authorisation model stays where the
display enforces it.

## `key`: what a kept prop is kept ON

An object id is minted from the tree: `view.<owner-slug>.<view_id>` for the
wrapper, then the child index chain below it. That is a function of the tree
alone, which is what makes the same tree sent twice patch the same objects --
and it is an identity only as long as the tree's shape does not move.

It moves. A node of a tree may therefore declare its own `key`, and the object
is then named `<parent>/<key>` instead of `<parent>/<index>`, while `ord` -- the
drawing order -- still comes from the index. A key is any non-empty string of at
most 512 characters without a `/` -- the separator the index chain is written
with, and the only character `parse_object_id` splits on. 512 rather than the
64 an id segment gets elsewhere, because the keys that matter are paths: a cell
path with its slashes written as tildes is already 120 characters deep on a real
colony.

**Two rules the door enforces on every CHILD of a node, since 1.0.2.** A key
must be unique among its **siblings**, and it must not be a plain number. Two
siblings naming the same key mint the same id, and a key like `"3"` names
exactly what the unkeyed fourth child beside it names -- either way one node of
the view would be overwritten by another and disappear from the screen with
nothing in the receipt to act on. So the tree is refused as a whole, the way a
bad `keep` list already is: the receipt carries `invalid_view` and a `detail`
naming the key and the parent it collided under, and nothing is written
([#568](https://github.com/mmeyerlein/meclaw/issues/568)). Give a key a prefix
that an index cannot have (`colony-view` uses `n.` and `h.`) and derive it from
something already unique, and neither refusal is reachable. The root of a
view's tree is exempt from both: it is an only child under a wrapper named for
its owner and its `view_id`, so there is nothing for its key to collide with.

The difference is what `keep` is worth. An index is a **slot**: insert one
sibling ahead of a node and every id behind it now belongs to a different thing,
so the props the sender asked to keep are handed to the new occupant of the
slot. Measured on a running colony under
[#544](https://github.com/mmeyerlein/meclaw/issues/544): a picture whose edge
count had grown by three had 103 of 104 boxes standing at a position that had
been computed for some **other** cell, and three cells held two objects each. A
key that says what the node **is** -- a cell path, a row id -- makes the kept
prop follow the thing, which is the only reading under which `keep` means
anything at all.

A view that names no keys is unchanged -- and a view **kind** names none by
nature: `prose` and `component` say what a view is, not what the nodes of its
tree are called. Keys live in the tree a sender draws, and the one shipped
sender that draws any is `colony-view`, on its hive and cell rectangles: it
derives every key from a colony path behind an `n.`/`h.` prefix, so its keys are
unique by construction and none of them can be a plain number. No shipped view
can trip either refusal.

## The components this scope defines

| component | layer | what it is |
|---|---|---|
| `display-shell` | `content` | the page root. `stylesheet` emits the link to the token sheet, and the shell carries this scope's own two-column rule |
| `display-region` | `content` | one per region, a direct child of the root, and the parent of every view standing in it |
| `display-view-prose` | `navigation` | a glass card with an optional title and a paragraph |
| `display-view-custom` | `content` | the wrapper an application's own tree hangs in |
| `display-mic` | `content` | the hold-to-talk button, its transcript line and its state line, plus the browser half that runs them |

**The region is on that list because a view hangs under a region rather than
under the root** -- that is what makes a column a place. It used to be on it for
a different reason, that the root could hold exactly one child, and that reason
is retracted: [#394](https://github.com/mmeyerlein/meclaw/issues/394) gave a
materialised page n+1 statics for n slots, so the root holds both regions and
renders both.

`display-view-prose` writes `glass--thin`, and glass is a navigation-layer
material -- a content component that names one of the three glass classes is
refused at `component.define`. `display-view-custom` is content on purpose, and
that half is load-bearing: glass never sits on glass, so a content wrapper is
what lets an application put its own glass pane inside a view.

None of them is `editable`. A prop a browser may write is an authorisation
an application grants over its **own** component; the frame around it is not a
thing anybody drags.

## Talking to the screen

The screen carries a button. Hold it -- pointer or space bar -- and what you say
goes to a `voice` cell; let go and the transcript appears on the line beside the
button, and the answer is played back through it.

There is no second port and no second connection. The button joins a
`voice:<call>` topic on the socket the page is already holding, and the `web`
cell hands those frames to whichever cell in the process is mounted under the
name this scope was given. That name is `params.voice_mount` of the `compose`
cell, `voice` by default, so a screen can be pointed at a voice cell that was
mounted as something else. With nothing mounted under the name, the join is
refused and the button says so on the page rather than failing quietly. One
socket holds at most four live calls at once, and a fifth join is refused with
`too many voice topics on this socket` until one of the four is given up.

The frames are the wire protocol's own, unchanged -- `docs/voice-wire-protocol.md`
is the whole of it, and both modes, `4409` and `client_too_slow` mean there what
they mean on a socket of the voice cell's own.

Two browser rules travel with it. A microphone needs a **secure context**: an
`https://` origin, or `localhost` / `127.0.0.1`. Reached over a LAN address on
plain `http://`, the button says the microphone needs https or localhost instead
of asking for a permission the browser will not grant. And the sample rates come
from the `hello` frame, because the cell never resamples: the page adapts, cutting
20 ms PCM16 frames at the rate that was declared.

What is not here: no transcript history, no list of turns, no way to scroll back.
The line beside the button holds the last thing that was heard and nothing more.
A conversation on the screen is a view like any other, put up by whoever owns it
on the `partial` lane of the agent it belongs to.

## `ttl_ms` expires a view, it does not remove it

A view is expired when `now_ms - updated_at >= ttl_ms`. An expired view is not
drawn -- and that is all. **Nothing sweeps.** The row stays in the table, and
the screen stops showing it at the **next** compose, which is the next time
anybody writes or withdraws a view on this screen. A screen nobody writes to
keeps showing an expired view until somebody does. Say it plainly rather than
imply a timer that is not there: a `ttl_ms` is a promise about what will be
drawn, not about when.

## Wiring

A display is a **screen**, and a screen belongs to a person rather than to an
agent. So it stands beside the agents rather than inside one:

```
<member>/channels/display-<screen>
```

- **Agents write to it.** An assistant sends `in_view` and `in_withdraw` at the
  hive path. Its `reply_to` **is** the owner, so two agents never collide under
  the same `view_id` and neither can withdraw the other's view.
- **Applications write to it the same way.** An application stands at
  `<member>/apps/<name>` and sends the identical two lanes. Nothing about the
  wire distinguishes an app from an assistant, which is the point: the screen
  does not have a privileged writer.
- **The screen answers back through the member.** `event` and `receipt` leave
  the hive carrying `owner` and `view_id` **twice**: in the body, for whoever
  reads the message, and on `hop.owner` / `hop.view_id`, for whoever routes it.
  The second copy is not a convenience -- an edge condition in this substrate is
  evaluated against `context.*` and `hop.*` and never against the body
  (`crates/meclaw-colony/src/cel_eval.rs`, `bind_ctx`), so an owner that lived
  only in the body could not be routed on at all (GH #459). Both keys are always
  present and empty where the object id would not parse, so an unattributable
  message fails every owner guard by construction rather than landing somewhere
  arbitrary.

  The member routes on that owner: a path under `assistants/` reaches the agent
  as an ordinary `in_turn` carrying `hop.kind`, one under `apps/` reaches the app
  on the lane it arrived on, and one that is neither leaves the member on
  `error`. That is the whole return path, and it needs no registry.
- **And the viewer's identity, where a proxy named one.** With `identity_header`
  set, every semantic event of that connection carries `hop.user_id`. A `hop` is
  single-hop, so the out-edge of the screen owes it a promotion into the context
  -- `"user_id": "has(hop.user_id) ? hop.user_id : ''"` beside the two channel
  keys in its `set_context`, the same line a channel's entry edge carries
  (`examples/organism/grow-screen.json`). Without it the identity is gone one hop
  past the screen.
- **What `<base href>` means for a component.** The page carries
  `<base href="<prefix>/<mount>/">`, so a relative URL in any component
  resolves under this screen — and a bare `#fragment` resolves against the base
  as well, which makes `href="#foo"` on `/display/a` a navigation to
  `/display/#foo` rather than a scroll. No shipped component writes either; an
  application that wants a fragment link writes it in full (`href="a#foo"`).
- **One mount per screen.** Two screens are two instances under two names on
  the colony's one listener. The `display` in this template is the first name,
  not the only one; a second screen takes its own with `override_params.web.mount`,
  and its page is then at `/<that name>/`. Since `web@2.0.0` there is no port to
  hand out: a single reverse-proxy location block in front of the listener is
  still a complete access statement, and it now reads `location ^~ /<mount>/`.

The hive is the address: `params.ports` is empty, so no edge reaches a cell
inside it. A caller names the hive and a lane on `hop.route`.

| lane | direction | meaning |
|---|---|---|
| `in_view` | in | put this view up under this name, replacing whatever stood there |
| `in_withdraw` | in | take it down |
| `event` | out | something a person did on the screen that the display could not absorb locally |
| `receipt` | out | a write was refused, with the code, the identity and a detail string |

An event whose object id will not parse back into an `(owner, view_id)` leaves
**anyway**, with both keys present and empty. A view the display holds and this
scope cannot attribute is a defect somebody has to see; dropping the event would
make it invisible. Since GH #459 it does not even have to dead-letter: the member
sends an event or a receipt it cannot place out on its own `error` lane, carrying
the lane it was on in `hop.kind`.

## Authentication is external, forever

This scope does not authenticate, and it is not going to (R-W8-2). Put a reverse
proxy in front of the colony's listener -- nginx, traefik, caddy -- and let that
terminate TLS and decide who gets through. Since `web@2.0.0` the screen binds
nothing itself, so where the colony listens is one decision for the whole
colony rather than one per screen. A proxy that serves the screen under a path
of its own says so with `X-Forwarded-Prefix`, and every link the page writes
moves with it.

Whoever reaches the mount sees the screen and can write whatever a component
declared `editable`. There is no allowlist, no rate limit and no session of its
own. Anything in the object tree is on a page, and the page is served to whoever
reaches the mount -- a view is not a place for a secret.
