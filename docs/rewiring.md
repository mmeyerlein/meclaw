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
3. **`remove_nodes` does not delete.** It disconnects: every edge naming **this
   exact path** at one end goes — the registry entry, the directory and the
   `cell.db` stay. The way back is **`add_edges`**: draw the same edges again and
   the connectivity recompute makes the node active once more, `cell.db` and all
   its rows included. An `add_nodes` on the same path is a **resume** of the same
   cell and commits — but it wires nothing, and activity is edge-derived.

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

The `diff` knows eight operations — `add_nodes`, `remove_nodes`, `swap_nodes`,
`move_nodes`, `add_edges`, `remove_edges`, `add_templates`, `seed_rows` — and
**only** those eight: a key none of them reads is `schema`, not a silent no-op. The refusal
names the key and the legal vocabulary before anything is written.

Edge endpoints are paths relative to the `scope`, at any depth — and **`.` names the
scope root itself** (GH #487). `{"from": "./telegram", "to": "."}` is therefore the lane
that leaves the level, and `{"from": ".", "to": "./talky"}` the door into it — the very
spelling a `params.graph` uses for its own level, and the one the recipes below use
throughout. Until GH #487 it was legal at boot only; `add_edges` refused it as
`edge_schema`, which forced the lane to be declared one level up and widened the
declaration's `scope` for no reason. `remove_edges` reads the same vocabulary. It is
resolved at the point of use: the diff keeps the spelling it was submitted in.

An `add_nodes` entry may optionally carry `"birth": "inactive"` (GH #437): the cell is
then wired, registered and persisted inactive **without** starting — useful for a cell
whose counterpart tolerates only one consumer. The next mutation that **addresses it
itself** wakes it — one that names its path, as an endpoint of an `add_edges`, say. The
declaration is durable (GH #491): it survives a restart, and a mutation elsewhere in the
tree never wakes the cell, however far its connectivity recompute reaches. That is the
difference from sleeping a node with `remove_edges`, which achieves the same thing from
the other side: there the node has no edge left, here it carries a marker.

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

---

## Dissolving a channel level

This is what it used to look like: one chat had a hive of its own.
`channels/channel` held the connector inside a second hive wrapped around it, and
next to it the slot the active talky generation sat in — six segments down to the
proxy cell, for a plurality that never arrived. Since GH #303 the connector is
**one** cell (the `telegram-connector` cell), the `channel` level is retired, and the
lanes it used to normalise belong to the level above it: `channels`.

What follows is the conversion of a **running** tree — three mutations and one
check. The repository ships the templates and this recipe; the run is the
operator's.

### Before and after

| | address |
|---|---|
| before | `…/assistants/<agent>/channels/channel/telegram-connector/proxy` and `…/channels/channel/terminal` |
| after | `…/assistants/<agent>/channels/<connector>` and `…/assistants/<agent>/channels/<talky>` |

Four segments instead of six, and the two occupants stand next to each other
instead of inside one another. Every lane that used to need two edges — one into
the hive, one from there into the cell — is one edge afterwards.

### 1. Put the two cells there

Scope is `channels`, the hive that already exists.

```json
{
  "scope": "/org/…/assistants/<agent>/channels",
  "ctx": {"model": "<the brain's model, as a resolved literal>"},
  "diff": {
    "add_nodes": [
      {"name": "telegram", "template": "telegram-connector@2.0.1",
       "override_params": {"bot_token": "${TELEGRAM_BOT_TOKEN}"}},
      {"name": "talky", "template": "talky@4.6.0"}
    ]
  }
}
```

`override_params` is **flat** here. On the old tree the key was
`telegram-connector/proxy`, because the template was a subtree; a single-cell
template has nothing to address (`meclaw-overview.en.md` § Mutation operations).
Compare the old instance's remaining params field by field, as with any move —
and **the token stays a `${VAR}`**. A recipe that names a value ships a secret.

**This is a new generation, not a move.** A talky is a hive with descendants, and
that is exactly what `move_nodes` refuses explicitly (§ Removing a cell for
real). The new generation starts with an empty session memory, the old one stays
disconnected and complete. If the break must not fall in the middle of a
conversation, put the cut on a closed session.

**And the token tolerates no second reader.** A second `getUpdates` consumer on
the same token gets `409 Conflict`, and the two steal each other's updates. The
window between step 1 and step 3 is therefore not the harmless one from
§ "Template: move a capability from one hive to another": the three mutations run
back to back, not spread over a coffee break, and not in the busy part of the
day.

### 2. Draw the edges

Two kinds in one diff: the pair inside the level, and the lanes the dissolved
hive used to carry outward.

```jsonc
{"scope": "/org/…/assistants/<agent>/channels",
 "diff": {"add_edges": [
   // the connector's out side -- ONE wire, sorted by two conditions
   {"from": "./telegram", "to": ".",
    "condition": "!has(hop.error_code)",
    "modifier": {"set_hop": {"route": "'turn'"},
                 "set_context": {"channel": "hop.chat_id",
                                 "chat_id": "hop.chat_id",
                                 "user_id": "hop.user_id"}}},
   {"from": "./telegram", "to": ".",
    "condition": "has(hop.error_code)",
    "modifier": {"set_hop": {"route": "'error'"}}},

   // the pair inside: the finished answer back into the chat
   {"from": "./talky", "to": "./telegram",
    "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped) && !has(hop.degraded)"},

   // inbound: ONE edge each, where two used to stand
   {"from": ".", "to": "./talky",
    "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.channel) && context.channel == <chat-id>",
    "modifier": {"set_context": {"channel_open_history": "'0'"}}},
   {"from": ".", "to": "./telegram",
    "condition": "has(hop.route) && hop.route == 'in_reply' && has(context.channel) && context.channel == <chat-id>"},
   // in_tool | in_advice | in_bundle | in_memory_call | in_thread_call |
   // in_sweep | in_prune | in_round_sweep: same shape, target ./talky
   …

   // outbound: the talky's lanes, unchanged but for the sender
   {"from": "./talky", "to": ".", "condition": "has(hop.route) && hop.route == 'write'"},
   // turn_write | extraction | recall | prune | tool | error: same shape
   …

   // a round that ran out of iterations, and a turn the store could not
   // assemble, are not answers
   {"from": "./talky", "to": ".",
    "condition": "has(hop.route) && hop.route == 'answer' && (has(hop.round_capped) || has(hop.degraded))",
    "modifier": {"set_hop": {"route": "'error'"}}}
 ]}}
```

**Those two conditions are the whole replacement for the dissolved hive.** It
normalised the connector's wire onto `turn` and `error`; the cell does not, it
sends everything on one wire and the caller sorts it: `!has(hop.error_code)` is
the turn, `has(hop.error_code)` is the failure. Draw the first edge and forget
the second and you get a colony that goes quiet exactly where somebody is waiting
for an answer — and with the hive gone there is no `required_drains` left to stop
you. **The level that holds the connector owes the `error` drain.** The same goes
for the pair the talky declares itself: `in_prune` is paired with `prune`, and a
prune ingress without a plain `prune` drain makes every operator cut dead-letter
its own answer.

**The `context.channel` condition is new, and it carries the assignment.** The
return from the shared firewall used to land in *the* hive that was the chat; now
it lands on `channels`, and which of the cells below is meant is decided by the
condition. With a single channel it may be left out; from the second one on it is
the only thing telling them apart. **The comparison is typed the way the
platform's chat id is**: numeric on Telegram, so without quotes, a string on
Slack (`meclaw-overview.en.md` § Standard header convention).

**The old edges are still standing here.** They go in step 3, and not all of them
by the same route — what hangs on that is written out there.

### 3. Disconnect the old hive

**A hive cannot be addressed with `remove_nodes`.** This is the one place where
the recipe does not look the way you would write it: `remove_nodes[].match.name`
is resolved against the **cell registry**, and a hive has no row there — it lives
in the hive scopes. A match on `./channel` is therefore `match_no_hit`, and since
validation is all-or-nothing, the **whole** mutation fails on it. (`swap_nodes`
asks both namespaces, `remove_nodes` does not; the spec promises more here than
the code delivers — GH #390.)

So: two operations in one diff. `remove_nodes` for the two real cells,
`remove_edges` for the edges whose end is a hive.

```json
{
  "scope": "/org/…/assistants/<agent>/channels",
  "diff": {
    "remove_nodes": [
      {"match": {"name": "./channel/terminal"}},
      {"match": {"name": "./channel/telegram-connector/proxy"}}
    ],
    "remove_edges": [
      {"match": {"from": ".", "to": "./channel"}},
      {"match": {"from": "./channel", "to": "."}},
      {"match": {"from": "./channel", "to": "./channel/telegram-connector"}},
      {"match": {"from": "./channel/telegram-connector", "to": "./channel"}}
    ]
  }
}
```

**Which entry takes which edge** — the channel hive's thirteen and the wrapper
hive's three, with `C` for the channel path, `T` for the statist, `W` for the
wrapper hive and `P` for the `proxy` cell:

| entry | takes | how many |
|---|---|---|
| `remove_nodes ./channel/terminal` | everything with `T` at one end: `C -> T` (three doors), `T -> C` (six lanes), `T -> W` (the answer) | 10 |
| `remove_nodes ./channel/telegram-connector/proxy` | everything with `P` at one end: `W -> P`, `P -> W` (twice, `turn` and `error`) | 3 |
| `remove_edges C -> W` | the `in_reply` door into the wrapper hive | 1 |
| `remove_edges W -> C` | both of the wrapper hive's exits | 2 |
| `remove_edges . -> ./channel` / `./channel -> .` | the lanes the instantiating mutation drew between `channels` and the hive | as many as there are |

`remove_nodes` takes the edges naming **the matched path itself** at one end —
and only those; that is why the `proxy` is on the list and not the hive above it.
A multi-segment `match.name` is allowed here: it is the same namespace decision
`add_nodes` asks. And if the slot no longer holds the `terminal` statist but a
talky generation, **its** name goes in the first line; the generation stays wired
on the inside, which is the whole point of preserving it.

**A `remove_edges` pattern without a `condition` takes every edge between the
named pair** — which is why the wrapper hive's two exits are one entry and not
two. A missing `default` leaves the **routing phase** unconstrained alongside it,
so the pattern hits regular and default edges alike. The other way round: **every pattern must hit at least
one edge in the pre-state**, or the mutation is `match_no_hit`. So read
`/colony/graph` first and write down only the pairs that are really there.

**What the cascade really is.** A `remove_nodes` does cascade over the subtree —
but in **connectivity**, not in the edge table: the recompute walks the whole
subtree, flips every node below to `active = false` and stops its task. Read
"cascade" as "every edge below it goes" and you leave edges standing that you
believe are gone.

**Disconnected, not deleted — and the two hives stay.** The two cells keep their
directory, `cell_id` and `cell.db` (no-delete policy), and `channel` and
`telegram-connector` are left behind as **empty scope markers**: no edges, no
occupants, no traffic. There is no live operation for that; getting the
directories themselves out is the colony-stopped list in § Removing a cell for
real — and it is not required. The way back is `add_edges`: draw the old edges
once more and the recompute makes the level active again. An `add_nodes` on the
same path commits as a resume, but it wires nothing.

**A rejected step leaves nothing behind.** Since GH #276 the colony registers
only behind every check that can judge the diff itself, and the two rejects that
can still fall after that roll back: the registry entry, the `colony.db` row and
an already-spawned cell are gone again before the `422` reaches the caller. A
failed step is therefore a **retry**, not a cleanup. It used to be both — the
first observed case left thirteen cells standing, including a second `proxy`
polling the same bot token as the one already running. Exactly the doubling step
1 is afraid of.

### 4. Checking it

```bash
curl -s http://<host>:<port>/colony/graph
curl -s 'http://<host>:<port>/colony/dead_letters?limit=10'
```

**The address is the first check**, and it is read off four path segments: both
occupants sit under `…/assistants/<agent>/channels/<name>` — `assistants`, the
agent, `channels`, one single-segment name. A fifth segment means the mutation's
scope sat too high and the cells landed inside the old hive.

Then three questions to the same graph:

- Does an edge still name the `channel` path **itself**, or does an edge cross
  the old subtree's boundary — one end inside, the other outside? Either means
  step 3 did not run, did not commit, or missed a path. **Edges with both ends
  below `channels/channel` stay, and they should** — they hang off inactive nodes
  and are the expected residue, not the finding. After step 3 no edge is left
  inside the old subtree; if a generation is still parked there, it is wired on
  the inside and stays that way.
- Does `./telegram -> .` appear **twice**, once per condition? Once means the
  drain is missing.
- Does a lane appear **twice** because step 2 ran twice? A fan-out shows up in
  operation only as duplicated answers.

`/colony/graph` filters by scope, not by activity: the disconnected nodes and
whatever is still wired inside a preserved generation stay visible in it — and
which node is active is something the graph does not say anyway (§ Checking that
it worked). Visibility is therefore not a failure; an edge crossing the boundary
is one. The two empty hives, by contrast, do **not** show up there at all: the
node list comes from the cell registry, where a hive has no row, and after step 3
they carry no edges either. Their evidence is the directories, not the graph.

Read dead letters by `created_at`, do not count them. And the answer that counts
does not come from the graph: write one line into the chat and wait for it to be
answered.

### What you give up

One thing, and it is the reason this is a decision rather than a tidy-up:
**which generations belong to this channel stops being structural and becomes
edge-derived.** The hive was the chat's identity — what lay inside it belonged to
it, and that stood in the path. Afterwards it stands in the edges: this talky
belongs to this chat because an edge with `context.channel == <chat-id>`
addresses it, and the previous generation belongs to it because an edge once did.
Reading a chat's history means reading edges and their conditions, not a
directory.

The generation swap itself stays what it was. `swap_nodes` swings **every
external edge** of one implementation at once (`meclaw-overview.en.md`
§ Mutation operations), and at the `channels` level this talky's edges are
external — a swap there swings every lane at the same time, the old generation
stays disconnected and complete, and the way back is the same swap in the other
direction.
