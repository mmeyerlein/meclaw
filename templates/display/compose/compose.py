"""The compose cell: a view in, one screen out.

THIS FILE IS THE SOURCE. `config.json` carries a byte-identical copy of it in
`params.script_inline`. Edit here, then regenerate the copy:

    python3 -c 'import json,io; p="templates/display/compose/config.json"; \
d=json.load(io.open(p)); \
d["params"]["script_inline"]=io.open("templates/display/compose/compose.py").read(); \
io.open(p,"w").write(json.dumps(d,indent=2,ensure_ascii=False)+"\n")'

Why the copy exists at all: a `code` cell's `script_path` is handed to the
interpreter verbatim with no working directory of its own, so a relative path
would resolve against the daemon's cwd and an absolute path baked into a
template is the exported-tree defect class of GH #20. So the runtime form is
`script_inline`, and this file is what a person reads, greps, diffs and runs:

    python3 templates/display/compose/compose.py < some-stdin-doc.json

# What this cell is

It is the whole of the screen's bookkeeping, and it draws nothing itself. An
agent or an app says "here is a view of mine, put it up"; this cell decides
what the display's object tree must therefore look like, and says so as
`object.*` calls. It holds no model, opens no socket and makes no layout
judgement beyond two regions and an order: a declared `ord`, then first
appearance, and never the moment a view was last written.

It is NOT the owner of a view's content. The content is whatever the sender
sent, rendered by whatever component the sender defined. This cell only ever
wraps it, places it, and takes it away again.

It is NOT the owner of identity either. The owner of a view is
`envelope.reply_to` -- the path of the cell that emitted the message -- and
never a field in the body. A body may repeat it, and a body that repeats it
WRONG is refused rather than believed: that is the whole of `not_owner`.

# The four passes, and why they are cut here

The discriminator is the ENVELOPE HEADER, never the shape of the body. A body
is written by whoever sent it; a header is written by the edge that carried it,
and the edges of this hive are the only thing that knows where a message has
already been. Guessing a pass from the body is how a reply gets mistaken for a
request, which is how a loop starts.

Pass 1 (`hop.route` is `in_view`, `in_withdraw` or `event`): a REQUEST. The two
write lanes are validated and turned into ONE store bundle -- a `select` of the
whole table, then a `delete` of this owner's row for this `view_id`, then (on
`in_view` only) an `insert` of the new one. The select comes FIRST on purpose:
it is the before-state, and it is the only chance to see it, because the delete
is about to remove the row it would have described. The request itself rides
along as a JSON string on `hop.display_request`, which the hive's own edge
promotes into `context` -- `hop` survives exactly one edge and pass 2 is one
edge further on than that.

Delete-then-insert IS the primary key. A `store` schema declaration carries
column types and nothing else -- no PRIMARY KEY, no UNIQUE, no index -- so
`(owner, view_id)` is an identity this cell keeps by hand, in one bundle, in
that order.

The third lane is a browser event the `web` cell could not absorb locally. The
object id it carries is parsed back into the `(owner, view_id)` that produced
it, and the event leaves the hive with both attached, so a member can route it
to the one agent that put the view up. If the id does not parse, the event goes
out ANYWAY without them: a dead letter somebody can read beats a silent drop.

Pass 2 (`context.display_origin == 'views'`): the store answered. The
after-state is computed IN MEMORY from the before-state -- minus the row that
was deleted, plus the row that was inserted -- because a second select would be
a second round trip for a set this cell already knows. Expired views are
dropped from the picture here and the rest is put in a deterministic order:
region, then the `ord` the view declared, then identity. The order a person
SEES is settled one pass later, in `build`, because it needs the seats the
display is already holding -- see `seated`.

Pass 3 (`context.display_origin == 'read'`): the display answered the query.
The question that answer settles is "is this page MINE", and there are two ways
it is not: no page at `/` at all (`query` is refused), or a page whose root is
somebody else's. BOTH are the bootstrap case. Reading only the refusal is the
GH #402 defect: `display/web` refs the `web` template, which SEEDS a demo page
at `/`, so the query succeeds, the vocabulary is never defined, and every
`object.create` comes back `unknown_component` while the deletes land. The
bootstrap adopts the page and DELETES NOTHING -- those objects are not this
cell's to remove.

An `object.update` whose props the display already holds is not sent at all
(GH #412). The `web` cell applies a bundle through its single database actor,
and a browser's own `object:set` is served by that same actor: a full rewrite
of an unchanged tree holds it for the length of the rewrite, and anything a
person did in that window is written late while the rewrite's diffs re-render
it where it was.

Pass 4 (`context.display_origin == 'patch'`): emit NOTHING. This pass is the
whole reason the cell needs a discriminator. Without it every acknowledgement
falls through to "ask again" -- one request becomes two, two become four, and
the routing loop wedges on a full mailbox inside twenty seconds (GH #161).
"""
import hashlib
import json
import sys
import time

# The table this cell keeps, and the projection it reads back. Both are the
# store's own vocabulary; the column list is written once so a select can never
# disagree with an insert about what a row is.
TABLE = "views"
COLUMNS = [
    "owner",
    "view_id",
    "region",
    "ord",
    "kind",
    "content",
    "components",
    "ttl_ms",
    "updated_at",
]

# The regions a screen has, in the order they stand on the page, and a closed
# list rather than a free string: an unknown region is a view nobody would ever
# see, which is worse than a refusal the sender can read.
#
# `main` is the wide column and the DEFAULT, so every view that named no region
# before this version lands exactly where it landed then. `aside` is the narrow
# column beside it, and it is what the second region is FOR: a clock, a weather
# tile, a countdown -- things that are true whether or not anybody is talking,
# and that have no business pushing a conversation down the page (GH #609).
REGIONS = ("main", "aside")
REGION_INDEX = dict((name, i) for i, name in enumerate(REGIONS))

# The object ids, by class. Deterministic and prefixed: an id has to be
# derivable from the row without a side table, and it has to say which class it
# belongs to, because deletion sweeps by prefix.
ROOT_ID = "display.root"
REGION_PREFIX = "display.region."
MIC_ID = "display.mic"
VIEW_PREFIX = "view."
PAGE_ROUTE = "/"
PAGE_TITLE = "display"

# `view_id` is `[a-z0-9-]{1,64}`. Spelled as a character set rather than a
# regex so this script needs nothing but the three standard modules.
ID_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-")
ID_MAX = 64
# A node key is an identity, not a name a person types: a cell path with its
# slashes written as tildes is already 120 characters deep in a real colony.
KEY_MAX = 512

# `ord` is a sort key, not a list index: gaps leave room to insert without
# renumbering anything the display already holds.
ORD_STEP = 10

# Where a view sits in its region before anything has ever placed it: behind
# everything the screen already holds. A view's SEAT is the `ord` the display
# is holding it at right now, which is how "first appearance" is remembered
# without a column for it -- the screen remembers the order of the screen.
NEW_SEAT = 1 << 40

# How deep a component tree may be. The `web` cell stops rendering at 64 levels
# and reports the object it stopped at; refusing earlier, at the door, turns
# that into an answer to whoever wrote the tree.
MAX_DEPTH = 32

# The closed error surface of this hive. Every refusal leaves on the `receipt`
# lane carrying exactly one of these.
ERRORS = (
    "not_owner",
    "owner_unknown",
    "invalid_view",
    "component_prefix",
    "store_failed",
)

# ---------------------------------------------------------------------------
# The display's own vocabulary
#
# Four components, defined by message on the bootstrap pass and stored as rows.
# The layer of each one is a decision the `web` cell ENFORCES rather than
# documents: glass is a navigation-layer material there, and a content
# component that writes `glass--thin` is refused at definition time.

# The two columns, as the display's OWN rule rather than a line in a token
# sheet: `vision.css` belongs to the `web` template and describes a design
# language, and so does the sheet below -- while "main is wide and aside is
# narrow" is a statement about THIS screen. The rules travel in the shell, in
# the one `<style>` block the shell renders, ahead of the sheet. Written with
# no two `{` adjacent, because `{{` is the component language's own marker.
LAYOUT_RULES = (
    ".display-columns { display: flex; flex-direction: row;"
    " align-items: flex-start; gap: var(--gap, 16px); }"
    ' .display-columns > [data-region="main"] { flex: 1 1 0; min-width: 0; }'
    ' .display-columns > [data-region="aside"]'
    " { flex: 0 0 clamp(15rem, 22%, 24rem); min-width: 0; }"
    # An empty aside is not a narrow empty column: every screen that has never
    # heard of a region would otherwise lose a fifth of its width to nothing.
    ' .display-columns > [data-region="aside"]:empty { display: none; }'
    " @media (max-width: 60rem)"
    " { .display-columns { flex-direction: column; }"
    ' .display-columns > [data-region="aside"] { flex: 1 1 auto; } }'
    # The microphone sits out of the columns, bottom right, because it belongs to
    # the SCREEN rather than to anything standing on it: a person looks for the
    # button in the same place whatever is up. Fixed rather than absolute, so it
    # stays put on a long page. Only the PLACEMENT is said here; what the button
    # looks like is the sheet's business (its "furniture" section).
    " .display-mic { position: fixed; right: 16px; bottom: 16px; z-index: 8;"
    " display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }"
)

# The design language of the screen, byte-identical to `display-dna.css`
# beside this file. THAT FILE IS THE SOURCE; this constant is the copy a
# running cell renders, and a drift lock compares the two with the copy of this
# script inside `config.json`. A constant and not a file read, because a `code`
# cell runs out of `script_inline` and has no working directory to read a
# sheet from. The sheet is a raw string with no `"""` in it, and it is never
# minified: two `{` or two `}` side by side would be the component language's
# marker, and the lock refuses both.
KIT_CSS = r"""/* The display's design language -- meclaw display DNA v1 (kit 6).
 *
 * THIS FILE IS THE SOURCE. `compose/compose.py` carries a byte-identical copy
 * in its `KIT_CSS` constant and `compose/config.json` carries that file again
 * in `params.script_inline`; a drift lock compares all three
 * (`the_design_language_is_the_same_bytes_in_three_places`). The compose cell
 * writes the constant into the `display-shell` template, last in the shell's
 * one style block, so a screen needs nothing installed beside it and no hand
 * ever sends a sheet to a running tree.
 *
 * It belongs to the TEMPLATE, not to the binary: a screen that should look
 * different costs a template edit, not a release.
 *
 * What it is: warm glass on a creme ground, one light, one accent, three
 * alphas of one ink, windows carried by a specular edge and two shadows and
 * nothing else. Every class is `display-<name>`; the three glass classes and
 * `.inner` are borrowed from `/vision.css` and keep their names. Loads AFTER
 * `/vision.css` and re-tokenises it to light: the same custom property names,
 * the DNA's values. The nine base components of the `web` template keep
 * working.
 *
 * Order: tokens, ground, material, type, windows (A), content (B), states,
 * the screen, the screen's own furniture, the scene, night, fallbacks,
 * motion, screen sizes.
 *
 * Rules the sheet obeys: glass only on the navigation layer, never glass on
 * glass, text never on the material (always on `.inner`). Templates write
 * classes and data attributes, never colours. No external request and no
 * face: the two faces are an operator asset, declared from `params.font_base`
 * or not at all, and the fallback stacks in `--font-ui` and `--font-voice`
 * carry the type until then. Never minified: two `{` or two `}` side by side
 * are the component language's own marker.
 */

/* ── 1. Tokens ───────────────────────────────────────────────────────────
 * The names are /vision.css's, the values the DNA's. Overriding :root is the
 * whole light mode — no new vocabulary, so a component defined at runtime
 * still only knows the words the template shipped with. */
:root {
  color-scheme: light;

  /* Warm glass: the tint is creme, not neutral white, which over a creme
   * ground would read as fog. Blur stays in the web corridor (8–20px);
   * backdrop-filter is expensive and three glass surfaces per screen is the
   * budget. */
  --glass-tint: rgba(255, 251, 246, 0.62);
  --glass-tint-thin: rgba(255, 251, 246, 0.42);
  --glass-tint-thick: rgba(255, 251, 246, 0.78);
  --glass-blur: 18px;
  --glass-saturate: 140%;
  --glass-brightness: 1.02;
  --glass-focus: rgba(255, 251, 246, 0.8);
  --glass-opaque: #fffbf6;

  /* `.inner` is the substrate's rule that text never sits on the material —
   * a rule about classes, not a demand for a second surface. Without a fill
   * and without an outline it stops being the inner frame it was in kit 1.
   * --inner-fill-strong stays for the two things that really are a surface
   * of their own: a notification and a media frame. */
  --inner-fill: transparent;
  --inner-fill-strong: rgba(255, 255, 255, 0.55);

  /* Vibrancy: three alphas of ONE ink, not three greys, so the foreground
   * still picks up what the glass lets through. Secondary is .82, not the
   * dark sheet's .66. Measured off a render, on the brightest point of the
   * ground: .66 gives 5.0:1, .78 gives 6.5:1, .82 gives 7.2:1 — and the
   * contract wants >= 7:1 for body copy. On a plain .inner it measures 7.9:1.
   * Tertiary is the label tier (kicker, footnote) and never body copy; it
   * measures ~3.6:1, which is the DNA's own trade. */
  --fg-primary: rgba(43, 29, 25, 0.96);
  --fg-secondary: rgba(43, 29, 25, 0.82);
  --fg-tertiary: rgba(43, 29, 25, 0.55);
  /* No hairlines. Rows are separated by the space they already have, a table
   * head by a band. Both names stay, and every `border: 1px solid` below
   * stays with them — a transparent border still occupies its pixel, so
   * removing the line moves nothing. */
  --hairline: transparent;
  --hairline-strong: transparent;

  /* One accent, two jobs: vermilion is a text/marker colour (only bold, only
   * large), coral is a surface colour. Never two accents in one view. */
  --accent: #b93a25;
  --accent-soft: #e8664f;
  --accent-wash: rgba(232, 102, 79, 0.14);
  /* Rung 2 of the ladder: the 1px coral glow that rides in --shadow-2. It is
   * the faintest step, so it carries a little more than kit 1 gave it. */
  --accent-ring: rgba(232, 102, 79, 0.5);

  /* Concentric geometry. The inner radius is derived, never re-typed:
   * 22 − 8 = 14. visionOS windows measure ~32px; 22 is the DNA's correction
   * ("a touch too round") and still reads as a window, not a pebble. */
  --r-window: 22px;
  --r-pad: 8px;
  --r-inner: calc(var(--r-window) - var(--r-pad));
  --r-control: 12px;
  --r-chip: 14px;
  --r-capsule: 999px;
  --pad-window: 22px;
  --pad-inner: 16px;
  --gap: 16px;
  --gap-s: 8px;
  --gap-l: 24px;

  /* ONE edge. The specular top is the light falling on the near lip of the
   * glass and it is the only line a window gets. The other two rim tokens
   * keep their names and are emptied, so no rule below can grow a second
   * border by accident. */
  --rim-top: inset 0 1.5px 0 rgba(255, 255, 255, 1);
  --rim-edge: 0 0 transparent;
  --rim-bottom: 0 0 transparent;
  /* The sheen stands in for refraction, which no browser can do: a fixed
   * 135deg highlight, always top-left. Convention, not physics. */
  --sheen: linear-gradient(135deg, rgba(255, 255, 255, 0.55) 0%,
    rgba(255, 255, 255, 0.16) 26%, rgba(255, 255, 255, 0) 56%);

  /* Shadow channels, Open-Props style: hue + strength instead of four
   * hand-mixed rgba chains. A shadow on creme must not be grey, and the night
   * switch is then two lines. */
  --shadow-hue: 18 31% 23%;
  --shadow-strength: 10%;
  --shadow-contact: 0 1px 2px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.4)),
    0 3px 8px -4px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.8));
  --shadow-cast: 0 26px 52px -20px
    hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 4));
  --shadow-spatial: var(--shadow-contact), var(--shadow-cast);
  --shadow-1: var(--shadow-spatial);
  --shadow-2: 0 2px 4px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.6)),
    0 44px 84px -26px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 5.5)),
    0 0 0 1px var(--accent-ring);

  /* Grain: anti-banding for a large blurred gradient, not a texture. */
  --grain-opacity: 0.045;
  --grain: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='120' height='120'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='2'/></filter><rect width='120' height='120' filter='url(%23n)'/></svg>");

  /* The visionOS weight bump: body medium, titles semibold to bold — type
   * over a translucent backdrop needs the stroke. Line height is a function
   * of size (1.5 down to 1.1), never one global value. */
  --font-ui: Inter, "SF Pro Text", "SF Pro", system-ui, -apple-system,
    "Segoe UI", Roboto, sans-serif;
  --font-voice: Fraunces, "Iowan Old Style", Georgia, serif;
  --w-body: 500;
  --w-title: 600;
  --w-strong: 700;
  --t-caption: 12px;
  --t-small: 14px;
  --t-body: 16px;
  --t-title-3: 20px;
  --t-title-2: 24px;
  --t-title-1: 34px;
  --t-value: 34px;
  --t-voice: 30px;
  --leading-body: 1.5;
  --leading-title: 1.18;

  /* cubic-bezier(.32,.72,0,1) is Apple's .default spring (response 0.55,
   * damping 1.0) as a curve: it settles without overshoot. */
  --ease: cubic-bezier(0.32, 0.72, 0, 1);
  --t-hover: 150ms;
  --t-leave: 240ms;
  --t-enter: 360ms;
  --t-focus: 420ms;

  /* The ground, plus the one number visionOS publishes: an ornament overlaps
   * the window's bottom edge by 20pt. */
  --ground: #f7efe6;
  --ground-2: #efe2d4;
  --bg-void: #f7efe6;
  --ornament-overlap: 20px;
}

/* ── 2. Ground: creme with one light ─────────────────────────────────────
 * One light, drifting 2vw over 30s. It is why the glass has anything to
 * refract. 30s, not the 5s loop visionOS forbids: motion in the periphery is
 * an attention magnet. */
html {
  background-color: var(--ground);
}

body {
  min-height: 100vh;
  margin: 0;
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 76% 22%, rgba(246, 201, 176, 0.9) 0%, rgba(246, 201, 176, 0) 62%),
    radial-gradient(38% 34% at 8% 96%, rgba(226, 87, 63, 0.14) 0%, rgba(226, 87, 63, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
  background-attachment: fixed;
  background-repeat: no-repeat;
  font-family: var(--font-ui);
  font-size: var(--t-body);
  font-weight: var(--w-body);
  line-height: var(--leading-body);
  letter-spacing: 0.004em;
  color: var(--fg-primary);
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
  animation: display-light-drift 30s ease-in-out infinite alternate;
}

@keyframes display-light-drift {
  from { background-position: 0 0, 0 0, 0 0; }
  to { background-position: 2vw -1vw, -2vw 1vw, 0 0; }
}

/* Grain over the ground, under everything else. */
body::after {
  content: "";
  position: fixed;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  opacity: var(--grain-opacity);
  background-image: var(--grain);
  mix-blend-mode: overlay;
}

/* ── 3. Material: the glass classes, re-tokenised ────────────────────────
 * /vision.css sets --glass-blur ON .glass--thin and .glass--thick, where a
 * :root override cannot reach it, so both are restated. The box-shadow is
 * restated to slot --rim-edge between the specular and the bottom edge. */
.glass,
.glass--thin,
.glass--thick {
  box-shadow: var(--rim-top), var(--shadow-1);
}

.glass { --glass-blur: 18px; }
.glass--thin { --glass-blur: 14px; }
.glass--thick { --glass-blur: 22px; }

/* The inner surface: the only place text may sit. No fill and no contour of
 * its own — a window is one surface. */
.inner {
  border-radius: var(--r-inner);
  padding: var(--pad-inner);
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  background-color: var(--inner-fill);
  box-shadow: none;
}

.inner > .inner {
  box-shadow: none;
  background: none;
  padding: 0;
}

/* Anything the base sheet paints with white alpha, repainted in ink. */
.table th,
.table td { border-bottom-color: var(--hairline); }
.table th { color: var(--fg-tertiary); }
.table thead { background-color: rgba(74, 46, 39, 0.06); }

.button {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.55);
  box-shadow: var(--rim-top), var(--shadow-contact);
}

.button:hover { background-color: rgba(255, 255, 255, 0.78); }

.input {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.5);
  border-color: var(--hairline-strong);
}

.badge {
  color: var(--fg-secondary);
  background-color: rgba(255, 255, 255, 0.5);
  box-shadow: none;
}

.button:focus-visible,
.input:focus-visible { outline-color: var(--accent); }

/* ── 4. Type ─────────────────────────────────────────────────────────────
 * Sans informs, serif speaks. The serif is the companion: once per screen,
 * one sentence, never a paragraph. */
.title-1, .title-2, .title-3 {
  font-weight: var(--w-title);
  letter-spacing: -0.015em;
}

.title-1 { font-size: var(--t-title-1); line-height: 1.14; }
.title-2 { font-size: var(--t-title-2); line-height: 1.2; }
.title-3 { font-size: var(--t-title-3); line-height: 1.25; }

.text { color: var(--fg-secondary); }
.text--secondary { color: var(--fg-secondary); }
.text--tertiary { color: var(--fg-tertiary); }


/* ── 5. Catalogue A — windows (navigation layer, the only glass) ─────────
 * A window is `glass` plus one child, `.inner`: text may not sit on the
 * material, so the fill IS the interior. That makes the concentric rule
 * exact — the window insets by --r-pad (8px), the fill curves by 22 − 8 —
 * and is why these windows drop the base sheet's 22px window padding. */

.display-pane,
.display-panel,
.display-overlay,
.display-ornament {
  position: relative;
  isolation: isolate;
  display: flex;
  flex-direction: column;
  border-radius: var(--r-window);
  padding: var(--r-pad);
  transition:
    transform var(--t-focus) var(--ease),
    opacity var(--t-focus) var(--ease),
    filter var(--t-focus) var(--ease),
    box-shadow var(--t-focus) var(--ease),
    background-color var(--t-focus) var(--ease);
}

/* The sheen sits behind the content, inside the window's own stacking
 * context, so it never touches text. */
.display-pane::before,
.display-panel::before,
.display-overlay::before,
.display-ornament::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: -1;
  border-radius: inherit;
  background-image: var(--sheen);
  pointer-events: none;
}

.display-pane > .inner,
.display-panel > .inner,
.display-overlay > .inner {
  flex: 1 1 auto;
  gap: 10px;
}

.display-pane[data-region="aside"] {
  background-color: var(--glass-tint-thin);
  --glass-blur: 14px;
}

/* A window's body is the column its children stand in. */
.display-pane-body,
.display-panel-body {
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  min-inline-size: 0;
}

.display-ornament-text,
.display-status-text { color: var(--fg-secondary); }

/* The label voice: every kicker, caption and column head. Small means bold —
 * below 15px the weight goes up, never down. */
.display-kicker,
.display-pane-kicker,
.display-value-label,
.display-list-title,
.display-weather-place,
.display-clock-zone,
.display-timer-label,
.display-chat-title,
.display-notification-source,
.display-choice-label,
.display-progress-label,
.display-table-caption,
.display-document-source,
.display-table-grid thead th {
  margin: 0;
  font-size: var(--t-caption);
  font-weight: var(--w-title);
  line-height: 1.35;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--fg-tertiary);
}

/* The figure voice: one number, tight and tabular. */
.display-value-number,
.display-weather-temp,
.display-clock-time,
.display-timer-remaining {
  margin: 0;
  font-size: var(--t-value);
  font-weight: var(--w-title);
  line-height: 1.08;
  letter-spacing: -0.028em;
  font-variant-numeric: tabular-nums;
  color: var(--fg-primary);
}

/* The title voice. */
.display-pane-title,
.display-panel-title,
.display-overlay-title,
.display-document-title,
.display-notification-title {
  margin: 0;
  font-size: var(--t-title-3);
  font-weight: var(--w-title);
  line-height: 1.25;
  letter-spacing: -0.014em;
  color: var(--fg-primary);
}

/* Lists are lists: ul, ol and li, stripped of their bullets. */
.display-list-items,
.display-weather-series,
.display-chat-lines,
.display-choice-options {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
}

/* display-panel — the detail view. `scroll` fades content into the pane's edge
 * instead of letting it stop there. */
.display-panel--scroll > .inner {
  max-block-size: 44vh;
  overflow-y: auto;
  -webkit-mask-image: linear-gradient(to bottom, transparent 0, #000 20px,
    #000 calc(100% - 20px), transparent 100%);
  mask-image: linear-gradient(to bottom, transparent 0, #000 20px,
    #000 calc(100% - 20px), transparent 100%);
}

/* display-overlay — the interruption. Root-level, never inside a pane, the one
 * window allowed above everything. --ttl is its countdown, spent as the
 * duration of the hairline at its foot. Placement uses `translate`, not
 * `transform`, so the state rules can scale it without restating it. */
.display-overlay {
  z-index: 20;
  max-inline-size: 34rem;
}

.display-overlay[data-position="bottom"] {
  position: fixed;
  left: 50%;
  bottom: 96px;
  translate: -50%;
}

.display-overlay[data-position="center"] {
  position: fixed;
  left: 50%;
  top: 50%;
  translate: -50% -50%;
}

.display-overlay::after {
  content: "";
  position: absolute;
  left: var(--r-pad);
  right: var(--r-pad);
  bottom: 3px;
  block-size: 2px;
  border-radius: var(--r-capsule);
  background-color: var(--accent-soft);
  transform-origin: left center;
  animation: display-timer-drain calc(var(--ttl, 0) * 1ms) linear both;
}

.display-overlay-body { margin: 0; color: var(--fg-secondary); }

.display-overlay-actions,
.display-notification-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--gap-s);
  align-items: center;
}

/* display-ornament — the system line, and visionOS' only published z-number: it
 * sits outside the window and overlaps its bottom edge by 20pt. Flat, that
 * reads as "in front of". Capsule, one accent dot. */
.display-ornament {
  z-index: 10;
  align-self: center;
  margin-top: calc(-1 * var(--ornament-overlap));
  padding: 5px;
  border-radius: var(--r-capsule);
  background-color: var(--glass-tint-thin);
  --glass-blur: 14px;
}

.display-ornament > .inner {
  flex-direction: row;
  align-items: center;
  gap: 12px;
  padding: 6px 16px;
  border-radius: var(--r-capsule);
  font-size: var(--t-small);
  color: var(--fg-secondary);
  white-space: nowrap;
}

.display-ornament-dot,
.display-status-dot {
  inline-size: 8px;
  block-size: 8px;
  flex: none;
  border-radius: var(--r-capsule);
  background-color: var(--fg-tertiary);
}

.display-ornament-dot {
  background-color: var(--accent-soft);
  box-shadow: 0 0 0 4px var(--accent-wash);
}

.display-ornament-count {
  font-size: var(--t-caption);
  font-weight: var(--w-title);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

/* ── 6. Catalogue B — content (sits on .inner, never on the material) ──── */

/* display-stack — the only layout element. A stack in a stack is a group. */
.display-stack {
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  min-inline-size: 0;
}

.display-stack--row {
  flex-direction: row;
  flex-wrap: wrap;
  align-items: flex-start;
}

.display-stack[data-gap="s"] { gap: var(--gap-s); }
.display-stack[data-gap="m"] { gap: var(--gap); }
.display-stack[data-gap="l"] { gap: var(--gap-l); }

/* display-value — one number with its unit riding small. */
.display-value,
.display-clock,
.display-timer,
.display-weather,
.display-chat,
.display-list,
.display-choice,
.display-document,
.display-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-inline-size: 0;
}

.display-value-unit,
.display-weather-unit {
  margin-inline-start: 3px;
  font-size: 0.5em;
  letter-spacing: 0;
  color: var(--fg-tertiary);
}

.display-value[data-size="s"], .display-clock[data-size="s"] { --t-value: 24px; }
.display-value[data-size="l"], .display-clock[data-size="l"] { --t-value: 52px; }

/* display-text — body copy. The secondary ink measures 7.45:1 on the fill. */
.display-text {
  margin: 0;
  max-inline-size: 62ch;
  font-size: var(--t-body);
  line-height: 1.5;
  color: var(--fg-primary);
}

.display-text--secondary { color: var(--fg-secondary); }

/* display-voice — the companion: one sentence, one italic accent word at its
 * end, never a paragraph. */
.display-voice {
  margin: 0;
  max-inline-size: 30ch;
  font-family: var(--font-voice);
  font-variation-settings: "SOFT" 55, "WONK" 0, "opsz" 96;
  font-size: var(--t-voice);
  font-weight: 500;
  line-height: 1.22;
  letter-spacing: -0.012em;
  color: var(--fg-primary);
  text-wrap: balance;
}

.display-voice-accent {
  font-style: italic;
  font-weight: 400;
  color: var(--accent);
  /* An oblique cut leans into whatever follows it; give the slant room. */
  padding-inline-end: 0.06em;
}

.display-voice[data-size="l"] { --t-voice: 40px; }

/* display-list / display-item — rows with a hairline, never cards inside cards. */
.display-item {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 8px 0;
  border-top: 1px solid var(--hairline);
  font-size: var(--t-small);
  line-height: 1.45;
}

.display-list-items > .display-item:first-child,
.display-weather-series > .display-item:first-child { border-top: 0; }
.display-list--plain .display-item { border-top: 0; padding: 3px 0; }

.display-item-marker {
  flex: none;
  min-inline-size: 2.4em;
  font-family: var(--font-voice);
  font-variation-settings: "SOFT" 55, "opsz" 72;
  font-size: 17px;
  font-weight: var(--w-title);
  color: var(--fg-tertiary);
}

.display-item-k { flex: 1 1 auto; min-inline-size: 0; color: var(--fg-primary); }
/* A value may be a sentence, not only a number: it shrinks and wraps rather
 * than running out of the line. */
.display-item-v {
  flex: 0 1 auto;
  min-inline-size: 0;
  overflow-wrap: anywhere;
  font-variant-numeric: tabular-nums;
  color: var(--fg-secondary);
}
.display-item--accent .display-item-marker { color: var(--accent); }
.display-item--accent .display-item-k { font-weight: var(--w-title); }

/* display-table — a figure around a grid. */
.display-table { margin: 0; }

.display-table-grid {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--t-small);
  font-variant-numeric: tabular-nums;
}

.display-table-caption { caption-side: top; text-align: left; margin-bottom: 10px; }

.display-table-grid th,
.display-table-grid td {
  text-align: left;
  padding: 7px 10px 7px 0;
  border-bottom: 1px solid var(--hairline);
}

.display-table-grid thead th {
  letter-spacing: 0.08em;
  white-space: nowrap;
  border-bottom-color: var(--hairline-strong);
}

.display-table-grid td:first-child { color: var(--fg-primary); }
.display-table-grid tbody tr:last-child td { border-bottom: 0; }

/* display-weather */
.display-weather-now {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0;
}

.display-weather-glyph { flex: none; font-size: 34px; line-height: 1; }
.display-weather-condition { margin: 0; font-size: var(--t-small); color: var(--fg-secondary); }

.display-weather-range {
  display: flex;
  gap: 10px;
  margin: 0;
  font-size: var(--t-small);
  font-variant-numeric: tabular-nums;
  color: var(--fg-secondary);
}

.display-weather-hi { color: var(--fg-primary); font-weight: var(--w-title); }
.display-weather-lo { color: var(--fg-tertiary); }
.display-weather-range .display-weather-place { margin-inline-start: auto; }

/* display-clock */
.display-clock-date { margin: 0; font-size: var(--t-small); color: var(--fg-secondary); }

/* display-timer — the escalation lives in § 7, the geometry here. */
.display-timer-bar,
.display-progress-track {
  block-size: 6px;
  border-radius: var(--r-capsule);
  background-color: rgba(74, 46, 39, 0.1);
  overflow: hidden;
}

.display-timer-fill,
.display-progress-fill {
  display: block;
  block-size: 100%;
  inline-size: 100%;
  border-radius: inherit;
  background-color: var(--accent-soft);
  transform-origin: left center;
}

/* display-chat — who speaks is a role, and the bubble follows it. */
.display-chat-lines { gap: 8px; }

.display-chat-line {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-inline-size: 34ch;
  padding: 9px 14px;
  border-radius: var(--r-chip);
  font-size: var(--t-small);
  line-height: 1.45;
  color: var(--fg-primary);
}

/* The two speakers -- the kit 5 rule, carried into kit 6 (ruling, 08.09.):
 * the companion is kit 1 unchanged, a light fill and its hairline; "you"
 * keeps kit 1's peach fill and states the same contour loudly, 2px at 35 %. */
.display-chat-line[data-role="you"] {
  align-self: flex-end;
  align-items: flex-end;
  background-color: var(--accent-wash);
  box-shadow: inset 0 0 0 2px rgba(74, 46, 39, 0.35);
  border-end-end-radius: 5px;
}

.display-chat-line[data-role="companion"] {
  align-self: flex-start;
  background-color: rgba(255, 255, 255, 0.55);
  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.08);
  border-end-start-radius: 5px;
}

.display-chat-line-text { margin: 0; }

.display-chat-line-time {
  font-size: 11px;
  font-weight: var(--w-title);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

.display-chat-line--partial .display-chat-line-text {
  color: var(--fg-secondary);
  font-style: italic;
}

.display-chat-line--partial .display-chat-line-text::after { content: "\2009…"; }

/* display-notification */
.display-notification {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 14px 16px;
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);
  /* Quiet keeps its hairline, darker than kit 1 (ruling, 08.09.): the edge
   * against the window's inner fill has to read as an edge. */
  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.22),
    inset 0 1px 0 rgba(255, 255, 255, 0.8);
}

.display-notification-title { font-size: var(--t-body); line-height: 1.3; }
.display-notification-body { margin: 0; font-size: var(--t-small); color: var(--fg-secondary); }

.display-notification-time {
  font-size: var(--t-caption);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

/* Notification is kit 1 unchanged (ruling, 08.09.): a light fill with its
 * hairline, and urgent as the 3px accent bar at the leading edge. */
.display-notification[data-level="urgent"] {
  /* Ruling, 08.09.: the relevant frame around it (depth-2 shadow with its
   * coral glow) and the dark bar at the leading edge twice as wide. */
  box-shadow: inset 6px 0 0 var(--accent-soft),
    inset 0 0 0 1px rgba(74, 46, 39, 0.1),
    inset 0 1px 0 rgba(255, 255, 255, 0.9),
    var(--shadow-2);
  /* The bar breathes like the urgent ring does: one signal, one rhythm. */
  animation: display-notification-breathe 1000ms ease-in-out infinite;
}

@keyframes display-notification-breathe {
  0%, 100% {
    box-shadow: inset 4px 0 0 var(--accent-soft),
      inset 0 0 0 1px rgba(74, 46, 39, 0.1),
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      var(--shadow-2);
  }
  50% {
    box-shadow: inset 10px 0 0 var(--accent-soft),
      inset 0 0 0 1px rgba(74, 46, 39, 0.1),
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      var(--shadow-2);
  }
}

.display-notification[data-level="urgent"] .display-notification-source { color: var(--accent); }

/* display-media / display-chart — the figure is inline SVG from the sender; the sheet
 * owns the frame, so no component carries its own palette. */
.display-media,
.display-chart { display: flex; flex-direction: column; gap: 8px; margin: 0; }

.display-media-frame {
  overflow: hidden;
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);
}

.display-media[data-ratio="16:9"] .display-media-frame { aspect-ratio: 16 / 9; }
.display-media[data-ratio="1:1"] .display-media-frame { aspect-ratio: 1 / 1; }

.display-media-image,
.display-media-frame > svg,
.display-chart-figure > svg { display: block; inline-size: 100%; block-size: auto; }

.display-media-image {
  block-size: 100%;
  object-fit: contain;
  border-radius: var(--r-inner);
}

/* A framed ratio letterboxes its figure; it never crops a drawing. */
.display-media[data-ratio] .display-media-frame > svg { block-size: 100%; object-fit: contain; }

.display-media-caption,
.display-chart-caption { font-size: var(--t-caption); color: var(--fg-tertiary); }

/* display-document */
.display-document-body { font-size: var(--t-body); color: var(--fg-secondary); }
.display-document-body > * { margin: 0 0 10px; }
.display-document-body > *:last-child { margin-bottom: 0; }
.display-document-body strong { color: var(--fg-primary); font-weight: var(--w-title); }

.display-document-pages {
  margin: 0;
  padding-top: 8px;
  border-top: 1px solid var(--hairline);
  font-size: var(--t-caption);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

.display-document-total::before { content: " / "; }

/* display-status */
.display-status {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  font-size: var(--t-small);
  color: var(--fg-secondary);
}

.display-status[data-kind="listening"] .display-status-dot {
  background-color: var(--accent-soft);
  box-shadow: 0 0 0 4px var(--accent-wash);
  animation: display-pulse 2.4s var(--ease) infinite;
}

.display-status[data-kind="speaking"] .display-status-dot {
  background-color: var(--accent);
  box-shadow: 0 0 0 4px var(--accent-wash);
}

.display-status[data-kind="thinking"] .display-status-dot {
  background-color: var(--fg-secondary);
  animation: display-pulse 1.6s var(--ease) infinite;
}

.display-status[data-kind="offline"] .display-status-dot {
  background-color: transparent;
  box-shadow: inset 0 0 0 1.5px var(--fg-tertiary);
}

@keyframes display-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.45; }
}

/* display-action / display-option — controls: control radius, rim without the cast
 * shadow (they sit on a pane, they do not float above one). Hover is a
 * brightening, never an outline. */
.display-action,
.display-option-chip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border: 0;
  font: inherit;
  font-size: var(--t-small);
  font-weight: var(--w-title);
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.62);
  box-shadow: var(--rim-top), var(--shadow-contact);
  cursor: pointer;
  transition: filter var(--t-hover) var(--ease),
    transform var(--t-hover) var(--ease),
    background-color var(--t-hover) var(--ease);
}

.display-action { min-block-size: 40px; padding: 9px 18px; border-radius: var(--r-control); }

.display-option-chip {
  min-block-size: 36px;
  padding: 7px 14px;
  border-radius: var(--r-chip);
  font-weight: var(--w-body);
  color: var(--fg-secondary);
}

.display-action:hover,
.display-option-chip:hover { filter: brightness(1.06); transform: translateY(-2px); }
.display-action:active,
.display-option-chip:active { transform: translateY(0) scale(0.985); }

.display-action:focus-visible,
.display-option-chip:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }

.display-action--primary,
.display-option--selected > .display-option-chip {
  color: #2b1d19;
  font-weight: var(--w-title);
  background-color: var(--accent-soft);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.45), var(--shadow-contact);
}

/* display-choice */
.display-choice-options { flex-direction: row; flex-wrap: wrap; gap: 8px; }
.display-option { display: inline-flex; }

/* display-chart */
.display-chart-figure { display: block; }

/* display-progress */
.display-progress-fill {
  inline-size: calc(var(--value, 0) * 1%);
  transition: inline-size var(--t-focus) var(--ease);
}

/* Fields — an inner element that is a THING and not a line of text sits in the
 * window the way a quiet notification does (ruling, 08.09.): a light fill, one
 * hairline that reads as an edge, one light edge on top, the derived radius,
 * and its own padding. The window keeps its own inset, so a field inside one is
 * inset twice; that is the point, and it is why the padding is stated here
 * rather than borrowed from the pane.
 *
 * Rows of text are NOT fields. `.display-text`, `.display-voice`, `.display-kicker`,
 * `.display-value`, `.display-action`, `.display-stack`, `.display-item`, `.display-chat-line` and
 * `.display-option` stay type on the inner surface. The contour is an inset shadow
 * and never a border, so no field grows by the pixel it wears — the same reason
 * the four state rings are shadows. */
.display-list,
.display-table,
.display-chat,
.display-document,
.display-media,
.display-chart,
.display-choice,
.display-progress,
.display-weather,
.display-clock,
.display-timer,
.display-status {
  padding: 14px 16px;
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);
  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.22),
    inset 0 1px 0 rgba(255, 255, 255, 0.8);
}

/* A picture inside a field needs no second surface under it, and a table head
 * stays legible as a band rather than as the line kit 6 took away. */
.display-media .display-media-frame { background-color: transparent; }
.display-table-grid thead { background-color: rgba(74, 46, 39, 0.06); }

/* ── 7. States (contract § 3) ────────────────────────────────────────────
 * Focus is handed out by the STATE, never by a click and never by a gaze: on
 * a screen without eyes, the companion looks for you. The server writes
 * data-state on exactly one window. */

:where(.display-pane, .display-panel, .display-overlay, .display-ornament)[data-state="hidden"] {
  display: none;
}

/* What is on the screen has focus. A window nobody assigned a state to is not
   a window in the background: the ladder below assigns a state, and the ladder
   is what the next wave will use. Until then every pane stands at the top rung.
   The neighbour rule stays on the ATTRIBUTE on purpose -- with focus as the
   default, a `:has()` that fired on the default would make every window recede
   in front of every other one. `:where()` keeps the rule at 0-1-0 like every
   rung of the ladder, so it wins over the base rule above by order alone and
   loses to the forced-colours fallback below. */
.display-pane:where(:not([data-state]), [data-state=""]) {
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2);
}

/* Rung 1 — ambient. It inherits what relevant used to carry: the plain
 * window, its specular edge and the contact-and-cast shadow, stated here so
 * the ladder can be read in one place. Scale and opacity are kit 1's. */
:where(.display-pane, .display-panel, .display-overlay)[data-state="ambient"] {
  transform: scale(0.94);
  opacity: 0.78;
  background-color: var(--glass-tint-thin);
  --glass-blur: 14px;
  box-shadow: var(--rim-top), var(--shadow-1);
}

/* Rung 2 — relevant takes what focus used to have: the depth-2 shadow, whose
 * last layer is a 1px coral glow. The lift stays with focus; only the edge
 * moved down a step. */
:where(.display-pane, .display-panel, .display-overlay)[data-state="relevant"] {
  transform: none;
  opacity: 1;
  box-shadow: var(--rim-top), var(--shadow-2);
}

/* Rung 3 — focus takes what urgent used to have: a 2px coral ring, drawn as
 * an inset shadow so it costs no space. Depth 2 is still depth 2: the lift,
 * the scale and the flat text are kit 1's. */
:where(.display-pane, .display-panel, .display-overlay)[data-state="focus"] {
  transform: translateY(-4px) scale(1.02);
  opacity: 1;
  background-color: var(--glass-focus);
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top),
    var(--shadow-2);
  z-index: 5;
}

/* Rung 4 — urgent is new: a 4px ring in vermilion, the darker half of the
 * accent, on the densest glass in the sheet. Twice the ring focus wears and a
 * different colour, so the last step of the ladder cannot be mistaken for the
 * one before it. Still an inset shadow, so the window does not change size. */
:where(.display-pane, .display-panel, .display-overlay)[data-state="urgent"] {
  transform: translateY(-4px) scale(1.02);
  opacity: 1;
  background-color: var(--glass-tint-thick);
  box-shadow: inset 0 0 0 4px var(--accent-soft), var(--rim-top), var(--shadow-2);
  z-index: 30;
  /* Urgent breathes in the WIDTH of its coral ring (ruling, 08.09.): 3px to
   * 6px and back once a second. No fill, no halo -- the ring is the signal. */
  animation: display-urgent-breathe 1000ms ease-in-out infinite;
}

@keyframes display-urgent-breathe {
  0%, 100% {
    box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2),
      0 0 0 0 rgba(232, 102, 79, 0);
  }
  50% {
    box-shadow: inset 0 0 0 7px var(--accent-soft), var(--rim-top), var(--shadow-2),
      0 0 0 5px rgba(232, 102, 79, 0.35);
  }
}

/* Level of detail follows the state — the Nest Hub rule. The step decides
 * WHAT is in the window: .display-lead always, .display-line from relevant on,
 * .display-detail only in focus and urgent. */
.display-lead { display: block; }

:where(.display-pane, .display-panel, .display-overlay)[data-state="ambient"]
  :is(.display-line, .display-detail) { display: none; }

:where(.display-pane, .display-panel, .display-overlay)[data-state="relevant"]
  .display-detail { display: none; }

/* Neighbours give way. One :has() on the group and the whole ensemble reacts,
 * with no JavaScript and no per-element bookkeeping. */
.display-scene:has(> [data-state="focus"], > [data-state="urgent"])
  > :where(.display-pane, .display-panel)[data-state]:not([data-state="focus"]):not([data-state="urgent"]),
.display-columns:has([data-state="focus"], [data-state="urgent"])
  :where(.display-pane, .display-panel)[data-state]:not([data-state="focus"]):not([data-state="urgent"]):not([data-state="hidden"]) {
  transform: scale(0.985);
  filter: blur(1.5px);
  opacity: 0.72;
}

.display-scene:has(> [data-state="focus"], > [data-state="urgent"])
  > :where(.display-pane, .display-panel)[data-state="ambient"] {
  transform: scale(0.925);
  opacity: 0.6;
}

/* Life cycle: fresh → settled → leaving. A whole tree after a lost frame
 * carries `settled`, so nothing flies in twice. */
:where(.display-pane, .display-panel, .display-overlay, .display-ornament)[data-age="fresh"] {
  animation: display-enter var(--t-enter) var(--ease) both;
}

:where(.display-pane, .display-panel, .display-overlay, .display-ornament)[data-age="leaving"] {
  animation: display-leave var(--t-leave) var(--ease) both;
  pointer-events: none;
}

@keyframes display-enter {
  from { opacity: 0; transform: translateY(10px) scale(0.985); }
  to { opacity: 1; transform: none; }
}

@keyframes display-leave {
  from { opacity: 1; transform: none; filter: blur(0); }
  to { opacity: 0; transform: translateY(-6px); filter: blur(2px); }
}

/* For engines that animate a new node before a class can reach it. */
@starting-style {
  :where(.display-pane, .display-panel, .display-overlay)[data-age="fresh"] {
    opacity: 0;
    transform: translateY(10px) scale(0.985);
  }
}

/* Timer escalation without a server tick. The template writes --end-at
 * (epoch ms) and --total (ms); the start follows from both, the elapsed time
 * from an optional --now the shell stamps once per page. A negative
 * animation-delay drops the animation in at the right frame and the browser
 * runs the rest. With no --now the bar drains over --total from first paint —
 * the honest degradation. */
.display-timer {
  --start-at: calc(var(--end-at) - var(--total));
  --elapsed: calc(var(--now, var(--start-at)) - var(--start-at));
}

.display-timer-fill {
  animation:
    display-timer-drain calc(var(--total, 0) * 1ms) linear
      calc(var(--elapsed, 0) * -1ms) both,
    display-timer-heat calc(var(--total, 0) * 1ms) linear
      calc(var(--elapsed, 0) * -1ms) both;
}

.display-timer-remaining {
  animation: display-timer-alarm calc(var(--total, 0) * 1ms) linear
    calc(var(--elapsed, 0) * -1ms) both;
}

@keyframes display-timer-drain {
  from { transform: scaleX(1); }
  to { transform: scaleX(0); }
}

/* The last tenth is the escalation: coral turns to vermilion. */
@keyframes display-timer-heat {
  0%, 82% { background-color: var(--accent-soft); }
  92%, 100% { background-color: var(--accent); }
}

@keyframes display-timer-alarm {
  0%, 88% { color: var(--fg-primary); }
  96%, 100% { color: var(--accent); }
}

.display-timer--done .display-timer-fill {
  animation: none;
  transform: scaleX(1);
  background-color: var(--accent);
}

.display-timer--done .display-timer-remaining { animation: none; color: var(--accent); }

/* Stable identity. The template writes `view-transition-name: <id>` inline,
 * so the sheet only says how a transition moves, never which one. Glass keeps
 * its blur under a name; during the transition itself the snapshot is flat,
 * which is why the DNA says glass does not move. */
::view-transition-group(*) {
  animation-duration: var(--t-focus);
  animation-timing-function: var(--ease);
}

/* ── 8. The screen ───────────────────────────────────────────────────────
 * The kit had a gallery page before it had a screen. This is what the screen
 * says about its own typography and its air: one size, one weight, one
 * leading, and the space between the two regions. Which region is wide and
 * which is narrow is NOT said here -- that is a statement about this screen
 * and stays in the shell's own layout rules, beside the sheet. */
.display-columns {
  position: relative;
  z-index: 1;
  --gap: 20px;
  padding: 28px;
  gap: 24px;
  font-family: var(--font-ui);
  font-size: 15px;
  font-weight: var(--w-body);
  line-height: 1.5;
  letter-spacing: 0.004em;
  color: var(--fg-primary);
}

.display-columns > [data-region] { gap: 24px; }

@media (max-width: 60rem) {
  .display-columns { padding: 16px; gap: 16px; }
}

/* ── 9. The screen's own furniture ───────────────────────────────────────
 * The microphone is the one thing on the screen that belongs to the screen
 * and not to anything standing on it. Where it sits is the shell's business;
 * what it looks like is the sheet's: the one capsule the DNA allows, thin
 * glass with no frame, and the accent surface when the key is down. Its two
 * lines are the label tier, on the same three alphas as everything else. */
.display-mic-button {
  padding: 10px 18px;
  border: 0;
  border-radius: var(--r-capsule);
  font: inherit;
  font-size: var(--t-small);
  font-weight: var(--w-title);
  color: var(--fg-primary);
  background-color: var(--glass-tint-thin);
  -webkit-backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  box-shadow: var(--rim-top), var(--shadow-contact);
  cursor: pointer;
  touch-action: none;
  -webkit-user-select: none;
  user-select: none;
}

/* The pressed state is the only feedback that the key is down, so it is a
 * real change and not a hover: the accent surface, with ink on it. */
.display-mic-button[aria-pressed="true"] {
  color: #2b1d19;
  background-color: var(--accent-soft);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.45), var(--shadow-contact);
}

.display-mic-button:disabled {
  color: var(--fg-tertiary);
  cursor: default;
}

.display-mic-button:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }

.display-mic-line {
  max-width: 22rem;
  text-align: right;
  font-size: var(--t-small);
  color: var(--fg-secondary);
}

.display-mic-state {
  font-size: var(--t-caption);
  color: var(--fg-tertiary);
}

/* ── 10. The scene — a stack that is a picture of a screen ──────────────
 * `display-stack` with `scene` writes it: a screen inside the page, and the
 * group the focus rule hangs its :has() on. It carries the ground, so glass
 * has something behind it. 16:9 is the reference frame; that it is mostly
 * empty is the point. */
.display-scene {
  position: relative;
  isolation: isolate;
  align-items: flex-start;
  padding: 30px;
  overflow: hidden;
  border-radius: var(--r-window);
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 76% 22%, rgba(246, 201, 176, 0.9) 0%, rgba(246, 201, 176, 0) 62%),
    radial-gradient(38% 34% at 8% 96%, rgba(226, 87, 63, 0.14) 0%, rgba(226, 87, 63, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
  box-shadow: var(--shadow-1);
}

/* The reference frame, asked for by ratio: 16:9, capped at 1280 wide so a
 * 1920 page does not blow it up to 1600 x 900, panes centred in the void. */
.display-scene[data-ratio="16:9"] {
  aspect-ratio: 16 / 9;
  inline-size: 100%;
  max-inline-size: 1280px;
  margin-inline: auto;
  align-items: center;
}

.display-scene > .display-pane,
.display-scene > .display-panel { flex: 1 1 0; min-inline-size: 0; }
.display-scene > .display-pane[data-region="aside"] { flex: 0 0 clamp(190px, 20%, 264px); }
.display-scene > .display-ornament { align-self: flex-end; margin: auto auto 0; }
.display-scene .display-voice { max-inline-size: 34ch; }

/* ── 11. Night — a variant, not a second design ──────────────────────────
 * Nothing below is a new rule; it is the same vocabulary with other values. */
[data-ground="night"] {
  color-scheme: dark;
  --ground: #17100e;
  --ground-2: #241612;
  --bg-void: #17100e;
  --glass-tint: rgba(255, 246, 236, 0.085);
  --glass-tint-thin: rgba(255, 246, 236, 0.05);
  --glass-tint-thick: rgba(255, 246, 236, 0.13);
  --glass-focus: rgba(255, 246, 236, 0.13);
  --glass-saturate: 160%;
  --glass-brightness: 1.05;
  --glass-opaque: #2a1a16;
  --inner-fill: rgba(0, 0, 0, 0.16);
  --inner-fill-strong: rgba(0, 0, 0, 0.24);
  --fg-primary: rgba(247, 239, 230, 0.96);
  --fg-secondary: rgba(247, 239, 230, 0.72);
  --fg-tertiary: rgba(247, 239, 230, 0.46);
  --hairline: rgba(247, 239, 230, 0.13);
  --hairline-strong: rgba(247, 239, 230, 0.24);
  --accent: #f0876e;
  --accent-soft: #f0876e;
  --accent-wash: rgba(240, 135, 110, 0.18);
  --accent-ring: rgba(240, 135, 110, 0.28);
  --rim-top: inset 0 1px 0 rgba(255, 255, 255, 0.26);
  --rim-edge: 0 0 transparent;
  --rim-bottom: 0 0 transparent;
  --sheen: linear-gradient(135deg, rgba(255, 255, 255, 0.14) 0%,
    rgba(255, 255, 255, 0.04) 26%, rgba(255, 255, 255, 0) 56%);
  /* The two lines the shadow channels were built for. */
  --shadow-hue: 20 32% 8%;
  --shadow-strength: 30%;
}

[data-ground="night"],
[data-ground="night"] .display-scene {
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 72% 38%, rgba(240, 135, 110, 0.3) 0%, rgba(240, 135, 110, 0) 64%),
    radial-gradient(40% 36% at 10% 8%, rgba(246, 201, 176, 0.1) 0%, rgba(246, 201, 176, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
}

/* Fills are materials, not colour roles, so their literals switch by hand. */
[data-ground="night"] :is(.inner, .display-notification, .display-media-frame) {
  box-shadow: none;
}

[data-ground="night"] :is(.display-action, .display-option-chip, .button, .badge,
  .input, .display-chat-line[data-role="companion"]) {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.12);
  box-shadow: var(--rim-top);
}

[data-ground="night"] :is(.display-action--primary,
  .display-option--selected > .display-option-chip) {
  color: #1b1210;
  background-color: var(--accent-soft);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.3);
}

[data-ground="night"] :is(.display-timer-bar, .display-progress-track) {
  background-color: rgba(255, 255, 255, 0.12);
}

[data-ground="night"] .display-chat-line[data-role="you"] {
  background-color: var(--accent-wash);
}

/* ── 12. Fallbacks ───────────────────────────────────────────────────────
 * The material has three ways of not existing; each gets an opaque surface
 * rather than a degraded glass, because a translucent fill with no blur
 * behind it is text over noise. Every window carries one of the three glass
 * classes, so those three are the whole list. */

@supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
  :is(.glass, .glass--thin, .glass--thick) { background-color: var(--glass-opaque); }
}

@media (prefers-reduced-transparency: reduce) {
  :is(.glass, .glass--thin, .glass--thick) {
    background-color: var(--glass-opaque);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }

  :is(.display-pane, .display-panel, .display-overlay, .display-ornament)::before { display: none; }
  body { background-image: none; animation: none; }
  body::after { display: none; }
}

@media (prefers-contrast: more) {
  :root {
    --fg-secondary: rgba(43, 29, 25, 0.92);
    --fg-tertiary: rgba(43, 29, 25, 0.76);
    --hairline: rgba(74, 46, 39, 0.3);
    --hairline-strong: rgba(74, 46, 39, 0.5);
    --rim-edge: 0 0 transparent;
    --inner-fill: rgba(255, 255, 255, 0.72);
    --inner-fill-strong: rgba(255, 255, 255, 0.86);
    --accent: #962a17;
  }

  [data-ground="night"] {
    --fg-secondary: rgba(247, 239, 230, 0.9);
    --fg-tertiary: rgba(247, 239, 230, 0.74);
    --rim-edge: 0 0 transparent;
  }

  :is(.glass, .glass--thin, .glass--thick) {
    background-color: var(--glass-opaque);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }

  .display-scene:has(> [data-state="focus"], > [data-state="urgent"])
    > :where(.display-pane, .display-panel)[data-state]:not([data-state="focus"]):not([data-state="urgent"]) {
    filter: none;
    opacity: 0.9;
  }
}

/* Forced colours: the OS owns every colour and we get out of the way. */
@media (forced-colors: active) {
  .display-pane,
  .display-panel,
  .display-overlay,
  .display-ornament,
  .display-notification,
  .display-action,
  .display-option-chip,
  .display-media-frame,
  .display-chat-line,
  .display-scene {
    background: Canvas;
    color: CanvasText;
    border: 1px solid CanvasText;
    box-shadow: none;
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
    forced-color-adjust: none;
  }

  .display-action--primary,
  .display-option--selected > .display-option-chip { background: Highlight; color: HighlightText; }

  .display-ornament-dot,
  .display-status-dot,
  .display-timer-fill,
  .display-progress-fill { background: Highlight; }

  :is(.display-pane, .display-panel, .display-overlay, .display-ornament)::before { display: none; }
  .display-scene, body { background-image: none; }
  .display-scene:has(> [data-state="focus"]) > .display-pane { filter: none; opacity: 1; }
}

/* ── 13. Motion ──────────────────────────────────────────────────────────
 * Reduced motion: the light stands still, nothing translates or scales, and a
 * state change is a 150ms crossfade. The base sheet flattens every duration
 * to 0.01ms with !important, so the crossfade has to shout back — that is the
 * only !important here. */
@media (prefers-reduced-motion: reduce) {
  body { animation: none; }

  .display-pane,
  .display-panel,
  .display-overlay,
  .display-ornament,
  .display-action,
  .display-option-chip,
  .display-progress-fill {
    transition: opacity 150ms linear !important;
    animation: none !important;
  }

  :where(.display-pane, .display-panel, .display-overlay)[data-state="ambient"],
  :where(.display-pane, .display-panel, .display-overlay)[data-state="focus"],
  :where(.display-pane, .display-panel, .display-overlay)[data-state="urgent"],
  .display-action:hover,
  .display-option-chip:hover { transform: none; }

  .display-scene:has(> [data-state="focus"], > [data-state="urgent"])
    > :where(.display-pane, .display-panel)[data-state]:not([data-state="focus"]):not([data-state="urgent"]),
  .display-columns:has([data-state="focus"], [data-state="urgent"])
    :where(.display-pane, .display-panel)[data-state]:not([data-state="focus"]):not([data-state="urgent"]):not([data-state="hidden"]) {
    transform: none;
    filter: none;
  }

  .display-status .display-status-dot { animation: none !important; }
  /* Urgent keeps breathing under reduced motion, only slower: it is the one
   * animation that carries meaning (ruling, 08.09.). */
  :where(.display-pane, .display-panel, .display-overlay)[data-state="urgent"] {
    animation: display-urgent-breathe 2000ms ease-in-out infinite !important;
  }
  .display-notification[data-level="urgent"] {
    animation: display-notification-breathe 2000ms ease-in-out infinite !important;
  }

  /* Timer and overlay keep their clocks: they are information, not decoration. */
  .display-timer-fill,
  .display-timer-remaining {
    animation-duration: calc(var(--total, 0) * 1ms) !important;
  }

  .display-overlay::after { animation-duration: calc(var(--ttl, 0) * 1ms) !important; }
}

/* ── 14. Screen sizes ────────────────────────────────────────────────────
 * 1920 is the reference; 1280 and 390 stay readable. Nothing here changes the
 * language, only how much air it gets. */
@media (max-width: 80rem) {
  .display-scene { padding: 22px; }
}

@media (max-width: 48rem) {
  :root {
    --pad-window: 18px;
    --pad-inner: 14px;
    --r-window: 18px;
    --t-voice: 24px;
    --t-value: 30px;
  }

  .display-scene {
    aspect-ratio: auto;
    flex-direction: column;
    align-items: stretch;
    padding: 16px;
  }
  .display-scene > .display-pane[data-region="aside"] { flex: 1 1 auto; }
  .display-ornament > .inner { white-space: normal; }
}
"""

# One style block, in this order: the layout rules, the faces (a prop, raw,
# empty unless the operator serves fonts), and the sheet LAST -- it
# re-tokenises `vision.css` at equal specificity and needs nothing stacked to
# win.
SHELL_TEMPLATE = (
    '{{#if stylesheet}}<link rel="stylesheet" href="vision.css">{{/if}}'
    "<style>" + LAYOUT_RULES + "{{&faces}}" + KIT_CSS + "</style>"
    + '<div class="stack display-columns">{{children}}</div>'
)

REGION_TEMPLATE = '<div class="stack" data-region="{{region}}">{{children}}</div>'

# The prose view wears the catalogue: a thin `display-pane` outside, a
# `display-kicker` for the title and a `display-text` for the paragraph inside,
# on the pane's `.inner` fill. The same glass as before, in the sheet's words.
PROSE_TEMPLATE = (
    '<section class="display-pane glass--thin" data-view="{{view_id}}"'
    ' data-owner="{{owner}}"><div class="inner">'
    '{{#if title}}<p class="display-kicker display-line">{{title}}</p>{{/if}}'
    '<div class="display-pane-body">'
    '<p class="display-text display-line">{{body}}</p></div></div></section>'
)

# A bare wrapper: `data-view` and `data-owner` are its identity, and it wears
# no class, because no sheet on this screen has a rule for one. What it looks
# like is the business of the tree hung inside it.
CUSTOM_TEMPLATE = (
    '<div data-view="{{view_id}}" data-owner="{{owner}}">{{children}}</div>'
)

# ---------------------------------------------------------------------------
# Catalogue A. Windows -- navigation layer, and the only glass in the catalogue.
# Taken from the kit that drew the design language, under the screen's own prefix.
#
# `glass{{#if thin}}--thin{{/if}}` renders as `glass` or as `glass--thin`. The
# language has no `else`, and the alternative -- two classes and a sheet rule
# that undoes one of them -- would put the material in two places. The class
# scanner in the `web` cell blanks every tag before it splits on whitespace, so
# what it sees here is the word `glass`, which is what this component means.

PANE_TEMPLATE = (
    '<section class="display-pane glass{{#if thin}}--thin{{/if}}" id="{{pane_id}}"'
    ' data-state="{{state}}" data-age="{{age}}" data-region="{{region}}"'
    ' style="view-transition-name: {{pane_id}}">'
    '<div class="inner">'
    '{{#if kicker}}<p class="display-pane-kicker display-line">{{kicker}}</p>{{/if}}'
    '{{#if title}}<h2 class="display-pane-title display-lead">{{title}}</h2>{{/if}}'
    '<div class="display-pane-body">{{children}}</div>'
    "</div></section>"
)

PANEL_TEMPLATE = (
    '<section class="display-panel glass{{#if scroll}} display-panel--scroll{{/if}}"'
    ' id="{{pane_id}}" data-state="{{state}}" data-age="{{age}}"'
    ' style="view-transition-name: {{pane_id}}">'
    '<div class="inner">'
    '{{#if title}}<h2 class="display-panel-title display-lead">{{title}}</h2>{{/if}}'
    '<div class="display-panel-body">{{children}}</div>'
    "</div></section>"
)

# `--ttl` is the overlay's own countdown, spent by the sheet as an animation
# duration. The server never ticks: the browser owns elapsed time.
OVERLAY_TEMPLATE = (
    '<aside class="display-overlay glass--thick" id="{{pane_id}}"'
    ' data-state="{{state}}" data-position="{{position}}"'
    ' style="view-transition-name: {{pane_id}}; --ttl: {{ttl_ms}}">'
    '<div class="inner">'
    '{{#if title}}<h2 class="display-overlay-title display-lead">{{title}}</h2>{{/if}}'
    '{{#if body}}<p class="display-overlay-body display-line">{{body}}</p>{{/if}}'
    '<div class="display-overlay-actions">{{children}}</div>'
    "</div></aside>"
)

ORNAMENT_TEMPLATE = (
    '<nav class="display-ornament glass"><div class="inner">'
    '{{#if dot}}<span class="display-ornament-dot"></span>{{/if}}'
    '{{#if text}}<span class="display-ornament-text display-lead">{{text}}</span>{{/if}}'
    '{{#if count}}<span class="display-ornament-count">{{count}}</span>{{/if}}'
    "{{children}}</div></nav>"
)

# ---------------------------------------------------------------------------
# Catalogue B. Content -- everything that sits on a window's `.inner` fill.

VALUE_TEMPLATE = (
    '<div class="display-value" data-size="{{size}}" data-trend="{{trend}}">'
    '<p class="display-value-number display-lead">{{value}}'
    '{{#if unit}}<span class="display-value-unit">{{unit}}</span>{{/if}}</p>'
    '{{#if label}}<p class="display-value-label display-line">{{label}}</p>{{/if}}'
    "</div>"
)

TEXT_TEMPLATE = (
    '<p class="display-text display-line{{#if secondary}} display-text--secondary{{/if}}">'
    "{{body}}</p>"
)

# The one italic accent word of a spoken line. One per line, never two: the
# emphasis is the companion's, and two emphases are none.
VOICE_TEMPLATE = (
    '<p class="display-voice display-lead" data-size="{{size}}">{{text}}'
    '{{#if accent}} <em class="display-voice-accent">{{accent}}</em>{{/if}}</p>'
)

KICKER_TEMPLATE = '<p class="display-kicker display-line">{{text}}</p>'

LIST_TEMPLATE = (
    '<div class="display-list{{#if plain}} display-list--plain{{/if}}">'
    '{{#if title}}<h3 class="display-list-title display-lead">{{title}}</h3>{{/if}}'
    '<ul class="display-list-items">{{children}}</ul></div>'
)

ITEM_TEMPLATE = (
    '<li class="display-item display-line{{#if accent}} display-item--accent{{/if}}">'
    '{{#if marker}}<span class="display-item-marker">{{marker}}</span>{{/if}}'
    '{{#if k}}<span class="display-item-k">{{k}}</span>{{/if}}'
    '{{#if v}}<span class="display-item-v">{{v}}</span>{{/if}}</li>'
)

# A table has as many columns as it has, which the template language cannot
# say. So the two rows-of-cells props are raw HTML, composed by whoever sends
# the object -- and everything a model wrote has to be escaped on the way in.
TABLE_TEMPLATE = (
    '<figure class="display-table display-detail"><table class="display-table-grid">'
    '{{#if caption}}<caption class="display-table-caption">{{caption}}</caption>{{/if}}'
    "{{#if head}}<thead>{{&head}}</thead>{{/if}}"
    "<tbody>{{&rows}}</tbody></table></figure>"
)

WEATHER_TEMPLATE = (
    '<div class="display-weather">'
    '<p class="display-weather-now display-lead">'
    '{{#if glyph}}<span class="display-weather-glyph">{{glyph}}</span>{{/if}}'
    '<span class="display-weather-temp">{{temp}}'
    '{{#if unit}}<span class="display-weather-unit">{{unit}}</span>{{/if}}</span></p>'
    '{{#if condition}}<p class="display-weather-condition display-line">{{condition}}</p>{{/if}}'
    '<p class="display-weather-range display-line">'
    '{{#if hi}}<span class="display-weather-hi">{{hi}}</span>{{/if}}'
    '{{#if lo}}<span class="display-weather-lo">{{lo}}</span>{{/if}}'
    '{{#if place}}<span class="display-weather-place">{{place}}</span>{{/if}}</p>'
    '<ul class="display-weather-series display-detail">{{children}}</ul></div>'
)

CLOCK_TEMPLATE = (
    '<div class="display-clock" data-size="{{size}}">'
    '<p class="display-clock-time display-lead">{{time}}</p>'
    '{{#if date}}<p class="display-clock-date display-line">{{date}}</p>{{/if}}'
    '{{#if zone}}<p class="display-clock-zone display-detail">{{zone}}</p>{{/if}}</div>'
)

# `--end-at` is an epoch in milliseconds and `--total` the span it was set for.
# The bar is a CSS animation with a negative delay computed from the two, so a
# page that was loaded late still shows the right remainder without a tick.
TIMER_TEMPLATE = (
    '<div class="display-timer{{#if done}} display-timer--done{{/if}}"'
    ' style="--end-at: {{end_at}}; --total: {{total_ms}}">'
    '<p class="display-timer-remaining display-lead">{{remaining}}</p>'
    '{{#if label}}<p class="display-timer-label display-line">{{label}}</p>{{/if}}'
    '<div class="display-timer-bar"><span class="display-timer-fill"></span></div></div>'
)

CHAT_TEMPLATE = (
    '<div class="display-chat">'
    '{{#if title}}<h3 class="display-chat-title display-lead">{{title}}</h3>{{/if}}'
    '<ol class="display-chat-lines">{{children}}</ol></div>'
)

CHAT_LINE_TEMPLATE = (
    '<li class="display-chat-line display-line'
    '{{#if partial}} display-chat-line--partial{{/if}}" data-role="{{role}}">'
    '<p class="display-chat-line-text">{{text}}</p>'
    '{{#if time}}<span class="display-chat-line-time display-detail">{{time}}</span>{{/if}}'
    "</li>"
)

NOTIFICATION_TEMPLATE = (
    '<div class="display-notification" data-level="{{level}}">'
    '{{#if source}}<p class="display-notification-source display-line">{{source}}</p>{{/if}}'
    '<h3 class="display-notification-title display-lead">{{title}}</h3>'
    '{{#if body}}<p class="display-notification-body display-detail">{{body}}</p>{{/if}}'
    '{{#if time}}<span class="display-notification-time display-detail">{{time}}</span>{{/if}}'
    '<div class="display-notification-actions display-detail">{{children}}</div></div>'
)

# `figure` is the way in for an inline SVG or a frame; `src` is the way in for a
# file the display can already reach. A specimen uses one or the other.
MEDIA_TEMPLATE = (
    '<figure class="display-media" data-ratio="{{ratio}}">'
    '{{#if figure}}<div class="display-media-frame">{{&figure}}</div>{{/if}}'
    '{{#if src}}<img class="display-media-image" src="{{src}}" alt="{{alt}}">{{/if}}'
    '{{#if caption}}<figcaption class="display-media-caption display-line">{{caption}}'
    "</figcaption>{{/if}}</figure>"
)

DOCUMENT_TEMPLATE = (
    '<article class="display-document">'
    '{{#if title}}<h2 class="display-document-title display-lead">{{title}}</h2>{{/if}}'
    '{{#if source}}<p class="display-document-source display-line">{{source}}</p>{{/if}}'
    '<div class="display-document-body display-detail">{{&body}}</div>'
    '{{#if pages}}<p class="display-document-pages display-detail">'
    "<span>{{page}}</span>"
    '<span class="display-document-total">{{pages}}</span></p>{{/if}}</article>'
)

STATUS_TEMPLATE = (
    '<p class="display-status display-lead" data-kind="{{kind}}">'
    '<span class="display-status-dot"></span>'
    '<span class="display-status-text">{{text}}</span></p>'
)

ACTION_TEMPLATE = (
    '<button class="display-action{{#if primary}} display-action--primary{{/if}}"'
    ' type="button"{{#if event}} phx-click="{{event}}"{{/if}}>{{label}}</button>'
)

CHOICE_TEMPLATE = (
    '<div class="display-choice">'
    '{{#if label}}<p class="display-choice-label display-line">{{label}}</p>{{/if}}'
    '<ul class="display-choice-options">{{children}}</ul></div>'
)

OPTION_TEMPLATE = (
    '<li class="display-option{{#if selected}} display-option--selected{{/if}}">'
    '<button class="display-option-chip" type="button"'
    '{{#if event}} phx-click="{{event}}"{{/if}}>{{label}}</button></li>'
)

CHART_TEMPLATE = (
    '<figure class="display-chart display-detail">'
    '<div class="display-chart-figure">{{&figure}}</div>'
    '{{#if caption}}<figcaption class="display-chart-caption display-line">{{caption}}'
    "</figcaption>{{/if}}</figure>"
)

# `scene` is the container the state rules need: the focus rule dims a pane's
# NEIGHBOURS, which is a statement about a group, and `:has()` needs an element
# to hang it on. It is deliberately NOT the way to put windows in a row -- a
# scene is a picture of a screen, with a screen's proportions and a screen's
# neighbour effect, and a row of specimens that each want to be seen on their own
# terms is a row and nothing more.
#
# `ratio` is what tells the sheet which kind of screen: a scene with no ratio
# takes its height from what is in it.
STACK_TEMPLATE = (
    '<div class="display-stack{{#if row}} display-stack--row{{/if}}'
    '{{#if scene}} display-scene{{/if}}" data-gap="{{gap}}"'
    ' data-ratio="{{ratio}}">{{children}}</div>'
)

PROGRESS_TEMPLATE = (
    '<div class="display-progress" data-ratio="{{value}}" style="--value: {{value}}">'
    '<div class="display-progress-track"><span class="display-progress-fill"></span></div>'
    '{{#if label}}<p class="display-progress-label display-line">{{label}}</p>{{/if}}</div>'
)

# The name of the `voice` cell this screen speaks to, from `params.voice_mount`.
# A module-level default, so a caller that says nothing gets the shipped name;
# the dispatcher below replaces it with what this cell was configured with, once
# per message. It is a MOUNT and not a path: the button joins `voice:<call>` on
# this page's own socket, and the `web` cell hands the frames to whichever cell
# holds that name in the process (GH #643).
VOICE_MOUNT = "voice"

# Where the two faces are served from, from `params.font_base`. A URL prefix
# RELATIVE to the page's own base (the page carries `<base href>`), so an
# absolute path would break under the next proxy prefix and an `https://` would
# be an external request. Empty is the shipped default and means no
# `@font-face` at all: a set but unserved base sends two requests into nothing.
FONT_BASE = ""


def faces(font_base):
    """The two @font-face rules, or none at all.

    No font file ships in this repository: the faces are an operator asset and
    reach the page from a directory in front of the listener. An empty base is
    the default and means the fallback stacks in the sheet carry the type.
    """
    if not font_base:
        return ""
    # A directory, with or without its trailing slash: `fonts` and `fonts/`
    # both serve `fonts/inter.woff2`, and a base glued to a file name is not.
    if not font_base.endswith("/"):
        font_base += "/"
    return (
        '@font-face { font-family: "Inter"; font-style: normal;'
        " font-weight: 100 900; font-display: swap;"
        ' src: url("%sinter.woff2") format("woff2"); }'
        ' @font-face { font-family: "Fraunces"; font-style: normal;'
        " font-weight: 100 900; font-display: swap;"
        ' font-variation-settings: "SOFT" 55, "WONK" 0;'
        ' src: url("%sfraunces.woff2") format("woff2"); }'
    ) % (font_base, font_base)

# The button, its transcript line and its state line. `phx-hook` is what makes
# the client below run against this element; the mount rides as a data attribute,
# so the page needs no configuration of its own.
MIC_TEMPLATE = (
    '<div class="display-mic" id="display-mic" phx-hook="DisplayMic"'
    ' data-mount="{{mount}}">'
    '<button type="button" class="display-mic-button" aria-pressed="false">'
    "hold to talk</button>"
    '<span class="display-mic-line" data-role="transcript"></span>'
    '<span class="display-mic-state" data-role="state"></span>'
    "</div><script>{{&client_js}}</script>"
)

# The browser half, in plain browser APIs: no library, no CDN, nothing installed.
#
# It rides in the component as a raw prop, the pattern `colony-view` uses, and
# that is not decoration: the script runs in the DEAD render, before the shell's
# socket constructor reads `window.SurfaceHooks`, and a morph never re-runs it. A
# hook registered any later would never be found.
#
# What it does: takes the socket the page already holds, joins `voice:<call>` on
# it, reads the `hello` push for the two rates it has to adapt to, cuts 20 ms
# PCM16 frames out of the microphone in an `AudioWorklet` and pushes them as
# binary with an empty `ref`, and schedules what comes back gap-free through an
# `AudioContext` at the rate that was declared. The counters on
# `window.__displayMic` are there to be read by a test driving a real browser.
#
# Three things it does NOT hide. Without a secure context there is no microphone
# at all, and the button says so instead of failing silently; a REFUSED microphone
# says so too, because an unhandled rejection there left the button doing nothing
# and looking fine. And the worklet's decimation is nearest-sample: adequate for
# speech into a recogniser at 16 kHz from a 48 kHz device, and what the built-in
# test page does. It is not a filter.
#
# Two things it undoes. The playback context is built when `hello` names its rate,
# which is not a gesture -- so the first press resumes it, because a context that
# started suspended has a clock that does not run and the gap-free scheduling
# would schedule against it. And `destroyed()` gives every life back: the two
# window listeners, the microphone's tracks, both contexts and the channel.
# LiveView re-mounts a hook after a reconnect, and a wall screen reconnects all
# day.
MIC_CLIENT_JS = (
    "(function (root) {\n"
    "  var hook = {\n"
    "    mounted: function () {\n"
    "      var el = this.el, mount = el.dataset.mount || \"voice\";\n"
    "      var st = { sent: 0, played: 0, turns: 0, speakEnd: 0, hello: null, code: null };\n"
    "      root.__displayMic = st;\n"
    "      var line = el.querySelector('[data-role=\"transcript\"]'), state = el.querySelector('[data-role=\"state\"]'), btn = el.querySelector(\"button\");\n"
    "      var socket = root.SurfaceSocket && root.SurfaceSocket.getSocket && root.SurfaceSocket.getSocket();\n"
    "      if (!socket) { state.textContent = \"no socket\"; return; }\n"
    "      var call = \"\", topic = \"\", chan = null, joined = false, joinWait = null;\n"
    "      var ctx = null, mctx = null, worklet = null, stream = null, playAt = 0;\n"
    "      function frame(obj) { chan.push(\"frame\", obj); }\n"
    "      function onFrame(f) {\n"
    "        if (f.type === \"hello\") {\n"
    "          st.hello = f;\n"
    "          // A rejoin says hello again. Closing the old context first is what keeps\n"
    "          // a screen that reconnects all day from running into the browser's own\n"
    "          // cap on how many a page may have.\n"
    "          if (ctx) { try { ctx.close(); } catch (e) { /* already closed */ } }\n"
    "          ctx = new (root.AudioContext || root.webkitAudioContext)({ sampleRate: f.audio_out ? f.audio_out.sample_rate : 24000 });\n"
    "          state.textContent = idleText();\n"
    "        }\n"
    "        else if (f.type === \"partial\") { line.textContent = f.text; }\n"
    "        else if (f.type === \"turn\") { st.turns++; line.textContent = f.text; }\n"
    "        else if (f.type === \"speak_start\") { playAt = 0; }\n"
    "        else if (f.type === \"speak_end\") { st.speakEnd++; }\n"
    "        else if (f.type === \"error\") { state.textContent = f.code; }\n"
    "      }\n"
    "      function onAudio(buf) {\n"
    "        if (!ctx) return;\n"
    "        var pcm = new Int16Array(buf), f32 = new Float32Array(pcm.length);\n"
    "        for (var i = 0; i < pcm.length; i++) f32[i] = pcm[i] / 32768;\n"
    "        var b = ctx.createBuffer(1, f32.length, ctx.sampleRate); b.getChannelData(0).set(f32);\n"
    "        var src = ctx.createBufferSource(); src.buffer = b; src.connect(ctx.destination);\n"
    "        var t = Math.max(ctx.currentTime + 0.02, playAt); src.start(t); playAt = t + b.duration; st.played++;\n"
    "      }\n"
    "      // A close is the end of a CALL, never of the button: a wall screen is\n"
    "      // looked at all day and the next press is a new call. Only a refused\n"
    "      // join is final -- that one is an answer about the screen itself.\n"
    "      function onClose(c) { st.code = c.code; state.textContent = \"closed \" + c.code; joined = false; }\n"
    "      // The line under the button when nothing is happening: what the cell\n"
    "      // said it is made of, and an empty line before it has said anything.\n"
    "      function idleText() { return st.hello ? st.hello.stt + (st.hello.tts ? \"/\" + st.hello.tts : \"\") : \"\"; }\n"
    "      // The join is AWAITED, and that is not politeness: a text push is\n"
    "      // buffered by the client until the join is acknowledged, a raw binary\n"
    "      // push is not. Audio sent between `join()` and its `ok` would either\n"
    "      // be dropped or arrive at the cell BEFORE the hold that frames it.\n"
    "      // One join at a time: a press during a join in flight waits for that\n"
    "      // one instead of opening a second channel on the same socket.\n"
    "      function join() {\n"
    "        if (joinWait) return joinWait;\n"
    "        call = \"c\" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);\n"
    "        topic = \"voice:\" + call;\n"
    "        chan = socket.channel(topic, { mount: mount, mode: \"hold\", sample_rate: 16000 });\n"
    "        chan.on(\"frame\", onFrame); chan.on(\"audio\", onAudio); chan.on(\"close\", onClose);\n"
    "        state.textContent = \"joining\\u2026\";\n"
    "        joinWait = new Promise(function (done) {\n"
    "          chan.join()\n"
    "            .receive(\"ok\", function () { joined = true; joinWait = null; state.textContent = idleText(); done(true); })\n"
    "            .receive(\"error\", function (e) { state.textContent = (e && e.reason) || \"refused\"; btn.disabled = true; joined = false; joinWait = null; done(false); })\n"
    "            .receive(\"timeout\", function () { state.textContent = \"the screen never answered\"; joined = false; joinWait = null; done(false); });\n"
    "        });\n"
    "        return joinWait;\n"
    "      }\n"
    "      join();\n"
    "      function sendAudio(ab) { if (!joined) return; socket.push({ topic: topic, event: \"audio\", payload: ab, ref: \"\", join_ref: chan.joinRef() }); st.sent++; }\n"
    "      var WORKLET = \"class P extends AudioWorkletProcessor{constructor(o){super();this.rate=o.processorOptions.rate;this.acc=[];this.pos=0}process(i){var ch=i[0]&&i[0][0];if(!ch)return true;var r=sampleRate/this.rate;for(var k=0;k<ch.length;k+=r){this.acc.push(Math.max(-1,Math.min(1,ch[Math.floor(k)])))}var n=Math.floor(this.rate/50);while(this.acc.length>=n){var out=new Int16Array(n);for(var j=0;j<n;j++)out[j]=this.acc[j]*32767;this.acc=this.acc.slice(n);this.port.postMessage(out.buffer,[out.buffer])}return true}}registerProcessor('mic',P);\";\n"
    "      async function openMic() {\n"
    "        if (!root.isSecureContext) { state.textContent = \"microphone needs https or localhost\"; return false; }\n"
    "        // The permission prompt opens INSIDE the gesture, and a person who is\n"
    "        // being asked is looking at a button that does nothing. Saying so is\n"
    "        // the difference between waiting and a screen that is broken.\n"
    "        state.textContent = \"asking for the microphone\\u2026\";\n"
    "        try {\n"
    "          stream = await navigator.mediaDevices.getUserMedia({ audio: true });\n"
    "        } catch (e) {\n"
    "          // A refused or missing microphone is the ordinary case, not a crash:\n"
    "          // an unhandled rejection here left the button doing nothing at all,\n"
    "          // which is the silence this line exists against.\n"
    "          state.textContent = \"microphone refused\";\n"
    "          return false;\n"
    "        }\n"
    "        var rate = (st.hello && st.hello.audio_in && st.hello.audio_in.sample_rate) || 16000;\n"
    "        mctx = new (root.AudioContext || root.webkitAudioContext)();\n"
    "        try {\n"
    "          await mctx.audioWorklet.addModule(URL.createObjectURL(new Blob([WORKLET], { type: \"text/javascript\" })));\n"
    "          var src = mctx.createMediaStreamSource(stream);\n"
    "          worklet = new AudioWorkletNode(mctx, \"mic\", { processorOptions: { rate: rate } });\n"
    "        } catch (e) {\n"
    "          state.textContent = \"no audio worklet\";\n"
    "          return false;\n"
    "        }\n"
    "        worklet.port.onmessage = function (e) { if (holding) sendAudio(e.data); };\n"
    "        src.connect(worklet); return true;\n"
    "      }\n"
    "      var holding = false, pressed = false;\n"
    "      async function down() {\n"
    "        if (holding || btn.disabled) return;\n"
    "        pressed = true;\n"
    "        if (!joined && !(await join())) return;\n"
    "        // The playback context was built on join, which is not a gesture: under\n"
    "        // an autoplay policy it starts suspended and its clock does not run, so\n"
    "        // the gap-free scheduling would schedule against a stopped clock. This is\n"
    "        // the first gesture there is, so it is where it gets resumed.\n"
    "        if (ctx && ctx.state === \"suspended\") { try { await ctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (mctx && mctx.state === \"suspended\") { try { await mctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (!worklet && !(await openMic())) return;\n"
    "        // Let go while the browser was still asking: the microphone is open\n"
    "        // now and the gesture is over, so the next press is a whole take.\n"
    "        if (!pressed) { state.textContent = \"press again\"; return; }\n"
    "        // And the waiting sentence goes: a screen that still says it is\n"
    "        // asking for the microphone, minutes after it got one, is the same\n"
    "        // kind of lie as a screen that said nothing at all.\n"
    "        holding = true; btn.setAttribute(\"aria-pressed\", \"true\"); state.textContent = \"listening\\u2026\"; frame({ type: \"hold\" });\n"
    "      }\n"
    "      function up() { pressed = false; if (!holding) return; holding = false; btn.setAttribute(\"aria-pressed\", \"false\"); state.textContent = idleText(); frame({ type: \"release\" }); }\n"
    "      // A display is a surface other people's components render onto, so the\n"
    "      // window-wide key must keep its hands off their controls -- and off the\n"
    "      // button once a refused join disabled it.\n"
    "      function typing(e) {\n"
    "        var t = e.target;\n"
    "        if (!t || t === root || t === document.body) return false;\n"
    "        var tag = (t.tagName || \"\").toLowerCase();\n"
    "        return tag === \"input\" || tag === \"textarea\" || tag === \"select\" || t.isContentEditable === true;\n"
    "      }\n"
    "      function keydown(e) { if (e.code !== \"Space\" || e.repeat || btn.disabled || typing(e)) return; e.preventDefault(); down(); }\n"
    "      function keyup(e) { if (e.code !== \"Space\" || typing(e)) return; e.preventDefault(); up(); }\n"
    "      btn.addEventListener(\"pointerdown\", down); btn.addEventListener(\"pointerup\", up); btn.addEventListener(\"pointerleave\", up);\n"
    "      root.addEventListener(\"keydown\", keydown);\n"
    "      root.addEventListener(\"keyup\", keyup);\n"
    "      st.down = down; st.up = up; st.cancel = function () { frame({ type: \"cancel\" }); };\n"
    "      // What a re-mount has to undo. LiveView re-mounts a hook after a reconnect,\n"
    "      // and without this the listeners, the microphone and the contexts of every\n"
    "      // previous life stay open.\n"
    "      this.__displayMicTeardown = function () {\n"
    "        root.removeEventListener(\"keydown\", keydown);\n"
    "        root.removeEventListener(\"keyup\", keyup);\n"
    "        holding = false; pressed = false; joined = false; joinWait = null;\n"
    "        if (stream) { stream.getTracks().forEach(function (t) { t.stop(); }); stream = null; }\n"
    "        if (worklet) { try { worklet.port.onmessage = null; worklet.disconnect(); } catch (e) { /* gone */ } worklet = null; }\n"
    "        if (mctx) { try { mctx.close(); } catch (e) { /* gone */ } mctx = null; }\n"
    "        if (ctx) { try { ctx.close(); } catch (e) { /* gone */ } ctx = null; }\n"
    "        if (chan) { try { chan.leave(); } catch (e) { /* the socket may be gone already */ } chan = null; }\n"
    "      };\n"
    "    },\n"
    "    destroyed: function () {\n"
    "      if (this.__displayMicTeardown) { this.__displayMicTeardown(); this.__displayMicTeardown = null; }\n"
    "    }\n"
    "  };\n"
    "  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, { DisplayMic: hook });\n"
    "})(window);\n"
)


def _c(name, template, props, layer="content"):
    """One `component.define` argument bag of the catalogue.

    `editable` is empty on every component of the catalogue: a prop a browser
    may write is an authorisation, and nothing on these screens is dragged.
    """
    return {
        "name": name,
        "template": template,
        "prop_schema": props,
        "editable": [],
        "layer": layer,
    }


def windows():
    """Catalogue A: the four glass components, all `layer: "navigation"`.

    The ornament is a thing, not a place: a component an application hangs
    into its view, which the sheet fixes to the bottom edge. The two regions
    stay the only places on this screen.
    """
    return [
        _c("display-pane", PANE_TEMPLATE, {
            "pane_id": "text", "state": "text", "age": "text",
            "kicker": "text", "title": "text", "region": "text",
            "thin": "boolean",
        }, "navigation"),
        _c("display-panel", PANEL_TEMPLATE, {
            "pane_id": "text", "state": "text", "age": "text",
            "title": "text", "scroll": "boolean",
        }, "navigation"),
        _c("display-overlay", OVERLAY_TEMPLATE, {
            "pane_id": "text", "state": "text", "title": "text",
            "body": "text", "ttl_ms": "int", "position": "text",
        }, "navigation"),
        _c("display-ornament", ORNAMENT_TEMPLATE, {
            "text": "text", "dot": "boolean", "count": "int",
        }, "navigation"),
    ]


def contents():
    """Catalogue B: the twenty-two content components, all `layer: "content"`.

    None of them writes glass: a content component sits on a window's
    `.inner` fill, and the `web` cell refuses the material on this layer.
    """
    return [
        _c("display-value", VALUE_TEMPLATE, {
            "value": "text", "unit": "text", "label": "text",
            "trend": "text", "size": "text"}),
        _c("display-text", TEXT_TEMPLATE, {"body": "text", "secondary": "boolean"}),
        _c("display-voice", VOICE_TEMPLATE, {
            "text": "text", "accent": "text", "size": "text"}),
        _c("display-kicker", KICKER_TEMPLATE, {"text": "text"}),
        _c("display-list", LIST_TEMPLATE, {"title": "text", "plain": "boolean"}),
        _c("display-item", ITEM_TEMPLATE, {
            "k": "text", "v": "text", "marker": "text", "accent": "boolean"}),
        _c("display-table", TABLE_TEMPLATE, {
            "caption": "text", "head": "html", "rows": "html"}),
        _c("display-weather", WEATHER_TEMPLATE, {
            "temp": "text", "unit": "text", "condition": "text",
            "glyph": "text", "hi": "text", "lo": "text", "place": "text"}),
        _c("display-clock", CLOCK_TEMPLATE, {
            "time": "text", "date": "text", "zone": "text", "size": "text"}),
        _c("display-timer", TIMER_TEMPLATE, {
            "label": "text", "remaining": "text", "end_at": "int",
            "total_ms": "int", "done": "boolean"}),
        _c("display-chat", CHAT_TEMPLATE, {"title": "text"}),
        _c("display-chat-line", CHAT_LINE_TEMPLATE, {
            "role": "text", "text": "text", "time": "text",
            "partial": "boolean"}),
        _c("display-notification", NOTIFICATION_TEMPLATE, {
            "source": "text", "title": "text", "body": "text",
            "time": "text", "level": "text"}),
        _c("display-media", MEDIA_TEMPLATE, {
            "src": "text", "alt": "text", "caption": "text",
            "figure": "html", "ratio": "text"}),
        _c("display-document", DOCUMENT_TEMPLATE, {
            "title": "text", "body": "html", "page": "int",
            "pages": "int", "source": "text"}),
        _c("display-status", STATUS_TEMPLATE, {"kind": "text", "text": "text"}),
        _c("display-action", ACTION_TEMPLATE, {
            "label": "text", "event": "text", "primary": "boolean"}),
        _c("display-choice", CHOICE_TEMPLATE, {"label": "text"}),
        _c("display-option", OPTION_TEMPLATE, {
            "label": "text", "event": "text", "selected": "boolean"}),
        _c("display-chart", CHART_TEMPLATE, {"figure": "html", "caption": "text"}),
        _c("display-stack", STACK_TEMPLATE, {
            "row": "boolean", "gap": "text", "scene": "boolean",
            "ratio": "text"}),
        _c("display-progress", PROGRESS_TEMPLATE, {
            "value": "int", "label": "text"}),
    ]


def components():
    """The display's own components, in the order they are defined.

    First the five the screen is made of, then the catalogue: the windows and
    the content an application may name without defining them. A component
    has to exist before an object names it, and the legs of a bundle run in
    call order.

    None of them is `editable`. A prop a browser may write is an authorisation
    an application grants over its OWN component; the frame around it is not a
    thing anybody drags.
    """
    return own() + windows() + contents()


def own():
    """The five the screen is made of: shell, regions, two wrappers, the mic."""
    return [
        {
            # `faces` is typed `"html"` because that is what makes a prop RAW:
            # an `@font-face` block rendered escaped is not an `@font-face`.
            "name": "display-shell",
            "template": SHELL_TEMPLATE,
            # `vocab` is never rendered: the template does not name it. It is a
            # note the root carries about which vocabulary it was built against
            # (see VOCAB), so a later pass can read it back.
            "prop_schema": {"stylesheet": "boolean", "faces": "html", "vocab": "text"},
            "editable": [],
            "layer": "content",
        },
        {
            # One per entry in REGIONS, all of them direct children of the
            # root. That USED to be impossible: a materialised page carried two
            # statics whatever the child count, so the closing static landed
            # between the first and the second child and everything from the
            # second on rendered outside the element meant to contain it. GH
            # #394 replaced that with n+1 statics for n slots, and the `web`
            # README says so in as many words -- "a composition CHOICE now
            # rather than a constraint". So the one-child rule is retracted
            # here too, and the two regions stand side by side (GH #609).
            "name": "display-region",
            "template": REGION_TEMPLATE,
            "prop_schema": {"region": "text"},
            "editable": [],
            "layer": "content",
        },
        {
            # Navigation, because it writes `glass--thin`, and glass is a
            # navigation-layer material. A `layer: "content"` component that
            # names one of the three glass classes is refused by the `web` cell
            # at `component.define`.
            "name": "display-view-prose",
            "template": PROSE_TEMPLATE,
            "prop_schema": {
                "view_id": "text",
                "owner": "text",
                "title": "text",
                "body": "text",
            },
            "editable": [],
            "layer": "navigation",
        },
        {
            # Content, and that is the load-bearing half: an application's own
            # glass card sits INSIDE this wrapper, and glass on glass is refused
            # where the edge is made. Glass on a content parent is allowed, so a
            # content wrapper is what lets an app bring its own pane.
            "name": "display-view-custom",
            "template": CUSTOM_TEMPLATE,
            "prop_schema": {"view_id": "text", "owner": "text"},
            "editable": [],
            "layer": "content",
        },
        {
            # The microphone (GH #643). `client_js` is typed `"html"` because
            # that is what makes a prop RAW: a script rendered escaped is a
            # script that does nothing. Everything else on this screen stays
            # escaped, and `mount` with it -- a mount name is configuration and
            # must not be able to close a tag.
            "name": "display-mic",
            "template": MIC_TEMPLATE,
            "prop_schema": {"mount": "text", "client_js": "html"},
            "editable": [],
            "layer": "content",
        },
    ]


# The fingerprint of the vocabulary: `components()` as canonical JSON, hashed,
# cut to twelve hex characters. Computed ONCE, at import, because the list is
# a constant of the code that runs -- a screen whose root carries another
# fingerprint was built by other code, and gets every definition again
# (`patches()`). Twelve characters are 48 bits: enough that two vocabularies
# never collide by accident, short enough to read in a prop (GH #670).
VOCAB = hashlib.sha256(
    json.dumps(components(), sort_keys=True).encode("utf-8")
).hexdigest()[:12]


# ---------------------------------------------------------------------------
# Wire helpers


def now_ms():
    """Epoch milliseconds. `updated_at` is an int and never a formatted date."""
    return int(time.time() * 1000)


def tool_call(args, tid):
    """One operation, as a UBF `tool_call` turn."""
    return {
        "origin": "assistant",
        "type": "tool_call",
        "id": tid,
        "text": json.dumps(args, sort_keys=True),
    }


def text_turn(text):
    """One plain turn. `messages` is mandatory on every body that crosses the
    substrate: a body without it is refused as `invalid_ubf_body` before it
    reaches an edge, which shows up as a dead letter rather than as an answer.
    """
    return {"origin": "assistant", "type": "text", "text": text}


def emission(route, body, **header):
    """One message on `route`, carrying `body`'s slots."""
    head = {"route": route}
    head.update(header)
    out = {"header": head}
    out.update(body)
    return out


def canon(value):
    """One byte form for a JSON value, whatever side it came from.

    A `json` column is stored as TEXT and read back as a string, while a value
    this cell just built is still a Python object. Comparing the two directly
    would report a difference that is only a serialisation, which is exactly
    the comparison the component dedup rests on.
    """
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except (TypeError, ValueError):
            return value
    return json.dumps(value, sort_keys=True)


def refuse(code, detail, view_id, owner):
    """ONE emission on the `receipt` lane, and nothing else.

    All four keys are always present, empty where unknown: the member that
    routes a receipt back reads `owner`, and a key that is sometimes missing is
    a router branch nobody tests.

    The same two keys ride on the HOP as well, and that is not a duplication for
    convenience: an edge condition in this substrate sees `context.*` and
    `hop.*` and nothing else (`crates/meclaw-colony/src/cel_eval.rs`, `bind_ctx`),
    so a receipt whose owner lives only in the body cannot be routed back to
    that owner at all. The body copy is what the receiving cell reads; the hop
    copy is what the member's graph reads (GH #459).
    """
    return [
        emission(
            "receipt",
            {
                "messages": [text_turn("%s: %s" % (code, detail))],
                "receipt": {
                    "error_code": code,
                    "view_id": view_id or "",
                    "owner": owner or "",
                    "detail": detail,
                },
            },
            owner=owner or "",
            view_id=view_id or "",
        )
    ]


# ---------------------------------------------------------------------------
# Pass 1: a request


def is_view_id(value):
    """`[a-z0-9-]{1,64}`, and nothing looser.

    The id ends up inside an object id and inside a `data-view` attribute, so
    the set is closed at the door rather than escaped at every use site.
    """
    return (
        isinstance(value, str)
        and 1 <= len(value) <= ID_MAX
        and all(c in ID_CHARS for c in value)
    )


def is_node_key(value):
    """Whether a component-tree node may name its own identity with this.

    A key stands where the child index would stand, so it must not carry the
    separator the index chain is written with, and it must not be empty -- an
    id with an empty segment is two different ids depending on who splits it.
    Everything else a path can hold is allowed, because the keys that matter
    are paths: `parse_object_id` only ever reads the FIRST segment of an id, so
    a `~` or a `.` further along is already in the language.
    """
    return (
        isinstance(value, str)
        and 1 <= len(value) <= KEY_MAX
        and "/" not in value
    )


def check_node(node, depth):
    """A component-tree node, or the reason it is not one."""
    if depth > MAX_DEPTH:
        return "the component tree is deeper than %d levels" % MAX_DEPTH
    if not isinstance(node, dict):
        return "a component tree node is not an object"
    name = node.get("component")
    if not isinstance(name, str) or not name:
        return 'a component tree node carries no "component" name'
    if not isinstance(node.get("props", {}), dict):
        return '"props" is not an object'
    keep = node.get("keep", [])
    if not isinstance(keep, list) or not all(isinstance(k, str) for k in keep):
        return '"keep" is not a list of prop names'
    key = node.get("key")
    if key is not None and not is_node_key(key):
        return '"key" is not a usable object key'
    kids = node.get("children", [])
    if not isinstance(kids, list):
        return '"children" is not a list'
    # A key stands where the index would stand, so two siblings naming the same
    # one mint the same object id and the second would simply overwrite the
    # first -- an accepted view with a node missing from the screen and nothing
    # in the receipt to act on (GH #568). A key that is a plain number is the
    # same collision one door along: `"3"` names exactly what the unkeyed
    # fourth child beside it names. Both are refused here, where the sender is
    # still being told why.
    seen = set()
    for kid in kids:
        why = check_node(kid, depth + 1)
        if why:
            return why
        kid_key = kid.get("key") if isinstance(kid, dict) else None
        if not is_node_key(kid_key):
            continue
        if kid_key.isdigit():
            return (
                '"key" %r is a number and would collide with the index language'
                % (kid_key,)
            )
        if kid_key in seen:
            return '"key" %r collides with a sibling under %s' % (kid_key, name)
        seen.add(kid_key)
    return None


def check_components(declared, view_id):
    """The `component.define` arguments a view brings, or the reason they fail.

    Every name must start with `<view_id>-`. The component library of a display
    is ONE namespace shared by every application writing to that screen, so a
    prefix is what keeps two apps from redefining each other's vocabulary out
    from under a page that is already rendered.
    """
    if not isinstance(declared, list):
        return None, "invalid_view", '"components" is not a list'
    out = []
    for item in declared:
        if not isinstance(item, dict):
            return None, "invalid_view", "a component definition is not an object"
        name = item.get("name")
        if not isinstance(name, str) or not name:
            return None, "invalid_view", 'a component definition carries no "name"'
        if not name.startswith(view_id + "-"):
            return (
                None,
                "component_prefix",
                "the component %r does not start with %r" % (name, view_id + "-"),
            )
        if not isinstance(item.get("template"), str):
            return None, "invalid_view", 'the component %r has no "template"' % name
        if not isinstance(item.get("prop_schema"), dict):
            return None, "invalid_view", 'the component %r has no "prop_schema"' % name
        out.append(
            dict(
                (k, item[k])
                for k in ("name", "template", "prop_schema", "editable", "layer")
                if k in item
            )
        )
    return out, None, None


def declared_ord(view):
    """The `ord` a view asked for, or 0.

    A BAND rather than a slot: a standing widget asks for -10 and stands above
    a conversation that asked for nothing, and two views in one band are still
    ordered by everything after it. Anything that is not a plain integer counts
    as 0 here; the door refuses it outright, and this is the reading for a row
    that is already in the table.
    """
    value = view.get("ord")
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return value


def validate(body, owner, withdraw):
    """`(row, error_code, detail)` -- exactly one of the first and the second."""
    view_id = body.get("view_id")
    if not is_view_id(view_id):
        return None, "invalid_view", '"view_id" must match [a-z0-9-]{1,64}'

    # The body may repeat the owner. It may not disagree with the envelope: a
    # sender that could name somebody else's owner could withdraw their views.
    claimed = body.get("owner")
    if claimed is not None and claimed != owner:
        return (
            None,
            "not_owner",
            "the body claims owner %r, the sender is %r" % (claimed, owner),
        )

    if withdraw:
        return {"owner": owner, "view_id": view_id}, None, None

    region = body.get("region")
    if region is None:
        region = REGIONS[0]
    if region not in REGIONS:
        return None, "invalid_view", "unknown region %r" % (region,)

    kind = body.get("kind")
    if kind not in ("prose", "component"):
        return None, "invalid_view", 'unknown kind %r ("prose" or "component")' % (kind,)

    content = body.get("content")
    if not isinstance(content, dict):
        return None, "invalid_view", '"content" is not an object'

    declared = body.get("components")
    if kind == "prose":
        if not isinstance(content.get("body"), str):
            return None, "invalid_view", 'a prose view needs a "body" string'
        title = content.get("title")
        if title is not None and not isinstance(title, str):
            return None, "invalid_view", 'a prose "title" is not a string'
        if declared:
            return None, "invalid_view", "a prose view brings no components"
        clean = []
    else:
        why = check_node(content, 0)
        if why:
            return None, "invalid_view", why
        clean, code, detail = check_components(
            declared if declared is not None else [], view_id
        )
        if code:
            return None, code, detail

    ttl_ms = body.get("ttl_ms")
    if ttl_ms is None:
        ttl_ms = 0
    if isinstance(ttl_ms, bool) or not isinstance(ttl_ms, int) or ttl_ms < 0:
        return None, "invalid_view", '"ttl_ms" is not a non-negative integer'

    # Signed and unbounded on purpose: it is compared, never used as an index,
    # and a widget that wants to stand above everything says so with a negative
    # number instead of asking every other sender to move down.
    view_ord = body.get("ord")
    if view_ord is None:
        view_ord = 0
    if isinstance(view_ord, bool) or not isinstance(view_ord, int):
        return None, "invalid_view", '"ord" is not an integer'

    return (
        {
            "owner": owner,
            "view_id": view_id,
            "region": region,
            "ord": view_ord,
            "kind": kind,
            # The two `json` columns are written as canonical text so the value
            # that comes back out of the store compares byte for byte against
            # the value that went in.
            "content": canon(content),
            "components": canon(clean),
            "ttl_ms": ttl_ms,
            "updated_at": now_ms(),
        },
        None,
        None,
    )


def pass_request(body, envelope, withdraw):
    """A write lane: validate, then ONE store bundle."""
    owner = envelope.get("reply_to")
    if not isinstance(owner, str) or not owner:
        return refuse(
            "owner_unknown",
            "the message carries no envelope.reply_to, so it has no owner",
            body.get("view_id") if isinstance(body.get("view_id"), str) else "",
            "",
        )

    row, code, detail = validate(body, owner, withdraw)
    if code:
        vid = body.get("view_id")
        return refuse(code, detail, vid if isinstance(vid, str) else "", owner)

    view_id = row["view_id"]
    legs = [
        # Leg 0 is the before-state, and it has to be read before leg 1 removes
        # the row it describes. A bundle is not a transaction, but its legs do
        # run in call order.
        tool_call(
            {"operation": "select", "table": TABLE, "columns": COLUMNS},
            "d-select",
        ),
        tool_call(
            {
                "operation": "delete",
                "table": TABLE,
                "where": {"owner": owner, "view_id": view_id},
            },
            "d-delete",
        ),
    ]
    request = {"withdraw": withdraw, "owner": owner, "view_id": view_id}
    if not withdraw:
        legs.append(
            tool_call({"operation": "insert", "table": TABLE, "row": row}, "d-insert")
        )
        request["row"] = row

    return [
        emission(
            "views",
            {"messages": legs},
            display_request=json.dumps(request, sort_keys=True),
        )
    ]


def parse_object_id(oid):
    """`(owner, view_id)` out of an object id, or `(None, None)`.

    The inverse of how an id is built: `view.<slug>.<view_id>/<i>/<j>`, where
    the slug is the owner path with `/` written as `~` -- a path segment inside
    an id would otherwise be indistinguishable from the child index chain.
    """
    if not isinstance(oid, str) or not oid.startswith(VIEW_PREFIX):
        return None, None
    wrapper = oid.split("/")[0]
    rest = wrapper[len(VIEW_PREFIX) :]
    if "." not in rest:
        return None, None
    slug, view_id = rest.rsplit(".", 1)
    if not slug or not is_view_id(view_id):
        return None, None
    return slug.replace("~", "/"), view_id


def event_object_id(event):
    """The object id a browser event names, preferring the key `id`."""
    value = event.get("value")
    if isinstance(value, str):
        return value if value.startswith(VIEW_PREFIX) else None
    if not isinstance(value, dict):
        return None
    candidates = []
    if isinstance(value.get("id"), str):
        candidates.append(value["id"])
    for key in sorted(value):
        if key != "id" and isinstance(value[key], str):
            candidates.append(value[key])
    for candidate in candidates:
        if candidate.startswith(VIEW_PREFIX):
            return candidate
    return None


def pass_event(body):
    """A semantic browser event, handed out of the hive with its addressee.

    An event whose id will not parse still leaves. A view the display holds and
    this cell cannot attribute is a defect somebody has to see; dropping the
    event would make it invisible, while a message with no `owner` dead-letters
    where a person can read it.

    `owner` and `view_id` leave on the HOP as well as in the body, always
    present and empty where the id would not parse: the member routes the event
    back to whoever owns the view, and an edge condition can only read
    `context.*` and `hop.*` (GH #459). An empty `owner` therefore fails every
    owner guard by construction and the event dead-letters, which is the
    behaviour this scope wanted in the first place.
    """
    event = body.get("event")
    if not isinstance(event, dict):
        return []
    out = {
        "messages": body.get("messages") or [text_turn(str(event.get("name") or ""))],
        "event": event,
    }
    owner, view_id = parse_object_id(event_object_id(event))
    if owner is not None:
        out["owner"] = owner
        out["view_id"] = view_id
    return [
        emission(
            "event",
            out,
            owner=owner or "",
            view_id=view_id or "",
        )
    ]


# ---------------------------------------------------------------------------
# Pass 2: the store answered


def bundle_failed(body, hop):
    """The leg that failed, or None.

    Every leg is checked, leg 0 included: a select that could not run leaves
    this cell with no before-state, and computing an after-state out of nothing
    would silently blank the screen.
    """
    if hop.get("error_code"):
        return str(hop["error_code"])
    for entry in body.get("results") or []:
        if isinstance(entry, dict) and entry.get("error_code"):
            return "%s on %s" % (entry["error_code"], entry.get("operation") or "?")
    return None


def read_rows(body):
    """The rows of leg 0, or None when the reply does not carry any."""
    msgs = body.get("messages") or []
    if not msgs:
        return None
    try:
        doc = json.loads(str(msgs[0].get("text") or ""))
    except (TypeError, ValueError, AttributeError):
        return None
    if not isinstance(doc, list):
        return None
    return [r for r in doc if isinstance(r, dict)]


def expired(row, now):
    """A view is expired when `now - updated_at >= ttl_ms`, and `ttl_ms` is set.

    Nothing sweeps: an expired row stays in the table and simply stops being
    drawn. The next compose is what makes it disappear from the screen.
    """
    try:
        ttl = int(row.get("ttl_ms") or 0)
        written = int(row.get("updated_at") or 0)
    except (TypeError, ValueError):
        return False
    return ttl > 0 and now - written >= ttl


def pass_views(body, ctx, hop):
    """The store's answer: compute the after-state, then ask the display."""
    try:
        request = json.loads(str(ctx.get("display_request") or ""))
    except (TypeError, ValueError):
        request = None
    if not isinstance(request, dict):
        return []

    owner = str(request.get("owner") or "")
    view_id = str(request.get("view_id") or "")
    row = request.get("row") if isinstance(request.get("row"), dict) else None

    why = bundle_failed(body, hop)
    if why:
        return refuse("store_failed", why, view_id, owner)

    before = read_rows(body)
    if before is None:
        return refuse(
            "store_failed", "the store's reply carried no rows for leg 0", view_id, owner
        )

    prior = None
    after = []
    for old in before:
        if str(old.get("owner") or "") == owner and str(old.get("view_id") or "") == view_id:
            prior = old
            continue
        after.append(old)
    if row is not None:
        after.append(row)

    now = now_ms()
    live = [r for r in after if not expired(r, now)]
    # A `select` without `order_by` is explicitly an unspecified selection, so
    # the determinism has to be made here -- and it is made WITHOUT a clock:
    # region, the band the view asked for, then identity. Sorting on
    # `updated_at` was GH #609 itself, and it is not a step this list takes any
    # more; the seats that decide what a person sees are read one pass later,
    # off the display, in `seated`.
    live.sort(
        key=lambda r: (
            REGION_INDEX.get(str(r.get("region") or REGIONS[0]), 0),
            declared_ord(r),
            str(r.get("owner") or ""),
            str(r.get("view_id") or ""),
        )
    )

    # The vocabulary only travels when it CHANGED. A `component.define`
    # re-renders every route in the display, so an app that ticks once a second
    # and re-sends the same definitions would re-render the whole screen once a
    # second for no difference at all.
    define = []
    if row is not None:
        if prior is None or canon(prior.get("components")) != row["components"]:
            parsed = json.loads(row["components"])
            define = parsed if isinstance(parsed, list) else []

    plan = {"views": live, "define": define}
    return [
        emission(
            "read",
            {"messages": [tool_call({"op": "query", "route": PAGE_ROUTE}, "d-query")]},
            display_views=json.dumps(plan, sort_keys=True),
        )
    ]


# ---------------------------------------------------------------------------
# Pass 3: the display answered


def read_objects(body):
    """What the display holds, or None when the answer was not a `query` one.

    `{id: {"props": …, "parent": …, "ord": …}}` -- the place an object sits is
    read back as well as its props, because the order of the screen lives in
    `ord` and an `object.update` cannot move anything.
    """
    msgs = body.get("messages") or []
    if not msgs:
        return None
    try:
        doc = json.loads(str(msgs[-1].get("text") or ""))
    except (TypeError, ValueError, AttributeError):
        return None
    if not isinstance(doc, dict) or not isinstance(doc.get("objects"), list):
        return None
    out = {}
    for obj in doc["objects"]:
        if isinstance(obj, dict) and obj.get("id"):
            props = obj.get("props")
            try:
                ord_ = int(obj.get("ord") or 0)
            except (TypeError, ValueError):
                ord_ = 0
            out[str(obj["id"])] = {
                "props": props if isinstance(props, dict) else {},
                "parent": obj.get("parent"),
                "ord": ord_,
            }
    return out


def add_tree(want, parent, node, index):
    """One component-tree node and everything under it, as objects.

    The id is the index chain in `children` order, which makes it a function of
    the tree alone: the same tree sent twice patches the same objects, and a
    node that moved is an update rather than a delete plus a create.

    Unless the node names its own `key`, and then the id is that instead of the
    index -- while `ord`, the drawing order, still comes from the index. The
    difference is what a `keep` prop is worth. An index is a SLOT: insert one
    sibling ahead of a node and every id behind it now belongs to a different
    thing, so the props the sender asked to keep are handed to the new occupant
    of the slot. Measured on a running colony under GH #544: a picture whose
    edge count had grown by three had 103 of 104 boxes standing at a position
    that had been computed for some OTHER cell, and three cells held two objects
    each. A key that says WHAT the node is -- a cell path, a row id -- makes the
    kept prop follow the thing, which is the only reading under which `keep`
    means anything at all.
    """
    key = node.get("key")
    oid = "%s/%s" % (parent, key if is_node_key(key) else index)
    want[oid] = {
        "component": str(node.get("component") or ""),
        "parent": parent,
        "ord": index * ORD_STEP,
        "props": dict(node.get("props") or {}),
        "keep": [k for k in (node.get("keep") or []) if isinstance(k, str)],
    }
    for j, kid in enumerate(node.get("children") or []):
        if isinstance(kid, dict):
            add_tree(want, oid, kid, j)


def drawable(views):
    """The views that can be drawn at all, each with its wrapper id.

    A row the screen cannot address -- no owner, an id that is not one, a
    region this version does not have, content that will not parse -- is
    skipped rather than refused. It was refused at the door; a row that got
    past that is a defect somebody has to be able to see the REST of the
    screen through.
    """
    out = []
    for view in views:
        region = str(view.get("region") or REGIONS[0])
        if region not in REGIONS:
            continue
        owner = str(view.get("owner") or "")
        view_id = str(view.get("view_id") or "")
        if not owner or not is_view_id(view_id):
            continue
        content = view.get("content")
        if isinstance(content, str):
            try:
                content = json.loads(content)
            except (TypeError, ValueError):
                continue
        if not isinstance(content, dict):
            continue
        wrapper = "%s%s.%s" % (VIEW_PREFIX, owner.replace("/", "~"), view_id)
        out.append((region, owner, view_id, wrapper, content, view))
    return out


def seat_of(wrapper, region, have):
    """The `ord` the display is already holding this view at, or `NEW_SEAT`.

    A view the screen does not hold, or holds under ANOTHER region, is new
    here: moving a widget from `main` to `aside` puts it at the end of the
    aside rather than at whatever height it happened to have in the column it
    came from.
    """
    held = have.get(wrapper)
    if not isinstance(held, dict) or held.get("parent") != REGION_PREFIX + region:
        return NEW_SEAT
    try:
        return int(held.get("ord") or 0)
    except (TypeError, ValueError):
        return NEW_SEAT


def seated(views, have):
    """`(region, index, row)` for every view, in the order it stands.

    Three keys, and the interesting one is the key that is NOT among them: the
    moment a view was last written does not appear at all. Sorting on it was
    GH #609 -- a view rewritten every twenty seconds took the top slot on every
    tick, not because it was important but because it was recent, which is the
    right answer for a card and the wrong one for anything standing.

    1. the `ord` the view DECLARED, default 0.
    2. the seat the display is already holding it at. That is FIRST APPEARANCE,
       remembered by the screen instead of by a column: a new view sorts behind
       everything already up, is given the next seat, and keeps it through
       every rewrite until something above it goes away. A page this cell has
       to bootstrap has no seats at all, and every view on it is new together.
    3. `(owner, view_id)`, so the one tie left is broken on identity rather
       than on whatever order the store happened to return.
    """
    out = []
    rows = drawable(views)
    for region in REGIONS:
        here = [r for r in rows if r[0] == region]
        here.sort(
            key=lambda r: (declared_ord(r[5]), seat_of(r[3], region, have), r[1], r[2])
        )
        for i, row in enumerate(here):
            out.append((region, i, row))
    return out


def build(views, have=None):
    """Every object the screen should hold, keyed by id.

    `have` is what the display is holding now, and it is an INPUT to the layout
    rather than only something to diff against: it carries the seats, and the
    seats are the order of the screen (see `seated`).
    """
    have = have if isinstance(have, dict) else {}
    want = {
        ROOT_ID: {
            "component": "display-shell",
            "parent": None,
            "ord": 0,
            "props": {"stylesheet": True, "faces": faces(FONT_BASE), "vocab": VOCAB},
            "keep": [],
        }
    }
    # Every region exists whether or not anything is in it: a region is a
    # structural promise, not a consequence of there being views. Their `ord`
    # is the order of the declaration, which is what puts `main` left of
    # `aside` -- two regions at `ord: 0` were the second half of GH #609.
    for i, region in enumerate(REGIONS):
        want[REGION_PREFIX + region] = {
            "component": "display-region",
            "parent": ROOT_ID,
            "ord": i * ORD_STEP,
            "props": {"region": region},
            "keep": [],
        }

    # The microphone, behind the regions and outside both of them: it belongs to
    # the screen and not to a column, and its own stylesheet takes it out of the
    # flow. Written on every tick like a region, because it is structural in the
    # same way -- a screen HAS a microphone, whether or not anybody is talking.
    want[MIC_ID] = {
        "component": "display-mic",
        "parent": ROOT_ID,
        "ord": len(REGIONS) * ORD_STEP,
        "props": {"mount": VOICE_MOUNT, "client_js": MIC_CLIENT_JS},
        "keep": [],
    }

    for region, i, row in seated(views, have):
        _, owner, view_id, wrapper, content, view = row
        parent = REGION_PREFIX + region
        if str(view.get("kind") or "") == "prose":
            want[wrapper] = {
                "component": "display-view-prose",
                "parent": parent,
                "ord": i * ORD_STEP,
                "props": {
                    "view_id": view_id,
                    "owner": owner,
                    # Always written, empty when absent: `object.update` merges
                    # per key, so a title left out would stand for ever.
                    "title": str(content.get("title") or ""),
                    "body": str(content.get("body") or ""),
                },
                "keep": [],
            }
        else:
            want[wrapper] = {
                "component": "display-view-custom",
                "parent": parent,
                "ord": i * ORD_STEP,
                "props": {"view_id": view_id, "owner": owner},
                "keep": [],
            }
            add_tree(want, wrapper, content, 0)
    return want


def update_props(spec):
    """The props of an update, with the kept ones left out.

    `object.update` merges per key, so a prop this cell does not name keeps the
    value the display holds -- which is the value a browser wrote. That is what
    makes `keep` the counterpart of the component's own `editable`: the
    component says what a browser MAY write, and `keep` says that this cell will
    not write over it on the next tick. A create writes everything, because
    there is nothing to preserve yet.
    """
    props = dict(spec["props"])
    for key in spec.get("keep") or []:
        props.pop(key, None)
    return props


def patches(want, have, define, bootstrap):
    """The calls that turn `have` into `want`, in an order the display accepts."""
    calls = []
    # A running screen whose root was built against another vocabulary is not
    # a bootstrap and still needs every definition again: new code, old
    # language, and the definitions only ever travelled on the bootstrap pass.
    # Once per change of vocabulary, not per tick -- the root's `vocab` is
    # brought up to date by the same `object.update` that compares the root's
    # props below, so the next pass sees them agree (GH #670).
    stale = (not bootstrap) and ROOT_ID in have \
        and have[ROOT_ID]["props"].get("vocab") != VOCAB
    if bootstrap or stale:
        for component in components():
            calls.append(dict({"op": "component.define"}, **component))
    # The application's own vocabulary, after this cell's own: a view component
    # may only be created once the component it names exists, and the legs of a
    # bundle run in call order.
    for component in define:
        if isinstance(component, dict):
            calls.append(dict({"op": "component.define"}, **component))

    if bootstrap:
        root = want[ROOT_ID]
        calls.append(
            {
                "op": "object.create",
                "id": ROOT_ID,
                "component": root["component"],
                "ord": root["ord"],
                "props": root["props"],
            }
        )
        calls.append(
            {"op": "page.set", "route": PAGE_ROUTE, "root": ROOT_ID, "title": PAGE_TITLE}
        )
    elif ROOT_ID in have:
        props = update_props(want[ROOT_ID])
        held = have[ROOT_ID]["props"]
        if any(held.get(k) != v for k, v in props.items()):
            calls.append({"op": "object.update", "id": ROOT_ID, "props": props})

    # Sorted, because sorted IS parent-before-child here: a region sorts before
    # every `view.` id, and a wrapper sorts before its own index chain.
    for oid in sorted(k for k in want if k != ROOT_ID):
        spec = want[oid]
        if oid in have:
            held = have[oid]
            props = update_props(spec)
            if any(held["props"].get(k) != v for k, v in props.items()):
                calls.append({"op": "object.update", "id": oid, "props": props})
            # A move is its own operation: `object.update` writes props and
            # nothing else, so the order of the screen -- which lives in `ord`
            # -- would never actually change without this. A rewrite alone
            # moves nothing now; what moves a view is a view above it going
            # away, a region change, or a different `ord`.
            if held["parent"] != spec["parent"] or held["ord"] != spec["ord"]:
                calls.append(
                    {
                        "op": "object.move",
                        "id": oid,
                        "parent": spec["parent"],
                        "ord": spec["ord"],
                    }
                )
        else:
            calls.append(
                {
                    "op": "object.create",
                    "id": oid,
                    "component": spec["component"],
                    "parent": spec["parent"],
                    "ord": spec["ord"],
                    "props": spec["props"],
                }
            )

    # What no view claims any more. Leaf first, because `object.delete` does not
    # cascade and names the children standing in the way. Only ids this cell
    # mints are swept: a foreign object on an adopted page is not ours.
    orphans = [
        k
        for k in have
        if k not in want and (k.startswith(VIEW_PREFIX) or k.startswith(REGION_PREFIX))
    ]
    for oid in sorted(orphans, reverse=True):
        calls.append({"op": "object.delete", "id": oid})
    return calls


def pass_read(body, ctx):
    """The display's answer: ONE bundle that makes the screen match the table."""
    try:
        plan = json.loads(str(ctx.get("display_views") or ""))
    except (TypeError, ValueError):
        plan = None
    if not isinstance(plan, dict):
        return []
    views = plan.get("views")
    define = plan.get("define")

    have = read_objects(body)
    # "Is this page mine", not "did the query fail". A display whose `/` has
    # never been set refuses the query; a display carrying the `web` template's
    # own seeded demo answers it, with a tree that has no root of ours in it.
    # Both are bootstrap (GH #402).
    bootstrap = have is None or ROOT_ID not in have
    if bootstrap:
        # Deliberately NOT the foreign objects: another route may still point
        # at them, and they were never this cell's to remove.
        have = {}

    want = build(views if isinstance(views, list) else [], have)
    calls = patches(want, have, define if isinstance(define, list) else [], bootstrap)
    if not calls:
        # Nothing to say. A bundle with no legs is refused as `invalid_input`
        # by the display, so silence is the only honest form of "no change".
        return []
    return [emission("patch", {"messages": [tool_call(c, "d-%d" % i) for i, c in enumerate(calls)]})]


# ---------------------------------------------------------------------------
# The dispatcher


def main():
    global VOICE_MOUNT, FONT_BASE
    doc = json.load(sys.stdin)
    # The two params this cell reads. `voice_mount` names the `voice` cell the
    # screen's microphone joins, so one screen can be pointed at a voice cell
    # that was mounted under another name -- and a screen with no voice cell
    # beside it simply has a button whose join is refused, out loud, on the
    # page. `font_base` is where the operator serves the two faces from, or
    # empty for no faces at all.
    params = doc.get("params") or {}
    if isinstance(params, dict):
        VOICE_MOUNT = str(params.get("voice_mount") or "voice")
        FONT_BASE = str(params.get("font_base") or "")
    body = doc.get("body") or {}
    envelope = doc.get("envelope") or {}
    header = envelope.get("header") or {}
    hop = header.get("hop") or {}
    ctx = header.get("context") or {}
    origin = str(ctx.get("display_origin") or "")

    # Pass 4 FIRST, because it is the terminating one and the cheapest to get
    # wrong. `display_origin` is stamped by the hive's own edges and travels
    # back on the reply; nothing in the body could tell these apart.
    if origin == "patch":
        return []
    if origin == "read":
        return pass_read(body, ctx)
    if origin == "views":
        return pass_views(body, ctx, hop)

    route = str(hop.get("route") or "")
    if route == "event":
        return pass_event(body)
    if route in ("in_view", "in_withdraw"):
        return pass_request(body, envelope, route == "in_withdraw")
    return []


if __name__ == "__main__":
    out = main()
    # One emission is written as an object, several as an array, and an empty
    # list stays an empty array -- which is how a `code` cell says "nothing to
    # send" (`parse_stdout_json`: a top-level array of length 0 is zero
    # emissions).
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
