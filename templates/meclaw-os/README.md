# `meclaw-os@1.0.2`

The colony shell: the outermost of the four composition levels, and the tree everything
else is grown into. It holds no cell of its own. It holds two occupants, one empty
container and nineteen edges, and its entire job is to be the boundary those things share.

## The rule this level was authored under

**A level owns what its siblings must share.**

All four levels — `meclaw-os`, `org`, `member`, `assistant` — repeat that sentence in the
same words, because it is the only test that decides what belongs at a level and what does
not. Ask it of anything you are tempted to add here: do *all* organisations of this colony
share it? A capability broker, yes — one broker means one answer to "may this actor do this
thing", and two would mean two. A control loop, yes — one colony, one hand on the params.
A memory, no: memory belongs to the **member** (GH #122), and a group is an audience, not a
holder. A persona, a model, a channel: no, no, no. Each of those is owned further down, and
a shell that grew one would have stopped being a boundary and become a participant.

## What is inside

| Occupant | What it is | Why it is at THIS level |
|---|---|---|
| `access` | a `ref` to `access@2.0.5` — the capability broker, with its own interior `vault` | every organisation asks the same broker; two brokers are two answers to one question |
| `steward` | a `ref` to `steward@2.0.11` — the control loop | one colony, one loop; it ships with every goal disabled |
| `orgs` | a real, empty, open container hive that declares nothing | the address an organisation is instantiated **at**; the shell declares where, not which — and declares the container's lanes for it (see below) |

Both occupants are pinned to an **exact** version. A bare name resolves to the newest
version present on disk, so a shell that named one would silently adopt a new broker the
day a bump landed — which is exactly the drift `registry.template_chain` exists to make
visible, not to excuse. Whatever the reference form, the resolved exact version is what
gets stamped into a grown node's chain, outermost first.

## Lanes

The hive path is the address; `hop.route` is the lane. `params.ports` is **absent** — the
open state — so the shell draws no boundary of its own around its occupants.

| In | Demands | Reaches |
|---|---|---|
| `in_request` | `context.requester` | the broker: may this actor do this thing |
| `in_invoke` | `context.requester` | the broker: spend a grant it already issued |
| `in_cycle` | — | the control loop: run a cycle now rather than at the next tick |
| `in_turn` | — | the `orgs` container, and through it whichever organisation stands inside |
| `in_recall` | — | a question against a member's memory, asked from outside the organisation |
| `in_brief` | — | a read against a member's curated record |
| `in_propose` | — | a write against that record: a correction, a trust decision, a subscription |

| Out | What it carries |
|---|---|
| `grant` | the broker's verdict — a handle, never a credential |
| `ack` | an outcome that was **decided**: the broker spending a grant, or an organisation acknowledging a proposed change. Two senders, one lane |
| `mutate` | the change the loop decided on, as an ordinary params update |
| `answer` | what an organisation produced for whoever asked — a turn answered, a brief read |
| `reject` | a refusal from inside an organisation that is a **verdict**, not a failure |
| `error` | a lane of the broker, of the loop or of an organisation failed and it was not a verdict. **The colony that instantiates this shell must drain it, and `reject` with it.** |

Both broker lanes demand a promoted `context.requester`, and the shell demands it too. A
level forwards a requirement; it cannot satisfy one. An edge that carries `in_request` into
this shell without having promoted the requester somewhere upstream is refused with
`hive_contract` before anything is staged — a grant issued to whoever asked loudest is the
one failure the broker cannot recover from afterwards.

## The nineteen edges

Every one of them is a door or an exit, and every declared lane has exactly one. Nothing
inside the shell talks to anything else inside it, because the occupants are strangers:
the broker knows nothing about the loop, the loop asks the colony rather than the broker,
and neither of them knows an organisation exists.

```json
{"add_edges": [
  {"from": "<shell>",           "to": "<shell>/access",  "condition": "has(hop.route) && hop.route == 'in_request'"},
  {"from": "<shell>",           "to": "<shell>/access",  "condition": "has(hop.route) && hop.route == 'in_invoke'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'grant'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'ack'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>",           "to": "<shell>/steward", "condition": "has(hop.route) && hop.route == 'in_cycle'"},
  {"from": "<shell>/steward",   "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'mutate'"},
  {"from": "<shell>/steward",   "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_turn'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_recall'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_brief'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_propose'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'answer'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'ack'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'reject'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'write'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'turn_write'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'prune'"}
]}
```

Those nineteen travel with the template; they are shown here so a reader can see the shape,
not so anyone has to draw them. The last three arrived when the level below them started
re-emitting what a member's assistants raise and nothing at member level consumes — nobody
re-derived the list by hand, `gh302_meclaw_os_shell.rs` read `templates/org/config.json`
off the tree and went red until this level moved with it.

## The container

`orgs` is a **real** hive with no children, no edges and **no `params` block at all**.

- **Real**, because #303 forces the same shape one level down — `channels` has to be a node
  to carry its fan-in edges once — and four levels with one shape beat four levels with
  three. The live tree already grew this way.
- **Empty**, because an organisation is instantiated *into* it. It ships as an address, not
  as a tenant.
- **Open** — `params.ports` absent, not `[]` — because the mutation that instantiates an
  organisation draws edges to that organisation, and a sealed hive refuses exactly those
  endpoints with `hive_port_boundary`.
- **Silent**, and that is the part worth explaining.

### Why the container declares nothing and the level declares everything

The obvious shape is the wrong one: give the container its own `params.contract` naming the
transit lanes, so a reader can wire against it. That declaration is a trap, and it is a trap
that hides itself.

A hive contract is checked inward: every accepted lane must route from the hive path **to a
cell inside it**, every emitted lane must route from inside back out. The container is empty
by construction, so it has no inside — no lane it declared could ever have a door. The check
does not fire while nothing addresses the hive (`hive_path_is_wired` treats an unaddressed
contract as dormant), which means such a declaration ships **green**. It stops being green
the moment somebody draws the first edge to the container — an operator, an example, the
mutation that instantiates the first organisation — and from that moment **every** mutation
of the colony is refused with `hive_contract` until an organisation actually stands inside.
A declaration that is green only because nobody is looking is the same defect class as the
slot this wave removed from all four levels, and it is caught the same way: by asking what
the code does rather than what the field is named.

So the rule, and all four levels follow it: **the level declares the transit lanes, and the
level's own edges satisfy them from birth.** `in_turn` has a door because the shell routes
it into `./orgs`, and `orgs` lies inside the shell; `answer`, `ack`, `reject`, `error`,
`write`, `turn_write` and `prune` have exits because the shell routes them back out of
`./orgs`. Below the container there is nothing to route to
until an organisation is instantiated, and the mutation that grows one draws its own edges —
but the level's promise is already true and already checkable on the day it ships.

## What is deliberately not here

**No second vault.** GH #302's original sketch listed `vault` beside `access` at this level.
It is not here, and that is a ruling (Q20), not an omission: `access@2.0.5` already carries
its own interior `vault`, reachable from nowhere outside, and the standalone `vault`
template attests its inbound edges against `params.broker` and `params.sealed_neighbors`.
With no broker at this level, a second one would boot locked and inert — a credential store
that answers nobody, which reads like a working component right up to the moment somebody
needs it.

**No swallowing sink.** No `terminal`, and no refusal lane that ends inside the shell. The
dead-letter queue is the record (GH #284, ruling Q2). Both `error` exits leave the level;
what happens to them is the colony's decision, made where the colony is wired.

**No `turn` lane outward, and that is the union rule doing its job.** A level declares the
union of what its occupants accept and emit, **minus the lanes a sibling inside the level
consumes itself** — explicitly written out, with the versions each entry was derived from in
its `because`. `turn` is the one subtraction on this level: a member consumes its own turn
behind its screen (`./assistants -> ./firewall`), so nothing ever reaches an organisation's
boundary on that lane, and an exit for it here could only ever have been an edge that never
fires. What does come back out is `answer`, `ack`, `reject` and `error`. Two of those,
`ack` and `error`, have **two senders and one declaration**: a caller subscribes to a lane,
not to whoever raised it.

**No `connect` lane outward.** The broker emits one, and this shell does not pass it on.
`access` requires `connect` to be the *only* edge that reaches a connector cell, and a
connector lives four levels down — so that edge is drawn where the connector stands, in the
same mutation that instantiates it. Re-emitting the lane here would offer a second way in
and quietly end the broker's monopoly, which the substrate has no permission layer to
restore.

**The unbound behaviour of `orgs` is undeclared, and that is measured rather than
overlooked.** The substrate's slot declaration governs an address that does **not** exist;
a container hive that exists but has no children counts as *bound*, so a declared `unbound`
word could never fire for it — and writing `params.ports` for the slot's sake would have
**sealed** this shell, turning a silent declaration into a harmful one. So a message that
reaches `orgs` before an organisation is instantiated into it takes the ordinary path. The
full finding was read out of the slot resolution itself:
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps aside as soon as the target is a registered hive scope.

**This shell's steward talks to the colony directly.** Since GH #267 the loop's meter and
probe ask `/colony/ledger` for their numbers instead of opening a database they do not own
— `STEWARD_COLONY_DB` was retracted with that change. The two absolute lanes that carry
those asks travel with the `ref`, so this level writes no edge for them; it also must not
seal them away, which is one more reason `params.ports` stays absent here. The virtual
endpoints are in bounds at every scope, so wrapping the loop one level deeper changes
nothing about them. Their two settings, `STEWARD_MAX_LEDGER_ROWS` and
`STEWARD_PROBE_LEDGER_TRIES`, both carry defaults inside the loop's own cells and need no
pass-through from this level; an operator who wants other values sets them in the
environment the colony runs in.

**No `requires` block.** This template substitutes nothing in its own config values, so it
declares nothing. The requirements of `access` and `steward` are **not** repeated here:
W3's validator resolves each node's ref chain back to the template it came from and
collects that template's own declaration, so a requirement of a referenced template is
already a requirement of this composite.

## Growing a colony out of it

```json
{"scope": "/",
 "diff": {"add_nodes": [{"name": "os", "template": "meclaw-os@1.0.2"}],
          "add_edges": []}}
```

One node and no edges at all: the shell is the outermost boundary, so what reaches it comes
from outside the colony and what leaves it leaves the colony. The broker, the control loop,
the `orgs` container and the nineteen edges between them came with the template.

Then one organisation at a time, into the container, each with its transit edges in the
**same** mutation — a hive is an island until an edge crosses into it:

```json
{"scope": "/os",
 "diff": {"add_nodes": [{"name": "orgs/acme", "template": "org@1.0.1"}],
          "add_edges": [{"from": "./orgs", "to": "./orgs/acme",
                         "condition": "has(hop.route) && hop.route == 'in_turn'"},
                        {"from": "./orgs/acme", "to": "./orgs",
                         "condition": "has(hop.route) && hop.route == 'answer'"}]}}
```

**The spelling is worth copying exactly.** A node is addressed by its `name` plus the
mutation's `scope`, and the scope is the level *above* the container — the node's `name`
carries the `/`. Writing `"scope": "/os/orgs"` and then `"from": "."` does not work: an
`add_edges` endpoint is resolved as a node name, and `.` names none, so the whole diff is
rejected with `edge_schema`. An **absolute** endpoint does not work either — `"from":
"/os/orgs/acme"` is refused with `scope_out_of_bounds` before anything else is looked at,
whatever the scope. Endpoints are scope-relative, always.

Eleven such edges carry an organisation (four lanes down, seven back up), and
[`examples/organism`](../../examples/organism/) is the whole stack written out that way:
five declarations, one per level.

The broker starts inert by design — every seeded policy row ships disabled — and so does
the control loop: every charter goal ships disabled too. A fresh shell therefore grants
nothing and changes nothing until an operator turns on exactly what they mean.
