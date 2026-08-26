# `web@1.1.0`

A display as one cell, with a port of its own. One `web` cell, one listener, one
`cell.db`, and a token stylesheet in the visionOS design language shipped as seed
data -- so a display looks like something before anybody has designed anything.

**The cell owns the listener, and that is the whole point.** Until W8 the one
surface belonged to the CLI: `--api` bound the one port, and everything
display-shaped competed for that one address. A colony could not open a second
display, and a display could not come into being by mutation with a port of its
own. This template is the other arrangement: instantiate it twice, give each
instance its own port, and you have two displays that share nothing but the
substrate underneath them.

## The cell

| path | type | what it holds |
|---|---|---|
| the template root itself | `web` | the object tree, the component library, the pages and the assets -- one `cell.db` |

Nothing sits below it. `./web` is the node, not a scope with a door: there is no
`hive_port_boundary` to trip over and no lane name to hit.

## What it serves, and where

The cell owns its whole origin. Four things answer on it, and the order matters
because the last one is a wildcard:

| path | what it is |
|---|---|
| `/live/websocket` | the LiveView transport. A plain GET here is a `400`, not a `404` -- the path is right, the request is not. |
| `/@client/<file>` | the two vendored Phoenix bundles, compiled into the binary. A closed list, so a file name out of a URL can never traverse anywhere. |
| `/` and `/<route>` | a page out of the **`pages` table**, rendered and kept. A route nothing declares is a `404`, never a blank page. |
| any other path | a file out of the **`assets` table** -- `/vision.css` is the one this template ships. The page map is asked first and the asset map second, so a page and a file can never shadow each other by accident. |

**A page load costs no cell call.** What is served is a snapshot the handler half
published: no database read, no message, no diff work. A colony that is wedged
therefore still serves its pages, and the client then visibly fails to *connect*
-- a state a person can read, instead of a blank screen.

**The `pages` table is the only route source.** There is no `cell.surface` key
any more: it was removed together with the `/surface/*` path it declared, and a
tree that still carries one no longer boots -- it is refused by name, key and
file (`config.md` § `cell`). Two grammars for one thing was the risk; one of the
two is gone rather than ignored. A route is a plain segment chain (`/`, `/a`,
`/a/b`, segments of `[a-z0-9-]`), with no `@` (those are the cell's own files)
and no `live` (that is the transport).

## Giving an instance its own port

`params.port` is **required and owned**: there is no default in the cell type,
because two instances sharing a default would be a bind race rather than a
configuration. The `7800` in this template is the first port, not the only one. A
second display takes its own. The template is one cell, so `override_params` takes
the flat form -- there is no path inside it to address:

```json
{"name": "web-two", "template": "web@1.1.0",
 "override_params": {"port": 7801}}
```

**RETRACTED: `port` and `bind` are immutable.** Up to `web@1.0.0` this page
said: *"Both stand in the params overlay's `KNOWN_KEYS` and in its
`IMMUTABLE_KEYS`. A params update that names either is refused as `Immutable`
-- loudly, and with no partial apply … `override_params` at **instantiation**
is therefore the one moment the port is chosen … A second display is a second
instance, never a rebind of the first."* **That refusal is withdrawn** (GH #410,
`web@1.1.0`). This type's `IMMUTABLE_KEYS` is empty.

The half that still holds is the last sentence: a second display is still a
second instance with its own port. What no longer holds is that a *first* one
cannot move. Moving a running display from loopback to a LAN bind used to mean
re-instantiating the cell and replaying every hand-made object position, because
a new instance is a new `cell.db`; it is now one message:

```json
{"params": {"bind": "0.0.0.0"}}
```

The listener closes, the new address is bound, every joined viewer is dropped
and reconnects on its own. The `cell.db` is untouched -- same objects, same
components, same pages, same files. A value the socket cannot take (a name
nothing resolves, a port somebody else holds) is refused to the sender as
`invalid_input` with the text `bind failed: …`, the display comes back on its
old address, and nothing is written: a respawn can never replay an address the
display was never on. What *did* bind is remembered, so a restart keeps the
move.

`params.external_timeout_ms` (default `5000`) is the ordinary A-timeout around
I/O the cell itself starts. It also bounds the wait for a rebind's verdict.

**The contract moved with the capability** (`contract.version` `1.0.0` →
`1.1.0`). `consumes.body.messages` was **required**, which would have refused a
params update at the door — `consumes_violation`, and the cell never called. It
is optional now, and `params` is declared beside it. Nothing is lost: a
declarative type check cannot tell a display patch from a params update, so the
refusal moved to the only side that can. A body carrying neither slot comes back
`invalid_input` from the cell itself.

## Authentication is external, forever

This cell type does not authenticate, and it is not going to (R-W8-2). Put a
reverse proxy in front of it -- nginx, traefik, caddy -- and let that terminate
TLS and decide who gets through. Everything about this template follows from
that one decision:

- **The default bind is `127.0.0.1`.** A type that never authenticates must not
  be reachable off-host by default. Setting `bind` to `0.0.0.0` is a decision
  somebody makes on purpose. Since `web@1.1.0` it can be made in a params update
  on a running cell as well as in the mutation that creates it -- and taken back
  the same way, which it could not be before.
- **There is no allowlist, no rate limit and no session of its own.** Whoever
  reaches the port sees the display and can move whatever a component declared
  `editable`.
- **The `editable` declaration is the authorization.** A browser may write the
  props a component named and nothing else; anything else comes back
  `not_editable` with no write.

## The two classes of browser event

Which class an event belongs to is decided by the **component's declaration**,
never by the event's name.

- **Local.** An `object:set {id, prop, value}` on a prop the component declared
  `editable` is executed by the cell itself as CRUD on its own database, followed
  by one diff to every viewer of that page, the sender included. **Zero topology
  round trip**: no message is created. A drag on a node must not be a
  conversation with the router.
- **Semantic.** Everything else -- a button, a form, later a microphone frame --
  leaves as an ordinary **source emission** on `hop.route = "event"`, exactly as
  the `proxy` cell emits an inbound platform turn. The header carries
  `event_name`, `session_id` and `page_route`.

This template declares `contract.ingress.context: ["session_id"]`: the cell states
that messages are born at it carrying the page load's own id. **Lifting it into
`context.session_id` is the entry edge's job** (`set_context`), not the cell's --
a cell says what it knows, an edge decides what that means for the graph. That is
the proxy precedent, and it is what keeps this cell ignorant of the topology it
hangs in. A display whose events nobody listens for dead-letters visibly rather
than disappearing quietly.

## The Vision token sheet

`seed/assets.jsonl` ships one asset row, `/vision.css`: the design system as
data. No build step, no library, no import -- every rule spends a custom property,
so a component template only ever writes class names, and a model defining a
component at runtime has a vocabulary it can reach without inventing colours.

What is in it: the glass material (`backdrop-filter` blur, saturation and a
brightness clamp) with an opaque `@supports` fallback where there is no backdrop
filter at all; three vibrancy tiers of one white foreground; concentric radii,
where the inner radius is **derived** (`--r-inner: calc(var(--r-window) -
var(--r-pad))`) rather than re-typed, because a card inset inside a window has to
curve tighter by exactly its padding or the two curves visibly disagree; an
asymmetric rim light, because the light is above; a grain field that exists only
to break the banding a large blurred gradient shows on 8-bit displays; a two-part
spatial shadow (contact plus cast -- one shadow can be either, and a floating
pane needs both); a scroll-edge mask; and a type scale with the visionOS weight
bump, body at medium and titles at bold, because type over a translucent moving
backdrop needs the extra stroke.

Three media blocks switch the material off on purpose:
`prefers-reduced-transparency` and `prefers-contrast` replace it with the opaque
fill, and `forced-colors` hands every colour back to the operating system.

**The stylesheet link rides on `stack`, and nowhere else.** The cell's shell
links no stylesheet at all -- it writes a `<title>`, the container div and the
two client bundles, and that is deliberate: a shell that linked a file would be
the cell type deciding what a display looks like. So the link is a *component's*
output. The root object carries `stylesheet: true`, and `stack` emits
`<link rel="stylesheet" href="/vision.css">` when that prop is set -- once, on
the page root, and not again inside every nested stack. A page whose root
forgets the prop renders unstyled, which is a thing you can see and fix.

The leading slash is load-bearing: an asset answers on the path its row names,
and there is no path normalisation anywhere in this cell. `/vision.css` is the
file; `vision.css` would be a link to nothing.

## The nine components

`seed/components.jsonl` ships the Vision set. Nine rows, no more -- a set small
enough to hold in your head and complete enough to build a page out of:

| component | layer | what it is |
|---|---|---|
| `stack` | content | the page root and every group inside it. `stylesheet` emits the link; `row` lays its children out sideways. |
| `card` | navigation | a glass pane with an optional title. Its children sit in `.inner`, on the pane's own fill. |
| `heading` | content | a section title. `lead` makes it the one title a page leads with. |
| `text` | content | a paragraph. `secondary` drops it to the second vibrancy tier. |
| `table` | content | rows and an optional head, both declared `"html"` -- the one place in the set where markup is passed through rather than escaped. |
| `button` | content | a control. With `event` set it emits that name as a **semantic** browser event. |
| `input` | content | a field whose `value` is `editable`. With `id` set to its own object id it writes that value back on blur, on the **local** lane. |
| `badge` | content | a caption in a pill. |
| `ornament` | navigation | the floating dock: glass, fixed to the bottom edge. |

### Two rules the cell enforces, rather than documents

Both come from the design language this borrows from, and both are refusals
(`invalid_input`) rather than advice:

1. **Glass is a navigation-layer material.** A component that declares
   `layer: "content"` and writes one of the three closed class names `glass`,
   `glass--thin` or `glass--thick` is refused -- glass lives on the navigation
   layer only. That is why `card` and `ornament` are the two navigation members
   of the set: they are the two that *are* glass.
2. **Glass never sits on glass.** `object.create` refuses a child whose
   component is navigation glass under a parent whose component is too. The
   check is on the edge the create makes: put a content component between two
   panes, and they nest.

The first rule is checked at `component.define` **and** at seed time -- a seeded
component goes through the same check, because a rule that only guarded the
message path would be a rule every shipped template walks past. The second is
checked where the edge is made, and makes no claim about an `object.move` that
reparents a pane afterwards.

The rules are about the class vocabulary, not about CSS in general: a component
that reaches for `backdrop-filter` in an inline `style` is outside what they can
see, and pretending otherwise would be worse than saying so.

## What ships in the seed

| file | rows |
|---|---|
| `seed/components.jsonl` | the nine Vision components |
| `seed/objects.jsonl` | `root` with one `text` child, and the `/demo` tree |
| `seed/pages.jsonl` | a page at `/`, titled *Vision*, and the demo page |
| `seed/assets.jsonl` | `/vision.css` |

**`/demo` is the set looking at itself.** Every one of the nine appears on it and
nothing else does -- the demo page at `/demo` is composed of nothing but the
nine, which is what makes it a usable smoke target: if a component is broken,
the page shows it.

**Both shipped pages give their root exactly one child.** A materialised page is
statics *around* slots, and a page root's own template contributes two statics --
what stands before `{{children}}` and what stands after. The served body and the
packed tree interleave the two lists one for one, so a root with several direct
children would put the closing static in the middle of the page. So the root
stack carries the stylesheet link and one child, and that child holds the
content. It is also the cheaper diff: every write to this page re-renders one
slot.

**The seed loads once**, on first spawn only (`OpenStatus::Created`). A display
that re-seeded on every wake would resurrect objects an operator had deleted. The
file names are a closed set -- a `seed/widgets.jsonl` is a **typo that gets
reported**, not a table that comes into being -- and each header is checked
against the real columns, so a seed written for an older schema fails loudly
instead of writing into columns that moved. Both checks run in the plan phase
(`--validate`), not at the first boot.

## Components are data

A component is a row: a name, a template body, a prop schema, an `editable`
declaration and a layer. A model can define one at runtime with
`component.define`, and the template is parsed **at definition time** with the
same parser the renderer uses -- so an unknown `{{…}}` is answered to whoever
wrote it, at the moment they write it.

The template language is closed, and there is no fifth form:

| form | meaning |
|---|---|
| `{{prop}}` | the prop's value, HTML-escaped |
| `{{&prop}}` | the value raw -- honoured only where `prop_schema` types the prop as `"html"` |
| `{{children}}` | the object's children, in `ord` order |
| `{{#if prop}}…{{/if}}` | the enclosed text, if the prop is present, non-empty and not `false` |

### Which brain to point at it

**`openai/gpt-oss-20b` on Groq**, if you want one recommendation. It was the
only model in the W8 bracket that answered **both** wire formats validly in
every repeat while still reaching a correct rendered picture in under a second
(0.74 s), and it is the cheapest of the models that managed that. For JSON ops
specifically, `openai/gpt-oss-120b` on Cerebras is the faster pick (0.80 s).

Two things that measurement is worth saying out loud, because neither is
guessable: the **host matters as much as the model** — the same
`gpt-oss-120b` reaches a picture in 0.80 s on Cerebras and 1.85 s on SambaNova,
so pin the provider — and the fastest first token is not the fastest picture. A
model that streams sooner but emits a second JSON document after the first one
produces no picture at all.

The measurement, the full ranking and its limits are in
`plans/wellen-2026-08-21/receipts/w8-brain-ranking.md` (GH #384).

## What it is not

- **Not a web server you configure.** There is no static directory, no rewrite
  rule and no vhost. What it serves is what is in its four tables.
- **Not an application.** It renders what it was sent and reports what a person
  did; what either means belongs to the topology around it.
- **Not a shared surface.** One instance, one port, one display. Two displays are
  two instances -- that is what the type being deliberately *multiple* is for.
- **Not a place for secrets.** Anything in the object tree is on a page, and the
  page is served to whoever reaches the port.
