# `display@2.2.3`

One screen, reached at `/<mount>/` on the colony's one listener, that many
agents and applications write onto at the same time. A **view** is a named, owned, optionally expiring piece of
that screen: whoever sends one owns it, replaces it under the same name, and
takes it down again. Nobody who writes to it needs to know that anybody else
does.

```
in_view / in_withdraw / in_notice  ->  compose (code)  <->  views (store)
                                             |                what is up
                                             v
                                          web (display)   the page, under its mount
```

## What the screen is for: display hygiene

A guideline before the mechanics, because every later decision on this screen
is measured against it.

**Focus is the state of the whole screen**, not the highlighting of one active
element. It answers which information should be visible at this moment -- and
which, deliberately, should not. The screen must look clearly structured, calm
and relevant at every moment. Visible is only what has concrete use in the
current context; everything else is hidden, reduced or moved to the back. That
principle is *display hygiene*, and it is a continuous duty of the display
system rather than a one-off design choice: every planned or executed change
of the screen asks what is relevant to the member right now, what has
priority, what supports the current task, what merely distracts, and what can
disappear entirely without losing anything the member needs. The screen is
always reduced to the minimum necessary information state.

**The screen has no agenda of its own.** It is not a source of information.
Its content comes from applications, from the member's agents and from system
states with immediate display relevance, and it shows nothing permanently only
because interfaces traditionally do. A clock is an application like any other
and obeys the same rules of priority, focus and visibility: in a high-focus
situation it is noise and goes; on an otherwise empty screen it may stand.

**Priority is dynamic.** No fixed hierarchy, and the member's main agent does
not automatically outrank everything -- a calendar with an imminent appointment
may matter more than the agent's current output. Whoever judges takes the
current context, the member's activity, time relevance, urgency, importance,
running interactions, the cost of an interruption, the member's own
preferences and the current focus level into account, and decides not only how
something is shown but whether, when and ahead of what.

**Display hygiene is personal.** Members differ in what they want shown,
prioritised, arranged or hidden; those preferences are learned and kept. A
correction the member has to repeat is not a situational correction any more
but, probably, one of that member's display rules, and it becomes part of the
persistent profile that shapes later decisions.

**The guiding sentence:** *show as little as possible at every moment -- and
everything that truly matters at that moment.* Relevance is not static; it
arises from context, time, priority, activity and the member's preferences.

Until 2.1.0 the sheet gave every window the same weight and the compose cell
judged nothing. Since 2.2.0 the screen judges, and the next section says how;
this one is the measure it is built against.

## The screen curates what it shows

**Words.** A **window** is an object of one of four components --
`display-pane`, `display-panel`, `display-overlay`, or the `display-view-prose`
wrapper a prose view renders as. Everything else on the screen is content
inside a window. An application says **hints** about a window, as props on its
tree node or as slots of the `in_view` body: `context` (a word, such as
`conversation` or `ambient`), `relevance` (0-1), `class` (one of
`system_error`, `error`, `warning`, `important_note`, `note`), `pinned`
(boolean), `relevant_until` (epoch milliseconds), `touched` (an epoch, as
text), and `state`, of which it may say exactly two words -- `urgent` and
`hidden`. Any other state word is dropped at the door. The **screen state** lives on the root: `focus`, the bar (0-1),
and `weights`, a JSON map from context to 0-1. The **rung** is what the
compose cell writes on every window as `state`: `hidden`, `ambient`,
`relevant`, `focus` or `urgent` -- the ladder the sheet has had since 2.1.0.
Beside it the cell writes `age` (`fresh`, `settled`, `leaving`), `since` (the
epoch millisecond of the window's last touch) and `score`.

**A touch.** A window is touched when it is new or when its content props --
everything the sender said, minus what the cell writes -- differ from what the
display holds. A touch sets `since` to now; nothing else moves it. Rewriting a
view with the same content is not a touch, so a clock that rewrites itself
every twenty seconds touches nothing.

**`touched`** -- an epoch the application changes when the window's content is
new; the pane's own props are what the screen compares, and an answer that
lives in a child component is invisible to that comparison unless the
application says so (GH #689, since 2.2.2). A speech window whose answer
stands in a `display-text` under it is never touched by the answer alone, so
the application writes the moment of the answer into `touched`, and a changed
`touched` is a touch like any other changed prop. The child's props are not
read on purpose: a clock that rewrites a child every twenty seconds would
otherwise touch its window on every tick. The judge sees the value as
`touched_at` beside the pass's own `touched` flag.

**The score.** For every window, on every pass:

```
w      = weights[context], 0.5 for a context the map does not name
r      = relevance, or the class default, or 0.5
r      = min(r, 0.2)                              once now >= relevant_until
decay  = 1                                        while pinned, or for linger_ms after the touch
       = 1 - (now - since - linger_ms) / fade_ms  after that, down to 0
score  = 0 when the app said hidden; 1 when it said urgent; else w x r x decay
```

**The bar and the weights.** A window is visible only when `score >= focus`.
With no judge on the screen the floor sets the bar to `focus_default` -- unless
no window in `main` reaches it, and then the bar is 0, so what simply stands
may show on an empty screen. The floor's weights are one rule and no memory:
the context of the last touched window weighs 1, every other context weighs
0.5. So a weather window loses half its score the moment the conversation is
touched, and a window that stood below the bar to begin with goes at once.
Once a judge has written the map, the map stands; a touch adds only a context
the map does not know, at 1. A window **below the bar** is `hidden`: gone from
the page, still in the table, back on its next touch.

**The rungs.** Among the visible windows, **exactly one window in `main`** has
the focus rung: the highest score, ties broken by the youngest `since`, then
the id. A window that arrived on this very pass is `fresh` and at most
`relevant` -- it takes the focus one frame later, so the enter keyframe and the
focus lift never fight. `aside` never carries the focus rung: a window there
is `relevant` at or above the midpoint between the bar and 1, `ambient` below
it, and so is every window in `main` that is not the focus. An `urgent` an
application said stands alone: no window is the focus beside it, and of two
urgent windows the younger keeps the word while the older is `relevant`.

**Two frames.** A window leaves over two frames. One the table no longer has
is not deleted on the pass that notices: it is laid back byte for byte with `age: leaving`, so the sheet
plays the leave keyframe, and the next pass deletes it, leaf first. A window
pushed below the bar while standing on the page is `hidden` and `leaving` in
the same update for the same reason. A window on its way out that comes back
is `settled`, never `fresh` -- nothing flies in twice.

**Presence follows the rung.** How loud a window is, a person reads off its
title: at `ambient` it is a caption in the tertiary ink, at `relevant` a small
label, at `focus` the big title, at `urgent` the big title in the accent,
breathing. One title slot on every window, and no fixed kicker form. Two more
words an application may say: `tone` (`accent` colours the window's figure,
`muted` lowers its whole fill to the secondary ink) and `pinned`, which freezes
the decay and, on a pane, is rendered as `data-pinned`.

**The judge: a minimal display model that re-judges the whole screen.** The
floor is one rule; the judge is a model. Beside the compose cell stands
`judge`, an `llm` cell with one prompt, one schema and nothing else -- no
tools, no memory beyond what the root carries, no learning. It is the master
over focus; applications and the floor give hints. When the knob `judge` is
`on`, the compose cell asks it after every pass that touched at least one
window, and **never on a tick**, never on a write whose props were the same
as before, and not twice inside `judge_min_interval_ms` -- inside that
interval the floor judges alone -- and the interval counts from the question
as much as from the answer (`asked_at` on the root), so a judge that never
answers is still asked at most once per interval. What it sees is the whole
situation as one JSON user turn: every window with its id, owner, view id,
region, context, relevance, class, `pinned`, its rung, its `since` and age in
seconds, whether this pass touched it, and a glimpse of its text; the root's
bar and weights; the knobs as sentences. Its instructions are the guideline
above, word for word, and the nine factors it weighs: the current context,
the person's current activity, time relevance, urgency, importance, running
interactions, the cost of an interruption, the person's preferences, and the
current focus level. It answers with one JSON object -- `focus` (the bar),
`weights` (per context), `windows[]` with an optional `hidden` or `relevance`
per id -- and the answer comes back on the lane `in_verdict` as a pass without
a write: the root takes the bar, the weights and `judged_at`, a named window
takes `judged_hidden` or `judged_relevance`, and the pass then curates as
always (rungs, frames, the next due moment). Numbers are clamped to 0-1, an
unknown id is ignored, a fenced answer is unwrapped, and a verdict that is
late (the cell waits eight seconds, `external_timeout_ms`; measured, a small
model over a public gateway answers in two to four), an error
(`finish_reason` other than `stop`) or not JSON changes nothing: the floor has
already drawn, and a touch clears the judged props of the window it touched. **A verdict has a lifetime:** it rules for `linger_ms +
fade_ms` from `judged_at` -- as long as anything it judged could still be
fading -- and then the floor judges again until the next verdict, so a judge
that fell silent never leaves its bar standing over a screen it no longer
sees -- and a verdict's `hidden` and `relevance` on a window fade with the
verdict, so a clock the judge hid comes back by itself. The model, key and endpoint are params of the `judge` cell itself
(`DISPLAY_JUDGE_MODEL`, `DISPLAY_JUDGE_API_KEY`, `DISPLAY_JUDGE_BASE_URL`);
the model is empty as shipped, so a screen without one is judged by the floor
alone. What a judgement costs is tokens per content change -- the situation
is a few hundred tokens, the verdict is capped at 600 -- times content changes
per hour; `judge_min_interval_ms` is the brake.

**The knobs** stand in `params` and in `contract.settings` with the same
default: `linger_ms` (20000), `fade_ms` (120000), `focus_default` (0.3),
`ground` (`day`; `night` switches the sheet's night variant, and nothing
switches it by itself), `judge` (`off`), `judge_min_interval_ms` (3000) and
`notice_defaults` (per class, `[relevance, ttl_ms]`). They are the member's
dials: what a member wants shown differently is another value on that
member's own screen.

## What it is not

- **Not a window manager.** Nothing overlaps, nothing has a z-order, nothing is
  resized, and there is no camera. A screen is two columns of views, and inside
  a column an order that is not time.
- **The compose cell is not the model.** It is deterministic and offline: it
  opens no socket and, given the same table and the same display, produces
  the same bundle. The judge beside it is the one cell in this hive that asks
  a provider, and the screen stands complete without it.
- **A judgement of relevance, not of layout.** Two columns stay two columns;
  what the screen decides is which window is visible, how loud, and for how
  long. Where a window stands is a band inside a column and a tie broken on
  `(owner, view_id)`, and an application that wants a different arrangement
  builds it inside its own view, where it belongs.
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
which is worse than a refusal the sender can read. The columns differ in one
more way since 2.2.0: `aside` never carries the focus rung -- a window there is
relevant or ambient, and may be urgent if its application says so.

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
template rather than as a rule in `/vision.css`: that sheet belongs to the
`web` template and describes the base language of every surface, while *main
is wide and aside is narrow* is a statement about this screen. Two flex
columns, the aside at `clamp(15rem, 22%, 24rem)`, `display: none` while it is
empty, and stacked one above the other under 60rem.

The shell carries it in **one `<style>` block, together with the design
language**: first the layout (where the columns and the microphone sit), then
the faces built from `params.font_base` (below), then the sheet
`compose/display-dna.css`, byte for byte. The sheet comes last on purpose --
it re-tokenises `/vision.css` at equal specificity, so it needs no stacked
`:root` to win. The file is the source; `compose.py` carries the same bytes in
`KIT_CSS`, and a test compares the two with the copy inside `config.json`. So
the language reaches a screen the way everything else does, by
`component.define`, and a change to it is a change to a file in this template
and a new version, never a message a hand sends to a running colony.

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
| `invalid_notice` | a notice whose `class` is not one of the five, that has no `text` and no `error_code` to translate, or whose `view_id` does not match `[a-z0-9-]{1,64}` |
| `store_failed` | a leg of the store bundle came back with an `error_code` |

Every receipt carries `error_code`, `owner`, `view_id` and a `detail` string.
Every one of those keys is always present, empty where unknown: a key that is
sometimes missing is a router branch nobody tests.

## Notices: a classified message becomes a window of its sender

Not everything that belongs on the screen is worth composing a view for. A
sender that only has a sentence and a weight puts it on `in_notice`:

```json
{"messages": [], "class": "note", "text": "the kettle is on"}
```

`class` is one of five words -- `system_error`, `error`, `warning`,
`important_note`, `note` -- and says how loud the notice is; `text` is the
message. The compose cell wraps it into a **prose view owned by the sender**
(`envelope.reply_to`, never the body): the title is the class word, the body
is the text, and `context`, `relevance` and `ttl_ms` come from the class
defaults in `params.notice_defaults` unless the body says otherwise. The view
stands in `main` under the name `notice-<class>-<sha256(text)[:8]>` (the class
word spelt with hyphens), so the same sentence twice is one window and a
different sentence is another; a body may name its own `view_id` instead. The
store bundle is the one a view write builds, and a refusal is one `receipt`
carrying `invalid_notice`.

**A channel's failure is one of them.** The member's graph re-stamps a
channel's `error` towards the screen as `in_notice` (the third screen edge
the `builder` recipe draws since its 1.10.0). Such a message carries no `class` and no `text`, only
`hop.error_code` -- and the cell translates the code through a table that
lives in this template: `stt_failed` becomes *The microphone did not catch
that.*, `speak_failed` *The voice could not speak just now.*, `busy`,
`no_answer` and `call_refused` *The call did not go through.*, `failed` and
`line_write_failed` *The telephone line failed.*; a code the table does not
know is shown as *A part of the colony failed: `<code>`*, with the code in it
on purpose. The class is `system_error`, the context is `system`, and
`meta.detail` -- the socket code, the provider's sentence -- **never reaches
the screen**. The screen's own clock has no `reply_to` and so never becomes a
notice.

**A prose view carries the same hints.** `content` of an `in_view` with
`kind: prose` may say `context`, `relevance`, `class`, `pinned` and
`relevant_until` beside `title` and `body`; they reach the wrapper the view is
rendered as and the curator scores with them. A prose view that names no
context stands in its owner's -- the sender's path with `/` spelt `~` -- so
the answer somebody just wrote weighs 1.0 and is visible, whatever else holds
the bar.

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

Five the screen is made of, and a catalogue of **twenty-six** an application may
name in its tree without defining them: four windows and twenty-two pieces of
content, the vocabulary the sheet is written against.

| component | layer | what it is |
|---|---|---|
| `display-shell` | `content` | the page root. `stylesheet` emits the link to the base sheet, `faces` carries the `@font-face` rules built from `params.font_base`, `vocab` a fingerprint of this list, `ground` the sheet's `day` or `night`; `focus`, `weights`, `judged_at`, `asked_at` and `due` are the screen state the compose cell keeps there |
| `display-region` | `content` | one per region, a direct child of the root, and the parent of every view standing in it |
| `display-view-prose` | `navigation` | a `display-pane` with an optional title and a paragraph -- a window, so it carries the five hints and the curator's `state`, `age`, `since`, `score`, `judged_relevance`, `judged_hidden` |
| `display-view-custom` | `content` | the wrapper an application's own tree hangs in |
| `display-mic` | `content` | the hold-to-talk button, its transcript line and its state line, plus the browser half that runs them |
| `display-pane`, `display-panel`, `display-overlay`, `display-ornament` | `navigation` | the four windows, and the only glass in the catalogue: a pane in the flow, a taller panel, an overlay above the page, an ornament that is a thing rather than a place. The first three carry the hints (`context`, `relevance`, `class`, `pinned`, `relevant_until`) and the curator's props; pane and panel also `tone` |
| `display-value`, `display-text`, `display-voice`, `display-kicker`, `display-list`, `display-item`, `display-table`, `display-weather`, `display-clock`, `display-timer`, `display-chat`, `display-chat-line`, `display-notification`, `display-media`, `display-document`, `display-status`, `display-action`, `display-choice`, `display-option`, `display-chart`, `display-stack`, `display-progress` | `content` | the twenty-two pieces of content a window holds, each with the props its `prop_schema` declares |

**The root carries a fingerprint of this list**, `vocab`, twelve hex characters
over the definitions as JSON. The definitions travel on the bootstrap pass, and
they travel again when a running screen's root holds another fingerprint: a
compose cell swapped for a newer version finds a page that is already its own
and defines the whole list again, once per change of vocabulary and never per
tick. The same economy an application's components have -- they travel when
they changed -- for the screen's own language.

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

## `params.font_base`

The sheet names two faces, Inter and Fraunces, at the head of two fallback
stacks, and declares neither. `font_base` is the directory the two files are
served from, relative to the page's own base, and the compose cell builds the
two `@font-face` rules from it. Empty, the shipped default, means no
`@font-face` at all: the fallback stacks carry the type and the page makes no
request. The file names are fixed, `inter.woff2` and `fraunces.woff2`, so what
an operator provides is one directory, named with or without its trailing
slash. No font file ships in this repository.

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

### What the button says, since 2.0.1

The line under the button is the only thing a person has to go on, so it speaks
at all three moments a press can end somewhere other than a turn.

- **While the browser is asking**, it says `asking for the microphone…`. The
  permission prompt opens inside the gesture, on a screen that has never been
  given the microphone before, and the press cannot become a hold until it is
  answered. A page that said nothing there looked like a page that does nothing.
- **When the answer came after the key went up**, it says `press again`. The
  microphone is open now and the gesture is over; the next press is a whole
  take, and nothing was recorded of the one that asked for it.
- **While it holds**, it says `listening…`, and the line goes back to naming the
  providers when the key comes up. A waiting sentence that stays on the screen
  after the wait is over is the same lie as a screen that says nothing.
- **While it joins**, it says `joining…`. A press waits for the join to be
  acknowledged before it holds: the frames of a hold are buffered until then,
  the raw audio pushes behind them are not, and audio that overtook its own
  `hold` would reach the cell with no boundary open.
- **When the call closed**, it says `closed <code>` and the button stays alive.
  A close ends a CALL, not the screen: the next press joins a new one. Only a
  REFUSED join is final — no mount under that name, or no room on the socket —
  because that answer is about this screen rather than about one call, and
  pressing again cannot change it.

A hold survives the button moving under the pointer: the pointer is captured on
press and released on `pointerup`, `pointercancel` or a lost capture, never on
`pointerleave`. On a fresh screen the line is empty until the join has answered,
and a line that appeared on press used to grow the block upward and slide the
button out from under the pointer (GH #684, since 2.2.1); the line has a height
before it speaks now, and a page that loses focus or goes hidden releases the
hold, so a key held while switching windows does not keep the microphone open.
And the button is a fixed point (GH #689, since 2.2.2): the transcript line and
the state line stand in a wrapper of their own that floats above the button,
out of its flow, so a line that grows -- a long transcript, say -- moves
nothing but itself.

What is not here: no transcript history, no list of turns, no way to scroll back.
The line beside the button holds the last thing that was heard and nothing more.
A conversation on the screen is a view like any other, put up by whoever owns it
on the `partial` lane of the agent it belongs to.

## `ttl_ms` expires a view, and the screen's own clock strikes when a view is due

A view is expired when `now_ms - updated_at >= ttl_ms`. An expired view is not
drawn; the row stays in the table and the screen stops showing it at the next
pass. Until 2.1.0 that next pass was the next time anybody wrote, and a screen
nobody wrote to kept showing an expired view. Since 2.2.0 **the screen's own
clock strikes when a view is due**, and the next pass takes it down -- over two
frames, like any window that leaves. So a `ttl_ms` is a promise about when
after all, exact to the second.

## A due clock: the compose cell predicts, the clock strikes once

Everything on the screen that changes by itself changes at a moment the compose
cell can compute: a fading score crosses the bar or the midpoint at
`since + linger_ms + fade_ms x (1 - target / (w x r))`, a `fresh` or `leaving`
window is over its frame one second later, a `relevant_until` runs out, a
`ttl_ms` runs out. After every pass `next_due()` takes the earliest of them and
**orders exactly one one-shot strike** for it from `clock`, a `timer` cell
beside the compose cell with no schedule of its own: `remove` the previous
order (its id stands on the root as `due`), `add` the next, named `due`, never
earlier than the next full second. A pass that computes the moment already
standing on the root orders the same second again without removing it first,
and the clock treats a repeated order as one order: an `add` it already holds
is acknowledged and changes nothing, an `add` on a removed row of the same id
revives it ([#690](https://github.com/mmeyerlein/meclaw/issues/690), since
2.2.3: before, such a pass removed and re-added one id, the timer marked the
row `removed`, the `add` collided with it, and the moment never struck). A
screen on which nothing can change orders nothing. The strike comes back on
the lane `in_tick` as a **pass without a write**: one `select` of the table,
then pass 2 and 3 as always; the order that struck is not asked to be
removed, it is gone already. That is not a poll
([#553](https://github.com/mmeyerlein/meclaw/issues/553)): every strike has a
name and a time, two strikes in a row carry two different reasons, and
the ambient clock that rewrites itself every twenty seconds is a write whose
identical props are no touch -- it goes through none of this. A `remove` on an
order that already struck is answered `schedule_not_found` by the timer; the
hive's edge stamps that `in_tick_error`, and the compose cell swallows it.
The order's id is derived from the moment it is due, so two passes that agree
on the moment agree on the order: when two passes run close together and both
read the root's `due` before the other wrote its own, the second `add` is the
same order, the clock acknowledges it as the one it already holds, and one
order results instead of two standing side by side
([#681](https://github.com/mmeyerlein/meclaw/issues/681); until #690 the
second answered `schedule_id_exists`).

## Wiring

A display is a **screen**, and a screen belongs to a person rather than to an
agent. So it stands beside the agents rather than inside one:

```
<member>/channels/display-<screen>
```

- **Agents write to it.** An assistant sends `in_view`, `in_withdraw` and
  `in_notice` at the hive path. Its `reply_to` **is** the owner, so two agents
  never collide under the same `view_id` and neither can withdraw the other's
  view.
- **A channel's failure reaches it as a notice.** The member's `channels`
  container draws a third edge for its screen -- `. -> ./<screen>` for
  `hop.route == 'error'`, re-stamped `in_notice` -- so a channel that failed is
  a system notice on the person's screen; the exit edge to the member stays,
  so the operator sees it too. Inside this hive the lane is one more edge from
  `.` to `./compose`, beside `in_view` and `in_withdraw`.
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
  keys in its `set_context`, the same line a channel's entry edge carries.
  Without it the identity is gone one hop past the screen. The whole edge, as
  `examples/organism/grow-screen.json` writes it:

  ```json
  {
    "from": "./display",
    "to": ".",
    "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')",
    "modifier": {
      "set_context": {
        "channel_node": "'display'",
        "channel": "'display'",
        "user_id": "has(hop.user_id) ? hop.user_id : ''"
      }
    }
  }
  ```
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

Inside it stand five cells since 2.2.0 -- `compose`, `views`, `web`, `clock`
and `judge` -- and the edges between them are the hive's own business. Beside
the four passes' edges, five more carry time and judgement: `compose -> clock`
on `hop.route == 'due'` (the order), `clock -> compose` on `schedule_name ==
'due'` re-stamped `in_tick` (the strike) and on `msg_type ==
'timer_op_error'` re-stamped `in_tick_error` (an order that already struck),
`compose -> judge` on `hop.route == 'judge'` (the question) and `judge ->
compose` on `has(hop.finish_reason)` re-stamped `in_verdict` (the answer, good
or failed -- the compose cell reads `finish_reason` itself). The two return
edges are unconditional on purpose: a timer's or a model's failure that
matched no edge would dead-letter as `no_route`, and a screen should never owe
a dead letter to its own clock.

| lane | direction | meaning |
|---|---|---|
| `in_view` | in | put this view up under this name, replacing whatever stood there |
| `in_withdraw` | in | take it down |
| `in_notice` | in | put a classified message up as a prose view of the sender, or translate a channel's `error_code` into one |
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
