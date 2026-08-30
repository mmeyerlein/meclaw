# `display@1.0.0`

One screen, on a port of its own, that many agents and applications write onto
at the same time. A **view** is a named, owned, optionally expiring piece of
that screen: whoever sends one owns it, replaces it under the same name, and
takes it down again. Nobody who writes to it needs to know that anybody else
does.

```
in_view / in_withdraw  ->  compose (code)  <->  views (store)
                                 |                what is up
                                 v
                              web (display)   the page, on its own port
```

## What it is not

- **Not a window manager.** Nothing overlaps, nothing has a z-order, nothing is
  resized, and there is no camera. A screen is a column of views in one order,
  and that order is time.
- **Not a model.** The compose cell is deterministic and offline: it opens no
  socket, asks nothing and decides nothing about content. Given the same table
  and the same display it produces the same bundle.
- **Not a layout judgement.** It places views newest first and breaks a tie on
  `(owner, view_id)`. That is the whole of its taste. An application that wants
  a different arrangement builds it inside its own view, where it belongs.
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
   this cell already knows. Expired views drop out here, the rest is sorted, and
   the result travels on `hop.display_views` with one `query` at the display.
3. **The display answered.** The question that answer settles is *is this page
   mine*, and there are two ways it is not: there is no page at `/` at all, or
   there is one and its root is somebody else's. Both are the **bootstrap** case
   -- see below. Then, either way, one bundle of `object.*` calls. The order of
   the screen lives in `ord`, and `object.update` writes props and nothing else,
   so a view that moved up the column is patched with an `object.move` beside
   whatever else changed about it.
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

## What is on the screen: the `views` table

| column | type | what it holds |
|---|---|---|
| `owner` | `text` | the `envelope.reply_to` of whoever put the view up |
| `view_id` | `text` | that sender's own name for it, `[a-z0-9-]{1,64}` |
| `region` | `text` | where on the screen. v1 knows one: `main` |
| `kind` | `text` | `prose` or `component` |
| `content` | `json` | the prose `{title, body}`, or the root node of a component tree |
| `components` | `json` | the `component.define` arguments the view brought with it |
| `ttl_ms` | `int` | how long the view stays fresh. `0` is forever |
| `updated_at` | `int` | epoch milliseconds, which is what orders the screen |

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
| `invalid_view` | a missing or wrongly typed field, an unknown `kind`, an unknown `region` |
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

A node of a component tree may declare `"keep": ["x", "y"]`. On an
`object.update` against an object the display **already holds**, those prop keys
are left out of the call. `object.update` merges per key, so the value the
browser wrote stands. On an `object.create` everything is written, because there
is nothing to preserve yet.

That makes `keep` the exact counterpart of the component's own `editable`
declaration: the component says what a browser **may** write, and `keep` says
that the next tick will not write over it. A hand-set position survives every
tick that does not mean to move it, and the authorisation model stays where the
display enforces it.

## The components this scope defines

| component | layer | what it is |
|---|---|---|
| `display-shell` | `content` | the page root. `stylesheet` emits the link to the token sheet |
| `display-region` | `content` | the one child of the root, and the parent of every view |
| `display-view-prose` | `navigation` | a glass card with an optional title and a paragraph |
| `display-view-custom` | `content` | the wrapper an application's own tree hangs in |

**The region is on that list because the root gets exactly one child** -- a
materialised page interleaves statics and slots one for one, and a root with
several direct children would put the closing static in the middle of the page
(`web` README, *Both shipped pages give their root exactly one child*). So the
root holds the region, and every view hangs under the region.

`display-view-prose` writes `glass--thin`, and glass is a navigation-layer
material -- a content component that names one of the three glass classes is
refused at `component.define`. `display-view-custom` is content on purpose, and
that half is load-bearing: glass never sits on glass, so a content wrapper is
what lets an application put its own glass pane inside a view.

None of them is `editable`. A prop a browser may write is an authorisation
an application grants over its **own** component; the frame around it is not a
thing anybody drags.

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
- **One port per screen.** Two screens are two instances on two ports, not two
  routes on one, which is what makes a single reverse-proxy location block a
  complete access statement for one of them. The `7899` in this template is the
  first port, not the only one; a second screen takes its own with
  `override_params`.

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
proxy in front of it -- nginx, traefik, caddy -- and let that terminate TLS and
decide who gets through. The default bind is `127.0.0.1`, because a surface that
never authenticates must not be reachable off-host by default; setting `bind` to
`0.0.0.0` is a decision somebody makes on purpose, and since the display cell
can rebind a running listener it is a decision that can also be taken back.

Whoever reaches the port sees the screen and can write whatever a component
declared `editable`. There is no allowlist, no rate limit and no session of its
own. Anything in the object tree is on a page, and the page is served to whoever
reaches the port -- a view is not a place for a secret.
