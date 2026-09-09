# Rewiring a running colony

Recipes for changing a colony while it runs: add a cell, move a capability,
seal a hive, dissolve a level. Every recipe is a mutation body plus the checks
around it.

Written for operators who post mutations by hand or from a script. Read the
three sentences below, then go to the recipe you need; the recipes stand on
their own and run from the smallest change to the largest.

The words are on [`meclaw.md`](meclaw.md), the mutation format in `meclaw-overview.md`
§ Mutation format. Every example here comes from a real run.

## The three sentences that explain everything else

1. A path is an address. A cell carries its identity in its `cell_id` and its
   `cell.db`, which is why `move_nodes` can change the address without touching
   the cell. Instantiating a new cell and disconnecting the old one gives you a
   copy with an empty memory instead. Sometimes that is what you want, and
   usually it is not.
2. Edges are the wiring, and a node is only an address. Moving a capability
   means moving edges. The cell itself is the cheap part.
3. `remove_nodes` disconnects and never deletes. Every edge naming this exact
   path at one end goes; the registry entry, the directory and the `cell.db`
   stay. The way back is `add_edges`: draw the same edges again and the
   connectivity recompute makes the node active once more, `cell.db` and all its
   rows included. An `add_nodes` on the same path is a resume of the same cell
   and commits, but it wires nothing, and activity is edge-derived.

## Instantiate a cell and wire it in one diff

The ordinary case. Scope is the hive the cell should live in, `name` is a
single-segment name inside that scope, and edges are scope-relative (`./name`).

```json
{
  "scope": "/org/…/assistants/sam/cogny",
  "ctx": {},
  "diff": {
    "add_nodes": [{"name": "fetch", "template": "fetcher"}],
    "add_edges": [
      {"from": "./split", "to": "./fetch",
       "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'"},
      {"from": "./fetch", "to": "./collector",
       "modifier": {"set_hop": {"route": "'in_tool'"}}}
    ]
  }
}
```

The `diff` knows eight operations and only those eight: `add_nodes`,
`remove_nodes`, `swap_nodes`, `move_nodes`, `add_edges`, `remove_edges`,
`add_templates`, `seed_rows`. A key that none of them reads is a `schema`
refusal, never a silent no-op, and the refusal names the key and the legal
vocabulary before anything is written.

Edge endpoints are paths relative to the `scope`, at any depth, and `.` names
the scope root itself (GH #487). `{"from": "./telegram", "to": "."}` is
therefore the lane that leaves the level, and `{"from": ".", "to": "./talky"}`
the door into it. That is the spelling a `params.graph` uses for its own level,
and the one the recipes below use throughout. `remove_edges` reads the same
vocabulary. Resolution happens at the point of use, so the diff keeps the
spelling it was submitted in.

An `add_nodes` entry may carry `"birth": "inactive"` (GH #437). The cell is then
wired, registered and persisted inactive without starting, which is useful for a
cell whose counterpart tolerates only one consumer. It wakes on the next
mutation that addresses it by name, as an endpoint of an `add_edges` for
instance. The declaration is durable (GH #491): it survives a restart, and a
mutation elsewhere in the tree leaves the cell asleep, however far its
connectivity recompute reaches. Sleeping a node with `remove_edges` arrives at
the same state from the other side. There the node has no edge left, here it
carries a marker.

```bash
curl -X POST http://<host>:<port>/colony/mutations \
     -H 'content-type: application/json' -d @mutation.json
# 200 {"mutation":{"id":"…","outcome":"committed"}}
```

That still does not make the tool usable. See § An edge is not a tool.

### A deep name and its edge in one diff

`add_nodes[].name` may be multi-segment (`"talky/fetch"` under scope `sam`), and
an edge in the same diff may address it. One function decides what a diff name
means, and every check asks it. Pinned in
`gh166_wire_a_deep_node_in_the_same_diff`, including the case that must keep
failing: a deep endpoint that names nothing is still rejected.

Setting the scope to the target hive stays the better habit, because then `name`
stays single-segment. The deep form works. The scope is what the mutation is
checked against, and a scope that names the hive you are working in makes the
check say something useful when you get it wrong.

A target may no longer be an address inside a sealed hive.
`"./collector/assemble"` names a node that exists in the pre-state, and since
`collector` declares `ports: []` it is refused with `hive_port_boundary`. Wire
to `./collector` and name the lane (`meclaw-overview.md` § The hive boundary).

## Move a capability from one hive to another

From a real run: three tool cells sat in `sam/` and were reached only by
`talky/split`. They belonged in `talky/`.

One mutation is enough for this since GH #166. Two is still the safer shape when
the cells are moved instead of created, because the new cells live under scope
`talky` and the old edges under scope `sam`, and one diff spanning both scopes
leaves you guessing which half applied when it fails. Where two are used, their
order is a decision:

| Order | Window in between | Consequence |
|---|---|---|
| add first, then disconnect | both copies wired | one tool call fans out and runs twice |
| disconnect first, then add | neither copy wired | one tool call dead-letters |

The first was taken. A failed second mutation leaves a working colony behind, a
failed first one leaves a broken colony. The window is a second, and you do not
put it in the busy part of the day.

```jsonc
// A. Scope is the TARGET hive, so the names stay single-segment
{"scope": "/org/…/sam/talky",
 "diff": {"add_nodes": [{"name": "fetch", "template": "fetcher"}, …],
          "add_edges": [{"from": "./split", "to": "./fetch", "condition": "…"},
                        {"from": "./fetch", "to": "./collector",
                         "modifier": {"set_hop": {"route": "'in_tool'"}}}, …]}}

// B. Scope is the OLD hive; remove_nodes takes all their edges with them
{"scope": "/org/…/sam",
 "diff": {"remove_nodes": [{"match": {"name": "fetch"}},
                           {"match": {"name": "search"}},
                           {"match": {"name": "shell"}}]}}
```

No extra `remove_edges`. `remove_nodes` removes every edge the node takes part
in, and listing those edges again would be a second description of one fact.

Check the params match first. The new cell comes from a template, while the old
one runs with whatever stood there at instantiation time. Compare field by
field, the instance `config.json` against the template `config.json`, or the
move is also a silent retune.

## An edge is not a tool

An edge routes a call. It does not make the model make one. An `llm` cell offers
exactly the tools that sit in its `cell.db` as `system.tools.*` (`extract_tools`,
independent of `system_order`, because tools travel as their own API field,
outside the prompt).

Two places take the schema, and you need both.

The live one is a message with no `messages` slot, which is a system update by
definition and triggers no inference:

```json
POST /messages
{"target": "/org/…/brain",
 "body": {"system": {"tools": {"bash": {"text": "{\"type\":\"function\", …}"}}}}}
```

The address is the cell itself, and that works as long as it does not stand
behind a sealed hive's boundary. Where it does — a `cogny`, a `talky`, any
template with `params.ports` — its path is no address from outside:
`<hive>/<cell>` is refused with `hive_boundary` since GH #612 rather than
delivered past the door (`meclaw-overview.md` § The hive boundary). The live
route is then the lane the hive declared for it — for `cogny` that is `in_pack`,
addressed at the hive path with `"hop": {"route": "in_pack"}` — and what has no
lane goes through the seed below.

A `202` says the message was submitted. Whether it arrived is a separate
question, so go and look. Reading it from outside the colony, read-only, is an
operator's business and not a cell's (`meclaw-overview.md` § Database
isolation). The moment that read wants to happen inside a topology, it is a
message.

```bash
sqlite3 'file:…/cogny/brain/cell.db?mode=ro' \
        'select slot_path, length(value) from system order by slot_path'
```

The durable one is the same schemas in the cell's `seed/system.jsonl`. The seed
only fires on a freshly created `cell.db`, so it is the safety net for a
rebuild. It does not replace the message.

The slot is named after the tool, and the `name` inside the schema must be
exactly the `hop.tool_name` the dispatcher edge keys on. Otherwise the model
calls a tool with no edge behind it, the call dead-letters, and in the chat it
looks like a model failure.

## A new cell type needs a template first

`add_nodes` references templates from the `--templates` directory. A new
template becomes visible without a restart:

```bash
mkdir -p templates/file-tool         # config.json + template.json
curl -X POST http://<host>:<port>/colony/templates/rescan
# {"rescan":{"status":"ok"}}
curl -s http://<host>:<port>/colony/templates   # confirm it is there
```

`${VAR:-default}` in `params` stays literal in the instance `config.json` and is
resolved at spawn. A default in the template is therefore the way to offer a
value without baking an absolute host path into an exportable tree, and without
a restart for a new `.env` variable.

`bash`, `code` and `harness` instantiated from a template get the default-deny
sandbox block automatically (GH #85), and it is then visible in the instance
`config.json`. Anything wider is declared in the template, never patched in
afterwards.

## Security surfaces you open by wiring

Connecting a tool is a decision about a capability. Three cases are worth naming
out loud instead of inheriting.

- `bash`: the sandbox is the whole fence (`params.sandbox`). The template
  default is `restricted` with `network: deny` and the runtime file set only, so
  the shell can run `date` and `uname` and cannot write anything. A writing
  shell is something the template says out loud.
- `file` and `edit`: `params.base_path` is the whole fence, and there is no
  second one. Two instances with different `base_path` mean the model may read
  what it cannot change, or the other way round. Point them at the same
  directory.
- Ingress edges: who may talk to a colony is a `condition` on the entry edge,
  and no configuration value carries it. A second chat platform without the same
  condition opens the same memory to everyone.

## Checking that it worked

```bash
curl -s http://<host>:<port>/colony/graph          # nodes and edges
curl -s 'http://<host>:<port>/colony/dead_letters?limit=10'
```

Look in the graph. Did the old edge actually stop existing, or does it exist
twice now? A fan-out is the typical result of a half-finished rewiring, and in
operation it shows up only as duplicated tool results.

Read dead letters by `created_at` instead of counting them. The list is
historical, and old entries say nothing about your change.

The graph does not say which nodes are disconnected. Disconnected cells keep
appearing as nodes without edges (no-delete), on the topology surface too. A
node with no edge at all is almost always a disconnected one. That is a
heuristic, and the graph will not confirm it.

## What happens on the surface

A newly instantiated cell has no stored position and is placed by the automatic
layout. In a hand-arranged colony it can land on top of a pinned cell (GH #167).
After a rewiring, look at the picture.

And the rule that precedes every verification: a check does not write. Positions
live in a `cell.db`, and a verification script that simulates a drag overwrites
handwork nobody can restore. Open the store with `mode=ro`, run the render with
no move event, and treat a write emission coming out of it as a bug in the
script.

## Removing a cell for real

`remove_nodes` disconnects. Getting rid of the directory is an operator action
with the colony stopped:

1. `remove_nodes` (mutation), so the edges leave the edge table
2. stop the colony
3. delete the registry row in `colony.db`
4. remove the directory
5. start the colony

The parent hive's `params.graph.edges` stay where they are. They are never
rewritten after instantiation, and that is fine: since GH #168 the edge table is
the topology on a reboot, for the planner too. It always was for the runtime
(`colony_task` hydrates from `colony.db` and logs "params.graph hints ignored").
Only the bootstrap planner still believed the file; that answer now holds at
boot as well. The file is the seed, the edge table is the state.

A move is no longer a rebuild (GH #169). `move_nodes` arrived in v0.14.0: a node
changes its address and keeps its identity, with the same `cell_id`, the same
`cell.db` with all its rows, and one `rename(2)` on disk. The edges swing with
it. So "does this cell hold state?" is no longer a fork in the road before a
move, only the question of whether `move_nodes` applies.

It applies to a single node inside the mutation's scope. A hive and a node with
descendants are refused explicitly instead of done by halves, and so are a
target that is already occupied and a move that leaves the scope. Pinned in
`gh169_a_move_keeps_the_cell_and_changes_its_address`, including the question
that counts: the colony boots again from what the move persisted.

The rebuild, meaning a new cell that takes the edges while the old one is
disconnected, remains the way for what `move_nodes` refuses, and for a move that
is also meant to change the template. For everything else it is the more
expensive option, with amnesia.

## Putting an existing hive behind its boundary

The rule is in the overview (`meclaw-overview.md` § The hive boundary). It binds
every hive and every template, and it also says what the conversion has to
produce: `ports: []`, doors from the inside, a `params.contract`, and lanes
named functionally. What follows is only the order in which a grown hive gets
there without knocking the colony over. Nine hives in one pass turned up five
things not worth guessing at.

### A topology lives in four places, not two

| Place | What it holds | Who writes it |
|---|---|---|
| `edges` in `colony.db` | the running wiring | mutations |
| `params.graph.edges` per hive `config.json` | the birth design | never rewritten |
| `hive_scopes` | which path is a hive at all | boot / mutation |
| `registry` | which cell sits where | boot / mutation |

Rename a hive touching the first two and forgetting `hive_scopes`, and the next
boot answers every edge naming the new path with `DanglingEndpoint`. The reason
is obvious in hindsight: a hive that is not a scope is not an endpoint.

### Innermost first

The deepest hives first, then the ones above them. Both orders converge, since
an edge already naming the inner hive folds on cleanly at the next step. Only
innermost-first lets each step be verified on its own.

### An inner hive's own graph is not a caller

`. -> ./assemble` inside a sealed collector resolves to two paths below the
outer boundary. That is no access from outside, and it must not be repointed.
Matching endpoints with `str.replace` finds it anyway; resolving them against
the hive they are written in does not. Endpoints are relative (`.`, `./a/b`), so
they mean something only together with their owner.

### Some folds are rewrites

Two edges that differ only in which child they leave from become identical once
both ends fall onto the hive:

    ./talky -> memory/recall   set_context recall_origin: 'talky'
    ./cogny -> memory/recall   set_context recall_origin: 'cogny'

After that both fire on every recall. The discrimination has to move inside,
where each out-door sets its own origin, and one edge leaves the hive. That is a
rewrite, and it has to be written out.

### An in-door is never a catch-all

Tempting, and wrong:

```json
{"from": ".", "to": "./drain",
 "condition": "!has(hop.route) || !hop.route.startsWith('in_')"}
```

The hive's own outbound traffic travels through the hive path too, so every
message an out-door hands to `.` would match this and land in the sink as well.
In-doors are positive lists.

### The order, short

1. write the contract: which `hop.route` in, which out. That is the whole design
   work, and the rest is mechanics. Since GH #173 it is declared as well as
   written down in prose: `params.contract` with `accepts` and `emits`
   (`config.md` § `params.contract`). The lane names are the decision here. They
   say what a caller wants, never where it lands inside; carrying an old port
   name over as a lane completes step 6 and defeats its purpose
   (`meclaw-overview.md` § The three requirements)
2. stop the colony, because the edge table and the files have to change together
3. resolve colliding edges first (see § Some folds are rewrites)
4. fold the boundary edges, so the inner end becomes the hive
5. put the doors into the edge table and into the hive's `params.graph`
6. declare `ports`: `[]` means the hive path itself is the only address, and a
   finished conversion has no other value. Put `contract` next to it, carrying
   the same lane list as step 1
7. repoint `params.graph` of every other hive that reached inside
8. on a rename, additionally: `registry`, `hive_scopes`, the directory, and the
   surface's position rows
9. start, wait for an answer, and roll everything back if it does not come

Step 9 earns its place. A boot that rejects one edge exits instead of limping,
so "the unit is active" does not answer the question. `/colony/graph` does: it
is served by the colony itself and needs the topology to have loaded.

## Dissolving a channel level

On an older tree one chat had a hive of its own. `channels/channel` held the
connector inside a second hive wrapped around it, and next to it the slot the
active talky generation sat in. That was six segments down to the proxy cell,
for a plurality that never arrived. Since GH #303 the connector is one cell (the
`telegram-connector` cell), the `channel` level is retired, and the lanes it
used to normalise belong to the level above it, `channels`.

What follows converts a running tree in three mutations and one check. The
repository ships the templates and this recipe; the run is the operator's.

### Before and after

| | address |
|---|---|
| before | `…/assistants/<agent>/channels/channel/telegram-connector/proxy` and `…/channels/channel/terminal` |
| after | `…/assistants/<agent>/channels/<connector>` and `…/assistants/<agent>/channels/<talky>` |

Four segments instead of six, and the two occupants stand next to each other
instead of inside one another. Every lane that used to need two edges, one into
the hive and one from there into the cell, is one edge afterwards.

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
      {"name": "talky", "template": "talky@5.1.0"}
    ]
  }
}
```

`override_params` is flat here. On the old tree the key was
`telegram-connector/proxy`, because the template was a subtree; a single-cell
template has nothing to address (`meclaw-overview.md` § Mutation operations).
Compare the old instance's remaining params field by field, as with any move.
And the token stays a `${VAR}`, because a recipe that names a value ships a
secret.

Treat this as a new generation. `move_nodes` cannot do it: a talky is a hive
with descendants, and that is exactly what the operation refuses explicitly
(§ Removing a cell for real). The new generation starts with an empty session
memory, the old one stays disconnected and complete. If the break must not fall
in the middle of a conversation, put the cut on a closed session.

The token also tolerates no second reader. A second `getUpdates` consumer on the
same token gets `409 Conflict`, and the two steal each other's updates. The
window between step 1 and step 3 is therefore not the harmless one from § Move a
capability from one hive to another. The three mutations run back to back, and
not in the busy part of the day.

### 2. Draw the edges

Two kinds in one diff: the pair inside the level, and the lanes the dissolved
hive used to carry outward.

```jsonc
{"scope": "/org/…/assistants/<agent>/channels",
 "diff": {"add_edges": [
   // the connector's out side: ONE wire, sorted by two conditions
   {"from": "./telegram", "to": ".",
    "condition": "!has(hop.error_code)",
    "modifier": {"set_hop": {"route": "'turn'"},
                 "set_context": {"channel": "hop.chat_id",
                                 "chat_id": "hop.chat_id",
                                 "user_id": "hop.user_id"}}},
   {"from": "./telegram", "to": ".",
    "condition": "has(hop.error_code)",
    "modifier": {"set_hop": {"route": "'error'"}}},

   // the pair inside: the finished answer back into the chat. A REAL answer
   // carries neither `round_capped` nor (since `collector@3.5.0`) `partial`:
   // both keys are written by the seam, and a real answer does not leave it
   {"from": "./talky", "to": "./telegram",
    "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped) && !has(hop.degraded)"},

   // inbound: ONE edge each, where two used to stand
   {"from": ".", "to": "./talky",
    "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.channel) && context.channel == <chat-id>",
    "modifier": {"set_context": {"channel_open_history": "'0'"}}},
   {"from": ".", "to": "./telegram",
    "condition": "has(hop.route) && hop.route == 'in_reply' && has(context.channel) && context.channel == <chat-id>"},
   // in_tool | in_advice | in_bundle | in_thread_call |
   // in_sweep | in_prune | in_round_sweep: same shape, target ./talky
   …

   // outbound: the talky's lanes, unchanged but for the sender
   {"from": "./talky", "to": ".", "condition": "has(hop.route) && hop.route == 'write'"},
   // turn_write | sidecar | recall | prune | tool | error: same shape
   …

   // a round that ran out of iterations, and a turn the store could not
   // assemble, are not answers. Since `collector@3.5.0` the first sort carries a
   // named PARTIAL ANSWER as its last turn and `hop.partial == '1'` beside it.
   // That is the key to test if you would rather let it through than drain it
   {"from": "./talky", "to": ".",
    "condition": "has(hop.route) && hop.route == 'answer' && (has(hop.round_capped) || has(hop.degraded))",
    "modifier": {"set_hop": {"route": "'error'"}}}
 ]}}
```

Those two conditions are the whole replacement for the dissolved hive. It
normalised the connector's wire onto `turn` and `error`. The cell sends
everything on one wire and the caller sorts it: `!has(hop.error_code)` is the
turn, `has(hop.error_code)` is the failure. Draw the first edge and forget the
second and you get a colony that goes quiet exactly where somebody is waiting
for an answer, and with the hive gone there is no `required_drains` left to stop
you. The level that holds the connector owes the `error` drain. The same goes
for the pair the talky declares itself: `in_prune` is paired with `prune`, and a
prune ingress without a plain `prune` drain makes every operator cut
dead-letter its own answer.

The `context.channel` condition carries the assignment. The return from the
shared firewall used to land in the one hive that was the chat; now it lands on
`channels`, and the condition decides which of the cells below is meant. With a
single channel it may be left out; from the second one on it is the only thing
telling them apart. The comparison is typed the way the platform's chat id is,
so numeric on Telegram, without quotes, and a string on Slack
(`meclaw-overview.md` § Standard header convention).

The old edges are still standing at this point. They go in step 3, and not all
of them by the same route, which is written out there.

### 3. Disconnect the old hive

A hive cannot be addressed with `remove_nodes`. This is the one place where the
recipe does not look the way you would write it: `remove_nodes[].match.name` is
resolved against the cell registry, and a hive has no row there, since it lives
in the hive scopes. A match on `./channel` is therefore `match_no_hit`, and
because validation is all-or-nothing, the whole mutation fails on it.
(`swap_nodes` asks both namespaces, `remove_nodes` does not.)

So: two operations in one diff. `remove_nodes` for the two real cells, and
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

Both operations take a `match` pattern and never a bare name or a bare edge.
Which entry takes which edge, for the channel hive's thirteen and the wrapper
hive's three, with `C` for the channel path, `T` for the statist, `W` for the
wrapper hive and `P` for the `proxy` cell:

| entry | takes | how many |
|---|---|---|
| `remove_nodes ./channel/terminal` | everything with `T` at one end: `C -> T` (three doors), `T -> C` (six lanes), `T -> W` (the answer) | 10 |
| `remove_nodes ./channel/telegram-connector/proxy` | everything with `P` at one end: `W -> P`, `P -> W` (twice, `turn` and `error`) | 3 |
| `remove_edges C -> W` | the `in_reply` door into the wrapper hive | 1 |
| `remove_edges W -> C` | both of the wrapper hive's exits | 2 |
| `remove_edges . -> ./channel` / `./channel -> .` | the lanes the instantiating mutation drew between `channels` and the hive | as many as there are |

`remove_nodes` takes the edges naming the matched path itself at one end, and
only those, which is why the `proxy` is on the list and the hive above it is
not. A multi-segment `match.name` is allowed here: it is the same namespace
decision `add_nodes` asks. And if the slot no longer holds the `terminal`
statist but a talky generation, that generation's name goes in the first line.
It stays wired on the inside, which is the whole point of preserving it.

A `remove_edges` pattern without a `condition` takes every edge between the
named pair, which is why the wrapper hive's two exits are one entry and not two.
A missing `default` leaves the routing phase unconstrained alongside it, so the
pattern hits regular and default edges alike. The other way round: every pattern
must hit at least one edge in the pre-state, or the mutation is `match_no_hit`.
So read `/colony/graph` first and write down only the pairs that are really
there.

A `remove_nodes` does cascade over the subtree, in connectivity and not in the
edge table: the recompute walks the whole subtree, flips every node below to
`active = false` and stops its task. Read "cascade" as "every edge below it
goes" and you leave edges standing that you believe are gone.

Disconnected is not deleted, and the two hives stay. The two cells keep their
directory, `cell_id` and `cell.db` (no-delete policy), and `channel` and
`telegram-connector` are left behind as empty scope markers, with no edges, no
occupants and no traffic. The colony has no live operation that removes them.
Getting the directories themselves out is the colony-stopped list in § Removing
a cell for real, and it is not required.

A rejected step leaves nothing behind. Since GH #276 the colony registers only
behind every check that can judge the diff itself, and the two rejects that can
still fall after that roll back: the registry entry, the `colony.db` row and an
already-spawned cell are gone again before the `422` reaches the caller. A
failed step is therefore something you simply retry, with nothing to clean up
first.

### 4. Checking it

```bash
curl -s http://<host>:<port>/colony/graph
curl -s 'http://<host>:<port>/colony/dead_letters?limit=10'
```

The address is the first check, and it is read off four path segments. Both
occupants sit under `…/assistants/<agent>/channels/<name>`, meaning
`assistants`, the agent, `channels`, one single-segment name. A fifth segment
means the mutation's scope sat too high and the cells landed inside the old
hive.

Then three questions to the same graph:

- Does an edge still name the `channel` path itself, or does an edge cross the
  old subtree's boundary, one end inside and the other outside? Either means
  step 3 did not run, did not commit, or missed a path. Edges with both ends
  below `channels/channel` stay, and they should: they hang off inactive nodes
  and are the expected residue. After step 3 no edge is left inside the old
  subtree; if a generation is still parked there, it is wired on the inside and
  stays that way.
- Does `./telegram -> .` appear twice, once per condition? Once means the drain
  is missing.
- Does a lane appear twice because step 2 ran twice?

`/colony/graph` filters by scope and not by activity, so the disconnected nodes
and whatever is still wired inside a preserved generation stay visible in it.
Which node is active is something the graph does not say anyway (§ Checking that
it worked). Visibility on its own is therefore fine, while an edge crossing the
boundary is the finding. The two empty hives do not show up there at all: the
node list comes from the cell registry, where a hive has no row, and after step
3 they carry no edges either. Their evidence is the directories.

The answer that counts does not come from the graph: write one line into the
chat and wait for it to be answered.

### What you give up

One thing, and it is the reason this counts as a decision instead of a tidy-up.
Which generations belong to this channel stops being structural and becomes
edge-derived. The hive was the chat's identity, what lay inside it belonged to
it, and that stood in the path. Afterwards it stands in the edges: this talky
belongs to this chat because an edge with `context.channel == <chat-id>`
addresses it, and the previous generation belongs to it because an edge once
did. Reading a chat's history means reading edges and their conditions. No
directory holds the answer any more.

The generation swap itself stays what it was. `swap_nodes` swings every external
edge of one implementation at once (`meclaw-overview.md` § Mutation operations),
and at the `channels` level this talky's edges are external. A swap there swings
every lane at the same time, the old generation stays disconnected and complete,
and the way back is the same swap in the other direction.
