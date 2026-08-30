"""The topology snapshot: ask the colony's graph endpoint, hand the answer on.

THIS FILE IS THE SOURCE. `config.json` carries a byte-identical copy in
`script_inline`, and a drift lock compares the two. Edit here.

# Why this is a separate cell on a timer

A reply from a `/colony/*` endpoint is built as a **fresh** message: new trace,
no `parent_message_id`, no `correlation_id`, and **no `context`**
(`crates/meclaw-colony/src/colony_dispatch.rs`, `emit_reply_or_done`). So that
leg can never be part of somebody's request: the answer would come back carrying
nothing that says who asked, and a cell has no way to supply the pairing
afterwards. An on-disk pointer would only be sound under a lease that guarantees
one request at a time, which is not a promise anything here makes.

There is no request path in this hive for it to be part of either, and that is
the deeper reason. An app produces a VIEW: it says what the picture is, on a
timer, and a display holds it and serves it. Nobody is waiting on this cell when
it runs, so nothing has to be correlated back to anybody. A colony's graph
changes on mutation, not on mouse movement, which makes a minute an honest
interval rather than a compromise.

# Two passes

Pass 1 (the timer ticked, or somebody asked on `in_refresh`): emit one read to
the colony's graph endpoint. Nothing else.

Pass 2 (the colony answered): hand the graph to the layout on the `snapshot`
lane, unread. What a picture needs out of it is the layout's business, and this
cell deliberately knows none of it -- it is the lane to the colony's own
read-only topology endpoint and nothing more.

There is no third pass. The layout never answers this cell, so there is no
acknowledgement to recognise: the loop that has to be guarded against elsewhere
([#161](https://github.com/mmeyerlein/meclaw/issues/161), one tick becoming two
becoming four) cannot form here, because the reply flows on to the display
instead of back.
"""
import json
import sys


def main():
    doc = json.load(sys.stdin)
    body = doc.get("body") or {}

    # ---- pass 2: the colony's graph reply nests its answer under a `graph` slot
    # -- verified against a live reply, because the first version of this cell
    # looked for `nodes` at the top level and therefore re-asked on every tick
    # without ever producing a snapshot.
    #
    # A body check and not a header check, and that is forced: a `/colony/*` reply
    # arrives on a fresh envelope with no `context` and no `hop` of ours, so there
    # is nothing else to look at.
    reply = body.get("graph")
    if isinstance(reply, dict) and isinstance(reply.get("nodes"), list):
        return [{
            "header": {"route": "snapshot"},
            "messages": [],
            "graph": {
                "scope": reply.get("scope") or "/",
                "nodes": reply.get("nodes") or [],
                "edges": reply.get("edges") or [],
            },
        }]

    # ---- pass 1: ask.
    #
    # The target is NOT set here and cannot be: a `code` cell's stdout is a body,
    # and where it goes is the edge's decision. The lane to the graph endpoint
    # is declared by this hive's OWN config.json and travels with the template
    # (GH #163). That endpoint is one of the absolute endpoints a mutation may
    # draw: it is not a cell but the colony's read-only topology endpoint, and it
    # is the sanctioned way to learn topology -- § Database isolation forbids
    # reading `colony.db`, so refusing the lane would only have pushed somebody
    # there. `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` enumerates the drawable
    # endpoints by name; the counts-never-content `/colony/ledger` joined it with
    # GH #267.
    #
    # `reply_to` is not set here either, and also cannot be: the substrate stamps
    # every cell emission with the emitting cell's own path, so the colony's answer
    # comes back to this cell without anybody asking for it.
    return [{"header": {"route": "ask_colony"}, "messages": [], "query": {"scope": "/"}}]


if __name__ == "__main__":
    out = main()
    # A single emission is written as an object, several as an array -- and an
    # empty list stays an empty array, which is how a `code` cell says "nothing to
    # send" (`parse_stdout_json`: a top-level array of length 0 is zero emissions).
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
