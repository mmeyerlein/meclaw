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
import uuid
from datetime import datetime, timezone

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

# The screen's judgement (GH #679). A WINDOW is an object of one of these four
# components; everything else on the screen is content inside one. The curator
# writes the curator props on a window and on nothing else; an application
# writes the hints, and may say exactly two words about the state. `touched`
# is one of the hints (GH #689): an epoch the application changes when the
# window's content is new. The screen compares a window's OWN props, and an answer that lives
# in a child component is invisible to that comparison unless the application
# says so -- a child's props are not read on purpose, or a clock that rewrites
# a child every twenty seconds would touch its window on every tick.
WINDOWS = ("display-pane", "display-panel", "display-overlay", "display-view-prose")
CURATOR_KEYS = ("state", "age", "since", "score", "judged_relevance", "judged_hidden",
                "topic_dupe", "rung", "topic_relevance")
HINT_KEYS = ("context", "relevance", "class", "pinned", "relevant_until", "touched",
             "topic", "modal")
APP_WORDS = ("urgent", "hidden")
# The namespace an order's id is derived from. Deterministic per due time, so
# two passes that compute the same moment order the same id and the second
# `add` collides instead of standing beside the first (GH #681).
DUE_NAMESPACE = uuid.UUID("6f2d7c1a-3a1e-4d7b-9d5a-1c2b3e4f5a60")
# The knobs as shipped; `main()` reads the member's dials over them.
DEFAULT_LINGER_MS = 20000
DEFAULT_FADE_MS = 120000
DEFAULT_FOCUS = 0.3
DEFAULT_WEIGHT = 0.5
# Per notice class, `[relevance, ttl_ms]` as shipped (`params.notice_defaults`
# says otherwise). `CLASS_RELEVANCE` is the first half, the one the score reads.
NOTICE_DEFAULTS = {"system_error": (0.9, 60000), "error": (0.8, 60000),
                   "warning": (0.7, 120000), "important_note": (0.7, 300000),
                   "note": (0.4, 300000)}
CLASS_RELEVANCE = {k: v[0] for k, v in NOTICE_DEFAULTS.items()}
KNOBS = {
    "linger_ms": DEFAULT_LINGER_MS,
    "fade_ms": DEFAULT_FADE_MS,
    "focus_default": DEFAULT_FOCUS,
    "judge": "off",
    "judge_min_interval_ms": 3000,
    "dock_max": 7,
}
# The sheet's ground: `day`, or `night` when the operator says so
# (`params.ground`). A word, not a clock -- nothing switches it by itself.
GROUND = "day"

# The object ids, by class. Deterministic and prefixed: an id has to be
# derivable from the row without a side table, and it has to say which class it
# belongs to, because deletion sweeps by prefix.
ROOT_ID = "display.root"
REGION_PREFIX = "display.region."
# The OS mark (D-2, D-3): the one fixed anchor at the bottom right, and the
# push-to-talk button itself. A NEW id rather than the microphone's, because
# it is another thing in another place with another markup (OR-D7); the old
# `display.mic` is swept as an orphan on the first pass after the lift.
OS_ID = "display.os"
OLD_MIC_ID = "display.mic"
VIEW_PREFIX = "view."
# The dock (D-1, D-4): one tile per PRESENT window, the highest rank on top.
# A child of the root and not of a region, because it is a layer over the
# canvas rather than a column beside it.
DOCK_ID = "display.dock"
DOCK_PREFIX = DOCK_ID + "/"
# The key an application gives the child that is its tile (R-D1).
TILE_KEY = "tile"
# One line, and a line is a line: the sheet gives a tile one row of text.
TILE_LINE_MAX = 24
# The glyph a window gets when it brought no tile of its own. Text, not
# images: the two faces cover these, and so does a kiosk's system font.
TILE_GLYPHS = {
    "conversation": "\U0001F4AC",
    "ambient": "\u25d4",
    "system": "\u26a0",
    "system_error": "\u26a0",
    "error": "\u26a0",
    "warning": "\u25b3",
    "important_note": "!",
    "note": "\u00b7",
}
TILE_FALLBACK_GLYPH = "\u2022"
# The dock's order stays readable even under a judge that weighs a context to
# nothing: a weight below this is read as this for the RANK only, never for
# the score (OR-D10).
RANK_FLOOR = 0.05
# The exits of one screen state (R-D3) and their profiles. A member has ONE
# display hive; a physical screen is an exit of it, and the profile is what
# the renderer knows about that exit.
DEFAULT_SCREENS = {"tv": {"display_type": "tv", "viewing_distance_m": 3.0,
                          "physical_size_in": 55, "inputs": ["audio"]}}
DEFAULT_SCREEN = "tv"
# No formula out of DPI, a table (OR-D9): three types are what there is, and
# everything past this would be tuning. The distance modulates linearly
# against the reference distance of the type, clamped.
SCREEN_BASE = {"tv": 1.6, "monitor": 1.0, "phone": 1.0}
SCREEN_REFERENCE = {"tv": 3.0, "monitor": 0.7, "phone": 0.35}
SCALE_MIN = 0.8
SCALE_MAX = 2.2
DOCK_MAX_BY_TYPE = {"tv": 7, "phone": 5, "monitor": 8}
# What this cell was configured with, per message (`read_knobs`).
SCREENS = dict(DEFAULT_SCREENS)
SCREEN = DEFAULT_SCREEN
PAGE_ROUTE = "/"
PAGE_TITLE = "display"

# The components that bring a script (§ 2.5). It used to be exactly one and
# the lock said so as a number; a number is not a decision, so this is the
# list, and the lock reads it. Two: the OS mark, whose hook is the gesture,
# and the shell, whose hook is the screen's own motion.
SCRIPTED = ("display-os", "display-shell")

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

# The canvas, as the display's OWN rule rather than a line in a token sheet:
# `vision.css` belongs to the `web` template and describes a design language,
# and so does the sheet below -- while "the canvas is one centred column and
# the dock floats beside it" is a statement about THIS screen. The rules
# travel in the shell, in the one `<style>` block, ahead of the sheet. Written
# with no two `{` adjacent, because `{{` is the component language's own marker.
#
# Since 2.3.0 there is no narrow column. `aside` is still accepted -- ambient
# and older senders say it, and refusing a word for nothing is a broken
# contract -- but it is drawn as canvas: the region box generates nothing
# (`display: contents`) and its windows stand in the one column with
# everything else (OR-D3). Where the dock and the OS mark sit is the sheet's
# business now, because both are derived from the profile's scale.
LAYOUT_RULES = (
    ".display-columns { display: flex; flex-direction: column;"
    " align-items: center; gap: var(--gap, 16px); }"
    " .display-columns > [data-region] { display: contents; }"
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
 * the screen, the screen's own furniture, the scene, the dock, night, the
 * profile, fallbacks, motion, screen sizes.
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

  /* The screen's scale (D-21..D-24). `--scale` is written INLINE on the root
   * out of the display profile -- a television three metres away is far, not
   * small, and no pixel width says that. 1 when nothing says otherwise, so a
   * page with no shell around it (the gallery) is unchanged. The four sizes
   * the dock and the OS mark are made of derive from it, and so does the
   * type, through `--type-scale`. Restated on `.display-columns` in § 12,
   * where the inline value can reach them: a custom property that uses
   * var() is substituted where it is DECLARED, so a declaration on :root
   * would never see an override further down. */
  --scale: 1;
  --type-scale: 1;
  --tile: calc(5.25rem * var(--scale));
  --os: calc(3.25rem * var(--scale));
  --dock-pad: calc(0.75rem * var(--scale));
  --dock-gap: calc(0.5rem * var(--scale));
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

/* Hidden is gone -- unless it is on its way out, in which case the leave
 * keyframe below plays first and the next pass deletes it (0-2-0). */
:where(.display-pane, .display-panel, .display-overlay, .display-ornament)[data-state="hidden"]:not([data-age="leaving"]) {
  display: none;
}

/* A window nobody assigned a state to stands at the top rung: the compose
   cell writes a state on every window it minds, so an empty one is a window
   some other hand put up. Both windows that scroll and those that do not.
   `:is()` on the class and
   `:where()` on the state keep the rule at 0-1-0 like every rung of the
   ladder, so it wins over the `.glass` base rule above by order alone and loses to the
   forced-colours fallback below. */
:is(.display-pane, .display-panel):where(:not([data-state]), [data-state=""]) {
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2);
}

/* Rungs 1 and 2 -- ambient and relevant -- have no rule on a window since
 * 2.3.0: nothing ambient or relevant stands on the canvas. The canvas
 * carries what is large, focus and urgent, and everything else on the
 * ladder is a TILE (§ 11), where the rung is a colour and a ring. The
 * curator writes only `focus`, `urgent` and `hidden` on a window; the rung
 * itself travels on the tile. */

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

/* ── 7b. Presence follows the rung ──────────────────────────────────────
 * A person reads how loud a window is off its title: size, colour, motion.
 * The rung decides all three (GH #679). Focus is the big title; urgent the
 * big title in the accent, breathing. The two rungs below focus have no
 * title rule since 2.3.0: a window on the canvas is never ambient or
 * relevant, and a tile has no title. One title slot on every window -- the
 * pane's, the panel's, the prose view's -- and no fixed kicker form. Each
 * rule is 0-2-0, so it wins over the title voice above (0-1-0). */
[data-state="focus"] :is(.display-pane-title, .display-panel-title) {
  font-size: var(--t-title-2);
  color: var(--fg-primary);
}

[data-state="urgent"] :is(.display-pane-title, .display-panel-title) {
  font-size: var(--t-title-2);
  color: var(--accent);
  animation: display-title-breathe 1000ms ease-in-out infinite;
}

/* The title breathes in its ink, not in a ring: the ring is the window's. */
@keyframes display-title-breathe {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

/* ── 7c. Tone: one accent, or a quieter voice ───────────────────────────
 * `tone` is a word an application says about a window: `accent` colours its
 * figure -- the one number a value, a clock, a weather tile or a timer shows
 * -- in the accent; `muted` lowers the whole fill to the secondary ink. No
 * `alert`: an alarm is the urgent rung. Both 0-2-0. */
[data-tone="accent"] :is(.display-value-number, .display-weather-temp, .display-clock-time, .display-timer-remaining) {
  color: var(--accent);
}

[data-tone="muted"] .inner { color: var(--fg-secondary); }

/* Level of detail followed the state until 2.2.3 -- the Nest Hub rule,
 * .display-line from relevant on, .display-detail from focus on. Since 2.3.0
 * a window on the canvas is in focus or urgent, and both show everything;
 * the reduced levels are the tile's, which shows a glyph, a value and a
 * line by construction. The three levels keep their names -- an
 * application still says which line is the lead -- and all three are drawn. */
.display-lead, .display-line, .display-detail { display: block; }

/* Blur is for modal moments only (D-12, since 2.3.0). Until 2.2.3 a window in
 * focus made every other window on the screen recede -- one `:has()` on the
 * group, no JavaScript, and exactly the wrong meaning: asking about the
 * weather is not a moment that has to be finished before anything else can
 * happen. What is left is the modal case, which an application says about
 * itself and which nothing in this wave says. */
.display-columns:has([data-modal="true"])
  :where(.display-pane, .display-panel, .display-overlay)[data-state]:not([data-modal="true"]):not([data-state="hidden"]) {
  filter: blur(6px);
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
 * leading, and the space between the windows. That the canvas is one centred
 * column is NOT said here -- that is a statement about this screen and stays
 * in the shell's own layout rules, beside the sheet. */
.display-columns {
  position: relative;
  z-index: 1;
  --gap: 20px;
  padding: 28px;
  /* The dock is an overlay, so the canvas keeps its own gutter: a window that
   * ran under the tiles would be unreadable exactly where it matters. */
  padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad));
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
  .display-columns {
    padding: 16px;
    gap: 16px;
    /* The shorthand above would take the dock's gutter with it. */
    padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad));
  }
  /* A television is far, not small: it keeps its air. */
  .display-columns[data-profile="tv"] { padding: 28px; padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad)); gap: 24px; }
}

/* ── 9. The screen's own furniture: the OS mark ──────────────────────────
 * The mark is the one thing on the screen that belongs to the screen and not
 * to anything standing on it, and since 2.3.0 it is also the button: press
 * and hold to speak, release to send (D-3). It has no card and no capsule --
 * a power symbol in the current ink with a stylised S in its gap, drawn in
 * strokes so it costs one colour. What was heard is NOT said beside it
 * (D-17): the mark says its phase in light, and the words go to the chat
 * application. The line below is for a screen reader and is not drawn. */
.display-os {
  position: fixed;
  inset-inline-end: var(--dock-pad);
  inset-block-end: var(--dock-pad);
  inline-size: var(--os);
  block-size: var(--os);
  z-index: 30;
  background: transparent;
  touch-action: none;
}

.display-os-mark {
  display: block;
  inline-size: 100%;
  block-size: 100%;
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--fg-secondary);
  cursor: pointer;
  touch-action: none;
  -webkit-user-select: none;
  user-select: none;
  transition: color var(--t-hover) var(--ease), filter var(--t-hover) var(--ease);
}

.display-os-glyph {
  display: block;
  inline-size: 100%;
  block-size: 100%;
}

.display-os-ring {
  fill: none;
  stroke: currentColor;
  stroke-width: 5;
  stroke-linecap: round;
}

.display-os-s {
  fill: none;
  stroke: currentColor;
  stroke-width: 4.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.display-os-mark[aria-pressed="true"] { color: var(--accent); }

.display-os-mark:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 3px;
  border-radius: var(--r-capsule);
}

.display-os[data-phase="listening"] .display-os-mark {
  color: var(--accent);
  filter: drop-shadow(0 0 calc(0.5rem * var(--scale)) var(--accent-ring));
}

.display-os[data-phase="sending"] .display-os-mark { color: var(--accent-soft); }

.display-os[data-phase="speaking"] .display-os-mark {
  color: var(--accent-soft);
  animation: display-os-pulse 1400ms ease-in-out infinite;
}

.display-os[data-phase="error"] .display-os-mark { color: var(--fg-tertiary); }

@keyframes display-os-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

/* Said, never shown: the phase is light on the mark, and this is the same
 * thing for somebody who cannot see the light. */
.display-os-state {
  position: absolute;
  inline-size: 1px;
  block-size: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}

/* ── 10. The scene — a stack that is a picture of a screen ──────────────
 * `display-stack` with `scene` writes it: a screen inside the page. It
 * carries the ground, so glass has something behind it (the modal rule hangs
 * its :has() on `.display-columns`, not here). 16:9 is the reference frame;
 * that it is mostly empty is the point. */
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

/* ── 11. The dock and the canvas it floats over ──────────────────────────
 * The canvas answers "what is in focus", the dock "what is active at all"
 * (D-27). The dock is the same place and the same size whatever the canvas
 * does, so it is fixed and it is a layer, not a column in the flow (D-10).
 * It fills from the bottom, because its origin is the OS mark below it, and
 * it ends above the mark by exactly the mark's own height. */
.display-dock {
  position: fixed;
  inset-block: 0;
  inset-inline-end: 0;
  z-index: 20;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: var(--dock-gap);
  inline-size: calc(var(--tile) + 2 * var(--dock-pad));
  padding: var(--dock-pad);
  padding-block-end: calc(var(--os) + 2 * var(--dock-pad));
  /* The layer itself takes no clicks; the tiles do. A click that fell
   * through the dock onto the canvas would be a click nobody aimed. */
  pointer-events: none;
}

/* ONE size, and no second background inside it: a tile is one surface with
 * three lines on it, not a window with an interior (D-1, S8). */
.display-tile {
  position: relative;
  isolation: isolate;
  pointer-events: auto;
  display: grid;
  grid-template-rows: auto 1fr auto;
  gap: 2px;
  inline-size: var(--tile);
  aspect-ratio: 1;
  padding: calc(var(--dock-pad) * 0.8);
  border-radius: var(--r-control);
  background-color: var(--glass-tint-thin);
  -webkit-backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  box-shadow: var(--rim-top), var(--shadow-contact);
  color: var(--fg-primary);
  overflow: hidden;
  transition:
    transform var(--t-focus) var(--ease),
    opacity var(--t-focus) var(--ease),
    box-shadow var(--t-focus) var(--ease);
}

.display-tile-glyph {
  font-size: var(--t-title-3);
  line-height: 1;
  color: var(--fg-secondary);
}

/* One size, always (D-1): the value and the line are one line each, cut
 * with an ellipsis. A long fact does not grow the tile. */
.display-tile-value {
  align-self: end;
  font-size: var(--t-title-2);
  font-weight: var(--w-title);
  line-height: 1.05;
  font-variant-numeric: tabular-nums;
  color: var(--fg-primary);
  max-inline-size: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.display-tile-line {
  font-size: var(--t-caption);
  line-height: 1.2;
  color: var(--fg-tertiary);
  max-inline-size: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Presence on a tile is a ring and a colour, never a size (D-1). Below the
 * bar the tile reads as ambient: present, quiet -- a window under the bar is
 * hidden on the canvas, but its tile is not, and it must not stand fully
 * opaque beside a relevant one. */
.display-tile[data-state="ambient"],
.display-tile[data-state="hidden"] { opacity: 0.72; }

.display-tile[data-state="relevant"] { opacity: 1; }

.display-tile[data-state="focus"] {
  opacity: 1;
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-contact);
}

.display-tile[data-state="urgent"] {
  opacity: 1;
  background-color: var(--glass-tint-thick);
  box-shadow: inset 0 0 0 3px var(--accent), var(--rim-top), var(--shadow-contact);
  animation: display-urgent-breathe 1000ms ease-in-out infinite;
}

/* The same object, just large right now: the tile steps back a little rather
 * than disappearing, because dual representation is the rule (D-7). */
.display-tile[data-on-canvas="true"] { opacity: 0.7; }

/* Pinned is the one word about the dock a person says (D-15): relevance
 * sinks, the tile stays. Until 2.3.0 it froze a number and showed nothing. */
.display-tile[data-pinned="true"]::after {
  content: "";
  position: absolute;
  inset-block-start: 6px;
  inset-inline-end: 6px;
  inline-size: 6px;
  block-size: 6px;
  border-radius: var(--r-capsule);
  background-color: var(--accent-soft);
}

/* The frame a zoom lands on, set by the hook for the length of one movement. */
.display-tile[data-zoomed="true"] {
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2);
}

/* The canvas is one centred column since 2.3.0, and a window in focus is
 * compact and bounded rather than a strip across the width (D-11). Every
 * window on the canvas that is not inside another window: a prose window
 * under the region, an application's window under its wrapper, a card
 * under the stack an application put around it. */
.display-columns > [data-region] :where(.display-pane, .display-panel, .display-overlay):not(:where(.display-pane, .display-panel, .display-overlay) *) {
  inline-size: 100%;
  max-inline-size: clamp(22rem, 44vw, 40rem);
  margin-inline: auto;
}

/* An application's window hangs in a wrapper (`display-view-custom`, a bare
 * `<div data-view>`). The wrapper is a box of its own and would be the
 * column's flex item -- shrink-to-fit, every window another width, the
 * chat a strip (D-11). So the wrapper generates no box either, like the
 * region above it, and the window inside is the item the rule above sizes.
 * A prose window carries `data-view` ITSELF and is a window, not a wrapper:
 * it keeps its box, or this 0-3-0 rule would beat the 0-2-0 hidden rule
 * above and draw a hidden prose window's title and body on the canvas. */
.display-columns > [data-region] > [data-view]:not(.display-pane, .display-panel, .display-overlay) { display: contents; }

/* And a stack an application put directly under its wrapper, around its
 * cards, generates no box either: the canvas carries only what is large,
 * so there is nothing for a stack to lay out, and a card inside one would
 * otherwise be a strip across the width again. */
.display-columns > [data-region] > [data-view] > .display-stack { display: contents; }

/* ── 11b. Night — a variant, not a second design ──────────────────────────
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

/* ── 12. The profile: one number, and the type and the dock follow ───────
 * The root says which screen this is (`data-profile`), what that screen can
 * take (`data-inputs`) and one scale. Everything below is derived; there is
 * no second sheet for a television and no media query that pretends a far
 * screen is a small one. */
.display-columns {
  --type-scale: var(--scale);
  --tile: calc(5.25rem * var(--scale));
  --os: calc(3.25rem * var(--scale));
  --dock-pad: calc(0.75rem * var(--scale));
  --dock-gap: calc(0.5rem * var(--scale));
  --t-caption: calc(12px * var(--type-scale));
  --t-small: calc(14px * var(--type-scale));
  --t-body: calc(16px * var(--type-scale));
  --t-title-3: calc(20px * var(--type-scale));
  --t-title-2: calc(24px * var(--type-scale));
  --t-title-1: calc(34px * var(--type-scale));
  --t-value: calc(34px * var(--type-scale));
  --t-voice: calc(30px * var(--type-scale));
  font-size: var(--t-body);
}

/* Three metres away the second and third ink are not readable at the alphas
 * a desk gets (D-24). Only the two that carry label and body text move; the
 * accent and the material stay what they are. */
.display-columns[data-profile="tv"] {
  --fg-secondary: rgba(43, 29, 25, 0.92);
  --fg-tertiary: rgba(43, 29, 25, 0.78);
}

.display-columns[data-profile="tv"][data-ground="night"] {
  --fg-secondary: rgba(247, 239, 230, 0.9);
  --fg-tertiary: rgba(247, 239, 230, 0.74);
}

/* `data-inputs` steers what is VISIBLE and nothing else: a screen with no
 * audio keeps its mark and loses the light that says it is listening. Which
 * device an answer came from is the member's knowledge, never the screen's. */
.display-columns:not([data-inputs~="audio"]) .display-os-mark { opacity: 0.5; }

.display-columns:not([data-inputs~="pointer"]):not([data-inputs~="touch"]) .display-os-mark {
  cursor: default;
}

/* ── 13. Fallbacks ───────────────────────────────────────────────────────
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

  .display-tile {
    background-color: var(--glass-opaque);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }
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
  .display-tile,
  .display-dock,
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
  .display-os-ring,
  .display-os-s { stroke: CanvasText; }
}

/* ── 14. Motion ──────────────────────────────────────────────────────────
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

  :where(.display-pane, .display-panel, .display-overlay)[data-state="focus"],
  :where(.display-pane, .display-panel, .display-overlay)[data-state="urgent"],
  .display-action:hover,
  .display-option-chip:hover { transform: none; }

  /* The dock does not move under reduced motion either: a tile changes its
   * rank by being somewhere else next frame, not by sliding there. */
  .display-tile {
    transition: opacity 150ms linear !important;
    animation: none !important;
  }

  .display-tile[data-state="urgent"] {
    animation: display-urgent-breathe 2000ms ease-in-out infinite !important;
  }

  .display-os[data-phase="speaking"] .display-os-mark { animation: none !important; }

  .display-status .display-status-dot { animation: none !important; }
  /* Urgent keeps breathing under reduced motion, only slower: it is the one
   * animation that carries meaning (ruling, 08.09.). */
  :where(.display-pane, .display-panel, .display-overlay)[data-state="urgent"] {
    animation: display-urgent-breathe 2000ms ease-in-out infinite !important;
  }
  [data-state="urgent"] :is(.display-pane-title, .display-panel-title) {
    animation: display-title-breathe 2000ms ease-in-out infinite !important;
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

/* ── 15. Screen sizes ────────────────────────────────────────────────────
 * 1920 is the reference; 1280 and 390 stay readable. Nothing here changes the
 * language, only how much air it gets. */
@media (max-width: 80rem) {
  .display-scene { padding: 22px; }
  /* A television is far, not small: its air comes from the profile. */
  .display-columns[data-profile="tv"] .display-scene { padding: 30px; }
}

@media (max-width: 48rem) {
  :root {
    --pad-window: 18px;
    --pad-inner: 14px;
    --r-window: 18px;
    --t-voice: 24px;
    --t-value: 30px;
  }

  /* Same rule, the other way round: the profile wins over the width. The
   * `:root` block above still serves the gallery page, which has no shell. */
  .display-columns[data-profile="tv"] {
    --pad-window: 22px;
    --pad-inner: 16px;
    --r-window: 22px;
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
# The root carries the screen's own motion (`phx-hook="DisplayScene"`, the
# script in `client_js`, raw): the hook has to see the whole screen, because a
# window that left the canvas and the tile it went into are two elements one
# patch touches, and only somebody holding both ends can move one into the
# other. The script stands AFTER the shell's element and before the socket
# boot, in the dead render, the way the OS mark's does.
SHELL_TEMPLATE = (
    '{{#if stylesheet}}<link rel="stylesheet" href="vision.css">{{/if}}'
    "<style>" + LAYOUT_RULES + "{{&faces}}" + KIT_CSS + "</style>"
    + '<div class="stack display-columns" id="display-shell"'
    + ' phx-hook="DisplayScene" data-ground="{{ground}}"'
    + ' data-profile="{{profile}}" data-inputs="{{inputs}}"'
    + ' data-screen="{{screen}}" style="--scale: {{scale}}">{{children}}</div>'
    + "<script>{{&client_js}}</script>"
)

REGION_TEMPLATE = '<div class="stack" data-region="{{region}}">{{children}}</div>'

# The prose view wears the catalogue: a thin `display-pane` outside, a
# `display-pane-title` for the title and a `display-text` for the paragraph
# inside, on the pane's `.inner` fill. It is a WINDOW (GH #679): the curator
# writes state, age, since and score on it, and the sheet reads them here.
# One title slot, no fixed kicker -- how loud the title is follows the rung.
PROSE_TEMPLATE = (
    '<section class="display-pane glass--thin" data-view="{{view_id}}"'
    ' data-owner="{{owner}}" data-state="{{state}}" data-age="{{age}}"'
    ' data-since="{{since}}" data-score="{{score}}"><div class="inner">'
    '{{#if title}}<h2 class="display-pane-title display-lead">{{title}}</h2>{{/if}}'
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
    ' data-state="{{state}}" data-age="{{age}}"'
    ' data-since="{{since}}" data-score="{{score}}" data-tone="{{tone}}" data-pinned="{{pinned}}"'
    ' data-modal="{{modal}}" data-topic="{{topic}}"'
    ' data-region="{{region}}"'
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
    ' data-since="{{since}}" data-score="{{score}}" data-tone="{{tone}}" data-pinned="{{pinned}}"'
    ' data-modal="{{modal}}" data-topic="{{topic}}"'
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
    ' data-state="{{state}}" data-age="{{age}}" data-since="{{since}}" data-score="{{score}}"'
    ' data-modal="{{modal}}" data-topic="{{topic}}"'
    ' data-position="{{position}}"'
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
# The dock (GH #695, spec § 2.4). A vertical column at the right edge, on its
# own layer above the canvas, holding one tile per PRESENT window ordered by
# rank. It is not a region: a region is a place on the canvas, and the dock is
# the answer to a different question -- "what is active at all" rather than
# "what is in focus". `count` and `profile` are copies the sheet and a reader
# can use without walking back up to the root.
DOCK_TEMPLATE = (
    '<div class="display-dock" id="display-dock" aria-label="dock"'
    ' data-count="{{count}}" data-profile="{{profile}}">{{children}}</div>'
)

# One tile. ONE size, always (D-1): a glyph, a value and a line, and the
# presence rung as a colour and a ring rather than as a size. `for` is the
# `pane_id` of the window it stands for, which is what lets the client match
# the two halves of one object for the zoom. `data-end-at` is the epoch the
# seconds run to; the client writes the remainder into `[data-role=…]` once a
# second, and the server never ticks for it.
TILE_TEMPLATE = (
    '<div class="display-tile" data-for="{{for}}" data-state="{{state}}"'
    ' data-rank="{{rank}}" data-pinned="{{pinned}}"'
    ' data-on-canvas="{{on_canvas}}" data-topic="{{topic}}"'
    ' data-end-at="{{end_at}}">'
    '{{#if glyph}}<span class="display-tile-glyph">{{glyph}}</span>{{/if}}'
    '{{#if value}}<span class="display-tile-value" data-role="remaining">{{value}}</span>{{/if}}'
    '{{#if line}}<span class="display-tile-line">{{line}}</span>{{/if}}'
    "</div>"
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

# `--end-at` is an epoch in milliseconds, `--total` the span it was set for and
# `--now` the moment the page was rendered -- the bar is a CSS animation with a
# negative delay computed from the three, so a page loaded late shows the right
# remainder without a tick. `data-end-at` is the same epoch for the client's
# own second: the sheet draws the bar, the hook writes the digits (D-27).
TIMER_TEMPLATE = (
    '<div class="display-timer{{#if done}} display-timer--done{{/if}}"'
    ' data-end-at="{{end_at}}"'
    ' style="--end-at: {{end_at}}; --total: {{total_ms}}; --now: {{now}}">'
    '<p class="display-timer-remaining display-lead" data-role="remaining">{{remaining}}</p>'
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

# The OS mark (§ 2.6, D-2/D-3). A ring that opens to the right, and a
# stylised S standing in that opening with its outer edge exactly on the ring's
# outer edge, so the two read as »OS« (2.3.2; until 2.3.1 the ring opened at the
# top and the S stood in the gap, then right of the centre). The opening is just
# wide enough to leave a small gap around the S. Drawn
# in the current ink and nothing else: no card, no capsule, no fill -- it
# stands ON the canvas and the canvas shows through it (D-3). `phx-hook`
# keeps the name the client half has always had: the gesture did not change,
# only where the words go. What was heard does NOT stand beside the mark any
# more (D-17); the state line is left for a screen reader and the mark itself
# says its phase in light (`data-phase`).
OS_TEMPLATE = (
    '<div class="display-os" id="display-os" phx-hook="DisplayMic"'
    ' data-mount="{{mount}}" data-phase="">'
    '<button type="button" class="display-os-mark" aria-pressed="false"'
    ' aria-label="hold to talk">'
    '<svg class="display-os-glyph" viewBox="0 0 64 64" aria-hidden="true"'
    ' focusable="false">'
    '<path class="display-os-ring" d="M 47.1 16 A 22 22 0 1 0 47.1 48"></path>'
    '<path class="display-os-s" d="M 52.25 27 C 52.25 24.25 50 22 47.25 22'
    " C 44.5 22 42.25 24.25 42.25 27 C 42.25 29.75 44.5 32 47.25 32"
    " C 50 32 52.25 34.25 52.25 37 C 52.25 39.75 50 42 47.25 42"
    ' C 44.5 42 42.25 39.75 42.25 37"></path>'
    "</svg></button>"
    '<span class="display-os-state" data-role="state" aria-live="polite"></span>'
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
# `window.__displayMic`, and the text of the last turn beside them, are there
# to be read by a test driving a real browser: since 2.3.0 the words are not
# written into the page any more (D-17), so the counter is where a proof
# reads them.
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
OS_CLIENT_JS = (
    "(function (root) {\n"
    "  var hook = {\n"
    "    mounted: function () {\n"
    "      var el = this.el, mount = el.dataset.mount || \"voice\";\n"
    "      var st = { sent: 0, played: 0, turns: 0, speakEnd: 0, hello: null, code: null };\n"
    "      root.__displayMic = st;\n"
    "      var state = el.querySelector('[data-role=\"state\"]'), btn = el.querySelector(\"button\");\n"
    "      function phase(p) { el.setAttribute(\"data-phase\", p); }\n"
    "      function say(t) { state.textContent = t; }\n"
    "      var socket = root.SurfaceSocket && root.SurfaceSocket.getSocket && root.SurfaceSocket.getSocket();\n"
    "      if (!socket) { say(\"no socket\"); return; }\n"
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
    "          say(idleText());\n"
    "        }\n"
    "        else if (f.type === \"partial\") { /* the words live in the chat application now (D-18) */ }\n"
    "        else if (f.type === \"turn\") { st.turns++; st.text = f.text; phase(\"\"); }\n"
    "        else if (f.type === \"speak_start\") { playAt = 0; phase(\"speaking\"); }\n"
    "        else if (f.type === \"speak_end\") { st.speakEnd++; phase(\"\"); }\n"
    "        else if (f.type === \"error\") { say(f.code); phase(\"error\"); }\n"
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
    "      function onClose(c) { st.code = c.code; say(\"closed \" + c.code); phase(\"\"); joined = false; }\n"
    "      // The state line when nothing is happening: what the cell said it is\n"
    "      // made of, and an empty line before it has said anything. Said for a\n"
    "      // screen reader; the mark itself shows its phase in light.\n"
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
    "        say(\"joining\\u2026\");\n"
    "        joinWait = new Promise(function (done) {\n"
    "          chan.join()\n"
    "            .receive(\"ok\", function () { joined = true; joinWait = null; say(idleText()); done(true); })\n"
    "            .receive(\"error\", function (e) { say((e && e.reason) || \"refused\"); phase(\"error\"); btn.disabled = true; joined = false; joinWait = null; done(false); })\n"
    "            .receive(\"timeout\", function () { say(\"the screen never answered\"); joined = false; joinWait = null; done(false); });\n"
    "        });\n"
    "        return joinWait;\n"
    "      }\n"
    "      join();\n"
    "      function sendAudio(ab) { if (!joined) return; socket.push({ topic: topic, event: \"audio\", payload: ab, ref: \"\", join_ref: chan.joinRef() }); st.sent++; }\n"
    "      var WORKLET = \"class P extends AudioWorkletProcessor{constructor(o){super();this.rate=o.processorOptions.rate;this.acc=[];this.pos=0}process(i){var ch=i[0]&&i[0][0];if(!ch)return true;var r=sampleRate/this.rate;for(var k=0;k<ch.length;k+=r){this.acc.push(Math.max(-1,Math.min(1,ch[Math.floor(k)])))}var n=Math.floor(this.rate/50);while(this.acc.length>=n){var out=new Int16Array(n);for(var j=0;j<n;j++)out[j]=this.acc[j]*32767;this.acc=this.acc.slice(n);this.port.postMessage(out.buffer,[out.buffer])}return true}}registerProcessor('mic',P);\";\n"
    "      async function openMic() {\n"
    "        if (!root.isSecureContext) { say(\"microphone needs https or localhost\"); return false; }\n"
    "        // The permission prompt opens INSIDE the gesture, and a person who is\n"
    "        // being asked is looking at a button that does nothing. Saying so is\n"
    "        // the difference between waiting and a screen that is broken.\n"
    "        say(\"asking for the microphone\\u2026\");\n"
    "        try {\n"
    "          stream = await navigator.mediaDevices.getUserMedia({ audio: true });\n"
    "        } catch (e) {\n"
    "          // A refused or missing microphone is the ordinary case, not a crash:\n"
    "          // an unhandled rejection here left the button doing nothing at all,\n"
    "          // which is the silence this line exists against.\n"
    "          say(\"microphone refused\");\n"
    "          return false;\n"
    "        }\n"
    "        var rate = (st.hello && st.hello.audio_in && st.hello.audio_in.sample_rate) || 16000;\n"
    "        mctx = new (root.AudioContext || root.webkitAudioContext)();\n"
    "        try {\n"
    "          await mctx.audioWorklet.addModule(URL.createObjectURL(new Blob([WORKLET], { type: \"text/javascript\" })));\n"
    "          var src = mctx.createMediaStreamSource(stream);\n"
    "          worklet = new AudioWorkletNode(mctx, \"mic\", { processorOptions: { rate: rate } });\n"
    "        } catch (e) {\n"
    "          say(\"no audio worklet\");\n"
    "          return false;\n"
    "        }\n"
    "        worklet.port.onmessage = function (e) { if (holding) sendAudio(e.data); };\n"
    "        src.connect(worklet); return true;\n"
    "      }\n"
    "      var holding = false, pressed = false;\n"
    "      async function down(e) {\n"
    "        if (holding || btn.disabled) return;\n"
    "        pressed = true;\n"
    "        // The gesture stays on the button whatever moves under the pointer --\n"
    "        // the button itself, when the state line below it grows on a fresh\n"
    "        // screen, or a finger that drifts while holding (GH #684).\n"
    "        if (e && e.pointerId !== undefined && btn.setPointerCapture) { try { btn.setPointerCapture(e.pointerId); } catch (err) { /* no capture, no harm */ } }\n"
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
    "        if (!pressed) { say(\"press again\"); return; }\n"
    "        // And the waiting sentence goes: a screen that still says it is\n"
    "        // asking for the microphone, minutes after it got one, is the same\n"
    "        // kind of lie as a screen that said nothing at all.\n"
    "        holding = true; btn.setAttribute(\"aria-pressed\", \"true\"); say(\"listening\\u2026\"); phase(\"listening\"); frame({ type: \"hold\" });\n"
    "      }\n"
    "      function up() { pressed = false; if (!holding) return; holding = false; btn.setAttribute(\"aria-pressed\", \"false\"); say(idleText()); phase(\"sending\"); frame({ type: \"release\" }); }\n"
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
    "      // A hold ends when the page does: a key held while switching windows\n"
    "      // would otherwise keep the microphone open with nothing left to release it.\n"
    "      function onBlur() { up(); }\n"
    "      function onHide() { if (document.hidden) up(); }\n"
    "      btn.addEventListener(\"pointerdown\", down); btn.addEventListener(\"pointerup\", up); btn.addEventListener(\"pointercancel\", up); btn.addEventListener(\"lostpointercapture\", up);\n"
    "      window.addEventListener(\"blur\", onBlur); document.addEventListener(\"visibilitychange\", onHide);\n"
    "      root.addEventListener(\"keydown\", keydown);\n"
    "      root.addEventListener(\"keyup\", keyup);\n"
    "      st.down = down; st.up = up; st.cancel = function () { frame({ type: \"cancel\" }); };\n"
    "      // What a re-mount has to undo. LiveView re-mounts a hook after a reconnect,\n"
    "      // and without this the listeners, the microphone and the contexts of every\n"
    "      // previous life stay open.\n"
    "      this.__displayMicTeardown = function () {\n"
    "        root.removeEventListener(\"keydown\", keydown);\n"
    "        root.removeEventListener(\"keyup\", keyup);\n"
    "        window.removeEventListener(\"blur\", onBlur); document.removeEventListener(\"visibilitychange\", onHide);\n"
    "        holding = false; pressed = false; joined = false; joinWait = null;\n"
    "        phase(\"\"); btn.setAttribute(\"aria-pressed\", \"false\");\n"
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

# The screen's own motion (§ 2.5). A hook on the ROOT, because the thing it
# has to see is the whole screen: a window that left the canvas and the tile
# it went into are two elements the patch touches in one go, and only somebody
# holding both ends can move one into the other.
#
# It rides as a raw prop, the pattern the OS mark uses, and for the same
# reason: the script runs in the DEAD render, before the shell's socket
# constructor reads `window.SurfaceHooks`, and a morph never re-runs it.
#
# FLIP and not the document's view-transition call: LiveView patches with
# morphdom and cannot be wrapped in one, and `::view-transition-group(*)` moves
# every window at once. The names stay inline on the windows -- they cost nothing
# and they are where a later wave would start.
SCENE_CLIENT_JS = (
    "(function (root) {\n"
    "  var EASE = \"cubic-bezier(.32,.72,0,1)\";\n"
    "  function reduced() {\n"
    "    return !!(root.matchMedia && root.matchMedia(\"(prefers-reduced-motion: reduce)\").matches);\n"
    "  }\n"
    "  // A duration out of the sheet's own tokens, so the motion is the\n"
    "  // template's decision and not this script's.\n"
    "  function dur(el, name, fallback) {\n"
    "    var v = root.getComputedStyle(el).getPropertyValue(name).trim();\n"
    "    var n = parseFloat(v);\n"
    "    return isNaN(n) ? fallback : (v.indexOf(\"ms\") > -1 ? n : n * 1000);\n"
    "  }\n"
    "  // Every tile by the window it stands for, and every window by its id.\n"
    "  // The two halves of one object carry the same name on purpose.\n"
    "  function scan(el) {\n"
    "    var out = { tiles: {}, wins: {} }, i;\n"
    "    var t = el.querySelectorAll(\".display-tile[data-for]\");\n"
    "    for (i = 0; i < t.length; i++) {\n"
    "      out.tiles[t[i].getAttribute(\"data-for\")] = { node: t[i], rect: t[i].getBoundingClientRect() };\n"
    "    }\n"
    "    var w = el.querySelectorAll(\"[data-region] [id][data-state]\");\n"
    "    for (i = 0; i < w.length; i++) {\n"
    "      out.wins[w[i].id] = { node: w[i], rect: w[i].getBoundingClientRect(),\n"
    "        state: w[i].getAttribute(\"data-state\"), age: w[i].getAttribute(\"data-age\") };\n"
    "    }\n"
    "    return out;\n"
    "  }\n"
    "  function onCanvas(s) { return s === \"focus\" || s === \"urgent\"; }\n"
    "  // One movement: the element is put back where it was and let go.\n"
    "  function move(node, from, to, ms, fade) {\n"
    "    if (!node.animate || !to.width || !to.height) return false;\n"
    "    var dx = from.left - to.left, dy = from.top - to.top;\n"
    "    var sx = from.width / to.width, sy = from.height / to.height;\n"
    "    node.animate([\n"
    "      { transformOrigin: \"top left\", transform: \"translate(\" + dx + \"px,\" + dy + \"px) scale(\" + sx + \",\" + sy + \")\", opacity: fade },\n"
    "      { transformOrigin: \"top left\", transform: \"none\", opacity: 1 }\n"
    "    ], { duration: ms, easing: EASE });\n"
    "    return true;\n"
    "  }\n"
    "  function away(node, from, to, ms) {\n"
    "    if (!node.animate || !from.width || !from.height) return false;\n"
    "    var dx = to.left - from.left, dy = to.top - from.top;\n"
    "    var sx = to.width / from.width, sy = to.height / from.height;\n"
    "    node.animate([\n"
    "      { transformOrigin: \"top left\", transform: \"none\", opacity: 1 },\n"
    "      { transformOrigin: \"top left\", transform: \"translate(\" + dx + \"px,\" + dy + \"px) scale(\" + sx + \",\" + sy + \")\", opacity: 0 }\n"
    "    ], { duration: ms, easing: EASE, fill: \"forwards\" });\n"
    "    return true;\n"
    "  }\n"
    "  function blink(node, ms) {\n"
    "    node.setAttribute(\"data-zoomed\", \"true\");\n"
    "    root.setTimeout(function () { node.removeAttribute(\"data-zoomed\"); }, ms);\n"
    "  }\n"
    "  // The zoom, both ways, plus the dock's own reordering. Nothing here\n"
    "  // knows what an object IS -- only that a tile and a window share a name.\n"
    "  function flip(el, before, st) {\n"
    "    if (reduced()) return;\n"
    "    var after = scan(el), id;\n"
    "    var enter = dur(el, \"--t-enter\", 360), leave = dur(el, \"--t-leave\", 240);\n"
    "    for (id in after.wins) {\n"
    "      var now = after.wins[id], was = before.wins[id];\n"
    "      if (!onCanvas(now.state) || (was && onCanvas(was.state))) continue;\n"
    "      var from = before.tiles[id];\n"
    "      if (!from) continue;\n"
    "      if (move(now.node, from.rect, now.rect, enter, 0.4)) st.flips++;\n"
    "      if (after.tiles[id]) blink(after.tiles[id].node, enter);\n"
    "    }\n"
    "    for (id in before.wins) {\n"
    "      var gone = !after.wins[id] || after.wins[id].age === \"leaving\"\n"
    "        || !onCanvas(after.wins[id].state);\n"
    "      if (!gone || !onCanvas(before.wins[id].state)) continue;\n"
    "      var tile = after.tiles[id];\n"
    "      var node = after.wins[id] ? after.wins[id].node : null;\n"
    "      if (!tile || !node) continue;\n"
    "      if (away(node, before.wins[id].rect, tile.rect, leave)) st.flips++;\n"
    "    }\n"
    "    for (id in after.tiles) {\n"
    "      var old = before.tiles[id];\n"
    "      if (!old) continue;\n"
    "      var dy = old.rect.top - after.tiles[id].rect.top;\n"
    "      if (Math.abs(dy) < 1) continue;\n"
    "      after.tiles[id].node.animate(\n"
    "        [{ transform: \"translateY(\" + dy + \"px)\" }, { transform: \"none\" }],\n"
    "        { duration: leave, easing: EASE });\n"
    "      st.flips++;\n"
    "    }\n"
    "  }\n"
    "  var CHIME_EVERY_MS = 2000, CHIME_MAX_MS = 60000;\n"
    "  function mmss(ms) {\n"
    "    if (ms < 0) ms = 0;\n"
    "    var s = Math.round(ms / 1000), m = Math.floor(s / 60);\n"
    "    s = s - m * 60;\n"
    "    return (m < 10 ? \"0\" : \"\") + m + \":\" + (s < 10 ? \"0\" : \"\") + s;\n"
    "  }\n"
    "  // The server's resolution is twenty seconds, so the seconds are the\n"
    "  // browser's (D-27). This is real time semantics, not polling: nothing\n"
    "  // is asked of the colony, a clock is read.\n"
    "  function tick(el, st) {\n"
    "    var nodes = el.querySelectorAll(\"[data-end-at]\"), now = Date.now(), i;\n"
    "    for (i = 0; i < nodes.length; i++) {\n"
    "      var end = parseInt(nodes[i].getAttribute(\"data-end-at\"), 10);\n"
    "      if (!end) continue;\n"
    "      // The bar's --now is stamped ONCE per element: the sheet turns it\n"
    "      // into a negative animation delay, and re-stamping would re-seek\n"
    "      // the animation on every second.\n"
    "      if (!nodes[i].dataset.nowStamped) {\n"
    "        nodes[i].style.setProperty(\"--now\", String(now));\n"
    "        nodes[i].dataset.nowStamped = \"1\";\n"
    "      }\n"
    "      // Only where the template marked a slot. Writing into the\n"
    "      // element itself would empty a tile of its glyph and its line.\n"
    "      var slot = nodes[i].querySelector('[data-role=\"remaining\"]');\n"
    "      if (slot) slot.textContent = mmss(end - now);\n"
    "    }\n"
    "    st.ticks++;\n"
    "  }\n"
    "  // Two tones, three times, out of oscillators: no asset ships for this\n"
    "  // and nothing speaks (speaking needs a call, and that is another wave).\n"
    "  function chime(st) {\n"
    "    try {\n"
    "      if (!st.audio) st.audio = new (root.AudioContext || root.webkitAudioContext)();\n"
    "      var ctx = st.audio;\n"
    "      if (ctx.state === \"suspended\") {\n"
    "        if (!st.said && root.console) { st.said = true; root.console.info(\"display: the chime is muted until a gesture or an autoplay flag\"); }\n"
    "        return;\n"
    "      }\n"
    "      var t0 = ctx.currentTime, tones = [880, 1175, 880], i;\n"
    "      for (i = 0; i < tones.length; i++) {\n"
    "        var at = t0 + i * 0.22;\n"
    "        var osc = ctx.createOscillator(), gain = ctx.createGain();\n"
    "        osc.type = \"sine\"; osc.frequency.value = tones[i];\n"
    "        gain.gain.setValueAtTime(0.0001, at);\n"
    "        gain.gain.exponentialRampToValueAtTime(0.25, at + 0.02);\n"
    "        gain.gain.exponentialRampToValueAtTime(0.0001, at + 0.18);\n"
    "        osc.connect(gain); gain.connect(ctx.destination);\n"
    "        osc.start(at); osc.stop(at + 0.2);\n"
    "      }\n"
    "      st.chimes++;\n"
    "    } catch (e) { /* a page with no audio stays a page */ }\n"
    "  }\n"
    "  // A window that ARRIVES urgent rings; one that has been urgent for a\n"
    "  // while rings again every two seconds, and gives up after a minute --\n"
    "  // the ring has an end, which is the whole of D-14/2.\n"
    "  function ring(el, st, seen) {\n"
    "    var n = el.querySelectorAll('[data-region] [id][data-state=\"urgent\"]');\n"
    "    var now = {}, fresh = false, any = false, i, id;\n"
    "    for (i = 0; i < n.length; i++) { now[n[i].id] = true; any = true; }\n"
    "    for (id in now) { if (!seen.ids[id]) fresh = true; }\n"
    "    seen.ids = now;\n"
    "    if (!any) { seen.since = 0; return; }\n"
    "    var t = Date.now();\n"
    "    if (fresh) { seen.since = t; seen.last = t; chime(st); return; }\n"
    "    if (seen.since && t - seen.since < CHIME_MAX_MS && t - seen.last >= CHIME_EVERY_MS) {\n"
    "      seen.last = t; chime(st);\n"
    "    }\n"
    "  }\n"
    "  // An autoplay policy starts the context suspended and its clock does\n"
    "  // not run. The first gesture on the page is where it is resumed -- the\n"
    "  // same trade the OS mark makes, and the kiosk flag is the other half.\n"
    "  function arm(st) {\n"
    "    function go() {\n"
    "      try {\n"
    "        if (!st.audio) st.audio = new (root.AudioContext || root.webkitAudioContext)();\n"
    "        if (st.audio.state === \"suspended\") st.audio.resume();\n"
    "      } catch (e) { /* no audio on this page */ }\n"
    "    }\n"
    "    root.addEventListener(\"pointerdown\", go, true);\n"
    "    root.addEventListener(\"keydown\", go, true);\n"
    "    return function () {\n"
    "      root.removeEventListener(\"pointerdown\", go, true);\n"
    "      root.removeEventListener(\"keydown\", go, true);\n"
    "    };\n"
    "  }\n"
    "  var hook = {\n"
    "    mounted: function () {\n"
    "      var el = this.el;\n"
    "      var st = { flips: 0, ticks: 0, chimes: 0, audio: null, said: false };\n"
    "      root.__displayScene = st;\n"
    "      // What has rung is remembered across mounts: a wall screen that\n"
    "      // reconnects all day must not chime again for a window it heard.\n"
    "      var seen = root.__displaySceneSeen || (root.__displaySceneSeen = { ids: {}, since: 0, last: 0 });\n"
    "      var disarm = arm(st);\n"
    "      var iv = root.setInterval(function () { tick(el, st); ring(el, st, seen); }, 1000);\n"
    "      st.tick = function () { tick(el, st); };\n"
    "      st.ring = function () { ring(el, st, seen); };\n"
    "      this.__scene = { st: st, before: scan(el), seen: seen, iv: iv, disarm: disarm };\n"
    "      tick(el, st); ring(el, st, seen);\n"
    "    },\n"
    "    beforeUpdate: function () {\n"
    "      if (this.__scene) this.__scene.before = scan(this.el);\n"
    "    },\n"
    "    updated: function () {\n"
    "      if (!this.__scene) return;\n"
    "      flip(this.el, this.__scene.before, this.__scene.st);\n"
    "      tick(this.el, this.__scene.st);\n"
    "      ring(this.el, this.__scene.st, this.__scene.seen);\n"
    "    },\n"
    "    destroyed: function () {\n"
    "      if (!this.__scene) return;\n"
    "      root.clearInterval(this.__scene.iv);\n"
    "      this.__scene.disarm();\n"
    "      if (this.__scene.st.audio) { try { this.__scene.st.audio.close(); } catch (e) { /* gone */ } }\n"
    "      this.__scene = null;\n"
    "    }\n"
    "  };\n"
    "  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, { DisplayScene: hook });\n"
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


# What the curator writes on every window, and the hints an application may
# send about one (GH #679). `since`, `score` and `relevance` travel as text:
# the template language reads an `int 0` as empty, and `as_unit()` reads a
# number out of a string. The `web` cell refuses an undeclared prop, so a
# hint has to stand here before an application may say it.
CURATED = {
    "since": "text", "score": "text",
    "judged_relevance": "text", "judged_hidden": "boolean",
    "context": "text", "relevance": "text", "class": "text",
    "pinned": "boolean", "relevant_until": "int", "touched": "text",
    # The subject of the window (R-D4), whether the canvas behind it may
    # blur (D-12), and the floor's mark that another application already
    # says this (spec 2.10).
    "topic": "text", "modal": "boolean", "topic_dupe": "boolean",
    # The window's place on the ladder (ambient, relevant, focus, urgent,
    # hidden). `state` is the canvas's word and knows three of them -- focus,
    # urgent, hidden -- because the canvas carries only what is large (spec
    # 2.2); the tile and the judge read the rung.
    "rung": "text",
    # The relevance a standing window borrowed from an answer it took (R-D4,
    # OR-D-Bau-6): text of a number while the answer stands, else empty.
    "topic_relevance": "text",
}


def windows():
    """Catalogue A: the four glass components, all `layer: "navigation"`.

    The ornament is a thing, not a place: a component an application hangs
    into its view, which the sheet fixes to the bottom edge. The two regions
    stay the only places on this screen.
    """
    return [
        _c("display-pane", PANE_TEMPLATE, dict({
            "pane_id": "text", "state": "text", "age": "text",
            "kicker": "text", "title": "text", "region": "text",
            "thin": "boolean", "tone": "text",
        }, **CURATED), "navigation"),
        _c("display-panel", PANEL_TEMPLATE, dict({
            "pane_id": "text", "state": "text", "age": "text",
            "title": "text", "scroll": "boolean", "tone": "text",
        }, **CURATED), "navigation"),
        _c("display-overlay", OVERLAY_TEMPLATE, dict({
            "pane_id": "text", "state": "text", "age": "text", "title": "text",
            "body": "text", "ttl_ms": "int", "position": "text",
        }, **CURATED), "navigation"),
        _c("display-ornament", ORNAMENT_TEMPLATE, {
            "text": "text", "dot": "boolean", "count": "int",
        }, "navigation"),
    ]


def contents():
    """Catalogue B: the twenty-four content components, all `layer: "content"`.

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
            "total_ms": "int", "now": "int", "done": "boolean"}),
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
        _c("display-dock", DOCK_TEMPLATE, {
            "count": "int", "profile": "text"}),
        _c("display-tile", TILE_TEMPLATE, {
            "glyph": "text", "line": "text", "value": "text", "topic": "text",
            "for": "text", "state": "text", "on_canvas": "boolean",
            "pinned": "boolean", "rank": "text", "end_at": "int"}),
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
    """The five the screen is made of: shell, regions, two wrappers, the OS mark."""
    return [
        {
            # `faces` is typed `"html"` because that is what makes a prop RAW:
            # an `@font-face` block rendered escaped is not an `@font-face`.
            "name": "display-shell",
            "template": SHELL_TEMPLATE,
            # `vocab` is never rendered: the template does not name it. It is a
            # note the root carries about which vocabulary it was built against
            # (see VOCAB), so a later pass can read it back.
            "prop_schema": {"stylesheet": "boolean", "faces": "html", "vocab": "text",
                            # The screen's state (GH #679): the bar, the
                            # per-context weights, when the judge last spoke,
                            # and the schedule the clock holds. None rendered.
                            "focus": "text", "weights": "text",
                            "judged_at": "text", "asked_at": "text", "due": "text",
                            # The operator's ground: `day` or `night`.
                            "ground": "text",
                            # The screen this tree is rendered for (§ 2.8):
                            # which exit of the one screen state it is, which
                            # kind of display, which inputs that display has,
                            # and the one number every size derives from. The
                            # floor computes them from the `screens` setting;
                            # the sheet reads them off the root.
                            "screen": "text", "profile": "text",
                            "inputs": "text", "scale": "text",
                            # What did not fit in the dock, and the exits as
                            # JSON: the judge reads the first, and the floor
                            # compares the second to know whether the routes
                            # have to be written again.
                            "dock_overflow": "int", "screens": "text",
                            # The screen's own motion, once the scene hook
                            # ships; empty until then. `"html"` is what makes
                            # a prop RAW, and a script rendered escaped is a
                            # script that does nothing.
                            "client_js": "html"},
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
                # A prose view is a window: the curator writes on it, and
                # the sender may hint (the `in_notice` and `in_view` bodies).
                "state": "text", "age": "text", "since": "text", "score": "text",
                "judged_relevance": "text", "judged_hidden": "boolean",
                "context": "text", "relevance": "text", "class": "text",
                "pinned": "boolean", "relevant_until": "int", "touched": "text",
                "topic": "text", "modal": "boolean", "topic_dupe": "boolean",
                "rung": "text", "topic_relevance": "text",
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
            # The OS mark (§ 2.6). `client_js` is typed `"html"` because that
            # is what makes a prop RAW: a script rendered escaped is a script
            # that does nothing. Everything else stays escaped, and `mount`
            # with it -- a mount name is configuration and must not be able to
            # close a tag. The name changed with 2.3.0 and the id with it; the
            # HOOK kept its name, because the gesture is the same one.
            "name": "display-os",
            "template": OS_TEMPLATE,
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


def check_prose_hints(content):
    """Why the hints of a prose `content` do not hold, or None (GH #679)."""
    if "context" in content and not isinstance(content["context"], str):
        return 'a prose "context" is not a string'
    if "class" in content and content["class"] not in NOTICE_CLASSES:
        return 'unknown "class" %r' % (content["class"],)
    rel = content.get("relevance")
    if rel is not None and (isinstance(rel, bool) or not isinstance(rel, (int, float))):
        return 'a prose "relevance" is not a number'
    if "pinned" in content and not isinstance(content["pinned"], bool):
        return 'a prose "pinned" is not a boolean'
    until = content.get("relevant_until")
    if until is not None and (isinstance(until, bool) or not isinstance(until, int)):
        return 'a prose "relevant_until" is not an integer'
    touched = content.get("touched")
    if touched is not None and (isinstance(touched, bool)
                                or not isinstance(touched, (str, int))):
        return 'a prose "touched" is not a string or an integer'
    if "topic" in content and not isinstance(content["topic"], str):
        return 'a prose "topic" is not a string'
    if "modal" in content and not isinstance(content["modal"], bool):
        return 'a prose "modal" is not a boolean'
    return None


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
        why = check_prose_hints(content)
        if why:
            return None, "invalid_view", why
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
    return write_row(owner, row, withdraw)


def write_row(owner, row, withdraw=False):
    """ONE store bundle that puts `row` up under `(owner, view_id)`, or takes it down.

    The same bundle for a view and for a notice: select the before-state,
    delete what stood under the name, insert the row -- and the request rides
    the hop so pass 2 knows what it is looking at.
    """
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


NOTICE_CLASSES = ("system_error", "error", "warning", "important_note", "note")
# A channel's failure, translated. The codes are the substrate's public error_code
# strings; a code this table does not know is still shown, with the code in it.
NOTICE_TEXT = {
    "stt_failed": "The microphone did not catch that.",
    "speak_failed": "The voice could not speak just now.",
    "busy": "The call did not go through.",
    "no_answer": "The call did not go through.",
    "call_refused": "The call did not go through.",
    "failed": "The telephone line failed.",
    "line_write_failed": "The telephone line failed.",
}
NOTICE_FALLBACK = "A part of the colony failed: %s"


def owner_slug(owner):
    """The context a sender's own windows stand in when they name none."""
    return str(owner or "").replace("/", "~")


def notice_row(body, hop, owner, now, knobs):
    """The view row an `in_notice` becomes, or (None, code, detail)."""
    code = str(hop.get("error_code") or "")
    klass = str(body.get("class") or ("system_error" if code else ""))
    if klass not in NOTICE_CLASSES:
        return None, "invalid_notice", 'unknown "class" %r' % (klass,)
    text = body.get("text")
    if not isinstance(text, str) or not text:
        # A channel's failure says nothing but its code; the table speaks for
        # it, and `meta.detail` is never read.
        text = NOTICE_TEXT.get(code, NOTICE_FALLBACK % code) if code else None
    if not text:
        return None, "invalid_notice", 'a notice needs a "text" string'
    rel, ttl = NOTICE_DEFAULTS.get(klass, (DEFAULT_WEIGHT, 0))
    context = str(body.get("context") or ("system" if klass == "system_error" else owner_slug(owner)))
    digest = hashlib.sha256(text.encode("utf-8")).hexdigest()[:8]
    view_id = str(body.get("view_id") or ("notice-%s-%s" % (klass.replace("_", "-"), digest)))
    if not is_view_id(view_id):
        return None, "invalid_notice", '"view_id" must match [a-z0-9-]{1,64}'
    content = {"title": klass.replace("_", " "), "body": text, "context": context,
               "relevance": as_unit(body.get("relevance"), rel), "class": klass}
    ttl_ms = body.get("ttl_ms")
    if isinstance(ttl_ms, bool) or not isinstance(ttl_ms, int) or ttl_ms < 0:
        ttl_ms = ttl
    return {"owner": owner, "view_id": view_id, "region": "main", "ord": 0, "kind": "prose",
            "content": canon(content), "components": "[]", "ttl_ms": ttl_ms,
            "updated_at": now}, None, None


def pass_notice(body, envelope, hop):
    """A classified message becomes a prose view of its sender: the same bundle as `in_view`."""
    owner = envelope.get("reply_to")
    if not isinstance(owner, str) or not owner:
        return refuse("owner_unknown", "the message carries no envelope.reply_to, so it has no owner", "", "")
    row, code, detail = notice_row(body, hop, owner, now_ms(), KNOBS)
    if code:
        vid = body.get("view_id")
        return refuse(code, detail, vid if isinstance(vid, str) else "", owner)
    return write_row(owner, row)


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

    The row stays in the table and simply stops being drawn; the next pass
    is what makes it disappear from the screen -- and since the due clock
    (`next_due`) that pass is ordered for the moment the view expires.
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

    plan = {"views": live, "define": define, "now": now}
    if isinstance(request.get("verdict"), dict):
        plan["verdict"] = request["verdict"]
    if request.get("struck"):
        plan["struck"] = str(request["struck"])
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
                # The component too: a window the table no longer has is
                # laid back for one more frame (`ghosts`), and only a window
                # is -- a wrapper with nothing under it is not.
                "component": str(obj.get("component") or ""),
            }
    return out


def add_tree(want, parent, node, index, tiles=None):
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
    props = dict(node.get("props") or {})
    # The state is the curator's word. An application may say `urgent` or
    # `hidden` about a window; any other word is dropped before the curator
    # looks, so a `focus` an app claims never reaches the screen (GH #679).
    if props.get("state") not in APP_WORDS:
        props.pop("state", None)
    # `age` belongs to the channel: it is how the screen tells a window that
    # arrived from one that is on its way out, and an application's word for
    # it would fly a window in twice.
    props.pop("age", None)
    want[oid] = {
        "component": str(node.get("component") or ""),
        "parent": parent,
        "ord": index * ORD_STEP,
        "props": props,
        "keep": [k for k in (node.get("keep") or []) if isinstance(k, str)],
    }
    kids = [k for k in (node.get("children") or []) if isinstance(k, dict)]
    # The tile is TAKEN OUT (R-D1): a window's child keyed `tile` is what the
    # application says about itself in one line, and it belongs in the dock,
    # not a second time inside the big window. Exactly one per window; a
    # second one keeps its place in the tree and is drawn like any child.
    if tiles is not None and str(node.get("component") or "") in WINDOWS:
        rest = []
        for kid in kids:
            if (oid not in tiles and kid.get("key") == TILE_KEY
                    and str(kid.get("component") or "") == "display-tile"):
                tiles[oid] = dict(kid.get("props") or {})
            else:
                rest.append(kid)
        kids = rest
    for j, kid in enumerate(kids):
        add_tree(want, oid, kid, j, tiles)


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
        seat = int(held.get("ord") or 0)
    except (TypeError, ValueError):
        return NEW_SEAT
    # A seat below zero is not a seat: it is the band `canvas_order` lifts an
    # urgent window into. When the window stops ringing it is seated again
    # like a new arrival, behind everything standing -- not at the top, where
    # the lift happened to leave it.
    return seat if seat >= 0 else NEW_SEAT


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


# ---------------------------------------------------------------------------
# The curator (GH #679): a score per window, a bar on the root, five rungs


def region_of(oid, want):
    """The region a window stands in: up the parent chain to a display-region."""
    spec = want.get(oid)
    while spec is not None and spec.get("component") != "display-region":
        spec = want.get(spec.get("parent"))
    return str(((spec or {}).get("props") or {}).get("region") or "main")


def content_props(props):
    """Everything the sender said, minus what this cell writes: the basis of 'touched'."""
    return {k: v for k, v in (props or {}).items() if k not in CURATOR_KEYS}


def as_unit(value, default):
    """A number clamped to 0..1, or the default when it is not a number."""
    try:
        return min(1.0, max(0.0, float(value)))
    except (TypeError, ValueError):
        return default


def decay_of(props, now, knobs):
    """1 while a window is pinned or still in its linger, then linear to 0.

    The one place the fade is computed: the score reads it, the rank reads it,
    and presence IS it -- a window with no decay left and no pin is gone from
    the dock whatever anybody judged (spec 2.2).
    """
    since = int(props.get("since") or now)
    if props.get("pinned") is True or now - since < knobs["linger_ms"]:
        return 1.0
    return max(0.0, 1.0 - (now - since - knobs["linger_ms"]) / float(knobs["fade_ms"]))


def relevance_of(props, now):
    """The judged relevance, else the hinted one, else the class's -- clamped after its hour."""
    judged = str(props.get("judged_relevance") or "")
    r = as_unit(judged, None) if judged else None
    if r is None:
        r = as_unit(props.get("relevance"),
                    CLASS_RELEVANCE.get(str(props.get("class") or ""), DEFAULT_WEIGHT))
        # An answer the window took from another application (OR-D-Bau-6a):
        # the standing window is as relevant as the card it stands in for.
        # Unless the judge said a number -- a verdict overrules the floor's
        # mark, which is why it stands under the judged branch and not over it.
        borrowed = str(props.get("topic_relevance") or "")
        if borrowed:
            r = max(r, as_unit(borrowed, 0.0))
    until = props.get("relevant_until")
    if isinstance(until, (int, float)) and until > 0 and now >= until:
        r = min(r, 0.2)
    return r


def score_of(props, weights, now, knobs):
    """w x r x decay, per the design: hidden is 0, urgent is 1. What may be LARGE."""
    word = str(props.get("state") or "")
    if (word == "hidden" or props.get("judged_hidden") is True
            or props.get("topic_dupe") is True):
        return 0.0
    if word == "urgent":
        return 1.0
    w = as_unit(weights.get(str(props.get("context") or "")), DEFAULT_WEIGHT)
    return round(w * relevance_of(props, now) * decay_of(props, now, knobs), 4)


def deadline_stands(props, now):
    """Whether the window named a `relevant_until` that has not passed yet."""
    until = props.get("relevant_until")
    return (isinstance(until, (int, float)) and not isinstance(until, bool)
            and until > 0 and now < until)


def rank_of(props, weights, now, knobs):
    """The dock's order: the same arithmetic WITHOUT the hidden clamp.

    The judge decides what is large, never what exists (OR-D1), so a window it
    hid still has a place in the dock -- and a context it weighed to nothing
    still has one, because the weight is floored at RANK_FLOOR here and only
    here. A window that is present only by its deadline (OR-D-Bau-7) has faded
    to nothing; it is read as RANK_FLOOR so it keeps a readable, last place.
    """
    if str(props.get("state") or "") == "urgent":
        return 1.0
    w = max(as_unit(weights.get(str(props.get("context") or "")), DEFAULT_WEIGHT), RANK_FLOOR)
    decay = decay_of(props, now, knobs)
    if decay <= 0.0 and deadline_stands(props, now):
        decay = RANK_FLOOR
    return round(w * relevance_of(props, now) * decay, 4)


def is_present(props, now, knobs):
    """In the table and not yet faded, or not yet past its own deadline: the one condition for a tile.

    Pinned, or decay left, or a `relevant_until` that has not passed
    (OR-D-Bau-7): a window that names its own deadline is present until it --
    a timer that rang stands as a quiet tile for the minutes its application
    said, and the score stays decay-driven, so it is never large by that.
    """
    return (props.get("pinned") is True or decay_of(props, now, knobs) > 0.0
            or deadline_stands(props, now))


def ghosts(want, have):
    """Windows the screen holds and nobody wants any more: one more frame, as leaving.

    A window (and its subtree) missing from `want` is laid back byte for byte
    with `age: leaving`; the next pass finds it already leaving and lets
    `patches()` delete it. The wrappers above it stand with it for that frame,
    because a delete does not cascade. A wrapper with no window under it is no
    ghost.
    """
    for oid, held in have.items():
        if oid in want or held.get("component") not in WINDOWS:
            continue
        # A mirrored exit's window (`desk.view.`) is a copy, never a ghost:
        # the copy follows the original, and the original is what leaves.
        if not oid.startswith(VIEW_PREFIX):
            continue
        if ((held.get("props") or {}).get("age")) == "leaving":
            continue
        want[oid] = dict(held, props=dict(held.get("props") or {}, age="leaving"))
        for kid, kspec in have.items():
            if kid.startswith(oid + "/") and kid not in want:
                want[kid] = dict(kspec)
        parent = held.get("parent")
        while parent and parent not in want and parent in have:
            want[parent] = dict(have[parent])
            parent = have[parent].get("parent")
    return want


def topic_dupes(windows, have, now, knobs):
    """A fresh window repeating another application's subject yields to it (R-D4).

    The floor decides this, not the judge: three to five seconds of latency
    would put a second weather card on the screen and take it off again. An
    owner is never compared with itself (R-D4a) -- three timers are three
    timers, and it is the APPLICATION that decides how many windows it has.
    The judge may overrule by naming the window with `hidden: false`, which
    `apply_verdict` turns into a cleared mark. Returns the standing windows
    that took an answer, for the touched list.
    """
    taken = []
    for oid, spec in windows.items():
        p = spec["props"]
        topic = str(p.get("topic") or "")
        p["topic_dupe"] = False
        if not topic:
            continue
        owner, _ = parse_object_id(oid)
        rivals = [o for o, s in windows.items()
                  if o != oid
                  and str(s["props"].get("topic") or "") == topic
                  and parse_object_id(o)[0] != owner
                  and s["props"].get("age") != "fresh"
                  and is_present(s["props"], now, knobs)]
        if not rivals:
            continue
        was = ((have.get(oid) or {}).get("props") or {}).get("topic_dupe") is True
        if p.get("age") != "fresh" and not was:
            continue
        p["topic_dupe"] = True
        if p.get("age") == "fresh":
            # The standing window takes the answer: touched, so it is a focus
            # candidate and zooms out of its tile. TOUCHED, not only `since`:
            # the floor's weights follow the last touch, and a since-tie broken
            # by id would hand the weight to the repetition instead.
            # And it is as relevant as the answer it took (OR-D-Bau-6a), if
            # that is more than its own word: the card said how much the
            # member wanted this, and the window that shows it inherits that.
            offered = relevance_of(p, now)
            for o in rivals:
                rp = windows[o]["props"]
                rp["since"] = now
                held_r = as_unit(str(rp.get("topic_relevance") or ""), 0.0)
                if offered > max(relevance_of(rp, now), held_r):
                    rp["topic_relevance"] = str(offered)
                if o not in taken:
                    taken.append(o)
    return taken


def curate(want, have, now, knobs, verdict=None, tiles=None):
    """Every window's state, age, since and score -- the screen's judgement.

    Objects in, the same objects out with the curator props written in place
    (and the judge's verdict applied first, when the pass carries one).
    Reads the bar and the weights off the root's props (the floor writes them
    when nothing else did), never off anything outside `want`/`have`.
    """
    root = want[ROOT_ID]["props"]
    # A ghost (`ghosts()`) keeps state, since and score as the display holds
    # them and stands aside: not touched, not scored, not a candidate.
    windows = {oid: spec for oid, spec in want.items()
               if spec.get("component") in WINDOWS
               and spec["props"].get("age") != "leaving"}
    touched = []
    for oid, spec in windows.items():
        prior = (have.get(oid) or {}).get("props") or {}
        p = spec["props"]
        # Only what the sender says NOW is compared: `object.update` merges
        # per key, so a prop said once and left out later stands on the
        # screen, and leaving it out is not a touch. A kept prop is the
        # browser's, never the sender's, and does not count either.
        sent = {k: v for k, v in content_props(p).items() if k not in (spec.get("keep") or [])}
        said = str(prior.get("touched") or "") != str(p.get("touched") or "")
        if p.get("pinned") is True:
            # A pinned window's own content change is no touch (D-15):
            # pinned means the tile stays, not that it asks for attention.
            # The clock rewrites its time and the weather its degrees
            # without waking the screen -- only the `touched` hint does,
            # and the rival touch of `topic_dupes` below. Arrival is no
            # touch either: `since` stays at 0, "never touched", so the
            # floor's weight does not go to a context nobody asked about.
            hit = said
        else:
            hit = oid not in have or any(prior.get(k) != v for k, v in sent.items())
        if hit:
            touched.append(oid)
            # The moment of the touch is the pass -- unless the application
            # said when (GH #689): a `touched` hint that is an epoch inside
            # the fade window, later than the last touch and not in the
            # future, IS the moment. An answer stored at 12:00:00.000 whose
            # view reaches the screen a pass after the card it caused is
            # older than that card, and the window that took the answer
            # stays the last touched one (OR-D-Bau-6, -8).
            p["since"] = now
            if said:
                moment = as_int(p.get("touched"), 0)
                floor = max(int(prior.get("since") or 0),
                            now - knobs["linger_ms"] - knobs["fade_ms"])
                if floor < moment <= now:
                    p["since"] = moment
            p["judged_relevance"] = ""
            p["judged_hidden"] = False
            p["topic_relevance"] = ""
        elif oid not in have:
            p["since"] = 0
            p["judged_relevance"] = ""
            p["judged_hidden"] = False
            p["topic_relevance"] = ""
        else:
            p["since"] = int(prior.get("since") or 0)
            # A borrowed relevance (OR-D-Bau-6a) stands as long as a verdict
            # would -- linger + fade from the rival touch, which is the
            # window's `since` -- and goes with the next real touch above.
            borrowed = str(prior.get("topic_relevance") or "")
            p["topic_relevance"] = borrowed if (
                borrowed and now - p["since"] < knobs["linger_ms"] + knobs["fade_ms"]) else ""
            # The judged props are part of the verdict and fade with it
            # (OR-C-Bau-11): once the verdict no longer stands they are
            # dropped on carry-over, exactly as a touch drops them.
            standing = verdict_stands(root, now, knobs)
            p["judged_relevance"] = str(prior.get("judged_relevance") or "") if standing else ""
            p["judged_hidden"] = standing and prior.get("judged_hidden") is True
        # A window on its way out that comes back is settled: no second entrance.
        p["age"] = "fresh" if oid not in have else "settled"
    answered = topic_dupes(windows, have, now, knobs)
    touched += [o for o in answered if o not in touched]
    if isinstance(verdict, dict):
        apply_verdict(want, windows, verdict, now, knobs)
    weights = floor_weights(root, windows, touched, now, knobs, answered)
    for oid, spec in windows.items():
        spec["props"]["score"] = score_of(spec["props"], weights, now, knobs)
    bar = floor_bar(root, windows, want, now, knobs)
    root["focus"] = bar
    root["weights"] = json.dumps(weights, sort_keys=True)
    assign_rungs(windows, want, bar, have)
    canvas_order(want, windows)
    for oid, spec in windows.items():
        # Pushed under the bar while standing on the page: hidden AND leaving
        # in the same update, so the sheet plays the leave before `display:
        # none` takes hold. The next pass finds it hidden already and settles it.
        prior = (have.get(oid) or {}).get("props") or {}
        p = spec["props"]
        if (p["state"] == "hidden" and oid in have
                and prior.get("state") != "hidden" and prior.get("age") != "leaving"):
            p["age"] = "leaving"
    # The rank AFTER the rungs: `assign_rungs` leaves `state == "urgent"` on
    # exactly the windows whose application said so, so the rank reads the
    # hint back off the rung without a second bookkeeping.
    ranks = {oid: rank_of(spec["props"], weights, now, knobs)
             for oid, spec in windows.items()}
    dock(want, windows, ranks, tiles if isinstance(tiles, dict) else {}, now, knobs)
    return want, touched


def verdict_stands(root, now, knobs):
    """A verdict rules until the situation it judged has faded: linger + fade.

    After that the floor judges again (OR-C-Bau-7): a judge that fell silent
    after its last verdict -- timeout, quota, no model -- must not leave its
    bar standing over a screen it no longer sees.
    """
    try:
        judged = int(root.get("judged_at") or 0)
    except (TypeError, ValueError):
        return False
    return judged > 0 and now - judged < knobs["linger_ms"] + knobs["fade_ms"]


def floor_weights(root, windows, touched, now, knobs, answered=()):
    """The judge's map, or the floor: the last touched context weighs 1, every other 0.5.

    The floor has no memory of its own -- the windows carry `since`, so the
    last touched context is read off them on every pass, and a context
    touched before that falls back to the default. Once a judge has spoken
    its map stands; a touch adds only a context the map does not know.
    """
    judged = verdict_stands(root, now, knobs)
    try:
        held = json.loads(str(root.get("weights") or "")) or {}
    except ValueError:
        held = {}
    if not isinstance(held, dict):
        held = {}
    if not windows:
        return {}
    if judged:
        weights = dict(held)
        for oid in touched:
            ctx = str(windows[oid]["props"].get("context") or "")
            if ctx and ctx not in weights:
                weights[ctx] = 1.0
        return weights
    # A window with no context says nothing about the weights: the last
    # touched window AMONG THOSE THAT NAME ONE decides, ties on `since`
    # broken by id. A repetition the floor holds back (`topic_dupe`) says
    # nothing either: the standing window took its answer, and the weight
    # goes with the answer, not with the copy.
    named = [o for o in windows if str(windows[o]["props"].get("context") or "")
             and windows[o]["props"].get("topic_dupe") is not True
             # A `since` of 0 is a pinned window nobody touched: it stands,
             # but it does not steer the weights.
             and int(windows[o]["props"].get("since") or 0) > 0]
    if not named:
        return {}
    # A window that took another application's answer in THIS pass is the
    # last touched one (OR-D-Bau-6b): the rival touch IS the answer to the
    # turn that touched the chat beside it. And while the answer stands
    # (`topic_relevance`), it wins a tie on `since` against the question.
    pool = [o for o in answered if o in named] or named
    last = max(pool, key=lambda o: (int(windows[o]["props"].get("since") or 0),
                                    bool(windows[o]["props"].get("topic_relevance")), o))
    # The weight of a touch fades like a verdict (OR-D-Bau-8): linger + fade
    # after the last touch every context weighs the default again, so a
    # pinned window that took an answer goes back to its tile by itself.
    if now - int(windows[last]["props"].get("since") or 0) >= knobs["linger_ms"] + knobs["fade_ms"]:
        return {}
    return {str(windows[last]["props"]["context"]): 1.0}


def floor_bar(root, windows, want, now, knobs):
    """The judge's bar while its verdict stands, else focus_default -- or 0 on an empty canvas.

    `aside` is a word the screen still accepts and draws as the canvas
    (OR-D3), so the rule that lowers the bar for an empty canvas reads EVERY
    window and not only the wide column: the empty screen has an empty canvas
    and a dock with a clock in it (S1, D-1).
    """
    if verdict_stands(root, now, knobs) and root.get("focus") not in (None, ""):
        return as_unit(root.get("focus"), knobs["focus_default"])
    scores = [s["props"]["score"] for s in windows.values()]
    return knobs["focus_default"] if any(v >= knobs["focus_default"] for v in scores) else 0.0


def assign_rungs(windows, want, bar, have):
    """hidden below the bar; every urgent window urgent; one focus; then the midpoint.

    An urgent window used to take the focus away from everything else (one
    ringing timer hid the answer somebody was reading). Now the canvas carries
    both: the urgent windows stand on top, and the focus is chosen among the
    rest (OR-D2, from D-14).

    Two words per window. The RUNG is the place on the ladder, and the tile
    and the judge read it. The STATE is the canvas's word, and the canvas
    carries only what is large (spec 2.2): focus and urgent stand on it,
    everything else is hidden there and is a tile. A window that steps down
    from the focus leaves the canvas the way a window under the bar does --
    with one `leaving` frame -- and its tile does not move.
    """
    urgent = [o for o, s in windows.items() if s["props"].get("state") == "urgent"
              and s["props"].get("topic_dupe") is not True]
    visible = [o for o, s in windows.items()
               if s["props"]["score"] >= bar and s["props"]["score"] > 0]
    candidates = [o for o in visible
                  if o not in urgent and windows[o]["props"]["age"] != "fresh"]
    # No focus under a lowered bar. `floor_bar` answers 0 when nothing on the
    # screen reaches the bar, and that gives every window a rung -- ambient,
    # relevant, a coloured tile -- but does not make the best of nothing
    # large: the empty screen is an empty canvas and a dock with a clock in
    # it (spec 2.2, S1).
    top = None if bar <= 0 else max(
        candidates, key=lambda o: (windows[o]["props"]["score"],
                                   windows[o]["props"]["since"], o), default=None)
    mid = (bar + 1.0) / 2.0
    for oid, spec in windows.items():
        p = spec["props"]
        if oid in urgent:
            rung = "urgent"
        elif oid not in visible:
            rung = "hidden"
        elif oid == top:
            rung = "focus"
        elif p["score"] >= mid:
            rung = "relevant"
        else:
            rung = "ambient"
        p["rung"] = rung
        p["state"] = rung if rung in ("focus", "urgent") else "hidden"


def canvas_order(want, windows):
    """Urgent windows above the focus, the youngest of them on top (spec 2.3).

    The order of the canvas is the `ord` of the WRAPPERS, and the seats
    (`seated`) hand out `0, 10, 20 ...` per region. An urgent window is lifted
    into a band below zero instead, so it stands over everything the seats
    ordered; when it stops being urgent the next pass gives it its seat back.
    """
    ringing = [o for o, s in windows.items() if s["props"].get("state") == "urgent"]
    ringing.sort(key=lambda o: (-int(windows[o]["props"].get("since") or 0), o))
    for i, oid in enumerate(ringing):
        wrapper = oid.split("/")[0]
        if wrapper in want:
            want[wrapper]["ord"] = -(len(ringing) - i) * ORD_STEP


def tile_id(oid):
    """The dock child of a window, derived from the window's own id.

    A window id carries slashes (`view.<slug>.<view_id>/c.a`), and a slash in
    an object id is the child chain: written as a tilde it is one segment
    again, and the tile is a child of the dock rather than of the window.
    """
    return "%s%s.%s" % (DOCK_PREFIX, TILE_KEY, oid.replace("/", "~"))


def cut(text, limit=TILE_LINE_MAX):
    """One line, at most `limit` characters, an ellipsis where it was cut."""
    text = " ".join(str(text if text is not None else "").split())
    return text if len(text) <= limit else text[:limit - 1] + "\u2026"


def fallback_tile(oid, props):
    """The tile of a window that brought none: a glyph, a line, no value (R-D1)."""
    owner, _ = parse_object_id(oid)
    glyph = (TILE_GLYPHS.get(str(props.get("context") or ""))
             or TILE_GLYPHS.get(str(props.get("class") or ""))
             or TILE_FALLBACK_GLYPH)
    line = props.get("title") or props.get("kicker") or owner_slug(owner)
    return {"glyph": glyph, "line": line, "value": "", "end_at": 0}


def tile_for(oid, props):
    """The name a tile carries for its window: the `pane_id`, else the object id.

    The window's element wears its `pane_id` as the DOM id, and the client
    matches a tile to a window by that name alone. A prose window has no
    `pane_id` and no id in the DOM; its tile names the object instead, which
    is unique and matches nothing on purpose.
    """
    return str(props.get("pane_id") or oid)


def keep_in_dock(present, windows, knobs):
    """The tiles that fit: the lowest ranks fall, pinned and urgent never do.

    `present` arrives in rank order, so the cut is a slice. The order of what
    is kept is the order it came in -- dropping a tile never reshuffles the
    ones above it.
    """
    limit = knobs.get("dock_max") or 0
    if limit <= 0 or len(present) <= limit:
        return present
    held = [o for o in present if windows[o]["props"].get("pinned") is True
            or windows[o]["props"].get("state") == "urgent"]
    rest = [o for o in present if o not in held]
    keep = set(held) | set(rest[:max(0, limit - len(held))])
    return [o for o in present if o in keep]


def dock(want, windows, ranks, tiles, now, knobs):
    """The dock object and one tile per present window, the highest rank on top.

    Presence, not the rung: a window the judge hid, or one that never reached
    the bar, has a tile as long as it stands in the table and has not faded.
    Ties go to the younger window, then to the id, so two passes over the same
    situation order the dock the same way.
    """
    # A repetition (`topic_dupe`) has no tile: it is the same statement as
    # the standing window's, and that one has the tile already.
    present = [o for o in windows
               if windows[o]["props"].get("topic_dupe") is not True
               and is_present(windows[o]["props"], now, knobs)]
    present.sort(key=lambda o: (-ranks[o], -int(windows[o]["props"].get("since") or 0), o))
    kept = keep_in_dock(present, windows, knobs)
    # What did not fit, on the ROOT: the judge is told the dock is full and
    # reads the number instead of counting the same set a second time.
    want[ROOT_ID]["props"]["dock_overflow"] = len(present) - len(kept)
    present = kept
    want[DOCK_ID] = {
        "component": "display-dock",
        "parent": ROOT_ID,
        "ord": len(REGIONS) * ORD_STEP,
        # A copy of the root's profile, so a sheet rule inside the dock's
        # slot does not have to walk back up the tree.
        "props": {"count": len(present),
                  "profile": str((want.get(ROOT_ID) or {}).get("props", {}).get("profile") or "")},
        "keep": [],
    }
    for i, oid in enumerate(present):
        p = windows[oid]["props"]
        face = fallback_tile(oid, p)
        for key in ("glyph", "line", "value", "end_at"):
            said = (tiles.get(oid) or {}).get(key)
            if said not in (None, ""):
                face[key] = said
        want[tile_id(oid)] = {
            "component": "display-tile",
            "parent": DOCK_ID,
            "ord": i * ORD_STEP,
            "props": {
                "glyph": str(face["glyph"] or TILE_FALLBACK_GLYPH),
                "line": cut(face["line"]),
                "value": str(face["value"] or ""),
                "end_at": face["end_at"] if isinstance(face["end_at"], int)
                          and not isinstance(face["end_at"], bool) else 0,
                "topic": str(p.get("topic") or ""),
                "for": tile_for(oid, p),
                # The rung, not the canvas's word: a tile is the one place
                # where ambient and relevant are still drawn.
                "state": str(p.get("rung") or p.get("state") or ""),
                "on_canvas": p.get("state") in ("focus", "urgent"),
                "pinned": p.get("pinned") is True,
                "rank": str(ranks[oid]),
            },
            "keep": [],
        }
    return want


def scale_text(scale):
    """The scale as the root wears it: `1.6`, `1.0`, `1.25` -- one value, one spelling.

    Two places write it (the root and every mirrored exit) and one sheet reads
    it, so the spelling is a function and not a format string in two hands.
    """
    text = ("%.2f" % scale).rstrip("0")
    return text + "0" if text.endswith(".") else text


def screen_profile(entry):
    """`(display_type, scale, inputs)` out of one `screens` entry.

    An entry that says nothing readable is a television at its reference
    distance: a screen with a broken setting still has to be a screen.
    """
    entry = entry if isinstance(entry, dict) else {}
    kind = str(entry.get("display_type") or "tv")
    if kind not in SCREEN_BASE:
        kind = "tv"
    try:
        distance = float(entry.get("viewing_distance_m"))
    except (TypeError, ValueError):
        distance = SCREEN_REFERENCE[kind]
    if distance <= 0:
        distance = SCREEN_REFERENCE[kind]
    scale = SCREEN_BASE[kind] * (distance / SCREEN_REFERENCE[kind])
    scale = min(SCALE_MAX, max(SCALE_MIN, scale))
    inputs = sorted({str(i) for i in (entry.get("inputs") or []) if isinstance(i, str)})
    return kind, round(scale, 2), " ".join(inputs)


def root_profile():
    """The three words the sheet reads off the root, plus the name of the exit."""
    kind, scale, inputs = screen_profile(SCREENS.get(SCREEN))
    return {"screen": SCREEN, "profile": kind, "inputs": inputs,
            "scale": scale_text(scale)}


def mirror_screens(want):
    """One copy of the whole tree per FURTHER exit, and the routes of all of them.

    The curator runs once and the tree is built once (R-D3): a second exit is
    the same objects under a prefix, with another profile at its root. Ids
    repeat across PAGES, which is what makes them the same windows -- each
    route materialises its own document, so two roots never meet in one DOM.
    """
    pages = [{"route": PAGE_ROUTE, "root": ROOT_ID, "title": PAGE_TITLE}]
    originals = sorted(want)
    for name in sorted(SCREENS):
        if name == SCREEN:
            pages.append({"route": "/" + name, "root": ROOT_ID, "title": PAGE_TITLE})
            continue
        kind, scale, inputs = screen_profile(SCREENS[name])
        for oid in originals:
            spec = want[oid]
            copy = {
                "component": spec["component"],
                "parent": None if spec["parent"] is None else name + "." + spec["parent"],
                "ord": spec["ord"],
                "props": dict(spec["props"]),
                "keep": list(spec.get("keep") or []),
            }
            if oid == ROOT_ID:
                copy["props"].update({"screen": name, "profile": kind,
                                      "inputs": inputs, "scale": scale_text(scale)})
            elif spec["component"] == "display-dock":
                copy["props"]["profile"] = kind
            want[name + "." + oid] = copy
        pages.append({"route": "/" + name, "root": name + "." + ROOT_ID, "title": PAGE_TITLE})
    return pages


def page_ops(want, have, pages, bootstrap):
    """The routes of this screen: written on the bootstrap, and again when the exits change.

    `page.set` refuses a root that does not exist yet, so these calls go LAST
    in the bundle -- after every `object.create` the patch carries.
    """
    held = (have.get(ROOT_ID) or {}).get("props") or {}
    mine = want[ROOT_ID]["props"]
    # The exits AND which of them is the default: a new default moves `/` and
    # the two names between one root and a prefixed one, with the same
    # `screens` text on both sides.
    if (not bootstrap and held.get("screens") == mine.get("screens")
            and held.get("screen") == mine.get("screen")):
        return []
    return [dict({"op": "page.set"}, **page) for page in pages]


def build(views, have=None, now=None, knobs=None, verdict=None):
    """Every object the screen should hold, keyed by id.

    `have` is what the display is holding now, and it is an INPUT to the layout
    rather than only something to diff against: it carries the seats, and the
    seats are the order of the screen (see `seated`).
    """
    have = have if isinstance(have, dict) else {}
    # The tiles the applications handed in, collected while the trees are read
    # (an empty map means every window falls back).
    tiles = {}
    want = {
        ROOT_ID: {
            "component": "display-shell",
            "parent": None,
            "ord": 0,
            "props": dict({"stylesheet": True, "faces": faces(FONT_BASE),
                           "vocab": VOCAB, "ground": GROUND,
                           # What did not fit in the dock; `dock()` overwrites
                           # it every pass, and the judge reads it.
                           "dock_overflow": 0,
                           # The screen's own motion: the scene hook, raw,
                           # rendered after the shell's element.
                           "client_js": SCENE_CLIENT_JS,
                           "screens": json.dumps(SCREENS, sort_keys=True)},
                          **root_profile()),
            "keep": [],
        }
    }
    # The screen's state lives on the root and is carried over from what the
    # display holds: the bar and the weights are the curator's memory between
    # passes, and `object.update` merges per key, so a prop left out would
    # stand for ever. `curate()` rewrites `focus` and `weights` below.
    held_root = (have.get(ROOT_ID) or {}).get("props") or {}
    for key in ("focus", "weights", "judged_at", "due", "asked_at"):
        want[ROOT_ID]["props"][key] = held_root.get(key, "")
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

    # The OS mark, behind the regions AND behind the dock, outside all of
    # them: it belongs to the screen and not to a column, its own sheet takes
    # it out of the flow, and it is a SIBLING of the dock rather than a child
    # (OR-D4) -- its `client_js` must survive every re-render of the dock.
    # Written on every tick like a region, because it is structural in the
    # same way: a screen HAS a way to speak to it.
    want[OS_ID] = {
        "component": "display-os",
        "parent": ROOT_ID,
        "ord": (len(REGIONS) + 1) * ORD_STEP,
        "props": {"mount": VOICE_MOUNT, "client_js": OS_CLIENT_JS},
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
                    # The hints, the same way: a prose view that names no
                    # context stands in its owner's, so the answer somebody
                    # just wrote weighs 1.0 and is visible.
                    "context": str(content.get("context") or owner_slug(owner)),
                    "relevance": content.get("relevance") if isinstance(
                        content.get("relevance"), (int, float)) else "",
                    "class": str(content.get("class") or ""),
                    "pinned": content.get("pinned") is True,
                    "relevant_until": content.get("relevant_until") if isinstance(
                        content.get("relevant_until"), int) else 0,
                    "touched": str(content.get("touched") or ""),
                    "topic": str(content.get("topic") or ""),
                    "modal": content.get("modal") is True,
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
            add_tree(want, wrapper, content, 0, tiles)
    ghosts(want, have)
    return curate(want, have, now or now_ms(), knobs or KNOBS, verdict, tiles)


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
    elif ROOT_ID in have:
        props = update_props(want[ROOT_ID])
        held = have[ROOT_ID]["props"]
        if any(held.get(k) != v for k, v in props.items()):
            calls.append({"op": "object.update", "id": ROOT_ID, "props": props})

    # The roots of the further exits first (`mirror_screens`): a root sorts
    # AFTER its own regions and dock (`display.region` < `display.root`), so
    # it cannot take its place in the sorted walk below. Then the rest,
    # sorted, because sorted IS parent-before-child: a region sorts before
    # every `view.` id, and a wrapper sorts before its own index chain.
    roots = sorted(k for k in want if k != ROOT_ID and want[k]["parent"] is None)
    rest = sorted(k for k in want if k != ROOT_ID and want[k]["parent"] is not None)
    for oid in roots + rest:
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
    orphans = [k for k in have if k not in want and is_ours(k)]
    # Deepest first: an exit taken out of `screens` leaves a whole tree
    # behind, root included, and a root sorts BEFORE its regions -- so the
    # order is the depth in the held tree, not the spelling of the id.
    for oid in sorted(orphans, key=lambda k: (-depth_of(k, have), k)):
        calls.append({"op": "object.delete", "id": oid})
    return calls


def depth_of(oid, have):
    """How many parents an object has on the held screen."""
    n, seen = 0, set()
    parent = (have.get(oid) or {}).get("parent")
    while parent and parent in have and parent not in seen:
        seen.add(parent)
        n += 1
        parent = have[parent].get("parent")
    return n


def is_ours(oid):
    """Whether this cell minted the id -- on this screen or on one of its exits.

    An exit's objects carry the exit's name as a prefix (`desk.view.`), and a
    screen that is taken out of `screens` leaves its whole tree behind. So the
    sweep reads the id after the first dot as well, and only accepts what this
    cell mints on either side of it.
    """
    if oid.startswith((VIEW_PREFIX, REGION_PREFIX, DOCK_PREFIX)) or oid == OLD_MIC_ID:
        return True
    rest = oid.split(".", 1)[1] if "." in oid else ""
    return (rest.startswith((VIEW_PREFIX, REGION_PREFIX, DOCK_PREFIX))
            or rest in (ROOT_ID, OS_ID, DOCK_ID, OLD_MIC_ID))


# ---------------------------------------------------------------------------
# The clock: what is due, and the one strike ordered for it (GH #679)


def iso_z(ms):
    """An epoch in milliseconds as the timer's `at`: RFC 3339, UTC, rounded UP to the second.

    The timer is exact to the second and refuses an `at` that is already past
    when the order arrives; a moment cut DOWN to its second could be, a moment
    rounded up never is.
    """
    return datetime.fromtimestamp(-(-int(ms) // 1000), timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def cross_at(props, weights, bar, now, knobs):
    """The next moment this window's fading score crosses the bar or the midpoint, or None.

    score(t) = w * r * (1 - (t - since - linger) / fade) once t is past the linger;
    solved for score == target. Pinned and urgent windows do not fade; a
    window whose rung is hidden fades only out of the DOCK -- its one moment
    is the zero crossing, when its tile goes (spec 2.2).
    """
    rung = str(props.get("rung") or props.get("state") or "")
    if props.get("pinned") is True or rung == "urgent":
        return None
    w = as_unit(weights.get(str(props.get("context") or "")), DEFAULT_WEIGHT)
    # The same relevance the score reads -- judged, else hinted, else the
    # class's, with a borrowed answer and the clamp past `relevant_until` --
    # so the clock strikes at the crossings the score will actually make.
    peak = w * relevance_of(props, now)
    if peak <= 0:
        return None
    starts = int(props.get("since") or now) + knobs["linger_ms"]
    crossings = []
    if rung != "hidden":
        for target in (bar, (bar + 1.0) / 2.0):
            if 0 < target < peak:
                crossings.append(starts + int(knobs["fade_ms"] * (1.0 - target / peak)) + 1)
    crossings.append(starts + knobs["fade_ms"] + 1)      # score reaches 0
    future = [t for t in crossings if t > now]
    return min(future) if future else None


def next_due(want, views, now, knobs):
    """The earliest moment something on the screen changes if nothing else happens."""
    root = want[ROOT_ID]["props"]
    bar = as_unit(root.get("focus"), knobs["focus_default"])
    try:
        weights = json.loads(str(root.get("weights") or "")) or {}
    except ValueError:
        weights = {}
    if not isinstance(weights, dict):
        weights = {}
    due = []
    for spec in want.values():
        if spec.get("component") not in WINDOWS:
            continue
        p = spec["props"]
        if p.get("age") in ("fresh", "leaving"):
            due.append(now + 1000)
        t = cross_at(p, weights, bar, now, knobs)
        if t:
            due.append(t)
        # The fade of the floor's weight (OR-D-Bau-8) is a moment of the
        # screen too: a pinned window whose context weighs 1 by its touch
        # steps off the canvas then, and a pinned window has no fade of its
        # own to strike for.
        since = int(p.get("since") or 0)
        if p.get("pinned") is True and since > 0:
            fade = since + knobs["linger_ms"] + knobs["fade_ms"] + 1
            if fade > now:
                due.append(fade)
        until = p.get("relevant_until")
        if isinstance(until, (int, float)) and not isinstance(until, bool) and until > now:
            due.append(int(until) + 1)
    for row in views:
        try:
            ttl = int(row.get("ttl_ms") or 0)
            written = int(row.get("updated_at") or 0)
        except (TypeError, ValueError):
            continue
        if ttl > 0:
            due.append(written + ttl + 1)
    return min(due) if due else None


def due_ops(want, have, views, now, knobs, struck=""):
    """Zero, one or two timer ops: remove the last order, add the next. Writes root.due.

    Each pass replaces the previous order, so the clock holds at most one
    schedule for this screen. The order that just struck (`struck`, the
    strike's own `schedule_id`) is gone from the timer and is not removed; a
    `remove` on any other order that already struck is answered
    `schedule_not_found`, which the hive's edge turns into `in_tick_error`
    -- expected, and ignored.

    The order's id is derived from the moment it is due (`DUE_NAMESPACE`),
    not drawn at random: two passes that run close together read the same
    stale `due`, remove the same old order and each add their own -- with a
    deterministic id the two adds are the same order, and the clock takes
    the second as the same order: acknowledged, nothing changes, one order
    results (GH #681; until #690 the second answered `schedule_id_exists`).
    The moment is the second the timer strikes at, rounded up like `iso_z`.

    And the same id is never removed and added in one pass (GH #690): a
    pass that computes the moment already standing on the root orders the
    same second again without removing it first -- a `remove` marks the
    timer's row `removed`, and an `add` of the same id right after it
    collided with that row, so the moment never struck. The clock treats a
    repeated order as one order: an `add` it already holds is acknowledged
    and changes nothing, an `add` on a removed row of the same id revives
    it (a second revisited after another moment came between). So the `add`
    is always sent, and the root's `due` is never a promise the clock does
    not hold.
    """
    old = str(((have.get(ROOT_ID) or {}).get("props") or {}).get("due") or "")
    at = next_due(want, views, now, knobs)
    ops = []
    if at is None:
        if old and old != struck:
            ops.append(emission("due", {"messages": [], "op": "remove", "schedule_id": old}))
        want[ROOT_ID]["props"]["due"] = ""
        return ops
    # The timer is exact to the second and refuses an `at` in the past, so
    # the earliest order is the next full second.
    at_ms = max(at, now + 1000)
    # The id is the SECOND the timer is told (the same rounding as `iso_z`),
    # not the millisecond: two passes a few ms apart compute two due
    # milliseconds for the same strike, and they have to be the same order.
    sec = -(-int(at_ms) // 1000)
    sid = str(uuid.uuid5(DUE_NAMESPACE, "due:%d" % sec))
    want[ROOT_ID]["props"]["due"] = sid
    if old and old not in (struck, sid):
        ops.append(emission("due", {"messages": [], "op": "remove", "schedule_id": old}))
    ops.append(emission("due", {"messages": [], "op": "add", "schedule_id": sid,
                                "schedule_name": "due", "at": iso_z(at_ms),
                                "emit_to": ".", "emit_body": {"messages": []}}))
    return ops


def pass_tick(hop):
    """A pass without a write: read the table, then let pass 2 and 3 run as usual.

    The strike's own `schedule_id` rides along as `struck`, so pass 3 does not
    ask the timer to remove an order it has already fired.
    """
    legs = [tool_call({"operation": "select", "table": TABLE, "columns": COLUMNS}, "d-select")]
    request = {"tick": True}
    if hop.get("schedule_id"):
        request["struck"] = str(hop["schedule_id"])
    return [emission("views", {"messages": legs},
                     display_request=json.dumps(request, sort_keys=True))]


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

    # The clock is the one the views pass read, so the two halves of one
    # round agree on `now`; a plan without one (an older sender) reads the
    # clock here.
    try:
        now = int(plan.get("now") or now_ms())
    except (TypeError, ValueError):
        now = now_ms()
    views = views if isinstance(views, list) else []
    verdict = plan.get("verdict") if isinstance(plan.get("verdict"), dict) else None
    want, touched = build(views, have, now, KNOBS, verdict)
    # The order for the clock is computed BEFORE the patch is cut, so the
    # root's `object.update` carries the new `due`. The judge is asked after
    # it -- and never on the pass that carries its own verdict.
    ops = due_ops(want, have, views, now, KNOBS, str(plan.get("struck") or ""))
    if verdict is None:
        ops += judge_ops(want, have, touched, now, KNOBS)
    # The exits are mirrored AFTER the clock and the judge have read the one
    # tree: a copy is a rendering of the same state, and neither the schedule
    # nor the situation may count a window twice.
    pages = mirror_screens(want)
    calls = patches(want, have, define if isinstance(define, list) else [], bootstrap)
    calls += page_ops(want, have, pages, bootstrap)
    if not calls:
        # Nothing to say. A bundle with no legs is refused as `invalid_input`
        # by the display, so silence is the only honest form of "no change".
        return ops
    return [emission("patch", {"messages": [tool_call(c, "d-%d" % i) for i, c in enumerate(calls)]})] + ops


# ---------------------------------------------------------------------------
# The judge: the situation it sees, the question, the verdict (GH #679)

JUDGE_INSTRUCTIONS = """You are the judge of one person's screen. The guideline you judge by, word for word (the screen's README, section "What the screen is for"):
Focus is the state of the whole screen, not the highlighting of one active element. It answers which information should be visible at this moment -- and which, deliberately, should not. The screen must look clearly structured, calm and relevant at every moment. Visible is only what has concrete use in the current context; everything else is hidden, reduced or moved to the back. That principle is display hygiene, and it is a continuous duty of the display system rather than a one-off design choice: every planned or executed change of the screen asks what is relevant to the member right now, what has priority, what supports the current task, what merely distracts, and what can disappear entirely without losing anything the member needs. The screen is always reduced to the minimum necessary information state.
The screen has no agenda of its own. It is not a source of information. Its content comes from applications, from the member's agents and from system states with immediate display relevance, and it shows nothing permanently only because interfaces traditionally do. A clock is an application like any other and obeys the same rules of priority, focus and visibility: in a high-focus situation it is noise and goes; on an otherwise empty screen it may stand.
Priority is dynamic. No fixed hierarchy, and the member's main agent does not automatically outrank everything -- a calendar with an imminent appointment may matter more than the agent's current output. Whoever judges takes the current context, the member's activity, time relevance, urgency, importance, running interactions, the cost of an interruption, the member's own preferences and the current focus level into account, and decides not only how something is shown but whether, when and ahead of what.
Display hygiene is personal. Members differ in what they want shown, prioritised, arranged or hidden; those preferences are learned and kept. A correction the member has to repeat is not a situational correction any more but, probably, one of that member's display rules, and it becomes part of the persistent profile that shapes later decisions.
The guiding sentence: show as little as possible at every moment -- and everything that truly matters at that moment. Relevance is not static; it arises from context, time, priority, activity and the member's preferences.
Judge the whole screen anew. Weigh: the current context, the person's current activity, time relevance, urgency, importance, running interactions, the cost of an interruption, the person's preferences (given as sentences), and the current focus level.
The dock on the right shows everything that is present; your verdict decides only what stands LARGE on the canvas. A window you hide keeps its tile, so hiding costs the member nothing but the space.
A window marked `topic_dupe` is a fresh window repeating what a standing window of ANOTHER application already says; the floor holds it back. Name it with `hidden: false` if the repetition is the better window.
A weight below 0.05 is read as 0.05 for the dock's order only, so the dock stays readable whatever you weigh to nothing.
Answer with ONE JSON object and nothing else:
{"focus": <0..1, the bar: a window is visible only when its score reaches it; high means an empty, concentrated screen>,
 "weights": {"<context>": <0..1>, ...},
 "windows": [{"id": "<object id>", "hidden": <true|false, optional>, "relevance": <0..1, optional>}]}
Windows you do not name keep their hints. A context you do not name weighs 0.5."""


def situation(want, have, touched, now, knobs):
    """What the judge sees: every window with its hints and a glimpse of its text."""
    root = want[ROOT_ID]["props"]
    try:
        weights = json.loads(str(root.get("weights") or "{}"))
    except ValueError:
        weights = {}
    windows = []
    for oid, spec in want.items():
        if spec.get("component") not in WINDOWS:
            continue
        p = spec["props"]
        glimpse = " ".join(str(p.get(k) or "") for k in ("title", "kicker", "body", "text"))[:200]
        owner, view_id = parse_object_id(oid)
        windows.append({"id": oid, "owner": owner or "", "view_id": view_id or "",
                        "region": region_of(oid, want), "context": p.get("context") or "",
                        "since": int(p.get("since") or now),
                        "relevance": p.get("relevance"), "class": p.get("class") or "",
                        "pinned": p.get("pinned") is True,
                        # The rung: the judge weighs the ladder, and
                        # `on_canvas` below says which of them are large.
                        "state": p.get("rung") or p.get("state"),
                        "age_s": max(0, (now - int(p.get("since") or now)) // 1000),
                        "touched": oid in touched,
                        # The application's own word, when it said one (GH #689).
                        "touched_at": str(p.get("touched") or ""),
                        # The dock, so the verdict knows what it is NOT deciding.
                        # Rank and canvas are read off the WINDOW: a window
                        # that fell out of a full dock still ranks and may
                        # still be the focus.
                        "topic": str(p.get("topic") or ""),
                        "topic_dupe": p.get("topic_dupe") is True,
                        # What the window borrowed from an answer it took;
                        # a verdict's `relevance` overrules it.
                        "topic_relevance": str(p.get("topic_relevance") or ""),
                        "tile": tile_id(oid) in want,
                        "rank": str(rank_of(p, weights, now, knobs)),
                        "on_canvas": p.get("state") in ("focus", "urgent"),
                        "text": glimpse.strip()})
    return {"now": now, "focus": root.get("focus"), "weights": weights,
            # Both come off the root, where `dock()` and `build()` left them:
            # the dock is counted once per pass, not twice.
            "screen": str(root.get("screen") or ""),
            "dock_overflow": int(root.get("dock_overflow") or 0),
            "preferences": ["a touched window keeps full weight for %d s, then fades over %d s"
                            % (knobs["linger_ms"] // 1000, knobs["fade_ms"] // 1000)],
            "windows": windows}


def judge_ops(want, have, touched, now, knobs):
    """One message to the judge, or none: only on a content change, only when allowed."""
    if knobs.get("judge") != "on" or not touched:
        return []
    held = (have.get(ROOT_ID) or {}).get("props") or {}
    # The brake counts from the QUESTION as well as from the answer: a judge
    # that never answers (no model, a slow provider) is still asked at most
    # once per interval.
    last = max(as_int(held.get("judged_at"), 0), as_int(held.get("asked_at"), 0))
    if last and now - last < knobs["judge_min_interval_ms"]:
        return []
    want[ROOT_ID]["props"]["asked_at"] = now
    return [{"header": {"route": "judge"},
             "system": {"instructions": {"text": JUDGE_INSTRUCTIONS}},
             "messages": [{"origin": "user", "type": "text",
                           "text": json.dumps(situation(want, have, touched, now, knobs), sort_keys=True)}]}]


def parse_model_json(text):
    """A fenced or wrapped JSON answer, tolerated (the memory hive's lesson)."""
    t = (text or "").strip()
    if t.startswith("```"):
        t = t.split("\n", 1)[1] if "\n" in t else t[3:]
        if t.rstrip().endswith("```"):
            t = t.rstrip()[:-3]
    try:
        return json.loads(t)
    except ValueError:
        a, b = t.find("{"), t.rfind("}")
        if a >= 0 and b > a:
            try:
                return json.loads(t[a:b + 1])
            except ValueError:
                return None
    return None


def pass_verdict(body, hop):
    """The judge answered: a pass without a write that carries the verdict into pass 3."""
    if str(hop.get("finish_reason") or "") != "stop":
        return []                                   # error, length, filter: the floor stands
    text = next((m.get("text") for m in body.get("messages") or []
                 if isinstance(m, dict) and m.get("type") == "text" and m.get("text")), "")
    verdict = parse_model_json(text)
    if not isinstance(verdict, dict):
        sys.stderr.write("judge: no JSON in the verdict\n")
        return []
    legs = [tool_call({"operation": "select", "table": TABLE, "columns": COLUMNS}, "d-select")]
    return [emission("views", {"messages": legs},
                     display_request=json.dumps({"tick": True, "verdict": verdict}, sort_keys=True))]


def apply_verdict(want, windows, verdict, now, knobs):
    """The judge's word on the root and on the windows it named, before the score."""
    root = want[ROOT_ID]["props"]
    root["judged_at"] = now
    root["focus"] = as_unit(verdict.get("focus"), knobs["focus_default"])
    weights = verdict.get("weights") if isinstance(verdict.get("weights"), dict) else {}
    root["weights"] = json.dumps({str(k): as_unit(v, DEFAULT_WEIGHT) for k, v in weights.items()},
                                 sort_keys=True)
    for entry in verdict.get("windows") or []:
        if not isinstance(entry, dict) or str(entry.get("id") or "") not in windows:
            continue
        p = windows[str(entry["id"])]["props"]
        if isinstance(entry.get("hidden"), bool):
            p["judged_hidden"] = entry["hidden"]
        if entry.get("hidden") is False:
            # The judge overrules the floor's duplicate mark by saying the
            # window may stand: it names the window, so it saw it.
            p["topic_dupe"] = False
        if isinstance(entry.get("relevance"), (int, float)) and not isinstance(entry.get("relevance"), bool):
            p["judged_relevance"] = str(as_unit(entry["relevance"], DEFAULT_WEIGHT))


# ---------------------------------------------------------------------------
# The dispatcher


def as_int(value, default):
    """A positive integer, or the default when it is not one."""
    try:
        n = int(value)
    except (TypeError, ValueError):
        return default
    return n if n > 0 else default


def read_knobs(params):
    """The curator's dials out of `params`, over the defaults (GH #679).

    Every knob is declared in `contract.settings` with the same
    default: they are the member's dials, and what a member wants shown
    differently is another value on that member's own screen.
    """
    KNOBS["linger_ms"] = as_int(params.get("linger_ms"), DEFAULT_LINGER_MS)
    KNOBS["fade_ms"] = as_int(params.get("fade_ms"), DEFAULT_FADE_MS)
    KNOBS["focus_default"] = as_unit(params.get("focus_default"), DEFAULT_FOCUS)
    KNOBS["judge"] = "on" if str(params.get("judge") or "") == "on" else "off"
    KNOBS["judge_min_interval_ms"] = as_int(params.get("judge_min_interval_ms"), 3000)
    global SCREENS, SCREEN
    said = params.get("screens")
    said = said if isinstance(said, dict) else {}
    SCREENS = {k: v for k, v in said.items() if is_view_id(k) and isinstance(v, dict)}
    if not SCREENS:
        SCREENS = dict(DEFAULT_SCREENS)
    name = str(params.get("default_screen") or DEFAULT_SCREEN)
    SCREEN = name if name in SCREENS else sorted(SCREENS)[0]
    KNOBS["dock_max"] = as_int(
        params.get("dock_max"),
        DOCK_MAX_BY_TYPE.get(screen_profile(SCREENS[SCREEN])[0], 7))
    global GROUND
    GROUND = "night" if str(params.get("ground") or "") == "night" else "day"
    defaults = params.get("notice_defaults")
    if isinstance(defaults, dict):
        for name, pair in defaults.items():
            if isinstance(pair, list) and len(pair) == 2:
                NOTICE_DEFAULTS[str(name)] = (as_unit(pair[0], DEFAULT_WEIGHT),
                                              as_int(pair[1], 60000))
        CLASS_RELEVANCE.clear()
        CLASS_RELEVANCE.update((k, v[0]) for k, v in NOTICE_DEFAULTS.items())


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
        read_knobs(params)
    body = doc.get("body") or {}
    envelope = doc.get("envelope") or {}
    header = envelope.get("header") or {}
    hop = header.get("hop") or {}
    ctx = header.get("context") or {}
    origin = str(ctx.get("display_origin") or "")
    route = str(hop.get("route") or "")

    # The hive's own clock and judge answer BEFORE the origin is read: the
    # question to either leaves during a read pass and carries that pass's
    # context with it, and the context comes back on the reply. The lane the
    # hive's edge stamps on the reply is the one thing that says what it is.
    if route == "in_tick":
        return pass_tick(hop)
    if route == "in_tick_error":
        # An order that was already struck cannot be removed; expected, silent.
        return []
    if route == "in_verdict":
        return pass_verdict(body, hop)

    # Pass 4 FIRST, because it is the terminating one and the cheapest to get
    # wrong. `display_origin` is stamped by the hive's own edges and travels
    # back on the reply; nothing in the body could tell these apart.
    if origin == "patch":
        return []
    if origin == "read":
        return pass_read(body, ctx)
    if origin == "views":
        return pass_views(body, ctx, hop)

    if route == "event":
        return pass_event(body)
    if route in ("in_view", "in_withdraw"):
        return pass_request(body, envelope, route == "in_withdraw")
    if route == "in_notice":
        return pass_notice(body, envelope, hop)
    return []


if __name__ == "__main__":
    out = main()
    # One emission is written as an object, several as an array, and an empty
    # list stays an empty array -- which is how a `code` cell says "nothing to
    # send" (`parse_stdout_json`: a top-level array of length 0 is zero
    # emissions).
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
