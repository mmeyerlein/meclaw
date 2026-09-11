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

# The two columns, as the display's OWN rule rather than a line in the token
# sheet: `vision.css` belongs to the `web` template and describes a design
# language, while "main is wide and aside is narrow" is a statement about THIS
# screen. It travels in the shell so that a display needs nothing installed
# beside it. Written with no two `{` adjacent, because `{{` is the component
# language's own marker.
LAYOUT_CSS = (
    "<style>"
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
    # stays put on a long page.
    " .display-mic { position: fixed; right: 16px; bottom: 16px; z-index: 8;"
    " display: flex; flex-direction: column; align-items: flex-end; gap: 4px;"
    " font: 13px/1.4 ui-sans-serif, system-ui, sans-serif; }"
    " .display-mic-button { padding: 10px 18px; border-radius: 999px;"
    " border: 1px solid currentColor; background: rgba(255,255,255,0.86);"
    " cursor: pointer; touch-action: none; -webkit-user-select: none;"
    " user-select: none; }"
    # The pressed state is the only feedback that the key is down, so it is a
    # real change and not a hover: an attribute the client writes.
    ' .display-mic-button[aria-pressed="true"] { background: #b3261e;'
    " color: #ffffff; }"
    " .display-mic-button:disabled { opacity: 0.5; cursor: default; }"
    " .display-mic-line { max-width: 22rem; text-align: right; }"
    " .display-mic-state { opacity: 0.6; font-size: 11.5px; }"
    "</style>"
)

SHELL_TEMPLATE = (
    '{{#if stylesheet}}<link rel="stylesheet" href="vision.css">{{/if}}'
    + LAYOUT_CSS
    + '<div class="stack display-columns">{{children}}</div>'
)

REGION_TEMPLATE = '<div class="stack" data-region="{{region}}">{{children}}</div>'

PROSE_TEMPLATE = (
    '<section class="glass--thin card" data-view="{{view_id}}"'
    ' data-owner="{{owner}}">'
    "{{#if title}}<h2 class=\"title-3\">{{title}}</h2>{{/if}}"
    '<div class="inner"><p class="text">{{body}}</p></div></section>'
)

CUSTOM_TEMPLATE = (
    '<div class="view" data-view="{{view_id}}" data-owner="{{owner}}">'
    "{{children}}</div>"
)

# The name of the `voice` cell this screen speaks to, from `params.voice_mount`.
# A module-level default, so a caller that says nothing gets the shipped name;
# the dispatcher below replaces it with what this cell was configured with, once
# per message. It is a MOUNT and not a path: the button joins `voice:<call>` on
# this page's own socket, and the `web` cell hands the frames to whichever cell
# holds that name in the process (GH #643).
VOICE_MOUNT = "voice"

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
    "      var call = \"c\" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);\n"
    "      var topic = \"voice:\" + call;\n"
    "      var chan = socket.channel(topic, { mount: mount, mode: \"hold\", sample_rate: 16000 });\n"
    "      var ctx = null, mctx = null, worklet = null, stream = null, playAt = 0;\n"
    "      function frame(obj) { chan.push(\"frame\", obj); }\n"
    "      chan.on(\"frame\", function (f) {\n"
    "        if (f.type === \"hello\") {\n"
    "          st.hello = f;\n"
    "          // A rejoin says hello again. Closing the old context first is what keeps\n"
    "          // a screen that reconnects all day from running into the browser's own\n"
    "          // cap on how many a page may have.\n"
    "          if (ctx) { try { ctx.close(); } catch (e) { /* already closed */ } }\n"
    "          ctx = new (root.AudioContext || root.webkitAudioContext)({ sampleRate: f.audio_out ? f.audio_out.sample_rate : 24000 });\n"
    "          state.textContent = f.stt + (f.tts ? \"/\" + f.tts : \"\");\n"
    "        }\n"
    "        else if (f.type === \"partial\") { line.textContent = f.text; }\n"
    "        else if (f.type === \"turn\") { st.turns++; line.textContent = f.text; }\n"
    "        else if (f.type === \"speak_start\") { playAt = 0; }\n"
    "        else if (f.type === \"speak_end\") { st.speakEnd++; }\n"
    "        else if (f.type === \"error\") { state.textContent = f.code; }\n"
    "      });\n"
    "      chan.on(\"audio\", function (buf) {\n"
    "        if (!ctx) return;\n"
    "        var pcm = new Int16Array(buf), f32 = new Float32Array(pcm.length);\n"
    "        for (var i = 0; i < pcm.length; i++) f32[i] = pcm[i] / 32768;\n"
    "        var b = ctx.createBuffer(1, f32.length, ctx.sampleRate); b.getChannelData(0).set(f32);\n"
    "        var src = ctx.createBufferSource(); src.buffer = b; src.connect(ctx.destination);\n"
    "        var t = Math.max(ctx.currentTime + 0.02, playAt); src.start(t); playAt = t + b.duration; st.played++;\n"
    "      });\n"
    "      chan.on(\"close\", function (c) { st.code = c.code; state.textContent = \"closed \" + c.code; btn.disabled = true; });\n"
    "      chan.join().receive(\"error\", function (e) { state.textContent = (e && e.reason) || \"refused\"; btn.disabled = true; });\n"
    "      function sendAudio(ab) { socket.push({ topic: topic, event: \"audio\", payload: ab, ref: \"\", join_ref: chan.joinRef() }); st.sent++; }\n"
    "      var WORKLET = \"class P extends AudioWorkletProcessor{constructor(o){super();this.rate=o.processorOptions.rate;this.acc=[];this.pos=0}process(i){var ch=i[0]&&i[0][0];if(!ch)return true;var r=sampleRate/this.rate;for(var k=0;k<ch.length;k+=r){this.acc.push(Math.max(-1,Math.min(1,ch[Math.floor(k)])))}var n=Math.floor(this.rate/50);while(this.acc.length>=n){var out=new Int16Array(n);for(var j=0;j<n;j++)out[j]=this.acc[j]*32767;this.acc=this.acc.slice(n);this.port.postMessage(out.buffer,[out.buffer])}return true}}registerProcessor('mic',P);\";\n"
    "      async function openMic() {\n"
    "        if (!root.isSecureContext) { state.textContent = \"microphone needs https or localhost\"; return false; }\n"
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
    "      var holding = false;\n"
    "      async function down() {\n"
    "        if (holding || btn.disabled) return;\n"
    "        // The playback context was built on join, which is not a gesture: under\n"
    "        // an autoplay policy it starts suspended and its clock does not run, so\n"
    "        // the gap-free scheduling would schedule against a stopped clock. This is\n"
    "        // the first gesture there is, so it is where it gets resumed.\n"
    "        if (ctx && ctx.state === \"suspended\") { try { await ctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (mctx && mctx.state === \"suspended\") { try { await mctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (!worklet && !(await openMic())) return;\n"
    "        holding = true; btn.setAttribute(\"aria-pressed\", \"true\"); frame({ type: \"hold\" });\n"
    "      }\n"
    "      function up() { if (!holding) return; holding = false; btn.setAttribute(\"aria-pressed\", \"false\"); frame({ type: \"release\" }); }\n"
    "      // A display is a surface other people's components render onto, so the\n"
    "      // window-wide key must keep its hands off their controls -- and off the\n"
    "      // button once a refusal or a close disabled it.\n"
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
    "        holding = false;\n"
    "        if (stream) { stream.getTracks().forEach(function (t) { t.stop(); }); stream = null; }\n"
    "        if (worklet) { try { worklet.port.onmessage = null; worklet.disconnect(); } catch (e) { /* gone */ } worklet = null; }\n"
    "        if (mctx) { try { mctx.close(); } catch (e) { /* gone */ } mctx = null; }\n"
    "        if (ctx) { try { ctx.close(); } catch (e) { /* gone */ } ctx = null; }\n"
    "        try { chan.leave(); } catch (e) { /* the socket may be gone already */ }\n"
    "      };\n"
    "    },\n"
    "    destroyed: function () {\n"
    "      if (this.__displayMicTeardown) { this.__displayMicTeardown(); this.__displayMicTeardown = null; }\n"
    "    }\n"
    "  };\n"
    "  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, { DisplayMic: hook });\n"
    "})(window);\n"
)


def components():
    """The display's own components, in the order they are defined.

    None of them is `editable`. A prop a browser may write is an authorisation
    an application grants over its OWN component; the frame around it is not a
    thing anybody drags.
    """
    return [
        {
            "name": "display-shell",
            "template": SHELL_TEMPLATE,
            "prop_schema": {"stylesheet": "boolean"},
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
            "props": {"stylesheet": True},
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
    if bootstrap:
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
    stale = [
        k
        for k in have
        if k not in want and (k.startswith(VIEW_PREFIX) or k.startswith(REGION_PREFIX))
    ]
    for oid in sorted(stale, reverse=True):
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
    global VOICE_MOUNT
    doc = json.load(sys.stdin)
    # The one param this cell reads. It names the `voice` cell the screen's
    # microphone joins, so one screen can be pointed at a voice cell that was
    # mounted under another name -- and a screen with no voice cell beside it
    # simply has a button whose join is refused, out loud, on the page.
    params = doc.get("params") or {}
    if isinstance(params, dict):
        VOICE_MOUNT = str(params.get("voice_mount") or "voice")
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
