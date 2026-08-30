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
judgement beyond an order: newest view first.

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
dropped from the picture here, sorted newest-first, ties broken on
`(owner, view_id)` so two views written in the same millisecond do not swap
places between ticks.

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
    "kind",
    "content",
    "components",
    "ttl_ms",
    "updated_at",
]

# The regions a screen has. One, in v1 -- and a closed list rather than a free
# string, because an unknown region is a view nobody would ever see, which is
# worse than a refusal the sender can read.
REGIONS = ("main",)

# The object ids, by class. Deterministic and prefixed: an id has to be
# derivable from the row without a side table, and it has to say which class it
# belongs to, because deletion sweeps by prefix.
ROOT_ID = "display.root"
REGION_PREFIX = "display.region."
VIEW_PREFIX = "view."
PAGE_ROUTE = "/"
PAGE_TITLE = "display"

# `view_id` is `[a-z0-9-]{1,64}`. Spelled as a character set rather than a
# regex so this script needs nothing but the three standard modules.
ID_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-")
ID_MAX = 64

# `ord` is a sort key, not a list index: gaps leave room to insert without
# renumbering anything the display already holds.
ORD_STEP = 10

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

SHELL_TEMPLATE = (
    '{{#if stylesheet}}<link rel="stylesheet" href="/vision.css">{{/if}}'
    '<div class="stack">{{children}}</div>'
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
            # The region exists so the root can have EXACTLY ONE child. A
            # materialised page interleaves statics and slots one for one, and a
            # root with several direct children would put the closing static in
            # the middle of the page (`web` README, "Both shipped pages give
            # their root exactly one child"). Every view hangs under the region,
            # and the region is what the root holds.
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
    kids = node.get("children", [])
    if not isinstance(kids, list):
        return '"children" is not a list'
    for kid in kids:
        why = check_node(kid, depth + 1)
        if why:
            return why
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

    return (
        {
            "owner": owner,
            "view_id": view_id,
            "region": region,
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
    # Newest first, and a tie broken on identity rather than on whatever order
    # the store happened to return: a `select` without `order_by` is explicitly
    # an unspecified selection, so the determinism has to be made here.
    live.sort(
        key=lambda r: (
            -int(r.get("updated_at") or 0),
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
    """
    oid = "%s/%d" % (parent, index)
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


def build(views):
    """Every object the screen should hold, keyed by id."""
    want = {
        ROOT_ID: {
            "component": "display-shell",
            "parent": None,
            "ord": 0,
            "props": {"stylesheet": True},
            "keep": [],
        }
    }
    # The region exists whether or not anything is in it: the root's one child
    # is a structural promise, not a consequence of there being views.
    for region in REGIONS:
        want[REGION_PREFIX + region] = {
            "component": "display-region",
            "parent": ROOT_ID,
            "ord": 0,
            "props": {"region": region},
            "keep": [],
        }

    for i, view in enumerate(views):
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
            # -- would never actually change without this. A view that has just
            # been rewritten is the newest one and belongs at the top; without a
            # move it would keep the slot it had when it was created.
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

    want = build(views if isinstance(views, list) else [])
    calls = patches(want, have, define if isinstance(define, list) else [], bootstrap)
    if not calls:
        # Nothing to say. A bundle with no legs is refused as `invalid_input`
        # by the display, so silence is the only honest form of "no change".
        return []
    return [emission("patch", {"messages": [tool_call(c, "d-%d" % i) for i, c in enumerate(calls)]})]


# ---------------------------------------------------------------------------
# The dispatcher


def main():
    doc = json.load(sys.stdin)
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
