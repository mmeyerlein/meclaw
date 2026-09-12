# `web@2.0.1`

A display as one cell, with a name of its own. One `web` cell, one `cell.db`,
one mount on the colony's listener, and a token stylesheet in the visionOS
design language shipped as seed data -- so a display looks like something before
anybody has designed anything.

**The cell owns its mount, and that is the whole point.** Until W8 the one
surface belonged to the CLI: `--api` bound the one port, and everything
display-shaped competed for that one address. A colony could not open a second
display, and a display could not come into being by mutation. This template is
the other arrangement: instantiate it twice, give each instance its own name,
and you have two displays that share nothing but the substrate underneath them.

**And since `web@2.0.0` they share the listener too.** A cell type gets a port
only when there is no other way, and a display has another way: the colony's one
listener peeks the first path segment of every connection it accepts and hands
the stream, unread, to whoever registered that name. So `params.port` and
`params.bind` are gone, `params.mount` is required, and one reverse-proxy rule
in front of one listener covers a colony's whole surface.

## Migrating from `web@1.1.0`

Drop `port`, drop `bind`, add `mount`. A params document that still carries
either key is refused at parse, by name:

```text
port: removed in web 2.0.0 — the cell is reached at /<mount>/ on the colony's
listener; drop the key and name a mount
```

The page moves with it: `http://host:7800/` becomes `http://<listener>/<mount>/`,
where `<listener>` is the address the colony's `--api` bound. Everything else --
the `cell.db`, the object tree, the pages, the seed, the contract -- is
unchanged.

## The cell

| path | type | what it holds |
|---|---|---|
| the template root itself | `web` | the object tree, the component library, the pages and the assets -- one `cell.db` |

Nothing sits below it. `./web` is the node, not a scope with a door: there is no
`hive_port_boundary` to trip over and no lane name to hit.

## What it serves, and where

The cell owns everything under `/<mount>/`. Four things answer there, and the
order matters because the last one is a wildcard:

| path | what it is |
|---|---|
| `/<mount>/live/websocket` | the LiveView transport. A plain GET here is a `400`, not a `404` -- the path is right, the request is not. |
| `/<mount>/@client/<file>` | the two vendored Phoenix bundles, compiled into the binary. A closed list, so a file name out of a URL can never traverse anywhere. |
| `/<mount>/` and `/<mount>/<route>` | a page out of the **`pages` table**, rendered and kept. A route nothing declares is a `404`, never a blank page. |
| any other path under the mount | a file out of the **`assets` table** -- `vision.css` is the one this template ships. The page map is asked first and the asset map second, so a page and a file can never shadow each other by accident. |

**Behind a proxy the whole set moves together.** The shell reads
`X-Forwarded-Prefix`, checks it against `^/[A-Za-z0-9._~/-]{0,200}$` without a
trailing slash, ignores it if it is anything else, and writes every URL it emits
-- the socket, the two bundles and a `<base href>` -- from that prefix plus the
mount. So one nginx block

```nginx
location ^~ /alpha/ {
    proxy_pass http://127.0.0.1:7777/;
    proxy_set_header X-Forwarded-Prefix /alpha;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

serves the display at `https://host/alpha/<mount>/`, and the page a browser
gets knows it. **The proxy must strip the prefix it announces**: the header
says what was taken off the path, and the listener routes by the first segment
of what arrives, so `/alpha/<mount>/` forwarded whole reaches no mount. In
nginx the trailing slash on `proxy_pass` is what does the stripping. And the
three upgrade lines carry the LiveView socket; without them the page renders
and never connects, because what reaches `/<mount>/live/websocket` is then a
plain GET. **A page's own links should be relative** for the same reason the shipped
stylesheet link is: a page is materialised before any request, so nothing in it
can know the prefix, and `<base>` is what makes a relative URL resolve under the
mount from any route depth.

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

## Giving an instance its own mount

`params.mount` is **required and owned**: there is no default in the cell type,
because two instances sharing a default would be a mount collision rather than a
configuration. The `web` in this template is the first name, not the only one. A
second display takes its own. The template is one cell, so `override_params`
takes the flat form -- there is no path inside it to address:

```json
{"name": "web-two", "template": "web@2.0.1",
 "override_params": {"mount": "screen"}}
```

The grammar is the substrate's (`[a-z0-9-]{1,64}`, and none of the names the API
already answers: `colony`, `messages`, `health`, `ui`, `live`, `@client`). A
name another cell already holds is reported and the display serves nobody --
loudly, and without taking its cell down, because a collision is an operator's
mistake to read rather than a crash loop.

**A rename takes effect on the next life.** No param of this type is immutable,
so a params update may name a new mount:

```json
{"params": {"mount": "screen"}}
```

It is written to the overlay and read when the cell next starts. The name is
registered once per life, and remounting a running display would move it out
from under whichever proxy rule points at it while its viewers still hold
sockets on the old name. The `cell.db` is untouched either way -- same objects,
same components, same pages, same files.

`params.identity_header` (default `""`) names the request header a proxy in
front puts the viewer's identity in. With it set, the value on a socket's
upgrade request rides as `hop.user_id` on every semantic event of that
connection. Empty by default, and deliberately: a header a client can set
without a proxy is not an identity. A `hop` is single-hop, so the entry edge out
of this cell owes the stamp a promotion into the context --
`"user_id": "has(hop.user_id) ? hop.user_id : ''"` in its `set_context`, the
same line a channel's entry edge carries -- or the identity ends at the first
edge.

`params.external_timeout_ms` (default `5000`) is the ordinary A-timeout around
I/O the cell itself starts.

**The contract moved with the capability** (`contract.version` `1.0.0` →
`1.1.0`, and `2.0.0` with the removal above). `consumes.body.messages` was **required**, which would have refused a
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

- **The cell binds nothing.** Since `web@2.0.0` there is no address to get
  wrong: the colony's one listener is the only socket, and where that listener
  binds is the operator's decision, made once for the whole colony.
- **There is no allowlist, no rate limit and no session of its own.** Whoever
  reaches the mount sees the display and can move whatever a component declared
  `editable`. Separating one viewer from another -- auth, cookies, storage --
  is the proxy's job; `identity_header` is how the cell learns what the proxy
  decided, and nothing more.
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
  `event_name`, `session_id`, `page_route` and, when `identity_header` names a
  header the proxy actually sent, `user_id`.

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
links no stylesheet at all -- it writes a `<title>`, the container div, a
`<base>` and the two client bundles, and that is deliberate: a shell that linked
a file would be the cell type deciding what a display looks like. So the link is
a *component's* output. The root object carries `stylesheet: true`, and `stack`
emits `<link rel="stylesheet" href="vision.css">` when that prop is set -- once,
on the page root, and not again inside every nested stack. A page whose root
forgets the prop renders unstyled, which is a thing you can see and fix.

The MISSING leading slash is load-bearing since `web@2.0.0`. The asset row is
still `/vision.css` -- an asset answers on the path its row names, and there is
no path normalisation anywhere in this cell -- but the page is served under a
mount, and possibly under a proxy prefix as well, neither of which a
materialised page can know. So the link is relative and the shell's
`<base href="<prefix>/<mount>/">` resolves it, from `/<mount>/` and from
`/<mount>/a/b` alike. A link written `/vision.css` would leave the mount and ask
the listener for a surface called `vision.css`.

**The one exception, and why it is not one.** The shell's `<head>` carries a
handful of inline CSS lines for the LiveView *connection states* --
`phx-loading`, `phx-error`, `phx-client-error`, `phx-server-error`, the classes
the vendored client writes on the `data-phx-main` container the shell itself
emits. They turn a dead socket into a fixed banner, *connection lost -- this
page may be out of date*. That is not the cell type deciding what a display
looks like: it is the runtime this shell ships being visible in its own states,
on the shell's own element. A runtime that publishes a state nothing can see is
half delivered -- and before this, only `templates/colony-view` styled those
classes, so every other page of every display drew a frozen picture with nothing
on screen to say it had stopped moving.

A page overrides the default by declaring the same rules: the block is in
`<head>`, a page's own `<style>` arrives in the body, and at equal specificity
the later rule wins. There is never a second banner, because `::after` is one
box per element. The default deliberately does **not** dim the page: `opacity`
on the container would fade the banner too, and `opacity` on its children
multiplies with whatever the page already dims further down. Full contract:
`docs/cell-types.en.md` § `web`.

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

One more rule of the same kind, and this one the cell does not check either. A
view's `client_css` goes into the page raw. It describes the view.
`html`, `body`, `:root` and a document-level `@media (prefers-color-scheme)`
belong to the screen that holds the page; a view that writes one of them paints
over every other view standing beside it. Reading a stylesheet for that would be
a second language in the substrate, so the rule is named here and pinned on the
shipped sheet instead (GH #671).

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

The measurement, the full ranking and its limits are recorded in GH #384.

## What it is not

- **Not a web server you configure.** There is no static directory, no rewrite
  rule and no vhost. What it serves is what is in its four tables.
- **Not an application.** It renders what it was sent and reports what a person
  did; what either means belongs to the topology around it.
- **Not a shared surface.** One instance, one mount, one display. Two displays
  are two instances -- that is what the type being deliberately *multiple* is
  for.
- **Not a place for secrets.** Anything in the object tree is on a page, and the
  page is served to whoever reaches the mount.
