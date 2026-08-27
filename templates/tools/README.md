# `tools@1.1.0`

The tool surface of one assistant as **one node with one contract**: `tool_call` in,
`tool_result` out.

That sentence is the whole template. Everything below is either a consequence of it or an
honest note about what it does not yet cover.

## The contract

| direction | lane | what it carries |
|---|---|---|
| in | `tool_call` | one call a brain decided to make. `hop.tool_name` says which tool; the body is the `tool_call` turn a dispatcher split out of the brain's bundle. |
| out | `tool_result` | what the tool answered -- **whatever the tool was**. |
| out | `build` | a structural wish, or a manifest being submitted -- the one lane on which a tool of this surface reaches OUT of the assistant. |
| in | `in_build_result` | the answer to that: a draft manifest, or the receipt of a submission. |

**One RESULT lane and no second.** A tool that failed answers on `tool_result` too: a
non-zero exit code, a 404, an empty result list are ordinary results and the round
continues. Only `hop.error_code` distinguishes a tool's own refusal, and the caller reads
that off the message rather than off the route. A second result lane would put the choice
of tool back into the caller's edge table, which is exactly the coupling this hive exists
to remove.

**`build` is not a result -- it is the REACH of this surface** (GH #425 / R6). Until the
builder intake this surface reached nowhere; now it carries a tool whose whole job is to
address `/os/builder`, four levels up, and to carry a submission there under the caller's
own identity. That fact is declared here rather than left to be discovered in an edge
table -- the same reason `sandbox_union` exists one level down, where a process radius
existed collectively and was invisible while it was spread over several files.

`params.required_drains` pairs both directions in the lane form: *a caller that sends me
`tool_call` must subscribe to `tool_result`*, and *a level that sends me `in_build_result`
must subscribe to `build`*. A mutation that wires one half without the other is refused
with `required_drain_missing` before anything is staged. There is no topology in which
handing this hive a call and not taking the answer back is the intended shape -- the tool
would run, cost whatever it costs, and answer into nothing while the brain waits for a turn
that was produced and delivered nowhere.

## Why it is sealed

`params.ports` is `[]`. The hive path is the only address, and that is the point rather
than a restriction: **the tool surface is a contract, not a set of addresses.** If a caller
could draw an edge to a single tool cell, the set of tools would live in the caller's edge
table, and every change to it would be a change to the caller. Sealed, adding a fourth tool
is one occupant directory and two internal edges -- a door and an exit; the caller's single
edge does not move.

This is also what makes the acceptance in GH #286 reachable: replacing three tool cells
with one code-executing cell is a `swap_nodes` on this node. The caller does not change,
because the contract does not change.

## The occupants

| directory | cell type | reached on | `params.sandbox` |
|---|---|---|---|
| `bash/` | `bash` | `hop.tool_name == 'bash'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `web_fetch/` | `web_fetch` | `hop.tool_name == 'web_fetch'` | none |
| `web_search/` | `web_search` | `hop.tool_name == 'web_search'` | none |
| `unknown/` | `code` | nothing else fired | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `build/` | `code` | `hop.tool_name == 'build_topology'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `apply/` | `code` | `hop.tool_name == 'apply_manifest'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |

`build` and `apply` are the two halves of one round and carry the tightest block in the
tree: they compute nothing, reach nothing and store nothing. What they do is carry -- a
wish out and a draft back, then that same draft out again and a receipt back. Both phases
of both cells are recognised POSITIVELY, and the correlation of the round travels in the
CONTEXT (`build_call_id`), because `hop` is a one-hop compartment and the builder is eight
hops away.

The three tools are each a copy of a shape the library already carries -- `bash` after the
coder pipeline's runner, `web_fetch` after the daily digest's fetcher, `web_search` after
the minimal `web_search` cell type -- so no cell type is invented here. `unknown` is not a
tool at all; it is the subject of the dispatch section below.

**The two `none` rows are the shipped truth, not an oversight.** Egress is what those two
cells are for: `network: "deny"` would turn them off, and a process sandbox has no notion
of an allowed host. They are written down as they are so that the blast radius of this hive
is a thing one can read instead of a thing one discovers. `bash` carries the block GH #286
measured on the shipped instance, verbatim.

## The declared blast radius

The table above is four separate files, and what they add up to is a fifth fact nobody was
writing down. `template.json` writes it down, in two machine-readable blocks.

**`sandbox_union` -- the widest value of every axis, over every occupant.**

| axis | union | who sets it |
|---|---|---|
| `trust` | `full` | `web_fetch`, `web_search` -- neither declares a block |
| `network` | `allow` | the same two |
| `filesystem` | `unrestricted` | the same two |

A union is not an average. `bash` and `unknown` are as restricted as the library gets, and
they tighten **nothing** about the hive as a whole: what they bound is what happens once
dispatch has already chosen them. Either unsandboxed occupant alone puts every axis at its
widest, and that is the honest reading of what handing this hive a `tool_call` can set in
motion.

That is uncomfortable to read, and it is supposed to be. The radius is not new -- GH #286
measured it on the shipped instance -- it was merely spread across four files where nobody
had to look at it whole. **The union is what a replacement is measured against: a
code-executing cell that needs exactly this is not a widening; one that needs more is.**

**`reentrancy` -- one entry per occupant, all six `true` today.**

| occupant | reentrant | why |
|---|---|---|
| `bash` | yes | one-shot, no session state, no working directory carried between calls |
| `web_fetch` | yes | stateless request/response, one GET per call |
| `web_search` | yes | stateless request/response, no cursor, no pagination state |
| `unknown` | yes | it formats a string; there is no state to reach |
| `build` | yes | it reads a hop and a turn and writes a turn; the correlation runs over the round's own context |
| `apply` | yes | the same, and the ORDER of two submissions is the colony's to decide at the door, never this cell's to assume |

`params.max_concurrency` (4 / 4 / 8) is a queue, not a serialisation: the call over the cap
waits and still answers on its own `tool_result`.

All four are stated even though the answer is four times the same word, because the hazard
this declaration exists for is a **swap** that quietly makes a parallel tool round
sequential -- and an occupant nobody declared is exactly the one whose serialisation
surprises the caller. The check runs in both directions: every occupant directory has an
entry, and every entry names a directory that exists, so an entry cannot outlive the cell
it described.

Both blocks are additive to the substrate: `parse_template_json` reads `name`, `version`,
`description` and `tags` and ignores every other top-level key, so nothing here changes how
this template boots. They exist to be **read** and to be **diffed**.

Pinned by
[`crates/meclaw-cells/tests/gh286_the_declared_sandbox_union_is_the_real_one.rs`](../../crates/meclaw-cells/tests/gh286_the_declared_sandbox_union_is_the_real_one.rs),
which recomputes the union from the occupant directories through the substrate's own
sandbox parser rather than carrying a copy of it.

## Swapping the whole hive

The acceptance GH #286 asked for, in its own terms: replacing the N tool cells behind this
contract with **one** code-executing cell is a single `swap_nodes` on this node. The caller
does not change, because the contract does not change -- one edge in on `tool_call`, one
edge back on `tool_result`, the same two lanes before and after.

What *does* change is the blast radius, and the point of the two blocks above is that the
change is not a thing to be discovered later:

* the `sandbox_union` of the replacement is either the same three values or wider, and
  **the diff of `template.json` is where that becomes visible** -- in the same review, next
  to the swap that caused it;
* its `reentrancy` block either still says `true` for what serves the calls, or it does
  not, and a caller that was fanning a tool round out across four occupants learns that
  from a declaration instead of from a latency graph.

A swap that leaves both blocks untouched is claiming the radius did not move. That claim is
now checkable, which is the whole difference between this template and the N peer cells it
replaces.

## The dispatch, and why it is in here

**Adding a tool is a change INSIDE this hive. It is never a change to the caller's
out-edges.** The caller has one edge to the hive path and keeps it, whether there are
three tools behind the contract or thirty. That sentence is the whole reason the
distribution lives here rather than in the caller's edge table, and it is what GH #286
asked to be said out loud.

Eight edges do it, and every one of them is `from` or `to` the hive path itself -- no
caller can name any of them, because `params.ports` is `[]`.

**Four doors, out of `.`.** Three ordinary conditioned edges, one per tool, each asking for
the accepted lane first and then narrowing *within* it on `hop.tool_name`:

```json
{ "from": ".", "to": "./bash",
  "condition": "has(hop.route) && hop.route == 'tool_call' && has(hop.tool_name) && hop.tool_name == 'bash'" }
```

They are mutually exclusive by construction -- three equality tests against three distinct
literals -- so a known tool name selects exactly one of them. Overlapping positives would
not be an error, they would simply stay fan-out; none is authored, and none should be.

**The lane term is not decoration.** An occupant's answer travels back out through this
same hive path, so a door that asked only about `hop.tool_name` would also be offered every
answer -- and an answer that happened to carry the name would be handed straight back to
its own sender, round after round until the TTL ran out. Asking for the lane first says
what the door is actually for: *inbound calls*, not everything that passes the hive path.

Then the fourth door, which is the one worth reading twice:

```json
{ "from": ".", "to": "./unknown", "default": true,
  "condition": "has(hop.route) && hop.route == 'tool_call'" }
```

A **default edge** (GH #283) is consulted only after every ordinary edge out of the same
sender has declined. Two consequences, and the hive's whole behaviour follows from them:

* A known tool name fires one positive edge, and the default therefore **never runs**.
  Suppression asks whether *any* regular edge decided, not whether exactly one did -- there
  is no dispatch group here and no exactly-one semantics.
* An unknown tool name -- or a `tool_call` carrying no `hop.tool_name` at all -- fires no
  positive edge, so the default is consulted, its guard holds, and the call reaches
  `./unknown`, which answers it.

**Why the guard is written even though this hive accepts one lane.** An unguarded default
is legal and boots; it earns a boot advisory and nothing more. It would also be silent
about what it consumes. The guard names the traffic out loud -- *this default is for the
`tool_call` lane* -- so a second lane added to the contract tomorrow does not silently
inherit a refusal cell that was never meant for it.

**Four exits, into `.`.** One per occupant, each stamping the hive's single outward lane:

```json
{ "from": "./bash", "to": ".",
  "condition": "has(hop.operation) && hop.operation == 'bash'",
  "modifier": { "set_hop": { "route": "'tool_result'" } } }
```

Ordinary conditioned fan-in edges, and **none of them is a default**. Default suppression
is per *sender*; these senders are the occupants, not the hive path, so they cannot
interact with the call-side default at all. The condition reads `hop.operation` because
that is what each of the three tool cells stamps on every emission it has -- success,
non-zero exit, 404, timeout alike -- and `unknown` stamps `operation: "unknown"` for the
same reason: the exit says *which occupant answered*, and the lane says *what kind of
message it is*.

**And one thing an author adding a tool should keep anyway.** The lane guard above makes it
safe, but an answer still has no business carrying the key the dispatch runs on: a cell
emission mints a fresh hop, and none of the three shipped cells writes `tool_name` into it
-- they stamp `operation` and their own outcome keys. `./unknown` is the deliberate
exception, because naming the tool that was asked for is the whole content of its refusal.
Both halves are pinned:
`the_shipped_dispatch_is_three_narrowing_doors_and_one_guarded_default` holds the doors to
the lane term, and `no_occupant_answers_with_the_key_the_doors_dispatch_on` holds the
occupants independently of it -- so shortening one condition does not silently remove both.

### `unknown/` is a cell, not a sink

The alternative to a default edge is no default edge, and then an unknown tool name is a
silent nothing: the message dead-letters as `no_route` while the brain waits for a turn
that never comes. GH #284's reasoning applies here in reverse -- this lane **has a
consumer that does something**, so it is state (1) of that ruling and a real cell. It is
not a `terminal`: it emits.

What it emits is one `tool_result` carrying `hop.error_code: "unknown_tool"` and
`hop.tool_name` set to the name that was asked for, verbatim, including the empty string
when the call carried none. The caller reads the refusal off the message, exactly as it
reads a non-zero exit code or a 404 -- which is why there is still only one outward lane.

It carries the **tightest sandbox in the hive** (`trust: restricted`, `network: deny`,
`filesystem.runtime: true`). It formats a string; it must contribute nothing to the
sandbox union this template declares.

Pinned by
[`crates/meclaw-cells/tests/gh286_one_call_reaches_exactly_one_tool.rs`](../../crates/meclaw-cells/tests/gh286_one_call_reaches_exactly_one_tool.rs),
which boots this tree in a colony: one named call reaches one occupant and the default
stays silent, and an unknown name comes back as one typed `tool_result` with no dead letter
anywhere.

## Wiring it

One edge in, one edge out, in the same mutation:

```json
[
  { "from": "./brain", "to": "./tools",
    "condition": "has(hop.route) && hop.route == 'tool'",
    "modifier": { "set_hop": { "route": "'tool_call'" } } },
  { "from": "./tools", "to": "./collector",
    "condition": "has(hop.route) && hop.route == 'tool_result'" }
]
```

The names on the caller's side are whatever that level calls its brain and its fan-in; what
is fixed is the pair of lanes and the fact that there are two edges, never one.

## The environment surface

`template.json`'s `requires.env` declares the two tokens the `web_search` occupant binds,
and says which value each one fills:

| key | binds | required |
|---|---|---|
| `SEARCH_ENDPOINT` | `web_search`'s `params.endpoint`, written `${SEARCH_ENDPOINT:-http://127.0.0.1:8080/search}` | no |
| `SEARCH_API_KEY` | `web_search`'s `params.api_key`, written `${SEARCH_API_KEY:-}` | no |

Neither is required, and that is a decision worth stating rather than leaving to be
inferred. Both tokens carry a `:-` default, so an instantiation without them succeeds --
and it has to: an assistant that wants the shell and the fetcher must not be refused over a
search endpoint it will never call. `SEARCH_API_KEY` in particular is optional in the
occupant's own contract (`contract.settings.api_key`: empty means no `Authorization`
header, and the cell queries anonymously), so requiring it here would contradict the cell it
configures.

What the declaration buys instead is that a builder learns this template's environment
surface by **reading** it. The cost is stated too: unset, `SEARCH_ENDPOINT` points the
search occupant at a loopback placeholder, and a machine with no search shim behind it
answers nothing. That is not an error and nothing anywhere reports it -- which is precisely
why the key is written down here.

Neither token carries a value in this tree and neither ever should: what is declared is the
name of a secret, never the secret.

## What does NOT live here

- **Tool schemas.** They belong in the calling brain's `system.tools`, next to the model
  that has to choose. A schema here would be a second copy of the same list, and the two
  would drift on the first tool anyone added.
- **Per-tool credentials.** A cell that needs one binds it itself, late, from the
  environment. This hive holds none and can therefore leak none.
- **Judgement.** Which tool to call, and whether it should have been called at all, was
  decided upstream. A hive that re-decided it would be a second brain with no conversation
  in front of it.
- **A memory tool.** A memory is a hive of the **member** (ADR-0002 E1) and reaching it is
  a lane of its own, drawn at the level that owns it -- not an occupant of this one.

Pinned by
[`crates/meclaw-cells/tests/gh286_the_tools_hive_has_one_door.rs`](../../crates/meclaw-cells/tests/gh286_the_tools_hive_has_one_door.rs),
which reads this template off disk: one lane in, one lane out, the seal, and the drain
pairing as the substrate's own reader collects it.
