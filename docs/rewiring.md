# Rewiring a running colony

How to add, move and re-wire cells **without** stopping the colony — and which
traps are the same ones every time.

This is the operator's view of `meclaw-overview.en.md` § Mutation format: that
document says what a mutation *is*, this one says how to *drive* it. The examples
are real runs against a running colony, not sketches.

German version: `rewiring.md`.

---

## The three sentences that explain everything else

1. **A path is an address, not a name.** A cell carries its identity in its
   `cell_id` and its `cell.db`, not in its path — which is why `move_nodes` can
   change the address without touching the cell. Instantiating a new one and
   disconnecting the old gives you a copy with an empty memory instead; sometimes
   that is exactly right, and usually it is not.
2. **Edges are the wiring, nodes are only addresses.** Moving a capability means
   moving edges — the cell is the cheap part.
3. **`remove_nodes` does not delete.** It disconnects: every edge the node takes
   part in goes, the registry entry, the directory and the `cell.db` stay. One
   `add_nodes` on the same path brings it back, `cell.db` and all its rows
   included.

---

## Template: instantiate a cell and wire it in the same breath

The ordinary case. Scope is the hive the cell should live in; `name` is a
**single-segment** name inside that scope; edges are scope-relative (`./name`).

```json
{
  "scope": "/org/…/assistants/egon/cogny",
  "ctx": {},
  "diff": {
    "add_nodes": [{"name": "shell", "template": "bash-tool"}],
    "add_edges": [
      {"from": "./split", "to": "./shell",
       "condition": "has(hop.tool_name) && hop.tool_name == 'bash'"},
      {"from": "./shell", "to": "./collector",
       "modifier": {"set_hop": {"route": "'in_tool'"}}}
    ]
  }
}
```

```bash
curl -X POST http://<host>:<port>/colony/mutations \
     -H 'content-type: application/json' -d @mutation.json
# 200 {"mutation":{"id":"…","outcome":"committed"}}
```

That does **not** make the tool usable — see § "The other half".

### A deep name and its edge, in one diff (GH #166)

`add_nodes[].name` **may** be multi-segment (`"talky/fetch"` under scope `egon`),
and an edge in the same diff may address it. That is the fix in v0.14.0; before it
the instantiation worked and the edge came back with

```
422 {"error_code":"edge_schema","details":"EdgeSchema(\"to='./talky/fetch' unknown\")"}
```

because the post-state node set carried the name as written while the endpoint
check resolved a multi-segment target against absolute paths, and the two
namespaces never met. One function decides what a diff name means now, and every
check asks it — pinned in `gh166_wire_a_deep_node_in_the_same_diff`, including the
case that must keep failing: a deep endpoint that names nothing is still rejected.

**Still the better habit:** set the scope to the target hive, so `name` stays
single-segment. Not because the deep form breaks — it does not — but because the
scope is what the mutation is checked against, and a scope that names the hive you
are working in makes the check say something useful when you get it wrong.

**What a target may NOT be any more:** an address inside a sealed hive.
`"./collector/assemble"` used to be a fine target because that node exists in the
pre-state; since `collector` declares `ports: []` it is refused with
`hive_port_boundary`. Wire to `./collector` and name the lane — see
`meclaw-overview.en.md` § The hive boundary.
---

## Template: move a capability from one hive to another

From a real run: three tool cells sat in `egon/` and were reached only by
`talky/split`. They belonged in `talky/`.

This can be one mutation since GH #166 — but two is still the safer shape when
the cells are moved rather than created, because the new cells live under scope
`talky` and the old edges under scope `egon`, and one diff that spans both scopes
is one diff whose failure leaves you guessing which half applied. Where two are
used, their order is a decision, not a preference:

| Order | Window in between | Consequence |
|---|---|---|
| **add first, then disconnect** | both copies wired | one tool call fans out and runs **twice** |
| disconnect first, then add | neither copy wired | one tool call **dead-letters** |

The first was taken: a failed second mutation leaves a working colony behind, a
failed first one leaves a broken colony. The window is a second, and you do not
put it in the busy part of the day.

```jsonc
// A — scope is the TARGET hive, so the names stay single-segment
{"scope": "/org/…/egon/talky",
 "diff": {"add_nodes": [{"name": "fetch", "template": "web-fetch-tool"}, …],
          "add_edges": [{"from": "./split", "to": "./fetch", "condition": "…"},
                        {"from": "./fetch", "to": "./collector",
                         "modifier": {"set_hop": {"route": "'in_tool'"}}}, …]}}

// B — scope is the OLD hive; remove_nodes takes all their edges with them
{"scope": "/org/…/egon",
 "diff": {"remove_nodes": [{"match": {"name": "fetch"}},
                           {"match": {"name": "search"}},
                           {"match": {"name": "shell"}}]}}
```

**No extra `remove_edges`.** `remove_nodes` removes every edge the node takes part
in; listing those edges again would be a second description of one fact.

**Check the params match first.** The new cell comes from a template, the old one
runs with whatever stood there at instantiation time. Compare field by field
(instance `config.json` against template `config.json`) — otherwise the move is
also a silent retune.

---

## The other half: an edge is not a tool

An edge routes a call; it does not make the model make one. An `llm` cell offers
exactly the tools that sit in its `cell.db` as `system.tools.*` (`extract_tools`,
independent of `system_order` — tools are their own API field, not a prompt
section).

Two ways, and you need both:

**Live** — a message with no `messages` slot is a system update by definition and
triggers **no** inference:

```json
POST /messages
{"target": "/org/…/cogny/brain",
 "body": {"system": {"tools": {"bash": {"text": "{\"type\":\"function\", …}"}}}}}
```

A `202` means "submitted", not "arrived". Go and look — and note what kind of
read this is: **an operator with a shell is not a cell.** Database isolation
(`meclaw-overview.en.md` § Database isolation) binds cells, and it has no
exception: a cell never opens another cell's `cell.db`, not even to read. You may,
from outside the colony, read-only, to answer a question the API does not answer.
The moment that read wants to happen *inside* a topology, it is a message.

```bash
sqlite3 'file:…/cogny/brain/cell.db?mode=ro' \
        'select slot_path, length(value) from system order by slot_path'
```

**Durably** — the same schemas in the cell's `seed/system.jsonl`. The seed only
fires on a **freshly created** `cell.db`, so it is not a replacement for the
message but the insurance for a rebuild.

The slot is named after the tool, and the `name` inside the schema must be
**exactly** the `hop.tool_name` the dispatcher edge keys on. Otherwise the model
calls a tool with no edge behind it → dead letter, and in the chat it looks like a
model failure.

---

## New cell types: template first

`add_nodes` references templates from the `--templates` directory. A new template
becomes visible without a restart:

```bash
mkdir -p templates/file-tool         # config.json + template.json
curl -X POST http://<host>:<port>/colony/templates/rescan
# {"rescan":{"status":"ok"}}
curl -s http://<host>:<port>/colony/templates   # confirm it is there
```

`${VAR:-default}` in `params` stays **literal** in the instance `config.json` and
is resolved at spawn. A default in the template is therefore the way to offer a
value without baking an absolute host path into an exportable tree — and without a
restart for a new `.env` variable.

`bash`, `code` and `harness` instantiated from a template get the default-deny
sandbox block automatically (GH #85). It is then visible in the instance
`config.json`. Anything wider is declared in the template, not patched in
afterwards.

---

## Security surfaces you open by wiring

Connecting a tool is a decision about a capability, not about an edge. Three cases
worth **naming** rather than inheriting:

- **`bash`** — the sandbox is the whole fence (`params.sandbox`). The template
  default is `restricted` / `network: deny` / runtime file set only: the shell can
  run `date` and `uname`, it cannot write anything. A writing shell is something
  the template says out loud.
- **`file` / `edit`** — `params.base_path` is the whole fence, there is no second
  one. Two instances with different `base_path` means the model may read what it
  cannot change (or the other way round). Point them at the same directory.
- **Ingress edges** — who may talk to a colony is a `condition` on the entry edge,
  not a configuration value. A second chat platform without the same condition
  opens the same memory to everyone.

---

## Checking that it worked

```bash
curl -s http://<host>:<port>/colony/graph          # nodes and edges
curl -s 'http://<host>:<port>/colony/dead_letters?limit=10'
```

Look in the graph: did the old edge actually stop existing, or does it exist twice
now? A fan-out is the typical result of a half-finished rewiring, and in operation
it shows up only as duplicated tool results.

Check dead letters by `created_at`, do not just count them — the list is
historical, and old entries say nothing about your change.

What the graph does **not** say: which nodes are disconnected. Disconnected cells
keep appearing as nodes without edges (no-delete), on the topology surface too. A
node with no edge at all is almost always a disconnected one — but that is a
heuristic, not an answer.

---

## What happens on the surface

A newly instantiated cell has no stored position and is placed by the automatic
layout — in a hand-arranged colony it can land on top of a pinned cell (GH #167).
After a rewiring, look at the picture.

And the rule that precedes every verification: **a check does not write.**
Positions live in a `cell.db`; a verification script that simulates a drag
overwrites handwork nobody can restore. Open the store with `mode=ro`, run the
render with no move event, and if a write emission comes out of it, that is a bug
in the script.

---

## Removing a cell for real — and the second place its edges live

`remove_nodes` disconnects; it does not delete. Getting rid of the directory is an
operator action with the colony stopped:

1. `remove_nodes` (mutation) — edges leave the edge table
2. stop the colony
3. delete the registry row in `colony.db`
4. remove the directory
5. start the colony

The parent hive's `params.graph.edges` stay where they are. They are **never**
rewritten after instantiation, and that is fine: since GH #168 the **edge table**
is the topology on a reboot, for the planner too. It always was for the runtime
(`colony_task` hydrates from `colony.db` and logs "params.graph hints ignored");
only the bootstrap planner still believed the file, and died on an edge the
mutation had long since removed:

    bootstrap_from_filesystem failed: DanglingEndpoint … endpoint: Path("/…/search")
    Error: bootstrap failed

systemd restarted into a loop until someone edited the file by hand — and asking
`colony.db` about those edges was falsely reassuring, because there they really
were gone. That answer now holds at boot as well: the file is the **seed**, not
the state.

Which also gives rebuilding from the tree its meaning back: a colony rewired by
mutation and rebooted from its own directory is the colony that was running — the
removed lanes do not come back.

**And a move is no longer a rebuild** (GH #169). Since v0.14.0 there is
`move_nodes`: a node changes its address and **keeps its identity** — the same
`cell_id`, the same `cell.db` with all its rows, one `rename(2)` on disk, and the
edges swing with it. So "does this cell hold state?" is no longer a fork in the
road before a move, only the question of whether `move_nodes` applies.

It applies to a single node inside the mutation's scope. A hive and a node with
descendants are **refused explicitly** rather than done by halves, as is a target
that is already occupied and a move that leaves the scope. Pinned in
`gh169_a_move_keeps_the_cell_and_changes_its_address`, including the question that
counts: the colony boots again from what the move persisted.

The rebuild — new cell, take the edges, disconnect the old one — remains the way
for what `move_nodes` refuses, and for a move that is also meant to change the
template. For everything else it is the more expensive option, with amnesia.

---

## Putting an existing hive behind its boundary

The rule is in the overview (`meclaw-overview.en.md` § The hive boundary) — it
**binds every hive and every template**, and it also says what the conversion has
to produce: `ports: []`, doors from the inside, a `params.contract`, and lanes
named functionally. What follows is only the **order** in which a grown hive gets
there without knocking the colony over. Nine hives in one pass turned up five
things not worth guessing at.

### A topology lives in four places, not two

| Place | What it holds | Who writes it |
|---|---|---|
| `edges` in `colony.db` | the running wiring | mutations |
| `params.graph.edges` per hive `config.json` | the birth design | **never** rewritten |
| `hive_scopes` | which path is a hive at all | boot / mutation |
| `registry` | which cell sits where | boot / mutation |

Rename a hive touching the first two and forgetting `hive_scopes`, and the next
boot answers **every** edge naming the new path with `DanglingEndpoint`. The
reason is unglamorous and obvious in hindsight: **a hive that is not a scope is
not an endpoint.**

### Innermost first

The deepest hives first, then the ones above them. Both orders converge — an
edge already naming the inner hive folds on cleanly at the next step — but only
innermost-first lets each step be verified on its own.

### An inner hive's own graph is not a caller

`. -> ./assemble` inside a sealed collector resolves to two paths below the outer
boundary. That is **not** an access from outside and must not be repointed.
Matching endpoints with `str.replace` finds it anyway; resolving them against the
hive they are written in does not. Endpoints are relative (`.`, `./a/b`) — they
mean something only together with their owner.

### Some folds are rewrites

Two edges that differ only in which **child** they leave from become identical
once both ends fall onto the hive:

    ./talky -> memory/recall   set_context recall_origin: 'talky'
    ./cogny -> memory/recall   set_context recall_origin: 'cogny'

After that both fire on every recall. The discrimination has to move inside —
each out-door sets its own origin — and **one** edge leaves the hive. That is a
rewrite, not a fold, and belongs written out rather than guessed.

### An in-door is never a catch-all

Tempting, and wrong:

```json
{"from": ".", "to": "./drain",
 "condition": "!has(hop.route) || !hop.route.startsWith('in_')"}
```

because the hive's own **outbound** traffic travels through the hive path too:
every message an out-door hands to `.` would match this and land in the sink as
well. In-doors are positive lists.

### The order, short

1. write the contract: which `hop.route` in, which out — that is the whole design
   work, the rest is mechanics. Since GH #173 it is not only written down in
   prose but **declared**: `params.contract` with `accepts` and `emits`
   (`config.en.md` § `params.contract`). **The lane names are the decision
   here**: they say what a caller wants, never where it lands inside — carrying
   an old port name over as a lane completes step 6 and defeats its purpose
   (`meclaw-overview.en.md` § The three requirements)
2. stop the colony (the edge table and the files have to change together)
3. resolve colliding edges first (see "rewrites")
4. fold the boundary edges: the inner end becomes the hive
5. the doors into the edge table **and** into the hive's `params.graph`
6. declare `ports` — `[]` means "the hive path itself is the only address", and
   a finished conversion has no other value — and `contract` next to it, carrying
   the same lane list as step 1
7. repoint `params.graph` of **every other** hive that reached inside
8. on a rename, additionally: `registry`, `hive_scopes`, the directory, and the
   surface's position rows
9. start, **wait for an answer**, and roll everything back if it does not come

Step 9 is not decoration. A boot that rejects one edge does not limp, it exits —
so "the unit is active" does not answer the question. `/colony/graph` does: it is
served by the colony itself and needs the topology to have loaded.
